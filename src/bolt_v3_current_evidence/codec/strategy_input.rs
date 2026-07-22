use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    decode, encode_line,
    entry_skip::{
        EntryBlockReasonV1, EntryPricingBlockReasonV1, EntryRealizedVolatilitySnapshotV1Wire,
        RvGateResultV1,
    },
    validate_envelope, validate_recorded_at,
};
use crate::bolt_v3_current_evidence::{
    facts::{
        BlockedStrategyInputObservationFact, StrategyInputDetails,
        StrategyInputMarketSelectionOutcome, StrategyInputRvState, SubmissionLinkage,
        SubmitLinkedStrategyInputSnapshotFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockedStrategyInputLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    blocked_strategy_input_observation: BlockedStrategyInputWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitLinkedStrategyInputLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    snapshot: SubmitLinkedStrategyInputWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockedStrategyInputWireV1 {
    details: StrategyInputDetailsWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitLinkedStrategyInputWireV1 {
    details: StrategyInputDetailsWireV1,
    submission: SubmissionLinkageWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyInputDetailsWireV1 {
    strategy_id: String,
    configured_target_id: String,
    market_selection_ruleset_id: String,
    market_selection_outcome: StrategyInputMarketSelectionOutcomeV1,
    market_id: Option<String>,
    polymarket_condition_id: Option<String>,
    polymarket_market_slug: Option<String>,
    polymarket_question_id: Option<String>,
    up_instrument_id: Option<String>,
    down_instrument_id: Option<String>,
    market_selection_timestamp_ms: Option<u64>,
    selected_market_observed_timestamp_ms: Option<u64>,
    polymarket_market_start_timestamp_ms: Option<u64>,
    polymarket_market_end_timestamp_ms: Option<u64>,
    price_to_beat_source: String,
    price_to_beat_value: String,
    reference_quote_ts_event: u64,
    spot_price: String,
    fast_venue_available: bool,
    reference_current_price: Option<String>,
    reference_current_price_available: bool,
    reference_current_price_source_id: Option<String>,
    reference_current_price_failed_over: Option<bool>,
    realized_volatility: StrategyInputRvStateWireV1,
    seconds_to_market_end: u64,
    pricing_kurtosis: String,
    theta_decay_factor: String,
    theta_scaled_min_edge_bps: String,
    fair_probability_up: String,
    uncertainty_band_probability: String,
    expected_edge_basis_points: String,
    worst_case_edge_basis_points: String,
    up_worst_case_edge_basis_points: Option<String>,
    down_worst_case_edge_basis_points: Option<String>,
    gate_blocked_by: Vec<EntryBlockReasonV1>,
    pricing_blocked_by: Vec<EntryPricingBlockReasonV1>,
    fast_venue_name: Option<String>,
    fast_venue_age_ms: Option<u64>,
    fast_venue_jitter_ms: Option<u64>,
    fast_venue_incoherent: bool,
    lead_agreement_corr: Option<String>,
    fee_rate_basis_points: String,
    selected_side: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StrategyInputMarketSelectionOutcomeV1 {
    Current,
    Next,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum StrategyInputRvStateWireV1 {
    Absent {
        gate_result: RvGateResultV1,
    },
    Present {
        selected_annualized_decimal: Option<String>,
        gate_result: RvGateResultV1,
        receive_watermark_ms: Option<u64>,
        snapshot: Box<EntryRealizedVolatilitySnapshotV1Wire>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SubmissionLinkageWireV1 {
    instrument_id: String,
    order_side: String,
    price: String,
    quantity: String,
    client_order_id: String,
}

pub(super) fn encode_blocked(
    fact: BlockedStrategyInputObservationFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::BlockedStrategyInputObservation;
    let descriptor = super::current_line_descriptor(purpose);
    let details =
        StrategyInputDetailsWireV1::try_from(fact.details).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &BlockedStrategyInputLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            blocked_strategy_input_observation: BlockedStrategyInputWireV1 { details },
        },
    )
}

pub(super) fn decode_blocked(
    line: &str,
    line_number: usize,
) -> Result<BlockedStrategyInputObservationFact> {
    let decoded: BlockedStrategyInputLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::BlockedStrategyInputObservationV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    Ok(BlockedStrategyInputObservationFact {
        details: decoded
            .blocked_strategy_input_observation
            .details
            .try_into()?,
    })
}

pub(super) fn encode_submit(
    fact: SubmitLinkedStrategyInputSnapshotFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    let purpose = KnownPurpose::SubmitLinkedStrategyInputSnapshot;
    let descriptor = super::current_line_descriptor(purpose);
    let details =
        StrategyInputDetailsWireV1::try_from(fact.details).map_err(RecordFailure::Rejected)?;
    let submission =
        SubmissionLinkageWireV1::try_from(fact.submission).map_err(RecordFailure::Rejected)?;
    encode_line(
        purpose,
        &SubmitLinkedStrategyInputLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            snapshot: SubmitLinkedStrategyInputWireV1 {
                details,
                submission,
            },
        },
    )
}

pub(super) fn decode_submit(
    line: &str,
    line_number: usize,
) -> Result<SubmitLinkedStrategyInputSnapshotFact> {
    let decoded: SubmitLinkedStrategyInputLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::SubmitLinkedStrategyInputSnapshotV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    Ok(SubmitLinkedStrategyInputSnapshotFact {
        details: decoded.snapshot.details.try_into()?,
        submission: decoded.snapshot.submission.try_into()?,
    })
}

impl TryFrom<StrategyInputDetails> for StrategyInputDetailsWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: StrategyInputDetails) -> Result<Self> {
        ensure!(
            value.reference_quote_ts_event > 0,
            "reference quote timestamp must be positive"
        );
        Ok(Self {
            strategy_id: required_text(value.strategy_id, "strategy_id")?,
            configured_target_id: required_text(
                value.configured_target_id,
                "configured_target_id",
            )?,
            market_selection_ruleset_id: required_text(
                value.market_selection_ruleset_id,
                "market_selection_ruleset_id",
            )?,
            market_selection_outcome: value.market_selection_outcome.into(),
            market_id: optional_text(value.market_id, "market_id")?,
            polymarket_condition_id: optional_text(
                value.polymarket_condition_id,
                "polymarket_condition_id",
            )?,
            polymarket_market_slug: optional_text(
                value.polymarket_market_slug,
                "polymarket_market_slug",
            )?,
            polymarket_question_id: optional_text(
                value.polymarket_question_id,
                "polymarket_question_id",
            )?,
            up_instrument_id: optional_text(value.up_instrument_id, "up_instrument_id")?,
            down_instrument_id: optional_text(value.down_instrument_id, "down_instrument_id")?,
            market_selection_timestamp_ms: value.market_selection_timestamp_ms,
            selected_market_observed_timestamp_ms: value.selected_market_observed_timestamp_ms,
            polymarket_market_start_timestamp_ms: value.polymarket_market_start_timestamp_ms,
            polymarket_market_end_timestamp_ms: value.polymarket_market_end_timestamp_ms,
            price_to_beat_source: required_text(
                value.price_to_beat_source,
                "price_to_beat_source",
            )?,
            price_to_beat_value: required_number(value.price_to_beat_value, "price_to_beat_value")?,
            reference_quote_ts_event: value.reference_quote_ts_event,
            spot_price: required_number(value.spot_price, "spot_price")?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price: optional_number(
                value.reference_current_price,
                "reference_current_price",
            )?,
            reference_current_price_available: value.reference_current_price_available,
            reference_current_price_source_id: optional_text(
                value.reference_current_price_source_id,
                "reference_current_price_source_id",
            )?,
            reference_current_price_failed_over: value.reference_current_price_failed_over,
            realized_volatility: value.realized_volatility.try_into()?,
            seconds_to_market_end: value.seconds_to_market_end,
            pricing_kurtosis: required_number(value.pricing_kurtosis, "pricing_kurtosis")?,
            theta_decay_factor: required_number(value.theta_decay_factor, "theta_decay_factor")?,
            theta_scaled_min_edge_bps: required_number(
                value.theta_scaled_min_edge_bps,
                "theta_scaled_min_edge_bps",
            )?,
            fair_probability_up: required_number(value.fair_probability_up, "fair_probability_up")?,
            uncertainty_band_probability: required_number(
                value.uncertainty_band_probability,
                "uncertainty_band_probability",
            )?,
            expected_edge_basis_points: required_number(
                value.expected_edge_basis_points,
                "expected_edge_basis_points",
            )?,
            worst_case_edge_basis_points: required_number(
                value.worst_case_edge_basis_points,
                "worst_case_edge_basis_points",
            )?,
            up_worst_case_edge_basis_points: optional_number(
                value.up_worst_case_edge_basis_points,
                "up_worst_case_edge_basis_points",
            )?,
            down_worst_case_edge_basis_points: optional_number(
                value.down_worst_case_edge_basis_points,
                "down_worst_case_edge_basis_points",
            )?,
            gate_blocked_by: value.gate_blocked_by.into_iter().map(Into::into).collect(),
            pricing_blocked_by: value
                .pricing_blocked_by
                .into_iter()
                .map(Into::into)
                .collect(),
            fast_venue_name: optional_text(value.fast_venue_name, "fast_venue_name")?,
            fast_venue_age_ms: value.fast_venue_age_ms,
            fast_venue_jitter_ms: value.fast_venue_jitter_ms,
            fast_venue_incoherent: value.fast_venue_incoherent,
            lead_agreement_corr: optional_number(value.lead_agreement_corr, "lead_agreement_corr")?,
            fee_rate_basis_points: required_number(
                value.fee_rate_basis_points,
                "fee_rate_basis_points",
            )?,
            selected_side: optional_text(value.selected_side, "selected_side")?,
        })
    }
}

impl TryFrom<StrategyInputDetailsWireV1> for StrategyInputDetails {
    type Error = anyhow::Error;

    fn try_from(value: StrategyInputDetailsWireV1) -> Result<Self> {
        ensure!(
            value.reference_quote_ts_event > 0,
            "reference quote timestamp must be positive"
        );
        Ok(Self {
            strategy_id: required_text(value.strategy_id, "strategy_id")?,
            configured_target_id: required_text(
                value.configured_target_id,
                "configured_target_id",
            )?,
            market_selection_ruleset_id: required_text(
                value.market_selection_ruleset_id,
                "market_selection_ruleset_id",
            )?,
            market_selection_outcome: value.market_selection_outcome.into(),
            market_id: optional_text(value.market_id, "market_id")?,
            polymarket_condition_id: optional_text(
                value.polymarket_condition_id,
                "polymarket_condition_id",
            )?,
            polymarket_market_slug: optional_text(
                value.polymarket_market_slug,
                "polymarket_market_slug",
            )?,
            polymarket_question_id: optional_text(
                value.polymarket_question_id,
                "polymarket_question_id",
            )?,
            up_instrument_id: optional_text(value.up_instrument_id, "up_instrument_id")?,
            down_instrument_id: optional_text(value.down_instrument_id, "down_instrument_id")?,
            market_selection_timestamp_ms: value.market_selection_timestamp_ms,
            selected_market_observed_timestamp_ms: value.selected_market_observed_timestamp_ms,
            polymarket_market_start_timestamp_ms: value.polymarket_market_start_timestamp_ms,
            polymarket_market_end_timestamp_ms: value.polymarket_market_end_timestamp_ms,
            price_to_beat_source: required_text(
                value.price_to_beat_source,
                "price_to_beat_source",
            )?,
            price_to_beat_value: required_number(value.price_to_beat_value, "price_to_beat_value")?,
            reference_quote_ts_event: value.reference_quote_ts_event,
            spot_price: required_number(value.spot_price, "spot_price")?,
            fast_venue_available: value.fast_venue_available,
            reference_current_price: optional_number(
                value.reference_current_price,
                "reference_current_price",
            )?,
            reference_current_price_available: value.reference_current_price_available,
            reference_current_price_source_id: optional_text(
                value.reference_current_price_source_id,
                "reference_current_price_source_id",
            )?,
            reference_current_price_failed_over: value.reference_current_price_failed_over,
            realized_volatility: value.realized_volatility.try_into()?,
            seconds_to_market_end: value.seconds_to_market_end,
            pricing_kurtosis: required_number(value.pricing_kurtosis, "pricing_kurtosis")?,
            theta_decay_factor: required_number(value.theta_decay_factor, "theta_decay_factor")?,
            theta_scaled_min_edge_bps: required_number(
                value.theta_scaled_min_edge_bps,
                "theta_scaled_min_edge_bps",
            )?,
            fair_probability_up: required_number(value.fair_probability_up, "fair_probability_up")?,
            uncertainty_band_probability: required_number(
                value.uncertainty_band_probability,
                "uncertainty_band_probability",
            )?,
            expected_edge_basis_points: required_number(
                value.expected_edge_basis_points,
                "expected_edge_basis_points",
            )?,
            worst_case_edge_basis_points: required_number(
                value.worst_case_edge_basis_points,
                "worst_case_edge_basis_points",
            )?,
            up_worst_case_edge_basis_points: optional_number(
                value.up_worst_case_edge_basis_points,
                "up_worst_case_edge_basis_points",
            )?,
            down_worst_case_edge_basis_points: optional_number(
                value.down_worst_case_edge_basis_points,
                "down_worst_case_edge_basis_points",
            )?,
            gate_blocked_by: value.gate_blocked_by.into_iter().map(Into::into).collect(),
            pricing_blocked_by: value
                .pricing_blocked_by
                .into_iter()
                .map(Into::into)
                .collect(),
            fast_venue_name: optional_text(value.fast_venue_name, "fast_venue_name")?,
            fast_venue_age_ms: value.fast_venue_age_ms,
            fast_venue_jitter_ms: value.fast_venue_jitter_ms,
            fast_venue_incoherent: value.fast_venue_incoherent,
            lead_agreement_corr: optional_number(value.lead_agreement_corr, "lead_agreement_corr")?,
            fee_rate_basis_points: required_number(
                value.fee_rate_basis_points,
                "fee_rate_basis_points",
            )?,
            selected_side: optional_text(value.selected_side, "selected_side")?,
        })
    }
}

impl From<StrategyInputMarketSelectionOutcome> for StrategyInputMarketSelectionOutcomeV1 {
    fn from(value: StrategyInputMarketSelectionOutcome) -> Self {
        match value {
            StrategyInputMarketSelectionOutcome::Current => Self::Current,
            StrategyInputMarketSelectionOutcome::Next => Self::Next,
        }
    }
}

impl From<StrategyInputMarketSelectionOutcomeV1> for StrategyInputMarketSelectionOutcome {
    fn from(value: StrategyInputMarketSelectionOutcomeV1) -> Self {
        match value {
            StrategyInputMarketSelectionOutcomeV1::Current => Self::Current,
            StrategyInputMarketSelectionOutcomeV1::Next => Self::Next,
        }
    }
}

impl TryFrom<StrategyInputRvState> for StrategyInputRvStateWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: StrategyInputRvState) -> Result<Self> {
        Ok(match value {
            StrategyInputRvState::Absent { gate_result } => Self::Absent {
                gate_result: gate_result.into(),
            },
            StrategyInputRvState::Present {
                selected_annualized_decimal,
                gate_result,
                receive_watermark_ms,
                snapshot,
            } => Self::Present {
                selected_annualized_decimal: optional_number(
                    selected_annualized_decimal,
                    "realized_volatility.selected_annualized_decimal",
                )?,
                gate_result: gate_result.into(),
                receive_watermark_ms,
                snapshot: Box::new(EntryRealizedVolatilitySnapshotV1Wire::try_from(
                    snapshot.as_ref(),
                )?),
            },
        })
    }
}

impl TryFrom<StrategyInputRvStateWireV1> for StrategyInputRvState {
    type Error = anyhow::Error;

    fn try_from(value: StrategyInputRvStateWireV1) -> Result<Self> {
        Ok(match value {
            StrategyInputRvStateWireV1::Absent { gate_result } => Self::Absent {
                gate_result: gate_result.into(),
            },
            StrategyInputRvStateWireV1::Present {
                selected_annualized_decimal,
                gate_result,
                receive_watermark_ms,
                snapshot,
            } => Self::Present {
                selected_annualized_decimal: optional_number(
                    selected_annualized_decimal,
                    "realized_volatility.selected_annualized_decimal",
                )?,
                gate_result: gate_result.into(),
                receive_watermark_ms,
                snapshot: Box::new((*snapshot).try_into()?),
            },
        })
    }
}

impl TryFrom<SubmissionLinkage> for SubmissionLinkageWireV1 {
    type Error = anyhow::Error;

    fn try_from(value: SubmissionLinkage) -> Result<Self> {
        Ok(Self {
            instrument_id: required_text(value.instrument_id, "submission.instrument_id")?,
            order_side: required_text(value.order_side, "submission.order_side")?,
            price: required_number(value.price, "submission.price")?,
            quantity: required_number(value.quantity, "submission.quantity")?,
            client_order_id: required_text(value.client_order_id, "submission.client_order_id")?,
        })
    }
}

impl TryFrom<SubmissionLinkageWireV1> for SubmissionLinkage {
    type Error = anyhow::Error;

    fn try_from(value: SubmissionLinkageWireV1) -> Result<Self> {
        Ok(Self {
            instrument_id: required_text(value.instrument_id, "submission.instrument_id")?,
            order_side: required_text(value.order_side, "submission.order_side")?,
            price: required_number(value.price, "submission.price")?,
            quantity: required_number(value.quantity, "submission.quantity")?,
            client_order_id: required_text(value.client_order_id, "submission.client_order_id")?,
        })
    }
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
