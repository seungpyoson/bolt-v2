use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use nautilus_model::identifiers::ClientOrderId;
use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::contracts::{
    ConfiguredLeaseAuthority, DurableSubmissionIntent, FencingToken, LiveSubmissionRecord, OwnerId,
    PoolId, PoolOwnershipLease, PreparedPolicyEpoch, ReservationLifecycleState,
    RiskReservationOfferedLoadEnvelope, RiskReservationSubstrateConfig, RiskReservationWorkBounds,
    RiskStateVersion,
};
use crate::bolt_v3_risk_reservation_substrate::{
    reservation_ledger::{
        LifecycleReconciliationEventIdentity, LifecycleReconciliationFault,
        LifecycleReconciliationFaultKind, RiskReservationCommit, RiskReservationError,
        RiskReservationRejection, RiskReservationTotals, RiskReservationTransaction,
        SubstrateReservationRecord, build_admission_token, evaluate_stateful_caps,
    },
    risk_classifier::ConcentrationBucket,
    risk_kernel::{RiskExposure, RiskKernel, RiskKernelError},
};

#[derive(Debug, Clone)]
pub struct FencedRiskStateStore {
    inner: Arc<Mutex<FencedRiskStateStoreInner>>,
    lease_authority: ConfiguredLeaseAuthority,
    work_bounds: RiskReservationWorkBounds,
    offered_load_envelope: Option<RiskReservationOfferedLoadEnvelope>,
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
    AdmissionShed,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleMutationResult {
    pub risk_state_version: RiskStateVersion,
    pub lifecycle_state: ReservationLifecycleState,
}

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
impl FencedRiskStateStore {
    pub fn new(config: RiskReservationSubstrateConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FencedRiskStateStoreInner::new())),
            lease_authority: config.pool_lease_authority,
            work_bounds: config.work_bounds,
            offered_load_envelope: config.offered_load_envelope,
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
        let pool_id = lease.pool_id().clone();
        if inner
            .submission_intents
            .keys()
            .any(|(intent_pool_id, _)| intent_pool_id == &pool_id)
        {
            return Err(RiskStateMutationError::ReconciliationRequired);
        }
        let current_version = *inner
            .versions
            .entry(pool_id.clone())
            .or_insert(RiskStateVersion::zero());
        let will_mutate = reconciliation_would_release_reserved_orphans(&inner, &pool_id);
        let next_version = if will_mutate {
            Some(
                current_version
                    .next()
                    .map_err(|_| RiskStateMutationError::VersionOverflow)?,
            )
        } else {
            None
        };
        let mutated = Self::finalize_reconciliation(&mut inner, &pool_id)?;
        if let Some(version) = next_version {
            debug_assert!(mutated);
            inner.versions.insert(pool_id.clone(), version);
            inner.mutations.push(DurableRiskMutationRecord {
                pool_id,
                fencing_token: lease.fencing_token(),
                mutation: DurableRiskMutation::new(
                    version.get().to_string(),
                    RiskMutationKind::Reconciliation,
                ),
                risk_state_version: version,
            });
            return Ok(version);
        }
        debug_assert!(!mutated);
        Ok(current_version)
    }

    fn finalize_reconciliation(
        inner: &mut FencedRiskStateStoreInner,
        pool_id: &PoolId,
    ) -> Result<bool, RiskStateMutationError> {
        if inner.reservation_records.iter().any(|record| {
            &record.pool_id == pool_id
                && (record.lifecycle_state == ReservationLifecycleState::ReconciliationRequired
                    || !record.unresolved_lifecycle_reconciliation_faults.is_empty())
        }) {
            return Err(RiskStateMutationError::ReconciliationRequired);
        }

        if inner.reservation_records.iter().any(|record| {
            &record.pool_id == pool_id
                && record.lifecycle_state != ReservationLifecycleState::Reserved
                && inner
                    .submission_intents
                    .contains_key(&(pool_id.clone(), record.admission_token.token_id.clone()))
                && !inner
                    .live_submission_records
                    .contains_key(&(pool_id.clone(), record.admission_token.token_id.clone()))
        }) {
            return Err(RiskStateMutationError::ReconciliationRequired);
        }

        // FR-041/FR-067: an un-submitted (`Reserved`) reservation has no venue
        // order, so a fenced-out predecessor's orphaned reservation must be
        // released here; otherwise no venue lifecycle event can free it.
        let orphaned: Vec<SubstrateReservationRecord> = inner
            .reservation_records
            .iter()
            .filter(|record| {
                &record.pool_id == pool_id
                    && record.lifecycle_state == ReservationLifecycleState::Reserved
            })
            .cloned()
            .collect();
        if !orphaned.is_empty() {
            {
                let totals = inner
                    .reservation_totals
                    .entry(pool_id.clone())
                    .or_insert_with(RiskReservationTotals::empty);
                for record in &orphaned {
                    totals.release_open_order_remainder(record);
                }
            }
            for record in &orphaned {
                let reservation_key = (pool_id.clone(), record.admission_token.token_id.clone());
                if let Some(idempotent) = inner.idempotent_reservations.remove(&reservation_key)
                    && inner
                        .consumed_permits
                        .get(&idempotent.permit_id)
                        .is_some_and(|idempotency_key| {
                            idempotency_key == &record.admission_token.token_id
                        })
                {
                    inner.consumed_permits.remove(&idempotent.permit_id);
                }
            }
            inner.reservation_records.retain(|record| {
                !(&record.pool_id == pool_id
                    && record.lifecycle_state == ReservationLifecycleState::Reserved)
            });
        }

        inner.reconciled.insert(pool_id.clone(), true);
        Ok(!orphaned.is_empty())
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

    fn bind_initial_policy_epoch(
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
        if &active_epoch.pool_id != lease.pool_id() {
            return Err(RiskStateMutationError::InvalidMutation);
        }
        let current_version = inner
            .versions
            .get(lease.pool_id())
            .copied()
            .unwrap_or_else(RiskStateVersion::zero);
        if current_version != expected_version {
            return Err(RiskStateMutationError::StaleRiskStateVersion);
        }
        if inner
            .policy_epoch_states
            .get(lease.pool_id())
            .is_some_and(|state| state.active_epoch.is_some())
        {
            return Err(RiskStateMutationError::InvalidMutation);
        }
        if inner
            .mutations
            .iter()
            .any(|record| &record.pool_id == lease.pool_id())
            || inner
                .reservation_records
                .iter()
                .any(|record| &record.pool_id == lease.pool_id())
        {
            return Err(RiskStateMutationError::InvalidMutation);
        }

        let state = ActivePolicyEpochState {
            active_epoch: Some(active_epoch),
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
        Ok(state.snapshot(current_version))
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

    fn reservation_record_for_client_order(
        &self,
        lease: &PoolOwnershipLease,
        client_order_id: ClientOrderId,
    ) -> Result<Option<SubstrateReservationRecord>, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        validate_lease(&inner, lease, &self.lease_authority)?;
        Ok(
            matching_reservation_record_index_for_client_order(&inner, lease, client_order_id)
                .ok()
                .map(|record_index| inner.reservation_records[record_index].clone()),
        )
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

    pub fn reserved_risk_totals(
        &self,
        pool_id: &PoolId,
    ) -> Result<RiskReservationTotals, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        Ok(inner
            .reservation_totals
            .get(pool_id)
            .cloned()
            .unwrap_or_else(RiskReservationTotals::empty))
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

    fn apply_order_lifecycle_state(
        &self,
        lease: &PoolOwnershipLease,
        client_order_id: ClientOrderId,
        event_id: &str,
        ts_event_unix_nanos: u64,
        event_sequence: Option<u64>,
        target_state: ReservationLifecycleState,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        validate_lifecycle_mutation_id(event_id)?;

        let record_index =
            matching_reservation_record_index_for_client_order(&inner, lease, client_order_id)?;
        if let Some(result) = lifecycle_event_preflight(
            &mut inner,
            lease,
            record_index,
            LifecycleEventInput {
                event_id,
                ts_event_unix_nanos,
                event_sequence,
                kind: LifecycleReconciliationFaultKind::OrderStatus,
                order_status: Some(target_state),
            },
        )? {
            return Ok(result);
        }
        let current_state = inner.reservation_records[record_index].lifecycle_state;
        if current_state != target_state
            && !order_status_transition_allowed(current_state, target_state)
        {
            return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
        }
        if open_remainder_release_state(target_state)
            && terminal_order_status_has_blocking_fault(
                &inner.reservation_records[record_index],
                ts_event_unix_nanos,
                event_sequence,
            )
        {
            let version = next_pool_version(&mut inner, lease.pool_id())
                .map_err(RiskSubmissionMutationError::State)?;
            let lifecycle_state = {
                let record = &mut inner.reservation_records[record_index];
                record_lifecycle_event_success(
                    record,
                    event_id,
                    ts_event_unix_nanos,
                    event_sequence,
                    LifecycleReconciliationFaultKind::OrderStatus,
                    Some(target_state),
                );
                record.lifecycle_state
            };
            inner.mutations.push(DurableRiskMutationRecord {
                pool_id: lease.pool_id().clone(),
                fencing_token: lease.fencing_token(),
                mutation: DurableRiskMutation::new(
                    event_id.to_string(),
                    RiskMutationKind::Lifecycle,
                ),
                risk_state_version: version,
            });
            return Ok(LifecycleMutationResult {
                risk_state_version: version,
                lifecycle_state,
            });
        }
        let state_changed = current_state != target_state;
        let release_open_remainder = state_changed
            && open_remainder_release_state(target_state)
            && inner.reservation_records[record_index].open_order_remainder_held;
        let release_record = release_open_remainder.then(|| {
            let mut released = inner.reservation_records[record_index].clone();
            released.lifecycle_state = target_state;
            released
        });

        let version = next_pool_version(&mut inner, lease.pool_id())
            .map_err(RiskSubmissionMutationError::State)?;
        {
            let record = &mut inner.reservation_records[record_index];
            record.lifecycle_state = target_state;
            if release_open_remainder {
                record.remaining_fillable_quantity = Decimal::ZERO;
                record.open_order_remainder_held = false;
            }
            record_lifecycle_event_success(
                record,
                event_id,
                ts_event_unix_nanos,
                event_sequence,
                LifecycleReconciliationFaultKind::OrderStatus,
                Some(target_state),
            );
        }
        if let Some(release_record) = release_record {
            inner
                .reservation_totals
                .entry(lease.pool_id().clone())
                .or_insert_with(RiskReservationTotals::empty)
                .release_open_order_remainder(&release_record);
        }
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(event_id.to_string(), RiskMutationKind::Lifecycle),
            risk_state_version: version,
        });
        Ok(LifecycleMutationResult {
            risk_state_version: version,
            lifecycle_state: target_state,
        })
    }

    fn mark_cancel_requested(
        &self,
        lease: &PoolOwnershipLease,
        client_order_id: ClientOrderId,
        mutation_id: &str,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        validate_lifecycle_mutation_id(mutation_id)?;

        let record_index =
            matching_reservation_record_index_for_client_order(&inner, lease, client_order_id)?;
        let current_state = inner.reservation_records[record_index].lifecycle_state;
        if current_state == ReservationLifecycleState::CancelRequested {
            let risk_state_version = inner
                .versions
                .get(lease.pool_id())
                .copied()
                .unwrap_or_else(RiskStateVersion::zero);
            return Ok(LifecycleMutationResult {
                risk_state_version,
                lifecycle_state: ReservationLifecycleState::CancelRequested,
            });
        }
        if !cancel_request_source_state(current_state) {
            return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
        }

        let version = next_pool_version(&mut inner, lease.pool_id())
            .map_err(RiskSubmissionMutationError::State)?;
        inner.reservation_records[record_index].lifecycle_state =
            ReservationLifecycleState::CancelRequested;
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(
                mutation_id.to_string(),
                RiskMutationKind::Lifecycle,
            ),
            risk_state_version: version,
        });
        Ok(LifecycleMutationResult {
            risk_state_version: version,
            lifecycle_state: ReservationLifecycleState::CancelRequested,
        })
    }

    fn apply_authoritative_fill(
        &self,
        lease: &PoolOwnershipLease,
        client_order_id: ClientOrderId,
        event_id: &str,
        ts_event_unix_nanos: u64,
        event_sequence: Option<u64>,
        fill_quantity: Decimal,
        remaining_fillable_quantity: Decimal,
        actual_conservative_liquidation_value: Decimal,
        actual_governor_cost_basis: Decimal,
        terminal_cash_flows: Vec<Decimal>,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        validate_lifecycle_mutation_id(event_id)?;

        let record_index =
            matching_reservation_record_index_for_client_order(&inner, lease, client_order_id)?;
        if let Some(result) = lifecycle_event_preflight(
            &mut inner,
            lease,
            record_index,
            LifecycleEventInput {
                event_id,
                ts_event_unix_nanos,
                event_sequence,
                kind: LifecycleReconciliationFaultKind::Fill,
                order_status: None,
            },
        )? {
            return Ok(result);
        }
        validate_lifecycle_exposure_input(
            fill_quantity,
            remaining_fillable_quantity,
            actual_conservative_liquidation_value,
            actual_governor_cost_basis,
            &terminal_cash_flows,
        )?;
        let record = &inner.reservation_records[record_index];
        if !fill_transition_allowed(record.lifecycle_state) {
            return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
        }
        let old_equity_floor_stress_loss = record.filled_position_equity_floor_stress_loss;
        let old_governor_realized_loss = record.filled_position_governor_realized_loss;
        let mut filled_position_exposure =
            record
                .filled_position_exposure
                .clone()
                .unwrap_or_else(|| RiskExposure {
                    instrument_id: record.instrument_id.clone(),
                    buckets: record.buckets.clone(),
                    quantity: Decimal::ZERO,
                    conservative_liquidation_value: Decimal::ZERO,
                    governor_cost_basis: Decimal::ZERO,
                    terminal_cash_flows: terminal_cash_flows.clone(),
                });
        filled_position_exposure.quantity += fill_quantity;
        filled_position_exposure.conservative_liquidation_value +=
            actual_conservative_liquidation_value;
        filled_position_exposure.governor_cost_basis += actual_governor_cost_basis;
        filled_position_exposure.terminal_cash_flows = terminal_cash_flows;
        let new_equity_floor_stress_loss = monotonic_risk_metric(
            old_equity_floor_stress_loss,
            equity_floor_stress_loss(&filled_position_exposure)?,
        );
        let new_governor_realized_loss = monotonic_risk_metric(
            old_governor_realized_loss,
            governor_realized_loss(&filled_position_exposure)?,
        );
        let target_state = if remaining_fillable_quantity == Decimal::ZERO {
            ReservationLifecycleState::Filled
        } else {
            ReservationLifecycleState::PartiallyFilled
        };
        let buckets = record.buckets.clone();

        let version = next_pool_version(&mut inner, lease.pool_id())
            .map_err(RiskSubmissionMutationError::State)?;
        let record = &mut inner.reservation_records[record_index];
        record.lifecycle_state = target_state;
        record.remaining_fillable_quantity = remaining_fillable_quantity;
        record.filled_position_exposure = Some(filled_position_exposure);
        record.filled_position_equity_floor_stress_loss = new_equity_floor_stress_loss;
        record.filled_position_governor_realized_loss = new_governor_realized_loss;
        record.filled_position_held = true;
        record_lifecycle_event_success(
            record,
            event_id,
            ts_event_unix_nanos,
            event_sequence,
            LifecycleReconciliationFaultKind::Fill,
            None,
        );
        inner
            .reservation_totals
            .entry(lease.pool_id().clone())
            .or_insert_with(RiskReservationTotals::empty)
            .apply_filled_position_risk_delta(
                &buckets,
                old_equity_floor_stress_loss,
                old_governor_realized_loss,
                new_equity_floor_stress_loss,
                new_governor_realized_loss,
            );
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(event_id.to_string(), RiskMutationKind::Lifecycle),
            risk_state_version: version,
        });
        Ok(LifecycleMutationResult {
            risk_state_version: version,
            lifecycle_state: target_state,
        })
    }

    fn apply_settlement_truth(
        &self,
        lease: &PoolOwnershipLease,
        client_order_id: ClientOrderId,
        event_id: &str,
        ts_event_unix_nanos: u64,
        event_sequence: Option<u64>,
        terminal_final: bool,
        reconciliation_complete: bool,
        conservative_liquidation_value: Decimal,
        governor_cost_basis: Decimal,
        terminal_cash_flows: Vec<Decimal>,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        let mut inner = self.inner.lock().map_err(|_| {
            RiskSubmissionMutationError::State(RiskStateMutationError::AmbiguousLeaseState)
        })?;
        validate_lease(&inner, lease, &self.lease_authority)
            .map_err(RiskSubmissionMutationError::State)?;
        validate_lifecycle_mutation_id(event_id)?;

        let record_index =
            matching_reservation_record_index_for_client_order(&inner, lease, client_order_id)?;
        if let Some(result) = lifecycle_event_preflight(
            &mut inner,
            lease,
            record_index,
            LifecycleEventInput {
                event_id,
                ts_event_unix_nanos,
                event_sequence,
                kind: LifecycleReconciliationFaultKind::Settlement,
                order_status: None,
            },
        )? {
            return Ok(result);
        }
        validate_settlement_exposure_input(
            conservative_liquidation_value,
            governor_cost_basis,
            &terminal_cash_flows,
        )?;
        let record = &inner.reservation_records[record_index];
        let settlement_source_state = record.lifecycle_state;
        if !settlement_transition_allowed(settlement_source_state) {
            return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
        }
        let Some(existing_exposure) = record.filled_position_exposure.clone() else {
            return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
        };
        let old_equity_floor_stress_loss = record.filled_position_equity_floor_stress_loss;
        let old_governor_realized_loss = record.filled_position_governor_realized_loss;
        let revised_exposure = RiskExposure {
            conservative_liquidation_value,
            governor_cost_basis,
            terminal_cash_flows,
            ..existing_exposure
        };
        let new_equity_floor_stress_loss = monotonic_risk_metric(
            old_equity_floor_stress_loss,
            equity_floor_stress_loss(&revised_exposure)?,
        );
        let new_governor_realized_loss = monotonic_risk_metric(
            old_governor_realized_loss,
            governor_realized_loss(&revised_exposure)?,
        );
        let target_state = if terminal_final && reconciliation_complete {
            ReservationLifecycleState::Settled
        } else {
            settlement_source_state
        };
        let buckets = record.buckets.clone();
        let release_open_order_remainder =
            target_state == ReservationLifecycleState::Settled && record.open_order_remainder_held;
        let release_filled_position =
            target_state == ReservationLifecycleState::Settled && record.filled_position_held;
        let release_record = (release_open_order_remainder || release_filled_position).then(|| {
            let mut released = record.clone();
            released.filled_position_exposure = Some(revised_exposure.clone());
            released.filled_position_equity_floor_stress_loss = new_equity_floor_stress_loss;
            released.filled_position_governor_realized_loss = new_governor_realized_loss;
            released
        });

        let version = next_pool_version(&mut inner, lease.pool_id())
            .map_err(RiskSubmissionMutationError::State)?;
        let record = &mut inner.reservation_records[record_index];
        record.lifecycle_state = target_state;
        record.filled_position_exposure = Some(revised_exposure);
        record.filled_position_equity_floor_stress_loss = new_equity_floor_stress_loss;
        record.filled_position_governor_realized_loss = new_governor_realized_loss;
        if release_open_order_remainder {
            record.remaining_fillable_quantity = Decimal::ZERO;
            record.open_order_remainder_held = false;
        }
        if release_filled_position {
            record.filled_position_held = false;
        }
        record_lifecycle_event_success(
            record,
            event_id,
            ts_event_unix_nanos,
            event_sequence,
            LifecycleReconciliationFaultKind::Settlement,
            None,
        );
        let totals = inner
            .reservation_totals
            .entry(lease.pool_id().clone())
            .or_insert_with(RiskReservationTotals::empty);
        totals.apply_filled_position_risk_delta(
            &buckets,
            old_equity_floor_stress_loss,
            old_governor_realized_loss,
            new_equity_floor_stress_loss,
            new_governor_realized_loss,
        );
        if let Some(release_record) = &release_record {
            if release_open_order_remainder {
                totals.release_open_order_remainder(release_record);
            }
            if release_filled_position {
                totals.release_filled_position(release_record);
            }
        }
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id: lease.pool_id().clone(),
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(event_id.to_string(), RiskMutationKind::Lifecycle),
            risk_state_version: version,
        });
        Ok(LifecycleMutationResult {
            risk_state_version: version,
            lifecycle_state: target_state,
        })
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
        let pool_id = lease.pool_id().clone();
        let version = inner
            .versions
            .get(&pool_id)
            .copied()
            .unwrap_or_else(RiskStateVersion::zero)
            .next()
            .map_err(|_| {
                RiskSubmissionMutationError::State(RiskStateMutationError::VersionOverflow)
            })?;
        Self::finalize_reconciliation(&mut inner, &pool_id)
            .map_err(RiskSubmissionMutationError::State)?;
        inner.versions.insert(pool_id.clone(), version);
        inner.mutations.push(DurableRiskMutationRecord {
            pool_id,
            fencing_token: lease.fencing_token(),
            mutation: DurableRiskMutation::new(
                version.get().to_string(),
                RiskMutationKind::Reconciliation,
            ),
            risk_state_version: version,
        });
        Ok(version)
    }

    /// Runs the whole FR-001 compare-and-reserve transaction under the pool
    /// state mutex.
    ///
    /// Worst-case complexity: with `P` current positions, at most `B`
    /// configured buckets per exposure, at most `T` configured terminal
    /// cash-flow states per exposure, and `R` indexed live reservation keys,
    /// this critical section runs in `O(P * (B + T) + B + log R)` time. It
    /// performs no external I/O, acquires no nested mutable lock, uses only
    /// pre-resolved immutable descriptor/policy/fee/classifier data carried by
    /// the transaction, and allocates only bounded token/record/index data plus
    /// bounded `B`-sized dimension sets. The configured maximum position,
    /// bucket, and terminal-scenario sizes are enforced before idempotent token
    /// replay and again after the coherent `risk_state_version` check before
    /// the kernel evaluation, so an over-bound transaction reserves nothing.
    /// The offered-load shed gate runs only on risk-increasing compare-and-reserve, uses
    /// the substrate-owned scalar in-flight reservation count, records through the existing
    /// policy alert source, and fails closed before kernel evaluation. The runtime owns the
    /// bounded event queue, fair-queue scheduling, and wall-clock latency policy; this
    /// substrate does not spawn threads, timers, or a second serialization path.
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
        transaction.enforce_work_bounds(&self.work_bounds)?;

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
        if let Some(envelope) = self.offered_load_envelope {
            enforce_offered_load_envelope(&mut inner, lease, &transaction, envelope)?;
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
        transaction.enforce_work_bounds(&self.work_bounds)?;

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
            buckets: transaction.kernel_input.candidate.buckets.clone(),
            assessment: assessment.clone(),
            evaluated_dimensions: evaluation.evaluated_dimensions.clone(),
            lifecycle_state: ReservationLifecycleState::Reserved,
            reserved_order_quantity: transaction.candidate.quantity,
            remaining_fillable_quantity: transaction.candidate.quantity,
            open_order_remainder_held: true,
            filled_position_exposure: None,
            filled_position_equity_floor_stress_loss: Decimal::ZERO,
            filled_position_governor_realized_loss: Decimal::ZERO,
            filled_position_held: false,
            applied_lifecycle_event_ids: BTreeSet::new(),
            unresolved_lifecycle_reconciliation_faults: BTreeMap::new(),
            last_lifecycle_ts_event_unix_nanos: None,
            last_lifecycle_event_sequence: None,
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

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
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

    pub fn bind_initial_policy_epoch(
        &self,
        active_epoch: PreparedPolicyEpoch,
        expected_version: RiskStateVersion,
        bound_band_coverage_attestation_digests: Vec<String>,
        risk_increasing_admission_enabled: bool,
        safety_action_enabled: bool,
    ) -> Result<PolicyEpochSnapshot, RiskStateMutationError> {
        self.store.bind_initial_policy_epoch(
            &self.lease,
            active_epoch,
            expected_version,
            bound_band_coverage_attestation_digests,
            risk_increasing_admission_enabled,
            safety_action_enabled,
        )
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

    pub fn reservation_record_for_client_order(
        &self,
        client_order_id: ClientOrderId,
    ) -> Result<Option<SubstrateReservationRecord>, RiskStateMutationError> {
        self.store
            .reservation_record_for_client_order(&self.lease, client_order_id)
    }

    pub fn reserved_bucket_stress_loss(
        &self,
        bucket: &ConcentrationBucket,
    ) -> Result<rust_decimal::Decimal, RiskStateMutationError> {
        self.store
            .reserved_bucket_stress_loss(self.lease.pool_id(), bucket)
    }

    pub fn reserved_risk_totals(&self) -> Result<RiskReservationTotals, RiskStateMutationError> {
        self.store.reserved_risk_totals(self.lease.pool_id())
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

    pub(crate) fn apply_order_lifecycle_state(
        &self,
        client_order_id: ClientOrderId,
        event_id: &str,
        ts_event_unix_nanos: u64,
        event_sequence: Option<u64>,
        target_state: ReservationLifecycleState,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        self.store.apply_order_lifecycle_state(
            &self.lease,
            client_order_id,
            event_id,
            ts_event_unix_nanos,
            event_sequence,
            target_state,
        )
    }

    pub fn mark_cancel_requested(
        &self,
        client_order_id: ClientOrderId,
        mutation_id: &str,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        self.store
            .mark_cancel_requested(&self.lease, client_order_id, mutation_id)
    }

    pub fn apply_authoritative_fill(
        &self,
        client_order_id: ClientOrderId,
        event_id: &str,
        ts_event_unix_nanos: u64,
        event_sequence: Option<u64>,
        fill_quantity: Decimal,
        remaining_fillable_quantity: Decimal,
        actual_conservative_liquidation_value: Decimal,
        actual_governor_cost_basis: Decimal,
        terminal_cash_flows: Vec<Decimal>,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        self.store.apply_authoritative_fill(
            &self.lease,
            client_order_id,
            event_id,
            ts_event_unix_nanos,
            event_sequence,
            fill_quantity,
            remaining_fillable_quantity,
            actual_conservative_liquidation_value,
            actual_governor_cost_basis,
            terminal_cash_flows,
        )
    }

    pub fn apply_settlement_truth(
        &self,
        client_order_id: ClientOrderId,
        event_id: &str,
        ts_event_unix_nanos: u64,
        event_sequence: Option<u64>,
        terminal_final: bool,
        reconciliation_complete: bool,
        conservative_liquidation_value: Decimal,
        governor_cost_basis: Decimal,
        terminal_cash_flows: Vec<Decimal>,
    ) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
        self.store.apply_settlement_truth(
            &self.lease,
            client_order_id,
            event_id,
            ts_event_unix_nanos,
            event_sequence,
            terminal_final,
            reconciliation_complete,
            conservative_liquidation_value,
            governor_cost_basis,
            terminal_cash_flows,
        )
    }

    pub(crate) fn complete_reconciliation(
        &self,
    ) -> Result<RiskStateVersion, RiskSubmissionMutationError> {
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
    Lifecycle,
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
    RiskKernel(RiskKernelError),
    UnknownReservation,
    UnknownSubmissionIntent,
    AdmissionTokenMismatch,
    ReservationNotReserved,
    SubmissionIntentConflict,
    InvalidLifecycleTransition,
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

#[allow(clippy::result_large_err)]
fn enforce_offered_load_envelope(
    inner: &mut FencedRiskStateStoreInner,
    lease: &PoolOwnershipLease,
    transaction: &RiskReservationTransaction,
    envelope: RiskReservationOfferedLoadEnvelope,
) -> Result<(), RiskReservationError> {
    let max_supported = envelope.max_supported_in_flight_risk_increasing_admissions();
    let offered_load = inner.reservation_totals.get(lease.pool_id()).map_or_else(
        || RiskReservationTotals::empty().open_order_count(),
        RiskReservationTotals::open_order_count,
    );
    if offered_load < max_supported {
        return Ok(());
    }

    let mut state = inner
        .policy_epoch_states
        .get(lease.pool_id())
        .cloned()
        .unwrap_or_else(ActivePolicyEpochState::no_policy_loaded);
    if !state
        .alerts
        .iter()
        .any(|alert| alert.reason == PolicyEpochAlertReason::AdmissionShed)
    {
        state.alerts.push(PolicyEpochAlert {
            reason: PolicyEpochAlertReason::AdmissionShed,
            epoch_id: transaction.candidate.policy_epoch_id.clone(),
        });
    }
    inner
        .policy_epoch_states
        .insert(lease.pool_id().clone(), state);

    Err(RiskReservationError::AdmissionShed {
        max_supported_in_flight_risk_increasing_admissions: max_supported,
        offered_in_flight_risk_increasing_admissions: offered_load,
    })
}

#[allow(clippy::result_large_err)]
fn validate_risk_increasing_policy_epoch(
    state: Option<&ActivePolicyEpochState>,
    candidate_policy_epoch_id: &str,
) -> Result<(), RiskReservationError> {
    let Some(state) = state else {
        return Err(RiskReservationError::NoActivePolicyEpoch);
    };
    if !state.risk_increasing_admission_enabled {
        return Err(RiskReservationError::RiskIncreasingAdmissionDisabled);
    }
    let Some(active_epoch) = &state.active_epoch else {
        return Err(RiskReservationError::NoActivePolicyEpoch);
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

fn matching_reservation_record_index_for_client_order(
    inner: &FencedRiskStateStoreInner,
    lease: &PoolOwnershipLease,
    client_order_id: ClientOrderId,
) -> Result<usize, RiskSubmissionMutationError> {
    let intent = inner
        .submission_intents
        .iter()
        .find_map(|((pool_id, _), intent)| {
            (pool_id == lease.pool_id() && intent.client_order_id == client_order_id)
                .then_some(intent)
        })
        .ok_or(RiskSubmissionMutationError::UnknownSubmissionIntent)?;
    matching_reservation_record_index(
        &inner.reservation_records,
        lease.pool_id(),
        &intent.admission_token.reservation_id,
    )
    .ok_or(RiskSubmissionMutationError::UnknownReservation)
}

fn reconciliation_would_release_reserved_orphans(
    inner: &FencedRiskStateStoreInner,
    pool_id: &PoolId,
) -> bool {
    inner.reservation_records.iter().any(|record| {
        &record.pool_id == pool_id && record.lifecycle_state == ReservationLifecycleState::Reserved
    })
}

fn validate_lifecycle_mutation_id(mutation_id: &str) -> Result<(), RiskSubmissionMutationError> {
    if mutation_id.trim().is_empty() {
        return Err(RiskSubmissionMutationError::State(
            RiskStateMutationError::InvalidMutation,
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct LifecycleEventInput<'a> {
    event_id: &'a str,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
    kind: LifecycleReconciliationFaultKind,
    order_status: Option<ReservationLifecycleState>,
}

impl LifecycleEventInput<'_> {
    fn identity(&self) -> LifecycleReconciliationEventIdentity {
        LifecycleReconciliationEventIdentity::new(self.kind, self.event_id)
    }
}

fn lifecycle_event_preflight(
    inner: &mut FencedRiskStateStoreInner,
    lease: &PoolOwnershipLease,
    record_index: usize,
    event: LifecycleEventInput<'_>,
) -> Result<Option<LifecycleMutationResult>, RiskSubmissionMutationError> {
    let record = &inner.reservation_records[record_index];
    let event_identity = event.identity();
    if record.applied_lifecycle_event_ids.contains(&event_identity) {
        return Ok(Some(LifecycleMutationResult {
            risk_state_version: current_pool_version(inner, lease.pool_id()),
            lifecycle_state: record.lifecycle_state,
        }));
    }
    if record
        .unresolved_lifecycle_reconciliation_faults
        .contains_key(&event_identity)
        && lifecycle_event_ordering_fault(record, event.ts_event_unix_nanos, event.event_sequence)
    {
        return Ok(Some(LifecycleMutationResult {
            risk_state_version: current_pool_version(inner, lease.pool_id()),
            lifecycle_state: ReservationLifecycleState::ReconciliationRequired,
        }));
    }
    if lifecycle_event_ordering_fault(record, event.ts_event_unix_nanos, event.event_sequence) {
        return mark_lifecycle_reconciliation_required(inner, lease, record_index, event).map(Some);
    }
    Ok(None)
}

fn lifecycle_event_ordering_fault(
    record: &SubstrateReservationRecord,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
) -> bool {
    if record
        .last_lifecycle_ts_event_unix_nanos
        .is_some_and(|last_ts| ts_event_unix_nanos < last_ts)
    {
        return true;
    }
    let Some(event_sequence) = event_sequence else {
        return false;
    };
    let Some(last_sequence) = record.last_lifecycle_event_sequence else {
        return false;
    };
    event_sequence <= last_sequence || event_sequence > last_sequence.saturating_add(1)
}

fn mark_lifecycle_reconciliation_required(
    inner: &mut FencedRiskStateStoreInner,
    lease: &PoolOwnershipLease,
    record_index: usize,
    event: LifecycleEventInput<'_>,
) -> Result<LifecycleMutationResult, RiskSubmissionMutationError> {
    let version =
        next_pool_version(inner, lease.pool_id()).map_err(RiskSubmissionMutationError::State)?;
    {
        let record = &mut inner.reservation_records[record_index];
        record.lifecycle_state = ReservationLifecycleState::ReconciliationRequired;
        record.unresolved_lifecycle_reconciliation_faults.insert(
            event.identity(),
            LifecycleReconciliationFault {
                kind: event.kind,
                order_status: event.order_status,
                ts_event_unix_nanos: event.ts_event_unix_nanos,
                event_sequence: event.event_sequence,
            },
        );
    }
    inner.reconciled.insert(lease.pool_id().clone(), false);
    inner.mutations.push(DurableRiskMutationRecord {
        pool_id: lease.pool_id().clone(),
        fencing_token: lease.fencing_token(),
        mutation: DurableRiskMutation::new(event.event_id.to_string(), RiskMutationKind::Lifecycle),
        risk_state_version: version,
    });
    Ok(LifecycleMutationResult {
        risk_state_version: version,
        lifecycle_state: ReservationLifecycleState::ReconciliationRequired,
    })
}

fn record_lifecycle_event_success(
    record: &mut SubstrateReservationRecord,
    event_id: &str,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
    applied_kind: LifecycleReconciliationFaultKind,
    applied_state: Option<ReservationLifecycleState>,
) {
    let event_identity = LifecycleReconciliationEventIdentity::new(applied_kind, event_id);
    record
        .applied_lifecycle_event_ids
        .insert(event_identity.clone());
    record
        .unresolved_lifecycle_reconciliation_faults
        .remove(&event_identity);
    if applied_kind == LifecycleReconciliationFaultKind::OrderStatus
        && applied_state.is_some_and(open_remainder_release_state)
    {
        record
            .unresolved_lifecycle_reconciliation_faults
            .retain(|_, fault| {
                !terminal_order_status_supersedes_fault(fault, ts_event_unix_nanos, event_sequence)
            });
    }
    record.last_lifecycle_ts_event_unix_nanos = Some(ts_event_unix_nanos);
    if let Some(event_sequence) = event_sequence {
        record.last_lifecycle_event_sequence = Some(event_sequence);
    }
}

fn terminal_order_status_supersedes_fault(
    fault: &LifecycleReconciliationFault,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
) -> bool {
    fault.kind == LifecycleReconciliationFaultKind::OrderStatus
        && (fault.order_status.is_some_and(open_remainder_release_state)
            || (fault.ts_event_unix_nanos, fault.event_sequence)
                < (ts_event_unix_nanos, event_sequence))
}

/// A terminal order-status is absorbed and advances ordering, but remains held
/// in `ReconciliationRequired` while any non-superseded fault remains so
/// exposure events replay before completion. The blocking fault's exact
/// re-delivery or a later superseding terminal resolves the hold.
///
/// If an absorbed terminal's blocking fault resolves to a non-terminal state
/// such as `PartiallyFilled` on a contradictory feed, the open remainder stays
/// held fail-closed with no automatic release. This intentional S0
/// over-reserve is tracked for graceful release in #1013.
fn terminal_order_status_has_blocking_fault(
    record: &SubstrateReservationRecord,
    ts_event_unix_nanos: u64,
    event_sequence: Option<u64>,
) -> bool {
    record
        .unresolved_lifecycle_reconciliation_faults
        .values()
        .any(|fault| {
            !terminal_order_status_supersedes_fault(fault, ts_event_unix_nanos, event_sequence)
        })
}

fn current_pool_version(inner: &FencedRiskStateStoreInner, pool_id: &PoolId) -> RiskStateVersion {
    inner
        .versions
        .get(pool_id)
        .copied()
        .unwrap_or_else(RiskStateVersion::zero)
}

fn validate_lifecycle_exposure_input(
    fill_quantity: Decimal,
    remaining_fillable_quantity: Decimal,
    conservative_liquidation_value: Decimal,
    governor_cost_basis: Decimal,
    terminal_cash_flows: &[Decimal],
) -> Result<(), RiskSubmissionMutationError> {
    if fill_quantity <= Decimal::ZERO
        || remaining_fillable_quantity < Decimal::ZERO
        || conservative_liquidation_value < Decimal::ZERO
        || governor_cost_basis < Decimal::ZERO
        || terminal_cash_flows.is_empty()
    {
        return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
    }
    Ok(())
}

fn validate_settlement_exposure_input(
    conservative_liquidation_value: Decimal,
    governor_cost_basis: Decimal,
    terminal_cash_flows: &[Decimal],
) -> Result<(), RiskSubmissionMutationError> {
    if conservative_liquidation_value < Decimal::ZERO
        || governor_cost_basis < Decimal::ZERO
        || terminal_cash_flows.is_empty()
    {
        return Err(RiskSubmissionMutationError::InvalidLifecycleTransition);
    }
    Ok(())
}

fn order_status_transition_allowed(
    current_state: ReservationLifecycleState,
    target_state: ReservationLifecycleState,
) -> bool {
    matches!(
        (current_state, target_state),
        (
            ReservationLifecycleState::Submitted,
            ReservationLifecycleState::Open
        ) | (
            ReservationLifecycleState::SubmissionUnknown,
            ReservationLifecycleState::Open
        ) | (
            ReservationLifecycleState::Open,
            ReservationLifecycleState::Open
        ) | (
            ReservationLifecycleState::ReconciliationRequired,
            ReservationLifecycleState::Open
        )
    ) || (target_state == ReservationLifecycleState::CancelRequested
        && cancel_request_source_state(current_state))
        || (open_remainder_release_state(target_state)
            && non_fillable_confirmation_source_state(current_state))
}

fn cancel_request_source_state(current_state: ReservationLifecycleState) -> bool {
    matches!(
        current_state,
        ReservationLifecycleState::Submitted
            | ReservationLifecycleState::Open
            | ReservationLifecycleState::PartiallyFilled
    )
}

fn non_fillable_confirmation_source_state(current_state: ReservationLifecycleState) -> bool {
    matches!(
        current_state,
        ReservationLifecycleState::Submitted
            | ReservationLifecycleState::Open
            | ReservationLifecycleState::PartiallyFilled
            | ReservationLifecycleState::CancelRequested
            | ReservationLifecycleState::SubmissionUnknown
            | ReservationLifecycleState::ReconciliationRequired
    )
}

fn open_remainder_release_state(target_state: ReservationLifecycleState) -> bool {
    matches!(
        target_state,
        ReservationLifecycleState::CancelConfirmed | ReservationLifecycleState::ExpiredConfirmed
    )
}

fn fill_transition_allowed(current_state: ReservationLifecycleState) -> bool {
    matches!(
        current_state,
        ReservationLifecycleState::Submitted
            | ReservationLifecycleState::Open
            | ReservationLifecycleState::PartiallyFilled
            | ReservationLifecycleState::CancelRequested
            | ReservationLifecycleState::ReconciliationRequired
    )
}

fn settlement_transition_allowed(current_state: ReservationLifecycleState) -> bool {
    matches!(
        current_state,
        ReservationLifecycleState::Filled
            | ReservationLifecycleState::CancelConfirmed
            | ReservationLifecycleState::ExpiredConfirmed
    )
}

fn equity_floor_stress_loss(
    exposure: &RiskExposure,
) -> Result<Decimal, RiskSubmissionMutationError> {
    RiskKernel::equity_floor_stress_loss_for_exposure(exposure)
        .map_err(RiskSubmissionMutationError::RiskKernel)
}

fn governor_realized_loss(exposure: &RiskExposure) -> Result<Decimal, RiskSubmissionMutationError> {
    RiskKernel::governor_realized_loss_for_exposure(exposure)
        .map_err(RiskSubmissionMutationError::RiskKernel)
}

fn monotonic_risk_metric(previous: Decimal, recomputed: Decimal) -> Decimal {
    if recomputed > previous {
        recomputed
    } else {
        previous
    }
}
