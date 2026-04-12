mod support;

use std::{collections::VecDeque, fs, future::Future, path::Path, sync::{Arc, Mutex}, time::Duration};

use anyhow::{Result, anyhow};
use bolt_v2::{
    config::{
        AuditConfig, Config, ExecClientEntry, ExecClientSecrets, LoggingConfig, NodeConfig,
        RawCaptureConfig, ReferenceConfig, ReferenceVenueEntry, ReferenceVenueKind, RulesetConfig,
        RulesetVenueKind, StrategyEntry, StreamingCaptureConfig,
    },
    platform::{
        audit::{
            AuditReceiver, AuditSpoolConfig, AuditUploader, ReferenceVenueSnapshot,
            VenueHealthState, spawn_audit_worker,
        },
        reference::ReferenceSnapshot,
        resolution_basis::{CandleInterval, ResolutionBasis, ResolutionSourceKind},
        ruleset::CandidateMarket,
        runtime::{
            CandidateMarketLoadFuture, CandidateMarketLoader, PlatformAuditTaskFactory,
            PlatformRuntimeServices, wire_platform_runtime_with_services_and_registry,
        },
    },
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig, msgbus::publish_any};
use nautilus_live::node::{LiveNode, LiveNodeHandle, NodeState};
use nautilus_model::identifiers::TraderId;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    stub_runtime_strategy::{
        clear_stub_runtime_observations, stub_runtime_build_count, stub_runtime_snapshots,
    },
    test_strategy_build_context, test_strategy_registry,
};
use tempfile::tempdir;
use tokio::{sync::Notify, task::LocalSet};

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
    configs: Arc<Mutex<Vec<AuditSpoolConfig>>>,
}

impl RecordingAuditTaskFactory {
    fn new(uploader: MockUploader) -> Self {
        Self {
            uploader,
            configs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn configs(&self) -> Vec<AuditSpoolConfig> {
        self.configs.lock().unwrap().clone()
    }
}

impl PlatformAuditTaskFactory for RecordingAuditTaskFactory {
    fn spawn(
        &self,
        audit_rx: AuditReceiver,
        audit_config: AuditSpoolConfig,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        self.configs.lock().unwrap().push(audit_config.clone());
        let worker = spawn_audit_worker(audit_rx, self.uploader.clone(), audit_config);
        tokio::task::spawn_local(async move {
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
        tokio::task::spawn_local(async move {
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
            chainlink: None,
            venues: vec![ReferenceVenueEntry {
                name: "BINANCE-BTC".to_string(),
                kind: ReferenceVenueKind::Binance,
                instrument_id: "BTCUSDT.BINANCE".to_string(),
                base_weight: 1.0,
                stale_after_ms: 5_000,
                disable_after_ms: 10_000,
                chainlink: None,
            }],
        },
        rulesets: vec![RulesetConfig {
            id: "PRIMARY".to_string(),
            venue: RulesetVenueKind::Polymarket,
            tag_slug: "bitcoin".to_string(),
            event_slug_prefix: "btc-updown-5m-".to_string(),
            resolution_basis: "binance_btcusdt_1m".to_string(),
            min_time_to_expiry_secs: 60,
            max_time_to_expiry_secs: 900,
            min_liquidity_num: 1_000.0,
            require_accepting_orders: true,
            freeze_before_end_secs: 90,
            selector_poll_interval_ms: 25,
            candidate_load_timeout_secs: 7,
        }],
        audit: Some(AuditConfig {
            local_dir: audit_dir.to_str().unwrap().to_string(),
            s3_uri: "s3://bucket/audit".to_string(),
            ship_interval_secs: 1,
            upload_attempt_timeout_secs: 13,
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

fn lifecycle_test_config(audit_dir: &Path) -> Config {
    let mut cfg = test_config(audit_dir);
    cfg.exec_clients = vec![ExecClientEntry {
        name: "TEST".to_string(),
        kind: "mock_exec".to_string(),
        config: toml::toml! {
            client_id = "TEST"
            account_id = "TEST-ACCOUNT"
            venue = "POLYMARKET"
        }
        .into(),
        secrets: ExecClientSecrets {
            region: "us-east-1".to_string(),
            pk: None,
            api_key: None,
            api_secret: None,
            passphrase: None,
        },
    }];
    cfg.strategies = vec![StrategyEntry {
        kind: "stub_runtime".to_string(),
        config: toml::toml! {
            strategy_id = "RUNTIME-STUB-001"
        }
        .into(),
    }];
    cfg
}

fn build_lifecycle_node() -> LiveNode {
    LiveNode::builder(TraderId::from("BOLT-001"), Environment::Live)
        .unwrap()
        .with_name("TEST-NODE")
        .with_logging(LoggerConfig::default())
        .with_reconciliation(false)
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
        .add_data_client(
            Some("TEST".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new("TEST", "POLYMARKET")),
        )
        .unwrap()
        .add_exec_client(
            Some("TEST".to_string()),
            Box::new(MockExecutionClientFactory),
            Box::new(MockExecClientConfig::new(
                "TEST",
                "TEST-ACCOUNT",
                "POLYMARKET",
            )),
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

fn selector_poll_budget(poll_interval_ms: u64) -> Duration {
    Duration::from_millis(poll_interval_ms.saturating_mul(12)).max(Duration::from_millis(500))
}

async fn wait_for_selector_state(
    spool_dir: &Path,
    state: &str,
    min_count: usize,
    timeout: Duration,
) {
    tokio::time::timeout(timeout, async {
        loop {
            let matching = local_records(spool_dir)
                .into_iter()
                .filter(|record| record["kind"] == "selector_decision" && record["state"] == state)
                .count();
            if matching >= min_count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_condition_or_stop<F>(
    timeout: Duration,
    stop_handle: &LiveNodeHandle,
    description: &str,
    mut condition: F,
) -> Result<()>
where
    F: FnMut() -> bool,
{
    match tokio::time::timeout(timeout, async {
        loop {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    {
        Ok(()) => Ok(()),
        Err(_) => {
            stop_handle.stop();
            Err(anyhow!("timed out waiting for {description}"))
        }
    }
}

async fn wait_for_running(handle: &LiveNodeHandle) {
    while !handle.is_running() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn run_multithread_localset_test<F>(test: F)
where
    F: Future<Output = ()> + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = LocalSet::new();

    runtime.block_on(local.run_until(test));
}

fn wire_platform_runtime_with_services(
    node: &mut LiveNode,
    cfg: &Config,
    services: PlatformRuntimeServices,
) -> anyhow::Result<bolt_v2::platform::runtime::PlatformRuntimeGuards> {
    let registry = test_strategy_registry();
    let build_context = test_strategy_build_context();
    wire_platform_runtime_with_services_and_registry(node, cfg, services, &registry, &build_context)
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

struct SequencedLoader {
    responses: Arc<Mutex<VecDeque<Vec<CandidateMarket>>>>,
    fallback: Vec<CandidateMarket>,
}

impl SequencedLoader {
    fn new(responses: Vec<Vec<CandidateMarket>>) -> Self {
        let fallback = responses.last().cloned().unwrap_or_default();
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            fallback,
        }
    }
}

impl CandidateMarketLoader for SequencedLoader {
    fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
        let next = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone());
        Box::pin(async move { Ok(next) })
    }
}

#[test]
fn platform_runtime_starts_and_stops_with_node() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let uploader = MockUploader::default();
        let audit_task_factory = Arc::new(RecordingAuditTaskFactory::new(uploader.clone()));
        let services = services_with(Vec::new(), audit_task_factory.clone());

        let mut node = build_node();
        let handle = node.handle();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::task::spawn_local(async move {
            wait_for_running(&handle).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();

        assert_eq!(node.state(), NodeState::Stopped);
        assert!(!uploader.calls().is_empty());
        assert_eq!(
            audit_task_factory.configs()[0].upload_attempt_timeout,
            Duration::from_secs(cfg.audit.as_ref().unwrap().upload_attempt_timeout_secs)
        );
    });
}

#[test]
fn no_eligible_market_emits_idle_decision_and_keeps_running() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let uploader = MockUploader::default();
        let services = services_with(
            Vec::new(),
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
        );

        let mut node = build_node();
        let handle = node.handle();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let stop_handle = handle.clone();
        tokio::task::spawn_local(async move {
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
    });
}

fn candidate_market(
    market_id: &str,
    instrument_id: &str,
    liquidity_num: f64,
    seconds_to_end: u64,
) -> CandidateMarket {
    CandidateMarket {
        market_id: market_id.to_string(),
        instrument_id: instrument_id.to_string(),
        condition_id: format!("{market_id}-condition"),
        up_token_id: format!("{market_id}-up"),
        down_token_id: format!("{market_id}-down"),
        start_ts_ms: Some(1_744_444_800_000),
        tag_slug: "bitcoin".to_string(),
        declared_resolution_basis: binance_btcusdt_1m(),
        accepting_orders: true,
        liquidity_num,
        seconds_to_end,
    }
}

fn binance_btcusdt_1m() -> ResolutionBasis {
    ResolutionBasis::ExchangeCandle {
        source: ResolutionSourceKind::Binance,
        pair: "btcusdt".to_string(),
        interval: CandleInterval::OneMinute,
    }
}

#[test]
fn selector_runtime_emits_reject_records_with_final_decision_for_same_tick() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 1_000;
        let uploader = MockUploader::default();
        let services = services_with(
            vec![
                CandidateMarket {
                    market_id: "mkt-low-liquidity".to_string(),
                    instrument_id: "LOW_LIQ.POLYMARKET".to_string(),
                    condition_id: "mkt-low-liquidity-condition".to_string(),
                    up_token_id: "mkt-low-liquidity-up".to_string(),
                    down_token_id: "mkt-low-liquidity-down".to_string(),
                    start_ts_ms: Some(1_744_444_800_000),
                    tag_slug: "bitcoin".to_string(),
                    declared_resolution_basis: binance_btcusdt_1m(),
                    accepting_orders: true,
                    liquidity_num: 999.0,
                    seconds_to_end: 120,
                },
                candidate_market("mkt-active", "ACTIVE.POLYMARKET", 2_000.0, 120),
            ],
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
        );

        let mut node = build_node();
        let handle = node.handle();
        let stop_handle = handle.clone();
        let spool_dir = dir.path().to_path_buf();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::task::spawn_local(async move {
            wait_for_running(&handle).await;
            wait_for_kind_record_count(&spool_dir, "eligibility_reject", 1).await;
            wait_for_kind_record_count(&spool_dir, "selector_decision", 1).await;
            stop_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();

        let records = uploaded_records(&uploader);
        assert_eq!(count_kind_records(&records, "eligibility_reject"), 1);
        assert_eq!(count_kind_records(&records, "selector_decision"), 1);

        let reject_record = records.iter().find(|record| {
            record["kind"] == "eligibility_reject"
                && record["ruleset_id"] == "PRIMARY"
                && record["market_id"] == "mkt-low-liquidity"
                && record["instrument_id"] == "LOW_LIQ.POLYMARKET"
                && record["reason"] == "low_liquidity"
        });
        let decision_record = records.iter().find(|record| {
            record["kind"] == "selector_decision"
                && record["state"] == "active"
                && record["ruleset_id"] == "PRIMARY"
                && record["market_id"] == "mkt-active"
                && record["instrument_id"] == "ACTIVE.POLYMARKET"
        });

        assert!(
            reject_record.is_some(),
            "expected eligibility reject audit record, got {records:?}"
        );
        assert!(
            decision_record.is_some(),
            "expected selector decision audit record, got {records:?}"
        );
        assert_eq!(reject_record.unwrap()["ts_ms"], 1_000);
        assert_eq!(decision_record.unwrap()["ts_ms"], 1_000);
    });
}

#[test]
fn eligible_market_emits_active_decision() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let uploader = MockUploader::default();
        let services = services_with(
            vec![candidate_market(
                "mkt-active",
                "ACTIVE.POLYMARKET",
                2_000.0,
                120,
            )],
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
        );

        let mut node = build_node();
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::task::spawn_local(async move {
            wait_for_running(&handle).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();

        let records = uploaded_records(&uploader);
        assert!(records.iter().any(|record| {
            record["kind"] == "selector_decision"
                && record["state"] == "active"
                && record["ruleset_id"] == "PRIMARY"
                && record["market_id"] == "mkt-active"
                && record["instrument_id"] == "ACTIVE.POLYMARKET"
                && record["reason"].is_null()
        }));
    });
}

#[test]
fn active_selector_state_registers_exactly_one_runtime_strategy() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = lifecycle_test_config(dir.path());
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let activation_budget = Duration::from_secs(1);
        let spool_dir = dir.path().to_path_buf();
        let services = services_with(
            vec![candidate_market(
                "mkt-active-runtime",
                "ACTIVE-RT.POLYMARKET",
                2_000.0,
                120,
            )],
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
        );

        let mut node = build_lifecycle_node();
        let trader = std::rc::Rc::clone(node.kernel().trader());
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "active", 1, poll_budget).await;
            wait_for_condition_or_stop(
                activation_budget,
                &stop_handle,
                "exactly one runtime-managed strategy for an active selector state",
                || trader.borrow().strategy_ids().len() == 1,
            )
            .await?;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        run_result.unwrap();
        guards.shutdown().await.unwrap();
        control_result.unwrap();

        assert_eq!(trader.borrow().strategy_ids().len(), 1);
    });
}

#[test]
fn freeze_window_market_emits_freeze_decision() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let uploader = MockUploader::default();
        let services = services_with(
            vec![candidate_market(
                "mkt-freeze",
                "FREEZE.POLYMARKET",
                2_000.0,
                60,
            )],
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
        );

        let mut node = build_node();
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::spawn(async move {
            wait_for_running(&handle).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            stop_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();

        let records = uploaded_records(&uploader);
        assert!(records.iter().any(|record| {
            record["kind"] == "selector_decision"
                && record["state"] == "freeze"
                && record["ruleset_id"] == "PRIMARY"
                && record["market_id"] == "mkt-freeze"
                && record["instrument_id"] == "FREEZE.POLYMARKET"
                && record["reason"] == "freeze window"
        }));
    });
}

#[test]
fn no_eligible_market_keeps_persistent_runtime_strategy_registered() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let spool_dir = dir.path().to_path_buf();
        let cfg = lifecycle_test_config(dir.path());
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let services = services_with(
            Vec::new(),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
        );

        let mut node = build_lifecycle_node();
        let trader = std::rc::Rc::clone(node.kernel().trader());
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "idle", 1, poll_budget).await;
            assert!(
                trader.borrow().strategy_ids().len() == 1,
                "persistent runtime strategy should stay registered even when no market is eligible"
            );
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        run_result.unwrap();
        guards.shutdown().await.unwrap();
        control_result.unwrap();

        assert_eq!(trader.borrow().strategy_ids().len(), 1);
    });
}

#[test]
fn runtime_strategy_persists_across_switch_idle_and_resume_snapshots() {
    run_multithread_localset_test(async {
        clear_stub_runtime_observations();

        let dir = tempdir().unwrap();
        let spool_dir = dir.path().to_path_buf();
        let mut cfg = lifecycle_test_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 10;
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let strategy_id = "RUNTIME-STUB-001";
        let services = services_with_loader(
            Arc::new(SequencedLoader::new(vec![
                vec![candidate_market("mkt-active-a", "ACTIVE-A.POLYMARKET", 2_000.0, 120)],
                vec![candidate_market("mkt-active-b", "ACTIVE-B.POLYMARKET", 2_000.0, 120)],
                Vec::new(),
                vec![candidate_market("mkt-active-c", "ACTIVE-C.POLYMARKET", 2_000.0, 120)],
            ])),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
        );

        let mut node = build_lifecycle_node();
        let trader = std::rc::Rc::clone(node.kernel().trader());
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "active", 3, poll_budget).await;
            wait_for_selector_state(&spool_dir, "idle", 1, poll_budget).await;
            wait_for_condition_or_stop(
                Duration::from_secs(1),
                &stop_handle,
                "persistent runtime strategy snapshot sequence",
                || {
                    stub_runtime_build_count(strategy_id) == 1
                        && stub_runtime_snapshots(strategy_id).len() >= 4
                        && trader.borrow().strategy_ids().len() == 1
                },
            )
            .await?;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        run_result.unwrap();
        guards.shutdown().await.unwrap();
        control_result.unwrap();

        assert_eq!(stub_runtime_build_count(strategy_id), 1);
        assert_eq!(trader.borrow().strategy_ids().len(), 1);

        let snapshots = stub_runtime_snapshots(strategy_id);
        assert_eq!(snapshots.len(), 4);
        assert_eq!(
            snapshots
                .iter()
                .map(|snapshot| match &snapshot.decision.state {
                    bolt_v2::platform::ruleset::SelectionState::Active { market } => {
                        format!("active:{}", market.instrument_id)
                    }
                    bolt_v2::platform::ruleset::SelectionState::Idle { .. } => "idle".to_string(),
                    bolt_v2::platform::ruleset::SelectionState::Freeze { market, .. } => {
                        format!("freeze:{}", market.instrument_id)
                    }
                })
                .collect::<Vec<_>>(),
            vec![
                "active:ACTIVE-A.POLYMARKET".to_string(),
                "active:ACTIVE-B.POLYMARKET".to_string(),
                "idle".to_string(),
                "active:ACTIVE-C.POLYMARKET".to_string(),
            ]
        );
    });
}

#[test]
fn ruleset_mode_rejects_duplicate_runtime_strategy_templates() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = lifecycle_test_config(dir.path());
        cfg.strategies.push(StrategyEntry {
            kind: "stub_runtime".to_string(),
            config: toml::toml! {
                strategy_id = "RUNTIME-STUB-002"
            }
            .into(),
        });
        let services = services_with(
            Vec::new(),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
        );

        let mut node = build_lifecycle_node();
        let error = match wire_platform_runtime_with_services(&mut node, &cfg, services) {
            Ok(_) => panic!("ruleset mode should reject duplicate runtime strategy templates"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("at most one registered strategy template"),
            "{error}"
        );
    });
}

#[test]
fn cancellation_token_stops_background_tasks_cleanly() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let uploader = MockUploader::default();
        let services = services_with(
            Vec::new(),
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
        );

        let mut node = build_node();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::time::timeout(Duration::from_millis(500), guards.shutdown())
            .await
            .expect("runtime shutdown should not hang")
            .unwrap();

        assert!(uploader.calls().len() <= 1);
    });
}

#[test]
fn audit_task_failure_triggers_fail_closed_shutdown() {
    run_multithread_localset_test(async {
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
    });
}

#[test]
fn selector_loader_failure_triggers_fail_closed_shutdown() {
    struct FailingLoader;

    impl CandidateMarketLoader for FailingLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            Box::pin(async { Err(anyhow!("injected selector failure")) })
        }
    }

    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let uploader = MockUploader::default();
        let services = services_with_loader(
            Arc::new(FailingLoader),
            Arc::new(RecordingAuditTaskFactory::new(uploader)),
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
    });
}

#[test]
fn runtime_passes_ruleset_loader_timeout_through_config() {
    #[derive(Default)]
    struct CapturingLoader {
        seen_rulesets: Arc<Mutex<Vec<RulesetConfig>>>,
    }

    impl CapturingLoader {
        fn seen_rulesets(&self) -> Vec<RulesetConfig> {
            self.seen_rulesets.lock().unwrap().clone()
        }
    }

    impl CandidateMarketLoader for CapturingLoader {
        fn load(&self, ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            self.seen_rulesets.lock().unwrap().push(ruleset);
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = test_config(dir.path());
        cfg.rulesets[0].candidate_load_timeout_secs = 42;
        let loader = Arc::new(CapturingLoader::default());
        let uploader = MockUploader::default();
        let services = services_with_loader(
            loader.clone(),
            Arc::new(RecordingAuditTaskFactory::new(uploader)),
        );

        let mut node = build_node();
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::spawn(async move {
            wait_for_running(&handle).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
            stop_handle.stop();
        });

        node.run().await.unwrap();
        guards.shutdown().await.unwrap();

        assert!(
            loader
                .seen_rulesets()
                .iter()
                .any(|ruleset| ruleset.candidate_load_timeout_secs == 42),
            "selector loader should receive the configured candidate load timeout"
        );
    });
}

#[test]
fn reference_snapshot_is_forwarded_into_audit_spool() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let publish_topic = cfg.reference.publish_topic.clone();
        let uploader = MockUploader::default();
        let services = services_with(
            Vec::new(),
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
        );

        let mut node = build_node();
        let handle = node.handle();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        tokio::task::spawn_local(async move {
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
                        observed_ts_ms: Some(4_200),
                        venue_kind: bolt_v2::platform::reference::VenueKind::Orderbook,
                        observed_price: Some(42.0),
                        observed_bid: Some(41.9),
                        observed_ask: Some(42.1),
                    },
                    bolt_v2::platform::reference::EffectiveVenueState {
                        venue_name: "KRAKEN-BTC".to_string(),
                        base_weight: 0.3,
                        effective_weight: 0.0,
                        stale: true,
                        health: bolt_v2::platform::reference::VenueHealth::Disabled {
                            reason: "feed lagging".to_string(),
                        },
                        observed_ts_ms: Some(4_100),
                        venue_kind: bolt_v2::platform::reference::VenueKind::Oracle,
                        observed_price: Some(43.0),
                        observed_bid: None,
                        observed_ask: None,
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
                            observed_ts_ms: Some(4_200),
                            venue_kind: bolt_v2::platform::audit::VenueKindState::Orderbook,
                            observed_price: Some(42.0),
                            observed_bid: Some(41.9),
                            observed_ask: Some(42.1),
                        },
                        ReferenceVenueSnapshot {
                            venue_name: "KRAKEN-BTC".to_string(),
                            base_weight: 0.3,
                            effective_weight: 0.0,
                            stale: true,
                            health: VenueHealthState::Disabled,
                            reason: Some("feed lagging".to_string()),
                            observed_ts_ms: Some(4_100),
                            venue_kind: bolt_v2::platform::audit::VenueKindState::Oracle,
                            observed_price: Some(43.0),
                            observed_bid: None,
                            observed_ask: None,
                        },
                    ])
                    .unwrap()
        }));
    });
}

#[test]
fn background_producers_stop_emitting_before_runtime_shutdown() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let cfg = test_config(dir.path());
        let publish_topic = cfg.reference.publish_topic.clone();
        let services = services_with(
            Vec::new(),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
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
    });
}

#[test]
fn reference_snapshot_audit_send_failure_surfaces_through_shutdown() {
    struct PendingLoader;

    impl CandidateMarketLoader for PendingLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            Box::pin(std::future::pending())
        }
    }

    run_multithread_localset_test(async {
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

        tokio::task::spawn_local(async move {
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
    });
}
