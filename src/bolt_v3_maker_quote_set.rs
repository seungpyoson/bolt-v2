//! Shared binary maker quote-set driver.
//!
//! This composes family quote targets, quote-control, and lifecycle state without
//! owning collateral admission, a strategy shell, or any NT calls. Shared submit
//! admission is the sole collateral authority after economics seals the order.

use crate::{
    bolt_v3_maker_quote_control::{QuoteControlDecision, QuoteControlInput, drive_quote_leg},
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_quote_lifecycle::{Leg, MarketQuote},
    bolt_v3_quoting::{QuoteSide, QuoteTargetLeg, QuoteTargets},
    bolt_v3_requote_budget::RequoteBudgetPair,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteSetInput {
    pub targets: QuoteTargets,
    pub yes_quantity: f64,
    pub no_quantity: f64,
    pub yes_resting_price: Option<f64>,
    pub no_resting_price: Option<f64>,
    pub requote_threshold: f64,
    pub eps: f64,
    pub now_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteSetBlockReason {
    UnsupportedQuoteSide,
    InvalidQuantity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteSetLegDecision {
    pub control: QuoteControlDecision,
    pub blocked_by: Option<QuoteSetBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteSetDecision {
    pub yes: QuoteSetLegDecision,
    pub no: QuoteSetLegDecision,
}

/// Drive both binary legs from ONE market event.
///
/// Both legs are co-quoted at a single `input.now_ms` — one logical quote tick.
/// That single timestamp is the precondition for the requote budget's same-tick
/// min-interval exemption: the two legs reserve from the shared budget at the
/// same clock, so the budget must not throttle the second leg as if it were a
/// distinct later tick. The single `now_ms` field structurally enforces the
/// contract — there is no per-leg timestamp, so a caller cannot drive the two
/// legs from two clocks through this driver. A strategy shell wiring this driver
/// MUST preserve that contract: one market event in, one `now_ms`, both legs out.
pub fn drive_binary_quote_set(
    market: &mut MarketQuote,
    budget: &mut RequoteBudgetPair,
    input: QuoteSetInput,
) -> QuoteSetDecision {
    let yes = drive_quote_set_leg(
        market,
        budget,
        QuoteSetLegInput {
            leg: Leg::Yes,
            target: input.targets.leg_a,
            quantity: input.yes_quantity,
            resting_price: input.yes_resting_price,
            requote_threshold: input.requote_threshold,
            eps: input.eps,
            now_ms: input.now_ms,
        },
    );
    let no = drive_quote_set_leg(
        market,
        budget,
        QuoteSetLegInput {
            leg: Leg::No,
            target: input.targets.leg_b,
            quantity: input.no_quantity,
            resting_price: input.no_resting_price,
            requote_threshold: input.requote_threshold,
            eps: input.eps,
            now_ms: input.now_ms,
        },
    );

    QuoteSetDecision { yes, no }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct QuoteSetLegInput {
    leg: Leg,
    target: QuoteTargetLeg,
    quantity: f64,
    resting_price: Option<f64>,
    requote_threshold: f64,
    eps: f64,
    now_ms: u64,
}

fn drive_quote_set_leg(
    market: &mut MarketQuote,
    budget: &mut RequoteBudgetPair,
    input: QuoteSetLegInput,
) -> QuoteSetLegDecision {
    if input.target.side != QuoteSide::Buy {
        return blocked(QuoteSetBlockReason::UnsupportedQuoteSide);
    }
    if !is_positive_finite(input.quantity) {
        return blocked(QuoteSetBlockReason::InvalidQuantity);
    }

    let control = drive_quote_leg(
        market,
        budget,
        QuoteControlInput {
            leg: input.leg,
            desired_price: input.target.price,
            resting_price: input.resting_price,
            requote_threshold: input.requote_threshold,
            eps: input.eps,
            now_ms: input.now_ms,
        },
    );

    QuoteSetLegDecision {
        control,
        blocked_by: None,
    }
}

fn blocked(reason: QuoteSetBlockReason) -> QuoteSetLegDecision {
    QuoteSetLegDecision {
        control: no_control_action(),
        blocked_by: Some(reason),
    }
}

fn no_control_action() -> QuoteControlDecision {
    QuoteControlDecision {
        action: None,
        proposal: None,
        blocked_by: None,
        requote_needed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_quote_lifecycle::{LegEvent, LegState, LifecycleAction, MarketAction};
    use crate::bolt_v3_requote_budget::RequoteBudget;

    const WINDOW_MS: u64 = 60_000;
    const NOW: u64 = 1_000;

    fn pair(submit_cap: u64, rest_cap: u64) -> RequoteBudgetPair {
        pair_with_interval(submit_cap, rest_cap, 0)
    }

    fn pair_with_interval(
        submit_cap: u64,
        rest_cap: u64,
        min_interval_ms: u64,
    ) -> RequoteBudgetPair {
        RequoteBudgetPair::new(
            RequoteBudget::new(submit_cap, WINDOW_MS, min_interval_ms),
            RequoteBudget::new(rest_cap, WINDOW_MS, min_interval_ms),
        )
    }

    fn buy_leg(price: f64) -> QuoteTargetLeg {
        QuoteTargetLeg {
            side: QuoteSide::Buy,
            price,
            size_notional: price,
        }
    }

    fn fresh_input() -> QuoteSetInput {
        QuoteSetInput {
            targets: QuoteTargets {
                leg_a: buy_leg(0.40),
                leg_b: buy_leg(0.45),
            },
            yes_quantity: 1.0,
            no_quantity: 1.0,
            yes_resting_price: None,
            no_resting_price: None,
            requote_threshold: 0.01,
            eps: 1e-9,
            now_ms: NOW,
        }
    }

    #[test]
    fn two_fresh_submit_plans_mint_independent_proposals_without_charging() {
        let mut market = MarketQuote::new(false);
        let mut budget = pair(4, 8);
        let decision = drive_binary_quote_set(&mut market, &mut budget, fresh_input());
        assert_eq!(
            decision.yes.control.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Submit,
            })
        );
        assert_eq!(
            decision.no.control.action,
            Some(MarketAction::Leg {
                leg: Leg::No,
                action: LifecycleAction::Submit,
            })
        );
        assert!(decision.yes.control.proposal.is_some());
        assert!(decision.no.control.proposal.is_some());
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
    }

    #[test]
    fn both_binary_legs_quote_in_one_cycle_under_a_nonzero_min_interval() {
        // Both legs are driven at the SAME now_ms through the SAME shared budget. A
        // budget whose min-interval throttle did not exempt co-incident ticks would
        // admit the YES leg's submit, advance last_emit to now, then refuse the NO
        // leg's submit at the same now (delta 0 < interval) — quoting only one side
        // of the binary market every cycle. With the same-tick exemption both legs
        // must submit. This is the driver-level differential guard for that fix.
        let mut market = MarketQuote::new(false);
        let mut budget = pair_with_interval(4, 8, 500);
        let decision = drive_binary_quote_set(&mut market, &mut budget, fresh_input());
        assert_eq!(
            decision.yes.control.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Submit,
            })
        );
        assert_eq!(
            decision.no.control.action,
            Some(MarketAction::Leg {
                leg: Leg::No,
                action: LifecycleAction::Submit,
            }),
            "the second binary leg must not be throttled by the first leg's same-tick emit"
        );
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Idle);
        assert_eq!(market.leg_state(Leg::No), LegState::Idle);
    }

    #[test]
    fn a_repricing_leg_and_a_fresh_leg_co_quote_at_one_tick_with_distinct_costs() {
        // The steady-state cross-class same-tick path the fix must protect: the YES
        // leg is already Resting and must reprice (cancel+resubmit = 1 submit + 2
        // REST), while the NO leg is Idle and submits fresh (1 submit + 1 REST) —
        // BOTH driven at the same now_ms through the SHARED pair. The YES cancel
        // commits first and advances last_emit on both sub-budgets to now; the
        // same-tick exemption is the only thing that then lets the NO submit through
        // at the same tick. The asymmetric REST cost must accumulate to 3 in-window
        // (2 from the reprice + 1 from the fresh submit) at one tick. A non-exempting
        // gate would throttle the NO leg and strand one side of the binary market, so
        // this is the driver-level differential guard for the cross-cost-class case.
        let mut market = MarketQuote::new(false);
        market.on_leg_event(
            Leg::Yes,
            LegEvent::QuoteTrigger {
                requote_needed: true,
            },
        );
        market.on_leg_event(Leg::Yes, LegEvent::Accepted);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);

        let mut budget = pair_with_interval(4, 8, 500);
        let input = QuoteSetInput {
            targets: QuoteTargets {
                leg_a: buy_leg(0.55),
                leg_b: buy_leg(0.45),
            },
            yes_quantity: 1.0,
            no_quantity: 1.0,
            yes_resting_price: Some(0.40),
            no_resting_price: None,
            requote_threshold: 0.01,
            eps: 1e-9,
            now_ms: NOW,
        };
        let decision = drive_binary_quote_set(&mut market, &mut budget, input);

        assert_eq!(
            decision.yes.control.action,
            Some(MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            })
        );
        assert_eq!(
            decision.no.control.action,
            Some(MarketAction::Leg {
                leg: Leg::No,
                action: LifecycleAction::Submit,
            }),
            "the fresh NO leg must not be throttled by the YES reprice's same-tick emit"
        );
        // 1 submit (reprice) + 1 submit (fresh) = 2 submit commands; 2 REST (reprice)
        // + 1 REST (fresh) = 3 REST calls, all charged at a single tick.
        assert_eq!(budget.submit_commands_in_window(), 0);
        assert_eq!(budget.rest_cost_in_window(), 0);
        assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
        assert_eq!(market.leg_state(Leg::No), LegState::Idle);
    }
}
