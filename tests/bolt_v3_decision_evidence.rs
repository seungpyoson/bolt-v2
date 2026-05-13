mod support;

use std::sync::Arc;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_decision_evidence::{
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence, decision_evidence_path,
    },
    clients::polymarket::FeeProvider,
    strategies::registry::StrategyBuildContext,
};
use futures_util::future::{BoxFuture, FutureExt};
use rust_decimal::Decimal;

struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<Decimal> {
        None
    }

    fn warm(&self, _token_id: &str) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

#[derive(Debug)]
struct NoopDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
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
fn eth_chainlink_taker_records_evidence_then_admission_before_only_direct_submit_call() {
    let source = include_str!("../src/strategies/eth_chainlink_taker.rs");
    let evidence_index = source
        .find(".record_order_intent(&intent)")
        .expect("strategy must record decision evidence");
    let admission_index = source
        .find(".submit_admission().admit(&request)")
        .expect("strategy wrapper must submit through admission");
    let submit_index = source
        .find("self.submit_order(order, None, Some(client_id))")
        .expect("strategy wrapper must own the only direct NT submit call");

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
fn strategy_build_context_requires_decision_evidence_value() {
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        "platform.reference.test".to_string(),
        Arc::new(NoopDecisionEvidenceWriter),
        Arc::new(bolt_v2::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed()),
    );

    assert_eq!(context.reference_publish_topic(), "platform.reference.test");
}
