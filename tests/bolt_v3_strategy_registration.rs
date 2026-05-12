//! Integration tests for the bolt-v3 strategy registration boundary.
//!
//! Phase 3 proves that configured bolt-v3 strategies are registered through a
//! generic boundary before NT's runner starts.

mod support;

use std::sync::{Mutex, MutexGuard};

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
        register_bolt_v3_strategies_on_node_with,
        register_bolt_v3_strategies_on_node_with_bindings, register_bolt_v3_strategies_with,
    },
};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;

static LIVE_NODE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_live_node_test() -> MutexGuard<'static, ()> {
    LIVE_NODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn bolt_v3_registers_configured_strategy_through_binding() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let summary = register_bolt_v3_strategies_with(&loaded, |strategy| {
        Ok(format!(
            "{}:{}",
            strategy.config.strategy_archetype.as_str(),
            strategy.config.strategy_instance_id
        ))
    })
    .expect("configured strategies should register through the injected binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(
        summary.registered[0].strategy_instance_id,
        loaded.strategies[0].config.strategy_instance_id
    );
    assert_eq!(
        summary.registered[0].strategy_archetype.as_str(),
        loaded.strategies[0].config.strategy_archetype.as_str()
    );
}

#[test]
fn bolt_v3_registers_configured_strategy_on_nt_trader_through_binding() {
    let _guard = lock_live_node_test();
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let (mut node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build before strategy registration");

    let expected_strategy_id = StrategyId::from("BOLT-V3-PHASE3-STUB");
    register_bolt_v3_strategies_on_node_with(&mut node, &loaded, |node, _strategy| {
        node.add_strategy(support::stub_runtime_strategy::StubRuntimeStrategy::new(
            expected_strategy_id.as_str(),
        ))
        .map_err(|source| BoltV3StrategyRegistrationError::Binding {
            strategy_instance_id: "BOLT-V3-PHASE3-STUB".to_string(),
            strategy_archetype: "test_binding".to_string(),
            message: source.to_string(),
        })?;
        Ok(expected_strategy_id)
    })
    .expect("injected strategy binding should register on NT trader");

    let strategy_ids = node.kernel().trader().borrow().strategy_ids();
    assert_eq!(strategy_ids, vec![expected_strategy_id]);
}

#[test]
fn bolt_v3_registers_configured_strategy_through_runtime_binding_table() {
    let _guard = lock_live_node_test();
    fn register_stub(
        node: &mut LiveNode,
        context: StrategyRegistrationContext<'_>,
    ) -> Result<StrategyId, BoltV3StrategyRegistrationError> {
        let strategy_id = StrategyId::from("BOLT-V3-PHASE3-BINDING");
        node.add_strategy(support::stub_runtime_strategy::StubRuntimeStrategy::new(
            strategy_id.as_str(),
        ))
        .map_err(|source| BoltV3StrategyRegistrationError::Binding {
            strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
            strategy_archetype: context
                .strategy
                .config
                .strategy_archetype
                .as_str()
                .to_string(),
            message: source.to_string(),
        })?;
        Ok(strategy_id)
    }

    const TEST_BINDINGS: &[StrategyRuntimeBinding] = &[StrategyRuntimeBinding {
        key: "binary_oracle_edge_taker",
        register: register_stub,
    }];

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let (mut node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build before strategy registration");

    let summary =
        register_bolt_v3_strategies_on_node_with_bindings(&mut node, &loaded, TEST_BINDINGS)
            .expect("configured strategy should register through matching runtime binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("BOLT-V3-PHASE3-BINDING")]
    );
}

#[test]
fn bolt_v3_rejects_unsupported_strategy_archetype_before_runtime() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    let error = register_bolt_v3_strategies_with(&loaded, |_strategy| {
        Err(BoltV3StrategyRegistrationError::UnsupportedStrategy {
            strategy_archetype: "unsupported_strategy".to_string(),
        })
    })
    .expect_err("unsupported strategy must fail before runtime");

    assert!(
        error
            .to_string()
            .contains("unsupported bolt-v3 strategy archetype"),
        "unexpected error: {error}"
    );

    loaded.strategies.clear();
    let summary = register_bolt_v3_strategies_with(&loaded, |_strategy| {
        unreachable!("empty strategy list must not invoke binding")
    })
    .expect("empty strategy list should be accepted as no registrations");

    assert!(summary.registered.is_empty());
}

#[test]
fn bolt_v3_core_strategy_registration_has_no_concrete_strategy_leakage() {
    let source_paths = [
        "src/main.rs",
        "src/bolt_v3_live_node.rs",
        "src/bolt_v3_strategy_registration.rs",
    ];
    let forbidden = ["eth_chainlink_taker", "binary_oracle_edge_taker"];

    for relative in source_paths {
        let path = support::repo_path(relative);
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        for token in forbidden {
            assert!(
                !source.contains(token),
                "{relative} must not hardcode concrete strategy token `{token}`"
            );
        }
    }
}
