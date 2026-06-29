//! Bolt-v3 shared numeric and probability primitives.
//!
//! This module is the single home for the foundational numeric constants and
//! probability helpers imported by both the market-family pricing layer and the
//! strategy layer. It has no `crate::` dependencies, so it sits below every
//! other Bolt-v3 module and can be imported without introducing a cycle.

pub(crate) const ZERO_F64: f64 = 0.0;
pub(crate) const UNIT_F64: f64 = 1.0;
pub(crate) const TWO_F64: f64 = 2.0;
pub(crate) const HALF_F64: f64 = UNIT_F64 / TWO_F64;
pub(crate) const POWER_OF_TWO: i32 = 2;
pub(crate) const DAYS_PER_YEAR_F64: f64 = 365.25;
pub(crate) const HOURS_PER_DAY_F64: f64 = 24.0;
pub(crate) const MINUTES_PER_HOUR_F64: f64 = 60.0;
pub(crate) const SECONDS_PER_MINUTE_F64: f64 = 60.0;
pub(crate) const SECONDS_PER_YEAR_F64: f64 =
    DAYS_PER_YEAR_F64 * HOURS_PER_DAY_F64 * MINUTES_PER_HOUR_F64 * SECONDS_PER_MINUTE_F64;
pub(crate) const MILLIS_PER_SECOND_U64: u64 = 1_000;
pub(crate) const MILLIS_PER_MINUTE_U64: u64 = 60_000;
pub(crate) const NANOS_PER_MILLI_U64: u64 = MILLIS_PER_SECOND_U64 * MILLIS_PER_SECOND_U64;
pub(crate) const MILLIS_PER_SECOND_F64: f64 = MILLIS_PER_SECOND_U64 as f64;
pub(crate) const BPS_DENOMINATOR: f64 = 10_000.0;
pub(crate) const CENTS_PER_SHARE: f64 = 100.0;
pub(crate) const MIDPOINT_DIVISOR_F64: f64 = 2.0;
pub(crate) const QUADRATIC_RISK_DIVISOR: f64 = 2.0;
pub(crate) const NOTIONAL_FLOAT_TOLERANCE_EPSILON_MULTIPLIER: f64 = 10_000.0;
/// Length of a SHA-256 digest rendered as lowercase hex (32 bytes -> 64 chars).
/// Single source for every digest-shape guard so no module recomputes it.
pub(crate) const SHA256_HEX_DIGEST_LEN: usize = 64;

/// The single digest-shape guard for SHA-256 hex strings: exactly
/// `SHA256_HEX_DIGEST_LEN` lowercase-hex chars. Every digest producer in the
/// crate (`hex::encode(Sha256::digest(..))`) emits lowercase, so uppercase
/// `A-F` is never a legitimate digest and is rejected fail-closed. Lives here
/// (no `crate::` deps) so every validator delegates instead of re-implementing.
pub(crate) fn is_sha256_hex_digest(value: &str) -> bool {
    value.len() == SHA256_HEX_DIGEST_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > ZERO_F64
}

pub(crate) fn is_non_negative_finite(value: f64) -> bool {
    value.is_finite() && value >= ZERO_F64
}

pub(crate) fn notional_float_tolerance(reference_notional: f64) -> f64 {
    reference_notional.abs() * f64::EPSILON * NOTIONAL_FLOAT_TOLERANCE_EPSILON_MULTIPLIER
}

pub(crate) mod financial_value_private {
    pub trait Sealed {}
    pub trait NoDefaultProbe {
        fn financial_value_default_readd_fence();
    }

    pub trait DefaultProbe {
        fn financial_value_default_readd_fence();
    }

    impl<T: Default> DefaultProbe for T {
        fn financial_value_default_readd_fence() {}
    }
}

#[allow(private_bounds)]
pub trait FinancialValue:
    financial_value_private::Sealed + financial_value_private::NoDefaultProbe
{
}

use financial_value_private::{DefaultProbe as _, NoDefaultProbe as _};

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct ProbabilityValue(f64);

impl ProbabilityValue {
    pub const fn try_from_unit(value: f64) -> Option<Self> {
        if value == value && value != f64::INFINITY && value != f64::NEG_INFINITY {
            if value >= ZERO_F64 && value <= UNIT_F64 {
                return Some(Self(value));
            }
        }
        None
    }

    pub const fn get(self) -> f64 {
        self.0
    }
}

impl financial_value_private::Sealed for ProbabilityValue {}
impl financial_value_private::NoDefaultProbe for ProbabilityValue {
    fn financial_value_default_readd_fence() {}
}
impl FinancialValue for ProbabilityValue {}

const _: fn() = ProbabilityValue::financial_value_default_readd_fence;

pub(crate) fn bounded_probability_from_finite(value: f64) -> Option<ProbabilityValue> {
    if !value.is_finite() {
        return None;
    }
    let bounded = if value < ZERO_F64 {
        ZERO_F64
    } else if value > UNIT_F64 {
        UNIT_F64
    } else {
        value
    };
    ProbabilityValue::try_from_unit(bounded)
}

pub(crate) fn sanitize_probability(value: f64) -> Option<f64> {
    ProbabilityValue::try_from_unit(value).map(ProbabilityValue::get)
}

pub(crate) fn sanitize_open_probability(value: f64, eps: f64) -> Option<f64> {
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

pub(crate) fn sanitize_non_negative(value: f64) -> f64 {
    // Sanitizers that feed min/max cap chains must collapse NaN and infinities
    // to zero so ordinary comparisons cannot let non-finite values leak past a
    // fail-closed guard.
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
    fn sanitize_open_probability_accepts_strict_interior_values() {
        let eps = 0.01;

        assert_eq!(sanitize_open_probability(HALF_F64, eps), Some(HALF_F64));
        assert_eq!(sanitize_open_probability(0.011, eps), Some(0.011));
        assert_eq!(sanitize_open_probability(0.989, eps), Some(0.989));
    }

    #[test]
    fn sanitize_open_probability_rejects_edges_bad_collar_and_non_finite_inputs() {
        let eps = 0.01;

        assert_eq!(sanitize_open_probability(eps, eps), None);
        assert_eq!(sanitize_open_probability(UNIT_F64 - eps, eps), None);
        assert_eq!(sanitize_open_probability(ZERO_F64, eps), None);
        assert_eq!(sanitize_open_probability(UNIT_F64, eps), None);
        assert_eq!(sanitize_open_probability(HALF_F64, ZERO_F64), None);
        assert_eq!(sanitize_open_probability(HALF_F64, HALF_F64), None);
        assert_eq!(sanitize_open_probability(f64::NAN, eps), None);
        assert_eq!(sanitize_open_probability(HALF_F64, f64::INFINITY), None);
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
    fn notional_float_tolerance_scales_with_reference_notional() {
        assert_eq!(notional_float_tolerance(ZERO_F64), ZERO_F64);
        assert_eq!(
            notional_float_tolerance(-BPS_DENOMINATOR),
            notional_float_tolerance(BPS_DENOMINATOR)
        );
        assert!(notional_float_tolerance(BPS_DENOMINATOR) > notional_float_tolerance(UNIT_F64));
    }

    #[test]
    fn notional_float_tolerance_uses_named_epsilon_multiplier() {
        assert_eq!(
            notional_float_tolerance(BPS_DENOMINATOR),
            BPS_DENOMINATOR * f64::EPSILON * NOTIONAL_FLOAT_TOLERANCE_EPSILON_MULTIPLIER
        );
    }

    #[test]
    fn cents_per_share_unit_conversion_constant_is_shared() {
        assert_eq!(CENTS_PER_SHARE, 100.0);
    }

    #[test]
    fn probability_value_accepts_unit_interval_only() {
        assert_eq!(
            ProbabilityValue::try_from_unit(ZERO_F64).map(ProbabilityValue::get),
            Some(ZERO_F64)
        );
        assert_eq!(
            ProbabilityValue::try_from_unit(0.5).map(ProbabilityValue::get),
            Some(0.5)
        );
        assert_eq!(
            ProbabilityValue::try_from_unit(UNIT_F64).map(ProbabilityValue::get),
            Some(UNIT_F64)
        );
        assert_eq!(ProbabilityValue::try_from_unit(-0.5), None);
        assert_eq!(ProbabilityValue::try_from_unit(1.5), None);
        assert_eq!(ProbabilityValue::try_from_unit(f64::NAN), None);
        assert_eq!(ProbabilityValue::try_from_unit(f64::INFINITY), None);
    }

    #[test]
    fn bounded_probability_rejects_non_finite_before_bounding() {
        assert_eq!(
            bounded_probability_from_finite(-0.5).map(ProbabilityValue::get),
            Some(ZERO_F64)
        );
        assert_eq!(
            bounded_probability_from_finite(0.5).map(ProbabilityValue::get),
            Some(0.5)
        );
        assert_eq!(
            bounded_probability_from_finite(1.5).map(ProbabilityValue::get),
            Some(UNIT_F64)
        );
        assert_eq!(bounded_probability_from_finite(f64::NAN), None);
        assert_eq!(bounded_probability_from_finite(f64::INFINITY), None);
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
