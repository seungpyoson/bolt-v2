mod support;

use bolt_v2::{config::Config, materialize_live_config};
use std::fs;
use support::{TempCaseDir, runtime_toml_with_reference_venue};
use toml::Value;

#[test]
fn parses_runtime_config_with_optional_streaming_section() {
    let toml = r#"
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

        [streaming]
        catalog_path = "var/catalog"
        flush_interval_ms = 1000
        contract_path = "/opt/bolt-v2/contracts/polymarket.toml"
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();

    assert_eq!(cfg.node.timeout_connection_secs, 60);
    assert_eq!(cfg.exec_engine.position_check_interval_secs, None);
    assert_eq!(cfg.raw_capture.output_dir, "/srv/bolt-v2/var/raw");
    assert_eq!(cfg.streaming.catalog_path, "var/catalog");
    assert_eq!(cfg.streaming.flush_interval_ms, 1000);
    assert_eq!(
        cfg.streaming.contract_path.as_deref(),
        Some("/opt/bolt-v2/contracts/polymarket.toml")
    );
}

#[test]
fn runtime_config_defaults_raw_capture_output_dir_to_srv_path() {
    let toml = r#"
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
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();

    assert_eq!(cfg.raw_capture.output_dir, "/srv/bolt-v2/var/raw");
}

#[test]
fn runtime_config_parses_ruleset_selector_table() {
    let toml = r#"
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

        [reference.binance]
        region = "eu-west-1"
        api_key = "/bolt/binance/api-key"
        api_secret = "/bolt/binance/api-secret"
        environment = "Mainnet"
        product_types = ["SPOT"]
        instrument_status_poll_secs = 3600

        [[reference.venues]]
        name = "BINANCE-BTC"
        type = "binance"
        instrument_id = "BTCUSDT.BINANCE"
        base_weight = 0.35
        stale_after_ms = 1500
        disable_after_ms = 5000

        [[rulesets]]
        id = "PRIMARY"
        venue = "polymarket"
        resolution_basis = "binance_btcusdt_1m"
        min_time_to_expiry_secs = 60
        max_time_to_expiry_secs = 900
        min_liquidity_num = 1000
        require_accepting_orders = true
        freeze_before_end_secs = 90
        selector_poll_interval_ms = 1000
        candidate_load_timeout_secs = 30

        [rulesets.selector]
        tag_slug = "bitcoin"
        event_slug_prefix = "btc-updown"

        [audit]
        local_dir = "/srv/bolt-v2/var/audit"
        s3_uri = "s3://bolt-runtime-history/phase1"
        ship_interval_secs = 30
        upload_attempt_timeout_secs = 30
        roll_max_bytes = 1048576
        roll_max_secs = 300
        max_local_backlog_bytes = 10485760
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();

    assert!(cfg.strategies.is_empty());
    assert_eq!(cfg.rulesets[0].id, "PRIMARY");
    assert_eq!(
        cfg.rulesets[0].selector["tag_slug"].as_str(),
        Some("bitcoin")
    );
    assert_eq!(
        cfg.rulesets[0].selector["event_slug_prefix"].as_str(),
        Some("btc-updown")
    );
}

#[test]
fn runtime_config_parses_optional_exec_engine_position_check_interval() {
    let toml = r#"
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

        [exec_engine]
        position_check_interval_secs = 17

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
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();

    assert_eq!(cfg.exec_engine.position_check_interval_secs, Some(17.0));
}

#[test]
fn runtime_config_load_rejects_non_positive_exec_engine_position_check_interval() {
    let tempdir = TempCaseDir::new("exec-engine-position-check-invalid");
    let path = tempdir.path().join("live.toml");
    let toml = r#"
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

        [exec_engine]
        position_check_interval_secs = 0

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
    "#;

    fs::write(&path, toml).expect("runtime config should be written");

    let error = Config::load(&path).expect_err("non-positive position check interval must fail");

    assert!(
        error
            .to_string()
            .contains("exec_engine.position_check_interval_secs")
    );
}

#[test]
fn rendered_operator_config_can_enable_streaming_without_changing_runtime_schema() {
    let tempdir = TempCaseDir::new("config-parsing");
    std::fs::write(
        tempdir.path().join("Cargo.toml"),
        "[package]\nname = \"temp\"\n",
    )
    .unwrap();
    let config_dir = tempdir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let input_path = config_dir.join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let toml = r#"
        [node]
        name = "bolt-v2"
        trader_id = "TRADER-001"

        [polymarket]
        event_slug = "btc-updown-5m"
        instrument_id = "0xabc-12345678901234567890.POLYMARKET"
        account_id = "POLYMARKET-001"
        funder = "0xdeadbeef"

        [secrets]
        pk = "/pk"
        api_key = "/key"
        api_secret = "/secret"
        passphrase = "/pass"

        [raw_capture]
        output_dir = "/srv/bolt-v2/var/raw"

        [streaming]
        catalog_path = "var/catalog"
        flush_interval_ms = 250
        contract_path = "contracts/polymarket.toml"
    "#;

    fs::write(&input_path, toml).unwrap();
    let error = materialize_live_config(&input_path, &output_path)
        .expect_err("non-phase1 operator config should fail closed")
        .to_string();

    assert!(
        error.contains("at least one ruleset or strategy"),
        "expected fail-closed runtime-shape error, got: {error}"
    );
    assert!(!output_path.exists());
}

#[test]
fn rendered_runtime_toml_preserves_phase1_platform_values() {
    let tempdir = TempCaseDir::new("phase1-config-parsing");
    let input_path = tempdir.path().join("live.local.toml");
    let output_path = tempdir.path().join("live.toml");
    let toml = r#"
        [node]
        name = "BOLT-V2-001"
        trader_id = "BOLT-001"

        [polymarket]
        instrument_id = "0xabc-123.POLYMARKET"
        account_id = "POLYMARKET-001"
        funder = "0xabc"
        signature_type = 2

        [secrets]
        region = "eu-west-1"
        pk = "/bolt/poly/pk"
        api_key = "/bolt/poly/key"
        api_secret = "/bolt/poly/secret"
        passphrase = "/bolt/poly/passphrase"

        [raw_capture]
        output_dir = "/srv/bolt-v2/var/raw"

        [reference]
        publish_topic = "platform.reference.default"
        min_publish_interval_ms = 100

        [reference.binance]
        region = "eu-west-1"
        api_key = "/bolt/binance/api-key"
        api_secret = "/bolt/binance/api-secret"
        environment = "Mainnet"
        product_types = ["SPOT"]
        instrument_status_poll_secs = 3600

        [[reference.venues]]
        name = "BINANCE-BTC"
        type = "binance"
        instrument_id = "BTCUSDT.BINANCE"
        base_weight = 0.35
        stale_after_ms = 1500
        disable_after_ms = 5000

        [[rulesets]]
        id = "PRIMARY"
        venue = "polymarket"
        resolution_basis = "binance_btcusdt_1m"
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
    "#;

    fs::write(&input_path, toml).unwrap();
    materialize_live_config(&input_path, &output_path).unwrap();
    let rendered = fs::read_to_string(&output_path).unwrap();
    let value: Value = toml::from_str(&rendered).unwrap();

    assert!(
        !rendered.contains("event_slugs"),
        "ruleset-backed runtime config should not emit event slugs: {rendered}"
    );
    assert!(
        value.get("strategies").is_none(),
        "ruleset-backed runtime config should omit runtime strategy templates: {rendered}"
    );
    assert_eq!(
        value["reference"]["venues"][0]["type"].as_str(),
        Some("binance")
    );
    assert_eq!(value["rulesets"][0]["id"].as_str(), Some("PRIMARY"));
    assert_eq!(
        value["rulesets"][0]["require_accepting_orders"].as_bool(),
        Some(true)
    );
    assert_eq!(
        value["rulesets"][0]["selector_poll_interval_ms"].as_integer(),
        Some(250)
    );
    assert_eq!(
        value["rulesets"][0]["candidate_load_timeout_secs"].as_integer(),
        Some(12)
    );
    assert_eq!(
        value["audit"]["s3_uri"].as_str(),
        Some("s3://bolt-runtime-history/phase1")
    );
    assert_eq!(
        value["audit"]["upload_attempt_timeout_secs"].as_integer(),
        Some(45)
    );
}

#[test]
fn parses_runtime_config_with_nested_chainlink_reference_settings() {
    let toml = runtime_toml_with_reference_venue(
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 5"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8"#,
        "chainlink_btcusd",
    );

    let cfg: Config = toml::from_str(&toml).unwrap();
    let shared = cfg
        .reference
        .chainlink
        .as_ref()
        .expect("shared chainlink settings should parse");
    let chainlink = cfg.reference.venues[0]
        .chainlink
        .as_ref()
        .expect("chainlink settings should parse");

    assert_eq!(shared.region, "us-east-1");
    assert_eq!(shared.api_key, "/bolt/chainlink/api_key");
    assert_eq!(shared.api_secret, "/bolt/chainlink/api_secret");
    assert_eq!(shared.ws_url, "wss://streams.chain.link");
    assert_eq!(shared.ws_reconnect_alert_threshold, 5);
    assert_eq!(
        chainlink.feed_id,
        "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
    );
    assert_eq!(chainlink.price_scale, 8);
}

#[test]
fn runtime_config_rejects_incomplete_chainlink_shared_settings_at_parse_time() {
    let toml = runtime_toml_with_reference_venue(
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link""#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8"#,
        "chainlink_btcusd",
    );

    let error = toml::from_str::<Config>(&toml)
        .expect_err("incomplete chainlink shared settings should fail to parse")
        .to_string();

    assert!(error.contains("ws_reconnect_alert_threshold"));
}

#[test]
fn runtime_config_rejects_chainlink_venue_without_nested_chainlink_settings() {
    let tempdir = TempCaseDir::new("runtime-chainlink-missing");
    let path = tempdir.path().join("live.toml");
    let toml = runtime_toml_with_reference_venue(
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 5"#,
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000"#,
        "chainlink_btcusd",
    );

    fs::write(&path, toml).unwrap();
    let error = Config::load(&path)
        .expect_err("chainlink runtime config without nested settings should fail")
        .to_string();

    assert!(error.contains("reference.venues[0].chainlink"));
}

#[test]
fn runtime_config_rejects_chainlink_venue_without_shared_chainlink_settings() {
    let tempdir = TempCaseDir::new("runtime-chainlink-missing-shared");
    let path = tempdir.path().join("live.toml");
    let toml = runtime_toml_with_reference_venue(
        "",
        r#"[[reference.venues]]
name = "CHAINLINK-BTC"
type = "chainlink"
instrument_id = "BTCUSD.CHAINLINK"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8"#,
        "chainlink_btcusd",
    );

    fs::write(&path, toml).unwrap();
    let error = Config::load(&path)
        .expect_err("chainlink runtime config without shared settings should fail")
        .to_string();

    assert!(error.contains("reference.chainlink"));
}

#[test]
fn runtime_config_rejects_orphaned_chainlink_settings_on_non_chainlink_venue() {
    let tempdir = TempCaseDir::new("runtime-chainlink-orphan");
    let path = tempdir.path().join("live.toml");
    let toml = runtime_toml_with_reference_venue(
        r#"[reference.chainlink]
region = "us-east-1"
api_key = "/bolt/chainlink/api_key"
api_secret = "/bolt/chainlink/api_secret"
ws_url = "wss://streams.chain.link"
ws_reconnect_alert_threshold = 5"#,
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000
[reference.venues.chainlink]
feed_id = "0x00036b4aa7e57ca7b68ae1bf45653f56b656fd3aa335ef7fae696b663f1b8472"
price_scale = 8"#,
        "binance_btcusdt_1m",
    );

    fs::write(&path, toml).unwrap();
    let error = Config::load(&path)
        .expect_err("orphaned chainlink settings should fail")
        .to_string();

    assert!(error.contains("reference.chainlink"));
    assert!(error.contains("reference.venues[0].chainlink"));
}

#[test]
fn runtime_config_rejects_unknown_ruleset_field_at_parse_time() {
    let toml = runtime_toml_with_reference_venue(
        "",
        r#"[[reference.venues]]
name = "BINANCE-BTC"
type = "binance"
instrument_id = "BTCUSDT.BINANCE"
base_weight = 1.0
stale_after_ms = 1500
disable_after_ms = 5000"#,
        "binance_btcusdt_1m",
    )
    .replace(
        "candidate_load_timeout_secs = 12",
        "candidate_load_timeout_secs = 12\nselector_poll_intrvl_ms = 250",
    );

    let error = toml::from_str::<Config>(&toml)
        .expect_err("unknown ruleset field should fail to parse")
        .to_string();

    assert!(error.contains("selector_poll_intrvl_ms"));
}

#[test]
fn parses_minimal_bolt_v3_root_and_strategy_config() {
    use bolt_v2::bolt_v3_archetypes::binary_oracle_edge_taker::{
        ArchetypeOrderType, ArchetypeTimeInForce, ParametersBlock,
    };
    use bolt_v2::bolt_v3_config::{RuntimeMode, load_bolt_v3_config};
    use bolt_v2::bolt_v3_market_families::updown::{TargetBlock, TargetKind};

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("minimal v3 config should load");

    assert_eq!(loaded.root.schema_version, 1);
    assert_eq!(loaded.root.trader_id, "BOLT-001");
    assert_eq!(loaded.root.runtime.mode, RuntimeMode::Live);
    assert_eq!(
        loaded.root.venues["polymarket_main"].kind.as_str(),
        "polymarket"
    );
    assert_eq!(
        loaded.root.venues["binance_reference"].kind.as_str(),
        "binance"
    );
    assert!(loaded.root.venues["polymarket_main"].execution.is_some());
    assert!(loaded.root.venues["binance_reference"].execution.is_none());

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
    assert_eq!(target.cadence_seconds, 300);
    let parameters: ParametersBlock = strategy
        .parameters
        .clone()
        .try_into()
        .expect("fixture parameters block should deserialize as binary_oracle_edge_taker");
    assert_eq!(parameters.entry_order.order_type, ArchetypeOrderType::Limit);
    assert_eq!(
        parameters.entry_order.time_in_force,
        ArchetypeTimeInForce::Fok
    );
    assert_eq!(parameters.exit_order.order_type, ArchetypeOrderType::Market);
    assert_eq!(
        parameters.exit_order.time_in_force,
        ArchetypeTimeInForce::Ioc
    );
    assert!(strategy.reference_data.contains_key("primary"));
    assert_eq!(
        strategy.reference_data["primary"].venue,
        "binance_reference"
    );
}

#[test]
fn rejects_unknown_bolt_v3_config_fields() {
    use bolt_v2::bolt_v3_config::BoltV3RootConfig;

    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = fixture.replace(
        "schema_version = 1",
        "schema_version = 1\nunexpected_root_field = \"nope\"",
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
        .try_into::<bolt_v2::bolt_v3_archetypes::binary_oracle_edge_taker::ParametersBlock>()
        .expect_err("unknown field inside [parameters] should fail archetype typed deserialization")
        .to_string();
    assert!(
        parameters_error.contains("bogus_parameter"),
        "archetype deserialization error should name the unknown strategy field, got: {parameters_error}"
    );
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
                assert_eq!(report.findings[0].venue_key, "polymarket_main");
                assert_eq!(report.findings[0].env_var, forbidden);
            }
            other => panic!("expected ForbiddenEnv error, got {other:?}"),
        }
    }
}

#[test]
fn rejects_polymarket_execution_venue_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

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
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
default_max_notional_per_order = "10.00"

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
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[execution]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for polymarket execution venue, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_reference_data_venue_missing_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

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
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
default_max_notional_per_order = "10.00"

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

[venues.binance_reference]
kind = "binance"

[venues.binance_reference.data]
product_types = ["spot"]
environment = "mainnet"
base_url_http = "https://binance.test.invalid/http"
base_url_ws = "wss://binance.test.invalid/ws"
instrument_status_poll_seconds = 3600
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("binance-data-only TOML should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[data]")
            && m.contains("required [secrets] block")),
        "expected missing-secrets failure for binance reference-data venue, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_venue_numeric_fields_at_zero() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

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
reconciliation_lookback_mins = 0
timeout_portfolio_seconds = 10
timeout_disconnection_seconds = 10
delay_post_stop_seconds = 5
timeout_shutdown_seconds = 10

[risk]
default_max_notional_per_order = "10.00"

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

[venues.polymarket_main.data]
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
base_url_gamma = "https://gamma-api.polymarket.com"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 0
ws_timeout_seconds = 0
subscribe_new_markets = false
update_instruments_interval_minutes = 0
websocket_max_subscriptions_per_connection = 0

[venues.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder_address = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_seconds = 0
max_retries = 0
retry_delay_initial_milliseconds = 0
retry_delay_max_milliseconds = 0
ack_timeout_seconds = 0

[venues.polymarket_main.secrets]
private_key_ssm_path = "/bolt/polymarket_main/private_key"
api_key_ssm_path = "/bolt/polymarket_main/api_key"
api_secret_ssm_path = "/bolt/polymarket_main/api_secret"
passphrase_ssm_path = "/bolt/polymarket_main/passphrase"
"#;

    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket bounds TOML should parse");
    let messages = validate_root_only(&root);
    let expected = [
        "venues.polymarket_main.data.http_timeout_seconds must be a positive integer",
        "venues.polymarket_main.data.ws_timeout_seconds must be a positive integer",
        "venues.polymarket_main.data.update_instruments_interval_minutes must be a positive integer",
        "venues.polymarket_main.data.websocket_max_subscriptions_per_connection must be a positive integer",
        "venues.polymarket_main.execution.http_timeout_seconds must be a positive integer",
        "venues.polymarket_main.execution.max_retries must be a positive integer",
        "venues.polymarket_main.execution.retry_delay_initial_milliseconds must be a positive integer",
        "venues.polymarket_main.execution.retry_delay_max_milliseconds must be a positive integer",
        "venues.polymarket_main.execution.ack_timeout_seconds must be a positive integer",
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
            .replace("schema_version = 1", "schema_version = 2");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated_root).expect("mutated root should parse with raw u32");
    let root_messages = validate_root_only(&root);
    assert!(
        root_messages
            .iter()
            .any(|m| m.contains("root schema_version=2 is unsupported")),
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
    .replace("schema_version = 1", "schema_version = 7");
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

fn replace_in_fixture_root(needle: &str, replacement: &str) -> String {
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    assert!(
        fixture.contains(needle),
        "fixture must contain `{needle}` for this validation test to mutate"
    );
    fixture.replace(needle, replacement)
}

#[test]
fn rejects_orphan_secrets_block_without_data_or_execution() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "[venues.binance_reference.data]\nproduct_types = [\"spot\"]\nenvironment = \"mainnet\"\nbase_url_http = \"https://api.binance.com\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_http\nbase_url_ws = \"wss://stream.binance.com:9443/ws\" # NT: nautilus_binance::config::BinanceDataClientConfig.base_url_ws\ninstrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs\n\n",
        "",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("orphan-secrets fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("[secrets]")
            && m.contains("no [data] block is configured")),
        "expected orphan-secrets validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_ssm_paths_missing_leading_slash() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "api_key_ssm_path = \"/bolt/binance_reference/api_key\"",
        "api_key_ssm_path = \"bolt/binance_reference/api_key\"",
    );
    let root: BoltV3RootConfig = toml::from_str(&mutated).expect("ssm-path mutation should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("api_key_ssm_path")
            && m.contains("absolute-style SSM parameter path starting with `/`")),
        "expected SSM-path leading-slash validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_funder_address_with_invalid_evm_syntax() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"",
        "funder_address = \"0xZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("invalid-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder_address")
            && m.contains("not a valid EVM public address")),
        "expected EVM-syntax validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_funder_address_zero_address() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"",
        "funder_address = \"0x0000000000000000000000000000000000000000\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder_address")
            && m.contains("zero address")),
        "expected zero-address validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_missing_funder_address_for_poly_proxy_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"\n",
        "",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("missing-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("funder_address")
            && m.contains("required when signature_type is `poly_proxy` or `poly_gnosis_safe`")),
        "expected required-funder validation error, got: {messages:#?}"
    );
}

#[test]
fn allows_missing_funder_address_for_eoa_signature_type() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let without_funder = replace_in_fixture_root(
        "funder_address = \"0x1111111111111111111111111111111111111111\"\n",
        "",
    );
    let with_eoa = without_funder.replace(
        "signature_type = \"poly_proxy\"",
        "signature_type = \"eoa\"",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&with_eoa).expect("eoa-without-funder fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        !messages.iter().any(|m| m.contains("funder_address")),
        "EOA signature must allow absent funder_address, got: {messages:#?}"
    );
}

#[test]
fn rejects_binance_data_zero_instrument_status_poll_seconds() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let mutated = replace_in_fixture_root(
        "instrument_status_poll_seconds = 3600 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
        "instrument_status_poll_seconds = 0 # NT: BinanceDataClientConfig.instrument_status_poll_secs",
    );
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("zero-poll-interval fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("binance_reference")
            && m.contains("instrument_status_poll_seconds")
            && m.contains("must be a positive integer")),
        "expected positive-integer poll-interval validation error, got: {messages:#?}"
    );
}

#[test]
fn rejects_polymarket_data_only_venue_with_secrets_block() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let execution_block = "[venues.polymarket_main.execution]\naccount_id = \"POLYMARKET-001\"\nsignature_type = \"poly_proxy\"\nfunder_address = \"0x1111111111111111111111111111111111111111\"\nbase_url_http = \"https://clob.polymarket.com\"\nbase_url_ws = \"wss://ws-subscriptions-clob.polymarket.com/ws/user\"\nbase_url_data_api = \"https://data-api.polymarket.com\"\nhttp_timeout_seconds = 60\nmax_retries = 3\nretry_delay_initial_milliseconds = 250\nretry_delay_max_milliseconds = 2000\nack_timeout_seconds = 5\n\n";
    let mutated = replace_in_fixture_root(execution_block, "");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("polymarket data-only secrets fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages.iter().any(|m| m.contains("polymarket_main")
            && m.contains("[secrets]")
            && m.contains("[execution]")),
        "expected Polymarket data-only secrets validation error, got: {messages:#?}"
    );
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
fn rejects_more_than_one_polymarket_venue_in_current_slice() {
    use bolt_v2::{bolt_v3_config::BoltV3RootConfig, bolt_v3_validate::validate_root_only};

    let extra_venue = "\n\n[venues.polymarket_secondary]\nkind = \"polymarket\"\n\n[venues.polymarket_secondary.data]\nbase_url_http = \"https://test.invalid/clob\"\nbase_url_ws = \"wss://test.invalid/ws/market\"\nbase_url_gamma = \"https://test.invalid/gamma\"\nbase_url_data_api = \"https://test.invalid/data\"\nhttp_timeout_seconds = 60\nws_timeout_seconds = 30\nsubscribe_new_markets = false\nupdate_instruments_interval_minutes = 60\nwebsocket_max_subscriptions_per_connection = 200\n\n[venues.polymarket_secondary.secrets]\nprivate_key_ssm_path = \"/bolt/polymarket_secondary/private_key\"\napi_key_ssm_path = \"/bolt/polymarket_secondary/api_key\"\napi_secret_ssm_path = \"/bolt/polymarket_secondary/api_secret\"\npassphrase_ssm_path = \"/bolt/polymarket_secondary/passphrase\"\n";
    let fixture = std::fs::read_to_string(support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture should be readable");
    let mutated = format!("{fixture}{extra_venue}");
    let root: BoltV3RootConfig =
        toml::from_str(&mutated).expect("two-polymarket-venues fixture should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("at most one [venues.<id>] block per kind")
                && m.contains("polymarket")),
        "expected one-venue-per-kind validation error, got: {messages:#?}"
    );
}
