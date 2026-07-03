use std::collections::BTreeMap;

use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientOrderId, InstrumentId},
};

use crate::{
    bolt_v3_binary_outcome_edge::{BinaryOutcomeEdgeBlockReason, BinaryOutcomeEdgeResult},
    bolt_v3_decision_evidence::{
        BoltV3BinaryOutcomeEdgeBlockReason, BoltV3EntryBlockReason, BoltV3EntryPricingBlockReason,
        BoltV3EntrySkipEvidence, BoltV3EntrySkipReasonCategory,
        BoltV3RealizedVolatilitySourceDiagnosticEvidence,
    },
    bolt_v3_market_families::OutcomeSide,
    bolt_v3_numeric::Probability,
    bolt_v3_taker_pricing::TakerPricingBlockReason,
};

use super::{
    ENTRY_BLOCK_REASON_ENTRY_GATE_BLOCKED, ENTRY_BLOCK_REASON_ENTRY_POSITION_CONTRACT_UNSUPPORTED,
    ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING, ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED,
    ENTRY_BLOCK_REASON_HISTORICAL_ENTRY_FEE_UNAVAILABLE, ENTRY_BLOCK_REASON_INSTRUMENT_ID_MISSING,
    ENTRY_BLOCK_REASON_INSTRUMENT_MISSING_FROM_CACHE,
    ENTRY_BLOCK_REASON_LIMIT_NOTIONAL_EXCEEDS_SIZED_NOTIONAL, ENTRY_BLOCK_REASON_NO_SIDE_SELECTED,
    ENTRY_BLOCK_REASON_ONE_POSITION_INVARIANT_VIOLATION,
    ENTRY_BLOCK_REASON_POSITION_CONTRACT_INVALID, ENTRY_BLOCK_REASON_QUANTITY_NOT_POSITIVE,
    ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED, ENTRY_BLOCK_REASON_SIZED_NOTIONAL_NOT_POSITIVE,
    ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED, SelectionPhase, exposure::ExposureOccupancy,
    exposure_occupancy_to_evidence, forced_flat_reason_to_evidence, option_evidence_number,
    outcome_side_to_evidence,
};
use crate::bolt_v3_feed_health::ForcedFlatReason;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EntryBlockReason {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntryGateDecision {
    pub(super) blocked_by: Vec<EntryBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EntryPricingInputs {
    pub(super) spot_price: f64,
    pub(super) strike_price: f64,
    pub(super) seconds_to_expiry: u64,
    pub(super) realized_vol: f64,
    pub(super) theta_scaled_min_edge_bps: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EntryPricingBlockReason {
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
    /// The sized re-evaluation oscillated: the final re-priced edge does not
    /// support the resized notional, so the entry fails closed.
    SizedNotionalUnsupported(OutcomeSide),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RealizedVolatilityEvidenceFields {
    pub(super) surface_id: String,
    pub(super) as_of_ms: Option<u64>,
    pub(super) annualized_decimal: String,
    pub(super) measured_annualized_decimal: String,
    pub(super) noise_robust_annualized_decimal: String,
    pub(super) continuous_annualized_decimal: String,
    pub(super) jump_annualized_decimal: String,
    pub(super) forecast_annualized_decimal: String,
    pub(super) pricing_component: String,
    pub(super) seconds_per_annum: String,
    pub(super) aggregation: String,
    pub(super) sources_used: Vec<String>,
    pub(super) source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub(super) unknown_source_rejections: BTreeMap<String, u64>,
    pub(super) blockers: Vec<String>,
    pub(super) config_fingerprint: String,
}

pub(super) fn entry_pricing_block_reason_from_taker(
    reason: TakerPricingBlockReason,
) -> EntryPricingBlockReason {
    match reason {
        TakerPricingBlockReason::SpotPriceMissing => EntryPricingBlockReason::SpotPriceMissing,
        TakerPricingBlockReason::ReferenceCurrentPriceStale => {
            EntryPricingBlockReason::ReferenceCurrentPriceStale
        }
        TakerPricingBlockReason::StrikePriceMissing => EntryPricingBlockReason::StrikePriceMissing,
        TakerPricingBlockReason::SecondsToExpiryMissing => {
            EntryPricingBlockReason::SecondsToExpiryMissing
        }
        TakerPricingBlockReason::RealizedVolNotReady => {
            EntryPricingBlockReason::RealizedVolNotReady
        }
        TakerPricingBlockReason::ThetaScalerUnavailable => {
            EntryPricingBlockReason::ThetaScalerUnavailable
        }
        TakerPricingBlockReason::FairProbabilityUnavailable => {
            EntryPricingBlockReason::FairProbabilityUnavailable
        }
    }
}

pub(super) fn push_executable_edge_pricing_block(
    reasons: &mut Vec<EntryPricingBlockReason>,
    side: OutcomeSide,
    reason: Option<BinaryOutcomeEdgeBlockReason>,
) {
    match reason {
        Some(BinaryOutcomeEdgeBlockReason::FeeUnavailable) => {
            reasons.push(EntryPricingBlockReason::FeeUnavailable(side));
        }
        Some(
            reason @ (BinaryOutcomeEdgeBlockReason::MissingOrderBook
            | BinaryOutcomeEdgeBlockReason::InsufficientDepth
            | BinaryOutcomeEdgeBlockReason::InvalidProbability
            | BinaryOutcomeEdgeBlockReason::InvalidCost
            | BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
            | BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold
            | BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge),
        ) => {
            reasons.push(EntryPricingBlockReason::ExecutableEdgeUnavailable(
                side, reason,
            ));
        }
        None => {}
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntryEvaluation {
    pub(super) gate: EntryGateDecision,
    pub(super) pricing_blocked_by: Vec<EntryPricingBlockReason>,
    pub(super) fair_probability_up: Option<Probability>,
    pub(super) uncertainty_band_probability: Option<Probability>,
    pub(super) up_executable_edge: Option<BinaryOutcomeEdgeResult>,
    pub(super) down_executable_edge: Option<BinaryOutcomeEdgeResult>,
    pub(super) up_worst_case_ev_bps: Option<f64>,
    pub(super) down_worst_case_ev_bps: Option<f64>,
    pub(super) sized_executable_edge: Option<BinaryOutcomeEdgeResult>,
    pub(super) sized_worst_case_ev_bps: Option<f64>,
    pub(super) min_worst_case_ev_bps: Option<f64>,
    pub(super) expected_ev_per_notional: Option<f64>,
    pub(super) book_impact_cap_notional: Option<f64>,
    pub(super) sized_notional: Option<f64>,
    pub(super) selected_side: Option<OutcomeSide>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntrySubmissionDecision {
    pub(super) evaluation: EntryEvaluation,
    pub(super) instrument_id: Option<InstrumentId>,
    pub(super) order_side: Option<OrderSide>,
    pub(super) price: Option<f64>,
    pub(super) quantity_value: Option<f64>,
    pub(super) client_order_id: Option<ClientOrderId>,
    pub(super) blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntryEvaluationLogFields {
    pub(super) market_id: Option<String>,
    pub(super) phase: SelectionPhase,
    pub(super) gate_blocked_by: Vec<EntryBlockReason>,
    pub(super) pricing_blocked_by: Vec<EntryPricingBlockReason>,
    pub(super) spot_price: Option<f64>,
    pub(super) spot_venue_name: Option<String>,
    pub(super) reference_current_price: Option<f64>,
    pub(super) interval_open: Option<f64>,
    pub(super) seconds_to_expiry: Option<u64>,
    pub(super) realized_vol: Option<f64>,
    pub(super) realized_vol_source_venue: Option<String>,
    pub(super) realized_vol_source_ts_ms: Option<u64>,
    pub(super) pricing_kurtosis: f64,
    pub(super) theta_decay_factor: f64,
    pub(super) theta_scaled_min_edge_bps: Option<f64>,
    pub(super) fair_probability_up: Option<f64>,
    pub(super) fair_probability_down: Option<f64>,
    pub(super) uncertainty_band_probability: Option<f64>,
    pub(super) uncertainty_band_live: bool,
    pub(super) uncertainty_band_reason: &'static str,
    pub(super) lead_agreement_corr: Option<f64>,
    pub(super) fast_venue_age_ms: Option<u64>,
    pub(super) fast_venue_jitter_ms: Option<u64>,
    pub(super) up_fee_bps: Option<f64>,
    pub(super) down_fee_bps: Option<f64>,
    pub(super) up_entry_cost: Option<f64>,
    pub(super) down_entry_cost: Option<f64>,
    pub(super) up_entry_limit_price: Option<f64>,
    pub(super) down_entry_limit_price: Option<f64>,
    pub(super) up_gross_cost_cents: Option<f64>,
    pub(super) down_gross_cost_cents: Option<f64>,
    pub(super) up_fee_cost_cents: Option<f64>,
    pub(super) down_fee_cost_cents: Option<f64>,
    pub(super) up_slippage_buffer_cents: Option<f64>,
    pub(super) down_slippage_buffer_cents: Option<f64>,
    pub(super) up_total_adjusted_cost_cents: Option<f64>,
    pub(super) down_total_adjusted_cost_cents: Option<f64>,
    pub(super) up_edge_cents_per_share: Option<f64>,
    pub(super) down_edge_cents_per_share: Option<f64>,
    pub(super) up_worst_case_ev_bps: Option<f64>,
    pub(super) down_worst_case_ev_bps: Option<f64>,
    pub(super) sized_fee_bps: Option<f64>,
    pub(super) sized_entry_cost: Option<f64>,
    pub(super) sized_entry_limit_price: Option<f64>,
    pub(super) sized_gross_cost_cents: Option<f64>,
    pub(super) sized_fee_cost_cents: Option<f64>,
    pub(super) sized_slippage_buffer_cents: Option<f64>,
    pub(super) sized_total_adjusted_cost_cents: Option<f64>,
    pub(super) sized_edge_cents_per_share: Option<f64>,
    pub(super) sized_worst_case_ev_bps: Option<f64>,
    pub(super) expected_ev_per_notional: Option<f64>,
    pub(super) order_notional_target: f64,
    pub(super) maximum_position_notional: f64,
    pub(super) risk_lambda: f64,
    pub(super) sizing_ev_reference_bps: u64,
    pub(super) book_impact_cap_bps: u64,
    pub(super) book_impact_cap_notional: Option<f64>,
    pub(super) sized_notional: Option<f64>,
    pub(super) selected_side: Option<OutcomeSide>,
    pub(super) fast_venue_available: bool,
    pub(super) reference_current_price_available: bool,
    pub(super) reference_current_price_available_without_fast_venue: bool,
    pub(super) lead_quality_policy_applied: bool,
    pub(super) lead_quality_reason: &'static str,
    pub(super) final_fee_amount_known: bool,
    pub(super) final_fee_amount_reason: &'static str,
    pub(super) submission_instrument_id: Option<InstrumentId>,
    pub(super) submission_order_side: Option<OrderSide>,
    pub(super) submission_price: Option<f64>,
    pub(super) submission_quantity_value: Option<f64>,
    pub(super) submission_client_order_id: Option<ClientOrderId>,
    pub(super) submission_blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EntrySkipDedupeKey {
    pub(super) reason_category: BoltV3EntrySkipReasonCategory,
    pub(super) gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pub(super) pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    pub(super) market_id: Option<String>,
    pub(super) interval_open: Option<String>,
    pub(super) fast_venue_available: bool,
    pub(super) reference_current_price_available: bool,
    pub(super) fast_venue_incoherent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ForcedFlatEvidenceInputs {
    pub(super) stale_reference_after_ms: Option<u64>,
    pub(super) last_reference_ts_ms: Option<u64>,
    pub(super) min_liquidity_required: Option<String>,
    pub(super) liquidity_available: Option<String>,
    pub(super) frozen: bool,
    pub(super) metadata_matches_selection: bool,
    pub(super) fast_venue_incoherent: bool,
}

impl BoltV3EntrySkipEvidence {
    pub(super) fn from_entry_skip(
        strategy_id: String,
        now_ms: u64,
        reason_category: BoltV3EntrySkipReasonCategory,
        unclassified_context: Option<String>,
        fields: &EntryEvaluationLogFields,
        forced_flat_inputs: ForcedFlatEvidenceInputs,
    ) -> Self {
        Self {
            strategy_id,
            now_ms,
            reason_category,
            unclassified_context,
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
            phase: format!("{:?}", fields.phase),
            seconds_to_market_end: fields.seconds_to_expiry,
            spot_price: option_evidence_number(fields.spot_price),
            reference_current_price: option_evidence_number(fields.reference_current_price),
            fast_venue_available: fields.fast_venue_available,
            reference_current_price_available: fields.reference_current_price_available,
            realized_vol: option_evidence_number(fields.realized_vol),
            realized_vol_source_venue: fields.realized_vol_source_venue.clone(),
            realized_vol_source_ts_ms: fields.realized_vol_source_ts_ms,
            fair_probability_up: option_evidence_number(fields.fair_probability_up),
            fair_probability_down: option_evidence_number(fields.fair_probability_down),
            selected_side: fields.selected_side.map(outcome_side_to_evidence),
            sized_notional: option_evidence_number(fields.sized_notional),
            sized_worst_case_ev_bps: option_evidence_number(fields.sized_worst_case_ev_bps),
            sized_edge_cents_per_share: option_evidence_number(fields.sized_edge_cents_per_share),
            theta_scaled_min_edge_bps: option_evidence_number(fields.theta_scaled_min_edge_bps),
            up_fee_bps: option_evidence_number(fields.up_fee_bps),
            down_fee_bps: option_evidence_number(fields.down_fee_bps),
            submission_blocked_reason: fields
                .submission_blocked_reason
                .and_then(entry_skip_reason_category_from_str)
                .or(Some(reason_category)),
            stale_reference_after_ms: forced_flat_inputs.stale_reference_after_ms,
            last_reference_ts_ms: forced_flat_inputs.last_reference_ts_ms,
            min_liquidity_required: forced_flat_inputs.min_liquidity_required,
            liquidity_available: forced_flat_inputs.liquidity_available,
            frozen: forced_flat_inputs.frozen,
            metadata_matches_selection: forced_flat_inputs.metadata_matches_selection,
            fast_venue_incoherent: forced_flat_inputs.fast_venue_incoherent,
        }
    }
}

pub(super) fn entry_block_reason_to_evidence(reason: &EntryBlockReason) -> BoltV3EntryBlockReason {
    match reason {
        EntryBlockReason::PhaseNotActive => BoltV3EntryBlockReason::PhaseNotActive,
        EntryBlockReason::MetadataMismatch => BoltV3EntryBlockReason::MetadataMismatch,
        EntryBlockReason::ActiveBookNotPriced => BoltV3EntryBlockReason::ActiveBookNotPriced,
        EntryBlockReason::BookCrossed => BoltV3EntryBlockReason::BookCrossed,
        EntryBlockReason::IntervalOpenMissing => BoltV3EntryBlockReason::IntervalOpenMissing,
        EntryBlockReason::WarmupIncomplete => BoltV3EntryBlockReason::WarmupIncomplete,
        EntryBlockReason::FeesNotReady => BoltV3EntryBlockReason::FeesNotReady,
        EntryBlockReason::RecoveryMode => BoltV3EntryBlockReason::RecoveryMode,
        EntryBlockReason::MarketCoolingDown => BoltV3EntryBlockReason::MarketCoolingDown,
        EntryBlockReason::SpotSpikeCooldown => BoltV3EntryBlockReason::SpotSpikeCooldown,
        EntryBlockReason::ForcedFlat(reason) => {
            BoltV3EntryBlockReason::ForcedFlat(forced_flat_reason_to_evidence(reason))
        }
        EntryBlockReason::OnePositionInvariant(occupancy) => {
            BoltV3EntryBlockReason::OnePositionInvariant(exposure_occupancy_to_evidence(*occupancy))
        }
    }
}

fn binary_edge_block_reason_to_evidence(
    reason: BinaryOutcomeEdgeBlockReason,
) -> BoltV3BinaryOutcomeEdgeBlockReason {
    match reason {
        BinaryOutcomeEdgeBlockReason::MissingOrderBook => {
            BoltV3BinaryOutcomeEdgeBlockReason::MissingOrderBook
        }
        BinaryOutcomeEdgeBlockReason::InsufficientDepth => {
            BoltV3BinaryOutcomeEdgeBlockReason::InsufficientDepth
        }
        BinaryOutcomeEdgeBlockReason::InvalidProbability => {
            BoltV3BinaryOutcomeEdgeBlockReason::InvalidProbability
        }
        BinaryOutcomeEdgeBlockReason::InvalidCost => {
            BoltV3BinaryOutcomeEdgeBlockReason::InvalidCost
        }
        BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape => {
            BoltV3BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
        }
        BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold => {
            BoltV3BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold
        }
        BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge => {
            BoltV3BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge
        }
        BinaryOutcomeEdgeBlockReason::FeeUnavailable => {
            BoltV3BinaryOutcomeEdgeBlockReason::FeeUnavailable
        }
    }
}

pub(super) fn entry_pricing_block_reason_to_evidence(
    reason: &EntryPricingBlockReason,
) -> BoltV3EntryPricingBlockReason {
    match reason {
        EntryPricingBlockReason::SpotPriceMissing => {
            BoltV3EntryPricingBlockReason::SpotPriceMissing
        }
        EntryPricingBlockReason::ReferenceCurrentPriceStale => {
            BoltV3EntryPricingBlockReason::ReferenceCurrentPriceStale
        }
        EntryPricingBlockReason::StrikePriceMissing => {
            BoltV3EntryPricingBlockReason::StrikePriceMissing
        }
        EntryPricingBlockReason::SecondsToExpiryMissing => {
            BoltV3EntryPricingBlockReason::SecondsToExpiryMissing
        }
        EntryPricingBlockReason::RealizedVolNotReady => {
            BoltV3EntryPricingBlockReason::RealizedVolNotReady
        }
        EntryPricingBlockReason::ThetaScalerUnavailable => {
            BoltV3EntryPricingBlockReason::ThetaScalerUnavailable
        }
        EntryPricingBlockReason::UncertaintyBandUnavailable => {
            BoltV3EntryPricingBlockReason::UncertaintyBandUnavailable
        }
        EntryPricingBlockReason::FairProbabilityUnavailable => {
            BoltV3EntryPricingBlockReason::FairProbabilityUnavailable
        }
        EntryPricingBlockReason::FeeUnavailable(side) => {
            BoltV3EntryPricingBlockReason::FeeUnavailable(outcome_side_to_evidence(*side))
        }
        EntryPricingBlockReason::ExecutableEntryCostUnavailable(side) => {
            BoltV3EntryPricingBlockReason::ExecutableEntryCostUnavailable(outcome_side_to_evidence(
                *side,
            ))
        }
        EntryPricingBlockReason::ExecutableEdgeUnavailable(side, reason) => {
            BoltV3EntryPricingBlockReason::ExecutableEdgeUnavailable(
                outcome_side_to_evidence(*side),
                binary_edge_block_reason_to_evidence(*reason),
            )
        }
        EntryPricingBlockReason::SizedNotionalUnsupported(side) => {
            BoltV3EntryPricingBlockReason::SizedNotionalUnsupported(outcome_side_to_evidence(*side))
        }
    }
}

pub(super) fn entry_skip_reason_category_from_str(
    reason: &str,
) -> Option<BoltV3EntrySkipReasonCategory> {
    match reason {
        ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED => {
            Some(BoltV3EntrySkipReasonCategory::StrategyCoreNotRegistered)
        }
        ENTRY_BLOCK_REASON_ENTRY_GATE_BLOCKED => {
            Some(BoltV3EntrySkipReasonCategory::EntryGateBlocked)
        }
        ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED => {
            Some(BoltV3EntrySkipReasonCategory::EntryPricingBlocked)
        }
        ENTRY_BLOCK_REASON_NO_SIDE_SELECTED => Some(BoltV3EntrySkipReasonCategory::NoSideSelected),
        ENTRY_BLOCK_REASON_SIZED_NOTIONAL_NOT_POSITIVE => {
            Some(BoltV3EntrySkipReasonCategory::SizedNotionalNotPositive)
        }
        ENTRY_BLOCK_REASON_INSTRUMENT_ID_MISSING => {
            Some(BoltV3EntrySkipReasonCategory::InstrumentIdMissing)
        }
        ENTRY_BLOCK_REASON_INSTRUMENT_MISSING_FROM_CACHE => {
            Some(BoltV3EntrySkipReasonCategory::InstrumentMissingFromCache)
        }
        ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING => {
            Some(BoltV3EntrySkipReasonCategory::EntryPriceMissing)
        }
        ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED => {
            Some(BoltV3EntrySkipReasonCategory::QuantityRoundingFailed)
        }
        ENTRY_BLOCK_REASON_LIMIT_NOTIONAL_EXCEEDS_SIZED_NOTIONAL => {
            Some(BoltV3EntrySkipReasonCategory::LimitNotionalExceedsSizedNotional)
        }
        ENTRY_BLOCK_REASON_QUANTITY_NOT_POSITIVE => {
            Some(BoltV3EntrySkipReasonCategory::QuantityNotPositive)
        }
        ENTRY_BLOCK_REASON_POSITION_CONTRACT_INVALID => {
            Some(BoltV3EntrySkipReasonCategory::PositionContractInvalid)
        }
        ENTRY_BLOCK_REASON_ENTRY_POSITION_CONTRACT_UNSUPPORTED => {
            Some(BoltV3EntrySkipReasonCategory::EntryPositionContractUnsupported)
        }
        ENTRY_BLOCK_REASON_HISTORICAL_ENTRY_FEE_UNAVAILABLE => {
            Some(BoltV3EntrySkipReasonCategory::HistoricalEntryFeeUnavailable)
        }
        ENTRY_BLOCK_REASON_ONE_POSITION_INVARIANT_VIOLATION => {
            Some(BoltV3EntrySkipReasonCategory::OnePositionInvariantViolation)
        }
        _ => None,
    }
}
