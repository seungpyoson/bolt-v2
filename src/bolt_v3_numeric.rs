//! Bolt-v3 shared numeric and probability primitives.
//!
//! This module is the single home for the foundational numeric constants and
//! probability helpers imported by both the market-family pricing layer and the
//! strategy layer. It has no `crate::` dependencies, so it sits below every
//! other Bolt-v3 module and can be imported without introducing a cycle.

pub const ZERO_F64: f64 = 0.0;
pub const UNIT_F64: f64 = 1.0;
pub const POWER_OF_TWO: i32 = 2;
pub const DAYS_PER_YEAR_F64: f64 = 365.25;
pub const HOURS_PER_DAY_F64: f64 = 24.0;
pub const MINUTES_PER_HOUR_F64: f64 = 60.0;
pub const SECONDS_PER_MINUTE_F64: f64 = 60.0;
pub const SECONDS_PER_YEAR_F64: f64 =
    DAYS_PER_YEAR_F64 * HOURS_PER_DAY_F64 * MINUTES_PER_HOUR_F64 * SECONDS_PER_MINUTE_F64;

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
}
