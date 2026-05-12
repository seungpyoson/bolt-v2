//! Integration tests for the bolt-v3 strategy registration boundary.
//!
//! Phase 3 proves that configured bolt-v3 strategies are registered through a
//! generic boundary before NT's runner starts.

mod support;

use std::sync::{Mutex, MutexGuard};

use bolt_v2::{
    bolt_v3_archetypes::binary_oracle_edge_taker,
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, StrategyRegistrationContext, StrategyRuntimeBinding,
        register_bolt_v3_strategies_on_node_with,
        register_bolt_v3_strategies_on_node_with_bindings, register_bolt_v3_strategies_with,
    },
    strategies::{eth_chainlink_taker::EthChainlinkTakerBuilder, registry::StrategyBuilder},
    validate::ValidationError,
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
    let mut empty_loaded = loaded.clone();
    empty_loaded.strategies.clear();
    let (mut built, _summary) = build_bolt_v3_live_node_with_summary(
        &empty_loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("v3 LiveNode should build before strategy registration");

    let expected_strategy_id = StrategyId::from("BOLT-V3-PHASE3-STUB");
    register_bolt_v3_strategies_on_node_with(built.node_mut(), &loaded, |node, _strategy| {
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

    let strategy_ids = built.node().kernel().trader().borrow().strategy_ids();
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
    let mut empty_loaded = loaded.clone();
    empty_loaded.strategies.clear();
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve");
    let (mut built, _summary) = build_bolt_v3_live_node_with_summary(
        &empty_loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("v3 LiveNode should build before strategy registration");

    let submit_admission = built.submit_admission_arc();
    let summary = register_bolt_v3_strategies_on_node_with_bindings(
        built.node_mut(),
        &loaded,
        &resolved,
        submit_admission,
        TEST_BINDINGS,
    )
    .expect("configured strategy should register through matching runtime binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(
        built.node().kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("BOLT-V3-PHASE3-BINDING")]
    );
}

#[test]
fn binary_oracle_runtime_mapping_produces_existing_taker_raw_config() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "bitcoin_updown_main")
        .expect("fixture should include initial binary oracle strategy");

    let raw = binary_oracle_edge_taker::raw_taker_config(strategy)
        .expect("binary oracle strategy should map into existing taker raw config");

    let mut errors: Vec<ValidationError> = Vec::new();
    EthChainlinkTakerBuilder::validate_config(
        &raw,
        "strategies.bitcoin_updown_main.parameters.runtime",
        &mut errors,
    );

    assert!(
        errors.is_empty(),
        "mapped taker config should validate: {errors:?}"
    );
    let table = raw
        .as_table()
        .expect("mapped raw taker config should be a table");
    assert_eq!(
        table.get("strategy_id").and_then(|value| value.as_str()),
        Some("binary_oracle_edge_taker-001")
    );
    assert_eq!(
        table.get("client_id").and_then(|value| value.as_str()),
        Some("polymarket_main")
    );
    assert_eq!(
        table
            .get("period_duration_secs")
            .and_then(|value| value.as_integer()),
        Some(300)
    );
    assert_eq!(
        table
            .get("warmup_tick_count")
            .and_then(|value| value.as_integer()),
        Some(20)
    );
    assert!(
        table.get("reference_publish_topic").is_none(),
        "reference_publish_topic belongs in StrategyBuildContext, not raw taker config"
    );
}

#[test]
fn bolt_v3_live_node_build_registers_configured_binary_oracle_strategy() {
    let _guard = lock_live_node_test();
    let (_tempdir, loaded) =
        support::load_bolt_v3_config_with_temp_catalog("strategy-registration");

    let (built, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode build should register configured bolt-v3 strategies");

    assert_eq!(
        built.node().kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("binary_oracle_edge_taker-001")]
    );
}

#[test]
fn binary_oracle_runtime_binding_requires_decision_evidence_persistence_config() {
    let (_tempdir, loaded) =
        support::load_bolt_v3_config_with_temp_catalog("decision-evidence-config");

    assert_eq!(
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path,
        "bolt_v3/decision/order_intents.jsonl"
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
    let forbidden = [
        "eth_chainlink_taker",
        "binary_oracle_edge_taker",
        "polymarket_main",
        "chainlink_btcusd",
    ];

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

    let mapping_source = std::fs::read_to_string(support::repo_path(
        "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs",
    ))
    .expect("runtime binding module should be readable");
    assert!(
        mapping_source.contains("binary_oracle_edge_taker"),
        "concrete runtime binding module should own concrete strategy mapping"
    );
}
