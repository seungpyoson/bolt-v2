use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[cfg(test)]
use std::str::FromStr;

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, component::Component, timer::TimeEvent};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{CustomData, DataType, IndexPriceUpdate, QuoteTick, TradeTick},
    enums::PositionSide,
};
use nautilus_model::{
    enums::{OrderSide, OrderType, TimeInForce},
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, Venue},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::{
    Decimal,
    prelude::{FromPrimitive, ToPrimitive},
};
use toml::Value;

use crate::{
    bolt_v3_binary_outcome_edge::{
        BinaryOutcomeEdgeBlockReason, BinaryOutcomeEdgeInputs, BinaryOutcomeEdgeResult,
        evaluate_binary_outcome_edge,
    },
    bolt_v3_book_sizing::{
        OutcomeBookState, OutcomeBookSubscriptions, should_replace_book_subscriptions,
    },
    bolt_v3_config::{ReferencePriceBlock, ReferencePriceDriftPolicy},
    bolt_v3_decision_evidence::{
        BoltV3EntrySkipEvidence, BoltV3EntrySkipReasonCategory, BoltV3ExitDecisionEvidence,
        BoltV3ExitEvaluationEvidence, BoltV3ExitRvGateResult, BoltV3ExitRvSnapshotBlocker,
        BoltV3ExitTriggerSource, BoltV3ExposureOccupancy, BoltV3ForcedFlatReason,
        BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OutcomeSide,
        BoltV3RealizedVolatilitySourceDiagnosticEvidence, BoltV3RvGateResult,
        BoltV3StrategyInputEvidenceSnapshot, number_evidence as evidence_number,
        option_number_evidence as option_evidence_number,
        option_probability_evidence as option_evidence_probability, probability_evidence,
        realized_vol_blocker_to_exit_evidence, realized_volatility_aggregation_evidence_label,
        realized_volatility_block_reason_evidence_label,
        realized_volatility_pricing_component_evidence_label,
    },
    bolt_v3_executable_cost::{
        ExactSizeVwap, ExecutableBookQuote, ExecutableCostBreakdown, executable_cost_breakdown,
        price_exact_size_vwap,
    },
    bolt_v3_market_families::{self, FairProbabilityInputs, OutcomeSide},
    bolt_v3_numeric::{
        BPS_DENOMINATOR, MIDPOINT_DIVISOR_F64, MILLIS_PER_SECOND_U64, Probability,
        SECONDS_PER_YEAR_F64, UNIT_F64, is_non_negative_finite, is_positive_finite,
        notional_float_tolerance,
    },
    bolt_v3_order_execution::{
        BoltV3SubmitContext, BoltV3SubmitRoutingOutcome, BoltV3SubmitRoutingRequest,
    },
    bolt_v3_position_contract::is_observed_open_side,
    bolt_v3_providers::{
        market_quote_buy_min_notional_for_execution_venue,
        normalize_base_order_quantity_for_execution_venue as provider_normalize_base_order_quantity,
    },
    bolt_v3_reference_price::{
        ReferencePriceSelection, ReferencePriceSelector, ReferencePriceSourceHealth,
        ReferencePriceSourceSpec, ReferencePriceSourceStatus, ReferencePriceUpdate, ReferenceQuote,
        reference_price_source_is_runtime_available, reference_price_source_is_unsupported,
    },
    bolt_v3_sizing::{RobustSizingInputs, choose_robust_size},
    bolt_v3_submit_admission::{
        BoltV3RiskReducingExitPositionInput, BoltV3SubmitAdmissionRequest,
        BoltV3SubmitAdmissionRequestInput, BoltV3SubmitLifecyclePolicy,
        build_submit_admission_request_from_order, limit_notional_exceeds_sized_notional,
    },
    bolt_v3_taker_pricing::{
        FastSpotObservation, TakerPricingConfig, TakerPricingRequest,
        TakerPricingState as PricingState,
    },
    bolt_v3_taker_updown_signal::{
        SideSelectionInputs, UncertaintyBandInputs, choose_entry_side, outcome_side_evidence_label,
        time_uncertainty_probability, uncertainty_band_probability,
    },
    bolt_v3_timestamp_domain::{NtStrategyClockMs, VenueEventMs},
    bolt_v3_trade_flow::SignedTradeFlowConfig,
    strategies::registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder, ValidationError},
};

#[cfg(test)]
use nautilus_model::enums::{
    AggressorSide, BookAction, OmsType as NtOmsType, TrailingOffsetType, TriggerType,
};

#[cfg(test)]
use crate::{
    bolt_v3_decision_evidence::{
        BoltV3EntryPricingBlockReason, BoltV3ExitBlockedReason, BoltV3ExitDecisionOutcome,
    },
    bolt_v3_market_families::{MarketSelectionOutcome, SelectedMarketSourceIdentity},
    bolt_v3_submit_admission::{BoltV3RiskReducingExitProof, BoltV3SubmitIntentKind},
    bolt_v3_taker_pricing::VenueTimingState,
    bolt_v3_taker_updown_signal::{price_agreement_corr, price_gap_probability},
    strategies::registry::FeeProvider,
};

mod selection;

#[cfg(test)]
use self::selection::CandidateMarket;
use self::selection::{
    RuntimeSelectionSnapshot, SelectionPhase, SelectionState, apply_selection_snapshot_to_active,
    idle_selection_snapshot, same_market_interval_rollover, selected_market_on_execution_venue,
    selection_book_subscriptions, selection_snapshot_from_instruments,
    strategy_input_market_selection_outcome,
};

mod config;

pub use self::config::BinaryOracleEdgeTakerBuilder;
#[cfg(test)]
use self::config::BinaryOracleEdgeTakerOrderConfig;
use self::config::{BinaryOracleEdgeTakerConfig, BinaryOracleEdgeTakerFieldType};

mod exposure;

use self::exposure::{
    BlindRecoveryReason, BlindRecoveryState, ConfiguredPositionContract, EntryReconcileReason,
    ExitPendingState, ExposureOccupancy, ExposureState, ManagedPositionOrigin,
    ManagedPositionState, OpenPositionState, PendingEntryState, PendingExitState,
    PositionMaterializationSpec, UnsupportedObservedReason, UnsupportedObservedState,
    infer_strategy_position_side_from_entry_fill, managed_position_effective_entry_cost,
    supports_strategy_managed_position,
};
use crate::bolt_v3_feed_health::{
    ForcedFlatInputs, ForcedFlatReason, evaluate_forced_flat_predicates,
};

mod entry_decision;

use self::entry_decision::{
    EntryBlockReason, EntryEvaluation, EntryEvaluationLogFields, EntryGateDecision,
    EntryPricingBlockReason, EntryPricingInputs, EntrySkipDedupeKey, EntrySubmissionDecision,
    ForcedFlatEvidenceInputs, RealizedVolatilityEvidenceFields, entry_block_reason_to_evidence,
    entry_pricing_block_reason_from_taker, entry_pricing_block_reason_to_evidence,
    entry_skip_reason_category_from_str, push_executable_edge_pricing_block,
};

mod exit_decision;

use self::exit_decision::{
    ExitDecision, ExitDecisionDedupeKey, ExitEvaluation, ExitEvaluationLogFields,
    ExitEvaluationTriggerContext, ExitOutcomeKey, ExitSubmissionDecision, evaluate_exit_decision,
    exit_decision_evidence_from_optional,
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
    ActiveMarketState, MarketLifecycleLedger, OutcomeFeeState,
    reference_current_price_boundary_changed, refresh_fee_readiness_for_active,
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

#[derive(Debug, Clone, Copy)]
struct ExecutableEntryProbe {
    order_side: OrderSide,
    vwap: ExactSizeVwap,
    fee_bps: f64,
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

fn reference_price_source_provider_identifier<'a>(
    reference_price: &'a ReferencePriceBlock,
    source_id: &str,
) -> Option<&'a str> {
    let source = reference_price.sources.get(source_id)?;
    source.instrument_id.as_deref().or(source.symbol.as_deref())
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

fn executable_edge_fee_bps(result: Option<BinaryOutcomeEdgeResult>) -> Option<f64> {
    let result = result?;
    let gross_cost_cents = result.cost_breakdown.gross_cost_cents;
    if !is_positive_finite(gross_cost_cents) {
        return None;
    }
    Some(result.cost_breakdown.fee_cost_cents / gross_cost_cents * BPS_DENOMINATOR)
        .filter(|value| is_non_negative_finite(*value))
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
        realized_volatility_max_source_age_ms: None,
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

fn exit_rv_gate_result_from_shared(result: BoltV3RvGateResult) -> BoltV3ExitRvGateResult {
    match result {
        BoltV3RvGateResult::Accepted => BoltV3ExitRvGateResult::Accepted,
        BoltV3RvGateResult::MissingSnapshot => BoltV3ExitRvGateResult::MissingSnapshot,
        BoltV3RvGateResult::MissingEvaluationEventTime => {
            BoltV3ExitRvGateResult::MissingEvaluationEventTime
        }
        BoltV3RvGateResult::RejectedFutureDated => BoltV3ExitRvGateResult::RejectedFutureDated,
        BoltV3RvGateResult::RejectedStale => BoltV3ExitRvGateResult::RejectedStale,
        BoltV3RvGateResult::RejectedNotReady => BoltV3ExitRvGateResult::RejectedNotReady,
    }
}

fn reference_quote_outside_live_window(
    quote: &ReferenceQuote,
    interval_start_ms: VenueEventMs,
    interval_end_ms: VenueEventMs,
    now_ms: NtStrategyClockMs,
    max_source_age_ms: u64,
) -> bool {
    let observed_ts_ms = VenueEventMs::new(quote.observed_ts_ms());
    observed_ts_ms < interval_start_ms
        || observed_ts_ms > interval_end_ms
        || observed_ts_ms.value() > now_ms.value()
        || now_ms.value().saturating_sub(observed_ts_ms.value()) > max_source_age_ms
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
    last_reported_exposure_occupancy: Cell<Option<ExposureOccupancy>>,
    last_recorded_entry_skip: Option<EntrySkipDedupeKey>,
    last_recorded_exit_decision: Option<ExitDecisionDedupeKey>,
    pricing: PricingState,
    latest_signal_quote: Option<FastSpotObservation>,
    latest_selected_reference_quote: Option<SelectedReferenceQuoteEvidence>,
    reference_price_selector: Option<ReferencePriceSelector>,
    reference_price_quotes: BTreeMap<String, ReferenceQuote>,
    reference_price_source_health: BTreeMap<String, ReferencePriceSourceHealth>,
    selection_missing_since_ms: Option<u64>,
    resolution_strike_index_subscription: Option<InstrumentId>,
    resolution_strike_custom_subscription: Option<DataType>,
    resolution_strike_fetch_sequence: u64,
    entry_reject_state: BTreeMap<InstrumentId, EntryRejectState>,
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
            last_reported_exposure_occupancy: Cell::new(None),
            last_recorded_entry_skip: None,
            last_recorded_exit_decision: None,
            pricing,
            latest_signal_quote: None,
            latest_selected_reference_quote: None,
            reference_price_selector,
            reference_price_quotes: BTreeMap::new(),
            reference_price_source_health,
            selection_missing_since_ms: None,
            resolution_strike_index_subscription: None,
            resolution_strike_custom_subscription: None,
            resolution_strike_fetch_sequence: INITIAL_COUNTER_U64,
            entry_reject_state: BTreeMap::new(),
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
        let previous_phase = previous_active.phase;
        let previous_fee_instrument_ids = previous_active.outcome_fees.instrument_ids();
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
        let reactivated_into_active =
            previous_phase != SelectionPhase::Active && self.active.phase == SelectionPhase::Active;
        let same_market_interval_rollover =
            same_market_interval_rollover(&previous_active, &self.active);
        let next_fee_instrument_ids = self.active.outcome_fees.instrument_ids();
        if previous_fee_instrument_ids != next_fee_instrument_ids
            || (same_market_interval_rollover && !next_fee_instrument_ids.is_empty())
            || (reactivated_into_active && !next_fee_instrument_ids.is_empty())
        {
            self.trigger_fee_warm_for_market();
            self.refresh_fee_readiness();
        }
        self.refresh_current_reference_price_selection_at(now_ms);
        self.sync_exposure_context_from_active();
        self.prune_market_lifecycle(now_ms);
        self.refresh_book_subscriptions_for_current_state();
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                now_ms,
                ExitEvaluationTriggerContext::new(
                    BoltV3ExitTriggerSource::SelectionUpdate,
                    now_ms,
                    None,
                ),
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

    fn observe_signal_quote(&mut self, quote: &FastSpotObservation) {
        self.latest_signal_quote = Some(quote.clone());
        self.pricing
            .observe_signal_quote(quote, &taker_pricing_config(&self.config));
        self.after_signal_quote_observed(quote.observed_ts_ms, quote.received_ts_ms);
    }

    fn observe_invalid_signal_quote(
        &mut self,
        venue: &str,
        observed_ts_ms: u64,
        received_ts_ms: Option<u64>,
    ) {
        self.latest_signal_quote = None;
        self.pricing
            .observe_invalid_signal_quote(venue, observed_ts_ms);
        self.after_signal_quote_observed(observed_ts_ms, received_ts_ms);
    }

    fn after_signal_quote_observed(&mut self, observed_ts_ms: u64, received_ts_ms: Option<u64>) {
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        self.refresh_fee_readiness();
        self.sync_exposure_context_from_active();
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                observed_ts_ms,
                ExitEvaluationTriggerContext::new(
                    BoltV3ExitTriggerSource::SignalQuote,
                    observed_ts_ms,
                    received_ts_ms,
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
    fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        self.active.observe_reference_snapshot(snapshot);
        self.pricing.observe_reference_snapshot(
            snapshot,
            self.config.lead_agreement_min_corr,
            self.config.lead_jitter_max_ms,
        );
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        self.refresh_fee_readiness();
        self.sync_exposure_context_from_active();
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                snapshot.ts_ms,
                ExitEvaluationTriggerContext::new(
                    BoltV3ExitTriggerSource::ReferenceUpdate,
                    snapshot.ts_ms,
                    None,
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

    fn signal_quote_from_tick(&self, quote: &QuoteTick) -> Option<FastSpotObservation> {
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
            received_ts_ms: Some(quote.ts_init.as_u64() / NANOS_PER_MILLI_U64),
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

    fn refresh_fee_readiness(&mut self) {
        refresh_fee_readiness_for_active(&mut self.active, self.context.fee_provider());
    }

    fn trigger_fee_warm_for_market(&self) {
        let instrument_ids = self.active.outcome_fees.instrument_ids();
        if instrument_ids.is_empty() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for instrument_id in instrument_ids {
            let fee_provider = self.context.fee_provider_arc();
            handle.spawn(async move {
                let _ = fee_provider.warm(instrument_id).await;
            });
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
        // selection structurally impossible and fails closed (P5-5 / Codex P5).
        let execution_venue = self.context.execution_venue();
        let instruments = {
            let cache = self.cache();
            cache
                .instrument_ids(None)
                .into_iter()
                .filter_map(|instrument_id| cache.instrument(instrument_id).cloned())
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

    fn observe_reference_price_update(&mut self, update: &ReferencePriceUpdate) {
        self.ensure_reference_price_runtime_state();
        let quote = match update.to_reference_quote() {
            Ok(quote) => quote,
            Err(error) => {
                self.mark_reference_price_source_status(
                    update.source_id(),
                    ReferencePriceSourceStatus::MalformedFrame,
                    Some(update.observed_ts_ms()),
                    Some(update.received_ts_ms()),
                );
                log::warn!(
                    "binary_oracle_edge_taker malformed reference price update ignored: {error}; source_id={} strategy_id={}",
                    update.source_id(),
                    self.config.strategy_id,
                );
                return;
            }
        };
        let Some(existing_health) = self.reference_price_source_health.get(quote.source_id())
        else {
            return;
        };
        if matches!(
            existing_health.status(),
            ReferencePriceSourceStatus::Disabled | ReferencePriceSourceStatus::UnsupportedSymbol
        ) {
            return;
        }
        if existing_health.provider() != quote.provider() {
            let expected_provider = existing_health.provider().as_str().to_string();
            self.mark_reference_price_source_status(
                quote.source_id(),
                ReferencePriceSourceStatus::MalformedFrame,
                Some(quote.observed_ts_ms()),
                Some(quote.received_ts_ms()),
            );
            log::warn!(
                "binary_oracle_edge_taker reference current price provider mismatch ignored: source_id={} expected_provider={} actual_provider={} strategy_id={}",
                quote.source_id(),
                expected_provider,
                quote.provider().as_str(),
                self.config.strategy_id,
            );
            return;
        }
        if let Some(expected_provider_instrument) = self
            .config
            .reference_current_price
            .as_ref()
            .and_then(|reference_price| {
                reference_price_source_provider_identifier(reference_price, quote.source_id())
            })
            .map(ToString::to_string)
            && quote.provider_instrument() != expected_provider_instrument
        {
            self.mark_reference_price_source_status(
                quote.source_id(),
                ReferencePriceSourceStatus::MalformedFrame,
                Some(quote.observed_ts_ms()),
                Some(quote.received_ts_ms()),
            );
            log::warn!(
                "binary_oracle_edge_taker reference current price provider instrument mismatch ignored: source_id={} expected_provider_instrument={} actual_provider_instrument={} strategy_id={}",
                quote.source_id(),
                expected_provider_instrument,
                quote.provider_instrument(),
                self.config.strategy_id,
            );
            return;
        }

        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        if let (Some(reference_price), Some(interval_start_ms), Some(interval_end_ms)) = (
            self.config.reference_current_price.as_ref(),
            self.active.interval_start_ms,
            self.active.interval_end_ms,
        ) && reference_quote_outside_live_window(
            &quote,
            VenueEventMs::new(interval_start_ms),
            VenueEventMs::new(interval_end_ms),
            NtStrategyClockMs::new(now_ms),
            reference_price.max_source_age_ms,
        ) {
            let newer_than_accepted_quote = self
                .reference_price_quotes
                .get(quote.source_id())
                .is_none_or(|existing| existing.observed_ts_ms() < quote.observed_ts_ms());
            let newer_than_recorded_health = self
                .reference_price_source_health
                .get(quote.source_id())
                .and_then(ReferencePriceSourceHealth::observed_ts_ms)
                .is_none_or(|observed_ts_ms| observed_ts_ms < quote.observed_ts_ms());
            if newer_than_accepted_quote && newer_than_recorded_health {
                self.mark_reference_price_source_status(
                    quote.source_id(),
                    ReferencePriceSourceStatus::Stale,
                    Some(quote.observed_ts_ms()),
                    Some(quote.received_ts_ms()),
                );
            }
            log::warn!(
                "binary_oracle_edge_taker stale reference current price ignored: source_id={} observed_ts_ms={} now_ms={} strategy_id={}",
                quote.source_id(),
                quote.observed_ts_ms(),
                now_ms,
                self.config.strategy_id,
            );
            self.observe_current_reference_price_selection(
                interval_start_ms,
                interval_end_ms,
                now_ms,
            );
            return;
        }

        if self
            .reference_price_quotes
            .get(quote.source_id())
            .is_some_and(|existing| existing.observed_ts_ms() >= quote.observed_ts_ms())
        {
            return;
        }

        match self
            .reference_price_source_health
            .get_mut(quote.source_id())
        {
            Some(health) => health.update(
                ReferencePriceSourceStatus::Available,
                Some(quote.observed_ts_ms()),
                Some(quote.received_ts_ms()),
            ),
            None => {
                self.reference_price_source_health.insert(
                    quote.source_id().to_string(),
                    ReferencePriceSourceHealth::available(&quote),
                );
            }
        }
        match self.reference_price_quotes.get_mut(quote.source_id()) {
            Some(existing) => *existing = quote.clone(),
            None => {
                self.reference_price_quotes
                    .insert(quote.source_id().to_string(), quote.clone());
            }
        }

        let (Some(interval_start_ms), Some(interval_end_ms)) =
            (self.active.interval_start_ms, self.active.interval_end_ms)
        else {
            return;
        };
        self.observe_current_reference_price_selection(interval_start_ms, interval_end_ms, now_ms);
    }

    fn select_current_reference_price(
        &mut self,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
    ) -> Option<ReferencePriceSelection> {
        let selector = self.reference_price_selector.as_mut()?;
        let quotes = self
            .reference_price_quotes
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let selection = selector.select(interval_start_ms, interval_end_ms, now_ms, &quotes);
        self.refresh_reference_price_source_statuses(interval_start_ms, interval_end_ms, now_ms);
        if selection.is_none() {
            self.clear_reference_current_price_selection_state();
        }
        selection
    }

    fn observe_current_reference_price_selection(
        &mut self,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
    ) {
        let Some(selection) =
            self.select_current_reference_price(interval_start_ms, interval_end_ms, now_ms)
        else {
            return;
        };
        let selected_source_id = selection.source_id().to_string();
        if let Some(selected_quote) = self.reference_price_quotes.get(&selected_source_id)
            && self
                .active
                .observe_reference_price_quote(selected_quote, selection.failed_over())
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
            self.refresh_fee_readiness();
        }
    }

    fn refresh_current_reference_price_selection_at(&mut self, now_ms: u64) {
        if let (Some(interval_start_ms), Some(interval_end_ms)) =
            (self.active.interval_start_ms, self.active.interval_end_ms)
        {
            self.observe_current_reference_price_selection(
                interval_start_ms,
                interval_end_ms,
                now_ms,
            );
        }
    }

    fn refresh_reference_price_source_statuses(
        &mut self,
        interval_start_ms: u64,
        interval_end_ms: u64,
        now_ms: u64,
    ) {
        let Some(reference_price) = &self.config.reference_current_price else {
            return;
        };
        let drift_exceeded = reference_price.drift_policy == ReferencePriceDriftPolicy::Block
            && self
                .reference_price_selector
                .as_ref()
                .and_then(ReferencePriceSelector::last_cross_source_drift_bps)
                .is_some_and(|drift_bps| {
                    drift_bps > f64::from(reference_price.max_source_drift_bps)
                });
        let updates = reference_price
            .source_order
            .iter()
            .filter_map(|source_id| {
                let source = reference_price.sources.get(source_id)?;
                if !reference_price_source_is_runtime_available(reference_price, source) {
                    return None;
                }
                let (status, observed_ts_ms, received_ts_ms) =
                    match self.reference_price_quotes.get(source_id) {
                        Some(quote)
                            if reference_quote_outside_live_window(
                                quote,
                                VenueEventMs::new(interval_start_ms),
                                VenueEventMs::new(interval_end_ms),
                                NtStrategyClockMs::new(now_ms),
                                reference_price.max_source_age_ms,
                            ) =>
                        {
                            self.reference_price_source_health
                                .get(source_id)
                                .filter(|health| {
                                    health.status() == ReferencePriceSourceStatus::Stale
                                        && health.observed_ts_ms().is_some_and(|observed_ts_ms| {
                                            observed_ts_ms > quote.observed_ts_ms()
                                        })
                                })
                                .map(|health| {
                                    (
                                        health.status(),
                                        health.observed_ts_ms(),
                                        health.received_ts_ms(),
                                    )
                                })
                                .unwrap_or((
                                    ReferencePriceSourceStatus::Stale,
                                    Some(quote.observed_ts_ms()),
                                    Some(quote.received_ts_ms()),
                                ))
                        }
                        Some(quote) if drift_exceeded => (
                            ReferencePriceSourceStatus::DriftExceeded,
                            Some(quote.observed_ts_ms()),
                            Some(quote.received_ts_ms()),
                        ),
                        Some(quote)
                            if self
                                .reference_price_source_health
                                .get(source_id)
                                .is_some_and(|health| {
                                    health.status() == ReferencePriceSourceStatus::Stale
                                        && health.observed_ts_ms().is_some_and(|observed_ts_ms| {
                                            observed_ts_ms > quote.observed_ts_ms()
                                        })
                                }) =>
                        {
                            let health = self
                                .reference_price_source_health
                                .get(source_id)
                                .expect("reference price source health checked above");
                            (
                                health.status(),
                                health.observed_ts_ms(),
                                health.received_ts_ms(),
                            )
                        }
                        Some(quote) => (
                            ReferencePriceSourceStatus::Available,
                            Some(quote.observed_ts_ms()),
                            Some(quote.received_ts_ms()),
                        ),
                        None => self
                            .reference_price_source_health
                            .get(source_id)
                            .map(|health| match health.status() {
                                ReferencePriceSourceStatus::AuthRejected
                                | ReferencePriceSourceStatus::SubscriptionRejected
                                | ReferencePriceSourceStatus::Stale
                                | ReferencePriceSourceStatus::MalformedFrame
                                | ReferencePriceSourceStatus::Disconnected
                                | ReferencePriceSourceStatus::DriftExceeded => (
                                    health.status(),
                                    health.observed_ts_ms(),
                                    health.received_ts_ms(),
                                ),
                                _ => (ReferencePriceSourceStatus::Silent, None, None),
                            })
                            .unwrap_or((ReferencePriceSourceStatus::Silent, None, None)),
                    };
                Some((source_id.clone(), status, observed_ts_ms, received_ts_ms))
            })
            .collect::<Vec<_>>();
        for (source_id, status, observed_ts_ms, received_ts_ms) in updates {
            self.mark_reference_price_source_status(
                &source_id,
                status,
                observed_ts_ms,
                received_ts_ms,
            );
        }
    }

    fn ensure_reference_price_runtime_state(&mut self) {
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

    fn mark_reference_price_source_status(
        &mut self,
        source_id: &str,
        status: ReferencePriceSourceStatus,
        observed_ts_ms: Option<u64>,
        received_ts_ms: Option<u64>,
    ) {
        if let Some(health) = self.reference_price_source_health.get_mut(source_id) {
            health.update(status, observed_ts_ms, received_ts_ms);
        }
    }

    fn current_market_id(&self) -> Option<&str> {
        self.active.market_id.as_deref()
    }

    fn tracked_observed_position(&self) -> Option<&OpenPositionState> {
        self.exposure.observed_position()
    }

    fn tracked_observed_position_mut(&mut self) -> Option<&mut OpenPositionState> {
        self.exposure.observed_position_mut()
    }

    fn managed_position(&self) -> Option<&ManagedPositionState> {
        self.exposure.managed_position()
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
            .managed_position()
            .and_then(|managed| managed.pending_entry.as_ref())
            .is_some_and(|pending| pending.client_order_id == client_order_id);
        if !matches_pending_entry {
            return;
        }
        if !self.event_instrument_matches_held_exposure(event_instrument_id) {
            return;
        }
        if let Some(managed) = self.exposure.managed_position_mut() {
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
        self.exposure =
            ExposureState::UnsupportedObserved(UnsupportedObservedState { observed, reason });
        self.refresh_book_subscriptions_for_current_state();
    }

    fn bootstrap_recovery_from_cache(&mut self) {
        // Scope recovery to the configured execution venue. The shared NT cache can hold positions
        // from every registered execution client; a foreign-venue position must never be accepted
        // into Managed state because the exit path would build/submit an order for it with no
        // additional venue gate. Filtering the cache read by execution venue makes a wrong-venue
        // recovery structurally impossible and fails closed (P5-5 / Codex P5).
        let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
        let execution_venue = self.context.execution_venue();
        let cached_positions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let cache = self.cache();
            cache
                .positions_open(Some(&execution_venue), None, Some(&strategy_id), None, None)
                .into_iter()
                .map(|position| OpenPositionState {
                    market_id: None,
                    instrument_id: position.instrument_id,
                    position_id: position.id,
                    outcome_side: None,
                    outcome_fees: OutcomeFeeState::empty(),
                    historical_entry_fee_bps: None,
                    entry_order_side: position.entry,
                    side: position.side,
                    quantity: position.quantity,
                    avg_px_open: position.avg_px_open,
                    interval_open: None,
                    selection_published_at_ms: None,
                    seconds_to_expiry_at_selection: None,
                    book: OutcomeBookState::from_instrument_id(position.instrument_id),
                })
                .collect::<Vec<_>>()
        }));

        let cached_positions = match cached_positions {
            Ok(cached_positions) => cached_positions,
            Err(_) => {
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

        if cached_positions.is_empty() {
            self.exposure = ExposureState::Flat;
            return;
        }

        if cached_positions.len() > 1 {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::MultipleOpenPositions {
                    count: cached_positions.len(),
                },
            });
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap found multiple open positions: strategy_id={} position_count={} leaving recovery mode blind to position bootstrap",
                self.config.strategy_id,
                cached_positions.len(),
            );
            return;
        }

        let open_position = cached_positions
            .into_iter()
            .next()
            .expect("checked non-empty recovery position set");
        let exposure = self.bootstrapped_exposure_for(open_position, execution_venue);
        self.exposure = exposure;
    }

    /// Classify a single recovered open position into the exposure state to adopt.
    ///
    /// The recovery probe already scopes the cache read to the execution venue, so a
    /// foreign-venue position should never reach here in production. This is the single
    /// fail-closed adoption decision and re-asserts the venue invariant structurally
    /// (defense in depth) BEFORE any contract check: the exit path would otherwise
    /// build/submit an order for a wrong-venue position with no further venue gate
    /// (P5-5 / Codex P5). A foreign-venue position is quarantined to blind recovery and
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
            ExposureState::Managed(ManagedPositionState {
                position: open_position,
                origin: ManagedPositionOrigin::RecoveryBootstrap,
                pending_entry: None,
            })
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
                observed: open_position,
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

    fn clear_pending_entry_state(&mut self) {
        if matches!(self.exposure, ExposureState::PendingEntry(_)) {
            self.exposure = ExposureState::Flat;
            self.prune_market_lifecycle_at_current_time();
        }
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
            .and_then(|pending| pending.market_id.clone())
        {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .tracked_observed_position()
            .and_then(|position| position.market_id.clone())
        {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .exposure
            .exit_pending()
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
        if !self.active.outcome_fees.market_ready() {
            blocked_by.push(EntryBlockReason::FeesNotReady);
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
        let Some(open_position) = self.managed_position().map(|managed| &managed.position) else {
            return self.active_forced_flat_reasons_at(now_ms);
        };

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

    fn realized_volatility_max_source_age_ms(&self) -> Option<u64> {
        self.context
            .realized_volatility_max_source_age_ms_for_surface(
                &self.config.realized_volatility_surface_id,
            )
    }

    fn runtime_taker_pricing_config(&self) -> TakerPricingConfig<'_> {
        let mut config = taker_pricing_config(&self.config);
        config.realized_volatility_max_source_age_ms = self.realized_volatility_max_source_age_ms();
        config
    }

    fn current_pricing_venue_event_ms(&self) -> Option<VenueEventMs> {
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

    fn current_realized_vol_for_gate_at(
        &self,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> Option<f64> {
        self.pricing.current_realized_vol_at(
            realized_vol_gate_event_ms,
            self.realized_volatility_max_source_age_ms(),
        )
    }

    fn current_realized_vol_source_for_gate_at(
        &self,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> (Option<String>, Option<u64>) {
        self.pricing.current_realized_vol_source_at(
            realized_vol_gate_event_ms,
            self.realized_volatility_max_source_age_ms(),
        )
    }

    #[cfg(test)]
    fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.current_realized_vol_for_gate_at(Some(VenueEventMs::new(now_ms)))
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

    fn current_entry_pricing_inputs_at(
        &self,
        now_ms: u64,
    ) -> std::result::Result<EntryPricingInputs, Vec<EntryPricingBlockReason>> {
        self.pricing
            .entry_pricing_inputs_at(
                &self.runtime_taker_pricing_config(),
                TakerPricingRequest {
                    now_ms,
                    realized_vol_gate_event_ms: self.current_pricing_venue_event_ms(),
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

    fn current_fair_probability_up_at(&self, now_ms: u64) -> Option<Probability> {
        self.pricing
            .entry_pricing_at(
                &self.runtime_taker_pricing_config(),
                TakerPricingRequest {
                    now_ms,
                    realized_vol_gate_event_ms: self.current_pricing_venue_event_ms(),
                    strike_price: self.active.interval_open,
                    seconds_to_market_end: self.current_seconds_to_expiry_at(now_ms),
                },
            )
            .ok()
            .and_then(|result| Probability::new(result.fair_probability_up))
    }

    fn current_position_fast_spot(&self) -> Option<&FastSpotObservation> {
        let open_position = &self.managed_position()?.position;
        if open_position.market_id.as_deref() != self.active.market_id.as_deref() {
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
            &self.runtime_taker_pricing_config(),
            self.current_seconds_to_expiry_at(now_ms),
        )
    }

    fn current_uncertainty_band_probability_at(
        &self,
        now_ms: u64,
        up_fee_bps: f64,
        down_fee_bps: f64,
    ) -> Option<Probability> {
        let seconds_to_expiry = self.current_seconds_to_expiry_at(now_ms)?;
        let realized_vol =
            self.current_realized_vol_for_gate_at(self.current_pricing_venue_event_ms())?;
        self.uncertainty_band_probability_for_seconds(
            seconds_to_expiry,
            realized_vol,
            up_fee_bps,
            down_fee_bps,
        )
    }

    fn uncertainty_band_probability_for_seconds(
        &self,
        seconds_to_expiry: u64,
        realized_vol: f64,
        up_fee_bps: f64,
        down_fee_bps: f64,
    ) -> Option<Probability> {
        let time_uncertainty_probability =
            time_uncertainty_probability(realized_vol, seconds_to_expiry, SECONDS_PER_YEAR_F64)?;
        let fee_uncertainty_probability =
            Probability::clamped(up_fee_bps.max(down_fee_bps) / BPS_DENOMINATOR)?;
        let lead_gap_probability = self.pricing.last_lead_gap_probability?;
        let jitter_penalty_probability = self.pricing.last_jitter_penalty_probability?;

        uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability,
            jitter_penalty_probability,
            time_uncertainty_probability,
            fee_uncertainty_probability,
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
        let (realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.current_realized_vol_source_for_gate_at(self.current_pricing_venue_event_ms());

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
            realized_vol: self
                .current_realized_vol_for_gate_at(self.current_pricing_venue_event_ms()),
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
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
                EVIDENCE_REASON_DERIVED_FROM_LEAD_GAP_JITTER_TIME_AND_FEE
            } else {
                EVIDENCE_REASON_UNCERTAINTY_BAND_UNAVAILABLE
            },
            lead_agreement_corr: self
                .pricing
                .last_lead_agreement_corr
                .map(Probability::value),
            fast_venue_age_ms: self.pricing.last_fast_venue_age_ms,
            fast_venue_jitter_ms: self.pricing.last_fast_venue_jitter_ms,
            up_fee_bps: executable_edge_fee_bps(evaluation.up_executable_edge),
            down_fee_bps: executable_edge_fee_bps(evaluation.down_executable_edge),
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
            up_fee_cost_cents: executable_edge_cost_component(
                evaluation.up_executable_edge,
                |cost| cost.fee_cost_cents,
            ),
            down_fee_cost_cents: executable_edge_cost_component(
                evaluation.down_executable_edge,
                |cost| cost.fee_cost_cents,
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
            sized_fee_bps: executable_edge_fee_bps(evaluation.sized_executable_edge),
            sized_entry_cost: executable_edge_vwap_price(evaluation.sized_executable_edge),
            sized_entry_limit_price: executable_edge_limit_price(evaluation.sized_executable_edge),
            sized_gross_cost_cents: executable_edge_cost_component(
                evaluation.sized_executable_edge,
                |cost| cost.gross_cost_cents,
            ),
            sized_fee_cost_cents: executable_edge_cost_component(
                evaluation.sized_executable_edge,
                |cost| cost.fee_cost_cents,
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
            final_fee_amount_known: false,
            final_fee_amount_reason:
                EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_AND_SIZE_SELECTION,
            submission_instrument_id: submission.instrument_id,
            submission_order_side: submission.order_side,
            submission_price: submission.price,
            submission_quantity_value: submission.quantity_value,
            submission_client_order_id: submission.client_order_id,
            submission_blocked_reason: submission.blocked_reason,
        }
    }

    fn log_entry_evaluation(&self, now_ms: u64, submission: &EntrySubmissionDecision) {
        let fields = self.entry_evaluation_log_fields_at(now_ms, submission);
        let blocked = !fields.gate_blocked_by.is_empty() || !fields.pricing_blocked_by.is_empty();

        if blocked {
            log::warn!(
                "binary_oracle_edge_taker entry blocked: strategy_id={} reasons={:?}",
                self.config.strategy_id,
                fields.gate_blocked_by
            );
            if fields
                .gate_blocked_by
                .contains(&EntryBlockReason::FeesNotReady)
            {
                log::warn!(
                    "binary_oracle_edge_taker fee-rate unavailable: strategy_id={} entry remains fail-closed",
                    self.config.strategy_id
                );
            }
            log::warn!(
                "binary_oracle_edge_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_fee_bps={:?} down_fee_bps={:?} up_entry_cost={:?} down_entry_cost={:?} up_entry_limit_price={:?} down_entry_limit_price={:?} up_gross_cost_cents={:?} down_gross_cost_cents={:?} up_fee_cost_cents={:?} down_fee_cost_cents={:?} up_slippage_buffer_cents={:?} down_slippage_buffer_cents={:?} up_total_adjusted_cost_cents={:?} down_total_adjusted_cost_cents={:?} up_edge_cents_per_share={:?} down_edge_cents_per_share={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} sized_fee_bps={:?} sized_entry_cost={:?} sized_entry_limit_price={:?} sized_gross_cost_cents={:?} sized_fee_cost_cents={:?} sized_slippage_buffer_cents={:?} sized_total_adjusted_cost_cents={:?} sized_edge_cents_per_share={:?} sized_worst_case_ev_bps={:?} expected_ev_per_notional={:?} order_notional_target={} maximum_position_notional={} risk_lambda={} sizing_ev_reference_bps={} book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?} selected_side={:?} fast_venue_available={} reference_current_price_available={} reference_current_price_available_without_fast_venue={} lead_quality_policy_applied={} lead_quality_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                fields.up_fee_bps,
                fields.down_fee_bps,
                fields.up_entry_cost,
                fields.down_entry_cost,
                fields.up_entry_limit_price,
                fields.down_entry_limit_price,
                fields.up_gross_cost_cents,
                fields.down_gross_cost_cents,
                fields.up_fee_cost_cents,
                fields.down_fee_cost_cents,
                fields.up_slippage_buffer_cents,
                fields.down_slippage_buffer_cents,
                fields.up_total_adjusted_cost_cents,
                fields.down_total_adjusted_cost_cents,
                fields.up_edge_cents_per_share,
                fields.down_edge_cents_per_share,
                fields.up_worst_case_ev_bps,
                fields.down_worst_case_ev_bps,
                fields.sized_fee_bps,
                fields.sized_entry_cost,
                fields.sized_entry_limit_price,
                fields.sized_gross_cost_cents,
                fields.sized_fee_cost_cents,
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
                fields.final_fee_amount_known,
                fields.final_fee_amount_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity_value,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        } else {
            log::info!(
                "binary_oracle_edge_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_fee_bps={:?} down_fee_bps={:?} up_entry_cost={:?} down_entry_cost={:?} up_entry_limit_price={:?} down_entry_limit_price={:?} up_gross_cost_cents={:?} down_gross_cost_cents={:?} up_fee_cost_cents={:?} down_fee_cost_cents={:?} up_slippage_buffer_cents={:?} down_slippage_buffer_cents={:?} up_total_adjusted_cost_cents={:?} down_total_adjusted_cost_cents={:?} up_edge_cents_per_share={:?} down_edge_cents_per_share={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} sized_fee_bps={:?} sized_entry_cost={:?} sized_entry_limit_price={:?} sized_gross_cost_cents={:?} sized_fee_cost_cents={:?} sized_slippage_buffer_cents={:?} sized_total_adjusted_cost_cents={:?} sized_edge_cents_per_share={:?} sized_worst_case_ev_bps={:?} expected_ev_per_notional={:?} order_notional_target={} maximum_position_notional={} risk_lambda={} sizing_ev_reference_bps={} book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?} selected_side={:?} fast_venue_available={} reference_current_price_available={} reference_current_price_available_without_fast_venue={} lead_quality_policy_applied={} lead_quality_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                fields.up_fee_bps,
                fields.down_fee_bps,
                fields.up_entry_cost,
                fields.down_entry_cost,
                fields.up_entry_limit_price,
                fields.down_entry_limit_price,
                fields.up_gross_cost_cents,
                fields.down_gross_cost_cents,
                fields.up_fee_cost_cents,
                fields.down_fee_cost_cents,
                fields.up_slippage_buffer_cents,
                fields.down_slippage_buffer_cents,
                fields.up_total_adjusted_cost_cents,
                fields.down_total_adjusted_cost_cents,
                fields.up_edge_cents_per_share,
                fields.down_edge_cents_per_share,
                fields.up_worst_case_ev_bps,
                fields.down_worst_case_ev_bps,
                fields.sized_fee_bps,
                fields.sized_entry_cost,
                fields.sized_entry_limit_price,
                fields.sized_gross_cost_cents,
                fields.sized_fee_cost_cents,
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
                fields.final_fee_amount_known,
                fields.final_fee_amount_reason,
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
        let open_position = self.managed_position().map(|managed| &managed.position);
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

    fn record_entry_skip_once(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        reason_category: BoltV3EntrySkipReasonCategory,
        unclassified_context: Option<String>,
    ) -> Result<()> {
        let fields = self.entry_evaluation_log_fields_at(now_ms, decision);
        let forced_flat_inputs = self.entry_forced_flat_evidence_inputs();
        let key = EntrySkipDedupeKey {
            reason_category,
            gate_blocked_by: fields
                .gate_blocked_by
                .iter()
                .map(entry_block_reason_to_evidence)
                .collect(),
            pricing_blocked_by: fields
                .pricing_blocked_by
                .iter()
                .map(entry_pricing_block_reason_to_evidence)
                .collect(),
            market_id: fields.market_id.clone(),
            interval_open: option_evidence_number(fields.interval_open),
            fast_venue_available: fields.fast_venue_available,
            reference_current_price_available: fields.reference_current_price_available,
            fast_venue_incoherent: forced_flat_inputs.fast_venue_incoherent,
        };
        if self.last_recorded_entry_skip.as_ref() == Some(&key) {
            return Ok(());
        }
        let evidence = BoltV3EntrySkipEvidence::from_entry_skip(
            self.config.strategy_id.clone(),
            now_ms,
            reason_category,
            unclassified_context,
            &fields,
            forced_flat_inputs,
        );
        if let Err(error) = self
            .context
            .decision_evidence()
            .record_entry_skip(&evidence)
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
        self.last_recorded_entry_skip = Some(key);
        Ok(())
    }

    fn record_and_log_entry_skip(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        reason: &'static str,
    ) -> Result<()> {
        let reason_category = entry_skip_reason_category_from_str(reason)
            .unwrap_or(BoltV3EntrySkipReasonCategory::Unclassified);
        let unclassified_context = (reason_category == BoltV3EntrySkipReasonCategory::Unclassified)
            .then(|| reason.to_string());
        self.record_entry_skip_once(now_ms, decision, reason_category, unclassified_context)?;
        log::warn!(
            "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
            self.config.strategy_id,
            reason
        );
        Ok(())
    }

    fn record_exit_decision_once(
        &mut self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
        decision: &ExitSubmissionDecision,
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
        if decision.blocked_reason == Some(EXIT_BLOCK_REASON_NO_OPEN_POSITION)
            && decision.forced_flat_reasons.is_empty()
        {
            return Ok(());
        }

        let fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let evidence = BoltV3ExitDecisionEvidence::from_exit_decision(
            self.config.strategy_id.clone(),
            now_ms,
            &fields,
            self.exit_forced_flat_evidence_inputs(),
        );
        let key = ExitDecisionDedupeKey {
            market_id: evidence.market_id.clone(),
            position_id: evidence.position_id.clone(),
            forced_flat_reasons: evidence.forced_flat_reasons.clone(),
            exit_decision: evidence.exit_decision,
            blocked_reason: evidence.blocked_reason,
        };
        if self.last_recorded_exit_decision.as_ref() == Some(&key) {
            return Ok(());
        }
        if let Err(error) = self
            .context
            .decision_evidence()
            .record_exit_decision(&evidence)
        {
            // A telemetry-write failure must NEVER block a risk-reducing exit:
            // record_exit_decision_once is called immediately before the exit
            // order is built and submitted. Surface the lost write at the
            // highest non-panicking severity and let the exit proceed.
            log::error!(
                "binary_oracle_edge_taker exit decision evidence write failed: strategy_id={} error={error:#}",
                self.config.strategy_id
            );
        }
        self.last_recorded_exit_decision = Some(key);
        Ok(())
    }

    fn entry_fee_bps_at_price(&self, side: OutcomeSide, entry_price: f64) -> Option<f64> {
        let instrument_id = self.instrument_id_for_side(side)?;
        let instrument = self.current_instrument(instrument_id)?;
        let entry_price = Decimal::from_f64(entry_price)?;
        self.context
            .fee_provider()
            .entry_fee_bps(&instrument, entry_price)?
            .to_f64()
    }

    /// Resolve the max entry fee bound (in bps) used to compute the
    /// fee-inclusive admission notional from the configured fee provider.
    /// Fail-closed: a missing instrument context or absent fee bound is a hard error so the
    /// downstream cap check never silently passes a raw notional.
    ///
    /// SYMMETRIC-FEE ASSUMPTION (A12): both entry AND risk-reducing-exit
    /// admission scale their notional by THIS entry-fee bound. Polymarket
    /// charges the same fee on either leg, so the entry bound is the exact
    /// exit bound today. Should a venue ever charge a strictly larger exit fee,
    /// using the (smaller) entry bound here would UNDERSTATE an exit's
    /// fee-inclusive notional. That direction fails OPEN for the cap, so a
    /// venue with asymmetric (higher exit) fees MUST add an exit-fee bound and
    /// route exits through it before being admitted — do not silently reuse the
    /// entry bound for an asymmetric-fee venue.
    fn max_entry_fee_bps_for_admission(
        &self,
        instrument_id: InstrumentId,
        price: Decimal,
    ) -> Result<Decimal> {
        let instrument = self.current_instrument(instrument_id).with_context(|| {
            format!(
                "bolt-v3 submit admission requires cached instrument for instrument_id={instrument_id}"
            )
        })?;
        let max_fee_bps = self
            .context
            .fee_provider()
            .max_entry_fee_bps(&instrument, price)
        .with_context(|| {
            format!(
                "bolt-v3 submit admission requires a max entry fee bound for instrument_id={instrument_id}"
            )
        })?;
        anyhow::ensure!(
            max_fee_bps >= Decimal::ZERO,
            "bolt-v3 submit admission max entry fee bound must be non-negative for instrument_id={instrument_id}, got {max_fee_bps}"
        );
        Ok(max_fee_bps)
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
    ) -> Option<&'static str> {
        match self.entry_reject_state.get(&instrument_id)? {
            EntryRejectState::Malformed => Some(ENTRY_BLOCK_REASON_ENTRY_MALFORMED_REJECTED),
            EntryRejectState::Balance => Some(ENTRY_BLOCK_REASON_ENTRY_BALANCE_REJECTED),
            EntryRejectState::Unfillable { book } => {
                let current_book = self.active_book_for_outcome(selected_side);
                (current_book == book)
                    .then_some(ENTRY_BLOCK_REASON_ENTRY_UNFILLABLE_REJECTED_UNCHANGED_BOOK)
            }
        }
    }

    fn record_entry_reject(&mut self, event: &nautilus_model::events::OrderRejected) {
        let Some(pending) = self.pending_entry().cloned() else {
            return;
        };
        if pending.client_order_id != event.client_order_id
            || pending.instrument_id != event.instrument_id
        {
            return;
        }

        match classify_entry_reject_reason(event.reason.as_str()) {
            Some(EntryRejectClass::Malformed) => {
                self.entry_reject_state
                    .insert(event.instrument_id, EntryRejectState::Malformed);
            }
            Some(EntryRejectClass::Balance) => {
                self.entry_reject_state
                    .insert(event.instrument_id, EntryRejectState::Balance);
            }
            Some(EntryRejectClass::Unfillable) => {
                self.entry_reject_state.insert(
                    event.instrument_id,
                    EntryRejectState::Unfillable {
                        book: pending.book.clone(),
                    },
                );
            }
            None => {
                self.entry_reject_state.insert(
                    event.instrument_id,
                    EntryRejectState::Unfillable {
                        book: pending.book.clone(),
                    },
                );
            }
        }
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
        let fee_bps = self
            .entry_fee_bps_at_price(side, vwap.vwap_price)
            .filter(|value| is_non_negative_finite(*value))
            .ok_or(BinaryOutcomeEdgeBlockReason::FeeUnavailable)?;
        Ok(ExecutableEntryProbe {
            order_side,
            vwap,
            fee_bps,
        })
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
        let cost_breakdown = match executable_cost_breakdown(
            probe.vwap,
            probe.fee_bps,
            self.config.slippage_buffer_bps,
        ) {
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

    fn adjusted_probability_up_for_fee_uncertainty(
        &self,
        now_ms: u64,
        side: OutcomeSide,
        fair_probability_up: Probability,
        fee_uncertainty_bps: f64,
    ) -> Option<(Probability, Probability)> {
        let uncertainty_band_probability = self.current_uncertainty_band_probability_at(
            now_ms,
            fee_uncertainty_bps,
            fee_uncertainty_bps,
        )?;
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
        self.core.trader_id()?;
        let cache = self.cache();
        cache.instrument(&instrument_id).cloned()
    }

    fn normalize_base_order_quantity_for_execution_venue(
        &self,
        instrument: &InstrumentAny,
        quantity: Quantity,
    ) -> Option<Quantity> {
        let quantity_decimal = Decimal::from_f64(quantity.as_f64())?;
        let normalized = provider_normalize_base_order_quantity(
            self.context.execution_venue(),
            quantity_decimal,
        )?;
        instrument
            .try_make_qty(normalized.to_f64()?, Some(true))
            .ok()
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
        preserved: Option<&OpenPositionState>,
        pending_context: Option<&PendingEntryState>,
        spec: PositionMaterializationSpec,
        trust_pending_outcome_side: bool,
    ) -> OpenPositionState {
        OpenPositionState {
            market_id: preserved
                .and_then(|position| position.market_id.clone())
                .or_else(|| pending_context.and_then(|pending| pending.market_id.clone())),
            instrument_id: spec.instrument_id,
            position_id: spec.position_id,
            outcome_side: preserved
                .and_then(|position| position.outcome_side)
                .or_else(|| {
                    if trust_pending_outcome_side {
                        pending_context.and_then(|pending| pending.outcome_side)
                    } else {
                        None
                    }
                })
                .filter(|_| {
                    self.configured_position_contract()
                        .ok()
                        .is_some_and(|contract| {
                            supports_strategy_managed_position(
                                spec.entry_order_side,
                                spec.side,
                                contract,
                            )
                        })
                }),
            outcome_fees: preserved
                .map(|position| position.outcome_fees.clone())
                .or_else(|| pending_context.map(|pending| pending.outcome_fees.clone()))
                .unwrap_or_else(OutcomeFeeState::empty),
            historical_entry_fee_bps: preserved
                .and_then(|position| position.historical_entry_fee_bps)
                .or_else(|| pending_context.and_then(|pending| pending.historical_entry_fee_bps)),
            entry_order_side: spec.entry_order_side,
            side: spec.side,
            quantity: spec.quantity,
            avg_px_open: spec.avg_px_open,
            interval_open: preserved
                .and_then(|position| position.interval_open)
                .or_else(|| pending_context.and_then(|pending| pending.interval_open)),
            selection_published_at_ms: preserved
                .and_then(|position| position.selection_published_at_ms)
                .or_else(|| pending_context.and_then(|pending| pending.selection_published_at_ms)),
            seconds_to_expiry_at_selection: preserved
                .and_then(|position| position.seconds_to_expiry_at_selection)
                .or_else(|| {
                    pending_context.and_then(|pending| pending.seconds_to_expiry_at_selection)
                }),
            book: match (
                preserved.map(|position| position.book.clone()),
                pending_context.map(|pending| pending.book.clone()),
            ) {
                (Some(book), _) | (None, Some(book)) => book,
                (None, None) => OutcomeBookState::from_instrument_id(spec.instrument_id),
            },
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
        instrument_id: InstrumentId,
        position_id: PositionId,
        entry_order_side: OrderSide,
        side: PositionSide,
        quantity: Quantity,
        avg_px_open: f64,
    ) {
        // Venue invariant (defense in depth): a live position event must be on the
        // execution venue, or it would be adopted into Managed and the exit path
        // would submit against a foreign instrument_id. Quarantine before any
        // Managed/ExitPending transition via the shared venue-adoption guard.
        if self.quarantine_foreign_venue_event(instrument_id) {
            return;
        }
        let preserved = self
            .managed_position()
            .filter(|managed| {
                managed.position.position_id == position_id
                    && managed.position.instrument_id == instrument_id
            })
            .map(|managed| managed.position.clone());
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
            self.exposure = if let Some(pending) = pending_context {
                ExposureState::EntryReconcilePending {
                    pending,
                    reason: EntryReconcileReason::InvalidObservedPosition {
                        entry_order_side,
                        side,
                    },
                }
            } else {
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::InvalidLivePosition {
                        entry_order_side,
                        side: Some(side),
                    },
                })
            };
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
            .managed_position()
            .filter(|managed| {
                managed.position.position_id == position_id
                    && managed.position.instrument_id == instrument_id
            })
            .map(|managed| managed.origin)
        {
            Some(origin) => origin,
            None if pending_matches => ManagedPositionOrigin::StrategyEntry,
            None => ManagedPositionOrigin::RecoveryBootstrap,
        };
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
        let pending_entry = pending_context
            .clone()
            .filter(|pending| self.entry_order_may_remain_working(&pending.client_order_id));
        self.exposure = match self.exposure.exit_pending().cloned() {
            Some(exit_pending)
                if exit_pending.position.as_ref().is_some_and(|managed| {
                    managed.position.position_id == position_id
                        && managed.position.instrument_id == instrument_id
                }) =>
            {
                let mut pending_exit = exit_pending.pending_exit;
                if pending_exit.fill_received {
                    // NT position updates are produced from fills; once an exit fill is known,
                    // an open position event here is authoritative residual exposure.
                    pending_exit.residual_position_observed_after_fill = true;
                }
                ExitPendingState {
                    position: Some(ManagedPositionState {
                        position: materialized_position,
                        origin,
                        pending_entry,
                    }),
                    pending_exit,
                }
                .into_state_after_exit_update()
            }
            _ => ExposureState::Managed(ManagedPositionState {
                position: materialized_position,
                origin,
                pending_entry,
            }),
        };
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
    }

    fn mark_exit_order_terminal(
        &mut self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) {
        let Some(mut exit_pending) = self.exposure.exit_pending().cloned() else {
            return;
        };
        if exit_pending.pending_exit.client_order_id != client_order_id {
            return;
        }
        if !self.event_instrument_matches_held_exposure(event_instrument_id) {
            return;
        }
        exit_pending.pending_exit.terminal_received = true;
        self.exposure = exit_pending.into_state_after_exit_update();
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
    }

    fn seconds_to_expiry_from_selection(
        selection_published_at_ms: Option<u64>,
        seconds_to_expiry_at_selection: Option<u64>,
        now_ms: u64,
    ) -> Option<u64> {
        let published_at_ms = selection_published_at_ms?;
        let seconds_to_expiry_at_selection = seconds_to_expiry_at_selection?;
        let elapsed_seconds = now_ms.saturating_sub(published_at_ms) / MILLIS_PER_SECOND_U64;
        Some(seconds_to_expiry_at_selection.saturating_sub(elapsed_seconds))
    }

    fn sync_exposure_context_from_active(&mut self) {
        let active_market_id = self.active.market_id.clone();
        let active_outcome_fees = self.active.outcome_fees.clone();
        let active_interval_open = self.active.interval_open;
        let active_selection_published_at_ms = self.active.selection_published_at_ms;
        let active_seconds_to_expiry_at_selection = self.active.seconds_to_expiry_at_selection;
        let active_up_instrument_id = self.active.books.up.instrument_id;
        let active_down_instrument_id = self.active.books.down.instrument_id;
        let active_up_book = self.active.books.up.clone();
        let active_down_book = self.active.books.down.clone();
        let Some(open_position) = self.tracked_observed_position_mut() else {
            return;
        };

        if active_up_instrument_id == Some(open_position.instrument_id) {
            open_position.market_id = active_market_id.clone();
            open_position.outcome_side = Some(OutcomeSide::Up);
            open_position.outcome_fees = active_outcome_fees.clone();
            open_position.interval_open = active_interval_open;
            open_position.selection_published_at_ms = active_selection_published_at_ms;
            open_position.seconds_to_expiry_at_selection = active_seconds_to_expiry_at_selection;
            open_position.book = active_up_book;
        } else if active_down_instrument_id == Some(open_position.instrument_id) {
            open_position.market_id = active_market_id;
            open_position.outcome_side = Some(OutcomeSide::Down);
            open_position.outcome_fees = active_outcome_fees;
            open_position.interval_open = active_interval_open;
            open_position.selection_published_at_ms = active_selection_published_at_ms;
            open_position.seconds_to_expiry_at_selection = active_seconds_to_expiry_at_selection;
            open_position.book = active_down_book;
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
            .and_then(|position| position.position.outcome_side)
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

    fn current_position_seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        let open_position = &self.managed_position()?.position;
        Self::seconds_to_expiry_from_selection(
            open_position.selection_published_at_ms,
            open_position.seconds_to_expiry_at_selection,
            now_ms,
        )
    }

    fn current_position_fair_probability_up_for_gate_at(
        &self,
        now_ms: u64,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> Option<Probability> {
        let open_position = &self.managed_position()?.position;
        let spot_price = self.current_position_spot_price()?;
        let strike_price = open_position
            .interval_open
            .filter(|value| is_positive_finite(*value))?;
        let seconds_to_expiry = self.current_position_seconds_to_expiry_at(now_ms)?;
        let realized_vol = self.current_realized_vol_for_gate_at(realized_vol_gate_event_ms)?;
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
        self.current_position_fair_probability_up_for_gate_at(
            now_ms,
            Some(VenueEventMs::new(now_ms)),
        )
    }

    fn current_position_uncertainty_band_probability_for_gate_at(
        &self,
        now_ms: u64,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> Option<Probability> {
        let seconds_to_expiry = self.current_position_seconds_to_expiry_at(now_ms)?;
        let realized_vol = self.current_realized_vol_for_gate_at(realized_vol_gate_event_ms)?;
        let up_fee_bps = self.position_outcome_fee_bps(OutcomeSide::Up)?;
        let down_fee_bps = self.position_outcome_fee_bps(OutcomeSide::Down)?;
        self.uncertainty_band_probability_for_seconds(
            seconds_to_expiry,
            realized_vol,
            up_fee_bps,
            down_fee_bps,
        )
    }

    fn current_hold_ev_bps_for_gate_at(
        &self,
        now_ms: u64,
        side: OutcomeSide,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> Option<f64> {
        let fair_probability_up = self
            .current_position_fair_probability_up_for_gate_at(now_ms, realized_vol_gate_event_ms)?;
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        let fee_bps = self.open_position_historical_entry_fee_bps()?;
        let total_entry_cost = effective_entry_cost * (UNIT_F64 + fee_bps / BPS_DENOMINATOR);
        if !is_positive_finite(total_entry_cost) {
            return None;
        }
        let success_probability = match side {
            OutcomeSide::Up => fair_probability_up,
            OutcomeSide::Down => fair_probability_up.complement(),
        };

        Some(
            ((success_probability.value() - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR,
        )
    }

    #[cfg(test)]
    fn current_hold_ev_bps_at(&self, now_ms: u64, side: OutcomeSide) -> Option<f64> {
        self.current_hold_ev_bps_for_gate_at(now_ms, side, Some(VenueEventMs::new(now_ms)))
    }

    fn current_exit_ev_bps_at(
        &self,
        side: OutcomeSide,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<f64> {
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        let historical_entry_fee_bps = self.open_position_historical_entry_fee_bps()?;
        let current_exit_fee_bps = self.position_outcome_fee_bps(side)?;
        let total_entry_cost =
            effective_entry_cost * (UNIT_F64 + historical_entry_fee_bps / BPS_DENOMINATOR);
        if !is_positive_finite(total_entry_cost) {
            return None;
        }

        let current_exit_value =
            self.current_exit_value_for_open_position_with_config(order_config)?;
        let net_exit_value =
            current_exit_value * (UNIT_F64 - current_exit_fee_bps / BPS_DENOMINATOR);
        if !is_positive_finite(net_exit_value) {
            return None;
        }

        Some(((net_exit_value - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR)
    }

    fn open_position_historical_entry_fee_bps(&self) -> Option<f64> {
        self.managed_position()?.position.historical_entry_fee_bps
    }

    fn historical_entry_fee_log_fields(&self) -> (bool, &'static str) {
        let Some(managed_position) = self.managed_position() else {
            return (false, EVIDENCE_REASON_NO_MANAGED_POSITION);
        };

        if managed_position.position.historical_entry_fee_bps.is_some() {
            (true, EVIDENCE_REASON_CAPTURED_FROM_STRATEGY_ENTRY_STATE)
        } else if managed_position.origin == ManagedPositionOrigin::RecoveryBootstrap {
            (
                false,
                EVIDENCE_REASON_RECOVERY_BOOTSTRAP_POSITION_MISSING_ORIGINAL_FEE_RATE,
            )
        } else {
            (
                false,
                EVIDENCE_REASON_POSITION_STATE_MISSING_ORIGINAL_FEE_RATE,
            )
        }
    }

    fn position_outcome_fee_bps(&self, side: OutcomeSide) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let instrument_id = match side {
            OutcomeSide::Up => open_position.outcome_fees.up_instrument_id,
            OutcomeSide::Down => open_position.outcome_fees.down_instrument_id,
        }?;
        self.context.fee_provider().fee_bps(instrument_id)?.to_f64()
    }

    fn exit_evaluation_with_rv_gate_at(
        &self,
        now_ms: u64,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> ExitEvaluation {
        let mut evaluation = ExitEvaluation {
            position_outcome_side: self.open_position_outcome_side(),
            forced_flat_reasons: self.position_forced_flat_reasons_at(now_ms),
            hold_ev_bps: None,
            exit_ev_bps: None,
            exit_decision: None,
            blocked_reason: None,
        };

        if self.managed_position().is_none() {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_NO_OPEN_POSITION);
            return evaluation;
        }
        if self.exposure.exit_pending().is_some() {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING);
            return evaluation;
        }

        if !evaluation.forced_flat_reasons.is_empty() {
            evaluation.exit_decision = Some(ExitDecision::Exit);
            return evaluation;
        }

        if self
            .managed_position()
            .and_then(|managed| managed.pending_entry.as_ref())
            .is_some()
        {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING);
            return evaluation;
        }

        let Some(position_outcome_side) = evaluation.position_outcome_side else {
            evaluation.exit_decision = Some(ExitDecision::Hold);
            return evaluation;
        };

        let Ok(order_config) = self.normal_exit_order_execution_config() else {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID);
            return evaluation;
        };
        evaluation.hold_ev_bps = self.current_hold_ev_bps_for_gate_at(
            now_ms,
            position_outcome_side,
            realized_vol_gate_event_ms,
        );
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
        self.exit_evaluation_with_rv_gate_at(now_ms, Some(VenueEventMs::new(now_ms)))
    }

    fn exit_evaluation_for_trigger_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> ExitEvaluation {
        self.exit_evaluation_with_rv_gate_at(now_ms, trigger_context.venue_event_ms())
    }

    #[cfg(test)]
    fn exit_submission_decision_at(&self, now_ms: u64) -> ExitSubmissionDecision {
        let evaluation = self.exit_evaluation_at(now_ms);
        self.exit_submission_decision_from_evaluation(evaluation)
    }

    fn exit_submission_decision_for_trigger_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> ExitSubmissionDecision {
        let evaluation = self.exit_evaluation_for_trigger_at(now_ms, trigger_context);
        self.exit_submission_decision_from_evaluation(evaluation)
    }

    fn exit_submission_decision_from_evaluation(
        &self,
        evaluation: ExitEvaluation,
    ) -> ExitSubmissionDecision {
        let mut decision = ExitSubmissionDecision {
            evaluation: evaluation.clone(),
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
            blocked_reason: evaluation.blocked_reason,
            forced_flat_reasons: evaluation.forced_flat_reasons.clone(),
        };

        if evaluation.blocked_reason == Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING) {
            return decision;
        }

        let Some(exit_decision) = evaluation.exit_decision else {
            // No exit decision was produced: preserve the precise evaluation-level
            // block reason (e.g. ExitAlreadyPending, NoOpenPosition) so the recorded
            // decision trace names the real cause; only synthesize the generic
            // ExitDecisionUnavailable when the evaluation supplied no reason at all.
            decision.blocked_reason = evaluation
                .blocked_reason
                .or(Some(EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE));
            return decision;
        };
        if exit_decision == ExitDecision::Hold {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_HOLD);
            return decision;
        }

        let Some(open_position) = self.managed_position().map(|managed| &managed.position) else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_OPEN_POSITION_MISSING);
            return decision;
        };
        let Ok(order_config) =
            self.exit_order_execution_config(!evaluation.forced_flat_reasons.is_empty())
        else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID);
            return decision;
        };
        if order_config.order_template.is_quote_quantity {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUOTE_QUANTITY_UNSUPPORTED);
            return decision;
        }
        let Some((order_side, price)) =
            self.current_exit_order_for_open_position_with_config(&order_config)
        else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_PRICE_MISSING);
            return decision;
        };
        if !is_positive_finite(open_position.quantity.as_f64()) {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE);
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

    fn exit_realized_volatility_gate_fields_at(
        &self,
        realized_vol_gate_event_ms: Option<VenueEventMs>,
    ) -> (
        Option<u64>,
        bool,
        Vec<BoltV3ExitRvSnapshotBlocker>,
        Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
        BoltV3ExitRvGateResult,
        Option<u64>,
    ) {
        let Some(snapshot) = self
            .pricing
            .latest_realized_vol_snapshot_for_surface(&self.config.realized_volatility_surface_id)
        else {
            return (
                None,
                false,
                Vec::new(),
                Vec::new(),
                BoltV3ExitRvGateResult::MissingSnapshot,
                None,
            );
        };
        let gate_result = self.pricing.classify_realized_vol_gate(
            &self.config.realized_volatility_surface_id,
            realized_vol_gate_event_ms,
            self.realized_volatility_max_source_age_ms(),
        );
        let blockers = snapshot
            .blocked_reasons
            .iter()
            .copied()
            .map(realized_vol_blocker_to_exit_evidence)
            .collect();
        let diagnostics = snapshot
            .source_diagnostics
            .iter()
            .map(BoltV3RealizedVolatilitySourceDiagnosticEvidence::from_realized_vol_diagnostic)
            .collect();
        let future_dating_delta_ms = realized_vol_gate_event_ms.and_then(|event_ms| {
            (snapshot.as_of_ms > event_ms.value()).then_some(snapshot.as_of_ms - event_ms.value())
        });
        (
            Some(snapshot.as_of_ms),
            snapshot.ready,
            blockers,
            diagnostics,
            exit_rv_gate_result_from_shared(gate_result),
            future_dating_delta_ms,
        )
    }

    fn exit_evaluation_log_fields_at(
        &self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
        decision: &ExitSubmissionDecision,
    ) -> ExitEvaluationLogFields {
        let open_position = self.managed_position().map(|managed| &managed.position);
        let (historical_entry_fee_rate_known, historical_entry_fee_rate_reason) =
            self.historical_entry_fee_log_fields();
        let realized_vol_gate_event_ms = trigger_context.venue_event_ms();
        let (realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.current_realized_vol_source_for_gate_at(realized_vol_gate_event_ms);
        let (
            rv_snapshot_as_of_ms,
            rv_snapshot_ready,
            rv_snapshot_blockers,
            rv_source_diagnostics,
            rv_gate_result,
            rv_future_dating_delta_ms,
        ) = self.exit_realized_volatility_gate_fields_at(realized_vol_gate_event_ms);
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
            reference_current_price: self.pricing.last_reference_current_price(),
            interval_open: open_position.and_then(|position| position.interval_open),
            seconds_to_expiry: self.current_position_seconds_to_expiry_at(now_ms),
            realized_vol: self.current_realized_vol_for_gate_at(realized_vol_gate_event_ms),
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
            rv_surface_id: self.config.realized_volatility_surface_id.clone(),
            rv_snapshot_as_of_ms,
            rv_snapshot_ready,
            rv_snapshot_blockers,
            rv_source_diagnostics,
            rv_gate_result,
            rv_future_dating_delta_ms,
            exit_eval_now_ms: now_ms,
            exit_trigger_source: trigger_context.source,
            trigger_ts_event_ms: trigger_context.ts_event_ms,
            trigger_ts_init_ms: trigger_context.ts_init_ms,
            pricing_kurtosis: self.config.pricing_kurtosis,
            exit_hysteresis_bps: self.config.exit_hysteresis_bps,
            fair_probability_up: self
                .current_position_fair_probability_up_for_gate_at(
                    now_ms,
                    realized_vol_gate_event_ms,
                )
                .map(Probability::value),
            fair_probability_down: self
                .current_position_fair_probability_up_for_gate_at(
                    now_ms,
                    realized_vol_gate_event_ms,
                )
                .map(|value| value.complement().value()),
            uncertainty_band_probability: self
                .current_position_uncertainty_band_probability_for_gate_at(
                    now_ms,
                    realized_vol_gate_event_ms,
                )
                .map(Probability::value),
            up_fee_bps: self.position_outcome_fee_bps(OutcomeSide::Up),
            down_fee_bps: self.position_outcome_fee_bps(OutcomeSide::Down),
            hold_ev_bps: decision.evaluation.hold_ev_bps,
            exit_ev_bps: decision.evaluation.exit_ev_bps,
            exit_decision: decision.evaluation.exit_decision,
            historical_entry_fee_rate_known,
            historical_entry_fee_rate_reason,
            final_fee_amount_known: false,
            final_fee_amount_reason:
                EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_SIZE_AND_ACTUAL_FILL,
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
        decision: &ExitSubmissionDecision,
    ) {
        let fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let blocked = fields.submission_blocked_reason.is_some();
        if blocked {
            if should_warn_on_exit_submission_block(fields.submission_blocked_reason) {
                log::warn!(
                    "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                    fields.up_fee_bps,
                    fields.down_fee_bps,
                    fields.hold_ev_bps,
                    fields.exit_ev_bps,
                    fields.exit_decision,
                    fields.historical_entry_fee_rate_known,
                    fields.historical_entry_fee_rate_reason,
                    fields.final_fee_amount_known,
                    fields.final_fee_amount_reason,
                    fields.submission_instrument_id,
                    fields.submission_order_side,
                    fields.submission_price,
                    fields.submission_quantity,
                    fields.submission_client_order_id,
                    fields.submission_blocked_reason,
                );
            } else {
                log::debug!(
                    "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                    fields.up_fee_bps,
                    fields.down_fee_bps,
                    fields.hold_ev_bps,
                    fields.exit_ev_bps,
                    fields.exit_decision,
                    fields.historical_entry_fee_rate_known,
                    fields.historical_entry_fee_rate_reason,
                    fields.final_fee_amount_known,
                    fields.final_fee_amount_reason,
                    fields.submission_instrument_id,
                    fields.submission_order_side,
                    fields.submission_price,
                    fields.submission_quantity,
                    fields.submission_client_order_id,
                    fields.submission_blocked_reason,
                );
            }
        } else {
            log::info!(
                "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_current_price={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                fields.up_fee_bps,
                fields.down_fee_bps,
                fields.hold_ev_bps,
                fields.exit_ev_bps,
                fields.exit_decision,
                fields.historical_entry_fee_rate_known,
                fields.historical_entry_fee_rate_reason,
                fields.final_fee_amount_known,
                fields.final_fee_amount_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        }
    }

    fn submit_order_with_decision_evidence(
        &mut self,
        intent: BoltV3OrderIntentEvidence,
        order: nautilus_model::orders::OrderAny,
        submit_context: BoltV3SubmitContext,
    ) -> Result<BoltV3SubmitRoutingOutcome> {
        // A15: build the (fallible) admission request BEFORE recording the
        // order-intent evidence line. The request build can fail (e.g. a
        // market-style order whose instrument declares no structural price
        // ceiling, or an unresolvable fee bound), in which case the order never
        // fires —
        // recording the intent first would leave an orphan evidence line for an
        // order that was never submitted. Recording after the build keeps the
        // evidence chain truthful: an order-intent line exists only once the
        // order is fully valued and about to enter admission.
        let request = self.submit_admission_request_from_order(&intent, &order)?;
        let policy = self.context.order_execution_policy();
        let decision_evidence = self.context.decision_evidence_arc();
        let submit_admission = self.context.submit_admission_arc();
        let routing = BoltV3SubmitRoutingRequest::new(
            decision_evidence.as_ref(),
            submit_admission.as_ref(),
            intent,
            request,
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

    fn submit_admission_request_from_order(
        &self,
        intent: &BoltV3OrderIntentEvidence,
        order: &nautilus_model::orders::OrderAny,
    ) -> Result<BoltV3SubmitAdmissionRequest> {
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
        let quote_quantity_uses_submitted_notional = is_quote_quantity
            && matches!(order, OrderAny::Market(_))
            && order.order_side() == OrderSide::Buy;
        let quote_quantity_last_price =
            if is_quote_quantity && !quote_quantity_uses_submitted_notional {
                self.quote_quantity_last_price_for_order(order)
            } else {
                None
            };
        let quote_quantity_reference_price =
            if is_quote_quantity && !quote_quantity_uses_submitted_notional {
                self.quote_quantity_reference_price_for_order(order)
            } else {
                None
            };
        let risk_reducing_exit_position_context = if matches!(
            intent.intent_kind,
            BoltV3OrderIntentKind::Exit
        ) {
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
                managed_position.position.position_id.as_str(),
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
                    position_id,
                    instrument_id: instrument_id.as_str(),
                    position_side: *position_side,
                    position_quantity: *position_quantity,
                }
            },
        );

        build_submit_admission_request_from_order(
            BoltV3SubmitAdmissionRequestInput {
                execution_client_id: &self.config.client_id,
                intent,
                order,
                instrument: instrument.as_ref(),
                quote_quantity_last_price,
                quote_quantity_reference_price,
                lifecycle_policy: self.submit_lifecycle_policy(),
                risk_reducing_exit_position,
            },
            |price| self.max_entry_fee_bps_for_admission(order.instrument_id(), price),
        )
    }

    fn quote_quantity_last_price_for_order(
        &self,
        order: &nautilus_model::orders::OrderAny,
    ) -> Option<Price> {
        match order {
            OrderAny::Market(_) | OrderAny::MarketToLimit(_) => {
                self.market_order_cache_price_for_order(order)
            }
            OrderAny::StopMarket(_) | OrderAny::MarketIfTouched(_) => order.trigger_price(),
            OrderAny::TrailingStopMarket(_) | OrderAny::TrailingStopLimit(_) => {
                order.trigger_price()
            }
            _ => order.price(),
        }
    }

    fn market_order_cache_price_for_order(
        &self,
        order: &nautilus_model::orders::OrderAny,
    ) -> Option<Price> {
        let cache = self.cache();
        if let Some(last_quote) = cache.quote(&order.instrument_id()) {
            return match order.order_side() {
                OrderSide::Buy => Some(last_quote.ask_price),
                OrderSide::Sell => Some(last_quote.bid_price),
                _ => None,
            };
        }
        cache
            .trade(&order.instrument_id())
            .map(|last_trade| last_trade.price)
    }

    /// Side-appropriate top-of-book price (best ask for a BUY, best bid for a
    /// SELL) used by [`admission_base_notional_from_order`] to pick a
    /// conservative effective price for a quote-quantity order's quote→base
    /// conversion. `None` when no current quote tick is cached, which the shared
    /// helper treats as "no top-of-book" and falls back to the last price.
    fn quote_quantity_reference_price_for_order(
        &self,
        order: &nautilus_model::orders::OrderAny,
    ) -> Option<Price> {
        let cache = self.cache();
        let quote_tick = cache.quote(&order.instrument_id())?;
        match order.order_side() {
            OrderSide::Buy => Some(quote_tick.ask_price),
            OrderSide::Sell => Some(quote_tick.bid_price),
            _ => None,
        }
    }

    fn realized_volatility_evidence_fields(&self) -> RealizedVolatilityEvidenceFields {
        let realized_volatility_snapshot = self
            .pricing
            .latest_realized_vol_snapshot_for_surface(&self.config.realized_volatility_surface_id);
        match realized_volatility_snapshot {
            Some(snapshot) => RealizedVolatilityEvidenceFields {
                surface_id: snapshot.surface_id.clone(),
                as_of_ms: Some(snapshot.as_of_ms),
                annualized_decimal: snapshot
                    .annualized_realized_vol_decimal
                    .map_or_else(String::new, evidence_number),
                measured_annualized_decimal: snapshot
                    .measured_annualized_realized_vol_decimal
                    .map_or_else(String::new, evidence_number),
                noise_robust_annualized_decimal: snapshot
                    .noise_robust_annualized_realized_vol_decimal
                    .map_or_else(String::new, evidence_number),
                continuous_annualized_decimal: snapshot
                    .continuous_annualized_realized_vol_decimal
                    .map_or_else(String::new, evidence_number),
                jump_annualized_decimal: snapshot
                    .jump_annualized_realized_vol_decimal
                    .map_or_else(String::new, evidence_number),
                forecast_annualized_decimal: snapshot
                    .forecast_annualized_realized_vol_decimal
                    .map_or_else(String::new, evidence_number),
                pricing_component: realized_volatility_pricing_component_evidence_label(
                    snapshot.pricing_component,
                )
                .to_string(),
                seconds_per_annum: evidence_number(snapshot.seconds_per_annum),
                aggregation: realized_volatility_aggregation_evidence_label(snapshot.aggregate_method)
                    .to_string(),
                sources_used: snapshot.sources_used.clone(),
                source_diagnostics: snapshot
                    .source_diagnostics
                    .iter()
                    .map(
                        BoltV3RealizedVolatilitySourceDiagnosticEvidence::from_realized_vol_diagnostic,
                    )
                    .collect(),
                unknown_source_rejections: snapshot.unknown_source_rejections.clone(),
                blockers: snapshot
                    .blocked_reasons
                    .iter()
                    .map(|reason| {
                        realized_volatility_block_reason_evidence_label(*reason).to_string()
                    })
                    .collect(),
                config_fingerprint: snapshot.config_fingerprint.clone(),
            },
            None => RealizedVolatilityEvidenceFields {
                surface_id: String::new(),
                as_of_ms: None,
                annualized_decimal: String::new(),
                measured_annualized_decimal: String::new(),
                noise_robust_annualized_decimal: String::new(),
                continuous_annualized_decimal: String::new(),
                jump_annualized_decimal: String::new(),
                forecast_annualized_decimal: String::new(),
                pricing_component: String::new(),
                seconds_per_annum: String::new(),
                aggregation: String::new(),
                sources_used: Vec::new(),
                source_diagnostics: Vec::new(),
                unknown_source_rejections: BTreeMap::new(),
                blockers: Vec::new(),
                config_fingerprint: String::new(),
            },
        }
    }

    fn blocked_entry_strategy_input_evidence_snapshot_at(
        &self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
    ) -> Result<BoltV3StrategyInputEvidenceSnapshot> {
        let realized_volatility = self.realized_volatility_evidence_fields();
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

        Ok(BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: self.config.strategy_id.clone(),
            configured_target_id: self.config.configured_target_id.clone(),
            market_selection_ruleset_id: self.config.configured_target_id.clone(),
            market_selection_outcome: market_selection_outcome.to_string(),
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
                .map_or_else(String::new, evidence_number),
            reference_quote_ts_event,
            spot_price: self
                .evidence_spot_price()
                .map_or_else(String::new, evidence_number),
            fast_venue_available,
            reference_current_price: reference_current_price.map(evidence_number),
            reference_current_price_available,
            reference_current_price_source_id: self.evidence_reference_current_price_source_id(),
            reference_current_price_failed_over: self
                .evidence_reference_current_price_failed_over(),
            realized_volatility: String::new(),
            realized_volatility_surface_id: realized_volatility.surface_id,
            realized_volatility_as_of_ms: realized_volatility.as_of_ms,
            realized_volatility_annualized_decimal: realized_volatility.annualized_decimal,
            realized_volatility_measured_annualized_decimal: realized_volatility
                .measured_annualized_decimal,
            realized_volatility_noise_robust_annualized_decimal: realized_volatility
                .noise_robust_annualized_decimal,
            realized_volatility_continuous_annualized_decimal: realized_volatility
                .continuous_annualized_decimal,
            realized_volatility_jump_annualized_decimal: realized_volatility
                .jump_annualized_decimal,
            realized_volatility_forecast_annualized_decimal: realized_volatility
                .forecast_annualized_decimal,
            realized_volatility_pricing_component: realized_volatility.pricing_component,
            realized_volatility_seconds_per_annum: realized_volatility.seconds_per_annum,
            realized_volatility_aggregation: realized_volatility.aggregation,
            realized_volatility_sources_used: realized_volatility.sources_used,
            realized_volatility_source_diagnostics: realized_volatility.source_diagnostics,
            realized_volatility_unknown_source_rejections: realized_volatility
                .unknown_source_rejections,
            realized_volatility_blockers: realized_volatility.blockers,
            realized_volatility_config_fingerprint: realized_volatility.config_fingerprint,
            seconds_to_market_end,
            pricing_kurtosis: evidence_number(self.config.pricing_kurtosis),
            theta_decay_factor: evidence_number(self.config.theta_decay_factor),
            theta_scaled_min_edge_bps: decision
                .evaluation
                .min_worst_case_ev_bps
                .filter(|value| value.is_finite())
                .map_or_else(String::new, evidence_number),
            fair_probability_up: decision
                .evaluation
                .fair_probability_up
                .map_or_else(String::new, probability_evidence),
            uncertainty_band_probability: decision
                .evaluation
                .uncertainty_band_probability
                .map_or_else(String::new, probability_evidence),
            expected_edge_basis_points: expected_edge_basis_points
                .filter(|value| value.is_finite())
                .map_or_else(String::new, evidence_number),
            worst_case_edge_basis_points: worst_case_edge_basis_points
                .filter(|value| value.is_finite())
                .map_or_else(String::new, evidence_number),
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
            lead_agreement_corr: option_evidence_probability(self.pricing.last_lead_agreement_corr),
            fee_rate_basis_points: String::new(),
            selected_side: decision
                .evaluation
                .selected_side
                .map(outcome_side_evidence_label)
                .map(str::to_string),
            submission_instrument_id: String::new(),
            submission_order_side: String::new(),
            submission_price: String::new(),
            submission_quantity: String::new(),
            client_order_id: String::new(),
        })
    }

    fn entry_strategy_input_evidence_snapshot_at(
        &self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        client_order_id: ClientOrderId,
        price: &Price,
        quantity: &Quantity,
    ) -> Result<BoltV3StrategyInputEvidenceSnapshot> {
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
        let realized_volatility = self
            .current_realized_vol_for_gate_at(self.current_pricing_venue_event_ms())
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires realized volatility")
            })?;
        let seconds_to_market_end = self.current_seconds_to_expiry_at(now_ms).ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires seconds to market end")
        })?;
        let selected_side = decision.evaluation.selected_side.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires selected side")
        })?;
        let fee_rate_basis_points = self
            .entry_fee_bps_at_price(selected_side, price.as_f64())
            .filter(|value| is_non_negative_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires selected outcome fee")
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
        let realized_volatility_fields = self.realized_volatility_evidence_fields();
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
        Ok(BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: self.config.strategy_id.clone(),
            configured_target_id: self.config.configured_target_id.clone(),
            market_selection_ruleset_id: self.config.configured_target_id.clone(),
            market_selection_outcome: market_selection_outcome.to_string(),
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
            reference_current_price_source_id: self.evidence_reference_current_price_source_id(),
            reference_current_price_failed_over: self
                .evidence_reference_current_price_failed_over(),
            realized_volatility: evidence_number(realized_volatility),
            realized_volatility_surface_id: realized_volatility_fields.surface_id,
            realized_volatility_as_of_ms: realized_volatility_fields.as_of_ms,
            realized_volatility_annualized_decimal: realized_volatility_fields.annualized_decimal,
            realized_volatility_measured_annualized_decimal: realized_volatility_fields
                .measured_annualized_decimal,
            realized_volatility_noise_robust_annualized_decimal: realized_volatility_fields
                .noise_robust_annualized_decimal,
            realized_volatility_continuous_annualized_decimal: realized_volatility_fields
                .continuous_annualized_decimal,
            realized_volatility_jump_annualized_decimal: realized_volatility_fields
                .jump_annualized_decimal,
            realized_volatility_forecast_annualized_decimal: realized_volatility_fields
                .forecast_annualized_decimal,
            realized_volatility_pricing_component: realized_volatility_fields.pricing_component,
            realized_volatility_seconds_per_annum: realized_volatility_fields.seconds_per_annum,
            realized_volatility_aggregation: realized_volatility_fields.aggregation,
            realized_volatility_sources_used: realized_volatility_fields.sources_used,
            realized_volatility_source_diagnostics: realized_volatility_fields.source_diagnostics,
            realized_volatility_unknown_source_rejections: realized_volatility_fields
                .unknown_source_rejections,
            realized_volatility_blockers: realized_volatility_fields.blockers,
            realized_volatility_config_fingerprint: realized_volatility_fields.config_fingerprint,
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
            lead_agreement_corr: option_evidence_probability(self.pricing.last_lead_agreement_corr),
            fee_rate_basis_points: evidence_number(fee_rate_basis_points),
            selected_side: Some(outcome_side_evidence_label(selected_side).to_string()),
            submission_instrument_id: instrument_id.to_string(),
            submission_order_side: order_side.to_string(),
            submission_price: price.to_string(),
            submission_quantity: quantity.to_string(),
            client_order_id: client_order_id.to_string(),
        })
    }

    fn submit_lifecycle_policy(&self) -> BoltV3SubmitLifecyclePolicy {
        BoltV3SubmitLifecyclePolicy::new(
            self.config.manage_contingent_orders
                || self.config.manage_gtd_expiry
                || self.config.manage_stop,
        )
    }

    #[cfg(test)]
    fn try_submit_exit_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        self.try_submit_exit_order_for_trigger(
            now_ms,
            ExitEvaluationTriggerContext::unknown(now_ms),
        )
    }

    /// Evaluate and (if admitted) submit an exit order, then record durable #885
    /// exit-evaluation evidence flood-gated by [`ExitOutcomeKey`].
    ///
    /// `trigger_context` supplies the event timestamp domain for realized-volatility
    /// consumption. Local/unknown triggers hold unless a real venue event can price
    /// the RV freshness decision.
    fn try_submit_exit_order_for_trigger(
        &mut self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> Result<Option<ClientOrderId>> {
        let (result, decision) = self.try_submit_exit_order_inner(now_ms, trigger_context)?;
        self.record_exit_evaluation_evidence(now_ms, &decision, trigger_context, result.is_some());
        Ok(result)
    }

    fn try_submit_exit_order_inner(
        &mut self,
        now_ms: u64,
        trigger_context: ExitEvaluationTriggerContext,
    ) -> Result<(Option<ClientOrderId>, ExitSubmissionDecision)> {
        self.refresh_realized_volatility_snapshot_at(now_ms);
        self.refresh_current_reference_price_selection_at(now_ms);
        let mut decision = self.exit_submission_decision_for_trigger_at(now_ms, trigger_context);

        let Some(instrument_id) = decision.instrument_id else {
            self.record_exit_decision_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            return Ok((None, decision));
        };
        let Some(order_side) = decision.order_side else {
            self.record_exit_decision_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            return Ok((None, decision));
        };
        let Some(raw_price) = decision.price else {
            self.record_exit_decision_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            return Ok((None, decision));
        };
        let Some(mut quantity) = decision.quantity else {
            self.record_exit_decision_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            return Ok((None, decision));
        };
        let order_config = decision
            .execution_config()
            .ok_or_else(|| anyhow::anyhow!("exit submission decision missing order config"))?;
        let instrument = self
            .current_instrument(instrument_id)
            .ok_or_else(|| anyhow::anyhow!("exit instrument missing from cache"))?;
        let Some(normalized_quantity) =
            self.normalize_base_order_quantity_for_execution_venue(&instrument, quantity)
        else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE);
            self.record_exit_decision_once(now_ms, trigger_context, &decision)?;
            self.log_exit_evaluation(now_ms, trigger_context, &decision);
            return Ok((None, decision));
        };
        quantity = normalized_quantity;
        decision.quantity = Some(quantity);
        let price = Price::new(raw_price, instrument.price_precision());
        let client_order_id = self.core.order_factory().generate_client_order_id();
        decision.client_order_id = Some(client_order_id);
        self.record_exit_decision_once(now_ms, trigger_context, &decision)?;
        self.log_exit_evaluation(now_ms, trigger_context, &decision);
        let order = self.build_exit_order_with_execution_config(
            order_config,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )?;

        let client_id = ClientId::from(self.config.client_id.as_str());
        let Some(managed_position) = self.managed_position().cloned() else {
            anyhow::bail!("exit submit requires managed position state");
        };
        if !decision.forced_flat_reasons.is_empty()
            && let Some(pending_entry) = managed_position.pending_entry.as_ref()
        {
            self.cancel_resting_order(pending_entry.client_order_id, client_id)
                .with_context(|| {
                    format!(
                        "forced-flat exit could not cancel pending entry client_order_id={}",
                        pending_entry.client_order_id
                    )
                })?;
        }
        self.exposure = ExposureState::ExitPending(ExitPendingState {
            position: Some(managed_position.clone()),
            pending_exit: PendingExitState {
                client_order_id,
                market_id: managed_position.position.market_id.clone(),
                position_id: Some(managed_position.position.position_id),
                fill_received: false,
                close_received: false,
                terminal_received: false,
                residual_position_observed_after_fill: false,
            },
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

        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.config.strategy_id.clone(),
            BoltV3OrderIntentKind::Exit,
            price.to_string(),
            &order,
        );

        match self.submit_order_with_decision_evidence(
            intent,
            order,
            BoltV3SubmitContext::with_client_id_and_position_id(
                client_id,
                managed_position.position.position_id,
            ),
        ) {
            Ok(BoltV3SubmitRoutingOutcome::Submitted) => {}
            Ok(BoltV3SubmitRoutingOutcome::SkippedByPolicy) => {}
            Err(error) => {
                self.exposure = ExposureState::Managed(managed_position);
                return Err(error);
            }
        }

        Ok((Some(client_order_id), decision))
    }

    /// Assemble the #885 exit-evaluation evidence from the same single-source field
    /// builders that drive the existing exit log (`exit_evaluation_log_fields_at` +
    /// `realized_volatility_evidence_fields` + the shared RV-gate classifier), then
    /// emit it flood-gated by [`ExitOutcomeKey`]. Observability only: this never
    /// changes the trading decision (the decision was already computed in
    /// `try_submit_exit_order_inner`).
    fn record_exit_evaluation_evidence(
        &mut self,
        now_ms: u64,
        decision: &ExitSubmissionDecision,
        trigger_context: ExitEvaluationTriggerContext,
        submitted: bool,
    ) {
        let trigger_source = trigger_context.source;
        let trigger_ts_event_ms = Some(trigger_context.ts_event_ms as i64);
        let trigger_ts_init_ms = trigger_context.ts_init_ms.map(|value| value as i64);
        let rv_gate_event_ms = trigger_context.venue_event_ms();
        let log_fields = self.exit_evaluation_log_fields_at(now_ms, trigger_context, decision);
        let rv_fields = self.realized_volatility_evidence_fields();
        let rv_gate_result = self.pricing.classify_realized_vol_gate(
            &self.config.realized_volatility_surface_id,
            rv_gate_event_ms,
            self.realized_volatility_max_source_age_ms(),
        );
        let exit_eval_now_ms = now_ms as i64;
        let rv_as_of_ms = rv_fields.as_of_ms.map(|value| value as i64);
        let rv_as_of_minus_now_ms = rv_as_of_ms
            .zip(rv_gate_event_ms)
            .map(|(as_of, event_ms)| as_of - event_ms.value() as i64);
        let rv_ready = self
            .pricing
            .latest_realized_vol_snapshot_for_surface(&self.config.realized_volatility_surface_id)
            .is_some_and(|snapshot| snapshot.ready_realized_vol().is_some());

        let exit_decision = exit_decision_evidence_from_optional(decision.evaluation.exit_decision);
        let outcome_key = ExitOutcomeKey {
            exit_decision,
            submission_blocked_reason: decision.blocked_reason,
            rv_gate_result,
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
            if !submitted && !changed {
                return;
            }
        }

        let evidence = BoltV3ExitEvaluationEvidence {
            position_id: log_fields.position_id.map(|id| id.to_string()),
            market_id: log_fields.market_id.clone(),
            instrument_id: log_fields.position_instrument_id.map(|id| id.to_string()),
            client_order_id: decision.client_order_id.map(|id| id.to_string()),
            exit_eval_now_ms,
            exit_trigger_source: trigger_source,
            trigger_ts_event_ms,
            trigger_ts_init_ms,
            rv_surface_id: rv_fields.surface_id.clone(),
            rv_as_of_ms,
            rv_ready,
            rv_blockers: rv_fields.blockers.clone(),
            rv_source_diagnostics: rv_fields
                .source_diagnostics
                .iter()
                .map(|diagnostic| format!("{}:{}", diagnostic.source_id, diagnostic.status))
                .collect(),
            rv_gate_result,
            rv_as_of_minus_now_ms,
            hold_ev_bps: log_fields.hold_ev_bps.map(evidence_number),
            exit_ev_bps: log_fields.exit_ev_bps.map(evidence_number),
            exit_decision,
            forced_flat_reasons: log_fields
                .forced_flat_reasons
                .iter()
                .map(|reason| format!("{reason:?}"))
                .collect(),
            submission_order_side: log_fields
                .submission_order_side
                .map(|side| side.to_string()),
            submission_price: log_fields.submission_price.map(evidence_number),
            submission_quantity: log_fields
                .submission_quantity
                .map(|quantity| quantity.to_string()),
            submission_blocked_reason: log_fields.submission_blocked_reason.map(str::to_string),
        };

        if let Err(error) = self
            .context
            .decision_evidence()
            .record_exit_evaluation(&evidence)
        {
            log::error!(
                "binary_oracle_edge_taker exit evidence write failed: strategy_id={} position_id={:?} error={:#}",
                self.config.strategy_id,
                evidence.position_id,
                error,
            );
        }
    }

    fn entry_submission_decision_at(&self, now_ms: u64) -> EntrySubmissionDecision {
        let evaluation = self.entry_evaluation_at(now_ms);
        let mut decision = EntrySubmissionDecision {
            evaluation: evaluation.clone(),
            instrument_id: self.active.instrument_id,
            order_side: None,
            price: None,
            quantity_value: None,
            client_order_id: None,
            blocked_reason: None,
        };

        if self.core.trader_id().is_none() {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED);
            return decision;
        }

        if !evaluation.gate.blocked_by.is_empty() {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_GATE_BLOCKED);
            return decision;
        }
        if !evaluation.pricing_blocked_by.is_empty() {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED);
            return decision;
        }

        let Some(selected_side) = evaluation.selected_side else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_NO_SIDE_SELECTED);
            return decision;
        };
        let Some(sized_notional) = evaluation
            .sized_notional
            .filter(|value| is_positive_finite(*value))
        else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_SIZED_NOTIONAL_NOT_POSITIVE);
            return decision;
        };

        let Some(instrument_id) = self.instrument_id_for_side(selected_side) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_INSTRUMENT_ID_MISSING);
            return decision;
        };
        let Some(instrument) = self.current_instrument(instrument_id) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_INSTRUMENT_MISSING_FROM_CACHE);
            return decision;
        };
        if let Some(reason) = self.entry_reject_block_reason_for(instrument_id, selected_side) {
            decision.blocked_reason = Some(reason);
            return decision;
        }
        let Some(submission_vwap) =
            executable_submission_vwap_from_evaluation(&evaluation, selected_side)
        else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING);
            return decision;
        };
        let price = submission_vwap.limit_price;
        let quantity_value = if self.config.entry_order.is_quote_quantity {
            let Some(min_notional) =
                market_quote_buy_min_notional_for_execution_venue(self.context.execution_venue())
            else {
                decision.blocked_reason =
                    Some(ENTRY_BLOCK_REASON_ENTRY_QUOTE_NOTIONAL_MINIMUM_UNMODELED);
                return decision;
            };
            let Some(sized_notional_decimal) = Decimal::from_f64(sized_notional) else {
                decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED);
                return decision;
            };
            if sized_notional_decimal < min_notional {
                decision.blocked_reason =
                    Some(ENTRY_BLOCK_REASON_ENTRY_QUOTE_NOTIONAL_BELOW_VENUE_MINIMUM);
                return decision;
            }
            sized_notional
        } else {
            let max_quantity_at_limit = sized_notional / price;
            if !is_positive_finite(max_quantity_at_limit) {
                decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING);
                return decision;
            }
            let shares_value = submission_vwap.vwap_quantity.min(max_quantity_at_limit);
            let Ok(quantity) = instrument.try_make_qty(shares_value, Some(true)) else {
                decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED);
                return decision;
            };
            let Some(quantity) =
                self.normalize_base_order_quantity_for_execution_venue(&instrument, quantity)
            else {
                decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED);
                return decision;
            };
            let quantity_value = quantity.as_f64();
            let limit_notional = price * quantity_value;
            if limit_notional_exceeds_sized_notional(limit_notional, sized_notional) {
                decision.blocked_reason =
                    Some(ENTRY_BLOCK_REASON_LIMIT_NOTIONAL_EXCEEDS_SIZED_NOTIONAL);
                return decision;
            }
            quantity_value
        };
        if !is_positive_finite(quantity_value) {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_NOT_POSITIVE);
            return decision;
        }

        let Ok(contract) = self.configured_position_contract() else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_POSITION_CONTRACT_INVALID);
            return decision;
        };
        let order_side = contract.entry_order_side;
        let position_side = contract.entry_position_side;
        if !supports_strategy_managed_position(order_side, position_side, contract) {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_POSITION_CONTRACT_UNSUPPORTED);
            return decision;
        }

        decision.instrument_id = Some(instrument_id);
        decision.order_side = Some(order_side);
        decision.price = Some(price);
        decision.quantity_value = Some(quantity_value);
        decision
    }

    fn try_submit_entry_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        self.refresh_realized_volatility_snapshot_at(now_ms);
        let decision = self.entry_submission_decision_at(now_ms);
        self.log_entry_evaluation(now_ms, &decision);

        if decision.blocked_reason == Some(ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED)
            && decision
                .evaluation
                .pricing_blocked_by
                .contains(&EntryPricingBlockReason::RealizedVolNotReady)
            && self
                .pricing
                .latest_realized_vol_snapshot_for_surface(
                    &self.config.realized_volatility_surface_id,
                )
                .is_some()
        {
            let strategy_input_snapshot =
                self.blocked_entry_strategy_input_evidence_snapshot_at(now_ms, &decision)?;
            self.context
                .decision_evidence()
                .record_strategy_input_snapshot(&strategy_input_snapshot)?;
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
        let Some(historical_entry_fee_bps) = decision
            .evaluation
            .selected_side
            .and_then(|selected_side| self.entry_fee_bps_at_price(selected_side, price))
        else {
            self.record_and_log_entry_skip(
                now_ms,
                &decision,
                ENTRY_BLOCK_REASON_HISTORICAL_ENTRY_FEE_UNAVAILABLE,
            )?;
            return Ok(None);
        };
        let instrument = self
            .current_instrument(instrument_id)
            .ok_or_else(|| anyhow::anyhow!("entry instrument missing from cache"))?;
        let quantity = instrument.try_make_qty(quantity_value, Some(true))?;

        if self.exposure_occupancy().is_some() {
            self.record_entry_skip_once(
                now_ms,
                &decision,
                BoltV3EntrySkipReasonCategory::OnePositionInvariantViolation,
                None,
            )?;
            if let Err(error) = self.enforce_one_position_invariant() {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={} error={error:#}",
                    self.config.strategy_id,
                    ENTRY_BLOCK_REASON_ONE_POSITION_INVARIANT_VIOLATION
                );
            } else {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    ENTRY_BLOCK_REASON_ONE_POSITION_INVARIANT_VIOLATION
                );
            }
            return Ok(None);
        }

        self.entry_reject_state.remove(&instrument_id);
        self.last_recorded_entry_skip = None;
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

        let client_id = ClientId::from(self.config.client_id.as_str());
        self.exposure = ExposureState::PendingEntry(PendingEntryState {
            client_order_id,
            market_id: self.current_market_id().map(str::to_string),
            instrument_id,
            outcome_side: decision.evaluation.selected_side,
            outcome_fees: self.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(historical_entry_fee_bps),
            interval_open: self.active.interval_open,
            selection_published_at_ms: self.active.selection_published_at_ms,
            seconds_to_expiry_at_selection: self.active.seconds_to_expiry_at_selection,
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

        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.config.strategy_id.clone(),
            BoltV3OrderIntentKind::Entry,
            price.to_string(),
            &order,
        );

        match self
            .context
            .decision_evidence()
            .record_strategy_input_snapshot(&strategy_input_snapshot)
            .and_then(|()| {
                self.submit_order_with_decision_evidence(
                    intent,
                    order,
                    BoltV3SubmitContext::with_client_id(client_id),
                )
            }) {
            Ok(BoltV3SubmitRoutingOutcome::Submitted) => {}
            Ok(BoltV3SubmitRoutingOutcome::SkippedByPolicy) => {
                self.clear_pending_entry_state();
            }
            Err(error) => {
                self.clear_pending_entry_state();
                return Err(error);
            }
        }

        Ok(Some(client_order_id))
    }

    fn entry_evaluation_at(&self, now_ms: u64) -> EntryEvaluation {
        let gate = self.entry_gate_decision_at(now_ms);
        let mut evaluation = EntryEvaluation {
            gate,
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

        let pricing_inputs = match self.current_entry_pricing_inputs_at(now_ms) {
            Ok(inputs) => inputs,
            Err(blocked_by) => {
                evaluation.pricing_blocked_by = blocked_by;
                return evaluation;
            }
        };
        evaluation.min_worst_case_ev_bps = Some(pricing_inputs.theta_scaled_min_edge_bps);

        let fair_probability_up = match self.current_fair_probability_up_at(now_ms) {
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

        let fee_uncertainty_bps = match (up_probe.as_ref().ok(), down_probe.as_ref().ok()) {
            (Some(up), Some(down)) => Some(up.fee_bps.max(down.fee_bps)),
            (Some(up), None) => Some(up.fee_bps),
            (None, Some(down)) => Some(down.fee_bps),
            (None, None) => None,
        };
        let Some(fee_uncertainty_bps) = fee_uncertainty_bps else {
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Up,
                up_probe.err(),
            );
            push_executable_edge_pricing_block(
                &mut evaluation.pricing_blocked_by,
                OutcomeSide::Down,
                down_probe.err(),
            );
            return evaluation;
        };
        let uncertainty_band_probability = match self.current_uncertainty_band_probability_at(
            now_ms,
            fee_uncertainty_bps,
            fee_uncertainty_bps,
        ) {
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
            let selected_worst_case_ev_bps = match selected_side {
                OutcomeSide::Up => evaluation.up_worst_case_ev_bps,
                OutcomeSide::Down => evaluation.down_worst_case_ev_bps,
            };
            let expected_ev_per_notional =
                selected_worst_case_ev_bps.map(|ev_bps| ev_bps / BPS_DENOMINATOR);
            let book_impact_cap_notional = self.visible_book_notional_cap(selected_side);
            evaluation.expected_ev_per_notional = expected_ev_per_notional;
            evaluation.book_impact_cap_notional = book_impact_cap_notional;
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
                        let sized_fee_uncertainty_bps = fee_uncertainty_bps.max(probe.fee_bps);
                        let Some((selected_uncertainty_band, adjusted_probability_up)) = self
                            .adjusted_probability_up_for_fee_uncertainty(
                                now_ms,
                                selected_side,
                                fair_probability_up,
                                sized_fee_uncertainty_bps,
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
                    selected_sized_probe,
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
                    let sized_expected_ev_per_notional =
                        sized_executable_edge.edge_bps / BPS_DENOMINATOR;
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
                        let resized_fee_uncertainty_bps =
                            fee_uncertainty_bps.max(resized_probe.fee_bps);
                        let Some((resized_uncertainty_band, resized_adjusted_probability_up)) =
                            self.adjusted_probability_up_for_fee_uncertainty(
                                now_ms,
                                selected_side,
                                fair_probability_up,
                                resized_fee_uncertainty_bps,
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
                            resized_probe,
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
                        let final_expected_ev_per_notional =
                            resized_executable_edge.edge_bps / BPS_DENOMINATOR;
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
        self.register_selection_retry_timer();
        self.subscribe_reference_prices();
        self.subscribe_signal_quotes();
        self.subscribe_realized_volatility_sources();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.unsubscribe_realized_volatility_sources();
        self.unsubscribe_signal_quotes();
        self.unsubscribe_reference_prices();
        self.unsubscribe_resolution_strike();
        self.deregister_selection_retry_timer();
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        if event.name.as_str() == self.selection_retry_timer_name() {
            let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
            self.refresh_selection_from_cache(now_ms);
            self.retry_missing_live_input_subscriptions_at(now_ms);
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        if self
            .signal_instrument_id()
            .is_some_and(|instrument_id| quote.instrument_id == instrument_id)
        {
            if let Some(signal_quote) = self.signal_quote_from_tick(quote) {
                self.observe_signal_quote(&signal_quote);
            } else if let Some(signal_venue) = self.config.signal_venue.clone() {
                self.observe_invalid_signal_quote(
                    &signal_venue,
                    quote.ts_event.as_u64() / NANOS_PER_MILLI_U64,
                    Some(quote.ts_init.as_u64() / NANOS_PER_MILLI_U64),
                );
            }
        }
        for snapshot in self.context.observe_realized_volatility_quote(quote) {
            self.pricing.observe_realized_vol_snapshot(snapshot);
        }
        Ok(())
    }

    fn on_index_price(&mut self, update: &IndexPriceUpdate) -> anyhow::Result<()> {
        if self
            .resolution_instrument_id()
            .is_some_and(|instrument_id| update.instrument_id == instrument_id)
        {
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
            self.observe_reference_price_update(update);
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
            if let Some(open_position) = self.tracked_observed_position_mut() {
                open_position.book.update_from_deltas(deltas);
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

        self.refresh_fee_readiness();
        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        if matches!(self.exposure, ExposureState::Managed(_))
            && let Err(error) = self.try_submit_exit_order_for_trigger(
                now_ms,
                ExitEvaluationTriggerContext::new(
                    BoltV3ExitTriggerSource::BookDelta,
                    deltas.ts_event.as_u64() / NANOS_PER_MILLI_U64,
                    Some(deltas.ts_init.as_u64() / NANOS_PER_MILLI_U64),
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
            && let Err(error) = self.try_submit_entry_order(now_ms)
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

    fn on_order_filled(
        &mut self,
        event: &nautilus_model::events::OrderFilled,
    ) -> anyhow::Result<()> {
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
            .exit_pending()
            .is_some_and(|exit| exit.pending_exit.client_order_id == event.client_order_id);

        if entry_fill {
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
                    return Ok(());
                }
                if let Some(exit_pending) = self.exposure.exit_pending_mut() {
                    exit_pending
                        .pending_exit
                        .residual_position_observed_after_fill = true;
                }
                if !keep_pending_entry {
                    self.clear_managed_pending_entry_for_client_order(
                        event.client_order_id,
                        event.instrument_id,
                    );
                }
            } else if let (Some(position_id), Some(position_side)) =
                (event.position_id, position_side)
            {
                // Venue invariant (shared guard): never adopt a foreign-venue fill
                // into Managed — the exit path would submit against it. Same
                // venue-adoption class as the position-event path above.
                if self.quarantine_foreign_venue_event(event.instrument_id) {
                    return Ok(());
                }
                self.exposure = ExposureState::Managed(ManagedPositionState {
                    position: self.build_open_position_state(
                        None,
                        pending_context.as_ref(),
                        PositionMaterializationSpec {
                            instrument_id: event.instrument_id,
                            position_id,
                            entry_order_side: event.order_side,
                            side: position_side,
                            quantity: event.last_qty,
                            avg_px_open: event.last_px.as_f64(),
                        },
                        true,
                    ),
                    origin: ManagedPositionOrigin::StrategyEntry,
                    pending_entry: pending_context.clone().filter(|_| keep_pending_entry),
                });
                self.sync_exposure_context_from_active();
                self.refresh_book_subscriptions_for_current_state();
            } else {
                if let Some(pending) = pending_context.clone() {
                    let reason = if event.position_id.is_none() {
                        EntryReconcileReason::AwaitingPositionMaterialization
                    } else {
                        EntryReconcileReason::UnsupportedEntryFillSide {
                            order_side: event.order_side,
                        }
                    };
                    self.exposure = ExposureState::EntryReconcilePending { pending, reason };
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
            if let Some(market_id) = pending_context.and_then(|pending| pending.market_id.clone()) {
                self.record_market_fill(&market_id, now_ms);
            }
        } else if exit_fill {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return Ok(());
            }
            if let Some(market_id) = self
                .exposure
                .exit_pending()
                .and_then(|exit| exit.pending_exit.market_id.clone())
                .or_else(|| self.current_position_market_id())
            {
                self.record_market_fill(&market_id, now_ms);
            }
            if let Some(exit_pending) = self.exposure.exit_pending_mut() {
                exit_pending.pending_exit.fill_received = true;
                if exit_pending.pending_exit.close_received {
                    self.exposure = ExposureState::Flat;
                }
            }
        }
        self.prune_market_lifecycle(now_ms);
        Ok(())
    }

    fn on_order_canceled(
        &mut self,
        event: &nautilus_model::events::OrderCanceled,
    ) -> anyhow::Result<()> {
        self.clear_pending_entry_for_client_order(event.client_order_id, event.instrument_id);
        self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
        Ok(())
    }
}

nautilus_strategy!(BinaryOracleEdgeTaker, {
    fn on_order_rejected(&mut self, event: nautilus_model::events::OrderRejected) {
        self.record_entry_reject(&event);
        self.clear_pending_entry_for_client_order(event.client_order_id, event.instrument_id);
        self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn on_order_expired(&mut self, event: nautilus_model::events::OrderExpired) {
        self.clear_pending_entry_for_client_order(event.client_order_id, event.instrument_id);
        self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn on_position_opened(&mut self, _event: nautilus_model::events::PositionOpened) {
        self.materialize_position_from_event(
            _event.instrument_id,
            _event.position_id,
            _event.entry,
            _event.side,
            _event.quantity,
            _event.avg_px_open,
        );
    }

    fn on_position_changed(&mut self, _event: nautilus_model::events::PositionChanged) {
        self.materialize_position_from_event(
            _event.instrument_id,
            _event.position_id,
            _event.entry,
            _event.side,
            _event.quantity,
            _event.avg_px_open,
        );
    }

    fn on_position_closed(&mut self, event: nautilus_model::events::PositionClosed) {
        // Reclaim the exit-evidence flood-guard entry for this terminal position:
        // a closed position never re-emits exit evidence, so its dedup key is dead
        // state. Removal here is behavior-neutral and bounds the map over a long run.
        self.last_exit_evidence_outcome.remove(&event.position_id);
        let managed_position_close = match &self.exposure {
            ExposureState::Managed(position)
                if position.position.position_id == event.position_id =>
            {
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

        let exit_pending_close = self.exposure.exit_pending().is_some_and(|exit_pending| {
            exit_pending.pending_exit.position_id == Some(event.position_id)
        });
        if exit_pending_close {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if let ExposureState::ExitPending(exit_pending) = &mut self.exposure {
                exit_pending.pending_exit.close_received = true;
                exit_pending.position = None;
                if exit_pending.is_terminal() {
                    self.exposure = ExposureState::Flat;
                }
            }
        } else if matches!(
            &self.exposure,
            ExposureState::UnsupportedObserved(observed)
                if observed.observed.position_id == event.position_id
        ) {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if matches!(
                &self.exposure,
                ExposureState::UnsupportedObserved(observed)
                    if observed.observed.position_id == event.position_id
            ) {
                self.exposure = ExposureState::Flat;
            }
        } else {
            // Entry reconciliation may not have a position id yet; the instrument is the
            // strongest available key for a close that races ahead of position materialization.
            if matches!(
                &self.exposure,
                ExposureState::EntryReconcilePending { pending, .. }
                    if pending.instrument_id == event.instrument_id
            ) {
                self.exposure = ExposureState::Flat;
            }
        }
        self.refresh_book_subscriptions_for_current_state();
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }
});

pub const KEY: &str = stringify!(binary_oracle_edge_taker);

impl StrategyBuilder for BinaryOracleEdgeTakerBuilder {
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

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(Self::build_strategy(raw, context)?))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = BinaryOracleEdgeTaker::new(Self::parse_config(raw)?, context.clone());
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
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
const EVIDENCE_REASON_DERIVED_FROM_LEAD_GAP_JITTER_TIME_AND_FEE: &str =
    "derived_from_lead_gap_jitter_time_and_fee";
const EVIDENCE_REASON_UNCERTAINTY_BAND_UNAVAILABLE: &str = "uncertainty_band_unavailable";
const EVIDENCE_REASON_NO_FAST_VENUE_CLEARED_LEAD_QUALITY_THRESHOLDS: &str =
    "no_fast_venue_cleared_lead_quality_thresholds";
const EVIDENCE_REASON_LEAD_QUALITY_THRESHOLDS_APPLIED_TO_LIVE_FAST_SPOT_SELECTION: &str =
    "lead_quality_thresholds_applied_to_live_fast_spot_selection";
const EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_AND_SIZE_SELECTION: &str =
    "final_fee_requires_side_price_and_size_selection";
const EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_SIZE_AND_ACTUAL_FILL: &str =
    "final_fee_requires_side_price_size_and_actual_fill";
const EVIDENCE_REASON_NO_MANAGED_POSITION: &str = "no_managed_position";
const EVIDENCE_REASON_CAPTURED_FROM_STRATEGY_ENTRY_STATE: &str =
    "captured_from_strategy_entry_state";
const EVIDENCE_REASON_RECOVERY_BOOTSTRAP_POSITION_MISSING_ORIGINAL_FEE_RATE: &str =
    "recovery_bootstrap_position_missing_original_fee_rate";
const EVIDENCE_REASON_POSITION_STATE_MISSING_ORIGINAL_FEE_RATE: &str =
    "position_state_missing_original_fee_rate";
const ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED: &str = "strategy_core_not_registered";
const ENTRY_BLOCK_REASON_ENTRY_GATE_BLOCKED: &str = "entry_gate_blocked";
const ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED: &str = "entry_pricing_blocked";
const ENTRY_BLOCK_REASON_NO_SIDE_SELECTED: &str = "no_side_selected";
const ENTRY_BLOCK_REASON_SIZED_NOTIONAL_NOT_POSITIVE: &str = "sized_notional_not_positive";
const ENTRY_BLOCK_REASON_INSTRUMENT_ID_MISSING: &str = "instrument_id_missing";
const ENTRY_BLOCK_REASON_INSTRUMENT_MISSING_FROM_CACHE: &str = "instrument_missing_from_cache";
const ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING: &str = "entry_price_missing";
const ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED: &str = "quantity_rounding_failed";
const ENTRY_BLOCK_REASON_LIMIT_NOTIONAL_EXCEEDS_SIZED_NOTIONAL: &str =
    "limit_notional_exceeds_sized_notional";
const ENTRY_BLOCK_REASON_QUANTITY_NOT_POSITIVE: &str = "quantity_not_positive";
const ENTRY_BLOCK_REASON_POSITION_CONTRACT_INVALID: &str = "position_contract_invalid";
const ENTRY_BLOCK_REASON_ENTRY_POSITION_CONTRACT_UNSUPPORTED: &str =
    "entry_position_contract_unsupported";
const ENTRY_BLOCK_REASON_HISTORICAL_ENTRY_FEE_UNAVAILABLE: &str =
    "historical_entry_fee_unavailable";
const ENTRY_BLOCK_REASON_ONE_POSITION_INVARIANT_VIOLATION: &str =
    "one_position_invariant_violation";
const ENTRY_BLOCK_REASON_ENTRY_MALFORMED_REJECTED: &str = "entry_malformed_rejected";
const ENTRY_BLOCK_REASON_ENTRY_BALANCE_REJECTED: &str = "entry_balance_rejected";
const ENTRY_BLOCK_REASON_ENTRY_UNFILLABLE_REJECTED_UNCHANGED_BOOK: &str =
    "entry_unfillable_rejected_unchanged_book";
const ENTRY_BLOCK_REASON_ENTRY_QUOTE_NOTIONAL_BELOW_VENUE_MINIMUM: &str =
    "entry_quote_notional_below_venue_minimum";
const ENTRY_BLOCK_REASON_ENTRY_QUOTE_NOTIONAL_MINIMUM_UNMODELED: &str =
    "entry_quote_notional_minimum_unmodeled";
const EXIT_BLOCK_REASON_NO_OPEN_POSITION: &str = "no_open_position";
const EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING: &str = "exit_already_pending";
const EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING: &str = "entry_order_still_working";
const EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE: &str = "exit_decision_unavailable";
const EXIT_BLOCK_REASON_EXIT_HOLD: &str = "exit_hold";
const EXIT_BLOCK_REASON_OPEN_POSITION_MISSING: &str = "open_position_missing";
const EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID: &str = "exit_order_config_invalid";
const EXIT_BLOCK_REASON_EXIT_QUOTE_QUANTITY_UNSUPPORTED: &str = "exit_quote_quantity_unsupported";
const EXIT_BLOCK_REASON_EXIT_PRICE_MISSING: &str = "exit_price_missing";
const EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE: &str = "exit_quantity_not_positive";

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

fn outcome_side_to_evidence(side: OutcomeSide) -> BoltV3OutcomeSide {
    match side {
        OutcomeSide::Up => BoltV3OutcomeSide::Up,
        OutcomeSide::Down => BoltV3OutcomeSide::Down,
    }
}

fn forced_flat_reason_to_evidence(reason: &ForcedFlatReason) -> BoltV3ForcedFlatReason {
    match reason {
        ForcedFlatReason::Freeze => BoltV3ForcedFlatReason::Freeze,
        ForcedFlatReason::StaleReference => BoltV3ForcedFlatReason::StaleReference,
        ForcedFlatReason::ThinBook => BoltV3ForcedFlatReason::ThinBook,
        ForcedFlatReason::MetadataMismatch => BoltV3ForcedFlatReason::MetadataMismatch,
        ForcedFlatReason::FastVenueIncoherent => BoltV3ForcedFlatReason::FastVenueIncoherent,
    }
}

fn exposure_occupancy_to_evidence(occupancy: ExposureOccupancy) -> BoltV3ExposureOccupancy {
    match occupancy {
        ExposureOccupancy::PendingEntry => BoltV3ExposureOccupancy::PendingEntry,
        ExposureOccupancy::EntryReconcilePending => BoltV3ExposureOccupancy::EntryReconcilePending,
        ExposureOccupancy::ManagedPosition => BoltV3ExposureOccupancy::ManagedPosition,
        ExposureOccupancy::ExitPending => BoltV3ExposureOccupancy::ExitPending,
        ExposureOccupancy::UnsupportedObserved => BoltV3ExposureOccupancy::UnsupportedObserved,
        ExposureOccupancy::BlindRecovery => BoltV3ExposureOccupancy::BlindRecovery,
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

fn should_warn_on_exit_submission_block(reason: Option<&str>) -> bool {
    !matches!(reason, Some(reason) if reason == EXIT_BLOCK_REASON_NO_OPEN_POSITION
        || reason == EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING
        || reason == EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING
        || reason == EXIT_BLOCK_REASON_EXIT_HOLD)
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
fn submit_admission_request_from_order_for_client(
    execution_client_id: &str,
    intent: &BoltV3OrderIntentEvidence,
    order: &nautilus_model::orders::OrderAny,
) -> Result<BoltV3SubmitAdmissionRequest> {
    // Base-only test helper: it has no strategy cache/instrument context, so it
    // cannot size quote-quantity orders (that requires the full
    // `admission_base_notional_from_order` path with an instrument) and it cannot
    // value a market-style order — ENTRY OR EXIT (production values any price-less
    // base-quantity order at the instrument's structural price ceiling via the
    // strategy method). It is NOT a divergent copy of the notional
    // math — for the shapes it DOES accept (base-quantity firm-limit orders) it
    // reuses the shared base-quantity definition so the order is sized
    // identically here and in the production strategy.
    anyhow::ensure!(
        !order.is_quote_quantity(),
        "test submit admission helper requires strategy cache context for quote-quantity orders"
    );
    anyhow::ensure!(
        order.price().is_some(),
        "test submit admission helper cannot value a market-style order (no firm limit price): production values it at the instrument price ceiling — use `strategy.submit_admission_request_from_order` with a cache-seeded instrument"
    );
    build_submit_admission_request_from_order(
        BoltV3SubmitAdmissionRequestInput {
            execution_client_id,
            intent,
            order,
            instrument: None,
            quote_quantity_last_price: None,
            quote_quantity_reference_price: None,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
            risk_reducing_exit_position: None,
        },
        |_| Ok(Decimal::ZERO),
    )
}

#[cfg(test)]
mod tests;
