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
    QUADRATIC_RISK_DIVISOR, ZERO_F64, clamp_probability, is_non_negative_finite,
    is_positive_finite, sanitize_non_negative,
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
    let target_scale = clamp_probability(
        inputs.expected_ev_per_notional
            / (QUADRATIC_RISK_DIVISOR * inputs.risk_lambda * inputs.ev_reference_per_notional),
    );
    (sanitize_non_negative(inputs.order_notional_target) * target_scale).min(cap)
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
}
