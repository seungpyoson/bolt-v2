//! Shared maker quote target math.

use crate::bolt_v3_maker_model::GmReservationBand;
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
    /// Dollar order notional for this leg, sized off the protective half-spread
    /// (the GM/CG edge proxy) by `maker_robust_size`, never off directional EV.
    pub size_notional: f64,
}

/// Two target legs produced by shared quote layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuoteTargets {
    pub leg_a: QuoteTargetLeg,
    pub leg_b: QuoteTargetLeg,
}

/// Pure quote-layout inputs for one market family. The fair value and the
/// reservation edges arrive together as a single [`GmReservationBand`] minted by
/// `gm_binary_quote` — they cannot be supplied as three independent scalars, so
/// the layout's fair can never disagree with the band edges it is laid out around.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FamilyQuoteInputs {
    pub band: GmReservationBand,
    pub inventory_skew: f64,
    pub half_spread_floor: f64,
    pub max_half_spread: f64,
    pub eps: f64,
    pub tau: f64,
    pub reference_tau: f64,
    pub time_widen_cap: f64,
    /// Operator per-order dollar target for each maker quote leg. Scaled by the
    /// captured edge (relative to `max_half_spread`) in `maker_robust_size`.
    pub order_notional_target: f64,
    /// Operator cap on per-leg dollar notional; clamps the sized target.
    pub maximum_position_notional: f64,
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

/// Compose binary YES/NO bid legs around a Glosten-Milgrom reservation `band` —
/// the sole carrier of the fair value (`band.p_up()`) and the reservation edges,
/// so the layout's fair is definitionally the value the band was derived from.
#[allow(clippy::too_many_arguments)]
pub fn compose_binary_legs(
    band: GmReservationBand,
    half_spread_floor: f64,
    max_half_spread: f64,
    tau: f64,
    reference_tau: f64,
    time_widen_cap: f64,
    inventory_skew: f64,
    eps: f64,
) -> Option<BinaryLegPrices> {
    let p_up = band.p_up();
    let (resolved_bid, resolved_ask) = resolve_band(
        p_up,
        band.bid(),
        band.ask(),
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
    use crate::bolt_v3_maker_model::gm_binary_quote;
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
        assert!(
            (time_widening_factor(2.0 * REF_TAU, REF_TAU, WIDEN_CAP).unwrap() - UNIT_F64).abs()
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

    #[test]
    fn compose_binary_legs_emits_open_interval_yes_no_bids() {
        // The only way to obtain a band is gm_binary_quote, so the test exercises
        // the canonical chain end to end: fair 0.60, mu 0.04 -> an interior band.
        let band = gm_binary_quote(0.60, 0.04).expect("interior band");
        let legs = compose_binary_legs(
            band, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS,
        )
        .expect("non-degenerate inputs quote");

        // At the reference horizon (widening factor 1), zero floor and zero skew,
        // the layout is an identity on the band: the YES bid is EXACTLY the band's
        // bid and the NO bid is EXACTLY 1 - the band's ask. This pins the spread
        // MAGNITUDE, not just its sign — any half-spread, widening-factor, floor or
        // skew inflation moves the legs off the band edges and fails here (a
        // regression a sign-only `< fair` check would silently pass).
        assert!((legs.yes_price - band.bid()).abs() < EPSILON);
        assert!((legs.no_price - (UNIT_F64 - band.ask())).abs() < EPSILON);
        // The YES bid sits below the fair; the NO bid sits below (1 - fair).
        assert!(legs.yes_price < band.p_up());
        assert!(legs.no_price < UNIT_F64 - band.p_up());
        // Both legs stay strictly inside the open probability interval.
        assert!(legs.yes_price > TEST_EPS && legs.yes_price < UNIT_F64 - TEST_EPS);
        assert!(legs.no_price > TEST_EPS && legs.no_price < UNIT_F64 - TEST_EPS);
    }

    #[test]
    fn compose_binary_legs_applies_widening_and_prunes_degenerate_pairs() {
        let band = gm_binary_quote(0.50, 0.08).expect("interior band");
        let calm = compose_binary_legs(
            band, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS,
        )
        .expect("calm band quotes");
        let near = compose_binary_legs(
            band,
            ZERO_F64,
            UNIT_F64,
            REF_TAU / 4.0,
            REF_TAU,
            WIDEN_CAP,
            0.0,
            TEST_EPS,
        )
        .expect("widened band still quotes");

        // A shorter horizon widens the defensive spread, pushing both bids down.
        assert!(near.yes_price < calm.yes_price);
        assert!(near.no_price < calm.no_price);
        // ...and it widens by EXACTLY the time factor: the YES leg's deviation from
        // the band mid scales by the factor (itself independently pinned in
        // time_widening_widens_only_and_respects_cap), so a factor-magnitude bug that
        // still preserves the `near < calm` direction is caught here too.
        let factor = time_widening_factor(REF_TAU / 4.0, REF_TAU, WIDEN_CAP).expect("factor");
        let mid = (band.bid() + band.ask()) / 2.0;
        assert!(((mid - near.yes_price) - factor * (mid - calm.yes_price)).abs() < EPSILON);
        // A small positive inventory skew (net-long-YES) must lean the quote to
        // REDUCE risk: relative to the neutral (zero-skew) `calm` legs it lowers the
        // YES bid and raises the NO bid. This pins the SIGN of the skew, not merely
        // that a large skew prunes -- a sign-flipped yes_raw/no_raw would move both
        // legs the wrong way and fail here while still passing the large-skew prune
        // assertion below.
        let skewed = compose_binary_legs(
            band, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.01, TEST_EPS,
        )
        .expect("small skew still quotes");
        assert!(skewed.yes_price < calm.yes_price);
        assert!(skewed.no_price > calm.no_price);
        // A large inventory skew pushes the implied YES ask (1 - no_price) below the
        // fair, so the straddle guard (yes_price <= p_up <= yes_ask) fails -> pruned.
        assert!(
            compose_binary_legs(
                band, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.20, TEST_EPS
            )
            .is_none()
        );
        // A zero-spread band (mu -> 0) collapses yes + no to 1 -> pruned.
        let flat = gm_binary_quote(0.50, 0.0).expect("zero-mu band");
        assert!(
            compose_binary_legs(
                flat, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS
            )
            .is_none()
        );
    }

    #[test]
    fn compose_binary_legs_rejects_horizon_and_eps_failures() {
        // A well-formed band still fails closed when the layout knobs are degenerate.
        // (The crossed/non-straddling band is unconstructable through gm_binary_quote
        // and is covered at the scalar layer in resolve_band_rejects_degenerate_bands.)
        let band = gm_binary_quote(0.50, 0.04).expect("interior band");
        // Zero time-to-expiry: the widening factor is undefined -> fail closed.
        assert!(
            compose_binary_legs(
                band, ZERO_F64, UNIT_F64, ZERO_F64, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS
            )
            .is_none()
        );
        // A degenerate epsilon collapses the open-interval sanitizer via its eps
        // COLLAR branch -> fail closed.
        assert!(
            compose_binary_legs(
                band, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, 0.5
            )
            .is_none()
        );
        // A VALID epsilon but a wide band under heavy widening drives a leg strictly
        // below the open interval, so sanitize_open_probability fails on its VALUE
        // branch (eps < value < 1-eps), distinct from the collar branch above. This
        // is the integration-layer guard that a widening/skew bug pushing a leg out
        // of (0, 1) fails closed through the canonical chain: band (0.05, 0.95) at a
        // 1/100 horizon widens the half-spread 10x to 4.5, so yes_raw = 0.5 - 4.5 =
        // -4.0 -> rejected. The downstream sum and straddle guards do NOT catch a
        // negative leg, so only this value-branch check holds the line.
        assert!(
            compose_binary_legs(
                gm_binary_quote(0.50, 0.9).expect("interior band"),
                ZERO_F64,
                UNIT_F64,
                REF_TAU / 100.0,
                REF_TAU,
                WIDEN_CAP,
                0.0,
                TEST_EPS,
            )
            .is_none()
        );
    }
}
