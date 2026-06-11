use bolt_v2::{
    bolt_v3_maker_event_fence::{ClientOrderId, OrderIdentity},
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerOrderIntent, MakerOrderPlanBlockReason, MakerOrderPlanInput,
        maker_order_intents_from_quote_set,
    },
    bolt_v3_maker_quote_set::{QuoteSetInput, drive_binary_quote_set},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LegState, LifecycleAction, MarketAction, MarketQuote,
    },
    bolt_v3_quoting::{QuoteSide, QuoteTargetLeg, QuoteTargets},
    bolt_v3_requote_budget::RequoteBudget,
};
use nautilus_model::identifiers::InstrumentId;

const EPSILON: f64 = 0.001;

#[test]
fn quote_set_submit_actions_materialize_submit_intents_with_next_identities() {
    let mut market = MarketQuote::new(false);
    let mut budget = RequoteBudget::new(2, 60_000, 0);
    let quote_set = drive_binary_quote_set(&mut market, &mut budget, quote_set_input(7.0));

    let plan = maker_order_intents_from_quote_set(plan_input(&quote_set));

    assert_eq!(
        plan.yes.intent,
        Some(MakerOrderIntent::Submit {
            leg: Leg::Yes,
            instrument_id: yes_instrument(),
            order_identity: identity("yes-next", 2),
            price: 0.40,
            quantity: 10.0,
        })
    );
    assert_eq!(
        plan.no.intent,
        Some(MakerOrderIntent::Submit {
            leg: Leg::No,
            instrument_id: no_instrument(),
            order_identity: identity("no-next", 2),
            price: 0.30,
            quantity: 10.0,
        })
    );
    assert_eq!(plan.yes.blocked_by, None);
    assert_eq!(plan.no.blocked_by, None);
}

#[test]
fn cancel_requote_uses_active_identity_not_next_submit_identity() {
    let mut market = resting_yes_market();
    let mut budget = RequoteBudget::new(1, 60_000, 0);
    let mut input = quote_set_input(0.0);
    input.targets.leg_a.price = 0.52;
    input.yes_resting_price = Some(0.50);
    let quote_set = drive_binary_quote_set(&mut market, &mut budget, input);

    let plan = maker_order_intents_from_quote_set(plan_input(&quote_set));

    assert_eq!(
        plan.yes.intent,
        Some(MakerOrderIntent::Cancel {
            leg: Leg::Yes,
            instrument_id: yes_instrument(),
            order_identity: identity("yes-active", 1),
        })
    );
    assert_eq!(plan.yes.blocked_by, None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
}

#[test]
fn missing_active_identity_blocks_cancel_intent_without_fabricating_order_id() {
    let mut market = resting_yes_market();
    let mut budget = RequoteBudget::new(1, 60_000, 0);
    let mut input = quote_set_input(0.0);
    input.targets.leg_a.price = 0.52;
    input.yes_resting_price = Some(0.50);
    let quote_set = drive_binary_quote_set(&mut market, &mut budget, input);
    let mut plan_input = plan_input(&quote_set);
    plan_input.yes.active_order = None;

    let plan = maker_order_intents_from_quote_set(plan_input);

    assert_eq!(plan.yes.intent, None);
    assert_eq!(
        plan.yes.blocked_by,
        Some(MakerOrderPlanBlockReason::MissingActiveOrderIdentity)
    );
}

#[test]
fn quote_set_blocked_leg_produces_no_order_intent_for_that_leg() {
    let mut market = MarketQuote::new(false);
    let mut budget = RequoteBudget::new(2, 60_000, 0);
    let quote_set = drive_binary_quote_set(&mut market, &mut budget, quote_set_input(6.0));

    let plan = maker_order_intents_from_quote_set(plan_input(&quote_set));

    assert!(matches!(
        plan.yes.intent,
        Some(MakerOrderIntent::Submit { leg: Leg::Yes, .. })
    ));
    assert_eq!(plan.no.intent, None);
    assert_eq!(plan.no.blocked_by, None);
}

fn plan_input(
    quote_set: &bolt_v2::bolt_v3_maker_quote_set::QuoteSetDecision,
) -> MakerOrderPlanInput<'_> {
    MakerOrderPlanInput {
        quote_set,
        targets: quote_targets(),
        yes_quantity: 10.0,
        no_quantity: 10.0,
        yes: MakerLegBinding {
            instrument_id: yes_instrument(),
            active_order: Some(identity("yes-active", 1)),
            next_order: Some(identity("yes-next", 2)),
        },
        no: MakerLegBinding {
            instrument_id: no_instrument(),
            active_order: Some(identity("no-active", 1)),
            next_order: Some(identity("no-next", 2)),
        },
    }
}

fn quote_set_input(available_collateral: f64) -> QuoteSetInput<'static> {
    QuoteSetInput {
        targets: quote_targets(),
        yes_quantity: 10.0,
        no_quantity: 10.0,
        yes_resting_price: None,
        no_resting_price: None,
        open_commitments: &[],
        max_fee_bps: 0.0,
        available_collateral,
        requote_threshold: 0.01,
        eps: EPSILON,
        now_ms: 1_000,
        action_cost: 1,
    }
}

fn quote_targets() -> QuoteTargets {
    QuoteTargets {
        leg_a: QuoteTargetLeg {
            side: QuoteSide::Buy,
            price: 0.40,
        },
        leg_b: QuoteTargetLeg {
            side: QuoteSide::Buy,
            price: 0.30,
        },
    }
}

fn resting_yes_market() -> MarketQuote {
    let mut market = MarketQuote::new(false);
    assert_eq!(
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: false,
            },
        ),
        Some(MarketAction::Leg {
            leg: Leg::Yes,
            action: LifecycleAction::Submit,
        })
    );
    assert_eq!(market.on_leg_event(Leg::Yes, LegEvent::Accepted), None);
    assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
    market
}

fn identity(value: &str, generation: u64) -> OrderIdentity {
    OrderIdentity::new(ClientOrderId::new(value.to_string()), generation)
}

fn yes_instrument() -> InstrumentId {
    InstrumentId::from("condition-MATCH-YES.POLYMARKET")
}

fn no_instrument() -> InstrumentId {
    InstrumentId::from("condition-MATCH-NO.POLYMARKET")
}
