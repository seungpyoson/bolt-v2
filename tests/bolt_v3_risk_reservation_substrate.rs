use std::collections::{BTreeMap, BTreeSet};

use bolt_v2::bolt_v3_risk_reservation_substrate::{
    contracts::{
        ActiveDescriptorView, AdmissionCandidate, AdmissionToken, AdmittedOrder,
        ConfiguredLeaseAuthority, LeaseAuthorityBackend, ModelRiskEvaluationScope, PoolId,
        PoolOwnershipLease, PreparedPolicyEpoch, RiskAssessment, RiskPreviewInput, RiskSizingView,
        SafetyAction, SafetyPolicyEnvelope, SizingDecisionPermit,
    },
    risk_classifier::{
        ConcentrationBucket, ConcentrationBucketDimension, RiskClassificationError,
        RiskClassificationPolicy, RiskClassifier, RiskDescriptorCanonicalAttributes,
    },
    risk_kernel::{
        RiskCandidate, RiskEvaluationScope, RiskExposure, RiskKernel, RiskKernelError,
        RiskKernelInput, RiskPortfolioSnapshot,
    },
    state_owner::{
        DurableRiskMutation, FencedRiskStateStore, RiskMutationKind, RiskStateMutationError,
        RiskStateOwner,
    },
};
use rust_decimal::Decimal;

#[test]
fn sc_014_shared_store_rejects_paused_owner_after_pool_ownership_transfers() {
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        "risk-reservation-pool-leases",
    )
    .expect("lease authority dependency must be explicitly configured");
    let pool_id = PoolId::new("capital-pool-a").expect("pool id should be valid");
    let store = FencedRiskStateStore::new(lease_authority);

    let first_owner = RiskStateOwner::acquire(store.clone(), pool_id.clone(), "owner-a")
        .expect("first owner should acquire the pool");
    first_owner
        .reconcile_before_new_risk()
        .expect("first owner reconciles before risk");
    assert_eq!(
        first_owner
            .commit_durable_mutation(DurableRiskMutation::new(
                "reservation-a",
                RiskMutationKind::Reservation
            ))
            .expect("current owner should commit")
            .get(),
        1
    );

    let successor = RiskStateOwner::acquire(store.clone(), pool_id.clone(), "owner-b")
        .expect("successor should acquire a higher fencing token");
    assert_eq!(
        successor.commit_durable_mutation(DurableRiskMutation::new(
            "reservation-b-before-reconcile",
            RiskMutationKind::Reservation
        )),
        Err(RiskStateMutationError::ReconciliationRequired),
        "successor must reconcile durable intents and NT truth before new risk"
    );
    successor
        .reconcile_before_new_risk()
        .expect("successor reconciles before risk");
    assert_eq!(
        successor
            .commit_durable_mutation(DurableRiskMutation::new(
                "reservation-b",
                RiskMutationKind::Reservation
            ))
            .expect("successor should commit after reconciliation")
            .get(),
        2
    );

    assert_eq!(
        first_owner.commit_durable_mutation(DurableRiskMutation::new(
            "stale-reservation",
            RiskMutationKind::Reservation
        )),
        Err(RiskStateMutationError::StaleFencingToken),
        "the authoritative shared store must reject the paused former owner"
    );
    assert_eq!(
        first_owner.commit_durable_mutation(DurableRiskMutation::new(
            "stale-submission",
            RiskMutationKind::Submission
        )),
        Err(RiskStateMutationError::StaleFencingToken),
        "stale owners cannot submit after ownership transfer"
    );
    let recorded = store
        .durable_mutation_records()
        .expect("shared store records should be readable");
    assert_eq!(
        recorded.len(),
        2,
        "stale former owner must not mutate durable risk state"
    );
    assert_eq!(recorded[0].fencing_token.get(), 1);
    assert_eq!(recorded[1].fencing_token.get(), 2);
}

#[test]
fn shared_store_rejects_lease_from_unconfigured_authority() {
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        "risk-reservation-pool-leases",
    )
    .expect("lease authority dependency must be explicitly configured");
    let other_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        "risk-reservation-pool-leases-shadow",
    )
    .expect("alternate dependency name should parse");
    let pool_id = PoolId::new("capital-pool-b").expect("pool id should be valid");
    let store = FencedRiskStateStore::new(lease_authority);

    let owner = RiskStateOwner::acquire(store.clone(), pool_id.clone(), "owner-a")
        .expect("owner should acquire the pool");
    owner
        .reconcile_before_new_risk()
        .expect("owner reconciles before risk");

    let wrong_authority_lease = PoolOwnershipLease::new(
        pool_id,
        owner.lease().owner_id().clone(),
        owner.lease().fencing_token(),
        other_authority,
    );
    assert_eq!(
        store.commit_durable_mutation(
            &wrong_authority_lease,
            DurableRiskMutation::new("wrong-authority", RiskMutationKind::Reservation)
        ),
        Err(RiskStateMutationError::AmbiguousLeaseState),
        "the authoritative shared store must reject mutations from an unconfigured authority"
    );
}

#[test]
fn s0_contracts_are_public_single_source_for_sizing_imports() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}

    assert_send_sync::<AdmissionCandidate>();
    assert_send_sync::<RiskSizingView>();
    assert_send_sync::<ActiveDescriptorView>();
    assert_send_sync::<ModelRiskEvaluationScope>();
    assert_send_sync::<RiskPreviewInput>();
    assert_send_sync::<RiskAssessment>();
    assert_send_sync::<AdmissionToken>();
    assert_send_sync::<AdmittedOrder>();
    assert_send_sync::<SizingDecisionPermit>();
    assert_send_sync::<SafetyAction>();
    assert_send_sync::<PreparedPolicyEpoch>();
    assert_send_sync::<SafetyPolicyEnvelope>();
}

#[test]
fn s1_classifier_derives_complete_buckets_from_canonical_descriptor_not_caller_claims() {
    let policy = classification_policy([
        dimension("payoff_class", "descriptor_payoff_class"),
        dimension("settlement_group", "descriptor_settlement_group"),
    ]);
    let descriptor = RiskDescriptorCanonicalAttributes::new(BTreeMap::from([
        (
            "descriptor_payoff_class".to_string(),
            "class-alpha".to_string(),
        ),
        (
            "descriptor_settlement_group".to_string(),
            "group-alpha".to_string(),
        ),
    ]))
    .expect("descriptor attributes are canonical and complete");
    let caller_claims = vec![bucket("caller_only", "wrong")];

    let classification = RiskClassifier::classify(&descriptor, &policy, &caller_claims)
        .expect("complete canonical descriptor should classify");

    assert_eq!(
        classification.buckets(),
        &BTreeSet::from([
            bucket("payoff_class", "class-alpha"),
            bucket("settlement_group", "group-alpha")
        ])
    );
    assert_eq!(
        classification.diagnostic_caller_declared_buckets(),
        caller_claims.as_slice(),
        "caller-declared buckets are retained only as diagnostics"
    );
}

#[test]
fn s1_classifier_missing_canonical_attribute_fails_closed() {
    let policy = classification_policy([
        dimension("payoff_class", "descriptor_payoff_class"),
        dimension("settlement_group", "descriptor_settlement_group"),
    ]);
    let descriptor = RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
        "descriptor_payoff_class".to_string(),
        "class-alpha".to_string(),
    )]))
    .expect("one canonical attribute is intentionally absent");

    let error = RiskClassifier::classify(&descriptor, &policy, &[])
        .expect_err("missing canonical bucket attribute must fail closed");

    assert_eq!(
        error,
        RiskClassificationError::MissingCanonicalAttribute {
            attribute: "descriptor_settlement_group".to_string()
        }
    );
}

#[test]
fn s1_kernel_keeps_equity_stress_loss_distinct_from_governor_realized_loss() {
    let input = RiskKernelInput {
        risk_state_version:
            bolt_v2::bolt_v3_risk_reservation_substrate::contracts::RiskStateVersion::new(7),
        portfolio: RiskPortfolioSnapshot {
            positions: Vec::new(),
        },
        candidate: candidate(
            "candidate-instrument",
            [bucket("risk_class", "alpha")],
            6,
            10,
            0,
        ),
        evaluation_scope: RiskEvaluationScope::CandidateInstrument {
            instrument_id: "candidate-instrument".to_string(),
        },
        portfolio_scope_id: "portfolio-scope".to_string(),
    };

    let assessment = RiskKernel::evaluate(&input).expect("recognized instrument scope");

    assert_eq!(assessment.equity_floor_stress_loss, dec(6));
    assert_eq!(assessment.governor_realized_loss, dec(10));
    assert_ne!(
        assessment.equity_floor_stress_loss, assessment.governor_realized_loss,
        "the stress-loss metric and governor realized-loss metric must not collapse into one scalar"
    );
}

#[test]
fn s1_kernel_returns_current_and_post_candidate_stress_for_candidate_instrument_scope() {
    let input = scoped_stress_fixture(RiskEvaluationScope::CandidateInstrument {
        instrument_id: "candidate-instrument".to_string(),
    });

    let assessment =
        RiskKernel::evaluate(&input).expect("candidate instrument scope is recognized");

    assert_eq!(assessment.current_scope_equity_floor_stress_loss, dec(5));
    assert_eq!(
        assessment.post_candidate_scope_equity_floor_stress_loss,
        dec(8)
    );
}

#[test]
fn s1_kernel_returns_current_and_post_candidate_stress_for_bucket_scope() {
    let input = scoped_stress_fixture(RiskEvaluationScope::ConcentrationBucket(bucket(
        "risk_class",
        "alpha",
    )));

    let assessment = RiskKernel::evaluate(&input).expect("candidate bucket scope is recognized");

    assert_eq!(assessment.current_scope_equity_floor_stress_loss, dec(8));
    assert_eq!(
        assessment.post_candidate_scope_equity_floor_stress_loss,
        dec(11)
    );
}

#[test]
fn s1_kernel_returns_current_and_post_candidate_stress_for_portfolio_scope() {
    let input = scoped_stress_fixture(RiskEvaluationScope::Portfolio {
        scope_id: "portfolio-scope".to_string(),
    });

    let assessment = RiskKernel::evaluate(&input).expect("portfolio scope is recognized");

    assert_eq!(assessment.current_scope_equity_floor_stress_loss, dec(19));
    assert_eq!(
        assessment.post_candidate_scope_equity_floor_stress_loss,
        dec(22)
    );
}

#[test]
fn s1_kernel_unrecognized_scope_fails_closed() {
    let input = scoped_stress_fixture(RiskEvaluationScope::ConcentrationBucket(bucket(
        "risk_class",
        "unknown",
    )));

    let error = RiskKernel::evaluate(&input)
        .expect_err("unrecognized caller-declared scope must fail closed");

    assert_eq!(error, RiskKernelError::UnrecognizedEvaluationScope);
}

#[test]
fn s1_kernel_documents_bounded_io_free_complexity() {
    let source = include_str!("../src/bolt_v3_risk_reservation_substrate/risk_kernel.rs");

    assert!(source.contains("Worst-case complexity"));
    assert!(source.contains("O(P * (B + T))"));
    assert!(source.contains("no I/O"));
    assert!(!source.contains("std::fs"));
    assert!(!source.contains("std::net"));
}

fn scoped_stress_fixture(evaluation_scope: RiskEvaluationScope) -> RiskKernelInput {
    RiskKernelInput {
        risk_state_version:
            bolt_v2::bolt_v3_risk_reservation_substrate::contracts::RiskStateVersion::new(9),
        portfolio: RiskPortfolioSnapshot {
            positions: vec![
                exposure(
                    "candidate-instrument",
                    [bucket("risk_class", "alpha")],
                    7,
                    8,
                    2,
                ),
                exposure("other-alpha", [bucket("risk_class", "alpha")], 4, 5, 1),
                exposure("other-beta", [bucket("risk_class", "beta")], 13, 15, 2),
            ],
        },
        candidate: candidate(
            "candidate-instrument",
            [bucket("risk_class", "alpha")],
            4,
            6,
            1,
        ),
        evaluation_scope,
        portfolio_scope_id: "portfolio-scope".to_string(),
    }
}

fn classification_policy(
    dimensions: impl IntoIterator<Item = ConcentrationBucketDimension>,
) -> RiskClassificationPolicy {
    RiskClassificationPolicy::new(dimensions.into_iter().collect())
        .expect("classification policy should be valid")
}

fn dimension(bucket_class: &str, canonical_attribute: &str) -> ConcentrationBucketDimension {
    ConcentrationBucketDimension::new(bucket_class, canonical_attribute)
        .expect("bucket dimension should be valid")
}

fn bucket(bucket_class: &str, bucket_value: &str) -> ConcentrationBucket {
    ConcentrationBucket::new(bucket_class, bucket_value).expect("bucket should be valid")
}

fn candidate(
    instrument_id: &str,
    buckets: impl IntoIterator<Item = ConcentrationBucket>,
    conservative_liquidation_value: i64,
    governor_cost_basis: i64,
    worst_terminal_cash_flow: i64,
) -> RiskCandidate {
    RiskCandidate {
        instrument_id: instrument_id.to_string(),
        buckets: BTreeSet::from_iter(buckets),
        quantity: dec(1),
        conservative_liquidation_value: dec(conservative_liquidation_value),
        governor_cost_basis: dec(governor_cost_basis),
        terminal_cash_flows: vec![dec(worst_terminal_cash_flow), dec(99)],
    }
}

fn exposure(
    instrument_id: &str,
    buckets: impl IntoIterator<Item = ConcentrationBucket>,
    conservative_liquidation_value: i64,
    governor_cost_basis: i64,
    worst_terminal_cash_flow: i64,
) -> RiskExposure {
    RiskExposure {
        instrument_id: instrument_id.to_string(),
        buckets: BTreeSet::from_iter(buckets),
        quantity: dec(1),
        conservative_liquidation_value: dec(conservative_liquidation_value),
        governor_cost_basis: dec(governor_cost_basis),
        terminal_cash_flows: vec![dec(worst_terminal_cash_flow), dec(99)],
    }
}

fn dec(value: i64) -> Decimal {
    Decimal::new(value, 0)
}
