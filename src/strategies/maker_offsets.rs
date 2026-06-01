//! Offset composition for the binary-oracle maker (W3 — FR-022: open-interval
//! quotes + a defined precedence + clamp/prune).
//!
//! The maker's two quote legs are built by stacking several *offsets* onto a fair
//! value. FR-022 requires that this stacking have a **defined precedence** and a
//! terminal **clamp/prune** so every emitted leg lands strictly inside the open
//! interval `(ε, 1−ε)` and the YES/NO pair stays jointly consistent and
//! non-self-crossing. This module owns that precedence in exactly one place; the
//! [`crate::strategies::maker_quote::BinaryFamily`] layout calls it rather than
//! re-deriving the order, so a new offset slot is added here and nowhere else.
//!
//! ## Precedence (applied in this fixed order)
//!
//! 1. **Base reservation band** — the model `[reservation_bid, reservation_ask]`
//!    floored/capped through the single shared
//!    [`crate::strategies::maker_quote::resolve_band`]; never re-implemented here.
//! 2. **Time-widening** — multiply the *resolved* half-spread by
//!    [`time_widening_factor`] (always `≥ 1.0`: widen only). A binary's price
//!    variance blows up `~1/√τ` into expiry, so the maker must WIDEN, never
//!    tighten, as time-to-expiry `τ` shrinks. This is a distinct concern from the
//!    `max_half_spread` cap (which detects a stale/toxic *model* spread): the
//!    factor is bounded by its own configured cap.
//! 3. **Inventory skew** — lean the pair against net inventory (YES bid down / NO
//!    bid up for a net-long-YES maker), the secondary
//!    [`crate::strategies::maker_model::inventory_skew`] term.
//! 4. **Reward-shaping slot** — additive reward-capture offset (FR-060, W7). It is
//!    a documented pass-through (contributes exactly zero) until the W7 reward
//!    layer owns it; it occupies its precedence position now so wiring it later
//!    never reorders the stack.
//! 5. **Terminal clamp + prune** — every leg through
//!    [`crate::bolt_v3_numeric::sanitize_open_probability`] (strict `(ε, 1−ε)`),
//!    then the joint guards: the two bids must leave positive edge (`yes + no < 1`)
//!    and the implied YES market must still bracket fair (no skew slides the pair
//!    wholly off fair). Any failure is a fail-closed `None` — no quote this tick.
//!
//! Pure: no NautilusTrader type, no async, no I/O, no clock; every numeric literal
//! comes from [`crate::bolt_v3_numeric`].

use crate::bolt_v3_numeric::{HALF_F64, UNIT_F64, ZERO_F64, sanitize_open_probability};
use crate::strategies::maker_quote::resolve_band;

/// The two-sided binary quote legs produced by [`compose_binary_legs`], both in
/// `(ε, 1−ε)` probability units: a YES bid and a NO bid on the two outcome tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinaryLegPrices {
    /// The YES-token bid price.
    pub yes_price: f64,
    /// The NO-token bid price.
    pub no_price: f64,
}

/// The reward-shaping offset (precedence slot 4, FR-060 / W7).
///
/// Returns the additive price-unit offset the reward layer wants applied to the
/// half-spread. Until W7 owns it this is a documented pass-through that always
/// contributes [`ZERO_F64`]; it exists so the precedence slot is occupied and
/// wiring the real reward policy later never reorders the stack. Fail-closed only
/// in the trivial sense — a pass-through cannot produce a non-finite value.
pub fn reward_shaping_offset() -> f64 {
    ZERO_F64
}

/// The time-widening multiplier for the half-spread (precedence slot 2).
///
/// A binary outcome token's price variance blows up `~1/√τ` as time-to-expiry `τ`
/// shrinks toward settlement, so the maker must WIDEN its spread into expiry, not
/// tighten it. The factor grows as `τ` falls below the reference horizon and is
/// `1.0` (no change) at or above the reference — widening is one-directional:
///
/// ```text
/// factor = clamp( sqrt(reference_tau / tau), 1.0, cap )
/// ```
///
/// so for `τ ≥ reference_tau` the raw ratio is `≤ 1` and is clamped UP to `1.0`
/// (never tighten), and for `τ < reference_tau` it grows like `1/√τ`, bounded
/// above by `cap`.
///
/// Fail-closed (returns `None`) when:
/// - any input is non-finite;
/// - `tau` or `reference_tau` is not strictly positive (a non-positive horizon is
///   meaningless and `reference_tau / tau` would be degenerate);
/// - `cap` is below `1.0` — a cap that could force tightening is a
///   misconfiguration (the floor of the factor is always `1.0`).
pub fn time_widening_factor(tau: f64, reference_tau: f64, cap: f64) -> Option<f64> {
    if !tau.is_finite() || !reference_tau.is_finite() || !cap.is_finite() {
        return None;
    }
    if !(tau > ZERO_F64 && reference_tau > ZERO_F64) {
        return None;
    }
    // A cap below the no-widening floor could force a tighten — reject it.
    if cap < UNIT_F64 {
        return None;
    }
    let raw = (reference_tau / tau).sqrt();
    // `raw` is finite and non-negative here (positive finite inputs); a NaN guard
    // is redundant but cheap on this money path.
    if !raw.is_finite() {
        return None;
    }
    // Widen only: never below 1.0; bounded above by the configured cap.
    Some(raw.max(UNIT_F64).min(cap))
}

/// Compose the two binary quote legs by applying the FR-022 precedence stack in
/// its fixed order, returning the YES/NO bid pair strictly inside `(ε, 1−ε)`.
///
/// `p_up` is the sanitized fair probability (the caller has already validated it
/// on the closed `[0, 1]`). The arguments mirror the precedence:
/// `[reservation_bid, reservation_ask]` + `[half_spread_floor, max_half_spread]`
/// drive slot 1; `tau` / `reference_tau` / `time_widen_cap` drive slot 2;
/// `inventory_skew` drives slot 3; `eps` drives the slot-5 clamp.
///
/// Fail-closed (returns `None` — "no quotable target this tick") at any stage:
/// - the shared [`resolve_band`] rejects the band (non-finite, misconfigured
///   floor/cap, non-bracketing, or wider than the cap);
/// - [`time_widening_factor`] rejects the horizon inputs;
/// - any leg falls outside the open interval `(eps, 1−eps)` after clamping;
/// - the two bids fail to leave positive edge (`yes + no ≥ 1`);
/// - inventory skew slides the implied YES market wholly off the fair `p_up`.
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
    // Slot 1 — base reservation band through the single shared resolver. This
    // owns the floor/cap and the pre-skew bracket discipline; never duplicated.
    let (resolved_bid, resolved_ask) = resolve_band(
        p_up,
        reservation_bid,
        reservation_ask,
        half_spread_floor,
        max_half_spread,
    )?;
    let mid = HALF_F64 * (resolved_bid + resolved_ask);
    let half_spread = HALF_F64 * (resolved_ask - resolved_bid);

    // Slot 2 — time-widening (widen only). Applied to the resolved half-spread, a
    // concern distinct from the model-spread cap enforced in slot 1.
    let factor = time_widening_factor(tau, reference_tau, time_widen_cap)?;
    let widened_half = half_spread * factor;
    let widened_bid = mid - widened_half;
    let widened_ask = mid + widened_half;

    // Slot 4 — reward-shaping pass-through (occupies its slot; zero until W7).
    let reward = reward_shaping_offset();

    // Slot 3 — inventory skew leans the pair; slot 4 reward offset is folded in at
    // its precedence position. A positive skew (net long YES) lowers the YES bid
    // and raises the NO bid, leaning toward the lighter side.
    let yes_raw = widened_bid - inventory_skew + reward;
    let no_raw = (UNIT_F64 - widened_ask) + inventory_skew + reward;

    // Slot 5 — terminal clamp to the OPEN interval, then the joint prune guards.
    let yes_price = sanitize_open_probability(yes_raw, eps)?;
    let no_price = sanitize_open_probability(no_raw, eps)?;

    // Positive edge: a YES bid + NO bid summing to ≥ 1 is a guaranteed loss
    // (exactly one token resolves to 1). The skew cancels in this sum, so this
    // only proves the widened band still has positive width — it is NOT the
    // bracket check below.
    if yes_price + no_price >= UNIT_F64 {
        return None;
    }
    // Non-self-crossing bracket: the implied YES market is
    // `[yes_price, UNIT_F64 − no_price]`. A `|skew|` larger than the widened
    // half-spread slides this whole market to one side of `p_up` while each leg
    // stays a valid probability and the sum stays < 1 — that must fail closed.
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

    /// Tolerance for closed-form float comparisons.
    const EPSILON: f64 = 1e-9;
    /// A small but non-degenerate open-interval collar for the composition tests.
    const TEST_EPS: f64 = 1e-6;
    /// Reference time-to-expiry horizon; equal `tau` gives a factor of exactly 1.
    const REF_TAU: f64 = 3_600.0;
    /// A permissive widening cap that never binds in the geometry tests.
    const WIDEN_CAP: f64 = 10.0;

    #[test]
    fn time_widening_is_one_at_or_above_the_reference_horizon() {
        // At the reference horizon the factor is exactly 1.0 (no widening).
        assert!(
            (time_widening_factor(REF_TAU, REF_TAU, WIDEN_CAP).unwrap() - UNIT_F64).abs() < EPSILON
        );
        // Well above the reference, the raw ratio is < 1 but is clamped up to 1.0
        // — the maker never tightens.
        assert!(
            (time_widening_factor(4.0 * REF_TAU, REF_TAU, WIDEN_CAP).unwrap() - UNIT_F64).abs()
                < EPSILON
        );
    }

    #[test]
    fn time_widening_grows_as_tau_shrinks_into_expiry() {
        // τ at 1/4 of the reference: sqrt(4) = 2.0× the half-spread.
        let quarter = time_widening_factor(REF_TAU / 4.0, REF_TAU, WIDEN_CAP).unwrap();
        assert!((quarter - 2.0).abs() < EPSILON);
        // Monotone: less time to expiry -> a strictly wider factor.
        let near = time_widening_factor(REF_TAU / 16.0, REF_TAU, WIDEN_CAP).unwrap();
        assert!(near > quarter, "factor must grow as tau shrinks");
    }

    #[test]
    fn time_widening_is_bounded_above_by_the_cap() {
        // sqrt(reference/tiny) would explode, but the cap binds it.
        let cap = 3.0;
        let f = time_widening_factor(REF_TAU / 1_000_000.0, REF_TAU, cap).unwrap();
        assert!(
            (f - cap).abs() < EPSILON,
            "near-expiry blowup must be capped"
        );
    }

    #[test]
    fn time_widening_fails_closed_on_bad_horizon_inputs() {
        // Non-positive tau / reference are meaningless horizons.
        assert!(time_widening_factor(ZERO_F64, REF_TAU, WIDEN_CAP).is_none());
        assert!(time_widening_factor(-1.0, REF_TAU, WIDEN_CAP).is_none());
        assert!(time_widening_factor(REF_TAU, ZERO_F64, WIDEN_CAP).is_none());
        // A cap below 1.0 could force a tighten.
        assert!(time_widening_factor(REF_TAU, REF_TAU, 0.5).is_none());
        // Non-finite inputs.
        assert!(time_widening_factor(f64::NAN, REF_TAU, WIDEN_CAP).is_none());
        assert!(time_widening_factor(REF_TAU, f64::INFINITY, WIDEN_CAP).is_none());
        assert!(time_widening_factor(REF_TAU, REF_TAU, f64::NAN).is_none());
    }

    #[test]
    fn reward_shaping_slot_is_a_zero_pass_through_for_now() {
        // W7 owns this; until then it must contribute exactly zero so it never
        // perturbs the composed legs.
        assert_eq!(reward_shaping_offset(), ZERO_F64);
    }

    /// Compose with a symmetric reservation band of `half_spread` around `fair`, a
    /// permissive floor/cap, the reference horizon (factor 1), and the small test
    /// collar — isolating the geometry from the widening and gate behaviour.
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
    fn composes_two_open_interval_bids_around_fair() {
        let legs = compose_symmetric(0.60, 0.02, 0.0).expect("non-degenerate inputs quote");
        // YES bid sits below P(up)=0.60, NO bid below P(down)=0.40, both interior.
        assert!(legs.yes_price < 0.60 && legs.yes_price > 0.50);
        assert!(legs.no_price < 0.40 && legs.no_price > 0.30);
        assert!(legs.yes_price > TEST_EPS && legs.yes_price < UNIT_F64 - TEST_EPS);
        assert!(legs.no_price > TEST_EPS && legs.no_price < UNIT_F64 - TEST_EPS);
    }

    #[test]
    fn time_widening_widens_the_composed_legs_into_expiry() {
        // Far from expiry (factor 1): a tight band quotes both bids near fair.
        let calm = compose_symmetric(0.50, 0.04, 0.0).expect("calm band quotes");
        // Near expiry (tau = reference/4 -> factor 2): the same model band is
        // widened, pushing both bids strictly lower.
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
        assert!(
            near.yes_price < calm.yes_price,
            "widening must lower the YES bid"
        );
        assert!(
            near.no_price < calm.no_price,
            "widening must lower the NO bid"
        );
        // Even widened, the legs stay strictly inside the open interval.
        assert!(near.yes_price > TEST_EPS && near.no_price > TEST_EPS);
    }

    #[test]
    fn rejects_a_skew_that_slides_the_pair_off_fair() {
        // A skew larger than the half-spread slides the implied YES market wholly
        // to one side of fair while each leg is a valid probability and the sum
        // stays < 1 (skew cancels in the sum) — must fail closed at slot 5.
        assert!(compose_symmetric(0.50, 0.02, 0.20).is_none());
        // A lean within the half-spread still quotes and brackets fair.
        let leaned = compose_symmetric(0.50, 0.04, 0.02).expect("a within-band lean quotes");
        assert!(leaned.yes_price <= 0.50 && 0.50 <= UNIT_F64 - leaned.no_price);
    }

    #[test]
    fn rejects_a_leg_driven_to_the_open_interval_boundary() {
        // A YES bid pushed to exactly 0 (or the eps collar) by an extreme band is
        // a degenerate quote: sanitize_open_probability rejects it (the SC-002
        // latent bug that the closed-interval sanitizer used to admit).
        assert!(compose_symmetric(0.01, 0.5, 0.0).is_none());
        // Band [0.40, 0.60] composes both legs at 0.40. A narrow eps=0.1 collar
        // leaves 0.40 interior, so the pair quotes.
        let inside = compose_binary_legs(
            0.50, 0.40, 0.60, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, 0.1,
        );
        assert!(inside.is_some());
        // A wider eps=0.45 collar swallows the 0.40 leg edge -> pruned.
        let excluded = compose_binary_legs(
            0.50, 0.40, 0.60, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, 0.45,
        );
        assert!(
            excluded.is_none(),
            "an eps collar wider than the leg must prune it"
        );
    }

    #[test]
    fn rejects_a_crossed_band_through_the_shared_resolver() {
        // A crossed reservation (bid above ask) is rejected by resolve_band before
        // any offset is applied — slot 1 fails closed.
        assert!(
            compose_binary_legs(
                0.50, 0.70, 0.30, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS,
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_a_zero_edge_band() {
        // A collapsed reservation [fair, fair] with a zero floor: half-spread 0,
        // the two bids would sum to exactly 1 (zero edge) -> fail closed.
        assert!(compose_symmetric(0.50, 0.0, 0.0).is_none());
    }

    #[test]
    fn propagates_a_time_widening_failure_as_no_quote() {
        // A degenerate horizon (tau = 0) fails closed at slot 2.
        assert!(
            compose_binary_legs(
                0.50, 0.48, 0.52, ZERO_F64, UNIT_F64, ZERO_F64, REF_TAU, WIDEN_CAP, 0.0, TEST_EPS,
            )
            .is_none()
        );
    }

    #[test]
    fn rejects_a_degenerate_eps_collar() {
        // eps at/above 0.5 collapses the admissible interval — slot 5 fails closed.
        assert!(
            compose_binary_legs(
                0.50, 0.48, 0.52, ZERO_F64, UNIT_F64, REF_TAU, REF_TAU, WIDEN_CAP, 0.0, 0.5,
            )
            .is_none()
        );
    }
}
