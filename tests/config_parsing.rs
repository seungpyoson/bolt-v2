use bolt_v2::config::Config;

#[test]
fn parses_minimal_data_lake_config() {
    let toml = r#"
        [node]
        name = "bolt-v2"
        trader_id = "TRADER-001"
        account_id = "PM-001"
        client_id = "PM"
        environment = "Live"
        load_state = true
        save_state = true

        [logging]
        stdout_level = "Info"
        file_level = "Off"

        [timeouts]
        connection_secs = 60
        reconciliation_secs = 30
        portfolio_secs = 10
        disconnection_secs = 10
        post_stop_delay_secs = 10
        shutdown_delay_secs = 5

        [venue]
        event_slug = "election-2028"
        instrument_id = "0xabc-123"
        reconciliation_enabled = true
        reconciliation_lookback_mins = 60
        subscribe_new_markets = true

        [strategy]
        strategy_id = "EXEC-001"
        log_data = true
        order_qty = "1"
        tob_offset_ticks = 1
        use_post_only = true

        [wallet]
        signature_type_id = 0
        funder = "0xdeadbeef"

        [wallet.secrets]
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
    "#;

    let cfg: Config = toml::from_str(toml).unwrap();

    assert_eq!(cfg.raw_capture.output_dir, "var/raw");
    assert!(cfg.venue.subscribe_new_markets);
    assert_eq!(cfg.streaming.catalog_path, "var/catalog");
}
