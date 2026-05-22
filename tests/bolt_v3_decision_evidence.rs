mod support;

use std::sync::Arc;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields, decision_evidence_path,
    },
    strategies::registry::FeeProvider,
    strategies::registry::StrategyBuildContext,
};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::enums::{OrderSide, OrderType, TimeInForce};
use nautilus_model::identifiers::InstrumentId;
use rust_decimal::Decimal;

struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

#[derive(Debug)]
struct NoopDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        Ok(())
    }

    fn record_admission_decision(&self, _decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        Ok(())
    }
}

#[test]
fn decision_evidence_path_stays_under_configured_catalog_directory() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-decision-evidence-path");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let path = decision_evidence_path(&loaded).expect("fixture evidence path should resolve");

    assert!(path.starts_with(temp.path()));
    assert_eq!(
        path.strip_prefix(temp.path()).unwrap(),
        std::path::Path::new("bolt-v3/decision-evidence/order-intents.jsonl")
    );
}

#[test]
fn decision_evidence_path_rejects_absolute_or_parent_traversal() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    for invalid in ["/tmp/order-intents.jsonl", "../order-intents.jsonl"] {
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path = invalid.to_string();
        let error = decision_evidence_path(&loaded)
            .expect_err("invalid decision evidence path should be rejected");
        assert!(
            error
                .to_string()
                .contains("order_intents_relative_path must be non-empty, relative"),
            "unexpected error for {invalid}: {error:#}"
        );
    }
}

#[test]
fn binary_oracle_edge_taker_records_evidence_then_admission_before_only_direct_submit_call() {
    let source = include_str!("../src/strategies/binary_oracle_edge_taker.rs");
    let evidence_index = source
        .find(".record_order_intent(&intent)")
        .expect("strategy must record decision evidence");
    let admission_index = source
        .find(".submit_admission().admit(&request)")
        .expect("strategy wrapper must submit through admission");
    let submit_index = source
        .find(
            "self.submit_order(\n            order,\n            submit_context.position_id,\n            submit_context.client_id,\n            submit_context.params,\n        )",
        )
        .expect("strategy wrapper must thread submit context into the only direct NT submit call");

    assert!(
        evidence_index < admission_index && admission_index < submit_index,
        "decision evidence must be recorded before submit admission before NT submit"
    );
    // This intentionally scans the whole strategy source, including in-file
    // tests, because no code path should bypass the evidence wrapper.
    assert_eq!(
        source.matches("self.submit_order(").count(),
        1,
        "direct NT submit calls must stay inside evidence wrapper only"
    );
}

#[test]
fn binary_oracle_edge_taker_exit_submit_threads_managed_position_id_to_nt() {
    let source = include_str!("../src/strategies/binary_oracle_edge_taker.rs");

    assert!(
        source.contains(
            "SubmitContext::with_client_id_and_position_id(\n                client_id,\n                managed_position.position.position_id,\n            )"
        ),
        "exit submits must pass the managed PositionId into NT submit_order"
    );
}

#[test]
fn strategy_build_context_requires_decision_evidence_value() {
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        Arc::new(NoopDecisionEvidenceWriter),
        Arc::new(
            bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                NoopDecisionEvidenceWriter,
            )),
        ),
    );

    assert!(
        context
            .decision_evidence()
            .record_order_intent(&BoltV3OrderIntentEvidence {
                strategy_id: "strategy-a".to_string(),
                intent_kind: BoltV3OrderIntentKind::Entry,
                instrument_id: "instrument-a".to_string(),
                client_order_id: "order-a".to_string(),
                order_side: OrderSide::Buy.to_string(),
                price: "0.50".to_string(),
                quantity: "1".to_string(),
                order_fields: BoltV3OrderIntentOrderFields {
                    order_type: OrderType::Limit.to_string(),
                    time_in_force: TimeInForce::Gtc.to_string(),
                    price: Some("0.50".to_string()),
                    trigger_price: None,
                    activation_price: None,
                    trigger_type: None,
                    trigger_instrument_id: None,
                    trailing_offset: None,
                    trailing_offset_type: None,
                    expire_time_unix_nanos: None,
                    is_post_only: false,
                    is_reduce_only: false,
                    is_quote_quantity: false,
                },
            })
            .is_ok()
    );
}
