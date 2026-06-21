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
use nautilus_common::enums::Environment;
use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_common::{
    cache::CacheView,
    clients::{DataClient, ExecutionClient},
    clock::Clock,
    messages::data::{SubscribeInstrument, SubscribeQuotes, SubscribeTrades},
    messages::execution::SubmitOrder,
};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    accounts::AccountAny,
    enums::OmsType,
    identifiers::{AccountId, ClientId, ClientOrderId, InstrumentId, StrategyId, TraderId, Venue},
    types::{AccountBalance, MarginBalance},
};

const TEST_DELAY_POST_STOP_SECS: u64 = 0;
const TEST_TRADER_ID: &str = "TESTER-001";

#[track_caller]
pub fn fast_test_live_node() -> LiveNode {
    LiveNode::builder(TraderId::from(TEST_TRADER_ID), Environment::Live)
        .expect("LiveNode builder should initialize with test defaults")
        .with_delay_post_stop_secs(TEST_DELAY_POST_STOP_SECS)
        .build()
        .expect("LiveNode should build with test defaults")
}

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);
static MOCK_DATA_SUBSCRIPTIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MOCK_EXEC_SUBMISSIONS: OnceLock<Mutex<Vec<RecordedSubmitOrder>>> = OnceLock::new();

#[derive(Debug, Default)]
pub struct RecordingDecisionEvidenceWriter {
    records: Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence>>,
    admission_decisions:
        Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence>>,
    order_rejects: Mutex<Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderRejectEvidence>>,
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

    pub fn order_rejects(
        &self,
    ) -> Vec<bolt_v2::bolt_v3_decision_evidence::BoltV3OrderRejectEvidence> {
        self.order_rejects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
    for RecordingDecisionEvidenceWriter
{
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &bolt_v2::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
    ) -> anyhow::Result<()> {
        Ok(())
    }

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

    fn record_basket_admission_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &bolt_v2::bolt_v3_decision_evidence::BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &bolt_v2::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &bolt_v2::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_exit_evaluation(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3ExitEvaluationEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3LossGovernorHaltEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_order_reject(
        &self,
        evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderRejectEvidence,
    ) -> anyhow::Result<()> {
        self.order_rejects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(evidence.clone());
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

    pub fn persist(self) -> PathBuf {
        let path = self.path.clone();
        std::mem::forget(self);
        path
    }
}

pub fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// Execution venue of the binary-option fixtures these integration tests build a
/// `StrategyBuildContext` against (`tests/fixtures/bolt_v3/root.toml` routes `polymarket_main` to
/// POLYMARKET). Production resolves the execution venue from config —
/// `root.clients[execution_client_id].venue` — and is venue-agnostic; these tests do not exercise
/// venue-scoped market selection, so the value is inert here. Centralized so the fixture venue
/// lives in ONE place rather than scattered literals across the integration-test files.
pub fn fixture_execution_venue() -> nautilus_model::identifiers::Venue {
    nautilus_model::identifiers::Venue::from("POLYMARKET")
}

pub fn repo_text(relative: &str) -> String {
    fs::read_to_string(repo_path(relative))
        .unwrap_or_else(|error| panic!("repo text `{relative}` should read: {error}"))
}

/// Repo-relative root path for a gated registry key (file or directory), via the
/// lib integrity owner. Tests feed this straight into the producer write-side so
/// the path can never diverge from the registry across a split.
pub fn registry_relative_root(key: &str) -> &'static str {
    bolt_v2::bolt_v3_source_integrity::registry_relative_root(key)
}

/// Absolute path of a gated registry root, via the lib integrity owner.
pub fn registry_root_path(key: &str) -> PathBuf {
    repo_path(registry_relative_root(key))
}

/// Whole-module source text for a gated registry key, via the lib integrity
/// owner. Replaces the scattered `include_str!` of a monolith root in text-scan
/// tests.
pub fn module_source_text(key: &str) -> String {
    bolt_v2::bolt_v3_source_integrity::module_source_text(key)
}

/// Production-only module source text for a gated registry key (the bottom
/// `#[cfg(test)] mod tests` submodule excluded), via the lib integrity owner.
pub fn production_module_source_text(key: &str) -> String {
    bolt_v2::bolt_v3_source_integrity::production_module_source_text(key)
}

/// Path of the lexically-first venue contract under the repo's `contracts/`
/// directory. Venue-agnostic: no venue name is written here. It is an arbitrary
/// valid envelope for machinery/negative tests; a second contract that sorts
/// earlier would be picked, so tests asserting venue-specific facts load their
/// contract explicitly rather than relying on this selection.
pub fn first_contract_path() -> PathBuf {
    let dir = repo_path("contracts");
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("contracts dir {} must be readable: {error}", dir.display()))
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
        .collect();
    paths.sort();
    paths.into_iter().next().unwrap_or_else(|| {
        panic!(
            "at least one venue contract must ship under {}",
            dir.display()
        )
    })
}

/// Integration-test contract fixture: load the first shipped venue contract via
/// the production loader, then swap in caller-supplied `streams`. No venue name,
/// budget value, settlement kind, or policy is written here — the envelope is
/// sourced entirely from the shipped config (the single source of truth). Mirrors
/// the in-crate `venue_contract::sample_contract_with_streams`.
pub fn sample_contract_with_streams(
    streams: std::collections::BTreeMap<String, bolt_v2::venue_contract::StreamContract>,
) -> bolt_v2::venue_contract::VenueContract {
    let path = first_contract_path();
    let mut contract = bolt_v2::venue_contract::VenueContract::load_and_validate(&path)
        .unwrap_or_else(|error| panic!("shipped contract {} must load: {error}", path.display()));
    contract.streams = streams;
    contract
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

    pub fn with_connect_delay_ms(mut self, milliseconds: u64) -> Self {
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
    pub fn with_disconnect_delay_ms(mut self, milliseconds: u64) -> Self {
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
        _cache: CacheView,
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
        _cache: CacheView,
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
        "/bolt/polymarket/private-key" => Ok(FAKE_BOLT_V3_POLYMARKET_PRIVATE_KEY.to_string()),
        "/bolt/polymarket/api-key" => Ok("polymarket-api-key".to_string()),
        "/bolt/polymarket/api-secret" => Ok("YWJj".to_string()),
        "/bolt/polymarket/api-passphrase" => Ok("polymarket-passphrase".to_string()),
        "/bolt/binance_reference/api_key" => Ok("binance-api-key".to_string()),
        "/bolt/binance_reference/api_secret" => Ok(FAKE_BOLT_V3_BINANCE_API_SECRET.to_string()),
        "/bolt/testnet/chainlink/api-key" => Ok("chainlink-api-key".to_string()),
        "/bolt/testnet/chainlink/api-secret" => Ok("chainlink-api-secret".to_string()),
        "/bolt/polyresearch/api-key" => Ok("polyresearch-api-key".to_string()),
        _ => Err("unexpected SSM path requested by bolt-v3 fake resolver"),
    }
}
