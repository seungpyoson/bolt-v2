//! Shared maker lifecycle-action to order-intent mapper.
//!
//! Quote planning and lifecycle control stay pure. This module binds their
//! approved actions to caller-supplied instruments and order identities so the
//! runtime strategy shell can execute explicit commands without fabricating IDs.

use crate::{
    bolt_v3_maker_event_fence::OrderIdentity,
    bolt_v3_maker_quote_set::QuoteSetDecision,
    bolt_v3_quote_lifecycle::{Leg, LifecycleAction, MarketAction},
    bolt_v3_quoting::{QuoteSide, QuoteTargets},
};
use nautilus_model::{enums::OrderSide, identifiers::InstrumentId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerLegBinding {
    pub instrument_id: InstrumentId,
    pub active_order: Option<OrderIdentity>,
    pub next_order: Option<OrderIdentity>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerOrderPlanInput<'a> {
    pub quote_set: &'a QuoteSetDecision,
    pub targets: QuoteTargets,
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerMarketActionOrderInput {
    pub action: MarketAction,
    pub targets: QuoteTargets,
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MakerOrderIntent {
    Submit {
        leg: Leg,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        order_identity: OrderIdentity,
        price: f64,
        quantity: f64,
    },
    Cancel {
        leg: Leg,
        instrument_id: InstrumentId,
        order_identity: OrderIdentity,
    },
    CancelAll {
        leg: Option<Leg>,
        instrument_id: InstrumentId,
        order_side: Option<OrderSide>,
    },
    Modify {
        leg: Leg,
        instrument_id: InstrumentId,
        order_identity: OrderIdentity,
        price: f64,
        quantity: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerOrderPlanBlockReason {
    MissingNextOrderIdentity,
    MissingActiveOrderIdentity,
    ActionLegMismatch,
    UnsupportedMarketAction,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerLegOrderPlan {
    pub intent: Option<MakerOrderIntent>,
    pub blocked_by: Option<MakerOrderPlanBlockReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerOrderPlan {
    pub yes: MakerLegOrderPlan,
    pub no: MakerLegOrderPlan,
}

pub fn maker_order_intents_from_quote_set(input: MakerOrderPlanInput<'_>) -> MakerOrderPlan {
    MakerOrderPlan {
        yes: maker_leg_order_intent(MakerLegOrderInput {
            expected_leg: Leg::Yes,
            action: input.quote_set.yes.control.action,
            quote_side: input.targets.leg_a.side,
            price: input.targets.leg_a.price,
            quantity: input.yes_quantity,
            binding: input.yes,
        }),
        no: maker_leg_order_intent(MakerLegOrderInput {
            expected_leg: Leg::No,
            action: input.quote_set.no.control.action,
            quote_side: input.targets.leg_b.side,
            price: input.targets.leg_b.price,
            quantity: input.no_quantity,
            binding: input.no,
        }),
    }
}

pub fn maker_order_intent_from_market_action(
    input: MakerMarketActionOrderInput,
) -> MakerLegOrderPlan {
    let action = input.action;
    let plan = maker_order_plan_from_market_action(input);
    match action {
        MarketAction::Leg { leg: Leg::Yes, .. } | MarketAction::CancelAllBothLegs => plan.yes,
        MarketAction::Leg { leg: Leg::No, .. } => plan.no,
        MarketAction::CancelAllOneSide { leg: Leg::Yes } => plan.yes,
        MarketAction::CancelAllOneSide { leg: Leg::No } => plan.no,
    }
}

pub fn maker_order_plan_from_market_action(input: MakerMarketActionOrderInput) -> MakerOrderPlan {
    match input.action {
        MarketAction::Leg { leg: Leg::Yes, .. } => {
            maker_market_leg_order_plan(MakerLegOrderInput {
                expected_leg: Leg::Yes,
                action: Some(input.action),
                quote_side: input.targets.leg_a.side,
                price: input.targets.leg_a.price,
                quantity: input.yes_quantity,
                binding: input.yes,
            })
        }
        MarketAction::Leg { leg: Leg::No, .. } => maker_market_leg_order_plan(MakerLegOrderInput {
            expected_leg: Leg::No,
            action: Some(input.action),
            quote_side: input.targets.leg_b.side,
            price: input.targets.leg_b.price,
            quantity: input.no_quantity,
            binding: input.no,
        }),
        MarketAction::CancelAllBothLegs => cancel_all_both_legs_intent(input.yes, input.no),
        MarketAction::CancelAllOneSide { leg: Leg::Yes } => MakerOrderPlan {
            yes: cancel_all_intent(
                Some(Leg::Yes),
                input.yes.instrument_id,
                Some(order_side_from_quote_side(input.targets.leg_a.side)),
            ),
            no: no_order_intent(),
        },
        MarketAction::CancelAllOneSide { leg: Leg::No } => MakerOrderPlan {
            yes: no_order_intent(),
            no: cancel_all_intent(
                Some(Leg::No),
                input.no.instrument_id,
                Some(order_side_from_quote_side(input.targets.leg_b.side)),
            ),
        },
    }
}

fn cancel_all_both_legs_intent(yes: MakerLegBinding, no: MakerLegBinding) -> MakerOrderPlan {
    let yes_instrument_id = yes.instrument_id;
    let no_instrument_id = no.instrument_id;
    let same_instrument = no_instrument_id == yes_instrument_id;
    let yes_plan = cancel_all_intent(Some(Leg::Yes), yes_instrument_id, None);
    let no_plan = if same_instrument {
        no_order_intent()
    } else {
        cancel_all_intent(Some(Leg::No), no_instrument_id, None)
    };

    MakerOrderPlan {
        yes: yes_plan,
        no: no_plan,
    }
}

fn cancel_all_intent(
    leg: Option<Leg>,
    instrument_id: InstrumentId,
    order_side: Option<OrderSide>,
) -> MakerLegOrderPlan {
    MakerLegOrderPlan {
        intent: Some(MakerOrderIntent::CancelAll {
            leg,
            instrument_id,
            order_side,
        }),
        blocked_by: None,
    }
}

fn maker_market_leg_order_plan(input: MakerLegOrderInput) -> MakerOrderPlan {
    match input.expected_leg {
        Leg::Yes => MakerOrderPlan {
            yes: maker_leg_order_intent(input),
            no: no_order_intent(),
        },
        Leg::No => MakerOrderPlan {
            yes: no_order_intent(),
            no: maker_leg_order_intent(input),
        },
    }
}

#[derive(Debug, Clone, PartialEq)]
struct MakerLegOrderInput {
    expected_leg: Leg,
    action: Option<MarketAction>,
    quote_side: QuoteSide,
    price: f64,
    quantity: f64,
    binding: MakerLegBinding,
}

fn maker_leg_order_intent(input: MakerLegOrderInput) -> MakerLegOrderPlan {
    let Some(action) = input.action else {
        return no_order_intent();
    };

    let MarketAction::Leg { leg, action } = action else {
        return blocked(MakerOrderPlanBlockReason::UnsupportedMarketAction);
    };
    if leg != input.expected_leg {
        return blocked(MakerOrderPlanBlockReason::ActionLegMismatch);
    }

    match action {
        LifecycleAction::Submit => submit_intent(input),
        LifecycleAction::Cancel => cancel_intent(input),
        LifecycleAction::Modify => modify_intent(input),
    }
}

fn submit_intent(input: MakerLegOrderInput) -> MakerLegOrderPlan {
    let Some(order_identity) = input.binding.next_order else {
        return blocked(MakerOrderPlanBlockReason::MissingNextOrderIdentity);
    };
    MakerLegOrderPlan {
        intent: Some(MakerOrderIntent::Submit {
            leg: input.expected_leg,
            instrument_id: input.binding.instrument_id,
            order_side: order_side_from_quote_side(input.quote_side),
            order_identity,
            price: input.price,
            quantity: input.quantity,
        }),
        blocked_by: None,
    }
}

fn cancel_intent(input: MakerLegOrderInput) -> MakerLegOrderPlan {
    let Some(order_identity) = input.binding.active_order else {
        return blocked(MakerOrderPlanBlockReason::MissingActiveOrderIdentity);
    };
    MakerLegOrderPlan {
        intent: Some(MakerOrderIntent::Cancel {
            leg: input.expected_leg,
            instrument_id: input.binding.instrument_id,
            order_identity,
        }),
        blocked_by: None,
    }
}

fn modify_intent(input: MakerLegOrderInput) -> MakerLegOrderPlan {
    let Some(order_identity) = input.binding.active_order else {
        return blocked(MakerOrderPlanBlockReason::MissingActiveOrderIdentity);
    };
    MakerLegOrderPlan {
        intent: Some(MakerOrderIntent::Modify {
            leg: input.expected_leg,
            instrument_id: input.binding.instrument_id,
            order_identity,
            price: input.price,
            quantity: input.quantity,
        }),
        blocked_by: None,
    }
}

fn no_order_intent() -> MakerLegOrderPlan {
    MakerLegOrderPlan {
        intent: None,
        blocked_by: None,
    }
}

fn blocked(reason: MakerOrderPlanBlockReason) -> MakerLegOrderPlan {
    MakerLegOrderPlan {
        intent: None,
        blocked_by: Some(reason),
    }
}

fn order_side_from_quote_side(side: QuoteSide) -> OrderSide {
    match side {
        QuoteSide::Buy => OrderSide::Buy,
        QuoteSide::Sell => OrderSide::Sell,
    }
}
