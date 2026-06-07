use bolt_v2::bolt_v3_iv::{
    error::IvRejectReason,
    policy::{
        IvFallbackCandidate, IvFallbackPolicy, IvInterpolationPolicy, IvPolicyError, IvPolicyInput,
        IvProjectionPolicy, IvQuorumPolicy, interpolate_smile, project_scalar, resolve_fallback,
        resolve_quorum,
    },
    provenance::IvPolicyDecision,
    store::IvSmilePoint,
    time::UnixNanos,
};

fn input(product_id: &str, value: f64, ts: u64) -> IvPolicyInput {
    IvPolicyInput {
        product_id: product_id.to_string(),
        value,
        ts_event_ns: UnixNanos::new(ts),
    }
}

#[test]
fn projection_policy_rejects_inputs_outside_configured_skew() {
    let policy = IvProjectionPolicy {
        policy_id: "configured-projection".to_string(),
        max_projection_input_skew_ns: 5,
    };

    assert_eq!(
        project_scalar(&policy, &[input("a", 0.2, 1_000), input("b", 0.3, 1_020)]),
        Err(IvPolicyError::Rejected {
            reason: IvRejectReason::ProjectionRejected,
            policy_id: "configured-projection".to_string(),
        })
    );
}

#[test]
fn interpolation_policy_records_decision_and_rejects_unconfigured_extrapolation() {
    let policy = IvInterpolationPolicy {
        policy_id: "configured-interpolation".to_string(),
        allow_extrapolation: false,
        minimum_points: 2,
    };

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
    assert!(
        output
            .policy_decisions
            .contains(&IvPolicyDecision::Interpolation)
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
fn fallback_and_quorum_policies_record_typed_decisions() {
    let fallback = resolve_fallback(
        &IvFallbackPolicy {
            policy_id: "configured-fallback".to_string(),
            ordered_candidate_ids: vec!["primary".to_string(), "backup".to_string()],
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
    assert!(
        fallback
            .policy_decisions
            .contains(&IvPolicyDecision::Fallback)
    );

    let quorum = resolve_quorum(
        &IvQuorumPolicy {
            policy_id: "configured-quorum".to_string(),
            minimum_sources: 2,
            agreement_band: 0.05,
        },
        &[
            input("source-a", 0.30, 1_000),
            input("source-b", 0.33, 1_000),
        ],
    )
    .unwrap();
    assert!(quorum.policy_decisions.contains(&IvPolicyDecision::Quorum));

    assert!(matches!(
        resolve_quorum(
            &IvQuorumPolicy {
                policy_id: "configured-quorum".to_string(),
                minimum_sources: 3,
                agreement_band: 0.05,
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
