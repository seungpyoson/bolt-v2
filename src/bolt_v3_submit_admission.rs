use crate::bolt_v3_capital_admission::{
    CapitalAdmissionDecision, CapitalAdmissionGate, CapitalAdmissionGateInputs,
    CapitalAdmissionPolicy, CapitalAdmissionReason, CapitalAdmissionRequest,
    CapitalAdmissionReservationEvidence, IntentLiquidity, IntentOrderKind, IntentSide,
    LiabilityError, ProductAdmissionSnapshot, ProductKind,
};
use crate::bolt_v3_capital_admission_state::{
    NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
    PortfolioCapitalAdmissionSnapshot, ProviderCollateralAllowanceSnapshot,
    ReservationLedgerSnapshot,
};
use crate::bolt_v3_capital_reservation::{
    CapitalPoolSnapshot, ReservationRejectionReason, ReservationRequest,
};
#[cfg(test)]
use crate::bolt_v3_current_evidence::DecisionEvidenceRecorder;
use crate::bolt_v3_current_evidence::{
    AdmissionDecisionOutcome, AdmissionDetails, AdmissionEconomicsCalculationFactor,
    AdmissionEconomicsClass, AdmissionEconomicsComponent, AdmissionEconomicsDetails,
    AdmissionEconomicsInventoryApplication, AdmissionEconomicsKind, AdmissionEconomicsNativeEffect,
    AdmissionEconomicsNativeUnit, AdmissionEconomicsPointEstimate,
    AdmissionEconomicsRiskBoundAuthority, AdmissionEconomicsScope, AdmissionEconomicsSource,
    AdmissionEconomicsTreatment, AdmissionEconomicsValuation, AdmissionRejectionReason,
    AdmittedEntryAdmissionFact, BasketAdmissionDetails, BasketAdmissionGrantedFact,
    BasketAdmissionIntentKind, BasketAdmittedLeg, CapitalAdmissionRebuildFact,
    CapitalAdmissionRebuildOutcome, CapitalAdmissionRebuildSource, CapitalAdmissionRejectionReason,
    CommittedAdmission, EvidenceOrderSide, LossGovernorHaltFact,
    LossHaltReason as EvidenceLossHaltReason, LossSnapshotSource,
    LossSnapshotStaleReason as EvidenceLossSnapshotStaleReason, NonBlockingRecordOutcome,
    OrderIntentDetails, OrderRejectFact, OrderRejectReason, OrderRejectSource,
    ProviderCollateralAllowanceCaptureEndpoint as EvidenceCaptureEndpoint,
    ProviderCollateralAllowanceCaptureErrorClass as EvidenceCaptureErrorClass,
    ProviderCollateralAllowanceCaptureFailureFact, RecordFailure, RejectedEntryAdmissionFact,
    ReservationAttribution, ReservationProductKind, ReservationRecoveryEvent,
    ReservationRecoveryFacts, RiskReducingExitAdmissionFact, StaleLossReason,
    SubmitAdmissionEvidence, SubmitReservationFillFact, SubmitReservationFillSource,
};
#[cfg(feature = "test-current-evidence-inspection")]
use crate::bolt_v3_current_evidence::{
    BasketAdmissionRejectedFact, BasketAdmissionRejectionReason,
};
use crate::bolt_v3_economics_runtime::{EconomicsAdmission, EconomicsAdmissionPurpose};
use crate::bolt_v3_evidence_sampling::{EpisodeFirstNs, evict_oldest_episodes_over_cap};
use crate::bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind};
use crate::bolt_v3_loss_governor::{
    LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSnapshotDiagnostics,
    LossSnapshotStaleReason, LossSourceObservationTimestamps,
    evaluate_loss_admission_with_observations, loss_snapshot_stale_reason,
};
use crate::bolt_v3_numeric::{is_positive_finite, notional_float_tolerance};
use crate::bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceCaptureFailureEvidence;
use crate::economics::EconomicsError;
use crate::integrations::nautilus::economics::economics_order_binding;
use anyhow::Context;
use nautilus_model::{
    data::{QuoteTick, TradeTick},
    enums::{OrderSide, PositionSide},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::Price,
};
use rust_decimal::Decimal;
use std::{
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3SubmitIntentKind {
    Entry,
    RiskReducingExit,
}

pub const PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE_RESERVATION_SOURCE: &str =
    "provider_collateral_allowance_capture_failure";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3ExchangeMutationCounts {
    pub submit: u64,
    pub cancel: u64,
    pub modify: u64,
    pub transfer: u64,
    pub account: u64,
}

impl BoltV3ExchangeMutationCounts {
    pub const fn none() -> Self {
        Self {
            submit: 0,
            cancel: 0,
            modify: 0,
            transfer: 0,
            account: 0,
        }
    }

    pub fn total(self) -> Result<u64, BoltV3SubmitAdmissionError> {
        self.submit
            .checked_add(self.cancel)
            .and_then(|total| total.checked_add(self.modify))
            .and_then(|total| total.checked_add(self.transfer))
            .and_then(|total| total.checked_add(self.account))
            .ok_or(BoltV3SubmitAdmissionError::ExchangeMutationCountOverflow)
    }
}

pub fn validate_no_exchange_mutations(
    counts: BoltV3ExchangeMutationCounts,
) -> Result<u64, BoltV3SubmitAdmissionError> {
    let mutation_count = counts.total()?;
    if mutation_count == 0 {
        return Ok(mutation_count);
    }
    Err(BoltV3SubmitAdmissionError::ExchangeMutationsObserved { mutation_count })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3LiveSubmitApprovalLimits {
    pub max_order_count: u32,
    pub max_order_notional: Decimal,
}

pub fn live_submit_count_cap_outcome(
    current_count: u32,
    claim_count: u32,
    max_order_count: u32,
) -> AdmissionDecisionOutcome {
    match current_count.checked_add(claim_count) {
        Some(total) if total <= max_order_count => AdmissionDecisionOutcome::Admitted,
        Some(_) | None => {
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted)
        }
    }
}

fn stale_loss_reason_key(reason: StaleLossReason) -> &'static str {
    match reason {
        StaleLossReason::MissingSnapshot => "missing_snapshot",
        StaleLossReason::SourceEmpty => "source_empty",
        StaleLossReason::FutureDated => "future_dated",
        StaleLossReason::AgeExceeded => "age_exceeded",
        StaleLossReason::MissingRequiredField => "missing_required_field",
    }
}

fn loss_snapshot_stale_reason_to_halt_reason(reason: LossSnapshotStaleReason) -> StaleLossReason {
    match reason {
        LossSnapshotStaleReason::MissingSnapshot => StaleLossReason::MissingSnapshot,
        LossSnapshotStaleReason::SourceEmpty => StaleLossReason::SourceEmpty,
        LossSnapshotStaleReason::FutureDated => StaleLossReason::FutureDated,
        LossSnapshotStaleReason::AgeExceeded => StaleLossReason::AgeExceeded,
        LossSnapshotStaleReason::MissingRequiredField => StaleLossReason::MissingRequiredField,
    }
}

fn capital_admission_rejection_reason(
    reason: ReservationRejectionReason,
) -> CapitalAdmissionRejectionReason {
    match reason {
        ReservationRejectionReason::MissingEvidence => {
            CapitalAdmissionRejectionReason::MissingEvidence
        }
        ReservationRejectionReason::StaleRequest => CapitalAdmissionRejectionReason::StaleRequest,
        ReservationRejectionReason::PoolMismatch => CapitalAdmissionRejectionReason::PoolMismatch,
        ReservationRejectionReason::OverBudget => CapitalAdmissionRejectionReason::OverBudget,
        ReservationRejectionReason::InvalidRequest => {
            CapitalAdmissionRejectionReason::InvalidRequest
        }
        ReservationRejectionReason::CollateralGroupMismatch => {
            CapitalAdmissionRejectionReason::CollateralGroupMismatch
        }
        ReservationRejectionReason::DuplicateReservation => {
            CapitalAdmissionRejectionReason::DuplicateReservation
        }
        ReservationRejectionReason::UnknownReservation => {
            CapitalAdmissionRejectionReason::UnknownReservation
        }
        ReservationRejectionReason::UnknownRelease => {
            CapitalAdmissionRejectionReason::UnknownRelease
        }
        ReservationRejectionReason::ReconciliationRequired => {
            CapitalAdmissionRejectionReason::ReconciliationRequired
        }
    }
}

fn capital_admission_rebuild_outcome(
    decision: &BoltV3SubmitCapitalAdmissionRebuildDecision,
) -> Result<CapitalAdmissionRebuildOutcome, BoltV3SubmitAdmissionError> {
    match (decision.accepted, decision.reason) {
        (true, None) => Ok(CapitalAdmissionRebuildOutcome::Accepted),
        (false, Some(reason)) => Ok(CapitalAdmissionRebuildOutcome::Rejected(
            capital_admission_rejection_reason(reason),
        )),
        (false, None) => Err(BoltV3SubmitAdmissionError::InvariantViolation {
            invariant:
                BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRebuildRejectionReason,
        }),
        (true, Some(_)) => Err(BoltV3SubmitAdmissionError::InvariantViolation {
            invariant:
                BoltV3SubmitAdmissionInvariant::AcceptedCapitalAdmissionRebuildHasRejectionReason,
        }),
    }
}

fn admission_outcome_key(outcome: &AdmissionDecisionOutcome) -> &'static str {
    match outcome {
        AdmissionDecisionOutcome::Admitted => "admitted",
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::KillSwitchLatched) => {
            "rejected_kill_switch_latched"
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted) => {
            "rejected_loss_governor_halted"
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NonPositiveNotional) => {
            "rejected_non_positive_notional"
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NotionalCapExceeded) => {
            "rejected_notional_cap_exceeded"
        }
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::ClientOrderAlreadyAuthorized,
        ) => "rejected_client_order_already_authorized",
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::InvalidRiskReducingExitProof,
        ) => "rejected_invalid_risk_reducing_exit_proof",
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted) => {
            "rejected_count_cap_exhausted"
        }
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid,
        ) => "rejected_kill_switch_forced_reduction_proof_invalid",
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded,
        ) => "rejected_kill_switch_forced_reduction_cap_exceeded",
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::EconomicsSealRejected) => {
            "rejected_economics_seal"
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission) => {
            "rejected_capital_admission"
        }
    }
}

fn submit_admission_order_side_key(side: OrderSide) -> &'static str {
    match side {
        OrderSide::Buy => "buy",
        OrderSide::Sell => "sell",
        OrderSide::NoOrderSide => "no_order_side",
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Arc<Mutex<BoltV3SubmitAdmissionInner>>,
    reject_episodes: Mutex<BTreeMap<String, RejectEpisode>>,
    decision_evidence: SubmitAdmissionEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitAdmissionOperatorHealthSnapshot {
    pub kill_switch_state: KillSwitchState,
    pub capital_admission_state: Option<NtDerivedCapitalAdmissionState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3SubmitAdmissionHealthReadError {
    StateLockPoisoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3LossFreshness {
    pub account_state_count: u64,
    pub portfolio_snapshot_count: u64,
    pub position_event_count: u64,
    pub last_account_state_ts_ns: Option<u64>,
    pub last_portfolio_snapshot_ts_ns: Option<u64>,
    pub last_position_event_ts_ns: Option<u64>,
}

impl BoltV3LossFreshness {
    const fn empty() -> Self {
        Self {
            account_state_count: 0,
            portfolio_snapshot_count: 0,
            position_event_count: 0,
            last_account_state_ts_ns: None,
            last_portfolio_snapshot_ts_ns: None,
            last_position_event_ts_ns: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LossHaltEpisode {
    count: u32,
    first_halt_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RejectEpisode {
    count: u32,
    first_ns: u64,
    last_client_order_id: String,
}

impl EpisodeFirstNs for RejectEpisode {
    fn first_ns(&self) -> u64 {
        self.first_ns
    }
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    kill_switch_state: KillSwitchState,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    admitted_order_count: u32,
    admitted_order_count_by_execution_client: BTreeMap<String, u32>,
    capital_admission_state_revision: u64,
    evidence_free_non_reservation_admissions:
        BTreeMap<String, BoltV3EvidenceFreeNonReservationPhase>,
    committed_admission_authority: ReservationRecoveryFacts,
    loss_policy: Option<LossGovernorPolicy>,
    loss_snapshot: Option<LossSnapshot>,
    loss_source_observations: LossSourceObservationTimestamps,
    loss_freshness: BoltV3LossFreshness,
    loss_halt_episodes: BTreeMap<String, LossHaltEpisode>,
    capital_admission: Option<BoltV3SubmitCapitalAdmissionState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoltV3EvidenceFreeNonReservationPhase {
    // Before the sink boundary this identity is removable only by the exact
    // permit rollback. After the boundary it remains process-local authority
    // for the rest of the process: without a committed evidence record,
    // projection omission cannot prove that reusing the client order ID is
    // safe.
    PermitHeld,
    SinkInvoked,
}

#[derive(Debug, Clone)]
pub(crate) struct BoltV3OpenOrderAdmissionAuthoritySnapshot {
    committed: ReservationRecoveryFacts,
    evidence_free_non_reservation_admissions:
        BTreeMap<String, BoltV3EvidenceFreeNonReservationPhase>,
}

impl BoltV3OpenOrderAdmissionAuthoritySnapshot {
    pub(crate) fn authorizes_non_reservation_order(&self, client_order_id: &str) -> bool {
        self.committed
            .authorizes_non_reservation_order(client_order_id)
            || self
                .evidence_free_non_reservation_admissions
                .get(client_order_id)
                == Some(&BoltV3EvidenceFreeNonReservationPhase::SinkInvoked)
    }

    pub(crate) fn reservation_attribution(
        &self,
        client_order_id: &str,
    ) -> Option<&ReservationAttribution> {
        self.committed.reservation_attribution(client_order_id)
    }
}

#[derive(Debug, Clone)]
struct BoltV3SubmitCapitalAdmissionState {
    venue_id: String,
    account_id: String,
    product_kind: ProductKind,
    collateral_currency: String,
    capital_pool: CapitalPoolSnapshot,
    policy: CapitalAdmissionPolicy,
    state: Option<NtDerivedCapitalAdmissionState>,
    latest_reservation_mutation_observed_at_ns: Option<u64>,
    provider_collateral_allowance_capture_failure_source: Option<String>,
    provider_collateral_allowance_capture_failure_observed_at_ns: Option<u64>,
    gate: CapitalAdmissionGate,
    next_sequence: u64,
    client_order_reservations: BTreeMap<String, BoltV3SubmitReservationRecord>,
    unresolved_lifecycle_reservations: (usize, usize),
    fill_evidence_integrity_failed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationRecord {
    attribution: ReservationAttribution,
    phase: BoltV3SubmitReservationPhase,
    phase_revision: u64,
    unresolved_in_projection: bool,
    fill_metadata: Option<BoltV3SubmitReservationFillMetadata>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3SubmitReservationPhase {
    Reserved,
    SinkInvoked,
    ObservedOpen,
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3SubmitReservationLifecycleSnapshot {
    pub phase: BoltV3SubmitReservationPhase,
    pub phase_revision: u64,
}

#[derive(Debug)]
pub(crate) struct BoltV3SubmitTerminalReservationCapability {
    client_order_id: String,
    phase: BoltV3SubmitReservationPhase,
    phase_revision: u64,
}

impl BoltV3SubmitTerminalReservationCapability {
    pub(crate) fn client_order_id(&self) -> &str {
        &self.client_order_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoltV3SubmitTerminalReservationDecision {
    Retired,
    NotRetained,
    StaleCapability,
    RevisionExhausted,
    LedgerMismatch,
    EvidenceRejected,
    InvariantViolation(BoltV3SubmitAdmissionError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationFillMetadata {
    instrument_id: String,
    side: BoltV3CompiledOrderSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionConfig {
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: ProductKind,
    pub collateral_currency: String,
    pub capital_pool: CapitalPoolSnapshot,
    pub policy: CapitalAdmissionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionNtComponents {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioCapitalAdmissionSnapshot,
    pub provider_collateral_allowance: ProviderCollateralAllowanceSnapshot,
    pub order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot,
    pub product_state: ProductAdmissionSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionOpenOrderReservation {
    pub client_order_id: String,
    pub attribution: ReservationAttribution,
    pub submit_reservation_id: String,
    pub collateral_group_id: String,
    pub liability: Decimal,
    pub instrument_id: String,
    pub side: BoltV3CompiledOrderSide,
    pub open_quantity: Decimal,
    pub original_quantity: Decimal,
    pub filled_quantity: Decimal,
    pub liability_factor: Decimal,
    pub additive_liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
    pub observed_at_ns: u64,
    pub evidence_source: CapitalAdmissionRebuildSource,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub reservations: Vec<BoltV3SubmitCapitalAdmissionOpenOrderReservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoltV3SubmitCapitalAdmissionOpenOrderEvidence {
    pub(crate) client_order_id: String,
    pub(crate) instrument_id: String,
    pub(crate) side: BoltV3CompiledOrderSide,
    pub(crate) open_quantity: Decimal,
    pub(crate) limit_price: Decimal,
    pub(crate) observed_at_ns: u64,
    pub(crate) evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance {
    pub account_id: String,
    pub collateral_currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionRebuildDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub rebuilt_reservation_count: usize,
    pub live_reserved_liability: Decimal,
    pub missing_nt_account_cache_balance:
        Option<BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance>,
}

impl BoltV3SubmitCapitalAdmissionRebuildDecision {
    pub fn with_missing_nt_account_cache_balance(
        mut self,
        account_id: String,
        collateral_currency: String,
    ) -> Self {
        self.missing_nt_account_cache_balance =
            Some(BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance {
                account_id,
                collateral_currency,
            });
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitCapitalAdmissionRebuildAuditContext {
    observed_at_ns: u64,
    source: CapitalAdmissionRebuildSource,
    observed_open_order_count: usize,
    all_open_orders_attributed: bool,
    unresolved_sink_invoked_reservation_count: usize,
    unresolved_observed_open_reservation_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionFillUpdate {
    pub client_order_id: String,
    pub trade_id: String,
    pub instrument_id: String,
    pub side: BoltV3CompiledOrderSide,
    pub fill_quantity: Decimal,
    pub observed_at_ns: u64,
    pub reconciliation: bool,
    pub evidence_source: SubmitReservationFillSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitReservationFillEvidenceDecision {
    pub accepted: bool,
    pub unknown_reservation: bool,
}

impl BoltV3SubmitReservationFillEvidenceDecision {
    fn unknown() -> Self {
        Self {
            accepted: false,
            unknown_reservation: true,
        }
    }
}

impl BoltV3SubmitAdmissionState {
    pub fn new(decision_evidence: impl Into<SubmitAdmissionEvidence>) -> Self {
        Self::new_with_live_submit_limits(decision_evidence, BTreeMap::new())
    }

    pub fn new_without_live_submit_limits(
        decision_evidence: impl Into<SubmitAdmissionEvidence>,
    ) -> Self {
        Self::new(decision_evidence)
    }

    pub fn new_with_live_submit_limits(
        decision_evidence: impl Into<SubmitAdmissionEvidence>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence.into(),
            live_submit_approval_limits,
            None,
            None,
        )
    }

    pub(crate) fn new_with_live_submit_limits_and_optional_controls(
        decision_evidence: impl Into<SubmitAdmissionEvidence>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
        loss_policy: Option<LossGovernorPolicy>,
        capital_admission: Option<BoltV3SubmitCapitalAdmissionConfig>,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence.into(),
            live_submit_approval_limits,
            loss_policy,
            capital_admission,
        )
    }

    pub fn new_with_loss_governor(
        decision_evidence: impl Into<SubmitAdmissionEvidence>,
        loss_policy: LossGovernorPolicy,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence.into(),
            BTreeMap::new(),
            Some(loss_policy),
            None,
        )
    }

    pub fn new_with_capital_admission(
        decision_evidence: impl Into<SubmitAdmissionEvidence>,
        capital_admission: BoltV3SubmitCapitalAdmissionConfig,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence.into(),
            BTreeMap::new(),
            None,
            Some(capital_admission),
        )
    }

    pub fn new_with_loss_governor_and_capital_admission(
        decision_evidence: impl Into<SubmitAdmissionEvidence>,
        loss_policy: LossGovernorPolicy,
        capital_admission: BoltV3SubmitCapitalAdmissionConfig,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence.into(),
            BTreeMap::new(),
            Some(loss_policy),
            Some(capital_admission),
        )
    }

    fn new_with_optional_controls(
        decision_evidence: SubmitAdmissionEvidence,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
        loss_policy: Option<LossGovernorPolicy>,
        capital_admission: Option<BoltV3SubmitCapitalAdmissionConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BoltV3SubmitAdmissionInner {
                kill_switch_state: KillSwitchState::Armed,
                live_submit_approval_limits,
                admitted_order_count: 0,
                admitted_order_count_by_execution_client: BTreeMap::new(),
                capital_admission_state_revision: 0,
                evidence_free_non_reservation_admissions: BTreeMap::new(),
                committed_admission_authority: ReservationRecoveryFacts::default(),
                loss_policy,
                loss_snapshot: None,
                // No feed event has been observed at construction time, so all
                // per-source last-seen timestamps are genuinely unavailable.
                loss_source_observations: LossSourceObservationTimestamps::unobserved(),
                loss_freshness: BoltV3LossFreshness::empty(),
                loss_halt_episodes: BTreeMap::new(),
                capital_admission: capital_admission.map(|config| {
                    BoltV3SubmitCapitalAdmissionState {
                        venue_id: config.venue_id,
                        account_id: config.account_id,
                        product_kind: config.product_kind,
                        collateral_currency: config.collateral_currency,
                        capital_pool: config.capital_pool,
                        policy: config.policy,
                        state: None,
                        latest_reservation_mutation_observed_at_ns: None,
                        provider_collateral_allowance_capture_failure_source: None,
                        provider_collateral_allowance_capture_failure_observed_at_ns: None,
                        gate: CapitalAdmissionGate::unreconciled(),
                        next_sequence: 0,
                        client_order_reservations: BTreeMap::new(),
                        unresolved_lifecycle_reservations: (0, 0),
                        fill_evidence_integrity_failed: false,
                    }
                }),
            })),
            reject_episodes: Mutex::new(BTreeMap::new()),
            decision_evidence,
        }
    }

    pub fn update_loss_snapshot(&self, snapshot: LossSnapshot) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.loss_source_observations = snapshot.source_observations;
        inner.loss_snapshot = Some(snapshot);
    }

    pub fn update_loss_source_observations(&self, observations: LossSourceObservationTimestamps) {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_source_observations = observations;
    }

    pub fn update_loss_freshness(&self, freshness: BoltV3LossFreshness) {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_freshness = freshness;
    }

    pub fn loss_governor_policy(&self) -> Option<LossGovernorPolicy> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_policy
            .clone()
    }

    pub fn loss_snapshot(&self) -> Option<LossSnapshot> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_snapshot
            .clone()
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn update_capital_admission_nt_components(
        &self,
        components: BoltV3SubmitCapitalAdmissionNtComponents,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            refresh_capital_admission_state_from_components(capital_admission, components);
        }
    }

    pub(crate) fn capital_admission_state_revision(&self) -> u64 {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .capital_admission_state_revision
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn update_capital_admission_nt_components_after_accepted_allowance_snapshot(
        &self,
        components: BoltV3SubmitCapitalAdmissionNtComponents,
        accepted_allowance_observed_at_ns: u64,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            if capital_admission
                .provider_collateral_allowance_capture_failure_observed_at_ns
                .is_some_and(|failure_observed_at_ns| {
                    accepted_allowance_observed_at_ns > failure_observed_at_ns
                })
            {
                capital_admission.provider_collateral_allowance_capture_failure_source = None;
                capital_admission.provider_collateral_allowance_capture_failure_observed_at_ns =
                    None;
            }
            refresh_capital_admission_state_from_components(capital_admission, components);
        }
    }

    pub fn invalidate_capital_admission_for_nt_projection_request(&self) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if advance_capital_admission_state_revision(&mut inner.capital_admission_state_revision)
            .is_err()
        {
            log::error!(
                "capital admission state revision exhausted while invalidating the NT projection"
            );
        }
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            capital_admission.gate.invalidate_reconciliation();
        }
    }

    pub fn suspend_capital_admission_for_provider_collateral_allowance_capture_failure(
        &self,
        evidence: ProviderCollateralAllowanceCaptureFailureEvidence,
    ) {
        let endpoint = match evidence.endpoint {
            crate::bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceCaptureEndpoint::ProviderCollateralAllowanceSnapshot => {
                EvidenceCaptureEndpoint::ProviderCollateralAllowanceSnapshot
            }
            crate::bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance => {
                EvidenceCaptureEndpoint::ClobBalanceAllowance
            }
        };
        let error_class = match evidence.error_class {
            crate::bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceCaptureErrorClass::Unknown => {
                EvidenceCaptureErrorClass::Unknown
            }
            crate::bolt_v3_provider_collateral_allowance::ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode => {
                EvidenceCaptureErrorClass::TransportOrDecode
            }
        };
        let fact = ProviderCollateralAllowanceCaptureFailureFact {
            source: evidence.source.clone(),
            observed_at_ns: evidence.observed_at_ns,
            endpoint,
            error_class,
            captures_missed: evidence.captures_missed,
        };
        if let NonBlockingRecordOutcome::Failed(error) = self
            .decision_evidence
            .record_provider_collateral_allowance_capture_failure(fact)
        {
            log::error!(
                "failed to record provider collateral allowance capture failure evidence: {error}"
            );
        }
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if advance_capital_admission_state_revision(&mut inner.capital_admission_state_revision)
            .is_err()
        {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            log::error!(
                "capital admission state revision exhausted while recording a collateral allowance capture failure"
            );
            return;
        }
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            capital_admission.gate.invalidate_reconciliation();
            capital_admission.provider_collateral_allowance_capture_failure_source =
                Some(evidence.source);
            capital_admission.provider_collateral_allowance_capture_failure_observed_at_ns =
                Some(evidence.observed_at_ns);
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                evidence.observed_at_ns,
                PROVIDER_COLLATERAL_ALLOWANCE_CAPTURE_FAILURE_RESERVATION_SOURCE.to_string(),
                false,
            );
        }
    }

    pub fn operator_health_snapshot(
        &self,
    ) -> Result<BoltV3SubmitAdmissionOperatorHealthSnapshot, BoltV3SubmitAdmissionHealthReadError>
    {
        let inner = self
            .inner
            .lock()
            .map_err(|_| BoltV3SubmitAdmissionHealthReadError::StateLockPoisoned)?;
        Ok(BoltV3SubmitAdmissionOperatorHealthSnapshot {
            kill_switch_state: inner.kill_switch_state.clone(),
            capital_admission_state: inner
                .capital_admission
                .as_ref()
                .and_then(|capital_admission| capital_admission.state.clone()),
        })
    }

    #[cfg(test)]
    pub(crate) fn poison_inner_for_test(&self) {
        let _guard = self
            .inner
            .lock()
            .expect("test should acquire submit admission lock before poisoning it");
        panic!("poison submit admission inner");
    }

    #[cfg(test)]
    fn poison_reject_episodes_for_test(&self) {
        let _guard = self
            .reject_episodes
            .lock()
            .expect("test should acquire reject episodes lock before poisoning it");
        panic!("poison submit admission reject episodes");
    }

    pub fn capital_admission_state_snapshot(&self) -> Option<NtDerivedCapitalAdmissionState> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .capital_admission
            .as_ref()?
            .state
            .clone()
    }

    pub fn capital_admission_state_observed_at_ns(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .capital_admission
            .as_ref()?
            .state
            .as_ref()
            .map(|state| state.observed_at_ns)
    }

    pub fn capital_admission_configured(&self) -> bool {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .capital_admission
            .is_some()
    }

    pub(crate) fn install_recovered_admission_authority(
        &self,
        authority: Arc<ReservationRecoveryFacts>,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        assert!(
            inner.committed_admission_authority.is_empty(),
            "recovered admission authority must be installed exactly once"
        );
        inner.committed_admission_authority = authority.as_ref().clone();
    }

    pub(crate) fn open_order_admission_authority_snapshot(
        &self,
    ) -> BoltV3OpenOrderAdmissionAuthoritySnapshot {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        BoltV3OpenOrderAdmissionAuthoritySnapshot {
            committed: inner.committed_admission_authority.clone(),
            evidence_free_non_reservation_admissions: inner
                .evidence_free_non_reservation_admissions
                .clone(),
        }
    }

    pub fn loss_governor_configured(&self) -> bool {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_policy
            .is_some()
    }

    pub fn capital_admission_live_reserved_liability(&self) -> Option<Decimal> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let capital_admission = inner.capital_admission.as_ref()?;
        Some(
            capital_admission
                .gate
                .live_reserved_liability(&capital_admission.capital_pool.pool_id),
        )
    }

    pub fn capital_admission_has_live_reservation(&self, client_order_id: &str) -> bool {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner
            .capital_admission
            .as_ref()
            .is_some_and(|capital_admission| {
                capital_admission
                    .client_order_reservations
                    .contains_key(client_order_id)
            })
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn remove_capital_ledger_reservation_only_for_test(&self, client_order_id: &str) -> bool {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(capital_admission) = inner.capital_admission.as_mut() else {
            return false;
        };
        let Some(record) = capital_admission
            .client_order_reservations
            .get(client_order_id)
        else {
            return false;
        };
        let pool_id = record.attribution.capital_pool_id.clone();
        let reservation_id = record.attribution.submit_reservation_id.clone();
        capital_admission
            .gate
            .rollback_uncommitted_reservation(&pool_id, &reservation_id)
            .is_some()
    }

    pub(crate) fn terminal_reservation_capabilities(
        &self,
    ) -> Vec<BoltV3SubmitTerminalReservationCapability> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner
            .capital_admission
            .as_ref()
            .into_iter()
            .flat_map(|state| state.client_order_reservations.iter())
            .filter_map(|(client_order_id, record)| match record.phase {
                BoltV3SubmitReservationPhase::SinkInvoked
                | BoltV3SubmitReservationPhase::ObservedOpen => {
                    Some(BoltV3SubmitTerminalReservationCapability {
                        client_order_id: client_order_id.clone(),
                        phase: record.phase,
                        phase_revision: record.phase_revision,
                    })
                }
                BoltV3SubmitReservationPhase::Reserved => None,
            })
            .collect()
    }

    pub(crate) fn retire_terminal_reservation(
        &self,
        capability: BoltV3SubmitTerminalReservationCapability,
        observed_at_ns: u64,
    ) -> BoltV3SubmitTerminalReservationDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(live) = inner.capital_admission.as_ref() else {
            return BoltV3SubmitTerminalReservationDecision::NotRetained;
        };
        let Some(record) = live
            .client_order_reservations
            .get(&capability.client_order_id)
        else {
            return BoltV3SubmitTerminalReservationDecision::NotRetained;
        };
        match (
            record.phase,
            record.phase_revision,
            capability.phase,
            capability.phase_revision,
        ) {
            (
                BoltV3SubmitReservationPhase::SinkInvoked
                | BoltV3SubmitReservationPhase::ObservedOpen,
                record_revision,
                capability_phase,
                capability_revision,
            ) if record.phase == capability_phase && record_revision == capability_revision => {}
            _ => return BoltV3SubmitTerminalReservationDecision::StaleCapability,
        }
        let Some(next_revision) = inner.capital_admission_state_revision.checked_add(1) else {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            return BoltV3SubmitTerminalReservationDecision::RevisionExhausted;
        };
        let mut candidate = live.clone();
        let Some(record) = candidate
            .client_order_reservations
            .get(&capability.client_order_id)
            .cloned()
        else {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            inner.capital_admission_state_revision = next_revision;
            return BoltV3SubmitTerminalReservationDecision::LedgerMismatch;
        };
        let release = candidate.gate.remove_existing(
            &record.attribution.capital_pool_id,
            &record.attribution.collateral_group_id,
            &record.attribution.submit_reservation_id,
        );
        if !release.accepted {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            inner.capital_admission_state_revision = next_revision;
            return BoltV3SubmitTerminalReservationDecision::LedgerMismatch;
        }
        candidate
            .client_order_reservations
            .remove(&capability.client_order_id);
        let unresolved = unresolved_reservation_counts(&candidate.client_order_reservations);
        candidate.unresolved_lifecycle_reservations = unresolved;
        let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: true,
            reason: None,
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: candidate
                .gate
                .live_reserved_liability(&candidate.capital_pool.pool_id),
            missing_nt_account_cache_balance: None,
        };
        let audit_context = BoltV3SubmitCapitalAdmissionRebuildAuditContext {
            observed_at_ns,
            source: CapitalAdmissionRebuildSource::NtOpenOrderCache,
            observed_open_order_count: 0,
            all_open_orders_attributed: true,
            unresolved_sink_invoked_reservation_count: unresolved.0,
            unresolved_observed_open_reservation_count: unresolved.1,
        };
        match self.record_capital_admission_rebuild(&audit_context, &decision) {
            Ok(true) => {}
            Ok(false) => {
                if let Some(capital_admission) = inner.capital_admission.as_mut() {
                    capital_admission.gate.invalidate_reconciliation();
                }
                inner.capital_admission_state_revision = next_revision;
                return BoltV3SubmitTerminalReservationDecision::EvidenceRejected;
            }
            Err(error) => {
                if let Some(capital_admission) = inner.capital_admission.as_mut() {
                    capital_admission.gate.invalidate_reconciliation();
                }
                inner.capital_admission_state_revision = next_revision;
                return BoltV3SubmitTerminalReservationDecision::InvariantViolation(error);
            }
        }
        inner.capital_admission = Some(candidate);
        inner.capital_admission_state_revision = next_revision;
        BoltV3SubmitTerminalReservationDecision::Retired
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    #[doc(hidden)]
    pub fn retire_terminal_reservation_for_test(
        &self,
        client_order_id: &str,
        observed_at_ns: u64,
    ) -> bool {
        let capability = self
            .terminal_reservation_capabilities()
            .into_iter()
            .find(|capability| capability.client_order_id() == client_order_id);
        capability.is_some_and(|capability| {
            matches!(
                self.retire_terminal_reservation(capability, observed_at_ns),
                BoltV3SubmitTerminalReservationDecision::Retired
            )
        })
    }

    pub fn capital_admission_reconciled(&self) -> Option<bool> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let capital_admission = inner.capital_admission.as_ref()?;
        Some(
            capital_admission.gate.is_reconciled()
                && capital_admission
                    .provider_collateral_allowance_capture_failure_source
                    .is_none(),
        )
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    pub fn unresolved_lifecycle_reservation_counts_for_test(&self) -> Option<(usize, usize)> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .capital_admission
            .as_ref()
            .map(|state| state.unresolved_lifecycle_reservations)
    }

    pub(crate) fn capital_admission_open_order_reservation_from_attribution(
        &self,
        evidence: BoltV3SubmitCapitalAdmissionOpenOrderEvidence,
        attribution: &ReservationAttribution,
    ) -> Option<BoltV3SubmitCapitalAdmissionOpenOrderReservation> {
        if evidence.client_order_id.trim().is_empty()
            || evidence.instrument_id.trim().is_empty()
            || evidence.evidence_label.trim().is_empty()
            || evidence.open_quantity <= Decimal::ZERO
            || evidence.limit_price < Decimal::ZERO
            || evidence.limit_price > Decimal::ONE
        {
            return None;
        }
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let capital_admission = inner.capital_admission.as_ref()?;
        let state = capital_admission.state.as_ref()?;
        let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &state.product_state;
        if attribution.client_order_id != evidence.client_order_id
            || attribution.venue_id != capital_admission.venue_id
            || attribution.account_id != capital_admission.account_id
            || attribution.product_kind
                != reservation_product_kind_from_capital(capital_admission.product_kind)
            || attribution.collateral_currency != capital_admission.collateral_currency
            || attribution.capital_pool_id != capital_admission.capital_pool.pool_id
            || attribution.collateral_group_id != product.collateral_coupled_group_id
            || attribution.instrument_id != evidence.instrument_id
            || attribution.side != evidence_order_side(evidence.side)
        {
            return None;
        }
        if evidence.instrument_id != product.yes_instrument_id
            && evidence.instrument_id != product.no_instrument_id
        {
            return None;
        }
        let submitted_quantity = attribution.submitted_quantity.parse::<Decimal>().ok()?;
        let liability_factor = attribution.liability_factor.parse::<Decimal>().ok()?;
        let additive_liability = attribution.additive_liability.parse::<Decimal>().ok()?;
        let reserved_liability = attribution.reserved_liability.parse::<Decimal>().ok()?;
        if submitted_quantity <= Decimal::ZERO
            || evidence.open_quantity > submitted_quantity
            || liability_factor < Decimal::ZERO
            || liability_factor > Decimal::ONE
            || additive_liability < Decimal::ZERO
            || reserved_liability <= Decimal::ZERO
        {
            return None;
        }
        let expected_liability_factor = match evidence.side.to_capital_admission() {
            IntentSide::Buy => evidence.limit_price,
            IntentSide::Sell => Decimal::ZERO,
        };
        let submitted_liability =
            checked_lifecycle_liability(submitted_quantity, liability_factor, additive_liability)?;
        if liability_factor != expected_liability_factor
            || reserved_liability != submitted_liability
        {
            return None;
        }
        let filled_quantity = submitted_quantity.checked_sub(evidence.open_quantity)?;
        let open_liability = checked_lifecycle_liability(
            evidence.open_quantity,
            liability_factor,
            additive_liability,
        )?;
        Some(BoltV3SubmitCapitalAdmissionOpenOrderReservation {
            client_order_id: evidence.client_order_id,
            attribution: attribution.clone(),
            submit_reservation_id: attribution.submit_reservation_id.clone(),
            collateral_group_id: attribution.collateral_group_id.clone(),
            liability: open_liability,
            instrument_id: attribution.instrument_id.clone(),
            side: evidence.side,
            open_quantity: evidence.open_quantity,
            original_quantity: submitted_quantity,
            filled_quantity,
            liability_factor,
            additive_liability,
            observed_at_ns: evidence.observed_at_ns,
            evidence_label: "bolt_known_reservation_attribution".to_string(),
        })
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn rebuild_capital_admission_open_order_reservations_for_test(
        &self,
        open_order_reservations: Vec<BoltV3SubmitCapitalAdmissionOpenOrderReservation>,
        now_ns: u64,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        self.rebuild_capital_admission_open_order_snapshot(
            BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
                observed_at_ns: now_ns,
                evidence_source: CapitalAdmissionRebuildSource::BoltRecoveredOpenOrderReservations,
                observed_open_order_count: open_order_reservations.len(),
                all_open_orders_attributed: true,
                reservations: open_order_reservations,
            },
            now_ns,
        )
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    fn rebuild_capital_admission_open_order_snapshot(
        &self,
        snapshot: BoltV3SubmitCapitalAdmissionOpenOrderSnapshot,
        now_ns: u64,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        self.rebuild_capital_admission_open_order_snapshot_with_projection(
            snapshot, now_ns, None, None, None,
        )
    }

    pub(crate) fn commit_capital_admission_nt_projection(
        &self,
        expected_epoch: u64,
        components: Option<BoltV3SubmitCapitalAdmissionNtComponents>,
        accepted_allowance_observed_at_ns: Option<u64>,
        snapshot: BoltV3SubmitCapitalAdmissionOpenOrderSnapshot,
        now_ns: u64,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        self.rebuild_capital_admission_open_order_snapshot_with_projection(
            snapshot,
            now_ns,
            Some(expected_epoch),
            components,
            accepted_allowance_observed_at_ns,
        )
    }

    fn rebuild_capital_admission_open_order_snapshot_with_projection(
        &self,
        snapshot: BoltV3SubmitCapitalAdmissionOpenOrderSnapshot,
        now_ns: u64,
        expected_epoch: Option<u64>,
        components: Option<BoltV3SubmitCapitalAdmissionNtComponents>,
        accepted_allowance_observed_at_ns: Option<u64>,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        let rebuilt_order_lifecycle = OrderLifecycleCapitalAdmissionSnapshot {
            source: snapshot.evidence_source.as_str().to_string(),
            observed_at_ns: snapshot.observed_at_ns,
            open_order_count: snapshot.observed_open_order_count,
            all_open_orders_attributed: snapshot.all_open_orders_attributed,
        };
        let mut audit_context = BoltV3SubmitCapitalAdmissionRebuildAuditContext {
            observed_at_ns: snapshot.observed_at_ns,
            source: snapshot.evidence_source,
            observed_open_order_count: snapshot.observed_open_order_count,
            all_open_orders_attributed: snapshot.all_open_orders_attributed,
            unresolved_sink_invoked_reservation_count: 0,
            unresolved_observed_open_reservation_count: 0,
        };
        let attempted_reservation_count = snapshot.observed_open_order_count;
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if expected_epoch
            .is_some_and(|expected_epoch| expected_epoch != inner.capital_admission_state_revision)
        {
            return Ok(BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: inner
                    .capital_admission
                    .as_ref()
                    .map(|capital_admission| {
                        capital_admission
                            .gate
                            .live_reserved_liability(&capital_admission.capital_pool.pool_id)
                    })
                    .unwrap_or(Decimal::ZERO),
                missing_nt_account_cache_balance: None,
            });
        }
        let Some(candidate_phase_revision) = inner.capital_admission_state_revision.checked_add(1)
        else {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            return Ok(BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::ReconciliationRequired),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: inner
                    .capital_admission
                    .as_ref()
                    .map(|capital_admission| {
                        capital_admission
                            .gate
                            .live_reserved_liability(&capital_admission.capital_pool.pool_id)
                    })
                    .unwrap_or(Decimal::ZERO),
                missing_nt_account_cache_balance: None,
            });
        };
        let Some(mut candidate_capital_admission) = inner.capital_admission.clone() else {
            return Ok(BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: snapshot.all_open_orders_attributed,
                reason: (!snapshot.all_open_orders_attributed)
                    .then_some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count: 0,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            });
        };
        let capital_admission = &mut candidate_capital_admission;
        if let Some(mut components) = components {
            if capital_admission
                .provider_collateral_allowance_capture_failure_observed_at_ns
                .is_some_and(|failure_observed_at_ns| {
                    accepted_allowance_observed_at_ns
                        .is_some_and(|accepted| accepted > failure_observed_at_ns)
                })
            {
                capital_admission.provider_collateral_allowance_capture_failure_source = None;
                capital_admission.provider_collateral_allowance_capture_failure_observed_at_ns =
                    None;
            }
            components.order_lifecycle.all_open_orders_attributed = false;
            refresh_capital_admission_state_from_components(capital_admission, components);
        }
        if capital_admission.fill_evidence_integrity_failed {
            capital_admission.gate.invalidate_reconciliation();
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                snapshot.observed_at_ns,
                snapshot.evidence_source.as_str().to_string(),
                false,
            );
            let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: capital_admission
                    .gate
                    .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                missing_nt_account_cache_balance: None,
            };
            return self.finish_capital_admission_rebuild(
                &mut inner,
                &audit_context,
                candidate_phase_revision,
                decision,
            );
        }
        if !snapshot.all_open_orders_attributed {
            if let Some(state) = capital_admission.state.as_mut() {
                apply_rebuild_order_lifecycle(state, rebuilt_order_lifecycle);
            }
            capital_admission.gate.invalidate_reconciliation();
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                snapshot.observed_at_ns,
                snapshot.evidence_source.as_str().to_string(),
                false,
            );
            let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: capital_admission
                    .gate
                    .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                missing_nt_account_cache_balance: None,
            };
            return self.finish_capital_admission_rebuild(
                &mut inner,
                &audit_context,
                candidate_phase_revision,
                decision,
            );
        }
        let Some(state) = capital_admission.state.as_ref() else {
            capital_admission.gate.invalidate_reconciliation();
            let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: capital_admission
                    .gate
                    .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                missing_nt_account_cache_balance: None,
            };
            return self.finish_capital_admission_rebuild(
                &mut inner,
                &audit_context,
                candidate_phase_revision,
                decision,
            );
        };
        capital_admission.capital_pool.source = state.portfolio.source.clone();
        capital_admission.capital_pool.observed_at_ns = state.portfolio.observed_at_ns;

        let retained_index = capital_admission.client_order_reservations.clone();
        let mut rebuilt_index = BTreeMap::new();
        let mut reservation_requests: Vec<CapitalAdmissionReservationEvidence> =
            Vec::with_capacity(snapshot.reservations.len() + retained_index.len());
        for (index, reservation) in snapshot.reservations.into_iter().enumerate() {
            if rebuilt_index.contains_key(&reservation.client_order_id) {
                capital_admission.gate.invalidate_reconciliation();
                let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                    accepted: false,
                    reason: Some(ReservationRejectionReason::DuplicateReservation),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: 0,
                    live_reserved_liability: capital_admission
                        .gate
                        .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                    missing_nt_account_cache_balance: None,
                };
                return self.finish_capital_admission_rebuild(
                    &mut inner,
                    &audit_context,
                    candidate_phase_revision,
                    decision,
                );
            }
            if !rebuilt_open_order_reservation_attribution_valid(&reservation) {
                capital_admission.gate.invalidate_reconciliation();
                let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                    accepted: false,
                    reason: Some(ReservationRejectionReason::MissingEvidence),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: 0,
                    live_reserved_liability: capital_admission
                        .gate
                        .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                    missing_nt_account_cache_balance: None,
                };
                return self.finish_capital_admission_rebuild(
                    &mut inner,
                    &audit_context,
                    candidate_phase_revision,
                    decision,
                );
            }
            if retained_index
                .get(&reservation.client_order_id)
                .is_some_and(|retained| {
                    retained.phase == BoltV3SubmitReservationPhase::Reserved
                        || retained.attribution != reservation.attribution
                })
            {
                capital_admission.gate.invalidate_reconciliation();
                let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                    accepted: false,
                    reason: Some(ReservationRejectionReason::MissingEvidence),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: 0,
                    live_reserved_liability: capital_admission
                        .gate
                        .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                    missing_nt_account_cache_balance: None,
                };
                return self.finish_capital_admission_rebuild(
                    &mut inner,
                    &audit_context,
                    candidate_phase_revision,
                    decision,
                );
            }

            let submit_reservation_id = reservation.submit_reservation_id;
            let collateral_group_id = reservation.collateral_group_id;
            let instrument_id = reservation.instrument_id;
            let side = reservation.side;
            let observed_at_ns = reservation.observed_at_ns;
            rebuilt_index.insert(
                reservation.client_order_id,
                BoltV3SubmitReservationRecord {
                    attribution: reservation.attribution,
                    phase: BoltV3SubmitReservationPhase::ObservedOpen,
                    phase_revision: candidate_phase_revision,
                    unresolved_in_projection: false,
                    fill_metadata: Some(BoltV3SubmitReservationFillMetadata {
                        instrument_id,
                        side,
                    }),
                },
            );
            reservation_requests.push(
                ReservationRequest {
                    request_id: submit_reservation_id,
                    pool_id: capital_admission.capital_pool.pool_id.clone(),
                    collateral_group_id,
                    liability: reservation.liability,
                    observed_at_ns,
                    evidence_label: reservation.evidence_label,
                }
                .into(),
            );
        }

        for (client_order_id, mut record) in retained_index {
            match (rebuilt_index.contains_key(&client_order_id), record.phase) {
                (true, _) => {}
                (
                    false,
                    BoltV3SubmitReservationPhase::SinkInvoked
                    | BoltV3SubmitReservationPhase::ObservedOpen,
                ) => {
                    let retained = capital_admission.gate.retained_reservation(
                        &record.attribution.capital_pool_id,
                        &record.attribution.collateral_group_id,
                        &record.attribution.submit_reservation_id,
                    );
                    let Ok(retained) = retained else {
                        capital_admission.gate.invalidate_reconciliation();
                        let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                            accepted: false,
                            reason: Some(ReservationRejectionReason::MissingEvidence),
                            attempted_reservation_count,
                            rebuilt_reservation_count: 0,
                            live_reserved_liability: capital_admission
                                .gate
                                .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                            missing_nt_account_cache_balance: None,
                        };
                        return self.finish_capital_admission_rebuild(
                            &mut inner,
                            &audit_context,
                            candidate_phase_revision,
                            decision,
                        );
                    };
                    reservation_requests.push(
                        CapitalAdmissionReservationEvidence::RetainedLifecycleReservation(retained),
                    );
                    record.unresolved_in_projection = true;
                    rebuilt_index.insert(client_order_id, record);
                }
                (false, BoltV3SubmitReservationPhase::Reserved) => {
                    capital_admission.gate.invalidate_reconciliation();
                    let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
                        accepted: false,
                        reason: Some(ReservationRejectionReason::MissingEvidence),
                        attempted_reservation_count,
                        rebuilt_reservation_count: 0,
                        live_reserved_liability: capital_admission
                            .gate
                            .live_reserved_liability(&capital_admission.capital_pool.pool_id),
                        missing_nt_account_cache_balance: None,
                    };
                    return self.finish_capital_admission_rebuild(
                        &mut inner,
                        &audit_context,
                        candidate_phase_revision,
                        decision,
                    );
                }
            }
        }

        let unresolved = unresolved_reservation_counts(&rebuilt_index);
        audit_context.unresolved_sink_invoked_reservation_count = unresolved.0;
        audit_context.unresolved_observed_open_reservation_count = unresolved.1;
        capital_admission.unresolved_lifecycle_reservations = unresolved;

        let mut candidate_gate = capital_admission.gate.clone();
        let gate_decision = candidate_gate.rebuild_open_order_reservations(
            &capital_admission.capital_pool,
            &reservation_requests,
            now_ns,
            capital_admission.policy.min_remaining_pool_balance,
        );
        let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: gate_decision.accepted,
            reason: gate_decision.reason,
            attempted_reservation_count: gate_decision.attempted_reservation_count,
            rebuilt_reservation_count: gate_decision.rebuilt_reservation_count,
            live_reserved_liability: gate_decision.live_reserved_liability,
            missing_nt_account_cache_balance: None,
        };
        match decision.accepted {
            false => {
                capital_admission.gate.invalidate_reconciliation();
                self.finish_capital_admission_rebuild(
                    &mut inner,
                    &audit_context,
                    candidate_phase_revision,
                    decision,
                )
            }
            true => match self.record_capital_admission_rebuild(&audit_context, &decision) {
                Ok(false) => {
                    let live_reserved_liability =
                        inner
                            .capital_admission
                            .as_mut()
                            .map_or(Decimal::ZERO, |live| {
                                live.gate.invalidate_reconciliation();
                                refresh_capital_admission_reservation_snapshot_with_source(
                                    live,
                                    audit_context.observed_at_ns,
                                    audit_context.source.as_str().to_string(),
                                    false,
                                );
                                live.gate
                                    .live_reserved_liability(&live.capital_pool.pool_id)
                            });
                    inner.capital_admission_state_revision = candidate_phase_revision;
                    Ok(BoltV3SubmitCapitalAdmissionRebuildDecision {
                        accepted: false,
                        reason: Some(ReservationRejectionReason::MissingEvidence),
                        attempted_reservation_count: decision.attempted_reservation_count,
                        rebuilt_reservation_count: 0,
                        live_reserved_liability,
                        missing_nt_account_cache_balance: None,
                    })
                }
                Err(error) => {
                    if let Some(live) = inner.capital_admission.as_mut() {
                        live.gate.invalidate_reconciliation();
                        refresh_capital_admission_reservation_snapshot_with_source(
                            live,
                            audit_context.observed_at_ns,
                            audit_context.source.as_str().to_string(),
                            false,
                        );
                    }
                    inner.capital_admission_state_revision = candidate_phase_revision;
                    Err(error)
                }
                Ok(true) => {
                    let prior_unresolved = inner
                        .capital_admission
                        .as_ref()
                        .map(|state| state.unresolved_lifecycle_reservations)
                        .unwrap_or((0, 0));
                    if prior_unresolved != unresolved {
                        log::warn!(
                            "bolt-v3 capital admission unresolved lifecycle reservations changed: sink_invoked={} observed_open={}",
                            unresolved.0,
                            unresolved.1,
                        );
                    }
                    capital_admission.gate = candidate_gate;
                    capital_admission.client_order_reservations = rebuilt_index;
                    if let Some(state) = capital_admission.state.as_mut() {
                        apply_rebuild_order_lifecycle(state, rebuilt_order_lifecycle);
                    }
                    refresh_capital_admission_reservation_snapshot(capital_admission, now_ns);
                    inner.capital_admission = Some(candidate_capital_admission);
                    inner.capital_admission_state_revision = candidate_phase_revision;
                    Ok(decision)
                }
            },
        }
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn rebuild_capital_admission_open_order_snapshot_for_test(
        &self,
        snapshot: BoltV3SubmitCapitalAdmissionOpenOrderSnapshot,
        now_ns: u64,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        self.rebuild_capital_admission_open_order_snapshot(snapshot, now_ns)
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    #[doc(hidden)]
    pub fn capital_admission_state_revision_for_test(&self) -> u64 {
        self.capital_admission_state_revision()
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    #[doc(hidden)]
    pub fn set_capital_admission_state_revision_for_test(&self, revision: u64) {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .capital_admission_state_revision = revision;
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn capital_admission_reservation_lifecycle_for_test(
        &self,
        client_order_id: &str,
    ) -> Option<BoltV3SubmitReservationLifecycleSnapshot> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let record = inner
            .capital_admission
            .as_ref()?
            .client_order_reservations
            .get(client_order_id)?;
        Some(BoltV3SubmitReservationLifecycleSnapshot {
            phase: record.phase,
            phase_revision: record.phase_revision,
        })
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    #[doc(hidden)]
    pub fn commit_capital_admission_nt_projection_for_test(
        &self,
        expected_epoch: u64,
        components: Option<BoltV3SubmitCapitalAdmissionNtComponents>,
        accepted_allowance_observed_at_ns: Option<u64>,
        snapshot: BoltV3SubmitCapitalAdmissionOpenOrderSnapshot,
        now_ns: u64,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        self.commit_capital_admission_nt_projection(
            expected_epoch,
            components,
            accepted_allowance_observed_at_ns,
            snapshot,
            now_ns,
        )
    }

    fn finish_capital_admission_rebuild(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        context: &BoltV3SubmitCapitalAdmissionRebuildAuditContext,
        failure_revision: u64,
        decision: BoltV3SubmitCapitalAdmissionRebuildDecision,
    ) -> Result<BoltV3SubmitCapitalAdmissionRebuildDecision, BoltV3SubmitAdmissionError> {
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            capital_admission.gate.invalidate_reconciliation();
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                context.observed_at_ns,
                context.source.as_str().to_string(),
                false,
            );
        }
        inner.capital_admission_state_revision = failure_revision;
        if self.record_capital_admission_rebuild(context, &decision)? {
            return Ok(decision);
        }

        Ok(BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: decision.attempted_reservation_count,
            rebuilt_reservation_count: 0,
            live_reserved_liability: inner
                .capital_admission
                .as_ref()
                .map(|capital_admission| {
                    capital_admission
                        .gate
                        .live_reserved_liability(&capital_admission.capital_pool.pool_id)
                })
                .unwrap_or(Decimal::ZERO),
            missing_nt_account_cache_balance: None,
        })
    }

    fn record_capital_admission_rebuild(
        &self,
        context: &BoltV3SubmitCapitalAdmissionRebuildAuditContext,
        decision: &BoltV3SubmitCapitalAdmissionRebuildDecision,
    ) -> Result<bool, BoltV3SubmitAdmissionError> {
        let outcome = capital_admission_rebuild_outcome(decision)?;
        let audit = CapitalAdmissionRebuildFact {
            observed_at_ns: context.observed_at_ns,
            source: context.source,
            observed_open_order_count: context.observed_open_order_count,
            all_open_orders_attributed: context.all_open_orders_attributed,
            outcome,
            attempted_reservation_count: decision.attempted_reservation_count,
            recovered_reservation_count: decision.rebuilt_reservation_count,
            live_reserved_liability: decision.live_reserved_liability.to_string(),
            unresolved_sink_invoked_reservation_count: context
                .unresolved_sink_invoked_reservation_count,
            unresolved_observed_open_reservation_count: context
                .unresolved_observed_open_reservation_count,
        };
        Ok(self
            .decision_evidence
            .record_capital_admission_rebuild(audit)
            .is_ok())
    }

    pub fn record_capital_admission_fill_evidence(
        &self,
        update: BoltV3SubmitCapitalAdmissionFillUpdate,
    ) -> BoltV3SubmitReservationFillEvidenceDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let known_non_reservation_order = inner
            .evidence_free_non_reservation_admissions
            .get(&update.client_order_id)
            == Some(&BoltV3EvidenceFreeNonReservationPhase::SinkInvoked)
            || inner
                .committed_admission_authority
                .authorizes_non_reservation_order(&update.client_order_id);
        let BoltV3SubmitAdmissionInner {
            committed_admission_authority,
            capital_admission,
            capital_admission_state_revision,
            ..
        } = &mut *inner;
        let Some(capital_admission) = capital_admission.as_mut() else {
            return BoltV3SubmitReservationFillEvidenceDecision::unknown();
        };
        let live_index = capital_admission
            .client_order_reservations
            .get(&update.client_order_id)
            .cloned();
        if live_index.is_none() && known_non_reservation_order {
            return BoltV3SubmitReservationFillEvidenceDecision {
                accepted: true,
                unknown_reservation: false,
            };
        }
        let expected = live_index
            .as_ref()
            .and_then(|index| {
                index.fill_metadata.as_ref().map(|metadata| {
                    (
                        index.attribution.submit_reservation_id.clone(),
                        metadata.instrument_id.clone(),
                        metadata.side,
                    )
                })
            })
            .or_else(|| {
                committed_admission_authority
                    .reservation_attribution(&update.client_order_id)
                    .and_then(|attribution| {
                        let side = match attribution.side {
                            EvidenceOrderSide::Buy => BoltV3CompiledOrderSide::Buy,
                            EvidenceOrderSide::Sell => BoltV3CompiledOrderSide::Sell,
                            EvidenceOrderSide::Unspecified => return None,
                        };
                        Some((
                            attribution.submit_reservation_id.clone(),
                            attribution.instrument_id.clone(),
                            side,
                        ))
                    })
            });
        let Some((submit_reservation_id, instrument_id, side)) = expected else {
            return reject_capital_admission_fill_evidence_integrity(
                capital_admission,
                capital_admission_state_revision,
                update.observed_at_ns,
            );
        };
        if update.trade_id.trim().len() != update.trade_id.len()
            || update.trade_id.is_empty()
            || update.fill_quantity <= Decimal::ZERO
            || update.instrument_id != instrument_id
            || update.side != side
        {
            return reject_capital_admission_fill_evidence_integrity(
                capital_admission,
                capital_admission_state_revision,
                update.observed_at_ns,
            );
        }
        if let Some(existing) = committed_admission_authority.reservation_fill(
            &update.client_order_id,
            &submit_reservation_id,
            &update.trade_id,
        ) {
            if reservation_fill_update_matches(existing, &update) {
                return BoltV3SubmitReservationFillEvidenceDecision {
                    accepted: true,
                    unknown_reservation: false,
                };
            }
            return reject_capital_admission_fill_evidence_integrity(
                capital_admission,
                capital_admission_state_revision,
                update.observed_at_ns,
            );
        }
        let Some(index) = live_index else {
            log::warn!(
                "bolt-v3 submit admission received new fill for non-live client_order_id={}",
                update.client_order_id
            );
            return reject_capital_admission_fill_evidence_integrity(
                capital_admission,
                capital_admission_state_revision,
                update.observed_at_ns,
            );
        };
        if index.fill_metadata.is_none() {
            return reject_capital_admission_fill_evidence_integrity(
                capital_admission,
                capital_admission_state_revision,
                update.observed_at_ns,
            );
        }
        match advance_capital_admission_state_revision(capital_admission_state_revision) {
            Ok(()) => {}
            Err(_) => {
                capital_admission.gate.invalidate_reconciliation();
                log::error!(
                    "capital admission state revision exhausted while recording fill evidence"
                );
                return BoltV3SubmitReservationFillEvidenceDecision::unknown();
            }
        }
        let fill_evidence = SubmitReservationFillFact {
            client_order_id: update.client_order_id.clone(),
            submit_reservation_id: index.attribution.submit_reservation_id.clone(),
            trade_id: update.trade_id.clone(),
            instrument_id: update.instrument_id.clone(),
            side: match update.side {
                BoltV3CompiledOrderSide::Buy => EvidenceOrderSide::Buy,
                BoltV3CompiledOrderSide::Sell => EvidenceOrderSide::Sell,
            },
            fill_quantity: update.fill_quantity.to_string(),
            observed_at_ns: update.observed_at_ns,
            reconciliation: update.reconciliation,
            source: update.evidence_source,
        };
        let recovery_fill = fill_evidence.clone();
        if self
            .decision_evidence
            .record_submit_reservation_fill(fill_evidence)
            .is_err()
        {
            fail_capital_admission_fill_evidence_integrity(
                capital_admission,
                update.observed_at_ns,
            );
            return BoltV3SubmitReservationFillEvidenceDecision {
                accepted: false,
                unknown_reservation: false,
            };
        }
        if let Err(error) =
            committed_admission_authority.apply(ReservationRecoveryEvent::Fill(recovery_fill))
        {
            log::error!(
                "durable submit-reservation fill could not update the committed evidence index: {error:#}"
            );
            fail_capital_admission_fill_evidence_integrity(
                capital_admission,
                update.observed_at_ns,
            );
            return BoltV3SubmitReservationFillEvidenceDecision {
                accepted: false,
                unknown_reservation: false,
            };
        }
        BoltV3SubmitReservationFillEvidenceDecision {
            accepted: true,
            unknown_reservation: false,
        }
    }

    pub(crate) fn reject_capital_admission_fill_evidence(&self, observed_at_ns: u64) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let BoltV3SubmitAdmissionInner {
            capital_admission,
            capital_admission_state_revision,
            ..
        } = &mut *inner;
        if let Some(capital_admission) = capital_admission.as_mut() {
            let _ = reject_capital_admission_fill_evidence_integrity(
                capital_admission,
                capital_admission_state_revision,
                observed_at_ns,
            );
        }
    }

    pub fn replace_kill_switch_state(&self, state: KillSwitchState) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.kill_switch_state = state;
    }

    pub fn kill_switch_state_kind(&self) -> KillSwitchStateKind {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .kill_switch_state
            .kind()
    }

    pub fn kill_switch_state(&self) -> KillSwitchState {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .kill_switch_state
            .clone()
    }

    /// Test-only entrypoint for exercising admission policy independently of
    /// the production economics seal.
    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_at_inner(request, None, current_unix_ns()?)
    }

    pub(crate) fn admit_with_economics_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        economics: &EconomicsAdmission,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_at_inner(request, Some(economics), now_ns)
    }

    /// Test-only event-time entrypoint for admission policy tests that do not
    /// exercise the production economics seal.
    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn admit_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_at_inner(request, None, now_ns)
    }

    fn admit_at_inner(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        economics: Option<&EconomicsAdmission>,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        ensure_economics_available(economics, now_ns)?;
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let mut evaluation = self.evaluate(&mut inner, request, now_ns)?;
        if let Err(error) = evaluation.validate() {
            if let Some(rollback) = evaluation.rollback.as_ref() {
                rollback_capital_admission_reservation(&mut inner, rollback);
            }
            return Err(error);
        }
        let mut admitted_counter_update = None;
        if evaluation.outcome == AdmissionDecisionOutcome::Admitted {
            let current_execution_client_count = inner
                .admitted_order_count_by_execution_client
                .get(&request.execution_client_id)
                .copied()
                .unwrap_or(0);
            let counter_values = match checked_admission_counter_values(
                inner.admitted_order_count,
                current_execution_client_count,
                1,
            ) {
                Ok(values) => values,
                Err(error) => {
                    if let Some(rollback) = evaluation.rollback.as_ref() {
                        rollback_capital_admission_reservation(&mut inner, rollback);
                    }
                    let loss_snapshot_diagnostics = evaluation.loss_snapshot_diagnostics.take();
                    evaluation = BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        AdmissionDecisionOutcome::Rejected(
                            AdmissionRejectionReason::CountCapExhausted,
                        ),
                        evaluation.admission_now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
                    if let Err(err) = self.record_admission_decision(
                        &mut inner,
                        request,
                        &evaluation,
                        economics,
                        now_ns,
                    ) {
                        return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                            reason: format!("{err:#}"),
                        });
                    }
                    return Err(error);
                }
            };
            let counter_rollback = BoltV3SubmitAdmissionCounterRollback {
                execution_client_id: request.execution_client_id.clone(),
                order_count: 1,
                uncommitted_evidence_free_client_order_ids: Vec::new(),
            };
            admitted_counter_update = Some((
                counter_values.admitted_order_count,
                counter_values.execution_client_order_count,
                counter_rollback,
            ));
        }
        let evidence_authority = match self.record_admission_decision(
            &mut inner,
            request,
            &evaluation,
            economics,
            now_ns,
        ) {
            Ok(authority) => authority,
            Err(err) => {
                if let Some(rollback) = evaluation.rollback.as_ref() {
                    rollback_capital_admission_reservation(&mut inner, rollback);
                }
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
        };
        match evaluation.outcome {
            AdmissionDecisionOutcome::Admitted => {
                let Some((
                    next_admitted_order_count,
                    next_execution_client_count,
                    mut counter_rollback,
                )) = admitted_counter_update
                else {
                    return Err(BoltV3SubmitAdmissionError::InvariantViolation {
                        invariant: BoltV3SubmitAdmissionInvariant::MissingAdmittedCounterUpdate,
                    });
                };
                let requires_evidence_free_non_reservation_tracking =
                    evidence_authority.requires_evidence_free_non_reservation_tracking();
                let submit_evidence_authority = match evidence_authority.into_submit_authority() {
                    Ok(authority) => authority,
                    Err(error) => {
                        if let Some(rollback) = evaluation.rollback.as_ref() {
                            rollback_capital_admission_reservation(&mut inner, rollback);
                        }
                        return Err(error);
                    }
                };
                inner.admitted_order_count = next_admitted_order_count;
                inner.admitted_order_count_by_execution_client.insert(
                    request.execution_client_id.clone(),
                    next_execution_client_count,
                );
                if evaluation.reservation_attribution.is_none()
                    && requires_evidence_free_non_reservation_tracking
                {
                    inner.evidence_free_non_reservation_admissions.insert(
                        request.client_order_id.clone(),
                        BoltV3EvidenceFreeNonReservationPhase::PermitHeld,
                    );
                    counter_rollback
                        .uncommitted_evidence_free_client_order_ids
                        .push(request.client_order_id.clone());
                }
                Ok(BoltV3SubmitAdmissionPermit {
                    inner: self.inner.clone(),
                    rollbacks: evaluation.rollback.into_iter().collect(),
                    counter_rollback: Some(counter_rollback),
                    evidence_authority: Some(submit_evidence_authority),
                    committed: false,
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::KillSwitchLatched) => {
                Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
                    state: inner.kill_switch_state.kind(),
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted) => {
                Err(BoltV3SubmitAdmissionError::LossGovernorHalted {
                    reasons: evaluation.loss_halt_reasons,
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NonPositiveNotional) => {
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NotionalCapExceeded) => {
                Err(BoltV3SubmitAdmissionError::NotionalCapExceeded)
            }
            AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::ClientOrderAlreadyAuthorized,
            ) => Err(BoltV3SubmitAdmissionError::ClientOrderAlreadyAuthorized),
            AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::InvalidRiskReducingExitProof,
            ) => Err(BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof),
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted) => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission) => Err(
                capital_admission_rejection_error(evaluation.capital_admission_rejection),
            ),
            AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid
                | AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded,
            ) => Err(retired_forced_reduction_outcome_error()),
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::EconomicsSealRejected) => {
                Err(BoltV3SubmitAdmissionError::EconomicsSealRejected)
            }
        }
    }

    pub(crate) fn evaluate_and_record_without_consuming_capacity_with_economics_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        economics: &EconomicsAdmission,
        now_ns: u64,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        self.evaluate_and_record_without_consuming_capacity_at(request, Some(economics), now_ns)
    }

    fn evaluate_and_record_without_consuming_capacity_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        economics: Option<&EconomicsAdmission>,
        now_ns: u64,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        ensure_economics_available(economics, now_ns)?;
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let evaluation = self.evaluate(&mut inner, request, now_ns)?;
        if let Err(error) = evaluation.validate() {
            if let Some(rollback) = evaluation.rollback.as_ref() {
                rollback_capital_admission_reservation(&mut inner, rollback);
            }
            return Err(error);
        }
        let record_result =
            self.record_admission_decision(&mut inner, request, &evaluation, economics, now_ns);
        if let Some(rollback) = evaluation.rollback.as_ref() {
            rollback_capital_admission_reservation(&mut inner, rollback);
        }
        if let Err(err) = record_result {
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }
        Self::admission_result(&inner, &evaluation)
    }

    fn record_admission_decision(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
        evaluation: &BoltV3SubmitAdmissionEvaluation,
        economics: Option<&EconomicsAdmission>,
        now_ns: u64,
    ) -> Result<RecordedAdmissionAuthority, anyhow::Error> {
        evaluation.validate().map_err(anyhow::Error::from)?;
        let economics_details = economics.map(admission_economics_details).transpose()?;
        let details = AdmissionDetails {
            strategy_id: request.strategy_id.clone(),
            execution_client_id: request.execution_client_id.clone(),
            client_order_id: request.client_order_id.clone(),
            instrument_id: request.instrument_id.clone(),
            notional: request.notional.to_string(),
            loss_halt_reasons: evaluation
                .loss_halt_reasons
                .iter()
                .copied()
                .map(loss_halt_reason_to_evidence)
                .collect(),
            snapshot_present: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| diagnostics.snapshot_present),
            snapshot_observed_at_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.snapshot_observed_at_ns),
            admission_now_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .map_or(evaluation.admission_now_ns, |diagnostics| {
                    diagnostics.admission_now_ns
                }),
            snapshot_age_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.snapshot_age_ns),
            max_snapshot_age_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.max_snapshot_age_ns),
            snapshot_source: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.snapshot_source),
            per_trade_pnl_present: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| diagnostics.per_trade_pnl_present),
            daily_pnl_present: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| diagnostics.daily_pnl_present),
            rolling_pnl_present: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| diagnostics.rolling_pnl_present),
            current_equity_present: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| diagnostics.current_equity_present),
            peak_equity_present: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .is_some_and(|diagnostics| diagnostics.peak_equity_present),
            last_account_state_observed_at_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.last_account_state_observed_at_ns),
            last_portfolio_snapshot_observed_at_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.last_portfolio_snapshot_observed_at_ns),
            last_position_event_observed_at_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.last_position_event_observed_at_ns),
            stale_reason: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.stale_reason)
                .map(loss_snapshot_stale_reason_to_evidence),
            loss_snapshot_observed_at_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .and_then(|diagnostics| diagnostics.snapshot_observed_at_ns),
            loss_eval_now_ns: evaluation
                .loss_snapshot_diagnostics
                .as_ref()
                .map(|diagnostics| diagnostics.admission_now_ns),
            economics: economics_details,
        };
        let result = match (request.intent_kind, evaluation.outcome) {
            (BoltV3SubmitIntentKind::Entry, AdmissionDecisionOutcome::Admitted) => {
                let fact = AdmittedEntryAdmissionFact {
                    details,
                    reservation: evaluation.reservation_attribution.clone(),
                };
                let mut next_authority = inner.committed_admission_authority.clone();
                next_authority.apply(ReservationRecoveryEvent::AdmittedEntry(Box::new(
                    fact.clone(),
                )))?;
                let committed = self
                    .decision_evidence
                    .record_admitted_entry_admission(fact)
                    .map_err(anyhow::Error::from)?;
                inner.committed_admission_authority = next_authority;
                Ok(RecordedAdmissionAuthority::CommittedEntry(committed))
            }
            (BoltV3SubmitIntentKind::Entry, AdmissionDecisionOutcome::Rejected(reason)) => {
                if let NonBlockingRecordOutcome::Failed(error) = self
                    .decision_evidence
                    .record_rejected_entry_admission(RejectedEntryAdmissionFact { details, reason })
                {
                    log::error!("rejected entry admission evidence failed: {error}");
                }
                Ok(RecordedAdmissionAuthority::NoExternalActionAuthority)
            }
            (BoltV3SubmitIntentKind::RiskReducingExit, outcome) => {
                let fact = RiskReducingExitAdmissionFact { details, outcome };
                let next_authority = if outcome == AdmissionDecisionOutcome::Admitted {
                    let mut next_authority = inner.committed_admission_authority.clone();
                    next_authority.apply(ReservationRecoveryEvent::RiskReducingExit(Box::new(
                        fact.clone(),
                    )))?;
                    Some(next_authority)
                } else {
                    None
                };
                let evidence_committed = match self
                    .decision_evidence
                    .record_risk_reducing_exit_admission(fact.clone())
                {
                    NonBlockingRecordOutcome::Appended(_) => true,
                    NonBlockingRecordOutcome::Failed(error) => {
                        log::error!("risk-reducing exit admission evidence failed: {error}");
                        false
                    }
                };
                if evidence_committed && let Some(next_authority) = next_authority {
                    inner.committed_admission_authority = next_authority;
                }
                Ok(RecordedAdmissionAuthority::RiskReducing { evidence_committed })
            }
        };
        if evaluation.outcome == AdmissionDecisionOutcome::Admitted {
            self.reject_episodes
                .lock()
                .expect("submit admission reject episodes mutex should not be poisoned")
                .clear();
        } else if evaluation.outcome
            != AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted)
        {
            // Loss-governor halts are MECE with order-level rejects; RC5 records loss halts.
            self.record_submit_admission_order_reject(request, evaluation, now_ns);
        }
        result
    }

    fn record_submit_admission_order_reject(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        evaluation: &BoltV3SubmitAdmissionEvaluation,
        now_ns: u64,
    ) {
        let side_key = submit_admission_order_side_key(request.order_side);
        let stable_episode_key = format!(
            "{}/{}/{}",
            request.instrument_id,
            side_key,
            admission_outcome_key(&evaluation.outcome)
        );
        let (prior_client_order_id, retry_count, elapsed_ns, should_emit) = {
            let mut reject_episodes = self
                .reject_episodes
                .lock()
                .expect("submit admission reject episodes mutex should not be poisoned");
            let episode = if let Some(episode) = reject_episodes.get_mut(&stable_episode_key) {
                episode
            } else {
                reject_episodes.insert(
                    stable_episode_key.clone(),
                    RejectEpisode {
                        count: 0,
                        first_ns: now_ns,
                        last_client_order_id: String::new(),
                    },
                );
                reject_episodes
                    .get_mut(&stable_episode_key)
                    .expect("reject episode should exist after insertion")
            };
            let prior_client_order_id = if episode.count > 0 {
                Some(episode.last_client_order_id.clone())
            } else {
                None
            };
            episode.count = episode.count.saturating_add(1);
            episode.last_client_order_id = request.client_order_id.clone();
            let elapsed_ns = now_ns.saturating_sub(episode.first_ns);
            let retry_count = episode.count;
            let should_emit = episode.count.is_power_of_two();
            // Bound the map while the lock is held: the get_mut-first insert/update
            // above is the only growth point, and admits clear() the whole map, so a
            // sustained-reject regime (no admit ever fires) is the only path that
            // grows it. Eviction drops only the oldest evidence-sampling episode and
            // never touches a trading decision. Re-uses the shared helper so the
            // bound lives in exactly one place.
            evict_oldest_episodes_over_cap(
                &mut reject_episodes,
                self.decision_evidence.reject_episode_max_count(),
            );
            (prior_client_order_id, retry_count, elapsed_ns, should_emit)
        };
        if !should_emit {
            return;
        }

        let evidence = OrderRejectFact {
            reject_source: OrderRejectSource::SubmitAdmission,
            reject_reason: OrderRejectReason::AdmissionRejected,
            admission_outcome: Some(evaluation.outcome),
            raw_reason_text: None,
            instrument_id: request.instrument_id.clone(),
            order_side: Some(match request.order_side {
                OrderSide::NoOrderSide => EvidenceOrderSide::Unspecified,
                OrderSide::Buy => EvidenceOrderSide::Buy,
                OrderSide::Sell => EvidenceOrderSide::Sell,
            }),
            raw_price: None,
            raw_quantity: Some(request.order_quantity.to_string()),
            raw_maker_amount: None,
            raw_taker_amount: None,
            normalized_price: None,
            normalized_quantity: None,
            normalized_maker_amount: None,
            normalized_taker_amount: None,
            venue_price_precision: None,
            venue_size_precision: None,
            venue_min_notional: None,
            prior_client_order_id,
            client_order_id: request.client_order_id.clone(),
            retry_count,
            stable_episode_key,
            elapsed_ns,
        };
        if let NonBlockingRecordOutcome::Failed(err) = self
            .decision_evidence
            .record_submit_admission_order_reject(evidence.clone())
        {
            log::error!(
                "bolt-v3 order reject evidence write failed: stable_episode_key={} admission_outcome={:?} prior_client_order_id={:?}: {err:#}",
                evidence.stable_episode_key,
                evidence.admission_outcome,
                evidence.prior_client_order_id
            );
        }
    }

    fn record_stale_loss_governor_halt(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        loss_policy: &LossGovernorPolicy,
        now_ns: u64,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        let snapshot = inner.loss_snapshot.as_ref();
        let stale_reason = stale_loss_governor_halt_reason(loss_policy, snapshot, now_ns)?;
        let source_for_key = snapshot
            .and_then(|snapshot| snapshot.source)
            .map(LossSnapshotSource::as_str)
            .unwrap_or("none");
        let stable_halt_key = format!("{}:{}", stale_loss_reason_key(stale_reason), source_for_key);

        let (retry_count, elapsed_since_first_halt_ns) =
            if let Some(episode) = inner.loss_halt_episodes.get_mut(&stable_halt_key) {
                episode.count = episode.count.saturating_add(1);
                (episode.count, now_ns.saturating_sub(episode.first_halt_ns))
            } else {
                inner.loss_halt_episodes.insert(
                    stable_halt_key.clone(),
                    LossHaltEpisode {
                        count: 1,
                        first_halt_ns: now_ns,
                    },
                );
                (1, 0)
            };
        if !retry_count.is_power_of_two() {
            return Ok(());
        }

        let freshness = inner.loss_freshness;
        let evidence = LossGovernorHaltFact {
            snapshot_present: snapshot.is_some(),
            snapshot_observed_at_ns: snapshot.map(|snapshot| snapshot.observed_at_ns),
            admission_now_ns: now_ns,
            snapshot_age_ns: snapshot.and_then(|snapshot| {
                if snapshot.observed_at_ns <= now_ns {
                    Some(now_ns - snapshot.observed_at_ns)
                } else {
                    None
                }
            }),
            max_snapshot_age_ns: loss_policy.max_snapshot_age_ns,
            snapshot_source: snapshot.and_then(|snapshot| snapshot.source),
            has_per_trade_pnl: snapshot.is_some_and(|snapshot| snapshot.per_trade_pnl.is_some()),
            has_daily_pnl: snapshot.is_some_and(|snapshot| snapshot.daily_pnl.is_some()),
            has_rolling_pnl: snapshot.is_some_and(|snapshot| snapshot.rolling_pnl.is_some()),
            has_current_equity: snapshot.is_some_and(|snapshot| snapshot.current_equity.is_some()),
            has_peak_equity: snapshot.is_some_and(|snapshot| snapshot.peak_equity.is_some()),
            last_account_state_ts_ns: freshness.last_account_state_ts_ns,
            last_portfolio_snapshot_ts_ns: freshness.last_portfolio_snapshot_ts_ns,
            last_position_event_ts_ns: freshness.last_position_event_ts_ns,
            account_state_count: freshness.account_state_count,
            portfolio_snapshot_count: freshness.portfolio_snapshot_count,
            position_event_count: freshness.position_event_count,
            stale_reason,
            stable_halt_key,
            retry_count,
            elapsed_since_first_halt_ns,
        };

        if let NonBlockingRecordOutcome::Failed(err) = self
            .decision_evidence
            .record_loss_governor_halt(evidence.clone())
        {
            log::error!(
                "bolt-v3 loss governor halt evidence write failed: stable_halt_key={} stale_reason={:?}: {err:#}",
                evidence.stable_halt_key,
                evidence.stale_reason
            );
        }
        Ok(())
    }

    fn admission_result(
        inner: &BoltV3SubmitAdmissionInner,
        evaluation: &BoltV3SubmitAdmissionEvaluation,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        evaluation.validate()?;
        match evaluation.outcome {
            AdmissionDecisionOutcome::Admitted => Ok(()),
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::KillSwitchLatched) => {
                Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
                    state: inner.kill_switch_state.kind(),
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted) => {
                Err(BoltV3SubmitAdmissionError::LossGovernorHalted {
                    reasons: evaluation.loss_halt_reasons.clone(),
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NonPositiveNotional) => {
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NotionalCapExceeded) => {
                Err(BoltV3SubmitAdmissionError::NotionalCapExceeded)
            }
            AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::ClientOrderAlreadyAuthorized,
            ) => Err(BoltV3SubmitAdmissionError::ClientOrderAlreadyAuthorized),
            AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::InvalidRiskReducingExitProof,
            ) => Err(BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof),
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted) => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission) => Err(
                capital_admission_rejection_error(evaluation.capital_admission_rejection),
            ),
            AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid
                | AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded,
            ) => Err(retired_forced_reduction_outcome_error()),
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::EconomicsSealRejected) => {
                Err(BoltV3SubmitAdmissionError::EconomicsSealRejected)
            }
        }
    }

    pub(crate) fn reserve_basket_submit_slots_with_evidence<F>(
        &self,
        execution_client_id: &str,
        claims: &[BoltV3BasketSubmitSlotClaim],
        details: &BasketAdmissionDetails,
        record_grant: F,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError>
    where
        F: FnOnce(BasketAdmissionGrantedFact) -> Result<CommittedAdmission, RecordFailure>,
    {
        let now_ns = current_unix_ns()?;
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");

        let claim_count = checked_basket_claim_count(claims.len())?;

        let mut outcome = if claims.is_empty() {
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NonPositiveNotional)
        } else {
            AdmissionDecisionOutcome::Admitted
        };
        let mut rejected_evaluation: Option<BoltV3SubmitAdmissionEvaluation> = None;
        let mut rollbacks = Vec::new();
        let mut reservation_attributions = Vec::with_capacity(claims.len());
        let mut admitted_intent_kinds = Vec::with_capacity(claims.len());

        for claim in claims {
            let admitted_intent_kind = match claim.intent_kind {
                BoltV3SubmitIntentKind::Entry => BasketAdmissionIntentKind::Entry,
                BoltV3SubmitIntentKind::RiskReducingExit => {
                    BasketAdmissionIntentKind::RiskReducingExit
                }
            };
            let request = basket_submit_request(&details.strategy_id, execution_client_id, claim);
            let evaluation = match self.evaluate(&mut inner, &request, now_ns) {
                Ok(evaluation) => evaluation,
                Err(error) => {
                    rollback_capital_admission_reservations(&mut inner, &rollbacks);
                    return Err(error);
                }
            };
            if let Err(error) = evaluation.validate() {
                let mut invalid_rollbacks = rollbacks.clone();
                if let Some(rollback) = evaluation.rollback.as_ref() {
                    invalid_rollbacks.push(rollback.clone());
                }
                rollback_capital_admission_reservations(&mut inner, &invalid_rollbacks);
                return Err(error);
            }
            outcome = evaluation.outcome;
            if outcome != AdmissionDecisionOutcome::Admitted {
                rejected_evaluation = Some(evaluation);
                break;
            }
            if let Some(rollback) = evaluation.rollback {
                rollbacks.push(rollback);
            }
            reservation_attributions.push(evaluation.reservation_attribution);
            admitted_intent_kinds.push(admitted_intent_kind);
        }

        if outcome == AdmissionDecisionOutcome::Admitted
            && let Some(limits) = inner.live_submit_approval_limits.get(execution_client_id)
        {
            let current_count = inner
                .admitted_order_count_by_execution_client
                .get(execution_client_id)
                .copied()
                .unwrap_or(0);
            outcome =
                live_submit_count_cap_outcome(current_count, claim_count, limits.max_order_count);
        }

        if outcome != AdmissionDecisionOutcome::Admitted {
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            if let Some(evaluation) = rejected_evaluation.as_ref() {
                Self::admission_result(&inner, evaluation)?;
            }
            return Err(submit_admission_error_from_outcome(
                outcome,
                inner.kill_switch_state.kind(),
            ));
        }

        let current_client_count = inner
            .admitted_order_count_by_execution_client
            .get(execution_client_id)
            .copied()
            .unwrap_or(0);
        let counter_values = match checked_admission_counter_values(
            inner.admitted_order_count,
            current_client_count,
            claim_count,
        ) {
            Ok(values) => values,
            Err(error) => {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(error);
            }
        };
        let mut counter_rollback = BoltV3SubmitAdmissionCounterRollback {
            execution_client_id: execution_client_id.to_string(),
            order_count: claim_count,
            uncommitted_evidence_free_client_order_ids: Vec::new(),
        };

        let pure_risk_reducing = admitted_intent_kinds
            .iter()
            .all(|kind| *kind == BasketAdmissionIntentKind::RiskReducingExit);
        let grant_fact = BasketAdmissionGrantedFact {
            details: details.clone(),
            admitted_legs: claims
                .iter()
                .zip(reservation_attributions)
                .zip(admitted_intent_kinds)
                .map(|((claim, reservation), intent_kind)| BasketAdmittedLeg {
                    client_order_id: claim.client_order_id.clone(),
                    instrument_id: claim.instrument_id.clone(),
                    intent_kind,
                    reservation,
                })
                .collect(),
        };
        let mut next_authority = inner.committed_admission_authority.clone();
        if next_authority
            .apply(ReservationRecoveryEvent::BasketGranted(grant_fact.clone()))
            .is_err()
        {
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            return Err(BoltV3SubmitAdmissionError::ClientOrderAlreadyAuthorized);
        }
        let (evidence_authority, pure_exit_evidence_unavailable) = match record_grant(grant_fact) {
            Ok(committed) => {
                inner.committed_admission_authority = next_authority;
                (
                    BoltV3SubmitEvidenceAuthority::CommittedBasket(committed),
                    false,
                )
            }
            Err(err) if pure_risk_reducing => {
                log::error!(
                    "bolt-v3 pure risk-reducing basket evidence write failed; exit remains admitted: {err:#}"
                );
                (BoltV3SubmitEvidenceAuthority::RiskReducing, true)
            }
            Err(err) => {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
        };

        inner.admitted_order_count = counter_values.admitted_order_count;
        inner.admitted_order_count_by_execution_client.insert(
            execution_client_id.to_string(),
            counter_values.execution_client_order_count,
        );
        if pure_exit_evidence_unavailable {
            for leg in claims {
                inner.evidence_free_non_reservation_admissions.insert(
                    leg.client_order_id.clone(),
                    BoltV3EvidenceFreeNonReservationPhase::PermitHeld,
                );
                counter_rollback
                    .uncommitted_evidence_free_client_order_ids
                    .push(leg.client_order_id.clone());
            }
        }

        Ok(BoltV3SubmitAdmissionPermit {
            inner: self.inner.clone(),
            rollbacks,
            counter_rollback: Some(counter_rollback),
            evidence_authority: Some(evidence_authority),
            committed: false,
        })
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    pub fn reserve_basket_submit_slots(
        &self,
        execution_client_id: &str,
        claims: &[BoltV3BasketSubmitSlotClaim],
        details: &BasketAdmissionDetails,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        let result = self.reserve_basket_submit_slots_with_evidence(
            execution_client_id,
            claims,
            details,
            |fact| self.decision_evidence.record_basket_admission_granted(fact),
        );
        if result.is_err() {
            self.record_basket_submit_rejection_for_test(details);
        }
        result
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    fn record_basket_submit_rejection_for_test(&self, details: &BasketAdmissionDetails) {
        if let NonBlockingRecordOutcome::Failed(error) = self
            .decision_evidence
            .record_basket_admission_rejected(BasketAdmissionRejectedFact {
                details: details.clone(),
                reason: BasketAdmissionRejectionReason::SubmitSlots,
            })
        {
            log::error!("basket submit rejection evidence failed: {error}");
        }
    }

    fn evaluate(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionEvaluation, BoltV3SubmitAdmissionError> {
        if inner
            .committed_admission_authority
            .authorizes_order(&request.client_order_id)
            || inner
                .evidence_free_non_reservation_admissions
                .contains_key(&request.client_order_id)
        {
            return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                AdmissionDecisionOutcome::Rejected(
                    AdmissionRejectionReason::ClientOrderAlreadyAuthorized,
                ),
                now_ns,
            ));
        }
        if inner.kill_switch_state.kind() != KillSwitchStateKind::Armed {
            return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::KillSwitchLatched),
                now_ns,
            ));
        }
        let mut loss_snapshot_diagnostics = None;
        if let Some(loss_policy) = inner.loss_policy.clone()
            && request.intent_kind == BoltV3SubmitIntentKind::Entry
        {
            let decision = evaluate_loss_admission_with_observations(
                &loss_policy,
                inner.loss_snapshot.as_ref(),
                now_ns,
                inner.loss_source_observations,
            );
            loss_snapshot_diagnostics = Some(decision.diagnostics.clone());
            if !decision.accepted {
                if decision
                    .halt_reasons
                    .contains(&LossHaltReason::StaleLossSnapshot)
                {
                    self.record_stale_loss_governor_halt(inner, &loss_policy, now_ns)?;
                }
                return Ok(BoltV3SubmitAdmissionEvaluation::loss_halt(
                    decision.halt_reasons,
                    decision.diagnostics,
                ));
            }
            inner.loss_halt_episodes.clear();
        }
        if request.notional <= Decimal::ZERO {
            return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NonPositiveNotional),
                now_ns,
            )
            .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
        }
        if let Some(limits) = inner
            .live_submit_approval_limits
            .get(&request.execution_client_id)
        {
            if request.notional > limits.max_order_notional {
                return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                    AdmissionDecisionOutcome::Rejected(
                        AdmissionRejectionReason::NotionalCapExceeded,
                    ),
                    now_ns,
                )
                .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
            }
            let current_count = inner
                .admitted_order_count_by_execution_client
                .get(&request.execution_client_id)
                .copied()
                .unwrap_or(0);
            if live_submit_count_cap_outcome(current_count, 1, limits.max_order_count)
                == AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted)
            {
                return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                    AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted),
                    now_ns,
                )
                .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
            }
        }
        match request.intent_kind {
            BoltV3SubmitIntentKind::Entry => {}
            BoltV3SubmitIntentKind::RiskReducingExit => {
                let Some(proof) = request.risk_reducing_exit_proof.as_ref() else {
                    return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        AdmissionDecisionOutcome::Rejected(
                            AdmissionRejectionReason::InvalidRiskReducingExitProof,
                        ),
                        now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
                };
                if !proof.is_valid_for_shape(
                    &request.instrument_id,
                    request.order_side,
                    request.order_quantity,
                ) {
                    return Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        AdmissionDecisionOutcome::Rejected(
                            AdmissionRejectionReason::InvalidRiskReducingExitProof,
                        ),
                        now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
                }
            }
        }
        if inner.capital_admission.is_some() {
            match evaluate_capital_admission_submit(inner, request, now_ns) {
                BoltV3CapitalAdmissionSubmitDecision::Rejected { reason } => {
                    return Ok(BoltV3SubmitAdmissionEvaluation::capital_admission_rejected(
                        reason, now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
                }
                BoltV3CapitalAdmissionSubmitDecision::Accepted(accepted) => {
                    return Ok(BoltV3SubmitAdmissionEvaluation::admitted_with_rollback(
                        accepted.rollback,
                        accepted.reservation_attribution,
                        now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics));
                }
                BoltV3CapitalAdmissionSubmitDecision::Invariant { invariant } => {
                    return Err(BoltV3SubmitAdmissionError::InvariantViolation { invariant });
                }
            }
        }
        Ok(BoltV3SubmitAdmissionEvaluation::without_loss_halt(
            AdmissionDecisionOutcome::Admitted,
            now_ns,
        )
        .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics))
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .admitted_order_count
    }

    /// Number of retained reject-episode keys. Observability-only accessor used to
    /// prove the reject-episode map stays bounded under a sustained-reject regime;
    /// it never participates in an admission decision.
    pub fn reject_episode_count(&self) -> usize {
        self.reject_episodes
            .lock()
            .expect("submit admission reject episodes mutex should not be poisoned")
            .len()
    }

    /// Configured upper bound for the reject-episode map.
    #[must_use]
    pub fn reject_episode_capacity(&self) -> usize {
        self.decision_evidence.reject_episode_max_count()
    }
}

fn loss_halt_reason_to_evidence(reason: LossHaltReason) -> EvidenceLossHaltReason {
    match reason {
        LossHaltReason::PerTradeLossLimit => EvidenceLossHaltReason::PerTradeLossLimit,
        LossHaltReason::DailyLossLimit => EvidenceLossHaltReason::DailyLossLimit,
        LossHaltReason::RollingLossLimit => EvidenceLossHaltReason::RollingLossLimit,
        LossHaltReason::MaxDrawdownLimit => EvidenceLossHaltReason::MaxDrawdownLimit,
        LossHaltReason::StaleLossSnapshot => EvidenceLossHaltReason::StaleLossSnapshot,
    }
}

fn loss_snapshot_stale_reason_to_evidence(
    reason: LossSnapshotStaleReason,
) -> EvidenceLossSnapshotStaleReason {
    match reason {
        LossSnapshotStaleReason::MissingSnapshot => {
            EvidenceLossSnapshotStaleReason::MissingSnapshot
        }
        LossSnapshotStaleReason::SourceEmpty => EvidenceLossSnapshotStaleReason::SourceEmpty,
        LossSnapshotStaleReason::FutureDated => EvidenceLossSnapshotStaleReason::FutureDated,
        LossSnapshotStaleReason::AgeExceeded => EvidenceLossSnapshotStaleReason::AgeExceeded,
        LossSnapshotStaleReason::MissingRequiredField => {
            EvidenceLossSnapshotStaleReason::MissingRequiredField
        }
    }
}

fn stale_loss_governor_halt_reason(
    policy: &LossGovernorPolicy,
    snapshot: Option<&LossSnapshot>,
    now_ns: u64,
) -> Result<StaleLossReason, BoltV3SubmitAdmissionError> {
    match snapshot {
        None => Ok(StaleLossReason::MissingSnapshot),
        Some(snapshot) => loss_snapshot_stale_reason(policy, snapshot, now_ns)
            .map(loss_snapshot_stale_reason_to_halt_reason)
            .ok_or(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant: BoltV3SubmitAdmissionInvariant::MissingStaleLossReason,
            }),
    }
}

fn capital_admission_rejection_error(
    reason: Option<BoltV3CapitalAdmissionRejectReason>,
) -> BoltV3SubmitAdmissionError {
    match reason {
        Some(reason) => BoltV3SubmitAdmissionError::CapitalAdmissionRejected { reason },
        None => BoltV3SubmitAdmissionError::InvariantViolation {
            invariant: BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRejectionReason,
        },
    }
}

fn submit_admission_error_from_outcome(
    outcome: AdmissionDecisionOutcome,
    kill_switch_state: KillSwitchStateKind,
) -> BoltV3SubmitAdmissionError {
    match outcome {
        AdmissionDecisionOutcome::Admitted => BoltV3SubmitAdmissionError::InvariantViolation {
            invariant: BoltV3SubmitAdmissionInvariant::ExpectedRejectedOutcome,
        },
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::KillSwitchLatched) => {
            BoltV3SubmitAdmissionError::KillSwitchLatched {
                state: kill_switch_state,
            }
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted) => {
            BoltV3SubmitAdmissionError::InvariantViolation {
                invariant: BoltV3SubmitAdmissionInvariant::MissingLossHaltReasons,
            }
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NonPositiveNotional) => {
            BoltV3SubmitAdmissionError::NonPositiveNotional
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::NotionalCapExceeded) => {
            BoltV3SubmitAdmissionError::NotionalCapExceeded
        }
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::ClientOrderAlreadyAuthorized,
        ) => BoltV3SubmitAdmissionError::ClientOrderAlreadyAuthorized,
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::InvalidRiskReducingExitProof,
        ) => BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof,
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CountCapExhausted) => {
            BoltV3SubmitAdmissionError::CountCapExhausted
        }
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission) => {
            capital_admission_rejection_error(None)
        }
        AdmissionDecisionOutcome::Rejected(
            AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid
            | AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded,
        ) => retired_forced_reduction_outcome_error(),
        AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::EconomicsSealRejected) => {
            BoltV3SubmitAdmissionError::EconomicsSealRejected
        }
    }
}

fn retired_forced_reduction_outcome_error() -> BoltV3SubmitAdmissionError {
    BoltV3SubmitAdmissionError::InvariantViolation {
        invariant: BoltV3SubmitAdmissionInvariant::KillSwitchForcedReductionReachedNormalAdmission,
    }
}

#[derive(Debug)]
enum RecordedAdmissionAuthority {
    CommittedEntry(CommittedAdmission),
    RiskReducing { evidence_committed: bool },
    NoExternalActionAuthority,
}

impl RecordedAdmissionAuthority {
    fn requires_evidence_free_non_reservation_tracking(&self) -> bool {
        matches!(
            self,
            Self::RiskReducing {
                evidence_committed: false,
            }
        )
    }

    fn into_submit_authority(
        self,
    ) -> Result<BoltV3SubmitEvidenceAuthority, BoltV3SubmitAdmissionError> {
        match self {
            Self::CommittedEntry(committed) => {
                Ok(BoltV3SubmitEvidenceAuthority::CommittedEntry(committed))
            }
            Self::RiskReducing { .. } => Ok(BoltV3SubmitEvidenceAuthority::RiskReducing),
            Self::NoExternalActionAuthority => {
                Err(BoltV3SubmitAdmissionError::InvariantViolation {
                    invariant:
                        BoltV3SubmitAdmissionInvariant::AdmittedSubmitMissingEvidenceAuthority,
                })
            }
        }
    }
}

#[derive(Debug)]
enum BoltV3SubmitEvidenceAuthority {
    CommittedEntry(CommittedAdmission),
    CommittedBasket(CommittedAdmission),
    RiskReducing,
}

impl BoltV3SubmitEvidenceAuthority {
    fn consume(self) {
        match self {
            Self::CommittedEntry(committed) | Self::CommittedBasket(committed) => drop(committed),
            Self::RiskReducing => {}
        }
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionPermit {
    inner: Arc<Mutex<BoltV3SubmitAdmissionInner>>,
    rollbacks: Vec<BoltV3CapitalAdmissionReservationRollback>,
    counter_rollback: Option<BoltV3SubmitAdmissionCounterRollback>,
    evidence_authority: Option<BoltV3SubmitEvidenceAuthority>,
    committed: bool,
}

impl BoltV3SubmitAdmissionPermit {
    pub fn commit_submitted(mut self) {
        self.prepare_sink_invocation()
            .expect("admitted reservation must remain exact until the submit boundary")
            .commit();
    }

    pub(crate) fn prepare_sink_invocation(
        &mut self,
    ) -> Result<BoltV3PreparedSubmitSinkInvocation<'_>, ReservationRejectionReason> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| ReservationRejectionReason::ReconciliationRequired)?;
        let prepared = prepare_submit_sink_invocation(
            &inner,
            &self.rollbacks,
            self.counter_rollback.as_ref(),
        )?;
        let evidence_authority = self
            .evidence_authority
            .take()
            .ok_or(ReservationRejectionReason::ReconciliationRequired)?;
        Ok(BoltV3PreparedSubmitSinkInvocation {
            inner,
            prepared: Some(prepared),
            evidence_authority: Some(evidence_authority),
            evidence_slot: &mut self.evidence_authority,
            committed: &mut self.committed,
        })
    }
}

#[derive(Debug)]
struct PreparedSubmitSinkInvocationTransition {
    candidate_capital_admission: Option<BoltV3SubmitCapitalAdmissionState>,
    candidate_evidence_free_non_reservation_admissions:
        Option<BTreeMap<String, BoltV3EvidenceFreeNonReservationPhase>>,
    final_revision: u64,
}

#[must_use = "prepared capital admission must cross the submit boundary or remain refundable"]
pub(crate) struct BoltV3PreparedSubmitSinkInvocation<'a> {
    inner: MutexGuard<'a, BoltV3SubmitAdmissionInner>,
    prepared: Option<PreparedSubmitSinkInvocationTransition>,
    evidence_authority: Option<BoltV3SubmitEvidenceAuthority>,
    evidence_slot: &'a mut Option<BoltV3SubmitEvidenceAuthority>,
    committed: &'a mut bool,
}

impl BoltV3PreparedSubmitSinkInvocation<'_> {
    pub(crate) fn commit(&mut self) {
        if let Some(prepared) = self.prepared.take() {
            commit_submit_sink_invocation(&mut self.inner, prepared);
        }
        if let Some(authority) = self.evidence_authority.take() {
            authority.consume();
        }
        *self.committed = true;
    }
}

impl Drop for BoltV3PreparedSubmitSinkInvocation<'_> {
    fn drop(&mut self) {
        *self.evidence_slot = self.evidence_authority.take();
    }
}

fn prepare_submit_sink_invocation(
    inner: &BoltV3SubmitAdmissionInner,
    rollbacks: &[BoltV3CapitalAdmissionReservationRollback],
    counter_rollback: Option<&BoltV3SubmitAdmissionCounterRollback>,
) -> Result<PreparedSubmitSinkInvocationTransition, ReservationRejectionReason> {
    let evidence_free_client_order_ids = counter_rollback
        .map(|rollback| {
            rollback
                .uncommitted_evidence_free_client_order_ids
                .as_slice()
        })
        .unwrap_or_default();
    let candidate_evidence_free_non_reservation_admissions =
        if evidence_free_client_order_ids.is_empty() {
            None
        } else {
            let mut candidate = inner.evidence_free_non_reservation_admissions.clone();
            let mut unique_client_order_ids = BTreeSet::new();
            for client_order_id in evidence_free_client_order_ids {
                if !unique_client_order_ids.insert(client_order_id)
                    || candidate.get(client_order_id)
                        != Some(&BoltV3EvidenceFreeNonReservationPhase::PermitHeld)
                {
                    return Err(ReservationRejectionReason::UnknownReservation);
                }
                candidate.insert(
                    client_order_id.clone(),
                    BoltV3EvidenceFreeNonReservationPhase::SinkInvoked,
                );
            }
            Some(candidate)
        };
    let mut candidate_capital_admission = if rollbacks.is_empty() {
        None
    } else {
        Some(
            inner
                .capital_admission
                .clone()
                .ok_or(ReservationRejectionReason::UnknownReservation)?,
        )
    };
    let mut next_revision = inner.capital_admission_state_revision;
    for rollback in rollbacks {
        let capital_admission = candidate_capital_admission
            .as_mut()
            .ok_or(ReservationRejectionReason::UnknownReservation)?;
        next_revision = next_revision
            .checked_add(1)
            .ok_or(ReservationRejectionReason::ReconciliationRequired)?;
        let record = capital_admission
            .client_order_reservations
            .get(&rollback.client_order_id)
            .ok_or(ReservationRejectionReason::UnknownReservation)?;
        match (
            record.phase,
            record.attribution.submit_reservation_id == rollback.submit_reservation_id,
            record.attribution.capital_pool_id == rollback.pool_id,
        ) {
            (BoltV3SubmitReservationPhase::Reserved, true, true) => {}
            _ => return Err(ReservationRejectionReason::UnknownReservation),
        }
        capital_admission.gate.retained_reservation(
            &record.attribution.capital_pool_id,
            &record.attribution.collateral_group_id,
            &record.attribution.submit_reservation_id,
        )?;
        let record = capital_admission
            .client_order_reservations
            .get_mut(&rollback.client_order_id)
            .ok_or(ReservationRejectionReason::UnknownReservation)?;
        record.phase = BoltV3SubmitReservationPhase::SinkInvoked;
        record.phase_revision = next_revision;
    }
    Ok(PreparedSubmitSinkInvocationTransition {
        candidate_capital_admission,
        candidate_evidence_free_non_reservation_admissions,
        final_revision: next_revision,
    })
}

fn commit_submit_sink_invocation(
    inner: &mut BoltV3SubmitAdmissionInner,
    prepared: PreparedSubmitSinkInvocationTransition,
) {
    if let Some(candidate_capital_admission) = prepared.candidate_capital_admission {
        inner.capital_admission = Some(candidate_capital_admission);
    }
    if let Some(candidate_evidence_free_non_reservation_admissions) =
        prepared.candidate_evidence_free_non_reservation_admissions
    {
        inner.evidence_free_non_reservation_admissions =
            candidate_evidence_free_non_reservation_admissions;
    }
    inner.capital_admission_state_revision = prepared.final_revision;
}

impl Drop for BoltV3SubmitAdmissionPermit {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        rollback_unsubmitted_permit(&mut inner, &self.rollbacks, self.counter_rollback.as_ref());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitAdmissionCounterRollback {
    execution_client_id: String,
    order_count: u32,
    uncommitted_evidence_free_client_order_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BoltV3SubmitAdmissionCounterValues {
    admitted_order_count: u32,
    execution_client_order_count: u32,
}

fn checked_basket_claim_count(claim_count: usize) -> Result<u32, BoltV3SubmitAdmissionError> {
    u32::try_from(claim_count).map_err(|_| BoltV3SubmitAdmissionError::CountCapExhausted)
}

fn checked_admission_counter_values(
    admitted_order_count: u32,
    execution_client_order_count: u32,
    claim_count: u32,
) -> Result<BoltV3SubmitAdmissionCounterValues, BoltV3SubmitAdmissionError> {
    let admitted_order_count = admitted_order_count
        .checked_add(claim_count)
        .ok_or(BoltV3SubmitAdmissionError::CountCapExhausted)?;
    let execution_client_order_count = execution_client_order_count
        .checked_add(claim_count)
        .ok_or(BoltV3SubmitAdmissionError::CountCapExhausted)?;
    Ok(BoltV3SubmitAdmissionCounterValues {
        admitted_order_count,
        execution_client_order_count,
    })
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionEvaluation {
    outcome: AdmissionDecisionOutcome,
    admission_now_ns: u64,
    loss_halt_reasons: Vec<LossHaltReason>,
    loss_snapshot_diagnostics: Option<LossSnapshotDiagnostics>,
    capital_admission_rejection: Option<BoltV3CapitalAdmissionRejectReason>,
    rollback: Option<BoltV3CapitalAdmissionReservationRollback>,
    reservation_attribution: Option<ReservationAttribution>,
}

impl BoltV3SubmitAdmissionEvaluation {
    fn validate(&self) -> Result<(), BoltV3SubmitAdmissionError> {
        let has_loss_context = !self.loss_halt_reasons.is_empty();
        let has_capital_context = self.capital_admission_rejection.is_some();
        let has_admitted_context =
            self.rollback.is_some() || self.reservation_attribution.is_some();
        match self.outcome {
            AdmissionDecisionOutcome::Admitted if !has_loss_context && !has_capital_context => {
                Ok(())
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted)
                if !has_loss_context =>
            {
                Err(BoltV3SubmitAdmissionError::InvariantViolation {
                    invariant: BoltV3SubmitAdmissionInvariant::MissingLossHaltReasons,
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted)
                if !has_capital_context && !has_admitted_context =>
            {
                Ok(())
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission)
                if !has_capital_context =>
            {
                Err(BoltV3SubmitAdmissionError::InvariantViolation {
                    invariant:
                        BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRejectionReason,
                })
            }
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission)
                if !has_loss_context && !has_admitted_context =>
            {
                Ok(())
            }
            AdmissionDecisionOutcome::Rejected(_)
                if !has_loss_context && !has_capital_context && !has_admitted_context =>
            {
                Ok(())
            }
            AdmissionDecisionOutcome::Admitted | AdmissionDecisionOutcome::Rejected(_) => {
                Err(BoltV3SubmitAdmissionError::InvariantViolation {
                    invariant:
                        BoltV3SubmitAdmissionInvariant::ContradictoryAdmissionDecisionContext,
                })
            }
        }
    }

    fn without_loss_halt(outcome: AdmissionDecisionOutcome, admission_now_ns: u64) -> Self {
        Self {
            outcome,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            capital_admission_rejection: None,
            rollback: None,
            reservation_attribution: None,
        }
    }

    fn loss_halt(
        loss_halt_reasons: Vec<LossHaltReason>,
        diagnostics: LossSnapshotDiagnostics,
    ) -> Self {
        let admission_now_ns = diagnostics.admission_now_ns;
        Self {
            outcome: AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::LossGovernorHalted,
            ),
            admission_now_ns,
            loss_halt_reasons,
            loss_snapshot_diagnostics: Some(diagnostics),
            capital_admission_rejection: None,
            rollback: None,
            reservation_attribution: None,
        }
    }

    fn capital_admission_rejected(
        reason: BoltV3CapitalAdmissionRejectReason,
        admission_now_ns: u64,
    ) -> Self {
        Self {
            outcome: AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission),
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            capital_admission_rejection: Some(reason),
            rollback: None,
            reservation_attribution: None,
        }
    }

    fn admitted_with_rollback(
        rollback: Option<BoltV3CapitalAdmissionReservationRollback>,
        reservation_attribution: Option<ReservationAttribution>,
        admission_now_ns: u64,
    ) -> Self {
        Self {
            outcome: AdmissionDecisionOutcome::Admitted,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            capital_admission_rejection: None,
            rollback,
            reservation_attribution,
        }
    }

    fn with_loss_snapshot_diagnostics(
        mut self,
        diagnostics: Option<LossSnapshotDiagnostics>,
    ) -> Self {
        if diagnostics.is_some() {
            self.loss_snapshot_diagnostics = diagnostics;
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3RiskReducingExitProof {
    pub position_id: String,
    pub instrument_id: String,
    pub position_side: PositionSide,
    pub exit_order_side: OrderSide,
    pub position_quantity: Decimal,
    pub exit_quantity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3RiskReducingExitPositionInput<'a> {
    pub position_id: &'a str,
    pub instrument_id: &'a str,
    pub position_side: PositionSide,
    pub position_quantity: Decimal,
}

impl BoltV3RiskReducingExitProof {
    fn is_valid_for_shape(
        &self,
        instrument_id: &str,
        order_side: OrderSide,
        order_quantity: Decimal,
    ) -> bool {
        self.instrument_id == instrument_id
            && self.exit_order_side == order_side
            && self.exit_quantity == order_quantity
            && self.position_quantity > Decimal::ZERO
            && self.exit_quantity > Decimal::ZERO
            && self.exit_quantity <= self.position_quantity
            && matches!(
                (self.position_side, order_side),
                (PositionSide::Long, OrderSide::Sell) | (PositionSide::Short, OrderSide::Buy)
            )
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionRequest {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
    pub order_side: OrderSide,
    pub order_quantity: Decimal,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
    pub admission_evidence: Option<BoltV3CompiledOrderAdmissionEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3CompiledOrderAdmissionEvidence {
    pub venue_id: String,
    pub product_kind: BoltV3CompiledProductKind,
    pub side: BoltV3CompiledOrderSide,
    pub quantity: Decimal,
    pub effective_price: Decimal,
    pub order_kind: BoltV3CompiledOrderKind,
    pub liquidity: BoltV3CompiledOrderLiquidity,
    pub quote_set_id: Option<String>,
    pub prediction_market_outcome: Option<PredictionMarketOutcomeSide>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledProductKind {
    PredictionMarketBinary,
}

impl BoltV3CompiledProductKind {
    fn to_capital_admission(self) -> ProductKind {
        match self {
            Self::PredictionMarketBinary => ProductKind::PredictionMarketBinary,
        }
    }
}

fn reservation_product_kind(product_kind: BoltV3CompiledProductKind) -> ReservationProductKind {
    match product_kind {
        BoltV3CompiledProductKind::PredictionMarketBinary => {
            ReservationProductKind::PredictionMarketBinary
        }
    }
}

fn reservation_product_kind_from_capital(product_kind: ProductKind) -> ReservationProductKind {
    match product_kind {
        ProductKind::PredictionMarketBinary => ReservationProductKind::PredictionMarketBinary,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderSide {
    Buy,
    Sell,
}

impl BoltV3CompiledOrderSide {
    fn to_capital_admission(self) -> IntentSide {
        match self {
            Self::Buy => IntentSide::Buy,
            Self::Sell => IntentSide::Sell,
        }
    }
}

fn evidence_order_side(side: BoltV3CompiledOrderSide) -> EvidenceOrderSide {
    match side {
        BoltV3CompiledOrderSide::Buy => EvidenceOrderSide::Buy,
        BoltV3CompiledOrderSide::Sell => EvidenceOrderSide::Sell,
    }
}

fn reservation_fill_update_matches(
    fact: &SubmitReservationFillFact,
    update: &BoltV3SubmitCapitalAdmissionFillUpdate,
) -> bool {
    fact.client_order_id == update.client_order_id
        && fact.trade_id == update.trade_id
        && fact.instrument_id == update.instrument_id
        && fact.side == evidence_order_side(update.side)
        && fact
            .fill_quantity
            .parse::<Decimal>()
            .is_ok_and(|quantity| quantity == update.fill_quantity)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderKind {
    Limit,
    Market,
}

impl BoltV3CompiledOrderKind {
    fn to_capital_admission(self) -> IntentOrderKind {
        match self {
            Self::Limit => IntentOrderKind::Limit,
            Self::Market => IntentOrderKind::Market,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderLiquidity {
    RestingMaker,
    Taker,
}

impl BoltV3CompiledOrderLiquidity {
    fn to_capital_admission(self) -> IntentLiquidity {
        match self {
            Self::RestingMaker => IntentLiquidity::RestingMaker,
            Self::Taker => IntentLiquidity::Taker,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictionMarketOutcomeSide {
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3CapitalAdmissionReservationRollback {
    client_order_id: String,
    submit_reservation_id: String,
    pool_id: String,
    observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3CapitalAdmissionAcceptance {
    rollback: Option<BoltV3CapitalAdmissionReservationRollback>,
    reservation_attribution: Option<ReservationAttribution>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BoltV3CapitalAdmissionSubmitDecision {
    Accepted(Box<BoltV3CapitalAdmissionAcceptance>),
    Rejected {
        reason: BoltV3CapitalAdmissionRejectReason,
    },
    Invariant {
        invariant: BoltV3SubmitAdmissionInvariant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValidatedCapitalAdmissionDecision {
    Accepted {
        accepted_quantity: Decimal,
        reserved_liability: Decimal,
    },
    Rejected {
        reason: BoltV3CapitalAdmissionRejectReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketSubmitSlotClaim {
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
    pub order_side: OrderSide,
    pub order_quantity: Decimal,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
    pub admission_evidence: Option<BoltV3CompiledOrderAdmissionEvidence>,
}

fn basket_submit_request(
    strategy_id: &str,
    execution_client_id: &str,
    claim: &BoltV3BasketSubmitSlotClaim,
) -> BoltV3SubmitAdmissionRequest {
    BoltV3SubmitAdmissionRequest {
        strategy_id: strategy_id.to_string(),
        execution_client_id: execution_client_id.to_string(),
        client_order_id: claim.client_order_id.clone(),
        instrument_id: claim.instrument_id.clone(),
        notional: claim.notional,
        order_side: claim.order_side,
        order_quantity: claim.order_quantity,
        intent_kind: claim.intent_kind,
        risk_reducing_exit_proof: claim.risk_reducing_exit_proof.clone(),
        admission_evidence: claim.admission_evidence.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct OrderValuationContext<'a> {
    pub last_quote: Option<QuoteTick>,
    pub last_trade: Option<TradeTick>,
    pub instrument: Option<&'a InstrumentAny>,
}

impl OrderValuationContext<'_> {
    pub const fn empty() -> Self {
        Self {
            last_quote: None,
            last_trade: None,
            instrument: None,
        }
    }

    /// Preserves the historical valuation contract for quote-present unsided
    /// market-style orders: without a Buy or Sell side, no quote price is
    /// usable and this helper must not substitute the last trade. Submit
    /// admission separately rejects every unsided quote-quantity market-style
    /// order before valuation.
    pub fn prices_for_order(&self, order: &OrderAny) -> (Option<Price>, Option<Price>) {
        let uses_submitted_notional = order.is_quote_quantity()
            && matches!(order, OrderAny::Market(_))
            && order.order_side() == OrderSide::Buy;
        if !order.is_quote_quantity() || uses_submitted_notional {
            return (None, None);
        }

        let quote_side_price = self.last_quote.and_then(|quote| match order.order_side() {
            OrderSide::Buy => Some(quote.ask_price),
            OrderSide::Sell => Some(quote.bid_price),
            _ => None,
        });
        let last_price = match order {
            OrderAny::Market(_) | OrderAny::MarketToLimit(_) => {
                if self.last_quote.is_some() && quote_side_price.is_none() {
                    None
                } else {
                    quote_side_price.or_else(|| self.last_trade.map(|trade| trade.price))
                }
            }
            OrderAny::StopMarket(_) | OrderAny::MarketIfTouched(_) => order.trigger_price(),
            OrderAny::TrailingStopMarket(_) | OrderAny::TrailingStopLimit(_) => {
                order.trigger_price()
            }
            _ => order.price(),
        };
        (last_price, quote_side_price)
    }
}

#[derive(Debug, Clone)]
pub struct BoltV3SubmitAdmissionRequestInput<'a> {
    pub execution_client_id: &'a str,
    pub intent: &'a OrderIntentDetails,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub order: &'a OrderAny,
    pub valuation: OrderValuationContext<'a>,
    pub risk_reducing_exit_position: Option<BoltV3RiskReducingExitPositionInput<'a>>,
}

fn compiled_order_price_source(fallback_price: String, order: &OrderAny) -> String {
    order
        .price()
        .map(|price| price.to_string())
        .or_else(|| order.trigger_price().map(|price| price.to_string()))
        .or_else(|| order.activation_price().map(|price| price.to_string()))
        .unwrap_or(fallback_price)
}

#[derive(Debug)]
pub struct BoltV3EconomicsSubmitAdmission {
    request: BoltV3SubmitAdmissionRequest,
    economics: EconomicsAdmission,
    required_remaining_margin_ns: u64,
}

impl BoltV3EconomicsSubmitAdmission {
    pub const fn request(&self) -> &BoltV3SubmitAdmissionRequest {
        &self.request
    }

    pub const fn economics(&self) -> &EconomicsAdmission {
        &self.economics
    }

    pub(crate) fn into_parts(self) -> (BoltV3SubmitAdmissionRequest, EconomicsAdmission, u64) {
        (
            self.request,
            self.economics,
            self.required_remaining_margin_ns,
        )
    }

    pub(crate) fn with_compiled_order_admission_evidence(
        mut self,
        evidence: BoltV3CompiledOrderAdmissionEvidence,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            self.request.admission_evidence.is_none(),
            "compiled-order admission evidence is single-authority"
        );
        let expected_side = match self.request.order_side {
            OrderSide::Buy => BoltV3CompiledOrderSide::Buy,
            OrderSide::Sell => BoltV3CompiledOrderSide::Sell,
            OrderSide::NoOrderSide => {
                anyhow::bail!("compiled-order admission evidence requires a sided order")
            }
        };
        anyhow::ensure!(
            evidence.side == expected_side && evidence.quantity == self.request.order_quantity,
            "compiled-order admission evidence does not bind the final order"
        );
        self.request.admission_evidence = Some(evidence);
        Ok(self)
    }
}

pub(crate) fn validate_economics_remaining_margin_at(
    economics: &EconomicsAdmission,
    required_margin_ns: u64,
    now_ns: u64,
) -> Result<(), BoltV3SubmitAdmissionError> {
    let requested_at_ns = economics.request().requested_at_ns;
    let valid_until_ns = economics.quote().valid_until_ns();
    if now_ns < requested_at_ns
        || valid_until_ns
            .checked_sub(now_ns)
            .is_none_or(|remaining| remaining < required_margin_ns)
    {
        return Err(
            BoltV3SubmitAdmissionError::EconomicsRemainingMarginUnavailable {
                requested_at_ns,
                valid_until_ns,
                observed_at_ns: now_ns,
                required_margin_ns,
            },
        );
    }
    Ok(())
}

pub(crate) fn validate_economics_submit_authority(
    request: &BoltV3SubmitAdmissionRequest,
    economics: &EconomicsAdmission,
    order: &OrderAny,
    execution_client_id: &str,
) -> Result<(), BoltV3SubmitAdmissionError> {
    if economics_order_binding(order).ok().as_ref() != Some(economics.order_binding())
        || request.notional != economics.full_reservation_liability()
        || request.execution_client_id != execution_client_id
        || economics.request().execution_client_id.as_str() != execution_client_id
    {
        return Err(BoltV3SubmitAdmissionError::EconomicsOrderMismatch);
    }
    Ok(())
}

fn ensure_economics_available(
    economics: Option<&EconomicsAdmission>,
    now_ns: u64,
) -> Result<(), BoltV3SubmitAdmissionError> {
    if let Some(economics) = economics {
        economics
            .quote()
            .capability_health()
            .allows_admission(now_ns)
            .map_err(BoltV3SubmitAdmissionError::EconomicsUnavailable)?;
    }
    Ok(())
}

fn admission_economics_details(
    economics: &EconomicsAdmission,
) -> anyhow::Result<AdmissionEconomicsDetails> {
    let quote = economics.quote();
    let missing_forecast_component_ids = quote
        .missing_forecast_component_ids()
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    let mut components = Vec::with_capacity(quote.components().len());
    for evaluated in quote.components() {
        let component = evaluated.component();
        components.push(AdmissionEconomicsComponent {
            component_id: component.component_id.to_string(),
            class: match component.class {
                crate::economics::EconomicClass::Charge => AdmissionEconomicsClass::Charge,
                crate::economics::EconomicClass::Credit => AdmissionEconomicsClass::Credit,
            },
            economic_kind: admission_economics_kind(component.kind),
            scope: admission_economics_scope(&component.scope),
            point_estimate: match &component.point_estimate {
                crate::economics::PointEstimate::NonZero(effect) => {
                    AdmissionEconomicsPointEstimate::NonZero {
                        effect: admission_economics_native_effect(effect),
                    }
                }
                crate::economics::PointEstimate::ProvenZero { factor_id } => {
                    AdmissionEconomicsPointEstimate::ProvenZero {
                        factor_id: factor_id.to_string(),
                    }
                }
            },
            point_valuation: evaluated.point_valuation().map(Into::into),
            debit_risk_bound: component
                .debit_risk_bound
                .as_ref()
                .map(admission_economics_native_effect),
            debit_risk_bound_valuation: evaluated.debit_risk_bound_valuation().map(Into::into),
            treatment: admission_economics_treatment(component.admission_treatment),
            calculation_factors: component
                .calculation_factors
                .iter()
                .map(|factor| AdmissionEconomicsCalculationFactor {
                    factor_id: factor.factor_id.to_string(),
                    value: factor.value.to_string(),
                })
                .collect(),
            formula_id: component.formula_id.to_string(),
            source: AdmissionEconomicsSource {
                source_id: component.source.source.to_string(),
                snapshot_id: component.source.snapshot_id.to_string(),
                source_at_ns: component.source.source_at_ns,
                fetched_at_ns: component.source.fetched_at_ns,
                valid_until_ns: component.source.valid_until_ns,
            },
        });
    }
    let source_snapshot_ids = quote
        .source_snapshot_ids()
        .iter()
        .chain(economics.net_edge().basis.source_snapshot_ids.iter())
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(AdmissionEconomicsDetails {
        decision_correlation_id: quote.decision_correlation_id().to_string(),
        core_total: quote.core_total().to_string(),
        core_net_edge: economics.net_edge().core_net_edge.to_string(),
        core_edge_ratio: economics.net_edge().core_edge_ratio.to_string(),
        forecast_net_edge: economics.net_edge().forecast_net_edge.to_string(),
        forecast_complete: quote.forecast_complete(),
        missing_forecast_component_ids: missing_forecast_component_ids.into_iter().collect(),
        valid_until_ns: quote.valid_until_ns(),
        forecast_valid_until_ns: quote.forecast_valid_until_ns(),
        source_snapshot_ids,
        reservation_basis: economics.reservation_basis().to_string(),
        full_reservation_liability: economics.full_reservation_liability().to_string(),
        components,
    })
}

fn admission_economics_kind(kind: crate::economics::EconomicKind) -> AdmissionEconomicsKind {
    match kind {
        crate::economics::EconomicKind::Execution(
            crate::economics::ExecutionKind::ProtocolTrading,
        ) => AdmissionEconomicsKind::ProtocolTrading,
        crate::economics::EconomicKind::Execution(
            crate::economics::ExecutionKind::AttachedRoutingCharge,
        ) => AdmissionEconomicsKind::AttachedRoutingCharge,
        crate::economics::EconomicKind::Carry(crate::economics::CarryKind::Funding) => {
            AdmissionEconomicsKind::Funding
        }
        crate::economics::EconomicKind::Carry(crate::economics::CarryKind::BorrowInterest) => {
            AdmissionEconomicsKind::BorrowInterest
        }
        crate::economics::EconomicKind::Carry(
            crate::economics::CarryKind::SuppliedBalanceInterest,
        ) => AdmissionEconomicsKind::SuppliedBalanceInterest,
        crate::economics::EconomicKind::Incentive(crate::economics::IncentiveKind::MakerRebate) => {
            AdmissionEconomicsKind::MakerRebate
        }
        crate::economics::EconomicKind::Incentive(
            crate::economics::IncentiveKind::LiquidityReward,
        ) => AdmissionEconomicsKind::LiquidityReward,
        crate::economics::EconomicKind::Incentive(
            crate::economics::IncentiveKind::HoldingReward,
        ) => AdmissionEconomicsKind::HoldingReward,
        crate::economics::EconomicKind::Incentive(
            crate::economics::IncentiveKind::ReferralReward,
        ) => AdmissionEconomicsKind::ReferralReward,
        crate::economics::EconomicKind::Incentive(crate::economics::IncentiveKind::FeeCredit) => {
            AdmissionEconomicsKind::FeeCredit
        }
    }
}

fn admission_economics_scope(scope: &crate::economics::EconomicScope) -> AdmissionEconomicsScope {
    match scope {
        crate::economics::EconomicScope::Decision {
            decision_correlation_id,
        } => AdmissionEconomicsScope::Decision {
            decision_correlation_id: decision_correlation_id.to_string(),
        },
        crate::economics::EconomicScope::PositionInterval {
            position_id,
            starts_at_ns,
            ends_at_ns,
        } => AdmissionEconomicsScope::PositionInterval {
            position_id: position_id.to_string(),
            starts_at_ns: *starts_at_ns,
            ends_at_ns: *ends_at_ns,
        },
        crate::economics::EconomicScope::Action { action_id } => AdmissionEconomicsScope::Action {
            action_id: action_id.to_string(),
        },
    }
}

fn admission_economics_native_effect(
    effect: &crate::economics::SignedNativeEffect,
) -> AdmissionEconomicsNativeEffect {
    match effect {
        crate::economics::SignedNativeEffect::CurrencyAmount {
            amount,
            currency_id,
        } => AdmissionEconomicsNativeEffect {
            amount: amount.to_string(),
            unit: AdmissionEconomicsNativeUnit::Currency {
                currency_id: currency_id.to_string(),
            },
            inventory_application: None,
        },
        crate::economics::SignedNativeEffect::AssetQuantity {
            quantity,
            asset_id,
            inventory_application,
        } => AdmissionEconomicsNativeEffect {
            amount: quantity.to_string(),
            unit: AdmissionEconomicsNativeUnit::Asset {
                asset_id: asset_id.to_string(),
            },
            inventory_application: Some(match inventory_application {
                crate::economics::InventoryApplication::AlreadyAppliedToGrossFill => {
                    AdmissionEconomicsInventoryApplication::AlreadyAppliedToGrossFill
                }
                crate::economics::InventoryApplication::ApplyOnceToNetPortfolio => {
                    AdmissionEconomicsInventoryApplication::ApplyOnceToNetPortfolio
                }
            }),
        },
    }
}

impl From<&crate::economics::ValuationEvidence> for AdmissionEconomicsValuation {
    fn from(valuation: &crate::economics::ValuationEvidence) -> Self {
        Self {
            native_effect: admission_economics_native_effect(&valuation.native_effect),
            normalized_amount: valuation.normalized_amount.to_string(),
            reporting_currency: valuation.reporting_currency.to_string(),
            route_id: valuation.route_id.as_ref().map(ToString::to_string),
            source_snapshot_ids: valuation
                .source_snapshot_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            valued_at_ns: valuation.valued_at_ns,
            valid_until_ns: valuation.valid_until_ns,
        }
    }
}

fn admission_economics_treatment(
    treatment: crate::economics::AdmissionTreatment,
) -> AdmissionEconomicsTreatment {
    match treatment {
        crate::economics::AdmissionTreatment::GuaranteedConditionalOnAction => {
            AdmissionEconomicsTreatment::GuaranteedConditionalOnAction
        }
        crate::economics::AdmissionTreatment::RiskBound { authority } => {
            AdmissionEconomicsTreatment::RiskBound {
                authority: match authority {
                    crate::economics::RiskBoundAuthority::VenueMaximum => {
                        AdmissionEconomicsRiskBoundAuthority::VenueMaximum
                    }
                    crate::economics::RiskBoundAuthority::VenueRateCapWithPriceStress => {
                        AdmissionEconomicsRiskBoundAuthority::VenueRateCapWithPriceStress
                    }
                    crate::economics::RiskBoundAuthority::OperatorRiskLimit => {
                        AdmissionEconomicsRiskBoundAuthority::OperatorRiskLimit
                    }
                },
            }
        }
        crate::economics::AdmissionTreatment::ForecastOnly => {
            AdmissionEconomicsTreatment::ForecastOnly
        }
    }
}

pub fn build_submit_admission_request_from_economics(
    input: BoltV3SubmitAdmissionRequestInput<'_>,
    economics: EconomicsAdmission,
    required_remaining_margin_ns: u64,
) -> anyhow::Result<BoltV3EconomicsSubmitAdmission> {
    let facts = order_admission_facts(&input)?;
    let expected_purpose = match input.intent_kind {
        BoltV3SubmitIntentKind::Entry => EconomicsAdmissionPurpose::TradingEdge,
        BoltV3SubmitIntentKind::RiskReducingExit => EconomicsAdmissionPurpose::RiskReduction,
    };
    anyhow::ensure!(
        economics.purpose() == expected_purpose,
        "economics admission purpose does not match the final order intent"
    );
    anyhow::ensure!(
        economics.order_binding()
            == &economics_order_binding(input.order).map_err(|error| anyhow::anyhow!(error))?,
        "economics admission does not bind the final order"
    );
    anyhow::ensure!(
        economics.request().execution_client_id.as_str() == input.execution_client_id,
        "economics admission execution client does not match the final order route"
    );
    anyhow::ensure!(
        economics.request().instrument_id.as_str() == input.order.instrument_id().to_string(),
        "economics admission instrument does not match the final order"
    );
    let expected_side = match input.order.order_side() {
        OrderSide::Buy => crate::economics::OrderSide::Buy,
        OrderSide::Sell => crate::economics::OrderSide::Sell,
        OrderSide::NoOrderSide => anyhow::bail!("economics admission requires a sided order"),
    };
    anyhow::ensure!(
        economics.request().order_side == expected_side,
        "economics admission side does not match the final order"
    );
    anyhow::ensure!(
        economics.reservation_basis() == facts.reservation_basis,
        "economics admission reservation basis does not match the final order"
    );
    let notional = economics.full_reservation_liability();
    let request = submit_admission_request_from_facts(input, facts, notional);
    anyhow::ensure!(
        required_remaining_margin_ns > 0,
        "economics submit admission requires a positive remaining margin"
    );
    Ok(BoltV3EconomicsSubmitAdmission {
        request,
        economics,
        required_remaining_margin_ns,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoltV3OrderAdmissionFacts {
    pub price: Decimal,
    pub order_quantity: Decimal,
    pub reservation_basis: Decimal,
}

pub(crate) fn order_admission_facts(
    input: &BoltV3SubmitAdmissionRequestInput<'_>,
) -> anyhow::Result<BoltV3OrderAdmissionFacts> {
    let client_order_id = input.order.client_order_id().to_string();
    let unsided_quote_quantity_market_style = input.order.is_quote_quantity()
        && matches!(
            input.order,
            OrderAny::Market(_) | OrderAny::MarketToLimit(_)
        )
        && !matches!(input.order.order_side(), OrderSide::Buy | OrderSide::Sell);
    anyhow::ensure!(
        !unsided_quote_quantity_market_style,
        "bolt-v3 submit admission requires an explicit buy or sell side for quote-quantity market-style client_order_id={}",
        client_order_id
    );
    let quantity_source = input.order.quantity().to_string();
    let quantity = Decimal::from_str(quantity_source.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission quantity is not a decimal for client_order_id={}",
            client_order_id
        )
    })?;
    let price_source = compiled_order_price_source(input.intent.price.clone(), input.order);
    let price = Decimal::from_str(price_source.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission price is not a decimal for client_order_id={}",
            client_order_id
        )
    })?;
    let (quote_quantity_last_price, quote_quantity_reference_price) =
        input.valuation.prices_for_order(input.order);
    let notional = if input.order.is_quote_quantity() {
        let instrument = input.valuation.instrument.with_context(|| {
            format!(
                "bolt-v3 submit admission missing instrument context for quote-quantity client_order_id={}",
                client_order_id
            )
        })?;
        match admission_base_notional_from_order(
            input.order,
            instrument,
            price,
            quantity,
            quote_quantity_last_price,
            quote_quantity_reference_price,
        ) {
            Some(base_notional) => base_notional,
            None => {
                anyhow::ensure!(
                    !instrument.is_inverse(),
                    "bolt-v3 submit admission cannot value a quote-quantity order on an inverse instrument from the raw quote quantity (client_order_id={})",
                    client_order_id
                );
                quantity
            }
        }
    } else {
        base_quantity_admission_notional(price, quantity)
    };
    let reservation_basis = if input.order.price().is_none() && !input.order.is_quote_quantity() {
        let price_ceiling = input
            .valuation
            .instrument
            .and_then(|instrument| instrument.max_price())
            .map(|ceiling| ceiling.as_decimal());
        market_style_admission_ceiling_notional(price_ceiling, quantity).with_context(|| {
            format!(
                "bolt-v3 submit admission refuses a market-style order without a structural price ceiling for client_order_id={}",
                client_order_id
            )
        })?
    } else {
        notional
    };

    Ok(BoltV3OrderAdmissionFacts {
        price,
        order_quantity: quantity,
        reservation_basis,
    })
}

#[cfg(test)]
pub(crate) fn build_submit_admission_request_from_order_for_test(
    input: BoltV3SubmitAdmissionRequestInput<'_>,
) -> anyhow::Result<BoltV3SubmitAdmissionRequest> {
    let facts = order_admission_facts(&input)?;
    Ok(submit_admission_request_from_facts(
        input,
        facts,
        facts.reservation_basis,
    ))
}

fn submit_admission_request_from_facts(
    input: BoltV3SubmitAdmissionRequestInput<'_>,
    facts: BoltV3OrderAdmissionFacts,
    notional: Decimal,
) -> BoltV3SubmitAdmissionRequest {
    let intent_kind = input.intent_kind;
    let risk_reducing_exit_proof =
        if matches!(intent_kind, BoltV3SubmitIntentKind::RiskReducingExit) {
            input
                .risk_reducing_exit_position
                .map(|position| BoltV3RiskReducingExitProof {
                    position_id: position.position_id.to_string(),
                    instrument_id: position.instrument_id.to_string(),
                    position_side: position.position_side,
                    exit_order_side: input.order.order_side(),
                    position_quantity: position.position_quantity,
                    exit_quantity: facts.order_quantity,
                })
        } else {
            None
        };

    BoltV3SubmitAdmissionRequest {
        strategy_id: input.intent.strategy_id.clone(),
        execution_client_id: input.execution_client_id.to_string(),
        client_order_id: input.order.client_order_id().to_string(),
        instrument_id: input.order.instrument_id().to_string(),
        notional,
        order_side: input.order.order_side(),
        order_quantity: facts.order_quantity,
        intent_kind,
        risk_reducing_exit_proof,
        admission_evidence: None,
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3QuoteQuantityAdmissionInput {
    pub order_side: BoltV3QuoteQuantityOrderSide,
    pub is_quote_quantity: bool,
    pub is_inverse: bool,
    pub submitted_quote_quantity: Decimal,
    pub calculated_notional: Decimal,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3QuoteQuantityOrderSide {
    Buy,
    Sell,
    Other,
}

/// Floor a quote-quantity order's admission notional at the submitted quote
/// quantity. A non-inverse quote-quantity order commits exactly its submitted
/// quote quantity in settlement currency — on either side and for ANY order kind
/// (Limit, StopLimit, Market, …) — so the per-order cap must never be checked
/// against a smaller value.
///
/// When the venue rounds the derived base quantity DOWN to size precision, NT's
/// effective notional can land a sub-tick BELOW the committed quote quantity. The
/// floor is therefore applied to every non-inverse quote-quantity Buy/Sell order
/// regardless of kind — restricting it (to one side, or to Limit/StopLimit) would
/// leave the excluded shapes' safety dependent on a precision coincidence rather
/// than a structural guarantee. The conservative effective-price pull that feeds
/// `calculated_notional` is a separate, Limit/StopLimit-only concern handled in
/// [`quote_quantity_effective_price`]; this floor is the kind-independent
/// backstop. Inverse instruments do not denominate the quote quantity in
/// settlement currency, so the floor is skipped for them.
pub fn conservative_quote_quantity_admission_notional(
    input: BoltV3QuoteQuantityAdmissionInput,
) -> Decimal {
    if input.is_quote_quantity
        && !input.is_inverse
        && matches!(
            input.order_side,
            BoltV3QuoteQuantityOrderSide::Buy | BoltV3QuoteQuantityOrderSide::Sell
        )
    {
        return input
            .calculated_notional
            .max(input.submitted_quote_quantity);
    }
    input.calculated_notional
}

/// Admission base notional for a BASE-quantity order: the product of the
/// already-rounded order's price and quantity. This is the single definition of
/// the base-quantity notional; every submit path (and the base-only test helper)
/// derives it from here so there is no divergent `price * quantity` copy.
pub fn base_quantity_admission_notional(order_price: Decimal, order_quantity: Decimal) -> Decimal {
    order_price * order_quantity
}

/// Single source of truth for "given a built venue-precision order, its
/// instrument, and a conservative reference/last price, what is the conservative
/// admission BASE notional?"
///
/// Both submit paths derive their admission notional from a built order through
/// THIS function so base-quantity and quote-quantity orders are sized
/// identically everywhere. Divergent per-call-site notional math is forbidden
/// (NO DUAL PATHS): a price*quantity shortcut UNDERSTATES the real cash debit of
/// a quote-quantity (quote-currency-denominated) order, understating the
/// per-order cap.
///
/// Contract for the inputs:
/// - `order_price` / `order_quantity` are the Decimal price and quantity of the
///   already-rounded order actually handed to the venue. For a BASE-quantity
///   order the result is exactly `order_price * order_quantity`, unchanged from
///   the historical per-call-site computation.
/// - `last_price` is the conservative reference/last price used to value a
///   quote-quantity order. For a quote-quantity order it MUST be `Some`; when a
///   caller cannot resolve a reference price it passes `None` and this function
///   returns `None` so the caller can apply its own degraded fallback. It is
///   ignored for base-quantity orders.
/// - `quote_reference_price` is the side-appropriate top-of-book price (best ask
///   for a BUY, best bid for a SELL) used to pick a conservative effective price
///   for the quote→base conversion. `None` means no top-of-book is available, in
///   which case the effective price is the `last_price` (matching the historical
///   no-quote-tick fallback).
///
/// For a quote-quantity, non-inverse Limit/StopLimit order the effective price is
/// pulled toward the book (`min(last, ask)` for a BUY, `max(last, bid)` for a
/// SELL) so the quote→base conversion yields the LARGEST base quantity the order
/// could fill — the conservative direction that never understates the notional.
/// The result is then floored by [`conservative_quote_quantity_admission_notional`].
pub fn admission_base_notional_from_order(
    order: &OrderAny,
    instrument: &InstrumentAny,
    order_price: Decimal,
    order_quantity: Decimal,
    last_price: Option<Price>,
    quote_reference_price: Option<Price>,
) -> Option<Decimal> {
    if !order.is_quote_quantity() {
        return Some(base_quantity_admission_notional(
            order_price,
            order_quantity,
        ));
    }
    // Fail CLOSED on an inverse quote-quantity order at the SHARED admission
    // helper (A6). An inverse instrument denominates the quote quantity in the
    // QUOTE currency, not the settlement currency, so neither
    // `calculate_notional_value` here nor the submitted-quote-quantity floor in
    // [`conservative_quote_quantity_admission_notional`] yields a settlement-
    // currency notional the per-order cap can be checked against — both would
    // UNDERSTATE the real cash debit. This is the single, structural rejection
    // point: returning `None` makes the production strategy path treat an
    // inverse quote-quantity order as unvaluable and refuse it, rather than
    // relying on a per-caller fallback to notice the
    // inverse case. This system trades only non-inverse binary options; carrying
    // currency-aware settlement notional would be the alternative, but the
    // fail-closed reject is the conservative default until such an instrument is
    // intentionally supported. Reachable only if an inverse instrument enters the
    // universe (the market-family filters gate it out today), but the defense
    // lives here so the cap can never be silently understated.
    if instrument.is_inverse() {
        return None;
    }
    let last_px = last_price?;
    let effective_price =
        quote_quantity_effective_price(order, instrument, last_px, quote_reference_price);
    let effective_quantity = instrument.calculate_base_quantity(order.quantity(), effective_price);
    let calculated_notional = instrument
        .calculate_notional_value(effective_quantity, last_px, Some(true))
        .as_decimal();
    let submitted_quote_quantity = Decimal::from_str(order.quantity().to_string().trim()).ok()?;
    Some(conservative_quote_quantity_admission_notional(
        BoltV3QuoteQuantityAdmissionInput {
            order_side: match order.order_side() {
                OrderSide::Buy => BoltV3QuoteQuantityOrderSide::Buy,
                OrderSide::Sell => BoltV3QuoteQuantityOrderSide::Sell,
                _ => BoltV3QuoteQuantityOrderSide::Other,
            },
            is_quote_quantity: order.is_quote_quantity(),
            is_inverse: instrument.is_inverse(),
            submitted_quote_quantity,
            calculated_notional,
        },
    ))
}

/// Conservative effective price for the quote→base conversion of a
/// quote-quantity order. Mirrors the production cache-driven selection: for a
/// non-inverse Limit/StopLimit order it pulls the price toward the book
/// (`min(last, ask)` for a BUY, `max(last, bid)` for a SELL) so a smaller
/// effective price yields a larger base quantity — the conservative direction.
/// Every other shape, or a missing top-of-book, falls back to `last_price`.
fn quote_quantity_effective_price(
    order: &OrderAny,
    instrument: &InstrumentAny,
    last_price: Price,
    quote_reference_price: Option<Price>,
) -> Price {
    if !order.is_quote_quantity()
        || instrument.is_inverse()
        || !matches!(order, OrderAny::Limit(_) | OrderAny::StopLimit(_))
    {
        return last_price;
    }
    let Some(quote_reference_price) = quote_reference_price else {
        return last_price;
    };
    match order.order_side() {
        OrderSide::Buy => last_price.min(quote_reference_price),
        OrderSide::Sell => last_price.max(quote_reference_price),
        _ => last_price,
    }
}

pub(crate) fn limit_notional_exceeds_sized_notional(
    limit_notional: f64,
    sized_notional: f64,
) -> bool {
    if !is_positive_finite(limit_notional) || !is_positive_finite(sized_notional) {
        return true;
    }
    limit_notional > sized_notional + notional_float_tolerance(sized_notional)
}

/// Admission notional for a market-style order — one with NO firm limit price
/// (Market / StopMarket / MarketIfTouched / TrailingStopMarket). Such an order
/// carries no venue-enforced price bound: it can fill anywhere up to the
/// instrument's structural price ceiling. The per-order cap must therefore be
/// checked against that ceiling — the only price the venue physically cannot
/// exceed — never against a reference-price estimate or a configured slippage
/// budget (an estimate is not a bound). Fails CLOSED when the instrument
/// declares no ceiling: an order whose worst-case cash cost cannot be bounded
/// must not be admitted.
pub fn market_style_admission_ceiling_notional(
    price_ceiling: Option<Decimal>,
    order_quantity: Decimal,
) -> Result<Decimal, BoltV3SubmitAdmissionError> {
    let ceiling = price_ceiling.ok_or(BoltV3SubmitAdmissionError::MissingPriceCeiling)?;
    Ok(base_quantity_admission_notional(ceiling, order_quantity))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CapitalAdmissionRejectReason {
    MissingAdmissionEvidence,
    VenueMismatch,
    AccountMismatch,
    ProductKindMismatch,
    CollateralCurrencyMismatch,
    UnsupportedProductKind,
    MissingPredictionMarketOutcome,
    OutcomeInstrumentMismatch,
    DuplicateClientOrderId,
    OrderShapeMismatch,
    MissingNtState,
    StaleNtState,
    UnattributedNtState,
    ReconciliationRequired,
    OverBudget,
    Reservation(ReservationRejectionReason),
    Liability(LiabilityError),
    Loss(LossHaltReason),
    AcceptedQuantityMismatch,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionInvariant {
    AdmittedSubmitMissingEvidenceAuthority,
    AcceptedCapitalAdmissionRebuildHasRejectionReason,
    ContradictoryAdmissionDecisionContext,
    ExpectedRejectedOutcome,
    InvalidCapitalAdmissionDecisionContext,
    KillSwitchForcedReductionReachedNormalAdmission,
    MissingAdmittedCounterUpdate,
    MissingCapitalAdmissionRebuildRejectionReason,
    MissingCapitalAdmissionRejectionReason,
    MissingLossHaltReasons,
    MissingStaleLossReason,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    KillSwitchLatched {
        state: KillSwitchStateKind,
    },
    LossGovernorHalted {
        reasons: Vec<LossHaltReason>,
    },
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
    ClientOrderAlreadyAuthorized,
    MissingPriceCeiling,
    ExchangeMutationCountOverflow,
    ExchangeMutationsObserved {
        mutation_count: u64,
    },
    InvalidRiskReducingExitProof,
    EconomicsOrderMismatch,
    EconomicsRemainingMarginUnavailable {
        requested_at_ns: u64,
        valid_until_ns: u64,
        observed_at_ns: u64,
        required_margin_ns: u64,
    },
    EconomicsUnavailable(EconomicsError),
    CapitalAdmissionRejected {
        reason: BoltV3CapitalAdmissionRejectReason,
    },
    InvariantViolation {
        invariant: BoltV3SubmitAdmissionInvariant,
    },
    EconomicsSealRejected,
    EvidenceWriteFailed {
        reason: String,
    },
    SystemClock {
        reason: String,
    },
}

impl std::fmt::Display for BoltV3SubmitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KillSwitchLatched { state } => write!(
                f,
                "bolt-v3 submit admission is blocked by kill-switch state {state:?}"
            ),
            Self::LossGovernorHalted { reasons } => {
                let reasons = reasons
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                write!(
                    f,
                    "bolt-v3 submit admission loss governor halted: {reasons}"
                )
            }
            Self::CountCapExhausted => {
                write!(f, "bolt-v3 submit admission order count cap is exhausted")
            }
            Self::NonPositiveNotional => {
                write!(f, "bolt-v3 submit admission notional must be positive")
            }
            Self::NotionalCapExceeded => {
                write!(f, "bolt-v3 submit admission notional cap is exceeded")
            }
            Self::ClientOrderAlreadyAuthorized => write!(
                f,
                "bolt-v3 submit admission client-order identity is already durably authorized"
            ),
            Self::MissingPriceCeiling => write!(
                f,
                "bolt-v3 submit admission refuses a market-style order without a declared instrument price ceiling"
            ),
            Self::ExchangeMutationCountOverflow => {
                write!(
                    f,
                    "bolt-v3 strategy-free exchange mutation counter overflowed"
                )
            }
            Self::ExchangeMutationsObserved { mutation_count } => write!(
                f,
                "bolt-v3 strategy-free exchange mutation guard observed {mutation_count} mutating request(s)"
            ),
            Self::InvalidRiskReducingExitProof => write!(
                f,
                "bolt-v3 submit admission risk-reducing exit proof is invalid"
            ),
            Self::EconomicsOrderMismatch => write!(
                f,
                "bolt-v3 submit admission final order does not match its economics authority"
            ),
            Self::EconomicsRemainingMarginUnavailable {
                requested_at_ns,
                valid_until_ns,
                observed_at_ns,
                required_margin_ns,
            } => write!(
                f,
                "bolt-v3 submit admission economics lacks remaining lifetime: requested_at_ns={requested_at_ns} valid_until_ns={valid_until_ns} observed_at_ns={observed_at_ns} required_margin_ns={required_margin_ns}"
            ),
            Self::EconomicsUnavailable(error) => {
                write!(
                    f,
                    "bolt-v3 submit admission economics is unavailable: {error}"
                )
            }
            Self::CapitalAdmissionRejected { reason } => {
                write!(
                    f,
                    "bolt-v3 submit admission capital admission rejected: {reason:?}"
                )
            }
            Self::InvariantViolation { invariant } => write!(
                f,
                "bolt-v3 submit admission invariant violated: {invariant:?}"
            ),
            Self::EconomicsSealRejected => {
                write!(f, "bolt-v3 submit admission economics seal was rejected")
            }
            Self::EvidenceWriteFailed { reason } => {
                write!(
                    f,
                    "bolt-v3 submit admission failed to record decision evidence: {reason}"
                )
            }
            Self::SystemClock { reason } => {
                write!(f, "bolt-v3 submit admission system clock error: {reason}")
            }
        }
    }
}

fn evaluate_capital_admission_submit(
    inner: &mut BoltV3SubmitAdmissionInner,
    request: &BoltV3SubmitAdmissionRequest,
    now_ns: u64,
) -> BoltV3CapitalAdmissionSubmitDecision {
    let BoltV3SubmitAdmissionInner {
        capital_admission_state_revision,
        capital_admission,
        ..
    } = inner;
    let Some(capital_admission) = capital_admission.as_mut() else {
        return accepted_without_reservation();
    };
    let Some(evidence) = request.admission_evidence.as_ref() else {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::MissingAdmissionEvidence,
        );
    };
    if evidence.venue_id != capital_admission.venue_id {
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::VenueMismatch);
    }
    let product_kind = evidence.product_kind.to_capital_admission();
    if product_kind != capital_admission.product_kind {
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::ProductKindMismatch);
    }
    if product_kind != ProductKind::PredictionMarketBinary {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::UnsupportedProductKind,
        );
    }
    if !compiled_order_side_matches_request(evidence.side, request.order_side)
        || evidence.quantity != request.order_quantity
    {
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::OrderShapeMismatch);
    }
    if capital_admission
        .client_order_reservations
        .contains_key(&request.client_order_id)
    {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::DuplicateClientOrderId,
        );
    }
    if capital_admission
        .provider_collateral_allowance_capture_failure_source
        .is_some()
    {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::ReconciliationRequired,
        );
    }
    let Some(state) = capital_admission.state.as_ref() else {
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::MissingNtState);
    };
    if state.portfolio.venue_id != capital_admission.venue_id {
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::VenueMismatch);
    }
    if state.portfolio.account_id != capital_admission.account_id {
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::AccountMismatch);
    }
    if state.portfolio.collateral_currency != capital_admission.collateral_currency {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::CollateralCurrencyMismatch,
        );
    }
    if !capital_admission.gate.is_reconciled() {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::ReconciliationRequired,
        );
    }
    let ProductAdmissionSnapshot::PredictionMarketBinary(product) = &state.product_state;
    let Some(outcome) = evidence.prediction_market_outcome else {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::MissingPredictionMarketOutcome,
        );
    };
    let outcome_position = match outcome {
        PredictionMarketOutcomeSide::Yes => {
            if request.instrument_id != product.yes_instrument_id {
                return rejected_capital_admission(
                    BoltV3CapitalAdmissionRejectReason::OutcomeInstrumentMismatch,
                );
            }
            product.yes_position
        }
        PredictionMarketOutcomeSide::No => {
            if request.instrument_id != product.no_instrument_id {
                return rejected_capital_admission(
                    BoltV3CapitalAdmissionRejectReason::OutcomeInstrumentMismatch,
                );
            }
            product.no_position
        }
    };

    if request.intent_kind == BoltV3SubmitIntentKind::RiskReducingExit {
        if evidence.side == BoltV3CompiledOrderSide::Sell && evidence.quantity <= outcome_position {
            return accepted_without_reservation();
        }
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::Liability(
            LiabilityError::InsufficientInventory,
        ));
    }

    let Some(reservation_revision) = capital_admission_state_revision.checked_add(1) else {
        capital_admission.gate.invalidate_reconciliation();
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::ReconciliationRequired,
        );
    };
    capital_admission.next_sequence += 1;
    let submit_reservation_id = format!(
        "{}#{}",
        request.client_order_id, capital_admission.next_sequence
    );
    let admission_request = CapitalAdmissionRequest {
        intent_id: submit_reservation_id.clone(),
        strategy_id: request.strategy_id.clone(),
        instrument_id: request.instrument_id.clone(),
        pool_id: capital_admission.capital_pool.pool_id.clone(),
        product_kind,
        side: evidence.side.to_capital_admission(),
        quantity: evidence.quantity,
        limit_price: evidence.effective_price,
        order_kind: evidence.order_kind.to_capital_admission(),
        liquidity: evidence.liquidity.to_capital_admission(),
        quote_set_id: evidence.quote_set_id.clone(),
        now_ns,
    };
    let decision = capital_admission
        .gate
        .evaluate_and_reserve(CapitalAdmissionGateInputs {
            request: &admission_request,
            state: Some(state),
            policy: &capital_admission.policy,
            loss_policy: None,
            capital_pool: &capital_admission.capital_pool,
        });
    let decision = match validate_capital_admission_decision(&decision) {
        Ok(decision) => decision,
        Err(invariant) => {
            capital_admission.gate.rollback_uncommitted_reservation(
                &capital_admission.capital_pool.pool_id,
                &submit_reservation_id,
            );
            return BoltV3CapitalAdmissionSubmitDecision::Invariant { invariant };
        }
    };
    let (admitted_quantity, reserved_liability) = match decision {
        ValidatedCapitalAdmissionDecision::Accepted {
            accepted_quantity,
            reserved_liability,
        } => (accepted_quantity, reserved_liability),
        ValidatedCapitalAdmissionDecision::Rejected { reason } => {
            return rejected_capital_admission(reason);
        }
    };
    if admitted_quantity != evidence.quantity {
        capital_admission.gate.rollback_uncommitted_reservation(
            &capital_admission.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::AcceptedQuantityMismatch,
        );
    }
    let Some(additive_liability) = checked_additive_liability(&capital_admission.policy) else {
        capital_admission.gate.rollback_uncommitted_reservation(
            &capital_admission.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::Liability(
            LiabilityError::ArithmeticOverflow,
        ));
    };
    let liability_factor = match evidence.side.to_capital_admission() {
        IntentSide::Buy => evidence.effective_price,
        IntentSide::Sell => Decimal::ZERO,
    };
    let reservation_attribution = ReservationAttribution {
        client_order_id: request.client_order_id.clone(),
        submit_reservation_id: submit_reservation_id.clone(),
        venue_id: capital_admission.venue_id.clone(),
        account_id: capital_admission.account_id.clone(),
        product_kind: reservation_product_kind(evidence.product_kind),
        collateral_currency: capital_admission.collateral_currency.clone(),
        capital_pool_id: capital_admission.capital_pool.pool_id.clone(),
        collateral_group_id: product.collateral_coupled_group_id.clone(),
        instrument_id: request.instrument_id.clone(),
        side: evidence_order_side(evidence.side),
        submitted_quantity: admitted_quantity.to_string(),
        liability_factor: liability_factor.to_string(),
        additive_liability: additive_liability.to_string(),
        reserved_liability: reserved_liability.to_string(),
        observed_at_ns: now_ns,
    };
    capital_admission.client_order_reservations.insert(
        request.client_order_id.clone(),
        BoltV3SubmitReservationRecord {
            attribution: reservation_attribution.clone(),
            phase: BoltV3SubmitReservationPhase::Reserved,
            phase_revision: reservation_revision,
            unresolved_in_projection: false,
            fill_metadata: Some(BoltV3SubmitReservationFillMetadata {
                instrument_id: request.instrument_id.clone(),
                side: evidence.side,
            }),
        },
    );
    *capital_admission_state_revision = reservation_revision;
    BoltV3CapitalAdmissionSubmitDecision::Accepted(Box::new(BoltV3CapitalAdmissionAcceptance {
        rollback: Some(BoltV3CapitalAdmissionReservationRollback {
            client_order_id: request.client_order_id.clone(),
            submit_reservation_id,
            pool_id: capital_admission.capital_pool.pool_id.clone(),
            observed_at_ns: now_ns,
        }),
        reservation_attribution: Some(reservation_attribution),
    }))
}

fn compiled_order_side_matches_request(
    evidence_side: BoltV3CompiledOrderSide,
    request_side: OrderSide,
) -> bool {
    matches!(
        (evidence_side, request_side),
        (BoltV3CompiledOrderSide::Buy, OrderSide::Buy)
            | (BoltV3CompiledOrderSide::Sell, OrderSide::Sell)
    )
}

fn checked_additive_liability(policy: &CapitalAdmissionPolicy) -> Option<Decimal> {
    match policy.fee_slippage_policy.as_ref() {
        Some(policy) => policy
            .max_fee_liability
            .checked_add(policy.max_slippage_liability),
        None => Some(Decimal::ZERO),
    }
}

fn checked_lifecycle_liability(
    quantity: Decimal,
    liability_factor: Decimal,
    additive_liability: Decimal,
) -> Option<Decimal> {
    quantity
        .checked_mul(liability_factor)?
        .checked_add(additive_liability)
}

fn refresh_capital_admission_state_from_components(
    capital_admission: &mut BoltV3SubmitCapitalAdmissionState,
    components: BoltV3SubmitCapitalAdmissionNtComponents,
) {
    if capital_admission.fill_evidence_integrity_failed {
        capital_admission.gate.invalidate_reconciliation();
    }
    if capital_admission.gate.is_reconciled()
        && components.order_lifecycle.open_order_count > 0
        && !components.order_lifecycle.all_open_orders_attributed
    {
        capital_admission.gate.invalidate_reconciliation();
    }
    let state = compose_capital_admission_state_from_components(
        components,
        capital_admission.gate.is_reconciled()
            && capital_admission
                .provider_collateral_allowance_capture_failure_source
                .is_none(),
        capital_admission.latest_reservation_mutation_observed_at_ns,
    );
    capital_admission.capital_pool.source = state.portfolio.source.clone();
    capital_admission.capital_pool.observed_at_ns = state.portfolio.observed_at_ns;
    capital_admission.state = Some(state);
}

fn fail_capital_admission_fill_evidence_integrity(
    capital_admission: &mut BoltV3SubmitCapitalAdmissionState,
    observed_at_ns: u64,
) {
    capital_admission.fill_evidence_integrity_failed = true;
    capital_admission.gate.invalidate_reconciliation();
    refresh_capital_admission_reservation_snapshot(capital_admission, observed_at_ns);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapitalAdmissionStateRevisionExhausted;

fn advance_capital_admission_state_revision(
    revision: &mut u64,
) -> Result<(), CapitalAdmissionStateRevisionExhausted> {
    let next = revision
        .checked_add(1)
        .ok_or(CapitalAdmissionStateRevisionExhausted)?;
    *revision = next;
    Ok(())
}

fn reject_capital_admission_fill_evidence_integrity(
    capital_admission: &mut BoltV3SubmitCapitalAdmissionState,
    revision: &mut u64,
    observed_at_ns: u64,
) -> BoltV3SubmitReservationFillEvidenceDecision {
    match advance_capital_admission_state_revision(revision) {
        Ok(()) => fail_capital_admission_fill_evidence_integrity(capital_admission, observed_at_ns),
        Err(_) => {
            capital_admission.gate.invalidate_reconciliation();
            log::error!("capital admission state revision exhausted while rejecting fill evidence");
        }
    }
    BoltV3SubmitReservationFillEvidenceDecision::unknown()
}

fn unresolved_reservation_counts(
    records: &BTreeMap<String, BoltV3SubmitReservationRecord>,
) -> (usize, usize) {
    let sink_invoked = records
        .values()
        .filter(|record| {
            record.unresolved_in_projection
                && matches!(record.phase, BoltV3SubmitReservationPhase::SinkInvoked)
        })
        .count();
    let observed_open = records
        .values()
        .filter(|record| {
            record.unresolved_in_projection
                && matches!(record.phase, BoltV3SubmitReservationPhase::ObservedOpen)
        })
        .count();
    (sink_invoked, observed_open)
}

fn apply_rebuild_order_lifecycle(
    state: &mut NtDerivedCapitalAdmissionState,
    rebuilt: OrderLifecycleCapitalAdmissionSnapshot,
) {
    state.observed_at_ns = state.observed_at_ns.max(rebuilt.observed_at_ns);
    state.order_lifecycle = rebuilt;
}

fn refresh_capital_admission_reservation_snapshot(
    capital_admission: &mut BoltV3SubmitCapitalAdmissionState,
    observed_at_ns: u64,
) {
    refresh_capital_admission_reservation_snapshot_with_source(
        capital_admission,
        observed_at_ns,
        "bolt_reservation_ledger".to_string(),
        capital_admission.gate.is_reconciled(),
    );
}

fn refresh_capital_admission_reservation_snapshot_with_source(
    capital_admission: &mut BoltV3SubmitCapitalAdmissionState,
    observed_at_ns: u64,
    source: String,
    all_live_reservations_attributed: bool,
) {
    capital_admission.latest_reservation_mutation_observed_at_ns = Some(observed_at_ns);
    let Some(current_state) = capital_admission.state.take() else {
        return;
    };
    let components = nt_components_from_existing_capital_admission_state(current_state);
    let mut state = compose_capital_admission_state_from_components(
        components,
        all_live_reservations_attributed,
        capital_admission.latest_reservation_mutation_observed_at_ns,
    );
    state.reservation_snapshot.source = source;
    state.reservation_snapshot.observed_at_ns = observed_at_ns;
    state.reservation_snapshot.all_live_reservations_attributed = all_live_reservations_attributed;
    state.observed_at_ns = state.observed_at_ns.max(observed_at_ns);
    capital_admission.state = Some(state);
}

fn nt_components_from_existing_capital_admission_state(
    state: NtDerivedCapitalAdmissionState,
) -> BoltV3SubmitCapitalAdmissionNtComponents {
    let product_observed_at_ns = match &state.product_state {
        ProductAdmissionSnapshot::PredictionMarketBinary(snapshot) => snapshot.observed_at_ns,
    };
    let mut observed_at_ns = state
        .portfolio
        .observed_at_ns
        .max(state.provider_collateral_allowance.observed_at_ns)
        .max(state.order_lifecycle.observed_at_ns)
        .max(product_observed_at_ns);
    if let Some(loss_snapshot) = state.loss_snapshot.as_ref() {
        observed_at_ns = observed_at_ns.max(loss_snapshot.observed_at_ns);
    }
    BoltV3SubmitCapitalAdmissionNtComponents {
        source: state.source,
        observed_at_ns,
        portfolio: state.portfolio,
        provider_collateral_allowance: state.provider_collateral_allowance,
        order_lifecycle: state.order_lifecycle,
        product_state: state.product_state,
        loss_snapshot: state.loss_snapshot,
    }
}

fn rebuilt_open_order_reservation_attribution_valid(
    reservation: &BoltV3SubmitCapitalAdmissionOpenOrderReservation,
) -> bool {
    if reservation.client_order_id != reservation.attribution.client_order_id
        || reservation.submit_reservation_id != reservation.attribution.submit_reservation_id
        || reservation.collateral_group_id != reservation.attribution.collateral_group_id
        || reservation.instrument_id != reservation.attribution.instrument_id
        || evidence_order_side(reservation.side) != reservation.attribution.side
        || reservation.instrument_id.trim().is_empty()
        || reservation.open_quantity <= Decimal::ZERO
        || reservation.original_quantity <= Decimal::ZERO
        || reservation.filled_quantity < Decimal::ZERO
        || reservation.filled_quantity > reservation.original_quantity
        || reservation.open_quantity > reservation.original_quantity
        || reservation.liability_factor < Decimal::ZERO
        || reservation.liability_factor > Decimal::ONE
        || reservation.additive_liability < Decimal::ZERO
    {
        return false;
    }
    if reservation
        .original_quantity
        .checked_sub(reservation.filled_quantity)
        != Some(reservation.open_quantity)
    {
        return false;
    }
    checked_lifecycle_liability(
        reservation.open_quantity,
        reservation.liability_factor,
        reservation.additive_liability,
    ) == Some(reservation.liability)
}

fn compose_capital_admission_state_from_components(
    components: BoltV3SubmitCapitalAdmissionNtComponents,
    gate_reconciled: bool,
    latest_reservation_mutation_observed_at_ns: Option<u64>,
) -> NtDerivedCapitalAdmissionState {
    let reservation_observed_at_ns = latest_reservation_mutation_observed_at_ns
        .map_or(components.observed_at_ns, |observed_at_ns| {
            components.observed_at_ns.max(observed_at_ns)
        });
    NtDerivedCapitalAdmissionState {
        source: components.source,
        observed_at_ns: components.observed_at_ns.max(reservation_observed_at_ns),
        portfolio: components.portfolio,
        provider_collateral_allowance: components.provider_collateral_allowance,
        order_lifecycle: components.order_lifecycle,
        product_state: components.product_state,
        reservation_snapshot: ReservationLedgerSnapshot {
            source: "bolt_reservation_ledger".to_string(),
            observed_at_ns: reservation_observed_at_ns,
            all_live_reservations_attributed: gate_reconciled,
        },
        loss_snapshot: components.loss_snapshot,
    }
}

fn accepted_without_reservation() -> BoltV3CapitalAdmissionSubmitDecision {
    BoltV3CapitalAdmissionSubmitDecision::Accepted(Box::new(BoltV3CapitalAdmissionAcceptance {
        rollback: None,
        reservation_attribution: None,
    }))
}

fn validate_capital_admission_decision(
    decision: &CapitalAdmissionDecision,
) -> Result<ValidatedCapitalAdmissionDecision, BoltV3SubmitAdmissionInvariant> {
    if decision.accepted {
        let (Some(accepted_quantity), Some(calculated_liability), Some(reserved_liability)) = (
            decision.accepted_quantity,
            decision.calculated_liability,
            decision.reserved_liability,
        ) else {
            return Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext);
        };
        if !decision.reasons.is_empty()
            || decision.evidence.original_quantity != decision.original_quantity
            || decision.evidence.accepted_quantity != Some(accepted_quantity)
            || decision.evidence.calculated_liability != Some(calculated_liability)
            || decision.evidence.reserved_liability != Some(reserved_liability)
        {
            return Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext);
        }
        return Ok(ValidatedCapitalAdmissionDecision::Accepted {
            accepted_quantity,
            reserved_liability,
        });
    }

    if decision.accepted_quantity.is_some()
        || decision.evidence.original_quantity != decision.original_quantity
    {
        return Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext);
    }
    let liability_context_is_consistent = match (
        decision.calculated_liability,
        decision.reserved_liability,
        decision.evidence.accepted_quantity,
        decision.evidence.calculated_liability,
        decision.evidence.reserved_liability,
    ) {
        (None, None, None, None, None) => true,
        (
            Some(calculated),
            Some(reserved),
            Some(_),
            Some(evidence_calculated),
            Some(evidence_reserved),
        ) => calculated == evidence_calculated && reserved == evidence_reserved,
        _ => false,
    };
    if !liability_context_is_consistent {
        return Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext);
    }

    Ok(ValidatedCapitalAdmissionDecision::Rejected {
        reason: map_capital_admission_rejection(&decision.reasons)?,
    })
}

fn rejected_capital_admission(
    reason: BoltV3CapitalAdmissionRejectReason,
) -> BoltV3CapitalAdmissionSubmitDecision {
    BoltV3CapitalAdmissionSubmitDecision::Rejected { reason }
}

fn rollback_capital_admission_reservation(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3CapitalAdmissionReservationRollback,
) {
    rollback_capital_admission_reservations(inner, std::slice::from_ref(rollback));
}

fn rollback_capital_admission_reservations(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollbacks: &[BoltV3CapitalAdmissionReservationRollback],
) -> bool {
    match prepare_capital_admission_reservation_rollback(inner, rollbacks) {
        Ok(prepared) => {
            commit_capital_admission_reservation_rollback(inner, prepared);
            true
        }
        Err(reason) => {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            log::error!(
                "bolt-v3 capital reservation rollback lost exact ownership; preserving every live reservation: reason={reason:?}"
            );
            false
        }
    }
}

#[derive(Debug)]
struct PreparedCapitalAdmissionReservationRollback {
    candidate_capital_admission: Option<BoltV3SubmitCapitalAdmissionState>,
    final_revision: u64,
}

fn prepare_capital_admission_reservation_rollback(
    inner: &BoltV3SubmitAdmissionInner,
    rollbacks: &[BoltV3CapitalAdmissionReservationRollback],
) -> Result<PreparedCapitalAdmissionReservationRollback, ReservationRejectionReason> {
    if rollbacks.is_empty() {
        return Ok(PreparedCapitalAdmissionReservationRollback {
            candidate_capital_admission: None,
            final_revision: inner.capital_admission_state_revision,
        });
    }

    let mut candidate_capital_admission = inner
        .capital_admission
        .clone()
        .ok_or(ReservationRejectionReason::UnknownReservation)?;
    let mut final_revision = inner.capital_admission_state_revision;
    let mut client_order_ids = BTreeSet::new();
    let mut reservation_ids = BTreeSet::new();
    let mut rollback_observed_at_ns = None;

    for rollback in rollbacks.iter().rev() {
        if !client_order_ids.insert(rollback.client_order_id.as_str())
            || !reservation_ids.insert((
                rollback.pool_id.as_str(),
                rollback.submit_reservation_id.as_str(),
            ))
        {
            return Err(ReservationRejectionReason::DuplicateReservation);
        }
        final_revision = final_revision
            .checked_add(1)
            .ok_or(ReservationRejectionReason::ReconciliationRequired)?;
        let record = candidate_capital_admission
            .client_order_reservations
            .get(&rollback.client_order_id)
            .ok_or(ReservationRejectionReason::UnknownReservation)?;
        if record.phase != BoltV3SubmitReservationPhase::Reserved
            || record.attribution.client_order_id != rollback.client_order_id
            || record.attribution.submit_reservation_id != rollback.submit_reservation_id
            || record.attribution.capital_pool_id != rollback.pool_id
        {
            return Err(ReservationRejectionReason::InvalidRequest);
        }
        candidate_capital_admission.gate.retained_reservation(
            &rollback.pool_id,
            &record.attribution.collateral_group_id,
            &rollback.submit_reservation_id,
        )?;
        candidate_capital_admission
            .gate
            .rollback_uncommitted_reservation(&rollback.pool_id, &rollback.submit_reservation_id)
            .ok_or(ReservationRejectionReason::UnknownReservation)?;
        candidate_capital_admission
            .client_order_reservations
            .remove(&rollback.client_order_id)
            .ok_or(ReservationRejectionReason::UnknownReservation)?;
        rollback_observed_at_ns = Some(
            rollback_observed_at_ns
                .unwrap_or(rollback.observed_at_ns)
                .max(rollback.observed_at_ns),
        );
    }

    candidate_capital_admission.unresolved_lifecycle_reservations =
        unresolved_reservation_counts(&candidate_capital_admission.client_order_reservations);
    if let Some(observed_at_ns) = rollback_observed_at_ns {
        refresh_capital_admission_reservation_snapshot(
            &mut candidate_capital_admission,
            observed_at_ns,
        );
    }
    Ok(PreparedCapitalAdmissionReservationRollback {
        candidate_capital_admission: Some(candidate_capital_admission),
        final_revision,
    })
}

fn commit_capital_admission_reservation_rollback(
    inner: &mut BoltV3SubmitAdmissionInner,
    prepared: PreparedCapitalAdmissionReservationRollback,
) {
    if let Some(candidate_capital_admission) = prepared.candidate_capital_admission {
        inner.capital_admission = Some(candidate_capital_admission);
    }
    inner.capital_admission_state_revision = prepared.final_revision;
}

fn rollback_unsubmitted_permit(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollbacks: &[BoltV3CapitalAdmissionReservationRollback],
    counter_rollback: Option<&BoltV3SubmitAdmissionCounterRollback>,
) {
    let counter_values = match counter_rollback {
        Some(rollback) => match checked_admission_counter_rollback(inner, rollback) {
            Some(values) => Some(values),
            None => return,
        },
        None => None,
    };
    let prepared_capital = match prepare_capital_admission_reservation_rollback(inner, rollbacks) {
        Ok(prepared) => prepared,
        Err(reason) => {
            if let Some(capital_admission) = inner.capital_admission.as_mut() {
                capital_admission.gate.invalidate_reconciliation();
            }
            log::error!(
                "bolt-v3 submit permit rollback lost exact capital ownership; preserving all remaining permit state: reason={reason:?}"
            );
            return;
        }
    };

    commit_capital_admission_reservation_rollback(inner, prepared_capital);
    if let (Some(rollback), Some(values)) = (counter_rollback, counter_values) {
        apply_admission_counter_rollback(inner, rollback, values);
    }
}

fn checked_admission_counter_rollback(
    inner: &BoltV3SubmitAdmissionInner,
    rollback: &BoltV3SubmitAdmissionCounterRollback,
) -> Option<BoltV3SubmitAdmissionCounterValues> {
    let Some(admitted_order_count) = inner.admitted_order_count.checked_sub(rollback.order_count)
    else {
        log::error!(
            "bolt-v3 submit admission counter rollback exceeded the admitted total; preserving the higher fail-closed count"
        );
        return None;
    };
    let Some(execution_client_order_count) = inner
        .admitted_order_count_by_execution_client
        .get(&rollback.execution_client_id)
        .and_then(|count| count.checked_sub(rollback.order_count))
    else {
        log::error!(
            "bolt-v3 submit admission counter rollback exceeded or found no execution-client total; preserving the higher fail-closed counts"
        );
        return None;
    };
    let mut transient_ids = BTreeSet::new();
    for client_order_id in &rollback.uncommitted_evidence_free_client_order_ids {
        if !transient_ids.insert(client_order_id.as_str())
            || !inner
                .evidence_free_non_reservation_admissions
                .get(client_order_id)
                .is_some_and(|phase| *phase == BoltV3EvidenceFreeNonReservationPhase::PermitHeld)
        {
            log::error!(
                "bolt-v3 submit admission counter rollback lost exact transient authority; preserving all fail-closed counts"
            );
            return None;
        }
    }
    Some(BoltV3SubmitAdmissionCounterValues {
        admitted_order_count,
        execution_client_order_count,
    })
}

fn apply_admission_counter_rollback(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3SubmitAdmissionCounterRollback,
    values: BoltV3SubmitAdmissionCounterValues,
) {
    inner.admitted_order_count = values.admitted_order_count;
    if values.execution_client_order_count == 0 {
        inner
            .admitted_order_count_by_execution_client
            .remove(&rollback.execution_client_id);
    } else {
        inner.admitted_order_count_by_execution_client.insert(
            rollback.execution_client_id.clone(),
            values.execution_client_order_count,
        );
    }
    for client_order_id in &rollback.uncommitted_evidence_free_client_order_ids {
        inner
            .evidence_free_non_reservation_admissions
            .remove(client_order_id);
    }
}

#[cfg(test)]
fn rollback_admission_counters(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3SubmitAdmissionCounterRollback,
) {
    if let Some(values) = checked_admission_counter_rollback(inner, rollback) {
        apply_admission_counter_rollback(inner, rollback, values);
    }
}

fn map_capital_admission_rejection(
    reasons: &[CapitalAdmissionReason],
) -> Result<BoltV3CapitalAdmissionRejectReason, BoltV3SubmitAdmissionInvariant> {
    match reasons {
        [] => Err(BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRejectionReason),
        [CapitalAdmissionReason::MissingNtState] => {
            Ok(BoltV3CapitalAdmissionRejectReason::MissingNtState)
        }
        [CapitalAdmissionReason::StaleNtState(_)] => {
            Ok(BoltV3CapitalAdmissionRejectReason::StaleNtState)
        }
        [CapitalAdmissionReason::UnattributedNtState(_)] => {
            Ok(BoltV3CapitalAdmissionRejectReason::UnattributedNtState)
        }
        [
            CapitalAdmissionReason::Reservation(ReservationRejectionReason::ReconciliationRequired),
        ] => Ok(BoltV3CapitalAdmissionRejectReason::ReconciliationRequired),
        [CapitalAdmissionReason::Reservation(ReservationRejectionReason::OverBudget)] => {
            Ok(BoltV3CapitalAdmissionRejectReason::OverBudget)
        }
        [CapitalAdmissionReason::Reservation(reason)] => {
            Ok(BoltV3CapitalAdmissionRejectReason::Reservation(*reason))
        }
        [CapitalAdmissionReason::Liability(reason)] => {
            Ok(BoltV3CapitalAdmissionRejectReason::Liability(*reason))
        }
        [CapitalAdmissionReason::Loss(reason)] => {
            Ok(BoltV3CapitalAdmissionRejectReason::Loss(*reason))
        }
        [CapitalAdmissionReason::Invariant(_)] | [_, _, ..] => {
            Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext)
        }
    }
}

fn current_unix_ns() -> Result<u64, BoltV3SubmitAdmissionError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| BoltV3SubmitAdmissionError::SystemClock {
            reason: format!("system time before UNIX_EPOCH: {source}"),
        })?
        .as_nanos();
    nanos
        .try_into()
        .map_err(|_| BoltV3SubmitAdmissionError::SystemClock {
            reason: format!("unix nanoseconds does not fit u64: {nanos}"),
        })
}

impl std::error::Error for BoltV3SubmitAdmissionError {}

#[cfg(test)]
mod fail_closed_invariant_tests {
    use super::*;

    #[test]
    fn missing_capital_rejection_reason_is_a_typed_invariant_error() {
        assert_eq!(
            capital_admission_rejection_error(None),
            BoltV3SubmitAdmissionError::InvariantViolation {
                invariant: BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRejectionReason,
            }
        );
        assert_eq!(
            capital_admission_rejection_error(
                Some(BoltV3CapitalAdmissionRejectReason::OverBudget,)
            ),
            BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
                reason: BoltV3CapitalAdmissionRejectReason::OverBudget,
            }
        );
    }

    #[test]
    fn capital_admission_submit_decisions_make_rejection_context_structural() {
        assert_eq!(
            accepted_without_reservation(),
            BoltV3CapitalAdmissionSubmitDecision::Accepted(Box::new(
                BoltV3CapitalAdmissionAcceptance {
                    rollback: None,
                    reservation_attribution: None,
                },
            ))
        );
        assert_eq!(
            rejected_capital_admission(BoltV3CapitalAdmissionRejectReason::OverBudget),
            BoltV3CapitalAdmissionSubmitDecision::Rejected {
                reason: BoltV3CapitalAdmissionRejectReason::OverBudget,
            }
        );
    }

    #[test]
    fn lower_capital_decision_context_is_validated_before_submit() {
        let evidence = crate::bolt_v3_capital_admission::CapitalAdmissionEvidence {
            sources: Vec::new(),
            original_quantity: Decimal::ONE,
            accepted_quantity: Some(Decimal::ONE),
            calculated_liability: Some(Decimal::ONE),
            reserved_liability: Some(Decimal::ONE),
        };
        let accepted = crate::bolt_v3_capital_admission::CapitalAdmissionDecision {
            accepted: true,
            original_quantity: Decimal::ONE,
            accepted_quantity: Some(Decimal::ONE),
            calculated_liability: Some(Decimal::ONE),
            reserved_liability: Some(Decimal::ONE),
            pool_id: "pool".to_string(),
            evidence,
            reasons: Vec::new(),
        };
        let mut invalid_accepted = accepted.clone();
        invalid_accepted.reserved_liability = None;

        assert_eq!(
            validate_capital_admission_decision(&invalid_accepted),
            Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext)
        );

        let mut liability_rejected = accepted.clone();
        liability_rejected.accepted = false;
        liability_rejected.accepted_quantity = None;
        liability_rejected.reasons = vec![
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::Liability(
                crate::bolt_v3_capital_admission::LiabilityError::InsufficientAllowance,
            ),
        ];
        assert_eq!(
            validate_capital_admission_decision(&liability_rejected),
            Ok(ValidatedCapitalAdmissionDecision::Rejected {
                reason: BoltV3CapitalAdmissionRejectReason::Liability(
                    crate::bolt_v3_capital_admission::LiabilityError::InsufficientAllowance,
                ),
            })
        );

        liability_rejected.reasons.push(
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::Reservation(
                ReservationRejectionReason::OverBudget,
            ),
        );
        assert_eq!(
            validate_capital_admission_decision(&liability_rejected),
            Err(BoltV3SubmitAdmissionInvariant::InvalidCapitalAdmissionDecisionContext)
        );
    }

    #[test]
    fn contradictory_capital_rebuild_results_are_typed_invariant_errors() {
        let decision = |accepted, reason| BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted,
            reason,
            attempted_reservation_count: 0,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
            missing_nt_account_cache_balance: None,
        };

        assert_eq!(
            capital_admission_rebuild_outcome(&decision(false, None)),
            Err(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant:
                    BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRebuildRejectionReason,
            })
        );
        assert_eq!(
            capital_admission_rebuild_outcome(&decision(
                true,
                Some(ReservationRejectionReason::OverBudget),
            )),
            Err(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant:
                    BoltV3SubmitAdmissionInvariant::AcceptedCapitalAdmissionRebuildHasRejectionReason,
            })
        );
    }

    #[test]
    fn rebuild_audit_boundary_propagates_the_typed_invariant() {
        let admission =
            BoltV3SubmitAdmissionState::new(Arc::new(DecisionEvidenceRecorder::recording()));
        let mut inner = admission
            .inner
            .lock()
            .expect("submit admission fixture mutex should not be poisoned");
        let result = admission.finish_capital_admission_rebuild(
            &mut inner,
            &BoltV3SubmitCapitalAdmissionRebuildAuditContext {
                observed_at_ns: 1,
                source: CapitalAdmissionRebuildSource::NtOpenOrderCache,
                observed_open_order_count: 0,
                all_open_orders_attributed: true,
                unresolved_sink_invoked_reservation_count: 0,
                unresolved_observed_open_reservation_count: 0,
            },
            1,
            BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: false,
                reason: None,
                attempted_reservation_count: 0,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            },
        );

        assert_eq!(
            result,
            Err(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant:
                    BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRebuildRejectionReason,
            })
        );
    }

    #[test]
    fn outcome_only_error_mapping_never_fabricates_missing_context() {
        for (outcome, invariant) in [
            (
                AdmissionDecisionOutcome::Admitted,
                BoltV3SubmitAdmissionInvariant::ExpectedRejectedOutcome,
            ),
            (
                AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted),
                BoltV3SubmitAdmissionInvariant::MissingLossHaltReasons,
            ),
            (
                AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::CapitalAdmission),
                BoltV3SubmitAdmissionInvariant::MissingCapitalAdmissionRejectionReason,
            ),
            (
                AdmissionDecisionOutcome::Rejected(
                    AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid,
                ),
                BoltV3SubmitAdmissionInvariant::KillSwitchForcedReductionReachedNormalAdmission,
            ),
        ] {
            assert_eq!(
                submit_admission_error_from_outcome(outcome, KillSwitchStateKind::Armed),
                BoltV3SubmitAdmissionError::InvariantViolation { invariant }
            );
        }
    }

    #[test]
    fn concrete_evaluation_boundary_rejects_missing_loss_context() {
        let admission =
            BoltV3SubmitAdmissionState::new(Arc::new(DecisionEvidenceRecorder::recording()));
        let inner = admission
            .inner
            .lock()
            .expect("submit admission fixture mutex should not be poisoned");
        let evaluation = BoltV3SubmitAdmissionEvaluation::without_loss_halt(
            AdmissionDecisionOutcome::Rejected(AdmissionRejectionReason::LossGovernorHalted),
            1,
        );

        assert_eq!(
            BoltV3SubmitAdmissionState::admission_result(&inner, &evaluation),
            Err(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant: BoltV3SubmitAdmissionInvariant::MissingLossHaltReasons,
            })
        );
    }

    #[test]
    fn contradictory_stale_loss_state_is_a_typed_invariant_error() {
        let policy = LossGovernorPolicy {
            max_snapshot_age_ns: 1_000,
            max_per_trade_loss: None,
            max_daily_loss: None,
            max_rolling_loss: None,
            max_drawdown: None,
        };
        let snapshot = LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 10_000,
            per_trade_pnl: None,
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        };

        assert_eq!(
            stale_loss_governor_halt_reason(&policy, Some(&snapshot), 10_001),
            Err(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant: BoltV3SubmitAdmissionInvariant::MissingStaleLossReason,
            })
        );
    }

    #[test]
    fn checked_counter_values_reject_each_overflow_boundary() {
        for (total, client) in [(u32::MAX, 0), (0, u32::MAX)] {
            assert_eq!(
                checked_admission_counter_values(total, client, 1),
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            );
        }

        assert_eq!(
            checked_admission_counter_values(u32::MAX - 1, u32::MAX - 1, 1),
            Ok(BoltV3SubmitAdmissionCounterValues {
                admitted_order_count: u32::MAX,
                execution_client_order_count: u32::MAX,
            })
        );
    }

    #[test]
    fn basket_claim_count_conversion_rejects_unrepresentable_lengths() {
        assert_eq!(checked_basket_claim_count(u32::MAX as usize), Ok(u32::MAX));
        if let Some(unrepresentable) = (u32::MAX as usize).checked_add(1) {
            assert_eq!(
                checked_basket_claim_count(unrepresentable),
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            );
        }
    }

    #[test]
    fn admitted_submit_without_authority_is_a_typed_invariant_error() {
        assert!(matches!(
            RecordedAdmissionAuthority::NoExternalActionAuthority.into_submit_authority(),
            Err(BoltV3SubmitAdmissionError::InvariantViolation {
                invariant: BoltV3SubmitAdmissionInvariant::AdmittedSubmitMissingEvidenceAuthority,
            })
        ));
    }

    #[test]
    fn open_order_authority_excludes_evidence_free_identity_until_sink_invocation() {
        let authority = |phase| BoltV3OpenOrderAdmissionAuthoritySnapshot {
            committed: ReservationRecoveryFacts::default(),
            evidence_free_non_reservation_admissions: BTreeMap::from([(
                "evidence-free-exit".to_string(),
                phase,
            )]),
        };

        assert!(
            !authority(BoltV3EvidenceFreeNonReservationPhase::PermitHeld)
                .authorizes_non_reservation_order("evidence-free-exit")
        );
        assert!(
            authority(BoltV3EvidenceFreeNonReservationPhase::SinkInvoked)
                .authorizes_non_reservation_order("evidence-free-exit")
        );
    }

    #[test]
    fn valid_counter_rollback_is_atomic_and_releases_transient_authority() {
        let admission =
            BoltV3SubmitAdmissionState::new(Arc::new(DecisionEvidenceRecorder::recording()));
        let mut inner = admission
            .inner
            .lock()
            .expect("submit admission fixture mutex should not be poisoned");
        inner.admitted_order_count = 1;
        inner
            .admitted_order_count_by_execution_client
            .insert("execution".to_string(), 1);
        inner.evidence_free_non_reservation_admissions.insert(
            "transient".to_string(),
            BoltV3EvidenceFreeNonReservationPhase::PermitHeld,
        );

        rollback_admission_counters(
            &mut inner,
            &BoltV3SubmitAdmissionCounterRollback {
                execution_client_id: "execution".to_string(),
                order_count: 1,
                uncommitted_evidence_free_client_order_ids: vec!["transient".to_string()],
            },
        );

        assert_eq!(inner.admitted_order_count, 0);
        assert_eq!(
            inner
                .admitted_order_count_by_execution_client
                .get("execution"),
            None
        );
        assert!(
            !inner
                .evidence_free_non_reservation_admissions
                .contains_key("transient")
        );
    }

    #[test]
    fn inconsistent_counter_rollback_preserves_all_fail_closed_state() {
        for (total, client_count) in [(0, Some(1)), (1, Some(0)), (1, None)] {
            let admission =
                BoltV3SubmitAdmissionState::new(Arc::new(DecisionEvidenceRecorder::recording()));
            let mut inner = admission
                .inner
                .lock()
                .expect("submit admission fixture mutex should not be poisoned");
            inner.admitted_order_count = total;
            if let Some(client_count) = client_count {
                inner
                    .admitted_order_count_by_execution_client
                    .insert("execution".to_string(), client_count);
            }
            inner.evidence_free_non_reservation_admissions.insert(
                "transient".to_string(),
                BoltV3EvidenceFreeNonReservationPhase::PermitHeld,
            );
            let before_client_counts = inner.admitted_order_count_by_execution_client.clone();

            rollback_admission_counters(
                &mut inner,
                &BoltV3SubmitAdmissionCounterRollback {
                    execution_client_id: "execution".to_string(),
                    order_count: 1,
                    uncommitted_evidence_free_client_order_ids: vec!["transient".to_string()],
                },
            );

            assert_eq!(inner.admitted_order_count, total);
            assert_eq!(
                inner.admitted_order_count_by_execution_client,
                before_client_counts
            );
            assert!(
                inner
                    .evidence_free_non_reservation_admissions
                    .contains_key("transient")
            );
        }
    }
}

#[cfg(test)]
mod notional_guard_tests {
    use crate::bolt_v3_numeric::{BPS_DENOMINATOR, MIDPOINT_DIVISOR_F64, notional_float_tolerance};

    #[test]
    fn limit_notional_guard_allows_scaled_float_noise() {
        let sized_notional = BPS_DENOMINATOR;
        let tolerance = notional_float_tolerance(sized_notional);
        let representational_overage = sized_notional + (tolerance / MIDPOINT_DIVISOR_F64);
        let material_overage = sized_notional + (tolerance * MIDPOINT_DIVISOR_F64);

        assert!(!super::limit_notional_exceeds_sized_notional(
            representational_overage,
            sized_notional
        ));
        assert!(super::limit_notional_exceeds_sized_notional(
            material_overage,
            sized_notional
        ));
    }

    #[test]
    fn limit_notional_guard_blocks_non_finite_inputs() {
        assert!(super::limit_notional_exceeds_sized_notional(
            f64::NAN,
            BPS_DENOMINATOR
        ));
        assert!(super::limit_notional_exceeds_sized_notional(
            BPS_DENOMINATOR,
            f64::INFINITY
        ));
    }
}

#[cfg(test)]
mod economics_health_tests {
    use super::*;
    use crate::{
        bolt_v3_economics_runtime::{
            EconomicsAdmission, EconomicsAdmissionPurpose, EconomicsOrderBinding,
        },
        economics::EconomicsError,
    };

    #[test]
    fn expired_economics_fails_before_admission_mutates_state() {
        let evidence = Arc::new(DecisionEvidenceRecorder::recording());
        let admission = BoltV3SubmitAdmissionState::new(Arc::clone(&evidence));
        let request = BoltV3SubmitAdmissionRequest {
            strategy_id: "strategy-expired-economics".to_string(),
            execution_client_id: "execution-client-expired-economics".to_string(),
            client_order_id: "client-order-expired-economics".to_string(),
            instrument_id: "instrument-expired-economics".to_string(),
            notional: Decimal::ONE,
            order_side: OrderSide::Buy,
            order_quantity: Decimal::ONE,
            intent_kind: BoltV3SubmitIntentKind::Entry,
            risk_reducing_exit_proof: None,
            admission_evidence: None,
        };
        let economics = EconomicsAdmission::for_routing_test_with_validity(
            &request.execution_client_id,
            &request.instrument_id,
            crate::economics::OrderSide::Buy,
            EconomicsOrderBinding::from_sha256([7; 32]),
            EconomicsAdmissionPurpose::TradingEdge,
            Decimal::ONE,
            Decimal::ONE,
            10,
        );

        assert!(matches!(
            admission.admit_at_inner(&request, Some(&economics), 11),
            Err(BoltV3SubmitAdmissionError::EconomicsUnavailable(
                EconomicsError::RequiredCapabilityStale { valid_until_ns: 10 }
            ))
        ));
        assert_eq!(admission.admitted_order_count(), 0);
        assert_eq!(admission.reject_episode_count(), 0);
        assert!(
            evidence
                .recorded_facts()
                .expect("recorded current evidence must decode")
                .is_empty()
        );
    }
}

#[cfg(test)]
mod loss_governor_halt_evidence_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn recording_evidence() -> Arc<DecisionEvidenceRecorder> {
        Arc::new(DecisionEvidenceRecorder::recording())
    }

    fn failing_evidence() -> Arc<DecisionEvidenceRecorder> {
        let recorder = recording_evidence();
        recorder.fail_machine_writes_for_test();
        recorder
    }

    #[test]
    fn loss_snapshot_source_labels_are_stable_typed_projections() {
        assert_eq!(
            LossSnapshotSource::NtLossRuntimeFeed.as_str(),
            "nt_loss_runtime_feed"
        );
        assert_eq!(LossSnapshotSource::Unknown.as_str(), "unknown");
        assert_eq!(LossSnapshotSource::Other.as_str(), "other");
    }

    fn stale_loss_snapshot() -> LossSnapshot {
        LossSnapshot {
            source: Some(LossSnapshotSource::NtPortfolioSnapshot),
            observed_at_ns: 1_000,
            per_trade_pnl: Some(Decimal::ZERO),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
            source_observations: LossSourceObservationTimestamps::unobserved(),
        }
    }

    fn entry_request(strategy_id: String, client_order_id: String) -> BoltV3SubmitAdmissionRequest {
        BoltV3SubmitAdmissionRequest {
            strategy_id,
            execution_client_id: "execution-client-loss-halt".to_string(),
            client_order_id,
            instrument_id: "instrument-loss-halt-yes".to_string(),
            notional: Decimal::ONE,
            order_side: OrderSide::Buy,
            order_quantity: Decimal::ONE,
            intent_kind: BoltV3SubmitIntentKind::Entry,
            risk_reducing_exit_proof: None,
            admission_evidence: None,
        }
    }

    #[test]
    fn operator_health_snapshot_reports_poisoned_submit_admission_state() {
        let admission = BoltV3SubmitAdmissionState::new(recording_evidence());
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            admission.poison_inner_for_test();
        }));
        assert!(poisoned.is_err(), "test setup must poison the state lock");

        assert_eq!(
            admission.operator_health_snapshot(),
            Err(BoltV3SubmitAdmissionHealthReadError::StateLockPoisoned)
        );
    }

    #[test]
    fn poisoned_submit_admission_state_prevents_permit_and_provider_submit_side_effect() {
        let admission = BoltV3SubmitAdmissionState::new(recording_evidence());
        let request = entry_request(
            "strategy-poisoned-submit-state".to_string(),
            "client-order-poisoned-submit-state".to_string(),
        );
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            admission.poison_inner_for_test();
        }));
        assert!(poisoned.is_err(), "test setup must poison the state lock");
        let provider_submit_count = AtomicU64::new(0);

        let admission_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Ok(permit) = admission.admit_at(&request, 1_000) {
                provider_submit_count.fetch_add(1, Ordering::SeqCst);
                // Do not drop a recovered permit here: Drop re-locks the deliberately
                // poisoned inner state and would mask a fail-open recovery regression.
                std::mem::forget(permit);
            }
        }));

        let panic = admission_result.expect_err(
            "a poisoned admission state must not return control to the provider-submit branch",
        );
        assert!(
            crate::panic_payload_message(panic.as_ref())
                .contains("submit admission state mutex should not be poisoned"),
            "admission must panic specifically because the submit-admission state is poisoned"
        );
        assert_eq!(
            provider_submit_count.load(Ordering::SeqCst),
            0,
            "a poisoned admission state must have no provider submit side effect"
        );
    }

    #[test]
    fn poisoned_reject_episode_state_prevents_permit_and_provider_submit_side_effect() {
        let admission = BoltV3SubmitAdmissionState::new(recording_evidence());
        let request = entry_request(
            "strategy-poisoned-reject-episodes".to_string(),
            "client-order-poisoned-reject-episodes".to_string(),
        );
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            admission.poison_reject_episodes_for_test();
        }));
        assert!(
            poisoned.is_err(),
            "test setup must poison the reject-episode lock"
        );
        let provider_submit_count = AtomicU64::new(0);

        let admission_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Ok(permit) = admission.admit_at(&request, 1_000) {
                provider_submit_count.fetch_add(1, Ordering::SeqCst);
                // Keep permit destruction from becoming an unrelated panic that could
                // satisfy the poison assertion after an incorrect lock recovery.
                std::mem::forget(permit);
            }
        }));

        let panic = admission_result.expect_err(
            "a poisoned reject-episode state must not return control to the provider-submit branch",
        );
        assert!(
            crate::panic_payload_message(panic.as_ref())
                .contains("submit admission reject episodes mutex should not be poisoned"),
            "admission must panic specifically because the reject-episode state is poisoned"
        );
        assert_eq!(
            provider_submit_count.load(Ordering::SeqCst),
            0,
            "a poisoned reject-episode state must have no provider submit side effect"
        );
    }

    #[test]
    fn poisoned_reject_episode_state_blocks_rejected_path_without_provider_submit_side_effect() {
        let admission = BoltV3SubmitAdmissionState::new(recording_evidence());
        let mut request = entry_request(
            "strategy-poisoned-rejected-episode".to_string(),
            "client-order-poisoned-rejected-episode".to_string(),
        );
        request.notional = Decimal::ZERO;
        let routing_admission = BoltV3SubmitAdmissionState::new(recording_evidence());
        assert!(
            matches!(
                routing_admission.admit_at(&request, 1_000),
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            ),
            "the rejected-path fixture must route through non-positive-notional rejection"
        );
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            admission.poison_reject_episodes_for_test();
        }));
        assert!(
            poisoned.is_err(),
            "test setup must poison the reject-episode lock"
        );
        let provider_submit_count = AtomicU64::new(0);

        let admission_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if let Ok(permit) = admission.admit_at(&request, 1_000) {
                provider_submit_count.fetch_add(1, Ordering::SeqCst);
                // A recovered rejected path must return normally and fail the panic
                // assertion; permit destruction must not supply that expected panic.
                std::mem::forget(permit);
            }
        }));

        let panic = admission_result.expect_err(
            "a poisoned rejected-path episode lock must stop admission before returning",
        );
        assert!(
            crate::panic_payload_message(panic.as_ref())
                .contains("submit admission reject episodes mutex should not be poisoned"),
            "the rejected path must panic specifically because reject episodes are poisoned"
        );
        assert_eq!(
            provider_submit_count.load(Ordering::SeqCst),
            0,
            "a rejected request must never reach the provider-submit continuation"
        );
    }

    #[test]
    fn loss_governor_halt_evidence_write_failure_logs_error_and_rejects() {
        if !crate::bolt_v3_test_log_capture::enter_isolated_log_capture(
            "loss_governor_halt_evidence_write_failure_logs_error_and_rejects",
            "loss-governor-evidence-write",
        ) {
            return;
        }

        let writer = failing_evidence();
        let admission = BoltV3SubmitAdmissionState::new_with_loss_governor(
            writer.clone(),
            LossGovernorPolicy {
                max_snapshot_age_ns: 100,
                max_per_trade_loss: Some(Decimal::ONE),
                max_daily_loss: None,
                max_rolling_loss: None,
                max_drawdown: None,
            },
        );
        admission.update_loss_snapshot(stale_loss_snapshot());
        let request = entry_request(
            "strategy-loss-halt-log-guard".to_string(),
            "client-order-loss-halt-log-guard".to_string(),
        );

        let (result, records) = crate::bolt_v3_test_log_capture::with_captured_logs(|| {
            admission.admit_at(&request, 1_200)
        });

        match result {
            Err(BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }) => assert_eq!(
                reasons,
                vec![LossHaltReason::StaleLossSnapshot],
                "stale loss snapshot must still reject the submit request"
            ),
            Err(error) => panic!("expected loss-governor halt rejection, got {error:?}"),
            Ok(_) => panic!("expected loss-governor halt rejection, got permit"),
        }

        assert_eq!(
            writer.attempts_for(
                crate::bolt_v3_current_evidence::generated_contract::KnownPurpose::LossGovernorHalt,
            ),
            1,
            "the failing writer must still receive the halt evidence"
        );
        let matching = records
            .into_iter()
            .filter(|(_, message)| message.contains("loss governor halt evidence write failed"))
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "the halt evidence-write failure must be surfaced exactly once; got {matching:?}"
        );
        assert_eq!(
            matching[0].0,
            log::Level::Error,
            "the halt evidence-write failure must be surfaced at error! severity"
        );
    }
}
