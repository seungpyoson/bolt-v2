use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Barrier},
    thread,
};

use bolt_v2::bolt_v3_risk_reservation_substrate::{
    admission_service::{
        AdmissionReserveError, AdmissionService, BoundReusableSafetyState, CallerRiskDiagnostics,
        RiskCapDimension,
    },
    contracts::{
        ActiveDescriptorView, AdmissionCandidate, AdmissionToken, AdmittedOrder,
        ConfiguredLeaseAuthority, LeaseAuthorityBackend, ModelRiskEvaluationScope, PoolId,
        PoolOwnershipLease, PreparedPolicyEpoch, RiskAssessment, RiskPreviewInput, RiskSizingView,
        RiskStateVersion, SafetyAction, SafetyPolicyEnvelope, SizingDecisionPermit,
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
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        format!("{pool_id}-lease-authority"),
    )
    .expect("lease authority dependency should be valid");
    let store = FencedRiskStateStore::new(lease_authority);
    let owner = RiskStateOwner::acquire(
        store,
        PoolId::new(pool_id).expect("pool id should be valid"),
        owner_id,
    )
    .expect("risk state owner should acquire the pool");
    owner
        .reconcile_before_new_risk()
        .expect("owner should reconcile before admission");
    AdmissionService::new(owner)
}

fn unlatched_safety(risk_state_version: RiskStateVersion) -> BoundReusableSafetyState {
    BoundReusableSafetyState {
        risk_state_version,
        kill_switch_latched: false,
        loss_governor_halted: false,
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
    let bucket_attribute = "descriptor_risk_class";
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
            bucket_stress_loss_headrooms: BTreeMap::from([(bucket.clone(), bucket_headroom)]),
            open_order_headroom: 1,
            position_quantity_headroom: dec(100),
        },
        active_descriptor: ActiveDescriptorView {
            instrument_id: instrument_id.to_string(),
            descriptor_version: "descriptor-version".to_string(),
            policy_epoch_id: "policy-epoch".to_string(),
            terminal_state_ids: vec!["terminal-loss".to_string(), "terminal-gain".to_string()],
            terminal_cash_flows: vec![dec(0), dec(99)],
        },
        descriptor_attributes: RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
            bucket_attribute.to_string(),
            bucket.bucket_value().to_string(),
        )]))
        .expect("descriptor attributes should classify"),
        classification_policy: classification_policy([dimension(
            bucket.bucket_class(),
            bucket_attribute,
        )]),
        caller_declared_buckets: Vec::new(),
        portfolio: RiskPortfolioSnapshot {
            positions: Vec::new(),
        },
        portfolio_scope_id: pool_id.to_string(),
    })
    .expect("published view should resolve active descriptor and classification")
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
