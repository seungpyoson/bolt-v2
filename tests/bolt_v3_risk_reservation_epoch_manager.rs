use std::collections::{BTreeMap, BTreeSet};

use bolt_v2::bolt_v3_risk_reservation_substrate::{
    admission_service::{
        AdmissionReserveError, AdmissionService, BoundReusableSafetyState,
        SafetyActionAdmissionRequest, SafetyActionProofDomain,
    },
    contracts::{
        ActiveDescriptorView, AdmissionCandidate, ConfiguredLeaseAuthority, LeaseAuthorityBackend,
        ModelRiskEvaluationScope, PolicyApproval, PoolId, PreparedEpochAttestation,
        PreparedEpochDescriptor, PreparedPolicyEpoch, RiskPreviewInput, RiskSizingView,
        RiskStateVersion, SafetyAction, SafetyEnvelopeInvariant, SafetyPolicyEnvelope,
        SafetyPolicyEnvelopeRanges, SizingDecisionPermit,
    },
    epoch_manager::{
        EpochManager, PolicyEpochActivationError, PolicyEpochAlertReason, PolicyEpochPrepareError,
        PolicyEpochRevaluationError, PostCutoverAdmissionState, PreparedEpochRevaluationInput,
        PreparedEpochRevaluator, SafetyPolicyEnvelopeViolation, VenueEventDrain,
        VenueEventDrainError, VenueEventDrainReport,
    },
    risk_classifier::{
        ConcentrationBucket, ConcentrationBucketDimension, RiskClassificationPolicy,
        RiskDescriptorCanonicalAttributes,
    },
    risk_kernel::{RiskExposure, RiskExposureSetInput, RiskPortfolioSnapshot},
    risk_view_publisher::{PublishedRiskView, RiskViewPublicationInput, RiskViewPublisher},
    state_owner::{FencedRiskStateStore, RiskMutationKind, RiskStateMutationError, RiskStateOwner},
};
use rust_decimal::Decimal;

#[test]
fn s6a_cutover_is_observed_all_old_then_all_new_without_mixed_epoch_state() {
    let pool_id = "epoch-cutover-pool";
    let (_store, owner, manager) = epoch_context(pool_id, "epoch-cutover-owner");
    let envelope = envelope(pool_id, ["classifier-old", "classifier-new"]);
    let old_epoch = epoch(pool_id, "epoch-old", "descriptor-old", "classifier-old", 10);
    let new_epoch = epoch(pool_id, "epoch-new", "descriptor-new", "classifier-new", 12);

    let old_cutover = manager
        .prepare_policy_epoch(
            old_epoch.clone(),
            envelope.clone(),
            1_000,
            &mut RecordingDrain::default(),
            &mut RecordingRevaluator::compliant(),
        )
        .expect("old epoch should prepare");
    manager
        .activate_prepared_epoch(old_cutover)
        .expect("old epoch should activate");

    let old_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable");
    assert_epoch_snapshot(
        &old_snapshot,
        "epoch-old",
        "classifier-old",
        "descriptor-old",
    );

    let prepared_new = manager
        .prepare_policy_epoch(
            new_epoch,
            envelope,
            1_001,
            &mut RecordingDrain::default(),
            &mut RecordingRevaluator::compliant(),
        )
        .expect("new epoch should prepare without mutating active state");
    let still_old = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable before activation");
    assert_epoch_snapshot(&still_old, "epoch-old", "classifier-old", "descriptor-old");

    let activation = manager
        .activate_prepared_epoch(prepared_new)
        .expect("new epoch should atomically activate");
    let new_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable after activation");
    assert_epoch_snapshot(
        &new_snapshot,
        "epoch-new",
        "classifier-new",
        "descriptor-new",
    );
    assert_eq!(
        new_snapshot.risk_state_version,
        activation.risk_state_version
    );
    assert!(new_snapshot.risk_increasing_admission_enabled);
    assert!(new_snapshot.safety_action_enabled);

    let mutations = owner
        .durable_mutation_records()
        .expect("mutation records should be readable");
    assert_eq!(
        mutations
            .iter()
            .filter(|record| record.mutation.kind() == RiskMutationKind::PolicyEpoch)
            .count(),
        2,
        "each activation must be one versioned policy-epoch mutation"
    );
}

#[test]
fn s6a_stale_prepared_epoch_activation_rejects_and_keeps_active_epoch() {
    let pool_id = "epoch-stale-cutover-pool";
    let (_store, owner, manager) = epoch_context(pool_id, "epoch-stale-cutover-owner");
    let envelope = envelope(pool_id, ["classifier-old", "classifier-new"]);
    let old_epoch = epoch(pool_id, "epoch-old", "descriptor-old", "classifier-old", 10);
    let new_epoch = epoch(pool_id, "epoch-new", "descriptor-new", "classifier-new", 12);

    manager
        .activate_prepared_epoch(
            manager
                .prepare_policy_epoch(
                    old_epoch,
                    envelope.clone(),
                    1_000,
                    &mut RecordingDrain::default(),
                    &mut RecordingRevaluator::compliant(),
                )
                .expect("old epoch should prepare"),
        )
        .expect("old epoch should activate");

    let prepared_new = manager
        .prepare_policy_epoch(
            new_epoch,
            envelope,
            1_001,
            &mut RecordingDrain::default(),
            &mut RecordingRevaluator::compliant(),
        )
        .expect("new epoch should prepare against current risk-state version");
    let prepared_source_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable after prepare");
    assert_epoch_snapshot(
        &prepared_source_snapshot,
        "epoch-old",
        "classifier-old",
        "descriptor-old",
    );

    let service = AdmissionService::new(owner.clone());
    let old_epoch_view = published_view_for_epoch_descriptor(
        pool_id,
        prepared_source_snapshot.risk_state_version,
        "epoch-old",
        "descriptor-old",
    );
    let mut intervening_candidate = admission_candidate(
        pool_id,
        prepared_source_snapshot.risk_state_version,
        "epoch-old",
    );
    intervening_candidate.expected_descriptor_version = "descriptor-old".to_string();
    service
        .compare_and_reserve(
            &old_epoch_view,
            intervening_candidate,
            BoundReusableSafetyState {
                risk_state_version: prepared_source_snapshot.risk_state_version,
                kill_switch_latched: false,
                loss_governor_halted: false,
            },
            None,
            1_002,
        )
        .expect("intervening reserve should advance the same risk-state version domain");

    let advanced_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable after intervening reserve");
    assert_epoch_snapshot(
        &advanced_snapshot,
        "epoch-old",
        "classifier-old",
        "descriptor-old",
    );
    assert_ne!(
        advanced_snapshot.risk_state_version, prepared_source_snapshot.risk_state_version,
        "intervening owner mutation must advance the risk-state version"
    );

    let activation_error = manager
        .activate_prepared_epoch(prepared_new)
        .expect_err("stale prepared cutover must fail closed");
    assert_eq!(
        activation_error,
        PolicyEpochActivationError::StateMutation(RiskStateMutationError::StaleRiskStateVersion)
    );

    let retained_snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable after stale activation rejection");
    assert_eq!(
        retained_snapshot.risk_state_version, advanced_snapshot.risk_state_version,
        "rejected stale activation must not advance or install"
    );
    assert_epoch_snapshot(
        &retained_snapshot,
        "epoch-old",
        "classifier-old",
        "descriptor-old",
    );
}

#[test]
fn s6a_partial_revaluation_failure_leaves_old_epoch_and_no_new_risk_alert() {
    let pool_id = "epoch-revalue-failure-pool";
    let (_store, owner, manager) = epoch_context(pool_id, "epoch-revalue-failure-owner");
    let envelope = envelope(pool_id, ["classifier-old", "classifier-new"]);
    let old_epoch = epoch(pool_id, "epoch-old", "descriptor-old", "classifier-old", 10);
    let new_epoch = epoch(pool_id, "epoch-new", "descriptor-new", "classifier-new", 12);
    manager
        .activate_prepared_epoch(
            manager
                .prepare_policy_epoch(
                    old_epoch,
                    envelope.clone(),
                    1_000,
                    &mut RecordingDrain::default(),
                    &mut RecordingRevaluator::compliant(),
                )
                .expect("old epoch should prepare"),
        )
        .expect("old epoch should activate");

    let error = manager
        .prepare_policy_epoch(
            new_epoch,
            envelope,
            1_001,
            &mut RecordingDrain::default(),
            &mut RecordingRevaluator::partial_failure("reservation-revalue"),
        )
        .expect_err("partial revaluation failure must fail closed");

    assert!(matches!(
        error,
        PolicyEpochPrepareError::RevaluationFailed(_)
    ));
    let snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable after failed prepare");
    assert_epoch_snapshot(&snapshot, "epoch-old", "classifier-old", "descriptor-old");
    assert!(!snapshot.risk_increasing_admission_enabled);
    assert!(snapshot.safety_action_enabled);
    assert_eq!(
        snapshot.alerts.last().map(|alert| alert.reason),
        Some(PolicyEpochAlertReason::PartialRevaluationFailure)
    );
}

#[test]
fn s6a_catastrophic_but_valid_epoch_outside_envelope_fails_before_activation() {
    let pool_id = "epoch-envelope-pool";
    let (_store, owner, manager) = epoch_context(pool_id, "epoch-envelope-owner");
    let envelope = envelope(pool_id, ["classifier-valid"]);
    let outside_range_epoch = epoch(
        pool_id,
        "epoch-outside-envelope",
        "descriptor-valid",
        "classifier-valid",
        500,
    );
    let mut drain = RecordingDrain::default();

    let error = manager
        .prepare_policy_epoch(
            outside_range_epoch,
            envelope,
            1_000,
            &mut drain,
            &mut RecordingRevaluator::compliant(),
        )
        .expect_err("syntactically valid but out-of-envelope value must fail closed");

    assert!(matches!(
        error,
        PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::TerminalCashFlowOutOfRange { .. }
        )
    ));
    assert_eq!(
        drain.calls, 0,
        "envelope rejection happens before venue drain or activation"
    );
    let snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable");
    assert!(snapshot.active_epoch.is_none());
    assert!(snapshot.risk_increasing_admission_enabled);
}

#[test]
fn s6a_band_coverage_attestation_fails_closed_until_s6b_verifier_exists() {
    let pool_id = "epoch-attestation-pool";
    let (_store, _owner, manager) = epoch_context(pool_id, "epoch-attestation-owner");
    let envelope = envelope(pool_id, ["classifier-attested"]);
    let mut attested_epoch = epoch(
        pool_id,
        "epoch-attested",
        "descriptor-attested",
        "classifier-attested",
        10,
    );
    attested_epoch
        .declared_attestations
        .push(PreparedEpochAttestation::BandCoverageAttestation {
            attestation_digest: "attestation-digest".to_string(),
        });

    let error = manager
        .prepare_policy_epoch(
            attested_epoch,
            envelope,
            1_000,
            &mut RecordingDrain::default(),
            &mut RecordingRevaluator::compliant(),
        )
        .expect_err("S6a must not pass unverified band coverage attestations");

    assert_eq!(
        error,
        PolicyEpochPrepareError::AttestationVerificationUnavailable
    );
}

#[test]
fn s6a_non_compliant_current_exposure_activates_no_new_risk_but_allows_safety_action() {
    let pool_id = "epoch-non-compliant-pool";
    let (_store, owner, manager) = epoch_context(pool_id, "epoch-non-compliant-owner");
    let envelope = envelope(pool_id, ["classifier-new"]);
    let new_epoch = epoch(pool_id, "epoch-new", "descriptor-new", "classifier-new", 10);
    let prepared = manager
        .prepare_policy_epoch(
            new_epoch,
            envelope,
            1_000,
            &mut RecordingDrain::default(),
            &mut RecordingRevaluator::non_compliant(),
        )
        .expect("non-compliant exposure is an activation policy, not a prepare error");

    manager
        .activate_prepared_epoch(prepared)
        .expect("cutover should activate with no-new-risk state");
    let snapshot = owner
        .policy_epoch_snapshot()
        .expect("policy state should be readable");
    assert_epoch_snapshot(&snapshot, "epoch-new", "classifier-new", "descriptor-new");
    assert!(!snapshot.risk_increasing_admission_enabled);
    assert!(snapshot.safety_action_enabled);

    let service = AdmissionService::new(owner.clone());
    let view = published_view_for_epoch(pool_id, snapshot.risk_state_version);
    let reserve_error = service
        .compare_and_reserve(
            &view,
            admission_candidate(pool_id, snapshot.risk_state_version, "epoch-new"),
            BoundReusableSafetyState {
                risk_state_version: snapshot.risk_state_version,
                kill_switch_latched: false,
                loss_governor_halted: false,
            },
            None,
            1_001,
        )
        .expect_err("risk-increasing admission must be disabled after non-compliant cutover");
    assert_eq!(
        reserve_error,
        AdmissionReserveError::RiskIncreasingAdmissionDisabled
    );

    let safety = service
        .admit_safety_action(
            &view,
            SafetyActionAdmissionRequest {
                action_id: "safety-action".to_string(),
                action: SafetyAction::ReduceOnlyCloseExistingPosition {
                    position_id: "position-to-reduce".to_string(),
                },
                safety_state: BoundReusableSafetyState {
                    risk_state_version: snapshot.risk_state_version,
                    kill_switch_latched: false,
                    loss_governor_halted: false,
                },
                after: RiskExposureSetInput {
                    risk_state_version: snapshot.risk_state_version,
                    exposures: Vec::new(),
                },
                proof_domain: SafetyActionProofDomain {
                    max_exposure_count: 1,
                    before_exposure_count: 0,
                    after_exposure_count: 0,
                },
            },
        )
        .expect("SafetyAction must remain enabled in no-new-risk state");
    assert_eq!(
        safety.source_risk_state_version,
        snapshot.risk_state_version
    );
}

#[derive(Default)]
struct RecordingDrain {
    calls: usize,
}

impl VenueEventDrain for RecordingDrain {
    fn drain_queued_venue_events(&mut self) -> Result<VenueEventDrainReport, VenueEventDrainError> {
        self.calls += 1;
        Ok(VenueEventDrainReport {
            drained_event_count: 0,
        })
    }
}

struct RecordingRevaluator {
    outcome: RevaluationOutcome,
}

enum RevaluationOutcome {
    Compliant,
    NonCompliant,
    PartialFailure(String),
}

impl RecordingRevaluator {
    fn compliant() -> Self {
        Self {
            outcome: RevaluationOutcome::Compliant,
        }
    }

    fn non_compliant() -> Self {
        Self {
            outcome: RevaluationOutcome::NonCompliant,
        }
    }

    fn partial_failure(item_id: &str) -> Self {
        Self {
            outcome: RevaluationOutcome::PartialFailure(item_id.to_string()),
        }
    }
}

impl PreparedEpochRevaluator for RecordingRevaluator {
    fn revalue_under_prepared_epoch(
        &mut self,
        input: PreparedEpochRevaluationInput<'_>,
    ) -> Result<PostCutoverAdmissionState, PolicyEpochRevaluationError> {
        assert!(!input.prepared_epoch.descriptor_map.is_empty());
        assert_eq!(input.drain_report.drained_event_count, 0);
        match &self.outcome {
            RevaluationOutcome::Compliant => Ok(PostCutoverAdmissionState {
                current_exposure_compliant: true,
            }),
            RevaluationOutcome::NonCompliant => Ok(PostCutoverAdmissionState {
                current_exposure_compliant: false,
            }),
            RevaluationOutcome::PartialFailure(item_id) => {
                Err(PolicyEpochRevaluationError::PartialFailure {
                    revalued_item_count: 1,
                    failed_item_id: item_id.clone(),
                })
            }
        }
    }
}

fn epoch_context(
    pool_id: &str,
    owner_id: &str,
) -> (FencedRiskStateStore, RiskStateOwner, EpochManager) {
    let lease_authority = ConfiguredLeaseAuthority::new(
        LeaseAuthorityBackend::DynamoDbConditionalWrite,
        format!("{pool_id}-lease-authority"),
    )
    .expect("lease authority dependency should be valid");
    let store = FencedRiskStateStore::new(lease_authority);
    let owner = RiskStateOwner::acquire(
        store.clone(),
        PoolId::new(pool_id).expect("pool id should be valid"),
        owner_id,
    )
    .expect("risk state owner should acquire pool");
    owner
        .reconcile_before_new_risk()
        .expect("owner should reconcile before epoch activation");
    let manager = EpochManager::new(owner.clone());
    (store, owner, manager)
}

fn envelope<const N: usize>(pool_id: &str, classifier_versions: [&str; N]) -> SafetyPolicyEnvelope {
    SafetyPolicyEnvelope {
        envelope_id: "envelope".to_string(),
        envelope_version: "envelope-version".to_string(),
        environment: "test-environment".to_string(),
        pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
        ranges: SafetyPolicyEnvelopeRanges {
            max_descriptor_count: 4,
            max_terminal_states_per_descriptor: 4,
            min_terminal_cash_flow: dec(-100),
            max_terminal_cash_flow: dec(100),
            max_sizing_policy_versions: 4,
            max_activation_horizon_unix_nanos: 100,
        },
        permitted_model_versions: BTreeSet::from(["model-version".to_string()]),
        permitted_fallback_model_versions: BTreeSet::from(["fallback-version".to_string()]),
        permitted_classifier_versions: classifier_versions.into_iter().map(String::from).collect(),
        permitted_fee_model_versions: BTreeSet::from(["fee-version".to_string()]),
        permitted_sizing_policy_versions: BTreeSet::from(["sizing-version".to_string()]),
        required_approval_ids: BTreeSet::from(["approval".to_string()]),
        required_approval_digest: "approval-digest".to_string(),
        invariants: BTreeSet::from([SafetyEnvelopeInvariant::DescriptorPolicyEpochMatchesBundle]),
    }
}

fn epoch(
    pool_id: &str,
    epoch_id: &str,
    descriptor_version: &str,
    classifier_version: &str,
    terminal_cash_flow: i64,
) -> PreparedPolicyEpoch {
    PreparedPolicyEpoch {
        epoch_id: epoch_id.to_string(),
        environment: "test-environment".to_string(),
        pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
        policy_digest: "policy-digest".to_string(),
        descriptor_map_digest: "descriptor-map-digest".to_string(),
        descriptor_map: BTreeMap::from([(
            "candidate-instrument".to_string(),
            PreparedEpochDescriptor {
                active_descriptor: ActiveDescriptorView {
                    instrument_id: "candidate-instrument".to_string(),
                    descriptor_version: descriptor_version.to_string(),
                    policy_epoch_id: epoch_id.to_string(),
                    terminal_state_ids: vec![
                        "terminal-loss".to_string(),
                        "terminal-gain".to_string(),
                    ],
                    terminal_cash_flows: vec![dec(terminal_cash_flow), dec(99)],
                },
                descriptor_attributes: RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
                    "descriptor_risk_class".to_string(),
                    "alpha".to_string(),
                )]))
                .expect("descriptor attributes should be valid"),
            },
        )]),
        classifier_version: classifier_version.to_string(),
        classification_policy: classification_policy(),
        model_version: "model-version".to_string(),
        fallback_model_version: "fallback-version".to_string(),
        fee_model_version: "fee-version".to_string(),
        sizing_policy_versions: vec!["sizing-version".to_string()],
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

fn assert_epoch_snapshot(
    snapshot: &bolt_v2::bolt_v3_risk_reservation_substrate::state_owner::PolicyEpochSnapshot,
    epoch_id: &str,
    classifier_version: &str,
    descriptor_version: &str,
) {
    let active = snapshot
        .active_epoch
        .as_ref()
        .expect("active epoch should be present");
    assert_eq!(active.epoch_id, epoch_id);
    assert_eq!(active.classifier_version, classifier_version);
    assert_eq!(
        active
            .descriptor_map
            .get("candidate-instrument")
            .map(|descriptor| descriptor.active_descriptor.descriptor_version.as_str()),
        Some(descriptor_version)
    );
}

fn published_view_for_epoch(
    pool_id: &str,
    risk_state_version: RiskStateVersion,
) -> PublishedRiskView {
    published_view_for_epoch_descriptor(pool_id, risk_state_version, "epoch-new", "descriptor-new")
}

fn published_view_for_epoch_descriptor(
    pool_id: &str,
    risk_state_version: RiskStateVersion,
    policy_epoch_id: &str,
    descriptor_version: &str,
) -> PublishedRiskView {
    let bucket = bucket();
    RiskViewPublisher::publish(RiskViewPublicationInput {
        sizing_view: RiskSizingView {
            risk_state_version,
            reconciliation_ready: true,
            reference_growth_wealth: dec(100),
            conservative_liquidation_equity: dec(100),
            free_collateral: dec(100),
            equity_floor_headroom: dec(100),
            governor_headroom: dec(100),
            global_stress_loss_headroom: dec(100),
            bucket_stress_loss_headrooms: BTreeMap::from([(bucket.clone(), dec(100))]),
            open_order_headroom: 1,
            position_quantity_headroom: dec(100),
        },
        active_descriptor: ActiveDescriptorView {
            instrument_id: "candidate-instrument".to_string(),
            descriptor_version: descriptor_version.to_string(),
            policy_epoch_id: policy_epoch_id.to_string(),
            terminal_state_ids: vec!["terminal-loss".to_string(), "terminal-gain".to_string()],
            terminal_cash_flows: vec![dec(10), dec(99)],
        },
        descriptor_attributes: RiskDescriptorCanonicalAttributes::new(BTreeMap::from([(
            "descriptor_risk_class".to_string(),
            bucket.bucket_value().to_string(),
        )]))
        .expect("descriptor attributes should be valid"),
        classification_policy: classification_policy(),
        caller_declared_buckets: Vec::new(),
        portfolio: RiskPortfolioSnapshot {
            positions: Vec::<RiskExposure>::new(),
        },
        portfolio_scope_id: pool_id.to_string(),
    })
    .expect("published view should be valid")
}

fn admission_candidate(
    pool_id: &str,
    risk_state_version: RiskStateVersion,
    policy_epoch_id: &str,
) -> AdmissionCandidate {
    let preview = RiskPreviewInput {
        pool_id: PoolId::new(pool_id).expect("pool id should be valid"),
        instrument_id: "candidate-instrument".to_string(),
        model_risk_scope: ModelRiskEvaluationScope::CandidateInstrument {
            instrument_id: "candidate-instrument".to_string(),
        },
        side: "long".to_string(),
        quantity: dec(1),
        order_type: "limit".to_string(),
        time_in_force: "gtc".to_string(),
        max_unit_price: Some(dec(2)),
        max_cash_outlay: dec(2),
        source_view_version: risk_state_version,
        policy_epoch_id: policy_epoch_id.to_string(),
    };
    AdmissionCandidate {
        intent_id: "intent".to_string(),
        idempotency_key: "idempotency".to_string(),
        pool_id: preview.pool_id,
        instrument_id: preview.instrument_id,
        model_risk_scope: preview.model_risk_scope,
        expected_descriptor_version: "descriptor-new".to_string(),
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

fn bucket() -> ConcentrationBucket {
    ConcentrationBucket::new("risk_class", "alpha").expect("bucket should be valid")
}

fn classification_policy() -> RiskClassificationPolicy {
    RiskClassificationPolicy::new(vec![
        ConcentrationBucketDimension::new("risk_class", "descriptor_risk_class")
            .expect("dimension should be valid"),
    ])
    .expect("classification policy should be valid")
}

fn dec(value: i64) -> Decimal {
    Decimal::new(value, 0)
}
