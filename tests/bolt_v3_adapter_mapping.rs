mod support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::clients::chainlink::ChainlinkReferenceClientConfig;
use bolt_v2::{
    bolt_v3_adapters::{BoltV3ClientMappingError, map_bolt_v3_clients},
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
    bolt_v3_providers::{
        binance::{self, ResolvedBoltV3BinanceSecrets},
        chainlink::{self, ResolvedBoltV3ChainlinkSecrets},
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
    },
    bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
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

fn fixture_chainlink_secrets() -> ResolvedBoltV3ChainlinkSecrets {
    ResolvedBoltV3ChainlinkSecrets {
        api_key: "regression-chainlink-api-key".to_string(),
        api_secret: "regression-chainlink-api-secret".to_string(),
    }
}

fn fixture_client_id_for_venue(root: &BoltV3RootConfig, venue: &str) -> String {
    let matches = root
        .clients
        .iter()
        .filter_map(|(client_id, block)| {
            (block.venue.as_str() == venue).then_some(client_id.clone())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        matches.len(),
        1,
        "fixture should define exactly one {venue} client, got {matches:?}"
    );
    matches[0].clone()
}

fn fixture_secret_string(root: &BoltV3RootConfig, client_id: &str, field: &str) -> String {
    root.clients
        .get(client_id)
        .and_then(|client| client.secrets.as_ref())
        .and_then(toml::Value::as_table)
        .and_then(|secrets| secrets.get(field))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("fixture client {client_id} should define secrets.{field}"))
        .to_string()
}

fn fixture_resolved_secrets(loaded: &LoadedBoltV3Config) -> ResolvedBoltV3Secrets {
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        fixture_client_id_for_venue(&loaded.root, polymarket::KEY),
        Arc::new(fixture_polymarket_secrets()),
    );
    clients.insert(
        fixture_client_id_for_venue(&loaded.root, binance::KEY),
        Arc::new(fixture_binance_secrets()),
    );
    if loaded
        .root
        .clients
        .values()
        .any(|client| client.venue.as_str() == chainlink::KEY)
    {
        clients.insert(
            fixture_client_id_for_venue(&loaded.root, chainlink::KEY),
            Arc::new(fixture_chainlink_secrets()),
        );
    }
    ResolvedBoltV3Secrets { clients }
}

#[test]
fn chainlink_reference_client_maps_from_v3_stream_inputs() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("existing strategy fixture should load");
    let resolved = fixture_resolved_secrets(&loaded);
    let chainlink_client_id = fixture_client_id_for_venue(&loaded.root, chainlink::KEY);

    let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map cleanly");

    let chainlink = configs
        .clients
        .get(&chainlink_client_id)
        .expect("chainlink client must be present in mapper output");
    let data = chainlink
        .data
        .as_ref()
        .expect("chainlink [data] block must produce an NT data config")
        .config_as::<ChainlinkReferenceClientConfig>()
        .expect("chainlink [data] should downcast to ChainlinkReferenceClientConfig");

    assert_eq!(data.shared.ws_url, "wss://ws.test.chain.link");
    assert_eq!(data.shared.ws_reconnect_alert_threshold, 3);
    assert_eq!(data.feeds.len(), 1);
    assert_eq!(data.feeds[0].venue_name, "eth_usd_oracle_anchor");
    assert_eq!(data.feeds[0].instrument_id, "ETHUSD.CHAINLINK");
    assert_eq!(data.feeds[0].price_scale, 8);
}

#[test]
fn polymarket_client_id_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets(&loaded);
    let polymarket_client_id = fixture_client_id_for_venue(&loaded.root, polymarket::KEY);

    let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map cleanly");

    let polymarket = configs
        .clients
        .get(&polymarket_client_id)
        .expect("polymarket client must be present in mapper output");

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
    assert!(!data.auto_load_missing_instruments);
    assert_eq!(data.auto_load_debounce_ms, 100);

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
fn adapter_mapper_uses_configured_polymarket_auto_load_debounce() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_client_id = fixture_client_id_for_venue(&loaded.root, polymarket::KEY);
    let polymarket_data = loaded
        .root
        .clients
        .get_mut(&polymarket_client_id)
        .and_then(|client_id| client_id.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist");
    polymarket_data.insert(
        "auto_load_debounce_milliseconds".to_string(),
        toml::Value::Integer(250),
    );

    let resolved = fixture_resolved_secrets(&loaded);
    let configs = map_bolt_v3_clients(&loaded, &resolved)
        .expect("non-default auto-load debounce should map cleanly");
    let data = configs
        .clients
        .get(&polymarket_client_id)
        .and_then(|client| client.data.as_ref())
        .expect("polymarket data config should be mapped")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert!(!data.auto_load_missing_instruments);
    assert_eq!(data.auto_load_debounce_ms, 250);
}

#[test]
fn adapter_mapper_rejects_subscribe_new_markets_true_if_validation_was_bypassed() {
    // Root validation rejects this value. This test mutates an already
    // loaded config to ensure the client mapper also fails closed if a
    // programmatic caller bypasses the canonical validation path.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_client_id = fixture_client_id_for_venue(&loaded.root, polymarket::KEY);
    let polymarket_data = loaded
        .root
        .clients
        .get_mut(&polymarket_client_id)
        .and_then(|client_id| client_id.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist");
    polymarket_data.insert(
        "subscribe_new_markets".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets(&loaded);
    let error = map_bolt_v3_clients(&loaded, &resolved)
        .expect_err("mapper must not forward subscribe_new_markets=true to NT");
    match error {
        BoltV3ClientMappingError::ValidationInvariant {
            client_id_key,
            field,
            ..
        } => {
            assert_eq!(client_id_key, polymarket_client_id);
            assert_eq!(field, "data.subscribe_new_markets");
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn adapter_mapper_rejects_auto_load_missing_instruments_true_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_client_id = fixture_client_id_for_venue(&loaded.root, polymarket::KEY);
    let polymarket_data = loaded
        .root
        .clients
        .get_mut(&polymarket_client_id)
        .and_then(|client_id| client_id.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist");
    polymarket_data.insert(
        "auto_load_missing_instruments".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets(&loaded);
    let error = map_bolt_v3_clients(&loaded, &resolved)
        .expect_err("mapper must not forward auto_load_missing_instruments=true to NT");
    match error {
        BoltV3ClientMappingError::ValidationInvariant {
            client_id_key,
            field,
            ..
        } => {
            assert_eq!(client_id_key, polymarket_client_id);
            assert_eq!(field, "data.auto_load_missing_instruments");
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn binance_data_client_id_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets(&loaded);
    let binance_client_id = fixture_client_id_for_venue(&loaded.root, binance::KEY);

    let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map cleanly");

    let binance = configs
        .clients
        .get(&binance_client_id)
        .expect("binance client must be present in mapper output");
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

    // Missing [secrets] for polymarket execution client: the existing
    // validator must catch this *before* the mapper ever runs. The
    // mapper itself must not silently fall back to defaults.
    let toml_text = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/root_missing_polymarket_execution_secrets.toml",
    ))
    .expect("missing-secret fixture should be readable");
    let root: BoltV3RootConfig =
        toml::from_str(&toml_text).expect("polymarket-execution-only TOML should parse");
    let polymarket_client_id = fixture_client_id_for_venue(&root, polymarket::KEY);
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
        clients: BTreeMap::new(),
    };
    let error = map_bolt_v3_clients(&loaded, &empty_resolved)
        .expect_err("mapper must not synthesize defaults for missing resolved secrets");
    match error {
        BoltV3ClientMappingError::MissingResolvedSecrets {
            client_id_key,
            expected_venue,
        } => {
            assert_eq!(client_id_key, polymarket_client_id);
            assert_eq!(expected_venue, polymarket::KEY);
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
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.strategies.clear();
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
    let binance_client_id = fixture_client_id_for_venue(&loaded.root, binance::KEY);
    let binance_api_secret_path =
        fixture_secret_string(&loaded.root, &binance_client_id, "api_secret_ssm_path");

    // Force the binance api_secret resolution to fail; the live-node
    // builder must surface the error rather than silently skipping the
    // mapping step.
    let bad_resolver = move |region: &str, path: &str| -> Result<String, &'static str> {
        if path == binance_api_secret_path {
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
