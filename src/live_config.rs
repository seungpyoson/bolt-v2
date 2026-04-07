include!("live_config_impl.rs");

#[cfg(test)]
mod tests {
    use super::{LiveLocalConfig, render_runtime_config};
    use crate::config::Config;

    #[test]
    fn rendered_live_config_uses_wrapper_schema_for_operator_lane() {
        let raw = r#"
[node]
name = "BOLT-V2-001"
trader_id = "BOLT-001"

[logging]

[timeouts]

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[strategy]

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"
"#;
        let source_path = std::path::Path::new("config/live.local.toml");
        let output_path = std::path::Path::new("config/live.toml");
        let input: LiveLocalConfig =
            toml::from_str(raw).expect("inline live local input should parse");
        let rendered = render_runtime_config(&input, source_path, output_path)
            .expect("rendered config should serialize");
        let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");

        assert!(rendered.contains("# Source of truth: config/live.local.toml"));
        assert!(rendered.contains(
            "# Regenerate with: cargo run --bin render_live_config -- --input config/live.local.toml --output config/live.toml"
        ));

        assert_eq!(cfg.node.environment, "Live");
        assert!(!cfg.node.load_state);
        assert!(!cfg.node.save_state);
        assert_eq!(cfg.logging.stdout_level, "Info");
        assert_eq!(cfg.logging.file_level, "Debug");
        assert_eq!(cfg.node.timeout_connection_secs, 60);
        assert_eq!(cfg.node.timeout_reconciliation_secs, 60);
        assert_eq!(cfg.node.timeout_portfolio_secs, 10);
        assert_eq!(cfg.node.timeout_disconnection_secs, 10);
        assert_eq!(cfg.node.delay_post_stop_secs, 5);
        assert_eq!(cfg.node.delay_shutdown_secs, 5);

        assert_eq!(cfg.data_clients.len(), 1);
        assert_eq!(cfg.data_clients[0].name, "POLYMARKET");
        assert_eq!(cfg.data_clients[0].kind, "polymarket");
        assert_eq!(
            cfg.data_clients[0].config["subscribe_new_markets"]
                .as_bool()
                .expect("subscribe_new_markets should be a bool"),
            false
        );
        assert_eq!(
            cfg.data_clients[0].config["update_instruments_interval_mins"]
                .as_integer()
                .expect("update interval should be an integer"),
            60
        );
        assert_eq!(
            cfg.data_clients[0].config["ws_max_subscriptions"]
                .as_integer()
                .expect("ws max subscriptions should be an integer"),
            200
        );
        assert_eq!(
            cfg.data_clients[0].config["event_slugs"]
                .as_array()
                .expect("event slugs should be an array")
                .len(),
            1
        );

        assert_eq!(cfg.exec_clients.len(), 1);
        assert_eq!(cfg.exec_clients[0].name, "POLYMARKET");
        assert_eq!(cfg.exec_clients[0].kind, "polymarket");
        assert_eq!(
            cfg.exec_clients[0].config["account_id"]
                .as_str()
                .expect("account id should be present"),
            "POLYMARKET-001"
        );
        assert_eq!(
            cfg.exec_clients[0].config["signature_type"]
                .as_integer()
                .expect("signature type should be present"),
            2
        );
        assert_eq!(cfg.exec_clients[0].secrets.region, "eu-west-1");

        assert_eq!(cfg.strategies.len(), 1);
        assert_eq!(cfg.strategies[0].kind, "exec_tester");
        assert_eq!(
            cfg.strategies[0].config["client_id"]
                .as_str()
                .expect("client id should be present"),
            "POLYMARKET"
        );
        assert_eq!(
            cfg.strategies[0].config["order_qty"]
                .as_str()
                .expect("order qty should be present"),
            "5"
        );
        assert_eq!(
            cfg.strategies[0].config["tob_offset_ticks"]
                .as_integer()
                .expect("offset should be present"),
            5
        );
        assert_eq!(
            cfg.strategies[0].config["use_post_only"]
                .as_bool()
                .expect("use_post_only should be present"),
            true
        );
    }
}
