pub use crate::bolt_v3_risk_reservation_substrate::reservation_ledger::{
    BoundReusableSafetyState, CallerRiskDiagnostics, RiskCapDimension,
    RiskReservationCommit as AdmissionReservation, RiskReservationError as AdmissionReserveError,
};

use std::collections::BTreeSet;

use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{AdmissionCandidate, RiskStateVersion, SafetyAction},
    instrument_risk_registry::{DescriptorRegistryAdmissionError, InstrumentRiskRegistry},
    reservation_ledger::{RiskReservationTransaction, SubstrateReservationRecord},
    risk_classifier::ConcentrationBucket,
    risk_kernel::{RiskExposureSetInput, RiskKernel, RiskKernelError, RiskLossMetrics},
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
    pub before: RiskExposureSetInput,
    pub after: RiskExposureSetInput,
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
    pub before_exposure_count: usize,
    pub after_exposure_count: usize,
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
    ProofVersionMismatch {
        before: RiskStateVersion,
        after: RiskStateVersion,
    },
    ProofDomainExceeded {
        max_exposure_count: usize,
        before_exposure_count: usize,
        after_exposure_count: usize,
    },
    Kernel(RiskKernelError),
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
        validate_safety_action_request(&request)?;

        let before = RiskKernel::evaluate_exposure_set(&request.before)
            .map_err(SafetyActionAdmissionError::Kernel)?;
        let after = RiskKernel::evaluate_exposure_set(&request.after)
            .map_err(SafetyActionAdmissionError::Kernel)?;
        let increased_metrics = increased_safety_action_metrics(&before, &after);
        if !increased_metrics.is_empty() {
            return Err(SafetyActionAdmissionError::RiskIncreased {
                increased_metrics,
                before_equity_floor_stress_loss: before.equity_floor_stress_loss,
                after_equity_floor_stress_loss: after.equity_floor_stress_loss,
                before_governor_realized_loss: before.governor_realized_loss,
                after_governor_realized_loss: after.governor_realized_loss,
            });
        }

        let source_risk_state_version = request.before.risk_state_version;
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
            proof_domain: request.proof_domain,
        })
    }
}

fn validate_safety_action_request(
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
    if request.before.risk_state_version != request.after.risk_state_version {
        return Err(SafetyActionAdmissionError::ProofVersionMismatch {
            before: request.before.risk_state_version,
            after: request.after.risk_state_version,
        });
    }
    if request.safety_state.risk_state_version != request.before.risk_state_version {
        return Err(SafetyActionAdmissionError::SafetyStateVersionMismatch {
            expected: request.before.risk_state_version,
            actual: request.safety_state.risk_state_version,
        });
    }
    if request.proof_domain.max_exposure_count == 0
        || request.proof_domain.before_exposure_count != request.before.exposures.len()
        || request.proof_domain.after_exposure_count != request.after.exposures.len()
    {
        return Err(SafetyActionAdmissionError::InvalidProofDomain);
    }
    if request.before.exposures.len() > request.proof_domain.max_exposure_count
        || request.after.exposures.len() > request.proof_domain.max_exposure_count
    {
        return Err(SafetyActionAdmissionError::ProofDomainExceeded {
            max_exposure_count: request.proof_domain.max_exposure_count,
            before_exposure_count: request.before.exposures.len(),
            after_exposure_count: request.after.exposures.len(),
        });
    }
    Ok(())
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
