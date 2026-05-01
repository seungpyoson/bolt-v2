mod support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::{
    bolt_v3_adapters::{BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
    bolt_v3_providers::polymarket,
    bolt_v3_secrets::{
        ResolvedBoltV3BinanceSecrets, ResolvedBoltV3PolymarketSecrets, ResolvedBoltV3Secrets,
        ResolvedBoltV3VenueSecrets,
    },
};
use nautilus_binance::common::enums::{
    BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
};
use nautilus_binance::config::BinanceDataClientConfig;
use nautilus_polymarket::{
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
};

fn fixture_polymarket_secrets() -> ResolvedBoltV3PolymarketSecrets {
    ResolvedBoltV3PolymarketSecrets {
        private_key: "regression-poly-private-key".to_string(),
        api_key: "regression-poly-api-key".to_string(),
        api_secret: "regression-poly-api-secret".to_string(),
        passphrase: "regression-poly-passphrase".to_string(),
    }
}

fn fixture_binance_secrets() -> ResolvedBoltV3BinanceSecrets {
    ResolvedBoltV3BinanceSecrets {
        api_key: "regression-binance-api-key".to_string(),
        api_secret: "regression-binance-api-secret".to_string(),
    }
}

fn fixture_resolved_secrets() -> ResolvedBoltV3Secrets {
    let mut venues: BTreeMap<String, ResolvedBoltV3VenueSecrets> = BTreeMap::new();
    venues.insert(
        "polymarket_main".to_string(),
        Arc::new(fixture_polymarket_secrets()),
    );
    venues.insert(
        "binance_reference".to_string(),
        Arc::new(fixture_binance_secrets()),
    );
    ResolvedBoltV3Secrets { venues }
}

#[test]
fn polymarket_venue_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();

    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");

    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket [data] should downcast to NT PolymarketDataClientConfig");
    assert_eq!(
        data.base_url_http.as_deref(),
        Some("https://clob.polymarket.com")
    );
    assert_eq!(
        data.base_url_ws.as_deref(),
        Some("wss://ws-subscriptions-clob.polymarket.com/ws/market")
    );
    assert_eq!(
        data.base_url_gamma.as_deref(),
        Some("https://gamma-api.polymarket.com")
    );
    assert_eq!(
        data.base_url_data_api.as_deref(),
        Some("https://data-api.polymarket.com")
    );
    assert_eq!(data.http_timeout_secs, 60);
    assert_eq!(data.ws_timeout_secs, 30);
    assert_eq!(data.ws_max_subscriptions, 200);
    assert_eq!(data.update_instruments_interval_mins, 60);
    assert!(!data.subscribe_new_markets);

    let exec = polymarket
        .execution
        .as_ref()
        .expect("polymarket [execution] block must produce an NT exec config")
        .config_as::<PolymarketExecClientConfig>()
        .expect("polymarket [execution] should downcast to NT PolymarketExecClientConfig");
    assert_eq!(exec.signature_type, NtPolymarketSignatureType::PolyProxy);
    assert_eq!(
        exec.private_key.as_deref(),
        Some(fixture_polymarket_secrets().private_key.as_str())
    );
    assert_eq!(
        exec.api_key.as_deref(),
        Some(fixture_polymarket_secrets().api_key.as_str())
    );
    assert_eq!(
        exec.api_secret.as_deref(),
        Some(fixture_polymarket_secrets().api_secret.as_str())
    );
    assert_eq!(
        exec.passphrase.as_deref(),
        Some(fixture_polymarket_secrets().passphrase.as_str())
    );
    assert_eq!(
        exec.funder.as_deref(),
        Some("0x1111111111111111111111111111111111111111")
    );
    assert_eq!(
        exec.base_url_http.as_deref(),
        Some("https://clob.polymarket.com")
    );
    assert_eq!(
        exec.base_url_ws.as_deref(),
        Some("wss://ws-subscriptions-clob.polymarket.com/ws/user")
    );
    assert_eq!(
        exec.base_url_data_api.as_deref(),
        Some("https://data-api.polymarket.com")
    );
    assert_eq!(exec.http_timeout_secs, 60);
    assert_eq!(exec.max_retries, 3);
    assert_eq!(exec.retry_delay_initial_ms, 250);
    assert_eq!(exec.retry_delay_max_ms, 2000);
    assert_eq!(exec.ack_timeout_secs, 5);
}

#[test]
fn adapter_mapper_rejects_subscribe_new_markets_true_if_validation_was_bypassed() {
    // Root validation rejects this value. This test mutates an already
    // loaded config to ensure the adapter mapper also fails closed if a
    // programmatic caller bypasses the canonical validation path.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_data = loaded
        .root
        .venues
        .get_mut("polymarket_main")
        .and_then(|venue| venue.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist");
    polymarket_data.insert(
        "subscribe_new_markets".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must not forward subscribe_new_markets=true to NT");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key, field, ..
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(field, "data.subscribe_new_markets");
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn binance_data_venue_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();

    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

    let binance = configs
        .venues
        .get("binance_reference")
        .expect("binance_reference must be present in mapper output");
    let data = binance
        .data
        .as_ref()
        .expect("binance [data] block must produce an NT data config")
        .config_as::<BinanceDataClientConfig>()
        .expect("binance [data] should downcast to NT BinanceDataClientConfig");

    assert_eq!(data.product_types, vec![NtBinanceProductType::Spot]);
    assert_eq!(data.environment, NtBinanceEnvironment::Mainnet);
    // The bolt-v3 binance data schema now requires explicit
    // base_url_http and base_url_ws so NT cannot silently fall back to
    // its compiled-in default Binance endpoints. Both must arrive at
    // NT as `Some(...)` carrying the configured fixture value.
    assert_eq!(
        data.base_url_http.as_deref(),
        Some("https://api.binance.com")
    );
    assert_eq!(
        data.base_url_ws.as_deref(),
        Some("wss://stream.binance.com:9443/ws")
    );
    assert_eq!(
        data.api_key.as_deref(),
        Some(fixture_binance_secrets().api_key.as_str())
    );
    assert_eq!(
        data.api_secret.as_deref(),
        Some(fixture_binance_secrets().api_secret.as_str())
    );
    assert_eq!(data.instrument_status_poll_secs, 3600);
}

#[test]
fn missing_or_invalid_root_config_remains_caught_by_validation_not_mapper_defaults() {
    use bolt_v2::bolt_v3_validate::validate_root_only;

    // Missing [secrets] for polymarket execution venue: the existing
    // validator must catch this *before* the mapper ever runs. The
    // mapper itself must not silently fall back to defaults.
    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "live"

[nautilus]
load_state = true
save_state = true
timeout_connection_seconds = 30
timeout_reconciliation_seconds = 60
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_client_ids = []
debug = false
graceful_shutdown_on_error = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_seconds = 0
external_client_ids = []
debug = false
reconciliation = true
reconciliation_startup_delay_seconds = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_milliseconds = 2000
inflight_check_threshold_milliseconds = 5000
inflight_check_retries = 5
open_check_interval_seconds = 0
open_check_lookback_mins = 60
open_check_threshold_milliseconds = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_milliseconds = 100
position_check_interval_seconds = 0
position_check_lookback_mins = 60
position_check_threshold_milliseconds = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_seconds = 0
graceful_shutdown_on_error = false
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"
nt_bypass = false
nt_max_order_submit_rate = "100/00:00:01"
nt_max_order_modify_rate = "100/00:00:01"
nt_max_notional_per_order = {}
nt_debug = false
nt_graceful_shutdown_on_error = false
nt_qsize = 100000

[logging]
standard_output_level = "INFO"
file_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_milliseconds = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-1"

[venues.polymarket_main]
kind = "polymarket"

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 60
max_retries = 3
retry_delay_initial_milliseconds = 250
retry_delay_max_milliseconds = 2000
ack_timeout_seconds = 5
"#;
    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket-execution-only TOML should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("required [secrets] block")),
        "validator must reject missing [secrets] for polymarket execution: {messages:#?}"
    );

    // Construct a LoadedBoltV3Config that bypassed validation so we can
    // confirm the mapper itself does not silently fill in adapter
    // defaults for the missing [secrets]: it must surface as a mapping
    // error driven by the resolved-secrets gap, not a default.
    let loaded = LoadedBoltV3Config {
        root_path: support::repo_path("tests/fixtures/bolt_v3/root.toml"),
        root,
        strategies: Vec::new(),
    };
    let empty_resolved = ResolvedBoltV3Secrets {
        venues: BTreeMap::new(),
    };
    let error = map_bolt_v3_adapters(&loaded, &empty_resolved)
        .expect_err("mapper must not synthesize defaults for missing resolved secrets");
    match error {
        BoltV3AdapterMappingError::MissingResolvedSecrets {
            venue_key,
            expected_provider_key,
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(expected_provider_key, polymarket::KEY);
        }
        other => panic!("expected MissingResolvedSecrets, got {other}"),
    }
}

#[test]
fn live_node_build_path_runs_adapter_mapping_after_secret_resolution() {
    // The fake resolver in `tests/support/mod.rs` returns a synthetic
    // PKCS8 Ed25519 secret for binance and placeholders for polymarket;
    // the mapper sits between secret resolution and LiveNode::build, so
    // a successful build proves the mapper accepted the resolved
    // secrets without the build path silently bypassing it.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let _node = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("v3 LiveNode should build through the adapter mapping boundary");
}

#[test]
fn live_node_build_path_propagates_adapter_mapping_failures() {
    // Inject a resolver that hands back an empty string for a polymarket
    // SSM path. Resolution itself succeeds (the resolver is the source
    // of truth for "I got a value"), and then the mapper boundary plumbs
    // the resolved secrets into PolymarketExecClientConfig where the
    // empty string round-trips into the NT-native field as Some("").
    //
    // This regression guards against future refactors that would skip
    // the adapter mapping step entirely. The current mapper does not
    // re-validate string shape; if future requirements need shape
    // checks at the mapper boundary, this test is the place to assert
    // that they fire.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Force the binance api_secret resolution to fail; the live-node
    // builder must surface the error rather than silently skipping the
    // mapping step.
    let bad_resolver = |region: &str, path: &str| -> Result<String, &'static str> {
        if path == "/bolt/binance_reference/api_secret" {
            Err("simulated SSM permissions denied")
        } else {
            support::fake_bolt_v3_resolver(region, path)
        }
    };
    let error = build_bolt_v3_live_node_with(&loaded, |_| false, bad_resolver)
        .expect_err("resolver failure must surface through the live-node build path");
    matches!(error, BoltV3LiveNodeError::SecretResolution(_))
        .then_some(())
        .expect("expected SecretResolution error variant from build_bolt_v3_live_node_with");
}

#[test]
fn adapter_mapper_module_remains_a_no_trade_boundary() {
    // The mapper boundary is enforced by source-level inspection so a
    // future regression that pulls a factory or LiveNode runner into
    // the adapter module would fail in CI rather than silently break
    // the no-trade contract. Forbidden tokens are kept here in the
    // integration test (not in the module's own source) to avoid the
    // assertion self-tripping when it scans its own definition file.
    let source = include_str!("../src/bolt_v3_adapters.rs");
    for forbidden in [
        "PolymarketDataClientFactory",
        "PolymarketExecutionClientFactory",
        "BinanceDataClientFactory",
        "add_data_client",
        "add_exec_client",
        "register_data_client",
        "register_exec_client",
        ".connect(",
        ".disconnect(",
        ".run(",
        "LiveNode::build",
        "LiveNode::new",
        "submit_order",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_adapters.rs must remain a no-trade boundary; \
             source unexpectedly references `{forbidden}`"
        );
    }
}
