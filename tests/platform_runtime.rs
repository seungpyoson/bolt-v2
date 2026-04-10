mod support;

use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Result, anyhow};
use bolt_v2::{
    config::{
        AuditConfig, Config, LoggingConfig, NodeConfig, RawCaptureConfig, ReferenceConfig,
        ReferenceVenueEntry, ReferenceVenueKind, RulesetConfig, RulesetVenueKind,
        StreamingCaptureConfig,
    },
    platform::{
        audit::{
            AuditReceiver, AuditSpoolConfig, AuditUploader, ReferenceVenueSnapshot,
            VenueHealthState, spawn_audit_worker,
        },
        reference::ReferenceSnapshot,
        ruleset::CandidateMarket,
        runtime::{
            CandidateMarketLoadFuture, CandidateMarketLoader, PlatformAuditTaskFactory,
            PlatformRuntimeServices, wire_platform_runtime_with_services,
        },
    },
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig, msgbus::publish_any};
use nautilus_live::node::{LiveNode, LiveNodeHandle, NodeState};
use nautilus_model::identifiers::TraderId;
use support::MockDataClientConfig;
use support::MockDataClientFactory;
use tempfile::tempdir;
use tokio::sync::Notify;

#[derive(Clone, Debug)]
struct UploadCall {
    contents: String,
}

#[derive(Clone, Default)]
struct MockUploader {
    calls: Arc<Mutex<Vec<UploadCall>>>,
}

impl MockUploader {
    fn calls(&self) -> Vec<UploadCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl AuditUploader for MockUploader {
    fn upload_file(
        &self,
        local_path: &Path,
        _s3_uri: &str,
    ) -> impl std::future::Future<Output = Result<()>> + Send {
        let calls = Arc::clone(&self.calls);

        async move {
            let contents = std::fs::read_to_string(local_path)?;
            calls.lock().unwrap().push(UploadCall { contents });
            Ok(())
        }
    }
}

struct RecordingAuditTaskFactory {
    uploader: MockUploader,
}

impl PlatformAuditTaskFactory for RecordingAuditTaskFactory {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        audit_config: AuditSpoolConfig,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        let worker = spawn_audit_worker(audit_rx, self.uploader.clone(), audit_config);
        tokio::spawn(async move {
            cancellation.cancelled().await;
            worker.shutdown().await
        })
    }
}

struct FailingAuditTaskFactory {
    release: Arc<Notify>,
}

impl PlatformAuditTaskFactory for FailingAuditTaskFactory {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        _audit_config: AuditSpoolConfig,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        let release = Arc::clone(&self.release);
        tokio::spawn(async move {
            let _audit_rx = audit_rx;

            tokio::select! {
                _ = cancellation.cancelled() => Ok(()),
                _ = release.notified() => Err(anyhow!("injected audit failure")),
            }
        })
    }
}

struct DroppedReceiverAuditTaskFactory;

impl PlatformAuditTaskFactory for DroppedReceiverAuditTaskFactory {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        _audit_config: AuditSpoolConfig,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        tokio::spawn(async move {
            drop(audit_rx);
            cancellation.cancelled().await;
            Ok(())
        })
    }
}

fn test_config(audit_dir: &Path) -> Config {
    Config {
        node: NodeConfig {
            name: "TEST-NODE".to_string(),
            trader_id: "BOLT-001".to_string(),
            environment: "Live".to_string(),
            load_state: false,
            save_state: false,
            timeout_connection_secs: 1,
            timeout_reconciliation_secs: 1,
            timeout_portfolio_secs: 1,
            timeout_disconnection_secs: 1,
            delay_post_stop_secs: 0,
            delay_shutdown_secs: 0,
        },
        logging: LoggingConfig {
            stdout_level: "Info".to_string(),
            file_level: "Debug".to_string(),
        },
        data_clients: Vec::new(),
        exec_clients: Vec::new(),
        strategies: Vec::new(),
        raw_capture: RawCaptureConfig::default(),
        streaming: StreamingCaptureConfig::default(),
        reference: ReferenceConfig {
            publish_topic: format!(
                "platform.reference.test.{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            min_publish_interval_ms: 0,
            venues: vec![ReferenceVenueEntry {
                name: "BINANCE-BTC".to_string(),
                kind: ReferenceVenueKind::Binance,
                instrument_id: "BTCUSDT.BINANCE".to_string(),
                base_weight: 1.0,
                stale_after_ms: 5_000,
                disable_after_ms: 10_000,
            }],
        },
        rulesets: vec![RulesetConfig {
            id: "PRIMARY".to_string(),
            venue: RulesetVenueKind::Polymarket,
            tag_slug: "bitcoin".to_string(),
            resolution_basis: "binance_btcusdt_1m".to_string(),
            min_time_to_expiry_secs: 60,
            max_time_to_expiry_secs: 900,
            min_liquidity_num: 1_000.0,
            require_accepting_orders: true,
            freeze_before_end_secs: 30,
        }],
        audit: Some(AuditConfig {
            local_dir: audit_dir.to_str().unwrap().to_string(),
            s3_uri: "s3://bucket/audit".to_string(),
            ship_interval_secs: 1,
            roll_max_bytes: 1_048_576,
            roll_max_secs: 300,
            max_local_backlog_bytes: 4 * 1_048_576,
        }),
    }
}

fn build_node() -> LiveNode {
    LiveNode::builder(TraderId::from("BOLT-001"), Environment::Live)
        .unwrap()
        .with_name("TEST-NODE")
        .with_logging(LoggerConfig::default())
        .with_timeout_connection(1)
        .with_timeout_disconnection_secs(1)
        .with_delay_post_stop_secs(0)
        .with_delay_shutdown_secs(0)
        .add_data_client(
            Some("BINANCE".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new("BINANCE", "BINANCE")),
        )
        .unwrap()
        .build()
        .unwrap()
}

fn uploaded_records(uploader: &MockUploader) -> Vec<serde_json::Value> {
    uploader
        .calls()
        .into_iter()
        .flat_map(|call| {
            call.contents
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl_files(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            files.push(path);
        }
    }
}

fn local_records(spool_dir: &Path) -> Vec<serde_json::Value> {
    let mut files = Vec::new();
    collect_jsonl_files(spool_dir, &mut files);
    files.sort();

    files
        .into_iter()
        .flat_map(|path| {
            let Ok(contents) = fs::read_to_string(path) else {
                return Vec::new();
            };

            contents
                .lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn count_kind_records(records: &[serde_json::Value], kind: &str) -> usize {
    records
        .iter()
        .filter(|record| record["kind"] == kind)
        .count()
}

async fn wait_for_kind_record_count(spool_dir: &Path, kind: &str, min_count: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let records = local_records(spool_dir);
            if count_kind_records(&records, kind) >= min_count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_running(handle: &LiveNodeHandle) {
    while !handle.is_running() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn runtime_error(run_result: Result<()>, shutdown_result: Result<()>) -> anyhow::Error {
    match (run_result.err(), shutdown_result.err()) {
        (Some(error), _) => error,
        (_, Some(error)) => error,
        (None, None) => panic!("runtime failure should surface through run or shutdown"),
    }
}

fn services_with_loader(
    candidate_loader: Arc<dyn CandidateMarketLoader>,
    audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
) -> PlatformRuntimeServices {
    PlatformRuntimeServices {
        selector_poll_interval: Duration::from_millis(25),
        candidate_loader,
        audit_task_factory,
        now_ms: Arc::new(|| 1_000),
    }
}

fn services_with(
    markets: Vec<CandidateMarket>,
    audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
) -> PlatformRuntimeServices {
    struct StaticLoader {
        markets: Vec<CandidateMarket>,
    }

    impl CandidateMarketLoader for StaticLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            let markets = self.markets.clone();
            Box::pin(async move { Ok(markets) })
        }
    }

    services_with_loader(Arc::new(StaticLoader { markets }), audit_task_factory)
}

#[tokio::test(flavor = "current_thread")]
async fn platform_runtime_starts_and_stops_with_node() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let uploader = MockUploader::default();
    let services = services_with(
        Vec::new(),
        Arc::new(RecordingAuditTaskFactory {
            uploader: uploader.clone(),
        }),
    );

    let mut node = build_node();
    let handle = node.handle();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    tokio::spawn(async move {
        wait_for_running(&handle).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop();
    });

    node.run().await.unwrap();
    guards.shutdown().await.unwrap();

    assert_eq!(node.state(), NodeState::Stopped);
    assert!(!uploader.calls().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn no_eligible_market_emits_idle_decision_and_keeps_running() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let uploader = MockUploader::default();
    let services = services_with(
        Vec::new(),
        Arc::new(RecordingAuditTaskFactory {
            uploader: uploader.clone(),
        }),
    );

    let mut node = build_node();
    let handle = node.handle();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    let stop_handle = handle.clone();
    tokio::spawn(async move {
        wait_for_running(&handle).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(handle.is_running(), "idle selector must not stop the node");
        stop_handle.stop();
    });

    node.run().await.unwrap();
    guards.shutdown().await.unwrap();

    let records = uploaded_records(&uploader);
    assert!(records.iter().any(|record| {
        record["kind"] == "selector_decision"
            && record["state"] == "idle"
            && record["reason"] == "no eligible market"
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_token_stops_background_tasks_cleanly() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let uploader = MockUploader::default();
    let services = services_with(
        Vec::new(),
        Arc::new(RecordingAuditTaskFactory {
            uploader: uploader.clone(),
        }),
    );

    let mut node = build_node();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    tokio::time::timeout(Duration::from_millis(500), guards.shutdown())
        .await
        .expect("runtime shutdown should not hang")
        .unwrap();

    assert!(uploader.calls().len() <= 1);
}

#[tokio::test(flavor = "current_thread")]
async fn audit_task_failure_triggers_fail_closed_shutdown() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let release = Arc::new(Notify::new());
    let services = services_with(
        Vec::new(),
        Arc::new(FailingAuditTaskFactory {
            release: Arc::clone(&release),
        }),
    );

    let mut node = build_node();
    let handle = node.handle();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    tokio::spawn(async move {
        wait_for_running(&handle).await;
        release.notify_one();
    });

    let run_result = node.run().await;
    assert_eq!(node.state(), NodeState::Stopped);
    let shutdown_result = guards.shutdown().await;
    let error = runtime_error(run_result, shutdown_result);
    assert!(
        error.to_string().contains("platform audit task failed")
            || error.to_string().contains("injected audit failure"),
        "{error:#}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn selector_loader_failure_triggers_fail_closed_shutdown() {
    struct FailingLoader;

    impl CandidateMarketLoader for FailingLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            Box::pin(async { Err(anyhow!("injected selector failure")) })
        }
    }

    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let uploader = MockUploader::default();
    let services = services_with_loader(
        Arc::new(FailingLoader),
        Arc::new(RecordingAuditTaskFactory { uploader }),
    );

    let mut node = build_node();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    let run_result = tokio::time::timeout(Duration::from_secs(1), node.run())
        .await
        .expect("selector failure should stop the node");
    assert_eq!(node.state(), NodeState::Stopped);
    let shutdown_result = guards.shutdown().await;
    let error = runtime_error(run_result, shutdown_result);
    assert!(
        error.to_string().contains("selector polling failed")
            || error.to_string().contains("injected selector failure"),
        "{error:#}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn reference_snapshot_is_forwarded_into_audit_spool() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let publish_topic = cfg.reference.publish_topic.clone();
    let uploader = MockUploader::default();
    let services = services_with(
        Vec::new(),
        Arc::new(RecordingAuditTaskFactory {
            uploader: uploader.clone(),
        }),
    );

    let mut node = build_node();
    let handle = node.handle();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    tokio::spawn(async move {
        wait_for_running(&handle).await;

        let snapshot = ReferenceSnapshot {
            ts_ms: 4_242,
            topic: publish_topic.clone(),
            fair_value: Some(42.5),
            confidence: 0.8,
            venues: vec![
                bolt_v2::platform::reference::EffectiveVenueState {
                    venue_name: "BINANCE-BTC".to_string(),
                    base_weight: 0.7,
                    effective_weight: 0.7,
                    stale: false,
                    health: bolt_v2::platform::reference::VenueHealth::Healthy,
                    observed_price: Some(42.0),
                },
                bolt_v2::platform::reference::EffectiveVenueState {
                    venue_name: "KRAKEN-BTC".to_string(),
                    base_weight: 0.3,
                    effective_weight: 0.0,
                    stale: true,
                    health: bolt_v2::platform::reference::VenueHealth::Disabled {
                        reason: "feed lagging".to_string(),
                    },
                    observed_price: Some(43.0),
                },
            ],
        };
        publish_any(publish_topic.into(), &snapshot);

        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.stop();
    });

    node.run().await.unwrap();
    guards.shutdown().await.unwrap();

    let records = uploaded_records(&uploader);
    assert!(records.iter().any(|record| {
        record["kind"] == "reference_snapshot"
            && record["ts_ms"] == 4_242
            && record["topic"] == cfg.reference.publish_topic
            && record["fair_value"] == 42.5
            && record["confidence"] == 0.8
            && record["venues"]
                == serde_json::to_value(vec![
                    ReferenceVenueSnapshot {
                        venue_name: "BINANCE-BTC".to_string(),
                        base_weight: 0.7,
                        effective_weight: 0.7,
                        stale: false,
                        health: VenueHealthState::Healthy,
                        reason: None,
                        observed_price: Some(42.0),
                    },
                    ReferenceVenueSnapshot {
                        venue_name: "KRAKEN-BTC".to_string(),
                        base_weight: 0.3,
                        effective_weight: 0.0,
                        stale: true,
                        health: VenueHealthState::Disabled,
                        reason: Some("feed lagging".to_string()),
                        observed_price: Some(43.0),
                    },
                ])
                .unwrap()
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn background_producers_stop_emitting_before_runtime_shutdown() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let publish_topic = cfg.reference.publish_topic.clone();
    let services = services_with(
        Vec::new(),
        Arc::new(RecordingAuditTaskFactory {
            uploader: MockUploader::default(),
        }),
    );

    let mut node = build_node();
    let handle = node.handle();
    let stop_handle = handle.clone();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    tokio::spawn(async move {
        wait_for_running(&handle).await;

        publish_any(
            publish_topic.clone().into(),
            &ReferenceSnapshot {
                ts_ms: 6_000,
                topic: publish_topic,
                fair_value: Some(12.5),
                confidence: 0.7,
                venues: Vec::new(),
            },
        );

        tokio::time::sleep(Duration::from_millis(60)).await;
        stop_handle.stop();
    });

    node.run().await.unwrap();

    wait_for_kind_record_count(dir.path(), "selector_decision", 1).await;
    wait_for_kind_record_count(dir.path(), "reference_snapshot", 1).await;

    let before_gap_records = local_records(dir.path());
    let selector_before_gap = count_kind_records(&before_gap_records, "selector_decision");
    let snapshot_before_gap = count_kind_records(&before_gap_records, "reference_snapshot");

    for ts_ms in [7_001_u64, 7_002, 7_003] {
        publish_any(
            cfg.reference.publish_topic.clone().into(),
            &ReferenceSnapshot {
                ts_ms,
                topic: cfg.reference.publish_topic.clone(),
                fair_value: Some(99.0),
                confidence: 0.5,
                venues: Vec::new(),
            },
        );
    }

    tokio::time::sleep(Duration::from_millis(90)).await;

    let after_gap_records = local_records(dir.path());
    assert_eq!(
        count_kind_records(&after_gap_records, "selector_decision"),
        selector_before_gap,
        "selector loop should stop emitting after node stop and before runtime shutdown"
    );
    assert_eq!(
        count_kind_records(&after_gap_records, "reference_snapshot"),
        snapshot_before_gap,
        "snapshot forwarding should stop emitting after node stop and before runtime shutdown"
    );

    guards.shutdown().await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn reference_snapshot_audit_send_failure_surfaces_through_shutdown() {
    struct PendingLoader;

    impl CandidateMarketLoader for PendingLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            Box::pin(std::future::pending())
        }
    }

    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let publish_topic = cfg.reference.publish_topic.clone();
    let services = services_with_loader(
        Arc::new(PendingLoader),
        Arc::new(DroppedReceiverAuditTaskFactory),
    );

    let mut node = build_node();
    let handle = node.handle();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    tokio::spawn(async move {
        wait_for_running(&handle).await;

        for _ in 0..50 {
            publish_any(
                publish_topic.clone().into(),
                &ReferenceSnapshot {
                    ts_ms: 5_001,
                    topic: "reference.test".to_string(),
                    fair_value: Some(1.5),
                    confidence: 0.9,
                    venues: Vec::new(),
                },
            );

            if !handle.is_running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    let run_result = tokio::time::timeout(Duration::from_secs(1), node.run())
        .await
        .expect("snapshot audit send failure should stop the node");
    assert_eq!(node.state(), NodeState::Stopped);
    let shutdown_result = guards.shutdown().await;
    let error = runtime_error(run_result, shutdown_result);
    assert!(
        error
            .to_string()
            .contains("reference snapshot audit send failed"),
        "{error:#}"
    );
}
