//! Shared binary maker quote-set driver.
//!
//! This composes family quote targets, per-market reservation, quote-control,
//! and lifecycle state without owning a strategy shell or any NT calls.

use crate::{
    bolt_v3_maker_quote_control::{QuoteControlDecision, QuoteControlInput, drive_quote_leg},
    bolt_v3_maker_reservation::{
        BuyCommitment, ReservationDecision, ReservationRequest, evaluate_reservation,
    },
    bolt_v3_numeric::is_positive_finite,
    bolt_v3_quote_lifecycle::{Leg, LifecycleAction, MarketAction, MarketQuote},
    bolt_v3_quoting::{QuoteSide, QuoteTargetLeg, QuoteTargets},
    bolt_v3_requote_budget::RequoteBudget,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteSetInput<'a> {
    pub targets: QuoteTargets,
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
    pub action_cost: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteSetBlockReason {
    UnsupportedQuoteSide,
    InvalidQuantity,
    ReservationRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteSetLegDecision {
    pub control: QuoteControlDecision,
    pub blocked_by: Option<QuoteSetBlockReason>,
    pub reservation: Option<ReservationDecision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteSetDecision {
    pub yes: QuoteSetLegDecision,
    pub no: QuoteSetLegDecision,
}

pub fn drive_binary_quote_set(
    market: &mut MarketQuote,
    budget: &mut RequoteBudget,
    input: QuoteSetInput<'_>,
) -> QuoteSetDecision {
    let mut accepted_commitments = input.open_commitments.to_vec();
    let yes = drive_quote_set_leg(
        market,
        budget,
        &mut accepted_commitments,
        QuoteSetLegInput {
            leg: Leg::Yes,
            target: input.targets.leg_a,
            quantity: input.yes_quantity,
            resting_price: input.yes_resting_price,
            max_fee_bps: input.max_fee_bps,
            available_collateral: input.available_collateral,
            requote_threshold: input.requote_threshold,
            eps: input.eps,
            now_ms: input.now_ms,
            action_cost: input.action_cost,
        },
    );
    let no = drive_quote_set_leg(
        market,
        budget,
        &mut accepted_commitments,
        QuoteSetLegInput {
            leg: Leg::No,
            target: input.targets.leg_b,
            quantity: input.no_quantity,
            resting_price: input.no_resting_price,
            max_fee_bps: input.max_fee_bps,
            available_collateral: input.available_collateral,
            requote_threshold: input.requote_threshold,
            eps: input.eps,
            now_ms: input.now_ms,
            action_cost: input.action_cost,
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
    max_fee_bps: f64,
    available_collateral: f64,
    requote_threshold: f64,
    eps: f64,
    now_ms: u64,
    action_cost: u64,
}

fn drive_quote_set_leg(
    market: &mut MarketQuote,
    budget: &mut RequoteBudget,
    accepted_commitments: &mut Vec<BuyCommitment>,
    input: QuoteSetLegInput,
) -> QuoteSetLegDecision {
    if input.target.side != QuoteSide::Buy {
        return blocked(QuoteSetBlockReason::UnsupportedQuoteSide);
    }
    if !is_positive_finite(input.quantity) {
        return blocked(QuoteSetBlockReason::InvalidQuantity);
    }

    let mut candidate_market = *market;
    let mut candidate_budget = budget.clone();
    let control = drive_quote_leg(
        &mut candidate_market,
        &mut candidate_budget,
        QuoteControlInput {
            leg: input.leg,
            desired_price: input.target.price,
            resting_price: input.resting_price,
            requote_threshold: input.requote_threshold,
            eps: input.eps,
            now_ms: input.now_ms,
            action_cost: input.action_cost,
        },
    );

    let mut reservation = None;
    let submit_commitment = if is_submit_action(control.action) {
        let candidate = BuyCommitment::new(input.target.price, input.quantity);
        let decision = evaluate_reservation(ReservationRequest {
            open: accepted_commitments,
            candidate,
            max_fee_bps: input.max_fee_bps,
            available_collateral: input.available_collateral,
        });
        reservation = Some(decision);
        if decision == ReservationDecision::Reject {
            return QuoteSetLegDecision {
                control: no_control_action(),
                blocked_by: Some(QuoteSetBlockReason::ReservationRejected),
                reservation,
            };
        }
        Some(candidate)
    } else {
        None
    };

    *market = candidate_market;
    *budget = candidate_budget;
    if let Some(commitment) = submit_commitment {
        accepted_commitments.push(commitment);
    }

    QuoteSetLegDecision {
        control,
        blocked_by: None,
        reservation,
    }
}

fn is_submit_action(action: Option<MarketAction>) -> bool {
    matches!(
        action,
        Some(MarketAction::Leg {
            action: LifecycleAction::Submit,
            ..
        })
    )
}

fn blocked(reason: QuoteSetBlockReason) -> QuoteSetLegDecision {
    QuoteSetLegDecision {
        control: no_control_action(),
        blocked_by: Some(reason),
        reservation: None,
    }
}

fn no_control_action() -> QuoteControlDecision {
    QuoteControlDecision {
        action: None,
        blocked_by: None,
        requote_needed: false,
    }
}
