use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU8, AtomicU16, AtomicU32, Ordering},
    },
};

use anyhow::{Context, Result, anyhow};
use nautilus_model::orders::{Order, OrderAny};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_capital_reservation::ReservationRejectionReason;
use crate::bolt_v3_config::LoadedBoltV3Config;
use crate::bolt_v3_numeric::Probability;
use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;
use crate::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
    RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceDiagnostic,
    RealizedVolSourceRejectReason, RealizedVolSourceStatus, ValidRealizedVol,
};
use crate::bolt_v3_timestamp_domain::LocalReceiveMs;
use crate::bolt_v3_venue_truth::VenueTruthDivergenceAlarmClass;
use crate::bolt_v3_venue_truth::{VenueTruthCaptureFailureEvidence, VenueTruthDivergenceEvidence};

fn serialize_optional_local_receive_ms<S>(
    value: &Option<LocalReceiveMs>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    value.map(LocalReceiveMs::value).serialize(serializer)
}

pub const BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION: u32 = 15;
pub const BOLT_V3_DECISION_EVIDENCE_GATE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BOLT_V3_ORDER_INTENT_GATE_ID: &str = "bolt_v3.order_intent";
pub const BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID: &str = "bolt_v3.capital_admission_rebuild";
pub const BOLT_V3_SUBMIT_ADMISSION_GATE_ID: &str = "bolt_v3.submit_admission";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID: &str = "bolt_v3.strategy_input_snapshot";
pub const BOLT_V3_ENTRY_SKIP_GATE_ID: &str = "bolt_v3.entry_skip";
pub const BOLT_V3_EXIT_DECISION_GATE_ID: &str = "bolt_v3.exit_decision";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID: &str = "bolt_v3.loss_governor_halt";
pub const BOLT_V3_REQUOTE_THROTTLE_GATE_ID: &str = "bolt_v3.requote_throttle";
pub const BOLT_V3_SETTLEMENT_GATE_ID: &str = "bolt_v3.settlement";
pub const BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID: &str = "bolt_v3.venue_truth_capture_failure";
pub const BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID: &str = "bolt_v3.venue_truth_divergence";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND: &str = "strategy_input_snapshot";
pub const BOLT_V3_ORDER_INTENT_RECORD_KIND: &str = "order_intent";
pub const BOLT_V3_ADMISSION_DECISION_RECORD_KIND: &str = "admission_decision";
pub const BOLT_V3_ENTRY_SKIP_RECORD_KIND: &str = "entry_skip";
pub const BOLT_V3_EXIT_DECISION_RECORD_KIND: &str = "exit_decision";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND: &str = "loss_governor_halt";
pub const BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND: &str = "requote_throttle";
pub const BOLT_V3_SETTLEMENT_RECORD_KIND: &str = "settlement";
pub const BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND: &str = "settlement_booking_error";
pub const BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND: &str = "terminal_settlement";
pub const BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND: &str = "venue_truth_capture_failure";
pub const BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND: &str = "venue_truth_divergence";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_SUBSYSTEM: &str = "loss_governor";
const BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND: &str = "basket_admission_decision";
const BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND: &str = "capital_admission_rebuild";
const BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND: &str = "submit_reservation_metadata";
const BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND: &str = "submit_reservation_fill";
const SUBMIT_RESERVATION_METADATA_PRODUCT_KIND_BINARY: &str = "prediction_market_binary";
const SUBMIT_RESERVATION_METADATA_SIDE_BUY: &str = "buy";
const SUBMIT_RESERVATION_METADATA_SIDE_SELL: &str = "sell";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT: &str = "current";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_NEXT: &str = "next";
pub const BOLT_V3_EXIT_EVALUATION_GATE_ID: &str = "bolt_v3.exit_evaluation";
pub const BOLT_V3_EXIT_EVALUATION_RECORD_KIND: &str = "exit_evaluation";
pub const BOLT_V3_ORDER_REJECT_GATE_ID: &str = "bolt_v3.order_reject";
pub const BOLT_V3_ORDER_REJECT_RECORD_KIND: &str = "order_reject";
pub const BOLT_V3_ORDER_LIFECYCLE_GATE_ID: &str = "bolt_v3.order_lifecycle";
pub const BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND: &str = "order_lifecycle";

// Pre-Capsule suppression is deliberately process-global and conservative: no
// volatile field can select storage, clear a bit, or create a new episode. The
// first three masks are the frozen six-byte legacy diagnostic boundary. The
// remaining masks close every non-recovery producer over an exact enum domain.
// Masked rows are non-authoritative diagnostics whose consumers tolerate
// conservative under-emission; recovery-bearing truth bypasses every mask.
const BLOCKED_RV_DOMAIN: u32 = 12;
const ENTRY_SKIP_DOMAIN: u32 = 16;
const REQUOTE_THROTTLE_DOMAIN: u32 = 12;
const BASKET_ADMISSION_DOMAIN: u32 = 12;
const EXIT_DECISION_DOMAIN: u32 = 4;
const EXIT_EVALUATION_DOMAIN: u32 = 24;
const LOSS_GOVERNOR_HALT_DOMAIN: u32 = 5;
pub(crate) const ORDER_REJECT_DOMAIN: u32 = 28;
const VENUE_TRUTH_CAPTURE_FAILURE_DOMAIN: u32 = 1;
const VENUE_TRUTH_DIVERGENCE_DOMAIN: u32 = 3;
pub const BOLT_V3_NON_RECOVERY_MAX_EMISSIONS: u32 = BLOCKED_RV_DOMAIN
    + ENTRY_SKIP_DOMAIN
    + REQUOTE_THROTTLE_DOMAIN
    + BASKET_ADMISSION_DOMAIN
    + EXIT_DECISION_DOMAIN
    + EXIT_EVALUATION_DOMAIN
    + LOSS_GOVERNOR_HALT_DOMAIN
    + ORDER_REJECT_DOMAIN
    + VENUE_TRUTH_CAPTURE_FAILURE_DOMAIN
    + VENUE_TRUTH_DIVERGENCE_DOMAIN;
const _: [(); 117] = [(); BOLT_V3_NON_RECOVERY_MAX_EMISSIONS as usize];

static BLOCKED_RV_NOVELTY: AtomicU16 = AtomicU16::new(0);
static ENTRY_SKIP_NOVELTY: AtomicU16 = AtomicU16::new(0);
static REQUOTE_THROTTLE_NOVELTY: AtomicU16 = AtomicU16::new(0);
const _: [(); 6] = [(); std::mem::size_of::<AtomicU16>() * 3];
static BASKET_ADMISSION_NOVELTY: AtomicU16 = AtomicU16::new(0);
static EXIT_DECISION_NOVELTY: AtomicU8 = AtomicU8::new(0);
static EXIT_EVALUATION_NOVELTY: AtomicU32 = AtomicU32::new(0);
static LOSS_GOVERNOR_HALT_NOVELTY: AtomicU8 = AtomicU8::new(0);
static ORDER_REJECT_NOVELTY: AtomicU32 = AtomicU32::new(0);
static VENUE_TRUTH_CAPTURE_FAILURE_NOVELTY: AtomicU8 = AtomicU8::new(0);
static VENUE_TRUTH_DIVERGENCE_NOVELTY: AtomicU8 = AtomicU8::new(0);

fn mark_u8_once(mask: &AtomicU8, index: u32, domain: u32) -> bool {
    debug_assert!(index < domain && domain <= u8::BITS);
    let bit = 1_u8 << index;
    mask.fetch_or(bit, Ordering::Relaxed) & bit == 0
}

fn mark_u16_once(mask: &AtomicU16, index: u32, domain: u32) -> bool {
    debug_assert!(index < domain && domain <= u16::BITS);
    let bit = 1_u16 << index;
    mask.fetch_or(bit, Ordering::Relaxed) & bit == 0
}

fn mark_u32_once(mask: &AtomicU32, index: u32, domain: u32) -> bool {
    debug_assert!(index < domain && domain <= u32::BITS);
    let bit = 1_u32 << index;
    mask.fetch_or(bit, Ordering::Relaxed) & bit == 0
}

pub trait BoltV3DecisionEvidenceWriter: std::fmt::Debug + Send + Sync {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()>;

    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()>;
    fn record_admission_decision(&self, decision: &BoltV3AdmissionDecisionEvidence) -> Result<()>;
    fn record_basket_admission_decision(
        &self,
        decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()>;
    fn record_capital_admission_rebuild_audit(
        &self,
        audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()>;
    fn record_submit_reservation_metadata(
        &self,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()>;
    fn record_submit_reservation_fill(
        &self,
        fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()>;

    fn record_entry_skip(&self, skip: &BoltV3EntrySkipEvidence) -> Result<()>;
    fn record_exit_decision(&self, decision: &BoltV3ExitDecisionEvidence) -> Result<()>;
    fn record_exit_evaluation(&self, evidence: &BoltV3ExitEvaluationEvidence) -> Result<()>;
    fn record_loss_governor_halt(&self, evidence: &BoltV3LossGovernorHaltEvidence) -> Result<()>;
    fn record_order_reject(&self, evidence: &BoltV3OrderRejectEvidence) -> Result<()>;
    fn record_order_lifecycle(&self, _evidence: &BoltV3OrderLifecycleEvidence) -> Result<()> {
        Ok(())
    }
    fn record_requote_throttle(&self, throttle: &BoltV3RequoteThrottleEvidence) -> Result<()>;
    fn record_settlement(&self, evidence: &BoltV3SettlementEvidence) -> Result<()>;
    fn record_settlement_booking_error(
        &self,
        evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> Result<()>;
    fn record_terminal_settlement(
        &self,
        _evidence: &BoltV3TerminalSettlementEvidence,
    ) -> Result<()> {
        anyhow::bail!("terminal settlement evidence writer is not configured")
    }
    fn record_venue_truth_capture_failure(
        &self,
        evidence: &VenueTruthCaptureFailureEvidence,
    ) -> Result<()> {
        let _ = evidence;
        Ok(())
    }
    fn record_venue_truth_divergence(&self, evidence: &VenueTruthDivergenceEvidence) -> Result<()> {
        let _ = evidence;
        Ok(())
    }
    fn drain_shutdown(&self) -> Result<()>;
}

/// Risk direction of a runtime trading decision, used by [`commit_decision`]
/// (and [`record_decision`]) to select the single repo-wide evidence-write
/// failure rule.
///
/// The rule exists because two requirements are in tension and must never be
/// resolved ad hoc at each call site (the historical source of inconsistent
/// `?`-vs-swallow handling):
///
/// * A decision that ADDS or RE-ENABLES exposure must fail closed if its
///   durable record cannot be written — we never take on new risk we could not
///   later reconstruct from the evidence stream.
/// * A decision that REDUCES or removes exposure (exit, cancel, flatten, halt)
///   must NEVER be blocked by an evidence-write failure — risk reduction is
///   more important than its own log line, so the failure is surfaced loudly
///   and the act proceeds anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskDirection {
    /// Opens or increases exposure, or re-enables trading. Fail closed.
    NewRisk,
    /// Reduces or removes exposure (exit / cancel / flatten / halt). Never
    /// blocked by an evidence-write failure.
    RiskReducing,
    /// Pure observation or bookkeeping with no exposure change. Never blocked.
    Neutral,
}

impl RiskDirection {
    /// Whether an evidence-write failure must abort the guarded action before
    /// it runs. Only risk-increasing decisions fail closed.
    #[must_use]
    pub const fn evidence_write_failure_blocks(self) -> bool {
        matches!(self, Self::NewRisk)
    }
}

/// The single chokepoint through which every irreversible trading action must
/// pass. It emits the action's durable decision-evidence record FIRST, applies
/// the one repo-wide write-failure rule keyed on [`RiskDirection`], then runs
/// the action.
///
/// `emit` writes the durable record (typically one `record_*` call on a
/// [`BoltV3DecisionEvidenceWriter`]). `act` performs the irreversible effect
/// (venue submit / cancel, NT trading-state flip). For [`RiskDirection::NewRisk`]
/// an `emit` failure aborts before `act` runs (record-before-act, fail closed).
/// For [`RiskDirection::RiskReducing`] / [`RiskDirection::Neutral`] an `emit`
/// failure is logged at `error` and `act` runs anyway, so a lost log can never
/// strand a risk reduction.
///
/// The irreversible act primitives (`submit_order_via_nt`, `cancel_order_via_nt`,
/// `cancel_all_orders_via_nt`, and the loss-governor / recovery
/// `set_trading_state`) are being adopted into this chokepoint incrementally.
/// This primitive owns the failure rule; later slices still need to route the
/// production act paths through it and add source-fence enforcement.
pub fn commit_decision<T>(
    risk: RiskDirection,
    emit: impl FnOnce() -> Result<()>,
    act: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if let Err(error) = emit() {
        if risk.evidence_write_failure_blocks() {
            return Err(error).context(
                "bolt-v3 decision evidence write failed for a risk-increasing action; \
                 aborting before the irreversible act (fail closed)",
            );
        }
        log::error!(
            "bolt-v3 decision evidence write failed for a risk-reducing/neutral action; \
             surfacing at error and proceeding so risk reduction is never blocked: error={error:#}"
        );
    }
    act()
}

/// Record-only variant of [`commit_decision`] for decisions whose "action" is
/// inaction — an entry skip, a fair-value block, a feed-health gate. Applies the
/// identical failure rule: a [`RiskDirection::NewRisk`] write failure propagates
/// as `Err`; a [`RiskDirection::RiskReducing`] / [`RiskDirection::Neutral`]
/// write failure is logged at `error` and swallowed so the caller proceeds.
pub fn record_decision(risk: RiskDirection, emit: impl FnOnce() -> Result<()>) -> Result<()> {
    commit_decision(risk, emit, || Ok(()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentKind {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3SubmitIntentKind {
    Entry,
    RiskReducingExit,
    ReplaceSubmit,
    KillSwitchForcedReduction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentClampNotEvaluatedReason {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum BoltV3OrderIntentClampOutcome {
    WithinBounds,
    Clamped {
        original_quantity: String,
    },
    Rejected,
    NotEvaluated {
        reason: BoltV3OrderIntentClampNotEvaluatedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderIntentEvidence {
    pub strategy_id: String,
    pub intent_kind: BoltV3OrderIntentKind,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
    pub clamp_outcome: Option<BoltV3OrderIntentClampOutcome>,
    pub order_fields: BoltV3OrderIntentOrderFields,
}

pub(crate) fn compiled_order_price_source(fallback_price: String, order: &OrderAny) -> String {
    selected_compiled_order_price_source(
        order.price().map(|price| price.to_string()),
        order.trigger_price().map(|price| price.to_string()),
        order.activation_price().map(|price| price.to_string()),
        fallback_price,
    )
}

fn selected_compiled_order_price_source(
    price: Option<String>,
    trigger_price: Option<String>,
    activation_price: Option<String>,
    fallback_price: String,
) -> String {
    price
        .or(trigger_price)
        .or(activation_price)
        .unwrap_or(fallback_price)
}

impl BoltV3OrderIntentEvidence {
    pub fn from_compiled_order(
        strategy_id: String,
        intent_kind: BoltV3OrderIntentKind,
        fallback_price: String,
        order: &OrderAny,
    ) -> Self {
        Self {
            strategy_id,
            intent_kind,
            instrument_id: order.instrument_id().to_string(),
            client_order_id: order.client_order_id().to_string(),
            order_side: order.order_side().to_string(),
            price: compiled_order_price_source(fallback_price, order),
            quantity: order.quantity().to_string(),
            clamp_outcome: None,
            order_fields: BoltV3OrderIntentOrderFields::from_order(order),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderIntentOrderFields {
    pub order_type: String,
    pub time_in_force: String,
    pub price: Option<String>,
    pub trigger_price: Option<String>,
    pub activation_price: Option<String>,
    pub trigger_type: Option<String>,
    pub trigger_instrument_id: Option<String>,
    pub trailing_offset: Option<String>,
    pub trailing_offset_type: Option<String>,
    pub expire_time_unix_nanos: Option<String>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

impl BoltV3OrderIntentOrderFields {
    pub fn from_order(order: &OrderAny) -> Self {
        Self {
            order_type: order.order_type().to_string(),
            time_in_force: order.time_in_force().to_string(),
            price: order.price().map(|price| price.to_string()),
            trigger_price: order.trigger_price().map(|price| price.to_string()),
            activation_price: order.activation_price().map(|price| price.to_string()),
            trigger_type: order
                .trigger_type()
                .map(|trigger_type| trigger_type.to_string()),
            trigger_instrument_id: order
                .trigger_instrument_id()
                .map(|trigger_instrument_id| trigger_instrument_id.to_string()),
            trailing_offset: order
                .trailing_offset()
                .map(|trailing_offset| trailing_offset.to_string()),
            trailing_offset_type: order
                .trailing_offset_type()
                .map(|trailing_offset_type| trailing_offset_type.to_string()),
            expire_time_unix_nanos: order
                .expire_time()
                .map(|expire_time| expire_time.as_u64().to_string()),
            is_post_only: order.is_post_only(),
            is_reduce_only: order.is_reduce_only(),
            is_quote_quantity: order.is_quote_quantity(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3RealizedVolatilitySourceDiagnosticEvidence {
    pub source_id: String,
    pub source_class: String,
    pub sample_kind: String,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub status: String,
    pub annualized_realized_volatility_decimal: Option<String>,
    pub measured_annualized_realized_volatility_decimal: Option<String>,
    pub noise_robust_annualized_realized_volatility_decimal: Option<String>,
    pub continuous_annualized_realized_volatility_decimal: Option<String>,
    pub jump_annualized_realized_volatility_decimal: Option<String>,
    pub first_sample_ts_ms: Option<u64>,
    pub last_sample_ts_ms: Option<u64>,
    pub raw_sample_count: usize,
    pub grid_sample_count: usize,
    pub coverage_ratio: String,
    pub max_inter_sample_gap_ms: Option<u64>,
    pub last_rejected_reason: Option<String>,
    pub last_rejected_event_ts_ms: Option<u64>,
    pub last_rejected_recv_ts_ms: Option<u64>,
    pub rejection_counters: BTreeMap<String, u64>,
    pub block_reason: Option<String>,
}

impl BoltV3RealizedVolatilitySourceDiagnosticEvidence {
    pub fn from_realized_vol_diagnostic(diagnostic: &RealizedVolSourceDiagnostic) -> Self {
        Self {
            source_id: diagnostic.source_id.clone(),
            source_class: realized_volatility_source_class_evidence_label(diagnostic.source_class)
                .to_string(),
            sample_kind: realized_volatility_sample_kind_evidence_label(diagnostic.sample_kind)
                .to_string(),
            enabled: diagnostic.enabled,
            counts_toward_quorum: diagnostic.counts_toward_quorum,
            status: realized_volatility_source_status_evidence_label(diagnostic.status).to_string(),
            annualized_realized_volatility_decimal: diagnostic
                .annualized_realized_vol_decimal
                .map(number_evidence),
            measured_annualized_realized_volatility_decimal: diagnostic
                .measured_annualized_realized_vol_decimal
                .map(number_evidence),
            noise_robust_annualized_realized_volatility_decimal: diagnostic
                .noise_robust_annualized_realized_vol_decimal
                .map(number_evidence),
            continuous_annualized_realized_volatility_decimal: diagnostic
                .continuous_annualized_realized_vol_decimal
                .map(number_evidence),
            jump_annualized_realized_volatility_decimal: diagnostic
                .jump_annualized_realized_vol_decimal
                .map(number_evidence),
            first_sample_ts_ms: diagnostic.first_sample_ts_ms,
            last_sample_ts_ms: diagnostic.last_sample_ts_ms,
            raw_sample_count: diagnostic.raw_sample_count,
            grid_sample_count: diagnostic.grid_sample_count,
            coverage_ratio: number_evidence(diagnostic.coverage_ratio),
            max_inter_sample_gap_ms: diagnostic.max_inter_sample_gap_ms,
            last_rejected_reason: diagnostic
                .last_rejected_reason
                .map(realized_volatility_reject_reason_evidence_label)
                .map(str::to_string),
            last_rejected_event_ts_ms: diagnostic.last_rejected_event_ts_ms,
            last_rejected_recv_ts_ms: diagnostic.last_rejected_recv_ts_ms,
            rejection_counters: diagnostic
                .rejection_counters
                .iter()
                .map(|(reason, count)| {
                    (
                        realized_volatility_reject_reason_evidence_label(*reason).to_string(),
                        *count,
                    )
                })
                .collect(),
            block_reason: diagnostic
                .block_reason
                .map(realized_volatility_block_reason_evidence_label)
                .map(str::to_string),
        }
    }
}

pub fn realized_volatility_aggregation_evidence_label(
    aggregation: RealizedVolAggregation,
) -> &'static str {
    match aggregation {
        RealizedVolAggregation::UpperQuantile { .. } => "upper_quantile",
        RealizedVolAggregation::Median => "median",
        RealizedVolAggregation::TrimmedMean { .. } => "trimmed_mean",
        RealizedVolAggregation::MedianWithUpperQuantileGuard { .. } => {
            "median_with_upper_quantile_guard"
        }
    }
}

pub fn realized_volatility_block_reason_evidence_label(
    reason: RealizedVolBlockReason,
) -> &'static str {
    match reason {
        RealizedVolBlockReason::InvalidConfig => "invalid_config",
        RealizedVolBlockReason::QuorumNotReady => "quorum_not_ready",
        RealizedVolBlockReason::SourceStale => "source_stale",
        RealizedVolBlockReason::CoverageBelowMinimum => "coverage_below_minimum",
        RealizedVolBlockReason::InterSampleGapExceeded => "inter_sample_gap_exceeded",
        RealizedVolBlockReason::SourceClassMismatch => "source_class_mismatch",
        RealizedVolBlockReason::SampleKindMismatch => "sample_kind_mismatch",
        RealizedVolBlockReason::CrossSourceDispersion => "cross_source_dispersion",
        RealizedVolBlockReason::AnnualizationBasisInvalid => "annualization_basis_invalid",
        RealizedVolBlockReason::NotWarm => "not_warm",
    }
}

/// Projects an RV-engine [`RealizedVolBlockReason`] onto the exit-evidence
/// [`BoltV3ExitRvSnapshotBlocker`]. Lives here, alongside its sibling
/// [`realized_volatility_block_reason_evidence_label`], so strategy code consumes
/// the projection without owning the realized-volatility block-reason taxonomy
/// (enforced by the strategy source fence).
pub fn realized_vol_blocker_to_exit_evidence(
    reason: RealizedVolBlockReason,
) -> BoltV3ExitRvSnapshotBlocker {
    match reason {
        RealizedVolBlockReason::InvalidConfig => BoltV3ExitRvSnapshotBlocker::InvalidConfig,
        RealizedVolBlockReason::QuorumNotReady => BoltV3ExitRvSnapshotBlocker::QuorumNotReady,
        RealizedVolBlockReason::SourceStale => BoltV3ExitRvSnapshotBlocker::SourceStale,
        RealizedVolBlockReason::CoverageBelowMinimum => {
            BoltV3ExitRvSnapshotBlocker::CoverageBelowMinimum
        }
        RealizedVolBlockReason::InterSampleGapExceeded => {
            BoltV3ExitRvSnapshotBlocker::InterSampleGapExceeded
        }
        RealizedVolBlockReason::SourceClassMismatch => {
            BoltV3ExitRvSnapshotBlocker::SourceClassMismatch
        }
        RealizedVolBlockReason::SampleKindMismatch => {
            BoltV3ExitRvSnapshotBlocker::SampleKindMismatch
        }
        RealizedVolBlockReason::CrossSourceDispersion => {
            BoltV3ExitRvSnapshotBlocker::CrossSourceDispersion
        }
        RealizedVolBlockReason::AnnualizationBasisInvalid => {
            BoltV3ExitRvSnapshotBlocker::AnnualizationBasisInvalid
        }
        RealizedVolBlockReason::NotWarm => BoltV3ExitRvSnapshotBlocker::NotWarm,
    }
}

pub fn realized_volatility_pricing_component_evidence_label(
    component: RealizedVolPricingComponent,
) -> &'static str {
    match component {
        RealizedVolPricingComponent::Measured => "measured",
        RealizedVolPricingComponent::NoiseRobust => "noise_robust",
        RealizedVolPricingComponent::Continuous => "continuous",
        RealizedVolPricingComponent::Forecast => "forecast",
    }
}

fn realized_volatility_source_class_evidence_label(
    source_class: RealizedVolSourceClass,
) -> &'static str {
    match source_class {
        RealizedVolSourceClass::SpotQuote => "spot_quote",
        RealizedVolSourceClass::Trade => "trade",
        RealizedVolSourceClass::Mark => "mark",
        RealizedVolSourceClass::Index => "index",
    }
}

fn realized_volatility_sample_kind_evidence_label(
    sample_kind: RealizedVolSampleKind,
) -> &'static str {
    match sample_kind {
        RealizedVolSampleKind::Midpoint => "midpoint",
        RealizedVolSampleKind::Trade => "trade",
        RealizedVolSampleKind::Mark => "mark",
        RealizedVolSampleKind::Index => "index",
    }
}

fn realized_volatility_source_status_evidence_label(
    status: RealizedVolSourceStatus,
) -> &'static str {
    match status {
        RealizedVolSourceStatus::Ready => "ready",
        RealizedVolSourceStatus::Blocked => "blocked",
        RealizedVolSourceStatus::DiagnosticOnly => "diagnostic_only",
        RealizedVolSourceStatus::Waiting => "waiting",
    }
}

fn realized_volatility_reject_reason_evidence_label(
    reason: RealizedVolSourceRejectReason,
) -> &'static str {
    match reason {
        RealizedVolSourceRejectReason::DisabledSource => "disabled_source",
        RealizedVolSourceRejectReason::InvalidPrice => "invalid_price",
        RealizedVolSourceRejectReason::SourceClassMismatch => "source_class_mismatch",
        RealizedVolSourceRejectReason::SampleKindMismatch => "sample_kind_mismatch",
        RealizedVolSourceRejectReason::EventTimeRegression => "event_time_regression",
        RealizedVolSourceRejectReason::DuplicateTimestamp => "duplicate_timestamp",
        RealizedVolSourceRejectReason::StaleSameEventUpdate => "stale_same_event_update",
        RealizedVolSourceRejectReason::ReceiveBeforeEvent => "receive_before_event",
        RealizedVolSourceRejectReason::EventReceiveLagExceeded => "event_receive_lag_exceeded",
    }
}

pub(crate) fn number_evidence(value: f64) -> String {
    value.to_string()
}

pub(crate) fn probability_evidence(probability: Probability) -> String {
    number_evidence(probability.value())
}

pub(crate) fn option_probability_evidence(probability: Option<Probability>) -> Option<String> {
    probability.map(probability_evidence)
}

pub(crate) fn option_number_evidence(value: Option<f64>) -> Option<String> {
    value.filter(|value| value.is_finite()).map(number_evidence)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OutcomeSide {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ForcedFlatReason {
    Freeze,
    StaleReference,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExposureOccupancy {
    PendingEntry,
    EntryReconcilePending,
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3EntryBlockReason {
    PhaseNotActive,
    MetadataMismatch,
    ActiveBookNotPriced,
    BookCrossed,
    IntervalOpenMissing,
    WarmupIncomplete,
    FeesNotReady,
    RecoveryMode,
    MarketCoolingDown,
    SpotSpikeCooldown,
    ForcedFlat(BoltV3ForcedFlatReason),
    OnePositionInvariant(BoltV3ExposureOccupancy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3BinaryOutcomeEdgeBlockReason {
    MissingOrderBook,
    InsufficientDepth,
    InvalidProbability,
    InvalidCost,
    UnsupportedOrderShape,
    EdgeBelowThreshold,
    SpreadOrSlippageWipedEdge,
    FeeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3EntryPricingBlockReason {
    SpotPriceMissing,
    ReferenceCurrentPriceStale,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    ThetaScalerUnavailable,
    UncertaintyBandUnavailable,
    FairProbabilityUnavailable,
    FeeUnavailable(BoltV3OutcomeSide),
    ExecutableEntryCostUnavailable(BoltV3OutcomeSide),
    ExecutableEdgeUnavailable(BoltV3OutcomeSide, BoltV3BinaryOutcomeEdgeBlockReason),
    SizedNotionalUnsupported(BoltV3OutcomeSide),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3EntrySkipReasonCategory {
    StrategyCoreNotRegistered,
    EntryGateBlocked,
    EntryPricingBlocked,
    NoSideSelected,
    SizedNotionalNotPositive,
    InstrumentIdMissing,
    InstrumentMissingFromCache,
    EntryPriceMissing,
    QuantityRoundingFailed,
    LimitNotionalExceedsSizedNotional,
    QuantityNotPositive,
    PositionContractInvalid,
    EntryPositionContractUnsupported,
    HistoricalEntryFeeUnavailable,
    OnePositionInvariantViolation,
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3EntrySkipEvidence {
    pub strategy_id: String,
    pub now_ms: u64,
    pub reason_category: BoltV3EntrySkipReasonCategory,
    pub unclassified_context: Option<String>,
    pub gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pub pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    pub market_id: Option<String>,
    pub phase: String,
    pub seconds_to_market_end: Option<u64>,
    pub spot_price: Option<String>,
    pub reference_current_price: Option<String>,
    pub fast_venue_available: bool,
    pub reference_current_price_available: bool,
    pub realized_vol: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub realized_vol_gate_result: Option<BoltV3RvGateResult>,
    #[serde(serialize_with = "serialize_optional_local_receive_ms")]
    pub realized_vol_receive_watermark_ms: Option<LocalReceiveMs>,
    pub realized_vol_snapshot: Option<BoltV3EntryRealizedVolatilitySnapshotEvidence>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub selected_side: Option<BoltV3OutcomeSide>,
    pub sized_notional: Option<String>,
    pub sized_worst_case_ev_bps: Option<String>,
    pub sized_edge_cents_per_share: Option<String>,
    pub theta_scaled_min_edge_bps: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub submission_blocked_reason: Option<BoltV3EntrySkipReasonCategory>,
    pub stale_reference_after_ms: Option<u64>,
    pub last_reference_ts_ms: Option<u64>,
    pub min_liquidity_required: Option<String>,
    pub liquidity_available: Option<String>,
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub fast_venue_incoherent: bool,
}

#[derive(Deserialize)]
struct BoltV3EntrySkipEvidenceWire {
    strategy_id: String,
    now_ms: u64,
    reason_category: BoltV3EntrySkipReasonCategory,
    unclassified_context: Option<String>,
    gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    market_id: Option<String>,
    phase: String,
    seconds_to_market_end: Option<u64>,
    spot_price: Option<String>,
    reference_current_price: Option<String>,
    fast_venue_available: Option<bool>,
    reference_current_price_available: Option<bool>,
    realized_vol: Option<String>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    realized_vol_gate_result: Option<BoltV3RvGateResult>,
    realized_vol_receive_watermark_ms: Option<u64>,
    realized_vol_snapshot: Option<BoltV3EntryRealizedVolatilitySnapshotEvidence>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    selected_side: Option<BoltV3OutcomeSide>,
    sized_notional: Option<String>,
    sized_worst_case_ev_bps: Option<String>,
    sized_edge_cents_per_share: Option<String>,
    theta_scaled_min_edge_bps: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    submission_blocked_reason: Option<BoltV3EntrySkipReasonCategory>,
    stale_reference_after_ms: Option<u64>,
    last_reference_ts_ms: Option<u64>,
    min_liquidity_required: Option<String>,
    liquidity_available: Option<String>,
    frozen: bool,
    metadata_matches_selection: bool,
    fast_venue_incoherent: bool,
}

impl<'de> Deserialize<'de> for BoltV3EntrySkipEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3EntrySkipEvidenceWire::deserialize(deserializer)?;
        let realized_vol_gate_result = wire.realized_vol_gate_result.or_else(|| {
            legacy_admitted_rv_fields(
                wire.realized_vol.as_deref(),
                wire.realized_vol_source_venue.as_deref(),
                wire.realized_vol_source_ts_ms,
            )
        });
        Ok(Self {
            strategy_id: wire.strategy_id,
            now_ms: wire.now_ms,
            reason_category: wire.reason_category,
            unclassified_context: wire.unclassified_context,
            gate_blocked_by: wire.gate_blocked_by,
            pricing_blocked_by: wire.pricing_blocked_by,
            market_id: wire.market_id,
            phase: wire.phase,
            seconds_to_market_end: wire.seconds_to_market_end,
            spot_price: wire.spot_price,
            reference_current_price: wire.reference_current_price,
            fast_venue_available: wire.fast_venue_available.unwrap_or(false),
            reference_current_price_available: wire
                .reference_current_price_available
                .unwrap_or(false),
            realized_vol: wire.realized_vol,
            realized_vol_source_venue: wire.realized_vol_source_venue,
            realized_vol_source_ts_ms: wire.realized_vol_source_ts_ms,
            realized_vol_gate_result,
            realized_vol_receive_watermark_ms: wire
                .realized_vol_receive_watermark_ms
                .map(LocalReceiveMs::new),
            realized_vol_snapshot: wire.realized_vol_snapshot,
            fair_probability_up: wire.fair_probability_up,
            fair_probability_down: wire.fair_probability_down,
            selected_side: wire.selected_side,
            sized_notional: wire.sized_notional,
            sized_worst_case_ev_bps: wire.sized_worst_case_ev_bps,
            sized_edge_cents_per_share: wire.sized_edge_cents_per_share,
            theta_scaled_min_edge_bps: wire.theta_scaled_min_edge_bps,
            up_fee_bps: wire.up_fee_bps,
            down_fee_bps: wire.down_fee_bps,
            submission_blocked_reason: wire.submission_blocked_reason,
            stale_reference_after_ms: wire.stale_reference_after_ms,
            last_reference_ts_ms: wire.last_reference_ts_ms,
            min_liquidity_required: wire.min_liquidity_required,
            liquidity_available: wire.liquidity_available,
            frozen: wire.frozen,
            metadata_matches_selection: wire.metadata_matches_selection,
            fast_venue_incoherent: wire.fast_venue_incoherent,
        })
    }
}

fn legacy_admitted_rv_fields(
    realized_vol: Option<&str>,
    source_venue: Option<&str>,
    source_ts_ms: Option<u64>,
) -> Option<BoltV3RvGateResult> {
    (realized_vol.is_some_and(valid_legacy_rv_value)
        && source_venue.is_some_and(|value| !value.is_empty())
        && source_ts_ms.is_some())
    .then_some(BoltV3RvGateResult::Accepted)
}

fn valid_legacy_rv_value(value: &str) -> bool {
    value
        .parse::<f64>()
        .is_ok_and(|value| ValidRealizedVol::new(value).is_some())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3EntryRealizedVolatilitySnapshotEvidence {
    pub surface_id: String,
    pub as_of_ms: Option<u64>,
    pub annualized_decimal: String,
    pub measured_annualized_decimal: String,
    pub noise_robust_annualized_decimal: String,
    pub continuous_annualized_decimal: String,
    pub jump_annualized_decimal: String,
    pub forecast_annualized_decimal: String,
    pub pricing_component: String,
    pub seconds_per_annum: String,
    pub aggregation: String,
    pub sources_used: Vec<String>,
    pub source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub unknown_source_rejections: BTreeMap<String, u64>,
    pub blockers: Vec<String>,
    pub config_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitDecisionOutcome {
    Exit,
    ExitFailClosed,
    Hold,
    Blocked,
}

/// Closed taxonomy of the runtime path that triggered an exit evaluation. Used by
/// `BoltV3ExitEvaluationEvidence` so the durable record explains *which clock base*
/// produced `exit_eval_now_ms` (the 2026-06-20 incident root cause was a
/// timestamp-base mismatch between the exit clock and the realized-vol snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitTriggerSource {
    SignalQuote,
    ReferenceUpdate,
    SelectionUpdate,
    BookDelta,
    Unknown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitRvSnapshotBlocker {
    InvalidConfig,
    QuorumNotReady,
    SourceStale,
    CoverageBelowMinimum,
    InterSampleGapExceeded,
    SourceClassMismatch,
    SampleKindMismatch,
    CrossSourceDispersion,
    AnnualizationBasisInvalid,
    NotWarm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitRvGateResult {
    Accepted,
    RejectedFutureDated,
    RejectedStale,
    RejectedNotReady,
    MissingSnapshot,
    MissingEvaluationEventTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitBlockedReason {
    NoOpenPosition,
    ExitAlreadyPending,
    EntryOrderStillWorking,
    ExitDecisionUnavailable,
    ExitHold,
    PositionIntervalEnded,
    PositionIntervalUnknown,
    OpenPositionMissing,
    ExitOrderConfigInvalid,
    ExitQuoteQuantityUnsupported,
    ExitPriceMissing,
    ExitQuantityNotPositive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3ExitDecisionEvidence {
    pub strategy_id: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub position_instrument_id: Option<String>,
    pub position_outcome_side: Option<BoltV3OutcomeSide>,
    pub forced_flat_reasons: Vec<BoltV3ForcedFlatReason>,
    pub spot_price: Option<String>,
    pub spot_venue_name: Option<String>,
    pub fast_venue_available: bool,
    pub reference_current_price: Option<String>,
    pub reference_current_price_available: bool,
    pub interval_open: Option<String>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub uncertainty_band_probability: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub hold_ev_bps: Option<String>,
    pub exit_ev_bps: Option<String>,
    pub realized_vol: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub exit_eval_now_ms: u64,
    pub exit_trigger_source: BoltV3ExitTriggerSource,
    pub trigger_ts_event_ms: u64,
    pub trigger_ts_init_ms: Option<u64>,
    pub rv_surface_id: String,
    pub rv_snapshot_as_of_ms: Option<u64>,
    pub rv_snapshot_ready: bool,
    pub rv_snapshot_has_ready_realized_vol: Option<bool>,
    pub rv_snapshot_receive_watermark_ms: Option<u64>,
    pub rv_max_source_age_ms: Option<u64>,
    pub rv_snapshot_blockers: Vec<BoltV3ExitRvSnapshotBlocker>,
    pub rv_source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub rv_gate_result: BoltV3ExitRvGateResult,
    pub rv_future_dating_delta_ms: Option<u64>,
    pub exit_hysteresis_bps: String,
    pub exit_decision: BoltV3ExitDecisionOutcome,
    pub blocked_reason: Option<BoltV3ExitBlockedReason>,
    pub client_order_id: Option<String>,
    pub submission_order_side: Option<String>,
    pub submission_price: Option<String>,
    pub submission_quantity: Option<String>,
    pub seconds_to_market_end: Option<u64>,
    pub ts_ms: u64,
    pub stale_reference_after_ms: Option<u64>,
    pub last_reference_ts_ms: Option<u64>,
    pub min_liquidity_required: Option<String>,
    pub liquidity_available: Option<String>,
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub fast_venue_incoherent: bool,
}

#[derive(Deserialize)]
struct BoltV3ExitDecisionEvidenceWire {
    strategy_id: String,
    market_id: Option<String>,
    position_id: Option<String>,
    position_instrument_id: Option<String>,
    position_outcome_side: Option<BoltV3OutcomeSide>,
    forced_flat_reasons: Vec<BoltV3ForcedFlatReason>,
    spot_price: Option<String>,
    spot_venue_name: Option<String>,
    fast_venue_available: Option<bool>,
    reference_current_price: Option<String>,
    reference_current_price_available: Option<bool>,
    interval_open: Option<String>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    uncertainty_band_probability: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    realized_vol: Option<String>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    exit_eval_now_ms: u64,
    exit_trigger_source: BoltV3ExitTriggerSource,
    trigger_ts_event_ms: u64,
    trigger_ts_init_ms: Option<u64>,
    rv_surface_id: String,
    rv_snapshot_as_of_ms: Option<u64>,
    rv_snapshot_ready: bool,
    rv_snapshot_has_ready_realized_vol: Option<bool>,
    rv_snapshot_receive_watermark_ms: Option<u64>,
    rv_max_source_age_ms: Option<u64>,
    rv_snapshot_blockers: Vec<BoltV3ExitRvSnapshotBlocker>,
    rv_source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    rv_gate_result: BoltV3ExitRvGateResult,
    rv_future_dating_delta_ms: Option<u64>,
    exit_hysteresis_bps: String,
    exit_decision: BoltV3ExitDecisionOutcome,
    blocked_reason: Option<BoltV3ExitBlockedReason>,
    client_order_id: Option<String>,
    submission_order_side: Option<String>,
    submission_price: Option<String>,
    submission_quantity: Option<String>,
    seconds_to_market_end: Option<u64>,
    ts_ms: u64,
    stale_reference_after_ms: Option<u64>,
    last_reference_ts_ms: Option<u64>,
    min_liquidity_required: Option<String>,
    liquidity_available: Option<String>,
    frozen: bool,
    metadata_matches_selection: bool,
    fast_venue_incoherent: bool,
}

impl<'de> Deserialize<'de> for BoltV3ExitDecisionEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3ExitDecisionEvidenceWire::deserialize(deserializer)?;
        Ok(Self {
            strategy_id: wire.strategy_id,
            market_id: wire.market_id,
            position_id: wire.position_id,
            position_instrument_id: wire.position_instrument_id,
            position_outcome_side: wire.position_outcome_side,
            forced_flat_reasons: wire.forced_flat_reasons,
            spot_price: wire.spot_price,
            spot_venue_name: wire.spot_venue_name,
            fast_venue_available: wire.fast_venue_available.unwrap_or(false),
            reference_current_price: wire.reference_current_price,
            reference_current_price_available: wire
                .reference_current_price_available
                .unwrap_or(false),
            interval_open: wire.interval_open,
            fair_probability_up: wire.fair_probability_up,
            fair_probability_down: wire.fair_probability_down,
            uncertainty_band_probability: wire.uncertainty_band_probability,
            up_fee_bps: wire.up_fee_bps,
            down_fee_bps: wire.down_fee_bps,
            hold_ev_bps: wire.hold_ev_bps,
            exit_ev_bps: wire.exit_ev_bps,
            realized_vol: wire.realized_vol,
            realized_vol_source_venue: wire.realized_vol_source_venue,
            realized_vol_source_ts_ms: wire.realized_vol_source_ts_ms,
            exit_eval_now_ms: wire.exit_eval_now_ms,
            exit_trigger_source: wire.exit_trigger_source,
            trigger_ts_event_ms: wire.trigger_ts_event_ms,
            trigger_ts_init_ms: wire.trigger_ts_init_ms,
            rv_surface_id: wire.rv_surface_id,
            rv_snapshot_as_of_ms: wire.rv_snapshot_as_of_ms,
            rv_snapshot_ready: wire.rv_snapshot_ready,
            rv_snapshot_has_ready_realized_vol: wire.rv_snapshot_has_ready_realized_vol,
            rv_snapshot_receive_watermark_ms: wire.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: wire.rv_max_source_age_ms,
            rv_snapshot_blockers: wire.rv_snapshot_blockers,
            rv_source_diagnostics: wire.rv_source_diagnostics,
            rv_gate_result: wire.rv_gate_result,
            rv_future_dating_delta_ms: wire.rv_future_dating_delta_ms,
            exit_hysteresis_bps: wire.exit_hysteresis_bps,
            exit_decision: wire.exit_decision,
            blocked_reason: wire.blocked_reason,
            client_order_id: wire.client_order_id,
            submission_order_side: wire.submission_order_side,
            submission_price: wire.submission_price,
            submission_quantity: wire.submission_quantity,
            seconds_to_market_end: wire.seconds_to_market_end,
            ts_ms: wire.ts_ms,
            stale_reference_after_ms: wire.stale_reference_after_ms,
            last_reference_ts_ms: wire.last_reference_ts_ms,
            min_liquidity_required: wire.min_liquidity_required,
            liquidity_available: wire.liquidity_available,
            frozen: wire.frozen,
            metadata_matches_selection: wire.metadata_matches_selection,
            fast_venue_incoherent: wire.fast_venue_incoherent,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3LossHaltReason {
    PerTradeLossLimit,
    DailyLossLimit,
    RollingLossLimit,
    MaxDrawdownLimit,
    StaleLossSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3LossSnapshotSource {
    NtLossRuntimeFeed,
    NtPortfolioSnapshot,
    NtAccountSnapshot,
    NtAccountAndPositionSnapshot,
    NtPositionEvent,
    NtPositionChanged,
    NtPositionClosed,
    NtPositionAdjusted,
    NtCapitalAdmissionState,
    BoltLossSnapshot,
    LossGovernor,
    Unknown,
    Other,
}

const LOSS_SNAPSHOT_SOURCE_NT_LOSS_RUNTIME_FEED: &str = stringify!(nt_loss_runtime_feed);
const LOSS_SNAPSHOT_SOURCE_NT_PORTFOLIO_SNAPSHOT: &str = stringify!(nt_portfolio_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_SNAPSHOT: &str = stringify!(nt_account_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_AND_POSITION_SNAPSHOT: &str =
    stringify!(nt_account_and_position_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_EVENT: &str = stringify!(nt_position_event);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_CHANGED: &str = stringify!(nt_position_changed);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_CLOSED: &str = stringify!(nt_position_closed);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_ADJUSTED: &str = stringify!(nt_position_adjusted);
const LOSS_SNAPSHOT_SOURCE_NT_CAPITAL_ADMISSION_STATE: &str =
    stringify!(nt_capital_admission_state);
const LOSS_SNAPSHOT_SOURCE_BOLT_LOSS_SNAPSHOT: &str = stringify!(bolt_loss_snapshot);
const LOSS_SNAPSHOT_SOURCE_LOSS_GOVERNOR: &str = stringify!(loss_governor);

#[must_use]
pub fn loss_snapshot_source_to_evidence(source: &str) -> BoltV3LossSnapshotSource {
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
        LOSS_SNAPSHOT_SOURCE_NT_CAPITAL_ADMISSION_STATE => {
            BoltV3LossSnapshotSource::NtCapitalAdmissionState
        }
        LOSS_SNAPSHOT_SOURCE_BOLT_LOSS_SNAPSHOT => BoltV3LossSnapshotSource::BoltLossSnapshot,
        LOSS_SNAPSHOT_SOURCE_LOSS_GOVERNOR => BoltV3LossSnapshotSource::LossGovernor,
        _ if source.trim().is_empty() => BoltV3LossSnapshotSource::Unknown,
        _ => BoltV3LossSnapshotSource::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3LossSnapshotStaleReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3TradingState {
    Active,
    Halted,
    Reducing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RequoteActionCostClass {
    FreshSubmit,
    CancelResubmit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RequoteThrottleBlockReason {
    RequoteBudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RequoteThrottleBound {
    SubmitCommandWindow,
    RestCallWindow,
    MinInterval,
    WindowCap,
    OutOfOrderTs,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3RequoteThrottleEvidence {
    pub strategy_id: String,
    pub family_key: String,
    pub market_id: Option<String>,
    pub leg: String,
    pub now_ms: u64,
    pub observed_at_ns: u64,
    pub action_cost_class: BoltV3RequoteActionCostClass,
    pub block_reason: BoltV3RequoteThrottleBlockReason,
    pub bound_by: BoltV3RequoteThrottleBound,
    pub submit_commands_in_window: usize,
    pub submit_command_cap: u64,
    pub submit_window_ms: u64,
    pub rest_cost_in_window: u64,
    pub rest_cap_per_minute: u64,
    pub rest_window_ms: u64,
    pub min_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SettlementEvidence {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: String,
    pub position_id: String,
    pub instrument_id: String,
    pub product_id: String,
    pub outcome_side: BoltV3OutcomeSide,
    pub entry_order_side: String,
    pub quantity: String,
    pub entry_price: String,
    pub family_key: String,
    pub strike_price: String,
    pub resolution_instrument_id: String,
    pub resolution_ts_event_ns: u64,
    pub reference_close_price: String,
    pub payout_per_share: String,
    pub terminal_value: String,
    pub realized_pnl: String,
    pub settlement_currency: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3SettlementBookingErrorReason {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SettlementBookingErrorEvidence {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub instrument_id: Option<String>,
    pub resolution_instrument_id: Option<String>,
    pub reason: BoltV3SettlementBookingErrorReason,
    pub detail: String,
    pub observed_at_ns: u64,
    /// Backward-compatible reader field for records written before the canonical
    /// `terminal_settlement` schema. New writers leave this unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_lifecycle: Option<BoltV3OrderLifecycleEvidence>,
}

/// The canonical durable result of either valid terminal-eligibility leg.
/// Legacy booking-error records remain readable, but all new terminal releases
/// write this schema regardless of whether they occur live or during recovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3TerminalSettlementEvidence {
    pub settlement_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booking_error: Option<BoltV3SettlementBookingErrorEvidence>,
    pub lifecycle: BoltV3OrderLifecycleEvidence,
}

impl BoltV3TerminalSettlementEvidence {
    fn validate(&self) -> Result<()> {
        if let Some(booking_error) = self.booking_error.as_ref() {
            anyhow::ensure!(
                booking_error.settlement_key == self.settlement_key,
                "terminal settlement booking-error key does not match canonical key"
            );
            anyhow::ensure!(
                booking_error.terminal_lifecycle.is_none(),
                "canonical terminal settlement cannot contain nested legacy lifecycle evidence"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StrategyInputEvidenceSnapshot {
    pub strategy_id: String,
    pub configured_target_id: String,
    pub market_selection_ruleset_id: String,
    pub market_selection_outcome: String,
    pub market_id: Option<String>,
    pub polymarket_condition_id: Option<String>,
    pub polymarket_market_slug: Option<String>,
    pub polymarket_question_id: Option<String>,
    pub up_instrument_id: Option<String>,
    pub down_instrument_id: Option<String>,
    pub market_selection_timestamp_ms: Option<u64>,
    pub selected_market_observed_timestamp_ms: Option<u64>,
    pub polymarket_market_start_timestamp_ms: Option<u64>,
    pub polymarket_market_end_timestamp_ms: Option<u64>,
    pub price_to_beat_source: String,
    pub price_to_beat_value: String,
    pub reference_quote_ts_event: u64,
    pub spot_price: String,
    pub fast_venue_available: bool,
    pub reference_current_price: Option<String>,
    pub reference_current_price_available: bool,
    pub reference_current_price_source_id: Option<String>,
    pub reference_current_price_failed_over: Option<bool>,
    pub realized_volatility: String,
    pub realized_volatility_surface_id: String,
    pub realized_volatility_as_of_ms: Option<u64>,
    pub realized_volatility_gate_result: Option<BoltV3RvGateResult>,
    #[serde(serialize_with = "serialize_optional_local_receive_ms")]
    pub realized_volatility_receive_watermark_ms: Option<LocalReceiveMs>,
    pub realized_volatility_annualized_decimal: String,
    pub realized_volatility_measured_annualized_decimal: String,
    pub realized_volatility_noise_robust_annualized_decimal: String,
    pub realized_volatility_continuous_annualized_decimal: String,
    pub realized_volatility_jump_annualized_decimal: String,
    pub realized_volatility_forecast_annualized_decimal: String,
    pub realized_volatility_pricing_component: String,
    pub realized_volatility_seconds_per_annum: String,
    pub realized_volatility_aggregation: String,
    pub realized_volatility_sources_used: Vec<String>,
    pub realized_volatility_source_diagnostics:
        Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub realized_volatility_unknown_source_rejections: BTreeMap<String, u64>,
    pub realized_volatility_blockers: Vec<String>,
    pub realized_volatility_config_fingerprint: String,
    pub seconds_to_market_end: u64,
    pub pricing_kurtosis: String,
    pub theta_decay_factor: String,
    pub theta_scaled_min_edge_bps: String,
    pub fair_probability_up: String,
    pub uncertainty_band_probability: String,
    pub expected_edge_basis_points: String,
    pub worst_case_edge_basis_points: String,
    pub up_worst_case_edge_basis_points: Option<String>,
    pub down_worst_case_edge_basis_points: Option<String>,
    pub gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pub pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    pub fast_venue_name: Option<String>,
    pub fast_venue_age_ms: Option<u64>,
    pub fast_venue_jitter_ms: Option<u64>,
    pub fast_venue_incoherent: bool,
    pub lead_agreement_corr: Option<String>,
    pub fee_rate_basis_points: String,
    pub selected_side: Option<String>,
    pub submission_instrument_id: String,
    pub submission_order_side: String,
    pub submission_price: String,
    pub submission_quantity: String,
    pub client_order_id: String,
}

#[derive(Deserialize)]
struct BoltV3StrategyInputEvidenceSnapshotWire {
    strategy_id: String,
    configured_target_id: String,
    market_selection_ruleset_id: String,
    market_selection_outcome: String,
    market_id: Option<String>,
    polymarket_condition_id: Option<String>,
    polymarket_market_slug: Option<String>,
    polymarket_question_id: Option<String>,
    up_instrument_id: Option<String>,
    down_instrument_id: Option<String>,
    market_selection_timestamp_ms: Option<u64>,
    selected_market_observed_timestamp_ms: Option<u64>,
    polymarket_market_start_timestamp_ms: Option<u64>,
    polymarket_market_end_timestamp_ms: Option<u64>,
    price_to_beat_source: String,
    price_to_beat_value: String,
    reference_quote_ts_event: u64,
    spot_price: String,
    fast_venue_available: Option<bool>,
    reference_current_price: Option<String>,
    reference_current_price_available: Option<bool>,
    reference_current_price_source_id: Option<String>,
    reference_current_price_failed_over: Option<bool>,
    realized_volatility: String,
    realized_volatility_surface_id: String,
    realized_volatility_as_of_ms: Option<u64>,
    realized_volatility_gate_result: Option<BoltV3RvGateResult>,
    realized_volatility_receive_watermark_ms: Option<u64>,
    realized_volatility_annualized_decimal: String,
    realized_volatility_measured_annualized_decimal: String,
    realized_volatility_noise_robust_annualized_decimal: String,
    realized_volatility_continuous_annualized_decimal: String,
    realized_volatility_jump_annualized_decimal: String,
    realized_volatility_forecast_annualized_decimal: String,
    realized_volatility_pricing_component: String,
    realized_volatility_seconds_per_annum: String,
    realized_volatility_aggregation: String,
    realized_volatility_sources_used: Vec<String>,
    realized_volatility_source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    realized_volatility_unknown_source_rejections: BTreeMap<String, u64>,
    realized_volatility_blockers: Vec<String>,
    realized_volatility_config_fingerprint: String,
    seconds_to_market_end: u64,
    pricing_kurtosis: String,
    theta_decay_factor: String,
    theta_scaled_min_edge_bps: String,
    fair_probability_up: String,
    uncertainty_band_probability: String,
    expected_edge_basis_points: String,
    worst_case_edge_basis_points: String,
    up_worst_case_edge_basis_points: Option<String>,
    down_worst_case_edge_basis_points: Option<String>,
    gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    fast_venue_name: Option<String>,
    fast_venue_age_ms: Option<u64>,
    fast_venue_jitter_ms: Option<u64>,
    fast_venue_incoherent: bool,
    lead_agreement_corr: Option<String>,
    fee_rate_basis_points: String,
    selected_side: Option<String>,
    submission_instrument_id: String,
    submission_order_side: String,
    submission_price: String,
    submission_quantity: String,
    client_order_id: String,
}

impl<'de> Deserialize<'de> for BoltV3StrategyInputEvidenceSnapshot {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3StrategyInputEvidenceSnapshotWire::deserialize(deserializer)?;
        let realized_volatility_gate_result = wire
            .realized_volatility_gate_result
            .or_else(|| legacy_admitted_strategy_input_rv_fields(&wire));
        Ok(Self {
            strategy_id: wire.strategy_id,
            configured_target_id: wire.configured_target_id,
            market_selection_ruleset_id: wire.market_selection_ruleset_id,
            market_selection_outcome: wire.market_selection_outcome,
            market_id: wire.market_id,
            polymarket_condition_id: wire.polymarket_condition_id,
            polymarket_market_slug: wire.polymarket_market_slug,
            polymarket_question_id: wire.polymarket_question_id,
            up_instrument_id: wire.up_instrument_id,
            down_instrument_id: wire.down_instrument_id,
            market_selection_timestamp_ms: wire.market_selection_timestamp_ms,
            selected_market_observed_timestamp_ms: wire.selected_market_observed_timestamp_ms,
            polymarket_market_start_timestamp_ms: wire.polymarket_market_start_timestamp_ms,
            polymarket_market_end_timestamp_ms: wire.polymarket_market_end_timestamp_ms,
            price_to_beat_source: wire.price_to_beat_source,
            price_to_beat_value: wire.price_to_beat_value,
            reference_quote_ts_event: wire.reference_quote_ts_event,
            spot_price: wire.spot_price,
            fast_venue_available: wire.fast_venue_available.unwrap_or(false),
            reference_current_price: wire.reference_current_price,
            reference_current_price_available: wire
                .reference_current_price_available
                .unwrap_or(false),
            reference_current_price_source_id: wire.reference_current_price_source_id,
            reference_current_price_failed_over: wire.reference_current_price_failed_over,
            realized_volatility: wire.realized_volatility,
            realized_volatility_surface_id: wire.realized_volatility_surface_id,
            realized_volatility_as_of_ms: wire.realized_volatility_as_of_ms,
            realized_volatility_gate_result,
            realized_volatility_receive_watermark_ms: wire
                .realized_volatility_receive_watermark_ms
                .map(LocalReceiveMs::new),
            realized_volatility_annualized_decimal: wire.realized_volatility_annualized_decimal,
            realized_volatility_measured_annualized_decimal: wire
                .realized_volatility_measured_annualized_decimal,
            realized_volatility_noise_robust_annualized_decimal: wire
                .realized_volatility_noise_robust_annualized_decimal,
            realized_volatility_continuous_annualized_decimal: wire
                .realized_volatility_continuous_annualized_decimal,
            realized_volatility_jump_annualized_decimal: wire
                .realized_volatility_jump_annualized_decimal,
            realized_volatility_forecast_annualized_decimal: wire
                .realized_volatility_forecast_annualized_decimal,
            realized_volatility_pricing_component: wire.realized_volatility_pricing_component,
            realized_volatility_seconds_per_annum: wire.realized_volatility_seconds_per_annum,
            realized_volatility_aggregation: wire.realized_volatility_aggregation,
            realized_volatility_sources_used: wire.realized_volatility_sources_used,
            realized_volatility_source_diagnostics: wire.realized_volatility_source_diagnostics,
            realized_volatility_unknown_source_rejections: wire
                .realized_volatility_unknown_source_rejections,
            realized_volatility_blockers: wire.realized_volatility_blockers,
            realized_volatility_config_fingerprint: wire.realized_volatility_config_fingerprint,
            seconds_to_market_end: wire.seconds_to_market_end,
            pricing_kurtosis: wire.pricing_kurtosis,
            theta_decay_factor: wire.theta_decay_factor,
            theta_scaled_min_edge_bps: wire.theta_scaled_min_edge_bps,
            fair_probability_up: wire.fair_probability_up,
            uncertainty_band_probability: wire.uncertainty_band_probability,
            expected_edge_basis_points: wire.expected_edge_basis_points,
            worst_case_edge_basis_points: wire.worst_case_edge_basis_points,
            up_worst_case_edge_basis_points: wire.up_worst_case_edge_basis_points,
            down_worst_case_edge_basis_points: wire.down_worst_case_edge_basis_points,
            gate_blocked_by: wire.gate_blocked_by,
            pricing_blocked_by: wire.pricing_blocked_by,
            fast_venue_name: wire.fast_venue_name,
            fast_venue_age_ms: wire.fast_venue_age_ms,
            fast_venue_jitter_ms: wire.fast_venue_jitter_ms,
            fast_venue_incoherent: wire.fast_venue_incoherent,
            lead_agreement_corr: wire.lead_agreement_corr,
            fee_rate_basis_points: wire.fee_rate_basis_points,
            selected_side: wire.selected_side,
            submission_instrument_id: wire.submission_instrument_id,
            submission_order_side: wire.submission_order_side,
            submission_price: wire.submission_price,
            submission_quantity: wire.submission_quantity,
            client_order_id: wire.client_order_id,
        })
    }
}

fn legacy_admitted_strategy_input_rv_fields(
    wire: &BoltV3StrategyInputEvidenceSnapshotWire,
) -> Option<BoltV3RvGateResult> {
    (valid_legacy_rv_value(&wire.realized_volatility)
        && !wire.realized_volatility_surface_id.is_empty()
        && wire.realized_volatility_as_of_ms.is_some()
        && !wire.realized_volatility_sources_used.is_empty())
    .then_some(BoltV3RvGateResult::Accepted)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3AdmissionOutcome {
    Admitted,
    RejectedKillSwitchLatched,
    RejectedSubmitLifecycleDisallowed,
    RejectedLossGovernorHalted,
    RejectedNonPositiveNotional,
    RejectedNotionalCapExceeded,
    RejectedInvalidRiskReducingExitProof,
    RejectedCountCapExhausted,
    RejectedKillSwitchForcedReductionProofInvalid,
    RejectedKillSwitchForcedReductionCapExceeded,
    RejectedCapitalAdmission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3AdmissionDecisionEvidence {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: String,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub outcome: BoltV3AdmissionOutcome,
    pub loss_halt_reasons: Vec<BoltV3LossHaltReason>,
    pub snapshot_present: bool,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: u64,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: Option<u64>,
    pub snapshot_source: Option<BoltV3LossSnapshotSource>,
    pub per_trade_pnl_present: bool,
    pub daily_pnl_present: bool,
    pub rolling_pnl_present: bool,
    pub current_equity_present: bool,
    pub peak_equity_present: bool,
    pub last_account_state_observed_at_ns: Option<u64>,
    pub last_portfolio_snapshot_observed_at_ns: Option<u64>,
    pub last_position_event_observed_at_ns: Option<u64>,
    pub stale_reason: Option<BoltV3LossSnapshotStaleReason>,
    pub loss_snapshot_observed_at_ns: Option<u64>,
    pub loss_eval_now_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3CapitalAdmissionRebuildAuditEvidence {
    pub observed_at_ns: u64,
    pub source: String,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub recovered_reservation_count: usize,
    pub live_reserved_liability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SubmitReservationMetadataEvidence {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: String,
    pub collateral_currency: String,
    pub capital_pool_id: String,
    pub collateral_group_id: String,
    pub instrument_id: String,
    pub side: String,
    pub submitted_quantity: String,
    pub liability_factor: String,
    pub additive_liability: String,
    pub reserved_liability: String,
    pub observed_at_ns: u64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SubmitReservationFillEvidence {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub trade_id: String,
    pub instrument_id: String,
    pub side: String,
    pub fill_quantity: String,
    pub observed_at_ns: u64,
    pub reconciliation: bool,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3BasketAdmissionOutcome {
    Admitted,
    RejectedBasketNotionalCapExceeded,
    RejectedMaxOpenBasketCapExceeded,
    RejectedStaleScannerEvidence,
    RejectedStaleSubmitRecheck,
    RejectedNonPositiveCandidateCost,
    RejectedNonPositiveEdge,
    RejectedEdgeThreshold,
    RejectedMissingGroupingProof,
    RejectedMissingSettlementRules,
    RejectedRetryBudgetExceeded,
    RejectedSubmitSlots,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3BasketAdmissionDecisionEvidence {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub basket_id: String,
    pub group_id: String,
    pub leg_instrument_ids: Vec<String>,
    pub total_notional: String,
    pub leg_order_count: u32,
    pub outcome: BoltV3BasketAdmissionOutcome,
}

/// Closed result of the realized-vol staleness gate as observed at exit-evaluation
/// time. `RejectedFutureDated` and `RejectedNotReady` are split apart here even
/// though the production gate collapses them into a single `None`, so evidence can
/// distinguish the incident's future-dated case from a not-yet-warm surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RvGateResult {
    Accepted,
    MissingSnapshot,
    MissingEvaluationEventTime,
    RejectedFutureDated,
    RejectedStale,
    RejectedNotReady,
}

/// Closed taxonomy of *why* a loss-governor snapshot was treated as stale. The
/// production governor collapses all of these into one `StaleLossSnapshot` halt
/// reason; this enum is the evidence-only decomposition (behaviour-neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3StaleLossReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

/// Closed taxonomy of the layer that rejected an order, for `BoltV3OrderRejectEvidence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RejectSource {
    SubmitAdmission,
    Venue,
    NtExecution,
    Internal,
}

/// Closed, best-effort classification of a venue/NT order rejection. The raw NT
/// reason text is preserved separately in `BoltV3OrderRejectEvidence`; this enum is
/// only the structural bucket so reads stay enum-driven (no free-form reason blobs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderRejectReason {
    AdmissionRejected,
    PrecisionRejected,
    MinSizeRejected,
    MinNotionalRejected,
    InsufficientBalance,
    DuplicateClientOrderId,
    Other,
}

/// Durable evidence for a single exit evaluation (RCA #885 root causes 1-4). Decimal
/// quantities are stored as strings and timestamps as integers, matching the existing
/// records in this module. Flood-gated by the strategy: emitted on outcome-key change
/// or actual submission, never per tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3ExitEvaluationEvidence {
    pub position_id: Option<String>,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub client_order_id: Option<String>,
    pub exit_eval_now_ms: i64,
    pub exit_trigger_source: BoltV3ExitTriggerSource,
    pub trigger_ts_event_ms: Option<i64>,
    pub trigger_ts_init_ms: Option<i64>,
    pub rv_surface_id: String,
    pub rv_as_of_ms: Option<i64>,
    pub rv_ready: bool,
    pub rv_snapshot_receive_watermark_ms: Option<i64>,
    pub rv_max_source_age_ms: Option<u64>,
    pub rv_blockers: Vec<String>,
    pub rv_source_diagnostics: Vec<String>,
    pub rv_gate_result: BoltV3RvGateResult,
    pub rv_as_of_minus_now_ms: Option<i64>,
    pub spot_price: Option<String>,
    pub spot_venue_name: Option<String>,
    pub fast_venue_available: bool,
    pub reference_current_price: Option<String>,
    pub reference_current_price_available: bool,
    pub interval_open: Option<String>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub uncertainty_band_probability: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub hold_ev_bps: Option<String>,
    pub exit_ev_bps: Option<String>,
    pub exit_decision: BoltV3ExitDecisionOutcome,
    pub forced_flat_reasons: Vec<String>,
    pub submission_order_side: Option<String>,
    pub submission_price: Option<String>,
    pub submission_quantity: Option<String>,
    pub submission_blocked_reason: Option<String>,
}

#[derive(Deserialize)]
struct BoltV3ExitEvaluationEvidenceWire {
    position_id: Option<String>,
    market_id: Option<String>,
    instrument_id: Option<String>,
    client_order_id: Option<String>,
    exit_eval_now_ms: i64,
    exit_trigger_source: BoltV3ExitTriggerSource,
    trigger_ts_event_ms: Option<i64>,
    trigger_ts_init_ms: Option<i64>,
    rv_surface_id: String,
    rv_as_of_ms: Option<i64>,
    rv_ready: bool,
    rv_snapshot_receive_watermark_ms: Option<i64>,
    rv_max_source_age_ms: Option<u64>,
    rv_blockers: Vec<String>,
    rv_source_diagnostics: Vec<String>,
    rv_gate_result: BoltV3RvGateResult,
    rv_as_of_minus_now_ms: Option<i64>,
    spot_price: Option<String>,
    spot_venue_name: Option<String>,
    fast_venue_available: Option<bool>,
    reference_current_price: Option<String>,
    reference_current_price_available: Option<bool>,
    interval_open: Option<String>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    uncertainty_band_probability: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    exit_decision: BoltV3ExitDecisionOutcome,
    forced_flat_reasons: Vec<String>,
    submission_order_side: Option<String>,
    submission_price: Option<String>,
    submission_quantity: Option<String>,
    submission_blocked_reason: Option<String>,
}

impl<'de> Deserialize<'de> for BoltV3ExitEvaluationEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3ExitEvaluationEvidenceWire::deserialize(deserializer)?;
        if wire.trigger_ts_init_ms.is_some_and(i64::is_negative) {
            return Err(serde::de::Error::custom(
                "trigger_ts_init_ms must be non-negative",
            ));
        }
        if wire
            .rv_snapshot_receive_watermark_ms
            .is_some_and(i64::is_negative)
        {
            return Err(serde::de::Error::custom(
                "rv_snapshot_receive_watermark_ms must be non-negative",
            ));
        }
        Ok(Self {
            position_id: wire.position_id,
            market_id: wire.market_id,
            instrument_id: wire.instrument_id,
            client_order_id: wire.client_order_id,
            exit_eval_now_ms: wire.exit_eval_now_ms,
            exit_trigger_source: wire.exit_trigger_source,
            trigger_ts_event_ms: wire.trigger_ts_event_ms,
            trigger_ts_init_ms: wire.trigger_ts_init_ms,
            rv_surface_id: wire.rv_surface_id,
            rv_as_of_ms: wire.rv_as_of_ms,
            rv_ready: wire.rv_ready,
            rv_snapshot_receive_watermark_ms: wire.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: wire.rv_max_source_age_ms,
            rv_blockers: wire.rv_blockers,
            rv_source_diagnostics: wire.rv_source_diagnostics,
            rv_gate_result: wire.rv_gate_result,
            rv_as_of_minus_now_ms: wire.rv_as_of_minus_now_ms,
            spot_price: wire.spot_price,
            spot_venue_name: wire.spot_venue_name,
            fast_venue_available: wire.fast_venue_available.unwrap_or(false),
            reference_current_price: wire.reference_current_price,
            reference_current_price_available: wire
                .reference_current_price_available
                .unwrap_or(false),
            interval_open: wire.interval_open,
            fair_probability_up: wire.fair_probability_up,
            fair_probability_down: wire.fair_probability_down,
            uncertainty_band_probability: wire.uncertainty_band_probability,
            up_fee_bps: wire.up_fee_bps,
            down_fee_bps: wire.down_fee_bps,
            hold_ev_bps: wire.hold_ev_bps,
            exit_ev_bps: wire.exit_ev_bps,
            exit_decision: wire.exit_decision,
            forced_flat_reasons: wire.forced_flat_reasons,
            submission_order_side: wire.submission_order_side,
            submission_price: wire.submission_price,
            submission_quantity: wire.submission_quantity,
            submission_blocked_reason: wire.submission_blocked_reason,
        })
    }
}

/// Durable evidence for a loss-governor halt observed at the submit-admission consumer
/// (RCA #885 root cause 5). Captures snapshot freshness and per-source last-seen
/// timestamps so a stale halt can be explained from disk. Episode-gated (exponential
/// sampling keyed by `stable_halt_key`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3LossGovernorHaltEvidence {
    pub snapshot_present: bool,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: u64,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: u64,
    pub snapshot_source: Option<String>,
    pub has_per_trade_pnl: bool,
    pub has_daily_pnl: bool,
    pub has_rolling_pnl: bool,
    pub has_current_equity: bool,
    pub has_peak_equity: bool,
    pub last_account_state_ts_ns: Option<u64>,
    pub last_portfolio_snapshot_ts_ns: Option<u64>,
    pub last_position_event_ts_ns: Option<u64>,
    pub account_state_count: u64,
    pub portfolio_snapshot_count: u64,
    pub position_event_count: u64,
    pub stale_reason: BoltV3StaleLossReason,
    pub stable_halt_key: String,
    pub retry_count: u32,
    pub elapsed_since_first_halt_ns: u64,
}

/// Durable evidence correlating an order rejection with its retry episode (RCA #885
/// root causes 6-7). Raw and normalized amounts plus venue precision constraints are
/// captured best-effort; bolt never computes NT-adapter-internal maker/taker amounts,
/// so those stay null unless the NT event exposes them. Episode-gated by
/// `stable_episode_key`, which excludes the per-attempt client_order_id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderRejectEvidence {
    pub reject_source: BoltV3RejectSource,
    pub reject_reason: BoltV3OrderRejectReason,
    pub admission_outcome: Option<BoltV3AdmissionOutcome>,
    pub raw_reason_text: Option<String>,
    pub instrument_id: String,
    pub order_side: Option<String>,
    pub raw_price: Option<String>,
    pub raw_quantity: Option<String>,
    pub raw_maker_amount: Option<String>,
    pub raw_taker_amount: Option<String>,
    pub normalized_price: Option<String>,
    pub normalized_quantity: Option<String>,
    pub normalized_maker_amount: Option<String>,
    pub normalized_taker_amount: Option<String>,
    pub venue_price_precision: Option<u32>,
    pub venue_size_precision: Option<u32>,
    pub venue_min_notional: Option<String>,
    pub prior_client_order_id: Option<String>,
    pub client_order_id: String,
    pub retry_count: u32,
    pub backoff_cooldown_state: Option<String>,
    pub stable_episode_key: String,
    pub elapsed_ns: u64,
}

/// Closed taxonomy of strategy-local order lifecycle transitions. These records
/// make lifecycle substitutes distinguishable from normal submit/fill evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderLifecycleTransition {
    BoundaryReclassification,
    EntryFillMaterialized,
    EntryReconcilePending,
    PositionTruthRematerialized,
    PositionClosed,
    ResidualRemanaged,
    RestartOpenOrderAdopted,
    RestartOpenOrderRecoveryBlocked,
    SettlementEvidenceRecoveryBlocked,
    /// Live or recovery path: settlement booking failed terminally; exposure released.
    SettlementBookingTerminal,
    OrderDenied,
    OrderRejected,
    OrderCanceled,
    OrderExpired,
    OrderFilled,
    ReconcileQueryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderLifecycleOutcome {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderLifecycleEvidence {
    pub strategy_id: String,
    pub transition: BoltV3OrderLifecycleTransition,
    pub outcome: BoltV3OrderLifecycleOutcome,
    pub source: String,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub position_id: Option<String>,
    pub client_order_id: Option<String>,
    pub prior_client_order_id: Option<String>,
    pub raw_reason_text: Option<String>,
    pub order_side: Option<String>,
    pub filled_quantity: Option<String>,
    pub residual_quantity: Option<String>,
    pub ts_event_ns: Option<u64>,
}

#[derive(Debug)]
#[cfg(not(test))]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    file: Mutex<std::fs::File>,
}

#[derive(Debug)]
#[cfg(test)]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    file: Mutex<std::fs::File>,
    fail_append: bool,
    append_attempts: std::sync::atomic::AtomicUsize,
}

impl JsonlBoltV3DecisionEvidenceWriter {
    pub fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self> {
        let path = decision_evidence_path(loaded)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create decision evidence directory `{}`",
                    parent.display()
                )
            })?;
        }
        let file = open_decision_evidence_append_file(&path).with_context(|| {
            format!("failed to open decision evidence file `{}`", path.display())
        })?;
        Ok(Self {
            file: Mutex::new(file),
            #[cfg(test)]
            fail_append: false,
            #[cfg(test)]
            append_attempts: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_test_path(path: &Path) -> Result<Self> {
        let file = open_decision_evidence_append_file(path).with_context(|| {
            format!(
                "failed to open test decision evidence file `{}`",
                path.display()
            )
        })?;
        Ok(Self {
            file: Mutex::new(file),
            fail_append: false,
            append_attempts: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_test_path_with_append_failure(path: &Path) -> Result<Self> {
        let mut writer = Self::from_test_path(path)?;
        writer.fail_append = true;
        Ok(writer)
    }

    #[cfg(test)]
    pub(crate) fn append_attempts(&self) -> usize {
        self.append_attempts.load(Ordering::Relaxed)
    }

    fn append_line(&self, line: &[u8]) -> Result<()> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow!("decision evidence writer lock is poisoned"))?;
        file.write_all(line)
            .context("failed to write decision evidence record")?;
        file.sync_data()
            .context("failed to sync decision evidence to disk")?;
        Ok(())
    }

    pub fn drain_shutdown(&self) -> Result<()> {
        let file = self
            .file
            .lock()
            .map_err(|_| anyhow!("decision evidence writer lock is poisoned"))?;
        file.sync_all()
            .context("failed to drain decision evidence to disk")?;
        Ok(())
    }
}

const fn rv_gate_index(result: BoltV3RvGateResult) -> u32 {
    match result {
        BoltV3RvGateResult::Accepted => 0,
        BoltV3RvGateResult::MissingSnapshot => 1,
        BoltV3RvGateResult::MissingEvaluationEventTime => 2,
        BoltV3RvGateResult::RejectedFutureDated => 3,
        BoltV3RvGateResult::RejectedStale => 4,
        BoltV3RvGateResult::RejectedNotReady => 5,
    }
}

const fn entry_skip_index(reason: BoltV3EntrySkipReasonCategory) -> u32 {
    match reason {
        BoltV3EntrySkipReasonCategory::StrategyCoreNotRegistered => 0,
        BoltV3EntrySkipReasonCategory::EntryGateBlocked => 1,
        BoltV3EntrySkipReasonCategory::EntryPricingBlocked => 2,
        BoltV3EntrySkipReasonCategory::NoSideSelected => 3,
        BoltV3EntrySkipReasonCategory::SizedNotionalNotPositive => 4,
        BoltV3EntrySkipReasonCategory::InstrumentIdMissing => 5,
        BoltV3EntrySkipReasonCategory::InstrumentMissingFromCache => 6,
        BoltV3EntrySkipReasonCategory::EntryPriceMissing => 7,
        BoltV3EntrySkipReasonCategory::QuantityRoundingFailed => 8,
        BoltV3EntrySkipReasonCategory::LimitNotionalExceedsSizedNotional => 9,
        BoltV3EntrySkipReasonCategory::QuantityNotPositive => 10,
        BoltV3EntrySkipReasonCategory::PositionContractInvalid => 11,
        BoltV3EntrySkipReasonCategory::EntryPositionContractUnsupported => 12,
        BoltV3EntrySkipReasonCategory::HistoricalEntryFeeUnavailable => 13,
        BoltV3EntrySkipReasonCategory::OnePositionInvariantViolation => 14,
        BoltV3EntrySkipReasonCategory::Unclassified => 15,
    }
}

const fn basket_admission_index(outcome: &BoltV3BasketAdmissionOutcome) -> u32 {
    match outcome {
        BoltV3BasketAdmissionOutcome::Admitted => 0,
        BoltV3BasketAdmissionOutcome::RejectedBasketNotionalCapExceeded => 1,
        BoltV3BasketAdmissionOutcome::RejectedMaxOpenBasketCapExceeded => 2,
        BoltV3BasketAdmissionOutcome::RejectedStaleScannerEvidence => 3,
        BoltV3BasketAdmissionOutcome::RejectedStaleSubmitRecheck => 4,
        BoltV3BasketAdmissionOutcome::RejectedNonPositiveCandidateCost => 5,
        BoltV3BasketAdmissionOutcome::RejectedNonPositiveEdge => 6,
        BoltV3BasketAdmissionOutcome::RejectedEdgeThreshold => 7,
        BoltV3BasketAdmissionOutcome::RejectedMissingGroupingProof => 8,
        BoltV3BasketAdmissionOutcome::RejectedMissingSettlementRules => 9,
        BoltV3BasketAdmissionOutcome::RejectedRetryBudgetExceeded => 10,
        BoltV3BasketAdmissionOutcome::RejectedSubmitSlots => 11,
    }
}

const fn exit_decision_index(outcome: BoltV3ExitDecisionOutcome) -> u32 {
    match outcome {
        BoltV3ExitDecisionOutcome::Exit => 0,
        BoltV3ExitDecisionOutcome::ExitFailClosed => 1,
        BoltV3ExitDecisionOutcome::Hold => 2,
        BoltV3ExitDecisionOutcome::Blocked => 3,
    }
}

const fn stale_loss_index(reason: BoltV3StaleLossReason) -> u32 {
    match reason {
        BoltV3StaleLossReason::MissingSnapshot => 0,
        BoltV3StaleLossReason::SourceEmpty => 1,
        BoltV3StaleLossReason::FutureDated => 2,
        BoltV3StaleLossReason::AgeExceeded => 3,
        BoltV3StaleLossReason::MissingRequiredField => 4,
    }
}

const fn reject_source_index(source: BoltV3RejectSource) -> u32 {
    match source {
        BoltV3RejectSource::SubmitAdmission => 0,
        BoltV3RejectSource::Venue => 1,
        BoltV3RejectSource::NtExecution => 2,
        BoltV3RejectSource::Internal => 3,
    }
}

const fn reject_reason_index(reason: BoltV3OrderRejectReason) -> u32 {
    match reason {
        BoltV3OrderRejectReason::AdmissionRejected => 0,
        BoltV3OrderRejectReason::PrecisionRejected => 1,
        BoltV3OrderRejectReason::MinSizeRejected => 2,
        BoltV3OrderRejectReason::MinNotionalRejected => 3,
        BoltV3OrderRejectReason::InsufficientBalance => 4,
        BoltV3OrderRejectReason::DuplicateClientOrderId => 5,
        BoltV3OrderRejectReason::Other => 6,
    }
}

pub(crate) const fn order_reject_novelty_index(
    source: BoltV3RejectSource,
    reason: BoltV3OrderRejectReason,
) -> u32 {
    reject_source_index(source) * 7 + reject_reason_index(reason)
}

const fn venue_truth_divergence_index(class: VenueTruthDivergenceAlarmClass) -> u32 {
    match class {
        VenueTruthDivergenceAlarmClass::TrueDivergence => 0,
        VenueTruthDivergenceAlarmClass::OrderingViolation => 1,
        VenueTruthDivergenceAlarmClass::SilentChannel => 2,
    }
}

fn claim_blocked_rv(gate: BoltV3RvGateResult, watermark_present: bool) -> bool {
    mark_u16_once(
        &BLOCKED_RV_NOVELTY,
        rv_gate_index(gate) * 2 + u32::from(watermark_present),
        BLOCKED_RV_DOMAIN,
    )
}

fn claim_entry_skip(reason: BoltV3EntrySkipReasonCategory) -> bool {
    mark_u16_once(
        &ENTRY_SKIP_NOVELTY,
        entry_skip_index(reason),
        ENTRY_SKIP_DOMAIN,
    )
}

fn claim_basket_admission(outcome: &BoltV3BasketAdmissionOutcome) -> bool {
    mark_u16_once(
        &BASKET_ADMISSION_NOVELTY,
        basket_admission_index(outcome),
        BASKET_ADMISSION_DOMAIN,
    )
}

fn claim_exit_decision(outcome: BoltV3ExitDecisionOutcome) -> bool {
    mark_u8_once(
        &EXIT_DECISION_NOVELTY,
        exit_decision_index(outcome),
        EXIT_DECISION_DOMAIN,
    )
}

fn claim_exit_evaluation(outcome: BoltV3ExitDecisionOutcome, gate: BoltV3RvGateResult) -> bool {
    mark_u32_once(
        &EXIT_EVALUATION_NOVELTY,
        exit_decision_index(outcome) * 6 + rv_gate_index(gate),
        EXIT_EVALUATION_DOMAIN,
    )
}

fn claim_loss_halt(reason: BoltV3StaleLossReason) -> bool {
    mark_u8_once(
        &LOSS_GOVERNOR_HALT_NOVELTY,
        stale_loss_index(reason),
        LOSS_GOVERNOR_HALT_DOMAIN,
    )
}

fn claim_order_reject(source: BoltV3RejectSource, reason: BoltV3OrderRejectReason) -> bool {
    mark_u32_once(
        &ORDER_REJECT_NOVELTY,
        order_reject_novelty_index(source, reason),
        ORDER_REJECT_DOMAIN,
    )
}

fn claim_requote_throttle(leg: u32, bound: u32) -> bool {
    mark_u16_once(
        &REQUOTE_THROTTLE_NOVELTY,
        leg * 6 + bound,
        REQUOTE_THROTTLE_DOMAIN,
    )
}

fn claim_venue_truth_capture_failure() -> bool {
    mark_u8_once(
        &VENUE_TRUTH_CAPTURE_FAILURE_NOVELTY,
        0,
        VENUE_TRUTH_CAPTURE_FAILURE_DOMAIN,
    )
}

fn claim_venue_truth_divergence(class: VenueTruthDivergenceAlarmClass) -> bool {
    mark_u8_once(
        &VENUE_TRUTH_DIVERGENCE_NOVELTY,
        venue_truth_divergence_index(class),
        VENUE_TRUTH_DIVERGENCE_DOMAIN,
    )
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        if snapshot.client_order_id.is_empty() {
            let gate = snapshot
                .realized_volatility_gate_result
                .unwrap_or(BoltV3RvGateResult::MissingSnapshot);
            if !claim_blocked_rv(
                gate,
                snapshot.realized_volatility_receive_watermark_ms.is_some(),
            ) {
                return Ok(());
            }
        }
        let line = encode_strategy_input_snapshot_line(snapshot)?;
        self.append_line(&line)
    }

    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        let line = encode_order_intent_line(intent)?;
        self.append_line(&line)
    }

    fn record_admission_decision(&self, decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        let line = encode_admission_decision_line(decision)?;
        self.append_line(&line)
    }

    fn record_basket_admission_decision(
        &self,
        decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        if !claim_basket_admission(&decision.outcome) {
            return Ok(());
        }
        let line = encode_basket_admission_decision_line(decision)?;
        self.append_line(&line)
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()> {
        let line = encode_capital_admission_rebuild_audit_line(audit)?;
        self.append_line(&line)
    }

    fn record_submit_reservation_metadata(
        &self,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        let line = encode_submit_reservation_metadata_line(metadata)?;
        self.append_line(&line)
    }

    fn record_submit_reservation_fill(
        &self,
        fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        let line = encode_submit_reservation_fill_line(fill)?;
        self.append_line(&line)
    }

    fn record_entry_skip(&self, skip: &BoltV3EntrySkipEvidence) -> Result<()> {
        if !claim_entry_skip(skip.reason_category) {
            return Ok(());
        }
        let line = encode_entry_skip_line(skip)?;
        self.append_line(&line)
    }

    fn record_exit_decision(&self, decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        if !claim_exit_decision(decision.exit_decision) {
            return Ok(());
        }
        let line = encode_exit_decision_line(decision)?;
        self.append_line(&line)
    }

    fn record_exit_evaluation(&self, evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
        if !claim_exit_evaluation(evidence.exit_decision, evidence.rv_gate_result) {
            return Ok(());
        }
        let line = encode_exit_evaluation_line(evidence)?;
        #[cfg(test)]
        {
            self.append_attempts.fetch_add(1, Ordering::Relaxed);
            if self.fail_append {
                anyhow::bail!("injected decision evidence append failure");
            }
        }
        self.append_line(&line)
    }

    fn record_loss_governor_halt(&self, evidence: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        if !claim_loss_halt(evidence.stale_reason) {
            return Ok(());
        }
        let line = encode_loss_governor_halt_line(evidence)?;
        self.append_line(&line)
    }

    fn record_order_reject(&self, evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
        if !claim_order_reject(evidence.reject_source, evidence.reject_reason) {
            return Ok(());
        }
        let line = encode_order_reject_line(evidence)?;
        self.append_line(&line)
    }

    fn record_order_lifecycle(&self, evidence: &BoltV3OrderLifecycleEvidence) -> Result<()> {
        let line = encode_order_lifecycle_line(evidence)?;
        self.append_line(&line)
    }

    fn record_requote_throttle(&self, throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        let leg = match throttle.leg.as_str() {
            "yes" => 0,
            "no" => 1,
            other => anyhow::bail!("unregistered requote throttle leg `{other}`"),
        };
        let bound = match throttle.bound_by {
            BoltV3RequoteThrottleBound::SubmitCommandWindow => 0,
            BoltV3RequoteThrottleBound::RestCallWindow => 1,
            BoltV3RequoteThrottleBound::MinInterval => 2,
            BoltV3RequoteThrottleBound::WindowCap => 3,
            BoltV3RequoteThrottleBound::OutOfOrderTs => 4,
            BoltV3RequoteThrottleBound::Overflow => 5,
        };
        if !claim_requote_throttle(leg, bound) {
            return Ok(());
        }
        let line = encode_requote_throttle_line(throttle)?;
        self.append_line(&line)
    }

    fn record_settlement(&self, evidence: &BoltV3SettlementEvidence) -> Result<()> {
        let line = encode_settlement_line(evidence)?;
        self.append_line(&line)
    }

    fn record_settlement_booking_error(
        &self,
        evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> Result<()> {
        let line = encode_settlement_booking_error_line(evidence)?;
        self.append_line(&line)
    }

    fn record_terminal_settlement(
        &self,
        evidence: &BoltV3TerminalSettlementEvidence,
    ) -> Result<()> {
        let line = encode_terminal_settlement_line(evidence)?;
        self.append_line(&line)
    }

    fn record_venue_truth_capture_failure(
        &self,
        evidence: &VenueTruthCaptureFailureEvidence,
    ) -> Result<()> {
        if !claim_venue_truth_capture_failure() {
            return Ok(());
        }
        let line = encode_venue_truth_capture_failure_line(evidence)?;
        self.append_line(&line)
    }

    fn record_venue_truth_divergence(&self, evidence: &VenueTruthDivergenceEvidence) -> Result<()> {
        if !claim_venue_truth_divergence(evidence.alarm_class) {
            return Ok(());
        }
        let line = encode_venue_truth_divergence_line(evidence)?;
        self.append_line(&line)
    }

    fn drain_shutdown(&self) -> Result<()> {
        JsonlBoltV3DecisionEvidenceWriter::drain_shutdown(self)
    }
}

/// Validates `persistence.decision_evidence.order_intents_relative_path` as the
/// single source of truth for the predicate: the trimmed value must be
/// non-empty, relative, and contain no `..` component so it always stays under
/// `catalog_directory`. Returns the operator-facing error string on rejection so
/// both config-load validation and the runtime path builder share one check.
pub(crate) fn validate_decision_evidence_relative_path(raw: &str) -> Result<(), String> {
    let relative = Path::new(raw.trim());
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(
            "persistence.decision_evidence.order_intents_relative_path must be non-empty, relative, and stay under catalog_directory"
                .to_string(),
        );
    }
    Ok(())
}

pub fn decision_evidence_path(loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
    let raw = &loaded
        .root
        .persistence
        .decision_evidence
        .order_intents_relative_path;
    validate_decision_evidence_relative_path(raw).map_err(|message| anyhow!(message))?;
    Ok(Path::new(&loaded.root.persistence.catalog_directory).join(Path::new(raw.trim())))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3EntryDecisionEvidenceChain {
    pub snapshot: BoltV3StrategyInputEvidenceSnapshot,
    pub intent: BoltV3OrderIntentEvidence,
    pub admission: BoltV3AdmissionDecisionEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitReservationRecoveryEvidence {
    pub metadata_by_client_order_id: BTreeMap<String, BoltV3RecoveredSubmitReservationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3RecoveredSubmitReservationEvidence {
    pub metadata: BoltV3SubmitReservationMetadataEvidence,
    pub fill_trade_ids: BTreeSet<String>,
}

pub fn read_latest_entry_decision_evidence_chain(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<BoltV3EntryDecisionEvidenceChain> {
    let path = path.as_ref();
    let mut file = open_regular_decision_evidence_file(path)
        .context("failed to open regular file bolt-v3 decision evidence")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("failed to read bolt-v3 decision evidence file")?;
    if bytes.len() as u64 > max_bytes {
        return Err(anyhow!(
            "bolt-v3 decision evidence file exceeds max_bytes={max_bytes}"
        ));
    }

    let mut snapshots = BTreeMap::<String, BoltV3StrategyInputEvidenceSnapshot>::new();
    let mut intents = BTreeMap::<String, BoltV3OrderIntentEvidence>::new();
    let mut admissions = BTreeMap::<String, BoltV3AdmissionDecisionEvidence>::new();
    let mut latest = None;
    let mut first_older_schema_index = None;
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let header: DecisionEvidenceEnvelopeHeader =
            serde_json::from_slice(line).with_context(|| {
                format!("failed to parse bolt-v3 decision evidence envelope at line index {index}")
            })?;
        if decision_evidence_header_is_below_current_schema(&header) {
            first_older_schema_index.get_or_insert(index);
            continue;
        }
        match header.kind.as_str() {
            "strategy_input_snapshot" => {
                header.validate(
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND,
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                    index,
                )?;
                let decoded: StrategyInputSnapshotLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 strategy input snapshot line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    "strategy_input_snapshot",
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                    index,
                )?;
                snapshots.insert(decoded.snapshot.client_order_id.clone(), decoded.snapshot);
            }
            "order_intent" => {
                header.validate(
                    BOLT_V3_ORDER_INTENT_RECORD_KIND,
                    BOLT_V3_ORDER_INTENT_GATE_ID,
                    index,
                )?;
                let decoded: OrderIntentLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order intent line at index {index}")
                    })?;
                decoded.validate_header("order_intent", BOLT_V3_ORDER_INTENT_GATE_ID, index)?;
                if decoded.intent.intent_kind == BoltV3OrderIntentKind::Entry {
                    intents.insert(decoded.intent.client_order_id.clone(), decoded.intent);
                }
            }
            "admission_decision" => {
                header.validate(
                    BOLT_V3_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: AdmissionDecisionLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 admission decision line at index {index}")
                    })?;
                decoded.validate_header(
                    "admission_decision",
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                if decoded.decision.intent_kind == BoltV3SubmitIntentKind::Entry {
                    let client_order_id = decoded.decision.client_order_id.clone();
                    admissions.insert(client_order_id.clone(), decoded.decision);
                    if let (Some(snapshot), Some(intent), Some(admission)) = (
                        snapshots.get(&client_order_id),
                        intents.get(&client_order_id),
                        admissions.get(&client_order_id),
                    ) {
                        latest = Some(validate_entry_decision_chain(
                            snapshot.clone(),
                            intent.clone(),
                            admission.clone(),
                        )?);
                    }
                }
            }
            "basket_admission_decision" => {
                header.validate(
                    BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: BasketAdmissionDecisionLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 basket admission decision line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
            }
            "capital_admission_rebuild" => {
                header.validate(
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND,
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID,
                    index,
                )?;
                let decoded: CapitalAdmissionRebuildAuditLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 capital admission rebuild audit line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND,
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID,
                    index,
                )?;
            }
            "submit_reservation_metadata" => {
                header.validate(
                    BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: SubmitReservationMetadataLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 submit reservation metadata line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
            }
            "submit_reservation_fill" => {
                header.validate(
                    BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: SubmitReservationFillLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 submit reservation fill line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_ENTRY_SKIP_RECORD_KIND => {
                header.validate(
                    BOLT_V3_ENTRY_SKIP_RECORD_KIND,
                    BOLT_V3_ENTRY_SKIP_GATE_ID,
                    index,
                )?;
                let decoded: EntrySkipLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 entry skip line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ENTRY_SKIP_RECORD_KIND,
                    BOLT_V3_ENTRY_SKIP_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_EXIT_DECISION_RECORD_KIND => {
                header.validate(
                    BOLT_V3_EXIT_DECISION_RECORD_KIND,
                    BOLT_V3_EXIT_DECISION_GATE_ID,
                    index,
                )?;
                let decoded: ExitDecisionLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 exit decision line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_EXIT_DECISION_RECORD_KIND,
                    BOLT_V3_EXIT_DECISION_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_EXIT_EVALUATION_RECORD_KIND => {
                header.validate(
                    BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
                    BOLT_V3_EXIT_EVALUATION_GATE_ID,
                    index,
                )?;
                let decoded: ExitEvaluationLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 exit evaluation line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
                    BOLT_V3_EXIT_EVALUATION_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
                    BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
                    index,
                )?;
                let decoded: LossGovernorHaltLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 loss governor halt line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
                    BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_ORDER_REJECT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_ORDER_REJECT_RECORD_KIND,
                    BOLT_V3_ORDER_REJECT_GATE_ID,
                    index,
                )?;
                let decoded: OrderRejectLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order reject line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ORDER_REJECT_RECORD_KIND,
                    BOLT_V3_ORDER_REJECT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_SETTLEMENT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_SETTLEMENT_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
                let decoded: SettlementLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 settlement line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_SETTLEMENT_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND => {
                header.validate(
                    BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
                let decoded: SettlementBookingErrorLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 settlement booking-error line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
                let decoded: TerminalSettlementLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 terminal settlement line at index {index}")
                    })?;
                decoded.validate_header(index)?;
            }
            BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND,
                    BOLT_V3_ORDER_LIFECYCLE_GATE_ID,
                    index,
                )?;
                let decoded: OrderLifecycleLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order lifecycle line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND,
                    BOLT_V3_ORDER_LIFECYCLE_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID,
                    index,
                )?;
                let decoded: VenueTruthCaptureFailureLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 venue truth capture failure line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID,
                    index,
                )?;
                let decoded: VenueTruthDivergenceLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 venue truth divergence line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND,
                    BOLT_V3_REQUOTE_THROTTLE_GATE_ID,
                    index,
                )?;
                let decoded: RequoteThrottleLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 requote throttle line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND,
                    BOLT_V3_REQUOTE_THROTTLE_GATE_ID,
                    index,
                )?;
            }
            other => {
                return Err(anyhow!(
                    "unsupported bolt-v3 decision evidence kind `{other}` at line index {index}"
                ));
            }
        }
    }
    match latest {
        Some(chain) => Ok(chain),
        None => {
            if let Some(index) = first_older_schema_index {
                Err(anyhow!(
                    "bolt-v3 decision evidence schema_version mismatch at line index {index}"
                ))
            } else {
                Err(anyhow!(
                    "bolt-v3 decision evidence has no complete entry decision chain"
                ))
            }
        }
    }
}

pub fn read_submit_reservation_recovery_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<BoltV3SubmitReservationRecoveryEvidence> {
    let path = path.as_ref();
    let mut file = open_regular_decision_evidence_file(path)
        .context("failed to open regular file bolt-v3 decision evidence")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("failed to read bolt-v3 decision evidence file")?;
    if bytes.len() as u64 > max_bytes {
        return Err(anyhow!(
            "bolt-v3 decision evidence file exceeds max_bytes={max_bytes}"
        ));
    }

    let mut metadata_by_client_order_id =
        BTreeMap::<String, BoltV3SubmitReservationMetadataEvidence>::new();
    let mut fills = Vec::<BoltV3SubmitReservationFillEvidence>::new();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let header: DecisionEvidenceEnvelopeHeader =
            serde_json::from_slice(line).with_context(|| {
                format!("failed to parse bolt-v3 decision evidence envelope at line index {index}")
            })?;
        if decision_evidence_header_is_below_current_schema_non_recovery_record(&header) {
            continue;
        }
        match header.kind.as_str() {
            "strategy_input_snapshot" => {
                header.validate(
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND,
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                    index,
                )?;
                let decoded: StrategyInputSnapshotLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 strategy input snapshot line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND,
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                    index,
                )?;
            }
            "order_intent" => {
                header.validate(
                    BOLT_V3_ORDER_INTENT_RECORD_KIND,
                    BOLT_V3_ORDER_INTENT_GATE_ID,
                    index,
                )?;
                let decoded: OrderIntentLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order intent line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ORDER_INTENT_RECORD_KIND,
                    BOLT_V3_ORDER_INTENT_GATE_ID,
                    index,
                )?;
            }
            "admission_decision" => {
                header.validate(
                    BOLT_V3_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: AdmissionDecisionLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 admission decision line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
            }
            "basket_admission_decision" => {
                header.validate(
                    BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: BasketAdmissionDecisionLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 basket admission decision line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
            }
            "capital_admission_rebuild" => {
                header.validate(
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND,
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID,
                    index,
                )?;
                let decoded: CapitalAdmissionRebuildAuditLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 capital admission rebuild audit line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND,
                    BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID,
                    index,
                )?;
            }
            "submit_reservation_metadata" => {
                header.validate(
                    BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: SubmitReservationMetadataLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 submit reservation metadata line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                validate_submit_reservation_metadata(&decoded.metadata).with_context(|| {
                    format!("invalid submit reservation metadata at line index {index}")
                })?;
                let replace = metadata_by_client_order_id
                    .get(&decoded.metadata.client_order_id)
                    .map(|existing| decoded.metadata.observed_at_ns > existing.observed_at_ns)
                    .unwrap_or(true);
                if replace {
                    metadata_by_client_order_id
                        .insert(decoded.metadata.client_order_id.clone(), decoded.metadata);
                }
            }
            "submit_reservation_fill" => {
                header.validate(
                    BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                let decoded: SubmitReservationFillLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 submit reservation fill line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND,
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                validate_submit_reservation_fill(&decoded.fill).with_context(|| {
                    format!("invalid submit reservation fill at line index {index}")
                })?;
                fills.push(decoded.fill);
            }
            BOLT_V3_ENTRY_SKIP_RECORD_KIND => {
                header.validate(
                    BOLT_V3_ENTRY_SKIP_RECORD_KIND,
                    BOLT_V3_ENTRY_SKIP_GATE_ID,
                    index,
                )?;
                let decoded: EntrySkipLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 entry skip line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ENTRY_SKIP_RECORD_KIND,
                    BOLT_V3_ENTRY_SKIP_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_EXIT_DECISION_RECORD_KIND => {
                header.validate(
                    BOLT_V3_EXIT_DECISION_RECORD_KIND,
                    BOLT_V3_EXIT_DECISION_GATE_ID,
                    index,
                )?;
                let decoded: ExitDecisionLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 exit decision line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_EXIT_DECISION_RECORD_KIND,
                    BOLT_V3_EXIT_DECISION_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_EXIT_EVALUATION_RECORD_KIND => {
                header.validate(
                    BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
                    BOLT_V3_EXIT_EVALUATION_GATE_ID,
                    index,
                )?;
                let decoded: ExitEvaluationLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 exit evaluation line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
                    BOLT_V3_EXIT_EVALUATION_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
                    BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
                    index,
                )?;
                let decoded: LossGovernorHaltLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 loss governor halt line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
                    BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_ORDER_REJECT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_ORDER_REJECT_RECORD_KIND,
                    BOLT_V3_ORDER_REJECT_GATE_ID,
                    index,
                )?;
                let decoded: OrderRejectLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order reject line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ORDER_REJECT_RECORD_KIND,
                    BOLT_V3_ORDER_REJECT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_SETTLEMENT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_SETTLEMENT_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
                let decoded: SettlementLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 settlement line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_SETTLEMENT_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND => {
                header.validate(
                    BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
                let decoded: SettlementBookingErrorLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 settlement booking-error line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND => {
                header.validate(
                    BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND,
                    BOLT_V3_SETTLEMENT_GATE_ID,
                    index,
                )?;
                let decoded: TerminalSettlementLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 terminal settlement line at index {index}")
                    })?;
                decoded.validate_header(index)?;
            }
            BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND,
                    BOLT_V3_ORDER_LIFECYCLE_GATE_ID,
                    index,
                )?;
                let decoded: OrderLifecycleLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order lifecycle line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND,
                    BOLT_V3_ORDER_LIFECYCLE_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID,
                    index,
                )?;
                let decoded: VenueTruthCaptureFailureLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 venue truth capture failure line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID,
                    index,
                )?;
                let decoded: VenueTruthDivergenceLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 venue truth divergence line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND,
                    BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID,
                    index,
                )?;
            }
            BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND => {
                header.validate(
                    BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND,
                    BOLT_V3_REQUOTE_THROTTLE_GATE_ID,
                    index,
                )?;
                let decoded: RequoteThrottleLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 requote throttle line at index {index}")
                    })?;
                decoded.validate_header(
                    BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND,
                    BOLT_V3_REQUOTE_THROTTLE_GATE_ID,
                    index,
                )?;
            }
            other => {
                return Err(anyhow!(
                    "unsupported bolt-v3 decision evidence kind `{other}` at line index {index}"
                ));
            }
        }
    }

    let mut recovered = BTreeMap::new();
    for (client_order_id, metadata) in metadata_by_client_order_id {
        let fill_trade_ids = fills
            .iter()
            .filter(|fill| {
                fill.client_order_id == client_order_id
                    && fill.submit_reservation_id == metadata.submit_reservation_id
            })
            .map(|fill| fill.trade_id.clone())
            .collect::<BTreeSet<_>>();
        recovered.insert(
            client_order_id,
            BoltV3RecoveredSubmitReservationEvidence {
                metadata,
                fill_trade_ids,
            },
        );
    }

    Ok(BoltV3SubmitReservationRecoveryEvidence {
        metadata_by_client_order_id: recovered,
    })
}

fn decision_evidence_header_is_below_current_schema(
    header: &DecisionEvidenceEnvelopeHeader,
) -> bool {
    header.schema_version < BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
}

/// Audit-only (non-recovery) record kinds carry no reservation state, so an
/// older-schema instance is safe to skip rather than fail the entire recovery
/// read (otherwise one stale audit line poisons recovery of every valid
/// current-schema reservation after it). Reservation-bearing kinds
/// (submit_reservation_metadata / submit_reservation_fill) are deliberately NOT
/// skipped here: an unparseable legacy reservation record must still fail closed
/// at `header.validate`, so startup degrades to the unreconciled gate instead of
/// silently ignoring a possibly-open reservation.
fn decision_evidence_header_is_below_current_schema_non_recovery_record(
    header: &DecisionEvidenceEnvelopeHeader,
) -> bool {
    decision_evidence_header_is_below_current_schema(header)
        && matches!(
            header.kind.as_str(),
            BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND
                | BOLT_V3_ORDER_INTENT_RECORD_KIND
                | BOLT_V3_ADMISSION_DECISION_RECORD_KIND
                | BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND
                | BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND
                // Legacy pre-v14 audit-only kind. It never carries reservation state.
                | "position_sizer_rebuild"
                | BOLT_V3_ENTRY_SKIP_RECORD_KIND
                | BOLT_V3_EXIT_DECISION_RECORD_KIND
                | BOLT_V3_EXIT_EVALUATION_RECORD_KIND
                | BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND
                | BOLT_V3_ORDER_REJECT_RECORD_KIND
                | BOLT_V3_SETTLEMENT_RECORD_KIND
                | BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND
                | BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND
                | BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND
                | BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND
                | BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND
        )
}

/// Sole runtime raw-file reader for semantic decision-evidence consumers.
///
/// Callers receive decoded lines but never a file handle, keeping open/read
/// authority and the no-follow regular-file checks inside this module.
pub(crate) fn read_decision_evidence_jsonl_lines(path: &Path) -> Result<Vec<String>> {
    let file = open_regular_decision_evidence_file(path)
        .context("failed to open regular file bolt-v3 decision evidence")?;
    BufReader::new(file)
        .lines()
        .collect::<std::io::Result<Vec<_>>>()
        .context("failed to read bolt-v3 decision evidence file")
}

fn open_regular_decision_evidence_file(path: &Path) -> std::io::Result<fs::File> {
    let pre_open_metadata = fs::symlink_metadata(path)?;
    validate_decision_evidence_regular_file(&pre_open_metadata)?;
    let file = open_decision_evidence_file_no_follow(path)?;
    let opened_metadata = file.metadata()?;
    validate_decision_evidence_regular_file(&opened_metadata)?;
    validate_same_decision_evidence_file(&pre_open_metadata, &opened_metadata)?;
    let post_open_metadata = fs::symlink_metadata(path)?;
    validate_decision_evidence_regular_file(&post_open_metadata)?;
    validate_same_decision_evidence_file(&opened_metadata, &post_open_metadata)?;
    Ok(file)
}

fn open_decision_evidence_append_file(path: &Path) -> std::io::Result<fs::File> {
    match fs::symlink_metadata(path) {
        Ok(pre_open_metadata) => {
            validate_decision_evidence_regular_file(&pre_open_metadata)?;
            let file = open_decision_evidence_append_existing_no_follow(path)?;
            let opened_metadata = file.metadata()?;
            validate_decision_evidence_regular_file(&opened_metadata)?;
            validate_same_decision_evidence_file(&pre_open_metadata, &opened_metadata)?;
            let post_open_metadata = fs::symlink_metadata(path)?;
            validate_decision_evidence_regular_file(&post_open_metadata)?;
            validate_same_decision_evidence_file(&opened_metadata, &post_open_metadata)?;
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let file = open_decision_evidence_append_new_no_follow(path)?;
            let opened_metadata = file.metadata()?;
            validate_decision_evidence_regular_file(&opened_metadata)?;
            let post_open_metadata = fs::symlink_metadata(path)?;
            validate_decision_evidence_regular_file(&post_open_metadata)?;
            validate_same_decision_evidence_file(&opened_metadata, &post_open_metadata)?;
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

fn open_decision_evidence_append_existing_no_follow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.append(true);
    configure_decision_evidence_append_options(&mut options);
    options.open(path)
}

fn open_decision_evidence_append_new_no_follow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.append(true).create_new(true);
    configure_decision_evidence_append_options(&mut options);
    options.open(path)
}

#[cfg(unix)]
fn configure_decision_evidence_append_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options
        .mode(PRIVATE_ARTIFACT_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn configure_decision_evidence_append_options(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn open_decision_evidence_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_decision_evidence_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().read(true).open(path)
}

fn validate_decision_evidence_regular_file(metadata: &fs::Metadata) -> std::io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bolt-v3 decision evidence path is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_same_decision_evidence_file(
    left: &fs::Metadata,
    right: &fs::Metadata,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if left.dev() != right.dev() || left.ino() != right.ino() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid bolt-v3 decision evidence file identity during open",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_same_decision_evidence_file(
    _left: &fs::Metadata,
    _right: &fs::Metadata,
) -> std::io::Result<()> {
    Ok(())
}

fn validate_entry_decision_chain(
    snapshot: BoltV3StrategyInputEvidenceSnapshot,
    intent: BoltV3OrderIntentEvidence,
    admission: BoltV3AdmissionDecisionEvidence,
) -> Result<BoltV3EntryDecisionEvidenceChain> {
    if snapshot.strategy_id != intent.strategy_id || snapshot.strategy_id != admission.strategy_id {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence strategy_id mismatch"
        ));
    }
    if snapshot.submission_instrument_id != intent.instrument_id
        || snapshot.submission_instrument_id != admission.instrument_id
    {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence instrument_id mismatch"
        ));
    }
    if snapshot.submission_order_side != intent.order_side {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence order_side mismatch"
        ));
    }
    if snapshot.submission_price != intent.price {
        return Err(anyhow!("bolt-v3 entry decision evidence price mismatch"));
    }
    if snapshot.submission_quantity != intent.quantity {
        return Err(anyhow!("bolt-v3 entry decision evidence quantity mismatch"));
    }
    Ok(BoltV3EntryDecisionEvidenceChain {
        snapshot,
        intent,
        admission,
    })
}

fn validate_submit_reservation_metadata(
    metadata: &BoltV3SubmitReservationMetadataEvidence,
) -> Result<()> {
    for (field, value) in [
        ("client_order_id", metadata.client_order_id.as_str()),
        (
            "submit_reservation_id",
            metadata.submit_reservation_id.as_str(),
        ),
        ("venue_id", metadata.venue_id.as_str()),
        ("account_id", metadata.account_id.as_str()),
        ("product_kind", metadata.product_kind.as_str()),
        ("collateral_currency", metadata.collateral_currency.as_str()),
        ("capital_pool_id", metadata.capital_pool_id.as_str()),
        ("collateral_group_id", metadata.collateral_group_id.as_str()),
        ("instrument_id", metadata.instrument_id.as_str()),
        ("side", metadata.side.as_str()),
        ("source", metadata.source.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(anyhow!(
                "submit reservation metadata {field} must be non-empty"
            ));
        }
    }
    if metadata.observed_at_ns == 0 {
        return Err(anyhow!(
            "submit reservation metadata observed_at_ns must be positive"
        ));
    }
    if metadata.product_kind != SUBMIT_RESERVATION_METADATA_PRODUCT_KIND_BINARY {
        return Err(anyhow!(
            "submit reservation metadata product_kind must use canonical `{}` encoding",
            SUBMIT_RESERVATION_METADATA_PRODUCT_KIND_BINARY
        ));
    }
    if !matches!(
        metadata.side.as_str(),
        SUBMIT_RESERVATION_METADATA_SIDE_BUY | SUBMIT_RESERVATION_METADATA_SIDE_SELL
    ) {
        return Err(anyhow!(
            "submit reservation metadata side must use canonical `{}` or `{}` encoding",
            SUBMIT_RESERVATION_METADATA_SIDE_BUY,
            SUBMIT_RESERVATION_METADATA_SIDE_SELL
        ));
    }
    require_positive_decimal(
        &metadata.submitted_quantity,
        "submit reservation metadata submitted_quantity",
    )?;
    require_non_negative_decimal(
        &metadata.liability_factor,
        "submit reservation metadata liability_factor",
    )?;
    require_non_negative_decimal(
        &metadata.additive_liability,
        "submit reservation metadata additive_liability",
    )?;
    require_positive_decimal(
        &metadata.reserved_liability,
        "submit reservation metadata reserved_liability",
    )?;
    Ok(())
}

fn validate_submit_reservation_fill(fill: &BoltV3SubmitReservationFillEvidence) -> Result<()> {
    for (field, value) in [
        ("client_order_id", fill.client_order_id.as_str()),
        ("submit_reservation_id", fill.submit_reservation_id.as_str()),
        ("trade_id", fill.trade_id.as_str()),
        ("instrument_id", fill.instrument_id.as_str()),
        ("side", fill.side.as_str()),
        ("source", fill.source.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(anyhow!("submit reservation fill {field} must be non-empty"));
        }
    }
    if fill.observed_at_ns == 0 {
        return Err(anyhow!(
            "submit reservation fill observed_at_ns must be positive"
        ));
    }
    require_positive_decimal(&fill.fill_quantity, "submit reservation fill fill_quantity")?;
    Ok(())
}

fn require_positive_decimal(value: &str, field: &str) -> Result<Decimal> {
    let decimal = parse_decimal(value, field)?;
    if decimal <= Decimal::ZERO {
        return Err(anyhow!("{field} must be positive"));
    }
    Ok(decimal)
}

fn require_non_negative_decimal(value: &str, field: &str) -> Result<Decimal> {
    let decimal = parse_decimal(value, field)?;
    if decimal < Decimal::ZERO {
        return Err(anyhow!("{field} must be non-negative"));
    }
    Ok(decimal)
}

fn parse_decimal(value: &str, field: &str) -> Result<Decimal> {
    value
        .parse::<Decimal>()
        .with_context(|| format!("{field} must parse as decimal"))
}

#[derive(Deserialize)]
struct DecisionEvidenceEnvelopeHeader {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
}

impl DecisionEvidenceEnvelopeHeader {
    fn validate(&self, expected_kind: &str, expected_gate_id: &str, index: usize) -> Result<()> {
        if self.schema_version != BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION {
            return Err(anyhow!(
                "bolt-v3 decision evidence schema_version mismatch at line index {index}"
            ));
        }
        if self.recorded_at_utc_ns <= 0 {
            return Err(anyhow!(
                "bolt-v3 decision evidence recorded_at_utc_ns must be positive at line index {index}"
            ));
        }
        if self.gate_id != expected_gate_id {
            return Err(anyhow!(
                "bolt-v3 decision evidence gate_id mismatch at line index {index}"
            ));
        }
        if self.gate_version != BOLT_V3_DECISION_EVIDENCE_GATE_VERSION {
            return Err(anyhow!(
                "bolt-v3 decision evidence gate_version mismatch at line index {index}"
            ));
        }
        if self.kind != expected_kind {
            return Err(anyhow!(
                "bolt-v3 decision evidence kind mismatch at line index {index}"
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct StrategyInputSnapshotLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    snapshot: BoltV3StrategyInputEvidenceSnapshot,
}

impl StrategyInputSnapshotLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

#[derive(Deserialize)]
struct OrderIntentLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    intent: BoltV3OrderIntentEvidence,
}

impl OrderIntentLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

#[derive(Deserialize)]
struct AdmissionDecisionLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    decision: BoltV3AdmissionDecisionEvidence,
}

#[derive(Deserialize)]
struct BasketAdmissionDecisionLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    decision: BoltV3BasketAdmissionDecisionEvidence,
}

#[derive(Deserialize)]
struct CapitalAdmissionRebuildAuditLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    audit: BoltV3CapitalAdmissionRebuildAuditEvidence,
}

#[derive(Deserialize)]
struct SubmitReservationMetadataLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    metadata: BoltV3SubmitReservationMetadataEvidence,
}

#[derive(Deserialize)]
struct SubmitReservationFillLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    fill: BoltV3SubmitReservationFillEvidence,
}

#[derive(Deserialize)]
struct EntrySkipLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    entry_skip: BoltV3EntrySkipEvidence,
}

#[derive(Deserialize)]
struct ExitDecisionLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    exit_decision: BoltV3ExitDecisionEvidence,
}

#[derive(Deserialize)]
struct RequoteThrottleLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    requote_throttle: BoltV3RequoteThrottleEvidence,
}

#[derive(Deserialize)]
struct VenueTruthCaptureFailureLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    capture_failure: VenueTruthCaptureFailureEvidence,
}

#[derive(Deserialize)]
struct VenueTruthDivergenceLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    divergence: VenueTruthDivergenceEvidence,
}

impl AdmissionDecisionLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl BasketAdmissionDecisionLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.decision;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl CapitalAdmissionRebuildAuditLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.audit;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl SubmitReservationMetadataLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.metadata;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl SubmitReservationFillLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.fill;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl EntrySkipLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.entry_skip;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl ExitDecisionLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.exit_decision;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl RequoteThrottleLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.requote_throttle;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl VenueTruthCaptureFailureLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.capture_failure;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

impl VenueTruthDivergenceLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.divergence;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

#[derive(Serialize)]
struct OrderIntentLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    intent: &'a BoltV3OrderIntentEvidence,
}

#[derive(Serialize)]
struct StrategyInputSnapshotLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    snapshot: &'a BoltV3StrategyInputEvidenceSnapshot,
}

#[derive(Serialize)]
struct AdmissionDecisionLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    decision: &'a BoltV3AdmissionDecisionEvidence,
}

#[derive(Serialize)]
struct BasketAdmissionDecisionLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    decision: &'a BoltV3BasketAdmissionDecisionEvidence,
}

#[derive(Serialize)]
struct CapitalAdmissionRebuildAuditLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    audit: &'a BoltV3CapitalAdmissionRebuildAuditEvidence,
}

#[derive(Serialize)]
struct SubmitReservationMetadataLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    metadata: &'a BoltV3SubmitReservationMetadataEvidence,
}

#[derive(Serialize)]
struct SubmitReservationFillLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    fill: &'a BoltV3SubmitReservationFillEvidence,
}

#[derive(Serialize)]
struct EntrySkipLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    entry_skip: &'a BoltV3EntrySkipEvidence,
}

#[derive(Serialize)]
struct ExitDecisionLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    exit_decision: &'a BoltV3ExitDecisionEvidence,
}

#[derive(Serialize)]
struct RequoteThrottleLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    requote_throttle: &'a BoltV3RequoteThrottleEvidence,
}

#[derive(Serialize)]
struct VenueTruthCaptureFailureLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    capture_failure: &'a VenueTruthCaptureFailureEvidence,
}

#[derive(Serialize)]
struct VenueTruthDivergenceLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    divergence: &'a VenueTruthDivergenceEvidence,
}

fn current_utc_ns() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("UTC timestamp must fit in i64 nanoseconds")
}

fn encode_order_intent_line(intent: &BoltV3OrderIntentEvidence) -> Result<Vec<u8>> {
    let envelope = OrderIntentLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_ORDER_INTENT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: "order_intent",
        intent,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize order intent decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_strategy_input_snapshot_line(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
) -> Result<Vec<u8>> {
    let envelope = StrategyInputSnapshotLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: "strategy_input_snapshot",
        snapshot,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize strategy input evidence snapshot")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_admission_decision_line(decision: &BoltV3AdmissionDecisionEvidence) -> Result<Vec<u8>> {
    let envelope = AdmissionDecisionLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: "admission_decision",
        decision,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize admission decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_basket_admission_decision_line(
    decision: &BoltV3BasketAdmissionDecisionEvidence,
) -> Result<Vec<u8>> {
    let envelope = BasketAdmissionDecisionLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND,
        decision,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize basket admission decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_capital_admission_rebuild_audit_line(
    audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
) -> Result<Vec<u8>> {
    let envelope = CapitalAdmissionRebuildAuditLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_CAPITAL_ADMISSION_REBUILD_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND,
        audit,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize capital admission rebuild audit evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_submit_reservation_metadata_line(
    metadata: &BoltV3SubmitReservationMetadataEvidence,
) -> Result<Vec<u8>> {
    let envelope = SubmitReservationMetadataLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND,
        metadata,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize submit reservation metadata evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_submit_reservation_fill_line(
    fill: &BoltV3SubmitReservationFillEvidence,
) -> Result<Vec<u8>> {
    let envelope = SubmitReservationFillLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND,
        fill,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize submit reservation fill evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_entry_skip_line(skip: &BoltV3EntrySkipEvidence) -> Result<Vec<u8>> {
    let envelope = EntrySkipLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_ENTRY_SKIP_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_ENTRY_SKIP_RECORD_KIND,
        entry_skip: skip,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize entry skip evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct ExitEvaluationLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    evidence: &'a BoltV3ExitEvaluationEvidence,
}

#[derive(Deserialize)]
struct ExitEvaluationLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    evidence: BoltV3ExitEvaluationEvidence,
}

impl ExitEvaluationLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.evidence;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

fn encode_exit_evaluation_line(evidence: &BoltV3ExitEvaluationEvidence) -> Result<Vec<u8>> {
    if evidence.trigger_ts_init_ms.is_some_and(i64::is_negative) {
        anyhow::bail!("trigger_ts_init_ms must be non-negative");
    }
    if evidence
        .rv_snapshot_receive_watermark_ms
        .is_some_and(i64::is_negative)
    {
        anyhow::bail!("rv_snapshot_receive_watermark_ms must be non-negative");
    }
    let envelope = ExitEvaluationLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_EXIT_EVALUATION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
        evidence,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize exit evaluation evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_exit_decision_line(decision: &BoltV3ExitDecisionEvidence) -> Result<Vec<u8>> {
    let envelope = ExitDecisionLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_EXIT_DECISION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_EXIT_DECISION_RECORD_KIND,
        exit_decision: decision,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize exit decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct LossGovernorHaltLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    evidence: &'a BoltV3LossGovernorHaltEvidence,
}

#[derive(Deserialize)]
struct LossGovernorHaltLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    evidence: BoltV3LossGovernorHaltEvidence,
}

impl LossGovernorHaltLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.evidence;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

fn encode_loss_governor_halt_line(evidence: &BoltV3LossGovernorHaltEvidence) -> Result<Vec<u8>> {
    let envelope = LossGovernorHaltLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
        evidence,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize loss governor halt evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_requote_throttle_line(throttle: &BoltV3RequoteThrottleEvidence) -> Result<Vec<u8>> {
    let envelope = RequoteThrottleLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_REQUOTE_THROTTLE_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND,
        requote_throttle: throttle,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize requote throttle evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct SettlementLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    settlement: &'a BoltV3SettlementEvidence,
}

#[derive(Deserialize)]
struct SettlementLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    settlement: BoltV3SettlementEvidence,
}

impl SettlementLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.settlement;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

fn encode_settlement_line(evidence: &BoltV3SettlementEvidence) -> Result<Vec<u8>> {
    let envelope = SettlementLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SETTLEMENT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_SETTLEMENT_RECORD_KIND,
        settlement: evidence,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize settlement evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct SettlementBookingErrorLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    booking_error: &'a BoltV3SettlementBookingErrorEvidence,
}

#[derive(Deserialize)]
struct SettlementBookingErrorLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    booking_error: BoltV3SettlementBookingErrorEvidence,
}

impl SettlementBookingErrorLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.booking_error;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

fn encode_settlement_booking_error_line(
    evidence: &BoltV3SettlementBookingErrorEvidence,
) -> Result<Vec<u8>> {
    let envelope = SettlementBookingErrorLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SETTLEMENT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
        booking_error: evidence,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize settlement booking-error evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct TerminalSettlementLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    terminal_settlement: &'a BoltV3TerminalSettlementEvidence,
}

#[derive(Deserialize)]
struct TerminalSettlementLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    terminal_settlement: BoltV3TerminalSettlementEvidence,
}

impl TerminalSettlementLineOwned {
    fn validate_header(&self, index: usize) -> Result<()> {
        self.header.validate(
            BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND,
            BOLT_V3_SETTLEMENT_GATE_ID,
            index,
        )?;
        self.terminal_settlement
            .validate()
            .with_context(|| format!("invalid terminal settlement evidence at index {index}"))
    }
}

fn encode_terminal_settlement_line(evidence: &BoltV3TerminalSettlementEvidence) -> Result<Vec<u8>> {
    evidence
        .validate()
        .context("invalid terminal settlement evidence")?;
    let envelope = TerminalSettlementLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SETTLEMENT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND,
        terminal_settlement: evidence,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize terminal settlement evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_venue_truth_capture_failure_line(
    evidence: &VenueTruthCaptureFailureEvidence,
) -> Result<Vec<u8>> {
    let envelope = VenueTruthCaptureFailureLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND,
        capture_failure: evidence,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize venue truth capture failure evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_venue_truth_divergence_line(evidence: &VenueTruthDivergenceEvidence) -> Result<Vec<u8>> {
    let envelope = VenueTruthDivergenceLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND,
        divergence: evidence,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize venue truth divergence evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct OrderRejectLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    evidence: &'a BoltV3OrderRejectEvidence,
}

#[derive(Deserialize)]
struct OrderRejectLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    evidence: BoltV3OrderRejectEvidence,
}

impl OrderRejectLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.evidence;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

fn encode_order_reject_line(evidence: &BoltV3OrderRejectEvidence) -> Result<Vec<u8>> {
    let envelope = OrderRejectLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_ORDER_REJECT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_ORDER_REJECT_RECORD_KIND,
        evidence,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize order reject evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[derive(Serialize)]
struct OrderLifecycleLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    evidence: &'a BoltV3OrderLifecycleEvidence,
}

#[derive(Deserialize)]
struct OrderLifecycleLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    evidence: BoltV3OrderLifecycleEvidence,
}

impl OrderLifecycleLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.evidence;
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

fn encode_order_lifecycle_line(evidence: &BoltV3OrderLifecycleEvidence) -> Result<Vec<u8>> {
    let envelope = OrderLifecycleLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_ORDER_LIFECYCLE_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND,
        evidence,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize order lifecycle evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

/// Reads every `exit_evaluation` record (current schema) from a decision-evidence
/// log, in file order. Records of other kinds and older schema versions are skipped,
/// so this targeted reader is resilient to forward-compatible additions.
pub fn read_exit_evaluation_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3ExitEvaluationEvidence>> {
    read_kind_evidence(
        path,
        max_bytes,
        BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
        BOLT_V3_EXIT_EVALUATION_GATE_ID,
        |line, index| {
            let decoded: ExitEvaluationLineOwned =
                serde_json::from_slice(line).with_context(|| {
                    format!("failed to parse bolt-v3 exit evaluation line at index {index}")
                })?;
            decoded.validate_header(
                BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
                BOLT_V3_EXIT_EVALUATION_GATE_ID,
                index,
            )?;
            Ok(decoded.evidence)
        },
    )
}

/// Reads every `loss_governor_halt` record (current schema) from a decision-evidence
/// log, in file order. See [`read_exit_evaluation_evidence`] for skip semantics.
pub fn read_loss_governor_halt_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3LossGovernorHaltEvidence>> {
    read_kind_evidence(
        path,
        max_bytes,
        BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
        BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
        |line, index| {
            let decoded: LossGovernorHaltLineOwned =
                serde_json::from_slice(line).with_context(|| {
                    format!("failed to parse bolt-v3 loss governor halt line at index {index}")
                })?;
            decoded.validate_header(
                BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
                BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
                index,
            )?;
            Ok(decoded.evidence)
        },
    )
}

/// Reads every `order_reject` record (current schema) from a decision-evidence log,
/// in file order. See [`read_exit_evaluation_evidence`] for skip semantics.
pub fn read_order_reject_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3OrderRejectEvidence>> {
    read_kind_evidence(
        path,
        max_bytes,
        BOLT_V3_ORDER_REJECT_RECORD_KIND,
        BOLT_V3_ORDER_REJECT_GATE_ID,
        |line, index| {
            let decoded: OrderRejectLineOwned =
                serde_json::from_slice(line).with_context(|| {
                    format!("failed to parse bolt-v3 order reject line at index {index}")
                })?;
            decoded.validate_header(
                BOLT_V3_ORDER_REJECT_RECORD_KIND,
                BOLT_V3_ORDER_REJECT_GATE_ID,
                index,
            )?;
            Ok(decoded.evidence)
        },
    )
}

/// Reads every `settlement` record (current schema) from a decision-evidence log,
/// in file order. Duplicate settlement keys fail closed because startup uses these
/// keys as the idempotency proof for settlement booking.
pub fn read_settlement_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3SettlementEvidence>> {
    let records = read_settlement_evidence_records(path, max_bytes)?;
    fail_closed_on_duplicate_settlement_keys(&records)?;
    Ok(records)
}

fn read_settlement_evidence_records(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3SettlementEvidence>> {
    read_kind_evidence(
        path,
        max_bytes,
        BOLT_V3_SETTLEMENT_RECORD_KIND,
        BOLT_V3_SETTLEMENT_GATE_ID,
        |line, index| {
            let decoded: SettlementLineOwned = serde_json::from_slice(line).with_context(|| {
                format!("failed to parse bolt-v3 settlement line at index {index}")
            })?;
            decoded.validate_header(
                BOLT_V3_SETTLEMENT_RECORD_KIND,
                BOLT_V3_SETTLEMENT_GATE_ID,
                index,
            )?;
            Ok(decoded.settlement)
        },
    )
}

/// Reads every `settlement_booking_error` record (current schema) from a
/// decision-evidence log, in file order. These records are audit evidence for
/// accepted fail-closed settlement behavior; startup recovery filters them by
/// the same structural settlement-key scope used for settled keys.
pub fn read_settlement_booking_error_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3SettlementBookingErrorEvidence>> {
    read_kind_evidence(
        path,
        max_bytes,
        BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
        BOLT_V3_SETTLEMENT_GATE_ID,
        |line, index| {
            let decoded: SettlementBookingErrorLineOwned = serde_json::from_slice(line)
                .with_context(|| {
                    format!(
                        "failed to parse bolt-v3 settlement booking-error line at index {index}"
                    )
                })?;
            decoded.validate_header(
                BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND,
                BOLT_V3_SETTLEMENT_GATE_ID,
                index,
            )?;
            Ok(decoded.booking_error)
        },
    )
}

/// Reads the single production schema written by every terminal release.
pub fn read_terminal_settlement_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3TerminalSettlementEvidence>> {
    read_kind_evidence(
        path,
        max_bytes,
        BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND,
        BOLT_V3_SETTLEMENT_GATE_ID,
        |line, index| {
            let decoded: TerminalSettlementLineOwned =
                serde_json::from_slice(line).with_context(|| {
                    format!("failed to parse bolt-v3 terminal settlement line at index {index}")
                })?;
            decoded.validate_header(index)?;
            Ok(decoded.terminal_settlement)
        },
    )
}

pub fn read_terminal_settlement_keys_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    Ok(read_terminal_settlement_evidence(path, max_bytes)?
        .into_iter()
        .filter_map(|evidence| {
            recovery_scope_settlement_keys
                .contains(&evidence.settlement_key)
                .then_some(evidence.settlement_key)
        })
        .collect())
}

/// Seeds startup settlement idempotency from durable settlement keys relevant to
/// positions currently within recovery scope. The bound is structural: the caller
/// supplies the position-derived keys; this reader never truncates by count or age.
pub fn read_settlement_keys_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let settlements = read_settlement_evidence_records(path, max_bytes)?;
    let mut recovered = BTreeSet::new();
    for evidence in settlements {
        if !recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            continue;
        }
        if !recovered.insert(evidence.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&evidence.settlement_key));
        }
    }
    Ok(recovered)
}

pub fn read_settlement_booking_error_keys_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let path = path.as_ref();
    let booking_errors = read_settlement_booking_error_evidence(path, max_bytes)?;
    let mut recovered = BTreeSet::new();
    for evidence in booking_errors {
        if !recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            continue;
        }
        if !recovered.insert(evidence.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&evidence.settlement_key));
        }
    }
    for evidence in read_terminal_settlement_evidence(path, max_bytes)? {
        if recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            // Canonical keys seed recovery eligibility while the dedicated
            // canonical-key set prevents restart from appending the record again.
            recovered.insert(evidence.settlement_key);
        }
    }
    Ok(recovered)
}

pub fn read_settlement_evidence_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<Vec<BoltV3SettlementEvidence>> {
    let settlements = read_settlement_evidence_records(path, max_bytes)?;
    let mut seen = BTreeSet::new();
    let mut recovered = Vec::new();
    for evidence in settlements {
        if !recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            continue;
        }
        if !seen.insert(evidence.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&evidence.settlement_key));
        }
        recovered.push(evidence);
    }
    Ok(recovered)
}

fn fail_closed_on_duplicate_settlement_keys(records: &[BoltV3SettlementEvidence]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for record in records {
        if !seen.insert(record.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&record.settlement_key));
        }
    }
    Ok(())
}

fn duplicate_settlement_key_error(settlement_key: &str) -> anyhow::Error {
    anyhow!("duplicate settlement key `{settlement_key}` in bolt-v3 settlement evidence")
}

/// Shared body for the kind-specific evidence readers above. Reads the whole file
/// under `max_bytes`, then for each non-empty line parses the envelope header,
/// skips records of other kinds and older schema versions, and decodes matching
/// lines via `decode`.
fn read_kind_evidence<T>(
    path: impl AsRef<Path>,
    max_bytes: u64,
    target_kind: &str,
    expected_gate_id: &str,
    decode: impl Fn(&[u8], usize) -> Result<T>,
) -> Result<Vec<T>> {
    let path = path.as_ref();
    let mut file = open_regular_decision_evidence_file(path)
        .context("failed to open regular file bolt-v3 decision evidence")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("failed to read bolt-v3 decision evidence file")?;
    if bytes.len() as u64 > max_bytes {
        return Err(anyhow!(
            "bolt-v3 decision evidence file exceeds max_bytes={max_bytes}"
        ));
    }
    let mut records = Vec::new();
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let header: DecisionEvidenceEnvelopeHeader =
            serde_json::from_slice(line).with_context(|| {
                format!("failed to parse bolt-v3 decision evidence envelope at line index {index}")
            })?;
        if header.schema_version < BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION {
            continue;
        }
        if header.kind.as_str() != target_kind {
            continue;
        }
        header.validate(target_kind, expected_gate_id, index)?;
        records.push(decode(line, index)?);
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{OrderSide, OrderType, TimeInForce, TriggerType},
        identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
        orders::StopMarketOrder,
        types::{Price, Quantity},
    };

    #[test]
    fn decision_evidence_schema_version_tracks_position_interval_wire_shape() {
        assert_eq!(BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION, 15);
    }

    #[test]
    fn probability_evidence_uses_probability_value_bytes() {
        let probability = Probability::new(0.6).expect("fixture probability");

        assert_eq!(probability_evidence(probability), "0.6");
        assert_eq!(
            option_probability_evidence(Some(probability)),
            Some("0.6".to_string())
        );
        assert_eq!(option_probability_evidence(None), None);
    }

    mod decision_commit_chokepoint {
        use super::*;

        #[test]
        fn new_risk_aborts_act_when_evidence_write_fails() {
            let acted = std::cell::Cell::new(false);
            let result: anyhow::Result<i32> = commit_decision(
                RiskDirection::NewRisk,
                || Err(anyhow::anyhow!("evidence write boom")),
                || {
                    acted.set(true);
                    Ok(7)
                },
            );
            assert!(
                result.is_err(),
                "a risk-increasing evidence-write failure must fail closed"
            );
            assert!(
                !acted.get(),
                "the irreversible act must not run after a fail-closed abort"
            );
        }

        #[test]
        fn risk_reducing_runs_act_despite_evidence_write_failure() {
            let acted = std::cell::Cell::new(false);
            let result: anyhow::Result<i32> = commit_decision(
                RiskDirection::RiskReducing,
                || Err(anyhow::anyhow!("evidence write boom")),
                || {
                    acted.set(true);
                    Ok(7)
                },
            );
            assert_eq!(
                result.expect("risk reduction must never be blocked by an evidence-write failure"),
                7
            );
            assert!(
                acted.get(),
                "the risk-reducing act must run even when the evidence write fails"
            );
        }

        #[test]
        fn neutral_runs_act_despite_evidence_write_failure() {
            let acted = std::cell::Cell::new(false);
            let result: anyhow::Result<i32> = commit_decision(
                RiskDirection::Neutral,
                || Err(anyhow::anyhow!("evidence write boom")),
                || {
                    acted.set(true);
                    Ok(1)
                },
            );
            assert_eq!(result.expect("a neutral act must not be blocked"), 1);
            assert!(acted.get());
        }

        #[test]
        fn emit_runs_before_act_on_success() {
            let trace = std::cell::Cell::new(String::new());
            let result: anyhow::Result<&str> = commit_decision(
                RiskDirection::NewRisk,
                || {
                    trace.set(format!("{}emit;", trace.take()));
                    Ok(())
                },
                || {
                    trace.set(format!("{}act;", trace.take()));
                    Ok("done")
                },
            );
            assert_eq!(result.expect("success path returns the act value"), "done");
            assert_eq!(
                trace.take(),
                "emit;act;",
                "the durable record must be emitted before the irreversible act runs"
            );
        }

        #[test]
        fn record_only_new_risk_propagates_write_failure() {
            let result = record_decision(RiskDirection::NewRisk, || Err(anyhow::anyhow!("boom")));
            assert!(
                result.is_err(),
                "a risk-increasing record-only decision must fail closed"
            );
        }

        #[test]
        fn record_only_risk_reducing_swallows_write_failure() {
            let result =
                record_decision(RiskDirection::RiskReducing, || Err(anyhow::anyhow!("boom")));
            assert!(
                result.is_ok(),
                "a risk-reducing record-only decision must not surface the write error to the caller"
            );
        }

        #[test]
        fn evidence_write_failure_blocks_only_for_new_risk() {
            assert!(RiskDirection::NewRisk.evidence_write_failure_blocks());
            assert!(!RiskDirection::RiskReducing.evidence_write_failure_blocks());
            assert!(!RiskDirection::Neutral.evidence_write_failure_blocks());
        }
    }

    fn parse_line(line: &[u8]) -> serde_json::Value {
        assert!(line.ends_with(b"\n"), "line must end with newline");
        let json = std::str::from_utf8(&line[..line.len() - 1]).expect("line is utf8");
        serde_json::from_str(json).expect("line is json")
    }

    #[test]
    fn old_strategy_input_snapshot_schema_with_reference_fair_value_is_rejected() {
        let line = br#"{
            "schema_version":9,
            "recorded_at_utc_ns":1,
            "gate_id":"bolt_v3.strategy_input_snapshot",
            "gate_version":"0.1.0",
            "kind":"strategy_input_snapshot",
            "snapshot":{"reference_fair_value":"100.0"}
        }"#;
        let header: DecisionEvidenceEnvelopeHeader =
            serde_json::from_slice(line).expect("old envelope header should parse");

        let err = header
            .validate(
                BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND,
                BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                0,
            )
            .expect_err("old v9 strategy-input snapshots must fail the schema gate");

        assert!(
            err.to_string().contains("schema_version mismatch"),
            "old evidence should fail on schema version, got: {err:#}"
        );
    }

    #[test]
    fn legacy_schema_13_rebuild_audit_skips_but_reservations_fail_closed() {
        let legacy_audit = DecisionEvidenceEnvelopeHeader {
            schema_version: 13,
            recorded_at_utc_ns: 1,
            gate_id: "bolt_v3.position_sizer_rebuild".to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: "position_sizer_rebuild".to_string(),
        };
        assert!(
            decision_evidence_header_is_below_current_schema_non_recovery_record(&legacy_audit),
            "legacy schema-13 audit-only rebuild records must remain skippable"
        );

        let legacy_reservation = DecisionEvidenceEnvelopeHeader {
            schema_version: 13,
            recorded_at_utc_ns: 1,
            gate_id: BOLT_V3_SUBMIT_ADMISSION_GATE_ID.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND.to_string(),
        };
        assert!(
            !decision_evidence_header_is_below_current_schema_non_recovery_record(
                &legacy_reservation
            ),
            "legacy schema-13 reservation records must not be skipped"
        );
        let error = legacy_reservation
            .validate(
                BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND,
                BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                0,
            )
            .expect_err("legacy schema-13 reservation metadata must fail closed");
        assert!(
            error.to_string().contains("schema_version mismatch"),
            "reservation metadata should fail closed on schema mismatch, got: {error:#}"
        );
    }

    #[test]
    fn encode_order_intent_line_wraps_intent_with_metadata() {
        let intent = BoltV3OrderIntentEvidence {
            strategy_id: "strategy-one".to_string(),
            intent_kind: BoltV3OrderIntentKind::Entry,
            instrument_id: "instrument-one".to_string(),
            client_order_id: "client-order-one".to_string(),
            order_side: OrderSide::Buy.to_string(),
            price: "0.42".to_string(),
            quantity: "1".to_string(),
            clamp_outcome: None,
            order_fields: BoltV3OrderIntentOrderFields {
                order_type: OrderType::Limit.to_string(),
                time_in_force: TimeInForce::Gtc.to_string(),
                price: Some("0.42".to_string()),
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                expire_time_unix_nanos: None,
                is_post_only: true,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
        };

        let line = encode_order_intent_line(&intent).expect("intent should encode");
        let decoded = parse_line(&line);

        assert_eq!(
            decoded["schema_version"],
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded["gate_id"], BOLT_V3_ORDER_INTENT_GATE_ID);
        assert_eq!(
            decoded["gate_version"],
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION
        );
        assert_eq!(decoded["kind"], "order_intent");
        assert!(
            decoded["recorded_at_utc_ns"]
                .as_i64()
                .map(|ns| ns > 0)
                .unwrap_or(false),
            "recorded_at_utc_ns must be a positive i64; got {:?}",
            decoded["recorded_at_utc_ns"]
        );
        let intent = &decoded["intent"];
        assert_eq!(intent["strategy_id"], "strategy-one");
        assert_eq!(intent["intent_kind"], "entry");
        assert_eq!(intent["order_side"], OrderSide::Buy.to_string());
        assert_eq!(intent["clamp_outcome"], serde_json::Value::Null);
        assert_eq!(
            intent["order_fields"]["order_type"],
            OrderType::Limit.to_string()
        );
        assert_eq!(
            intent["order_fields"]["time_in_force"],
            TimeInForce::Gtc.to_string()
        );
        assert_eq!(intent["order_fields"]["price"], "0.42");
        assert_eq!(
            intent["order_fields"]["trigger_price"],
            serde_json::Value::Null
        );
        assert_eq!(intent["order_fields"]["is_post_only"], true);
        assert_eq!(intent["order_fields"]["is_reduce_only"], false);
        assert_eq!(intent["order_fields"]["is_quote_quantity"], false);
    }

    #[test]
    fn order_intent_from_compiled_order_binds_selected_nt_order_fields() {
        let quantity = Quantity::new(2.0, 2);
        let trigger_price = Price::new(0.52, 2);
        let trigger_instrument_id = InstrumentId::from("trigger-instrument.SIM");
        let order = OrderAny::StopMarket(
            StopMarketOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-one"),
                InstrumentId::from("instrument-one.SIM"),
                ClientOrderId::from("client-order-one"),
                OrderSide::Buy,
                quantity,
                trigger_price,
                TriggerType::LastPrice,
                TimeInForce::Gtc,
                None,
                false,
                false,
                None,
                None,
                Some(trigger_instrument_id),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                UUID4::new(),
                UnixNanos::from(1_u64),
            )
            .expect("stop-market order should be valid"),
        );

        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            "strategy-one".to_string(),
            BoltV3OrderIntentKind::Entry,
            "0.42".to_string(),
            &order,
        );

        assert_eq!(intent.instrument_id, order.instrument_id().to_string());
        assert_eq!(intent.client_order_id, order.client_order_id().to_string());
        assert_eq!(intent.order_side, order.order_side().to_string());
        assert_eq!(intent.price, trigger_price.to_string());
        assert_eq!(intent.quantity, quantity.to_string());
        assert_eq!(
            intent.order_fields.order_type,
            OrderType::StopMarket.to_string()
        );
        assert_eq!(
            intent.order_fields.time_in_force,
            TimeInForce::Gtc.to_string()
        );
        assert_eq!(intent.order_fields.price, None);
        assert_eq!(
            intent.order_fields.trigger_price,
            Some(trigger_price.to_string())
        );
        assert_eq!(
            intent.order_fields.trigger_type,
            Some(TriggerType::LastPrice.to_string())
        );
        assert_eq!(
            intent.order_fields.trigger_instrument_id,
            Some(trigger_instrument_id.to_string())
        );
        assert!(!intent.order_fields.is_post_only);
        assert!(!intent.order_fields.is_reduce_only);
        assert!(!intent.order_fields.is_quote_quantity);
    }

    #[test]
    fn compiled_order_price_source_prefers_activation_price_before_fallback() {
        let activation_price = Price::new(0.48, 2).to_string();
        let fallback_price = Price::new(0.40, 2).to_string();

        assert_eq!(
            selected_compiled_order_price_source(
                None,
                None,
                Some(activation_price.clone()),
                fallback_price,
            ),
            activation_price
        );
    }

    #[test]
    fn encode_strategy_input_snapshot_line_wraps_snapshot_with_metadata() {
        let snapshot = BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: "strategy-one".to_string(),
            configured_target_id: "target-one".to_string(),
            market_selection_ruleset_id: "target-one".to_string(),
            market_selection_outcome: "current".to_string(),
            market_id: Some("market-one".to_string()),
            polymarket_condition_id: Some("condition-one".to_string()),
            polymarket_market_slug: Some("market-slug-one".to_string()),
            polymarket_question_id: Some("question-one".to_string()),
            up_instrument_id: Some("instrument-up".to_string()),
            down_instrument_id: Some("instrument-down".to_string()),
            market_selection_timestamp_ms: Some(1000),
            selected_market_observed_timestamp_ms: Some(1000),
            polymarket_market_start_timestamp_ms: Some(1000),
            polymarket_market_end_timestamp_ms: Some(301000),
            price_to_beat_source: "source-one".to_string(),
            price_to_beat_value: "3100".to_string(),
            reference_quote_ts_event: 1200,
            spot_price: "3100.5".to_string(),
            fast_venue_available: true,
            reference_current_price: Some("3100.5".to_string()),
            reference_current_price_available: true,
            reference_current_price_source_id: Some("chainlink_primary".to_string()),
            reference_current_price_failed_over: Some(false),
            realized_volatility: "1.5".to_string(),
            realized_volatility_surface_id: String::new(),
            realized_volatility_as_of_ms: None,
            realized_volatility_gate_result: Some(BoltV3RvGateResult::MissingSnapshot),
            realized_volatility_receive_watermark_ms: None,
            realized_volatility_annualized_decimal: "1.5".to_string(),
            realized_volatility_measured_annualized_decimal: String::new(),
            realized_volatility_noise_robust_annualized_decimal: String::new(),
            realized_volatility_continuous_annualized_decimal: String::new(),
            realized_volatility_jump_annualized_decimal: String::new(),
            realized_volatility_forecast_annualized_decimal: String::new(),
            realized_volatility_pricing_component: String::new(),
            realized_volatility_seconds_per_annum: String::new(),
            realized_volatility_aggregation: String::new(),
            realized_volatility_sources_used: Vec::new(),
            realized_volatility_source_diagnostics: Vec::new(),
            realized_volatility_unknown_source_rejections: BTreeMap::new(),
            realized_volatility_blockers: Vec::new(),
            realized_volatility_config_fingerprint: String::new(),
            seconds_to_market_end: 300,
            pricing_kurtosis: "0".to_string(),
            theta_decay_factor: "0".to_string(),
            theta_scaled_min_edge_bps: "1".to_string(),
            fair_probability_up: "0.6".to_string(),
            uncertainty_band_probability: "0.01".to_string(),
            expected_edge_basis_points: "10".to_string(),
            worst_case_edge_basis_points: "10".to_string(),
            up_worst_case_edge_basis_points: Some("11".to_string()),
            down_worst_case_edge_basis_points: Some("9".to_string()),
            gate_blocked_by: Vec::new(),
            pricing_blocked_by: vec![BoltV3EntryPricingBlockReason::RealizedVolNotReady],
            fast_venue_name: Some("fast-source".to_string()),
            fast_venue_age_ms: Some(20),
            fast_venue_jitter_ms: Some(3),
            fast_venue_incoherent: false,
            lead_agreement_corr: Some("0.98".to_string()),
            fee_rate_basis_points: "0".to_string(),
            selected_side: Some("up".to_string()),
            submission_instrument_id: "instrument-up".to_string(),
            submission_order_side: "Buy".to_string(),
            submission_price: "0.50".to_string(),
            submission_quantity: "1".to_string(),
            client_order_id: "client-order-one".to_string(),
        };

        let line = encode_strategy_input_snapshot_line(&snapshot).expect("snapshot should encode");
        let decoded = parse_line(&line);

        assert_eq!(
            decoded["schema_version"],
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded["gate_id"], BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID);
        assert_eq!(
            decoded["gate_version"],
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION
        );
        assert_eq!(decoded["kind"], "strategy_input_snapshot");
        assert!(
            decoded["recorded_at_utc_ns"]
                .as_i64()
                .map(|ns| ns > 0)
                .unwrap_or(false),
            "recorded_at_utc_ns must be a positive i64; got {:?}",
            decoded["recorded_at_utc_ns"]
        );
        let snapshot_field = &decoded["snapshot"];
        assert_eq!(snapshot_field["strategy_id"], "strategy-one");
        assert_eq!(
            snapshot_field
                .as_object()
                .expect("snapshot should encode as an object")
                .len(),
            66
        );
        assert_eq!(
            snapshot_field["realized_volatility_gate_result"],
            "missing_snapshot"
        );
        assert_eq!(
            snapshot_field["realized_volatility_receive_watermark_ms"],
            serde_json::Value::Null
        );
        assert_eq!(snapshot_field["price_to_beat_source"], "source-one");
        assert_eq!(snapshot_field["up_worst_case_edge_basis_points"], "11");
        assert_eq!(snapshot_field["down_worst_case_edge_basis_points"], "9");
        assert_eq!(
            snapshot_field["pricing_blocked_by"],
            serde_json::json!(["realized_vol_not_ready"])
        );
        assert_eq!(snapshot_field["fast_venue_name"], "fast-source");
        assert_eq!(snapshot_field["fast_venue_available"], true);
        assert_eq!(snapshot_field["fast_venue_age_ms"], 20);
        assert_eq!(snapshot_field["fast_venue_jitter_ms"], 3);
        assert_eq!(snapshot_field["fast_venue_incoherent"], false);
        assert_eq!(snapshot_field["lead_agreement_corr"], "0.98");
        assert_eq!(
            snapshot_field["reference_current_price_source_id"],
            "chainlink_primary"
        );
        assert_eq!(snapshot_field["reference_current_price_available"], true);
        assert_eq!(snapshot_field["reference_current_price_failed_over"], false);
        assert_eq!(snapshot_field["reference_quote_ts_event"], 1200);
        assert_eq!(snapshot_field["client_order_id"], "client-order-one");
    }

    #[test]
    fn encode_admission_decision_line_wraps_decision_with_metadata() {
        for outcome in [
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedKillSwitchLatched,
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed,
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted,
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional,
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
            BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof,
            BoltV3AdmissionOutcome::RejectedCountCapExhausted,
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid,
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded,
            BoltV3AdmissionOutcome::RejectedCapitalAdmission,
        ] {
            let decision = BoltV3AdmissionDecisionEvidence {
                strategy_id: "strategy-one".to_string(),
                execution_client_id: "execution-client-one".to_string(),
                client_order_id: "client-order-one".to_string(),
                instrument_id: "instrument-one".to_string(),
                notional: "1.0".to_string(),
                intent_kind: BoltV3SubmitIntentKind::Entry,
                outcome: outcome.clone(),
                loss_halt_reasons: match &outcome {
                    BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
                        vec![BoltV3LossHaltReason::StaleLossSnapshot]
                    }
                    _ => Vec::new(),
                },
                snapshot_present: true,
                snapshot_observed_at_ns: Some(1_000),
                admission_now_ns: 1_200,
                snapshot_age_ns: Some(200),
                max_snapshot_age_ns: Some(1_000),
                snapshot_source: Some(BoltV3LossSnapshotSource::NtPortfolioSnapshot),
                per_trade_pnl_present: true,
                daily_pnl_present: true,
                rolling_pnl_present: true,
                current_equity_present: true,
                peak_equity_present: true,
                last_account_state_observed_at_ns: None,
                last_portfolio_snapshot_observed_at_ns: None,
                last_position_event_observed_at_ns: None,
                stale_reason: None,
                loss_snapshot_observed_at_ns: Some(1_000),
                loss_eval_now_ns: Some(1_200),
            };

            let line = encode_admission_decision_line(&decision).expect("decision should encode");
            let decoded = parse_line(&line);

            assert_eq!(
                decoded["schema_version"],
                BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
            );
            assert_eq!(decoded["gate_id"], BOLT_V3_SUBMIT_ADMISSION_GATE_ID);
            assert_eq!(
                decoded["gate_version"],
                BOLT_V3_DECISION_EVIDENCE_GATE_VERSION
            );
            assert_eq!(decoded["kind"], "admission_decision");
            assert!(
                decoded["recorded_at_utc_ns"]
                    .as_i64()
                    .map(|ns| ns > 0)
                    .unwrap_or(false),
                "recorded_at_utc_ns must be a positive i64; got {:?}",
                decoded["recorded_at_utc_ns"]
            );
            let decision_field = &decoded["decision"];
            assert_eq!(decision_field["strategy_id"], "strategy-one");
            assert_eq!(
                decision_field["execution_client_id"],
                "execution-client-one"
            );
            assert_eq!(decision_field["notional"], "1.0");
            assert_eq!(decision_field["intent_kind"], "entry");
            let expected_outcome = match &outcome {
                BoltV3AdmissionOutcome::Admitted => "admitted",
                BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                    "rejected_submit_lifecycle_disallowed"
                }
                BoltV3AdmissionOutcome::RejectedLossGovernorHalted => {
                    "rejected_loss_governor_halted"
                }
                BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                    "rejected_non_positive_notional"
                }
                BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                    "rejected_notional_cap_exceeded"
                }
                BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
                    "rejected_invalid_risk_reducing_exit_proof"
                }
                BoltV3AdmissionOutcome::RejectedKillSwitchLatched => "rejected_kill_switch_latched",
                BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
                    "rejected_kill_switch_forced_reduction_proof_invalid"
                }
                BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
                    "rejected_kill_switch_forced_reduction_cap_exceeded"
                }
                BoltV3AdmissionOutcome::RejectedCountCapExhausted => "rejected_count_cap_exhausted",
                BoltV3AdmissionOutcome::RejectedCapitalAdmission => "rejected_capital_admission",
            };
            assert_eq!(decision_field["outcome"], expected_outcome);
            if outcome == BoltV3AdmissionOutcome::RejectedLossGovernorHalted {
                assert_eq!(
                    decision_field["loss_halt_reasons"],
                    serde_json::json!(["stale_loss_snapshot"])
                );
            }
        }
    }

    #[test]
    fn gate_version_constant_matches_package_version() {
        assert_eq!(
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            env!("CARGO_PKG_VERSION")
        );
    }

    fn sample_exit_evaluation_evidence_encode() -> BoltV3ExitEvaluationEvidence {
        BoltV3ExitEvaluationEvidence {
            position_id: Some("position-one".to_string()),
            market_id: Some("market-one".to_string()),
            instrument_id: Some("instrument-up".to_string()),
            client_order_id: Some("client-order-one".to_string()),
            exit_eval_now_ms: 1_700_000_000_000,
            exit_trigger_source: BoltV3ExitTriggerSource::ReferenceUpdate,
            trigger_ts_event_ms: Some(1_699_999_999_500),
            trigger_ts_init_ms: Some(1_699_999_999_800),
            rv_surface_id: "surface-one".to_string(),
            rv_as_of_ms: Some(1_699_999_995_000),
            rv_ready: true,
            rv_snapshot_receive_watermark_ms: Some(1_699_999_999_750),
            rv_max_source_age_ms: Some(30_000),
            rv_blockers: vec!["source_stale".to_string()],
            rv_source_diagnostics: vec!["source-a:ready".to_string()],
            rv_gate_result: BoltV3RvGateResult::RejectedFutureDated,
            rv_as_of_minus_now_ms: Some(-5_000),
            spot_price: Some("3100.5".to_string()),
            spot_venue_name: Some("venue-one".to_string()),
            fast_venue_available: true,
            reference_current_price: Some("3099.75".to_string()),
            reference_current_price_available: true,
            interval_open: Some("3100".to_string()),
            fair_probability_up: Some("0.55".to_string()),
            fair_probability_down: Some("0.45".to_string()),
            uncertainty_band_probability: Some("0.02".to_string()),
            up_fee_bps: Some("1.25".to_string()),
            down_fee_bps: Some("2.5".to_string()),
            hold_ev_bps: Some("12.5".to_string()),
            exit_ev_bps: Some("-3.0".to_string()),
            exit_decision: BoltV3ExitDecisionOutcome::ExitFailClosed,
            forced_flat_reasons: vec!["rv_gate_rejected".to_string()],
            submission_order_side: Some("Sell".to_string()),
            submission_price: Some("0.49".to_string()),
            submission_quantity: Some("1".to_string()),
            submission_blocked_reason: Some("rv_gate_rejected".to_string()),
        }
    }

    fn sample_loss_governor_halt_evidence_encode() -> BoltV3LossGovernorHaltEvidence {
        BoltV3LossGovernorHaltEvidence {
            snapshot_present: true,
            snapshot_observed_at_ns: Some(1_700_000_000_000_000_000),
            admission_now_ns: 1_700_000_005_000_000_000,
            snapshot_age_ns: Some(5_000_000_000),
            max_snapshot_age_ns: 5_000_000_000,
            snapshot_source: Some("portfolio_snapshot".to_string()),
            has_per_trade_pnl: true,
            has_daily_pnl: true,
            has_rolling_pnl: false,
            has_current_equity: true,
            has_peak_equity: false,
            last_account_state_ts_ns: Some(1_699_999_999_000_000_000),
            last_portfolio_snapshot_ts_ns: Some(1_700_000_000_000_000_000),
            last_position_event_ts_ns: Some(1_699_999_998_000_000_000),
            account_state_count: 3,
            portfolio_snapshot_count: 1,
            position_event_count: 7,
            stale_reason: BoltV3StaleLossReason::AgeExceeded,
            stable_halt_key: "halt-key-one".to_string(),
            retry_count: 2,
            elapsed_since_first_halt_ns: 10_000_000_000,
        }
    }

    fn sample_order_reject_evidence_encode() -> BoltV3OrderRejectEvidence {
        BoltV3OrderRejectEvidence {
            reject_source: BoltV3RejectSource::Venue,
            reject_reason: BoltV3OrderRejectReason::MinNotionalRejected,
            admission_outcome: Some(BoltV3AdmissionOutcome::Admitted),
            raw_reason_text: Some("min notional not met".to_string()),
            instrument_id: "instrument-up".to_string(),
            order_side: Some("Buy".to_string()),
            raw_price: Some("0.50".to_string()),
            raw_quantity: Some("1".to_string()),
            raw_maker_amount: None,
            raw_taker_amount: None,
            normalized_price: Some("0.50".to_string()),
            normalized_quantity: Some("1".to_string()),
            normalized_maker_amount: None,
            normalized_taker_amount: None,
            venue_price_precision: Some(2),
            venue_size_precision: Some(0),
            venue_min_notional: Some("1.0".to_string()),
            prior_client_order_id: Some("client-order-zero".to_string()),
            client_order_id: "client-order-one".to_string(),
            retry_count: 1,
            backoff_cooldown_state: Some("cooling".to_string()),
            stable_episode_key: "episode-key-one".to_string(),
            elapsed_ns: 2_000_000_000,
        }
    }

    fn sample_venue_truth_capture_failure_evidence_encode() -> VenueTruthCaptureFailureEvidence {
        VenueTruthCaptureFailureEvidence {
            source: "polymarket_venue_truth_rest".to_string(),
            observed_at_ns: 1_700_000_000_000_000_000,
            endpoint: "clob_balance_allowance".to_string(),
            error_class: "transport_or_decode".to_string(),
            captures_missed: 2,
        }
    }

    fn sample_venue_truth_divergence_evidence_encode() -> VenueTruthDivergenceEvidence {
        VenueTruthDivergenceEvidence {
            source: "polymarket_venue_truth_rest".to_string(),
            observed_at_ns: 1_700_000_000_000_000_100,
            account_id: "POLYMARKET-001".to_string(),
            field: "collateral_balance".to_string(),
            venue_value: "48.40".to_string(),
            prior_accepted_value: "50.00".to_string(),
            missing_explanation: "unexplained_collateral_delta".to_string(),
            alarm_class: VenueTruthDivergenceAlarmClass::TrueDivergence,
        }
    }

    #[test]
    fn encode_exit_evaluation_line_round_trips_through_owned_line() {
        let evidence = sample_exit_evaluation_evidence_encode();

        let line = encode_exit_evaluation_line(&evidence).expect("evidence should encode");
        assert!(line.ends_with(b"\n"), "encoded line must end with newline");
        let decoded: ExitEvaluationLineOwned =
            serde_json::from_slice(&line[..line.len() - 1]).expect("line should decode");
        decoded
            .validate_header(
                BOLT_V3_EXIT_EVALUATION_RECORD_KIND,
                BOLT_V3_EXIT_EVALUATION_GATE_ID,
                0,
            )
            .expect("encoded exit-evaluation header should validate");

        assert_eq!(
            decoded.header.schema_version,
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded.header.gate_id, BOLT_V3_EXIT_EVALUATION_GATE_ID);
        assert_eq!(decoded.header.kind, BOLT_V3_EXIT_EVALUATION_RECORD_KIND);
        assert_eq!(decoded.evidence, evidence);
    }

    #[test]
    fn encode_loss_governor_halt_line_round_trips_through_owned_line() {
        let evidence = sample_loss_governor_halt_evidence_encode();

        let line = encode_loss_governor_halt_line(&evidence).expect("evidence should encode");
        assert!(line.ends_with(b"\n"), "encoded line must end with newline");
        let decoded: LossGovernorHaltLineOwned =
            serde_json::from_slice(&line[..line.len() - 1]).expect("line should decode");
        decoded
            .validate_header(
                BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
                BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
                0,
            )
            .expect("encoded loss-governor-halt header should validate");

        assert_eq!(
            decoded.header.schema_version,
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded.header.gate_id, BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID);
        assert_eq!(decoded.header.kind, BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND);
        assert_eq!(decoded.evidence, evidence);
    }

    #[test]
    fn encode_order_reject_line_round_trips_through_owned_line() {
        let evidence = sample_order_reject_evidence_encode();

        let line = encode_order_reject_line(&evidence).expect("evidence should encode");
        assert!(line.ends_with(b"\n"), "encoded line must end with newline");
        let decoded: OrderRejectLineOwned =
            serde_json::from_slice(&line[..line.len() - 1]).expect("line should decode");
        decoded
            .validate_header(
                BOLT_V3_ORDER_REJECT_RECORD_KIND,
                BOLT_V3_ORDER_REJECT_GATE_ID,
                0,
            )
            .expect("encoded order-reject header should validate");

        assert_eq!(
            decoded.header.schema_version,
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded.header.gate_id, BOLT_V3_ORDER_REJECT_GATE_ID);
        assert_eq!(decoded.header.kind, BOLT_V3_ORDER_REJECT_RECORD_KIND);
        assert_eq!(decoded.evidence, evidence);
    }

    #[test]
    fn encode_venue_truth_capture_failure_line_round_trips_through_owned_line() {
        let evidence = sample_venue_truth_capture_failure_evidence_encode();

        let line =
            encode_venue_truth_capture_failure_line(&evidence).expect("evidence should encode");
        assert!(line.ends_with(b"\n"), "encoded line must end with newline");
        let decoded: VenueTruthCaptureFailureLineOwned =
            serde_json::from_slice(&line[..line.len() - 1]).expect("line should decode");
        decoded
            .validate_header(
                BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND,
                BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID,
                0,
            )
            .expect("encoded venue-truth capture-failure header should validate");

        assert_eq!(
            decoded.header.schema_version,
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(
            decoded.header.gate_id,
            BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_GATE_ID
        );
        assert_eq!(
            decoded.header.kind,
            BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND
        );
        assert_eq!(decoded.capture_failure, evidence);
    }

    #[test]
    fn encode_venue_truth_divergence_line_round_trips_through_owned_line() {
        let evidence = sample_venue_truth_divergence_evidence_encode();

        let line = encode_venue_truth_divergence_line(&evidence).expect("evidence should encode");
        assert!(line.ends_with(b"\n"), "encoded line must end with newline");
        let encoded: serde_json::Value =
            serde_json::from_slice(&line[..line.len() - 1]).expect("line should decode as JSON");
        assert_eq!(
            encoded["divergence"]["alarm_class"],
            serde_json::Value::String("true_divergence".to_string())
        );
        let decoded: VenueTruthDivergenceLineOwned =
            serde_json::from_slice(&line[..line.len() - 1]).expect("line should decode");
        decoded
            .validate_header(
                BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND,
                BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID,
                0,
            )
            .expect("encoded venue-truth divergence header should validate");

        assert_eq!(
            decoded.header.schema_version,
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(
            decoded.header.gate_id,
            BOLT_V3_VENUE_TRUTH_DIVERGENCE_GATE_ID
        );
        assert_eq!(
            decoded.header.kind,
            BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND
        );
        assert_eq!(decoded.divergence, evidence);
    }

    #[test]
    fn production_novelty_storage_has_fixed_large_sequence_and_failure_bound() {
        const CHILD_ENV: &str = "BOLT_V3_EVIDENCE_NOVELTY_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let output = std::process::Command::new(
                std::env::current_exe().expect("current test binary should be available"),
            )
            .arg("production_novelty_storage_has_fixed_large_sequence_and_failure_bound")
            .arg("--test-threads=1")
            .env(CHILD_ENV, "1")
            .output()
            .expect("fresh novelty child should launch");
            assert!(
                output.status.success(),
                "fresh novelty child failed\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            return;
        }

        let rv = [
            BoltV3RvGateResult::Accepted,
            BoltV3RvGateResult::MissingSnapshot,
            BoltV3RvGateResult::MissingEvaluationEventTime,
            BoltV3RvGateResult::RejectedFutureDated,
            BoltV3RvGateResult::RejectedStale,
            BoltV3RvGateResult::RejectedNotReady,
        ];
        let entry = [
            BoltV3EntrySkipReasonCategory::StrategyCoreNotRegistered,
            BoltV3EntrySkipReasonCategory::EntryGateBlocked,
            BoltV3EntrySkipReasonCategory::EntryPricingBlocked,
            BoltV3EntrySkipReasonCategory::NoSideSelected,
            BoltV3EntrySkipReasonCategory::SizedNotionalNotPositive,
            BoltV3EntrySkipReasonCategory::InstrumentIdMissing,
            BoltV3EntrySkipReasonCategory::InstrumentMissingFromCache,
            BoltV3EntrySkipReasonCategory::EntryPriceMissing,
            BoltV3EntrySkipReasonCategory::QuantityRoundingFailed,
            BoltV3EntrySkipReasonCategory::LimitNotionalExceedsSizedNotional,
            BoltV3EntrySkipReasonCategory::QuantityNotPositive,
            BoltV3EntrySkipReasonCategory::PositionContractInvalid,
            BoltV3EntrySkipReasonCategory::EntryPositionContractUnsupported,
            BoltV3EntrySkipReasonCategory::HistoricalEntryFeeUnavailable,
            BoltV3EntrySkipReasonCategory::OnePositionInvariantViolation,
            BoltV3EntrySkipReasonCategory::Unclassified,
        ];
        let basket = [
            BoltV3BasketAdmissionOutcome::Admitted,
            BoltV3BasketAdmissionOutcome::RejectedBasketNotionalCapExceeded,
            BoltV3BasketAdmissionOutcome::RejectedMaxOpenBasketCapExceeded,
            BoltV3BasketAdmissionOutcome::RejectedStaleScannerEvidence,
            BoltV3BasketAdmissionOutcome::RejectedStaleSubmitRecheck,
            BoltV3BasketAdmissionOutcome::RejectedNonPositiveCandidateCost,
            BoltV3BasketAdmissionOutcome::RejectedNonPositiveEdge,
            BoltV3BasketAdmissionOutcome::RejectedEdgeThreshold,
            BoltV3BasketAdmissionOutcome::RejectedMissingGroupingProof,
            BoltV3BasketAdmissionOutcome::RejectedMissingSettlementRules,
            BoltV3BasketAdmissionOutcome::RejectedRetryBudgetExceeded,
            BoltV3BasketAdmissionOutcome::RejectedSubmitSlots,
        ];
        let exits = [
            BoltV3ExitDecisionOutcome::Exit,
            BoltV3ExitDecisionOutcome::ExitFailClosed,
            BoltV3ExitDecisionOutcome::Hold,
            BoltV3ExitDecisionOutcome::Blocked,
        ];
        let stale = [
            BoltV3StaleLossReason::MissingSnapshot,
            BoltV3StaleLossReason::SourceEmpty,
            BoltV3StaleLossReason::FutureDated,
            BoltV3StaleLossReason::AgeExceeded,
            BoltV3StaleLossReason::MissingRequiredField,
        ];
        let sources = [
            BoltV3RejectSource::SubmitAdmission,
            BoltV3RejectSource::Venue,
            BoltV3RejectSource::NtExecution,
            BoltV3RejectSource::Internal,
        ];
        let reasons = [
            BoltV3OrderRejectReason::AdmissionRejected,
            BoltV3OrderRejectReason::PrecisionRejected,
            BoltV3OrderRejectReason::MinSizeRejected,
            BoltV3OrderRejectReason::MinNotionalRejected,
            BoltV3OrderRejectReason::InsufficientBalance,
            BoltV3OrderRejectReason::DuplicateClientOrderId,
            BoltV3OrderRejectReason::Other,
        ];

        let mut counts = [0_u32; 10];
        for index in 0..100_000_usize {
            counts[0] += u32::from(claim_blocked_rv(
                rv[index % rv.len()],
                (index / rv.len()) % 2 == 0,
            ));
            counts[1] += u32::from(claim_entry_skip(entry[index % entry.len()]));
            counts[2] += u32::from(claim_requote_throttle(
                ((index / 6) % 2) as u32,
                (index % 6) as u32,
            ));
            counts[3] += u32::from(claim_basket_admission(&basket[index % basket.len()]));
            counts[4] += u32::from(claim_exit_decision(exits[index % exits.len()]));
            counts[5] += u32::from(claim_exit_evaluation(
                exits[index % exits.len()],
                rv[(index / exits.len()) % rv.len()],
            ));
            counts[6] += u32::from(claim_loss_halt(stale[index % stale.len()]));
            counts[7] += u32::from(claim_order_reject(
                sources[index % sources.len()],
                reasons[(index / sources.len()) % reasons.len()],
            ));
            counts[8] += u32::from(claim_venue_truth_capture_failure());
            counts[9] += u32::from(claim_venue_truth_divergence(match index % 3 {
                0 => VenueTruthDivergenceAlarmClass::TrueDivergence,
                1 => VenueTruthDivergenceAlarmClass::OrderingViolation,
                _ => VenueTruthDivergenceAlarmClass::SilentChannel,
            }));
        }
        assert_eq!(counts, [12, 16, 12, 12, 4, 24, 5, 28, 1, 3]);
        assert_eq!(
            counts.into_iter().sum::<u32>(),
            BOLT_V3_NON_RECOVERY_MAX_EMISSIONS
        );
    }
}
