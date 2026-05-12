use std::path::PathBuf;

use rust_decimal::Decimal;

use bolt_v2::{
    bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport,
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    },
};

fn report(max_live_order_count: u32, max_notional_per_order: Decimal) -> BoltV3LiveCanaryGateReport {
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
