use anyhow::Result;
use rust_decimal::Decimal;

use super::{
    BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence, BoltV3ExitEvaluationEvidence,
    BoltV3StrategyInputEvidenceSnapshot,
    generated_contract::{ConsumerDisposition, KnownConsumer, KnownDecodedFact, disposition},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedFact {
    AdmittedEntryAdmission(AdmissionFact),
    BasketAdmissionGranted(BasketAdmissionGrantedFact),
    BasketAdmissionRejected(BasketAdmissionRejectedFact),
    CapitalAdmissionRebuild(CapitalAdmissionRebuildFact),
    EntrySkipObservation(Box<BoltV3EntrySkipEvidence>),
    EntryOrderIntent(OrderIntentFact),
    ExitEvaluation(Box<BoltV3ExitEvaluationEvidence>),
    ExitHoldDecision(Box<BoltV3ExitDecisionEvidence>),
    ExitSubmissionDecision(Box<BoltV3ExitDecisionEvidence>),
    ForcedReductionAdmission(AdmissionFact),
    RiskReducingExitOrderIntent(OrderIntentFact),
    RejectedEntryAdmission(AdmissionFact),
    RiskReducingExitAdmission(AdmissionFact),
    LossGovernorHalt(LossGovernorHaltFact),
    OrderLifecycle(OrderLifecycleFact),
    OrderReject(OrderRejectFact),
    RequoteThrottle(RequoteThrottleFact),
    Settlement(SettlementFact),
    SettlementBookingError(SettlementBookingErrorFact),
    TerminalSettlement(TerminalSettlementFact),
    VenueTruthCaptureFailure(VenueTruthCaptureFailureFact),
    VenueTruthDivergence(VenueTruthDivergenceFact),
    SubmitReservationFill(SubmitReservationFillFact),
    SubmitReservationMetadata(SubmitReservationMetadataFact),
    BlockedStrategyInputObservation(Box<BoltV3StrategyInputEvidenceSnapshot>),
    SubmitLinkedStrategyInputSnapshot(Box<BoltV3StrategyInputEvidenceSnapshot>),
}

impl DecodedFact {
    pub(crate) const fn id(&self) -> KnownDecodedFact {
        match self {
            Self::BlockedStrategyInputObservation(_) => {
                KnownDecodedFact::BlockedStrategyInputObservationV1
            }
            Self::SubmitLinkedStrategyInputSnapshot(_) => {
                KnownDecodedFact::SubmitLinkedStrategyInputSnapshotV1
            }
            Self::EntryOrderIntent(_) => KnownDecodedFact::EntryOrderIntentV1,
            Self::RiskReducingExitOrderIntent(_) => KnownDecodedFact::RiskReducingExitOrderIntentV1,
            Self::AdmittedEntryAdmission(_) => KnownDecodedFact::AdmittedEntryAdmissionV1,
            Self::RejectedEntryAdmission(_) => KnownDecodedFact::RejectedEntryAdmissionV1,
            Self::RiskReducingExitAdmission(_) => KnownDecodedFact::RiskReducingExitAdmissionV1,
            Self::ForcedReductionAdmission(_) => KnownDecodedFact::ForcedReductionAdmissionV1,
            Self::BasketAdmissionGranted(_) => KnownDecodedFact::BasketAdmissionGrantedV1,
            Self::BasketAdmissionRejected(_) => KnownDecodedFact::BasketAdmissionRejectedV1,
            Self::CapitalAdmissionRebuild(_) => KnownDecodedFact::CapitalAdmissionRebuildV1,
            Self::SubmitReservationMetadata(_) => KnownDecodedFact::SubmitReservationMetadataV1,
            Self::SubmitReservationFill(_) => KnownDecodedFact::SubmitReservationFillV1,
            Self::EntrySkipObservation(_) => KnownDecodedFact::EntrySkipObservationV1,
            Self::ExitSubmissionDecision(_) => KnownDecodedFact::ExitSubmissionDecisionV1,
            Self::ExitHoldDecision(_) => KnownDecodedFact::ExitHoldDecisionV1,
            Self::ExitEvaluation(_) => KnownDecodedFact::ExitEvaluationV1,
            Self::LossGovernorHalt(_) => KnownDecodedFact::LossGovernorHaltV1,
            Self::OrderReject(_) => KnownDecodedFact::OrderRejectV1,
            Self::OrderLifecycle(_) => KnownDecodedFact::OrderLifecycleV1,
            Self::RequoteThrottle(_) => KnownDecodedFact::RequoteThrottleObservationV1,
            Self::Settlement(_) => KnownDecodedFact::SettlementV1,
            Self::SettlementBookingError(_) => KnownDecodedFact::SettlementBookingErrorV1,
            Self::TerminalSettlement(_) => KnownDecodedFact::TerminalSettlementV1,
            Self::VenueTruthCaptureFailure(_) => KnownDecodedFact::VenueTruthCaptureFailureV1,
            Self::VenueTruthDivergence(_) => KnownDecodedFact::VenueTruthDivergenceV1,
        }
    }

    pub(crate) fn route(self, consumer: KnownConsumer) -> Result<Option<Self>> {
        match disposition(self.id(), consumer) {
            ConsumerDisposition::Irrelevant => Ok(None),
            ConsumerDisposition::Relevant => Ok(Some(self)),
        }
    }
}

pub(crate) enum SubmitReservationRecoveryEvent {
    Metadata(SubmitReservationMetadataFact),
    Fill(SubmitReservationFillFact),
}

pub(crate) fn route_submit_reservation_recovery(
    fact: DecodedFact,
) -> Result<Option<SubmitReservationRecoveryEvent>> {
    let Some(fact) = fact.route(KnownConsumer::SubmitReservationRecoveryV1)? else {
        return Ok(None);
    };
    Ok(Some(match fact {
        DecodedFact::SubmitReservationMetadata(value) => {
            SubmitReservationRecoveryEvent::Metadata(value)
        }
        DecodedFact::SubmitReservationFill(value) => SubmitReservationRecoveryEvent::Fill(value),
        _ => anyhow::bail!("registry routed an unsupported fact to reservation recovery"),
    }))
}

pub(crate) enum SettlementRecoveryEvent {
    Settlement(SettlementFact),
}

pub(crate) fn route_settlement_recovery(
    fact: DecodedFact,
) -> Result<Option<SettlementRecoveryEvent>> {
    let Some(fact) = fact.route(KnownConsumer::SettlementRecoveryV1)? else {
        return Ok(None);
    };
    Ok(Some(match fact {
        DecodedFact::Settlement(value) => SettlementRecoveryEvent::Settlement(value),
        _ => anyhow::bail!("registry routed an unsupported fact to settlement recovery"),
    }))
}

pub(crate) enum BookingRecoveryEvent {
    BookingError(SettlementBookingErrorFact),
    TerminalSettlement(Box<TerminalSettlementFact>),
}

pub(crate) fn route_booking_recovery(fact: DecodedFact) -> Result<Option<BookingRecoveryEvent>> {
    let Some(fact) = fact.route(KnownConsumer::BookingRecoveryV1)? else {
        return Ok(None);
    };
    Ok(Some(match fact {
        DecodedFact::SettlementBookingError(value) => BookingRecoveryEvent::BookingError(value),
        DecodedFact::TerminalSettlement(value) => {
            BookingRecoveryEvent::TerminalSettlement(Box::new(value))
        }
        _ => anyhow::bail!("registry routed an unsupported fact to booking recovery"),
    }))
}

pub(crate) enum ShadowPnlEvent {
    Snapshot(Box<BoltV3StrategyInputEvidenceSnapshot>),
    EntryIntent(OrderIntentFact),
    AdmittedEntry(AdmissionFact),
}

pub(crate) fn route_shadow_pnl(fact: DecodedFact) -> Result<Option<ShadowPnlEvent>> {
    let Some(fact) = fact.route(KnownConsumer::ShadowPnlV1)? else {
        return Ok(None);
    };
    Ok(Some(match fact {
        DecodedFact::SubmitLinkedStrategyInputSnapshot(value) => ShadowPnlEvent::Snapshot(value),
        DecodedFact::EntryOrderIntent(value) => ShadowPnlEvent::EntryIntent(value),
        DecodedFact::AdmittedEntryAdmission(value) => ShadowPnlEvent::AdmittedEntry(value),
        _ => anyhow::bail!("registry routed an unsupported fact to shadow PnL"),
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionOutcomeFact {
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
    RejectedCapitalAdmission,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AdmissionLossHaltReasonFact {
    PerTradeLossLimit,
    DailyLossLimit,
    RollingLossLimit,
    MaxDrawdownLimit,
    StaleLossSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionLossSnapshotSourceFact {
    NtLossRuntimeFeed,
    NtPortfolioSnapshot,
    NtAccountSnapshot,
    NtAccountAndPositionSnapshot,
    NtPositionEvent,
    NtPositionChanged,
    NtPositionClosed,
    NtPositionAdjusted,
    NtSizingState,
    NtCapitalAdmissionState,
    BoltLossSnapshot,
    LossGovernor,
    Unknown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionLossSnapshotStaleReasonFact {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionFact {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: Decimal,
    pub outcome: AdmissionOutcomeFact,
    pub loss_halt_reasons: Vec<AdmissionLossHaltReasonFact>,
    pub snapshot_present: Option<bool>,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: Option<u64>,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: Option<u64>,
    pub snapshot_source: Option<AdmissionLossSnapshotSourceFact>,
    pub per_trade_pnl_present: Option<bool>,
    pub daily_pnl_present: Option<bool>,
    pub rolling_pnl_present: Option<bool>,
    pub current_equity_present: Option<bool>,
    pub peak_equity_present: Option<bool>,
    pub last_account_state_observed_at_ns: Option<u64>,
    pub last_portfolio_snapshot_observed_at_ns: Option<u64>,
    pub last_position_event_observed_at_ns: Option<u64>,
    pub stale_reason: Option<AdmissionLossSnapshotStaleReasonFact>,
    pub loss_snapshot_observed_at_ns: Option<u64>,
    pub loss_eval_now_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderIntentClampNotEvaluatedReasonFact {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderIntentClampOutcomeFact {
    WithinBounds,
    Clamped {
        original_quantity: Decimal,
    },
    Rejected,
    NotEvaluated {
        reason: OrderIntentClampNotEvaluatedReasonFact,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentOrderFieldsFact {
    pub order_type: String,
    pub time_in_force: String,
    pub price: Option<Decimal>,
    pub trigger_price: Option<Decimal>,
    pub activation_price: Option<Decimal>,
    pub trigger_type: Option<String>,
    pub trigger_instrument_id: Option<String>,
    pub trailing_offset: Option<Decimal>,
    pub trailing_offset_type: Option<String>,
    pub expire_time_unix_nanos: Option<u64>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIntentFact {
    pub strategy_id: String,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: Decimal,
    pub quantity: Decimal,
    pub clamp_outcome: Option<OrderIntentClampOutcomeFact>,
    pub order_fields: OrderIntentOrderFieldsFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectSourceFact {
    SubmitAdmission,
    Venue,
    NtExecution,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderRejectReasonFact {
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
    pub reject_source: RejectSourceFact,
    pub reject_reason: OrderRejectReasonFact,
    pub admission_outcome: Option<AdmissionOutcomeFact>,
    pub raw_reason_text: Option<String>,
    pub instrument_id: String,
    pub order_side: Option<String>,
    pub raw_price: Option<Decimal>,
    pub raw_quantity: Option<Decimal>,
    pub raw_maker_amount: Option<Decimal>,
    pub raw_taker_amount: Option<Decimal>,
    pub normalized_price: Option<Decimal>,
    pub normalized_quantity: Option<Decimal>,
    pub normalized_maker_amount: Option<Decimal>,
    pub normalized_taker_amount: Option<Decimal>,
    pub venue_price_precision: Option<u32>,
    pub venue_size_precision: Option<u32>,
    pub venue_min_notional: Option<Decimal>,
    pub prior_client_order_id: Option<String>,
    pub client_order_id: String,
    pub retry_count: u32,
    pub backoff_cooldown_state: Option<String>,
    pub stable_episode_key: String,
    pub elapsed_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueTruthDivergenceAlarm {
    TrueDivergence,
    OrderingViolation,
    SilentChannel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueTruthCaptureFailureFact {
    pub source: String,
    pub observed_at_ns: u64,
    pub endpoint: String,
    pub error_class: String,
    pub captures_missed: u64,
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
    pub alarm_class: VenueTruthDivergenceAlarm,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSettlementFact {
    pub settlement_key: String,
    pub booking_error: Option<SettlementBookingErrorFact>,
    pub lifecycle: OrderLifecycleFact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementOutcomeSide {
    Up,
    Down,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementOrderSide {
    Buy,
    Sell,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementBookingErrorReason {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettlementFact {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: String,
    pub position_id: String,
    pub instrument_id: String,
    pub product_id: String,
    pub outcome_side: SettlementOutcomeSide,
    pub entry_order_side: SettlementOrderSide,
    pub quantity: Decimal,
    pub entry_price: Decimal,
    pub family_key: String,
    pub strike_price: Decimal,
    pub resolution_instrument_id: String,
    pub resolution_ts_event_ns: u64,
    pub reference_close_price: Decimal,
    pub payout_per_share: Decimal,
    pub terminal_value: Decimal,
    pub realized_pnl: Decimal,
    pub settlement_currency: String,
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
pub enum RequoteActionCostClass {
    FreshSubmit,
    CancelResubmit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequoteThrottleBlockReason {
    RequoteBudgetExhausted,
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
pub struct RequoteThrottleFact {
    pub strategy_id: String,
    pub family_key: String,
    pub market_id: Option<String>,
    pub leg: String,
    pub now_ms: u64,
    pub observed_at_ns: u64,
    pub action_cost_class: RequoteActionCostClass,
    pub block_reason: RequoteThrottleBlockReason,
    pub bound_by: RequoteThrottleBound,
    pub submit_commands_in_window: usize,
    pub submit_command_cap: u64,
    pub submit_window_ms: u64,
    pub rest_cost_in_window: u64,
    pub rest_cap_per_minute: u64,
    pub rest_window_ms: u64,
    pub min_interval_ms: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderLifecycleSide {
    Buy,
    Sell,
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
    pub order_side: Option<OrderLifecycleSide>,
    pub filled_quantity: Option<Decimal>,
    pub residual_quantity: Option<Decimal>,
    pub ts_event_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasketAdmissionRejection {
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
pub struct BasketAdmissionFact {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub basket_id: String,
    pub group_id: String,
    pub leg_instrument_ids: Vec<String>,
    pub total_notional: Decimal,
    pub leg_order_count: u32,
}

pub type BasketAdmissionGrantedFact = BasketAdmissionFact;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasketAdmissionRejectedFact {
    pub admission: BasketAdmissionFact,
    pub reason: BasketAdmissionRejection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapitalAdmissionRebuildRejectionReason {
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
pub struct CapitalAdmissionRebuildFact {
    pub observed_at_ns: u64,
    pub source: String,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub accepted: bool,
    pub reason: Option<CapitalAdmissionRebuildRejectionReason>,
    pub attempted_reservation_count: usize,
    pub recovered_reservation_count: usize,
    pub live_reserved_liability: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationProductKind {
    PredictionMarketBinary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReservationMetadataFact {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: ReservationProductKind,
    pub collateral_currency: String,
    pub capital_pool_id: String,
    pub collateral_group_id: String,
    pub instrument_id: String,
    pub side: ReservationSide,
    pub submitted_quantity: Decimal,
    pub liability_factor: Decimal,
    pub additive_liability: Decimal,
    pub reserved_liability: Decimal,
    pub observed_at_ns: u64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitReservationFillFact {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub trade_id: String,
    pub instrument_id: String,
    pub side: ReservationSide,
    pub fill_quantity: Decimal,
    pub observed_at_ns: u64,
    pub reconciliation: bool,
    pub source: String,
}
