mod support;

use bolt_v2::{config::Config, materialize_live_config};
use std::fs;
use support::{TempCaseDir, runtime_toml_with_reference_venue};
use toml::Value;

#[test]
fn parses_runtime_config_with_optional_streaming_section() {
    let toml = r#"
        strategies = []

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
        output_dir = "var/raw"

        [streaming]
        catalog_path = "var/catalog"
        flush_interval_ms = 1000
        contract_path = "/opt/bolt-v2/contracts/polymarket.toml"
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();

    assert_eq!(cfg.node.timeout_connection_secs, 60);
    assert_eq!(cfg.raw_capture.output_dir, "var/raw");
    assert_eq!(cfg.streaming.catalog_path, "var/catalog");
    assert_eq!(cfg.streaming.flush_interval_ms, 1000);
    assert_eq!(
        cfg.streaming.contract_path.as_deref(),
        Some("/opt/bolt-v2/contracts/polymarket.toml")
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
    assert_eq!(cfg.rulesets[0].event_slug_prefix, "btc-updown-5m-");
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

    fs::write(&path, &toml).unwrap();
    let cfg: Config = toml::from_str(&toml).expect("runtime config should parse");
    let errors = bolt_v2::validate::validate_runtime(&cfg);
    assert!(
        errors
            .iter()
            .any(|error| error.field == "reference.venues[0].chainlink"),
        "{errors:?}"
    );
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

    fs::write(&path, &toml).unwrap();
    let cfg: Config = toml::from_str(&toml).expect("runtime config should parse");
    let errors = bolt_v2::validate::validate_runtime(&cfg);
    assert!(
        errors
            .iter()
            .any(|error| error.field == "reference.chainlink"),
        "{errors:?}"
    );
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

    fs::write(&path, &toml).unwrap();
    let cfg: Config = toml::from_str(&toml).expect("runtime config should parse");
    let errors = bolt_v2::validate::validate_runtime(&cfg);
    assert!(
        errors
            .iter()
            .any(|error| error.field == "reference.chainlink"),
        "{errors:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error.field == "reference.venues[0].chainlink"),
        "{errors:?}"
    );
}
