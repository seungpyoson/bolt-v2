//! Pure portfolio-risk aggregator for the binary-oracle maker portfolio layer
//! (W5 — FR-041: roll each market's per-market reserved-collateral scalar up to
//! one portfolio cap and admit/deny the next reservation, fail-closed).
//!
//! This module owns the "per-market caps rolling up to a portfolio budget"
//! invariant. It holds, per market, the most recent reserved-collateral scalar
//! and the running total, and answers exactly one question: would committing a
//! market's next reservation push the portfolio's total reserved collateral past
//! the portfolio cap? If yes, the new quote is denied.
//!
//! ## Unit contract — fee-inclusive USDC, single source, no recompute
//!
//! CRITICAL (NO DUAL PATHS): this aggregator **consumes** each per-market
//! reservation as an owned `f64` and never recomputes it. The sole producer of
//! that scalar is [`crate::strategies::maker_reservation::worst_case_reservation`]
//! (the FR-040 per-market worst-case-simultaneous-fill gate). That function
//! returns the worst-case buy outflow **already grossed up by the venue fee
//! multiplier** — i.e. the real cash the venue would move. Therefore every
//! `reservation` passed to [`PortfolioReservations::try_reserve`] is in
//! **fee-inclusive USDC**, and the `portfolio_cap` carried by
//! [`PortfolioRiskCap`] MUST be expressed in the **same fee-inclusive USDC**
//! unit. The aggregator only sums and compares; it never re-derives a per-market
//! reservation, so there is one and only one place that defines the scalar's
//! units and fee gross-up (FR-040). Summing fee-inclusive scalars against a
//! fee-inclusive cap keeps the portfolio bound exact.
//!
//! ## Replace-not-add
//!
//! [`PortfolioReservations::try_reserve`] models a re-quote of an already-tracked
//! market by *replacing* that market's prior reservation, not adding to it:
//! `prospective_total = total - existing_for_market + reservation`. A market that
//! requotes therefore never double-counts against the portfolio cap, mirroring
//! the per-leg slot-replacement discipline in the single-market shell.
//!
//! ## Fail-closed
//!
//! [`PortfolioRiskCap::new`] returns `None` on a non-positive/non-finite cap, so
//! a degenerate config can never construct a permissive aggregator.
//! [`PortfolioReservations::try_reserve`] returns `false` and mutates nothing on
//! a non-positive/non-finite reservation (a zero/NaN reservation is a caller
//! bug) and on a prospective total that strictly exceeds the cap. An empty
//! tracker reserves nothing. Over-budget = deny the new quote, never silently
//! exceed.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default` (an empty tracker
//! is built explicitly via [`PortfolioReservations::empty`]). All numeric
//! invariants come from [`crate::bolt_v3_numeric`]; no inline runtime literal on
//! the production path.

use std::collections::BTreeMap;

use crate::bolt_v3_numeric::{ZERO_F64, is_positive_finite};
use crate::strategies::portfolio_selection::MarketKey;

/// The single portfolio-wide collateral ceiling, in fee-inclusive USDC (the same
/// unit FR-040 produces). Constructed only through [`PortfolioRiskCap::new`]; no
/// `Default`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortfolioRiskCap {
    portfolio_cap: f64,
}

impl PortfolioRiskCap {
    /// Validate-at-construction (mirrors `KillThresholds::new`). Returns `None`
    /// (fail-closed) unless `portfolio_cap` is positive and finite — a
    /// zero/negative/NaN cap would either fund nothing or admit anything.
    pub fn new(portfolio_cap: f64) -> Option<Self> {
        if is_positive_finite(portfolio_cap) {
            Some(Self { portfolio_cap })
        } else {
            None
        }
    }
}

/// Live per-market reservation tracker. Keys on the opaque [`MarketKey`] and
/// carries the running total so the cap check is O(1). Built via
/// [`PortfolioReservations::empty`] — never `Default`, per the legacy-default
/// fence (an empty book is a deliberate starting state, not an implicit one).
#[derive(Debug, Clone, PartialEq)]
pub struct PortfolioReservations {
    reserved_by_market: BTreeMap<MarketKey, f64>,
    total_reserved: f64,
}

impl PortfolioReservations {
    /// An empty tracker: no market reserved, zero total. The only constructor.
    pub fn empty() -> Self {
        Self {
            reserved_by_market: BTreeMap::new(),
            total_reserved: ZERO_F64,
        }
    }

    /// The current portfolio-wide total reserved collateral (fee-inclusive USDC).
    /// Read-only view for the shell's portfolio-posture rollup.
    pub fn total_reserved(&self) -> f64 {
        self.total_reserved
    }

    /// Try to commit `reservation` (fee-inclusive USDC, produced by
    /// [`crate::strategies::maker_reservation::worst_case_reservation`]) as
    /// `market`'s reservation. Replace-not-add: a market already tracked has its
    /// prior reservation swapped out, so a requote never double-counts.
    ///
    /// Returns `false` and mutates nothing (fail-closed) when: `reservation` is
    /// not positive-finite; or the prospective portfolio total strictly exceeds
    /// `cap.portfolio_cap` (strict `>`, matching the FR-040 per-market gate so
    /// the per-market and portfolio caps agree at the boundary). Otherwise it
    /// commits the new per-market value and total and returns `true`.
    pub fn try_reserve(
        &mut self,
        cap: &PortfolioRiskCap,
        market: MarketKey,
        reservation: f64,
    ) -> bool {
        if !is_positive_finite(reservation) {
            return false;
        }
        let existing = self
            .reserved_by_market
            .get(&market)
            .copied()
            .unwrap_or(ZERO_F64);
        let prospective_total = self.total_reserved - existing + reservation;
        // A non-finite prospective total (extreme accumulation) is itself a
        // refusal — never admit on degenerate arithmetic.
        if !prospective_total.is_finite() || prospective_total > cap.portfolio_cap {
            return false;
        }
        self.reserved_by_market.insert(market, reservation);
        self.total_reserved = prospective_total;
        true
    }

    /// Zero one market's reservation on kill/settlement, decrementing the total.
    /// A no-op for an untracked market. Idempotent.
    pub fn release(&mut self, market: &MarketKey) {
        if let Some(existing) = self.reserved_by_market.remove(market) {
            self.total_reserved -= existing;
            // Guard against accumulated float drift dipping the total below zero.
            if self.total_reserved < ZERO_F64 {
                self.total_reserved = ZERO_F64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> MarketKey {
        MarketKey::new(id.to_string())
    }

    #[test]
    fn cap_rejects_non_positive_and_non_finite() {
        assert!(PortfolioRiskCap::new(0.0).is_none());
        assert!(PortfolioRiskCap::new(-10.0).is_none());
        assert!(PortfolioRiskCap::new(f64::NAN).is_none());
        assert!(PortfolioRiskCap::new(f64::INFINITY).is_none());
        assert!(PortfolioRiskCap::new(100.0).is_some());
    }

    #[test]
    fn empty_tracker_reserves_nothing() {
        let tracker = PortfolioReservations::empty();
        assert_eq!(tracker.total_reserved(), 0.0);
    }

    #[test]
    fn reserves_within_cap_and_tracks_total() {
        let cap = PortfolioRiskCap::new(100.0).unwrap();
        let mut tracker = PortfolioReservations::empty();
        assert!(tracker.try_reserve(&cap, key("a"), 40.0));
        assert!(tracker.try_reserve(&cap, key("b"), 30.0));
        assert_eq!(tracker.total_reserved(), 70.0);
    }

    #[test]
    fn boundary_admits_at_equal_rejects_one_epsilon_over() {
        let cap = PortfolioRiskCap::new(50.0).unwrap();
        let mut tracker = PortfolioReservations::empty();
        // Exactly == cap admits (strict-> matches the FR-040 per-market gate).
        assert!(tracker.try_reserve(&cap, key("a"), 50.0));
        // One epsilon over (after replacing the same market) rejects.
        assert!(!tracker.try_reserve(&cap, key("a"), 50.0 + f64::EPSILON * 100.0));
        // The rejected reserve left the prior value intact.
        assert_eq!(tracker.total_reserved(), 50.0);
    }

    #[test]
    fn over_budget_denies_and_mutates_nothing() {
        let cap = PortfolioRiskCap::new(100.0).unwrap();
        let mut tracker = PortfolioReservations::empty();
        assert!(tracker.try_reserve(&cap, key("a"), 70.0));
        assert!(!tracker.try_reserve(&cap, key("b"), 40.0)); // 70 + 40 > 100
        assert_eq!(tracker.total_reserved(), 70.0);
    }

    #[test]
    fn requote_replaces_not_adds() {
        let cap = PortfolioRiskCap::new(100.0).unwrap();
        let mut tracker = PortfolioReservations::empty();
        assert!(tracker.try_reserve(&cap, key("a"), 60.0));
        // Re-quoting "a" to 90 replaces the 60 (not 60+90=150), so it fits.
        assert!(tracker.try_reserve(&cap, key("a"), 90.0));
        assert_eq!(tracker.total_reserved(), 90.0);
    }

    #[test]
    fn non_positive_or_non_finite_reservation_fails_closed() {
        let cap = PortfolioRiskCap::new(100.0).unwrap();
        let mut tracker = PortfolioReservations::empty();
        assert!(!tracker.try_reserve(&cap, key("a"), 0.0));
        assert!(!tracker.try_reserve(&cap, key("a"), -5.0));
        assert!(!tracker.try_reserve(&cap, key("a"), f64::NAN));
        assert!(!tracker.try_reserve(&cap, key("a"), f64::INFINITY));
        assert_eq!(tracker.total_reserved(), 0.0);
    }

    #[test]
    fn release_frees_budget_for_other_markets() {
        let cap = PortfolioRiskCap::new(100.0).unwrap();
        let mut tracker = PortfolioReservations::empty();
        assert!(tracker.try_reserve(&cap, key("a"), 80.0));
        assert!(!tracker.try_reserve(&cap, key("b"), 40.0)); // blocked by "a"
        tracker.release(&key("a"));
        assert_eq!(tracker.total_reserved(), 0.0);
        assert!(tracker.try_reserve(&cap, key("b"), 40.0)); // now fits
        // Releasing an untracked market is a no-op.
        tracker.release(&key("never"));
        assert_eq!(tracker.total_reserved(), 40.0);
    }
}
