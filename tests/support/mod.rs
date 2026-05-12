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
use bolt_v2::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_decision_events::BoltV3OrderSubmissionFacts,
    bolt_v3_release_identity::{bolt_v3_compiled_nautilus_trader_revision, bolt_v3_config_hash},
};
use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_common::{
    cache::Cache,
    clients::{DataClient, ExecutionClient},
    clock::Clock,
    live::runner::try_get_data_event_sender,
    messages::DataEvent,
    messages::data::{SubscribeInstrument, SubscribeQuotes, SubscribeTrades},
    messages::execution::SubmitOrder,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    accounts::AccountAny,
    enums::{OmsType, OrderSide, OrderType},
    identifiers::{
        AccountId, ClientId, ClientOrderId, InstrumentId, StrategyId, TraderId, Venue, VenueOrderId,
    },
    instruments::InstrumentAny,
    reports::ExecutionMassStatus,
    types::{AccountBalance, MarginBalance, Price, Quantity},
};
use serde::Deserialize;

static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);
static MOCK_DATA_SUBSCRIPTIONS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static MOCK_EXEC_SUBMISSIONS: OnceLock<Mutex<Vec<RecordedSubmitOrder>>> = OnceLock::new();
static MOCK_EXTERNAL_ORDER_REGISTRATIONS: OnceLock<Mutex<Vec<RecordedExternalOrderRegistration>>> =
    OnceLock::new();

fn mock_data_subscriptions() -> &'static Mutex<Vec<String>> {
    MOCK_DATA_SUBSCRIPTIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn mock_exec_submissions() -> &'static Mutex<Vec<RecordedSubmitOrder>> {
    MOCK_EXEC_SUBMISSIONS.get_or_init(|| Mutex::new(Vec::new()))
}

fn mock_external_order_registrations() -> &'static Mutex<Vec<RecordedExternalOrderRegistration>> {
    MOCK_EXTERNAL_ORDER_REGISTRATIONS.get_or_init(|| Mutex::new(Vec::new()))
}

pub fn clear_mock_data_subscriptions() {
    mock_data_subscriptions().lock().unwrap().clear();
}

pub fn recorded_mock_data_subscriptions() -> Vec<String> {
    mock_data_subscriptions().lock().unwrap().clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedSubmitOrder {
    pub trader_id: TraderId,
    pub client_id: Option<ClientId>,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub client_order_id: ClientOrderId,
    pub order_side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub price: Option<Price>,
}

pub fn clear_mock_exec_submissions() {
    mock_exec_submissions().lock().unwrap().clear();
}

pub fn recorded_mock_exec_submissions() -> Vec<RecordedSubmitOrder> {
    mock_exec_submissions().lock().unwrap().clone()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedExternalOrderRegistration {
    pub client_order_id: ClientOrderId,
    pub venue_order_id: VenueOrderId,
    pub instrument_id: InstrumentId,
    pub strategy_id: StrategyId,
    pub ts_init: UnixNanos,
}

pub fn clear_mock_external_order_registrations() {
    mock_external_order_registrations().lock().unwrap().clear();
}

pub fn recorded_mock_external_order_registrations() -> Vec<RecordedExternalOrderRegistration> {
    mock_external_order_registrations().lock().unwrap().clone()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalHttpStatus {
    Ok,
    NotFound,
}

impl LocalHttpStatus {
    fn line(self) -> &'static str {
        match self {
            Self::Ok => "200 OK",
            Self::NotFound => "404 Not Found",
        }
    }
}

pub fn local_http_json_response(status: LocalHttpStatus, body: &str) -> String {
    format!(
        "HTTP/1.1 {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        status.line(),
        body.len(),
        body
    )
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdownSelectedMarketLegFixture {
    pub outcome: String,
    pub token_id: String,
    pub instrument_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdownSelectedMarketFixture {
    pub name: String,
    pub readiness_role: Option<UpdownSelectedMarketReadinessRole>,
    pub runtime_role: Option<UpdownSelectedMarketRuntimeRole>,
    pub condition_id: String,
    pub question_id: String,
    pub market_slug: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub legs: Vec<UpdownSelectedMarketLegFixture>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdownSelectedMarketReadinessRole {
    Current,
    Stale,
    AmbiguousA,
    AmbiguousB,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdownSelectedMarketRuntimeRole {
    Default,
    RotationB,
    RecoveryA,
    RecoveryB,
}

impl UpdownSelectedMarketFixture {
    pub fn leg(&self, outcome: &str) -> &UpdownSelectedMarketLegFixture {
        self.legs
            .iter()
            .find(|leg| leg.outcome == outcome)
            .unwrap_or_else(|| panic!("fixture market {} should include {outcome} leg", self.name))
    }
}

#[derive(Debug, Deserialize)]
struct UpdownSelectedMarketsFixtureFile {
    markets: Vec<UpdownSelectedMarketFixture>,
}

fn load_updown_selected_markets_fixture_file() -> (PathBuf, UpdownSelectedMarketsFixtureFile) {
    let path = repo_path("tests/fixtures/bolt_v3_existing_strategy/updown_selected_markets.toml");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
    let fixture: UpdownSelectedMarketsFixtureFile = toml::from_str(&text)
        .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()));
    (path, fixture)
}

fn selected_market_fixture_by_name(
    path: &Path,
    fixture: UpdownSelectedMarketsFixtureFile,
    name: &str,
) -> UpdownSelectedMarketFixture {
    fixture
        .markets
        .into_iter()
        .find(|market| market.name == name)
        .unwrap_or_else(|| panic!("{} should define market fixture {name}", path.display()))
}

pub fn bolt_v3_updown_selected_market_fixture(name: &str) -> UpdownSelectedMarketFixture {
    let (path, fixture) = load_updown_selected_markets_fixture_file();
    selected_market_fixture_by_name(&path, fixture, name)
}

pub fn bolt_v3_updown_readiness_selected_market_fixture(
    role: UpdownSelectedMarketReadinessRole,
) -> UpdownSelectedMarketFixture {
    let (path, fixture) = load_updown_selected_markets_fixture_file();
    fixture
        .markets
        .into_iter()
        .find(|market| market.readiness_role == Some(role))
        .unwrap_or_else(|| {
            panic!(
                "{} should define readiness selected-market fixture role {role:?}",
                path.display()
            )
        })
}

pub fn bolt_v3_updown_runtime_selected_market_fixture(
    role: UpdownSelectedMarketRuntimeRole,
) -> UpdownSelectedMarketFixture {
    let (path, fixture) = load_updown_selected_markets_fixture_file();
    fixture
        .markets
        .into_iter()
        .find(|market| market.runtime_role == Some(role))
        .unwrap_or_else(|| {
            panic!(
                "{} should define runtime selected-market fixture role {role:?}",
                path.display()
            )
        })
}

pub fn attach_test_release_identity_manifest(loaded: &mut LoadedBoltV3Config, temp_dir: &Path) {
    let config_hash = bolt_v3_config_hash(loaded).expect("fixture config hash should compute");
    let nt_revision = bolt_v3_compiled_nautilus_trader_revision()
        .expect("fixture NT revision should resolve from Cargo.toml");
    let manifest_path = temp_dir.join("release-identity.toml");
    fs::write(
        &manifest_path,
        format!(
            r#"
release_id = "test-release"
git_commit_sha = "test-git-sha"
nautilus_trader_revision = "{nt_revision}"
binary_sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
cargo_lock_sha256 = "2222222222222222222222222222222222222222222222222222222222222222"
config_hash = "{config_hash}"
build_profile = "test"

[artifact_sha256]
bolt_v2 = "3333333333333333333333333333333333333333333333333333333333333333"
"#,
        ),
    )
    .expect("release identity manifest should write");
    loaded.root.release.identity_manifest_path = manifest_path.to_string_lossy().into_owned();
    let catalog_dir = temp_dir.join("catalog");
    fs::create_dir_all(&catalog_dir).expect("catalog dir should create");
    loaded.root.persistence.catalog_directory = catalog_dir.to_string_lossy().into_owned();
}

#[derive(Debug, Deserialize)]
struct BoltV3OrderSubmissionFactsFixture {
    order_type: String,
    time_in_force: String,
    instrument_id: String,
    side: String,
    price: f64,
    quantity: f64,
    is_quote_quantity: bool,
    is_post_only: bool,
    is_reduce_only: bool,
    client_order_id: Option<String>,
}

impl From<BoltV3OrderSubmissionFactsFixture> for BoltV3OrderSubmissionFacts {
    fn from(fixture: BoltV3OrderSubmissionFactsFixture) -> Self {
        Self {
            order_type: fixture.order_type,
            time_in_force: fixture.time_in_force,
            instrument_id: fixture.instrument_id,
            side: fixture.side,
            price: fixture.price,
            quantity: fixture.quantity,
            is_quote_quantity: fixture.is_quote_quantity,
            is_post_only: fixture.is_post_only,
            is_reduce_only: fixture.is_reduce_only,
            client_order_id: fixture.client_order_id,
        }
    }
}

pub fn bolt_v3_order_submission_facts_fixture(filename: &str) -> BoltV3OrderSubmissionFacts {
    let path = repo_path(&format!(
        "tests/fixtures/bolt_v3_decision_events/{filename}"
    ));
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
    let fixture: BoltV3OrderSubmissionFactsFixture = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()));
    fixture.into()
}

pub fn runtime_toml_with_reference_venue(
    reference_chainlink_block: &str,
    reference_venue_block: &str,
    resolution_basis: &str,
) -> String {
    format!(
        r#"
[node]
name = "bolt-v2"
trader_id = "TRADER-001"
environment = "Live"
load_state = true
save_state = true
timeout_connection_secs = 60
timeout_reconciliation_secs = 30
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 10
delay_shutdown_secs = 5

[logging]
stdout_level = "Info"
file_level = "Off"

[[data_clients]]
name = "POLYMARKET"
type = "polymarket"
[data_clients.config]
subscribe_new_markets = false
update_instruments_interval_mins = 60
ws_max_subscriptions = 200
event_slugs = ["btc-updown-5m"]

[[exec_clients]]
name = "POLYMARKET"
type = "polymarket"
[exec_clients.config]
account_id = "POLYMARKET-001"
signature_type = 2
funder = "0xdeadbeef"
[exec_clients.secrets]
region = "us-east-1"
pk = "/pk"
api_key = "/key"
api_secret = "/secret"
passphrase = "/pass"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

{reference_chainlink_block}

{reference_venue_block}

[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
resolution_basis = "{resolution_basis}"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 250
candidate_load_timeout_secs = 12
[rulesets.selector]
tag_slug = "bitcoin"

[audit]
local_dir = "/srv/bolt-v2/var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 45
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#
    )
}

pub fn live_local_chainlink_operator_input() -> String {
    r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[raw_capture]
output_dir = "/srv/bolt-v2/var/raw"

[reference]
publish_topic = "platform.reference.default"
min_publish_interval_ms = 100

[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 5

[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8

[[rulesets]]
id = "PRIMARY"
venue = "polymarket"
resolution_basis = "chainlink_btcusd"
min_time_to_expiry_secs = 60
max_time_to_expiry_secs = 900
min_liquidity_num = 1000
require_accepting_orders = true
freeze_before_end_secs = 90
selector_poll_interval_ms = 1000
candidate_load_timeout_secs = 30
[rulesets.selector]
tag_slug = "bitcoin"

[audit]
local_dir = "/srv/bolt-v2/var/audit"
s3_uri = "s3://bolt-runtime-history/phase1"
ship_interval_secs = 30
upload_attempt_timeout_secs = 30
roll_max_bytes = 1048576
roll_max_secs = 300
max_local_backlog_bytes = 10485760
"#
    .to_string()
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
    startup_instruments: Vec<InstrumentAny>,
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
            startup_instruments: Vec::new(),
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

    pub fn with_startup_instruments(mut self, instruments: Vec<InstrumentAny>) -> Self {
        self.startup_instruments = instruments;
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
    mass_status: Option<ExecutionMassStatus>,
}

impl MockExecClientConfig {
    pub fn new(client_id: &str, account_id: &str, venue: &str) -> Self {
        Self {
            client_id: client_id.to_string(),
            account_id: account_id.to_string(),
            venue: venue.to_string(),
            mass_status: None,
        }
    }

    pub fn with_mass_status(mut self, mass_status: ExecutionMassStatus) -> Self {
        self.mass_status = Some(mass_status);
        self
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
            cfg.startup_instruments.clone(),
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
            cfg.mass_status.clone(),
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
    startup_instruments: Vec<InstrumentAny>,
}

impl MockDataClient {
    fn new(
        client_id: ClientId,
        venue: Venue,
        connect_delay: Duration,
        connect_failure: Option<String>,
        disconnect_delay: Duration,
        disconnect_failure: Option<String>,
        startup_instruments: Vec<InstrumentAny>,
    ) -> Self {
        Self {
            client_id,
            venue,
            connected: false,
            connect_delay,
            connect_failure,
            disconnect_delay,
            disconnect_failure,
            startup_instruments,
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
    mass_status: Option<ExecutionMassStatus>,
}

impl MockExecutionClient {
    fn new(
        client_id: ClientId,
        account_id: AccountId,
        venue: Venue,
        oms_type: OmsType,
        mass_status: Option<ExecutionMassStatus>,
    ) -> Self {
        Self {
            client_id,
            account_id,
            venue,
            oms_type,
            connected: false,
            mass_status,
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
        if !self.startup_instruments.is_empty() {
            let sender = try_get_data_event_sender().ok_or_else(|| {
                anyhow::anyhow!("mock data client startup instruments require NT data event sender")
            })?;
            for instrument in self.startup_instruments.iter().cloned() {
                sender
                    .send(DataEvent::Instrument(instrument))
                    .map_err(|error| {
                        anyhow::anyhow!("failed to send startup instrument: {error}")
                    })?;
            }
        }
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

    async fn generate_mass_status(
        &self,
        _lookback_mins: Option<u64>,
    ) -> anyhow::Result<Option<ExecutionMassStatus>> {
        Ok(self.mass_status.clone())
    }

    fn register_external_order(
        &self,
        client_order_id: ClientOrderId,
        venue_order_id: VenueOrderId,
        instrument_id: InstrumentId,
        strategy_id: StrategyId,
        ts_init: UnixNanos,
    ) {
        mock_external_order_registrations().lock().unwrap().push(
            RecordedExternalOrderRegistration {
                client_order_id,
                venue_order_id,
                instrument_id,
                strategy_id,
                ts_init,
            },
        );
    }

    fn submit_order(&self, cmd: SubmitOrder) -> anyhow::Result<()> {
        mock_exec_submissions()
            .lock()
            .unwrap()
            .push(RecordedSubmitOrder {
                trader_id: cmd.trader_id,
                client_id: cmd.client_id,
                strategy_id: cmd.strategy_id,
                instrument_id: cmd.instrument_id,
                client_order_id: cmd.client_order_id,
                order_side: cmd.order_init.order_side,
                order_type: cmd.order_init.order_type,
                quantity: cmd.order_init.quantity,
                price: cmd.order_init.price,
            });
        Ok(())
    }
}

/// PKCS8-wrapped Ed25519 private key, base64-encoded. The bolt-v3 binance
/// shape validator (`crate::secrets::validate_binance_api_secret_shape`)
/// requires that the resolved api_secret decode as a valid PKCS8 Ed25519
/// key, so the fake resolver must hand back a value that satisfies it.
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
/// per-path synthetic values that satisfy the polymarket and binance
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
        "/bolt/chainlink_reference/api_key" => Ok("chainlink-api-key".to_string()),
        "/bolt/chainlink_reference/api_secret" => Ok("chainlink-api-secret".to_string()),
        _ => Err("unexpected SSM path requested by bolt-v3 fake resolver"),
    }
}
