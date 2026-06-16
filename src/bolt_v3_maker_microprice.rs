//! Book-imbalance / micro-price anchor for the binary-oracle maker (W3 — the
//! FR-022 "book-imbalance term", spec `specs/488-binary-oracle-maker/` line 160).
//!
//! The maker's fair value is the *oracle* prior — P(up) from the resolution
//! anchor — and that prior stays authoritative. This module computes a small,
//! bounded nudge from the venue's own top-of-book pressure so the maker leans
//! toward whichever side the book is heavier on, without ever letting the book
//! override the oracle. Three pure pieces:
//!
//! - [`book_imbalance`]: the normalised `(bid − ask)/(bid + ask)` size pressure
//!   in `[−1, 1]` (positive = bids heavier = buy-side pressure);
//! - [`micro_price`]: the size-weighted touch, which leans toward the heavier
//!   side of the book (more bid size pulls it up toward the ask, and vice versa);
//! - [`micro_price_anchor`]: a convex blend `(1−w)·oracle + w·micro` that nudges
//!   the oracle fair toward the micro-price by a config-sourced weight `w ∈
//!   [0, 1]`, and **falls back to the oracle fair** when the book is degenerate
//!   (no micro-price this tick). The oracle is the prior; the book only nudges.
//!
//! ## Input seam — top-of-book levels are fed in, not reconstructed here
//!
//! Some venue adapters stream L2 deltas rather than complete depth snapshots, so
//! the best-bid/best-ask price+size levels this module consumes are
//! reconstructed one layer up — the live shell maintains the order book from the
//! L2 delta stream and extracts the top of book before calling in. This pure
//! module deliberately knows nothing about that reconstruction: it takes the
//! already-extracted `best_bid`, `best_ask`, `bid_size`, `ask_size` scalars. The
//! L2 → top-of-book seam is the shell's job, NOT this module's.
//!
//! Pure: no NT type, no async, no I/O, no clock; every numeric literal (the unit
//! bounds and the zero/unit weight endpoints) comes from
//! [`crate::bolt_v3_numeric`].

use crate::bolt_v3_numeric::{
    UNIT_F64, ZERO_F64, is_non_negative_finite, is_positive_finite, sanitize_probability,
};

/// Normalised top-of-book size imbalance `(bid_size − ask_size)/(bid_size +
/// ask_size)`, in `[−1, 1]`.
///
/// Positive means the bid side is heavier (buy-side pressure), negative means
/// the ask side is heavier; the magnitude is the fraction of the touched size
/// that is one-sided. Bounded in `[−1, 1]` by construction: the numerator's
/// magnitude can never exceed the (non-negative) denominator.
///
/// Fail-closed (returns `None`, which the caller treats as "no book signal this
/// tick") when:
/// - either size is non-finite;
/// - either size is negative (a size is a non-negative quantity);
/// - both sizes are zero — an empty touch has no pressure to read, and the
///   `0/0` ratio is undefined.
pub fn book_imbalance(bid_size: f64, ask_size: f64) -> Option<f64> {
    if !is_non_negative_finite(bid_size) || !is_non_negative_finite(ask_size) {
        return None;
    }
    // Both sizes are now non-negative finite, so `total` is non-negative; the
    // only degenerate case left is an empty touch (both zero), where the ratio
    // would be 0/0. A single non-zero side is fine — it reads as full ±1 pressure.
    let total = bid_size + ask_size;
    if total <= ZERO_F64 {
        return None;
    }
    Some((bid_size - ask_size) / total)
}

/// Size-weighted touch (the "micro-price"): `(best_ask·bid_size +
/// best_bid·ask_size)/(bid_size + ask_size)`.
///
/// This leans toward the heavier side of the book: weighting the *ask* price by
/// the *bid* size means more resting bid size pulls the micro-price up toward the
/// ask, and more ask size pulls it down toward the bid — the standard
/// imbalance-weighted touch. The result always lies in `[best_bid, best_ask]`
/// (it is a convex combination of the two touch prices).
///
/// Fail-closed (returns `None`) when:
/// - any input is non-finite;
/// - either price is negative, or either size is negative;
/// - the book is crossed (`best_bid > best_ask`) — a crossed touch is stale or
///   corrupt, never a price to lean on;
/// - the total touched size is zero — no weights, so the weighted price is
///   undefined;
/// - the total touched size overflows to a non-finite value (absurd-but-finite
///   sizes summing past the float range) — a degenerate book, not a price to
///   quote.
///
/// When it returns `Some`, the result is guaranteed to lie in `[best_bid,
/// best_ask]`: the value is computed as an interpolation with a weight in
/// `[0, 1]`, so it is in-band by construction and benign float rounding is
/// snapped back into the band.
pub fn micro_price(best_bid: f64, best_ask: f64, bid_size: f64, ask_size: f64) -> Option<f64> {
    if !is_non_negative_finite(best_bid)
        || !is_non_negative_finite(best_ask)
        || !is_non_negative_finite(bid_size)
        || !is_non_negative_finite(ask_size)
    {
        return None;
    }
    // A crossed touch (bid above ask) is stale or corrupt market data.
    if best_bid > best_ask {
        return None;
    }
    // Sizes are now non-negative finite, so `total` is non-negative. A zero total
    // leaves the weighted price undefined (0/0); an absurd-but-finite pair whose
    // sum overflows past the float range is a degenerate book — fail closed.
    let total = bid_size + ask_size;
    if !is_positive_finite(total) {
        return None;
    }
    // Interpolate weight-first. The fraction of the way from the bid touch toward
    // the ask touch is `bid_size / total` — more resting bid size leans the price
    // toward the ask. The weight is in `[0, 1]` and well-conditioned because both
    // operands share scale. Computing it BEFORE touching prices avoids the
    // `price · size` products of the direct
    // `(best_ask·bid_size + best_bid·ask_size) / total` form, whose terms can
    // underflow (or overflow) asymmetrically and silently collapse the touch to a
    // band edge for extreme size magnitudes.
    let ask_weight = bid_size / total;
    let micro = best_bid + (best_ask - best_bid) * ask_weight;
    // `micro` lies in `[best_bid, best_ask]` by construction (weight in `[0, 1]`,
    // bounded by `best_ask`), so it is always finite. Float rounding in the
    // multiply/add can still land it up to ~1 ULP outside the band, so snap it back
    // to honor the documented postcondition exactly.
    Some(micro.clamp(best_bid, best_ask))
}

/// Convex blend of the oracle fair (the prior) and the book micro-price (the
/// nudge): `(1 − w)·oracle_fair + w·micro` for a weight `w ∈ [0, 1]`.
///
/// The oracle stays the PRIOR — at `w = 0` the anchor is exactly the oracle fair,
/// and the weight only ever pulls it a bounded fraction of the way toward the
/// book micro-price (exactly the micro-price at `w = 1`). The book only nudges;
/// it can never override the oracle.
///
/// `micro` is the [`micro_price`] result, passed as an `Option` so a degenerate
/// book this tick is handled here with a **graceful fallback to the oracle fair**
/// (no nudge applied) rather than dropping the quote — a missing book signal must
/// not stop the maker quoting off its prior.
///
/// Fail-closed (returns `None`) when:
/// - `oracle_fair` is non-finite;
/// - `w` is non-finite or lies outside `[0, 1]` (a misconfigured weight, not a
///   tick to silently re-normalise);
/// - the blended result is non-finite (extreme finite magnitudes overflowing the
///   blend) — defensive; unreachable for the probability-domain inputs this maker
///   actually feeds it.
///
/// When `micro` is `Some`, it is assumed already validated by [`micro_price`]
/// (finite); the blend re-checks finiteness defensively and falls back to the
/// oracle prior if a non-finite micro slips through.
pub fn micro_price_anchor(oracle_fair: f64, micro: Option<f64>, micro_weight: f64) -> Option<f64> {
    if !oracle_fair.is_finite() {
        return None;
    }
    let micro_weight = sanitize_probability(micro_weight)?;
    // Graceful fallback: a degenerate (or non-finite) book this tick means no
    // nudge — the maker quotes off its oracle prior unchanged.
    let micro = match micro {
        Some(m) if m.is_finite() => m,
        _ => return Some(oracle_fair),
    };
    // Endpoint exactness is guaranteed by construction, not by float luck:
    //   - w = 0: the blend below already returns the oracle exactly (`0·Δ = 0`),
    //     which is why it is written as `oracle + w·Δ` rather than
    //     `(1 − w)·oracle + w·micro` (the latter would drift at w = 0).
    //   - w = 1: `oracle + 1·(micro − oracle)` is NOT guaranteed to recover
    //     `micro` bit-exactly — the subtraction then re-addition can round (e.g.
    //     oracle = 0.03, micro = 0.30 lands on 0.30000000000000004) — so the
    //     w = 1 endpoint is short-circuited to the micro.
    // The interior blend `oracle + w·(micro − oracle)` is monotonic in w.
    if micro_weight == UNIT_F64 {
        return Some(micro);
    }
    let blended = oracle_fair + micro_weight * (micro - oracle_fair);
    // Defensive: with a finite oracle, a finite micro, and w ∈ [0, 1], the blend
    // is always finite for the probability inputs this maker uses. Guard it anyway
    // so arbitrary finite-but-extreme magnitudes (e.g. near ±f64::MAX) fail closed
    // rather than emit a non-finite quote.
    if !blended.is_finite() {
        return None;
    }
    Some(blended)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for closed-form float comparisons.
    const EPSILON: f64 = 1e-9;

    // ---- book_imbalance ----

    #[test]
    fn imbalance_is_zero_for_a_balanced_book() {
        assert!(book_imbalance(10.0, 10.0).unwrap().abs() < EPSILON);
    }

    #[test]
    fn imbalance_is_positive_when_bids_are_heavier() {
        // Heavier bids -> buy-side pressure -> positive imbalance.
        let imb = book_imbalance(30.0, 10.0).unwrap();
        assert!(imb > ZERO_F64);
        // (30 − 10)/(30 + 10) = 0.5.
        assert!((imb - 0.5).abs() < EPSILON);
    }

    #[test]
    fn imbalance_is_negative_when_asks_are_heavier() {
        assert!(book_imbalance(10.0, 30.0).unwrap() < ZERO_F64);
    }

    #[test]
    fn imbalance_saturates_to_plus_minus_one_on_a_one_sided_book() {
        // A single non-zero side reads as full ±1 pressure.
        assert!((book_imbalance(5.0, 0.0).unwrap() - UNIT_F64).abs() < EPSILON);
        assert!((book_imbalance(0.0, 5.0).unwrap() + UNIT_F64).abs() < EPSILON);
    }

    #[test]
    fn imbalance_is_bounded_in_unit_interval_property() {
        // Property: over a grid of non-negative sizes (not both zero) the
        // imbalance is always within [−1, 1].
        let sizes = [0.0, 0.001, 1.0, 7.5, 1_000.0, f64::MAX];
        for &b in &sizes {
            for &a in &sizes {
                if let Some(imb) = book_imbalance(b, a) {
                    assert!(
                        (-UNIT_F64..=UNIT_F64).contains(&imb),
                        "imbalance {imb} out of [−1, 1] for bid {b}, ask {a}"
                    );
                }
            }
        }
    }

    #[test]
    fn imbalance_fails_closed_on_degenerate_inputs() {
        // Both sizes zero -> 0/0 -> None.
        assert!(book_imbalance(0.0, 0.0).is_none());
        // Negative size is not a quantity.
        assert!(book_imbalance(-1.0, 10.0).is_none());
        assert!(book_imbalance(10.0, -1.0).is_none());
        // Non-finite sizes.
        assert!(book_imbalance(f64::NAN, 10.0).is_none());
        assert!(book_imbalance(10.0, f64::INFINITY).is_none());
    }

    // ---- micro_price ----

    #[test]
    fn micro_price_is_the_mid_for_a_balanced_book() {
        // Equal sizes -> the size-weighted touch is the simple mid.
        let micro = micro_price(0.40, 0.60, 10.0, 10.0).unwrap();
        assert!((micro - 0.50).abs() < EPSILON);
    }

    #[test]
    fn micro_price_leans_toward_the_heavier_side() {
        // Heavier bids pull the micro-price UP toward the ask; heavier asks pull
        // it DOWN toward the bid.
        let heavy_bids = micro_price(0.40, 0.60, 90.0, 10.0).unwrap();
        let heavy_asks = micro_price(0.40, 0.60, 10.0, 90.0).unwrap();
        assert!(heavy_bids > 0.50, "heavy bids lean micro toward the ask");
        assert!(heavy_asks < 0.50, "heavy asks lean micro toward the bid");
        // Symmetric mirror around the 0.50 mid.
        assert!(((heavy_bids - 0.50) + (heavy_asks - 0.50)).abs() < EPSILON);
    }

    #[test]
    fn micro_price_lies_between_bid_and_ask_property() {
        // Property: the size-weighted touch is a convex combination of the two
        // touch prices, so it always lies in [best_bid, best_ask].
        let best_bid = 0.42;
        let best_ask = 0.58;
        let sizes = [0.0, 0.5, 3.0, 25.0, 4_096.0];
        for &b in &sizes {
            for &a in &sizes {
                if let Some(micro) = micro_price(best_bid, best_ask, b, a) {
                    assert!(
                        best_bid - EPSILON <= micro && micro <= best_ask + EPSILON,
                        "micro {micro} outside [{best_bid}, {best_ask}] for bid {b}, ask {a}"
                    );
                }
            }
        }
    }

    #[test]
    fn micro_price_collapses_to_the_touch_when_one_side_is_empty() {
        // All weight on the bid size weights the ASK price -> micro = best_ask.
        assert!((micro_price(0.40, 0.60, 10.0, 0.0).unwrap() - 0.60).abs() < EPSILON);
        // All weight on the ask size weights the BID price -> micro = best_bid.
        assert!((micro_price(0.40, 0.60, 0.0, 10.0).unwrap() - 0.40).abs() < EPSILON);
    }

    #[test]
    fn micro_price_fails_closed_on_degenerate_inputs() {
        // Crossed touch (bid above ask).
        assert!(micro_price(0.60, 0.40, 10.0, 10.0).is_none());
        // Zero total size.
        assert!(micro_price(0.40, 0.60, 0.0, 0.0).is_none());
        // Negative price or size.
        assert!(micro_price(-0.40, 0.60, 10.0, 10.0).is_none());
        assert!(micro_price(0.40, 0.60, -10.0, 10.0).is_none());
        // Non-finite inputs.
        assert!(micro_price(f64::NAN, 0.60, 10.0, 10.0).is_none());
        assert!(micro_price(0.40, 0.60, 10.0, f64::INFINITY).is_none());
    }

    #[test]
    fn micro_price_returns_the_locked_touch_when_book_is_locked() {
        // A locked book (best_bid == best_ask) is not crossed; the weighted touch
        // collapses to that single price for any valid sizes.
        let micro = micro_price(0.50, 0.50, 10.0, 5.0).expect("locked book is valid");
        assert!((micro - 0.50).abs() < EPSILON);
    }

    #[test]
    fn micro_price_fails_closed_on_size_overflow() {
        // Absurd-but-finite sizes whose SUM overflows to a non-finite total leave
        // the weights undefined — fail closed. This is the only overflow path: the
        // weight-first interpolation forms no `price · size` product, so the touch
        // math itself never overflows.
        assert!(micro_price(0.40, 0.60, f64::MAX, f64::MAX).is_none());
    }

    #[test]
    fn micro_price_quotes_a_locked_book_at_the_largest_finite_price() {
        // A locked book at the largest finite price is valid, and the weight-first
        // form returns that price exactly. The direct
        // `(best_ask·bid_size + best_bid·ask_size)/total` form overflows its
        // numerator here (best_ask · bid_size = MAX · MAX = inf) and would
        // spuriously fail closed; the robust form does not.
        assert_eq!(
            micro_price(f64::MAX, f64::MAX, f64::MAX, 1.0),
            Some(f64::MAX)
        );
    }

    #[test]
    fn micro_price_clamps_benign_float_rounding_into_the_band() {
        // The weighted-sum quotient can round ~1 ULP outside [best_bid, best_ask]
        // for a locked book or an extreme size ratio on an inexact price; the
        // result must still be Some and honor the documented band postcondition.
        // Locked book (0.03/0.03): the raw quotient lands 1 ULP below 0.03.
        assert_eq!(micro_price(0.03, 0.03, 1.0, 10.0), Some(0.03));
        // Extreme size ratio on an inexact price: raw quotient lands 1 ULP above.
        let m = micro_price(0.0, 0.1, 1e200, 0.0).expect("finite, valid book");
        assert!((0.0..=0.1).contains(&m), "micro {m} must lie in [0.0, 0.1]");
    }

    #[test]
    fn micro_price_succeeds_for_large_but_finite_sizes() {
        // Large-but-finite sizes that do NOT overflow must still quote — the
        // overflow guard is not overzealous. Result is Some and in-band.
        let m = micro_price(0.40, 0.60, 1e200, 1e200).expect("large finite sizes are valid");
        assert!(
            (0.40..=0.60).contains(&m),
            "micro {m} must lie in [0.40, 0.60]"
        );
    }

    #[test]
    fn micro_price_stays_accurate_at_extreme_size_magnitudes() {
        // Equal sizes => the micro-price is the exact midpoint of the touch,
        // independent of the size magnitude. The naive
        // `(best_ask·bid_size + best_bid·ask_size)/total` form loses this at
        // subnormal sizes: each `price·size` product underflows to zero, so the
        // quotient collapses to a band edge and the clamp would silently mask the
        // gross error (in-band but wrong). The weight-first interpolation computes
        // `bid_size/total` (well-conditioned in [0, 1]) before touching prices, so
        // equal sizes still give the midpoint. Smallest positive subnormal sizes:
        let tiny = f64::from_bits(1);
        let m = micro_price(1e-300, 0.5, tiny, tiny).expect("finite, valid book");
        let midpoint = (1e-300 + 0.5) / 2.0; // == 0.25 to full f64 precision
        assert!(
            (m - midpoint).abs() < 1e-9,
            "equal sizes must give the midpoint {midpoint}, got {m}"
        );
    }

    // ---- micro_price_anchor ----

    #[test]
    fn anchor_is_exactly_the_oracle_when_weight_is_zero() {
        let oracle = 0.55;
        let anchored = micro_price_anchor(oracle, Some(0.61), 0.0).unwrap();
        assert_eq!(anchored, oracle, "w = 0 must return the oracle exactly");
    }

    #[test]
    fn anchor_is_exactly_the_micro_when_weight_is_one() {
        let micro = 0.61;
        let anchored = micro_price_anchor(0.55, Some(micro), 1.0).unwrap();
        assert_eq!(anchored, micro, "w = 1 must return the micro exactly");

        // Decimal pairs where the raw blend `oracle + 1·(micro − oracle)` drifts
        // by a ULP (here 0.03 + (0.30 − 0.03) = 0.30000000000000004) must STILL
        // return the micro bit-exactly — the w = 1 endpoint is short-circuited,
        // not left to float round-trip luck.
        let drifty = micro_price_anchor(0.03, Some(0.30), 1.0).unwrap();
        assert_eq!(
            drifty, 0.30,
            "w = 1 must return the micro exactly even when the raw blend drifts"
        );
    }

    #[test]
    fn anchor_is_the_convex_blend_for_an_interior_weight() {
        // (1 − 0.25)·0.40 + 0.25·0.80 = 0.30 + 0.20 = 0.50.
        let anchored = micro_price_anchor(0.40, Some(0.80), 0.25).unwrap();
        assert!((anchored - 0.50).abs() < EPSILON);
    }

    #[test]
    fn anchor_is_monotonic_in_weight_property() {
        // Property: with micro above the oracle, the anchor rises monotonically as
        // w sweeps 0 -> 1, staying within [oracle, micro] throughout.
        let oracle = 0.30;
        let micro = 0.70;
        let weights = [
            0.0,
            0.1,
            0.2,
            0.4,
            0.5,
            0.6,
            0.8,
            0.9,
            0.999_999_999_999_999_9,
            1.0,
        ];
        let mut prev = micro_price_anchor(oracle, Some(micro), weights[0]).unwrap();
        assert!((prev - oracle).abs() < EPSILON);
        for &w in &weights[1..] {
            let cur = micro_price_anchor(oracle, Some(micro), w).unwrap();
            assert!(cur >= prev - EPSILON, "anchor must rise with w (w = {w})");
            assert!(
                oracle - EPSILON <= cur && cur <= micro + EPSILON,
                "anchor {cur} outside [oracle, micro] at w = {w}"
            );
            prev = cur;
        }
    }

    #[test]
    fn anchor_is_monotonic_decreasing_when_micro_below_oracle() {
        // Mirror of the rising property: with the micro BELOW the oracle, the
        // anchor falls monotonically as w sweeps 0 -> 1, staying within
        // [micro, oracle] throughout.
        let oracle = 0.70;
        let micro = 0.30;
        let weights = [
            0.0,
            0.1,
            0.2,
            0.4,
            0.5,
            0.6,
            0.8,
            0.9,
            0.999_999_999_999_999_9,
            1.0,
        ];
        let mut prev = micro_price_anchor(oracle, Some(micro), weights[0]).unwrap();
        assert!((prev - oracle).abs() < EPSILON);
        for &w in &weights[1..] {
            let cur = micro_price_anchor(oracle, Some(micro), w).unwrap();
            assert!(cur <= prev + EPSILON, "anchor must fall with w (w = {w})");
            assert!(
                micro - EPSILON <= cur && cur <= oracle + EPSILON,
                "anchor {cur} outside [micro, oracle] at w = {w}"
            );
            prev = cur;
        }
    }

    #[test]
    fn anchor_falls_back_to_the_oracle_when_the_book_is_degenerate() {
        // A degenerate book yields micro = None; the anchor must still quote off
        // the oracle prior unchanged, for any valid weight.
        let oracle = 0.62;
        assert_eq!(micro_price_anchor(oracle, None, 0.5), Some(oracle));
        assert_eq!(micro_price_anchor(oracle, None, 1.0), Some(oracle));
        assert_eq!(micro_price_anchor(oracle, None, 0.0), Some(oracle));
    }

    #[test]
    fn anchor_falls_back_to_the_oracle_on_a_non_finite_micro() {
        // Defense-in-depth: a non-finite micro that slips past micro_price still
        // falls back to the oracle prior rather than poisoning the blend.
        let oracle = 0.48;
        assert_eq!(
            micro_price_anchor(oracle, Some(f64::NAN), 0.5),
            Some(oracle)
        );
        assert_eq!(
            micro_price_anchor(oracle, Some(f64::INFINITY), 0.5),
            Some(oracle)
        );
    }

    #[test]
    fn anchor_fails_closed_on_a_bad_oracle_or_weight() {
        // The prior itself unusable -> no quote.
        assert!(micro_price_anchor(f64::NAN, Some(0.5), 0.5).is_none());
        assert!(micro_price_anchor(f64::INFINITY, Some(0.5), 0.5).is_none());
        // Weight outside [0, 1] is a misconfiguration, not a tick to renormalise.
        assert!(micro_price_anchor(0.50, Some(0.5), -0.01).is_none());
        assert!(micro_price_anchor(0.50, Some(0.5), 1.01).is_none());
        assert!(micro_price_anchor(0.50, Some(0.5), f64::NAN).is_none());
        // A non-finite oracle fails closed even when the book is degenerate.
        assert!(micro_price_anchor(f64::NAN, None, 0.5).is_none());
    }

    #[test]
    fn anchor_fails_closed_on_a_non_finite_blend() {
        // Oracle and micro are each finite and the weight is in range, but their
        // extreme magnitudes overflow the blend to non-finite -> fail closed rather
        // than emit a non-finite quote.
        assert!(micro_price_anchor(f64::MAX, Some(f64::MIN), 0.5).is_none());
    }

    #[test]
    fn micro_price_feeds_the_anchor_end_to_end() {
        // The book signal flows through both functions: a heavier-bid book lifts
        // the micro above the oracle, and a positive weight nudges the anchor up
        // toward it (but never past it).
        let oracle = 0.50;
        let micro = micro_price(0.48, 0.52, 80.0, 20.0).expect("valid touch");
        assert!(micro > oracle, "heavier bids lift the micro above the mid");
        let anchored = micro_price_anchor(oracle, Some(micro), 0.30).unwrap();
        assert!(
            oracle < anchored && anchored < micro,
            "the book nudges the prior"
        );
    }
}
