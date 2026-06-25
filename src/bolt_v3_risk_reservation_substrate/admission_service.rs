pub use crate::bolt_v3_risk_reservation_substrate::reservation_ledger::{
    BoundReusableSafetyState, CallerRiskDiagnostics, RiskCapDimension,
    RiskReservationCommit as AdmissionReservation, RiskReservationError as AdmissionReserveError,
};

use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::AdmissionCandidate,
    reservation_ledger::{RiskReservationTransaction, SubstrateReservationRecord},
    risk_classifier::ConcentrationBucket,
    risk_view_publisher::PublishedRiskView,
    state_owner::{RiskStateMutationError, RiskStateOwner},
};

#[derive(Debug, Clone)]
pub struct AdmissionService {
    owner: RiskStateOwner,
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
}
