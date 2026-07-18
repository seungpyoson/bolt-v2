use crate::support;

use bolt_v2::bolt_v3_capital_admission::{CapitalAdmissionPolicy, ProductKind};
use bolt_v2::bolt_v3_capital_reservation::CapitalPoolSnapshot;
use bolt_v2::bolt_v3_config::load_bolt_v3_config;
use bolt_v2::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3BasketAdmissionDecisionEvidence,
    BoltV3CapitalAdmissionRebuildAuditEvidence, BoltV3DecisionEvidenceWriter,
    BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence, BoltV3ExitEvaluationEvidence,
    BoltV3LossGovernorHaltEvidence, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
    BoltV3OrderRejectEvidence, BoltV3OrderRejectReason, BoltV3RejectSource,
    BoltV3RequoteThrottleEvidence, BoltV3SettlementBookingErrorEvidence, BoltV3SettlementEvidence,
    BoltV3StaleLossReason, BoltV3StrategyInputEvidenceSnapshot,
    BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
};
use bolt_v2::bolt_v3_kill_switch::{KillSwitchHaltTrigger, KillSwitchState};
use bolt_v2::bolt_v3_live_node::build_bolt_v3_live_node_with;
use bolt_v2::bolt_v3_loss_governor::{
    LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSourceObservationTimestamps,
};
use bolt_v2::bolt_v3_strategy_context::StrategyBuildContext;
use bolt_v2::bolt_v3_submit_admission::{
    BoltV3CapitalAdmissionRejectReason, BoltV3KillSwitchForcedReductionClaim,
    BoltV3KillSwitchForcedReductionPolicy, BoltV3LiveSubmitApprovalLimits,
    BoltV3OrderLifecycleIntent, BoltV3QuoteQuantityAdmissionInput, BoltV3QuoteQuantityOrderSide,
    BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest,
    BoltV3SubmitAdmissionRequestInput, BoltV3SubmitAdmissionState,
    BoltV3SubmitCapitalAdmissionConfig, BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
    OrderValuationContext, build_submit_admission_request_from_order,
    conservative_quote_quantity_admission_notional, market_style_admission_ceiling_notional,
    order_economics_facts,
};
use nautilus_model::data::QuoteTick;
use nautilus_model::enums::{AssetClass, OrderSide, PositionSide, TimeInForce};
use nautilus_model::identifiers::{ClientOrderId, InstrumentId, StrategyId, Symbol, TraderId};
use nautilus_model::instruments::{BinaryOption, InstrumentAny};
use nautilus_model::orders::{LimitOrder, MarketOrder, MarketToLimitOrder, OrderAny};
use nautilus_model::types::{Currency, Price, Quantity};
use rust_decimal::Decimal;
use std::{
    collections::BTreeMap,
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::Duration,
};
use ustr::Ustr;

fn binary_option_with_max_price(instrument_id: InstrumentId) -> InstrumentAny {
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        Symbol::from("instrument-yes"),
        AssetClass::Alternative,
        Currency::USD(),
        nautilus_core::UnixNanos::from(1_u64),
        nautilus_core::UnixNanos::from(2_u64),
        2,
        2,
        Price::from("0.01"),
        Quantity::from("0.01"),
        Some(Ustr::from("YES")),
        None,
        None,
        Some(Quantity::from("0.01")),
        None,
        None,
        Some(Price::from("1.00")),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        nautilus_core::UnixNanos::from(1_u64),
        nautilus_core::UnixNanos::from(1_u64),
    ))
}

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
fn build_submit_admission_request_from_order_maps_base_limit_order() {
    let price = Price::new(0.50, 2);
    let quantity = Quantity::new(2.0, 2);
    let order = OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from("INSTRUMENT.SOURCE"),
            ClientOrderId::from("O-19700101-000000-001-A9-1"),
            OrderSide::Buy,
            quantity,
            price,
            TimeInForce::Gtc,
            None,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("limit order should be valid"),
    );
    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        "strategy-a".to_string(),
        BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );

    let input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: "hyperliquid_perps",
        intent: &intent,
        order: &order,
        valuation: OrderValuationContext::empty(),
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_position: None,
    };
    let economics_admission = support::sample_order_routing_handle("hyperliquid_perps")
        .quote_admission(
            bolt_v2::bolt_v3_order_execution::BoltV3OrderEconomicsIntent {
                request: &input,
                planned_fill_legs: vec![bolt_v2::bolt_v3_order_execution::BoltV3PlannedFillLeg {
                    price: Decimal::new(50, 2),
                    quantity: Decimal::new(2, 0),
                }],
                liquidity_role: bolt_v2::economics::LiquidityRoleAssumption::Taker,
                position: None,
                lifecycle_path: bolt_v2::economics::LifecyclePath::PlannedExit,
                requested_at_ns: 1,
                decision_correlation_id: "O-19700101-000000-001-A9-1",
                gross_expected_value: Decimal::ONE,
            },
        )
        .expect("sample economics admission should quote");
    let request = build_submit_admission_request_from_order(input, economics_admission)
        .expect("base limit admission request should build in shared admission module");

    assert_eq!(request.strategy_id, "strategy-a");
    assert_eq!(request.execution_client_id, "hyperliquid_perps");
    assert_eq!(request.client_order_id, "O-19700101-000000-001-A9-1");
    assert_eq!(request.instrument_id, "INSTRUMENT.SOURCE");
    assert_eq!(
        request.notional,
        Decimal::from_str_exact("1.0000").expect("expected decimal should parse")
    );
    assert_eq!(request.order_side, OrderSide::Buy);
    assert_eq!(
        request.order_quantity,
        Decimal::from_str_exact("2.00").expect("expected decimal should parse")
    );
    assert_eq!(request.intent_kind, BoltV3SubmitIntentKind::Entry);
}

#[test]
fn build_submit_admission_request_rejects_economics_purpose_mismatch() {
    let price = Price::new(0.50, 2);
    let order = OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from("INSTRUMENT.SOURCE"),
            ClientOrderId::from("O-19700101-000000-001-A9-PURPOSE"),
            OrderSide::Buy,
            Quantity::new(2.0, 2),
            price,
            TimeInForce::Gtc,
            None,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("limit order should be valid"),
    );
    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        "strategy-a".to_string(),
        BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );
    let input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: "hyperliquid_perps",
        intent: &intent,
        order: &order,
        valuation: OrderValuationContext::empty(),
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_position: None,
    };

    let error = build_submit_admission_request_from_order(
        input,
        support::sample_risk_reduction_economics_admission(Decimal::ONE),
    )
    .expect_err("entry evidence must reject a risk-reduction economics admission");

    assert!(
        error
            .to_string()
            .contains("economics admission purpose does not match final order intent")
    );
}

#[test]
fn order_valuation_context_selects_quote_quantity_prices_by_order_shape() {
    let instrument_id = InstrumentId::from("INSTRUMENT.SOURCE");
    let quantity = Quantity::new(2.0, 2);
    let quote = QuoteTick::new_checked(
        instrument_id,
        Price::new(0.39, 2),
        Price::new(0.41, 2),
        Quantity::new(10.0, 2),
        Quantity::new(10.0, 2),
        nautilus_core::UnixNanos::from(1_u64),
        nautilus_core::UnixNanos::from(1_u64),
    )
    .expect("quote should be valid");
    let market = OrderAny::Market(
        MarketOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            instrument_id,
            ClientOrderId::from("O-19700101-000000-001-A9-3"),
            OrderSide::Buy,
            quantity,
            TimeInForce::Gtc,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("quote-quantity market order should be valid"),
    );
    let limit_price = Price::new(0.50, 2);
    let limit = OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            instrument_id,
            ClientOrderId::from("O-19700101-000000-001-A9-4"),
            OrderSide::Buy,
            quantity,
            limit_price,
            TimeInForce::Gtc,
            None,
            false,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("quote-quantity limit order should be valid"),
    );
    let context = OrderValuationContext {
        last_quote: Some(quote),
        instrument: None,
    };

    assert_eq!(context.prices_for_order(&market), (None, None));
    assert_eq!(
        context.prices_for_order(&limit),
        (Some(limit_price), Some(Price::new(0.41, 2)))
    );
}

#[test]
fn quote_quantity_order_maps_quote_amount_to_canonical_fill_quantity() {
    let instrument_id = InstrumentId::from("INSTRUMENT.SOURCE");
    let instrument = binary_option_with_max_price(instrument_id);
    let price = Price::new(0.50, 2);
    let order = OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            instrument_id,
            ClientOrderId::from("O-19700101-000000-001-A9-40"),
            OrderSide::Buy,
            Quantity::new(25.0, 2),
            price,
            TimeInForce::Gtc,
            None,
            false,
            false,
            true,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("quote-quantity limit order should be valid"),
    );
    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        "strategy-a".to_string(),
        BoltV3OrderIntentKind::Entry,
        price.to_string(),
        &order,
    );
    let quote = QuoteTick::new_checked(
        instrument_id,
        Price::new(0.49, 2),
        price,
        Quantity::new(100.0, 2),
        Quantity::new(100.0, 2),
        nautilus_core::UnixNanos::from(1_u64),
        nautilus_core::UnixNanos::from(1_u64),
    )
    .expect("authoritative quote should be valid");
    let input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: "configured-execution-client",
        intent: &intent,
        order: &order,
        valuation: OrderValuationContext {
            last_quote: Some(quote),
            instrument: Some(&instrument),
        },
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_position: None,
    };

    let facts = order_economics_facts(&input).expect("canonical economics facts");

    assert_eq!(facts.planned_fill_quantity, Decimal::from(50));
}

#[test]
fn non_polymarket_market_order_uses_shared_structural_ceiling_valuation() {
    let instrument_id = InstrumentId::from("INSTRUMENT.HYPERLIQUID");
    let instrument = binary_option_with_max_price(instrument_id);
    let order = OrderAny::Market(
        MarketOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            instrument_id,
            ClientOrderId::from("O-19700101-000000-001-A9-5"),
            OrderSide::Buy,
            Quantity::new(2.0, 2),
            TimeInForce::Gtc,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("market order should be valid"),
    );
    let intent = BoltV3OrderIntentEvidence::from_compiled_order(
        "strategy-a".to_string(),
        BoltV3OrderIntentKind::Entry,
        "0.50".to_string(),
        &order,
    );

    let input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: "hyperliquid_perps",
        intent: &intent,
        order: &order,
        valuation: OrderValuationContext {
            instrument: Some(&instrument),
            ..OrderValuationContext::empty()
        },
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        risk_reducing_exit_position: None,
    };
    let economics_admission = support::sample_order_routing_handle("hyperliquid_perps")
        .quote_admission(
            bolt_v2::bolt_v3_order_execution::BoltV3OrderEconomicsIntent {
                request: &input,
                planned_fill_legs: vec![bolt_v2::bolt_v3_order_execution::BoltV3PlannedFillLeg {
                    price: Decimal::new(50, 2),
                    quantity: Decimal::new(2, 0),
                }],
                liquidity_role: bolt_v2::economics::LiquidityRoleAssumption::Taker,
                position: None,
                lifecycle_path: bolt_v2::economics::LifecyclePath::PlannedExit,
                requested_at_ns: 1,
                decision_correlation_id: "O-19700101-000000-001-A9-5",
                gross_expected_value: Decimal::ONE,
            },
        )
        .expect("sample economics admission should quote");
    let request = build_submit_admission_request_from_order(input, economics_admission)
        .expect("non-Polymarket market order should use the shared ceiling valuation");

    assert_eq!(request.execution_client_id, "hyperliquid_perps");
    assert_eq!(
        request.notional,
        Decimal::from_str_exact("2.0000").expect("expected decimal should parse")
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
fn economics_valuation_has_no_last_trade_or_raw_quantity_fallback() {
    let source = support::repo_text("src/bolt_v3_submit_admission.rs");

    assert!(!source.contains("quote_side_price.or_else"));
    assert!(!source.contains("from the raw quote quantity"));
}

#[test]
fn live_node_runner_does_not_require_evidence_gate_admission_before_nt_run() {
    let source = support::repo_text("src/bolt_v3_live_node.rs");
    let start = source
        .find("pub async fn run_bolt_v3_live_node")
        .expect("live runner entrypoint should exist");
    let end = source[start..]
        .find("fn classify_live_node_run_and_capture_shutdown")
        .map(|offset| start + offset)
        .expect("run classification should bound live runner source");
    let runner = &source[start..end];

    let run_index = runner
        .find("let run_future = node.run();")
        .expect("live runner should enter NT run through the wrapper");
    let capture_index = runner
        .find("wire_bolt_v3_runtime_capture(node, node_handle.clone(), loaded)")
        .expect("live runner should wire runtime capture before NT run");

    assert!(
        capture_index < run_index,
        "live runner must wire runtime capture before entering NT run"
    );
    assert!(
        !runner.contains("build_bolt_v3_live_submit_admission_report_from_config")
            && !runner.contains(".arm("),
        "live runner must not require the retired evidence gate before submit admission"
    );
    assert!(
        !runner.contains("consume_bolt_v3_live_runner_approval"),
        "live runner must not block startup on operator approval consumption"
    );
}

#[test]
fn ungated_submit_admission_allows_production_submit() {
    let admission = BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    let request = submit_request(Decimal::new(1, 0));

    let result = admission.admit(&request);
    let nt_submit_called = result.is_ok();

    result
        .expect("ungated production admission should allow a valid submit")
        .commit_submitted();
    assert!(nt_submit_called, "NT submit may be reached after admission");
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn expired_economics_quote_rejects_before_capacity_is_consumed() {
    let admission = BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    let mut request = submit_request(Decimal::ONE);
    request.economics_admission = support::sample_expired_economics_admission(Decimal::ONE);

    let error = admission
        .admit_at(&request, 3)
        .expect_err("expired economics must fail closed");

    assert_eq!(error, BoltV3SubmitAdmissionError::EconomicsQuoteExpired);
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn final_admission_rejects_post_quote_intent_purpose_mutation() {
    let admission = BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    ));
    let mut request = submit_request(Decimal::ONE);
    request.intent_kind = BoltV3SubmitIntentKind::RiskReducingExit;
    request.order_side = OrderSide::Sell;
    request.order_quantity = Decimal::new(264, 2);
    request.risk_reducing_exit_proof = Some(valid_risk_reducing_exit_proof());

    let error = admission
        .admit(&request)
        .expect_err("post-quote intent mutation must fail closed");

    assert_eq!(error, BoltV3SubmitAdmissionError::EconomicsOrderMismatch);
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn limited_admission_allows_first_submit_and_rejects_second_before_nt_submit() {
    let admission = limited_admission(1, Decimal::new(1, 0));

    let request = submit_request(Decimal::new(1, 0));
    let mut nt_submit_calls = 0;

    admission
        .admit(&request)
        .expect("first within-cap submit should admit")
        .commit_submitted();
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
fn dropped_uncommitted_permit_rolls_back_live_submit_count_slot() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    let request = submit_request(Decimal::new(1, 0));

    {
        let _permit = admission
            .admit(&request)
            .expect("within-cap submit should reserve a count slot");
        assert_eq!(admission.admitted_order_count(), 1);
    }

    assert_eq!(admission.admitted_order_count(), 0);
    admission
        .admit(&request)
        .expect("dropped permit should release the count slot for retry")
        .commit_submitted();
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn live_submit_approval_limits_bound_provider_submit_before_nt_submit() {
    let admission = BoltV3SubmitAdmissionState::new_with_live_submit_limits(
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count: 1,
                max_order_notional: Decimal::new(25, 0),
            },
        )]),
    );

    let over_approval_notional = admission.admit(&submit_request_for_execution_client(
        "hyperliquid_perps",
        Decimal::new(26, 0),
    ));
    let error = over_approval_notional
        .expect_err("provider approval notional must bound live submit admission");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalCapExceeded
    ));
    assert_eq!(admission.admitted_order_count(), 0);

    admission
        .admit(&submit_request_for_execution_client(
            "hyperliquid_perps",
            Decimal::new(25, 0),
        ))
        .expect("first order within provider approval limits should admit")
        .commit_submitted();

    let exhausted = admission.admit(&submit_request_for_execution_client(
        "hyperliquid_perps",
        Decimal::new(1, 0),
    ));
    let error = exhausted.expect_err("provider approval count must be consumed by admission");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn enabled_but_unfed_loss_governor_rejects_where_unconfigured_governor_admits() {
    let request = submit_request(Decimal::new(1, 0));
    BoltV3SubmitAdmissionState::new(Arc::new(support::RecordingDecisionEvidenceWriter::default()))
        .admit_at(&request, 5_000)
        .expect("without a loss governor, admission is decided by the remaining gates")
        .commit_submitted();

    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.clone(),
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(40, 0)),
        },
    );

    let error = admission
        .admit_at(&request, 5_000)
        .expect_err("enabled but unfed loss governor must fail closed");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::StaleLossSnapshot]
    ));
    assert_eq!(
        writer.admission_decisions()[0].outcome,
        BoltV3AdmissionOutcome::RejectedLossGovernorHalted
    );
    let halts = writer.loss_governor_halts();
    assert_eq!(halts.len(), 1);
    assert!(!halts[0].snapshot_present);
    assert_eq!(
        halts[0].stale_reason,
        BoltV3StaleLossReason::MissingSnapshot
    );
}

#[test]
fn over_notional_cap_rejects_before_nt_submit_without_consuming_count() {
    let admission = limited_admission(1, Decimal::new(1, 0));

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
fn sustained_rejects_over_distinct_keys_bound_the_reject_episode_map() {
    // RCA #885 R1: the reject-episode map is cleared only on an Admitted outcome.
    // Under a sustained-reject regime with rotating high-cardinality instrument ids
    // (and no admit ever firing), the map must NOT grow without bound. Drive
    // `cap + margin` DISTINCT instrument ids, each over the notional cap so the
    // outcome is RejectedNotionalCapExceeded, with NO interleaved admit, and assert
    // the map is held at the shared cap rather than the number of inserted keys.
    //
    // Without the bound this asserts `len() == cap + margin` (every key retained)
    // and fails; with the bound it asserts `len() == cap`.
    let admission = limited_admission(u32::MAX, Decimal::new(1, 0));
    let cap = BoltV3SubmitAdmissionState::reject_episode_capacity();
    let margin = 5usize;
    let inserted = cap + margin;

    for index in 0..inserted {
        let mut request = submit_request(Decimal::new(2, 0));
        // Distinct instrument id per iteration => distinct stable_episode_key
        // (`{instrument_id}/{side}/{outcome}`), so each reject is a new episode.
        request.instrument_id = format!("instrument-{index}");
        let error = admission
            .admit(&request)
            .expect_err("over-cap notional must reject");
        assert!(matches!(
            error,
            BoltV3SubmitAdmissionError::NotionalCapExceeded
        ));
    }

    // No admit ever fired, so the only thing keeping the map finite is eviction.
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(
        admission.reject_episode_count(),
        cap,
        "reject-episode map must be bounded at the shared cap, not the {inserted} inserted keys"
    );
}

#[test]
fn notional_equal_to_cap_is_admitted() {
    let admission = limited_admission(1, Decimal::new(1, 0));

    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("notional equal to cap should admit")
        .commit_submitted();

    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn post_quote_zero_notional_mutation_rejects_before_nt_submit_without_consuming_count() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    let mut request = submit_request(Decimal::ONE);
    request.notional = Decimal::ZERO;

    let result = admission.admit(&request);
    let nt_submit_called = result.is_ok();
    let error = result.expect_err("post-quote zero-notional mutation must reject");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::EconomicsOrderMismatch
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
fn quote_quantity_helper_preserves_equal_calculated_notional() {
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
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(Arc::new(
        support::RecordingDecisionEvidenceWriter::default(),
    )));
    let context = StrategyBuildContext::new(
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        admission.clone(),
        bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        support::fixture_execution_venue(),
    );

    assert!(Arc::ptr_eq(&admission, &context.submit_admission_arc()));
    context
        .submit_admission()
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("shared context admission should allow ungated production submits")
        .commit_submitted();
    assert_eq!(admission.admitted_order_count(), 1);
}

fn submit_request(notional: Decimal) -> BoltV3SubmitAdmissionRequest {
    submit_request_with_kind(notional, BoltV3SubmitIntentKind::Entry)
}

fn submit_request_for_execution_client(
    execution_client_id: &str,
    notional: Decimal,
) -> BoltV3SubmitAdmissionRequest {
    let mut request = submit_request(notional);
    request.execution_client_id = execution_client_id.to_string();
    request
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
    let (order_side, order_quantity, economics_admission) = match intent_kind {
        BoltV3SubmitIntentKind::RiskReducingExit => (
            OrderSide::Sell,
            Decimal::new(264, 2),
            support::sample_risk_reduction_economics_admission(notional),
        ),
        BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit => (
            OrderSide::Buy,
            Decimal::new(1, 0),
            support::sample_economics_admission(notional),
        ),
        BoltV3SubmitIntentKind::KillSwitchForcedReduction => (
            OrderSide::Sell,
            Decimal::new(1, 0),
            support::sample_risk_reduction_economics_admission(notional),
        ),
    };
    BoltV3SubmitAdmissionRequest {
        economics_admission,
        strategy_id: "strategy-a".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        client_order_id: "client-order-1".to_string(),
        instrument_id: "instrument-1".to_string(),
        notional,
        order_side,
        order_quantity,
        intent_kind,
        lifecycle_policy,
        risk_reducing_exit_proof,
        kill_switch_forced_reduction: None,
        admission_evidence: None,
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

fn limited_admission(max_order_count: u32, max_notional: Decimal) -> BoltV3SubmitAdmissionState {
    limited_admission_with_writer(
        Arc::new(support::RecordingDecisionEvidenceWriter::default()),
        max_order_count,
        max_notional,
    )
}

fn limited_admission_with_writer(
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
    max_order_count: u32,
    max_notional: Decimal,
) -> BoltV3SubmitAdmissionState {
    BoltV3SubmitAdmissionState::new_with_live_submit_limits(
        writer,
        BTreeMap::from([(
            "polymarket_main".to_string(),
            BoltV3LiveSubmitApprovalLimits {
                max_order_count,
                max_order_notional: max_notional,
            },
        )]),
    )
}

fn capital_admission_configured_admission_with_writer(
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> BoltV3SubmitAdmissionState {
    BoltV3SubmitAdmissionState::new_with_capital_admission(
        writer,
        BoltV3SubmitCapitalAdmissionConfig {
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: ProductKind::PredictionMarketBinary,
            collateral_currency: "PUSD".to_string(),
            capital_pool: CapitalPoolSnapshot {
                source: "bolt-submit-admission-test".to_string(),
                observed_at_ns: 1_000,
                pool_id: "polymarket-prediction-live".to_string(),
                max_pool_liability: Decimal::new(10, 0),
                committed_liability: Decimal::ZERO,
                max_snapshot_age_ns: 1_000,
            },
            policy: CapitalAdmissionPolicy {
                min_remaining_pool_balance: None,
            },
            dedupe_retention_ns: 1_000,
        },
    )
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

fn venue_truth_halted_kill_switch_state() -> KillSwitchState {
    KillSwitchState::Halted {
        halt_id: "venue-truth-halt".to_string(),
        trigger: KillSwitchHaltTrigger::venue_truth_divergence(
            "polymarket_venue_truth_rest",
            1_200,
            "venue truth divergence: UnexplainedCollateralDelta alarm_class=TrueDivergence",
        ),
    }
}

fn latched_kill_switch_states() -> Vec<KillSwitchState> {
    vec![
        KillSwitchState::Halting {
            halt_id: "halt-1".to_string(),
            trigger: KillSwitchHaltTrigger::loss_governor_breach(
                "loss-governor",
                1_000,
                "daily loss cap breached",
            ),
        },
        halted_kill_switch_state(),
        KillSwitchState::Cancelling {
            halt_id: "halt-1".to_string(),
        },
        KillSwitchState::Flattening {
            halt_id: "halt-1".to_string(),
        },
        KillSwitchState::Flat {
            halt_id: "halt-1".to_string(),
        },
        KillSwitchState::FailedManualIntervention {
            halt_id: "halt-1".to_string(),
            reason: "durable evidence write failed".to_string(),
        },
    ]
}

fn forced_reduction_policy() -> BoltV3KillSwitchForcedReductionPolicy {
    BoltV3KillSwitchForcedReductionPolicy::new(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        1,
        Decimal::new(10, 0),
    )
    .expect("valid forced-reduction policy should construct")
}

fn forced_reduction_claim(halt_id: &str) -> BoltV3KillSwitchForcedReductionClaim {
    BoltV3KillSwitchForcedReductionClaim::new(
        halt_id,
        "flatten-action-1",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .expect("valid forced-reduction claim should construct")
}

fn forced_reduction_request(
    notional: Decimal,
    claim: BoltV3KillSwitchForcedReductionClaim,
) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        kill_switch_forced_reduction: Some(claim),
        ..submit_request_with_kind(notional, BoltV3SubmitIntentKind::KillSwitchForcedReduction)
    }
}

#[test]
fn forced_reduction_policy_and_claim_expose_proof_metadata() {
    let policy = forced_reduction_policy();
    assert_eq!(
        policy.policy_sha256(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(policy.max_live_order_count(), 1);
    assert_eq!(policy.max_notional_per_order(), Decimal::new(10, 0));

    let claim = forced_reduction_claim("halt-1");
    assert_eq!(claim.halt_id(), "halt-1");
    assert_eq!(claim.action_id(), "flatten-action-1");
    assert_eq!(
        claim.policy_sha256(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
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

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
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

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("synthetic entry-skip write failure"))
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("synthetic exit-decision write failure"))
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
        Err(anyhow::anyhow!("synthetic requote-throttle write failure"))
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
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

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
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

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "blocking admission writer received entry-skip evidence"
        ))
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "blocking admission writer received exit-decision evidence"
        ))
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
        Err(anyhow::anyhow!(
            "blocking admission writer received requote-throttle evidence"
        ))
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct OrderRejectFailingDecisionEvidenceWriter {
    admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
    order_reject_attempts: Mutex<Vec<BoltV3OrderRejectEvidence>>,
}

impl OrderRejectFailingDecisionEvidenceWriter {
    fn new() -> Self {
        Self {
            admission_decisions: Mutex::new(Vec::new()),
            order_reject_attempts: Mutex::new(Vec::new()),
        }
    }

    fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
        self.admission_decisions
            .lock()
            .expect("order-reject failing writer mutex should not be poisoned")
            .clone()
    }

    fn order_reject_attempts(&self) -> Vec<BoltV3OrderRejectEvidence> {
        self.order_reject_attempts
            .lock()
            .expect("order-reject failing writer mutex should not be poisoned")
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for OrderRejectFailingDecisionEvidenceWriter {
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
            .expect("order-reject failing writer mutex should not be poisoned")
            .push(decision.clone());
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
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

    fn record_order_reject(&self, evidence: &BoltV3OrderRejectEvidence) -> anyhow::Result<()> {
        self.order_reject_attempts
            .lock()
            .expect("order-reject failing writer mutex should not be poisoned")
            .push(evidence.clone());
        Err(anyhow::anyhow!("synthetic order-reject write failure"))
    }

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_requote_throttle(
        &self,
        _throttle: &BoltV3RequoteThrottleEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Records admission decisions but errors on `record_loss_governor_halt`, so a
/// test can prove the trading-side admission outcome is unaffected when the
/// loss-halt evidence sink fails.
#[derive(Debug)]
struct LossHaltFailingDecisionEvidenceWriter {
    admission_decisions: Mutex<Vec<BoltV3AdmissionDecisionEvidence>>,
    loss_halt_attempts: Mutex<Vec<BoltV3LossGovernorHaltEvidence>>,
}

impl LossHaltFailingDecisionEvidenceWriter {
    fn new() -> Self {
        Self {
            admission_decisions: Mutex::new(Vec::new()),
            loss_halt_attempts: Mutex::new(Vec::new()),
        }
    }

    fn admission_decisions(&self) -> Vec<BoltV3AdmissionDecisionEvidence> {
        self.admission_decisions
            .lock()
            .expect("loss-halt failing writer mutex should not be poisoned")
            .clone()
    }

    fn loss_halt_attempts(&self) -> Vec<BoltV3LossGovernorHaltEvidence> {
        self.loss_halt_attempts
            .lock()
            .expect("loss-halt failing writer mutex should not be poisoned")
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for LossHaltFailingDecisionEvidenceWriter {
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
            .expect("loss-halt failing writer mutex should not be poisoned")
            .push(decision.clone());
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> anyhow::Result<()> {
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

    fn record_exit_evaluation(
        &self,
        _evidence: &BoltV3ExitEvaluationEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(
        &self,
        evidence: &BoltV3LossGovernorHaltEvidence,
    ) -> anyhow::Result<()> {
        self.loss_halt_attempts
            .lock()
            .expect("loss-halt failing writer mutex should not be poisoned")
            .push(evidence.clone());
        Err(anyhow::anyhow!(
            "synthetic loss-governor-halt write failure"
        ))
    }

    fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_requote_throttle(
        &self,
        _throttle: &BoltV3RequoteThrottleEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> anyhow::Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

#[test]
fn loss_halt_evidence_write_failure_does_not_change_admission_outcome() {
    // FIX 3b: a failing loss-governor-halt sink must not change the trading-side
    // admission outcome. Pair a recording writer (control) with a writer that
    // errors only on record_loss_governor_halt and assert identical admission
    // outcome + reasons.
    let policy = LossGovernorPolicy {
        max_snapshot_age_ns: 1_000,
        max_per_trade_loss: Some(Decimal::new(10, 0)),
        max_daily_loss: Some(Decimal::new(25, 0)),
        max_rolling_loss: Some(Decimal::new(30, 0)),
        max_drawdown: Some(Decimal::new(40, 0)),
    };
    let stale_snapshot = || LossSnapshot {
        source: "nt_loss_runtime_feed".to_string(),
        observed_at_ns: 1_000,
        per_trade_pnl: Some(Decimal::ZERO),
        daily_pnl: Some(Decimal::ZERO),
        rolling_pnl: Some(Decimal::ZERO),
        current_equity: Some(Decimal::new(1_000, 0)),
        peak_equity: Some(Decimal::new(1_000, 0)),
        source_observations: LossSourceObservationTimestamps::unobserved(),
    };

    let control_writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let control_admission =
        BoltV3SubmitAdmissionState::new_with_loss_governor(control_writer.clone(), policy.clone());
    control_admission.update_loss_snapshot(stale_snapshot());
    let control_error = control_admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_101)
        .expect_err("stale loss snapshot should reject through the loss governor");

    let failing_writer = Arc::new(LossHaltFailingDecisionEvidenceWriter::new());
    let failing_admission =
        BoltV3SubmitAdmissionState::new_with_loss_governor(failing_writer.clone(), policy);
    failing_admission.update_loss_snapshot(stale_snapshot());
    let failing_error = failing_admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_101)
        .expect_err("a failing loss-halt sink must not change the loss-governor rejection");

    // The trading-side error (outcome + reasons) is identical with and without the sink failure.
    match (&control_error, &failing_error) {
        (
            BoltV3SubmitAdmissionError::LossGovernorHalted {
                reasons: control_reasons,
            },
            BoltV3SubmitAdmissionError::LossGovernorHalted {
                reasons: failing_reasons,
            },
        ) => assert_eq!(control_reasons, failing_reasons),
        other => panic!("expected loss-governor halt on both paths, got {other:?}"),
    }
    assert_eq!(
        failing_writer.admission_decisions()[0].outcome,
        BoltV3AdmissionOutcome::RejectedLossGovernorHalted
    );
    // The sink was reached and did error (proving the swallow path is exercised).
    assert_eq!(failing_writer.loss_halt_attempts().len(), 1);
}

#[test]
fn stale_loss_halt_records_missing_snapshot_reason_with_no_age() {
    // FIX 3c: a None snapshot yields MissingSnapshot, a stable_halt_key prefixed
    // "missing_snapshot:", and no snapshot age.
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let admission = BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.clone(),
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(40, 0)),
        },
    );

    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 5_000)
        .expect_err("missing loss snapshot should reject through the loss governor");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::StaleLossSnapshot]
    ));

    let halts = writer.loss_governor_halts();
    assert_eq!(halts.len(), 1);
    let halt = &halts[0];
    assert_eq!(halt.stale_reason, BoltV3StaleLossReason::MissingSnapshot);
    assert!(
        halt.stable_halt_key.starts_with("missing_snapshot:"),
        "stable_halt_key should be prefixed with the stale reason: {}",
        halt.stable_halt_key
    );
    assert!(!halt.snapshot_present);
    assert_eq!(halt.snapshot_age_ns, None);
    assert_eq!(halt.snapshot_observed_at_ns, None);
}

#[test]
fn stale_loss_halt_records_future_dated_reason_with_no_age() {
    // FIX 3c: a future-dated snapshot (observed_at_ns > now_ns) yields FutureDated
    // and snapshot_age_ns == None (the observed_at_ns > now_ns branch).
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let admission = BoltV3SubmitAdmissionState::new_with_loss_governor(
        writer.clone(),
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(40, 0)),
        },
    );
    admission.update_loss_snapshot(LossSnapshot {
        source: "nt_loss_runtime_feed".to_string(),
        observed_at_ns: 9_000,
        per_trade_pnl: Some(Decimal::ZERO),
        daily_pnl: Some(Decimal::ZERO),
        rolling_pnl: Some(Decimal::ZERO),
        current_equity: Some(Decimal::new(1_000, 0)),
        peak_equity: Some(Decimal::new(1_000, 0)),
        source_observations: LossSourceObservationTimestamps::unobserved(),
    });

    let error = admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 5_000)
        .expect_err("future-dated loss snapshot should reject through the loss governor");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::StaleLossSnapshot]
    ));

    let halts = writer.loss_governor_halts();
    assert_eq!(halts.len(), 1);
    let halt = &halts[0];
    assert_eq!(halt.stale_reason, BoltV3StaleLossReason::FutureDated);
    assert!(
        halt.stable_halt_key.starts_with("future_dated:"),
        "stable_halt_key should be prefixed with the stale reason: {}",
        halt.stable_halt_key
    );
    assert!(halt.snapshot_present);
    assert_eq!(halt.snapshot_observed_at_ns, Some(9_000));
    assert_eq!(halt.snapshot_age_ns, None);
}

#[test]
fn admit_records_admission_decision_evidence_on_admit_outcome() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(1, 0));

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
    assert_eq!(
        decisions[0].execution_client_id,
        request.execution_client_id
    );
    assert_eq!(decisions[0].client_order_id, request.client_order_id);
    assert_eq!(decisions[0].instrument_id, request.instrument_id);
    assert_eq!(decisions[0].notional, request.notional.to_string());
    assert_eq!(decisions[0].intent_kind, request.intent_kind);
}

#[test]
fn single_order_reject_records_order_reject_evidence_on_reject_outcome() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 0, Decimal::new(1, 0));

    let request = submit_request(Decimal::new(1, 0));
    let error = admission
        .admit_at(&request, 1_000)
        .expect_err("zero live-order cap should reject the first submit");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert!(
        writer.records().is_empty(),
        "order-reject evidence must not use the order-intent channel"
    );
    assert_eq!(
        writer.admission_decisions().len(),
        1,
        "existing admission-decision evidence remains independently recorded"
    );
    let rejects = writer.order_rejects();
    assert_eq!(rejects.len(), 1);
    let reject = &rejects[0];
    assert_eq!(reject.reject_source, BoltV3RejectSource::SubmitAdmission);
    assert_eq!(
        reject.reject_reason,
        BoltV3OrderRejectReason::AdmissionRejected
    );
    assert_eq!(
        reject.admission_outcome,
        Some(BoltV3AdmissionOutcome::RejectedCountCapExhausted)
    );
    assert_eq!(reject.raw_reason_text, None);
    assert_eq!(reject.instrument_id, request.instrument_id);
    assert_eq!(reject.order_side.as_deref(), Some("buy"));
    assert_eq!(reject.raw_price, None);
    assert_eq!(reject.raw_quantity.as_deref(), Some("1"));
    assert_eq!(reject.raw_maker_amount, None);
    assert_eq!(reject.raw_taker_amount, None);
    assert_eq!(reject.normalized_price, None);
    assert_eq!(reject.normalized_quantity, None);
    assert_eq!(reject.normalized_maker_amount, None);
    assert_eq!(reject.normalized_taker_amount, None);
    assert_eq!(reject.venue_price_precision, None);
    assert_eq!(reject.venue_size_precision, None);
    assert_eq!(reject.venue_min_notional, None);
    assert_eq!(reject.prior_client_order_id, None);
    assert_eq!(reject.client_order_id, request.client_order_id);
    assert_eq!(reject.retry_count, 1);
    assert_eq!(reject.backoff_cooldown_state, None);
    assert_eq!(
        reject.stable_episode_key,
        "instrument-1/buy/rejected_count_cap_exhausted"
    );
    assert_eq!(reject.elapsed_ns, 0);
}

#[test]
fn loss_governor_halt_is_mece_with_order_reject_evidence() {
    let loss_writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let loss_admission = BoltV3SubmitAdmissionState::new_with_loss_governor(
        loss_writer.clone(),
        LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: Some(Decimal::new(10, 0)),
            max_daily_loss: Some(Decimal::new(25, 0)),
            max_rolling_loss: Some(Decimal::new(30, 0)),
            max_drawdown: Some(Decimal::new(40, 0)),
        },
    );
    loss_admission.update_loss_snapshot(LossSnapshot {
        source: "nt_loss_runtime_feed".to_string(),
        observed_at_ns: 1_000,
        per_trade_pnl: Some(Decimal::ZERO),
        daily_pnl: Some(Decimal::ZERO),
        rolling_pnl: Some(Decimal::ZERO),
        current_equity: Some(Decimal::new(1_000, 0)),
        peak_equity: Some(Decimal::new(1_000, 0)),
        source_observations: LossSourceObservationTimestamps::unobserved(),
    });

    let loss_error = loss_admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 2_101)
        .expect_err("stale loss snapshot should reject through the loss governor");

    assert!(matches!(
        loss_error,
        BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }
            if reasons == vec![LossHaltReason::StaleLossSnapshot]
    ));
    assert_eq!(loss_writer.admission_decisions().len(), 1);
    assert_eq!(
        loss_writer.admission_decisions()[0].outcome,
        BoltV3AdmissionOutcome::RejectedLossGovernorHalted
    );
    assert!(
        loss_writer.order_rejects().is_empty(),
        "loss-governor halts are recorded by loss-halt evidence, not order-reject evidence"
    );

    let count_cap_writer = Arc::new(support::RecordingDecisionEvidenceWriter::new());
    let count_cap_admission =
        limited_admission_with_writer(count_cap_writer.clone(), 0, Decimal::new(1, 0));

    let count_cap_error = count_cap_admission
        .admit_at(&submit_request(Decimal::new(1, 0)), 1_000)
        .expect_err("zero live-order cap should still emit order-reject evidence");

    assert!(matches!(
        count_cap_error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert_eq!(count_cap_writer.admission_decisions().len(), 1);
    let count_cap_rejects = count_cap_writer.order_rejects();
    assert_eq!(count_cap_rejects.len(), 1);
    assert_eq!(
        count_cap_rejects[0].admission_outcome,
        Some(BoltV3AdmissionOutcome::RejectedCountCapExhausted)
    );
}

#[test]
fn single_order_reject_evidence_samples_power_of_two_attempts_and_links_churn() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 0, Decimal::new(1, 0));

    for attempt in 1_u32..=8 {
        let mut request = submit_request(Decimal::new(1, 0));
        request.client_order_id = format!("client-order-{attempt}");

        let error = admission
            .admit_at(&request, 1_000 + u64::from(attempt))
            .expect_err("zero live-order cap should reject every submit attempt");
        assert!(matches!(
            error,
            BoltV3SubmitAdmissionError::CountCapExhausted
        ));
    }

    let rejects = writer.order_rejects();
    let retry_counts: Vec<u32> = rejects.iter().map(|reject| reject.retry_count).collect();
    assert_eq!(retry_counts, vec![1, 2, 4, 8]);
    for reject in rejects {
        let expected_prior = if reject.retry_count == 1 {
            None
        } else {
            Some(format!("client-order-{}", reject.retry_count - 1))
        };
        assert_eq!(
            reject.prior_client_order_id, expected_prior,
            "emitted reject at count {} should point to the immediately preceding attempt",
            reject.retry_count
        );
    }
}

#[test]
fn single_order_reject_episode_resets_after_admitted_submit() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 2, Decimal::new(1, 0));

    for attempt in 1_u32..=3 {
        let mut request = submit_request(Decimal::new(2, 0));
        request.client_order_id = format!("reject-before-accept-{attempt}");

        let error = admission
            .admit_at(&request, 1_000 + u64::from(attempt))
            .expect_err("above-cap notional should reject before the accept reset");
        assert!(matches!(
            error,
            BoltV3SubmitAdmissionError::NotionalCapExceeded
        ));
    }

    let mut accepted = submit_request(Decimal::new(1, 0));
    accepted.client_order_id = "accepted-client-order".to_string();
    admission
        .admit_at(&accepted, 2_000)
        .expect("within-cap submit should reset reject episodes")
        .commit_submitted();

    let mut rejected_after_accept = submit_request(Decimal::new(2, 0));
    rejected_after_accept.client_order_id = "reject-after-accept".to_string();
    let error = admission
        .admit_at(&rejected_after_accept, 3_000)
        .expect_err("above-cap notional should reject after the accept reset");
    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::NotionalCapExceeded
    ));

    let rejects = writer.order_rejects();
    let retry_counts: Vec<u32> = rejects.iter().map(|reject| reject.retry_count).collect();
    assert_eq!(retry_counts, vec![1, 2, 1]);
    let reset_reject = rejects
        .last()
        .expect("reject after accept should restart the episode");
    assert_eq!(reset_reject.retry_count, 1);
    assert_eq!(reset_reject.prior_client_order_id, None);
    assert_eq!(reset_reject.client_order_id, "reject-after-accept");
}

#[test]
fn order_reject_evidence_write_failure_preserves_admission_behavior() {
    let request = submit_request(Decimal::new(1, 0));
    let normal_writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let normal_admission =
        limited_admission_with_writer(normal_writer.clone(), 0, Decimal::new(1, 0));
    let failing_writer = Arc::new(OrderRejectFailingDecisionEvidenceWriter::new());
    let failing_admission =
        limited_admission_with_writer(failing_writer.clone(), 0, Decimal::new(1, 0));

    let normal_error = normal_admission
        .admit_at(&request, 1_000)
        .expect_err("baseline zero live-order cap should reject");
    let failing_error = failing_admission
        .admit_at(&request, 1_000)
        .expect_err("order-reject write failure must not mask admission rejection");

    assert!(matches!(
        normal_error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert!(matches!(
        failing_error,
        BoltV3SubmitAdmissionError::CountCapExhausted
    ));
    assert_eq!(
        normal_writer.admission_decisions(),
        failing_writer.admission_decisions()
    );
    assert_eq!(
        normal_writer.order_rejects(),
        failing_writer.order_reject_attempts()
    );
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
    let source =
        support::module_source_text(bolt_v2::bolt_v3_source_integrity::SUBMIT_ADMISSION_KEY);
    let source = source.as_str();

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
    let admission = limited_admission_with_writer(writer.clone(), 2, Decimal::new(5, 0));

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry at the configured cap should consume the entry slot")
        .commit_submitted();

    admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect("verified risk-reducing exit should admit within provider limits")
        .commit_submitted();
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
    let admission = limited_admission_with_writer(writer.clone(), 2, Decimal::new(5, 0));

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry at the configured cap should admit")
        .commit_submitted();

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
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(5, 0));

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
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(5, 0));

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
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(5, 0));

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
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(5, 0));

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
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(1, 0));

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("first entry should admit")
        .commit_submitted();

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
    let admission = limited_admission_with_writer(writer.clone(), 2, Decimal::new(5, 0));

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry should admit")
        .commit_submitted();
    admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect("first verified risk-reducing exit should admit")
        .commit_submitted();

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
    let admission = limited_admission_with_writer(writer.clone(), 3, Decimal::new(5, 0));

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry should admit")
        .commit_submitted();
    admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect("risk-reducing exit should admit")
        .commit_submitted();

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::ReplaceSubmit,
        ))
        .expect("replace-submit must use the independent replace slot")
        .commit_submitted();

    assert_eq!(admission.admitted_order_count(), 3);
}

#[test]
fn configured_capital_admission_rejects_replace_submit_before_admission() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = capital_admission_configured_admission_with_writer(writer.clone());

    let error = admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::ReplaceSubmit,
        ))
        .expect_err("replace-submit must enter the capital-admission reject path");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
            reason: BoltV3CapitalAdmissionRejectReason::ReplaceSubmitUnsupported
        }
    ));
    assert_eq!(admission.admitted_order_count(), 0);
    let decisions = writer.admission_decisions();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        decisions[0].outcome,
        BoltV3AdmissionOutcome::RejectedCapitalAdmission
    );
    assert_eq!(
        decisions[0].intent_kind,
        BoltV3SubmitIntentKind::ReplaceSubmit
    );
}

#[test]
fn replace_submit_rejects_when_lifecycle_policy_disables_replace() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(1, 0));

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
fn armed_kill_switch_preserves_existing_entry_admission_behavior() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    admission.replace_kill_switch_state(KillSwitchState::Armed);

    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 0),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("armed kill switch must preserve normal entry admission")
        .commit_submitted();

    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn latched_kill_switch_states_block_entry_and_replace_before_nt_submit_without_consuming_count() {
    for state in latched_kill_switch_states() {
        let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
        let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(1, 0));
        admission.replace_kill_switch_state(state);

        for intent_kind in [
            BoltV3SubmitIntentKind::Entry,
            BoltV3SubmitIntentKind::ReplaceSubmit,
        ] {
            let error = admission
                .admit(&submit_request_with_kind(Decimal::new(1, 1), intent_kind))
                .expect_err("latched kill switch must reject exposure-opening risk");

            assert!(matches!(
                error,
                BoltV3SubmitAdmissionError::KillSwitchLatched { .. }
            ));
        }
        assert_eq!(admission.admitted_order_count(), 0);
        let decisions = writer.admission_decisions();
        assert_eq!(decisions.len(), 2);
        assert!(
            decisions
                .iter()
                .all(|decision| decision.outcome
                    == BoltV3AdmissionOutcome::RejectedKillSwitchLatched)
        );
    }
}

#[test]
fn latched_kill_switch_blocks_replace_submit_even_when_lifecycle_policy_allows_replace() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    admission.replace_kill_switch_state(halted_kill_switch_state());

    let error = admission
        .admit(&submit_request_with_kind_and_policy(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::ReplaceSubmit,
            BoltV3SubmitLifecyclePolicy::new(true),
        ))
        .expect_err("latched kill switch must block replace-submit risk");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::KillSwitchLatched { .. }
    ));
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn latched_kill_switch_blocks_risk_reducing_exit_before_normal_admission() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 2, Decimal::new(5, 0));
    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should be admitted before latch")
        .commit_submitted();
    admission.replace_kill_switch_state(halted_kill_switch_state());

    let exit = admission
        .admit(&submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ))
        .expect_err("latched kill switch must block risk-reducing exit");

    assert!(matches!(
        exit,
        BoltV3SubmitAdmissionError::KillSwitchLatched { .. }
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
            BoltV3AdmissionOutcome::RejectedKillSwitchLatched,
        ]
    );
    assert_eq!(admission.admitted_order_count(), 1);
}

#[test]
fn venue_truth_latch_blocks_all_normal_submit_classes() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 10, Decimal::new(10, 0));
    admission.replace_kill_switch_state(venue_truth_halted_kill_switch_state());

    for request in [
        submit_request_with_kind(Decimal::new(1, 1), BoltV3SubmitIntentKind::Entry),
        submit_request_with_kind(Decimal::new(1, 1), BoltV3SubmitIntentKind::ReplaceSubmit),
        submit_request_with_kind_and_exit_proof(
            Decimal::new(264, 2),
            BoltV3SubmitIntentKind::RiskReducingExit,
            Some(valid_risk_reducing_exit_proof()),
        ),
    ] {
        let error = admission
            .admit(&request)
            .expect_err("venue truth latch must block normal submit class");

        assert!(matches!(
            error,
            BoltV3SubmitAdmissionError::KillSwitchLatched { .. }
        ));
    }
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(
        writer
            .admission_decisions()
            .iter()
            .all(|decision| decision.outcome == BoltV3AdmissionOutcome::RejectedKillSwitchLatched)
    );
}

#[test]
fn forced_reduction_requires_halt_action_and_policy_proof_before_cap_bypass() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    admission.replace_kill_switch_state(halted_kill_switch_state());

    for request in [
        submit_request_with_kind(
            Decimal::new(10, 0),
            BoltV3SubmitIntentKind::KillSwitchForcedReduction,
        ),
        forced_reduction_request(Decimal::new(10, 0), forced_reduction_claim("other-halt")),
    ] {
        let error = admission
            .admit(&request)
            .expect_err("forced reduction without matching proof must fail closed");
        assert!(matches!(
            error,
            BoltV3SubmitAdmissionError::KillSwitchForcedReductionProofInvalid
        ));
    }
    assert_eq!(admission.admitted_order_count(), 0);
}

#[test]
fn forced_reduction_is_only_admissible_while_kill_switch_is_latched() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    admission.configure_kill_switch_forced_reduction_policy(forced_reduction_policy());

    let error = admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect_err("forced reduction must not run while kill switch is armed");

    assert!(matches!(
        error,
        BoltV3SubmitAdmissionError::KillSwitchForcedReductionProofInvalid
    ));
}

#[test]
fn valid_forced_reduction_while_latched_bypasses_normal_count_and_notional_caps() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(1, 0));
    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should consume the only normal count slot")
        .commit_submitted();
    admission.replace_kill_switch_state(halted_kill_switch_state());
    admission.configure_kill_switch_forced_reduction_policy(forced_reduction_policy());

    admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect("valid forced reduction should bypass normal count and notional caps")
        .commit_submitted();

    let decisions = writer.admission_decisions();
    assert_eq!(
        decisions.last().map(|decision| decision.intent_kind),
        Some(BoltV3SubmitIntentKind::KillSwitchForcedReduction)
    );
    assert_eq!(
        decisions.last().map(|decision| decision.outcome.clone()),
        Some(BoltV3AdmissionOutcome::Admitted)
    );
    assert_eq!(admission.admitted_order_count(), 2);
}

#[test]
fn valid_forced_reduction_while_flattening_uses_matching_halt_policy_proof() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = limited_admission_with_writer(writer.clone(), 1, Decimal::new(1, 0));
    admission
        .admit(&submit_request_with_kind(
            Decimal::new(1, 1),
            BoltV3SubmitIntentKind::Entry,
        ))
        .expect("entry submit should consume the only normal count slot")
        .commit_submitted();
    admission.replace_kill_switch_state(KillSwitchState::Flattening {
        halt_id: "halt-1".to_string(),
    });
    admission.configure_kill_switch_forced_reduction_policy(forced_reduction_policy());

    admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect("valid flattening forced reduction should bypass normal count and notional caps")
        .commit_submitted();

    let decisions = writer.admission_decisions();
    assert_eq!(
        decisions.last().map(|decision| decision.intent_kind),
        Some(BoltV3SubmitIntentKind::KillSwitchForcedReduction)
    );
    assert_eq!(
        decisions.last().map(|decision| decision.outcome.clone()),
        Some(BoltV3AdmissionOutcome::Admitted)
    );
    assert_eq!(admission.admitted_order_count(), 2);
}

#[test]
fn forced_reduction_live_count_releases_terminal_order_before_next_admission() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    admission.replace_kill_switch_state(halted_kill_switch_state());
    admission.configure_kill_switch_forced_reduction_policy(forced_reduction_policy());

    admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect("first live forced reduction should be admitted")
        .commit_submitted();

    let capped = admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect_err("second live forced reduction should hit live cap");
    assert!(matches!(
        capped,
        BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded
    ));

    assert!(
        admission.record_kill_switch_forced_reduction_terminal("client-order-1"),
        "terminal release should consume the tracked forced-reduction client order id"
    );

    admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect("terminal forced reduction should release the live cap")
        .commit_submitted();
}

#[test]
fn dropped_uncommitted_forced_reduction_permit_rolls_back_live_cap() {
    let admission = limited_admission(1, Decimal::new(1, 0));
    admission.replace_kill_switch_state(halted_kill_switch_state());
    admission.configure_kill_switch_forced_reduction_policy(forced_reduction_policy());

    {
        let _permit = admission
            .admit(&forced_reduction_request(
                Decimal::new(10, 0),
                forced_reduction_claim("halt-1"),
            ))
            .expect("valid forced reduction should reserve the live cap");
        let capped = admission
            .admit(&forced_reduction_request(
                Decimal::new(10, 0),
                forced_reduction_claim("halt-1"),
            ))
            .expect_err("uncommitted forced reduction should hold the live cap");
        assert!(matches!(
            capped,
            BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded
        ));
    }

    admission
        .admit(&forced_reduction_request(
            Decimal::new(10, 0),
            forced_reduction_claim("halt-1"),
        ))
        .expect("dropped forced-reduction permit should release the live cap")
        .commit_submitted();
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
    let admission = limited_admission_with_writer(writer.clone(), 2, Decimal::new(1, 0));

    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("first valid submit should admit")
        .commit_submitted();
    admission
        .admit(&submit_request(Decimal::ZERO))
        .expect_err("zero notional must reject");
    admission
        .admit(&submit_request(Decimal::new(2, 0)))
        .expect_err("over-cap notional must reject");
    admission
        .admit(&submit_request(Decimal::new(1, 0)))
        .expect("first within-cap submit should admit")
        .commit_submitted();
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
            BoltV3AdmissionOutcome::Admitted,
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
    let admission = limited_admission_with_writer(
        Arc::new(FailingDecisionEvidenceWriter),
        1,
        Decimal::new(1, 0),
    );

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
    let admission = Arc::new(limited_admission_with_writer(
        writer.clone(),
        1,
        Decimal::new(1, 0),
    ));

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
        .expect("first admission should pass")
        .commit_submitted();
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
