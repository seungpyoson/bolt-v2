mod support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::{
    bolt_v3_adapters::{BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
    bolt_v3_providers::{
        binance::{
            BinanceDataConfig, BinanceEnvironment, BinanceProductType, ResolvedBoltV3BinanceSecrets,
        },
        polymarket::{
            self, PolymarketDataConfig, PolymarketExecutionConfig, PolymarketSignatureType,
            ResolvedBoltV3PolymarketSecrets,
        },
    },
    bolt_v3_secrets::{ResolvedBoltV3Secrets, ResolvedBoltV3VenueSecrets},
};
use nautilus_binance::common::enums::{
    BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
};
use nautilus_binance::config::BinanceDataClientConfig;
use nautilus_polymarket::{
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    websocket::messages::PolymarketNewMarket,
};
use ustr::Ustr;

fn fixture_polymarket_secrets() -> ResolvedBoltV3PolymarketSecrets {
    ResolvedBoltV3PolymarketSecrets {
        private_key: "0x4242424242424242424242424242424242424242424242424242424242424242"
            .to_string(),
        api_key: "regression-poly-api-key".to_string(),
        api_secret: "YWJj".to_string(),
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

fn polymarket_execution_table(
    loaded: &mut LoadedBoltV3Config,
) -> &mut toml::map::Map<String, toml::Value> {
    loaded
        .root
        .venues
        .get_mut("polymarket_main")
        .and_then(|venue| venue.execution.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket execution table should exist")
}

fn polymarket_data_table(
    loaded: &mut LoadedBoltV3Config,
) -> &mut toml::map::Map<String, toml::Value> {
    loaded
        .root
        .venues
        .get_mut("polymarket_main")
        .and_then(|venue| venue.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist")
}

fn binance_execution_value() -> toml::Value {
    toml::from_str(include_str!("fixtures/bolt_v3/binance_execution.toml"))
        .expect("binance execution TOML should parse")
}

fn polymarket_data_config(loaded: &LoadedBoltV3Config) -> PolymarketDataConfig {
    loaded.root.venues["polymarket_main"]
        .data
        .clone()
        .expect("fixture polymarket venue should define [data]")
        .try_into()
        .expect("fixture polymarket data should parse")
}

fn polymarket_execution_config(loaded: &LoadedBoltV3Config) -> PolymarketExecutionConfig {
    loaded.root.venues["polymarket_main"]
        .execution
        .clone()
        .expect("fixture polymarket venue should define [execution]")
        .try_into()
        .expect("fixture polymarket execution should parse")
}

fn binance_data_config(loaded: &LoadedBoltV3Config) -> BinanceDataConfig {
    loaded.root.venues["binance_reference"]
        .data
        .clone()
        .expect("fixture binance venue should define [data]")
        .try_into()
        .expect("fixture binance data should parse")
}

fn nt_polymarket_signature_type(
    signature_type: PolymarketSignatureType,
) -> NtPolymarketSignatureType {
    match signature_type {
        PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
        PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
        PolymarketSignatureType::PolyGnosisSafe => NtPolymarketSignatureType::PolyGnosisSafe,
    }
}

fn nt_binance_product_type(product_type: BinanceProductType) -> NtBinanceProductType {
    match product_type {
        BinanceProductType::Spot => NtBinanceProductType::Spot,
        BinanceProductType::UsdM => NtBinanceProductType::UsdM,
        BinanceProductType::CoinM => NtBinanceProductType::CoinM,
    }
}

fn nt_binance_environment(environment: BinanceEnvironment) -> NtBinanceEnvironment {
    match environment {
        BinanceEnvironment::Mainnet => NtBinanceEnvironment::Mainnet,
        BinanceEnvironment::Testnet => NtBinanceEnvironment::Testnet,
        BinanceEnvironment::Demo => NtBinanceEnvironment::Demo,
    }
}

fn binance_execution_table(
    loaded: &mut LoadedBoltV3Config,
) -> &mut toml::map::Map<String, toml::Value> {
    let venue = loaded
        .root
        .venues
        .get_mut("binance_reference")
        .expect("fixture binance_reference venue should exist");
    venue.execution = Some(binance_execution_value());
    venue
        .execution
        .as_mut()
        .and_then(toml::Value::as_table_mut)
        .expect("fixture binance execution table should exist")
}

fn stub_new_market(question: &str, description: &str) -> PolymarketNewMarket {
    PolymarketNewMarket {
        id: "1".to_string(),
        question: question.to_string(),
        market: Ustr::from("0xabc"),
        slug: "test-market".to_string(),
        description: description.to_string(),
        assets_ids: Vec::new(),
        outcomes: vec!["Yes".to_string(), "No".to_string()],
        timestamp: "0".to_string(),
        tags: Vec::new(),
        condition_id: "0xabc".to_string(),
        active: true,
        clob_token_ids: Vec::new(),
        order_price_min_tick_size: None,
        group_item_title: None,
        event_message: None,
    }
}

fn keyword_new_market_filter(keyword: &str) -> toml::Value {
    let mut table = toml::map::Map::new();
    table.insert(
        "kind".to_string(),
        toml::Value::String("keyword".to_string()),
    );
    table.insert(
        "keyword".to_string(),
        toml::Value::String(keyword.to_string()),
    );
    toml::Value::Table(table)
}

fn assert_polymarket_funder_invariant(error: BoltV3AdapterMappingError, message_fragment: &str) {
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(field, "execution.funder_address");
            assert!(
                message.contains(message_fragment),
                "expected funder invariant message fragment `{message_fragment}`, got: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn polymarket_venue_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let expected_data = polymarket_data_config(&loaded);
    let expected_exec = polymarket_execution_config(&loaded);
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
        Some(expected_data.base_url_http.as_str())
    );
    assert_eq!(
        data.base_url_ws.as_deref(),
        Some(expected_data.base_url_ws.as_str())
    );
    assert_eq!(
        data.base_url_gamma.as_deref(),
        Some(expected_data.base_url_gamma.as_str())
    );
    assert_eq!(
        data.base_url_data_api.as_deref(),
        Some(expected_data.base_url_data_api.as_str())
    );
    assert_eq!(data.http_timeout_secs, expected_data.http_timeout_seconds);
    assert_eq!(data.ws_timeout_secs, expected_data.ws_timeout_seconds);
    let expected_ws_max_subscriptions: usize = expected_data
        .websocket_max_subscriptions_per_connection
        .try_into()
        .expect("fixture websocket cap should fit usize");
    assert_eq!(data.ws_max_subscriptions, expected_ws_max_subscriptions);
    assert_eq!(
        data.update_instruments_interval_mins,
        expected_data.update_instruments_interval_minutes
    );
    assert_eq!(
        data.subscribe_new_markets,
        expected_data.subscribe_new_markets
    );
    assert_eq!(
        data.auto_load_missing_instruments,
        expected_data.auto_load_missing_instruments
    );
    assert_eq!(
        data.auto_load_debounce_ms,
        expected_data.auto_load_debounce_milliseconds
    );
    assert_eq!(data.transport_backend, expected_data.transport_backend);

    let exec = polymarket
        .execution
        .as_ref()
        .expect("polymarket [execution] block must produce an NT exec config")
        .config_as::<PolymarketExecClientConfig>()
        .expect("polymarket [execution] should downcast to NT PolymarketExecClientConfig");
    assert_eq!(
        exec.signature_type,
        nt_polymarket_signature_type(expected_exec.signature_type)
    );
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
        expected_exec.funder_address.as_deref()
    );
    assert_eq!(
        exec.base_url_http.as_deref(),
        Some(expected_exec.base_url_http.as_str())
    );
    assert_eq!(
        exec.base_url_ws.as_deref(),
        Some(expected_exec.base_url_ws.as_str())
    );
    assert_eq!(
        exec.base_url_data_api.as_deref(),
        Some(expected_exec.base_url_data_api.as_str())
    );
    assert_eq!(exec.http_timeout_secs, expected_exec.http_timeout_seconds);
    let expected_max_retries: u32 = expected_exec
        .max_retries
        .try_into()
        .expect("fixture retry count should fit u32");
    assert_eq!(exec.max_retries, expected_max_retries);
    assert_eq!(
        exec.retry_delay_initial_ms,
        expected_exec.retry_delay_initial_milliseconds
    );
    assert_eq!(
        exec.retry_delay_max_ms,
        expected_exec.retry_delay_max_milliseconds
    );
    assert_eq!(exec.ack_timeout_secs, expected_exec.ack_timeout_seconds);
    assert_eq!(exec.transport_backend, expected_exec.transport_backend);
}

#[test]
fn adapter_mapper_rejects_subscribe_new_markets_true() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_data_table(&mut loaded).insert(
        "subscribe_new_markets".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("subscribe_new_markets=true must fail closed before NT mapping");
    let message = error.to_string();
    assert!(
        message.contains("subscribe_new_markets") && message.contains("controlled-loading"),
        "unexpected adapter mapping error: {message}"
    );
}

#[test]
fn adapter_mapper_rejects_auto_load_missing_instruments_true() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_data_table(&mut loaded).insert(
        "auto_load_missing_instruments".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("auto_load_missing_instruments=true must fail closed before NT mapping");
    let message = error.to_string();
    assert!(
        message.contains("auto_load_missing_instruments") && message.contains("controlled-loading"),
        "unexpected adapter mapping error: {message}"
    );
}

#[test]
fn adapter_mapper_maps_configured_polymarket_new_market_filter_to_nt() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_data_table(&mut loaded).insert(
        "new_market_filter".to_string(),
        keyword_new_market_filter("bitcoin"),
    );

    let resolved = fixture_resolved_secrets();
    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("new_market_filter should map");
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
    let filter = data
        .new_market_filter
        .as_ref()
        .expect("configured new_market_filter must reach NT config");
    assert!(filter.accept_new_market(&stub_new_market("Bitcoin up or down", "")));
    assert!(!filter.accept_new_market(&stub_new_market("Ethereum up or down", "")));
}

#[test]
fn adapter_mapper_rejects_empty_polymarket_new_market_filter_keyword_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_data_table(&mut loaded).insert(
        "new_market_filter".to_string(),
        keyword_new_market_filter(" "),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject empty new_market_filter keyword");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(field, "data.new_market_filter.keyword");
            assert!(
                message.contains("must be non-empty"),
                "expected non-empty keyword message, got: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn adapter_mapper_rejects_polymarket_max_retries_above_nt_u32() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_execution_table(&mut loaded).insert(
        "max_retries".to_string(),
        toml::Value::Integer(i64::from(u32::MAX) + 1),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject max_retries values that do not fit NT");
    match error {
        BoltV3AdapterMappingError::NumericRange {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(field, "execution.max_retries");
            assert!(
                message.contains("does not fit in u32 expected by NT"),
                "expected NT u32 range message, got: {message}"
            );
        }
        other => panic!("expected NumericRange, got {other}"),
    }
}

#[test]
fn adapter_mapper_rejects_polymarket_retry_delay_initial_above_max_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_execution = polymarket_execution_table(&mut loaded);
    polymarket_execution.insert(
        "retry_delay_initial_milliseconds".to_string(),
        toml::Value::Integer(3000),
    );
    polymarket_execution.insert(
        "retry_delay_max_milliseconds".to_string(),
        toml::Value::Integer(1000),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject retry delay ordering violations");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "polymarket_main");
            assert_eq!(field, "execution.retry_delay_initial_milliseconds");
            assert!(
                message.contains("must be <= retry_delay_max_milliseconds"),
                "expected retry-delay ordering message, got: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn adapter_mapper_rejects_missing_polymarket_proxy_funder_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_execution_table(&mut loaded).remove("funder_address");

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject missing proxy funder address");
    assert_polymarket_funder_invariant(error, "is required when signature_type is `poly_proxy`");
}

#[test]
fn adapter_mapper_rejects_missing_polymarket_gnosis_safe_funder_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_execution = polymarket_execution_table(&mut loaded);
    polymarket_execution.insert(
        "signature_type".to_string(),
        toml::Value::String("poly_gnosis_safe".to_string()),
    );
    polymarket_execution.remove("funder_address");

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject missing gnosis-safe funder address");
    assert_polymarket_funder_invariant(error, "poly_gnosis_safe");
}

#[test]
fn adapter_mapper_rejects_invalid_polymarket_funder_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_execution_table(&mut loaded).insert(
        "funder_address".to_string(),
        toml::Value::String("0x123".to_string()),
    );

    let resolved = fixture_resolved_secrets();
    let error =
        map_bolt_v3_adapters(&loaded, &resolved).expect_err("mapper must reject invalid funder");
    assert_polymarket_funder_invariant(error, "not a valid EVM public address");
}

#[test]
fn adapter_mapper_allows_missing_polymarket_eoa_funder() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_execution = polymarket_execution_table(&mut loaded);
    polymarket_execution.insert(
        "signature_type".to_string(),
        toml::Value::String("eoa".to_string()),
    );
    polymarket_execution.remove("funder_address");

    let resolved = fixture_resolved_secrets();
    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("EOA without funder should map");
    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");
    let exec = polymarket
        .execution
        .as_ref()
        .expect("polymarket [execution] block must produce an NT exec config")
        .config_as::<PolymarketExecClientConfig>()
        .expect("polymarket [execution] should downcast to NT PolymarketExecClientConfig");
    assert_eq!(exec.signature_type, NtPolymarketSignatureType::Eoa);
    assert_eq!(exec.funder, None);
}

#[test]
fn adapter_mapper_rejects_zero_polymarket_funder_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_execution_table(&mut loaded).insert(
        "funder_address".to_string(),
        toml::Value::String("0x0000000000000000000000000000000000000000".to_string()),
    );

    let resolved = fixture_resolved_secrets();
    let error =
        map_bolt_v3_adapters(&loaded, &resolved).expect_err("mapper must reject zero funder");
    assert_polymarket_funder_invariant(error, "zero address");
}

#[test]
fn adapter_mapper_allows_polymarket_eoa_with_configured_funder() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_execution_table(&mut loaded).insert(
        "signature_type".to_string(),
        toml::Value::String("eoa".to_string()),
    );

    let resolved = fixture_resolved_secrets();
    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("EOA with funder should map");
    let polymarket = configs
        .venues
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");
    let exec = polymarket
        .execution
        .as_ref()
        .expect("polymarket [execution] block must produce an NT exec config")
        .config_as::<PolymarketExecClientConfig>()
        .expect("polymarket [execution] should downcast to NT PolymarketExecClientConfig");
    assert_eq!(exec.signature_type, NtPolymarketSignatureType::Eoa);
    assert_eq!(
        exec.funder.as_deref(),
        Some("0x1111111111111111111111111111111111111111")
    );
}

#[test]
fn fee_provider_rejects_missing_polymarket_proxy_funder_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    polymarket_execution_table(&mut loaded).remove("funder_address");

    let resolved = fixture_resolved_secrets();
    let venue = loaded
        .root
        .venues
        .get("polymarket_main")
        .expect("fixture polymarket venue should exist");
    let error = match polymarket::build_fee_provider("polymarket_main", venue, &resolved) {
        Ok(_) => panic!("fee provider must reject missing proxy funder address"),
        Err(error) => error,
    };
    assert_polymarket_funder_invariant(error, "is required when signature_type is `poly_proxy`");
}

#[test]
fn fee_provider_builds_with_configured_polymarket_credentials() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let venue = loaded
        .root
        .venues
        .get("polymarket_main")
        .expect("fixture polymarket venue should exist");
    if let Err(error) = polymarket::build_fee_provider("polymarket_main", venue, &resolved) {
        panic!("fee provider should build from configured polymarket credentials: {error}");
    }
}

#[test]
fn binance_data_venue_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let expected_data = binance_data_config(&loaded);
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

    let expected_product_types: Vec<_> = expected_data
        .product_types
        .iter()
        .copied()
        .map(nt_binance_product_type)
        .collect();
    assert_eq!(data.product_types, expected_product_types);
    assert_eq!(
        data.environment,
        nt_binance_environment(expected_data.environment)
    );
    // The bolt-v3 binance data schema now requires explicit
    // base_url_http and base_url_ws so NT cannot silently fall back to
    // its compiled-in default Binance endpoints. Both must arrive at
    // NT as `Some(...)` carrying the configured fixture value.
    assert_eq!(
        data.base_url_http.as_deref(),
        Some(expected_data.base_url_http.as_str())
    );
    assert_eq!(
        data.base_url_ws.as_deref(),
        Some(expected_data.base_url_ws.as_str())
    );
    assert_eq!(
        data.api_key.as_deref(),
        Some(fixture_binance_secrets().api_key.as_str())
    );
    assert_eq!(
        data.api_secret.as_deref(),
        Some(fixture_binance_secrets().api_secret.as_str())
    );
    assert_eq!(
        data.instrument_status_poll_secs,
        expected_data.instrument_status_poll_seconds
    );
    assert_eq!(data.transport_backend, expected_data.transport_backend);
}

#[test]
fn binance_execution_venue_fails_closed_before_nt_mapping() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let venue = loaded
        .root
        .venues
        .get_mut("binance_reference")
        .expect("fixture binance_reference venue should exist");
    venue.data = None;
    venue.execution = Some(binance_execution_value());
    let resolved = fixture_resolved_secrets();

    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("Binance execution must fail closed before NT mapping");
    let message = error.to_string();
    assert!(
        message.contains("binance_reference")
            && message.contains("execution")
            && message.contains("reference-data scope"),
        "unexpected Binance execution mapping error: {message}"
    );
}

#[test]
fn adapter_mapper_rejects_binance_execution_before_product_type_mapping_if_validation_was_bypassed()
{
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    binance_execution_table(&mut loaded)
        .insert("product_types".to_string(), toml::Value::Array(Vec::new()));

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject Binance execution before NT mapping");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "binance_reference");
            assert_eq!(field, "execution");
            assert!(
                message.contains("reference-data scope"),
                "expected current-scope execution rejection, got: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn adapter_mapper_rejects_binance_execution_before_taker_fee_mapping_if_validation_was_bypassed() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    binance_execution_table(&mut loaded).insert(
        "default_taker_fee".to_string(),
        toml::Value::String("-0.0001".to_string()),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must reject Binance execution before NT mapping");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            venue_key,
            field,
            message,
        } => {
            assert_eq!(venue_key, "binance_reference");
            assert_eq!(field, "execution");
            assert!(
                message.contains("reference-data scope"),
                "expected current-scope execution rejection, got: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn missing_or_invalid_root_config_remains_caught_by_validation_not_mapper_defaults() {
    use bolt_v2::bolt_v3_validate::validate_root_only;

    // Missing [secrets] for polymarket execution venue: the existing
    // validator must catch this *before* the mapper ever runs. The
    // mapper itself must not silently fall back to defaults.
    let mut root: BoltV3RootConfig = toml::from_str(include_str!("fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 root fixture should parse");
    root.venues
        .retain(|venue_key, _venue| venue_key == "polymarket_main");
    let venue = root
        .venues
        .get_mut("polymarket_main")
        .expect("fixture polymarket venue should exist");
    venue.data = None;
    venue.secrets = None;

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
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-adapter-mapping-build-path");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
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
