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

/// Closed-interval probability value for Bolt-v3 compute-layer math.
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_numeric::Probability;
///
/// fn accepts_probability(_: Probability) {}
///
/// accepts_probability(0.42_f64);
/// ```
///
/// ```compile_fail
/// use bolt_v2::bolt_v3_numeric::Probability;
///
/// let probability = Probability::new(0.42_f64).expect("valid probability");
/// let price = 10.0_f64;
///
/// let _mixed = probability + price;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Probability {
    value: f64,
}

impl Probability {
    pub fn new(value: f64) -> Option<Self> {
        sanitize_probability(value).map(|value| Self { value })
    }

    pub fn clamped(value: f64) -> Option<Self> {
        if value.is_finite() {
            Some(Self {
                value: clamp_probability(value),
            })
        } else {
            None
        }
    }

    pub fn value(self) -> f64 {
        self.value
    }

    pub fn complement(self) -> Self {
        Self {
            value: clamp_probability(UNIT_F64 - self.value),
        }
    }

    pub fn widened(self, band: Self) -> Self {
        Self {
            value: clamp_probability(self.value + band.value),
        }
    }

    pub fn narrowed(self, band: Self) -> Self {
        Self {
            value: clamp_probability(self.value - band.value),
        }
    }
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
    fn probability_new_rejects_out_of_range_and_non_finite() {
        assert_eq!(
            Probability::new(HALF_F64).map(Probability::value),
            Some(HALF_F64)
        );
        assert_eq!(
            Probability::new(ZERO_F64).map(Probability::value),
            Some(ZERO_F64)
        );
        assert_eq!(
            Probability::new(UNIT_F64).map(Probability::value),
            Some(UNIT_F64)
        );
        assert_eq!(Probability::new(f64::NAN), None);
        assert_eq!(Probability::new(-0.001), None);
        assert_eq!(Probability::new(1.001), None);
        assert_eq!(Probability::new(f64::INFINITY), None);
        assert_eq!(Probability::new(f64::NEG_INFINITY), None);
    }

    #[test]
    fn probability_clamped_rejects_non_finite_and_clamps_finite_values() {
        assert_eq!(
            Probability::clamped(-0.001).map(Probability::value),
            Some(ZERO_F64)
        );
        assert_eq!(
            Probability::clamped(1.001).map(Probability::value),
            Some(UNIT_F64)
        );
        assert_eq!(
            Probability::clamped(HALF_F64).map(Probability::value),
            Some(HALF_F64)
        );
        assert_eq!(Probability::clamped(f64::NAN), None);
        assert_eq!(Probability::clamped(f64::INFINITY), None);
        assert_eq!(Probability::clamped(f64::NEG_INFINITY), None);
    }

    #[test]
    fn probability_arithmetic_helpers_keep_values_in_bounds() {
        let probability = Probability::new(0.75).expect("valid probability");
        let band = Probability::new(HALF_F64).expect("valid probability");

        assert_eq!(probability.complement().value(), 0.25);
        assert_eq!(probability.widened(band).value(), UNIT_F64);
        assert_eq!(probability.narrowed(band).value(), 0.25);
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
