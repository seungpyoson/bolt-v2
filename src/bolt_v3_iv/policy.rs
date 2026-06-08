use serde::{Deserialize, Serialize};

use super::{
    error::IvRejectReason, provenance::IvPolicyDecision, store::IvSmilePoint, time::UnixNanos,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvPolicyInput {
    pub product_id: String,
    pub value: f64,
    pub ts_event_ns: UnixNanos,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvPolicyOutput {
    pub value: f64,
    pub policy_decisions: Vec<IvPolicyDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvPolicyError {
    Rejected {
        reason: IvRejectReason,
        policy_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IvProjectionKind {
    Mean,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvProjectionPolicy {
    pub policy_id: String,
    pub projection_kind: IvProjectionKind,
    pub minimum_points: usize,
    pub max_projection_input_skew_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvInterpolationPolicy {
    pub policy_id: String,
    pub allow_extrapolation: bool,
    pub minimum_points: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvFallbackPolicy {
    pub policy_id: String,
    pub ordered_candidate_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IvFallbackCandidate {
    pub candidate_id: String,
    pub value: f64,
    pub eligible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvQuorumPolicy {
    pub policy_id: String,
    pub minimum_sources: usize,
    pub agreement_band: f64,
}

pub fn project_scalar(
    policy: &IvProjectionPolicy,
    inputs: &[IvPolicyInput],
) -> Result<IvPolicyOutput, IvPolicyError> {
    if inputs.is_empty()
        || inputs.len() < policy.minimum_points
        || input_skew(inputs) > policy.max_projection_input_skew_ns
    {
        return Err(rejected(
            policy.policy_id.clone(),
            IvRejectReason::ProjectionRejected,
        ));
    }

    match policy.projection_kind {
        IvProjectionKind::Mean => Ok(IvPolicyOutput {
            value: average(inputs.iter().map(|input| input.value)).ok_or_else(|| {
                rejected(policy.policy_id.clone(), IvRejectReason::ProjectionRejected)
            })?,
            policy_decisions: vec![IvPolicyDecision::ProjectionDecision {
                policy_id: policy.policy_id.clone(),
                input_product_ids: inputs
                    .iter()
                    .map(|input| input.product_id.clone())
                    .collect(),
                projection_kind: "mean".to_string(),
                max_projection_input_skew_ns: policy.max_projection_input_skew_ns,
                accepted_input_ids: inputs
                    .iter()
                    .map(|input| input.product_id.clone())
                    .collect(),
                rejected_input_ids: Vec::new(),
            }],
        }),
    }
}

pub fn interpolate_smile(
    policy: &IvInterpolationPolicy,
    points: &[IvSmilePoint],
    strike: f64,
) -> Result<IvPolicyOutput, IvPolicyError> {
    if points.is_empty() || points.len() < policy.minimum_points {
        return Err(rejected(
            policy.policy_id.clone(),
            IvRejectReason::InterpolationRejected,
        ));
    }

    let mut points = points.to_vec();
    points.sort_by(|left, right| left.strike.total_cmp(&right.strike));
    let first = points.first().expect("minimum_points checked");
    let last = points.last().expect("minimum_points checked");

    if strike < first.strike || strike > last.strike {
        if !policy.allow_extrapolation {
            return Err(rejected(
                policy.policy_id.clone(),
                IvRejectReason::ExtrapolationRejected,
            ));
        }

        return Ok(IvPolicyOutput {
            value: if strike < first.strike {
                first.iv
            } else {
                last.iv
            },
            policy_decisions: vec![IvPolicyDecision::InterpolationDecision {
                policy_id: policy.policy_id.clone(),
                input_point_ids: points.iter().map(strike_id).collect(),
                method: "nearest_extrapolation".to_string(),
                minimum_points: policy.minimum_points,
                allow_extrapolation: policy.allow_extrapolation,
                accepted_range: Some(format!("{}..{}", first.strike, last.strike)),
                rejected_range: None,
            }],
        });
    }

    for window in points.windows(2) {
        let left = window[0];
        let right = window[1];
        if strike >= left.strike && strike <= right.strike {
            let width = right.strike - left.strike;
            let weight = if width == 0.0 {
                0.0
            } else {
                (strike - left.strike) / width
            };
            return Ok(IvPolicyOutput {
                value: left.iv + ((right.iv - left.iv) * weight),
                policy_decisions: vec![IvPolicyDecision::InterpolationDecision {
                    policy_id: policy.policy_id.clone(),
                    input_point_ids: vec![strike_id(&left), strike_id(&right)],
                    method: "linear".to_string(),
                    minimum_points: policy.minimum_points,
                    allow_extrapolation: policy.allow_extrapolation,
                    accepted_range: Some(format!("{}..{}", left.strike, right.strike)),
                    rejected_range: None,
                }],
            });
        }
    }

    Err(rejected(
        policy.policy_id.clone(),
        IvRejectReason::InterpolationRejected,
    ))
}

pub fn resolve_fallback(
    policy: &IvFallbackPolicy,
    candidates: &[IvFallbackCandidate],
) -> Result<IvPolicyOutput, IvPolicyError> {
    for candidate_id in &policy.ordered_candidate_ids {
        if let Some(candidate) = candidates
            .iter()
            .find(|candidate| candidate.eligible && candidate.candidate_id == *candidate_id)
        {
            return Ok(IvPolicyOutput {
                value: candidate.value,
                policy_decisions: vec![IvPolicyDecision::FallbackDecision {
                    policy_id: policy.policy_id.clone(),
                    candidate_order: policy.ordered_candidate_ids.clone(),
                    accepted_candidate: Some(candidate.candidate_id.clone()),
                    rejected_candidates: policy
                        .ordered_candidate_ids
                        .iter()
                        .take_while(|ordered_id| *ordered_id != candidate_id)
                        .cloned()
                        .collect(),
                }],
            });
        }
    }

    Err(rejected(
        policy.policy_id.clone(),
        IvRejectReason::FallbackRejected,
    ))
}

pub fn resolve_quorum(
    policy: &IvQuorumPolicy,
    inputs: &[IvPolicyInput],
) -> Result<IvPolicyOutput, IvPolicyError> {
    if inputs.is_empty() || inputs.len() < policy.minimum_sources {
        return Err(rejected(
            policy.policy_id.clone(),
            IvRejectReason::QuorumNotMet,
        ));
    }

    let min = inputs
        .iter()
        .map(|input| input.value)
        .fold(f64::INFINITY, f64::min);
    let max = inputs
        .iter()
        .map(|input| input.value)
        .fold(f64::NEG_INFINITY, f64::max);

    if max - min > policy.agreement_band {
        return Err(rejected(
            policy.policy_id.clone(),
            IvRejectReason::QuorumNotMet,
        ));
    }

    Ok(IvPolicyOutput {
        value: average(inputs.iter().map(|input| input.value))
            .ok_or_else(|| rejected(policy.policy_id.clone(), IvRejectReason::QuorumNotMet))?,
        policy_decisions: vec![IvPolicyDecision::QuorumDecision {
            policy_id: policy.policy_id.clone(),
            participating_sources: inputs
                .iter()
                .map(|input| input.product_id.clone())
                .collect(),
            rejected_sources: Vec::new(),
            agreement_band: policy.agreement_band,
            quorum_met: true,
        }],
    })
}

fn input_skew(inputs: &[IvPolicyInput]) -> u64 {
    match (
        inputs.iter().map(|input| input.ts_event_ns.get()).min(),
        inputs.iter().map(|input| input.ts_event_ns.get()).max(),
    ) {
        (Some(min_ts), Some(max_ts)) => max_ts.saturating_sub(min_ts),
        _ => u64::MIN,
    }
}

fn average(values: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0_u64;

    for value in values {
        sum += value;
        count += 1;
    }

    (count > 0).then(|| sum / count as f64)
}

fn strike_id(point: &IvSmilePoint) -> String {
    point.strike.to_string()
}

fn rejected(policy_id: String, reason: IvRejectReason) -> IvPolicyError {
    IvPolicyError::Rejected { reason, policy_id }
}
