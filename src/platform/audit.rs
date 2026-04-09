use std::{
    collections::VecDeque,
    fs,
    future::Future,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::{
    process::Command,
    sync::{
        mpsc::{UnboundedReceiver, UnboundedSender, error::TryRecvError, unbounded_channel},
        oneshot,
    },
    task::{JoinError, JoinHandle},
    time::{Instant, MissedTickBehavior, Sleep, interval_at, sleep_until, timeout},
};

use crate::raw_types::JsonlAppender;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VenueHealthState {
    Healthy,
    Disabled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SelectorState {
    Active,
    Freeze,
    Idle,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TradeEventKind {
    Submitted,
    Accepted,
    Filled,
    Canceled,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditRecord {
    ReferenceSnapshot {
        ts_ms: u64,
        topic: String,
        fair_value: Option<f64>,
        confidence: f64,
    },
    VenueStatus {
        ts_ms: u64,
        venue_name: String,
        status: VenueHealthState,
        reason: Option<String>,
    },
    SelectorDecision {
        ts_ms: u64,
        ruleset_id: String,
        state: SelectorState,
        market_id: Option<String>,
        instrument_id: Option<String>,
        reason: Option<String>,
    },
    TradeHistory {
        ts_ms: u64,
        strategy_id: String,
        instrument_id: String,
        client_order_id: String,
        event: TradeEventKind,
        pnl_delta: Option<f64>,
    },
    PnlSnapshot {
        ts_ms: u64,
        strategy_id: String,
        realized_pnl: f64,
        unrealized_pnl: Option<f64>,
    },
}

impl AuditRecord {
    fn ts_ms(&self) -> u64 {
        match self {
            Self::ReferenceSnapshot { ts_ms, .. }
            | Self::VenueStatus { ts_ms, .. }
            | Self::SelectorDecision { ts_ms, .. }
            | Self::TradeHistory { ts_ms, .. }
            | Self::PnlSnapshot { ts_ms, .. } => *ts_ms,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditSpoolConfig {
    pub spool_dir: PathBuf,
    pub s3_prefix: String,
    pub node_name: String,
    pub run_id: String,
    pub ship_interval: Duration,
    pub upload_attempt_timeout: Duration,
    pub roll_max_bytes: u64,
    pub roll_max_secs: u64,
    pub max_local_backlog_bytes: u64,
}

pub trait AuditUploader: Send + Sync + 'static {
    fn upload_file(
        &self,
        local_path: &Path,
        s3_uri: &str,
    ) -> impl Future<Output = Result<()>> + Send;
}

pub struct AwsCliUploader;

impl AuditUploader for AwsCliUploader {
    fn upload_file(
        &self,
        local_path: &Path,
        s3_uri: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        let local_path = local_path.to_path_buf();
        let s3_uri = s3_uri.to_string();
        async move {
            let mut command = Command::new("aws");
            command.kill_on_drop(true);
            let output = command
                .args(["s3", "cp"])
                .arg(&local_path)
                .arg(&s3_uri)
                .output()
                .await
                .context("failed to spawn `aws s3 cp` for audit upload")?;

            if output.status.success() {
                return Ok(());
            }

            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!(
                "`aws s3 cp` failed for {} -> {}: {}",
                local_path.display(),
                s3_uri,
                if stderr.is_empty() {
                    format!("exit status {}", output.status)
                } else {
                    stderr
                }
            );
        }
    }
}

#[derive(Debug)]
pub struct AuditSendError(pub AuditRecord);

impl std::fmt::Display for AuditSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "audit channel is closed")
    }
}

impl std::error::Error for AuditSendError {}

#[derive(Debug)]
struct AuditChannelState {
    closed: bool,
    tx: UnboundedSender<AuditRecord>,
}

#[derive(Clone, Debug)]
struct AuditChannelCloser {
    state: Arc<Mutex<AuditChannelState>>,
}

impl AuditChannelCloser {
    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
        }
    }
}

#[derive(Clone, Debug)]
pub struct AuditSender {
    state: Arc<Mutex<AuditChannelState>>,
}

impl AuditSender {
    pub fn send(&self, record: AuditRecord) -> Result<(), AuditSendError> {
        let state = self.state.lock().expect("audit channel mutex poisoned");
        if state.closed {
            return Err(AuditSendError(record));
        }

        state
            .tx
            .send(record)
            .map_err(|error| AuditSendError(error.0))
    }
}

pub struct AuditReceiver {
    rx: UnboundedReceiver<AuditRecord>,
    closer: AuditChannelCloser,
}

pub fn audit_channel() -> (AuditSender, AuditReceiver) {
    // This channel is intentionally unbounded. NautilusTrader producers cannot await, so
    // backpressure is enforced by local spool growth and max_local_backlog_bytes instead.
    let (tx, rx) = unbounded_channel::<AuditRecord>();
    let state = Arc::new(Mutex::new(AuditChannelState { closed: false, tx }));
    let closer = AuditChannelCloser {
        state: Arc::clone(&state),
    };

    (AuditSender { state }, AuditReceiver { rx, closer })
}

pub fn build_s3_key(
    prefix: &str,
    node_name: &str,
    run_id: &str,
    date: &str,
    sequence: u64,
) -> String {
    format!("{prefix}/date={date}/node={node_name}/run={run_id}/part-{sequence:020}.jsonl")
}

pub struct AuditWorkerHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    audit_closer: Option<AuditChannelCloser>,
    join_handle: JoinHandle<Result<()>>,
}

impl AuditWorkerHandle {
    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(audit_closer) = self.audit_closer.take() {
            audit_closer.close();
        }

        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        match self.join_handle.await {
            Ok(result) => result,
            Err(error) => Err(anyhow!("audit worker join failed: {error}")),
        }
    }
}

pub fn spawn_audit_worker<U>(
    audit_rx: AuditReceiver,
    uploader: U,
    config: AuditSpoolConfig,
) -> AuditWorkerHandle
where
    U: AuditUploader,
{
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let audit_closer = audit_rx.closer.clone();
    let join_handle = tokio::spawn(run_audit_worker(audit_rx, shutdown_rx, uploader, config));

    AuditWorkerHandle {
        shutdown_tx: Some(shutdown_tx),
        audit_closer: Some(audit_closer),
        join_handle,
    }
}

struct OpenAuditFile {
    sequence: u64,
    date: String,
    path: PathBuf,
    opened_at: Instant,
    bytes: u64,
}

struct PendingAuditFile {
    sequence: u64,
    date: String,
    path: PathBuf,
    bytes: u64,
    retry_not_before: Option<Instant>,
}

struct ActiveUpload {
    file: PendingAuditFile,
    s3_uri: String,
    join_handle: JoinHandle<Result<()>>,
}

enum UploadAttemptResult {
    Completed(std::result::Result<Result<()>, JoinError>),
    TimedOut,
}

struct AuditSpoolState<U> {
    config: AuditSpoolConfig,
    uploader: Arc<U>,
    appender: JsonlAppender,
    current_file: Option<OpenAuditFile>,
    pending_uploads: VecDeque<PendingAuditFile>,
    active_upload: Option<ActiveUpload>,
    next_sequence: u64,
}

impl<U> AuditSpoolState<U>
where
    U: AuditUploader,
{
    fn new(config: AuditSpoolConfig, uploader: U) -> Result<Self> {
        let (pending_uploads, next_sequence) = discover_retained_spool_files(&config.spool_dir)?;
        if next_sequence > 0 {
            persist_sequence_watermark(&config.spool_dir, next_sequence)?;
        }
        let state = Self {
            config,
            uploader: Arc::new(uploader),
            appender: JsonlAppender::new(),
            current_file: None,
            pending_uploads,
            active_upload: None,
            next_sequence,
        };
        state.ensure_backlog_within_limit()?;
        Ok(state)
    }

    fn append_record(&mut self, record: AuditRecord) -> Result<()> {
        let date = date_from_ts_ms(record.ts_ms())?;
        self.roll_if_date_changed(&date)?;
        self.roll_if_threshold_exceeded()?;

        let path = self.ensure_current_file(date)?.path.clone();
        self.appender.append(&path, &record).map_err(|error| {
            anyhow!(
                "failed to append audit record to {}: {error}",
                path.display()
            )
        })?;
        let bytes = file_len(&path)?;

        if let Some(current) = self.current_file.as_mut() {
            current.bytes = bytes;
        }

        if bytes >= self.config.roll_max_bytes {
            self.roll_current_file()?;
        }

        self.ensure_backlog_within_limit()
    }

    fn flush_expired_open_file(&mut self) -> Result<()> {
        if self
            .current_file
            .as_ref()
            .is_some_and(|file| file.opened_at.elapsed().as_secs() >= self.config.roll_max_secs)
        {
            self.roll_current_file()?;
        }

        Ok(())
    }

    fn finalize_current_file(&mut self) -> Result<()> {
        self.roll_current_file()
    }

    fn start_next_upload(&mut self, ignore_retry_not_before: bool) -> Result<()> {
        if self.active_upload.is_some() {
            return Ok(());
        }

        let now = Instant::now();
        let Some(ready_index) = self.pending_uploads.iter().position(|file| {
            ignore_retry_not_before || file.retry_not_before.is_none_or(|deadline| deadline <= now)
        }) else {
            return Ok(());
        };

        let Some(file) = self.pending_uploads.remove(ready_index) else {
            return Ok(());
        };

        let s3_uri = build_s3_key(
            &self.config.s3_prefix,
            &self.config.node_name,
            &self.config.run_id,
            &file.date,
            file.sequence,
        );
        let uploader = Arc::clone(&self.uploader);
        let local_path = file.path.clone();
        let s3_uri_for_task = s3_uri.clone();
        let join_handle =
            tokio::spawn(async move { uploader.upload_file(&local_path, &s3_uri_for_task).await });

        self.active_upload = Some(ActiveUpload {
            file,
            s3_uri,
            join_handle,
        });
        Ok(())
    }

    fn complete_active_upload(
        &mut self,
        upload_result: UploadAttemptResult,
        final_attempt: bool,
    ) -> Result<bool> {
        let active_upload = self
            .active_upload
            .take()
            .expect("active upload must exist when completion is handled");

        match upload_result {
            UploadAttemptResult::Completed(Ok(Ok(()))) => {
                fs::remove_file(&active_upload.file.path).with_context(|| {
                    format!(
                        "failed to remove uploaded audit spool file {}",
                        active_upload.file.path.display()
                    )
                })?;
                Ok(true)
            }
            UploadAttemptResult::Completed(Ok(Err(error))) => {
                if final_attempt {
                    Err(anyhow!(
                        "final audit upload failed for {} -> {}: {error}",
                        active_upload.file.path.display(),
                        active_upload.s3_uri
                    ))
                } else {
                    self.requeue_failed_upload(active_upload.file);
                    Ok(false)
                }
            }
            UploadAttemptResult::Completed(Err(error)) => Err(anyhow!(
                "audit upload task join failed for {} -> {}: {error}",
                active_upload.file.path.display(),
                active_upload.s3_uri
            )),
            UploadAttemptResult::TimedOut => {
                active_upload.join_handle.abort();
                if final_attempt {
                    Err(anyhow!(
                        "final audit upload timed out after {:?} for {} -> {}",
                        self.config.upload_attempt_timeout,
                        active_upload.file.path.display(),
                        active_upload.s3_uri
                    ))
                } else {
                    self.requeue_failed_upload(active_upload.file);
                    Ok(false)
                }
            }
        }
    }

    async fn finish_all_uploads(&mut self, final_attempt: bool) -> Result<()> {
        let mut first_error = None;

        loop {
            self.start_next_upload(final_attempt)?;
            let Some(active_upload) = self.active_upload.as_mut() else {
                return match first_error {
                    Some(error) => Err(error),
                    None => Ok(()),
                };
            };

            let upload_result = match timeout(
                self.config.upload_attempt_timeout,
                &mut active_upload.join_handle,
            )
            .await
            {
                Ok(upload_result) => UploadAttemptResult::Completed(upload_result),
                Err(_) => UploadAttemptResult::TimedOut,
            };
            match self.complete_active_upload(upload_result, final_attempt) {
                Ok(_) => {}
                Err(error) if final_attempt => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn abort_active_upload(&mut self) {
        if let Some(active_upload) = self.active_upload.take() {
            active_upload.join_handle.abort();
        }
    }

    fn ensure_current_file(&mut self, date: String) -> Result<&mut OpenAuditFile> {
        if self.current_file.is_none() {
            let sequence = self.next_sequence;
            self.next_sequence += 1;
            persist_sequence_watermark(&self.config.spool_dir, self.next_sequence)?;
            let path = local_part_path(&self.config.spool_dir, &date, sequence);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create audit spool directory {}",
                        parent.display()
                    )
                })?;
            }

            self.current_file = Some(OpenAuditFile {
                sequence,
                date,
                path,
                opened_at: Instant::now(),
                bytes: 0,
            });
        }

        Ok(self
            .current_file
            .as_mut()
            .expect("current_file must exist after initialization"))
    }

    fn roll_if_threshold_exceeded(&mut self) -> Result<()> {
        if self.current_file.as_ref().is_some_and(|file| {
            file.bytes >= self.config.roll_max_bytes
                || file.opened_at.elapsed().as_secs() >= self.config.roll_max_secs
        }) {
            self.roll_current_file()?;
        }

        Ok(())
    }

    fn roll_if_date_changed(&mut self, next_date: &str) -> Result<()> {
        if self
            .current_file
            .as_ref()
            .is_some_and(|file| file.date != next_date)
        {
            self.roll_current_file()?;
        }

        Ok(())
    }

    fn roll_current_file(&mut self) -> Result<()> {
        let Some(file) = self.current_file.take() else {
            return Ok(());
        };

        self.appender
            .close()
            .context("failed to close current audit JSONL appender")?;

        let bytes = file_len(&file.path)?;
        self.pending_uploads.push_back(PendingAuditFile {
            sequence: file.sequence,
            date: file.date,
            path: file.path,
            bytes,
            retry_not_before: None,
        });

        self.ensure_backlog_within_limit()
    }

    fn requeue_failed_upload(&mut self, mut file: PendingAuditFile) {
        file.retry_not_before = Some(
            Instant::now()
                .checked_add(self.config.ship_interval)
                .unwrap_or_else(Instant::now),
        );
        self.pending_uploads.push_front(file);
    }

    fn ensure_backlog_within_limit(&self) -> Result<()> {
        let pending_bytes: u64 = self.pending_uploads.iter().map(|file| file.bytes).sum();
        let open_bytes = self.current_file.as_ref().map_or(0, |file| file.bytes);
        let active_bytes = self
            .active_upload
            .as_ref()
            .map_or(0, |upload| upload.file.bytes);
        let total = pending_bytes
            .saturating_add(open_bytes)
            .saturating_add(active_bytes);

        if total > self.config.max_local_backlog_bytes {
            bail!(
                "max_local_backlog_bytes exceeded: retained {} bytes under {} with limit {}",
                total,
                self.config.spool_dir.display(),
                self.config.max_local_backlog_bytes
            );
        }

        Ok(())
    }

    fn next_roll_deadline(&self) -> Option<Instant> {
        self.current_file.as_ref().map(|file| {
            file.opened_at
                .checked_add(Duration::from_secs(self.config.roll_max_secs))
                .unwrap_or(file.opened_at)
        })
    }
}

async fn run_audit_worker<U>(
    mut audit_rx: AuditReceiver,
    mut shutdown_rx: oneshot::Receiver<()>,
    uploader: U,
    config: AuditSpoolConfig,
) -> Result<()>
where
    U: AuditUploader,
{
    let mut ticker = interval_at(Instant::now() + config.ship_interval, config.ship_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut state = AuditSpoolState::new(config, uploader)?;
    let mut roll_timer = Box::pin(sleep_until(dormant_roll_deadline()));
    reset_roll_timer(roll_timer.as_mut(), &state);
    state.start_next_upload(false)?;

    let result = async {
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown_rx => {
                    audit_rx.closer.close();
                    audit_rx.rx.close();
                    drain_audit_channel(&mut audit_rx, &mut state)?;
                    break;
                }
            upload_result = wait_for_active_upload(
                state.active_upload.as_mut(),
                state.config.upload_attempt_timeout,
            ) => {
                if state.complete_active_upload(upload_result, false)? {
                    state.start_next_upload(false)?;
                }
            }
                _ = &mut roll_timer => {
                    state.flush_expired_open_file()?;
                    state.start_next_upload(false)?;
                    reset_roll_timer(roll_timer.as_mut(), &state);
                }
                _ = ticker.tick() => {
                    state.flush_expired_open_file()?;
                    state.start_next_upload(false)?;
                    reset_roll_timer(roll_timer.as_mut(), &state);
                }
                maybe_record = audit_rx.rx.recv() => {
                match maybe_record {
                    Some(record) => {
                        state.append_record(record)?;
                        reset_roll_timer(roll_timer.as_mut(), &state);
                    }
                    None => break,
                }
                }
            }
        }

        drain_audit_channel(&mut audit_rx, &mut state)?;
        state.finalize_current_file()?;
        state.finish_all_uploads(true).await?;
        state
            .appender
            .close()
            .context("failed to close audit appender during shutdown")?;
        Ok(())
    }
    .await;

    if result.is_err() {
        state.abort_active_upload();
    }

    result
}

fn drain_audit_channel<U>(
    audit_rx: &mut AuditReceiver,
    state: &mut AuditSpoolState<U>,
) -> Result<()>
where
    U: AuditUploader,
{
    loop {
        match audit_rx.rx.try_recv() {
            Ok(record) => state.append_record(record)?,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

fn reset_roll_timer<U>(roll_timer: Pin<&mut Sleep>, state: &AuditSpoolState<U>)
where
    U: AuditUploader,
{
    let deadline = state
        .next_roll_deadline()
        .unwrap_or_else(dormant_roll_deadline);
    roll_timer.reset(deadline);
}

fn dormant_roll_deadline() -> Instant {
    Instant::now() + Duration::from_secs(60 * 60 * 24 * 365)
}

async fn wait_for_active_upload(
    active_upload: Option<&mut ActiveUpload>,
    upload_attempt_timeout: Duration,
) -> UploadAttemptResult {
    match active_upload {
        Some(active_upload) => {
            match timeout(upload_attempt_timeout, &mut active_upload.join_handle).await {
                Ok(upload_result) => UploadAttemptResult::Completed(upload_result),
                Err(_) => UploadAttemptResult::TimedOut,
            }
        }
        None => std::future::pending::<UploadAttemptResult>().await,
    }
}

fn local_part_path(spool_dir: &Path, date: &str, sequence: u64) -> PathBuf {
    spool_dir
        .join(format!("date={date}"))
        .join(format!("part-{sequence:020}.jsonl"))
}

fn sequence_watermark_path(spool_dir: &Path) -> PathBuf {
    spool_dir.join(".sequence-watermark")
}

fn file_len(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)
        .with_context(|| format!("failed to stat audit spool file {}", path.display()))?
        .len())
}

fn date_from_ts_ms(ts_ms: u64) -> Result<String> {
    let seconds = (ts_ms / 1_000) as i64;
    let nanos = ((ts_ms % 1_000) as u32) * 1_000_000;
    let timestamp = DateTime::<Utc>::from_timestamp(seconds, nanos)
        .ok_or_else(|| anyhow!("invalid audit timestamp millis: {ts_ms}"))?;
    Ok(timestamp.format("%Y-%m-%d").to_string())
}

fn discover_retained_spool_files(spool_dir: &Path) -> Result<(VecDeque<PendingAuditFile>, u64)> {
    let mut retained_files = Vec::new();
    collect_retained_spool_files(spool_dir, spool_dir, &mut retained_files)?;
    retained_files.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.date.cmp(&right.date))
            .then_with(|| left.path.cmp(&right.path))
    });

    let next_sequence = retained_files
        .iter()
        .map(|file| file.sequence)
        .max()
        .map_or(0, |max_sequence| max_sequence.saturating_add(1));
    let next_sequence = next_sequence.max(read_sequence_watermark(spool_dir)?.unwrap_or(0));

    Ok((retained_files.into_iter().collect(), next_sequence))
}

fn read_sequence_watermark(spool_dir: &Path) -> Result<Option<u64>> {
    let path = sequence_watermark_path(spool_dir);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read audit sequence watermark {}", path.display())
            });
        }
    };

    let value = contents.trim();
    if value.is_empty() {
        return Ok(None);
    }

    value.parse().map(Some).with_context(|| {
        format!(
            "failed to parse audit sequence watermark from {}",
            path.display()
        )
    })
}

fn persist_sequence_watermark(spool_dir: &Path, next_sequence: u64) -> Result<()> {
    fs::create_dir_all(spool_dir).with_context(|| {
        format!(
            "failed to create audit spool directory {}",
            spool_dir.display()
        )
    })?;

    let path = sequence_watermark_path(spool_dir);
    let temp_path = spool_dir.join(".sequence-watermark.tmp");
    fs::write(&temp_path, format!("{next_sequence}\n")).with_context(|| {
        format!(
            "failed to write audit sequence watermark {}",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, &path).with_context(|| {
        format!(
            "failed to persist audit sequence watermark {}",
            path.display()
        )
    })?;
    Ok(())
}

fn collect_retained_spool_files(
    root: &Path,
    path: &Path,
    retained_files: &mut Vec<PendingAuditFile>,
) -> Result<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to read audit spool directory {}", path.display())
            });
        }
    };

    for entry in entries {
        let entry = entry.with_context(|| {
            format!("failed to inspect audit spool directory {}", path.display())
        })?;
        let entry_path = entry.path();

        if entry
            .file_type()
            .with_context(|| format!("failed to stat audit spool entry {}", entry_path.display()))?
            .is_dir()
        {
            collect_retained_spool_files(root, &entry_path, retained_files)?;
            continue;
        }

        if entry_path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        let Some(retained_file) = parse_retained_spool_file(root, &entry_path)? else {
            continue;
        };
        retained_files.push(retained_file);
    }

    Ok(())
}

fn parse_retained_spool_file(root: &Path, path: &Path) -> Result<Option<PendingAuditFile>> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "audit spool file {} is not rooted under {}",
            path.display(),
            root.display()
        )
    })?;
    let mut components = relative.iter();
    let Some(date_component) = components.next().and_then(|component| component.to_str()) else {
        return Ok(None);
    };
    let Some(file_component) = components.next().and_then(|component| component.to_str()) else {
        return Ok(None);
    };

    if components.next().is_some() {
        return Ok(None);
    }

    let Some(date) = date_component.strip_prefix("date=") else {
        return Ok(None);
    };
    let Some(sequence) = file_component
        .strip_prefix("part-")
        .and_then(|name| name.strip_suffix(".jsonl"))
    else {
        return Ok(None);
    };
    let bytes = file_len(path)?;
    if bytes == 0 {
        fs::remove_file(path).with_context(|| {
            format!(
                "failed to prune empty retained audit spool file {}",
                path.display()
            )
        })?;
        return Ok(None);
    }
    validate_retained_spool_file(path)?;

    Ok(Some(PendingAuditFile {
        sequence: sequence.parse().with_context(|| {
            format!(
                "failed to parse audit spool sequence from {}",
                path.display()
            )
        })?,
        date: date.to_string(),
        path: path.to_path_buf(),
        bytes,
        retry_not_before: None,
    }))
}

fn validate_retained_spool_file(path: &Path) -> Result<()> {
    let file = fs::File::open(path).with_context(|| {
        format!(
            "failed to open retained audit spool file {}",
            path.display()
        )
    })?;
    let reader = BufReader::new(file);

    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line.with_context(|| {
            format!(
                "failed to read retained audit spool file {} at line {}",
                path.display(),
                line_number
            )
        })?;
        serde_json::from_str::<AuditRecord>(&line).with_context(|| {
            format!(
                "invalid retained audit spool file {} at line {}",
                path.display(),
                line_number
            )
        })?;
    }

    Ok(())
}
