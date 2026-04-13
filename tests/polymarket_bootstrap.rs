mod support;

use std::fs;

use bolt_v2::{
    clients::polymarket, config::Config, materialize_live_config,
    secrets::ResolvedPolymarketSecrets,
};
use log::LevelFilter;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::TraderId;
use support::{TempCaseDir, repo_path};

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
fn builds_live_node_without_pre_registering_exec_tester_in_ruleset_mode() {
    let tempdir = TempCaseDir::new("polymarket-bootstrap");
    let config_path = tempdir.path().join("live.toml");
    materialize_live_config(&repo_path("config/live.local.example.toml"), &config_path)
        .expect("tracked template should materialize");
    let cfg = Config::load(&config_path).expect("materialized config should load");
    assert!(
        !cfg.rulesets.is_empty(),
        "tracked seam config should exercise ruleset mode"
    );

    let trader_id = TraderId::from(cfg.node.trader_id.as_str());
    let environment = Environment::Live;
    let log_config = LoggerConfig {
        stdout_level: LevelFilter::Info,
        fileout_level: LevelFilter::Debug,
        ..Default::default()
    };

    let selector_inputs =
        polymarket::polymarket_ruleset_selectors(&cfg.rulesets).expect("selectors should parse");
    let (data_factory, data_config) =
        polymarket::build_data_client(&cfg.data_clients[0].config, &selector_inputs, None)
            .expect("data config should translate");
    let data_config_debug = format!("{data_config:?}");
    assert!(
        data_config_debug.contains("EventParamsFilter"),
        "ruleset mode should bootstrap the data client from selector-derived Gamma event params: {data_config_debug}"
    );
    assert!(
        !data_config_debug.contains("EventSlugFilter"),
        "ruleset mode should no longer bootstrap from legacy event slug filters: {data_config_debug}"
    );

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
        .add_data_client(
            Some(cfg.data_clients[0].name.clone()),
            data_factory,
            data_config,
        )
        .expect("data client should register")
        .add_exec_client(
            Some(cfg.exec_clients[0].name.clone()),
            exec_factory,
            exec_config,
        )
        .expect("exec client should register")
        .build()
        .expect("node should build");
    let trader = std::rc::Rc::clone(node.kernel().trader());

    assert!(
        trader.borrow().strategy_ids().is_empty(),
        "ruleset mode should leave static strategies unregistered until platform runtime owns them"
    );

    // This offline seam test stops at compile-checking the final `run()` call.
    // Polling it would turn the test into a live integration test against external services.
    let _run_future = node.run();
}

#[test]
fn ruleset_mode_rejects_legacy_event_slugs_during_bootstrap() {
    let tempdir = TempCaseDir::new("polymarket-bootstrap-legacy-event-slugs");
    let generated_path = tempdir.path().join("live.toml");
    materialize_live_config(
        &repo_path("config/live.local.example.toml"),
        &generated_path,
    )
    .expect("tracked template should materialize");

    let mutated = fs::read_to_string(&generated_path)
        .expect("materialized config should be readable")
        .replace(
            "ws_max_subscriptions = 200\n",
            "ws_max_subscriptions = 200\nevent_slugs = 7\n",
        );
    let mutated_path = tempdir.path().join("live-mutated.toml");
    fs::write(&mutated_path, mutated).expect("mutated config should be written");

    let error = Config::load(&mutated_path)
        .expect_err("ruleset mode should reject legacy event_slugs")
        .to_string();

    assert!(
        error.contains("data_clients[0].config.event_slugs"),
        "ruleset mode error should mention legacy event_slugs: {error}"
    );
    assert!(
        error.contains("must be omitted when rulesets are enabled"),
        "ruleset mode error should explain the field is forbidden: {error}"
    );
}
