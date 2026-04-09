use std::{
    collections::VecDeque,
    fs,
    future::Future,
    path::{Path, PathBuf},
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
    task::JoinHandle,
    time::{Instant, MissedTickBehavior, interval},
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
        fair_value: f64,
        confidence: f64,
    },
    VenueStatus {
        ts_ms: u64,
        venue_name: String,
        status: VenueHealthState,
        reason: String,
    },
    SelectorDecision {
        ts_ms: u64,
        ruleset_id: String,
        state: SelectorState,
        market_id: String,
        instrument_id: String,
        reason: String,
    },
    TradeHistory {
        ts_ms: u64,
        strategy_id: String,
        instrument_id: String,
        client_order_id: String,
        event: TradeEventKind,
        pnl_delta: f64,
    },
    PnlSnapshot {
        ts_ms: u64,
        strategy_id: String,
        realized_pnl: f64,
        unrealized_pnl: f64,
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
            let output = Command::new("aws")
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

pub type AuditSender = UnboundedSender<AuditRecord>;
pub type AuditReceiver = UnboundedReceiver<AuditRecord>;

pub fn audit_channel() -> (AuditSender, AuditReceiver) {
    // This channel is intentionally unbounded. NautilusTrader producers cannot await, so
    // backpressure is enforced by local spool growth and max_local_backlog_bytes instead.
    unbounded_channel::<AuditRecord>()
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
    join_handle: JoinHandle<Result<()>>,
}

impl AuditWorkerHandle {
    pub async fn shutdown(mut self) -> Result<()> {
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
    let join_handle = tokio::spawn(run_audit_worker(audit_rx, shutdown_rx, uploader, config));

    AuditWorkerHandle {
        shutdown_tx: Some(shutdown_tx),
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
}

struct AuditSpoolState<U> {
    config: AuditSpoolConfig,
    uploader: U,
    appender: JsonlAppender,
    current_file: Option<OpenAuditFile>,
    pending_uploads: VecDeque<PendingAuditFile>,
    next_sequence: u64,
}

impl<U> AuditSpoolState<U>
where
    U: AuditUploader,
{
    fn new(config: AuditSpoolConfig, uploader: U) -> Self {
        Self {
            config,
            uploader,
            appender: JsonlAppender::new(),
            current_file: None,
            pending_uploads: VecDeque::new(),
            next_sequence: 0,
        }
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

    async fn upload_ready_files(&mut self, final_attempt: bool) -> Result<()> {
        loop {
            let Some(next_file) = self.pending_uploads.front() else {
                return Ok(());
            };

            let s3_uri = build_s3_key(
                &self.config.s3_prefix,
                &self.config.node_name,
                &self.config.run_id,
                &next_file.date,
                next_file.sequence,
            );

            match self.uploader.upload_file(&next_file.path, &s3_uri).await {
                Ok(()) => {
                    fs::remove_file(&next_file.path).with_context(|| {
                        format!(
                            "failed to remove uploaded audit spool file {}",
                            next_file.path.display()
                        )
                    })?;
                    self.pending_uploads.pop_front();
                }
                Err(error) => {
                    if final_attempt {
                        return Err(anyhow!(
                            "final audit upload failed for {} -> {}: {error}",
                            next_file.path.display(),
                            s3_uri
                        ));
                    }
                    return Ok(());
                }
            }
        }
    }

    fn ensure_current_file(&mut self, date: String) -> Result<&mut OpenAuditFile> {
        if self.current_file.is_none() {
            let sequence = self.next_sequence;
            self.next_sequence += 1;
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
        });

        self.ensure_backlog_within_limit()
    }

    fn ensure_backlog_within_limit(&self) -> Result<()> {
        let pending_bytes: u64 = self.pending_uploads.iter().map(|file| file.bytes).sum();
        let open_bytes = self.current_file.as_ref().map_or(0, |file| file.bytes);
        let total = pending_bytes.saturating_add(open_bytes);

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
    let mut ticker = interval(config.ship_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut state = AuditSpoolState::new(config, uploader);

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                drain_audit_channel(&mut audit_rx, &mut state)?;
                break;
            }
            _ = ticker.tick() => {
                state.flush_expired_open_file()?;
                state.upload_ready_files(false).await?;
            }
            maybe_record = audit_rx.recv() => {
                match maybe_record {
                    Some(record) => state.append_record(record)?,
                    None => break,
                }
            }
        }
    }

    drain_audit_channel(&mut audit_rx, &mut state)?;
    state.finalize_current_file()?;
    state.upload_ready_files(true).await?;
    state
        .appender
        .close()
        .context("failed to close audit appender during shutdown")?;
    Ok(())
}

fn drain_audit_channel<U>(
    audit_rx: &mut AuditReceiver,
    state: &mut AuditSpoolState<U>,
) -> Result<()>
where
    U: AuditUploader,
{
    loop {
        match audit_rx.try_recv() {
            Ok(record) => state.append_record(record)?,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

fn local_part_path(spool_dir: &Path, date: &str, sequence: u64) -> PathBuf {
    spool_dir
        .join(format!("date={date}"))
        .join(format!("part-{sequence:020}.jsonl"))
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
