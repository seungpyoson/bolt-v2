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
        local_dir = "var/audit"
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
