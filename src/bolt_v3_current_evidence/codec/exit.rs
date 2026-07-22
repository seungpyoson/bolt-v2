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
        ExitBlockedReason, ExitDecisionDetails, ExitEvaluationDecision, ExitEvaluationFact,
        ExitHoldDecisionFact, ExitHoldOutcome, ExitSubmissionDecisionFact, ExitSubmissionOutcome,
        ExitTriggerSource,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitSubmissionLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_decision: ExitSubmissionWireV1,
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
struct ExitSubmissionWireV1 {
    details: ExitDecisionDetailsWireV1,
    outcome: ExitSubmissionOutcomeV1,
    submission: SubmissionLinkageWireV1,
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
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
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
    client_order_id: Option<String>,
    exit_eval_now_ms: i64,
    exit_trigger_source: ExitTriggerSourceV1,
    trigger_ts_event_ms: Option<i64>,
    trigger_ts_init_ms: Option<i64>,
    rv_surface_id: String,
    rv_as_of_ms: Option<i64>,
    rv_ready: bool,
    rv_snapshot_receive_watermark_ms: Option<i64>,
    rv_max_source_age_ms: Option<u64>,
    rv_blockers: Vec<String>,
    rv_source_diagnostics: Vec<String>,
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
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    decision: ExitEvaluationDecisionV1,
    forced_flat_reasons: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitSubmissionOutcomeV1 {
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
#[serde(tag = "action", rename_all = "snake_case")]
enum ExitEvaluationDecisionV1 {
    Submission {
        outcome: ExitSubmissionOutcomeV1,
        submission: SubmissionLinkageWireV1,
    },
    Hold {
        outcome: ExitHoldOutcomeV1,
        blocked_reason: Option<ExitBlockedReasonV1>,
    },
}

pub(super) fn encode_submission(
    fact: ExitSubmissionDecisionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::ExitSubmissionDecision;
    let descriptor = super::current_line_descriptor(purpose);
    let wire = ExitSubmissionWireV1::try_from(fact).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &ExitSubmissionLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            exit_decision: wire,
        },
    )
}

pub(super) fn decode_submission(
    line: &str,
    line_number: usize,
) -> Result<ExitSubmissionDecisionFact> {
    let decoded: ExitSubmissionLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ExitSubmissionDecisionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    decoded.exit_decision.try_into()
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

impl TryFrom<ExitSubmissionDecisionFact> for ExitSubmissionWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitSubmissionDecisionFact) -> Result<Self> {
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
            submission: value.submission.try_into()?,
        })
    }
}

impl TryFrom<ExitSubmissionWireV1> for ExitSubmissionDecisionFact {
    type Error = anyhow::Error;

    fn try_from(value: ExitSubmissionWireV1) -> Result<Self> {
        Ok(Self {
            details: value.details.try_into()?,
            outcome: value.outcome.into(),
            submission: value.submission.try_into()?,
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
            value.exit_eval_now_ms > 0 && value.trigger_ts_event_ms > 0 && value.ts_ms > 0,
            "exit decision timestamps must be positive"
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
            up_fee_bps: optional_number(value.up_fee_bps, "up_fee_bps")?,
            down_fee_bps: optional_number(value.down_fee_bps, "down_fee_bps")?,
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
            value.exit_eval_now_ms > 0 && value.trigger_ts_event_ms > 0 && value.ts_ms > 0,
            "exit decision timestamps must be positive"
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
            up_fee_bps: optional_number(value.up_fee_bps, "up_fee_bps")?,
            down_fee_bps: optional_number(value.down_fee_bps, "down_fee_bps")?,
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
            client_order_id: optional_text(value.client_order_id, "client_order_id")?,
            exit_eval_now_ms: value.exit_eval_now_ms,
            exit_trigger_source: value.exit_trigger_source.into(),
            trigger_ts_event_ms: value.trigger_ts_event_ms,
            trigger_ts_init_ms: value.trigger_ts_init_ms,
            rv_surface_id: required_text(value.rv_surface_id, "rv_surface_id")?,
            rv_as_of_ms: value.rv_as_of_ms,
            rv_ready: value.rv_ready,
            rv_snapshot_receive_watermark_ms: value.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: value.rv_max_source_age_ms,
            rv_blockers: canonical_texts(value.rv_blockers, "rv_blockers")?,
            rv_source_diagnostics: canonical_texts(
                value.rv_source_diagnostics,
                "rv_source_diagnostics",
            )?,
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
            up_fee_bps: optional_number(value.up_fee_bps, "up_fee_bps")?,
            down_fee_bps: optional_number(value.down_fee_bps, "down_fee_bps")?,
            hold_ev_bps: optional_number(value.hold_ev_bps, "hold_ev_bps")?,
            exit_ev_bps: optional_number(value.exit_ev_bps, "exit_ev_bps")?,
            decision: value.decision.try_into()?,
            forced_flat_reasons: canonical_texts(value.forced_flat_reasons, "forced_flat_reasons")?,
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
            client_order_id: optional_text(value.client_order_id, "client_order_id")?,
            exit_eval_now_ms: value.exit_eval_now_ms,
            exit_trigger_source: value.exit_trigger_source.into(),
            trigger_ts_event_ms: value.trigger_ts_event_ms,
            trigger_ts_init_ms: value.trigger_ts_init_ms,
            rv_surface_id: required_text(value.rv_surface_id, "rv_surface_id")?,
            rv_as_of_ms: value.rv_as_of_ms,
            rv_ready: value.rv_ready,
            rv_snapshot_receive_watermark_ms: value.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: value.rv_max_source_age_ms,
            rv_blockers: canonical_texts(value.rv_blockers, "rv_blockers")?,
            rv_source_diagnostics: canonical_texts(
                value.rv_source_diagnostics,
                "rv_source_diagnostics",
            )?,
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
            up_fee_bps: optional_number(value.up_fee_bps, "up_fee_bps")?,
            down_fee_bps: optional_number(value.down_fee_bps, "down_fee_bps")?,
            hold_ev_bps: optional_number(value.hold_ev_bps, "hold_ev_bps")?,
            exit_ev_bps: optional_number(value.exit_ev_bps, "exit_ev_bps")?,
            decision: value.decision.try_into()?,
            forced_flat_reasons: canonical_texts(value.forced_flat_reasons, "forced_flat_reasons")?,
        };
        validate_evaluation_times(&fact)?;
        Ok(fact)
    }
}

impl TryFrom<ExitEvaluationDecision> for ExitEvaluationDecisionV1 {
    type Error = anyhow::Error;

    fn try_from(value: ExitEvaluationDecision) -> Result<Self> {
        Ok(match value {
            ExitEvaluationDecision::Submission {
                outcome,
                submission,
            } => Self::Submission {
                outcome: outcome.into(),
                submission: submission.try_into()?,
            },
            ExitEvaluationDecision::Hold {
                outcome,
                blocked_reason,
            } => {
                validate_hold_shape(outcome, blocked_reason)?;
                Self::Hold {
                    outcome: outcome.into(),
                    blocked_reason: blocked_reason.map(Into::into),
                }
            }
        })
    }
}

impl TryFrom<ExitEvaluationDecisionV1> for ExitEvaluationDecision {
    type Error = anyhow::Error;

    fn try_from(value: ExitEvaluationDecisionV1) -> Result<Self> {
        Ok(match value {
            ExitEvaluationDecisionV1::Submission {
                outcome,
                submission,
            } => Self::Submission {
                outcome: outcome.into(),
                submission: submission.try_into()?,
            },
            ExitEvaluationDecisionV1::Hold {
                outcome,
                blocked_reason,
            } => {
                let outcome = outcome.into();
                let blocked_reason = blocked_reason.map(Into::into);
                validate_hold_shape(outcome, blocked_reason)?;
                Self::Hold {
                    outcome,
                    blocked_reason,
                }
            }
        })
    }
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
    ExitSubmissionOutcome,
    ExitSubmissionOutcomeV1,
    [Exit, ExitFailClosed]
);
bidirectional_unit_enum!(ExitHoldOutcome, ExitHoldOutcomeV1, [Hold, Blocked]);
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

fn canonical_texts(values: Vec<String>, field: &str) -> Result<Vec<String>> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| required_text(value, &format!("{field}[{index}]")))
        .collect()
}
