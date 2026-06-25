use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Barrier},
    thread,
};

use bolt_v2::bolt_v3_risk_reservation_substrate::{
    admission_service::{
        AdmissionReserveError, AdmissionService, BoundReusableSafetyState, CallerRiskDiagnostics,
        RiskCapDimension, RiskReservationWorkDimension, SafetyActionAdmissionError,
        SafetyActionAdmissionRequest, SafetyActionMetric, SafetyActionProofDomain,
    },
    contracts::{
        ActiveDescriptorView, AdmissionCandidate, AdmissionToken, AdmittedOrder,
        ConfiguredLeaseAuthority, LeaseAuthorityBackend, ModelRiskEvaluationScope, PoolId,
        PoolOwnershipLease, PreparedPolicyEpoch, RiskAssessment, RiskPreviewInput,
        RiskReservationOfferedLoadEnvelope, RiskReservationOfferedLoadEnvelopeError,
        RiskReservationSubstrateConfig, RiskReservationWorkBounds, RiskSizingView,
        RiskStateVersion, SafetyAction, SafetyPolicyEnvelope, SizingDecisionPermit,
    },
    instrument_risk_registry::{
        CertifiedActiveDescriptor, DescriptorActivationStatus, DescriptorCertificationDecision,
        DescriptorCertificationEvidence, DescriptorCoverageAttestation,
        DescriptorRegistryAdmissionError, DescriptorRegistryError, DescriptorTerminalStateEnvelope,
        InstrumentRiskDescriptor, InstrumentRiskRegistry, TerminalStateObservation,
    },
    lifecycle_reconciler::{LifecycleReconciler, NtExecutionTruth, NtOrderStatusReportTruth},
    risk_classifier::{
        ConcentrationBucket, ConcentrationBucketDimension, RiskClassificationError,
        RiskClassificationPolicy, RiskClassifier, RiskDescriptorCanonicalAttributes,
    },
    risk_kernel::{
        RiskCandidate, RiskEvaluationScope, RiskExposure, RiskExposureSetInput, RiskKernel,
        RiskKernelError, RiskKernelInput, RiskPortfolioSnapshot,
    },
    risk_view_publisher::{RiskViewPublicationInput, RiskViewPublisher},
    state_owner::{
        DurableRiskMutation, FencedRiskStateStore, PolicyEpochAlertReason, RiskMutationKind,
        RiskStateMutationError, RiskStateOwner,
    },
    submission_authority::{
        LiveSubmitBoundary, LiveSubmitReceipt, SubmissionAuthority, SubmissionAuthorityError,
    },
};
use nautilus_model::identifiers::ClientOrderId;
use rust_decimal::Decimal;

#[test]
fn s5_reduce_only_safety_action_is_admitted_while_kill_switch_and_governor_freeze_new_risk() {
    let (service, _owner, _store) = reconciled_risk_context("pool-s5-reduce", "owner-s5-reduce");
    let safety_bucket = bucket("risk_class", "safety");
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s5-reduce",
        "affected-instrument",
        safety_bucket.clone(),
        dec(100),
        dec(100),
        vec![exposure(
            "affected-instrument",
            [safety_bucket.clone()],
            12,
            14,
            2,
        )],
    );
    let frozen = BoundReusableSafetyState {
        risk_state_version: RiskStateVersion::zero(),
        kill_switch_latched: true,
        loss_governor_halted: true,
    };
    let request = reduce_only_safety_action_request(
        "safety-action-reduce",
        "position-reducible",
        RiskStateVersion::zero(),
        frozen,
        vec![exposure(
            "affected-instrument",
            [bucket("risk_class", "safety")],
            6,
            7,
            2,
        )],
        4,
    );

    let admission = service
        .admit_safety_action(&view, request)
        .expect("recomputed reduce-only proof should bypass the new-risk freeze");

    assert_eq!(admission.action_id, "safety-action-reduce");
    assert_eq!(admission.risk_state_version.get(), 1);
    assert_eq!(admission.before.equity_floor_stress_loss, dec(10));
    assert_eq!(admission.after.equity_floor_stress_loss, dec(4));
    assert_eq!(admission.before.governor_realized_loss, dec(12));
    assert_eq!(admission.after.governor_realized_loss, dec(5));
    assert_eq!(admission.proof_domain.before_exposure_count, 1);
    assert_eq!(admission.proof_domain.after_exposure_count, 1);
}

#[test]
fn s5_reduce_only_safety_action_is_admitted_before_new_risk_reconciliation() {
    let (service, _owner, _store) =
        unreconciled_risk_context("pool-s5-unreconciled", "owner-s5-unreconciled");
    let safety_bucket = bucket("risk_class", "safety");
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s5-unreconciled",
        "unreconciled-instrument",
        safety_bucket.clone(),
        dec(100),
        dec(100),
        vec![exposure(
            "affected-instrument",
            [safety_bucket.clone()],
            12,
            14,
            2,
        )],
    );
    let frozen_new_risk = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-unreconciled",
                "idempotency-unreconciled",
                "pool-s5-unreconciled",
                "unreconciled-instrument",
                RiskStateVersion::zero(),
                dec(1),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect_err("new-risk admission must remain frozen until reconciliation completes");
    assert!(matches!(
        frozen_new_risk,
        AdmissionReserveError::StateMutation(RiskStateMutationError::ReconciliationRequired)
    ));

    let request = reduce_only_safety_action_request(
        "safety-action-unreconciled",
        "position-unreconciled",
        RiskStateVersion::zero(),
        unlatched_safety(RiskStateVersion::zero()),
        vec![exposure(
            "affected-instrument",
            [bucket("risk_class", "safety")],
            6,
            7,
            2,
        )],
        4,
    );

    let admission = service
        .admit_safety_action(&view, request)
        .expect("recomputed reduce-only proof should bypass not-yet-reconciled new-risk freeze");

    assert_eq!(admission.risk_state_version.get(), 1);
}

#[test]
fn s5_fabricated_new_after_exposure_is_rejected_even_when_standalone_metrics_are_low() {
    let (service, _owner, _store) =
        reconciled_risk_context("pool-s5-fabricated", "owner-s5-fabricated");
    let safety_bucket = bucket("risk_class", "safety");
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s5-fabricated",
        "affected-instrument",
        safety_bucket.clone(),
        dec(100),
        dec(100),
        vec![exposure(
            "affected-instrument",
            [safety_bucket.clone()],
            12,
            14,
            2,
        )],
    );
    let frozen = BoundReusableSafetyState {
        risk_state_version: RiskStateVersion::zero(),
        kill_switch_latched: true,
        loss_governor_halted: true,
    };
    let request = reduce_only_safety_action_request(
        "safety-action-fabricated",
        "position-fabricated",
        RiskStateVersion::zero(),
        frozen,
        vec![exposure(
            "fabricated-instrument",
            [bucket("risk_class", "safety")],
            1,
            1,
            1,
        )],
        4,
    );

    let error = service
        .admit_safety_action(&view, request)
        .expect_err("a caller-fabricated after exposure must fail closed even with low metrics");

    assert_eq!(
        error,
        SafetyActionAdmissionError::AfterExposureNotReduction {
            new_exposure_count: 1,
            increased_exposure_count: 0,
            increased_metrics: BTreeSet::new(),
            before_equity_floor_stress_loss: dec(10),
            after_equity_floor_stress_loss: dec(0),
            before_governor_realized_loss: dec(12),
            after_governor_realized_loss: dec(0),
        }
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should remain readable")
            .is_empty(),
        "rejected SafetyActions must not mint a risk reservation"
    );
}

#[test]
fn s5_asymmetric_safety_action_increase_reports_exact_increased_metric() {
    let (service, _owner, _store) =
        reconciled_risk_context("pool-s5-disguised", "owner-s5-disguised");
    let safety_bucket = bucket("risk_class", "safety");
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s5-disguised",
        "affected-instrument",
        safety_bucket.clone(),
        dec(100),
        dec(100),
        vec![exposure(
            "affected-instrument",
            [safety_bucket.clone()],
            10,
            20,
            5,
        )],
    );
    let frozen = BoundReusableSafetyState {
        risk_state_version: RiskStateVersion::zero(),
        kill_switch_latched: true,
        loss_governor_halted: true,
    };
    let request = reduce_only_safety_action_request(
        "safety-action-disguised",
        "position-disguised",
        RiskStateVersion::zero(),
        frozen,
        vec![exposure(
            "affected-instrument",
            [bucket("risk_class", "safety")],
            9,
            21,
            5,
        )],
        4,
    );

    let error = service
        .admit_safety_action(&view, request)
        .expect_err("a one-metric increase must still fail closed");

    assert_eq!(
        error,
        SafetyActionAdmissionError::AfterExposureNotReduction {
            new_exposure_count: 0,
            increased_exposure_count: 1,
            increased_metrics: BTreeSet::from([SafetyActionMetric::GovernorRealizedLoss]),
            before_equity_floor_stress_loss: dec(5),
            after_equity_floor_stress_loss: dec(4),
            before_governor_realized_loss: dec(15),
            after_governor_realized_loss: dec(16),
        }
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should remain readable")
            .is_empty(),
        "rejected SafetyActions must not mint a risk reservation"
    );
}

#[test]
fn s5_safety_action_reduction_proof_fails_closed_when_exposure_domain_exceeds_bound() {
    let (service, _owner, _store) = reconciled_risk_context("pool-s5-bound", "owner-s5-bound");
    let safety_bucket = bucket("risk_class", "safety");
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s5-bound",
        "affected-a",
        safety_bucket.clone(),
        dec(100),
        dec(100),
        vec![
            exposure("affected-a", [safety_bucket.clone()], 8, 9, 2),
            exposure("affected-b", [safety_bucket], 7, 8, 2),
        ],
    );
    let request = reduce_only_safety_action_request(
        "safety-action-bound",
        "position-bound",
        RiskStateVersion::zero(),
        unlatched_safety(RiskStateVersion::zero()),
        vec![exposure(
            "affected-a",
            [bucket("risk_class", "safety")],
            4,
            5,
            2,
        )],
        1,
    );

    assert_eq!(
        service.admit_safety_action(&view, request),
        Err(SafetyActionAdmissionError::ProofDomainExceeded {
            max_exposure_count: 1,
            before_exposure_count: 2,
            after_exposure_count: 1,
        }),
        "SafetyAction proof must reject rather than scan beyond the configured bounded domain"
    );
}

#[test]
fn s5_safety_action_operation_set_is_closed_and_not_open_ended() {
    let contracts = include_str!("../src/bolt_v3_risk_reservation_substrate/contracts.rs");

    assert!(
        contracts.contains("pub enum SafetyAction")
            && contracts.contains("CancelExistingOrder")
            && contracts.contains("ReduceOnlyCloseExistingPosition"),
        "SafetyAction must remain the sealed substrate operation alphabet"
    );
    assert!(
        !contracts.contains("VenueRequiredAdministrative"),
        "SafetyAction must not expose an arbitrary administrative action escape hatch"
    );
}

#[test]
fn sc_014_shared_store_rejects_paused_owner_after_pool_ownership_transfers() {
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        "risk-reservation-pool-leases",
    )
    .expect("lease authority dependency must be explicitly configured");
    let pool_id = PoolId::new("capital-pool-a").expect("pool id should be valid");
    let store = FencedRiskStateStore::new(substrate_config(lease_authority, roomy_work_bounds()));

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
    let store = FencedRiskStateStore::new(substrate_config(lease_authority, roomy_work_bounds()));

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

#[test]
fn s7a_compare_and_reserve_fails_closed_when_current_positions_exceed_configured_bound() {
    let bounds = configured_work_bounds(1, 1, 2);
    let (service, owner, _store) = reconciled_risk_context_with_work_bounds(
        "pool-s7a-positions",
        "owner-s7a-positions",
        bounds,
    );
    let risk_bucket = bucket("risk_class", "alpha");
    let actual_count = bounds.max_current_position_count() + 1;
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s7a-positions",
        "candidate-instrument",
        risk_bucket.clone(),
        dec(100),
        dec(100),
        exposures_with_count(actual_count, risk_bucket),
    );
    let before_version = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable before reserve")
        .risk_state_version;

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7a-positions",
                "idempotency-s7a-positions",
                "pool-s7a-positions",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect_err("over-bound current position set must fail closed before reservation");

    assert_eq!(
        error,
        AdmissionReserveError::WorkBoundExceeded {
            dimension: RiskReservationWorkDimension::CurrentPositionCount,
            max_count: bounds.max_current_position_count(),
            actual_count,
        }
    );
    assert_no_reservation_effect(&service, &owner, before_version);
}

#[test]
fn s7a_compare_and_reserve_fails_closed_when_candidate_bucket_count_exceeds_configured_bound() {
    let bounds = configured_work_bounds(1, 1, 2);
    let (service, owner, _store) =
        reconciled_risk_context_with_work_bounds("pool-s7a-buckets", "owner-s7a-buckets", bounds);
    let buckets = vec![
        bucket("risk_class_alpha", "alpha"),
        bucket("risk_class_beta", "beta"),
    ];
    let actual_count = buckets.len();
    let view = published_view_with_classification(
        RiskStateVersion::zero(),
        "pool-s7a-buckets",
        "candidate-instrument",
        buckets,
        dec(100),
        dec(100),
        vec![dec(0), dec(99)],
        Vec::new(),
    );
    let before_version = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable before reserve")
        .risk_state_version;

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7a-buckets",
                "idempotency-s7a-buckets",
                "pool-s7a-buckets",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect_err("over-bound candidate bucket set must fail closed before reservation");

    assert_eq!(
        error,
        AdmissionReserveError::WorkBoundExceeded {
            dimension: RiskReservationWorkDimension::CandidateBucketCount,
            max_count: bounds.max_buckets_per_exposure(),
            actual_count,
        }
    );
    assert_no_reservation_effect(&service, &owner, before_version);
}

#[test]
fn s7a_compare_and_reserve_fails_closed_when_candidate_scenario_count_exceeds_configured_bound() {
    let bounds = configured_work_bounds(1, 1, 2);
    let (service, owner, _store) = reconciled_risk_context_with_work_bounds(
        "pool-s7a-scenarios",
        "owner-s7a-scenarios",
        bounds,
    );
    let actual_count = bounds.max_terminal_cash_flow_count_per_exposure() + 1;
    let view = published_view_with_classification(
        RiskStateVersion::zero(),
        "pool-s7a-scenarios",
        "candidate-instrument",
        vec![bucket("risk_class", "alpha")],
        dec(100),
        dec(100),
        terminal_cash_flows_with_count(actual_count),
        Vec::new(),
    );
    let before_version = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable before reserve")
        .risk_state_version;

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7a-scenarios",
                "idempotency-s7a-scenarios",
                "pool-s7a-scenarios",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect_err("over-bound candidate scenario set must fail closed before reservation");

    assert_eq!(
        error,
        AdmissionReserveError::WorkBoundExceeded {
            dimension: RiskReservationWorkDimension::CandidateTerminalCashFlowCount,
            max_count: bounds.max_terminal_cash_flow_count_per_exposure(),
            actual_count,
        }
    );
    assert_no_reservation_effect(&service, &owner, before_version);
}

#[test]
fn s7a_compare_and_reserve_accepts_within_configured_work_bounds() {
    let bounds = configured_work_bounds(1, 1, 2);
    let (service, _owner, _store) =
        reconciled_risk_context_with_work_bounds("pool-s7a-positive", "owner-s7a-positive", bounds);
    let risk_bucket = bucket("risk_class", "alpha");
    let view = published_view_with_positions(
        RiskStateVersion::zero(),
        "pool-s7a-positive",
        "candidate-instrument",
        risk_bucket.clone(),
        dec(100),
        dec(100),
        exposures_with_count(bounds.max_current_position_count(), risk_bucket),
    );

    let admission = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7a-positive",
                "idempotency-s7a-positive",
                "pool-s7a-positive",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("within-bound candidate should reserve normally");

    assert_eq!(admission.admission_token.risk_state_version.get(), 1);
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .len(),
        1
    );
}

#[test]
fn s7a_compare_and_reserve_documents_bounded_critical_section_complexity() {
    let source = include_str!("../src/bolt_v3_risk_reservation_substrate/state_owner.rs");

    assert!(source.contains("Worst-case complexity"));
    assert!(source.contains("O(P * (B + T) + B + log R)"));
    assert!(source.contains("no external I/O"));
    assert!(source.contains("no nested mutable lock"));
    assert!(source.contains("pre-resolved immutable descriptor/policy/fee/classifier data"));
    assert!(source.contains("configured maximum"));
    assert!(!source.contains("std::fs"));
    assert!(!source.contains("std::net"));
}

#[test]
fn s7b_within_offered_load_envelope_admission_lifecycle_and_safety_action_succeed() {
    let envelope = RiskReservationOfferedLoadEnvelope::new(2)
        .expect("configured offered-load envelope should be valid");
    let (service, owner, _store) = unreconciled_risk_context_with_offered_load_envelope(
        "pool-s7b-within",
        "owner-s7b-within",
        envelope,
    );
    owner
        .reconcile_before_new_risk()
        .expect("lifecycle reconciliation should bypass offered-load shedding");
    let bucket = bucket("risk_class", "within");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-s7b-within",
        "candidate-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
    );

    let admission = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7b-within",
                "idempotency-s7b-within",
                "pool-s7b-within",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("risk-increasing admission inside the envelope should reserve");
    assert_eq!(admission.admission_token.risk_state_version.get(), 1);

    let safety_view = published_view_with_positions(
        RiskStateVersion::new(1),
        "pool-s7b-within",
        "affected-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
        vec![exposure("affected-instrument", [bucket.clone()], 12, 14, 2)],
    );
    let safety = service
        .admit_safety_action(
            &safety_view,
            reduce_only_safety_action_request(
                "safety-action-s7b-within",
                "position-s7b-within",
                RiskStateVersion::new(1),
                unlatched_safety(RiskStateVersion::new(1)),
                vec![exposure(
                    "affected-instrument",
                    [bucket("risk_class", "within")],
                    6,
                    7,
                    2,
                )],
                4,
            ),
        )
        .expect("SafetyAction should bypass offered-load shedding inside the envelope");
    assert_eq!(safety.source_risk_state_version, RiskStateVersion::new(1));
    assert_eq!(safety.risk_state_version, RiskStateVersion::new(2));
}

#[test]
fn s7b_above_offered_load_envelope_sheds_alerts_and_reserves_nothing() {
    let envelope = RiskReservationOfferedLoadEnvelope::new(1)
        .expect("configured offered-load envelope should be valid");
    let (service, owner, _store) = reconciled_risk_context_with_offered_load_envelope(
        "pool-s7b-shed",
        "owner-s7b-shed",
        envelope,
    );
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-s7b-shed",
        "candidate-instrument",
        bucket("risk_class", "shed"),
        dec(100),
        dec(100),
    );
    service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7b-first",
                "idempotency-s7b-first",
                "pool-s7b-shed",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("first admission should fill the one-admission envelope");
    let before_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable before shed");
    assert_eq!(before_snapshot.risk_state_version, RiskStateVersion::new(1));
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable before shed")
            .len(),
        1
    );
    let over_envelope_view = published_view(
        RiskStateVersion::new(1),
        "pool-s7b-shed",
        "candidate-instrument",
        bucket("risk_class", "shed"),
        dec(100),
        dec(100),
    );

    let error = service
        .compare_and_reserve(
            &over_envelope_view,
            admission_candidate(
                "intent-s7b-shed",
                "idempotency-s7b-shed",
                "pool-s7b-shed",
                "candidate-instrument",
                RiskStateVersion::new(1),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::new(1)),
            None,
            1_001,
        )
        .expect_err("next risk-increasing admission above the envelope must shed");

    assert_eq!(
        error,
        AdmissionReserveError::AdmissionShed {
            max_supported_in_flight_risk_increasing_admissions: 1,
            offered_in_flight_risk_increasing_admissions: 1,
        }
    );
    let after_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable after shed");
    assert_eq!(
        after_snapshot.risk_state_version, before_snapshot.risk_state_version,
        "shed admission must not advance the risk state version"
    );
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable after shed")
            .len(),
        1,
        "shed admission must not write a reservation record"
    );
    assert_eq!(
        after_snapshot.alerts.last().map(|alert| alert.reason),
        Some(PolicyEpochAlertReason::AdmissionShed),
        "shed admission must record the operational alert through the policy alert source"
    );
}

#[test]
fn s7b_lifecycle_and_safety_action_bypass_shed_gate_while_admission_load_is_over_envelope() {
    let envelope = RiskReservationOfferedLoadEnvelope::new(1)
        .expect("configured offered-load envelope should be valid");
    let (service, owner, store) = reconciled_risk_context_with_offered_load_envelope(
        "pool-s7b-priority",
        "owner-s7b-priority-a",
        envelope,
    );
    let bucket = bucket("risk_class", "priority");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-s7b-priority",
        "candidate-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
    );
    service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s7b-priority-first",
                "idempotency-s7b-priority-first",
                "pool-s7b-priority",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("first admission should fill the one-admission envelope");
    assert_eq!(
        service
            .compare_and_reserve(
                &published_view(
                    RiskStateVersion::new(1),
                    "pool-s7b-priority",
                    "candidate-instrument",
                    bucket.clone(),
                    dec(100),
                    dec(100),
                ),
                admission_candidate(
                    "intent-s7b-priority-shed",
                    "idempotency-s7b-priority-shed",
                    "pool-s7b-priority",
                    "candidate-instrument",
                    RiskStateVersion::new(1),
                    dec(4),
                ),
                unlatched_safety(RiskStateVersion::new(1)),
                None,
                1_001,
            )
            .expect_err("test setup should place risk-increasing admissions over the envelope"),
        AdmissionReserveError::AdmissionShed {
            max_supported_in_flight_risk_increasing_admissions: 1,
            offered_in_flight_risk_increasing_admissions: 1,
        }
    );

    let successor = RiskStateOwner::acquire(
        store.clone(),
        PoolId::new("pool-s7b-priority").expect("pool id should be valid"),
        "owner-s7b-priority-b",
    )
    .expect("successor owner should acquire the pool for lifecycle reconciliation");
    let lifecycle_version = successor
        .reconcile_before_new_risk()
        .expect("lifecycle reconciliation should bypass the shed gate while over envelope");
    assert_eq!(lifecycle_version, RiskStateVersion::new(1));

    let priority_service = AdmissionService::new(successor.clone());
    let safety_view = published_view_with_positions(
        lifecycle_version,
        "pool-s7b-priority",
        "affected-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
        vec![exposure("affected-instrument", [bucket.clone()], 12, 14, 2)],
    );
    let safety = priority_service
        .admit_safety_action(
            &safety_view,
            reduce_only_safety_action_request(
                "safety-action-s7b-priority",
                "position-s7b-priority",
                lifecycle_version,
                unlatched_safety(lifecycle_version),
                vec![exposure(
                    "affected-instrument",
                    [bucket("risk_class", "priority")],
                    6,
                    7,
                    2,
                )],
                4,
            ),
        )
        .expect("SafetyAction should bypass the shed gate while admissions are over envelope");
    assert_eq!(safety.source_risk_state_version, lifecycle_version);
    assert_eq!(safety.risk_state_version, RiskStateVersion::new(2));

    assert_eq!(
        owner
            .commit_durable_mutation(DurableRiskMutation::new(
                "stale-owner-after-priority-reconcile",
                RiskMutationKind::Reservation,
            ))
            .expect_err("old owner should remain fenced after lifecycle handoff"),
        RiskStateMutationError::StaleFencingToken
    );
}

#[test]
fn s7b_zero_offered_load_envelope_fails_closed_at_construction() {
    assert_eq!(
        RiskReservationOfferedLoadEnvelope::new(0),
        Err(RiskReservationOfferedLoadEnvelopeError::ZeroMaxSupportedInFlightRiskIncreasingAdmissions)
    );
}

#[test]
fn s7b_zero_configured_offered_load_envelope_fails_closed_when_deserialized() {
    let error = toml::from_str::<RiskReservationSubstrateConfig>(
        r#"
enabled = true

[pool_lease_authority]
backend = "dynamo_db_conditional_write"
dependency_name = "risk-reservation-pool-leases"

[work_bounds]
max_current_position_count = 8
max_buckets_per_exposure = 8
max_terminal_cash_flow_count_per_exposure = 8

[offered_load_envelope]
max_supported_in_flight_risk_increasing_admissions = 0
"#,
    )
    .expect_err("zero configured envelope must fail closed while parsing config");

    assert!(
        error
            .to_string()
            .contains("ZeroMaxSupportedInFlightRiskIncreasingAdmissions")
    );
}

#[test]
fn s7b_documents_substrate_runtime_offered_load_boundary() {
    let contracts = include_str!("../src/bolt_v3_risk_reservation_substrate/contracts.rs");
    let state_owner = include_str!("../src/bolt_v3_risk_reservation_substrate/state_owner.rs");

    assert!(contracts.contains("runtime owns the bounded queue and fairness policy"));
    assert!(state_owner.contains("shed gate runs only on risk-increasing compare-and-reserve"));
    assert!(state_owner.contains("before kernel evaluation"));
}

#[test]
fn s2_sc_001_concurrent_correlated_candidates_never_breach_bucket_budget() {
    let bucket = bucket("risk_class", "alpha");
    let service = Arc::new(reconciled_admission_service(
        "pool-concurrency",
        "owner-concurrency",
    ));
    let view = Arc::new(published_view(
        RiskStateVersion::zero(),
        "pool-concurrency",
        "candidate-instrument",
        bucket.clone(),
        dec(10),
        dec(10),
    ));
    let safety = BoundReusableSafetyState {
        risk_state_version: RiskStateVersion::zero(),
        kill_switch_latched: false,
        loss_governor_halted: false,
    };
    let barrier = Arc::new(Barrier::new(2));

    let handles: Vec<_> = (0..2)
        .map(|index| {
            let service = Arc::clone(&service);
            let view = Arc::clone(&view);
            let safety = safety.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                service.compare_and_reserve(
                    view.as_ref(),
                    admission_candidate(
                        &format!("intent-concurrent-{index}"),
                        &format!("idempotency-concurrent-{index}"),
                        "pool-concurrency",
                        "candidate-instrument",
                        RiskStateVersion::zero(),
                        dec(6),
                    ),
                    safety,
                    None,
                    1_010,
                )
            })
        })
        .collect();

    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|handle| {
            handle
                .join()
                .expect("concurrent reserve thread should not panic")
        })
        .collect();

    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "only the admissible prefix may reserve against the shared correlated budget"
    );
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .len(),
        1,
        "the rejected concurrent candidate must not leave a partial reservation"
    );
    assert!(
        service
            .reserved_bucket_stress_loss(&bucket)
            .expect("reserved bucket total should be readable")
            <= dec(10),
        "shared bucket budget must never be observed breached"
    );
}

#[test]
fn s2_sc_002_caller_supplied_risk_numbers_are_ignored() {
    let bucket = bucket("risk_class", "alpha");
    let service = reconciled_admission_service("pool-caller-risk", "owner-caller-risk");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-caller-risk",
        "candidate-instrument",
        bucket,
        dec(5),
        dec(5),
    );
    let understated = CallerRiskDiagnostics {
        collateral_required: dec(0),
        equity_floor_stress_loss: dec(0),
        governor_realized_loss: dec(0),
    };

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-caller-risk",
                "idempotency-caller-risk",
                "pool-caller-risk",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(6),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            Some(understated),
            1_010,
        )
        .expect_err("authoritative kernel risk, not caller diagnostics, must control admission");

    let AdmissionReserveError::Rejected(rejection) = error else {
        panic!("expected cap rejection from kernel-computed risk, got {error:?}");
    };
    assert!(
        rejection
            .breached_dimensions
            .contains(&RiskCapDimension::GlobalStressLoss),
        "actual kernel stress exceeds the configured budget"
    );
    assert_eq!(
        rejection.diagnostic_mismatches.len(),
        3,
        "diagnostic caller risk numbers are recorded as mismatches but never trusted"
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .is_empty(),
        "rejected diagnostics cannot create a token or partial reservation"
    );
}

#[test]
fn s2_stale_view_reserve_is_rejected() {
    let bucket = bucket("risk_class", "alpha");
    let service = reconciled_admission_service("pool-stale-view", "owner-stale-view");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-stale-view",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );

    service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-first",
                "idempotency-first",
                "pool-stale-view",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(3),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("first reservation advances the risk-state version");

    let stale_error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-stale",
                "idempotency-stale",
                "pool-stale-view",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(3),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_011,
        )
        .expect_err("a superseded advisory view must fail closed before a token is issued");

    assert!(matches!(
        stale_error,
        AdmissionReserveError::StaleRiskStateVersion { .. }
    ));
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .len(),
        1,
        "stale reserve attempts must not add a second reservation"
    );
}

#[test]
fn s2_sc_013_every_cap_and_reused_safety_state_is_evaluated_inside_transaction() {
    let bucket = bucket("risk_class", "alpha");
    let service = reconciled_admission_service("pool-all-caps", "owner-all-caps");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-all-caps",
        "candidate-instrument",
        bucket.clone(),
        dec(2),
        dec(2),
    );

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-all-caps",
                "idempotency-all-caps",
                "pool-all-caps",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(6),
            ),
            BoundReusableSafetyState {
                risk_state_version: RiskStateVersion::zero(),
                kill_switch_latched: true,
                loss_governor_halted: true,
            },
            None,
            1_010,
        )
        .expect_err("breached caps and latched safety state must reject before token issuance");

    let AdmissionReserveError::Rejected(rejection) = error else {
        panic!("expected stateful rejection, got {error:?}");
    };
    assert_eq!(
        rejection.evaluated_dimensions,
        BTreeSet::from([
            RiskCapDimension::Collateral,
            RiskCapDimension::EquityFloorStressLoss,
            RiskCapDimension::GovernorRealizedLoss,
            RiskCapDimension::GlobalStressLoss,
            RiskCapDimension::ConcentrationBucket(bucket),
            RiskCapDimension::OpenOrderCount,
            RiskCapDimension::PositionQuantity,
            RiskCapDimension::KillSwitchLatch,
            RiskCapDimension::LossGovernorHalt,
        ]),
        "no stateful cap or reused safety state may be omitted from the transaction"
    );
    assert!(
        rejection.token_issued.is_none(),
        "rejected transactions must not issue a token that a later gate rejects"
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .is_empty(),
        "there is no rollback path because rejected transactions reserve nothing"
    );
}

#[test]
fn s2_preview_kernel_result_equals_commit_kernel_result_for_same_input() {
    let bucket = bucket("risk_class", "alpha");
    let service = reconciled_admission_service("pool-preview", "owner-preview");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-preview",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let preview_input = RiskPreviewInput {
        pool_id: PoolId::new("pool-preview").expect("pool id should be valid"),
        instrument_id: "candidate-instrument".to_string(),
        model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
            instrument_id: "candidate-instrument".to_string(),
        },
        side: "long".to_string(),
        quantity: dec(1),
        order_type: "limit".to_string(),
        time_in_force: "gtc".to_string(),
        max_unit_price: Some(dec(6)),
        max_cash_outlay: dec(6),
        source_view_version: RiskStateVersion::zero(),
        policy_epoch_id: "policy-epoch".to_string(),
    };

    let preview = RiskViewPublisher::preview(&view, &preview_input)
        .expect("preview should evaluate the pure kernel without reserving");
    let committed = service
        .compare_and_reserve(
            &view,
            admission_candidate_from_preview(
                "intent-preview",
                "idempotency-preview",
                preview_input,
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("same primitive facts should commit under the same kernel result");

    assert_eq!(
        committed.assessment, preview,
        "preview and commit must share the same pure kernel for identical inputs on one version"
    );
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .len(),
        1,
        "preview itself grants no capacity; only commit records the reservation"
    );
}

#[test]
fn s3_stale_descriptor_version_is_rejected_against_registry_resolved_active_version() {
    let bucket = bucket("risk_class", "alpha");
    let mut registry = InstrumentRiskRegistry::new();
    let descriptor_v1 = descriptor(
        "candidate-instrument",
        "descriptor-version-v1",
        "policy-epoch",
        &bucket,
        0,
    );
    let attestation_v1 = attestation_for(
        &descriptor_v1,
        "descriptor-producer",
        "descriptor-certifier",
    );
    registry
        .register_descriptor_version(descriptor_v1, Some(attestation_v1), 1_000)
        .expect("v1 descriptor should certify");
    registry
        .activate_descriptor_version(
            "candidate-instrument",
            "policy-epoch",
            "descriptor-version-v1",
        )
        .expect("v1 should activate");
    let descriptor_v2 = descriptor(
        "candidate-instrument",
        "descriptor-version-v2",
        "policy-epoch",
        &bucket,
        0,
    );
    let attestation_v2 = attestation_for(
        &descriptor_v2,
        "descriptor-producer",
        "descriptor-certifier",
    );
    registry
        .register_descriptor_version(descriptor_v2, Some(attestation_v2), 1_000)
        .expect("v2 descriptor should certify");
    registry
        .activate_descriptor_version(
            "candidate-instrument",
            "policy-epoch",
            "descriptor-version-v2",
        )
        .expect("v2 should supersede v1");

    let service = reconciled_admission_service("pool-stale-descriptor", "admission-owner");
    let certified = registry
        .resolve_active_descriptor("candidate-instrument", "policy-epoch")
        .expect("registry should resolve the active v2 descriptor");
    let view = published_view_from_certified(
        RiskStateVersion::zero(),
        "pool-stale-descriptor",
        bucket,
        dec(20),
        dec(20),
        certified,
    );
    let mut candidate = admission_candidate(
        "intent-stale-descriptor",
        "idempotency-stale-descriptor",
        "pool-stale-descriptor",
        "candidate-instrument",
        RiskStateVersion::zero(),
        dec(3),
    );
    candidate.expected_descriptor_version = "descriptor-version-v1".to_string();

    let error = service
        .compare_and_reserve_certified(
            &registry,
            &view,
            candidate,
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect_err("candidate naming a superseded descriptor version must fail closed");

    assert_eq!(
        error,
        DescriptorRegistryAdmissionError::DescriptorVersionMismatch {
            active_descriptor_version: "descriptor-version-v2".to_string(),
            candidate_descriptor_version: "descriptor-version-v1".to_string(),
        }
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .is_empty(),
        "stale descriptor mismatch must not mint a token or reserve"
    );
}

#[test]
fn s3_uncertified_or_digest_mismatched_descriptor_fails_closed() {
    let bucket = bucket("risk_class", "alpha");
    let descriptor = descriptor(
        "candidate-instrument",
        "descriptor-version",
        "policy-epoch",
        &bucket,
        0,
    );
    let mut uncertified_registry = InstrumentRiskRegistry::new();

    assert_eq!(
        uncertified_registry.register_descriptor_version(descriptor.clone(), None, 1_000),
        Err(DescriptorRegistryError::UncertifiedDescriptor)
    );
    assert_eq!(
        uncertified_registry.resolve_active_descriptor("candidate-instrument", "policy-epoch"),
        Err(DescriptorRegistryError::NoActiveDescriptor)
    );

    let mut digest_mismatch_registry = InstrumentRiskRegistry::new();
    let mut bad_attestation =
        attestation_for(&descriptor, "descriptor-producer", "descriptor-certifier");
    bad_attestation.descriptor_digest = hash("different-descriptor-content");

    assert_eq!(
        digest_mismatch_registry.register_descriptor_version(
            descriptor,
            Some(bad_attestation),
            1_000,
        ),
        Err(DescriptorRegistryError::AttestationDigestMismatch)
    );
    assert_eq!(
        digest_mismatch_registry.resolve_active_descriptor("candidate-instrument", "policy-epoch"),
        Err(DescriptorRegistryError::NoActiveDescriptor)
    );
}

#[test]
fn s3_self_certified_descriptor_fails_closed_for_the_reserving_identity() {
    let bucket = bucket("risk_class", "alpha");
    let mut registry = InstrumentRiskRegistry::new();
    let descriptor = descriptor(
        "candidate-instrument",
        "descriptor-version",
        "policy-epoch",
        &bucket,
        0,
    );
    let attestation = attestation_for(&descriptor, "descriptor-producer", "admission-owner");
    registry
        .register_descriptor_version(descriptor, Some(attestation), 1_000)
        .expect("descriptor can register, but same certifier/admitter must not reserve");
    registry
        .activate_descriptor_version("candidate-instrument", "policy-epoch", "descriptor-version")
        .expect("descriptor should activate");
    let service = reconciled_admission_service("pool-self-certified", "admission-owner");
    let certified = registry
        .resolve_active_descriptor("candidate-instrument", "policy-epoch")
        .expect("registry should resolve the active descriptor");
    let view = published_view_from_certified(
        RiskStateVersion::zero(),
        "pool-self-certified",
        bucket,
        dec(20),
        dec(20),
        certified,
    );

    let error = service
        .compare_and_reserve_certified(
            &registry,
            &view,
            admission_candidate(
                "intent-self-certified",
                "idempotency-self-certified",
                "pool-self-certified",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(3),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect_err("certifier identity must differ from the reserving identity");

    assert_eq!(
        error,
        DescriptorRegistryAdmissionError::CertifierMatchesAdmissionIdentity
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .is_empty(),
        "self-certified admission must halt before token issuance"
    );
}

#[test]
fn s3_descriptor_versions_are_immutable_and_changes_require_new_revalued_version() {
    let bucket = bucket("risk_class", "alpha");
    let mut registry = InstrumentRiskRegistry::new();
    let descriptor_v1 = descriptor(
        "candidate-instrument",
        "descriptor-version-v1",
        "policy-epoch",
        &bucket,
        0,
    );
    let attestation_v1 = attestation_for(
        &descriptor_v1,
        "descriptor-producer",
        "descriptor-certifier",
    );
    registry
        .register_descriptor_version(descriptor_v1.clone(), Some(attestation_v1), 1_000)
        .expect("v1 descriptor should certify");
    assert_eq!(
        registry
            .activate_descriptor_version(
                "candidate-instrument",
                "policy-epoch",
                "descriptor-version-v1"
            )
            .expect("first activation should not require revaluation"),
        DescriptorActivationStatus::InitialActivation
    );

    let mutated_v1 = descriptor(
        "candidate-instrument",
        "descriptor-version-v1",
        "policy-epoch",
        &bucket,
        -1,
    );
    let mutated_attestation =
        attestation_for(&mutated_v1, "descriptor-producer", "descriptor-certifier");
    assert_eq!(
        registry.register_descriptor_version(mutated_v1, Some(mutated_attestation), 1_000),
        Err(DescriptorRegistryError::ImmutableVersionMutationRejected)
    );

    let descriptor_v2 = descriptor(
        "candidate-instrument",
        "descriptor-version-v2",
        "policy-epoch",
        &bucket,
        -1,
    );
    let attestation_v2 = attestation_for(
        &descriptor_v2,
        "descriptor-producer",
        "descriptor-certifier",
    );
    registry
        .register_descriptor_version(descriptor_v2, Some(attestation_v2), 1_000)
        .expect("changed descriptor content must enter as a new version");
    assert_eq!(
        registry
            .activate_descriptor_version(
                "candidate-instrument",
                "policy-epoch",
                "descriptor-version-v2"
            )
            .expect("changed version activates only through revaluation status"),
        DescriptorActivationStatus::SupersededVersionRequiresRevaluation
    );
}

#[test]
fn s3_unmapped_terminal_state_emits_unknown_envelope_and_halts_admission_without_token() {
    let bucket = bucket("risk_class", "alpha");
    let mut registry = InstrumentRiskRegistry::new();
    let descriptor = descriptor(
        "candidate-instrument",
        "descriptor-version",
        "policy-epoch",
        &bucket,
        0,
    );
    let expected_envelope = descriptor.unknown_state_envelope.clone();
    let attestation = attestation_for(&descriptor, "descriptor-producer", "descriptor-certifier");
    registry
        .register_descriptor_version(descriptor, Some(attestation), 1_000)
        .expect("descriptor should certify");
    registry
        .activate_descriptor_version("candidate-instrument", "policy-epoch", "descriptor-version")
        .expect("descriptor should activate");

    assert_eq!(
        registry
            .observe_terminal_state("candidate-instrument", "policy-epoch", "unmapped-terminal")
            .expect("unknown state should emit an envelope"),
        TerminalStateObservation::Unknown(expected_envelope.clone())
    );

    let service = reconciled_admission_service("pool-unknown-state", "admission-owner");
    let certified = registry
        .resolve_active_descriptor("candidate-instrument", "policy-epoch")
        .expect("registry should still expose the active descriptor for review");
    let view = published_view_from_certified(
        RiskStateVersion::zero(),
        "pool-unknown-state",
        bucket,
        dec(20),
        dec(20),
        certified,
    );

    let error = service
        .compare_and_reserve_certified(
            &registry,
            &view,
            admission_candidate(
                "intent-unknown-state",
                "idempotency-unknown-state",
                "pool-unknown-state",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(3),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect_err("unknown terminal state must halt risk-increasing admission");

    assert_eq!(
        error,
        DescriptorRegistryAdmissionError::AdmissionHaltedByUnknownState {
            envelope: expected_envelope,
        }
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .is_empty(),
        "unknown-state halt must not mint an admission token"
    );
}

#[test]
fn s4_permit_consumption_is_atomic_and_double_spend_fails_closed() {
    let (service, _owner, _store) =
        reconciled_risk_context("pool-permit-consume", "owner-permit-consume");
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-permit-consume",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let permit = SizingDecisionPermit {
        permit_id: "permit-single-use".to_string(),
        source_view_version: RiskStateVersion::zero(),
        candidate_digest: "candidate-digest-a".to_string(),
    };

    service
        .compare_and_reserve(
            &view,
            admission_candidate_with_permit(
                "intent-permit-first",
                "idempotency-permit-first",
                "pool-permit-consume",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
                permit.clone(),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("first permit consumption should reserve");

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate_with_permit(
                "intent-permit-second",
                "idempotency-permit-second",
                "pool-permit-consume",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
                permit,
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_011,
        )
        .expect_err("a consumed permit must fail closed instead of authorizing a second order");

    assert_eq!(error, AdmissionReserveError::PermitAlreadyConsumed);
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .len(),
        1,
        "permit double-spend must not add a second reservation"
    );
}

#[test]
fn s4_same_idempotency_key_replays_existing_reservation_without_second_live_order() {
    let (service, owner, _store) =
        reconciled_risk_context("pool-idempotent-reserve", "owner-idempotent-reserve");
    let authority = SubmissionAuthority::new(owner);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-idempotent-reserve",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let candidate = admission_candidate(
        "intent-idempotent",
        "idempotency-idempotent",
        "pool-idempotent-reserve",
        "candidate-instrument",
        RiskStateVersion::zero(),
        dec(2),
    );

    let first = service
        .compare_and_reserve(
            &view,
            candidate.clone(),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("first idempotency key use should reserve");
    let replay = service
        .compare_and_reserve(
            &view,
            candidate,
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_011,
        )
        .expect("same idempotency key must replay the existing reservation result");

    assert_eq!(replay, first);
    assert_eq!(
        service
            .reservation_records()
            .expect("reservation records should be readable")
            .len(),
        1,
        "idempotent reservation replay must not create a second reservation"
    );

    let mut sink = RecordingLiveSubmitBoundary::default();
    let client_order_id = client_order_id("S4-IDEMPOTENT-ORDER");
    let submit = authority
        .submit_idempotently(&first, client_order_id, &mut sink, 1_100)
        .expect("first admitted order should reach the live boundary");
    let replay_submit = authority
        .submit_idempotently(&first, client_order_id, &mut sink, 1_101)
        .expect("submit replay should return the existing live result");

    assert_eq!(replay_submit, submit);
    assert_eq!(
        sink.submitted_client_order_ids(),
        vec![client_order_id],
        "submit replay must not send a second live order"
    );
}

#[test]
fn s4_submission_intent_is_durable_before_first_live_send() {
    let (service, owner, _store) =
        reconciled_risk_context("pool-durable-submit", "owner-durable-submit");
    let authority = SubmissionAuthority::new(owner);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-durable-submit",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-durable-submit",
                "idempotency-durable-submit",
                "pool-durable-submit",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("reservation should issue an admission token");
    let client_order_id = client_order_id("S4-DURABLE-ORDER");
    let mut sink = RecordingLiveSubmitBoundary::default();

    let admitted = authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("submission authority must persist intent and construct AdmittedOrder");

    let intents = authority
        .durable_submission_intents()
        .expect("durable intents should be readable");
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].client_order_id(), client_order_id);
    assert!(
        sink.submitted_client_order_ids().is_empty(),
        "durable intent must exist before the first live send"
    );

    authority
        .submit_prepared(admitted, &mut sink, 1_101)
        .expect("prepared admitted order should submit");
    assert_eq!(sink.submitted_client_order_ids(), vec![client_order_id]);
}

#[test]
fn s4_sc_004_restart_reconciles_durable_intent_to_one_live_order_and_one_reservation() {
    let (service, owner, store) = reconciled_risk_context("pool-restart", "owner-before-crash");
    let authority = SubmissionAuthority::new(owner);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-restart",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-restart",
                "idempotency-restart",
                "pool-restart",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("reservation should issue an admission token");
    let client_order_id = client_order_id("S4-RESTART-ORDER");
    authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("crash point: durable intent exists before send");

    let restarted_owner = RiskStateOwner::acquire(
        store,
        PoolId::new("pool-restart").expect("pool id should be valid"),
        "owner-after-crash",
    )
    .expect("successor owner should acquire the pool");
    let restarted_authority = SubmissionAuthority::new(restarted_owner.clone());
    let reconciler = LifecycleReconciler::new(restarted_owner);
    let mut sink = RecordingLiveSubmitBoundary::default();

    let summary = reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("restart reconciliation should recover the durable intent");

    assert_eq!(summary.live_order_count, 1);
    assert_eq!(summary.reservation_count, 1);
    assert!(
        summary.risk_state_version > reservation.admission_token.risk_state_version,
        "reconciliation must leave a coherent advanced risk-state version"
    );
    assert_eq!(
        sink.submitted_client_order_ids(),
        vec![client_order_id],
        "crash recovery must create exactly one live order"
    );

    let replay = restarted_authority
        .submit_durable_intent("idempotency-restart", &mut sink, 1_201)
        .expect("replay of recovered intent should return existing result");
    assert_eq!(replay.client_order_id, client_order_id);
    assert_eq!(
        sink.submitted_client_order_ids(),
        vec![client_order_id],
        "replay after reconciliation must not send a second live order"
    );
}

#[test]
fn s4_reconnect_reconciliation_uses_nt_order_status_report_identity() {
    let (service, owner, store) =
        reconciled_risk_context("pool-reconnect", "owner-before-reconnect");
    let authority = SubmissionAuthority::new(owner);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-reconnect",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-reconnect",
                "idempotency-reconnect",
                "pool-reconnect",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("reservation should issue an admission token");
    let client_order_id = client_order_id("S4-RECONNECT-ORDER");
    authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("durable intent should exist before reconnect reconciliation");

    let restarted_owner = RiskStateOwner::acquire(
        store,
        PoolId::new("pool-reconnect").expect("pool id should be valid"),
        "owner-after-reconnect",
    )
    .expect("successor owner should acquire the pool");
    let reconciler = LifecycleReconciler::new(restarted_owner);
    let mut sink = RecordingLiveSubmitBoundary::default();

    let summary = reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: vec![NtOrderStatusReportTruth {
                    client_order_id,
                    event_id: "nt-order-status-event".to_string(),
                    ts_event_unix_nanos: 1_150,
                    ts_init_unix_nanos: 1_151,
                }],
                fill_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("NT OrderStatusReport identity should reconcile the durable intent");

    assert_eq!(summary.live_order_count, 1);
    assert_eq!(summary.reservation_count, 1);
    assert!(
        sink.submitted_client_order_ids().is_empty(),
        "an NT OrderStatusReport for the ClientOrderId proves the order exists; no replay send needed"
    );
}

#[test]
fn s4_sc_012_submit_boundary_is_admitted_order_only_and_authority_owned() {
    let contracts = include_str!("../src/bolt_v3_risk_reservation_substrate/contracts.rs");
    assert!(
        contracts.contains("pub struct AdmittedOrder {\n    admission_token: AdmissionToken,"),
        "AdmittedOrder fields must stay private so external modules cannot construct it"
    );
    assert!(
        !contracts.contains("pub client_order_id"),
        "AdmittedOrder must not expose public struct-literal construction fields"
    );

    let authority_source =
        include_str!("../src/bolt_v3_risk_reservation_substrate/submission_authority.rs");
    assert!(
        authority_source.contains("fn submit_admitted_order")
            && authority_source.contains("order: AdmittedOrder"),
        "the live submit trait must accept only AdmittedOrder"
    );
    assert!(
        authority_source.contains("prepare_admitted_order")
            && authority_source.contains("prepare_submission_intent"),
        "submission authority must be the only public path that asks the state owner to move Reserved -> Submitted"
    );
    let state_owner_source =
        include_str!("../src/bolt_v3_risk_reservation_substrate/state_owner.rs");
    assert!(
        state_owner_source.contains("ReservationLifecycleState::Reserved")
            && state_owner_source.contains("ReservationLifecycleState::Submitted"),
        "the store transition that backs AdmittedOrder construction must be Reserved -> Submitted"
    );

    let forbidden_constructor = "from_submitted_reservation(";
    for (path, source) in risk_reservation_module_sources() {
        if path == "contracts.rs" || path == "submission_authority.rs" {
            continue;
        }
        assert!(
            !source.contains(forbidden_constructor),
            "{path} must not construct AdmittedOrder; only submission_authority owns the boundary"
        );
    }
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

fn reconciled_admission_service(pool_id: &str, owner_id: &str) -> AdmissionService {
    let (service, _owner, _store) = reconciled_risk_context(pool_id, owner_id);
    service
}

fn reconciled_risk_context(
    pool_id: &str,
    owner_id: &str,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    reconciled_risk_context_with_work_bounds(pool_id, owner_id, roomy_work_bounds())
}

fn reconciled_risk_context_with_work_bounds(
    pool_id: &str,
    owner_id: &str,
    work_bounds: RiskReservationWorkBounds,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    let (service, owner, store) =
        unreconciled_risk_context_with_work_bounds(pool_id, owner_id, work_bounds);
    owner
        .reconcile_before_new_risk()
        .expect("owner should reconcile before admission");
    (service, owner, store)
}

fn reconciled_risk_context_with_offered_load_envelope(
    pool_id: &str,
    owner_id: &str,
    envelope: RiskReservationOfferedLoadEnvelope,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    let (service, owner, store) =
        unreconciled_risk_context_with_offered_load_envelope(pool_id, owner_id, envelope);
    owner
        .reconcile_before_new_risk()
        .expect("owner should reconcile before admission");
    (service, owner, store)
}

fn unreconciled_risk_context(
    pool_id: &str,
    owner_id: &str,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    unreconciled_risk_context_with_work_bounds(pool_id, owner_id, roomy_work_bounds())
}

fn unreconciled_risk_context_with_work_bounds(
    pool_id: &str,
    owner_id: &str,
    work_bounds: RiskReservationWorkBounds,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        format!("{pool_id}-lease-authority"),
    )
    .expect("lease authority dependency should be valid");
    let store = FencedRiskStateStore::new(substrate_config(lease_authority, work_bounds));
    let owner = RiskStateOwner::acquire(
        store.clone(),
        PoolId::new(pool_id).expect("pool id should be valid"),
        owner_id,
    )
    .expect("risk state owner should acquire the pool");
    (AdmissionService::new(owner.clone()), owner, store)
}

fn unreconciled_risk_context_with_offered_load_envelope(
    pool_id: &str,
    owner_id: &str,
    envelope: RiskReservationOfferedLoadEnvelope,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        format!("{pool_id}-lease-authority"),
    )
    .expect("lease authority dependency should be valid");
    let store = FencedRiskStateStore::new(RiskReservationSubstrateConfig {
        enabled: true,
        pool_lease_authority: lease_authority,
        work_bounds: roomy_work_bounds(),
        offered_load_envelope: Some(envelope),
    });
    let owner = RiskStateOwner::acquire(
        store.clone(),
        PoolId::new(pool_id).expect("pool id should be valid"),
        owner_id,
    )
    .expect("risk state owner should acquire the pool");
    (AdmissionService::new(owner.clone()), owner, store)
}

fn substrate_config(
    lease_authority: ConfiguredLeaseAuthority,
    work_bounds: RiskReservationWorkBounds,
) -> RiskReservationSubstrateConfig {
    RiskReservationSubstrateConfig {
        enabled: true,
        pool_lease_authority: lease_authority,
        work_bounds,
        offered_load_envelope: None,
    }
}

fn roomy_work_bounds() -> RiskReservationWorkBounds {
    configured_work_bounds(8, 8, 8)
}

fn configured_work_bounds(
    max_current_position_count: usize,
    max_buckets_per_exposure: usize,
    max_terminal_cash_flow_count_per_exposure: usize,
) -> RiskReservationWorkBounds {
    RiskReservationWorkBounds::new(
        max_current_position_count,
        max_buckets_per_exposure,
        max_terminal_cash_flow_count_per_exposure,
    )
    .expect("configured work bounds should be valid")
}

fn unlatched_safety(risk_state_version: RiskStateVersion) -> BoundReusableSafetyState {
    BoundReusableSafetyState {
        risk_state_version,
        kill_switch_latched: false,
        loss_governor_halted: false,
    }
}

fn reduce_only_safety_action_request(
    action_id: &str,
    position_id: &str,
    risk_state_version: RiskStateVersion,
    safety_state: BoundReusableSafetyState,
    after: Vec<RiskExposure>,
    max_exposure_count: usize,
) -> SafetyActionAdmissionRequest {
    let after_exposure_count = after.len();
    SafetyActionAdmissionRequest {
        action_id: action_id.to_string(),
        action: SafetyAction::ReduceOnlyCloseExistingPosition {
            position_id: position_id.to_string(),
        },
        safety_state,
        after: RiskExposureSetInput {
            risk_state_version,
            exposures: after,
        },
        proof_domain: SafetyActionProofDomain {
            max_exposure_count,
            before_exposure_count: 0,
            after_exposure_count,
        },
    }
}

fn risk_reservation_module_sources() -> Vec<(String, String)> {
    let module_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("bolt_v3_risk_reservation_substrate");
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(&module_dir).expect("risk reservation module directory exists") {
        let entry = entry.expect("risk reservation module entry is readable");
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        sources.push((
            path.file_name()
                .expect("module source has a file name")
                .to_string_lossy()
                .into_owned(),
            std::fs::read_to_string(&path).expect("module source is readable"),
        ));
    }
    sources
}

fn published_view(
    risk_state_version: RiskStateVersion,
    pool_id: &str,
    instrument_id: &str,
    bucket: ConcentrationBucket,
    global_headroom: Decimal,
    bucket_headroom: Decimal,
) -> bolt_v2::bolt_v3_risk_reservation_substrate::risk_view_publisher::PublishedRiskView {
    published_view_with_positions(
        risk_state_version,
        pool_id,
        instrument_id,
        bucket,
        global_headroom,
        bucket_headroom,
        Vec::new(),
    )
}

fn published_view_with_positions(
    risk_state_version: RiskStateVersion,
    pool_id: &str,
    instrument_id: &str,
    bucket: ConcentrationBucket,
    global_headroom: Decimal,
    bucket_headroom: Decimal,
    positions: Vec<RiskExposure>,
) -> bolt_v2::bolt_v3_risk_reservation_substrate::risk_view_publisher::PublishedRiskView {
    published_view_with_classification(
        risk_state_version,
        pool_id,
        instrument_id,
        vec![bucket],
        global_headroom,
        bucket_headroom,
        vec![dec(0), dec(99)],
        positions,
    )
}

fn published_view_with_classification(
    risk_state_version: RiskStateVersion,
    pool_id: &str,
    instrument_id: &str,
    buckets: Vec<ConcentrationBucket>,
    global_headroom: Decimal,
    bucket_headroom: Decimal,
    terminal_cash_flows: Vec<Decimal>,
    positions: Vec<RiskExposure>,
) -> bolt_v2::bolt_v3_risk_reservation_substrate::risk_view_publisher::PublishedRiskView {
    let descriptor_attributes = BTreeMap::from_iter(buckets.iter().map(|bucket| {
        (
            format!("descriptor_{}", bucket.bucket_class()),
            bucket.bucket_value().to_string(),
        )
    }));
    let classification_dimensions = buckets.iter().map(|bucket| {
        dimension(
            bucket.bucket_class(),
            &format!("descriptor_{}", bucket.bucket_class()),
        )
    });
    RiskViewPublisher::publish(RiskViewPublicationInput {
        sizing_view: RiskSizingView {
            risk_state_version,
            reconciliation_ready: true,
            reference_growth_wealth: dec(100),
            conservative_liquidation_equity: dec(100),
            free_collateral: global_headroom,
            equity_floor_headroom: global_headroom,
            governor_headroom: global_headroom,
            global_stress_loss_headroom: global_headroom,
            bucket_stress_loss_headrooms: BTreeMap::from_iter(
                buckets
                    .iter()
                    .cloned()
                    .map(|bucket| (bucket, bucket_headroom)),
            ),
            open_order_headroom: 1,
            position_quantity_headroom: dec(100),
        },
        active_descriptor: ActiveDescriptorView {
            instrument_id: instrument_id.to_string(),
            descriptor_version: "descriptor-version".to_string(),
            policy_epoch_id: "policy-epoch".to_string(),
            terminal_state_ids: terminal_state_ids_with_count(terminal_cash_flows.len()),
            terminal_cash_flows,
        },
        descriptor_attributes: RiskDescriptorCanonicalAttributes::new(descriptor_attributes)
            .expect("descriptor attributes should classify"),
        classification_policy: classification_policy(classification_dimensions),
        caller_declared_buckets: Vec::new(),
        portfolio: RiskPortfolioSnapshot { positions },
        portfolio_scope_id: pool_id.to_string(),
    })
    .expect("published view should resolve active descriptor and classification")
}

fn exposures_with_count(count: usize, risk_bucket: ConcentrationBucket) -> Vec<RiskExposure> {
    (0..count)
        .map(|index| exposure(&format!("position-{index}"), [risk_bucket.clone()], 2, 3, 1))
        .collect()
}

fn terminal_cash_flows_with_count(count: usize) -> Vec<Decimal> {
    (0..count)
        .map(|index| if index == 0 { dec(0) } else { dec(99) })
        .collect()
}

fn terminal_state_ids_with_count(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("terminal-state-{index}"))
        .collect()
}

fn assert_no_reservation_effect(
    service: &AdmissionService,
    owner: &RiskStateOwner,
    before_version: RiskStateVersion,
) {
    assert_eq!(
        owner
            .policy_epoch_snapshot()
            .expect("policy state should be readable after rejected reserve")
            .risk_state_version,
        before_version,
        "over-bound reserve must not advance the risk state version"
    );
    assert!(
        service
            .reservation_records()
            .expect("reservation records should be readable after rejected reserve")
            .is_empty(),
        "over-bound reserve must not write a reservation record"
    );
}

fn published_view_from_certified(
    risk_state_version: RiskStateVersion,
    pool_id: &str,
    bucket: ConcentrationBucket,
    global_headroom: Decimal,
    bucket_headroom: Decimal,
    certified: CertifiedActiveDescriptor,
) -> bolt_v2::bolt_v3_risk_reservation_substrate::risk_view_publisher::PublishedRiskView {
    RiskViewPublisher::publish(RiskViewPublicationInput {
        sizing_view: RiskSizingView {
            risk_state_version,
            reconciliation_ready: true,
            reference_growth_wealth: dec(100),
            conservative_liquidation_equity: dec(100),
            free_collateral: global_headroom,
            equity_floor_headroom: global_headroom,
            governor_headroom: global_headroom,
            global_stress_loss_headroom: global_headroom,
            bucket_stress_loss_headrooms: BTreeMap::from([(bucket, bucket_headroom)]),
            open_order_headroom: 1,
            position_quantity_headroom: dec(100),
        },
        active_descriptor: certified.active_descriptor,
        descriptor_attributes: certified.descriptor_attributes,
        classification_policy: classification_policy([dimension(
            "risk_class",
            "descriptor_risk_class",
        )]),
        caller_declared_buckets: Vec::new(),
        portfolio: RiskPortfolioSnapshot {
            positions: Vec::new(),
        },
        portfolio_scope_id: pool_id.to_string(),
    })
    .expect("published view should consume the registry-certified active descriptor")
}

fn admission_candidate(
    intent_id: &str,
    idempotency_key: &str,
    pool_id: &str,
    instrument_id: &str,
    source_view_version: RiskStateVersion,
    max_cash_outlay: Decimal,
) -> AdmissionCandidate {
    admission_candidate_from_preview(
        intent_id,
        idempotency_key,
        RiskPreviewInput {
            pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
            instrument_id: instrument_id.to_string(),
            model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
                instrument_id: instrument_id.to_string(),
            },
            side: "long".to_string(),
            quantity: dec(1),
            order_type: "limit".to_string(),
            time_in_force: "gtc".to_string(),
            max_unit_price: Some(max_cash_outlay),
            max_cash_outlay,
            source_view_version,
            policy_epoch_id: "policy-epoch".to_string(),
        },
    )
}

fn admission_candidate_from_preview(
    intent_id: &str,
    idempotency_key: &str,
    preview: RiskPreviewInput,
) -> AdmissionCandidate {
    AdmissionCandidate {
        intent_id: intent_id.to_string(),
        idempotency_key: idempotency_key.to_string(),
        pool_id: preview.pool_id,
        instrument_id: preview.instrument_id,
        model_risk_scope: preview.model_risk_scope,
        expected_descriptor_version: "descriptor-version".to_string(),
        side: preview.side,
        quantity: preview.quantity,
        order_type: preview.order_type,
        time_in_force: preview.time_in_force,
        max_unit_price: preview.max_unit_price,
        max_cash_outlay: preview.max_cash_outlay,
        venue_model_version: "venue-model".to_string(),
        fee_model_version: "fee-model".to_string(),
        source_view_version: preview.source_view_version,
        policy_epoch_id: preview.policy_epoch_id,
        signal_binding: "signal-binding".to_string(),
        model_binding: "model-binding".to_string(),
        attestation_binding: "attestation-binding".to_string(),
        sizing_permit: SizingDecisionPermit {
            permit_id: "permit".to_string(),
            source_view_version: preview.source_view_version,
            candidate_digest: "candidate-digest".to_string(),
        },
        expires_at_unix_nanos: 2_000,
    }
}

fn admission_candidate_with_permit(
    intent_id: &str,
    idempotency_key: &str,
    pool_id: &str,
    instrument_id: &str,
    source_view_version: RiskStateVersion,
    max_cash_outlay: Decimal,
    sizing_permit: SizingDecisionPermit,
) -> AdmissionCandidate {
    AdmissionCandidate {
        sizing_permit,
        ..admission_candidate(
            intent_id,
            idempotency_key,
            pool_id,
            instrument_id,
            source_view_version,
            max_cash_outlay,
        )
    }
}

fn client_order_id(value: &str) -> ClientOrderId {
    ClientOrderId::from(value)
}

#[derive(Default)]
struct RecordingLiveSubmitBoundary {
    submitted: Vec<ClientOrderId>,
}

impl RecordingLiveSubmitBoundary {
    fn submitted_client_order_ids(&self) -> Vec<ClientOrderId> {
        self.submitted.clone()
    }
}

impl LiveSubmitBoundary for RecordingLiveSubmitBoundary {
    type Error = SubmissionAuthorityError;

    fn submit_admitted_order(
        &mut self,
        order: AdmittedOrder,
    ) -> Result<LiveSubmitReceipt, Self::Error> {
        self.submitted.push(order.client_order_id());
        Ok(LiveSubmitReceipt {
            client_order_id: order.client_order_id(),
            risk_state_version: order.risk_state_version(),
        })
    }
}

fn descriptor(
    instrument_id: &str,
    descriptor_version: &str,
    policy_epoch_id: &str,
    bucket: &ConcentrationBucket,
    worst_terminal_cash_flow: i64,
) -> InstrumentRiskDescriptor {
    InstrumentRiskDescriptor::new(
        instrument_id.to_string(),
        descriptor_version.to_string(),
        policy_epoch_id.to_string(),
        vec!["terminal-loss".to_string(), "terminal-gain".to_string()],
        vec![dec(worst_terminal_cash_flow), dec(99)],
        DescriptorTerminalStateEnvelope {
            terminal_state_id: "unknown-terminal-envelope".to_string(),
            terminal_cash_flow: dec(worst_terminal_cash_flow),
        },
        RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
            "descriptor_risk_class".to_string(),
            bucket.bucket_value().to_string(),
        )]))
        .expect("descriptor attributes should classify"),
    )
    .expect("descriptor fixture should be valid")
}

fn attestation_for(
    descriptor: &InstrumentRiskDescriptor,
    producer_identity: &str,
    certifier_identity: &str,
) -> DescriptorCoverageAttestation {
    DescriptorCoverageAttestation {
        descriptor_digest: descriptor
            .canonical_digest()
            .expect("descriptor fixture should digest"),
        producer_identity: producer_identity.to_string(),
        certifier_identity: certifier_identity.to_string(),
        decision: DescriptorCertificationDecision::Approved,
        evidence: DescriptorCertificationEvidence {
            evidence_digest: hash("descriptor-evidence"),
            terminal_state_count: descriptor.terminal_state_ids.len(),
            classification_attribute_count: 1,
        },
        valid_from_unix_nanos: 900,
        valid_until_unix_nanos: 2_000,
        revoked: false,
    }
}

fn hash(value: &str) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(value.as_bytes()))
}
