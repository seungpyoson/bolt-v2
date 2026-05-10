//! Integration tests for the bolt-v3 strategy-registration tracer.
//!
//! This slice proves only `v3 TOML -> existing strategy -> NT registration
//! -> idle node`. It must not introduce live submit, market subscriptions,
//! lifecycle, or reconciliation claims.

mod support;

use bolt_v2::{
    bolt_v3_config::{BoltV3StrategyConfig, LoadedStrategy, load_bolt_v3_config},
    bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
    bolt_v3_validate::validate_strategies,
};
use nautilus_live::node::NodeState;
use nautilus_model::identifiers::StrategyId;

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

fn existing_strategy_multi_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml")
}

#[test]
fn existing_strategy_fixture_selects_root_reference_stream() {
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");

    let stream_id = strategy
        .config
        .parameters
        .get("reference_stream_id")
        .and_then(toml::Value::as_str)
        .expect("existing-strategy fixture should set reference_stream_id");

    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("root should define selected reference stream");

    assert_eq!(stream.publish_topic, "reference.eth_usd");
    assert_eq!(stream.inputs.len(), 1);
    assert_eq!(stream.inputs[0].source_id, "eth_usd_oracle_anchor");
}

#[test]
fn bolt_v3_registers_existing_strategy_and_remains_idle_no_trade() {
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let expected_strategy_id =
        StrategyId::from(loaded.strategies[0].config.strategy_instance_id.as_str());

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register the existing strategy");

    assert_eq!(node.state(), NodeState::Idle);
    let trader = node.kernel().trader();
    let trader = trader.borrow();
    assert_eq!(trader.strategy_count(), 1);
    assert!(
        trader.strategy_ids().contains(&expected_strategy_id),
        "expected existing strategy registered in NT trader, got {:?}",
        trader.strategy_ids()
    );
}

#[test]
fn bolt_v3_registers_two_existing_strategy_instances_and_remains_idle_no_trade() {
    let loaded = load_bolt_v3_config(&existing_strategy_multi_root_fixture())
        .expect("v3 multi-strategy TOML fixture should load");
    let expected_strategy_ids: Vec<StrategyId> = loaded
        .strategies
        .iter()
        .map(|strategy| StrategyId::from(strategy.config.strategy_instance_id.as_str()))
        .collect();

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register both existing strategies");

    assert_eq!(node.state(), NodeState::Idle);
    let trader = node.kernel().trader();
    let trader = trader.borrow();
    assert_eq!(trader.strategy_count(), expected_strategy_ids.len());
    for expected_strategy_id in expected_strategy_ids {
        assert!(
            trader.strategy_ids().contains(&expected_strategy_id),
            "expected existing strategy registered in NT trader, got {:?}",
            trader.strategy_ids()
        );
    }
}

#[test]
fn eth_chainlink_taker_rejects_target_cadence_and_period_mismatch() {
    let loaded_root = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let config_path = support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/strategies/eth_chainlink_taker_period_mismatch.toml",
    );
    let strategy_toml = std::fs::read_to_string(&config_path)
        .expect("mismatch strategy fixture should be readable");
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&strategy_toml).expect("mismatch strategy envelope should parse");
    let loaded_strategy = LoadedStrategy {
        config_path,
        relative_path: "strategies/eth_chainlink_taker_period_mismatch.toml".to_string(),
        config: strategy,
    };

    let messages = validate_strategies(&loaded_root.root, &[loaded_strategy]);

    assert!(
        messages.iter().any(|message| message
            .contains("target.cadence_seconds must match parameters.period_duration_secs")),
        "expected cadence/period mismatch validation error, got {messages:#?}"
    );
}

#[test]
fn bolt_v3_strategy_registration_wiring_has_no_live_order_or_subscription_calls() {
    let sources = [
        support::repo_path("src/bolt_v3_strategy_registration.rs"),
        support::repo_path("src/bolt_v3_strategy_registration/eth_chainlink_taker.rs"),
    ]
    .map(|path| {
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()))
    });

    for forbidden in [
        "submit_order",
        "submit_order_list",
        "order_factory",
        "subscribe_book",
        "subscribe_quotes",
        "subscribe_trades",
    ] {
        assert!(
            sources.iter().all(|source| !source.contains(forbidden)),
            "bolt-v3 strategy-registration wiring must not call `{forbidden}`"
        );
    }
}
