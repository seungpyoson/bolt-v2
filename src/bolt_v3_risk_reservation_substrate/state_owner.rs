use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use nautilus_model::identifiers::ClientOrderId;

use crate::bolt_v3_risk_reservation_substrate::contracts::{
    ConfiguredLeaseAuthority, DurableSubmissionIntent, FencingToken, LiveSubmissionRecord, OwnerId,
    PoolId, PoolOwnershipLease, PreparedPolicyEpoch, ReservationLifecycleState, RiskStateVersion,
};
use crate::bolt_v3_risk_reservation_substrate::{
    reservation_ledger::{
        RiskReservationCommit, RiskReservationError, RiskReservationRejection,
        RiskReservationTotals, RiskReservationTransaction, SubstrateReservationRecord,
        build_admission_token, evaluate_stateful_caps,
    },
    risk_classifier::ConcentrationBucket,
    risk_kernel::RiskKernel,
};

#[derive(Debug, Clone)]
pub struct FencedRiskStateStore {
    inner: Arc<Mutex<FencedRiskStateStoreInner>>,
    lease_authority: ConfiguredLeaseAuthority,
}

#[derive(Debug)]
struct FencedRiskStateStoreInner {
    leases: BTreeMap<PoolId, LeaseRecord>,
    versions: BTreeMap<PoolId, RiskStateVersion>,
    reconciled: BTreeMap<PoolId, bool>,
    mutations: Vec<DurableRiskMutationRecord>,
    reservation_totals: BTreeMap<PoolId, RiskReservationTotals>,
    reservation_records: Vec<SubstrateReservationRecord>,
    idempotent_reservations: BTreeMap<(PoolId, String), IdempotentReservationRecord>,
    consumed_permits: BTreeMap<String, String>,
    submission_intents: BTreeMap<(PoolId, String), DurableSubmissionIntent>,
    live_submission_records: BTreeMap<(PoolId, String), LiveSubmissionRecord>,
    policy_epoch_states: BTreeMap<PoolId, ActivePolicyEpochState>,
}

impl FencedRiskStateStoreInner {
    fn new() -> Self {
        Self {
            leases: BTreeMap::new(),
            versions: BTreeMap::new(),
            reconciled: BTreeMap::new(),
            mutations: Vec::new(),
            reservation_totals: BTreeMap::new(),
            reservation_records: Vec::new(),
            idempotent_reservations: BTreeMap::new(),
            consumed_permits: BTreeMap::new(),
            submission_intents: BTreeMap::new(),
            live_submission_records: BTreeMap::new(),
            policy_epoch_states: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActivePolicyEpochState {
    active_epoch: Option<PreparedPolicyEpoch>,
    bound_band_coverage_attestation_digests: Vec<String>,
    risk_increasing_admission_enabled: bool,
    safety_action_enabled: bool,
    alerts: Vec<PolicyEpochAlert>,
}

impl ActivePolicyEpochState {
    fn no_policy_loaded() -> Self {
        Self {
            active_epoch: None,
            bound_band_coverage_attestation_digests: Vec::new(),
            risk_increasing_admission_enabled: true,
            safety_action_enabled: true,
            alerts: Vec::new(),
        }
    }

    fn snapshot(&self, risk_state_version: RiskStateVersion) -> PolicyEpochSnapshot {
        PolicyEpochSnapshot {
            risk_state_version,
            active_epoch: self.active_epoch.clone(),
            bound_band_coverage_attestation_digests: self
                .bound_band_coverage_attestation_digests
                .clone(),
            risk_increasing_admission_enabled: self.risk_increasing_admission_enabled,
            safety_action_enabled: self.safety_action_enabled,
            alerts: self.alerts.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEpochSnapshot {
    pub risk_state_version: RiskStateVersion,
    pub active_epoch: Option<PreparedPolicyEpoch>,
    pub bound_band_coverage_attestation_digests: Vec<String>,
    pub risk_increasing_admission_enabled: bool,
    pub safety_action_enabled: bool,
    pub alerts: Vec<PolicyEpochAlert>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEpochAlert {
    pub reason: PolicyEpochAlertReason,
    pub epoch_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyEpochAlertReason {
    PartialRevaluationFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IdempotentReservationRecord {
    commit: RiskReservationCommit,
    permit_id: String,
    candidate_digest: String,
    intent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LeaseRecord {
    owner_id: OwnerId,
    fencing_token: FencingToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableRiskMutationRecord {
    pub pool_id: PoolId,
    pub fencing_token: FencingToken,
    pub mutation: DurableRiskMutation,
    pub risk_state_version: RiskStateVersion,
}

impl FencedRiskStateStore {
    pub fn new(lease_authority: ConfiguredLeaseAuthority) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FencedRiskStateStoreInner::new())),
            lease_authority,
        }
    }

    pub fn acquire_lease(
        &self,
        pool_id: PoolId,
        owner_id: impl Into<String>,
    ) -> Result<PoolOwnershipLease, RiskStateMutationError> {
        let owner_id = OwnerId::new(owner_id).map_err(|_| RiskStateMutationError::InvalidOwner)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        let fencing_token = inner
            .leases
            .get(&pool_id)
            .map_or_else(
                || Ok(FencingToken::initial()),
                |lease| lease.fencing_token.next(),
            )
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;

        inner.leases.insert(
            pool_id.clone(),
            LeaseRecord {
                owner_id: owner_id.clone(),
                fencing_token,
            },
        );
        inner.reconciled.insert(pool_id.clone(), false);
        inner
            .versions
            .entry(pool_id.clone())
            .or_insert(RiskStateVersion::zero());

        Ok(PoolOwnershipLease::new(
            pool_id,
            owner_id,
            fencing_token,
            self.lease_authority.clone(),
        ))
    }

    fn mark_reconciled(
        &self,
        lease: &PoolOwnershipLease,
    ) -> Result<RiskStateVersion, RiskStateMutationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        inner.reconciled.insert(lease.pool_id().clone(), true);
        Ok(*inner
            .versions
            .entry(lease.pool_id().clone())
            .or_insert(RiskStateVersion::zero()))
    }

    pub fn commit_durable_mutation(
        &self,
        lease: &PoolOwnershipLease,
        mutation: DurableRiskMutation,
    ) -> Result<RiskStateVersion, RiskStateMutationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        if !inner
            .reconciled
            .get(lease.pool_id())
            .copied()
            .unwrap_or(false)
        {
            return Err(RiskStateMutationError::ReconciliationRequired);
        }
        if mutation.mutation_id.trim().is_empty() {
            return Err(RiskStateMutationError::InvalidMutation);
        }

        let version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero)
            .next()
            .map_err(|_| RiskStateMutationError::VersionOverflow)?;
        inner.versions.insert(lease.pool_id().clone(), version);
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation,
            risk_state_version: version,
        });
        Ok(version)
    }

    pub fn commit_safety_action(
        &self,
        lease: &PoolOwnershipLease,
        mutation_id: impl Into<String>,
        source_risk_state_version: RiskStateVersion,
    ) -> Result<RiskStateVersion, RiskStateMutationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        let mutation_id = mutation_id.into();
        if mutation_id.trim().is_empty() {
            return Err(RiskStateMutationError::InvalidMutation);
        }
        let current_version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero);
        if inner
            .policy_epoch_states
            .get(lease.pool_id())
            .is_some_and(|state| !state.safety_action_enabled)
        {
            return Err(RiskStateMutationError::SafetyActionDisabled);
        }
        if current_version != source_risk_state_version {
            return Err(RiskStateMutationError::StaleRiskStateVersion);
        }

        let version = current_version
            .next()
            .map_err(|_| RiskStateMutationError::VersionOverflow)?;
        inner.versions.insert(lease.pool_id().clone(), version);
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(mutation_id, RiskMutationKind::SafetyAction),
            risk_state_version: version,
        });
        Ok(version)
    }

    pub fn durable_mutation_records(
        &self,
    ) -> Result<Vec<DurableRiskMutationRecord>, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        Ok(inner.mutations.clone())
    }

    fn policy_epoch_snapshot(
        &self,
        lease: &PoolOwnershipLease,
    ) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        let risk_state_version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero);
        Ok(inner
            .policy_epoch_states
            .get(lease.pool_id())
            .cloned()
            .unwrap_or_else(ActivePolicyEpochState::no_policy_loaded)
            .snapshot(risk_state_version))
    }

    fn commit_policy_epoch_cutover(
        &self,
        lease: &PoolOwnershipLease,
        active_epoch: PreparedPolicyEpoch,
        expected_version: RiskStateVersion,
        bound_band_coverage_attestation_digests: Vec<String>,
        risk_increasing_admission_enabled: bool,
        safety_action_enabled: bool,
    ) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        if !inner
            .reconciled
            .get(lease.pool_id())
            .copied()
            .unwrap_or(false)
        {
            return Err(RiskStateMutationError::ReconciliationRequired);
        }
        let current_version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero);
        if current_version != expected_version {
            return Err(RiskStateMutationError::StaleRiskStateVersion);
        }

        let version = current_version
            .next()
            .map_err(|_| RiskStateMutationError::VersionOverflow)?;
        inner.versions.insert(lease.pool_id().clone(), version);
        let state = ActivePolicyEpochState {
            active_epoch: Some(active_epoch.clone()),
            bound_band_coverage_attestation_digests,
            risk_increasing_admission_enabled,
            safety_action_enabled,
            alerts: inner
                .policy_epoch_states
                .get(lease.pool_id())
                .map_or_else(Vec::new, |state| state.alerts.clone()),
        };
        inner
            .policy_epoch_states
            .insert(lease.pool_id().clone(), state.clone());
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(
                active_epoch.epoch_id,
                RiskMutationKind::PolicyEpoch,
            ),
            risk_state_version: version,
        });
        Ok(state.snapshot(version))
    }

    fn commit_policy_epoch_no_new_risk_alert(
        &self,
        lease: &PoolOwnershipLease,
        epoch_id: String,
        reason: PolicyEpochAlertReason,
    ) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        let version = next_pool_version(&mut inner, lease.pool_id())?;
        let mut state = inner
            .policy_epoch_states
            .get(lease.pool_id())
            .cloned()
            .unwrap_or_else(ActivePolicyEpochState::no_policy_loaded);
        state.risk_increasing_admission_enabled = false;
        state.safety_action_enabled = true;
        state.alerts.push(PolicyEpochAlert {
            reason,
            epoch_id: epoch_id.clone(),
        });
        inner
            .policy_epoch_states
            .insert(lease.pool_id().clone(), state.clone());
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(epoch_id, RiskMutationKind::PolicyEpoch),
            risk_state_version: version,
        });
        Ok(state.snapshot(version))
    }

    pub fn reservation_records(
        &self,
    ) -> Result<Vec<SubstrateReservationRecord>, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        Ok(inner.reservation_records.clone())
    }

    pub fn reserved_bucket_stress_loss(
        &self,
        pool_id: &PoolId,
        bucket: &ConcentrationBucket,
    ) -> Result<rust_decimal::Decimal, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        Ok(inner
            .reservation_totals
            .get(pool_id)
            .map(|totals| totals.reserved_bucket_stress_loss(bucket))
            .unwrap_or(rust_decimal::Decimal::ZERO))
    }

    fn prepare_submission_intent(
        &self,
        lease: &PoolOwnershipLease,
        reservation: &RiskReservationCommit,
        client_order_id: ClientOrderId,
        now_unix_nanos: u64,
    ) -> Result<DurableSubmissionIntent, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        if !inner
            .reconciled
            .get(lease.pool_id())
            .copied()
            .unwrap_or(false)
        {
            return Err(RiskSubmissionMutationError::State(
                RiskStateMutationError::ReconciliationRequired,
            ));
        }

        let idempotency_key = reservation.admission_token.token_id.clone();
        let map_key = (lease.pool_id().clone(), idempotency_key.clone());
        if let Some(existing) = inner.submission_intents.get(&map_key) {
            if existing.admission_token == reservation.admission_token
                && existing.client_order_id == client_order_id
            {
                return Ok(existing.clone());
            }
            return Err(RiskSubmissionMutationError::SubmissionIntentConflict);
        }

        let record_index = matching_reservation_record_index(
            &inner.reservation_records,
            lease.pool_id(),
            &reservation.admission_token.reservation_id,
        )
        .ok_or(RiskSubmissionMutationError::UnknownReservation)?;
        if inner.reservation_records[record_index].admission_token != reservation.admission_token {
            return Err(RiskSubmissionMutationError::AdmissionTokenMismatch);
        }
        if inner.reservation_records[record_index].lifecycle_state
            != ReservationLifecycleState::Reserved
        {
            return Err(RiskSubmissionMutationError::ReservationNotReserved);
        }
        let instrument_id = inner.reservation_records[record_index]
            .instrument_id
            .clone();

        let submitted_version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero)
            .next()
            .map_err(|_| {
                RiskSubmissionMutationError::State(RiskStateMutationError::VersionOverflow)
            })?;
        inner.reservation_records[record_index].lifecycle_state =
            ReservationLifecycleState::Submitted;
        inner
            .versions
            .insert(lease.pool_id().clone(), submitted_version);
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(
                idempotency_key.clone(),
                RiskMutationKind::Submission,
            ),
            risk_state_version: submitted_version,
        });

        let intent = DurableSubmissionIntent {
            admission_token: reservation.admission_token.clone(),
            client_order_id,
            instrument_id,
            persisted_at_unix_nanos: now_unix_nanos,
            submitted_risk_state_version: submitted_version,
        };
        inner.submission_intents.insert(map_key, intent.clone());
        Ok(intent)
    }

    fn durable_submission_intents(
        &self,
        lease: &PoolOwnershipLease,
    ) -> Result<Vec<DurableSubmissionIntent>, RiskSubmissionMutationError> {
        let inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        Ok(inner
            .submission_intents
            .iter()
            .filter_map(|((pool_id, _), intent)| {
                (pool_id == lease.pool_id()).then_some(intent.clone())
            })
            .collect())
    }

    fn durable_submission_intent(
        &self,
        lease: &PoolOwnershipLease,
        idempotency_key: &str,
    ) -> Result<DurableSubmissionIntent, RiskSubmissionMutationError> {
        let inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        inner
            .submission_intents
            .get(&(lease.pool_id().clone(), idempotency_key.to_string()))
            .cloned()
            .ok_or(RiskSubmissionMutationError::UnknownSubmissionIntent)
    }

    fn live_submission_record(
        &self,
        lease: &PoolOwnershipLease,
        idempotency_key: &str,
    ) -> Result<Option<LiveSubmissionRecord>, RiskSubmissionMutationError> {
        let inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        Ok(inner
            .live_submission_records
            .get(&(lease.pool_id().clone(), idempotency_key.to_string()))
            .cloned())
    }

    fn live_submission_records(
        &self,
        lease: &PoolOwnershipLease,
    ) -> Result<Vec<LiveSubmissionRecord>, RiskSubmissionMutationError> {
        let inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        Ok(inner
            .live_submission_records
            .iter()
            .filter_map(|((pool_id, _), record)| {
                (pool_id == lease.pool_id()).then_some(record.clone())
            })
            .collect())
    }

    fn record_live_submission(
        &self,
        lease: &PoolOwnershipLease,
        idempotency_key: &str,
        record: LiveSubmissionRecord,
    ) -> Result<LiveSubmissionRecord, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        let map_key = (lease.pool_id().clone(), idempotency_key.to_string());
        if let Some(existing) = inner.live_submission_records.get(&map_key) {
            if *existing == record {
                return Ok(existing.clone());
            }
            return Err(RiskSubmissionMutationError::SubmissionIntentConflict);
        }
        inner
            .live_submission_records
            .insert(map_key, record.clone());
        Ok(record)
    }

    fn complete_reconciliation(
        &self,
        lease: &PoolOwnershipLease,
    ) -> Result<RiskStateVersion, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        let version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero)
            .next()
            .map_err(|_| {
                RiskSubmissionMutationError::State(RiskStateMutationError::VersionOverflow)
            })?;
        inner.versions.insert(lease.pool_id().clone(), version);
        inner.reconciled.insert(lease.pool_id().clone(), true);
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(
                version.get().to_string(),
                RiskMutationKind::Reconciliation,
            ),
            risk_state_version: version,
        });
        Ok(version)
    }

    fn compare_and_reserve(
        &self,
        lease: &PoolOwnershipLease,
        transaction: RiskReservationTransaction,
    ) -> Result<RiskReservationCommit, RiskReservationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskReservationError::StateMutation(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskReservationError::StateMutation)?;
        if !inner
            .reconciled
            .get(lease.pool_id())
            .copied()
            .unwrap_or(false)
        {
            return Err(RiskReservationError::StateMutation(
                RiskStateMutationError::ReconciliationRequired,
            ));
        }

        if &transaction.candidate.pool_id != lease.pool_id() {
            return Err(RiskReservationError::PoolMismatch);
        }

        let idempotency_key = transaction.candidate.idempotency_key.clone();
        let permit_id = transaction.candidate.sizing_permit.permit_id.clone();
        let candidate_digest = transaction.candidate.sizing_permit.candidate_digest.clone();
        let intent_id = transaction.candidate.intent_id.clone();
        let reservation_key = (lease.pool_id().clone(), idempotency_key.clone());
        if let Some(existing) = inner.idempotent_reservations.get(&reservation_key) {
            if existing.permit_id == permit_id
                && existing.candidate_digest == candidate_digest
                && existing.intent_id == intent_id
            {
                return Ok(existing.commit.clone());
            }
            return Err(RiskReservationError::IdempotencyConflict);
        }
        if inner.consumed_permits.contains_key(&permit_id) {
            return Err(RiskReservationError::PermitAlreadyConsumed);
        }

        let current_version = *inner
            .versions
            .entry(lease.pool_id().clone())
            .or_insert(RiskStateVersion::zero());
        validate_risk_increasing_policy_epoch(
            inner.policy_epoch_states.get(lease.pool_id()),
            &transaction.candidate.policy_epoch_id,
        )?;
        transaction.validate_static(lease.pool_id(), current_version)?;

        let assessment = RiskKernel::evaluate(&transaction.kernel_input)
            .map_err(RiskReservationError::Kernel)?;
        let totals = inner
            .reservation_totals
            .entry(lease.pool_id().clone())
            .or_insert_with(RiskReservationTotals::empty);
        let evaluation = evaluate_stateful_caps(totals, &transaction, &assessment);
        if !evaluation.breached_dimensions.is_empty() {
            return Err(RiskReservationError::Rejected(RiskReservationRejection {
                evaluated_dimensions: evaluation.evaluated_dimensions,
                breached_dimensions: evaluation.breached_dimensions,
                diagnostic_mismatches: evaluation.diagnostic_mismatches,
                token_issued: None,
            }));
        }

        let committed_version = current_version.next().map_err(|_| {
            RiskReservationError::StateMutation(RiskStateMutationError::VersionOverflow)
        })?;
        let token = build_admission_token(&transaction, committed_version);
        totals.apply(&transaction, &assessment);
        inner
            .versions
            .insert(lease.pool_id().clone(), committed_version);
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(
                transaction.candidate.idempotency_key.clone(),
                RiskMutationKind::Reservation,
            ),
            risk_state_version: committed_version,
        });
        inner.reservation_records.push(SubstrateReservationRecord {
            pool_id: lease.pool_id().clone(),
            admission_token: token.clone(),
            instrument_id: transaction.candidate.instrument_id.clone(),
            assessment: assessment.clone(),
            evaluated_dimensions: evaluation.evaluated_dimensions.clone(),
            lifecycle_state: ReservationLifecycleState::Reserved,
        });
        let commit = RiskReservationCommit {
            admission_token: token,
            assessment,
            evaluated_dimensions: evaluation.evaluated_dimensions,
            diagnostic_mismatches: evaluation.diagnostic_mismatches,
        };
        inner
            .consumed_permits
            .insert(permit_id.clone(), idempotency_key.clone());
        inner.idempotent_reservations.insert(
            reservation_key,
            IdempotentReservationRecord {
                commit: commit.clone(),
                permit_id,
                candidate_digest,
                intent_id,
            },
        );

        Ok(commit)
    }
}

#[derive(Debug, Clone)]
pub struct RiskStateOwner {
    store: FencedRiskStateStore,
    lease: PoolOwnershipLease,
}

impl RiskStateOwner {
    pub fn acquire(
        store: FencedRiskStateStore,
        pool_id: PoolId,
        owner_id: impl Into<String>,
    ) -> Result<Self, RiskStateMutationError> {
        let lease = store.acquire_lease(pool_id, owner_id)?;
        Ok(Self { store, lease })
    }

    pub fn lease(&self) -> &PoolOwnershipLease {
        &self.lease
    }

    pub fn owner_id(&self) -> &OwnerId {
        self.lease.owner_id()
    }

    pub fn reconcile_before_new_risk(&self) -> Result<RiskStateVersion, RiskStateMutationError> {
        self.store.mark_reconciled(&self.lease)
    }

    pub fn commit_durable_mutation(
        &self,
        mutation: DurableRiskMutation,
    ) -> Result<RiskStateVersion, RiskStateMutationError> {
        self.store.commit_durable_mutation(&self.lease, mutation)
    }

    pub fn durable_mutation_records(
        &self,
    ) -> Result<Vec<DurableRiskMutationRecord>, RiskStateMutationError> {
        self.store.durable_mutation_records()
    }

    pub fn commit_safety_action(
        &self,
        mutation_id: impl Into<String>,
        source_risk_state_version: RiskStateVersion,
    ) -> Result<RiskStateVersion, RiskStateMutationError> {
        self.store
            .commit_safety_action(&self.lease, mutation_id, source_risk_state_version)
    }

    pub fn policy_epoch_snapshot(&self) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        self.store.policy_epoch_snapshot(&self.lease)
    }

    pub fn commit_policy_epoch_cutover(
        &self,
        active_epoch: PreparedPolicyEpoch,
        expected_version: RiskStateVersion,
        bound_band_coverage_attestation_digests: Vec<String>,
        risk_increasing_admission_enabled: bool,
        safety_action_enabled: bool,
    ) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        self.store.commit_policy_epoch_cutover(
            &self.lease,
            active_epoch,
            expected_version,
            bound_band_coverage_attestation_digests,
            risk_increasing_admission_enabled,
            safety_action_enabled,
        )
    }

    pub fn commit_policy_epoch_no_new_risk_alert(
        &self,
        epoch_id: String,
        reason: PolicyEpochAlertReason,
    ) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        self.store
            .commit_policy_epoch_no_new_risk_alert(&self.lease, epoch_id, reason)
    }

    pub fn compare_and_reserve(
        &self,
        transaction: RiskReservationTransaction,
    ) -> Result<RiskReservationCommit, RiskReservationError> {
        self.store.compare_and_reserve(&self.lease, transaction)
    }

    pub fn reservation_records(
        &self,
    ) -> Result<Vec<SubstrateReservationRecord>, RiskStateMutationError> {
        self.store.reservation_records()
    }

    pub fn reserved_bucket_stress_loss(
        &self,
        bucket: &ConcentrationBucket,
    ) -> Result<rust_decimal::Decimal, RiskStateMutationError> {
        self.store
            .reserved_bucket_stress_loss(self.lease.pool_id(), bucket)
    }

    pub fn prepare_submission_intent(
        &self,
        reservation: &RiskReservationCommit,
        client_order_id: ClientOrderId,
        now_unix_nanos: u64,
    ) -> Result<DurableSubmissionIntent, RiskSubmissionMutationError> {
        self.store.prepare_submission_intent(
            &self.lease,
            reservation,
            client_order_id,
            now_unix_nanos,
        )
    }

    pub fn durable_submission_intents(
        &self,
    ) -> Result<Vec<DurableSubmissionIntent>, RiskSubmissionMutationError> {
        self.store.durable_submission_intents(&self.lease)
    }

    pub fn durable_submission_intent(
        &self,
        idempotency_key: &str,
    ) -> Result<DurableSubmissionIntent, RiskSubmissionMutationError> {
        self.store
            .durable_submission_intent(&self.lease, idempotency_key)
    }

    pub fn live_submission_record(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<LiveSubmissionRecord>, RiskSubmissionMutationError> {
        self.store
            .live_submission_record(&self.lease, idempotency_key)
    }

    pub fn live_submission_records(
        &self,
    ) -> Result<Vec<LiveSubmissionRecord>, RiskSubmissionMutationError> {
        self.store.live_submission_records(&self.lease)
    }

    pub fn record_live_submission(
        &self,
        idempotency_key: &str,
        record: LiveSubmissionRecord,
    ) -> Result<LiveSubmissionRecord, RiskSubmissionMutationError> {
        self.store
            .record_live_submission(&self.lease, idempotency_key, record)
    }

    pub fn complete_reconciliation(&self) -> Result<RiskStateVersion, RiskSubmissionMutationError> {
        self.store.complete_reconciliation(&self.lease)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableRiskMutation {
    mutation_id: String,
    kind: RiskMutationKind,
}

impl DurableRiskMutation {
    pub fn new(mutation_id: impl Into<String>, kind: RiskMutationKind) -> Self {
        Self {
            mutation_id: mutation_id.into(),
            kind,
        }
    }

    pub fn mutation_id(&self) -> &str {
        &self.mutation_id
    }

    pub const fn kind(&self) -> RiskMutationKind {
        self.kind
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskMutationKind {
    Reservation,
    Submission,
    Reconciliation,
    SafetyAction,
    PolicyEpoch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskStateMutationError {
    InvalidOwner,
    InvalidMutation,
    AmbiguousLeaseState,
    StaleFencingToken,
    StaleRiskStateVersion,
    ReconciliationRequired,
    SafetyActionDisabled,
    VersionOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskSubmissionMutationError {
    State(RiskStateMutationError),
    UnknownReservation,
    UnknownSubmissionIntent,
    AdmissionTokenMismatch,
    ReservationNotReserved,
    SubmissionIntentConflict,
}

fn validate_lease(
    inner: &FencedRiskStateStoreInner,
    lease: &PoolOwnershipLease,
    lease_authority: &ConfiguredLeaseAuthority,
) -> Result<(), RiskStateMutationError> {
    if lease.lease_authority() != lease_authority {
        return Err(RiskStateMutationError::AmbiguousLeaseState);
    }
    let Some(current) = inner.leases.get(lease.pool_id()) else {
        return Err(RiskStateMutationError::StaleFencingToken);
    };
    if &current.owner_id != lease.owner_id() || current.fencing_token != lease.fencing_token() {
        return Err(RiskStateMutationError::StaleFencingToken);
    }
    Ok(())
}

fn next_pool_version(
    inner: &mut FencedRiskStateStoreInner,
    pool_id: &PoolId,
) -> Result<RiskStateVersion, RiskStateMutationError> {
    let version = inner
        .versions
        .get(pool_id)
        .copied()
        .unwrap_or_else(RiskStateVersion::zero)
        .next()
        .map_err(|_| RiskStateMutationError::VersionOverflow)?;
    inner.versions.insert(pool_id.clone(), version);
    Ok(version)
}

fn validate_risk_increasing_policy_epoch(
    state: Option<&ActivePolicyEpochState>,
    candidate_policy_epoch_id: &str,
) -> Result<(), RiskReservationError> {
    let Some(state) = state else {
        return Ok(());
    };
    if !state.risk_increasing_admission_enabled {
        return Err(RiskReservationError::RiskIncreasingAdmissionDisabled);
    }
    let Some(active_epoch) = &state.active_epoch else {
        return Ok(());
    };
    if active_epoch.epoch_id.as_str() != candidate_policy_epoch_id {
        return Err(RiskReservationError::ActivePolicyEpochMismatch {
            active_policy_epoch_id: active_epoch.epoch_id.clone(),
            candidate_policy_epoch_id: candidate_policy_epoch_id.to_string(),
        });
    }
    Ok(())
}

fn matching_reservation_record_index(
    records: &[SubstrateReservationRecord],
    pool_id: &PoolId,
    reservation_id: &str,
) -> Option<usize> {
    records.iter().position(|record| {
        &record.pool_id == pool_id && record.admission_token.reservation_id == reservation_id
    })
}
