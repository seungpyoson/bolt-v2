mod support;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::ReferencePriceProvider,
    bolt_v3_decision_evidence::BoltV3AdmissionOutcome,
    bolt_v3_loss_governor::{LossAdmissionDecision, LossHaltReason},
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
    bolt_v3_numeric::NANOS_PER_MILLI_U64,
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
use nautilus_common::{cache::Cache, clock::TestClock};
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
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

const TEST_REFERENCE_ASSET: &str = "reference_asset";
const TEST_REALIZED_VOL_SURFACE_ID: &str = "maker_reference_surface";
const TEST_REALIZED_VOL_SOURCE_ID: &str = "maker_reference_rv";

fn ready_realized_vol_snapshot(as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
    RealizedVolSnapshot {
        surface_id: TEST_REALIZED_VOL_SURFACE_ID.to_string(),
        as_of_ms,
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
        now_ms: 1_500,
        reference_quotes: &quotes,
        strike_price: Some(100.0),
        seconds_to_market_end: Some(300),
        realized_volatility_snapshot: &realized_volatility_snapshot,
        pricing_kurtosis: 0.25,
    };
    let expected_fair_probability_up = updown::fair_probability_up(&FairProbabilityInputs {
        spot_price: 100.05,
        strike_price: fair_input.strike_price.expect("fixture strike"),
        seconds_to_market_end: fair_input.seconds_to_market_end.expect("fixture expiry"),
        realized_vol: 1.5,
        pricing_kurtosis: fair_input.pricing_kurtosis,
    })
    .expect("updown fixture should price");
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
                quote_set: quote_set_inputs(),
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
                quote_set: quote_set_inputs(),
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
                    quote_set: quote_set_inputs(),
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
                quote_set: quote_set_inputs(),
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
        market_portfolio_max_active_markets: 3,
        market_portfolio_total_bankroll_notional: 1500.0,
        market_portfolio_min_slot_notional: 100.0,
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
    let mut observe = |state: &mut MakerMuState, aggressor: AggressorSide, ts_ms: u64| {
        let ts_ns = ts_ms * NANOS_PER_MILLI_U64;
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
    }
}

fn rejected_loss_decision(reason: LossHaltReason) -> LossAdmissionDecision {
    LossAdmissionDecision {
        accepted: false,
        halt_reasons: vec![reason],
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
