//! Bolt-v3 shared numeric and probability primitives.
//!
//! This module is the single home for the foundational numeric constants and
//! probability helpers imported by both the market-family pricing layer and the
//! strategy layer. It has no `crate::` dependencies, so it sits below every
//! other Bolt-v3 module and can be imported without introducing a cycle.

pub const ZERO_F64: f64 = 0.0;
pub const UNIT_F64: f64 = 1.0;
pub const TWO_F64: f64 = 2.0;
pub const HALF_F64: f64 = 0.5;
pub const POWER_OF_TWO: i32 = 2;
pub const DAYS_PER_YEAR_F64: f64 = 365.25;
pub const HOURS_PER_DAY_F64: f64 = 24.0;
pub const MINUTES_PER_HOUR_F64: f64 = 60.0;
pub const SECONDS_PER_MINUTE_F64: f64 = 60.0;
pub const SECONDS_PER_YEAR_F64: f64 =
    DAYS_PER_YEAR_F64 * HOURS_PER_DAY_F64 * MINUTES_PER_HOUR_F64 * SECONDS_PER_MINUTE_F64;
pub const MILLIS_PER_SECOND_U64: u64 = 1_000;
pub const MILLIS_PER_SECOND_F64: f64 = MILLIS_PER_SECOND_U64 as f64;
pub const NANOS_PER_MILLI_U64: u64 = 1_000_000;

pub fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > ZERO_F64
}

pub fn sanitize_probability(value: f64) -> Option<f64> {
    if value.is_finite() && (ZERO_F64..=UNIT_F64).contains(&value) {
        Some(value)
    } else {
        None
    }
}

/// Sanitize a probability that must live in the **open** interval `(eps, 1−eps)`.
///
/// This is the fail-closed guard for a generated *quote leg* (FR-022 / SC-002): a
/// quote at an exact `0.0` or `1.0` (or inside the `eps` collar) is degenerate —
/// it offers no edge and, at the boundaries, prices an already-decided outcome —
/// so it must never be admitted. It is the open-interval counterpart of
/// [`sanitize_probability`], which keeps guarding the *fair-value input* on the
/// closed `[0, 1]` (an input may legitimately sit at a boundary; an emitted quote
/// may not).
///
/// Returns `Some(value)` iff every condition holds, `None` otherwise:
/// - `value` and `eps` are both finite;
/// - `eps` is a real half-collar: `0 < eps < 0.5` (an `eps` at/above `0.5` would
///   collapse the admissible interval to empty or invert it);
/// - `value` is strictly interior: `eps < value < 1 − eps`.
pub fn sanitize_open_probability(value: f64, eps: f64) -> Option<f64> {
    if !value.is_finite() || !eps.is_finite() {
        return None;
    }
    if !(eps > ZERO_F64 && eps < HALF_F64) {
        return None;
    }
    if eps < value && value < UNIT_F64 - eps {
        Some(value)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_positive_finite_accepts_positive_finite() {
        assert!(is_positive_finite(1.0));
        assert!(is_positive_finite(f64::MIN_POSITIVE));
    }

    #[test]
    fn is_positive_finite_rejects_zero_negative_and_non_finite() {
        assert!(!is_positive_finite(ZERO_F64));
        assert!(!is_positive_finite(-1.0));
        assert!(!is_positive_finite(f64::NAN));
        assert!(!is_positive_finite(f64::INFINITY));
        assert!(!is_positive_finite(f64::NEG_INFINITY));
    }

    #[test]
    fn sanitize_probability_accepts_in_range_and_bounds() {
        assert_eq!(sanitize_probability(0.5), Some(0.5));
        assert_eq!(sanitize_probability(ZERO_F64), Some(ZERO_F64));
        assert_eq!(sanitize_probability(UNIT_F64), Some(UNIT_F64));
    }

    #[test]
    fn sanitize_probability_rejects_out_of_range_and_non_finite() {
        assert_eq!(sanitize_probability(-0.000_001), None);
        assert_eq!(sanitize_probability(1.000_001), None);
        assert_eq!(sanitize_probability(f64::NAN), None);
        assert_eq!(sanitize_probability(f64::INFINITY), None);
    }

    #[test]
    fn sanitize_open_probability_accepts_a_strict_interior_value() {
        let eps = 0.01;
        assert_eq!(sanitize_open_probability(0.5, eps), Some(0.5));
        // Just inside each collar edge is admitted.
        assert_eq!(sanitize_open_probability(0.011, eps), Some(0.011));
        assert_eq!(sanitize_open_probability(0.989, eps), Some(0.989));
    }

    #[test]
    fn sanitize_open_probability_rejects_the_collar_edges() {
        let eps = 0.01;
        // value == eps and value == 1 − eps are the closed-interval bounds the
        // open guard must exclude (this is the SC-002 latent bug fix).
        assert_eq!(sanitize_open_probability(eps, eps), None);
        assert_eq!(sanitize_open_probability(UNIT_F64 - eps, eps), None);
    }

    #[test]
    fn sanitize_open_probability_rejects_exact_zero_and_one() {
        // The boundaries `sanitize_probability` accepts must never be a quote leg.
        let eps = 0.01;
        assert_eq!(sanitize_probability(ZERO_F64), Some(ZERO_F64));
        assert_eq!(sanitize_probability(UNIT_F64), Some(UNIT_F64));
        assert_eq!(sanitize_open_probability(ZERO_F64, eps), None);
        assert_eq!(sanitize_open_probability(UNIT_F64, eps), None);
    }

    #[test]
    fn sanitize_open_probability_rejects_eps_outside_open_zero_to_half() {
        // eps must be a real half-collar: 0 < eps < 0.5.
        assert_eq!(sanitize_open_probability(0.5, ZERO_F64), None);
        assert_eq!(sanitize_open_probability(0.5, -0.01), None);
        assert_eq!(sanitize_open_probability(0.5, HALF_F64), None);
        assert_eq!(sanitize_open_probability(0.5, 0.6), None);
    }

    #[test]
    fn sanitize_open_probability_rejects_non_finite_value_or_eps() {
        assert_eq!(sanitize_open_probability(f64::NAN, 0.01), None);
        assert_eq!(sanitize_open_probability(f64::INFINITY, 0.01), None);
        assert_eq!(sanitize_open_probability(0.5, f64::NAN), None);
        assert_eq!(sanitize_open_probability(0.5, f64::INFINITY), None);
    }
}
