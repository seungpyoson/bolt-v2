use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::ReferencePriceProvider,
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_mu_estimator::{MuEstimatorConfig, MuHealthConfig, UsableMu},
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_order_dispatch::{
        MakerOrderCommandAuthority, MakerOrderCommandFailure, MakerOrderCommandFailureKind,
        MakerOrderCommandSink, MakerOrderDispatchInput, MakerOrderDispatchOutcome,
        MakerQuoteTransactionContext, dispatch_maker_order_command,
    },
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerMarketActionOrderInput, MakerOrderIntent,
        maker_order_plan_from_market_action,
    },
    bolt_v3_maker_quote_control::{QuoteControlInput, drive_quote_leg},
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_rate_budget::build_requote_budget_pair,
    bolt_v3_maker_runtime_order::{
        MakerRuntimeOrderDispatchInput, dispatch_maker_runtime_order_plan_with_command_router,
    },
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteBlockReason, MakerRuntimeQuoteInput,
        MakerRuntimeQuoteSetInput, MakerRuntimeReferenceFairValueBlockReason,
        MakerRuntimeReferenceFairValueInput, maker_reference_current_price_fair_value,
        maker_reference_current_price_fair_value_decision, plan_maker_runtime_quote,
    },
    bolt_v3_market_families::{FairProbabilityInputs, static_binary_event, updown},
    bolt_v3_order_execution::{
        BoltV3RestingRegistrationCommitParticipant, BoltV3RestingSubmitTransactionOutcome,
        RestingOrderCancelHandled,
    },
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LifecycleAction, MakerOrderLifecycleScopeIdentity,
        MakerQuoteLifecycleIdentity, MarketAction, MarketQuote, MarketState,
    },
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
        RealizedVolSnapshot,
    },
    bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote},
    bolt_v3_timestamp_domain::LocalReceiveMs,
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::binary_oracle_maker::mu::MakerMuState,
};
use nautilus_common::{
    clock::{Clock, TestClock},
    factories::OrderFactory,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::{AggressorSide, OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, StrategyId, TradeId, TraderId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};
use std::{
    cell::{RefCell, RefMut},
    collections::BTreeMap,
    rc::Rc,
};

const TEST_REFERENCE_ASSET: &str = "reference_asset";
const TEST_REALIZED_VOL_SURFACE_ID: &str = "maker_reference_surface";
const TEST_REALIZED_VOL_SOURCE_ID: &str = "maker_reference_rv";

fn ready_realized_vol_snapshot(as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
    realized_vol_snapshot(as_of_ms, realized_vol, true)
}

fn unready_realized_vol_snapshot(as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
    realized_vol_snapshot(as_of_ms, realized_vol, false)
}

fn realized_vol_snapshot(as_of_ms: u64, realized_vol: f64, ready: bool) -> RealizedVolSnapshot {
    RealizedVolSnapshot {
        surface_id: TEST_REALIZED_VOL_SURFACE_ID.to_string(),
        as_of_ms,
        latest_accepted_receive_ms: Some(LocalReceiveMs::new(as_of_ms)),
        annualized_realized_vol_decimal: Some(realized_vol),
        measured_annualized_realized_vol_decimal: Some(realized_vol),
        noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
        continuous_annualized_realized_vol_decimal: Some(realized_vol),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready,
        sources_used: vec![TEST_REALIZED_VOL_SOURCE_ID.to_string()],
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: if ready {
            Vec::new()
        } else {
            vec![RealizedVolBlockReason::QuorumNotReady]
        },
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: String::new(),
    }
}

#[test]
fn runtime_quote_tick_uses_family_quote_plan_and_produces_order_intents() {
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    );

    assert_eq!(decision.blocked_by, None);
    let quote_plan = decision
        .quote_plan
        .expect("supported family should produce a quote plan");
    assert_eq!(quote_plan.fair_probability_up, 0.60);
    let order_plan = decision
        .order_plan
        .expect("quote actions should map to maker order intents");

    match order_plan.yes.intent {
        Some(MakerOrderIntent::Submit {
            leg,
            instrument_id,
            price,
            quantity,
            ..
        }) => {
            assert_eq!(leg, Leg::Yes);
            assert_eq!(instrument_id, InstrumentId::from("YES.RUNTIME"));
            assert_eq!(price, quote_plan.targets.leg_a.price);
            assert_eq!(quantity, 2.0);
        }
        other => panic!("expected yes submit intent, got {other:?}"),
    }

    match order_plan.no.intent {
        Some(MakerOrderIntent::Submit {
            leg,
            instrument_id,
            price,
            quantity,
            ..
        }) => {
            assert_eq!(leg, Leg::No);
            assert_eq!(instrument_id, InstrumentId::from("NO.RUNTIME"));
            assert_eq!(price, quote_plan.targets.leg_b.price);
            assert_eq!(quantity, 3.0);
        }
        other => panic!("expected no submit intent, got {other:?}"),
    }

    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
}

#[test]
fn maker_reference_current_price_selection_feeds_family_runtime_quote_plan() {
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        500,
        25,
    )
    .expect("selector fixture should be valid");
    let quotes = vec![reference_quote(
        TEST_REFERENCE_ASSET,
        "primary",
        0.63,
        1_000,
    )];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(1_000, 1.5);

    let fair = maker_reference_current_price_fair_value(
        &mut selector,
        1_000,
        MakerRuntimeReferenceFairValueInput {
            family_key: static_binary_event::KEY,
            interval_start_ms: 1_000,
            interval_end_ms: 2_000,
            reference_quotes: &quotes,
            strike_price: Some(0.50),
            seconds_to_market_end: Some(0),
            realized_volatility_snapshot: &realized_volatility_snapshot,
            realized_volatility_max_source_age_ms: None,
            pricing_kurtosis: f64::NAN,
            evaluation_receive_ms: LocalReceiveMs::new(1_000),
        },
    )
    .expect("reference-current-price fair value should be available");

    assert_eq!(fair.source_id, "primary");
    assert_eq!(fair.reference_current_price, 0.63);
    assert_eq!(fair.reference_current_price_observed_ts_ms, 1_000);
    assert_eq!(
        fair.realized_vol_surface_id.as_deref(),
        Some(TEST_REALIZED_VOL_SURFACE_ID)
    );
    assert_eq!(
        fair.realized_vol_source_venue.as_deref(),
        Some(TEST_REALIZED_VOL_SOURCE_ID)
    );
    assert_eq!(fair.realized_vol_source_ts_ms, Some(1_000));
    assert_eq!(fair.fair_probability_up, 0.63);
    assert!(!fair.failed_over);

    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs_with_fair(
                static_binary_event::KEY,
                fair.fair_probability_up,
            ),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    );

    assert_eq!(decision.blocked_by, None);
    assert_eq!(
        decision
            .quote_plan
            .expect("reference fair value should feed a quote plan")
            .fair_probability_up,
        0.63
    );
    assert!(
        decision
            .order_plan
            .expect("reference fair value should feed maker orders")
            .yes
            .intent
            .is_some()
    );
}

#[test]
fn maker_pricing_does_not_compare_independent_venue_clocks() {
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        500,
        25,
    )
    .expect("selector fixture should be valid");
    let quotes = vec![reference_quote(
        TEST_REFERENCE_ASSET,
        "primary",
        0.63,
        1_000,
    )];
    let mut realized_volatility_snapshot = ready_realized_vol_snapshot(1_001, 1.5);
    realized_volatility_snapshot.latest_accepted_receive_ms = Some(LocalReceiveMs::new(999));

    let result = maker_reference_current_price_fair_value(
        &mut selector,
        1_000,
        MakerRuntimeReferenceFairValueInput {
            family_key: static_binary_event::KEY,
            interval_start_ms: 1_000,
            interval_end_ms: 2_000,
            reference_quotes: &quotes,
            strike_price: Some(0.50),
            seconds_to_market_end: Some(0),
            realized_volatility_snapshot: &realized_volatility_snapshot,
            realized_volatility_max_source_age_ms: None,
            pricing_kurtosis: f64::NAN,
            evaluation_receive_ms: LocalReceiveMs::new(1_000),
        },
    );

    assert!(
        result.is_some(),
        "maker pricing must not reject an RV source venue clock that leads by one millisecond"
    );
}

#[test]
fn rv_clock_domain_amendment_maker_route_owns_explicit_evaluation_receive_time() {
    let quote_receive_ms = LocalReceiveMs::new(1_000);
    let snapshot_receive_ms = LocalReceiveMs::new(1_100);
    let evaluation_receive_ms = LocalReceiveMs::new(1_150);
    let lifecycle_now_ms = 1_251;
    assert!(quote_receive_ms < snapshot_receive_ms);
    assert!(snapshot_receive_ms <= evaluation_receive_ms);
    assert!(
        evaluation_receive_ms.value() < lifecycle_now_ms,
        "the differential must distinguish caller-owned receive time from lifecycle wall time"
    );

    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        500,
        25,
    )
    .expect("selector fixture should be valid");
    let quotes = vec![reference_quote_received_at(
        TEST_REFERENCE_ASSET,
        "primary",
        0.63,
        1_000,
        quote_receive_ms.value(),
    )];
    let mut realized_volatility_snapshot = ready_realized_vol_snapshot(1_100, 1.5);
    realized_volatility_snapshot.latest_accepted_receive_ms = Some(snapshot_receive_ms);

    let result = maker_reference_current_price_fair_value_decision(
        &mut selector,
        lifecycle_now_ms,
        MakerRuntimeReferenceFairValueInput {
            family_key: static_binary_event::KEY,
            interval_start_ms: 1_000,
            interval_end_ms: 2_000,
            reference_quotes: &quotes,
            strike_price: Some(0.50),
            seconds_to_market_end: Some(0),
            realized_volatility_snapshot: &realized_volatility_snapshot,
            realized_volatility_max_source_age_ms: Some(100),
            pricing_kurtosis: f64::NAN,
            evaluation_receive_ms,
        },
    );

    assert_eq!(result.blocked_by, None);
    assert!(
        result.fair_value.is_some(),
        "maker pricing must evaluate RV freshness at the caller-owned receive stamp"
    );
}

#[test]
fn maker_reference_current_price_decision_records_taker_fair_value_inputs_and_blockers() {
    let quotes = vec![
        reference_quote(TEST_REFERENCE_ASSET, "primary", 99.0, 1_000),
        reference_quote(TEST_REFERENCE_ASSET, "backup", 101.0, 1_490),
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
    let input = MakerRuntimeReferenceFairValueInput {
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

    let decision = maker_reference_current_price_fair_value_decision(&mut selector, 1_500, input);

    assert_eq!(decision.blocked_by, None);
    let fair = decision
        .fair_value
        .expect("fresh backup reference current price should price");
    assert_eq!(fair.spot_price, 101.0);
    assert_eq!(
        fair.strike_price,
        input.strike_price.expect("fixture strike")
    );
    assert_eq!(
        fair.seconds_to_market_end,
        input.seconds_to_market_end.expect("fixture expiry")
    );
    assert_eq!(fair.realized_vol, 1.5);
    assert_eq!(fair.pricing_kurtosis, input.pricing_kurtosis);
    assert_eq!(fair.reference_current_price, 101.0);
    assert_eq!(fair.reference_current_price_source_id, "backup");
    assert_eq!(fair.reference_current_price_observed_ts_ms, 1_490);
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
    assert_eq!(
        fair.fair_probability_up,
        updown::fair_probability_up(&FairProbabilityInputs {
            spot_price: 101.0,
            strike_price: input.strike_price.expect("fixture strike"),
            seconds_to_market_end: input.seconds_to_market_end.expect("fixture expiry"),
            realized_vol: 1.5,
            pricing_kurtosis: input.pricing_kurtosis,
        })
        .expect("same updown fair-value inputs should price")
        .value()
    );

    let mut blocked_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let blocked = maker_reference_current_price_fair_value_decision(
        &mut blocked_selector,
        1_500,
        MakerRuntimeReferenceFairValueInput {
            family_key: updown::KEY,
            interval_start_ms: 1_000,
            interval_end_ms: 2_000,
            reference_quotes: &[],
            strike_price: input.strike_price,
            seconds_to_market_end: input.seconds_to_market_end,
            realized_volatility_snapshot: input.realized_volatility_snapshot,
            realized_volatility_max_source_age_ms: input.realized_volatility_max_source_age_ms,
            pricing_kurtosis: input.pricing_kurtosis,
            evaluation_receive_ms: input.evaluation_receive_ms,
        },
    );

    assert_eq!(blocked.fair_value, None);
    assert_eq!(
        blocked.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::ReferenceCurrentPriceUnavailable)
    );

    let unready_snapshot = unready_realized_vol_snapshot(1_400, 1.5);
    let mut rv_blocked_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let rv_blocked = maker_reference_current_price_fair_value_decision(
        &mut rv_blocked_selector,
        1_500,
        MakerRuntimeReferenceFairValueInput {
            realized_volatility_snapshot: &unready_snapshot,
            ..input
        },
    );

    assert_eq!(rv_blocked.fair_value, None);
    assert_eq!(
        rv_blocked.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::RealizedVolNotReady)
    );

    let stale_snapshot = ready_realized_vol_snapshot(1_400, 1.5);
    let mut stale_rv_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let stale_rv = maker_reference_current_price_fair_value_decision(
        &mut stale_rv_selector,
        1_500,
        MakerRuntimeReferenceFairValueInput {
            realized_volatility_snapshot: &stale_snapshot,
            realized_volatility_max_source_age_ms: Some(50),
            ..input
        },
    );

    assert_eq!(stale_rv.fair_value, None);
    assert_eq!(
        stale_rv.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::RealizedVolNotReady)
    );

    let mut missing_strike_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let missing_strike = maker_reference_current_price_fair_value_decision(
        &mut missing_strike_selector,
        1_500,
        MakerRuntimeReferenceFairValueInput {
            strike_price: None,
            ..input
        },
    );

    assert_eq!(missing_strike.fair_value, None);
    assert_eq!(
        missing_strike.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::StrikePriceMissing)
    );

    let mut missing_expiry_selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let missing_expiry = maker_reference_current_price_fair_value_decision(
        &mut missing_expiry_selector,
        1_500,
        MakerRuntimeReferenceFairValueInput {
            seconds_to_market_end: None,
            ..input
        },
    );

    assert_eq!(missing_expiry.fair_value, None);
    assert_eq!(
        missing_expiry.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::SecondsToExpiryMissing)
    );
}

#[test]
fn runtime_quote_tick_fails_closed_for_unsupported_family_without_mutation() {
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs("missing_family"),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    );

    assert_eq!(
        decision.blocked_by,
        Some(MakerRuntimeQuoteBlockReason::QuotePlanUnavailable)
    );
    assert!(decision.quote_plan.is_none());
    assert!(decision.quote_set.is_none());
    assert!(decision.order_plan.is_none());
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
}

#[test]
fn runtime_quote_order_plan_compiles_and_dispatches_both_legs() {
    let mut market = MarketQuote::new(
        MakerOrderLifecycleScopeIdentity::new(
            1_000,
            InstrumentId::from("YES.RUNTIME"),
            InstrumentId::from("NO.RUNTIME"),
        ),
        false,
    );
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    );
    let order_plan = decision
        .order_plan
        .as_ref()
        .expect("quote tick should produce maker order intents");

    let mut sink = RecordingMakerOrderSink::new();
    let quote_set = decision
        .quote_set
        .as_ref()
        .expect("quote tick should produce transaction proposals");
    let mut route_command =
        |command: &bolt_v2::bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
         submit_order_prefix: &str| {
            let proposal = match command {
                bolt_v2::bolt_v3_maker_order_compile::MakerCompiledOrderCommand::Submit {
                    leg: Leg::Yes,
                    ..
                } => quote_set.yes.control.proposal,
                bolt_v2::bolt_v3_maker_order_compile::MakerCompiledOrderCommand::Submit {
                    leg: Leg::No,
                    ..
                } => quote_set.no.control.proposal,
                other => panic!("fresh quote plan emitted non-submit command: {other:?}"),
            }
            .expect("submit command must carry its quote transaction proposal");
            dispatch_maker_order_command(
                MakerOrderDispatchInput {
                    command,
                    submit_order_prefix,
                    authority: MakerOrderCommandAuthority::Quote(
                        MakerQuoteTransactionContext::new(market.clone(), budget.clone(), proposal),
                    ),
                },
                &mut sink,
            )
        };
    let template = maker_limit_post_only_template();
    let dispatched = dispatch_maker_runtime_order_plan_with_command_router(
        MakerRuntimeOrderDispatchInput {
            order_plan,
            submit_template: &template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
        &mut route_command,
    )
    .expect("runtime order plan should dispatch");

    assert_eq!(
        dispatched.yes.dispatch,
        Some(MakerOrderDispatchOutcome::submitted_for_test(
            Leg::Yes,
            InstrumentId::from("YES.RUNTIME"),
            ClientOrderId::from("MAKER-YES-1"),
            Price::new(
                decision
                    .quote_plan
                    .as_ref()
                    .expect("quote plan should exist")
                    .targets
                    .leg_a
                    .price,
                2
            ),
            Quantity::new(2.0, 2),
        ))
    );
    assert_eq!(
        dispatched.no.dispatch,
        Some(MakerOrderDispatchOutcome::submitted_for_test(
            Leg::No,
            InstrumentId::from("NO.RUNTIME"),
            ClientOrderId::from("MAKER-NO-1"),
            Price::new(
                decision
                    .quote_plan
                    .as_ref()
                    .expect("quote plan should exist")
                    .targets
                    .leg_b
                    .price,
                2
            ),
            Quantity::new(3.0, 2),
        ))
    );
    assert_eq!(
        sink.submitted_client_order_ids(),
        vec![
            ClientOrderId::from("MAKER-YES-1"),
            ClientOrderId::from("MAKER-NO-1"),
        ]
    );
}

#[test]
fn maker_command_rejects_leg_instrument_mismatch_before_mutation() {
    let no_instrument_id = InstrumentId::from("NO.RUNTIME");
    let commands = [
        (
            LifecycleAction::Submit,
            MakerCompiledOrderCommand::Submit {
                leg: Leg::Yes,
                template: Box::new(maker_limit_post_only_template()),
                inputs: NtOrderBuildInputs {
                    instrument_id: no_instrument_id,
                    order_side: OrderSide::Buy,
                    quantity: Quantity::new(2.0, 2),
                    price: Some(Price::new(0.40, 2)),
                    client_order_id: ClientOrderId::from("MAKER-CROSS-LEG-SUBMIT"),
                },
                fallback_price: Price::new(0.40, 2),
            },
        ),
        (
            LifecycleAction::Cancel,
            MakerCompiledOrderCommand::Cancel {
                leg: Leg::Yes,
                instrument_id: no_instrument_id,
                client_order_id: ClientOrderId::from("MAKER-CROSS-LEG-CANCEL"),
            },
        ),
        (
            LifecycleAction::Modify,
            MakerCompiledOrderCommand::Modify {
                leg: Leg::Yes,
                instrument_id: no_instrument_id,
                client_order_id: ClientOrderId::from("MAKER-CROSS-LEG-MODIFY"),
                price: Price::new(0.41, 2),
                quantity: Quantity::new(2.0, 2),
            },
        ),
    ];

    for (action, command) in commands {
        let context = quote_transaction_context_for_action(action);
        let mut sink = RecordingMakerOrderSink::new();
        let error = dispatch_maker_order_command(
            MakerOrderDispatchInput {
                command: &command,
                submit_order_prefix: "maker_submit",
                authority: MakerOrderCommandAuthority::Quote(context),
            },
            &mut sink,
        )
        .expect_err("a command cannot bind one leg to the other leg's instrument");

        assert_eq!(error.kind(), MakerOrderCommandFailureKind::LifecycleScope);
        assert_eq!(sink.mutation_counts(), [0; 6]);
    }
}

#[test]
fn runtime_quote_order_plan_reconciles_yes_then_surfaces_no_leg_command_failure() {
    // A two-leg dispatch where the YES leg routes but the NO leg's command router
    // returns an error. Fix F turns that per-leg command failure into data instead of a
    // `?` abort: the dispatcher returns Ok with a partial outcome (YES dispatched, NO
    // carrying its command failure) so the caller can reconcile the YES identity before
    // failing loud, rather than orphaning it. Differential: under the prior `?`-abort
    // behavior the dispatcher returns Err and the `.expect` below panics.
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    );
    let order_plan = decision
        .order_plan
        .as_ref()
        .expect("quote tick should produce maker order intents");

    let template = maker_limit_post_only_template();
    let mut route_calls = 0_u32;
    let dispatched = dispatch_maker_runtime_order_plan_with_command_router(
        MakerRuntimeOrderDispatchInput {
            order_plan,
            submit_template: &template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
        &mut |_command, _submit_order_prefix| {
            route_calls += 1;
            if route_calls == 1 {
                Ok(MakerOrderDispatchOutcome::submitted_for_test(
                    Leg::Yes,
                    InstrumentId::from("YES.RUNTIME"),
                    ClientOrderId::from("MAKER-YES-1"),
                    Price::new(0.40, 2),
                    Quantity::new(2.0, 2),
                ))
            } else {
                Err(MakerOrderCommandFailure::for_test(
                    MakerOrderCommandFailureKind::SubmitPreparation,
                    "simulated NO-leg routing failure",
                ))
            }
        },
    )
    .expect("a per-leg command failure is data, not a dispatcher abort");

    assert_eq!(
        route_calls, 2,
        "YES routes, then NO is attempted because YES had no command failure"
    );
    assert!(
        dispatched.yes.dispatch.is_some(),
        "the YES leg dispatched before the NO failure"
    );
    assert!(
        dispatched.no.dispatch.is_none(),
        "the NO leg did not dispatch"
    );
    assert!(
        dispatched.no.command_failure.is_some(),
        "the NO leg captured its command failure as data"
    );
    assert_eq!(
        dispatched.command_failure(),
        dispatched.no.command_failure.as_ref(),
        "the combined command failure surfaces the NO-leg failure for the caller to fail loud on"
    );
}

#[test]
fn runtime_quote_order_plan_short_circuits_no_leg_when_yes_leg_command_fails() {
    // The mirror of the YES-then-NO reconcile, covering the opposite branch: the YES
    // leg's command router fails on its first (and only) call. Because the YES leg
    // carries a command failure, the dispatcher short-circuits and synthesizes an empty
    // NO leg rather than attempting its route, returning Ok with a partial outcome
    // whose combined command failure surfaces the YES failure for the caller to fail
    // loud on. Differential: if the short-circuit were dropped (the NO leg routed
    // unconditionally) route_calls would be 2; if the YES error were `?`-aborted the
    // `.expect` below would panic.
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let decision = plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    );
    let order_plan = decision
        .order_plan
        .as_ref()
        .expect("quote tick should produce maker order intents");

    let template = maker_limit_post_only_template();
    let mut route_calls = 0_u32;
    let dispatched = dispatch_maker_runtime_order_plan_with_command_router(
        MakerRuntimeOrderDispatchInput {
            order_plan,
            submit_template: &template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
        &mut |_command, _submit_order_prefix| {
            route_calls += 1;
            Err(MakerOrderCommandFailure::for_test(
                MakerOrderCommandFailureKind::SubmitPreparation,
                "simulated YES-leg routing failure",
            ))
        },
    )
    .expect("a per-leg command failure is data, not a dispatcher abort");

    assert_eq!(
        route_calls, 1,
        "the YES routing failure short-circuits the NO leg, which is never attempted"
    );
    assert!(
        dispatched.yes.command_failure.is_some(),
        "the YES leg captured its command failure as data"
    );
    assert!(
        dispatched.no.dispatch.is_none(),
        "the NO leg is synthesized empty when YES fails"
    );
    assert!(
        dispatched.no.command_failure.is_none(),
        "the synthesized NO leg carries no command failure of its own"
    );
    assert_eq!(
        dispatched.command_failure(),
        dispatched.yes.command_failure.as_ref(),
        "the combined command failure surfaces the YES-leg failure for the caller to fail loud on"
    );
}

#[test]
fn canceled_callback_cannot_consume_an_uncommitted_requote_proposal() {
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
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
    let order_plan = decision
        .order_plan
        .as_ref()
        .expect("requote tick should produce maker order intents");
    assert!(matches!(
        order_plan.yes.intent,
        Some(MakerOrderIntent::Cancel { .. })
    ));
    let submit_commands_before_cancel_confirm = budget.submit_commands_in_window();
    let rest_cost_before_cancel_confirm = budget.rest_cost_in_window();

    assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Canceled), None);
    assert_eq!(
        budget.submit_commands_in_window(),
        submit_commands_before_cancel_confirm
    );
    assert_eq!(
        budget.rest_cost_in_window(),
        rest_cost_before_cancel_confirm
    );

    assert_eq!(market.market_state(), MarketState::Idle);
}

#[test]
fn cancel_all_market_action_maps_to_each_bound_leg_instrument() {
    let targets = quote_targets();

    let order_plan = maker_order_plan_from_market_action(MakerMarketActionOrderInput {
        action: MarketAction::CancelAllBothLegs,
        targets,
        yes_quantity: 2.0,
        no_quantity: 3.0,
        yes: MakerLegBinding {
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            active_order: None,
            next_order: None,
        },
        no: MakerLegBinding {
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            active_order: None,
            next_order: None,
        },
    });

    assert_eq!(order_plan.yes.blocked_by, None);
    assert_eq!(order_plan.no.blocked_by, None);
    assert_eq!(
        order_plan.yes.intent,
        Some(MakerOrderIntent::CancelAll {
            leg: Some(Leg::Yes),
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: None,
        })
    );
    assert_eq!(
        order_plan.no.intent,
        Some(MakerOrderIntent::CancelAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            order_side: None,
        })
    );
}

#[test]
fn one_side_cancel_all_market_action_scopes_to_leg_order_side() {
    let targets = quote_targets();

    let order_plan = maker_order_plan_from_market_action(MakerMarketActionOrderInput {
        action: MarketAction::CancelAllOneSide { leg: Leg::No },
        targets,
        yes_quantity: 2.0,
        no_quantity: 3.0,
        yes: MakerLegBinding {
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            active_order: None,
            next_order: None,
        },
        no: MakerLegBinding {
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            active_order: None,
            next_order: None,
        },
    });

    assert_eq!(order_plan.yes.intent, None);
    assert_eq!(order_plan.yes.blocked_by, None);
    assert_eq!(
        order_plan.no.intent,
        Some(MakerOrderIntent::CancelAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            order_side: Some(OrderSide::Buy),
        })
    );
}

#[test]
fn cancel_all_runtime_order_plan_dispatches_both_leg_instruments() {
    let order_plan = maker_order_plan_from_market_action(MakerMarketActionOrderInput {
        action: MarketAction::CancelAllBothLegs,
        targets: quote_targets(),
        yes_quantity: 2.0,
        no_quantity: 3.0,
        yes: MakerLegBinding {
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            active_order: None,
            next_order: None,
        },
        no: MakerLegBinding {
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            active_order: None,
            next_order: None,
        },
    });

    let mut sink = RecordingMakerOrderSink::new();
    let mut route_command =
        |command: &bolt_v2::bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
         submit_order_prefix: &str| {
            dispatch_maker_order_command(
                MakerOrderDispatchInput {
                    command,
                    submit_order_prefix,
                    authority: MakerOrderCommandAuthority::ScopeCancelAll,
                },
                &mut sink,
            )
        };
    let template = maker_limit_post_only_template();
    let dispatched = dispatch_maker_runtime_order_plan_with_command_router(
        MakerRuntimeOrderDispatchInput {
            order_plan: &order_plan,
            submit_template: &template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
        &mut route_command,
    )
    .expect("cancel-all order plan should dispatch");

    assert_eq!(
        dispatched.yes.dispatch,
        Some(MakerOrderDispatchOutcome::CancelScopeHandled {
            leg: Some(Leg::Yes),
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: None,
            dispositions: Vec::new(),
        })
    );
    assert_eq!(
        dispatched.no.dispatch,
        Some(MakerOrderDispatchOutcome::CancelScopeHandled {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            order_side: None,
            dispositions: Vec::new(),
        })
    );
    assert_eq!(
        sink.canceled_all,
        vec![
            (Some(Leg::Yes), InstrumentId::from("YES.RUNTIME"), None),
            (Some(Leg::No), InstrumentId::from("NO.RUNTIME"), None),
        ]
    );
    assert!(sink.submitted.is_empty());
}

fn quote_plan_inputs(family_key: &str) -> MakerQuotePlanInputs<'_> {
    quote_plan_inputs_with_fair(family_key, 0.60)
}

fn quote_plan_inputs_with_fair(
    family_key: &str,
    oracle_fair_probability_up: f64,
) -> MakerQuotePlanInputs<'_> {
    MakerQuotePlanInputs {
        family_key,
        oracle_fair_probability_up,
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

/// Mint the toxicity μ these quote-plan fixtures use (0.10) the only way a
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

fn quote_targets() -> bolt_v2::bolt_v3_quoting::QuoteTargets {
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new_for_test(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    plan_maker_runtime_quote(
        &mut market,
        &mut budget,
        MakerRuntimeQuoteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            order_plan: order_plan_inputs(),
        },
    )
    .quote_plan
    .expect("supported family should produce quote targets")
    .targets
}

fn reference_quote(
    asset: &str,
    source_id: &str,
    price: f64,
    observed_ts_ms: u64,
) -> ReferenceQuote {
    reference_quote_received_at(asset, source_id, price, observed_ts_ms, observed_ts_ms)
}

fn reference_quote_received_at(
    asset: &str,
    source_id: &str,
    price: f64,
    observed_ts_ms: u64,
    received_ts_ms: u64,
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
        received_ts_ms,
    )
    .expect("reference quote fixture should be valid")
}

fn quote_set_inputs() -> MakerRuntimeQuoteSetInput {
    MakerRuntimeQuoteSetInput {
        yes_quantity: 2.0,
        no_quantity: 3.0,
        yes_resting_price: None,
        no_resting_price: None,
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

fn order_identity(client_order_id: &str, generation: u64) -> OrderIdentity {
    OrderIdentity::new(
        MakerClientOrderId::new(client_order_id.to_string()),
        generation,
    )
}

fn quote_transaction_context_for_action(action: LifecycleAction) -> MakerQuoteTransactionContext {
    let supports_modify = action == LifecycleAction::Modify;
    let mut market = MarketQuote::new(
        MakerOrderLifecycleScopeIdentity::new(
            1_000,
            InstrumentId::from("YES.RUNTIME"),
            InstrumentId::from("NO.RUNTIME"),
        ),
        supports_modify,
    );
    if action != LifecycleAction::Submit {
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: true,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
    }
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");
    let decision = drive_quote_leg(
        &mut market,
        &mut budget,
        QuoteControlInput {
            leg: Leg::Yes,
            desired_price: 0.6,
            resting_price: (action != LifecycleAction::Submit).then_some(0.5),
            requote_threshold: 0.01,
            eps: 1e-9,
            now_ms: 1_000,
        },
    );
    assert_eq!(
        decision.action,
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action
        })
    );
    MakerQuoteTransactionContext::new(
        market,
        budget,
        decision
            .proposal
            .expect("requested action must be proposed"),
    )
}

struct RecordingMakerOrderSink {
    clock: Rc<RefCell<dyn Clock>>,
    order_factory: RefCell<OrderFactory>,
    next_generation: u64,
    order_factory_calls: usize,
    prepared_submit_calls: usize,
    cancel_calls: usize,
    modify_calls: usize,
    submitted: Vec<OrderAny>,
    canceled_all: Vec<(Option<Leg>, InstrumentId, Option<OrderSide>)>,
}

impl RecordingMakerOrderSink {
    fn new() -> Self {
        let test_clock = Rc::new(RefCell::new(TestClock::new()));
        test_clock
            .borrow_mut()
            .set_time(UnixNanos::from(1_000_000_000_u64));
        let clock: Rc<RefCell<dyn Clock>> = test_clock;
        Self {
            order_factory: RefCell::new(OrderFactory::new(
                TraderId::new("MAKER-TRADER-001"),
                StrategyId::new("MAKER-RUNTIME-001"),
                None,
                None,
                clock.clone(),
                false,
                true,
            )),
            clock,
            next_generation: 1,
            order_factory_calls: 0,
            prepared_submit_calls: 0,
            cancel_calls: 0,
            modify_calls: 0,
            submitted: Vec::new(),
            canceled_all: Vec::new(),
        }
    }

    fn submitted_client_order_ids(&self) -> Vec<ClientOrderId> {
        self.submitted
            .iter()
            .map(|order| order.client_order_id())
            .collect()
    }

    fn begin_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation += 1;
        generation
    }

    fn mutation_counts(&self) -> [usize; 6] {
        [
            self.order_factory_calls,
            self.prepared_submit_calls,
            self.submitted.len(),
            self.cancel_calls,
            self.modify_calls,
            usize::try_from(self.next_generation - 1).expect("test generation fits usize"),
        ]
    }
}

impl MakerOrderCommandSink for RecordingMakerOrderSink {
    type PreparedSubmit = OrderAny;

    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.order_factory_calls += 1;
        self.order_factory.borrow_mut()
    }

    fn prepare_maker_order(&mut self, order: OrderAny) -> Result<Self::PreparedSubmit> {
        self.prepared_submit_calls += 1;
        Ok(order)
    }

    fn submit_maker_order(
        &mut self,
        order: Self::PreparedSubmit,
        mut participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> BoltV3RestingSubmitTransactionOutcome {
        let instrument_id = order.instrument_id();
        let order_side = order.order_side();
        let price = order
            .price()
            .expect("maker runtime quotes must be priced limit orders");
        let quantity = order.quantity();
        let client_order_id = order.client_order_id();
        let generation = self.begin_generation();
        let actor_now_ns = self.clock.borrow().timestamp_ns().as_u64();
        participant
            .arm_at_identity(MakerQuoteLifecycleIdentity::new(
                client_order_id.as_str(),
                generation,
            ))
            .expect("test submit participant must arm");
        participant
            .preflight_sink_invocation(generation, actor_now_ns)
            .expect("test submit participant must prepare the sink boundary")
            .commit();
        self.submitted.push(order);
        participant
            .settle_submitted(generation)
            .expect("test submit participant must commit");
        BoltV3RestingSubmitTransactionOutcome::submitted_with_linkage_for_test(
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        )
    }

    fn cancel_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        client_order_id: ClientOrderId,
        mut participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> Result<RestingOrderCancelHandled> {
        self.cancel_calls += 1;
        let generation = self.begin_generation();
        let actor_now_ns = self.clock.borrow().timestamp_ns().as_u64();
        participant.arm_at_identity(MakerQuoteLifecycleIdentity::new(
            client_order_id.as_str(),
            generation,
        ))?;
        participant
            .preflight_sink_invocation(generation, actor_now_ns)?
            .commit();
        participant.settle_nt_mutation_invoked(generation)?;
        anyhow::bail!("test sink should not receive cancel commands")
    }

    fn cancel_all_maker_orders(
        &mut self,
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<Vec<RestingOrderCancelHandled>> {
        self.canceled_all.push((leg, instrument_id, order_side));
        Ok(Vec::new())
    }

    fn modify_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        _client_order_id: ClientOrderId,
        _price: Price,
        _quantity: Quantity,
        _participant: Box<dyn BoltV3RestingRegistrationCommitParticipant>,
    ) -> Result<()> {
        self.modify_calls += 1;
        anyhow::bail!("test sink should not receive modify commands")
    }
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
