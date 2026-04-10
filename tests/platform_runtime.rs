mod support;

use std::{
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
        audit::{AuditReceiver, AuditSpoolConfig, AuditUploader, spawn_audit_worker},
        ruleset::CandidateMarket,
        runtime::{
            PlatformAuditTaskFactory, PlatformRuntimeServices, wire_platform_runtime_with_services,
        },
    },
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::{LiveNode, NodeState};
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
        _audit_rx: AuditReceiver,
        _audit_config: AuditSpoolConfig,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        let release = Arc::clone(&self.release);
        tokio::spawn(async move {
            release.notified().await;
            Err(anyhow!("injected audit failure"))
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

fn services_with(
    markets: Vec<CandidateMarket>,
    audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
) -> PlatformRuntimeServices {
    struct StaticLoader {
        markets: Vec<CandidateMarket>,
    }

    impl bolt_v2::platform::runtime::CandidateMarketLoader for StaticLoader {
        fn load(
            &self,
            _ruleset: RulesetConfig,
        ) -> bolt_v2::platform::runtime::CandidateMarketLoadFuture {
            let markets = self.markets.clone();
            Box::pin(async move { Ok(markets) })
        }
    }

    PlatformRuntimeServices {
        selector_poll_interval: Duration::from_millis(25),
        candidate_loader: Arc::new(StaticLoader { markets }),
        audit_task_factory,
        now_ms: Arc::new(|| 1_000),
    }
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
        while !handle.is_running() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

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
        while !handle.is_running() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

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
        while !handle.is_running() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        release.notify_waiters();
    });

    let run_result = node.run().await;
    assert_eq!(node.state(), NodeState::Stopped);
    let shutdown_result = guards.shutdown().await;
    let error = match (run_result.err(), shutdown_result.err()) {
        (Some(error), _) => error,
        (_, Some(error)) => error,
        (None, None) => panic!("audit failure should surface through run or shutdown"),
    };
    assert!(
        error.to_string().contains("platform audit task failed")
            || error.to_string().contains("injected audit failure"),
        "{error:#}"
    );
}
