//! Shared maker quote-control adapter.
//!
//! The quote lifecycle owns order-liveness transitions, while the requote budget
//! owns REST-call rate admission. This module composes those existing surfaces so
//! a denied budget cannot advance lifecycle state.

use crate::{
    bolt_v3_numeric::{is_non_negative_finite, sanitize_open_probability},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LegState, LifecycleAction, MarketAction, MarketQuote,
    },
    bolt_v3_requote_budget::RequoteBudgetPair,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteControlInput {
    pub leg: Leg,
    pub desired_price: f64,
    pub resting_price: Option<f64>,
    pub requote_threshold: f64,
    pub eps: f64,
    pub now_ms: u64,
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
    budget: &mut RequoteBudgetPair,
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

    if let Some(market_action) = action {
        if !reserve_action_budget(budget, input.now_ms, market_action) {
            return QuoteControlDecision {
                action: None,
                blocked_by: Some(QuoteControlBlockReason::RequoteBudgetExhausted),
                requote_needed,
            };
        }
    }

    *market = candidate;
    QuoteControlDecision {
        action,
        blocked_by: None,
        requote_needed,
    }
}

/// Reserve the two-budget requote cost of one lifecycle action atomically.
///
/// The cost class follows the action itself, never a caller-supplied scalar, so
/// the submit-command budget and the REST-call budget can never be collapsed
/// into a single window:
/// - `Submit` — a fresh post-only quote: one submit command + one REST call.
/// - `Cancel` — the cancel+resubmit reprice path (modify-unsupported venues).
///   The WHOLE round-trip (one submit command + two REST calls) is reserved up
///   front, so a cancel can never be emitted without the budget to resubmit and
///   strand the side. The replacement submit is driven later by the `Canceled`
///   confirmation, not by this gate, so it is never charged twice.
/// - `Modify` — an in-place amend (modify-capable venues): one REST call and no
///   submit command — the same cost class as a standalone cancel.
///
/// `on_leg_event` only ever yields a per-leg [`MarketAction::Leg`]; the
/// market-wide cancel-scope variants come from the governor/drain path. Should
/// one ever reach here it is charged as a single REST call (fail-closed).
fn reserve_action_budget(
    budget: &mut RequoteBudgetPair,
    now_ms: u64,
    action: MarketAction,
) -> bool {
    let lifecycle_action = match action {
        MarketAction::Leg { action, .. } => action,
        MarketAction::CancelAllBothLegs | MarketAction::CancelAllOneSide { .. } => {
            return budget.try_reserve_cancel(now_ms);
        }
    };
    match lifecycle_action {
        LifecycleAction::Submit => budget.try_reserve_fresh_submit(now_ms),
        LifecycleAction::Cancel => budget.try_reserve_cancel_resubmit(now_ms),
        LifecycleAction::Modify => budget.try_reserve_cancel(now_ms),
    }
}

fn blocked(reason: QuoteControlBlockReason) -> QuoteControlDecision {
    QuoteControlDecision {
        action: None,
        blocked_by: Some(reason),
        requote_needed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_requote_budget::RequoteBudget;

    const WINDOW_MS: u64 = 60_000;
    const NO_MIN_INTERVAL: u64 = 0;
    const NOW: u64 = 1_000;

    fn pair(submit_cap: u64, rest_cap: u64) -> RequoteBudgetPair {
        RequoteBudgetPair::new(
            RequoteBudget::new(submit_cap, WINDOW_MS, NO_MIN_INTERVAL),
            RequoteBudget::new(rest_cap, WINDOW_MS, NO_MIN_INTERVAL),
        )
    }

    fn resting_market(supports_modify: bool, leg: Leg) -> MarketQuote {
        let mut market = MarketQuote::new(supports_modify);
        market.on_leg_event(
            leg,
            LegEvent::QuoteTrigger {
                requote_needed: true,
            },
        );
        market.on_leg_event(leg, LegEvent::Accepted);
        assert_eq!(market.leg_state(leg), LegState::Resting);
        market
    }

    fn requote_input(leg: Leg, desired: f64, resting: f64) -> QuoteControlInput {
        QuoteControlInput {
            leg,
            desired_price: desired,
            resting_price: Some(resting),
            requote_threshold: 0.01,
            eps: 1e-9,
            now_ms: NOW,
        }
    }

    #[test]
    fn fresh_submit_reserves_one_submit_command_and_one_rest_call() {
        let mut market = MarketQuote::new(false);
        let mut budget = pair(1, 8);
        let decision = drive_quote_leg(
            &mut market,
            &mut budget,
            QuoteControlInput {
                leg: Leg::Yes,
                desired_price: 0.40,
                resting_price: None,
                requote_threshold: 0.01,
                eps: 1e-9,
                now_ms: NOW,
            },
        );
        assert_eq!(
            decision.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Submit,
            })
        );
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 1);
        assert_eq!(market.leg_state(Leg::Yes), LegState::SubmitPending);
    }

    #[test]
    fn cancel_resubmit_requote_reserves_one_submit_command_and_two_rest_calls() {
        let mut market = resting_market(false, Leg::Yes);
        let mut budget = pair(1, 8);
        let decision = drive_quote_leg(
            &mut market,
            &mut budget,
            requote_input(Leg::Yes, 0.55, 0.40),
        );
        assert_eq!(
            decision.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            })
        );
        assert_eq!(budget.submit_commands_in_window(), 1);
        assert_eq!(budget.rest_cost_in_window(), 2);
        assert_eq!(market.leg_state(Leg::Yes), LegState::RequotePending);
    }

    #[test]
    fn in_place_modify_reserves_one_rest_call_and_zero_submit_command() {
        let mut market = resting_market(true, Leg::Yes);
        // Submit budget fully exhausted; an in-place modify costs zero submit
        // commands so it must still pass on its single REST call.
        let mut budget = pair(0, 8);
        let decision = drive_quote_leg(
            &mut market,
            &mut budget,
            requote_input(Leg::Yes, 0.55, 0.40),
        );
        assert_eq!(
            decision.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Modify,
            })
        );
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 1);
        assert_eq!(market.leg_state(Leg::Yes), LegState::ModifyPending);
    }

    #[test]
    fn exhausted_submit_budget_blocks_a_cancel_resubmit_even_when_rest_is_available() {
        // The decisive two-budget guard: a cancel+resubmit reprice needs one
        // submit command + two REST calls. REST budget is plentiful but the
        // submit budget is empty, so the cancel MUST be refused — a single
        // REST-only budget would wrongly admit the cancel and strand the leg.
        let mut market = resting_market(false, Leg::Yes);
        let mut budget = pair(0, 8);
        let decision = drive_quote_leg(
            &mut market,
            &mut budget,
            requote_input(Leg::Yes, 0.55, 0.40),
        );
        assert_eq!(decision.action, None);
        assert_eq!(
            decision.blocked_by,
            Some(QuoteControlBlockReason::RequoteBudgetExhausted)
        );
        assert!(decision.requote_needed);
        // Atomicity: REST is left uncharged despite being available, and the leg
        // stays Resting — no cancel was emitted, so the side is not stranded.
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
    }
}
