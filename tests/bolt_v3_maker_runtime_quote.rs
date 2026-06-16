use bolt_v2::{
    bolt_v3_maker_event_fence::{ClientOrderId as MakerClientOrderId, OrderIdentity},
    bolt_v3_maker_order_plan::{MakerLegBinding, MakerOrderIntent},
    bolt_v3_maker_quote_plan::MakerQuotePlanInputs,
    bolt_v3_maker_rate_budget::build_requote_budget_pair,
    bolt_v3_maker_runtime_quote::{
        MakerRuntimeOrderPlanInput, MakerRuntimeQuoteBlockReason, MakerRuntimeQuoteInput,
        MakerRuntimeQuoteSetInput, plan_maker_runtime_quote,
    },
    bolt_v3_market_families::static_binary_event,
    bolt_v3_quote_lifecycle::{Leg, MarketState},
};
use nautilus_model::identifiers::InstrumentId;

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
