//! Single-precision derivation for NautilusTrader catalog types.
//!
//! Invariant: all decimal-place counting and rescaling passes through this
//! module so that f64 round-trip artifacts (e.g. a 17-decimal rendering of a
//! true 5-decimal value) are recovered consistently in one place, and the
//! derived precision never exceeds NautilusTrader's fixed-precision cap.

use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use std::str::FromStr;

/// NautilusTrader standard `FIXED_PRECISION`. Source decimals are rounded to
/// this many places before precision is derived or a `Price`/`Quantity` is
/// built: some encodings render values as f64 round-trip artifacts (a true
/// `0.09656` as `"0.09655999999999999"`) whose spurious 15-17th place digits
/// would otherwise blow past the catalog's cap. Rounding recovers the intended
/// tick value and bounds precision to what the catalog can store.
pub const NT_FIXED_PRECISION: u32 = 9;

/// Significant decimal places of a decimal string after rounding to
/// [`NT_FIXED_PRECISION`] and stripping trailing zeros (`"643.3"` → 1,
/// `"5995"` → 0, `"0.09655999999999999"` → 5).
///
/// This is the authoritative precision-derivation path for all archive families
/// that infer precision from observed data rather than from a configured
/// increment. Rounding to [`NT_FIXED_PRECISION`] before counting eliminates
/// f64 round-trip artifacts so the derived value matches the true tick scale.
pub fn decimal_places(value: &str) -> Result<u8> {
    let decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    u8::try_from(decimal.round_dp(NT_FIXED_PRECISION).normalize().scale())
        .context("decimal scale exceeds u8")
}

/// Rescale a decimal string to exactly `precision` decimal places, after first
/// rounding to [`NT_FIXED_PRECISION`] to eliminate f64 round-trip artifacts.
///
/// Fails if the significant scale (after artifact rounding and trailing-zero
/// stripping) exceeds `precision` — a genuine sub-precision digit is refused,
/// never silently rounded away.
pub fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value)
        .with_context(|| format!("decimal {value:?}"))?
        .round_dp(NT_FIXED_PRECISION);
    ensure!(
        decimal.normalize().scale() <= u32::from(precision),
        "value {value:?} has more precision than venue allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Rescale a decimal string to exactly `precision` decimal places, after first
/// rounding to [`NT_FIXED_PRECISION`] to eliminate f64 round-trip artifacts.
///
/// Like [`rescaled`] but the error message uses "instrument" rather than
/// "venue" — suited to the configured-increment path where precision is
/// supplied per-instrument rather than derived from a scan of all observed
/// levels.
///
/// Trailing-zero padding (e.g. `"1.0"` at precision 0) is tolerated: it is
/// lossless when the dropped digit is zero. A genuine sub-precision digit
/// (e.g. `"1.05"` at precision 0) is refused.
pub fn rescaled_to(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value)
        .with_context(|| format!("decimal {value:?}"))?
        .round_dp(NT_FIXED_PRECISION);
    ensure!(
        decimal.normalize().scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

#[cfg(test)]
mod tests {
    use super::{decimal_places, rescaled, rescaled_to};

    #[test]
    fn decimal_places_rounds_f64_round_trip_artifacts() {
        // Some encodings render values as f64 round-trip noise (a true 0.09656
        // as "0.09655999999999999"); rounding to NT_FIXED_PRECISION recovers
        // the intended tick and keeps the derived precision within the catalog's
        // 9-place cap rather than blowing past it at the 17th decimal.
        assert_eq!(decimal_places("0.09655999999999999").unwrap(), 5);
        assert_eq!(decimal_places("0.09382000000000000").unwrap(), 5);
        assert_eq!(decimal_places("0.09382").unwrap(), 5);
        assert_eq!(decimal_places("5995").unwrap(), 0);
    }

    #[test]
    fn decimal_places_reads_clean_precision() {
        assert_eq!(decimal_places("643.3").unwrap(), 1);
        assert_eq!(decimal_places("5995").unwrap(), 0);
        assert_eq!(decimal_places("0.0001").unwrap(), 4);
    }

    #[test]
    fn rescaled_eliminates_artifact_and_yields_clean_tick() {
        assert_eq!(rescaled("0.09655999999999999", 5).unwrap(), "0.09656");
        assert_eq!(rescaled("0.09382000000000000", 5).unwrap(), "0.09382");
    }

    #[test]
    fn rescaled_rejects_genuine_subprecision() {
        let err = rescaled("1.05", 0).expect_err("sub-precision must be refused");
        assert!(err.to_string().contains("more precision"), "{err}");
    }

    #[test]
    fn rescaled_to_tolerates_trailing_zero_but_rejects_subprecision() {
        // Trailing-zero padding is dropped losslessly.
        assert_eq!(rescaled_to("1.0", 0).unwrap(), "1");
        assert_eq!(rescaled_to("764.0", 0).unwrap(), "764");
        assert_eq!(rescaled_to("636.50", 1).unwrap(), "636.5");
        // A genuine sub-precision digit is refused, never silently rounded.
        let err = rescaled_to("1.05", 0).expect_err("sub-precision must be refused");
        assert!(err.to_string().contains("more precision"), "{err}");
    }

    #[test]
    fn rescaled_to_eliminates_artifact_and_yields_clean_tick() {
        assert_eq!(rescaled_to("0.09655999999999999", 5).unwrap(), "0.09656");
    }
}
