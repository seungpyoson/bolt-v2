//! Pure reward-aware spread shaper for the binary-oracle maker reward layer
//! (W7 — FR-060: tighten the maker half-spread INTO the reward-eligibility band
//! when safe, NEVER widen, fail-closed). Fills the offset-precedence slot-4
//! `reward_shaping_offset` seam.
//!
//! A reward program pays makers that rest quotes within an eligibility band
//! (`max_spread` of the midpoint). If the W3-derived base half-spread is wider
//! than that band, the maker is leaving reward on the table; tightening toward
//! the band captures it. But tightening is the ONLY safe direction for a reward
//! signal: reward must never push quotes *wider* (that would be a directional
//! bet dressed up as reward capture) and must never tighten *past* the W3
//! adverse-selection floor (that would trade safety for reward — the one thing
//! FR-060 forbids). This module encodes that one-directional discipline
//! structurally, the mirror image of the time-widening factor's
//! widen-only discipline.
//!
//! ## The load-bearing safety property
//!
//! The returned value is a TIGHTENING-ONLY offset, bounded into
//! `[ZERO_F64, base_offset_cap]`. The caller subtracts it from the base half-
//! spread, so a non-negative bounded offset can only REDUCE the effective half-
//! spread toward `band.max_spread`, never increase it, and the cap means it can
//! never invert a leg's sign. It returns `None` (the caller substitutes
//! `ZERO_F64` = no shaping this tick) on every unsafe or pointless case:
//!
//! - any input non-finite;
//! - `native_daily_rate <= ZERO_F64` (no live reward pool → no reason to shape);
//! - `base_half_spread <= band.max_spread` (already inside the band → nothing to
//!   capture);
//! - the target tightened half-spread would fall below the W3 `half_spread_floor`
//!   the caller passes in (reward never overrides the GM adverse-selection
//!   floor).
//!
//! The math: `shaped_half = max(band.max_spread, half_spread_floor)`;
//! `offset = clamp(base_half_spread - shaped_half, ZERO_F64, base_offset_cap)`.
//! Taking the max with the floor guarantees the target never dips below the
//! safety floor; clamping at zero guarantees a never-widen offset; clamping at
//! the cap guarantees the offset can never cross a leg. Because the offset is
//! applied additively downstream and re-clamped by the joint positive-edge and
//! bracket guards in the leg composer, any output that would still invalidate a
//! leg is independently rejected — defence in depth.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default`. All numeric
//! invariants come from [`crate::bolt_v3_numeric`]; no inline runtime literal on
//! the production path.

use crate::bolt_v3_numeric::{ZERO_F64, is_positive_finite};

/// The reward-eligibility band facts for one market: `max_spread` is the
/// eligibility band half-width (in price units — quotes within it earn rewards),
/// `min_size` the minimum eligible resting size, and `native_daily_rate` the
/// pool's native daily payout rate. Constructed only through
/// [`RewardBand::new`]; no `Default`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RewardBand {
    max_spread: f64,
    min_size: f64,
    native_daily_rate: f64,
}

impl RewardBand {
    /// Validate-at-construction. Returns `None` (fail-closed) unless every field
    /// is finite and `>= ZERO_F64`. A negative band width, size, or daily rate is
    /// a wrong-unit or wrong-sign feed; a zero `native_daily_rate` is allowed at
    /// construction (it simply means "no live pool" and is handled as a no-shape
    /// case in [`reward_shaping_offset`]).
    pub fn new(max_spread: f64, min_size: f64, native_daily_rate: f64) -> Option<Self> {
        let fields = [max_spread, min_size, native_daily_rate];
        if fields.iter().all(|f| f.is_finite() && *f >= ZERO_F64) {
            Some(Self {
                max_spread,
                min_size,
                native_daily_rate,
            })
        } else {
            None
        }
    }

    /// The minimum eligible resting size for this band (caller leans on it when
    /// sizing a reward-shaped quote).
    pub fn min_size(&self) -> f64 {
        self.min_size
    }
}

/// The reward-capture, tighten-only half-spread offset, or `None` (no shaping —
/// the caller substitutes `ZERO_F64`).
///
/// `base_half_spread` is the W3-derived half-spread before shaping;
/// `half_spread_floor` is the W3 adverse-selection floor the offset may never
/// breach; `base_offset_cap` is the absolute bound on how much this slot may
/// move a leg. The returned offset is in `[ZERO_F64, base_offset_cap]` and may
/// only TIGHTEN (the caller subtracts it). See the module docs for the full
/// safety property and the `None` cases.
pub fn reward_shaping_offset(
    base_half_spread: f64,
    half_spread_floor: f64,
    band: RewardBand,
    base_offset_cap: f64,
) -> Option<f64> {
    // Any non-finite numeric input → no shaping (RewardBand fields are already
    // finite by construction; these three are caller-supplied this tick).
    if !(base_half_spread.is_finite()
        && half_spread_floor.is_finite()
        && base_offset_cap.is_finite())
    {
        return None;
    }
    // A non-positive offset cap leaves no room to shape at all; treat as no-shape
    // rather than a degenerate clamp window.
    if !is_positive_finite(base_offset_cap) {
        return None;
    }
    // No live reward pool → no reason to tighten toward a band that pays nothing.
    if !is_positive_finite(band.native_daily_rate) {
        return None;
    }
    // Already inside (or at) the band → nothing to capture, do not shape.
    if base_half_spread <= band.max_spread {
        return None;
    }
    // Target half-spread: tighten toward the band, but never below the W3 floor.
    let shaped_half = band.max_spread.max(half_spread_floor);
    // If tightening to the band would require crossing below the floor — i.e. the
    // floor itself is wider than the base — there is no safe room to shape.
    if base_half_spread <= shaped_half {
        return None;
    }
    let raw_offset = base_half_spread - shaped_half;
    // Tighten-only, bounded: never negative (no widening), never beyond the cap.
    let offset = raw_offset.clamp(ZERO_F64, base_offset_cap);
    if offset.is_finite() && offset > ZERO_F64 {
        Some(offset)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn band(max_spread: f64, rate: f64) -> RewardBand {
        RewardBand::new(max_spread, 1.0, rate).unwrap()
    }

    #[test]
    fn band_rejects_non_finite_and_negative() {
        assert!(RewardBand::new(f64::NAN, 1.0, 1.0).is_none());
        assert!(RewardBand::new(-0.01, 1.0, 1.0).is_none());
        assert!(RewardBand::new(0.02, -1.0, 1.0).is_none());
        assert!(RewardBand::new(0.02, 1.0, -1.0).is_none());
        // Zero native_daily_rate is constructible (no live pool).
        assert!(RewardBand::new(0.02, 1.0, 0.0).is_some());
    }

    #[test]
    fn tightens_toward_band_when_base_is_wider() {
        // base 0.10, band 0.04, floor 0.01 → shaped_half = 0.04, offset = 0.06.
        let offset = reward_shaping_offset(0.10, 0.01, band(0.04, 5.0), 1.0).unwrap();
        assert_eq!(offset, 0.10 - 0.04);
    }

    #[test]
    fn offset_is_bounded_by_cap() {
        // Raw offset would be 0.06, but the cap clamps it to 0.02.
        let offset = reward_shaping_offset(0.10, 0.01, band(0.04, 5.0), 0.02).unwrap();
        assert_eq!(offset, 0.02);
    }

    #[test]
    fn never_widens_when_already_inside_band() {
        // base 0.03 <= band 0.04 → nothing to capture, no shaping.
        assert_eq!(
            reward_shaping_offset(0.03, 0.01, band(0.04, 5.0), 1.0),
            None
        );
        // base exactly at band edge also yields no shaping.
        assert_eq!(
            reward_shaping_offset(0.04, 0.01, band(0.04, 5.0), 1.0),
            None
        );
    }

    #[test]
    fn respects_the_w3_floor_target_never_below_it() {
        // Floor 0.06 is wider than band 0.04; shaped_half = max(0.04,0.06)=0.06.
        // base 0.10 → offset = 0.04, target half-spread = 0.06 == floor, safe.
        let offset = reward_shaping_offset(0.10, 0.06, band(0.04, 5.0), 1.0).unwrap();
        assert_eq!(offset, 0.10 - 0.06);
    }

    #[test]
    fn no_shaping_when_floor_already_at_or_above_base() {
        // Floor 0.10 >= base 0.10 → shaped_half=0.10, base<=shaped_half → None.
        assert_eq!(
            reward_shaping_offset(0.10, 0.10, band(0.04, 5.0), 1.0),
            None
        );
        // Floor wider than base → no safe room to tighten.
        assert_eq!(
            reward_shaping_offset(0.08, 0.12, band(0.04, 5.0), 1.0),
            None
        );
    }

    #[test]
    fn fails_closed_on_zero_rate_and_non_finite_inputs() {
        // No live pool.
        assert_eq!(
            reward_shaping_offset(0.10, 0.01, band(0.04, 0.0), 1.0),
            None
        );
        // Non-finite base / floor / cap.
        assert_eq!(
            reward_shaping_offset(f64::NAN, 0.01, band(0.04, 5.0), 1.0),
            None
        );
        assert_eq!(
            reward_shaping_offset(0.10, f64::INFINITY, band(0.04, 5.0), 1.0),
            None
        );
        assert_eq!(
            reward_shaping_offset(0.10, 0.01, band(0.04, 5.0), f64::NAN),
            None
        );
        // Non-positive cap leaves no room.
        assert_eq!(
            reward_shaping_offset(0.10, 0.01, band(0.04, 5.0), 0.0),
            None
        );
    }

    #[test]
    fn offset_is_always_non_negative_and_within_cap() {
        // Sweep base half-spreads; every produced offset is in [0, cap] and the
        // implied target never dips below the floor.
        let cap = 0.03;
        let floor = 0.01;
        let b = band(0.04, 5.0);
        for i in 0..50 {
            let base = 0.001 * (i as f64);
            if let Some(offset) = reward_shaping_offset(base, floor, b, cap) {
                assert!((ZERO_F64..=cap).contains(&offset));
                // Tightening never crosses below the floor.
                let target = base - offset;
                assert!(target >= floor - f64::EPSILON);
            }
        }
    }
}
