use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    str::FromStr,
};

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, timer::TimeEvent};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{CustomData, IndexPriceUpdate, QuoteTick, TradeTick},
    enums::{OrderSide, OrderStatus, OrderType, PositionSide, TimeInForce},
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, Venue},
    instruments::{Instrument, InstrumentAny},
    orders::Order,
    types::{Currency, Price, Quantity},
};
#[cfg(test)]
use nautilus_trading::Strategy;
use nautilus_trading::{StrategyConfig, StrategyCore, StrategyNative};
use rust_decimal::{
    Decimal,
    prelude::{FromPrimitive, ToPrimitive},
};
use toml::Value;

use crate::bolt_v3_evidence_novelty::{
    EvidenceEpisodeId, EvidenceEpisodeParts, EvidenceNoveltyGuard, EvidenceStateOwner,
};
#[cfg(test)]
use crate::bolt_v3_numeric::UNIT_F64;
use crate::bolt_v3_strategy_context::StrategyBuildContext;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EvidenceNoveltyProductionDomain {
    BlockedStrategyInputSnapshot,
    EntrySkip,
}

enum CachedExitOrderLifecycle {
    Working,
    Terminal {
        transition: OrderLifecycleTransition,
        raw_reason_text: Option<&'static str>,
        correction: BoltV3ExitOrderCorrection,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExitOrderAuthorityObservation {
    Lifecycle,
    Correction { cause: BoltV3RecoveredExitCause },
}

struct ExitOrderLifecycleObservationInput {
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    transition: OrderLifecycleTransition,
    source: OrderLifecycleSource,
    raw_reason_text: Option<String>,
    ts_event_ns: u64,
    authority: ExitOrderAuthorityObservation,
}

#[derive(Debug)]
enum ExitAuthorityRecoveryBlock {
    Construction(anyhow::Error),
    InvalidTerminalZeroFill,
    CachedOrderMissing,
    Observation(anyhow::Error),
}

impl std::fmt::Display for ExitAuthorityRecoveryBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Construction(error) => write!(formatter, "construction failed: {error:#}"),
            Self::InvalidTerminalZeroFill => {
                formatter.write_str("recovered authority produced an invalid zero-fill shortcut")
            }
            Self::CachedOrderMissing => {
                formatter.write_str("authoritative cached order is missing")
            }
            Self::Observation(error) => write!(formatter, "order observation failed: {error:#}"),
        }
    }
}

enum ExitAuthorityRecoveryAttempt {
    Working(ExitPendingState),
    TerminalAwaitingPosition(ExitPendingState),
    Blocked(ExitAuthorityRecoveryBlock),
}

impl ExitOrderAuthorityObservation {
    const fn is_correction(self) -> bool {
        matches!(self, Self::Correction { .. })
    }

    const fn correction(self) -> BoltV3ExitOrderCorrection {
        match self {
            Self::Lifecycle => BoltV3ExitOrderCorrection::Unchanged,
            Self::Correction { .. } => BoltV3ExitOrderCorrection::FillAuthorityChanged,
        }
    }

    fn recovery_plan(
        self,
        authority: &BoltV3ExitOrderAuthorityHandle,
    ) -> ExitAuthorityRecoveryPlan {
        match self {
            Self::Lifecycle => ExitAuthorityRecoveryPlan::Resume(authority.clone()),
            Self::Correction { cause } => ExitAuthorityRecoveryPlan::Reconstruct(cause),
        }
    }
}

const fn classify_cached_exit_order_lifecycle(status: OrderStatus) -> CachedExitOrderLifecycle {
    match status {
        OrderStatus::Initialized
        | OrderStatus::Emulated
        | OrderStatus::Released
        | OrderStatus::Submitted
        | OrderStatus::Accepted
        | OrderStatus::Triggered
        | OrderStatus::PendingUpdate
        | OrderStatus::PendingCancel
        | OrderStatus::PartiallyFilled => CachedExitOrderLifecycle::Working,
        OrderStatus::Denied => CachedExitOrderLifecycle::Terminal {
            transition: OrderLifecycleTransition::OrderDenied,
            raw_reason_text: Some("cached_denied"),
            correction: BoltV3ExitOrderCorrection::Unchanged,
        },
        OrderStatus::Rejected => CachedExitOrderLifecycle::Terminal {
            transition: OrderLifecycleTransition::OrderRejected,
            raw_reason_text: Some("cached_rejected"),
            correction: BoltV3ExitOrderCorrection::Unchanged,
        },
        OrderStatus::Canceled => CachedExitOrderLifecycle::Terminal {
            transition: OrderLifecycleTransition::OrderCanceled,
            raw_reason_text: Some("cached_canceled"),
            correction: BoltV3ExitOrderCorrection::Unchanged,
        },
        OrderStatus::Expired => CachedExitOrderLifecycle::Terminal {
            transition: OrderLifecycleTransition::OrderExpired,
            raw_reason_text: Some("cached_expired"),
            correction: BoltV3ExitOrderCorrection::Unchanged,
        },
        OrderStatus::Filled => CachedExitOrderLifecycle::Terminal {
            transition: OrderLifecycleTransition::OrderFilled,
            raw_reason_text: Some("cached_filled"),
            correction: BoltV3ExitOrderCorrection::Unchanged,
        },
        OrderStatus::Voided => CachedExitOrderLifecycle::Terminal {
            transition: OrderLifecycleTransition::OrderFilled,
            raw_reason_text: Some("cached_voided"),
            correction: BoltV3ExitOrderCorrection::FillAuthorityChanged,
        },
    }
}

const fn production_novelty_domain(owner: EvidenceStateOwner) -> EvidenceNoveltyProductionDomain {
    match owner {
        EvidenceStateOwner::BlockedStrategyInputSnapshot => {
            EvidenceNoveltyProductionDomain::BlockedStrategyInputSnapshot
        }
        EvidenceStateOwner::EntrySkip => EvidenceNoveltyProductionDomain::EntrySkip,
    }
}

fn production_novelty_guard(owner: EvidenceStateOwner) -> Result<EvidenceNoveltyGuard> {
    let _domain = production_novelty_domain(owner);
    EvidenceNoveltyGuard::for_owner(owner)
}

use crate::{
    bolt_v3_binary_outcome_edge::{
        BinaryOutcomeEdgeBlockReason, BinaryOutcomeEdgeInputs, BinaryOutcomeEdgeResult,
        evaluate_binary_outcome_edge,
    },
    bolt_v3_book_sizing::{
        OutcomeBookState, OutcomeBookSubscriptions, should_replace_book_subscriptions,
    },
    bolt_v3_current_evidence::{
        BlockedStrategyInputObservationFact, EntrySkipReason as EvidenceEntrySkipReason,
        EvidenceOrderSide, ExitAttemptOutcome, ExitBlockedReason as EvidenceExitBlockedReason,
        ExitEvaluationFact, ExitHoldDecisionFact, ExitHoldOutcome, ExitIntentDecisionFact,
        ExitIntentOutcome, ExitPreparationStage, ExitPreparedOrderFact,
        ExitTriggerSource as EvidenceExitTriggerSource,
        ExposureOccupancy as EvidenceExposureOccupancy,
        ForcedFlatReason as EvidenceForcedFlatReason, NonBlockingRecordOutcome,
        ObservationRecordOutcome, OrderIntentDetails, OrderLifecycleFact, OrderLifecycleOutcome,
        OrderLifecycleSource, OrderLifecycleTransition, OutcomeSide as EvidenceOutcomeSide,
        PreparedOrderLinkage, RecoveredSettlementOutcome, RvGateResult as EvidenceRvGateResult,
        SettlementBookingErrorFact, SettlementBookingErrorReason, SettlementFact,
        StrategyInputDetails, StrategyInputRvState, SubmissionLinkage,
        SubmitLinkedStrategyInputSnapshotFact, TerminalSettlementFact,
        realized_vol_diagnostic_fact, settlement_kind,
    },
    bolt_v3_evidence_values::{
        number as evidence_number, optional_number as option_evidence_number,
        optional_probability as option_evidence_probability, probability as probability_evidence,
    },
    bolt_v3_executable_cost::{
        ExactSizeVwap, ExecutableBookQuote, ExecutableCostBreakdown, executable_cost_breakdown,
        price_exact_size_vwap,
    },
    bolt_v3_fair_value_pricing::{RealizedVolGateClassification, classify_rv_gate},
    bolt_v3_loss_protection::{PositionRealizedPnlObservation, RealizedPnlObservation},
    bolt_v3_market_families::{self, FairProbabilityInputs, OutcomeSide},
    bolt_v3_numeric::{
        BPS_DENOMINATOR, MIDPOINT_DIVISOR_F64, MILLIS_PER_SECOND_U64, Probability,
        SECONDS_PER_YEAR_F64, is_positive_finite, notional_float_tolerance,
    },
    bolt_v3_operator_health::BoltV3SettlementHealthTransition,
    bolt_v3_order_execution::{
        BoltV3CanonicalPositionAuthority, BoltV3CompileAndSealRiskReducingIocInput,
        BoltV3ExitAuthorityRecoveryHandle, BoltV3ExitAuthorityRecoveryRelease,
        BoltV3ExitOrderAuthorityHandle, BoltV3ExitOrderCorrection,
        BoltV3ExitOrderLifecycleReduction, BoltV3FinalOrderEconomicsInput,
        BoltV3FinalOrderEconomicsScenario, BoltV3PlannedFillLeg, BoltV3PositionReductionRelease,
        BoltV3RecoveredExitCause, BoltV3RiskReducingIocPreparationStage, BoltV3SubmitAttemptKind,
        BoltV3SubmitAttemptOutcome, BoltV3SubmitAttemptState, BoltV3SubmitContext,
        BoltV3SubmitRoutingRequest, BoltV3TakerEconomicsSizingInput, BoltV3TerminalValueEntry,
        BoltV3TerminalValueEntryPolicy, build_order_economics_submit_admission,
        compile_and_seal_risk_reducing_ioc, order_intent_details_from_compiled_order,
        prepared_order_linkage,
    },
    bolt_v3_order_intent::{
        MarketQuoteBuyQuantityError, make_market_quote_buy_quantity, normalize_base_order_quantity,
    },
    bolt_v3_position_contract::{BoltV3PositionMarketLifecycle, is_observed_open_side},
    bolt_v3_prediction_market_instrument::prediction_market_product_id_from_instrument_id,
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::QuoteSide,
    bolt_v3_reference_price::{
        ReferencePriceSelector, ReferencePriceSourceHealth, ReferencePriceSourceSpec,
        ReferencePriceSourceStatus, ReferencePriceUpdate, ReferenceQuote,
        reference_price_source_is_runtime_available, reference_price_source_is_unsupported,
    },
    bolt_v3_reference_price_health::{
        ReferencePriceLiveWindow, ReferencePriceUpdateObservation, ReferencePriceUpdateRejection,
        observe_reference_price_update as observe_reference_price_health_update,
        select_current_reference_price as select_reference_price_from_health,
    },
    bolt_v3_settlement_booking::{
        ResolutionSettlementDecision, ResolutionSettlementInput, SettlementPositionKey,
        SettlementPositionOrigin, SettlementRecoveryEntryDecision, SettlementTerminalKeyDelta,
        TerminalSettlementEligibility,
        bootstrap_recovery_from_cache as decide_bootstrap_recovery_from_cache,
        enter_blind_settlement_recovery as decide_blind_settlement_recovery,
        record_settlement_booking_error as decide_settlement_booking_error, recover_booking_facts,
        recover_settlement_facts, try_book_resolution_settlement,
    },
    bolt_v3_sizing::{RobustSizingInputs, choose_robust_size},
    bolt_v3_submit_admission::{
        BoltV3RiskReducingExitPositionInput, BoltV3SubmitAdmissionRequestInput,
        BoltV3SubmitIntentKind, OrderValuationContext, PredictionMarketOutcomeSide,
        limit_notional_exceeds_sized_notional,
    },
    bolt_v3_taker_pricing::{
        FastSpotObservation, TakerPricingConfig, TakerPricingRequest,
        TakerPricingState as PricingState,
    },
    bolt_v3_taker_updown_signal::{
        SideSelectionInputs, UncertaintyBandInputs, choose_entry_side,
        time_uncertainty_probability, uncertainty_band_probability,
    },
    bolt_v3_timestamp_domain::{LocalReceiveMs, VenueEventMs},
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::registry::{StrategyBuilder, ValidationError},
};

#[cfg(test)]
use nautilus_model::enums::{
    AggressorSide, BookAction, OmsType as NtOmsType, TrailingOffsetType, TriggerType,
};

#[cfg(test)]
use crate::{
    bolt_v3_market_families::{
        MarketSelectionOutcome, SelectedMarketEvidenceIdentity, SelectedMarketEvidenceOutcome,
        SelectedMarketSourceIdentity,
    },
    bolt_v3_submit_admission::{BoltV3RiskReducingExitProof, BoltV3SubmitAdmissionRequest},
    bolt_v3_taker_pricing::VenueTimingState,
    bolt_v3_taker_updown_signal::{price_agreement_corr, price_gap_probability},
};

pub mod archetype;

mod selection;

enum StrategyEconomicsInput {
    TerminalEntry {
        terminal_value_entry: BoltV3TerminalValueEntry,
        candidate_fill_levels: Vec<BoltV3PlannedFillLeg>,
    },
    #[cfg(test)]
    CompiledFinalOrderFixture,
}

#[cfg(test)]
use self::selection::CandidateMarket;
use self::selection::{
    RuntimeSelectionSnapshot, SelectionPhase, SelectionState, apply_selection_snapshot_to_active,
    idle_selection_snapshot, selected_market_on_execution_venue, selection_book_subscriptions,
    selection_snapshot_from_instruments, strategy_input_market_selection_outcome,
};

mod config;

pub use self::config::BinaryOracleEdgeTakerBuilder;
#[cfg(test)]
use self::config::BinaryOracleEdgeTakerOrderConfig;
use self::config::{BinaryOracleEdgeTakerConfig, BinaryOracleEdgeTakerFieldType};

mod exposure;

use self::exposure::{
    BlindRecoveryReason, BlindRecoveryState, ConfiguredPositionContract, EntryReconcileReason,
    ExitAttemptingState, ExitAuthorityFlatRecovery, ExitAuthorityRecoveryHoldState,
    ExitAuthorityRecoveryPlan, ExitLifecyclePhase, ExitPendingState, ExposureOccupancy,
    ExposureState, ManagedPositionContext, ManagedPositionOrigin, ManagedPositionState,
    OpenPositionState, PendingEntryState, PendingExitState, UnsupportedObservedReason,
    UnsupportedObservedState, infer_strategy_position_side_from_entry_fill,
    managed_position_effective_entry_cost, supports_strategy_managed_position,
};
use crate::bolt_v3_feed_health::{
    ForcedFlatInputs, ForcedFlatReason, evaluate_forced_flat_predicates,
};

mod entry_decision;

use self::entry_decision::{
    EntryBlockReason, EntryEvaluation, EntryEvaluationLogFields, EntryEvaluationReceiveContext,
    EntryGateDecision, EntryPricingBlockReason, EntryPricingInputs, EntryRealizedVolatilityReceipt,
    EntrySubmissionDecision, ForcedFlatEvidenceInputs, RealizedVolatilityEvidenceFields,
    blocked_strategy_input_canonical_state, entry_block_reason_to_evidence,
    entry_pricing_block_reason_from_taker, entry_pricing_block_reason_to_evidence,
    entry_skip_canonical_state, entry_skip_fact, entry_skip_reason_label,
    push_executable_edge_pricing_block,
};

mod exit_decision;

use self::exit_decision::{
    ExitDecision, ExitDecisionDedupeKey, ExitDecisionDisposition, ExitEvaluation,
    ExitEvaluationLogFields, ExitEvaluationTriggerContext, ExitIntentDecision, ExitOutcomeKey,
    ExitRealizedVolatilityGateReceipt, evaluate_exit_decision, exit_block_reason_label,
    exit_decision_details, exit_decision_evidence_from_optional,
};

mod orders;

#[cfg(test)]
use self::orders::{EntryOrderPlanInputs, build_entry_order_plan};
use self::orders::{
    ExitOrderExecutionConfig, parse_configured_oms_type, parse_configured_order_side,
    parse_configured_position_side,
};

mod runtime_state;

use self::runtime_state::{
    ActiveMarketState, MarketLifecycleLedger, reference_current_price_boundary_changed,
};
#[cfg(test)]
use self::runtime_state::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind};

mod subscriptions;

#[cfg(test)]
use self::subscriptions::{
    BookSubscriptionEvent, LiveInputSubscriptionRetryEvent, ReferencePriceSubscribeEvent,
    ResolutionStrikeSubscribeEvent,
};
#[cfg(test)]
use self::subscriptions::{
    REFERENCE_PRICE_SUBSCRIBE_ACTION, REFERENCE_PRICE_UNSUBSCRIBE_ACTION,
    ResolutionStrikeFetchTrigger,
};
use self::subscriptions::{
    ResolutionReportBoundarySubscriptionState, ResolutionReportSubscriptionOutcome,
    ResolutionStrikeReportBoundary,
};

#[derive(Debug, Clone)]
struct ExecutableEntryProbe {
    order_side: OrderSide,
    vwap: ExactSizeVwap,
}

const ORDER_LIFECYCLE_SOURCE_SELECTION_BOUNDARY: OrderLifecycleSource =
    OrderLifecycleSource::SelectionBoundary;
const ORDER_LIFECYCLE_SOURCE_ENTRY_FILL: OrderLifecycleSource = OrderLifecycleSource::EntryFill;
const ORDER_LIFECYCLE_SOURCE_POSITION_EVENT: OrderLifecycleSource =
    OrderLifecycleSource::PositionEvent;
const ORDER_LIFECYCLE_SOURCE_RESTART_BOOTSTRAP: OrderLifecycleSource =
    OrderLifecycleSource::RestartBootstrap;
const ORDER_LIFECYCLE_SOURCE_ORDER_DENIED: OrderLifecycleSource = OrderLifecycleSource::OrderDenied;
const ORDER_LIFECYCLE_SOURCE_ORDER_REJECTED: OrderLifecycleSource =
    OrderLifecycleSource::OrderRejected;
const ORDER_LIFECYCLE_SOURCE_ORDER_CANCELED: OrderLifecycleSource =
    OrderLifecycleSource::OrderCanceled;
const ORDER_LIFECYCLE_SOURCE_ORDER_EXPIRED: OrderLifecycleSource =
    OrderLifecycleSource::OrderExpired;
const ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS: OrderLifecycleSource =
    OrderLifecycleSource::ReconcilePass;
const ORDER_LIFECYCLE_SOURCE_SETTLEMENT_RECOVERY: OrderLifecycleSource =
    OrderLifecycleSource::SettlementEvidenceRecovery;
const ORDER_LIFECYCLE_SOURCE_SETTLEMENT_BOOKING_TERMINAL: OrderLifecycleSource =
    OrderLifecycleSource::SettlementBookingTerminal;
const ENTRY_RECONCILE_FILL_OBSERVED_TERMINAL_REASON: &str =
    "preserved fail-closed: fill observed, awaiting position truth";

#[derive(Debug, Clone)]
struct OrderLifecycleEvidenceInput {
    transition: OrderLifecycleTransition,
    outcome: OrderLifecycleOutcome,
    source: OrderLifecycleSource,
    market_id: Option<String>,
    instrument_id: Option<InstrumentId>,
    position_id: Option<PositionId>,
    client_order_id: Option<ClientOrderId>,
    prior_client_order_id: Option<ClientOrderId>,
    raw_reason_text: Option<String>,
    order_side: Option<OrderSide>,
    filled_quantity: Option<Quantity>,
    residual_quantity: Option<Quantity>,
    ts_event_ns: Option<u64>,
}

#[derive(Debug, Clone)]
struct PendingEntryTerminalEvidenceInput {
    pending: PendingEntryState,
    transition: OrderLifecycleTransition,
    outcome: OrderLifecycleOutcome,
    source: OrderLifecycleSource,
    raw_reason_text: Option<String>,
    filled_quantity: Option<Quantity>,
    ts_event_ns: u64,
}

#[derive(Debug, Clone)]
struct PendingEntryTerminalEventInput {
    client_order_id: ClientOrderId,
    event_instrument_id: InstrumentId,
    transition: OrderLifecycleTransition,
    source: OrderLifecycleSource,
    raw_reason_text: Option<String>,
    ts_event_ns: u64,
    terminal_proves_zero_fill: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PositionMaterializationSpec {
    instrument_id: InstrumentId,
    position_id: PositionId,
    entry_order_side: OrderSide,
    side: PositionSide,
    quantity: Quantity,
    avg_px_open: f64,
}

#[derive(Debug, Clone)]
struct FlatTerminalEntryOverride {
    client_order_id: ClientOrderId,
    market_id: Option<String>,
    instrument_id: InstrumentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalEvidenceState {
    PersistCanonical,
    CanonicalAlreadyDurable,
}

/// Project the strategy's trade-flow TOML knobs into the buffer's runtime config
/// view. Single place that maps those fields onto [`SignedTradeFlowConfig`]
/// (mirrors [`realized_vol_config`]).
fn signed_trade_flow_config(config: &BinaryOracleEdgeTakerConfig) -> SignedTradeFlowConfig {
    SignedTradeFlowConfig {
        window_secs: config.trade_flow_window_secs,
        max_samples: config.trade_flow_max_samples,
    }
}

fn reference_price_selector_from_config(
    config: &BinaryOracleEdgeTakerConfig,
) -> Option<ReferencePriceSelector> {
    let reference_price = config.reference_current_price.as_ref()?;
    let source_specs = reference_price
        .source_order
        .iter()
        .filter_map(|source_id| {
            let source = reference_price.sources.get(source_id)?;
            reference_price_source_is_runtime_available(reference_price, source).then(|| {
                if source.required {
                    ReferencePriceSourceSpec::required(source_id.clone())
                } else {
                    ReferencePriceSourceSpec::optional(source_id.clone())
                }
            })
        })
        .collect::<Vec<_>>();
    if source_specs.is_empty() {
        return None;
    }
    Some(
        ReferencePriceSelector::new_with_source_specs_and_drift_policy(
            reference_price.asset.clone(),
            source_specs,
            reference_price.min_valid_sources,
            reference_price.max_source_age_ms,
            reference_price.max_source_drift_bps,
            reference_price.drift_policy,
        )
        .expect("validated reference_current_price selector config"),
    )
}

fn reference_price_source_health_from_config(
    config: &BinaryOracleEdgeTakerConfig,
) -> BTreeMap<String, ReferencePriceSourceHealth> {
    let Some(reference_price) = &config.reference_current_price else {
        return BTreeMap::new();
    };
    reference_price
        .sources
        .iter()
        .map(|(source_id, source)| {
            let status = if !source.enabled {
                ReferencePriceSourceStatus::Disabled
            } else if reference_price_source_is_unsupported(reference_price, source) {
                ReferencePriceSourceStatus::UnsupportedSymbol
            } else {
                ReferencePriceSourceStatus::Silent
            };
            (
                source_id.clone(),
                ReferencePriceSourceHealth::new(
                    source_id.clone(),
                    source.provider.clone(),
                    status,
                    None,
                    None,
                ),
            )
        })
        .collect()
}

fn order_price_for_side(
    book: &OutcomeBookState,
    order_side: OrderSide,
    is_post_only: bool,
) -> Option<f64> {
    if is_post_only {
        book.passive_price_for_order_side(order_side)
    } else {
        book.executable_price_for_order_side(order_side)
    }
}

fn executable_book_quote(book: &OutcomeBookState) -> ExecutableBookQuote<'_> {
    ExecutableBookQuote {
        best_bid: book.best_bid,
        best_ask: book.best_ask,
        bid_levels: &book.bid_levels,
        ask_levels: &book.ask_levels,
    }
}

fn visible_book_depth_side_for_order(
    order_side: OrderSide,
    is_post_only: bool,
) -> Option<OrderSide> {
    match (order_side, is_post_only) {
        (OrderSide::Buy, false) | (OrderSide::Sell, true) => Some(OrderSide::Buy),
        (OrderSide::Sell, false) | (OrderSide::Buy, true) => Some(OrderSide::Sell),
        _ => None,
    }
}

fn executable_edge_vwap_price(result: Option<BinaryOutcomeEdgeResult>) -> Option<f64> {
    result.and_then(|result| result.cost_breakdown.vwap_price)
}

fn executable_edge_limit_price(result: Option<BinaryOutcomeEdgeResult>) -> Option<f64> {
    result.and_then(|result| result.cost_breakdown.limit_price)
}

fn executable_edge_cost_component(
    result: Option<BinaryOutcomeEdgeResult>,
    component: fn(&ExecutableCostBreakdown) -> f64,
) -> Option<f64> {
    let result = result?;
    if !result.cost_breakdown.cost_available {
        return None;
    }
    let value = component(&result.cost_breakdown);
    value.is_finite().then_some(value)
}

fn executable_edge_worst_case_ev_bps(result: Option<BinaryOutcomeEdgeResult>) -> Option<f64> {
    let result = result?;
    if !result.cost_breakdown.cost_available {
        return None;
    }
    result.edge_bps.is_finite().then_some(result.edge_bps)
}

fn executable_edge_selectable_bps(result: Option<BinaryOutcomeEdgeResult>) -> Option<f64> {
    let result = result?;
    result.trade_allowed.then_some(result.edge_bps)
}

fn executable_submission_vwap_from_evaluation(
    evaluation: &EntryEvaluation,
    selected_side: OutcomeSide,
) -> Option<ExactSizeVwap> {
    if let Some(result) = evaluation
        .sized_executable_edge
        .as_ref()
        .filter(|result| result.selected_side == selected_side && result.trade_allowed)
    {
        return Some(ExactSizeVwap {
            vwap_price: result.cost_breakdown.vwap_price?,
            vwap_quantity: result.cost_breakdown.vwap_quantity?,
            limit_price: result.cost_breakdown.limit_price?,
            exact_size_filled: result.cost_breakdown.exact_size_filled,
            fill_legs: Vec::new(),
            candidate_levels: Vec::new(),
        });
    }
    let result = match selected_side {
        OutcomeSide::Up => evaluation.up_executable_edge.as_ref(),
        OutcomeSide::Down => evaluation.down_executable_edge.as_ref(),
    }?;
    if result.selected_side != selected_side || !result.trade_allowed {
        return None;
    }
    let cost = result.cost_breakdown;
    if !cost.cost_available || !cost.exact_size_filled {
        return None;
    }
    Some(ExactSizeVwap {
        vwap_price: cost.vwap_price.filter(|value| is_positive_finite(*value))?,
        vwap_quantity: cost
            .vwap_quantity
            .filter(|value| is_positive_finite(*value))?,
        limit_price: cost
            .limit_price
            .filter(|value| is_positive_finite(*value))?,
        exact_size_filled: cost.exact_size_filled,
        fill_legs: Vec::new(),
        candidate_levels: Vec::new(),
    })
}

fn executable_edge_cents_per_share(result: Option<BinaryOutcomeEdgeResult>) -> Option<f64> {
    let result = result?;
    if !result.cost_breakdown.cost_available {
        return None;
    }
    result
        .edge_cents_per_share
        .is_finite()
        .then_some(result.edge_cents_per_share)
}

fn taker_pricing_config(config: &BinaryOracleEdgeTakerConfig) -> TakerPricingConfig<'_> {
    TakerPricingConfig {
        realized_volatility_surface_id: config.realized_volatility_surface_id.clone(),
        realized_volatility_max_source_age_ms: Some(config.realized_volatility_max_source_age_ms),
        lead_agreement_min_corr: config.lead_agreement_min_corr,
        lead_jitter_max_ms: config.lead_jitter_max_ms,
        spike_guard_return_threshold: config.spike_guard_return_threshold,
        spike_guard_cooldown_secs: config.spike_guard_cooldown_secs,
        cadence_seconds: config.cadence_seconds,
        theta_decay_factor: config.theta_decay_factor,
        edge_threshold_basis_points: config.edge_threshold_basis_points,
        pricing_kurtosis: config.pricing_kurtosis,
        rotating_market_family: config.rotating_market_family.as_str(),
        max_reference_current_price_age_ms: config
            .reference_current_price
            .as_ref()
            .map(|reference_price| reference_price.max_source_age_ms),
    }
}

impl PricingState {
    #[cfg(test)]
    fn observe_reference_snapshot(
        &mut self,
        snapshot: &ReferenceSnapshot,
        min_agreement_corr: f64,
        max_jitter_ms: u64,
    ) {
        if let Some(reference_current_price) =
            snapshot.fair_value.filter(|reference_current_price| {
                reference_current_price.is_finite() && *reference_current_price > 0.0
            })
            && self
                .last_reference_current_price_ts_ms()
                .is_none_or(|last| snapshot.ts_ms > last)
        {
            self.set_last_reference_observation(
                Some(snapshot.ts_ms),
                Some(reference_current_price),
            );
        }

        let candidates = self.build_lead_venue_signals(snapshot);
        self.lead_quality_policy_applied = true;
        if let Some(candidate) =
            arbitrate_lead_reference(&candidates, min_agreement_corr, max_jitter_ms)
        {
            let (Some(price), Some(observed_ts_ms), Some(jitter_penalty_probability)) = (
                candidate.price,
                candidate.observed_ts_ms,
                PricingState::jitter_penalty_probability(candidate.jitter_ms, max_jitter_ms),
            ) else {
                self.set_selected_pricing_spot(None);
                self.last_lead_gap_probability = None;
                self.last_jitter_penalty_probability = None;
                self.last_lead_agreement_corr = None;
                self.last_fast_venue_age_ms = None;
                self.last_fast_venue_jitter_ms = None;
                self.fast_venue_incoherent = true;
                return;
            };
            let fast_spot = FastSpotObservation {
                venue: candidate.venue_name.clone(),
                price,
                observed_ts_ms,
                received_ts_ms: None,
            };
            self.set_selected_pricing_spot(Some(fast_spot));
            self.last_lead_gap_probability = Some(candidate.lead_gap_probability);
            self.last_jitter_penalty_probability = Some(jitter_penalty_probability);
            self.last_lead_agreement_corr = Some(candidate.agreement_corr);
            self.last_fast_venue_age_ms = Some(candidate.age_ms);
            self.last_fast_venue_jitter_ms = Some(candidate.jitter_ms);
            self.fast_venue_incoherent = false;
        } else {
            self.set_selected_pricing_spot(None);
            self.last_lead_gap_probability = None;
            self.last_jitter_penalty_probability = None;
            self.last_lead_agreement_corr = None;
            self.last_fast_venue_age_ms = None;
            self.last_fast_venue_jitter_ms = None;
            self.fast_venue_incoherent = !candidates.is_empty();
        }
    }

    #[cfg(test)]
    fn build_lead_venue_signals(&mut self, snapshot: &ReferenceSnapshot) -> Vec<LeadVenueSignal> {
        let reference_anchor = self.last_reference_current_price();
        let agreement_anchor = best_healthy_oracle_price(snapshot).or(reference_anchor);

        snapshot
            .venues
            .iter()
            .filter_map(|venue| {
                if venue.venue_kind != VenueKind::Orderbook
                    || venue.stale
                    || !matches!(venue.health, VenueHealth::Healthy)
                    || !venue.effective_weight.is_finite()
                    || venue.effective_weight <= 0.0
                {
                    return None;
                }

                let observed_price = venue.observed_price?;
                let observed_ts_ms = venue.observed_ts_ms?;
                if !observed_price.is_finite() || observed_price <= 0.0 {
                    return None;
                }

                let timing = self
                    .venue_timing
                    .entry(venue.venue_name.clone())
                    .or_insert_with(VenueTimingState::empty);
                let age_ms = snapshot.ts_ms.saturating_sub(observed_ts_ms);
                let current_interval_ms = timing
                    .last_observed_ts_ms
                    .map(|last_ts_ms| observed_ts_ms.saturating_sub(last_ts_ms));
                let jitter_ms = match (current_interval_ms, timing.last_interval_ms) {
                    (Some(current_interval_ms), Some(last_interval_ms)) => {
                        current_interval_ms.abs_diff(last_interval_ms)
                    }
                    _ => 0,
                };
                timing.last_observed_ts_ms = Some(observed_ts_ms);
                timing.last_interval_ms = current_interval_ms;

                let agreement_anchor =
                    agreement_anchor.filter(|anchor| anchor.is_finite() && *anchor > 0.0)?;
                let reference_anchor =
                    reference_anchor.filter(|anchor| anchor.is_finite() && *anchor > 0.0)?;
                let agreement_corr = price_agreement_corr(observed_price, agreement_anchor)?;
                let lead_gap_probability = price_gap_probability(observed_price, reference_anchor)?;

                Some(LeadVenueSignal {
                    venue_name: venue.venue_name.clone(),
                    price: Some(observed_price),
                    observed_ts_ms: Some(observed_ts_ms),
                    age_ms,
                    jitter_ms,
                    agreement_corr,
                    effective_weight: venue.effective_weight,
                    lead_gap_probability,
                })
            })
            .collect()
    }
}

pub struct BinaryOracleEdgeTaker {
    core: StrategyCore,
    config: BinaryOracleEdgeTakerConfig,
    context: StrategyBuildContext,
    active: ActiveMarketState,
    book_subscriptions: OutcomeBookSubscriptions,
    market_lifecycle: BTreeMap<String, MarketLifecycleLedger>,
    exposure: ExposureState,
    next_exit_attempt_generation: u64,
    last_flat_terminal_entry_override: Option<FlatTerminalEntryOverride>,
    last_reported_exposure_occupancy: Cell<Option<ExposureOccupancy>>,
    /// Suppression for the three producers that record once per semantic
    /// episode. The identity is each producer's own; only the mask arithmetic
    /// and the mark-before-write contract are shared.
    blocked_strategy_input_novelty: EvidenceNoveltyGuard,
    entry_skip_novelty: EvidenceNoveltyGuard,
    // Not registry-backed, and deliberately so. The frozen novelty registry
    // defines finite domains for the two producers above and none for this one,
    // so this stays the adjacent-repeat guard it has always been: it suppresses
    // an immediately repeated decision and re-emits on A-B-A. The census records
    // that as the outstanding migration to a closed exit-outcome domain rather
    // than claiming a suppression this does not implement.
    last_recorded_exit_decision: Option<ExitDecisionDedupeKey>,
    pricing: PricingState,
    latest_signal_quote: Option<FastSpotObservation>,
    latest_selected_reference_quote: Option<SelectedReferenceQuoteEvidence>,
    reference_price_selector: Option<ReferencePriceSelector>,
    reference_price_quotes: BTreeMap<String, ReferenceQuote>,
    reference_price_source_health: BTreeMap<String, ReferencePriceSourceHealth>,
    selection_missing_since_ms: Option<u64>,
    resolution_report_boundary_subscriptions:
        BTreeMap<ResolutionStrikeReportBoundary, ResolutionReportBoundarySubscriptionState>,
    resolution_strike_fetch_sequence: u64,
    entry_reject_state: BTreeMap<InstrumentId, EntryRejectState>,
    settled_position_keys: BTreeSet<String>,
    settlement_booking_error_keys: BTreeSet<String>,
    terminal_settlement_keys: BTreeSet<String>,
    settlement_close_fetch_attempts: BTreeMap<String, SettlementCloseFetchAttemptState>,
    /// Flood guard for entry-evaluation log volume: last gate+pricing block-reason
    /// sets. WARN/INFO only on set change (blocked↔unblocked); full field dump is debug.
    last_entry_block_reason_sets: Option<(Vec<EntryBlockReason>, Vec<EntryPricingBlockReason>)>,
    /// Flood guard for #885 exit-evaluation evidence: the last durable outcome key
    /// recorded per open position. A durable record is emitted only when this key
    /// changes (or on an actual submit), collapsing a per-tick exit flood (e.g. the
    /// 2026-06-20 incident) into one record per distinct outcome. The per-tick
    /// tracing log is unaffected.
    last_exit_evidence_outcome: BTreeMap<PositionId, ExitOutcomeKey>,
    #[cfg(test)]
    book_subscription_events: Vec<BookSubscriptionEvent>,
    /// Test-only observability for live-strike fetch attempts. Records each
    /// logical fetch trigger, not transport cleanup, so tests can assert that
    /// retries do not depend on NT forwarding an immediate index unsubscribe.
    #[cfg(test)]
    resolution_strike_subscribe_events: Vec<ResolutionStrikeSubscribeEvent>,
    #[cfg(test)]
    reference_price_subscribe_events: Vec<ReferencePriceSubscribeEvent>,
    #[cfg(test)]
    live_input_subscription_retry_events: Vec<LiveInputSubscriptionRetryEvent>,
}

#[derive(Clone, Copy, Debug)]
struct SettlementEvidenceComputation {
    outcome_side: OutcomeSide,
    strike_price: f64,
    payout_per_share: f64,
    terminal_value: f64,
    realized_pnl: f64,
}

#[derive(Clone, Debug)]
struct SettlementEvidenceIds {
    settlement_key: String,
    market_id: String,
    product_id: String,
}

enum ExitAttemptExecution {
    Completed {
        decision: ExitIntentDecision,
        outcome: ExitAttemptOutcome,
    },
    Rejected {
        decision: ExitIntentDecision,
        outcome: ExitAttemptOutcome,
        failure: anyhow::Error,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitAttemptDisposition {
    Submitted,
    NonSubmitted,
}

impl ExitAttemptExecution {
    fn completed(decision: ExitIntentDecision, outcome: ExitAttemptOutcome) -> Self {
        Self::Completed { decision, outcome }
    }

    fn rejected(
        decision: ExitIntentDecision,
        outcome: ExitAttemptOutcome,
        failure: anyhow::Error,
    ) -> Self {
        Self::Rejected {
            decision,
            outcome,
            failure,
        }
    }

    fn evidence(&self) -> (&ExitIntentDecision, &ExitAttemptOutcome) {
        match self {
            Self::Completed { decision, outcome }
            | Self::Rejected {
                decision, outcome, ..
            } => (decision, outcome),
        }
    }

    fn into_result(self) -> Result<Option<ClientOrderId>> {
        match self {
            Self::Rejected { failure, .. } => Err(failure),
            Self::Completed {
                outcome: ExitAttemptOutcome::Submitted { submitted_order },
                ..
            } => Ok(Some(ClientOrderId::from(
                submitted_order.client_order_id.as_str(),
            ))),
            Self::Completed {
                outcome:
                    ExitAttemptOutcome::Held { .. }
                    | ExitAttemptOutcome::Blocked { .. }
                    | ExitAttemptOutcome::PreparationRejected { .. }
                    | ExitAttemptOutcome::RouteRejected { .. }
                    | ExitAttemptOutcome::IntentEvidenceRejected { .. }
                    | ExitAttemptOutcome::AdmissionRejected { .. }
                    | ExitAttemptOutcome::PolicySkipped { .. }
                    | ExitAttemptOutcome::PreSinkRejected { .. }
                    | ExitAttemptOutcome::SinkRejected { .. },
                ..
            } => Ok(None),
        }
    }

    fn from_submit_state(
        decision: ExitIntentDecision,
        prepared_order: PreparedOrderLinkage,
        state: BoltV3SubmitAttemptState,
    ) -> (ExitAttemptDisposition, Self) {
        match state {
            BoltV3SubmitAttemptState::Submitted(submitted_order) => (
                ExitAttemptDisposition::Submitted,
                Self::completed(decision, ExitAttemptOutcome::Submitted { submitted_order }),
            ),
            BoltV3SubmitAttemptState::PolicySkipped => (
                ExitAttemptDisposition::NonSubmitted,
                Self::completed(
                    decision,
                    ExitAttemptOutcome::PolicySkipped { prepared_order },
                ),
            ),
            BoltV3SubmitAttemptState::RouteValidationRejected(reason) => (
                ExitAttemptDisposition::NonSubmitted,
                Self::rejected(
                    decision,
                    ExitAttemptOutcome::RouteRejected {
                        prepared_order,
                        reason: reason.clone(),
                    },
                    anyhow::anyhow!(
                        "exit submit did not reach the venue: outcome=RouteValidationRejected diagnostic={reason}"
                    ),
                ),
            ),
            BoltV3SubmitAttemptState::IntentEvidenceRejected(reason) => (
                ExitAttemptDisposition::NonSubmitted,
                Self::rejected(
                    decision,
                    ExitAttemptOutcome::IntentEvidenceRejected {
                        prepared_order,
                        reason: reason.clone(),
                    },
                    anyhow::anyhow!(
                        "exit submit did not reach the venue: outcome=IntentEvidenceRejected diagnostic={reason}"
                    ),
                ),
            ),
            BoltV3SubmitAttemptState::AdmissionRejected(reason) => (
                ExitAttemptDisposition::NonSubmitted,
                Self::rejected(
                    decision,
                    ExitAttemptOutcome::AdmissionRejected {
                        prepared_order,
                        reason: reason.clone(),
                    },
                    anyhow::anyhow!(
                        "exit submit did not reach the venue: outcome=AdmissionRejected diagnostic={reason}"
                    ),
                ),
            ),
            BoltV3SubmitAttemptState::PreSinkRejected(reason) => (
                ExitAttemptDisposition::NonSubmitted,
                Self::rejected(
                    decision,
                    ExitAttemptOutcome::PreSinkRejected {
                        prepared_order,
                        reason: reason.clone(),
                    },
                    anyhow::anyhow!(
                        "exit submit did not reach the venue: outcome=PreSinkRejected diagnostic={reason}"
                    ),
                ),
            ),
            BoltV3SubmitAttemptState::SinkRejected(reason) => (
                ExitAttemptDisposition::NonSubmitted,
                Self::rejected(
                    decision,
                    ExitAttemptOutcome::SinkRejected {
                        prepared_order,
                        reason: reason.clone(),
                    },
                    anyhow::anyhow!(
                        "exit submit did not reach the venue: outcome=SinkRejected diagnostic={reason}"
                    ),
                ),
            ),
        }
    }
}

fn non_action_exit_attempt_outcome(decision: &ExitIntentDecision) -> ExitAttemptOutcome {
    match decision.blocked_reason {
        Some(blocked_reason) => ExitAttemptOutcome::Blocked { blocked_reason },
        None => ExitAttemptOutcome::Held {
            outcome: ExitHoldOutcome::Hold,
        },
    }
}

fn rejected_exit_preparation(
    decision: ExitIntentDecision,
    stage: ExitPreparationStage,
    failure: anyhow::Error,
) -> ExitAttemptExecution {
    let reason = format!("{failure:#}");
    ExitAttemptExecution::rejected(
        decision,
        ExitAttemptOutcome::PreparationRejected { stage, reason },
        failure,
    )
}

fn evidence_preparation_stage(
    stage: BoltV3RiskReducingIocPreparationStage,
) -> ExitPreparationStage {
    match stage {
        BoltV3RiskReducingIocPreparationStage::OrderTemplate => ExitPreparationStage::OrderTemplate,
        BoltV3RiskReducingIocPreparationStage::PositionAuthority => {
            ExitPreparationStage::PositionAuthority
        }
        BoltV3RiskReducingIocPreparationStage::ExecutableLiquidity => {
            ExitPreparationStage::ExecutableLiquidity
        }
        BoltV3RiskReducingIocPreparationStage::EconomicsSeal => ExitPreparationStage::EconomicsSeal,
    }
}

#[derive(Clone, Debug)]
struct SettlementCloseFetchAttemptState {
    interval_end_ms: u64,
    attempt_count: u64,
    last_attempt_ms: Option<u64>,
}

#[derive(Clone, Debug)]
struct SelectedReferenceQuoteEvidence {
    quote: ReferenceQuote,
    failed_over: bool,
}

#[derive(Clone, Debug)]
enum EntryRejectState {
    Malformed,
    Balance,
    Unfillable { book: OutcomeBookState },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryRejectClass {
    Malformed,
    Balance,
    Unfillable,
}

impl BinaryOracleEdgeTaker {
    fn new(config: BinaryOracleEdgeTakerConfig, context: StrategyBuildContext) -> Self {
        let pricing = PricingState::from_config(&taker_pricing_config(&config));
        let reference_price_selector = reference_price_selector_from_config(&config);
        let reference_price_source_health = reference_price_source_health_from_config(&config);
        let oms_type = parse_configured_oms_type(CONFIG_FIELD_OMS_TYPE, &config.oms_type)
            .expect("validated binary_oracle_edge_taker oms_type");
        let market_exit_time_in_force = config.forced_exit_order.time_in_force;
        let external_order_claims = config
            .external_order_claims
            .iter()
            .map(|instrument_id| InstrumentId::from(instrument_id.as_str()))
            .collect::<Vec<_>>();
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                order_id_tag: Some(config.order_id_tag.clone()),
                use_uuid_client_order_ids: config.use_uuid_client_order_ids,
                use_hyphens_in_client_order_ids: config.use_hyphens_in_client_order_ids,
                oms_type: Some(oms_type),
                external_order_claims: Some(external_order_claims),
                manage_contingent_orders: config.manage_contingent_orders,
                manage_gtd_expiry: config.manage_gtd_expiry,
                manage_stop: config.manage_stop,
                market_exit_interval_ms: config.market_exit_interval_ms,
                market_exit_max_attempts: config.market_exit_max_attempts,
                market_exit_time_in_force,
                market_exit_reduce_only: config.forced_exit_order.is_reduce_only,
                log_events: config.log_events,
                log_commands: config.log_commands,
                log_rejected_due_post_only_as_warning: config.log_rejected_due_post_only_as_warning,
            }),
            config,
            context,
            active: ActiveMarketState::idle(),
            book_subscriptions: OutcomeBookSubscriptions::empty(),
            market_lifecycle: BTreeMap::new(),
            exposure: ExposureState::Flat,
            next_exit_attempt_generation: 0,
            last_flat_terminal_entry_override: None,
            last_reported_exposure_occupancy: Cell::new(None),
            blocked_strategy_input_novelty: production_novelty_guard(
                EvidenceStateOwner::BlockedStrategyInputSnapshot,
            )
            .expect("blocked strategy-input novelty owner is registered"),
            entry_skip_novelty: production_novelty_guard(EvidenceStateOwner::EntrySkip)
                .expect("entry-skip novelty owner is registered"),
            last_recorded_exit_decision: None,
            pricing,
            latest_signal_quote: None,
            latest_selected_reference_quote: None,
            reference_price_selector,
            reference_price_quotes: BTreeMap::new(),
            reference_price_source_health,
            selection_missing_since_ms: None,
            resolution_report_boundary_subscriptions: BTreeMap::new(),
            resolution_strike_fetch_sequence: INITIAL_COUNTER_U64,
            entry_reject_state: BTreeMap::new(),
            settled_position_keys: BTreeSet::new(),
            settlement_booking_error_keys: BTreeSet::new(),
            terminal_settlement_keys: BTreeSet::new(),
            last_entry_block_reason_sets: None,
            settlement_close_fetch_attempts: BTreeMap::new(),
            last_exit_evidence_outcome: BTreeMap::new(),
            #[cfg(test)]
            book_subscription_events: Vec::new(),
            #[cfg(test)]
            resolution_strike_subscribe_events: Vec::new(),
            #[cfg(test)]
            reference_price_subscribe_events: Vec::new(),
            #[cfg(test)]
            live_input_subscription_retry_events: Vec::new(),
        }
    }

    fn apply_selection_snapshot(&mut self, snapshot: RuntimeSelectionSnapshot) {
        let now_ms = snapshot.published_at_ms;
        let previous_active = self.active.clone();
        let next_selection_books = selection_book_subscriptions(&snapshot);
        apply_selection_snapshot_to_active(
            &mut self.active,
            &snapshot,
            self.config.warmup_tick_count,
        );
        self.active.books.up.instrument_id = next_selection_books.up_instrument_id;
        self.active.books.down.instrument_id = next_selection_books.down_instrument_id;
        self.active.apply_selection_timing(&snapshot);
        if reference_current_price_boundary_changed(&previous_active, &self.active) {
            self.latest_signal_quote = None;
            self.latest_selected_reference_quote = None;
            self.reset_reference_current_price_runtime_state();
        }
        // Bind the live strike to the market's interval-open boundary.
        //
        // Re-issue the strike subscribe whenever the strike is unresolved
        // (`price_to_beat` is `None`) and either a new interval was just selected
        // (the first attempt — for a future "Next" selection the Chainlink report
        // does not exist yet, so the pre-open attempt cannot bind) or wall-clock
        // has reached the interval-open boundary (`now_ms >= interval_start_ms`).
        //
        // This block runs only on the bounded selection-retry cadence (`on_start`
        // once plus the selection-retry timer), never per market-data tick, so
        // retrying on every open tick makes the fetch self-healing — a transient
        // REST failure at the open second no longer strands the strike for the
        // whole interval — without hammering the endpoint. The
        // `price_to_beat.is_none()` guard stops the retries the moment a strike
        // binds.
        if self.active.phase != SelectionPhase::Idle
            && let Some(interval_start_ms) = self.active.interval_start_ms
            && self.active.price_to_beat.is_none()
        {
            let interval_changed =
                self.active.interval_start_ms != previous_active.interval_start_ms;
            let interval_open = now_ms >= interval_start_ms;
            if interval_changed || interval_open {
                self.subscribe_resolution_strike();
            }
        }
        self.apply_reference_price_selection_at(now_ms);
        self.sync_exposure_context_from_active();
        self.reclassify_unreachable_pending_entry_at_selection_boundary(now_ms);
        self.prune_market_lifecycle(now_ms);
        self.refresh_book_subscriptions_for_current_state();
        if self.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                now_ms,
                ExitEvaluationTriggerContext::from_local_selection_handler(LocalReceiveMs::new(
                    now_ms,
                )),
            )
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on selection update: strategy_id={} market_id={:?} now_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                now_ms,
                error,
            );
        }
    }

    pub(super) fn check_resolution_feed_outage_at_market_end(&mut self, now_ms: u64) -> Result<()> {
        let Some(position) = self.settlement_position_candidate() else {
            return Ok(());
        };
        let Some(interval_end_ms) = position.lifecycle.interval_end_ms() else {
            self.record_missing_interval_end_settlement_booking_error(
                &position,
                now_ms.saturating_mul(NANOS_PER_MILLI_U64),
            )?;
            return Ok(());
        };
        if now_ms < interval_end_ms {
            return Ok(());
        }
        let settlement_key = settlement_key_for_position(&position)?;
        if self.settled_position_keys.contains(&settlement_key)
            || self.settlement_booking_error_keys.contains(&settlement_key)
        {
            return Ok(());
        }
        self.retry_settlement_close_fetch_or_record_terminal_failure(
            &position,
            settlement_key,
            interval_end_ms,
            now_ms,
        )
    }

    fn retry_settlement_close_fetch_or_record_terminal_failure(
        &mut self,
        position: &OpenPositionState,
        settlement_key: String,
        interval_end_ms: u64,
        now_ms: u64,
    ) -> Result<()> {
        let retry_budget = self.config.market_exit_max_attempts;
        let retry_interval_ms = self
            .config
            .retry_interval_seconds
            .saturating_mul(MILLIS_PER_SECOND_U64);
        let mut attempt_due = false;
        let attempts_exhausted = {
            let state = self
                .settlement_close_fetch_attempts
                .entry(settlement_key.clone())
                .or_insert(SettlementCloseFetchAttemptState {
                    interval_end_ms,
                    attempt_count: INITIAL_COUNTER_U64,
                    last_attempt_ms: None,
                });
            if state.interval_end_ms != interval_end_ms {
                *state = SettlementCloseFetchAttemptState {
                    interval_end_ms,
                    attempt_count: INITIAL_COUNTER_U64,
                    last_attempt_ms: None,
                };
            }
            let retry_due =
                Self::settlement_close_retry_due(state.last_attempt_ms, now_ms, retry_interval_ms);
            if state.attempt_count >= retry_budget {
                retry_due
            } else {
                attempt_due = retry_due;
                false
            }
        };
        if attempts_exhausted {
            self.record_settlement_booking_error(
                position,
                settlement_key,
                SettlementBookingErrorReason::ResolutionFeedMissing,
                "resolution feed missing after settlement close fetch attempts exhausted; settlement not booked".to_string(),
                now_ms.saturating_mul(NANOS_PER_MILLI_U64),
            )?;
        } else if attempt_due {
            match self.subscribe_resolution_settlement_close(interval_end_ms) {
                ResolutionReportSubscriptionOutcome::Dispatched => {
                    if let Some(state) = self
                        .settlement_close_fetch_attempts
                        .get_mut(&settlement_key)
                    {
                        state.attempt_count =
                            state.attempt_count.saturating_add(COUNTER_INCREMENT_U64);
                        state.last_attempt_ms = Some(now_ms);
                    }
                }
                ResolutionReportSubscriptionOutcome::MissingRoute => {
                    self.record_settlement_booking_error(
                        position,
                        settlement_key,
                        SettlementBookingErrorReason::ResolutionFeedMissing,
                        "resolution feed route unavailable for settlement close fetch; settlement not booked".to_string(),
                        now_ms.saturating_mul(NANOS_PER_MILLI_U64),
                    )?;
                }
                ResolutionReportSubscriptionOutcome::AssetBindingRejected => {
                    self.record_settlement_booking_error(
                        position,
                        settlement_key,
                        SettlementBookingErrorReason::ResolutionFeedMissing,
                        "resolution feed asset binding rejected settlement close fetch; settlement not booked".to_string(),
                        now_ms.saturating_mul(NANOS_PER_MILLI_U64),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn settlement_close_retry_due(
        last_attempt_ms: Option<u64>,
        now_ms: u64,
        retry_interval_ms: u64,
    ) -> bool {
        last_attempt_ms.is_none_or(|last_attempt_ms| {
            now_ms.saturating_sub(last_attempt_ms) >= retry_interval_ms
        })
    }

    fn try_book_resolution_settlement(&mut self, update: &IndexPriceUpdate) -> Result<()> {
        let Some(position) = self.settlement_position_candidate() else {
            return Ok(());
        };
        let settlement_key = settlement_key_for_position(&position)?;
        let position_key = settlement_position_key(&position, settlement_key);
        let outcome_side = position.lifecycle.outcome_side();
        let lot = outcome_side.map(|outcome_side| {
            crate::bolt_v3_binary_settlement::BinarySettlementLot {
                leg: settlement_leg_for_outcome(outcome_side),
                side: QuoteSide::Buy,
                quantity: position.quantity.as_f64(),
                entry_price: position.avg_px_open,
            }
        });
        let decision = try_book_resolution_settlement(
            ResolutionSettlementInput {
                position: &position_key,
                resolution_ts_ns: update.ts_event.as_u64(),
                family_key: self.config.rotating_market_family.as_str(),
                reference_close_price: update.value.as_f64(),
                strike_price: position.lifecycle.settlement_strike(),
                lot,
                market_id_present: position.lifecycle.market_id().is_some(),
                capability: self.context.settlement_capability(),
            },
            &self.settled_position_keys,
            &self.settlement_booking_error_keys,
        );
        let booking = match decision {
            ResolutionSettlementDecision::Skip(_) => return Ok(()),
            ResolutionSettlementDecision::BookingError { reason, detail } => {
                self.record_settlement_booking_error(
                    &position,
                    position_key.settlement_key,
                    reason,
                    detail,
                    update.ts_event.as_u64(),
                )?;
                return Ok(());
            }
            ResolutionSettlementDecision::Book(booking) => booking,
        };
        let outcome_side = outcome_side.expect("shared booking requires an outcome-side lot");
        let strike_price = position
            .lifecycle
            .settlement_strike()
            .expect("shared booking requires a settlement strike");
        let settlement_currency = self
            .context
            .settlement_currency()
            .expect("shared booking validates settlement currency");
        let market_id = position
            .lifecycle
            .market_id_owned()
            .expect("shared booking validates market id");
        let product_id = settlement_product_id(position.instrument_id)?;
        let evidence = self.settlement_evidence(
            &position,
            SettlementEvidenceIds {
                settlement_key: booking.settlement_key.clone(),
                market_id,
                product_id,
            },
            update,
            settlement_currency,
            SettlementEvidenceComputation {
                outcome_side,
                strike_price,
                payout_per_share: booking.payout_per_share,
                terminal_value: booking.result.terminal_value,
                realized_pnl: booking.result.realized_pnl,
            },
        );
        let committed = self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_settlement(evidence)?;
        self.settled_position_keys
            .insert(booking.settlement_key.clone());
        if booking.key_delta.remove_close_fetch_attempt {
            self.settlement_close_fetch_attempts
                .remove(&booking.key_delta.settlement_key);
        }
        if let Err(error) =
            self.apply_committed_settlement_runtime_effects(committed, settlement_currency)
        {
            self.enter_blind_settlement_recovery(error);
            return Ok(());
        }
        self.exposure = ExposureState::Flat;
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
        Ok(())
    }

    fn apply_committed_settlement_runtime_effects(
        &self,
        committed: crate::bolt_v3_current_evidence::CommittedSettlement,
        settlement_currency: Currency,
    ) -> Result<()> {
        self.apply_settlement_runtime_effects(committed.fact(), settlement_currency)
    }

    fn apply_settlement_runtime_effects(
        &self,
        evidence: &SettlementFact,
        settlement_currency: Currency,
    ) -> Result<()> {
        let Some(sink) = self.context.settlement_runtime_sink() else {
            return Ok(());
        };
        let account_id = self
            .context
            .settlement_account_id()
            .expect("shared booking validates settlement account id");
        sink.record_loss_governor_position_realized_pnl(
            settlement_position_realized_pnl_observation(
                account_id,
                evidence,
                settlement_currency,
            )?,
        )
    }

    fn record_settlement_booking_error(
        &mut self,
        position: &OpenPositionState,
        settlement_key: String,
        reason: SettlementBookingErrorReason,
        detail: String,
        observed_at_ns: u64,
    ) -> Result<()> {
        let origin = if position.lifecycle.interval_end_ms().is_none()
            && self.managed_position().is_some_and(|managed| {
                managed.origin == ManagedPositionOrigin::RecoveryBootstrap
                    && managed.position.position_id == position.position_id
            }) {
            SettlementPositionOrigin::RecoveryBootstrap
        } else {
            SettlementPositionOrigin::Live
        };
        let position_key = settlement_position_key(position, settlement_key);
        let Some(transition) = decide_settlement_booking_error(
            &position_key,
            origin,
            reason,
            detail,
            observed_at_ns,
            &self.settlement_booking_error_keys,
        )?
        else {
            return Ok(());
        };
        let evidence = self.settlement_booking_error_evidence(
            position,
            transition.eligibility.settlement_key.clone(),
            transition.reason,
            transition.detail,
            observed_at_ns,
        );
        let reason_detail = format!("reason={reason:?} detail={}", evidence.detail);
        self.apply_terminal_settlement_transition(
            position,
            transition.eligibility,
            transition.key_delta,
            evidence,
            reason_detail,
            TerminalEvidenceState::PersistCanonical,
        )
    }

    /// The only terminal-settlement transition. Eligibility is already encoded by
    /// [`TerminalSettlementEligibility`], so no caller can release a pending,
    /// live-manageable, or nonterminal position. The transition first ensures
    /// canonical durable evidence, then releases exposure, and finally attempts
    /// the fallible operator-health mutation/report.
    fn apply_terminal_settlement_transition(
        &mut self,
        position: &OpenPositionState,
        eligibility: TerminalSettlementEligibility,
        key_delta: SettlementTerminalKeyDelta,
        booking_error: SettlementBookingErrorFact,
        reason_detail: String,
        evidence_state: TerminalEvidenceState,
    ) -> Result<()> {
        let settlement_key = eligibility.settlement_key.clone();
        let health_emitter = self.context.settlement_health_transition_emitter().cloned();
        let lifecycle = self.order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition: OrderLifecycleTransition::SettlementBookingTerminal,
            outcome: OrderLifecycleOutcome::Flat,
            source: ORDER_LIFECYCLE_SOURCE_SETTLEMENT_BOOKING_TERMINAL,
            market_id: position.lifecycle.market_id_owned(),
            instrument_id: Some(position.instrument_id),
            position_id: Some(position.position_id),
            client_order_id: None,
            prior_client_order_id: None,
            raw_reason_text: Some(format!(
                "settlement_booking_terminal eligibility={} {reason_detail}",
                eligibility.reason.label()
            )),
            order_side: Some(position.entry_order_side),
            filled_quantity: None,
            residual_quantity: Some(position.quantity),
            ts_event_ns: Some(eligibility.observed_at_ns),
        });
        let terminal_evidence = TerminalSettlementFact {
            settlement_key: settlement_key.clone(),
            booking_error,
            lifecycle,
        };
        if evidence_state == TerminalEvidenceState::PersistCanonical {
            self.context
                .edge_taker_evidence()
                .expect("edge-taker strategy must own edge-taker evidence")
                .record_terminal_settlement(terminal_evidence)
                .context("failed to persist canonical terminal settlement evidence")?;
        }
        if key_delta.insert_terminal_key {
            self.terminal_settlement_keys.insert(settlement_key.clone());
        }
        if key_delta.insert_booking_error_key {
            self.settlement_booking_error_keys
                .insert(settlement_key.clone());
        }
        if key_delta.remove_close_fetch_attempt {
            self.settlement_close_fetch_attempts.remove(&settlement_key);
        }
        log::error!(
            "binary_oracle_edge_taker settlement booking terminal: strategy_id={} position_id={} instrument_id={} eligibility={} {reason_detail}",
            self.config.strategy_id,
            position.position_id,
            position.instrument_id,
            eligibility.reason.label(),
        );
        self.exposure = ExposureState::Flat;
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
        let health_transition = BoltV3SettlementHealthTransition {
            settlement_key: settlement_key.clone(),
            position_id: position.position_id.to_string(),
            reason: eligibility.reason.label().to_string(),
        };
        let health_result = health_emitter
            .context("terminal settlement health emitter is not configured")
            .and_then(|emitter| {
                emitter(health_transition).context("terminal settlement health emission failed")
            });
        if let Err(error) = health_result {
            log::error!(
                "binary_oracle_edge_taker terminal health reporting failed after durable release: strategy_id={} settlement_key={} position_id={} error={error:#}",
                self.config.strategy_id,
                settlement_key,
                position.position_id,
            );
        }
        Ok(())
    }

    fn record_missing_interval_end_settlement_booking_error(
        &mut self,
        position: &OpenPositionState,
        observed_at_ns: u64,
    ) -> Result<()> {
        if position.lifecycle.interval_end_ms().is_some() {
            return Ok(());
        }
        let settlement_key = settlement_key_for_position(position)?;
        if self.settled_position_keys.contains(&settlement_key)
            || self.settlement_booking_error_keys.contains(&settlement_key)
        {
            return Ok(());
        }
        self.record_settlement_booking_error(
            position,
            settlement_key,
            SettlementBookingErrorReason::SettlementInputInvalid,
            "settlement input missing interval end".to_string(),
            observed_at_ns,
        )
    }

    fn settlement_evidence(
        &self,
        position: &OpenPositionState,
        ids: SettlementEvidenceIds,
        update: &IndexPriceUpdate,
        settlement_currency: Currency,
        computation: SettlementEvidenceComputation,
    ) -> SettlementFact {
        SettlementFact {
            strategy_id: self.config.strategy_id.clone(),
            settlement_key: ids.settlement_key,
            market_id: ids.market_id,
            position_id: position.position_id.to_string(),
            instrument_id: position.instrument_id.to_string(),
            product_id: ids.product_id,
            outcome_side: outcome_side_to_evidence(computation.outcome_side),
            entry_order_side: evidence_order_side(position.entry_order_side),
            quantity: position.quantity.to_string(),
            entry_price: evidence_number(position.avg_px_open),
            family_key: self.config.rotating_market_family.clone(),
            strike_price: evidence_number(computation.strike_price),
            resolution_instrument_id: update.instrument_id.to_string(),
            resolution_ts_event_ns: update.ts_event.as_u64(),
            reference_close_price: evidence_number(update.value.as_f64()),
            payout_per_share: evidence_number(computation.payout_per_share),
            terminal_value: evidence_number(computation.terminal_value),
            realized_pnl: evidence_number(computation.realized_pnl),
            settlement_currency: settlement_currency.code.as_str().to_string(),
        }
    }

    fn settlement_booking_error_evidence(
        &self,
        position: &OpenPositionState,
        settlement_key: String,
        reason: SettlementBookingErrorReason,
        detail: String,
        observed_at_ns: u64,
    ) -> SettlementBookingErrorFact {
        SettlementBookingErrorFact {
            strategy_id: self.config.strategy_id.clone(),
            settlement_key,
            market_id: position.lifecycle.market_id_owned(),
            position_id: Some(position.position_id.to_string()),
            instrument_id: Some(position.instrument_id.to_string()),
            resolution_instrument_id: self
                .resolution_instrument_id()
                .map(|instrument_id| instrument_id.to_string()),
            reason,
            detail,
            observed_at_ns,
        }
    }

    fn settlement_position_candidate(&self) -> Option<OpenPositionState> {
        self.managed_position().map(|managed| managed.position)
    }

    fn observe_signal_quote(
        &mut self,
        quote: &FastSpotObservation,
        lifecycle_now_ms: u64,
        evaluation_receive_ms: LocalReceiveMs,
    ) {
        self.latest_signal_quote = Some(quote.clone());
        self.pricing
            .observe_signal_quote(quote, &taker_pricing_config(&self.config));
        self.after_signal_quote_observed(
            lifecycle_now_ms,
            quote.observed_ts_ms,
            evaluation_receive_ms,
        );
    }

    fn observe_invalid_signal_quote(
        &mut self,
        venue: &str,
        observed_ts_ms: u64,
        lifecycle_now_ms: u64,
        evaluation_receive_ms: LocalReceiveMs,
    ) {
        self.latest_signal_quote = None;
        self.pricing
            .observe_invalid_signal_quote(venue, observed_ts_ms);
        self.after_signal_quote_observed(lifecycle_now_ms, observed_ts_ms, evaluation_receive_ms);
    }

    fn after_signal_quote_observed(
        &mut self,
        lifecycle_now_ms: u64,
        observed_ts_ms: u64,
        evaluation_receive_ms: LocalReceiveMs,
    ) {
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        self.sync_exposure_context_from_active();
        if self.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                lifecycle_now_ms,
                ExitEvaluationTriggerContext::from_market_data(
                    EvidenceExitTriggerSource::SignalQuote,
                    observed_ts_ms,
                    evaluation_receive_ms,
                ),
            )
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on signal update: strategy_id={} market_id={:?} ts_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                observed_ts_ms,
                error,
            );
        }
    }

    #[cfg(test)]
    fn observe_reference_snapshot(
        &mut self,
        snapshot: &ReferenceSnapshot,
        receive_ms: LocalReceiveMs,
    ) {
        self.active.observe_reference_snapshot(snapshot);
        self.pricing.observe_reference_snapshot(
            snapshot,
            self.config.lead_agreement_min_corr,
            self.config.lead_jitter_max_ms,
        );
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        self.sync_exposure_context_from_active();
        if self.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                receive_ms.value(),
                ExitEvaluationTriggerContext::from_market_data(
                    EvidenceExitTriggerSource::ReferenceUpdate,
                    snapshot.ts_ms,
                    receive_ms,
                ),
            )
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on reference update: strategy_id={} market_id={:?} ts_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                snapshot.ts_ms,
                error,
            );
        }
    }

    fn signal_quote_from_tick(
        &self,
        quote: &QuoteTick,
        receive_ms: LocalReceiveMs,
    ) -> Option<FastSpotObservation> {
        let bid = quote.bid_price.as_f64();
        let ask = quote.ask_price.as_f64();
        if !is_positive_finite(bid) || !is_positive_finite(ask) {
            return None;
        }
        let midpoint = (bid + ask) / MIDPOINT_DIVISOR_F64;
        if !is_positive_finite(midpoint) {
            return None;
        }
        let observed_ts_ms = quote.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        let venue_name = self.config.signal_venue.as_ref()?;
        Some(FastSpotObservation {
            venue: venue_name.clone(),
            price: midpoint,
            observed_ts_ms,
            received_ts_ms: Some(receive_ms.value()),
        })
    }

    fn refresh_realized_volatility_snapshot_at(&mut self, now_ms: u64) {
        if let Some(snapshot) = self.context.refresh_realized_volatility_snapshot_at(
            &self.config.realized_volatility_surface_id,
            now_ms,
        ) {
            self.pricing.observe_realized_vol_snapshot(snapshot);
        }
    }

    fn selection_retry_timer_name(&self) -> String {
        format!("{}:selection_retry", self.config.strategy_id)
    }

    fn register_selection_retry_timer(&mut self) {
        let timer_name = self.selection_retry_timer_name();
        let strategy_id = self.config.strategy_id.clone();
        let interval_ns = self
            .config
            .retry_interval_seconds
            .saturating_mul(NANOS_PER_SECOND_U64);
        if let Err(error) =
            self.clock()
                .set_timer_ns(&timer_name, interval_ns, None, None, None, None, None)
        {
            log::error!(
                "binary_oracle_edge_taker selection retry timer registration failed: strategy_id={} error={:#}",
                strategy_id,
                error,
            );
        }
    }

    fn deregister_selection_retry_timer(&mut self) {
        let timer_name = self.selection_retry_timer_name();
        self.clock().cancel_timer(timer_name.as_str());
        self.replace_book_subscriptions(OutcomeBookSubscriptions::empty());
    }

    fn refresh_selection_from_cache(&mut self, now_ms: u64) {
        // Scope market selection to the configured execution venue. The shared NT cache can hold
        // instruments from EVERY registered data client (the shipped config registers several
        // non-Polymarket reference venues, and `load_state` can repopulate the cache across runs),
        // so an unscoped read could feed a foreign-venue instrument into selection. A real order can
        // only ever route to the execution client's venue, so any market on another venue must be
        // unselectable here. Filtering the cache read by the execution venue makes a wrong-venue
        // selection structurally impossible and fails closed.
        let execution_venue = self.context.execution_venue();
        let instruments = {
            let cache = self.cache();
            cache
                .instrument_ids(None)
                .into_iter()
                .filter_map(|instrument_id| cache.instrument(&instrument_id))
                .filter(|instrument| instrument.id().venue == execution_venue)
                .collect::<Vec<_>>()
        };
        let snapshot = selection_snapshot_from_instruments(&self.config, &instruments, now_ms);
        // Defense in depth: the venue-scoped read above cannot yield a foreign-venue market, but
        // assert it explicitly so any future widening of the read still fails closed here — a real
        // order must never fire against a market whose outcome instruments are on a venue other than
        // the execution venue.
        let snapshot = if selected_market_on_execution_venue(&snapshot, execution_venue) {
            snapshot
        } else {
            log::error!(
                "binary_oracle_edge_taker refusing a selected market whose outcome venue is not the execution venue: strategy_id={} execution_venue={}",
                self.config.strategy_id,
                execution_venue,
            );
            idle_selection_snapshot(
                &self.config,
                now_ms,
                SELECTION_BLOCK_REASON_TARGET_SELECTION_BLOCKED,
            )
        };
        if matches!(snapshot.decision.state, SelectionState::Idle { .. }) {
            if self.selection_missing_since_ms.is_none() {
                self.selection_missing_since_ms = Some(now_ms);
            }
            let missing_since_ms = self
                .selection_missing_since_ms
                .expect("selection_missing_since_ms set before blocked-target check");
            let blocked_after_ms = self
                .config
                .blocked_after_seconds
                .saturating_mul(MILLIS_PER_SECOND_U64);
            if now_ms.saturating_sub(missing_since_ms) >= blocked_after_ms {
                self.apply_selection_snapshot(idle_selection_snapshot(
                    &self.config,
                    now_ms,
                    SELECTION_BLOCK_REASON_TARGET_SELECTION_BLOCKED,
                ));
                return;
            }
        } else {
            self.selection_missing_since_ms = None;
        }
        self.apply_selection_snapshot(snapshot);
    }

    fn apply_reference_price_update(&mut self, update: &ReferencePriceUpdate) {
        self.initialize_reference_price_runtime_state();
        let Some(reference_price) = self.config.reference_current_price.clone() else {
            return;
        };
        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        let window = self
            .active
            .interval_start_ms
            .zip(self.active.interval_end_ms)
            .map(
                |(interval_start_ms, interval_end_ms)| ReferencePriceLiveWindow {
                    interval_start_ms,
                    interval_end_ms,
                    evaluation_now_ms: now_ms,
                },
            );
        let observation = observe_reference_price_health_update(
            &reference_price,
            update,
            window,
            &mut self.reference_price_selector,
            &mut self.reference_price_source_health,
            &mut self.reference_price_quotes,
        );
        self.log_reference_price_rejection(update, now_ms, observation.rejection.as_ref());
        self.apply_reference_price_observation(observation);
    }

    fn log_reference_price_rejection(
        &self,
        update: &ReferencePriceUpdate,
        now_ms: u64,
        rejection: Option<&ReferencePriceUpdateRejection>,
    ) {
        match rejection {
            Some(ReferencePriceUpdateRejection::MalformedFrame { detail }) => log::warn!(
                "binary_oracle_edge_taker malformed reference price update ignored: {detail}; source_id={} strategy_id={}",
                update.source_id(),
                self.config.strategy_id,
            ),
            Some(ReferencePriceUpdateRejection::ProviderMismatch { expected, actual }) => {
                log::warn!(
                    "binary_oracle_edge_taker reference current price provider mismatch ignored: source_id={} expected_provider={} actual_provider={} strategy_id={}",
                    update.source_id(),
                    expected,
                    actual,
                    self.config.strategy_id,
                )
            }
            Some(ReferencePriceUpdateRejection::ProviderInstrumentMismatch {
                expected,
                actual,
            }) => log::warn!(
                "binary_oracle_edge_taker reference current price provider instrument mismatch ignored: source_id={} expected_provider_instrument={} actual_provider_instrument={} strategy_id={}",
                update.source_id(),
                expected,
                actual,
                self.config.strategy_id,
            ),
            Some(ReferencePriceUpdateRejection::OutsideLiveWindow) => log::warn!(
                "binary_oracle_edge_taker stale reference current price ignored: source_id={} observed_ts_ms={} now_ms={} strategy_id={}",
                update.source_id(),
                update.observed_ts_ms(),
                now_ms,
                self.config.strategy_id,
            ),
            _ => {}
        }
    }

    fn apply_reference_price_observation(&mut self, observation: ReferencePriceUpdateObservation) {
        if !observation.selection_evaluated {
            return;
        }
        let (Some(selection), Some(selected_quote)) =
            (observation.selection, observation.selected_quote)
        else {
            self.clear_reference_current_price_selection_state();
            return;
        };
        if self
            .active
            .observe_reference_price_quote(&selected_quote, selection.failed_over())
        {
            self.latest_selected_reference_quote = Some(SelectedReferenceQuoteEvidence {
                quote: selected_quote.clone(),
                failed_over: selection.failed_over(),
            });
            self.pricing
                .observe_reference_current_price(&FastSpotObservation {
                    venue: selected_quote.source_id().to_string(),
                    price: selected_quote.price(),
                    observed_ts_ms: selected_quote.observed_ts_ms(),
                    received_ts_ms: Some(selected_quote.received_ts_ms()),
                });
            self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        }
    }

    fn apply_current_reference_price_selection(
        &mut self,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
    ) {
        let Some(reference_price) = self.config.reference_current_price.clone() else {
            return;
        };
        let observation = select_reference_price_from_health(
            &reference_price,
            &mut self.reference_price_selector,
            &mut self.reference_price_source_health,
            &self.reference_price_quotes,
            ReferencePriceLiveWindow {
                interval_start_ms,
                interval_end_ms,
                evaluation_now_ms: now_ms,
            },
        );
        self.apply_reference_price_observation(observation);
    }

    fn apply_reference_price_selection_at(&mut self, now_ms: u64) {
        if let (Some(interval_start_ms), Some(interval_end_ms)) =
            (self.active.interval_start_ms, self.active.interval_end_ms)
        {
            self.apply_current_reference_price_selection(
                interval_start_ms,
                interval_end_ms,
                now_ms,
            );
        }
    }

    fn initialize_reference_price_runtime_state(&mut self) {
        if self.config.reference_current_price.is_none() {
            return;
        }
        if self.reference_price_selector.is_none() {
            self.reference_price_selector = reference_price_selector_from_config(&self.config);
        }
        if self.reference_price_source_health.is_empty() {
            self.reference_price_source_health =
                reference_price_source_health_from_config(&self.config);
        }
    }

    fn clear_reference_current_price_selection_state(&mut self) {
        self.active.clear_reference_price_quote();
        self.pricing.clear_reference_current_price_state();
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
    }

    fn reset_reference_current_price_selection_state(&mut self) {
        self.active.reset_reference_price_quote();
        self.pricing.clear_reference_current_price_state();
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
    }

    fn reset_reference_current_price_runtime_state(&mut self) {
        self.reference_price_quotes.clear();
        self.reference_price_source_health =
            reference_price_source_health_from_config(&self.config);
        self.reference_price_selector = reference_price_selector_from_config(&self.config);
        self.reset_reference_current_price_selection_state();
    }

    /// The episode every registered evidence producer keys on.
    ///
    /// Built only from identity the market itself fixes -- strategy, configured
    /// target, execution venue, and the market's own gamma/condition/question
    /// ids plus its ordered outcome pair. Nothing here changes while the market
    /// does not, which is what makes an episode survive the input churn that
    /// used to reopen it.
    fn evidence_episode_id(&self) -> Result<EvidenceEpisodeId> {
        let identity =
            self.active.evidence_identity.as_ref().ok_or_else(|| {
                anyhow::anyhow!("evidence episode requires a bound market identity")
            })?;
        EvidenceEpisodeId::try_from(EvidenceEpisodeParts {
            strategy_id: self.config.strategy_id.clone(),
            target_id: self.config.configured_target_id.to_string(),
            venue_id: self.context.execution_venue().to_string(),
            market: identity.market().clone(),
        })
    }

    fn current_market_id(&self) -> Option<&str> {
        self.active.market_id.as_deref()
    }

    fn tracked_observed_position(&self) -> Option<OpenPositionState> {
        let context = self.exposure.tracked_position_context()?.clone();
        let spec = self
            .nt_open_position_projection(context.position_id)
            .ok()
            .flatten()?;
        open_position_from_nt_projection(context, spec)
    }

    fn managed_position(&self) -> Option<ManagedPositionState> {
        let context = self.exposure.managed_position_context()?.clone();
        let spec = self
            .nt_open_position_projection(context.position_id)
            .ok()
            .flatten()?;
        let origin = context.origin;
        let pending_entry = context.pending_entry.clone();
        Some(ManagedPositionState {
            position: open_position_from_nt_projection(context, spec)?,
            origin,
            pending_entry,
        })
    }

    fn tracked_position_context_mut(
        &mut self,
    ) -> Option<(
        &mut BoltV3PositionMarketLifecycle,
        InstrumentId,
        &mut OutcomeBookState,
    )> {
        let context = self.exposure.tracked_position_context_mut()?;
        Some((
            &mut context.lifecycle,
            context.instrument_id,
            &mut context.book,
        ))
    }

    fn pending_entry(&self) -> Option<&PendingEntryState> {
        self.exposure.pending_entry()
    }

    fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        self.exposure.pending_entry_mut()
    }

    fn entry_order_may_remain_working(&self, client_order_id: &ClientOrderId) -> bool {
        if !matches!(
            self.config.entry_order.time_in_force,
            TimeInForce::Gtc | TimeInForce::Gtd
        ) {
            return false;
        }

        let cached_closed = if self.is_registered() {
            self.cache()
                .order(client_order_id)
                .map(|order| order.is_closed())
        } else {
            None
        };

        match cached_closed {
            Some(closed) => !closed,
            None => true,
        }
    }

    fn clear_managed_pending_entry_for_client_order(
        &mut self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) {
        let matches_pending_entry = self
            .exposure
            .managed_position_context()
            .and_then(|managed| managed.pending_entry.as_ref())
            .is_some_and(|pending| pending.client_order_id == client_order_id);
        if !matches_pending_entry {
            return;
        }
        if !self.event_instrument_matches_held_exposure(event_instrument_id) {
            return;
        }
        if let Some(managed) = self.exposure.managed_position_context_mut() {
            managed.pending_entry = None;
            self.prune_market_lifecycle_at_current_time();
        }
    }

    fn clear_pending_entry_for_client_order(
        &mut self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) {
        let matches_pending_entry = matches!(
            &self.exposure,
            ExposureState::PendingEntry(pending) if pending.client_order_id == client_order_id
        );
        if matches_pending_entry {
            if !self.event_instrument_matches_held_exposure(event_instrument_id) {
                return;
            }
            self.exposure = ExposureState::Flat;
            self.prune_market_lifecycle_at_current_time();
            return;
        }

        self.clear_managed_pending_entry_for_client_order(client_order_id, event_instrument_id);
    }

    fn matching_pending_entry_snapshot(
        &self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) -> Option<PendingEntryState> {
        let pending = self.pending_entry()?.clone();
        (pending.client_order_id == client_order_id && pending.instrument_id == event_instrument_id)
            .then_some(pending)
    }

    fn matching_entry_reconcile_snapshot(
        &self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) -> Option<(PendingEntryState, bool)> {
        match &self.exposure {
            ExposureState::EntryReconcilePending { pending, reason }
                if pending.client_order_id == client_order_id
                    && pending.instrument_id == event_instrument_id =>
            {
                Some((
                    pending.clone(),
                    !matches!(reason, EntryReconcileReason::UnresolvedAtSelectionBoundary),
                ))
            }
            _ => None,
        }
    }

    fn remember_flat_terminal_entry_override(&mut self, pending: &PendingEntryState) {
        self.last_flat_terminal_entry_override = Some(FlatTerminalEntryOverride {
            client_order_id: pending.client_order_id,
            market_id: pending.lifecycle.market_id_owned(),
            instrument_id: pending.instrument_id,
        });
    }

    fn take_position_truth_rematerialization_override(
        &mut self,
        instrument_id: InstrumentId,
        origin: ManagedPositionOrigin,
    ) -> Option<FlatTerminalEntryOverride> {
        if origin != ManagedPositionOrigin::RecoveryBootstrap {
            self.last_flat_terminal_entry_override = None;
            return None;
        }
        if !matches!(self.exposure, ExposureState::Flat) {
            return None;
        }
        self.last_flat_terminal_entry_override
            .take()
            .filter(|terminal_override| terminal_override.instrument_id == instrument_id)
    }

    fn record_pending_entry_terminal_evidence(&self, input: PendingEntryTerminalEvidenceInput) {
        self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition: input.transition,
            outcome: input.outcome,
            source: input.source,
            market_id: input.pending.lifecycle.market_id_owned(),
            instrument_id: Some(input.pending.instrument_id),
            position_id: None,
            client_order_id: Some(input.pending.client_order_id),
            prior_client_order_id: None,
            raw_reason_text: input.raw_reason_text,
            order_side: None,
            filled_quantity: input.filled_quantity,
            residual_quantity: None,
            ts_event_ns: Some(input.ts_event_ns),
        });
    }

    fn resolve_pending_entry_terminal_event(&mut self, input: PendingEntryTerminalEventInput) {
        if let Some((pending, fill_observed)) =
            self.matching_entry_reconcile_snapshot(input.client_order_id, input.event_instrument_id)
        {
            if fill_observed && !input.terminal_proves_zero_fill {
                self.record_pending_entry_terminal_evidence(PendingEntryTerminalEvidenceInput {
                    pending,
                    transition: input.transition,
                    outcome: OrderLifecycleOutcome::EntryReconcilePending,
                    source: input.source,
                    raw_reason_text: Some(
                        ENTRY_RECONCILE_FILL_OBSERVED_TERMINAL_REASON.to_string(),
                    ),
                    filled_quantity: None,
                    ts_event_ns: input.ts_event_ns,
                });
                return;
            }

            // Zero-fill terminal feedback closes this accepted-entry loop. If later
            // provider-allowance position events contradict it, materialization re-manages
            // from that position truth instead of trusting the old pending order.
            self.remember_flat_terminal_entry_override(&pending);
            self.exposure = ExposureState::Flat;
            self.prune_market_lifecycle_at_current_time();
            self.record_pending_entry_terminal_evidence(PendingEntryTerminalEvidenceInput {
                pending,
                transition: input.transition,
                outcome: OrderLifecycleOutcome::Flat,
                source: input.source,
                raw_reason_text: input.raw_reason_text,
                filled_quantity: None,
                ts_event_ns: input.ts_event_ns,
            });
            return;
        }

        let pending_entry =
            self.matching_pending_entry_snapshot(input.client_order_id, input.event_instrument_id);
        self.clear_pending_entry_for_client_order(input.client_order_id, input.event_instrument_id);
        if let Some(pending_entry) = pending_entry {
            if matches!(self.exposure, ExposureState::Flat) {
                self.remember_flat_terminal_entry_override(&pending_entry);
            }
            self.record_pending_entry_terminal_evidence(PendingEntryTerminalEvidenceInput {
                pending: pending_entry,
                transition: input.transition,
                outcome: Self::lifecycle_outcome_for_exposure(&self.exposure),
                source: input.source,
                raw_reason_text: input.raw_reason_text,
                filled_quantity: None,
                ts_event_ns: input.ts_event_ns,
            });
        }
    }

    fn prune_market_lifecycle_at_current_time(&mut self) {
        if self.is_registered() {
            let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
            self.prune_market_lifecycle(now_ms);
        }
    }

    fn set_unsupported_observed_exposure(
        &mut self,
        observed: OpenPositionState,
        reason: UnsupportedObservedReason,
    ) {
        let pending_entry = self.pending_entry().cloned();
        let origin = if pending_entry.is_some() {
            ManagedPositionOrigin::StrategyEntry
        } else {
            ManagedPositionOrigin::RecoveryBootstrap
        };
        self.exposure = ExposureState::UnsupportedObserved(UnsupportedObservedState {
            context: managed_position_context(observed, origin, pending_entry),
            reason,
        });
        self.refresh_book_subscriptions_for_current_state();
    }

    fn bootstrap_recovery_from_cache(&mut self) {
        let observed_at_ns = self.clock().timestamp_ns().as_u64();
        // Scope recovery to the configured execution venue. The shared NT cache can hold positions
        // from every registered execution client; a foreign-venue position must never be accepted
        // into Managed state because the exit path would build/submit an order for it with no
        // additional venue gate. Filtering the cache read by execution venue makes a wrong-venue
        // recovery structurally impossible and fails closed.
        let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
        let execution_venue = self.context.execution_venue();
        let cached_recovery = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let cache = self.cache();
            let cached_positions = cache
                .positions_open(Some(&execution_venue), None, Some(&strategy_id), None, None)
                .into_iter()
                .map(|position| {
                    let lifecycle = BoltV3PositionMarketLifecycle::recover_from_instrument(
                        cache.instrument(&position.instrument_id).as_ref(),
                    );
                    OpenPositionState {
                        lifecycle,
                        instrument_id: position.instrument_id,
                        position_id: position.id,
                        entry_order_side: position.entry,
                        side: position.side,
                        quantity: position.quantity,
                        avg_px_open: position.avg_px_open,
                        book: OutcomeBookState::from_instrument_id(position.instrument_id),
                    }
                })
                .collect::<Vec<_>>();
            let mut settlement_scope_positions = cached_positions.clone();
            settlement_scope_positions.extend(
                cache
                    .positions_closed(Some(&execution_venue), None, Some(&strategy_id), None, None)
                    .into_iter()
                    .map(|position| {
                        let lifecycle = BoltV3PositionMarketLifecycle::recover_from_instrument(
                            cache.instrument(&position.instrument_id).as_ref(),
                        );
                        OpenPositionState {
                            lifecycle,
                            instrument_id: position.instrument_id,
                            position_id: position.id,
                            entry_order_side: position.entry,
                            side: position.side,
                            quantity: position.quantity,
                            avg_px_open: position.avg_px_open,
                            book: OutcomeBookState::from_instrument_id(position.instrument_id),
                        }
                    }),
            );
            (cached_positions, settlement_scope_positions)
        }));

        let (cached_positions, settlement_scope_positions) = match cached_recovery {
            Ok(cached_recovery) => cached_recovery,
            Err(_) => {
                let cache_probe_decision = decide_bootstrap_recovery_from_cache(
                    false,
                    None,
                    usize::default(),
                    observed_at_ns,
                    &self.settlement_booking_error_keys,
                    &self.terminal_settlement_keys,
                );
                debug_assert!(matches!(
                    cache_probe_decision,
                    SettlementRecoveryEntryDecision::EnterBlindCacheProbe
                ));
                self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::CacheProbeFailed,
                });
                log::warn!(
                    "binary_oracle_edge_taker recovery probe could not access cache: strategy_id={} entering fail-closed recovery mode",
                    self.config.strategy_id
                );
                return;
            }
        };

        if !self.recover_settlement_bootstrap_from_scope(&settlement_scope_positions) {
            return;
        }

        let open_position_count = cached_positions.len();
        if let SettlementRecoveryEntryDecision::Flat = decide_bootstrap_recovery_from_cache(
            true,
            None,
            open_position_count,
            observed_at_ns,
            &self.settlement_booking_error_keys,
            &self.terminal_settlement_keys,
        ) {
            self.exposure = ExposureState::Flat;
            return;
        }

        if let SettlementRecoveryEntryDecision::EnterBlindMultipleOpenPositions { count } =
            decide_bootstrap_recovery_from_cache(
                true,
                None,
                open_position_count,
                observed_at_ns,
                &self.settlement_booking_error_keys,
                &self.terminal_settlement_keys,
            )
        {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::MultipleOpenPositions { count },
            });
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap found multiple open positions: strategy_id={} position_count={} leaving recovery mode blind to position bootstrap",
                self.config.strategy_id,
                count,
            );
            return;
        }

        let open_position = cached_positions
            .into_iter()
            .next()
            .expect("checked non-empty recovery position set");
        // Mirror live terminal booking-error: recovered open positions whose
        // settlement key already has a booking-error record release exposure
        // rather than parking Managed forever.
        if let Ok(settlement_key) = settlement_key_for_position(&open_position) {
            if self.settled_position_keys.contains(&settlement_key) {
                self.exposure = ExposureState::Flat;
                self.sync_exposure_context_from_active();
                self.refresh_book_subscriptions_for_current_state();
                return;
            }
            let position_key = settlement_position_key(&open_position, settlement_key.clone());
            let decision = decide_bootstrap_recovery_from_cache(
                true,
                Some(&position_key),
                open_position_count,
                observed_at_ns,
                &self.settlement_booking_error_keys,
                &self.terminal_settlement_keys,
            );
            let SettlementRecoveryEntryDecision::ApplyPriorBookingError {
                eligibility,
                canonical_evidence_already_durable,
            } = decision
            else {
                if let SettlementRecoveryEntryDecision::EnterBlindSettlementRecovery {
                    detail,
                    ..
                } = decision
                {
                    self.enter_blind_settlement_recovery(anyhow::anyhow!(detail));
                    return;
                }
                let exposure = self.bootstrapped_exposure_for(open_position, execution_venue);
                self.exposure = exposure;
                self.adopt_restart_open_exit_order_from_cache(execution_venue, strategy_id);
                return;
            };
            if !canonical_evidence_already_durable {
                self.enter_blind_settlement_recovery(anyhow::anyhow!(
                    "booking-terminal recovery key `{settlement_key}` lacks canonical terminal evidence"
                ));
                return;
            }
            let Some(RecoveredSettlementOutcome::BookingTerminal(recovered_terminal)) = self
                .context
                .settlement_recovery()
                .and_then(|recovery| recovery.outcomes().get(&settlement_key))
            else {
                self.enter_blind_settlement_recovery(anyhow::anyhow!(
                    "booking-terminal recovery key `{settlement_key}` lacks typed terminal evidence"
                ));
                return;
            };
            if let Err(error) = self.apply_terminal_settlement_transition(
                &open_position,
                eligibility,
                SettlementTerminalKeyDelta {
                    settlement_key: settlement_key.clone(),
                    insert_booking_error_key: true,
                    insert_terminal_key: true,
                    remove_close_fetch_attempt: true,
                },
                recovered_terminal.booking_error.clone(),
                format!("prior_booking_error_key_on_restart settlement_key={settlement_key}"),
                TerminalEvidenceState::CanonicalAlreadyDurable,
            ) {
                self.enter_blind_settlement_recovery(error);
            }
            return;
        }
        let exposure = self.bootstrapped_exposure_for(open_position, execution_venue);
        self.exposure = exposure;
        self.adopt_restart_open_exit_order_from_cache(execution_venue, strategy_id);
    }

    fn recover_settlement_bootstrap_from_scope(
        &mut self,
        settlement_scope_positions: &[OpenPositionState],
    ) -> bool {
        if self.context.settlement_recovery().is_none() {
            return true;
        }
        let recovery_scope_settlement_keys = settlement_scope_positions
            .iter()
            .map(settlement_key_for_position)
            .collect::<Result<BTreeSet<_>>>();
        let recovery_scope_settlement_keys = match recovery_scope_settlement_keys {
            Ok(keys) => keys,
            Err(error) => {
                self.enter_blind_settlement_recovery(error);
                return false;
            }
        };
        let recovery_delta = match recover_settlement_facts(
            self.context.settlement_recovery(),
            &self.config.strategy_id,
            &recovery_scope_settlement_keys,
        ) {
            Ok(delta) => delta,
            Err(error) => {
                self.enter_blind_settlement_recovery(error);
                return false;
            }
        };
        let booking_delta = recover_booking_facts(
            self.context.booking_recovery(),
            &recovery_scope_settlement_keys,
        );

        for evidence in &recovery_delta.settled_evidence {
            let Some(settlement_currency) = self.context.settlement_currency() else {
                self.enter_blind_settlement_recovery(anyhow::anyhow!(
                    "recovered settlement requires configured settlement currency"
                ));
                return false;
            };
            if settlement_currency.to_string() != evidence.settlement_currency {
                self.enter_blind_settlement_recovery(anyhow::anyhow!(
                    "recovered settlement currency `{}` does not match configured `{settlement_currency}`",
                    evidence.settlement_currency
                ));
                return false;
            }
            if let Err(error) = self.apply_settlement_runtime_effects(evidence, settlement_currency)
            {
                self.enter_blind_settlement_recovery(error);
                return false;
            }
        }

        self.settled_position_keys
            .extend(recovery_delta.settled_position_keys);
        self.settlement_booking_error_keys
            .extend(booking_delta.booking_error_keys);
        self.terminal_settlement_keys
            .extend(recovery_delta.terminal_settlement_keys);
        self.terminal_settlement_keys
            .extend(booking_delta.terminal_settlement_keys);
        true
    }

    fn enter_blind_settlement_recovery(&mut self, error: anyhow::Error) {
        let decision = decide_blind_settlement_recovery(&error);
        let SettlementRecoveryEntryDecision::EnterBlindSettlementRecovery {
            transition,
            outcome,
            ..
        } = decision
        else {
            return;
        };
        let position = self.settlement_position_candidate();
        self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
            reason: BlindRecoveryReason::SettlementEvidenceRecoveryFailed,
        });
        self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition,
            outcome,
            source: ORDER_LIFECYCLE_SOURCE_SETTLEMENT_RECOVERY,
            market_id: position
                .as_ref()
                .and_then(|position| position.lifecycle.market_id_owned()),
            instrument_id: position.as_ref().map(|position| position.instrument_id),
            position_id: position.as_ref().map(|position| position.position_id),
            client_order_id: None,
            prior_client_order_id: None,
            raw_reason_text: Some("settlement_evidence_recovery_failed".to_string()),
            order_side: position.as_ref().map(|position| position.entry_order_side),
            filled_quantity: None,
            residual_quantity: position.as_ref().map(|position| position.quantity),
            ts_event_ns: None,
        });
        log::error!(
            "binary_oracle_edge_taker settlement recovery failed closed: strategy_id={} error={error:#}",
            self.config.strategy_id,
        );
    }

    fn recover_exit_authority_state(
        &self,
        position: Option<ManagedPositionContext>,
        instrument_id: InstrumentId,
        position_id: PositionId,
        pending_exit: PendingExitState,
        cause: BoltV3RecoveredExitCause,
    ) -> Result<ExitPendingState> {
        let order = self
            .cache()
            .order(&pending_exit.client_order_id)
            .with_context(|| {
                format!(
                    "recovered exit order is missing from NT cache: client_order_id={}",
                    pending_exit.client_order_id
                )
            })?;
        anyhow::ensure!(
            order.instrument_id() == instrument_id,
            "recovered exit order instrument does not match its position"
        );
        if let Some(order_position_id) = order.position_id() {
            anyhow::ensure!(
                order_position_id == position_id,
                "recovered exit order position identity mismatch"
            );
        }
        let position_authority = self
            .context
            .position_authority()
            .ok_or_else(|| anyhow::anyhow!("recovered exit requires position authority"))?;
        let venue_position_id = self.exit_authority_venue_position_id(position_id)?;
        let lease = position_authority.acquire(instrument_id, venue_position_id)?;
        let canonical = match self.canonical_position_authority(position_id, instrument_id)? {
            Some(canonical) => canonical,
            None => anyhow::bail!("recovered exit requires a canonical NT position"),
        };
        let authority = BoltV3ExitOrderAuthorityHandle::recovered(
            cause,
            pending_exit.client_order_id,
            instrument_id,
            position_id,
            canonical.signed_quantity,
            canonical.side,
            &order,
            lease,
        )?;
        authority.refresh_recovered_baseline(Some(&canonical))?;
        Ok(ExitPendingState {
            position,
            pending_exit,
            authority,
        })
    }

    fn exit_authority_venue_position_id(
        &self,
        position_id: PositionId,
    ) -> Result<Option<PositionId>> {
        match parse_configured_oms_type(CONFIG_FIELD_OMS_TYPE, &self.config.oms_type)? {
            nautilus_model::enums::OmsType::Hedging => Ok(Some(position_id)),
            nautilus_model::enums::OmsType::Netting => Ok(None),
            nautilus_model::enums::OmsType::Unspecified => {
                anyhow::bail!("exit authority requires a specified OMS type")
            }
        }
    }

    fn acquire_exit_authority_flat_recovery(
        &self,
        instrument_id: InstrumentId,
        position_id: PositionId,
    ) -> Result<BoltV3ExitAuthorityRecoveryHandle> {
        let capability = self
            .context
            .position_authority()
            .ok_or_else(|| anyhow::anyhow!("exit recovery requires position authority"))?;
        BoltV3ExitAuthorityRecoveryHandle::acquire(
            capability,
            instrument_id,
            self.exit_authority_venue_position_id(position_id)?,
        )
    }

    fn adopt_restart_open_exit_order_from_cache(
        &mut self,
        execution_venue: Venue,
        strategy_id: StrategyId,
    ) {
        let Some(managed_position) = self.managed_position() else {
            return;
        };
        if managed_position.origin != ManagedPositionOrigin::RecoveryBootstrap {
            return;
        }
        let Ok(contract) = self.configured_position_contract() else {
            return;
        };
        let position = managed_position.position.clone();
        let open_exit_order_attribution =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let cache = self.cache();
                cache
                    .orders_open(
                        Some(&execution_venue),
                        Some(&position.instrument_id),
                        Some(&strategy_id),
                        None,
                        Some(contract.exit_order_side),
                    )
                    .into_iter()
                    .map(|order| {
                        let client_order_id = order.client_order_id();
                        let attributed_to_position =
                            cache.position_id(&client_order_id) == Some(position.position_id);
                        (order.clone(), attributed_to_position)
                    })
                    .collect::<Vec<_>>()
            }));
        let open_exit_order_attribution = match open_exit_order_attribution {
            Ok(open_exit_order_attribution) => open_exit_order_attribution,
            Err(_) => {
                self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::CacheProbeFailed,
                });
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition: OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked,
                    outcome: OrderLifecycleOutcome::BlindRecovery,
                    source: ORDER_LIFECYCLE_SOURCE_RESTART_BOOTSTRAP,
                    market_id: position.lifecycle.market_id_owned(),
                    instrument_id: Some(position.instrument_id),
                    position_id: Some(position.position_id),
                    client_order_id: None,
                    prior_client_order_id: None,
                    raw_reason_text: Some("cache_probe_failed".to_string()),
                    order_side: Some(contract.exit_order_side),
                    filled_quantity: None,
                    residual_quantity: Some(position.quantity),
                    ts_event_ns: None,
                });
                return;
            }
        };
        let unattributed_open_exit_count = open_exit_order_attribution
            .iter()
            .filter(|(_, attributed_to_position)| !*attributed_to_position)
            .count();
        let attributed_open_exit_orders = open_exit_order_attribution
            .into_iter()
            .filter_map(|(order, attributed_to_position)| attributed_to_position.then_some(order))
            .collect::<Vec<_>>();
        if unattributed_open_exit_count > 0 {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::UnattributedRestartOpenExitOrder {
                    instrument_id: position.instrument_id,
                },
            });
            self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                transition: OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked,
                outcome: OrderLifecycleOutcome::BlindRecovery,
                source: ORDER_LIFECYCLE_SOURCE_RESTART_BOOTSTRAP,
                market_id: position.lifecycle.market_id_owned(),
                instrument_id: Some(position.instrument_id),
                position_id: Some(position.position_id),
                client_order_id: None,
                prior_client_order_id: None,
                raw_reason_text: Some("unattributed_open_exit_order".to_string()),
                order_side: Some(contract.exit_order_side),
                filled_quantity: None,
                residual_quantity: Some(position.quantity),
                ts_event_ns: None,
            });
            return;
        }
        if attributed_open_exit_orders.is_empty() {
            return;
        }
        if attributed_open_exit_orders.len() > 1 {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::AmbiguousRestartOpenExitOrders {
                    instrument_id: position.instrument_id,
                    count: attributed_open_exit_orders.len(),
                },
            });
            self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                transition: OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked,
                outcome: OrderLifecycleOutcome::BlindRecovery,
                source: ORDER_LIFECYCLE_SOURCE_RESTART_BOOTSTRAP,
                market_id: position.lifecycle.market_id_owned(),
                instrument_id: Some(position.instrument_id),
                position_id: Some(position.position_id),
                client_order_id: None,
                prior_client_order_id: None,
                raw_reason_text: Some("ambiguous_open_exit_orders".to_string()),
                order_side: Some(contract.exit_order_side),
                filled_quantity: None,
                residual_quantity: Some(position.quantity),
                ts_event_ns: None,
            });
            return;
        }
        let order = attributed_open_exit_orders
            .into_iter()
            .next()
            .expect("checked exactly one attributed open exit order");
        let client_order_id = order.client_order_id();
        let recovered_position = managed_position_context(
            position.clone(),
            managed_position.origin,
            managed_position.pending_entry,
        );
        let pending_exit = PendingExitState {
            client_order_id,
            submitted_at_ms: None,
            market_id: position.lifecycle.market_id_owned(),
            position_id: Some(position.position_id),
        };
        let recovered = self.recover_exit_authority_state(
            Some(recovered_position.clone()),
            position.instrument_id,
            position.position_id,
            pending_exit.clone(),
            BoltV3RecoveredExitCause::StartupAdoption,
        );
        let recovered = match recovered {
            Ok(recovered) => recovered,
            Err(error) => {
                let now_ns = self.clock().timestamp_ns().as_u64();
                self.enter_exit_authority_recovery_hold(
                    Some(recovered_position),
                    pending_exit,
                    position.instrument_id,
                    ExitAuthorityRecoveryPlan::Reconstruct(
                        BoltV3RecoveredExitCause::StartupAdoption,
                    ),
                    now_ns,
                );
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition: OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked,
                    outcome: OrderLifecycleOutcome::ExitPending,
                    source: ORDER_LIFECYCLE_SOURCE_RESTART_BOOTSTRAP,
                    market_id: position.lifecycle.market_id_owned(),
                    instrument_id: Some(position.instrument_id),
                    position_id: Some(position.position_id),
                    client_order_id: Some(client_order_id),
                    prior_client_order_id: None,
                    raw_reason_text: Some("exit_authority_recovery_hold".to_string()),
                    order_side: Some(contract.exit_order_side),
                    filled_quantity: Some(order.filled_qty()),
                    residual_quantity: Some(position.quantity),
                    ts_event_ns: None,
                });
                log::error!(
                    "binary_oracle_edge_taker restart exit authority entered recovery hold: strategy_id={} client_order_id={} error={error:#}",
                    self.config.strategy_id,
                    client_order_id,
                );
                return;
            }
        };
        self.exposure = ExposureState::ExitPending(recovered);
        self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition: OrderLifecycleTransition::RestartOpenOrderAdopted,
            outcome: OrderLifecycleOutcome::ExitPending,
            source: ORDER_LIFECYCLE_SOURCE_RESTART_BOOTSTRAP,
            market_id: position.lifecycle.market_id_owned(),
            instrument_id: Some(position.instrument_id),
            position_id: Some(position.position_id),
            client_order_id: Some(client_order_id),
            prior_client_order_id: None,
            raw_reason_text: None,
            order_side: Some(contract.exit_order_side),
            filled_quantity: None,
            residual_quantity: Some(position.quantity),
            ts_event_ns: None,
        });
    }

    /// Classify a single recovered open position into the exposure state to adopt.
    ///
    /// The recovery probe already scopes the cache read to the execution venue, so a
    /// foreign-venue position should never reach here in production. This is the single
    /// fail-closed adoption decision and re-asserts the venue invariant structurally
    /// (defense in depth) BEFORE any contract check: the exit path would otherwise
    /// build/submit an order for a wrong-venue position with no further venue gate.
    /// A foreign-venue position is quarantined to blind recovery and
    /// is never managed.
    fn bootstrapped_exposure_for(
        &self,
        open_position: OpenPositionState,
        execution_venue: Venue,
    ) -> ExposureState {
        if open_position.instrument_id.venue != execution_venue {
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap quarantined foreign-venue cached position: strategy_id={} position_id={} instrument_id={} instrument_venue={} execution_venue={}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.instrument_id.venue,
                execution_venue,
            );
            return ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition {
                    instrument_venue: open_position.instrument_id.venue,
                    execution_venue,
                },
            });
        }
        if self
            .configured_position_contract()
            .ok()
            .is_some_and(|contract| {
                supports_strategy_managed_position(
                    open_position.entry_order_side,
                    open_position.side,
                    contract,
                )
            })
        {
            log::warn!(
                "binary_oracle_edge_taker recovery bootstrap loaded cached open position: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?} quantity={} avg_px_open={}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
                open_position.quantity,
                open_position.avg_px_open,
            );
            ExposureState::Managed(managed_position_context(
                open_position,
                ManagedPositionOrigin::RecoveryBootstrap,
                None,
            ))
        } else if is_observed_open_side(open_position.side) {
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap quarantined unsupported cached position: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?} quantity={} avg_px_open={}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
                open_position.quantity,
                open_position.avg_px_open,
            );
            ExposureState::UnsupportedObserved(UnsupportedObservedState {
                context: managed_position_context(
                    open_position,
                    ManagedPositionOrigin::RecoveryBootstrap,
                    None,
                ),
                reason: UnsupportedObservedReason::BootstrappedUnsupportedContract,
            })
        } else {
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap received invalid cached position side: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
            );
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::InvalidBootstrappedPosition {
                    entry_order_side: open_position.entry_order_side,
                    side: open_position.side,
                },
            })
        }
    }

    fn exposure_occupancy(&self) -> Option<ExposureOccupancy> {
        self.exposure.occupancy()
    }

    fn lifecycle_outcome_for_exposure(exposure: &ExposureState) -> OrderLifecycleOutcome {
        match exposure {
            ExposureState::Flat => OrderLifecycleOutcome::Flat,
            ExposureState::PendingEntry(_) => OrderLifecycleOutcome::PendingEntry,
            ExposureState::EntryReconcilePending { .. } => {
                OrderLifecycleOutcome::EntryReconcilePending
            }
            ExposureState::Managed(_) => OrderLifecycleOutcome::Managed,
            ExposureState::ExitAttempting(_)
            | ExposureState::ExitPending(_)
            | ExposureState::TerminalExitAwaitingPosition(_)
            | ExposureState::ExitAuthorityRecoveryHold(_) => OrderLifecycleOutcome::ExitPending,
            ExposureState::UnsupportedObserved(_) => OrderLifecycleOutcome::UnsupportedObserved,
            ExposureState::BlindRecovery(_) => OrderLifecycleOutcome::BlindRecovery,
        }
    }

    fn record_order_lifecycle_evidence(&self, input: OrderLifecycleEvidenceInput) {
        if let Err(error) = self.persist_order_lifecycle_evidence(input) {
            log::error!(
                "binary_oracle_edge_taker order lifecycle evidence write failed: strategy_id={} error={error:#}",
                self.config.strategy_id,
            );
        }
    }

    fn persist_order_lifecycle_evidence(&self, input: OrderLifecycleEvidenceInput) -> Result<()> {
        let evidence = self.order_lifecycle_evidence(input);
        self.persist_order_lifecycle_record(&evidence)
    }

    fn order_lifecycle_evidence(&self, input: OrderLifecycleEvidenceInput) -> OrderLifecycleFact {
        OrderLifecycleFact {
            strategy_id: self.config.strategy_id.clone(),
            transition: input.transition,
            outcome: input.outcome,
            source: input.source,
            market_id: input.market_id,
            instrument_id: input
                .instrument_id
                .map(|instrument_id| instrument_id.to_string()),
            position_id: input.position_id.map(|position_id| position_id.to_string()),
            client_order_id: input
                .client_order_id
                .map(|client_order_id| client_order_id.to_string()),
            prior_client_order_id: input
                .prior_client_order_id
                .map(|client_order_id| client_order_id.to_string()),
            raw_reason_text: input.raw_reason_text,
            order_side: input.order_side.map(evidence_order_side),
            filled_quantity: input.filled_quantity.map(|quantity| quantity.to_string()),
            residual_quantity: input.residual_quantity.map(|quantity| quantity.to_string()),
            ts_event_ns: input.ts_event_ns,
        }
    }

    fn persist_order_lifecycle_record(&self, evidence: &OrderLifecycleFact) -> Result<()> {
        match self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_order_lifecycle(evidence.clone())
        {
            NonBlockingRecordOutcome::Appended(_) => Ok(()),
            NonBlockingRecordOutcome::Failed(error) => Err(anyhow::Error::from(error))
                .with_context(|| {
                    format!(
                        "order lifecycle evidence write failed: transition={:?} source={:?}",
                        evidence.transition, evidence.source
                    )
                }),
        }
    }

    fn clear_pending_entry_state(&mut self) {
        if matches!(self.exposure, ExposureState::PendingEntry(_)) {
            self.exposure = ExposureState::Flat;
            self.prune_market_lifecycle_at_current_time();
        }
    }

    fn reclassify_unreachable_pending_entry_at_selection_boundary(&mut self, now_ms: u64) {
        let ExposureState::PendingEntry(pending) = &self.exposure else {
            return;
        };
        if self.active.books.up.instrument_id == Some(pending.instrument_id)
            || self.active.books.down.instrument_id == Some(pending.instrument_id)
        {
            return;
        }
        let pending = pending.clone();
        self.exposure = ExposureState::EntryReconcilePending {
            pending: pending.clone(),
            reason: EntryReconcileReason::UnresolvedAtSelectionBoundary,
        };
        self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition: OrderLifecycleTransition::BoundaryReclassification,
            outcome: OrderLifecycleOutcome::EntryReconcilePending,
            source: ORDER_LIFECYCLE_SOURCE_SELECTION_BOUNDARY,
            market_id: pending.lifecycle.market_id_owned(),
            instrument_id: Some(pending.instrument_id),
            position_id: None,
            client_order_id: Some(pending.client_order_id),
            prior_client_order_id: None,
            raw_reason_text: None,
            order_side: None,
            filled_quantity: None,
            residual_quantity: None,
            ts_event_ns: Some(now_ms.saturating_mul(NANOS_PER_MILLI_U64)),
        });
        log::error!(
            "binary_oracle_edge_taker pending entry unresolved at selection boundary: strategy_id={} instrument_id={} client_order_id={} entering fail-closed recovery",
            self.config.strategy_id,
            pending.instrument_id,
            pending.client_order_id,
        );
    }

    fn enforce_one_position_invariant(&self) -> Result<()> {
        let Some(occupancy) = self.exposure_occupancy() else {
            return Ok(());
        };

        let message = format!("one-position invariant occupied by {occupancy:?}");
        if cfg!(debug_assertions) {
            panic!("{message}");
        }

        self.report_one_position_invariant_violation(occupancy);
        anyhow::bail!("{message}");
    }

    fn report_one_position_invariant_violation(&self, occupancy: ExposureOccupancy) {
        if self.last_reported_exposure_occupancy.get() == Some(occupancy) {
            return;
        }
        self.last_reported_exposure_occupancy.set(Some(occupancy));
        let message = format!("one-position invariant occupied by {occupancy:?}");
        log::error!("{message}");
    }

    fn market_in_cooldown(&self, market_id: &str, now_ms: u64) -> bool {
        self.market_lifecycle
            .get(market_id)
            .is_some_and(|ledger| ledger.in_cooldown(now_ms))
    }

    fn arm_market_cooldown(&mut self, market_id: &str, now_ms: u64) {
        self.market_lifecycle
            .entry(market_id.to_string())
            .or_insert_with(MarketLifecycleLedger::empty)
            .cooldown_expires_at_ms = Some(
            now_ms.saturating_add(
                self.config
                    .reentry_cooldown_secs
                    .saturating_mul(MILLIS_PER_SECOND_U64),
            ),
        );
    }

    fn record_market_fill(&mut self, market_id: &str, now_ms: u64) {
        self.arm_market_cooldown(market_id, now_ms);
        let ledger = self
            .market_lifecycle
            .entry(market_id.to_string())
            .or_insert_with(MarketLifecycleLedger::empty);
        ledger.churn_count = ledger.churn_count.saturating_add(COUNTER_INCREMENT_U64);
        self.prune_market_lifecycle(now_ms);
    }

    #[cfg(test)]
    fn market_churn_count(&self, market_id: &str) -> u64 {
        self.market_lifecycle
            .get(market_id)
            .map(|ledger| ledger.churn_count)
            .unwrap_or(0)
    }

    fn prune_market_lifecycle(&mut self, now_ms: u64) {
        let retained_market_ids = self.retained_market_lifecycle_ids();
        self.market_lifecycle.retain(|market_id, ledger| {
            retained_market_ids.contains(market_id) || ledger.in_cooldown(now_ms)
        });
        self.prune_entry_reject_state();
    }

    fn retained_market_lifecycle_ids(&self) -> BTreeSet<String> {
        let mut retained = BTreeSet::new();
        if let Some(market_id) = self.active.market_id.clone() {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .pending_entry()
            .and_then(|pending| pending.lifecycle.market_id_owned())
        {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .tracked_observed_position()
            .and_then(|position| position.lifecycle.market_id_owned())
        {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .exposure
            .exit_pending_snapshot()
            .and_then(|exit| exit.pending_exit.market_id.clone())
        {
            retained.insert(market_id);
        }
        retained
    }

    fn prune_entry_reject_state(&mut self) {
        let retained_instruments = self.retained_entry_reject_instrument_ids();
        self.entry_reject_state
            .retain(|instrument_id, _| retained_instruments.contains(instrument_id));
    }

    fn retained_entry_reject_instrument_ids(&self) -> BTreeSet<InstrumentId> {
        let mut retained = BTreeSet::new();
        if let Some(instrument_id) = self.instrument_id_for_side(OutcomeSide::Up) {
            retained.insert(instrument_id);
        }
        if let Some(instrument_id) = self.instrument_id_for_side(OutcomeSide::Down) {
            retained.insert(instrument_id);
        }
        retained
    }

    fn entry_gate_decision_at(&self, now_ms: u64) -> EntryGateDecision {
        let mut blocked_by = Vec::new();

        if self.active.phase != SelectionPhase::Active {
            blocked_by.push(EntryBlockReason::PhaseNotActive);
        }
        if !self.active.books.metadata_matches_selection() {
            blocked_by.push(EntryBlockReason::MetadataMismatch);
        }
        if !self.active.books.is_priced() {
            blocked_by.push(EntryBlockReason::ActiveBookNotPriced);
        }
        if self.active.books.any_crossed() {
            blocked_by.push(EntryBlockReason::BookCrossed);
        }
        if self.active.interval_open.is_none() {
            blocked_by.push(EntryBlockReason::IntervalOpenMissing);
        }
        if !self.active.warmup_complete() {
            blocked_by.push(EntryBlockReason::WarmupIncomplete);
        }
        if self.exposure.is_recovering() {
            blocked_by.push(EntryBlockReason::RecoveryMode);
        }
        if self
            .current_market_id()
            .is_some_and(|market_id| self.market_in_cooldown(market_id, now_ms))
        {
            blocked_by.push(EntryBlockReason::MarketCoolingDown);
        }
        if self
            .pricing
            .spike_until_ms
            .is_some_and(|spike_until_ms| now_ms < spike_until_ms)
        {
            blocked_by.push(EntryBlockReason::SpotSpikeCooldown);
        }
        for reason in self
            .active_forced_flat_reasons_at(now_ms)
            .into_iter()
            .filter(|reason| *reason != ForcedFlatReason::MetadataMismatch)
        {
            blocked_by.push(EntryBlockReason::ForcedFlat(reason));
        }
        if let Some(occupancy) = self.exposure_occupancy() {
            if should_report_one_position_gate_violation(occupancy) {
                self.report_one_position_invariant_violation(occupancy);
            }
            blocked_by.push(EntryBlockReason::OnePositionInvariant(occupancy));
        } else {
            self.last_reported_exposure_occupancy.set(None);
        }

        EntryGateDecision { blocked_by }
    }

    fn effective_stale_reference_after_ms(&self) -> u64 {
        self.config.forced_flat_stale_reference_ms
    }

    fn active_forced_flat_reasons_at(&self, now_ms: u64) -> Vec<ForcedFlatReason> {
        evaluate_forced_flat_predicates(&ForcedFlatInputs {
            frozen: self.active.phase == SelectionPhase::Freeze,
            metadata_matches_selection: self.active.books.metadata_matches_selection(),
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_reference_after_ms: self.effective_stale_reference_after_ms(),
            liquidity_available: self.active.books.minimum_liquidity(),
            min_liquidity_required: self.config.forced_flat_thin_book_min_liquidity,
            fast_venue_incoherent: self.active.fast_venue_incoherent,
        })
        .into_iter()
        .collect()
    }

    fn position_forced_flat_reasons_at(&self, now_ms: u64) -> Vec<ForcedFlatReason> {
        let Some(managed_position) = self.managed_position() else {
            return self.active_forced_flat_reasons_at(now_ms);
        };
        let open_position = &managed_position.position;

        evaluate_forced_flat_predicates(&ForcedFlatInputs {
            frozen: self.active.phase == SelectionPhase::Freeze,
            metadata_matches_selection: open_position.book.metadata_matches_selection(),
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_reference_after_ms: self.effective_stale_reference_after_ms(),
            liquidity_available: open_position.book.liquidity_available,
            min_liquidity_required: self.config.forced_flat_thin_book_min_liquidity,
            fast_venue_incoherent: self.active.fast_venue_incoherent,
        })
        .into_iter()
        .collect()
    }

    /// Preserve the existing reference-price freshness coordinate. This venue
    /// event clock is intentionally separate from the RV receive-clock gate.
    fn current_reference_pricing_event_ms(&self) -> Option<VenueEventMs> {
        [
            self.pricing
                .selected_pricing_spot()
                .map(|spot| VenueEventMs::new(spot.observed_ts_ms)),
            self.pricing
                .last_reference_current_price_ts_ms()
                .map(VenueEventMs::new),
            self.active.last_reference_ts_ms.map(VenueEventMs::new),
        ]
        .into_iter()
        .flatten()
        .max()
    }

    #[cfg(test)]
    fn current_entry_pricing_inputs_at(
        &self,
        now_ms: u64,
    ) -> std::result::Result<EntryPricingInputs, Vec<EntryPricingBlockReason>> {
        self.current_entry_pricing_inputs_for_receive_at(
            now_ms,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(now_ms)),
        )
    }

    #[cfg(test)]
    fn current_fair_probability_up_at(&self, now_ms: u64) -> Option<Probability> {
        self.current_fair_probability_up_for_receive_at(
            now_ms,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(now_ms)),
        )
    }

    #[cfg(test)]
    fn entry_evaluation_at(&self, now_ms: u64) -> EntryEvaluation {
        self.entry_evaluation_for_receive_at(
            now_ms,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(now_ms)),
        )
    }

    #[cfg(test)]
    fn entry_submission_decision_at(&self, now_ms: u64) -> EntrySubmissionDecision {
        self.entry_submission_decision_for_receive_at(
            now_ms,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(now_ms)),
        )
    }

    #[cfg(test)]
    fn try_submit_entry_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        self.try_submit_entry_order_for_receive(
            now_ms,
            EntryEvaluationReceiveContext::new(LocalReceiveMs::new(now_ms)),
        )
    }

    fn current_realized_vol_for_gate_at(
        &self,
        realized_vol_gate_receive_ms: LocalReceiveMs,
    ) -> Option<f64> {
        self.pricing.current_realized_vol_at(
            realized_vol_gate_receive_ms,
            Some(self.config.realized_volatility_max_source_age_ms),
        )
    }

    #[cfg(test)]
    fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.current_realized_vol_for_gate_at(LocalReceiveMs::new(now_ms))
    }

    fn evidence_spot_price(&self) -> Option<f64> {
        self.pricing
            .spot_price()
            .filter(|value| is_positive_finite(*value))
            .or_else(|| {
                self.latest_signal_quote
                    .as_ref()
                    .map(|quote| quote.price)
                    .filter(|value| is_positive_finite(*value))
            })
    }

    fn evidence_spot_venue_name(&self) -> Option<String> {
        self.pricing
            .selected_pricing_spot()
            .map(|spot| spot.venue.clone())
            .or_else(|| {
                self.latest_signal_quote
                    .as_ref()
                    .filter(|quote| is_positive_finite(quote.price))
                    .map(|quote| quote.venue.clone())
            })
    }

    fn selected_reference_quote_for_evidence(&self) -> Option<&SelectedReferenceQuoteEvidence> {
        self.latest_selected_reference_quote.as_ref()
    }

    fn evidence_reference_current_price(&self) -> Option<f64> {
        self.pricing
            .last_reference_current_price()
            .filter(|value| is_positive_finite(*value))
            .or_else(|| {
                self.active
                    .reference_current_price
                    .filter(|value| is_positive_finite(*value))
            })
            .or_else(|| {
                self.selected_reference_quote_for_evidence()
                    .map(|evidence| evidence.quote.price())
                    .filter(|value| is_positive_finite(*value))
            })
    }

    fn evidence_reference_current_price_source_id(&self) -> Option<String> {
        self.active
            .reference_current_price_source_id
            .clone()
            .or_else(|| {
                self.selected_reference_quote_for_evidence()
                    .map(|evidence| evidence.quote.source_id().to_string())
            })
    }

    fn evidence_reference_current_price_failed_over(&self) -> Option<bool> {
        self.active.reference_current_price_failed_over.or_else(|| {
            self.selected_reference_quote_for_evidence()
                .map(|evidence| evidence.failed_over)
        })
    }

    fn current_seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        self.active.seconds_to_expiry_at(now_ms)
    }

    fn current_entry_pricing_inputs_for_receive_at(
        &self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
    ) -> std::result::Result<EntryPricingInputs, Vec<EntryPricingBlockReason>> {
        self.pricing
            .entry_pricing_inputs_at(
                &taker_pricing_config(&self.config),
                TakerPricingRequest {
                    now_ms,
                    realized_vol_gate_receive_ms: receive_context.receive_ms(),
                    reference_gate_event_ms: self.current_reference_pricing_event_ms(),
                    strike_price: self.active.interval_open,
                    seconds_to_market_end: self.current_seconds_to_expiry_at(now_ms),
                },
            )
            .map(|result| EntryPricingInputs {
                spot_price: result.spot_price,
                strike_price: result.strike_price,
                seconds_to_expiry: result.seconds_to_market_end,
                realized_vol: result.realized_vol,
                theta_scaled_min_edge_bps: result.theta_scaled_min_edge_bps,
            })
            .map_err(|blocked_by| {
                blocked_by
                    .into_iter()
                    .map(entry_pricing_block_reason_from_taker)
                    .collect()
            })
    }

    fn current_fair_probability_up_for_receive_at(
        &self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
    ) -> Option<Probability> {
        self.pricing
            .entry_pricing_at(
                &taker_pricing_config(&self.config),
                TakerPricingRequest {
                    now_ms,
                    realized_vol_gate_receive_ms: receive_context.receive_ms(),
                    reference_gate_event_ms: self.current_reference_pricing_event_ms(),
                    strike_price: self.active.interval_open,
                    seconds_to_market_end: self.current_seconds_to_expiry_at(now_ms),
                },
            )
            .ok()
            .and_then(|result| Probability::new(result.fair_probability_up))
    }

    fn current_position_fast_spot(&self) -> Option<&FastSpotObservation> {
        let open_position = &self.managed_position()?.position;
        if open_position.lifecycle.market_id() != self.active.market_id.as_deref() {
            return None;
        }
        self.pricing.selected_pricing_spot()
    }

    fn current_position_spot_price(&self) -> Option<f64> {
        self.current_position_fast_spot()
            .map(|spot| spot.price)
            .filter(|value| is_positive_finite(*value))
    }

    fn current_scaled_min_edge_bps_at(&self, now_ms: u64) -> Option<f64> {
        self.pricing.theta_scaled_min_edge_bps_for(
            &taker_pricing_config(&self.config),
            self.current_seconds_to_expiry_at(now_ms),
        )
    }

    fn current_uncertainty_band_probability_for_gate_at(
        &self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
    ) -> Option<Probability> {
        let seconds_to_expiry = self.current_seconds_to_expiry_at(now_ms)?;
        let realized_vol = self.current_realized_vol_for_gate_at(receive_context.receive_ms())?;
        self.uncertainty_band_probability_for_seconds(seconds_to_expiry, realized_vol)
    }

    fn uncertainty_band_probability_for_seconds(
        &self,
        seconds_to_expiry: u64,
        realized_vol: f64,
    ) -> Option<Probability> {
        let time_uncertainty_probability =
            time_uncertainty_probability(realized_vol, seconds_to_expiry, SECONDS_PER_YEAR_F64)?;
        let lead_gap_probability = self.pricing.last_lead_gap_probability?;
        let jitter_penalty_probability = self.pricing.last_jitter_penalty_probability?;

        uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability,
            jitter_penalty_probability,
            time_uncertainty_probability,
        })
    }

    fn entry_evaluation_log_fields_at(
        &self,
        now_ms: u64,
        submission: &EntrySubmissionDecision,
    ) -> EntryEvaluationLogFields {
        let evaluation = &submission.evaluation;
        let spot_venue_name = self.evidence_spot_venue_name();
        let fast_venue_available = self.pricing.selected_pricing_spot().is_some();
        let reference_current_price = self.evidence_reference_current_price();
        let reference_current_price_available =
            self.pricing.last_reference_current_price().is_some();
        let realized_volatility_receipt = &evaluation.realized_volatility_receipt;

        EntryEvaluationLogFields {
            market_id: self.active.market_id.clone(),
            phase: self.active.phase,
            gate_blocked_by: evaluation.gate.blocked_by.clone(),
            pricing_blocked_by: evaluation.pricing_blocked_by.clone(),
            spot_price: self.evidence_spot_price(),
            spot_venue_name,
            reference_current_price,
            interval_open: self.active.interval_open,
            seconds_to_expiry: self.current_seconds_to_expiry_at(now_ms),
            realized_vol: realized_volatility_receipt.realized_vol,
            realized_vol_source_venue: realized_volatility_receipt.source_venue.clone(),
            realized_vol_source_ts_ms: realized_volatility_receipt.source_ts_ms,
            realized_vol_gate_result: realized_volatility_receipt.gate_result,
            realized_vol_receive_watermark_ms: realized_volatility_receipt.receive_watermark_ms,
            realized_volatility_evidence: realized_volatility_receipt.evidence.clone(),
            pricing_kurtosis: self.config.pricing_kurtosis,
            theta_decay_factor: self.config.theta_decay_factor,
            theta_scaled_min_edge_bps: evaluation
                .min_worst_case_ev_bps
                .or_else(|| self.current_scaled_min_edge_bps_at(now_ms)),
            fair_probability_up: evaluation.fair_probability_up.map(Probability::value),
            fair_probability_down: evaluation
                .fair_probability_up
                .map(|value| value.complement().value()),
            uncertainty_band_probability: evaluation
                .uncertainty_band_probability
                .map(Probability::value),
            uncertainty_band_live: evaluation.uncertainty_band_probability.is_some(),
            uncertainty_band_reason: if evaluation.uncertainty_band_probability.is_some() {
                EVIDENCE_REASON_DERIVED_FROM_LEAD_GAP_JITTER_AND_TIME
            } else {
                EVIDENCE_REASON_UNCERTAINTY_BAND_UNAVAILABLE
            },
            lead_agreement_corr: self
                .pricing
                .last_lead_agreement_corr
                .map(Probability::value),
            fast_venue_age_ms: self.pricing.last_fast_venue_age_ms,
            fast_venue_jitter_ms: self.pricing.last_fast_venue_jitter_ms,
            up_entry_cost: executable_edge_vwap_price(evaluation.up_executable_edge),
            down_entry_cost: executable_edge_vwap_price(evaluation.down_executable_edge),
            up_entry_limit_price: executable_edge_limit_price(evaluation.up_executable_edge),
            down_entry_limit_price: executable_edge_limit_price(evaluation.down_executable_edge),
            up_gross_cost_cents: executable_edge_cost_component(
                evaluation.up_executable_edge,
                |cost| cost.gross_cost_cents,
            ),
            down_gross_cost_cents: executable_edge_cost_component(
                evaluation.down_executable_edge,
                |cost| cost.gross_cost_cents,
            ),
            up_slippage_buffer_cents: executable_edge_cost_component(
                evaluation.up_executable_edge,
                |cost| cost.slippage_buffer_cents,
            ),
            down_slippage_buffer_cents: executable_edge_cost_component(
                evaluation.down_executable_edge,
                |cost| cost.slippage_buffer_cents,
            ),
            up_total_adjusted_cost_cents: executable_edge_cost_component(
                evaluation.up_executable_edge,
                |cost| cost.total_adjusted_cost_cents,
            ),
            down_total_adjusted_cost_cents: executable_edge_cost_component(
                evaluation.down_executable_edge,
                |cost| cost.total_adjusted_cost_cents,
            ),
            up_edge_cents_per_share: executable_edge_cents_per_share(evaluation.up_executable_edge),
            down_edge_cents_per_share: executable_edge_cents_per_share(
                evaluation.down_executable_edge,
            ),
            up_worst_case_ev_bps: evaluation.up_worst_case_ev_bps,
            down_worst_case_ev_bps: evaluation.down_worst_case_ev_bps,
            sized_entry_cost: executable_edge_vwap_price(evaluation.sized_executable_edge),
            sized_entry_limit_price: executable_edge_limit_price(evaluation.sized_executable_edge),
            sized_gross_cost_cents: executable_edge_cost_component(
                evaluation.sized_executable_edge,
                |cost| cost.gross_cost_cents,
            ),
            sized_slippage_buffer_cents: executable_edge_cost_component(
                evaluation.sized_executable_edge,
                |cost| cost.slippage_buffer_cents,
            ),
            sized_total_adjusted_cost_cents: executable_edge_cost_component(
                evaluation.sized_executable_edge,
                |cost| cost.total_adjusted_cost_cents,
            ),
            sized_edge_cents_per_share: executable_edge_cents_per_share(
                evaluation.sized_executable_edge,
            ),
            sized_worst_case_ev_bps: evaluation.sized_worst_case_ev_bps,
            expected_ev_per_notional: evaluation.expected_ev_per_notional,
            order_notional_target: self.config.order_notional_target,
            maximum_position_notional: self.config.maximum_position_notional,
            risk_lambda: self.config.risk_lambda,
            sizing_ev_reference_bps: self.config.sizing_ev_reference_bps,
            book_impact_cap_bps: self.config.book_impact_cap_bps,
            book_impact_cap_notional: evaluation.book_impact_cap_notional,
            sized_notional: evaluation.sized_notional,
            selected_side: evaluation.selected_side,
            fast_venue_available,
            reference_current_price_available,
            reference_current_price_available_without_fast_venue: !fast_venue_available
                && reference_current_price_available,
            lead_quality_policy_applied: self.pricing.lead_quality_policy_applied,
            lead_quality_reason: if self.pricing.fast_venue_incoherent {
                EVIDENCE_REASON_NO_FAST_VENUE_CLEARED_LEAD_QUALITY_THRESHOLDS
            } else {
                EVIDENCE_REASON_LEAD_QUALITY_THRESHOLDS_APPLIED_TO_LIVE_FAST_SPOT_SELECTION
            },
            submission_instrument_id: submission.instrument_id,
            submission_order_side: submission.order_side,
            submission_price: submission.price,
            submission_quantity_value: submission.quantity_value,
            submission_client_order_id: submission.client_order_id,
            submission_blocked_reason: submission.blocked_reason,
        }
    }

    fn log_entry_evaluation(&mut self, now_ms: u64, submission: &EntrySubmissionDecision) {
        let fields = self.entry_evaluation_log_fields_at(now_ms, submission);
        let blocked = !fields.gate_blocked_by.is_empty() || !fields.pricing_blocked_by.is_empty();
        log::debug!(
            "binary_oracle_edge_taker entry realized-volatility receipt: strategy_id={} snapshot={:?}",
            self.config.strategy_id,
            fields.realized_volatility_evidence
        );

        if blocked {
            let reason_sets = (
                fields.gate_blocked_by.clone(),
                fields.pricing_blocked_by.clone(),
            );
            let reason_set_changed =
                self.last_entry_block_reason_sets.as_ref() != Some(&reason_sets);
            if reason_set_changed {
                log::warn!(
                    "binary_oracle_edge_taker entry blocked: strategy_id={} gate_blocked_by={:?} pricing_blocked_by={:?}",
                    self.config.strategy_id,
                    fields.gate_blocked_by,
                    fields.pricing_blocked_by
                );
                self.last_entry_block_reason_sets = Some(reason_sets);
            }
            // Full field dump every blocked tick is debug-only (state-change WARN above).
            log::debug!(
                "binary_oracle_edge_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} realized_vol_gate_result={:?} realized_vol_receive_watermark_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_entry_cost={:?} down_entry_cost={:?} up_entry_limit_price={:?} down_entry_limit_price={:?} up_gross_cost_cents={:?} down_gross_cost_cents={:?} up_slippage_buffer_cents={:?} down_slippage_buffer_cents={:?} up_total_adjusted_cost_cents={:?} down_total_adjusted_cost_cents={:?} up_edge_cents_per_share={:?} down_edge_cents_per_share={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} sized_entry_cost={:?} sized_entry_limit_price={:?} sized_gross_cost_cents={:?} sized_slippage_buffer_cents={:?} sized_total_adjusted_cost_cents={:?} sized_edge_cents_per_share={:?} sized_worst_case_ev_bps={:?} expected_ev_per_notional={:?} order_notional_target={} maximum_position_notional={} risk_lambda={} sizing_ev_reference_bps={} book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?} selected_side={:?} fast_venue_available={} reference_current_price_available={} reference_current_price_available_without_fast_venue={} lead_quality_policy_applied={} lead_quality_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                self.config.strategy_id,
                fields.market_id,
                fields.phase,
                fields.gate_blocked_by,
                fields.pricing_blocked_by,
                fields.spot_price,
                fields.spot_venue_name,
                fields.reference_current_price,
                fields.interval_open,
                fields.seconds_to_expiry,
                fields.realized_vol,
                fields.realized_vol_source_venue,
                fields.realized_vol_source_ts_ms,
                fields.realized_vol_gate_result,
                fields.realized_vol_receive_watermark_ms,
                fields.pricing_kurtosis,
                fields.theta_decay_factor,
                fields.theta_scaled_min_edge_bps,
                fields.fair_probability_up,
                fields.fair_probability_down,
                fields.uncertainty_band_probability,
                fields.uncertainty_band_live,
                fields.uncertainty_band_reason,
                fields.lead_agreement_corr,
                fields.fast_venue_age_ms,
                fields.fast_venue_jitter_ms,
                fields.up_entry_cost,
                fields.down_entry_cost,
                fields.up_entry_limit_price,
                fields.down_entry_limit_price,
                fields.up_gross_cost_cents,
                fields.down_gross_cost_cents,
                fields.up_slippage_buffer_cents,
                fields.down_slippage_buffer_cents,
                fields.up_total_adjusted_cost_cents,
                fields.down_total_adjusted_cost_cents,
                fields.up_edge_cents_per_share,
                fields.down_edge_cents_per_share,
                fields.up_worst_case_ev_bps,
                fields.down_worst_case_ev_bps,
                fields.sized_entry_cost,
                fields.sized_entry_limit_price,
                fields.sized_gross_cost_cents,
                fields.sized_slippage_buffer_cents,
                fields.sized_total_adjusted_cost_cents,
                fields.sized_edge_cents_per_share,
                fields.sized_worst_case_ev_bps,
                fields.expected_ev_per_notional,
                fields.order_notional_target,
                fields.maximum_position_notional,
                fields.risk_lambda,
                fields.sizing_ev_reference_bps,
                fields.book_impact_cap_bps,
                fields.book_impact_cap_notional,
                fields.sized_notional,
                fields.selected_side,
                fields.fast_venue_available,
                fields.reference_current_price_available,
                fields.reference_current_price_available_without_fast_venue,
                fields.lead_quality_policy_applied,
                fields.lead_quality_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity_value,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        } else {
            // One INFO line only on the transition from blocked → unblocked.
            if self.last_entry_block_reason_sets.take().is_some() {
                log::info!(
                    "binary_oracle_edge_taker entry unblocked: strategy_id={}",
                    self.config.strategy_id
                );
            }
            log::debug!(
                "binary_oracle_edge_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} realized_vol_gate_result={:?} realized_vol_receive_watermark_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_entry_cost={:?} down_entry_cost={:?} up_entry_limit_price={:?} down_entry_limit_price={:?} up_gross_cost_cents={:?} down_gross_cost_cents={:?} up_slippage_buffer_cents={:?} down_slippage_buffer_cents={:?} up_total_adjusted_cost_cents={:?} down_total_adjusted_cost_cents={:?} up_edge_cents_per_share={:?} down_edge_cents_per_share={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} sized_entry_cost={:?} sized_entry_limit_price={:?} sized_gross_cost_cents={:?} sized_slippage_buffer_cents={:?} sized_total_adjusted_cost_cents={:?} sized_edge_cents_per_share={:?} sized_worst_case_ev_bps={:?} expected_ev_per_notional={:?} order_notional_target={} maximum_position_notional={} risk_lambda={} sizing_ev_reference_bps={} book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?} selected_side={:?} fast_venue_available={} reference_current_price_available={} reference_current_price_available_without_fast_venue={} lead_quality_policy_applied={} lead_quality_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                self.config.strategy_id,
                fields.market_id,
                fields.phase,
                fields.gate_blocked_by,
                fields.pricing_blocked_by,
                fields.spot_price,
                fields.spot_venue_name,
                fields.reference_current_price,
                fields.interval_open,
                fields.seconds_to_expiry,
                fields.realized_vol,
                fields.realized_vol_source_venue,
                fields.realized_vol_source_ts_ms,
                fields.realized_vol_gate_result,
                fields.realized_vol_receive_watermark_ms,
                fields.pricing_kurtosis,
                fields.theta_decay_factor,
                fields.theta_scaled_min_edge_bps,
                fields.fair_probability_up,
                fields.fair_probability_down,
                fields.uncertainty_band_probability,
                fields.uncertainty_band_live,
                fields.uncertainty_band_reason,
                fields.lead_agreement_corr,
                fields.fast_venue_age_ms,
                fields.fast_venue_jitter_ms,
                fields.up_entry_cost,
                fields.down_entry_cost,
                fields.up_entry_limit_price,
                fields.down_entry_limit_price,
                fields.up_gross_cost_cents,
                fields.down_gross_cost_cents,
                fields.up_slippage_buffer_cents,
                fields.down_slippage_buffer_cents,
                fields.up_total_adjusted_cost_cents,
                fields.down_total_adjusted_cost_cents,
                fields.up_edge_cents_per_share,
                fields.down_edge_cents_per_share,
                fields.up_worst_case_ev_bps,
                fields.down_worst_case_ev_bps,
                fields.sized_entry_cost,
                fields.sized_entry_limit_price,
                fields.sized_gross_cost_cents,
                fields.sized_slippage_buffer_cents,
                fields.sized_total_adjusted_cost_cents,
                fields.sized_edge_cents_per_share,
                fields.sized_worst_case_ev_bps,
                fields.expected_ev_per_notional,
                fields.order_notional_target,
                fields.maximum_position_notional,
                fields.risk_lambda,
                fields.sizing_ev_reference_bps,
                fields.book_impact_cap_bps,
                fields.book_impact_cap_notional,
                fields.sized_notional,
                fields.selected_side,
                fields.fast_venue_available,
                fields.reference_current_price_available,
                fields.reference_current_price_available_without_fast_venue,
                fields.lead_quality_policy_applied,
                fields.lead_quality_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity_value,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        }
    }

    fn entry_forced_flat_evidence_inputs(&self) -> ForcedFlatEvidenceInputs {
        ForcedFlatEvidenceInputs {
            stale_reference_after_ms: Some(self.effective_stale_reference_after_ms()),
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            min_liquidity_required: option_evidence_number(Some(
                self.config.forced_flat_thin_book_min_liquidity,
            )),
            liquidity_available: option_evidence_number(self.active.books.minimum_liquidity()),
            frozen: self.active.phase == SelectionPhase::Freeze,
            metadata_matches_selection: self.active.books.metadata_matches_selection(),
            fast_venue_incoherent: self.active.fast_venue_incoherent,
        }
    }

    fn exit_forced_flat_evidence_inputs(&self) -> ForcedFlatEvidenceInputs {
        let managed_position = self.managed_position();
        let open_position = managed_position.as_ref().map(|managed| &managed.position);
        ForcedFlatEvidenceInputs {
            stale_reference_after_ms: Some(self.effective_stale_reference_after_ms()),
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            min_liquidity_required: option_evidence_number(Some(
                self.config.forced_flat_thin_book_min_liquidity,
            )),
            liquidity_available: option_evidence_number(
                open_position
                    .and_then(|position| position.book.liquidity_available)
                    .or_else(|| self.active.books.minimum_liquidity()),
            ),
            frozen: self.active.phase == SelectionPhase::Freeze,
            metadata_matches_selection: open_position
                .map(|position| position.book.metadata_matches_selection())
                .unwrap_or_else(|| self.active.books.metadata_matches_selection()),
            fast_venue_incoherent: self.active.fast_venue_incoherent,
        }
    }

    /// Returns `true` when a new skip was recorded (not evidence-deduped).
    fn record_entry_skip_once(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        reason_category: EvidenceEntrySkipReason,
    ) -> Result<bool> {
        let fields = self.entry_evaluation_log_fields_at(now_ms, decision);
        let forced_flat_inputs = self.entry_forced_flat_evidence_inputs();
        // The episode is the market; the novelty axis is the skip reason. The
        // blocker sets, availability flags and interval that used to form the
        // key are diagnostics that change while the market does not, so keying
        // on them reopened the episode on churn and re-emitted every tick.
        //
        // No bound market means no episode to attribute this to, and that is a
        // reachable state rather than a fault: `StrategyCoreNotRegistered` is
        // raised before the strategy is registered at all, when no market has
        // been selected. Not recorded, and deliberately not an error -- an
        // `Err` here would propagate into the strategy callback, which evidence
        // is never allowed to abort. The skip is still logged by the caller.
        let Ok(episode) = self.evidence_episode_id() else {
            log::debug!(
                "entry skip {reason_category:?} has no bound market episode, so it is not \
                 recorded as episode evidence"
            );
            return Ok(false);
        };
        let state = entry_skip_canonical_state(reason_category);
        // Claims before the write below: a claimed state stays claimed even if
        // the write fails, so a broken sink cannot become the flood this guard
        // exists to prevent.
        if !self.entry_skip_novelty.claim_once(&episode, state)? {
            return Ok(false);
        }
        let evidence = entry_skip_fact(
            self.config.strategy_id.clone(),
            now_ms,
            reason_category,
            &fields,
            forced_flat_inputs,
        );
        if let ObservationRecordOutcome::FailureReported(error) = self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_entry_skip_observation(evidence)
        {
            // An entry skip is declining new risk: a telemetry-write failure
            // must never abort the strategy callback (which would skip
            // downstream safety logic such as enforce_one_position_invariant).
            // Surface the lost write at the highest non-panicking severity and
            // let the skip path proceed.
            log::error!(
                "binary_oracle_edge_taker entry skip evidence write failed: strategy_id={} error={error:#}",
                self.config.strategy_id
            );
        }
        Ok(true)
    }

    fn record_and_log_entry_skip(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        reason: EvidenceEntrySkipReason,
    ) -> Result<()> {
        // WARN keyed on the same evidence dedupe as record_entry_skip_once.
        if self.record_entry_skip_once(now_ms, decision, reason)? {
            log::warn!(
                "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
                self.config.strategy_id,
                entry_skip_reason_label(reason)
            );
        }
        Ok(())
    }

    fn record_exit_intent_or_hold_once(
        &mut self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
        decision: &ExitIntentDecision,
    ) -> Result<()> {
        let action_chosen = decision.blocked_reason.is_none()
            && decision.instrument_id.is_some()
            && decision.order_side.is_some()
            && decision.price.is_some()
            && decision.quantity.is_some();
        if !action_chosen
            && decision.forced_flat_reasons.is_empty()
            && decision.blocked_reason.is_none()
        {
            return Ok(());
        }
        if decision.blocked_reason == Some(EvidenceExitBlockedReason::NoOpenPosition)
            && decision.forced_flat_reasons.is_empty()
        {
            return Ok(());
        }

        let fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let details = exit_decision_details(
            self.config.strategy_id.clone(),
            now_ms,
            &fields,
            self.exit_forced_flat_evidence_inputs(),
        );
        let blocked_reason = fields.submission_blocked_reason;
        let disposition = if action_chosen {
            if details.forced_flat_reasons.is_empty() {
                ExitDecisionDisposition::Exit
            } else {
                ExitDecisionDisposition::ExitFailClosed
            }
        } else if blocked_reason.is_some() {
            ExitDecisionDisposition::Blocked
        } else {
            ExitDecisionDisposition::Hold
        };
        let key = ExitDecisionDedupeKey {
            market_id: details.market_id.clone(),
            position_id: details.position_id.clone(),
            forced_flat_reasons: details.forced_flat_reasons.clone(),
            exit_decision: disposition,
            blocked_reason,
        };
        // An adjacent-repeat guard, not episode novelty: the frozen registry
        // defines no exit-outcome domain, so this suppresses only an
        // immediately repeated decision and re-emits on A-B-A. Recorded as the
        // outstanding migration in the census rather than dressed up as
        // suppression it does not provide. Marked before the write, so a failed
        // write cannot turn a repeated decision into a per-tick retry.
        if self.last_recorded_exit_decision.as_ref() == Some(&key) {
            return Ok(());
        }
        self.last_recorded_exit_decision = Some(key);
        let failure = if action_chosen {
            match self
                .context
                .edge_taker_evidence()
                .expect("edge-taker strategy must own edge-taker evidence")
                .record_exit_intent_decision(ExitIntentDecisionFact {
                    details,
                    outcome: if disposition == ExitDecisionDisposition::ExitFailClosed {
                        ExitIntentOutcome::ExitFailClosed
                    } else {
                        ExitIntentOutcome::Exit
                    },
                }) {
                NonBlockingRecordOutcome::Appended(_) => None,
                NonBlockingRecordOutcome::Failed(error) => Some(error),
            }
        } else {
            match self
                .context
                .edge_taker_evidence()
                .expect("edge-taker strategy must own edge-taker evidence")
                .record_exit_hold_decision(ExitHoldDecisionFact {
                    details,
                    outcome: if disposition == ExitDecisionDisposition::Blocked {
                        ExitHoldOutcome::Blocked
                    } else {
                        ExitHoldOutcome::Hold
                    },
                    blocked_reason,
                }) {
                ObservationRecordOutcome::Appended(_)
                | ObservationRecordOutcome::FailureSuppressed => None,
                ObservationRecordOutcome::FailureReported(error) => Some(error),
            }
        };
        if let Some(error) = failure {
            // A telemetry-write failure must NEVER block a risk-reducing exit:
            // The intent fact is diagnostic and cannot gate a risk-reducing exit.
            // Surface the lost write at the
            // highest non-panicking severity and let the exit proceed.
            log::error!(
                "binary_oracle_edge_taker exit decision evidence write failed: strategy_id={} error={error:#}",
                self.config.strategy_id
            );
        }
        Ok(())
    }

    fn record_exit_prepared_order(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
        decision: &ExitIntentDecision,
        prepared_order: crate::bolt_v3_current_evidence::PreparedOrderLinkage,
    ) {
        let fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let details = exit_decision_details(
            self.config.strategy_id.clone(),
            now_ms,
            &fields,
            self.exit_forced_flat_evidence_inputs(),
        );
        let outcome = if details.forced_flat_reasons.is_empty() {
            ExitIntentOutcome::Exit
        } else {
            ExitIntentOutcome::ExitFailClosed
        };
        if let NonBlockingRecordOutcome::Failed(error) = self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_exit_prepared_order(ExitPreparedOrderFact {
                details,
                outcome,
                prepared_order,
            })
        {
            log::error!(
                "binary_oracle_edge_taker prepared-exit evidence write failed: strategy_id={} error={error:#}",
                self.config.strategy_id
            );
        }
    }

    fn active_book_for_outcome(&self, side: OutcomeSide) -> &OutcomeBookState {
        match side {
            OutcomeSide::Up => &self.active.books.up,
            OutcomeSide::Down => &self.active.books.down,
        }
    }

    fn configured_entry_order_side(&self) -> Result<OrderSide> {
        parse_configured_order_side(CONFIG_FIELD_ENTRY_ORDER_SIDE, &self.config.entry_order.side)
    }

    fn executable_edge_order_shape_block_reason(&self) -> Option<BinaryOutcomeEdgeBlockReason> {
        (!BinaryOracleEdgeTakerBuilder::entry_order_shape_supported(&self.config.entry_order))
            .then_some(BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape)
    }

    fn entry_reject_block_reason_for(
        &self,
        instrument_id: InstrumentId,
        selected_side: OutcomeSide,
    ) -> Option<EvidenceEntrySkipReason> {
        match self.entry_reject_state.get(&instrument_id)? {
            EntryRejectState::Malformed => Some(EvidenceEntrySkipReason::EntryMalformedRejected),
            EntryRejectState::Balance => Some(EvidenceEntrySkipReason::EntryBalanceRejected),
            EntryRejectState::Unfillable { book } => {
                let current_book = self.active_book_for_outcome(selected_side);
                (current_book == book)
                    .then_some(EvidenceEntrySkipReason::EntryUnfillableRejectedUnchangedBook)
            }
        }
    }

    fn record_entry_reject_state(
        &mut self,
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        raw_reason: &str,
    ) {
        let Some(pending) = self.pending_entry().cloned() else {
            return;
        };
        if pending.client_order_id != client_order_id || pending.instrument_id != instrument_id {
            return;
        }

        match classify_entry_reject_reason(raw_reason) {
            Some(EntryRejectClass::Malformed) => {
                self.entry_reject_state
                    .insert(instrument_id, EntryRejectState::Malformed);
            }
            Some(EntryRejectClass::Balance) => {
                self.entry_reject_state
                    .insert(instrument_id, EntryRejectState::Balance);
            }
            Some(EntryRejectClass::Unfillable) => {
                self.entry_reject_state.insert(
                    instrument_id,
                    EntryRejectState::Unfillable {
                        book: pending.book.clone(),
                    },
                );
            }
            None => {
                self.entry_reject_state.insert(
                    instrument_id,
                    EntryRejectState::Unfillable {
                        book: pending.book.clone(),
                    },
                );
            }
        }
    }

    fn record_entry_reject(&mut self, event: &nautilus_model::events::OrderRejected) {
        self.record_entry_reject_state(
            event.client_order_id,
            event.instrument_id,
            event.reason.as_str(),
        );
    }

    fn preliminary_edge_pricing_notional_for_side(&self, side: OutcomeSide) -> f64 {
        let mut notional = self.config.order_notional_target;
        if is_positive_finite(self.config.maximum_position_notional) {
            notional = notional.min(self.config.maximum_position_notional);
        }
        if let Some(impact_cap_notional) = self
            .visible_book_notional_cap(side)
            .filter(|value| is_positive_finite(*value))
        {
            notional = notional.min(impact_cap_notional);
        }
        notional
    }

    fn executable_entry_probe_for_side(
        &self,
        side: OutcomeSide,
        order_side: OrderSide,
        edge_pricing_notional: f64,
    ) -> Result<ExecutableEntryProbe, BinaryOutcomeEdgeBlockReason> {
        let book = self.active_book_for_outcome(side);
        let quote = executable_book_quote(book);
        let vwap = price_exact_size_vwap(
            &quote,
            order_side,
            edge_pricing_notional,
            self.config.vwap_depth_limit_bps,
        )?;
        Ok(ExecutableEntryProbe { order_side, vwap })
    }

    fn executable_edge_for_side(
        &self,
        side: OutcomeSide,
        fair_probability_up: Probability,
        adjusted_probability_up: Probability,
        minimum_edge_bps: f64,
        probe: ExecutableEntryProbe,
    ) -> BinaryOutcomeEdgeResult {
        if let Some(reason) = self.executable_edge_order_shape_block_reason() {
            return BinaryOutcomeEdgeResult::blocked(side, reason);
        }
        let cost_breakdown =
            match executable_cost_breakdown(&probe.vwap, self.config.slippage_buffer_bps) {
                Ok(cost_breakdown) => cost_breakdown,
                Err(reason) => return BinaryOutcomeEdgeResult::blocked(side, reason.into()),
            };
        evaluate_binary_outcome_edge(&BinaryOutcomeEdgeInputs {
            side,
            fair_probability_up: Some(fair_probability_up),
            adjusted_probability_up: Some(adjusted_probability_up),
            order_side: probe.order_side,
            cost_breakdown,
            minimum_edge_bps,
        })
    }

    fn adjusted_probability_up_for_uncertainty(
        &self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
        side: OutcomeSide,
        fair_probability_up: Probability,
    ) -> Option<(Probability, Probability)> {
        let seconds_to_expiry = self.current_seconds_to_expiry_at(now_ms)?;
        let realized_vol = self.current_realized_vol_for_gate_at(receive_context.receive_ms())?;
        let uncertainty_band_probability =
            self.uncertainty_band_probability_for_seconds(seconds_to_expiry, realized_vol)?;
        let adjusted_probability_up = match side {
            OutcomeSide::Up => fair_probability_up.narrowed(uncertainty_band_probability),
            OutcomeSide::Down => fair_probability_up.widened(uncertainty_band_probability),
        };
        Some((uncertainty_band_probability, adjusted_probability_up))
    }

    fn robust_sizing_inputs(
        &self,
        expected_ev_per_notional: f64,
        impact_cap_notional: f64,
    ) -> RobustSizingInputs {
        RobustSizingInputs {
            expected_ev_per_notional,
            ev_reference_per_notional: self.config.sizing_ev_reference_bps as f64 / BPS_DENOMINATOR,
            risk_lambda: self.config.risk_lambda,
            order_notional_target: self.config.order_notional_target,
            maximum_position_notional: self.config.maximum_position_notional,
            impact_cap_notional,
        }
    }

    fn economics_adjusted_entry_edge_ratio(
        &self,
        now_ms: u64,
        selected_side: OutcomeSide,
        edge: BinaryOutcomeEdgeResult,
        probe: &ExecutableEntryProbe,
        minimum_edge_bps: f64,
    ) -> Option<f64> {
        let instrument_id = self.instrument_id_for_side(selected_side)?;
        let planned_fill_legs = probe
            .vwap
            .fill_legs
            .iter()
            .map(|leg| {
                Some(BoltV3PlannedFillLeg {
                    price: Decimal::from_f64(leg.price)?,
                    quantity: Decimal::from_f64(leg.quantity)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let minimum_core_edge_ratio = Decimal::from_f64(minimum_edge_bps / BPS_DENOMINATOR)?;
        let terminal_value_entry = BoltV3TerminalValueEntry::try_new(
            Decimal::from_f64(edge.adjusted_probability)?,
            BoltV3TerminalValueEntryPolicy::MinimumCoreEdgeRatio(minimum_core_edge_ratio),
        )
        .ok()?;
        let requested_at_ns = now_ms.checked_mul(NANOS_PER_MILLI_U64)?;
        let sizing_quote = self
            .context
            .order_economics()
            .quote_taker_sizing(BoltV3TakerEconomicsSizingInput {
                instrument_id,
                order_side: probe.order_side,
                planned_fill_legs,
                terminal_value_entry,
                requested_at_ns,
                decision_correlation_id: &self.config.strategy_id,
            })
            .ok()?;
        sizing_quote
            .net_edge()
            .core_edge_ratio
            .to_f64()
            .filter(|value| is_positive_finite(*value))
    }

    fn visible_book_notional_cap(&self, side: OutcomeSide) -> Option<f64> {
        let order_side = self.configured_entry_order_side().ok()?;
        let book_depth_side =
            visible_book_depth_side_for_order(order_side, self.config.entry_order.is_post_only)?;
        let capped_execution = self
            .active_book_for_outcome(side)
            .max_execution_within_vwap_slippage_bps(
                book_depth_side,
                self.config.book_impact_cap_bps,
            )
            .filter(|execution| is_positive_finite(execution.quantity))?;
        Some(capped_execution.quantity * capped_execution.vwap_price)
    }

    fn instrument_id_for_side(&self, side: OutcomeSide) -> Option<InstrumentId> {
        match side {
            OutcomeSide::Up => self.active.books.up.instrument_id,
            OutcomeSide::Down => self.active.books.down.instrument_id,
        }
    }

    fn current_instrument(&self, instrument_id: InstrumentId) -> Option<InstrumentAny> {
        DataActor::trader_id(self)?;
        let cache = self.cache();
        cache.instrument(&instrument_id)
    }

    fn pending_entry_context_for(&self, instrument_id: InstrumentId) -> Option<PendingEntryState> {
        let pending = self.pending_entry()?.clone();
        if pending.instrument_id != instrument_id {
            return None;
        }

        Some(pending)
    }

    fn build_open_position_state(
        &self,
        preserved: Option<&ManagedPositionContext>,
        pending_context: Option<&PendingEntryState>,
        spec: PositionMaterializationSpec,
        trust_pending_outcome_side: bool,
    ) -> OpenPositionState {
        let position_contract_supported =
            self.configured_position_contract()
                .ok()
                .is_some_and(|contract| {
                    supports_strategy_managed_position(spec.entry_order_side, spec.side, contract)
                });
        let lifecycle = preserved
            .map(|context| context.lifecycle.clone())
            .or_else(|| {
                pending_context.map(|pending| {
                    if trust_pending_outcome_side {
                        pending.lifecycle.clone()
                    } else {
                        pending.lifecycle.clone().without_outcome_side()
                    }
                })
            })
            .unwrap_or_else(BoltV3PositionMarketLifecycle::missing);
        let lifecycle = if position_contract_supported {
            lifecycle
        } else {
            lifecycle.without_outcome_side()
        };
        OpenPositionState {
            lifecycle,
            instrument_id: spec.instrument_id,
            position_id: spec.position_id,
            entry_order_side: spec.entry_order_side,
            side: spec.side,
            quantity: spec.quantity,
            avg_px_open: spec.avg_px_open,
            book: match (
                preserved.map(|context| context.book.clone()),
                pending_context.map(|pending| pending.book.clone()),
            ) {
                (Some(book), _) | (None, Some(book)) => book,
                (None, None) => OutcomeBookState::from_instrument_id(spec.instrument_id),
            },
        }
    }

    fn nt_open_position_projection(
        &self,
        position_id: PositionId,
    ) -> Result<Option<PositionMaterializationSpec>> {
        let execution_venue = self.context.execution_venue();
        let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
        let cache = self.cache();
        let position = cache
            .positions_open(Some(&execution_venue), None, Some(&strategy_id), None, None)
            .into_iter()
            .find(|position| position.id == position_id);
        Ok(position.map(|position| PositionMaterializationSpec {
            instrument_id: position.instrument_id,
            position_id: position.id,
            entry_order_side: position.entry,
            side: position.side,
            quantity: position.quantity,
            avg_px_open: position.avg_px_open,
        }))
    }

    fn nt_canonical_open_position_projection(&self) -> Result<Option<PositionMaterializationSpec>> {
        let execution_venue = self.context.execution_venue();
        let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
        let positions = {
            let cache = self.cache();
            cache
                .positions_open(Some(&execution_venue), None, Some(&strategy_id), None, None)
                .into_iter()
                .map(|position| PositionMaterializationSpec {
                    instrument_id: position.instrument_id,
                    position_id: position.id,
                    entry_order_side: position.entry,
                    side: position.side,
                    quantity: position.quantity,
                    avg_px_open: position.avg_px_open,
                })
                .collect::<Vec<_>>()
        };
        match positions.as_slice() {
            [] => Ok(None),
            [position] => Ok(Some(*position)),
            _ => anyhow::bail!(
                "NT cache contains {} open positions for strategy `{strategy_id}`",
                positions.len()
            ),
        }
    }

    /// Venue-adoption guard shared by every live event path that materializes a
    /// `Managed` position from an external event's `instrument_id` (position
    /// events via `materialize_position_from_event`, entry fills via
    /// `on_order_filled`). A live event must be on the configured execution
    /// venue; NT routes events per strategy/execution-client so a foreign-venue
    /// event should never arrive, but these paths adopt the event `instrument_id`
    /// into `Managed` with no further venue gate and the exit path then submits
    /// against it. Rather than rely only on inherited NT routing, quarantine a
    /// foreign-venue event to blind recovery and signal the caller to stop
    /// before any `Managed`/`ExitPending` transition. Returns `true` when the
    /// event was quarantined (the caller must return early), `false` when the
    /// event is on the execution venue. Mirrors the recovery-path guard in
    /// `bootstrapped_exposure_for` (single `ForeignVenuePosition` classification).
    fn quarantine_foreign_venue_event(&mut self, instrument_id: InstrumentId) -> bool {
        let execution_venue = self.context.execution_venue();
        if instrument_id.venue == execution_venue {
            return false;
        }
        log::error!(
            "binary_oracle_edge_taker live event quarantined foreign-venue instrument: strategy_id={} instrument_id={} instrument_venue={} execution_venue={}",
            self.config.strategy_id,
            instrument_id,
            instrument_id.venue,
            execution_venue,
        );
        self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
            reason: BlindRecoveryReason::ForeignVenuePosition {
                instrument_venue: instrument_id.venue,
                execution_venue,
            },
        });
        self.refresh_book_subscriptions_for_current_state();
        true
    }

    fn event_instrument_matches_held_exposure(
        &mut self,
        event_instrument_id: InstrumentId,
    ) -> bool {
        let Some(held_instrument_id) = self.exposure.held_instrument_id() else {
            return true;
        };
        if event_instrument_id == held_instrument_id {
            return true;
        }
        if self.quarantine_foreign_venue_event(event_instrument_id) {
            return false;
        }
        log::warn!(
            "binary_oracle_edge_taker ignored exposure terminal event for mismatched instrument: strategy_id={} event_instrument_id={} held_instrument_id={}",
            self.config.strategy_id,
            event_instrument_id,
            held_instrument_id,
        );
        false
    }

    fn materialize_position_from_event(
        &mut self,
        spec: PositionMaterializationSpec,
        ts_event_ns: u64,
    ) -> bool {
        if self.quarantine_foreign_venue_event(spec.instrument_id) {
            return false;
        }
        if !is_observed_open_side(spec.side) {
            self.materialize_position_from_truth(
                spec,
                ts_event_ns,
                ORDER_LIFECYCLE_SOURCE_POSITION_EVENT,
            );
            return false;
        }
        match self.nt_open_position_projection(spec.position_id) {
            Ok(Some(nt_spec)) => {
                self.materialize_position_from_truth(
                    nt_spec,
                    ts_event_ns,
                    ORDER_LIFECYCLE_SOURCE_POSITION_EVENT,
                );
                self.managed_position()
                    .is_some_and(|managed| managed.position.position_id == spec.position_id)
            }
            Ok(None) => {
                log::warn!(
                    "binary_oracle_edge_taker ignored position event absent from the NT open-position cache: strategy_id={} position_id={}",
                    self.config.strategy_id,
                    spec.position_id,
                );
                false
            }
            Err(error) => {
                self.enter_blind_settlement_recovery(error);
                false
            }
        }
    }

    fn materialize_position_from_truth(
        &mut self,
        spec: PositionMaterializationSpec,
        ts_event_ns: u64,
        source: OrderLifecycleSource,
    ) {
        let PositionMaterializationSpec {
            instrument_id,
            position_id,
            entry_order_side,
            side,
            quantity,
            avg_px_open,
        } = spec;
        // Venue invariant (defense in depth): a live position event must be on the
        // execution venue, or it would be adopted into Managed and the exit path
        // would submit against a foreign instrument_id. Quarantine before any
        // Managed/ExitPending transition via the shared venue-adoption guard.
        if self.quarantine_foreign_venue_event(instrument_id) {
            return;
        }
        let preserved = self
            .exposure
            .managed_position_context()
            .filter(|managed| {
                managed.position_id == position_id && managed.instrument_id == instrument_id
            })
            .cloned();
        let pending_context = self.pending_entry_context_for(instrument_id);
        let pending_matches = pending_context.is_some();
        let observed_open_side = is_observed_open_side(side);
        let tradable_position_supported =
            self.configured_position_contract()
                .ok()
                .is_some_and(|contract| {
                    supports_strategy_managed_position(entry_order_side, side, contract)
                });

        if !observed_open_side {
            if let Some(pending) = pending_context.clone() {
                let market_id = pending.lifecycle.market_id_owned();
                let client_order_id = pending.client_order_id;
                self.exposure = ExposureState::EntryReconcilePending {
                    pending,
                    reason: EntryReconcileReason::InvalidObservedPosition {
                        entry_order_side,
                        side,
                    },
                };
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition: OrderLifecycleTransition::EntryReconcilePending,
                    outcome: OrderLifecycleOutcome::EntryReconcilePending,
                    source,
                    market_id,
                    instrument_id: Some(instrument_id),
                    position_id: Some(position_id),
                    client_order_id: Some(client_order_id),
                    prior_client_order_id: None,
                    raw_reason_text: None,
                    order_side: Some(entry_order_side),
                    filled_quantity: None,
                    residual_quantity: None,
                    ts_event_ns: Some(ts_event_ns),
                });
            } else {
                self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::InvalidLivePosition {
                        entry_order_side,
                        side: Some(side),
                    },
                });
            }
            log::error!(
                "binary_oracle_edge_taker position event carried unsupported position side: strategy_id={} instrument_id={} position_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                instrument_id,
                position_id,
                entry_order_side,
                side,
            );
            self.refresh_book_subscriptions_for_current_state();
            return;
        }

        if !tradable_position_supported {
            log::error!(
                "binary_oracle_edge_taker quarantining unsupported observed position contract: strategy_id={} instrument_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                instrument_id,
                entry_order_side,
                side,
            );
            self.set_unsupported_observed_exposure(
                self.build_open_position_state(
                    preserved.as_ref(),
                    pending_context.as_ref(),
                    PositionMaterializationSpec {
                        instrument_id,
                        position_id,
                        entry_order_side,
                        side,
                        quantity,
                        avg_px_open,
                    },
                    false,
                ),
                UnsupportedObservedReason::LiveUnsupportedContract,
            );
            return;
        }

        let origin = match self
            .exposure
            .managed_position_context()
            .filter(|managed| {
                managed.position_id == position_id && managed.instrument_id == instrument_id
            })
            .map(|managed| managed.origin)
        {
            Some(origin) => origin,
            None if pending_matches => ManagedPositionOrigin::StrategyEntry,
            None => ManagedPositionOrigin::RecoveryBootstrap,
        };
        let position_truth_rematerialization_override =
            self.take_position_truth_rematerialization_override(instrument_id, origin);
        let materialized_position = self.build_open_position_state(
            preserved.as_ref(),
            pending_context.as_ref(),
            PositionMaterializationSpec {
                instrument_id,
                position_id,
                entry_order_side,
                side,
                quantity,
                avg_px_open,
            },
            pending_matches,
        );
        let rematerialized_market_id = materialized_position.lifecycle.market_id_owned();
        let rematerialized_quantity = materialized_position.quantity;
        let pending_entry = pending_context
            .clone()
            .filter(|pending| self.entry_order_may_remain_working(&pending.client_order_id));
        let managed_context =
            managed_position_context(materialized_position, origin, pending_entry);
        self.exposure = match self.exposure.clone() {
            ExposureState::ExitAttempting(mut attempt)
                if attempt.authority.position_id() == position_id
                    && attempt.authority.instrument_id() == instrument_id =>
            {
                attempt.managed = managed_context;
                ExposureState::ExitAttempting(attempt)
            }
            ExposureState::ExitAttempting(attempt) => ExposureState::ExitAttempting(attempt),
            ExposureState::ExitPending(mut exit_pending)
                if exit_pending.authority.position_id() == position_id
                    && exit_pending.authority.instrument_id() == instrument_id =>
            {
                exit_pending.position = Some(managed_context);
                ExposureState::ExitPending(exit_pending)
            }
            ExposureState::TerminalExitAwaitingPosition(mut exit_pending)
                if exit_pending.authority.position_id() == position_id
                    && exit_pending.authority.instrument_id() == instrument_id =>
            {
                exit_pending.position = Some(managed_context);
                ExposureState::TerminalExitAwaitingPosition(exit_pending)
            }
            ExposureState::ExitPending(exit_pending) => ExposureState::ExitPending(exit_pending),
            ExposureState::TerminalExitAwaitingPosition(exit_pending) => {
                ExposureState::TerminalExitAwaitingPosition(exit_pending)
            }
            ExposureState::ExitAuthorityRecoveryHold(hold)
                if hold.instrument_id == instrument_id
                    && hold
                        .pending_exit
                        .position_id
                        .is_none_or(|held_position_id| held_position_id == position_id) =>
            {
                let mut pending_exit = hold.pending_exit;
                pending_exit.position_id = Some(position_id);
                ExposureState::ExitAuthorityRecoveryHold(ExitAuthorityRecoveryHoldState {
                    position: Some(managed_context),
                    pending_exit,
                    ..hold
                })
            }
            ExposureState::ExitAuthorityRecoveryHold(hold) => {
                ExposureState::ExitAuthorityRecoveryHold(hold)
            }
            ExposureState::Flat
            | ExposureState::PendingEntry(_)
            | ExposureState::EntryReconcilePending { .. }
            | ExposureState::Managed(_)
            | ExposureState::UnsupportedObserved(_)
            | ExposureState::BlindRecovery(_) => ExposureState::Managed(managed_context),
        };
        if self.exposure.exit_authority_recovery_hold().is_some() {
            self.try_recover_exit_authority_hold(ts_event_ns);
        } else {
            self.refresh_exit_authority_baseline();
        }
        if let ExposureState::TerminalExitAwaitingPosition(exit_pending) = &self.exposure {
            let exit_pending = exit_pending.clone();
            self.try_release_terminal_exit(&exit_pending, source, None, ts_event_ns);
        }
        if let Some(terminal_override) = position_truth_rematerialization_override {
            self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                transition: OrderLifecycleTransition::PositionTruthRematerialized,
                outcome: Self::lifecycle_outcome_for_exposure(&self.exposure),
                source,
                market_id: terminal_override.market_id.or(rematerialized_market_id),
                instrument_id: Some(instrument_id),
                position_id: Some(position_id),
                client_order_id: Some(terminal_override.client_order_id),
                prior_client_order_id: None,
                raw_reason_text: None,
                order_side: Some(entry_order_side),
                filled_quantity: None,
                residual_quantity: Some(rematerialized_quantity),
                ts_event_ns: Some(ts_event_ns),
            });
        }
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
    }

    fn enter_exit_authority_recovery_hold(
        &mut self,
        position: Option<ManagedPositionContext>,
        mut pending_exit: PendingExitState,
        instrument_id: InstrumentId,
        plan: ExitAuthorityRecoveryPlan,
        now_ns: u64,
    ) {
        let position_id = pending_exit
            .position_id
            .or_else(|| position.as_ref().map(|position| position.position_id));
        pending_exit.position_id = position_id;
        let flat_recovery = match position_id {
            Some(position_id) => {
                match self.acquire_exit_authority_flat_recovery(instrument_id, position_id) {
                    Ok(authority) => ExitAuthorityFlatRecovery::Armed(authority),
                    Err(error) => {
                        log::error!(
                            "binary_oracle_edge_taker exit recovery flat-proof lease unavailable: strategy_id={} client_order_id={} error={error:#}",
                            self.config.strategy_id,
                            pending_exit.client_order_id,
                        );
                        ExitAuthorityFlatRecovery::AwaitingLease
                    }
                }
            }
            None => ExitAuthorityFlatRecovery::AwaitingLease,
        };
        self.exposure = ExposureState::ExitAuthorityRecoveryHold(ExitAuthorityRecoveryHoldState {
            position,
            instrument_id,
            pending_exit,
            plan,
            flat_recovery,
        });
        self.try_recover_exit_authority_hold(now_ns);
    }

    fn reconcile_exit_order_lifecycle(&mut self, input: ExitOrderLifecycleObservationInput) {
        let ExitOrderLifecycleObservationInput {
            client_order_id,
            instrument_id: event_instrument_id,
            transition,
            source,
            raw_reason_text,
            ts_event_ns,
            authority: observation,
        } = input;
        let Some(exit_pending) = self.exposure.exit_pending_snapshot() else {
            return;
        };
        if exit_pending.pending_exit.client_order_id != client_order_id {
            return;
        }
        if !self.event_instrument_matches_held_exposure(event_instrument_id) {
            return;
        }
        let cached_order = self.cache().order(&client_order_id);
        let Some(cached_order) = cached_order else {
            self.enter_exit_authority_recovery_hold(
                exit_pending.position.clone(),
                exit_pending.pending_exit.clone(),
                event_instrument_id,
                observation.recovery_plan(&exit_pending.authority),
                ts_event_ns,
            );
            return;
        };
        let lifecycle = match exit_pending.authority.observe_order(
            &cached_order,
            ts_event_ns,
            observation.correction(),
        ) {
            Ok(lifecycle) => lifecycle,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker terminal exit authority reconciliation failed: strategy_id={} client_order_id={client_order_id} error={error:#}",
                    self.config.strategy_id,
                );
                self.enter_exit_authority_recovery_hold(
                    exit_pending.position.clone(),
                    exit_pending.pending_exit.clone(),
                    event_instrument_id,
                    observation.recovery_plan(&exit_pending.authority),
                    ts_event_ns,
                );
                return;
            }
        };
        match lifecycle {
            BoltV3ExitOrderLifecycleReduction::Working => {
                if observation.is_correction() {
                    self.exposure = ExposureState::ExitPending(exit_pending);
                }
                return;
            }
            BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition => {
                self.exposure = ExposureState::TerminalExitAwaitingPosition(exit_pending.clone());
                let released = self.try_release_terminal_exit(
                    &exit_pending,
                    source,
                    raw_reason_text.as_deref(),
                    ts_event_ns,
                );
                if released {
                    return;
                }
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition,
                    outcome: OrderLifecycleOutcome::ExitPending,
                    source,
                    market_id: exit_pending.pending_exit.market_id.clone(),
                    instrument_id: Some(event_instrument_id),
                    position_id: exit_pending.pending_exit.position_id,
                    client_order_id: Some(client_order_id),
                    prior_client_order_id: None,
                    raw_reason_text,
                    order_side: Some(cached_order.order_side()),
                    filled_quantity: Some(cached_order.filled_qty()),
                    residual_quantity: None,
                    ts_event_ns: Some(ts_event_ns),
                });
                return;
            }
            BoltV3ExitOrderLifecycleReduction::TerminalZeroFill => {}
        }
        let terminal_market_id = exit_pending.pending_exit.market_id.clone().or_else(|| {
            exit_pending
                .position
                .as_ref()
                .and_then(|managed| managed.lifecycle.market_id_owned())
        });
        let terminal_position_id = exit_pending.pending_exit.position_id.or_else(|| {
            exit_pending
                .position
                .as_ref()
                .map(|managed| managed.position_id)
        });
        let nt_residual = match terminal_position_id {
            Some(position_id) => self.nt_open_position_projection(position_id),
            None => Ok(None),
        };
        if let Ok(Some(spec)) = nt_residual.as_ref() {
            let origin = exit_pending
                .position
                .as_ref()
                .map(|managed| managed.origin)
                .unwrap_or(ManagedPositionOrigin::RecoveryBootstrap);
            let pending_entry = exit_pending
                .position
                .as_ref()
                .and_then(|managed| managed.pending_entry.clone());
            let residual_position = self.build_open_position_state(
                exit_pending.position.as_ref(),
                pending_entry.as_ref(),
                *spec,
                pending_entry.is_some(),
            );
            let residual_quantity = residual_position.quantity;
            self.exposure = ExposureState::Managed(managed_position_context(
                residual_position.clone(),
                origin,
                pending_entry,
            ));
            self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                transition: OrderLifecycleTransition::ResidualRemanaged,
                outcome: OrderLifecycleOutcome::Managed,
                source,
                market_id: residual_position.lifecycle.market_id_owned(),
                instrument_id: Some(residual_position.instrument_id),
                position_id: Some(residual_position.position_id),
                client_order_id: Some(client_order_id),
                prior_client_order_id: None,
                raw_reason_text: raw_reason_text.clone(),
                order_side: None,
                filled_quantity: None,
                residual_quantity: Some(residual_quantity),
                ts_event_ns: Some(ts_event_ns),
            });
            self.sync_exposure_context_from_active();
            self.refresh_book_subscriptions_for_current_state();
            return;
        }
        if let Err(error) = nt_residual {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::CacheProbeFailed,
            });
            log::error!(
                "binary_oracle_edge_taker could not project terminal exit state from NT cache: strategy_id={} error={error:#}",
                self.config.strategy_id,
            );
        } else {
            self.exposure = ExposureState::Flat;
        }
        self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
            transition,
            outcome: Self::lifecycle_outcome_for_exposure(&self.exposure),
            source,
            market_id: terminal_market_id,
            instrument_id: Some(event_instrument_id),
            position_id: terminal_position_id,
            client_order_id: Some(client_order_id),
            prior_client_order_id: None,
            raw_reason_text,
            order_side: None,
            filled_quantity: None,
            residual_quantity: None,
            ts_event_ns: Some(ts_event_ns),
        });
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
    }

    fn reconcile_cached_exit_order_on_timer(&mut self) {
        let Some((phase, exit_pending)) = self.exposure.exit_lifecycle() else {
            return;
        };
        let client_order_id = exit_pending.pending_exit.client_order_id;
        let Some(order) = self.cache().order(&client_order_id) else {
            let now_ns = self.clock().timestamp_ns().as_u64();
            self.enter_exit_authority_recovery_hold(
                exit_pending.position.clone(),
                exit_pending.pending_exit.clone(),
                exit_pending.authority.instrument_id(),
                ExitAuthorityRecoveryPlan::Resume(exit_pending.authority.clone()),
                now_ns,
            );
            return;
        };
        let (transition, raw_reason_text, observation) =
            match classify_cached_exit_order_lifecycle(order.status()) {
                CachedExitOrderLifecycle::Working => (
                    OrderLifecycleTransition::OrderFilled,
                    Some("cached_working"),
                    if phase == ExitLifecyclePhase::TerminalAwaitingPosition {
                        ExitOrderAuthorityObservation::Correction {
                            cause: BoltV3RecoveredExitCause::FillVoidReopen,
                        }
                    } else {
                        ExitOrderAuthorityObservation::Lifecycle
                    },
                ),
                CachedExitOrderLifecycle::Terminal {
                    transition,
                    raw_reason_text,
                    correction,
                } => (
                    transition,
                    raw_reason_text,
                    match correction {
                        BoltV3ExitOrderCorrection::FillAuthorityChanged => {
                            ExitOrderAuthorityObservation::Correction {
                                cause: BoltV3RecoveredExitCause::FillVoidReopen,
                            }
                        }
                        BoltV3ExitOrderCorrection::Unchanged => {
                            ExitOrderAuthorityObservation::Lifecycle
                        }
                    },
                ),
            };
        let terminal_event_ns = order.ts_last().as_u64();
        self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id,
            instrument_id: order.instrument_id(),
            transition,
            source: ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
            raw_reason_text: raw_reason_text.map(str::to_string),
            ts_event_ns: terminal_event_ns,
            authority: observation,
        });
    }

    fn canonical_position_authority(
        &self,
        position_id: PositionId,
        instrument_id: InstrumentId,
    ) -> Result<Option<BoltV3CanonicalPositionAuthority>> {
        let cache = self.cache();
        let Some(position) = cache.position(&position_id) else {
            return Ok(None);
        };
        anyhow::ensure!(
            position.instrument_id == instrument_id,
            "canonical exit position instrument identity mismatch"
        );
        let side = match position.side {
            PositionSide::Long => nautilus_model::enums::PositionSideSpecified::Long,
            PositionSide::Short => nautilus_model::enums::PositionSideSpecified::Short,
            PositionSide::Flat => nautilus_model::enums::PositionSideSpecified::Flat,
            PositionSide::NoPositionSide => {
                anyhow::bail!("canonical exit position has no specified side")
            }
        };
        let target_scope = match parse_configured_oms_type(
            CONFIG_FIELD_OMS_TYPE,
            &self.config.oms_type,
        )? {
            nautilus_model::enums::OmsType::Hedging => {
                crate::bolt_v3_order_execution::BoltV3CanonicalPositionTargetScope::Exact
            }
            nautilus_model::enums::OmsType::Netting => {
                let matching_positions = cache.positions(
                    Some(&instrument_id.venue),
                    Some(&instrument_id),
                    None,
                    Some(&position.account_id),
                    None,
                );
                if matching_positions.len() == 1 && matching_positions[0].id == position_id {
                    crate::bolt_v3_order_execution::BoltV3CanonicalPositionTargetScope::Exact
                } else {
                    crate::bolt_v3_order_execution::BoltV3CanonicalPositionTargetScope::AmbiguousNettingAggregate
                }
            }
            nautilus_model::enums::OmsType::Unspecified => {
                anyhow::bail!("canonical exit position requires a specified OMS type")
            }
        };
        Ok(Some(BoltV3CanonicalPositionAuthority {
            signed_quantity: position.signed_decimal_qty(),
            side,
            trade_ids: position.trade_ids().into_iter().collect(),
            target_scope,
        }))
    }

    fn refresh_exit_authority_baseline(&self) {
        let Some(exit_pending) = self.exposure.exit_pending_snapshot() else {
            return;
        };
        let Some(position_id) = exit_pending.pending_exit.position_id else {
            return;
        };
        let Some(instrument_id) = exit_pending
            .position
            .as_ref()
            .map(|position| position.instrument_id)
        else {
            return;
        };
        let canonical = match self.canonical_position_authority(position_id, instrument_id) {
            Ok(canonical) => canonical,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker recovered exit baseline cache read failed: strategy_id={} client_order_id={} error={error:#}",
                    self.config.strategy_id,
                    exit_pending.pending_exit.client_order_id,
                );
                return;
            }
        };
        if let Err(error) = exit_pending
            .authority
            .refresh_recovered_baseline(canonical.as_ref())
        {
            log::error!(
                "binary_oracle_edge_taker recovered exit baseline remains unavailable: strategy_id={} client_order_id={} error={error:#}",
                self.config.strategy_id,
                exit_pending.pending_exit.client_order_id,
            );
        }
    }

    fn try_recover_exit_authority_hold(&mut self, now_ns: u64) {
        let Some(mut hold) = self.exposure.exit_authority_recovery_hold().cloned() else {
            return;
        };
        let position_id = hold.pending_exit.position_id.or_else(|| {
            let cache = self.cache();
            cache
                .order(&hold.pending_exit.client_order_id)
                .and_then(|order| order.position_id())
                .or_else(|| cache.position_id(&hold.pending_exit.client_order_id))
        });
        let Some(position_id) = position_id else {
            log::error!(
                "binary_oracle_edge_taker exit authority recovery lacks position attribution: strategy_id={} client_order_id={}",
                self.config.strategy_id,
                hold.pending_exit.client_order_id,
            );
            return;
        };
        if matches!(hold.flat_recovery, ExitAuthorityFlatRecovery::AwaitingLease) {
            match self.acquire_exit_authority_flat_recovery(hold.instrument_id, position_id) {
                Ok(authority) => {
                    hold.flat_recovery = ExitAuthorityFlatRecovery::Armed(authority);
                }
                Err(error) => {
                    log::error!(
                        "binary_oracle_edge_taker exit recovery flat-proof lease remains unavailable: strategy_id={} client_order_id={} error={error:#}",
                        self.config.strategy_id,
                        hold.pending_exit.client_order_id,
                    );
                }
            }
        }
        let recovered = match &hold.plan {
            ExitAuthorityRecoveryPlan::Reconstruct(cause) => self.recover_exit_authority_state(
                hold.position.clone(),
                hold.instrument_id,
                position_id,
                hold.pending_exit.clone(),
                *cause,
            ),
            ExitAuthorityRecoveryPlan::Resume(authority) => (|| {
                let canonical = self
                    .canonical_position_authority(position_id, hold.instrument_id)
                    .and_then(|canonical| {
                        canonical
                            .context("exit authority recovery still lacks its canonical position")
                    })?;
                authority.refresh_recovered_baseline(Some(&canonical))?;
                Ok(ExitPendingState {
                    position: hold.position.clone(),
                    pending_exit: hold.pending_exit.clone(),
                    authority: authority.clone(),
                })
            })(),
        };
        let attempt = match recovered {
            Err(error) => ExitAuthorityRecoveryAttempt::Blocked(
                ExitAuthorityRecoveryBlock::Construction(error),
            ),
            Ok(recovered) => match self.cache().order(&hold.pending_exit.client_order_id) {
                None => ExitAuthorityRecoveryAttempt::Blocked(
                    ExitAuthorityRecoveryBlock::CachedOrderMissing,
                ),
                Some(order) => {
                    let correction = match &hold.plan {
                        ExitAuthorityRecoveryPlan::Reconstruct(
                            BoltV3RecoveredExitCause::FillVoidReopen,
                        ) => BoltV3ExitOrderCorrection::FillAuthorityChanged,
                        ExitAuthorityRecoveryPlan::Reconstruct(
                            BoltV3RecoveredExitCause::StartupAdoption,
                        ) => BoltV3ExitOrderCorrection::Unchanged,
                        ExitAuthorityRecoveryPlan::Resume(_) => {
                            match classify_cached_exit_order_lifecycle(order.status()) {
                                CachedExitOrderLifecycle::Terminal { correction, .. } => correction,
                                CachedExitOrderLifecycle::Working => {
                                    BoltV3ExitOrderCorrection::Unchanged
                                }
                            }
                        }
                    };
                    match recovered
                        .authority
                        .observe_order(&order, now_ns, correction)
                    {
                        Ok(BoltV3ExitOrderLifecycleReduction::Working) => {
                            ExitAuthorityRecoveryAttempt::Working(recovered)
                        }
                        Ok(BoltV3ExitOrderLifecycleReduction::TerminalAwaitingPosition) => {
                            ExitAuthorityRecoveryAttempt::TerminalAwaitingPosition(recovered)
                        }
                        Ok(BoltV3ExitOrderLifecycleReduction::TerminalZeroFill) => {
                            ExitAuthorityRecoveryAttempt::Blocked(
                                ExitAuthorityRecoveryBlock::InvalidTerminalZeroFill,
                            )
                        }
                        Err(error) => ExitAuthorityRecoveryAttempt::Blocked(
                            ExitAuthorityRecoveryBlock::Observation(error),
                        ),
                    }
                }
            },
        };
        match attempt {
            ExitAuthorityRecoveryAttempt::TerminalAwaitingPosition(recovered) => {
                self.exposure = ExposureState::TerminalExitAwaitingPosition(recovered.clone());
                self.try_release_terminal_exit(
                    &recovered,
                    ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
                    Some("exit_authority_recovered"),
                    now_ns,
                );
            }
            ExitAuthorityRecoveryAttempt::Working(recovered) => {
                self.exposure = ExposureState::ExitPending(recovered);
            }
            ExitAuthorityRecoveryAttempt::Blocked(reason) => {
                if self.try_release_exit_authority_recovery_flat(&hold, position_id, now_ns) {
                    return;
                }
                self.exposure = ExposureState::ExitAuthorityRecoveryHold(hold);
                log::error!(
                    "binary_oracle_edge_taker exit authority recovery remains held: strategy_id={} reason={reason}",
                    self.config.strategy_id,
                );
            }
        }
    }

    fn try_release_exit_authority_recovery_flat(
        &mut self,
        hold: &ExitAuthorityRecoveryHoldState,
        position_id: PositionId,
        now_ns: u64,
    ) -> bool {
        if self
            .cache()
            .order(&hold.pending_exit.client_order_id)
            .is_some_and(|order| !order.is_closed())
        {
            return false;
        }
        let ExitAuthorityFlatRecovery::Armed(authority) = &hold.flat_recovery else {
            return false;
        };
        let canonical = match self.canonical_position_authority(position_id, hold.instrument_id) {
            Ok(canonical) => canonical,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker exit recovery flat proof could not read canonical position: strategy_id={} client_order_id={} error={error:#}",
                    self.config.strategy_id,
                    hold.pending_exit.client_order_id,
                );
                return false;
            }
        };
        let release = match authority.release_flat(canonical.as_ref()) {
            Ok(release) => release,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker exit recovery flat proof failed: strategy_id={} client_order_id={} error={error:#}",
                    self.config.strategy_id,
                    hold.pending_exit.client_order_id,
                );
                return false;
            }
        };
        match release {
            BoltV3ExitAuthorityRecoveryRelease::AwaitingAuthority => false,
            BoltV3ExitAuthorityRecoveryRelease::Flat => {
                self.exposure = ExposureState::Flat;
                if let Some(market_id) = hold.pending_exit.market_id.as_deref() {
                    self.arm_market_cooldown(market_id, now_ns / NANOS_PER_MILLI_U64);
                }
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition: OrderLifecycleTransition::ResidualRemanaged,
                    outcome: OrderLifecycleOutcome::Flat,
                    source: ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
                    market_id: hold.pending_exit.market_id.clone(),
                    instrument_id: Some(hold.instrument_id),
                    position_id: Some(position_id),
                    client_order_id: Some(hold.pending_exit.client_order_id),
                    prior_client_order_id: None,
                    raw_reason_text: Some("exit_authority_recovery_flat_proven".to_string()),
                    order_side: None,
                    filled_quantity: None,
                    residual_quantity: None,
                    ts_event_ns: Some(now_ns),
                });
                self.sync_exposure_context_from_active();
                self.refresh_book_subscriptions_for_current_state();
                true
            }
        }
    }

    fn try_release_terminal_exit(
        &mut self,
        exit_pending: &ExitPendingState,
        source: OrderLifecycleSource,
        raw_reason_text: Option<&str>,
        ts_event_ns: u64,
    ) -> bool {
        let Some(position_id) = exit_pending.pending_exit.position_id else {
            return false;
        };
        let Some(instrument_id) = exit_pending
            .position
            .as_ref()
            .map(|position| position.instrument_id)
        else {
            return false;
        };
        let canonical = match self.canonical_position_authority(position_id, instrument_id) {
            Ok(canonical) => canonical,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker canonical position authority read failed: strategy_id={} client_order_id={} error={error:#}",
                    self.config.strategy_id,
                    exit_pending.pending_exit.client_order_id,
                );
                return false;
            }
        };
        let release = match exit_pending.authority.release(canonical.as_ref()) {
            Ok(release) => release,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker terminal exit fence evaluation failed: strategy_id={} client_order_id={} error={error:#}",
                    self.config.strategy_id,
                    exit_pending.pending_exit.client_order_id,
                );
                return false;
            }
        };
        match release {
            BoltV3PositionReductionRelease::AwaitingAuthority => false,
            BoltV3PositionReductionRelease::Flat => {
                self.exposure = ExposureState::Flat;
                if let Some(market_id) = exit_pending.pending_exit.market_id.as_deref() {
                    self.arm_market_cooldown(market_id, ts_event_ns / NANOS_PER_MILLI_U64);
                }
                self.sync_exposure_context_from_active();
                self.refresh_book_subscriptions_for_current_state();
                true
            }
            BoltV3PositionReductionRelease::Residual { signed_quantity } => {
                let residual = match self.nt_open_position_projection(position_id) {
                    Ok(Some(residual))
                        if residual.quantity.as_decimal() == signed_quantity.abs() =>
                    {
                        residual
                    }
                    Ok(_) => return false,
                    Err(error) => {
                        log::error!(
                            "binary_oracle_edge_taker proven residual projection failed: strategy_id={} client_order_id={} error={error:#}",
                            self.config.strategy_id,
                            exit_pending.pending_exit.client_order_id,
                        );
                        return false;
                    }
                };
                let origin = exit_pending
                    .position
                    .as_ref()
                    .map(|position| position.origin)
                    .unwrap_or(ManagedPositionOrigin::RecoveryBootstrap);
                let pending_entry = exit_pending
                    .position
                    .as_ref()
                    .and_then(|position| position.pending_entry.clone());
                let residual_position = self.build_open_position_state(
                    exit_pending.position.as_ref(),
                    pending_entry.as_ref(),
                    residual,
                    pending_entry.is_some(),
                );
                let residual_quantity = residual_position.quantity;
                self.exposure = ExposureState::Managed(managed_position_context(
                    residual_position.clone(),
                    origin,
                    pending_entry,
                ));
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition: OrderLifecycleTransition::ResidualRemanaged,
                    outcome: OrderLifecycleOutcome::Managed,
                    source,
                    market_id: residual_position.lifecycle.market_id_owned(),
                    instrument_id: Some(instrument_id),
                    position_id: Some(position_id),
                    client_order_id: Some(exit_pending.pending_exit.client_order_id),
                    prior_client_order_id: None,
                    raw_reason_text: raw_reason_text.map(str::to_string),
                    order_side: None,
                    filled_quantity: None,
                    residual_quantity: Some(residual_quantity),
                    ts_event_ns: Some(ts_event_ns),
                });
                self.sync_exposure_context_from_active();
                self.refresh_book_subscriptions_for_current_state();
                true
            }
        }
    }

    fn sync_exposure_context_from_active(&mut self) {
        let active_market_id = self.active.market_id.clone();
        let active_strike_price = self.active.price_to_beat;
        let active_interval_end_ms = self.active.interval_end_ms;
        let active_interval_open = self.active.interval_open;
        let active_selection_published_at_ms = self.active.selection_published_at_ms;
        let active_seconds_to_expiry_at_selection = self.active.seconds_to_expiry_at_selection;
        let active_up_instrument_id = self.active.books.up.instrument_id;
        let active_down_instrument_id = self.active.books.down.instrument_id;
        let active_up_book = self.active.books.up.clone();
        let active_down_book = self.active.books.down.clone();
        let allow_missing_interval_lifecycle_repair = self
            .exposure
            .managed_position_context()
            .is_some_and(|managed| managed.origin == ManagedPositionOrigin::StrategyEntry);
        let Some((lifecycle, instrument_id, book)) = self.tracked_position_context_mut() else {
            return;
        };

        if active_up_instrument_id == Some(instrument_id) {
            if !lifecycle.market_matches_or_missing(active_market_id.as_deref()) {
                return;
            }
            let active_lifecycle = BoltV3PositionMarketLifecycle::from_entry_context(
                active_market_id.clone(),
                Some(OutcomeSide::Up),
                active_strike_price,
                active_interval_open,
                active_interval_end_ms,
                active_selection_published_at_ms,
                active_seconds_to_expiry_at_selection,
            );
            let lifecycle_repair_allowed = lifecycle.interval_end_matches(&active_lifecycle)
                || (allow_missing_interval_lifecycle_repair
                    && lifecycle.interval_end_ms().is_none());
            if lifecycle_repair_allowed {
                lifecycle.fill_missing_from(&active_lifecycle);
            }
            if lifecycle.interval_end_matches(&active_lifecycle) {
                *book = active_up_book;
            }
        } else if active_down_instrument_id == Some(instrument_id) {
            if !lifecycle.market_matches_or_missing(active_market_id.as_deref()) {
                return;
            }
            let active_lifecycle = BoltV3PositionMarketLifecycle::from_entry_context(
                active_market_id,
                Some(OutcomeSide::Down),
                active_strike_price,
                active_interval_open,
                active_interval_end_ms,
                active_selection_published_at_ms,
                active_seconds_to_expiry_at_selection,
            );
            let lifecycle_repair_allowed = lifecycle.interval_end_matches(&active_lifecycle)
                || (allow_missing_interval_lifecycle_repair
                    && lifecycle.interval_end_ms().is_none());
            if lifecycle_repair_allowed {
                lifecycle.fill_missing_from(&active_lifecycle);
            }
            if lifecycle.interval_end_matches(&active_lifecycle) {
                *book = active_down_book;
            }
        }
    }

    fn desired_book_subscriptions_for_active(&self) -> OutcomeBookSubscriptions {
        let mut next = OutcomeBookSubscriptions {
            up_instrument_id: self.active.books.up.instrument_id,
            down_instrument_id: self.active.books.down.instrument_id,
            tracked_position_instrument_id: None,
        };

        if let Some(open_position) = self.tracked_observed_position()
            && next.up_instrument_id != Some(open_position.instrument_id)
            && next.down_instrument_id != Some(open_position.instrument_id)
        {
            next.tracked_position_instrument_id = Some(open_position.instrument_id);
        } else if let Some(pending_entry_instrument_id) =
            self.pending_entry().map(|pending| pending.instrument_id)
            && next.up_instrument_id != Some(pending_entry_instrument_id)
            && next.down_instrument_id != Some(pending_entry_instrument_id)
        {
            next.tracked_position_instrument_id = Some(pending_entry_instrument_id);
        }

        next
    }

    fn refresh_book_subscriptions_for_current_state(&mut self) {
        let next = self.desired_book_subscriptions_for_active();
        if should_replace_book_subscriptions(&self.book_subscriptions, &next) {
            self.replace_book_subscriptions(next);
        }
    }

    fn open_position_outcome_side(&self) -> Option<OutcomeSide> {
        self.managed_position()
            .and_then(|position| position.position.lifecycle.outcome_side())
    }

    fn configured_position_contract(&self) -> Result<ConfiguredPositionContract> {
        Ok(ConfiguredPositionContract {
            entry_order_side: parse_configured_order_side(
                CONFIG_FIELD_ENTRY_ORDER_SIDE,
                &self.config.entry_order.side,
            )?,
            entry_position_side: parse_configured_position_side(
                CONFIG_FIELD_ENTRY_ORDER_POSITION_SIDE,
                &self.config.entry_order.position_side,
            )?,
            exit_order_side: parse_configured_order_side(
                CONFIG_FIELD_EXIT_ORDER_SIDE,
                &self.config.exit_order.side,
            )?,
            exit_position_side: parse_configured_position_side(
                CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE,
                &self.config.exit_order.position_side,
            )?,
        })
    }

    fn open_position_effective_entry_cost(&self) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let contract = self.configured_position_contract().ok()?;
        managed_position_effective_entry_cost(
            open_position,
            contract.entry_order_side,
            contract.entry_position_side,
        )
    }

    fn current_exit_order_for_open_position_with_config(
        &self,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<(OrderSide, f64)> {
        let open_position = &self.managed_position()?.position;
        if open_position.side != order_config.position_side {
            return None;
        }

        let order_side = order_config.side;
        let price = match order_config.order_template.order_type {
            OrderType::StopMarket | OrderType::MarketIfTouched => {
                order_config.order_template.trigger_price
            }
            OrderType::TrailingStopMarket => order_config
                .order_template
                .trigger_price
                .or(order_config.order_template.activation_price),
            _ => order_price_for_side(
                &open_position.book,
                order_side,
                order_config.order_template.is_post_only,
            ),
        }?;

        Some((order_side, price)).filter(|(_, price)| is_positive_finite(*price))
    }

    fn current_exit_value_for_open_position_with_config(
        &self,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<f64> {
        self.current_exit_order_for_open_position_with_config(order_config)
            .map(|(_, price)| price)
    }

    fn current_position_market_id(&self) -> Option<String> {
        self.exposure.current_position_market_id()
    }

    #[cfg(test)]
    fn current_position_fair_probability_up_for_gate_at(
        &self,
        now_ms: u64,
        realized_vol_gate_receive_ms: LocalReceiveMs,
    ) -> Option<Probability> {
        let realized_vol = self.current_realized_vol_for_gate_at(realized_vol_gate_receive_ms)?;
        self.current_position_fair_probability_up_for_realized_vol_at(now_ms, realized_vol)
    }

    fn current_position_fair_probability_up_for_realized_vol_at(
        &self,
        now_ms: u64,
        realized_vol: f64,
    ) -> Option<Probability> {
        let open_position = &self.managed_position()?.position;
        let spot_price = self.current_position_spot_price()?;
        let strike_price = open_position.lifecycle.settlement_strike()?;
        let seconds_to_expiry = open_position.lifecycle.seconds_to_expiry_at(now_ms)?;
        bolt_v3_market_families::fair_probability_up_for_family(
            &self.config.rotating_market_family,
            &FairProbabilityInputs {
                spot_price,
                strike_price,
                seconds_to_market_end: seconds_to_expiry,
                realized_vol,
                pricing_kurtosis: self.config.pricing_kurtosis,
            },
        )
    }

    #[cfg(test)]
    fn current_position_fair_probability_up_at(&self, now_ms: u64) -> Option<Probability> {
        self.current_position_fair_probability_up_for_gate_at(now_ms, LocalReceiveMs::new(now_ms))
    }

    fn current_position_uncertainty_band_probability_for_realized_vol_at(
        &self,
        now_ms: u64,
        realized_vol: f64,
    ) -> Option<Probability> {
        let seconds_to_expiry = self
            .managed_position()?
            .position
            .lifecycle
            .seconds_to_expiry_at(now_ms)?;
        self.uncertainty_band_probability_for_seconds(seconds_to_expiry, realized_vol)
    }

    #[cfg(test)]
    fn current_hold_ev_bps_for_gate_at(
        &self,
        now_ms: u64,
        side: OutcomeSide,
        realized_vol_gate_receive_ms: LocalReceiveMs,
    ) -> Option<f64> {
        let fair_probability_up = self.current_position_fair_probability_up_for_gate_at(
            now_ms,
            realized_vol_gate_receive_ms,
        )?;
        self.current_hold_ev_bps_for_fair_probability(side, fair_probability_up)
    }

    fn current_hold_ev_bps_for_fair_probability(
        &self,
        side: OutcomeSide,
        fair_probability_up: Probability,
    ) -> Option<f64> {
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        if !is_positive_finite(effective_entry_cost) {
            return None;
        }
        let success_probability = match side {
            OutcomeSide::Up => fair_probability_up,
            OutcomeSide::Down => fair_probability_up.complement(),
        };

        Some(
            ((success_probability.value() - effective_entry_cost) / effective_entry_cost)
                * BPS_DENOMINATOR,
        )
    }

    #[cfg(test)]
    fn current_hold_ev_bps_at(&self, now_ms: u64, side: OutcomeSide) -> Option<f64> {
        self.current_hold_ev_bps_for_gate_at(now_ms, side, LocalReceiveMs::new(now_ms))
    }

    fn current_exit_ev_bps_at(
        &self,
        _side: OutcomeSide,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<f64> {
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        if !is_positive_finite(effective_entry_cost) {
            return None;
        }

        let current_exit_value =
            self.current_exit_value_for_open_position_with_config(order_config)?;
        if !is_positive_finite(current_exit_value) {
            return None;
        }

        Some(((current_exit_value - effective_entry_cost) / effective_entry_cost) * BPS_DENOMINATOR)
    }

    fn exit_realized_volatility_gate_receipt_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> ExitRealizedVolatilityGateReceipt {
        let evaluation_receive_ms = trigger_context.receive_ms();
        let max_source_age_ms = self.config.realized_volatility_max_source_age_ms;
        let snapshot = self
            .pricing
            .latest_realized_vol_snapshot_for_surface(&self.config.realized_volatility_surface_id);
        let gate_result =
            classify_rv_gate(snapshot, evaluation_receive_ms, Some(max_source_age_ms));
        let snapshot_as_of_ms = snapshot.map(|snapshot| snapshot.as_of_ms);
        let snapshot_receive_watermark_ms =
            snapshot.and_then(|snapshot| snapshot.latest_accepted_receive_ms);
        let snapshot_ready = snapshot.is_some_and(|snapshot| snapshot.ready);
        let ready_realized_vol = snapshot
            .and_then(|snapshot| snapshot.ready_realized_vol())
            .map(|value| value.get());
        let snapshot_has_ready_realized_vol = ready_realized_vol.is_some();
        let accepted = gate_result == EvidenceRvGateResult::Accepted;
        let realized_vol = if accepted { ready_realized_vol } else { None };
        let realized_vol_source_venue = if accepted {
            snapshot.and_then(|snapshot| snapshot.sources_used.first().cloned())
        } else {
            None
        };
        let realized_vol_source_ts_ms = if accepted { snapshot_as_of_ms } else { None };
        let raw_snapshot_blockers = snapshot
            .map(|snapshot| snapshot.blocked_reasons.clone())
            .unwrap_or_default();
        let source_diagnostics = snapshot
            .map(|snapshot| {
                snapshot
                    .source_diagnostics
                    .iter()
                    .map(realized_vol_diagnostic_fact)
                    .collect()
            })
            .unwrap_or_default();
        let snapshot_as_of_minus_trigger_event_ms = snapshot_as_of_ms
            .zip(trigger_context.venue_event_ms().map(VenueEventMs::value))
            .map(|(snapshot_as_of_ms, trigger_event_ms)| {
                i128::from(snapshot_as_of_ms) - i128::from(trigger_event_ms)
            });
        let fair_probability_up = realized_vol.and_then(|realized_vol| {
            self.current_position_fair_probability_up_for_realized_vol_at(now_ms, realized_vol)
        });
        let uncertainty_band_probability = realized_vol.and_then(|realized_vol| {
            self.current_position_uncertainty_band_probability_for_realized_vol_at(
                now_ms,
                realized_vol,
            )
        });

        ExitRealizedVolatilityGateReceipt {
            gate_result,
            surface_id: self.config.realized_volatility_surface_id.clone(),
            max_source_age_ms,
            evaluation_receive_ms,
            snapshot_as_of_ms,
            snapshot_receive_watermark_ms,
            snapshot_ready,
            snapshot_has_ready_realized_vol,
            realized_vol,
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
            raw_snapshot_blockers,
            source_diagnostics,
            snapshot_as_of_minus_trigger_event_ms,
            fair_probability_up: fair_probability_up.map(Probability::value),
            fair_probability_down: fair_probability_up.map(|value| value.complement().value()),
            uncertainty_band_probability: uncertainty_band_probability.map(Probability::value),
        }
    }

    fn exit_evaluation_with_receipt_at(
        &self,
        now_ms: u64,
        realized_volatility_receipt: ExitRealizedVolatilityGateReceipt,
    ) -> ExitEvaluation {
        let mut evaluation = ExitEvaluation {
            realized_volatility_receipt,
            position_outcome_side: self.open_position_outcome_side(),
            forced_flat_reasons: self.position_forced_flat_reasons_at(now_ms),
            hold_ev_bps: None,
            exit_ev_bps: None,
            exit_decision: None,
            blocked_reason: None,
        };

        if self.managed_position().is_none() {
            evaluation.blocked_reason = Some(EvidenceExitBlockedReason::NoOpenPosition);
            return evaluation;
        }
        if self.exposure.exit_pending_snapshot().is_some() {
            evaluation.blocked_reason = Some(EvidenceExitBlockedReason::ExitAlreadyPending);
            return evaluation;
        }

        if self
            .managed_position()
            .is_some_and(|managed| managed.position.lifecycle.interval_ended_at(now_ms))
        {
            evaluation.blocked_reason = Some(EvidenceExitBlockedReason::PositionIntervalEnded);
            return evaluation;
        }
        if self
            .managed_position()
            .is_some_and(|managed| managed.position.lifecycle.interval_end_ms().is_none())
        {
            evaluation.blocked_reason = Some(EvidenceExitBlockedReason::PositionIntervalUnknown);
            return evaluation;
        }

        if !evaluation.forced_flat_reasons.is_empty() {
            evaluation.exit_decision = Some(ExitDecision::Exit);
            return evaluation;
        }

        if self
            .exposure
            .managed_position_context()
            .and_then(|managed| managed.pending_entry.as_ref())
            .is_some()
        {
            evaluation.blocked_reason = Some(EvidenceExitBlockedReason::EntryOrderStillWorking);
            return evaluation;
        }

        let Some(position_outcome_side) = evaluation.position_outcome_side else {
            evaluation.exit_decision = Some(ExitDecision::Hold);
            return evaluation;
        };

        let Ok(order_config) = self.normal_exit_order_execution_config() else {
            evaluation.blocked_reason = Some(EvidenceExitBlockedReason::ExitOrderConfigInvalid);
            return evaluation;
        };
        evaluation.hold_ev_bps = evaluation
            .realized_volatility_receipt
            .fair_probability_up
            .and_then(Probability::new)
            .and_then(|fair_probability_up| {
                self.current_hold_ev_bps_for_fair_probability(
                    position_outcome_side,
                    fair_probability_up,
                )
            });
        evaluation.exit_ev_bps = self.current_exit_ev_bps_at(position_outcome_side, &order_config);
        evaluation.exit_decision = Some(evaluate_exit_decision(
            evaluation.hold_ev_bps,
            evaluation.exit_ev_bps,
            self.config.exit_hysteresis_bps as f64,
        ));
        evaluation
    }

    #[cfg(test)]
    fn exit_evaluation_at(&self, now_ms: u64) -> ExitEvaluation {
        self.exit_evaluation_for_trigger_at(now_ms, ExitEvaluationTriggerContext::unknown(now_ms))
    }

    fn exit_evaluation_for_trigger_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> ExitEvaluation {
        let receipt = self.exit_realized_volatility_gate_receipt_at(now_ms, trigger_context);
        self.exit_evaluation_with_receipt_at(now_ms, receipt)
    }

    #[cfg(test)]
    fn exit_intent_decision_at(&self, now_ms: u64) -> ExitIntentDecision {
        let evaluation = self.exit_evaluation_at(now_ms);
        self.exit_intent_decision_from_evaluation(evaluation)
    }

    fn exit_intent_decision_for_trigger_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> ExitIntentDecision {
        let evaluation = self.exit_evaluation_for_trigger_at(now_ms, trigger_context);
        self.exit_intent_decision_from_evaluation(evaluation)
    }

    fn exit_intent_decision_from_evaluation(
        &self,
        evaluation: ExitEvaluation,
    ) -> ExitIntentDecision {
        let blocked_reason = evaluation.blocked_reason;
        let forced_flat_reasons = evaluation.forced_flat_reasons.clone();
        let forced_flat = !forced_flat_reasons.is_empty();
        let exit_decision = evaluation.exit_decision;
        let mut decision = ExitIntentDecision {
            evaluation,
            instrument_id: None,
            order_type: None,
            order_side: None,
            position_side: None,
            time_in_force: None,
            price: None,
            quantity: None,
            client_order_id: None,
            is_post_only: None,
            is_reduce_only: None,
            is_quote_quantity: None,
            expire_time_unix_nanos: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            blocked_reason,
            forced_flat_reasons,
        };

        if blocked_reason == Some(EvidenceExitBlockedReason::EntryOrderStillWorking) {
            return decision;
        }

        let Some(exit_decision) = exit_decision else {
            // No exit decision was produced: preserve the precise evaluation-level
            // block reason (e.g. ExitAlreadyPending, NoOpenPosition) so the recorded
            // decision trace names the real cause; only synthesize the generic
            // ExitDecisionUnavailable when the evaluation supplied no reason at all.
            decision.blocked_reason =
                blocked_reason.or(Some(EvidenceExitBlockedReason::ExitDecisionUnavailable));
            return decision;
        };
        if exit_decision == ExitDecision::Hold {
            decision.blocked_reason = Some(EvidenceExitBlockedReason::ExitHold);
            return decision;
        }

        let Some(managed_position) = self.managed_position() else {
            decision.blocked_reason = Some(EvidenceExitBlockedReason::OpenPositionMissing);
            return decision;
        };
        let open_position = &managed_position.position;
        let Ok(order_config) = self.exit_order_execution_config(forced_flat) else {
            decision.blocked_reason = Some(EvidenceExitBlockedReason::ExitOrderConfigInvalid);
            return decision;
        };
        if order_config.order_template.is_quote_quantity {
            decision.blocked_reason = Some(EvidenceExitBlockedReason::ExitQuoteQuantityUnsupported);
            return decision;
        }
        let Some((order_side, price)) =
            self.current_exit_order_for_open_position_with_config(&order_config)
        else {
            decision.blocked_reason = Some(EvidenceExitBlockedReason::ExitPriceMissing);
            return decision;
        };
        if !is_positive_finite(open_position.quantity.as_f64()) {
            decision.blocked_reason = Some(EvidenceExitBlockedReason::ExitQuantityNotPositive);
            return decision;
        }

        decision.instrument_id = Some(open_position.instrument_id);
        decision.order_type = Some(order_config.order_template.order_type);
        decision.order_side = Some(order_side);
        decision.position_side = Some(order_config.position_side);
        decision.time_in_force = Some(order_config.order_template.time_in_force);
        decision.price = Some(price);
        decision.quantity = Some(open_position.quantity);
        decision.is_post_only = Some(order_config.order_template.is_post_only);
        decision.is_reduce_only = Some(order_config.order_template.is_reduce_only);
        decision.is_quote_quantity = Some(order_config.order_template.is_quote_quantity);
        decision.expire_time_unix_nanos = order_config.order_template.expire_time_unix_nanos;
        decision.trigger_price = order_config.order_template.trigger_price;
        decision.activation_price = order_config.order_template.activation_price;
        decision.trigger_type = order_config.order_template.trigger_type;
        decision.trigger_instrument_id = order_config.order_template.trigger_instrument_id;
        decision.trailing_offset = order_config.order_template.trailing_offset;
        decision.trailing_offset_type = order_config.order_template.trailing_offset_type;
        decision.blocked_reason = None;
        decision
    }

    fn exit_evaluation_log_fields_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
        decision: &ExitIntentDecision,
    ) -> ExitEvaluationLogFields {
        let managed_position = self.managed_position();
        let open_position = managed_position.as_ref().map(|managed| &managed.position);
        let receipt = &decision.evaluation.realized_volatility_receipt;
        let rv_snapshot_blockers = receipt
            .raw_snapshot_blockers
            .iter()
            .copied()
            .map(Into::into)
            .collect();
        let rv_future_dating_delta_ms = receipt
            .snapshot_as_of_minus_trigger_event_ms
            .filter(|delta_ms| delta_ms.is_positive())
            .and_then(|delta_ms| u64::try_from(delta_ms).ok());
        let fast_venue_available = self.pricing.selected_pricing_spot().is_some();
        ExitEvaluationLogFields {
            market_id: self.current_position_market_id(),
            phase: self.active.phase,
            position_outcome_side: decision.evaluation.position_outcome_side,
            position_id: open_position.map(|position| position.position_id),
            position_instrument_id: open_position.map(|position| position.instrument_id),
            position_quantity: open_position.map(|position| position.quantity),
            position_avg_px_open: open_position.map(|position| position.avg_px_open),
            forced_flat_reasons: decision.forced_flat_reasons.clone(),
            spot_price: self.current_position_spot_price(),
            spot_venue_name: self
                .current_position_fast_spot()
                .map(|spot| spot.venue.clone()),
            fast_venue_available,
            reference_current_price: self.pricing.last_reference_current_price(),
            interval_open: open_position
                .and_then(|position| position.lifecycle.settlement_strike()),
            seconds_to_expiry: open_position
                .and_then(|position| position.lifecycle.seconds_to_expiry_at(now_ms)),
            realized_vol: receipt.realized_vol,
            realized_vol_source_venue: receipt.realized_vol_source_venue.clone(),
            realized_vol_source_ts_ms: receipt.realized_vol_source_ts_ms,
            rv_surface_id: receipt.surface_id.clone(),
            rv_snapshot_as_of_ms: receipt.snapshot_as_of_ms,
            rv_snapshot_ready: receipt.snapshot_ready,
            rv_snapshot_has_ready_realized_vol: Some(receipt.snapshot_has_ready_realized_vol),
            rv_snapshot_receive_watermark_ms: receipt
                .snapshot_receive_watermark_ms
                .map(LocalReceiveMs::value),
            rv_max_source_age_ms: Some(receipt.max_source_age_ms),
            rv_snapshot_blockers,
            rv_source_diagnostics: receipt.source_diagnostics.clone(),
            rv_gate_result: receipt.gate_result,
            rv_future_dating_delta_ms,
            exit_eval_now_ms: now_ms,
            exit_trigger_source: trigger_context.source,
            trigger_ts_event_ms: trigger_context.ts_event_ms,
            trigger_ts_init_ms: receipt.evaluation_receive_ms.map(LocalReceiveMs::value),
            pricing_kurtosis: self.config.pricing_kurtosis,
            exit_hysteresis_bps: self.config.exit_hysteresis_bps,
            fair_probability_up: receipt.fair_probability_up,
            fair_probability_down: receipt.fair_probability_down,
            uncertainty_band_probability: receipt.uncertainty_band_probability,
            hold_ev_bps: decision.evaluation.hold_ev_bps,
            exit_ev_bps: decision.evaluation.exit_ev_bps,
            exit_decision: decision.evaluation.exit_decision,
            submission_instrument_id: decision.instrument_id,
            submission_order_side: decision.order_side,
            submission_price: decision.price,
            submission_quantity: decision.quantity,
            submission_client_order_id: decision.client_order_id,
            submission_blocked_reason: decision.blocked_reason,
        }
    }

    fn log_exit_evaluation(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
        decision: &ExitIntentDecision,
    ) {
        let fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let blocked = fields.submission_blocked_reason.is_some();
        if blocked {
            if should_warn_on_exit_submission_block(fields.submission_blocked_reason) {
                log::warn!(
                    "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                    self.config.strategy_id,
                    fields.market_id,
                    fields.phase,
                    fields.position_outcome_side,
                    fields.position_id,
                    fields.position_instrument_id,
                    fields.position_quantity,
                    fields.position_avg_px_open,
                    fields.forced_flat_reasons,
                    fields.spot_price,
                    fields.spot_venue_name,
                    fields.reference_current_price,
                    fields.interval_open,
                    fields.seconds_to_expiry,
                    fields.realized_vol,
                    fields.realized_vol_source_venue,
                    fields.realized_vol_source_ts_ms,
                    fields.pricing_kurtosis,
                    fields.exit_hysteresis_bps,
                    fields.fair_probability_up,
                    fields.fair_probability_down,
                    fields.uncertainty_band_probability,
                    fields.hold_ev_bps,
                    fields.exit_ev_bps,
                    fields.exit_decision,
                    fields.submission_instrument_id,
                    fields.submission_order_side,
                    fields.submission_price,
                    fields.submission_quantity,
                    fields.submission_client_order_id,
                    fields
                        .submission_blocked_reason
                        .map(exit_block_reason_label),
                );
            } else {
                log::debug!(
                    "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                    self.config.strategy_id,
                    fields.market_id,
                    fields.phase,
                    fields.position_outcome_side,
                    fields.position_id,
                    fields.position_instrument_id,
                    fields.position_quantity,
                    fields.position_avg_px_open,
                    fields.forced_flat_reasons,
                    fields.spot_price,
                    fields.spot_venue_name,
                    fields.reference_current_price,
                    fields.interval_open,
                    fields.seconds_to_expiry,
                    fields.realized_vol,
                    fields.realized_vol_source_venue,
                    fields.realized_vol_source_ts_ms,
                    fields.pricing_kurtosis,
                    fields.exit_hysteresis_bps,
                    fields.fair_probability_up,
                    fields.fair_probability_down,
                    fields.uncertainty_band_probability,
                    fields.hold_ev_bps,
                    fields.exit_ev_bps,
                    fields.exit_decision,
                    fields.submission_instrument_id,
                    fields.submission_order_side,
                    fields.submission_price,
                    fields.submission_quantity,
                    fields.submission_client_order_id,
                    fields
                        .submission_blocked_reason
                        .map(exit_block_reason_label),
                );
            }
        } else {
            log::info!(
                "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                self.config.strategy_id,
                fields.market_id,
                fields.phase,
                fields.position_outcome_side,
                fields.position_id,
                fields.position_instrument_id,
                fields.position_quantity,
                fields.position_avg_px_open,
                fields.forced_flat_reasons,
                fields.spot_price,
                fields.spot_venue_name,
                fields.reference_current_price,
                fields.interval_open,
                fields.seconds_to_expiry,
                fields.realized_vol,
                fields.realized_vol_source_venue,
                fields.realized_vol_source_ts_ms,
                fields.pricing_kurtosis,
                fields.exit_hysteresis_bps,
                fields.fair_probability_up,
                fields.fair_probability_down,
                fields.uncertainty_band_probability,
                fields.hold_ev_bps,
                fields.exit_ev_bps,
                fields.exit_decision,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity,
                fields.submission_client_order_id,
                fields
                    .submission_blocked_reason
                    .map(exit_block_reason_label),
            );
        }
    }

    #[cfg(test)]
    fn submit_order_with_decision_evidence(
        &mut self,
        intent: OrderIntentDetails,
        intent_kind: BoltV3SubmitIntentKind,
        order: nautilus_model::orders::OrderAny,
        submit_context: BoltV3SubmitContext,
    ) -> Result<BoltV3SubmitAttemptOutcome> {
        self.submit_order_with_decision_evidence_inner(
            intent,
            intent_kind,
            order,
            submit_context,
            StrategyEconomicsInput::CompiledFinalOrderFixture,
        )
    }

    #[cfg(test)]
    fn submit_order_with_decision_evidence_inner(
        &mut self,
        intent: OrderIntentDetails,
        intent_kind: BoltV3SubmitIntentKind,
        order: nautilus_model::orders::OrderAny,
        submit_context: BoltV3SubmitContext,
        economics_input: StrategyEconomicsInput,
    ) -> Result<BoltV3SubmitAttemptOutcome> {
        let (intent, order, sealed) =
            self.prepare_order_economics_submission(intent, intent_kind, order, economics_input)?;
        Ok(self.route_prepared_order_submission(intent, order, sealed, submit_context))
    }

    fn prepare_order_economics_submission(
        &self,
        intent: OrderIntentDetails,
        intent_kind: BoltV3SubmitIntentKind,
        order: nautilus_model::orders::OrderAny,
        economics_input: StrategyEconomicsInput,
    ) -> Result<(
        OrderIntentDetails,
        nautilus_model::orders::OrderAny,
        crate::bolt_v3_submit_admission::BoltV3EconomicsSubmitAdmission,
    )> {
        let sealed = self.economics_submit_admission_from_order(
            &intent,
            intent_kind,
            &order,
            economics_input,
        )?;
        Ok((intent, order, sealed))
    }

    fn route_prepared_order_submission(
        &mut self,
        intent: OrderIntentDetails,
        order: nautilus_model::orders::OrderAny,
        sealed: crate::bolt_v3_submit_admission::BoltV3EconomicsSubmitAdmission,
        submit_context: BoltV3SubmitContext,
    ) -> BoltV3SubmitAttemptOutcome {
        let decision_evidence = self
            .context
            .order_execution_evidence()
            .expect("edge-taker strategy must own order-intent evidence");
        let submit_admission = self.context.submit_admission_arc();
        let policy = self.context.order_execution_policy();
        let routing = BoltV3SubmitRoutingRequest::with_economics(
            &decision_evidence,
            submit_admission.as_ref(),
            intent,
            sealed,
        );
        policy.route_submit(routing, self, order, submit_context)
    }

    fn cancel_resting_order(
        &mut self,
        client_order_id: ClientOrderId,
        client_id: ClientId,
    ) -> Result<()> {
        self.context.order_execution_policy().route_cancel(
            self,
            client_order_id,
            Some(client_id),
            None,
        )?;
        Ok(())
    }

    #[cfg(test)]
    fn submit_admission_request_from_order(
        &self,
        intent: &OrderIntentDetails,
        intent_kind: BoltV3SubmitIntentKind,
        order: &nautilus_model::orders::OrderAny,
    ) -> Result<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest> {
        let client_order_id = order.client_order_id().to_string();
        let is_quote_quantity = order.is_quote_quantity();
        let instrument = if is_quote_quantity {
            Some(self.current_instrument(order.instrument_id()).with_context(|| {
                format!(
                    "bolt-v3 submit admission missing instrument context for quote-quantity client_order_id={client_order_id}"
                )
            })?)
        } else if order.price().is_none() {
            self.current_instrument(order.instrument_id())
        } else {
            None
        };
        let (last_quote, last_trade) = if is_quote_quantity {
            let cache = self.cache();
            (
                cache.quote(&order.instrument_id()),
                cache.trade(&order.instrument_id()),
            )
        } else {
            (None, None)
        };
        let risk_reducing_exit_position_context = if intent_kind != BoltV3SubmitIntentKind::Entry {
            let managed_position = self.managed_position().ok_or_else(|| {
                anyhow::anyhow!(
                    "bolt-v3 submit admission risk-reducing exit requires managed position state for client_order_id={client_order_id}"
                )
            })?;
            let position_quantity =
                Decimal::from_f64(managed_position.position.quantity.as_f64()).with_context(
                    || {
                        format!(
                            "bolt-v3 submit admission position quantity is not a decimal for client_order_id={client_order_id}"
                        )
                    },
                )?;
            Some((
                managed_position.position.position_id.to_string(),
                managed_position.position.instrument_id.to_string(),
                managed_position.position.side,
                position_quantity,
            ))
        } else {
            None
        };
        let risk_reducing_exit_position = risk_reducing_exit_position_context.as_ref().map(
            |(position_id, instrument_id, position_side, position_quantity)| {
                BoltV3RiskReducingExitPositionInput {
                    position_id: position_id.as_str(),
                    instrument_id: instrument_id.as_str(),
                    position_side: *position_side,
                    position_quantity: *position_quantity,
                }
            },
        );
        crate::bolt_v3_submit_admission::build_submit_admission_request_from_order_for_test(
            BoltV3SubmitAdmissionRequestInput {
                execution_client_id: &self.config.client_id,
                intent,
                intent_kind,
                order,
                valuation: OrderValuationContext {
                    last_quote,
                    last_trade,
                    instrument: instrument.as_ref(),
                },
                risk_reducing_exit_position,
            },
        )
    }

    fn economics_submit_admission_from_order(
        &self,
        intent: &OrderIntentDetails,
        intent_kind: BoltV3SubmitIntentKind,
        order: &nautilus_model::orders::OrderAny,
        economics_input: StrategyEconomicsInput,
    ) -> Result<crate::bolt_v3_submit_admission::BoltV3EconomicsSubmitAdmission> {
        let client_order_id = order.client_order_id().to_string();
        let is_quote_quantity = order.is_quote_quantity();
        let instrument = if is_quote_quantity {
            Some(self.current_instrument(order.instrument_id()).with_context(|| {
                format!(
                    "bolt-v3 submit admission missing instrument context for quote-quantity client_order_id={}",
                    client_order_id
                )
            })?)
        } else if order.price().is_none() {
            self.current_instrument(order.instrument_id())
        } else {
            None
        };
        let (last_quote, last_trade) = if is_quote_quantity {
            let cache = self.cache();
            (
                cache.quote(&order.instrument_id()),
                cache.trade(&order.instrument_id()),
            )
        } else {
            (None, None)
        };
        let risk_reducing_exit_position_context = if intent_kind != BoltV3SubmitIntentKind::Entry {
            let managed_position = self.managed_position().ok_or_else(|| {
                anyhow::anyhow!(
                    "bolt-v3 submit admission risk-reducing exit requires managed position state for client_order_id={client_order_id}"
                )
            })?;
            let position_quantity =
                Decimal::from_f64(managed_position.position.quantity.as_f64()).with_context(
                    || {
                        format!(
                            "bolt-v3 submit admission position quantity is not a decimal for client_order_id={client_order_id}"
                        )
                    },
                )?;
            Some((
                managed_position.position.position_id.to_string(),
                managed_position.position.instrument_id.to_string(),
                managed_position.position.side,
                position_quantity,
            ))
        } else {
            None
        };
        let risk_reducing_exit_position = risk_reducing_exit_position_context.as_ref().map(
            |(position_id, instrument_id, position_side, position_quantity)| {
                BoltV3RiskReducingExitPositionInput {
                    position_id: position_id.as_str(),
                    instrument_id: instrument_id.as_str(),
                    position_side: *position_side,
                    position_quantity: *position_quantity,
                }
            },
        );

        let admission_input = BoltV3SubmitAdmissionRequestInput {
            execution_client_id: &self.config.client_id,
            intent,
            intent_kind,
            order,
            valuation: OrderValuationContext {
                last_quote,
                last_trade,
                instrument: instrument.as_ref(),
            },
            risk_reducing_exit_position: risk_reducing_exit_position.clone(),
        };
        #[cfg(test)]
        let facts = crate::bolt_v3_submit_admission::order_admission_facts(&admission_input)?;
        #[cfg(test)]
        let planned_exit_scenario = |stored_entry_cost_per_unit: Decimal| {
            let managed_position = self.managed_position().ok_or_else(|| {
                anyhow::anyhow!(
                    "risk-reducing economics requires managed position state for client_order_id={client_order_id}"
                )
            })?;
            let position = self.context.order_economics().planned_exit_position(
                managed_position.position.position_id,
                managed_position.position.side,
                facts.order_quantity,
            )?;
            BoltV3FinalOrderEconomicsScenario::planned_risk_reducing_exit(
                stored_entry_cost_per_unit,
                position,
            )
        };
        let (scenario, candidate_fill_levels) = match economics_input {
            StrategyEconomicsInput::TerminalEntry {
                terminal_value_entry,
                candidate_fill_levels,
            } => (
                BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(terminal_value_entry),
                candidate_fill_levels,
            ),
            #[cfg(test)]
            StrategyEconomicsInput::CompiledFinalOrderFixture => {
                let quantity = if order.is_quote_quantity() {
                    let raw_base = facts
                        .order_quantity
                        .checked_div(facts.price)
                        .context("test economics fill quantity division failed")?;
                    let increment = instrument
                        .as_ref()
                        .context("quote-quantity test economics requires instrument context")?
                        .size_increment()
                        .as_decimal();
                    raw_base
                        .checked_div(increment)
                        .context("test economics size-increment division failed")?
                        .ceil()
                        .checked_mul(increment)
                        .context("test economics size-increment multiplication failed")?
                } else {
                    facts.order_quantity
                };
                let scenario = match intent_kind {
                    BoltV3SubmitIntentKind::Entry => {
                        BoltV3FinalOrderEconomicsScenario::TerminalValueEntry(
                            BoltV3TerminalValueEntry::try_new(
                                facts
                                    .price
                                    .checked_add(Decimal::ONE)
                                    .context("test terminal value overflow")?,
                                BoltV3TerminalValueEntryPolicy::Breakeven,
                            )?,
                        )
                    }
                    BoltV3SubmitIntentKind::RiskReducingExit => planned_exit_scenario(facts.price)?,
                    BoltV3SubmitIntentKind::KillSwitchForcedReduction => {
                        anyhow::bail!("edge-taker fixture cannot construct forced reduction")
                    }
                };
                (
                    scenario,
                    vec![BoltV3PlannedFillLeg {
                        price: facts.price,
                        quantity,
                    }],
                )
            }
        };
        anyhow::ensure!(
            scenario.intent_kind() == intent_kind,
            "strategy economics scenario does not match the routed submit intent"
        );
        build_order_economics_submit_admission(
            self.context.order_economics(),
            BoltV3FinalOrderEconomicsInput {
                execution_client_id: &self.config.client_id,
                intent,
                order,
                valuation: admission_input.valuation,
                risk_reducing_exit_position,
                scenario,
                candidate_fill_levels,
                requested_at_ns: order.ts_init().as_u64(),
                decision_correlation_id: order.client_order_id().as_str(),
            },
        )
    }

    #[cfg(test)]
    fn realized_volatility_evidence_fields(&self) -> RealizedVolatilityEvidenceFields {
        self.realized_volatility_evidence_fields_from_snapshot(
            self.pricing.latest_realized_vol_snapshot_for_surface(
                &self.config.realized_volatility_surface_id,
            ),
        )
    }

    fn realized_volatility_evidence_fields_from_snapshot(
        &self,
        realized_volatility_snapshot: Option<
            &crate::bolt_v3_realized_volatility::RealizedVolSnapshot,
        >,
    ) -> RealizedVolatilityEvidenceFields {
        match realized_volatility_snapshot {
            Some(snapshot) => RealizedVolatilityEvidenceFields {
                surface_id: snapshot.surface_id.clone(),
                as_of_ms: Some(snapshot.as_of_ms),
                annualized_decimal: snapshot
                    .annualized_realized_vol_decimal
                    .map(evidence_number),
                measured_annualized_decimal: snapshot
                    .measured_annualized_realized_vol_decimal
                    .map(evidence_number),
                noise_robust_annualized_decimal: snapshot
                    .noise_robust_annualized_realized_vol_decimal
                    .map(evidence_number),
                continuous_annualized_decimal: snapshot
                    .continuous_annualized_realized_vol_decimal
                    .map(evidence_number),
                jump_annualized_decimal: snapshot
                    .jump_annualized_realized_vol_decimal
                    .map(evidence_number),
                forecast_annualized_decimal: snapshot
                    .forecast_annualized_realized_vol_decimal
                    .map(evidence_number),
                pricing_component: Some(snapshot.pricing_component.into()),
                seconds_per_annum: evidence_number(snapshot.seconds_per_annum),
                aggregation: Some(snapshot.aggregate_method.into()),
                sources_used: snapshot.sources_used.clone(),
                source_diagnostics: snapshot
                    .source_diagnostics
                    .iter()
                    .map(realized_vol_diagnostic_fact)
                    .collect(),
                unknown_source_rejections: snapshot.unknown_source_rejections.clone(),
                blockers: snapshot
                    .blocked_reasons
                    .iter()
                    .map(|reason| (*reason).into())
                    .collect(),
                config_fingerprint: snapshot.config_fingerprint.clone(),
            },
            None => RealizedVolatilityEvidenceFields {
                surface_id: String::new(),
                as_of_ms: None,
                annualized_decimal: None,
                measured_annualized_decimal: None,
                noise_robust_annualized_decimal: None,
                continuous_annualized_decimal: None,
                jump_annualized_decimal: None,
                forecast_annualized_decimal: None,
                pricing_component: None,
                seconds_per_annum: String::new(),
                aggregation: None,
                sources_used: Vec::new(),
                source_diagnostics: Vec::new(),
                unknown_source_rejections: BTreeMap::new(),
                blockers: Vec::new(),
                config_fingerprint: String::new(),
            },
        }
    }

    fn entry_realized_volatility_receipt_at(
        &self,
        evaluation_receive_ms: LocalReceiveMs,
    ) -> EntryRealizedVolatilityReceipt {
        let classification = self.pricing.classify_realized_vol_snapshot(
            &self.config.realized_volatility_surface_id,
            evaluation_receive_ms,
            Some(self.config.realized_volatility_max_source_age_ms),
        );
        match classification {
            RealizedVolGateClassification::Accepted(accepted) => {
                let evidence = self
                    .realized_volatility_evidence_fields_from_snapshot(Some(&accepted.snapshot));
                EntryRealizedVolatilityReceipt {
                    gate_result: EvidenceRvGateResult::Accepted,
                    receive_watermark_ms: Some(accepted.receive_watermark_ms),
                    realized_vol: Some(accepted.ready_realized_vol),
                    source_venue: accepted.source_venue,
                    source_ts_ms: Some(accepted.source_as_of_ms),
                    evidence,
                }
            }
            RealizedVolGateClassification::Rejected {
                gate_result,
                snapshot,
            } => {
                let receive_watermark_ms = snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.latest_accepted_receive_ms);
                let evidence =
                    self.realized_volatility_evidence_fields_from_snapshot(snapshot.as_ref());
                EntryRealizedVolatilityReceipt {
                    gate_result,
                    receive_watermark_ms,
                    realized_vol: None,
                    source_venue: None,
                    source_ts_ms: None,
                    evidence,
                }
            }
        }
    }

    fn strategy_input_rv_state(receipt: &EntryRealizedVolatilityReceipt) -> StrategyInputRvState {
        match receipt.evidence.to_durable_snapshot() {
            Some(snapshot) => StrategyInputRvState::Present {
                selected_annualized_decimal: receipt.realized_vol.map(evidence_number),
                gate_result: receipt.gate_result,
                receive_watermark_ms: receipt.receive_watermark_ms.map(LocalReceiveMs::value),
                snapshot: Box::new(snapshot),
            },
            None => StrategyInputRvState::Absent {
                gate_result: receipt.gate_result,
                receive_watermark_ms: receipt.receive_watermark_ms.map(LocalReceiveMs::value),
            },
        }
    }

    fn blocked_entry_strategy_input_evidence_snapshot_at(
        &self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
    ) -> Result<BlockedStrategyInputObservationFact> {
        let market_selection_outcome =
            strategy_input_market_selection_outcome(self.active.market_selection_outcome);
        let reference_quote_ts_event = self.active.last_reference_ts_ms.ok_or_else(|| {
            anyhow::anyhow!(
                "blocked entry strategy input evidence requires reference quote timestamp"
            )
        })?;
        let seconds_to_market_end = self.current_seconds_to_expiry_at(now_ms).ok_or_else(|| {
            anyhow::anyhow!("blocked entry strategy input evidence requires seconds to market end")
        })?;
        let expected_edge_basis_points = decision
            .evaluation
            .expected_ev_per_notional
            .filter(|value| value.is_finite())
            .map(|value| value * BPS_DENOMINATOR);
        let worst_case_edge_basis_points = match decision.evaluation.selected_side {
            Some(OutcomeSide::Up) => decision.evaluation.up_worst_case_ev_bps,
            Some(OutcomeSide::Down) => decision.evaluation.down_worst_case_ev_bps,
            None => None,
        };
        let reference_current_price = self.evidence_reference_current_price();
        let fast_venue_available = self.pricing.selected_pricing_spot().is_some();
        let reference_current_price_available =
            self.pricing.last_reference_current_price().is_some();

        Ok(BlockedStrategyInputObservationFact {
            details: StrategyInputDetails {
                strategy_id: self.config.strategy_id.clone(),
                configured_target_id: self.config.configured_target_id.clone(),
                market_selection_ruleset_id: self.config.configured_target_id.clone(),
                market_selection_outcome,
                market_id: self.active.market_id.clone(),
                polymarket_condition_id: self
                    .active
                    .source_identity
                    .as_ref()
                    .map(|identity| identity.condition_id.clone()),
                polymarket_market_slug: self
                    .active
                    .source_identity
                    .as_ref()
                    .map(|identity| identity.market_slug.clone()),
                polymarket_question_id: self
                    .active
                    .source_identity
                    .as_ref()
                    .map(|identity| identity.question_id.clone()),
                up_instrument_id: self
                    .active
                    .books
                    .up
                    .instrument_id
                    .map(|instrument_id| instrument_id.to_string()),
                down_instrument_id: self
                    .active
                    .books
                    .down
                    .instrument_id
                    .map(|instrument_id| instrument_id.to_string()),
                market_selection_timestamp_ms: self.active.selection_published_at_ms,
                selected_market_observed_timestamp_ms: self.active.selection_published_at_ms,
                polymarket_market_start_timestamp_ms: self.active.interval_start_ms,
                polymarket_market_end_timestamp_ms: self.active.interval_end_ms,
                price_to_beat_source: self.config.price_to_beat_source.clone(),
                price_to_beat_value: self
                    .active
                    .price_to_beat
                    .filter(|value| is_positive_finite(*value))
                    .map(evidence_number),
                reference_quote_ts_event,
                spot_price: self.evidence_spot_price().map(evidence_number),
                fast_venue_available,
                reference_current_price: reference_current_price.map(evidence_number),
                reference_current_price_available,
                reference_current_price_source_id: self
                    .evidence_reference_current_price_source_id(),
                reference_current_price_failed_over: self
                    .evidence_reference_current_price_failed_over(),
                realized_volatility: Self::strategy_input_rv_state(
                    &decision.evaluation.realized_volatility_receipt,
                ),
                seconds_to_market_end,
                pricing_kurtosis: evidence_number(self.config.pricing_kurtosis),
                theta_decay_factor: evidence_number(self.config.theta_decay_factor),
                theta_scaled_min_edge_bps: decision
                    .evaluation
                    .min_worst_case_ev_bps
                    .filter(|value| value.is_finite())
                    .map(evidence_number),
                fair_probability_up: decision
                    .evaluation
                    .fair_probability_up
                    .map(probability_evidence),
                uncertainty_band_probability: decision
                    .evaluation
                    .uncertainty_band_probability
                    .map(probability_evidence),
                expected_edge_basis_points: expected_edge_basis_points
                    .filter(|value| value.is_finite())
                    .map(evidence_number),
                worst_case_edge_basis_points: worst_case_edge_basis_points
                    .filter(|value| value.is_finite())
                    .map(evidence_number),
                up_worst_case_edge_basis_points: option_evidence_number(
                    decision.evaluation.up_worst_case_ev_bps,
                ),
                down_worst_case_edge_basis_points: option_evidence_number(
                    decision.evaluation.down_worst_case_ev_bps,
                ),
                gate_blocked_by: decision
                    .evaluation
                    .gate
                    .blocked_by
                    .iter()
                    .map(entry_block_reason_to_evidence)
                    .collect(),
                pricing_blocked_by: decision
                    .evaluation
                    .pricing_blocked_by
                    .iter()
                    .map(entry_pricing_block_reason_to_evidence)
                    .collect(),
                fast_venue_name: self.evidence_spot_venue_name(),
                fast_venue_age_ms: self.pricing.last_fast_venue_age_ms,
                fast_venue_jitter_ms: self.pricing.last_fast_venue_jitter_ms,
                fast_venue_incoherent: self.pricing.fast_venue_incoherent,
                lead_agreement_corr: option_evidence_probability(
                    self.pricing.last_lead_agreement_corr,
                ),
                selected_side: decision
                    .evaluation
                    .selected_side
                    .map(outcome_side_to_evidence),
            },
        })
    }

    fn record_blocked_entry_strategy_input_snapshot_once(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
    ) -> Result<()> {
        // The episode is the market; the novelty axis is the RV gate result
        // paired with watermark presence -- the registry's twelve-state domain.
        // The snapshot's own fields carried surface ids, blocker lists and
        // per-source states, all of which churn while the market does not.
        // Unattributable without a bound market -- see the matching note in
        // `record_entry_skip_once` for why this is `Ok` and not an error.
        let Ok(episode) = self.evidence_episode_id() else {
            log::debug!(
                "blocked strategy-input snapshot has no bound market episode, so it is not \
                 recorded as episode evidence"
            );
            return Ok(());
        };
        let state = blocked_strategy_input_canonical_state(
            decision.evaluation.realized_volatility_receipt.gate_result,
            decision
                .evaluation
                .realized_volatility_receipt
                .receive_watermark_ms
                .is_some(),
        );
        // Claim before payload construction and append. A malformed telemetry
        // snapshot or broken sink stays bounded to one attempt for this
        // registered state and cannot abort or flood the strategy callback.
        if !self
            .blocked_strategy_input_novelty
            .claim_once(&episode, state)?
        {
            return Ok(());
        }
        let snapshot =
            match self.blocked_entry_strategy_input_evidence_snapshot_at(now_ms, decision) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    log::error!(
                        "blocked strategy-input observation payload construction failed: {error}"
                    );
                    return Ok(());
                }
            };
        if let ObservationRecordOutcome::FailureReported(error) = self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_blocked_strategy_input_observation(snapshot)
        {
            log::error!("blocked strategy-input observation failed: {error}");
        }
        Ok(())
    }

    fn entry_strategy_input_evidence_snapshot_at(
        &self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        client_order_id: ClientOrderId,
        price: &Price,
        quantity: &Quantity,
    ) -> Result<SubmitLinkedStrategyInputSnapshotFact> {
        let price_to_beat = self
            .active
            .price_to_beat
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires source-bound price_to_beat")
            })?;
        let interval_open = self
            .active
            .interval_open
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires positive interval_open")
            })?;
        if (interval_open - price_to_beat).abs() > f64::EPSILON {
            anyhow::bail!(
                "entry strategy input evidence requires interval_open to match source-bound price_to_beat"
            );
        }
        let reference_quote_ts_event = self.active.last_reference_ts_ms.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires reference quote timestamp")
        })?;
        let spot_price = self
            .pricing
            .spot_price()
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| anyhow::anyhow!("entry strategy input evidence requires spot price"))?;
        let realized_volatility = decision
            .evaluation
            .realized_volatility_receipt
            .realized_vol
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires realized volatility")
            })?;
        let seconds_to_market_end = self.current_seconds_to_expiry_at(now_ms).ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires seconds to market end")
        })?;
        let selected_side = decision.evaluation.selected_side.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires selected side")
        })?;
        let theta_scaled_min_edge_bps = decision
            .evaluation
            .min_worst_case_ev_bps
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires theta-scaled minimum edge")
            })?;
        let fair_probability_up = decision.evaluation.fair_probability_up.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires fair probability")
        })?;
        let uncertainty_band_probability = decision
            .evaluation
            .uncertainty_band_probability
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires uncertainty band")
            })?;
        let expected_edge_basis_points = decision
            .evaluation
            .expected_ev_per_notional
            .filter(|value| value.is_finite())
            .map(|value| value * BPS_DENOMINATOR)
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires expected edge")
            })?;
        let worst_case_edge_basis_points = match selected_side {
            OutcomeSide::Up => decision.evaluation.up_worst_case_ev_bps,
            OutcomeSide::Down => decision.evaluation.down_worst_case_ev_bps,
        }
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires selected worst-case edge")
        })?;
        let market_start_timestamp_ms = self.active.interval_start_ms.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires market start timestamp")
        })?;
        let market_selection_timestamp_ms =
            self.active.selection_published_at_ms.ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires market selection timestamp")
            })?;
        let market_end_timestamp_ms = self.active.interval_end_ms.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires market end timestamp")
        })?;
        let market_selection_outcome =
            strategy_input_market_selection_outcome(self.active.market_selection_outcome);
        let realized_volatility_snapshot = decision
            .evaluation
            .realized_volatility_receipt
            .evidence
            .to_durable_snapshot()
            .context("entry strategy input evidence requires a complete RV snapshot")?;
        let instrument_id = decision.instrument_id.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires submission instrument id")
        })?;
        let order_side = decision.order_side.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires submission order side")
        })?;
        let reference_current_price = self.evidence_reference_current_price();
        let fast_venue_available = self.pricing.selected_pricing_spot().is_some();
        let reference_current_price_available =
            self.pricing.last_reference_current_price().is_some();
        Ok(SubmitLinkedStrategyInputSnapshotFact {
            details: StrategyInputDetails {
                strategy_id: self.config.strategy_id.clone(),
                configured_target_id: self.config.configured_target_id.clone(),
                market_selection_ruleset_id: self.config.configured_target_id.clone(),
                market_selection_outcome,
                market_id: self.active.market_id.clone(),
                polymarket_condition_id: self
                    .active
                    .source_identity
                    .as_ref()
                    .map(|identity| identity.condition_id.clone()),
                polymarket_market_slug: self
                    .active
                    .source_identity
                    .as_ref()
                    .map(|identity| identity.market_slug.clone()),
                polymarket_question_id: self
                    .active
                    .source_identity
                    .as_ref()
                    .map(|identity| identity.question_id.clone()),
                up_instrument_id: self
                    .active
                    .books
                    .up
                    .instrument_id
                    .map(|instrument_id| instrument_id.to_string()),
                down_instrument_id: self
                    .active
                    .books
                    .down
                    .instrument_id
                    .map(|instrument_id| instrument_id.to_string()),
                market_selection_timestamp_ms: Some(market_selection_timestamp_ms),
                selected_market_observed_timestamp_ms: Some(market_selection_timestamp_ms),
                polymarket_market_start_timestamp_ms: Some(market_start_timestamp_ms),
                polymarket_market_end_timestamp_ms: Some(market_end_timestamp_ms),
                price_to_beat_source: self.config.price_to_beat_source.clone(),
                price_to_beat_value: evidence_number(price_to_beat),
                reference_quote_ts_event,
                spot_price: evidence_number(spot_price),
                fast_venue_available,
                reference_current_price: reference_current_price.map(evidence_number),
                reference_current_price_available,
                reference_current_price_source_id: self
                    .evidence_reference_current_price_source_id(),
                reference_current_price_failed_over: self
                    .evidence_reference_current_price_failed_over(),
                realized_volatility: StrategyInputRvState::Present {
                    selected_annualized_decimal: Some(evidence_number(realized_volatility)),
                    gate_result: decision.evaluation.realized_volatility_receipt.gate_result,
                    receive_watermark_ms: decision
                        .evaluation
                        .realized_volatility_receipt
                        .receive_watermark_ms
                        .map(LocalReceiveMs::value),
                    snapshot: Box::new(realized_volatility_snapshot),
                },
                seconds_to_market_end,
                pricing_kurtosis: evidence_number(self.config.pricing_kurtosis),
                theta_decay_factor: evidence_number(self.config.theta_decay_factor),
                theta_scaled_min_edge_bps: evidence_number(theta_scaled_min_edge_bps),
                fair_probability_up: probability_evidence(fair_probability_up),
                uncertainty_band_probability: probability_evidence(uncertainty_band_probability),
                expected_edge_basis_points: evidence_number(expected_edge_basis_points),
                worst_case_edge_basis_points: evidence_number(worst_case_edge_basis_points),
                up_worst_case_edge_basis_points: option_evidence_number(
                    decision.evaluation.up_worst_case_ev_bps,
                ),
                down_worst_case_edge_basis_points: option_evidence_number(
                    decision.evaluation.down_worst_case_ev_bps,
                ),
                gate_blocked_by: decision
                    .evaluation
                    .gate
                    .blocked_by
                    .iter()
                    .map(entry_block_reason_to_evidence)
                    .collect(),
                pricing_blocked_by: decision
                    .evaluation
                    .pricing_blocked_by
                    .iter()
                    .map(entry_pricing_block_reason_to_evidence)
                    .collect(),
                fast_venue_name: self.evidence_spot_venue_name(),
                fast_venue_age_ms: self.pricing.last_fast_venue_age_ms,
                fast_venue_jitter_ms: self.pricing.last_fast_venue_jitter_ms,
                fast_venue_incoherent: self.pricing.fast_venue_incoherent,
                lead_agreement_corr: option_evidence_probability(
                    self.pricing.last_lead_agreement_corr,
                ),
                selected_side: Some(outcome_side_to_evidence(selected_side)),
            },
            submission: SubmissionLinkage {
                instrument_id: instrument_id.to_string(),
                order_side: evidence_order_side(order_side),
                price: price.to_string(),
                quantity: quantity.to_string(),
                client_order_id: client_order_id.to_string(),
            },
        })
    }

    /// Evaluate and (if admitted) submit an exit order, then record durable #885
    /// exit-evaluation evidence flood-gated by [`ExitOutcomeKey`].
    ///
    /// `trigger_context` supplies the receive timestamp used for realized-volatility
    /// consumption. Every production trigger has one; a structurally absent receive
    /// stamp remains fail-closed.
    fn try_submit_exit_order_for_trigger(
        &mut self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> Result<Option<ClientOrderId>> {
        let attempt = self.try_submit_exit_order_inner(now_ms, trigger_context)?;
        let (decision, outcome) = attempt.evidence();
        self.record_exit_evaluation_evidence(now_ms, decision, trigger_context, outcome);
        attempt.into_result()
    }

    fn allocate_exit_attempt_generation(&mut self) -> Result<u64> {
        let generation = self
            .next_exit_attempt_generation
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("exit attempt generation overflow"))?;
        self.next_exit_attempt_generation = generation;
        Ok(generation)
    }

    fn resolve_exit_attempt(&mut self, generation: u64, disposition: ExitAttemptDisposition) {
        let current = std::mem::replace(&mut self.exposure, ExposureState::Flat);
        self.exposure = match current {
            ExposureState::ExitAttempting(attempt) if attempt.generation == generation => {
                match disposition {
                    ExitAttemptDisposition::Submitted => {
                        ExposureState::ExitPending(attempt.into_pending())
                    }
                    ExitAttemptDisposition::NonSubmitted => ExposureState::Managed(attempt.managed),
                }
            }
            // A synchronous NT callback already advanced this generation. Its
            // authoritative state transition wins over the stale route return.
            ExposureState::ExitAttempting(attempt) => ExposureState::ExitAttempting(attempt),
            ExposureState::Flat => ExposureState::Flat,
            ExposureState::PendingEntry(pending) => ExposureState::PendingEntry(pending),
            ExposureState::EntryReconcilePending { pending, reason } => {
                ExposureState::EntryReconcilePending { pending, reason }
            }
            ExposureState::Managed(managed) => ExposureState::Managed(managed),
            ExposureState::ExitPending(exit) => ExposureState::ExitPending(exit),
            ExposureState::TerminalExitAwaitingPosition(exit) => {
                ExposureState::TerminalExitAwaitingPosition(exit)
            }
            ExposureState::ExitAuthorityRecoveryHold(hold) => {
                ExposureState::ExitAuthorityRecoveryHold(hold)
            }
            ExposureState::UnsupportedObserved(observed) => {
                ExposureState::UnsupportedObserved(observed)
            }
            ExposureState::BlindRecovery(recovery) => ExposureState::BlindRecovery(recovery),
        };
    }

    fn try_submit_exit_order_inner(
        &mut self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> Result<ExitAttemptExecution> {
        self.refresh_realized_volatility_snapshot_at(now_ms);
        self.apply_reference_price_selection_at(now_ms);
        if let Some(position) = self.settlement_position_candidate()
            && position.lifecycle.interval_end_ms().is_none()
        {
            self.record_missing_interval_end_settlement_booking_error(
                &position,
                now_ms.saturating_mul(NANOS_PER_MILLI_U64),
            )?;
        }
        let mut decision = self.exit_intent_decision_for_trigger_at(now_ms, trigger_context);

        let Some(instrument_id) = decision.instrument_id else {
            self.record_exit_intent_or_hold_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            let outcome = non_action_exit_attempt_outcome(&decision);
            return Ok(ExitAttemptExecution::completed(decision, outcome));
        };
        let Some(order_side) = decision.order_side else {
            self.record_exit_intent_or_hold_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            let outcome = non_action_exit_attempt_outcome(&decision);
            return Ok(ExitAttemptExecution::completed(decision, outcome));
        };
        let Some(raw_price) = decision.price else {
            self.record_exit_intent_or_hold_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            let outcome = non_action_exit_attempt_outcome(&decision);
            return Ok(ExitAttemptExecution::completed(decision, outcome));
        };
        let Some(quantity) = decision.quantity else {
            self.record_exit_intent_or_hold_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            let outcome = non_action_exit_attempt_outcome(&decision);
            return Ok(ExitAttemptExecution::completed(decision, outcome));
        };
        self.record_exit_intent_or_hold_once(now_ms, trigger_context, &decision)?;
        let Some(order_config) = decision.execution_config() else {
            let failure = anyhow::anyhow!("exit intent decision missing order config");
            return Ok(rejected_exit_preparation(
                decision,
                ExitPreparationStage::OrderTemplate,
                failure,
            ));
        };
        let Some(instrument) = self.current_instrument(instrument_id) else {
            let failure = anyhow::anyhow!("exit instrument missing from cache");
            return Ok(rejected_exit_preparation(
                decision,
                ExitPreparationStage::InstrumentAuthority,
                failure,
            ));
        };
        let Some(managed_position) = self.managed_position() else {
            let failure = anyhow::anyhow!("exit submit requires managed position state");
            return Ok(rejected_exit_preparation(
                decision,
                ExitPreparationStage::PositionAuthority,
                failure,
            ));
        };
        let requested_price = Price::new(raw_price, instrument.price_precision());
        let client_order_id = self.core.order_factory().generate_client_order_id();
        decision.client_order_id = Some(client_order_id);
        let order = match self.build_exit_order_with_execution_config(
            order_config,
            instrument_id,
            order_side,
            quantity,
            requested_price,
            client_order_id,
        ) {
            Ok(order) => order,
            Err(failure) => {
                return Ok(rejected_exit_preparation(
                    decision,
                    ExitPreparationStage::OrderTemplate,
                    failure,
                ));
            }
        };

        let client_id = ClientId::from(self.config.client_id.as_str());
        let Some(managed_context) = self.exposure.managed_position_context().cloned() else {
            let failure = anyhow::anyhow!("exit submit requires managed position context");
            return Ok(rejected_exit_preparation(
                decision,
                ExitPreparationStage::PositionAuthority,
                failure,
            ));
        };
        let original_signed_position_quantity = match managed_position.position.side {
            PositionSide::Long => managed_position.position.quantity.as_decimal(),
            PositionSide::Short => -managed_position.position.quantity.as_decimal(),
            PositionSide::Flat | PositionSide::NoPositionSide => {
                let failure =
                    anyhow::anyhow!("exit submission requires a non-flat canonical position");
                return Ok(rejected_exit_preparation(
                    decision,
                    ExitPreparationStage::PositionAuthority,
                    failure,
                ));
            }
        };
        let prediction_market_outcome = match managed_position.position.lifecycle.outcome_side() {
            Some(OutcomeSide::Up) => PredictionMarketOutcomeSide::Yes,
            Some(OutcomeSide::Down) => PredictionMarketOutcomeSide::No,
            None => {
                let failure = anyhow::anyhow!(
                    "exit submission requires a canonical prediction-market outcome"
                );
                return Ok(rejected_exit_preparation(
                    decision,
                    ExitPreparationStage::PositionAuthority,
                    failure,
                ));
            }
        };
        let intent = order_intent_details_from_compiled_order(
            self.config.strategy_id.clone(),
            requested_price.to_string(),
            &order,
        );
        let Some(entry_cost) = self
            .open_position_effective_entry_cost()
            .filter(|value| is_positive_finite(*value))
            .and_then(Decimal::from_f64)
        else {
            let failure = anyhow::anyhow!("exit economics requires a valid entry cost basis");
            return Ok(rejected_exit_preparation(
                decision,
                ExitPreparationStage::EconomicsSeal,
                failure,
            ));
        };
        let Some(position_authority) = self.context.position_authority() else {
            let failure = anyhow::anyhow!("exit submission requires position authority capability");
            return Ok(rejected_exit_preparation(
                decision,
                ExitPreparationStage::PositionAuthority,
                failure,
            ));
        };
        let venue_position_id =
            match self.exit_authority_venue_position_id(managed_position.position.position_id) {
                Ok(venue_position_id) => venue_position_id,
                Err(failure) => {
                    return Ok(rejected_exit_preparation(
                        decision,
                        ExitPreparationStage::OrderTemplate,
                        failure,
                    ));
                }
            };
        let compiled =
            match compile_and_seal_risk_reducing_ioc(BoltV3CompileAndSealRiskReducingIocInput {
                economics: self.context.order_economics(),
                execution_venue: self.context.execution_venue(),
                execution_client_id: &self.config.client_id,
                instrument: &instrument,
                book: executable_book_quote(&managed_position.position.book),
                vwap_depth_limit_bps: self.config.vwap_depth_limit_bps,
                intent,
                requested_order: order,
                position_id: managed_position.position.position_id,
                position_authority,
                venue_position_id,
                position_side: managed_position.position.side,
                prediction_market_outcome,
                stored_entry_cost_per_unit: entry_cost,
                requested_at_ns: now_ms.saturating_mul(NANOS_PER_MILLI_U64),
                decision_correlation_id: client_order_id.as_str(),
            }) {
                Ok(compiled) => compiled,
                Err(failure) => {
                    let stage = evidence_preparation_stage(failure.stage());
                    return Ok(rejected_exit_preparation(
                        decision,
                        stage,
                        anyhow::Error::new(failure),
                    ));
                }
            };
        let (intent, order, sealed, compiled, position_authority_lease) = compiled.into_parts();
        let quantity = compiled.quantity;
        let price = compiled.worst_executable_price;
        decision.quantity = Some(quantity);
        decision.price = Some(price.as_f64());
        let exit_authority = match BoltV3ExitOrderAuthorityHandle::locally_submitted(
            client_order_id,
            instrument_id,
            managed_position.position.position_id,
            original_signed_position_quantity,
            managed_position.position.side.as_specified(),
            quantity,
            position_authority_lease,
        ) {
            Ok(authority) => authority,
            Err(failure) => {
                return Ok(rejected_exit_preparation(
                    decision,
                    ExitPreparationStage::PositionAuthority,
                    failure,
                ));
            }
        };
        let prepared_order = prepared_order_linkage(&intent);
        self.record_exit_prepared_order(now_ms, trigger_context, &decision, prepared_order.clone());
        self.log_exit_evaluation(now_ms, trigger_context, &decision);
        let generation = match self.allocate_exit_attempt_generation() {
            Ok(generation) => generation,
            Err(failure) => {
                let reason = format!("{failure:#}");
                return Ok(ExitAttemptExecution::rejected(
                    decision,
                    ExitAttemptOutcome::RouteRejected {
                        prepared_order,
                        reason,
                    },
                    failure,
                ));
            }
        };
        if !decision.forced_flat_reasons.is_empty()
            && let Some(pending_entry) = managed_position.pending_entry.as_ref()
            && let Err(failure) = self
                .cancel_resting_order(pending_entry.client_order_id, client_id)
                .with_context(|| {
                    format!(
                        "forced-flat exit could not cancel pending entry client_order_id={}",
                        pending_entry.client_order_id
                    )
                })
        {
            let reason = format!("{failure:#}");
            return Ok(ExitAttemptExecution::rejected(
                decision,
                ExitAttemptOutcome::RouteRejected {
                    prepared_order,
                    reason,
                },
                failure,
            ));
        }
        self.exposure = ExposureState::ExitAttempting(ExitAttemptingState {
            generation,
            managed: managed_context,
            pending_exit: PendingExitState {
                client_order_id,
                submitted_at_ms: Some(now_ms),
                market_id: managed_position.position.lifecycle.market_id_owned(),
                position_id: Some(managed_position.position.position_id),
            },
            authority: exit_authority,
        });
        log::info!(
            "binary_oracle_edge_taker exit submit: strategy_id={} instrument_id={} order_side={:?} price={} quantity={} client_order_id={}",
            self.config.strategy_id,
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        );
        let outcome = self.route_prepared_order_submission(
            intent,
            order,
            sealed,
            BoltV3SubmitContext::with_client_id_and_position_id(
                client_id,
                managed_position.position.position_id,
            ),
        );
        let (disposition, attempt) =
            ExitAttemptExecution::from_submit_state(decision, prepared_order, outcome.into_state());
        self.resolve_exit_attempt(generation, disposition);
        Ok(attempt)
    }

    fn build_exit_evaluation_evidence(
        &self,
        now_ms: u64,
        decision: &ExitIntentDecision,
        trigger_context: ExitEvaluationTriggerContext,
        log_fields: &ExitEvaluationLogFields,
        outcome: &ExitAttemptOutcome,
    ) -> Result<ExitEvaluationFact> {
        let receipt = &decision.evaluation.realized_volatility_receipt;
        let checked_timestamp = |field: &'static str, value: u64| {
            i64::try_from(value).with_context(|| {
                format!("exit evaluation evidence {field} exceeds i64::MAX: {value}")
            })
        };

        // Validate all absolute timestamps before narrowing the signed diagnostic delta.
        let trigger_ts_event_ms = Some(checked_timestamp(
            "trigger_ts_event_ms",
            trigger_context.ts_event_ms,
        )?);
        let trigger_ts_init_ms = receipt
            .evaluation_receive_ms
            .map(|value| checked_timestamp("trigger_ts_init_ms", value.value()))
            .transpose()?;
        let exit_eval_now_ms = checked_timestamp("exit_eval_now_ms", now_ms)?;
        let rv_as_of_ms = receipt
            .snapshot_as_of_ms
            .map(|value| checked_timestamp("rv_as_of_ms", value))
            .transpose()?;
        let rv_snapshot_receive_watermark_ms = receipt
            .snapshot_receive_watermark_ms
            .map(|value| checked_timestamp("rv_snapshot_receive_watermark_ms", value.value()))
            .transpose()?;
        let rv_as_of_minus_now_ms = receipt
            .snapshot_as_of_minus_trigger_event_ms
            .map(|delta_ms| {
                i64::try_from(delta_ms).with_context(|| {
                    format!(
                        "exit evaluation evidence rv_as_of_minus_now_ms exceeds i64 range: {delta_ms}"
                    )
                })
            })
            .transpose()?;
        let reference_current_price = option_evidence_number(log_fields.reference_current_price);

        Ok(ExitEvaluationFact {
            position_id: log_fields.position_id.map(|id| id.to_string()),
            market_id: log_fields.market_id.clone(),
            instrument_id: log_fields.position_instrument_id.map(|id| id.to_string()),
            exit_eval_now_ms,
            exit_trigger_source: trigger_context.source,
            trigger_ts_event_ms,
            trigger_ts_init_ms,
            rv_surface_id: receipt.surface_id.clone(),
            rv_as_of_ms,
            rv_ready: receipt.snapshot_has_ready_realized_vol,
            rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: Some(receipt.max_source_age_ms),
            rv_blockers: receipt
                .raw_snapshot_blockers
                .iter()
                .map(|reason| (*reason).into())
                .collect(),
            rv_source_diagnostics: receipt.source_diagnostics.clone(),
            rv_gate_result: receipt.gate_result,
            rv_as_of_minus_now_ms,
            spot_price: option_evidence_number(log_fields.spot_price),
            spot_venue_name: log_fields.spot_venue_name.clone(),
            fast_venue_available: log_fields.fast_venue_available,
            reference_current_price_available: reference_current_price.is_some(),
            reference_current_price,
            interval_open: option_evidence_number(log_fields.interval_open),
            fair_probability_up: option_evidence_number(log_fields.fair_probability_up),
            fair_probability_down: option_evidence_number(log_fields.fair_probability_down),
            uncertainty_band_probability: option_evidence_number(
                log_fields.uncertainty_band_probability,
            ),
            hold_ev_bps: option_evidence_number(log_fields.hold_ev_bps),
            exit_ev_bps: option_evidence_number(log_fields.exit_ev_bps),
            outcome: outcome.clone(),
            forced_flat_reasons: log_fields
                .forced_flat_reasons
                .iter()
                .map(forced_flat_reason_to_evidence)
                .collect(),
        })
    }

    /// Emit immutable #885 exit-evaluation evidence flood-gated by
    /// [`ExitOutcomeKey`]. Evidence construction and writer failures are record-local:
    /// they are logged after the existing dedupe mark and never alter trading state.
    fn record_exit_evaluation_evidence(
        &mut self,
        now_ms: u64,
        decision: &ExitIntentDecision,
        trigger_context: ExitEvaluationTriggerContext,
        outcome: &ExitAttemptOutcome,
    ) {
        let log_fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let exit_decision = exit_decision_evidence_from_optional(decision.evaluation.exit_decision);
        let outcome_key = ExitOutcomeKey {
            exit_decision,
            submission_blocked_reason: decision.blocked_reason,
            rv_gate_result: decision.evaluation.realized_volatility_receipt.gate_result,
        };

        // Flood guard: collapse a per-tick exit flood (same outcome key) into a single
        // durable record per open position. An actual submit always records (it is a
        // distinct, rare event). The per-tick tracing log above is untouched.
        if let Some(position_id) = log_fields.position_id {
            let mut changed = true;
            if let Some(last_outcome) = self.last_exit_evidence_outcome.get_mut(&position_id) {
                if last_outcome == &outcome_key {
                    changed = false;
                } else {
                    *last_outcome = outcome_key;
                }
            } else {
                self.last_exit_evidence_outcome
                    .insert(position_id, outcome_key);
            }
            let is_attempt = !matches!(
                outcome,
                ExitAttemptOutcome::Held { .. } | ExitAttemptOutcome::Blocked { .. }
            );
            if !is_attempt && !changed {
                return;
            }
        }

        let evidence = match self.build_exit_evaluation_evidence(
            now_ms,
            decision,
            trigger_context,
            &log_fields,
            outcome,
        ) {
            Ok(evidence) => evidence,
            Err(error) => {
                log::error!(
                    "binary_oracle_edge_taker exit evidence build failed: strategy_id={} position_id={:?} error={:#}",
                    self.config.strategy_id,
                    log_fields.position_id,
                    error,
                );
                return;
            }
        };

        if let ObservationRecordOutcome::FailureReported(error) = self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_exit_evaluation(evidence.clone())
        {
            log::error!(
                "binary_oracle_edge_taker exit evidence write failed: strategy_id={} position_id={:?} error={:#}",
                self.config.strategy_id,
                evidence.position_id,
                error,
            );
        }
    }

    fn entry_submission_decision_for_receive_at(
        &self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
    ) -> EntrySubmissionDecision {
        let evaluation = self.entry_evaluation_for_receive_at(now_ms, receive_context);
        let mut decision = EntrySubmissionDecision {
            evaluation: evaluation.clone(),
            instrument_id: self.active.instrument_id,
            order_side: None,
            price: None,
            quantity_value: None,
            planned_fill_legs: Vec::new(),
            client_order_id: None,
            blocked_reason: None,
        };

        if DataActor::trader_id(self).is_none() {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::StrategyCoreNotRegistered);
            return decision;
        }

        if !evaluation.gate.blocked_by.is_empty() {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::EntryGateBlocked);
            return decision;
        }
        if !evaluation.pricing_blocked_by.is_empty() {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::EntryPricingBlocked);
            return decision;
        }

        let Some(selected_side) = evaluation.selected_side else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::NoSideSelected);
            return decision;
        };
        let Some(sized_notional) = evaluation
            .sized_notional
            .filter(|value| is_positive_finite(*value))
        else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::SizedNotionalNotPositive);
            return decision;
        };

        let Some(instrument_id) = self.instrument_id_for_side(selected_side) else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::InstrumentIdMissing);
            return decision;
        };
        let Some(instrument) = self.current_instrument(instrument_id) else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::InstrumentMissingFromCache);
            return decision;
        };
        if let Some(reason) = self.entry_reject_block_reason_for(instrument_id, selected_side) {
            decision.blocked_reason = Some(reason);
            return decision;
        }
        let Some(submission_vwap) =
            executable_submission_vwap_from_evaluation(&evaluation, selected_side)
        else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::EntryPriceMissing);
            return decision;
        };
        let price = submission_vwap.limit_price;
        let quantity_value = if self.config.entry_order.is_quote_quantity {
            let Some(sized_notional_decimal) = Decimal::from_f64(sized_notional) else {
                decision.blocked_reason = Some(EvidenceEntrySkipReason::QuantityRoundingFailed);
                return decision;
            };
            match make_market_quote_buy_quantity(
                self.context.execution_venue(),
                &instrument,
                sized_notional_decimal,
            ) {
                Ok(quantity) => quantity.as_f64(),
                Err(MarketQuoteBuyQuantityError::MinimumUnmodeled) => {
                    decision.blocked_reason =
                        Some(EvidenceEntrySkipReason::EntryQuoteNotionalMinimumUnmodeled);
                    return decision;
                }
                Err(MarketQuoteBuyQuantityError::BelowMinimum) => {
                    decision.blocked_reason =
                        Some(EvidenceEntrySkipReason::EntryQuoteNotionalBelowVenueMinimum);
                    return decision;
                }
                Err(MarketQuoteBuyQuantityError::QuantityInvalid) => {
                    decision.blocked_reason = Some(EvidenceEntrySkipReason::QuantityRoundingFailed);
                    return decision;
                }
            }
        } else {
            let max_quantity_at_limit = sized_notional / price;
            if !is_positive_finite(max_quantity_at_limit) {
                decision.blocked_reason = Some(EvidenceEntrySkipReason::EntryPriceMissing);
                return decision;
            }
            let shares_value = submission_vwap.vwap_quantity.min(max_quantity_at_limit);
            let Ok(quantity) = instrument.try_make_qty(shares_value, Some(true)) else {
                decision.blocked_reason = Some(EvidenceEntrySkipReason::QuantityRoundingFailed);
                return decision;
            };
            let Some(quantity) = normalize_base_order_quantity(
                self.context.execution_venue(),
                &instrument,
                quantity,
            ) else {
                decision.blocked_reason = Some(EvidenceEntrySkipReason::QuantityRoundingFailed);
                return decision;
            };
            let quantity_value = quantity.as_f64();
            let limit_notional = price * quantity_value;
            if limit_notional_exceeds_sized_notional(limit_notional, sized_notional) {
                decision.blocked_reason =
                    Some(EvidenceEntrySkipReason::LimitNotionalExceedsSizedNotional);
                return decision;
            }
            quantity_value
        };
        if !is_positive_finite(quantity_value) {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::QuantityNotPositive);
            return decision;
        }

        let Ok(contract) = self.configured_position_contract() else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::PositionContractInvalid);
            return decision;
        };
        let order_side = contract.entry_order_side;
        let position_side = contract.entry_position_side;
        if !supports_strategy_managed_position(order_side, position_side, contract) {
            decision.blocked_reason =
                Some(EvidenceEntrySkipReason::EntryPositionContractUnsupported);
            return decision;
        }
        let planned_fill_legs =
            match self.executable_entry_probe_for_side(selected_side, order_side, sized_notional) {
                Ok(probe) => probe
                    .vwap
                    .candidate_levels
                    .into_iter()
                    .map(|leg| {
                        Some(BoltV3PlannedFillLeg {
                            price: Decimal::from_f64(leg.price)?,
                            quantity: Decimal::from_f64(leg.quantity)?,
                        })
                    })
                    .collect::<Option<Vec<_>>>(),
                Err(_) => None,
            };
        let Some(planned_fill_legs) = planned_fill_legs else {
            decision.blocked_reason = Some(EvidenceEntrySkipReason::EntryPriceMissing);
            return decision;
        };

        decision.instrument_id = Some(instrument_id);
        decision.order_side = Some(order_side);
        decision.price = Some(price);
        decision.quantity_value = Some(quantity_value);
        decision.planned_fill_legs = planned_fill_legs;
        decision
    }

    fn try_submit_entry_order_for_receive(
        &mut self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
    ) -> Result<Option<ClientOrderId>> {
        self.refresh_realized_volatility_snapshot_at(now_ms);
        let decision = self.entry_submission_decision_for_receive_at(now_ms, receive_context);
        self.submit_admitted_entry_decision(now_ms, decision)
    }

    fn submit_admitted_entry_decision(
        &mut self,
        now_ms: u64,
        decision: EntrySubmissionDecision,
    ) -> Result<Option<ClientOrderId>> {
        self.log_entry_evaluation(now_ms, &decision);

        let realized_volatility_not_ready = decision.blocked_reason
            == Some(EvidenceEntrySkipReason::EntryPricingBlocked)
            && decision
                .evaluation
                .pricing_blocked_by
                .contains(&EntryPricingBlockReason::RealizedVolNotReady)
            && !decision
                .evaluation
                .realized_volatility_receipt
                .evidence
                .surface_id
                .is_empty();
        // Leaving the blocked condition no longer resets suppression. Under the
        // closed registry a claimed state stays claimed for the life of the
        // market episode, so a gate that flaps in and out of readiness records
        // each distinct state once rather than once per flap -- which is what
        // makes the per-episode record count finite. A genuinely new market is a
        // new episode and starts with nothing claimed.
        if realized_volatility_not_ready {
            self.record_blocked_entry_strategy_input_snapshot_once(now_ms, &decision)?;
        }

        if let Some(reason) = decision.blocked_reason {
            self.record_and_log_entry_skip(now_ms, &decision, reason)?;
        }

        let Some(instrument_id) = decision.instrument_id else {
            return Ok(None);
        };
        let Some(order_side) = decision.order_side else {
            return Ok(None);
        };
        let Some(price) = decision.price else {
            return Ok(None);
        };
        let Some(quantity_value) = decision.quantity_value else {
            return Ok(None);
        };
        let instrument = self
            .current_instrument(instrument_id)
            .ok_or_else(|| anyhow::anyhow!("entry instrument missing from cache"))?;
        let quantity = instrument.try_make_qty(quantity_value, Some(true))?;

        if self.exposure_occupancy().is_some() {
            let newly_recorded = self.record_entry_skip_once(
                now_ms,
                &decision,
                EvidenceEntrySkipReason::OnePositionInvariantViolation,
            )?;
            // Keep WARN on the same dedupe as evidence (not per-tick), then
            // propagate the invariant failure so admission fails closed.
            if newly_recorded {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    entry_skip_reason_label(EvidenceEntrySkipReason::OnePositionInvariantViolation)
                );
            }
            self.enforce_one_position_invariant()?;
            unreachable!("occupied exposure must fail the one-position invariant");
        }

        // A successful submit no longer resets skip suppression, for the same
        // reason as the blocked-snapshot producer above: within one market
        // episode each skip reason is worth recording once.
        self.entry_reject_state.remove(&instrument_id);
        let price = Price::new(price, instrument.price_precision());
        let client_order_id = self.core.order_factory().generate_client_order_id();
        let order = self.build_configured_entry_order(
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )?;
        let strategy_input_snapshot = self.entry_strategy_input_evidence_snapshot_at(
            now_ms,
            &decision,
            client_order_id,
            &price,
            &quantity,
        )?;
        let terminal_value_per_unit = decision
            .evaluation
            .sized_executable_edge
            .and_then(|edge| Decimal::from_f64(edge.adjusted_probability))
            .ok_or_else(|| anyhow::anyhow!("entry economics requires a terminal value"))?;
        let minimum_core_edge_ratio = decision
            .evaluation
            .min_worst_case_ev_bps
            .filter(|value| value.is_finite())
            .and_then(|value| Decimal::from_f64(value / BPS_DENOMINATOR))
            .ok_or_else(|| {
                anyhow::anyhow!("entry economics requires a finite theta-scaled minimum edge ratio")
            })?;
        let intent = order_intent_details_from_compiled_order(
            self.config.strategy_id.clone(),
            price.to_string(),
            &order,
        );
        let (intent, order, sealed) = self.prepare_order_economics_submission(
            intent,
            BoltV3SubmitIntentKind::Entry,
            order,
            StrategyEconomicsInput::TerminalEntry {
                terminal_value_entry: BoltV3TerminalValueEntry::try_new(
                    terminal_value_per_unit,
                    BoltV3TerminalValueEntryPolicy::MinimumCoreEdgeRatio(minimum_core_edge_ratio),
                )?,
                candidate_fill_levels: decision.planned_fill_legs.clone(),
            },
        )?;

        let client_id = ClientId::from(self.config.client_id.as_str());
        self.last_flat_terminal_entry_override = None;
        self.exposure = ExposureState::PendingEntry(PendingEntryState {
            client_order_id,
            submitted_at_ms: Some(now_ms),
            lifecycle: BoltV3PositionMarketLifecycle::from_entry_context(
                self.current_market_id().map(str::to_string),
                decision.evaluation.selected_side,
                self.active.price_to_beat,
                self.active.interval_open,
                self.active.interval_end_ms,
                self.active.selection_published_at_ms,
                self.active.seconds_to_expiry_at_selection,
            ),
            instrument_id,
            book: match decision.evaluation.selected_side {
                Some(OutcomeSide::Up)
                    if self.active.books.up.instrument_id == Some(instrument_id) =>
                {
                    self.active.books.up.clone()
                }
                Some(OutcomeSide::Down)
                    if self.active.books.down.instrument_id == Some(instrument_id) =>
                {
                    self.active.books.down.clone()
                }
                _ => OutcomeBookState::from_instrument_id(instrument_id),
            },
        });
        log::info!(
            "binary_oracle_edge_taker entry submit: strategy_id={} instrument_id={} order_side={:?} price={} quantity={} client_order_id={}",
            self.config.strategy_id,
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        );

        if let Err(error) = self
            .context
            .edge_taker_evidence()
            .expect("edge-taker strategy must own edge-taker evidence")
            .record_submit_linked_strategy_input_snapshot(strategy_input_snapshot)
        {
            self.clear_pending_entry_state();
            return Err(anyhow::Error::from(error));
        }
        let outcome = self.route_prepared_order_submission(
            intent,
            order,
            sealed,
            BoltV3SubmitContext::with_client_id(client_id),
        );
        match outcome.kind() {
            BoltV3SubmitAttemptKind::Submitted => {}
            BoltV3SubmitAttemptKind::PolicySkipped => {
                self.clear_pending_entry_state();
            }
            BoltV3SubmitAttemptKind::RouteValidationRejected
            | BoltV3SubmitAttemptKind::IntentEvidenceRejected
            | BoltV3SubmitAttemptKind::AdmissionRejected
            | BoltV3SubmitAttemptKind::PreSinkRejected
            | BoltV3SubmitAttemptKind::SinkRejected => {
                self.clear_pending_entry_state();
                anyhow::bail!(
                    "entry submit did not reach the venue: outcome={:?} diagnostic={}",
                    outcome.kind(),
                    outcome.diagnostic().unwrap_or("none")
                );
            }
        }

        Ok(Some(client_order_id))
    }

    fn entry_evaluation_for_receive_at(
        &self,
        now_ms: u64,
        receive_context: EntryEvaluationReceiveContext,
    ) -> EntryEvaluation {
        let gate = self.entry_gate_decision_at(now_ms);
        let realized_volatility_receipt =
            self.entry_realized_volatility_receipt_at(receive_context.receive_ms());
        let mut evaluation = EntryEvaluation {
            gate,
            realized_volatility_receipt,
            pricing_blocked_by: Vec::new(),
            fair_probability_up: None,
            uncertainty_band_probability: None,
            up_executable_edge: None,
            down_executable_edge: None,
            up_worst_case_ev_bps: None,
            down_worst_case_ev_bps: None,
            sized_executable_edge: None,
            sized_worst_case_ev_bps: None,
            min_worst_case_ev_bps: None,
            expected_ev_per_notional: None,
            book_impact_cap_notional: None,
            sized_notional: None,
            selected_side: None,
        };

        if !evaluation.gate.blocked_by.is_empty() {
            return evaluation;
        }

        let pricing_inputs =
            match self.current_entry_pricing_inputs_for_receive_at(now_ms, receive_context) {
                Ok(inputs) => inputs,
                Err(blocked_by) => {
                    evaluation.pricing_blocked_by = blocked_by;
                    return evaluation;
                }
            };
        evaluation.min_worst_case_ev_bps = Some(pricing_inputs.theta_scaled_min_edge_bps);

        let fair_probability_up =
            match self.current_fair_probability_up_for_receive_at(now_ms, receive_context) {
                Some(value) => value,
                None => {
                    evaluation
                        .pricing_blocked_by
                        .push(EntryPricingBlockReason::FairProbabilityUnavailable);
                    return evaluation;
                }
            };
        evaluation.fair_probability_up = Some(fair_probability_up);

        if let Some(reason) = self.executable_edge_order_shape_block_reason() {
            evaluation.up_executable_edge =
                Some(BinaryOutcomeEdgeResult::blocked(OutcomeSide::Up, reason));
            evaluation.down_executable_edge =
                Some(BinaryOutcomeEdgeResult::blocked(OutcomeSide::Down, reason));
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Up,
                Some(reason),
            );
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Down,
                Some(reason),
            );
            return evaluation;
        }

        let Ok(order_side) = self.configured_entry_order_side() else {
            evaluation.pricing_blocked_by.push(
                EntryPricingBlockReason::ExecutableEntryCostUnavailable(OutcomeSide::Up),
            );
            evaluation.pricing_blocked_by.push(
                EntryPricingBlockReason::ExecutableEntryCostUnavailable(OutcomeSide::Down),
            );
            return evaluation;
        };
        let up_probe = self.executable_entry_probe_for_side(
            OutcomeSide::Up,
            order_side,
            self.preliminary_edge_pricing_notional_for_side(OutcomeSide::Up),
        );
        let down_probe = self.executable_entry_probe_for_side(
            OutcomeSide::Down,
            order_side,
            self.preliminary_edge_pricing_notional_for_side(OutcomeSide::Down),
        );
        if let Err(reason) = up_probe {
            evaluation.up_executable_edge =
                Some(BinaryOutcomeEdgeResult::blocked(OutcomeSide::Up, reason));
        }
        if let Err(reason) = down_probe {
            evaluation.down_executable_edge =
                Some(BinaryOutcomeEdgeResult::blocked(OutcomeSide::Down, reason));
        }

        if up_probe.is_err() && down_probe.is_err() {
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Up,
                up_probe.as_ref().err().copied(),
            );
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Down,
                down_probe.as_ref().err().copied(),
            );
            return evaluation;
        }
        let uncertainty_band_probability =
            match self.current_uncertainty_band_probability_for_gate_at(now_ms, receive_context) {
                Some(value) => value,
                None => {
                    evaluation
                        .pricing_blocked_by
                        .push(EntryPricingBlockReason::UncertaintyBandUnavailable);
                    return evaluation;
                }
            };
        evaluation.uncertainty_band_probability = Some(uncertainty_band_probability);

        let up_adjusted_probability_up = fair_probability_up.narrowed(uncertainty_band_probability);
        let down_adjusted_probability_up =
            fair_probability_up.widened(uncertainty_band_probability);
        let up_executable_edge = match up_probe {
            Ok(probe) => self.executable_edge_for_side(
                OutcomeSide::Up,
                fair_probability_up,
                up_adjusted_probability_up,
                pricing_inputs.theta_scaled_min_edge_bps,
                probe,
            ),
            Err(reason) => BinaryOutcomeEdgeResult::blocked(OutcomeSide::Up, reason),
        };
        let down_executable_edge = match down_probe {
            Ok(probe) => self.executable_edge_for_side(
                OutcomeSide::Down,
                fair_probability_up,
                down_adjusted_probability_up,
                pricing_inputs.theta_scaled_min_edge_bps,
                probe,
            ),
            Err(reason) => BinaryOutcomeEdgeResult::blocked(OutcomeSide::Down, reason),
        };
        evaluation.up_worst_case_ev_bps =
            executable_edge_worst_case_ev_bps(Some(up_executable_edge));
        evaluation.down_worst_case_ev_bps =
            executable_edge_worst_case_ev_bps(Some(down_executable_edge));
        evaluation.up_executable_edge = Some(up_executable_edge);
        evaluation.down_executable_edge = Some(down_executable_edge);

        evaluation.selected_side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: executable_edge_selectable_bps(evaluation.up_executable_edge),
            down_worst_ev_bps: executable_edge_selectable_bps(evaluation.down_executable_edge),
            min_worst_case_ev_bps: pricing_inputs.theta_scaled_min_edge_bps,
        });
        if evaluation.selected_side.is_none() {
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Up,
                up_executable_edge.block_reason,
            );
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Down,
                down_executable_edge.block_reason,
            );
            return evaluation;
        }
        if let Some(selected_side) = evaluation.selected_side {
            let selected_edge = match selected_side {
                OutcomeSide::Up => evaluation.up_executable_edge,
                OutcomeSide::Down => evaluation.down_executable_edge,
            };
            let expected_ev_per_notional = selected_edge.and_then(|edge| {
                let probe = self
                    .executable_entry_probe_for_side(
                        selected_side,
                        order_side,
                        self.preliminary_edge_pricing_notional_for_side(selected_side),
                    )
                    .ok()?;
                self.economics_adjusted_entry_edge_ratio(
                    now_ms,
                    selected_side,
                    edge,
                    &probe,
                    pricing_inputs.theta_scaled_min_edge_bps,
                )
            });
            let book_impact_cap_notional = self.visible_book_notional_cap(selected_side);
            evaluation.expected_ev_per_notional = expected_ev_per_notional;
            evaluation.book_impact_cap_notional = book_impact_cap_notional;
            if expected_ev_per_notional.is_none() {
                evaluation.pricing_blocked_by.push(
                    EntryPricingBlockReason::ExecutableEntryCostUnavailable(selected_side),
                );
                evaluation.selected_side = None;
                return evaluation;
            }
            if let (Some(expected_ev_per_notional), Some(book_impact_cap_notional)) =
                (expected_ev_per_notional, book_impact_cap_notional)
            {
                evaluation.sized_notional = Some(choose_robust_size(
                    &self.robust_sizing_inputs(expected_ev_per_notional, book_impact_cap_notional),
                ));
            }
            if let Some(sized_notional) = evaluation
                .sized_notional
                .filter(|value| is_positive_finite(*value))
            {
                let selected_sized_probe = match self.executable_entry_probe_for_side(
                    selected_side,
                    order_side,
                    sized_notional,
                ) {
                    Ok(probe) => {
                        let Some((selected_uncertainty_band, adjusted_probability_up)) = self
                            .adjusted_probability_up_for_uncertainty(
                                now_ms,
                                receive_context,
                                selected_side,
                                fair_probability_up,
                            )
                        else {
                            evaluation
                                .pricing_blocked_by
                                .push(EntryPricingBlockReason::UncertaintyBandUnavailable);
                            evaluation.selected_side = None;
                            evaluation.sized_notional = None;
                            evaluation.expected_ev_per_notional = None;
                            return evaluation;
                        };
                        evaluation.uncertainty_band_probability = Some(selected_uncertainty_band);
                        (probe, adjusted_probability_up)
                    }
                    Err(reason) => {
                        let sized_executable_edge =
                            BinaryOutcomeEdgeResult::blocked(selected_side, reason);
                        evaluation.sized_worst_case_ev_bps =
                            executable_edge_worst_case_ev_bps(Some(sized_executable_edge));
                        evaluation.sized_executable_edge = Some(sized_executable_edge);
                        push_executable_edge_pricing_block(
                            &mut evaluation.pricing_blocked_by,
                            selected_side,
                            Some(reason),
                        );
                        evaluation.selected_side = None;
                        evaluation.sized_notional = None;
                        evaluation.expected_ev_per_notional = None;
                        return evaluation;
                    }
                };
                let (selected_sized_probe, selected_adjusted_probability_up) = selected_sized_probe;
                let sized_executable_edge = self.executable_edge_for_side(
                    selected_side,
                    fair_probability_up,
                    selected_adjusted_probability_up,
                    pricing_inputs.theta_scaled_min_edge_bps,
                    selected_sized_probe.clone(),
                );
                evaluation.sized_worst_case_ev_bps =
                    executable_edge_worst_case_ev_bps(Some(sized_executable_edge));
                evaluation.sized_executable_edge = Some(sized_executable_edge);
                if sized_executable_edge.trade_allowed {
                    let Some(book_impact_cap_notional) = evaluation.book_impact_cap_notional else {
                        evaluation.selected_side = None;
                        evaluation.sized_notional = None;
                        evaluation.expected_ev_per_notional = None;
                        return evaluation;
                    };
                    let Some(sized_expected_ev_per_notional) = self
                        .economics_adjusted_entry_edge_ratio(
                            now_ms,
                            selected_side,
                            sized_executable_edge,
                            &selected_sized_probe,
                            pricing_inputs.theta_scaled_min_edge_bps,
                        )
                    else {
                        evaluation.pricing_blocked_by.push(
                            EntryPricingBlockReason::ExecutableEntryCostUnavailable(selected_side),
                        );
                        evaluation.selected_side = None;
                        evaluation.sized_notional = None;
                        evaluation.expected_ev_per_notional = None;
                        return evaluation;
                    };
                    evaluation.expected_ev_per_notional = Some(sized_expected_ev_per_notional);
                    let resized_notional = choose_robust_size(&self.robust_sizing_inputs(
                        sized_expected_ev_per_notional,
                        book_impact_cap_notional,
                    ));
                    if is_positive_finite(resized_notional)
                        && (resized_notional - sized_notional).abs()
                            > notional_float_tolerance(sized_notional)
                    {
                        let resized_probe = match self.executable_entry_probe_for_side(
                            selected_side,
                            order_side,
                            resized_notional,
                        ) {
                            Ok(probe) => probe,
                            Err(reason) => {
                                let resized_executable_edge =
                                    BinaryOutcomeEdgeResult::blocked(selected_side, reason);
                                evaluation.sized_worst_case_ev_bps =
                                    executable_edge_worst_case_ev_bps(Some(
                                        resized_executable_edge,
                                    ));
                                evaluation.sized_executable_edge = Some(resized_executable_edge);
                                push_executable_edge_pricing_block(
                                    &mut evaluation.pricing_blocked_by,
                                    selected_side,
                                    Some(reason),
                                );
                                evaluation.selected_side = None;
                                evaluation.sized_notional = None;
                                evaluation.expected_ev_per_notional = None;
                                return evaluation;
                            }
                        };
                        let Some((resized_uncertainty_band, resized_adjusted_probability_up)) =
                            self.adjusted_probability_up_for_uncertainty(
                                now_ms,
                                receive_context,
                                selected_side,
                                fair_probability_up,
                            )
                        else {
                            evaluation
                                .pricing_blocked_by
                                .push(EntryPricingBlockReason::UncertaintyBandUnavailable);
                            evaluation.selected_side = None;
                            evaluation.sized_notional = None;
                            evaluation.expected_ev_per_notional = None;
                            return evaluation;
                        };
                        evaluation.uncertainty_band_probability = Some(resized_uncertainty_band);
                        let resized_executable_edge = self.executable_edge_for_side(
                            selected_side,
                            fair_probability_up,
                            resized_adjusted_probability_up,
                            pricing_inputs.theta_scaled_min_edge_bps,
                            resized_probe.clone(),
                        );
                        evaluation.sized_worst_case_ev_bps =
                            executable_edge_worst_case_ev_bps(Some(resized_executable_edge));
                        evaluation.sized_executable_edge = Some(resized_executable_edge);
                        // The accepted (size, edge) pair must be self-consistent:
                        // the final re-priced edge must itself support the resized
                        // notional. A cliff-shaped book otherwise oscillates — a
                        // small first pass fills cheap, the EV jump saturates the
                        // resize to the full target, and the thin full-target edge
                        // would be traded at a size it cannot support.
                        let final_expected_ev_per_notional = self
                            .economics_adjusted_entry_edge_ratio(
                                now_ms,
                                selected_side,
                                resized_executable_edge,
                                &resized_probe,
                                pricing_inputs.theta_scaled_min_edge_bps,
                            );
                        if resized_executable_edge.trade_allowed
                            && final_expected_ev_per_notional.is_none()
                        {
                            evaluation.pricing_blocked_by.push(
                                EntryPricingBlockReason::ExecutableEntryCostUnavailable(
                                    selected_side,
                                ),
                            );
                            evaluation.selected_side = None;
                            evaluation.sized_notional = None;
                            evaluation.expected_ev_per_notional = None;
                            return evaluation;
                        }
                        let final_expected_ev_per_notional =
                            final_expected_ev_per_notional.unwrap_or_default();
                        let final_supported_notional =
                            choose_robust_size(&self.robust_sizing_inputs(
                                final_expected_ev_per_notional,
                                book_impact_cap_notional,
                            ));
                        let resized_notional_supported = resized_notional
                            <= final_supported_notional
                                + notional_float_tolerance(final_supported_notional);
                        if resized_executable_edge.trade_allowed && resized_notional_supported {
                            evaluation.sized_notional = Some(resized_notional);
                            evaluation.expected_ev_per_notional =
                                Some(final_expected_ev_per_notional);
                        } else if resized_executable_edge.trade_allowed {
                            evaluation.pricing_blocked_by.push(
                                EntryPricingBlockReason::SizedNotionalUnsupported(selected_side),
                            );
                            // Keep the re-priced edge evidence, but clear the
                            // executable intent fields so submission stays
                            // blocked for this unsupported notional.
                            evaluation.selected_side = None;
                            evaluation.sized_notional = None;
                            evaluation.expected_ev_per_notional = None;
                        } else {
                            push_executable_edge_pricing_block(
                                &mut evaluation.pricing_blocked_by,
                                selected_side,
                                resized_executable_edge.block_reason,
                            );
                            evaluation.selected_side = None;
                            evaluation.sized_notional = None;
                            evaluation.expected_ev_per_notional = None;
                        }
                    }
                } else {
                    push_executable_edge_pricing_block(
                        &mut evaluation.pricing_blocked_by,
                        selected_side,
                        sized_executable_edge.block_reason,
                    );
                    evaluation.selected_side = None;
                    evaluation.sized_notional = None;
                    evaluation.expected_ev_per_notional = None;
                }
            }
        }
        evaluation
    }
}

impl std::fmt::Debug for BinaryOracleEdgeTaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryOracleEdgeTaker")
            .field("config", &self.config)
            .finish()
    }
}

impl DataActor for BinaryOracleEdgeTaker {
    fn on_start(&mut self) -> Result<()> {
        self.bootstrap_recovery_from_cache();
        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        self.refresh_selection_from_cache(now_ms);
        self.ensure_startup_subscription_derivations()?;
        self.register_selection_retry_timer();
        self.subscribe_reference_prices()?;
        self.subscribe_signal_quotes();
        self.subscribe_realized_volatility_sources();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.unsubscribe_realized_volatility_sources();
        self.unsubscribe_signal_quotes();
        self.unsubscribe_reference_prices()?;
        self.unsubscribe_resolution_strike();
        self.deregister_selection_retry_timer();
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if event.name.as_str() == self.selection_retry_timer_name() {
            self.refresh_selection_from_cache(now_ms);
            self.retry_missing_live_input_subscriptions_at(now_ms);
        }
        self.try_recover_exit_authority_hold(event.ts_event.as_u64());
        self.refresh_exit_authority_baseline();
        self.reconcile_cached_exit_order_on_timer();
        if let ExposureState::TerminalExitAwaitingPosition(exit_pending) = &self.exposure {
            let exit_pending = exit_pending.clone();
            self.try_release_terminal_exit(
                &exit_pending,
                ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
                None,
                event.ts_event.as_u64(),
            );
        }
        self.check_resolution_feed_outage_at_market_end(now_ms)?;
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        let lifecycle_now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        for snapshot in self.context.observe_realized_volatility_quote(quote) {
            self.pricing.observe_realized_vol_snapshot(snapshot);
        }
        if self
            .signal_instrument_id()
            .is_some_and(|instrument_id| quote.instrument_id == instrument_id)
        {
            let receive_ms = LocalReceiveMs::new(quote.ts_init.as_u64() / NANOS_PER_MILLI_U64);
            if let Some(signal_quote) = self.signal_quote_from_tick(quote, receive_ms) {
                self.observe_signal_quote(&signal_quote, lifecycle_now_ms, receive_ms);
            } else if let Some(signal_venue) = self.config.signal_venue.clone() {
                self.observe_invalid_signal_quote(
                    &signal_venue,
                    quote.ts_event.as_u64() / NANOS_PER_MILLI_U64,
                    lifecycle_now_ms,
                    receive_ms,
                );
            }
        }
        Ok(())
    }

    fn on_index_price(&mut self, update: &IndexPriceUpdate) -> anyhow::Result<()> {
        if self
            .resolution_instrument_id()
            .is_some_and(|instrument_id| update.instrument_id == instrument_id)
        {
            self.try_book_resolution_settlement(update)?;
            let window_open_ms = update.ts_event.as_u64() / NANOS_PER_MILLI_U64;
            let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
            self.active
                .observe_resolution_strike(update.value.as_f64(), window_open_ms, now_ms);
            self.sync_exposure_context_from_active();
        }
        for snapshot in self.context.observe_realized_volatility_index_price(update) {
            self.pricing.observe_realized_vol_snapshot(snapshot);
        }
        Ok(())
    }

    fn on_data(&mut self, data: &CustomData) -> anyhow::Result<()> {
        if let Some(update) = ReferencePriceUpdate::from_custom_data(data) {
            self.apply_reference_price_update(update);
            self.sync_exposure_context_from_active();
        }
        Ok(())
    }

    fn on_book_deltas(
        &mut self,
        deltas: &nautilus_model::data::OrderBookDeltas,
    ) -> anyhow::Result<()> {
        let mut matched = self.active.books.update_from_deltas(deltas);
        self.sync_exposure_context_from_active();
        if self
            .tracked_observed_position()
            .is_some_and(|position| position.instrument_id == deltas.instrument_id)
            && !(self.active.books.up.instrument_id == Some(deltas.instrument_id)
                || self.active.books.down.instrument_id == Some(deltas.instrument_id))
        {
            if let Some((_, _, book)) = self.tracked_position_context_mut() {
                book.update_from_deltas(deltas);
            }
            matched = true;
        }
        if self
            .pending_entry()
            .is_some_and(|pending| pending.instrument_id == deltas.instrument_id)
            && !(self.active.books.up.instrument_id == Some(deltas.instrument_id)
                || self.active.books.down.instrument_id == Some(deltas.instrument_id))
        {
            if let Some(pending) = self.pending_entry_mut() {
                pending.book.update_from_deltas(deltas);
            }
            matched = true;
        }

        if !matched {
            return Ok(());
        }

        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        if matches!(self.exposure, ExposureState::Managed(_))
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                now_ms,
                ExitEvaluationTriggerContext::from_market_data(
                    EvidenceExitTriggerSource::BookDelta,
                    deltas.ts_event.as_u64() / NANOS_PER_MILLI_U64,
                    LocalReceiveMs::new(deltas.ts_init.as_u64() / NANOS_PER_MILLI_U64),
                ),
            )
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on book delta: strategy_id={} instrument_id={} error={:#}",
                self.config.strategy_id,
                deltas.instrument_id,
                error
            );
        }
        if self.exposure_occupancy().is_none()
            && let Err(error) = self.try_submit_entry_order_for_receive(
                now_ms,
                EntryEvaluationReceiveContext::new(LocalReceiveMs::new(
                    deltas.ts_init.as_u64() / NANOS_PER_MILLI_U64,
                )),
            )
        {
            log::error!(
                "binary_oracle_edge_taker entry submit failed on book delta: strategy_id={} instrument_id={} error={:#}",
                self.config.strategy_id,
                deltas.instrument_id,
                error
            );
        }
        Ok(())
    }

    fn on_trade(&mut self, trade: &TradeTick) -> anyhow::Result<()> {
        for snapshot in self.context.observe_realized_volatility_trade(trade) {
            self.pricing.observe_realized_vol_snapshot(snapshot);
        }
        if let Some(trade_flow) = self.active.trade_flow.get_mut(&trade.instrument_id) {
            trade_flow.observe(trade);
        }
        Ok(())
    }
}

impl BinaryOracleEdgeTaker {
    fn enter_fill_void_exit_recovery(&mut self, event: &nautilus_model::events::OrderFillVoided) {
        let Ok(contract) = self.configured_position_contract() else {
            return;
        };
        if event.order_side != contract.exit_order_side
            || event.instrument_id.venue != self.context.execution_venue()
        {
            return;
        }
        let cached_order = self.cache().order(&event.client_order_id);
        let position_id = event.position_id.or_else(|| {
            cached_order
                .as_ref()
                .and_then(|order| order.position_id())
                .or_else(|| self.cache().position_id(&event.client_order_id))
        });
        let position = self
            .exposure
            .managed_position_context()
            .filter(|position| position.instrument_id == event.instrument_id)
            .cloned()
            .or_else(|| {
                position_id
                    .and_then(|position_id| self.nt_open_position_projection(position_id).ok())
                    .flatten()
                    .map(|spec| {
                        managed_position_context(
                            self.build_open_position_state(None, None, spec, false),
                            ManagedPositionOrigin::RecoveryBootstrap,
                            None,
                        )
                    })
            });
        self.enter_exit_authority_recovery_hold(
            position,
            PendingExitState {
                client_order_id: event.client_order_id,
                submitted_at_ms: None,
                market_id: self.active.market_id.clone(),
                position_id,
            },
            event.instrument_id,
            ExitAuthorityRecoveryPlan::Reconstruct(BoltV3RecoveredExitCause::FillVoidReopen),
            event.ts_event.as_u64(),
        );
        if cached_order.is_none() {
            log::error!(
                "binary_oracle_edge_taker fill-void recovery lacks cached order and remains held: strategy_id={} client_order_id={}",
                self.config.strategy_id,
                event.client_order_id,
            );
        }
        self.try_recover_exit_authority_hold(event.ts_event.as_u64());
    }

    fn handle_order_filled(&mut self, event: &nautilus_model::events::OrderFilled) {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        let entry_fill = self
            .pending_entry()
            .is_some_and(|pending| pending.client_order_id == event.client_order_id);
        let managed_entry_fill = self.managed_position().is_some_and(|managed| {
            managed
                .pending_entry
                .as_ref()
                .is_some_and(|pending| pending.client_order_id == event.client_order_id)
        });
        let exit_fill = self
            .exposure
            .exit_pending_snapshot()
            .is_some_and(|exit| exit.pending_exit.client_order_id == event.client_order_id);

        if entry_fill {
            let entry_reconcile_materialization =
                matches!(self.exposure, ExposureState::EntryReconcilePending { .. });
            let pending_context = self.pending_entry_context_for(event.instrument_id);
            let keep_pending_entry = self.entry_order_may_remain_working(&event.client_order_id);
            let position_side = self
                .configured_position_contract()
                .ok()
                .and_then(|contract| {
                    infer_strategy_position_side_from_entry_fill(
                        event.order_side,
                        contract.entry_order_side,
                        contract.entry_position_side,
                    )
                });
            if managed_entry_fill {
                if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                    return;
                }
                if !keep_pending_entry {
                    self.clear_managed_pending_entry_for_client_order(
                        event.client_order_id,
                        event.instrument_id,
                    );
                }
            } else if let Some(position_id) = event.position_id
                && let Some(position_side) = position_side
                && self.materialize_position_from_event(
                    PositionMaterializationSpec {
                        instrument_id: event.instrument_id,
                        position_id,
                        entry_order_side: event.order_side,
                        side: position_side,
                        quantity: event.last_qty,
                        avg_px_open: event.last_px.as_f64(),
                    },
                    event.ts_event.as_u64(),
                )
            {
                if entry_reconcile_materialization {
                    self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                        transition: OrderLifecycleTransition::EntryFillMaterialized,
                        outcome: OrderLifecycleOutcome::Managed,
                        source: ORDER_LIFECYCLE_SOURCE_ENTRY_FILL,
                        market_id: pending_context
                            .as_ref()
                            .and_then(|pending| pending.lifecycle.market_id_owned()),
                        instrument_id: Some(event.instrument_id),
                        position_id: Some(position_id),
                        client_order_id: Some(event.client_order_id),
                        prior_client_order_id: None,
                        raw_reason_text: None,
                        order_side: Some(event.order_side),
                        filled_quantity: Some(event.last_qty),
                        residual_quantity: None,
                        ts_event_ns: Some(event.ts_event.as_u64()),
                    });
                }
            } else {
                if matches!(self.exposure, ExposureState::BlindRecovery(_)) {
                    // The shared materialization guard already selected a typed fail-closed
                    // state (for example, a foreign-venue position). Do not overwrite it
                    // with the less precise generic inference failure below.
                } else if let Some(pending) = pending_context.clone() {
                    let reason = if event.position_id.is_none() {
                        EntryReconcileReason::AwaitingPositionMaterialization
                    } else {
                        EntryReconcileReason::UnsupportedEntryFillSide {
                            order_side: event.order_side,
                        }
                    };
                    self.exposure = ExposureState::EntryReconcilePending {
                        pending: pending.clone(),
                        reason,
                    };
                    self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                        transition: OrderLifecycleTransition::EntryReconcilePending,
                        outcome: OrderLifecycleOutcome::EntryReconcilePending,
                        source: ORDER_LIFECYCLE_SOURCE_ENTRY_FILL,
                        market_id: pending.lifecycle.market_id_owned(),
                        instrument_id: Some(event.instrument_id),
                        position_id: event.position_id,
                        client_order_id: Some(event.client_order_id),
                        prior_client_order_id: None,
                        raw_reason_text: None,
                        order_side: Some(event.order_side),
                        filled_quantity: Some(event.last_qty),
                        residual_quantity: None,
                        ts_event_ns: Some(event.ts_event.as_u64()),
                    });
                } else {
                    self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                        reason: BlindRecoveryReason::InvalidLivePosition {
                            entry_order_side: event.order_side,
                            side: position_side,
                        },
                    });
                }
                log::error!(
                    "binary_oracle_edge_taker entry fill could not materialize configured position contract: strategy_id={} client_order_id={} instrument_id={} order_side={:?} position_id_present={} position_side_inferable={}",
                    self.config.strategy_id,
                    event.client_order_id,
                    event.instrument_id,
                    event.order_side,
                    event.position_id.is_some(),
                    position_side.is_some(),
                );
            }
            if let Some(market_id) =
                pending_context.and_then(|pending| pending.lifecycle.market_id_owned())
            {
                self.record_market_fill(&market_id, now_ms);
            }
        } else if exit_fill {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if let Some(market_id) = self
                .exposure
                .exit_pending_snapshot()
                .and_then(|exit| exit.pending_exit.market_id.clone())
                .or_else(|| self.current_position_market_id())
            {
                self.record_market_fill(&market_id, now_ms);
            }
            self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
                client_order_id: event.client_order_id,
                instrument_id: event.instrument_id,
                transition: OrderLifecycleTransition::OrderFilled,
                source: ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
                raw_reason_text: None,
                ts_event_ns: event.ts_event.as_u64(),
                authority: ExitOrderAuthorityObservation::Lifecycle,
            });
        }
        self.prune_market_lifecycle(now_ms);
    }

    fn handle_order_canceled(&mut self, event: &nautilus_model::events::OrderCanceled) {
        self.resolve_pending_entry_terminal_event(PendingEntryTerminalEventInput {
            client_order_id: event.client_order_id,
            event_instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderCanceled,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_CANCELED,
            raw_reason_text: None,
            ts_event_ns: event.ts_event.as_u64(),
            terminal_proves_zero_fill: false,
        });
        self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id: event.client_order_id,
            instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderCanceled,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_CANCELED,
            raw_reason_text: None,
            ts_event_ns: event.ts_event.as_u64(),
            authority: ExitOrderAuthorityObservation::Lifecycle,
        });
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn handle_order_fill_voided(&mut self, event: &nautilus_model::events::OrderFillVoided) {
        let Some(_) = self
            .exposure
            .exit_pending_snapshot()
            .filter(|exit| exit.pending_exit.client_order_id == event.client_order_id)
        else {
            self.enter_fill_void_exit_recovery(event);
            return;
        };
        self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id: event.client_order_id,
            instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderFilled,
            source: ORDER_LIFECYCLE_SOURCE_RECONCILE_PASS,
            raw_reason_text: event.reason.as_ref().map(ToString::to_string),
            ts_event_ns: event.ts_event.as_u64(),
            authority: ExitOrderAuthorityObservation::Correction {
                cause: BoltV3RecoveredExitCause::FillVoidReopen,
            },
        });
    }
}

nautilus_trading::nautilus_strategy!(BinaryOracleEdgeTaker, {
    fn on_order_filled(&mut self, event: &nautilus_model::events::OrderFilled) {
        self.handle_order_filled(event);
    }

    fn on_order_fill_voided(&mut self, event: &nautilus_model::events::OrderFillVoided) {
        self.handle_order_fill_voided(event);
    }

    fn on_order_canceled(&mut self, event: &nautilus_model::events::OrderCanceled) {
        self.handle_order_canceled(event);
    }

    fn on_order_rejected(&mut self, event: nautilus_model::events::OrderRejected) {
        self.record_entry_reject(&event);
        self.resolve_pending_entry_terminal_event(PendingEntryTerminalEventInput {
            client_order_id: event.client_order_id,
            event_instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderRejected,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_REJECTED,
            raw_reason_text: Some(event.reason.to_string()),
            ts_event_ns: event.ts_event.as_u64(),
            terminal_proves_zero_fill: true,
        });
        self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id: event.client_order_id,
            instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderRejected,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_REJECTED,
            raw_reason_text: Some(event.reason.to_string()),
            ts_event_ns: event.ts_event.as_u64(),
            authority: ExitOrderAuthorityObservation::Lifecycle,
        });
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn on_order_denied(&mut self, event: nautilus_model::events::OrderDenied) {
        self.record_entry_reject_state(
            event.client_order_id,
            event.instrument_id,
            event.reason.as_str(),
        );
        self.resolve_pending_entry_terminal_event(PendingEntryTerminalEventInput {
            client_order_id: event.client_order_id,
            event_instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderDenied,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_DENIED,
            raw_reason_text: Some(event.reason.to_string()),
            ts_event_ns: event.ts_event.as_u64(),
            terminal_proves_zero_fill: true,
        });
        self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id: event.client_order_id,
            instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderDenied,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_DENIED,
            raw_reason_text: Some(event.reason.to_string()),
            ts_event_ns: event.ts_event.as_u64(),
            authority: ExitOrderAuthorityObservation::Lifecycle,
        });
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn on_order_expired(&mut self, event: nautilus_model::events::OrderExpired) {
        self.resolve_pending_entry_terminal_event(PendingEntryTerminalEventInput {
            client_order_id: event.client_order_id,
            event_instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderExpired,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_EXPIRED,
            raw_reason_text: None,
            ts_event_ns: event.ts_event.as_u64(),
            terminal_proves_zero_fill: false,
        });
        self.reconcile_exit_order_lifecycle(ExitOrderLifecycleObservationInput {
            client_order_id: event.client_order_id,
            instrument_id: event.instrument_id,
            transition: OrderLifecycleTransition::OrderExpired,
            source: ORDER_LIFECYCLE_SOURCE_ORDER_EXPIRED,
            raw_reason_text: None,
            ts_event_ns: event.ts_event.as_u64(),
            authority: ExitOrderAuthorityObservation::Lifecycle,
        });
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn on_position_opened(&mut self, _event: nautilus_model::events::PositionOpened) {
        self.materialize_position_from_event(
            PositionMaterializationSpec {
                instrument_id: _event.instrument_id,
                position_id: _event.position_id,
                entry_order_side: _event.entry,
                side: _event.side,
                quantity: _event.quantity,
                avg_px_open: _event.avg_px_open,
            },
            _event.ts_event.as_u64(),
        );
    }

    fn on_position_changed(&mut self, _event: nautilus_model::events::PositionChanged) {
        self.materialize_position_from_event(
            PositionMaterializationSpec {
                instrument_id: _event.instrument_id,
                position_id: _event.position_id,
                entry_order_side: _event.entry,
                side: _event.side,
                quantity: _event.quantity,
                avg_px_open: _event.avg_px_open,
            },
            _event.ts_event.as_u64(),
        );
    }

    fn on_position_closed(&mut self, event: nautilus_model::events::PositionClosed) {
        if self.quarantine_foreign_venue_event(event.instrument_id) {
            return;
        }
        if let Some((phase, exit_pending)) =
            self.exposure.exit_lifecycle().and_then(|(phase, exit)| {
                (exit.pending_exit.position_id == Some(event.position_id)).then_some((phase, exit))
            })
        {
            if phase == ExitLifecyclePhase::TerminalAwaitingPosition {
                self.try_release_terminal_exit(
                    &exit_pending,
                    ORDER_LIFECYCLE_SOURCE_POSITION_EVENT,
                    None,
                    event.ts_event.as_u64(),
                );
            }
            return;
        }
        if self
            .exposure
            .exit_authority_recovery_hold()
            .is_some_and(|hold| hold.pending_exit.position_id == Some(event.position_id))
        {
            self.try_recover_exit_authority_hold(event.ts_event.as_u64());
            return;
        }
        match self.nt_canonical_open_position_projection() {
            Ok(Some(position)) => {
                self.materialize_position_from_truth(
                    position,
                    event.ts_event.as_u64(),
                    ORDER_LIFECYCLE_SOURCE_POSITION_EVENT,
                );
                return;
            }
            Ok(None) => {}
            Err(error) => {
                self.enter_blind_settlement_recovery(error);
                return;
            }
        }
        // Reclaim the exit-evidence flood-guard entry for this terminal position:
        // a closed position never re-emits exit evidence, so its dedup key is dead
        // state. Removal here is behavior-neutral and bounds the map over a long run.
        self.last_exit_evidence_outcome.remove(&event.position_id);
        // The adjacent-repeat guard has to be reclaimed here too, and this is not
        // symmetry for its own sake: NautilusTrader reuses `{instrument}-{strategy}`
        // as the netting `PositionId`, so a later position carries the same id as
        // this one. Leaving the key behind lets that position's *first* exit
        // decision match a closed position's last and be suppressed -- silence on
        // the one record a new position most needs.
        if self
            .last_recorded_exit_decision
            .as_ref()
            .is_some_and(|key| key.position_id.as_deref() == Some(event.position_id.as_str()))
        {
            self.last_recorded_exit_decision = None;
        }
        let managed_position_close = match &self.exposure {
            ExposureState::Managed(position) if position.position_id == event.position_id => {
                Some(position.pending_entry.clone())
            }
            _ => None,
        };
        if let Some(pending_entry) = managed_position_close {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if let Some(pending_entry) = pending_entry {
                let client_order_id = pending_entry.client_order_id;
                self.exposure = ExposureState::PendingEntry(pending_entry);
                let client_id = ClientId::from(self.config.client_id.as_str());
                if let Err(error) = self.cancel_resting_order(client_order_id, client_id) {
                    log::error!(
                        "binary_oracle_edge_taker external position close could not cancel pending entry: strategy_id={} client_order_id={} error={error}",
                        self.config.strategy_id,
                        client_order_id,
                    );
                }
            } else {
                self.exposure = ExposureState::Flat;
            }
            self.refresh_book_subscriptions_for_current_state();
            self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
            return;
        }

        let exit_pending_close = self
            .exposure
            .exit_pending_snapshot()
            .filter(|exit_pending| {
                exit_pending.pending_exit.position_id == Some(event.position_id)
            });
        if let Some(exit_pending) = exit_pending_close {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            let closed_market_id = exit_pending.pending_exit.market_id.or_else(|| {
                exit_pending
                    .position
                    .and_then(|managed| managed.lifecycle.market_id_owned())
            });
            self.exposure = ExposureState::Flat;
            if let Some(market_id) = closed_market_id {
                self.arm_market_cooldown(&market_id, event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
            }
        } else if matches!(
            &self.exposure,
            ExposureState::UnsupportedObserved(observed)
                if observed.context.position_id == event.position_id
        ) {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if matches!(
                &self.exposure,
                ExposureState::UnsupportedObserved(observed)
                    if observed.context.position_id == event.position_id
            ) {
                self.exposure = ExposureState::Flat;
            }
        } else {
            // Entry reconciliation may not have a position id yet; the instrument is the
            // strongest available key for a close that races ahead of position materialization.
            let entry_reconcile_close = match &self.exposure {
                ExposureState::EntryReconcilePending { pending, .. }
                    if pending.instrument_id == event.instrument_id =>
                {
                    Some(pending.clone())
                }
                _ => None,
            };
            if let Some(pending) = entry_reconcile_close {
                self.exposure = ExposureState::Flat;
                self.record_order_lifecycle_evidence(OrderLifecycleEvidenceInput {
                    transition: OrderLifecycleTransition::PositionClosed,
                    outcome: OrderLifecycleOutcome::Flat,
                    source: ORDER_LIFECYCLE_SOURCE_POSITION_EVENT,
                    market_id: pending.lifecycle.market_id_owned(),
                    instrument_id: Some(event.instrument_id),
                    position_id: Some(event.position_id),
                    client_order_id: Some(pending.client_order_id),
                    prior_client_order_id: None,
                    raw_reason_text: None,
                    order_side: None,
                    filled_quantity: None,
                    residual_quantity: None,
                    ts_event_ns: Some(event.ts_event.as_u64()),
                });
            }
        }
        self.refresh_book_subscriptions_for_current_state();
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }
});

pub const KEY: &str = stringify!(binary_oracle_edge_taker);

impl StrategyBuilder for BinaryOracleEdgeTakerBuilder {
    type Strategy = BinaryOracleEdgeTaker;

    fn kind() -> &'static str {
        KEY
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        let Some(table) = raw.as_table() else {
            Self::push_wrong_type(
                errors,
                field_prefix.to_string(),
                BinaryOracleEdgeTakerFieldType::Table,
                raw,
            );
            return;
        };

        Self::validate_table(table, field_prefix, errors);
    }

    fn build_typed(raw: &Value, context: &StrategyBuildContext) -> Result<Self::Strategy> {
        Self::build_strategy(raw, context)
    }
}

fn expire_time_from_config(value: Option<u64>) -> Option<UnixNanos> {
    value.map(UnixNanos::from)
}

fn trigger_price_from_config(
    prefix: &'static str,
    trigger_price: Option<f64>,
    price_precision: u8,
) -> Result<Option<Price>> {
    price_from_config(prefix, "trigger_price", trigger_price, price_precision)
}

fn activation_price_from_config(
    prefix: &'static str,
    activation_price: Option<f64>,
    price_precision: u8,
) -> Result<Option<Price>> {
    price_from_config(
        prefix,
        "activation_price",
        activation_price,
        price_precision,
    )
}

fn price_from_config(
    prefix: &'static str,
    field: &'static str,
    value: Option<f64>,
    price_precision: u8,
) -> Result<Option<Price>> {
    value
        .map(|value| {
            anyhow::ensure!(
                is_positive_finite(value),
                "{prefix}_{field} must be positive and finite"
            );
            Ok(Price::new(value, price_precision))
        })
        .transpose()
}

fn trailing_offset_from_config(
    prefix: &'static str,
    trailing_offset: Option<f64>,
) -> Result<Option<Decimal>> {
    trailing_offset
        .map(|value| {
            anyhow::ensure!(
                is_positive_finite(value),
                "{prefix}_trailing_offset must be positive and finite"
            );
            Decimal::from_f64(value)
                .ok_or_else(|| anyhow::anyhow!("{prefix}_trailing_offset must be decimal"))
        })
        .transpose()
}

const INITIAL_COUNTER_U64: u64 = 0;
const COUNTER_INCREMENT_U64: u64 = 1;
const NANOS_PER_MILLI_U64: u64 = 1_000_000;
const NANOS_PER_SECOND_U64: u64 = 1_000_000_000;
const CONFIG_FIELD_OMS_TYPE: &str = "oms_type";
const CONFIG_FIELD_ENTRY_ORDER_SIDE: &str = "entry_order_side";
const CONFIG_FIELD_ENTRY_ORDER_POSITION_SIDE: &str = "entry_order_position_side";
const CONFIG_FIELD_EXIT_ORDER_SIDE: &str = "exit_order_side";
const CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE: &str = "exit_order_position_side";
const CONFIG_FIELD_FORCED_EXIT_ORDER_SIDE: &str = "forced_exit_order_side";
const CONFIG_FIELD_FORCED_EXIT_ORDER_POSITION_SIDE: &str = "forced_exit_order_position_side";
const ORDER_CONFIGURATION_PREFIX_ENTRY: &str = "entry";
const ORDER_CONFIGURATION_PREFIX_EXIT: &str = "exit";
const SELECTION_BLOCK_REASON_TARGET_SELECTION_BLOCKED: &str = "target_selection_blocked";
const EVIDENCE_REASON_DERIVED_FROM_LEAD_GAP_JITTER_AND_TIME: &str =
    "derived_from_lead_gap_jitter_and_time";
const EVIDENCE_REASON_UNCERTAINTY_BAND_UNAVAILABLE: &str = "uncertainty_band_unavailable";
const EVIDENCE_REASON_NO_FAST_VENUE_CLEARED_LEAD_QUALITY_THRESHOLDS: &str =
    "no_fast_venue_cleared_lead_quality_thresholds";
const EVIDENCE_REASON_LEAD_QUALITY_THRESHOLDS_APPLIED_TO_LIVE_FAST_SPOT_SELECTION: &str =
    "lead_quality_thresholds_applied_to_live_fast_spot_selection";
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct LeadVenueSignal {
    venue_name: String,
    price: Option<f64>,
    observed_ts_ms: Option<u64>,
    age_ms: u64,
    jitter_ms: u64,
    agreement_corr: Probability,
    effective_weight: f64,
    lead_gap_probability: Probability,
}

#[cfg(test)]
impl LeadVenueSignal {
    fn is_eligible(&self, min_agreement_corr: f64, max_jitter_ms: u64) -> bool {
        self.agreement_corr.value() >= min_agreement_corr
            && self.jitter_ms <= max_jitter_ms
            && self.effective_weight.is_finite()
            && self.effective_weight > 0.0
    }
}

#[cfg(test)]
fn arbitrate_lead_reference(
    candidates: &[LeadVenueSignal],
    min_agreement_corr: f64,
    max_jitter_ms: u64,
) -> Option<&LeadVenueSignal> {
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| {
            lead_composite_score(candidate, min_agreement_corr, max_jitter_ms)
                .map(|score| (candidate, score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(_, left_score), (_, right_score)| right_score.total_cmp(left_score));

    let (best_candidate, best_score) = ranked.first().copied()?;
    if ranked
        .get(1)
        .is_some_and(|(_, second_score)| second_score == &best_score)
    {
        return None;
    }

    Some(best_candidate)
}

#[cfg(test)]
fn lead_composite_score(
    candidate: &LeadVenueSignal,
    min_agreement_corr: f64,
    max_jitter_ms: u64,
) -> Option<f64> {
    if !candidate.is_eligible(min_agreement_corr, max_jitter_ms) {
        return None;
    }

    let freshness_score = 1.0 / (candidate.age_ms as f64 + 1.0);
    let jitter_score = 1.0 / (candidate.jitter_ms as f64 + 1.0);

    Some(candidate.agreement_corr.value() + freshness_score + jitter_score)
}

#[cfg(test)]
fn best_healthy_oracle_price(snapshot: &ReferenceSnapshot) -> Option<f64> {
    snapshot
        .venues
        .iter()
        .filter(|venue| {
            venue.venue_kind == VenueKind::Oracle
                && !venue.stale
                && matches!(venue.health, VenueHealth::Healthy)
                && venue.effective_weight.is_finite()
                && venue.effective_weight > 0.0
                && venue
                    .observed_price
                    .is_some_and(|price| price.is_finite() && price > 0.0)
        })
        .max_by(|lhs, rhs| {
            lhs.effective_weight
                .total_cmp(&rhs.effective_weight)
                .then_with(|| lhs.observed_ts_ms.cmp(&rhs.observed_ts_ms))
                .then_with(|| lhs.venue_name.cmp(&rhs.venue_name))
        })
        .and_then(|venue| venue.observed_price)
}

fn outcome_side_to_evidence(side: OutcomeSide) -> EvidenceOutcomeSide {
    match side {
        OutcomeSide::Up => EvidenceOutcomeSide::Up,
        OutcomeSide::Down => EvidenceOutcomeSide::Down,
    }
}

fn settlement_leg_for_outcome(side: OutcomeSide) -> Leg {
    match side {
        OutcomeSide::Up => Leg::Yes,
        OutcomeSide::Down => Leg::No,
    }
}

fn settlement_position_realized_pnl_observation(
    account_id: &str,
    evidence: &SettlementFact,
    settlement_currency: Currency,
) -> Result<PositionRealizedPnlObservation> {
    Ok(PositionRealizedPnlObservation {
        account_id: account_id.to_string(),
        instrument_id: evidence.instrument_id.clone(),
        position_id: evidence.position_id.clone(),
        event_id: Some(evidence.settlement_key.clone()),
        observed: RealizedPnlObservation {
            source: settlement_kind(),
            observed_at_unix_nanos: evidence.resolution_ts_event_ns,
            realized_pnl: Decimal::from_str(&evidence.realized_pnl).with_context(|| {
                format!(
                    "settlement evidence realized_pnl parse failed for key `{}`",
                    evidence.settlement_key
                )
            })?,
            settlement_currency,
        },
        cumulative_realized_pnl: false,
        closes_position: true,
    })
}

fn evidence_order_side(value: OrderSide) -> EvidenceOrderSide {
    match value {
        OrderSide::NoOrderSide => EvidenceOrderSide::Unspecified,
        OrderSide::Buy => EvidenceOrderSide::Buy,
        OrderSide::Sell => EvidenceOrderSide::Sell,
    }
}

fn settlement_key_for_position(position: &OpenPositionState) -> Result<String> {
    let mut key = settlement_product_id(position.instrument_id)?;
    key.push(':');
    key.push_str(position.position_id.as_ref());
    Ok(key)
}

fn managed_position_context(
    position: OpenPositionState,
    origin: ManagedPositionOrigin,
    pending_entry: Option<PendingEntryState>,
) -> ManagedPositionContext {
    ManagedPositionContext {
        lifecycle: position.lifecycle,
        instrument_id: position.instrument_id,
        position_id: position.position_id,
        book: position.book,
        origin,
        pending_entry,
    }
}

fn open_position_from_nt_projection(
    context: ManagedPositionContext,
    spec: PositionMaterializationSpec,
) -> Option<OpenPositionState> {
    (spec.position_id == context.position_id && spec.instrument_id == context.instrument_id)
        .then_some(OpenPositionState {
            lifecycle: context.lifecycle,
            instrument_id: context.instrument_id,
            position_id: context.position_id,
            entry_order_side: spec.entry_order_side,
            side: spec.side,
            quantity: spec.quantity,
            avg_px_open: spec.avg_px_open,
            book: context.book,
        })
}

fn settlement_position_key(
    position: &OpenPositionState,
    settlement_key: String,
) -> SettlementPositionKey {
    SettlementPositionKey {
        settlement_key,
        position_id: position.position_id.to_string(),
        interval_end_ms: position.lifecycle.interval_end_ms(),
    }
}

fn settlement_product_id(instrument_id: InstrumentId) -> Result<String> {
    prediction_market_product_id_from_instrument_id(&instrument_id).ok_or_else(|| {
        anyhow::anyhow!(
            "settlement product id is unbound for instrument `{}`",
            instrument_id
        )
    })
}

fn forced_flat_reason_to_evidence(reason: &ForcedFlatReason) -> EvidenceForcedFlatReason {
    match reason {
        ForcedFlatReason::Freeze => EvidenceForcedFlatReason::Freeze,
        ForcedFlatReason::StaleReference => EvidenceForcedFlatReason::StaleReference,
        ForcedFlatReason::ThinBook => EvidenceForcedFlatReason::ThinBook,
        ForcedFlatReason::MetadataMismatch => EvidenceForcedFlatReason::MetadataMismatch,
        ForcedFlatReason::FastVenueIncoherent => EvidenceForcedFlatReason::FastVenueIncoherent,
    }
}

fn exposure_occupancy_to_evidence(occupancy: ExposureOccupancy) -> EvidenceExposureOccupancy {
    match occupancy {
        ExposureOccupancy::PendingEntry => EvidenceExposureOccupancy::PendingEntry,
        ExposureOccupancy::EntryReconcilePending => {
            EvidenceExposureOccupancy::EntryReconcilePending
        }
        ExposureOccupancy::ManagedPosition => EvidenceExposureOccupancy::ManagedPosition,
        ExposureOccupancy::ExitPending => EvidenceExposureOccupancy::ExitPending,
        ExposureOccupancy::UnsupportedObserved => EvidenceExposureOccupancy::UnsupportedObserved,
        ExposureOccupancy::BlindRecovery => EvidenceExposureOccupancy::BlindRecovery,
    }
}

fn should_report_one_position_gate_violation(occupancy: ExposureOccupancy) -> bool {
    matches!(
        occupancy,
        ExposureOccupancy::EntryReconcilePending
            | ExposureOccupancy::UnsupportedObserved
            | ExposureOccupancy::BlindRecovery
    )
}

fn should_warn_on_exit_submission_block(reason: Option<EvidenceExitBlockedReason>) -> bool {
    !matches!(
        reason,
        Some(
            EvidenceExitBlockedReason::NoOpenPosition
                | EvidenceExitBlockedReason::ExitAlreadyPending
                | EvidenceExitBlockedReason::EntryOrderStillWorking
                | EvidenceExitBlockedReason::PositionIntervalEnded
                | EvidenceExitBlockedReason::PositionIntervalUnknown
                | EvidenceExitBlockedReason::ExitHold
        )
    )
}

fn classify_entry_reject_reason(raw_reason: &str) -> Option<EntryRejectClass> {
    let reason = raw_reason.to_lowercase();
    if reason.contains("precision")
        || reason.contains("accuracy")
        || reason.contains("invalid amount")
        || reason.contains("invalid order amount")
        || reason.contains("min size")
        || reason.contains("minimum size")
        || reason.contains("minimum notional")
        || reason.contains("too small")
        || reason_has_ascii_token(&reason, "tick")
    {
        return Some(EntryRejectClass::Malformed);
    }

    if reason.contains("insufficient balance")
        || reason.contains("not enough balance")
        || reason.contains("balance is not enough")
        || (reason_has_ascii_token(&reason, "balance")
            && reason_has_ascii_token(&reason, "allowance"))
    {
        return Some(EntryRejectClass::Balance);
    }

    if reason_has_ascii_token(&reason, "fok")
        && (reason.contains("no match")
            || reason.contains("not match")
            || reason.contains("could not be matched")
            || reason.contains("unfillable")
            || reason.contains("not fill")
            || reason.contains("fill or kill"))
    {
        return Some(EntryRejectClass::Unfillable);
    }

    None
}

fn reason_has_ascii_token(reason: &str, token: &str) -> bool {
    reason
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part == token)
}

#[cfg(test)]
mod tests;
