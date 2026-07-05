use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bolt_v2::{
    bolt_v3_basket_admission::{
        BoltV3BasketAdmissionError, BoltV3BasketAdmissionLimits,
        BoltV3BasketAdmissionReleaseReason, BoltV3BasketAdmissionRequest,
        BoltV3BasketAdmissionState,
    },
    bolt_v3_capital_admission::{
        CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
        ProductAdmissionSnapshot, ProductKind,
    },
    bolt_v3_capital_admission_state::{
        OrderLifecycleCapitalAdmissionSnapshot, PortfolioCapitalAdmissionSnapshot,
        VenueSpendabilitySnapshot,
    },
    bolt_v3_capital_reservation::CapitalPoolSnapshot,
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
        BoltV3BasketAdmissionDecisionEvidence, BoltV3BasketAdmissionOutcome,
        BoltV3CapitalAdmissionRebuildAuditEvidence, BoltV3DecisionEvidenceWriter,
        BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence, BoltV3ExitEvaluationEvidence,
        BoltV3LossGovernorHaltEvidence, BoltV3OrderIntentEvidence, BoltV3OrderRejectEvidence,
        BoltV3RequoteThrottleEvidence, BoltV3SettlementBookingErrorEvidence,
        BoltV3SettlementEvidence, BoltV3StrategyInputEvidenceSnapshot,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
    },
    bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState},
    bolt_v3_outcome_group_scanner::{OutcomeGroupLegScanEvidence, OutcomeGroupScanEvidence},
    bolt_v3_outcome_groups::{
        AttestedLegRef, AttestedPayoutVector, CanonicalField, GroupingProof,
        NormalizedPriceScaleEvidence, OrderConstraintSource, OutcomeGroup, OutcomeGroupSourceKind,
        OutcomeLeg, OutcomeLegOrderConstraints, OutcomeLegRole, PolymarketDiscoveryScopeEvidence,
        PositiveSideBinding, PriceScaleAssertionSource, RoleBindingProof, SettlementRules,
        SettlementSourceKind, TerminalPayoutDerivation, TerminalState, TerminalStateConvention,
        TerminalStateKind, ValidatedOutcomeGroup, build_leg_map, canonical_fingerprint,
        derive_standard_payout_matrix, expected_metadata_fingerprint,
        payout_vector_attestation_sha256, role_binding_attestation_sha256,
    },
    bolt_v3_submit_admission::{
        BoltV3BasketSubmitSlotClaim, BoltV3CompiledOrderAdmissionEvidence, BoltV3CompiledOrderKind,
        BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide, BoltV3CompiledProductKind,
        BoltV3LiveSubmitApprovalLimits, BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionError,
        BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
        BoltV3SubmitCapitalAdmissionConfig, BoltV3SubmitCapitalAdmissionNtComponents,
        BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy, PredictionMarketOutcomeSide,
        live_submit_count_cap_outcome,
    },
};
use nautilus_model::{
    enums::{OrderSide, PositionSide},
    identifiers::{InstrumentId, Venue},
};
use rust_decimal::Decimal;

fn decimal_literal(value: &str) -> Decimal {
    Decimal::from_str_exact(&value.replace('_', "")).expect("decimal literal should parse")
}

macro_rules! dec {
    ($($value:tt)+) => {
        decimal_literal(stringify!($($value)+))
    };
}

#[test]
fn basket_admission_reserves_whole_basket_records_keyed_evidence_and_releases_exposure() {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let basket_state = BoltV3BasketAdmissionState::new(writer.clone(), admission_limits());
    let submit_state = submit_state(writer.clone(), 4, dec!(10));
    let group = fixture_group();
    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1111.111111), 1_000);
    let claims = entry_claims(&group, dec!(0.9));

    let mut permit = basket_state
        .admit(
            &basket_request("basket-1", &group, &scan, claims.clone()),
            &submit_state,
        )
        .expect("first basket should admit");
    permit.commit_submitted();

    assert_eq!(submit_state.admitted_order_count(), 2);
    let decisions = writer.basket_admission_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].strategy_id, "complete-set-arb");
    assert_eq!(decisions[0].basket_id, "basket-1");
    assert_eq!(decisions[0].group_id, group.group_id);
    assert_eq!(
        decisions[0].leg_instrument_ids,
        claims
            .iter()
            .map(|claim| claim.instrument_id.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(decisions[0].outcome, BoltV3BasketAdmissionOutcome::Admitted);

    let capped = basket_state
        .admit(
            &basket_request("basket-2", &group, &scan, claims.clone()),
            &submit_state,
        )
        .expect_err("second open basket should hit max-open cap");
    assert_eq!(capped, BoltV3BasketAdmissionError::MaxOpenBasketCapExceeded);

    basket_state
        .release_basket("basket-1", BoltV3BasketAdmissionReleaseReason::Terminal)
        .expect("terminal release should free basket exposure reservation");
    drop(permit);
    let mut second_permit = basket_state
        .admit(
            &basket_request("basket-2", &group, &scan, claims),
            &submit_state,
        )
        .expect("released exposure should allow the next basket");
    second_permit.commit_submitted();

    assert_eq!(
        submit_state.admitted_order_count(),
        4,
        "submit-order approvals are monotonic and are not released with basket exposure"
    );
}

#[test]
fn dropped_basket_admission_permit_releases_open_reservation() {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let basket_state = BoltV3BasketAdmissionState::new(writer.clone(), admission_limits());
    let submit_state = submit_state(writer, 4, dec!(10));
    let group = fixture_group();
    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1111.111111), 1_000);
    let claims = entry_claims(&group, dec!(0.9));

    let permit = basket_state
        .admit(
            &basket_request("basket-1", &group, &scan, claims.clone()),
            &submit_state,
        )
        .expect("basket should admit");
    assert_eq!(submit_state.admitted_order_count(), 2);
    drop(permit);
    assert_eq!(
        submit_state.admitted_order_count(),
        0,
        "dropping before downstream submit commit must roll back submit slots"
    );

    let mut second_permit = basket_state
        .admit(
            &basket_request("basket-2", &group, &scan, claims),
            &submit_state,
        )
        .expect("dropped permit should release the open basket reservation");
    second_permit.commit_submitted();
    assert_eq!(submit_state.admitted_order_count(), 2);
}

#[test]
fn stuck_reason_cannot_release_basket_exposure_reservation() {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let basket_state = BoltV3BasketAdmissionState::new(writer.clone(), admission_limits());
    let submit_state = submit_state(writer, 4, dec!(10));
    let group = fixture_group();
    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1111.111111), 1_000);
    let claims = entry_claims(&group, dec!(0.9));

    let mut permit = basket_state
        .admit(
            &basket_request("basket-1", &group, &scan, claims.clone()),
            &submit_state,
        )
        .expect("basket should admit");
    permit.commit_submitted();

    assert!(
        basket_state
            .release_basket("basket-1", BoltV3BasketAdmissionReleaseReason::Stuck)
            .is_err(),
        "stuck exposure must stay reserved"
    );
    drop(permit);
    assert_eq!(
        basket_state
            .admit(
                &basket_request("basket-2", &group, &scan, claims),
                &submit_state,
            )
            .expect_err("stuck exposure should still count against open basket cap"),
        BoltV3BasketAdmissionError::MaxOpenBasketCapExceeded
    );
}

#[test]
fn basket_admission_rejects_stale_or_non_admissible_scanner_and_group_evidence() {
    let group = fixture_group();
    let scan = scan_evidence(&group, dec!(11), dec!(1), dec!(1000), 1_000);
    assert_basket_rejects(
        "basket notional cap",
        basket_request(
            "basket-notional",
            &group,
            &scan,
            entry_claims(&group, dec!(5.5)),
        ),
        BoltV3BasketAdmissionError::BasketNotionalCapExceeded,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    assert_basket_rejects(
        "stale scanner evidence",
        basket_request(
            "basket-stale",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        )
        .with_now_unix_ms(3_001),
        BoltV3BasketAdmissionError::StaleScannerEvidence,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 2_001);
    assert_basket_rejects(
        "future scanner evidence",
        basket_request(
            "basket-future-scan",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::StaleScannerEvidence,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    assert_basket_rejects(
        "stale submit recheck",
        basket_request(
            "basket-stale-submit",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        )
        .with_submit_recheck_observed_unix_ms(1_499),
        BoltV3BasketAdmissionError::StaleSubmitRecheck,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    assert_basket_rejects(
        "future submit recheck",
        basket_request(
            "basket-future-submit",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        )
        .with_submit_recheck_observed_unix_ms(2_001),
        BoltV3BasketAdmissionError::StaleSubmitRecheck,
    );

    let scan = scan_evidence(&group, Decimal::ZERO, dec!(1), dec!(1000), 1_000);
    assert_basket_rejects(
        "non-positive candidate cost",
        basket_request(
            "basket-zero-cost",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::NonPositiveCandidateCost,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    let mut missing_claim = entry_claims(&group, dec!(0.9));
    missing_claim.pop();
    assert_basket_rejects(
        "submit claim count mismatch",
        basket_request("basket-claim-count", &group, &scan, missing_claim),
        BoltV3BasketAdmissionError::SubmitClaimsMismatch,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    let mut wrong_instrument_claims = entry_claims(&group, dec!(0.9));
    wrong_instrument_claims[0].instrument_id = "other.POLYMARKET".to_string();
    assert_basket_rejects(
        "submit claim instrument mismatch",
        basket_request(
            "basket-claim-instrument",
            &group,
            &scan,
            wrong_instrument_claims,
        ),
        BoltV3BasketAdmissionError::SubmitClaimsMismatch,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    assert_basket_rejects(
        "submit claim notional exceeds scanned leg",
        basket_request(
            "basket-claim-notional",
            &group,
            &scan,
            entry_claims(&group, dec!(0.91)),
        ),
        BoltV3BasketAdmissionError::SubmitClaimsMismatch,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(-0.1), dec!(1000), 1_000);
    assert_basket_rejects(
        "non-positive edge",
        basket_request(
            "basket-negative-edge",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::NonPositiveEdge,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(99.99), 1_000);
    assert_basket_rejects(
        "edge threshold",
        basket_request(
            "basket-edge-threshold",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::EdgeThreshold,
    );

    let no_grouping = group_without_grouping_proof(&group);
    let scan = scan_evidence_without_grouping_proof(&group);
    assert_basket_rejects(
        "missing grouping proof",
        basket_request(
            "basket-no-grouping-proof",
            &no_grouping,
            &scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::MissingGroupingProof,
    );

    let mut mismatched_group_scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    mismatched_group_scan.group_id = "other-group".to_string();
    assert_basket_rejects(
        "scanner group id must bind to requested group",
        basket_request(
            "basket-mismatched-group-id",
            &group,
            &mismatched_group_scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::GroupingProofMismatch,
    );

    let mut mismatched_proof_scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    mismatched_proof_scan.grouping_proof = Some(GroupingProof::HyperliquidOutcome {
        question: 42,
        outcome_indices: vec![0, 1],
        proof_fingerprint: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
    });
    assert_basket_rejects(
        "scanner grouping proof must bind to requested group",
        basket_request(
            "basket-mismatched-grouping-proof",
            &group,
            &mismatched_proof_scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::GroupingProofMismatch,
    );

    let no_settlement = group_without_settlement_rules(&group);
    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    assert_basket_rejects(
        "missing settlement rules",
        basket_request(
            "basket-no-settlement",
            &no_settlement,
            &scan,
            entry_claims(&group, dec!(0.9)),
        ),
        BoltV3BasketAdmissionError::MissingSettlementRules,
    );

    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    assert_basket_rejects(
        "retry budget",
        basket_request(
            "basket-retry-budget",
            &group,
            &scan,
            entry_claims(&group, dec!(0.9)),
        )
        .with_retry_count(2),
        BoltV3BasketAdmissionError::RetryBudgetExceeded,
    );
}

#[test]
fn basket_submit_slots_share_single_order_gate_and_count_cap_arithmetic() {
    assert_eq!(
        live_submit_count_cap_outcome(u32::MAX, 1, u32::MAX),
        BoltV3AdmissionOutcome::RejectedCountCapExhausted,
        "overflow in current-count plus leg-count must reject"
    );

    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = submit_state(writer.clone(), 2, dec!(10));
    submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &entry_claims(&fixture_group(), dec!(0.9)),
            &basket_slot_evidence("exact-cap", &fixture_group()),
        )
        .expect("two-leg basket should exactly consume a two-order cap")
        .commit_submitted();
    assert_eq!(submit_gate.admitted_order_count(), 2);

    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = submit_state(writer.clone(), 2, dec!(10));
    submit_gate
        .admit(&single_order_request("seed-order", dec!(1)))
        .expect("seed single order should consume one slot")
        .commit_submitted();
    let exhausted = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &entry_claims(&fixture_group(), dec!(0.9)),
            &basket_slot_evidence("count-cap", &fixture_group()),
        )
        .expect_err("current count plus basket leg count should exceed cap");
    assert_eq!(exhausted, BoltV3SubmitAdmissionError::CountCapExhausted);

    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = submit_state(writer, 2, dec!(0.5));
    let notional = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &entry_claims(&fixture_group(), dec!(0.9)),
            &basket_slot_evidence("notional-cap", &fixture_group()),
        )
        .expect_err("per-leg submit notional cap must be reused for basket claims");
    assert_eq!(notional, BoltV3SubmitAdmissionError::NotionalCapExceeded);
}

#[test]
fn basket_submit_slots_carry_capital_admission_evidence_into_shared_gate() {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let basket_state = BoltV3BasketAdmissionState::new(writer.clone(), admission_limits());
    let submit_gate = capital_admission_submit_state(writer.clone());
    let group = fixture_group();
    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    let mut claims = entry_claims(&group, dec!(0.9));
    attach_capital_admission(&mut claims);
    seed_capital_admission_for_claims(&submit_gate, &claims);

    let mut permit = basket_state
        .admit(
            &basket_request("capital-admission-basket", &group, &scan, claims.clone()),
            &submit_gate,
        )
        .expect("capital-admission basket claims should pass through the shared submit gate");
    permit.commit_submitted();

    assert_eq!(submit_gate.admitted_order_count(), 2);
    assert_eq!(
        submit_gate.capital_admission_live_reserved_liability(),
        Some(dec!(2.4)),
        "both committed basket legs must retain their capital-admission reservations"
    );
    for claim in &claims {
        assert!(
            submit_gate.capital_admission_has_live_reservation(&claim.client_order_id),
            "basket leg {} must retain its submit reservation",
            claim.client_order_id
        );
    }
    assert_eq!(
        writer.submit_reservation_metadata().len(),
        2,
        "basket legs must record submit reservation metadata before commit"
    );
}

#[test]
fn basket_admission_rolls_back_submit_slots_when_metadata_evidence_fails() {
    let writer = Arc::new(RecordingBasketDecisionWriter {
        fail_submit_reservation_metadata: AtomicBool::new(true),
        ..Default::default()
    });
    let basket_state = BoltV3BasketAdmissionState::new(writer.clone(), admission_limits());
    let submit_gate = capital_admission_submit_state(writer.clone());
    let group = fixture_group();
    let scan = scan_evidence(&group, dec!(1.8), dec!(0.2), dec!(1000), 1_000);
    let mut claims = entry_claims(&group, dec!(0.9));
    attach_capital_admission(&mut claims);
    seed_capital_admission_for_claims(&submit_gate, &claims);

    let error = basket_state
        .admit(
            &basket_request("metadata-failure-basket", &group, &scan, claims.clone()),
            &submit_gate,
        )
        .expect_err("metadata evidence failure must reject the basket admission");

    assert!(matches!(
        error,
        BoltV3BasketAdmissionError::SubmitAdmissionFailed(
            BoltV3SubmitAdmissionError::EvidenceWriteFailed { .. }
        )
    ));
    assert_eq!(submit_gate.admitted_order_count(), 0);
    assert_eq!(
        submit_gate.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert!(
        writer.basket_admission_decisions().is_empty(),
        "admitted basket evidence must not be recorded before per-leg reservation metadata"
    );
    for claim in &claims {
        assert!(
            !submit_gate.capital_admission_has_live_reservation(&claim.client_order_id),
            "failed metadata write must roll back {}",
            claim.client_order_id
        );
    }

    writer
        .fail_submit_reservation_metadata
        .store(false, Ordering::SeqCst);
    let mut retry_claims = entry_claims(&group, dec!(0.9));
    attach_capital_admission(&mut retry_claims);
    seed_capital_admission_for_claims(&submit_gate, &retry_claims);
    let mut plain_permit = basket_state
        .admit(
            &basket_request("metadata-failure-basket", &group, &scan, retry_claims),
            &submit_gate,
        )
        .expect("failed submit reservation must release the open basket");
    plain_permit.commit_submitted();
}

#[test]
fn basket_submit_slots_reject_capital_admission_that_does_not_match_order_shape() {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = capital_admission_submit_state(writer);
    let group = fixture_group();
    let mut claims = entry_claims(&group, dec!(0.9));
    attach_capital_admission(&mut claims);
    seed_capital_admission_for_claims(&submit_gate, &claims);
    claims[0]
        .admission_evidence
        .as_mut()
        .expect("fixture should carry capital admission")
        .quantity = dec!(0.5);

    let rejected = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &claims,
            &basket_slot_evidence("shape-mismatch", &group),
        )
        .expect_err("capital admission evidence must bind to submitted order shape");

    assert_eq!(
        rejected,
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: bolt_v2::bolt_v3_submit_admission::BoltV3CapitalAdmissionRejectReason::OrderShapeMismatch,
        }
    );
    assert_eq!(submit_gate.admitted_order_count(), 0);
    assert_eq!(
        submit_gate.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    for claim in &claims {
        assert!(
            !submit_gate.capital_admission_has_live_reservation(&claim.client_order_id),
            "rejected basket leg {} must not retain a reservation",
            claim.client_order_id
        );
    }
}

#[test]
fn dropped_capital_admission_basket_submit_permit_rolls_back_all_leg_reservations() {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = capital_admission_submit_state(writer.clone());
    let group = fixture_group();
    let mut claims = entry_claims(&group, dec!(0.9));
    attach_capital_admission(&mut claims);
    seed_capital_admission_for_claims(&submit_gate, &claims);

    let permit = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &claims,
            &basket_slot_evidence("rollback-capital-admission", &group),
        )
        .expect("capital-admission basket should reserve before caller submits");
    assert_eq!(submit_gate.admitted_order_count(), 2);
    assert_eq!(
        submit_gate.capital_admission_live_reserved_liability(),
        Some(dec!(2.4))
    );

    drop(permit);

    assert_eq!(submit_gate.admitted_order_count(), 0);
    assert_eq!(
        submit_gate.capital_admission_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    for claim in &claims {
        assert!(
            !submit_gate.capital_admission_has_live_reservation(&claim.client_order_id),
            "dropping the basket permit must release {}",
            claim.client_order_id
        );
    }
}

#[test]
fn basket_submit_slots_enforce_kill_switch_and_risk_reducing_proof_binding() {
    let group = fixture_group();
    let first_leg = group
        .tradable_legs
        .values()
        .next()
        .expect("fixture should have at least one leg");

    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = submit_state(writer.clone(), 2, dec!(10));
    submit_gate.replace_kill_switch_state(halted_kill_switch_state());
    let latched_entry = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &entry_claims(&group, dec!(0.9)),
            &basket_slot_evidence("latched-entry", &group),
        )
        .expect_err("entry baskets must obey the existing kill-switch latch");
    assert!(matches!(
        latched_entry,
        BoltV3SubmitAdmissionError::KillSwitchLatched { .. }
    ));

    let latched_risk_reducing_claim = risk_reducing_claim(first_leg, valid_exit_proof(first_leg));
    let latched_risk_reducing = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &[latched_risk_reducing_claim],
            &basket_slot_evidence("risk-reducing", &group),
        )
        .expect_err("risk-reducing repair/unwind claims must obey the kill-switch latch");
    assert!(matches!(
        latched_risk_reducing,
        BoltV3SubmitAdmissionError::KillSwitchLatched { .. }
    ));

    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let submit_gate = submit_state(writer, 2, dec!(10));
    let mut mismatched = valid_exit_proof(first_leg);
    mismatched.exit_order_side = OrderSide::Buy;
    let invalid = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &[risk_reducing_claim(first_leg, mismatched)],
            &basket_slot_evidence("invalid-proof", &group),
        )
        .expect_err("risk-reducing proof must bind to instrument, side, and quantity");
    assert_eq!(
        invalid,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    );

    let mut mismatched = valid_exit_proof(first_leg);
    mismatched.instrument_id = "other.POLYMARKET".to_string();
    let invalid = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &[risk_reducing_claim(first_leg, mismatched)],
            &basket_slot_evidence("invalid-instrument-proof", &group),
        )
        .expect_err("risk-reducing proof must bind to the submitted instrument");
    assert_eq!(
        invalid,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    );

    let mut mismatched = valid_exit_proof(first_leg);
    mismatched.exit_quantity = dec!(2);
    let invalid = submit_gate
        .reserve_basket_submit_slots(
            "polymarket_main",
            &[risk_reducing_claim(first_leg, mismatched)],
            &basket_slot_evidence("invalid-quantity-proof", &group),
        )
        .expect_err("risk-reducing proof must bind to the submitted quantity");
    assert_eq!(
        invalid,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    );
}

fn assert_basket_rejects(
    case_name: &str,
    request: BoltV3BasketAdmissionRequest<'_>,
    expected: BoltV3BasketAdmissionError,
) {
    let writer = Arc::new(RecordingBasketDecisionWriter::default());
    let basket_state = BoltV3BasketAdmissionState::new(writer.clone(), admission_limits());
    let submit_state = submit_state(writer, 4, dec!(10));

    let actual = basket_state
        .admit(&request, &submit_state)
        .expect_err(case_name);

    assert_eq!(actual, expected, "{case_name}");
    assert_eq!(
        submit_state.admitted_order_count(),
        0,
        "{case_name}: basket-level rejects must not consume submit slots"
    );
}

#[derive(Debug, Default)]
struct RecordingBasketDecisionWriter {
    admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
    basket_admission_decisions: Mutex<Vec<BoltV3BasketAdmissionDecisionEvidence>>,
    submit_reservation_metadata: Mutex<Vec<BoltV3SubmitReservationMetadataEvidence>>,
    fail_submit_reservation_metadata: AtomicBool,
}

impl RecordingBasketDecisionWriter {
    fn basket_admission_decisions(&self) -> Vec<BoltV3BasketAdmissionDecisionEvidence> {
        self.basket_admission_decisions
            .lock()
            .expect("basket decisions mutex should not be poisoned")
            .clone()
    }

    fn submit_reservation_metadata(&self) -> Vec<BoltV3SubmitReservationMetadataEvidence> {
        self.submit_reservation_metadata
            .lock()
            .expect("submit reservation metadata mutex should not be poisoned")
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for RecordingBasketDecisionWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_admission_decision(
        &self,
        decision: &BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        self.admission_decisions
            .lock()
            .expect("admission decisions mutex should not be poisoned")
            .push(decision.clone());
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        self.basket_admission_decisions
            .lock()
            .expect("basket admission decisions mutex should not be poisoned")
            .push(decision.clone());
        Ok(())
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        if self.fail_submit_reservation_metadata.load(Ordering::SeqCst) {
            anyhow::bail!("submit reservation metadata write failed");
        }
        self.submit_reservation_metadata
            .lock()
            .expect("submit reservation metadata mutex should not be poisoned")
            .push(metadata.clone());
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> anyhow::Result<()> {
        anyhow::bail!("basket admission writer received entry-skip evidence")
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> anyhow::Result<()> {
        anyhow::bail!("basket admission writer received exit-decision evidence")
    }

    fn record_exit_evaluation(
        &self,
        _evidence: &BoltV3ExitEvaluationEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(
        &self,
        _evidence: &BoltV3LossGovernorHaltEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_requote_throttle(
        &self,
        _throttle: &BoltV3RequoteThrottleEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket admission writer received requote-throttle evidence")
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> anyhow::Result<()> {
        anyhow::bail!("basket admission writer received settlement evidence")
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> anyhow::Result<()> {
        anyhow::bail!("basket admission writer received settlement booking-error evidence")
    }
}

fn admission_limits() -> BoltV3BasketAdmissionLimits {
    BoltV3BasketAdmissionLimits {
        max_basket_notional: dec!(10),
        max_open_baskets: 1,
        min_edge_bps: dec!(100),
        max_scanner_evidence_age_ms: 2_000,
        max_submit_recheck_age_ms: 500,
        max_retry_count: 1,
    }
}

fn submit_state(
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
    max_order_count: u32,
    max_order_notional: Decimal,
) -> BoltV3SubmitAdmissionState {
    BoltV3SubmitAdmissionState::new_with_live_submit_limits(
        writer,
        BTreeMap::from([(
            "polymarket_main".to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count,
                max_order_notional,
            },
        )]),
    )
}

fn capital_admission_submit_state(
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> BoltV3SubmitAdmissionState {
    let observed_at_ns = capital_admission_fixture_observed_at_ns();
    BoltV3SubmitAdmissionState::new_with_capital_admission(
        writer,
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USDC".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "basket-admission-test".to_string(),
                observed_at_ns,
                pool_id: "polymarket-pool".to_string(),
                max_pool_liability: dec!(10),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: capital_admission_fixture_max_snapshot_age_ns(),
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
                fee_slippage_policy: Some(FeeSlippagePolicy {
                    max_fee_liability: dec!(0.1),
                    max_slippage_liability: dec!(0.2),
                }),
            },
            dedupe_retention_ns: 1_000,
        },
    )
}

fn seed_capital_admission_for_claims(
    submit_gate: &BoltV3SubmitAdmissionState,
    claims: &[BoltV3BasketSubmitSlotClaim],
) {
    let observed_at_ns = capital_admission_fixture_observed_at_ns();
    submit_gate.update_capital_admission_nt_components(capital_admission_components(
        claims,
        observed_at_ns,
    ));
    let rebuild =
        submit_gate.rebuild_capital_admission_open_order_reservations(Vec::new(), observed_at_ns);
    assert!(
        rebuild.accepted,
        "empty open-order rebuild should reconcile the test capital admission"
    );
    assert_eq!(submit_gate.capital_admission_reconciled(), Some(true));
}

fn capital_admission_fixture_observed_at_ns() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test system time should be after UNIX_EPOCH")
        .as_nanos();
    u64::try_from(nanos).expect("test UNIX timestamp should fit in u64")
}

fn capital_admission_fixture_max_snapshot_age_ns() -> u64 {
    u64::try_from(Duration::from_secs(60).as_nanos())
        .expect("fixture freshness horizon should fit in u64")
}

fn basket_request<'a>(
    basket_id: &'a str,
    group: &'a OutcomeGroup,
    scan_evidence: &'a OutcomeGroupScanEvidence,
    submit_claims: Vec<BoltV3BasketSubmitSlotClaim>,
) -> BoltV3BasketAdmissionRequest<'a> {
    BoltV3BasketAdmissionRequest {
        strategy_id: "complete-set-arb",
        basket_id,
        execution_client_id: "polymarket_main",
        group,
        scanner_evidence: scan_evidence,
        submit_claims,
        now_unix_ms: 2_000,
        submit_recheck_observed_unix_ms: 2_000,
        retry_count: 0,
    }
}

fn basket_slot_evidence(
    basket_id: &str,
    group: &OutcomeGroup,
) -> BoltV3BasketAdmissionDecisionEvidence {
    BoltV3BasketAdmissionDecisionEvidence {
        strategy_id: "complete-set-arb".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        basket_id: basket_id.to_string(),
        group_id: group.group_id.clone(),
        leg_instrument_ids: group
            .tradable_legs
            .values()
            .take(2)
            .map(|leg| leg.instrument_id.to_string())
            .collect(),
        total_notional: dec!(1.8).to_string(),
        leg_order_count: 2,
        outcome: BoltV3BasketAdmissionOutcome::Admitted,
    }
}

fn entry_claims(group: &OutcomeGroup, notional: Decimal) -> Vec<BoltV3BasketSubmitSlotClaim> {
    group
        .tradable_legs
        .values()
        .take(2)
        .map(|leg| BoltV3BasketSubmitSlotClaim {
            client_order_id: format!("{}-entry-order", leg.leg_id),
            instrument_id: leg.instrument_id.to_string(),
            order_side: OrderSide::Buy,
            order_quantity: dec!(1),
            notional,
            intent_kind: BoltV3SubmitIntentKind::Entry,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
            risk_reducing_exit_proof: None,
            admission_evidence: None,
        })
        .collect()
}

fn risk_reducing_claim(
    leg: &OutcomeLeg,
    proof: BoltV3RiskReducingExitProof,
) -> BoltV3BasketSubmitSlotClaim {
    BoltV3BasketSubmitSlotClaim {
        client_order_id: format!("{}-exit-order", leg.leg_id),
        instrument_id: leg.instrument_id.to_string(),
        order_side: OrderSide::Sell,
        order_quantity: dec!(1),
        notional: dec!(0.9),
        intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
        risk_reducing_exit_proof: Some(proof),
        admission_evidence: None,
    }
}

fn attach_capital_admission(claims: &mut [BoltV3BasketSubmitSlotClaim]) {
    for (index, claim) in claims.iter_mut().enumerate() {
        claim.admission_evidence = Some(BoltV3CompiledOrderAdmissionEvidence {
            venue_id: "POLYMARKET".to_string(),
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side: BoltV3CompiledOrderSide::Buy,
            quantity: claim.order_quantity,
            effective_price: claim.notional,
            order_kind: BoltV3CompiledOrderKind::Limit,
            liquidity: BoltV3CompiledOrderLiquidity::Taker,
            quote_set_id: None,
            prediction_market_outcome: Some(if index == 0 {
                PredictionMarketOutcomeSide::Yes
            } else {
                PredictionMarketOutcomeSide::No
            }),
        });
    }
}

fn capital_admission_components(
    claims: &[BoltV3BasketSubmitSlotClaim],
    observed_at_ns: u64,
) -> BoltV3SubmitCapitalAdmissionNtComponents {
    let yes_instrument_id = claims
        .first()
        .expect("capital-admission basket fixture needs a yes claim")
        .instrument_id
        .clone();
    let no_instrument_id = claims
        .get(1)
        .expect("capital-admission basket fixture needs a no claim")
        .instrument_id
        .clone();
    BoltV3SubmitCapitalAdmissionNtComponents {
        source: "basket-admission-test-state".to_string(),
        observed_at_ns,
        portfolio: PortfolioCapitalAdmissionSnapshot {
            source: "basket-admission-test-portfolio".to_string(),
            observed_at_ns,
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "USDC".to_string(),
            free_collateral: dec!(100),
            total_equity: dec!(100),
        },
        venue_spendability: VenueSpendabilitySnapshot {
            source: "basket-admission-test-spendability".to_string(),
            observed_at_ns,
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            collateral_currency: "USDC".to_string(),
            spendable_collateral: dec!(100),
            collateral_allowance: dec!(100),
        },
        order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot {
            source: "basket-admission-test-open-orders".to_string(),
            observed_at_ns,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "basket-admission-test-product".to_string(),
                observed_at_ns,
                yes_instrument_id,
                no_instrument_id,
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: dec!(100),
                conditional_token_allowance: dec!(100),
                collateral_coupled_group_id: "basket-admission-group".to_string(),
            },
        ),
        loss_snapshot: None,
    }
}

fn valid_exit_proof(leg: &OutcomeLeg) -> BoltV3RiskReducingExitProof {
    BoltV3RiskReducingExitProof {
        position_id: "position-1".to_string(),
        instrument_id: leg.instrument_id.to_string(),
        position_side: PositionSide::Long,
        exit_order_side: OrderSide::Sell,
        position_quantity: dec!(5),
        exit_quantity: dec!(1),
    }
}

fn single_order_request(client_order_id: &str, notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "complete-set-arb".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: client_order_id.to_string(),
        instrument_id: "seed.POLYMARKET".to_string(),
        notional,
        order_side: OrderSide::Buy,
        order_quantity: dec!(1),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
        risk_reducing_exit_proof: None,
        kill_switch_forced_reduction: None,
        admission_evidence: None,
    }
}

fn scan_evidence(
    group: &OutcomeGroup,
    total_cost: Decimal,
    absolute_edge: Decimal,
    edge_bps: Decimal,
    observed_unix_ms: u64,
) -> OutcomeGroupScanEvidence {
    let leg_costs = group
        .tradable_legs
        .values()
        .take(2)
        .map(|leg| OutcomeGroupLegScanEvidence {
            leg_id: leg.leg_id.clone(),
            instrument_id: leg.instrument_id,
            order_side: OrderSide::Buy,
            target_notional: dec!(0.9),
            executable_quantity: dec!(1),
            gross_cost: dec!(0.9),
            fee_cost: Decimal::ZERO,
            slippage_buffer: Decimal::ZERO,
            total_adjusted_cost: dec!(0.9),
            vwap_price: dec!(0.9),
            limit_price: dec!(0.9),
            observed_unix_ms,
        })
        .collect();
    OutcomeGroupScanEvidence {
        group_id: group.group_id.clone(),
        grouping_proof: group.grouping_proof.clone(),
        leg_costs,
        state_payouts: BTreeMap::from([("home".to_string(), dec!(2))]),
        guaranteed_payout: total_cost + absolute_edge,
        total_gross_cost: total_cost,
        total_fee_cost: Decimal::ZERO,
        total_slippage_buffer: Decimal::ZERO,
        total_adjusted_cost: total_cost,
        absolute_edge,
        edge_bps,
        min_depth_quantity: dec!(1),
        admissible: true,
        block_reason: None,
    }
}

fn scan_evidence_without_grouping_proof(group: &OutcomeGroup) -> OutcomeGroupScanEvidence {
    OutcomeGroupScanEvidence {
        grouping_proof: None,
        ..scan_evidence(group, dec!(1.8), dec!(0.2), dec!(1000), 1_000)
    }
}

fn group_without_grouping_proof(group: &OutcomeGroup) -> OutcomeGroup {
    OutcomeGroup {
        grouping_proof: None,
        ..group.clone()
    }
}

fn group_without_settlement_rules(group: &OutcomeGroup) -> OutcomeGroup {
    OutcomeGroup {
        settlement_rules: SettlementRules {
            terminal_state_convention: TerminalStateConvention::Unsupported(
                "missing settlement rules".to_string(),
            ),
            ..group.settlement_rules.clone()
        },
        ..group.clone()
    }
}

fn halted_kill_switch_state() -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "halt-1".to_string(),
        trigger: KillSwitchHaltTrigger::loss_governor_breach(
            "loss-governor",
            1_000,
            "daily loss cap breached",
        ),
    }
}

trait BasketRequestTestExt<'a> {
    fn with_now_unix_ms(self, now_unix_ms: u64) -> Self;
    fn with_submit_recheck_observed_unix_ms(self, observed_unix_ms: u64) -> Self;
    fn with_retry_count(self, retry_count: u32) -> Self;
}

impl<'a> BasketRequestTestExt<'a> for BoltV3BasketAdmissionRequest<'a> {
    fn with_now_unix_ms(mut self, now_unix_ms: u64) -> Self {
        self.now_unix_ms = now_unix_ms;
        self
    }

    fn with_submit_recheck_observed_unix_ms(mut self, observed_unix_ms: u64) -> Self {
        self.submit_recheck_observed_unix_ms = observed_unix_ms;
        self
    }

    fn with_retry_count(mut self, retry_count: u32) -> Self {
        self.retry_count = retry_count;
        self
    }
}

fn fixture_group() -> OutcomeGroup {
    let mut terminal_states = BTreeMap::new();
    for state in ["home", "away"] {
        terminal_states.insert(
            state.to_string(),
            TerminalState {
                state_id: state.to_string(),
                label: state.to_string(),
                kind: TerminalStateKind::Standard,
            },
        );
    }
    terminal_states.insert(
        "void_refund".to_string(),
        TerminalState {
            state_id: "void_refund".to_string(),
            label: "void_refund".to_string(),
            kind: TerminalStateKind::Void,
        },
    );

    let legs = build_leg_map(vec![
        leg(
            "home-positive",
            "home",
            "true",
            OutcomeLegRole::PaysOnTerminalState("home".to_string()),
        ),
        leg(
            "away-positive",
            "away",
            "true",
            OutcomeLegRole::PaysOnTerminalState("away".to_string()),
        ),
        leg(
            "home-negative",
            "home",
            "false",
            OutcomeLegRole::PaysUnlessTerminalState("home".to_string()),
        ),
        leg(
            "away-negative",
            "away",
            "false",
            OutcomeLegRole::PaysUnlessTerminalState("away".to_string()),
        ),
    ])
    .expect("fixture leg ids are unique");

    let mut payout_matrix = derive_standard_payout_matrix(
        &terminal_states,
        &legs,
        TerminalStateConvention::ExactlyOneWinner,
    )
    .expect("standard payout matrix should derive");
    let void_cols = payout_matrix
        .cols
        .iter()
        .map(|leg_id| {
            let leg = &legs[leg_id];
            AttestedLegRef::OutcomeAndSide {
                outcome_label: leg.outcome_label.clone(),
                side_label: leg.side_label.clone(),
            }
        })
        .collect::<Vec<_>>();
    let void_payouts = vec![dec!(1); void_cols.len()];
    payout_matrix
        .payout_per_unit_by_state
        .insert("void_refund".to_string(), void_payouts.clone());
    let void_vector = AttestedPayoutVector {
        terminal_state_id: "void_refund".to_string(),
        label: "void_refund".to_string(),
        cols: void_cols.clone(),
        payouts: void_payouts.clone(),
        refund_convention: "operator_attested_static_payout_per_unit".to_string(),
        attestation_sha256: payout_vector_attestation_sha256(
            "void_refund",
            "void_refund",
            &void_cols,
            &void_payouts,
            "operator_attested_static_payout_per_unit",
        ),
    };
    let bindings = vec![
        PositiveSideBinding {
            terminal_state_label: "home".to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId("home-positive".to_string()),
            pays_unless_leg: AttestedLegRef::NativeLegId("home-negative".to_string()),
        },
        PositiveSideBinding {
            terminal_state_label: "away".to_string(),
            pays_on_leg: AttestedLegRef::NativeLegId("away-positive".to_string()),
            pays_unless_leg: AttestedLegRef::NativeLegId("away-negative".to_string()),
        },
    ];
    let mut group = OutcomeGroup {
        group_id: "fixture-neg-risk".to_string(),
        source_client_id: "polymarket_main".into(),
        venue: Venue::from("POLYMARKET"),
        source_kind: OutcomeGroupSourceKind::Polymarket,
        settlement_asset_id: "USDC".to_string(),
        terminal_states,
        tradable_legs: legs,
        payout_matrix,
        grouping_proof: Some(GroupingProof::PolymarketNegRisk {
            neg_risk_market_id: "fixture-neg-risk".to_string(),
            discovery_scope: PolymarketDiscoveryScopeEvidence {
                source_id: "fixture-source".to_string(),
                event_slugs: Vec::new(),
                market_slugs: Vec::new(),
                gamma_query_fingerprint: None,
                cache_key_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                    ["cache_key", "fixture"],
                    "basket-admission",
                )]),
            },
            market_slugs: vec!["home-market".to_string(), "away-market".to_string()],
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["grouping", "neg_risk_market_id"],
                "fixture-neg-risk",
            )]),
        }),
        role_binding_proof: Some(RoleBindingProof::OperatorAttested {
            attestation_id: "fixture-source".to_string(),
            positive_side_bindings: bindings.clone(),
            attestation_sha256: role_binding_attestation_sha256(&bindings),
            proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                ["role_binding", "source_id"],
                "fixture-source",
            )]),
        }),
        settlement_rules: SettlementRules {
            terminal_state_convention: TerminalStateConvention::ExactlyOneWinner,
            settlement_source_kind: SettlementSourceKind::VenueStructuredFields,
            non_standard_terminal_payouts: vec![void_vector],
            terminal_payout_derivation: TerminalPayoutDerivation::StandardRowsPlusAttestedVectors,
        },
        freshness_source_id: "fixture-source".to_string(),
        metadata_fingerprint: String::new(),
    };
    group.metadata_fingerprint = expected_metadata_fingerprint(&group);
    ValidatedOutcomeGroup::validate(&group).expect("fixture group should validate");
    group
}

fn leg(leg_id: &str, outcome_label: &str, side_label: &str, role: OutcomeLegRole) -> OutcomeLeg {
    OutcomeLeg {
        leg_id: leg_id.to_string(),
        instrument_id: instrument_id(leg_id),
        native_leg_id: leg_id.to_string(),
        settlement_asset_id: "USDC".to_string(),
        outcome_label: outcome_label.to_string(),
        side_label: side_label.to_string(),
        leg_role: role,
        price_scale: NormalizedPriceScaleEvidence::BinaryOnePayoutEqualsOneSettlementUnit {
            settlement_asset_id: "USDC".to_string(),
            payout_per_contract: dec!(1),
            price_units_per_payout: dec!(1),
            assertion_source: PriceScaleAssertionSource::VenueStructuredFields {
                proof_fingerprint: canonical_fingerprint(vec![CanonicalField::new(
                    ["price_scale", "native_leg_id"],
                    leg_id,
                )]),
            },
        },
        order_constraints: OutcomeLegOrderConstraints {
            min_quantity: dec!(1),
            min_notional: Some(dec!(0.1)),
            quantity_step: dec!(1),
            constraint_source: OrderConstraintSource::ConfigFloorWithNtPrecision {
                source_id: "fixture-source".to_string(),
            },
        },
    }
}

fn instrument_id(leg_id: &str) -> InstrumentId {
    InstrumentId::from(format!("{leg_id}.POLYMARKET"))
}
