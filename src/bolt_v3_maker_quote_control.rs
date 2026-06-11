//! Shared maker quote-control adapter.
//!
//! The quote lifecycle owns order-liveness transitions, while the requote budget
//! owns REST-call rate admission. This module composes those existing surfaces so
//! a denied budget cannot advance lifecycle state.

use crate::{
    bolt_v3_numeric::{is_non_negative_finite, sanitize_open_probability},
    bolt_v3_quote_lifecycle::{Leg, LegEvent, LegState, MarketAction, MarketQuote},
    bolt_v3_requote_budget::RequoteBudget,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteControlInput {
    pub leg: Leg,
    pub desired_price: f64,
    pub resting_price: Option<f64>,
    pub requote_threshold: f64,
    pub eps: f64,
    pub now_ms: u64,
    pub action_cost: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteControlBlockReason {
    InvalidDesiredPrice,
    InvalidRestingPrice,
    InvalidRequoteThreshold,
    MissingRestingPrice,
    RequoteBudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteControlDecision {
    pub action: Option<MarketAction>,
    pub blocked_by: Option<QuoteControlBlockReason>,
    pub requote_needed: bool,
}

pub fn drive_quote_leg(
    market: &mut MarketQuote,
    budget: &mut RequoteBudget,
    input: QuoteControlInput,
) -> QuoteControlDecision {
    let Some(desired_price) = sanitize_open_probability(input.desired_price, input.eps) else {
        return blocked(QuoteControlBlockReason::InvalidDesiredPrice);
    };
    if !is_non_negative_finite(input.requote_threshold) {
        return blocked(QuoteControlBlockReason::InvalidRequoteThreshold);
    }
    let resting_price = match input.resting_price {
        Some(price) => {
            let Some(validated) = sanitize_open_probability(price, input.eps) else {
                return blocked(QuoteControlBlockReason::InvalidRestingPrice);
            };
            Some(validated)
        }
        None => None,
    };
    if market.leg_state(input.leg) == LegState::Resting && resting_price.is_none() {
        return blocked(QuoteControlBlockReason::MissingRestingPrice);
    }

    let requote_needed = resting_price
        .map(|price| (desired_price - price).abs() >= input.requote_threshold)
        .unwrap_or(true);
    let mut candidate = *market;
    let action = candidate.on_leg_event(input.leg, LegEvent::QuoteTrigger { requote_needed });

    if action.is_some() && !budget.try_acquire(input.now_ms, input.action_cost) {
        return QuoteControlDecision {
            action: None,
            blocked_by: Some(QuoteControlBlockReason::RequoteBudgetExhausted),
            requote_needed,
        };
    }

    *market = candidate;
    QuoteControlDecision {
        action,
        blocked_by: None,
        requote_needed,
    }
}

fn blocked(reason: QuoteControlBlockReason) -> QuoteControlDecision {
    QuoteControlDecision {
        action: None,
        blocked_by: Some(reason),
        requote_needed: false,
    }
}
