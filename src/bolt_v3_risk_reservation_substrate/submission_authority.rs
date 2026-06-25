use nautilus_model::identifiers::ClientOrderId;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{AdmittedOrder, DurableSubmissionIntent, LiveSubmissionRecord, RiskStateVersion},
    reservation_ledger::RiskReservationCommit,
    state_owner::{RiskStateOwner, RiskSubmissionMutationError},
};

#[derive(Debug, Clone)]
pub struct SubmissionAuthority {
    owner: RiskStateOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSubmitReceipt {
    pub client_order_id: ClientOrderId,
    pub risk_state_version: RiskStateVersion,
}

pub trait LiveSubmitBoundary {
    type Error;

    /// ```compile_fail
    /// use bolt_v2::bolt_v3_risk_reservation_substrate::{
    ///     contracts::AdmissionToken,
    ///     submission_authority::{LiveSubmitBoundary, LiveSubmitReceipt},
    /// };
    ///
    /// struct Boundary;
    ///
    /// impl LiveSubmitBoundary for Boundary {
    ///     type Error = ();
    ///
    ///     fn submit_admitted_order(
    ///         &mut self,
    ///         order: bolt_v2::bolt_v3_risk_reservation_substrate::contracts::AdmittedOrder,
    ///     ) -> Result<LiveSubmitReceipt, Self::Error> {
    ///         Ok(LiveSubmitReceipt {
    ///             client_order_id: order.client_order_id(),
    ///             risk_state_version: order.risk_state_version(),
    ///         })
    ///     }
    /// }
    ///
    /// fn bypass(boundary: &mut Boundary, token: AdmissionToken) {
    ///     boundary.submit_admitted_order(token);
    /// }
    /// ```
    fn submit_admitted_order(
        &mut self,
        order: AdmittedOrder,
    ) -> Result<LiveSubmitReceipt, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmissionAuthorityError {
    State(RiskSubmissionMutationError),
    LiveSubmitRejected,
}

impl From<RiskSubmissionMutationError> for SubmissionAuthorityError {
    fn from(error: RiskSubmissionMutationError) -> Self {
        Self::State(error)
    }
}

impl SubmissionAuthority {
    pub fn new(owner: RiskStateOwner) -> Self {
        Self { owner }
    }

    pub fn prepare_admitted_order(
        &self,
        reservation: &RiskReservationCommit,
        client_order_id: ClientOrderId,
        now_unix_nanos: u64,
    ) -> Result<AdmittedOrder, SubmissionAuthorityError> {
        let intent =
            self.owner
                .prepare_submission_intent(reservation, client_order_id, now_unix_nanos)?;
        Ok(admitted_order_from_intent(intent))
    }

    pub fn durable_submission_intents(
        &self,
    ) -> Result<Vec<DurableSubmissionIntent>, SubmissionAuthorityError> {
        self.owner
            .durable_submission_intents()
            .map_err(SubmissionAuthorityError::from)
    }

    pub fn submit_idempotently<B>(
        &self,
        reservation: &RiskReservationCommit,
        client_order_id: ClientOrderId,
        boundary: &mut B,
        now_unix_nanos: u64,
    ) -> Result<LiveSubmitReceipt, SubmissionAuthorityError>
    where
        B: LiveSubmitBoundary<Error = SubmissionAuthorityError>,
    {
        let admitted = self.prepare_admitted_order(reservation, client_order_id, now_unix_nanos)?;
        self.submit_prepared(admitted, boundary, now_unix_nanos)
    }

    pub fn submit_durable_intent<B>(
        &self,
        idempotency_key: &str,
        boundary: &mut B,
        now_unix_nanos: u64,
    ) -> Result<LiveSubmitReceipt, SubmissionAuthorityError>
    where
        B: LiveSubmitBoundary<Error = SubmissionAuthorityError>,
    {
        if let Some(existing) = self.owner.live_submission_record(idempotency_key)? {
            return Ok(live_receipt(existing));
        }
        let intent = self.owner.durable_submission_intent(idempotency_key)?;
        let admitted = admitted_order_from_intent(intent);
        self.submit_prepared(admitted, boundary, now_unix_nanos)
    }

    pub fn submit_prepared<B>(
        &self,
        admitted: AdmittedOrder,
        boundary: &mut B,
        _now_unix_nanos: u64,
    ) -> Result<LiveSubmitReceipt, SubmissionAuthorityError>
    where
        B: LiveSubmitBoundary<Error = SubmissionAuthorityError>,
    {
        let idempotency_key = admitted.idempotency_key().to_string();
        if let Some(existing) = self.owner.live_submission_record(&idempotency_key)? {
            return Ok(live_receipt(existing));
        }
        let receipt = boundary.submit_admitted_order(admitted)?;
        let record = self.owner.record_live_submission(
            &idempotency_key,
            LiveSubmissionRecord {
                client_order_id: receipt.client_order_id,
                risk_state_version: receipt.risk_state_version,
            },
        )?;
        Ok(live_receipt(record))
    }
}

fn admitted_order_from_intent(intent: DurableSubmissionIntent) -> AdmittedOrder {
    AdmittedOrder::from_submitted_reservation(
        intent.admission_token,
        intent.client_order_id,
        intent.instrument_id,
        intent.submitted_risk_state_version,
    )
}

fn live_receipt(record: LiveSubmissionRecord) -> LiveSubmitReceipt {
    LiveSubmitReceipt {
        client_order_id: record.client_order_id,
        risk_state_version: record.risk_state_version,
    }
}
