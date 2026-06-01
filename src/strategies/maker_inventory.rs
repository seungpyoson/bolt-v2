//! Net directional inventory for the binary-oracle maker (W3 — Maker model:
//! inventory accounting).
//!
//! A two-sided maker accumulates a position every time a resting quote is hit.
//! Without tracking it, the `inventory_skew` fed to the quote layout
//! ([`crate::strategies::maker_quote::FamilyQuoteInputs`]) is a free input that
//! nothing produces, and the maker has no net-position → skew feedback loop —
//! directional risk grows unbounded into a binary's settlement. This module is
//! that book: it folds each fill into a signed net position, which the maker
//! model's [`crate::strategies::maker_model::inventory_skew`] turns into the
//! secondary skew, and which the governor reads to go reduce-only at a cap.
//!
//! Pure: no NautilusTrader type. NT remains the owner of fills/positions; the NT
//! handler feeds confirmed fills in here. The sign convention is directional in
//! the leg-a ("up"/YES) outcome: a YES buy lengthens the position, a NO buy
//! shortens it (a NO share is the anti-YES outcome), and sells reverse — so the
//! net is the directional imbalance a binary maker actually carries, not a raw
//! share count. No hardcoded literal (zero comes from
//! [`crate::bolt_v3_numeric`]).

use crate::bolt_v3_numeric::{ZERO_F64, is_positive_finite};
use crate::strategies::maker_quote::QuoteSide;
use crate::strategies::quote_lifecycle::Leg;

/// The maker's net directional inventory, accumulated from fills.
///
/// Constructed flat via [`MakerInventory::flat`] (no `Default`: the bolt-v3
/// legacy-default fence forbids a `Default` impl on the production surface, and a
/// maker must name its starting book rather than inherit a zeroed one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerInventory {
    /// Net directional position in base/share units. Positive = net long the
    /// YES (leg-a / "up") outcome — i.e. more YES exposure than NO.
    net_position: f64,
}

impl MakerInventory {
    /// A flat book — zero net position.
    pub fn flat() -> Self {
        Self {
            net_position: ZERO_F64,
        }
    }

    /// The current net directional position (positive = net long YES).
    pub fn net_position(&self) -> f64 {
        self.net_position
    }

    /// Fold one confirmed fill into the book. A YES buy lengthens the position,
    /// a NO buy shortens it (NO is the anti-YES outcome); a sell on either leg
    /// reverses its sign.
    ///
    /// Returns `false` and leaves the book unchanged when `qty` is not a positive
    /// finite share count — fail-closed against a malformed fill (a zero or
    /// negative quantity is never a real fill and must not silently corrupt the
    /// net position).
    ///
    /// Accumulation is raw f64: over a long fill history `net_position` can drift
    /// by a floating-point epsilon, which only ever trips the downstream
    /// reduce-only cap ([`crate::strategies::maker_model::inventory_skew`])
    /// marginally *early* — the fail-closed direction. Exact on-grid quantisation
    /// (so the cap comparison is reproducible regardless of fill order) arrives
    /// when fills are sized through NautilusTrader's `make_qty` in the live shell;
    /// it is intentionally not duplicated here (NT owns size/tick rounding).
    pub fn apply_fill(&mut self, leg: Leg, side: QuoteSide, qty: f64) -> bool {
        if !is_positive_finite(qty) {
            return false;
        }
        let signed = match (leg, side) {
            (Leg::Yes, QuoteSide::Buy) => qty,
            (Leg::Yes, QuoteSide::Sell) => -qty,
            (Leg::No, QuoteSide::Buy) => -qty,
            (Leg::No, QuoteSide::Sell) => qty,
        };
        self.net_position += signed;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for accumulated-float comparisons.
    const EPSILON: f64 = 1e-9;

    #[test]
    fn a_fresh_book_is_flat() {
        assert_eq!(MakerInventory::flat().net_position(), 0.0);
    }

    #[test]
    fn yes_buy_lengthens_and_no_buy_shortens_the_position() {
        let mut book = MakerInventory::flat();
        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, 3.0));
        assert!((book.net_position() - 3.0).abs() < EPSILON);
        // A NO buy is the anti-YES outcome -> shortens the directional position.
        assert!(book.apply_fill(Leg::No, QuoteSide::Buy, 2.0));
        assert!((book.net_position() - 1.0).abs() < EPSILON);
    }

    #[test]
    fn sells_reverse_each_leg_sign() {
        let mut book = MakerInventory::flat();
        book.apply_fill(Leg::Yes, QuoteSide::Buy, 5.0);
        // Selling YES (an exit) reduces the long-YES position.
        assert!(book.apply_fill(Leg::Yes, QuoteSide::Sell, 2.0));
        assert!((book.net_position() - 3.0).abs() < EPSILON);
        // Selling NO lengthens the directional (YES-equivalent) position.
        assert!(book.apply_fill(Leg::No, QuoteSide::Sell, 1.0));
        assert!((book.net_position() - 4.0).abs() < EPSILON);
    }

    #[test]
    fn a_matched_pair_nets_to_flat() {
        // Buying equal YES and NO is directionally riskless: net returns to zero.
        let mut book = MakerInventory::flat();
        book.apply_fill(Leg::Yes, QuoteSide::Buy, 4.0);
        book.apply_fill(Leg::No, QuoteSide::Buy, 4.0);
        assert!(book.net_position().abs() < EPSILON);
    }

    #[test]
    fn a_non_positive_or_non_finite_qty_is_rejected_and_leaves_the_book_unchanged() {
        let mut book = MakerInventory::flat();
        book.apply_fill(Leg::Yes, QuoteSide::Buy, 2.0);
        assert!(!book.apply_fill(Leg::Yes, QuoteSide::Buy, 0.0));
        assert!(!book.apply_fill(Leg::Yes, QuoteSide::Buy, -1.0));
        assert!(!book.apply_fill(Leg::No, QuoteSide::Buy, f64::NAN));
        assert!(!book.apply_fill(Leg::No, QuoteSide::Buy, f64::INFINITY));
        // The one valid fill is all that registered.
        assert!((book.net_position() - 2.0).abs() < EPSILON);
    }
}
