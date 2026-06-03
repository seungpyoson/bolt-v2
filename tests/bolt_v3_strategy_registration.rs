mod support;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_archetypes::binary_oracle_edge_taker,
    bolt_v3_config::{
        DECISION_REFERENCE_GATE_ROLE, DataClientReadinessProbeBookType, LiveCanaryProofPolicyBlock,
        LiveCanaryProofTimeInForce, ReferenceDataBlock, load_bolt_v3_config,
    },
    bolt_v3_live_node::{build_bolt_v3_live_node_with_summary, make_bolt_v3_live_node_builder},
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
        BoltV3SubmitLifecyclePolicy,
    },
    strategies::{
        binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder,
        registry::{FeeProvider, StrategyBuildContext, StrategyBuilder, ValidationError},
    },
};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientId, InstrumentId, StrategyId},
};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

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
    let decision_evidence: Arc<
        dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    > = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(
        decision_evidence.clone(),
    ));
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
            decision_evidence.clone(),
        )
        .expect("configured strategy should register through matching runtime binding");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(admission.admitted_order_count(), 1);
    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("BOLT-V3-PHASE3-BINDING")]
    );
}

#[test]
fn bolt_v3_registration_context_includes_operator_readiness_gate_session() {
    fn register_stub(
        node: &mut LiveNode,
        context: bolt_v2::bolt_v3_strategy_registration::StrategyRegistrationContext<'_>,
    ) -> Result<StrategyId, bolt_v2::bolt_v3_strategy_registration::BoltV3StrategyRegistrationError>
    {
        let readiness = context
            .readiness_evidence
            .as_ref()
            .expect("registration context should include normalized readiness evidence");
        assert_eq!(readiness.gate_session_hash, "a".repeat(64));
        assert_eq!(readiness.selected_market_key, "b".repeat(64));
        let resolution = readiness
            .gate_evidence
            .get("resolution")
            .expect("readiness evidence should include the resolution role");
        assert_eq!(resolution.satisfaction_kind, "no_resolution");
        assert_eq!(
            resolution.resolution_identity.as_deref(),
            Some("configured-reference-price")
        );
        assert!(resolution.provider_kind.is_none());
        let runtime_seed = context
            .runtime_readiness_seed
            .as_ref()
            .expect("source-owned decision_reference should provide a runtime readiness seed");
        assert_eq!(runtime_seed.gate_session_hash, "a".repeat(64));
        assert_eq!(runtime_seed.selected_market_key, "b".repeat(64));
        assert_eq!(runtime_seed.price_to_beat_value, 3_100.0);
        assert_eq!(runtime_seed.reference_price, 3_101.0);
        assert_eq!(
            runtime_seed.reference_quote_ts_event,
            runtime_seed.market_start_timestamp_ms
        );
        assert!(
            runtime_seed.market_end_timestamp_ms > runtime_seed.market_start_timestamp_ms,
            "runtime readiness seed should preserve a forward market window"
        );
        assert_eq!(runtime_seed.realized_volatility, 1.5);
        assert_eq!(runtime_seed.reference_venue, "resolution_oracle_primary");

        let strategy_id = StrategyId::from("BOLT-V3-READINESS-CONTEXT");
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

    let loaded = support::loaded_bolt_v3_live_canary_with_satisfied_report(1, Decimal::new(1, 0));
    let mut empty_loaded = loaded.clone();
    empty_loaded.strategies.clear();
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve");
    let decision_evidence: Arc<
        dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    > = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(
        decision_evidence.clone(),
    ));
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
            admission,
            decision_evidence,
        )
        .expect("configured strategy should receive readiness evidence during registration");

    assert_eq!(summary.registered.len(), loaded.strategies.len());
    assert_eq!(
        node.kernel().trader().borrow().strategy_ids(),
        vec![StrategyId::from("BOLT-V3-READINESS-CONTEXT")]
    );
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(1, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        canary_proof_claim: None,
        risk_reducing_exit_proof: None,
    }
}

#[test]
fn binary_oracle_runtime_mapping_produces_existing_taker_raw_config() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");

    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy should map into existing taker raw config");

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
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
        Some("resolution_oracle_primary")
    );
    assert_eq!(
        table
            .get("reference_instrument_id")
            .and_then(|value| value.as_str()),
        Some("configured-reference-price")
    );
    assert!(
        !table.contains_key("reference_publish_topic"),
        "reference input must come from configured NT reference_data, not a bolt msgbus topic"
    );
    assert_eq!(
        table
            .get("price_to_beat_source")
            .and_then(|value| value.as_str()),
        Some("chainlink_data_streams.configured-reference-price")
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
        Some("configured_updown_target")
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
        Some("CONFIGURED_ASSET")
    );
    assert_eq!(
        table
            .get("cadence_slug_token")
            .and_then(|value| value.as_str()),
        Some("configuredwindow")
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
fn binary_oracle_runtime_mapping_uses_target_resolution_mapping_without_chainlink_special_case() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let provider = loaded
        .root
        .gate_providers
        .as_mut()
        .and_then(|providers| providers.get_mut("resolution_oracle_primary"))
        .expect("fixture should include a resolution provider");
    provider.provider_kind = Some("pyth".to_string());

    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let mapping = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("resolution"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|resolution| resolution.get_mut("market_mappings"))
        .and_then(toml::Value::as_array_mut)
        .and_then(|mappings| mappings.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include a resolution gate mapping");
    mapping.insert(
        "resolution_kind".to_string(),
        toml::Value::String("pyth".to_string()),
    );
    mapping.insert(
        "resolution_identity".to_string(),
        toml::Value::String("configured-pyth-resolution".to_string()),
    );

    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let raw = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded)
        .expect("binary oracle strategy should not require Chainlink in the archetype bridge");

    assert_eq!(
        raw.as_table()
            .and_then(|table| table.get("price_to_beat_source"))
            .and_then(|value| value.as_str()),
        Some("pyth.configured-pyth-resolution")
    );
}

#[test]
fn binary_oracle_runtime_mapping_rejects_decision_reference_resolution_identity_that_parses_as_instrument_id()
 {
    // P7 re-audit (GPT): the source-owned decision_reference path binds
    // reference_instrument_id = decision_reference.resolution_identity, and the
    // strategy accessor parses it with InstrumentId::from_str(..).ok(). The source-
    // owned path relies on resolution_identity NOT being a valid NT instrument id
    // so the accessor returns None and the strategy does not spuriously subscribe
    // to venue quotes (its reference arrives via the readiness seed). Enforce that
    // invariant at the archetype bridge: a decision_reference resolution_identity
    // that parses as an InstrumentId must fail LOUD, not silently enable an NT
    // reference subscription that could ingest the wrong reference data.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let mapping = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("decision_reference"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|decision_reference| decision_reference.get_mut("market_mappings"))
        .and_then(toml::Value::as_array_mut)
        .and_then(|mappings| mappings.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include a decision_reference gate mapping");
    mapping.insert(
        "resolution_identity".to_string(),
        toml::Value::String("REFERENCE.SOURCE".to_string()),
    );

    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let error = binary_oracle_edge_taker::raw_taker_config(strategy, &loaded).expect_err(
        "a decision_reference resolution_identity that parses as an NT InstrumentId must be rejected",
    );
    let rendered = error.to_string();
    assert!(
        rendered.contains("resolution_identity") && rendered.contains("REFERENCE.SOURCE"),
        "rejection should name the offending resolution_identity, got: {rendered}"
    );
}

#[test]
fn binary_oracle_runtime_mapping_preserves_post_only_gtc_entry_order() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("post-only GTC entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("limit")
    );
    assert_eq!(
        entry.get("time_in_force").and_then(toml::Value::as_str),
        Some("gtc")
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        entry.get("is_reduce_only").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        entry
            .get("is_quote_quantity")
            .and_then(toml::Value::as_bool),
        Some(false)
    );
}

#[test]
fn binary_oracle_runtime_mapping_preserves_stop_market_entry_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("stop_market".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("last_price".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("StopMarket entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("stop_market")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );
    assert_eq!(
        entry.get("trigger_type").and_then(toml::Value::as_str),
        Some("last_price")
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "StopMarket runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("StopMarket runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_market_if_touched_entry_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("market_if_touched".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("MarketIfTouched entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("market_if_touched")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "MarketIfTouched runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("MarketIfTouched runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_market_if_touched_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("market_if_touched".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), toml::Value::Float(0.48));
    exit_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("mark_price".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("MarketIfTouched exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("market_if_touched")
    );
    assert_eq!(
        exit.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.48)
    );
    assert_eq!(
        exit.get("trigger_type").and_then(toml::Value::as_str),
        Some("mark_price")
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(false)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "MarketIfTouched exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("MarketIfTouched exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_trailing_stop_market_entry_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("trailing_stop_market".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("last_price".to_string()),
    );
    entry_order.insert("trailing_offset".to_string(), toml::Value::Float(2.5));
    entry_order.insert(
        "trailing_offset_type".to_string(),
        toml::Value::String("basis_points".to_string()),
    );
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("TrailingStopMarket entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("trailing_stop_market")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );
    assert_eq!(
        entry.get("trigger_type").and_then(toml::Value::as_str),
        Some("last_price")
    );
    assert_eq!(
        entry.get("trailing_offset").and_then(toml::Value::as_float),
        Some(2.5)
    );
    assert_eq!(
        entry
            .get("trailing_offset_type")
            .and_then(toml::Value::as_str),
        Some("basis_points")
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "TrailingStopMarket entry runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("TrailingStopMarket entry runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_trailing_stop_market_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("trailing_stop_market".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("activation_price".to_string(), toml::Value::Float(0.48));
    exit_order.insert(
        "trigger_type".to_string(),
        toml::Value::String("mark_price".to_string()),
    );
    exit_order.insert("trailing_offset".to_string(), toml::Value::Float(3.0));
    exit_order.insert(
        "trailing_offset_type".to_string(),
        toml::Value::String("ticks".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(false));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("TrailingStopMarket exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("trailing_stop_market")
    );
    assert_eq!(
        exit.get("activation_price").and_then(toml::Value::as_float),
        Some(0.48)
    );
    assert_eq!(
        exit.get("trigger_type").and_then(toml::Value::as_str),
        Some("mark_price")
    );
    assert_eq!(
        exit.get("trailing_offset").and_then(toml::Value::as_float),
        Some(3.0)
    );
    assert_eq!(
        exit.get("trailing_offset_type")
            .and_then(toml::Value::as_str),
        Some("ticks")
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "TrailingStopMarket exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("TrailingStopMarket exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_stop_limit_entry_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("stop_limit".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.52));
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("StopLimit entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("stop_limit")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.52)
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "StopLimit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("StopLimit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_limit_if_touched_entry_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let entry_order = parameters
        .get_mut("entry_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include entry_order table");
    entry_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit_if_touched".to_string()),
    );
    entry_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    entry_order.insert("trigger_price".to_string(), toml::Value::Float(0.39));
    entry_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("LimitIfTouched entry order should map into runtime config");
    let entry = raw
        .as_table()
        .and_then(|table| table.get("entry_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include entry_order");

    assert_eq!(
        entry.get("order_type").and_then(toml::Value::as_str),
        Some("limit_if_touched")
    );
    assert_eq!(
        entry.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.39)
    );
    assert_eq!(
        entry.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "LimitIfTouched runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("LimitIfTouched runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_post_only_gtc_exit_order() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("post-only GTC exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("limit")
    );
    assert_eq!(
        exit.get("time_in_force").and_then(toml::Value::as_str),
        Some("gtc")
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        exit.get("is_reduce_only").and_then(toml::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        exit.get("is_quote_quantity").and_then(toml::Value::as_bool),
        Some(false)
    );
}

#[test]
fn binary_oracle_runtime_mapping_preserves_stop_limit_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("stop_limit".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), toml::Value::Float(0.48));
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("StopLimit exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("stop_limit")
    );
    assert_eq!(
        exit.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.48)
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "StopLimit exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("StopLimit exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_preserves_limit_if_touched_exit_order_round_trip() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    let parameters = loaded.strategies[strategy_index]
        .config
        .parameters
        .as_table_mut()
        .expect("fixture parameters should be a TOML table");
    let exit_order = parameters
        .get_mut("exit_order")
        .and_then(toml::Value::as_table_mut)
        .expect("fixture parameters should include exit_order table");
    exit_order.insert(
        "order_type".to_string(),
        toml::Value::String("limit_if_touched".to_string()),
    );
    exit_order.insert(
        "time_in_force".to_string(),
        toml::Value::String("gtc".to_string()),
    );
    exit_order.insert("trigger_price".to_string(), toml::Value::Float(0.46));
    exit_order.insert("is_post_only".to_string(), toml::Value::Boolean(true));

    let raw =
        binary_oracle_edge_taker::raw_taker_config(&loaded.strategies[strategy_index], &loaded)
            .expect("LimitIfTouched exit order should map into runtime config");
    let exit = raw
        .as_table()
        .and_then(|table| table.get("exit_order"))
        .and_then(toml::Value::as_table)
        .expect("mapped runtime config should include exit_order");

    assert_eq!(
        exit.get("order_type").and_then(toml::Value::as_str),
        Some("limit_if_touched")
    );
    assert_eq!(
        exit.get("trigger_price").and_then(toml::Value::as_float),
        Some(0.46)
    );
    assert_eq!(
        exit.get("is_post_only").and_then(toml::Value::as_bool),
        Some(true)
    );

    let mut errors: Vec<ValidationError> = Vec::new();
    BinaryOracleEdgeTakerBuilder::validate_config(
        &raw,
        "strategies.configured_updown_main.parameters.runtime",
        &mut errors,
    );
    assert!(
        errors.is_empty(),
        "LimitIfTouched exit runtime table should validate: {errors:?}"
    );
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer.clone(),
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer)),
        support::fixture_execution_venue(),
    );
    BinaryOracleEdgeTakerBuilder::build(&raw, &context)
        .expect("LimitIfTouched exit runtime table should parse into the strategy config");
}

#[test]
fn binary_oracle_runtime_mapping_uses_configured_reference_data_role_key() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let strategy_index = loaded
        .strategies
        .iter()
        .position(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    loaded.strategies[strategy_index]
        .config
        .target
        .as_table_mut()
        .expect("target should be a table")
        .get_mut("gate_subscriptions")
        .expect("gate subscriptions should exist")
        .as_table_mut()
        .expect("gate subscriptions should be a table")
        .remove(DECISION_REFERENCE_GATE_ROLE);
    loaded.strategies[strategy_index]
        .config
        .reference_data
        .insert(
            "reference".to_string(),
            ReferenceDataBlock {
                data_client_id: ClientId::from("polymarket_main"),
                instrument_id: InstrumentId::from("REFERENCE.SOURCE"),
            },
        );

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
        Some("polymarket_main")
    );
    assert_eq!(
        table
            .get("reference_instrument_id")
            .and_then(|value| value.as_str()),
        Some("REFERENCE.SOURCE")
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
fn bolt_v3_live_node_build_registers_only_generic_canary_proof_executor_when_enabled() {
    let mut loaded =
        support::loaded_bolt_v3_live_canary_with_satisfied_report(1, Decimal::new(5, 0));
    configure_canary_proof_policy_and_artifacts(&mut loaded);

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode build should register the proof executor");

    assert_eq!(
        node.registered_strategy_ids(),
        vec![StrategyId::from("canary-proof-executor-proof")]
    );
}

#[test]
fn binary_oracle_strategy_source_does_not_own_canary_proof_claim() {
    let source = include_str!("../src/strategies/binary_oracle_edge_taker.rs");

    assert!(
        !source.contains("CANARY_PROOF_CLAIM"),
        "proof-only live order claim must stay owned by the generic proof executor"
    );
}

#[test]
fn canary_proof_executor_waits_for_submit_time_book_before_submit_attempt() {
    let source = support::repo_text("src/bolt_v3_canary_proof_executor.rs");
    let on_start_body = source
        .split("fn on_start")
        .nth(1)
        .and_then(|tail| tail.split("fn on_stop").next())
        .expect("canary proof executor on_start body should exist");
    assert!(
        on_start_body.contains("self.subscribe_book_deltas"),
        "proof executor must subscribe to immediate book data"
    );
    assert!(
        on_start_body.contains("self.subscribe_book_at_interval"),
        "proof executor must subscribe to TOML-owned book snapshots so quiet books still trigger submit-time checks"
    );
    assert!(
        !on_start_body.contains("try_submit_proof_order"),
        "proof executor must not submit from startup before a submit-time book is observed"
    );

    let deltas_body = source
        .split("fn on_book_deltas")
        .nth(1)
        .and_then(|tail| tail.split("fn on_book").next())
        .expect("canary proof executor on_book_deltas body should exist");
    assert!(
        deltas_body.contains("self.try_submit_proof_order(None)?"),
        "book deltas must be the submit attempt boundary through the NT cache book"
    );

    let book_body = source
        .split("fn on_book(")
        .nth(1)
        .and_then(|tail| tail.split("nautilus_strategy!").next())
        .expect("canary proof executor on_book body should exist");
    assert!(
        book_body.contains("self.try_submit_proof_order(Some(order_book))?"),
        "book snapshots must be a submit attempt boundary using the observed book"
    );
}

#[test]
fn polymarket_source_proof_collectors_use_configured_market_rotation_attempts() {
    let source =
        support::repo_text("src/bolt_v3_providers/polymarket/entry_decision_source_inputs.rs");

    assert!(
        source.contains("entry_decision_source_rotation_max_attempts"),
        "source-proof collectors must derive attempt count from live_canary.proof_policy rotation config"
    );
    assert!(
        source.contains("selected_entry_decision_market_attempts"),
        "source-proof collectors must build configured market attempts instead of selecting one market"
    );
    assert!(
        source.contains("select_entry_decision_market_with_two_sided_books"),
        "source-proof collectors must skip no-book or one-sided markets before writing source artifacts"
    );
    assert!(
        !source.contains("let selected = selected_entry_decision_market("),
        "source-proof collectors must not fail closed on the first selected market before trying configured attempts"
    );
}

#[test]
fn binary_oracle_registration_resolves_fee_provider_through_provider_boundary() {
    let source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");
    assert!(
        source.contains("resolve_fee_provider"),
        "binary_oracle_edge_taker registration should call the generic fee-provider resolver"
    );

    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-fee-provider-boundary");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("configured Polymarket strategy should register through provider boundary");

    assert_eq!(
        node.registered_strategy_ids(),
        vec![StrategyId::from("binary_oracle_edge_taker-001")]
    );
}

fn configure_canary_proof_policy_and_artifacts(
    loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
) {
    let live_canary = loaded
        .root
        .live_canary
        .as_mut()
        .expect("test live canary config should exist");
    live_canary.proof_policy = Some(LiveCanaryProofPolicyBlock {
        enabled: true,
        policy_kind: "least_bad_strategy_candidate".to_string(),
        proof_claim: "proof_only".to_string(),
        executor_strategy_id: "canary-proof-executor-proof".to_string(),
        strategy_instance_id: "configured_updown_main".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        book_type: DataClientReadinessProbeBookType::L2Mbp,
        book_snapshot_interval_millis: 1_000,
        time_in_force: LiveCanaryProofTimeInForce::Fok,
        is_post_only: false,
        is_reduce_only: false,
        is_quote_quantity: false,
        notional_mode: "fixed".to_string(),
        proof_notional: "5.00".to_string(),
        candidate_score_source: "proof_source".to_string(),
        allow_negative_expected_ev: true,
        rotation_observation_enabled: true,
        rotation_min_distinct_markets: 1,
        rotation_max_attempts: 1,
    });
    let operator_evidence = live_canary
        .operator_evidence
        .as_mut()
        .expect("test operator evidence should exist");
    let evidence_dir = PathBuf::from(&operator_evidence.canary_evidence_path)
        .parent()
        .expect("canary evidence path should have a parent")
        .to_path_buf();
    let candidate_source_path = evidence_dir.join("canary-proof-candidate-source.json");
    let order_intent_path = evidence_dir.join("canary-proof-order-intent.json");
    write_json_artifact(
        &candidate_source_path,
        serde_json::json!({
            "record_kind": "bolt_v3_canary_proof_candidate_source",
            "proof_claim": "proof_only",
            "current_source_ref": "a".repeat(64),
            "candidate_count": 1,
            "candidates": [{
                "strategy_instance_id": "configured_updown_main",
                "execution_client_id": "polymarket_main",
                "instrument_id": "configured-condition-UP.POLYMARKET",
                "order_side": "Buy",
                "candidate_score": "-0.01",
                "source_refs": ["a".repeat(64)],
                "sizing_price": "0.50",
                "constraints": {
                    "sizing_mode": "BaseQuantity",
                    "quantity_step": "0.01",
                    "min_quantity": "1.00",
                    "min_notional": "1.00"
                }
            }]
        }),
    );
    write_json_artifact(
        &order_intent_path,
        serde_json::json!({
            "record_kind": "bolt_v3_canary_proof_order_intent",
            "proof_claim": "proof_only",
            "strategy_instance_id": "configured_updown_main",
            "execution_client_id": "polymarket_main",
            "instrument_id": "configured-condition-UP.POLYMARKET",
            "order_side": "Buy",
            "notional": "5.00",
            "quantity": "10.00",
            "source_refs": ["a".repeat(64)]
        }),
    );
    operator_evidence.canary_proof_candidate_source_path =
        Some(candidate_source_path.to_string_lossy().to_string());
    operator_evidence.canary_proof_candidate_source_sha256 =
        Some(sha256_file(&candidate_source_path));
    operator_evidence.canary_proof_order_intent_path =
        Some(order_intent_path.to_string_lossy().to_string());
    operator_evidence.canary_proof_order_intent_sha256 = Some(sha256_file(&order_intent_path));
}

fn write_json_artifact(path: &Path, value: serde_json::Value) {
    fs::write(
        path,
        serde_json::to_vec(&value).expect("test JSON artifact should encode"),
    )
    .expect("test JSON artifact should write");
}

fn sha256_file(path: &Path) -> String {
    hex::encode(Sha256::digest(
        fs::read(path).expect("test artifact should read"),
    ))
}

#[test]
fn fee_provider_resolution_does_not_warm_during_registration() {
    let resolver_source = include_str!("../src/bolt_v3_providers/mod.rs");
    let archetype_source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");

    assert!(
        !resolver_source.contains(".warm("),
        "fee-provider resolver must construct only; fee warm remains in strategy runtime readiness"
    );
    assert!(
        !archetype_source.contains(".warm("),
        "runtime registration must not warm fee providers"
    );
}

#[test]
fn binary_oracle_registration_forwards_readiness_gate_session_to_build_context() {
    let archetype_source = include_str!("../src/bolt_v3_archetypes/binary_oracle_edge_taker.rs");

    assert!(
        archetype_source.contains("context.readiness_evidence.clone()"),
        "binary oracle registration should consume readiness evidence from the generic context"
    );
    assert!(
        archetype_source.contains(".with_readiness_evidence(readiness_evidence)"),
        "binary oracle registration should pass readiness evidence into StrategyBuildContext"
    );
}

#[test]
fn binary_oracle_runtime_rejects_execution_client_id_without_execution_block() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence-data-only-exec-client");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let mut polymarket_data_only = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture should include polymarket_main")
        .clone();
    polymarket_data_only.execution = None;
    polymarket_data_only.secrets = None;
    loaded
        .root
        .clients
        .insert("polymarket_data_only".to_string(), polymarket_data_only);
    let strategy = loaded
        .strategies
        .iter_mut()
        .find(|strategy| strategy.config.strategy_instance_id == "configured_updown_main")
        .expect("fixture should include initial binary oracle strategy");
    strategy.config.execution_client_id = "polymarket_data_only".into();

    let error =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect_err("data-only client must not be used for execution");

    let message = error.to_string();
    assert!(message.contains("polymarket_data_only"), "{message}");
    assert!(
        message.contains("is required by the existing taker fee-provider boundary"),
        "{message}"
    );
}

#[test]
fn fee_provider_source_fence_blocks_concrete_provider_in_shared_layers() {
    const SOURCE_FENCE_MAX_FILE_BYTES: u64 = 1024 * 1024;

    fn forbidden_fee_provider_reference(line: &str) -> bool {
        line.contains("bolt_v3_providers::polymarket")
            || line.contains("polymarket::")
            || line.contains("build_fee_provider")
    }

    fn source_contains_forbidden_fee_provider_reference(source: &str) -> bool {
        source.lines().any(forbidden_fee_provider_reference)
    }

    fn strip_rust_comments(source: &str) -> String {
        enum State {
            Code,
            LineComment,
            BlockComment,
            String { escaped: bool },
            RawString { hashes: usize },
        }

        fn raw_string_hashes_at(chars: &[char], index: usize) -> Option<usize> {
            if chars.get(index) != Some(&'r') {
                return None;
            }
            let mut cursor = index + 1;
            let mut hashes = 0;
            while chars.get(cursor) == Some(&'#') {
                hashes += 1;
                cursor += 1;
            }
            (chars.get(cursor) == Some(&'"')).then_some(hashes)
        }

        let chars = source.chars().collect::<Vec<_>>();
        let mut output = String::with_capacity(source.len());
        let mut state = State::Code;
        let mut index = 0;
        while let Some(&current) = chars.get(index) {
            match state {
                State::Code => {
                    if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'/') {
                        state = State::LineComment;
                        index += 2;
                    } else if chars.get(index) == Some(&'/') && chars.get(index + 1) == Some(&'*') {
                        state = State::BlockComment;
                        index += 2;
                    } else if let Some(hashes) = raw_string_hashes_at(&chars, index) {
                        output.push('r');
                        index += 1;
                        for _ in 0..hashes {
                            output.push('#');
                            index += 1;
                        }
                        output.push('"');
                        index += 1;
                        state = State::RawString { hashes };
                    } else if current == '"' {
                        output.push(current);
                        state = State::String { escaped: false };
                        index += 1;
                    } else {
                        output.push(current);
                        index += 1;
                    }
                }
                State::LineComment => {
                    if current == '\n' {
                        output.push(current);
                        state = State::Code;
                    }
                    index += 1;
                }
                State::BlockComment => {
                    if current == '\n' {
                        output.push(current);
                        index += 1;
                    } else if current == '*' && chars.get(index + 1) == Some(&'/') {
                        state = State::Code;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                State::String { escaped } => {
                    output.push(current);
                    state = if escaped {
                        State::String { escaped: false }
                    } else if current == '\\' {
                        State::String { escaped: true }
                    } else if current == '"' {
                        State::Code
                    } else {
                        State::String { escaped: false }
                    };
                    index += 1;
                }
                State::RawString { hashes } => {
                    output.push(current);
                    if current == '"' {
                        let closes_raw_string =
                            (1..=hashes).all(|offset| chars.get(index + offset) == Some(&'#'));
                        if closes_raw_string {
                            for offset in 1..=hashes {
                                output.push(chars[index + offset]);
                            }
                            index += hashes + 1;
                            state = State::Code;
                        } else {
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
            }
        }
        output
    }

    fn read_source_fence_target(repo_root: &std::path::Path, relative: &str) -> String {
        let path = repo_root.join(relative);
        let metadata = std::fs::metadata(&path).expect("source-fence target metadata should load");
        assert!(
            metadata.is_file(),
            "source-fence target must be a file: {relative}"
        );
        assert!(
            metadata.len() <= SOURCE_FENCE_MAX_FILE_BYTES,
            "source-fence target {relative} is {} bytes; limit is {SOURCE_FENCE_MAX_FILE_BYTES}",
            metadata.len()
        );
        std::fs::read_to_string(path).expect("source-fence target should be readable")
    }

    assert!(
        source_contains_forbidden_fee_provider_reference("let _ = polymarket::build_fee_provider;"),
        "positive control must catch direct concrete provider construction"
    );
    assert!(
        !source_contains_forbidden_fee_provider_reference(&strip_rust_comments(
            "// let _ = polymarket::build_fee_provider;"
        )),
        "negative control must ignore direct construction in line comments"
    );
    assert!(
        !source_contains_forbidden_fee_provider_reference(&strip_rust_comments(
            "/* let _ = polymarket::build_fee_provider; */"
        )),
        "negative control must ignore direct construction in block comments"
    );
    assert_eq!(
        strip_rust_comments("let text = \"// this is string content\";"),
        "let text = \"// this is string content\";",
        "comment stripping must not treat line-comment markers inside strings as comments"
    );

    fn push_rs_files(repo_root: &std::path::Path, directory: &str, files: &mut Vec<String>) {
        fn push_rs_files_from_path(
            repo_root: &std::path::Path,
            path: &std::path::Path,
            files: &mut Vec<String>,
        ) {
            for entry in std::fs::read_dir(path).expect("source-fence directory should be readable")
            {
                let entry = entry.expect("source-fence directory entry should be readable");
                let file_type = entry
                    .file_type()
                    .expect("source-fence directory entry type should be readable");
                let path = entry.path();
                if file_type.is_dir() {
                    push_rs_files_from_path(repo_root, &path, files);
                } else if file_type.is_file()
                    && path.extension().is_some_and(|extension| extension == "rs")
                {
                    files.push(
                        path.strip_prefix(repo_root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }

        push_rs_files_from_path(repo_root, &repo_root.join(directory), files);
    }

    let recursive_temp = support::TempCaseDir::new("fee-provider-source-fence-recursive");
    let nested_strategy_dir = recursive_temp.path().join("src/strategies/nested");
    std::fs::create_dir_all(&nested_strategy_dir)
        .expect("recursive source-fence control directory should be created");
    std::fs::write(nested_strategy_dir.join("mod.rs"), "")
        .expect("recursive source-fence control Rust file should be created");
    std::fs::write(nested_strategy_dir.join("notes.txt"), "")
        .expect("recursive source-fence control non-Rust file should be created");
    let mut recursive_control_files = Vec::new();
    push_rs_files(
        recursive_temp.path(),
        "src/strategies",
        &mut recursive_control_files,
    );
    recursive_control_files.sort();
    assert_eq!(
        recursive_control_files,
        vec!["src/strategies/nested/mod.rs".to_string()],
        "source-fence collection must recurse into nested strategy modules and ignore non-Rust files"
    );

    let repo_root = support::repo_path("");
    let mut files = Vec::new();
    push_rs_files(&repo_root, "src/bolt_v3_archetypes", &mut files);
    push_rs_files(&repo_root, "src/strategies", &mut files);
    files.extend([
        "src/bolt_v3_strategy_registration.rs".to_string(),
        "src/bolt_v3_submit_admission.rs".to_string(),
        "src/bolt_v3_order_intent.rs".to_string(),
    ]);

    let mut violations = Vec::new();
    files.sort();
    files.dedup();
    for relative in files {
        let source = strip_rust_comments(&read_source_fence_target(&repo_root, &relative));
        for (line_index, line) in source.lines().enumerate() {
            if forbidden_fee_provider_reference(line) {
                violations.push(format!("{}:{}", relative, line_index + 1));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "concrete provider construction leaked into shared registration layers: {violations:?}"
    );
}
