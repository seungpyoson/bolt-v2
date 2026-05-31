mod support;

use bolt_v2::bolt_v3_config::load_bolt_v3_config;
use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
    BoltV3OrderIntentEvidence, BoltV3StrategyInputEvidenceSnapshot,
};
use bolt_v2::bolt_v3_live_node::build_bolt_v3_live_node_with;
use bolt_v2::bolt_v3_loss_governor::{LossGovernorPolicy, LossHaltReason, LossSnapshot};
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3OrderLifecycleIntent, BoltV3QuoteQuantityAdmissionInput, BoltV3QuoteQuantityOrderSide,
    BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest,
    BoltV3SubmitAdmissionState, BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
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
    let run_index = runner
        .find("let run_future = node.run();")
        .expect("live runner should enter NT run after submit admission is armed");

    assert!(
        report_index < arm_index && arm_index < run_index,
        "live runner must derive admission bounds, arm submit admission, then enter NT run"
    );
    assert!(
        !runner.contains("consume_bolt_v3_live_runner_approval"),
        "live runner must not block startup on operator approval consumption"
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
