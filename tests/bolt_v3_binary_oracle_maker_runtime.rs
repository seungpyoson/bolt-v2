use crate::support;

/// The portfolio market these cycles quote. A family key names a category the
/// portfolio may hold several markets from, so the two are not interchangeable.
const MARKET_KEY: &str = "eth-static-event";

use bolt_v2::{
    bolt_v3_config::ReferencePriceProvider,
    bolt_v3_current_evidence::{
        AdmissionDecisionOutcome, DecisionEvidenceRecorder, EvidenceRequoteLeg,
        RequoteActionCostClass, RequoteThrottleBlockReason, RequoteThrottleBound,
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
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteBlockReason, MakerRuntimeQuoteInput,
        MakerRuntimeQuoteSetInput, MakerRuntimeReferenceFairValueBlockReason,
        plan_maker_runtime_quote,
    },
    bolt_v3_market_families::{
        FairProbabilityInputs, MarketSelectionOutcome, MarketSelectionTarget,
        market_selection_candidate_windows_from_target, static_binary_event, updown,
    },
    bolt_v3_order_execution::{BoltV3OrderExecutionPolicy, BoltV3TerminalValueEntry},
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LifecycleAction, MarketAction, MarketQuote, MarketState,
    },
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
        RealizedVolSnapshot,
    },
    bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote},
    bolt_v3_strategy_context::StrategyBuildContext,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    bolt_v3_timestamp_domain::LocalReceiveMs,
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::binary_oracle_maker::{
        BinaryOracleMaker, BinaryOracleMakerConfig, BinaryOracleMakerLifecycleError,
        BinaryOracleMakerMarketActionRouteInput, BinaryOracleMakerReferenceFairValueInput,
        BinaryOracleMakerRiskRouteInput, BinaryOracleMakerRuntimeQuoteRouteInput,
        BinaryOracleMakerRuntimeReferenceQuoteBlockReason,
        BinaryOracleMakerRuntimeReferenceQuoteRouteInput, BinaryOracleMakerStrikePrice,
        mu::MakerMuState,
    },
};
use nautilus_common::{
    actor::{DataActorNative, registry::try_get_actor_unchecked},
    cache::Cache,
    clock::{Clock, TestClock},
    component::Component,
    enums::Environment,
    msgbus::{self, MessageBus, set_message_bus, switchboard::get_event_order_topic},
    timer::{TimeEvent, TimeEventCallback},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::{
    data::TradeTick,
    enums::{AggressorSide, OrderSide, OrderType, TimeInForce},
    events::OrderEventAny,
    identifiers::{
        AccountId, ClientOrderId, InstrumentId, StrategyId, TradeId, TraderId, Venue, VenueOrderId,
    },
    orders::{LimitOrder, Order, OrderAny, stubs::TestOrderEventStubs},
    types::{Price, Quantity},
};
use nautilus_portfolio::portfolio::Portfolio;
use nautilus_system::{ClockFactory, trader::Trader};
use nautilus_trading::{Strategy, StrategyNative};
use rust_decimal::Decimal;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

const TEST_REFERENCE_ASSET: &str = "ETH";
const TEST_REALIZED_VOL_SURFACE_ID: &str = "maker_reference_surface";
const TEST_REALIZED_VOL_SOURCE_ID: &str = "maker_reference_rv";

fn bound_strike<'a>(
    market_key: &'a str,
    underlying_asset: &'a str,
    interval_start_ms: u64,
    price: f64,
) -> BinaryOracleMakerStrikePrice<'a> {
    BinaryOracleMakerStrikePrice::try_new(market_key, underlying_asset, interval_start_ms, price)
        .expect("strike fixture should be valid")
}

#[test]
fn maker_strike_accepts_a_canonical_market_key_with_internal_whitespace() {
    let strike = BinaryOracleMakerStrikePrice::try_new("eth 1h", "ETH", 1_000, 100.0)
        .expect("stable market identities permit internal whitespace");
    assert_eq!(strike.market_key(), "eth 1h");
}

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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
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
            Some(
                BoltV3TerminalValueEntry::try_new(Decimal::new(9, 1), Decimal::ZERO)
                    .expect("maker terminal value should construct"),
            ),
        )
        .expect("maker submit should route through shared execution context");

    assert_eq!(
        outcome,
        MakerOrderDispatchOutcome::policy_skipped_for_test(
            Leg::Yes,
            InstrumentId::from("YES.RUNTIME"),
            ClientOrderId::from("MAKER-YES-1"),
            Price::new(0.40, 2),
            Quantity::new(2.0, 2),
        )
    );
    assert_eq!(admission.admitted_order_count(), 0);
    assert_eq!(writer.records().len(), 1);
    assert_eq!(writer.records()[0].strategy_id, "maker-strategy");
    assert_eq!(writer.admission_decisions().len(), 1);
    assert_eq!(
        writer.admission_decisions()[0].outcome,
        AdmissionDecisionOutcome::Admitted
    );
    let decisions = writer.admission_decisions();
    let economics = decisions[0]
        .economics
        .as_ref()
        .expect("maker admission evidence must retain its economics lineage");
    assert_eq!(economics.decision_correlation_id, "MAKER-YES-1");
    assert_eq!(economics.core_total, "0");
    assert_eq!(economics.core_net_edge, "1");
    assert_eq!(economics.forecast_net_edge, "1");
    assert!(economics.forecast_complete);
    assert!(economics.missing_forecast_component_ids.is_empty());
    assert_eq!(economics.source_snapshot_ids, vec!["fixture-market-info"]);
    assert_eq!(economics.reservation_basis, "0.8000");
    assert_eq!(economics.full_reservation_liability, "0.8000");
}

#[test]
fn maker_submit_without_terminal_value_fails_before_order_evidence() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
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
            client_order_id: ClientOrderId::from("MAKER-YES-MISSING-VALUE"),
        },
        fallback_price: Price::new(0.40, 2),
    };

    let error = maker
        .route_maker_order_command(&command, "maker_submit", None)
        .expect_err("maker submit without terminal value must fail closed");

    assert!(
        error
            .to_string()
            .contains("terminal-value economics scenario")
    );
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.admission_decisions().is_empty());
}

#[test]
fn maker_runtime_quote_tick_routes_both_legs_through_shared_context_in_shadow() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission.clone());
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("well-formed rate config builds a budget");

    let outcome = maker
        .route_maker_runtime_quote(
            MARKET_KEY,
            &mut market,
            &mut budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote_plan: quote_plan_inputs(static_binary_event::KEY),
                quote_set: quote_set_inputs(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
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
    let yes_dispatch = orders
        .yes
        .dispatch
        .as_ref()
        .expect("the active YES leg should dispatch");
    let no_dispatch = orders
        .no
        .dispatch
        .as_ref()
        .expect("the active NO leg should dispatch");
    let runtime_market = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the routed market remains active");
    let assert_runtime_policy_skip = |dispatch: &MakerOrderDispatchOutcome,
                                      leg: Leg,
                                      price: Price,
                                      quantity: Quantity| {
        let MakerOrderDispatchOutcome::SubmitAttempt {
            leg: dispatched_leg,
            instrument_id,
            prepared_client_order_id,
            price: dispatched_price,
            quantity: dispatched_quantity,
            transaction,
        } = dispatch
        else {
            panic!("expected a runtime submit attempt, got {dispatch:?}");
        };
        assert_eq!(*dispatched_leg, leg);
        assert_eq!(
            *instrument_id,
            runtime_market.leg_binding(leg).instrument_id,
            "the route must target the active runtime binding, never a caller-supplied instrument"
        );
        assert!(runtime_market.leg_binding(leg).active_order.is_none());
        assert_eq!(
            runtime_market
                .leg_binding(leg)
                .next_order
                .as_ref()
                .expect("a policy skip retains the prepared identity")
                .client_order_id()
                .as_str(),
            prepared_client_order_id.as_str()
        );
        assert_eq!(*dispatched_price, price);
        assert_eq!(*dispatched_quantity, quantity);
        assert!(matches!(
            transaction,
            bolt_v2::bolt_v3_order_execution::BoltV3RestingSubmitTransactionOutcome::Attempt(
                outcome
            ) if outcome.kind()
                == bolt_v2::bolt_v3_order_execution::BoltV3SubmitAttemptKind::PolicySkipped
        ));
    };
    assert_runtime_policy_skip(
        yes_dispatch,
        Leg::Yes,
        Price::new(quote_plan.targets.leg_a.price, 2),
        Quantity::new(2.0, 2),
    );
    assert_runtime_policy_skip(
        no_dispatch,
        Leg::No,
        Price::new(quote_plan.targets.leg_b.price, 2),
        Quantity::new(3.0, 2),
    );
    assert_eq!(market.market_state(), MarketState::Quoting);
    assert_eq!(budget.submit_commands_in_window(), 2);
    assert_eq!(budget.rest_cost_in_window(), 2);
    assert_eq!(admission.admitted_order_count(), 0);

    let records = writer.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].strategy_id, "maker-strategy");
    assert_eq!(records[0].instrument_id, RUNTIME_YES_INSTRUMENT);
    assert_eq!(records[1].strategy_id, "maker-strategy");
    assert_eq!(records[1].instrument_id, RUNTIME_NO_INSTRUMENT);
    assert_eq!(writer.admission_decisions().len(), 2);
}

#[test]
fn maker_runtime_quote_records_requote_throttle_once_per_blocked_leg_edge() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission.clone());
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let route_input = || BinaryOracleMakerRuntimeQuoteRouteInput {
        quote_plan: quote_plan_inputs(static_binary_event::KEY),
        quote_set: quote_set_inputs(),
        submit_template: &submit_template,
        price_precision: 2,
        quantity_precision: 2,
        submit_order_prefix: "maker_submit",
    };

    maker
        .route_maker_runtime_quote(MARKET_KEY, &mut market, &mut budget, route_input())
        .expect("first quote cycle should route the granted leg and record the denied leg");
    maker
        .route_maker_runtime_quote(MARKET_KEY, &mut market, &mut budget, route_input())
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
    assert_eq!(
        throttle.market_id.as_deref(),
        Some("market-will-sample-maker-resolve-yes"),
        "evidence must name the resolved Gamma market, not the configured key"
    );
    assert_eq!(throttle.leg, EvidenceRequoteLeg::No);
    assert_eq!(
        throttle.action_cost_class,
        RequoteActionCostClass::FreshSubmit
    );
    assert_eq!(
        throttle.block_reason,
        RequoteThrottleBlockReason::RequoteBudgetExhausted
    );
    assert_eq!(throttle.bound_by, RequoteThrottleBound::SubmitCommandWindow);
    assert_eq!(throttle.submit_commands_in_window, 1);
    assert_eq!(throttle.submit_command_cap, 1);
    assert_eq!(throttle.rest_cost_in_window, 1);
    assert_eq!(throttle.rest_cap_per_minute, 100);
}

#[test]
fn maker_runtime_quote_rejects_an_inactive_market_before_planning() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker =
        BinaryOracleMaker::new(maker_config(), maker_context(writer.recorder(), admission));
    register_maker_for_order_factory(&mut maker);
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");

    let result = maker.route_maker_runtime_quote(
        MARKET_KEY,
        &mut market,
        &mut budget,
        BinaryOracleMakerRuntimeQuoteRouteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        result.is_err(),
        "an inactive market must fail before quote planning"
    );
    assert_eq!(
        market.market_state(),
        MarketState::Idle,
        "ownership failure must not advance the quote FSM"
    );
    assert_eq!(
        budget.submit_commands_in_window(),
        0,
        "ownership failure must not spend submit budget"
    );
    assert_eq!(
        budget.rest_cost_in_window(),
        0,
        "ownership failure must not spend rest budget"
    );
    assert!(
        writer.records().is_empty(),
        "ownership failure must not route order intents"
    );
    assert!(
        writer.requote_throttles().is_empty(),
        "failed ownership validation must not emit evidence"
    );
}

#[test]
fn maker_run_quote_cycle_rejects_an_inactive_market_without_mutation() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
    );
    register_maker_for_order_factory(&mut maker);
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let result = maker.run_quote_cycle(
        MARKET_KEY,
        &mut market,
        &mut budget,
        BinaryOracleMakerQuoteCycleInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &submit_template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        result.is_err(),
        "an inactive market key must remain an authority error at the quote-cycle boundary"
    );
    assert_eq!(maker.runtime().active_market_count(), 0);
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_runtime_quote_rejects_a_family_mismatch_before_planning() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission);
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");

    let result = maker.route_maker_runtime_quote(
        MARKET_KEY,
        &mut market,
        &mut budget,
        BinaryOracleMakerRuntimeQuoteRouteInput {
            quote_plan: quote_plan_inputs(updown::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        result.is_err(),
        "caller family must match the active runtime binding"
    );
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_run_quote_cycle_rejects_a_family_mismatch_before_minting_identities() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission);
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let result = maker.run_quote_cycle(
        MARKET_KEY,
        &mut market,
        &mut budget,
        BinaryOracleMakerQuoteCycleInput {
            quote_plan: quote_plan_inputs(updown::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &submit_template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(result.is_err());
    let runtime = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the runtime market remains active");
    for leg in [Leg::Yes, Leg::No] {
        assert_eq!(runtime.leg_binding(leg).active_order, None);
        assert_eq!(runtime.leg_binding(leg).next_order, None);
    }
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_run_quote_cycle_waits_for_a_preloaded_next_window_without_mutation() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) =
        maker_with_active_next_updown_market(writer.recorder(), admission.clone());
    let runtime_market = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the next updown window is preloaded as an active runtime market");
    assert!(
        runtime_market.start_timestamp_milliseconds() > RUNTIME_NOW_MS,
        "the selected market must genuinely be a future window"
    );
    assert!(
        runtime_market.expiration_timestamp_milliseconds()
            > runtime_market.start_timestamp_milliseconds(),
        "the future fixture must have a valid half-open quote window"
    );

    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");
    let submit_template = maker_limit_post_only_template();
    let result = maker.run_quote_cycle(
        MARKET_KEY,
        &mut market,
        &mut budget,
        BinaryOracleMakerQuoteCycleInput {
            quote_plan: quote_plan_inputs(updown::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &submit_template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        matches!(result, Ok(None)),
        "a legitimately preloaded future market is active but not yet quotable: {result:?}"
    );
    let runtime_market = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("waiting must keep the preloaded market active");
    for leg in [Leg::Yes, Leg::No] {
        assert_eq!(runtime_market.leg_binding(leg).active_order, None);
        assert_eq!(runtime_market.leg_binding(leg).next_order, None);
    }
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert_eq!(admission.admitted_order_count(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_runtime_quote_blocks_timestamps_outside_the_active_window_before_mutation() {
    for at_or_after_end in [false, true] {
        let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
        let (mut maker, _cache) =
            maker_with_active_static_market(writer.recorder(), admission.clone());
        let (interval_start_ms, interval_end_ms) = {
            let runtime_market = maker
                .runtime()
                .market(MARKET_KEY)
                .expect("the static runtime market is active");
            (
                runtime_market.start_timestamp_milliseconds(),
                runtime_market.expiration_timestamp_milliseconds(),
            )
        };
        assert!(
            interval_start_ms > 0,
            "fixture supports a pre-window timestamp"
        );
        let mut quote_set = quote_set_inputs();
        quote_set.now_ms = if at_or_after_end {
            interval_end_ms
        } else {
            interval_start_ms - 1
        };
        let mut market = MarketQuote::new(false);
        let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
            .expect("ample requote budget fixture builds");

        let outcome = maker
            .route_maker_runtime_quote(
                MARKET_KEY,
                &mut market,
                &mut budget,
                BinaryOracleMakerRuntimeQuoteRouteInput {
                    quote_plan: quote_plan_inputs(static_binary_event::KEY),
                    quote_set,
                    submit_template: &maker_limit_post_only_template(),
                    price_precision: 2,
                    quantity_precision: 2,
                    submit_order_prefix: "maker_submit",
                },
            )
            .expect("an unavailable runtime window is a normal blocked quote state");

        assert!(
            outcome.quote.quote_plan.is_none(),
            "an unavailable window must stop before quote planning"
        );
        assert!(outcome.quote.quote_set.is_none());
        assert!(outcome.quote.order_plan.is_none());
        assert_eq!(
            outcome.quote.blocked_by,
            Some(MakerRuntimeQuoteBlockReason::RuntimeWindowUnavailable)
        );
        assert!(outcome.orders.is_none());
        let runtime_market = maker
            .runtime()
            .market(MARKET_KEY)
            .expect("the rejected market remains active");
        for leg in [Leg::Yes, Leg::No] {
            assert_eq!(runtime_market.leg_binding(leg).active_order, None);
            assert_eq!(runtime_market.leg_binding(leg).next_order, None);
        }
        assert_eq!(market.market_state(), MarketState::Idle);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(writer.records().is_empty());
        assert!(writer.requote_throttles().is_empty());
    }
}

#[test]
fn maker_run_quote_cycle_waits_for_timestamps_outside_the_active_window_before_mutation() {
    for at_or_after_end in [false, true] {
        let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
        let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
        let (mut maker, _cache) =
            maker_with_active_static_market(writer.recorder(), admission.clone());
        let (interval_start_ms, interval_end_ms) = {
            let runtime_market = maker
                .runtime()
                .market(MARKET_KEY)
                .expect("the static runtime market is active");
            (
                runtime_market.start_timestamp_milliseconds(),
                runtime_market.expiration_timestamp_milliseconds(),
            )
        };
        assert!(
            interval_start_ms > 0,
            "fixture supports a pre-window timestamp"
        );
        let mut quote_set = quote_set_inputs();
        quote_set.now_ms = if at_or_after_end {
            interval_end_ms
        } else {
            interval_start_ms - 1
        };
        let mut market = MarketQuote::new(false);
        let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
            .expect("ample requote budget fixture builds");
        let submit_template = maker_limit_post_only_template();

        let result = maker.run_quote_cycle(
            MARKET_KEY,
            &mut market,
            &mut budget,
            BinaryOracleMakerQuoteCycleInput {
                quote_plan: quote_plan_inputs(static_binary_event::KEY),
                quote_set,
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        );

        assert!(
            matches!(result, Ok(None)),
            "an unavailable cadence window must wait without routing: {result:?}"
        );
        let runtime_market = maker
            .runtime()
            .market(MARKET_KEY)
            .expect("the rejected market remains active");
        for leg in [Leg::Yes, Leg::No] {
            assert_eq!(runtime_market.leg_binding(leg).active_order, None);
            assert_eq!(runtime_market.leg_binding(leg).next_order, None);
        }
        assert_eq!(market.market_state(), MarketState::Idle);
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(admission.admitted_order_count(), 0);
        assert!(writer.records().is_empty());
        assert!(writer.requote_throttles().is_empty());
    }
}

#[test]
fn maker_runtime_reference_quote_rejects_an_inactive_market_before_fair_value() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker =
        BinaryOracleMaker::new(maker_config(), maker_context(writer.recorder(), admission));
    register_maker_for_order_factory(&mut maker);
    let realized_volatility_snapshot = ready_realized_vol_snapshot(1_400, 1.5);
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");

    let result = maker.route_maker_runtime_reference_quote(
        MARKET_KEY,
        &mut market,
        &mut budget,
        &mut selector,
        BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
            reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                reference_quotes: &[],
                strike: Some(bound_strike(MARKET_KEY, TEST_REFERENCE_ASSET, 1_000, 100.0)),
                realized_volatility_snapshot: &realized_volatility_snapshot,
                realized_volatility_max_source_age_ms: None,
                pricing_kurtosis: 0.25,
                evaluation_receive_ms: LocalReceiveMs::new(1_500),
            },
            quote_plan: quote_plan_inputs(updown::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        result.is_err(),
        "inactive ownership must fail before even a no-fair-value decision"
    );
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_runtime_reference_quote_rejects_a_family_mismatch_before_fair_value() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission);
    let quotes = [reference_quote(
        TEST_REFERENCE_ASSET,
        "primary",
        100.05,
        1_490,
    )];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(1_400, 1.5);
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");

    let result = maker.route_maker_runtime_reference_quote(
        MARKET_KEY,
        &mut market,
        &mut budget,
        &mut selector,
        BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
            reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                reference_quotes: &quotes,
                strike: Some(bound_strike(MARKET_KEY, TEST_REFERENCE_ASSET, 1_000, 100.0)),
                realized_volatility_snapshot: &realized_volatility_snapshot,
                realized_volatility_max_source_age_ms: None,
                pricing_kurtosis: 0.25,
                evaluation_receive_ms: LocalReceiveMs::new(1_500),
            },
            quote_plan: quote_plan_inputs(updown::KEY),
            quote_set: quote_set_inputs(),
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        result.is_err(),
        "reference pricing must not use a caller family that disagrees with the runtime"
    );
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_runtime_reference_quote_rejects_a_selector_for_another_asset() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_updown_market(writer.recorder(), admission);
    let interval_start_ms = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the updown runtime market is active")
        .start_timestamp_milliseconds();
    let quotes = [reference_quote(
        "BTC",
        "primary",
        100.05,
        RUNTIME_NOW_MS - 10,
    )];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(RUNTIME_NOW_MS - 100, 1.5);
    let mut selector = ReferencePriceSelector::new("BTC", vec!["primary".to_string()], 1, 100, 25)
        .expect("foreign selector fixture should be valid");
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");
    let mut quote_set = quote_set_inputs();
    quote_set.now_ms = RUNTIME_NOW_MS;

    let result = maker.route_maker_runtime_reference_quote(
        MARKET_KEY,
        &mut market,
        &mut budget,
        &mut selector,
        BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
            reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                reference_quotes: &quotes,
                strike: Some(bound_strike(
                    MARKET_KEY,
                    RUNTIME_UPDOWN_ASSET,
                    interval_start_ms,
                    100.0,
                )),
                realized_volatility_snapshot: &realized_volatility_snapshot,
                realized_volatility_max_source_age_ms: None,
                pricing_kurtosis: 0.25,
                evaluation_receive_ms: LocalReceiveMs::new(RUNTIME_NOW_MS),
            },
            quote_plan: quote_plan_inputs(updown::KEY),
            quote_set,
            submit_template: &maker_limit_post_only_template(),
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        },
    );

    assert!(
        result.is_err(),
        "reference selector asset must match the active runtime market"
    );
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_runtime_reference_quote_rejects_a_strike_from_another_market_asset_or_window() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_updown_market(writer.recorder(), admission);
    let runtime_market = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the updown runtime market is active");
    let interval_start_ms = runtime_market.start_timestamp_milliseconds();
    let quotes = [reference_quote(
        TEST_REFERENCE_ASSET,
        "primary",
        100.05,
        RUNTIME_NOW_MS - 10,
    )];
    let foreign_strikes = [
        (
            "market",
            bound_strike(
                "other-market",
                RUNTIME_UPDOWN_ASSET,
                interval_start_ms,
                100.0,
            ),
        ),
        (
            "asset",
            bound_strike(MARKET_KEY, "BTC", interval_start_ms, 100.0),
        ),
        (
            "window",
            bound_strike(
                MARKET_KEY,
                RUNTIME_UPDOWN_ASSET,
                interval_start_ms + 1,
                100.0,
            ),
        ),
    ];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(RUNTIME_NOW_MS - 100, 1.5);
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");
    for (mismatch, foreign_strike) in foreign_strikes {
        let mut quote_set = quote_set_inputs();
        quote_set.now_ms = RUNTIME_NOW_MS;
        let error = maker
            .route_maker_runtime_reference_quote(
                MARKET_KEY,
                &mut market,
                &mut budget,
                &mut selector,
                BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                    reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                        reference_quotes: &quotes,
                        strike: Some(foreign_strike),
                        realized_volatility_snapshot: &realized_volatility_snapshot,
                        realized_volatility_max_source_age_ms: None,
                        pricing_kurtosis: 0.25,
                        evaluation_receive_ms: LocalReceiveMs::new(RUNTIME_NOW_MS),
                    },
                    quote_plan: quote_plan_inputs(updown::KEY),
                    quote_set,
                    submit_template: &maker_limit_post_only_template(),
                    price_precision: 2,
                    quantity_precision: 2,
                    submit_order_prefix: "maker_submit",
                },
            )
            .expect_err("a foreign strike must fail before fair-value pricing");
        assert!(
            error.to_string().contains(mismatch),
            "the failure must identify the mismatched strike field: {error:#}"
        );
    }

    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert!(writer.records().is_empty());
    assert!(writer.requote_throttles().is_empty());
}

#[test]
fn maker_runtime_reference_quote_blocks_an_unavailable_window_without_touching_the_selector() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_updown_market(writer.recorder(), admission.clone());
    record_one_budget_block_for_family(&mut maker, MARKET_KEY, updown::KEY);
    let records_before_wait = writer.records().len();
    let throttles_before_wait = writer.requote_throttles().len();
    let admissions_before_wait = admission.admitted_order_count();
    assert_eq!(throttles_before_wait, 1, "the test must seed one episode");
    let runtime_market = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the updown runtime market is active");
    let interval_start_ms = runtime_market.start_timestamp_milliseconds();
    let interval_end_ms = runtime_market.expiration_timestamp_milliseconds();
    let quotes = [
        reference_quote(
            TEST_REFERENCE_ASSET,
            "primary",
            100.05,
            interval_end_ms - 10,
        ),
        reference_quote(TEST_REFERENCE_ASSET, "backup", 101.05, interval_end_ms - 10),
    ];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(interval_end_ms - 100, 1.5);
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("40/00:01:00", 100, 500)
        .expect("ample requote budget fixture builds");
    let mut quote_set = quote_set_inputs();
    quote_set.now_ms = interval_end_ms;
    assert_eq!(selector.last_cross_source_drift_bps(), None);

    let outcome = maker
        .route_maker_runtime_reference_quote(
            MARKET_KEY,
            &mut market,
            &mut budget,
            &mut selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                    reference_quotes: &quotes,
                    strike: Some(bound_strike(
                        MARKET_KEY,
                        RUNTIME_UPDOWN_ASSET,
                        interval_start_ms,
                        100.0,
                    )),
                    realized_volatility_snapshot: &realized_volatility_snapshot,
                    realized_volatility_max_source_age_ms: None,
                    pricing_kurtosis: 0.25,
                    evaluation_receive_ms: LocalReceiveMs::new(interval_end_ms),
                },
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set,
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("an unavailable runtime window is a normal blocked reference state");

    assert!(
        outcome.fair_value.fair_value.is_none(),
        "an unavailable window must stop before fair-value pricing"
    );
    assert_eq!(
        outcome.fair_value.blocked_by,
        Some(MakerRuntimeReferenceFairValueBlockReason::RuntimeWindowUnavailable)
    );
    assert!(outcome.quote.is_none());
    assert!(outcome.orders.is_none());
    assert_eq!(
        outcome.blocked_by,
        Some(
            BinaryOracleMakerRuntimeReferenceQuoteBlockReason::FairValue(
                MakerRuntimeReferenceFairValueBlockReason::RuntimeWindowUnavailable,
            )
        )
    );
    assert_eq!(
        selector.last_cross_source_drift_bps(),
        None,
        "the two live quotes would set drift if the selector were evaluated"
    );
    assert_eq!(market.market_state(), MarketState::Idle);
    assert_eq!(budget.submit_commands_in_window(), 0);
    assert_eq!(budget.rest_cost_in_window(), 0);
    assert_eq!(writer.records().len(), records_before_wait);
    assert_eq!(writer.requote_throttles().len(), throttles_before_wait);
    assert_eq!(admission.admitted_order_count(), admissions_before_wait);

    record_one_budget_block_for_family(&mut maker, MARKET_KEY, updown::KEY);
    assert_eq!(
        writer.requote_throttles().len(),
        throttles_before_wait,
        "waiting must preserve the existing episode rather than clear and re-emit it"
    );
}

/// A blocked leg whose *observation* alternates while its blocked state does not
/// must still emit one record.
///
/// `bound_by` is computed from `now_ms` against the budget window, so walking the
/// clock backwards and forwards flips it between `SubmitCommandWindow` and
/// `OutOfOrderTs` without anything about the block changing. While `bound_by` was
/// part of the dedupe identity and only the newest identity per leg was kept,
/// each flip missed the previous entry and re-emitted -- so N alternations wrote
/// N records, and a leg that oscillated wrote on every tick. That is the flooding
/// class this evidence path exists to avoid.
#[test]
fn maker_runtime_quote_records_one_throttle_while_the_bound_oscillates() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission.clone());
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let route_input = |now_ms: u64| {
        let mut quote_set = quote_set_inputs();
        quote_set.now_ms = now_ms;
        BinaryOracleMakerRuntimeQuoteRouteInput {
            quote_plan: quote_plan_inputs(static_binary_event::KEY),
            quote_set,
            submit_template: &submit_template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        }
    };

    // Forward, backward, forward, backward: four evaluations of one unchanged
    // blocked leg, alternating only in what the clock says.
    for now_ms in [
        RUNTIME_NOW_MS,
        RUNTIME_NOW_MS - 500,
        RUNTIME_NOW_MS,
        RUNTIME_NOW_MS - 500,
    ] {
        maker
            .route_maker_runtime_quote(MARKET_KEY, &mut market, &mut budget, route_input(now_ms))
            .expect("an oscillating bound must not fail the quote route");
    }

    let throttles = writer.requote_throttles();
    assert_eq!(
        throttles.len(),
        1,
        "an oscillating bound is one blocked episode, not one record per \
         alternation: {throttles:#?}"
    );
    assert_eq!(
        throttles[0].block_reason,
        RequoteThrottleBlockReason::RequoteBudgetExhausted
    );
}

/// Two markets the portfolio quotes from one family are two episodes.
///
/// A family key names a category -- `updown`, `static_binary_event` -- and the
/// portfolio may hold several markets from it at once; only `market_key` is
/// validated unique. While the episode identity was keyed by family, both
/// markets were one episode: the second to block matched the first and emitted
/// nothing, so an operator saw one throttled market where there were two.
#[test]
fn maker_runtime_quote_records_each_market_in_a_shared_family() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut instruments = runtime_static_instruments();
    instruments.extend(runtime_second_static_instruments());
    let (mut maker, _cache) = maker_with_active_markets(
        writer.recorder(),
        admission.clone(),
        vec![
            runtime_static_declaration(),
            runtime_second_static_declaration(),
        ],
        instruments,
    );
    let submit_template = maker_limit_post_only_template();

    // One budget each, because the throttle is per market.
    for market_key in [RUNTIME_MARKET_KEY, RUNTIME_SECOND_MARKET_KEY] {
        let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
        let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
            .expect("one-submit budget fixture builds");
        maker
            .route_maker_runtime_quote(
                market_key,
                &mut market,
                &mut budget,
                BinaryOracleMakerRuntimeQuoteRouteInput {
                    quote_plan: quote_plan_inputs(static_binary_event::KEY),
                    quote_set: quote_set_inputs(),
                    submit_template: &submit_template,
                    price_precision: 2,
                    quantity_precision: 2,
                    submit_order_prefix: "maker_submit",
                },
            )
            .expect("a second market in the same family must not fail the quote route");
    }

    let throttles = writer.requote_throttles();
    let mut markets: Vec<_> = throttles
        .iter()
        .filter_map(|throttle| throttle.market_id.clone())
        .collect();
    markets.sort();
    markets.dedup();
    assert_eq!(
        markets,
        vec![
            "market-will-sample-maker-resolve-yes".to_string(),
            "market-will-sample-second-maker-resolve-yes".to_string(),
        ],
        "each blocked market is its own episode and names itself: {throttles:#?}"
    );
    assert!(
        throttles
            .iter()
            .all(|throttle| throttle.family_key == static_binary_event::KEY),
        "the family stays on the record as the category it is: {throttles:#?}"
    );
}

#[test]
fn maker_runtime_quote_records_a_cadence_only_successor_after_refresh() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, cache) = maker_with_active_static_market(writer.recorder(), admission);
    let before = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("initial cadence market is active");
    let predecessor_identity = before.concrete_identity();
    let predecessor_yes = before.leg_binding(Leg::Yes).instrument_id;
    let predecessor_no = before.leg_binding(Leg::No).instrument_id;
    record_one_budget_block(&mut maker, RUNTIME_MARKET_KEY);

    refresh_maker_instruments(&mut maker, &cache, runtime_static_instruments_rolled());

    let successor = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("rolled cadence market is active");
    let successor_identity = successor.concrete_identity();
    assert_ne!(successor_identity, predecessor_identity);
    assert_eq!(
        successor.leg_binding(Leg::Yes).instrument_id,
        predecessor_yes
    );
    assert_eq!(successor.leg_binding(Leg::No).instrument_id, predecessor_no);
    assert_eq!(
        successor_identity.evidence_identity(),
        predecessor_identity.evidence_identity(),
        "the venue evidence identity must stay fixed while only the cadence start changes"
    );
    assert_eq!(
        successor_identity.gamma_market_id(),
        predecessor_identity.gamma_market_id(),
        "the cadence start is the only concrete-identity discriminator in this fixture"
    );

    record_one_budget_block(&mut maker, RUNTIME_MARKET_KEY);
    assert_eq!(
        writer.requote_throttles().len(),
        2,
        "a new cadence start must not inherit the predecessor's throttle episode"
    );
}

#[test]
fn maker_runtime_quote_records_an_instrument_only_successor_after_refresh() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, cache) = maker_with_active_static_market(writer.recorder(), admission);
    let before = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("initial instrument market is active");
    let predecessor_identity = before.concrete_identity();
    let predecessor_yes = before.leg_binding(Leg::Yes).instrument_id;
    let predecessor_no = before.leg_binding(Leg::No).instrument_id;
    record_one_budget_block(&mut maker, RUNTIME_MARKET_KEY);

    refresh_maker_instruments(
        &mut maker,
        &cache,
        runtime_static_instruments_reissued_yes(),
    );

    let successor = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("reissued-instrument market is active");
    let successor_identity = successor.concrete_identity();
    assert_ne!(successor_identity, predecessor_identity);
    assert_ne!(
        successor.leg_binding(Leg::Yes).instrument_id,
        predecessor_yes
    );
    assert_eq!(successor.leg_binding(Leg::No).instrument_id, predecessor_no);
    assert_eq!(
        successor_identity.evidence_identity(),
        predecessor_identity.evidence_identity(),
        "the venue evidence identity must stay fixed while only the internal YES instrument changes"
    );
    assert_eq!(
        successor_identity.gamma_market_id(),
        predecessor_identity.gamma_market_id(),
        "the YES instrument is the only concrete-identity discriminator in this fixture"
    );

    record_one_budget_block(&mut maker, RUNTIME_MARKET_KEY);
    assert_eq!(
        writer.requote_throttles().len(),
        2,
        "a same-window instrument reissue must not inherit the predecessor's throttle episode"
    );
}

#[test]
fn maker_runtime_metadata_identity_round_trip_prunes_each_predecessor_episode() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, cache) = maker_with_active_static_market(writer.recorder(), admission);
    let identity_a = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("initial metadata identity is active")
        .concrete_identity();
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    maker
        .run_quote_cycle(
            RUNTIME_MARKET_KEY,
            &mut market,
            &mut budget,
            BinaryOracleMakerQuoteCycleInput {
                quote_plan: quote_plan_inputs(RUNTIME_STATIC_FAMILY),
                quote_set: quote_set_inputs(),
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("the first identity routes")
        .expect("the first identity is active");
    assert_eq!(writer.requote_throttles().len(), 1);
    let before_correction = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the first identity remains active");
    let yes_next = before_correction
        .leg_binding(Leg::Yes)
        .next_order
        .clone()
        .expect("the policy-skipped YES leg retains its pending handle");
    let no_next = before_correction
        .leg_binding(Leg::No)
        .next_order
        .clone()
        .expect("the throttled NO leg retains its pending handle");

    refresh_maker_instruments(
        &mut maker,
        &cache,
        runtime_static_instruments_with_question_id("question-corrected"),
    );
    let identity_b = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the corrected metadata identity is active")
        .concrete_identity();
    assert_ne!(identity_b, identity_a);
    assert_eq!(
        maker
            .runtime()
            .market(RUNTIME_MARKET_KEY)
            .expect("the corrected identity is active")
            .leg_binding(Leg::Yes)
            .next_order,
        Some(yes_next.clone())
    );
    assert_eq!(
        maker
            .runtime()
            .market(RUNTIME_MARKET_KEY)
            .expect("the corrected identity is active")
            .leg_binding(Leg::No)
            .next_order,
        Some(no_next.clone())
    );
    maker
        .run_quote_cycle(
            RUNTIME_MARKET_KEY,
            &mut market,
            &mut budget,
            BinaryOracleMakerQuoteCycleInput {
                quote_plan: quote_plan_inputs(RUNTIME_STATIC_FAMILY),
                quote_set: quote_set_inputs(),
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("the corrected identity routes with the retained quote state")
        .expect("the corrected identity remains active");
    assert_eq!(writer.requote_throttles().len(), 2);
    let after_b = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the corrected identity remains active");
    let yes_after_b = after_b.leg_binding(Leg::Yes).next_order.clone();
    let no_after_b = after_b.leg_binding(Leg::No).next_order.clone();

    refresh_maker_instruments(&mut maker, &cache, runtime_static_instruments());
    let reverted = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the original metadata identity is active again");
    assert_eq!(reverted.concrete_identity(), identity_a);
    assert_eq!(reverted.leg_binding(Leg::Yes).next_order, yes_after_b);
    assert_eq!(reverted.leg_binding(Leg::No).next_order, no_after_b);
    maker
        .run_quote_cycle(
            RUNTIME_MARKET_KEY,
            &mut market,
            &mut budget,
            BinaryOracleMakerQuoteCycleInput {
                quote_plan: quote_plan_inputs(RUNTIME_STATIC_FAMILY),
                quote_set: quote_set_inputs(),
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("the reverted identity routes with the retained quote state")
        .expect("the reverted identity remains active");
    assert_eq!(
        writer.requote_throttles().len(),
        3,
        "A -> B -> A must record three genuine episodes; retaining by market key alone suppresses the second A"
    );
}

/// A leg that blocks, spends a cycle with nothing to quote, then blocks again is
/// two episodes.
///
/// At the position cap the planner returns no quote set at all, so no leg is
/// evaluated for a throttle. While the episode clear was gated on there being a
/// quote set, that cycle left the first episode standing and the second block
/// deduped against it -- silence exactly where a fresh block should be reported,
/// which is the mirror of the flooding this dedupe exists to prevent.
#[test]
fn maker_runtime_quote_records_a_second_block_across_a_cycle_with_no_quote_set() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_static_market(writer.recorder(), admission.clone());
    let mut market = bolt_v2::bolt_v3_quote_lifecycle::MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();

    let route_input = |at_cap: bool| {
        let mut quote_plan = quote_plan_inputs(static_binary_event::KEY);
        if at_cap {
            quote_plan.net_position = quote_plan.position_cap;
        }
        BinaryOracleMakerRuntimeQuoteRouteInput {
            quote_plan,
            quote_set: quote_set_inputs(),
            submit_template: &submit_template,
            price_precision: 2,
            quantity_precision: 2,
            submit_order_prefix: "maker_submit",
        }
    };

    maker
        .route_maker_runtime_quote(MARKET_KEY, &mut market, &mut budget, route_input(false))
        .expect("the first blocked cycle routes");
    let at_cap = maker
        .route_maker_runtime_quote(MARKET_KEY, &mut market, &mut budget, route_input(true))
        .expect("the capped cycle routes");
    // Pin the mechanism rather than trusting the fixture: if this cycle still
    // planned a quote set, the assertion below would pass for another reason.
    assert!(
        at_cap.quote.quote_set.is_none(),
        "at the position cap the planner must produce no quote set: {:#?}",
        at_cap.quote
    );
    maker
        .route_maker_runtime_quote(MARKET_KEY, &mut market, &mut budget, route_input(false))
        .expect("the re-blocked cycle routes");

    // The `yes` leg spends the single submit this budget allows, so `no` is the
    // leg the throttle blocks.
    let throttles = writer.requote_throttles();
    let blocked_records = throttles
        .iter()
        .filter(|throttle| throttle.leg == EvidenceRequoteLeg::No)
        .count();
    assert_eq!(
        blocked_records, 2,
        "a block, a cycle with nothing to quote, and a block again is two \
         episodes, not one: {throttles:#?}"
    );
}

#[test]
fn maker_runtime_reference_quote_route_uses_shared_fair_value_inputs_and_blocks_before_quote() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let (mut maker, _cache) = maker_with_active_updown_market(writer.recorder(), admission.clone());
    let runtime_market = maker
        .runtime()
        .market(MARKET_KEY)
        .expect("the updown runtime market is active");
    let interval_start_ms = runtime_market.start_timestamp_milliseconds();
    let interval_end_ms = runtime_market.expiration_timestamp_milliseconds();
    let expected_seconds_to_market_end = interval_end_ms.saturating_sub(RUNTIME_NOW_MS) / 1_000;
    let quotes = vec![
        reference_quote(TEST_REFERENCE_ASSET, "primary", 99.0, interval_start_ms),
        reference_quote(TEST_REFERENCE_ASSET, "backup", 100.05, RUNTIME_NOW_MS - 10),
    ];
    let realized_volatility_snapshot = ready_realized_vol_snapshot(RUNTIME_NOW_MS - 100, 1.5);
    let mut selector = ReferencePriceSelector::new(
        TEST_REFERENCE_ASSET,
        vec!["primary".to_string(), "backup".to_string()],
        1,
        100,
        25,
    )
    .expect("selector fixture should be valid");
    let fair_input = BinaryOracleMakerReferenceFairValueInput {
        reference_quotes: &quotes,
        strike: Some(bound_strike(
            MARKET_KEY,
            RUNTIME_UPDOWN_ASSET,
            interval_start_ms,
            100.0,
        )),
        realized_volatility_snapshot: &realized_volatility_snapshot,
        realized_volatility_max_source_age_ms: None,
        pricing_kurtosis: 0.25,
        evaluation_receive_ms: LocalReceiveMs::new(RUNTIME_NOW_MS),
    };
    let quote_set_at_reference_evaluation = || {
        let mut quote_set = quote_set_inputs();
        quote_set.now_ms = RUNTIME_NOW_MS;
        quote_set
    };
    let expected_fair_probability_up = updown::fair_probability_up(&FairProbabilityInputs {
        spot_price: 100.05,
        strike_price: fair_input.strike.expect("fixture strike").price(),
        seconds_to_market_end: expected_seconds_to_market_end,
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
            MARKET_KEY,
            &mut market,
            &mut budget,
            &mut selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: fair_input,
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set: quote_set_at_reference_evaluation(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
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
        fair_input.strike.expect("fixture strike").price()
    );
    assert_eq!(fair.seconds_to_market_end, expected_seconds_to_market_end);
    assert_eq!(fair.realized_vol, 1.5);
    assert_eq!(fair.pricing_kurtosis, fair_input.pricing_kurtosis);
    assert_eq!(fair.reference_current_price, 100.05);
    assert_eq!(fair.source_id, "backup");
    assert_eq!(fair.reference_current_price_source_id, "backup");
    assert_eq!(
        fair.reference_current_price_observed_ts_ms,
        RUNTIME_NOW_MS - 10
    );
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
    assert_eq!(fair.realized_vol_source_ts_ms, Some(RUNTIME_NOW_MS - 100));
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

    let blocked_writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let blocked_admission = Arc::new(BoltV3SubmitAdmissionState::new(blocked_writer.recorder()));
    let (mut blocked_maker, _cache) =
        maker_with_active_updown_market(blocked_writer.recorder(), blocked_admission);
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
            MARKET_KEY,
            &mut blocked_market,
            &mut blocked_budget,
            &mut blocked_selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                    reference_quotes: &[],
                    ..fair_input
                },
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set: quote_set_at_reference_evaluation(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
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

    let missing_input_writer =
        support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let missing_input_admission = Arc::new(BoltV3SubmitAdmissionState::new(
        missing_input_writer.recorder(),
    ));
    let (mut missing_input_maker, _cache) =
        maker_with_active_updown_market(missing_input_writer.recorder(), missing_input_admission);
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
    let expected_blocker = MakerRuntimeReferenceFairValueBlockReason::StrikePriceMissing;

    let missing_input = missing_input_maker
        .route_maker_runtime_reference_quote(
            MARKET_KEY,
            &mut missing_input_market,
            &mut missing_input_budget,
            &mut missing_input_selector,
            BinaryOracleMakerRuntimeReferenceQuoteRouteInput {
                reference_fair_value: BinaryOracleMakerReferenceFairValueInput {
                    strike: None,
                    ..fair_input
                },
                quote_plan: quote_plan_inputs(updown::KEY),
                quote_set: quote_set_at_reference_evaluation(),
                submit_template: &maker_limit_post_only_template(),
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
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

#[test]
fn maker_canceled_confirmation_routes_prepaid_replacement_submit_in_shadow() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
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
            terminal_value_entry: Some(
                BoltV3TerminalValueEntry::try_new(Decimal::ONE, Decimal::ZERO)
                    .expect("maker terminal value should construct"),
            ),
        })
        .expect("maker should route pre-paid replacement submit through shared context");

    assert_eq!(
        outcome.order.dispatch,
        Some(MakerOrderDispatchOutcome::policy_skipped_for_test(
            Leg::Yes,
            InstrumentId::from("YES.RUNTIME"),
            ClientOrderId::from("MAKER-YES-2"),
            Price::new(targets.leg_a.price, 2),
            Quantity::new(quote_set.yes_quantity, 2),
        ))
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config(),
        maker_context(writer.recorder(), admission.clone()),
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

fn maker_context(
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    maker_context_with_writer(writer, admission)
}

fn maker_context_with_writer(
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    StrategyBuildContext::new(
        maker_order_economics(),
        bolt_v2::bolt_v3_strategy_context::StrategyDecisionEvidence::maker_for_test(writer),
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("MAKER.TEST"),
    )
}

fn maker_order_economics() -> bolt_v2::bolt_v3_order_execution::BoltV3OrderEconomicsHandle {
    maker_order_economics_at(1)
}

fn maker_order_economics_at(
    source_at_ns: u64,
) -> bolt_v2::bolt_v3_order_execution::BoltV3OrderEconomicsHandle {
    support::economics::polymarket_order_economics_for(
        "maker_execution_client",
        &[
            "YES.RUNTIME",
            "NO.RUNTIME",
            "MAKER-RT-YES.SIM",
            "MAKER-RT-NO.SIM",
            "MAKER-RT-SECOND-YES.SIM",
            "MAKER-RT-SECOND-NO.SIM",
            "MAKER-RT-UP.SIM",
            "MAKER-RT-DOWN.SIM",
            "MAKER-RT-YES-REISSUED.SIM",
        ],
        source_at_ns,
    )
}

fn accepted_maker_order(client_order_id: ClientOrderId) -> OrderAny {
    let mut order = OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("maker-strategy"),
            InstrumentId::from("MAKER-RT-YES.SIM"),
            client_order_id,
            OrderSide::Buy,
            Quantity::new(1.0, 2),
            Price::new(0.50, 2),
            TimeInForce::Gtc,
            None,
            true,
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
            UUID4::new(),
            UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)),
        )
        .expect("the tracked maker-order fixture is valid"),
    );
    let submitted = TestOrderEventStubs::submitted(&order, AccountId::from("ACCOUNT-001"));
    order
        .apply(submitted)
        .expect("the maker-order fixture submits");
    let accepted = TestOrderEventStubs::accepted(
        &order,
        AccountId::from("ACCOUNT-001"),
        VenueOrderId::from("VENUE-DRAIN-1"),
    );
    order
        .apply(accepted)
        .expect("the maker-order fixture is accepted");
    order
}

fn register_maker_for_order_factory(maker: &mut BinaryOracleMaker) {
    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock.borrow_mut().set_time(UnixNanos::from(1_u64));
    let cache = Rc::new(RefCell::new(Cache::default()));
    register_maker_strategy_core(maker, clock, cache);
}

fn register_maker_strategy_core(
    maker: &mut BinaryOracleMaker,
    clock: Rc<RefCell<dyn Clock>>,
    cache: Rc<RefCell<Cache>>,
) {
    let portfolio = Rc::new(RefCell::new(Portfolio::new(
        clock.clone(),
        cache.clone(),
        None,
    )));
    StrategyNative::strategy_core_mut(maker)
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

fn quote_set_inputs() -> MakerRuntimeQuoteSetInput {
    MakerRuntimeQuoteSetInput {
        yes_quantity: 2.0,
        no_quantity: 3.0,
        yes_resting_price: None,
        no_resting_price: None,
        requote_threshold: 0.001,
        eps: 0.001,
        now_ms: RUNTIME_NOW_MS,
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
const RUNTIME_YES_CLOB_TOKEN_ID: &str = "MAKER-RT-YES";
const RUNTIME_NO_CLOB_TOKEN_ID: &str = "MAKER-RT-NO";
const RUNTIME_SECOND_STATIC_SLUG: &str = "will-sample-second-maker-resolve-yes";
const RUNTIME_SECOND_STATIC_CONDITION_ID: &str = "condition-sample-second-maker";
const RUNTIME_SECOND_MARKET_KEY: &str = "btc-static-event";
const RUNTIME_SECOND_YES_INSTRUMENT: &str = "MAKER-RT-SECOND-YES.SIM";
const RUNTIME_SECOND_NO_INSTRUMENT: &str = "MAKER-RT-SECOND-NO.SIM";
const RUNTIME_UPDOWN_ASSET: &str = "ETH";
const RUNTIME_UPDOWN_CADENCE_SECONDS: u64 = 3_600;
const RUNTIME_UPDOWN_CADENCE_TOKEN: &str = "1h";
const RUNTIME_UP_INSTRUMENT: &str = "MAKER-RT-UP.SIM";
const RUNTIME_DOWN_INSTRUMENT: &str = "MAKER-RT-DOWN.SIM";
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

fn runtime_second_static_declaration() -> MakerMarketDeclaration {
    MakerMarketDeclaration {
        market_key: RUNTIME_SECOND_MARKET_KEY.to_string(),
        family_key: RUNTIME_STATIC_FAMILY.to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_seconds: 3_600,
        cadence_slug_token: RUNTIME_SECOND_STATIC_SLUG.to_string(),
        static_condition_id: Some(RUNTIME_SECOND_STATIC_CONDITION_ID.to_string()),
        static_yes_outcome: Some(RUNTIME_STATIC_YES_OUTCOME.to_string()),
        static_no_outcome: Some(RUNTIME_STATIC_NO_OUTCOME.to_string()),
    }
}

fn runtime_updown_declaration() -> MakerMarketDeclaration {
    MakerMarketDeclaration {
        market_key: MARKET_KEY.to_string(),
        family_key: updown::KEY.to_string(),
        underlying_asset: RUNTIME_UPDOWN_ASSET.to_string(),
        cadence_seconds: RUNTIME_UPDOWN_CADENCE_SECONDS,
        cadence_slug_token: RUNTIME_UPDOWN_CADENCE_TOKEN.to_string(),
        static_condition_id: None,
        static_yes_outcome: None,
        static_no_outcome: None,
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
    runtime_binary_option_for_static_market(
        instrument_id,
        outcome,
        RUNTIME_STATIC_SLUG,
        RUNTIME_STATIC_CONDITION_ID,
        market_id,
        activation_milliseconds,
    )
}

fn runtime_binary_option_for_static_market(
    instrument_id: &str,
    outcome: &str,
    market_slug: &str,
    condition_id: &str,
    market_id: &str,
    activation_milliseconds: u64,
) -> InstrumentAny {
    let question_id = format!("question-{market_slug}");
    runtime_binary_option_for_static_market_with_identity(
        instrument_id,
        instrument_id.split('.').next().unwrap_or(instrument_id),
        outcome,
        market_slug,
        condition_id,
        market_id,
        question_id.as_str(),
        activation_milliseconds,
    )
}

#[allow(clippy::too_many_arguments)]
fn runtime_binary_option_for_static_market_with_identity(
    instrument_id: &str,
    raw_symbol: &str,
    outcome: &str,
    market_slug: &str,
    condition_id: &str,
    market_id: &str,
    question_id: &str,
    activation_milliseconds: u64,
) -> InstrumentAny {
    runtime_binary_option_for_static_market_with_identity_and_expiration(
        instrument_id,
        raw_symbol,
        outcome,
        market_slug,
        condition_id,
        market_id,
        question_id,
        activation_milliseconds,
        RUNTIME_NOW_MS + 30_000,
    )
}

#[allow(clippy::too_many_arguments)]
fn runtime_binary_option_for_static_market_with_identity_and_expiration(
    instrument_id: &str,
    raw_symbol: &str,
    outcome: &str,
    market_slug: &str,
    condition_id: &str,
    market_id: &str,
    question_id: &str,
    activation_milliseconds: u64,
    expiration_milliseconds: u64,
) -> InstrumentAny {
    let mut info = Params::new();
    for (key, value) in [
        ("market_slug", market_slug),
        ("market_id", market_id),
        ("condition_id", condition_id),
        ("question_id", question_id),
    ] {
        info.insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
    // Production instruments carry this -- the pinned adapter always writes it
    // (`http/parse.rs`), so a fixture without it is not a real instrument.
    info.insert("neg_risk".to_string(), serde_json::Value::Bool(false));
    InstrumentAny::BinaryOption(BinaryOption::new(
        InstrumentId::from(instrument_id),
        Symbol::from(raw_symbol),
        AssetClass::Alternative,
        Currency::USDC(),
        (activation_milliseconds.saturating_mul(1_000_000)).into(),
        (expiration_milliseconds.saturating_mul(1_000_000)).into(),
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

fn runtime_static_instruments_with_question_id(question_id: &str) -> Vec<InstrumentAny> {
    let market_id = format!("market-{RUNTIME_STATIC_SLUG}");
    vec![
        runtime_binary_option_for_static_market_with_identity(
            RUNTIME_YES_INSTRUMENT,
            RUNTIME_YES_CLOB_TOKEN_ID,
            RUNTIME_STATIC_YES_OUTCOME,
            RUNTIME_STATIC_SLUG,
            RUNTIME_STATIC_CONDITION_ID,
            market_id.as_str(),
            question_id,
            RUNTIME_NOW_MS - 1_000,
        ),
        runtime_binary_option_for_static_market_with_identity(
            RUNTIME_NO_INSTRUMENT,
            RUNTIME_NO_CLOB_TOKEN_ID,
            RUNTIME_STATIC_NO_OUTCOME,
            RUNTIME_STATIC_SLUG,
            RUNTIME_STATIC_CONDITION_ID,
            market_id.as_str(),
            question_id,
            RUNTIME_NOW_MS - 1_000,
        ),
    ]
}

fn runtime_updown_instruments() -> Vec<InstrumentAny> {
    runtime_updown_instruments_for(MarketSelectionOutcome::Current)
}

fn runtime_next_updown_instruments() -> Vec<InstrumentAny> {
    runtime_updown_instruments_for(MarketSelectionOutcome::Next)
}

fn runtime_updown_instruments_for(selection_outcome: MarketSelectionOutcome) -> Vec<InstrumentAny> {
    let declaration = runtime_updown_declaration();
    let cadence_seconds =
        i64::try_from(declaration.cadence_seconds).expect("test cadence fits i64");
    let target = MarketSelectionTarget {
        family_key: declaration.family_key.as_str(),
        underlying_asset: declaration.underlying_asset.as_str(),
        cadence_seconds,
        cadence_slug_token: declaration.cadence_slug_token.as_str(),
        static_condition_id: None,
        static_yes_outcome: None,
        static_no_outcome: None,
    };
    let window = market_selection_candidate_windows_from_target(target, RUNTIME_NOW_MS)
        .expect("updown candidate windows compute")
        .into_iter()
        .find(|window| window.outcome == selection_outcome)
        .expect("updown fixture has the requested window");
    let market_id = format!("market-{}", window.market_slug);
    let condition_id = format!("condition-{}", window.market_slug);
    let question_id = format!("question-{}", window.market_slug);
    let expiration_milliseconds = match selection_outcome {
        MarketSelectionOutcome::Current => RUNTIME_NOW_MS + 30_000,
        MarketSelectionOutcome::Next => window
            .start_timestamp_milliseconds
            .checked_add(RUNTIME_UPDOWN_CADENCE_SECONDS * 1_000)
            .expect("fixture expiration fits u64"),
    };
    vec![
        runtime_binary_option_for_static_market_with_identity_and_expiration(
            RUNTIME_UP_INSTRUMENT,
            RUNTIME_UP_INSTRUMENT
                .split('.')
                .next()
                .expect("fixture instrument has a raw symbol"),
            "Up",
            window.market_slug.as_str(),
            condition_id.as_str(),
            market_id.as_str(),
            question_id.as_str(),
            window.start_timestamp_milliseconds,
            expiration_milliseconds,
        ),
        runtime_binary_option_for_static_market_with_identity_and_expiration(
            RUNTIME_DOWN_INSTRUMENT,
            RUNTIME_DOWN_INSTRUMENT
                .split('.')
                .next()
                .expect("fixture instrument has a raw symbol"),
            "Down",
            window.market_slug.as_str(),
            condition_id.as_str(),
            market_id.as_str(),
            question_id.as_str(),
            window.start_timestamp_milliseconds,
            expiration_milliseconds,
        ),
    ]
}

fn runtime_second_static_instruments() -> Vec<InstrumentAny> {
    let market_id = format!("market-{RUNTIME_SECOND_STATIC_SLUG}");
    vec![
        runtime_binary_option_for_static_market(
            RUNTIME_SECOND_YES_INSTRUMENT,
            RUNTIME_STATIC_YES_OUTCOME,
            RUNTIME_SECOND_STATIC_SLUG,
            RUNTIME_SECOND_STATIC_CONDITION_ID,
            &market_id,
            RUNTIME_NOW_MS - 1_000,
        ),
        runtime_binary_option_for_static_market(
            RUNTIME_SECOND_NO_INSTRUMENT,
            RUNTIME_STATIC_NO_OUTCOME,
            RUNTIME_SECOND_STATIC_SLUG,
            RUNTIME_SECOND_STATIC_CONDITION_ID,
            &market_id,
            RUNTIME_NOW_MS - 1_000,
        ),
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
/// slug / market_id / outcomes / raw venue token) but with the YES leg re-issued
/// under a NEW internal instrument id. A retain keyed only on the window start or
/// evidence identity would keep the stale YES instrument; the leg instrument id is
/// read live by the trade-subscription differ, so this is the regression guard that
/// a re-issued instrument under an unchanged window start is treated as a roll
/// (fail-closed) rather than a silent retain.
fn runtime_static_instruments_reissued_yes() -> Vec<InstrumentAny> {
    let market_id = format!("market-{RUNTIME_STATIC_SLUG}");
    let question_id = format!("question-{RUNTIME_STATIC_SLUG}");
    vec![
        runtime_binary_option_for_static_market_with_identity(
            RUNTIME_YES_INSTRUMENT_REISSUED,
            RUNTIME_YES_CLOB_TOKEN_ID,
            RUNTIME_STATIC_YES_OUTCOME,
            RUNTIME_STATIC_SLUG,
            RUNTIME_STATIC_CONDITION_ID,
            market_id.as_str(),
            question_id.as_str(),
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
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> StrategyBuildContext {
    StrategyBuildContext::new(
        maker_order_economics_at(RUNTIME_NOW_MS.saturating_mul(1_000_000)),
        bolt_v2::bolt_v3_strategy_context::StrategyDecisionEvidence::maker_for_test(writer),
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("SIM"),
    )
}

/// Register the maker with a real NT core whose clock reads `RUNTIME_NOW_MS`, so
/// the static instruments (whose activation/expiration bracket `RUNTIME_NOW_MS`)
/// are selectable at `on_start`. Returns the cache so the test can seed it.
fn register_maker_at_runtime_now_lifecycle_only(
    maker: &mut BinaryOracleMaker,
) -> Rc<RefCell<Cache>> {
    register_maker_at_runtime_now_lifecycle_only_with_quote_timer_handler(maker, true)
}

/// Register the maker against a `TestClock` set to `RUNTIME_NOW_MS`. When
/// `wire_quote_timer_handler` is true this also registers the clock's default
/// time-event handler, mirroring NT's `DataActor::register` (which wires it in
/// production); without it `TestClock::set_timer_ns` returns "No callbacks
/// provided" and `on_start`'s quote-timer registration fails loud. The
/// `DataActorNative::core_mut` registration used here is intentionally
/// lifecycle-only and does not initialize the strategy order factory.
fn register_maker_at_runtime_now_lifecycle_only_with_quote_timer_handler(
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
    maker
        .core_mut()
        .register(TraderId::from("TRADER-001"), clock, cache.clone())
        .expect("maker test strategy should register with NT core");
    cache
}

fn register_maker_at_runtime_now_for_order_factory(
    maker: &mut BinaryOracleMaker,
) -> Rc<RefCell<Cache>> {
    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock
        .borrow_mut()
        .set_time(UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)));
    clock
        .borrow_mut()
        .register_default_handler(TimeEventCallback::from(|_event: TimeEvent| {}));
    let cache = Rc::new(RefCell::new(Cache::default()));
    register_maker_strategy_core(maker, clock, cache.clone());
    cache
}

fn maker_with_active_static_market(
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> (BinaryOracleMaker, Rc<RefCell<Cache>>) {
    maker_with_active_markets(
        writer,
        admission,
        vec![runtime_static_declaration()],
        runtime_static_instruments(),
    )
}

fn maker_with_active_updown_market(
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> (BinaryOracleMaker, Rc<RefCell<Cache>>) {
    maker_with_active_markets(
        writer,
        admission,
        vec![runtime_updown_declaration()],
        runtime_updown_instruments(),
    )
}

fn maker_with_active_next_updown_market(
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
) -> (BinaryOracleMaker, Rc<RefCell<Cache>>) {
    maker_with_active_markets(
        writer,
        admission,
        vec![runtime_updown_declaration()],
        runtime_next_updown_instruments(),
    )
}

fn maker_with_active_markets(
    writer: Arc<DecisionEvidenceRecorder>,
    admission: Arc<BoltV3SubmitAdmissionState>,
    declarations: Vec<MakerMarketDeclaration>,
    instruments: Vec<InstrumentAny>,
) -> (BinaryOracleMaker, Rc<RefCell<Cache>>) {
    let mut config = maker_config();
    config.markets = declarations;
    let mut maker = BinaryOracleMaker::new(config, maker_sim_context(writer, admission));
    let cache = register_maker_at_runtime_now_for_order_factory(&mut maker);
    for instrument in instruments {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the active maker market");
    }
    DataActor::on_start(&mut maker).expect("active maker fixture resolves its market");
    (maker, cache)
}

fn refresh_maker_instruments(
    maker: &mut BinaryOracleMaker,
    cache: &Rc<RefCell<Cache>>,
    instruments: Vec<InstrumentAny>,
) {
    cache.borrow_mut().reset();
    for instrument in instruments {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("refreshing the maker instrument cache");
    }
    let timer_event = TimeEvent::new(
        ustr::Ustr::from("maker-strategy:quote_loop"),
        UUID4::new(),
        UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)),
        UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)),
    );
    DataActor::on_time_event(maker, &timer_event)
        .expect("the maker timer refreshes active markets");
}

fn record_one_budget_block(maker: &mut BinaryOracleMaker, market_key: &str) {
    record_one_budget_block_for_family(maker, market_key, RUNTIME_STATIC_FAMILY);
}

fn record_one_budget_block_for_family(
    maker: &mut BinaryOracleMaker,
    market_key: &str,
    family_key: &str,
) {
    let mut market = MarketQuote::new(false);
    let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
        .expect("one-submit budget fixture builds");
    let submit_template = maker_limit_post_only_template();
    maker
        .route_maker_runtime_quote(
            market_key,
            &mut market,
            &mut budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote_plan: quote_plan_inputs(family_key),
                quote_set: quote_set_inputs(),
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect("an active market routes its first budget block");
}

#[test]
fn maker_on_start_resolves_declared_markets_from_the_execution_venue_cache() {
    // The NT shell wiring: on_start reads the execution-venue-scoped instrument
    // cache, resolves the declared markets through the shared engine, and tracks
    // them in the runtime. With both leg instruments cached on the maker's venue,
    // the declared market becomes active (an empty cache would leave it idle).
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer.recorder(), admission),
    );
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer.recorder(), admission),
    );
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
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
fn maker_graceful_stop_defers_until_tracked_orders_close_and_rejects_new_quotes() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let order_economics = maker_order_economics_at(RUNTIME_NOW_MS.saturating_mul(1_000_000));
    let context = StrategyBuildContext::new(
        order_economics.clone(),
        bolt_v2::bolt_v3_strategy_context::StrategyDecisionEvidence::maker_for_test(
            writer.recorder(),
        ),
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("SIM"),
    );
    let mut maker = BinaryOracleMaker::new(maker_config_with_static_market(), context);
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the active maker market");
    }
    Component::initialize(&mut maker).expect("the registered maker initializes");
    Component::start(&mut maker).expect("the maker starts through the real NT lifecycle");
    assert!(Component::is_running(&maker));
    assert_eq!(maker.runtime().active_market_count(), 1);

    let client_order_id = ClientOrderId::from("MAKER-DRAIN-1");
    let accepted_order = accepted_maker_order(client_order_id);
    cache
        .borrow_mut()
        .add_order(accepted_order.clone(), None, None, false)
        .expect("the accepted maker order is present in the NT cache");
    order_economics
        .reconcile_fill_void_at(
            client_order_id,
            Some(accepted_order.clone()),
            RUNTIME_NOW_MS.saturating_mul(1_000_000),
        )
        .expect("a reopened maker order creates cancellation-only tracking");
    assert_eq!(order_economics.resting_order_ids().unwrap().len(), 1);

    assert!(
        !<BinaryOracleMaker as Strategy>::stop(&mut maker),
        "a tracked order must defer component stop"
    );
    assert!(Component::is_running(&maker));

    let mut quote = MarketQuote::new(false);
    let mut budget =
        build_requote_budget_pair("1/00:01:00", 100, 500).expect("the quote budget fixture builds");
    let submit_template = maker_limit_post_only_template();
    let error = maker
        .route_maker_runtime_quote(
            RUNTIME_MARKET_KEY,
            &mut quote,
            &mut budget,
            BinaryOracleMakerRuntimeQuoteRouteInput {
                quote_plan: quote_plan_inputs(RUNTIME_STATIC_FAMILY),
                quote_set: quote_set_inputs(),
                submit_template: &submit_template,
                price_precision: 2,
                quantity_precision: 2,
                submit_order_prefix: "maker_submit",
            },
        )
        .expect_err("draining must reject quote work before planning mutates state");
    assert_eq!(
        error.downcast_ref::<BinaryOracleMakerLifecycleError>(),
        Some(&BinaryOracleMakerLifecycleError::Draining)
    );
    assert_eq!(quote.market_state(), MarketState::Idle);
    assert_eq!(writer.records().len(), 0);

    let canceled = TestOrderEventStubs::canceled(
        &accepted_order,
        AccountId::from("ACCOUNT-001"),
        Some(VenueOrderId::from("VENUE-DRAIN-1")),
    );
    cache
        .borrow_mut()
        .update_order(&canceled)
        .expect("the terminal event updates authoritative NT cache state");
    let OrderEventAny::Canceled(canceled) = &canceled else {
        unreachable!("the test stub produced an order-canceled event")
    };
    <BinaryOracleMaker as Strategy>::on_order_canceled(&mut maker, canceled);

    assert!(order_economics.resting_order_ids().unwrap().is_empty());
    assert!(Component::is_stopped(&maker));
    assert_eq!(maker.runtime().active_market_count(), 0);
}

#[test]
fn maker_trader_stop_closure_defers_and_keeps_order_callbacks_live() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let order_economics = maker_order_economics_at(RUNTIME_NOW_MS.saturating_mul(1_000_000));
    let context = StrategyBuildContext::new(
        order_economics.clone(),
        bolt_v2::bolt_v3_strategy_context::StrategyDecisionEvidence::maker_for_test(
            writer.recorder(),
        ),
        admission,
        BoltV3OrderExecutionPolicy::shadow(),
        Venue::from("SIM"),
    );
    let maker = BinaryOracleMaker::new(maker_config_with_static_market(), context);

    let trader_id = TraderId::from("TRADER-001");
    let instance_id = UUID4::new();
    let clock_factory = ClockFactory::new(|| {
        let clock = TestClock::new();
        clock.set_time(UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)));
        Rc::new(RefCell::new(clock))
    });
    let clock = clock_factory.clock();
    let cache = Rc::new(RefCell::new(Cache::default()));
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the active maker market");
    }
    let portfolio = Rc::new(RefCell::new(Portfolio::new(clock, cache.clone(), None)));
    set_message_bus(Rc::new(RefCell::new(MessageBus::new(
        trader_id,
        instance_id,
        Some("maker-stop-deferral".to_string()),
        None,
    ))));
    let mut trader = Trader::new(
        trader_id,
        instance_id,
        Environment::Backtest,
        clock_factory,
        cache.clone(),
        portfolio,
    );
    trader
        .add_strategy(maker)
        .expect("Trader registers the maker and its stop callback");
    let strategy_id = *trader
        .strategy_ids()
        .first()
        .expect("the maker is registered");
    trader
        .start_strategy(&strategy_id)
        .expect("Trader starts the registered maker");

    let client_order_id = ClientOrderId::from("MAKER-TRADER-DRAIN-1");
    let accepted_order = accepted_maker_order(client_order_id);
    cache
        .borrow_mut()
        .add_order(accepted_order.clone(), None, None, false)
        .expect("the accepted maker order is present in the NT cache");
    order_economics
        .reconcile_fill_void_at(
            client_order_id,
            Some(accepted_order.clone()),
            RUNTIME_NOW_MS.saturating_mul(1_000_000),
        )
        .expect("a reopened maker order creates cancellation-only tracking");

    trader
        .stop_strategy(&strategy_id)
        .expect("Trader invokes the maker stop callback");
    {
        let maker = try_get_actor_unchecked::<BinaryOracleMaker>(&strategy_id.inner())
            .expect("the deferred maker remains registered");
        assert!(
            Component::is_running(&*maker),
            "a false stop callback must leave the strategy running"
        );
    }

    let canceled = TestOrderEventStubs::canceled(
        &accepted_order,
        AccountId::from("ACCOUNT-001"),
        Some(VenueOrderId::from("VENUE-DRAIN-1")),
    );
    cache
        .borrow_mut()
        .update_order(&canceled)
        .expect("the terminal event updates authoritative NT cache state");
    msgbus::publish_order_event(get_event_order_topic(strategy_id), &canceled);

    {
        let maker = try_get_actor_unchecked::<BinaryOracleMaker>(&strategy_id.inner())
            .expect("the stopped maker remains registered until Trader removes it");
        assert!(order_economics.resting_order_ids().unwrap().is_empty());
        assert!(
            Component::is_stopped(&*maker),
            "the registered callback completes the deferred stop"
        );
        assert_eq!(maker.runtime().active_market_count(), 0);
    }
    trader
        .remove_strategy(&strategy_id)
        .expect("the characterization test removes its registered strategy");
}

#[test]
fn maker_lifecycle_retires_throttle_episodes_from_the_runtime_owned_store() {
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer.recorder(), admission),
    );
    let cache = register_maker_at_runtime_now_for_order_factory(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }
    DataActor::on_start(&mut maker).expect("on_start resolves the declared market");
    let lifecycle_identity = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the declared market is active after on_start")
        .concrete_identity();

    let record_block = |maker: &mut BinaryOracleMaker| {
        let mut quote = MarketQuote::new(false);
        let mut budget = build_requote_budget_pair("1/00:01:00", 100, 500)
            .expect("one-submit budget fixture builds");
        let submit_template = maker_limit_post_only_template();
        maker
            .route_maker_runtime_quote(
                RUNTIME_MARKET_KEY,
                &mut quote,
                &mut budget,
                BinaryOracleMakerRuntimeQuoteRouteInput {
                    quote_plan: quote_plan_inputs(RUNTIME_STATIC_FAMILY),
                    quote_set: quote_set_inputs(),
                    submit_template: &submit_template,
                    price_precision: 2,
                    quantity_precision: 2,
                    submit_order_prefix: "maker_submit",
                },
            )
            .expect("the first denied leg in an active lifecycle records");
    };

    record_block(&mut maker);
    assert_eq!(writer.requote_throttles().len(), 1);

    // A real timer refresh drives the market out of the active set. The episode
    // must leave with it, so resolving the same concrete market again records its
    // first new block. This pins the strategy shell to the runtime-owned store;
    // there is no caller-supplied vector that can be replaced with a decoy.
    cache.borrow_mut().reset();
    let timer_event = TimeEvent::new(
        ustr::Ustr::from("maker-strategy:quote_loop"),
        UUID4::new(),
        UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)),
        UnixNanos::from(RUNTIME_NOW_MS.saturating_mul(1_000_000)),
    );
    DataActor::on_time_event(&mut maker, &timer_event)
        .expect("the timer refresh retires the missing market");
    assert_eq!(maker.runtime().active_market_count(), 0);

    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("re-seeding the same concrete market");
    }
    DataActor::on_time_event(&mut maker, &timer_event)
        .expect("the timer refresh resolves the market again");
    assert_eq!(
        maker
            .runtime()
            .market(RUNTIME_MARKET_KEY)
            .expect("the re-resolved market is active")
            .concrete_identity(),
        lifecycle_identity,
        "the refresh must re-resolve the same concrete market"
    );
    record_block(&mut maker);
    assert_eq!(
        writer.requote_throttles().len(),
        2,
        "the same concrete market must record again after a real refresh retires its prior episode"
    );

    // Stop is the other market-retirement path. It must clear the same owned
    // store before a same-market restart.
    DataActor::on_stop(&mut maker).expect("on_stop retires the active market");
    DataActor::on_start(&mut maker).expect("on_start resolves the same market after stop");
    assert_eq!(
        maker
            .runtime()
            .market(RUNTIME_MARKET_KEY)
            .expect("the restarted market is active")
            .concrete_identity(),
        lifecycle_identity,
        "stop/start must resolve the same concrete market"
    );
    record_block(&mut maker);
    assert_eq!(
        writer.requote_throttles().len(),
        3,
        "the same concrete market must record again after stop/start"
    );
}

#[test]
fn maker_on_start_fails_loud_when_quote_interval_overflows_the_nanosecond_clock() {
    // register_quote_timer converts quote_interval_ms into nanoseconds with a
    // checked_mul; a value so large that the ms -> ns conversion overflows u64 must
    // abort on_start (fail loud) rather than silently run with a wrong/saturated
    // cadence. Differential: it fails if the checked_mul guard is reverted to the
    // prior saturating_mul (which would silently clamp instead of erroring).
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let config = BinaryOracleMakerConfig {
        quote_interval_ms: u64::MAX,
        ..maker_config_with_static_market()
    };
    let mut maker = BinaryOracleMaker::new(config, maker_sim_context(writer.recorder(), admission));
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
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
fn maker_start_rejects_cancel_recovery_cadence_without_margin() {
    // The fixture config has a 1s cancel-retry timeout and a 5s refresh margin.
    // At a 2.5s drive cadence, the worst timer phase plus one rounded-up retry is
    // exactly 5s, so the strict pre-expiry recovery guarantee does not hold.
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let config = BinaryOracleMakerConfig {
        quote_interval_ms: 2_500,
        ..maker_config_with_static_market()
    };
    let mut maker = BinaryOracleMaker::new(config, maker_sim_context(writer.recorder(), admission));
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    let error = DataActor::on_start(&mut maker)
        .expect_err("a cadence without strict cancel-recovery margin must fail on_start");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("cancel-recovery cadence"),
        "on_start should name the unsafe cancel-recovery cadence: {rendered}"
    );
    assert_eq!(
        maker.runtime().active_market_count(),
        0,
        "cadence validation must fail before market refresh"
    );
}

#[test]
fn maker_start_rejects_cancel_recovery_cadence_arithmetic_overflow() {
    // This interval still converts from milliseconds to nanoseconds, but adding
    // the worst timer phase to one rounded retry interval overflows u64.
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let config = BinaryOracleMakerConfig {
        quote_interval_ms: u64::MAX / 1_000_000,
        ..maker_config_with_static_market()
    };
    let mut maker = BinaryOracleMaker::new(config, maker_sim_context(writer.recorder(), admission));
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    let error = DataActor::on_start(&mut maker)
        .expect_err("overflowing cancel-recovery cadence arithmetic must fail on_start");
    let rendered = format!("{error:#}");
    assert!(
        rendered.contains("cancel-recovery cadence arithmetic overflow"),
        "on_start should fail loud on cancel-recovery arithmetic overflow: {rendered}"
    );
    assert_eq!(
        maker.runtime().active_market_count(),
        0,
        "overflow validation must fail before market refresh"
    );
}

#[test]
fn maker_start_accepts_bounded_cancel_recovery_cadence() {
    // 2s cadence + ceil(1s / 2s) * 2s = 4s, strictly below the 5s margin.
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let config = BinaryOracleMakerConfig {
        quote_interval_ms: 2_000,
        ..maker_config_with_static_market()
    };
    let mut maker = BinaryOracleMaker::new(config, maker_sim_context(writer.recorder(), admission));
    let cache = register_maker_at_runtime_now_lifecycle_only(&mut maker);
    for instrument in runtime_static_instruments() {
        cache
            .borrow_mut()
            .add_instrument(instrument)
            .expect("seeding the venue cache with a maker instrument");
    }

    DataActor::on_start(&mut maker)
        .expect("a cadence with strict cancel-recovery margin should start");
    assert_eq!(maker.runtime().active_market_count(), 1);
    DataActor::on_stop(&mut maker).expect("the accepted maker cadence should stop cleanly");
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer.recorder(), admission),
    );
    let cache =
        register_maker_at_runtime_now_lifecycle_only_with_quote_timer_handler(&mut maker, false);
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
    let writer = support::current_evidence::RecordingDecisionEvidenceWriter::default();
    let admission = Arc::new(BoltV3SubmitAdmissionState::new(writer.recorder()));
    let mut maker = BinaryOracleMaker::new(
        maker_config_with_static_market(),
        maker_sim_context(writer.recorder(), admission.clone()),
    );
    let cache = register_maker_at_runtime_now_for_order_factory(&mut maker);
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
            },
        )
        .expect("run_quote_cycle routes an active market")
        .expect("an active market yields a quote-cycle outcome");

    let orders = outcome
        .orders
        .expect("a fresh market quote cycle dispatches leg order intent");
    let no_id = InstrumentId::from(RUNTIME_NO_INSTRUMENT);
    match &orders.yes.dispatch {
        Some(MakerOrderDispatchOutcome::SubmitAttempt {
            instrument_id,
            transaction,
            ..
        }) => {
            assert_eq!(
                *instrument_id, yes_id,
                "the YES leg intent must target the resolved YES instrument"
            );
            assert!(matches!(
                transaction,
                bolt_v2::bolt_v3_order_execution::BoltV3RestingSubmitTransactionOutcome::Attempt(
                    outcome
                ) if outcome.kind()
                    == bolt_v2::bolt_v3_order_execution::BoltV3SubmitAttemptKind::PolicySkipped
            ));
        }
        other => panic!("expected a YES submit intent in shadow, got {other:?}"),
    }
    // Clause (c) is per-leg: the NO leg mints + dispatches its own intent. Asserting
    // only the YES leg would let a regression that dropped the NO-leg rotation, or
    // transposed both rotations onto YES, ship green.
    match &orders.no.dispatch {
        Some(MakerOrderDispatchOutcome::SubmitAttempt {
            instrument_id,
            transaction,
            ..
        }) => {
            assert_eq!(
                *instrument_id, no_id,
                "the NO leg intent must target the resolved NO instrument"
            );
            assert!(matches!(
                transaction,
                bolt_v2::bolt_v3_order_execution::BoltV3RestingSubmitTransactionOutcome::Attempt(
                    outcome
                ) if outcome.kind()
                    == bolt_v2::bolt_v3_order_execution::BoltV3SubmitAttemptKind::PolicySkipped
            ));
        }
        other => panic!("expected a NO submit intent in shadow, got {other:?}"),
    }
    // Shadow chokepoint: intent emitted, nothing admitted to the venue.
    assert_eq!(admission.admitted_order_count(), 0);

    // A policy skip is not a submission: neither leg may fabricate an active order,
    // and each pre-minted identity remains in `next_order` for a later eligible
    // attempt.
    let market_runtime = maker
        .runtime()
        .market(RUNTIME_MARKET_KEY)
        .expect("the market is still active after the cycle");
    assert!(
        market_runtime.leg_binding(Leg::Yes).active_order.is_none(),
        "a policy-skipped YES intent must not fabricate an active order"
    );
    assert!(
        market_runtime.leg_binding(Leg::Yes).next_order.is_some(),
        "the minted next YES identity remains available after a policy skip"
    );
    assert!(
        market_runtime.leg_binding(Leg::No).active_order.is_none(),
        "a policy-skipped NO intent must not fabricate an active order"
    );
    assert!(
        market_runtime.leg_binding(Leg::No).next_order.is_some(),
        "the minted next NO identity remains available after a policy skip"
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
