use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use crate::bolt_v3_risk_reservation_substrate::contracts::{
    ConfiguredLeaseAuthority, FencingToken, OwnerId, PoolId, PoolOwnershipLease, RiskStateVersion,
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
        }
    }
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

    pub fn durable_mutation_records(
        &self,
    ) -> Result<Vec<DurableRiskMutationRecord>, RiskStateMutationError> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| RiskStateMutationError::AmbiguousLeaseState)?;
        Ok(inner.mutations.clone())
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

        let current_version = *inner
            .versions
            .entry(lease.pool_id().clone())
            .or_insert(RiskStateVersion::zero());
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
            assessment: assessment.clone(),
            evaluated_dimensions: evaluation.evaluated_dimensions.clone(),
        });

        Ok(RiskReservationCommit {
            admission_token: token,
            assessment,
            evaluated_dimensions: evaluation.evaluated_dimensions,
            diagnostic_mismatches: evaluation.diagnostic_mismatches,
        })
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

    pub fn reconcile_before_new_risk(&self) -> Result<RiskStateVersion, RiskStateMutationError> {
        self.store.mark_reconciled(&self.lease)
    }

    pub fn commit_durable_mutation(
        &self,
        mutation: DurableRiskMutation,
    ) -> Result<RiskStateVersion, RiskStateMutationError> {
        self.store.commit_durable_mutation(&self.lease, mutation)
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
    ReconciliationRequired,
    VersionOverflow,
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
