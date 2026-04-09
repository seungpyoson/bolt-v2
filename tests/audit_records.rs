use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow};
use bolt_v2::platform::audit::{
    AuditRecord, AuditSpoolConfig, AuditUploader, SelectorState, TradeEventKind,
    VenueHealthState, audit_channel, build_s3_key, spawn_audit_worker,
};
use serde_json::Value;
use tempfile::tempdir;

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

#[derive(Default)]
struct MockUploaderState {
    outcomes: VecDeque<bool>,
    calls: Vec<UploadCall>,
}

impl MockUploader {
    fn with_outcomes(outcomes: impl IntoIterator<Item = bool>) -> Self {
        Self {
            state: Arc::new(Mutex::new(MockUploaderState {
                outcomes: outcomes.into_iter().collect(),
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
            let mut state = state.lock().unwrap();
            state.calls.push(UploadCall {
                local_path,
                s3_uri,
                contents,
            });

            let succeeds = state.outcomes.pop_front().unwrap_or(true);
            if succeeds {
                Ok(())
            } else {
                Err(anyhow!("mock upload failure"))
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
    assert!(jsonl_files(dir.path()).is_empty());

    drop(audit_tx);
    worker.shutdown().await.unwrap();
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
        error.to_string().contains("failed to create audit spool directory")
            || error.to_string().contains("failed to append audit record"),
        "{error:#}"
    );
}
