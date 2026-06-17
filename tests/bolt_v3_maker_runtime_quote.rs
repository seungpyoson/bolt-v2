use anyhow::Result;
use bolt_v2::{
    bolt_v3_config::ReferencePriceProvider,
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_order_dispatch::{MakerOrderCommandSink, MakerOrderDispatchOutcome},
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerMarketActionOrderInput, MakerOrderIntent,
        maker_order_intent_from_market_action, maker_order_plan_from_market_action,
    },
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_rate_budget::build_requote_budget_pair,
    bolt_v3_maker_runtime_order::{
        MakerRuntimeOrderDispatchInput, dispatch_maker_runtime_order_plan,
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
};
use nautilus_common::{
    clock::{Clock, TestClock},
    factories::OrderFactory,
};
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

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
            strike_price: 0.50,
            seconds_to_market_end: 0,
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
        strike_price: 100.0,
        seconds_to_market_end: 300,
        realized_volatility_snapshot: &realized_volatility_snapshot,
        pricing_kurtosis: 0.25,
    };

    let decision = maker_reference_current_price_fair_value_decision(&mut selector, input);

    assert_eq!(decision.blocked_by, None);
    let fair = decision
        .fair_value
        .expect("fresh backup reference current price should price");
    assert_eq!(fair.spot_price, 101.0);
    assert_eq!(fair.strike_price, input.strike_price);
    assert_eq!(fair.seconds_to_market_end, input.seconds_to_market_end);
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
            strike_price: input.strike_price,
            seconds_to_market_end: input.seconds_to_market_end,
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
        informed_fraction: 0.10,
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
    order_factory: OrderFactory,
    submitted: Vec<OrderAny>,
    canceled_all: Vec<(Option<Leg>, InstrumentId, Option<OrderSide>)>,
}

impl RecordingMakerOrderSink {
    fn new() -> Self {
        let clock: Rc<RefCell<dyn Clock>> = Rc::new(RefCell::new(TestClock::new()));
        Self {
            order_factory: OrderFactory::new(
                TraderId::new("MAKER-TRADER-001"),
                StrategyId::new("MAKER-RUNTIME-001"),
                None,
                None,
                clock,
                false,
                true,
            ),
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
    fn order_factory(&mut self) -> &mut OrderFactory {
        &mut self.order_factory
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
