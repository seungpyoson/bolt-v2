use std::collections::{BTreeMap, BTreeSet};

use rust_decimal::Decimal;

use crate::{
    bolt_v3_capital_reservation::ReservationLedger,
    bolt_v3_risk_reservation_substrate::{
        contracts::{
            AdmissionCandidate, AdmissionToken, PoolId, RiskAssessment, RiskSizingView,
            RiskStateVersion,
        },
        risk_classifier::ConcentrationBucket,
        risk_kernel::{RiskKernelError, RiskKernelInput},
        state_owner::RiskStateMutationError,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstrateReservationLedger {
    ledger: ReservationLedger,
    risk_state_version: RiskStateVersion,
}

impl SubstrateReservationLedger {
    pub fn from_existing_ledger(
        ledger: ReservationLedger,
        risk_state_version: RiskStateVersion,
    ) -> Self {
        Self {
            ledger,
            risk_state_version,
        }
    }

    pub fn ledger(&self) -> &ReservationLedger {
        &self.ledger
    }

    pub const fn risk_state_version(&self) -> RiskStateVersion {
        self.risk_state_version
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReservationTransaction {
    pub candidate: AdmissionCandidate,
    pub kernel_input: RiskKernelInput,
    pub sizing_view: RiskSizingView,
    pub safety_state: BoundReusableSafetyState,
    pub caller_diagnostics: Option<CallerRiskDiagnostics>,
    pub now_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundReusableSafetyState {
    pub risk_state_version: RiskStateVersion,
    pub kill_switch_latched: bool,
    pub loss_governor_halted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerRiskDiagnostics {
    pub collateral_required: Decimal,
    pub equity_floor_stress_loss: Decimal,
    pub governor_realized_loss: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskDiagnosticMismatch {
    pub metric: CallerRiskMetric,
    pub caller_value: Decimal,
    pub authoritative_value: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallerRiskMetric {
    CollateralRequired,
    EquityFloorStressLoss,
    GovernorRealizedLoss,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RiskCapDimension {
    Collateral,
    EquityFloorStressLoss,
    GovernorRealizedLoss,
    GlobalStressLoss,
    ConcentrationBucket(ConcentrationBucket),
    OpenOrderCount,
    PositionQuantity,
    KillSwitchLatch,
    LossGovernorHalt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReservationCommit {
    pub admission_token: AdmissionToken,
    pub assessment: RiskAssessment,
    pub evaluated_dimensions: BTreeSet<RiskCapDimension>,
    pub diagnostic_mismatches: Vec<RiskDiagnosticMismatch>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReservationRejection {
    pub evaluated_dimensions: BTreeSet<RiskCapDimension>,
    pub breached_dimensions: BTreeSet<RiskCapDimension>,
    pub diagnostic_mismatches: Vec<RiskDiagnosticMismatch>,
    pub token_issued: Option<AdmissionToken>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskReservationError {
    StaleRiskStateVersion {
        expected: RiskStateVersion,
        actual: RiskStateVersion,
    },
    SafetyStateVersionMismatch {
        expected: RiskStateVersion,
        actual: RiskStateVersion,
    },
    PoolMismatch,
    CandidateExpired,
    PermitVersionMismatch,
    InvalidCandidate,
    Kernel(RiskKernelError),
    Rejected(RiskReservationRejection),
    StateMutation(RiskStateMutationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubstrateReservationRecord {
    pub pool_id: PoolId,
    pub admission_token: AdmissionToken,
    pub assessment: RiskAssessment,
    pub evaluated_dimensions: BTreeSet<RiskCapDimension>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReservationTotals {
    collateral_required: Decimal,
    equity_floor_stress_loss: Decimal,
    governor_realized_loss: Decimal,
    global_stress_loss: Decimal,
    bucket_stress_loss: BTreeMap<ConcentrationBucket, Decimal>,
    open_order_count: u64,
    position_quantity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReservationEvaluation {
    pub evaluated_dimensions: BTreeSet<RiskCapDimension>,
    pub breached_dimensions: BTreeSet<RiskCapDimension>,
    pub diagnostic_mismatches: Vec<RiskDiagnosticMismatch>,
}

impl RiskReservationTotals {
    pub fn empty() -> Self {
        Self {
            collateral_required: Decimal::ZERO,
            equity_floor_stress_loss: Decimal::ZERO,
            governor_realized_loss: Decimal::ZERO,
            global_stress_loss: Decimal::ZERO,
            bucket_stress_loss: BTreeMap::new(),
            open_order_count: 0,
            position_quantity: Decimal::ZERO,
        }
    }

    pub fn reserved_bucket_stress_loss(&self, bucket: &ConcentrationBucket) -> Decimal {
        self.bucket_stress_loss
            .get(bucket)
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    pub fn apply(&mut self, transaction: &RiskReservationTransaction, assessment: &RiskAssessment) {
        self.collateral_required += assessment.collateral_required;
        self.equity_floor_stress_loss += assessment.equity_floor_stress_loss;
        self.governor_realized_loss += assessment.governor_realized_loss;
        self.global_stress_loss += assessment.equity_floor_stress_loss;
        for bucket in &transaction.kernel_input.candidate.buckets {
            *self
                .bucket_stress_loss
                .entry(bucket.clone())
                .or_insert(Decimal::ZERO) += assessment.equity_floor_stress_loss;
        }
        self.open_order_count = self.open_order_count.saturating_add(1);
        self.position_quantity += transaction.candidate.quantity;
    }
}

impl RiskReservationTransaction {
    pub fn validate_static(
        &self,
        lease_pool_id: &PoolId,
        current_version: RiskStateVersion,
    ) -> Result<(), RiskReservationError> {
        if &self.candidate.pool_id != lease_pool_id {
            return Err(RiskReservationError::PoolMismatch);
        }
        if self.candidate.source_view_version != current_version
            || self.sizing_view.risk_state_version != current_version
            || self.kernel_input.risk_state_version != current_version
        {
            return Err(RiskReservationError::StaleRiskStateVersion {
                expected: current_version,
                actual: self.candidate.source_view_version,
            });
        }
        if self.safety_state.risk_state_version != current_version {
            return Err(RiskReservationError::SafetyStateVersionMismatch {
                expected: current_version,
                actual: self.safety_state.risk_state_version,
            });
        }
        if self.candidate.expires_at_unix_nanos <= self.now_unix_nanos {
            return Err(RiskReservationError::CandidateExpired);
        }
        if self.candidate.sizing_permit.source_view_version != self.candidate.source_view_version {
            return Err(RiskReservationError::PermitVersionMismatch);
        }
        if self.candidate.intent_id.trim().is_empty()
            || self.candidate.idempotency_key.trim().is_empty()
            || self.candidate.instrument_id.trim().is_empty()
            || self.candidate.policy_epoch_id.trim().is_empty()
            || self.candidate.quantity <= Decimal::ZERO
            || self.candidate.max_cash_outlay < Decimal::ZERO
        {
            return Err(RiskReservationError::InvalidCandidate);
        }
        Ok(())
    }
}

pub fn evaluate_stateful_caps(
    totals: &RiskReservationTotals,
    transaction: &RiskReservationTransaction,
    assessment: &RiskAssessment,
) -> RiskReservationEvaluation {
    let mut evaluated_dimensions = BTreeSet::new();
    let mut breached_dimensions = BTreeSet::new();

    evaluate_decimal_dimension(
        RiskCapDimension::Collateral,
        totals.collateral_required,
        assessment.collateral_required,
        transaction.sizing_view.free_collateral,
        &mut evaluated_dimensions,
        &mut breached_dimensions,
    );
    evaluate_decimal_dimension(
        RiskCapDimension::EquityFloorStressLoss,
        totals.equity_floor_stress_loss,
        assessment.equity_floor_stress_loss,
        transaction.sizing_view.equity_floor_headroom,
        &mut evaluated_dimensions,
        &mut breached_dimensions,
    );
    evaluate_decimal_dimension(
        RiskCapDimension::GovernorRealizedLoss,
        totals.governor_realized_loss,
        assessment.governor_realized_loss,
        transaction.sizing_view.governor_headroom,
        &mut evaluated_dimensions,
        &mut breached_dimensions,
    );
    evaluate_decimal_dimension(
        RiskCapDimension::GlobalStressLoss,
        totals.global_stress_loss,
        assessment.equity_floor_stress_loss,
        transaction.sizing_view.global_stress_loss_headroom,
        &mut evaluated_dimensions,
        &mut breached_dimensions,
    );
    for bucket in &transaction.kernel_input.candidate.buckets {
        let dimension = RiskCapDimension::ConcentrationBucket(bucket.clone());
        evaluate_decimal_dimension(
            dimension,
            totals.reserved_bucket_stress_loss(bucket),
            assessment.equity_floor_stress_loss,
            transaction
                .sizing_view
                .bucket_stress_loss_headrooms
                .get(bucket)
                .copied()
                .unwrap_or(Decimal::ZERO),
            &mut evaluated_dimensions,
            &mut breached_dimensions,
        );
    }
    evaluated_dimensions.insert(RiskCapDimension::OpenOrderCount);
    if totals.open_order_count >= transaction.sizing_view.open_order_headroom {
        breached_dimensions.insert(RiskCapDimension::OpenOrderCount);
    }
    evaluate_decimal_dimension(
        RiskCapDimension::PositionQuantity,
        totals.position_quantity,
        transaction.candidate.quantity,
        transaction.sizing_view.position_quantity_headroom,
        &mut evaluated_dimensions,
        &mut breached_dimensions,
    );
    evaluated_dimensions.insert(RiskCapDimension::KillSwitchLatch);
    if transaction.safety_state.kill_switch_latched {
        breached_dimensions.insert(RiskCapDimension::KillSwitchLatch);
    }
    evaluated_dimensions.insert(RiskCapDimension::LossGovernorHalt);
    if transaction.safety_state.loss_governor_halted {
        breached_dimensions.insert(RiskCapDimension::LossGovernorHalt);
    }

    RiskReservationEvaluation {
        evaluated_dimensions,
        breached_dimensions,
        diagnostic_mismatches: diagnostic_mismatches(transaction, assessment),
    }
}

pub fn build_admission_token(
    transaction: &RiskReservationTransaction,
    risk_state_version: RiskStateVersion,
) -> AdmissionToken {
    AdmissionToken {
        token_id: transaction.candidate.idempotency_key.clone(),
        pool_id: transaction.candidate.pool_id.clone(),
        risk_state_version,
        reservation_id: transaction.candidate.intent_id.clone(),
        expires_at_unix_nanos: transaction.candidate.expires_at_unix_nanos,
    }
}

fn evaluate_decimal_dimension(
    dimension: RiskCapDimension,
    current_reserved: Decimal,
    candidate_required: Decimal,
    headroom: Decimal,
    evaluated_dimensions: &mut BTreeSet<RiskCapDimension>,
    breached_dimensions: &mut BTreeSet<RiskCapDimension>,
) {
    evaluated_dimensions.insert(dimension.clone());
    if current_reserved + candidate_required > headroom {
        breached_dimensions.insert(dimension);
    }
}

fn diagnostic_mismatches(
    transaction: &RiskReservationTransaction,
    assessment: &RiskAssessment,
) -> Vec<RiskDiagnosticMismatch> {
    let Some(diagnostics) = &transaction.caller_diagnostics else {
        return Vec::new();
    };
    [
        (
            CallerRiskMetric::CollateralRequired,
            diagnostics.collateral_required,
            assessment.collateral_required,
        ),
        (
            CallerRiskMetric::EquityFloorStressLoss,
            diagnostics.equity_floor_stress_loss,
            assessment.equity_floor_stress_loss,
        ),
        (
            CallerRiskMetric::GovernorRealizedLoss,
            diagnostics.governor_realized_loss,
            assessment.governor_realized_loss,
        ),
    ]
    .into_iter()
    .filter_map(|(metric, caller_value, authoritative_value)| {
        (caller_value != authoritative_value).then_some(RiskDiagnosticMismatch {
            metric,
            caller_value,
            authoritative_value,
        })
    })
    .collect()
}
