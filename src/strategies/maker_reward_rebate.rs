//! Pure maker-rebate accrual for the binary-oracle maker reward layer
//! (W7 — FR-060: book the maker rebate earned on a maker fill so reward income
//! enters PnL accounting on the same basis as settlement, fail-closed).
//!
//! Rewards are additive on top of the assumed-edge core. When a maker fill
//! occurs, the venue's binary fee curve generates a taker-side fee, and the
//! maker earns a configured share of it as a rebate. This module owns that one
//! accrual: it never charges, only credits, so reward income flows into the same
//! f64 accounting accumulator shape the settlement layer uses — one accounting
//! path, no parallel reward ledger semantics.
//!
//! ## The binary fee curve
//!
//! On the binary CLOB the per-share fee follows `fee_rate × p × (1 − p)`, the
//! symmetric curve that peaks at the midpoint `p = 0.5` and vanishes at the
//! `{0, 1}` resolution edges (a share that is already certain carries no fee).
//! The maker rebate on a fill of `filled_shares` at `fill_price = p` is
//! therefore:
//!
//! ```text
//! rebate = rebate_share × (filled_shares × fee_rate × p × (1 − p))
//! ```
//!
//! Every factor is non-negative on its valid domain (`filled_shares > 0`,
//! `fee_rate >= 0`, `rebate_share ∈ [0, 1]`, `p ∈ [0, 1]` so `p(1 − p) >= 0`), so
//! the result is structurally `>= ZERO_F64` — a rebate is income, never a charge.
//!
//! ## Fail-closed
//!
//! [`RebateSchedule::new`] rejects a non-finite field, a negative `fee_rate`, or
//! a `rebate_share` outside `[0, 1]` (a share `> 1` would pay out more than the
//! fee — a misconfig). [`maker_rebate`] returns `None` (no accrual) on a
//! non-positive/non-finite `filled_shares` (only a real fill accrues — the same
//! guard the inventory layer applies to a fill quantity), a `fill_price` outside
//! the `[0, 1]` probability domain, or any non-finite product.
//! [`RebateLedger::fold`] returns `false` and leaves the ledger unchanged on any
//! rejected fill.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default` (an empty ledger
//! is built explicitly via [`RebateLedger::flat`]). All numeric invariants come
//! from [`crate::bolt_v3_numeric`]; no inline runtime literal on the production
//! path.

use crate::bolt_v3_numeric::{UNIT_F64, ZERO_F64, is_positive_finite, sanitize_probability};

/// The venue rebate schedule: the per-share `fee_rate` of the binary fee curve
/// and the maker's `rebate_share` of that fee. Constructed only through
/// [`RebateSchedule::new`]; no `Default` (a zero schedule is a deliberate "no
/// rebate program" choice, not an implicit one).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RebateSchedule {
    fee_rate: f64,
    rebate_share: f64,
}

impl RebateSchedule {
    /// Validate-at-construction. Returns `None` (fail-closed) unless every field
    /// is finite, `fee_rate >= ZERO_F64` (a negative fee makes no sense), and
    /// `rebate_share ∈ [ZERO_F64, UNIT_F64]` (a share above one would pay more
    /// than the fee collected — a misconfig).
    pub fn new(fee_rate: f64, rebate_share: f64) -> Option<Self> {
        let fee_ok = fee_rate.is_finite() && fee_rate >= ZERO_F64;
        let share_ok = rebate_share.is_finite() && (ZERO_F64..=UNIT_F64).contains(&rebate_share);
        if fee_ok && share_ok {
            Some(Self {
                fee_rate,
                rebate_share,
            })
        } else {
            None
        }
    }
}

/// The maker rebate earned on a single maker fill, or `None` (no accrual) when
/// the fill is degenerate. `filled_shares` must be positive-finite (only a real
/// fill accrues); `fill_price` must lie on the `[0, 1]` binary probability domain
/// (a price outside it is a wrong-unit feed). The result is always
/// `>= ZERO_F64` — income, never a charge.
pub fn maker_rebate(filled_shares: f64, fill_price: f64, schedule: RebateSchedule) -> Option<f64> {
    if !is_positive_finite(filled_shares) {
        return None;
    }
    let price = sanitize_probability(fill_price)?;
    // Symmetric binary fee curve fee_rate * p * (1 - p), then the maker's share.
    let fee = filled_shares * schedule.fee_rate * price * (UNIT_F64 - price);
    let rebate = schedule.rebate_share * fee;
    if rebate.is_finite() && rebate >= ZERO_F64 {
        Some(rebate)
    } else {
        None
    }
}

/// A running accumulator of accrued maker rebate income. Built via
/// [`RebateLedger::flat`] (no `Default`), mirroring the raw-f64 accumulation
/// discipline of the inventory/settlement accounting primitives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RebateLedger {
    accrued: f64,
}

impl RebateLedger {
    /// A fresh ledger with zero accrued rebate. The only constructor.
    pub fn flat() -> Self {
        Self { accrued: ZERO_F64 }
    }

    /// Total rebate accrued so far. Always `>= ZERO_F64` (only non-negative
    /// rebates are ever folded in). Note: raw f64 summation carries the usual
    /// floating-point drift; this is an accounting figure, not an exact ledger.
    pub fn accrued(&self) -> f64 {
        self.accrued
    }

    /// Fold a single maker fill's rebate into the ledger. Returns `true` and
    /// adds the rebate on a valid fill; returns `false` and leaves `accrued`
    /// unchanged (fail-closed) on any fill [`maker_rebate`] rejects.
    pub fn fold(&mut self, filled_shares: f64, fill_price: f64, schedule: RebateSchedule) -> bool {
        match maker_rebate(filled_shares, fill_price, schedule) {
            Some(rebate) => {
                self.accrued += rebate;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_rejects_bad_fields() {
        assert!(RebateSchedule::new(f64::NAN, 0.5).is_none());
        assert!(RebateSchedule::new(-0.01, 0.5).is_none()); // negative fee
        assert!(RebateSchedule::new(0.02, -0.1).is_none()); // share < 0
        assert!(RebateSchedule::new(0.02, 1.01).is_none()); // share > 1
        assert!(RebateSchedule::new(0.0, 0.0).is_some()); // zero is valid (no program)
        assert!(RebateSchedule::new(0.02, 1.0).is_some());
    }

    #[test]
    fn rebate_matches_fee_curve_at_midpoint() {
        let schedule = RebateSchedule::new(0.02, 0.5).unwrap();
        // p=0.5 → p(1-p)=0.25. fee = 100 * 0.02 * 0.25; rebate = 0.5 * fee.
        let expected = 0.5 * (100.0 * 0.02 * 0.5 * 0.5);
        assert_eq!(maker_rebate(100.0, 0.5, schedule), Some(expected));
    }

    #[test]
    fn rebate_matches_fee_curve_at_small_price() {
        let schedule = RebateSchedule::new(0.02, 0.5).unwrap();
        let p = 0.05;
        let expected = 0.5 * (100.0 * 0.02 * p * (1.0 - p));
        assert_eq!(maker_rebate(100.0, p, schedule), Some(expected));
    }

    #[test]
    fn rebate_scales_linearly_in_shares_and_share() {
        let schedule_half = RebateSchedule::new(0.02, 0.5).unwrap();
        let schedule_full = RebateSchedule::new(0.02, 1.0).unwrap();
        let small = maker_rebate(100.0, 0.4, schedule_half).unwrap();
        let double_shares = maker_rebate(200.0, 0.4, schedule_half).unwrap();
        let double_share = maker_rebate(100.0, 0.4, schedule_full).unwrap();
        assert_eq!(double_shares, 2.0 * small);
        assert_eq!(double_share, 2.0 * small);
    }

    #[test]
    fn zero_rebate_share_yields_zero_income() {
        let schedule = RebateSchedule::new(0.02, 0.0).unwrap();
        assert_eq!(maker_rebate(100.0, 0.5, schedule), Some(0.0));
    }

    #[test]
    fn fails_closed_on_non_positive_shares() {
        let schedule = RebateSchedule::new(0.02, 0.5).unwrap();
        assert_eq!(maker_rebate(0.0, 0.5, schedule), None);
        assert_eq!(maker_rebate(-10.0, 0.5, schedule), None);
        assert_eq!(maker_rebate(f64::NAN, 0.5, schedule), None);
    }

    #[test]
    fn fails_closed_on_out_of_domain_price() {
        let schedule = RebateSchedule::new(0.02, 0.5).unwrap();
        assert_eq!(maker_rebate(100.0, -0.01, schedule), None);
        assert_eq!(maker_rebate(100.0, 1.01, schedule), None);
        assert_eq!(maker_rebate(100.0, f64::INFINITY, schedule), None);
        // Domain edges p=0 and p=1 are valid but the fee curve vanishes there.
        assert_eq!(maker_rebate(100.0, 0.0, schedule), Some(0.0));
        assert_eq!(maker_rebate(100.0, 1.0, schedule), Some(0.0));
    }

    #[test]
    fn ledger_accrues_valid_and_ignores_rejected() {
        let schedule = RebateSchedule::new(0.02, 0.5).unwrap();
        let mut ledger = RebateLedger::flat();
        assert_eq!(ledger.accrued(), 0.0);
        assert!(ledger.fold(100.0, 0.5, schedule));
        let after_one = ledger.accrued();
        assert!(after_one > 0.0);
        // A rejected fill leaves the ledger untouched and returns false.
        assert!(!ledger.fold(-1.0, 0.5, schedule));
        assert_eq!(ledger.accrued(), after_one);
        // A second valid fill keeps accruing.
        assert!(ledger.fold(100.0, 0.5, schedule));
        assert_eq!(ledger.accrued(), 2.0 * after_one);
    }
}
