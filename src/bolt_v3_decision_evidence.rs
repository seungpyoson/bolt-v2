use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};
use nautilus_model::orders::{Order, OrderAny};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_capital_reservation::ReservationRejectionReason;
use crate::bolt_v3_config::LoadedBoltV3Config;
use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;
use crate::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
    RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceDiagnostic,
    RealizedVolSourceRejectReason, RealizedVolSourceStatus,
};

pub const BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION: u32 = 12;
pub const BOLT_V3_DECISION_EVIDENCE_GATE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BOLT_V3_ORDER_INTENT_GATE_ID: &str = "bolt_v3.order_intent";
pub const BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID: &str = "bolt_v3.position_sizer_rebuild";
pub const BOLT_V3_SUBMIT_ADMISSION_GATE_ID: &str = "bolt_v3.submit_admission";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID: &str = "bolt_v3.strategy_input_snapshot";
pub const BOLT_V3_ENTRY_SKIP_GATE_ID: &str = "bolt_v3.entry_skip";
pub const BOLT_V3_EXIT_DECISION_GATE_ID: &str = "bolt_v3.exit_decision";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID: &str = "bolt_v3.loss_governor_halt";
pub const BOLT_V3_REQUOTE_THROTTLE_GATE_ID: &str = "bolt_v3.requote_throttle";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND: &str = "strategy_input_snapshot";
pub const BOLT_V3_ORDER_INTENT_RECORD_KIND: &str = "order_intent";
pub const BOLT_V3_ADMISSION_DECISION_RECORD_KIND: &str = "admission_decision";
pub const BOLT_V3_ENTRY_SKIP_RECORD_KIND: &str = "entry_skip";
pub const BOLT_V3_EXIT_DECISION_RECORD_KIND: &str = "exit_decision";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND: &str = "loss_governor_halt";
pub const BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND: &str = "requote_throttle";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_SUBSYSTEM: &str = "loss_governor";
const BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND: &str = "basket_admission_decision";
const BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND: &str = "position_sizer_rebuild";
const BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND: &str = "submit_reservation_metadata";
const BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND: &str = "submit_reservation_fill";
const PRE_POSITION_SIZER_RECOVERY_SCHEMA_VERSION: u32 = 9;
const SUBMIT_RESERVATION_METADATA_PRODUCT_KIND_BINARY: &str = "prediction_market_binary";
const SUBMIT_RESERVATION_METADATA_SIDE_BUY: &str = "buy";
const SUBMIT_RESERVATION_METADATA_SIDE_SELL: &str = "sell";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT: &str = "current";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_NEXT: &str = "next";

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
    fn record_position_sizer_rebuild_audit(
        &self,
        audit: &BoltV3PositionSizerRebuildAuditEvidence,
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
    fn record_loss_governor_halt(&self, halt: &BoltV3LossGovernorHaltEvidence) -> Result<()>;
    fn record_requote_throttle(&self, throttle: &BoltV3RequoteThrottleEvidence) -> Result<()>;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderIntentEvidence {
    pub strategy_id: String,
    pub intent_kind: BoltV3OrderIntentKind,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
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

fn number_evidence(value: f64) -> String {
    value.to_string()
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub realized_vol: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitDecisionOutcome {
    Exit,
    ExitFailClosed,
    Hold,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitBlockedReason {
    NoOpenPosition,
    ExitAlreadyPending,
    EntryOrderStillWorking,
    ExitDecisionUnavailable,
    ExitHold,
    OpenPositionMissing,
    ExitOrderConfigInvalid,
    ExitQuoteQuantityUnsupported,
    ExitPriceMissing,
    ExitQuantityNotPositive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3ExitDecisionEvidence {
    pub strategy_id: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub position_instrument_id: Option<String>,
    pub position_outcome_side: Option<BoltV3OutcomeSide>,
    pub forced_flat_reasons: Vec<BoltV3ForcedFlatReason>,
    pub hold_ev_bps: Option<String>,
    pub exit_ev_bps: Option<String>,
    pub realized_vol: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    #[serde(default)]
    pub realized_volatility_source_diagnostics:
        Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub exit_hysteresis_bps: String,
    pub exit_decision: BoltV3ExitDecisionOutcome,
    pub blocked_reason: Option<BoltV3ExitBlockedReason>,
    pub client_order_id: Option<String>,
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
pub enum BoltV3TradingState {
    Active,
    Halted,
    Reducing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3LossGovernorHaltEvidence {
    pub observed_at_ns: u64,
    pub source: String,
    pub halt_reasons: Vec<BoltV3LossHaltReason>,
    pub per_trade_pnl: Option<String>,
    pub daily_pnl: Option<String>,
    pub rolling_pnl: Option<String>,
    pub current_equity: Option<String>,
    pub peak_equity: Option<String>,
    pub max_per_trade_loss: Option<String>,
    pub max_daily_loss: Option<String>,
    pub max_rolling_loss: Option<String>,
    pub max_drawdown: Option<String>,
    pub max_snapshot_age_ns: u64,
    pub previous_trading_state: BoltV3TradingState,
    pub target_trading_state: BoltV3TradingState,
    pub subsystem: String,
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
    pub reference_current_price: Option<String>,
    pub reference_current_price_source_id: Option<String>,
    pub reference_current_price_failed_over: Option<bool>,
    pub realized_volatility: String,
    pub realized_volatility_surface_id: String,
    pub realized_volatility_as_of_ms: Option<u64>,
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
    RejectedPositionSizing,
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
    pub loss_snapshot_observed_at_ns: Option<u64>,
    pub loss_eval_now_ns: Option<u64>,
    pub max_snapshot_age_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3PositionSizerRebuildAuditEvidence {
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

#[derive(Debug)]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    file: Mutex<std::fs::File>,
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
        })
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
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
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
        let line = encode_basket_admission_decision_line(decision)?;
        self.append_line(&line)
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> Result<()> {
        let line = encode_position_sizer_rebuild_audit_line(audit)?;
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
        let line = encode_entry_skip_line(skip)?;
        self.append_line(&line)
    }

    fn record_exit_decision(&self, decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        let line = encode_exit_decision_line(decision)?;
        self.append_line(&line)
    }

    fn record_loss_governor_halt(&self, halt: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        let line = encode_loss_governor_halt_line(halt)?;
        self.append_line(&line)
    }

    fn record_requote_throttle(&self, throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        let line = encode_requote_throttle_line(throttle)?;
        self.append_line(&line)
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
        if header.schema_version < BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION {
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
            "position_sizer_rebuild" => {
                header.validate(
                    BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND,
                    BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID,
                    index,
                )?;
                let decoded: PositionSizerRebuildAuditLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 position sizer rebuild audit line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND,
                    BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID,
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
        if is_pre_position_sizer_recovery_non_recovery_record(&header) {
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
            "position_sizer_rebuild" => {
                header.validate(
                    BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND,
                    BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID,
                    index,
                )?;
                let decoded: PositionSizerRebuildAuditLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 position sizer rebuild audit line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND,
                    BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID,
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

fn is_pre_position_sizer_recovery_non_recovery_record(
    header: &DecisionEvidenceEnvelopeHeader,
) -> bool {
    header.schema_version == PRE_POSITION_SIZER_RECOVERY_SCHEMA_VERSION
        && matches!(
            header.kind.as_str(),
            BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND
                | BOLT_V3_ORDER_INTENT_RECORD_KIND
                | BOLT_V3_ADMISSION_DECISION_RECORD_KIND
                | BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND
        )
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
struct PositionSizerRebuildAuditLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    audit: BoltV3PositionSizerRebuildAuditEvidence,
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
struct LossGovernorHaltLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    loss_governor_halt: BoltV3LossGovernorHaltEvidence,
}

#[derive(Deserialize)]
struct RequoteThrottleLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    requote_throttle: BoltV3RequoteThrottleEvidence,
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

impl PositionSizerRebuildAuditLineOwned {
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

impl LossGovernorHaltLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        let _ = &self.loss_governor_halt;
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
struct PositionSizerRebuildAuditLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    audit: &'a BoltV3PositionSizerRebuildAuditEvidence,
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
struct LossGovernorHaltLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    loss_governor_halt: &'a BoltV3LossGovernorHaltEvidence,
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

fn encode_position_sizer_rebuild_audit_line(
    audit: &BoltV3PositionSizerRebuildAuditEvidence,
) -> Result<Vec<u8>> {
    let envelope = PositionSizerRebuildAuditLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_POSITION_SIZER_REBUILD_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_POSITION_SIZER_REBUILD_RECORD_KIND,
        audit,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize position sizer rebuild audit evidence")?;
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

fn encode_loss_governor_halt_line(halt: &BoltV3LossGovernorHaltEvidence) -> Result<Vec<u8>> {
    let envelope = LossGovernorHaltLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_LOSS_GOVERNOR_HALT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND,
        loss_governor_halt: halt,
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
    fn encode_order_intent_line_wraps_intent_with_metadata() {
        let intent = BoltV3OrderIntentEvidence {
            strategy_id: "strategy-one".to_string(),
            intent_kind: BoltV3OrderIntentKind::Entry,
            instrument_id: "instrument-one".to_string(),
            client_order_id: "client-order-one".to_string(),
            order_side: OrderSide::Buy.to_string(),
            price: "0.42".to_string(),
            quantity: "1".to_string(),
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
            reference_current_price: Some("3100.5".to_string()),
            reference_current_price_source_id: Some("chainlink_primary".to_string()),
            reference_current_price_failed_over: Some(false),
            realized_volatility: "1.5".to_string(),
            realized_volatility_surface_id: String::new(),
            realized_volatility_as_of_ms: None,
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
            62
        );
        assert_eq!(snapshot_field["price_to_beat_source"], "source-one");
        assert_eq!(snapshot_field["up_worst_case_edge_basis_points"], "11");
        assert_eq!(snapshot_field["down_worst_case_edge_basis_points"], "9");
        assert_eq!(
            snapshot_field["pricing_blocked_by"],
            serde_json::json!(["realized_vol_not_ready"])
        );
        assert_eq!(snapshot_field["fast_venue_name"], "fast-source");
        assert_eq!(snapshot_field["fast_venue_age_ms"], 20);
        assert_eq!(snapshot_field["fast_venue_jitter_ms"], 3);
        assert_eq!(snapshot_field["fast_venue_incoherent"], false);
        assert_eq!(snapshot_field["lead_agreement_corr"], "0.98");
        assert_eq!(
            snapshot_field["reference_current_price_source_id"],
            "chainlink_primary"
        );
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
            BoltV3AdmissionOutcome::RejectedPositionSizing,
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
                        vec!["stale_loss_snapshot".to_string()]
                    }
                    _ => Vec::new(),
                },
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
                BoltV3AdmissionOutcome::RejectedPositionSizing => "rejected_position_sizing",
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
}
