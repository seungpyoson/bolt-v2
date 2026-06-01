//! Instrument-agnostic quote-target layout for the maker (W2 / WG).
//!
//! The maker's *quote layout* — given a fair value, a half-spread, and an
//! inventory skew, where do the two legs rest — is the one piece that genuinely
//! differs by instrument type, so it sits behind the [`MakerFamily`] seam. A
//! binary family lays out two BIDS on the YES/NO outcome tokens around P(up); a
//! linear-perp family lays out a BID and an ASK around the mid. The agnostic
//! engine ([`crate::strategies::quote_lifecycle::MarketQuote`] +
//! [`crate::strategies::requote_budget`]) consumes the resulting [`QuoteTargets`]
//! identically, so adding an instrument type never touches the engine — only a
//! new `MakerFamily` impl.
//!
//! This module owns only the *layout*. Fair-value computation stays
//! family-specific (binary via the `MarketFamily` digital model; perp via the
//! order-book mid) and is fed in as `fair`. The lean perp family here is the
//! WG-scope proof of agnosticism; funding, margin/liquidation, mark/index, the
//! optimal-spread model, and settlement are the production gap recorded in
//! `specs/488-binary-oracle-maker/plan.md` (workstream WG) — it is NOT
//! live-tradeable. Pure: no NT type, no hardcoded literal (probability/price
//! bounds come from [`crate::bolt_v3_numeric`]).

use crate::bolt_v3_numeric::{UNIT_F64, is_positive_finite, sanitize_probability};

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

/// Lays out the two quote legs around a fair value for one instrument type.
///
/// The engine is agnostic to which impl it holds; it only consumes
/// [`QuoteTargets`]. Adding an instrument type is a new impl here, nothing in the
/// engine.
pub trait MakerFamily {
    /// Lay out the two legs around `fair`, given the maker's `half_spread` and an
    /// `inventory_skew` adjustment (both in the family's price units). Returns
    /// `None` when the inputs are degenerate — fail-closed, so the engine treats
    /// `None` as "no quotable target this tick".
    fn quote_targets(
        &self,
        fair: f64,
        half_spread: f64,
        inventory_skew: f64,
    ) -> Option<QuoteTargets>;
}

/// Binary (YES/NO outcome-token) family: both legs are BIDS, one on each token,
/// around P(up) and P(down) = 1 − P(up). `fair` is P(up) in [0, 1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinaryFamily;

impl MakerFamily for BinaryFamily {
    fn quote_targets(
        &self,
        fair: f64,
        half_spread: f64,
        inventory_skew: f64,
    ) -> Option<QuoteTargets> {
        let p_up = sanitize_probability(fair)?;
        let p_down = UNIT_F64 - p_up;
        // Bid each outcome token below its fair probability to earn the spread;
        // the inventory skew leans the pair toward the lighter side.
        let yes_price = sanitize_probability(p_up - half_spread - inventory_skew)?;
        let no_price = sanitize_probability(p_down - half_spread + inventory_skew)?;
        if !is_positive_finite(yes_price) || !is_positive_finite(no_price) {
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
/// `fair` mid. Lean (fixed half-spread + linear inventory skew) — the production
/// gap (funding, margin, mark/index, optimal spread, settlement) is in the WG
/// plan; this is an architecture proof, not a live perp maker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinearPerpFamily;

impl MakerFamily for LinearPerpFamily {
    fn quote_targets(
        &self,
        fair: f64,
        half_spread: f64,
        inventory_skew: f64,
    ) -> Option<QuoteTargets> {
        if !is_positive_finite(fair) || !half_spread.is_finite() || !inventory_skew.is_finite() {
            return None;
        }
        // Skew shifts both quotes against inventory (long -> lean down to sell).
        let bid = fair - half_spread - inventory_skew;
        let ask = fair + half_spread - inventory_skew;
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

    /// The engine holds a family as a trait object and consumes the same
    /// `QuoteTargets` shape regardless of instrument type.
    fn lay_out(family: &dyn MakerFamily, fair: f64) -> Option<QuoteTargets> {
        family.quote_targets(fair, 0.02, 0.0)
    }

    #[test]
    fn binary_lays_out_two_bids_around_fair() {
        let targets = BinaryFamily
            .quote_targets(0.60, 0.02, 0.0)
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
        let flat = BinaryFamily.quote_targets(0.50, 0.02, 0.0).unwrap();
        let long_yes = BinaryFamily.quote_targets(0.50, 0.02, 0.05).unwrap();
        // A positive skew (long YES) lowers the YES bid and raises the NO bid.
        assert!(long_yes.leg_a.price < flat.leg_a.price);
        assert!(long_yes.leg_b.price > flat.leg_b.price);
    }

    #[test]
    fn binary_degenerate_inputs_return_none() {
        // Fair probability outside [0,1].
        assert!(BinaryFamily.quote_targets(1.5, 0.02, 0.0).is_none());
        // A spread so wide the bid would be non-positive.
        assert!(BinaryFamily.quote_targets(0.01, 0.5, 0.0).is_none());
        // Non-finite spread.
        assert!(BinaryFamily.quote_targets(0.5, f64::NAN, 0.0).is_none());
    }

    #[test]
    fn perp_lays_out_bid_and_ask_around_mid() {
        let targets = LinearPerpFamily
            .quote_targets(100.0, 0.5, 0.0)
            .expect("non-degenerate perp inputs should quote");
        assert_eq!(targets.leg_a.side, QuoteSide::Buy);
        assert_eq!(targets.leg_b.side, QuoteSide::Sell);
        assert_eq!(targets.leg_a.price, 99.5);
        assert_eq!(targets.leg_b.price, 100.5);
        assert!(targets.leg_a.price < targets.leg_b.price);
    }

    #[test]
    fn perp_inventory_skew_leans_both_quotes_down() {
        let flat = LinearPerpFamily.quote_targets(100.0, 0.5, 0.0).unwrap();
        let long = LinearPerpFamily.quote_targets(100.0, 0.5, 0.3).unwrap();
        assert!(long.leg_a.price < flat.leg_a.price);
        assert!(long.leg_b.price < flat.leg_b.price);
    }

    #[test]
    fn perp_degenerate_inputs_return_none() {
        // Non-positive mid.
        assert!(LinearPerpFamily.quote_targets(0.0, 0.5, 0.0).is_none());
        // Skew so large the bid goes non-positive.
        assert!(LinearPerpFamily.quote_targets(1.0, 0.1, 5.0).is_none());
        // Non-finite mid.
        assert!(
            LinearPerpFamily
                .quote_targets(f64::INFINITY, 0.5, 0.0)
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
}
