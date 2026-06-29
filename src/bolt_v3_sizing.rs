//! Strategy-agnostic intent-sizing primitives shared by every bolt-v3
//! strategy archetype.
//!
//! `choose_robust_size` converts a strategy's dimensionless expected-EV
//! fraction into a dollar notional anchored at the operator's per-strategy
//! dollar target. It is the ONE shared "how much do I want" primitive: it is
//! not coupled to any archetype, venue, or instrument, and a new strategy
//! that sizes dollar notionals must consume it (or extend this module)
//! rather than re-implement sizing locally.
//!
//! Capital *enforcement* (reservation, admission, platform-wide caps) is a
//! separate concern that belongs to the submit path, not here; the only
//! caps applied in this module are the per-strategy inputs the caller
//! supplies.

use crate::bolt_v3_numeric::{
    ProbabilityValue, QUADRATIC_RISK_DIVISOR, ZERO_F64, bounded_probability_from_finite,
    is_non_negative_finite, is_positive_finite, sanitize_non_negative,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RobustSizingInputs {
    pub(crate) expected_ev_per_notional: f64,
    pub(crate) ev_reference_per_notional: f64,
    pub(crate) risk_lambda: f64,
    pub(crate) order_notional_target: f64,
    pub(crate) maximum_position_notional: f64,
    pub(crate) impact_cap_notional: f64,
}

pub(crate) fn choose_robust_size(inputs: &RobustSizingInputs) -> f64 {
    if !is_positive_finite(inputs.expected_ev_per_notional) {
        return ZERO_F64;
    }

    let cap = sanitize_non_negative(inputs.order_notional_target)
        .min(sanitize_non_negative(inputs.maximum_position_notional))
        .min(sanitize_non_negative(inputs.impact_cap_notional));
    if cap <= ZERO_F64 {
        return ZERO_F64;
    }

    if !is_non_negative_finite(inputs.risk_lambda) {
        return ZERO_F64;
    }
    // Every declared input is validated unconditionally, even on paths that
    // do not consume it: the zero-lambda escape hatch still requires a valid
    // EV reference so an invalid input can never select the cap.
    if !is_positive_finite(inputs.ev_reference_per_notional) {
        return ZERO_F64;
    }
    if inputs.risk_lambda == ZERO_F64 {
        return cap;
    }

    // Dimensional contract: the EV fraction only scales the operator's dollar
    // target; it is never itself a dollar amount. A signal whose worst-case EV
    // reaches 2λ × ev_reference saturates the scale at the full target.
    let Some(target_scale) = bounded_probability_from_finite(
        inputs.expected_ev_per_notional
            / (QUADRATIC_RISK_DIVISOR * inputs.risk_lambda * inputs.ev_reference_per_notional),
    )
    .map(ProbabilityValue::get) else {
        return ZERO_F64;
    };
    (sanitize_non_negative(inputs.order_notional_target) * target_scale).min(cap)
}

/// Maker order notional for one quote leg, sized off the protective half-spread
/// (the GM/CG edge proxy) rather than directional EV.
///
/// The GM/CG binary maker is directional-EV break-even, so it cannot consume
/// [`choose_robust_size`]: that primitive fails closed to ZERO on non-positive
/// EV (`expected_ev_per_notional`), which is exactly the maker's standing regime
/// — it would emit perpetual zero-size quotes (§16#13). This sibling instead
/// sizes on the half-spread the maker actually captures. It NEVER reads an EV
/// sign.
///
/// It is the half-spread analogue of `choose_robust_size`'s
/// `EV / ev_reference` scaling — same shape, same module, same dollar-anchored
/// contract: a strictly positive protective edge is required to quote at all
/// (a non-finite or non-positive `half_spread` sizes to ZERO, never the cap),
/// and the operator's per-order dollar target is scaled by the captured edge
/// relative to `reference_half_spread` (the widest protective half-spread, which
/// earns the full target), saturating once the edge reaches the reference and
/// clamped to the position-notional cap. Like `choose_robust_size` it fails
/// closed to [`ZERO_F64`] on any non-finite or negative input, so an invalid
/// input can never select a positive size.
pub(crate) fn maker_robust_size(
    half_spread: f64,
    reference_half_spread: f64,
    order_notional_target: f64,
    maximum_position_notional: f64,
) -> f64 {
    // No protective edge => no quote. Unlike the taker the maker cannot gate on
    // EV (it is break-even), so the half-spread it captures is the gate.
    if !is_positive_finite(half_spread) {
        return ZERO_F64;
    }
    // The reference edge anchors the scale; a non-positive/non-finite reference
    // can never define a valid scale, so it fails closed rather than dividing.
    if !is_positive_finite(reference_half_spread) {
        return ZERO_F64;
    }

    let cap = sanitize_non_negative(order_notional_target)
        .min(sanitize_non_negative(maximum_position_notional));
    if cap <= ZERO_F64 {
        return ZERO_F64;
    }

    // Dimensional contract mirrors choose_robust_size: the dimensionless edge
    // ratio only scales the operator's dollar target; it is never itself a
    // dollar amount. An edge at or beyond the reference saturates the scale at
    // the full target.
    let Some(edge_scale) = bounded_probability_from_finite(half_spread / reference_half_spread)
        .map(ProbabilityValue::get)
    else {
        return ZERO_F64;
    };
    (sanitize_non_negative(order_notional_target) * edge_scale).min(cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task4_robust_sizing_shrinks_with_risk_and_respects_caps() {
        let low_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.10,
            ev_reference_per_notional: 0.05,
            risk_lambda: 0.5,
            order_notional_target: 100.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let high_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.10,
            ev_reference_per_notional: 0.05,
            risk_lambda: 2.0,
            order_notional_target: 100.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let capped = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.10,
            ev_reference_per_notional: 0.05,
            risk_lambda: 0.5,
            order_notional_target: 100.0,
            maximum_position_notional: 12.0,
            impact_cap_notional: 7.5,
        });

        assert!(high_risk < low_risk);
        assert_eq!(capped, 7.5);
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 0.0,
                ev_reference_per_notional: 0.05,
                risk_lambda: 0.5,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            0.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 0.10,
                ev_reference_per_notional: 0.05,
                risk_lambda: 0.0,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            100.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 0.10,
                ev_reference_per_notional: 0.05,
                risk_lambda: -0.1,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            0.0
        );
    }

    #[test]
    fn robust_sizing_prices_barely_qualifying_signal_in_dollars_not_pennies() {
        // Regression for #618: a barely qualifying 150 bps worst-case-EV signal with
        // the deployed risk_lambda must produce a dollar-scaled order, not the raw
        // EV fraction reinterpreted as dollars ($0.015).
        let sized_notional = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.015,
            ev_reference_per_notional: 0.05,
            risk_lambda: 0.5,
            order_notional_target: 5.0,
            maximum_position_notional: 10.0,
            impact_cap_notional: 100.0,
        });

        assert!(
            (1.0..=5.0).contains(&sized_notional),
            "sized_notional {sized_notional} must land in [$1, $5]"
        );
        assert!((sized_notional - 0.015).abs() > 0.5);
    }

    #[test]
    fn robust_sizing_strong_signal_reaches_operator_target() {
        // Worked example from #618: entry 50c, worst-case p 0.60 -> ev ~= 1,700 bps.
        // A strong signal saturates the scale and lands exactly on the operator's
        // order_notional_target.
        let sized_notional = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.17,
            ev_reference_per_notional: 0.05,
            risk_lambda: 0.5,
            order_notional_target: 5.0,
            maximum_position_notional: 10.0,
            impact_cap_notional: 100.0,
        });

        assert_eq!(sized_notional, 5.0);
    }

    #[test]
    fn robust_sizing_is_dimensionally_anchored_to_the_dollar_target() {
        // Doubling the dollar anchor doubles the size while every dimensionless
        // input stays fixed; the EV fraction alone never equals dollars.
        let base = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.015,
            ev_reference_per_notional: 0.05,
            risk_lambda: 0.5,
            order_notional_target: 5.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let doubled_target = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 0.015,
            ev_reference_per_notional: 0.05,
            risk_lambda: 0.5,
            order_notional_target: 10.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });

        assert!((doubled_target - 2.0 * base).abs() < 1e-12);
        assert!((base - 0.015).abs() > 0.5);
    }

    #[test]
    fn robust_sizing_fails_closed_on_non_finite_risk_lambda() {
        // PR #623 review reproduction: a NaN/non-finite risk_lambda must size to
        // zero, never fall through the zero-lambda arm to the cap. The
        // is_non_negative_finite guard runs before the zero-lambda escape hatch.
        for risk_lambda in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                choose_robust_size(&RobustSizingInputs {
                    expected_ev_per_notional: 0.015,
                    ev_reference_per_notional: 0.05,
                    risk_lambda,
                    order_notional_target: 5.0,
                    maximum_position_notional: 10.0,
                    impact_cap_notional: 100.0,
                }),
                0.0
            );
        }
    }

    #[test]
    fn robust_sizing_fails_closed_on_invalid_ev_reference() {
        for ev_reference_per_notional in [0.0, -0.05, f64::NAN, f64::INFINITY] {
            assert_eq!(
                choose_robust_size(&RobustSizingInputs {
                    expected_ev_per_notional: 0.015,
                    ev_reference_per_notional,
                    risk_lambda: 0.5,
                    order_notional_target: 5.0,
                    maximum_position_notional: 10.0,
                    impact_cap_notional: 100.0,
                }),
                0.0
            );
        }
    }

    #[test]
    fn robust_sizing_fails_closed_on_invalid_ev_reference_even_with_zero_lambda() {
        // PR #623 review reproduction: every declared input is validated
        // unconditionally — the zero-lambda escape hatch must not skip the
        // ev_reference guard and return the cap on an invalid reference.
        for ev_reference_per_notional in [0.0, -0.05, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                choose_robust_size(&RobustSizingInputs {
                    expected_ev_per_notional: 0.015,
                    ev_reference_per_notional,
                    risk_lambda: 0.0,
                    order_notional_target: 5.0,
                    maximum_position_notional: 10.0,
                    impact_cap_notional: 100.0,
                }),
                0.0
            );
        }
    }

    #[test]
    fn robust_sizing_fails_closed_before_ev_reference_on_non_positive_ev() {
        // Guard-order pin: non-positive EV fails closed before the sizing path
        // observes otherwise invalid inputs.
        for expected_ev_per_notional in [0.0, -0.015] {
            assert_eq!(
                choose_robust_size(&RobustSizingInputs {
                    expected_ev_per_notional,
                    ev_reference_per_notional: f64::NAN,
                    risk_lambda: 0.0,
                    order_notional_target: 5.0,
                    maximum_position_notional: 10.0,
                    impact_cap_notional: 100.0,
                }),
                0.0
            );
        }
    }

    #[test]
    fn maker_robust_size_sizes_positively_in_the_break_even_regime() {
        // The headline §16#13 property: with a positive protective half-spread the
        // maker sizes a POSITIVE dollar order, even though it carries no positive
        // directional EV. `choose_robust_size` has no edge input and would return
        // ZERO across this whole regime; this primitive sizes off the edge alone,
        // so it is structurally impossible for an EV sign to zero it out (there is
        // no EV parameter to read).
        let size = maker_robust_size(0.05, 0.10, 5.0, 10.0);
        assert!(
            size > 0.0,
            "positive edge must yield a positive maker size, got {size}"
        );
        assert!(
            (size - 2.5).abs() < 1e-12,
            "0.05/0.10 scale on a $5 target is $2.50, got {size}"
        );
    }

    #[test]
    fn maker_robust_size_requires_a_strictly_positive_protective_edge() {
        // No-edge gate. The bounded probability conversion also rejects
        // non-finite ratios, so the non-finite rows are redundant fail-closed
        // coverage rather than the sole proof of this pre-gate.
        // The 0.0/-0.01/-inf rows are independently floored to zero by
        // the bounded conversion and pass with or without the gate; they are kept
        // as ordinary boundary coverage, not as gate proof.
        for half_spread in [0.0, -0.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                maker_robust_size(half_spread, 0.10, 5.0, 10.0),
                0.0,
                "half_spread={half_spread} has no protective edge and must size to zero"
            );
        }
    }

    #[test]
    fn maker_robust_size_grows_monotonically_with_the_captured_edge() {
        // Magnitude differential: the size must depend on the edge MAGNITUDE, not
        // merely be gated by its sign. A constant-cap variant (size independent of
        // `half_spread`) would return the same value for both and fail here.
        let thin = maker_robust_size(0.04, 0.10, 10.0, 100.0);
        let wide = maker_robust_size(0.08, 0.10, 10.0, 100.0);
        assert!(
            wide > thin,
            "a wider protective edge must size larger: thin={thin}, wide={wide}"
        );
        assert!(
            (thin - 4.0).abs() < 1e-12,
            "0.04/0.10 scale on $10 is $4, got {thin}"
        );
        assert!(
            (wide - 8.0).abs() < 1e-12,
            "0.08/0.10 scale on $10 is $8, got {wide}"
        );
    }

    #[test]
    fn maker_robust_size_saturates_at_the_reference_edge_and_respects_the_cap() {
        // An edge at or beyond the reference earns the full per-order target.
        assert_eq!(maker_robust_size(0.10, 0.10, 10.0, 100.0), 10.0);
        assert_eq!(maker_robust_size(0.50, 0.10, 10.0, 100.0), 10.0);
        // The position-notional cap clamps the saturated target.
        assert_eq!(maker_robust_size(0.50, 0.10, 100.0, 12.0), 12.0);
    }

    #[test]
    fn maker_robust_size_is_dimensionally_anchored_to_the_dollar_target() {
        // Doubling the dollar anchor doubles the size while the dimensionless edge
        // ratio stays fixed; the edge ratio alone is never dollars.
        let base = maker_robust_size(0.05, 0.10, 5.0, 100.0);
        let doubled = maker_robust_size(0.05, 0.10, 10.0, 100.0);
        assert!((doubled - 2.0 * base).abs() < 1e-12);
        assert!((base - 2.5).abs() < 1e-12);
    }

    #[test]
    fn maker_robust_size_fails_closed_on_invalid_reference_target_or_cap() {
        // A non-finite/non-positive reference edge cannot define a scale.
        for reference_half_spread in [0.0, -0.10, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                maker_robust_size(0.05, reference_half_spread, 5.0, 10.0),
                0.0
            );
        }
        // A non-finite/negative target or cap sanitizes to zero capacity => no size.
        for order_notional_target in [-1.0, f64::NAN, f64::NEG_INFINITY] {
            assert_eq!(
                maker_robust_size(0.05, 0.10, order_notional_target, 10.0),
                0.0
            );
        }
        for maximum_position_notional in [0.0, -1.0, f64::NAN, f64::NEG_INFINITY] {
            assert_eq!(
                maker_robust_size(0.05, 0.10, 5.0, maximum_position_notional),
                0.0
            );
        }
    }
}
