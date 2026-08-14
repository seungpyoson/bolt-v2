//! Shared maker quote-control adapter.
//!
//! The quote lifecycle owns order-liveness transitions, while the requote budget
//! owns REST-call rate admission. This module composes those existing surfaces so
//! a denied budget cannot advance lifecycle state.

use crate::{
    bolt_v3_numeric::{is_non_negative_finite, sanitize_open_probability},
    bolt_v3_quote_lifecycle::{
        Leg, LegEvent, LegState, LifecycleAction, MarketAction, MarketQuote,
        QuoteLegTransitionProposal,
    },
    bolt_v3_requote_budget::{RequoteBudgetPair, RequoteBudgetReservationProposal},
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
    pub proposal: Option<MakerQuoteCommandProposal>,
    pub blocked_by: Option<QuoteControlBlockReason>,
    pub requote_needed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MakerQuoteCommandProposal {
    pub action: MarketAction,
    pub lifecycle: QuoteLegTransitionProposal,
    pub budget: MakerQuoteBudgetProposal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MakerQuoteBudgetProposal {
    Reserve(RequoteBudgetReservationProposal),
    Prepaid { generation: u64, now_ms: u64 },
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
    let lifecycle = market.propose_leg_event(input.leg, LegEvent::QuoteTrigger { requote_needed });
    let action = lifecycle.map(|proposal| MarketAction::Leg {
        leg: proposal.leg(),
        action: proposal.action(),
    });

    let proposal = if let Some(market_action) = action {
        let Some(budget_proposal) = propose_action_budget(
            market,
            budget,
            input.now_ms,
            market_action,
            Some(lifecycle.expect("an action must carry a lifecycle proposal")),
        ) else {
            return QuoteControlDecision {
                action: None,
                proposal: None,
                blocked_by: Some(QuoteControlBlockReason::RequoteBudgetExhausted),
                requote_needed,
            };
        };
        Some(MakerQuoteCommandProposal {
            action: market_action,
            lifecycle: lifecycle.expect("an action must carry a lifecycle proposal"),
            budget: budget_proposal,
        })
    } else {
        None
    };
    QuoteControlDecision {
        action,
        proposal,
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
/// market-wide cancel-scope variants (`CancelAllBothLegs`, `CancelAllOneSide`)
/// are budgeted by the governor/drain path, never this per-leg gate. They are
/// structurally unreachable here, so the arm refuses outright (charges nothing,
/// emits nothing) rather than under-charge a multi-order cancel as one REST call.
fn propose_action_budget(
    market: &MarketQuote,
    budget: &RequoteBudgetPair,
    now_ms: u64,
    action: MarketAction,
    lifecycle: Option<QuoteLegTransitionProposal>,
) -> Option<MakerQuoteBudgetProposal> {
    let lifecycle_action = match action {
        MarketAction::Leg { action, .. } => action,
        MarketAction::CancelAllBothLegs | MarketAction::CancelAllOneSide { .. } => {
            return None;
        }
    };
    let lifecycle = lifecycle?;
    match lifecycle_action {
        LifecycleAction::Submit
            if lifecycle.prior_state() == LegState::ReplacementPendingBackoff =>
        {
            market
                .prepaid_generation(lifecycle.leg())
                .map(|generation| MakerQuoteBudgetProposal::Prepaid { generation, now_ms })
                .or_else(|| {
                    budget
                        .propose_fresh_submit(now_ms)
                        .ok()
                        .map(MakerQuoteBudgetProposal::Reserve)
                })
        }
        LifecycleAction::Submit => budget
            .propose_fresh_submit(now_ms)
            .ok()
            .map(MakerQuoteBudgetProposal::Reserve),
        LifecycleAction::Cancel => budget
            .propose_cancel_resubmit(now_ms)
            .ok()
            .map(MakerQuoteBudgetProposal::Reserve),
        LifecycleAction::Modify => budget
            .propose_rest(now_ms)
            .ok()
            .map(MakerQuoteBudgetProposal::Reserve),
    }
}

fn blocked(reason: QuoteControlBlockReason) -> QuoteControlDecision {
    QuoteControlDecision {
        action: None,
        proposal: None,
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
    fn fresh_submit_planning_proposes_without_advancing_or_charging() {
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
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
    }

    #[test]
    fn cancel_resubmit_planning_proposes_without_advancing_or_charging() {
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
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
    }

    #[test]
    fn in_place_modify_planning_proposes_without_advancing_or_charging() {
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
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
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

    #[test]
    fn cancel_resubmit_planning_only_mints_the_prepaid_reservation_proposal() {
        // The cancel+resubmit cost (1 submit command + 2 REST calls) is reserved
        // WHOLE when the cancel is emitted. The replacement submit (T5) is driven by
        // the venue's Canceled confirmation, which the NT handler feeds straight to
        // the lifecycle via on_leg_event — NOT back through drive_quote_leg — so it
        // spends the pre-paid token instead of charging the budget a second time.
        // This pins that contract: a resubmit driven the wrong way (through
        // drive_quote_leg) would re-enter reserve_action_budget and double-charge.
        let mut market = resting_market(false, Leg::Yes);
        let mut budget = pair(4, 8);

        // Planning mints the one cancel+resubmit proposal without charging or
        // advancing. Shared execution owns its later reservation and settlement.
        let cancel = drive_quote_leg(
            &mut market,
            &mut budget,
            requote_input(Leg::Yes, 0.55, 0.40),
        );
        assert_eq!(
            cancel.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            })
        );
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(budget.outstanding_submit_cost(), 0);
        assert_eq!(budget.outstanding_rest_cost(), 0);
    }

    #[test]
    fn the_market_wide_cancel_arm_fails_closed_and_charges_nothing() {
        // reserve_action_budget's match is exhaustive over MarketAction, but the
        // market-wide cancel variants never arrive here: on_leg_event only ever yields
        // MarketAction::Leg, and CancelAllBothLegs / CancelAllOneSide originate in
        // drain()/cancel_one_side(), outside this per-leg gate. The arm is therefore
        // structurally unreachable; this pins its fail-closed contract so a future
        // refactor that accidentally routed a market-wide cancel through this gate is
        // REFUSED (charging and emitting nothing) rather than silently under-charging
        // a multi-order cancel as a single REST call. The prior behavior charged one
        // REST and allowed, so this test also fails against that pre-fix variant.
        let budget = pair(8, 8);
        assert!(
            propose_action_budget(
                &MarketQuote::new(false),
                &budget,
                NOW,
                MarketAction::CancelAllBothLegs,
                None,
            )
            .is_none()
        );
        assert!(
            propose_action_budget(
                &MarketQuote::new(false),
                &budget,
                NOW,
                MarketAction::CancelAllOneSide { leg: Leg::Yes },
                None,
            )
            .is_none()
        );
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
    }
}
