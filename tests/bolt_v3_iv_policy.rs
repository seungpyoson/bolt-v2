use bolt_v2::bolt_v3_iv::{
    error::IvRejectReason,
    policy::{
        IvBasisSelection, IvEvidenceMapping, IvExtrapolationPolicy, IvFallbackCandidate,
        IvFallbackPolicy, IvInterpolationMethod, IvInterpolationPolicy, IvPolicyError,
        IvPolicyInput, IvProjectionKind, IvProjectionPolicy, IvQuorumPolicy, IvQuorumTieBreak,
        IvStrikeSelection, IvTenorSelection, interpolate_smile, project_scalar, resolve_fallback,
        resolve_quorum,
    },
    provenance::{IvPolicyDecision, IvRejectedCandidate},
    store::IvSmilePoint,
    time::UnixNanos,
};

fn input(product_id: &str, value: f64, ts: u64) -> IvPolicyInput {
    IvPolicyInput {
        product_id: product_id.to_string(),
        source_id: product_id.to_string(),
        selector_fingerprint: format!("{product_id}-selector"),
        basis: "mark".to_string(),
        convention: "configured-convention".to_string(),
        value,
        ts_event_ns: UnixNanos::new(ts),
    }
}

fn projection_policy(
    minimum_points: usize,
    max_projection_input_skew_ns: u64,
) -> IvProjectionPolicy {
    IvProjectionPolicy {
        policy_id: "configured-projection".to_string(),
        projection_kind: IvProjectionKind::Mean,
        basis_selection: IvBasisSelection::PreserveInputBasis,
        source_eligibility: Vec::new(),
        strike_selection: IvStrikeSelection::AllConfiguredStrikes,
        tenor_selection: IvTenorSelection::AllConfiguredTenors,
        evidence_mapping: IvEvidenceMapping::PreserveEvidenceKind,
        minimum_points,
        max_projection_input_skew_ns,
        fallback_policy_ref: None,
        interpolation_policy_ref: None,
        quorum_policy_ref: None,
    }
}

fn interpolation_policy(extrapolation: IvExtrapolationPolicy) -> IvInterpolationPolicy {
    IvInterpolationPolicy {
        policy_id: "configured-interpolation".to_string(),
        method: IvInterpolationMethod::Linear,
        strike_axis: "strike".to_string(),
        tenor_axis: "expiry".to_string(),
        minimum_points: 2,
        eligible_sources: vec!["configured-source".to_string()],
        extrapolation,
    }
}

#[test]
fn projection_policy_rejects_inputs_outside_configured_skew() {
    let policy = projection_policy(1, 5);

    assert_eq!(
        project_scalar(&policy, &[input("a", 0.2, 1_000), input("b", 0.3, 1_020)]),
        Err(IvPolicyError::Rejected {
            reason: IvRejectReason::ProjectionRejected,
            policy_id: "configured-projection".to_string(),
        })
    );
}

#[test]
fn projection_policy_rejects_when_minimum_input_count_is_not_met() {
    let policy = projection_policy(2, 5);

    assert_eq!(
        project_scalar(&policy, &[input("a", 0.2, 1_000)]),
        Err(IvPolicyError::Rejected {
            reason: IvRejectReason::ProjectionRejected,
            policy_id: "configured-projection".to_string(),
        })
    );
}

#[test]
fn interpolation_policy_records_decision_and_rejects_unconfigured_extrapolation() {
    let policy = interpolation_policy(IvExtrapolationPolicy::Reject);

    let output = interpolate_smile(
        &policy,
        &[
            IvSmilePoint {
                strike: 90.0,
                iv: 0.20,
            },
            IvSmilePoint {
                strike: 100.0,
                iv: 0.30,
            },
        ],
        95.0,
    )
    .unwrap();
    assert_eq!(output.value, 0.25);
    assert_eq!(
        output.policy_decisions,
        vec![IvPolicyDecision::InterpolationDecision {
            policy_id: "configured-interpolation".to_string(),
            input_point_ids: vec!["90".to_string(), "100".to_string()],
            strike_axis: "strike".to_string(),
            tenor_axis: "expiry".to_string(),
            method: "linear".to_string(),
            minimum_points: 2,
            extrapolation: "reject".to_string(),
            eligible_sources: vec!["configured-source".to_string()],
            accepted_range: Some("90..100".to_string()),
            rejected_range: None,
        }]
    );

    assert!(matches!(
        interpolate_smile(
            &policy,
            &[
                IvSmilePoint {
                    strike: 90.0,
                    iv: 0.20
                },
                IvSmilePoint {
                    strike: 100.0,
                    iv: 0.30
                },
            ],
            110.0,
        ),
        Err(IvPolicyError::Rejected {
            reason: IvRejectReason::ExtrapolationRejected,
            ..
        })
    ));
}

#[test]
fn quorum_policy_requires_distinct_source_ids() {
    assert!(matches!(
        resolve_quorum(
            &IvQuorumPolicy {
                policy_id: "configured-quorum".to_string(),
                minimum_sources: 2,
                eligible_sources: vec!["source-a".to_string()],
                agreement_band: 0.05,
                tie_break: IvQuorumTieBreak::Mean,
            },
            &[
                input("source-a", 0.30, 1_000),
                input("source-a", 0.31, 1_000)
            ],
        ),
        Err(IvPolicyError::Rejected {
            reason: IvRejectReason::QuorumNotMet,
            ..
        })
    ));
}

#[test]
fn fallback_and_quorum_policies_record_typed_decisions() {
    let fallback = resolve_fallback(
        &IvFallbackPolicy {
            policy_id: "configured-fallback".to_string(),
            candidate_order: vec!["primary".to_string(), "backup".to_string()],
            eligible_sources: vec!["configured-source".to_string()],
            maximum_timestamp_skew_ns: 10,
            required_provenance_fields: vec!["raw_event_id".to_string()],
        },
        &[
            IvFallbackCandidate {
                candidate_id: "backup".to_string(),
                value: 0.31,
                eligible: true,
            },
            IvFallbackCandidate {
                candidate_id: "primary".to_string(),
                value: 0.30,
                eligible: false,
            },
        ],
    )
    .unwrap();
    assert_eq!(fallback.value, 0.31);
    assert_eq!(
        fallback.policy_decisions,
        vec![IvPolicyDecision::FallbackDecision {
            policy_id: "configured-fallback".to_string(),
            candidate_order: vec!["primary".to_string(), "backup".to_string()],
            accepted_candidate: Some("backup".to_string()),
            rejected_candidates: vec![IvRejectedCandidate {
                candidate_id: "primary".to_string(),
                reason: "candidate_not_eligible".to_string(),
            }],
            maximum_timestamp_skew_ns: 10,
            eligible_sources: vec!["configured-source".to_string()],
            required_provenance_fields: vec!["raw_event_id".to_string()],
        }]
    );

    let quorum = resolve_quorum(
        &IvQuorumPolicy {
            policy_id: "configured-quorum".to_string(),
            minimum_sources: 2,
            eligible_sources: vec!["source-a".to_string(), "source-b".to_string()],
            agreement_band: 0.05,
            tie_break: IvQuorumTieBreak::Mean,
        },
        &[
            input("source-a", 0.30, 1_000),
            input("source-b", 0.33, 1_000),
        ],
    )
    .unwrap();
    assert_eq!(
        quorum.policy_decisions,
        vec![IvPolicyDecision::QuorumDecision {
            policy_id: "configured-quorum".to_string(),
            participating_sources: vec!["source-a".to_string(), "source-b".to_string()],
            rejected_sources: Vec::new(),
            agreement_band: 0.05,
            tie_break: "mean".to_string(),
            quorum_met: true,
        }]
    );

    assert!(matches!(
        resolve_quorum(
            &IvQuorumPolicy {
                policy_id: "configured-quorum".to_string(),
                minimum_sources: 3,
                eligible_sources: vec!["source-a".to_string(), "source-b".to_string()],
                agreement_band: 0.05,
                tie_break: IvQuorumTieBreak::Mean,
            },
            &[
                input("source-a", 0.30, 1_000),
                input("source-b", 0.33, 1_000)
            ],
        ),
        Err(IvPolicyError::Rejected {
            reason: IvRejectReason::QuorumNotMet,
            ..
        })
    ));
}
