//! Instrument-agnostic quote-target layout for the maker (W2 / WG / W3 slice 2).
//!
//! The maker's *quote layout* — given a fair value, a reservation band, and an
//! inventory skew, where do the two legs rest — is the one piece that genuinely
//! differs by instrument type, so it sits behind the [`MakerFamily`] seam. A
//! binary family lays out two BIDS on the YES/NO outcome tokens around P(up); a
//! linear-perp family lays out a BID and an ASK around the mid. The agnostic
//! engine ([`crate::strategies::quote_lifecycle::MarketQuote`] +
//! [`crate::strategies::requote_budget`]) consumes the resulting [`QuoteTargets`]
//! identically, so adding an instrument type never touches the engine — only a
//! new `MakerFamily` impl.
//!
//! ## The reservation band (W3 slice 2)
//!
//! Each family is fed a [`FamilyQuoteInputs`]: a `fair` value, a pre-skew
//! reservation `[reservation_bid, reservation_ask]` produced by the family's
//! *spread model* (the Glosten-Milgrom posteriors for a binary; mid ± a lean
//! half-spread for a perp), an `inventory_skew`, and a `[half_spread_floor,
//! max_half_spread]` band. Every family resolves that band through the single
//! shared [`resolve_band`] free function, so a crossed or out-of-bound band can
//! never be silently accepted by one instrument type while another rejects it —
//! the fail-closed band logic lives in exactly one place. This module owns only
//! the *layout* and the *band* — fair value and the spread model stay one layer
//! up (binary via the `MarketFamily` digital model + the
//! [`crate::strategies::maker_model`] Glosten-Milgrom spread; perp via the
//! order-book mid) and are fed in already computed.
//!
//! The lean perp family here is the WG-scope proof of agnosticism; funding,
//! margin/liquidation, mark/index, the optimal-spread model, and settlement are
//! the production gap recorded in `specs/488-binary-oracle-maker/plan.md`
//! (workstream WG) — it is NOT live-tradeable. Pure: no NT type, no hardcoded
//! literal (probability/price bounds and the two/half divisor come from
//! [`crate::bolt_v3_numeric`]).

use crate::bolt_v3_numeric::{
    TWO_F64, UNIT_F64, ZERO_F64, is_positive_finite, sanitize_probability,
};

/// The side a quote leg rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteSide {
    /// A resting bid (the maker buys if hit).
    Buy,
    /// A resting ask/offer (the maker sells if lifted).
    Sell,
}

/// One leg of a two-sided quote: a side and the limit price to rest at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteTargetLeg {
    pub side: QuoteSide,
    pub price: f64,
}

/// The two legs a maker wants resting, produced by an instrument family.
///
/// `leg_a` maps to [`crate::strategies::quote_lifecycle::Leg::Yes`], `leg_b` to
/// `Leg::No`, so the engine drives both regardless of instrument type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteTargets {
    pub leg_a: QuoteTargetLeg,
    pub leg_b: QuoteTargetLeg,
}

/// The inputs one family needs to lay out a two-sided quote for a single tick.
///
/// Constructed explicitly by the engine every tick — there is deliberately no
/// `Default`, because a maker must never quote off zeroed inputs (a zero
/// reservation band is a guaranteed-loss quote). All fields are in the family's
/// own price units (probability for binary, price for perp) and are
/// config/model-sourced f64s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FamilyQuoteInputs {
    /// The family fair value (P(up) for a binary, the mid for a perp). The
    /// reservation band must bracket it; see [`resolve_band`].
    pub fair: f64,
    /// The pre-skew reservation bid from the family's spread model — the price at
    /// which the model is willing to buy (Glosten-Milgrom `E[V | sell]` for a
    /// binary; `mid − lean_half_spread` for a perp).
    pub reservation_bid: f64,
    /// The pre-skew reservation ask from the family's spread model — the price at
    /// which the model is willing to sell (`E[V | buy]` for a binary;
    /// `mid + lean_half_spread` for a perp).
    pub reservation_ask: f64,
    /// The secondary inventory skew (family price units): positive means the
    /// maker is net long and leans its quotes to reduce.
    pub inventory_skew: f64,
    /// The minimum half-spread — the base/processing + fee floor. The resolved
    /// band is never tighter than this, so the maker never quotes below cost.
    pub half_spread_floor: f64,
    /// The maximum half-spread. A reservation wider than this is degenerate
    /// (stale or toxic) and yields no quote this tick.
    pub max_half_spread: f64,
}

/// The single source of the fail-closed reservation band, shared by every
/// instrument family so a degenerate band can never slip through one family
/// while another rejects it (this is the class fix for the binary/perp
/// dual-path: previously the binary family silently accepted a negative band).
///
/// Given a model reservation `[reservation_bid, reservation_ask]` that must
/// bracket `fair`, enforce the `[floor, cap]` half-spread band and return the
/// floored/capped `(bid, ask)` centred on the reservation mid.
///
/// Returns `None` (fail-closed — the engine treats it as "no quotable target
/// this tick") when:
/// - any input is non-finite;
/// - the `floor` is negative or the `cap` is below the `floor` (misconfigured);
/// - the reservation does not bracket `fair` — a crossed band, or one that puts
///   both quotes on the same side of fair, is a guaranteed-loss layout;
/// - the resolved half-spread exceeds the `cap`.
pub fn resolve_band(
    fair: f64,
    reservation_bid: f64,
    reservation_ask: f64,
    half_spread_floor: f64,
    max_half_spread: f64,
) -> Option<(f64, f64)> {
    if !fair.is_finite()
        || !reservation_bid.is_finite()
        || !reservation_ask.is_finite()
        || !half_spread_floor.is_finite()
        || !max_half_spread.is_finite()
    {
        return None;
    }
    // A floor below zero (quoting below cost) or a cap below the floor (an empty
    // band) is a misconfiguration, not a tick to quote on.
    if half_spread_floor < ZERO_F64 || max_half_spread < half_spread_floor {
        return None;
    }
    // The reservation must bracket the fair value. A crossed band
    // (`reservation_bid > reservation_ask`) — e.g. a negative model half-spread
    // that puts the bid above fair — fails this and is rejected for every family
    // alike, closing the prior binary-only fail-open gap.
    if !(reservation_bid <= fair && fair <= reservation_ask) {
        return None;
    }
    let half_spread = (reservation_ask - reservation_bid) / TWO_F64;
    if half_spread < ZERO_F64 {
        return None;
    }
    // Never quote tighter than the cost floor; reject a band wider than the cap.
    let half_spread = half_spread.max(half_spread_floor);
    if half_spread > max_half_spread {
        return None;
    }
    let mid = (reservation_bid + reservation_ask) / TWO_F64;
    Some((mid - half_spread, mid + half_spread))
}

/// Lays out the two quote legs around a fair value for one instrument type.
///
/// The engine is agnostic to which impl it holds; it only consumes
/// [`QuoteTargets`]. Adding an instrument type is a new impl here, nothing in the
/// engine.
pub trait MakerFamily {
    /// Lay out the two legs from `inputs`. Returns `None` when the inputs are
    /// degenerate — fail-closed, so the engine treats `None` as "no quotable
    /// target this tick".
    fn quote_targets(&self, inputs: FamilyQuoteInputs) -> Option<QuoteTargets>;
}

/// Binary (YES/NO outcome-token) family: both legs are BIDS, one on each token,
/// around P(up) and P(down) = 1 − P(up). `fair` is P(up) in `[0, 1]` and the
/// reservation is the Glosten-Milgrom posterior band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinaryFamily;

impl MakerFamily for BinaryFamily {
    fn quote_targets(&self, inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
        // The binary fair value and reservation are probabilities.
        let p_up = sanitize_probability(inputs.fair)?;
        let (resolved_bid, resolved_ask) = resolve_band(
            p_up,
            inputs.reservation_bid,
            inputs.reservation_ask,
            inputs.half_spread_floor,
            inputs.max_half_spread,
        )?;
        // Lay out two BIDS: YES around the resolved bid, NO around 1 − resolved
        // ask. A positive inventory skew (net long YES) lowers the YES bid and
        // raises the NO bid, leaning the pair toward the lighter side.
        let yes_price = sanitize_probability(resolved_bid - inputs.inventory_skew)?;
        let no_price = sanitize_probability((UNIT_F64 - resolved_ask) + inputs.inventory_skew)?;
        if !is_positive_finite(yes_price) || !is_positive_finite(no_price) {
            return None;
        }
        // The two bids must leave positive edge: a YES bid plus a NO bid summing
        // to ≥ 1 is a guaranteed loss, since exactly one token resolves to 1. The
        // floor on the half-spread normally guarantees this; this is the final
        // fail-closed net.
        if yes_price + no_price >= UNIT_F64 {
            return None;
        }
        Some(QuoteTargets {
            leg_a: QuoteTargetLeg {
                side: QuoteSide::Buy,
                price: yes_price,
            },
            leg_b: QuoteTargetLeg {
                side: QuoteSide::Buy,
                price: no_price,
            },
        })
    }
}

/// Linear perpetual-futures family: a BID and an ASK on one instrument around the
/// `fair` mid. Lean (a model half-spread + linear inventory skew) — the
/// production gap (funding, margin, mark/index, optimal spread, settlement) is in
/// the WG plan; this is an architecture proof, not a live perp maker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearPerpFamily;

impl MakerFamily for LinearPerpFamily {
    fn quote_targets(&self, inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
        let (resolved_bid, resolved_ask) = resolve_band(
            inputs.fair,
            inputs.reservation_bid,
            inputs.reservation_ask,
            inputs.half_spread_floor,
            inputs.max_half_spread,
        )?;
        // Skew shifts both quotes against inventory (long -> lean down to sell).
        let bid = resolved_bid - inputs.inventory_skew;
        let ask = resolved_ask - inputs.inventory_skew;
        if !is_positive_finite(bid) || bid >= ask {
            return None;
        }
        Some(QuoteTargets {
            leg_a: QuoteTargetLeg {
                side: QuoteSide::Buy,
                price: bid,
            },
            leg_b: QuoteTargetLeg {
                side: QuoteSide::Sell,
                price: ask,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::maker_model::gm_binary_quote;

    /// Tolerance for closed-form float comparisons.
    const EPSILON: f64 = 1e-9;

    /// Build inputs with a symmetric reservation band of `half_spread` around
    /// `fair` and a permissive `[0, 1]` floor/cap, for the layout tests that only
    /// exercise the geometry (not the floor/cap gates).
    fn symmetric_inputs(fair: f64, half_spread: f64, skew: f64) -> FamilyQuoteInputs {
        FamilyQuoteInputs {
            fair,
            reservation_bid: fair - half_spread,
            reservation_ask: fair + half_spread,
            inventory_skew: skew,
            half_spread_floor: 0.0,
            max_half_spread: 1.0,
        }
    }

    /// The engine holds a family as a trait object and consumes the same
    /// `QuoteTargets` shape regardless of instrument type.
    fn lay_out(family: &dyn MakerFamily, fair: f64) -> Option<QuoteTargets> {
        family.quote_targets(symmetric_inputs(fair, 0.02, 0.0))
    }

    #[test]
    fn binary_lays_out_two_bids_around_fair() {
        let targets = BinaryFamily
            .quote_targets(symmetric_inputs(0.60, 0.02, 0.0))
            .expect("non-degenerate binary inputs should quote");
        // Both legs are bids on the two outcome tokens.
        assert_eq!(targets.leg_a.side, QuoteSide::Buy);
        assert_eq!(targets.leg_b.side, QuoteSide::Buy);
        // YES bid sits below P(up)=0.60, NO bid below P(down)=0.40.
        assert!(targets.leg_a.price < 0.60 && targets.leg_a.price > 0.50);
        assert!(targets.leg_b.price < 0.40 && targets.leg_b.price > 0.30);
    }

    #[test]
    fn binary_inventory_skew_leans_yes_below_no_adjustment() {
        let flat = BinaryFamily
            .quote_targets(symmetric_inputs(0.50, 0.02, 0.0))
            .unwrap();
        let long_yes = BinaryFamily
            .quote_targets(symmetric_inputs(0.50, 0.02, 0.05))
            .unwrap();
        // A positive skew (long YES) lowers the YES bid and raises the NO bid.
        assert!(long_yes.leg_a.price < flat.leg_a.price);
        assert!(long_yes.leg_b.price > flat.leg_b.price);
    }

    #[test]
    fn binary_degenerate_inputs_return_none() {
        // Fair probability outside [0,1].
        assert!(
            BinaryFamily
                .quote_targets(symmetric_inputs(1.5, 0.02, 0.0))
                .is_none()
        );
        // A spread so wide the YES bid would be non-positive.
        assert!(
            BinaryFamily
                .quote_targets(symmetric_inputs(0.01, 0.5, 0.0))
                .is_none()
        );
        // Non-finite spread -> non-finite reservation.
        assert!(
            BinaryFamily
                .quote_targets(symmetric_inputs(0.5, f64::NAN, 0.0))
                .is_none()
        );
    }

    #[test]
    fn perp_lays_out_bid_and_ask_around_mid() {
        let targets = LinearPerpFamily
            .quote_targets(symmetric_inputs(100.0, 0.5, 0.0))
            .expect("non-degenerate perp inputs should quote");
        assert_eq!(targets.leg_a.side, QuoteSide::Buy);
        assert_eq!(targets.leg_b.side, QuoteSide::Sell);
        assert!((targets.leg_a.price - 99.5).abs() < EPSILON);
        assert!((targets.leg_b.price - 100.5).abs() < EPSILON);
        assert!(targets.leg_a.price < targets.leg_b.price);
    }

    #[test]
    fn perp_inventory_skew_leans_both_quotes_down() {
        let flat = LinearPerpFamily
            .quote_targets(symmetric_inputs(100.0, 0.5, 0.0))
            .unwrap();
        let long = LinearPerpFamily
            .quote_targets(symmetric_inputs(100.0, 0.5, 0.3))
            .unwrap();
        assert!(long.leg_a.price < flat.leg_a.price);
        assert!(long.leg_b.price < flat.leg_b.price);
    }

    #[test]
    fn perp_degenerate_inputs_return_none() {
        // Non-positive mid.
        assert!(
            LinearPerpFamily
                .quote_targets(symmetric_inputs(0.0, 0.5, 0.0))
                .is_none()
        );
        // Skew so large the bid goes non-positive.
        assert!(
            LinearPerpFamily
                .quote_targets(symmetric_inputs(1.0, 0.1, 5.0))
                .is_none()
        );
        // Non-finite mid.
        assert!(
            LinearPerpFamily
                .quote_targets(symmetric_inputs(f64::INFINITY, 0.5, 0.0))
                .is_none()
        );
    }

    #[test]
    fn engine_consumes_either_family_through_the_same_trait_object() {
        // Same call site, two instrument types — proving the engine is agnostic.
        let binary = lay_out(&BinaryFamily, 0.60).expect("binary quotes");
        let perp = lay_out(&LinearPerpFamily, 100.0).expect("perp quotes");
        // Binary is two bids; perp is a bid and an ask. The engine sees only
        // QuoteTargets in both cases.
        assert_eq!(binary.leg_a.side, QuoteSide::Buy);
        assert_eq!(binary.leg_b.side, QuoteSide::Buy);
        assert_eq!(perp.leg_a.side, QuoteSide::Buy);
        assert_eq!(perp.leg_b.side, QuoteSide::Sell);
    }

    // ---- W3 slice 2: the shared resolve_band ----

    #[test]
    fn resolve_band_centers_floored_band_on_the_reservation_mid() {
        // Reservation [0.58, 0.62] around fair 0.60, floor below the model spread.
        let (bid, ask) = resolve_band(0.60, 0.58, 0.62, 0.0, 1.0).expect("valid band");
        assert!((bid - 0.58).abs() < EPSILON);
        assert!((ask - 0.62).abs() < EPSILON);
    }

    #[test]
    fn resolve_band_enforces_the_half_spread_floor() {
        // Model spread is 0.005 each side but the cost floor is 0.02: the band is
        // widened symmetrically to the floor, never quoted below cost.
        let (bid, ask) = resolve_band(0.50, 0.495, 0.505, 0.02, 1.0).expect("floored band");
        assert!((bid - 0.48).abs() < EPSILON, "bid floored to mid-0.02");
        assert!((ask - 0.52).abs() < EPSILON, "ask floored to mid+0.02");
    }

    #[test]
    fn resolve_band_rejects_a_band_wider_than_the_cap() {
        // Half-spread 0.10 exceeds the 0.05 cap -> no quote (stale/toxic band).
        assert!(resolve_band(0.50, 0.40, 0.60, 0.0, 0.05).is_none());
    }

    #[test]
    fn resolve_band_rejects_a_reservation_that_does_not_bracket_fair() {
        // Crossed band: bid above ask.
        assert!(resolve_band(0.50, 0.70, 0.30, 0.0, 1.0).is_none());
        // Both quotes above fair (model disagrees with fair) -> fail-closed.
        assert!(resolve_band(0.40, 0.60, 0.70, 0.0, 1.0).is_none());
        // Both quotes below fair.
        assert!(resolve_band(0.60, 0.30, 0.40, 0.0, 1.0).is_none());
    }

    #[test]
    fn resolve_band_rejects_non_finite_inputs_and_inverted_floor_cap() {
        assert!(resolve_band(f64::NAN, 0.4, 0.6, 0.0, 1.0).is_none());
        assert!(resolve_band(0.5, f64::INFINITY, 0.6, 0.0, 1.0).is_none());
        // Negative floor = quoting below cost.
        assert!(resolve_band(0.5, 0.4, 0.6, -0.01, 1.0).is_none());
        // Cap below floor = an empty band.
        assert!(resolve_band(0.5, 0.4, 0.6, 0.10, 0.05).is_none());
    }

    // ---- W3 slice 2: the Glosten-Milgrom model feeds the binary layout ----

    #[test]
    fn gm_model_feeds_the_binary_reservation_into_the_layout() {
        // The spread model lives one layer up: the GM posteriors become the
        // reservation band, then the family lays the two bids out.
        let gm = gm_binary_quote(0.4, 0.5).expect("interior fair, valid mu");
        let inputs = FamilyQuoteInputs {
            fair: 0.4,
            reservation_bid: gm.bid,
            reservation_ask: gm.ask,
            inventory_skew: 0.0,
            half_spread_floor: 0.0,
            max_half_spread: 1.0,
        };
        let targets = BinaryFamily.quote_targets(inputs).expect("gm band quotes");
        // YES rests at the GM bid; NO rests at 1 − GM ask. (gm.bid=2/11, gm.ask=2/3.)
        assert!((targets.leg_a.price - gm.bid).abs() < EPSILON);
        assert!((targets.leg_b.price - (UNIT_F64 - gm.ask)).abs() < EPSILON);
        // Positive edge: the two bids sum below 1.
        assert!(targets.leg_a.price + targets.leg_b.price < UNIT_F64);
    }

    // ---- W3 slice 2: gm-3 — the binary/perp fail-closed dual-path is closed ----

    #[test]
    fn both_families_reject_the_same_crossed_band() {
        // A negative model half-spread (bid above ask) used to be silently
        // accepted by the binary family and rejected only by the perp family.
        // Both now reject it through the shared resolve_band.
        let crossed = FamilyQuoteInputs {
            fair: 0.50,
            reservation_bid: 0.70,
            reservation_ask: 0.30,
            inventory_skew: 0.0,
            half_spread_floor: 0.0,
            max_half_spread: 1.0,
        };
        assert!(
            BinaryFamily.quote_targets(crossed).is_none(),
            "binary must reject a crossed band (gm-3 regression)"
        );
        assert!(
            LinearPerpFamily.quote_targets(crossed).is_none(),
            "perp must reject the same crossed band"
        );
    }

    #[test]
    fn binary_sum_guard_rejects_a_zero_edge_band() {
        // Floor 0 and a collapsed reservation [fair, fair]: half-spread 0, so the
        // two bids would sum to exactly 1 (zero edge) -> fail-closed.
        let zero_edge = FamilyQuoteInputs {
            fair: 0.50,
            reservation_bid: 0.50,
            reservation_ask: 0.50,
            inventory_skew: 0.0,
            half_spread_floor: 0.0,
            max_half_spread: 1.0,
        };
        assert!(BinaryFamily.quote_targets(zero_edge).is_none());
    }

    #[test]
    fn binary_floor_guarantees_positive_edge_on_a_zero_model_spread() {
        // Same collapsed model band, but a positive cost floor widens it so the
        // two bids leave edge (sum < 1).
        let floored = FamilyQuoteInputs {
            fair: 0.50,
            reservation_bid: 0.50,
            reservation_ask: 0.50,
            inventory_skew: 0.0,
            half_spread_floor: 0.02,
            max_half_spread: 1.0,
        };
        let targets = BinaryFamily
            .quote_targets(floored)
            .expect("floored band quotes");
        assert!(targets.leg_a.price + targets.leg_b.price < UNIT_F64);
        // Symmetric at fair 0.50: each bid is 0.48.
        assert!((targets.leg_a.price - 0.48).abs() < EPSILON);
        assert!((targets.leg_b.price - 0.48).abs() < EPSILON);
    }
}
