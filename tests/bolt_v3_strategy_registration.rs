mod support;

use bolt_v2::{
    bolt_v3_archetypes::binary_oracle_edge_taker,
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    strategies::{eth_chainlink_taker::EthChainlinkTakerBuilder, registry::StrategyBuilder},
    validate::ValidationError,
};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;

#[test]
fn bolt_v3_registers_configured_strategy_through_runtime_binding_table() {
    fn register_stub(
        node: &mut LiveNode,
        context: bolt_v2::bolt_v3_strategy_registration::StrategyRegistrationContext<'_>,
    ) -> Result<StrategyId, bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError>
    {
        let strategy_id = StrategyId::from("BOLT-V3-PHASE3-BINDING");
        node.add_strategy(support::stub_runtime_strategy::StubRuntimeStrategy::new(
            strategy_id.as_str(),
        ))
        .map_err(|source| {
            bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError::Binding {
                strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
                strategy_archetype: context
                    .strategy
                    .config
                    .strategy_archetype
                    .as_str()
                    .to_string(),
                message: source.to_string(),
            }
        })?;
        Ok(strategy_id)
    }

    const TEST_BINDINGS: &[bolt_v2::bolt_v3_strategy_registration::StrategyRuntimeBinding] = &[
        bolt_v2::bolt_v3_strategy_registration::StrategyRuntimeBinding {
            key: "binary_oracle_edge_taker",
            register: register_stub,
        },
    ];

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-binding-decision-evidence");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let mut empty_loaded = loaded.clone();
    empty_loaded.strategies.clear();
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve");
    let (mut node, _summary) = build_bolt_v3_live_node_with_summary(
        &empty_loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("v3 LiveNode should build before strategy registration");

    let summary =
        bolt_v2::bolt_v3_strategy_registration::register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            &loaded,
            &resolved,
            TEST_BINDINGS,
        )
        .expect("configured strategy should register through matching runtime binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
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
}

#[test]
fn bolt_v3_live_node_build_registers_configured_binary_oracle_strategy() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode build should register configured bolt-v3 strategies");

    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("binary_oracle_edge_taker-001")]
    );
}
