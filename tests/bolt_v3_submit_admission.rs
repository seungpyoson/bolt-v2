mod support;

use bolt_v2::bolt_v3_config::load_bolt_v3_config;
use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter,
    BoltV3OrderIntentEvidence, BoltV3StrategyInputEvidenceSnapshot,
};
use bolt_v2::bolt_v3_live_node::build_bolt_v3_live_node_with;
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3OrderLifecycleIntent, BoltV3QuoteQuantityAdmissionInput, BoltV3QuoteQuantityOrderSide,
    BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
    BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
    conservative_quote_quantity_admission_notional, market_style_admission_ceiling_notional,
    rounded_order_admission_notional,
};
use bolt_v2::strategies::registry::FeeProvider;
use bolt_v2::strategies::registry::StrategyBuildContext;
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_model::identifiers::InstrumentId;
use rust_decimal::Decimal;
use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::Duration,
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
fn live_node_runner_consumes_operator_approval_before_arming_submit_admission() {
    let source = support::repo_text("src/bolt_v3_live_node.rs");
    let start = source
        .find("pub async fn run_bolt_v3_live_node")
        .expect("live runner entrypoint should exist");
    let end = source[start..]
        .find("pub async fn controlled_no_submit_readiness")
        .map(|offset| start + offset)
        .expect("next public function should bound live runner source");
    let runner = &source[start..end];

    let preflight_index = runner
        .find("check_bolt_v3_live_canary_pre_consumption_gate")
        .expect("live runner must use the pre-consumption gate before approval consumption");
    let consume_index = runner
        .find("consume_bolt_v3_live_runner_approval")
        .expect("live runner must atomically consume approval before arming submit admission");
    let arm_index = runner
        .find(".arm(")
        .expect("live runner should arm submit admission");

    assert!(
        preflight_index < consume_index && consume_index < arm_index,
        "live runner must preflight, atomically consume approval, then arm submit admission"
    );
    assert!(
        !runner.contains("check_bolt_v3_live_canary_gate(loaded)"),
        "live runner must not accept replayable pre-existing approval consumption proof"
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

fn submit_request_with_kind(
    notional: Decimal,
    intent_kind: BoltV3SubmitIntentKind,
) -> BoltV3SubmitAdmissionRequest {
    submit_request_with_kind_and_policy(
        notional,
        intent_kind,
        BoltV3SubmitLifecyclePolicy::new(true),
    )
}

fn submit_request_with_kind_and_policy(
    notional: Decimal,
    intent_kind: BoltV3SubmitIntentKind,
    lifecycle_policy: BoltV3SubmitLifecyclePolicy,
) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: "strategy-a".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        intent_kind,
        lifecycle_policy,
        canary_proof_claim: None,
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
fn risk_reducing_exit_is_admitted_within_operator_count_cap() {
    let admission = BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    admission
        .arm(support::validated_bolt_v3_live_canary_gate_report(
            2,
            Decimal::new(1, 0),
        ))
        .expect("valid gate report should arm admission");

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should consume the canary entry slot");
    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::RiskReducingExit,
        ))
        .expect("risk-reducing exit submit should remain admissible after one entry");

    let replace = admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::ReplaceSubmit,
        ))
        .expect_err("replace-submit must not bypass exhausted canary budget");

    assert!(matches!(
        replace,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert_eq!(admission.admitted_order_count(), 2);
}

#[test]
fn risk_reducing_exit_cannot_exceed_operator_live_order_count_cap() {
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
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should consume the only operator-approved count slot");

    let exit = admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::RiskReducingExit,
        ))
        .expect_err("risk-reducing exit must not exceed max_live_order_count=1");

    assert!(matches!(
        exit,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    let outcomes: Vec<BoltV3AdmissionOutcome> = writer
        .admission_decisions()
        .into_iter()
        .map(|decision| decision.outcome)
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
fn replace_submit_consumes_operator_count_budget_before_risk_reducing_exit() {
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
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should consume one canary count slot");
    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::ReplaceSubmit,
        ))
        .expect("replace-submit should be admissible when lifecycle policy enables it");

    let exit = admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::RiskReducingExit,
        ))
        .expect_err("replace-submit must consume count budget and leave no extra exit slot");

    assert!(matches!(
        exit,
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
fn strict_count_cap_rejects_second_submit_risk_reducing_exit() {
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
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should consume the only canary count slot");

    let exit = admission
        .admit(&submit_request_with_kind_and_policy(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::RiskReducingExit,
            BoltV3SubmitLifecyclePolicy::new(true),
        ))
        .expect_err("risk-reducing exit must not bypass exhausted count");

    assert!(matches!(
        exit,
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
