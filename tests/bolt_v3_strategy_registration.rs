mod support;

use bolt_v2::{
    bolt_v3_archetypes::binary_oracle_edge_taker,
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node_with_summary, make_bolt_v3_live_node_builder},
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState},
    strategies::{
        binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder,
        registry::{StrategyBuilder, ValidationError},
    },
};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::StrategyId;
use rust_decimal::Decimal;
use std::sync::Arc;

#[test]
fn bolt_v3_registers_configured_strategy_through_runtime_binding_table() {
    fn register_stub(
        node: &mut LiveNode,
        context: bolt_v2::bolt_v3_strategy_registration::StrategyRegistrationContext<'_>,
    ) -> Result<StrategyId, bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError>
    {
        assert_eq!(context.strategy_kind, "stub_runtime_strategy");
        context
            .submit_admission
            .arm(support::validated_bolt_v3_live_canary_gate_report(
                1,
                Decimal::new(1, 0),
            ))
            .map_err(|error| {
                bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError::Binding {
                    strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
                    strategy_archetype: context
                        .strategy
                        .config
                        .strategy_archetype
                        .as_str()
                        .to_string(),
                    message: format!("submit admission arm failed: {error:?}"),
                }
            })?;
        context
            .submit_admission
            .admit(&submit_request(Decimal::new(1, 0)))
            .map_err(|error| {
                bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError::Binding {
                    strategy_instance_id: context.strategy.config.strategy_instance_id.clone(),
                    strategy_archetype: context
                        .strategy
                        .config
                        .strategy_archetype
                        .as_str()
                        .to_string(),
                    message: format!("submit admission admit failed: {error:?}"),
                }
            })?;
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

    fn stub_strategy_kind() -> &'static str {
        "stub_runtime_strategy"
    }

    const TEST_BINDINGS: &[bolt_v2::bolt_v3_strategy_registration::StrategyRuntimeBinding] = &[
        bolt_v2::bolt_v3_strategy_registration::StrategyRuntimeBinding {
            key: "binary_oracle_edge_taker",
            strategy_kind: stub_strategy_kind,
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
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed());
    let mut node = make_bolt_v3_live_node_builder(&empty_loaded)
        .expect("v3 LiveNodeBuilder should construct before strategy registration")
        .build()
        .expect("v3 LiveNode should build before strategy registration");

    let summary =
        bolt_v2::bolt_v3_strategy_registration::register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            &loaded,
            &resolved,
            TEST_BINDINGS,
            admission.clone(),
        )
        .expect("configured strategy should register through matching runtime binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(admission.admitted_order_count(), 1);
    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("BOLT-V3-PHASE3-BINDING")]
    );
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
    }
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

    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy should map into existing taker raw config");

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
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
        table.get("order_id_tag").and_then(|value| value.as_str()),
        Some("001")
    );
    assert_eq!(
        table.get("oms_type").and_then(|value| value.as_str()),
        Some("netting")
    );
    assert_eq!(
        table.get("client_id").and_then(|value| value.as_str()),
        Some("polymarket_main")
    );
    assert_eq!(
        table
            .get("reference_venue")
            .and_then(|value| value.as_str()),
        Some("binance_reference")
    );
    assert_eq!(
        table
            .get("reference_instrument_id")
            .and_then(|value| value.as_str()),
        Some("BTCUSDT.BINANCE")
    );
    assert!(
        !table.contains_key("reference_publish_topic"),
        "reference input must come from configured NT reference_data, not a bolt msgbus topic"
    );
    assert_eq!(
        table
            .get("cadence_seconds")
            .and_then(|value| value.as_integer()),
        Some(300)
    );
    assert_eq!(
        table
            .get("configured_target_id")
            .and_then(|value| value.as_str()),
        Some("btc_updown_5m")
    );
    assert_eq!(
        table.get("target_kind").and_then(|value| value.as_str()),
        Some("rotating_market")
    );
    assert_eq!(
        table
            .get("rotating_market_family")
            .and_then(|value| value.as_str()),
        Some("updown")
    );
    assert_eq!(
        table
            .get("underlying_asset")
            .and_then(|value| value.as_str()),
        Some("BTC")
    );
    assert_eq!(
        table
            .get("cadence_slug_token")
            .and_then(|value| value.as_str()),
        Some("5m")
    );
    assert_eq!(
        table
            .get("market_selection_rule")
            .and_then(|value| value.as_str()),
        Some("active_or_next")
    );
    assert_eq!(
        table
            .get("retry_interval_seconds")
            .and_then(|value| value.as_integer()),
        Some(5)
    );
    assert_eq!(
        table
            .get("blocked_after_seconds")
            .and_then(|value| value.as_integer()),
        Some(60)
    );
    assert_eq!(
        table
            .get("warmup_tick_count")
            .and_then(|value| value.as_integer()),
        Some(20)
    );
    assert_eq!(
        table
            .get("entry_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("order_type"))
            .and_then(|value| value.as_str()),
        Some("limit")
    );
    assert_eq!(
        table
            .get("entry_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("time_in_force"))
            .and_then(|value| value.as_str()),
        Some("fok")
    );
    assert_eq!(
        table
            .get("exit_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("order_type"))
            .and_then(|value| value.as_str()),
        Some("market")
    );
    assert_eq!(
        table
            .get("exit_order")
            .and_then(|value| value.as_table())
            .and_then(|order| order.get("time_in_force"))
            .and_then(|value| value.as_str()),
        Some("ioc")
    );
}

#[test]
fn binary_oracle_runtime_mapping_uses_configured_reference_data_role_key() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "bitcoin_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let reference_data = loaded.strategies[strategy_index]
        .config
        .reference_data
        .remove("spot")
        .expect("fixture should include reference data role");
    loaded.strategies[strategy_index]
        .config
        .reference_data
        .insert("reference".to_string(), reference_data);

    let strategy = &loaded.strategies[strategy_index];
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy should use the configured reference_data role key");
    let table = raw
        .as_table()
        .expect("mapped raw taker config should be a table");

    assert_eq!(
        table
            .get("reference_venue")
            .and_then(|value| value.as_str()),
        Some("binance_reference")
    );
    assert_eq!(
        table
            .get("reference_instrument_id")
            .and_then(|value| value.as_str()),
        Some("BTCUSDT.BINANCE")
    );
}

#[test]
fn binary_oracle_runtime_mapping_uses_market_family_target_projection() {
    let source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");

    assert!(
        !source.contains("updown::deserialize_target_block"),
        "binary_oracle_edge_taker runtime mapping must not deserialize an updown target directly"
    );
    assert!(
        source.contains("target_runtime_fields_from_target"),
        "binary_oracle_edge_taker runtime mapping should consume the market-family target projection"
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
        node.registered_strategy_ids(),
        vec![StrategyId::from("binary_oracle_edge_taker-001")]
    );
}

#[test]
fn binary_oracle_runtime_rejects_strategy_venue_that_cannot_load_target_family() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence-binance-target-family");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "bitcoin_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    strategy.config.venue = "binance_reference".to_string();

    let error =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect_err("Binance venue must not load the configured target family");

    let message = error.to_string();
    assert!(message.contains("binance_reference"), "{message}");
    assert!(
        message.contains("does not support that market family"),
        "{message}"
    );
}
