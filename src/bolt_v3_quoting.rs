//! Shared maker quote target math.

use crate::bolt_v3_numeric::{HALF_F64, UNIT_F64, ZERO_F64, sanitize_open_probability};

/// Resting quote side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteSide {
    /// A resting bid.
    Buy,
    /// A resting ask/offer.
    Sell,
}

/// One target leg for a two-sided maker quote.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteTargetLeg {
    pub side: QuoteSide,
    pub price: f64,
}

/// Two target legs produced by shared quote layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteTargets {
    pub leg_a: QuoteTargetLeg,
    pub leg_b: QuoteTargetLeg,
}

/// Pure scalar quote-layout inputs for one market family.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FamilyQuoteInputs {
    pub fair: f64,
    pub reservation_bid: f64,
    pub reservation_ask: f64,
    pub inventory_skew: f64,
    pub half_spread_floor: f64,
    pub max_half_spread: f64,
    pub eps: f64,
    pub tau: f64,
    pub reference_tau: f64,
    pub time_widen_cap: f64,
}

/// Two binary outcome-token bid prices.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinaryLegPrices {
    pub yes_price: f64,
    pub no_price: f64,
}

/// Resolve a fail-closed reservation band around `fair`.
pub fn resolve_band(
    fair: f64,
    reservation_bid: f64,
    reservation_ask: f64,
    half_spread_floor: f64,
    max_half_spread: f64,
) -> Option<(f64, f64)> {
    if !fair.is_finite()
        || !reservation_bid.is_finite()
        || !reservation_ask.is_finite()
        || !half_spread_floor.is_finite()
        || !max_half_spread.is_finite()
    {
        return None;
    }
    if half_spread_floor < ZERO_F64 || max_half_spread < half_spread_floor {
        return None;
    }
    if !(reservation_bid <= fair && fair <= reservation_ask) {
        return None;
    }

    let half_spread = (reservation_ask - reservation_bid) * HALF_F64;
    if half_spread < ZERO_F64 {
        return None;
    }
    let half_spread = half_spread.max(half_spread_floor);
    if half_spread > max_half_spread {
        return None;
    }

    let mid = (reservation_bid + reservation_ask) * HALF_F64;
    Some((mid - half_spread, mid + half_spread))
}

/// Time-to-expiry half-spread widening factor.
pub fn time_widening_factor(tau: f64, reference_tau: f64, cap: f64) -> Option<f64> {
    if !tau.is_finite() || !reference_tau.is_finite() || !cap.is_finite() {
        return None;
    }
    if !(tau > ZERO_F64 && reference_tau > ZERO_F64) {
        return None;
    }
    if cap < UNIT_F64 {
        return None;
    }

    let raw = (reference_tau / tau).sqrt();
    if !raw.is_finite() {
        return None;
    }
    Some(raw.max(UNIT_F64).min(cap))
}

/// Neutral reward-shaping offset for this pure quote-layout slice.
pub fn reward_shaping_offset() -> f64 {
    ZERO_F64
}

/// Compose binary YES/NO bid legs from the shared scalar precedence stack.
#[allow(clippy::too_many_arguments)]
pub fn compose_binary_legs(
    p_up: f64,
    reservation_bid: f64,
    reservation_ask: f64,
    half_spread_floor: f64,
    max_half_spread: f64,
    tau: f64,
    reference_tau: f64,
    time_widen_cap: f64,
    inventory_skew: f64,
    eps: f64,
) -> Option<BinaryLegPrices> {
    let (resolved_bid, resolved_ask) = resolve_band(
        p_up,
        reservation_bid,
        reservation_ask,
        half_spread_floor,
        max_half_spread,
    )?;
    let mid = HALF_F64 * (resolved_bid + resolved_ask);
    let half_spread = HALF_F64 * (resolved_ask - resolved_bid);

    let factor = time_widening_factor(tau, reference_tau, time_widen_cap)?;
    let widened_half = half_spread * factor;
    let widened_bid = mid - widened_half;
    let widened_ask = mid + widened_half;

    let reward = reward_shaping_offset();
    let yes_raw = widened_bid - inventory_skew + reward;
    let no_raw = (UNIT_F64 - widened_ask) + inventory_skew + reward;

    let yes_price = sanitize_open_probability(yes_raw, eps)?;
    let no_price = sanitize_open_probability(no_raw, eps)?;

    if yes_price + no_price >= UNIT_F64 {
        return None;
    }
    let yes_ask = UNIT_F64 - no_price;
    if !(yes_price <= p_up && p_up <= yes_ask) {
        return None;
    }

    Some(BinaryLegPrices {
        yes_price,
        no_price,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_numeric::{UNIT_F64, ZERO_F64};

    const EPSILON: f64 = 1e-9;
    const TEST_EPS: f64 = 1e-6;
    const REF_TAU: f64 = 3_600.0;
    const WIDEN_CAP: f64 = 10.0;

    #[test]
    fn resolve_band_centers_floored_band_on_the_reservation_mid() {
        let (bid, ask) = resolve_band(0.60, 0.58, 0.62, 0.0, 1.0).expect("valid band");

        assert!((bid - 0.58).abs() < EPSILON);
        assert!((ask - 0.62).abs() < EPSILON);
    }

    #[test]
    fn resolve_band_enforces_the_half_spread_floor() {
        let (bid, ask) = resolve_band(0.50, 0.495, 0.505, 0.02, 1.0).expect("floored band");

        assert!((bid - 0.48).abs() < EPSILON);
        assert!((ask - 0.52).abs() < EPSILON);
    }

    #[test]
    fn resolve_band_rejects_degenerate_bands() {
        assert!(resolve_band(0.50, 0.40, 0.60, 0.0, 0.05).is_none());
        assert!(resolve_band(0.50, 0.70, 0.30, 0.0, 1.0).is_none());
        assert!(resolve_band(0.40, 0.60, 0.70, 0.0, 1.0).is_none());
        assert!(resolve_band(0.60, 0.30, 0.40, 0.0, 1.0).is_none());
        assert!(resolve_band(f64::NAN, 0.4, 0.6, 0.0, 1.0).is_none());
        assert!(resolve_band(0.5, f64::INFINITY, 0.6, 0.0, 1.0).is_none());
        assert!(resolve_band(0.5, 0.4, 0.6, -0.01, 1.0).is_none());
        assert!(resolve_band(0.5, 0.4, 0.6, 0.10, 0.05).is_none());
    }

    #[test]
    fn time_widening_widens_only_and_respects_cap() {
        assert!(
            (time_widening_factor(REF_TAU, REF_TAU, WIDEN_CAP).unwrap() - UNIT_F64).abs() < EPSILON
        );
        assert!(
            (time_widening_factor(4.0 * REF_TAU, REF_TAU, WIDEN_CAP).unwrap() - UNIT_F64).abs()
                < EPSILON
        );

        let quarter = time_widening_factor(REF_TAU / 4.0, REF_TAU, WIDEN_CAP).unwrap();
        assert!((quarter - 2.0).abs() < EPSILON);
        let capped = time_widening_factor(REF_TAU / 1_000_000.0, REF_TAU, 3.0).unwrap();
        assert!((capped - 3.0).abs() < EPSILON);
    }

    #[test]
    fn time_widening_and_reward_shaping_fail_closed_or_pass_through() {
        assert!(time_widening_factor(ZERO_F64, REF_TAU, WIDEN_CAP).is_none());
        assert!(time_widening_factor(-1.0, REF_TAU, WIDEN_CAP).is_none());
        assert!(time_widening_factor(REF_TAU, ZERO_F64, WIDEN_CAP).is_none());
        assert!(time_widening_factor(REF_TAU, REF_TAU, 0.5).is_none());
        assert!(time_widening_factor(f64::NAN, REF_TAU, WIDEN_CAP).is_none());
        assert_eq!(reward_shaping_offset(), ZERO_F64);
    }

    fn compose_symmetric(fair: f64, half_spread: f64, skew: f64) -> Option<BinaryLegPrices> {
        compose_binary_legs(
            fair,
            fair - half_spread,
            fair + half_spread,
            ZERO_F64,
            UNIT_F64,
            REF_TAU,
            REF_TAU,
            WIDEN_CAP,
            skew,
            TEST_EPS,
        )
    }

    #[test]
    fn compose_binary_legs_emits_open_interval_yes_no_bids() {
        let legs = compose_symmetric(0.60, 0.02, 0.0).expect("non-degenerate inputs quote");

        assert!(legs.yes_price < 0.60 && legs.yes_price > 0.50);
        assert!(legs.no_price < 0.40 && legs.no_price > 0.30);
        assert!(legs.yes_price > TEST_EPS && legs.yes_price < UNIT_F64 - TEST_EPS);
        assert!(legs.no_price > TEST_EPS && legs.no_price < UNIT_F64 - TEST_EPS);
    }

    #[test]
    fn compose_binary_legs_applies_widening_and_prunes_degenerate_pairs() {
        let calm = compose_symmetric(0.50, 0.04, 0.0).expect("calm band quotes");
        let near = compose_binary_legs(
            0.50,
            0.46,
            0.54,
            ZERO_F64,
            UNIT_F64,
            REF_TAU / 4.0,
            REF_TAU,
            WIDEN_CAP,
            0.0,
            TEST_EPS,
        )
        .expect("widened band still quotes");

        assert!(near.yes_price < calm.yes_price);
        assert!(near.no_price < calm.no_price);
        assert!(compose_symmetric(0.50, 0.02, 0.20).is_none());
        assert!(compose_symmetric(0.01, 0.5, 0.0).is_none());
        assert!(compose_symmetric(0.50, 0.0, 0.0).is_none());
    }

    #[test]
    fn compose_binary_legs_rejects_crossed_horizon_and_eps_failures() {
        assert!(
            compose_binary_legs(
                0.50, 0.70, 0.30, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS,
            )
            .is_none()
        );
        assert!(
            compose_binary_legs(
                0.50, 0.48, 0.52, ZERO_F64, UNIT_F64, ZERO_F64, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS,
            )
            .is_none()
        );
        assert!(
            compose_binary_legs(
                0.50, 0.48, 0.52, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, 0.5,
            )
            .is_none()
        );
    }
}
