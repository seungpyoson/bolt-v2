//! Integration tests for the bolt-v3 client registration boundary.
//!
//! These tests guard the contract that:
//!   1. The bolt-v3 LiveNode build path actually invokes the
//!      registration boundary after adapter mapping.
//!   2. NT client registration only fires after secret resolution and
//!      adapter mapping both succeed; missing or mismatched secrets
//!      surface as the matching `BoltV3LiveNodeError` variant *before*
//!      registration.
//!   3. Registered NT client kinds match the configured client blocks
//!      (verified via `data_engine.registered_clients()` and
//!      `exec_engine.client_ids()` after `LiveNodeBuilder::build`).
//!   4. The registration module source itself does not introduce any
//!      connect / disconnect / run / subscribe / order-submit path.

mod support;

use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, ClientBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{
        BoltV3LiveNodeError, build_bolt_v3_all_configured_client_mapping_live_node_with_summary,
        build_bolt_v3_live_node_with_summary,
    },
};
use nautilus_model::identifiers::ClientId;

fn fixture_loaded_config_with_binance_reference() -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.insert(
        "binance_reference".to_string(),
        toml::from_str(&support::repo_text(
            "tests/fixtures/bolt_v3/binance_reference_client.toml",
        ))
        .expect("binance provider fixture client should parse"),
    );
    loaded
}

fn data_client_from_toml(value: &str) -> ClientBlock {
    toml::from_str(value).expect("test data client block should parse")
}

fn add_all_requested_data_clients(loaded: &mut LoadedBoltV3Config) {
    let clients: &[(&str, &str)] = &[
        (
            "binance_spot_data",
            r#"
venue = "BINANCE"

[data]
product_type = "spot"
environment = "mainnet"
base_url_http = "https://api.binance.com"
base_url_ws = "wss://stream-sbe.binance.com/ws"
spot_market_data_mode = "sbe"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"

[secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
"#,
        ),
        (
            "binance_usdm_data",
            r#"
venue = "BINANCE"

[data]
product_type = "usd_m"
environment = "testnet"
base_url_http = "https://demo-fapi.binance.com"
base_url_ws = "wss://fstream.binancefuture.com/ws"
spot_market_data_mode = "sbe"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"

[secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
"#,
        ),
        (
            "binance_coinm_data",
            r#"
venue = "BINANCE"

[data]
product_type = "coin_m"
environment = "testnet"
base_url_http = "https://testnet.binancefuture.com"
base_url_ws = "wss://dstream.binancefuture.com/ws"
spot_market_data_mode = "sbe"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"

[secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
"#,
        ),
        (
            "bitmex_data",
            r#"
venue = "BITMEX"

[data]
environment = "testnet"
active_only = false
transport_backend = "sockudo"
"#,
        ),
        (
            "bybit_data",
            r#"
venue = "BYBIT"

[data]
product_types = ["spot", "linear", "inverse", "option"]
environment = "testnet"
transport_backend = "sockudo"
"#,
        ),
        (
            "coinbase_data",
            r#"
venue = "COINBASE"

[data]
environment = "Live"
transport_backend = "sockudo"
"#,
        ),
        (
            "deribit_data",
            r#"
venue = "DERIBIT"

[data]
product_types = ["future", "option", "spot", "future_combo", "option_combo"]
environment = "testnet"
transport_backend = "sockudo"
"#,
        ),
        (
            "kraken_spot_data",
            r#"
venue = "KRAKEN"

[data]
product_type = "spot"
environment = "live"
transport_backend = "sockudo"
"#,
        ),
        (
            "kraken_futures_data",
            r#"
venue = "KRAKEN"

[data]
product_type = "futures"
environment = "demo"
transport_backend = "sockudo"
"#,
        ),
        (
            "okx_data",
            r#"
venue = "OKX"

[data]
instrument_types = ["SPOT", "MARGIN", "SWAP", "FUTURES", "EVENTS"]
contract_types = ["linear", "inverse"]
load_spreads = true
environment = "demo"
transport_backend = "sockudo"
"#,
        ),
    ];

    for (client_key, client_toml) in clients {
        loaded.root.clients.insert(
            (*client_key).to_string(),
            data_client_from_toml(client_toml),
        );
    }
}

#[test]
fn live_node_build_path_registers_only_strategy_bound_signal_data_and_exec() {
    // The trade build path (`build_bolt_v3_live_node_with_summary`) registers
    // ONLY strategy-bound clients: the strategy `execution_client_id`, its
    // `[signal_data].*.data_client_id`, its enabled
    // runtime-available `[reference_current_price].source.*.client_id`, and the proof-policy
    // `execution_client_id`. The fixture strategy
    // (tests/fixtures/bolt_v3/strategies/binary_oracle.toml) sets
    // `execution_client_id = "polymarket_main"` and a strategy-bound signal
    // feed at `okx_data`, plus the strategy-bound reference-current-price
    // sources at `chainlink_reference` and `polyresearch_reference`. The extra
    // `binance_reference` client is unbound and must still be excluded.
    //
    // We keep `fixture_loaded_config_with_binance_reference` so the exclusion is
    // meaningful: an unbound client is present in config yet must NOT register.
    //
    // Coverage of the SEPARATE concerns:
    //   - broad-readiness registration of every requested data client (without
    //     extra execution clients) is covered by
    //     `live_node_registration_can_load_all_requested_data_clients_without_extra_execution_clients`,
    //     which exercises
    //     `build_bolt_v3_all_configured_client_mapping_live_node_with_summary`;
    //   - positive signal-data scoping (a strategy-bound signal data
    //     client IS registered) is covered by the
    //     `trade_transport_config_keeps_only_strategy_bound_clients` unit test.
    let mut loaded = fixture_loaded_config_with_binance_reference();
    let temp = support::TempCaseDir::new("bolt-v3-client-registration-build-path");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let (node, summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build through the registration boundary");

    // The scoped trade runner records exactly the strategy-bound
    // execution/data client, signal data client, and reference-current-price
    // clients.
    assert_eq!(
        summary.clients.len(),
        4,
        "scoped trade path registers only strategy-bound clients; got {:?}",
        summary.clients.keys().collect::<Vec<_>>()
    );
    let polymarket = summary
        .clients
        .get("polymarket_main")
        .expect("polymarket_main (strategy execution_client_id) must appear in summary");
    assert!(
        polymarket.data,
        "fixture polymarket_main has a [data] block"
    );
    assert!(
        polymarket.execution,
        "fixture polymarket_main has an [execution] block"
    );
    let okx = summary
        .clients
        .get("okx_data")
        .expect("okx_data (strategy signal_data client) must appear in summary");
    assert!(okx.data, "fixture okx_data has a [data] block");
    assert!(!okx.execution, "fixture okx_data has no [execution] block");
    let chainlink = summary
        .clients
        .get("chainlink_reference")
        .expect("chainlink_reference (strategy reference_current_price source client) must appear in summary");
    assert!(
        chainlink.data,
        "fixture chainlink_reference has a [data] block"
    );
    assert!(
        !chainlink.execution,
        "fixture chainlink_reference has no [execution] block"
    );
    let polyresearch = summary
        .clients
        .get("polyresearch_reference")
        .expect("polyresearch_reference (strategy reference_current_price source client) must appear in summary");
    assert!(
        polyresearch.data,
        "fixture polyresearch_reference has a [data] block"
    );
    assert!(
        !polyresearch.execution,
        "fixture polyresearch_reference has no [execution] block"
    );
    assert!(
        !summary.clients.contains_key("binance_reference"),
        "binance_reference is unbound (no strategy signal_data or reference_current_price binding) and must be excluded from the scoped summary; got {:?}",
        summary.clients.keys().collect::<Vec<_>>()
    );

    // NT-side state confirms the actual registrations happened and that the
    // unbound probe client was excluded all the way through `factory.create`
    // and `engine.register_client`, without a parallel NT mock. The bolt-v3
    // venue identifier is reused as the NT registration name, so the NT
    // engines expose ClientIds matching the configured keys.
    let registered_data: Vec<ClientId> = node.registered_data_client_ids();
    assert!(
        registered_data.contains(&ClientId::from("polymarket_main")),
        "data engine should expose polymarket_main; got {registered_data:?}"
    );
    assert!(
        registered_data.contains(&ClientId::from("okx_data")),
        "data engine should expose strategy signal data client okx_data; got {registered_data:?}"
    );
    assert!(
        registered_data.contains(&ClientId::from("chainlink_reference")),
        "data engine should expose strategy reference_current_price source client chainlink_reference; got {registered_data:?}"
    );
    assert!(
        registered_data.contains(&ClientId::from("polyresearch_reference")),
        "data engine should expose strategy reference_current_price source client polyresearch_reference; got {registered_data:?}"
    );
    assert!(
        !registered_data.contains(&ClientId::from("binance_reference")),
        "scoped runner must EXCLUDE the unbound binance_reference data client; got {registered_data:?}"
    );

    let registered_exec: Vec<ClientId> = node.registered_exec_client_ids();
    assert!(
        registered_exec.contains(&ClientId::from("polymarket_main")),
        "exec engine should expose polymarket_main; got {registered_exec:?}"
    );
    assert!(
        !registered_exec.contains(&ClientId::from("binance_reference")),
        "binance_reference has no [execution] block and is unbound, must not be on the exec engine; got {registered_exec:?}"
    );
}

#[test]
fn live_node_registration_can_load_all_requested_data_clients_without_extra_execution_clients() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    add_all_requested_data_clients(&mut loaded);
    let temp = support::TempCaseDir::new("bolt-v3-all-requested-data-client-registration");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let (node, summary) = build_bolt_v3_all_configured_client_mapping_live_node_with_summary(
        &loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("all requested data clients should register through the LiveNode boundary");

    for client_key in [
        "polymarket_main",
        "binance_spot_data",
        "binance_usdm_data",
        "binance_coinm_data",
        "bitmex_data",
        "bybit_data",
        "coinbase_data",
        "deribit_data",
        "kraken_spot_data",
        "kraken_futures_data",
        "okx_data",
    ] {
        let row = summary
            .clients
            .get(client_key)
            .unwrap_or_else(|| panic!("{client_key} must appear in registration summary"));
        assert!(row.data, "{client_key} must register as data-capable");
        assert!(
            node.registered_data_client_ids()
                .contains(&ClientId::from(client_key)),
            "{client_key} must be registered with NT data engine"
        );
    }

    let mut expected_exec: Vec<ClientId> = loaded
        .root
        .clients
        .iter()
        .filter(|(_, client)| client.execution.is_some())
        .map(|(client_key, _)| ClientId::from(client_key.as_str()))
        .collect();
    expected_exec.sort();
    let mut registered_exec = node.registered_exec_client_ids();
    registered_exec.sort();
    assert_eq!(
        registered_exec, expected_exec,
        "registering requested data clients must not create execution clients beyond TOML-declared [execution] blocks"
    );
}

#[test]
fn missing_polymarket_private_key_secret_fails_before_registration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Inject a resolver that fails on the polymarket private_key SSM
    // path. This must surface as `SecretResolution`, never reaching
    // registration.
    let bad_resolver = |region: &str, path: &str| -> Result<String, &'static str> {
        if path == "/bolt/polymarket/private-key" {
            Err("simulated SSM permissions denied for polymarket private key")
        } else {
            support::fake_bolt_v3_resolver(region, path)
        }
    };
    let error = build_bolt_v3_live_node_with_summary(&loaded, |_| false, bad_resolver)
        .expect_err("missing private key must block before registration");

    assert!(
        matches!(error, BoltV3LiveNodeError::SecretResolution(_)),
        "expected SecretResolution variant, got {error:?}"
    );
}

#[test]
fn forbidden_credential_env_var_fails_before_registration() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Forbidden env-var detection is the very first gate; registration
    // must never run when a credential env var is set.
    let error = build_bolt_v3_live_node_with_summary(
        &loaded,
        |var| var == "POLYMARKET_PK",
        support::fake_bolt_v3_resolver,
    )
    .expect_err("forbidden env var must block before registration");
    assert!(
        matches!(error, BoltV3LiveNodeError::ForbiddenEnv(_)),
        "expected ForbiddenEnv variant, got {error:?}"
    );
}

#[test]
fn registration_module_remains_a_no_trade_boundary() {
    // Source-level inspection of the registration module. The module
    // is allowed to name NT factories (that is the whole point), but
    // it must never call connect, disconnect, run, subscribe, market
    // selection, order construction, or order submission. Forbidden
    // tokens live in this integration test (not in the module's own
    // source) so the assertion does not self-trip.
    let source = include_str!("../src/bolt_v3_client_registration.rs");
    for forbidden in [
        ".connect(",
        ".disconnect(",
        "node.run(",
        ".start(",
        ".stop(",
        "subscribe_quote_ticks",
        "subscribe_trade_ticks",
        "subscribe_order_book_deltas",
        "subscribe_order_book_snapshots",
        "subscribe_instruments",
        "select_market",
        "submit_order",
        "submit_order_list",
        "modify_order",
        "cancel_order",
        "OrderBuilder",
        "PolymarketOrderBuilder",
        "OrderSubmitter",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_client_registration.rs must remain a no-trade boundary; \
             source unexpectedly references `{forbidden}`"
        );
    }
}

#[test]
fn empty_clients_root_config_registers_zero_clients() {
    // Build a synthetic root config with zero clients so registration
    // must succeed but produce an empty summary, and the resulting
    // node must expose no registered NT clients.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-empty-client-registration");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let empty_root = BoltV3RootConfig {
        clients: BTreeMap::new(),
        ..loaded.root.clone()
    };
    let empty_loaded = LoadedBoltV3Config {
        root_path: loaded.root_path.clone(),
        config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        root: empty_root,
        strategies: Vec::new(),
    };

    // No clients means no SSM paths are touched; the resolver is never
    // called, so the closure body cannot be reached.
    let resolver = |_region: &str, _path: &str| -> Result<String, &'static str> {
        Err("resolver must not be called when no clients are configured")
    };
    let (node, summary) = build_bolt_v3_live_node_with_summary(&empty_loaded, |_| false, resolver)
        .expect("empty client set should still build a clean LiveNode");
    assert!(summary.clients.is_empty());
    assert!(node.registered_data_client_ids().is_empty());
    assert!(node.registered_exec_client_ids().is_empty());
}
