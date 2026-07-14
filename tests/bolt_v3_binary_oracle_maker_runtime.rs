use crate::support;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::ReferencePriceProvider,
    bolt_v3_decision_evidence::{
        BoltV3AdmissionOutcome, BoltV3DecisionEvidenceWriter, BoltV3RequoteActionCostClass,
        BoltV3RequoteThrottleBlockReason, BoltV3RequoteThrottleBound,
        BoltV3RequoteThrottleEvidence,
    },
    bolt_v3_loss_governor::{LossAdmissionDecision, LossHaltReason, LossSnapshotDiagnostics},
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_mu_estimator::{MuEstimatorConfig, MuHealthConfig, UsableMu},
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_order_dispatch::MakerOrderDispatchOutcome,
    bolt_v3_maker_order_plan::{MakerLegBinding, MakerMarketActionOrderInput},
    bolt_v3_maker_quote_plan::{MakerQuotePlanInputs, plan_maker_quote_targets},
    bolt_v3_maker_rate_budget::build_requote_budget_pair,
    bolt_v3_maker_risk::{MakerLossRiskPolicy, MakerRiskBlockReason, MakerRiskMode},
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteInput, MakerRuntimeQuoteSetInput,
        MakerRuntimeReferenceFairValueBlockReason, MakerRuntimeReferenceFairValueInput,
        plan_maker_runtime_quote,
    },
    bolt_v3_market_families::{FairProbabilityInputs, static_binary_event, updown},
    bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LifecycleAction, MarketAction, MarketQuote, MarketState,
    },
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
        RealizedVolSnapshot,
    },
    bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote},
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionState, BoltV3SubmitLifecyclePolicy},
    bolt_v3_timestamp_domain::LocalReceiveMs,
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::{
        binary_oracle_maker::{
            BinaryOracleMaker, BinaryOracleMakerConfig, BinaryOracleMakerMarketActionRouteInput,
            BinaryOracleMakerRiskRouteInput, BinaryOracleMakerRuntimeQuoteRouteInput,
            BinaryOracleMakerRuntimeReferenceQuoteBlockReason,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput, mu::MakerMuState,
        },
        registry::{FeeProvider, StrategyBuildContext},
    },
};
use futures_util::{FutureExt, future::BoxFuture};
use nautilus_common::{
    cache::Cache,
    clock::{Clock, TestClock},
    timer::{TimeEvent, TimeEventCallback},
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::{AggressorSide, OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, TradeId, TraderId, Venue},
    types::{Price, Quantity},
};
use nautilus_portfolio::portfolio::Portfolio;
use nautilus_trading::Strategy;
use rust_decimal::Decimal;
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

const TEST_REFERENCE_ASSET: &str = "reference_asset";
const TEST_REALIZED_VOL_SURFACE_ID: &str = "maker_reference_surface";
const TEST_REALIZED_VOL_SOURCE_ID: &str = "maker_reference_rv";

fn ready_realized_vol_snapshot(as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
    RealizedVolSnapshot {
        surface_id: TEST_REALIZED_VOL_SURFACE_ID.to_string(),
        as_of_ms,
        latest_accepted_receive_ms: Some(bolt_v2::bolt_v3_timestamp_domain::LocalReceiveMs::new(
            as_of_ms,
        )),
        annualized_realized_vol_decimal: Some(realized_vol),
        measured_annualized_realized_vol_decimal: Some(realized_vol),
        noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
        continuous_annualized_realized_vol_decimal: Some(realized_vol),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready: true,
        sources_used: vec![TEST_REALIZED_VOL_SOURCE_ID.to_string()],
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::<RealizedVolBlockReason>::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: String::new(),
    }
}

#[test]
fn maker_runtime_submit_routes_through_shared_context_in_shadow() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let command = MakerCompiledOrderCommand::Submit {
        leg: Leg::Yes,
        template: Box::new(maker_limit_post_only_template()),
        inputs: NtOrderBuildInputs {
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: OrderSide::Buy,
            quantity: Quantity::new(2.0, 2),
            price: Some(Price::new(0.40, 2)),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
        },
        fallback_price: Price::new(0.40, 2),
    };

    let outcome = maker
        .route_maker_order_command(
            &command,
            "maker_submit",
            Decimal::ZERO,
            BoltV3SubmitLifecyclePolicy::new(true),
        )
        .expect("maker submit should route through shared execution context");

    assert_eq!(
        outcome,
        MakerOrderDispatchOutcome::Submitted {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
            price: Price::new(0.40, 2),
            quantity: Quantity::new(2.0, 2),
        }
    );
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(writer.records().len(), 1);
    assert_eq!(writer.records()[0].strategy_id, "maker-strategy");
    assert_eq!(writer.admission_decisions().len(), 1);
    assert_eq!(
        writer.admission_decisions()[0].outcome,
        BoltV3AdmissionOutcome::Admitted
    );
}

#[test]
fn maker_runtime_quote_tick_routes_both_legs_through_shared_context_in_shadow() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let outcome = maker
        .route_maker_runtime_quote(
            &mut market,
            &mut budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote: MakerRuntimeQuoteInput {
                    quote_plan: quote_plan_inputs(static_binary_event::KEY),
                    quote_set: quote_set_inputs(),
                    order_plan: order_plan_inputs(),
                },
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
                max_fee_bps: Decimal::ZERO,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            },
        )
        .expect("maker quote tick should route through shared execution context");

    let quote_plan = outcome
        .quote
        .quote_plan
        .as_ref()
        .expect("maker quote tick should produce quote targets");
    assert_eq!(quote_plan.fair_probability_up, 0.60);
    let orders = outcome
        .orders
        .expect("maker quote tick should dispatch both leg order commands");
    assert_eq!(
        orders.yes.dispatch,
        Some(MakerOrderDispatchOutcome::Submitted {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
            price: Price::new(quote_plan.targets.leg_a.price, 2),
            quantity: Quantity::new(2.0, 2),
        })
    );
    assert_eq!(
        orders.no.dispatch,
        Some(MakerOrderDispatchOutcome::Submitted {
            leg: Leg::No,
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-NO-1"),
            price: Price::new(quote_plan.targets.leg_b.price, 2),
            quantity: Quantity::new(3.0, 2),
        })
    );
    assert_eq!(market.market_state(), MarketState::Quoting);
    assert_eq!(budget.submit_commands_in_window(), 2);
    assert_eq!(budget.rest_cost_in_window(), 2);
    assert_eq!(admission.admitted_order_count(), 0);

    let records = writer.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].strategy_id, "maker-strategy");
    assert_eq!(records[0].instrument_id, "YES.RUNTIME");
    assert_eq!(records[1].strategy_id, "maker-strategy");
    assert_eq!(records[1].instrument_id, "NO.RUNTIME");
    assert_eq!(writer.admission_decisions().len(), 2);
}

#[test]
fn maker_runtime_quote_records_requote_throttle_once_per_blocked_leg_edge() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let route_input = || BinaryOracleMakerRuntimeQuoteRouteInput {
        quote: MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
        submit_template: &submit_template,
        price_precision: 2,
        quantity_precision: 2,
        submit_order_prefix: "maker_submit",
        max_fee_bps: Decimal::ZERO,
        submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
    };

    maker
        .route_maker_runtime_quote(&mut market, &mut budget, route_input())
        .expect("first quote cycle should route the granted leg and record the denied leg");
    maker
        .route_maker_runtime_quote(&mut market, &mut budget, route_input())
        .expect("repeated blocked quote cycle should be deduped");

    let throttles = writer.requote_throttles();
    assert_eq!(
        throttles.len(),
        1,
        "same blocked leg state must emit one throttle evidence record"
    );
    let throttle = &throttles[0];
    assert_eq!(throttle.strategy_id, "maker-strategy");
    assert_eq!(throttle.family_key, static_binary_event::KEY);
    assert_eq!(throttle.leg, "no");
    assert_eq!(
        throttle.action_cost_class,
        BoltV3RequoteActionCostClass::FreshSubmit
    );
    assert_eq!(
        throttle.block_reason,
        BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted
    );
    assert_eq!(
        throttle.bound_by,
        BoltV3RequoteThrottleBound::SubmitCommandWindow
    );
    assert_eq!(throttle.submit_commands_in_window, 1);
    assert_eq!(throttle.submit_command_cap, 1);
    assert_eq!(throttle.rest_cost_in_window, 1);
    assert_eq!(throttle.rest_cap_per_minute, 100);
}

#[test]
fn maker_runtime_quote_surfaces_requote_throttle_write_failure_at_error_and_proceeds() {
    let logger = install_capturing_logger();
    let _observer_guard = CAPTURING_LOGGER_OBSERVERS
        .lock()
        .expect("capturing logger observer mutex poisoned");
    logger.reset();

    let failure_message = "injected maker requote-throttle evidence write failure";
    let writer = Arc::new(FailingRequoteThrottleDecisionEvidenceWriter::new(
        failure_message,
    ));
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context_with_writer(writer.clone(), admission),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let outcome = maker.route_maker_runtime_quote(
        &mut market,
        &mut budget,
        BinaryOracleMakerRuntimeQuoteRouteInput {
            quote: MakerRuntimeQuoteInput {
                quote_plan: quote_plan_inputs(static_binary_event::KEY),
                quote_set: quote_set_inputs(),
                order_plan: order_plan_inputs(),
            },
            submit_template: &submit_template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
            max_fee_bps: Decimal::ZERO,
            submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        },
    );

    assert!(
        outcome.is_ok(),
        "requote-throttle evidence write failure must not propagate"
    );
    let throttles = writer.requote_throttles();
    assert_eq!(
        throttles.len(),
        1,
        "the failing writer must be called exactly once for the blocked leg"
    );
    assert_eq!(
        throttles[0].block_reason,
        BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted
    );

    let matching: Vec<(log::Level, String)> = logger
        .records()
        .into_iter()
        .filter(|(_, message)| {
            message.contains("binary_oracle_maker requote throttle evidence write failed")
                && message.contains(failure_message)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "the requote-throttle evidence write failure must be surfaced exactly once, not swallowed; got {matching:?}"
    );
    assert_eq!(
        matching[0].0,
        log::Level::Error,
        "the requote-throttle evidence write failure must be surfaced at error! severity, not warn!"
    );
}

#[test]
fn maker_runtime_reference_quote_route_uses_shared_fair_value_inputs_and_blocks_before_quote() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let quotes = vec![
        reference_quote(TEST_REFERENCE_ASSET, "primary", 99.0, 1_000),
        reference_quote(TEST_REFERENCE_ASSET, "backup", 100.05, 1_490),
    ];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(1_400, 1.5);
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let fair_input = MakerRuntimeReferenceFairValueInput {
        family_key: updown::KEY,
        interval_start_ms: 1_000,
        interval_end_ms: 2_000,
        reference_quotes: &quotes,
        strike_price: Some(100.0),
        seconds_to_market_end: Some(300),
        realized_volatility_snapshot: &realized_volatility_snapshot,
        realized_volatility_max_source_age_ms: None,
        pricing_kurtosis: 0.25,
        evaluation_receive_ms: LocalReceiveMs::new(1_500),
    };
    let quote_set_at_reference_evaluation = || {
        let mut quote_set = quote_set_inputs();
        quote_set.now_ms = 1_500;
        quote_set
    };
    let expected_fair_probability_up = updown::fair_probability_up(&FairProbabilityInputs {
        spot_price: 100.05,
        strike_price: fair_input.strike_price.expect("fixture strike"),
        seconds_to_market_end: fair_input.seconds_to_market_end.expect("fixture expiry"),
        realized_vol: 1.5,
        pricing_kurtosis: fair_input.pricing_kurtosis,
    })
    .expect("updown fixture should price")
    .value();
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let outcome = maker
        .route_maker_runtime_reference_quote(
            &mut market,
            &mut budget,
            &mut selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: fair_input,
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set: quote_set_at_reference_evaluation(),
                order_plan: order_plan_inputs(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
                max_fee_bps: Decimal::ZERO,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            },
        )
        .expect("maker reference quote tick should route through shared context");

    assert_eq!(outcome.blocked_by, None);
    assert_eq!(outcome.fair_value.blocked_by, None);
    let fair = outcome
        .fair_value
        .fair_value
        .as_ref()
        .expect("fresh backup reference current price should price");
    assert_eq!(fair.spot_price, 100.05);
    assert_eq!(
        fair.strike_price,
        fair_input.strike_price.expect("fixture strike")
    );
    assert_eq!(
        fair.seconds_to_market_end,
        fair_input.seconds_to_market_end.expect("fixture expiry")
    );
    assert_eq!(fair.realized_vol, 1.5);
    assert_eq!(fair.pricing_kurtosis, fair_input.pricing_kurtosis);
    assert_eq!(fair.reference_current_price, 100.05);
    assert_eq!(fair.source_id, "backup");
    assert_eq!(fair.reference_current_price_source_id, "backup");
    assert_eq!(fair.reference_current_price_observed_ts_ms, 1_490);
    assert!(fair.failed_over);
    assert!(fair.reference_current_price_failed_over);
    assert_eq!(
        fair.realized_vol_surface_id.as_deref(),
        Some(TEST_REALIZED_VOL_SURFACE_ID)
    );
    assert_eq!(
        fair.realized_vol_source_venue.as_deref(),
        Some(TEST_REALIZED_VOL_SOURCE_ID)
    );
    assert_eq!(fair.realized_vol_source_ts_ms, Some(1_400));
    assert_eq!(fair.fair_probability_up, expected_fair_probability_up);
    let quote_plan = outcome
        .quote
        .as_ref()
        .and_then(|decision| decision.quote_plan.as_ref())
        .expect("reference fair value should feed a maker quote plan");
    assert_eq!(quote_plan.fair_probability_up, expected_fair_probability_up);
    assert!(
        outcome.orders.is_some(),
        "reference-priced quote should dispatch maker orders"
    );
    assert_eq!(market.market_state(), MarketState::Quoting);
    assert_eq!(writer.records().len(), 2);

    let blocked_writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let blocked_admission = Arc::new(BoltV3SubmitAdmissionState::new(blocked_writer.clone()));
    let mut blocked_maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(blocked_writer.clone(), blocked_admission),
    );
    register_maker_for_order_factory(&mut blocked_maker);
    let mut blocked_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let mut blocked_market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut blocked_budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let blocked = blocked_maker
        .route_maker_runtime_reference_quote(
            &mut blocked_market,
            &mut blocked_budget,
            &mut blocked_selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: MakerRuntimeReferenceFairValueInput {
                    reference_quotes: &[],
                    ..fair_input
                },
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set: quote_set_at_reference_evaluation(),
                order_plan: order_plan_inputs(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
                max_fee_bps: Decimal::ZERO,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            },
        )
        .expect("maker reference quote blocker should be a route outcome");

    assert_eq!(blocked.fair_value.fair_value, None);
    assert_eq!(
        blocked.fair_value.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::ReferenceCurrentPriceUnavailable)
    );
    assert_eq!(
        blocked.blocked_by,
        Some(
            BinaryOracleMakerRuntimeReferenceQuoteBlockReason::FairValue(
                MakerRuntimeReferenceFairValueBlockReason::ReferenceCurrentPriceUnavailable
            )
        )
    );
    assert_eq!(blocked.quote, None);
    assert_eq!(blocked.orders, None);
    assert_eq!(blocked_market.market_state(), MarketState::Idle);
    assert_eq!(blocked_budget.submit_commands_in_window(), 0);
    assert_eq!(blocked_budget.rest_cost_in_window(), 0);
    assert_eq!(blocked_writer.records().len(), 0);

    for (reference_fair_value, expected_blocker) in [
        (
            MakerRuntimeReferenceFairValueInput {
                strike_price: None,
                ..fair_input
            },
            MakerRuntimeReferenceFairValueBlockReason::StrikePriceMissing,
        ),
        (
            MakerRuntimeReferenceFairValueInput {
                seconds_to_market_end: None,
                ..fair_input
            },
            MakerRuntimeReferenceFairValueBlockReason::SecondsToExpiryMissing,
        ),
    ] {
        let missing_input_writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
        let missing_input_admission = Arc::new(BoltV3SubmitAdmissionState::new(
            missing_input_writer.clone(),
        ));
        let mut missing_input_maker = BinaryOracleMaker::new(
            maker_config(),
            maker_context(missing_input_writer.clone(), missing_input_admission),
        );
        register_maker_for_order_factory(&mut missing_input_maker);
        let mut missing_input_selector = ReferencePriceSelector::new(
            TEST_REFERENCE_ASSET,
            vec!["primary".to_string(), "backup".to_string()],
            1,
            100,
            25,
        )
        .expect("selector fixture should be valid");
        let mut missing_input_market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
        let mut missing_input_budget = build_requote_budget_pair("40/00:01:00", 100, 500)
            .expect("well-formed rate config builds a budget");

        let missing_input = missing_input_maker
            .route_maker_runtime_reference_quote(
                &mut missing_input_market,
                &mut missing_input_budget,
                &mut missing_input_selector,
                BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                    reference_fair_value,
                    quote_plan: quote_plan_inputs(reference_fair_value.family_key),
                    quote_set: quote_set_at_reference_evaluation(),
                    order_plan: order_plan_inputs(),
                    submit_template: &maker_limit_post_only_template(),
                    price_precision: 2,
                    quantity_precision: 2,
                    submit_order_prefix: "maker_submit",
                    max_fee_bps: Decimal::ZERO,
                    submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
                },
            )
            .expect("maker reference quote shared fair-value blocker should be a route outcome");

        assert_eq!(missing_input.fair_value.fair_value, None);
        assert_eq!(missing_input.fair_value.blocked_by, Some(expected_blocker));
        assert_eq!(
            missing_input.blocked_by,
            Some(BinaryOracleMakerRuntimeReferenceQuoteBlockReason::FairValue(expected_blocker))
        );
        assert_eq!(missing_input.quote, None);
        assert_eq!(missing_input.orders, None);
        assert_eq!(missing_input_market.market_state(), MarketState::Idle);
        assert_eq!(missing_input_budget.submit_commands_in_window(), 0);
        assert_eq!(missing_input_budget.rest_cost_in_window(), 0);
        assert_eq!(missing_input_writer.records().len(), 0);
    }

    let unsupported_writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let unsupported_admission =
        Arc::new(BoltV3SubmitAdmissionState::new(unsupported_writer.clone()));
    let mut unsupported_maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(unsupported_writer.clone(), unsupported_admission),
    );
    register_maker_for_order_factory(&mut unsupported_maker);
    let mut unsupported_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let mut unsupported_market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut unsupported_budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let unsupported = unsupported_maker
        .route_maker_runtime_reference_quote(
            &mut unsupported_market,
            &mut unsupported_budget,
            &mut unsupported_selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: MakerRuntimeReferenceFairValueInput {
                    family_key: "missing_family",
                    ..fair_input
                },
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set: quote_set_at_reference_evaluation(),
                order_plan: order_plan_inputs(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
                max_fee_bps: Decimal::ZERO,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            },
        )
        .expect("maker reference quote fair-value blocker should be a route outcome");

    assert_eq!(unsupported.fair_value.fair_value, None);
    assert_eq!(
        unsupported.fair_value.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::FairProbabilityUnavailable)
    );
    assert_eq!(
        unsupported.blocked_by,
        Some(
            BinaryOracleMakerRuntimeReferenceQuoteBlockReason::FairValue(
                MakerRuntimeReferenceFairValueBlockReason::FairProbabilityUnavailable
            )
        )
    );
    assert_eq!(unsupported.quote, None);
    assert_eq!(unsupported.orders, None);
    assert_eq!(unsupported_market.market_state(), MarketState::Idle);
    assert_eq!(unsupported_budget.submit_commands_in_window(), 0);
    assert_eq!(unsupported_budget.rest_cost_in_window(), 0);
    assert_eq!(unsupported_writer.records().len(), 0);
}

#[test]
fn maker_canceled_confirmation_routes_prepaid_replacement_submit_in_shadow() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    assert_eq!(
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false
            }
        ),
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);

    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let mut quote_set = quote_set_inputs();
    quote_set.yes_resting_price = Some(0.40);
    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set,
            order_plan: MakerRuntimeOrderPlanInput {
                yes: MakerLegBinding {
                    instrument_id: InstrumentId::from("YES.RUNTIME"),
                    active_order: Some(order_identity("MAKER-YES-1", 1)),
                    next_order: Some(order_identity("MAKER-YES-2", 2)),
                },
                no: MakerLegBinding {
                    instrument_id: InstrumentId::from("NO.RUNTIME"),
                    active_order: None,
                    next_order: Some(order_identity("MAKER-NO-1", 1)),
                },
            },
        },
    );
    let targets = decision
        .quote_plan
        .as_ref()
        .expect("requote should have quote targets")
        .targets;
    let submit_commands_before_cancel_confirm = budget.submit_commands_in_window();
    let rest_cost_before_cancel_confirm = budget.rest_cost_in_window();
    let action = market
        .on_leg_event(Leg::Yes, LegEvent::Canceled)
        .expect("cancel confirmation should emit pre-paid replacement submit");

    assert_eq!(
        budget.submit_commands_in_window(),
        submit_commands_before_cancel_confirm
    );
    assert_eq!(
        budget.rest_cost_in_window(),
        rest_cost_before_cancel_confirm
    );

    let outcome = maker
        .route_maker_market_action(BinaryOracleMakerMarketActionRouteInput {
            action: MakerMarketActionOrderInput {
                action,
                targets,
                yes_quantity: quote_set.yes_quantity,
                no_quantity: quote_set.no_quantity,
                yes: MakerLegBinding {
                    instrument_id: InstrumentId::from("YES.RUNTIME"),
                    active_order: None,
                    next_order: Some(order_identity("MAKER-YES-2", 2)),
                },
                no: MakerLegBinding {
                    instrument_id: InstrumentId::from("NO.RUNTIME"),
                    active_order: None,
                    next_order: Some(order_identity("MAKER-NO-1", 1)),
                },
            },
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
            max_fee_bps: Decimal::ZERO,
            submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        })
        .expect("maker should route pre-paid replacement submit through shared context");

    assert_eq!(
        outcome.order.dispatch,
        Some(MakerOrderDispatchOutcome::Submitted {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-YES-2"),
            price: Price::new(targets.leg_a.price, 2),
            quantity: Quantity::new(quote_set.yes_quantity, 2),
        })
    );
    assert_eq!(
        budget.submit_commands_in_window(),
        submit_commands_before_cancel_confirm
    );
    assert_eq!(
        budget.rest_cost_in_window(),
        rest_cost_before_cancel_confirm
    );
    assert_eq!(admission.admitted_order_count(), 0);

    let records = writer.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].strategy_id, "maker-strategy");
    assert_eq!(records[0].instrument_id, "YES.RUNTIME");
    assert_eq!(records[0].client_order_id, "MAKER-YES-2");
    assert_eq!(writer.admission_decisions().len(), 1);
}

#[test]
fn maker_loss_risk_route_soft_holds_without_order_mutation() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = resting_market_quote();
    let loss_decision = accepted_loss_decision();
    let submit_template = maker_limit_post_only_template();

    let outcome = maker
        .route_maker_loss_risk(
            &mut market,
            risk_route_input(
                &loss_decision,
                MakerLossRiskPolicy {
                    on_loss_breach: MakerRiskMode::HardFlat,
                    on_untrusted_snapshot: MakerRiskMode::CancelOnly,
                },
                &submit_template,
            ),
        )
        .expect("accepted loss decision should route through risk shell");

    assert_eq!(outcome.risk.mode, MakerRiskMode::SoftHold);
    assert_eq!(outcome.risk.action, None);
    assert_eq!(outcome.risk.blocked_by, None);
    assert_eq!(outcome.orders, None);
    assert_eq!(market.market_state(), MarketState::Quoting);
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(writer.records().len(), 0);
    assert_eq!(writer.admission_decisions().len(), 0);
}

#[test]
fn maker_loss_risk_route_drains_quotes_for_untrusted_loss_snapshot() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = resting_market_quote();
    let loss_decision = rejected_loss_decision(LossHaltReason::StaleLossSnapshot);
    let submit_template = maker_limit_post_only_template();

    let outcome = maker
        .route_maker_loss_risk(
            &mut market,
            risk_route_input(
                &loss_decision,
                MakerLossRiskPolicy {
                    on_loss_breach: MakerRiskMode::HardFlat,
                    on_untrusted_snapshot: MakerRiskMode::CancelOnly,
                },
                &submit_template,
            ),
        )
        .expect("untrusted loss snapshot should drain via shared maker order route");

    assert_eq!(outcome.risk.mode, MakerRiskMode::CancelOnly);
    assert_eq!(outcome.risk.action, Some(MarketAction::CancelAllBothLegs));
    assert_eq!(outcome.risk.blocked_by, None);
    let orders = outcome
        .orders
        .expect("cancel-only risk action should dispatch cancel-all commands");
    assert_eq!(
        orders.yes.dispatch,
        Some(MakerOrderDispatchOutcome::CanceledAll {
            leg: Some(Leg::Yes),
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: None,
        })
    );
    assert_eq!(
        orders.no.dispatch,
        Some(MakerOrderDispatchOutcome::CanceledAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            order_side: None,
        })
    );
    assert_eq!(market.market_state(), MarketState::Draining);
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(writer.records().len(), 0);
    assert_eq!(writer.admission_decisions().len(), 0);
}

#[test]
fn maker_loss_risk_route_hard_flat_does_not_hide_unsupported_active_reduce() {
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.clone(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = resting_market_quote();
    let loss_decision = rejected_loss_decision(LossHaltReason::DailyLossLimit);
    let submit_template = maker_limit_post_only_template();

    let outcome = maker
        .route_maker_loss_risk(
            &mut market,
            risk_route_input(
                &loss_decision,
                MakerLossRiskPolicy {
                    on_loss_breach: MakerRiskMode::HardFlat,
                    on_untrusted_snapshot: MakerRiskMode::CancelOnly,
                },
                &submit_template,
            ),
        )
        .expect("hard-flat loss decision should drain through shared maker order route");

    assert_eq!(outcome.risk.mode, MakerRiskMode::HardFlat);
    assert_eq!(outcome.risk.action, Some(MarketAction::CancelAllBothLegs));
    assert_eq!(
        outcome.risk.blocked_by,
        Some(MakerRiskBlockReason::HardFlatReduceUnsupported)
    );
    let orders = outcome
        .orders
        .expect("hard-flat drain action should dispatch cancel-all commands");
    assert_eq!(
        orders.yes.dispatch,
        Some(MakerOrderDispatchOutcome::CanceledAll {
            leg: Some(Leg::Yes),
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: None,
        })
    );
    assert_eq!(
        orders.no.dispatch,
        Some(MakerOrderDispatchOutcome::CanceledAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            order_side: None,
        })
    );
    assert_eq!(market.market_state(), MarketState::Draining);
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(writer.records().len(), 0);
    assert_eq!(writer.admission_decisions().len(), 0);
}

#[derive(Debug)]
struct FailingRequoteThrottleDecisionEvidenceWriter {
    failure_message: String,
    requote_throttles: Mutex<Vec<BoltV3RequoteThrottleEvidence>>,
}

impl FailingRequoteThrottleDecisionEvidenceWriter {
    fn new(failure_message: impl Into<String>) -> Self {
        Self {
            failure_message: failure_message.into(),
            requote_throttles: Mutex::new(Vec::new()),
        }
    }

    fn requote_throttles(&self) -> Vec<BoltV3RequoteThrottleEvidence> {
        self.requote_throttles
            .lock()
            .expect("requote throttle evidence mutex poisoned")
            .clone()
    }
}

impl BoltV3DecisionEvidenceWriter for FailingRequoteThrottleDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &bolt_v2::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        Ok(())
    }

    fn record_order_intent(
        &self,
        _intent: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_admission_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        _audit: &bolt_v2::bolt_v3_decision_evidence::BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &bolt_v2::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &bolt_v2::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_entry_skip(
        &self,
        _skip: &bolt_v2::bolt_v3_decision_evidence::BoltV3EntrySkipEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_exit_decision(
        &self,
        _decision: &bolt_v2::bolt_v3_decision_evidence::BoltV3ExitDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(
        &self,
        _halt: &bolt_v2::bolt_v3_decision_evidence::BoltV3LossGovernorHaltEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_requote_throttle(&self, throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        self.requote_throttles
            .lock()
            .expect("requote throttle evidence mutex poisoned")
            .push(throttle.clone());
        anyhow::bail!("{}", self.failure_message)
    }

    fn record_exit_evaluation(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3ExitEvaluationEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_order_reject(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3OrderRejectEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_settlement(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3SettlementEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &bolt_v2::bolt_v3_decision_evidence::BoltV3SettlementBookingErrorEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct CapturingLogger {
    records: Mutex<Vec<(log::Level, String)>>,
}

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

impl CapturingLogger {
    fn reset(&self) {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clear();
    }

    fn records(&self) -> Vec<(log::Level, String)> {
        self.records
            .lock()
            .expect("capturing logger mutex poisoned")
            .clone()
    }
}

static CAPTURING_LOGGER: OnceLock<&'static CapturingLogger> = OnceLock::new();
static CAPTURING_LOGGER_OBSERVERS: Mutex<()> = Mutex::new(());

fn install_capturing_logger() -> &'static CapturingLogger {
    static INSTALL_OUTCOME: OnceLock<bool> = OnceLock::new();
    let logger = CAPTURING_LOGGER.get_or_init(|| Box::leak(Box::new(CapturingLogger::default())));
    let installed = *INSTALL_OUTCOME.get_or_init(|| log::set_logger(*logger).is_ok());
    assert!(
        installed,
        "capturing logger could not claim the global log slot; another logger is installed"
    );
    log::set_max_level(log::LevelFilter::Trace);
    logger
}

#[derive(Debug)]
struct NoopFeeProvider;

impl FeeProvider for NoopFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        None
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

fn maker_context(
    writer: Arc<support::RecordingDecisionEvidenceWriter>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    maker_context_with_writer(writer, admission)
}

fn maker_context_with_writer(
    writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer,
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("MAKER.TEST"),
    )
}

fn register_maker_for_order_factory(maker: &mut BinaryOracleMaker) {
    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock.borrow_mut().set_time(UnixNanos::from(1_u64));
    let cache = Rc::new(RefCell::new(Cache::default()));
    let portfolio = Rc::new(RefCell::new(Portfolio::new(
        cache.clone(),
        clock.clone(),
        None,
    )));
    maker
        .core_mut()
        .register(TraderId::from("TRADER-001"), clock, cache, portfolio)
        .expect("maker test strategy should register with NT core");
}

fn maker_config() -> BinaryOracleMakerConfig {
    BinaryOracleMakerConfig {
        strategy_id: "maker-strategy".to_string(),
        order_id_tag: "001".to_string(),
        oms_type: "netting".to_string(),
        client_id: "maker_execution_client".to_string(),
        trade_flow_window_secs: 600,
        trade_flow_max_samples: 1000,
        mu_min_classified_samples: 4,
        mu_stale_window_ms: 60_000,
        mu_min_floor: 0.05,
        requote_min_interval_ms: 500,
        quote_interval_ms: 1_000,
        market_portfolio_max_active_markets: 3,
        market_portfolio_total_bankroll_notional: 1500.0,
        market_portfolio_min_slot_notional: 100.0,
        markets_config_digest: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            .to_string(),
        markets: Vec::new(),
    }
}

fn quote_plan_inputs(family_key: &str) -> MakerQuotePlanInputs<'_> {
    MakerQuotePlanInputs {
        family_key,
        oracle_fair_probability_up: 0.60,
        informed_fraction: gate_cleared_informed_fraction(),
        top_of_book: None,
        microprice_weight: 0.0,
        net_position: 0.0,
        inventory_skew_gain: 0.05,
        position_cap: 10.0,
        half_spread_floor: 0.01,
        max_half_spread: 0.30,
        eps: 0.001,
        tau: 60.0,
        reference_tau: 300.0,
        time_widen_cap: 3.0,
        order_notional_target: 10.0,
        maximum_position_notional: 20.0,
    }
}

/// Mint the toxicity μ this quote-plan fixture uses (0.10) the only way a
/// `UsableMu` can be obtained: through the fail-closed μ gate. `UsableMu` has no
/// public constructor — the newtype's whole purpose is that nothing but
/// [`MakerMuState::usable_mu_for`] can produce one — so this helper drives a real
/// `MakerMuState` with a deterministic warmup flow (11 buyer + 9 seller unit
/// trades → |11 − 9| / 20 = 0.10) and reads the gate, exactly as the runtime will.
fn gate_cleared_informed_fraction() -> UsableMu {
    const BUYS: u64 = 11;
    const SELLS: u64 = 9;
    const STEP_MS: u64 = 1_000;
    // The SI millisecond → nanosecond factor (the crate's NANOS_PER_MILLI_U64 is
    // pub(crate) and unreachable from this integration-test crate).
    const NANOS_PER_MILLI: u64 = 1_000_000;
    let mut state = MakerMuState::new(
        MuEstimatorConfig {
            min_classified_samples: 4,
        },
        MuHealthConfig {
            stale_window_ms: 600_000,
            mu_min_floor: 0.05,
        },
        SignedTradeFlowConfig {
            window_secs: 600,
            max_samples: 1_000,
        },
    );
    let instrument = InstrumentId::from("MUFIXTURE.SIM");
    let mut ts_ms = STEP_MS;
    let observe = |state: &mut MakerMuState, aggressor: AggressorSide, ts_ms: u64| {
        let ts_ns = ts_ms * NANOS_PER_MILLI;
        let trade = TradeTick::new_checked(
            instrument,
            Price::new(0.50, 2),
            Quantity::new(1.0, 0),
            aggressor,
            TradeId::from(format!("MUFIX{ts_ns}").as_str()),
            UnixNanos::from(ts_ns),
            UnixNanos::from(ts_ns),
        )
        .expect("valid fixture trade tick");
        state.observe(&trade);
    };
    for _ in 0..BUYS {
        observe(&mut state, AggressorSide::Buyer, ts_ms);
        ts_ms += STEP_MS;
    }
    for _ in 0..SELLS {
        observe(&mut state, AggressorSide::Seller, ts_ms);
        ts_ms += STEP_MS;
    }
    // `now_ms` == the newest trade's timestamp, so the flow is fresh and the gate
    // clears, yielding the 0.10 imbalance magnitude.
    let now_ms = ts_ms - STEP_MS;
    state
        .usable_mu_for(&instrument, now_ms)
        .expect("warmup flow clears the μ gate")
}

fn quote_set_inputs() -> MakerRuntimeQuoteSetInput<'static> {
    MakerRuntimeQuoteSetInput {
        yes_quantity: 2.0,
        no_quantity: 3.0,
        yes_resting_price: None,
        no_resting_price: None,
        open_commitments: &[],
        max_fee_bps: 0.0,
        available_collateral: 100.0,
        requote_threshold: 0.001,
        eps: 0.001,
        now_ms: 1_000,
    }
}

fn order_plan_inputs() -> MakerRuntimeOrderPlanInput {
    MakerRuntimeOrderPlanInput {
        yes: MakerLegBinding {
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            active_order: None,
            next_order: Some(order_identity("MAKER-YES-1", 1)),
        },
        no: MakerLegBinding {
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            active_order: None,
            next_order: Some(order_identity("MAKER-NO-1", 1)),
        },
    }
}

fn risk_route_input<'a>(
    loss_decision: &'a LossAdmissionDecision,
    policy: MakerLossRiskPolicy,
    submit_template: &'a NtOrderTemplate,
) -> BinaryOracleMakerRiskRouteInput<'a> {
    let quote_set = quote_set_inputs();
    let order_plan = order_plan_inputs();
    BinaryOracleMakerRiskRouteInput {
        loss_decision,
        policy,
        targets: plan_maker_quote_targets(quote_plan_inputs(static_binary_event::KEY))
            .expect("risk-route fixture should produce quote targets")
            .targets,
        yes_quantity: quote_set.yes_quantity,
        no_quantity: quote_set.no_quantity,
        yes: order_plan.yes,
        no: order_plan.no,
        submit_template,
        price_precision: 2,
        quantity_precision: 2,
        submit_order_prefix: "maker_submit",
        max_fee_bps: Decimal::ZERO,
        submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
    }
}

fn resting_market_quote() -> MarketQuote {
    let mut market = MarketQuote::new(false);
    assert_eq!(
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false
            }
        ),
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
    assert_eq!(
        market.on_leg_event(
            Leg::No,
            LegEvent::QuoteTrigger {
                requote_needed: false
            }
        ),
        Some(MarketAction::Leg {
            leg: Leg::No,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(market.on_leg_event(Leg::No, LegEvent::Accepted), None);
    assert_eq!(market.market_state(), MarketState::Quoting);
    market
}

fn accepted_loss_decision() -> LossAdmissionDecision {
    LossAdmissionDecision {
        accepted: true,
        halt_reasons: Vec::new(),
        diagnostics: LossSnapshotDiagnostics::not_evaluated(1),
    }
}

fn rejected_loss_decision(reason: LossHaltReason) -> LossAdmissionDecision {
    LossAdmissionDecision {
        accepted: false,
        halt_reasons: vec![reason],
        diagnostics: LossSnapshotDiagnostics::not_evaluated(1),
    }
}

fn order_identity(client_order_id: &str, generation: u64) -> OrderIdentity {
    OrderIdentity::new(
        MakerClientOrderId::new(client_order_id.to_string()),
        generation,
    )
}

fn maker_limit_post_only_template() -> NtOrderTemplate {
    NtOrderTemplate {
        order_type: OrderType::Limit,
        time_in_force: TimeInForce::Gtc,
        expire_time: None,
        trigger_price: None,
        activation_price: None,
        trigger_type: None,
        trigger_instrument_id: None,
        trailing_offset: None,
        trailing_offset_type: None,
        is_post_only: true,
        is_reduce_only: false,
        is_quote_quantity: false,
    }
}

fn reference_quote(
    asset: &str,
    source_id: &str,
    price: f64,
    observed_ts_ms: u64,
) -> ReferenceQuote {
    ReferenceQuote::try_new(
        asset,
        source_id,
        ReferencePriceProvider::new("fixture_provider")
            .expect("fixture provider identifier should be valid"),
        "fixture_feed",
        price,
        None,
        None,
        observed_ts_ms,
        observed_ts_ms,
    )
    .expect("reference quote fixture should be valid")
}

// ---------------------------------------------------------------------------
// PR-B runtime foundation: the maker consumes PR-A's per-market bindings to
// resolve its active market set, reconcile trade subscriptions, and track the
// per-leg order identities the quote cycle mints. These fixtures use the STATIC
// binary-event family because it identifies a market by a fixed condition id +
// outcomes (no engine-derived time slug), so the fixtures are self-contained.
// Mirrors the proven `binding.rs` static resolver fixtures.
// ---------------------------------------------------------------------------

use bolt_v2::{
    bolt_v3_maker_market_selection::MakerMarketPortfolioPolicy,
    strategies::binary_oracle_maker::{binding::MakerMarketDeclaration, runtime::MakerRuntime},
};
use nautilus_core::Params;
use nautilus_model::{
    enums::AssetClass,
    identifiers::Symbol,
    instruments::{BinaryOption, InstrumentAny},
    types::Currency,
};

const RUNTIME_NOW_MS: u64 = 1_700_000_000_000;
const RUNTIME_STATIC_FAMILY: &str = "static_binary_event";
const RUNTIME_STATIC_SLUG: &str = "will-sample-maker-resolve-yes";
const RUNTIME_STATIC_CONDITION_ID: &str = "condition-sample-maker";
const RUNTIME_STATIC_YES_OUTCOME: &str = "Yes";
const RUNTIME_STATIC_NO_OUTCOME: &str = "No";
const RUNTIME_MARKET_KEY: &str = "eth-static-event";
const RUNTIME_YES_INSTRUMENT: &str = "MAKER-RT-YES.SIM";
const RUNTIME_NO_INSTRUMENT: &str = "MAKER-RT-NO.SIM";
// The YES leg's instrument id after a (hypothetical) venue re-issue of the same
// period's market: a distinct InstrumentId resolved at the same window start.
const RUNTIME_YES_INSTRUMENT_REISSUED: &str = "MAKER-RT-YES-REISSUED.SIM";

fn runtime_static_declaration() -> MakerMarketDeclaration {
    MakerMarketDeclaration {
        market_key: RUNTIME_MARKET_KEY.to_string(),
        family_key: RUNTIME_STATIC_FAMILY.to_string(),
        underlying_asset: "ETH".to_string(),
        cadence_seconds: 3_600,
        cadence_slug_token: RUNTIME_STATIC_SLUG.to_string(),
        static_condition_id: Some(RUNTIME_STATIC_CONDITION_ID.to_string()),
        static_yes_outcome: Some(RUNTIME_STATIC_YES_OUTCOME.to_string()),
        static_no_outcome: Some(RUNTIME_STATIC_NO_OUTCOME.to_string()),
    }
}

fn runtime_binary_option(instrument_id: &str, outcome: &str) -> InstrumentAny {
    runtime_binary_option_with_market_id(
        instrument_id,
        outcome,
        &format!("market-{RUNTIME_STATIC_SLUG}"),
        RUNTIME_NOW_MS - 1_000,
    )
}

fn runtime_binary_option_with_market_id(
    instrument_id: &str,
    outcome: &str,
    market_id: &str,
    activation_milliseconds: u64,
) -> InstrumentAny {
    let question_id = format!("question-{RUNTIME_STATIC_SLUG}");
    let mut info = Params::new();
    for (key, value) in [
        ("market_slug", RUNTIME_STATIC_SLUG),
        ("market_id", market_id),
        ("condition_id", RUNTIME_STATIC_CONDITION_ID),
        ("question_id", question_id.as_str()),
    ] {
        info.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from(instrument_id),
        Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
        AssetClass::Alternative,
        Currency::USDC(),
        (activation_milliseconds.saturating_mul(1_000_000)).into(),
        ((RUNTIME_NOW_MS + 30_000).saturating_mul(1_000_000)).into(),
        3,
        2,
        Price::from("0.001"),
        Quantity::from("0.01"),
        Some(ustr::Ustr::from(outcome)),
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
        None,
        Some(info),
        1.into(),
        1.into(),
    ))
}

fn runtime_static_instruments() -> Vec<InstrumentAny> {
    vec![
        runtime_binary_option(RUNTIME_YES_INSTRUMENT, RUNTIME_STATIC_YES_OUTCOME),
        runtime_binary_option(RUNTIME_NO_INSTRUMENT, RUNTIME_STATIC_NO_OUTCOME),
    ]
}

/// The same declared static market resolved on a LATER cadence window: identical
/// instrument ids / slug / condition id / outcomes AND an unchanged venue
/// `market_id` (so a `market_id`-keyed retain would wrongly retain the stale
/// window), but a later window start — `start_timestamp_milliseconds`, which the
/// static resolver derives from the instrument activation. That window start is the
/// field `apply_resolution` keys the retain-vs-reset decision on, so this fixture is
/// the regression guard for the cadence-roll bug: discriminating a roll only by the
/// venue `market_id` (which a venue may reuse across windows) would keep re-quoting
/// the expired window's instruments.
fn runtime_static_instruments_rolled() -> Vec<InstrumentAny> {
    let market_id = format!("market-{RUNTIME_STATIC_SLUG}");
    vec![
        runtime_binary_option_with_market_id(
            RUNTIME_YES_INSTRUMENT,
            RUNTIME_STATIC_YES_OUTCOME,
            &market_id,
            RUNTIME_NOW_MS - 500,
        ),
        runtime_binary_option_with_market_id(
            RUNTIME_NO_INSTRUMENT,
            RUNTIME_STATIC_NO_OUTCOME,
            &market_id,
            RUNTIME_NOW_MS - 500,
        ),
    ]
}

/// The same declared static market resolved on the SAME cadence window (identical
/// activation => identical `start_timestamp_milliseconds`, same condition id /
/// slug / market_id / outcomes) but with the YES leg re-issued under a NEW
/// instrument id. A retain keyed only on the window start would keep the stale YES
/// instrument; the leg instrument id is read live by the trade-subscription differ,
/// so this is the regression guard that a re-issued instrument under an unchanged
/// window start is treated as a roll (fail-closed) rather than a silent retain.
fn runtime_static_instruments_reissued_yes() -> Vec<InstrumentAny> {
    let market_id = format!("market-{RUNTIME_STATIC_SLUG}");
    vec![
        runtime_binary_option_with_market_id(
            RUNTIME_YES_INSTRUMENT_REISSUED,
            RUNTIME_STATIC_YES_OUTCOME,
            &market_id,
            RUNTIME_NOW_MS - 1_000,
        ),
        runtime_binary_option_with_market_id(
            RUNTIME_NO_INSTRUMENT,
            RUNTIME_STATIC_NO_OUTCOME,
            &market_id,
            RUNTIME_NOW_MS - 1_000,
        ),
    ]
}

fn runtime_portfolio_policy() -> MakerMarketPortfolioPolicy {
    MakerMarketPortfolioPolicy {
        max_active_markets: 3,
        total_bankroll_notional: 1_500.0,
        min_slot_notional: 100.0,
    }
}

#[test]
fn maker_runtime_refresh_resolves_declared_market_and_emits_subscription_delta() {
    // PR-B consumes PR-A's declared markets: a declared static market whose
    // YES/NO instruments are discoverable resolves to one active market, both leg
    // instruments become the newly-active trade-subscription delta, and the active
    // market carries leg bindings whose order identities are UNSET — the quote
    // cycle mints them per cycle, not at resolution. A resolver that silently
    // dropped the market would leave the runtime empty (the pre-PR-B state).
    let mut runtime = MakerRuntime::empty();
    let yes_id = InstrumentId::from(RUNTIME_YES_INSTRUMENT);
    let no_id = InstrumentId::from(RUNTIME_NO_INSTRUMENT);

    let refresh = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );

    assert_eq!(runtime.active_market_count(), 1);
    assert!(
        refresh.misses.is_empty(),
        "a discoverable market must not miss: {:?}",
        refresh.misses
    );
    assert!(refresh.subscribe.contains(&yes_id));
    assert!(refresh.subscribe.contains(&no_id));
    assert!(refresh.unsubscribe.is_empty());
    let market = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("the resolved declared market is active");
    assert_eq!(market.leg_binding(Leg::Yes).instrument_id, yes_id);
    assert_eq!(market.leg_binding(Leg::No).instrument_id, no_id);
    assert!(
        market.leg_binding(Leg::Yes).active_order.is_none(),
        "resolution must not assign an active order identity"
    );
    assert!(
        market.leg_binding(Leg::Yes).next_order.is_none(),
        "resolution must not assign a next order identity"
    );
}

#[test]
fn maker_runtime_refresh_reports_miss_for_undiscoverable_market_not_silent_drop() {
    // Fail-closed: a declared market with no matching instruments surfaces as a
    // miss and produces no active market and no subscription — never a silent idle.
    let mut runtime = MakerRuntime::empty();

    let refresh = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &[],
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );

    assert_eq!(runtime.active_market_count(), 0);
    assert_eq!(refresh.misses.len(), 1);
    assert!(refresh.subscribe.is_empty());
    assert!(refresh.unsubscribe.is_empty());
}

#[test]
fn maker_runtime_refresh_rerolls_when_a_leg_instrument_changes_under_the_same_window() {
    // Fail-closed retain guard (PR #853 external review): the retain-vs-reset
    // decision compares the resolved leg instrument ids, not just the cadence window
    // start. If a venue re-issues the period's market under a new instrument id at an
    // UNCHANGED window start, retaining by window start alone would keep the stale
    // leg instrument — and the leg instrument id is read live by the trade-
    // subscription differ, so the maker would stay subscribed to the gone feed and
    // never subscribe the re-issued one. The second refresh below changes only the
    // YES instrument id (same activation => same start_timestamp_milliseconds), so:
    //   - with the instrument-aware retain predicate it is treated as a roll: the
    //     active binding swaps to the re-issued YES, the re-issued id is subscribed,
    //     and the stale id is unsubscribed;
    //   - with a window-start-only predicate it would be retained: the active binding
    //     keeps the stale YES, `subscribe` omits the re-issued id, and `unsubscribe`
    //     omits the stale id — all three assertions below then fail.
    let mut runtime = MakerRuntime::empty();
    runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );

    let reissued_yes = InstrumentId::from(RUNTIME_YES_INSTRUMENT_REISSUED);
    let stale_yes = InstrumentId::from(RUNTIME_YES_INSTRUMENT);
    let refresh = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments_reissued_yes(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );

    assert_eq!(
        runtime.active_market_count(),
        1,
        "the re-issued market stays active (a roll rebuilds, it does not drop)"
    );
    let market = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("the re-issued declared market is active");
    assert_eq!(
        market.leg_binding(Leg::Yes).instrument_id,
        reissued_yes,
        "a re-issued leg instrument under the same window start must replace the stale one"
    );
    assert!(
        refresh.subscribe.contains(&reissued_yes),
        "the re-issued instrument must be newly subscribed: {:?}",
        refresh.subscribe
    );
    assert!(
        refresh.unsubscribe.contains(&stale_yes),
        "the stale instrument feed must be dropped: {:?}",
        refresh.unsubscribe
    );
}

// ---------------------------------------------------------------------------
// PR-B NT shell: on_start resolves the declared markets against the
// execution-venue-scoped instrument cache and populates the runtime, and one
// intent-only quote cycle mints + assigns + rotates leg order identities while
// the global shadow chokepoint suppresses every venue mutation.
// ---------------------------------------------------------------------------

use bolt_v2::strategies::binary_oracle_maker::BinaryOracleMakerQuoteCycleInput;
use nautilus_common::actor::DataActor;

fn maker_config_with_static_market() -> BinaryOracleMakerConfig {
    BinaryOracleMakerConfig {
        markets: vec![runtime_static_declaration()],
        ..maker_config()
    }
}

/// A build context whose execution venue is a single-token `SIM`, so the static
/// instruments (`*.SIM`) pass the maker's execution-venue cache filter. (The
/// other maker fixtures use a dotted `MAKER.TEST` venue, which never matches an
/// `InstrumentId`'s parsed venue and so is unsuitable for the cache-read path.)
fn maker_sim_context(
    writer: Arc<support::RecordingDecisionEvidenceWriter>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    StrategyBuildContext::new(
        Arc::new(NoopFeeProvider),
        writer,
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("SIM"),
    )
}

/// Register the maker with a real NT core whose clock reads `RUNTIME_NOW_MS`, so
/// the static instruments (whose activation/expiration bracket `RUNTIME_NOW_MS`)
/// are selectable at `on_start`. Returns the cache so the test can seed it.
fn register_maker_at_runtime_now(maker: &mut BinaryOracleMaker) -> Rc<RefCell<Cache>> {
    register_maker_at_runtime_now_with_quote_timer_handler(maker, true)
}

/// Register the maker against a `TestClock` set to `RUNTIME_NOW_MS`. When
/// `wire_quote_timer_handler` is true this also registers the clock's default
/// time-event handler, mirroring NT's `DataActor::register` (which wires it in
/// production); without it `TestClock::set_timer_ns` returns "No callbacks
/// provided" and `on_start`'s quote-timer registration fails loud. The bare
/// `core_mut().register` used here performs only the core registration, so the
/// handler must be wired explicitly to reproduce the live start path.
fn register_maker_at_runtime_now_with_quote_timer_handler(
    maker: &mut BinaryOracleMaker,
    wire_quote_timer_handler: bool,
) -> Rc<RefCell<Cache>> {
    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)));
    if wire_quote_timer_handler {
        clock
            .borrow_mut()
            .register_default_handler(TimeEventCallback::from(|_event: TimeEvent| {}));
    }
    let cache = Rc::new(RefCell::new(Cache::default()));
    let portfolio = Rc::new(RefCell::new(Portfolio::new(
        cache.clone(),
        clock.clone(),
        None,
    )));
    maker
        .core_mut()
        .register(
            TraderId::from("TRADER-001"),
            clock,
            cache.clone(),
            portfolio,
        )
        .expect("maker test strategy should register with NT core");
    cache
}

#[test]
fn maker_on_start_resolves_declared_markets_from_the_execution_venue_cache() {
    // The NT shell wiring: on_start reads the execution-venue-scoped instrument
    // cache, resolves the declared markets through the shared engine, and tracks
    // them in the runtime. With both leg instruments cached on the maker's venue,
    // the declared market becomes active (an empty cache would leave it idle).
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer, admission),
    );
    let cache = register_maker_at_runtime_now(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    // `on_start` is declared by both `DataActor` and `Strategy` (a subtrait); the
    // actor lifecycle invokes `DataActor::on_start` (the maker's override), so the
    // test drives that exact method.
    DataActor::on_start(&mut maker).expect("on_start resolves and subscribes the declared markets");

    assert_eq!(maker.runtime().active_market_count(), 1);
    let market = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the declared market is active after on_start");
    assert_eq!(
        market.leg_binding(Leg::Yes).instrument_id,
        InstrumentId::from(RUNTIME_YES_INSTRUMENT)
    );
    assert_eq!(
        market.leg_binding(Leg::No).instrument_id,
        InstrumentId::from(RUNTIME_NO_INSTRUMENT)
    );
}

#[test]
fn maker_on_stop_resets_runtime_so_a_restart_re_resolves_and_re_subscribes() {
    // on_stop resets the runtime to empty (after unsubscribing) so a stop/start
    // restart re-resolves from empty and re-emits the full trade-subscription delta.
    // Without that reset the runtime keeps its active markets, the next on_start's
    // refresh diffs before == after, no subscribe delta is emitted, and the maker
    // runs active with no trade feeds. The post-on_stop `active_market_count() == 0`
    // assertion below is differential: it fails if the on_stop runtime reset is
    // removed (the count would stay 1, and no re-subscribe would be emitted).
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer, admission),
    );
    let cache = register_maker_at_runtime_now(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    DataActor::on_start(&mut maker).expect("first on_start resolves the declared market");
    assert_eq!(
        maker.runtime().active_market_count(),
        1,
        "the declared market is active after the first on_start"
    );

    DataActor::on_stop(&mut maker).expect("on_stop tears the runtime down cleanly");
    assert_eq!(
        maker.runtime().active_market_count(),
        0,
        "on_stop must reset the runtime to empty so a restart re-emits the subscribe delta"
    );

    DataActor::on_start(&mut maker).expect("second on_start re-resolves from the empty runtime");
    assert_eq!(
        maker.runtime().active_market_count(),
        1,
        "the restart re-activates the declared market from an empty runtime"
    );
}

#[test]
fn maker_on_start_fails_loud_when_quote_interval_overflows_the_nanosecond_clock() {
    // register_quote_timer converts quote_interval_ms into nanoseconds with a
    // checked_mul; a value so large that the ms -> ns conversion overflows u64 must
    // abort on_start (fail loud) rather than silently run with a wrong/saturated
    // cadence. Differential: it fails if the checked_mul guard is reverted to the
    // prior saturating_mul (which would silently clamp instead of erroring).
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let config = BinaryOracleMakerConfig {
        quote_interval_ms: u64::MAX,
        ..maker_config_with_static_market()
    };
    let mut maker = BinaryOracleMaker::new(config, maker_sim_context(writer, admission));
    let cache = register_maker_at_runtime_now(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    let error = DataActor::on_start(&mut maker)
        .expect_err("an overflowing quote_interval_ms must fail on_start");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("overflows the nanosecond clock"),
        "on_start should fail loud naming the nanosecond-clock overflow: {rendered}"
    );
    // No half-started runtime: on_start registers the quote timer BEFORE resolving
    // markets, so the abort precedes any market subscription even though the cache
    // already holds the declared instruments. Differential: reordering on_start to
    // refresh markets before the timer would leave active_market_count() == 1 here.
    assert_eq!(
        maker.runtime().active_market_count(),
        0,
        "a failed on_start must leave no resolved/subscribed markets behind"
    );
}

#[test]
fn maker_on_start_fails_loud_when_the_quote_timer_cannot_register() {
    // register_quote_timer registers the autonomous quote/refresh timer through
    // NT's clock default time-event handler, which the actor lifecycle wires in
    // production (DataActor::register). If that registration fails, on_start must
    // abort (fail loud) rather than run resolved markets with no quote/refresh
    // cadence (never reconciling a cadence roll). Differential: a clock with NO
    // default handler makes TestClock::set_timer_ns return "No callbacks
    // provided", so on_start must error naming the timer-registration failure. If
    // register_quote_timer is reverted to logging-and-swallowing that error,
    // on_start returns Ok and this expect_err fails.
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer, admission),
    );
    let cache = register_maker_at_runtime_now_with_quote_timer_handler(&mut maker, false);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    let error = DataActor::on_start(&mut maker)
        .expect_err("a quote timer that cannot register must fail on_start");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("quote timer registration failed"),
        "on_start should fail loud naming the quote timer registration failure: {rendered}"
    );
    // No half-started runtime: the quote timer registers BEFORE markets resolve, so
    // a registration failure aborts on_start before any market subscription even
    // though the cache holds the declared instruments. Differential: reordering
    // on_start to refresh markets first would leave active_market_count() == 1 here.
    assert_eq!(
        maker.runtime().active_market_count(),
        0,
        "a failed on_start must leave no resolved/subscribed markets behind"
    );
}

#[test]
fn maker_run_quote_cycle_assigns_identities_and_emits_intent_in_shadow() {
    // The keystone PR-B behavior: once a market is active, one quote cycle mints
    // fresh leg order identities, emits order INTENT through the shared execution
    // context, and rotates the dispatched identity from `next` to `active`. The
    // global shadow chokepoint suppresses every venue mutation, so the intent is
    // produced but nothing is admitted.
    let writer = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.clone()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer, admission.clone()),
    );
    let cache = register_maker_at_runtime_now(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }
    DataActor::on_start(&mut maker).expect("on_start resolves the declared market");
    assert_eq!(maker.runtime().active_market_count(), 1);

    let yes_id = InstrumentId::from(RUNTIME_YES_INSTRUMENT);
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let submit_template = maker_limit_post_only_template();

    let outcome = maker
        .run_quote_cycle(
            RUNTIME_MARKET_KEY,
            &mut market,
            &mut budget,
            BinaryOracleMakerQuoteCycleInput {
                quote_plan: quote_plan_inputs(static_binary_event::KEY),
                quote_set: quote_set_inputs(),
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
                max_fee_bps: Decimal::ZERO,
                submit_lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            },
        )
        .expect("run_quote_cycle routes an active market")
        .expect("an active market yields a quote-cycle outcome");

    let orders = outcome
        .orders
        .expect("a fresh market quote cycle dispatches leg order intent");
    let no_id = InstrumentId::from(RUNTIME_NO_INSTRUMENT);
    match &orders.yes.dispatch {
        Some(MakerOrderDispatchOutcome::Submitted { instrument_id, .. }) => {
            assert_eq!(
                *instrument_id, yes_id,
                "the YES leg intent must target the resolved YES instrument"
            );
        }
        other => panic!("expected a YES submit intent in shadow, got {other:?}"),
    }
    // Clause (c) is per-leg: the NO leg mints + dispatches its own intent. Asserting
    // only the YES leg would let a regression that dropped the NO-leg rotation, or
    // transposed both rotations onto YES, ship green.
    match &orders.no.dispatch {
        Some(MakerOrderDispatchOutcome::Submitted { instrument_id, .. }) => {
            assert_eq!(
                *instrument_id, no_id,
                "the NO leg intent must target the resolved NO instrument"
            );
        }
        other => panic!("expected a NO submit intent in shadow, got {other:?}"),
    }
    // Shadow chokepoint: intent emitted, nothing admitted to the venue.
    assert_eq!(admission.admitted_order_count(), 0);

    // The dispatched identity rotated from `next` to `active` on BOTH legs; the next
    // slot is consumed so the following cycle mints a fresh generation.
    let market_runtime = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the market is still active after the cycle");
    assert!(
        market_runtime.leg_binding(Leg::Yes).active_order.is_some(),
        "a submitted YES intent must rotate the minted identity to active"
    );
    assert!(
        market_runtime.leg_binding(Leg::Yes).next_order.is_none(),
        "the minted next YES identity is consumed by the submit"
    );
    assert!(
        market_runtime.leg_binding(Leg::No).active_order.is_some(),
        "a submitted NO intent must rotate the minted identity to active"
    );
    assert!(
        market_runtime.leg_binding(Leg::No).next_order.is_none(),
        "the minted next NO identity is consumed by the submit"
    );
}

#[test]
fn maker_runtime_retains_identities_on_same_window_and_rebuilds_on_roll() {
    // Stateful clauses (b)/(c) invariant + the cadence-roll id guard. A second
    // refresh whose market resolves to the SAME window start retains the assigned
    // identities and the monotonic per-leg generation counters; a rolled window
    // start (same declared market_key and an UNCHANGED venue market_id, new cadence
    // window) rebuilds the runtime with unset identities but carries the generation
    // counters forward, and the post-roll re-mint must never reproduce a pre-roll
    // client order id. The single-pass suite begins from `MakerRuntime::empty()`, so
    // it never enters the retain branch — an inverted retain guard, a retain keyed on
    // the (unchanged) market_id, or a re-mint that dropped the carried generation
    // would otherwise ship green.
    let mut runtime = MakerRuntime::empty();
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(
        runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"),
        "the declared market is active, so minting succeeds"
    );
    let pre_roll_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("the declared market is active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a next YES identity was minted");

    // Second refresh, SAME instruments => SAME window start => retain.
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert_eq!(
        runtime
            .market(RUNTIME_MARKET_KEY)
            .expect("the market stays active across an unchanged-window refresh")
            .leg_binding(Leg::Yes)
            .next_order
            .as_ref(),
        Some(&pre_roll_yes),
        "an unchanged cadence window retains the assigned identity (no reset)"
    );
    // The retained generation counter is monotonic: the next mint advances past the
    // pre-roll id rather than repeating it.
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let advanced_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("market active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a fresh YES identity after a retained refresh");
    assert_ne!(
        advanced_yes.client_order_id(),
        pre_roll_yes.client_order_id(),
        "a retained window advances the generation, never repeats an id"
    );

    // Third refresh, ROLLED window start => rebuilt runtime, identities unset, the
    // per-leg generation carried forward (not reset).
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments_rolled(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(
        runtime
            .market(RUNTIME_MARKET_KEY)
            .expect("the rolled market is active")
            .leg_binding(Leg::Yes)
            .next_order
            .is_none(),
        "a rolled cadence window rebuilds the runtime with unset identities"
    );
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let post_roll_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("market active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a YES identity on the rolled window");
    assert_ne!(
        post_roll_yes.client_order_id(),
        pre_roll_yes.client_order_id(),
        "a post-roll re-mint must never reproduce a pre-roll client order id: the \
         carried generation counter advances across the roll, so the id stays unique \
         even though a window roll also moves the window start"
    );
}

#[test]
fn maker_runtime_mints_a_unique_id_when_a_leg_instrument_rerolls_under_the_same_window() {
    // The instrument-only-roll id guard. A leg-instrument re-issue at an UNCHANGED
    // window start is a roll (`same_window` is false), but the window start the client
    // order id embeds does NOT change — so the window start alone cannot discriminate
    // the post-roll id from the pre-roll one. Only the per-leg generation counter,
    // CARRIED forward across the roll, keeps the re-minted id unique. If the roll
    // reset the generation to 0, the post-roll re-mint would reproduce the pre-roll
    // client order id (NautilusTrader never reuses a `ClientOrderId`), so this fails
    // on a generation-reset-on-roll rebuild.
    let mut runtime = MakerRuntime::empty();
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(
        runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"),
        "the declared market is active, so minting succeeds"
    );
    let pre_roll_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("the declared market is active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a next YES identity was minted");

    // Second refresh: SAME window start, the YES leg re-issued under a NEW instrument
    // id => `same_window` is false => roll (not retain), but the embedded window start
    // is unchanged, so only the carried generation can keep the re-mint unique.
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments_reissued_yes(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(
        runtime
            .market(RUNTIME_MARKET_KEY)
            .expect("the re-issued market is active")
            .leg_binding(Leg::Yes)
            .next_order
            .is_none(),
        "the instrument-only roll rebuilds the runtime with unset identities"
    );
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let post_roll_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("market active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a YES identity on the re-issued window");
    assert_ne!(
        post_roll_yes.client_order_id(),
        pre_roll_yes.client_order_id(),
        "a leg-instrument re-issue at an unchanged window start must still mint a \
         unique client order id: the window start cannot discriminate it, so the \
         per-leg generation must carry forward across the roll (never reset to 0)"
    );
}

#[test]
fn maker_runtime_mints_a_unique_id_after_a_market_drops_and_refills_the_same_window() {
    // The drop/refill id guard (PR #853 external review: GPT/GLM/Kimi). A market can
    // leave the active set WITHOUT a window roll — a transient resolution miss, or the
    // shared planner blocking the whole plan that cycle — then refill the SAME cadence
    // window on a later refresh. The client order id embeds the window start, which is
    // unchanged across such a gap, so only a generation that SURVIVES the drop keeps the
    // re-mint unique. The per-(market_key, leg) high-water lives on `MakerRuntime`, not
    // on the per-refresh per-market runtime, so it persists across the gap; if the
    // generation reset to 0 on the refill, the re-mint would reproduce the pre-drop
    // client order id (NautilusTrader never reuses a `ClientOrderId`), failing the final
    // assertion.
    let mut runtime = MakerRuntime::empty();
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let pre_drop_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("the declared market is active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a next YES identity was minted");

    // Drop: the market resolves to nothing this refresh (no matching instruments), so
    // it leaves the active set entirely (surfaced as a miss, never a silent retain).
    let dropped = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &[],
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert_eq!(
        runtime.active_market_count(),
        0,
        "the unresolvable market drops out of the active set"
    );
    assert_eq!(
        dropped.misses.len(),
        1,
        "the drop is surfaced as a miss, not a silent idle"
    );

    // Refill: the SAME declared market resolves again at the SAME window start.
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(
        runtime
            .market(RUNTIME_MARKET_KEY)
            .expect("the refilled market is active")
            .leg_binding(Leg::Yes)
            .next_order
            .is_none(),
        "a refill rebuilds the runtime with unset identities"
    );
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let post_refill_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("market active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a YES identity on the refilled window");
    assert_ne!(
        post_refill_yes.client_order_id(),
        pre_drop_yes.client_order_id(),
        "a market that drops and refills the same window must not re-mint a consumed \
         client order id: the per-(market_key, leg) generation high-water survives the \
         drop, so the re-mint advances past the pre-drop generation"
    );
}

#[test]
fn maker_runtime_deactivate_all_preserves_generation_high_water_for_a_restart() {
    // The within-process stop/start id guard. `on_stop` deactivates the runtime (clears
    // the active markets so the next `on_start` re-emits the full subscription delta)
    // but must NOT discard the per-(market_key, leg) generation high-water: a restart
    // re-resolves the SAME cadence window, and the client order id embeds the window
    // start, so a re-mint from generation 0 would reproduce a client order id the
    // pre-stop run consumed. `deactivate_all` retains the high-water; replacing the
    // runtime with `empty()` (the pre-fix behaviour) would reset it and fail the final
    // assertion. The subscribe-delta assertion also pins that deactivation still clears
    // the active set (so the restart re-subscribes), the reason on_stop clears markets.
    let mut runtime = MakerRuntime::empty();
    let _ = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let pre_stop_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("the declared market is active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a next YES identity was minted");

    // Stop: deactivate the active set (as `on_stop` does).
    runtime.deactivate_all();
    assert_eq!(
        runtime.active_market_count(),
        0,
        "deactivation clears the active set so a restart re-subscribes"
    );

    // Restart: `on_start` re-resolves the SAME window.
    let restart = runtime.refresh_active_markets(
        &[runtime_static_declaration()],
        &runtime_static_instruments(),
        RUNTIME_NOW_MS,
        runtime_portfolio_policy(),
    );
    assert!(
        restart
            .subscribe
            .contains(&InstrumentId::from(RUNTIME_YES_INSTRUMENT)),
        "a deactivated restart re-emits the full subscribe delta: {:?}",
        restart.subscribe
    );
    assert!(runtime.mint_next_identities(RUNTIME_MARKET_KEY, "001"));
    let post_restart_yes = runtime
        .market(RUNTIME_MARKET_KEY)
        .expect("market active")
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("a YES identity after restart");
    assert_ne!(
        post_restart_yes.client_order_id(),
        pre_stop_yes.client_order_id(),
        "a within-process restart must not re-mint a consumed client order id: \
         deactivation preserves the generation high-water, so the re-mint advances"
    );
}
