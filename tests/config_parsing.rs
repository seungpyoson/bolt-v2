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
    use bolt_v2::bolt_v3_config::{
        ArchetypeOrderType, ArchetypeTimeInForce, RuntimeMode, StrategyArchetype, TargetKind,
        VenueKind, load_bolt_v3_config,
    };

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("minimal v3 config should load");

    assert_eq!(loaded.root.schema_version, 1);
    assert_eq!(loaded.root.trader_id, "BOLT-001");
    assert_eq!(loaded.root.runtime.mode, RuntimeMode::Live);
    assert_eq!(
        loaded.root.venues["polymarket_main"].kind,
        VenueKind::Polymarket
    );
    assert_eq!(
        loaded.root.venues["binance_reference"].kind,
        VenueKind::Binance
    );
    assert!(loaded.root.venues["polymarket_main"].execution.is_some());
    assert!(loaded.root.venues["binance_reference"].execution.is_none());

    assert_eq!(loaded.strategies.len(), 1);
    let strategy = &loaded.strategies[0].config;
    assert_eq!(
        strategy.strategy_archetype,
        StrategyArchetype::BinaryOracleEdgeTaker
    );
    assert_eq!(strategy.target.kind, TargetKind::RotatingMarket);
    assert_eq!(strategy.target.cadence_seconds, 300);
    assert_eq!(
        strategy.parameters.entry_order.order_type,
        ArchetypeOrderType::Limit
    );
    assert_eq!(
        strategy.parameters.entry_order.time_in_force,
        ArchetypeTimeInForce::Fok
    );
    assert_eq!(
        strategy.parameters.exit_order.order_type,
        ArchetypeOrderType::Market
    );
    assert_eq!(
        strategy.parameters.exit_order.time_in_force,
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

    let strategy_error =
        toml::from_str::<bolt_v2::bolt_v3_config::BoltV3StrategyConfig>(&mutated_strategy)
            .expect_err("unknown strategy field should fail to parse")
            .to_string();
    assert!(
        strategy_error.contains("bogus_parameter"),
        "error should name the unknown strategy field, got: {strategy_error}"
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
        let result = build_bolt_v3_live_node_with(&loaded, |var| var == forbidden);
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
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
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
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
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
bypass = false
max_order_submit_count = 20
max_order_submit_interval_seconds = 1
max_order_modify_count = 20
max_order_modify_interval_seconds = 1
default_max_notional_per_order = "10.00"

[logging]
standard_output_level = "INFO"
file_level = "INFO"
log_directory = "/var/log/bolt"

[persistence]
state_directory = "/var/lib/bolt/state"
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
