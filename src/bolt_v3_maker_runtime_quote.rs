//! Shared maker runtime quote orchestrator.
//!
//! This is the thin runtime bridge over the pure maker pipeline:
//! quote-plan inputs resolve family quote targets, those targets drive the
//! quote-set lifecycle controller, and approved lifecycle actions map to maker
//! order intents. It owns no market-family selection, no venue facts, no clocks,
//! and no config defaults; callers supply already-resolved runtime inputs.

use crate::{
    bolt_v3_maker_order_plan::{
        MakerLegBinding, MakerOrderPlan, MakerOrderPlanInput, maker_order_intents_from_quote_set,
    },
    bolt_v3_maker_quote_plan::{MakerQuotePlan, MakerQuotePlanInputs, plan_maker_quote_targets},
    bolt_v3_maker_quote_set::{QuoteSetDecision, QuoteSetInput, drive_binary_quote_set},
    bolt_v3_maker_reservation::BuyCommitment,
    bolt_v3_quote_lifecycle::MarketQuote,
    bolt_v3_requote_budget::RequoteBudgetPair,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeQuoteInput<'a> {
    pub quote_plan: MakerQuotePlanInputs<'a>,
    pub quote_set: MakerRuntimeQuoteSetInput<'a>,
    pub order_plan: MakerRuntimeOrderPlanInput,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerRuntimeQuoteSetInput<'a> {
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes_resting_price: Option<f64>,
    pub no_resting_price: Option<f64>,
    pub open_commitments: &'a [BuyCommitment],
    pub max_fee_bps: f64,
    pub available_collateral: f64,
    pub requote_threshold: f64,
    pub eps: f64,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakerRuntimeOrderPlanInput {
    pub yes: MakerLegBinding,
    pub no: MakerLegBinding,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerRuntimeQuoteDecision {
    pub quote_plan: Option<MakerQuotePlan>,
    pub quote_set: Option<QuoteSetDecision>,
    pub order_plan: Option<MakerOrderPlan>,
    pub blocked_by: Option<MakerRuntimeQuoteBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerRuntimeQuoteBlockReason {
    QuotePlanUnavailable,
}

pub fn plan_maker_runtime_quote(
    market: &mut MarketQuote,
    budget: &mut RequoteBudgetPair,
    input: MakerRuntimeQuoteInput<'_>,
) -> MakerRuntimeQuoteDecision {
    let Some(quote_plan) = plan_maker_quote_targets(input.quote_plan) else {
        return blocked(MakerRuntimeQuoteBlockReason::QuotePlanUnavailable);
    };

    let quote_set = drive_binary_quote_set(
        market,
        budget,
        QuoteSetInput {
            targets: quote_plan.targets,
            yes_quantity: input.quote_set.yes_quantity,
            no_quantity: input.quote_set.no_quantity,
            yes_resting_price: input.quote_set.yes_resting_price,
            no_resting_price: input.quote_set.no_resting_price,
            open_commitments: input.quote_set.open_commitments,
            max_fee_bps: input.quote_set.max_fee_bps,
            available_collateral: input.quote_set.available_collateral,
            requote_threshold: input.quote_set.requote_threshold,
            eps: input.quote_set.eps,
            now_ms: input.quote_set.now_ms,
        },
    );
    let order_plan = maker_order_intents_from_quote_set(MakerOrderPlanInput {
        quote_set: &quote_set,
        targets: quote_plan.targets,
        yes_quantity: input.quote_set.yes_quantity,
        no_quantity: input.quote_set.no_quantity,
        yes: input.order_plan.yes,
        no: input.order_plan.no,
    });

    MakerRuntimeQuoteDecision {
        quote_plan: Some(quote_plan),
        quote_set: Some(quote_set),
        order_plan: Some(order_plan),
        blocked_by: None,
    }
}

fn blocked(reason: MakerRuntimeQuoteBlockReason) -> MakerRuntimeQuoteDecision {
    MakerRuntimeQuoteDecision {
        quote_plan: None,
        quote_set: None,
        order_plan: None,
        blocked_by: Some(reason),
    }
}
