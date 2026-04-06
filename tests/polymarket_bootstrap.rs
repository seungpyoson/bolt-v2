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

#[test]
fn builds_live_node_and_registers_exec_tester_on_real_polymarket_seam() {
    let cfg = Config::load(std::path::Path::new("config/examples/polymarket-exec-tester.toml"))
        .expect("example config should load");

    let trader_id = TraderId::from(cfg.node.trader_id.as_str());
    let environment = Environment::Live;
    let log_config = LoggerConfig {
        stdout_level: LevelFilter::Info,
        fileout_level: LevelFilter::Debug,
        ..Default::default()
    };

    let (data_factory, data_config) = polymarket::build_data_client(&cfg.data_clients[0].config)
        .expect("data config should translate");

    let dummy = ResolvedPolymarketSecrets {
        private_key: "0x1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        api_key: "test_api_key".to_string(),
        api_secret: "dGVzdF9zZWNyZXQ=".to_string(),
        passphrase: "test_pass".to_string(),
    };
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

    let _run_future = node.run();
}
