//! Pure quiet-market phantom-LP selector for the binary-oracle maker reward
//! layer (W7 — FR-060: decide whether a reward market is a viable phantom-LP
//! candidate, fail-closed).
//!
//! A "phantom-LP" market is a quiet, funded reward market where resting two-
//! sided quotes earn the native reward with near-zero adverse selection: no fill
//! is needed to be paid, and because trades arrive far slower than once per hour,
//! the resting quotes are almost never picked off. This module is the pure
//! predicate that decides whether a market qualifies, plus the two supporting
//! pure computations the selection layer needs: the diluted expected daily
//! reward, and whether the venue's two-sided-quoting requirement applies at the
//! market's midpoint.
//!
//! ## Candidate conditions (ALL must hold)
//!
//! 1. **Funded**: `native_daily_rate >= min_daily_rate` — the pool pays above the
//!    payout floor, so resting quotes earn something.
//! 2. **Quiet**: `observed_trades_per_hour <= max_trades_per_hour` — low trade
//!    rate means low adverse selection (the whole phantom-LP thesis).
//! 3. **Not saturated**: `estimated_share_fraction >= min_share_fraction` — the
//!    book is not so crowded that this maker's diluted share of the reward is
//!    negligible.
//!
//! Any non-finite stat makes the market a non-candidate (fail-closed: an unknown
//! market is never quoted on a guess).
//!
//! ## Diluted yield, never undiluted
//!
//! [`expected_daily_reward`] returns `native_daily_rate × estimated_share_fraction`
//! — the DILUTED yield this maker can actually expect given competition, never
//! the undiluted pool size. Ranking on the undiluted pool over-states every
//! market's attractiveness; the diluted product is the honest selection signal.
//!
//! ## Two-sided requirement
//!
//! Near the `{0, 1}` resolution edges the venue requires quotes on BOTH outcome
//! tokens to earn the reward; in the interior single-sided quoting can qualify.
//! [`two_sided_required`] is a separate pure predicate over the sanitized
//! midpoint and the caller-supplied band edges (NO HARDCODES — the edges are
//! config thresholds, never literal in code), so the selection layer knows when
//! single-sided scoring applies.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default`. All numeric
//! invariants come from [`crate::bolt_v3_numeric`]; no inline runtime literal on
//! the production path.

use crate::bolt_v3_numeric::{UNIT_F64, ZERO_F64, is_positive_finite, sanitize_probability};

/// Caller-supplied phantom-LP qualification thresholds. Constructed only through
/// [`PhantomLpThresholds::new`]; no `Default`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhantomLpThresholds {
    min_daily_rate: f64,
    max_trades_per_hour: f64,
    min_share_fraction: f64,
}

impl PhantomLpThresholds {
    /// Validate-at-construction. Returns `None` (fail-closed) unless
    /// `min_daily_rate` and `max_trades_per_hour` are finite and `>= ZERO_F64`,
    /// and `min_share_fraction` is positive-finite and `<= UNIT_F64` (a share
    /// fraction is in `(0, 1]`; a zero floor would admit a fully-diluted market,
    /// a fraction above one is not a fraction).
    pub fn new(
        min_daily_rate: f64,
        max_trades_per_hour: f64,
        min_share_fraction: f64,
    ) -> Option<Self> {
        let rate_ok = min_daily_rate.is_finite() && min_daily_rate >= ZERO_F64;
        let trades_ok = max_trades_per_hour.is_finite() && max_trades_per_hour >= ZERO_F64;
        let share_ok = is_positive_finite(min_share_fraction) && min_share_fraction <= UNIT_F64;
        if rate_ok && trades_ok && share_ok {
            Some(Self {
                min_daily_rate,
                max_trades_per_hour,
                min_share_fraction,
            })
        } else {
            None
        }
    }
}

/// The runtime-observed per-market reward facts the selector reasons over.
/// `estimated_share_fraction` is the competition-weighted estimate of this
/// maker's slice of the pool, not a naive count.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarketRewardStats {
    pub native_daily_rate: f64,
    pub observed_trades_per_hour: f64,
    pub estimated_share_fraction: f64,
}

/// `true` only when the market is funded, quiet, and not saturated (all three
/// conditions hold). Any non-finite stat → `false` (fail-closed).
pub fn is_phantom_lp_candidate(stats: MarketRewardStats, thresholds: PhantomLpThresholds) -> bool {
    if !(stats.native_daily_rate.is_finite()
        && stats.observed_trades_per_hour.is_finite()
        && stats.estimated_share_fraction.is_finite())
    {
        return false;
    }
    let funded = stats.native_daily_rate >= thresholds.min_daily_rate;
    let quiet = stats.observed_trades_per_hour <= thresholds.max_trades_per_hour;
    let not_saturated = stats.estimated_share_fraction >= thresholds.min_share_fraction;
    funded && quiet && not_saturated
}

/// The DILUTED expected daily reward, `native_daily_rate × estimated_share_fraction`,
/// or `None` on any non-finite stat or a non-finite product. Never the undiluted
/// pool — this is the honest per-maker selection signal.
pub fn expected_daily_reward(stats: MarketRewardStats) -> Option<f64> {
    if !(stats.native_daily_rate.is_finite() && stats.estimated_share_fraction.is_finite()) {
        return None;
    }
    let diluted = stats.native_daily_rate * stats.estimated_share_fraction;
    if diluted.is_finite() {
        Some(diluted)
    } else {
        None
    }
}

/// `true` when the market midpoint sits near a resolution edge (below
/// `lower_edge` or above `upper_edge`), where the venue requires two-sided
/// quotes to earn the reward. The edges are caller-supplied config thresholds.
///
/// A midpoint outside the `[0, 1]` probability domain (a wrong-unit feed) or
/// edges that are not themselves valid probabilities are treated as
/// requiring two-sided quoting (fail-closed: the stricter, lower-risk posture
/// when the inputs are degenerate).
pub fn two_sided_required(midpoint: f64, lower_edge: f64, upper_edge: f64) -> bool {
    let (Some(mid), Some(lower), Some(upper)) = (
        sanitize_probability(midpoint),
        sanitize_probability(lower_edge),
        sanitize_probability(upper_edge),
    ) else {
        return true;
    };
    // Degenerate band (lower not strictly below upper) → require two-sided.
    if lower >= upper {
        return true;
    }
    mid < lower || mid > upper
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds() -> PhantomLpThresholds {
        // funded >= 1.0/day, quiet <= 0.5 trades/hr, share >= 0.05.
        PhantomLpThresholds::new(1.0, 0.5, 0.05).unwrap()
    }

    fn stats(rate: f64, trades: f64, share: f64) -> MarketRewardStats {
        MarketRewardStats {
            native_daily_rate: rate,
            observed_trades_per_hour: trades,
            estimated_share_fraction: share,
        }
    }

    #[test]
    fn thresholds_reject_bad_fields() {
        assert!(PhantomLpThresholds::new(f64::NAN, 0.5, 0.05).is_none());
        assert!(PhantomLpThresholds::new(-1.0, 0.5, 0.05).is_none());
        assert!(PhantomLpThresholds::new(1.0, -0.5, 0.05).is_none());
        assert!(PhantomLpThresholds::new(1.0, 0.5, 0.0).is_none()); // share floor must be >0
        assert!(PhantomLpThresholds::new(1.0, 0.5, 1.01).is_none()); // share > 1
        assert!(PhantomLpThresholds::new(1.0, 0.5, 0.05).is_some());
    }

    #[test]
    fn candidate_true_only_when_all_three_pass() {
        let t = thresholds();
        // Funded, quiet, not saturated.
        assert!(is_phantom_lp_candidate(stats(2.0, 0.1, 0.20), t));
        // Underfunded.
        assert!(!is_phantom_lp_candidate(stats(0.5, 0.1, 0.20), t));
        // Too busy (adverse selection).
        assert!(!is_phantom_lp_candidate(stats(2.0, 5.0, 0.20), t));
        // Saturated book (share below floor).
        assert!(!is_phantom_lp_candidate(stats(2.0, 0.1, 0.01), t));
    }

    #[test]
    fn any_non_finite_stat_fails_closed() {
        let t = thresholds();
        assert!(!is_phantom_lp_candidate(stats(f64::NAN, 0.1, 0.20), t));
        assert!(!is_phantom_lp_candidate(stats(2.0, f64::INFINITY, 0.20), t));
        assert!(!is_phantom_lp_candidate(stats(2.0, 0.1, f64::NAN), t));
    }

    #[test]
    fn quiet_vs_busy_contrast_at_the_boundary() {
        let t = thresholds();
        // Exactly at the quiet threshold qualifies (<=).
        assert!(is_phantom_lp_candidate(stats(2.0, 0.5, 0.20), t));
        // Just over does not.
        assert!(!is_phantom_lp_candidate(
            stats(2.0, 0.5 + f64::EPSILON, 0.20),
            t
        ));
    }

    #[test]
    fn expected_reward_is_the_diluted_product() {
        assert_eq!(expected_daily_reward(stats(10.0, 0.0, 0.25)), Some(2.5));
        // Non-finite stat → None.
        assert_eq!(expected_daily_reward(stats(f64::NAN, 0.0, 0.25)), None);
        assert_eq!(expected_daily_reward(stats(10.0, 0.0, f64::INFINITY)), None);
    }

    #[test]
    fn two_sided_required_near_edges_only() {
        // Interior midpoint with a [0.10, 0.90] band → single-sided ok.
        assert!(!two_sided_required(0.50, 0.10, 0.90));
        // Below the lower edge → two-sided required.
        assert!(two_sided_required(0.05, 0.10, 0.90));
        // Above the upper edge → two-sided required.
        assert!(two_sided_required(0.95, 0.10, 0.90));
    }

    #[test]
    fn two_sided_boundary_is_inclusive_interior() {
        // Exactly at an edge is NOT past it → interior, single-sided ok.
        assert!(!two_sided_required(0.10, 0.10, 0.90));
        assert!(!two_sided_required(0.90, 0.10, 0.90));
    }

    #[test]
    fn two_sided_fails_closed_on_degenerate_inputs() {
        // Out-of-domain midpoint → require two-sided (stricter posture).
        assert!(two_sided_required(1.5, 0.10, 0.90));
        assert!(two_sided_required(f64::NAN, 0.10, 0.90));
        // Inverted / degenerate band → require two-sided.
        assert!(two_sided_required(0.50, 0.90, 0.10));
        assert!(two_sided_required(0.50, 0.50, 0.50));
    }
}
