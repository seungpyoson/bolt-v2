use nautilus_model::identifiers::ClientOrderId;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{LiveSubmissionRecord, RiskStateVersion},
    state_owner::{RiskStateMutationError, RiskStateOwner, RiskSubmissionMutationError},
    submission_authority::{LiveSubmitBoundary, SubmissionAuthority, SubmissionAuthorityError},
};

#[derive(Debug, Clone)]
pub struct LifecycleReconciler {
    owner: RiskStateOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtExecutionTruth {
    pub order_status_reports: Vec<NtOrderStatusReportTruth>,
    pub fill_reports: Vec<NtFillReportTruth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtOrderStatusReportTruth {
    pub client_order_id: ClientOrderId,
    pub event_id: String,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtFillReportTruth {
    pub client_order_id: ClientOrderId,
    pub event_id: String,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationSummary {
    pub live_order_count: usize,
    pub reservation_count: usize,
    pub risk_state_version: RiskStateVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleReconciliationError {
    State(RiskSubmissionMutationError),
    ReservationRead(RiskStateMutationError),
    Submit(SubmissionAuthorityError),
}

impl From<RiskSubmissionMutationError> for LifecycleReconciliationError {
    fn from(error: RiskSubmissionMutationError) -> Self {
        Self::State(error)
    }
}

impl LifecycleReconciler {
    pub fn new(owner: RiskStateOwner) -> Self {
        Self { owner }
    }

    pub fn reconcile_restart<B>(
        &self,
        truth: NtExecutionTruth,
        boundary: &mut B,
        now_unix_nanos: u64,
    ) -> Result<ReconciliationSummary, LifecycleReconciliationError>
    where
        B: LiveSubmitBoundary<Error = SubmissionAuthorityError>,
    {
        let authority = SubmissionAuthority::new(self.owner.clone());
        for intent in self.owner.durable_submission_intents()? {
            let idempotency_key = intent.idempotency_key().to_string();
            if self
                .owner
                .live_submission_record(&idempotency_key)?
                .is_some()
            {
                continue;
            }
            if truth
                .order_status_reports
                .iter()
                .any(|report| report.client_order_id == intent.client_order_id())
            {
                self.owner.record_live_submission(
                    &idempotency_key,
                    LiveSubmissionRecord {
                        client_order_id: intent.client_order_id(),
                        risk_state_version: intent.submitted_risk_state_version,
                    },
                )?;
            } else {
                authority
                    .submit_durable_intent(&idempotency_key, boundary, now_unix_nanos)
                    .map_err(LifecycleReconciliationError::Submit)?;
            }
        }

        let risk_state_version = self.owner.complete_reconciliation()?;
        let live_order_count = self.owner.live_submission_records()?.len();
        let reservation_count = self
            .owner
            .reservation_records()
            .map_err(LifecycleReconciliationError::ReservationRead)?
            .len();

        Ok(ReconciliationSummary {
            live_order_count,
            reservation_count,
            risk_state_version,
        })
    }
}
