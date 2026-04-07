use bolt_v2::{
    clients::polymarket,
    config::Config,
    secrets::ResolvedPolymarketSecrets,
    strategies::exec_tester,
};
use log::LevelFilter;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::TraderId;

fn bootstrap_test_secrets() -> ResolvedPolymarketSecrets {
    ResolvedPolymarketSecrets {
        // Build a syntactically valid EVM key without embedding a credential-shaped literal.
        private_key: format!("0x{}", "1".repeat(64)),
        api_key: "placeholder-api-key-for-tests".to_string(),
        api_secret: "placeholder-api-secret-for-tests".to_string(),
        passphrase: "placeholder-passphrase-for-tests".to_string(),
    }
}

#[test]
fn seam_test_uses_non_secret_placeholders() {
    let dummy = bootstrap_test_secrets();

    assert!(dummy.private_key.starts_with("0x"));
    assert_eq!(dummy.private_key.len(), 66);
    assert!(dummy.private_key[2..].chars().all(|ch| ch == '1'));
    assert_eq!(dummy.api_key, "placeholder-api-key-for-tests");
    assert_eq!(dummy.api_secret, "placeholder-api-secret-for-tests");
    assert_eq!(dummy.passphrase, "placeholder-passphrase-for-tests");
}

#[test]
fn builds_live_node_and_registers_exec_tester_before_polling_run_on_real_polymarket_seam() {
    let rendered = bolt_v2::render_live_config_from_path(
        std::path::Path::new("config/live.local.example.toml"),
        std::path::Path::new("config/live.toml"),
    )
    .expect("tracked operator template should render");
    let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");

    let trader_id = TraderId::from(cfg.node.trader_id.as_str());
    let environment = Environment::Live;
    let log_config = LoggerConfig {
        stdout_level: LevelFilter::Info,
        fileout_level: LevelFilter::Debug,
        ..Default::default()
    };

    let (data_factory, data_config) = polymarket::build_data_client(&cfg.data_clients[0].config)
        .expect("data config should translate");

    let dummy = bootstrap_test_secrets();
    let (exec_factory, exec_config) =
        polymarket::build_exec_client(&cfg.exec_clients[0].config, trader_id, dummy)
            .expect("exec config should translate");

    let mut node = LiveNode::builder(trader_id, environment)
        .expect("builder should construct")
        .with_name(cfg.node.name.clone())
        .with_logging(log_config)
        .with_load_state(cfg.node.load_state)
        .with_save_state(cfg.node.save_state)
        .with_timeout_connection(cfg.node.timeout_connection_secs)
        .with_timeout_reconciliation(cfg.node.timeout_reconciliation_secs)
        .with_timeout_portfolio(cfg.node.timeout_portfolio_secs)
        .with_timeout_disconnection_secs(cfg.node.timeout_disconnection_secs)
        .with_delay_post_stop_secs(cfg.node.delay_post_stop_secs)
        .with_delay_shutdown_secs(cfg.node.delay_shutdown_secs)
        .add_data_client(Some(cfg.data_clients[0].name.clone()), data_factory, data_config)
        .expect("data client should register")
        .add_exec_client(Some(cfg.exec_clients[0].name.clone()), exec_factory, exec_config)
        .expect("exec client should register")
        .build()
        .expect("node should build");

    let strategy = exec_tester::build_exec_tester(&cfg.strategies[0].config)
        .expect("strategy should translate");
    node.add_strategy(strategy)
        .expect("strategy should register");

    // This offline seam test stops at compile-checking the final `run()` call.
    // Polling it would turn the test into a live integration test against external services.
    let _run_future = node.run();
}
