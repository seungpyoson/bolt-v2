mod support;

use std::{
    collections::BTreeMap,
    env,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Mutex, OnceLock},
};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3ClientConfig, BoltV3ClientConfigs, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig,
    },
    bolt_v3_client_registration::register_bolt_v3_clients,
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{make_bolt_v3_live_node_builder, make_live_node_config},
    bolt_v3_reference_actor_registration::register_bolt_v3_reference_actors,
    bolt_v3_release_identity::bolt_v3_compiled_nautilus_trader_revision,
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_strategy_registration::register_bolt_v3_strategies,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::NodeState;
use nautilus_model::{
    accounts::AccountAny,
    enums::{AccountType, AssetClass, OrderSide, OrderStatus, OrderType, TimeInForce},
    events::AccountState,
    identifiers::{AccountId, ClientId, InstrumentId, Symbol, Venue, VenueOrderId},
    instruments::{InstrumentAny, binary_option::BinaryOption},
    reports::{ExecutionMassStatus, OrderStatusReport},
    types::{AccountBalance, Currency, Money, Price, Quantity},
};
use serde::Deserialize;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_external_order_registrations, recorded_mock_external_order_registrations,
};
use tempfile::TempDir;

static RUNTIME_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct OpenOrderFixture {
    load_state: bool,
    save_state: bool,
    delay_post_stop_seconds: u64,
    timeout_disconnection_seconds: u64,
    timeout_reconciliation_seconds: u64,
    reconciliation_startup_delay_seconds: u64,
    account_type: String,
    account_base_currency: String,
    account_total: String,
    account_locked: String,
    account_free: String,
    condition_id: String,
    up_token_id: String,
    down_token_id: String,
    price_increment: String,
    size_increment: String,
    asset_class: String,
    currency: String,
    order_side: String,
    order_type: String,
    time_in_force: String,
    order_status: String,
    order_price: String,
    order_quantity: String,
    filled_quantity: String,
    venue_order_id: String,
    activation_ts_ns: u64,
    expiration_ts_ns: u64,
    report_ts_ns: u64,
}

#[test]
fn bolt_v3_maps_toml_reconciliation_settings_to_nt_live_config() {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    let config = make_live_node_config(&loaded);
    let expected = &loaded.root.nautilus.exec_engine;

    assert_eq!(config.exec_engine.reconciliation, expected.reconciliation);
    assert_eq!(
        config.exec_engine.reconciliation_startup_delay_secs,
        expected.reconciliation_startup_delay_seconds as f64
    );
    assert_eq!(
        config.exec_engine.filter_unclaimed_external_orders,
        expected.filter_unclaimed_external_orders
    );
    assert_eq!(
        config.exec_engine.generate_missing_orders,
        expected.generate_missing_orders
    );
}

#[test]
fn bolt_v3_startup_reconciliation_imports_external_open_order_into_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_external_order_registrations();
    let temp_dir = TempDir::new().unwrap();
    let open_order = open_order_fixture();
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    loaded.root.nautilus.load_state = open_order.load_state;
    loaded.root.nautilus.save_state = open_order.save_state;
    loaded.root.nautilus.delay_post_stop_seconds = open_order.delay_post_stop_seconds;
    loaded.root.nautilus.timeout_disconnection_seconds = open_order.timeout_disconnection_seconds;
    loaded.root.nautilus.timeout_reconciliation_seconds = open_order.timeout_reconciliation_seconds;
    loaded
        .root
        .nautilus
        .exec_engine
        .reconciliation_startup_delay_seconds = open_order.reconciliation_startup_delay_seconds;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let up = instrument_id(&open_order.condition_id, &open_order.up_token_id, &loaded);
    let down = instrument_id(&open_order.condition_id, &open_order.down_token_id, &loaded);
    let instruments = vec![
        binary_option(up, &open_order.up_token_id, &open_order),
        binary_option(down, &open_order.down_token_id, &open_order),
    ];
    let mass_status = external_open_order_mass_status(&loaded, up, &open_order);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) = register_bolt_v3_clients(
        builder,
        mock_client_configs_from_loaded(&loaded, instruments, mass_status),
    )
    .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    node.kernel()
        .cache()
        .borrow_mut()
        .add_account(account_from_fixture(&loaded, &open_order))
        .expect("mock account should seed NT cache before startup reconciliation");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            node.start()
                .await
                .expect("mock-only LiveNode start should run startup reconciliation");
            assert_eq!(node.state(), NodeState::Running);

            let registrations = recorded_mock_external_order_registrations();
            assert_eq!(registrations.len(), 1);
            assert_eq!(
                registrations[0].venue_order_id,
                VenueOrderId::from(open_order.venue_order_id.as_str())
            );
            assert_eq!(registrations[0].instrument_id, up);

            {
                let cache_handle = node.kernel().cache();
                let cache = cache_handle.borrow();
                let order_side = fixture_value("order_side", &open_order.order_side);
                let open_orders = cache.orders_open(None, Some(&up), None, None, Some(order_side));
                assert_eq!(
                    open_orders.len(),
                    1,
                    "startup reconciliation should import external open order into NT cache"
                );
            }

            node.stop()
                .await
                .expect("mock-only LiveNode stop should succeed");
        });
}

#[test]
fn pinned_nt_startup_reconciliation_registers_external_orders_with_execution_clients() {
    let nt_root = pinned_nt_checkout();
    let node_source = fs::read_to_string(nt_root.join("crates/live/src/node.rs"))
        .expect("pinned NT live node source should read");
    let reconciliation_block = node_source
        .split("async fn perform_startup_reconciliation")
        .nth(1)
        .expect("pinned NT live node should define startup reconciliation")
        .split("async fn run_reconciliation_checks")
        .next()
        .expect("startup reconciliation block should precede executor init");

    assert!(
        reconciliation_block.contains("reconcile_execution_mass_status"),
        "NT startup reconciliation should reconcile mass status through live manager"
    );
    assert!(
        reconciliation_block.contains("result.external_orders"),
        "NT startup reconciliation should surface external orders from reconciliation result"
    );
    assert!(
        reconciliation_block.contains("exec_engine.register_external_order"),
        "NT startup reconciliation should hand external orders back to execution clients"
    );
}

#[test]
fn pinned_nt_polymarket_can_generate_mass_status_but_does_not_track_external_orders() {
    let nt_root = pinned_nt_checkout();
    let source =
        fs::read_to_string(nt_root.join("crates/adapters/polymarket/src/execution/mod.rs"))
            .expect("pinned NT Polymarket execution source should read");
    let mass_status_method = source
        .split("async fn generate_mass_status")
        .nth(1)
        .expect("Polymarket execution client should implement mass status generation")
        .split("fn process_cancel_result")
        .next()
        .expect("mass status method should precede cancel helper");

    assert!(
        mass_status_method.contains("reconciliation::generate_mass_status"),
        "Polymarket adapter should delegate mass-status generation to its reconciliation module"
    );

    let register_method = source
        .split("fn register_external_order")
        .nth(1)
        .expect("Polymarket execution client should implement external-order registration hook")
        .split("fn on_instrument")
        .next()
        .expect("external-order registration hook should precede instrument callback");
    let register_body = register_method
        .split_once('{')
        .and_then(|(_, rest)| rest.rsplit_once('}').map(|(body, _)| body))
        .expect("external-order registration hook body should parse");
    let non_empty_lines = register_body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    assert!(
        non_empty_lines.is_empty(),
        "Polymarket external-order registration hook should currently be empty; \
         if upstream NT implements this, F10 blocker status must be re-evaluated: {non_empty_lines:?}"
    );
}

fn runtime_test_mutex() -> &'static Mutex<()> {
    RUNTIME_TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

fn open_order_fixture() -> OpenOrderFixture {
    let path = support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/reconciliation/open_order.toml",
    );
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()))
}

fn fixture_value<T>(field: &str, value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| panic!("fixture field {field}={value:?} should parse: {error}"))
}

fn strategy_config(loaded: &LoadedBoltV3Config) -> &bolt_v2::bolt_v3_config::BoltV3StrategyConfig {
    &loaded
        .strategies
        .first()
        .expect("fixture should load one strategy")
        .config
}

fn execution_client_id(loaded: &LoadedBoltV3Config) -> String {
    strategy_config(loaded).execution_client_id.clone()
}

fn execution_account_id(loaded: &LoadedBoltV3Config, client_id: &str) -> String {
    loaded
        .root
        .clients
        .get(client_id)
        .and_then(|client| client.execution.as_ref())
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("account_id"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("{client_id} execution config should include account_id"))
        .to_string()
}

fn execution_venue(loaded: &LoadedBoltV3Config) -> String {
    let client_id = execution_client_id(loaded);
    loaded
        .root
        .clients
        .get(&client_id)
        .unwrap_or_else(|| panic!("{client_id} should exist in root clients"))
        .venue
        .as_str()
        .to_string()
}

fn instrument_id(condition_id: &str, token_id: &str, loaded: &LoadedBoltV3Config) -> InstrumentId {
    InstrumentId::from(format!("{condition_id}-{token_id}.{}", execution_venue(loaded)).as_str())
}

fn binary_option(
    instrument_id: InstrumentId,
    token_id: &str,
    fixture: &OpenOrderFixture,
) -> InstrumentAny {
    let price_increment = Price::from(fixture.price_increment.as_str());
    let size_increment = Quantity::from(fixture.size_increment.as_str());
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        Symbol::new(token_id),
        fixture_value::<AssetClass>("asset_class", &fixture.asset_class),
        fixture_value::<Currency>("currency", &fixture.currency),
        UnixNanos::from(fixture.activation_ts_ns),
        UnixNanos::from(fixture.expiration_ts_ns),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn external_open_order_mass_status(
    loaded: &LoadedBoltV3Config,
    instrument_id: InstrumentId,
    fixture: &OpenOrderFixture,
) -> ExecutionMassStatus {
    let client_id = execution_client_id(loaded);
    let account_id = execution_account_id(loaded, &client_id);
    let ts = UnixNanos::from(fixture.report_ts_ns);
    let mut mass_status = ExecutionMassStatus::new(
        ClientId::from(client_id.as_str()),
        AccountId::from(account_id.as_str()),
        Venue::from(execution_venue(loaded).as_str()),
        ts,
        None,
    );
    let order = OrderStatusReport::new(
        AccountId::from(account_id.as_str()),
        instrument_id,
        None,
        VenueOrderId::from(fixture.venue_order_id.as_str()),
        fixture_value::<OrderSide>("order_side", &fixture.order_side),
        fixture_value::<OrderType>("order_type", &fixture.order_type),
        fixture_value::<TimeInForce>("time_in_force", &fixture.time_in_force),
        fixture_value::<OrderStatus>("order_status", &fixture.order_status),
        Quantity::from(fixture.order_quantity.as_str()),
        Quantity::from(fixture.filled_quantity.as_str()),
        ts,
        ts,
        ts,
        None,
    )
    .with_price(Price::from(fixture.order_price.as_str()));
    mass_status.add_order_reports(vec![order]);
    mass_status
}

fn account_from_fixture(loaded: &LoadedBoltV3Config, fixture: &OpenOrderFixture) -> AccountAny {
    let client_id = execution_client_id(loaded);
    let account_id = execution_account_id(loaded, &client_id);
    let base_currency =
        fixture_value::<Currency>("account_base_currency", &fixture.account_base_currency);
    let account_balance = AccountBalance::new(
        Money::from(fixture.account_total.as_str()),
        Money::from(fixture.account_locked.as_str()),
        Money::from(fixture.account_free.as_str()),
    );
    AccountState::new(
        AccountId::from(account_id.as_str()),
        fixture_value::<AccountType>("account_type", &fixture.account_type),
        vec![account_balance],
        Vec::new(),
        true,
        UUID4::new(),
        UnixNanos::from(fixture.report_ts_ns),
        UnixNanos::from(fixture.report_ts_ns),
        Some(base_currency),
    )
    .into()
}

fn mock_client_configs_from_loaded(
    loaded: &LoadedBoltV3Config,
    startup_instruments: Vec<InstrumentAny>,
    mass_status: ExecutionMassStatus,
) -> BoltV3ClientConfigs {
    let execution_client_id = execution_client_id(loaded);
    let clients = loaded
        .root
        .clients
        .iter()
        .map(|(client_id, client)| {
            let venue = client.venue.as_str();
            let data_config = if *client_id == execution_client_id {
                MockDataClientConfig::new(client_id, venue)
                    .with_startup_instruments(startup_instruments.clone())
            } else {
                MockDataClientConfig::new(client_id, venue)
            };
            let data = client.data.as_ref().map(|_| BoltV3DataClientAdapterConfig {
                factory: Box::new(MockDataClientFactory),
                config: Box::new(data_config),
            });
            let execution = client.execution.as_ref().map(|_| {
                let config = MockExecClientConfig::new(
                    client_id,
                    execution_account_id(loaded, client_id).as_str(),
                    venue,
                )
                .with_mass_status(mass_status.clone());
                BoltV3ExecutionClientAdapterConfig {
                    factory: Box::new(MockExecutionClientFactory),
                    config: Box::new(config),
                }
            });
            (client_id.clone(), BoltV3ClientConfig { data, execution })
        })
        .collect::<BTreeMap<_, _>>();
    BoltV3ClientConfigs { clients }
}

fn pinned_nt_checkout() -> PathBuf {
    let revision =
        bolt_v3_compiled_nautilus_trader_revision().expect("Cargo.toml should pin one NT revision");
    let short_revision = revision
        .get(..7)
        .expect("NT revision should be at least 7 chars");
    let cargo_home = cargo_home();
    let checkouts = cargo_home.join("git/checkouts");

    for entry in fs::read_dir(&checkouts).expect("Cargo git checkouts dir should read") {
        let entry = entry.expect("Cargo git checkout entry should read");
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.starts_with("nautilus_trader-") {
            continue;
        }
        let candidate = entry.path().join(short_revision);
        if candidate.is_dir() {
            return candidate;
        }
    }

    panic!(
        "pinned NT checkout {short_revision} not found under {}; run cargo fetch/test first",
        checkouts.display()
    );
}

fn cargo_home() -> PathBuf {
    if let Some(path) = env::var_os("CARGO_HOME") {
        return PathBuf::from(path);
    }

    let home = env::var_os("HOME").expect("HOME should be set when CARGO_HOME is unset");
    Path::new(&home).join(".cargo")
}
