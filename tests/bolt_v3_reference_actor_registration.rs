//! Integration tests for bolt-v3 reference actor registration.
//!
//! This slice proves only `v3 TOML -> selected reference stream -> NT
//! ReferenceActor -> LiveNode Idle`. It must not introduce start/run,
//! market-data subscriptions through user APIs, order construction, submit,
//! lifecycle, or reconciliation claims.

mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config, bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
};
use nautilus_live::node::NodeState;

fn existing_strategy_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

fn existing_strategy_multi_root_fixture() -> std::path::PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml")
}

#[test]
fn bolt_v3_registers_selected_reference_actor_and_remains_idle_no_trade() {
    let mut loaded = load_bolt_v3_config(&existing_strategy_root_fixture())
        .expect("v3 TOML fixture should load");
    let unused_stream = loaded
        .root
        .reference_streams
        .get("eth_usd")
        .expect("fixture should define eth_usd stream")
        .clone();
    loaded
        .root
        .reference_streams
        .insert("unused_eth_usd".to_string(), unused_stream);

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should register selected reference actor");

    assert_eq!(node.state(), NodeState::Idle);
    let trader = node.kernel().trader();
    let trader = trader.borrow();
    assert_eq!(
        trader.actor_count(),
        1,
        "only the strategy-selected reference stream should register an actor; got {:?}",
        trader.actor_ids()
    );
}

#[test]
fn bolt_v3_registers_one_reference_actor_for_shared_stream_across_strategies() {
    let loaded = load_bolt_v3_config(&existing_strategy_multi_root_fixture())
        .expect("v3 multi-strategy TOML fixture should load");

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should register one shared reference actor");

    assert_eq!(node.state(), NodeState::Idle);
    let trader = node.kernel().trader();
    let trader = trader.borrow();
    assert_eq!(trader.strategy_count(), 2);
    assert_eq!(
        trader.actor_count(),
        1,
        "shared reference_stream_id should register one actor; got {:?}",
        trader.actor_ids()
    );
}

#[test]
fn reference_actor_registration_wiring_has_no_connect_run_order_or_user_subscription_calls() {
    let source_path = support::repo_path("src/bolt_v3_reference_actor_registration.rs");
    let source = std::fs::read_to_string(&source_path)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", source_path.display()));

    for forbidden in [
        "connect_bolt_v3_clients",
        ".connect(",
        ".start(",
        ".run(",
        "submit_order",
        "submit_order_list",
        "order_factory",
        "subscribe_book",
        "subscribe_quotes",
        "subscribe_trades",
    ] {
        assert!(
            !source.contains(forbidden),
            "bolt-v3 reference actor registration must not call `{forbidden}`"
        );
    }
}
