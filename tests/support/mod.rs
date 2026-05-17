#![allow(dead_code)]

pub(crate) mod stub_runtime_strategy;

use std::{
    any::Any,
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_common::{
    cache::Cache,
    clients::{DataClient, ExecutionClient},
    clock::Clock,
    messages::data::{SubscribeInstrument, SubscribeQuotes, SubscribeTrades},
    messages::execution::SubmitOrder,
};
use nautilus_model::{
    accounts::AccountAny,
    enums::OmsType,
    identifiers::{AccountId, ClientId, ClientOrderId, InstrumentId, StrategyId, Venue},
    types::{AccountBalance, MarginBalance},
};

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);
static MOCK_DATA_SUBSCRIPTIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MOCK_EXEC_SUBMISSIONS: OnceLock<Mutex<Vec<RecordedSubmitOrder>>> = OnceLock::new();

#[derive(Debug, Default)]
pub struct RecordingDecisionEvidenceWriter {
    records: Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence>>,
    admission_decisions:
        Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence>>,
}

impl RecordingDecisionEvidenceWriter {
    pub fn records(&self) -> Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn admission_decisions(
        &self,
    ) -> Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence> {
        self.admission_decisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
    for RecordingDecisionEvidenceWriter
{
    fn record_order_intent(
        &self,
        intent: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
    ) -> anyhow::Result<()> {
        self.records
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(intent.clone());
        Ok(())
    }

    fn record_admission_decision(
        &self,
        decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        self.admission_decisions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(decision.clone());
        Ok(())
    }
}

fn mock_data_subscriptions() -> &'static Mutex<Vec<String>> {
    MOCK_DATA_SUBSCRIPTIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn mock_exec_submissions() -> &'static Mutex<Vec<RecordedSubmitOrder>> {
    MOCK_EXEC_SUBMISSIONS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn clear_mock_data_subscriptions() {
    mock_data_subscriptions().lock().unwrap().clear();
}

pub fn recorded_mock_data_subscriptions() -> Vec<String> {
    mock_data_subscriptions().lock().unwrap().clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedSubmitOrder {
    pub client_id: Option<ClientId>,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub client_order_id: ClientOrderId,
}

pub fn clear_mock_exec_submissions() {
    mock_exec_submissions().lock().unwrap().clear();
}

pub fn recorded_mock_exec_submissions() -> Vec<RecordedSubmitOrder> {
    mock_exec_submissions().lock().unwrap().clone()
}

pub struct TempCaseDir {
    path: PathBuf,
}

impl TempCaseDir {
    pub fn new(label: &str) -> Self {
        let timestamp_nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        let counter = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dirname = format!("bolt-v2-{label}-{timestamp_nanos}-{counter}");
        let path = std::env::temp_dir().join(dirname);
        fs::create_dir_all(&path).expect("temp case dir should be created");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

pub fn validated_bolt_v3_live_canary_gate_report(
    max_live_order_count: u32,
    max_notional_per_order: rust_decimal::Decimal,
) -> bolt_v2::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport {
    let root_path = repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded =
        bolt_v2::bolt_v3_config::load_bolt_v3_config(&root_path).expect("fixture should load");
    let temp = TempCaseDir::new("bolt-v3-validated-gate-report");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    loaded.root.risk.default_max_notional_per_order = max_notional_per_order.to_string();
    let report_path = temp.path().join("no-submit-readiness.json");
    write_satisfied_no_submit_readiness_report(&report_path);
    loaded.root.live_canary = Some(bolt_v2::bolt_v3_config::LiveCanaryBlock {
        approval_id: "operator-approved-canary-001".to_string(),
        no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
        max_live_order_count,
        max_notional_per_order: max_notional_per_order.to_string(),
        max_no_submit_readiness_report_bytes: 4096,
        operator_evidence: None,
    });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build");
    runtime
        .block_on(bolt_v2::bolt_v3_live_canary_gate::check_bolt_v3_live_canary_gate(&loaded))
        .expect("valid live canary fixture should pass gate")
}

fn write_satisfied_no_submit_readiness_report(path: &Path) {
    use bolt_v2::bolt_v3_no_submit_readiness_schema::{
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, LIVE_NODE_BUILD_STAGE,
        OPERATOR_APPROVAL_STAGE, REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE,
        SECRET_RESOLUTION_STAGE,
    };

    let report = serde_json::json!({
        "stages": [
            { "stage": OPERATOR_APPROVAL_STAGE, "status": "satisfied" },
            { "stage": SECRET_RESOLUTION_STAGE, "status": "satisfied" },
            { "stage": LIVE_NODE_BUILD_STAGE, "status": "satisfied" },
            { "stage": CONTROLLED_CONNECT_STAGE, "status": "satisfied" },
            { "stage": REFERENCE_READINESS_STAGE, "status": "satisfied" },
            { "stage": CONTROLLED_DISCONNECT_STAGE, "status": "satisfied" },
            { "stage": REPORT_WRITE_STAGE, "status": "satisfied" }
        ]
    });
    fs::write(
        path,
        serde_json::to_vec(&report).expect("report JSON should encode"),
    )
    .expect("readiness report should be written");
}

impl Drop for TempCaseDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
pub struct MockDataClientConfig {
    client_id: String,
    venue: String,
    connect_delay: Duration,
    connect_failure: Option<String>,
    disconnect_delay: Duration,
    disconnect_failure: Option<String>,
}

impl MockDataClientConfig {
    pub fn new(client_id: &str, venue: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            venue: venue.to_string(),
            connect_delay: Duration::ZERO,
            connect_failure: None,
            disconnect_delay: Duration::ZERO,
            disconnect_failure: None,
        }
    }

    pub fn with_connect_delay_milliseconds(mut self, milliseconds: u64) -> Self {
        self.connect_delay = Duration::from_millis(milliseconds);
        self
    }

    /// Configures the mock to surface an `Err(...)` from its
    /// `DataClient::connect` implementation. The pinned NT
    /// `DataEngine::connect` swallows the error and logs it, so the
    /// client's `is_connected()` flag stays false; controlled-connect
    /// callers see this through `kernel.check_engines_connected()`
    /// returning false after dispatch returns.
    pub fn with_connect_failure(mut self, message: &str) -> Self {
        self.connect_failure = Some(message.to_string());
        self
    }

    /// Configures the mock to sleep for the given number of
    /// milliseconds inside `DataClient::disconnect` before flipping
    /// its `connected` flag. Used to drive the bolt-v3
    /// controlled-disconnect timeout path without touching real I/O.
    pub fn with_disconnect_delay_milliseconds(mut self, milliseconds: u64) -> Self {
        self.disconnect_delay = Duration::from_millis(milliseconds);
        self
    }

    /// Configures the mock to surface an `Err(...)` from its
    /// `DataClient::disconnect` implementation. The bolt-v3
    /// controlled-disconnect boundary must propagate this as
    /// `DisconnectFailed` rather than silently swallowing it.
    pub fn with_disconnect_failure(mut self, message: &str) -> Self {
        self.disconnect_failure = Some(message.to_string());
        self
    }
}

impl ClientConfig for MockDataClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
pub struct MockExecClientConfig {
    client_id: String,
    account_id: String,
    venue: String,
}

impl MockExecClientConfig {
    pub fn new(client_id: &str, account_id: &str, venue: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            account_id: account_id.to_string(),
            venue: venue.to_string(),
        }
    }
}

impl ClientConfig for MockExecClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
pub struct MockDataClientFactory;

impl DataClientFactory for MockDataClientFactory {
    fn create(
        &self,
        _name: &str,
        config: &dyn ClientConfig,
        _cache: Rc<RefCell<Cache>>,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let cfg = config
            .as_any()
            .downcast_ref::<MockDataClientConfig>()
            .ok_or_else(|| anyhow::anyhow!("MockDataClientFactory received wrong config type"))?;

        Ok(Box::new(MockDataClient::new(
            ClientId::from(cfg.client_id.as_str()),
            Venue::from(cfg.venue.as_str()),
            cfg.connect_delay,
            cfg.connect_failure.clone(),
            cfg.disconnect_delay,
            cfg.disconnect_failure.clone(),
        )))
    }

    fn name(&self) -> &str {
        "mock-data"
    }

    fn config_type(&self) -> &str {
        "MockDataClientConfig"
    }
}

#[derive(Debug)]
pub struct MockExecutionClientFactory;

impl ExecutionClientFactory for MockExecutionClientFactory {
    fn create(
        &self,
        _name: &str,
        config: &dyn ClientConfig,
        _cache: Rc<RefCell<Cache>>,
    ) -> anyhow::Result<Box<dyn ExecutionClient>> {
        let cfg = config
            .as_any()
            .downcast_ref::<MockExecClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!("MockExecutionClientFactory received wrong config type")
            })?;

        Ok(Box::new(MockExecutionClient::new(
            ClientId::from(cfg.client_id.as_str()),
            AccountId::from(cfg.account_id.as_str()),
            Venue::from(cfg.venue.as_str()),
            OmsType::Netting,
        )))
    }

    fn name(&self) -> &str {
        "mock-exec"
    }

    fn config_type(&self) -> &str {
        "MockExecClientConfig"
    }
}

#[derive(Debug)]
struct MockDataClient {
    client_id: ClientId,
    venue: Venue,
    connected: bool,
    connect_delay: Duration,
    connect_failure: Option<String>,
    disconnect_delay: Duration,
    disconnect_failure: Option<String>,
}

impl MockDataClient {
    fn new(
        client_id: ClientId,
        venue: Venue,
        connect_delay: Duration,
        connect_failure: Option<String>,
        disconnect_delay: Duration,
        disconnect_failure: Option<String>,
    ) -> Self {
        Self {
            client_id,
            venue,
            connected: false,
            connect_delay,
            connect_failure,
            disconnect_delay,
            disconnect_failure,
        }
    }
}

#[derive(Debug)]
struct MockExecutionClient {
    client_id: ClientId,
    account_id: AccountId,
    venue: Venue,
    oms_type: OmsType,
    connected: bool,
}

impl MockExecutionClient {
    fn new(client_id: ClientId, account_id: AccountId, venue: Venue, oms_type: OmsType) -> Self {
        Self {
            client_id,
            account_id,
            venue,
            oms_type,
            connected: false,
        }
    }
}

#[async_trait(?Send)]
impl DataClient for MockDataClient {
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
        if !self.connect_delay.is_zero() {
            tokio::time::sleep(self.connect_delay).await;
        }
        if let Some(message) = &self.connect_failure {
            return Err(anyhow::anyhow!(message.clone()));
        }
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if !self.disconnect_delay.is_zero() {
            tokio::time::sleep(self.disconnect_delay).await;
        }
        if let Some(message) = &self.disconnect_failure {
            return Err(anyhow::anyhow!(message.clone()));
        }
        self.connected = false;
        Ok(())
    }

    fn subscribe_instrument(&mut self, cmd: SubscribeInstrument) -> anyhow::Result<()> {
        mock_data_subscriptions()
            .lock()
            .unwrap()
            .push(cmd.instrument_id.to_string());
        Ok(())
    }

    fn subscribe_quotes(&mut self, cmd: SubscribeQuotes) -> anyhow::Result<()> {
        mock_data_subscriptions()
            .lock()
            .unwrap()
            .push(cmd.instrument_id.to_string());
        Ok(())
    }

    fn subscribe_trades(&mut self, cmd: SubscribeTrades) -> anyhow::Result<()> {
        mock_data_subscriptions()
            .lock()
            .unwrap()
            .push(cmd.instrument_id.to_string());
        Ok(())
    }
}

#[async_trait(?Send)]
impl ExecutionClient for MockExecutionClient {
    fn is_connected(&self) -> bool {
        self.connected
    }

    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn account_id(&self) -> AccountId {
        self.account_id
    }

    fn venue(&self) -> Venue {
        self.venue
    }

    fn oms_type(&self) -> OmsType {
        self.oms_type
    }

    fn get_account(&self) -> Option<AccountAny> {
        None
    }

    fn generate_account_state(
        &self,
        _balances: Vec<AccountBalance>,
        _margins: Vec<MarginBalance>,
        _reported: bool,
        _ts_event: nautilus_core::UnixNanos,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn submit_order(&self, cmd: SubmitOrder) -> anyhow::Result<()> {
        mock_exec_submissions()
            .lock()
            .unwrap()
            .push(RecordedSubmitOrder {
                client_id: cmd.client_id,
                strategy_id: cmd.strategy_id,
                instrument_id: cmd.instrument_id,
                client_order_id: cmd.client_order_id,
            });
        Ok(())
    }
}

/// PKCS8-wrapped Ed25519 private key, base64-encoded. The bolt-v3 Binance
/// provider validator requires that the resolved api_secret decode as a
/// valid PKCS8 Ed25519 key, so the fake resolver must hand back a value
/// that satisfies it.
const FAKE_BOLT_V3_BINANCE_API_SECRET: &str =
    "MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f";

/// 32-byte secp256k1 private key in hex (with the `0x` prefix the NT
/// Polymarket adapter accepts). The NT `PolymarketExecutionClient::new`
/// constructor parses this into an EVM signer at registration time, so
/// the fake resolver must hand back a value that decodes to a valid
/// secp256k1 scalar; the all-`0x42` byte sequence is well within the
/// curve order and is shared across bolt-v3 build-path tests.
const FAKE_BOLT_V3_POLYMARKET_PRIVATE_KEY: &str =
    "0x4242424242424242424242424242424242424242424242424242424242424242";

/// Synthetic SSM resolver for bolt-v3 LiveNode build tests. Returns
/// per-path placeholder values that satisfy the polymarket and binance
/// secret schemas declared in `tests/fixtures/bolt_v3/root.toml` so the
/// build path can run all the way through `LiveNodeBuilder::build`
/// (which invokes the real NT `factory.create` for every registered
/// client) without reaching the network. The polymarket private key
/// must be a valid 32-byte secp256k1 hex value because NT's
/// `PolymarketExecutionClient::new` parses it into a signer; the
/// polymarket api_secret must be valid base64 because NT's
/// `Credential::new` decodes it into HMAC key material.
pub fn fake_bolt_v3_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
    match path {
        "/bolt/polymarket_main/private_key" => Ok(FAKE_BOLT_V3_POLYMARKET_PRIVATE_KEY.to_string()),
        "/bolt/polymarket_main/api_key" => Ok("polymarket-api-key".to_string()),
        "/bolt/polymarket_main/api_secret" => Ok("YWJj".to_string()),
        "/bolt/polymarket_main/passphrase" => Ok("polymarket-passphrase".to_string()),
        "/bolt/binance_reference/api_key" => Ok("binance-api-key".to_string()),
        "/bolt/binance_reference/api_secret" => Ok(FAKE_BOLT_V3_BINANCE_API_SECRET.to_string()),
        _ => Err("unexpected SSM path requested by bolt-v3 fake resolver"),
    }
}
