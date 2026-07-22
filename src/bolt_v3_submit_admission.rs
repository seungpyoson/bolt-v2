use crate::bolt_v3_capital_admission::{
    CapitalAdmissionGate, CapitalAdmissionGateInputs, CapitalAdmissionLifecycleAction,
    CapitalAdmissionLifecycleKind, CapitalAdmissionLifecycleUpdate, CapitalAdmissionPolicy,
    CapitalAdmissionRequest, IntentLiquidity, IntentOrderKind, IntentSide,
    ProductAdmissionSnapshot, ProductKind,
};
use crate::bolt_v3_capital_admission_state::{
    NtDerivedCapitalAdmissionState, OrderLifecycleCapitalAdmissionSnapshot,
    PortfolioCapitalAdmissionSnapshot, ReservationLedgerSnapshot, VenueSpendabilitySnapshot,
    capital_admission_source_is_accepted_venue_truth,
};
use crate::bolt_v3_capital_reservation::{
    CapitalPoolSnapshot, ReservationRejectionReason, ReservationRequest,
};
use crate::bolt_v3_decision_evidence::{
    BOLT_V3_REJECT_EVIDENCE_MAX_EPISODES, BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
    BoltV3BasketAdmissionDecisionEvidence, BoltV3BasketAdmissionOutcome,
    BoltV3CapitalAdmissionRebuildAuditEvidence, BoltV3DecisionEvidenceWriter,
    BoltV3DecisionEvidenceWriterExt, BoltV3LossGovernorHaltEvidence, BoltV3LossHaltReason,
    BoltV3LossSnapshotStaleReason, BoltV3OrderIntentEvidence, BoltV3OrderIntentKind,
    BoltV3OrderRejectEvidence, BoltV3OrderRejectReason, BoltV3RecoveredSubmitReservationEvidence,
    BoltV3RejectSource, BoltV3StaleLossReason, BoltV3SubmitReservationFillEvidence,
    BoltV3SubmitReservationMetadataEvidence, EpisodeFirstNs, compiled_order_price_source,
    evict_oldest_episodes_over_cap, loss_snapshot_source_to_evidence,
};
use crate::bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind};
use crate::bolt_v3_loss_governor::{
    LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSnapshotDiagnostics,
    LossSnapshotStaleReason, LossSourceObservationTimestamps,
    evaluate_loss_admission_with_observations, loss_snapshot_stale_reason,
};
use crate::bolt_v3_numeric::{is_positive_finite, notional_float_tolerance};
use crate::bolt_v3_observed_dedupe::prune_observed_dedupe_entries;
use crate::bolt_v3_venue_truth::{VenueTruthCaptureFailureEvidence, VenueTruthDivergenceEvidence};
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
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub use crate::bolt_v3_decision_evidence::BoltV3SubmitIntentKind;

const SUBMIT_ADMISSION_BPS_DENOMINATOR: u32 = 10_000;
pub const VENUE_TRUTH_CAPTURE_FAILURE_RESERVATION_SOURCE: &str = "venue_truth_capture_failure";

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
) -> BoltV3AdmissionOutcome {
    match current_count.checked_add(claim_count) {
        Some(total) if total <= max_order_count => BoltV3AdmissionOutcome::Admitted,
        Some(_) | None => BoltV3AdmissionOutcome::RejectedCountCapExhausted,
    }
}

fn stale_loss_reason_key(reason: BoltV3StaleLossReason) -> &'static str {
    match reason {
        BoltV3StaleLossReason::MissingSnapshot => "missing_snapshot",
        BoltV3StaleLossReason::SourceEmpty => "source_empty",
        BoltV3StaleLossReason::FutureDated => "future_dated",
        BoltV3StaleLossReason::AgeExceeded => "age_exceeded",
        BoltV3StaleLossReason::MissingRequiredField => "missing_required_field",
    }
}

fn admission_outcome_key(outcome: &BoltV3AdmissionOutcome) -> &'static str {
    match outcome {
        BoltV3AdmissionOutcome::Admitted => "admitted",
        BoltV3AdmissionOutcome::RejectedKillSwitchLatched => "rejected_kill_switch_latched",
        BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
            "rejected_submit_lifecycle_disallowed"
        }
        BoltV3AdmissionOutcome::RejectedLossGovernorHalted => "rejected_loss_governor_halted",
        BoltV3AdmissionOutcome::RejectedNonPositiveNotional => "rejected_non_positive_notional",
        BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => "rejected_notional_cap_exceeded",
        BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
            "rejected_invalid_risk_reducing_exit_proof"
        }
        BoltV3AdmissionOutcome::RejectedCountCapExhausted => "rejected_count_cap_exhausted",
        BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
            "rejected_kill_switch_forced_reduction_proof_invalid"
        }
        BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
            "rejected_kill_switch_forced_reduction_cap_exceeded"
        }
        BoltV3AdmissionOutcome::RejectedCapitalAdmission => "rejected_capital_admission",
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
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
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
    kill_switch_forced_reduction_policy: Option<BoltV3KillSwitchForcedReductionPolicy>,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    admitted_order_count: u32,
    admitted_order_count_by_execution_client: BTreeMap<String, u32>,
    live_kill_switch_forced_reduction_order_count: u32,
    live_kill_switch_forced_reduction_client_order_ids: BTreeSet<String>,
    loss_policy: Option<LossGovernorPolicy>,
    loss_snapshot: Option<LossSnapshot>,
    loss_source_observations: LossSourceObservationTimestamps,
    loss_freshness: BoltV3LossFreshness,
    loss_halt_episodes: BTreeMap<String, LossHaltEpisode>,
    capital_admission: Option<BoltV3SubmitCapitalAdmissionState>,
}

#[derive(Debug)]
struct BoltV3SubmitCapitalAdmissionState {
    venue_id: String,
    account_id: String,
    product_kind: ProductKind,
    collateral_currency: String,
    capital_pool: CapitalPoolSnapshot,
    policy: CapitalAdmissionPolicy,
    dedupe_retention_ns: u64,
    state: Option<NtDerivedCapitalAdmissionState>,
    latest_reservation_mutation_observed_at_ns: Option<u64>,
    venue_truth_capture_failure_source: Option<String>,
    venue_truth_capture_failure_observed_at_ns: Option<u64>,
    gate: CapitalAdmissionGate,
    next_sequence: u64,
    client_order_reservations: BTreeMap<String, BoltV3SubmitReservationIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationIndex {
    submit_reservation_id: String,
    collateral_group_id: String,
    fill_metadata: Option<BoltV3SubmitReservationFillMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitReservationFillMetadata {
    instrument_id: String,
    side: BoltV3CompiledOrderSide,
    original_quantity: Decimal,
    filled_quantity: Decimal,
    liability_factor: Decimal,
    additive_liability: Decimal,
    last_lifecycle_observed_at_ns: u64,
    seen_trade_ids: BTreeMap<String, u64>,
    recovered_from_startup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionConfig {
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: ProductKind,
    pub collateral_currency: String,
    pub capital_pool: CapitalPoolSnapshot,
    pub policy: CapitalAdmissionPolicy,
    pub dedupe_retention_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionNtComponents {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioCapitalAdmissionSnapshot,
    pub venue_spendability: VenueSpendabilitySnapshot,
    pub order_lifecycle: OrderLifecycleCapitalAdmissionSnapshot,
    pub product_state: ProductAdmissionSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionOpenOrderReservation {
    pub client_order_id: String,
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
    pub seen_trade_ids: BTreeSet<String>,
    pub recovered_from_startup: bool,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
    pub observed_at_ns: u64,
    pub evidence_label: String,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub reservations: Vec<BoltV3SubmitCapitalAdmissionOpenOrderReservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionOpenOrderEvidence {
    pub client_order_id: String,
    pub instrument_id: String,
    pub side: BoltV3CompiledOrderSide,
    pub open_quantity: Decimal,
    pub limit_price: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
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
    source: String,
    observed_open_order_count: usize,
    all_open_orders_attributed: bool,
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
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionLifecycleUpdate {
    pub client_order_id: String,
    pub collateral_group_id: String,
    pub remaining_liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
    pub kind: CapitalAdmissionLifecycleKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitCapitalAdmissionLifecycleDecision {
    pub accepted: bool,
    pub unknown_reservation: bool,
    pub action: CapitalAdmissionLifecycleAction,
}

impl BoltV3SubmitCapitalAdmissionLifecycleDecision {
    fn unknown() -> Self {
        Self {
            accepted: true,
            unknown_reservation: true,
            action: CapitalAdmissionLifecycleAction::None,
        }
    }
}

impl BoltV3SubmitAdmissionState {
    pub fn new(decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>) -> Self {
        Self::new_with_live_submit_limits(decision_evidence, BTreeMap::new())
    }

    pub fn new_without_live_submit_limits(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
    ) -> Self {
        Self::new(decision_evidence)
    }

    pub fn new_with_live_submit_limits(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    ) -> Self {
        Self::new_with_optional_controls(decision_evidence, live_submit_approval_limits, None, None)
    }

    pub(crate) fn new_with_live_submit_limits_and_optional_controls(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
        loss_policy: Option<LossGovernorPolicy>,
        capital_admission: Option<BoltV3SubmitCapitalAdmissionConfig>,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            live_submit_approval_limits,
            loss_policy,
            capital_admission,
        )
    }

    pub fn new_with_loss_governor(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: LossGovernorPolicy,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            BTreeMap::new(),
            Some(loss_policy),
            None,
        )
    }

    pub fn new_with_capital_admission(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        capital_admission: BoltV3SubmitCapitalAdmissionConfig,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            BTreeMap::new(),
            None,
            Some(capital_admission),
        )
    }

    pub fn new_with_loss_governor_and_capital_admission(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: LossGovernorPolicy,
        capital_admission: BoltV3SubmitCapitalAdmissionConfig,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            BTreeMap::new(),
            Some(loss_policy),
            Some(capital_admission),
        )
    }

    fn new_with_optional_controls(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
        loss_policy: Option<LossGovernorPolicy>,
        capital_admission: Option<BoltV3SubmitCapitalAdmissionConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BoltV3SubmitAdmissionInner {
                kill_switch_state: KillSwitchState::Armed,
                kill_switch_forced_reduction_policy: None,
                live_submit_approval_limits,
                admitted_order_count: 0,
                admitted_order_count_by_execution_client: BTreeMap::new(),
                live_kill_switch_forced_reduction_order_count: 0,
                live_kill_switch_forced_reduction_client_order_ids: BTreeSet::new(),
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
                        dedupe_retention_ns: config.dedupe_retention_ns,
                        state: None,
                        latest_reservation_mutation_observed_at_ns: None,
                        venue_truth_capture_failure_source: None,
                        venue_truth_capture_failure_observed_at_ns: None,
                        gate: CapitalAdmissionGate::unreconciled(),
                        next_sequence: 0,
                        client_order_reservations: BTreeMap::new(),
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

    pub fn update_capital_admission_nt_components_after_accepted_venue_truth_capture(
        &self,
        components: BoltV3SubmitCapitalAdmissionNtComponents,
        accepted_capture_observed_at_ns: u64,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            if capital_admission
                .venue_truth_capture_failure_observed_at_ns
                .is_some_and(|failure_observed_at_ns| {
                    accepted_capture_observed_at_ns > failure_observed_at_ns
                })
            {
                capital_admission.venue_truth_capture_failure_source = None;
                capital_admission.venue_truth_capture_failure_observed_at_ns = None;
            }
            refresh_capital_admission_state_from_components(capital_admission, components);
        }
    }

    pub fn suspend_capital_admission_for_venue_truth_capture_failure(
        &self,
        evidence: VenueTruthCaptureFailureEvidence,
    ) {
        if let Err(error) = self
            .decision_evidence
            .record_venue_truth_capture_failure(&evidence)
        {
            log::error!("failed to record venue truth capture failure evidence: {error:#}");
        }
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            capital_admission.venue_truth_capture_failure_source = Some(evidence.source);
            capital_admission.venue_truth_capture_failure_observed_at_ns =
                Some(evidence.observed_at_ns);
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                evidence.observed_at_ns,
                VENUE_TRUTH_CAPTURE_FAILURE_RESERVATION_SOURCE.to_string(),
                false,
            );
        }
    }

    pub fn record_venue_truth_divergence_evidence(
        &self,
        evidence: &VenueTruthDivergenceEvidence,
    ) -> anyhow::Result<()> {
        self.decision_evidence
            .record_venue_truth_divergence(evidence)
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

    pub fn capital_admission_reconciled(&self) -> Option<bool> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let capital_admission = inner.capital_admission.as_ref()?;
        Some(
            capital_admission.gate.is_reconciled()
                && capital_admission
                    .venue_truth_capture_failure_source
                    .is_none(),
        )
    }

    pub fn capital_admission_open_order_reservation_from_evidence(
        &self,
        evidence: BoltV3SubmitCapitalAdmissionOpenOrderEvidence,
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
        if evidence.instrument_id != product.yes_instrument_id
            && evidence.instrument_id != product.no_instrument_id
        {
            return None;
        }
        let additive_liability = checked_additive_liability(&capital_admission.policy)?;
        if additive_liability < Decimal::ZERO {
            return None;
        }
        let liability_factor = match evidence.side.to_capital_admission() {
            IntentSide::Buy => evidence.limit_price,
            IntentSide::Sell => Decimal::ZERO,
        };
        if liability_factor < Decimal::ZERO || liability_factor > Decimal::ONE {
            return None;
        }
        let liability = checked_lifecycle_liability(
            evidence.open_quantity,
            liability_factor,
            additive_liability,
        )?;
        Some(BoltV3SubmitCapitalAdmissionOpenOrderReservation {
            client_order_id: evidence.client_order_id.clone(),
            submit_reservation_id: format!(
                "{}#{}",
                evidence.client_order_id, evidence.observed_at_ns
            ),
            collateral_group_id: product.collateral_coupled_group_id.clone(),
            liability,
            instrument_id: evidence.instrument_id,
            side: evidence.side,
            open_quantity: evidence.open_quantity,
            original_quantity: evidence.open_quantity,
            filled_quantity: Decimal::ZERO,
            liability_factor,
            additive_liability,
            seen_trade_ids: BTreeSet::new(),
            recovered_from_startup: false,
            observed_at_ns: evidence.observed_at_ns,
            evidence_label: evidence.evidence_label,
        })
    }

    pub fn capital_admission_open_order_reservation_from_known_metadata(
        &self,
        evidence: BoltV3SubmitCapitalAdmissionOpenOrderEvidence,
        recovered: &BoltV3RecoveredSubmitReservationEvidence,
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
        let metadata = &recovered.metadata;
        if metadata.client_order_id != evidence.client_order_id
            || metadata.venue_id != capital_admission.venue_id
            || metadata.account_id != capital_admission.account_id
            || metadata.product_kind != product_kind_evidence_value(capital_admission.product_kind)
            || metadata.collateral_currency != capital_admission.collateral_currency
            || metadata.capital_pool_id != capital_admission.capital_pool.pool_id
            || metadata.collateral_group_id != product.collateral_coupled_group_id
            || metadata.instrument_id != evidence.instrument_id
            || metadata.side != compiled_order_side_evidence_value(evidence.side)
        {
            return None;
        }
        if evidence.instrument_id != product.yes_instrument_id
            && evidence.instrument_id != product.no_instrument_id
        {
            return None;
        }
        let submitted_quantity = metadata.submitted_quantity.parse::<Decimal>().ok()?;
        let liability_factor = metadata.liability_factor.parse::<Decimal>().ok()?;
        let additive_liability = metadata.additive_liability.parse::<Decimal>().ok()?;
        let reserved_liability = metadata.reserved_liability.parse::<Decimal>().ok()?;
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
            submit_reservation_id: metadata.submit_reservation_id.clone(),
            collateral_group_id: metadata.collateral_group_id.clone(),
            liability: open_liability,
            instrument_id: metadata.instrument_id.clone(),
            side: evidence.side,
            open_quantity: evidence.open_quantity,
            original_quantity: submitted_quantity,
            filled_quantity,
            liability_factor,
            additive_liability,
            seen_trade_ids: recovered.fill_trade_ids.clone(),
            recovered_from_startup: true,
            observed_at_ns: evidence.observed_at_ns,
            evidence_label: "bolt_known_reservation_metadata".to_string(),
        })
    }

    pub fn rebuild_capital_admission_open_order_reservations(
        &self,
        open_order_reservations: Vec<BoltV3SubmitCapitalAdmissionOpenOrderReservation>,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionRebuildDecision {
        self.rebuild_capital_admission_open_order_snapshot(
            BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
                observed_at_ns: now_ns,
                evidence_label: "bolt_recovered_open_order_reservations".to_string(),
                observed_open_order_count: open_order_reservations.len(),
                all_open_orders_attributed: true,
                reservations: open_order_reservations,
            },
            now_ns,
        )
    }

    pub fn rebuild_capital_admission_open_order_snapshot(
        &self,
        snapshot: BoltV3SubmitCapitalAdmissionOpenOrderSnapshot,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionRebuildDecision {
        let rebuilt_order_lifecycle = OrderLifecycleCapitalAdmissionSnapshot {
            source: snapshot.evidence_label.clone(),
            observed_at_ns: snapshot.observed_at_ns,
            open_order_count: snapshot.observed_open_order_count,
            all_open_orders_attributed: snapshot.all_open_orders_attributed,
        };
        let audit_context = BoltV3SubmitCapitalAdmissionRebuildAuditContext {
            observed_at_ns: snapshot.observed_at_ns,
            source: snapshot.evidence_label.clone(),
            observed_open_order_count: snapshot.observed_open_order_count,
            all_open_orders_attributed: snapshot.all_open_orders_attributed,
        };
        let attempted_reservation_count = snapshot.observed_open_order_count;
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(capital_admission) = inner.capital_admission.as_mut() else {
            return BoltV3SubmitCapitalAdmissionRebuildDecision {
                accepted: true,
                reason: None,
                attempted_reservation_count: 0,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            };
        };
        if !snapshot.all_open_orders_attributed {
            if snapshot.observed_open_order_count > 0
                && let Some(state) = capital_admission.state.as_mut()
            {
                state.observed_at_ns = state.observed_at_ns.max(snapshot.observed_at_ns);
                if !order_lifecycle_is_accepted_venue_truth(&state.order_lifecycle) {
                    state.order_lifecycle = rebuilt_order_lifecycle;
                }
            }
            capital_admission.gate = CapitalAdmissionGate::unreconciled();
            capital_admission.client_order_reservations.clear();
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                snapshot.observed_at_ns,
                snapshot.evidence_label,
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
            return self.finish_capital_admission_rebuild(&mut inner, &audit_context, decision);
        }
        let Some(state) = capital_admission.state.as_ref() else {
            capital_admission.gate = CapitalAdmissionGate::unreconciled();
            capital_admission.client_order_reservations.clear();
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
            return self.finish_capital_admission_rebuild(&mut inner, &audit_context, decision);
        };
        capital_admission.capital_pool.source = state.portfolio.source.clone();
        capital_admission.capital_pool.observed_at_ns = state.portfolio.observed_at_ns;

        let mut rebuilt_index = BTreeMap::new();
        let mut reservation_requests = Vec::with_capacity(snapshot.reservations.len());
        for (index, reservation) in snapshot.reservations.into_iter().enumerate() {
            if rebuilt_index.contains_key(&reservation.client_order_id) {
                capital_admission.gate = CapitalAdmissionGate::unreconciled();
                capital_admission.client_order_reservations.clear();
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
                return self.finish_capital_admission_rebuild(&mut inner, &audit_context, decision);
            }
            if !rebuilt_open_order_reservation_metadata_valid(&reservation) {
                capital_admission.gate = CapitalAdmissionGate::unreconciled();
                capital_admission.client_order_reservations.clear();
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
                return self.finish_capital_admission_rebuild(&mut inner, &audit_context, decision);
            }

            let submit_reservation_id = reservation.submit_reservation_id;
            let collateral_group_id = reservation.collateral_group_id;
            let instrument_id = reservation.instrument_id;
            let side = reservation.side;
            let original_quantity = reservation.original_quantity;
            let filled_quantity = reservation.filled_quantity;
            let liability_factor = reservation.liability_factor;
            let additive_liability = reservation.additive_liability;
            let seen_trade_ids = reservation.seen_trade_ids;
            let recovered_from_startup = reservation.recovered_from_startup;
            let observed_at_ns = reservation.observed_at_ns;
            rebuilt_index.insert(
                reservation.client_order_id,
                BoltV3SubmitReservationIndex {
                    submit_reservation_id: submit_reservation_id.clone(),
                    collateral_group_id: collateral_group_id.clone(),
                    fill_metadata: Some(BoltV3SubmitReservationFillMetadata {
                        instrument_id,
                        side,
                        original_quantity,
                        filled_quantity,
                        liability_factor,
                        additive_liability,
                        last_lifecycle_observed_at_ns: observed_at_ns,
                        seen_trade_ids: seen_trade_ids
                            .into_iter()
                            .map(|trade_id| (trade_id, observed_at_ns))
                            .collect(),
                        recovered_from_startup,
                    }),
                },
            );
            reservation_requests.push(ReservationRequest {
                request_id: submit_reservation_id,
                pool_id: capital_admission.capital_pool.pool_id.clone(),
                collateral_group_id,
                liability: reservation.liability,
                observed_at_ns,
                evidence_label: reservation.evidence_label,
            });
        }

        capital_admission.client_order_reservations.clear();
        let decision = capital_admission.gate.rebuild_open_order_reservations(
            &capital_admission.capital_pool,
            &reservation_requests,
            now_ns,
            capital_admission.policy.min_remaining_pool_balance,
        );
        if decision.accepted {
            capital_admission.client_order_reservations = rebuilt_index;
            if snapshot.observed_open_order_count > 0
                && let Some(state) = capital_admission.state.as_mut()
            {
                state.observed_at_ns = state.observed_at_ns.max(now_ns);
                if !order_lifecycle_is_accepted_venue_truth(&state.order_lifecycle) {
                    state.order_lifecycle = rebuilt_order_lifecycle;
                }
            }
            refresh_capital_admission_reservation_snapshot(capital_admission, now_ns);
        }

        let decision = BoltV3SubmitCapitalAdmissionRebuildDecision {
            accepted: decision.accepted,
            reason: decision.reason,
            attempted_reservation_count: decision.attempted_reservation_count,
            rebuilt_reservation_count: decision.rebuilt_reservation_count,
            live_reserved_liability: decision.live_reserved_liability,
            missing_nt_account_cache_balance: None,
        };
        self.finish_capital_admission_rebuild(&mut inner, &audit_context, decision)
    }

    fn finish_capital_admission_rebuild(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        context: &BoltV3SubmitCapitalAdmissionRebuildAuditContext,
        decision: BoltV3SubmitCapitalAdmissionRebuildDecision,
    ) -> BoltV3SubmitCapitalAdmissionRebuildDecision {
        let audit = BoltV3CapitalAdmissionRebuildAuditEvidence {
            observed_at_ns: context.observed_at_ns,
            source: context.source.clone(),
            observed_open_order_count: context.observed_open_order_count,
            all_open_orders_attributed: context.all_open_orders_attributed,
            accepted: decision.accepted,
            reason: decision.reason,
            attempted_reservation_count: decision.attempted_reservation_count,
            recovered_reservation_count: decision.rebuilt_reservation_count,
            live_reserved_liability: decision.live_reserved_liability.to_string(),
        };
        if self
            .decision_evidence
            .record_capital_admission_rebuild_audit(&audit)
            .is_ok()
        {
            return decision;
        }

        if let Some(capital_admission) = inner.capital_admission.as_mut() {
            capital_admission.gate = CapitalAdmissionGate::unreconciled();
            capital_admission.client_order_reservations.clear();
            refresh_capital_admission_reservation_snapshot_with_source(
                capital_admission,
                context.observed_at_ns,
                context.source.clone(),
                false,
            );
        }
        BoltV3SubmitCapitalAdmissionRebuildDecision {
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
        }
    }

    pub fn apply_capital_admission_lifecycle_update(
        &self,
        update: BoltV3SubmitCapitalAdmissionLifecycleUpdate,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionLifecycleDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(capital_admission) = inner.capital_admission.as_mut() else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let Some(index) = capital_admission
            .client_order_reservations
            .get(&update.client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received capital-admission lifecycle update for unknown client_order_id={}",
                update.client_order_id
            );
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let update_observed_at_ns = update.observed_at_ns;
        let lifecycle_update = CapitalAdmissionLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: capital_admission.capital_pool.pool_id.clone(),
            collateral_group_id: update.collateral_group_id,
            remaining_liability: update.remaining_liability,
            observed_at_ns: update.observed_at_ns,
            evidence_label: update.evidence_label,
            kind: update.kind,
        };
        let decision = capital_admission.gate.apply_lifecycle_update(
            &capital_admission.capital_pool,
            &lifecycle_update,
            now_ns,
            capital_admission.policy.min_remaining_pool_balance,
        );
        if decision.accepted
            && update.kind == CapitalAdmissionLifecycleKind::Terminal
            && capital_admission
                .client_order_reservations
                .get(&update.client_order_id)
                .map(|current| current.submit_reservation_id.as_str())
                == Some(index.submit_reservation_id.as_str())
        {
            capital_admission
                .client_order_reservations
                .remove(&update.client_order_id);
        }
        if decision.accepted {
            refresh_capital_admission_reservation_snapshot(
                capital_admission,
                update_observed_at_ns,
            );
        }
        BoltV3SubmitCapitalAdmissionLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
            action: decision.action,
        }
    }

    pub fn apply_capital_admission_fill_update(
        &self,
        update: BoltV3SubmitCapitalAdmissionFillUpdate,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionLifecycleDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(capital_admission) = inner.capital_admission.as_mut() else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let dedupe_retention_ns = capital_admission.dedupe_retention_ns;
        let Some(index) = capital_admission
            .client_order_reservations
            .get(&update.client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received capital-admission fill update for unknown client_order_id={}",
                update.client_order_id
            );
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let Some(metadata) = index.fill_metadata.clone() else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        if update.trade_id.trim().is_empty()
            || update.fill_quantity <= Decimal::ZERO
            || update.instrument_id != metadata.instrument_id
            || update.side != metadata.side
        {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        }
        let mut metadata = metadata;
        prune_observed_dedupe_entries(
            &mut metadata.seen_trade_ids,
            update.observed_at_ns,
            dedupe_retention_ns,
        );
        if metadata.seen_trade_ids.contains_key(&update.trade_id) {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision {
                accepted: true,
                unknown_reservation: false,
                action: CapitalAdmissionLifecycleAction::None,
            };
        }
        let Some(next_observed_at_ns) = metadata.last_lifecycle_observed_at_ns.checked_add(1)
        else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let lifecycle_observed_at_ns = update.observed_at_ns.max(next_observed_at_ns);
        if metadata.recovered_from_startup && update.reconciliation {
            if let Some(current) = capital_admission
                .client_order_reservations
                .get_mut(&update.client_order_id)
                .filter(|current| current.submit_reservation_id == index.submit_reservation_id)
                && let Some(current_metadata) = current.fill_metadata.as_mut()
            {
                current_metadata.last_lifecycle_observed_at_ns = lifecycle_observed_at_ns;
                prune_observed_dedupe_entries(
                    &mut current_metadata.seen_trade_ids,
                    update.observed_at_ns,
                    dedupe_retention_ns,
                );
                current_metadata
                    .seen_trade_ids
                    .insert(update.trade_id, update.observed_at_ns);
            }
            refresh_capital_admission_reservation_snapshot(
                capital_admission,
                lifecycle_observed_at_ns,
            );
            return BoltV3SubmitCapitalAdmissionLifecycleDecision {
                accepted: true,
                unknown_reservation: false,
                action: CapitalAdmissionLifecycleAction::None,
            };
        }
        let lifecycle_now_ns = now_ns.max(lifecycle_observed_at_ns);
        let Some(unclamped_filled_quantity) =
            metadata.filled_quantity.checked_add(update.fill_quantity)
        else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let new_filled_quantity = metadata.original_quantity.min(unclamped_filled_quantity);
        let Some(remaining_quantity) = metadata.original_quantity.checked_sub(new_filled_quantity)
        else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let clamped = unclamped_filled_quantity > metadata.original_quantity;
        let remaining_liability = if remaining_quantity > Decimal::ZERO {
            let Some(remaining_liability) = checked_lifecycle_liability(
                remaining_quantity,
                metadata.liability_factor,
                metadata.additive_liability,
            ) else {
                return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
            };
            remaining_liability
        } else {
            Decimal::ZERO
        };
        let lifecycle_update = CapitalAdmissionLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: capital_admission.capital_pool.pool_id.clone(),
            collateral_group_id: index.collateral_group_id.clone(),
            remaining_liability,
            observed_at_ns: lifecycle_observed_at_ns,
            evidence_label: if clamped {
                "nt_order_fill_clamped".to_string()
            } else {
                update.evidence_label.clone()
            },
            kind: if remaining_quantity > Decimal::ZERO {
                CapitalAdmissionLifecycleKind::LiveResidual
            } else {
                CapitalAdmissionLifecycleKind::Terminal
            },
        };
        let fill_evidence = BoltV3SubmitReservationFillEvidence {
            client_order_id: update.client_order_id.clone(),
            submit_reservation_id: index.submit_reservation_id.clone(),
            trade_id: update.trade_id.clone(),
            instrument_id: update.instrument_id.clone(),
            side: compiled_order_side_evidence_value(update.side).to_string(),
            fill_quantity: update.fill_quantity.to_string(),
            observed_at_ns: update.observed_at_ns,
            reconciliation: update.reconciliation,
            source: update.evidence_label.clone(),
        };
        if self
            .decision_evidence
            .record_submit_reservation_fill(&fill_evidence)
            .is_err()
        {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision {
                accepted: false,
                unknown_reservation: false,
                action: CapitalAdmissionLifecycleAction::None,
            };
        }
        let decision = capital_admission.gate.apply_lifecycle_update(
            &capital_admission.capital_pool,
            &lifecycle_update,
            lifecycle_now_ns,
            capital_admission.policy.min_remaining_pool_balance,
        );
        if decision.accepted {
            if lifecycle_update.kind == CapitalAdmissionLifecycleKind::Terminal {
                if capital_admission
                    .client_order_reservations
                    .get(&update.client_order_id)
                    .map(|current| current.submit_reservation_id.as_str())
                    == Some(index.submit_reservation_id.as_str())
                {
                    capital_admission
                        .client_order_reservations
                        .remove(&update.client_order_id);
                }
            } else if let Some(current) = capital_admission
                .client_order_reservations
                .get_mut(&update.client_order_id)
                .filter(|current| current.submit_reservation_id == index.submit_reservation_id)
                && let Some(current_metadata) = current.fill_metadata.as_mut()
            {
                current_metadata.filled_quantity = new_filled_quantity;
                current_metadata.last_lifecycle_observed_at_ns = lifecycle_observed_at_ns;
                prune_observed_dedupe_entries(
                    &mut current_metadata.seen_trade_ids,
                    update.observed_at_ns,
                    dedupe_retention_ns,
                );
                current_metadata
                    .seen_trade_ids
                    .insert(update.trade_id, update.observed_at_ns);
            }
            refresh_capital_admission_reservation_snapshot(
                capital_admission,
                lifecycle_observed_at_ns,
            );
        }
        BoltV3SubmitCapitalAdmissionLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
            action: decision.action,
        }
    }

    pub fn apply_capital_admission_terminal_order_event(
        &self,
        client_order_id: String,
        observed_at_ns: u64,
        evidence_label: String,
    ) -> BoltV3SubmitCapitalAdmissionLifecycleDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(capital_admission) = inner.capital_admission.as_mut() else {
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let Some(index) = capital_admission
            .client_order_reservations
            .get(&client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received capital-admission terminal order event for unknown client_order_id={}",
                client_order_id
            );
            return BoltV3SubmitCapitalAdmissionLifecycleDecision::unknown();
        };
        let lifecycle_update = CapitalAdmissionLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: capital_admission.capital_pool.pool_id.clone(),
            collateral_group_id: index.collateral_group_id,
            remaining_liability: Decimal::ZERO,
            observed_at_ns,
            evidence_label,
            kind: CapitalAdmissionLifecycleKind::Terminal,
        };
        let decision = capital_admission.gate.apply_lifecycle_update(
            &capital_admission.capital_pool,
            &lifecycle_update,
            observed_at_ns,
            capital_admission.policy.min_remaining_pool_balance,
        );
        if decision.accepted
            && capital_admission
                .client_order_reservations
                .get(&client_order_id)
                .map(|current| current.submit_reservation_id.as_str())
                == Some(index.submit_reservation_id.as_str())
        {
            capital_admission
                .client_order_reservations
                .remove(&client_order_id);
        }
        if decision.accepted {
            refresh_capital_admission_reservation_snapshot(capital_admission, observed_at_ns);
        }
        BoltV3SubmitCapitalAdmissionLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
            action: decision.action,
        }
    }

    pub fn record_kill_switch_forced_reduction_terminal(&self, client_order_id: &str) -> bool {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if !inner
            .live_kill_switch_forced_reduction_client_order_ids
            .remove(client_order_id)
        {
            return false;
        }
        inner.live_kill_switch_forced_reduction_order_count = inner
            .live_kill_switch_forced_reduction_order_count
            .saturating_sub(1);
        true
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

    pub fn configure_kill_switch_forced_reduction_policy(
        &self,
        policy: BoltV3KillSwitchForcedReductionPolicy,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.kill_switch_forced_reduction_policy = Some(policy);
    }

    pub fn admit(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        self.admit_at(request, current_unix_ns()?)
    }

    pub fn admit_at(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let mut evaluation = self.evaluate(&mut inner, request, now_ns);
        let mut admitted_counter_update = None;
        if evaluation.outcome == BoltV3AdmissionOutcome::Admitted {
            let admitted_order_count_before = inner.admitted_order_count;
            let forced_reduction_order_count_before =
                inner.live_kill_switch_forced_reduction_order_count;
            let Some(next_admitted_order_count) = inner.admitted_order_count.checked_add(1) else {
                if let Some(rollback) = evaluation.rollback.as_ref() {
                    rollback_capital_admission_reservation(&mut inner, rollback);
                }
                evaluation.outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                if let Err(err) = self.record_admission_decision(request, &evaluation, now_ns) {
                    return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                        reason: format!("{err:#}"),
                    });
                }
                return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
            };
            let current_execution_client_count = inner
                .admitted_order_count_by_execution_client
                .get(&request.execution_client_id)
                .copied()
                .unwrap_or(0);
            let Some(next_execution_client_count) = current_execution_client_count.checked_add(1)
            else {
                if let Some(rollback) = evaluation.rollback.as_ref() {
                    rollback_capital_admission_reservation(&mut inner, rollback);
                }
                evaluation.outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                if let Err(err) = self.record_admission_decision(request, &evaluation, now_ns) {
                    return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                        reason: format!("{err:#}"),
                    });
                }
                return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
            };
            let next_forced_reduction_count = if request.intent_kind
                == BoltV3SubmitIntentKind::KillSwitchForcedReduction
            {
                let Some(next) = inner
                    .live_kill_switch_forced_reduction_order_count
                    .checked_add(1)
                else {
                    if let Some(rollback) = evaluation.rollback.as_ref() {
                        rollback_capital_admission_reservation(&mut inner, rollback);
                    }
                    evaluation.outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                    if let Err(err) = self.record_admission_decision(request, &evaluation, now_ns) {
                        return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                            reason: format!("{err:#}"),
                        });
                    }
                    return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
                };
                next
            } else {
                inner.live_kill_switch_forced_reduction_order_count
            };
            let forced_reduction_client_order_id = (request.intent_kind
                == BoltV3SubmitIntentKind::KillSwitchForcedReduction)
                .then(|| request.client_order_id.clone());
            let counter_rollback = BoltV3SubmitAdmissionCounterRollback {
                execution_client_id: request.execution_client_id.clone(),
                order_count: next_admitted_order_count.saturating_sub(admitted_order_count_before),
                forced_reduction_count: next_forced_reduction_count
                    .saturating_sub(forced_reduction_order_count_before),
                forced_reduction_client_order_id: forced_reduction_client_order_id.clone(),
            };
            admitted_counter_update = Some((
                next_admitted_order_count,
                next_execution_client_count,
                next_forced_reduction_count,
                forced_reduction_client_order_id,
                counter_rollback,
            ));
        }
        if evaluation.outcome == BoltV3AdmissionOutcome::Admitted
            && let Some(metadata) = evaluation.reservation_metadata.as_ref()
            && let Err(err) = self
                .decision_evidence
                .record_submit_reservation_metadata(metadata)
        {
            if let Some(rollback) = evaluation.rollback.as_ref() {
                rollback_capital_admission_reservation(&mut inner, rollback);
            }
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }
        if let Err(err) = self.record_admission_decision(request, &evaluation, now_ns) {
            if let Some(rollback) = evaluation.rollback.as_ref() {
                rollback_capital_admission_reservation(&mut inner, rollback);
            }
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }
        match evaluation.outcome {
            BoltV3AdmissionOutcome::Admitted => {
                let Some((
                    next_admitted_order_count,
                    next_execution_client_count,
                    next_forced_reduction_count,
                    forced_reduction_client_order_id,
                    counter_rollback,
                )) = admitted_counter_update
                else {
                    return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
                };
                inner.admitted_order_count = next_admitted_order_count;
                inner.admitted_order_count_by_execution_client.insert(
                    request.execution_client_id.clone(),
                    next_execution_client_count,
                );
                inner.live_kill_switch_forced_reduction_order_count = next_forced_reduction_count;
                if let Some(client_order_id) = forced_reduction_client_order_id {
                    inner
                        .live_kill_switch_forced_reduction_client_order_ids
                        .insert(client_order_id);
                }
                Ok(BoltV3SubmitAdmissionPermit {
                    inner: self.inner.clone(),
                    rollbacks: evaluation.rollback.into_iter().collect(),
                    counter_rollback: Some(counter_rollback),
                    committed: false,
                })
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchLatched => {
                Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
                    state: inner.kill_switch_state.kind(),
                })
            }
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                Err(BoltV3SubmitAdmissionError::SubmitLifecycleDisallowed {
                    intent: request.intent_kind,
                })
            }
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
                Err(BoltV3SubmitAdmissionError::LossGovernorHalted {
                    reasons: evaluation.loss_halt_reasons,
                })
            }
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            }
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                Err(BoltV3SubmitAdmissionError::NotionalCapExceeded)
            }
            BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
                Err(BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof)
            }
            BoltV3AdmissionOutcome::RejectedCountCapExhausted => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
            BoltV3AdmissionOutcome::RejectedCapitalAdmission => {
                Err(BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
                    reason: evaluation
                        .capital_admission_rejection
                        .unwrap_or(BoltV3CapitalAdmissionRejectReason::Rejected),
                })
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
                Err(BoltV3SubmitAdmissionError::KillSwitchForcedReductionProofInvalid)
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
                Err(BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded)
            }
        }
    }

    pub fn evaluate_and_record_without_consuming_capacity(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let now_ns = current_unix_ns()?;
        let evaluation = self.evaluate(&mut inner, request, now_ns);
        let record_result = self.record_admission_decision(request, &evaluation, now_ns);
        if let Some(rollback) = evaluation.rollback.as_ref() {
            rollback_capital_admission_reservation(&mut inner, rollback);
        }
        if let Err(err) = record_result {
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }
        Self::admission_result(&inner, request, &evaluation)
    }

    fn record_admission_decision(
        &self,
        request: &BoltV3SubmitAdmissionRequest,
        evaluation: &BoltV3SubmitAdmissionEvaluation,
        now_ns: u64,
    ) -> Result<(), anyhow::Error> {
        let evidence = BoltV3AdmissionDecisionEvidence {
            strategy_id: request.strategy_id.clone(),
            execution_client_id: request.execution_client_id.clone(),
            client_order_id: request.client_order_id.clone(),
            instrument_id: request.instrument_id.clone(),
            notional: request.notional.to_string(),
            intent_kind: request.intent_kind,
            outcome: evaluation.outcome.clone(),
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
                .and_then(|diagnostics| diagnostics.snapshot_source.as_deref())
                .map(loss_snapshot_source_to_evidence),
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
        };
        let result = match evidence.intent_kind {
            BoltV3SubmitIntentKind::Entry => {
                if evidence.outcome == BoltV3AdmissionOutcome::Admitted {
                    self.decision_evidence
                        .record_admitted_entry_admission(&evidence)
                } else {
                    self.decision_evidence
                        .record_rejected_entry_admission(&evidence)
                }
            }
            BoltV3SubmitIntentKind::RiskReducingExit => self
                .decision_evidence
                .record_risk_reducing_exit_admission(&evidence),
            BoltV3SubmitIntentKind::KillSwitchForcedReduction => self
                .decision_evidence
                .record_forced_reduction_admission(&evidence),
            BoltV3SubmitIntentKind::ReplaceSubmit => Err(anyhow::anyhow!(
                "replace admission has no registered evidence purpose"
            )),
        };
        if evaluation.outcome == BoltV3AdmissionOutcome::Admitted {
            self.reject_episodes
                .lock()
                .expect("submit admission reject episodes mutex should not be poisoned")
                .clear();
        } else if evaluation.outcome != BoltV3AdmissionOutcome::RejectedLossGovernorHalted {
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
                BOLT_V3_REJECT_EVIDENCE_MAX_EPISODES,
            );
            (prior_client_order_id, retry_count, elapsed_ns, should_emit)
        };
        if !should_emit {
            return;
        }

        let evidence = BoltV3OrderRejectEvidence {
            reject_source: BoltV3RejectSource::SubmitAdmission,
            reject_reason: BoltV3OrderRejectReason::AdmissionRejected,
            admission_outcome: Some(evaluation.outcome.clone()),
            raw_reason_text: None,
            instrument_id: request.instrument_id.clone(),
            order_side: Some(side_key.to_string()),
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
            backoff_cooldown_state: None,
            stable_episode_key,
            elapsed_ns,
        };
        if let Err(err) = self.decision_evidence.record_order_reject(&evidence) {
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
    ) {
        let snapshot = inner.loss_snapshot.as_ref();
        let stale_reason = match snapshot {
            Some(snapshot) => loss_snapshot_stale_reason(loss_policy, snapshot, now_ns)
                .unwrap_or(BoltV3StaleLossReason::MissingSnapshot),
            None => BoltV3StaleLossReason::MissingSnapshot,
        };
        let source_for_key = snapshot
            .map(|snapshot| snapshot.source.as_str())
            .filter(|source| !source.trim().is_empty())
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
            return;
        }

        let freshness = inner.loss_freshness;
        let evidence = BoltV3LossGovernorHaltEvidence {
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
            snapshot_source: snapshot.map(|snapshot| snapshot.source.clone()),
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

        if let Err(err) = self.decision_evidence.record_loss_governor_halt(&evidence) {
            log::error!(
                "bolt-v3 loss governor halt evidence write failed: stable_halt_key={} stale_reason={:?}: {err:#}",
                evidence.stable_halt_key,
                evidence.stale_reason
            );
        }
    }

    fn admission_result(
        inner: &BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
        evaluation: &BoltV3SubmitAdmissionEvaluation,
    ) -> Result<(), BoltV3SubmitAdmissionError> {
        match evaluation.outcome {
            BoltV3AdmissionOutcome::Admitted => Ok(()),
            BoltV3AdmissionOutcome::RejectedKillSwitchLatched => {
                Err(BoltV3SubmitAdmissionError::KillSwitchLatched {
                    state: inner.kill_switch_state.kind(),
                })
            }
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                Err(BoltV3SubmitAdmissionError::SubmitLifecycleDisallowed {
                    intent: request.intent_kind,
                })
            }
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
                Err(BoltV3SubmitAdmissionError::LossGovernorHalted {
                    reasons: evaluation.loss_halt_reasons.clone(),
                })
            }
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                Err(BoltV3SubmitAdmissionError::NonPositiveNotional)
            }
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                Err(BoltV3SubmitAdmissionError::NotionalCapExceeded)
            }
            BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
                Err(BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof)
            }
            BoltV3AdmissionOutcome::RejectedCountCapExhausted => {
                Err(BoltV3SubmitAdmissionError::CountCapExhausted)
            }
            BoltV3AdmissionOutcome::RejectedCapitalAdmission => {
                Err(BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
                    reason: evaluation
                        .capital_admission_rejection
                        .unwrap_or(BoltV3CapitalAdmissionRejectReason::Rejected),
                })
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
                Err(BoltV3SubmitAdmissionError::KillSwitchForcedReductionProofInvalid)
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
                Err(BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded)
            }
        }
    }

    pub fn reserve_basket_submit_slots(
        &self,
        execution_client_id: &str,
        claims: &[BoltV3BasketSubmitSlotClaim],
        evidence: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<BoltV3SubmitAdmissionPermit, BoltV3SubmitAdmissionError> {
        let now_ns = current_unix_ns()?;
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");

        let mut outcome = if claims.is_empty() {
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional
        } else {
            BoltV3AdmissionOutcome::Admitted
        };
        let mut rejected_intent = claims
            .first()
            .map(|claim| claim.intent_kind)
            .unwrap_or(BoltV3SubmitIntentKind::Entry);
        let mut rejected_request: Option<BoltV3SubmitAdmissionRequest> = None;
        let mut rejected_evaluation: Option<BoltV3SubmitAdmissionEvaluation> = None;
        let mut rollbacks = Vec::new();
        let mut reservation_metadata = Vec::new();

        for claim in claims {
            let request = basket_submit_request(&evidence.strategy_id, execution_client_id, claim);
            let evaluation = self.evaluate(&mut inner, &request, now_ns);
            outcome = evaluation.outcome.clone();
            if outcome != BoltV3AdmissionOutcome::Admitted {
                rejected_intent = claim.intent_kind;
                rejected_request = Some(request);
                rejected_evaluation = Some(evaluation);
                break;
            }
            if let Some(rollback) = evaluation.rollback {
                rollbacks.push(rollback);
            }
            if let Some(metadata) = evaluation.reservation_metadata {
                reservation_metadata.push(metadata);
            }
        }

        let claim_count = match u32::try_from(claims.len()) {
            Ok(value) => value,
            Err(_) => {
                outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                u32::MAX
            }
        };
        let forced_reduction_count = match claims
            .iter()
            .filter(|claim| claim.intent_kind == BoltV3SubmitIntentKind::KillSwitchForcedReduction)
            .count()
            .try_into()
        {
            Ok(value) => value,
            Err(_) => {
                outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                u32::MAX
            }
        };

        if outcome == BoltV3AdmissionOutcome::Admitted
            && inner
                .admitted_order_count
                .checked_add(claim_count)
                .is_none()
        {
            outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
        }

        if outcome == BoltV3AdmissionOutcome::Admitted
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

        let mut evidence = evidence.clone();
        evidence.outcome = basket_outcome_from_submit_outcome(outcome.clone());

        if outcome != BoltV3AdmissionOutcome::Admitted {
            if let Err(err) = self
                .decision_evidence
                .record_basket_admission_rejected(&evidence)
            {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            if let (Some(request), Some(evaluation)) =
                (rejected_request.as_ref(), rejected_evaluation.as_ref())
            {
                Self::admission_result(&inner, request, evaluation)?;
            }
            return Err(submit_admission_error_from_outcome(
                outcome,
                inner.kill_switch_state.kind(),
                rejected_intent,
            ));
        }

        let Some(next_admitted_order_count) = inner.admitted_order_count.checked_add(claim_count)
        else {
            evidence.outcome = basket_outcome_from_submit_outcome(
                BoltV3AdmissionOutcome::RejectedCountCapExhausted,
            );
            if let Err(err) = self
                .decision_evidence
                .record_basket_admission_rejected(&evidence)
            {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
        };
        let current_client_count = inner
            .admitted_order_count_by_execution_client
            .get(execution_client_id)
            .copied()
            .unwrap_or(0);
        let Some(next_client_count) = current_client_count.checked_add(claim_count) else {
            evidence.outcome = basket_outcome_from_submit_outcome(
                BoltV3AdmissionOutcome::RejectedCountCapExhausted,
            );
            if let Err(err) = self
                .decision_evidence
                .record_basket_admission_rejected(&evidence)
            {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
        };
        let Some(next_forced_reduction_order_count) = inner
            .live_kill_switch_forced_reduction_order_count
            .checked_add(forced_reduction_count)
        else {
            evidence.outcome = basket_outcome_from_submit_outcome(
                BoltV3AdmissionOutcome::RejectedCountCapExhausted,
            );
            if let Err(err) = self
                .decision_evidence
                .record_basket_admission_rejected(&evidence)
            {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
        };

        let counter_rollback = BoltV3SubmitAdmissionCounterRollback {
            execution_client_id: execution_client_id.to_string(),
            order_count: claim_count,
            forced_reduction_count,
            forced_reduction_client_order_id: None,
        };

        for metadata in &reservation_metadata {
            if let Err(err) = self
                .decision_evidence
                .record_submit_reservation_metadata(metadata)
            {
                rollback_capital_admission_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
        }

        if let Err(err) = self
            .decision_evidence
            .record_basket_admission_granted(&evidence)
        {
            rollback_capital_admission_reservations(&mut inner, &rollbacks);
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }

        inner.admitted_order_count = next_admitted_order_count;
        inner
            .admitted_order_count_by_execution_client
            .insert(execution_client_id.to_string(), next_client_count);
        inner.live_kill_switch_forced_reduction_order_count = next_forced_reduction_order_count;

        Ok(BoltV3SubmitAdmissionPermit {
            inner: self.inner.clone(),
            rollbacks,
            counter_rollback: Some(counter_rollback),
            committed: false,
        })
    }

    fn evaluate(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
        now_ns: u64,
    ) -> BoltV3SubmitAdmissionEvaluation {
        if request.intent_kind == BoltV3SubmitIntentKind::KillSwitchForcedReduction {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                Self::evaluate_kill_switch_forced_reduction(inner, request),
                now_ns,
            );
        }
        if matches!(
            request.intent_kind,
            BoltV3SubmitIntentKind::Entry
                | BoltV3SubmitIntentKind::RiskReducingExit
                | BoltV3SubmitIntentKind::ReplaceSubmit
        ) && inner.kill_switch_state.kind() != KillSwitchStateKind::Armed
        {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedKillSwitchLatched,
                now_ns,
            );
        }
        if !request.lifecycle_policy.allows(request.intent_kind) {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed,
                now_ns,
            );
        }
        let mut loss_snapshot_diagnostics = None;
        if let Some(loss_policy) = inner.loss_policy.clone()
            && matches!(
                request.intent_kind,
                BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
            )
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
                    self.record_stale_loss_governor_halt(inner, &loss_policy, now_ns);
                }
                return BoltV3SubmitAdmissionEvaluation::loss_halt(
                    decision.halt_reasons,
                    decision.diagnostics,
                );
            }
            inner.loss_halt_episodes.clear();
        }
        if request.notional <= Decimal::ZERO {
            return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                BoltV3AdmissionOutcome::RejectedNonPositiveNotional,
                now_ns,
            )
            .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
        }
        if let Some(limits) = inner
            .live_submit_approval_limits
            .get(&request.execution_client_id)
        {
            if request.notional > limits.max_order_notional {
                return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                    BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
                    now_ns,
                )
                .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
            }
            let current_count = inner
                .admitted_order_count_by_execution_client
                .get(&request.execution_client_id)
                .copied()
                .unwrap_or(0);
            if live_submit_count_cap_outcome(current_count, 1, limits.max_order_count)
                == BoltV3AdmissionOutcome::RejectedCountCapExhausted
            {
                return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                    BoltV3AdmissionOutcome::RejectedCountCapExhausted,
                    now_ns,
                )
                .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
            }
        }
        match request.intent_kind {
            BoltV3SubmitIntentKind::Entry => {}
            BoltV3SubmitIntentKind::RiskReducingExit => {
                let Some(proof) = request.risk_reducing_exit_proof.as_ref() else {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof,
                        now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
                };
                if !proof.is_valid_for_shape(
                    &request.instrument_id,
                    request.order_side,
                    request.order_quantity,
                ) {
                    return BoltV3SubmitAdmissionEvaluation::without_loss_halt(
                        BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof,
                        now_ns,
                    )
                    .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
                }
            }
            BoltV3SubmitIntentKind::ReplaceSubmit => {}
            BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
                unreachable!("kill-switch forced reduction is evaluated before normal admission")
            }
        }
        if inner.capital_admission.is_some()
            && matches!(
                request.intent_kind,
                BoltV3SubmitIntentKind::Entry
                    | BoltV3SubmitIntentKind::RiskReducingExit
                    | BoltV3SubmitIntentKind::ReplaceSubmit
            )
        {
            let decision = evaluate_capital_admission_submit(inner, request, now_ns);
            if !decision.accepted {
                return BoltV3SubmitAdmissionEvaluation::capital_admission_rejected(
                    decision.reason,
                    now_ns,
                )
                .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
            }
            return BoltV3SubmitAdmissionEvaluation::admitted_with_rollback(
                decision.rollback,
                decision.reservation_metadata,
                now_ns,
            )
            .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics);
        }
        BoltV3SubmitAdmissionEvaluation::without_loss_halt(BoltV3AdmissionOutcome::Admitted, now_ns)
            .with_loss_snapshot_diagnostics(loss_snapshot_diagnostics)
    }

    fn evaluate_kill_switch_forced_reduction(
        inner: &BoltV3SubmitAdmissionInner,
        request: &BoltV3SubmitAdmissionRequest,
    ) -> BoltV3AdmissionOutcome {
        let Some(policy) = inner.kill_switch_forced_reduction_policy.as_ref() else {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        };
        let Some(claim) = request.kill_switch_forced_reduction.as_ref() else {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        };
        let Some(halt_id) = forced_reduction_admissible_halt_id(&inner.kill_switch_state) else {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        };
        if claim.halt_id() != halt_id || claim.policy_sha256() != policy.policy_sha256() {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid;
        }
        // KillSwitchState exposes the admissible halt id, not the planner's action id.
        // The action binding is enforced when the flatten planner constructs the
        // forced-reduction claim, and the strategy/policy fence confines claim
        // construction to that owning path.
        if request.notional <= Decimal::ZERO {
            return BoltV3AdmissionOutcome::RejectedNonPositiveNotional;
        }
        if request.notional > policy.max_notional_per_order() {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded;
        }
        if inner
            .live_kill_switch_forced_reduction_client_order_ids
            .contains(&request.client_order_id)
        {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded;
        }
        if inner.live_kill_switch_forced_reduction_order_count >= policy.max_live_order_count() {
            return BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded;
        }
        BoltV3AdmissionOutcome::Admitted
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

    /// Upper bound the reject-episode map is held to. Surfaces the shared
    /// `BOLT_V3_REJECT_EVIDENCE_MAX_EPISODES` source-of-truth so callers (and the
    /// bound test) can assert against it without re-stating the literal.
    #[must_use]
    pub fn reject_episode_capacity() -> usize {
        BOLT_V3_REJECT_EVIDENCE_MAX_EPISODES
    }
}

fn basket_outcome_from_submit_outcome(
    outcome: BoltV3AdmissionOutcome,
) -> BoltV3BasketAdmissionOutcome {
    match outcome {
        BoltV3AdmissionOutcome::Admitted => BoltV3BasketAdmissionOutcome::Admitted,
        BoltV3AdmissionOutcome::RejectedKillSwitchLatched
        | BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed
        | BoltV3AdmissionOutcome::RejectedLossGovernorHalted
        | BoltV3AdmissionOutcome::RejectedNonPositiveNotional
        | BoltV3AdmissionOutcome::RejectedNotionalCapExceeded
        | BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof
        | BoltV3AdmissionOutcome::RejectedCountCapExhausted
        | BoltV3AdmissionOutcome::RejectedCapitalAdmission
        | BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid
        | BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
            BoltV3BasketAdmissionOutcome::RejectedSubmitSlots
        }
    }
}

fn loss_halt_reason_to_evidence(reason: LossHaltReason) -> BoltV3LossHaltReason {
    match reason {
        LossHaltReason::PerTradeLossLimit => BoltV3LossHaltReason::PerTradeLossLimit,
        LossHaltReason::DailyLossLimit => BoltV3LossHaltReason::DailyLossLimit,
        LossHaltReason::RollingLossLimit => BoltV3LossHaltReason::RollingLossLimit,
        LossHaltReason::MaxDrawdownLimit => BoltV3LossHaltReason::MaxDrawdownLimit,
        LossHaltReason::StaleLossSnapshot => BoltV3LossHaltReason::StaleLossSnapshot,
    }
}

fn loss_snapshot_stale_reason_to_evidence(
    reason: LossSnapshotStaleReason,
) -> BoltV3LossSnapshotStaleReason {
    match reason {
        LossSnapshotStaleReason::MissingSnapshot => BoltV3LossSnapshotStaleReason::MissingSnapshot,
        LossSnapshotStaleReason::SourceEmpty => BoltV3LossSnapshotStaleReason::SourceEmpty,
        LossSnapshotStaleReason::FutureDated => BoltV3LossSnapshotStaleReason::FutureDated,
        LossSnapshotStaleReason::AgeExceeded => BoltV3LossSnapshotStaleReason::AgeExceeded,
        LossSnapshotStaleReason::MissingRequiredField => {
            BoltV3LossSnapshotStaleReason::MissingRequiredField
        }
    }
}

fn submit_admission_error_from_outcome(
    outcome: BoltV3AdmissionOutcome,
    kill_switch_state: KillSwitchStateKind,
    intent: BoltV3SubmitIntentKind,
) -> BoltV3SubmitAdmissionError {
    match outcome {
        BoltV3AdmissionOutcome::Admitted => {
            unreachable!("admitted outcome does not convert to a submit admission error")
        }
        BoltV3AdmissionOutcome::RejectedKillSwitchLatched => {
            BoltV3SubmitAdmissionError::KillSwitchLatched {
                state: kill_switch_state,
            }
        }
        BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
            BoltV3SubmitAdmissionError::SubmitLifecycleDisallowed { intent }
        }
        BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
            BoltV3SubmitAdmissionError::LossGovernorHalted {
                reasons: Vec::new(),
            }
        }
        BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
            BoltV3SubmitAdmissionError::NonPositiveNotional
        }
        BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
            BoltV3SubmitAdmissionError::NotionalCapExceeded
        }
        BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
            BoltV3SubmitAdmissionError::InvalidRiskReducingExitProof
        }
        BoltV3AdmissionOutcome::RejectedCountCapExhausted => {
            BoltV3SubmitAdmissionError::CountCapExhausted
        }
        BoltV3AdmissionOutcome::RejectedCapitalAdmission => {
            BoltV3SubmitAdmissionError::CapitalAdmissionRejected {
                reason: BoltV3CapitalAdmissionRejectReason::Rejected,
            }
        }
        BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
            BoltV3SubmitAdmissionError::KillSwitchForcedReductionProofInvalid
        }
        BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
            BoltV3SubmitAdmissionError::KillSwitchForcedReductionCapExceeded
        }
    }
}

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionPermit {
    inner: Arc<Mutex<BoltV3SubmitAdmissionInner>>,
    rollbacks: Vec<BoltV3CapitalAdmissionReservationRollback>,
    counter_rollback: Option<BoltV3SubmitAdmissionCounterRollback>,
    committed: bool,
}

impl BoltV3SubmitAdmissionPermit {
    pub fn commit_submitted(mut self) {
        self.committed = true;
        self.rollbacks.clear();
        self.counter_rollback = None;
    }
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
        if let Some(counter_rollback) = self.counter_rollback.as_ref() {
            rollback_admission_counters(&mut inner, counter_rollback);
        }
        rollback_capital_admission_reservations(&mut inner, &self.rollbacks);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitAdmissionCounterRollback {
    execution_client_id: String,
    order_count: u32,
    forced_reduction_count: u32,
    forced_reduction_client_order_id: Option<String>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionEvaluation {
    outcome: BoltV3AdmissionOutcome,
    admission_now_ns: u64,
    loss_halt_reasons: Vec<LossHaltReason>,
    loss_snapshot_diagnostics: Option<LossSnapshotDiagnostics>,
    capital_admission_rejection: Option<BoltV3CapitalAdmissionRejectReason>,
    rollback: Option<BoltV3CapitalAdmissionReservationRollback>,
    reservation_metadata: Option<BoltV3SubmitReservationMetadataEvidence>,
}

impl BoltV3SubmitAdmissionEvaluation {
    fn without_loss_halt(outcome: BoltV3AdmissionOutcome, admission_now_ns: u64) -> Self {
        Self {
            outcome,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            capital_admission_rejection: None,
            rollback: None,
            reservation_metadata: None,
        }
    }

    fn loss_halt(
        loss_halt_reasons: Vec<LossHaltReason>,
        diagnostics: LossSnapshotDiagnostics,
    ) -> Self {
        let admission_now_ns = diagnostics.admission_now_ns;
        Self {
            outcome: BoltV3AdmissionOutcome::RejectedLossGovernorHalted,
            admission_now_ns,
            loss_halt_reasons,
            loss_snapshot_diagnostics: Some(diagnostics),
            capital_admission_rejection: None,
            rollback: None,
            reservation_metadata: None,
        }
    }

    fn capital_admission_rejected(
        reason: BoltV3CapitalAdmissionRejectReason,
        admission_now_ns: u64,
    ) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::RejectedCapitalAdmission,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            capital_admission_rejection: Some(reason),
            rollback: None,
            reservation_metadata: None,
        }
    }

    fn admitted_with_rollback(
        rollback: Option<BoltV3CapitalAdmissionReservationRollback>,
        reservation_metadata: Option<BoltV3SubmitReservationMetadataEvidence>,
        admission_now_ns: u64,
    ) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::Admitted,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            capital_admission_rejection: None,
            rollback,
            reservation_metadata,
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
pub struct BoltV3KillSwitchForcedReductionPolicy {
    policy_sha256: String,
    max_live_order_count: u32,
    max_notional_per_order: Decimal,
}

impl BoltV3KillSwitchForcedReductionPolicy {
    pub fn new(
        policy_sha256: impl Into<String>,
        max_live_order_count: u32,
        max_notional_per_order: Decimal,
    ) -> Result<Self, BoltV3KillSwitchForcedReductionError> {
        let policy_sha256 = policy_sha256.into();
        if policy_sha256.len() != 64 || !policy_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BoltV3KillSwitchForcedReductionError::InvalidPolicySha256);
        }
        if max_live_order_count == 0 {
            return Err(BoltV3KillSwitchForcedReductionError::NonPositiveMaxLiveOrderCount);
        }
        if max_notional_per_order <= Decimal::ZERO {
            return Err(BoltV3KillSwitchForcedReductionError::NonPositiveMaxNotional);
        }
        Ok(Self {
            policy_sha256,
            max_live_order_count,
            max_notional_per_order,
        })
    }

    pub fn policy_sha256(&self) -> &str {
        &self.policy_sha256
    }

    pub fn max_live_order_count(&self) -> u32 {
        self.max_live_order_count
    }

    pub fn max_notional_per_order(&self) -> Decimal {
        self.max_notional_per_order
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3KillSwitchForcedReductionClaim {
    halt_id: String,
    action_id: String,
    policy_sha256: String,
}

impl BoltV3KillSwitchForcedReductionClaim {
    pub fn new(
        halt_id: impl Into<String>,
        action_id: impl Into<String>,
        policy_sha256: impl Into<String>,
    ) -> Result<Self, BoltV3KillSwitchForcedReductionError> {
        let halt_id = halt_id.into();
        let action_id = action_id.into();
        let policy_sha256 = policy_sha256.into();
        if halt_id.trim().is_empty() {
            return Err(BoltV3KillSwitchForcedReductionError::MissingHaltId);
        }
        if action_id.trim().is_empty() {
            return Err(BoltV3KillSwitchForcedReductionError::MissingActionId);
        }
        if policy_sha256.len() != 64 || !policy_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(BoltV3KillSwitchForcedReductionError::InvalidPolicySha256);
        }
        Ok(Self {
            halt_id,
            action_id,
            policy_sha256,
        })
    }

    pub fn halt_id(&self) -> &str {
        &self.halt_id
    }

    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    pub fn policy_sha256(&self) -> &str {
        &self.policy_sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3KillSwitchForcedReductionError {
    MissingHaltId,
    MissingActionId,
    InvalidPolicySha256,
    NonPositiveMaxLiveOrderCount,
    NonPositiveMaxNotional,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum BoltV3OrderLifecycleIntent {
    Entry,
    RiskReducingExit,
    ReplaceSubmit,
    PlainCancel,
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct BoltV3SubmitLifecyclePolicy {
    replace_submit: bool,
}

impl BoltV3SubmitLifecyclePolicy {
    pub fn new(replace_submit: bool) -> Self {
        Self { replace_submit }
    }

    pub fn submit_intent_for(
        &self,
        intent: BoltV3OrderLifecycleIntent,
    ) -> Result<Option<BoltV3SubmitIntentKind>, BoltV3SubmitAdmissionError> {
        match intent {
            BoltV3OrderLifecycleIntent::Entry => Ok(Some(BoltV3SubmitIntentKind::Entry)),
            BoltV3OrderLifecycleIntent::RiskReducingExit => {
                Ok(Some(BoltV3SubmitIntentKind::RiskReducingExit))
            }
            BoltV3OrderLifecycleIntent::ReplaceSubmit if self.replace_submit => {
                Ok(Some(BoltV3SubmitIntentKind::ReplaceSubmit))
            }
            BoltV3OrderLifecycleIntent::ReplaceSubmit => Ok(None),
            BoltV3OrderLifecycleIntent::PlainCancel => Ok(None),
        }
    }

    fn allows(&self, intent: BoltV3SubmitIntentKind) -> bool {
        match intent {
            BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::RiskReducingExit => true,
            BoltV3SubmitIntentKind::ReplaceSubmit => self.replace_submit,
            BoltV3SubmitIntentKind::KillSwitchForcedReduction => true,
        }
    }
}

impl BoltV3SubmitIntentKind {
    pub fn is_venue_position_exit_clamp_eligible(self) -> bool {
        matches!(
            self,
            BoltV3SubmitIntentKind::RiskReducingExit
                | BoltV3SubmitIntentKind::KillSwitchForcedReduction
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
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    pub risk_reducing_exit_proof: Option<BoltV3RiskReducingExitProof>,
    pub kill_switch_forced_reduction: Option<BoltV3KillSwitchForcedReductionClaim>,
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

fn compiled_product_kind_evidence_value(product_kind: BoltV3CompiledProductKind) -> &'static str {
    match product_kind {
        BoltV3CompiledProductKind::PredictionMarketBinary => "prediction_market_binary",
    }
}

fn product_kind_evidence_value(product_kind: ProductKind) -> &'static str {
    match product_kind {
        ProductKind::PredictionMarketBinary => "prediction_market_binary",
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

fn compiled_order_side_evidence_value(side: BoltV3CompiledOrderSide) -> &'static str {
    match side {
        BoltV3CompiledOrderSide::Buy => "buy",
        BoltV3CompiledOrderSide::Sell => "sell",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CompiledOrderKind {
    Limit,
}

impl BoltV3CompiledOrderKind {
    fn to_capital_admission(self) -> IntentOrderKind {
        match self {
            Self::Limit => IntentOrderKind::Limit,
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
struct BoltV3CapitalAdmissionSubmitDecision {
    accepted: bool,
    reason: BoltV3CapitalAdmissionRejectReason,
    rollback: Option<BoltV3CapitalAdmissionReservationRollback>,
    reservation_metadata: Option<BoltV3SubmitReservationMetadataEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3BasketSubmitSlotClaim {
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
    pub order_side: OrderSide,
    pub order_quantity: Decimal,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
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
        lifecycle_policy: claim.lifecycle_policy,
        risk_reducing_exit_proof: claim.risk_reducing_exit_proof.clone(),
        kill_switch_forced_reduction: None,
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
    pub intent: &'a BoltV3OrderIntentEvidence,
    pub order: &'a OrderAny,
    pub valuation: OrderValuationContext<'a>,
    pub lifecycle_policy: BoltV3SubmitLifecyclePolicy,
    pub risk_reducing_exit_position: Option<BoltV3RiskReducingExitPositionInput<'a>>,
}

pub fn build_submit_admission_request_from_order<F>(
    input: BoltV3SubmitAdmissionRequestInput<'_>,
    max_fee_bps_for_price: F,
) -> anyhow::Result<BoltV3SubmitAdmissionRequest>
where
    F: FnOnce(Decimal) -> anyhow::Result<Decimal>,
{
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
    let max_fee_bps = max_fee_bps_for_price(price)?;
    let notional = if input.order.price().is_none() && !input.order.is_quote_quantity() {
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
    let notional = fee_inclusive_admission_notional(notional, max_fee_bps)?;
    let intent_kind = match input.intent.intent_kind {
        BoltV3OrderIntentKind::Entry => BoltV3SubmitIntentKind::Entry,
        BoltV3OrderIntentKind::Exit => BoltV3SubmitIntentKind::RiskReducingExit,
    };
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
                    exit_quantity: quantity,
                })
        } else {
            None
        };

    Ok(BoltV3SubmitAdmissionRequest {
        strategy_id: input.intent.strategy_id.clone(),
        execution_client_id: input.execution_client_id.to_string(),
        client_order_id,
        instrument_id: input.order.instrument_id().to_string(),
        notional,
        order_side: input.order.order_side(),
        order_quantity: quantity,
        intent_kind,
        lifecycle_policy: input.lifecycle_policy,
        risk_reducing_exit_proof,
        kill_switch_forced_reduction: None,
        admission_evidence: None,
    })
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

pub fn fee_inclusive_admission_notional(
    notional: Decimal,
    max_fee_bps: Decimal,
) -> Result<Decimal, BoltV3SubmitAdmissionError> {
    checked_fee_inclusive_admission_notional(notional, max_fee_bps)
        .ok_or(BoltV3SubmitAdmissionError::NotionalArithmeticOverflow)
}

pub(crate) fn checked_fee_inclusive_admission_notional(
    notional: Decimal,
    max_fee_bps: Decimal,
) -> Option<Decimal> {
    let fee_rate = max_fee_bps.checked_div(Decimal::from(SUBMIT_ADMISSION_BPS_DENOMINATOR))?;
    let fee_multiplier = Decimal::ONE.checked_add(fee_rate)?;
    notional.checked_mul(fee_multiplier)
}

/// Cap-bypass-via-rounding guard for submit paths that carry an operator
/// intent SEPARATE from the order actually built.
///
/// Callers must pass the base notional of the already-rounded order
/// (`rounded_base_notional`) — i.e. the product of the venue-precision
/// `Price`/`Quantity` actually submitted — together with the operator-intended
/// raw notional that authorized the order. Banker's rounding to venue precision
/// can round a quantity or price UP, so the rounded base notional can exceed the
/// intended notional. When that happens this helper fails CLOSED: a rounded
/// order may never debit more than the operator approved, so admission is
/// refused rather than letting the cap be bypassed by rounding.
///
/// On success it returns the fee-inclusive admission notional computed from the
/// rounded base, so the cap check downstream sees the same cash debit the venue
/// will incur.
///
/// Scope: this guard is required precisely for any path where the operator
/// approves an explicit `order_intent.notional` BEFORE the venue-precision order
/// is constructed. Paths that build the venue-precision order first and derive
/// admission notional from that already-rounded order structurally do not need
/// this guard: the strict-`>` cap check in [`BoltV3SubmitAdmissionState::admit`]
/// already evaluates the exact order handed to the venue — there is no separate
/// unrounded intent for rounding to bypass. Both paths share the same
/// fee-inclusive cap arithmetic via [`fee_inclusive_admission_notional`].
pub fn rounded_order_admission_notional(
    rounded_base_notional: Decimal,
    intended_notional: Decimal,
    max_fee_bps: Decimal,
) -> Result<Decimal, BoltV3SubmitAdmissionError> {
    if rounded_base_notional > intended_notional {
        return Err(BoltV3SubmitAdmissionError::RoundedNotionalExceedsIntent {
            rounded_base_notional,
            intended_notional,
        });
    }
    fee_inclusive_admission_notional(rounded_base_notional, max_fee_bps)
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

fn forced_reduction_admissible_halt_id(state: &KillSwitchState) -> Option<&str> {
    match state {
        KillSwitchState::Halting { halt_id, .. }
        | KillSwitchState::Halted { halt_id, .. }
        | KillSwitchState::Flattening { halt_id } => Some(halt_id),
        KillSwitchState::Armed
        | KillSwitchState::Cancelling { .. }
        | KillSwitchState::Flat { .. }
        | KillSwitchState::FailedManualIntervention { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3CapitalAdmissionRejectReason {
    Rejected,
    MissingAdmissionEvidence,
    VenueMismatch,
    AccountMismatch,
    ProductKindMismatch,
    CollateralCurrencyMismatch,
    UnsupportedProductKind,
    MissingPredictionMarketOutcome,
    OutcomeInstrumentMismatch,
    ReplaceSubmitUnsupported,
    DuplicateClientOrderId,
    OrderShapeMismatch,
    MissingNtState,
    StaleNtState,
    UnattributedNtState,
    ReconciliationRequired,
    OverBudget,
    CapitalAdmissionRejected,
    AcceptedQuantityMismatch,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum BoltV3SubmitAdmissionError {
    KillSwitchLatched {
        state: KillSwitchStateKind,
    },
    SubmitLifecycleDisallowed {
        intent: BoltV3SubmitIntentKind,
    },
    LossGovernorHalted {
        reasons: Vec<LossHaltReason>,
    },
    CountCapExhausted,
    NonPositiveNotional,
    NotionalCapExceeded,
    NotionalArithmeticOverflow,
    MissingPriceCeiling,
    RoundedNotionalExceedsIntent {
        rounded_base_notional: Decimal,
        intended_notional: Decimal,
    },
    ExchangeMutationCountOverflow,
    ExchangeMutationsObserved {
        mutation_count: u64,
    },
    InvalidRiskReducingExitProof,
    CapitalAdmissionRejected {
        reason: BoltV3CapitalAdmissionRejectReason,
    },
    KillSwitchForcedReductionProofInvalid,
    KillSwitchForcedReductionCapExceeded,
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
            Self::SubmitLifecycleDisallowed { intent } => write!(
                f,
                "bolt-v3 submit admission lifecycle policy disallows {intent:?} submit"
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
            Self::NotionalArithmeticOverflow => {
                write!(f, "bolt-v3 submit admission notional arithmetic overflowed")
            }
            Self::MissingPriceCeiling => write!(
                f,
                "bolt-v3 submit admission refuses a market-style order without a declared instrument price ceiling"
            ),
            Self::RoundedNotionalExceedsIntent {
                rounded_base_notional,
                intended_notional,
            } => write!(
                f,
                "bolt-v3 submit admission rejected: rounded order notional {rounded_base_notional} exceeded operator-intended notional {intended_notional}"
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
            Self::CapitalAdmissionRejected { reason } => {
                write!(
                    f,
                    "bolt-v3 submit admission capital admission rejected: {reason:?}"
                )
            }
            Self::KillSwitchForcedReductionProofInvalid => write!(
                f,
                "bolt-v3 submit admission kill-switch forced reduction proof is invalid"
            ),
            Self::KillSwitchForcedReductionCapExceeded => write!(
                f,
                "bolt-v3 submit admission kill-switch forced reduction cap is exceeded"
            ),
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
    let Some(capital_admission) = inner.capital_admission.as_mut() else {
        return accepted_without_reservation();
    };
    if request.intent_kind == BoltV3SubmitIntentKind::ReplaceSubmit {
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::ReplaceSubmitUnsupported,
        );
    }
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
        .venue_truth_capture_failure_source
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
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::CapitalAdmissionRejected,
        );
    }

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
    if !decision.accepted {
        return rejected_capital_admission(map_capital_admission_rejection(&decision.reasons));
    }
    if decision.accepted_quantity != Some(evidence.quantity) {
        capital_admission.gate.rollback_uncommitted_reservation(
            &capital_admission.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::AcceptedQuantityMismatch,
        );
    }
    let admitted_quantity = decision
        .accepted_quantity
        .expect("accepted capital admission decision should carry accepted quantity");
    let reserved_liability = decision
        .reserved_liability
        .expect("accepted capital admission decision should carry liability");
    let Some(additive_liability) = checked_additive_liability(&capital_admission.policy) else {
        capital_admission.gate.rollback_uncommitted_reservation(
            &capital_admission.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_capital_admission(
            BoltV3CapitalAdmissionRejectReason::CapitalAdmissionRejected,
        );
    };
    let liability_factor = match evidence.side.to_capital_admission() {
        IntentSide::Buy => evidence.effective_price,
        IntentSide::Sell => Decimal::ZERO,
    };
    let reservation_metadata = BoltV3SubmitReservationMetadataEvidence {
        client_order_id: request.client_order_id.clone(),
        submit_reservation_id: submit_reservation_id.clone(),
        venue_id: capital_admission.venue_id.clone(),
        account_id: capital_admission.account_id.clone(),
        product_kind: compiled_product_kind_evidence_value(evidence.product_kind).to_string(),
        collateral_currency: capital_admission.collateral_currency.clone(),
        capital_pool_id: capital_admission.capital_pool.pool_id.clone(),
        collateral_group_id: product.collateral_coupled_group_id.clone(),
        instrument_id: request.instrument_id.clone(),
        side: compiled_order_side_evidence_value(evidence.side).to_string(),
        submitted_quantity: admitted_quantity.to_string(),
        liability_factor: liability_factor.to_string(),
        additive_liability: additive_liability.to_string(),
        reserved_liability: reserved_liability.to_string(),
        observed_at_ns: now_ns,
        source: "submit_admission".to_string(),
    };
    capital_admission.client_order_reservations.insert(
        request.client_order_id.clone(),
        BoltV3SubmitReservationIndex {
            submit_reservation_id: submit_reservation_id.clone(),
            collateral_group_id: product.collateral_coupled_group_id.clone(),
            fill_metadata: Some(BoltV3SubmitReservationFillMetadata {
                instrument_id: request.instrument_id.clone(),
                side: evidence.side,
                original_quantity: admitted_quantity,
                filled_quantity: Decimal::ZERO,
                liability_factor,
                additive_liability,
                last_lifecycle_observed_at_ns: now_ns,
                seen_trade_ids: BTreeMap::new(),
                recovered_from_startup: false,
            }),
        },
    );
    BoltV3CapitalAdmissionSubmitDecision {
        accepted: true,
        reason: BoltV3CapitalAdmissionRejectReason::Rejected,
        rollback: Some(BoltV3CapitalAdmissionReservationRollback {
            client_order_id: request.client_order_id.clone(),
            submit_reservation_id,
            pool_id: capital_admission.capital_pool.pool_id.clone(),
            observed_at_ns: now_ns,
        }),
        reservation_metadata: Some(reservation_metadata),
    }
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
    mut components: BoltV3SubmitCapitalAdmissionNtComponents,
) {
    preserve_fresher_order_lifecycle(capital_admission.state.as_ref(), &mut components);
    if capital_admission.gate.is_reconciled()
        && components.order_lifecycle.open_order_count > 0
        && !components.order_lifecycle.all_open_orders_attributed
    {
        capital_admission.gate = CapitalAdmissionGate::unreconciled();
    }
    if !capital_admission.gate.is_reconciled()
        && capital_admission.client_order_reservations.is_empty()
        && components.order_lifecycle.open_order_count == 0
        && components.order_lifecycle.all_open_orders_attributed
    {
        capital_admission.gate = CapitalAdmissionGate::reconciled();
    }
    let state = compose_capital_admission_state_from_components(
        components,
        capital_admission.gate.is_reconciled()
            && capital_admission
                .venue_truth_capture_failure_source
                .is_none(),
        capital_admission.latest_reservation_mutation_observed_at_ns,
    );
    capital_admission.capital_pool.source = state.portfolio.source.clone();
    capital_admission.capital_pool.observed_at_ns = state.portfolio.observed_at_ns;
    capital_admission.state = Some(state);
}

fn preserve_fresher_order_lifecycle(
    current_state: Option<&NtDerivedCapitalAdmissionState>,
    components: &mut BoltV3SubmitCapitalAdmissionNtComponents,
) {
    let Some(current_state) = current_state else {
        return;
    };
    let current = &current_state.order_lifecycle;
    let incoming = &components.order_lifecycle;
    let current_is_newer = current.observed_at_ns > incoming.observed_at_ns;
    let incoming_is_same_open_set_downgrade = current.observed_at_ns == incoming.observed_at_ns
        && current.open_order_count == incoming.open_order_count
        && current.all_open_orders_attributed
        && !incoming.all_open_orders_attributed;
    if current_is_newer || incoming_is_same_open_set_downgrade {
        components.order_lifecycle = current.clone();
    }
}

fn order_lifecycle_is_accepted_venue_truth(
    order_lifecycle: &OrderLifecycleCapitalAdmissionSnapshot,
) -> bool {
    capital_admission_source_is_accepted_venue_truth(&order_lifecycle.source)
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
        .max(state.venue_spendability.observed_at_ns)
        .max(state.order_lifecycle.observed_at_ns)
        .max(product_observed_at_ns);
    if let Some(loss_snapshot) = state.loss_snapshot.as_ref() {
        observed_at_ns = observed_at_ns.max(loss_snapshot.observed_at_ns);
    }
    BoltV3SubmitCapitalAdmissionNtComponents {
        source: state.source,
        observed_at_ns,
        portfolio: state.portfolio,
        venue_spendability: state.venue_spendability,
        order_lifecycle: state.order_lifecycle,
        product_state: state.product_state,
        loss_snapshot: state.loss_snapshot,
    }
}

fn rebuilt_open_order_reservation_metadata_valid(
    reservation: &BoltV3SubmitCapitalAdmissionOpenOrderReservation,
) -> bool {
    if reservation.instrument_id.trim().is_empty()
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
        venue_spendability: components.venue_spendability,
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
    BoltV3CapitalAdmissionSubmitDecision {
        accepted: true,
        reason: BoltV3CapitalAdmissionRejectReason::Rejected,
        rollback: None,
        reservation_metadata: None,
    }
}

fn rejected_capital_admission(
    reason: BoltV3CapitalAdmissionRejectReason,
) -> BoltV3CapitalAdmissionSubmitDecision {
    BoltV3CapitalAdmissionSubmitDecision {
        accepted: false,
        reason,
        rollback: None,
        reservation_metadata: None,
    }
}

fn rollback_capital_admission_reservation(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3CapitalAdmissionReservationRollback,
) {
    let Some(capital_admission) = inner.capital_admission.as_mut() else {
        return;
    };
    capital_admission
        .gate
        .rollback_uncommitted_reservation(&rollback.pool_id, &rollback.submit_reservation_id);
    if capital_admission
        .client_order_reservations
        .get(&rollback.client_order_id)
        .map(|current| current.submit_reservation_id.as_str())
        == Some(rollback.submit_reservation_id.as_str())
    {
        capital_admission
            .client_order_reservations
            .remove(&rollback.client_order_id);
    }
    refresh_capital_admission_reservation_snapshot(capital_admission, rollback.observed_at_ns);
}

fn rollback_capital_admission_reservations(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollbacks: &[BoltV3CapitalAdmissionReservationRollback],
) {
    for rollback in rollbacks.iter().rev() {
        rollback_capital_admission_reservation(inner, rollback);
    }
}

fn rollback_admission_counters(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3SubmitAdmissionCounterRollback,
) {
    inner.admitted_order_count = inner
        .admitted_order_count
        .saturating_sub(rollback.order_count);
    let mut remove_execution_client = false;
    if let Some(count) = inner
        .admitted_order_count_by_execution_client
        .get_mut(&rollback.execution_client_id)
    {
        *count = count.saturating_sub(rollback.order_count);
        remove_execution_client = *count == 0;
    }
    if remove_execution_client {
        inner
            .admitted_order_count_by_execution_client
            .remove(&rollback.execution_client_id);
    }
    inner.live_kill_switch_forced_reduction_order_count = inner
        .live_kill_switch_forced_reduction_order_count
        .saturating_sub(rollback.forced_reduction_count);
    if let Some(client_order_id) = rollback.forced_reduction_client_order_id.as_ref() {
        inner
            .live_kill_switch_forced_reduction_client_order_ids
            .remove(client_order_id);
    }
}

fn map_capital_admission_rejection(
    reasons: &[crate::bolt_v3_capital_admission::CapitalAdmissionReason],
) -> BoltV3CapitalAdmissionRejectReason {
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::MissingNtState
        )
    }) {
        return BoltV3CapitalAdmissionRejectReason::MissingNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::StaleNtState(_)
        )
    }) {
        return BoltV3CapitalAdmissionRejectReason::StaleNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::UnattributedNtState(_)
        )
    }) {
        return BoltV3CapitalAdmissionRejectReason::UnattributedNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::Reservation(
                crate::bolt_v3_capital_reservation::ReservationRejectionReason::ReconciliationRequired,
            )
        )
    }) {
        return BoltV3CapitalAdmissionRejectReason::ReconciliationRequired;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_capital_admission::CapitalAdmissionReason::Reservation(
                crate::bolt_v3_capital_reservation::ReservationRejectionReason::OverBudget,
            )
        )
    }) {
        return BoltV3CapitalAdmissionRejectReason::OverBudget;
    }
    BoltV3CapitalAdmissionRejectReason::CapitalAdmissionRejected
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
mod loss_governor_halt_evidence_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Debug, Default)]
    struct FailingLossGovernorHaltEvidenceWriter {
        halts: Mutex<Vec<BoltV3LossGovernorHaltEvidence>>,
    }

    impl FailingLossGovernorHaltEvidenceWriter {
        fn halts(&self) -> Vec<BoltV3LossGovernorHaltEvidence> {
            self.halts
                .lock()
                .expect("loss-governor halt writer mutex poisoned")
                .clone()
        }
    }

    impl BoltV3DecisionEvidenceWriter for FailingLossGovernorHaltEvidenceWriter {
        fn try_record_command(
            &self,
            command: crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceCommand,
        ) -> anyhow::Result<()> {
            if let crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceCommand::LossGovernorHalt(evidence) = command {
                self.halts
                    .lock()
                    .expect("loss-governor halt writer mutex poisoned")
                    .push(evidence.clone());
                anyhow::bail!(
                    "injected loss-governor halt evidence write failure for {}",
                    evidence.stable_halt_key
                );
            }
            Ok(())
        }

        fn drain_shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct CapturingLogger {
        records: Mutex<Vec<(log::Level, String)>>,
    }

    impl log::Log for CapturingLogger {
        fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &log::Record<'_>) {
            self.records
                .lock()
                .expect("capturing logger mutex poisoned")
                .push((record.level(), record.args().to_string()));
        }

        fn flush(&self) {}
    }

    impl CapturingLogger {
        fn reset(&self) {
            self.records
                .lock()
                .expect("capturing logger mutex poisoned")
                .clear();
        }

        fn records(&self) -> Vec<(log::Level, String)> {
            self.records
                .lock()
                .expect("capturing logger mutex poisoned")
                .clone()
        }
    }

    static CAPTURING_LOGGER: std::sync::OnceLock<&'static CapturingLogger> =
        std::sync::OnceLock::new();
    static CAPTURING_LOGGER_OBSERVERS: Mutex<()> = Mutex::new(());
    static NEXT_LOSS_HALT_SOURCE_ID: AtomicU64 = AtomicU64::new(0);

    fn install_capturing_logger() -> &'static CapturingLogger {
        static INSTALL_OUTCOME: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let logger =
            CAPTURING_LOGGER.get_or_init(|| Box::leak(Box::new(CapturingLogger::default())));
        let installed = *INSTALL_OUTCOME.get_or_init(|| log::set_logger(*logger).is_ok());
        assert!(
            installed,
            "capturing logger could not claim the global log slot; another logger is installed"
        );
        log::set_max_level(log::LevelFilter::Trace);
        *logger
    }

    fn stale_loss_snapshot(source: String) -> LossSnapshot {
        LossSnapshot {
            source,
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
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
            risk_reducing_exit_proof: None,
            kill_switch_forced_reduction: None,
            admission_evidence: None,
        }
    }

    #[test]
    fn operator_health_snapshot_reports_poisoned_submit_admission_state() {
        let admission = BoltV3SubmitAdmissionState::new(Arc::new(
            FailingLossGovernorHaltEvidenceWriter::default(),
        ));
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
        let admission = BoltV3SubmitAdmissionState::new(Arc::new(
            FailingLossGovernorHaltEvidenceWriter::default(),
        ));
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
        let admission = BoltV3SubmitAdmissionState::new(Arc::new(
            FailingLossGovernorHaltEvidenceWriter::default(),
        ));
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
        let admission = BoltV3SubmitAdmissionState::new(Arc::new(
            FailingLossGovernorHaltEvidenceWriter::default(),
        ));
        let mut request = entry_request(
            "strategy-poisoned-rejected-episode".to_string(),
            "client-order-poisoned-rejected-episode".to_string(),
        );
        request.notional = Decimal::ZERO;
        let routing_admission = BoltV3SubmitAdmissionState::new(Arc::new(
            FailingLossGovernorHaltEvidenceWriter::default(),
        ));
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
        let logger = install_capturing_logger();
        let _observer_guard = CAPTURING_LOGGER_OBSERVERS
            .lock()
            .expect("capturing logger observer mutex poisoned");
        logger.reset();

        let source = format!(
            "loss_halt_admission_write_failure_source_{}",
            NEXT_LOSS_HALT_SOURCE_ID.fetch_add(1, Ordering::SeqCst)
        );
        let writer = Arc::new(FailingLossGovernorHaltEvidenceWriter::default());
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
        admission.update_loss_snapshot(stale_loss_snapshot(source.clone()));
        let request = entry_request(
            "strategy-loss-halt-log-guard".to_string(),
            "client-order-loss-halt-log-guard".to_string(),
        );

        let result = admission.admit_at(&request, 1_200);

        match result {
            Err(BoltV3SubmitAdmissionError::LossGovernorHalted { reasons }) => assert_eq!(
                reasons,
                vec![LossHaltReason::StaleLossSnapshot],
                "stale loss snapshot must still reject the submit request"
            ),
            Err(error) => panic!("expected loss-governor halt rejection, got {error:?}"),
            Ok(_) => panic!("expected loss-governor halt rejection, got permit"),
        }

        let halts = writer.halts();
        assert_eq!(
            halts.len(),
            1,
            "the failing writer must still receive the halt evidence"
        );
        assert_eq!(halts[0].snapshot_source.as_deref(), Some(source.as_str()));
        assert_eq!(halts[0].retry_count, 1);

        let stable_halt_key = halts[0].stable_halt_key.clone();
        let matching = logger
            .records()
            .into_iter()
            .filter(|(_, message)| {
                message.contains("loss governor halt evidence write failed")
                    && message.contains(&stable_halt_key)
            })
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
