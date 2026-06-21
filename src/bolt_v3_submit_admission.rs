use crate::bolt_v3_capital_reservation::{
    CapitalPoolSnapshot, ReservationRejectionReason, ReservationRequest,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3BasketAdmissionDecisionEvidence,
    BoltV3BasketAdmissionOutcome, BoltV3DecisionEvidenceWriter, BoltV3LossHaltReason,
    BoltV3LossSnapshotSource, BoltV3LossSnapshotStaleReason, BoltV3OrderIntentEvidence,
    BoltV3OrderIntentKind, BoltV3PositionSizerRebuildAuditEvidence,
    BoltV3RecoveredSubmitReservationEvidence, BoltV3SubmitReservationFillEvidence,
    BoltV3SubmitReservationMetadataEvidence, compiled_order_price_source,
};
use crate::bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind};
use crate::bolt_v3_loss_governor::{
    LossGovernorPolicy, LossHaltReason, LossSnapshot, LossSnapshotDiagnostics,
    LossSnapshotStaleReason, LossSourceObservationTimestamps,
    evaluate_loss_admission_with_observations,
};
use crate::bolt_v3_numeric::{is_positive_finite, notional_float_tolerance};
use crate::bolt_v3_observed_dedupe::prune_observed_dedupe_entries;
use crate::bolt_v3_position_sizer::{
    IntentLiquidity, IntentOrderKind, IntentSide, PositionSizingAdmissionGate,
    PositionSizingGateInputs, PositionSizingLifecycleAction, PositionSizingLifecycleKind,
    PositionSizingLifecycleUpdate, PositionSizingRequest, ProductKind, ProductSizingSnapshot,
    SizingPolicy,
};
use crate::bolt_v3_sizing_state::{
    NtDerivedSizingState, OrderLifecycleSizingSnapshot, PortfolioSizingSnapshot,
    ReservationLedgerSnapshot, VenueSpendabilitySnapshot,
};
use anyhow::Context;
use nautilus_model::{
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

const LOSS_SNAPSHOT_SOURCE_NT_LOSS_RUNTIME_FEED: &str = stringify!(nt_loss_runtime_feed);
const LOSS_SNAPSHOT_SOURCE_NT_PORTFOLIO_SNAPSHOT: &str = stringify!(nt_portfolio_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_SNAPSHOT: &str = stringify!(nt_account_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_AND_POSITION_SNAPSHOT: &str =
    stringify!(nt_account_and_position_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_EVENT: &str = stringify!(nt_position_event);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_CHANGED: &str = stringify!(nt_position_changed);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_CLOSED: &str = stringify!(nt_position_closed);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_ADJUSTED: &str = stringify!(nt_position_adjusted);
const LOSS_SNAPSHOT_SOURCE_NT_SIZING_STATE: &str = stringify!(nt_sizing_state);
const LOSS_SNAPSHOT_SOURCE_BOLT_LOSS_SNAPSHOT: &str = stringify!(bolt_loss_snapshot);
const LOSS_SNAPSHOT_SOURCE_LOSS_GOVERNOR: &str = stringify!(loss_governor);

const SUBMIT_ADMISSION_BPS_DENOMINATOR: u32 = 10_000;

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

#[derive(Debug)]
pub struct BoltV3SubmitAdmissionState {
    inner: Arc<Mutex<BoltV3SubmitAdmissionInner>>,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionInner {
    kill_switch_state: KillSwitchState,
    kill_switch_forced_reduction_policy: Option<BoltV3KillSwitchForcedReductionPolicy>,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    admitted_order_count: u32,
    admitted_order_count_by_execution_client: BTreeMap<String, u32>,
    live_kill_switch_forced_reduction_order_count: u32,
    loss_policy: Option<LossGovernorPolicy>,
    loss_snapshot: Option<LossSnapshot>,
    loss_source_observations: LossSourceObservationTimestamps,
    position_sizer: Option<BoltV3SubmitPositionSizerState>,
}

#[derive(Debug)]
struct BoltV3SubmitPositionSizerState {
    venue_id: String,
    account_id: String,
    product_kind: ProductKind,
    collateral_currency: String,
    capital_pool: CapitalPoolSnapshot,
    policy: SizingPolicy,
    dedupe_retention_ns: u64,
    state: Option<NtDerivedSizingState>,
    latest_reservation_mutation_observed_at_ns: Option<u64>,
    gate: PositionSizingAdmissionGate,
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
pub struct BoltV3SubmitPositionSizerConfig {
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: ProductKind,
    pub collateral_currency: String,
    pub capital_pool: CapitalPoolSnapshot,
    pub policy: SizingPolicy,
    pub dedupe_retention_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingNtComponents {
    pub source: String,
    pub observed_at_ns: u64,
    pub portfolio: PortfolioSizingSnapshot,
    pub venue_spendability: VenueSpendabilitySnapshot,
    pub order_lifecycle: OrderLifecycleSizingSnapshot,
    pub product_state: ProductSizingSnapshot,
    pub loss_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingOpenOrderReservation {
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
pub struct BoltV3SubmitPositionSizingOpenOrderSnapshot {
    pub observed_at_ns: u64,
    pub evidence_label: String,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub reservations: Vec<BoltV3SubmitPositionSizingOpenOrderReservation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingOpenOrderEvidence {
    pub client_order_id: String,
    pub instrument_id: String,
    pub side: BoltV3CompiledOrderSide,
    pub open_quantity: Decimal,
    pub limit_price: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingMissingNtAccountCacheBalance {
    pub account_id: String,
    pub collateral_currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingRebuildDecision {
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub rebuilt_reservation_count: usize,
    pub live_reserved_liability: Decimal,
    pub missing_nt_account_cache_balance:
        Option<BoltV3SubmitPositionSizingMissingNtAccountCacheBalance>,
}

impl BoltV3SubmitPositionSizingRebuildDecision {
    pub fn with_missing_nt_account_cache_balance(
        mut self,
        account_id: String,
        collateral_currency: String,
    ) -> Self {
        self.missing_nt_account_cache_balance =
            Some(BoltV3SubmitPositionSizingMissingNtAccountCacheBalance {
                account_id,
                collateral_currency,
            });
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitPositionSizingRebuildAuditContext {
    observed_at_ns: u64,
    source: String,
    observed_open_order_count: usize,
    all_open_orders_attributed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingFillUpdate {
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
pub struct BoltV3SubmitPositionSizingLifecycleUpdate {
    pub client_order_id: String,
    pub collateral_group_id: String,
    pub remaining_liability: Decimal,
    pub observed_at_ns: u64,
    pub evidence_label: String,
    pub kind: PositionSizingLifecycleKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitPositionSizingLifecycleDecision {
    pub accepted: bool,
    pub unknown_reservation: bool,
    pub action: PositionSizingLifecycleAction,
}

impl BoltV3SubmitPositionSizingLifecycleDecision {
    fn unknown() -> Self {
        Self {
            accepted: true,
            unknown_reservation: true,
            action: PositionSizingLifecycleAction::None,
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
        position_sizer: Option<BoltV3SubmitPositionSizerConfig>,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            live_submit_approval_limits,
            loss_policy,
            position_sizer,
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

    pub fn new_with_position_sizer(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        position_sizer: BoltV3SubmitPositionSizerConfig,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            BTreeMap::new(),
            None,
            Some(position_sizer),
        )
    }

    pub fn new_with_loss_governor_and_position_sizer(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        loss_policy: LossGovernorPolicy,
        position_sizer: BoltV3SubmitPositionSizerConfig,
    ) -> Self {
        Self::new_with_optional_controls(
            decision_evidence,
            BTreeMap::new(),
            Some(loss_policy),
            Some(position_sizer),
        )
    }

    fn new_with_optional_controls(
        decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
        live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
        loss_policy: Option<LossGovernorPolicy>,
        position_sizer: Option<BoltV3SubmitPositionSizerConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(BoltV3SubmitAdmissionInner {
                kill_switch_state: KillSwitchState::Armed,
                kill_switch_forced_reduction_policy: None,
                live_submit_approval_limits,
                admitted_order_count: 0,
                admitted_order_count_by_execution_client: BTreeMap::new(),
                live_kill_switch_forced_reduction_order_count: 0,
                loss_policy,
                loss_snapshot: None,
                loss_source_observations: LossSourceObservationTimestamps::default(),
                position_sizer: position_sizer.map(|config| BoltV3SubmitPositionSizerState {
                    venue_id: config.venue_id,
                    account_id: config.account_id,
                    product_kind: config.product_kind,
                    collateral_currency: config.collateral_currency,
                    capital_pool: config.capital_pool,
                    policy: config.policy,
                    dedupe_retention_ns: config.dedupe_retention_ns,
                    state: None,
                    latest_reservation_mutation_observed_at_ns: None,
                    gate: PositionSizingAdmissionGate::unreconciled(),
                    next_sequence: 0,
                    client_order_reservations: BTreeMap::new(),
                }),
            })),
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

    pub fn update_loss_source_observations(
        &self,
        observations: LossSourceObservationTimestamps,
    ) {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_source_observations = observations;
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

    pub fn update_position_sizing_nt_components(
        &self,
        components: BoltV3SubmitPositionSizingNtComponents,
    ) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        if let Some(position_sizer) = inner.position_sizer.as_mut() {
            refresh_position_sizer_state_from_components(position_sizer, components);
        }
    }

    pub fn position_sizer_state_snapshot(&self) -> Option<NtDerivedSizingState> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .position_sizer
            .as_ref()?
            .state
            .clone()
    }

    pub fn position_sizer_state_observed_at_ns(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .position_sizer
            .as_ref()?
            .state
            .as_ref()
            .map(|state| state.observed_at_ns)
    }

    pub fn position_sizer_configured(&self) -> bool {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .position_sizer
            .is_some()
    }

    pub fn loss_governor_configured(&self) -> bool {
        self.inner
            .lock()
            .expect("submit admission state mutex should not be poisoned")
            .loss_policy
            .is_some()
    }

    pub fn position_sizer_live_reserved_liability(&self) -> Option<Decimal> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let position_sizer = inner.position_sizer.as_ref()?;
        Some(
            position_sizer
                .gate
                .live_reserved_liability(&position_sizer.capital_pool.pool_id),
        )
    }

    pub fn position_sizer_has_live_reservation(&self, client_order_id: &str) -> bool {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.position_sizer.as_ref().is_some_and(|position_sizer| {
            position_sizer
                .client_order_reservations
                .contains_key(client_order_id)
        })
    }

    pub fn position_sizer_reconciled(&self) -> Option<bool> {
        let inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let position_sizer = inner.position_sizer.as_ref()?;
        Some(position_sizer.gate.is_reconciled())
    }

    pub fn position_sizing_open_order_reservation_from_evidence(
        &self,
        evidence: BoltV3SubmitPositionSizingOpenOrderEvidence,
    ) -> Option<BoltV3SubmitPositionSizingOpenOrderReservation> {
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
        let position_sizer = inner.position_sizer.as_ref()?;
        let state = position_sizer.state.as_ref()?;
        let ProductSizingSnapshot::PredictionMarketBinary(product) = &state.product_state;
        if evidence.instrument_id != product.yes_instrument_id
            && evidence.instrument_id != product.no_instrument_id
        {
            return None;
        }
        let additive_liability = checked_additive_liability(&position_sizer.policy)?;
        if additive_liability < Decimal::ZERO {
            return None;
        }
        let liability_factor = match evidence.side.to_position_sizer() {
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
        Some(BoltV3SubmitPositionSizingOpenOrderReservation {
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

    pub fn position_sizing_open_order_reservation_from_known_metadata(
        &self,
        evidence: BoltV3SubmitPositionSizingOpenOrderEvidence,
        recovered: &BoltV3RecoveredSubmitReservationEvidence,
    ) -> Option<BoltV3SubmitPositionSizingOpenOrderReservation> {
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
        let position_sizer = inner.position_sizer.as_ref()?;
        let state = position_sizer.state.as_ref()?;
        let ProductSizingSnapshot::PredictionMarketBinary(product) = &state.product_state;
        let metadata = &recovered.metadata;
        if metadata.client_order_id != evidence.client_order_id
            || metadata.venue_id != position_sizer.venue_id
            || metadata.account_id != position_sizer.account_id
            || metadata.product_kind != product_kind_evidence_value(position_sizer.product_kind)
            || metadata.collateral_currency != position_sizer.collateral_currency
            || metadata.capital_pool_id != position_sizer.capital_pool.pool_id
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
        let expected_liability_factor = match evidence.side.to_position_sizer() {
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
        Some(BoltV3SubmitPositionSizingOpenOrderReservation {
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

    pub fn rebuild_position_sizing_open_order_reservations(
        &self,
        open_order_reservations: Vec<BoltV3SubmitPositionSizingOpenOrderReservation>,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingRebuildDecision {
        self.rebuild_position_sizing_open_order_snapshot(
            BoltV3SubmitPositionSizingOpenOrderSnapshot {
                observed_at_ns: now_ns,
                evidence_label: "bolt_recovered_open_order_reservations".to_string(),
                observed_open_order_count: open_order_reservations.len(),
                all_open_orders_attributed: true,
                reservations: open_order_reservations,
            },
            now_ns,
        )
    }

    pub fn rebuild_position_sizing_open_order_snapshot(
        &self,
        snapshot: BoltV3SubmitPositionSizingOpenOrderSnapshot,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingRebuildDecision {
        let rebuilt_order_lifecycle = OrderLifecycleSizingSnapshot {
            source: snapshot.evidence_label.clone(),
            observed_at_ns: snapshot.observed_at_ns,
            open_order_count: snapshot.observed_open_order_count,
            all_open_orders_attributed: snapshot.all_open_orders_attributed,
        };
        let audit_context = BoltV3SubmitPositionSizingRebuildAuditContext {
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
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingRebuildDecision {
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
                && let Some(state) = position_sizer.state.as_mut()
            {
                state.observed_at_ns = state.observed_at_ns.max(snapshot.observed_at_ns);
                state.order_lifecycle = rebuilt_order_lifecycle;
            }
            position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
            position_sizer.client_order_reservations.clear();
            refresh_position_sizer_reservation_snapshot_with_source(
                position_sizer,
                snapshot.observed_at_ns,
                snapshot.evidence_label,
                false,
            );
            let decision = BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: position_sizer
                    .gate
                    .live_reserved_liability(&position_sizer.capital_pool.pool_id),
                missing_nt_account_cache_balance: None,
            };
            return self.finish_position_sizer_rebuild(&mut inner, &audit_context, decision);
        }
        let Some(state) = position_sizer.state.as_ref() else {
            position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
            position_sizer.client_order_reservations.clear();
            let decision = BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count,
                rebuilt_reservation_count: 0,
                live_reserved_liability: position_sizer
                    .gate
                    .live_reserved_liability(&position_sizer.capital_pool.pool_id),
                missing_nt_account_cache_balance: None,
            };
            return self.finish_position_sizer_rebuild(&mut inner, &audit_context, decision);
        };
        position_sizer.capital_pool.source = state.portfolio.source.clone();
        position_sizer.capital_pool.observed_at_ns = state.portfolio.observed_at_ns;

        let mut rebuilt_index = BTreeMap::new();
        let mut reservation_requests = Vec::with_capacity(snapshot.reservations.len());
        for (index, reservation) in snapshot.reservations.into_iter().enumerate() {
            if rebuilt_index.contains_key(&reservation.client_order_id) {
                position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
                position_sizer.client_order_reservations.clear();
                let decision = BoltV3SubmitPositionSizingRebuildDecision {
                    accepted: false,
                    reason: Some(ReservationRejectionReason::DuplicateReservation),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: 0,
                    live_reserved_liability: position_sizer
                        .gate
                        .live_reserved_liability(&position_sizer.capital_pool.pool_id),
                    missing_nt_account_cache_balance: None,
                };
                return self.finish_position_sizer_rebuild(&mut inner, &audit_context, decision);
            }
            if !rebuilt_open_order_reservation_metadata_valid(&reservation) {
                position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
                position_sizer.client_order_reservations.clear();
                let decision = BoltV3SubmitPositionSizingRebuildDecision {
                    accepted: false,
                    reason: Some(ReservationRejectionReason::MissingEvidence),
                    attempted_reservation_count: index + 1,
                    rebuilt_reservation_count: 0,
                    live_reserved_liability: position_sizer
                        .gate
                        .live_reserved_liability(&position_sizer.capital_pool.pool_id),
                    missing_nt_account_cache_balance: None,
                };
                return self.finish_position_sizer_rebuild(&mut inner, &audit_context, decision);
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
                pool_id: position_sizer.capital_pool.pool_id.clone(),
                collateral_group_id,
                liability: reservation.liability,
                observed_at_ns,
                evidence_label: reservation.evidence_label,
            });
        }

        position_sizer.client_order_reservations.clear();
        let decision = position_sizer.gate.rebuild_open_order_reservations(
            &position_sizer.capital_pool,
            &reservation_requests,
            now_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted {
            position_sizer.client_order_reservations = rebuilt_index;
            if snapshot.observed_open_order_count > 0
                && let Some(state) = position_sizer.state.as_mut()
            {
                state.observed_at_ns = state.observed_at_ns.max(now_ns);
                state.order_lifecycle = rebuilt_order_lifecycle;
            }
            refresh_position_sizer_reservation_snapshot(position_sizer, now_ns);
        }

        let decision = BoltV3SubmitPositionSizingRebuildDecision {
            accepted: decision.accepted,
            reason: decision.reason,
            attempted_reservation_count: decision.attempted_reservation_count,
            rebuilt_reservation_count: decision.rebuilt_reservation_count,
            live_reserved_liability: decision.live_reserved_liability,
            missing_nt_account_cache_balance: None,
        };
        self.finish_position_sizer_rebuild(&mut inner, &audit_context, decision)
    }

    fn finish_position_sizer_rebuild(
        &self,
        inner: &mut BoltV3SubmitAdmissionInner,
        context: &BoltV3SubmitPositionSizingRebuildAuditContext,
        decision: BoltV3SubmitPositionSizingRebuildDecision,
    ) -> BoltV3SubmitPositionSizingRebuildDecision {
        let audit = BoltV3PositionSizerRebuildAuditEvidence {
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
            .record_position_sizer_rebuild_audit(&audit)
            .is_ok()
        {
            return decision;
        }

        if let Some(position_sizer) = inner.position_sizer.as_mut() {
            position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
            position_sizer.client_order_reservations.clear();
            refresh_position_sizer_reservation_snapshot_with_source(
                position_sizer,
                context.observed_at_ns,
                context.source.clone(),
                false,
            );
        }
        BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: decision.attempted_reservation_count,
            rebuilt_reservation_count: 0,
            live_reserved_liability: inner
                .position_sizer
                .as_ref()
                .map(|position_sizer| {
                    position_sizer
                        .gate
                        .live_reserved_liability(&position_sizer.capital_pool.pool_id)
                })
                .unwrap_or(Decimal::ZERO),
            missing_nt_account_cache_balance: None,
        }
    }

    pub fn apply_position_sizing_lifecycle_update(
        &self,
        update: BoltV3SubmitPositionSizingLifecycleUpdate,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingLifecycleDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let Some(index) = position_sizer
            .client_order_reservations
            .get(&update.client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received position-sizer lifecycle update for unknown client_order_id={}",
                update.client_order_id
            );
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let update_observed_at_ns = update.observed_at_ns;
        let lifecycle_update = PositionSizingLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: position_sizer.capital_pool.pool_id.clone(),
            collateral_group_id: update.collateral_group_id,
            remaining_liability: update.remaining_liability,
            observed_at_ns: update.observed_at_ns,
            evidence_label: update.evidence_label,
            kind: update.kind,
        };
        let decision = position_sizer.gate.apply_lifecycle_update(
            &position_sizer.capital_pool,
            &lifecycle_update,
            now_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted
            && update.kind == PositionSizingLifecycleKind::Terminal
            && position_sizer
                .client_order_reservations
                .get(&update.client_order_id)
                .map(|current| current.submit_reservation_id.as_str())
                == Some(index.submit_reservation_id.as_str())
        {
            position_sizer
                .client_order_reservations
                .remove(&update.client_order_id);
        }
        if decision.accepted {
            refresh_position_sizer_reservation_snapshot(position_sizer, update_observed_at_ns);
        }
        BoltV3SubmitPositionSizingLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
            action: decision.action,
        }
    }

    pub fn apply_position_sizing_fill_update(
        &self,
        update: BoltV3SubmitPositionSizingFillUpdate,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingLifecycleDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let dedupe_retention_ns = position_sizer.dedupe_retention_ns;
        let Some(index) = position_sizer
            .client_order_reservations
            .get(&update.client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received position-sizer fill update for unknown client_order_id={}",
                update.client_order_id
            );
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let Some(metadata) = index.fill_metadata.clone() else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        if update.trade_id.trim().is_empty()
            || update.fill_quantity <= Decimal::ZERO
            || update.instrument_id != metadata.instrument_id
            || update.side != metadata.side
        {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        }
        let mut metadata = metadata;
        prune_observed_dedupe_entries(
            &mut metadata.seen_trade_ids,
            update.observed_at_ns,
            dedupe_retention_ns,
        );
        if metadata.seen_trade_ids.contains_key(&update.trade_id) {
            return BoltV3SubmitPositionSizingLifecycleDecision {
                accepted: true,
                unknown_reservation: false,
                action: PositionSizingLifecycleAction::None,
            };
        }
        let Some(next_observed_at_ns) = metadata.last_lifecycle_observed_at_ns.checked_add(1)
        else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let lifecycle_observed_at_ns = update.observed_at_ns.max(next_observed_at_ns);
        if metadata.recovered_from_startup && update.reconciliation {
            if let Some(current) = position_sizer
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
            refresh_position_sizer_reservation_snapshot(position_sizer, lifecycle_observed_at_ns);
            return BoltV3SubmitPositionSizingLifecycleDecision {
                accepted: true,
                unknown_reservation: false,
                action: PositionSizingLifecycleAction::None,
            };
        }
        let lifecycle_now_ns = now_ns.max(lifecycle_observed_at_ns);
        let Some(unclamped_filled_quantity) =
            metadata.filled_quantity.checked_add(update.fill_quantity)
        else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let new_filled_quantity = metadata.original_quantity.min(unclamped_filled_quantity);
        let Some(remaining_quantity) = metadata.original_quantity.checked_sub(new_filled_quantity)
        else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let clamped = unclamped_filled_quantity > metadata.original_quantity;
        let remaining_liability = if remaining_quantity > Decimal::ZERO {
            let Some(remaining_liability) = checked_lifecycle_liability(
                remaining_quantity,
                metadata.liability_factor,
                metadata.additive_liability,
            ) else {
                return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
            };
            remaining_liability
        } else {
            Decimal::ZERO
        };
        let lifecycle_update = PositionSizingLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: position_sizer.capital_pool.pool_id.clone(),
            collateral_group_id: index.collateral_group_id.clone(),
            remaining_liability,
            observed_at_ns: lifecycle_observed_at_ns,
            evidence_label: if clamped {
                "nt_order_fill_clamped".to_string()
            } else {
                update.evidence_label.clone()
            },
            kind: if remaining_quantity > Decimal::ZERO {
                PositionSizingLifecycleKind::LiveResidual
            } else {
                PositionSizingLifecycleKind::Terminal
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
            return BoltV3SubmitPositionSizingLifecycleDecision {
                accepted: false,
                unknown_reservation: false,
                action: PositionSizingLifecycleAction::None,
            };
        }
        let decision = position_sizer.gate.apply_lifecycle_update(
            &position_sizer.capital_pool,
            &lifecycle_update,
            lifecycle_now_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted {
            if lifecycle_update.kind == PositionSizingLifecycleKind::Terminal {
                if position_sizer
                    .client_order_reservations
                    .get(&update.client_order_id)
                    .map(|current| current.submit_reservation_id.as_str())
                    == Some(index.submit_reservation_id.as_str())
                {
                    position_sizer
                        .client_order_reservations
                        .remove(&update.client_order_id);
                }
            } else if let Some(current) = position_sizer
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
            refresh_position_sizer_reservation_snapshot(position_sizer, lifecycle_observed_at_ns);
        }
        BoltV3SubmitPositionSizingLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
            action: decision.action,
        }
    }

    pub fn apply_position_sizing_terminal_order_event(
        &self,
        client_order_id: String,
        observed_at_ns: u64,
        evidence_label: String,
    ) -> BoltV3SubmitPositionSizingLifecycleDecision {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        let Some(position_sizer) = inner.position_sizer.as_mut() else {
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let Some(index) = position_sizer
            .client_order_reservations
            .get(&client_order_id)
            .cloned()
        else {
            log::warn!(
                "bolt-v3 submit admission received position-sizer terminal order event for unknown client_order_id={}",
                client_order_id
            );
            return BoltV3SubmitPositionSizingLifecycleDecision::unknown();
        };
        let lifecycle_update = PositionSizingLifecycleUpdate {
            intent_id: index.submit_reservation_id.clone(),
            pool_id: position_sizer.capital_pool.pool_id.clone(),
            collateral_group_id: index.collateral_group_id,
            remaining_liability: Decimal::ZERO,
            observed_at_ns,
            evidence_label,
            kind: PositionSizingLifecycleKind::Terminal,
        };
        let decision = position_sizer.gate.apply_lifecycle_update(
            &position_sizer.capital_pool,
            &lifecycle_update,
            observed_at_ns,
            position_sizer.policy.min_remaining_pool_balance,
        );
        if decision.accepted
            && position_sizer
                .client_order_reservations
                .get(&client_order_id)
                .map(|current| current.submit_reservation_id.as_str())
                == Some(index.submit_reservation_id.as_str())
        {
            position_sizer
                .client_order_reservations
                .remove(&client_order_id);
        }
        if decision.accepted {
            refresh_position_sizer_reservation_snapshot(position_sizer, observed_at_ns);
        }
        BoltV3SubmitPositionSizingLifecycleDecision {
            accepted: decision.accepted,
            unknown_reservation: false,
            action: decision.action,
        }
    }

    pub fn record_kill_switch_forced_reduction_terminal(&self) {
        let mut inner = self
            .inner
            .lock()
            .expect("submit admission state mutex should not be poisoned");
        inner.live_kill_switch_forced_reduction_order_count = inner
            .live_kill_switch_forced_reduction_order_count
            .saturating_sub(1);
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
        let mut evaluation = Self::evaluate(&mut inner, request, now_ns);
        let mut admitted_counter_update = None;
        if evaluation.outcome == BoltV3AdmissionOutcome::Admitted {
            let admitted_order_count_before = inner.admitted_order_count;
            let forced_reduction_order_count_before =
                inner.live_kill_switch_forced_reduction_order_count;
            let Some(next_admitted_order_count) = inner.admitted_order_count.checked_add(1) else {
                if let Some(rollback) = evaluation.rollback.as_ref() {
                    rollback_position_sizer_reservation(&mut inner, rollback);
                }
                evaluation.outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                if let Err(err) = self.record_admission_decision(request, &evaluation) {
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
                    rollback_position_sizer_reservation(&mut inner, rollback);
                }
                evaluation.outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                if let Err(err) = self.record_admission_decision(request, &evaluation) {
                    return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                        reason: format!("{err:#}"),
                    });
                }
                return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
            };
            let next_forced_reduction_count =
                if request.intent_kind == BoltV3SubmitIntentKind::KillSwitchForcedReduction {
                    let Some(next) = inner
                        .live_kill_switch_forced_reduction_order_count
                        .checked_add(1)
                    else {
                        if let Some(rollback) = evaluation.rollback.as_ref() {
                            rollback_position_sizer_reservation(&mut inner, rollback);
                        }
                        evaluation.outcome = BoltV3AdmissionOutcome::RejectedCountCapExhausted;
                        if let Err(err) = self.record_admission_decision(request, &evaluation) {
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
            let counter_rollback = BoltV3SubmitAdmissionCounterRollback {
                execution_client_id: request.execution_client_id.clone(),
                order_count: next_admitted_order_count.saturating_sub(admitted_order_count_before),
                forced_reduction_count: next_forced_reduction_count
                    .saturating_sub(forced_reduction_order_count_before),
            };
            admitted_counter_update = Some((
                next_admitted_order_count,
                next_execution_client_count,
                next_forced_reduction_count,
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
                rollback_position_sizer_reservation(&mut inner, rollback);
            }
            return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                reason: format!("{err:#}"),
            });
        }
        if let Err(err) = self.record_admission_decision(request, &evaluation) {
            if let Some(rollback) = evaluation.rollback.as_ref() {
                rollback_position_sizer_reservation(&mut inner, rollback);
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
            BoltV3AdmissionOutcome::RejectedPositionSizing => {
                Err(BoltV3SubmitAdmissionError::PositionSizingRejected {
                    reason: evaluation
                        .position_sizer_rejection
                        .unwrap_or(BoltV3PositionSizerRejectReason::Rejected),
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
        let evaluation = Self::evaluate(&mut inner, request, current_unix_ns()?);
        let record_result = self.record_admission_decision(request, &evaluation);
        if let Some(rollback) = evaluation.rollback.as_ref() {
            rollback_position_sizer_reservation(&mut inner, rollback);
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
        self.decision_evidence.record_admission_decision(&evidence)
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
            BoltV3AdmissionOutcome::RejectedPositionSizing => {
                Err(BoltV3SubmitAdmissionError::PositionSizingRejected {
                    reason: evaluation
                        .position_sizer_rejection
                        .unwrap_or(BoltV3PositionSizerRejectReason::Rejected),
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
            let evaluation = Self::evaluate(&mut inner, &request, now_ns);
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
                .record_basket_admission_decision(&evidence)
            {
                rollback_position_sizer_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_position_sizer_reservations(&mut inner, &rollbacks);
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
                .record_basket_admission_decision(&evidence)
            {
                rollback_position_sizer_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_position_sizer_reservations(&mut inner, &rollbacks);
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
                .record_basket_admission_decision(&evidence)
            {
                rollback_position_sizer_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_position_sizer_reservations(&mut inner, &rollbacks);
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
                .record_basket_admission_decision(&evidence)
            {
                rollback_position_sizer_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
            rollback_position_sizer_reservations(&mut inner, &rollbacks);
            return Err(BoltV3SubmitAdmissionError::CountCapExhausted);
        };

        let counter_rollback = BoltV3SubmitAdmissionCounterRollback {
            execution_client_id: execution_client_id.to_string(),
            order_count: claim_count,
            forced_reduction_count,
        };

        for metadata in &reservation_metadata {
            if let Err(err) = self
                .decision_evidence
                .record_submit_reservation_metadata(metadata)
            {
                rollback_position_sizer_reservations(&mut inner, &rollbacks);
                return Err(BoltV3SubmitAdmissionError::EvidenceWriteFailed {
                    reason: format!("{err:#}"),
                });
            }
        }

        if let Err(err) = self
            .decision_evidence
            .record_basket_admission_decision(&evidence)
        {
            rollback_position_sizer_reservations(&mut inner, &rollbacks);
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
            BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
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
        if let Some(loss_policy) = inner.loss_policy.as_ref()
            && matches!(
                request.intent_kind,
                BoltV3SubmitIntentKind::Entry | BoltV3SubmitIntentKind::ReplaceSubmit
            )
        {
            let decision = evaluate_loss_admission_with_observations(
                loss_policy,
                inner.loss_snapshot.as_ref(),
                now_ns,
                inner.loss_source_observations,
            );
            loss_snapshot_diagnostics = Some(decision.diagnostics.clone());
            if !decision.accepted {
                return BoltV3SubmitAdmissionEvaluation::loss_halt(
                    decision.halt_reasons,
                    decision.diagnostics,
                );
            }
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
        if inner.position_sizer.is_some()
            && matches!(
                request.intent_kind,
                BoltV3SubmitIntentKind::Entry
                    | BoltV3SubmitIntentKind::RiskReducingExit
                    | BoltV3SubmitIntentKind::ReplaceSubmit
            )
        {
            let decision = evaluate_position_sizer_submit(inner, request, now_ns);
            if !decision.accepted {
                return BoltV3SubmitAdmissionEvaluation::position_sizer_rejected(
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
        if request.notional <= Decimal::ZERO {
            return BoltV3AdmissionOutcome::RejectedNonPositiveNotional;
        }
        if request.notional > policy.max_notional_per_order() {
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
        | BoltV3AdmissionOutcome::RejectedPositionSizing
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

fn loss_snapshot_source_to_evidence(source: &str) -> BoltV3LossSnapshotSource {
    match source {
        LOSS_SNAPSHOT_SOURCE_NT_LOSS_RUNTIME_FEED => BoltV3LossSnapshotSource::NtLossRuntimeFeed,
        LOSS_SNAPSHOT_SOURCE_NT_PORTFOLIO_SNAPSHOT => BoltV3LossSnapshotSource::NtPortfolioSnapshot,
        LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_SNAPSHOT => BoltV3LossSnapshotSource::NtAccountSnapshot,
        LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_AND_POSITION_SNAPSHOT => {
            BoltV3LossSnapshotSource::NtAccountAndPositionSnapshot
        }
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_EVENT => BoltV3LossSnapshotSource::NtPositionEvent,
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_CHANGED => BoltV3LossSnapshotSource::NtPositionChanged,
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_CLOSED => BoltV3LossSnapshotSource::NtPositionClosed,
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_ADJUSTED => BoltV3LossSnapshotSource::NtPositionAdjusted,
        LOSS_SNAPSHOT_SOURCE_NT_SIZING_STATE => BoltV3LossSnapshotSource::NtSizingState,
        LOSS_SNAPSHOT_SOURCE_BOLT_LOSS_SNAPSHOT => BoltV3LossSnapshotSource::BoltLossSnapshot,
        LOSS_SNAPSHOT_SOURCE_LOSS_GOVERNOR => BoltV3LossSnapshotSource::LossGovernor,
        _ if source.trim().is_empty() => BoltV3LossSnapshotSource::Unknown,
        _ => BoltV3LossSnapshotSource::Other,
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
        BoltV3AdmissionOutcome::RejectedPositionSizing => {
            BoltV3SubmitAdmissionError::PositionSizingRejected {
                reason: BoltV3PositionSizerRejectReason::Rejected,
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
    rollbacks: Vec<BoltV3PositionSizerReservationRollback>,
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
        rollback_position_sizer_reservations(&mut inner, &self.rollbacks);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3SubmitAdmissionCounterRollback {
    execution_client_id: String,
    order_count: u32,
    forced_reduction_count: u32,
}

#[derive(Debug)]
struct BoltV3SubmitAdmissionEvaluation {
    outcome: BoltV3AdmissionOutcome,
    admission_now_ns: u64,
    loss_halt_reasons: Vec<LossHaltReason>,
    loss_snapshot_diagnostics: Option<LossSnapshotDiagnostics>,
    position_sizer_rejection: Option<BoltV3PositionSizerRejectReason>,
    rollback: Option<BoltV3PositionSizerReservationRollback>,
    reservation_metadata: Option<BoltV3SubmitReservationMetadataEvidence>,
}

impl BoltV3SubmitAdmissionEvaluation {
    fn without_loss_halt(outcome: BoltV3AdmissionOutcome, admission_now_ns: u64) -> Self {
        Self {
            outcome,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            position_sizer_rejection: None,
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
            position_sizer_rejection: None,
            rollback: None,
            reservation_metadata: None,
        }
    }

    fn position_sizer_rejected(
        reason: BoltV3PositionSizerRejectReason,
        admission_now_ns: u64,
    ) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::RejectedPositionSizing,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            position_sizer_rejection: Some(reason),
            rollback: None,
            reservation_metadata: None,
        }
    }

    fn admitted_with_rollback(
        rollback: Option<BoltV3PositionSizerReservationRollback>,
        reservation_metadata: Option<BoltV3SubmitReservationMetadataEvidence>,
        admission_now_ns: u64,
    ) -> Self {
        Self {
            outcome: BoltV3AdmissionOutcome::Admitted,
            admission_now_ns,
            loss_halt_reasons: Vec::new(),
            loss_snapshot_diagnostics: None,
            position_sizer_rejection: None,
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
    pub position_sizing: Option<BoltV3CompiledOrderSizingEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3CompiledOrderSizingEvidence {
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
    fn to_position_sizer(self) -> ProductKind {
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
    fn to_position_sizer(self) -> IntentSide {
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
    fn to_position_sizer(self) -> IntentOrderKind {
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
    fn to_position_sizer(self) -> IntentLiquidity {
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
struct BoltV3PositionSizerReservationRollback {
    client_order_id: String,
    submit_reservation_id: String,
    pool_id: String,
    observed_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BoltV3PositionSizerSubmitDecision {
    accepted: bool,
    reason: BoltV3PositionSizerRejectReason,
    rollback: Option<BoltV3PositionSizerReservationRollback>,
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
    pub position_sizing: Option<BoltV3CompiledOrderSizingEvidence>,
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
        position_sizing: claim.position_sizing.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct BoltV3SubmitAdmissionRequestInput<'a> {
    pub execution_client_id: &'a str,
    pub intent: &'a BoltV3OrderIntentEvidence,
    pub order: &'a OrderAny,
    pub instrument: Option<&'a InstrumentAny>,
    pub quote_quantity_last_price: Option<Price>,
    pub quote_quantity_reference_price: Option<Price>,
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
    let notional = if input.order.is_quote_quantity() {
        let instrument = input.instrument.with_context(|| {
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
            input.quote_quantity_last_price,
            input.quote_quantity_reference_price,
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
        position_sizing: None,
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
pub enum BoltV3PositionSizerRejectReason {
    Rejected,
    MissingSizingEvidence,
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
    SizingRejected,
    SizedQuantityMismatch,
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
    PositionSizingRejected {
        reason: BoltV3PositionSizerRejectReason,
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
            Self::PositionSizingRejected { reason } => {
                write!(
                    f,
                    "bolt-v3 submit admission position sizing rejected: {reason:?}"
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

fn evaluate_position_sizer_submit(
    inner: &mut BoltV3SubmitAdmissionInner,
    request: &BoltV3SubmitAdmissionRequest,
    now_ns: u64,
) -> BoltV3PositionSizerSubmitDecision {
    let Some(position_sizer) = inner.position_sizer.as_mut() else {
        return accepted_without_reservation();
    };
    if request.intent_kind == BoltV3SubmitIntentKind::ReplaceSubmit {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::ReplaceSubmitUnsupported);
    }
    let Some(evidence) = request.position_sizing.as_ref() else {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::MissingSizingEvidence);
    };
    if evidence.venue_id != position_sizer.venue_id {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::VenueMismatch);
    }
    let product_kind = evidence.product_kind.to_position_sizer();
    if product_kind != position_sizer.product_kind {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::ProductKindMismatch);
    }
    if product_kind != ProductKind::PredictionMarketBinary {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::UnsupportedProductKind);
    }
    if !compiled_order_side_matches_request(evidence.side, request.order_side)
        || evidence.quantity != request.order_quantity
    {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::OrderShapeMismatch);
    }
    if position_sizer
        .client_order_reservations
        .contains_key(&request.client_order_id)
    {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::DuplicateClientOrderId);
    }
    let Some(state) = position_sizer.state.as_ref() else {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::MissingNtState);
    };
    if state.portfolio.venue_id != position_sizer.venue_id {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::VenueMismatch);
    }
    if state.portfolio.account_id != position_sizer.account_id {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::AccountMismatch);
    }
    if state.portfolio.collateral_currency != position_sizer.collateral_currency {
        return rejected_position_sizer(
            BoltV3PositionSizerRejectReason::CollateralCurrencyMismatch,
        );
    }
    if !position_sizer.gate.is_reconciled() {
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::ReconciliationRequired);
    }
    let ProductSizingSnapshot::PredictionMarketBinary(product) = &state.product_state;
    let Some(outcome) = evidence.prediction_market_outcome else {
        return rejected_position_sizer(
            BoltV3PositionSizerRejectReason::MissingPredictionMarketOutcome,
        );
    };
    let outcome_position = match outcome {
        PredictionMarketOutcomeSide::Yes => {
            if request.instrument_id != product.yes_instrument_id {
                return rejected_position_sizer(
                    BoltV3PositionSizerRejectReason::OutcomeInstrumentMismatch,
                );
            }
            product.yes_position
        }
        PredictionMarketOutcomeSide::No => {
            if request.instrument_id != product.no_instrument_id {
                return rejected_position_sizer(
                    BoltV3PositionSizerRejectReason::OutcomeInstrumentMismatch,
                );
            }
            product.no_position
        }
    };

    if request.intent_kind == BoltV3SubmitIntentKind::RiskReducingExit {
        if evidence.side == BoltV3CompiledOrderSide::Sell && evidence.quantity <= outcome_position {
            return accepted_without_reservation();
        }
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::SizingRejected);
    }

    position_sizer.next_sequence += 1;
    let submit_reservation_id = format!(
        "{}#{}",
        request.client_order_id, position_sizer.next_sequence
    );
    let sizing_request = PositionSizingRequest {
        intent_id: submit_reservation_id.clone(),
        strategy_id: request.strategy_id.clone(),
        instrument_id: request.instrument_id.clone(),
        pool_id: position_sizer.capital_pool.pool_id.clone(),
        product_kind,
        side: evidence.side.to_position_sizer(),
        quantity: evidence.quantity,
        limit_price: evidence.effective_price,
        order_kind: evidence.order_kind.to_position_sizer(),
        liquidity: evidence.liquidity.to_position_sizer(),
        quote_set_id: evidence.quote_set_id.clone(),
        now_ns,
    };
    let decision = position_sizer
        .gate
        .evaluate_and_reserve(PositionSizingGateInputs {
            request: &sizing_request,
            state: Some(state),
            policy: &position_sizer.policy,
            loss_policy: None,
            capital_pool: &position_sizer.capital_pool,
        });
    if !decision.accepted {
        return rejected_position_sizer(map_sized_rejection(&decision.reasons));
    }
    if decision.sized_quantity != Some(evidence.quantity) {
        position_sizer.gate.rollback_uncommitted_reservation(
            &position_sizer.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::SizedQuantityMismatch);
    }
    let admitted_quantity = decision
        .sized_quantity
        .expect("accepted position sizing decision should carry sized quantity");
    let reserved_liability = decision
        .liability_after_sizing
        .expect("accepted position sizing decision should carry liability");
    let Some(additive_liability) = checked_additive_liability(&position_sizer.policy) else {
        position_sizer.gate.rollback_uncommitted_reservation(
            &position_sizer.capital_pool.pool_id,
            &submit_reservation_id,
        );
        return rejected_position_sizer(BoltV3PositionSizerRejectReason::SizingRejected);
    };
    let liability_factor = match evidence.side.to_position_sizer() {
        IntentSide::Buy => evidence.effective_price,
        IntentSide::Sell => Decimal::ZERO,
    };
    let reservation_metadata = BoltV3SubmitReservationMetadataEvidence {
        client_order_id: request.client_order_id.clone(),
        submit_reservation_id: submit_reservation_id.clone(),
        venue_id: position_sizer.venue_id.clone(),
        account_id: position_sizer.account_id.clone(),
        product_kind: compiled_product_kind_evidence_value(evidence.product_kind).to_string(),
        collateral_currency: position_sizer.collateral_currency.clone(),
        capital_pool_id: position_sizer.capital_pool.pool_id.clone(),
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
    position_sizer.client_order_reservations.insert(
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
    BoltV3PositionSizerSubmitDecision {
        accepted: true,
        reason: BoltV3PositionSizerRejectReason::Rejected,
        rollback: Some(BoltV3PositionSizerReservationRollback {
            client_order_id: request.client_order_id.clone(),
            submit_reservation_id,
            pool_id: position_sizer.capital_pool.pool_id.clone(),
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

fn checked_additive_liability(policy: &SizingPolicy) -> Option<Decimal> {
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

fn refresh_position_sizer_state_from_components(
    position_sizer: &mut BoltV3SubmitPositionSizerState,
    mut components: BoltV3SubmitPositionSizingNtComponents,
) {
    preserve_fresher_order_lifecycle(position_sizer.state.as_ref(), &mut components);
    if position_sizer.gate.is_reconciled()
        && components.order_lifecycle.open_order_count > 0
        && !components.order_lifecycle.all_open_orders_attributed
    {
        position_sizer.gate = PositionSizingAdmissionGate::unreconciled();
    }
    if !position_sizer.gate.is_reconciled()
        && position_sizer.client_order_reservations.is_empty()
        && components.order_lifecycle.open_order_count == 0
        && components.order_lifecycle.all_open_orders_attributed
    {
        position_sizer.gate = PositionSizingAdmissionGate::reconciled();
    }
    let state = compose_position_sizing_state_from_components(
        components,
        position_sizer.gate.is_reconciled(),
        position_sizer.latest_reservation_mutation_observed_at_ns,
    );
    position_sizer.capital_pool.source = state.portfolio.source.clone();
    position_sizer.capital_pool.observed_at_ns = state.portfolio.observed_at_ns;
    position_sizer.state = Some(state);
}

fn preserve_fresher_order_lifecycle(
    current_state: Option<&NtDerivedSizingState>,
    components: &mut BoltV3SubmitPositionSizingNtComponents,
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

fn refresh_position_sizer_reservation_snapshot(
    position_sizer: &mut BoltV3SubmitPositionSizerState,
    observed_at_ns: u64,
) {
    refresh_position_sizer_reservation_snapshot_with_source(
        position_sizer,
        observed_at_ns,
        "bolt_reservation_ledger".to_string(),
        position_sizer.gate.is_reconciled(),
    );
}

fn refresh_position_sizer_reservation_snapshot_with_source(
    position_sizer: &mut BoltV3SubmitPositionSizerState,
    observed_at_ns: u64,
    source: String,
    all_live_reservations_attributed: bool,
) {
    position_sizer.latest_reservation_mutation_observed_at_ns = Some(observed_at_ns);
    let Some(current_state) = position_sizer.state.take() else {
        return;
    };
    let components = nt_components_from_existing_position_sizer_state(current_state);
    let mut state = compose_position_sizing_state_from_components(
        components,
        all_live_reservations_attributed,
        position_sizer.latest_reservation_mutation_observed_at_ns,
    );
    state.reservation_snapshot.source = source;
    state.reservation_snapshot.observed_at_ns = observed_at_ns;
    state.reservation_snapshot.all_live_reservations_attributed = all_live_reservations_attributed;
    state.observed_at_ns = state.observed_at_ns.max(observed_at_ns);
    position_sizer.state = Some(state);
}

fn nt_components_from_existing_position_sizer_state(
    state: NtDerivedSizingState,
) -> BoltV3SubmitPositionSizingNtComponents {
    let product_observed_at_ns = match &state.product_state {
        ProductSizingSnapshot::PredictionMarketBinary(snapshot) => snapshot.observed_at_ns,
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
    BoltV3SubmitPositionSizingNtComponents {
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
    reservation: &BoltV3SubmitPositionSizingOpenOrderReservation,
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

fn compose_position_sizing_state_from_components(
    components: BoltV3SubmitPositionSizingNtComponents,
    gate_reconciled: bool,
    latest_reservation_mutation_observed_at_ns: Option<u64>,
) -> NtDerivedSizingState {
    let reservation_observed_at_ns = latest_reservation_mutation_observed_at_ns
        .map_or(components.observed_at_ns, |observed_at_ns| {
            components.observed_at_ns.max(observed_at_ns)
        });
    NtDerivedSizingState {
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

fn accepted_without_reservation() -> BoltV3PositionSizerSubmitDecision {
    BoltV3PositionSizerSubmitDecision {
        accepted: true,
        reason: BoltV3PositionSizerRejectReason::Rejected,
        rollback: None,
        reservation_metadata: None,
    }
}

fn rejected_position_sizer(
    reason: BoltV3PositionSizerRejectReason,
) -> BoltV3PositionSizerSubmitDecision {
    BoltV3PositionSizerSubmitDecision {
        accepted: false,
        reason,
        rollback: None,
        reservation_metadata: None,
    }
}

fn rollback_position_sizer_reservation(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollback: &BoltV3PositionSizerReservationRollback,
) {
    let Some(position_sizer) = inner.position_sizer.as_mut() else {
        return;
    };
    position_sizer
        .gate
        .rollback_uncommitted_reservation(&rollback.pool_id, &rollback.submit_reservation_id);
    if position_sizer
        .client_order_reservations
        .get(&rollback.client_order_id)
        .map(|current| current.submit_reservation_id.as_str())
        == Some(rollback.submit_reservation_id.as_str())
    {
        position_sizer
            .client_order_reservations
            .remove(&rollback.client_order_id);
    }
    refresh_position_sizer_reservation_snapshot(position_sizer, rollback.observed_at_ns);
}

fn rollback_position_sizer_reservations(
    inner: &mut BoltV3SubmitAdmissionInner,
    rollbacks: &[BoltV3PositionSizerReservationRollback],
) {
    for rollback in rollbacks.iter().rev() {
        rollback_position_sizer_reservation(inner, rollback);
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
}

fn map_sized_rejection(
    reasons: &[crate::bolt_v3_position_sizer::SizedAdmissionReason],
) -> BoltV3PositionSizerRejectReason {
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::MissingNtState
        )
    }) {
        return BoltV3PositionSizerRejectReason::MissingNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::StaleNtState(_)
        )
    }) {
        return BoltV3PositionSizerRejectReason::StaleNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::UnattributedNtState(_)
        )
    }) {
        return BoltV3PositionSizerRejectReason::UnattributedNtState;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::Reservation(
                crate::bolt_v3_capital_reservation::ReservationRejectionReason::ReconciliationRequired,
            )
        )
    }) {
        return BoltV3PositionSizerRejectReason::ReconciliationRequired;
    }
    if reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::bolt_v3_position_sizer::SizedAdmissionReason::Reservation(
                crate::bolt_v3_capital_reservation::ReservationRejectionReason::OverBudget,
            )
        )
    }) {
        return BoltV3PositionSizerRejectReason::OverBudget;
    }
    BoltV3PositionSizerRejectReason::SizingRejected
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
