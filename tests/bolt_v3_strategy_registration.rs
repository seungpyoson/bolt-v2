//! Integration tests for the bolt-v3 strategy-registration tracer.
//!
//! This slice proves only `v3 TOML -> existing strategy -> NT registration
//! -> idle node`. It must not introduce live submit, market subscriptions,
//! lifecycle, or reconciliation claims.

mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config, bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
};
use nautilus_live::node::NodeState;
use nautilus_model::identifiers::StrategyId;

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

#[test]
fn existing_strategy_fixture_uses_explicit_placeholder_reference_topic() {
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let strategy = loaded
        .strategies
        .first()
        .expect("existing-strategy fixture should load one strategy");

    let topic = strategy
        .config
        .parameters
        .get("reference_publish_topic")
        .and_then(toml::Value::as_str)
        .expect("existing-strategy fixture should set reference_publish_topic");

    assert_eq!(topic, "reference.eth_usd.placeholder-mustchange");
    assert!(
        !topic.contains("chainlink"),
        "v3 tracer topic must not imply a specific reference producer"
    );
}

#[test]
fn bolt_v3_registers_existing_strategy_and_remains_idle_no_trade() {
    let loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register the existing strategy");

    assert_eq!(node.state(), NodeState::Idle);
    let trader = node.kernel().trader();
    let trader = trader.borrow();
    assert_eq!(trader.strategy_count(), 1);
    assert!(
        trader
            .strategy_ids()
            .contains(&StrategyId::from("ETHCHAINLINKTAKER-V3-001")),
        "expected existing strategy registered in NT trader, got {:?}",
        trader.strategy_ids()
    );
}

#[test]
fn bolt_v3_strategy_registration_wiring_has_no_live_order_or_subscription_calls() {
    let source =
        std::fs::read_to_string(support::repo_path("src/bolt_v3_strategy_registration.rs"))
            .expect("strategy-registration source should be readable");

    for forbidden in [
        "submit_order",
        "submit_order_list",
        "order_factory",
        "subscribe_book",
        "subscribe_quotes",
        "subscribe_trades",
    ] {
        assert!(
            !source.contains(forbidden),
            "bolt-v3 strategy-registration wiring must not call `{forbidden}`"
        );
    }
}
