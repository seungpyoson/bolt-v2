//! Crate-internal decision-math helpers extracted from the binary-oracle taker
//! strategy (slice A1 of #522).

use crate::bolt_v3_market_families::OutcomeSide;
use crate::bolt_v3_numeric::{
    BPS_DENOMINATOR, POWER_OF_TWO, QUADRATIC_RISK_DIVISOR, UNIT_F64, ZERO_F64, clamp_probability,
    is_non_negative_finite, is_positive_finite, sanitize_non_negative, sanitize_probability,
};

pub(crate) fn price_agreement_corr(observed_price: f64, anchor_price: f64) -> Option<f64> {
    if !is_positive_finite(observed_price) || !is_positive_finite(anchor_price) {
        return None;
    }
    Some(clamp_probability(
        UNIT_F64 - ((observed_price - anchor_price).abs() / anchor_price),
    ))
}

pub(crate) fn price_gap_probability(observed_price: f64, reference_price: f64) -> Option<f64> {
    if !is_positive_finite(observed_price) || !is_positive_finite(reference_price) {
        return None;
    }
    Some(clamp_probability(
        (observed_price - reference_price).abs() / reference_price,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct UncertaintyBandInputs {
    pub(crate) lead_gap_probability: f64,
    pub(crate) jitter_penalty_probability: f64,
    pub(crate) time_uncertainty_probability: f64,
    pub(crate) fee_uncertainty_probability: f64,
}

pub(crate) fn uncertainty_band_probability(inputs: &UncertaintyBandInputs) -> Option<f64> {
    sanitize_probability(
        sanitize_probability(inputs.lead_gap_probability)?
            + sanitize_probability(inputs.jitter_penalty_probability)?
            + sanitize_probability(inputs.time_uncertainty_probability)?
            + sanitize_probability(inputs.fee_uncertainty_probability)?,
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ThetaScalerInputs {
    pub(crate) seconds_to_market_end: u64,
    pub(crate) cadence_seconds: u64,
    pub(crate) theta_decay_factor: f64,
}

pub(crate) fn compute_theta_scaler(inputs: &ThetaScalerInputs) -> Option<f64> {
    if !is_non_negative_finite(inputs.theta_decay_factor) {
        return None;
    }
    if inputs.theta_decay_factor == ZERO_F64 {
        return Some(UNIT_F64);
    }
    if inputs.cadence_seconds == 0 {
        return None;
    }

    let ratio =
        clamp_probability(inputs.seconds_to_market_end as f64 / inputs.cadence_seconds as f64);
    Some(UNIT_F64 + inputs.theta_decay_factor * (UNIT_F64 - ratio).powi(POWER_OF_TWO))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RobustSizingInputs {
    pub(crate) expected_ev_per_notional: f64,
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
    if inputs.risk_lambda == ZERO_F64 {
        return cap;
    }

    (inputs.expected_ev_per_notional / (QUADRATIC_RISK_DIVISOR * inputs.risk_lambda)).min(cap)
}

pub(crate) fn outcome_side_evidence_label(side: OutcomeSide) -> &'static str {
    match side {
        OutcomeSide::Up => "up",
        OutcomeSide::Down => "down",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WorstCaseEvInputs {
    pub(crate) fair_probability: Option<f64>,
    pub(crate) uncertainty_band_probability: f64,
    pub(crate) executable_entry_cost: f64,
    pub(crate) fee_bps: Option<f64>,
}

pub(crate) fn compute_worst_case_ev_bps(
    side: OutcomeSide,
    inputs: &WorstCaseEvInputs,
) -> Option<f64> {
    let fair_probability = sanitize_probability(inputs.fair_probability?)?;
    let uncertainty_band_probability = sanitize_probability(inputs.uncertainty_band_probability)?;
    let executable_entry_cost = inputs.executable_entry_cost;
    let fee_bps = inputs.fee_bps?;

    if !is_positive_finite(executable_entry_cost) {
        return None;
    }
    if !is_non_negative_finite(fee_bps) {
        return None;
    }

    let p_lo = clamp_probability(fair_probability - uncertainty_band_probability);
    let p_hi = clamp_probability(fair_probability + uncertainty_band_probability);
    let worst_case_success_probability = match side {
        OutcomeSide::Up => p_lo,
        OutcomeSide::Down => UNIT_F64 - p_hi,
    };
    let total_entry_cost = executable_entry_cost * (UNIT_F64 + fee_bps / BPS_DENOMINATOR);

    if total_entry_cost <= ZERO_F64 {
        return None;
    }

    Some(((worst_case_success_probability - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SideSelectionInputs {
    pub(crate) up_worst_ev_bps: Option<f64>,
    pub(crate) down_worst_ev_bps: Option<f64>,
    pub(crate) min_worst_case_ev_bps: f64,
}

pub(crate) fn choose_entry_side(inputs: &SideSelectionInputs) -> Option<OutcomeSide> {
    if !inputs.min_worst_case_ev_bps.is_finite() {
        return None;
    }

    let up_worst_ev_bps = inputs.up_worst_ev_bps.filter(|value| value.is_finite());
    let down_worst_ev_bps = inputs.down_worst_ev_bps.filter(|value| value.is_finite());
    let up_clears = up_worst_ev_bps.is_some_and(|value| value > inputs.min_worst_case_ev_bps);
    let down_clears = down_worst_ev_bps.is_some_and(|value| value > inputs.min_worst_case_ev_bps);

    match (up_clears, down_clears) {
        (true, false) => Some(OutcomeSide::Up),
        (false, true) => Some(OutcomeSide::Down),
        (true, true) => match (up_worst_ev_bps, down_worst_ev_bps) {
            (Some(up), Some(down)) if up > down => Some(OutcomeSide::Up),
            (Some(up), Some(down)) if down > up => Some(OutcomeSide::Down),
            _ => None,
        },
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theta_scaler_helper_increases_near_expiry_and_can_be_disabled() {
        let start = compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_market_end: 300,
            cadence_seconds: 300,
            theta_decay_factor: 1.5,
        })
        .expect("valid theta inputs should compute");
        let near_expiry = compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_market_end: 30,
            cadence_seconds: 300,
            theta_decay_factor: 1.5,
        })
        .expect("valid theta inputs should compute");

        assert!((start - 1.0).abs() < 1e-9);
        assert!(near_expiry > start);
        assert_eq!(
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_market_end: 30,
                cadence_seconds: 300,
                theta_decay_factor: 0.0,
            }),
            Some(1.0)
        );
        assert!(
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_market_end: 30,
                cadence_seconds: 0,
                theta_decay_factor: 1.5,
            })
            .is_none()
        );
    }

    #[test]
    fn task4_uncertainty_band_grows_with_jitter_and_time_to_resolution() {
        let narrow = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");
        let wider_from_jitter = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.004,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");
        let wider_from_time = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.005,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");

        assert!(wider_from_jitter > narrow);
        assert!(wider_from_time > narrow);
    }

    #[test]
    fn task4_uncertainty_band_grows_with_fee_uncertainty() {
        let narrow = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");
        let wide = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.02,
        })
        .expect("valid uncertainty inputs should produce a band");

        assert!(wide > narrow);
    }

    #[test]
    fn task4_uncertainty_band_fails_closed_on_invalid_component() {
        assert_eq!(
            uncertainty_band_probability(&UncertaintyBandInputs {
                lead_gap_probability: f64::NAN,
                jitter_penalty_probability: 0.002,
                time_uncertainty_probability: 0.003,
                fee_uncertainty_probability: 0.0,
            }),
            None
        );
        assert_eq!(
            uncertainty_band_probability(&UncertaintyBandInputs {
                lead_gap_probability: 1.2,
                jitter_penalty_probability: 0.002,
                time_uncertainty_probability: 0.003,
                fee_uncertainty_probability: 0.0,
            }),
            None
        );
        assert_eq!(
            uncertainty_band_probability(&UncertaintyBandInputs {
                lead_gap_probability: 0.40,
                jitter_penalty_probability: 0.30,
                time_uncertainty_probability: 0.20,
                fee_uncertainty_probability: 0.20,
            }),
            None
        );
    }

    #[test]
    fn task4_robust_sizing_shrinks_with_risk_and_respects_caps() {
        let low_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 2.0,
            risk_lambda: 0.1,
            order_notional_target: 100.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let high_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 2.0,
            risk_lambda: 2.0,
            order_notional_target: 100.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let capped = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 2.0,
            risk_lambda: 0.1,
            order_notional_target: 100.0,
            maximum_position_notional: 12.0,
            impact_cap_notional: 7.5,
        });

        assert!(high_risk < low_risk);
        assert_eq!(capped, 7.5);
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 0.0,
                risk_lambda: 0.1,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            0.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 2.0,
                risk_lambda: 0.0,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            100.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 2.0,
                risk_lambda: -0.1,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            0.0
        );
    }

    #[test]
    fn task4_worst_case_ev_uses_side_specific_bounds_and_fees_fail_closed() {
        let up_zero_fee = compute_worst_case_ev_bps(
            OutcomeSide::Up,
            &WorstCaseEvInputs {
                fair_probability: Some(0.60),
                uncertainty_band_probability: 0.05,
                executable_entry_cost: 0.50,
                fee_bps: Some(0.0),
            },
        )
        .expect("up zero-fee EV should be computable");
        let up_paid_fee = compute_worst_case_ev_bps(
            OutcomeSide::Up,
            &WorstCaseEvInputs {
                fair_probability: Some(0.60),
                uncertainty_band_probability: 0.05,
                executable_entry_cost: 0.50,
                fee_bps: Some(200.0),
            },
        )
        .expect("up paid-fee EV should be computable");
        let down_zero_fee = compute_worst_case_ev_bps(
            OutcomeSide::Down,
            &WorstCaseEvInputs {
                fair_probability: Some(0.60),
                uncertainty_band_probability: 0.05,
                executable_entry_cost: 0.50,
                fee_bps: Some(0.0),
            },
        )
        .expect("down zero-fee EV should be computable");

        assert!(up_paid_fee < up_zero_fee);
        assert!(up_zero_fee > down_zero_fee);
        assert_eq!(
            compute_worst_case_ev_bps(
                OutcomeSide::Up,
                &WorstCaseEvInputs {
                    fair_probability: Some(0.60),
                    uncertainty_band_probability: 0.05,
                    executable_entry_cost: 0.50,
                    fee_bps: None,
                },
            ),
            None
        );
        assert_eq!(
            compute_worst_case_ev_bps(
                OutcomeSide::Up,
                &WorstCaseEvInputs {
                    fair_probability: Some(1.2),
                    uncertainty_band_probability: 0.05,
                    executable_entry_cost: 0.50,
                    fee_bps: Some(0.0),
                },
            ),
            None
        );
        assert_eq!(
            compute_worst_case_ev_bps(
                OutcomeSide::Up,
                &WorstCaseEvInputs {
                    fair_probability: Some(0.60),
                    uncertainty_band_probability: 1.5,
                    executable_entry_cost: 0.50,
                    fee_bps: Some(0.0),
                },
            ),
            None
        );
    }

    #[test]
    fn task4_side_selection_picks_higher_worst_case_ev_when_both_clear_threshold() {
        let side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: Some(9.0),
            down_worst_ev_bps: Some(11.0),
            min_worst_case_ev_bps: 8.0,
        });

        assert_eq!(side, Some(OutcomeSide::Down));
    }

    #[test]
    fn task4_side_selection_requires_strictly_greater_than_threshold() {
        let side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: Some(8.0),
            down_worst_ev_bps: Some(7.0),
            min_worst_case_ev_bps: 8.0,
        });

        assert_eq!(side, None);
    }

    #[test]
    fn task4_side_selection_treats_missing_or_invalid_side_ev_as_not_selectable() {
        assert_eq!(
            choose_entry_side(&SideSelectionInputs {
                up_worst_ev_bps: Some(9.0),
                down_worst_ev_bps: None,
                min_worst_case_ev_bps: 8.0,
            }),
            Some(OutcomeSide::Up)
        );
        assert_eq!(
            choose_entry_side(&SideSelectionInputs {
                up_worst_ev_bps: Some(f64::NAN),
                down_worst_ev_bps: Some(9.0),
                min_worst_case_ev_bps: 8.0,
            }),
            Some(OutcomeSide::Down)
        );
        assert_eq!(
            choose_entry_side(&SideSelectionInputs {
                up_worst_ev_bps: Some(f64::NAN),
                down_worst_ev_bps: None,
                min_worst_case_ev_bps: 8.0,
            }),
            None
        );
    }

    #[test]
    fn task4_side_selection_fails_closed_on_equal_positive_evs() {
        let side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: Some(9.0),
            down_worst_ev_bps: Some(9.0),
            min_worst_case_ev_bps: 8.0,
        });

        assert_eq!(side, None);
    }
}
