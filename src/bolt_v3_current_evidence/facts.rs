use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReservationMetadataFact {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReservationFillFact {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderIntentClampNotEvaluatedReason {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderIntentClampOutcome {
    WithinBounds,
    Clamped {
        original_quantity: String,
    },
    Rejected,
    NotEvaluated {
        reason: OrderIntentClampNotEvaluatedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentOrderFields {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentDetails {
    pub strategy_id: String,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
    pub clamp_outcome: Option<OrderIntentClampOutcome>,
    pub order_fields: OrderIntentOrderFields,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryOrderIntentFact {
    pub details: OrderIntentDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReducingExitOrderIntentFact {
    pub details: OrderIntentDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionDetails {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub basket_id: String,
    pub group_id: String,
    pub leg_instrument_ids: Vec<String>,
    pub total_notional: String,
    pub leg_order_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionGrantedFact {
    pub details: BasketAdmissionDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasketAdmissionRejectionReason {
    BasketNotionalCapExceeded,
    MaxOpenBasketCapExceeded,
    StaleScannerEvidence,
    StaleSubmitRecheck,
    NonPositiveCandidateCost,
    NonPositiveEdge,
    EdgeThreshold,
    MissingGroupingProof,
    MissingSettlementRules,
    RetryBudgetExceeded,
    SubmitSlots,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionRejectedFact {
    pub details: BasketAdmissionDetails,
    pub reason: BasketAdmissionRejectionReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionRejectionReason {
    MissingEvidence,
    StaleRequest,
    PoolMismatch,
    OverBudget,
    InvalidRequest,
    CollateralGroupMismatch,
    DuplicateReservation,
    UnknownReservation,
    UnknownRelease,
    ReconciliationRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapitalAdmissionRebuildOutcome {
    Accepted,
    Rejected(CapitalAdmissionRejectionReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapitalAdmissionRebuildFact {
    pub observed_at_ns: u64,
    pub source: String,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub outcome: CapitalAdmissionRebuildOutcome,
    pub attempted_reservation_count: usize,
    pub recovered_reservation_count: usize,
    pub live_reserved_liability: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequoteActionCostClass {
    FreshSubmit,
    CancelResubmit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequoteThrottleBound {
    SubmitCommandWindow,
    RestCallWindow,
    MinInterval,
    WindowCap,
    OutOfOrderTs,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequoteThrottleObservationFact {
    pub strategy_id: String,
    pub family_key: String,
    pub market_id: Option<String>,
    pub leg: String,
    pub now_ms: u64,
    pub observed_at_ns: u64,
    pub action_cost_class: RequoteActionCostClass,
    pub bound_by: RequoteThrottleBound,
    pub submit_commands_in_window: usize,
    pub submit_command_cap: u64,
    pub submit_window_ms: u64,
    pub rest_cost_in_window: u64,
    pub rest_cap_per_minute: u64,
    pub rest_window_ms: u64,
    pub min_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthCaptureFailureFact {
    pub source: String,
    pub observed_at_ns: u64,
    pub endpoint: String,
    pub error_class: String,
    pub captures_missed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthDivergenceAlarmClass {
    TrueDivergence,
    OrderingViolation,
    SilentChannel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthDivergenceFact {
    pub source: String,
    pub observed_at_ns: u64,
    pub account_id: String,
    pub field: String,
    pub venue_value: String,
    pub prior_accepted_value: String,
    pub missing_explanation: String,
    pub alarm_class: VenueTruthDivergenceAlarmClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleLossReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossGovernorHaltFact {
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
    pub stale_reason: StaleLossReason,
    pub stable_halt_key: String,
    pub retry_count: u32,
    pub elapsed_since_first_halt_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossHaltReason {
    PerTradeLossLimit,
    DailyLossLimit,
    RollingLossLimit,
    MaxDrawdownLimit,
    StaleLossSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossSnapshotSource {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossSnapshotStaleReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRejectionReason {
    KillSwitchLatched,
    SubmitLifecycleDisallowed,
    LossGovernorHalted,
    NonPositiveNotional,
    NotionalCapExceeded,
    InvalidRiskReducingExitProof,
    CountCapExhausted,
    KillSwitchForcedReductionProofInvalid,
    KillSwitchForcedReductionCapExceeded,
    CapitalAdmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionDecisionOutcome {
    Admitted,
    Rejected(AdmissionRejectionReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionDetails {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: String,
    pub loss_halt_reasons: Vec<LossHaltReason>,
    pub snapshot_present: bool,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: u64,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: Option<u64>,
    pub snapshot_source: Option<LossSnapshotSource>,
    pub per_trade_pnl_present: bool,
    pub daily_pnl_present: bool,
    pub rolling_pnl_present: bool,
    pub current_equity_present: bool,
    pub peak_equity_present: bool,
    pub last_account_state_observed_at_ns: Option<u64>,
    pub last_portfolio_snapshot_observed_at_ns: Option<u64>,
    pub last_position_event_observed_at_ns: Option<u64>,
    pub stale_reason: Option<LossSnapshotStaleReason>,
    pub loss_snapshot_observed_at_ns: Option<u64>,
    pub loss_eval_now_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedEntryAdmissionFact {
    pub details: AdmissionDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedEntryAdmissionFact {
    pub details: AdmissionDetails,
    pub reason: AdmissionRejectionReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskReducingExitAdmissionFact {
    pub details: AdmissionDetails,
    pub outcome: AdmissionDecisionOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForcedReductionAdmissionFact {
    pub details: AdmissionDetails,
    pub outcome: AdmissionDecisionOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderRejectSource {
    SubmitAdmission,
    Venue,
    NtExecution,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderRejectReason {
    AdmissionRejected,
    PrecisionRejected,
    MinSizeRejected,
    MinNotionalRejected,
    InsufficientBalance,
    DuplicateClientOrderId,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderRejectFact {
    pub reject_source: OrderRejectSource,
    pub reject_reason: OrderRejectReason,
    pub admission_outcome: Option<AdmissionDecisionOutcome>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntrySkipReason {
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
    EntryQuoteNotionalBelowVenueMinimum,
    EntryQuoteNotionalMinimumUnmodeled,
    QuantityNotPositive,
    PositionContractInvalid,
    EntryPositionContractUnsupported,
    HistoricalEntryFeeUnavailable,
    OnePositionInvariantViolation,
    EntryMalformedRejected,
    EntryBalanceRejected,
    EntryUnfillableRejectedUnchangedBook,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForcedFlatReason {
    Freeze,
    StaleReference,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureOccupancy {
    PendingEntry,
    EntryReconcilePending,
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryBlockReason {
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
    ForcedFlat(ForcedFlatReason),
    OnePositionInvariant(ExposureOccupancy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOutcomeEdgeBlockReason {
    MissingOrderBook,
    InsufficientDepth,
    InvalidProbability,
    InvalidCost,
    UnsupportedOrderShape,
    EdgeBelowThreshold,
    SpreadOrSlippageWipedEdge,
    FeeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryPricingBlockReason {
    SpotPriceMissing,
    ReferenceCurrentPriceStale,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    ThetaScalerUnavailable,
    UncertaintyBandUnavailable,
    FairProbabilityUnavailable,
    FeeUnavailable(OutcomeSide),
    ExecutableEntryCostUnavailable(OutcomeSide),
    ExecutableEdgeUnavailable(OutcomeSide, BinaryOutcomeEdgeBlockReason),
    SizedNotionalUnsupported(OutcomeSide),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RvGateResult {
    Accepted,
    MissingSnapshot,
    MissingEvaluationEventTime,
    RejectedFutureDated,
    RejectedStale,
    RejectedNotReady,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolPricingComponent {
    Measured,
    NoiseRobust,
    Continuous,
    Forecast,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolAggregation {
    UpperQuantile,
    Median,
    TrimmedMean,
    MedianWithUpperQuantileGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolSourceClass {
    SpotQuote,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolSampleKind {
    Midpoint,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolSourceStatus {
    Ready,
    Blocked,
    DiagnosticOnly,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RealizedVolSourceRejectReason {
    DisabledSource,
    InvalidPrice,
    SourceClassMismatch,
    SampleKindMismatch,
    EventTimeRegression,
    DuplicateTimestamp,
    StaleSameEventUpdate,
    ReceiveBeforeEvent,
    EventReceiveLagExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealizedVolBlockReason {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealizedVolatilitySourceDiagnosticFact {
    pub source_id: String,
    pub source_class: RealizedVolSourceClass,
    pub sample_kind: RealizedVolSampleKind,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub status: RealizedVolSourceStatus,
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
    pub last_rejected_reason: Option<RealizedVolSourceRejectReason>,
    pub last_rejected_event_ts_ms: Option<u64>,
    pub last_rejected_recv_ts_ms: Option<u64>,
    pub rejection_counters: BTreeMap<RealizedVolSourceRejectReason, u64>,
    pub block_reason: Option<RealizedVolBlockReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRealizedVolatilitySnapshotFact {
    pub surface_id: String,
    pub as_of_ms: Option<u64>,
    pub annualized_decimal: String,
    pub measured_annualized_decimal: String,
    pub noise_robust_annualized_decimal: String,
    pub continuous_annualized_decimal: String,
    pub jump_annualized_decimal: String,
    pub forecast_annualized_decimal: String,
    pub pricing_component: RealizedVolPricingComponent,
    pub seconds_per_annum: String,
    pub aggregation: RealizedVolAggregation,
    pub sources_used: Vec<String>,
    pub source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticFact>,
    pub unknown_source_rejections: BTreeMap<String, u64>,
    pub blockers: Vec<RealizedVolBlockReason>,
    pub config_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySkipFact {
    pub strategy_id: String,
    pub now_ms: u64,
    pub reason_category: EntrySkipReason,
    pub gate_blocked_by: Vec<EntryBlockReason>,
    pub pricing_blocked_by: Vec<EntryPricingBlockReason>,
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
    pub realized_vol_gate_result: Option<RvGateResult>,
    pub realized_vol_receive_watermark_ms: Option<u64>,
    pub realized_vol_snapshot: Option<EntryRealizedVolatilitySnapshotFact>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub selected_side: Option<OutcomeSide>,
    pub sized_notional: Option<String>,
    pub sized_worst_case_ev_bps: Option<String>,
    pub sized_edge_cents_per_share: Option<String>,
    pub theta_scaled_min_edge_bps: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub submission_blocked_reason: Option<EntrySkipReason>,
    pub stale_reference_after_ms: Option<u64>,
    pub last_reference_ts_ms: Option<u64>,
    pub min_liquidity_required: Option<String>,
    pub liquidity_available: Option<String>,
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub fast_venue_incoherent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyInputRvState {
    Absent {
        gate_result: RvGateResult,
    },
    Present {
        selected_annualized_decimal: String,
        gate_result: RvGateResult,
        receive_watermark_ms: Option<u64>,
        snapshot: EntryRealizedVolatilitySnapshotFact,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyInputDetails {
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
    pub realized_volatility: StrategyInputRvState,
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
    pub gate_blocked_by: Vec<EntryBlockReason>,
    pub pricing_blocked_by: Vec<EntryPricingBlockReason>,
    pub fast_venue_name: Option<String>,
    pub fast_venue_age_ms: Option<u64>,
    pub fast_venue_jitter_ms: Option<u64>,
    pub fast_venue_incoherent: bool,
    pub lead_agreement_corr: Option<String>,
    pub fee_rate_basis_points: String,
    pub selected_side: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedStrategyInputObservationFact {
    pub details: StrategyInputDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmissionLinkage {
    pub instrument_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
    pub client_order_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitLinkedStrategyInputSnapshotFact {
    pub details: StrategyInputDetails,
    pub submission: SubmissionLinkage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitTriggerSource {
    SignalQuote,
    ReferenceUpdate,
    SelectionUpdate,
    BookDelta,
    Unknown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitBlockedReason {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitDecisionDetails {
    pub strategy_id: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub position_instrument_id: Option<String>,
    pub position_outcome_side: Option<OutcomeSide>,
    pub forced_flat_reasons: Vec<ForcedFlatReason>,
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
    pub exit_trigger_source: ExitTriggerSource,
    pub trigger_ts_event_ms: u64,
    pub trigger_ts_init_ms: Option<u64>,
    pub rv_surface_id: String,
    pub rv_snapshot_as_of_ms: Option<u64>,
    pub rv_snapshot_ready: bool,
    pub rv_snapshot_has_ready_realized_vol: Option<bool>,
    pub rv_snapshot_receive_watermark_ms: Option<u64>,
    pub rv_max_source_age_ms: Option<u64>,
    pub rv_snapshot_blockers: Vec<RealizedVolBlockReason>,
    pub rv_source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticFact>,
    pub rv_gate_result: RvGateResult,
    pub rv_future_dating_delta_ms: Option<u64>,
    pub exit_hysteresis_bps: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitSubmissionOutcome {
    Exit,
    ExitFailClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitSubmissionDecisionFact {
    pub details: ExitDecisionDetails,
    pub outcome: ExitSubmissionOutcome,
    pub submission: SubmissionLinkage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitHoldOutcome {
    Hold,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitHoldDecisionFact {
    pub details: ExitDecisionDetails,
    pub outcome: ExitHoldOutcome,
    pub blocked_reason: Option<ExitBlockedReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitEvaluationDecision {
    Submission {
        outcome: ExitSubmissionOutcome,
        submission: SubmissionLinkage,
    },
    Hold {
        outcome: ExitHoldOutcome,
        blocked_reason: Option<ExitBlockedReason>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitEvaluationFact {
    pub position_id: Option<String>,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub client_order_id: Option<String>,
    pub exit_eval_now_ms: i64,
    pub exit_trigger_source: ExitTriggerSource,
    pub trigger_ts_event_ms: Option<i64>,
    pub trigger_ts_init_ms: Option<i64>,
    pub rv_surface_id: String,
    pub rv_as_of_ms: Option<i64>,
    pub rv_ready: bool,
    pub rv_snapshot_receive_watermark_ms: Option<i64>,
    pub rv_max_source_age_ms: Option<u64>,
    pub rv_blockers: Vec<String>,
    pub rv_source_diagnostics: Vec<String>,
    pub rv_gate_result: RvGateResult,
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
    pub decision: ExitEvaluationDecision,
    pub forced_flat_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeSide {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFact {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: String,
    pub position_id: String,
    pub instrument_id: String,
    pub product_id: String,
    pub outcome_side: OutcomeSide,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementBookingErrorReason {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementBookingErrorFact {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub instrument_id: Option<String>,
    pub resolution_instrument_id: Option<String>,
    pub reason: SettlementBookingErrorReason,
    pub detail: String,
    pub observed_at_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderLifecycleTransition {
    BoundaryReclassification,
    EntryFillMaterialized,
    EntryReconcilePending,
    PositionTruthRematerialized,
    PositionClosed,
    ResidualRemanaged,
    RestartOpenOrderAdopted,
    RestartOpenOrderRecoveryBlocked,
    SettlementEvidenceRecoveryBlocked,
    SettlementBookingTerminal,
    OrderDenied,
    OrderRejected,
    OrderCanceled,
    OrderExpired,
    OrderFilled,
    ReconcileQueryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderLifecycleOutcome {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLifecycleFact {
    pub strategy_id: String,
    pub transition: OrderLifecycleTransition,
    pub outcome: OrderLifecycleOutcome,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSettlementFact {
    pub settlement_key: String,
    pub booking_error: Option<SettlementBookingErrorFact>,
    pub lifecycle: OrderLifecycleFact,
}

#[derive(Debug, Default)]
pub struct StartupRecoveryFacts {
    reservation_metadata: BTreeMap<String, SubmitReservationMetadataFact>,
    reservation_fill_trade_ids: BTreeMap<(String, String), BTreeSet<String>>,
    settlements: BTreeMap<String, SettlementFact>,
    booking_error_keys: BTreeSet<String>,
    terminal_settlement_keys: BTreeSet<String>,
}

impl StartupRecoveryFacts {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.reservation_metadata.is_empty()
            && self.reservation_fill_trade_ids.is_empty()
            && self.settlements.is_empty()
            && self.booking_error_keys.is_empty()
            && self.terminal_settlement_keys.is_empty()
    }

    #[must_use]
    pub fn settlements(&self) -> &BTreeMap<String, SettlementFact> {
        &self.settlements
    }

    pub(super) fn apply(&mut self, fact: RecoveryFact) -> Result<()> {
        match fact {
            RecoveryFact::ReservationMetadata(metadata) => {
                ensure!(
                    !self
                        .reservation_metadata
                        .contains_key(&metadata.client_order_id),
                    "duplicate submit-reservation metadata for client_order_id `{}`",
                    metadata.client_order_id
                );
                self.reservation_metadata
                    .insert(metadata.client_order_id.clone(), metadata);
            }
            RecoveryFact::ReservationFill(fill) => {
                self.reservation_fill_trade_ids
                    .entry((fill.client_order_id, fill.submit_reservation_id))
                    .or_default()
                    .insert(fill.trade_id);
            }
            RecoveryFact::Settlement(settlement) => {
                ensure!(
                    !self.settlements.contains_key(&settlement.settlement_key),
                    "duplicate settlement evidence for settlement_key `{}`",
                    settlement.settlement_key
                );
                self.settlements
                    .insert(settlement.settlement_key.clone(), settlement);
            }
            RecoveryFact::BookingError(booking_error) => {
                self.booking_error_keys.insert(booking_error.settlement_key);
            }
            RecoveryFact::TerminalSettlement(terminal) => {
                if terminal.booking_error.is_some() {
                    self.booking_error_keys
                        .insert(terminal.settlement_key.clone());
                }
                self.terminal_settlement_keys
                    .insert(terminal.settlement_key);
            }
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        for (client_order_id, submit_reservation_id) in self.reservation_fill_trade_ids.keys() {
            let metadata = self
                .reservation_metadata
                .get(client_order_id)
                .with_context(|| {
                    format!(
                        "submit-reservation fill has no metadata for client_order_id `{client_order_id}`"
                    )
                })?;
            ensure!(
                metadata.submit_reservation_id == *submit_reservation_id,
                "submit-reservation fill `{client_order_id}` reservation `{submit_reservation_id}` does not match submit-reservation metadata `{}`",
                metadata.submit_reservation_id
            );
        }
        Ok(())
    }
}

pub(super) enum CurrentFact {
    BlockedStrategyInputObservation(Box<BlockedStrategyInputObservationFact>),
    SubmitLinkedStrategyInputSnapshot(Box<SubmitLinkedStrategyInputSnapshotFact>),
    EntryOrderIntent(EntryOrderIntentFact),
    RiskReducingExitOrderIntent(RiskReducingExitOrderIntentFact),
    AdmittedEntryAdmission(Box<AdmittedEntryAdmissionFact>),
    RejectedEntryAdmission(Box<RejectedEntryAdmissionFact>),
    RiskReducingExitAdmission(Box<RiskReducingExitAdmissionFact>),
    ForcedReductionAdmission(Box<ForcedReductionAdmissionFact>),
    BasketAdmissionGranted(BasketAdmissionGrantedFact),
    BasketAdmissionRejected(BasketAdmissionRejectedFact),
    CapitalAdmissionRebuild(CapitalAdmissionRebuildFact),
    SubmitReservationMetadata(SubmitReservationMetadataFact),
    SubmitReservationFill(SubmitReservationFillFact),
    EntrySkipObservation(Box<EntrySkipFact>),
    ExitSubmissionDecision(Box<ExitSubmissionDecisionFact>),
    ExitHoldDecision(Box<ExitHoldDecisionFact>),
    ExitEvaluation(Box<ExitEvaluationFact>),
    LossGovernorHalt(LossGovernorHaltFact),
    OrderReject(Box<OrderRejectFact>),
    OrderLifecycle(OrderLifecycleFact),
    RequoteThrottleObservation(RequoteThrottleObservationFact),
    Settlement(SettlementFact),
    SettlementBookingError(SettlementBookingErrorFact),
    TerminalSettlement(Box<TerminalSettlementFact>),
    VenueTruthCaptureFailure(VenueTruthCaptureFailureFact),
    VenueTruthDivergence(VenueTruthDivergenceFact),
}

pub(super) enum RecoveryFact {
    ReservationMetadata(SubmitReservationMetadataFact),
    ReservationFill(SubmitReservationFillFact),
    Settlement(SettlementFact),
    BookingError(SettlementBookingErrorFact),
    TerminalSettlement(TerminalSettlementFact),
}
