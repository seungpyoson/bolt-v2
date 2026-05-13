mod support;

use bolt_v2::bolt_v3_config::load_bolt_v3_config;
use bolt_v2::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport;
use bolt_v2::bolt_v3_live_node::build_bolt_v3_live_node_with;
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
};
use bolt_v2::clients::polymarket::FeeProvider;
use bolt_v2::strategies::registry::StrategyBuildContext;
use futures_util::future::{BoxFuture, FutureExt};
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::Arc;

#[test]
fn unarmed_submit_admission_rejects_before_nt_submit() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    let request = submit_request(Decimal::new(1, 0));

    let result = admission.admit(&request);
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("unarmed admission must reject");

    assert!(matches!(error, BoltV3SubmitAdmissionError::NotArmed));
    assert!(!nt_submit_called, "NT submit must not be reached");
}

#[test]
fn armed_admission_allows_first_submit_and_rejects_second_before_nt_submit() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(gate_report(1, Decimal::new(1, 0)))
        .expect("valid gate report should arm admission");

    let request = submit_request(Decimal::new(1, 0));
    let mut nt_submit_calls = 0;

    admission
        .admit(&request)
        .expect("first within-cap submit should admit");
    nt_submit_calls += 1;

    let second = admission.admit(&request);
    if second.is_ok() {
        nt_submit_calls += 1;
    }
    let error = second.expect_err("second submit must exhaust count cap");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert_eq!(admission.admitted_order_count(), 1);
    assert_eq!(nt_submit_calls, 1, "second NT submit must not be reached");
}

#[test]
fn over_notional_cap_rejects_before_nt_submit_without_consuming_count() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(gate_report(1, Decimal::new(1, 0)))
        .expect("valid gate report should arm admission");

    let result = admission.admit(&submit_request(Decimal::new(2, 0)));
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("over-cap notional must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalCapExceeded
    ));
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(!nt_submit_called, "NT submit must not be reached");
}

#[test]
fn notional_equal_to_cap_is_admitted() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(gate_report(1, Decimal::new(1, 0)))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("notional equal to cap should admit");

    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn non_positive_notional_rejects_before_nt_submit_without_consuming_count() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(gate_report(1, Decimal::new(1, 0)))
        .expect("valid gate report should arm admission");

    let result = admission.admit(&submit_request(Decimal::ZERO));
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("zero notional must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NonPositiveNotional
    ));
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(!nt_submit_called, "NT submit must not be reached");
}

#[test]
fn second_arm_rejects_without_mutating_validated_bounds() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(gate_report(1, Decimal::new(1, 0)))
        .expect("first valid gate report should arm admission");

    let error = admission
        .arm(gate_report(2, Decimal::new(2, 0)))
        .expect_err("second arm must reject");

    assert!(matches!(error, BoltV3SubmitAdmissionError::AlreadyArmed));

    let over_original_cap = admission
        .admit(&submit_request(Decimal::new(2, 0)))
        .expect_err("second arm must not mutate cap");

    assert!(matches!(
        over_original_cap,
        BoltV3SubmitAdmissionError::NotionalCapExceeded
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn fresh_live_node_build_exposes_unarmed_submit_admission_state() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-submit-admission-build");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    let error = runtime
        .submit_admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect_err("fresh build should expose unarmed admission");

    assert!(matches!(error, BoltV3SubmitAdmissionError::NotArmed));
}

#[test]
fn strategy_build_context_carries_shared_submit_admission_handle() {
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed());
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        "reference-topic".to_string(),
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        admission.clone(),
    );

    assert!(Arc::ptr_eq(&admission, &context.submit_admission_arc()));
    let error = context
        .submit_admission()
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect_err("shared context admission should still be unarmed");
    assert!(matches!(error, BoltV3SubmitAdmissionError::NotArmed));
}

#[derive(Debug)]
struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _token_id: &str) -> Option<Decimal> {
        None
    }

    fn warm(&self, _token_id: &str) -> BoxFuture<'_, anyhow::Result<()>> {
        async { Ok(()) }.boxed()
    }
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
    }
}

fn gate_report(
    max_live_order_count: u32,
    max_notional_per_order: Decimal,
) -> BoltV3LiveCanaryGateReport {
    BoltV3LiveCanaryGateReport {
        approval_id: "operator-approved-canary-001".to_string(),
        no_submit_readiness_report_path: PathBuf::from("no-submit-readiness.json"),
        max_no_submit_readiness_report_bytes: 4096,
        max_live_order_count,
        max_notional_per_order,
        root_max_notional_per_order: max_notional_per_order,
    }
}
