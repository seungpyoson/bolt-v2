//! Shared pure maker quote planner.
//!
//! This module is the bridge between the existing maker primitives and the
//! market-family quote seam. It owns no strategy state and performs no NT calls:
//! callers supply the current fair value, toxicity, inventory, top-of-book, and
//! configured quote-layout knobs, then receive family-specific quote targets.

use crate::{
    bolt_v3_maker_microprice::{micro_price, micro_price_anchor},
    bolt_v3_maker_model::{gm_binary_quote, inventory_skew},
    bolt_v3_market_families::maker_quote_targets_for_family,
    bolt_v3_quoting::{FamilyQuoteInputs, QuoteTargets},
};

/// Already-extracted top-of-book inputs for the outcome being anchored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerTopOfBook {
    pub best_bid: f64,
    pub best_ask: f64,
    pub bid_size: f64,
    pub ask_size: f64,
}

/// Config- and state-sourced inputs for one maker quote planning tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerQuotePlanInputs<'a> {
    pub family_key: &'a str,
    pub oracle_fair_probability_up: f64,
    pub informed_fraction: f64,
    pub top_of_book: Option<MakerTopOfBook>,
    pub microprice_weight: f64,
    pub net_position: f64,
    pub inventory_skew_gain: f64,
    pub position_cap: f64,
    pub half_spread_floor: f64,
    pub max_half_spread: f64,
    pub eps: f64,
    pub tau: f64,
    pub reference_tau: f64,
    pub time_widen_cap: f64,
    pub order_notional_target: f64,
    pub maximum_position_notional: f64,
}

/// Planned family-specific maker quote targets plus the intermediate economic
/// values that explain the plan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerQuotePlan {
    pub fair_probability_up: f64,
    pub reservation_bid: f64,
    pub reservation_ask: f64,
    pub inventory_skew: f64,
    pub targets: QuoteTargets,
}

pub fn plan_maker_quote_targets(inputs: MakerQuotePlanInputs<'_>) -> Option<MakerQuotePlan> {
    let micro = inputs
        .top_of_book
        .and_then(|book| micro_price(book.best_bid, book.best_ask, book.bid_size, book.ask_size));
    let fair_probability_up = micro_price_anchor(
        inputs.oracle_fair_probability_up,
        micro,
        inputs.microprice_weight,
    )?;
    let reservation = gm_binary_quote(fair_probability_up, inputs.informed_fraction)?;
    let inventory_skew = inventory_skew(
        inputs.net_position,
        inputs.inventory_skew_gain,
        inputs.position_cap,
    )?;
    let targets = maker_quote_targets_for_family(
        inputs.family_key,
        FamilyQuoteInputs {
            band: reservation,
            inventory_skew,
            half_spread_floor: inputs.half_spread_floor,
            max_half_spread: inputs.max_half_spread,
            eps: inputs.eps,
            tau: inputs.tau,
            reference_tau: inputs.reference_tau,
            time_widen_cap: inputs.time_widen_cap,
            order_notional_target: inputs.order_notional_target,
            maximum_position_notional: inputs.maximum_position_notional,
        },
    )?;

    Some(MakerQuotePlan {
        fair_probability_up,
        reservation_bid: reservation.bid(),
        reservation_ask: reservation.ask(),
        inventory_skew,
        targets,
    })
}
