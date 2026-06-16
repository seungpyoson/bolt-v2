mod support;

use anyhow::Result;
use bolt_v2::{
    bolt_v3_decision_evidence::BoltV3AdmissionOutcome,
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_order_compile::MakerCompiledOrderCommand,
    bolt_v3_maker_order_dispatch::MakerOrderDispatchOutcome,
    bolt_v3_maker_order_plan::{MakerLegBinding, MakerMarketActionOrderInput},
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_rate_budget::build_requote_budget_pair,
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteInput, MakerRuntimeQuoteSetInput,
        plan_maker_runtime_quote,
    },
    bolt_v3_market_families::static_binary_event,
    bolt_v3_order_execution::BoltV3OrderExecutionPolicy,
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate},
    bolt_v3_quote_lifecycle::{Leg, LegEvent, LifecycleAction, MarketAction, MarketState},
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionState, BoltV3SubmitLifecyclePolicy},
    strategies::{
        binary_oracle_maker::{
            BinaryOracleMaker, BinaryOracleMakerConfig, BinaryOracleMakerMarketActionRouteInput,
            BinaryOracleMakerRuntimeQuoteRouteInput,
        },
        registry::{FeeProvider, StrategyBuildContext},
    },
};
use futures_util::{FutureExt, future::BoxFuture};
use nautilus_common::{cache::Cache, clock::TestClock};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, TraderId, Venue},
    types::{Price, Quantity},
};
use nautilus_portfolio::portfolio::Portfolio;
use nautilus_trading::Strategy;
use rust_decimal::Decimal;
use std::{cell::RefCell, rc::Rc, sync::Arc};

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
    }
}

fn quote_plan_inputs(family_key: &str) -> MakerQuotePlanInputs<'_> {
    MakerQuotePlanInputs {
        family_key,
        oracle_fair_probability_up: 0.60,
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
