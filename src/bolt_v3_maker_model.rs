//! Glosten-Milgrom / Copeland-Galai adverse-selection quote model for the
//! binary-oracle maker (W3 — Maker model, slice 1: the GM half-spread).
//!
//! This is the maker's *edge*: the half-spread that compensates the maker for
//! the risk that an incoming order is informed. For a binary (Bernoulli-valued)
//! outcome token the Glosten-Milgrom (1985) sequential-trade model has a closed
//! form, so there is nothing to approximate — given the fair probability `p` that
//! the YES outcome resolves true and the fraction `μ` of incoming flow that is
//! informed, the break-even bid and ask are exact Bayesian posteriors:
//!
//! ```text
//! ask = E[V | buy]  = p(1+μ) / [ p(1+μ) + (1−p)(1−μ) ]
//! bid = E[V | sell] = p(1−μ) / [ p(1−μ) + (1−p)(1+μ) ]
//! ```
//!
//! derived from: value `V ∈ {0,1}` with prior `P(V=1)=p`; an informed trader
//! (probability `μ`) buys iff `V=1` and sells iff `V=0`; an uninformed trader
//! (probability `1−μ`) buys or sells with equal probability. The ask is the
//! posterior `P(V=1 | buy)`, the bid is `P(V=1 | sell)`; quoting at these prices
//! makes the maker break even against the informed/uninformed mix, and the
//! half-spread `(ask−bid)/2` is the adverse-selection cost. At `μ=0` the spread
//! collapses to zero (no informed flow → no adverse selection); at `μ=1` it opens
//! to the full `[0,1]` (all flow is informed → the maker cannot quote inside).
//!
//! This is the *primary* spread driver. Avellaneda-Stoikov / GLFT inventory
//! control is deliberately NOT used here: those are unbounded-asset
//! inventory-variance models and are a units category error for a `(0,1)`-bounded
//! binary whose own variance blows up into expiry. Inventory enters only as a
//! secondary skew (a later W3 slice), and a base/processing-cost floor plus the
//! family quote layout (the `maker_quote` family layer) are wired in W3
//! slice 2. `μ` is sourced at runtime from the signed-trade-flow / VPIN estimator;
//! `p` is the family fair value. Pure: no NT type, no hardcoded literal (the
//! probability bounds and the two/half divisor come from
//! [`crate::bolt_v3_numeric`]).

use crate::bolt_v3_numeric::{
    TWO_F64, UNIT_F64, ZERO_F64, is_non_negative_finite, is_positive_finite, sanitize_probability,
};

/// The Glosten-Milgrom reservation band for the YES outcome token: the fair
/// probability `p_up` the band was computed from, together with the break-even
/// `bid = E[V|sell]` and `ask = E[V|buy]` posteriors that straddle it, all in
/// `(0, 1)` probability units. The maker rests its YES quote at `bid` and (when
/// laid out as a two-sided binary quote) its NO quote at `1 − ask`.
///
/// The fields are private and the sole constructor is [`gm_binary_quote`] — a
/// band cannot be assembled from a bare struct literal, so the fair value used to
/// lay out the quote (`p_up`) is definitionally the value its edges were derived
/// from. This is the canonical pricing chain: the quote layout
/// (`compose_binary_legs`) consumes only a band, never three loose scalars that
/// nothing forces to agree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GmReservationBand {
    p_up: f64,
    bid: f64,
    ask: f64,
}

impl GmReservationBand {
    /// The fair probability the YES outcome resolves true (the prior `p`).
    pub fn p_up(&self) -> f64 {
        self.p_up
    }
    /// `E[V | sell]` — the price at which the maker is willing to buy YES.
    pub fn bid(&self) -> f64 {
        self.bid
    }
    /// `E[V | buy]` — the price at which the maker is willing to sell YES.
    pub fn ask(&self) -> f64 {
        self.ask
    }
    /// The adverse-selection half-spread `(ask − bid)/2`, in probability units.
    pub fn half_spread(&self) -> f64 {
        (self.ask - self.bid) / TWO_F64
    }
}

/// The exact Glosten-Milgrom reservation band for a binary outcome token, given
/// the fair probability `fair_p_up` that the YES outcome resolves true and the
/// fraction `informed_fraction` (`μ`) of incoming flow that is informed. This is
/// the **sole** constructor of [`GmReservationBand`].
///
/// Fail-closed (returns `None`, which the engine treats as "no quotable target
/// this tick") when:
/// - `fair_p_up` is not a probability strictly inside `(0, 1)` — at the `{0, 1}`
///   boundaries the outcome is already decided and there is no two-sided quote;
/// - `informed_fraction` is not finite or lies outside `[0, 1]`;
/// - either posterior denominator is non-positive (degenerate mix).
pub fn gm_binary_quote(fair_p_up: f64, informed_fraction: f64) -> Option<GmReservationBand> {
    let p = sanitize_probability(fair_p_up)?;
    // A two-sided quote only exists for an undecided outcome.
    if !(p > ZERO_F64 && p < UNIT_F64) {
        return None;
    }
    let mu = sanitize_probability(informed_fraction)?;

    let p_down = UNIT_F64 - p;
    let informed_up = UNIT_F64 + mu;
    let informed_down = UNIT_F64 - mu;

    // ask = P(V=1 | buy): a buy is over-represented among informed (V=1) flow.
    let buy_up = p * informed_up;
    let buy_denom = buy_up + p_down * informed_down;
    // bid = P(V=1 | sell): a sell is over-represented among informed (V=0) flow.
    let sell_up = p * informed_down;
    let sell_denom = sell_up + p_down * informed_up;
    if !is_positive_finite(buy_denom) || !is_positive_finite(sell_denom) {
        return None;
    }

    Some(GmReservationBand {
        p_up: p,
        bid: sell_up / sell_denom,
        ask: buy_up / buy_denom,
    })
}

/// The Glosten-Milgrom adverse-selection half-spread `(ask − bid)/2` for the
/// binary, in probability units. `None` whenever [`gm_binary_quote`] is `None`.
pub fn gm_half_spread(fair_p_up: f64, informed_fraction: f64) -> Option<f64> {
    Some(gm_binary_quote(fair_p_up, informed_fraction)?.half_spread())
}

/// The secondary inventory skew the maker applies on top of the
/// Glosten-Milgrom half-spread, leaning its quotes to reduce a net position.
///
/// Linear in the net position with a configured `skew_gain` (price units per
/// share): a positive return leans a net-long-YES maker's quotes down on YES and
/// up on NO (see `FamilyQuoteInputs` and
/// `MakerInventory`). The skew is bounded
/// by `position_cap * skew_gain`, since the position itself is capped below.
///
/// Fail-closed (returns `None`) when:
/// - `net_position` is not finite;
/// - `skew_gain` is negative or not finite (a gain of zero disables the skew);
/// - `position_cap` is not a positive finite share count;
/// - `|net_position|` reaches or exceeds `position_cap` — the maker is at its
///   hard inventory limit and must stop quoting to add; the governor reads the
///   `None` as the signal to go reduce-only.
pub fn inventory_skew(net_position: f64, skew_gain: f64, position_cap: f64) -> Option<f64> {
    if !net_position.is_finite() || !is_non_negative_finite(skew_gain) {
        return None;
    }
    if !is_positive_finite(position_cap) {
        return None;
    }
    if net_position.abs() >= position_cap {
        return None;
    }
    Some(net_position * skew_gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for closed-form float comparisons.
    const EPSILON: f64 = 1e-9;
    /// Even-odds fair value used by the symmetry tests.
    const EVEN_ODDS: f64 = 0.5;

    #[test]
    fn no_informed_flow_collapses_the_spread_to_zero() {
        // μ = 0: every trade is uninformed noise, so a buy and a sell are equally
        // (un)informative and the posterior never moves off the prior.
        let quote = gm_binary_quote(0.6, 0.0).expect("interior fair, valid mu");
        assert!((quote.bid() - 0.6).abs() < EPSILON);
        assert!((quote.ask() - 0.6).abs() < EPSILON);
        assert!(gm_half_spread(0.6, 0.0).unwrap() < EPSILON);
    }

    #[test]
    fn fully_informed_flow_opens_the_full_unit_spread() {
        // μ = 1: every trade reveals the outcome, so the maker can only quote the
        // extremes — bid 0, ask 1 — and the half-spread is 1/2.
        let quote = gm_binary_quote(0.6, 1.0).expect("interior fair, valid mu");
        assert!(quote.bid() < EPSILON);
        assert!((quote.ask() - UNIT_F64).abs() < EPSILON);
        assert!((gm_half_spread(0.6, 1.0).unwrap() - 0.5).abs() < EPSILON);
    }

    #[test]
    fn ask_sits_above_fair_and_bid_below_for_interior_informed_flow() {
        let p = 0.6;
        let quote = gm_binary_quote(p, 0.2).expect("interior fair, valid mu");
        assert!(quote.bid() < p, "bid must sit below fair");
        assert!(quote.ask() > p, "ask must sit above fair");
        // Both legs stay inside the (0, 1) probability range.
        assert!(quote.bid() > ZERO_F64 && quote.ask() < UNIT_F64);
    }

    #[test]
    fn even_odds_quote_is_symmetric_about_one_half() {
        // At p = 0.5 the posteriors are mirror images: bid + ask = 1.
        let quote = gm_binary_quote(EVEN_ODDS, 0.3).expect("interior fair, valid mu");
        assert!((quote.bid() + quote.ask() - UNIT_F64).abs() < EPSILON);
        assert!((EVEN_ODDS - quote.bid() - (quote.ask() - EVEN_ODDS)).abs() < EPSILON);
    }

    #[test]
    fn half_spread_is_monotonic_in_informed_fraction() {
        // More toxic flow -> a wider defensive spread, strictly.
        let low = gm_half_spread(0.55, 0.1).unwrap();
        let mid = gm_half_spread(0.55, 0.3).unwrap();
        let high = gm_half_spread(0.55, 0.6).unwrap();
        assert!(low < mid, "spread must widen with informed fraction");
        assert!(mid < high, "spread must widen with informed fraction");
    }

    #[test]
    fn matches_the_closed_form_posterior() {
        // Spot-check against the hand-computed Bayesian posterior at p=0.4, mu=0.5.
        // ask = 0.4*1.5 / (0.4*1.5 + 0.6*0.5) = 0.60 / 0.90 = 2/3
        // bid = 0.4*0.5 / (0.4*0.5 + 0.6*1.5) = 0.20 / 1.10 = 2/11
        let quote = gm_binary_quote(0.4, 0.5).expect("interior fair, valid mu");
        assert!((quote.ask() - 2.0 / 3.0).abs() < EPSILON);
        assert!((quote.bid() - 2.0 / 11.0).abs() < EPSILON);
    }

    #[test]
    fn band_carries_its_fair_and_straddles_it() {
        // The band carries the exact fair it was computed from, and its edges
        // straddle it: bid <= p_up <= ask. This is the canonical-chain invariant
        // the newtype enforces by construction — a bare struct literal cannot
        // assemble a band whose p_up disagrees with its posteriors, so the value
        // used to lay out the quote is definitionally the value its edges came from.
        let band = gm_binary_quote(0.6, 0.2).expect("interior fair, valid mu");
        assert!((band.p_up() - 0.6).abs() < EPSILON);
        assert!(band.bid() <= band.p_up() && band.p_up() <= band.ask());
        // The half_spread accessor agrees with the raw edge difference.
        assert!((band.half_spread() - (band.ask() - band.bid()) / TWO_F64).abs() < EPSILON);
    }

    #[test]
    fn degenerate_inputs_fail_closed() {
        // Decided outcomes have no two-sided quote.
        assert!(gm_binary_quote(ZERO_F64, 0.3).is_none());
        assert!(gm_binary_quote(UNIT_F64, 0.3).is_none());
        // Fair value outside [0, 1].
        assert!(gm_binary_quote(1.5, 0.3).is_none());
        // Informed fraction outside [0, 1] or non-finite.
        assert!(gm_binary_quote(0.5, -0.1).is_none());
        assert!(gm_binary_quote(0.5, 1.1).is_none());
        assert!(gm_binary_quote(0.5, f64::NAN).is_none());
        assert!(gm_half_spread(f64::INFINITY, 0.3).is_none());
    }

    #[test]
    fn inventory_skew_is_linear_and_leans_to_reduce_a_long() {
        // Net long YES -> positive skew (the layout leans YES bid down, NO up).
        let skew = inventory_skew(4.0, 0.01, 100.0).expect("within cap");
        assert!((skew - 0.04).abs() < EPSILON);
        // Linear: doubling the position doubles the skew.
        let double = inventory_skew(8.0, 0.01, 100.0).unwrap();
        assert!((double - 2.0 * skew).abs() < EPSILON);
        // A net short flips the sign.
        assert!(inventory_skew(-4.0, 0.01, 100.0).unwrap() < ZERO_F64);
    }

    #[test]
    fn inventory_skew_zero_gain_disables_the_skew() {
        assert_eq!(inventory_skew(7.0, 0.0, 100.0), Some(0.0));
    }

    #[test]
    fn inventory_skew_fails_closed_beyond_the_position_cap() {
        // Inside the cap it quotes; over the cap the maker must go reduce-only.
        assert!(inventory_skew(49.9999, 0.01, 50.0).is_some());
        assert!(inventory_skew(50.0, 0.01, 50.0).is_none());
        assert!(inventory_skew(50.0001, 0.01, 50.0).is_none());
    }

    #[test]
    fn inventory_skew_fails_closed_on_bad_inputs() {
        assert!(inventory_skew(f64::NAN, 0.01, 100.0).is_none());
        assert!(inventory_skew(1.0, -0.01, 100.0).is_none());
        assert!(inventory_skew(1.0, f64::INFINITY, 100.0).is_none());
        // A non-positive cap is degenerate.
        assert!(inventory_skew(1.0, 0.01, 0.0).is_none());
        assert!(inventory_skew(1.0, 0.01, -5.0).is_none());
    }
}
