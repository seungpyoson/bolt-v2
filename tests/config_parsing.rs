mod support;

use bolt_v2::{config::Config, materialize_live_config};
use std::fs;
use support::TempCaseDir;

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

        [[strategies]]
        type = "exec_tester"
        [strategies.config]
        strategy_id = "EXEC-001"
        instrument_id = "0xabc-12345678901234567890.POLYMARKET"
        client_id = "POLYMARKET"
        order_qty = "1"
        log_data = true
        tob_offset_ticks = 1
        use_post_only = true

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
        output_dir = "var/raw"

        [streaming]
        catalog_path = "var/catalog"
        flush_interval_ms = 250
        contract_path = "contracts/polymarket.toml"
    "#;

    fs::write(&input_path, toml).unwrap();
    materialize_live_config(&input_path, &output_path).unwrap();
    let rendered = fs::read_to_string(&output_path).unwrap();
    let cfg: Config = toml::from_str(&rendered).unwrap();

    assert!(rendered.contains("[streaming]"));
    assert!(rendered.contains("[raw_capture]"));
    assert_eq!(cfg.node.timeout_connection_secs, 60);
    assert_eq!(cfg.raw_capture.output_dir, "var/raw");
    assert_eq!(cfg.streaming.catalog_path, "var/catalog");
    assert_eq!(cfg.streaming.flush_interval_ms, 250);
    let expected_root = std::fs::canonicalize(tempdir.path()).unwrap();
    assert_eq!(
        cfg.streaming.contract_path.as_deref(),
        Some(
            expected_root
                .join("contracts/polymarket.toml")
                .to_str()
                .unwrap()
        )
    );
}
