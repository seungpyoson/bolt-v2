use nautilus_model::identifiers::ClientOrderId;
use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{LiveSubmissionRecord, ReservationLifecycleState, RiskStateVersion},
    state_owner::{
        LifecycleMutationResult, RiskStateMutationError, RiskStateOwner,
        RiskSubmissionMutationError,
    },
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
    pub settlement_reports: Vec<NtSettlementTruth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtOrderStatusReportTruth {
    pub client_order_id: ClientOrderId,
    pub status: NtOrderStatusTruth,
    pub event_id: String,
    pub ts_event_unix_nanos: u64,
    pub event_sequence: Option<u64>,
    pub ts_init_unix_nanos: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtOrderStatusTruth {
    Open,
    CancelConfirmed,
    ExpiredConfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtFillReportTruth {
    pub client_order_id: ClientOrderId,
    pub event_id: String,
    pub ts_event_unix_nanos: u64,
    pub event_sequence: Option<u64>,
    pub ts_init_unix_nanos: u64,
    pub fill_quantity: Decimal,
    pub remaining_fillable_quantity: Decimal,
    pub actual_conservative_liquidation_value: Decimal,
    pub actual_governor_cost_basis: Decimal,
    pub terminal_cash_flows: Vec<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtSettlementTruth {
    pub client_order_id: ClientOrderId,
    pub event_id: String,
    pub ts_event_unix_nanos: u64,
    pub event_sequence: Option<u64>,
    pub ts_init_unix_nanos: u64,
    pub terminal_final: bool,
    pub reconciliation_complete: bool,
    pub conservative_liquidation_value: Decimal,
    pub governor_cost_basis: Decimal,
    pub terminal_cash_flows: Vec<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationSummary {
    pub live_order_count: usize,
    pub reservation_count: usize,
    pub risk_state_version: RiskStateVersion,
}

enum NtExecutionReplayEvent {
    OrderStatus(NtOrderStatusReportTruth),
    Fill(NtFillReportTruth),
    Settlement(NtSettlementTruth),
}

impl NtExecutionReplayEvent {
    const fn ordering_key(&self) -> (u64, Option<u64>) {
        match self {
            Self::OrderStatus(report) => (report.ts_event_unix_nanos, report.event_sequence),
            Self::Fill(report) => (report.ts_event_unix_nanos, report.event_sequence),
            Self::Settlement(report) => (report.ts_event_unix_nanos, report.event_sequence),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleEventSummary {
    pub risk_state_version: RiskStateVersion,
    pub lifecycle_state: ReservationLifecycleState,
}

impl From<LifecycleMutationResult> for LifecycleEventSummary {
    fn from(result: LifecycleMutationResult) -> Self {
        Self {
            risk_state_version: result.risk_state_version,
            lifecycle_state: result.lifecycle_state,
        }
    }
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

    pub fn apply_order_status_truth(
        &self,
        truth: NtOrderStatusReportTruth,
    ) -> Result<LifecycleEventSummary, LifecycleReconciliationError> {
        let target_state = match truth.status {
            NtOrderStatusTruth::Open => ReservationLifecycleState::Open,
            NtOrderStatusTruth::CancelConfirmed => ReservationLifecycleState::CancelConfirmed,
            NtOrderStatusTruth::ExpiredConfirmed => ReservationLifecycleState::ExpiredConfirmed,
        };
        self.owner
            .apply_order_lifecycle_state(
                truth.client_order_id,
                &truth.event_id,
                truth.ts_event_unix_nanos,
                truth.event_sequence,
                target_state,
            )
            .map(LifecycleEventSummary::from)
            .map_err(LifecycleReconciliationError::State)
    }

    pub fn apply_fill_truth(
        &self,
        truth: NtFillReportTruth,
    ) -> Result<LifecycleEventSummary, LifecycleReconciliationError> {
        self.owner
            .apply_authoritative_fill(
                truth.client_order_id,
                &truth.event_id,
                truth.ts_event_unix_nanos,
                truth.event_sequence,
                truth.fill_quantity,
                truth.remaining_fillable_quantity,
                truth.actual_conservative_liquidation_value,
                truth.actual_governor_cost_basis,
                truth.terminal_cash_flows,
            )
            .map(LifecycleEventSummary::from)
            .map_err(LifecycleReconciliationError::State)
    }

    pub fn apply_settlement_truth(
        &self,
        truth: NtSettlementTruth,
    ) -> Result<LifecycleEventSummary, LifecycleReconciliationError> {
        self.owner
            .apply_settlement_truth(
                truth.client_order_id,
                &truth.event_id,
                truth.ts_event_unix_nanos,
                truth.event_sequence,
                truth.terminal_final,
                truth.reconciliation_complete,
                truth.conservative_liquidation_value,
                truth.governor_cost_basis,
                truth.terminal_cash_flows,
            )
            .map(LifecycleEventSummary::from)
            .map_err(LifecycleReconciliationError::State)
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
        let known_at_venue_client_order_ids: Vec<ClientOrderId> = truth
            .order_status_reports
            .iter()
            .map(|report| report.client_order_id)
            .chain(
                truth
                    .fill_reports
                    .iter()
                    .map(|report| report.client_order_id),
            )
            .chain(
                truth
                    .settlement_reports
                    .iter()
                    .map(|report| report.client_order_id),
            )
            .collect();
        for intent in self.owner.durable_submission_intents()? {
            let idempotency_key = intent.idempotency_key().to_string();
            if self
                .owner
                .live_submission_record(&idempotency_key)?
                .is_some()
            {
                continue;
            }
            if known_at_venue_client_order_ids.contains(&intent.client_order_id()) {
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

        let mut replay_events = Vec::with_capacity(
            truth.order_status_reports.len()
                + truth.fill_reports.len()
                + truth.settlement_reports.len(),
        );
        replay_events.extend(
            truth
                .order_status_reports
                .into_iter()
                .map(NtExecutionReplayEvent::OrderStatus),
        );
        replay_events.extend(
            truth
                .fill_reports
                .into_iter()
                .map(NtExecutionReplayEvent::Fill),
        );
        replay_events.extend(
            truth
                .settlement_reports
                .into_iter()
                .map(NtExecutionReplayEvent::Settlement),
        );
        replay_events.sort_by_key(NtExecutionReplayEvent::ordering_key);

        for event in replay_events {
            match event {
                NtExecutionReplayEvent::OrderStatus(report) => {
                    self.apply_order_status_truth(report)?;
                }
                NtExecutionReplayEvent::Fill(report) => {
                    self.apply_fill_truth(report)?;
                }
                NtExecutionReplayEvent::Settlement(report) => {
                    self.apply_settlement_truth(report)?;
                }
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
