use anyhow::Result;
use bolt_v2::bolt_v3_canary_proof_policy::{
    CanaryProofCandidate, CanaryProofInstrumentConstraints, CanaryProofOrderSide,
    CanaryProofPolicyInput, CanaryProofPolicyRejection, CanaryProofSizingMode,
    CanaryProofSourcePacket, build_canary_proof_candidate_source_artifact,
    build_canary_proof_order_intent_artifact, select_canary_proof_candidate,
};
use bolt_v2::strategies::CanaryProofCandidateProvider;
use rust_decimal::Decimal;
use std::str::FromStr;

#[test]
fn proof_policy_selects_highest_scored_source_bound_candidate_without_venue_names() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![
            CanaryProofCandidate {
                strategy_instance_id: "configured_strategy".to_string(),
                execution_client_id: "configured_execution_client".to_string(),
                instrument_id: "instrument-a".to_string(),
                order_side: CanaryProofOrderSide::Buy,
                candidate_score: dec("-12.5"),
                source_refs: vec!["source-hash-a".to_string()],
                sizing_price: dec("0.50"),
                constraints: unconstrained_base_quantity_constraints(),
            },
            CanaryProofCandidate {
                strategy_instance_id: "configured_strategy".to_string(),
                execution_client_id: "configured_execution_client".to_string(),
                instrument_id: "instrument-b".to_string(),
                order_side: CanaryProofOrderSide::Sell,
                candidate_score: dec("-7.5"),
                source_refs: vec!["source-hash-a".to_string()],
                sizing_price: dec("0.25"),
                constraints: unconstrained_base_quantity_constraints(),
            },
        ],
    };

    let selected = select_canary_proof_candidate(&input).expect("candidate selected");

    assert_eq!(selected.instrument_id, "instrument-b");
    assert_eq!(selected.proof_claim, "proof_only");
    assert!(selected.sizing.notional_for_submit_admission() > Decimal::ZERO);
}

#[test]
fn proof_policy_rejects_negative_ev_when_disallowed() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: false,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("-0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: unconstrained_base_quantity_constraints(),
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("negative ev rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::ProofPolicyNegativeEvDisallowed
    );
}

#[test]
fn proof_policy_rejects_candidate_without_current_source_ref_before_ev_filter() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: false,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("-0.01"),
            source_refs: vec!["source-hash-b".to_string()],
            sizing_price: dec("0.50"),
            constraints: unconstrained_base_quantity_constraints(),
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("source mismatch rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::ProofCandidateSourceMismatch
    );
}

#[test]
fn proof_policy_rejects_rounded_quantity_below_adapter_minimum() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("0.01"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: CanaryProofInstrumentConstraints {
                sizing_mode: CanaryProofSizingMode::BaseQuantity,
                quantity_step: dec("0.01"),
                notional_step: None,
                min_quantity: Some(dec("1.00")),
                min_notional: None,
            },
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("below minimum quantity rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::InstrumentConstraintsBelowMinQuantity
    );
}

#[test]
fn proof_policy_rejects_rounded_notional_below_adapter_minimum() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("0.50"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: CanaryProofInstrumentConstraints {
                sizing_mode: CanaryProofSizingMode::BaseQuantity,
                quantity_step: dec("0.01"),
                notional_step: None,
                min_quantity: None,
                min_notional: Some(dec("1.00")),
            },
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("below minimum notional rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::InstrumentConstraintsBelowMinNotional
    );
}

#[test]
fn proof_policy_rounds_quantity_down_to_adapter_notional_step() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("5.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.47"),
            constraints: CanaryProofInstrumentConstraints {
                sizing_mode: CanaryProofSizingMode::BaseQuantity,
                quantity_step: dec("0.01"),
                notional_step: Some(dec("0.01")),
                min_quantity: None,
                min_notional: None,
            },
        }],
    };

    let selected = select_canary_proof_candidate(&input).expect("candidate selected");

    assert_eq!(selected.sizing.quantity_for_submit(), dec("10.00"));
    assert_eq!(
        selected.sizing.notional_for_submit_admission(),
        dec("4.7000")
    );
}

#[test]
fn proof_policy_rejects_non_positive_quantity_step() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: CanaryProofInstrumentConstraints {
                sizing_mode: CanaryProofSizingMode::BaseQuantity,
                quantity_step: Decimal::ZERO,
                notional_step: None,
                min_quantity: None,
                min_notional: None,
            },
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("invalid quantity step rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::InstrumentConstraintsInvalidQuantityStep
    );
}

#[test]
fn proof_policy_rejects_non_positive_notional_step() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: CanaryProofInstrumentConstraints {
                sizing_mode: CanaryProofSizingMode::BaseQuantity,
                quantity_step: dec("0.01"),
                notional_step: Some(Decimal::ZERO),
                min_quantity: None,
                min_notional: None,
            },
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("invalid notional step rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::InstrumentConstraintsInvalidNotionalStep
    );
}

#[test]
fn proof_policy_rejects_non_positive_sizing_price() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: Decimal::ZERO,
            constraints: unconstrained_base_quantity_constraints(),
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("invalid sizing price rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::InstrumentConstraintsInvalidSizingPrice
    );
}

#[test]
fn proof_policy_rejects_non_positive_proof_notional() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: Decimal::ZERO,
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "configured_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: unconstrained_base_quantity_constraints(),
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("non-positive notional rejected");

    assert_eq!(err, CanaryProofPolicyRejection::ProofNotionalNonPositive);
}

#[test]
fn proof_policy_rejects_candidate_for_different_execution_client() {
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: vec![CanaryProofCandidate {
            strategy_instance_id: "configured_strategy".to_string(),
            execution_client_id: "other_execution_client".to_string(),
            instrument_id: "instrument-a".to_string(),
            order_side: CanaryProofOrderSide::Buy,
            candidate_score: dec("0.01"),
            source_refs: vec!["source-hash-a".to_string()],
            sizing_price: dec("0.50"),
            constraints: unconstrained_base_quantity_constraints(),
        }],
    };

    let err = select_canary_proof_candidate(&input).expect_err("execution mismatch rejected");

    assert_eq!(
        err,
        CanaryProofPolicyRejection::ProofPolicyExecutionClientMismatch
    );
}

#[test]
fn strategy_provider_exposes_source_bound_canary_proof_candidates() {
    let provider = TestCanaryProofCandidateProvider {
        candidates: vec![configured_candidate(
            "instrument-a",
            CanaryProofOrderSide::Buy,
            "0.01",
            "source-hash-a",
        )],
    };
    let source_packet = CanaryProofSourcePacket {
        current_source_ref: "source-hash-a".to_string(),
    };

    let candidates = provider
        .canary_proof_candidates(&source_packet)
        .expect("test provider should return candidates");

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].instrument_id, "instrument-a");
    assert_eq!(candidates[0].source_refs, vec!["source-hash-a".to_string()]);
}

#[test]
fn candidate_source_artifact_binds_candidates_to_current_source_ref() {
    let source_packet = CanaryProofSourcePacket {
        current_source_ref: "source-hash-a".to_string(),
    };
    let artifact = build_canary_proof_candidate_source_artifact(
        &source_packet,
        vec![configured_candidate(
            "instrument-a",
            CanaryProofOrderSide::Buy,
            "0.01",
            "source-hash-a",
        )],
    )
    .expect("source-bound candidate artifact should build");

    assert_eq!(
        artifact.record_kind,
        "bolt_v3_canary_proof_candidate_source"
    );
    assert_eq!(artifact.proof_claim, "proof_only");
    assert_eq!(artifact.current_source_ref, "source-hash-a");
    assert_eq!(artifact.candidate_count, 1);
    assert_eq!(artifact.candidates[0].instrument_id, "instrument-a");
}

#[test]
fn proof_order_intent_artifact_uses_selected_source_bound_candidate() {
    let source_packet = CanaryProofSourcePacket {
        current_source_ref: "source-hash-a".to_string(),
    };
    let candidate_source = build_canary_proof_candidate_source_artifact(
        &source_packet,
        vec![
            configured_candidate(
                "instrument-a",
                CanaryProofOrderSide::Buy,
                "-12.5",
                "source-hash-a",
            ),
            configured_candidate(
                "instrument-b",
                CanaryProofOrderSide::Sell,
                "-7.5",
                "source-hash-a",
            ),
        ],
    )
    .expect("candidate source artifact should build");
    let input = CanaryProofPolicyInput {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        proof_claim: "proof_only".to_string(),
        proof_notional: dec("1.00"),
        max_notional_per_order: dec("5.00"),
        allow_negative_expected_ev: true,
        source_ready: true,
        current_source_ref: "source-hash-a".to_string(),
        candidates: Vec::new(),
    };

    let artifact = build_canary_proof_order_intent_artifact(&candidate_source, &input)
        .expect("proof order-intent artifact should build");

    assert_eq!(artifact.record_kind, "bolt_v3_canary_proof_order_intent");
    assert_eq!(artifact.proof_claim, "proof_only");
    assert_eq!(artifact.strategy_instance_id, "configured_strategy");
    assert_eq!(artifact.execution_client_id, "configured_execution_client");
    assert_eq!(artifact.instrument_id, "instrument-b");
    assert_eq!(artifact.order_side, CanaryProofOrderSide::Sell);
    assert_eq!(artifact.notional, dec("1.00"));
    assert_eq!(artifact.quantity, dec("2.00"));
    assert_eq!(artifact.source_refs, vec!["source-hash-a".to_string()]);
}

struct TestCanaryProofCandidateProvider {
    candidates: Vec<CanaryProofCandidate>,
}

impl CanaryProofCandidateProvider for TestCanaryProofCandidateProvider {
    fn canary_proof_candidates(
        &self,
        source_packet: &CanaryProofSourcePacket,
    ) -> Result<Vec<CanaryProofCandidate>> {
        Ok(self
            .candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .source_refs
                    .contains(&source_packet.current_source_ref)
            })
            .cloned()
            .collect())
    }
}

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).expect("test decimal should parse")
}

fn configured_candidate(
    instrument_id: &str,
    order_side: CanaryProofOrderSide,
    candidate_score: &str,
    source_ref: &str,
) -> CanaryProofCandidate {
    CanaryProofCandidate {
        strategy_instance_id: "configured_strategy".to_string(),
        execution_client_id: "configured_execution_client".to_string(),
        instrument_id: instrument_id.to_string(),
        order_side,
        candidate_score: dec(candidate_score),
        source_refs: vec![source_ref.to_string()],
        sizing_price: dec("0.50"),
        constraints: unconstrained_base_quantity_constraints(),
    }
}

fn unconstrained_base_quantity_constraints() -> CanaryProofInstrumentConstraints {
    CanaryProofInstrumentConstraints {
        sizing_mode: CanaryProofSizingMode::BaseQuantity,
        quantity_step: dec("0.01"),
        notional_step: None,
        min_quantity: None,
        min_notional: None,
    }
}
