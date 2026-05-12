use std::sync::Arc;

use anyhow::Result;
use bolt_v2::{clients::polymarket::FeeProvider, strategies::registry::StrategyBuildContext};
use futures_util::future::{BoxFuture, FutureExt};
use rust_decimal::Decimal;

mod support;

#[derive(Debug, Default)]
struct TestFeeProvider;

impl FeeProvider for TestFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<Decimal> {
        Some(Decimal::ZERO)
    }

    fn warm(&self, _token_id: &str) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

#[test]
fn strategy_build_context_rejects_missing_decision_evidence() {
    let result = StrategyBuildContext::try_new(
        Arc::new(TestFeeProvider),
        "platform.reference.test".to_string(),
        None,
    );
    let error = match result {
        Ok(_) => panic!("missing decision evidence must reject context construction"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("decision evidence writer is required"),
        "{error:#}"
    );
}

#[test]
fn decision_evidence_path_joins_configured_relative_path_under_catalog() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded =
        bolt_v2::bolt_v3_config::load_bolt_v3_config(&root_path).expect("fixture should load");

    let path = bolt_v2::bolt_v3_decision_evidence::decision_evidence_path(&loaded)
        .expect("configured relative path should resolve");

    assert_eq!(
        path,
        std::path::Path::new("/var/lib/bolt/catalog")
            .join("bolt_v3")
            .join("decision")
            .join("order_intents.jsonl")
    );
}

#[test]
fn decision_evidence_path_rejects_parent_traversal() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded =
        bolt_v2::bolt_v3_config::load_bolt_v3_config(&root_path).expect("fixture should load");
    loaded
        .root
        .persistence
        .decision_evidence
        .order_intents_relative_path = "../escape.jsonl".to_string();

    let error = bolt_v2::bolt_v3_decision_evidence::decision_evidence_path(&loaded)
        .expect_err("parent traversal must fail closed");

    assert!(
        error.to_string().contains("stay under catalog_directory"),
        "{error:#}"
    );
}

#[test]
fn eth_chainlink_taker_has_no_direct_submit_order_bypass() {
    let source = std::fs::read_to_string("src/strategies/eth_chainlink_taker.rs")
        .expect("strategy source should be readable");
    let direct_submit_count = source.matches("self.submit_order(").count();

    assert_eq!(
        direct_submit_count, 1,
        "only submit_order_with_decision_evidence may call NT submit directly"
    );

    let helper_index = source
        .find("fn submit_order_with_decision_evidence")
        .expect("strategy must expose one submit helper");
    let submit_index = source
        .find("self.submit_order(")
        .expect("helper must contain the only direct NT submit call");

    assert!(
        helper_index < submit_index,
        "the only direct submit call must be inside the evidence helper"
    );
    assert!(
        source[..submit_index].contains("record_order_intent"),
        "decision evidence must be recorded before the only direct NT submit call"
    );
}
