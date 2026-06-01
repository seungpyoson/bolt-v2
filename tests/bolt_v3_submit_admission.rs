mod support;

use bolt_v2::bolt_v3_capital_reservation::{CapitalPoolSnapshot, ReservationRejectionReason};
use bolt_v2::bolt_v3_config::load_bolt_v3_config;
use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
    BoltV3OrderIntentEvidence, BoltV3PositionSizerRebuildAuditEvidence,
    BoltV3RecoveredSubmitReservationEvidence, BoltV3StrategyInputEvidenceSnapshot,
    BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
    decision_evidence_path,
};
use bolt_v2::bolt_v3_live_node::build_bolt_v3_live_node_with;
use bolt_v2::bolt_v3_loss_governor::{LossGovernorPolicy, LossHaltReason, LossSnapshot};
use bolt_v2::bolt_v3_position_sizer::{
    FeeSlippagePolicy, PredictionMarketSizingSnapshot, ProductKind, ProductSizingSnapshot,
    SizingMode, SizingPolicy,
};
use bolt_v2::bolt_v3_position_sizer::{PositionSizingLifecycleAction, PositionSizingLifecycleKind};
use bolt_v2::bolt_v3_sizing_state::{
    NtDerivedSizingState, OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot,
    ReservationLedgerSnapshot,
};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3CompiledOrderKind, BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide,
    BoltV3CompiledOrderSizingEvidence, BoltV3CompiledProductKind, BoltV3OrderLifecycleIntent,
    BoltV3PositionSizerRejectReason, BoltV3QuoteQuantityAdmissionInput,
    BoltV3QuoteQuantityOrderSide, BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionError,
    BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind,
    BoltV3SubmitLifecyclePolicy, BoltV3SubmitPositionSizerConfig,
    BoltV3SubmitPositionSizingFillUpdate, BoltV3SubmitPositionSizingLifecycleUpdate,
    BoltV3SubmitPositionSizingNtComponents, BoltV3SubmitPositionSizingOpenOrderEvidence,
    BoltV3SubmitPositionSizingOpenOrderReservation, BoltV3SubmitPositionSizingOpenOrderSnapshot,
    BoltV3SubmitPositionSizingRebuildDecision, PredictionMarketOutcomeSide,
    conservative_quote_quantity_admission_notional, fee_inclusive_admission_notional,
    market_style_admission_ceiling_notional, rounded_order_admission_notional,
};
use bolt_v2::strategies::registry::FeeProvider;
use bolt_v2::strategies::registry::StrategyBuildContext;
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::enums::{OrderSide, PositionSide};
use nautilus_model::identifiers::InstrumentId;
use rust_decimal::Decimal;
use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
fn market_style_admission_ceiling_notional_values_at_instrument_price_ceiling() {
    // A market-style order (no firm limit price) can fill anywhere up to the
    // instrument's structural price ceiling, so its admission notional must be
    // valued at qty * ceiling — the hard bound the venue cannot exceed — never
    // at a reference-price estimate or a configured slippage budget.
    let ceiling = Decimal::from_str_exact("0.999").expect("ceiling should parse");
    let quantity = Decimal::from(100u32);

    let notional = market_style_admission_ceiling_notional(Some(ceiling), quantity)
        .expect("a declared ceiling should value the order");

    assert_eq!(
        notional,
        Decimal::from_str_exact("99.9").expect("expected notional should parse"),
        "market-style notional must be qty * instrument price ceiling"
    );
}

#[test]
fn market_style_admission_ceiling_notional_fails_closed_without_a_ceiling() {
    // With no declared ceiling there is no price the venue cannot exceed, so the
    // order's worst-case cash cost is unbounded and admission must be refused.
    let result = market_style_admission_ceiling_notional(None, Decimal::from(100u32));

    assert_eq!(
        result,
        Err(BoltV3SubmitAdmissionError::MissingPriceCeiling),
        "an unbounded market-style order with no declared ceiling must fail closed"
    );
}

#[test]
fn live_node_runtime_does_not_expose_manual_admission_or_raw_run_bypass() {
    let source = support::repo_text("src/bolt_v3_live_node.rs");

    assert!(
        !source.contains("pub submit_admission:"),
        "runtime must not expose submit admission for manual pre-arm"
    );
    assert!(
        !source.contains("impl Deref for BoltV3LiveNodeRuntime"),
        "runtime must not deref into raw LiveNode"
    );
    assert!(
        !source.contains("impl DerefMut for BoltV3LiveNodeRuntime"),
        "runtime must not deref mutably into raw LiveNode"
    );
}

#[test]
fn live_node_runner_arms_submit_admission_from_config_before_nt_run() {
    let source = support::repo_text("src/bolt_v3_live_node.rs");
    let start = source
        .find("pub async fn run_bolt_v3_live_node")
        .expect("live runner entrypoint should exist");
    let end = source[start..]
        .find("fn run_blocked_before_submit")
        .map(|offset| start + offset)
        .expect("next helper should bound live runner source");
    let runner = &source[start..end];

    let report_index = runner
        .find("build_bolt_v3_live_submit_admission_report_from_config")
        .expect("live runner must derive submit-admission bounds from config before arming");
    let arm_index = runner
        .find(".arm(")
        .expect("live runner should arm submit admission");
    let rebuild_index = runner
        .find("rebuild_position_sizer_from_nt_cache")
        .expect("live runner must reconcile NT cache state before arming submit admission");
    let rebuild_reject_index = runner
        .find("BoltV3LiveNodeError::StartupPositionSizerRebuild")
        .expect("live runner must reject unreconciled cache open orders before arming");
    let run_index = runner
        .find("let run_future = node.run();")
        .expect("live runner should enter NT run after submit admission is armed");

    assert!(
        report_index < rebuild_index
            && rebuild_index < rebuild_reject_index
            && rebuild_reject_index < arm_index
            && arm_index < run_index,
        "live runner must derive admission bounds, reconcile NT cache, reject unreconciled cache open orders, arm submit admission, then enter NT run"
    );
    assert!(
        !runner.contains("let _startup_rebuild"),
        "live runner must not discard the startup rebuild outcome"
    );
    assert!(
        !runner.contains("consume_bolt_v3_live_runner_approval"),
        "live runner must not block startup on operator approval consumption"
    );
}

#[test]
fn live_node_position_sizer_startup_rebuild_uses_only_known_reservation_metadata() {
    let source = support::repo_text("src/bolt_v3_live_node.rs");
    let start = source
        .find("pub fn rebuild_position_sizer_from_nt_cache")
        .expect("live runtime should expose startup position-sizer rebuild");
    let end = source[start..]
        .find("fn nt_open_order_evidence_from_order")
        .map(|offset| start + offset)
        .expect("open-order evidence helper should bound startup rebuild source");
    let rebuild = &source[start..end];

    assert!(
        rebuild.contains("read_submit_reservation_recovery_evidence"),
        "startup rebuild must read Bolt-owned reservation metadata before attributing live NT open orders"
    );
    assert!(
        rebuild.contains("position_sizing_open_order_reservation_from_known_metadata"),
        "startup rebuild must recover open-order reservations from Bolt-owned metadata"
    );
    assert!(
        !rebuild.contains("position_sizing_open_order_reservation_from_evidence("),
        "production startup rebuild must not use configured-contract fallback attribution for live NT open orders"
    );
}

#[test]
fn unarmed_submit_admission_rejects_before_nt_submit() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    let request = submit_request(Decimal::new(1, 0));

    let result = admission.admit(&request);
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("unarmed admission must reject");

    assert!(matches!(error, BoltV3SubmitAdmissionError::NotArmed));
    assert!(!nt_submit_called, "NT submit must not be reached");
}

#[test]
fn armed_admission_allows_first_submit_and_rejects_second_before_nt_submit() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
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
fn submit_admission_rejects_non_proof_only_canary_proof_claim() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(5, 0),
        ))
        .expect("valid gate report should arm admission");
    let mut request = submit_request(Decimal::new(1, 0));
    request.canary_proof_claim = Some("alpha_ready".to_string());

    let error = admission
        .admit(&request)
        .expect_err("non-proof-only claim must fail before submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidCanaryProofClaim
    ));
}

#[test]
fn over_notional_cap_rejects_before_nt_submit_without_consuming_count() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
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
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("notional equal to cap should admit");

    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn fee_inclusive_notional_rejects_when_fee_pushes_cash_debit_over_cap() {
    // Drive through the SAME production helper the canary proof executor calls
    // to turn a rounded order into its admission notional. The raw base notional
    // (4.98) is within the 5.0 cap, but a positive max entry fee (700 bps)
    // scales the admission notional above the cap. If the fee wrapper were
    // deleted from `rounded_order_admission_notional`, this would no longer
    // exceed the cap and the test would fail — it is not tautological.
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(5, 0),
        ))
        .expect("valid gate report should arm admission");
    let raw_base_notional = Decimal::new(498, 2);
    let intended_notional = raw_base_notional;
    let max_entry_fee_bps = Decimal::new(700, 0);
    let admission_notional =
        rounded_order_admission_notional(raw_base_notional, intended_notional, max_entry_fee_bps)
            .expect("within-intent base notional must not trip the rounding-growth guard");

    let error = admission
        .admit(&submit_request(admission_notional))
        .expect_err("fee-inclusive cash debit above cap must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalCapExceeded
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn fee_inclusive_notional_admits_same_base_when_fee_is_zero() {
    // Control arm for the fee boundary above: the IDENTICAL within-cap raw base
    // notional (4.98 < cap 5.0) with ZERO fee must be ADMITTED. This proves the
    // rejection above is produced by the fee path, not by the base notional —
    // remove the fee scaling and the over-cap test would collapse into this one.
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(5, 0),
        ))
        .expect("valid gate report should arm admission");
    let raw_base_notional = Decimal::new(498, 2);
    let intended_notional = raw_base_notional;
    let admission_notional =
        rounded_order_admission_notional(raw_base_notional, intended_notional, Decimal::ZERO)
            .expect("zero-fee within-intent base notional must not trip any guard");

    assert_eq!(
        admission_notional, raw_base_notional,
        "zero fee must leave the rounded base notional unscaled"
    );
    admission
        .admit(&submit_request(admission_notional))
        .expect("within-cap zero-fee admission notional must be admitted");
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn fee_inclusive_notional_cannot_exceed_operator_cap() {
    // F1 invariant: the fee-inclusive admission notional — the cash debit the
    // venue actually incurs — is hard-bounded by the operator-approved per-order
    // cap. Arm the gate with a report whose `max_notional_per_order()` IS the
    // cap, then build an admission request whose notional is exactly the
    // fee-inclusive notional of an order priced AT the cap with a positive fee.
    // Because any positive fee scales the notional strictly above the cap, the
    // strict-`>` cap check in `evaluate`/`admit` must reject it; admission can
    // never let a fee push the cash debit past the operator cap.
    let cap = Decimal::new(5, 0);
    let positive_fee_bps = Decimal::new(700, 0);
    let fee_inclusive_notional = fee_inclusive_admission_notional(cap, positive_fee_bps);
    assert!(
        fee_inclusive_notional > cap,
        "a positive fee must push the fee-inclusive notional strictly above the cap"
    );

    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(1, cap))
        .expect("valid gate report should arm admission");

    let result = admission.admit(&submit_request(fee_inclusive_notional));
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("fee-inclusive notional above the operator cap must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalCapExceeded
    ));
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(!nt_submit_called, "NT submit must not be reached");
}

#[test]
fn rounded_order_admission_notional_fails_closed_when_rounding_grows_past_intent() {
    // FIX #1 regression: banker's rounding to venue precision can round a
    // quantity (or price) UP, so the submitted order's base notional can exceed
    // the operator-approved intended notional. A canary proof intent of 5.30 USD
    // (qty 10.6 @ 0.50, cap 5.3053) rounds to qty 11 @ 0.50 = 5.50 base — 3.7%
    // over intent. The shared admission helper must refuse it before any cap or
    // fee scaling so a rounded order can never debit more than approved.
    let intended_notional = Decimal::new(530, 2);
    let rounded_base_notional = Decimal::new(550, 2);
    let max_entry_fee_bps = Decimal::ZERO;

    let error = rounded_order_admission_notional(
        rounded_base_notional,
        intended_notional,
        max_entry_fee_bps,
    )
    .expect_err("rounding-induced notional growth past operator intent must fail closed");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::RoundedNotionalExceedsIntent {
            rounded_base_notional: r,
            intended_notional: i,
        } if r == rounded_base_notional && i == intended_notional
    ));
}

#[test]
fn rounded_order_admission_notional_admits_when_rounded_base_equals_intent() {
    // Boundary control for the fail-closed guard above: when rounding does NOT
    // grow the order (rounded base == intended notional), admission proceeds and
    // the helper returns the fee-inclusive notional. This proves the guard
    // rejects only genuine rounding-induced growth, not every rounded order.
    let intended_notional = Decimal::new(530, 2);
    let rounded_base_notional = intended_notional;
    let max_entry_fee_bps = Decimal::ZERO;

    let admission_notional = rounded_order_admission_notional(
        rounded_base_notional,
        intended_notional,
        max_entry_fee_bps,
    )
    .expect("rounded base equal to intent must admit");

    assert_eq!(admission_notional, intended_notional);
}

#[test]
fn non_positive_notional_rejects_before_nt_submit_without_consuming_count() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
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
fn configured_loss_governor_rejects_entry_without_fresh_snapshot_before_nt_submit() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission =
        BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(writer.clone(), loss_policy());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let result = admission.admit_at(&submit_request(Decimal::new(1, 1)), 10_100);
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("missing loss snapshot must reject entry submits");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { ref reasons }
            if reasons == &[LossHaltReason::StaleLossSnapshot]
    ));
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(!nt_submit_called, "NT submit must not be reached");
    let decisions = writer.admission_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].outcome,
        BoltV3AdmissionOutcome::RejectedLossGovernorHalted
    );
}

#[test]
fn configured_loss_governor_admits_entry_after_fresh_below_limit_snapshot() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission =
        BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(writer.clone(), loss_policy());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");
    admission.update_loss_snapshot(fresh_below_limit_loss_snapshot(10_000));

    admission
        .admit_at(&submit_request(Decimal::new(1, 1)), 10_100)
        .expect("fresh below-limit loss snapshot should admit otherwise-valid entry");

    assert_eq!(admission.admitted_order_count(), 1);
    let decisions = writer.admission_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].outcome, BoltV3AdmissionOutcome::Admitted);
}

#[test]
fn configured_loss_governor_admit_uses_runtime_clock_after_fresh_snapshot_update() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(
        writer.clone(),
        broad_freshness_loss_policy(),
    );
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");
    admission.update_loss_snapshot(fresh_below_limit_loss_snapshot(current_test_unix_ns()));

    admission
        .admit(&submit_request(Decimal::new(1, 1)))
        .expect("live-facing admit should evaluate the updated snapshot against runtime time");

    assert_eq!(admission.admitted_order_count(), 1);
    let decisions = writer.admission_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].outcome, BoltV3AdmissionOutcome::Admitted);
}

#[test]
fn breached_loss_governor_halts_entries_but_allows_risk_reducing_exit_within_count_cap() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission =
        BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(writer.clone(), loss_policy());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");
    admission.update_loss_snapshot(breached_loss_snapshot(10_000));

    let entry = admission
        .admit_at(&submit_request(Decimal::new(1, 1)), 10_100)
        .expect_err("breached loss snapshot must reject entry risk");
    assert!(matches!(
        entry,
        BoltV3SubmitAdmissionError::LossGovernorHalted { ref reasons }
            if reasons == &[
                LossHaltReason::PerTradeLossLimit,
                LossHaltReason::DailyLossLimit,
                LossHaltReason::RollingLossLimit,
                LossHaltReason::MaxDrawdownLimit,
            ]
    ));

    admission
        .admit_at(
            &submit_request_with_kind(Decimal::new(1, 1), BoltV3SubmitIntentKind::RiskReducingExit),
            10_100,
        )
        .expect("loss halt must not block risk-reducing exit inside count cap");

    assert_eq!(admission.admitted_order_count(), 1);
    let outcomes: Vec<_> = writer
        .admission_decisions()
        .into_iter()
        .map(|decision| decision.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted,
            BoltV3AdmissionOutcome::Admitted,
        ]
    );
}

#[test]
fn quote_quantity_sell_limit_helper_floors_to_submitted_quote_quantity() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Sell,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(25019, 3),
            calculated_notional: Decimal::new(16679333, 6),
        });

    assert_eq!(
        notional,
        Decimal::new(25019, 3),
        "fractional SELL Limit fixture must floor with Decimal::max, not f64 or string comparison"
    );
}

#[test]
fn quote_quantity_sell_stop_limit_helper_floors_to_submitted_quote_quantity() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Sell,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(25019, 3),
            calculated_notional: Decimal::new(16679333, 6),
        });

    assert_eq!(
        notional,
        Decimal::new(25019, 3),
        "fractional SELL StopLimit fixture must floor with Decimal::max, not f64 or string comparison"
    );
}

#[test]
fn quote_quantity_sell_limit_helper_missing_quote_uses_submitted_quote_quantity() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Sell,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(2500, 2),
        });

    assert_eq!(notional, Decimal::new(2500, 2));
}

#[test]
fn quote_quantity_sell_stop_limit_helper_missing_quote_uses_submitted_quote_quantity() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Sell,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(2500, 2),
        });

    assert_eq!(notional, Decimal::new(2500, 2));
}

#[test]
fn quote_quantity_inverse_sell_limit_preserves_nt_notional() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Sell,
            is_quote_quantity: true,
            is_inverse: true,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(1665, 2),
        });

    assert_eq!(notional, Decimal::new(1665, 2));
}

#[test]
fn quote_quantity_inverse_sell_stop_limit_preserves_nt_notional() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Sell,
            is_quote_quantity: true,
            is_inverse: true,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(1665, 2),
        });

    assert_eq!(notional, Decimal::new(1665, 2));
}

#[test]
fn quote_quantity_buy_limit_helper_floors_to_submitted_quote_quantity() {
    // A non-inverse quote-quantity BUY commits exactly the submitted quote
    // quantity in settlement currency. The conservative effective-price pull
    // overstates in the typical case, but when the venue rounds the derived base
    // quantity DOWN (size precision), NT's effective notional can land a sub-tick
    // below the committed quote quantity. The floor must apply to BUY exactly as
    // it does to SELL, otherwise the per-order cap is checked against an
    // understated notional.
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Buy,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(249995, 4),
        });

    assert_eq!(
        notional,
        Decimal::new(2500, 2),
        "BUY Limit admission must not understate the committed quote quantity when base rounding leaves NT notional below it"
    );
}

#[test]
fn quote_quantity_buy_stop_limit_helper_floors_to_submitted_quote_quantity() {
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Buy,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(249995, 4),
        });

    assert_eq!(
        notional,
        Decimal::new(2500, 2),
        "BUY StopLimit admission must floor to the committed quote quantity"
    );
}

#[test]
fn quote_quantity_inverse_buy_limit_preserves_nt_notional() {
    // Inverse instruments do not denominate the quote quantity in settlement
    // currency, so the floor must stay skipped for an inverse BUY just as it is
    // for an inverse SELL.
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Buy,
            is_quote_quantity: true,
            is_inverse: true,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(1665, 2),
        });

    assert_eq!(notional, Decimal::new(1665, 2));
}

#[test]
fn quote_quantity_buy_market_helper_floors_to_submitted_quote_quantity() {
    // A non-inverse quote-quantity Market order commits the submitted quote
    // quantity in settlement currency just like a Limit order. `entry_order` can
    // be configured `is_quote_quantity = true` with `order_type = Market` (a
    // buildable production shape, no config block), so the floor must NOT be
    // restricted to Limit/StopLimit — otherwise a Market entry understates the
    // cap by the same base-rounding sub-tick the SELL/BUY Limit cases did.
    let notional =
        conservative_quote_quantity_admission_notional(BoltV3QuoteQuantityAdmissionInput {
            order_side: BoltV3QuoteQuantityOrderSide::Buy,
            is_quote_quantity: true,
            is_inverse: false,
            submitted_quote_quantity: Decimal::new(2500, 2),
            calculated_notional: Decimal::new(249995, 4),
        });

    assert_eq!(
        notional,
        Decimal::new(2500, 2),
        "quote-quantity Market admission must floor to the committed quote quantity, not just Limit/StopLimit"
    );
}

#[test]
fn quote_quantity_admission_helper_source_fence_blocks_market_tokens() {
    fn contains_forbidden_market_token(source: &str) -> bool {
        source
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//") && !trimmed.starts_with('*')
            })
            .any(|line| {
                line.contains("POLYMARKET")
                    || line.contains("binary_oracle")
                    || line.contains("updown")
            })
    }

    assert!(
        contains_forbidden_market_token("let venue = \"POLYMARKET\";"),
        "positive control must catch forbidden venue token"
    );
    assert!(
        contains_forbidden_market_token("fn binary_oracle_policy() {}"),
        "positive control must catch forbidden strategy token"
    );
    assert!(
        !contains_forbidden_market_token("// POLYMARKET appears only in a comment"),
        "comment text must not trip source fence"
    );

    let source = std::fs::read_to_string("src/bolt_v3_submit_admission.rs")
        .expect("submit-admission source should be readable");
    assert!(
        !contains_forbidden_market_token(&source),
        "shared submit-admission helper must remain venue, market, and strategy agnostic"
    );
}

#[test]
fn second_arm_rejects_without_mutating_validated_bounds() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("first valid gate report should arm admission");

    let error = admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            2,
            Decimal::new(2, 0),
        ))
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
fn fresh_live_node_build_keeps_submit_admission_internal() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-submit-admission-build");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let _runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");
}

#[test]
fn live_node_build_carries_configured_loss_governor_into_submit_admission() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-submit-admission-loss-governor-build");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    assert!(
        runtime.loss_governor_configured(),
        "live build must carry enabled [risk.loss_governor] into shared submit admission"
    );
}

#[test]
fn live_node_build_carries_configured_loss_governor_runtime_feed_subscription() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-submit-admission-loss-feed-build");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    assert!(
        runtime.loss_governor_runtime_feed_configured(),
        "live build must subscribe the configured loss governor to NT portfolio and position events"
    );
}

#[test]
fn live_node_build_carries_single_enabled_position_sizer_into_submit_admission() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded
        .root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should configure capital pools")[0]
        .enforce_submit_admission = true;
    let temp = support::TempCaseDir::new("bolt-v3-submit-admission-position-sizer-build");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    assert!(
        runtime.position_sizer_configured(),
        "live build must carry enabled capital-pool submit enforcement into shared submit admission"
    );
    assert!(
        runtime.position_sizer_runtime_feed_configured(),
        "live build must subscribe enabled submit enforcement to NT order lifecycle events"
    );
}

#[test]
fn live_node_position_sizer_cache_rebuild_stays_closed_without_components() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded
        .root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should configure capital pools")[0]
        .enforce_submit_admission = true;
    let temp = support::TempCaseDir::new("bolt-v3-position-sizer-cache-rebuild-no-components");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("fixture v3 LiveNode should build");

    let rebuild = runtime.rebuild_position_sizer_from_nt_cache(1_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 0,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(runtime.position_sizer_reconciled(), Some(false));
}

#[test]
fn live_node_position_sizer_without_strategies_records_rebuild_audit_evidence() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.strategies.clear();
    loaded
        .root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should configure capital pools")[0]
        .enforce_submit_admission = true;
    let temp = support::TempCaseDir::new("bolt-v3-position-sizer-no-strategy-rebuild-evidence");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();

    let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("position-sizer LiveNode without strategies should build");

    let rebuild = runtime.rebuild_position_sizer_from_nt_cache(1_000);

    assert!(!rebuild.accepted);
    let evidence_path = decision_evidence_path(&loaded).expect("evidence path should resolve");
    let evidence = std::fs::read_to_string(evidence_path)
        .expect("position-sizer rebuild audit evidence should be durable");
    assert!(
        evidence.contains("\"kind\":\"position_sizer_rebuild\""),
        "rebuild audit evidence must be written through the configured JSONL writer"
    );
}

#[test]
fn strategy_build_context_carries_shared_submit_admission_handle() {
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    )));
    let context = StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        admission.clone(),
        support::fixture_execution_venue(),
    );

    assert!(Arc::ptr_eq(&admission, &context.submit_admission_arc()));
    let error = context
        .submit_admission()
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect_err("shared context admission should still be unarmed");
    assert!(matches!(error, BoltV3SubmitAdmissionError::NotArmed));
}

#[test]
fn configured_submit_sizer_rejects_entry_without_nt_state_before_nt_submit() {
    let admission = position_sized_admission();
    arm_default(&admission);

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect_err("missing NT sizing state must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::MissingNtState
        }
    ));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_rejects_before_startup_rebuild_even_with_fresh_nt_state() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect_err("fresh NT sizing state alone must not open submit admission");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::ReconciliationRequired
        }
    ));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn direct_state_update_discards_hostile_reservation_evidence() {
    let admission = position_sized_admission();
    let mut hostile_state = fresh_sizing_state(1_000);
    hostile_state.reservation_snapshot.source = "hostile_feed".to_string();
    hostile_state.reservation_snapshot.observed_at_ns = 9_999;
    hostile_state
        .reservation_snapshot
        .all_live_reservations_attributed = true;

    admission.update_position_sizing_state(hostile_state);

    let state = admission
        .position_sizer_state_snapshot()
        .expect("component state should be retained");
    assert_eq!(state.reservation_snapshot.source, "bolt_reservation_ledger");
    assert_eq!(state.reservation_snapshot.observed_at_ns, 1_000);
    assert!(!state.reservation_snapshot.all_live_reservations_attributed);
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
}

#[test]
fn nt_component_update_preserves_submit_owned_reservation_snapshot() {
    let admission = position_sized_admission();

    admission.update_position_sizing_nt_components(fresh_components(1_000));

    let state = admission
        .position_sizer_state_snapshot()
        .expect("component state should be retained");
    assert_eq!(state.reservation_snapshot.source, "bolt_reservation_ledger");
    assert_eq!(state.reservation_snapshot.observed_at_ns, 1_000);
    assert!(!state.reservation_snapshot.all_live_reservations_attributed);
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
}

#[test]
fn position_sizer_direct_rebuild_keeps_gate_closed_without_components() {
    let admission = position_sized_admission();

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(Vec::new(), 1_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 0,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
}

#[test]
fn configured_submit_sizer_empty_startup_rebuild_opens_admission() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(Vec::new(), 1_000);

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: true,
            reason: None,
            attempted_reservation_count: 0,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(true));

    let permit = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_001)
        .expect("explicit startup rebuild should open submit admission");

    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    drop(permit);
}

#[test]
fn rebuild_refreshes_submit_owned_reservation_snapshot() {
    let admission = position_sized_admission();
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(Vec::new(), 1_000);

    assert!(rebuild.accepted);
    let state = admission
        .position_sizer_state_snapshot()
        .expect("component state should still be available");
    assert_eq!(state.reservation_snapshot.source, "bolt_reservation_ledger");
    assert_eq!(state.reservation_snapshot.observed_at_ns, 1_000);
    assert!(state.reservation_snapshot.all_live_reservations_attributed);
}

#[test]
fn unattributed_cache_open_order_keeps_position_sizer_unreconciled() {
    let admission = position_sized_admission();
    let mut components = fresh_components(900);
    components.order_lifecycle.open_order_count = 1;
    components.order_lifecycle.all_open_orders_attributed = false;
    admission.update_position_sizing_nt_components(components);

    let rebuild = admission.rebuild_position_sizing_open_order_snapshot(
        BoltV3SubmitPositionSizingOpenOrderSnapshot {
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_cache".to_string(),
            observed_open_order_count: 1,
            all_open_orders_attributed: false,
            reservations: Vec::new(),
        },
        1_000,
    );

    assert_eq!(
        rebuild.reason,
        Some(ReservationRejectionReason::MissingEvidence)
    );
    assert!(!rebuild.accepted);
    assert_eq!(rebuild.attempted_reservation_count, 1);
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
    let state = admission
        .position_sizer_state_snapshot()
        .expect("component state should remain available for diagnostics");
    assert_eq!(state.reservation_snapshot.source, "nt_open_order_cache");
    assert_eq!(state.reservation_snapshot.observed_at_ns, 1_000);
    assert!(!state.reservation_snapshot.all_live_reservations_attributed);
}

#[test]
fn rebuild_snapshot_preserves_client_order_id_for_terminal_release() {
    let admission = position_sized_admission();
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_snapshot(
        BoltV3SubmitPositionSizingOpenOrderSnapshot {
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_cache".to_string(),
            observed_open_order_count: 1,
            all_open_orders_attributed: true,
            reservations: vec![open_order_reservation(
                "client-1",
                "reservation-1",
                Decimal::new(43, 1),
            )],
        },
        1_000,
    );

    assert!(rebuild.accepted);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let release = admission.apply_position_sizing_terminal_order_event(
        "client-1".to_string(),
        1_100,
        "nt_order_terminal".to_string(),
    );

    assert!(release.accepted);
    assert!(!release.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn rebuild_snapshot_records_durable_position_sizer_audit_evidence() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = position_sized_admission_with_writer(writer.clone());
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_snapshot(
        BoltV3SubmitPositionSizingOpenOrderSnapshot {
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_cache".to_string(),
            observed_open_order_count: 1,
            all_open_orders_attributed: true,
            reservations: vec![open_order_reservation(
                "client-1",
                "reservation-1",
                Decimal::new(43, 1),
            )],
        },
        1_000,
    );

    assert!(rebuild.accepted);
    assert_eq!(
        writer.position_sizer_rebuild_audits(),
        vec![BoltV3PositionSizerRebuildAuditEvidence {
            observed_at_ns: 1_000,
            source: "nt_open_order_cache".to_string(),
            observed_open_order_count: 1,
            all_open_orders_attributed: true,
            accepted: true,
            reason: None,
            attempted_reservation_count: 1,
            recovered_reservation_count: 1,
            live_reserved_liability: Decimal::new(43, 1).to_string(),
        }]
    );
}

#[test]
fn rebuild_snapshot_fails_closed_when_position_sizer_audit_write_fails() {
    let admission =
        position_sized_admission_with_writer(Arc::new(FailingPositionSizerRebuildAuditWriter));
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_snapshot(
        BoltV3SubmitPositionSizingOpenOrderSnapshot {
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_cache".to_string(),
            observed_open_order_count: 1,
            all_open_orders_attributed: true,
            reservations: vec![open_order_reservation(
                "client-1",
                "reservation-1",
                Decimal::new(43, 1),
            )],
        },
        1_000,
    );

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_rebuilds_open_order_reservation_for_terminal_release() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(43, 1),
        )],
        1_000,
    );

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: true,
            reason: None,
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 1,
            live_reserved_liability: Decimal::new(43, 1),
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(true));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let release = admission.apply_position_sizing_terminal_order_event(
        "client-order-1".to_string(),
        1_100,
        "nt_order_terminal".to_string(),
    );

    assert!(release.accepted);
    assert!(!release.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_rebuilt_reservation_partial_fill_revalues_residual() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(43, 1),
        )],
        1_000,
    );

    assert!(rebuild.accepted);

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn configured_submit_sizer_open_order_evidence_builds_rebuild_reservation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let reservation = admission
        .position_sizing_open_order_reservation_from_evidence(
            BoltV3SubmitPositionSizingOpenOrderEvidence {
                client_order_id: "client-order-1".to_string(),
                instrument_id: "instrument-yes.VENUE-A".to_string(),
                side: BoltV3CompiledOrderSide::Buy,
                open_quantity: Decimal::new(10, 0),
                limit_price: Decimal::new(4, 1),
                observed_at_ns: 1_000,
                evidence_label: "nt_open_order_cache".to_string(),
            },
        )
        .expect("configured binary open order should build a rebuild reservation");

    assert_eq!(reservation.client_order_id, "client-order-1");
    assert_eq!(reservation.submit_reservation_id, "client-order-1#1000");
    assert_eq!(reservation.collateral_group_id, "group-1");
    assert_eq!(reservation.liability, Decimal::new(43, 1));
    assert_eq!(reservation.instrument_id, "instrument-yes.VENUE-A");
    assert_eq!(reservation.side, BoltV3CompiledOrderSide::Buy);
    assert_eq!(reservation.open_quantity, Decimal::new(10, 0));
    assert_eq!(reservation.liability_factor, Decimal::new(4, 1));
    assert_eq!(reservation.additive_liability, Decimal::new(3, 1));
    assert_eq!(reservation.observed_at_ns, 1_000);
    assert_eq!(reservation.evidence_label, "nt_open_order_cache");
}

#[test]
fn configured_submit_sizer_known_metadata_builds_rebuild_reservation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    let recovered = BoltV3RecoveredSubmitReservationEvidence {
        metadata: BoltV3SubmitReservationMetadataEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "client-order-1#1".to_string(),
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "USD".to_string(),
            capital_pool_id: "pool-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: "Buy".to_string(),
            submitted_quantity: "10".to_string(),
            liability_factor: "0.4".to_string(),
            additive_liability: "0.3".to_string(),
            reserved_liability: "4.3".to_string(),
            observed_at_ns: 950,
            source: "submit_admission".to_string(),
        },
        fill_trade_ids: ["trade-before-restart".to_string()].into(),
    };

    let reservation = admission
        .position_sizing_open_order_reservation_from_known_metadata(
            BoltV3SubmitPositionSizingOpenOrderEvidence {
                client_order_id: "client-order-1".to_string(),
                instrument_id: "instrument-yes.VENUE-A".to_string(),
                side: BoltV3CompiledOrderSide::Buy,
                open_quantity: Decimal::new(6, 0),
                limit_price: Decimal::new(4, 1),
                observed_at_ns: 1_000,
                evidence_label: "nt_open_order_cache".to_string(),
            },
            &recovered,
        )
        .expect("known Bolt metadata should rebuild residual open order");

    assert_eq!(reservation.submit_reservation_id, "client-order-1#1");
    assert_eq!(reservation.liability, Decimal::new(27, 1));
    assert_eq!(reservation.open_quantity, Decimal::new(6, 0));
    assert_eq!(reservation.original_quantity, Decimal::new(10, 0));
    assert_eq!(reservation.filled_quantity, Decimal::new(4, 0));
    assert!(reservation.seen_trade_ids.contains("trade-before-restart"));
    assert_eq!(
        reservation.evidence_label,
        "bolt_known_reservation_metadata"
    );
}

#[test]
fn configured_submit_sizer_known_metadata_rejects_residual_over_submitted_quantity() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    let recovered = BoltV3RecoveredSubmitReservationEvidence {
        metadata: BoltV3SubmitReservationMetadataEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "client-order-1#1".to_string(),
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "USD".to_string(),
            capital_pool_id: "pool-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: "Buy".to_string(),
            submitted_quantity: "10".to_string(),
            liability_factor: "0.4".to_string(),
            additive_liability: "0.3".to_string(),
            reserved_liability: "4.3".to_string(),
            observed_at_ns: 950,
            source: "submit_admission".to_string(),
        },
        fill_trade_ids: Default::default(),
    };

    let reservation = admission.position_sizing_open_order_reservation_from_known_metadata(
        BoltV3SubmitPositionSizingOpenOrderEvidence {
            client_order_id: "client-order-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            open_quantity: Decimal::new(11, 0),
            limit_price: Decimal::new(4, 1),
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_cache".to_string(),
        },
        &recovered,
    );

    assert!(reservation.is_none());
}

#[test]
fn configured_submit_sizer_recovered_reconciliation_fill_does_not_reduce_liability() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    let recovered = BoltV3RecoveredSubmitReservationEvidence {
        metadata: BoltV3SubmitReservationMetadataEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "client-order-1#1".to_string(),
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "USD".to_string(),
            capital_pool_id: "pool-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: "Buy".to_string(),
            submitted_quantity: "10".to_string(),
            liability_factor: "0.4".to_string(),
            additive_liability: "0.3".to_string(),
            reserved_liability: "4.3".to_string(),
            observed_at_ns: 950,
            source: "submit_admission".to_string(),
        },
        fill_trade_ids: Default::default(),
    };
    let reservation = admission
        .position_sizing_open_order_reservation_from_known_metadata(
            BoltV3SubmitPositionSizingOpenOrderEvidence {
                client_order_id: "client-order-1".to_string(),
                instrument_id: "instrument-yes.VENUE-A".to_string(),
                side: BoltV3CompiledOrderSide::Buy,
                open_quantity: Decimal::new(6, 0),
                limit_price: Decimal::new(4, 1),
                observed_at_ns: 1_000,
                evidence_label: "nt_open_order_cache".to_string(),
            },
            &recovered,
        )
        .expect("known Bolt metadata should rebuild residual open order");
    let rebuild =
        admission.rebuild_position_sizing_open_order_reservations(vec![reservation], 1_000);
    assert!(rebuild.accepted);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );

    let reconciliation = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-replayed-by-reconciliation".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(2, 0),
            observed_at_ns: 1_100,
            reconciliation: true,
            evidence_label: "nt_reconciliation_fill".to_string(),
        },
        1_100,
    );

    assert!(reconciliation.accepted);
    assert!(!reconciliation.unknown_reservation);
    assert_eq!(reconciliation.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );

    let replayed_as_incremental = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-replayed-by-reconciliation".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(2, 0),
            observed_at_ns: 1_101,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_101,
    );

    assert!(replayed_as_incremental.accepted);
    assert!(!replayed_as_incremental.unknown_reservation);
    assert_eq!(
        replayed_as_incremental.action,
        PositionSizingLifecycleAction::None
    );
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn configured_submit_sizer_rejects_open_order_evidence_for_unconfigured_contract() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let reservation = admission.position_sizing_open_order_reservation_from_evidence(
        BoltV3SubmitPositionSizingOpenOrderEvidence {
            client_order_id: "client-order-1".to_string(),
            instrument_id: "instrument-other.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            open_quantity: Decimal::new(10, 0),
            limit_price: Decimal::new(4, 1),
            observed_at_ns: 1_000,
            evidence_label: "nt_open_order_cache".to_string(),
        },
    );

    assert!(reservation.is_none());
}

#[test]
fn configured_submit_sizer_open_order_evidence_uses_zero_additive_without_fee_policy() {
    let admission = position_sized_admission_without_fee_slippage_policy();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));

    let reservation = admission
        .position_sizing_open_order_reservation_from_evidence(
            BoltV3SubmitPositionSizingOpenOrderEvidence {
                client_order_id: "client-order-1".to_string(),
                instrument_id: "instrument-yes.VENUE-A".to_string(),
                side: BoltV3CompiledOrderSide::Buy,
                open_quantity: Decimal::new(10, 0),
                limit_price: Decimal::new(4, 1),
                observed_at_ns: 1_000,
                evidence_label: "nt_open_order_cache".to_string(),
            },
        )
        .expect("configured binary open order should build without fee policy");

    assert_eq!(reservation.additive_liability, Decimal::ZERO);
    assert_eq!(reservation.liability, Decimal::new(4, 0));
}

#[test]
fn configured_submit_sizer_rejects_rebuilt_reservation_liability_mismatch() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "client-order-1#rebuilt",
            Decimal::new(44, 1),
        )],
        1_000,
    );

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_failed_rebuild_clears_stale_client_order_index_and_closes_gate() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("first submit should admit")
        .commit_submitted();

    let rebuild = admission.rebuild_position_sizing_open_order_reservations(
        vec![
            open_order_reservation(
                "client-order-1",
                "client-order-1#rebuilt-a",
                Decimal::new(43, 1),
            ),
            open_order_reservation(
                "client-order-1",
                "client-order-1#rebuilt-b",
                Decimal::new(43, 1),
            ),
        ],
        1_100,
    );

    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::DuplicateReservation),
            attempted_reservation_count: 2,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(false));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_101)
        .expect_err("failed rebuild must leave submit admission closed");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::ReconciliationRequired
        }
    ));
}

#[test]
fn configured_submit_sizer_reserves_entry_from_compiled_order_values() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    let permit = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit");

    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    drop(permit);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_records_reservation_metadata_before_admitted_decision() {
    let writer = Arc::new(SequencedSubmitReservationWriter::default());
    let admission = position_sized_admission_with_writer(writer.clone());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit");

    let events = writer.events();
    assert_eq!(events.len(), 2);
    let SequencedSubmitReservationEvent::Metadata(metadata) = &events[0] else {
        panic!("metadata must be recorded before admission decision: {events:?}");
    };
    assert_eq!(metadata.client_order_id, "client-order-1");
    assert_eq!(metadata.submit_reservation_id, "client-order-1#1");
    assert_eq!(metadata.venue_id, "VENUE-A");
    assert_eq!(metadata.account_id, "ACCOUNT-001");
    assert_eq!(metadata.product_kind, "prediction_market_binary");
    assert_eq!(metadata.collateral_currency, "USD");
    assert_eq!(metadata.capital_pool_id, "pool-1");
    assert_eq!(metadata.collateral_group_id, "group-1");
    assert_eq!(metadata.instrument_id, "instrument-yes.VENUE-A");
    assert_eq!(metadata.side, "Buy");
    assert_eq!(
        metadata
            .submitted_quantity
            .parse::<Decimal>()
            .expect("submitted quantity should parse"),
        Decimal::new(10, 0)
    );
    assert_eq!(
        metadata
            .liability_factor
            .parse::<Decimal>()
            .expect("liability factor should parse"),
        Decimal::new(40, 2)
    );
    assert_eq!(
        metadata
            .additive_liability
            .parse::<Decimal>()
            .expect("additive liability should parse"),
        Decimal::new(3, 1)
    );
    assert_eq!(
        metadata
            .reserved_liability
            .parse::<Decimal>()
            .expect("reserved liability should parse"),
        Decimal::new(43, 1)
    );
    assert_eq!(metadata.observed_at_ns, 1_000);
    assert_eq!(metadata.source, "submit_admission");
    assert!(matches!(
        &events[1],
        SequencedSubmitReservationEvent::Admission(decision)
            if decision.client_order_id == "client-order-1"
                && decision.outcome == BoltV3AdmissionOutcome::Admitted
    ));
}

#[test]
fn configured_submit_sizer_metadata_write_failure_rolls_back_without_admitted_decision() {
    let writer = Arc::new(FailingSubmitReservationMetadataWriter::default());
    let admission = position_sized_admission_with_writer(writer.clone());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect_err("metadata write failure must reject before admitted decision evidence");

    match error {
        BoltV3SubmitAdmissionError::EvidenceWriteFailed { reason } => {
            assert!(
                reason.contains("synthetic submit reservation metadata write failure"),
                "unexpected evidence write failure reason: {reason}"
            );
        }
        other => panic!("expected EvidenceWriteFailed, got {other:?}"),
    }
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    assert!(
        writer.admission_decisions().is_empty(),
        "admitted admission decision must not be written after metadata write failure"
    );
}

#[test]
fn configured_submit_sizer_refreshes_bootstrap_pool_snapshot_from_nt_state() {
    let admission = position_sized_admission_with_pool_observed_at(0);
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh NT state should refresh bootstrap pool evidence before reserve");
}

#[test]
fn configured_submit_sizer_reserves_no_outcome_buy_when_instrument_matches_nt_state() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    let mut request = sized_submit_request("client-order-1");
    request.instrument_id = "instrument-no.VENUE-A".to_string();
    request
        .position_sizing
        .as_mut()
        .expect("sized request should carry evidence")
        .prediction_market_outcome = Some(PredictionMarketOutcomeSide::No);

    let permit = admission
        .admit_at(&request, 1_000)
        .expect("NO outcome buy should reserve when instrument matches NT state");

    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
    drop(permit);
}

#[test]
fn configured_submit_sizer_uses_no_inventory_for_no_outcome_sell() {
    let admission = position_sized_admission();
    arm_default(&admission);
    let mut state = fresh_sizing_state(900);
    let ProductSizingSnapshot::PredictionMarketBinary(product) = &mut state.product_state;
    product.yes_position = Decimal::ZERO;
    product.no_position = Decimal::new(10, 0);
    admission.update_position_sizing_nt_components(components_from_state(state));
    rebuild_empty_position_sizer(&admission);

    let mut request = sized_submit_request("client-order-1");
    request.instrument_id = "instrument-no.VENUE-A".to_string();
    let evidence = request
        .position_sizing
        .as_mut()
        .expect("sized request should carry evidence");
    evidence.prediction_market_outcome = Some(PredictionMarketOutcomeSide::No);
    evidence.side = BoltV3CompiledOrderSide::Sell;

    let permit = admission
        .admit_at(&request, 1_000)
        .expect("NO outcome sell should use NO inventory");

    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(63, 1))
    );
    drop(permit);
}

#[test]
fn configured_submit_sizer_allows_no_outcome_risk_reducing_exit_without_reservation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    let mut state = fresh_sizing_state(900);
    let ProductSizingSnapshot::PredictionMarketBinary(product) = &mut state.product_state;
    product.yes_position = Decimal::ZERO;
    product.no_position = Decimal::new(10, 0);
    admission.update_position_sizing_nt_components(components_from_state(state));
    rebuild_empty_position_sizer(&admission);

    let mut request = sized_submit_request("client-order-1");
    request.intent_kind = BoltV3SubmitIntentKind::RiskReducingExit;
    request.instrument_id = "instrument-no.VENUE-A".to_string();
    let evidence = request
        .position_sizing
        .as_mut()
        .expect("sized request should carry evidence");
    evidence.prediction_market_outcome = Some(PredictionMarketOutcomeSide::No);
    evidence.side = BoltV3CompiledOrderSide::Sell;

    let permit = admission
        .admit_at(&request, 1_000)
        .expect("NO risk-reducing exit should admit against NO inventory");

    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
    drop(permit);
}

#[test]
fn configured_submit_sizer_keeps_committed_reservation_until_terminal_lifecycle_release() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let decision = admission.apply_position_sizing_lifecycle_update(
        BoltV3SubmitPositionSizingLifecycleUpdate {
            client_order_id: "client-order-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            remaining_liability: Decimal::ZERO,
            observed_at_ns: 1_100,
            evidence_label: "nt_order_terminal".to_string(),
            kind: PositionSizingLifecycleKind::Terminal,
        },
        1_100,
    );

    assert!(decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_revalues_residual_liability_from_fill_metadata() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn configured_submit_sizer_records_fill_evidence_before_revaluing_liability() {
    let writer = Arc::new(SequencedSubmitReservationWriter::default());
    let admission = position_sized_admission_with_writer(writer.clone());
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    let events = writer.events();
    assert_eq!(events.len(), 3);
    let SequencedSubmitReservationEvent::Fill(fill) = &events[2] else {
        panic!("fill evidence must be recorded before liability is reduced: {events:?}");
    };
    assert_eq!(fill.client_order_id, "client-order-1");
    assert_eq!(fill.submit_reservation_id, "client-order-1#1");
    assert_eq!(fill.trade_id, "trade-1");
    assert_eq!(fill.instrument_id, "instrument-yes.VENUE-A");
    assert_eq!(fill.side, "Buy");
    assert_eq!(
        fill.fill_quantity
            .parse::<Decimal>()
            .expect("fill quantity should parse"),
        Decimal::new(4, 0)
    );
    assert_eq!(fill.observed_at_ns, 1_100);
    assert!(!fill.reconciliation);
    assert_eq!(fill.source, "nt_order_fill");
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn configured_submit_sizer_fill_evidence_write_failure_leaves_liability_unchanged() {
    let writer = Arc::new(FailingSubmitReservationFillWriter);
    let admission = position_sized_admission_with_writer(writer);
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(!decision.accepted);
    assert!(!decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn configured_submit_sizer_keeps_per_order_additive_liability_after_partial_fill() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(9, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(7, 1))
    );
}

#[test]
fn configured_submit_sizer_applies_same_timestamp_fills_in_order() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let first = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        reconciliation: false,
        evidence_label: "nt_order_fill".to_string(),
    };
    let second = BoltV3SubmitPositionSizingFillUpdate {
        trade_id: "trade-2".to_string(),
        fill_quantity: Decimal::new(3, 0),
        ..first.clone()
    };

    let first_decision = admission.apply_position_sizing_fill_update(first, 1_100);
    let second_decision = admission.apply_position_sizing_fill_update(second, 1_100);

    assert!(first_decision.accepted);
    assert_eq!(
        first_decision.action,
        PositionSizingLifecycleAction::Revalued
    );
    assert!(second_decision.accepted);
    assert_eq!(
        second_decision.action,
        PositionSizingLifecycleAction::Revalued
    );
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(15, 1))
    );
}

#[test]
fn configured_submit_sizer_ignores_duplicate_fill_trade_id() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let fill = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        reconciliation: false,
        evidence_label: "nt_order_fill".to_string(),
    };

    let first = admission.apply_position_sizing_fill_update(fill.clone(), 1_100);
    let duplicate = admission.apply_position_sizing_fill_update(fill, 1_200);

    assert!(first.accepted);
    assert_eq!(first.action, PositionSizingLifecycleAction::Revalued);
    assert!(duplicate.accepted);
    assert_eq!(duplicate.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn configured_submit_sizer_full_fill_releases_reservation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(10, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Released);
    assert!(!decision.unknown_reservation);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );

    let terminal = admission.apply_position_sizing_terminal_order_event(
        "client-order-1".to_string(),
        1_200,
        "nt_order_terminal".to_string(),
    );
    assert!(terminal.unknown_reservation);
    assert_eq!(terminal.action, PositionSizingLifecycleAction::None);
}

#[test]
fn configured_submit_sizer_rejects_mismatched_fill_without_mutation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-no.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn configured_submit_sizer_rejects_invalid_fill_fields_without_mutation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let valid = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        reconciliation: false,
        evidence_label: "nt_order_fill".to_string(),
    };
    for invalid in [
        BoltV3SubmitPositionSizingFillUpdate {
            trade_id: " ".to_string(),
            ..valid.clone()
        },
        BoltV3SubmitPositionSizingFillUpdate {
            fill_quantity: Decimal::ZERO,
            ..valid.clone()
        },
        BoltV3SubmitPositionSizingFillUpdate {
            fill_quantity: Decimal::new(-1, 0),
            ..valid.clone()
        },
        BoltV3SubmitPositionSizingFillUpdate {
            side: BoltV3CompiledOrderSide::Sell,
            ..valid.clone()
        },
    ] {
        let decision = admission.apply_position_sizing_fill_update(invalid, 1_100);
        assert!(decision.unknown_reservation);
        assert_eq!(decision.action, PositionSizingLifecycleAction::None);
        assert_eq!(
            admission.position_sizer_live_reserved_liability(),
            Some(Decimal::new(43, 1))
        );
    }
}

#[test]
fn configured_submit_sizer_rejects_duplicate_trade_id_with_mismatched_content() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let valid = BoltV3SubmitPositionSizingFillUpdate {
        client_order_id: "client-order-1".to_string(),
        trade_id: "trade-1".to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        fill_quantity: Decimal::new(4, 0),
        observed_at_ns: 1_100,
        reconciliation: false,
        evidence_label: "nt_order_fill".to_string(),
    };
    let mismatched = BoltV3SubmitPositionSizingFillUpdate {
        instrument_id: "instrument-no.VENUE-A".to_string(),
        observed_at_ns: 1_200,
        ..valid.clone()
    };

    assert!(
        admission
            .apply_position_sizing_fill_update(valid, 1_100)
            .accepted
    );
    let decision = admission.apply_position_sizing_fill_update(mismatched, 1_200);

    assert!(decision.unknown_reservation);
    assert_eq!(decision.action, PositionSizingLifecycleAction::None);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(27, 1))
    );
}

#[test]
fn configured_submit_sizer_revalues_sell_residual_liability_from_fill_metadata() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    let mut request = sized_submit_request("client-order-1");
    request
        .position_sizing
        .as_mut()
        .expect("sized request should carry evidence")
        .side = BoltV3CompiledOrderSide::Sell;
    admission
        .admit_at(&request, 1_000)
        .expect("fresh sell sizing state and capacity should admit")
        .commit_submitted();
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(63, 1))
    );

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Sell,
            fill_quantity: Decimal::new(4, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Revalued);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(39, 1))
    );
}

#[test]
fn configured_submit_sizer_rebuilt_reservation_full_fill_releases_reservation() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    let rebuild = admission.rebuild_position_sizing_open_order_reservations(
        vec![open_order_reservation(
            "client-order-1",
            "submit-reservation-1",
            Decimal::new(43, 1),
        )],
        1_000,
    );
    assert!(rebuild.accepted);

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(10, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Released);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_clamps_overfill_to_full_release() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);
    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("fresh sizing state and capacity should admit")
        .commit_submitted();

    let decision = admission.apply_position_sizing_fill_update(
        BoltV3SubmitPositionSizingFillUpdate {
            client_order_id: "client-order-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "instrument-yes.VENUE-A".to_string(),
            side: BoltV3CompiledOrderSide::Buy,
            fill_quantity: Decimal::new(12, 0),
            observed_at_ns: 1_100,
            reconciliation: false,
            evidence_label: "nt_order_fill".to_string(),
        },
        1_100,
    );

    assert!(decision.accepted);
    assert_eq!(decision.action, PositionSizingLifecycleAction::Released);
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::ZERO)
    );
}

#[test]
fn configured_submit_sizer_rejects_duplicate_client_order_id_without_mutating_ledger() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect("first submit should admit")
        .commit_submitted();

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_001)
        .expect_err("duplicate client order id must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::DuplicateClientOrderId
        }
    ));
    assert_eq!(
        admission.position_sizer_live_reserved_liability(),
        Some(Decimal::new(43, 1))
    );
}

#[test]
fn configured_submit_sizer_unknown_lifecycle_client_order_id_returns_noop_outcome() {
    let admission = position_sized_admission();
    arm_default(&admission);

    let decision = admission.apply_position_sizing_lifecycle_update(
        BoltV3SubmitPositionSizingLifecycleUpdate {
            client_order_id: "unknown-client-order".to_string(),
            collateral_group_id: "group-1".to_string(),
            remaining_liability: Decimal::ZERO,
            observed_at_ns: 1_100,
            evidence_label: "nt_order_terminal".to_string(),
            kind: PositionSizingLifecycleKind::Terminal,
        },
        1_100,
    );

    assert!(decision.accepted);
    assert!(decision.unknown_reservation);
}

#[test]
fn configured_submit_sizer_rejects_outcome_instrument_mismatch() {
    let admission = position_sized_admission();
    arm_default(&admission);
    admission.update_position_sizing_nt_components(fresh_components(900));
    rebuild_empty_position_sizer(&admission);

    let mut mismatch = sized_submit_request("client-order-2");
    mismatch.instrument_id = "other.VENUE-A".to_string();
    let error = admission
        .admit_at(&mismatch, 1_000)
        .expect_err("instrument mismatch must reject");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::OutcomeInstrumentMismatch
        }
    ));
}

#[test]
fn configured_submit_sizer_rejects_collateral_mismatch_from_nt_state() {
    let admission = position_sized_admission();
    arm_default(&admission);
    let mut state = fresh_sizing_state(900);
    state.portfolio.collateral_currency = "USDC".to_string();
    admission.update_position_sizing_nt_components(components_from_state(state));

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect_err("collateral mismatch between configured pool and NT state must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::CollateralCurrencyMismatch
        }
    ));
}

#[test]
fn configured_submit_sizer_rejects_account_mismatch_from_nt_state() {
    let admission = position_sized_admission();
    arm_default(&admission);
    let mut state = fresh_sizing_state(900);
    state.portfolio.account_id = "ACCOUNT-002".to_string();
    admission.update_position_sizing_nt_components(components_from_state(state));

    let error = admission
        .admit_at(&sized_submit_request("client-order-1"), 1_000)
        .expect_err("account mismatch between configured pool and NT state must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::PositionSizingRejected {
            reason: BoltV3PositionSizerRejectReason::AccountMismatch
        }
    ));
}

#[derive(Debug)]
struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, anyhow::Result<()>> {
        async { Ok(()) }.boxed()
    }
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    submit_request_with_kind(notional, BoltV3SubmitIntentKind::Entry)
}

fn loss_policy() -> LossGovernorPolicy {
    LossGovernorPolicy {
        max_snapshot_age_ns: 1_000,
        max_per_trade_loss: Some(Decimal::new(10, 0)),
        max_daily_loss: Some(Decimal::new(25, 0)),
        max_rolling_loss: Some(Decimal::new(30, 0)),
        max_drawdown: Some(Decimal::new(40, 0)),
    }
}

fn broad_freshness_loss_policy() -> LossGovernorPolicy {
    LossGovernorPolicy {
        max_snapshot_age_ns: 60_000_000_000,
        ..loss_policy()
    }
}

fn current_test_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test clock should be after UNIX epoch")
        .as_nanos()
        .try_into()
        .expect("test unix timestamp should fit u64 nanoseconds")
}

fn fresh_below_limit_loss_snapshot(observed_at_ns: u64) -> LossSnapshot {
    LossSnapshot {
        source: "nt_portfolio_snapshot".to_string(),
        observed_at_ns,
        per_trade_pnl: Some(Decimal::new(-9, 0)),
        daily_pnl: Some(Decimal::new(-24, 0)),
        rolling_pnl: Some(Decimal::new(-29, 0)),
        current_equity: Some(Decimal::new(961, 0)),
        peak_equity: Some(Decimal::new(1000, 0)),
    }
}

fn breached_loss_snapshot(observed_at_ns: u64) -> LossSnapshot {
    LossSnapshot {
        source: "nt_portfolio_snapshot".to_string(),
        observed_at_ns,
        per_trade_pnl: Some(Decimal::new(-10, 0)),
        daily_pnl: Some(Decimal::new(-25, 0)),
        rolling_pnl: Some(Decimal::new(-30, 0)),
        current_equity: Some(Decimal::new(960, 0)),
        peak_equity: Some(Decimal::new(1000, 0)),
    }
}

fn submit_request_with_kind(
    notional: Decimal,
    intent_kind: BoltV3SubmitIntentKind,
) -> BoltV3SubmitAdmissionRequest {
    submit_request_with_kind_policy_and_exit_proof(
        notional,
        intent_kind,
        BoltV3SubmitLifecyclePolicy::new(true),
        None,
    )
}

fn submit_request_with_kind_and_policy(
    notional: Decimal,
    intent_kind: BoltV3SubmitIntentKind,
    lifecycle_policy: BoltV3SubmitLifecyclePolicy,
) -> BoltV3SubmitAdmissionRequest {
    submit_request_with_kind_policy_and_exit_proof(notional, intent_kind, lifecycle_policy, None)
}

fn submit_request_with_kind_and_exit_proof(
    notional: Decimal,
    intent_kind: BoltV3SubmitIntentKind,
    risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
) -> BoltV3SubmitAdmissionRequest {
    submit_request_with_kind_policy_and_exit_proof(
        notional,
        intent_kind,
        BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_proof,
    )
}

fn submit_request_with_kind_policy_and_exit_proof(
    notional: Decimal,
    intent_kind: BoltV3SubmitIntentKind,
    lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
) -> BoltV3SubmitAdmissionRequest {
    let (order_side, order_quantity) = match intent_kind {
        BoltV3SubmitIntentKind::RiskReducingExit => (OrderSide::Sell, Decimal::new(264, 2)),
        BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit => {
            (OrderSide::Buy, Decimal::new(1, 0))
        }
    };
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        order_side,
        order_quantity,
        intent_kind,
        lifecycle_policy,
        canary_proof_claim: None,
        risk_reducing_exit_proof,
        position_sizing: None,
    }
}

fn valid_risk_reducing_exit_proof() -> BoltV3RiskReducingExitProof {
    BoltV3RiskReducingExitProof {
        position_id: "position-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        position_side: PositionSide::Long,
        exit_order_side: OrderSide::Sell,
        position_quantity: Decimal::new(264, 2),
        exit_quantity: Decimal::new(264, 2),
    }
}

fn position_sized_admission() -> BoltV3SubmitAdmissionState {
    position_sized_admission_with_pool_observed_at(900)
}

fn position_sized_admission_without_fee_slippage_policy() -> BoltV3SubmitAdmissionState {
    position_sized_admission_with_policy(900, None)
}

fn position_sized_admission_with_pool_observed_at(
    pool_observed_at_ns: u64,
) -> BoltV3SubmitAdmissionState {
    position_sized_admission_with_policy(
        pool_observed_at_ns,
        Some(FeeSlippagePolicy {
            max_fee_liability: Decimal::new(10, 2),
            max_slippage_liability: Decimal::new(20, 2),
        }),
    )
}

fn position_sized_admission_with_policy(
    pool_observed_at_ns: u64,
    fee_slippage_policy: Option<FeeSlippagePolicy>,
) -> BoltV3SubmitAdmissionState {
    position_sized_admission_with_policy_and_writer(
        pool_observed_at_ns,
        fee_slippage_policy,
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
    )
}

fn position_sized_admission_with_writer(
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> BoltV3SubmitAdmissionState {
    position_sized_admission_with_policy_and_writer(
        900,
        Some(FeeSlippagePolicy {
            max_fee_liability: Decimal::new(10, 2),
            max_slippage_liability: Decimal::new(20, 2),
        }),
        writer,
    )
}

fn position_sized_admission_with_policy_and_writer(
    pool_observed_at_ns: u64,
    fee_slippage_policy: Option<FeeSlippagePolicy>,
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> BoltV3SubmitAdmissionState {
    BoltV3SubmitAdmissionState::new_unarmed_with_position_sizer(
        writer,
        BoltV3SubmitPositionSizerConfig {
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "USD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "bolt_submit_sizer_bootstrap".to_string(),
                observed_at_ns: pool_observed_at_ns,
                pool_id: "pool-1".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: 500,
            },
            policy: SizingPolicy {
                mode: SizingMode::RejectOnly,
                max_order_liability: Some(Decimal::new(10, 0)),
                min_remaining_pool_balance: None,
                fee_slippage_policy,
            },
        },
    )
}

fn arm_default(admission: &BoltV3SubmitAdmissionState) {
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            10,
            Decimal::new(10, 0),
        ))
        .expect("valid gate report should arm admission");
}

fn rebuild_empty_position_sizer(admission: &BoltV3SubmitAdmissionState) {
    let rebuild = admission.rebuild_position_sizing_open_order_reservations(Vec::new(), 1_000);
    assert_eq!(
        rebuild,
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: true,
            reason: None,
            attempted_reservation_count: 0,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
        }
    );
    assert_eq!(admission.position_sizer_reconciled(), Some(true));
}

fn open_order_reservation(
    client_order_id: &str,
    submit_reservation_id: &str,
    liability: Decimal,
) -> BoltV3SubmitPositionSizingOpenOrderReservation {
    BoltV3SubmitPositionSizingOpenOrderReservation {
        client_order_id: client_order_id.to_string(),
        submit_reservation_id: submit_reservation_id.to_string(),
        collateral_group_id: "group-1".to_string(),
        liability,
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        side: BoltV3CompiledOrderSide::Buy,
        open_quantity: Decimal::new(10, 0),
        original_quantity: Decimal::new(10, 0),
        filled_quantity: Decimal::ZERO,
        liability_factor: Decimal::new(4, 1),
        additive_liability: Decimal::new(3, 1),
        seen_trade_ids: Default::default(),
        recovered_from_startup: false,
        observed_at_ns: 900,
        evidence_label: "nt_open_order_rebuild".to_string(),
    }
}

fn sized_submit_request(client_order_id: &str) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        client_order_id: client_order_id.to_string(),
        instrument_id: "instrument-yes.VENUE-A".to_string(),
        notional: Decimal::new(4, 0),
        order_side: OrderSide::Buy,
        order_quantity: Decimal::new(10, 0),
        intent_kind: BoltV3SubmitIntentKind::Entry,
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        canary_proof_claim: None,
        risk_reducing_exit_proof: None,
        position_sizing: Some(BoltV3CompiledOrderSizingEvidence {
            venue_id: "VENUE-A".to_string(),
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side: BoltV3CompiledOrderSide::Buy,
            quantity: Decimal::new(10, 0),
            effective_price: Decimal::new(40, 2),
            order_kind: BoltV3CompiledOrderKind::Limit,
            liquidity: BoltV3CompiledOrderLiquidity::Taker,
            quote_set_id: None,
            prediction_market_outcome: Some(PredictionMarketOutcomeSide::Yes),
        }),
    }
}

fn fresh_sizing_state(observed_at_ns: u64) -> NtDerivedSizingState {
    NtDerivedSizingState {
        source: "nt_sizing_state".to_string(),
        observed_at_ns,
        portfolio: PortfolioSizingSnapshot {
            source: "nt_portfolio_snapshot".to_string(),
            observed_at_ns,
            venue_id: "VENUE-A".to_string(),
            account_id: "ACCOUNT-001".to_string(),
            collateral_currency: "USD".to_string(),
            free_collateral: Decimal::new(100, 0),
            total_equity: Decimal::new(100, 0),
        },
        order_lifecycle: OrderLifecycleSizingSnapshot {
            source: "nt_open_order_cache".to_string(),
            observed_at_ns,
            open_order_count: 0,
            all_open_orders_attributed: true,
        },
        product_state: ProductSizingSnapshot::PredictionMarketBinary(
            PredictionMarketSizingSnapshot {
                source: "nt_prediction_market_snapshot".to_string(),
                observed_at_ns,
                yes_instrument_id: "instrument-yes.VENUE-A".to_string(),
                no_instrument_id: "instrument-no.VENUE-A".to_string(),
                yes_position: Decimal::new(10, 0),
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::new(100, 0),
                conditional_token_allowance: Decimal::new(10, 0),
                collateral_coupled_group_id: "group-1".to_string(),
            },
        ),
        reservation_snapshot: ReservationLedgerSnapshot {
            source: "bolt_reservation_ledger".to_string(),
            observed_at_ns,
            all_live_reservations_attributed: true,
        },
        loss_snapshot: None,
    }
}

fn fresh_components(observed_at_ns: u64) -> BoltV3SubmitPositionSizingNtComponents {
    components_from_state(fresh_sizing_state(observed_at_ns))
}

fn components_from_state(state: NtDerivedSizingState) -> BoltV3SubmitPositionSizingNtComponents {
    BoltV3SubmitPositionSizingNtComponents {
        source: state.source,
        observed_at_ns: state.observed_at_ns,
        portfolio: state.portfolio,
        order_lifecycle: state.order_lifecycle,
        product_state: state.product_state,
        loss_snapshot: state.loss_snapshot,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SequencedSubmitReservationEvent {
    Metadata(BoltV3SubmitReservationMetadataEvidence),
    Admission(BoltV3AdmissionDecisionEvidence),
    Fill(BoltV3SubmitReservationFillEvidence),
}

#[derive(Debug, Default)]
struct SequencedSubmitReservationWriter {
    events: Mutex<Vec<SequencedSubmitReservationEvent>>,
}

impl SequencedSubmitReservationWriter {
    fn events(&self) -> Vec<SequencedSubmitReservationEvent> {
        self.events
            .lock()
            .expect("sequenced submit reservation writer mutex should not be poisoned")
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for SequencedSubmitReservationWriter {
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
        self.events
            .lock()
            .expect("sequenced submit reservation writer mutex should not be poisoned")
            .push(SequencedSubmitReservationEvent::Admission(decision.clone()));
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        self.events
            .lock()
            .expect("sequenced submit reservation writer mutex should not be poisoned")
            .push(SequencedSubmitReservationEvent::Metadata(metadata.clone()));
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        self.events
            .lock()
            .expect("sequenced submit reservation writer mutex should not be poisoned")
            .push(SequencedSubmitReservationEvent::Fill(fill.clone()));
        Ok(())
    }
}

#[derive(Debug, Default)]
struct FailingSubmitReservationMetadataWriter {
    admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
}

impl FailingSubmitReservationMetadataWriter {
    fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
        self.admission_decisions
            .lock()
            .expect("failing metadata writer mutex should not be poisoned")
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for FailingSubmitReservationMetadataWriter {
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
            .expect("failing metadata writer mutex should not be poisoned")
            .push(decision.clone());
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "synthetic submit reservation metadata write failure"
        ))
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailingSubmitReservationFillWriter;

impl BoltV3DecisionEvidenceWriter for FailingSubmitReservationFillWriter {
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
        _decision: &BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "synthetic submit reservation fill write failure"
        ))
    }
}

#[derive(Debug)]
struct FailingDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for FailingDecisionEvidenceWriter {
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
        _decision: &BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "synthetic admission-decision write failure"
        ))
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailingPositionSizerRebuildAuditWriter;

impl BoltV3DecisionEvidenceWriter for FailingPositionSizerRebuildAuditWriter {
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
        _decision: &BoltV3AdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "synthetic position-sizer rebuild audit write failure"
        ))
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct BlockingFirstAdmissionDecisionWriter {
    state: Mutex<BlockingFirstAdmissionDecisionWriterState>,
    entered: Condvar,
    released: Condvar,
}

#[derive(Debug, Default)]
struct BlockingFirstAdmissionDecisionWriterState {
    first_call_entered: bool,
    release_first_call: bool,
    admission_decisions: Vec<BoltV3AdmissionDecisionEvidence>,
}

impl BlockingFirstAdmissionDecisionWriter {
    fn wait_until_first_call_entered(&self) {
        let mut state = self
            .state
            .lock()
            .expect("blocking writer mutex should not be poisoned");
        while !state.first_call_entered {
            state = self
                .entered
                .wait(state)
                .expect("blocking writer condvar should not be poisoned");
        }
    }

    fn release_first_call(&self) {
        let mut state = self
            .state
            .lock()
            .expect("blocking writer mutex should not be poisoned");
        state.release_first_call = true;
        self.released.notify_all();
    }

    fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
        self.state
            .lock()
            .expect("blocking writer mutex should not be poisoned")
            .admission_decisions
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for BlockingFirstAdmissionDecisionWriter {
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
        let mut state = self
            .state
            .lock()
            .expect("blocking writer mutex should not be poisoned");
        if !state.first_call_entered {
            state.first_call_entered = true;
            self.entered.notify_all();
            while !state.release_first_call {
                state = self
                    .released
                    .wait(state)
                    .expect("blocking writer condvar should not be poisoned");
            }
        }
        state.admission_decisions.push(decision.clone());
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn admit_records_admission_decision_evidence_on_admit_outcome() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let request = submit_request(Decimal::new(1, 0));
    admission
        .admit(&request)
        .expect("first within-cap submit should admit");

    let decisions = writer.admission_decisions();
    assert_eq!(
        decisions.len(),
        1,
        "exactly one admission decision recorded"
    );
    assert_eq!(decisions[0].outcome, BoltV3AdmissionOutcome::Admitted);
    assert_eq!(decisions[0].strategy_id, request.strategy_id);
    assert_eq!(decisions[0].client_order_id, request.client_order_id);
    assert_eq!(decisions[0].instrument_id, request.instrument_id);
    assert_eq!(decisions[0].notional, request.notional.to_string());
    assert_eq!(decisions[0].intent_kind, request.intent_kind);
}

#[test]
fn entry_replace_and_exit_submit_intents_are_classified_before_admission() {
    let policy = BoltV3SubmitLifecyclePolicy::new(false);

    assert_eq!(
        policy.submit_intent_for(BoltV3OrderLifecycleIntent::Entry),
        Ok(Some(BoltV3SubmitIntentKind::Entry))
    );
    assert_eq!(
        policy.submit_intent_for(BoltV3OrderLifecycleIntent::RiskReducingExit),
        Ok(Some(BoltV3SubmitIntentKind::RiskReducingExit))
    );
    assert_eq!(
        policy.submit_intent_for(BoltV3OrderLifecycleIntent::ReplaceSubmit),
        Ok(None)
    );
}

#[test]
fn submit_lifecycle_policy_source_removes_dead_risk_reducing_exit_flag() {
    let source = include_str!("../src/bolt_v3_submit_admission.rs");

    assert!(
        !source.contains("_risk_reducing_exit_after_entry"),
        "strict count-cap enforcement must not retain a dead underscore-prefixed policy field"
    );
    assert!(
        !source.contains("risk_reducing_exit_after_entry: bool"),
        "strict count-cap enforcement must not retain a dead constructor parameter"
    );
}

#[test]
fn verified_risk_reducing_exit_after_entry_uses_exit_slot_not_entry_notional_or_entry_slot() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry at the configured cap should consume the entry slot");

    admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect("verified risk-reducing exit should bypass the entry notional cap and use its exit slot");
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|decision| decision.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::Admitted,
        ]
    );
    assert_eq!(admission.admitted_order_count(), 2);
}

#[test]
fn unproven_risk_reducing_exit_fails_closed_before_notional_bypass() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            2,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry at the configured cap should admit");

    let exit = admission
        .admit(&submit_request_with_kind(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
        ))
        .expect_err("unproven risk-reducing exit must not bypass the notional cap");

    assert!(matches!(
        exit,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    ));
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|d| d.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof,
        ]
    );
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn malformed_risk_reducing_exit_proof_fails_closed() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let mut proof = valid_risk_reducing_exit_proof();
    proof.exit_order_side = OrderSide::Buy;
    let error = admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(proof),
        ))
        .expect_err("a same-direction buy against a long position must not prove risk reduction");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    ));
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|d| d.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof]
    );
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn risk_reducing_exit_proof_must_match_actual_order_side() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let mut request = submit_request_with_kind_and_exit_proof(
        Decimal::new(264, 2),
        BoltV3SubmitIntentKind::RiskReducingExit,
        Some(valid_risk_reducing_exit_proof()),
    );
    request.order_side = OrderSide::Buy;

    let error = admission
        .admit(&request)
        .expect_err("request order side must match the proof side before an exit bypasses cap");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn risk_reducing_exit_proof_must_match_actual_order_quantity() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let mut request = submit_request_with_kind_and_exit_proof(
        Decimal::new(264, 2),
        BoltV3SubmitIntentKind::RiskReducingExit,
        Some(valid_risk_reducing_exit_proof()),
    );
    request.order_quantity = Decimal::new(132, 2);

    let error = admission
        .admit(&request)
        .expect_err("request order quantity must match proof quantity before an exit bypasses cap");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn risk_reducing_exit_proof_rejects_over_position_quantity() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let mut proof = valid_risk_reducing_exit_proof();
    proof.position_quantity = Decimal::new(1, 0);
    let error = admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(proof),
        ))
        .expect_err("exit quantity above position quantity must fail closed");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn second_entry_exhausts_entry_slot_even_when_exit_slot_is_unused() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("first entry should admit");

    let second_entry = admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect_err("second entry must not consume the independent exit slot");

    assert!(matches!(
        second_entry,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|d| d.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedCountCapExhausted,
        ]
    );
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn second_verified_risk_reducing_exit_exhausts_exit_slot() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry should admit");
    admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect("first verified risk-reducing exit should admit");

    let second_exit = admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect_err("second verified risk-reducing exit must exhaust the exit slot");

    assert!(matches!(
        second_exit,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|d| d.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedCountCapExhausted,
        ]
    );
    assert_eq!(admission.admitted_order_count(), 2);
}

#[test]
fn replace_submit_uses_replace_slot_after_entry_and_exit_slots_are_consumed() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(5, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry should admit");
    admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect("risk-reducing exit should admit");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::ReplaceSubmit,
        ))
        .expect("replace-submit must use the independent replace slot");

    assert_eq!(admission.admitted_order_count(), 3);
}

#[test]
fn replace_submit_rejects_when_lifecycle_policy_disables_replace() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let replace = admission
        .admit(&submit_request_with_kind_and_policy(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::ReplaceSubmit,
            BoltV3SubmitLifecyclePolicy::new(false),
        ))
        .expect_err("disabled lifecycle policy must reject replace-submit");

    assert!(matches!(
        replace,
        BoltV3SubmitAdmissionError::SubmitLifecycleDisallowed {
            intent: BoltV3SubmitIntentKind::ReplaceSubmit
        }
    ));
    let decisions = writer.admission_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].outcome,
        BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed
    );
    assert_eq!(
        decisions[0].intent_kind,
        BoltV3SubmitIntentKind::ReplaceSubmit
    );
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn plain_cancel_lifecycle_intent_is_not_a_submit_candidate() {
    let policy = BoltV3SubmitLifecyclePolicy::new(true);

    assert_eq!(
        policy.submit_intent_for(BoltV3OrderLifecycleIntent::PlainCancel),
        Ok(None),
        "plain cancel is not a live submit candidate and must not consume admission budget"
    );
}

#[test]
fn admit_records_admission_decision_evidence_for_each_rejection_path() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_unarmed(writer.clone());

    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect_err("unarmed admission must reject");
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");
    admission
        .admit(&submit_request(Decimal::ZERO))
        .expect_err("zero notional must reject");
    admission
        .admit(&submit_request(Decimal::new(2, 0)))
        .expect_err("over-cap notional must reject");
    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("first within-cap submit should admit");
    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect_err("second submit must exhaust count cap");

    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|d| d.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::RejectedNotArmed,
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional,
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedCountCapExhausted,
        ],
        "every admit return path must record evidence with the correct outcome"
    );
}

#[test]
fn admit_surfaces_evidence_write_failure_as_typed_error_and_does_not_consume_count() {
    let admission =
        BoltV3SubmitAdmissionState::new_unarmed(Arc::new(FailingDecisionEvidenceWriter));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let error = admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect_err("evidence-write failure must surface as a typed error");

    match error {
        BoltV3SubmitAdmissionError::EvidenceWriteFailed { reason } => {
            assert!(
                reason.contains("synthetic admission-decision write failure"),
                "wrapped reason must propagate the underlying writer error; got `{reason}`"
            );
        }
        other => panic!("expected EvidenceWriteFailed, got {other:?}"),
    }
    assert_eq!(
        admission.admitted_order_count(),
        0,
        "evidence-write failure must not consume an admission slot — the decision is not finalized until audit is durable"
    );
}

#[test]
fn admit_serializes_while_admission_evidence_is_in_flight() {
    let writer = Arc::new(BlockingFirstAdmissionDecisionWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new_unarmed(writer.clone()));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            1,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    let first_admission = admission.clone();
    let first_handle =
        thread::spawn(move || first_admission.admit(&submit_request(Decimal::new(1, 0))));
    writer.wait_until_first_call_entered();

    let (second_started_tx, second_started_rx) = mpsc::channel();
    let (second_result_tx, second_result_rx) = mpsc::channel();
    let second_admission = admission.clone();
    let second_handle = thread::spawn(move || {
        second_started_tx
            .send(())
            .expect("second admission start signal should send");
        let result = second_admission.admit(&submit_request(Decimal::new(1, 0)));
        second_result_tx
            .send(result)
            .expect("second admission result should send");
    });

    second_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second admission thread should start before first evidence write is released");
    assert!(matches!(
        second_result_rx.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));

    writer.release_first_call();
    first_handle
        .join()
        .expect("first admission thread should not panic")
        .expect("first admission should pass");
    let second = second_result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("second admission should complete after first evidence write is released");
    let second_error = second.expect_err("second admission must observe the consumed count slot");
    second_handle
        .join()
        .expect("second admission thread should not panic");

    assert!(matches!(
        second_error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|d| d.outcome)
        .collect();
    assert_eq!(
        outcomes,
        vec![
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedCountCapExhausted,
        ],
        "admission must serialize evaluate -> durable decision evidence -> counter mutation"
    );
    assert_eq!(admission.admitted_order_count(), 1);
}
