use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{decode, encode_line, validate_envelope, validate_recorded_at};
use crate::bolt_v3_current_evidence::{
    facts::{
        BinaryOutcomeEdgeBlockReason, EntryBlockReason, EntryPricingBlockReason,
        EntryRealizedVolatilitySnapshotFact, EntrySkipFact, EntrySkipReason, ExposureOccupancy,
        ForcedFlatReason, OutcomeSide, RealizedVolAggregation, RealizedVolBlockReason,
        RealizedVolPricingComponent, RealizedVolSampleKind, RealizedVolSourceClass,
        RealizedVolSourceRejectReason, RealizedVolSourceStatus,
        RealizedVolatilitySourceDiagnosticFact, RvGateResult,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntrySkipV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    entry_skip: EntrySkipV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntrySkipV1Wire {
    strategy_id: String,
    now_ms: u64,
    reason_category: EntrySkipReasonV1,
    gate_blocked_by: Vec<EntryBlockReasonV1>,
    pricing_blocked_by: Vec<EntryPricingBlockReasonV1>,
    market_id: Option<String>,
    phase: String,
    seconds_to_market_end: Option<u64>,
    spot_price: Option<String>,
    reference_current_price: Option<String>,
    fast_venue_available: bool,
    reference_current_price_available: bool,
    realized_vol: Option<String>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    realized_vol_gate_result: Option<RvGateResultV1>,
    realized_vol_receive_watermark_ms: Option<u64>,
    realized_vol_snapshot: Option<EntryRealizedVolatilitySnapshotV1Wire>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    selected_side: Option<OutcomeSideV1>,
    sized_notional: Option<String>,
    sized_worst_case_ev_bps: Option<String>,
    sized_edge_cents_per_share: Option<String>,
    theta_scaled_min_edge_bps: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    submission_blocked_reason: Option<EntrySkipReasonV1>,
    stale_reference_after_ms: Option<u64>,
    last_reference_ts_ms: Option<u64>,
    min_liquidity_required: Option<String>,
    liquidity_available: Option<String>,
    frozen: bool,
    metadata_matches_selection: bool,
    fast_venue_incoherent: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EntryRealizedVolatilitySnapshotV1Wire {
    surface_id: String,
    as_of_ms: Option<u64>,
    annualized_decimal: String,
    measured_annualized_decimal: String,
    noise_robust_annualized_decimal: String,
    continuous_annualized_decimal: String,
    jump_annualized_decimal: String,
    forecast_annualized_decimal: String,
    pricing_component: RealizedVolPricingComponentV1,
    seconds_per_annum: String,
    aggregation: RealizedVolAggregationV1,
    sources_used: Vec<String>,
    source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticV1Wire>,
    unknown_source_rejections: BTreeMap<String, u64>,
    blockers: Vec<RealizedVolBlockReasonV1>,
    config_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RealizedVolatilitySourceDiagnosticV1Wire {
    source_id: String,
    source_class: RealizedVolSourceClassV1,
    sample_kind: RealizedVolSampleKindV1,
    enabled: bool,
    counts_toward_quorum: bool,
    status: RealizedVolSourceStatusV1,
    annualized_realized_volatility_decimal: Option<String>,
    measured_annualized_realized_volatility_decimal: Option<String>,
    noise_robust_annualized_realized_volatility_decimal: Option<String>,
    continuous_annualized_realized_volatility_decimal: Option<String>,
    jump_annualized_realized_volatility_decimal: Option<String>,
    first_sample_ts_ms: Option<u64>,
    last_sample_ts_ms: Option<u64>,
    raw_sample_count: usize,
    grid_sample_count: usize,
    coverage_ratio: String,
    max_inter_sample_gap_ms: Option<u64>,
    last_rejected_reason: Option<RealizedVolSourceRejectReasonV1>,
    last_rejected_event_ts_ms: Option<u64>,
    last_rejected_recv_ts_ms: Option<u64>,
    rejection_counters: BTreeMap<RealizedVolSourceRejectReasonV1, u64>,
    block_reason: Option<RealizedVolBlockReasonV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntrySkipReasonV1 {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum OutcomeSideV1 {
    Up,
    Down,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ForcedFlatReasonV1 {
    Freeze,
    StaleReference,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExposureOccupancyV1 {
    PendingEntry,
    EntryReconcilePending,
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EntryBlockReasonV1 {
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
    ForcedFlat(ForcedFlatReasonV1),
    OnePositionInvariant(ExposureOccupancyV1),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BinaryOutcomeEdgeBlockReasonV1 {
    MissingOrderBook,
    InsufficientDepth,
    InvalidProbability,
    InvalidCost,
    UnsupportedOrderShape,
    EdgeBelowThreshold,
    SpreadOrSlippageWipedEdge,
    FeeUnavailable,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum EntryPricingBlockReasonV1 {
    SpotPriceMissing,
    ReferenceCurrentPriceStale,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    ThetaScalerUnavailable,
    UncertaintyBandUnavailable,
    FairProbabilityUnavailable,
    FeeUnavailable(OutcomeSideV1),
    ExecutableEntryCostUnavailable(OutcomeSideV1),
    ExecutableEdgeUnavailable(OutcomeSideV1, BinaryOutcomeEdgeBlockReasonV1),
    SizedNotionalUnsupported(OutcomeSideV1),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RvGateResultV1 {
    Accepted,
    MissingSnapshot,
    MissingEvaluationEventTime,
    RejectedFutureDated,
    RejectedStale,
    RejectedNotReady,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolPricingComponentV1 {
    Measured,
    NoiseRobust,
    Continuous,
    Forecast,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolAggregationV1 {
    UpperQuantile,
    Median,
    TrimmedMean,
    MedianWithUpperQuantileGuard,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolSourceClassV1 {
    SpotQuote,
    Trade,
    Mark,
    Index,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolSampleKindV1 {
    Midpoint,
    Trade,
    Mark,
    Index,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolSourceStatusV1 {
    Ready,
    Blocked,
    DiagnosticOnly,
    Waiting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolSourceRejectReasonV1 {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RealizedVolBlockReasonV1 {
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

pub(super) fn encode(
    fact: EntrySkipFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::EntrySkipObservation;
    let descriptor = super::current_line_descriptor(purpose);
    let entry_skip = EntrySkipV1Wire::try_from(&fact).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &EntrySkipV1Line {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            entry_skip,
        },
    )
}

pub(super) fn decode_fact(line: &str, line_number: usize) -> Result<EntrySkipFact> {
    let decoded: EntrySkipV1Line = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::EntrySkipObservationV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    EntrySkipFact::try_from(decoded.entry_skip)
}

fn required_text(value: &str, field: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be non-empty and canonical"
    );
    Ok(value.to_string())
}

impl TryFrom<&EntrySkipFact> for EntrySkipV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &EntrySkipFact) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            now_ms: positive(value.now_ms, "now_ms")?,
            reason_category: value.reason_category.into(),
            gate_blocked_by: value
                .gate_blocked_by
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
            pricing_blocked_by: value
                .pricing_blocked_by
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
            market_id: optional_text(&value.market_id, "market_id")?,
            phase: required_text(&value.phase, "phase")?,
            seconds_to_market_end: value.seconds_to_market_end,
            spot_price: optional_number(&value.spot_price, "spot_price")?,
            reference_current_price: optional_number(
                &value.reference_current_price,
                "reference_current_price",
            )?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price_available: value.reference_current_price_available,
            realized_vol: optional_number(&value.realized_vol, "realized_vol")?,
            realized_vol_source_venue: optional_text(
                &value.realized_vol_source_venue,
                "realized_vol_source_venue",
            )?,
            realized_vol_source_ts_ms: value.realized_vol_source_ts_ms,
            realized_vol_gate_result: value.realized_vol_gate_result.map(Into::into),
            realized_vol_receive_watermark_ms: value.realized_vol_receive_watermark_ms,
            realized_vol_snapshot: value
                .realized_vol_snapshot
                .as_ref()
                .map(EntryRealizedVolatilitySnapshotV1Wire::try_from)
                .transpose()?,
            fair_probability_up: optional_number(
                &value.fair_probability_up,
                "fair_probability_up",
            )?,
            fair_probability_down: optional_number(
                &value.fair_probability_down,
                "fair_probability_down",
            )?,
            selected_side: value.selected_side.map(Into::into),
            sized_notional: optional_number(&value.sized_notional, "sized_notional")?,
            sized_worst_case_ev_bps: optional_number(
                &value.sized_worst_case_ev_bps,
                "sized_worst_case_ev_bps",
            )?,
            sized_edge_cents_per_share: optional_number(
                &value.sized_edge_cents_per_share,
                "sized_edge_cents_per_share",
            )?,
            theta_scaled_min_edge_bps: optional_number(
                &value.theta_scaled_min_edge_bps,
                "theta_scaled_min_edge_bps",
            )?,
            up_fee_bps: optional_number(&value.up_fee_bps, "up_fee_bps")?,
            down_fee_bps: optional_number(&value.down_fee_bps, "down_fee_bps")?,
            submission_blocked_reason: value.submission_blocked_reason.map(Into::into),
            stale_reference_after_ms: value.stale_reference_after_ms,
            last_reference_ts_ms: value.last_reference_ts_ms,
            min_liquidity_required: optional_number(
                &value.min_liquidity_required,
                "min_liquidity_required",
            )?,
            liquidity_available: optional_number(
                &value.liquidity_available,
                "liquidity_available",
            )?,
            frozen: value.frozen,
            metadata_matches_selection: value.metadata_matches_selection,
            fast_venue_incoherent: value.fast_venue_incoherent,
        })
    }
}

impl TryFrom<EntrySkipV1Wire> for EntrySkipFact {
    type Error = anyhow::Error;

    fn try_from(value: EntrySkipV1Wire) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            now_ms: positive(value.now_ms, "now_ms")?,
            reason_category: value.reason_category.into(),
            gate_blocked_by: value.gate_blocked_by.into_iter().map(Into::into).collect(),
            pricing_blocked_by: value
                .pricing_blocked_by
                .into_iter()
                .map(Into::into)
                .collect(),
            market_id: optional_text(&value.market_id, "market_id")?,
            phase: required_text(&value.phase, "phase")?,
            seconds_to_market_end: value.seconds_to_market_end,
            spot_price: optional_number(&value.spot_price, "spot_price")?,
            reference_current_price: optional_number(
                &value.reference_current_price,
                "reference_current_price",
            )?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price_available: value.reference_current_price_available,
            realized_vol: optional_number(&value.realized_vol, "realized_vol")?,
            realized_vol_source_venue: optional_text(
                &value.realized_vol_source_venue,
                "realized_vol_source_venue",
            )?,
            realized_vol_source_ts_ms: value.realized_vol_source_ts_ms,
            realized_vol_gate_result: value.realized_vol_gate_result.map(Into::into),
            realized_vol_receive_watermark_ms: value.realized_vol_receive_watermark_ms,
            realized_vol_snapshot: value
                .realized_vol_snapshot
                .map(EntryRealizedVolatilitySnapshotFact::try_from)
                .transpose()?,
            fair_probability_up: optional_number(
                &value.fair_probability_up,
                "fair_probability_up",
            )?,
            fair_probability_down: optional_number(
                &value.fair_probability_down,
                "fair_probability_down",
            )?,
            selected_side: value.selected_side.map(Into::into),
            sized_notional: optional_number(&value.sized_notional, "sized_notional")?,
            sized_worst_case_ev_bps: optional_number(
                &value.sized_worst_case_ev_bps,
                "sized_worst_case_ev_bps",
            )?,
            sized_edge_cents_per_share: optional_number(
                &value.sized_edge_cents_per_share,
                "sized_edge_cents_per_share",
            )?,
            theta_scaled_min_edge_bps: optional_number(
                &value.theta_scaled_min_edge_bps,
                "theta_scaled_min_edge_bps",
            )?,
            up_fee_bps: optional_number(&value.up_fee_bps, "up_fee_bps")?,
            down_fee_bps: optional_number(&value.down_fee_bps, "down_fee_bps")?,
            submission_blocked_reason: value.submission_blocked_reason.map(Into::into),
            stale_reference_after_ms: value.stale_reference_after_ms,
            last_reference_ts_ms: value.last_reference_ts_ms,
            min_liquidity_required: optional_number(
                &value.min_liquidity_required,
                "min_liquidity_required",
            )?,
            liquidity_available: optional_number(
                &value.liquidity_available,
                "liquidity_available",
            )?,
            frozen: value.frozen,
            metadata_matches_selection: value.metadata_matches_selection,
            fast_venue_incoherent: value.fast_venue_incoherent,
        })
    }
}

impl TryFrom<&EntryRealizedVolatilitySnapshotFact> for EntryRealizedVolatilitySnapshotV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &EntryRealizedVolatilitySnapshotFact) -> Result<Self> {
        Ok(Self {
            surface_id: required_text(&value.surface_id, "rv.surface_id")?,
            as_of_ms: value.as_of_ms,
            annualized_decimal: required_number(
                &value.annualized_decimal,
                "rv.annualized_decimal",
            )?,
            measured_annualized_decimal: required_number(
                &value.measured_annualized_decimal,
                "rv.measured_annualized_decimal",
            )?,
            noise_robust_annualized_decimal: required_number(
                &value.noise_robust_annualized_decimal,
                "rv.noise_robust_annualized_decimal",
            )?,
            continuous_annualized_decimal: required_number(
                &value.continuous_annualized_decimal,
                "rv.continuous_annualized_decimal",
            )?,
            jump_annualized_decimal: required_number(
                &value.jump_annualized_decimal,
                "rv.jump_annualized_decimal",
            )?,
            forecast_annualized_decimal: required_number(
                &value.forecast_annualized_decimal,
                "rv.forecast_annualized_decimal",
            )?,
            pricing_component: value.pricing_component.into(),
            seconds_per_annum: required_number(&value.seconds_per_annum, "rv.seconds_per_annum")?,
            aggregation: value.aggregation.into(),
            sources_used: canonical_texts(&value.sources_used, "rv.sources_used")?,
            source_diagnostics: value
                .source_diagnostics
                .iter()
                .map(RealizedVolatilitySourceDiagnosticV1Wire::try_from)
                .collect::<Result<_>>()?,
            unknown_source_rejections: canonical_text_map(
                &value.unknown_source_rejections,
                "rv.unknown_source_rejections",
            )?,
            blockers: value.blockers.iter().copied().map(Into::into).collect(),
            config_fingerprint: required_text(&value.config_fingerprint, "rv.config_fingerprint")?,
        })
    }
}

impl TryFrom<EntryRealizedVolatilitySnapshotV1Wire> for EntryRealizedVolatilitySnapshotFact {
    type Error = anyhow::Error;

    fn try_from(value: EntryRealizedVolatilitySnapshotV1Wire) -> Result<Self> {
        Ok(Self {
            surface_id: required_text(&value.surface_id, "rv.surface_id")?,
            as_of_ms: value.as_of_ms,
            annualized_decimal: required_number(
                &value.annualized_decimal,
                "rv.annualized_decimal",
            )?,
            measured_annualized_decimal: required_number(
                &value.measured_annualized_decimal,
                "rv.measured_annualized_decimal",
            )?,
            noise_robust_annualized_decimal: required_number(
                &value.noise_robust_annualized_decimal,
                "rv.noise_robust_annualized_decimal",
            )?,
            continuous_annualized_decimal: required_number(
                &value.continuous_annualized_decimal,
                "rv.continuous_annualized_decimal",
            )?,
            jump_annualized_decimal: required_number(
                &value.jump_annualized_decimal,
                "rv.jump_annualized_decimal",
            )?,
            forecast_annualized_decimal: required_number(
                &value.forecast_annualized_decimal,
                "rv.forecast_annualized_decimal",
            )?,
            pricing_component: value.pricing_component.into(),
            seconds_per_annum: required_number(&value.seconds_per_annum, "rv.seconds_per_annum")?,
            aggregation: value.aggregation.into(),
            sources_used: canonical_texts(&value.sources_used, "rv.sources_used")?,
            source_diagnostics: value
                .source_diagnostics
                .into_iter()
                .map(RealizedVolatilitySourceDiagnosticFact::try_from)
                .collect::<Result<_>>()?,
            unknown_source_rejections: canonical_text_map(
                &value.unknown_source_rejections,
                "rv.unknown_source_rejections",
            )?,
            blockers: value.blockers.into_iter().map(Into::into).collect(),
            config_fingerprint: required_text(&value.config_fingerprint, "rv.config_fingerprint")?,
        })
    }
}

impl TryFrom<&RealizedVolatilitySourceDiagnosticFact> for RealizedVolatilitySourceDiagnosticV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &RealizedVolatilitySourceDiagnosticFact) -> Result<Self> {
        Ok(Self {
            source_id: required_text(&value.source_id, "rv.source.source_id")?,
            source_class: value.source_class.into(),
            sample_kind: value.sample_kind.into(),
            enabled: value.enabled,
            counts_toward_quorum: value.counts_toward_quorum,
            status: value.status.into(),
            annualized_realized_volatility_decimal: optional_number(
                &value.annualized_realized_volatility_decimal,
                "rv.source.annualized_realized_volatility_decimal",
            )?,
            measured_annualized_realized_volatility_decimal: optional_number(
                &value.measured_annualized_realized_volatility_decimal,
                "rv.source.measured_annualized_realized_volatility_decimal",
            )?,
            noise_robust_annualized_realized_volatility_decimal: optional_number(
                &value.noise_robust_annualized_realized_volatility_decimal,
                "rv.source.noise_robust_annualized_realized_volatility_decimal",
            )?,
            continuous_annualized_realized_volatility_decimal: optional_number(
                &value.continuous_annualized_realized_volatility_decimal,
                "rv.source.continuous_annualized_realized_volatility_decimal",
            )?,
            jump_annualized_realized_volatility_decimal: optional_number(
                &value.jump_annualized_realized_volatility_decimal,
                "rv.source.jump_annualized_realized_volatility_decimal",
            )?,
            first_sample_ts_ms: value.first_sample_ts_ms,
            last_sample_ts_ms: value.last_sample_ts_ms,
            raw_sample_count: value.raw_sample_count,
            grid_sample_count: value.grid_sample_count,
            coverage_ratio: required_number(&value.coverage_ratio, "rv.source.coverage_ratio")?,
            max_inter_sample_gap_ms: value.max_inter_sample_gap_ms,
            last_rejected_reason: value.last_rejected_reason.map(Into::into),
            last_rejected_event_ts_ms: value.last_rejected_event_ts_ms,
            last_rejected_recv_ts_ms: value.last_rejected_recv_ts_ms,
            rejection_counters: value
                .rejection_counters
                .iter()
                .map(|(reason, count)| ((*reason).into(), *count))
                .collect(),
            block_reason: value.block_reason.map(Into::into),
        })
    }
}

impl TryFrom<RealizedVolatilitySourceDiagnosticV1Wire> for RealizedVolatilitySourceDiagnosticFact {
    type Error = anyhow::Error;

    fn try_from(value: RealizedVolatilitySourceDiagnosticV1Wire) -> Result<Self> {
        Ok(Self {
            source_id: required_text(&value.source_id, "rv.source.source_id")?,
            source_class: value.source_class.into(),
            sample_kind: value.sample_kind.into(),
            enabled: value.enabled,
            counts_toward_quorum: value.counts_toward_quorum,
            status: value.status.into(),
            annualized_realized_volatility_decimal: optional_number(
                &value.annualized_realized_volatility_decimal,
                "rv.source.annualized_realized_volatility_decimal",
            )?,
            measured_annualized_realized_volatility_decimal: optional_number(
                &value.measured_annualized_realized_volatility_decimal,
                "rv.source.measured_annualized_realized_volatility_decimal",
            )?,
            noise_robust_annualized_realized_volatility_decimal: optional_number(
                &value.noise_robust_annualized_realized_volatility_decimal,
                "rv.source.noise_robust_annualized_realized_volatility_decimal",
            )?,
            continuous_annualized_realized_volatility_decimal: optional_number(
                &value.continuous_annualized_realized_volatility_decimal,
                "rv.source.continuous_annualized_realized_volatility_decimal",
            )?,
            jump_annualized_realized_volatility_decimal: optional_number(
                &value.jump_annualized_realized_volatility_decimal,
                "rv.source.jump_annualized_realized_volatility_decimal",
            )?,
            first_sample_ts_ms: value.first_sample_ts_ms,
            last_sample_ts_ms: value.last_sample_ts_ms,
            raw_sample_count: value.raw_sample_count,
            grid_sample_count: value.grid_sample_count,
            coverage_ratio: required_number(&value.coverage_ratio, "rv.source.coverage_ratio")?,
            max_inter_sample_gap_ms: value.max_inter_sample_gap_ms,
            last_rejected_reason: value.last_rejected_reason.map(Into::into),
            last_rejected_event_ts_ms: value.last_rejected_event_ts_ms,
            last_rejected_recv_ts_ms: value.last_rejected_recv_ts_ms,
            rejection_counters: value
                .rejection_counters
                .into_iter()
                .map(|(reason, count)| (reason.into(), count))
                .collect(),
            block_reason: value.block_reason.map(Into::into),
        })
    }
}

macro_rules! bidirectional_unit_enum {
    ($semantic:ty, $wire:ty, [$($variant:ident),+ $(,)?]) => {
        impl From<$semantic> for $wire {
            fn from(value: $semantic) -> Self {
                match value {
                    $(<$semantic>::$variant => Self::$variant,)+
                }
            }
        }

        impl From<$wire> for $semantic {
            fn from(value: $wire) -> Self {
                match value {
                    $(<$wire>::$variant => Self::$variant,)+
                }
            }
        }
    };
}

bidirectional_unit_enum!(
    EntrySkipReason,
    EntrySkipReasonV1,
    [
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
    ]
);
bidirectional_unit_enum!(OutcomeSide, OutcomeSideV1, [Up, Down]);
bidirectional_unit_enum!(
    ForcedFlatReason,
    ForcedFlatReasonV1,
    [
        Freeze,
        StaleReference,
        ThinBook,
        MetadataMismatch,
        FastVenueIncoherent,
    ]
);
bidirectional_unit_enum!(
    ExposureOccupancy,
    ExposureOccupancyV1,
    [
        PendingEntry,
        EntryReconcilePending,
        ManagedPosition,
        ExitPending,
        UnsupportedObserved,
        BlindRecovery,
    ]
);
bidirectional_unit_enum!(
    BinaryOutcomeEdgeBlockReason,
    BinaryOutcomeEdgeBlockReasonV1,
    [
        MissingOrderBook,
        InsufficientDepth,
        InvalidProbability,
        InvalidCost,
        UnsupportedOrderShape,
        EdgeBelowThreshold,
        SpreadOrSlippageWipedEdge,
        FeeUnavailable,
    ]
);
bidirectional_unit_enum!(
    RvGateResult,
    RvGateResultV1,
    [
        Accepted,
        MissingSnapshot,
        MissingEvaluationEventTime,
        RejectedFutureDated,
        RejectedStale,
        RejectedNotReady,
    ]
);
bidirectional_unit_enum!(
    RealizedVolPricingComponent,
    RealizedVolPricingComponentV1,
    [Measured, NoiseRobust, Continuous, Forecast,]
);
bidirectional_unit_enum!(
    RealizedVolAggregation,
    RealizedVolAggregationV1,
    [
        UpperQuantile,
        Median,
        TrimmedMean,
        MedianWithUpperQuantileGuard,
    ]
);
bidirectional_unit_enum!(
    RealizedVolSourceClass,
    RealizedVolSourceClassV1,
    [SpotQuote, Trade, Mark, Index,]
);
bidirectional_unit_enum!(
    RealizedVolSampleKind,
    RealizedVolSampleKindV1,
    [Midpoint, Trade, Mark, Index,]
);
bidirectional_unit_enum!(
    RealizedVolSourceStatus,
    RealizedVolSourceStatusV1,
    [Ready, Blocked, DiagnosticOnly, Waiting,]
);
bidirectional_unit_enum!(
    RealizedVolSourceRejectReason,
    RealizedVolSourceRejectReasonV1,
    [
        DisabledSource,
        InvalidPrice,
        SourceClassMismatch,
        SampleKindMismatch,
        EventTimeRegression,
        DuplicateTimestamp,
        StaleSameEventUpdate,
        ReceiveBeforeEvent,
        EventReceiveLagExceeded,
    ]
);
bidirectional_unit_enum!(
    RealizedVolBlockReason,
    RealizedVolBlockReasonV1,
    [
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
    ]
);

impl From<EntryBlockReason> for EntryBlockReasonV1 {
    fn from(value: EntryBlockReason) -> Self {
        match value {
            EntryBlockReason::PhaseNotActive => Self::PhaseNotActive,
            EntryBlockReason::MetadataMismatch => Self::MetadataMismatch,
            EntryBlockReason::ActiveBookNotPriced => Self::ActiveBookNotPriced,
            EntryBlockReason::BookCrossed => Self::BookCrossed,
            EntryBlockReason::IntervalOpenMissing => Self::IntervalOpenMissing,
            EntryBlockReason::WarmupIncomplete => Self::WarmupIncomplete,
            EntryBlockReason::FeesNotReady => Self::FeesNotReady,
            EntryBlockReason::RecoveryMode => Self::RecoveryMode,
            EntryBlockReason::MarketCoolingDown => Self::MarketCoolingDown,
            EntryBlockReason::SpotSpikeCooldown => Self::SpotSpikeCooldown,
            EntryBlockReason::ForcedFlat(reason) => Self::ForcedFlat(reason.into()),
            EntryBlockReason::OnePositionInvariant(occupancy) => {
                Self::OnePositionInvariant(occupancy.into())
            }
        }
    }
}

impl From<EntryBlockReasonV1> for EntryBlockReason {
    fn from(value: EntryBlockReasonV1) -> Self {
        match value {
            EntryBlockReasonV1::PhaseNotActive => Self::PhaseNotActive,
            EntryBlockReasonV1::MetadataMismatch => Self::MetadataMismatch,
            EntryBlockReasonV1::ActiveBookNotPriced => Self::ActiveBookNotPriced,
            EntryBlockReasonV1::BookCrossed => Self::BookCrossed,
            EntryBlockReasonV1::IntervalOpenMissing => Self::IntervalOpenMissing,
            EntryBlockReasonV1::WarmupIncomplete => Self::WarmupIncomplete,
            EntryBlockReasonV1::FeesNotReady => Self::FeesNotReady,
            EntryBlockReasonV1::RecoveryMode => Self::RecoveryMode,
            EntryBlockReasonV1::MarketCoolingDown => Self::MarketCoolingDown,
            EntryBlockReasonV1::SpotSpikeCooldown => Self::SpotSpikeCooldown,
            EntryBlockReasonV1::ForcedFlat(reason) => Self::ForcedFlat(reason.into()),
            EntryBlockReasonV1::OnePositionInvariant(occupancy) => {
                Self::OnePositionInvariant(occupancy.into())
            }
        }
    }
}

impl From<EntryPricingBlockReason> for EntryPricingBlockReasonV1 {
    fn from(value: EntryPricingBlockReason) -> Self {
        match value {
            EntryPricingBlockReason::SpotPriceMissing => Self::SpotPriceMissing,
            EntryPricingBlockReason::ReferenceCurrentPriceStale => Self::ReferenceCurrentPriceStale,
            EntryPricingBlockReason::StrikePriceMissing => Self::StrikePriceMissing,
            EntryPricingBlockReason::SecondsToExpiryMissing => Self::SecondsToExpiryMissing,
            EntryPricingBlockReason::RealizedVolNotReady => Self::RealizedVolNotReady,
            EntryPricingBlockReason::ThetaScalerUnavailable => Self::ThetaScalerUnavailable,
            EntryPricingBlockReason::UncertaintyBandUnavailable => Self::UncertaintyBandUnavailable,
            EntryPricingBlockReason::FairProbabilityUnavailable => Self::FairProbabilityUnavailable,
            EntryPricingBlockReason::FeeUnavailable(side) => Self::FeeUnavailable(side.into()),
            EntryPricingBlockReason::ExecutableEntryCostUnavailable(side) => {
                Self::ExecutableEntryCostUnavailable(side.into())
            }
            EntryPricingBlockReason::ExecutableEdgeUnavailable(side, reason) => {
                Self::ExecutableEdgeUnavailable(side.into(), reason.into())
            }
            EntryPricingBlockReason::SizedNotionalUnsupported(side) => {
                Self::SizedNotionalUnsupported(side.into())
            }
        }
    }
}

impl From<EntryPricingBlockReasonV1> for EntryPricingBlockReason {
    fn from(value: EntryPricingBlockReasonV1) -> Self {
        match value {
            EntryPricingBlockReasonV1::SpotPriceMissing => Self::SpotPriceMissing,
            EntryPricingBlockReasonV1::ReferenceCurrentPriceStale => {
                Self::ReferenceCurrentPriceStale
            }
            EntryPricingBlockReasonV1::StrikePriceMissing => Self::StrikePriceMissing,
            EntryPricingBlockReasonV1::SecondsToExpiryMissing => Self::SecondsToExpiryMissing,
            EntryPricingBlockReasonV1::RealizedVolNotReady => Self::RealizedVolNotReady,
            EntryPricingBlockReasonV1::ThetaScalerUnavailable => Self::ThetaScalerUnavailable,
            EntryPricingBlockReasonV1::UncertaintyBandUnavailable => {
                Self::UncertaintyBandUnavailable
            }
            EntryPricingBlockReasonV1::FairProbabilityUnavailable => {
                Self::FairProbabilityUnavailable
            }
            EntryPricingBlockReasonV1::FeeUnavailable(side) => Self::FeeUnavailable(side.into()),
            EntryPricingBlockReasonV1::ExecutableEntryCostUnavailable(side) => {
                Self::ExecutableEntryCostUnavailable(side.into())
            }
            EntryPricingBlockReasonV1::ExecutableEdgeUnavailable(side, reason) => {
                Self::ExecutableEdgeUnavailable(side.into(), reason.into())
            }
            EntryPricingBlockReasonV1::SizedNotionalUnsupported(side) => {
                Self::SizedNotionalUnsupported(side.into())
            }
        }
    }
}

fn positive(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}

fn optional_text(value: &Option<String>, field: &str) -> Result<Option<String>> {
    value
        .as_deref()
        .map(|value| required_text(value, field))
        .transpose()
}

fn required_number(value: &str, field: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be non-empty and canonical"
    );
    let parsed = value
        .parse::<f64>()
        .with_context(|| format!("`{field}` must parse as a number"))?;
    ensure!(parsed.is_finite(), "`{field}` must be finite");
    Ok(value.to_string())
}

fn optional_number(value: &Option<String>, field: &str) -> Result<Option<String>> {
    value
        .as_deref()
        .map(|value| required_number(value, field))
        .transpose()
}

fn canonical_texts(values: &[String], field: &str) -> Result<Vec<String>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| required_text(value, &format!("{field}[{index}]")))
        .collect()
}

fn canonical_text_map(
    values: &BTreeMap<String, u64>,
    field: &str,
) -> Result<BTreeMap<String, u64>> {
    values
        .iter()
        .map(|(key, value)| Ok((required_text(key, &format!("{field}.key"))?, *value)))
        .collect()
}
