mod support;

use std::{
    any::Any,
    cell::RefCell,
    collections::VecDeque,
    fs,
    future::Future,
    path::Path,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
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
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionState},
        runtime::{
            CandidateMarketLoadFuture, CandidateMarketLoader, PlatformAuditTaskFactory,
            PlatformRuntimeServices, RuntimeStrategyFactory, runtime_selection_topic,
            wire_platform_runtime_with_services,
        },
    },
    strategies::registry::StrategyBuilder,
};
use nautilus_common::factories::{ClientConfig, DataClientFactory};
use nautilus_common::{
    cache::Cache,
    clients::DataClient,
    clock::Clock,
    component::Component,
    enums::Environment,
    logging::logger::LoggerConfig,
    msgbus::{ShareableMessageHandler, publish_any, subscribe_any, unsubscribe_any},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::{LiveNode, LiveNodeHandle, NodeState};
use nautilus_model::{
    enums::{LiquiditySide, OmsType, OrderSide, OrderType},
    events::OrderFilled,
    identifiers::{
        AccountId, ClientId, ClientOrderId, PositionId, StrategyId, TradeId, TraderId, Venue,
        VenueOrderId,
    },
    instruments::{Instrument, InstrumentAny, stubs::binary_option},
    position::Position,
    types::{Currency, Money, Price, Quantity},
};
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    stub_runtime_strategy::{StubRuntimeStrategy, StubRuntimeStrategyBuilder},
};
use toml::Value;

fn polymarket_selector(tag_slug: &str) -> Value {
    let mut selector = toml::map::Map::new();
    selector.insert("tag_slug".to_string(), Value::String(tag_slug.to_string()));
    Value::Table(selector)
}
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
        exec_engine: bolt_v2::config::ExecEngineConfig::default(),
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
            binance: None,
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
            selector: polymarket_selector("bitcoin"),
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
    cfg.strategies = Vec::new();
    cfg
}

fn stub_runtime_lifecycle_config(audit_dir: &Path) -> Config {
    let mut cfg = lifecycle_test_config(audit_dir);
    cfg.strategies = vec![StrategyEntry {
        kind: StubRuntimeStrategyBuilder::kind().to_string(),
        config: toml::toml! {
            strategy_id = "STUB-RUNTIME-001"
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

#[derive(Debug)]
struct DelayedDataClientConfig {
    client_id: String,
    venue: String,
}

impl DelayedDataClientConfig {
    fn new(client_id: &str, venue: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            venue: venue.to_string(),
        }
    }
}

impl ClientConfig for DelayedDataClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
struct DelayedDataClientFactory {
    release: Arc<Notify>,
}

impl DataClientFactory for DelayedDataClientFactory {
    fn create(
        &self,
        _name: &str,
        config: &dyn ClientConfig,
        _cache: Rc<RefCell<Cache>>,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let cfg = config
            .as_any()
            .downcast_ref::<DelayedDataClientConfig>()
            .ok_or_else(|| anyhow!("DelayedDataClientFactory received wrong config type"))?;

        Ok(Box::new(DelayedDataClient {
            client_id: ClientId::from(cfg.client_id.as_str()),
            venue: Venue::from(cfg.venue.as_str()),
            release: Arc::clone(&self.release),
            connected: false,
        }))
    }

    fn name(&self) -> &str {
        "delayed-mock-data"
    }

    fn config_type(&self) -> &str {
        "DelayedDataClientConfig"
    }
}

#[derive(Debug)]
struct DelayedDataClient {
    client_id: ClientId,
    venue: Venue,
    release: Arc<Notify>,
    connected: bool,
}

#[async_trait(?Send)]
impl DataClient for DelayedDataClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<Venue> {
        Some(self.venue)
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn is_disconnected(&self) -> bool {
        !self.connected
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        self.release.notified().await;
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }
}

fn build_delayed_start_node(release: Arc<Notify>) -> LiveNode {
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
            Box::new(DelayedDataClientFactory { release }),
            Box::new(DelayedDataClientConfig::new("BINANCE", "BINANCE")),
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
        runtime_strategy_factory: Arc::new(|_trader, kind, _raw_config: &toml::Value| {
            Err(anyhow!(
                "unexpected runtime strategy build through zero-template services for kind {kind}"
            ))
        }),
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

fn seed_open_position_for_strategy(node: &LiveNode, strategy_id: StrategyId) {
    let instrument = InstrumentAny::BinaryOption(binary_option());
    let mut fill = OrderFilled::new(
        TraderId::from("BOLT-001"),
        strategy_id,
        instrument.id(),
        ClientOrderId::from("O-PREEMPT-001"),
        VenueOrderId::from("V-PREEMPT-001"),
        AccountId::from("TEST-ACCOUNT"),
        TradeId::from("E-PREEMPT-001"),
        OrderSide::Buy,
        OrderType::Market,
        Quantity::from("1"),
        Price::from("0.550"),
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        UnixNanos::default(),
        UnixNanos::default(),
        false,
        None,
        Some(Money::from("0.01 USDC")),
    );
    fill.position_id = Some(PositionId::from("P-PREEMPT-001"));

    let position = Position::new(&instrument, fill);
    let cache = node.kernel().cache();
    let mut cache = cache.borrow_mut();
    cache.add_instrument(instrument).unwrap();
    cache.add_position(&position, OmsType::Netting).unwrap();
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

fn stub_runtime_factory(builds: Arc<Mutex<Vec<String>>>) -> RuntimeStrategyFactory {
    Arc::new(move |trader, _kind, raw_config: &toml::Value| {
        let strategy_id = raw_config
            .get("strategy_id")
            .and_then(toml::Value::as_str)
            .context("stub runtime strategy requires strategy_id")?;
        builds.lock().unwrap().push(strategy_id.to_string());
        let strategy = StubRuntimeStrategy::new(strategy_id);
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());

        trader.borrow_mut().add_strategy(strategy)?;

        Ok(strategy_id)
    })
}

fn stub_runtime_services_with_loader(
    candidate_loader: Arc<dyn CandidateMarketLoader>,
    audit_task_factory: Arc<dyn PlatformAuditTaskFactory>,
    builds: Arc<Mutex<Vec<String>>>,
) -> PlatformRuntimeServices {
    PlatformRuntimeServices {
        candidate_loader,
        audit_task_factory,
        now_ms: Arc::new(|| 1_000),
        runtime_strategy_factory: stub_runtime_factory(builds),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn platform_runtime_starts_and_stops_with_node() {
    let dir = tempdir().unwrap();
    let cfg = test_config(dir.path());
    let uploader = MockUploader::default();
    let audit_task_factory = Arc::new(RecordingAuditTaskFactory::new(uploader.clone()));
    let services = services_with(Vec::new(), audit_task_factory.clone());

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
    assert_eq!(
        audit_task_factory.configs()[0].upload_attempt_timeout,
        Duration::from_secs(cfg.audit.as_ref().unwrap().upload_attempt_timeout_secs)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn no_eligible_market_emits_idle_decision_and_keeps_running() {
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

fn candidate_market(
    market_id: &str,
    instrument_id: &str,
    liquidity_num: f64,
    seconds_to_end: u64,
) -> CandidateMarket {
    let base = market_id.replace('-', "");
    CandidateMarket {
        market_id: market_id.to_string(),
        market_slug: market_id.to_string(),
        question_id: format!("question-{market_id}"),
        instrument_id: instrument_id.to_string(),
        condition_id: format!("0x{base}"),
        up_token_id: format!("{base}01"),
        down_token_id: format!("{base}02"),
        price_to_beat: None,
        price_to_beat_source: None,
        price_to_beat_observed_ts_ms: None,
        start_ts_ms: 1_700_000_000_000,
        end_ts_ms: 1_700_000_300_000,
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

#[tokio::test(flavor = "current_thread")]
async fn selector_runtime_emits_reject_records_with_final_decision_for_same_tick() {
    let dir = tempdir().unwrap();
    let mut cfg = test_config(dir.path());
    cfg.rulesets[0].selector_poll_interval_ms = 1_000;
    let uploader = MockUploader::default();
    let services = services_with(
        vec![
            CandidateMarket {
                market_id: "mkt-low-liquidity".to_string(),
                market_slug: "mkt-low-liquidity".to_string(),
                question_id: "question-mkt-low-liquidity".to_string(),
                instrument_id: "LOW_LIQ.POLYMARKET".to_string(),
                condition_id: "0xmktlowliquidity".to_string(),
                up_token_id: "mktlowliquidity01".to_string(),
                down_token_id: "mktlowliquidity02".to_string(),
                price_to_beat: None,
                price_to_beat_source: None,
                price_to_beat_observed_ts_ms: None,
                start_ts_ms: 1_700_000_000_000,
                end_ts_ms: 1_700_000_300_000,
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

    tokio::spawn(async move {
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
}

#[tokio::test(flavor = "current_thread")]
async fn eligible_market_emits_active_decision() {
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
            && record["state"] == "active"
            && record["ruleset_id"] == "PRIMARY"
            && record["market_id"] == "mkt-active"
            && record["instrument_id"] == "ACTIVE.POLYMARKET"
            && record["reason"].is_null()
    }));
}

#[test]
fn selector_keeps_loading_and_publishing_active_decision_while_positions_open() {
    struct CountingLoader {
        calls: Arc<AtomicUsize>,
        market: CandidateMarket,
    }

    impl CandidateMarketLoader for CountingLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let market = self.market.clone();
            Box::pin(async move { Ok(vec![market]) })
        }
    }

    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let spool_dir = dir.path().to_path_buf();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 10;
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let builds = Arc::new(Mutex::new(Vec::<String>::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let topic = runtime_selection_topic(&StrategyId::from("STUB-RUNTIME-001"));
        let observed = Rc::new(RefCell::new(Vec::<RuntimeSelectionSnapshot>::new()));
        let handler_observed = Rc::clone(&observed);
        let handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
            if let Some(snapshot) = message.downcast_ref::<RuntimeSelectionSnapshot>() {
                handler_observed.borrow_mut().push(snapshot.clone());
            }
        });
        let active_market = candidate_market("mkt-preempted", "ACTIVE.POLYMARKET", 2_000.0, 120);
        let uploader = MockUploader::default();
        let services = stub_runtime_services_with_loader(
            Arc::new(CountingLoader {
                calls: Arc::clone(&calls),
                market: active_market.clone(),
            }),
            Arc::new(RecordingAuditTaskFactory::new(uploader.clone())),
            Arc::clone(&builds),
        );

        let mut node = build_lifecycle_node();
        seed_open_position_for_strategy(&node, StrategyId::from("STUB-RUNTIME-001"));
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();
        subscribe_any(topic.clone().into(), handler.clone(), None);

        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "active", 1, poll_budget).await;
            wait_for_condition_or_stop(
                Duration::from_secs(1),
                &stop_handle,
                "runtime selection snapshot while positions remain open",
                || !observed.borrow().is_empty(),
            )
            .await?;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        let shutdown_result = guards.shutdown().await;
        unsubscribe_any(topic.into(), &handler);

        run_result.unwrap();
        control_result.unwrap();
        shutdown_result.unwrap();

        let records = uploaded_records(&uploader);
        assert!(records.iter().any(|record| {
            record["kind"] == "selector_decision"
                && record["state"] == "active"
                && record["market_id"] == "mkt-preempted"
                && record["instrument_id"] == "ACTIVE.POLYMARKET"
                && record["reason"].is_null()
        }));
        assert!(
            calls.load(Ordering::SeqCst) > 0,
            "selector should keep calling the loader while positions remain open"
        );
        assert!(
            observed.borrow().iter().any(|snapshot| {
                snapshot
                    == &RuntimeSelectionSnapshot {
                        ruleset_id: "PRIMARY".to_string(),
                        decision: bolt_v2::platform::ruleset::SelectionDecision {
                            ruleset_id: "PRIMARY".to_string(),
                            state: SelectionState::Active {
                                market: active_market.clone(),
                            },
                        },
                        eligible_candidates: vec![active_market.clone()],
                        published_at_ms: 1_000,
                    }
            }),
            "runtime selection snapshot should publish the active decision while positions remain open"
        );
        assert_eq!(builds.lock().unwrap().as_slice(), ["STUB-RUNTIME-001"]);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn selector_waits_for_running_before_polling_candidates() {
    struct CountingLoader {
        calls: Arc<AtomicUsize>,
    }

    impl CandidateMarketLoader for CountingLoader {
        fn load(&self, _ruleset: RulesetConfig) -> CandidateMarketLoadFuture {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    let dir = tempdir().unwrap();
    let mut cfg = test_config(dir.path());
    cfg.rulesets[0].selector_poll_interval_ms = 10;
    let release = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let services = services_with_loader(
        Arc::new(CountingLoader {
            calls: Arc::clone(&calls),
        }),
        Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
    );

    let mut node = build_delayed_start_node(Arc::clone(&release));
    let handle = node.handle();
    let stop_handle = handle.clone();
    let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

    let control = async {
        wait_for_condition_or_stop(
            Duration::from_secs(1),
            &stop_handle,
            "node entering Starting",
            || matches!(handle.state(), NodeState::Starting),
        )
        .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "selector must not poll candidates before node is Running"
        );
        release.notify_one();
        wait_for_running(&handle).await;
        wait_for_condition_or_stop(
            Duration::from_secs(1),
            &stop_handle,
            "selector polling after node reaches Running",
            || calls.load(Ordering::SeqCst) > 0,
        )
        .await?;
        stop_handle.stop();
        Ok::<(), anyhow::Error>(())
    };

    let (run_result, control_result) = tokio::join!(node.run(), control);
    run_result.unwrap();
    control_result.unwrap();
    guards.shutdown().await.unwrap();
}

#[test]
fn runtime_selection_snapshot_publishes_on_selection_changes() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 25;
        let topic = runtime_selection_topic(&StrategyId::from("STUB-RUNTIME-001"));
        let observed = Rc::new(RefCell::new(Vec::<RuntimeSelectionSnapshot>::new()));
        let handler_observed = Rc::clone(&observed);
        let handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
            if let Some(snapshot) = message.downcast_ref::<RuntimeSelectionSnapshot>() {
                handler_observed.borrow_mut().push(snapshot.clone());
            }
        });
        let builds = Arc::new(Mutex::new(Vec::<String>::new()));

        let first_market =
            candidate_market("mkt-active-runtime-a", "ACTIVE-A.POLYMARKET", 2_000.0, 120);
        let second_market =
            candidate_market("mkt-active-runtime-b", "ACTIVE-B.POLYMARKET", 2_500.0, 120);
        let services = stub_runtime_services_with_loader(
            Arc::new(SequencedLoader::new(vec![
                vec![first_market.clone()],
                vec![second_market.clone()],
            ])),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
            Arc::clone(&builds),
        );

        let mut node = build_lifecycle_node();
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let control = async {
            wait_for_running(&handle).await;
            subscribe_any(topic.clone().into(), handler.clone(), None);
            wait_for_condition_or_stop(
                Duration::from_secs(1),
                &stop_handle,
                "runtime selection snapshot publication",
                || !observed.borrow().is_empty(),
            )
            .await?;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        let shutdown_result = guards.shutdown().await;
        unsubscribe_any(topic.into(), &handler);

        run_result.unwrap();
        control_result.unwrap();
        shutdown_result.unwrap();

        let snapshots = observed.borrow();
        assert_eq!(builds.lock().unwrap().as_slice(), ["STUB-RUNTIME-001"]);
        assert_eq!(snapshots.len(), 2);
        assert_eq!(
            snapshots[0],
            RuntimeSelectionSnapshot {
                ruleset_id: "PRIMARY".to_string(),
                decision: bolt_v2::platform::ruleset::SelectionDecision {
                    ruleset_id: "PRIMARY".to_string(),
                    state: SelectionState::Active {
                        market: first_market.clone(),
                    },
                },
                eligible_candidates: vec![first_market],
                published_at_ms: 1_000,
            }
        );
        assert_eq!(
            snapshots[1],
            RuntimeSelectionSnapshot {
                ruleset_id: "PRIMARY".to_string(),
                decision: bolt_v2::platform::ruleset::SelectionDecision {
                    ruleset_id: "PRIMARY".to_string(),
                    state: SelectionState::Active {
                        market: second_market.clone(),
                    },
                },
                eligible_candidates: vec![second_market],
                published_at_ms: 1_000,
            }
        );
    });
}

#[test]
fn runtime_strategy_persists_component_id_across_selection_changes() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let spool_dir = dir.path().to_path_buf();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 10;
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let builds = Arc::new(Mutex::new(Vec::<String>::new()));
        let services = stub_runtime_services_with_loader(
            Arc::new(SequencedLoader::new(vec![
                vec![candidate_market(
                    "mkt-active-switch-a",
                    "ACTIVE-A.POLYMARKET",
                    2_000.0,
                    120,
                )],
                vec![candidate_market(
                    "mkt-active-switch-b",
                    "ACTIVE-B.POLYMARKET",
                    2_000.0,
                    120,
                )],
                Vec::new(),
                vec![candidate_market(
                    "mkt-freeze-after-idle",
                    "FREEZE-AFTER-IDLE.POLYMARKET",
                    2_000.0,
                    60,
                )],
            ])),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
            Arc::clone(&builds),
        );

        let mut node = build_lifecycle_node();
        let trader = std::rc::Rc::clone(node.kernel().trader());
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "active", 2, poll_budget).await;
            wait_for_selector_state(&spool_dir, "idle", 1, poll_budget).await;
            wait_for_selector_state(&spool_dir, "freeze", 1, poll_budget).await;
            wait_for_condition_or_stop(
                Duration::from_secs(1),
                &stop_handle,
                "runtime strategy persistence across active, switch, idle, and freeze transitions",
                || trader.borrow().strategy_ids() == vec![StrategyId::from("STUB-RUNTIME-001")],
            )
            .await?;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        run_result.unwrap();
        control_result.unwrap();

        assert_eq!(builds.lock().unwrap().as_slice(), ["STUB-RUNTIME-001"]);
        assert_eq!(
            trader.borrow().strategy_ids(),
            vec![StrategyId::from("STUB-RUNTIME-001")]
        );

        guards.shutdown().await.unwrap();
    });
}

#[test]
fn runtime_strategy_removes_only_at_shutdown() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let spool_dir = dir.path().to_path_buf();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 10;
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let builds = Arc::new(Mutex::new(Vec::<String>::new()));
        let services = stub_runtime_services_with_loader(
            Arc::new(SequencedLoader::new(vec![
                vec![candidate_market(
                    "mkt-active-runtime",
                    "ACTIVE-RT.POLYMARKET",
                    2_000.0,
                    120,
                )],
                Vec::new(),
                vec![candidate_market(
                    "mkt-freeze-runtime",
                    "FREEZE-RT.POLYMARKET",
                    2_000.0,
                    60,
                )],
            ])),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
            Arc::clone(&builds),
        );

        let mut node = build_lifecycle_node();
        let trader = std::rc::Rc::clone(node.kernel().trader());
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "active", 1, poll_budget).await;
            wait_for_selector_state(&spool_dir, "idle", 1, poll_budget).await;
            wait_for_selector_state(&spool_dir, "freeze", 1, poll_budget).await;
            wait_for_condition_or_stop(
                Duration::from_secs(1),
                &stop_handle,
                "runtime strategy to remain registered until shutdown begins",
                || trader.borrow().strategy_ids() == vec![StrategyId::from("STUB-RUNTIME-001")],
            )
            .await?;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        run_result.unwrap();
        control_result.unwrap();

        assert_eq!(
            trader.borrow().strategy_ids(),
            vec![StrategyId::from("STUB-RUNTIME-001")],
            "runtime strategy should remain registered until shutdown"
        );
        guards.shutdown().await.unwrap();
        assert_eq!(
            trader.borrow().strategy_ids(),
            Vec::<StrategyId>::new(),
            "runtime shutdown should remove the runtime-managed strategy"
        );
        assert_eq!(builds.lock().unwrap().as_slice(), ["STUB-RUNTIME-001"]);
    });
}

#[test]
fn runtime_does_not_remove_pre_registered_template_strategy_on_shutdown() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 10;
        let poll_budget = selector_poll_budget(cfg.rulesets[0].selector_poll_interval_ms);
        let builds = Arc::new(Mutex::new(Vec::<String>::new()));
        let services = stub_runtime_services_with_loader(
            Arc::new(SequencedLoader::new(vec![
                vec![candidate_market(
                    "mkt-active-pre-registered",
                    "ACTIVE-PRE.POLYMARKET",
                    2_000.0,
                    120,
                )],
                Vec::new(),
            ])),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
            Arc::clone(&builds),
        );

        let mut node = build_lifecycle_node();
        let trader = std::rc::Rc::clone(node.kernel().trader());
        let pre_registered = StubRuntimeStrategy::new("STUB-RUNTIME-001");
        node.add_strategy(pre_registered).unwrap();
        let handle = node.handle();
        let stop_handle = handle.clone();
        let guards = wire_platform_runtime_with_services(&mut node, &cfg, services).unwrap();

        let spool_dir = dir.path().to_path_buf();
        let control = async {
            wait_for_running(&handle).await;
            wait_for_selector_state(&spool_dir, "active", 1, poll_budget).await;
            stop_handle.stop();
            Ok::<(), anyhow::Error>(())
        };

        let (run_result, control_result) = tokio::join!(node.run(), control);
        run_result.unwrap();
        control_result.unwrap();

        assert_eq!(
            trader.borrow().strategy_ids(),
            vec![StrategyId::from("STUB-RUNTIME-001")]
        );
        guards.shutdown().await.unwrap();
        assert_eq!(
            trader.borrow().strategy_ids(),
            vec![StrategyId::from("STUB-RUNTIME-001")],
            "runtime shutdown should not remove an adopted pre-registered strategy"
        );
        assert!(
            builds.lock().unwrap().is_empty(),
            "runtime should not rebuild a pre-registered matching strategy"
        );
    });
}

#[tokio::test(flavor = "current_thread")]
async fn freeze_window_market_emits_freeze_decision() {
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
}

#[test]
fn ruleset_mode_rejects_duplicate_runtime_strategy_templates() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.strategies.push(StrategyEntry {
            kind: StubRuntimeStrategyBuilder::kind().to_string(),
            config: toml::toml! {
                strategy_id = "STUB-RUNTIME-002"
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
                .contains("at most one runtime strategy template"),
            "{error}"
        );
    });
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_token_stops_background_tasks_cleanly() {
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
}

#[test]
fn runtime_strategy_build_failure_surfaces_during_wiring() {
    run_multithread_localset_test(async {
        let dir = tempdir().unwrap();
        let mut cfg = stub_runtime_lifecycle_config(dir.path());
        cfg.rulesets[0].selector_poll_interval_ms = 10;
        cfg.strategies[0]
            .config
            .as_table_mut()
            .expect("stub runtime template config should be a table")
            .remove("strategy_id");
        let services = stub_runtime_services_with_loader(
            Arc::new(SequencedLoader::new(vec![vec![candidate_market(
                "mkt-active-runtime-failure",
                "ACTIVE-FAIL.POLYMARKET",
                2_000.0,
                120,
            )]])),
            Arc::new(RecordingAuditTaskFactory::new(MockUploader::default())),
            Arc::new(Mutex::new(Vec::<String>::new())),
        );

        let mut node = build_lifecycle_node();
        let error = match wire_platform_runtime_with_services(&mut node, &cfg, services) {
            Ok(_) => panic!("runtime strategy build failure should surface during wiring"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("failed building runtime-managed strategy from template")
                || error
                    .to_string()
                    .contains("runtime strategy template must include strategy_id")
                || error
                    .to_string()
                    .contains("stub runtime strategy requires strategy_id"),
            "{error:#}"
        );
    });
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_passes_ruleset_loader_timeout_through_config() {
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
}

#[tokio::test(flavor = "current_thread")]
async fn reference_snapshot_is_forwarded_into_audit_spool() {
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
}

#[tokio::test(flavor = "current_thread")]
async fn background_producers_stop_emitting_before_runtime_shutdown() {
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
