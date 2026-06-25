pub use crate::bolt_v3_risk_reservation_substrate::reservation_ledger::{
    BoundReusableSafetyState, CallerRiskDiagnostics, RiskCapDimension,
    RiskReservationCommit as AdmissionReservation, RiskReservationError as AdmissionReserveError,
    RiskReservationWorkDimension,
};

use std::collections::{BTreeMap, BTreeSet};

use nautilus_model::identifiers::ClientOrderId;
use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{AdmissionCandidate, RiskStateVersion, SafetyAction},
    instrument_risk_registry::{DescriptorRegistryAdmissionError, InstrumentRiskRegistry},
    reservation_ledger::{RiskReservationTransaction, SubstrateReservationRecord},
    risk_classifier::ConcentrationBucket,
    risk_kernel::{
        RiskExposure, RiskExposureSetInput, RiskKernel, RiskKernelError, RiskLossMetrics,
    },
    risk_view_publisher::PublishedRiskView,
    state_owner::{RiskStateMutationError, RiskStateOwner},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertifiedAdmissionReserveError {
    Registry(DescriptorRegistryAdmissionError),
    Reserve(AdmissionReserveError),
}

impl PartialEq<DescriptorRegistryAdmissionError> for CertifiedAdmissionReserveError {
    fn eq(&self, other: &DescriptorRegistryAdmissionError) -> bool {
        matches!(self, Self::Registry(error) if error == other)
    }
}

#[derive(Debug, Clone)]
pub struct AdmissionService {
    owner: RiskStateOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyActionAdmissionRequest {
    pub action_id: String,
    pub action: SafetyAction,
    pub safety_state: BoundReusableSafetyState,
    pub proof_domain: SafetyActionProofDomain,
}

/// Bound for the S5 reduction proof domain.
///
/// `max_exposure_count` is the configured finite upper bound for each
/// recomputed exposure set. The verifier rejects before calling the kernel when
/// either set exceeds it, so the proof cannot fall into an unbounded scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafetyActionProofDomain {
    pub max_exposure_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SafetyActionMetric {
    EquityFloorStressLoss,
    GovernorRealizedLoss,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetyActionAdmission {
    pub action_id: String,
    pub action: SafetyAction,
    pub source_risk_state_version: RiskStateVersion,
    pub risk_state_version: RiskStateVersion,
    pub before: RiskLossMetrics,
    pub after: RiskLossMetrics,
    pub proof_domain: SafetyActionProofDomain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyActionAdmissionError {
    InvalidAction,
    InvalidProofDomain,
    SafetyStateVersionMismatch {
        expected: RiskStateVersion,
        actual: RiskStateVersion,
    },
    ProofDomainExceeded {
        max_exposure_count: usize,
        before_exposure_count: usize,
        after_exposure_count: usize,
    },
    UnknownSafetyActionTarget,
    Kernel(RiskKernelError),
    AfterExposureNotReduction {
        new_exposure_count: usize,
        increased_exposure_count: usize,
        increased_metrics: BTreeSet<SafetyActionMetric>,
        before_equity_floor_stress_loss: Decimal,
        after_equity_floor_stress_loss: Decimal,
        before_governor_realized_loss: Decimal,
        after_governor_realized_loss: Decimal,
    },
    RiskIncreased {
        increased_metrics: BTreeSet<SafetyActionMetric>,
        before_equity_floor_stress_loss: Decimal,
        after_equity_floor_stress_loss: Decimal,
        before_governor_realized_loss: Decimal,
        after_governor_realized_loss: Decimal,
    },
    StateMutation(RiskStateMutationError),
}

impl AdmissionService {
    pub fn new(owner: RiskStateOwner) -> Self {
        Self { owner }
    }

    pub fn compare_and_reserve(
        &self,
        view: &PublishedRiskView,
        candidate: AdmissionCandidate,
        safety_state: BoundReusableSafetyState,
        caller_diagnostics: Option<CallerRiskDiagnostics>,
        now_unix_nanos: u64,
    ) -> Result<AdmissionReservation, AdmissionReserveError> {
        let kernel_input = view
            .kernel_input_for_candidate(&candidate)
            .map_err(|_| AdmissionReserveError::InvalidCandidate)?;
        self.owner.compare_and_reserve(RiskReservationTransaction {
            candidate,
            kernel_input,
            sizing_view: view.sizing_view().clone(),
            safety_state,
            caller_diagnostics,
            now_unix_nanos,
        })
    }

    pub fn compare_and_reserve_certified(
        &self,
        registry: &InstrumentRiskRegistry,
        view: &PublishedRiskView,
        candidate: AdmissionCandidate,
        safety_state: BoundReusableSafetyState,
        caller_diagnostics: Option<CallerRiskDiagnostics>,
        now_unix_nanos: u64,
    ) -> Result<AdmissionReservation, CertifiedAdmissionReserveError> {
        registry
            .validate_admission_binding(
                view.active_descriptor(),
                &candidate,
                self.owner.owner_id().as_str(),
            )
            .map_err(CertifiedAdmissionReserveError::Registry)?;
        self.compare_and_reserve(
            view,
            candidate,
            safety_state,
            caller_diagnostics,
            now_unix_nanos,
        )
        .map_err(CertifiedAdmissionReserveError::Reserve)
    }

    pub fn reservation_records(
        &self,
    ) -> Result<Vec<SubstrateReservationRecord>, RiskStateMutationError> {
        self.owner.reservation_records()
    }

    pub fn reserved_bucket_stress_loss(
        &self,
        bucket: &ConcentrationBucket,
    ) -> Result<Decimal, RiskStateMutationError> {
        self.owner.reserved_bucket_stress_loss(bucket)
    }

    pub fn admit_safety_action(
        &self,
        request: SafetyActionAdmissionRequest,
    ) -> Result<SafetyActionAdmission, SafetyActionAdmissionError> {
        validate_safety_action_identity(&request)?;
        let source_risk_state_version = self
            .owner
            .policy_epoch_snapshot()
            .map_err(SafetyActionAdmissionError::StateMutation)?
            .risk_state_version;
        let (before_input, after_input) =
            self.derive_safety_action_exposure_sets(source_risk_state_version, &request.action)?;
        validate_safety_action_request(&before_input, &after_input, &request)?;

        let before = RiskKernel::evaluate_exposure_set(&before_input)
            .map_err(SafetyActionAdmissionError::Kernel)?;
        let after = RiskKernel::evaluate_exposure_set(&after_input)
            .map_err(SafetyActionAdmissionError::Kernel)?;
        let increased_metrics = increased_safety_action_metrics(&before, &after);
        let exposure_reduction = check_after_exposure_reduction(&before_input, &after_input)
            .map_err(SafetyActionAdmissionError::Kernel)?;
        if exposure_reduction.has_violation() {
            return Err(SafetyActionAdmissionError::AfterExposureNotReduction {
                new_exposure_count: exposure_reduction.new_exposure_count,
                increased_exposure_count: exposure_reduction.increased_exposure_count,
                increased_metrics,
                before_equity_floor_stress_loss: before.equity_floor_stress_loss,
                after_equity_floor_stress_loss: after.equity_floor_stress_loss,
                before_governor_realized_loss: before.governor_realized_loss,
                after_governor_realized_loss: after.governor_realized_loss,
            });
        }
        if !increased_metrics.is_empty() {
            return Err(SafetyActionAdmissionError::RiskIncreased {
                increased_metrics,
                before_equity_floor_stress_loss: before.equity_floor_stress_loss,
                after_equity_floor_stress_loss: after.equity_floor_stress_loss,
                before_governor_realized_loss: before.governor_realized_loss,
                after_governor_realized_loss: after.governor_realized_loss,
            });
        }

        let risk_state_version = self
            .owner
            .commit_safety_action(&request.action_id, source_risk_state_version)
            .map_err(SafetyActionAdmissionError::StateMutation)?;

        Ok(SafetyActionAdmission {
            action_id: request.action_id,
            action: request.action,
            source_risk_state_version,
            risk_state_version,
            before,
            after,
            proof_domain: SafetyActionProofDomain {
                max_exposure_count: request.proof_domain.max_exposure_count,
            },
        })
    }

    fn derive_safety_action_exposure_sets(
        &self,
        risk_state_version: RiskStateVersion,
        action: &SafetyAction,
    ) -> Result<(RiskExposureSetInput, RiskExposureSetInput), SafetyActionAdmissionError> {
        let pool_id = self.owner.lease().pool_id().clone();
        let records = self
            .owner
            .reservation_records()
            .map_err(SafetyActionAdmissionError::StateMutation)?;
        let scoped_records = records
            .iter()
            .filter(|record| record.pool_id == pool_id)
            .collect::<Vec<_>>();

        let mut before_exposures = Vec::new();
        let mut after_exposures = Vec::new();
        let mut target_found = false;

        match action {
            SafetyAction::CancelExistingOrder { client_order_id } => {
                let target_record = self
                    .owner
                    .reservation_record_for_client_order(ClientOrderId::from(
                        client_order_id.as_str(),
                    ))
                    .map_err(SafetyActionAdmissionError::StateMutation)?
                    .ok_or(SafetyActionAdmissionError::UnknownSafetyActionTarget)?;
                if target_record.pool_id != pool_id || open_order_exposure(&target_record).is_none()
                {
                    return Err(SafetyActionAdmissionError::UnknownSafetyActionTarget);
                }

                for record in scoped_records {
                    if let Some(exposure) = open_order_exposure(record) {
                        before_exposures.push(exposure.clone());
                        if record.admission_token == target_record.admission_token {
                            target_found = true;
                        } else {
                            after_exposures.push(exposure);
                        }
                    }
                    if let Some(exposure) = record.filled_position_exposure.clone() {
                        before_exposures.push(exposure.clone());
                        after_exposures.push(exposure);
                    }
                }
            }
            SafetyAction::ReduceOnlyCloseExistingPosition { position_id } => {
                for record in scoped_records {
                    if let Some(exposure) = open_order_exposure(record) {
                        before_exposures.push(exposure.clone());
                        after_exposures.push(exposure);
                    }
                    if let Some(exposure) = record.filled_position_exposure.clone() {
                        before_exposures.push(exposure.clone());
                        if !target_found && filled_position_matches(record, position_id) {
                            target_found = true;
                        } else {
                            after_exposures.push(exposure);
                        }
                    }
                }
            }
        }

        if !target_found {
            return Err(SafetyActionAdmissionError::UnknownSafetyActionTarget);
        }

        Ok((
            RiskExposureSetInput {
                risk_state_version,
                exposures: before_exposures,
            },
            RiskExposureSetInput {
                risk_state_version,
                exposures: after_exposures,
            },
        ))
    }
}

fn validate_safety_action_identity(
    request: &SafetyActionAdmissionRequest,
) -> Result<(), SafetyActionAdmissionError> {
    if !is_clean_runtime_value(&request.action_id) {
        return Err(SafetyActionAdmissionError::InvalidAction);
    }
    match &request.action {
        SafetyAction::CancelExistingOrder { client_order_id } => {
            if !is_clean_runtime_value(client_order_id) {
                return Err(SafetyActionAdmissionError::InvalidAction);
            }
        }
        SafetyAction::ReduceOnlyCloseExistingPosition { position_id } => {
            if !is_clean_runtime_value(position_id) {
                return Err(SafetyActionAdmissionError::InvalidAction);
            }
        }
    }
    Ok(())
}

fn validate_safety_action_request(
    before: &RiskExposureSetInput,
    after: &RiskExposureSetInput,
    request: &SafetyActionAdmissionRequest,
) -> Result<(), SafetyActionAdmissionError> {
    if request.safety_state.risk_state_version != before.risk_state_version {
        return Err(SafetyActionAdmissionError::SafetyStateVersionMismatch {
            expected: before.risk_state_version,
            actual: request.safety_state.risk_state_version,
        });
    }
    if request.proof_domain.max_exposure_count == 0 {
        return Err(SafetyActionAdmissionError::InvalidProofDomain);
    }
    if before.exposures.len() > request.proof_domain.max_exposure_count
        || after.exposures.len() > request.proof_domain.max_exposure_count
    {
        return Err(SafetyActionAdmissionError::ProofDomainExceeded {
            max_exposure_count: request.proof_domain.max_exposure_count,
            before_exposure_count: before.exposures.len(),
            after_exposure_count: after.exposures.len(),
        });
    }
    Ok(())
}

fn open_order_exposure(record: &SubstrateReservationRecord) -> Option<RiskExposure> {
    if record.remaining_fillable_quantity <= Decimal::ZERO {
        return None;
    }
    Some(RiskExposure {
        instrument_id: record.instrument_id.clone(),
        buckets: record.buckets.clone(),
        quantity: record.remaining_fillable_quantity,
        conservative_liquidation_value: record.assessment.equity_floor_stress_loss,
        governor_cost_basis: record.assessment.governor_realized_loss,
        terminal_cash_flows: vec![Decimal::ZERO],
    })
}

fn filled_position_matches(record: &SubstrateReservationRecord, position_id: &str) -> bool {
    record.admission_token.reservation_id == position_id
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SafetyActionExposureIdentity {
    instrument_id: String,
    buckets: BTreeSet<ConcentrationBucket>,
}

impl From<&RiskExposure> for SafetyActionExposureIdentity {
    fn from(exposure: &RiskExposure) -> Self {
        Self {
            instrument_id: exposure.instrument_id.clone(),
            buckets: exposure.buckets.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SafetyActionExposureMetrics {
    equity_floor_stress_loss: Decimal,
    governor_realized_loss: Decimal,
}

impl SafetyActionExposureMetrics {
    fn add(&mut self, other: Self) {
        self.equity_floor_stress_loss += other.equity_floor_stress_loss;
        self.governor_realized_loss += other.governor_realized_loss;
    }

    fn exceeds(self, before: Self) -> bool {
        self.equity_floor_stress_loss > before.equity_floor_stress_loss
            || self.governor_realized_loss > before.governor_realized_loss
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SafetyActionExposureReduction {
    new_exposure_count: usize,
    increased_exposure_count: usize,
}

impl SafetyActionExposureReduction {
    fn none() -> Self {
        Self {
            new_exposure_count: 0,
            increased_exposure_count: 0,
        }
    }

    fn has_violation(self) -> bool {
        self.new_exposure_count > 0 || self.increased_exposure_count > 0
    }
}

fn check_after_exposure_reduction(
    before: &RiskExposureSetInput,
    after: &RiskExposureSetInput,
) -> Result<SafetyActionExposureReduction, RiskKernelError> {
    let before_by_identity = exposure_metrics_by_identity(before)?;
    let after_by_identity = exposure_metrics_by_identity(after)?;
    let mut reduction = SafetyActionExposureReduction::none();

    // S8 handoff: derive the exact `after` set from the named SafetyAction here.
    for (identity, after_metrics) in after_by_identity {
        let Some(before_metrics) = before_by_identity.get(&identity).copied() else {
            reduction.new_exposure_count += 1;
            continue;
        };
        if after_metrics.exceeds(before_metrics) {
            reduction.increased_exposure_count += 1;
        }
    }

    Ok(reduction)
}

fn exposure_metrics_by_identity(
    input: &RiskExposureSetInput,
) -> Result<BTreeMap<SafetyActionExposureIdentity, SafetyActionExposureMetrics>, RiskKernelError> {
    let mut metrics_by_identity = BTreeMap::new();
    for exposure in &input.exposures {
        let identity = SafetyActionExposureIdentity::from(exposure);
        let metrics = per_exposure_metrics(exposure)?;
        metrics_by_identity
            .entry(identity)
            .and_modify(|current: &mut SafetyActionExposureMetrics| current.add(metrics))
            .or_insert(metrics);
    }
    Ok(metrics_by_identity)
}

fn per_exposure_metrics(
    exposure: &RiskExposure,
) -> Result<SafetyActionExposureMetrics, RiskKernelError> {
    Ok(SafetyActionExposureMetrics {
        equity_floor_stress_loss: RiskKernel::equity_floor_stress_loss_for_exposure(exposure)?,
        governor_realized_loss: RiskKernel::governor_realized_loss_for_exposure(exposure)?,
    })
}

fn increased_safety_action_metrics(
    before: &RiskLossMetrics,
    after: &RiskLossMetrics,
) -> BTreeSet<SafetyActionMetric> {
    let mut increased = BTreeSet::new();
    if after.equity_floor_stress_loss > before.equity_floor_stress_loss {
        increased.insert(SafetyActionMetric::EquityFloorStressLoss);
    }
    if after.governor_realized_loss > before.governor_realized_loss {
        increased.insert(SafetyActionMetric::GovernorRealizedLoss);
    }
    increased
}

fn is_clean_runtime_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
