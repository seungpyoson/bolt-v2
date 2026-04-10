use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow};
use bolt_v2::platform::audit::{
    AuditRecord, AuditSpoolConfig, AuditUploader, ReferenceVenueSnapshot, SelectorState,
    TradeEventKind, VenueHealthState, audit_channel, build_s3_key, spawn_audit_worker,
};
use serde_json::Value;
use tempfile::tempdir;
use tokio::sync::Notify;

#[derive(Clone, Debug)]
struct UploadCall {
    local_path: PathBuf,
    s3_uri: String,
    contents: String,
}

#[derive(Clone, Default)]
struct MockUploader {
    state: Arc<Mutex<MockUploaderState>>,
}

#[derive(Clone)]
enum UploadOutcome {
    Succeed,
    Fail,
    DelayFail(Duration),
    Block(Arc<Notify>),
}

impl From<bool> for UploadOutcome {
    fn from(value: bool) -> Self {
        if value { Self::Succeed } else { Self::Fail }
    }
}

#[derive(Default)]
struct MockUploaderState {
    outcomes: VecDeque<UploadOutcome>,
    calls: Vec<UploadCall>,
}

impl MockUploader {
    fn with_outcomes(outcomes: impl IntoIterator<Item = impl Into<UploadOutcome>>) -> Self {
        Self {
            state: Arc::new(Mutex::new(MockUploaderState {
                outcomes: outcomes.into_iter().map(Into::into).collect(),
                calls: Vec::new(),
            })),
        }
    }

    fn calls(&self) -> Vec<UploadCall> {
        self.state.lock().unwrap().calls.clone()
    }

    fn attempt_count(&self) -> usize {
        self.state.lock().unwrap().calls.len()
    }
}

impl AuditUploader for MockUploader {
    fn upload_file(
        &self,
        local_path: &Path,
        s3_uri: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let local_path = local_path.to_path_buf();
        let s3_uri = s3_uri.to_string();
        let state = Arc::clone(&self.state);

        async move {
            let contents = fs::read_to_string(&local_path)?;
            let outcome = {
                let mut state = state.lock().unwrap();
                state.calls.push(UploadCall {
                    local_path,
                    s3_uri,
                    contents,
                });

                state.outcomes.pop_front().unwrap_or(UploadOutcome::Succeed)
            };

            match outcome {
                UploadOutcome::Succeed => Ok(()),
                UploadOutcome::Fail => Err(anyhow!("mock upload failure")),
                UploadOutcome::DelayFail(delay) => {
                    tokio::time::sleep(delay).await;
                    Err(anyhow!("mock upload failure"))
                }
                UploadOutcome::Block(release) => {
                    release.notified().await;
                    Ok(())
                }
            }
        }
    }
}

fn sample_record(ts_ms: u64) -> AuditRecord {
    AuditRecord::ReferenceSnapshot {
        ts_ms,
        topic: "midpoint".to_string(),
        fair_value: Some(0.51),
        confidence: 0.93,
        venues: vec![
            ReferenceVenueSnapshot {
                venue_name: "binance-btc".to_string(),
                base_weight: 0.7,
                effective_weight: 0.7,
                stale: false,
                health: VenueHealthState::Healthy,
                reason: None,
                observed_price: Some(0.5),
            },
            ReferenceVenueSnapshot {
                venue_name: "kraken-btc".to_string(),
                base_weight: 0.3,
                effective_weight: 0.0,
                stale: true,
                health: VenueHealthState::Disabled,
                reason: Some("feed lagging".to_string()),
                observed_price: Some(0.55),
            },
        ],
    }
}

fn decision_record(ts_ms: u64) -> AuditRecord {
    AuditRecord::SelectorDecision {
        ts_ms,
        ruleset_id: "ruleset-a".to_string(),
        state: SelectorState::Freeze,
        market_id: Some("market-1".to_string()),
        instrument_id: Some("instrument-1".to_string()),
        reason: Some("venue unhealthy".to_string()),
    }
}

fn history_record(ts_ms: u64) -> AuditRecord {
    AuditRecord::TradeHistory {
        ts_ms,
        strategy_id: "strategy-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        client_order_id: "order-1".to_string(),
        event: TradeEventKind::Filled,
        pnl_delta: Some(12.5),
    }
}

fn status_record(ts_ms: u64) -> AuditRecord {
    AuditRecord::VenueStatus {
        ts_ms,
        venue_name: "polymarket".to_string(),
        status: VenueHealthState::Disabled,
        reason: Some("maintenance".to_string()),
    }
}

fn pnl_snapshot_record(ts_ms: u64) -> AuditRecord {
    AuditRecord::PnlSnapshot {
        ts_ms,
        strategy_id: "strategy-1".to_string(),
        realized_pnl: 17.5,
        unrealized_pnl: Some(-2.25),
    }
}

fn config(spool_dir: &Path) -> AuditSpoolConfig {
    AuditSpoolConfig {
        spool_dir: spool_dir.to_path_buf(),
        s3_prefix: "s3://bucket/audit".to_string(),
        node_name: "node-a".to_string(),
        run_id: "run-42".to_string(),
        ship_interval: Duration::from_millis(25),
        upload_attempt_timeout: Duration::from_secs(30),
        roll_max_bytes: 200,
        roll_max_secs: 60,
        max_local_backlog_bytes: 4 * 1024 * 1024,
    }
}

fn jsonl_files(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };

    let mut paths = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            paths.extend(jsonl_files(&path));
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

fn write_retained_jsonl(path: &Path, records: &[AuditRecord]) {
    let contents = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, format!("{contents}\n")).unwrap();
}

async fn wait_for_attempts(uploader: &MockUploader, expected: usize) {
    for _ in 0..100 {
        if uploader.attempt_count() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!(
        "timed out waiting for {expected} upload attempts, saw {}",
        uploader.attempt_count()
    );
}

async fn wait_for_jsonl_file_count(root: &Path, expected: usize) {
    for _ in 0..100 {
        if jsonl_files(root).len() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!(
        "timed out waiting for at least {expected} jsonl files under {}",
        root.display()
    );
}

async fn wait_for_no_jsonl_files(root: &Path) {
    for _ in 0..100 {
        if jsonl_files(root).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    panic!(
        "timed out waiting for all jsonl files under {} to be removed",
        root.display()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn audit_records_serialize_as_jsonl() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    audit_tx.send(decision_record(200)).unwrap();
    audit_tx
        .send(AuditRecord::VenueStatus {
            ts_ms: 300,
            venue_name: "kalshi".to_string(),
            status: VenueHealthState::Healthy,
            reason: None,
        })
        .unwrap();
    audit_tx
        .send(AuditRecord::TradeHistory {
            ts_ms: 400,
            strategy_id: "strategy-1".to_string(),
            instrument_id: "instrument-1".to_string(),
            client_order_id: "order-2".to_string(),
            event: TradeEventKind::Accepted,
            pnl_delta: None,
        })
        .unwrap();
    audit_tx.send(pnl_snapshot_record(500)).unwrap();
    drop(audit_tx);

    worker.shutdown().await.unwrap();

    let calls = uploader.calls();
    assert_eq!(calls.len(), 1);
    let lines: Vec<Value> = calls[0]
        .contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();

    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["kind"], "reference_snapshot");
    assert_eq!(lines[0]["topic"], "midpoint");
    assert_eq!(lines[0]["fair_value"], 0.51);
    assert_eq!(lines[0]["venues"][0]["venue_name"], "binance-btc");
    assert_eq!(lines[0]["venues"][0]["health"], "healthy");
    assert!(lines[0]["venues"][0]["reason"].is_null());
    assert_eq!(lines[0]["venues"][1]["venue_name"], "kraken-btc");
    assert_eq!(lines[0]["venues"][1]["effective_weight"], 0.0);
    assert_eq!(lines[0]["venues"][1]["stale"], true);
    assert_eq!(lines[0]["venues"][1]["health"], "disabled");
    assert_eq!(lines[0]["venues"][1]["reason"], "feed lagging");
    assert_eq!(lines[0]["venues"][1]["observed_price"], 0.55);
    assert_eq!(lines[1]["kind"], "selector_decision");
    assert_eq!(lines[1]["state"], "freeze");
    assert_eq!(lines[1]["reason"], "venue unhealthy");
    assert_eq!(lines[1]["market_id"], "market-1");
    assert_eq!(lines[1]["instrument_id"], "instrument-1");
    assert_eq!(lines[2]["kind"], "venue_status");
    assert!(lines[2]["reason"].is_null());
    assert_eq!(lines[3]["kind"], "trade_history");
    assert!(lines[3]["pnl_delta"].is_null());
    assert_eq!(lines[4]["kind"], "pnl_snapshot");
    assert_eq!(lines[4]["unrealized_pnl"], -2.25);
}

#[tokio::test(flavor = "current_thread")]
async fn rolled_audit_files_are_uploaded_via_async_uploader_trait() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([true, true, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    audit_tx.send(history_record(200)).unwrap();

    wait_for_attempts(&uploader, 1).await;
    drop(audit_tx);
    worker.shutdown().await.unwrap();

    let calls = uploader.calls();
    assert!(
        calls.len() >= 2,
        "expected at least one rolled upload and one final upload, got {calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|call| call.s3_uri.starts_with("s3://bucket/audit/date=")),
        "{calls:?}"
    );
}

#[test]
fn s3_key_template_is_date_and_node_partitioned() {
    let key = build_s3_key("s3://bucket/audit", "node-a", "run-42", "2026-04-09", 17);

    assert_eq!(
        key,
        "s3://bucket/audit/date=2026-04-09/node=node-a/run=run-42/part-00000000000000000017.jsonl"
    );
    assert!(!key.contains("/kind="));
}

#[tokio::test(flavor = "current_thread")]
async fn failed_upload_keeps_local_file_for_retry() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([false, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    audit_tx.send(status_record(200)).unwrap();

    wait_for_attempts(&uploader, 1).await;
    let failed_call_path = uploader.calls()[0].local_path.clone();
    assert!(failed_call_path.exists());
    assert!(
        !jsonl_files(dir.path()).is_empty(),
        "expected retained local audit spool files after failed upload"
    );

    wait_for_attempts(&uploader, 2).await;
    wait_for_no_jsonl_files(dir.path()).await;

    drop(audit_tx);
    worker.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn failed_retained_upload_retries_on_next_ship_tick_not_failure_deadline() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let retained_path = retained_dir.join("part-00000000000000000007.jsonl");
    write_retained_jsonl(&retained_path, &[sample_record(1_000)]);

    let uploader = MockUploader::with_outcomes([
        UploadOutcome::DelayFail(Duration::from_millis(50)),
        UploadOutcome::Succeed,
    ]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.ship_interval = Duration::from_millis(250);
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        uploader.attempt_count(),
        1,
        "failed retained upload should remain deferred until the next ship tick"
    );

    tokio::time::timeout(Duration::from_millis(250), wait_for_attempts(&uploader, 2))
        .await
        .expect(
            "failed retained upload should retry on the first later ship tick, not failure time + ship_interval",
        );
    assert_eq!(
        uploader.attempt_count(),
        2,
        "failed retained upload should retry on the first later ship tick, not failure time + ship_interval"
    );

    drop(audit_tx);
    worker.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn failed_upload_immediately_starts_next_ready_file_without_retrying_failed_one() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let failed_path = retained_dir.join("part-00000000000000000007.jsonl");
    let ready_path = retained_dir.join("part-00000000000000000008.jsonl");
    write_retained_jsonl(&failed_path, &[sample_record(1_000)]);
    write_retained_jsonl(&ready_path, &[history_record(2_000)]);

    let uploader = MockUploader::with_outcomes([
        UploadOutcome::DelayFail(Duration::from_millis(50)),
        UploadOutcome::Succeed,
        UploadOutcome::Succeed,
    ]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.ship_interval = Duration::from_millis(250);
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    wait_for_attempts(&uploader, 1).await;
    tokio::time::timeout(Duration::from_millis(150), wait_for_attempts(&uploader, 2))
        .await
        .expect("ready retained file should start immediately after a non-final failure");

    let calls_after_ready_upload = uploader.calls();
    assert_eq!(calls_after_ready_upload[0].local_path, failed_path);
    assert_eq!(calls_after_ready_upload[1].local_path, ready_path);

    tokio::time::sleep(Duration::from_millis(100)).await;
    let calls_before_retry = uploader.calls();
    assert_eq!(
        calls_before_retry.len(),
        2,
        "failed upload should stay deferred until the next ship tick"
    );
    assert_eq!(
        calls_before_retry
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        1,
        "failed file should not retry before the next ship tick"
    );

    tokio::time::timeout(Duration::from_millis(250), wait_for_attempts(&uploader, 3))
        .await
        .expect("failed upload should retry on the next ship tick");
    assert_eq!(
        uploader
            .calls()
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        2,
        "failed file should retry once the next ship tick releases it"
    );

    drop(audit_tx);
    worker.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn failed_upload_retry_waits_for_ship_interval_even_if_earlier_roll_occurs() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([false, true, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.ship_interval = Duration::from_millis(250);
    cfg.roll_max_secs = 0;
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    wait_for_attempts(&uploader, 1).await;
    let failed_path = uploader.calls()[0].local_path.clone();

    audit_tx.send(history_record(200)).unwrap();
    wait_for_attempts(&uploader, 2).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    let calls_before_retry = uploader.calls();
    assert_eq!(
        calls_before_retry.len(),
        2,
        "roll activity should only upload the newly rolled file before ship_interval elapses"
    );
    assert_eq!(
        calls_before_retry
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        1,
        "failed upload should not retry on the earlier roll timer"
    );
    assert!(
        calls_before_retry
            .iter()
            .any(|call| call.local_path != failed_path),
        "newly rolled files should remain eligible for immediate first upload"
    );

    wait_for_attempts(&uploader, 3).await;
    let calls_after_retry = uploader.calls();
    assert_eq!(
        calls_after_retry
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        2,
        "failed upload should retry after ship_interval elapses"
    );

    drop(audit_tx);
    worker.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_attempts_deferred_failed_upload_before_retry_interval_elapses() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([false, false]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.ship_interval = Duration::from_millis(250);
    cfg.roll_max_bytes = 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    wait_for_attempts(&uploader, 1).await;

    let failed_path = uploader.calls()[0].local_path.clone();
    assert_eq!(
        uploader
            .calls()
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        1,
        "normal operation should only attempt the failed upload once before shutdown"
    );

    drop(audit_tx);
    let error = worker.shutdown().await.unwrap_err();

    assert!(
        error.to_string().contains("final audit upload failed"),
        "{error:#}"
    );
    assert_eq!(
        uploader
            .calls()
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        2,
        "shutdown should retry the deferred failed upload once even before ship_interval elapses"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_exhausts_final_upload_queue_before_returning_first_error() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([false, false, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.ship_interval = Duration::from_millis(250);
    cfg.roll_max_bytes = 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    wait_for_attempts(&uploader, 1).await;
    let failed_path = uploader.calls()[0].local_path.clone();

    audit_tx.send(history_record(200)).unwrap();
    drop(audit_tx);

    let error = worker.shutdown().await.unwrap_err();
    let calls = uploader.calls();

    assert!(
        error
            .to_string()
            .contains(&failed_path.display().to_string()),
        "{error:#}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.local_path == failed_path)
            .count(),
        2,
        "shutdown should retry the deferred failed upload once"
    );
    assert_eq!(
        calls.len(),
        3,
        "shutdown should continue through the remaining final upload queue"
    );

    let final_call_path = calls
        .iter()
        .find(|call| call.local_path != failed_path)
        .expect("expected the later final file to be attempted")
        .local_path
        .clone();
    assert!(
        !final_call_path.exists(),
        "successful final uploads should still be removed after queue exhaustion"
    );
    assert!(
        failed_path.exists(),
        "failed final uploads must remain local"
    );
    assert_eq!(jsonl_files(dir.path()), vec![failed_path]);
}

#[tokio::test(flavor = "current_thread")]
async fn backlog_limit_breach_returns_error() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([false, false, false, false]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    cfg.max_local_backlog_bytes = 150;
    let worker = spawn_audit_worker(audit_rx, uploader, cfg);

    audit_tx.send(sample_record(100)).unwrap();
    audit_tx.send(history_record(200)).unwrap();
    audit_tx.send(decision_record(300)).unwrap();
    drop(audit_tx);

    let error = worker.shutdown().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("max_local_backlog_bytes exceeded"),
        "{error:#}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_flushes_final_file_and_attempts_final_upload() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    drop(audit_tx);

    worker.shutdown().await.unwrap();

    let calls = uploader.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0]
            .local_path
            .ends_with("part-00000000000000000000.jsonl")
    );
    assert_eq!(jsonl_files(dir.path()).len(), 0);
    assert_eq!(calls[0].contents.lines().count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_rejects_sends_after_shutdown_begins() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let held_sender = audit_tx.clone();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();

    let shutdown_task = tokio::spawn(async move { worker.shutdown().await });
    tokio::task::yield_now().await;

    assert!(
        held_sender.send(sample_record(200)).is_err(),
        "send after shutdown began should be rejected"
    );
    drop(audit_tx);
    drop(held_sender);

    shutdown_task.await.unwrap().unwrap();

    let calls = uploader.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].contents.lines().count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn idle_open_file_rolls_on_age_even_when_ship_interval_is_longer() {
    let dir = tempdir().unwrap();
    let uploader = MockUploader::with_outcomes([true, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.ship_interval = Duration::from_secs(3);
    cfg.roll_max_secs = 1;
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();

    wait_for_attempts(&uploader, 1).await;
    drop(audit_tx);
    worker.shutdown().await.unwrap();

    let calls = uploader.calls();
    assert!(
        !calls.is_empty(),
        "expected at least one upload after age-based rolling"
    );
    assert_eq!(calls[0].contents.lines().count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn local_append_failure_is_fail_closed() {
    let dir = tempdir().unwrap();
    let blocked_spool_path = dir.path().join("blocked-spool");
    fs::write(&blocked_spool_path, b"not a directory").unwrap();

    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(&blocked_spool_path);
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader, cfg);

    audit_tx.send(sample_record(100)).unwrap();
    drop(audit_tx);

    let error = worker.shutdown().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed to create audit spool directory")
            || error
                .to_string()
                .contains("failed to read audit spool directory")
            || error.to_string().contains("failed to append audit record"),
        "{error:#}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restart_uploads_retained_files_before_reusing_sequence_numbers() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let retained_path = retained_dir.join("part-00000000000000000007.jsonl");
    write_retained_jsonl(&retained_path, &[sample_record(1_000)]);

    let uploader = MockUploader::with_outcomes([true, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    wait_for_attempts(&uploader, 1).await;
    audit_tx.send(sample_record(2_000)).unwrap();
    drop(audit_tx);
    worker.shutdown().await.unwrap();

    let calls = uploader.calls();
    assert!(
        calls.len() >= 2,
        "expected retained upload plus new upload, got {calls:?}"
    );
    assert_eq!(
        calls[0]
            .local_path
            .file_name()
            .and_then(|name| name.to_str()),
        Some("part-00000000000000000007.jsonl")
    );
    assert!(
        calls[0]
            .s3_uri
            .ends_with("/part-00000000000000000007.jsonl"),
        "retained file should keep its sequence in S3 key: {}",
        calls[0].s3_uri
    );
    assert!(
        calls[1]
            .local_path
            .ends_with("part-00000000000000000008.jsonl"),
        "new file should continue at the next sequence: {}",
        calls[1].local_path.display()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restart_prunes_empty_retained_jsonl_files_before_reusing_sequence_numbers() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let empty_path = retained_dir.join("part-00000000000000000008.jsonl");
    fs::write(&empty_path, b"").unwrap();

    let uploader = MockUploader::with_outcomes([true, true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !empty_path.exists(),
        "empty retained jsonl files should be pruned during startup"
    );
    assert_eq!(
        uploader.attempt_count(),
        0,
        "empty retained jsonl files should not be uploaded"
    );

    audit_tx.send(sample_record(2_000)).unwrap();
    drop(audit_tx);
    worker.shutdown().await.unwrap();

    let calls = uploader.calls();
    assert_eq!(
        calls.len(),
        1,
        "expected only the new upload, got {calls:?}"
    );
    assert!(
        calls[0]
            .local_path
            .ends_with("part-00000000000000000000.jsonl"),
        "new file should reuse the pruned empty sequence: {}",
        calls[0].local_path.display()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn restart_preserves_next_sequence_after_successful_upload_clears_local_spool() {
    let dir = tempdir().unwrap();

    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 10_000;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg.clone());

    audit_tx.send(sample_record(1_000)).unwrap();
    drop(audit_tx);
    worker.shutdown().await.unwrap();

    wait_for_no_jsonl_files(dir.path()).await;
    assert_eq!(uploader.calls().len(), 1);
    assert!(
        uploader.calls()[0]
            .local_path
            .ends_with("part-00000000000000000000.jsonl")
    );

    let restarted_uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let restarted_worker = spawn_audit_worker(audit_rx, restarted_uploader.clone(), cfg);

    audit_tx.send(sample_record(2_000)).unwrap();
    drop(audit_tx);
    restarted_worker.shutdown().await.unwrap();

    let calls = restarted_uploader.calls();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0]
            .local_path
            .ends_with("part-00000000000000000001.jsonl"),
        "restart should continue from the last issued sequence even after the spool is emptied: {}",
        calls[0].local_path.display()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn retained_legacy_reference_snapshot_without_venues_replays_on_restart() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let retained_path = retained_dir.join("part-00000000000000000007.jsonl");
    fs::write(
        &retained_path,
        concat!(
            "{\"kind\":\"reference_snapshot\",\"ts_ms\":1000,\"topic\":\"midpoint\",",
            "\"fair_value\":0.51,\"confidence\":0.93}\n"
        ),
    )
    .unwrap();

    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), config(dir.path()));

    drop(audit_tx);
    worker.shutdown().await.unwrap();

    wait_for_no_jsonl_files(dir.path()).await;
    let calls = uploader.calls();
    assert_eq!(calls.len(), 1);

    let lines: Vec<Value> = calls[0]
        .contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["kind"], "reference_snapshot");
    assert_eq!(lines[0]["topic"], "midpoint");
    assert_eq!(lines[0]["fair_value"], 0.51);
    assert_eq!(lines[0]["confidence"], 0.93);
    assert!(
        lines[0].get("venues").is_none(),
        "legacy retained rows should upload unchanged"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn corrupt_retained_jsonl_fails_closed_on_restart() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let retained_path = retained_dir.join("part-00000000000000000007.jsonl");
    fs::write(
        &retained_path,
        format!(
            "{}\n{}",
            serde_json::to_string(&sample_record(1_000)).unwrap(),
            "{\"kind\":\"reference_snapshot\",\"ts_ms\":1001"
        ),
    )
    .unwrap();

    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), config(dir.path()));

    drop(audit_tx);

    let error = worker.shutdown().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid retained audit spool file")
            && error.to_string().contains("at line 2"),
        "{error:#}"
    );
    assert_eq!(uploader.attempt_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn wrong_date_retained_jsonl_fails_closed_on_restart() {
    let dir = tempdir().unwrap();
    let retained_dir = dir.path().join("date=1970-01-01");
    fs::create_dir_all(&retained_dir).unwrap();
    let retained_path = retained_dir.join("part-00000000000000000007.jsonl");
    write_retained_jsonl(&retained_path, &[sample_record(86_400_000)]);

    let uploader = MockUploader::with_outcomes([true]);
    let (audit_tx, audit_rx) = audit_channel();
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), config(dir.path()));

    drop(audit_tx);

    let error = worker.shutdown().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("record date 1970-01-02 does not match path date 1970-01-01"),
        "{error:#}"
    );
    assert_eq!(uploader.attempt_count(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_upload_does_not_prevent_continued_spooling_or_backlog_enforcement() {
    let dir = tempdir().unwrap();
    let release_upload = Arc::new(Notify::new());
    let uploader = MockUploader::with_outcomes([
        UploadOutcome::Block(Arc::clone(&release_upload)),
        UploadOutcome::Fail,
        UploadOutcome::Fail,
        UploadOutcome::Fail,
    ]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    let retained_backlog_bytes = serde_json::to_vec(&sample_record(100)).unwrap().len() as u64
        + serde_json::to_vec(&history_record(200)).unwrap().len() as u64
        + serde_json::to_vec(&decision_record(300)).unwrap().len() as u64
        + 2;
    cfg.max_local_backlog_bytes = retained_backlog_bytes - 1;
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    wait_for_attempts(&uploader, 1).await;

    audit_tx.send(history_record(200)).unwrap();
    wait_for_jsonl_file_count(dir.path(), 2).await;
    assert_eq!(
        uploader.attempt_count(),
        1,
        "blocked upload should not allow a second upload to start yet"
    );

    let _ = audit_tx.send(decision_record(300));
    drop(audit_tx);

    let error = tokio::time::timeout(Duration::from_millis(500), worker.shutdown())
        .await
        .expect("shutdown should surface backlog failure even with a blocked upload")
        .unwrap_err();
    release_upload.notify_waiters();

    assert!(
        error
            .to_string()
            .contains("max_local_backlog_bytes exceeded"),
        "{error:#}"
    );
    assert!(
        jsonl_files(dir.path()).len() >= 2,
        "expected continued local spooling while upload was blocked"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_inflight_upload_times_out_on_shutdown_and_retains_spool_file() {
    let dir = tempdir().unwrap();
    let release_upload = Arc::new(Notify::new());
    let uploader = MockUploader::with_outcomes([UploadOutcome::Block(Arc::clone(&release_upload))]);
    let (audit_tx, audit_rx) = audit_channel();
    let mut cfg = config(dir.path());
    cfg.roll_max_bytes = 1;
    cfg.upload_attempt_timeout = Duration::from_millis(100);
    let worker = spawn_audit_worker(audit_rx, uploader.clone(), cfg);

    audit_tx.send(sample_record(100)).unwrap();
    wait_for_attempts(&uploader, 1).await;

    let blocked_call_path = uploader.calls()[0].local_path.clone();
    assert!(blocked_call_path.exists());

    drop(audit_tx);
    let error = tokio::time::timeout(Duration::from_millis(500), worker.shutdown())
        .await
        .expect("shutdown should error instead of hanging")
        .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error:#}");
    assert!(blocked_call_path.exists());
    assert_eq!(jsonl_files(dir.path()), vec![blocked_call_path.clone()]);

    release_upload.notify_waiters();
}
