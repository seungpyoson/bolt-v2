mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{
        BoltV3LiveNodeError, build_bolt_v3_live_node_with_summary, run_bolt_v3_live_node,
    },
    bolt_v3_start_readiness::BoltV3StartReadinessGateError,
};
use nautilus_live::node::NodeState;
use tempfile::TempDir;

const MARKET_SELECTION_TIMESTAMP_MILLISECONDS: i64 = 601_000;

#[test]
fn production_run_gate_rejects_missing_instruments_before_nt_run() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml");
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&root_path).expect("multi-strategy fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let expected_readiness_fact_count = loaded.strategies.len();
    let (mut node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register configured strategies");

    let error = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_bolt_v3_live_node(
            &mut node,
            &loaded,
            MARKET_SELECTION_TIMESTAMP_MILLISECONDS,
        ))
        .expect_err("missing selected-market instruments must block NT run");

    match error {
        BoltV3LiveNodeError::StartReadiness(BoltV3StartReadinessGateError::Blocked(report)) => {
            assert_eq!(
                report.instrument_readiness.facts.len(),
                expected_readiness_fact_count
            );
            assert_eq!(node.state(), NodeState::Idle);
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn production_run_wrapper_checks_start_readiness_before_nt_run() {
    let source = std::fs::read_to_string(support::repo_path("src/bolt_v3_live_node.rs"))
        .expect("bolt_v3_live_node.rs should read");
    let wrapper = source
        .split("pub async fn run_bolt_v3_live_node")
        .nth(1)
        .expect("production run wrapper should exist")
        .split("#[cfg(test)]")
        .next()
        .expect("production run wrapper body should be before module tests");
    let gate_position = wrapper
        .find("require_bolt_v3_start_readiness_gate")
        .expect("production run wrapper must require start readiness");
    let run_position = wrapper
        .find(".run().await")
        .expect("production run wrapper must be the only v3 NT run boundary");

    assert!(
        gate_position < run_position,
        "production run wrapper must fail closed before NT run"
    );
}
