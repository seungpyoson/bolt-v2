use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    decode, encode_line,
    entry_skip::{
        ForcedFlatReasonV1, OutcomeSideV1, RealizedVolBlockReasonV1,
        RealizedVolatilitySourceDiagnosticV1Wire, RvGateResultV1,
    },
    strategy_input::SubmissionLinkageWireV1,
    validate_envelope, validate_recorded_at,
};
use crate::bolt_v3_current_evidence::{
    facts::{
        ExitAttemptOutcome, ExitBlockedReason, ExitDecisionDetails, ExitEvaluationFact,
        ExitHoldDecisionFact, ExitHoldOutcome, ExitIntentDecisionFact, ExitIntentOutcome,
        ExitPreparationStage, ExitPreparedOrderFact, ExitTriggerSource, PreparedOrderLinkage,
        SubmissionLinkage, SubmittedOrderLinkage,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitPreparedOrderLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_prepared_order: ExitPreparedOrderWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitIntentLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_intent: ExitIntentWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitHoldLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_decision: ExitHoldWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitEvaluationLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_evaluation: ExitEvaluationWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitPreparedOrderWireV1 {
    details: ExitDecisionDetailsWireV1,
    outcome: ExitIntentOutcomeV1,
    prepared_order: SubmissionLinkageWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitIntentWireV1 {
    details: ExitDecisionDetailsWireV1,
    outcome: ExitIntentOutcomeV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitHoldWireV1 {
    details: ExitDecisionDetailsWireV1,
    outcome: ExitHoldOutcomeV1,
    blocked_reason: Option<ExitBlockedReasonV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitDecisionDetailsWireV1 {
    strategy_id: String,
    market_id: Option<String>,
    position_id: Option<String>,
    position_instrument_id: Option<String>,
    position_outcome_side: Option<OutcomeSideV1>,
    forced_flat_reasons: Vec<ForcedFlatReasonV1>,
    spot_price: Option<String>,
    spot_venue_name: Option<String>,
    fast_venue_available: bool,
    reference_current_price: Option<String>,
    reference_current_price_available: bool,
    interval_open: Option<String>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    uncertainty_band_probability: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    realized_vol: Option<String>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    exit_eval_now_ms: u64,
    exit_trigger_source: ExitTriggerSourceV1,
    trigger_ts_event_ms: u64,
    trigger_ts_init_ms: Option<u64>,
    rv_surface_id: String,
    rv_snapshot_as_of_ms: Option<u64>,
    rv_snapshot_ready: bool,
    rv_snapshot_has_ready_realized_vol: Option<bool>,
    rv_snapshot_receive_watermark_ms: Option<u64>,
    rv_max_source_age_ms: Option<u64>,
    rv_snapshot_blockers: Vec<RealizedVolBlockReasonV1>,
    rv_source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticV1Wire>,
    rv_gate_result: RvGateResultV1,
    rv_future_dating_delta_ms: Option<u64>,
    exit_hysteresis_bps: String,
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitEvaluationWireV1 {
    position_id: Option<String>,
    market_id: Option<String>,
    instrument_id: Option<String>,
    exit_eval_now_ms: i64,
    exit_trigger_source: ExitTriggerSourceV1,
    trigger_ts_event_ms: Option<i64>,
    trigger_ts_init_ms: Option<i64>,
    rv_surface_id: String,
    rv_as_of_ms: Option<i64>,
    rv_ready: bool,
    rv_snapshot_receive_watermark_ms: Option<i64>,
    rv_max_source_age_ms: Option<u64>,
    rv_blockers: Vec<RealizedVolBlockReasonV1>,
    rv_source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticV1Wire>,
    rv_gate_result: RvGateResultV1,
    rv_as_of_minus_now_ms: Option<i64>,
    spot_price: Option<String>,
    spot_venue_name: Option<String>,
    fast_venue_available: bool,
    reference_current_price: Option<String>,
    reference_current_price_available: bool,
    interval_open: Option<String>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    uncertainty_band_probability: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    outcome: ExitAttemptOutcomeV1,
    forced_flat_reasons: Vec<ForcedFlatReasonV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitIntentOutcomeV1 {
    Exit,
    ExitFailClosed,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitHoldOutcomeV1 {
    Hold,
    Blocked,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitBlockedReasonV1 {
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitTriggerSourceV1 {
    SignalQuote,
    ReferenceUpdate,
    SelectionUpdate,
    BookDelta,
    Unknown,
    Other,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitPreparationStageV1 {
    OrderTemplate,
    InstrumentAuthority,
    PositionAuthority,
    ExecutableLiquidity,
    EconomicsSeal,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ExitAttemptOutcomeV1 {
    Held {
        outcome: ExitHoldOutcomeV1,
    },
    Blocked {
        blocked_reason: ExitBlockedReasonV1,
    },
    PreparationRejected {
        stage: ExitPreparationStageV1,
        reason: String,
    },
    RouteRejected {
        prepared_order: SubmissionLinkageWireV1,
        reason: String,
    },
    IntentEvidenceRejected {
        prepared_order: SubmissionLinkageWireV1,
        reason: String,
    },
    AdmissionRejected {
        prepared_order: SubmissionLinkageWireV1,
        reason: String,
    },
    PolicySkipped {
        prepared_order: SubmissionLinkageWireV1,
    },
    PreSinkRejected {
        prepared_order: SubmissionLinkageWireV1,
        reason: String,
    },
    SinkRejected {
        prepared_order: SubmissionLinkageWireV1,
        reason: String,
    },
    Submitted {
        submitted_order: SubmissionLinkageWireV1,
    },
}

pub(super) fn encode_intent(
    fact: ExitIntentDecisionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::ExitIntentDecision;
    let descriptor = super::current_line_descriptor(purpose);
    let wire = ExitIntentWireV1::try_from(fact).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &ExitIntentLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            exit_intent: wire,
        },
    )
}

pub(super) fn decode_intent(line: &str, line_number: usize) -> Result<ExitIntentDecisionFact> {
    let decoded: ExitIntentLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ExitIntentDecisionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    decoded.exit_intent.try_into()
}

pub(super) fn encode_prepared_order(
    fact: ExitPreparedOrderFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::ExitPreparedOrder;
    let descriptor = super::current_line_descriptor(purpose);
    let wire = ExitPreparedOrderWireV1::try_from(fact).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &ExitPreparedOrderLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            exit_prepared_order: wire,
        },
    )
}

pub(super) fn decode_prepared_order(
    line: &str,
    line_number: usize,
) -> Result<ExitPreparedOrderFact> {
    let decoded: ExitPreparedOrderLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ExitPreparedOrderV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    decoded.exit_prepared_order.try_into()
}

pub(super) fn encode_hold(
    fact: ExitHoldDecisionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::ExitHoldDecision;
    let descriptor = super::current_line_descriptor(purpose);
    let wire = ExitHoldWireV1::try_from(fact).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &ExitHoldLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            exit_decision: wire,
        },
    )
}

pub(super) fn decode_hold(line: &str, line_number: usize) -> Result<ExitHoldDecisionFact> {
    let decoded: ExitHoldLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ExitHoldDecisionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    decoded.exit_decision.try_into()
}

pub(super) fn encode_evaluation(
    fact: ExitEvaluationFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::ExitEvaluation;
    let descriptor = super::current_line_descriptor(purpose);
    let wire = ExitEvaluationWireV1::try_from(fact).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &ExitEvaluationLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            exit_evaluation: wire,
        },
    )
}

pub(super) fn decode_evaluation(line: &str, line_number: usize) -> Result<ExitEvaluationFact> {
    let decoded: ExitEvaluationLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ExitEvaluationV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    decoded.exit_evaluation.try_into()
}

impl TryFrom<ExitPreparedOrderFact> for ExitPreparedOrderWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitPreparedOrderFact) -> Result<Self> {
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
            prepared_order: SubmissionLinkage::from(value.prepared_order).try_into()?,
        })
    }
}

impl TryFrom<ExitPreparedOrderWireV1> for ExitPreparedOrderFact {
    type Error = anyhow::Error;

    fn try_from(value: ExitPreparedOrderWireV1) -> Result<Self> {
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
            prepared_order: PreparedOrderLinkage::from(SubmissionLinkage::try_from(
                value.prepared_order,
            )?),
        })
    }
}

impl TryFrom<ExitIntentDecisionFact> for ExitIntentWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitIntentDecisionFact) -> Result<Self> {
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
        })
    }
}

impl TryFrom<ExitIntentWireV1> for ExitIntentDecisionFact {
    type Error = anyhow::Error;

    fn try_from(value: ExitIntentWireV1) -> Result<Self> {
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
        })
    }
}

impl TryFrom<ExitHoldDecisionFact> for ExitHoldWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitHoldDecisionFact) -> Result<Self> {
        validate_hold_shape(value.outcome, value.blocked_reason)?;
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
            blocked_reason: value.blocked_reason.map(Into::into),
        })
    }
}

impl TryFrom<ExitHoldWireV1> for ExitHoldDecisionFact {
    type Error = anyhow::Error;

    fn try_from(value: ExitHoldWireV1) -> Result<Self> {
        let outcome = value.outcome.into();
        let blocked_reason = value.blocked_reason.map(Into::into);
        validate_hold_shape(outcome, blocked_reason)?;
        Ok(Self {
            details: value.details.try_into()?,
            outcome,
            blocked_reason,
        })
    }
}

impl TryFrom<ExitDecisionDetails> for ExitDecisionDetailsWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitDecisionDetails) -> Result<Self> {
        ensure!(
            value.exit_eval_now_ms > 0 && value.ts_ms > 0,
            "exit decision evaluation and record timestamps must be positive"
        );
        Ok(Self {
            strategy_id: required_text(value.strategy_id, "strategy_id")?,
            market_id: optional_text(value.market_id, "market_id")?,
            position_id: optional_text(value.position_id, "position_id")?,
            position_instrument_id: optional_text(
                value.position_instrument_id,
                "position_instrument_id",
            )?,
            position_outcome_side: value.position_outcome_side.map(Into::into),
            forced_flat_reasons: value
                .forced_flat_reasons
                .into_iter()
                .map(Into::into)
                .collect(),
            spot_price: optional_number(value.spot_price, "spot_price")?,
            spot_venue_name: optional_text(value.spot_venue_name, "spot_venue_name")?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price: optional_number(
                value.reference_current_price,
                "reference_current_price",
            )?,
            reference_current_price_available: value.reference_current_price_available,
            interval_open: optional_number(value.interval_open, "interval_open")?,
            fair_probability_up: optional_number(value.fair_probability_up, "fair_probability_up")?,
            fair_probability_down: optional_number(
                value.fair_probability_down,
                "fair_probability_down",
            )?,
            uncertainty_band_probability: optional_number(
                value.uncertainty_band_probability,
                "uncertainty_band_probability",
            )?,
            hold_ev_bps: optional_number(value.hold_ev_bps, "hold_ev_bps")?,
            exit_ev_bps: optional_number(value.exit_ev_bps, "exit_ev_bps")?,
            realized_vol: optional_number(value.realized_vol, "realized_vol")?,
            realized_vol_source_venue: optional_text(
                value.realized_vol_source_venue,
                "realized_vol_source_venue",
            )?,
            realized_vol_source_ts_ms: value.realized_vol_source_ts_ms,
            exit_eval_now_ms: value.exit_eval_now_ms,
            exit_trigger_source: value.exit_trigger_source.into(),
            trigger_ts_event_ms: value.trigger_ts_event_ms,
            trigger_ts_init_ms: value.trigger_ts_init_ms,
            rv_surface_id: required_text(value.rv_surface_id, "rv_surface_id")?,
            rv_snapshot_as_of_ms: value.rv_snapshot_as_of_ms,
            rv_snapshot_ready: value.rv_snapshot_ready,
            rv_snapshot_has_ready_realized_vol: value.rv_snapshot_has_ready_realized_vol,
            rv_snapshot_receive_watermark_ms: value.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: value.rv_max_source_age_ms,
            rv_snapshot_blockers: value
                .rv_snapshot_blockers
                .into_iter()
                .map(Into::into)
                .collect(),
            rv_source_diagnostics: value
                .rv_source_diagnostics
                .iter()
                .map(RealizedVolatilitySourceDiagnosticV1Wire::try_from)
                .collect::<Result<_>>()?,
            rv_gate_result: value.rv_gate_result.into(),
            rv_future_dating_delta_ms: value.rv_future_dating_delta_ms,
            exit_hysteresis_bps: required_number(value.exit_hysteresis_bps, "exit_hysteresis_bps")?,
            seconds_to_market_end: value.seconds_to_market_end,
            ts_ms: value.ts_ms,
            stale_reference_after_ms: value.stale_reference_after_ms,
            last_reference_ts_ms: value.last_reference_ts_ms,
            min_liquidity_required: optional_number(
                value.min_liquidity_required,
                "min_liquidity_required",
            )?,
            liquidity_available: optional_number(value.liquidity_available, "liquidity_available")?,
            frozen: value.frozen,
            metadata_matches_selection: value.metadata_matches_selection,
            fast_venue_incoherent: value.fast_venue_incoherent,
        })
    }
}

impl TryFrom<ExitDecisionDetailsWireV1> for ExitDecisionDetails {
    type Error = anyhow::Error;

    fn try_from(value: ExitDecisionDetailsWireV1) -> Result<Self> {
        ensure!(
            value.exit_eval_now_ms > 0 && value.ts_ms > 0,
            "exit decision evaluation and record timestamps must be positive"
        );
        Ok(Self {
            strategy_id: required_text(value.strategy_id, "strategy_id")?,
            market_id: optional_text(value.market_id, "market_id")?,
            position_id: optional_text(value.position_id, "position_id")?,
            position_instrument_id: optional_text(
                value.position_instrument_id,
                "position_instrument_id",
            )?,
            position_outcome_side: value.position_outcome_side.map(Into::into),
            forced_flat_reasons: value
                .forced_flat_reasons
                .into_iter()
                .map(Into::into)
                .collect(),
            spot_price: optional_number(value.spot_price, "spot_price")?,
            spot_venue_name: optional_text(value.spot_venue_name, "spot_venue_name")?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price: optional_number(
                value.reference_current_price,
                "reference_current_price",
            )?,
            reference_current_price_available: value.reference_current_price_available,
            interval_open: optional_number(value.interval_open, "interval_open")?,
            fair_probability_up: optional_number(value.fair_probability_up, "fair_probability_up")?,
            fair_probability_down: optional_number(
                value.fair_probability_down,
                "fair_probability_down",
            )?,
            uncertainty_band_probability: optional_number(
                value.uncertainty_band_probability,
                "uncertainty_band_probability",
            )?,
            hold_ev_bps: optional_number(value.hold_ev_bps, "hold_ev_bps")?,
            exit_ev_bps: optional_number(value.exit_ev_bps, "exit_ev_bps")?,
            realized_vol: optional_number(value.realized_vol, "realized_vol")?,
            realized_vol_source_venue: optional_text(
                value.realized_vol_source_venue,
                "realized_vol_source_venue",
            )?,
            realized_vol_source_ts_ms: value.realized_vol_source_ts_ms,
            exit_eval_now_ms: value.exit_eval_now_ms,
            exit_trigger_source: value.exit_trigger_source.into(),
            trigger_ts_event_ms: value.trigger_ts_event_ms,
            trigger_ts_init_ms: value.trigger_ts_init_ms,
            rv_surface_id: required_text(value.rv_surface_id, "rv_surface_id")?,
            rv_snapshot_as_of_ms: value.rv_snapshot_as_of_ms,
            rv_snapshot_ready: value.rv_snapshot_ready,
            rv_snapshot_has_ready_realized_vol: value.rv_snapshot_has_ready_realized_vol,
            rv_snapshot_receive_watermark_ms: value.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: value.rv_max_source_age_ms,
            rv_snapshot_blockers: value
                .rv_snapshot_blockers
                .into_iter()
                .map(Into::into)
                .collect(),
            rv_source_diagnostics: value
                .rv_source_diagnostics
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_>>()?,
            rv_gate_result: value.rv_gate_result.into(),
            rv_future_dating_delta_ms: value.rv_future_dating_delta_ms,
            exit_hysteresis_bps: required_number(value.exit_hysteresis_bps, "exit_hysteresis_bps")?,
            seconds_to_market_end: value.seconds_to_market_end,
            ts_ms: value.ts_ms,
            stale_reference_after_ms: value.stale_reference_after_ms,
            last_reference_ts_ms: value.last_reference_ts_ms,
            min_liquidity_required: optional_number(
                value.min_liquidity_required,
                "min_liquidity_required",
            )?,
            liquidity_available: optional_number(value.liquidity_available, "liquidity_available")?,
            frozen: value.frozen,
            metadata_matches_selection: value.metadata_matches_selection,
            fast_venue_incoherent: value.fast_venue_incoherent,
        })
    }
}

impl TryFrom<ExitEvaluationFact> for ExitEvaluationWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitEvaluationFact) -> Result<Self> {
        validate_evaluation_times(&value)?;
        Ok(Self {
            position_id: optional_text(value.position_id, "position_id")?,
            market_id: optional_text(value.market_id, "market_id")?,
            instrument_id: optional_text(value.instrument_id, "instrument_id")?,
            exit_eval_now_ms: value.exit_eval_now_ms,
            exit_trigger_source: value.exit_trigger_source.into(),
            trigger_ts_event_ms: value.trigger_ts_event_ms,
            trigger_ts_init_ms: value.trigger_ts_init_ms,
            rv_surface_id: required_text(value.rv_surface_id, "rv_surface_id")?,
            rv_as_of_ms: value.rv_as_of_ms,
            rv_ready: value.rv_ready,
            rv_snapshot_receive_watermark_ms: value.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: value.rv_max_source_age_ms,
            rv_blockers: value.rv_blockers.into_iter().map(Into::into).collect(),
            rv_source_diagnostics: value
                .rv_source_diagnostics
                .iter()
                .map(RealizedVolatilitySourceDiagnosticV1Wire::try_from)
                .collect::<Result<_>>()?,
            rv_gate_result: value.rv_gate_result.into(),
            rv_as_of_minus_now_ms: value.rv_as_of_minus_now_ms,
            spot_price: optional_number(value.spot_price, "spot_price")?,
            spot_venue_name: optional_text(value.spot_venue_name, "spot_venue_name")?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price: optional_number(
                value.reference_current_price,
                "reference_current_price",
            )?,
            reference_current_price_available: value.reference_current_price_available,
            interval_open: optional_number(value.interval_open, "interval_open")?,
            fair_probability_up: optional_number(value.fair_probability_up, "fair_probability_up")?,
            fair_probability_down: optional_number(
                value.fair_probability_down,
                "fair_probability_down",
            )?,
            uncertainty_band_probability: optional_number(
                value.uncertainty_band_probability,
                "uncertainty_band_probability",
            )?,
            hold_ev_bps: optional_number(value.hold_ev_bps, "hold_ev_bps")?,
            exit_ev_bps: optional_number(value.exit_ev_bps, "exit_ev_bps")?,
            outcome: value.outcome.try_into()?,
            forced_flat_reasons: value
                .forced_flat_reasons
                .into_iter()
                .map(Into::into)
                .collect(),
        })
    }
}

impl TryFrom<ExitEvaluationWireV1> for ExitEvaluationFact {
    type Error = anyhow::Error;

    fn try_from(value: ExitEvaluationWireV1) -> Result<Self> {
        let fact = Self {
            position_id: optional_text(value.position_id, "position_id")?,
            market_id: optional_text(value.market_id, "market_id")?,
            instrument_id: optional_text(value.instrument_id, "instrument_id")?,
            exit_eval_now_ms: value.exit_eval_now_ms,
            exit_trigger_source: value.exit_trigger_source.into(),
            trigger_ts_event_ms: value.trigger_ts_event_ms,
            trigger_ts_init_ms: value.trigger_ts_init_ms,
            rv_surface_id: required_text(value.rv_surface_id, "rv_surface_id")?,
            rv_as_of_ms: value.rv_as_of_ms,
            rv_ready: value.rv_ready,
            rv_snapshot_receive_watermark_ms: value.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: value.rv_max_source_age_ms,
            rv_blockers: value.rv_blockers.into_iter().map(Into::into).collect(),
            rv_source_diagnostics: value
                .rv_source_diagnostics
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_>>()?,
            rv_gate_result: value.rv_gate_result.into(),
            rv_as_of_minus_now_ms: value.rv_as_of_minus_now_ms,
            spot_price: optional_number(value.spot_price, "spot_price")?,
            spot_venue_name: optional_text(value.spot_venue_name, "spot_venue_name")?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price: optional_number(
                value.reference_current_price,
                "reference_current_price",
            )?,
            reference_current_price_available: value.reference_current_price_available,
            interval_open: optional_number(value.interval_open, "interval_open")?,
            fair_probability_up: optional_number(value.fair_probability_up, "fair_probability_up")?,
            fair_probability_down: optional_number(
                value.fair_probability_down,
                "fair_probability_down",
            )?,
            uncertainty_band_probability: optional_number(
                value.uncertainty_band_probability,
                "uncertainty_band_probability",
            )?,
            hold_ev_bps: optional_number(value.hold_ev_bps, "hold_ev_bps")?,
            exit_ev_bps: optional_number(value.exit_ev_bps, "exit_ev_bps")?,
            outcome: value.outcome.try_into()?,
            forced_flat_reasons: value
                .forced_flat_reasons
                .into_iter()
                .map(Into::into)
                .collect(),
        };
        validate_evaluation_times(&fact)?;
        Ok(fact)
    }
}

impl TryFrom<ExitAttemptOutcome> for ExitAttemptOutcomeV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitAttemptOutcome) -> Result<Self> {
        Ok(match value {
            ExitAttemptOutcome::Held { outcome } => Self::Held {
                outcome: outcome.into(),
            },
            ExitAttemptOutcome::Blocked { blocked_reason } => Self::Blocked {
                blocked_reason: blocked_reason.into(),
            },
            ExitAttemptOutcome::PreparationRejected { stage, reason } => {
                Self::PreparationRejected {
                    stage: stage.into(),
                    reason: required_text(reason, "preparation_rejected.reason")?,
                }
            }
            ExitAttemptOutcome::RouteRejected {
                prepared_order,
                reason,
            } => Self::RouteRejected {
                prepared_order: prepared_linkage_to_wire(prepared_order)?,
                reason: required_text(reason, "route_rejected.reason")?,
            },
            ExitAttemptOutcome::IntentEvidenceRejected {
                prepared_order,
                reason,
            } => Self::IntentEvidenceRejected {
                prepared_order: prepared_linkage_to_wire(prepared_order)?,
                reason: required_text(reason, "intent_evidence_rejected.reason")?,
            },
            ExitAttemptOutcome::AdmissionRejected {
                prepared_order,
                reason,
            } => Self::AdmissionRejected {
                prepared_order: prepared_linkage_to_wire(prepared_order)?,
                reason: required_text(reason, "admission_rejected.reason")?,
            },
            ExitAttemptOutcome::PolicySkipped { prepared_order } => Self::PolicySkipped {
                prepared_order: prepared_linkage_to_wire(prepared_order)?,
            },
            ExitAttemptOutcome::PreSinkRejected {
                prepared_order,
                reason,
            } => Self::PreSinkRejected {
                prepared_order: prepared_linkage_to_wire(prepared_order)?,
                reason: required_text(reason, "pre_sink_rejected.reason")?,
            },
            ExitAttemptOutcome::SinkRejected {
                prepared_order,
                reason,
            } => Self::SinkRejected {
                prepared_order: prepared_linkage_to_wire(prepared_order)?,
                reason: required_text(reason, "sink_rejected.reason")?,
            },
            ExitAttemptOutcome::Submitted { submitted_order } => Self::Submitted {
                submitted_order: submitted_linkage_to_wire(submitted_order)?,
            },
        })
    }
}

impl TryFrom<ExitAttemptOutcomeV1> for ExitAttemptOutcome {
    type Error = anyhow::Error;

    fn try_from(value: ExitAttemptOutcomeV1) -> Result<Self> {
        Ok(match value {
            ExitAttemptOutcomeV1::Held { outcome } => Self::Held {
                outcome: outcome.into(),
            },
            ExitAttemptOutcomeV1::Blocked { blocked_reason } => Self::Blocked {
                blocked_reason: blocked_reason.into(),
            },
            ExitAttemptOutcomeV1::PreparationRejected { stage, reason } => {
                Self::PreparationRejected {
                    stage: stage.into(),
                    reason: required_text(reason, "preparation_rejected.reason")?,
                }
            }
            ExitAttemptOutcomeV1::RouteRejected {
                prepared_order,
                reason,
            } => Self::RouteRejected {
                prepared_order: prepared_linkage_from_wire(prepared_order)?,
                reason: required_text(reason, "route_rejected.reason")?,
            },
            ExitAttemptOutcomeV1::IntentEvidenceRejected {
                prepared_order,
                reason,
            } => Self::IntentEvidenceRejected {
                prepared_order: prepared_linkage_from_wire(prepared_order)?,
                reason: required_text(reason, "intent_evidence_rejected.reason")?,
            },
            ExitAttemptOutcomeV1::AdmissionRejected {
                prepared_order,
                reason,
            } => Self::AdmissionRejected {
                prepared_order: prepared_linkage_from_wire(prepared_order)?,
                reason: required_text(reason, "admission_rejected.reason")?,
            },
            ExitAttemptOutcomeV1::PolicySkipped { prepared_order } => Self::PolicySkipped {
                prepared_order: prepared_linkage_from_wire(prepared_order)?,
            },
            ExitAttemptOutcomeV1::PreSinkRejected {
                prepared_order,
                reason,
            } => Self::PreSinkRejected {
                prepared_order: prepared_linkage_from_wire(prepared_order)?,
                reason: required_text(reason, "pre_sink_rejected.reason")?,
            },
            ExitAttemptOutcomeV1::SinkRejected {
                prepared_order,
                reason,
            } => Self::SinkRejected {
                prepared_order: prepared_linkage_from_wire(prepared_order)?,
                reason: required_text(reason, "sink_rejected.reason")?,
            },
            ExitAttemptOutcomeV1::Submitted { submitted_order } => Self::Submitted {
                submitted_order: submitted_linkage_from_wire(submitted_order)?,
            },
        })
    }
}

fn prepared_linkage_to_wire(value: PreparedOrderLinkage) -> Result<SubmissionLinkageWireV1> {
    SubmissionLinkage::from(value).try_into()
}

fn prepared_linkage_from_wire(value: SubmissionLinkageWireV1) -> Result<PreparedOrderLinkage> {
    Ok(SubmissionLinkage::try_from(value)?.into())
}

fn submitted_linkage_to_wire(value: SubmittedOrderLinkage) -> Result<SubmissionLinkageWireV1> {
    SubmissionLinkage::from(value).try_into()
}

fn submitted_linkage_from_wire(value: SubmissionLinkageWireV1) -> Result<SubmittedOrderLinkage> {
    Ok(SubmissionLinkage::try_from(value)?.into())
}

macro_rules! bidirectional_unit_enum {
    ($semantic:ty, $wire:ty, [$($variant:ident),+ $(,)?]) => {
        impl From<$semantic> for $wire {
            fn from(value: $semantic) -> Self {
                match value { $(<$semantic>::$variant => Self::$variant,)+ }
            }
        }
        impl From<$wire> for $semantic {
            fn from(value: $wire) -> Self {
                match value { $(<$wire>::$variant => Self::$variant,)+ }
            }
        }
    };
}

bidirectional_unit_enum!(
    ExitIntentOutcome,
    ExitIntentOutcomeV1,
    [Exit, ExitFailClosed]
);
bidirectional_unit_enum!(ExitHoldOutcome, ExitHoldOutcomeV1, [Hold, Blocked]);
bidirectional_unit_enum!(
    ExitPreparationStage,
    ExitPreparationStageV1,
    [
        OrderTemplate,
        InstrumentAuthority,
        PositionAuthority,
        ExecutableLiquidity,
        EconomicsSeal
    ]
);
bidirectional_unit_enum!(
    ExitTriggerSource,
    ExitTriggerSourceV1,
    [
        SignalQuote,
        ReferenceUpdate,
        SelectionUpdate,
        BookDelta,
        Unknown,
        Other
    ]
);
bidirectional_unit_enum!(
    ExitBlockedReason,
    ExitBlockedReasonV1,
    [
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
        ExitQuantityNotPositive
    ]
);

fn validate_hold_shape(
    outcome: ExitHoldOutcome,
    blocked_reason: Option<ExitBlockedReason>,
) -> Result<()> {
    ensure!(
        matches!(
            (outcome, blocked_reason),
            (ExitHoldOutcome::Hold, None) | (ExitHoldOutcome::Blocked, Some(_))
        ),
        "exit hold outcome and blocked reason are contradictory"
    );
    Ok(())
}

fn validate_evaluation_times(value: &ExitEvaluationFact) -> Result<()> {
    ensure!(
        value.exit_eval_now_ms >= 0,
        "exit_eval_now_ms must be non-negative"
    );
    ensure!(
        value.trigger_ts_event_ms.is_none_or(|value| value >= 0),
        "trigger_ts_event_ms must be non-negative"
    );
    ensure!(
        value.trigger_ts_init_ms.is_none_or(|value| value >= 0),
        "trigger_ts_init_ms must be non-negative"
    );
    ensure!(
        value
            .rv_snapshot_receive_watermark_ms
            .is_none_or(|value| value >= 0),
        "rv_snapshot_receive_watermark_ms must be non-negative"
    );
    Ok(())
}

fn required_text(value: String, field: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be non-empty and canonical"
    );
    Ok(value)
}

fn optional_text(value: Option<String>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_text(value, field)).transpose()
}

fn required_number(value: String, field: &str) -> Result<String> {
    let value = required_text(value, field)?;
    let parsed = value
        .parse::<f64>()
        .with_context(|| format!("`{field}` must parse as a number"))?;
    ensure!(parsed.is_finite(), "`{field}` must be finite");
    Ok(value)
}

fn optional_number(value: Option<String>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_number(value, field)).transpose()
}
