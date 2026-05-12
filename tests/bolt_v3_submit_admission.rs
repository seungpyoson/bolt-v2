use std::path::PathBuf;

use rust_decimal::Decimal;

use bolt_v2::{
    bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport,
    bolt_v3_live_node::build_bolt_v3_live_node_with_summary,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    },
};

mod support;

fn report(
    max_live_order_count: u32,
    max_notional_per_order: Decimal,
) -> BoltV3LiveCanaryGateReport {
    BoltV3LiveCanaryGateReport {
        approval_id: "APPROVAL-001".to_string(),
        no_submit_readiness_report_path: PathBuf::from("reports/no-submit-readiness.json"),
        max_no_submit_readiness_report_bytes: 4096,
        max_live_order_count,
        max_notional_per_order,
        root_max_notional_per_order: Decimal::new(10, 0),
    }
}

fn request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "ETHCHAINLINKTAKER-001".to_string(),
        client_order_id: "O-19700101-000000-001-001-1".to_string(),
        instrument_id: "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
        notional,
    }
}

#[test]
fn submit_admission_rejects_when_gate_report_is_missing() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();

    let error = admission
        .admit(&request(Decimal::new(50, 2)))
        .expect_err("unarmed admission must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::MissingGateReport
    ));
}

#[test]
fn submit_admission_rejects_second_order_after_count_is_exhausted() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("valid report should arm admission");

    admission
        .admit(&request(Decimal::new(50, 2)))
        .expect("first order should consume the one-order canary budget");
    let error = admission
        .admit(&request(Decimal::new(50, 2)))
        .expect_err("second order must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::OrderCountExhausted {
            admitted_order_count: 1,
            max_live_order_count: 1
        }
    ));
}

#[test]
fn submit_admission_rejects_second_gate_report_arm() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("first valid report should arm admission");

    let error = admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect_err("second gate report must not replace admission bounds");

    assert!(matches!(error, BoltV3SubmitAdmissionError::AlreadyArmed));
}

#[test]
fn submit_admission_rejects_notional_above_gate_cap() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("valid report should arm admission");

    let error = admission
        .admit(&request(Decimal::new(101, 2)))
        .expect_err("over-cap notional must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalExceedsCap {
            notional,
            max_notional_per_order
        } if notional == Decimal::new(101, 2)
            && max_notional_per_order == Decimal::new(100, 2)
    ));
}

#[test]
fn submit_admission_rejects_non_positive_notional_without_consuming_budget() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed();
    admission
        .arm(report(1, Decimal::new(100, 2)))
        .expect("valid report should arm admission");

    let error = admission
        .admit(&request(Decimal::ZERO))
        .expect_err("zero notional must reject before NT submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidNotional { notional }
            if notional == Decimal::ZERO
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn bolt_v3_build_returns_unarmed_submit_admission_state() {
    let (_tempdir, loaded) =
        support::load_bolt_v3_config_with_temp_catalog("submit-admission-build");

    let (built, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("bolt-v3 build should succeed");

    let error = built
        .submit_admission()
        .admit(&request(Decimal::new(50, 2)))
        .expect_err("admission must be unarmed before run gate validates report");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::MissingGateReport
    ));
}

#[test]
fn eth_chainlink_taker_records_evidence_then_admits_then_submits_once() {
    let source = std::fs::read_to_string("src/strategies/eth_chainlink_taker.rs")
        .expect("strategy source should be readable");
    let helper_index = source
        .find("fn submit_order_with_decision_evidence")
        .expect("strategy must expose the submit helper");
    let evidence_index = source
        .find("record_order_intent(&intent)")
        .expect("helper must record decision evidence");
    let admission_index = source
        .find("submit_admission().admit(")
        .expect("helper must call submit admission");
    let submit_index = source
        .find("self.submit_order(")
        .expect("helper must contain the only NT submit call");

    assert!(helper_index < evidence_index);
    assert!(evidence_index < admission_index);
    assert!(admission_index < submit_index);
    assert_eq!(source.matches("self.submit_order(").count(), 1);
}
