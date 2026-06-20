use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::ReferencePriceProvider,
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_mu_estimator::{MuEstimatorConfig, MuHealthConfig, UsableMu},
    bolt_v3_maker_order_dispatch::{MakerOrderCommandSink, MakerOrderDispatchOutcome},
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerMarketActionOrderInput, MakerOrderIntent,
        maker_order_intent_from_market_action, maker_order_plan_from_market_action,
    },
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_rate_budget::build_requote_budget_pair,
    bolt_v3_maker_runtime_order::{
        MakerRuntimeOrderDispatchInput, dispatch_maker_runtime_order_plan,
        dispatch_maker_runtime_order_plan_with_command_router,
    },
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteBlockReason, MakerRuntimeQuoteInput,
        MakerRuntimeQuoteSetInput, MakerRuntimeReferenceFairValueBlockReason,
        MakerRuntimeReferenceFairValueInput, maker_reference_current_price_fair_value,
        maker_reference_current_price_fair_value_decision, plan_maker_runtime_quote,
    },
    bolt_v3_market_families::{FairProbabilityInputs, static_binary_event, updown},
    bolt_v3_order_intent::NtOrderTemplate,
    bolt_v3_quote_lifecycle::{Leg, LegEvent, LifecycleAction, MarketAction, MarketState},
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
        RealizedVolSnapshot,
    },
    bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote},
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
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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

    assert_eq!(market.market_state(), MarketState::Quoting);
    assert_eq!(budget.submit_commands_in_window(), 2);
    assert_eq!(budget.rest_cost_in_window(), 2);
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
        MakerRuntimeReferenceFairValueInput {
            family_key: static_binary_event::KEY,
            interval_start_ms: 1_000,
            interval_end_ms: 2_000,
            now_ms: 1_000,
            reference_quotes: &quotes,
            strike_price: Some(0.50),
            seconds_to_market_end: Some(0),
            realized_volatility_snapshot: &realized_volatility_snapshot,
            pricing_kurtosis: f64::NAN,
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

    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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
        now_ms: 1_500,
        reference_quotes: &quotes,
        strike_price: Some(100.0),
        seconds_to_market_end: Some(300),
        realized_volatility_snapshot: &realized_volatility_snapshot,
        pricing_kurtosis: 0.25,
    };

    let decision = maker_reference_current_price_fair_value_decision(&mut selector, input);

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
        MakerRuntimeReferenceFairValueInput {
            family_key: updown::KEY,
            interval_start_ms: 1_000,
            interval_end_ms: 2_000,
            now_ms: 1_500,
            reference_quotes: &[],
            strike_price: input.strike_price,
            seconds_to_market_end: input.seconds_to_market_end,
            realized_volatility_snapshot: input.realized_volatility_snapshot,
            pricing_kurtosis: input.pricing_kurtosis,
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
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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
    let dispatched = dispatch_maker_runtime_order_plan(
        MakerRuntimeOrderDispatchInput {
            order_plan,
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
        &mut sink,
    )
    .expect("runtime order plan should dispatch");

    assert_eq!(
        dispatched.yes.dispatch,
        Some(MakerOrderDispatchOutcome::Submitted {
            leg: Leg::Yes,
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-YES-1"),
            price: Price::new(
                decision
                    .quote_plan
                    .as_ref()
                    .expect("quote plan should exist")
                    .targets
                    .leg_a
                    .price,
                2
            ),
            quantity: Quantity::new(2.0, 2),
        })
    );
    assert_eq!(
        dispatched.no.dispatch,
        Some(MakerOrderDispatchOutcome::Submitted {
            leg: Leg::No,
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            client_order_id: ClientOrderId::from("MAKER-NO-1"),
            price: Price::new(
                decision
                    .quote_plan
                    .as_ref()
                    .expect("quote plan should exist")
                    .targets
                    .leg_b
                    .price,
                2
            ),
            quantity: Quantity::new(3.0, 2),
        })
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
fn runtime_quote_order_plan_reconciles_yes_then_surfaces_no_leg_routing_error() {
    // A two-leg dispatch where the YES leg routes but the NO leg's command router
    // returns an error. Fix F turns that per-leg routing error into data instead of a
    // `?` abort: the dispatcher returns Ok with a partial outcome (YES dispatched, NO
    // carrying its routing error) so the caller can reconcile the YES identity before
    // failing loud, rather than orphaning it. Differential: under the prior `?`-abort
    // behavior the dispatcher returns Err and the `.expect` below panics.
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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
                Ok(MakerOrderDispatchOutcome::Submitted {
                    leg: Leg::Yes,
                    instrument_id: InstrumentId::from("YES.RUNTIME"),
                    client_order_id: ClientOrderId::from("MAKER-YES-1"),
                    price: Price::new(0.40, 2),
                    quantity: Quantity::new(2.0, 2),
                })
            } else {
                anyhow::bail!("simulated NO-leg routing failure")
            }
        },
    )
    .expect("a per-leg routing error is data, not a dispatcher abort");

    assert_eq!(
        route_calls, 2,
        "YES routes, then NO is attempted because YES had no routing error"
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
        dispatched.no.routing_error.is_some(),
        "the NO leg captured its routing error as data"
    );
    assert_eq!(
        dispatched.routing_error(),
        dispatched.no.routing_error.as_deref(),
        "the combined routing_error surfaces the NO-leg failure for the caller to fail loud on"
    );
}

#[test]
fn runtime_quote_order_plan_short_circuits_no_leg_when_yes_leg_routing_fails() {
    // The mirror of the YES-then-NO reconcile, covering the opposite branch: the YES
    // leg's command router fails on its first (and only) call. Because the YES leg
    // carries a routing error, the dispatcher short-circuits and synthesizes an empty
    // NO leg rather than attempting its route, returning Ok with a partial outcome
    // whose combined routing_error surfaces the YES failure for the caller to fail
    // loud on. Differential: if the short-circuit were dropped (the NO leg routed
    // unconditionally) route_calls would be 2; if the YES error were `?`-aborted the
    // `.expect` below would panic.
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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
            anyhow::bail!("simulated YES-leg routing failure")
        },
    )
    .expect("a per-leg routing error is data, not a dispatcher abort");

    assert_eq!(
        route_calls, 1,
        "the YES routing failure short-circuits the NO leg, which is never attempted"
    );
    assert!(
        dispatched.yes.routing_error.is_some(),
        "the YES leg captured its routing error as data"
    );
    assert!(
        dispatched.no.dispatch.is_none(),
        "the NO leg is synthesized empty when YES fails"
    );
    assert!(
        dispatched.no.routing_error.is_none(),
        "the synthesized NO leg carries no routing error of its own"
    );
    assert_eq!(
        dispatched.routing_error(),
        dispatched.yes.routing_error.as_deref(),
        "the combined routing_error surfaces the YES-leg failure for the caller to fail loud on"
    );
}

#[test]
fn canceled_requote_action_maps_to_prepaid_replacement_submit_without_budget_charge() {
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

    let action = market
        .on_leg_event(Leg::Yes, LegEvent::Canceled)
        .expect("cancel confirmation should drive prepaid replacement submit");
    assert_eq!(
        budget.submit_commands_in_window(),
        submit_commands_before_cancel_confirm
    );
    assert_eq!(
        budget.rest_cost_in_window(),
        rest_cost_before_cancel_confirm
    );

    let replacement = maker_order_intent_from_market_action(MakerMarketActionOrderInput {
        action,
        targets: decision
            .quote_plan
            .as_ref()
            .expect("quote plan should exist")
            .targets,
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
    });

    match replacement.intent {
        Some(MakerOrderIntent::Submit {
            leg,
            instrument_id,
            order_identity: submitted_identity,
            price,
            quantity,
            ..
        }) => {
            assert_eq!(leg, Leg::Yes);
            assert_eq!(instrument_id, InstrumentId::from("YES.RUNTIME"));
            assert_eq!(submitted_identity, order_identity("MAKER-YES-2", 2));
            assert_eq!(
                price,
                decision
                    .quote_plan
                    .as_ref()
                    .expect("quote plan should exist")
                    .targets
                    .leg_a
                    .price
            );
            assert_eq!(quantity, quote_set.yes_quantity);
        }
        other => panic!("expected prepaid replacement submit intent, got {other:?}"),
    }
    assert_eq!(replacement.blocked_by, None);
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
    let dispatched = dispatch_maker_runtime_order_plan(
        MakerRuntimeOrderDispatchInput {
            order_plan: &order_plan,
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
        &mut sink,
    )
    .expect("cancel-all order plan should dispatch");

    assert_eq!(
        dispatched.yes.dispatch,
        Some(MakerOrderDispatchOutcome::CanceledAll {
            leg: Some(Leg::Yes),
            instrument_id: InstrumentId::from("YES.RUNTIME"),
            order_side: None,
        })
    );
    assert_eq!(
        dispatched.no.dispatch,
        Some(MakerOrderDispatchOutcome::CanceledAll {
            leg: Some(Leg::No),
            instrument_id: InstrumentId::from("NO.RUNTIME"),
            order_side: None,
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
    let mut observe = |state: &mut MakerMuState, aggressor: AggressorSide, ts_ms: u64| {
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
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
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

fn order_identity(client_order_id: &str, generation: u64) -> OrderIdentity {
    OrderIdentity::new(
        MakerClientOrderId::new(client_order_id.to_string()),
        generation,
    )
}

struct RecordingMakerOrderSink {
    order_factory: RefCell<OrderFactory>,
    submitted: Vec<OrderAny>,
    canceled_all: Vec<(Option<Leg>, InstrumentId, Option<OrderSide>)>,
}

impl RecordingMakerOrderSink {
    fn new() -> Self {
        let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
        Self {
            order_factory: RefCell::new(OrderFactory::new(
                TraderId::new("MAKER-TRADER-001"),
                StrategyId::new("MAKER-RUNTIME-001"),
                None,
                None,
                clock,
                false,
                true,
            )),
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
}

impl MakerOrderCommandSink for RecordingMakerOrderSink {
    fn order_factory(&mut self) -> RefMut<'_, OrderFactory> {
        self.order_factory.borrow_mut()
    }

    fn submit_maker_order(&mut self, order: OrderAny) -> Result<()> {
        self.submitted.push(order);
        Ok(())
    }

    fn cancel_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        _client_order_id: ClientOrderId,
    ) -> Result<()> {
        anyhow::bail!("test sink should not receive cancel commands")
    }

    fn cancel_all_maker_orders(
        &mut self,
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    ) -> Result<()> {
        self.canceled_all.push((leg, instrument_id, order_side));
        Ok(())
    }

    fn modify_maker_order(
        &mut self,
        _leg: Leg,
        _instrument_id: InstrumentId,
        _client_order_id: ClientOrderId,
        _price: Price,
        _quantity: Quantity,
    ) -> Result<()> {
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
