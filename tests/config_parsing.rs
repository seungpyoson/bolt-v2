use crate::support;

use std::fs;

use bolt_v2::strategies::binary_oracle_edge_taker::archetype::{
    BINARY_ORACLE_ENTRY_ORDER_REDUCE_ONLY_CODE, BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE,
};

const OLD_CHAINLINK_FIXTURE_FEED_ID: &str =
    "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472";
const CHAINLINK_BTC_TESTNET_FEED_ID: &str =
    "0x00037da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439";
const CHAINLINK_TEST_FEED_ID_PRIMARY: &str =
    "0x1111111111111111111111111111111111111111111111111111111111111111";
const CHAINLINK_TEST_FEED_ID_SECONDARY: &str =
    "0x2222222222222222222222222222222222222222222222222222222222222222";
const ZERO_CHAINLINK_FEED_ID: &str =
    "0x0000000000000000000000000000000000000000000000000000000000000000";

/// Shipped per-asset binary-oracle strategy files. The tracked production root
/// may enable only a subset, but every shipped strategy must keep validating.
const SHIPPED_BINARY_ORACLE_STRATEGY_FILES: &[&str] = &[
    "config/strategies/binary_oracle_btc.toml",
    "config/strategies/binary_oracle_eth.toml",
    "config/strategies/binary_oracle_sol.toml",
    "config/strategies/binary_oracle_bnb.toml",
    "config/strategies/binary_oracle_xrp.toml",
    "config/strategies/binary_oracle_doge.toml",
];
const TRACKED_PRODUCTION_BINARY_ORACLE_STRATEGY_FILES: &[&str] =
    &["config/strategies/binary_oracle_btc.toml"];
const PLACEHOLDER_POLYMARKET_FUNDER: &str = "0x1111111111111111111111111111111111111111";
const SYNTHETIC_FIXTURE_POLYMARKET_FUNDER: &str = "0xf1c7000000000000000000000000000000000001";

fn polymarket_funder(root: &toml::Value) -> Option<&str> {
    root.get("clients")
        .and_then(|value| value.get("polymarket_main"))
        .and_then(|value| value.get("execution"))
        .and_then(|value| value.get("funder"))
        .and_then(toml::Value::as_str)
}

fn assert_binary_oracle_entry_order_shape_rejected(messages: &[String], case_name: &str) {
    assert!(
        messages.iter().any(|message| validation_message_has_code(
            message,
            BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE
        )),
        "{case_name} should reject unsupported executable entry shape, got: {messages:#?}"
    );
}

fn validation_message_has_code(message: &str, code: &str) -> bool {
    message
        .split_ascii_whitespace()
        .any(|token| token.strip_prefix("error_code=") == Some(code))
}

#[test]
fn shipped_polymarket_secrets_use_eu_west_2_registry_paths() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;

    for relative_path in ["config/root.toml", "tests/fixtures/bolt_v3/root.toml"] {
        let loaded = load_bolt_v3_config(&support::repo_path(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should load: {error}"));

        assert_eq!(
            loaded.root.aws.region, "eu-west-2",
            "{relative_path} must resolve shipped SSM paths in eu-west-2"
        );

        let polymarket = loaded
            .root
            .clients
            .get("polymarket_main")
            .unwrap_or_else(|| panic!("{relative_path} must declare clients.polymarket_main"));
        let secrets = polymarket
            .secrets
            .as_ref()
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| {
                panic!("{relative_path} clients.polymarket_main.secrets must be a table")
            });
        for (field, expected_path) in [
            ("private_key_ssm_path", "/bolt/polymarket/private-key"),
            ("api_key_ssm_path", "/bolt/polymarket/api-key"),
            ("api_secret_ssm_path", "/bolt/polymarket/api-secret"),
            ("passphrase_ssm_path", "/bolt/polymarket/api-passphrase"),
        ] {
            assert_eq!(
                secrets.get(field).and_then(toml::Value::as_str),
                Some(expected_path),
                "{relative_path} clients.polymarket_main.secrets.{field} must use the eu-west-2 registry path"
            );
        }
    }
}

#[test]
fn bolt_v3_config_uses_nautilus_vocabulary_field_names() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::ClientId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let strategy = &loaded.strategies[0].config;
    assert_eq!(
        strategy.execution_client_id,
        nautilus_model::identifiers::ClientId::from("polymarket_main")
    );
    assert!(!strategy.signal_data.is_empty());

    let data_engine = &loaded.root.nautilus.data_engine;
    let exec_engine = &loaded.root.nautilus.exec_engine;
    assert!(data_engine.external_clients.is_empty());
    assert!(exec_engine.external_clients.is_empty());

    let _typed_check: &Vec<ClientId> = &data_engine.external_clients;
}

#[test]
fn bolt_v3_config_uses_clients_section_with_nt_venue_identifier() {
    // FINDING-3: TOML `[clients.<id>]` holds an NT Venue identifier in
    // `venue`. The NT Venue type wraps a Ustr and the serde macro enforces
    // correctness at parse time, so the `venue = "POLYMARKET"` value is
    // checked structurally as well as semantically.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::Venue;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let clients = &loaded.root.clients;
    assert!(clients.contains_key("polymarket_main"));

    let polymarket = &clients["polymarket_main"];
    assert_eq!(polymarket.venue, Venue::from("POLYMARKET"));
    assert!(polymarket.execution.is_some());
}

#[test]
fn root_validation_rejects_incoherent_polymarket_fee_rounding_before_runtime_binding() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "fee_rounding_mode = \"midpoint_nearest_even\"",
        "fee_rounding_mode = \"midpoint_away_from_zero\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("the provider formula remains syntactically valid TOML");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.execution.economics")
                && message.contains("fee_rounding_mode must be midpoint_nearest_even")
        }),
        "provider economics must fail during root validation, got: {messages:#?}"
    );
}

#[test]
fn config_load_rejects_unknown_valuation_origin_kind() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;

    let temp = tempfile::tempdir().expect("config-load tempdir should create");
    let strategies_dir = temp.path().join("strategies");
    fs::create_dir(&strategies_dir).expect("strategy fixture dir should create");
    fs::copy(
        support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        strategies_dir.join("binary_oracle.toml"),
    )
    .expect("strategy fixture should copy");
    let root_text = support::repo_text("tests/fixtures/bolt_v3/root.toml").replacen(
        "from_kind = \"currency\"",
        "from_kind = \"unsupported_native_kind\"",
        1,
    );
    let root_path = temp.path().join("root.toml");
    fs::write(&root_path, root_text).expect("mutated root fixture should write");

    let error = load_bolt_v3_config(&root_path)
        .expect_err("an unknown valuation origin kind must fail config load");
    assert!(
        error.to_string().contains("unsupported_native_kind"),
        "load failure must identify the unknown native-unit kind: {error}"
    );
}

#[test]
fn config_load_rejects_kill_switch_flatten_while_economics_is_quote_only() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;

    let temp = tempfile::tempdir().expect("config-load tempdir should create");
    let strategies_dir = temp.path().join("strategies");
    fs::create_dir(&strategies_dir).expect("strategy fixture dir should create");
    fs::copy(
        support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        strategies_dir.join("binary_oracle.toml"),
    )
    .expect("strategy fixture should copy");
    let root_text = support::repo_text("tests/fixtures/bolt_v3/root.toml")
        .replacen("enabled = false", "enabled = true", 1)
        .replacen(
            "flatten_open_positions_on_breach = false",
            "flatten_open_positions_on_breach = true",
            1,
        )
        .replacen(
            "account_ids = [\"POLYMARKET-001\"]\ninstrument_ids = []",
            "account_ids = [\"POLYMARKET-001\"]\ninstrument_ids = [\"condition-fixture-yes.POLYMARKET\"]",
            1,
        )
        .replacen(
            "[risk.loss_governor]",
            r#"[risk.kill_switch.flatten]
enabled = true
route_kind = "live_node_command_router"
max_live_order_count = 2
max_notional_per_order = "10.00"
order_type = "market"
time_in_force = "ioc"
is_post_only = false
is_reduce_only = true
is_quote_quantity = false

[risk.loss_governor]"#,
            1,
        );
    let root_path = temp.path().join("root.toml");
    fs::write(&root_path, root_text).expect("mutated root fixture should write");

    let error = load_bolt_v3_config(&root_path)
        .expect_err("quote-only economics must reject active forced-reduction routing");
    assert!(
        error
            .to_string()
            .contains("cannot route forced reductions while economics_slice=quote_only"),
        "load failure must identify the incompatible economics authority: {error}"
    );
}

#[test]
fn bolt_v3_root_trader_id_uses_nt_typed_identifier() {
    // `BoltV3RootConfig.trader_id` is typed as `nautilus_model::identifiers::TraderId`
    // so the NT identifier macro rejects empty strings at parse time instead of
    // leaving that as a bolt-side runtime check.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::TraderId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let trader_id: TraderId = loaded.root.trader_id;
    assert_eq!(trader_id, TraderId::from("BOLT-001"));
}

#[test]
fn bolt_v3_root_trader_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let mutated = replace_in_fixture_root("trader_id = \"BOLT-001\"", "trader_id = \"\"");
    let err = toml::from_str::<BoltV3RootConfig>(&mutated)
        .expect_err("empty trader_id should be rejected by NT TraderId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty trader_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_polymarket_account_id_uses_nt_typed_identifier() {
    // `PolymarketExecutionConfig.account_id` is typed as
    // `nautilus_model::identifiers::AccountId` so NT's identifier macro
    // rejects empty / invalid strings at parse time and the bolt
    // execution-config binding holds the same typed value the NT
    // PolymarketExecClientConfig expects, eliminating the
    // `AccountId::from(_.as_str())` round-trip.
    use bolt_v2::bolt_v3_providers::polymarket::PolymarketExecutionConfig;
    use nautilus_model::identifiers::AccountId;

    let exec_toml = r#"
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
transport_backend = "sockudo"
"#;
    let parsed: PolymarketExecutionConfig =
        toml::from_str(exec_toml).expect("polymarket execution block should parse");
    let account_id: AccountId = parsed.account_id;
    assert_eq!(account_id, AccountId::from("POLYMARKET-001"));
}

#[test]
fn bolt_v3_polymarket_account_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_providers::polymarket::PolymarketExecutionConfig;

    let exec_toml = r#"
account_id = ""
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
transport_backend = "sockudo"
"#;
    let err = toml::from_str::<PolymarketExecutionConfig>(exec_toml)
        .expect_err("empty account_id should be rejected by NT AccountId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty account_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_polymarket_and_nautilus_config_rejects_nt_field_aliases() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig,
        bolt_v3_providers::polymarket::{
            PolymarketDataConfig, PolymarketExecutionConfig, PolymarketSignatureType,
        },
    };

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let nt_named = fixture;

    let parsed: BoltV3RootConfig =
        toml::from_str(&nt_named).expect("NT-owned field names should parse");
    let polymarket = parsed
        .clients
        .get("polymarket_main")
        .expect("polymarket fixture client should exist");
    let data: PolymarketDataConfig = polymarket
        .data
        .clone()
        .expect("polymarket data block should exist")
        .try_into()
        .expect("polymarket data block should parse with NT names");
    assert_eq!(data.update_instruments_interval_mins, 1);
    assert_eq!(data.ws_max_subscriptions, 200);
    assert_eq!(data.base_url_rtds, "wss://ws-live-data.polymarket.com");
    assert_eq!(data.new_market_fetch_max_concurrency, 8);
    assert!(!data.resolve_poll_enabled);
    assert_eq!(data.resolve_poll_interval_secs, 30);
    assert_eq!(data.resolve_poll_grace_secs, 10);
    assert_eq!(data.resolve_poll_max_wait_secs, 1800);
    assert!(!data.auto_load_missing_instruments);
    assert_eq!(data.auto_load_debounce_ms, 250);
    assert_eq!(data.auto_load_max_retries, 12);
    assert_eq!(data.auto_load_retry_delay_initial_secs, 5);
    assert_eq!(data.auto_load_retry_delay_max_secs, 15);
    let execution: PolymarketExecutionConfig = polymarket
        .execution
        .clone()
        .expect("polymarket execution block should exist")
        .try_into()
        .expect("polymarket execution block should parse with NT names");
    assert_eq!(
        execution.signature_type,
        PolymarketSignatureType::PolyGnosisSafe
    );
    let funder = execution
        .funder
        .as_deref()
        .expect("fixture Polymarket execution should declare a funder");
    assert_eq!(
        funder.len(),
        42,
        "fixture Polymarket funder should have EVM address length"
    );
    assert!(
        funder != PLACEHOLDER_POLYMARKET_FUNDER,
        "fixture Polymarket funder must not use the placeholder value"
    );
    assert_eq!(parsed.nautilus.timeout_shutdown_secs, 10);

    let old_update = ["update_instruments", "_interval", "_minutes"].concat();
    let old_ws = ["websocket", "_max_subscriptions", "_per_connection"].concat();
    let old_funder = ["funder", "_address = "].concat();
    let old_shutdown = ["timeout", "_shutdown = "].concat();
    let aliases = nt_named
        .replace("update_instruments_interval_mins", &old_update)
        .replace("ws_max_subscriptions", &old_ws)
        .replace("funder = ", &old_funder)
        .replace("timeout_shutdown_secs = ", &old_shutdown);
    let error = toml::from_str::<BoltV3RootConfig>(&aliases)
        .expect_err("NT-owned alias field names should fail parse");
    let rendered = error.to_string();
    assert!(
        rendered.contains("unknown field"),
        "alias rejection should come from deny_unknown_fields, got: {rendered}"
    );
}

#[test]
fn retired_gate_config_block_is_rejected() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let source = fs::read_to_string(support::repo_path("config/root.toml"))
        .expect("root config should be readable");
    let stale_section = [
        "[live",
        "_canary.operator",
        "_evidence]\n",
        "enabled = true\n",
    ]
    .concat();
    let source = format!("{source}\n{stale_section}");

    let error = toml::from_str::<BoltV3RootConfig>(&source)
        .expect_err("retired gate config block should be rejected");
    let rendered = error.to_string();
    assert!(rendered.contains("unknown field"));
}

#[test]
fn shipped_chainlink_reference_config_uses_control_ping_heartbeat() {
    use bolt_v2::bolt_v3_providers::chainlink_reference::ChainlinkReferencePriceDataConfig;

    for relative_path in ["config/root.toml", "tests/fixtures/bolt_v3/root.toml"] {
        let source = fs::read_to_string(support::repo_path(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should be readable: {error}"));
        let parsed = toml::from_str::<toml::Value>(&source)
            .unwrap_or_else(|error| panic!("{relative_path} should parse: {error}"));
        let data_value = parsed
            .get("clients")
            .and_then(|value| value.get("chainlink_reference"))
            .and_then(|value| value.get("data"))
            .cloned()
            .unwrap_or_else(|| {
                panic!("{relative_path} should declare clients.chainlink_reference.data")
            });
        let data = data_value
            .as_table()
            .expect("chainlink reference data should be a table");
        let data_config: ChainlinkReferencePriceDataConfig =
            data_value.clone().try_into().unwrap_or_else(|error| {
                panic!("{relative_path} chainlink reference data should deserialize: {error}")
            });

        assert_eq!(
            data.get("heartbeat_secs").and_then(toml::Value::as_integer),
            Some(5),
            "{relative_path} should keep a configured heartbeat interval"
        );
        assert!(
            !data.contains_key("heartbeat_message"),
            "{relative_path} Chainlink reference WS must omit heartbeat_message so NT sends protocol Ping frames instead of text"
        );
        assert_eq!(
            data_config.heartbeat_message, None,
            "{relative_path} parsed Chainlink reference config must use protocol Ping frames"
        );
    }
}

#[test]
fn shipped_polyresearch_reference_config_uses_verified_gateway_endpoint() {
    const VERIFIED_ENDPOINT: &str = "wss://3j5lx6otd8.execute-api.eu-west-1.amazonaws.com/prod";
    const RETIRED_ENDPOINT: &str = "wss://ws.polynode.dev/ws";

    for relative_path in ["config/root.toml", "tests/fixtures/bolt_v3/root.toml"] {
        let source = fs::read_to_string(support::repo_path(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should be readable: {error}"));
        let parsed = toml::from_str::<toml::Value>(&source)
            .unwrap_or_else(|error| panic!("{relative_path} should parse: {error}"));
        let endpoint = parsed
            .get("clients")
            .and_then(|value| value.get("polyresearch_reference"))
            .and_then(|value| value.get("data"))
            .and_then(|value| value.get("websocket_endpoint"))
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "{relative_path} should declare clients.polyresearch_reference.data.websocket_endpoint"
                )
            });

        assert_eq!(
            endpoint, VERIFIED_ENDPOINT,
            "{relative_path} PolyResearch endpoint must match the verified apiKey gateway"
        );
        assert_ne!(
            endpoint, RETIRED_ENDPOINT,
            "{relative_path} must not point PolyResearch at the retired endpoint that returns 401"
        );
    }
}

#[test]
fn tracked_root_is_btc_only_live_profile() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;

    let loaded = load_bolt_v3_config(&support::repo_path("config/root.toml"))
        .expect("tracked root should load");

    assert_eq!(
        loaded.root.strategy_files,
        vec!["strategies/binary_oracle_btc.toml".to_string()],
        "tracked production root must enable only the BTC live strategy"
    );
    let surfaces = loaded
        .root
        .realized_volatility_surfaces
        .as_ref()
        .expect("tracked root must declare realized volatility surfaces");
    assert_eq!(
        surfaces.keys().cloned().collect::<Vec<_>>(),
        vec!["btc_usdt_midpoint_rv".to_string()],
        "tracked production root must carry only the BTC RV surface"
    );
    let reference_current_price = loaded.strategies[0]
        .config
        .reference_current_price
        .as_ref()
        .expect("tracked BTC strategy must declare reference_current_price");
    assert_eq!(
        reference_current_price.source_order,
        vec!["chainlink_primary".to_string()],
        "tracked BTC production reference source order must be Chainlink-only until PolyResearch is re-enabled deliberately"
    );
    assert!(
        !reference_current_price
            .sources
            .contains_key("polyresearch_backup"),
        "tracked BTC production strategy must not configure PolyResearch backup while its live subscribe ack path is unresolved"
    );
}

#[test]
fn shipped_roots_use_polymarket_safe_profile_without_placeholder_collateral() {
    for relative_path in ["config/root.toml", "tests/fixtures/bolt_v3/root.toml"] {
        let source = fs::read_to_string(support::repo_path(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should be readable: {error}"));
        let parsed = toml::from_str::<toml::Value>(&source)
            .unwrap_or_else(|error| panic!("{relative_path} should parse: {error}"));
        let execution = parsed
            .get("clients")
            .and_then(|value| value.get("polymarket_main"))
            .and_then(|value| value.get("execution"))
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| {
                panic!("{relative_path} should declare clients.polymarket_main.execution")
            });
        let funder = execution
            .get("funder")
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("{relative_path} Polymarket execution must declare funder"));

        assert_eq!(
            execution
                .get("signature_type")
                .and_then(toml::Value::as_str),
            Some("poly_gnosis_safe"),
            "{relative_path} must use the live Polymarket safe signature type"
        );
        assert_eq!(
            funder.len(),
            42,
            "{relative_path} Polymarket funder must be an EVM address"
        );
        assert_ne!(
            funder, PLACEHOLDER_POLYMARKET_FUNDER,
            "{relative_path} must not ship the placeholder Polymarket funder"
        );
        assert!(
            !execution.contains_key("on_chain_collateral"),
            "{relative_path} must not ship placeholder on-chain collateral config"
        );
    }
}

#[test]
fn bolt_v3_fixture_uses_synthetic_polymarket_funder() {
    let live_source = fs::read_to_string(support::repo_path("config/root.toml"))
        .expect("tracked production root should be readable");
    let fixture_source = fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("Bolt v3 fixture root should be readable");
    let live_root =
        toml::from_str::<toml::Value>(&live_source).expect("tracked production root should parse");
    let fixture_root =
        toml::from_str::<toml::Value>(&fixture_source).expect("Bolt v3 fixture root should parse");

    let live_funder = polymarket_funder(&live_root).expect("tracked root should declare funder");
    let fixture_funder =
        polymarket_funder(&fixture_root).expect("fixture root should declare funder");

    assert!(
        fixture_funder == SYNTHETIC_FIXTURE_POLYMARKET_FUNDER,
        "Bolt v3 fixture should use the deliberate synthetic Polymarket funder"
    );
    assert!(
        live_funder != SYNTHETIC_FIXTURE_POLYMARKET_FUNDER,
        "tracked production root should not use the synthetic fixture Polymarket funder"
    );
    assert!(
        fixture_funder != live_funder,
        "Bolt v3 fixture funder should not duplicate the tracked production funder"
    );
}

#[test]
fn tracked_root_does_not_ship_legacy_reference_live_probe() {
    let source = fs::read_to_string(support::repo_path("config/root.toml"))
        .expect("tracked root should be readable");
    let root: bolt_v2::bolt_v3_config::BoltV3RootConfig =
        toml::from_str(&source).expect("tracked root config should parse");

    assert!(
        root.reference_live_probe.is_none(),
        "tracked production root must not ship the legacy reference_live_probe because it depends on PolyResearch"
    );
}

#[test]
fn fixture_reference_live_probe_config_points_to_reference_clients() {
    let relative_path = "tests/fixtures/bolt_v3/root.toml";
    let source = fs::read_to_string(support::repo_path(relative_path))
        .unwrap_or_else(|error| panic!("{relative_path} should be readable: {error}"));
    let root: bolt_v2::bolt_v3_config::BoltV3RootConfig = toml::from_str(&source)
        .unwrap_or_else(|error| panic!("{relative_path} root config should parse: {error}"));
    let probe = root
        .reference_live_probe
        .as_ref()
        .unwrap_or_else(|| panic!("{relative_path} must configure reference_live_probe"));

    assert_eq!(probe.chainlink_client_id, "chainlink_reference");
    assert_eq!(probe.polyresearch_client_id, "polyresearch_reference");
    assert!(
        probe.duration_secs > 0,
        "{relative_path} reference live probe duration must be positive"
    );
    assert!(
        probe.min_chainlink_data_frames > 0,
        "{relative_path} Chainlink probe data-frame floor must be positive"
    );
    assert!(
        root.clients
            .get(&probe.chainlink_client_id)
            .is_some_and(|client| client.venue.as_str() == "CHAINLINK_REFERENCE_PRICE"),
        "{relative_path} Chainlink probe client must resolve to the Chainlink reference provider"
    );
    assert!(
        root.clients
            .get(&probe.polyresearch_client_id)
            .is_some_and(|client| client.venue.as_str() == "POLYRESEARCH_REFERENCE_PRICE"),
        "{relative_path} PolyResearch probe client must resolve to the PolyResearch reference provider"
    );
}

#[test]
fn reference_live_probe_rejects_zero_duration() {
    let mut root = fixture_root_config();
    root.reference_live_probe
        .as_mut()
        .expect("fixture should configure reference_live_probe")
        .duration_secs = 0;

    let messages = bolt_v2::bolt_v3_validate::validate_root_only(&root);

    assert!(
        messages.iter().any(
            |message| message.contains("reference_live_probe.duration_secs")
                && message.contains("must be positive")
        ),
        "zero reference_live_probe.duration_secs should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_live_probe_rejects_zero_chainlink_frame_floor() {
    let mut root = fixture_root_config();
    root.reference_live_probe
        .as_mut()
        .expect("fixture should configure reference_live_probe")
        .min_chainlink_data_frames = 0;

    let messages = bolt_v2::bolt_v3_validate::validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_live_probe.min_chainlink_data_frames")
                && message.contains("must be positive")
        }),
        "zero reference_live_probe.min_chainlink_data_frames should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_live_probe_rejects_whitespace_client_id() {
    let mut root = fixture_root_config();
    root.reference_live_probe
        .as_mut()
        .expect("fixture should configure reference_live_probe")
        .chainlink_client_id = " chainlink_reference ".to_string();

    let messages = bolt_v2::bolt_v3_validate::validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_live_probe.chainlink_client_id")
                && message.contains("must be non-empty without surrounding whitespace")
        }),
        "whitespace reference_live_probe.chainlink_client_id should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_live_probe_rejects_unconfigured_client_id() {
    let mut root = fixture_root_config();
    root.reference_live_probe
        .as_mut()
        .expect("fixture should configure reference_live_probe")
        .chainlink_client_id = "missing_chainlink_reference".to_string();

    let messages = bolt_v2::bolt_v3_validate::validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_live_probe.chainlink_client_id")
                && message.contains("must reference a configured client")
        }),
        "unconfigured reference_live_probe.chainlink_client_id should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_live_probe_rejects_wrong_provider_client() {
    let mut root = fixture_root_config();
    root.reference_live_probe
        .as_mut()
        .expect("fixture should configure reference_live_probe")
        .chainlink_client_id = "polymarket_main".to_string();

    let messages = bolt_v2::bolt_v3_validate::validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_live_probe.chainlink_client_id")
                && message.contains("must reference provider `CHAINLINK_REFERENCE_PRICE`")
                && message.contains("got `POLYMARKET`")
        }),
        "wrong-provider reference_live_probe.chainlink_client_id should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_live_probe_rejects_client_missing_data_or_secrets() {
    let mut root = fixture_root_config();
    let client = root
        .clients
        .get_mut("chainlink_reference")
        .expect("fixture should configure chainlink_reference");
    client.data = None;
    client.secrets = None;

    let messages = bolt_v2::bolt_v3_validate::validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_live_probe.chainlink_client_id")
                && message.contains("must reference a client with [data]")
        }),
        "missing data block should fail reference_live_probe validation, got: {messages:#?}"
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("reference_live_probe.chainlink_client_id")
                && message.contains("must reference a client with [secrets]")
        }),
        "missing secrets block should fail reference_live_probe validation, got: {messages:#?}"
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("error_variant=NtReconnectBudgetMissingData")
                && message.contains("CHAINLINK_REFERENCE_PRICE")
        }),
        "missing applicable-provider data must fail the typed reconnect-budget path, got: {messages:#?}"
    );
}

#[test]
fn config_load_rejects_reference_reconnect_timeout_at_or_below_startup_bound() {
    let cases = [
        ("chainlink_reference", "CHAINLINK_REFERENCE_PRICE"),
        ("polyresearch_reference", "POLYRESEARCH_REFERENCE_PRICE"),
    ];
    let mut failures = Vec::new();

    for (client_key, provider_key) in cases {
        for delta_ms in [0, -1] {
            match reference_reconnect_timeout_load_error(client_key, delta_ms) {
                Ok(rendered) => {
                    if !(rendered
                        .contains("error_variant=ReferenceReconnectTimeoutNotAboveStartupBound")
                        && rendered
                            .contains(&format!("clients.{client_key}.data.reconnect_timeout_ms"))
                        && rendered.contains(provider_key)
                        && rendered.contains("must be greater than nautilus startup bound"))
                    {
                        failures.push(format!(
                            "{client_key} delta_ms={delta_ms} error did not expose the named startup-bound violation: {rendered}"
                        ));
                    }
                }
                Err(message) => failures.push(message),
            }
        }
    }

    assert!(
        failures.is_empty(),
        "reference provider reconnect startup-bound validation failures: {failures:#?}"
    );
}

#[test]
fn config_load_rejects_malformed_nt_reconnect_budget_provider_data() {
    let mut failures = Vec::new();

    for (client_key, provider_key) in [
        ("chainlink_reference", "CHAINLINK_REFERENCE_PRICE"),
        ("polyresearch_reference", "POLYRESEARCH_REFERENCE_PRICE"),
    ] {
        match reference_reconnect_config_load(client_key, |_| {
            toml::Value::String("not-an-integer".to_string())
        }) {
            Ok(()) => failures.push(format!(
                "clients.{client_key}.data.reconnect_timeout_ms malformed typed config loaded successfully"
            )),
            Err(rendered) => {
                if !(rendered.contains("error_variant=NtReconnectBudgetInvalidData")
                    && rendered.contains(provider_key)
                    && rendered.contains("reconnect_timeout_ms"))
                {
                    failures.push(format!(
                        "{client_key} malformed typed config did not expose the named reconnect-budget error: {rendered}"
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "malformed NT reconnect-budget provider validation failures: {failures:#?}"
    );
}

#[test]
fn config_load_accepts_reference_reconnect_timeout_one_millisecond_above_startup_bound() {
    let mut failures = Vec::new();

    for client_key in ["chainlink_reference", "polyresearch_reference"] {
        if let Err(error) =
            reference_reconnect_timeout_relative_to_startup_bound_load(client_key, 1)
        {
            failures.push(format!(
                "clients.{client_key}.data.reconnect_timeout_ms at startup bound plus one failed to load: {error}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "reference provider positive startup-bound validation failures: {failures:#?}"
    );
}

#[test]
fn config_load_rejects_nautilus_startup_bound_overflow_for_reference_clients() {
    let rendered = reference_reconnect_startup_bound_overflow_load_error([i64::MAX; 3])
        .expect("overflowing Nautilus startup bound should fail config load");

    assert!(
        rendered.contains("error_variant=NautilusStartupBoundOverflow")
            && rendered.contains("nautilus.timeout_connection_secs")
            && rendered.contains("nautilus.timeout_reconciliation_secs")
            && rendered.contains("nautilus.timeout_portfolio_secs"),
        "startup-bound overflow should expose a named validation error with every summed field: {rendered}"
    );
}

#[test]
fn config_load_rejects_nautilus_startup_bound_millisecond_overflow_for_reference_clients() {
    let rendered = reference_reconnect_startup_bound_overflow_load_error([i64::MAX, 1, 1])
        .expect("Nautilus startup bound exceeding milliseconds should fail config load");

    assert!(
        rendered.contains("error_variant=NautilusStartupBoundMillisecondsOverflow")
            && rendered.contains("startup_bound_secs="),
        "millisecond conversion overflow should expose a named validation error: {rendered}"
    );
}

#[test]
fn shipped_chainlink_gate_provider_configs_keep_only_configured_feed_bindings() {
    for relative_path in ["config/root.toml", "tests/fixtures/bolt_v3/root.toml"] {
        let source = std::fs::read_to_string(support::repo_path(relative_path))
            .expect("root config should be readable");

        let parsed = toml::from_str::<toml::Value>(&source).expect("root TOML should parse");
        let feed_bindings = parsed
            .get("gate_providers")
            .and_then(|value| value.get("resolution_oracle_primary"))
            .and_then(|value| value.get("chainlink_data_streams"))
            .and_then(|value| value.get("feed_bindings"))
            .and_then(toml::Value::as_array)
            .expect("root config should declare Chainlink feed bindings");
        assert_eq!(
            feed_bindings.len(),
            1,
            "{relative_path} should not ship unused canonical Chainlink feed bindings"
        );
        // The strategy mapping uses `configured-reference-price`, but the
        // report fetch/decode path still needs the real BTC feed id for the
        // tracked BTC-only live profile.
        let gate_feed_id = feed_bindings[0]
            .get("feed_id")
            .and_then(toml::Value::as_str)
            .expect("gate-provider feed binding should declare a feed_id");
        assert_ne!(
            gate_feed_id, OLD_CHAINLINK_FIXTURE_FEED_ID,
            "{relative_path} gate-provider mapping should not ship the old generic Chainlink fixture feed"
        );
        assert_ne!(
            gate_feed_id, ZERO_CHAINLINK_FEED_ID,
            "{relative_path} gate-provider mapping should not ship the placeholder zero Chainlink feed"
        );
        assert_eq!(
            gate_feed_id, CHAINLINK_BTC_TESTNET_FEED_ID,
            "{relative_path} gate-provider mapping should use the pinned BTC Chainlink feed"
        );
        assert_eq!(
            feed_bindings[0]
                .get("report_schema_version")
                .and_then(toml::Value::as_integer),
            Some(3),
            "{relative_path} gate-provider mapping should use the Chainlink V3 report schema"
        );
        assert_eq!(
            feed_bindings[0]
                .get("report_decimal_scale")
                .and_then(toml::Value::as_integer),
            Some(18),
            "{relative_path} gate-provider mapping should use the 18-decimal Chainlink report scale"
        );
        assert_eq!(
            feed_bindings[0]
                .get("resolution_identity")
                .and_then(toml::Value::as_str),
            Some("configured-reference-price"),
            "{relative_path} feed binding should match the shipped strategy mapping"
        );
    }
}

#[test]
fn shipped_chainlink_data_streams_catalog_is_btc_only() {
    // The Chainlink reference websocket subscribes to every configured feed_id
    // in this catalog, so the tracked live root must keep this BTC-only with the
    // tracked strategy_files/RV surfaces.
    let source = fs::read_to_string(support::repo_path("config/root.toml"))
        .expect("root config should be readable");
    let parsed = toml::from_str::<toml::Value>(&source).expect("root TOML should parse");
    let feed_bindings = parsed
        .get("chainlink_data_streams")
        .and_then(|value| value.get("feed_bindings"))
        .and_then(toml::Value::as_array)
        .expect("root config should declare live Chainlink Data Streams feed bindings");

    assert_eq!(
        feed_bindings.len(),
        1,
        "the tracked live Chainlink Data Streams catalog must subscribe only to BTC"
    );

    let binding = feed_bindings
        .first()
        .expect("BTC feed binding should be present");
    let instrument_id = binding
        .get("instrument_id")
        .and_then(toml::Value::as_str)
        .expect("feed binding should declare instrument_id");
    let feed_id = binding
        .get("feed_id")
        .and_then(toml::Value::as_str)
        .expect("feed binding should declare feed_id");
    assert_eq!(
        instrument_id, "BTC-USD.CHAINLINK",
        "tracked live Chainlink Data Streams catalog must stay BTC-only"
    );
    assert_eq!(
        feed_id, CHAINLINK_BTC_TESTNET_FEED_ID,
        "BTC strike feed_id drifted from its pinned testnet feed"
    );
    assert_eq!(
        binding
            .get("report_schema_version")
            .and_then(toml::Value::as_integer),
        Some(3),
        "BTC must use the Chainlink V3 report schema"
    );
    assert_eq!(
        binding
            .get("report_decimal_scale")
            .and_then(toml::Value::as_integer),
        Some(18),
        "BTC must use the 18-decimal Chainlink report scale"
    );
    assert_eq!(
        binding
            .get("price_precision")
            .and_then(toml::Value::as_integer),
        Some(8),
        "BTC must use 8dp NT price precision"
    );
}

#[test]
fn bolt_v3_fixture_chainlink_data_streams_catalog_matches_fixture_strategy() {
    // The fixture is a generic CONFIGURED_ASSET bundle, not the production BTC
    // root. It should still pin the one configured Chainlink catalog binding to
    // the live BTC feed shape so fixture-based adapter tests exercise the same
    // report schema/scale as the tracked root.
    let source = fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture root config should be readable");
    let parsed = toml::from_str::<toml::Value>(&source).expect("fixture root TOML should parse");
    let feed_bindings = parsed
        .get("chainlink_data_streams")
        .and_then(|value| value.get("feed_bindings"))
        .and_then(toml::Value::as_array)
        .expect("fixture root config should declare Chainlink Data Streams feed bindings");

    assert_eq!(
        feed_bindings.len(),
        1,
        "fixture Chainlink Data Streams catalog should keep exactly one configured-asset binding"
    );

    let binding = feed_bindings
        .first()
        .expect("fixture Chainlink feed binding should be present");
    assert_eq!(
        binding.get("instrument_id").and_then(toml::Value::as_str),
        Some("CONFIGURED_ASSET-USD.CHAINLINK"),
        "fixture Chainlink catalog should stay aligned with the fixture strategy target"
    );
    assert_eq!(
        binding.get("feed_id").and_then(toml::Value::as_str),
        Some(CHAINLINK_BTC_TESTNET_FEED_ID),
        "fixture Chainlink feed_id should stay pinned to the BTC testnet feed"
    );
    assert_eq!(
        binding
            .get("report_schema_version")
            .and_then(toml::Value::as_integer),
        Some(3),
        "fixture Chainlink binding should use the Chainlink V3 report schema"
    );
    assert_eq!(
        binding
            .get("report_decimal_scale")
            .and_then(toml::Value::as_integer),
        Some(18),
        "fixture Chainlink binding should use the 18-decimal Chainlink report scale"
    );
    assert_eq!(
        binding
            .get("price_precision")
            .and_then(toml::Value::as_integer),
        Some(8),
        "fixture Chainlink binding should use 8dp NT price precision"
    );
}

#[test]
fn bolt_v3_strategy_execution_client_id_uses_nt_typed_identifier() {
    // `BoltV3StrategyConfig.execution_client_id` is typed as
    // `nautilus_model::identifiers::ClientId`. The strategy block is
    // parsed via `toml::from_str(&content)` directly (borrowed source),
    // so NT's `impl_serialization_for_identifier!` macro routes the
    // value through `ClientId::new_checked` and rejects empty / non-ascii
    // strings at parse time without a bolt-side runtime guard.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::identifiers::ClientId;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let strategy = &loaded.strategies[0].config;
    let execution_client_id: ClientId = strategy.execution_client_id;
    assert_eq!(execution_client_id, ClientId::from("polymarket_main"));

    assert!(!strategy.signal_data.is_empty());
}

#[test]
fn bolt_v3_strategy_execution_client_id_rejects_empty_string_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3StrategyConfig;

    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated = strategy_toml.replace(
        "execution_client_id = \"polymarket_main\"",
        "execution_client_id = \"\"",
    );
    let err = toml::from_str::<BoltV3StrategyConfig>(&mutated)
        .expect_err("empty execution_client_id should be rejected by NT ClientId serde");
    let rendered = err.to_string();
    assert!(
        rendered.contains("empty") || rendered.contains("invalid"),
        "rejection should explain the empty execution_client_id, got: {rendered}"
    );
}

#[test]
fn bolt_v3_strategy_oms_type_uses_nt_canonical_enum() {
    // FINDING-1: `strategy.oms_type` is typed as `nautilus_model::enums::OmsType`
    // (not a bolt shadow enum). NT's enum_strum_serde! macro makes deserialize
    // case-insensitive, so the fixture's `oms_type = "netting"` (lowercase)
    // continues to parse.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_model::enums::OmsType;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let oms_type: OmsType = loaded.strategies[0].config.oms_type;
    assert_eq!(oms_type, OmsType::Netting);
}

#[test]
fn bolt_v3_strategy_oms_type_accepts_nt_variants() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    for supported_oms_type in ["hedging", "unspecified"] {
        let mutated_strategy = std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable")
        .replace(
            "oms_type = \"netting\"",
            &format!("oms_type = \"{supported_oms_type}\""),
        );
        let strategy: BoltV3StrategyConfig =
            toml::from_str(&mutated_strategy).expect("oms_type should parse via NT enum");
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.iter().all(|message| !message.contains("oms_type")),
            "NT oms_type variant {supported_oms_type} should not be narrowed by bolt validation: {messages:#?}"
        );
    }
}

#[test]
fn binary_oracle_strategy_rejects_legacy_price_to_beat_feed_id_under_runtime() {
    let messages = legacy_binary_oracle_runtime_field_messages(
        "price_to_beat_feed_id = \"0x1111111111111111111111111111111111111111111111111111111111111111\"",
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.runtime.price_to_beat_feed_id")
                && message.contains("[gate_providers.<id>.")
        }),
        "legacy price_to_beat_feed_id must fail closed with a gate-provider migration message: {messages:#?}"
    );
}

#[test]
fn binary_oracle_strategy_rejects_legacy_price_to_beat_source_under_runtime() {
    let messages = legacy_binary_oracle_runtime_field_messages(
        "price_to_beat_source = \"chainlink_data_streams.report_at_boundary\"",
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.runtime.price_to_beat_source")
                && message.contains("target.gate_subscriptions")
        }),
        "legacy price_to_beat_source must fail closed with a target gate migration message: {messages:#?}"
    );
}

#[test]
fn bolt_v3_strategy_execution_client_id_rejects_data_only_client_with_client_vocabulary() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let execution_block = fixture_polymarket_execution_block();
    let root: BoltV3RootConfig = toml::from_str(&replace_in_fixture_root(&execution_block, ""))
        .expect("data-only polymarket fixture should parse");
    let strategy: BoltV3StrategyConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&root, &loaded);
    let rendered = messages.join("\n");
    assert!(rendered.contains("strategy execution_client_id `polymarket_main`"));
    assert!(rendered.contains("execution-capable client"));
    assert!(rendered.contains("referenced client has no [execution] block"));
    assert!(!rendered.contains("execution-capable venue"));
    assert!(!rendered.contains("referenced venue"));
}

#[test]
fn binary_oracle_resolution_retry_interval_accepts_chainlink_http_timeout_below_retry_interval() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let strategy_raw = format!(
        "{}\n\n[resolution_data]\ndata_client_id = \"chainlink_strike\"\ninstrument_id = \"CONFIGURED_ASSET-USD.CHAINLINK\"\n",
        std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml"
        ))
        .expect("strategy fixture should be readable"),
    );
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&strategy_raw).expect("strategy fixture should parse with resolution_data");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&root, &loaded);
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("http_timeout_secs")),
        "Chainlink strike timeout below retry interval should not fail timeout validation: {messages:#?}"
    );
}

#[test]
fn binary_oracle_resolution_retry_interval_rejects_chainlink_http_timeout_at_retry_interval() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let root: BoltV3RootConfig = toml::from_str(&replace_in_fixture_root(
        "http_timeout_secs = 4",
        "http_timeout_secs = 5",
    ))
    .expect("root fixture with equal Chainlink timeout should parse");
    let strategy_raw = format!(
        "{}\n\n[resolution_data]\ndata_client_id = \"chainlink_strike\"\ninstrument_id = \"CONFIGURED_ASSET-USD.CHAINLINK\"\n",
        std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml"
        ))
        .expect("strategy fixture should be readable"),
    );
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&strategy_raw).expect("strategy fixture should parse with resolution_data");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&root, &loaded);
    let rendered = messages.join("\n");
    assert!(
        rendered.contains("target.retry_interval_secs `5`")
            && rendered.contains("clients.chainlink_strike.data.http_timeout_secs `5`")
            && rendered.contains("same-boundary in-flight fetch dedupe"),
        "Chainlink strike timeout at retry interval must fail closed: {messages:#?}"
    );
}

#[test]
fn bolt_v3_polymarket_client_rejects_execution_without_data_block_with_client_vocabulary() {
    // Bug class: a config can split a Polymarket venue across two
    // `clients.<id>` blocks (one execution-only, one data-only). The
    // existing execution-capable strategy check still passes, but
    // `bolt_v3_providers::polymarket::build_market_slug_filters_for_client`
    // binds per-target market-slug filters to a single `client_key` and
    // skips every target whose `execution_client_id != client_key`.
    // Splitting the data adapter off the execution client_key therefore
    // silently strips the configured target market restriction during
    // data-client mapping. Fail closed inside the Polymarket provider
    // binding: every Polymarket client that declares `[execution]` must
    // also declare a co-located `[data]` block under the same
    // `clients.<id>`. The rule lives in the polymarket binding because
    // it is a Polymarket-internal invariant; core validation stays
    // provider-neutral per the source-fence.
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let polymarket_main_data_block = "[clients.polymarket_main.data]\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/market\"\nbase_url_rtds = \"wss://ws-live-data.polymarket.com\"\nbase_url_gamma = \"https://gamma-api.polymarket.com\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_secs = 60\nws_timeout_secs = 30\nsubscribe_new_markets = false\ndrop_quotes_missing_side = true\nnew_market_fetch_max_concurrency = 8\nauto_load_missing_instruments = false\nauto_load_debounce_ms = 250\nauto_load_max_retries = 12\nauto_load_retry_delay_initial_secs = 5\nauto_load_retry_delay_max_secs = 15\nresolve_poll_enabled = false\nresolve_poll_interval_secs = 30\nresolve_poll_grace_secs = 10\nresolve_poll_max_wait_secs = 1800\nupdate_instruments_interval_mins = 1\nws_max_subscriptions = 200\ntransport_backend = \"sockudo\"\n\n";
    let polymarket_data_only_client = "\n[clients.polymarket_data]\nvenue = \"POLYMARKET\"\n\n[clients.polymarket_data.data]\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/market\"\nbase_url_rtds = \"wss://ws-live-data.polymarket.com\"\nbase_url_gamma = \"https://gamma-api.polymarket.com\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_secs = 60\nws_timeout_secs = 30\nsubscribe_new_markets = false\ndrop_quotes_missing_side = true\nnew_market_fetch_max_concurrency = 8\nauto_load_missing_instruments = false\nauto_load_debounce_ms = 250\nauto_load_max_retries = 12\nauto_load_retry_delay_initial_secs = 5\nauto_load_retry_delay_max_secs = 15\nresolve_poll_enabled = false\nresolve_poll_interval_secs = 30\nresolve_poll_grace_secs = 10\nresolve_poll_max_wait_secs = 1800\nupdate_instruments_interval_mins = 1\nws_max_subscriptions = 200\ntransport_backend = \"sockudo\"\n";
    let split_fixture = format!(
        "{}{}",
        replace_in_fixture_root(polymarket_main_data_block, ""),
        polymarket_data_only_client
    );
    let root: BoltV3RootConfig =
        toml::from_str(&split_fixture).expect("split polymarket clients fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        rendered.contains("clients.polymarket_main"),
        "expected rejection citing `clients.polymarket_main`, got: {rendered}"
    );
    assert!(
        rendered.contains("provider=POLYMARKET"),
        "expected rejection to tag `provider=POLYMARKET`, got: {rendered}"
    );
    assert!(
        rendered.contains("declares [execution] but no [data] block"),
        "expected rejection citing `declares [execution] but no [data] block`, got: {rendered}"
    );
    assert!(
        rendered.contains("client_key"),
        "expected rejection to use NT client vocabulary `client_key`, got: {rendered}"
    );
    assert!(
        !rendered.contains("venues."),
        "rejection must not regress to stale `venues.` vocabulary, got: {rendered}"
    );
    assert!(
        !rendered.contains("data-capable venue"),
        "rejection must not regress to stale `data-capable venue` vocabulary, got: {rendered}"
    );
}

#[test]
fn bolt_v3_runtime_mode_uses_nt_environment_enum() {
    // FINDING-1: `runtime.mode` is typed as `nautilus_common::enums::Environment`
    // (not a bolt shadow `RuntimeMode`). NT's `Environment` derives serde
    // directly without `#[serde(rename_all)]`, so the on-disk fixture value
    // is PascalCase (`mode = "Live"`). Bolt's validator rejects Backtest and
    // Sandbox explicitly because the binary is a live-trading node only.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_common::enums::Environment;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let mode: Environment = loaded.root.runtime.mode;
    assert_eq!(mode, Environment::Live);
}

#[test]
fn bolt_v3_runtime_mode_rejects_backtest_and_sandbox_variants() {
    // FINDING-1: bolt-v3 only supports `Environment::Live`. The binary is a
    // live LiveNode wrapper, so the validator must reject `Backtest` and
    // `Sandbox` explicitly rather than letting them silently flow into NT's
    // kernel boot path.
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for variant in ["Backtest", "Sandbox"] {
        let mutated = replace_in_fixture_root("mode = \"Live\"", &format!("mode = \"{variant}\""));
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("non-Live Environment variant should parse via NT");
        let messages = validate_root_only(&root);
        assert!(
            messages
                .iter()
                .any(|m| m.contains("runtime.mode") && m.contains("Live")),
            "expected runtime.mode rejection citing Live variant for `{variant}`, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_order_execution_mode_is_required_under_runtime() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let missing = fixture_root_without_order_execution_mode();
    let error = toml::from_str::<BoltV3RootConfig>(&missing)
        .expect_err("runtime.order_execution_mode must be required and fail closed");

    assert!(
        error.to_string().contains("order_execution_mode"),
        "missing order_execution_mode should be named in the parse error: {error}"
    );
}

#[test]
fn bolt_v3_order_execution_mode_accepts_only_lowercase_live_and_shadow() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;
    use bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionMode;

    for (value, expected) in [
        ("live", BoltV3OrderExecutionMode::Live),
        ("shadow", BoltV3OrderExecutionMode::Shadow),
    ] {
        let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_order_execution_mode(value))
            .unwrap_or_else(|error| {
                panic!("lowercase runtime.order_execution_mode={value:?} should parse: {error}")
            });
        assert_eq!(root.runtime.order_execution_mode, expected);
    }

    for value in ["Live", "Shadow", "LIVE", "SHADOW"] {
        let error =
            toml::from_str::<BoltV3RootConfig>(&fixture_root_with_order_execution_mode(value))
                .expect_err("mixed-case order_execution_mode must not parse");
        assert!(
            error.to_string().contains("order_execution_mode"),
            "mixed-case value {value:?} should identify order_execution_mode: {error}"
        );
    }
}

#[test]
fn binary_oracle_parameters_reject_stale_strategy_local_submit_orders() {
    use bolt_v2::{
        bolt_v3_config::BoltV3StrategyConfig,
        strategies::binary_oracle_edge_taker::archetype::ParametersBlock,
    };

    let stale = fixture_strategy_with_submit_orders("true");
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&stale).expect("strategy envelope should still parse");
    let error = strategy
        .parameters
        .try_into::<ParametersBlock>()
        .expect_err("parameters.submit_orders must be rejected as stale strategy-local policy");

    assert!(
        error.to_string().contains("submit_orders"),
        "stale submit_orders rejection should name the field: {error}"
    );
}

#[test]
fn shadow_order_execution_mode_rejects_managed_venue_action_knobs() {
    for (field, stale_line, replacement) in [
        ("manage_stop", "manage_stop = false", "manage_stop = true"),
        (
            "manage_gtd_expiry",
            "manage_gtd_expiry = false",
            "manage_gtd_expiry = true",
        ),
        (
            "manage_contingent_orders",
            "manage_contingent_orders = false",
            "manage_contingent_orders = true",
        ),
        (
            "external_order_claims",
            "external_order_claims = []",
            "external_order_claims = [\"AUXILIARY.SOURCE\"]",
        ),
    ] {
        let root_toml = fixture_root_with_order_execution_mode("shadow");
        let strategy_toml =
            strategy_fixture_without_submit_orders().replace(stale_line, replacement);
        let messages =
            strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
        let rendered = messages.join("\n");
        assert!(
            rendered.contains(field)
                && rendered.contains("order_execution_mode")
                && rendered.contains("shadow"),
            "shadow runtime.order_execution_mode should reject {field}; got: {messages:#?}"
        );
    }
}

#[test]
fn shadow_order_execution_mode_reports_every_managed_venue_action_knob() {
    let root_toml = fixture_root_with_order_execution_mode("shadow");
    let strategy_toml = strategy_fixture_without_submit_orders()
        .replace("manage_stop = false", "manage_stop = true")
        .replace("manage_gtd_expiry = false", "manage_gtd_expiry = true")
        .replace(
            "manage_contingent_orders = false",
            "manage_contingent_orders = true",
        )
        .replace(
            "external_order_claims = []",
            "external_order_claims = [\"AUXILIARY.SOURCE\"]",
        );
    let messages =
        strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
    let rendered = messages.join("\n");

    for field in [
        "manage_stop",
        "manage_gtd_expiry",
        "manage_contingent_orders",
        "external_order_claims",
    ] {
        assert!(
            rendered.contains(field),
            "shadow validation should collect {field}; got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_logging_levels_use_nt_canonical_enum() {
    // FINDING-1: `logging.stdout_level` and `logging.fileout_level` are typed
    // as `nautilus_common::enums::LogLevel` (not a bolt shadow). NT's
    // LogLevel uses explicit `#[serde(rename = "UPPERCASE")]` per variant
    // (and notably uses `Warning` rather than the bolt shadow's `Warn`), so
    // the fixture's `stdout_level = "INFO"` / `fileout_level = "INFO"`
    // continue to parse unchanged.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use nautilus_common::enums::LogLevel;

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");

    let stdout_level: LogLevel = loaded.root.logging.stdout_level;
    let fileout_level: LogLevel = loaded.root.logging.fileout_level;
    assert_eq!(stdout_level, LogLevel::Info);
    assert_eq!(fileout_level, LogLevel::Info);
}

#[test]
fn bolt_v3_logging_levels_accept_nt_warning_uppercase_spelling() {
    // FINDING-1: NT spells warning-level as `"WARNING"` (its shadow had
    // `Warn`). Switching to the NT canonical enum means `stdout_level = "WARNING"`
    // is the supported spelling; `"WARN"` is no longer accepted. Lock this so
    // a future regression to `Warn` immediately fails.
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;
    use nautilus_common::enums::LogLevel;

    let warning_root =
        replace_in_fixture_root("stdout_level = \"INFO\"", "stdout_level = \"WARNING\"");
    let root: BoltV3RootConfig =
        toml::from_str(&warning_root).expect("NT WARNING level should parse");
    assert_eq!(root.logging.stdout_level, LogLevel::Warning);

    let warn_root = replace_in_fixture_root("stdout_level = \"INFO\"", "stdout_level = \"WARN\"");
    let err = toml::from_str::<BoltV3RootConfig>(&warn_root)
        .expect_err("legacy WARN spelling should be rejected by NT LogLevel");
    let rendered = err.to_string();
    assert!(
        rendered.contains("WARN") && (rendered.contains("variant") || rendered.contains("unknown")),
        "rejection should reference the WARN spelling and explain it is unknown, got: {rendered}"
    );
}

#[test]
fn bolt_v3_archetype_order_params_use_nt_canonical_enums() {
    // FINDING-1: archetype `[parameters.*_order]` rows are typed with NT's
    // canonical `OrderType` and `TimeInForce` (not bolt shadow enums).
    // NT serde is case-insensitive, so the fixture's `order_type = "market"`
    // and `time_in_force = "fok"` continue to parse unchanged.
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use bolt_v2::strategies::binary_oracle_edge_taker::archetype::ParametersBlock;
    use nautilus_model::enums::{OrderType, TimeInForce};

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("v3 config should load");
    let strategy = &loaded.strategies[0].config;
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .expect("fixture parameters block should deserialize as binary_oracle_edge_taker");

    let entry_order_type: OrderType = parameters.entry_order.order_type;
    let entry_tif: TimeInForce = parameters.entry_order.time_in_force;
    assert_eq!(entry_order_type, OrderType::Market);
    assert_eq!(entry_tif, TimeInForce::Fok);

    let exit_order_type: OrderType = parameters.exit_order.order_type;
    let exit_tif: TimeInForce = parameters.exit_order.time_in_force;
    assert_eq!(exit_order_type, OrderType::Market);
    assert_eq!(exit_tif, TimeInForce::Ioc);
}

#[test]
fn bolt_v3_archetype_runtime_parameters_reject_unknown_fields() {
    use bolt_v2::{
        bolt_v3_config::BoltV3StrategyConfig,
        strategies::binary_oracle_edge_taker::archetype::ParametersBlock,
    };

    let strategy_toml = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mutated = strategy_toml.replace(
        "lead_jitter_max_ms = 250",
        "lead_jitter_max_ms = 250\nrisk_lambdaa = 0.5",
    );
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&mutated).expect("strategy envelope should parse");
    let parsed: Result<ParametersBlock, _> = strategy.parameters.try_into();
    let err = parsed.expect_err("unknown fields inside [parameters.runtime] should fail parse");
    let rendered = err.to_string();
    assert!(
        rendered.contains("unknown field") && rendered.contains("risk_lambdaa"),
        "runtime parameter rejection should come from deny_unknown_fields, got: {rendered}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_post_only_gtc_entry_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    let maker_strategy = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace("time_in_force = \"fok\"", "time_in_force = \"gtc\"")
    .replacen("is_post_only = false", "is_post_only = true", 1);
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_strategy)
        .expect("post-only GTC entry order should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert_binary_oracle_entry_order_shape_rejected(&messages, "post-only GTC entry order");
}

#[test]
fn bolt_v3_archetype_accepts_post_only_gtc_exit_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    let taker_strategy = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let maker_exit_strategy = mutate_parameters_exit_order(&taker_strategy, |exit_block| {
        exit_block
            .replace("order_type = \"market\"", "order_type = \"limit\"")
            .replace("time_in_force = \"ioc\"", "time_in_force = \"gtc\"")
            .replacen("is_post_only = false", "is_post_only = true", 1)
    });
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_exit_strategy)
        .expect("post-only GTC exit order should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "post-only GTC exit order should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_mixed_maker_taker_order_configs() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let validate_strategy = |strategy_source: String, case_name: &str| {
        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
            .unwrap_or_else(|error| panic!("{case_name} should parse via NT order enums: {error}"));
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.is_empty(),
            "{case_name} should be accepted by binary_oracle_edge_taker validation: {messages:#?}"
        );
    };

    let maker_entry_taker_exit = fixture
        .replace("time_in_force = \"fok\"", "time_in_force = \"gtc\"")
        .replacen("is_post_only = false", "is_post_only = true", 1);
    let strategy: BoltV3StrategyConfig = toml::from_str(&maker_entry_taker_exit)
        .expect("maker entry with taker exit should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert_binary_oracle_entry_order_shape_rejected(&messages, "maker entry with taker exit");

    let maker_exit_taker_entry = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
            .replace("order_type = \"market\"", "order_type = \"limit\"")
            .replace("time_in_force = \"ioc\"", "time_in_force = \"gtc\"")
            .replacen("is_post_only = false", "is_post_only = true", 1)
    });
    validate_strategy(maker_exit_taker_entry, "taker entry with maker exit");
}

#[test]
fn bolt_v3_archetype_accepts_configured_forced_exit_order_template() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    let mut forced_exit_order = parameters
        .get("exit_order")
        .cloned()
        .expect("fixture parameters should include exit_order");
    let forced_exit_table = forced_exit_order
        .as_table_mut()
        .expect("forced exit fixture should be an order table");
    forced_exit_table.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    forced_exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    forced_exit_table.insert("is_post_only".to_string(), toml::Value::Boolean(true));
    parameters.insert("forced_exit_order".to_string(), forced_exit_order);

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "configured forced_exit_order should validate through the NT order template path: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_polymarket_unsupported_market_exit_shape() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let exit_table = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table")
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order");
    exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_table.insert("is_reduce_only".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.exit_order order_type=market has time_in_force=gtc")
                && message.contains("must use time_in_force=ioc or fok")
                && message.contains("configured execution provider")
        }),
        "market exit GTC should be rejected before it can reach the configured execution provider: {messages:#?}"
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.exit_order.is_reduce_only must be false")
                && message.contains("configured execution provider")
        }),
        "market exit reduce-only should be rejected before it can reach the configured execution provider: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_polymarket_unsupported_market_forced_exit_shape() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let forced_exit_table = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table")
        .get_mut("forced_exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include forced_exit_order");
    forced_exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    forced_exit_table.insert("is_reduce_only".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.forced_exit_order order_type=market has time_in_force=gtc")
                && message.contains("must use time_in_force=ioc or fok")
                && message.contains("configured execution provider")
        }),
        "market forced-exit GTC should be rejected before it can reach the configured execution provider: {messages:#?}"
    );
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.forced_exit_order.is_reduce_only must be false")
                && message.contains("configured execution provider")
        }),
        "market forced-exit reduce-only should be rejected before it can reach the configured execution provider: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_polymarket_limit_exit_reduce_only_shape() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let exit_table = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table")
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order");
    exit_table.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_table.insert("is_post_only".to_string(), toml::Value::Boolean(true));
    exit_table.insert("is_reduce_only".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.exit_order.is_reduce_only must be false")
                && message.contains("configured execution provider")
        }),
        "limit exit reduce-only should be rejected before it can reach the configured execution provider: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_polymarket_limit_forced_exit_reduce_only_shape() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let forced_exit_table = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table")
        .get_mut("forced_exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include forced_exit_order");
    forced_exit_table.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    forced_exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    forced_exit_table.insert("is_post_only".to_string(), toml::Value::Boolean(true));
    forced_exit_table.insert("is_reduce_only".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.forced_exit_order.is_reduce_only must be false")
                && message.contains("configured execution provider")
        }),
        "limit forced-exit reduce-only should be rejected before it can reach the configured execution provider: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_keeps_default_provider_market_exit_shape_permissive() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };
    use nautilus_model::identifiers::Venue;

    let mut stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let synthetic_venue = Venue::from("HYPERLIQUID");
    stable_root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture should include execution client")
        .venue = synthetic_venue;
    let settlement_pool = stable_root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should include settlement capital pools")
        .iter_mut()
        .find(|pool| pool.pool_id == "polymarket-prediction-live")
        .expect("fixture should include the execution account settlement pool");
    assert!(
        !settlement_pool.enforce_submit_admission,
        "provider-shape fixture must not arm submit admission"
    );
    settlement_pool.venue_id = synthetic_venue.as_str().to_string();
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let exit_table = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table")
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order");
    exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_table.insert("is_reduce_only".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.is_empty(),
        "default provider constraints should remain permissive for market exit shapes: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_manage_stop_with_non_market_forced_exit_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    strategy.manage_stop = true;
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    let forced_exit_table = parameters
        .get_mut("forced_exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include forced_exit_order");
    forced_exit_table.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    forced_exit_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    forced_exit_table.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("manage_stop")
                && message.contains("forced_exit_order")
                && message.contains("market")
        }),
        "manage_stop should reject forced_exit_order semantics NT close_all_positions cannot honor: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_non_positive_order_notional_target() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    for bad_value in ["0", "-1.00"] {
        let mut strategy: BoltV3StrategyConfig = toml::from_str(
            &fs::read_to_string(support::repo_path(
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
            ))
            .expect("strategy fixture should be readable"),
        )
        .expect("strategy fixture should parse");
        let parameters = strategy
            .parameters
            .as_table_mut()
            .expect("strategy parameters should be a table");
        parameters.insert(
            "order_notional_target".to_string(),
            toml::Value::String(bad_value.to_string()),
        );

        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];

        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.iter().any(|message| {
                message.contains("parameters.order_notional_target")
                    && message.contains("must be a positive decimal")
            }),
            "order_notional_target={bad_value} must fail closed as non-positive at load: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_non_positive_maximum_position_notional() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    for bad_value in ["0", "-5.00"] {
        let mut strategy: BoltV3StrategyConfig = toml::from_str(
            &fs::read_to_string(support::repo_path(
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
            ))
            .expect("strategy fixture should be readable"),
        )
        .expect("strategy fixture should parse");
        let parameters = strategy
            .parameters
            .as_table_mut()
            .expect("strategy parameters should be a table");
        parameters.insert(
            "maximum_position_notional".to_string(),
            toml::Value::String(bad_value.to_string()),
        );

        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];

        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.iter().any(|message| {
                message.contains("parameters.maximum_position_notional")
                    && message.contains("must be a positive decimal")
            }),
            "maximum_position_notional={bad_value} must fail closed as non-positive at load: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_order_notional_target_above_maximum_position_notional() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    // order target (5.00) <= root risk.default_max_notional_per_order (10.00) but ABOVE the
    // position cap (1.00): an unsatisfiable per-order target must fail closed at load, not
    // silently clamp at runtime.
    parameters.insert(
        "order_notional_target".to_string(),
        toml::Value::String("5.00".to_string()),
    );
    parameters.insert(
        "maximum_position_notional".to_string(),
        toml::Value::String("1.00".to_string()),
    );

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.order_notional_target")
                && message.contains("must be <=")
                && message.contains("parameters.maximum_position_notional")
        }),
        "order_notional_target above maximum_position_notional must fail closed at load: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_negative_edge_threshold_basis_points() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    // A negative edge threshold admits negative-edge (guaranteed-loss) entries and must
    // fail closed at load (A-EDGE).
    parameters.insert(
        "edge_threshold_basis_points".to_string(),
        toml::Value::Integer(-1),
    );

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.edge_threshold_basis_points")
                && message.contains("must be >= 0")
        }),
        "negative edge_threshold_basis_points must fail closed at load: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_zero_sizing_ev_reference_bps() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    let runtime = parameters
        .get_mut("runtime")
        .and_then(|value| value.as_table_mut())
        .expect("strategy runtime parameters should be a table");
    // A zero EV sizing reference makes the dollar-scale division undefined; the
    // runtime sizing path fails closed to a zero size and the strategy silently
    // never submits, so the misconfiguration must fail closed at load.
    runtime.insert(
        "sizing_ev_reference_bps".to_string(),
        toml::Value::Integer(0),
    );

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.runtime.sizing_ev_reference_bps")
                && message.contains("must be > 0")
        }),
        "zero sizing_ev_reference_bps must fail closed at load: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_oversized_sizing_ev_reference_bps() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    let runtime = parameters
        .get_mut("runtime")
        .and_then(|value| value.as_table_mut())
        .expect("strategy runtime parameters should be a table");
    runtime.insert(
        "sizing_ev_reference_bps".to_string(),
        toml::Value::Integer(10_001),
    );

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.runtime.sizing_ev_reference_bps")
                && message.contains("must be at most 10000")
        }),
        "oversized sizing_ev_reference_bps must fail closed at load: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_negative_or_non_finite_risk_lambda() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    // TOML floats legally admit negative, nan, and inf values. Each loads
    // through serde but makes the runtime sizing path fail soft to a zero
    // size (a silently dead strategy), so each must fail closed at load.
    for bad_risk_lambda in [-0.5, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut strategy: BoltV3StrategyConfig = toml::from_str(
            &fs::read_to_string(support::repo_path(
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
            ))
            .expect("strategy fixture should be readable"),
        )
        .expect("strategy fixture should parse");
        let parameters = strategy
            .parameters
            .as_table_mut()
            .expect("strategy parameters should be a table");
        let runtime = parameters
            .get_mut("runtime")
            .and_then(|value| value.as_table_mut())
            .expect("strategy runtime parameters should be a table");
        runtime.insert(
            "risk_lambda".to_string(),
            toml::Value::Float(bad_risk_lambda),
        );

        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];

        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.iter().any(|message| {
                message.contains("parameters.runtime.risk_lambda")
                    && message.contains("finite and >= 0")
            }),
            "risk_lambda {bad_risk_lambda} must fail closed at load: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_accepts_market_quote_quantity_entry_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    // Build the Lane 1 entry shape by cloning the fixture's market exit_order, then set
    // is_quote_quantity=true, is_reduce_only=false, and FOK because entries open the position
    // and must submit as venue-conformant market/FOK dollar-sized BUYs.
    let mut entry_order = parameters
        .get("exit_order")
        .cloned()
        .expect("fixture parameters should include a market exit_order");
    let entry_table = entry_order
        .as_table_mut()
        .expect("exit_order fixture should be an order table");
    entry_table.insert("side".to_string(), toml::Value::String("buy".to_string()));
    entry_table.insert(
        "position_side".to_string(),
        toml::Value::String("long".to_string()),
    );
    entry_table.insert("is_quote_quantity".to_string(), toml::Value::Boolean(true));
    entry_table.insert("is_reduce_only".to_string(), toml::Value::Boolean(false));
    entry_table.insert(
        "time_in_force".to_string(),
        toml::Value::String("fok".to_string()),
    );
    parameters.insert("entry_order".to_string(), entry_order);

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("parameters.entry_order")),
        "market quote-quantity FOK entry must be accepted at load: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_quote_quantity_limit_entry_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    let parameters = strategy
        .parameters
        .as_table_mut()
        .expect("strategy parameters should be a table");
    // Lane 1 only enables market/FOK dollar-sized BUYs; non-market quote-quantity entries
    // still fail closed through the executable-shape rule.
    let entry_order = parameters
        .get_mut("entry_order")
        .expect("fixture parameters should include an entry_order")
        .as_table_mut()
        .expect("entry_order fixture should be an order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    entry_order.insert("is_quote_quantity".to_string(), toml::Value::Boolean(true));

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.entry_order")
                && validation_message_has_code(
                    message,
                    BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE,
                )
        }),
        "limit quote-quantity entry must fail closed at load via executable shape: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_reduce_only_entry_order() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let strategy_source = fixture.replacen("is_reduce_only = false", "is_reduce_only = true", 1);
    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("reduce-only entry order should parse typed config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("entry_order")
                && validation_message_has_code(message, BINARY_ORACLE_ENTRY_ORDER_REDUCE_ONLY_CODE)
        }),
        "reduce-only entry order should be rejected before NT submission with a stable code: {messages:#?}"
    );
    assert!(
        !messages.iter().any(|message| validation_message_has_code(
            message,
            BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE
        )),
        "reduce-only entry order should use the specific reduce-only error, not the broad executable-shape error: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_entry_limit_gtc_and_accepts_exit_limit_fok() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let validate_strategy = |strategy_source: String, case_name: &str| {
        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
            .unwrap_or_else(|error| panic!("{case_name} should parse via NT order enums: {error}"));
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.is_empty(),
            "{case_name} should be accepted by NT-order invariant validation: {messages:#?}"
        );
    };

    let entry_market_gtc = fixture.replace("time_in_force = \"fok\"", "time_in_force = \"gtc\"");
    let strategy: BoltV3StrategyConfig = toml::from_str(&entry_market_gtc)
        .expect("entry market GTC without post-only should parse via NT order enums");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert_binary_oracle_entry_order_shape_rejected(
        &messages,
        "entry market GTC without post-only",
    );

    let exit_limit_fok = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
            .replace("order_type = \"market\"", "order_type = \"limit\"")
            .replace("time_in_force = \"ioc\"", "time_in_force = \"fok\"")
    });
    validate_strategy(exit_limit_fok, "exit limit FOK without post-only");
}

#[test]
fn bolt_v3_archetype_rejects_short_side_order_contract_until_short_economics_exists() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let short_strategy = fixture
        .replacen("side = \"buy\"", "side = \"sell\"", 1)
        .replacen("position_side = \"long\"", "position_side = \"short\"", 1);
    let short_strategy = mutate_parameters_exit_order(&short_strategy, |exit_block| {
        exit_block
            .replacen("side = \"sell\"", "side = \"buy\"", 1)
            .replacen("position_side = \"long\"", "position_side = \"short\"", 1)
    });

    let strategy: BoltV3StrategyConfig =
        toml::from_str(&short_strategy).expect("coherent short-side order contract should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("short-side") && message.contains("binary_oracle_edge_taker")
        }),
        "short-side order contract should be rejected until strategy short economics exists: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_incoherent_order_position_contract() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let incoherent_strategy = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block.replacen("side = \"sell\"", "side = \"buy\"", 1)
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&incoherent_strategy)
        .expect("incoherent order position contract should still parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("position contract is not supported")
                && message.contains("long requires entry side=buy, exit side=sell")
                && message.contains("binary_oracle_edge_taker")
        }),
        "incoherent order position contract should be rejected with contract guidance: {messages:#?}"
    );
}

#[test]
fn polymarket_post_order_params_declares_camel_case_is_post_only_flag() {
    use nautilus_polymarket::{common::enums::PolymarketOrderType, http::query::PostOrderParams};

    for post_only in [false, true] {
        let params = PostOrderParams {
            order_type: PolymarketOrderType::GTC,
            post_only,
        };
        let json = serde_json::to_value(params)
            .expect("official Polymarket PostOrderParams must serialize");
        let object = json
            .as_object()
            .expect("official Polymarket PostOrderParams must serialize as an object");

        assert_eq!(
            object.get("postOnly").and_then(serde_json::Value::as_bool),
            post_only.then_some(true)
        );
        assert!(!object.contains_key("post_only"));
    }
}

#[test]
fn bolt_v3_archetype_rejects_unsupported_nt_order_type_variants() {
    // NT exposes model variants which the pinned single-order OrderFactory does
    // not expose as public constructors. Those remain unsupported here even
    // though they parse through NT serde.
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");

    let strategy_fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    for (toml_order_type, nt_order_type) in [
        ("market_to_limit", "MARKET_TO_LIMIT"),
        ("trailing_stop_limit", "TRAILING_STOP_LIMIT"),
    ] {
        let mutated_strategy = strategy_fixture.replace(
            "order_type = \"market\"",
            &format!("order_type = \"{toml_order_type}\""),
        );
        let strategy: BoltV3StrategyConfig =
            toml::from_str(&mutated_strategy).unwrap_or_else(|error| {
                panic!("{toml_order_type} should parse via NT OrderType: {error}")
            });
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);
        assert!(
            messages.iter().any(|m| {
                m.contains("entry_order") && m.contains(nt_order_type) && m.contains("OrderFactory")
            }),
            "expected entry_order rejection citing the OrderFactory gap for {toml_order_type}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_gtd_limit_without_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let entry_gtd_source = mutate_parameters_entry_order(&fixture, |entry_block| {
        entry_block
            .replace("order_type = \"market\"", "order_type = \"limit\"")
            .replace("time_in_force = \"fok\"", "time_in_force = \"gtd\"")
            .replace("is_quote_quantity = true", "is_quote_quantity = false")
    });
    let entry_gtd_strategy: BoltV3StrategyConfig =
        toml::from_str(&entry_gtd_source).expect("gtd should parse via NT TimeInForce");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: entry_gtd_strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages
            .iter()
            .any(|m| { m.contains("entry_order") && m.contains("expire_time_unix_nanos") }),
        "expected entry_order GTD rejection requiring expiry, got: {messages:#?}"
    );

    let exit_gtd_strategy: BoltV3StrategyConfig =
        toml::from_str(&fixture.replace("time_in_force = \"ioc\"", "time_in_force = \"gtd\""))
            .expect("gtd should parse via NT TimeInForce");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: exit_gtd_strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages
            .iter()
            .any(|m| { m.contains("exit_order") && m.contains("time_in_force=gtd") }),
        "expected exit_order market GTD rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_order_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let entry_market_with_expiry = mutate_parameters_entry_order(&fixture, |entry_block| {
        entry_block.replace(
            "time_in_force = \"fok\"",
            "time_in_force = \"fok\"\nexpire_time_unix_nanos = 4102444800000000000",
        )
    });
    let entry_strategy: BoltV3StrategyConfig = toml::from_str(&entry_market_with_expiry)
        .expect("market entry expiry should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: entry_strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|m| {
            m.contains("entry_order")
                && m.contains("expire_time_unix_nanos")
                && m.contains("order_type=market")
        }),
        "expected entry_order market expiry rejection, got: {messages:#?}"
    );

    let exit_market_with_expiry = fixture.replace(
        "time_in_force = \"ioc\"\nis_post_only = false",
        "time_in_force = \"ioc\"\nexpire_time_unix_nanos = 4102444800000000000\nis_post_only = false",
    );
    let exit_strategy: BoltV3StrategyConfig = toml::from_str(&exit_market_with_expiry)
        .expect("market exit expiry should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: exit_strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|m| {
            m.contains("exit_order")
                && m.contains("expire_time_unix_nanos")
                && m.contains("order_type=market")
        }),
        "expected exit_order market expiry rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_entry_gtd_limit_order_with_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let gtd_strategy_source = fixture.replace(
        "time_in_force = \"fok\"\nis_post_only = false",
        "time_in_force = \"gtd\"\nexpire_time_unix_nanos = 4102444800000000000\nis_post_only = false",
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&gtd_strategy_source)
        .expect("GTD expiry should parse through the typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(
        &messages,
        "entry GTD market order with expiry",
    );
}

#[test]
fn bolt_v3_archetype_accepts_non_gtd_limit_expiry_as_nt_pass_through() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
            .replace("order_type = \"market\"", "order_type = \"limit\"")
            .replace("time_in_force = \"ioc\"", "time_in_force = \"fok\"")
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"fok\"\nexpire_time_unix_nanos = 4102444800000000000\nis_post_only = false",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("non-GTD exit limit expiry should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "non-GTD exit limit expiry should stay valid because pinned NT preserves it: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_non_triggered_entry_order_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture.replace(
        "time_in_force = \"fok\"\nis_post_only = false",
        "time_in_force = \"fok\"\ntrigger_price = 0.52\nis_post_only = false",
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("non-triggered entry trigger_price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected non-triggered entry_order trigger_price rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_non_triggered_exit_order_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block.replacen(
            "time_in_force = \"ioc\"\nis_post_only = false",
            "time_in_force = \"ioc\"\ntrigger_price = 0.48\nis_post_only = false",
            1,
        )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("non-triggered exit trigger_price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("exit_order") && m.contains("trigger_price")),
        "expected non-triggered exit_order trigger_price rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_stop_market_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_market_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_market\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_market_strategy_source)
        .expect("StopMarket trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(&messages, "StopMarket entry order");
}

#[test]
fn bolt_v3_archetype_rejects_triggered_entry_order_with_trigger_instrument_id() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_market_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_market\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\ntrigger_instrument_id = \"TRIGGER.SOURCE\"\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_market_strategy_source)
        .expect("trigger_instrument_id should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(
        &messages,
        "triggered entry order with trigger_instrument_id",
    );
}

#[test]
fn bolt_v3_archetype_rejects_non_triggered_entry_order_with_trigger_instrument_id() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture.replacen(
        "time_in_force = \"fok\"\nis_post_only = false",
        "time_in_force = \"fok\"\ntrigger_instrument_id = \"TRIGGER.SOURCE\"\nis_post_only = false",
        1,
    );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("non-triggered trigger_instrument_id should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_instrument_id")),
        "expected non-triggered entry_order trigger_instrument_id rejection, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let market_if_touched_strategy_source = fixture
        .replace(
            "order_type = \"market\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&market_if_touched_strategy_source)
        .expect("MarketIfTouched trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(&messages, "MarketIfTouched entry order");
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_without_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_entry_order(&fixture, |entry_block| {
        entry_block
            .replace(
                "order_type = \"market\"",
                "order_type = \"market_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"gtc\"\nis_post_only = false",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched entry without trigger price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected MarketIfTouched entry_order rejection requiring trigger_price, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_with_non_positive_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    for trigger_price in ["0.0", "-0.01"] {
        let strategy_source = fixture
            .replace(
                "order_type = \"market\"",
                "order_type = \"market_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                &format!(
                    "time_in_force = \"gtc\"\ntrigger_price = {trigger_price}\nis_post_only = false"
                ),
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source).expect(
            "MarketIfTouched entry with non-positive trigger price should parse typed order config",
        );
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
            "expected MarketIfTouched entry_order rejection for trigger_price={trigger_price}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_gtd_entry_without_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"market\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtd\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched GTD entry without expiry should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("expire_time_unix_nanos")),
        "expected MarketIfTouched entry_order GTD rejection requiring expiry, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_market_if_touched_entry_post_only() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let strategy_source = fixture
        .replace(
            "order_type = \"limit\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = true",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched post-only case should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("is_post_only")),
        "expected MarketIfTouched entry_order rejection for post-only, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_accepts_market_if_touched_exit_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
        .replace(
            "order_type = \"market\"",
            "order_type = \"market_if_touched\"",
        )
        .replace(
            "time_in_force = \"ioc\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.48\ntrigger_type = \"mark_price\"\nis_post_only = false",
        )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("MarketIfTouched exit order should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "MarketIfTouched exit order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_trailing_stop_market_entry_with_explicit_trailing_fields() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"limit\"",
            "order_type = \"trailing_stop_market\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\ntrailing_offset = 2.5\ntrailing_offset_type = \"basis_points\"\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("TrailingStopMarket entry should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(
        &messages,
        "TrailingStopMarket entry order with explicit trailing fields",
    );
}

#[test]
fn bolt_v3_archetype_rejects_trailing_stop_market_entry_with_nt_default_fields() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"limit\"",
            "order_type = \"trailing_stop_market\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrailing_offset = 2.5\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("TrailingStopMarket entry should parse NT-defaulted order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(
        &messages,
        "TrailingStopMarket entry order with NT-defaulted fields",
    );
}

#[test]
fn bolt_v3_archetype_accepts_trailing_stop_market_exit_with_activation_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
        .replace(
            "order_type = \"market\"",
            "order_type = \"trailing_stop_market\"",
        )
        .replace(
            "time_in_force = \"ioc\"\nis_post_only = false",
            "time_in_force = \"gtc\"\nactivation_price = 0.48\ntrigger_type = \"mark_price\"\ntrailing_offset = 3.0\ntrailing_offset_type = \"ticks\"\nis_post_only = false",
        )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("TrailingStopMarket exit should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "TrailingStopMarket exit order with explicit trailing fields should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_trailing_stop_market_invalid_combinations() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    for (case_name, order_fields, expected_field) in [
        (
            "missing_trailing_offset",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\ntrailing_offset_type = \"price\"\nis_post_only = false",
            "trailing_offset",
        ),
        (
            "missing_trigger_or_activation",
            "time_in_force = \"gtc\"\ntrigger_type = \"last_price\"\ntrailing_offset = 1.0\ntrailing_offset_type = \"price\"\nis_post_only = false",
            "trigger_price",
        ),
        (
            "non_positive_trigger_with_activation",
            "time_in_force = \"gtc\"\ntrigger_price = -0.01\nactivation_price = 0.48\ntrigger_type = \"last_price\"\ntrailing_offset = 1.0\ntrailing_offset_type = \"price\"\nis_post_only = false",
            "trigger_price",
        ),
        (
            "non_positive_activation_with_trigger",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nactivation_price = 0.0\ntrigger_type = \"last_price\"\ntrailing_offset = 1.0\ntrailing_offset_type = \"price\"\nis_post_only = false",
            "activation_price",
        ),
        (
            "gtd_without_expiry",
            "time_in_force = \"gtd\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\ntrailing_offset = 1.0\ntrailing_offset_type = \"price\"\nis_post_only = false",
            "expire_time_unix_nanos",
        ),
        (
            "is_post_only",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\ntrailing_offset = 1.0\ntrailing_offset_type = \"price\"\nis_post_only = true",
            "is_post_only",
        ),
    ] {
        let strategy_source = fixture
            .replace(
                "order_type = \"market\"",
                "order_type = \"trailing_stop_market\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                order_fields,
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
            .unwrap_or_else(|error| panic!("{case_name} should parse typed order config: {error}"));
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains(expected_field)),
            "expected TrailingStopMarket {case_name} rejection for {expected_field}, got: {messages:#?}"
        );
    }

    for trailing_offset in ["0.0", "-0.01"] {
        let strategy_source = fixture
            .replace(
                "order_type = \"market\"",
                "order_type = \"trailing_stop_market\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                &format!(
                    "time_in_force = \"gtc\"\ntrigger_price = 0.52\ntrigger_type = \"last_price\"\ntrailing_offset = {trailing_offset}\ntrailing_offset_type = \"price\"\nis_post_only = false"
                ),
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
            .expect("TrailingStopMarket non-positive trailing offset should parse typed config");
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains("trailing_offset")),
            "expected TrailingStopMarket rejection for trailing_offset={trailing_offset}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = fixture
        .replace("order_type = \"limit\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = true",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(&messages, "StopLimit entry order");
}

#[test]
fn bolt_v3_archetype_accepts_stop_limit_exit_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
            .replace("order_type = \"market\"", "order_type = \"stop_limit\"")
            .replace(
                "time_in_force = \"ioc\"\nis_post_only = false",
                "time_in_force = \"gtc\"\ntrigger_price = 0.48\nis_post_only = true",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit exit trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "StopLimit exit order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_limit_if_touched_entry_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"market\"",
            "order_type = \"limit_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\ntrigger_price = 0.39\nis_post_only = true",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("LimitIfTouched entry trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert_binary_oracle_entry_order_shape_rejected(&messages, "LimitIfTouched entry order");
}

#[test]
fn bolt_v3_archetype_accepts_limit_if_touched_exit_with_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block
            .replace(
                "order_type = \"market\"",
                "order_type = \"limit_if_touched\"",
            )
            .replace(
                "time_in_force = \"ioc\"\nis_post_only = false",
                "time_in_force = \"gtc\"\ntrigger_price = 0.46\nis_post_only = true",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("LimitIfTouched exit trigger price should parse through typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.is_empty(),
        "LimitIfTouched exit order with explicit trigger price should validate: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_limit_if_touched_entry_without_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_entry_order(&fixture, |entry_block| {
        entry_block
            .replace(
                "order_type = \"market\"",
                "order_type = \"limit_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"gtc\"\nis_post_only = false",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("LimitIfTouched entry without trigger price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected LimitIfTouched entry_order rejection requiring trigger_price, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_limit_if_touched_entry_with_non_positive_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    for trigger_price in ["0.0", "-0.01"] {
        let strategy_source = fixture
            .replace(
                "order_type = \"market\"",
                "order_type = \"limit_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                &format!(
                    "time_in_force = \"gtc\"\ntrigger_price = {trigger_price}\nis_post_only = false"
                ),
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source).expect(
            "LimitIfTouched entry with non-positive trigger price should parse typed order config",
        );
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
            "expected LimitIfTouched entry_order rejection for trigger_price={trigger_price}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_limit_if_touched_gtd_entry_without_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = fixture
        .replace(
            "order_type = \"market\"",
            "order_type = \"limit_if_touched\"",
        )
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtd\"\ntrigger_price = 0.39\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("LimitIfTouched GTD entry without expiry should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("expire_time_unix_nanos")),
        "expected LimitIfTouched entry_order GTD rejection requiring expiry, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_limit_if_touched_entry_quote_quantity() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let strategy_source = mutate_parameters_entry_order(&fixture, |entry_block| {
        entry_block
            .replace(
                "order_type = \"market\"",
                "order_type = \"limit_if_touched\"",
            )
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"gtc\"\ntrigger_price = 0.39\nis_post_only = false",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("LimitIfTouched boolean flag case should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    // Lane 1 only enables market/FOK quote-quantity entries; non-market quote-quantity
    // templates remain outside the executable entry shape.
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.entry_order")
                && validation_message_has_code(
                    message,
                    BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE,
                )
        }),
        "LimitIfTouched quote-quantity entry must fail closed at load via executable shape: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_exit_quote_quantity_until_exit_quote_sizing_exists() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let strategy_source = mutate_parameters_exit_order(&fixture, |exit_block| {
        exit_block.replacen("is_quote_quantity = false", "is_quote_quantity = true", 1)
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&strategy_source)
        .expect("exit quote-quantity config should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages.iter().any(|message| {
            message.contains("exit_order")
                && message.contains("is_quote_quantity")
                && message.contains("base position")
        }),
        "exit quote quantity should be rejected until exit quote sizing exists: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_limit_if_touched_exit_invalid_combinations() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let mut cases = vec![
        (
            "missing trigger",
            mutate_parameters_exit_order(&fixture, |exit_block| {
                exit_block
                    .replace(
                        "order_type = \"market\"",
                        "order_type = \"limit_if_touched\"",
                    )
                    .replace(
                        "time_in_force = \"ioc\"\nis_post_only = false",
                        "time_in_force = \"gtc\"\nis_post_only = false",
                    )
            }),
            "trigger_price",
        ),
        (
            "GTD without expiry",
            mutate_parameters_exit_order(&fixture, |exit_block| {
                exit_block
                    .replace(
                        "order_type = \"market\"",
                        "order_type = \"limit_if_touched\"",
                    )
                    .replace(
                        "time_in_force = \"ioc\"\nis_post_only = false",
                        "time_in_force = \"gtd\"\ntrigger_price = 0.46\nis_post_only = false",
                    )
            }),
            "expire_time_unix_nanos",
        ),
    ];
    for trigger_price in ["0.0", "-0.01"] {
        cases.push((
            "non-positive trigger",
            mutate_parameters_exit_order(&fixture, |exit_block| {
                exit_block
                .replace(
                    "order_type = \"market\"",
                    "order_type = \"limit_if_touched\"",
                )
                .replace(
                    "time_in_force = \"ioc\"\nis_post_only = false",
                    &format!(
                        "time_in_force = \"gtc\"\ntrigger_price = {trigger_price}\nis_post_only = false"
                    ),
                )
            }),
            "trigger_price",
        ));
    }
    for (case, exit_source, expected_field) in cases {
        let strategy: BoltV3StrategyConfig = toml::from_str(&exit_source)
            .expect("LimitIfTouched invalid exit case should parse typed order config");
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("exit_order") && m.contains(expected_field)),
            "expected LimitIfTouched exit_order rejection for {case}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_without_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = fixture
        .replace("order_type = \"market\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtc\"\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit entry without trigger price should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
        "expected StopLimit entry_order rejection requiring trigger_price, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_with_non_positive_trigger_price() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    for trigger_price in ["0.0", "-0.01"] {
        let stop_limit_strategy_source = fixture
            .replace("order_type = \"market\"", "order_type = \"stop_limit\"")
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                &format!(
                    "time_in_force = \"gtc\"\ntrigger_price = {trigger_price}\nis_post_only = false"
                ),
            );

        let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source).expect(
            "StopLimit entry with non-positive trigger price should parse typed order config",
        );
        let loaded = vec![LoadedStrategy {
            config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy,
        }];
        let messages = validate_strategies(&stable_root, &loaded);

        assert!(
            messages
                .iter()
                .any(|m| m.contains("entry_order") && m.contains("trigger_price")),
            "expected StopLimit entry_order rejection for trigger_price={trigger_price}, got: {messages:#?}"
        );
    }
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_gtd_entry_without_expiry() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let stop_limit_strategy_source = fixture
        .replace("order_type = \"market\"", "order_type = \"stop_limit\"")
        .replace(
            "time_in_force = \"fok\"\nis_post_only = false",
            "time_in_force = \"gtd\"\ntrigger_price = 0.52\nis_post_only = false",
        );

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit GTD entry without expiry should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    assert!(
        messages
            .iter()
            .any(|m| m.contains("entry_order") && m.contains("expire_time_unix_nanos")),
        "expected StopLimit entry_order GTD rejection requiring expiry, got: {messages:#?}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_stop_limit_entry_quote_quantity() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture should be readable"),
    )
    .expect("stable root should parse");
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let stop_limit_strategy_source = mutate_parameters_entry_order(&fixture, |entry_block| {
        entry_block
            .replace("order_type = \"market\"", "order_type = \"stop_limit\"")
            .replace(
                "time_in_force = \"fok\"\nis_post_only = false",
                "time_in_force = \"gtc\"\ntrigger_price = 0.52\nis_post_only = false",
            )
    });

    let strategy: BoltV3StrategyConfig = toml::from_str(&stop_limit_strategy_source)
        .expect("StopLimit boolean flag case should parse typed order config");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let messages = validate_strategies(&stable_root, &loaded);

    // Lane 1 only enables market/FOK quote-quantity entries; non-market quote-quantity
    // templates remain outside the executable entry shape.
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.entry_order")
                && validation_message_has_code(
                    message,
                    BINARY_ORACLE_ENTRY_ORDER_UNSUPPORTED_SHAPE_CODE,
                )
        }),
        "StopLimit quote-quantity entry must fail closed at load via executable shape: {messages:#?}"
    );
}

#[test]
fn parses_minimal_bolt_v3_root_and_strategy_config() {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;
    use bolt_v2::bolt_v3_market_families::updown::{TargetBlock, TargetKind};
    use bolt_v2::strategies::binary_oracle_edge_taker::archetype::ParametersBlock;
    use nautilus_common::enums::Environment;
    use nautilus_model::enums::{OrderType, TimeInForce};

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("minimal v3 config should load");

    assert_eq!(loaded.root.schema_version, 2);
    assert_eq!(
        loaded.root.trader_id,
        nautilus_model::identifiers::TraderId::from("BOLT-001")
    );
    assert_eq!(loaded.root.runtime.mode, Environment::Live);
    assert_eq!(
        loaded.root.clients["polymarket_main"].venue.as_str(),
        "POLYMARKET"
    );
    assert!(loaded.root.clients["polymarket_main"].execution.is_some());
    assert!(!loaded.root.clients.contains_key("binance_reference"));

    assert_eq!(loaded.strategies.len(), 1);
    let strategy = &loaded.strategies[0].config;
    assert_eq!(
        strategy.strategy_archetype.as_str(),
        "binary_oracle_edge_taker"
    );
    let target: TargetBlock = strategy
        .target
        .clone()
        .try_into()
        .expect("fixture target block should deserialize as updown TargetBlock");
    assert_eq!(target.kind, TargetKind::RotatingMarket);
    assert_eq!(target.cadence_secs, 300);
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .expect("fixture parameters block should deserialize as binary_oracle_edge_taker");
    assert_eq!(parameters.entry_order.order_type, OrderType::Market);
    assert_eq!(parameters.entry_order.time_in_force, TimeInForce::Fok);
    assert!(parameters.entry_order.is_quote_quantity);
    assert_eq!(parameters.exit_order.order_type, OrderType::Market);
    assert_eq!(parameters.exit_order.time_in_force, TimeInForce::Ioc);
    assert!(!strategy.signal_data.is_empty());
}

#[test]
fn rejects_unknown_bolt_v3_config_fields() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = fixture.replace(
        "schema_version = 2",
        "schema_version = 2\nunexpected_root_field = \"nope\"",
    );

    let error = toml::from_str::<BoltV3RootConfig>(&mutated)
        .expect_err("unknown root field should fail to parse")
        .to_string();
    assert!(
        error.contains("unexpected_root_field"),
        "error should name the unknown field, got: {error}"
    );

    let mutated_strategy = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace(
        "[parameters]\nedge_threshold_basis_points = 100",
        "[parameters]\nedge_threshold_basis_points = 100\nbogus_parameter = 7",
    );

    // The strategy envelope's `parameters` field is now archetype-
    // neutral raw TOML (`toml::Value`); unknown-field rejection inside
    // `[parameters]` moves from envelope-parse time to archetype typed
    // deserialization time. The first parse therefore succeeds, but
    // `try_into::<ParametersBlock>` (the per-archetype deserializer)
    // still rejects the unknown field by name.
    let strategy: bolt_v2::bolt_v3_config::BoltV3StrategyConfig = toml::from_str(&mutated_strategy)
        .expect(
            "strategy envelope parse should succeed when parameters is archetype-neutral raw TOML",
        );
    let parameters_error = strategy
        .parameters
        .try_into::<bolt_v2::strategies::binary_oracle_edge_taker::archetype::ParametersBlock>()
        .expect_err("unknown field inside [parameters] should fail archetype typed deserialization")
        .to_string();
    assert!(
        parameters_error.contains("bogus_parameter"),
        "archetype deserialization error should name the unknown strategy field, got: {parameters_error}"
    );
}

#[test]
fn accepts_canonical_gate_provider_root_blocks() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 4
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-reference-price"
value_kind = "price"
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8

[gate_providers.venue_metadata_primary]
provider_kind = "hyperliquid_hip4"
capabilities = ["market_metadata", "reference_value"]

[gate_providers.venue_metadata_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.venue_metadata_primary.hyperliquid_hip4]
metadata_scope = "asset_universe"
"#,
    ))
    .expect("canonical gate provider blocks should parse");

    let messages = validate_root_only(&root);
    assert!(
        messages.is_empty(),
        "canonical gate provider blocks should validate: {messages:#?}"
    );
}

#[test]
fn rejects_chainlink_data_streams_provider_level_feed_fields() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8
endpoint_id = "testnet-data-streams"
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
    ))
    .expect("legacy provider-level feed fields should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary.chainlink_data_streams")
                && message.contains("feed_bindings")
        }),
        "provider-level Chainlink feed fields should fail with feed_bindings guidance: {messages:#?}"
    );
}

#[test]
fn rejects_chainlink_data_streams_unknown_provider_field() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 4
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
unowned_connection_field = "must-fail"

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-reference-price"
value_kind = "price"
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8
"#,
    ))
    .expect("unknown Chainlink provider fields should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary.chainlink_data_streams")
                && message.contains("unowned_connection_field")
        }),
        "unknown Chainlink provider fields should fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_chainlink_data_streams_missing_rest_request_fields() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 4
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-reference-price"
value_kind = "price"
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8
"#,
    ))
    .expect("missing Chainlink request fields should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("rest_base_url")),
        "missing Chainlink rest_base_url should fail closed: {messages:#?}"
    );
}

/// A live `CHAINLINK_DATA_STREAMS` client whose connection config matches the
/// fixture's `resolution_oracle_primary` gate provider exactly. Injected before
/// `[clients.polymarket_main]` (re-appended at the end so the fixture stays
/// well-formed).
#[test]
fn chainlink_client_matching_gate_provider_connection_passes_single_source_guard() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // The fixture ships a live `chainlink_strike` strike client AND the chainlink
    // resolution-oracle gate provider. Their shared connection config matches, so
    // the single-source drift guard must stay quiet.
    let source = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("shipped chainlink client fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("must match gate_providers")),
        "a chainlink client matching the gate provider must not raise a single-source drift error: {messages:#?}"
    );
}

#[test]
fn chainlink_client_diverging_from_gate_provider_fails_single_source_guard() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Mutate ONLY the shipped strike client's REST endpoint so it diverges from
    // the gate provider's. The live strike path and the resolution-oracle
    // definition must reference one source, so this fails closed at config load.
    // The section-scoped replace ensures the gate provider's matching URL line is
    // left untouched.
    let mutated = replace_in_fixture_section(
        "[clients.chainlink_strike.data]",
        &[(
            "rest_base_url = \"https://api.testnet-dataengine.chain.link\"",
            "rest_base_url = \"https://api.divergent-dataengine.chain.link\"",
        )],
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("diverging chainlink client fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("clients.chainlink_strike.data.rest_base_url")
                && message.contains("must match gate_providers")
        }),
        "a chainlink client whose rest_base_url diverges from the gate provider must fail closed: {messages:#?}"
    );
}

#[test]
fn chainlink_client_http_timeout_diverging_from_gate_provider_fails_single_source_guard() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Mutate ONLY the shipped strike client's http_timeout_secs so it diverges
    // from the gate provider's. This exercises the integer comparison branch
    // (a different code path from the string fields), which the rest_base_url
    // test does not cover. The section-scoped replace leaves the gate
    // provider's matching value untouched.
    let mutated = replace_in_fixture_section(
        "[clients.chainlink_strike.data]",
        &[("http_timeout_secs = 4", "http_timeout_secs = 6")],
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("diverging chainlink client fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("clients.chainlink_strike.data.http_timeout_secs")
                && message.contains("must match gate_providers")
        }),
        "a chainlink client whose http_timeout_secs diverges from the gate provider must fail closed: {messages:#?}"
    );
}

#[test]
fn chainlink_client_api_key_ssm_diverging_from_gate_provider_fails_single_source_guard() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Mutate ONLY the shipped strike client's api_key SSM parameter path so the
    // live strike credentials diverge from the resolution-oracle gate
    // provider's. A split credential source must fail closed at config load —
    // the single-source guard's secrets-comparison branch, untested before.
    let mutated = replace_in_fixture_section(
        "[clients.chainlink_strike.secrets]",
        &[(
            "api_key_ssm_parameter = \"/bolt/testnet/chainlink/api-key\"",
            "api_key_ssm_parameter = \"/bolt/testnet/chainlink/api-key-divergent\"",
        )],
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("diverging chainlink client fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("clients.chainlink_strike.secrets.api_key_ssm_parameter")
                && message.contains("must match gate_providers")
        }),
        "a chainlink client whose api_key_ssm_parameter diverges from the gate provider must fail closed: {messages:#?}"
    );
}

#[test]
fn chainlink_gate_provider_non_https_rest_base_url_fails_closed() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // The resolution-oracle gate provider names the SAME Chainlink Data Streams
    // endpoint as the live strike client and must be held to the same transport
    // standard. A gate provider declaring an http:// rest_base_url must fail
    // closed on https grounds at config load — signed credentials must never
    // traverse plaintext regardless of which config block names the endpoint.
    // This is the gate-side companion to the client-side https check; before the
    // shared validator was wired into the gate validator, only a client/gate
    // mismatch fired and the gate's own transport was unchecked.
    let mutated = replace_in_fixture_section(
        "[gate_providers.resolution_oracle_primary.chainlink_data_streams]",
        &[(
            "rest_base_url = \"https://api.testnet-dataengine.chain.link\"",
            "rest_base_url = \"http://api.testnet-dataengine.chain.link\"",
        )],
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("gate provider http fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains(
                "gate_providers.resolution_oracle_primary.chainlink_data_streams.rest_base_url",
            ) && message.contains("https scheme")
        }),
        "a gate provider whose rest_base_url is not https must fail closed on https grounds: {messages:#?}"
    );
}

#[test]
fn chainlink_client_scheme_relative_report_endpoint_path_fails_closed() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // A `//host` report_endpoint_path is scheme-relative: joined against the base
    // it swaps the host while keeping https, redirecting the HMAC-signed strike
    // request off the configured endpoint. The non-empty check alone never caught
    // this; config load must fail closed on the endpoint-path resolver.
    let mutated = replace_in_fixture_section(
        "[clients.chainlink_strike.data]",
        &[(
            "report_endpoint_path = \"/api/v1/reports\"",
            "report_endpoint_path = \"//attacker.example/api/v1/reports\"",
        )],
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("scheme-relative endpoint fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("clients.chainlink_strike.data.report_endpoint_path")
                && message.contains("scheme-relative or authority reference")
        }),
        "a chainlink client report_endpoint_path that changes host must fail closed: {messages:#?}"
    );
}

#[test]
fn chainlink_gate_provider_scheme_relative_report_endpoint_path_fails_closed() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // The gate provider names the same endpoint and must be held to the same
    // resolver. Its prior check accepted `//host` (it starts with `/` and has no
    // `?`), so a scheme-relative path could redirect the resolution-oracle proof
    // request off-host. Config load must fail closed.
    let mutated = replace_in_fixture_section(
        "[gate_providers.resolution_oracle_primary.chainlink_data_streams]",
        &[(
            "report_endpoint_path = \"/api/v1/reports\"",
            "report_endpoint_path = \"//attacker.example/api/v1/reports\"",
        )],
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("scheme-relative gate endpoint fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains(
                "gate_providers.resolution_oracle_primary.chainlink_data_streams.report_endpoint_path",
            ) && message.contains("scheme-relative or authority reference")
        }),
        "a gate provider report_endpoint_path that changes host must fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_gate_provider_client_id_without_configured_client() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]
client_id = "missing_chainlink_client"

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 4
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-reference-price"
value_kind = "price"
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8
"#,
    ))
    .expect("dangling gate provider client_id should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary.client_id")
                && message.contains("missing_chainlink_client")
        }),
        "dangling gate provider client_id should fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_chainlink_data_streams_json_credential_parameter_shape() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
ssm_credential_parameter = "/bolt/gate-providers/chainlink/testnet"

[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "configured-reference-price"
value_kind = "price"
feed_id = "0x0000000000000000000000000000000000000000000000000000000000000000"
report_schema_version = 3
report_decimal_scale = 8
"#,
    ))
    .expect("legacy Chainlink credential shape should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("ssm_credential_parameter")
                && message.contains("api_key_ssm_parameter")
                && message.contains("api_secret_ssm_parameter")
        }),
        "legacy Chainlink JSON credential parameter should fail with migration guidance: {messages:#?}"
    );
}

#[test]
fn rejects_gate_provider_fields_under_strategy_runtime() {
    let strategy_toml = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace(
        "[parameters.runtime]\n",
        "[parameters.runtime]\nchainlink_data_streams_feed_id = \"0x0000000000000000000000000000000000000000000000000000000000000000\"\n",
    );

    let strategy: bolt_v2::bolt_v3_config::BoltV3StrategyConfig = toml::from_str(&strategy_toml)
        .expect("strategy envelope parse should keep parameters archetype-neutral");
    let error = strategy
        .parameters
        .try_into::<bolt_v2::strategies::binary_oracle_edge_taker::archetype::ParametersBlock>()
        .expect_err("gate provider source fields under [parameters.runtime] must be rejected")
        .to_string();

    assert!(
        error.contains("parameters.runtime.chainlink_data_streams_feed_id")
            && error.contains("[gate_providers.<id>.chainlink_data_streams]"),
        "runtime gate provider field rejection should point to the gate provider table, got: {error}"
    );
}

#[test]
fn bolt_v3_archetype_rejects_all_six_gate_coupled_runtime_fields() {
    // Class regression lock (Codex/internal-review forbidden_fields gap). All SIX provider-coupled
    // fields are DECLARED in the RuntimeParametersBlock `Wire` struct, so `deny_unknown_fields`
    // does NOT reject them — each is rejected only by a dedicated `is_some()` guard in the custom
    // Deserialize impl (src/strategies/binary_oracle_edge_taker/archetype.rs:165-194). Before this
    // test only three of the six guards had injection coverage; the other three
    // (price_to_beat_report_schema_version, price_to_beat_report_decimal_scale,
    // forced_flat_stale_chainlink_ms) could be deleted while the field stayed a Wire field —
    // silently accepting-and-dropping it with `cargo test` green. Inject EACH field under
    // [parameters.runtime] and assert the deserializer fails closed naming the field path, so
    // deleting any one guard now fails this test. Values are arbitrary: the guard fires on
    // presence (`is_some()`), independent of value type.
    let base = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    let forbidden_fields: [(&str, &str); 6] = [
        (
            "price_to_beat_source",
            "\"chainlink_data_streams.report_at_boundary\"",
        ),
        (
            "price_to_beat_feed_id",
            "\"0x0000000000000000000000000000000000000000000000000000000000000000\"",
        ),
        ("price_to_beat_report_schema_version", "3"),
        ("price_to_beat_report_decimal_scale", "8"),
        ("forced_flat_stale_chainlink_ms", "1500"),
        (
            "chainlink_data_streams_feed_id",
            "\"0x0000000000000000000000000000000000000000000000000000000000000000\"",
        ),
    ];

    for (field, value) in forbidden_fields {
        let strategy_toml = base.replace(
            "[parameters.runtime]\n",
            &format!("[parameters.runtime]\n{field} = {value}\n"),
        );
        assert!(
            strategy_toml.contains(&format!("{field} = {value}")),
            "fixture must expose a [parameters.runtime] table to inject {field} into",
        );

        let strategy: bolt_v2::bolt_v3_config::BoltV3StrategyConfig =
            toml::from_str(&strategy_toml)
                .expect("strategy envelope parse should keep parameters archetype-neutral");
        // A SUCCESSFUL deserialize here is the regression this test guards against (a deleted or
        // weakened `is_some()` guard), so the Ok branch fails loudly; the Err branch is the
        // fail-closed behavior and must name the offending `parameters.runtime.<field>` path.
        let error = match strategy
            .parameters
            .try_into::<bolt_v2::strategies::binary_oracle_edge_taker::archetype::ParametersBlock>(
        ) {
            Ok(_) => panic!(
                "provider-coupled runtime field {field} was ACCEPTED at deserialize \
                 (is_some() guard missing or deleted) — fail-closed rejection lost"
            ),
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains(&format!("parameters.runtime.{field}")),
            "rejection for {field} must name the field path parameters.runtime.{field}, got: {error}"
        );
    }
}

#[test]
fn rejects_gate_provider_fields_under_wrong_provider_subtable() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.hyperliquid_hip4]
metadata_scope = "asset_universe"
"#,
    ))
    .expect("wrong provider subtable should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary")
                && message.contains("provider_kind `chainlink_data_streams`")
                && message
                    .contains("[gate_providers.resolution_oracle_primary.chainlink_data_streams]")
        }),
        "wrong provider subtable should be rejected with the matching subtable path: {messages:#?}"
    );
}

#[test]
fn rejects_unregistered_gate_provider_kind() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "made_up_oracle"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.made_up_oracle]
endpoint_id = "test"
"#,
    ))
    .expect("unregistered provider kind should parse before registry validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary.provider_kind")
                && message.contains("made_up_oracle")
                && message.contains("unregistered")
        }),
        "unregistered gate provider kind should fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_gate_provider_without_provider_kind() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
"#,
    ))
    .expect("missing provider_kind should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary.provider_kind")
                && message.contains("required")
        }),
        "gate provider without provider_kind should fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_gate_provider_with_empty_capabilities() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = []

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
"#,
    ))
    .expect("empty capabilities should parse before semantic validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary.capabilities")
                && message.contains("one or more")
        }),
        "gate provider with empty capabilities should fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_test_double_gate_provider_in_operator_root() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&fixture_root_with_gate_providers(
        r#"
[gate_providers.fixture_resolution]
provider_kind = "test_double"
capabilities = ["resolution_value"]

[gate_providers.fixture_resolution.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.fixture_resolution.test_double]
fixture_sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
"#,
    ))
    .expect("test_double provider should parse before live/local operator validation");

    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.fixture_resolution.provider_kind")
                && message.contains("test_double")
                && message.contains("live/local operator TOML")
        }),
        "test_double gate providers must be rejected outside tests: {messages:#?}"
    );
}

#[test]
fn accepts_canonical_target_gate_subscription() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = ["chainlink_data_streams", "pyth", "exchange_index", "venue_native", "hyperliquid_hip4", "deribit_index", "outcome_oracle"]
allowed_value_kinds = ["price", "index", "outcome", "metadata"]
provider_preference = ["resolution_oracle_primary"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
provider_id = "resolution_oracle_primary"
"#,
    );

    assert!(
        messages.is_empty(),
        "canonical target gate subscription should validate: {messages:#?}"
    );
}

#[test]
fn accepts_no_resolution_target_gate_mapping_with_provider_kinds() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = ["chainlink_data_streams"]
allowed_value_kinds = ["price", "none"]
provider_preference = ["resolution_oracle_primary"]
allow_no_resolution = true

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
provider_id = "resolution_oracle_primary"

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "no_resolution"
resolution_identity = "none"
value_kind = "none"
"#,
    );

    assert!(
        messages.is_empty(),
        "valid no-resolution mapping should validate even when provider kinds are listed: {messages:#?}"
    );
}

#[test]
fn rejects_provider_capability_as_target_gate_role() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.market_metadata]
required = true
allowed_provider_kinds = ["venue_native", "hyperliquid_hip4"]
allowed_value_kinds = ["metadata"]
allow_no_resolution = false
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.market_metadata")
                && message.contains("provider capability")
                && message.contains("GateRole")
        }),
        "market_metadata capability must not be accepted as a gate role: {messages:#?}"
    );
}

#[test]
fn rejects_ambiguous_target_gate_market_mappings() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_ids = ["resolution_oracle_primary", "backup_resolution_oracle"]
allowed_provider_kinds = ["chainlink_data_streams"]
allowed_value_kinds = ["price"]
provider_preference = ["resolution_oracle_primary", "backup_resolution_oracle"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
provider_id = "resolution_oracle_primary"

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
provider_id = "backup_resolution_oracle"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.resolution.market_mappings")
                && message.contains("ambiguous")
        }),
        "duplicate market mappings should fail closed as ambiguous: {messages:#?}"
    );
}

#[test]
fn rejects_static_single_provider_subscription_for_rotating_market() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_ids = ["resolution_oracle_primary"]
allowed_value_kinds = ["price"]
allow_no_resolution = false
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.resolution")
                && message.contains("single static provider")
                && message.contains("rotating")
        }),
        "rotating markets must not collapse to a single static provider assumption: {messages:#?}"
    );
}

#[test]
fn rejects_multiple_matching_target_gate_providers_without_preference() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_ids = ["resolution_oracle_primary", "backup_resolution_oracle"]
allowed_provider_kinds = ["chainlink_data_streams"]
allowed_value_kinds = ["price"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.resolution.provider_preference")
                && message.contains("multiple")
                && message.contains("providers")
        }),
        "multiple matching providers require deterministic provider_preference: {messages:#?}"
    );
}

#[test]
fn rejects_target_gate_provider_kind_and_value_kind_mismatch() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = ["hyperliquid_hip4"]
allowed_value_kinds = ["price"]
provider_preference = ["venue_metadata_primary"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "metadata"
provider_id = "venue_metadata_primary"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.resolution")
                && message.contains("resolution_kind")
                && message.contains("allowed_provider_kinds")
                && message.contains("value_kind")
                && message.contains("allowed_value_kinds")
        }),
        "provider-kind/value-kind mismatches must fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_target_gate_mapping_without_allowed_provider_kinds() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_value_kinds = ["price"]
provider_preference = ["resolution_oracle_primary"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "configured-reference-price"
value_kind = "price"
provider_id = "resolution_oracle_primary"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.resolution.allowed_provider_kinds")
                && message.contains("provider-backed")
        }),
        "provider-backed mappings must not accept an unbounded provider-kind set: {messages:#?}"
    );
}

#[test]
fn rejects_invalid_no_resolution_target_gate_usage() {
    let messages = target_gate_subscription_messages(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = ["chainlink_data_streams"]
allowed_value_kinds = ["price"]
allow_no_resolution = true

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "no_resolution"
resolution_identity = "none"
value_kind = "price"
provider_id = "resolution_oracle_primary"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("target.gate_subscriptions.resolution.allow_no_resolution")
                && message.contains("no_resolution")
                && message.contains("value_kind")
        }),
        "invalid no-resolution policy should fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_chainlink_target_mapping_without_matching_feed_binding() {
    let root_toml = root_with_single_chainlink_feed_binding(
        "configured-primary-resolution",
        CHAINLINK_TEST_FEED_ID_PRIMARY,
    );
    let strategy_toml = strategy_with_single_chainlink_mapping("configured-secondary-resolution");

    let messages =
        strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
    assert!(
        messages.iter().any(|message| {
            message.contains("strategy `strategies/binary_oracle.toml`")
                && message.contains("configured-secondary-resolution")
                && message.contains("feed_bindings")
                && message.contains("no matching")
        }),
        "Chainlink target mapping without a matching feed binding must fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_chainlink_target_mapping_without_resolvable_provider_id() {
    let root_toml = root_with_single_chainlink_feed_binding(
        "configured-primary-resolution",
        CHAINLINK_TEST_FEED_ID_PRIMARY,
    );
    let strategy_toml = strategy_with_single_chainlink_mapping("configured-primary-resolution")
        .replace(
            "provider_preference = [\"resolution_oracle_primary\"]\n",
            "",
        )
        .replace("provider_id = \"resolution_oracle_primary\"\n", "");

    let messages =
        strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
    assert!(
        messages.iter().any(|message| {
            message.contains("strategy `strategies/binary_oracle.toml`")
                && message.contains("configured-primary-resolution")
                && message.contains("provider_id")
                && message.contains("chainlink_data_streams")
        }),
        "Chainlink target mappings must resolve a concrete provider_id for feed bindings: {messages:#?}"
    );
}

#[test]
fn rejects_target_gate_reference_to_missing_root_provider_id() {
    let root_toml = root_with_single_chainlink_feed_binding(
        "configured-primary-resolution",
        CHAINLINK_TEST_FEED_ID_PRIMARY,
    );
    let strategy_toml = fixture_strategy_with_target_gate_subscriptions(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_ids = ["venue_metadata_primary"]
allowed_provider_kinds = ["hyperliquid_hip4"]
allowed_value_kinds = ["metadata"]
provider_preference = ["venue_metadata_primary"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "hyperliquid_hip4"
resolution_identity = "configured-market-metadata"
value_kind = "metadata"
provider_id = "venue_metadata_primary"
"#,
    );

    let messages =
        strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
    assert!(
        messages.iter().any(|message| {
            message.contains("strategy `strategies/binary_oracle.toml`")
                && message.contains("target.gate_subscriptions.resolution")
                && message.contains("venue_metadata_primary")
                && message.contains("gate_providers")
        }),
        "target gate provider ids must resolve to root gate_providers: {messages:#?}"
    );
}

#[test]
fn rejects_unreachable_chainlink_feed_binding() {
    let root_toml = root_with_chainlink_feed_bindings(&[
        (
            "configured-primary-resolution",
            CHAINLINK_TEST_FEED_ID_PRIMARY,
        ),
        (
            "configured-secondary-resolution",
            CHAINLINK_TEST_FEED_ID_SECONDARY,
        ),
    ]);
    let strategy_toml = strategy_with_single_chainlink_mapping("configured-primary-resolution");

    let messages =
        strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
    assert!(
        messages.iter().any(|message| {
            message.contains("gate_providers.resolution_oracle_primary")
                && message.contains("configured-secondary-resolution")
                && message.contains("feed_bindings")
                && message.contains("not referenced by any loaded strategy")
        }),
        "unreachable Chainlink feed bindings must fail closed: {messages:#?}"
    );
}

#[test]
fn accepts_alternate_chainlink_mapping_with_matching_feed_binding() {
    let root_toml = root_with_single_chainlink_feed_binding(
        "configured-secondary-resolution",
        CHAINLINK_TEST_FEED_ID_SECONDARY,
    );
    let strategy_toml = strategy_with_single_chainlink_mapping("configured-secondary-resolution");

    let messages =
        strategy_validation_messages_for_root_and_strategy_toml(&root_toml, &strategy_toml);
    assert!(
        messages.is_empty(),
        "matching alternate Chainlink mapping should validate without fallback assumptions: {messages:#?}"
    );
}

#[test]
fn binary_oracle_fixture_uses_gate_subscription_without_provider_specific_runtime_fields() {
    let source = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");

    assert_binary_oracle_strategy_source_uses_gate_schema("fixture", &source);
}

#[test]
fn canonical_binary_oracle_config_uses_gate_subscription_without_provider_specific_runtime_fields()
{
    for relative_path in SHIPPED_BINARY_ORACLE_STRATEGY_FILES {
        let source = std::fs::read_to_string(support::repo_path(relative_path))
            .expect("canonical strategy config should be readable");

        assert_binary_oracle_strategy_source_uses_gate_schema(relative_path, &source);
    }
}

#[test]
fn shipped_strategy_config_surface_uses_canonical_binary_oracle_path() {
    let canonical_root = support::repo_path("config/root.toml");
    let legacy_root = support::repo_path("config/root.example.toml");
    let legacy_strategy = support::repo_path("config/strategies/binary_oracle.example.toml");
    let placeholder_strategy = support::repo_path("config/strategies/binary_oracle.toml");
    assert!(
        canonical_root.exists(),
        "tracked root config should live at config/root.toml"
    );
    assert!(
        !legacy_root.exists(),
        "tracked root config should not keep an .example.toml twin"
    );
    assert!(
        !legacy_strategy.exists(),
        "tracked strategy config should not keep an .example.toml twin"
    );
    assert!(
        !placeholder_strategy.exists(),
        "the single-placeholder strategy file should be replaced by per-asset files"
    );

    let root = std::fs::read_to_string(&canonical_root).expect("root config should be readable");
    for relative_path in SHIPPED_BINARY_ORACLE_STRATEGY_FILES {
        assert!(
            support::repo_path(relative_path).exists(),
            "tracked per-asset strategy config should live at {relative_path}"
        );
    }
    for relative_path in TRACKED_PRODUCTION_BINARY_ORACLE_STRATEGY_FILES {
        // Strip the `config/` prefix to the root-relative `strategy_files` form.
        let root_relative = relative_path
            .strip_prefix("config/")
            .expect("shipped strategy path should be under config/");
        assert!(
            root.contains(&format!("\"{root_relative}\"")),
            "root config should load the tracked production strategy path {root_relative}"
        );
    }
    assert!(
        !root.contains("binary_oracle.example.toml"),
        "root config should not reference the legacy .example strategy path"
    );

    let justfile = std::fs::read_to_string(support::repo_path("justfile"))
        .expect("justfile should be readable");
    assert!(
        justfile.contains("live_profile := env_var_or_default('BOLT_LIVE_PROFILE', '')"),
        "live recipes should source the operator-selected profile ID (single source of truth over the base template, #768)"
    );
    assert!(
        justfile.contains("ERROR: set BOLT_LIVE_PROFILE to an opaque profile ID"),
        "live recipes must fail closed instead of using a venue/market/strategy profile default"
    );
    assert!(
        justfile.contains("--profile \"{{live_profile}}\" --config-root config"),
        "live recipes must pass an opaque profile ID plus structural config root to the binary"
    );
    assert!(
        support::repo_text("src/bolt_v3_prod_profile.rs")
            .contains("ProfileError::InvalidProfileId"),
        "generate/verify must reject path-shaped profile inputs for systemd/direct CLI callers"
    );
    assert!(
        !justfile.contains("config/profiles/<profile>.overlay.toml"),
        "live recipes must not teach operators to put paths in BOLT_LIVE_PROFILE"
    );
    assert!(
        !justfile.contains("--output {{live_runtime}}")
            && !justfile.contains("--deployed {{live_runtime}}"),
        "live recipes must derive config/live.toml instead of accepting output/deployed path overrides"
    );
    assert!(
        !justfile.contains("live_root := \"config/live.local.toml\""),
        "live recipes must no longer source the gitignored operator root"
    );
    assert!(
        !justfile.contains("live_root := \"config/profiles/prod-btc-5m.overlay.toml\""),
        "live recipes must not hardcode a BTC 5m production profile"
    );
    assert!(
        !justfile.contains("live_root_example"),
        "live recipes should not keep a second example-root path"
    );
}

#[test]
fn shipped_binary_oracle_configs_do_not_canonicalize_one_reference_market_or_venue() {
    let strategy_paths =
        SHIPPED_BINARY_ORACLE_STRATEGY_FILES
            .iter()
            .copied()
            .chain(std::iter::once(
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
            ));
    for relative_path in strategy_paths {
        let source =
            std::fs::read_to_string(support::repo_path(relative_path)).expect("source should read");
        let forbidden = "binance_reference";
        assert!(
            !source.contains(forbidden),
            "{relative_path} must not make `{forbidden}` a canonical strategy config"
        );
    }

    for relative_path in ["config/root.toml", "tests/fixtures/bolt_v3/root.toml"] {
        let source =
            std::fs::read_to_string(support::repo_path(relative_path)).expect("source should read");
        for forbidden in [
            "[clients.binance_reference]",
            "[clients.binance_reference.data]",
            "[clients.binance_reference.secrets]",
            "/bolt/binance_reference/",
            "https://1rpc.io/matic",
            "chain_id = 137",
            "0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB",
        ] {
            assert!(
                !source.contains(forbidden),
                "{relative_path} must not make `{forbidden}` a canonical root example"
            );
        }
    }
}

#[test]
fn shipped_binary_oracle_configs_carry_sizing_ev_reference_for_deploy() {
    for relative_path in SHIPPED_BINARY_ORACLE_STRATEGY_FILES {
        let source =
            std::fs::read_to_string(support::repo_path(relative_path)).expect("source should read");
        let document = toml::from_str::<toml::Value>(&source)
            .unwrap_or_else(|error| panic!("{relative_path} should parse: {error}"));
        let runtime = document
            .get("parameters")
            .and_then(|parameters| parameters.get("runtime"))
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| panic!("{relative_path} must define [parameters.runtime]"));
        let sizing_ev_reference_bps = runtime
            .get("sizing_ev_reference_bps")
            .and_then(toml::Value::as_integer)
            .unwrap_or_else(|| panic!("{relative_path} must ship runtime.sizing_ev_reference_bps"));
        assert!(
            (1..=10_000).contains(&sizing_ev_reference_bps),
            "{relative_path} must deploy with sizing_ev_reference_bps inside the load-validated range"
        );
    }
}

#[test]
fn binary_oracle_archetype_exposes_provider_neutral_gate_requirements() {
    use std::collections::BTreeSet;

    use bolt_v2::{
        bolt_v3_archetypes::{GateRole, GateValueKind},
        strategies::binary_oracle_edge_taker::archetype as binary_oracle_edge_taker,
    };

    let requirements = binary_oracle_edge_taker::gate_requirements();
    assert_eq!(requirements.len(), 1);

    let requirement = &requirements[0];
    assert_eq!(requirement.role, GateRole::Resolution);
    assert!(requirement.required);
    assert_eq!(
        requirement.accepted_value_kinds,
        BTreeSet::from([GateValueKind::Price, GateValueKind::Outcome])
    );
    assert!(!requirement.allow_no_resolution);
}

#[test]
fn rejects_forbidden_polymarket_env_vars_before_client_build() {
    use bolt_v2::{
        bolt_v3_config::load_bolt_v3_config,
        bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
    };

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    for forbidden in [
        "POLYMARKET_PK",
        "POLYMARKET_FUNDER",
        "POLYMARKET_API_KEY",
        "POLYMARKET_API_SECRET",
        "POLYMARKET_PASSPHRASE",
    ] {
        let result = build_bolt_v3_live_node_with(
            &loaded,
            |var| var == forbidden,
            support::fake_bolt_v3_resolver,
        );
        let error = result.expect_err("forbidden env var must block LiveNode build");
        match error {
            BoltV3LiveNodeError::ForbiddenEnv(report) => {
                assert_eq!(report.findings.len(), 1, "{report}");
                assert_eq!(report.findings[0].client_key, "polymarket_main");
                assert_eq!(report.findings[0].env_var, forbidden);
            }
            other => panic!("expected ForbiddenEnv error, got {other:?}"),
        }
    }
}

#[test]
fn rejects_polymarket_execution_client_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 2
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "Live"
order_execution_mode = "live"

[nautilus]
load_state = true
save_state = true
shutdown_on_error = false
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

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
external_clients = []
debug = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 30
open_check_lookback_mins = 0
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 30
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
max_order_submit_rate = "33/00:01:00"
max_order_modify_rate = "33/00:01:00"
max_notional_per_order = {}
debug = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50
data_client_readiness_probe_poll_interval_ms = 50

[persistence.decision_evidence]
machine_relative_path = "bolt-v3/decision-evidence/current/machine.jsonl"
observation_relative_path = "bolt-v3/decision-evidence/current/observation.jsonl"
retired_relative_paths = ["bolt-v3/decision-evidence/order-intents.jsonl"]
reject_episode_max_count = 4096
recovery_evidence_max_bytes = 1048576

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-2"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
transport_backend = "sockudo"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket-execution-only TOML should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[execution]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for polymarket execution client, got: {messages:#?}"
    );
    assert!(rendered.contains("Polymarket execution client"));
    assert!(!rendered.contains("Polymarket execution venue"));
    assert!(rendered.contains("(provider=POLYMARKET)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_binance_reference_client_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = fixture_root_with_binance_reference_client()
        .replace(&binance_reference_secrets_block(), "");

    let root: BoltV3RootConfig =
        toml::from_str(&toml_text).expect("binance-data-only TOML should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[data]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for binance reference client, got: {messages:#?}"
    );
    assert!(rendered.contains("Binance reference-data client"));
    assert!(!rendered.contains("Binance reference-data venue"));
    assert!(rendered.contains("(provider=BINANCE)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_binance_spot_json_websocket_endpoint_for_reference_quotes() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};
    use nautilus_binance::common::consts::BINANCE_SPOT_WS_URL;

    let mutated = replace_in_binance_reference_fixture(
        "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
        &format!("base_url_ws = \"{BINANCE_SPOT_WS_URL}\""),
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("json-websocket binance fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("base_url_ws")
            && m.contains("SBE")
            && m.contains("subscribe_quotes")),
        "expected Binance Spot SBE websocket validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_spot_json_websocket_endpoint_with_trailing_slash() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};
    use nautilus_binance::common::consts::BINANCE_SPOT_WS_URL;

    let json_endpoint_with_trailing_slash =
        format!("{}/", BINANCE_SPOT_WS_URL.trim_end_matches('/'));
    let mutated = replace_in_binance_reference_fixture(
        "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
        &format!("base_url_ws = \"{json_endpoint_with_trailing_slash}\""),
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("json-websocket binance fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("base_url_ws")
            && m.contains("SBE")
            && m.contains("subscribe_quotes")),
        "expected trailing-slash Binance Spot JSON websocket validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_spot_json_websocket_endpoint_with_plain_ws_scheme() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};
    use nautilus_binance::common::consts::BINANCE_SPOT_WS_URL;

    let plain_ws_json_endpoint = BINANCE_SPOT_WS_URL.replacen("wss://", "ws://", 1);
    let mutated = replace_in_binance_reference_fixture(
        "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
        &format!("base_url_ws = \"{plain_ws_json_endpoint}\""),
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("plain-ws json-websocket binance fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("base_url_ws")
            && m.contains("SBE")
            && m.contains("subscribe_quotes")),
        "expected plain-ws Binance Spot JSON websocket validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_spot_json_websocket_host_with_different_path() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_binance_reference_fixture(
        "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
        "base_url_ws = \"wss://stream.binance.com:9443/stream\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("json-websocket host binance fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("base_url_ws")
            && m.contains("SBE")
            && m.contains("subscribe_quotes")),
        "expected Binance Spot JSON websocket host validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_invalid_binance_reference_websocket_urls_before_nt_mapping() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let cases = [
        ("", "non-empty URL"),
        ("   ", "non-empty URL"),
        ("not a url", "valid Binance Spot WebSocket URL"),
        (
            "http://stream-sbe.binance.com/ws",
            "valid Binance Spot WebSocket URL",
        ),
        (
            "https://stream-sbe.binance.com/ws",
            "valid Binance Spot WebSocket URL",
        ),
        (
            "ws:stream-sbe.binance.com/ws",
            "valid Binance Spot WebSocket URL",
        ),
        (
            "wss:/stream-sbe.binance.com/ws",
            "valid Binance Spot WebSocket URL",
        ),
    ];
    for (value, expected) in cases {
        let mutated = replace_in_binance_reference_fixture(
            "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
            &format!("base_url_ws = \"{value}\""),
        );
        let root: BoltV3RootConfig = toml::from_str(&mutated)
            .expect("invalid-url binance fixture should still parse as TOML");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains("binance_reference")
                && m.contains("base_url_ws")
                && m.contains(expected)),
            "expected Binance websocket URL validation error containing {expected:?} for {value:?}, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_binance_futures_websocket_url_without_spot_sbe_guidance() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated =
        replace_in_binance_reference_fixture("product_type = \"spot\"", "product_type = \"usd_m\"")
            .replace(
                "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
                "base_url_ws = \"https://fstream.binance.com/market/ws\"",
            );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid futures websocket fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        rendered.contains("clients.binance_reference.data.base_url_ws")
            && rendered.contains("valid Binance WebSocket URL"),
        "expected generic Binance websocket URL validation error, got: {messages:#?}"
    );
    assert!(
        !rendered.contains("Spot") && !rendered.contains("SBE"),
        "futures websocket validation must not emit Spot/SBE guidance, got: {rendered}"
    );
}

#[test]
fn accepts_binance_spot_sbe_and_proxy_websocket_endpoints() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for value in [
        "wss://stream-sbe.binance.com/ws",
        "ws://binance-sbe-proxy.test.invalid/ws",
        "wss://binance-sbe-proxy.test.invalid/ws",
    ] {
        let mutated = replace_in_binance_reference_fixture(
            "base_url_ws = \"wss://stream-sbe.binance.com/ws\"",
            &format!("base_url_ws = \"{value}\""),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("sbe/proxy binance fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            !messages
                .iter()
                .any(|m| m.contains("binance_reference") && m.contains("base_url_ws")),
            "expected {value:?} to avoid Binance websocket endpoint validation errors, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_binance_execution_block_with_provider_vocabulary() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = fixture_root_with_binance_reference_client();
    let mutated = format!("{fixture}\n[clients.binance_reference.execution]\nnot_allowed = true\n");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("binance execution mutation should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[execution]")
            && m.contains("not allowed")),
        "expected Binance execution-block rejection, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=BINANCE)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_market_data_only_provider_execution_secrets_and_direct_credentials() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = format!(
        r#"{fixture}

[clients.bybit_data]
venue = "BYBIT"

[clients.bybit_data.data]
product_types = ["spot", "linear"]
environment = "testnet"
transport_backend = "sockudo"
ws_reconnect_delay_secs = 5
api_key = "not-from-ssm"

[clients.bybit_data.execution]
not_allowed = true

[clients.bybit_data.secrets]
api_key_ssm_path = "/bolt/bybit/api_key"
"#
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("bybit data-only mutation should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|message| message.contains("bybit_data")
            && message.contains("data-only")
            && message.contains("[execution]")),
        "expected data-only execution-block rejection, got: {messages:#?}"
    );
    assert!(
        messages.iter().any(|message| message.contains("bybit_data")
            && message.contains("data-only")
            && message.contains("[secrets]")),
        "expected data-only secrets-block rejection, got: {messages:#?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("bybit_data.data.api_key")
                && message.contains("SSM-backed [secrets] binding")),
        "expected direct credential-field rejection, got: {messages:#?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("bybit_data.data")
                && message.contains("NT BYBIT data-client config")
                && message.contains("unknown field `ws_reconnect_delay_secs`")),
        "expected unknown NT data-field rejection, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=BYBIT)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_polymarket_client_numeric_fields_at_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let toml_text = r#"
schema_version = 2
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "Live"
order_execution_mode = "live"

[nautilus]
load_state = true
save_state = true
shutdown_on_error = false
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

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
external_clients = []
debug = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 30
open_check_lookback_mins = 0
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 30
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
max_order_submit_rate = "33/00:01:00"
max_order_modify_rate = "33/00:01:00"
max_notional_per_order = {}
debug = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50
data_client_readiness_probe_poll_interval_ms = 50

[persistence.decision_evidence]
machine_relative_path = "bolt-v3/decision-evidence/current/machine.jsonl"
observation_relative_path = "bolt-v3/decision-evidence/current/observation.jsonl"
retired_relative_paths = ["bolt-v3/decision-evidence/order-intents.jsonl"]
reject_episode_max_count = 4096
recovery_evidence_max_bytes = 1048576

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-2"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_rtds = "wss://ws-live-data.polymarket.com"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 0
ws_timeout_secs = 0
subscribe_new_markets = false
drop_quotes_missing_side = true
new_market_fetch_max_concurrency = 8
auto_load_missing_instruments = false
auto_load_debounce_ms = 250
auto_load_max_retries = 12
auto_load_retry_delay_initial_secs = 5
auto_load_retry_delay_max_secs = 15
resolve_poll_enabled = false
resolve_poll_interval_secs = 30
resolve_poll_grace_secs = 10
resolve_poll_max_wait_secs = 1800
update_instruments_interval_mins = 0
ws_max_subscriptions = 0
transport_backend = "sockudo"

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 0
max_retries = 0
retry_delay_initial_ms = 0
retry_delay_max_ms = 0
transport_backend = "sockudo"

[clients.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket/private-key"
api_key_ssm_path = "/bolt/polymarket/api-key"
api_secret_ssm_path = "/bolt/polymarket/api-secret"
passphrase_ssm_path = "/bolt/polymarket/api-passphrase"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket bounds TOML should parse");
    let messages = validate_root_only(&root);
    let expected = [
        "clients.polymarket_main.data.http_timeout_secs must be a positive integer",
        "clients.polymarket_main.data.ws_timeout_secs must be a positive integer",
        "clients.polymarket_main.data.update_instruments_interval_mins must be a positive integer",
        "clients.polymarket_main.data.ws_max_subscriptions must be a positive integer",
        "clients.polymarket_main.execution.http_timeout_secs must be a positive integer",
        "clients.polymarket_main.execution.max_retries must be a positive integer",
        "clients.polymarket_main.execution.retry_delay_initial_ms must be a positive integer",
        "clients.polymarket_main.execution.retry_delay_max_ms must be a positive integer",
    ];
    for needle in expected {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_unsupported_root_and_strategy_schema_versions() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::{validate_root_only, validate_strategies},
    };

    let mutated_root =
        std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture should be readable")
            .replace("schema_version = 2", "schema_version = 1");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated_root).expect("mutated root should parse with raw u32");
    let root_messages = validate_root_only(&root);
    assert!(
        root_messages
            .iter()
            .any(|m| m.contains("root schema_version=1 is unsupported")),
        "expected unsupported root schema version, got: {root_messages:#?}"
    );

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture should be readable"),
    )
    .expect("stable root should parse");

    let mutated_strategy = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .replace("schema_version = 2", "schema_version = 7");
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&mutated_strategy).expect("mutated strategy should parse with raw u32");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let strategy_messages = validate_strategies(&stable_root, &loaded);
    assert!(
        strategy_messages
            .iter()
            .any(|m| m.contains("schema_version=7 is unsupported")),
        "expected unsupported strategy schema version, got: {strategy_messages:#?}"
    );
}

#[test]
fn rejects_previous_strategy_schema_version_after_forced_exit_order_schema_update() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture should be readable"),
    )
    .expect("stable root should parse");

    let mut strategy: BoltV3StrategyConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path(
            "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
        ))
        .expect("strategy fixture should be readable"),
    )
    .expect("strategy fixture should parse");
    strategy.schema_version = 1;

    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];
    let strategy_messages = validate_strategies(&stable_root, &loaded);
    assert!(
        strategy_messages
            .iter()
            .any(|m| m.contains("schema_version=1 is unsupported")),
        "expected previous strategy schema version to be rejected, got: {strategy_messages:#?}"
    );
}

#[test]
fn shipped_binary_oracle_config_uses_supported_strategy_schema_version() {
    use bolt_v2::{
        bolt_v3_config::BoltV3StrategyConfig, bolt_v3_validate::SUPPORTED_STRATEGY_SCHEMA_VERSION,
    };

    for relative_path in SHIPPED_BINARY_ORACLE_STRATEGY_FILES {
        let strategy: BoltV3StrategyConfig = toml::from_str(
            &std::fs::read_to_string(support::repo_path(relative_path))
                .expect("canonical strategy config should be readable"),
        )
        .expect("canonical strategy config should parse");

        assert_eq!(
            strategy.schema_version, SUPPORTED_STRATEGY_SCHEMA_VERSION,
            "{relative_path} should declare the supported strategy schema version"
        );
    }
}

#[test]
fn shipped_binary_oracle_configs_omit_cadence_slug_token_and_derive_it() {
    // The updown cadence_slug_token is 100% determined by cadence_secs, so the
    // shipped operator configs OMIT it and rely on the shared derivation seam --
    // the redundant token lives nowhere in production config. This guards that
    // single-source contract end to end: every shipped config (a) carries no raw
    // cadence_slug_token, and (b) derives the contract token ("5m" for the 300s
    // cadence) through target_runtime_fields_from_target, the exact dispatcher
    // raw_taker_config uses. A config that re-introduces the redundant token, or
    // whose cadence drifts off the contract, breaks here.
    use bolt_v2::{
        bolt_v3_config::BoltV3StrategyConfig,
        bolt_v3_market_families::target_runtime_fields_from_target,
    };

    for relative_path in SHIPPED_BINARY_ORACLE_STRATEGY_FILES {
        let strategy: BoltV3StrategyConfig = toml::from_str(
            &std::fs::read_to_string(support::repo_path(relative_path))
                .expect("canonical strategy config should be readable"),
        )
        .expect("canonical strategy config should parse");
        let target = strategy
            .target
            .as_table()
            .unwrap_or_else(|| panic!("{relative_path} target should be a table"));

        assert!(
            !target.contains_key("cadence_slug_token"),
            "{relative_path} must OMIT cadence_slug_token and rely on derivation, \
             not restate the redundant token"
        );
        assert_eq!(
            target.get("cadence_secs").and_then(toml::Value::as_integer),
            Some(300),
            "{relative_path} target.cadence_secs should be the 300s updown cadence"
        );

        let runtime = target_runtime_fields_from_target(&strategy.target).unwrap_or_else(|error| {
            panic!("{relative_path} target must derive its runtime fields: {error}")
        });
        assert_eq!(
            runtime.cadence_slug_token, "5m",
            "{relative_path} must derive the updown runtime-contract token 5m from cadence_secs=300"
        );
    }
}

#[test]
fn shipped_binary_oracle_config_rejects_legacy_price_to_beat_feed_id_under_runtime() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy},
        bolt_v3_validate::validate_strategies,
    };

    let stable_root: BoltV3RootConfig = toml::from_str(
        &std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture should be readable"),
    )
    .expect("stable root should parse");
    let strategy_toml =
        binary_oracle_strategy_source_without_legacy_gate_runtime_fields_from_path(
            "config/strategies/binary_oracle_btc.toml",
        )
        .replace(
            "[parameters.runtime]\n",
            "[parameters.runtime]\nprice_to_beat_feed_id = \"0x1111111111111111111111111111111111111111111111111111111111111111\"\n",
        );
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&strategy_toml).expect("canonical strategy config should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("config/strategies/binary_oracle_btc.toml"),
        relative_path: "strategies/binary_oracle_btc.toml".to_string(),
        config: strategy,
    }];

    let messages = validate_strategies(&stable_root, &loaded);
    assert!(
        messages.iter().any(|message| {
            message.contains("parameters.runtime.price_to_beat_feed_id")
                && message.contains("[gate_providers.<id>.")
        }),
        "legacy price_to_beat_feed_id in shipped operator example must fail closed with a gate-provider migration message: {messages:#?}"
    );
}

#[test]
fn outcome_group_root_parses_polymarket_event_source() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig, bolt_v3_outcome_group_sources::OutcomeGroupSourceKind,
    };

    let root: BoltV3RootConfig = toml::from_str(&outcome_group_root_toml(
        &valid_polymarket_event_source_toml(),
    ))
    .expect("root with outcome_group_sources should parse");

    let sources = root
        .outcome_group_sources
        .as_ref()
        .expect("configured outcome_group_sources should be present");
    assert_eq!(sources.len(), 1);
    let source = &sources[0];
    assert_eq!(source.source_id, "poly_world_cup");
    assert_eq!(source.kind, OutcomeGroupSourceKind::GammaEvent);
    let expected_event_slugs = ["world-cup-final".to_owned()];
    assert_eq!(
        source.event_slugs.as_deref(),
        Some(expected_event_slugs.as_slice())
    );
}

#[test]
fn outcome_group_sources_reject_unknown_fields_at_root_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let source = valid_polymarket_event_source_toml().replace(
        "enabled = true",
        "enabled = true\nmisspelled_selector = true",
    );
    let error = toml::from_str::<BoltV3RootConfig>(&outcome_group_root_toml(&source))
        .expect_err("unknown outcome_group_sources field should fail serde closure");

    let rendered = error.to_string();
    assert!(
        rendered.contains("unknown field") && rendered.contains("misspelled_selector"),
        "unexpected parse error: {rendered}"
    );
}

#[test]
fn binary_oracle_roots_remain_backward_compatible_without_outcome_groups() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let root: BoltV3RootConfig =
        toml::from_str(&support::repo_text("tests/fixtures/bolt_v3/root.toml"))
            .expect("existing binary-oracle fixture should still parse");

    assert!(root.outcome_group_sources.is_none());
}

#[test]
fn outcome_group_root_parses_polymarket_market_slug_source() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig, bolt_v3_outcome_group_sources::OutcomeGroupSourceKind,
    };

    let root: BoltV3RootConfig = toml::from_str(&outcome_group_root_toml(
        &valid_polymarket_market_slug_source_toml(),
    ))
    .expect("market-slug-only source should parse");

    let sources = root
        .outcome_group_sources
        .as_ref()
        .expect("configured outcome_group_sources should be present");
    let source = &sources[0];
    assert_eq!(source.kind, OutcomeGroupSourceKind::GammaMarketSlug);
    let expected_market_slugs = ["winner-market".to_owned()];
    assert_eq!(
        source.market_slugs.as_deref(),
        Some(expected_market_slugs.as_slice())
    );
}

#[test]
fn outcome_group_root_parses_bounded_polymarket_gamma_query_source() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig, bolt_v3_outcome_group_sources::OutcomeGroupSourceKind,
    };

    let root: BoltV3RootConfig = toml::from_str(&outcome_group_root_toml(
        &valid_polymarket_gamma_query_source_toml(),
    ))
    .expect("bounded Gamma-query source should parse");

    let sources = root
        .outcome_group_sources
        .as_ref()
        .expect("configured outcome_group_sources should be present");
    let source = &sources[0];
    assert_eq!(source.kind, OutcomeGroupSourceKind::GammaQuery);
    assert_eq!(
        source.gamma_query.as_ref().map(|query| query.max_markets),
        Some(20)
    );
}

#[test]
fn outcome_group_root_parses_hyperliquid_hip4_question_source() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig, bolt_v3_outcome_group_sources::OutcomeGroupSourceKind,
    };

    let root: BoltV3RootConfig = toml::from_str(&outcome_group_root_toml(
        &valid_hyperliquid_hip4_source_toml(),
    ))
    .expect("HIP-4 outcome question source should parse");

    let sources = root
        .outcome_group_sources
        .as_ref()
        .expect("configured outcome_group_sources should be present");
    let source = &sources[0];
    assert_eq!(source.kind, OutcomeGroupSourceKind::Hip4);
    assert_eq!(source.question, Some(42));
}

#[test]
fn outcome_group_root_validation_fails_closed_on_source_shape_errors() {
    let duplicate_sources = format!(
        "{}\n{}",
        valid_polymarket_event_source_toml(),
        valid_polymarket_market_slug_source_toml().replace(
            "source_id = \"poly_market_slug\"",
            "source_id = \"poly_world_cup\""
        )
    );
    let missing_freshness = without_toml_sections(
        &valid_polymarket_event_source_toml(),
        &["outcome_group_sources.freshness"],
    );
    let missing_constraints = without_toml_sections(
        &valid_polymarket_event_source_toml(),
        &["outcome_group_sources.order_constraints"],
    );
    let missing_settlement = without_toml_sections(
        &valid_polymarket_event_source_toml(),
        &["outcome_group_sources.settlement_rules"],
    );
    let missing_role_bindings = without_toml_sections(
        &valid_polymarket_event_source_toml(),
        &["outcome_group_sources.role_bindings"],
    );
    let missing_payouts = without_toml_sections(
        &valid_polymarket_event_source_toml(),
        &["outcome_group_sources.settlement_rules.non_standard_terminal_payouts"],
    );
    let unbounded_query = valid_polymarket_gamma_query_source_toml().replace(
        "search = \"world cup\"\nmax_markets = 20",
        "max_markets = 20",
    );
    let sports_only_query = valid_polymarket_gamma_query_source_toml().replace(
        "search = \"world cup\"\nmax_markets = 20",
        "sports_market_types = [\"moneyline\"]\nmax_markets = 20",
    );
    let search_with_sports_query = valid_polymarket_gamma_query_source_toml().replace(
        "search = \"world cup\"\nmax_markets = 20",
        "search = \"world cup\"\nsports_market_types = [\"moneyline\"]\nmax_markets = 20",
    );
    let non_positive_min_quantity = valid_polymarket_event_source_toml().replace(
        "default_min_quantity = \"5\"",
        "default_min_quantity = \"0\"",
    );
    let non_positive_min_notional = valid_polymarket_event_source_toml().replace(
        "default_min_notional = \"1\"",
        "default_min_notional = \"0\"",
    );
    let missing_terminal_states = valid_polymarket_event_source_toml().replace(
        "terminal_state_labels = [\"home\", \"draw\", \"away\"]",
        "terminal_state_labels = []",
    );
    let missing_neg_risk = valid_polymarket_event_source_toml()
        .replace("expected_neg_risk_market_id = \"neg-risk-123\"\n", "");
    let unknown_client = valid_polymarket_event_source_toml().replace(
        "client_id = \"polymarket_main\"",
        "client_id = \"unknown_client\"",
    );
    let missing_freshness_max_age =
        valid_polymarket_event_source_toml().replace("max_age_ms = 500\n", "");
    let missing_freshness_max_clock_skew =
        valid_polymarket_event_source_toml().replace("max_clock_skew_ms = 250\n", "");
    let excessive_freshness_clock_skew = valid_polymarket_event_source_toml()
        .replace("max_clock_skew_ms = 250", "max_clock_skew_ms = 501");
    for (case, source, expected) in [
        (
            "duplicate source_id",
            duplicate_sources.as_str(),
            "outcome_group_sources source_id `poly_world_cup` is duplicated",
        ),
        (
            "unbounded query",
            unbounded_query.as_str(),
            "outcome_group_sources.poly_gamma_query.gamma_query must include at least one bounded selector",
        ),
        (
            "sports-only query",
            sports_only_query.as_str(),
            "outcome_group_sources.poly_gamma_query.gamma_query must include at least one bounded selector",
        ),
        (
            "search query with sports market types",
            search_with_sports_query.as_str(),
            "outcome_group_sources.poly_gamma_query.gamma_query.sports_market_types cannot be combined with search or market_query",
        ),
        (
            "missing freshness",
            missing_freshness.as_str(),
            "outcome_group_sources.poly_world_cup.freshness is required",
        ),
        (
            "missing order constraints",
            missing_constraints.as_str(),
            "outcome_group_sources.poly_world_cup.order_constraints is required",
        ),
        (
            "non-positive min quantity",
            non_positive_min_quantity.as_str(),
            "outcome_group_sources.poly_world_cup.order_constraints.default_min_quantity must be positive",
        ),
        (
            "non-positive min notional",
            non_positive_min_notional.as_str(),
            "outcome_group_sources.poly_world_cup.order_constraints.default_min_notional must be positive",
        ),
        (
            "missing settlement rules",
            missing_settlement.as_str(),
            "outcome_group_sources.poly_world_cup.settlement_rules is required",
        ),
        (
            "missing terminal states",
            missing_terminal_states.as_str(),
            "outcome_group_sources.poly_world_cup.terminal_state_labels must not be empty",
        ),
        (
            "missing role bindings",
            missing_role_bindings.as_str(),
            "outcome_group_sources.poly_world_cup.role_bindings is required for polymarket_gamma_event",
        ),
        (
            "missing non-standard payout vectors",
            missing_payouts.as_str(),
            "outcome_group_sources.poly_world_cup.settlement_rules.non_standard_terminal_payouts must not be empty",
        ),
        (
            "missing neg risk expectation",
            missing_neg_risk.as_str(),
            "outcome_group_sources.poly_world_cup.expected_neg_risk_market_id is required for polymarket_gamma_event",
        ),
        (
            "unknown client",
            unknown_client.as_str(),
            "outcome_group_sources.poly_world_cup.client_id `unknown_client` does not match any [clients.<id>] block",
        ),
        (
            "missing freshness max age",
            missing_freshness_max_age.as_str(),
            "outcome_group_sources.poly_world_cup.freshness.max_age_ms is required",
        ),
        (
            "missing freshness max clock skew",
            missing_freshness_max_clock_skew.as_str(),
            "outcome_group_sources.poly_world_cup.freshness.max_clock_skew_ms is required",
        ),
        (
            "freshness clock skew exceeds max age",
            excessive_freshness_clock_skew.as_str(),
            "outcome_group_sources.poly_world_cup.freshness.max_clock_skew_ms must be less than or equal to outcome_group_sources.poly_world_cup.freshness.max_age_ms",
        ),
    ] {
        let messages = outcome_group_root_validation_messages(source);
        assert!(
            messages.iter().any(|message| message.contains(expected)),
            "{case} should contain `{expected}`, got: {messages:#?}"
        );
    }
}

#[test]
fn binary_oracle_archetype_still_requires_realized_volatility_surface() {
    let strategy = support::repo_text("tests/fixtures/bolt_v3/strategies/binary_oracle.toml")
        .replace(
            "realized_volatility_surface_id = \"configured_rv_surface\"\n",
            "",
        );
    let messages = strategy_validation_messages_for_toml(&strategy);

    assert!(
        messages
            .iter()
            .any(|message| message.contains("realized_volatility_surface_id is required")),
        "binary oracle must retain its archetype-owned RV requirement: {messages:#?}"
    );
}

fn replace_in_fixture_root(needle: &str, replacement: &str) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    assert!(
        fixture.contains(needle),
        "fixture must contain `{needle}` for this validation test to mutate"
    );
    fixture.replace(needle, replacement)
}

fn fixture_root_without_decision_evidence_recovery_bound() -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    fixture
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .starts_with("recovery_evidence_max_bytes =")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fixture_polymarket_execution_block() -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let start = fixture
        .find("[clients.polymarket_main.execution]\n")
        .expect("fixture should contain Polymarket execution block");
    let rest = &fixture[start..];
    let end = rest
        .find("\n[clients.polymarket_main.secrets]")
        .expect("fixture should contain Polymarket secrets block after execution");
    rest[..end + 1].to_string()
}

fn replace_fixture_root_line_with_prefix(prefix: &str, replacement: Option<&str>) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mut hits = 0usize;
    let rewritten = fixture
        .lines()
        .filter_map(|line| {
            if line.trim_start().starts_with(prefix) {
                hits += 1;
                replacement.map(str::to_string)
            } else {
                Some(line.to_string())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        hits, 1,
        "fixture must contain exactly one line starting with `{prefix}`"
    );
    rewritten
}

fn fixture_root_with_order_execution_mode(mode: &str) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    if fixture
        .lines()
        .any(|line| line.trim_start().starts_with("order_execution_mode = "))
    {
        return fixture
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("order_execution_mode = ") {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    format!("{indent}order_execution_mode = \"{mode}\"")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    fixture.replace(
        "mode = \"Live\"",
        &format!("mode = \"Live\"\norder_execution_mode = \"{mode}\""),
    )
}

fn fixture_root_without_order_execution_mode() -> String {
    std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable")
        .lines()
        .filter(|line| !line.trim_start().starts_with("order_execution_mode = "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strategy_fixture_without_submit_orders() -> String {
    std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable")
    .lines()
    .filter(|line| !line.trim_start().starts_with("submit_orders = "))
    .collect::<Vec<_>>()
    .join("\n")
}

fn fixture_strategy_with_submit_orders(value: &str) -> String {
    let fixture = strategy_fixture_without_submit_orders();
    fixture.replace(
        "[parameters]\n",
        &format!("[parameters]\nsubmit_orders = {value}\n"),
    )
}

/// Replace one-line key assignments inside a single TOML table.
///
/// `replace_in_fixture_root` does a global `str::replace`, so a key that also
/// appears in another table (for example `qsize`) cannot be flipped in one block
/// alone without a multi-line anchor — and a multi-line `\n` anchor breaks on a
/// CRLF checkout. This walks the fixture line-by-line via `str::lines()`
/// (LF/CRLF agnostic) and rewrites only the lines whose trimmed text matches a
/// needle while inside `section_header`. Each needle must match exactly one line
/// in that table, so the mutation is both scoped and platform-independent.
fn replace_in_fixture_section(section_header: &str, replacements: &[(&str, &str)]) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mut in_section = false;
    let mut hits = vec![0usize; replacements.len()];
    let rewritten: Vec<String> = fixture
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_section = trimmed == section_header;
            }
            if in_section {
                for (idx, (needle, replacement)) in replacements.iter().enumerate() {
                    if trimmed == *needle {
                        hits[idx] += 1;
                        // Preserve the matched line's leading indentation so the
                        // mutated fixture keeps its original TOML formatting.
                        let indent = &line[..line.len() - line.trim_start().len()];
                        return format!("{indent}{replacement}");
                    }
                }
            }
            line.to_string()
        })
        .collect();
    for ((needle, _), hit) in replacements.iter().zip(hits) {
        assert_eq!(
            hit, 1,
            "section `{section_header}` must contain exactly one `{needle}` line to mutate, matched {hit}"
        );
    }
    rewritten.join("\n")
}

#[test]
fn parses_loss_governor_halt_actions_from_root_fixture() {
    use bolt_v2::{
        bolt_v3_config::BoltV3RootConfig,
        bolt_v3_loss_halt_actions::{LossGovernorRecoveryMode, LossGovernorTradingStateAction},
    };

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&fixture).expect("root fixture should parse");
    let loss_governor = root
        .risk
        .loss_governor
        .as_ref()
        .expect("root fixture should configure loss governor");

    assert_eq!(
        loss_governor.on_loss_breach_trading_state,
        Some(LossGovernorTradingStateAction::Reducing)
    );
    assert_eq!(
        loss_governor.on_untrusted_snapshot_trading_state,
        Some(LossGovernorTradingStateAction::Reducing)
    );
    assert_eq!(
        loss_governor.recovery_mode,
        Some(LossGovernorRecoveryMode::Manual)
    );
    assert_eq!(
        loss_governor.manual_recovery_evidence_max_path_bytes,
        Some(256)
    );
    assert_eq!(loss_governor.active_position_pnl_max_entries, Some(64));
}

#[test]
fn rejects_enabled_loss_governor_missing_halt_action_fields() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("on_loss_breach_trading_state = \"reducing\"\n", "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing action fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.loss_governor.on_loss_breach_trading_state")
                && message.contains("must be configured when enabled")
        }),
        "enabled loss governor should require explicit trading-state action: {messages:#?}"
    );
}

#[test]
fn rejects_enabled_loss_governor_missing_manual_recovery_evidence_path_limit() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("manual_recovery_evidence_max_path_bytes = 256\n", "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing manual recovery limit fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.loss_governor.manual_recovery_evidence_max_path_bytes")
                && message.contains("must be configured when enabled")
        }),
        "enabled loss governor should require explicit manual recovery evidence path limit: {messages:#?}"
    );
}

#[test]
fn rejects_enabled_loss_governor_zero_manual_recovery_evidence_path_limit() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "manual_recovery_evidence_max_path_bytes = 256",
        "manual_recovery_evidence_max_path_bytes = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero manual recovery limit fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.loss_governor.manual_recovery_evidence_max_path_bytes")
                && message.contains("positive integer")
        }),
        "enabled loss governor should reject zero manual recovery evidence path limit: {messages:#?}"
    );
}

#[test]
fn rejects_enabled_loss_governor_missing_active_position_pnl_cap() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("active_position_pnl_max_entries = 64\n", "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing active position PnL cap fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.loss_governor.active_position_pnl_max_entries")
                && message.contains("positive integer")
        }),
        "enabled loss governor should require an active position PnL cap: {messages:#?}"
    );
}

#[test]
fn rejects_enabled_loss_governor_zero_active_position_pnl_cap() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "active_position_pnl_max_entries = 64",
        "active_position_pnl_max_entries = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero active position PnL cap fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.loss_governor.active_position_pnl_max_entries")
                && message.contains("positive integer")
        }),
        "enabled loss governor should reject zero active position PnL cap: {messages:#?}"
    );
}

#[test]
fn rejects_enabled_loss_governor_untrusted_snapshot_noop_action() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "on_untrusted_snapshot_trading_state = \"reducing\"",
        "on_untrusted_snapshot_trading_state = \"none\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("untrusted snapshot noop action fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.loss_governor.on_untrusted_snapshot_trading_state")
                && message.contains("reducing or halted")
        }),
        "enabled loss governor should reject no-op untrusted snapshot action: {messages:#?}"
    );
}

#[test]
fn rejects_enabled_loss_governor_missing_threshold_fields() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (field, line) in [
        ("max_per_trade_loss", "max_per_trade_loss = \"2.50\"\n"),
        ("max_daily_loss", "max_daily_loss = \"7.50\"\n"),
        ("max_rolling_loss", "max_rolling_loss = \"10.00\"\n"),
        ("max_drawdown", "max_drawdown = \"15.00\"\n"),
    ] {
        let mutated = replace_in_fixture_root(line, "");
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("missing loss threshold fixture should parse");
        let messages = validate_root_only(&root);
        let label = format!("risk.loss_governor.{field}");

        assert!(
            messages.iter().any(|message| {
                message.contains(&label) && message.contains("must be configured when enabled")
            }),
            "enabled loss governor should require {label}: {messages:#?}"
        );
    }
}

#[test]
fn rejects_enabled_loss_governor_non_positive_thresholds() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (field, original) in [
        ("max_per_trade_loss", "max_per_trade_loss = \"2.50\""),
        ("max_daily_loss", "max_daily_loss = \"7.50\""),
        ("max_rolling_loss", "max_rolling_loss = \"10.00\""),
        ("max_drawdown", "max_drawdown = \"15.00\""),
    ] {
        let replacement = format!("{field} = \"0\"");
        let mutated = replace_in_fixture_root(original, &replacement);
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("zero loss threshold fixture should parse");
        let messages = validate_root_only(&root);
        let label = format!("risk.loss_governor.{field}");

        assert!(
            messages.iter().any(|message| {
                message.contains(&label) && message.contains("positive decimal")
            }),
            "enabled loss governor should reject non-positive {label}: {messages:#?}"
        );
    }
}

#[test]
fn enforced_polymarket_submit_admission_uses_registered_live_allowance_source() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = replace_in_fixture_root(
        "enforce_submit_admission = false",
        "enforce_submit_admission = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("enforced submit-admission fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().all(|message| {
            !message.contains("registered live provider collateral allowance source")
        }),
        "Polymarket submit admission should use its registered live allowance source: {messages:#?}"
    );
}

#[test]
fn enforced_submit_admission_rejects_provider_without_live_allowance_source() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = replace_in_fixture_root(
        "enforce_submit_admission = false",
        "enforce_submit_admission = true",
    )
    .replace("venue_id = \"POLYMARKET\"", "venue_id = \"BINANCE\"");
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("unsupported allowance provider fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.capital_pools[polymarket-prediction-live].venue_id")
                && message.contains("registered live provider collateral allowance source")
        }),
        "enforced admission must reject providers without the single live allowance path: {messages:#?}"
    );
}

#[test]
fn enforced_submit_admission_rejects_venue_without_attested_reconciliation_completeness() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = replace_in_fixture_root(
        "enforce_submit_admission = false",
        "enforce_submit_admission = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("enforced submit-admission fixture should parse");
    let messages = validate_root_only(&root);

    let unmet: Vec<&String> = messages
        .iter()
        .filter(|message| {
            message
                .contains("risk.capital_pools[polymarket-prediction-live].enforce_submit_admission")
                && message.contains("attests reconciliation completeness")
        })
        .collect();

    // Every unmet condition must be reported, not just the first. Five upstream
    // defects gate this flip; closing one does not clear the rest, so an engineer
    // who fixes one has to see the others still listed. Closing one can change
    // how another fails rather than fixing it, which is a reason to report all
    // five, not a reason to collapse them. Four are adapter defects and the
    // fifth is in the execution engine, so fixing the adapter alone cannot
    // empty this list.
    assert_eq!(
        unmet.len(),
        5,
        "arming submit admission must report every unmet condition: {messages:#?}"
    );
    assert!(
        unmet
            .iter()
            .any(|message| message.contains("silently partial")),
        "the mass-status partiality condition must be reported: {messages:#?}"
    );
    assert!(
        unmet
            .iter()
            .any(|message| message.contains("zero local-filled floor")),
        "the filled-quantity cap condition must be reported: {messages:#?}"
    );
    assert!(
        unmet
            .iter()
            .any(|message| message.contains("discards non-confirmed trades")),
        "the matched-but-unconfirmed condition must be reported: {messages:#?}"
    );
    // What that condition says the pinned engine does is asserted against the
    // pinned engine in `tests/nt_external_order_recovery.rs`, not here. An
    // earlier revision of this test checked that the condition string contained
    // the words "canceled or expired"; the claim those words made was wrong, and
    // the assertion stayed green through two review rounds because a substring
    // check cannot contradict a description of a dependency's behaviour. Only
    // the dependency can.
    assert!(
        unmet
            .iter()
            .any(|message| message.contains("none of the account's own maker orders")),
        "the unowned maker-trade condition must be reported: {messages:#?}"
    );
    assert!(
        unmet
            .iter()
            .any(|message| message.contains("lose positions inside the engine")),
        "the engine-side position drop must be reported: {messages:#?}"
    );
}

#[test]
fn unenforced_capital_pool_does_not_trip_the_reconciliation_completeness_gate() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Control for the gate above: the tracked configuration leaves admission
    // unenforced, so the gate must stay silent rather than blocking startup.
    let source = replace_in_fixture_root(
        "enforce_submit_admission = false",
        "enforce_submit_admission = false",
    );
    let root: BoltV3RootConfig = toml::from_str(&source).expect("tracked fixture should parse");
    let messages = validate_root_only(&root);

    // Must match the phrase the validator actually emits. An earlier version of
    // this control searched for wording the validator no longer produced, so it
    // passed even when the gate fired on an unenforced pool -- a control that
    // controls nothing is worse than no control.
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("attests reconciliation completeness")),
        "the completeness gate must not fire while admission is unenforced: {messages:#?}"
    );
}

#[test]
fn decision_evidence_rejects_missing_recovery_evidence_max_bytes() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let source = fixture_root_without_decision_evidence_recovery_bound();
    let error = toml::from_str::<BoltV3RootConfig>(&source)
        .expect_err("decision evidence without a recovery byte cap must not parse");
    assert!(
        error.to_string().contains("recovery_evidence_max_bytes"),
        "missing mandatory recovery byte cap must be explicit: {error}"
    );
}

#[test]
fn rejects_zero_decision_evidence_recovery_evidence_max_bytes() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = fixture_root_without_decision_evidence_recovery_bound().replace(
        "retired_relative_paths = [\"bolt-v3/decision-evidence/order-intents.jsonl\"]",
        "retired_relative_paths = [\"bolt-v3/decision-evidence/order-intents.jsonl\"]\nrecovery_evidence_max_bytes = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("zero recovery evidence cap fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("persistence.decision_evidence.recovery_evidence_max_bytes")
                && message.contains("positive integer")
        }),
        "recovery evidence max bytes must reject zero: {messages:#?}"
    );
}

#[test]
fn rejects_unbounded_decision_evidence_recovery_evidence_max_bytes() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "recovery_evidence_max_bytes = 1048576",
        "recovery_evidence_max_bytes = 18446744073709551615",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("u64::MAX evidence cap fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("persistence.decision_evidence.recovery_evidence_max_bytes")
                && message.contains("finite")
        }),
        "an unbounded current-evidence cap must be rejected: {messages:#?}"
    );
}

#[test]
fn enforced_submit_admission_accepts_positive_recovery_evidence_max_bytes() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = replace_in_fixture_root(
        "enforce_submit_admission = false",
        "enforce_submit_admission = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("positive recovery evidence cap fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        !messages.iter().any(|message| {
            message.contains("persistence.decision_evidence.recovery_evidence_max_bytes")
        }),
        "positive recovery evidence cap should satisfy enforced submit admission: {messages:#?}"
    );
}

#[test]
fn capital_pool_rejects_non_positive_thresholds() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (section, label, original, replacement) in [
        (
            "[[risk.capital_pools]]",
            "risk.capital_pools[polymarket-prediction-live].max_pool_liability",
            "max_pool_liability = \"25.00\"",
            "max_pool_liability = \"0\"",
        ),
        (
            "[[risk.capital_pools]]",
            "risk.capital_pools[polymarket-prediction-live].max_snapshot_age_ns",
            "max_snapshot_age_ns = 5000000000",
            "max_snapshot_age_ns = 0",
        ),
        (
            "[risk.capital_pools.capital_admission_policy]",
            "risk.capital_pools[polymarket-prediction-live].capital_admission_policy.min_remaining_pool_balance",
            "min_remaining_pool_balance = \"1.00\"",
            "min_remaining_pool_balance = \"0\"",
        ),
        (
            "[risk.capital_pools.capital_admission_policy.fee_slippage]",
            "risk.capital_pools[polymarket-prediction-live].capital_admission_policy.fee_slippage.max_fee_liability",
            "max_fee_liability = \"0.10\"",
            "max_fee_liability = \"0\"",
        ),
        (
            "[risk.capital_pools.capital_admission_policy.fee_slippage]",
            "risk.capital_pools[polymarket-prediction-live].capital_admission_policy.fee_slippage.max_slippage_liability",
            "max_slippage_liability = \"0.20\"",
            "max_slippage_liability = \"0\"",
        ),
    ] {
        let mutated = replace_in_fixture_section(section, &[(original, replacement)]);
        let root: BoltV3RootConfig = toml::from_str(&mutated)
            .expect("non-positive capital pool threshold fixture should parse");
        let messages = validate_root_only(&root);

        assert!(
            messages.iter().any(|message| {
                message.contains(label)
                    && (message.contains("positive decimal")
                        || message.contains("positive integer"))
            }),
            "capital pool threshold {label} should reject non-positive values: {messages:#?}"
        );
    }
}

#[test]
fn rejects_more_than_one_enforced_capital_pool() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = format!(
        "{}\n{}",
        replace_in_fixture_root(
            "enforce_submit_admission = false",
            "enforce_submit_admission = true"
        ),
        r#"
[[risk.capital_pools]]
pool_id = "secondary-prediction-live"
venue_id = "POLYMARKET"
account_id = "POLYMARKET-001"
collateral_currency = "PUSD"
product_kind = "prediction_market_binary"
enforce_submit_admission = true
max_pool_liability = "10.00"
max_snapshot_age_ns = 5000000000

[risk.capital_pools.prediction_market_binary]
yes_instrument_id = "condition-secondary-yes.POLYMARKET"
no_instrument_id = "condition-secondary-no.POLYMARKET"
collateral_coupled_group_id = "condition-secondary"

[risk.capital_pools.capital_admission_policy]
min_remaining_pool_balance = "1.00"

[risk.capital_pools.capital_admission_policy.fee_slippage]
max_fee_liability = "0.10"
max_slippage_liability = "0.20"
"#
    );
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("two enforced pool fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains(
                "risk.capital_pools may enable submit admission enforcement for at most one pool",
            )
        }),
        "multiple enforced capital pools must fail validation: {messages:#?}"
    );
}

#[test]
fn rejects_capital_pools_sharing_venue_account() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let source = format!(
        "{}\n{}",
        std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture root should read"),
        r#"
[[risk.capital_pools]]
pool_id = "secondary-prediction-live"
venue_id = "POLYMARKET"
account_id = "POLYMARKET-001"
collateral_currency = "USDC"
product_kind = "prediction_market_binary"
enforce_submit_admission = false
max_pool_liability = "10.00"
max_snapshot_age_ns = 5000000000

[risk.capital_pools.prediction_market_binary]
yes_instrument_id = "condition-secondary-yes.POLYMARKET"
no_instrument_id = "condition-secondary-no.POLYMARKET"
collateral_coupled_group_id = "condition-secondary"

[risk.capital_pools.capital_admission_policy]
min_remaining_pool_balance = "1.00"

[risk.capital_pools.capital_admission_policy.fee_slippage]
max_fee_liability = "0.10"
max_slippage_liability = "0.20"
"#
    );
    let root: BoltV3RootConfig =
        toml::from_str(&source).expect("duplicate venue/account pool fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("risk.capital_pools[secondary-prediction-live]")
                && message.contains("venue_id/account_id")
                && message.contains("unique")
        }),
        "capital pools sharing venue/account must fail validation: {messages:#?}"
    );
}

fn fixture_root_with_binance_reference_client() -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    format!("{fixture}\n{}", binance_reference_root_fixture())
}

fn binance_reference_root_fixture() -> String {
    support::repo_text("tests/fixtures/bolt_v3/binance_reference_root.toml")
}

fn binance_reference_client_data_block() -> String {
    let fixture = binance_reference_root_fixture();
    let start = fixture
        .find("[clients.binance_reference.data]")
        .expect("binance reference fixture must include a data block");
    let end = fixture
        .find("[clients.binance_reference.secrets]")
        .expect("binance reference fixture must include a secrets block");
    fixture[start..end].to_string()
}

fn binance_reference_secrets_block() -> String {
    let fixture = binance_reference_root_fixture();
    let start = fixture
        .find("[clients.binance_reference.secrets]")
        .expect("binance reference fixture must include a secrets block");
    fixture[start..].to_string()
}

fn replace_in_binance_reference_fixture(needle: &str, replacement: &str) -> String {
    let fixture = fixture_root_with_binance_reference_client();
    assert!(
        fixture.contains(needle),
        "binance validation fixture must contain `{needle}` for this test to mutate"
    );
    fixture.replace(needle, replacement)
}

fn fixture_root_with_gate_providers(gate_providers_toml: &str) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let fixture = without_toml_sections(&fixture, &["gate_providers."]);
    format!("{fixture}\n{gate_providers_toml}")
}

fn fixture_strategy_with_target_gate_subscriptions(gate_subscriptions_toml: &str) -> String {
    let fixture = std::fs::read_to_string(support::repo_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    ))
    .expect("strategy fixture should be readable");
    let fixture = without_toml_sections(&fixture, &["target.gate_subscriptions."]);
    format!("{fixture}\n{gate_subscriptions_toml}")
}

fn root_with_single_chainlink_feed_binding(resolution_identity: &str, feed_id: &str) -> String {
    root_with_chainlink_feed_bindings(&[(resolution_identity, feed_id)])
}

fn root_with_chainlink_feed_bindings(feed_bindings: &[(&str, &str)]) -> String {
    let bindings_toml = feed_bindings
        .iter()
        .map(|(resolution_identity, feed_id)| {
            format!(
                r#"
[[gate_providers.resolution_oracle_primary.chainlink_data_streams.feed_bindings]]
resolution_identity = "{resolution_identity}"
value_kind = "price"
feed_id = "{feed_id}"
report_schema_version = 3
report_decimal_scale = 8
"#
            )
        })
        .collect::<String>();
    fixture_root_with_gate_providers(&format!(
        r#"
[gate_providers.resolution_oracle_primary]
provider_kind = "chainlink_data_streams"
capabilities = ["resolution_value"]

[gate_providers.resolution_oracle_primary.freshness]
max_age_ms = 300000
max_clock_skew_ms = 5000

[gate_providers.resolution_oracle_primary.chainlink_data_streams]
endpoint_id = "testnet-data-streams"
rest_base_url = "https://api.testnet-dataengine.chain.link"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 4
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
{bindings_toml}
"#
    ))
}

fn strategy_with_single_chainlink_mapping(resolution_identity: &str) -> String {
    fixture_strategy_with_target_gate_subscriptions(&format!(
        r#"
[target.gate_subscriptions.resolution]
required = true
allowed_provider_kinds = ["chainlink_data_streams", "pyth", "exchange_index", "venue_native", "hyperliquid_hip4", "deribit_index", "outcome_oracle"]
allowed_value_kinds = ["price", "index", "outcome", "metadata"]
provider_preference = ["resolution_oracle_primary"]
allow_no_resolution = false

[[target.gate_subscriptions.resolution.market_mappings]]
family_key = "updown"
market_class = "binary_option"
resolution_kind = "chainlink_data_streams"
resolution_identity = "{resolution_identity}"
value_kind = "price"
provider_id = "resolution_oracle_primary"
"#
    ))
}

fn without_toml_sections(source: &str, section_prefixes: &[&str]) -> String {
    let mut keep_line = true;
    let mut lines = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            let header = trimmed.trim_start_matches('[');
            keep_line = !section_prefixes
                .iter()
                .any(|prefix| header.starts_with(prefix));
        }
        if keep_line {
            lines.push(line);
        }
    }
    let mut filtered = lines.join("\n");
    filtered.push('\n');
    filtered
}

fn outcome_group_root_toml(source_toml: &str) -> String {
    let fixture = support::repo_text("tests/fixtures/bolt_v3/root.toml").replace(
        "order_execution_mode = \"live\"",
        "order_execution_mode = \"shadow\"",
    );
    format!("{fixture}\n{source_toml}")
}

fn outcome_group_root_validation_messages(source_toml: &str) -> Vec<String> {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let root: BoltV3RootConfig = toml::from_str(&outcome_group_root_toml(source_toml))
        .expect("outcome-group root should parse before validation");
    validate_root_only(&root)
}

fn valid_polymarket_event_source_toml() -> String {
    let digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    format!(
        r#"
[[outcome_group_sources]]
source_id = "poly_world_cup"
client_id = "polymarket_main"
kind = "polymarket_gamma_event"
event_slugs = ["world-cup-final"]
sports_market_types = ["moneyline"]
expected_neg_risk_market_id = "neg-risk-123"
terminal_state_labels = ["home", "draw", "away"]
max_markets = 20
enabled = true

[outcome_group_sources.freshness]
max_age_ms = 500
max_clock_skew_ms = 250

[outcome_group_sources.order_constraints]
default_min_quantity = "5"
default_min_notional = "1"

[outcome_group_sources.role_bindings]
kind = "operator_attested_positive_side"
attestation_sha256 = "{digest}"
legs = [
  {{ terminal_state_label = "home", pays_on_terminal_state_native_leg_id = "home-positive", pays_unless_terminal_state_native_leg_id = "home-inverse" }},
  {{ terminal_state_label = "draw", pays_on_terminal_state_native_leg_id = "draw-positive", pays_unless_terminal_state_native_leg_id = "draw-inverse" }},
  {{ terminal_state_label = "away", pays_on_terminal_state_native_leg_id = "away-positive", pays_unless_terminal_state_native_leg_id = "away-inverse" }},
]

[outcome_group_sources.settlement_rules]
settlement_contract_id = "ctf-world-cup-final"
settlement_source_kind = "polymarket_ctf_uma"
terminal_state_convention = "exactly_one_winner"
void_policy = "refund_all_legs"
rounding_policy = "decimal_exact"
timing_policy = "venue_final_resolution"
attestation_sha256 = "{digest}"

[outcome_group_sources.settlement_rules.non_standard_terminal_payouts.void_refund]
convention = "operator_attested_static_payout_per_unit"
terminal_state_label = "void_refund"
legs = [
  {{ outcome_label = "home", side_label = "operator-positive", payout_per_unit = "1" }},
  {{ outcome_label = "home", side_label = "operator-inverse", payout_per_unit = "1" }},
  {{ outcome_label = "draw", side_label = "operator-positive", payout_per_unit = "1" }},
  {{ outcome_label = "draw", side_label = "operator-inverse", payout_per_unit = "1" }},
  {{ outcome_label = "away", side_label = "operator-positive", payout_per_unit = "1" }},
  {{ outcome_label = "away", side_label = "operator-inverse", payout_per_unit = "1" }},
]
attestation_sha256 = "{digest}"
"#
    )
}

fn valid_polymarket_market_slug_source_toml() -> String {
    valid_polymarket_event_source_toml()
        .replace(
            "source_id = \"poly_world_cup\"",
            "source_id = \"poly_market_slug\"",
        )
        .replace(
            "kind = \"polymarket_gamma_event\"",
            "kind = \"polymarket_gamma_market_slug\"",
        )
        .replace(
            "event_slugs = [\"world-cup-final\"]\nsports_market_types = [\"moneyline\"]\n",
            "market_slugs = [\"winner-market\"]\n",
        )
}

fn valid_polymarket_gamma_query_source_toml() -> String {
    valid_polymarket_event_source_toml()
        .replace("source_id = \"poly_world_cup\"", "source_id = \"poly_gamma_query\"")
        .replace("kind = \"polymarket_gamma_event\"", "kind = \"polymarket_gamma_query\"")
        .replace(
            "event_slugs = [\"world-cup-final\"]\nsports_market_types = [\"moneyline\"]\n",
            "",
        )
        .replace(
            "max_markets = 20\nenabled = true\n\n[outcome_group_sources.freshness]",
            "enabled = true\n\n[outcome_group_sources.gamma_query]\nsearch = \"world cup\"\nmax_markets = 20\n\n[outcome_group_sources.freshness]",
        )
}

fn valid_hyperliquid_hip4_source_toml() -> String {
    let digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    format!(
        r#"
[[outcome_group_sources]]
source_id = "hyperliquid_question"
client_id = "hyperliquid_main"
kind = "hyperliquid_hip4"
question = 42
terminal_state_labels = ["home", "draw", "away"]
max_groups = 10
enabled = false

[outcome_group_sources.freshness]
max_age_ms = 500
max_clock_skew_ms = 250

[outcome_group_sources.order_constraints]
default_min_quantity = "1"
default_min_notional = "1"

[outcome_group_sources.settlement_rules]
settlement_contract_id = "hip4-question-42"
settlement_source_kind = "hyperliquid_outcome_question"
terminal_state_convention = "exactly_one_winner"
void_policy = "operator_attested_fallback"
rounding_policy = "decimal_exact"
timing_policy = "venue_final_resolution"
attestation_sha256 = "{digest}"

[outcome_group_sources.settlement_rules.non_standard_terminal_payouts.fallback]
convention = "operator_attested_static_payout_per_unit"
terminal_state_label = "fallback"
legs = [
  {{ outcome_label = "home", side_label = "structured-positive", payout_per_unit = "0" }},
  {{ outcome_label = "draw", side_label = "structured-positive", payout_per_unit = "0" }},
  {{ outcome_label = "away", side_label = "structured-positive", payout_per_unit = "0" }},
]
attestation_sha256 = "{digest}"
"#
    )
}

fn target_gate_subscription_messages(gate_subscriptions_toml: &str) -> Vec<String> {
    let strategy_toml = fixture_strategy_with_target_gate_subscriptions(gate_subscriptions_toml);
    let strategy: bolt_v2::bolt_v3_config::BoltV3StrategyConfig = toml::from_str(&strategy_toml)
        .expect("strategy fixture with gate subscriptions should parse");
    let (_, errors) = bolt_v2::bolt_v3_market_families::validate_strategy_target(
        "strategy `binary_oracle`",
        &strategy.target,
    );
    errors.into_iter().map(|error| error.to_string()).collect()
}

fn fixture_root_config() -> bolt_v2::bolt_v3_config::BoltV3RootConfig {
    let root_toml = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("root fixture should be readable");
    toml::from_str(&root_toml).expect("root fixture should parse")
}

#[test]
fn every_live_config_requires_an_unfiltered_complete_nt_reconciliation_universe() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    type Mutation = Box<dyn Fn(&mut BoltV3RootConfig)>;
    let cases: Vec<(&str, Mutation)> = vec![
        (
            "reconciliation",
            Box::new(|root| root.nautilus.exec_engine.reconciliation = false),
        ),
        (
            "reconciliation_lookback_mins",
            Box::new(|root| root.nautilus.exec_engine.reconciliation_lookback_mins = 1),
        ),
        (
            "reconciliation_instrument_ids",
            Box::new(|root| {
                root.nautilus.exec_engine.reconciliation_instrument_ids =
                    vec!["YES-USD.POLYMARKET".to_string()];
            }),
        ),
        (
            "filter_unclaimed_external_orders",
            Box::new(|root| {
                root.nautilus.exec_engine.filter_unclaimed_external_orders = true;
            }),
        ),
        (
            "filter_position_reports",
            Box::new(|root| {
                root.nautilus.exec_engine.filter_position_reports = true;
            }),
        ),
        (
            "filtered_client_order_ids",
            Box::new(|root| {
                root.nautilus.exec_engine.filtered_client_order_ids = vec!["client-1".to_string()];
            }),
        ),
        (
            "generate_missing_orders",
            Box::new(|root| root.nautilus.exec_engine.generate_missing_orders = false),
        ),
        (
            "open_check_interval_secs",
            Box::new(|root| root.nautilus.exec_engine.open_check_interval_secs = 0),
        ),
        (
            "open_check_lookback_mins",
            Box::new(|root| root.nautilus.exec_engine.open_check_lookback_mins = 1),
        ),
        (
            "position_check_interval_secs",
            Box::new(|root| root.nautilus.exec_engine.position_check_interval_secs = 0),
        ),
    ];

    for (field, mutate) in cases {
        let mut root = fixture_root_config();
        root.risk.capital_pools = None;
        mutate(&mut root);
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|message| message.contains(field)),
            "Bolt live config must reject incomplete NT reconciliation field {field}: {messages:#?}"
        );
    }
}

fn reference_reconnect_timeout_load_error(
    client_key: &str,
    delta_ms: i64,
) -> Result<String, String> {
    match reference_reconnect_timeout_relative_to_startup_bound_load(client_key, delta_ms) {
        Ok(()) => Err(format!(
            "clients.{client_key}.data.reconnect_timeout_ms at startup bound delta_ms={delta_ms} loaded successfully"
        )),
        Err(error) => Ok(error),
    }
}

fn reference_reconnect_timeout_relative_to_startup_bound_load(
    client_key: &str,
    delta_ms: i64,
) -> Result<(), String> {
    reference_reconnect_config_load(client_key, |startup_bound_ms| {
        toml::Value::Integer(
            startup_bound_ms
                .checked_add(delta_ms)
                .expect("startup bound plus test delta should fit test integer"),
        )
    })
}

fn reference_reconnect_config_load(
    client_key: &str,
    reconnect_timeout: impl FnOnce(i64) -> toml::Value,
) -> Result<(), String> {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;

    let temp = tempfile::tempdir().expect("config-load tempdir should create");
    let strategies_dir = temp.path().join("strategies");
    fs::create_dir(&strategies_dir).expect("strategy fixture dir should create");
    fs::copy(
        support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        strategies_dir.join("binary_oracle.toml"),
    )
    .expect("strategy fixture should copy");

    let mut root: toml::Value =
        toml::from_str(&support::repo_text("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture TOML should parse as generic TOML");
    let startup_bound_ms = root_startup_bound_ms(&root);
    let valid_reconnect_timeout_ms = startup_bound_ms
        .checked_add(1)
        .expect("startup bound plus one millisecond should fit test integer");
    for reference_client in ["chainlink_reference", "polyresearch_reference"] {
        set_client_reconnect_timeout(
            &mut root,
            reference_client,
            toml::Value::Integer(valid_reconnect_timeout_ms),
        );
    }
    set_client_reconnect_timeout(&mut root, client_key, reconnect_timeout(startup_bound_ms));

    let root_path = temp.path().join("root.toml");
    let root_text = toml::to_string(&root).expect("mutated root TOML should serialize");
    fs::write(&root_path, root_text).expect("mutated root fixture should write");

    load_bolt_v3_config(&root_path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn reference_reconnect_startup_bound_overflow_load_error(
    timeout_secs: [i64; 3],
) -> Result<String, String> {
    use bolt_v2::bolt_v3_config::load_bolt_v3_config;

    let temp = tempfile::tempdir().expect("config-load tempdir should create");
    let strategies_dir = temp.path().join("strategies");
    fs::create_dir(&strategies_dir).expect("strategy fixture dir should create");
    fs::copy(
        support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        strategies_dir.join("binary_oracle.toml"),
    )
    .expect("strategy fixture should copy");

    let mut root: toml::Value =
        toml::from_str(&support::repo_text("tests/fixtures/bolt_v3/root.toml"))
            .expect("root fixture TOML should parse as generic TOML");
    let nautilus = root
        .get_mut("nautilus")
        .and_then(toml::Value::as_table_mut)
        .expect("root fixture should configure [nautilus]");
    for (field, timeout_secs) in [
        "timeout_connection_secs",
        "timeout_reconciliation_secs",
        "timeout_portfolio_secs",
    ]
    .into_iter()
    .zip(timeout_secs)
    {
        nautilus.insert(field.to_string(), toml::Value::Integer(timeout_secs));
    }

    let root_path = temp.path().join("root.toml");
    let root_text = toml::to_string(&root).expect("mutated root TOML should serialize");
    fs::write(&root_path, root_text).expect("mutated root fixture should write");

    match load_bolt_v3_config(&root_path) {
        Ok(_) => Err("overflowing Nautilus startup bound loaded successfully".to_string()),
        Err(error) => Ok(error.to_string()),
    }
}

fn root_startup_bound_ms(root: &toml::Value) -> i64 {
    use bolt_v2::bolt_v3_config::{BoltV3RootConfig, nautilus_startup_bound_secs};

    let root: BoltV3RootConfig = root
        .clone()
        .try_into()
        .expect("root fixture should deserialize for startup-bound calculation");
    let startup_bound_ms = std::time::Duration::from_secs(
        nautilus_startup_bound_secs(&root.nautilus)
            .expect("fixture startup bound should fit seconds"),
    )
    .as_millis();
    i64::try_from(startup_bound_ms).expect("fixture startup bound should fit test integer")
}

fn set_client_reconnect_timeout(
    root: &mut toml::Value,
    client_key: &str,
    reconnect_timeout: toml::Value,
) {
    let data = root
        .get_mut("clients")
        .and_then(toml::Value::as_table_mut)
        .and_then(|clients| clients.get_mut(client_key))
        .and_then(toml::Value::as_table_mut)
        .and_then(|client| client.get_mut("data"))
        .and_then(toml::Value::as_table_mut)
        .unwrap_or_else(|| panic!("root fixture should configure clients.{client_key}.data"));
    data.insert("reconnect_timeout_ms".to_string(), reconnect_timeout);
}

fn strategy_validation_messages_for_toml(strategy_toml: &str) -> Vec<String> {
    let root_toml = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("root fixture should be readable");
    strategy_validation_messages_for_root_and_strategy_toml(&root_toml, strategy_toml)
}

fn strategy_validation_messages_for_root_and_strategy_toml(
    root_toml: &str,
    strategy_toml: &str,
) -> Vec<String> {
    use bolt_v2::bolt_v3_config::{BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy};

    let stable_root: BoltV3RootConfig =
        toml::from_str(root_toml).expect("root fixture should parse");
    let strategy: BoltV3StrategyConfig =
        toml::from_str(strategy_toml).expect("strategy fixture should parse");
    let loaded = vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }];

    bolt_v2::bolt_v3_validate::validate_strategies(&stable_root, &loaded)
}

fn legacy_binary_oracle_runtime_field_messages(field_line: &str) -> Vec<String> {
    let strategy_toml = binary_oracle_strategy_source_without_legacy_gate_runtime_fields_from_path(
        "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
    )
    .replace(
        "[parameters.runtime]\n",
        &format!("[parameters.runtime]\n{field_line}\n"),
    );

    strategy_validation_messages_for_toml(&strategy_toml)
}

fn binary_oracle_strategy_source_without_legacy_gate_runtime_fields_from_path(
    relative_path: &str,
) -> String {
    let source = std::fs::read_to_string(support::repo_path(relative_path))
        .expect("binary oracle strategy source should be readable");
    let mut filtered = source
        .lines()
        .filter(|line| !is_legacy_binary_oracle_gate_runtime_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    filtered.push('\n');
    filtered
}

fn is_legacy_binary_oracle_gate_runtime_line(line: &&str) -> bool {
    let trimmed = line.trim_start();
    [
        "price_to_beat_source",
        "price_to_beat_feed_id",
        "price_to_beat_report_schema_version",
        "price_to_beat_report_decimal_scale",
        "forced_flat_stale_chainlink_ms",
    ]
    .iter()
    .any(|field| trimmed.starts_with(field))
}

fn assert_binary_oracle_strategy_source_uses_gate_schema(label: &str, source: &str) {
    assert!(
        source.contains("[target.gate_subscriptions.resolution]"),
        "binary_oracle {label} must declare the provider-neutral resolution gate subscription"
    );
    assert!(
        source.contains("[[target.gate_subscriptions.resolution.market_mappings]]"),
        "binary_oracle {label} must declare config-owned resolution market mappings"
    );
    for forbidden in [
        "price_to_beat_source",
        "price_to_beat_feed_id",
        "price_to_beat_report_schema_version",
        "price_to_beat_report_decimal_scale",
        "forced_flat_stale_chainlink_ms",
    ] {
        assert!(
            !source.contains(forbidden),
            "binary_oracle {label} must not retain provider-specific runtime field `{forbidden}`"
        );
    }
}

fn mutate_parameters_exit_order(fixture: &str, mutate: impl FnOnce(&str) -> String) -> String {
    let (before_exit, exit_and_after) = fixture
        .split_once("[parameters.exit_order]")
        .expect("fixture should include exit_order table");
    let (exit_block, after_forced_exit_marker) = exit_and_after
        .split_once("\n[parameters.forced_exit_order]")
        .expect("fixture should include forced_exit_order table after exit_order");
    format!(
        "{before_exit}[parameters.exit_order]{}\n[parameters.forced_exit_order]{after_forced_exit_marker}",
        mutate(exit_block)
    )
}

fn mutate_parameters_entry_order(fixture: &str, mutate: impl FnOnce(&str) -> String) -> String {
    let (before_entry, entry_and_after) = fixture
        .split_once("[parameters.entry_order]")
        .expect("fixture should include entry_order table");
    let (entry_block, after_exit_marker) = entry_and_after
        .split_once("\n[parameters.exit_order]")
        .expect("fixture should include exit_order table after entry_order");
    format!(
        "{before_entry}[parameters.entry_order]{}\n[parameters.exit_order]{after_exit_marker}",
        mutate(entry_block)
    )
}

#[test]
fn rejects_zero_explicit_nt_exec_runtime_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_binance_reference_fixture(
        "inflight_check_threshold_ms = 5000\ninflight_check_retries = 5",
        "inflight_check_threshold_ms = 0\ninflight_check_retries = 5",
    )
    .replace(
        "open_check_threshold_ms = 5000\nopen_check_missing_retries = 5",
        "open_check_threshold_ms = 0\nopen_check_missing_retries = 5",
    )
    .replace(
        "max_single_order_queries_per_cycle = 10\nsingle_order_query_delay_ms = 100",
        "max_single_order_queries_per_cycle = 0\nsingle_order_query_delay_ms = 100",
    )
    .replace(
        "position_check_threshold_ms = 5000\nposition_check_retries = 3",
        "position_check_threshold_ms = 0\nposition_check_retries = 3",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero NT exec defaults fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.inflight_check_threshold_ms must be a positive integer",
        "nautilus.exec_engine.open_check_threshold_ms must be a positive integer",
        "nautilus.exec_engine.max_single_order_queries_per_cycle must be a positive integer",
        "nautilus.exec_engine.position_check_threshold_ms must be a positive integer",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_zero_runtime_capture_start_poll_interval() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "runtime_capture_start_poll_interval_ms = 50",
        "runtime_capture_start_poll_interval_ms = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero runtime-capture poll fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|m| {
            m.contains("persistence.runtime_capture_start_poll_interval_ms")
                && m.contains("must be a positive integer")
        }),
        "expected positive-integer runtime-capture poll validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_zero_data_client_readiness_probe_poll_interval() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "data_client_readiness_probe_poll_interval_ms = 50",
        "data_client_readiness_probe_poll_interval_ms = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero data-client readiness poll fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|m| {
            m.contains("persistence.data_client_readiness_probe_poll_interval_ms")
                && m.contains("must be a positive integer")
        }),
        "expected positive-integer data-client readiness poll validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_zero_persistence_min_free_bytes() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "runtime_capture_start_poll_interval_ms = 50",
        "min_free_bytes = 0\nruntime_capture_start_poll_interval_ms = 50",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero min-free-bytes fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|m| {
            m.contains("persistence.min_free_bytes") && m.contains("must be a positive integer")
        }),
        "expected positive-integer min-free-bytes validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_absolute_decision_evidence_machine_relative_path() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "machine_relative_path = \"bolt-v3/decision-evidence/current/machine.jsonl\"",
        "machine_relative_path = \"/var/lib/bolt/decision-evidence/current/machine.jsonl\"",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated)
        .expect("absolute decision-evidence relative-path fixture should parse");
    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|m| {
            m.contains("persistence.decision_evidence.machine_relative_path")
                && m.contains("must be non-empty, relative, normalized")
        }),
        "expected config-load rejection of an absolute decision-evidence path, got: {messages:#?}"
    );
}

#[test]
fn rejects_colliding_decision_evidence_paths() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let machine_observation_collision = replace_in_fixture_root(
        "observation_relative_path = \"bolt-v3/decision-evidence/current/observation.jsonl\"",
        "observation_relative_path = \"bolt-v3/decision-evidence/current/machine.jsonl\"",
    );
    let root: BoltV3RootConfig = toml::from_str(&machine_observation_collision)
        .expect("colliding decision-evidence path fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("persistence.decision_evidence paths must be distinct")
        }),
        "machine and observation paths must be distinct: {messages:#?}"
    );

    let active_retired_collision = replace_in_fixture_root(
        "retired_relative_paths = [\"bolt-v3/decision-evidence/order-intents.jsonl\"]",
        "retired_relative_paths = [\"bolt-v3/decision-evidence/current/machine.jsonl\"]",
    );
    let root: BoltV3RootConfig = toml::from_str(&active_retired_collision)
        .expect("active-retired collision fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|message| {
            message.contains("persistence.decision_evidence paths must be distinct")
        }),
        "active and retired paths must be distinct: {messages:#?}"
    );
}

#[test]
fn rejects_noncanonical_decision_evidence_path_spellings() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for noncanonical in [
        "bolt-v3//decision-evidence/current/machine.jsonl",
        "bolt-v3/./decision-evidence/current/machine.jsonl",
        "bolt-v3/decision-evidence/current/machine.jsonl/",
        " bolt-v3/decision-evidence/current/machine.jsonl",
    ] {
        let mutated = replace_in_fixture_root(
            "machine_relative_path = \"bolt-v3/decision-evidence/current/machine.jsonl\"",
            &format!("machine_relative_path = \"{noncanonical}\""),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("noncanonical path fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|message| {
                message.contains("persistence.decision_evidence.machine_relative_path")
                    && message.contains("normalized")
            }),
            "noncanonical path `{noncanonical}` must be rejected: {messages:#?}"
        );
    }
}

#[test]
fn rejects_decision_evidence_path_ancestry_before_filesystem_mutation() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (from, to) in [
        (
            "machine_relative_path = \"bolt-v3/decision-evidence/current/machine.jsonl\"",
            "machine_relative_path = \"bolt-v3/decision-evidence/current\"",
        ),
        (
            "retired_relative_paths = [\"bolt-v3/decision-evidence/order-intents.jsonl\"]",
            "retired_relative_paths = [\"bolt-v3/decision-evidence/current\"]",
        ),
    ] {
        let mutated = replace_in_fixture_root(from, to);
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("path ancestry fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|message| {
                message.contains("persistence.decision_evidence paths")
                    && message.contains("ancestor")
            }),
            "path ancestry must be rejected before runtime mutation: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_nt_data_engine_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "time_bars_interval_type = \"LEFT_OPEN\"",
        "time_bars_interval_type = \"SIDEWAYS\"",
    )
    .replace(
        "time_bars_origins = {}",
        "time_bars_origins = { INVALID = 1 }",
    )
    .replace(
        "debug = false\nqsize = 100000",
        "debug = false\nqsize = 1000",
    );
    assert!(
        mutated.contains("time_bars_interval_type = \"SIDEWAYS\"")
            && mutated.contains("time_bars_origins = { INVALID = 1 }")
            && mutated.contains("qsize = 1000"),
        "test fixture mutation must exercise every invalid data-engine branch"
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT data-engine fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.data_engine.time_bars_interval_type is not valid",
        "nautilus.data_engine.time_bars_origins key `INVALID` is not a valid Nautilus bar aggregation",
        "nautilus.data_engine.qsize must match NT default",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_nt_exec_values_unsupported_by_rust_live_runtime() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root("snapshot_orders = false", "snapshot_orders = true")
        .replace("snapshot_positions = false", "snapshot_positions = true")
        .replace("purge_from_database = false", "purge_from_database = true")
        .replace("qsize = 100000", "qsize = 1000");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("unsupported NT exec values fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.snapshot_orders must be false",
        "nautilus.exec_engine.snapshot_positions must be false",
        "nautilus.exec_engine.purge_from_database must be false",
        "nautilus.exec_engine.qsize must match NT default",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_nt_exec_filter_identifiers() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "reconciliation_instrument_ids = []",
        "reconciliation_instrument_ids = [\"INVALID\"]",
    )
    .replace(
        "filtered_client_order_ids = []",
        "filtered_client_order_ids = [\"\"]",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT exec filter identifiers fixture should parse");
    let messages = validate_root_only(&root);
    for needle in [
        "nautilus.exec_engine.reconciliation_instrument_ids contains invalid instrument ID",
        "nautilus.exec_engine.filtered_client_order_ids contains invalid client order ID",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_invalid_external_client_id_at_parse_time() {
    // FINDING-8: external_clients is `Vec<ClientId>`, so an empty string is
    // rejected by ClientId's deserializer before the validator ever runs.
    // This keeps invalid identifiers from reaching runtime code through any
    // code path that bypasses validate_root_only.
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let mutated = replace_in_fixture_root(
        "emit_quotes_from_book_depths = false\nexternal_clients = []",
        "emit_quotes_from_book_depths = false\nexternal_clients = [\"\"]",
    );
    let parse_result = toml::from_str::<BoltV3RootConfig>(&mutated);
    let error = parse_result.expect_err("empty ClientId must be rejected at TOML parse time");
    let message = error.to_string();
    assert!(
        message.contains("invalid string for 'value'"),
        "expected ClientId rejection from serde, got: {message}"
    );
    assert!(
        message.contains("external_clients = [\"\"]"),
        "expected error to point at the offending external_clients entry, got: {message}"
    );
}

#[test]
fn rejects_nt_risk_bypass_key_at_parse_time() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    // Anchor on the unique `max_order_submit_rate` assignment rather than the
    // `[risk.nautilus]\n` header so the search pattern never matches a fixture
    // newline (avoids CRLF-normalization fragility). The injected `bypass = true`
    // line still lands inside the `[risk.nautilus]` block, immediately above the
    // rate assignment, producing a byte-identical mutation.
    let mutated = replace_in_fixture_root(
        "max_order_submit_rate = \"33/00:01:00\"",
        "bypass = true\nmax_order_submit_rate = \"33/00:01:00\"",
    );
    let error = toml::from_str::<BoltV3RootConfig>(&mutated)
        .expect_err("risk.nautilus.bypass must not be part of the TOML schema");
    let message = error.to_string();
    assert!(
        message.contains("unknown field `bypass`"),
        "expected unknown-field rejection for risk.nautilus.bypass, got: {message}"
    );
}

#[test]
fn shipped_root_configs_do_not_expose_nt_risk_bypass() {
    for (label, source) in [
        ("config/root.toml", include_str!("../config/root.toml")),
        (
            "tests/fixtures/bolt_v3/root.toml",
            include_str!("fixtures/bolt_v3/root.toml"),
        ),
    ] {
        // Scan lines for a key that is exactly `bypass` (trimmed, before `=`)
        // rather than matching `\nbypass =` substrings. This is platform-agnostic
        // (independent of CRLF/LF normalization), catches a `bypass =` key on any
        // line including the first, and does not false-match keys like
        // `bypass_logging`.
        let exposes_bypass = source.lines().any(|line| {
            line.split_once('=')
                .is_some_and(|(key, _)| key.trim() == "bypass")
        });
        assert!(
            !exposes_bypass,
            "{label} must not expose risk.nautilus.bypass"
        );
    }
}

#[test]
fn rejects_nt_risk_values_unsupported_by_rust_live_runtime() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated =
        replace_in_fixture_section("[risk.nautilus]", &[("qsize = 100000", "qsize = 1000")]);
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("unsupported NT risk values fixture should parse");
    let messages = validate_root_only(&root);
    for needle in ["risk.nautilus.qsize must match NT default"] {
        assert!(
            messages.iter().any(|m| m.contains(needle)),
            "expected `{needle}` in validation messages, got: {messages:#?}"
        );
    }
}

#[test]
fn maps_top_level_nt_shutdown_on_error() {
    use bolt_v2::{
        bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config},
        bolt_v3_live_node::make_live_node_config,
    };

    let mutated = replace_in_fixture_root("shutdown_on_error = false", "shutdown_on_error = true");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("top-level shutdown_on_error fixture should parse");
    let loaded = LoadedBoltV3Config {
        root_path: support::repo_path("tests/fixtures/bolt_v3/root.toml"),
        config_bundle_checksum: "test-checksum".to_string(),
        root,
        strategies: Vec::new(),
    };
    let cfg = make_live_node_config(&loaded);

    assert!(cfg.shutdown_on_error);
}

#[test]
fn rejects_invalid_nt_risk_rate_limit_strings() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (submit_rate, modify_rate) in [
        ("0/00:00:01", "100/00:00:00"),
        ("100", "33/00:01:00"),
        ("abc/00:00:01", "33/00:01:00"),
        ("100/00:01", "33/00:01:00"),
        ("100/00:00:01:00", "33/00:01:00"),
        ("100/00:60:00", "33/00:01:00"),
        ("100/00:00:60", "33/00:01:00"),
    ] {
        let mutated = replace_in_binance_reference_fixture(
            "max_order_submit_rate = \"33/00:01:00\"\nmax_order_modify_rate = \"33/00:01:00\"",
            &format!(
                "max_order_submit_rate = \"{submit_rate}\"\nmax_order_modify_rate = \"{modify_rate}\""
            ),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("invalid NT rate limit fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains(
                "risk.nautilus.max_order_submit_rate is not a valid Nautilus rate limit"
            )),
            "expected submit-rate validation message for `{submit_rate}`, got: {messages:#?}"
        );
        // Only the first case mutates modify_rate; the remaining cases keep it
        // valid so submit-rate parsing branches are isolated.
        if modify_rate == "100/00:00:00" {
            assert!(
                messages.iter().any(|m| m.contains(
                    "risk.nautilus.max_order_modify_rate is not a valid Nautilus rate limit"
                )),
                "expected modify-rate validation message for `{modify_rate}`, got: {messages:#?}"
            );
        } else {
            assert!(
                !messages.iter().any(|m| m.contains(
                    "risk.nautilus.max_order_modify_rate is not a valid Nautilus rate limit"
                )),
                "valid modify_rate `{modify_rate}` must not produce a modify-rate error: {messages:#?}"
            );
        }
    }
}

#[test]
fn rejects_nt_submit_rate_above_polymarket_egress_cap() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // The Polymarket REST egress ceiling is 100/min (NT HTTP_RATE_LIMIT), and a
    // market quote-quantity BUY command issues up to 3 REST requests (market
    // submit get_book + collateral balance + post_order), so the integer
    // order-rate ceiling is 33/min. A submit rate of 34/min over-drives egress
    // (34 * 3 = 102 > 100) and would block at egress (stale quotes) instead of
    // emitting a loud OrderDenied, so it must fail closed at config load.
    let mutated = replace_in_fixture_root(
        "max_order_submit_rate = \"33/00:01:00\"",
        "max_order_submit_rate = \"34/00:01:00\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("over-cap submit-rate fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("max_order_submit_rate")
            && m.contains("POLYMARKET")
            && m.contains("REST egress cap")),
        "submit rate above the POLYMARKET egress cap must fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_nt_modify_rate_above_polymarket_egress_cap() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // The Polymarket REST egress ceiling is 100/min (NT HTTP_RATE_LIMIT), and a
    // market quote-quantity BUY command issues up to 3 REST requests, so the
    // integer order-rate ceiling is 33/min. A modify rate of 34/min over-drives
    // egress (34 * 3 = 102 > 100), so it must fail closed.
    let mutated = replace_in_fixture_root(
        "max_order_modify_rate = \"33/00:01:00\"",
        "max_order_modify_rate = \"34/00:01:00\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("over-cap modify-rate fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("max_order_modify_rate")
            && m.contains("POLYMARKET")
            && m.contains("REST egress cap")),
        "modify rate above the POLYMARKET egress cap must fail closed: {messages:#?}"
    );
}

#[test]
fn accepts_nt_order_rates_at_polymarket_integer_command_ceiling() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // The Polymarket REST egress ceiling is 100/min (NT HTTP_RATE_LIMIT), and a
    // market quote-quantity BUY command issues up to 3 REST requests, so the
    // integer command-rate ceiling is 33/min. The boundary value is accepted
    // because 33 * 3 = 99 remains within the REST cap.
    let source = replace_in_fixture_root(
        "max_order_submit_rate = \"33/00:01:00\"",
        "max_order_submit_rate = \"33/00:01:00\"",
    );
    let root: BoltV3RootConfig = toml::from_str(&source).expect("at-cap fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages.iter().any(|m| m.contains("REST egress cap")),
        "order rates at the derived venue egress command ceiling must be accepted: {messages:#?}"
    );
}

#[test]
fn rejects_old_100_per_min_rate_now_overdrives_polymarket_fanout() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Regression guard for the per-order-command REST fanout the reconciliation
    // now applies. The old order-rate ceiling was the raw 100/min REST cap, but
    // a single Polymarket order command issues up to 3 REST requests, so
    // 100/min order commands = 300 REST/min = 3x the 100/min cap. The
    // order-rate ceiling is therefore 33/min, and the
    // previously-accepted 100/00:01:00 value must now fail closed.
    let mutated = replace_in_fixture_root(
        "max_order_submit_rate = \"33/00:01:00\"",
        "max_order_submit_rate = \"100/00:01:00\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("legacy 100/min submit-rate fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("max_order_submit_rate")
            && m.contains("POLYMARKET")
            && m.contains("REST egress cap")),
        "the legacy 100/min order rate now over-drives the 3x Polymarket REST fanout (100 * 3 = 300 > 100) and must fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_nt_rate_limit_string_whose_interval_overflows_u64() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // `hours` here fits in u64 but `hours * 3600` overflows it. The interval
    // computation is checked, so this must surface a loud validation message
    // instead of panicking (debug) or wrapping to a bogus interval (release).
    let mutated = replace_in_fixture_root(
        "max_order_submit_rate = \"33/00:01:00\"",
        "max_order_submit_rate = \"1/9999999999999999999:00:00\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("overflowing-interval fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m
            .contains("risk.nautilus.max_order_submit_rate is not a valid Nautilus rate limit")
            && m.contains("interval seconds overflow u64")),
        "an interval that overflows u64 must fail loud, not panic or wrap: {messages:#?}"
    );
}

#[test]
fn rejects_nt_submit_rate_above_egress_cap_under_dual_u64_saturation() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // Hand-verified dual-saturation vector: limit = 5e17, interval_seconds =
    // 51388888888889 * 3600 = 185000000000000400 (~1.85e17, fits u64). This is
    // a u128-overflow regression vector, not a boundary value, so the fanout
    // derate is irrelevant: the raw rate already saturates the REST cap. The
    // true rate is 5e17 * 60 / 1.85e17 ≈ 162/min, far above the 100/min
    // POLYMARKET REST egress cap regardless of the order-command fanout.
    // Under the old u64-saturating comparison both sides saturate to u64::MAX
    // (5e17*60 = 3e19 and 100*1.85e17 = 1.85e19 both exceed u64::MAX), so
    // MAX > MAX is false and the over-cap rate was wrongly accepted. The u128
    // comparison computes the true products (3e19 > 1.85e19) and rejects it.
    let mutated = replace_in_fixture_root(
        "max_order_submit_rate = \"33/00:01:00\"",
        "max_order_submit_rate = \"500000000000000000/51388888888889:00:00\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("dual-saturation submit-rate fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("max_order_submit_rate")
            && m.contains("POLYMARKET")
            && m.contains("REST egress cap")),
        "an over-cap rate that saturates both sides of the old u64 check must still fail closed: {messages:#?}"
    );
}

#[test]
fn fails_closed_on_execution_client_for_unmodeled_egress_venue() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    // An [execution] block on a venue whose REST egress ceiling bolt-v3 does not
    // model must fail closed rather than be silently skipped: a skipped venue
    // leaves the submit rate unreconciled. The config is also rejected for
    // declaring execution on a data-only provider; we only assert OUR error.
    let fixture = fixture_root_with_binance_reference_client();
    let mutated = format!("{fixture}\n[clients.binance_reference.execution]\nnot_allowed = true\n");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("unmodeled-egress execution mutation should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("clients.binance_reference")
                && m.contains("(provider=BINANCE)")
                && m.contains("models no REST egress cap")
                && m.contains("fail closed")),
        "an execution client on an unmodeled egress venue must fail closed: {messages:#?}"
    );
}

#[test]
fn rejects_invalid_nt_risk_max_notional_map_entries() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "max_notional_per_order = {}",
        "max_notional_per_order = { \"BAD\" = \"not-a-decimal\" }",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid NT max-notional map fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains(
            "risk.nautilus.max_notional_per_order key `BAD` is not a valid Nautilus instrument ID"
        )),
        "expected invalid instrument-id validation error, got: {messages:#?}"
    );
    assert!(
        messages.iter().any(|m| m
            .contains("risk.nautilus.max_notional_per_order[`BAD`] is not a valid decimal string")),
        "expected invalid notional validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_non_positive_nt_risk_max_notional_map_values() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for notional in ["0", "-1.00"] {
        let mutated = replace_in_fixture_root(
            "max_notional_per_order = {}",
            &format!("max_notional_per_order = {{ \"TRIGGER.SOURCE\" = \"{notional}\" }}"),
        );
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("non-positive NT max-notional fixture should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|m| m.contains(
                "risk.nautilus.max_notional_per_order[`TRIGGER.SOURCE`] must be a positive decimal string"
            )),
            "expected positive notional validation error for `{notional}`, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_orphan_secrets_block_without_data_or_execution() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let data_block = binance_reference_client_data_block();
    let mutated = replace_in_binance_reference_fixture(&data_block, "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("orphan-secrets fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[secrets]")
            && m.contains("no [data] block is configured")),
        "expected orphan-secrets validation error, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=BINANCE)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_ssm_paths_missing_leading_slash() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "private_key_ssm_path = \"/bolt/polymarket/private-key\"",
        "private_key_ssm_path = \"bolt/polymarket/private-key\"",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated).expect("ssm-path mutation should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("private_key_ssm_path")
            && m.contains("absolute-style SSM parameter path starting with `/`")),
        "expected SSM-path leading-slash validation error, got: {messages:#?}"
    );
    assert!(rendered.contains("clients.polymarket_main.secrets.private_key_ssm_path"));
    let legacy_path = ["venues", "polymarket_main"].join(".");
    assert!(!rendered.contains(&legacy_path));
}

#[test]
fn rejects_ssm_paths_with_leading_or_trailing_whitespace() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    for (field, original, replacement) in [
        (
            "clients.polymarket_main.secrets.private_key_ssm_path",
            "private_key_ssm_path = \"/bolt/polymarket/private-key\"",
            "private_key_ssm_path = \" /bolt/polymarket/private-key\"",
        ),
        (
            "clients.polymarket_main.secrets.api_secret_ssm_path",
            "api_secret_ssm_path = \"/bolt/polymarket/api-secret\"",
            "api_secret_ssm_path = \"/bolt/polymarket/api-secret \"",
        ),
    ] {
        let mutated = replace_in_fixture_root(original, replacement);
        let root: BoltV3RootConfig =
            toml::from_str(&mutated).expect("ssm whitespace mutation should parse");
        let messages = validate_root_only(&root);
        assert!(
            messages.iter().any(|message| message.contains(field)
                && message.contains("must not have leading or trailing whitespace")),
            "expected SSM whitespace validation error for {field}, got: {messages:#?}"
        );
    }
}

#[test]
fn rejects_polymarket_funder_with_invalid_evm_syntax() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_fixture_root_line_with_prefix(
        "funder = ",
        Some("funder = \"0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ\""),
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("not a valid EVM public address")),
        "expected EVM-syntax validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_funder_zero_address() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_fixture_root_line_with_prefix(
        "funder = ",
        Some("funder = \"0x0000000000000000000000000000000000000000\""),
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("zero address")),
        "expected zero-address validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_missing_funder_for_poly_gnosis_safe_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_fixture_root_line_with_prefix("funder = ", None);
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("required when signature_type is `poly_proxy` or `poly_gnosis_safe`")),
        "expected required-funder validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_missing_funder_for_poly_proxy_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let without_funder = replace_fixture_root_line_with_prefix("funder = ", None);
    let with_proxy = without_funder.replace(
        "signature_type = \"poly_gnosis_safe\"",
        "signature_type = \"poly_proxy\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&with_proxy).expect("poly-proxy missing-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder")
            && m.contains("required when signature_type is `poly_proxy` or `poly_gnosis_safe`")),
        "expected required-funder validation error for poly_proxy, got: {messages:#?}"
    );
}

#[test]
fn allows_missing_funder_for_eoa_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let without_funder = replace_fixture_root_line_with_prefix("funder = ", None);
    let with_eoa = without_funder.replace(
        "signature_type = \"poly_gnosis_safe\"",
        "signature_type = \"eoa\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&with_eoa).expect("eoa-without-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages.iter().any(|m| m.contains("funder")),
        "EOA signature must allow absent funder, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_data_zero_instrument_status_poll_secs() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_binance_reference_fixture(
        "instrument_status_poll_secs = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
        "instrument_status_poll_secs = 0 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-poll-interval fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("instrument_status_poll_secs")
            && m.contains("must be a positive integer")),
        "expected positive-integer poll-interval validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_only_client_with_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let execution_block = fixture_polymarket_execution_block();
    let mutated = replace_in_fixture_root(&execution_block, "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("polymarket data-only secrets fixture should parse");
    let messages = validate_root_only(&root);
    let rendered = messages.join("\n");
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[secrets]")
            && m.contains("[execution]")),
        "expected Polymarket data-only secrets validation error, got: {messages:#?}"
    );
    assert!(rendered.contains("(provider=POLYMARKET)"));
    assert!(!rendered.contains("(venue="));
}

#[test]
fn rejects_polymarket_data_subscribe_new_markets_true_in_current_slice() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "subscribe_new_markets = false",
        "subscribe_new_markets = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("subscribe_new_markets=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("subscribe_new_markets")
            && m.contains("must be false")),
        "expected subscribe_new_markets=true validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_missing_instruments_true_in_current_slice() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "auto_load_missing_instruments = false",
        "auto_load_missing_instruments = true",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("auto_load_missing_instruments=true fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_missing_instruments")
            && m.contains("must be false")),
        "expected auto_load_missing_instruments=true validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_debounce_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated =
        replace_in_fixture_root("auto_load_debounce_ms = 250", "auto_load_debounce_ms = 0");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("auto_load_debounce_ms=0 fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_debounce_ms")
            && m.contains("positive integer")),
        "expected auto_load_debounce_ms=0 validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_max_retries_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated =
        replace_in_fixture_root("auto_load_max_retries = 12", "auto_load_max_retries = 0");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("auto_load_max_retries=0 fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_max_retries")
            && m.contains("positive integer")),
        "expected auto_load_max_retries=0 validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_retry_delay_initial_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "auto_load_retry_delay_initial_secs = 5",
        "auto_load_retry_delay_initial_secs = 0",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated)
        .expect("auto_load_retry_delay_initial_secs=0 fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_retry_delay_initial_secs")
            && m.contains("positive integer")),
        "expected auto_load_retry_delay_initial_secs=0 validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_retry_delay_max_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "auto_load_retry_delay_max_secs = 15",
        "auto_load_retry_delay_max_secs = 0",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("auto_load_retry_delay_max_secs=0 fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_retry_delay_max_secs")
            && m.contains("positive integer")),
        "expected auto_load_retry_delay_max_secs=0 validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_auto_load_retry_initial_after_max() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "auto_load_retry_delay_max_secs = 15",
        "auto_load_retry_delay_max_secs = 4",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated)
        .expect("auto_load_retry_delay_initial_secs>max fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("auto_load_retry_delay_initial_secs")
            && m.contains("must be <=")
            && m.contains("auto_load_retry_delay_max_secs")),
        "expected auto_load_retry_delay_initial_secs>max validation error, got: {messages:#?}"
    );
}

#[test]
fn allows_multiple_configured_client_ids_for_same_nt_venue() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let extra_client = "\n\n[clients.polymarket_secondary]\nvenue = \"POLYMARKET\"\n\n[clients.polymarket_secondary.data]\nbase_url_http = \"https://test.invalid/clob\"\nbase_url_ws = \"wss://test.invalid/ws/market\"\nbase_url_rtds = \"wss://ws-live-data.polymarket.com\"\nbase_url_gamma = \"https://test.invalid/gamma\"\nbase_url_data_api = \"https://test.invalid/data\"\nhttp_timeout_secs = 60\nws_timeout_secs = 30\nsubscribe_new_markets = false\ndrop_quotes_missing_side = true\nnew_market_fetch_max_concurrency = 8\nauto_load_missing_instruments = false\nauto_load_debounce_ms = 250\nauto_load_max_retries = 12\nauto_load_retry_delay_initial_secs = 5\nauto_load_retry_delay_max_secs = 15\nresolve_poll_enabled = false\nresolve_poll_interval_secs = 30\nresolve_poll_grace_secs = 10\nresolve_poll_max_wait_secs = 1800\nupdate_instruments_interval_mins = 1\nws_max_subscriptions = 200\ntransport_backend = \"sockudo\"\n";
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = format!("{fixture}{extra_client}");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("two-polymarket-venues fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages
            .iter()
            .any(|m| m.contains("at most one [clients.<id>] block per venue")),
        "client routing is keyed by [clients.<id>], so same-venue client ids must be accepted; got: {messages:#?}"
    );
}

#[test]
fn allows_metadata_response_readiness_probe_without_static_quote_targets() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "quote"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 4
allow_metadata_target_sampling = false
"#
    ))
    .expect("metadata-response readiness probe should parse");

    let messages = validate_root_only(&root);

    assert!(
        messages.is_empty(),
        "metadata-response readiness probes should not require copied static quote target ids: {messages:#?}"
    );
}

#[test]
fn allows_metadata_response_readiness_probe_with_explicit_sampling_opt_in() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "quote"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 4
allow_metadata_target_sampling = true
"#
    ))
    .expect("metadata-response sampling readiness probe should parse");

    let messages = validate_root_only(&root);

    assert!(
        messages.is_empty(),
        "metadata-response sampling must be an explicit config-owned opt-in: {messages:#?}"
    );
}

#[test]
fn rejects_book_readiness_probe_without_book_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "book"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 4
allow_metadata_target_sampling = false
"#
    ))
    .expect("book readiness probe should parse so validation can reject missing book type");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.book_type")
                && message.contains("market_data_kind = \"book\"")
        }),
        "book probes must declare the NT book type in TOML: {messages:#?}"
    );
}

#[test]
fn rejects_quote_readiness_probe_with_book_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "quote"
book_type = "l2_mbp"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 4
allow_metadata_target_sampling = false
"#
    ))
    .expect("quote readiness probe should parse so validation can reject book type");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.book_type")
                && message.contains("market_data_kind = \"book\"")
        }),
        "quote probes must not carry a book subscription type: {messages:#?}"
    );
}

#[test]
fn allows_trade_readiness_probe() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
quote_target_source = "metadata_response"
chunk_size = 200
chunk_observation_window_seconds = 45
min_observed_targets = 10
"#
    ))
    .expect("trade chunk-count readiness probe should parse");

    let messages = validate_root_only(&root);

    assert!(
        messages.is_empty(),
        "a trade chunk-count readiness probe must validate cleanly: {messages:#?}"
    );
}

#[test]
fn rejects_trade_readiness_probe_with_book_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
book_type = "l2_mbp"
quote_target_source = "metadata_response"
chunk_size = 200
chunk_observation_window_seconds = 45
min_observed_targets = 10
"#
    ))
    .expect("trade readiness probe should parse so validation can reject book type");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.book_type")
                && message.contains("market_data_kind = \"book\"")
        }),
        "trade probes must not carry a book subscription type: {messages:#?}"
    );
}

#[test]
fn rejects_trade_chunk_count_probe_without_chunk_size() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
quote_target_source = "metadata_response"
chunk_observation_window_seconds = 45
min_observed_targets = 10
"#
    ))
    .expect("trade chunk-count probe should parse so validation can reject the missing chunk size");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.chunk_size")
                && message.contains("positive integer")
        }),
        "trade chunk-count probes must declare a config-owned chunk size: {messages:#?}"
    );
}

#[test]
fn rejects_trade_chunk_count_probe_without_observation_window() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
quote_target_source = "metadata_response"
chunk_size = 200
min_observed_targets = 10
"#
    ))
    .expect("trade chunk-count probe should parse so validation can reject the missing window");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains(
                "clients.polymarket_main.readiness_probe.chunk_observation_window_seconds",
            ) && message.contains("positive integer")
        }),
        "trade chunk-count probes must declare a config-owned per-chunk window: {messages:#?}"
    );
}

#[test]
fn rejects_trade_chunk_count_probe_without_min_observed_targets() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
quote_target_source = "metadata_response"
chunk_size = 200
chunk_observation_window_seconds = 45
"#
    ))
    .expect("trade chunk-count probe should parse so validation can reject the missing live bar");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.min_observed_targets")
                && message.contains("positive integer")
        }),
        "trade chunk-count probes must declare a config-owned required-live-markets count: {messages:#?}"
    );
}

#[test]
fn rejects_trade_chunk_count_probe_with_max_metadata_quote_targets() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
quote_target_source = "metadata_response"
chunk_size = 200
chunk_observation_window_seconds = 45
min_observed_targets = 10
max_metadata_quote_targets = 20
"#
    ))
    .expect("trade chunk-count probe should parse so validation can reject the fixed-sample bound");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.max_metadata_quote_targets")
                && message.contains("trade chunk-count")
        }),
        "a trade chunk-count probe has no fixed sample, so max_metadata_quote_targets must be rejected: {messages:#?}"
    );
}

#[test]
fn rejects_trade_chunk_count_probe_with_allow_metadata_target_sampling() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "trade"
quote_target_source = "metadata_response"
chunk_size = 200
chunk_observation_window_seconds = 45
min_observed_targets = 10
allow_metadata_target_sampling = true
"#
    ))
    .expect("trade chunk-count probe should parse so validation can reject the sampling opt-in");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message
                .contains("clients.polymarket_main.readiness_probe.allow_metadata_target_sampling")
                && message.contains("trade chunk-count")
        }),
        "a trade chunk-count probe does not sample, so allow_metadata_target_sampling must be rejected: {messages:#?}"
    );
}

#[test]
fn rejects_chunk_count_fields_on_book_probe() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "book"
book_type = "l2_mbp"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 20
allow_metadata_target_sampling = true
chunk_size = 200
chunk_observation_window_seconds = 45
"#
    ))
    .expect("book probe should parse so validation can reject chunk-count fields");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.chunk_size")
                && message.contains("market_data_kind = \"trade\"")
        }),
        "chunk_size is only valid for a trade chunk-count probe: {messages:#?}"
    );
    assert!(
        messages.iter().any(|message| {
            message.contains(
                "clients.polymarket_main.readiness_probe.chunk_observation_window_seconds",
            ) && message.contains("market_data_kind = \"trade\"")
        }),
        "chunk_observation_window_seconds is only valid for a trade chunk-count probe: {messages:#?}"
    );
}

#[test]
fn rejects_metadata_response_readiness_probe_with_min_quote_targets() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let error = toml::from_str::<BoltV3RootConfig>(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "quote"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 4
allow_metadata_target_sampling = false
min_metadata_quote_targets = 2
"#
    ))
    .expect_err("metadata-response readiness probes must not accept hardcoded min target counts");

    assert!(
        error.to_string().contains("min_metadata_quote_targets"),
        "parse error should name the removed min target count field: {error}"
    );
}

#[test]
fn rejects_metadata_response_readiness_probe_without_max_quote_targets() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "quote"
quote_target_source = "metadata_response"
"#
    ))
    .expect("metadata-response readiness probe should parse so validation can reject missing max");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe.max_metadata_quote_targets")
                && message.contains("positive integer")
        }),
        "metadata-response readiness probe must declare a config-owned safety bound: {messages:#?}"
    );
}

#[test]
fn rejects_readiness_probe_with_both_metadata_response_and_static_targets() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let root: BoltV3RootConfig = toml::from_str(&format!(
        r#"{fixture}

[clients.polymarket_main.readiness_probe]
market_data_kind = "quote"
quote_target_source = "metadata_response"
max_metadata_quote_targets = 4
allow_metadata_target_sampling = false

[clients.polymarket_main.readiness_probe.quote_targets.configured_quote_probe]
instrument_id = "CONFIGURED-PROBE.POLYMARKET"
"#
    ))
    .expect("mixed readiness probe should parse so validation can reject the dual target source");

    let messages = validate_root_only(&root);

    assert!(
        messages.iter().any(|message| {
            message.contains("clients.polymarket_main.readiness_probe")
                && message.contains("quote_target_source")
                && message.contains("quote_targets")
        }),
        "metadata-response readiness probes must reject static target ids in the same client block: {messages:#?}"
    );
}

#[test]
fn root_config_declares_requested_nt_data_clients_for_registration() {
    use bolt_v2::bolt_v3_config::{
        DataClientReadinessProbeMarketDataKind, DataClientReadinessProbeQuoteTargetSource,
        load_bolt_v3_config,
    };

    let loaded = load_bolt_v3_config(&support::repo_path("config/root.toml"))
        .expect("root.toml should load with requested data clients");

    for client_key in [
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
        "hyperliquid_data",
        "polymarket_main",
    ] {
        let client = loaded
            .root
            .clients
            .get(client_key)
            .unwrap_or_else(|| panic!("{client_key} must be configured in root.toml"));
        assert!(
            client.data.is_some(),
            "{client_key} must declare a [data] block"
        );
        let readiness_probe = client
            .readiness_probe
            .as_ref()
            .unwrap_or_else(|| panic!("{client_key} must declare a readiness_probe block"));
        if readiness_probe.market_data_kind == DataClientReadinessProbeMarketDataKind::Trade
            && readiness_probe.quote_target_source
                == DataClientReadinessProbeQuoteTargetSource::MetadataResponse
            && readiness_probe.chunk_size.is_some()
        {
            assert!(
                readiness_probe.chunk_observation_window_seconds.is_some()
                    && readiness_probe.min_observed_targets.is_some(),
                "{client_key} trade chunk-count probe must declare chunk_size, chunk_observation_window_seconds, and min_observed_targets"
            );
        } else {
            assert_eq!(
                readiness_probe.allow_metadata_target_sampling,
                Some(true),
                "{client_key} must explicitly opt into source-owned metadata sampling"
            );
        }
    }
}

#[test]
fn root_config_wires_hyperliquid_data_only_client_without_execution_or_secrets() {
    let loaded =
        bolt_v2::bolt_v3_config::load_bolt_v3_config(&support::repo_path("config/root.toml"))
            .expect("root.toml should load with Hyperliquid data client");
    let client = loaded
        .root
        .clients
        .get("hyperliquid_data")
        .expect("hyperliquid_data must be configured in root.toml");

    assert_eq!(client.venue.as_str(), "HYPERLIQUID");
    assert!(
        client.execution.is_none(),
        "issue #784 must not arm Hyperliquid execution"
    );
    assert!(
        client.secrets.is_none(),
        "data-only Hyperliquid client must not require signer material"
    );

    let data = client
        .data
        .as_ref()
        .and_then(toml::Value::as_table)
        .expect("hyperliquid_data must declare a [data] table");
    assert_eq!(
        data.get(stringify!(environment))
            .and_then(toml::Value::as_str),
        Some("mainnet")
    );
    assert_eq!(
        data.get(stringify!(base_url_ws))
            .and_then(toml::Value::as_str),
        Some("wss://api.hyperliquid.xyz/ws")
    );
    assert_eq!(
        data.get(stringify!(base_url_http))
            .and_then(toml::Value::as_str),
        Some("https://api.hyperliquid.xyz/info")
    );
    assert_eq!(
        data.get(stringify!(transport_backend))
            .and_then(toml::Value::as_str),
        Some("sockudo")
    );
    assert!(
        client.readiness_probe.is_some(),
        "production data clients must include strategy-free readiness coverage"
    );
}

#[test]
fn root_config_wires_single_hyperliquid_execution_client_for_all_surfaces() {
    use nautilus_model::identifiers::ClientId;

    let loaded =
        bolt_v2::bolt_v3_config::load_bolt_v3_config(&support::repo_path("config/root.toml"))
            .expect("root.toml should load with a Hyperliquid execution client");

    for client_key in [
        "hyperliquid_standard_perps_execution",
        "hyperliquid_spot_execution",
        "hyperliquid_hip3_execution",
        "hyperliquid_hip4_execution",
    ] {
        assert!(
            !loaded.root.clients.contains_key(client_key),
            "{client_key} must stay collapsed into the single Hyperliquid execution client"
        );
    }

    let client = loaded
        .root
        .clients
        .get("hyperliquid_execution")
        .expect("hyperliquid_execution must be configured in root.toml");

    assert_eq!(client.venue.as_str(), "HYPERLIQUID");
    assert!(
        client.data.is_none(),
        "issue #785 wires execution separately from the data-only #784 client"
    );
    let execution = client
        .execution
        .as_ref()
        .and_then(toml::Value::as_table)
        .expect("hyperliquid_execution must declare an [execution] table");
    assert_eq!(
        execution
            .get(stringify!(environment))
            .and_then(toml::Value::as_str),
        Some("mainnet")
    );
    assert_eq!(
        execution
            .get(stringify!(execution_mode))
            .and_then(toml::Value::as_str),
        Some("master_account_api_wallet")
    );
    assert_eq!(
        execution
            .get(stringify!(product_surfaces))
            .and_then(toml::Value::as_array)
            .expect("hyperliquid_execution product_surfaces must be an array")
            .iter()
            .map(toml::Value::as_str)
            .collect::<Vec<_>>(),
        vec![
            Some("standard_perps"),
            Some("spot"),
            Some("hip3_builder_perps"),
            Some("hip4_outcomes"),
        ]
    );
    assert_eq!(
        execution
            .get(stringify!(outcome_settlement_poll_secs))
            .and_then(toml::Value::as_integer),
        Some(30)
    );
    let live_submit = execution
        .get(stringify!(live_submit))
        .and_then(toml::Value::as_table)
        .expect("hyperliquid_execution must declare per-surface live-submit gates");
    for (surface, approval_id, approval_artifact_path, product_proof_artifact_path) in [
        (
            "standard_perps",
            "hl-standard-perps-mainnet-001",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-standard-perps-live-submit-approval.json",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-standard-perps-product-submit-proof.json",
        ),
        (
            "spot",
            "hl-spot-mainnet-001",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-spot-live-submit-approval.json",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-spot-product-submit-proof.json",
        ),
        (
            "hip3_builder_perps",
            "hl-hip3-builder-perps-mainnet-001",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-hip3-builder-perps-live-submit-approval.json",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-hip3-builder-perps-product-submit-proof.json",
        ),
        (
            "hip4_outcomes",
            "hl-hip4-outcomes-mainnet-001",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-hip4-outcomes-live-submit-approval.json",
            "/srv/bolt-v2/var/bolt-v3-live/operator/hyperliquid-hip4-outcomes-product-submit-proof.json",
        ),
    ] {
        let surface_config = live_submit
            .get(surface)
            .and_then(toml::Value::as_table)
            .unwrap_or_else(|| panic!("live_submit.{surface} must be configured"));
        assert_eq!(
            surface_config
                .get(stringify!(approval_id))
                .and_then(toml::Value::as_str),
            Some(approval_id)
        );
        assert_eq!(
            surface_config
                .get(stringify!(approval_artifact_path))
                .and_then(toml::Value::as_str),
            Some(approval_artifact_path)
        );
        assert_eq!(
            surface_config
                .get(stringify!(approval_artifact_max_bytes))
                .and_then(toml::Value::as_integer),
            Some(65536)
        );
        assert_eq!(
            surface_config
                .get(stringify!(max_order_count))
                .and_then(toml::Value::as_integer),
            Some(1)
        );
        assert_eq!(
            surface_config
                .get(stringify!(max_order_notional))
                .and_then(toml::Value::as_str),
            Some("10.00")
        );
        assert_eq!(
            surface_config
                .get(stringify!(product_proof_artifact_path))
                .and_then(toml::Value::as_str),
            Some(product_proof_artifact_path)
        );
        assert_eq!(
            surface_config
                .get(stringify!(product_proof_artifact_sha256))
                .and_then(toml::Value::as_str),
            Some("0000000000000000000000000000000000000000000000000000000000000000")
        );
        assert_eq!(
            surface_config
                .get(stringify!(product_proof_artifact_max_bytes))
                .and_then(toml::Value::as_integer),
            Some(65536)
        );
    }

    let secrets = client
        .secrets
        .as_ref()
        .and_then(toml::Value::as_table)
        .expect("hyperliquid_execution must declare SSM-backed [secrets]");
    assert_eq!(
        secrets
            .get(stringify!(private_key_ssm_path))
            .and_then(toml::Value::as_str),
        Some("/bolt/hyperliquid/master_api_wallet/private_key")
    );
    assert_eq!(
        secrets
            .get(stringify!(account_address_ssm_path))
            .and_then(toml::Value::as_str),
        Some("/bolt/hyperliquid/master_api_wallet/account_address")
    );
    for forbidden in [
        stringify!(private_key),
        stringify!(account_address),
        stringify!(vault_address),
    ] {
        assert!(
            !secrets.contains_key(forbidden),
            "Hyperliquid execution secrets must stay SSM-only; found raw field {forbidden}"
        );
    }
    assert_eq!(
        secrets.len(),
        2,
        "hyperliquid_execution must not introduce another secret source"
    );

    for strategy in &loaded.strategies {
        assert_eq!(
            strategy.config.execution_client_id,
            ClientId::from("polymarket_main"),
            "{} must not route to Hyperliquid until the live-submit packet is operator-approved",
            strategy.relative_path
        );
    }
}

#[test]
fn root_config_resolves_single_hyperliquid_execution_client_from_master_api_wallet() {
    use bolt_v2::{
        bolt_v3_config::load_bolt_v3_config, bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    };

    let loaded = load_bolt_v3_config(&support::repo_path("config/root.toml"))
        .expect("root.toml should load with the Hyperliquid execution client");
    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/polymarket/private-key" => {
            Ok("0x4242424242424242424242424242424242424242424242424242424242424242".to_string())
        }
        "/bolt/polymarket/api-key" => Ok("polymarket-api-key".to_string()),
        "/bolt/polymarket/api-secret" => Ok("YWJj".to_string()),
        "/bolt/polymarket/api-passphrase" => Ok("polymarket-passphrase".to_string()),
        "/bolt/binance/api-key" => Ok("binance-api-key".to_string()),
        "/bolt/binance/api-secret" => {
            Ok("MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".to_string())
        }
        "/bolt/testnet/chainlink/api-key" => Ok("chainlink-api-key".to_string()),
        "/bolt/testnet/chainlink/api-secret" => Ok("chainlink-api-secret".to_string()),
        "/bolt/polyresearch/api-key" => Ok("polyresearch-api-key".to_string()),
        "/bolt/hyperliquid/master_api_wallet/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        "/bolt/hyperliquid/master_api_wallet/private_key" => {
            Ok("0x1111111111111111111111111111111111111111111111111111111111111111".to_string())
        }
        _ => Err("unexpected root SSM path"),
    })
    .expect("root Hyperliquid execution client should resolve through the master API wallet");

    assert!(
        resolved.clients.contains_key("hyperliquid_execution"),
        "hyperliquid_execution should resolve through the root SSM resolver"
    );
    let client = loaded.root.clients.get("hyperliquid_execution").unwrap();
    let secrets = client.secrets.as_ref().unwrap().as_table().unwrap();
    assert_eq!(
        secrets
            .get(stringify!(private_key_ssm_path))
            .and_then(toml::Value::as_str),
        Some("/bolt/hyperliquid/master_api_wallet/private_key")
    );
}
