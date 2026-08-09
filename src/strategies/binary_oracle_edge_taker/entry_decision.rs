use std::collections::BTreeMap;

use nautilus_model::{
    enums::OrderSide,
    identifiers::{ClientOrderId, InstrumentId},
};

use crate::bolt_v3_evidence_novelty::EvidenceCanonicalState;
use crate::{
    bolt_v3_binary_outcome_edge::{BinaryOutcomeEdgeBlockReason, BinaryOutcomeEdgeResult},
    bolt_v3_current_evidence::{
        BinaryOutcomeEdgeBlockReason as EvidenceBinaryOutcomeEdgeBlockReason,
        EntryBlockReason as EvidenceEntryBlockReason,
        EntryPricingBlockReason as EvidenceEntryPricingBlockReason,
        EntryRealizedVolatilitySnapshotFact, EntrySkipFact,
        EntrySkipReason as EvidenceEntrySkipReason, EvidenceSelectionPhase,
        RealizedVolAggregation as EvidenceRealizedVolAggregation,
        RealizedVolBlockReason as EvidenceRealizedVolBlockReason,
        RealizedVolPricingComponent as EvidenceRealizedVolPricingComponent,
        RealizedVolatilitySourceDiagnosticFact, RvGateResult as EvidenceRvGateResult,
    },
    bolt_v3_market_families::OutcomeSide,
    bolt_v3_numeric::Probability,
    bolt_v3_taker_pricing::TakerPricingBlockReason,
    bolt_v3_timestamp_domain::LocalReceiveMs,
};

use super::{
    SelectionPhase, exposure::ExposureOccupancy, exposure_occupancy_to_evidence,
    forced_flat_reason_to_evidence, option_evidence_number, outcome_side_to_evidence,
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

/// Receive-clock ownership captured once by the entry trigger and shared by
/// every consumer participating in that evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EntryEvaluationReceiveContext {
    receive_ms: LocalReceiveMs,
}

impl EntryEvaluationReceiveContext {
    pub(super) const fn new(receive_ms: LocalReceiveMs) -> Self {
        Self { receive_ms }
    }

    pub(super) const fn receive_ms(self) -> LocalReceiveMs {
        self.receive_ms
    }
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
    ExecutableEntryCostUnavailable(OutcomeSide),
    ExecutableEdgeUnavailable(OutcomeSide, BinaryOutcomeEdgeBlockReason),
    /// The sized re-evaluation oscillated: the final re-priced edge does not
    /// support the resized notional, so the entry fails closed.
    SizedNotionalUnsupported(OutcomeSide),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RealizedVolatilityEvidenceFields {
    pub(super) surface_id: String,
    pub(super) as_of_ms: Option<u64>,
    pub(super) annualized_decimal: Option<String>,
    pub(super) measured_annualized_decimal: Option<String>,
    pub(super) noise_robust_annualized_decimal: Option<String>,
    pub(super) continuous_annualized_decimal: Option<String>,
    pub(super) jump_annualized_decimal: Option<String>,
    pub(super) forecast_annualized_decimal: Option<String>,
    pub(super) pricing_component: Option<EvidenceRealizedVolPricingComponent>,
    pub(super) seconds_per_annum: String,
    pub(super) aggregation: Option<EvidenceRealizedVolAggregation>,
    pub(super) sources_used: Vec<String>,
    pub(super) source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticFact>,
    pub(super) unknown_source_rejections: BTreeMap<String, u64>,
    pub(super) blockers: Vec<EvidenceRealizedVolBlockReason>,
    pub(super) config_fingerprint: String,
}

impl RealizedVolatilityEvidenceFields {
    pub(super) fn to_durable_snapshot(&self) -> Option<EntryRealizedVolatilitySnapshotFact> {
        Some(EntryRealizedVolatilitySnapshotFact {
            surface_id: self.surface_id.clone(),
            as_of_ms: self.as_of_ms,
            annualized_decimal: self.annualized_decimal.clone(),
            measured_annualized_decimal: self.measured_annualized_decimal.clone(),
            noise_robust_annualized_decimal: self.noise_robust_annualized_decimal.clone(),
            continuous_annualized_decimal: self.continuous_annualized_decimal.clone(),
            jump_annualized_decimal: self.jump_annualized_decimal.clone(),
            forecast_annualized_decimal: self.forecast_annualized_decimal.clone(),
            pricing_component: self.pricing_component?,
            seconds_per_annum: self.seconds_per_annum.clone(),
            aggregation: self.aggregation?,
            sources_used: self.sources_used.clone(),
            source_diagnostics: self.source_diagnostics.clone(),
            unknown_source_rejections: self.unknown_source_rejections.clone(),
            blockers: self.blockers.clone(),
            config_fingerprint: self.config_fingerprint.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct EntryRealizedVolatilityReceipt {
    pub(super) gate_result: EvidenceRvGateResult,
    pub(super) receive_watermark_ms: Option<crate::bolt_v3_timestamp_domain::LocalReceiveMs>,
    pub(super) realized_vol: Option<f64>,
    pub(super) source_venue: Option<String>,
    pub(super) source_ts_ms: Option<u64>,
    pub(super) evidence: RealizedVolatilityEvidenceFields,
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
    pub(super) realized_volatility_receipt: EntryRealizedVolatilityReceipt,
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
    pub(super) blocked_reason: Option<EvidenceEntrySkipReason>,
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
    pub(super) realized_vol_gate_result: EvidenceRvGateResult,
    pub(super) realized_vol_receive_watermark_ms:
        Option<crate::bolt_v3_timestamp_domain::LocalReceiveMs>,
    pub(super) realized_volatility_evidence: RealizedVolatilityEvidenceFields,
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
    pub(super) submission_blocked_reason: Option<EvidenceEntrySkipReason>,
}

/// The blocked-snapshot producer's semantic state, from the closed registry.
///
/// Named rather than computed from an index: the registry is the sole authority
/// for this domain, so a state that is not in it must not be expressible here.
/// The match is exhaustive, so adding an RV gate result fails this build until
/// the registry gains the matching pair.
pub(super) const fn blocked_strategy_input_canonical_state(
    gate_result: EvidenceRvGateResult,
    watermark_present: bool,
) -> EvidenceCanonicalState {
    match (gate_result, watermark_present) {
        (EvidenceRvGateResult::Accepted, false) => {
            EvidenceCanonicalState::BlockedStrategyInputAcceptedWatermarkAbsent
        }
        (EvidenceRvGateResult::Accepted, true) => {
            EvidenceCanonicalState::BlockedStrategyInputAcceptedWatermarkPresent
        }
        (EvidenceRvGateResult::MissingSnapshot, false) => {
            EvidenceCanonicalState::BlockedStrategyInputMissingSnapshotWatermarkAbsent
        }
        (EvidenceRvGateResult::MissingSnapshot, true) => {
            EvidenceCanonicalState::BlockedStrategyInputMissingSnapshotWatermarkPresent
        }
        (EvidenceRvGateResult::MissingEvaluationEventTime, false) => {
            EvidenceCanonicalState::BlockedStrategyInputMissingEvaluationEventTimeWatermarkAbsent
        }
        (EvidenceRvGateResult::MissingEvaluationEventTime, true) => {
            EvidenceCanonicalState::BlockedStrategyInputMissingEvaluationEventTimeWatermarkPresent
        }
        (EvidenceRvGateResult::RejectedFutureDated, false) => {
            EvidenceCanonicalState::BlockedStrategyInputRejectedFutureDatedWatermarkAbsent
        }
        (EvidenceRvGateResult::RejectedFutureDated, true) => {
            EvidenceCanonicalState::BlockedStrategyInputRejectedFutureDatedWatermarkPresent
        }
        (EvidenceRvGateResult::RejectedStale, false) => {
            EvidenceCanonicalState::BlockedStrategyInputRejectedStaleWatermarkAbsent
        }
        (EvidenceRvGateResult::RejectedStale, true) => {
            EvidenceCanonicalState::BlockedStrategyInputRejectedStaleWatermarkPresent
        }
        (EvidenceRvGateResult::RejectedNotReady, false) => {
            EvidenceCanonicalState::BlockedStrategyInputRejectedNotReadyWatermarkAbsent
        }
        (EvidenceRvGateResult::RejectedNotReady, true) => {
            EvidenceCanonicalState::BlockedStrategyInputRejectedNotReadyWatermarkPresent
        }
    }
}

/// The entry-skip producer's semantic state, from the closed registry.
///
/// The novelty axis is the skip reason itself, which is what the registry's
/// twenty-state domain enumerates. Exhaustive for the same reason as above.
pub(super) const fn entry_skip_canonical_state(
    reason: EvidenceEntrySkipReason,
) -> EvidenceCanonicalState {
    match reason {
        EvidenceEntrySkipReason::StrategyCoreNotRegistered => {
            EvidenceCanonicalState::EntrySkipStrategyCoreNotRegistered
        }
        EvidenceEntrySkipReason::EntryGateBlocked => {
            EvidenceCanonicalState::EntrySkipEntryGateBlocked
        }
        EvidenceEntrySkipReason::EntryPricingBlocked => {
            EvidenceCanonicalState::EntrySkipEntryPricingBlocked
        }
        EvidenceEntrySkipReason::NoSideSelected => EvidenceCanonicalState::EntrySkipNoSideSelected,
        EvidenceEntrySkipReason::SizedNotionalNotPositive => {
            EvidenceCanonicalState::EntrySkipSizedNotionalNotPositive
        }
        EvidenceEntrySkipReason::InstrumentIdMissing => {
            EvidenceCanonicalState::EntrySkipInstrumentIdMissing
        }
        EvidenceEntrySkipReason::InstrumentMissingFromCache => {
            EvidenceCanonicalState::EntrySkipInstrumentMissingFromCache
        }
        EvidenceEntrySkipReason::EntryPriceMissing => {
            EvidenceCanonicalState::EntrySkipEntryPriceMissing
        }
        EvidenceEntrySkipReason::QuantityRoundingFailed => {
            EvidenceCanonicalState::EntrySkipQuantityRoundingFailed
        }
        EvidenceEntrySkipReason::LimitNotionalExceedsSizedNotional => {
            EvidenceCanonicalState::EntrySkipLimitNotionalExceedsSizedNotional
        }
        EvidenceEntrySkipReason::EntryQuoteNotionalBelowVenueMinimum => {
            EvidenceCanonicalState::EntrySkipEntryQuoteNotionalBelowVenueMinimum
        }
        EvidenceEntrySkipReason::EntryQuoteNotionalMinimumUnmodeled => {
            EvidenceCanonicalState::EntrySkipEntryQuoteNotionalMinimumUnmodeled
        }
        EvidenceEntrySkipReason::QuantityNotPositive => {
            EvidenceCanonicalState::EntrySkipQuantityNotPositive
        }
        EvidenceEntrySkipReason::PositionContractInvalid => {
            EvidenceCanonicalState::EntrySkipPositionContractInvalid
        }
        EvidenceEntrySkipReason::EntryPositionContractUnsupported => {
            EvidenceCanonicalState::EntrySkipEntryPositionContractUnsupported
        }
        EvidenceEntrySkipReason::HistoricalEntryFeeUnavailable => {
            EvidenceCanonicalState::EntrySkipHistoricalEntryFeeUnavailable
        }
        EvidenceEntrySkipReason::OnePositionInvariantViolation => {
            EvidenceCanonicalState::EntrySkipOnePositionInvariantViolation
        }
        EvidenceEntrySkipReason::EntryMalformedRejected => {
            EvidenceCanonicalState::EntrySkipEntryMalformedRejected
        }
        EvidenceEntrySkipReason::EntryBalanceRejected => {
            EvidenceCanonicalState::EntrySkipEntryBalanceRejected
        }
        EvidenceEntrySkipReason::EntryUnfillableRejectedUnchangedBook => {
            EvidenceCanonicalState::EntrySkipEntryUnfillableRejectedUnchangedBook
        }
    }
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

pub(super) fn entry_skip_fact(
    strategy_id: String,
    now_ms: u64,
    reason_category: EvidenceEntrySkipReason,
    fields: &EntryEvaluationLogFields,
    forced_flat_inputs: ForcedFlatEvidenceInputs,
) -> EntrySkipFact {
    EntrySkipFact {
        strategy_id,
        now_ms,
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
        phase: match fields.phase {
            SelectionPhase::Active => EvidenceSelectionPhase::Active,
            SelectionPhase::Freeze => EvidenceSelectionPhase::Freeze,
            SelectionPhase::Idle => EvidenceSelectionPhase::Idle,
        },
        seconds_to_market_end: fields.seconds_to_expiry,
        spot_price: option_evidence_number(fields.spot_price),
        reference_current_price: option_evidence_number(fields.reference_current_price),
        fast_venue_available: fields.fast_venue_available,
        reference_current_price_available: fields.reference_current_price_available,
        realized_vol: option_evidence_number(fields.realized_vol),
        realized_vol_source_venue: fields.realized_vol_source_venue.clone(),
        realized_vol_source_ts_ms: fields.realized_vol_source_ts_ms,
        realized_vol_gate_result: Some(fields.realized_vol_gate_result),
        realized_vol_receive_watermark_ms: fields
            .realized_vol_receive_watermark_ms
            .map(LocalReceiveMs::value),
        realized_vol_snapshot: fields.realized_volatility_evidence.to_durable_snapshot(),
        fair_probability_up: option_evidence_number(fields.fair_probability_up),
        fair_probability_down: option_evidence_number(fields.fair_probability_down),
        selected_side: fields.selected_side.map(outcome_side_to_evidence),
        sized_notional: option_evidence_number(fields.sized_notional),
        sized_worst_case_ev_bps: option_evidence_number(fields.sized_worst_case_ev_bps),
        sized_edge_cents_per_share: option_evidence_number(fields.sized_edge_cents_per_share),
        theta_scaled_min_edge_bps: option_evidence_number(fields.theta_scaled_min_edge_bps),
        up_fee_bps: option_evidence_number(fields.up_fee_bps),
        down_fee_bps: option_evidence_number(fields.down_fee_bps),
        submission_blocked_reason: fields.submission_blocked_reason.or(Some(reason_category)),
        stale_reference_after_ms: forced_flat_inputs.stale_reference_after_ms,
        last_reference_ts_ms: forced_flat_inputs.last_reference_ts_ms,
        min_liquidity_required: forced_flat_inputs.min_liquidity_required,
        liquidity_available: forced_flat_inputs.liquidity_available,
        frozen: forced_flat_inputs.frozen,
        metadata_matches_selection: forced_flat_inputs.metadata_matches_selection,
        fast_venue_incoherent: forced_flat_inputs.fast_venue_incoherent,
    }
}

pub(super) fn entry_block_reason_to_evidence(
    reason: &EntryBlockReason,
) -> EvidenceEntryBlockReason {
    match reason {
        EntryBlockReason::PhaseNotActive => EvidenceEntryBlockReason::PhaseNotActive,
        EntryBlockReason::MetadataMismatch => EvidenceEntryBlockReason::MetadataMismatch,
        EntryBlockReason::ActiveBookNotPriced => EvidenceEntryBlockReason::ActiveBookNotPriced,
        EntryBlockReason::BookCrossed => EvidenceEntryBlockReason::BookCrossed,
        EntryBlockReason::IntervalOpenMissing => EvidenceEntryBlockReason::IntervalOpenMissing,
        EntryBlockReason::WarmupIncomplete => EvidenceEntryBlockReason::WarmupIncomplete,
        EntryBlockReason::FeesNotReady => EvidenceEntryBlockReason::FeesNotReady,
        EntryBlockReason::RecoveryMode => EvidenceEntryBlockReason::RecoveryMode,
        EntryBlockReason::MarketCoolingDown => EvidenceEntryBlockReason::MarketCoolingDown,
        EntryBlockReason::SpotSpikeCooldown => EvidenceEntryBlockReason::SpotSpikeCooldown,
        EntryBlockReason::ForcedFlat(reason) => {
            EvidenceEntryBlockReason::ForcedFlat(forced_flat_reason_to_evidence(reason))
        }
        EntryBlockReason::OnePositionInvariant(occupancy) => {
            EvidenceEntryBlockReason::OnePositionInvariant(exposure_occupancy_to_evidence(
                *occupancy,
            ))
        }
    }
}

fn binary_edge_block_reason_to_evidence(
    reason: BinaryOutcomeEdgeBlockReason,
) -> EvidenceBinaryOutcomeEdgeBlockReason {
    match reason {
        BinaryOutcomeEdgeBlockReason::MissingOrderBook => {
            EvidenceBinaryOutcomeEdgeBlockReason::MissingOrderBook
        }
        BinaryOutcomeEdgeBlockReason::InsufficientDepth => {
            EvidenceBinaryOutcomeEdgeBlockReason::InsufficientDepth
        }
        BinaryOutcomeEdgeBlockReason::InvalidProbability => {
            EvidenceBinaryOutcomeEdgeBlockReason::InvalidProbability
        }
        BinaryOutcomeEdgeBlockReason::InvalidCost => {
            EvidenceBinaryOutcomeEdgeBlockReason::InvalidCost
        }
        BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape => {
            EvidenceBinaryOutcomeEdgeBlockReason::UnsupportedOrderShape
        }
        BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold => {
            EvidenceBinaryOutcomeEdgeBlockReason::EdgeBelowThreshold
        }
        BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge => {
            EvidenceBinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge
        }
    }
}

pub(super) fn entry_pricing_block_reason_to_evidence(
    reason: &EntryPricingBlockReason,
) -> EvidenceEntryPricingBlockReason {
    match reason {
        EntryPricingBlockReason::SpotPriceMissing => {
            EvidenceEntryPricingBlockReason::SpotPriceMissing
        }
        EntryPricingBlockReason::ReferenceCurrentPriceStale => {
            EvidenceEntryPricingBlockReason::ReferenceCurrentPriceStale
        }
        EntryPricingBlockReason::StrikePriceMissing => {
            EvidenceEntryPricingBlockReason::StrikePriceMissing
        }
        EntryPricingBlockReason::SecondsToExpiryMissing => {
            EvidenceEntryPricingBlockReason::SecondsToExpiryMissing
        }
        EntryPricingBlockReason::RealizedVolNotReady => {
            EvidenceEntryPricingBlockReason::RealizedVolNotReady
        }
        EntryPricingBlockReason::ThetaScalerUnavailable => {
            EvidenceEntryPricingBlockReason::ThetaScalerUnavailable
        }
        EntryPricingBlockReason::UncertaintyBandUnavailable => {
            EvidenceEntryPricingBlockReason::UncertaintyBandUnavailable
        }
        EntryPricingBlockReason::FairProbabilityUnavailable => {
            EvidenceEntryPricingBlockReason::FairProbabilityUnavailable
        }
        EntryPricingBlockReason::ExecutableEntryCostUnavailable(side) => {
            EvidenceEntryPricingBlockReason::ExecutableEntryCostUnavailable(
                outcome_side_to_evidence(*side),
            )
        }
        EntryPricingBlockReason::ExecutableEdgeUnavailable(side, reason) => {
            EvidenceEntryPricingBlockReason::ExecutableEdgeUnavailable(
                outcome_side_to_evidence(*side),
                binary_edge_block_reason_to_evidence(*reason),
            )
        }
        EntryPricingBlockReason::SizedNotionalUnsupported(side) => {
            EvidenceEntryPricingBlockReason::SizedNotionalUnsupported(outcome_side_to_evidence(
                *side,
            ))
        }
    }
}

pub(super) const fn entry_skip_reason_label(reason: EvidenceEntrySkipReason) -> &'static str {
    match reason {
        EvidenceEntrySkipReason::StrategyCoreNotRegistered => "strategy_core_not_registered",
        EvidenceEntrySkipReason::EntryGateBlocked => "entry_gate_blocked",
        EvidenceEntrySkipReason::EntryPricingBlocked => "entry_pricing_blocked",
        EvidenceEntrySkipReason::NoSideSelected => "no_side_selected",
        EvidenceEntrySkipReason::SizedNotionalNotPositive => "sized_notional_not_positive",
        EvidenceEntrySkipReason::InstrumentIdMissing => "instrument_id_missing",
        EvidenceEntrySkipReason::InstrumentMissingFromCache => "instrument_missing_from_cache",
        EvidenceEntrySkipReason::EntryPriceMissing => "entry_price_missing",
        EvidenceEntrySkipReason::QuantityRoundingFailed => "quantity_rounding_failed",
        EvidenceEntrySkipReason::LimitNotionalExceedsSizedNotional => {
            "limit_notional_exceeds_sized_notional"
        }
        EvidenceEntrySkipReason::EntryQuoteNotionalBelowVenueMinimum => {
            "entry_quote_notional_below_venue_minimum"
        }
        EvidenceEntrySkipReason::EntryQuoteNotionalMinimumUnmodeled => {
            "entry_quote_notional_minimum_unmodeled"
        }
        EvidenceEntrySkipReason::QuantityNotPositive => "quantity_not_positive",
        EvidenceEntrySkipReason::PositionContractInvalid => "position_contract_invalid",
        EvidenceEntrySkipReason::EntryPositionContractUnsupported => {
            "entry_position_contract_unsupported"
        }
        EvidenceEntrySkipReason::HistoricalEntryFeeUnavailable => {
            "historical_entry_fee_unavailable"
        }
        EvidenceEntrySkipReason::OnePositionInvariantViolation => {
            "one_position_invariant_violation"
        }
        EvidenceEntrySkipReason::EntryMalformedRejected => "entry_malformed_rejected",
        EvidenceEntrySkipReason::EntryBalanceRejected => "entry_balance_rejected",
        EvidenceEntrySkipReason::EntryUnfillableRejectedUnchangedBook => {
            "entry_unfillable_rejected_unchanged_book"
        }
    }
}
