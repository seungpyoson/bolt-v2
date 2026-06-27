use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Barrier},
    thread,
};

use bolt_v2::bolt_v3_risk_reservation_substrate::{
    admission_service::{
        AdmissionReserveError, AdmissionService, BoundReusableSafetyState, CallerRiskDiagnostics,
        RiskCapDimension, RiskReservationWorkDimension, SafetyActionAdmissionError,
        SafetyActionAdmissionRequest, SafetyActionProofDomain,
    },
    contracts::{
        ActiveDescriptorView, AdmissionCandidate, AdmissionToken, AdmittedOrder,
        ConfiguredLeaseAuthority, LeaseAuthorityBackend, LiveSubmissionRecord,
        ModelRiskEvaluationScope, PolicyApproval, PoolId, PoolOwnershipLease,
        PreparedEpochDescriptor, PreparedPolicyEpoch, ReservationLifecycleState, RiskAssessment,
        RiskPreviewInput, RiskReservationOfferedLoadEnvelope,
        RiskReservationOfferedLoadEnvelopeError, RiskReservationSubstrateConfig,
        RiskReservationWorkBounds, RiskSizingView, RiskStateVersion, SafetyAction,
        SafetyPolicyEnvelope, SizingDecisionPermit,
    },
    instrument_risk_registry::{
        CertifiedActiveDescriptor, DescriptorActivationStatus, DescriptorCertificationDecision,
        DescriptorCertificationEvidence, DescriptorCoverageAttestation,
        DescriptorRegistryAdmissionError, DescriptorRegistryError, DescriptorTerminalStateEnvelope,
        InstrumentRiskDescriptor, InstrumentRiskRegistry, TerminalStateObservation,
    },
    lifecycle_reconciler::{
        LifecycleReconciler, LifecycleReconciliationError, NtExecutionTruth, NtFillReportTruth,
        NtOrderStatusReportTruth, NtOrderStatusTruth, NtSettlementTruth,
    },
    reservation_ledger::{
        LifecycleReconciliationFaultKind, RiskReservationCommit, RiskReservationError,
        RiskReservationTransaction, SubstrateReservationRecord,
    },
    risk_classifier::{
        ConcentrationBucket, ConcentrationBucketDimension, RiskClassificationError,
        RiskClassificationPolicy, RiskClassifier, RiskDescriptorCanonicalAttributes,
    },
    risk_kernel::{
        RiskCandidate, RiskEvaluationScope, RiskExposure, RiskKernel, RiskKernelError,
        RiskKernelInput, RiskPortfolioSnapshot,
    },
    risk_view_publisher::{RiskViewPublicationInput, RiskViewPublisher},
    state_owner::{
        DurableRiskMutation, FencedRiskStateStore, PolicyEpochAlertReason, RiskMutationKind,
        RiskStateMutationError, RiskStateOwner, RiskSubmissionMutationError,
    },
    submission_authority::{
        LiveSubmitBoundary, LiveSubmitReceipt, SubmissionAuthority, SubmissionAuthorityError,
    },
};
use nautilus_model::identifiers::ClientOrderId;
use rust_decimal::Decimal;

#[test]
fn s5_reduce_only_safety_action_is_admitted_while_kill_switch_and_governor_freeze_new_risk() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s5-reduce",
        "owner-s5-reduce",
        "intent-s5-reduce",
        "idempotency-s5-reduce",
        "S5-REDUCE-ORDER",
    );
    let service = AdmissionService::new(owner.clone());
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s5-reduce-open"))
        .expect("open truth should make the order fillable before the partial fill");
    reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s5-reduce-fill",
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("partial fill should create authoritative filled-position exposure");
    let filled_position_id = service
        .reservation_records()
        .expect("reservation ledger should remain readable")
        .into_iter()
        .find(|record| record.filled_position_exposure.is_some())
        .expect("authoritative fill should leave a filled-position ledger target")
        .admission_token
        .reservation_id;
    let source_version = owner
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;
    let frozen = BoundReusableSafetyState {
        risk_state_version: source_version,
        kill_switch_latched: true,
        loss_governor_halted: true,
    };
    let request =
        reduce_only_safety_action_request("safety-action-reduce", &filled_position_id, frozen, 4);

    let admission = service
        .admit_safety_action(request)
        .expect("derived reduce-only proof should bypass the new-risk freeze");

    assert_eq!(admission.action_id, "safety-action-reduce");
    assert_eq!(admission.source_risk_state_version, source_version);
    assert_eq!(
        admission.before.equity_floor_stress_loss - admission.after.equity_floor_stress_loss,
        dec(23),
        "derived after must remove the exact filled-position exposure from the ledger"
    );
    assert_eq!(
        admission.before.governor_realized_loss - admission.after.governor_realized_loss,
        dec(25),
        "derived after must remove the exact filled-position governor exposure from the ledger"
    );
    assert_eq!(admission.after.equity_floor_stress_loss, dec(20));
    assert_eq!(admission.after.governor_realized_loss, dec(20));
}

#[test]
fn s5_reduce_only_safety_action_is_admitted_before_new_risk_reconciliation() {
    let (service, owner, store) =
        reconciled_risk_context("pool-s5-unreconciled", "owner-s5-unreconciled");
    let bucket = bucket("risk_class", "safety");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-s5-unreconciled",
        "unreconciled-instrument",
        bucket,
        dec(100),
        dec(100),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s5-unreconciled-source",
                "idempotency-s5-unreconciled-source",
                "pool-s5-unreconciled",
                "unreconciled-instrument",
                RiskStateVersion::zero(),
                dec(20),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("initial reservation is allowed after explicit setup reconciliation");
    let authority = SubmissionAuthority::new(owner.clone());
    let client_order_id = client_order_id("S5-UNRECONCILED-ORDER");
    authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("submission should create open-order ledger target");
    let successor = RiskStateOwner::acquire(
        store,
        PoolId::new("pool-s5-unreconciled").expect("pool id should be valid"),
        "owner-s5-unreconciled-successor",
    )
    .expect("successor owner should acquire unreconciled pool");
    let service = AdmissionService::new(successor.clone());
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
    let source_version = successor
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;

    let request = cancel_order_safety_action_request(
        "safety-action-unreconciled",
        "S5-UNRECONCILED-ORDER",
        unlatched_safety(source_version),
        4,
    );

    let admission = service
        .admit_safety_action(request)
        .expect("derived cancel proof should bypass not-yet-reconciled new-risk freeze");

    assert_eq!(admission.source_risk_state_version, source_version);
    assert_eq!(admission.before.equity_floor_stress_loss, dec(20));
    assert_eq!(admission.after.equity_floor_stress_loss, Decimal::ZERO);
}

#[test]
fn f1_cancel_existing_order_derives_after_from_ledger_open_order() {
    let (service, owner, _store) = reconciled_risk_context("pool-f1-cancel", "owner-f1-cancel");
    let bucket = bucket("risk_class", "cancel");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-f1-cancel",
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-f1-cancel",
                "idempotency-f1-cancel",
                "pool-f1-cancel",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(20),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("reservation should create open-order ledger exposure");
    let client_order_id = client_order_id("F1-CANCEL-ORDER");
    SubmissionAuthority::new(owner.clone())
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("submission should bind client order id to reservation");
    let source_version = owner
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;

    let admission = service
        .admit_safety_action(cancel_order_safety_action_request(
            "safety-action-f1-cancel",
            "F1-CANCEL-ORDER",
            unlatched_safety(source_version),
            4,
        ))
        .expect("cancel action should derive after by removing the named open order");

    assert_eq!(
        admission.before.equity_floor_stress_loss - admission.after.equity_floor_stress_loss,
        dec(20)
    );
    assert_eq!(
        admission.before.governor_realized_loss - admission.after.governor_realized_loss,
        dec(20)
    );
    assert_eq!(admission.after.equity_floor_stress_loss, Decimal::ZERO);
    assert_eq!(admission.after.governor_realized_loss, Decimal::ZERO);
}

#[test]
fn f1_unknown_named_safety_action_target_fails_closed_without_commit() {
    let (service, owner, _store) = reconciled_risk_context("pool-f1-unknown", "owner-f1-unknown");
    let source_version = owner
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;

    let error = service
        .admit_safety_action(reduce_only_safety_action_request(
            "safety-action-f1-unknown",
            "missing-position",
            unlatched_safety(source_version),
            4,
        ))
        .expect_err("unknown named SafetyAction target must fail closed");

    assert_eq!(error, SafetyActionAdmissionError::UnknownSafetyActionTarget);
    assert_eq!(
        owner
            .policy_epoch_snapshot()
            .expect("version should remain readable after rejection")
            .risk_state_version,
        source_version,
        "unknown target must not commit a SafetyAction mutation"
    );
}

#[test]
fn f1_safety_action_request_has_no_caller_after_field() {
    let source = include_str!("../src/bolt_v3_risk_reservation_substrate/admission_service.rs");
    let request = source
        .split("pub struct SafetyActionAdmissionRequest")
        .nth(1)
        .and_then(|tail| tail.split("pub struct SafetyActionProofDomain").next())
        .expect("SafetyActionAdmissionRequest source block should be present");

    assert!(
        !request.contains("after:"),
        "caller-supplied after exposure must be structurally absent from SafetyActionAdmissionRequest"
    );
}

#[test]
fn s5_safety_action_reduction_proof_fails_closed_when_exposure_domain_exceeds_bound() {
    let (service, owner, _store) = reconciled_risk_context("pool-s5-bound", "owner-s5-bound");
    let bucket = bucket("risk_class", "safety");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-s5-bound",
        "affected-a",
        bucket,
        dec(100),
        dec(100),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-s5-bound",
                "idempotency-s5-bound",
                "pool-s5-bound",
                "affected-a",
                RiskStateVersion::zero(),
                dec(20),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_000,
        )
        .expect("reservation should create bounded-domain source exposure");
    let client_order_id = client_order_id("S5-BOUND-ORDER");
    SubmissionAuthority::new(owner.clone())
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("submission should bind client order id to reservation");
    let source_version = owner
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;
    let request = cancel_order_safety_action_request(
        "safety-action-bound",
        "S5-BOUND-ORDER",
        unlatched_safety(source_version),
        0,
    );

    assert_eq!(
        service.admit_safety_action(request),
        Err(SafetyActionAdmissionError::InvalidProofDomain),
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
    let client_order_id = client_order_id("S7B-WITHIN-ORDER");
    SubmissionAuthority::new(owner.clone())
        .prepare_admitted_order(&admission, client_order_id, 1_100)
        .expect("submission should bind client order id for derived SafetyAction");
    let source_version = owner
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;
    let safety = service
        .admit_safety_action(cancel_order_safety_action_request(
            "safety-action-s7b-within",
            "S7B-WITHIN-ORDER",
            unlatched_safety(source_version),
            4,
        ))
        .expect("SafetyAction should bypass offered-load shedding inside the envelope");
    assert_eq!(safety.source_risk_state_version, source_version);
    assert_eq!(
        safety.risk_state_version,
        source_version.next().expect("test version should advance")
    );
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
    assert_eq!(
        lifecycle_version,
        RiskStateVersion::new(2),
        "direct reconciliation releases the unsubmitted orphan and must advance the risk version"
    );

    let priority_service = AdmissionService::new(successor.clone());
    let priority_view = published_view(
        lifecycle_version,
        "pool-s7b-priority",
        "candidate-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
    );
    let reservation = priority_service
        .compare_and_reserve(
            &priority_view,
            admission_candidate(
                "intent-s7b-priority-safety-source",
                "idempotency-s7b-priority-safety-source",
                "pool-s7b-priority",
                "candidate-instrument",
                lifecycle_version,
                dec(20),
            ),
            unlatched_safety(lifecycle_version),
            None,
            1_200,
        )
        .expect("source reservation should be available before offered-load pressure");
    let client_order_id = client_order_id("S7B-PRIORITY-ORDER");
    SubmissionAuthority::new(successor.clone())
        .prepare_admitted_order(&reservation, client_order_id, 1_210)
        .expect("submission should bind client order id for derived SafetyAction");
    let source_version = successor
        .policy_epoch_snapshot()
        .expect("source version should be readable")
        .risk_state_version;
    let safety = priority_service
        .admit_safety_action(cancel_order_safety_action_request(
            "safety-action-s7b-priority",
            "S7B-PRIORITY-ORDER",
            unlatched_safety(source_version),
            4,
        ))
        .expect("SafetyAction should bypass the shed gate while admissions are over envelope");
    assert_eq!(safety.source_risk_state_version, source_version);
    assert_eq!(
        safety.risk_state_version,
        source_version.next().expect("test version should advance")
    );

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
fn s2_stale_transaction_reports_actual_mismatched_risk_state_version() {
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-static-version-reporting",
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let candidate = admission_candidate(
        "intent-static-version-reporting",
        "idempotency-static-version-reporting",
        "pool-static-version-reporting",
        "candidate-instrument",
        RiskStateVersion::zero(),
        dec(3),
    );
    let kernel_input = view
        .kernel_input_for_candidate(&candidate)
        .expect("candidate should produce a coherent kernel input before mutation");

    let sizing_view_mismatch = RiskReservationTransaction {
        candidate: candidate.clone(),
        kernel_input: kernel_input.clone(),
        sizing_view: {
            let mut sizing_view = view.sizing_view().clone();
            sizing_view.risk_state_version = RiskStateVersion::new(7);
            sizing_view
        },
        safety_state: unlatched_safety(RiskStateVersion::zero()),
        caller_diagnostics: None,
        now_unix_nanos: 1_010,
    };
    assert_eq!(
        sizing_view_mismatch.validate_static(
            &PoolId::new("pool-static-version-reporting").expect("pool id should be valid"),
            RiskStateVersion::zero(),
        ),
        Err(RiskReservationError::StaleRiskStateVersion {
            expected: RiskStateVersion::zero(),
            actual: RiskStateVersion::new(7),
        }),
        "stale sizing views must report the stale sizing-view version"
    );

    let kernel_input_mismatch = RiskReservationTransaction {
        candidate,
        kernel_input: RiskKernelInput {
            risk_state_version: RiskStateVersion::new(8),
            ..kernel_input
        },
        sizing_view: view.sizing_view().clone(),
        safety_state: unlatched_safety(RiskStateVersion::zero()),
        caller_diagnostics: None,
        now_unix_nanos: 1_010,
    };
    assert_eq!(
        kernel_input_mismatch.validate_static(
            &PoolId::new("pool-static-version-reporting").expect("pool id should be valid"),
            RiskStateVersion::zero(),
        ),
        Err(RiskReservationError::StaleRiskStateVersion {
            expected: RiskStateVersion::zero(),
            actual: RiskStateVersion::new(8),
        }),
        "stale kernel inputs must report the stale kernel-input version"
    );
}

#[test]
fn s6a_no_active_policy_epoch_rejects_risk_increasing_admission_without_mutation() {
    let (service, owner, _store) =
        risk_context_without_policy_epoch("pool-no-active-epoch", "owner-no-active-epoch");
    owner
        .reconcile_before_new_risk()
        .expect("owner should be reconciled so policy epoch validation is reached");
    let view = published_view(
        RiskStateVersion::zero(),
        "pool-no-active-epoch",
        "candidate-instrument",
        bucket("risk_class", "alpha"),
        dec(100),
        dec(100),
    );
    let before_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable before rejected reserve");
    assert_eq!(before_snapshot.risk_state_version, RiskStateVersion::zero());
    assert!(
        before_snapshot.active_epoch.is_none(),
        "test setup must not bind a policy epoch"
    );

    let error = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-no-active-epoch",
                "idempotency-no-active-epoch",
                "pool-no-active-epoch",
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(4),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect_err("risk-increasing admission must fail closed without an active policy epoch");

    assert_eq!(error, AdmissionReserveError::NoActivePolicyEpoch);
    assert_no_reservation_effect(&service, &owner, before_snapshot.risk_state_version);
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
fn s3_corrected_descriptor_activation_clears_unknown_state_halt() {
    let bucket = bucket("risk_class", "alpha");
    let mut registry = InstrumentRiskRegistry::new();
    let descriptor_v1 = descriptor(
        "candidate-instrument",
        "descriptor-version-v1",
        "policy-epoch",
        &bucket,
        0,
    );
    let expected_envelope = descriptor_v1.unknown_state_envelope.clone();
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
        .expect("v1 descriptor should activate");

    assert_eq!(
        registry
            .observe_terminal_state("candidate-instrument", "policy-epoch", "recovered-terminal")
            .expect("unknown state should halt admission under the active descriptor"),
        TerminalStateObservation::Unknown(expected_envelope)
    );

    let descriptor_v2 = InstrumentRiskDescriptor::new(
        "candidate-instrument".to_string(),
        "descriptor-version-v2".to_string(),
        "policy-epoch".to_string(),
        vec![
            "terminal-loss".to_string(),
            "terminal-gain".to_string(),
            "recovered-terminal".to_string(),
        ],
        vec![dec(0), dec(99), dec(1)],
        DescriptorTerminalStateEnvelope {
            terminal_state_id: "unknown-terminal-envelope-v2".to_string(),
            terminal_cash_flow: dec(0),
        },
        RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
            "descriptor_risk_class".to_string(),
            bucket.bucket_value().to_string(),
        )]))
        .expect("descriptor attributes should classify"),
    )
    .expect("corrected descriptor fixture should be valid");
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
        .expect("corrected descriptor should activate");

    let certified = registry
        .resolve_active_descriptor("candidate-instrument", "policy-epoch")
        .expect("corrected descriptor should be active");
    let view = published_view_from_certified(
        RiskStateVersion::zero(),
        "pool-halt-recovery",
        bucket,
        dec(20),
        dec(20),
        certified,
    );
    let mut candidate = admission_candidate(
        "intent-halt-recovery",
        "idempotency-halt-recovery",
        "pool-halt-recovery",
        "candidate-instrument",
        RiskStateVersion::zero(),
        dec(3),
    );
    candidate.expected_descriptor_version = "descriptor-version-v2".to_string();

    assert_eq!(
        registry.validate_admission_binding(
            view.active_descriptor(),
            &candidate,
            "admission-owner",
        ),
        Ok(()),
        "a corrected active descriptor must clear the stale unknown-state halt"
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
                settlement_reports: Vec::new(),
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
                    status: NtOrderStatusTruth::Open,
                    event_id: "nt-order-status-event".to_string(),
                    ts_event_unix_nanos: 1_150,
                    event_sequence: Some(1),
                    ts_init_unix_nanos: 1_151,
                }],
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
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
fn s4_restart_reconciliation_treats_fill_report_as_authoritative_venue_presence() {
    let pool_id = "pool-fill-only-restart";
    let idempotency_key = "idempotency-fill-only-restart";
    let (_reservation, _owner, store, client_order_id) = submitted_reservation_with_store(
        pool_id,
        "owner-before-fill-only-restart",
        "intent-fill-only-restart",
        idempotency_key,
        "S4-FILL-ONLY-RESTART",
    );

    let successor = RiskStateOwner::acquire(
        store,
        PoolId::new(pool_id).expect("pool id should be valid"),
        "owner-after-fill-only-restart",
    )
    .expect("successor owner should acquire the pool");
    let reconciler = LifecycleReconciler::new(successor.clone());
    let mut sink = RecordingLiveSubmitBoundary::default();

    let summary = reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: vec![nt_fill(
                    client_order_id,
                    "s4-fill-only-restart-fill",
                    dec(2),
                    dec(0),
                    dec(24),
                    dec(26),
                    vec![dec(1), dec(99)],
                )],
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("fill truth alone proves the order already reached the venue");

    assert!(
        sink.submitted_client_order_ids().is_empty(),
        "restart must not re-submit when the ClientOrderId appears only in fill truth"
    );
    assert_eq!(summary.live_order_count, 1);
    assert_eq!(
        successor
            .live_submission_record(idempotency_key)
            .expect("live submission lookup should be readable")
            .expect("fill-only truth should register a live submission")
            .client_order_id,
        client_order_id
    );
}

#[test]
fn s4_restart_reconciliation_releases_reserved_orphan_and_prunes_idempotency() {
    let pool_id = "pool-restart-orphan";
    let (service, owner, store) = reconciled_risk_context(pool_id, "owner-before-orphan");
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        pool_id,
        "candidate-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-restart-orphan",
                "idempotency-restart-orphan",
                pool_id,
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("unsubmitted reservation should reserve risk");
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable before restart")
            .open_order_count(),
        1
    );

    let restarted_owner = RiskStateOwner::acquire(
        store,
        PoolId::new(pool_id).expect("pool id should be valid"),
        "owner-after-orphan",
    )
    .expect("successor owner should acquire the pool");
    let reconciler = LifecycleReconciler::new(restarted_owner.clone());
    let mut sink = RecordingLiveSubmitBoundary::default();

    reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("restart reconciliation should release an unsubmitted orphan");

    assert_eq!(
        restarted_owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after restart")
            .open_order_count(),
        0,
        "unsubmitted predecessor orphans have no venue release path and must be released"
    );
    assert!(
        restarted_owner
            .reservation_records()
            .expect("reservation records should be readable after restart")
            .iter()
            .all(
                |record| record.admission_token != reservation.admission_token
                    && record.lifecycle_state != ReservationLifecycleState::Reserved
            ),
        "released orphan records must not remain as Reserved ledger entries"
    );

    let retry_version = restarted_owner
        .policy_epoch_snapshot()
        .expect("policy snapshot should expose post-reconcile version")
        .risk_state_version;
    let retry_view = published_view(
        retry_version,
        pool_id,
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
    );
    let retry = AdmissionService::new(restarted_owner.clone())
        .compare_and_reserve(
            &retry_view,
            admission_candidate(
                "intent-restart-orphan",
                "idempotency-restart-orphan",
                pool_id,
                "candidate-instrument",
                retry_version,
                dec(2),
            ),
            unlatched_safety(retry_version),
            None,
            1_210,
        )
        .expect("released orphan idempotency and permit keys should be reusable");
    assert_ne!(
        retry.admission_token, reservation.admission_token,
        "retry after orphan release must mint a coherent new reservation record"
    );
    assert_eq!(
        restarted_owner
            .reservation_records()
            .expect("reservation records should be readable after retry")
            .len(),
        1
    );
}

#[test]
fn s4_direct_reconcile_requires_restart_truth_for_live_durable_intents_and_logs_orphan_release() {
    let submitted_pool_id = "pool-direct-reconcile-submitted";
    let submitted_idempotency_key = "idempotency-direct-reconcile-submitted";
    let (_reservation, submitted_owner, submitted_store, submitted_client_order_id) =
        submitted_reservation_with_store(
            submitted_pool_id,
            "owner-before-direct-reconcile-submitted",
            "intent-direct-reconcile-submitted",
            submitted_idempotency_key,
            "S4-DIRECT-RECONCILE-SUBMITTED",
        );
    record_live_submission_for_test(
        &submitted_owner,
        submitted_idempotency_key,
        submitted_client_order_id,
    );
    let submitted_successor = RiskStateOwner::acquire(
        submitted_store,
        PoolId::new(submitted_pool_id).expect("pool id should be valid"),
        "owner-after-direct-reconcile-submitted",
    )
    .expect("successor owner should acquire the submitted pool");

    let submitted_direct_reconcile = submitted_successor.reconcile_before_new_risk();

    let orphan_pool_id = "pool-direct-reconcile-orphan";
    let (service, orphan_owner, orphan_store) =
        reconciled_risk_context(orphan_pool_id, "owner-before-direct-reconcile-orphan");
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        orphan_pool_id,
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
    );
    service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-direct-reconcile-orphan",
                "idempotency-direct-reconcile-orphan",
                orphan_pool_id,
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("unsubmitted reservation should reserve risk");
    assert_eq!(
        orphan_owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable before orphan handoff")
            .open_order_count(),
        1
    );

    let orphan_successor = RiskStateOwner::acquire(
        orphan_store,
        PoolId::new(orphan_pool_id).expect("pool id should be valid"),
        "owner-after-direct-reconcile-orphan",
    )
    .expect("successor owner should acquire the orphan pool");
    let before_version = orphan_successor
        .policy_epoch_snapshot()
        .expect("policy snapshot should expose pre-reconcile version")
        .risk_state_version;
    let before_mutation_count = orphan_successor
        .durable_mutation_records()
        .expect("durable records should be readable before direct reconcile")
        .len();

    let reconciled_version = orphan_successor
        .reconcile_before_new_risk()
        .expect("direct reconciliation may release a pool with only unsubmitted orphans");
    let records = orphan_successor
        .durable_mutation_records()
        .expect("durable records should be readable after direct reconcile");
    let submitted_requires_truth_replay =
        submitted_direct_reconcile == Err(RiskStateMutationError::ReconciliationRequired);
    let orphan_version_advanced = reconciled_version > before_version;
    let orphan_reconciliation_logged = records.len() == before_mutation_count + 1
        && records
            .last()
            .is_some_and(|record| record.mutation.kind() == RiskMutationKind::Reconciliation);

    assert!(
        submitted_requires_truth_replay && orphan_version_advanced && orphan_reconciliation_logged,
        "direct reconcile result = {submitted_direct_reconcile:?}; orphan version before = {before_version:?}, after = {reconciled_version:?}; mutation count before = {before_mutation_count}, after = {}; last mutation kind = {:?}",
        records.len(),
        records.last().map(|record| record.mutation.kind())
    );
    assert_eq!(
        orphan_successor
            .reserved_risk_totals()
            .expect("reserved totals should be readable after direct reconcile")
            .open_order_count(),
        0
    );
}

#[test]
fn s4_successor_reconciliation_fails_closed_on_dangling_submission_intent() {
    let pool_id = "pool-dangling-intent";
    let (service, owner, store) = reconciled_risk_context(pool_id, "owner-before-dangling");
    let authority = SubmissionAuthority::new(owner);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        pool_id,
        "candidate-instrument",
        bucket,
        dec(20),
        dec(20),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                "intent-dangling",
                "idempotency-dangling",
                pool_id,
                "candidate-instrument",
                RiskStateVersion::zero(),
                dec(2),
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("reservation should issue an admission token");
    let client_order_id = client_order_id("S4-DANGLING-INTENT");
    authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("crash point: durable intent exists before live submission record");

    let successor = RiskStateOwner::acquire(
        store,
        PoolId::new(pool_id).expect("pool id should be valid"),
        "owner-after-dangling",
    )
    .expect("successor owner should acquire the pool");
    assert_eq!(
        successor.reconcile_before_new_risk(),
        Err(RiskStateMutationError::ReconciliationRequired),
        "a submitted durable intent without a live submission record must not reopen new risk"
    );

    let reconciler = LifecycleReconciler::new(successor.clone());
    let mut sink = RecordingLiveSubmitBoundary::default();
    reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect(
            "restart reconciliation should redrive the dangling durable intent before reopening",
        );
    assert_eq!(sink.submitted_client_order_ids(), vec![client_order_id]);
    assert_eq!(
        successor
            .live_submission_records()
            .expect("live submission records should be readable")
            .len(),
        1,
        "restart redrive must leave a live submission record for future venue release"
    );
}

#[test]
fn s8a_partial_fill_moves_quantity_and_keeps_reserved_risk_monotonic() {
    let (reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8a-partial",
        "owner-s8a-partial",
        "intent-s8a-partial",
        "idempotency-s8a-partial",
        "S8A-PARTIAL-ORDER",
    );

    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8a-open-status"))
        .expect("authoritative NT open status should move the reservation to Open");
    let pre_fill_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before fill");
    let pre_fill_version = owner
        .policy_epoch_snapshot()
        .expect("policy snapshot should expose the current risk version")
        .risk_state_version;

    let fill = nt_fill(
        client_order_id,
        "s8a-partial-fill",
        dec(1),
        dec(1),
        dec(24),
        dec(26),
        vec![dec(1), dec(99)],
    );
    let summary = reconciler
        .apply_fill_truth(fill)
        .expect("authoritative NT fill truth should be accepted");

    assert_eq!(
        summary.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );
    assert_eq!(
        summary.risk_state_version,
        pre_fill_version
            .next()
            .expect("test version should advance once"),
        "one authoritative fill event must advance the coherent risk version exactly once"
    );
    let post_fill_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after fill");
    assert!(
        post_fill_totals.equity_floor_stress_loss() >= pre_fill_totals.equity_floor_stress_loss(),
        "equity-floor reserved risk must not fall merely because an order partially filled"
    );
    assert!(
        post_fill_totals.governor_realized_loss() >= pre_fill_totals.governor_realized_loss(),
        "governor reserved risk must not fall merely because an order partially filled"
    );

    let record = only_reservation_record(&owner);
    assert_eq!(
        record.admission_token, reservation.admission_token,
        "the existing reservation ledger record must remain the state owner"
    );
    assert_eq!(
        record.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );
    assert_eq!(record.remaining_fillable_quantity, dec(1));
    assert_eq!(
        record
            .filled_position_exposure
            .expect("filled quantity should be represented as position exposure")
            .quantity,
        dec(1)
    );
    assert_eq!(
        record.assessment.equity_floor_stress_loss,
        pre_fill_totals.equity_floor_stress_loss(),
        "the original conservative open-order reservation remains active for the remainder"
    );
}

#[test]
fn s8a_local_intent_without_authoritative_fill_truth_does_not_transition_or_revalue() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8a-local-intent",
        "owner-s8a-local-intent",
        "intent-s8a-local-intent",
        "idempotency-s8a-local-intent",
        "S8A-LOCAL-INTENT-ORDER",
    );

    let pre_reconcile_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before reconciliation");
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8a-local-open-status"))
        .expect("local submit intent plus NT open truth may only move to Open");
    let record = only_reservation_record(&owner);

    assert_eq!(record.lifecycle_state, ReservationLifecycleState::Open);
    assert!(
        record.filled_position_exposure.is_none(),
        "a local fill belief without NtFillReportTruth must not create filled exposure"
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should remain readable"),
        pre_reconcile_totals,
        "local intent cannot revalue or release reserved risk without authoritative fill truth"
    );
}

#[test]
fn s8a_settlement_revision_recomputes_without_releasing_reserved_risk() {
    let (_reservation, owner, reconciler, client_order_id) = filled_reservation_context(
        "pool-s8a-settlement-revision",
        "owner-s8a-settlement-revision",
        "intent-s8a-settlement-revision",
        "idempotency-s8a-settlement-revision",
        "S8A-SETTLEMENT-REVISION-ORDER",
    );
    let pre_revision_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before settlement revision");

    let revision = nt_settlement(
        client_order_id,
        "s8a-settlement-revision",
        false,
        true,
        dec(28),
        dec(30),
        vec![dec(-2), dec(99)],
    );
    let summary = reconciler
        .apply_settlement_truth(revision)
        .expect("authoritative settlement revision should recompute filled exposure");

    assert_eq!(summary.lifecycle_state, ReservationLifecycleState::Filled);
    let post_revision_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after settlement revision");
    assert!(
        post_revision_totals.equity_floor_stress_loss()
            >= pre_revision_totals.equity_floor_stress_loss(),
        "a settlement revision must not release equity-floor reservation before terminal finality"
    );
    assert!(
        post_revision_totals.governor_realized_loss()
            >= pre_revision_totals.governor_realized_loss(),
        "a settlement revision must not release governor reservation before terminal finality"
    );
    let record = only_reservation_record(&owner);
    let exposure = record
        .filled_position_exposure
        .expect("settlement revision should leave filled exposure active");
    assert_eq!(exposure.conservative_liquidation_value, dec(28));
    assert_eq!(exposure.governor_cost_basis, dec(30));
    assert_eq!(exposure.terminal_cash_flows, vec![dec(-2), dec(99)]);
}

#[test]
fn s8a_full_lifecycle_reaches_settled_only_after_terminal_final_reconciled_truth() {
    let (reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8a-happy-path",
        "owner-s8a-happy-path",
        "intent-s8a-happy-path",
        "idempotency-s8a-happy-path",
        "S8A-HAPPY-PATH-ORDER",
    );
    assert_eq!(
        only_reservation_record(&owner).lifecycle_state,
        ReservationLifecycleState::Submitted
    );

    let open = reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8a-happy-open"))
        .expect("authoritative NT open status should be accepted");
    assert_eq!(open.lifecycle_state, ReservationLifecycleState::Open);

    let partial = reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8a-happy-partial",
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("authoritative partial fill should be accepted");
    assert_eq!(
        partial.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );

    let filled = reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8a-happy-final-fill",
            dec(1),
            dec(0),
            dec(25),
            dec(27),
            vec![dec(1), dec(99)],
        ))
        .expect("authoritative final fill should be accepted");
    assert_eq!(filled.lifecycle_state, ReservationLifecycleState::Filled);

    let not_reconciled = reconciler
        .apply_settlement_truth(nt_settlement(
            client_order_id,
            "s8a-happy-final-not-reconciled",
            true,
            false,
            dec(25),
            dec(27),
            vec![dec(1), dec(99)],
        ))
        .expect("terminal truth without reconciliation must remain filled");
    assert_eq!(
        not_reconciled.lifecycle_state,
        ReservationLifecycleState::Filled
    );

    let settled = reconciler
        .apply_settlement_truth(nt_settlement(
            client_order_id,
            "s8a-happy-settled",
            true,
            true,
            dec(25),
            dec(27),
            vec![dec(1), dec(99)],
        ))
        .expect("terminal-final and reconciled settlement truth should settle the reservation");
    assert_eq!(settled.lifecycle_state, ReservationLifecycleState::Settled);
    assert_eq!(
        only_reservation_record(&owner).admission_token,
        reservation.admission_token,
        "the full lifecycle must advance the original reservation record, not a parallel model"
    );
}

#[test]
fn s8b_local_cancel_timeout_and_expiry_mark_intent_without_releasing_reserved_risk() {
    for (case_name, mutation_id, client_order_id_value) in [
        (
            "cancel-request",
            "s8b-local-cancel-request",
            "S8B-LOCAL-CANCEL-REQUEST",
        ),
        (
            "cancel-timeout",
            "s8b-local-cancel-timeout",
            "S8B-LOCAL-CANCEL-TIMEOUT",
        ),
        ("local-expiry", "s8b-local-expiry", "S8B-LOCAL-EXPIRY"),
    ] {
        let pool_id = format!("pool-s8b-local-{case_name}");
        let owner_id = format!("owner-s8b-local-{case_name}");
        let intent_id = format!("intent-s8b-local-{case_name}");
        let idempotency_key = format!("idempotency-s8b-local-{case_name}");
        let (_reservation, owner, _reconciler, client_order_id) = submitted_reservation_context(
            &pool_id,
            &owner_id,
            &intent_id,
            &idempotency_key,
            client_order_id_value,
        );
        let pre_cancel_totals = owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable before local cancel intent");

        let summary = owner
            .mark_cancel_requested(client_order_id, mutation_id)
            .expect("local cancel-like intent should mark CancelRequested");

        assert_eq!(
            summary.lifecycle_state,
            ReservationLifecycleState::CancelRequested
        );
        assert_eq!(
            owner
                .reserved_risk_totals()
                .expect("reserved totals should be readable after local cancel intent"),
            pre_cancel_totals,
            "local {case_name} must not release any reserved risk"
        );
        let record = only_reservation_record(&owner);
        assert_eq!(
            record.lifecycle_state,
            ReservationLifecycleState::CancelRequested
        );
        assert_eq!(record.remaining_fillable_quantity, dec(2));
        assert!(
            record.filled_position_exposure.is_none(),
            "local cancel intent alone must not synthesize a fill"
        );
    }
}

#[test]
fn s8b_cancel_confirmed_releases_open_remainder_but_retains_filled_position() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8b-cancel-confirmed",
        "owner-s8b-cancel-confirmed",
        "intent-s8b-cancel-confirmed",
        "idempotency-s8b-cancel-confirmed",
        "S8B-CANCEL-CONFIRMED",
    );
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8b-cancel-open"))
        .expect("authoritative open status should move the order to Open");
    reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8b-cancel-partial-fill",
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("partial fill should establish filled-position reservation");
    let pre_confirm_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before cancel confirmation");

    let summary = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status(
            client_order_id,
            "s8b-cancel-confirmed",
        ))
        .expect("authoritative cancel confirmation should release the open remainder");

    assert_eq!(
        summary.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    let record = only_reservation_record(&owner);
    assert_eq!(
        record.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    assert_eq!(record.remaining_fillable_quantity, Decimal::ZERO);
    let exposure = record
        .filled_position_exposure
        .expect("filled-position exposure must stay active after cancel confirmation");
    assert_eq!(exposure.quantity, dec(1));
    let post_confirm_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after cancel confirmation");
    assert!(
        post_confirm_totals.equity_floor_stress_loss()
            < pre_confirm_totals.equity_floor_stress_loss(),
        "cancel confirmation must remove the open-order assessment"
    );
    assert_eq!(
        post_confirm_totals.equity_floor_stress_loss(),
        record.filled_position_equity_floor_stress_loss
    );
    assert_eq!(
        post_confirm_totals.governor_realized_loss(),
        record.filled_position_governor_realized_loss
    );
    assert_eq!(post_confirm_totals.collateral_required(), Decimal::ZERO);
    assert_eq!(post_confirm_totals.open_order_count(), 0);
    assert_eq!(post_confirm_totals.position_quantity(), exposure.quantity);
}

#[test]
fn s8b_cancel_confirmed_filled_position_settlement_releases_all_reserved_components() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8b-cancel-settlement",
        "owner-s8b-cancel-settlement",
        "intent-s8b-cancel-settlement",
        "idempotency-s8b-cancel-settlement",
        "S8B-CANCEL-SETTLEMENT",
    );
    reconciler
        .apply_order_status_truth(nt_open_status(
            client_order_id,
            "s8b-cancel-settlement-open",
        ))
        .expect("authoritative open status should move the order to Open");
    reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8b-cancel-settlement-partial-fill",
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("partial fill should establish filled-position reservation");
    reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status(
            client_order_id,
            "s8b-cancel-settlement-confirmed",
        ))
        .expect("cancel confirmation should release only the open remainder");

    let settled = reconciler
        .apply_settlement_truth(nt_settlement(
            client_order_id,
            "s8b-cancel-settlement-final",
            true,
            true,
            dec(28),
            dec(30),
            vec![dec(-2), dec(99)],
        ))
        .expect("settlement truth must release the filled component after terminal cancel");

    assert_eq!(settled.lifecycle_state, ReservationLifecycleState::Settled);
    let totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after filled-position settlement");
    assert_eq!(totals.equity_floor_stress_loss(), Decimal::ZERO);
    assert_eq!(totals.governor_realized_loss(), Decimal::ZERO);
    assert_eq!(totals.position_quantity(), Decimal::ZERO);
    assert_eq!(totals.collateral_required(), Decimal::ZERO);
    assert_eq!(totals.open_order_count(), 0);
}

#[test]
fn s8b_late_fill_after_cancel_requested_still_applies_under_existing_reservation() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8b-late-fill",
        "owner-s8b-late-fill",
        "intent-s8b-late-fill",
        "idempotency-s8b-late-fill",
        "S8B-LATE-FILL",
    );
    let pre_cancel_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before cancel request");
    owner
        .mark_cancel_requested(client_order_id, "s8b-late-fill-cancel-requested")
        .expect("local cancel request should only mark intent");
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after cancel request"),
        pre_cancel_totals,
        "CancelRequested must keep the original reservation covering late fills"
    );

    let summary = reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8b-late-fill-after-cancel",
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("authoritative late fill after CancelRequested should apply");

    assert_eq!(
        summary.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );
    let record = only_reservation_record(&owner);
    assert_eq!(
        record.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );
    assert_eq!(record.remaining_fillable_quantity, dec(1));
    assert_eq!(
        record
            .filled_position_exposure
            .expect("late fill should create filled exposure")
            .quantity,
        dec(1)
    );
    let post_fill_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after late fill");
    assert!(
        post_fill_totals.equity_floor_stress_loss() >= pre_cancel_totals.equity_floor_stress_loss(),
        "late fill risk must be added while the original open reservation remains active"
    );
    assert_eq!(
        post_fill_totals.open_order_count(),
        pre_cancel_totals.open_order_count()
    );
}

#[test]
fn s8b_expired_confirmed_releases_unfilled_open_reservation() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8b-expired-confirmed",
        "owner-s8b-expired-confirmed",
        "intent-s8b-expired-confirmed",
        "idempotency-s8b-expired-confirmed",
        "S8B-EXPIRED-CONFIRMED",
    );
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8b-expired-open"))
        .expect("authoritative open status should move the order to Open");
    let pre_confirm_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before expiry confirmation");
    assert!(
        pre_confirm_totals.equity_floor_stress_loss() > Decimal::ZERO,
        "test setup must reserve open-order risk before expiry confirmation"
    );

    let summary = reconciler
        .apply_order_status_truth(nt_expired_confirmed_status(
            client_order_id,
            "s8b-expired-confirmed",
        ))
        .expect("authoritative expiry confirmation should release unfilled open reservation");

    assert_eq!(
        summary.lifecycle_state,
        ReservationLifecycleState::ExpiredConfirmed
    );
    let record = only_reservation_record(&owner);
    assert_eq!(
        record.lifecycle_state,
        ReservationLifecycleState::ExpiredConfirmed
    );
    assert_eq!(record.remaining_fillable_quantity, Decimal::ZERO);
    assert!(
        record.filled_position_exposure.is_none(),
        "unfilled expiry confirmation must not synthesize a filled position"
    );
    let post_confirm_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after expiry confirmation");
    assert_eq!(post_confirm_totals.collateral_required(), Decimal::ZERO);
    assert_eq!(
        post_confirm_totals.equity_floor_stress_loss(),
        Decimal::ZERO
    );
    assert_eq!(post_confirm_totals.governor_realized_loss(), Decimal::ZERO);
    assert_eq!(post_confirm_totals.open_order_count(), 0);
    assert_eq!(post_confirm_totals.position_quantity(), Decimal::ZERO);
}

#[test]
fn s8b_replace_keeps_old_and_new_reserved_until_old_is_confirmed_non_fillable() {
    let pool_id = "pool-s8b-replace";
    let (service, owner, _store) = reconciled_risk_context(pool_id, "owner-s8b-replace");
    let authority = SubmissionAuthority::new(owner.clone());
    let bucket = bucket("risk_class", "alpha");
    let old_view = published_view_with_open_order_headroom(
        RiskStateVersion::zero(),
        pool_id,
        "candidate-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
        2,
    );
    let old_reservation = service
        .compare_and_reserve(
            &old_view,
            admission_candidate_from_preview(
                "intent-s8b-replace-old",
                "idempotency-s8b-replace-old",
                RiskPreviewInput {
                    pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
                    instrument_id: "candidate-instrument".to_string(),
                    model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
                        instrument_id: "candidate-instrument".to_string(),
                    },
                    side: "long".to_string(),
                    quantity: dec(2),
                    order_type: "limit".to_string(),
                    time_in_force: "gtc".to_string(),
                    max_unit_price: Some(dec(20)),
                    max_cash_outlay: dec(20),
                    source_view_version: RiskStateVersion::zero(),
                    policy_epoch_id: "policy-epoch".to_string(),
                },
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("old reservation should be accepted");
    let old_client_order_id = client_order_id("S8B-REPLACE-OLD");
    authority
        .prepare_admitted_order(&old_reservation, old_client_order_id, 1_100)
        .expect("old reservation should move to Submitted");
    let reconciler = LifecycleReconciler::new(owner.clone());
    reconciler
        .apply_order_status_truth(nt_open_status(old_client_order_id, "s8b-replace-old-open"))
        .expect("old order should be open before replacement is reserved");
    let old_only_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before replacement");
    let replacement_view_version = owner
        .policy_epoch_snapshot()
        .expect("policy snapshot should expose replacement source version")
        .risk_state_version;
    let replacement_view = published_view_with_open_order_headroom(
        replacement_view_version,
        pool_id,
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
        2,
    );
    let replacement = service
        .compare_and_reserve(
            &replacement_view,
            admission_candidate_with_permit(
                "intent-s8b-replace-new",
                "idempotency-s8b-replace-new",
                pool_id,
                "candidate-instrument",
                replacement_view_version,
                dec(20),
                SizingDecisionPermit {
                    permit_id: "permit-s8b-replace-new".to_string(),
                    source_view_version: replacement_view_version,
                    candidate_digest: "candidate-digest-s8b-replace-new".to_string(),
                },
            ),
            unlatched_safety(replacement_view_version),
            None,
            1_200,
        )
        .expect("replacement reservation should be accepted while old remains live");
    let replacement_client_order_id = client_order_id("S8B-REPLACE-NEW");
    authority
        .prepare_admitted_order(&replacement, replacement_client_order_id, 1_210)
        .expect("replacement reservation should move to Submitted");
    let combined_totals = owner
        .reserved_risk_totals()
        .expect("combined totals should be readable after replacement reserve");

    assert_eq!(combined_totals.open_order_count(), 2);
    assert!(
        combined_totals.equity_floor_stress_loss() > old_only_totals.equity_floor_stress_loss(),
        "old and replacement exposure must both be reserved before old non-fillability"
    );

    let summary = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status(
            old_client_order_id,
            "s8b-replace-old-cancel-confirmed",
        ))
        .expect("old non-fillability confirmation should release only the old open reservation");

    assert_eq!(
        summary.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    let old_record = reservation_record_for_commit(&owner, &old_reservation);
    let replacement_record = reservation_record_for_commit(&owner, &replacement);
    assert_eq!(
        old_record.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    assert_eq!(
        replacement_record.lifecycle_state,
        ReservationLifecycleState::Submitted
    );
    let post_confirm_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after old cancel confirmation");
    assert_eq!(post_confirm_totals.open_order_count(), 1);
    assert_eq!(
        post_confirm_totals.equity_floor_stress_loss(),
        replacement_record.assessment.equity_floor_stress_loss
    );
    assert_eq!(
        post_confirm_totals.governor_realized_loss(),
        replacement_record.assessment.governor_realized_loss
    );
    assert_eq!(
        post_confirm_totals.collateral_required(),
        replacement_record.assessment.collateral_required
    );
    assert_eq!(
        post_confirm_totals.position_quantity(),
        replacement_record.reserved_order_quantity
    );
}

#[test]
fn s8c_duplicate_fill_event_id_is_idempotent() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8c-duplicate-fill",
        "owner-s8c-duplicate-fill",
        "intent-s8c-duplicate-fill",
        "idempotency-s8c-duplicate-fill",
        "S8C-DUPLICATE-FILL",
    );
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8c-duplicate-fill-open"))
        .expect("authoritative open status should establish ordering baseline");
    let fill = nt_fill(
        client_order_id,
        "s8c-fill-event",
        dec(1),
        dec(1),
        dec(24),
        dec(26),
        vec![dec(1), dec(99)],
    );
    let once = reconciler
        .apply_fill_truth(fill.clone())
        .expect("first authoritative fill should apply");
    let record_after_once = only_reservation_record(&owner);
    let totals_after_once = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after first fill");

    let duplicate = reconciler
        .apply_fill_truth(fill)
        .expect("duplicate authoritative fill event_id should be a no-op");

    assert_eq!(duplicate, once);
    assert_eq!(
        only_reservation_record(&owner),
        record_after_once,
        "duplicate fill event_id must not reapply quantity or lifecycle state"
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after duplicate fill"),
        totals_after_once,
        "duplicate fill event_id must not double-count filled-position risk"
    );
    assert_eq!(
        owner
            .policy_epoch_snapshot()
            .expect("policy snapshot should expose duplicate-fill version")
            .risk_state_version,
        once.risk_state_version,
        "duplicate fill event_id must not bump the coherent risk version"
    );
}

#[test]
fn s8c_duplicate_settlement_and_status_event_ids_are_idempotent() {
    let (_reservation, owner, reconciler, client_order_id) = filled_reservation_context(
        "pool-s8c-duplicate-settlement-status",
        "owner-s8c-duplicate-settlement-status",
        "intent-s8c-duplicate-settlement-status",
        "idempotency-s8c-duplicate-settlement-status",
        "S8C-DUPLICATE-SETTLEMENT-STATUS",
    );
    let settlement = nt_settlement(
        client_order_id,
        "s8c-settlement-event",
        false,
        true,
        dec(28),
        dec(30),
        vec![dec(-2), dec(99)],
    );
    let settlement_once = reconciler
        .apply_settlement_truth(settlement.clone())
        .expect("first settlement revision should apply");
    let record_after_settlement_once = only_reservation_record(&owner);
    let totals_after_settlement_once = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after first settlement");

    let settlement_duplicate = reconciler
        .apply_settlement_truth(settlement)
        .expect("duplicate settlement event_id should be a no-op");

    assert_eq!(settlement_duplicate, settlement_once);
    assert_eq!(
        only_reservation_record(&owner),
        record_after_settlement_once
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after duplicate settlement"),
        totals_after_settlement_once
    );
    assert_eq!(
        owner
            .policy_epoch_snapshot()
            .expect("policy snapshot should expose duplicate-settlement version")
            .risk_state_version,
        settlement_once.risk_state_version,
        "duplicate settlement event_id must not bump the coherent risk version"
    );

    let (status_reservation, status_owner, status_reconciler, status_client_order_id) =
        submitted_reservation_context(
            "pool-s8c-duplicate-status",
            "owner-s8c-duplicate-status",
            "intent-s8c-duplicate-status",
            "idempotency-s8c-duplicate-status",
            "S8C-DUPLICATE-STATUS",
        );
    let open_status = nt_open_status(status_client_order_id, "s8c-status-event");
    let status_once = status_reconciler
        .apply_order_status_truth(open_status.clone())
        .expect("first status event should apply");
    let record_after_status_once = only_reservation_record(&status_owner);
    let totals_after_status_once = status_owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable after first status");

    let status_duplicate = status_reconciler
        .apply_order_status_truth(open_status)
        .expect("duplicate status event_id should be a no-op");

    assert_eq!(status_duplicate, status_once);
    assert_eq!(
        only_reservation_record(&status_owner),
        record_after_status_once
    );
    assert_eq!(
        status_owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after duplicate status"),
        totals_after_status_once
    );
    assert_eq!(
        status_owner
            .policy_epoch_snapshot()
            .expect("policy snapshot should expose duplicate-status version")
            .risk_state_version,
        status_once.risk_state_version,
        "duplicate status event_id must not bump the coherent risk version"
    );
    assert_eq!(
        reservation_record_for_commit(&status_owner, &status_reservation).lifecycle_state,
        ReservationLifecycleState::Open
    );
}

#[test]
fn s8c_out_of_order_event_requires_reconciliation_and_blocks_new_risk() {
    let pool_id = "pool-s8c-out-of-order";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-out-of-order",
        "intent-s8c-out-of-order",
        "idempotency-s8c-out-of-order",
        "S8C-OUT-OF-ORDER",
    );
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8c-out-of-order-open"))
        .expect("first status event should apply");
    let before_fault_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before ordering fault");

    let fault = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-out-of-order-cancel",
            1_140,
            Some(2),
        ))
        .expect("older authoritative event should move the reservation to reconciliation");

    assert_eq!(
        fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    let faulted_record = only_reservation_record(&owner);
    assert_eq!(
        faulted_record.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert_eq!(
        faulted_record.remaining_fillable_quantity,
        dec(2),
        "ambiguous out-of-order non-fillability must fail closed without releasing capacity"
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should remain readable after ordering fault"),
        before_fault_totals,
        "out-of-order events must not silently apply lifecycle release"
    );
    assert_new_risk_blocked_by_reconciliation(&owner, pool_id, "s8c-out-of-order-successor");
}

#[test]
fn s8c_sequence_gap_requires_reconciliation_and_blocks_new_risk() {
    let pool_id = "pool-s8c-sequence-gap";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-sequence-gap",
        "intent-s8c-sequence-gap",
        "idempotency-s8c-sequence-gap",
        "S8C-SEQUENCE-GAP",
    );
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-sequence-gap-open",
            1_150,
            Some(1),
        ))
        .expect("first status event should apply");
    let before_fault_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before sequence gap");

    let fault = reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-sequence-gap-fill",
            1_160,
            Some(3),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect(
            "gapped authoritative event sequence should move the reservation to reconciliation",
        );

    assert_eq!(
        fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    let faulted_record = only_reservation_record(&owner);
    assert_eq!(
        faulted_record.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert!(
        faulted_record.filled_position_exposure.is_none(),
        "ambiguous gapped fill must fail closed without creating filled exposure"
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should remain readable after sequence gap"),
        before_fault_totals,
        "gapped events must not silently apply fill deltas"
    );
    assert_new_risk_blocked_by_reconciliation(&owner, pool_id, "s8c-sequence-gap-successor");
}

#[test]
fn s8c_malformed_fill_remainder_faults_without_wiping_sibling_capacity() {
    let pool_id = "pool-s8c-malformed-fill-remainder";
    let (service, owner, _store) =
        reconciled_risk_context(pool_id, "owner-s8c-malformed-fill-remainder");
    let authority = SubmissionAuthority::new(owner.clone());
    let bucket = bucket("risk_class", "alpha");
    let first_view = published_view_with_open_order_headroom(
        RiskStateVersion::zero(),
        pool_id,
        "candidate-instrument",
        bucket.clone(),
        dec(100),
        dec(100),
        3,
    );
    let first = service
        .compare_and_reserve(
            &first_view,
            admission_candidate_from_preview(
                "intent-s8c-malformed-fill-a",
                "idempotency-s8c-malformed-fill-a",
                RiskPreviewInput {
                    pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
                    instrument_id: "candidate-instrument".to_string(),
                    model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
                        instrument_id: "candidate-instrument".to_string(),
                    },
                    side: "long".to_string(),
                    quantity: dec(2),
                    order_type: "limit".to_string(),
                    time_in_force: "gtc".to_string(),
                    max_unit_price: Some(dec(20)),
                    max_cash_outlay: dec(20),
                    source_view_version: RiskStateVersion::zero(),
                    policy_epoch_id: "policy-epoch".to_string(),
                },
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("first reservation should be accepted");
    let first_client_order_id = client_order_id("S8C-MALFORMED-FILL-A");
    authority
        .prepare_admitted_order(&first, first_client_order_id, 1_100)
        .expect("first reservation should move to Submitted");

    let second_view_version = owner
        .policy_epoch_snapshot()
        .expect("policy snapshot should expose second source version")
        .risk_state_version;
    let second_view = published_view_with_open_order_headroom(
        second_view_version,
        pool_id,
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
        3,
    );
    let second = service
        .compare_and_reserve(
            &second_view,
            admission_candidate_from_preview(
                "intent-s8c-malformed-fill-b",
                "idempotency-s8c-malformed-fill-b",
                RiskPreviewInput {
                    pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
                    instrument_id: "candidate-instrument".to_string(),
                    model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
                        instrument_id: "candidate-instrument".to_string(),
                    },
                    side: "long".to_string(),
                    quantity: dec(2),
                    order_type: "limit".to_string(),
                    time_in_force: "gtc".to_string(),
                    max_unit_price: Some(dec(20)),
                    max_cash_outlay: dec(20),
                    source_view_version: second_view_version,
                    policy_epoch_id: "policy-epoch".to_string(),
                },
            ),
            unlatched_safety(second_view_version),
            None,
            1_200,
        )
        .expect("sibling reservation should be accepted");
    let second_client_order_id = client_order_id("S8C-MALFORMED-FILL-B");
    authority
        .prepare_admitted_order(&second, second_client_order_id, 1_210)
        .expect("sibling reservation should move to Submitted");

    let reconciler = LifecycleReconciler::new(owner.clone());
    reconciler
        .apply_order_status_truth(nt_open_status(
            first_client_order_id,
            "s8c-malformed-fill-a-open",
        ))
        .expect("first order should become Open before malformed fill");
    reconciler
        .apply_order_status_truth(nt_open_status(
            second_client_order_id,
            "s8c-malformed-fill-b-open",
        ))
        .expect("sibling order should become Open before malformed fill");
    let before_fault_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before malformed fill");

    let malformed = reconciler
        .apply_fill_truth(nt_fill(
            first_client_order_id,
            "s8c-malformed-fill-over-remaining",
            dec(1),
            dec(100),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("malformed fill should be absorbed as a reconciliation fault");

    assert_eq!(
        malformed.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    let first_record = reservation_record_for_commit(&owner, &first);
    assert_eq!(
        first_record.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert_eq!(
        first_record.remaining_fillable_quantity,
        dec(2),
        "malformed fill must not overwrite the authoritative remaining quantity"
    );
    assert!(
        first_record.filled_position_exposure.is_none(),
        "malformed fill must not synthesize filled-position exposure"
    );
    let second_record = reservation_record_for_commit(&owner, &second);
    assert_eq!(
        second_record.lifecycle_state,
        ReservationLifecycleState::Open
    );
    assert_eq!(second_record.remaining_fillable_quantity, dec(2));
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should remain readable after malformed fill"),
        before_fault_totals,
        "malformed fill must not mutate pool totals or wipe sibling capacity"
    );
    assert_new_risk_blocked_by_reconciliation(&owner, pool_id, "s8c-malformed-fill-successor");
}

#[test]
fn s8c_local_cancel_request_cannot_clear_reconciliation_fault() {
    let pool_id = "pool-s8c-local-cancel-fault";
    let idempotency_key = "idempotency-s8c-local-cancel-fault";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-local-cancel-fault",
        "intent-s8c-local-cancel-fault",
        idempotency_key,
        "S8C-LOCAL-CANCEL-FAULT",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-local-cancel-fault-open",
            1_150,
            Some(1),
        ))
        .expect("first status event should apply");
    reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-local-cancel-fault-gap",
            1_170,
            Some(3),
        ))
        .expect("gapped terminal truth should require reconciliation");

    let _cancel_attempt =
        owner.mark_cancel_requested(client_order_id, "s8c-local-cancel-fault-request");

    assert_eq!(
        owner.reconcile_before_new_risk(),
        Err(RiskStateMutationError::ReconciliationRequired),
        "local cancel intent must not clear a reconciliation fault"
    );
    let mut sink = RecordingLiveSubmitBoundary::default();
    assert_eq!(
        reconciler.reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        ),
        Err(LifecycleReconciliationError::State(
            RiskSubmissionMutationError::State(RiskStateMutationError::ReconciliationRequired),
        )),
        "restart finalization must still fail closed until authoritative truth resolves the fault"
    );
    assert_eq!(
        sink.submitted_client_order_ids(),
        Vec::<ClientOrderId>::new(),
        "restart finalization must not resubmit when the live submission record is already present"
    );
    assert_eq!(
        owner.reconcile_before_new_risk(),
        Err(RiskStateMutationError::ReconciliationRequired),
        "the pool must remain closed after failed restart finalization"
    );
    assert_eq!(
        only_reservation_record(&owner).lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert_new_risk_blocked_by_reconciliation(&owner, pool_id, "s8c-local-cancel-fault-successor");
}

#[test]
fn s8c_faulting_event_replays_after_sequence_gap_is_filled() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8c-replay-gap-event",
        "owner-s8c-replay-gap-event",
        "intent-s8c-replay-gap-event",
        "idempotency-s8c-replay-gap-event",
        "S8C-REPLAY-GAP-EVENT",
    );
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-replay-gap-open",
            1_150,
            Some(1),
        ))
        .expect("first status event should apply");
    let gapped_cancel = nt_cancel_confirmed_status_with_ordering(
        client_order_id,
        "s8c-replay-gap-cancel",
        1_170,
        Some(3),
    );
    reconciler
        .apply_order_status_truth(gapped_cancel.clone())
        .expect("gapped cancel should be held for reconciliation");
    reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-replay-gap-fill",
            1_160,
            Some(2),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("missing lower-sequence fill should apply after the fault");

    let replayed = reconciler
        .apply_order_status_truth(gapped_cancel)
        .expect("replayed gapped event should apply after the sequence gap is filled");

    assert_eq!(
        replayed.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed,
        "the faulting event must not be dropped as already applied"
    );
    assert_eq!(
        only_reservation_record(&owner).lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after replay")
            .open_order_count(),
        0
    );
}

#[test]
fn s8c_reconciliation_required_exits_on_corrected_terminal_truth_and_releases_totals() {
    let pool_id = "pool-s8c-reconciliation-exit";
    let idempotency_key = "idempotency-s8c-reconciliation-exit";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-reconciliation-exit",
        "intent-s8c-reconciliation-exit",
        idempotency_key,
        "S8C-RECONCILIATION-EXIT",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-reconciliation-exit-open",
            1_150,
            Some(1),
        ))
        .expect("first status event should apply");
    let before_fault_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before ordering fault");
    assert_eq!(before_fault_totals.open_order_count(), 1);

    let fault = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-reconciliation-exit-gap",
            1_170,
            Some(3),
        ))
        .expect("gapped terminal truth should move the reservation to reconciliation");
    assert_eq!(
        fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );

    let corrected = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-reconciliation-exit-corrected",
            1_160,
            Some(2),
        ))
        .expect("corrected terminal truth should exit reconciliation and release remainder");

    assert_eq!(
        corrected.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    let record = only_reservation_record(&owner);
    assert_eq!(
        record.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    assert_eq!(record.remaining_fillable_quantity, Decimal::ZERO);
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after corrected truth")
            .open_order_count(),
        0,
        "corrected terminal truth must not leave a permanent ReconciliationRequired leak"
    );
    let mut sink = RecordingLiveSubmitBoundary::default();
    reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("corrected terminal truth must let restart completion reopen the pool");
}

#[test]
fn s8c_later_open_status_fault_blocks_older_terminal_release() {
    let pool_id = "pool-s8c-later-open-fault";
    let idempotency_key = "idempotency-s8c-later-open-fault";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-later-open-fault",
        "intent-s8c-later-open-fault",
        idempotency_key,
        "S8C-LATER-OPEN-FAULT",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-later-open-fault-open-1",
            1_150,
            Some(1),
        ))
        .expect("first open status should apply");
    let before_fault_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before the later open fault");

    let fault = reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-later-open-fault-open-3",
            1_170,
            Some(3),
        ))
        .expect("gapped later open truth should fail closed into reconciliation");
    assert_eq!(
        fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );

    let terminal = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-later-open-fault-cancel-2",
            1_160,
            Some(2),
        ))
        .expect("older terminal truth should be processed without releasing live-order risk");
    assert_eq!(
        terminal.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );

    let record_after_terminal = only_reservation_record(&owner);
    assert_eq!(
        record_after_terminal.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert_eq!(
        record_after_terminal.remaining_fillable_quantity,
        dec(2),
        "a later open-status fault must keep the live order's open remainder reserved"
    );
    assert!(
        record_after_terminal.open_order_remainder_held,
        "a later open-status fault must keep the open-order reservation held"
    );
    assert!(
        record_after_terminal
            .unresolved_lifecycle_reconciliation_faults
            .values()
            .any(
                |fault| fault.kind == LifecycleReconciliationFaultKind::OrderStatus
                    && fault.order_status == Some(ReservationLifecycleState::Open)
                    && fault.ts_event_unix_nanos == 1_170
                    && fault.event_sequence == Some(3)
            ),
        "older terminal truth must not clear a later non-terminal order-status fault"
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should remain readable after stale terminal truth"),
        before_fault_totals,
        "stale terminal truth must not release open-order risk while later open truth is unresolved"
    );

    let mut sink = RecordingLiveSubmitBoundary::default();
    let error = reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect_err("the retained later-open fault must keep reconciliation fail-closed");
    assert_eq!(
        error,
        LifecycleReconciliationError::State(RiskSubmissionMutationError::State(
            RiskStateMutationError::ReconciliationRequired,
        )),
    );
}

#[test]
fn s8c_faulted_fill_replays_after_absorbed_terminal_before_reconciliation_completion() {
    let pool_id = "pool-s8c-fill-before-terminal-release";
    let idempotency_key = "idempotency-s8c-fill-before-terminal-release";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-fill-before-terminal-release",
        "intent-s8c-fill-before-terminal-release",
        idempotency_key,
        "S8C-FILL-BEFORE-TERMINAL-RELEASE",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-fill-before-terminal-release-open",
            1_150,
            Some(1),
        ))
        .expect("first open status should apply");
    let before_fill_fault_totals = owner
        .reserved_risk_totals()
        .expect("reserved totals should be readable before the fill fault");

    let fill_fault = reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-fill-before-terminal-release-fill",
            1_170,
            Some(3),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("gapped fill should fail closed into reconciliation");
    assert_eq!(
        fill_fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    let record_after_fill_fault = only_reservation_record(&owner);
    assert!(record_after_fill_fault.open_order_remainder_held);
    assert_eq!(record_after_fill_fault.remaining_fillable_quantity, dec(2));
    assert!(record_after_fill_fault.filled_position_exposure.is_none());
    assert!(
        record_after_fill_fault
            .unresolved_lifecycle_reconciliation_faults
            .values()
            .any(|fault| fault.kind == LifecycleReconciliationFaultKind::Fill
                && fault.ts_event_unix_nanos == 1_170
                && fault.event_sequence == Some(3)),
        "the gapped fill must be retained as the blocking exposure fault"
    );

    let terminal = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-fill-before-terminal-release-cancel",
            1_160,
            Some(2),
        ))
        .expect("terminal status should be absorbed while the fill fault remains");
    assert_eq!(
        terminal.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    let record_after_terminal = only_reservation_record(&owner);
    assert_eq!(
        record_after_terminal.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert!(
        record_after_terminal.open_order_remainder_held,
        "the terminal must not release open-order risk before the fill can replay"
    );
    assert_eq!(record_after_terminal.remaining_fillable_quantity, dec(2));
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should remain readable after the absorbed terminal"),
        before_fill_fault_totals,
        "absorbing the terminal must not release the open remainder while exposure is unresolved"
    );

    let replayed_fill = reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-fill-before-terminal-release-fill",
            1_170,
            Some(3),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("the exact fill replay should apply after the terminal advances the sequence");
    assert_eq!(
        replayed_fill.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );
    let record_after_replayed_fill = only_reservation_record(&owner);
    assert!(record_after_replayed_fill.open_order_remainder_held);
    assert_eq!(
        record_after_replayed_fill.remaining_fillable_quantity,
        dec(1)
    );
    assert!(
        record_after_replayed_fill
            .unresolved_lifecycle_reconciliation_faults
            .is_empty(),
        "the replayed fill must clear its reconciliation fault"
    );

    let mut sink = RecordingLiveSubmitBoundary::default();
    reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("clearing the fill fault should let restart reconciliation complete");
}

#[test]
// Fail-closed contract: on a contradictory feed (a fill venue-sequenced after a terminal), an
// absorbed terminal leaves the order at PartiallyFilled and the open remainder is HELD with no
// automatic release. This is intentional over-reserve (never under-reserve). Graceful release is #1013.
fn s8c_absorbed_terminal_partial_fill_holds_open_remainder_fail_closed() {
    let pool_id = "pool-s8c-absorbed-terminal-strand";
    let idempotency_key = "idempotency-s8c-absorbed-terminal-strand";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-absorbed-terminal-strand",
        "intent-s8c-absorbed-terminal-strand",
        idempotency_key,
        "S8C-ABSORBED-TERMINAL-STRAND",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-absorbed-terminal-strand-open",
            1_150,
            Some(1),
        ))
        .expect("open seq1 applies");
    reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-absorbed-terminal-strand-fill",
            1_170,
            Some(3),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("gapped fill faults");
    reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-absorbed-terminal-strand-cancel",
            1_160,
            Some(2),
        ))
        .expect("cancel seq2 absorbed");
    let replay = reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-absorbed-terminal-strand-fill",
            1_170,
            Some(3),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("fill replays after absorbed terminal");
    assert_eq!(
        replay.lifecycle_state,
        ReservationLifecycleState::PartiallyFilled
    );
    // Contract: settlement cannot release from PartiallyFilled; the open remainder stays held (fail-closed).
    let settle = reconciler.apply_settlement_truth(nt_settlement(
        client_order_id,
        "s8c-absorbed-terminal-strand-settle",
        true,
        true,
        dec(28),
        dec(30),
        vec![dec(-2), dec(99)],
    ));
    assert!(
        settle.is_err(),
        "settlement from absorbed-terminal PartiallyFilled is rejected (#1013)"
    );
    let record = only_reservation_record(&owner);
    assert!(
        record.open_order_remainder_held,
        "open remainder is held fail-closed, not released (#1013)"
    );
    assert!(
        record.unresolved_lifecycle_reconciliation_faults.is_empty(),
        "the pool finalizes (not wedged); only this reservation is over-reserved until #1013"
    );
}

#[test]
fn s8c_terminal_status_clears_stale_earlier_open_status_fault() {
    let pool_id = "pool-s8c-stale-open-fault";
    let idempotency_key = "idempotency-s8c-stale-open-fault";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-stale-open-fault",
        "intent-s8c-stale-open-fault",
        idempotency_key,
        "S8C-STALE-OPEN-FAULT",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-stale-open-fault-open-1",
            1_150,
            Some(1),
        ))
        .expect("first open status should apply");
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-stale-open-fault-open-2",
            1_160,
            Some(2),
        ))
        .expect("second open status should apply");

    let fault = reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-stale-open-fault-duplicate-open-1",
            1_150,
            Some(1),
        ))
        .expect("stale distinct open truth should fail closed into reconciliation");
    assert_eq!(
        fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert!(
        only_reservation_record(&owner)
            .unresolved_lifecycle_reconciliation_faults
            .values()
            .any(
                |fault| fault.kind == LifecycleReconciliationFaultKind::OrderStatus
                    && fault.order_status == Some(ReservationLifecycleState::Open)
                    && fault.ts_event_unix_nanos == 1_150
                    && fault.event_sequence == Some(1)
            ),
        "test setup must create the stale earlier order-status fault"
    );

    let terminal = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-stale-open-fault-cancel-3",
            1_170,
            Some(3),
        ))
        .expect("later terminal truth should make stale earlier open truth irrelevant");
    assert_eq!(
        terminal.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );

    let record_after_terminal = only_reservation_record(&owner);
    assert_eq!(
        record_after_terminal.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    assert!(
        record_after_terminal
            .unresolved_lifecycle_reconciliation_faults
            .is_empty(),
        "later terminal truth must clear stale earlier order-status faults"
    );
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after terminal truth")
            .open_order_count(),
        0
    );

    let mut sink = RecordingLiveSubmitBoundary::default();
    reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("stale earlier order-status fault must not wedge reconciliation");
}

#[test]
fn s8c_fill_and_settlement_may_share_event_id_without_cross_kind_idempotency() {
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        "pool-s8c-cross-kind-event-id",
        "owner-s8c-cross-kind-event-id",
        "intent-s8c-cross-kind-event-id",
        "idempotency-s8c-cross-kind-event-id",
        "S8C-CROSS-KIND-EVENT-ID",
    );
    reconciler
        .apply_order_status_truth(nt_open_status(
            client_order_id,
            "s8c-cross-kind-event-id-open",
        ))
        .expect("authoritative open status should move the order to Open");
    reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8c-cross-kind-event-id-shared",
            dec(2),
            dec(0),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("fill truth should apply even with a reusable caller event id");

    let settlement = reconciler
        .apply_settlement_truth(nt_settlement(
            client_order_id,
            "s8c-cross-kind-event-id-shared",
            true,
            true,
            dec(28),
            dec(30),
            vec![dec(-2), dec(99)],
        ))
        .expect("settlement truth must not be skipped by a same-id fill event");

    assert_eq!(
        settlement.lifecycle_state,
        ReservationLifecycleState::Settled
    );
    let record = only_reservation_record(&owner);
    assert_eq!(record.lifecycle_state, ReservationLifecycleState::Settled);
    let exposure = record
        .filled_position_exposure
        .expect("settlement should revise the filled exposure before release");
    assert_eq!(exposure.conservative_liquidation_value, dec(28));
    assert_eq!(exposure.governor_cost_basis, dec(30));
    assert_eq!(exposure.terminal_cash_flows, vec![dec(-2), dec(99)]);
    assert_eq!(
        owner
            .reserved_risk_totals()
            .expect("reserved totals should be readable after final settlement")
            .position_quantity(),
        Decimal::ZERO,
        "final settlement should apply and release the filled-position reservation"
    );
}

#[test]
fn s8c_faulted_fill_is_not_cleared_by_later_terminal_truth() {
    let pool_id = "pool-s8c-fill-fault-retained";
    let idempotency_key = "idempotency-s8c-fill-fault-retained";
    let (_reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        "owner-s8c-fill-fault-retained",
        "intent-s8c-fill-fault-retained",
        idempotency_key,
        "S8C-FILL-FAULT-RETAINED",
    );
    record_live_submission_for_test(&owner, idempotency_key, client_order_id);
    reconciler
        .apply_order_status_truth(nt_open_status_with_ordering(
            client_order_id,
            "s8c-fill-fault-retained-open",
            1_150,
            Some(1),
        ))
        .expect("first status event should apply");

    let fault = reconciler
        .apply_fill_truth(nt_fill_with_ordering(
            client_order_id,
            "s8c-fill-fault-retained-fill",
            1_170,
            Some(3),
            dec(1),
            dec(1),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("gapped fill must fail closed into reconciliation");
    assert_eq!(
        fault.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    let record_after_fill_fault = only_reservation_record(&owner);
    let fill_fault = record_after_fill_fault
        .unresolved_lifecycle_reconciliation_faults
        .values()
        .find(|fault| {
            fault.kind == LifecycleReconciliationFaultKind::Fill
                && fault.ts_event_unix_nanos == 1_170
                && fault.event_sequence == Some(3)
        })
        .expect("the unresolved fill fault must retain its ordering key");
    assert_eq!(fill_fault.kind, LifecycleReconciliationFaultKind::Fill);
    assert_eq!(fill_fault.order_status, None);
    assert_eq!(fill_fault.ts_event_unix_nanos, 1_170);
    assert_eq!(fill_fault.event_sequence, Some(3));

    let terminal = reconciler
        .apply_order_status_truth(nt_cancel_confirmed_status_with_ordering(
            client_order_id,
            "s8c-fill-fault-retained-cancel",
            1_160,
            Some(2),
        ))
        .expect("lower-sequence terminal status should apply");
    assert_eq!(
        terminal.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );

    let record_after_terminal = only_reservation_record(&owner);
    assert_eq!(
        record_after_terminal.lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
    assert!(
        record_after_terminal.filled_position_exposure.is_none(),
        "the faulted fill must not be synthesized without authoritative application"
    );
    assert!(
        record_after_terminal
            .unresolved_lifecycle_reconciliation_faults
            .values()
            .any(|fault| fault.kind == LifecycleReconciliationFaultKind::Fill
                && fault.ts_event_unix_nanos == 1_170
                && fault.event_sequence == Some(3)),
        "terminal status must not drop an unresolved fill fault"
    );

    let mut sink = RecordingLiveSubmitBoundary::default();
    let error = reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: Vec::new(),
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect_err("the retained fill fault must keep reconciliation fail-closed");
    assert_eq!(
        error,
        LifecycleReconciliationError::State(RiskSubmissionMutationError::State(
            RiskStateMutationError::ReconciliationRequired,
        )),
    );
}

#[test]
fn s8c_restart_reconciliation_refuses_to_finalize_with_reconciliation_required_record() {
    let pool_id = "pool-s8c-restart-fail-open";
    let (_reservation, owner, store, _client_order_id) = submitted_reservation_with_store(
        pool_id,
        "owner-s8c-restart-fail-open",
        "intent-s8c-restart-fail-open",
        "idempotency-s8c-restart-fail-open",
        "S8C-RESTART-FAIL-OPEN",
    );
    let successor = RiskStateOwner::acquire(
        store,
        PoolId::new(pool_id).expect("pool id should be valid"),
        "owner-s8c-restart-fail-open-successor",
    )
    .expect("successor owner should acquire the pool");
    let reconciler = LifecycleReconciler::new(successor.clone());
    let mut sink = RecordingLiveSubmitBoundary::default();

    let error = reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: vec![
                    nt_open_status_with_ordering(
                        client_order_id("S8C-RESTART-FAIL-OPEN"),
                        "s8c-restart-fail-open-status",
                        1_150,
                        Some(1),
                    ),
                    nt_cancel_confirmed_status_with_ordering(
                        client_order_id("S8C-RESTART-FAIL-OPEN"),
                        "s8c-restart-fail-open-gap",
                        1_170,
                        Some(3),
                    ),
                ],
                fill_reports: Vec::new(),
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect_err("restart reconciliation must not finalize over ReconciliationRequired records");
    assert_eq!(
        error,
        LifecycleReconciliationError::State(RiskSubmissionMutationError::State(
            RiskStateMutationError::ReconciliationRequired,
        )),
    );
    assert_new_risk_blocked_by_reconciliation(
        &successor,
        pool_id,
        "s8c-restart-fail-open-successor-intent",
    );
    assert_eq!(
        only_reservation_record(&owner).lifecycle_state,
        ReservationLifecycleState::ReconciliationRequired
    );
}

#[test]
fn s8c_restart_reconciliation_applies_truth_in_monotonic_cross_type_order() {
    let pool_id = "pool-s8c-restart-ordering";
    let (_reservation, owner, store, _client_order_id) = submitted_reservation_with_store(
        pool_id,
        "owner-s8c-restart-ordering",
        "intent-s8c-restart-ordering",
        "idempotency-s8c-restart-ordering",
        "S8C-RESTART-ORDERING",
    );
    let successor = RiskStateOwner::acquire(
        store,
        PoolId::new(pool_id).expect("pool id should be valid"),
        "owner-s8c-restart-ordering-successor",
    )
    .expect("successor owner should acquire the pool");
    let reconciler = LifecycleReconciler::new(successor.clone());
    let mut sink = RecordingLiveSubmitBoundary::default();

    reconciler
        .reconcile_restart(
            NtExecutionTruth {
                order_status_reports: vec![nt_cancel_confirmed_status_with_ordering(
                    client_order_id("S8C-RESTART-ORDERING"),
                    "s8c-restart-ordering-cancel",
                    1_170,
                    Some(3),
                )],
                fill_reports: vec![nt_fill_with_ordering(
                    client_order_id("S8C-RESTART-ORDERING"),
                    "s8c-restart-ordering-fill",
                    1_160,
                    Some(2),
                    dec(1),
                    dec(1),
                    dec(24),
                    dec(26),
                    vec![dec(1), dec(99)],
                )],
                settlement_reports: Vec::new(),
            },
            &mut sink,
            1_200,
        )
        .expect("restart reconciliation should apply fill before higher-sequence status");

    let record = only_reservation_record(&successor);
    assert_eq!(
        record.lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
    assert_eq!(
        record
            .filled_position_exposure
            .expect("cross-type monotonic replay should apply the fill exposure")
            .quantity,
        dec(1)
    );
    assert_eq!(
        successor
            .reserved_risk_totals()
            .expect("reserved totals should be readable after ordered replay")
            .open_order_count(),
        0,
        "higher-sequence terminal status should release only after the lower-sequence fill applies"
    );
    assert_eq!(
        only_reservation_record(&owner).lifecycle_state,
        ReservationLifecycleState::CancelConfirmed
    );
}

#[test]
fn s4_sc_012_submit_boundary_is_admitted_order_only_and_authority_owned() {
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
}

#[test]
fn s4_complete_reconciliation_is_not_a_public_direct_reconcile_door() {
    let state_owner_source =
        include_str!("../src/bolt_v3_risk_reservation_substrate/state_owner.rs");
    assert!(
        !state_owner_source.contains("    pub fn complete_reconciliation(&self)"),
        "RiskStateOwner::complete_reconciliation must be unreachable to direct callers"
    );

    let lifecycle_reconciler_source =
        include_str!("../src/bolt_v3_risk_reservation_substrate/lifecycle_reconciler.rs");
    assert!(
        lifecycle_reconciler_source.contains(".complete_reconciliation()?"),
        "LifecycleReconciler::reconcile_restart must own reconciliation completion"
    );
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

fn unreconciled_risk_context_with_work_bounds(
    pool_id: &str,
    owner_id: &str,
    work_bounds: RiskReservationWorkBounds,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    let (service, owner, store) =
        risk_context_without_policy_epoch_with_work_bounds(pool_id, owner_id, work_bounds);
    bind_default_policy_epoch(&owner, pool_id);
    (service, owner, store)
}

fn risk_context_without_policy_epoch(
    pool_id: &str,
    owner_id: &str,
) -> (AdmissionService, RiskStateOwner, FencedRiskStateStore) {
    risk_context_without_policy_epoch_with_work_bounds(pool_id, owner_id, roomy_work_bounds())
}

fn risk_context_without_policy_epoch_with_work_bounds(
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
    bind_default_policy_epoch(&owner, pool_id);
    (AdmissionService::new(owner.clone()), owner, store)
}

fn bind_default_policy_epoch(owner: &RiskStateOwner, pool_id: &str) {
    owner
        .bind_initial_policy_epoch(
            default_policy_epoch(pool_id),
            RiskStateVersion::zero(),
            Vec::new(),
            true,
            true,
        )
        .expect("default enabled policy epoch should bind for admission fixtures");
}

fn default_policy_epoch(pool_id: &str) -> PreparedPolicyEpoch {
    let bucket = bucket("risk_class", "alpha");
    PreparedPolicyEpoch {
        epoch_id: "policy-epoch".to_string(),
        environment: "test-environment".to_string(),
        pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
        policy_digest: "policy-digest".to_string(),
        descriptor_map_digest: "descriptor-map-digest".to_string(),
        descriptor_map: BTreeMap::from([(
            "candidate-instrument".to_string(),
            PreparedEpochDescriptor {
                active_descriptor: ActiveDescriptorView {
                    instrument_id: "candidate-instrument".to_string(),
                    descriptor_version: "descriptor-version".to_string(),
                    policy_epoch_id: "policy-epoch".to_string(),
                    terminal_state_ids: vec![
                        "terminal-state-0".to_string(),
                        "terminal-state-1".to_string(),
                    ],
                    terminal_cash_flows: vec![dec(0), dec(99)],
                },
                descriptor_attributes: RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
                    "descriptor_risk_class".to_string(),
                    bucket.bucket_value().to_string(),
                )]))
                .expect("descriptor attributes should be valid"),
            },
        )]),
        classifier_version: "classifier-version".to_string(),
        classification_policy: classification_policy([dimension(
            "risk_class",
            "descriptor_risk_class",
        )]),
        model_version: "model-version".to_string(),
        fallback_model_version: "fallback-version".to_string(),
        fee_model_version: "fee-version".to_string(),
        quantity_policy_versions: vec!["quantity-version".to_string()],
        approvals: vec![PolicyApproval {
            approval_id: "approval".to_string(),
            approver_id: "approver".to_string(),
            approved_at_unix_nanos: 900,
        }],
        approval_digest: "approval-digest".to_string(),
        declared_attestations: Vec::new(),
        activation_not_after_unix_nanos: 1_050,
    }
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
    safety_state: BoundReusableSafetyState,
    max_exposure_count: usize,
) -> SafetyActionAdmissionRequest {
    SafetyActionAdmissionRequest {
        action_id: action_id.to_string(),
        action: SafetyAction::ReduceOnlyCloseExistingPosition {
            position_id: position_id.to_string(),
        },
        safety_state,
        proof_domain: SafetyActionProofDomain { max_exposure_count },
    }
}

fn cancel_order_safety_action_request(
    action_id: &str,
    client_order_id: &str,
    safety_state: BoundReusableSafetyState,
    max_exposure_count: usize,
) -> SafetyActionAdmissionRequest {
    SafetyActionAdmissionRequest {
        action_id: action_id.to_string(),
        action: SafetyAction::CancelExistingOrder {
            client_order_id: client_order_id.to_string(),
        },
        safety_state,
        proof_domain: SafetyActionProofDomain { max_exposure_count },
    }
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
    published_view_with_classification_and_open_order_headroom(
        risk_state_version,
        pool_id,
        instrument_id,
        buckets,
        global_headroom,
        bucket_headroom,
        terminal_cash_flows,
        positions,
        1,
    )
}

fn published_view_with_open_order_headroom(
    risk_state_version: RiskStateVersion,
    pool_id: &str,
    instrument_id: &str,
    bucket: ConcentrationBucket,
    global_headroom: Decimal,
    bucket_headroom: Decimal,
    open_order_headroom: u64,
) -> bolt_v2::bolt_v3_risk_reservation_substrate::risk_view_publisher::PublishedRiskView {
    published_view_with_classification_and_open_order_headroom(
        risk_state_version,
        pool_id,
        instrument_id,
        vec![bucket],
        global_headroom,
        bucket_headroom,
        vec![dec(0), dec(99)],
        Vec::new(),
        open_order_headroom,
    )
}

fn published_view_with_classification_and_open_order_headroom(
    risk_state_version: RiskStateVersion,
    pool_id: &str,
    instrument_id: &str,
    buckets: Vec<ConcentrationBucket>,
    global_headroom: Decimal,
    bucket_headroom: Decimal,
    terminal_cash_flows: Vec<Decimal>,
    positions: Vec<RiskExposure>,
    open_order_headroom: u64,
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
            open_order_headroom,
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
            permit_id: format!("permit-{idempotency_key}"),
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

fn submitted_reservation_context(
    pool_id: &str,
    owner_id: &str,
    intent_id: &str,
    idempotency_key: &str,
    client_order_id_value: &str,
) -> (
    RiskReservationCommit,
    RiskStateOwner,
    LifecycleReconciler,
    ClientOrderId,
) {
    let (service, owner, _store) = reconciled_risk_context(pool_id, owner_id);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        pool_id,
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate_from_preview(
                intent_id,
                idempotency_key,
                RiskPreviewInput {
                    pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
                    instrument_id: "candidate-instrument".to_string(),
                    model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
                        instrument_id: "candidate-instrument".to_string(),
                    },
                    side: "long".to_string(),
                    quantity: dec(2),
                    order_type: "limit".to_string(),
                    time_in_force: "gtc".to_string(),
                    max_unit_price: Some(dec(20)),
                    max_cash_outlay: dec(20),
                    source_view_version: RiskStateVersion::zero(),
                    policy_epoch_id: "policy-epoch".to_string(),
                },
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("reservation should issue an admission token");
    let authority = SubmissionAuthority::new(owner.clone());
    let client_order_id = client_order_id(client_order_id_value);
    authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("submission authority should move the reservation to Submitted");
    (
        reservation,
        owner.clone(),
        LifecycleReconciler::new(owner),
        client_order_id,
    )
}

fn submitted_reservation_with_store(
    pool_id: &str,
    owner_id: &str,
    intent_id: &str,
    idempotency_key: &str,
    client_order_id_value: &str,
) -> (
    RiskReservationCommit,
    RiskStateOwner,
    FencedRiskStateStore,
    ClientOrderId,
) {
    let (service, owner, store) = reconciled_risk_context(pool_id, owner_id);
    let bucket = bucket("risk_class", "alpha");
    let view = published_view(
        RiskStateVersion::zero(),
        pool_id,
        "candidate-instrument",
        bucket,
        dec(100),
        dec(100),
    );
    let reservation = service
        .compare_and_reserve(
            &view,
            admission_candidate_from_preview(
                intent_id,
                idempotency_key,
                RiskPreviewInput {
                    pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
                    instrument_id: "candidate-instrument".to_string(),
                    model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
                        instrument_id: "candidate-instrument".to_string(),
                    },
                    side: "long".to_string(),
                    quantity: dec(2),
                    order_type: "limit".to_string(),
                    time_in_force: "gtc".to_string(),
                    max_unit_price: Some(dec(20)),
                    max_cash_outlay: dec(20),
                    source_view_version: RiskStateVersion::zero(),
                    policy_epoch_id: "policy-epoch".to_string(),
                },
            ),
            unlatched_safety(RiskStateVersion::zero()),
            None,
            1_010,
        )
        .expect("reservation should issue an admission token");
    let authority = SubmissionAuthority::new(owner.clone());
    let client_order_id = client_order_id(client_order_id_value);
    authority
        .prepare_admitted_order(&reservation, client_order_id, 1_100)
        .expect("submission authority should move the reservation to Submitted");
    (reservation, owner, store, client_order_id)
}

fn filled_reservation_context(
    pool_id: &str,
    owner_id: &str,
    intent_id: &str,
    idempotency_key: &str,
    client_order_id_value: &str,
) -> (
    RiskReservationCommit,
    RiskStateOwner,
    LifecycleReconciler,
    ClientOrderId,
) {
    let (reservation, owner, reconciler, client_order_id) = submitted_reservation_context(
        pool_id,
        owner_id,
        intent_id,
        idempotency_key,
        client_order_id_value,
    );
    reconciler
        .apply_order_status_truth(nt_open_status(client_order_id, "s8a-filled-open"))
        .expect("authoritative NT open status should move the reservation to Open");
    reconciler
        .apply_fill_truth(nt_fill(
            client_order_id,
            "s8a-filled-fill",
            dec(2),
            dec(0),
            dec(24),
            dec(26),
            vec![dec(1), dec(99)],
        ))
        .expect("authoritative NT fill truth should move the reservation to Filled");
    (reservation, owner, reconciler, client_order_id)
}

fn assert_new_risk_blocked_by_reconciliation(
    owner: &RiskStateOwner,
    pool_id: &str,
    successor_intent_id: &str,
) {
    let service = AdmissionService::new(owner.clone());
    let source_version = owner
        .policy_epoch_snapshot()
        .expect("policy snapshot should expose post-fault version")
        .risk_state_version;
    let view = published_view_with_open_order_headroom(
        source_version,
        pool_id,
        "candidate-instrument",
        bucket("risk_class", "alpha"),
        dec(100),
        dec(100),
        2,
    );
    let rejected = service
        .compare_and_reserve(
            &view,
            admission_candidate(
                successor_intent_id,
                &format!("{successor_intent_id}-idempotency"),
                pool_id,
                "candidate-instrument",
                source_version,
                dec(20),
            ),
            unlatched_safety(source_version),
            None,
            1_500,
        )
        .expect_err("risk-increasing admission must remain blocked until reconciliation");
    assert!(matches!(
        rejected,
        AdmissionReserveError::StateMutation(RiskStateMutationError::ReconciliationRequired)
    ));
}

fn nt_open_status(client_order_id: ClientOrderId, event_id: &str) -> NtOrderStatusReportTruth {
    nt_open_status_with_ordering(client_order_id, event_id, 1_150, None)
}

fn nt_open_status_with_ordering(
    client_order_id: ClientOrderId,
    event_id: &str,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
) -> NtOrderStatusReportTruth {
    NtOrderStatusReportTruth {
        client_order_id,
        status: NtOrderStatusTruth::Open,
        event_id: event_id.to_string(),
        ts_event_unix_nanos,
        event_sequence,
        ts_init_unix_nanos: ts_event_unix_nanos + 1,
    }
}

fn nt_cancel_confirmed_status(
    client_order_id: ClientOrderId,
    event_id: &str,
) -> NtOrderStatusReportTruth {
    nt_cancel_confirmed_status_with_ordering(client_order_id, event_id, 1_170, None)
}

fn nt_cancel_confirmed_status_with_ordering(
    client_order_id: ClientOrderId,
    event_id: &str,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
) -> NtOrderStatusReportTruth {
    NtOrderStatusReportTruth {
        client_order_id,
        status: NtOrderStatusTruth::CancelConfirmed,
        event_id: event_id.to_string(),
        ts_event_unix_nanos,
        event_sequence,
        ts_init_unix_nanos: ts_event_unix_nanos + 1,
    }
}

fn nt_expired_confirmed_status(
    client_order_id: ClientOrderId,
    event_id: &str,
) -> NtOrderStatusReportTruth {
    NtOrderStatusReportTruth {
        client_order_id,
        status: NtOrderStatusTruth::ExpiredConfirmed,
        event_id: event_id.to_string(),
        ts_event_unix_nanos: 1_180,
        event_sequence: None,
        ts_init_unix_nanos: 1_181,
    }
}

fn nt_fill(
    client_order_id: ClientOrderId,
    event_id: &str,
    fill_quantity: Decimal,
    remaining_fillable_quantity: Decimal,
    actual_conservative_liquidation_value: Decimal,
    actual_governor_cost_basis: Decimal,
    terminal_cash_flows: Vec<Decimal>,
) -> NtFillReportTruth {
    nt_fill_with_ordering(
        client_order_id,
        event_id,
        1_160,
        None,
        fill_quantity,
        remaining_fillable_quantity,
        actual_conservative_liquidation_value,
        actual_governor_cost_basis,
        terminal_cash_flows,
    )
}

fn nt_fill_with_ordering(
    client_order_id: ClientOrderId,
    event_id: &str,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
    fill_quantity: Decimal,
    remaining_fillable_quantity: Decimal,
    actual_conservative_liquidation_value: Decimal,
    actual_governor_cost_basis: Decimal,
    terminal_cash_flows: Vec<Decimal>,
) -> NtFillReportTruth {
    NtFillReportTruth {
        client_order_id,
        event_id: event_id.to_string(),
        ts_event_unix_nanos,
        event_sequence,
        ts_init_unix_nanos: ts_event_unix_nanos + 1,
        fill_quantity,
        remaining_fillable_quantity,
        actual_conservative_liquidation_value,
        actual_governor_cost_basis,
        terminal_cash_flows,
    }
}

fn nt_settlement(
    client_order_id: ClientOrderId,
    event_id: &str,
    terminal_final: bool,
    reconciliation_complete: bool,
    conservative_liquidation_value: Decimal,
    governor_cost_basis: Decimal,
    terminal_cash_flows: Vec<Decimal>,
) -> NtSettlementTruth {
    NtSettlementTruth {
        client_order_id,
        event_id: event_id.to_string(),
        ts_event_unix_nanos: 1_300,
        event_sequence: None,
        ts_init_unix_nanos: 1_301,
        terminal_final,
        reconciliation_complete,
        conservative_liquidation_value,
        governor_cost_basis,
        terminal_cash_flows,
    }
}

fn only_reservation_record(owner: &RiskStateOwner) -> SubstrateReservationRecord {
    let records = owner
        .reservation_records()
        .expect("reservation records should be readable");
    assert_eq!(records.len(), 1);
    records[0].clone()
}

fn reservation_record_for_commit(
    owner: &RiskStateOwner,
    reservation: &RiskReservationCommit,
) -> SubstrateReservationRecord {
    owner
        .reservation_records()
        .expect("reservation records should be readable")
        .into_iter()
        .find(|record| record.admission_token == reservation.admission_token)
        .expect("reservation record for commit should exist")
}

fn record_live_submission_for_test(
    owner: &RiskStateOwner,
    idempotency_key: &str,
    client_order_id: ClientOrderId,
) {
    let intent = owner
        .durable_submission_intent(idempotency_key)
        .expect("durable submission intent should be readable");
    owner
        .record_live_submission(
            idempotency_key,
            LiveSubmissionRecord {
                client_order_id,
                risk_state_version: intent.submitted_risk_state_version,
            },
        )
        .expect("live submission record should be writable for test setup");
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
