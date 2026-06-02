//! Bolt-v3 shared numeric and probability primitives.
//!
//! This module is the single home for the foundational numeric constants and
//! probability helpers imported by both the market-family pricing layer and the
//! strategy layer. It has no `crate::` dependencies, so it sits below every
//! other Bolt-v3 module and can be imported without introducing a cycle.

pub(crate) const ZERO_F64: f64 = 0.0;
pub(crate) const UNIT_F64: f64 = 1.0;
pub(crate) const POWER_OF_TWO: i32 = 2;
pub(crate) const DAYS_PER_YEAR_F64: f64 = 365.25;
pub(crate) const HOURS_PER_DAY_F64: f64 = 24.0;
pub(crate) const MINUTES_PER_HOUR_F64: f64 = 60.0;
pub(crate) const SECONDS_PER_MINUTE_F64: f64 = 60.0;
pub(crate) const SECONDS_PER_YEAR_F64: f64 =
    DAYS_PER_YEAR_F64 * HOURS_PER_DAY_F64 * MINUTES_PER_HOUR_F64 * SECONDS_PER_MINUTE_F64;
pub(crate) const MILLIS_PER_SECOND_U64: u64 = 1_000;
pub(crate) const MILLIS_PER_SECOND_F64: f64 = MILLIS_PER_SECOND_U64 as f64;
pub(crate) const BPS_DENOMINATOR: f64 = 10_000.0;
pub(crate) const MIDPOINT_DIVISOR_F64: f64 = 2.0;
pub(crate) const QUADRATIC_RISK_DIVISOR: f64 = 2.0;

pub(crate) fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > ZERO_F64
}

pub(crate) fn is_non_negative_finite(value: f64) -> bool {
    value.is_finite() && value >= ZERO_F64
}

pub(crate) fn clamp_probability(value: f64) -> f64 {
    value.clamp(ZERO_F64, UNIT_F64)
}

pub(crate) fn sanitize_probability(value: f64) -> Option<f64> {
    if value.is_finite() && (ZERO_F64..=UNIT_F64).contains(&value) {
        Some(value)
    } else {
        None
    }
}

pub(crate) fn sanitize_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(ZERO_F64)
    } else {
        ZERO_F64
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
    fn is_non_negative_finite_accepts_zero_and_positive_finite() {
        assert!(is_non_negative_finite(ZERO_F64));
        assert!(is_non_negative_finite(1.0));
        assert!(is_non_negative_finite(f64::MIN_POSITIVE));
    }

    #[test]
    fn is_non_negative_finite_rejects_negative_and_non_finite() {
        assert!(!is_non_negative_finite(-0.000_001));
        assert!(!is_non_negative_finite(-1.0));
        assert!(!is_non_negative_finite(f64::NAN));
        assert!(!is_non_negative_finite(f64::INFINITY));
        assert!(!is_non_negative_finite(f64::NEG_INFINITY));
    }

    #[test]
    fn clamp_probability_bounds_to_unit_interval() {
        assert_eq!(clamp_probability(-0.5), ZERO_F64);
        assert_eq!(clamp_probability(0.5), 0.5);
        assert_eq!(clamp_probability(1.5), UNIT_F64);
        assert_eq!(clamp_probability(ZERO_F64), ZERO_F64);
        assert_eq!(clamp_probability(UNIT_F64), UNIT_F64);
    }

    #[test]
    fn sanitize_non_negative_floors_at_zero_and_zeroes_non_finite() {
        assert_eq!(sanitize_non_negative(2.5), 2.5);
        assert_eq!(sanitize_non_negative(ZERO_F64), ZERO_F64);
        assert_eq!(sanitize_non_negative(-3.0), ZERO_F64);
        assert_eq!(sanitize_non_negative(f64::NAN), ZERO_F64);
        assert_eq!(sanitize_non_negative(f64::INFINITY), ZERO_F64);
        assert_eq!(sanitize_non_negative(f64::NEG_INFINITY), ZERO_F64);
    }
}
