use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, BoltV3CapitalAdmissionRebuildAuditEvidence,
    BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
    facts::{
        CapitalAdmissionRebuildFact, CapitalAdmissionRebuildRejectionReason,
        ReservationProductKind, ReservationSide, SubmitReservationFillFact,
        SubmitReservationMetadataFact,
    },
    generated_contract::{KnownPurpose, current_identity_for_purpose, identity_metadata},
    sink::{EncodedEvidenceRecord, RecordError},
};

use crate::bolt_v3_capital_reservation::ReservationRejectionReason;

mod admission;
mod basket_admission;
mod entry_skip;
mod exit;
mod loss_halt;
mod order_intent;
mod order_lifecycle;
mod order_reject;
mod requote_throttle;
mod settlement;
mod strategy_input;
mod terminal_settlement;
mod venue_truth;

pub fn settlement_record_kind() -> &'static str {
    let identity = current_identity_for_purpose(KnownPurpose::Settlement);
    identity_metadata(identity).0
}

fn current_utc_ns() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("UTC timestamp must fit in i64 nanoseconds")
}

fn project_to_wire<T, W>(value: &T, label: &str) -> Result<W>
where
    T: Serialize,
    W: DeserializeOwned,
{
    let value = serde_json::to_value(value)
        .with_context(|| format!("failed to serialize runtime {label}"))?;
    serde_json::from_value(value).with_context(|| format!("runtime {label} changed wire shape"))
}

fn project_from_wire<W, T>(wire: &W, label: &str) -> Result<T>
where
    W: Serialize,
    T: DeserializeOwned,
{
    let value = serde_json::to_value(wire)
        .with_context(|| format!("failed to serialize frozen {label} wire DTO"))?;
    serde_json::from_value(value)
        .with_context(|| format!("failed to project frozen {label} wire DTO"))
}

pub(crate) use admission::{
    decode_admitted_entry_admission, decode_forced_reduction_admission,
    decode_rejected_entry_admission, decode_risk_reducing_exit_admission,
};
pub use admission::{
    encode_admitted_entry_admission, encode_forced_reduction_admission,
    encode_rejected_entry_admission, encode_risk_reducing_exit_admission,
};
pub(crate) use basket_admission::{
    decode_basket_admission_granted, decode_basket_admission_rejected,
};
pub use basket_admission::{encode_basket_admission_granted, encode_basket_admission_rejected};
pub(crate) use entry_skip::decode_entry_skip_observation;
pub use entry_skip::encode_entry_skip_observation;
pub(crate) use exit::{
    decode_exit_evaluation, decode_exit_hold_decision, decode_exit_submission_decision,
};
pub use exit::{
    encode_exit_evaluation, encode_exit_hold_decision, encode_exit_submission_decision,
};
pub(crate) use loss_halt::decode_loss_governor_halt;
pub use loss_halt::encode_loss_governor_halt;
pub(crate) use order_intent::{decode_entry_order_intent, decode_risk_reducing_exit_order_intent};
pub use order_intent::{encode_entry_order_intent, encode_risk_reducing_exit_order_intent};
pub(crate) use order_lifecycle::decode_order_lifecycle;
pub use order_lifecycle::encode_order_lifecycle;
pub(crate) use order_reject::decode_order_reject;
pub use order_reject::encode_order_reject;
pub(crate) use requote_throttle::decode_requote_throttle;
pub use requote_throttle::encode_requote_throttle;
pub(crate) use settlement::{decode_settlement, decode_settlement_booking_error};
pub use settlement::{encode_settlement, encode_settlement_booking_error};
pub(crate) use strategy_input::{
    decode_blocked_strategy_input_observation, decode_submit_linked_strategy_input_snapshot,
};
pub use strategy_input::{
    encode_blocked_strategy_input_observation, encode_submit_linked_strategy_input_snapshot,
};
pub(crate) use terminal_settlement::decode_terminal_settlement;
pub use terminal_settlement::encode_terminal_settlement;
pub(crate) use venue_truth::{decode_venue_truth_capture_failure, decode_venue_truth_divergence};
pub use venue_truth::{encode_venue_truth_capture_failure, encode_venue_truth_divergence};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapitalAdmissionRebuildV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    audit: CapitalAdmissionRebuildV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapitalAdmissionRebuildV1Wire {
    observed_at_ns: u64,
    source: String,
    observed_open_order_count: usize,
    all_open_orders_attributed: bool,
    accepted: bool,
    reason: Option<CapitalAdmissionRebuildRejectionReasonV1>,
    attempted_reservation_count: usize,
    recovered_reservation_count: usize,
    live_reserved_liability: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CapitalAdmissionRebuildRejectionReasonV1 {
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitReservationMetadataV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    metadata: SubmitReservationMetadataV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitReservationMetadataV1Wire {
    client_order_id: String,
    submit_reservation_id: String,
    venue_id: String,
    account_id: String,
    product_kind: ReservationProductKindV1,
    collateral_currency: String,
    capital_pool_id: String,
    collateral_group_id: String,
    instrument_id: String,
    side: ReservationSideV1,
    submitted_quantity: String,
    liability_factor: String,
    additive_liability: String,
    reserved_liability: String,
    observed_at_ns: u64,
    source: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitReservationFillV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    fill: SubmitReservationFillV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitReservationFillV1Wire {
    client_order_id: String,
    submit_reservation_id: String,
    trade_id: String,
    instrument_id: String,
    side: ReservationSideV1,
    fill_quantity: String,
    observed_at_ns: u64,
    reconciliation: bool,
    source: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReservationProductKindV1 {
    PredictionMarketBinary,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReservationSideV1 {
    Buy,
    Sell,
}

pub fn encode_submit_reservation_metadata(
    evidence: &BoltV3SubmitReservationMetadataEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_submit_reservation_metadata_at(evidence, positive_recorded_at_utc_ns()?)
}

pub fn encode_capital_admission_rebuild(
    evidence: &BoltV3CapitalAdmissionRebuildAuditEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_capital_admission_rebuild_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_capital_admission_rebuild_at(
    evidence: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::CapitalAdmissionRebuild;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "audit",
        "capital-admission rebuild identity has wrong payload member"
    );
    let line = CapitalAdmissionRebuildV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        audit: CapitalAdmissionRebuildV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "capital-admission rebuild")
}

fn encode_submit_reservation_metadata_at(
    evidence: &BoltV3SubmitReservationMetadataEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::SubmitReservationMetadata;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "metadata",
        "submit-reservation metadata identity has wrong payload member"
    );
    let line = SubmitReservationMetadataV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        metadata: SubmitReservationMetadataV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "submit-reservation metadata")
}

pub fn encode_submit_reservation_fill(
    evidence: &BoltV3SubmitReservationFillEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_submit_reservation_fill_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_submit_reservation_fill_at(
    evidence: &BoltV3SubmitReservationFillEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::SubmitReservationFill;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "fill",
        "submit-reservation fill identity has wrong payload member"
    );
    let line = SubmitReservationFillV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        fill: SubmitReservationFillV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "submit-reservation fill")
}

impl TryFrom<&BoltV3SubmitReservationMetadataEvidence> for SubmitReservationMetadataV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3SubmitReservationMetadataEvidence) -> Result<Self> {
        ensure!(value.observed_at_ns > 0, "observed_at_ns must be positive");
        Ok(Self {
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            submit_reservation_id: required_text(
                &value.submit_reservation_id,
                "submit_reservation_id",
            )?,
            venue_id: required_text(&value.venue_id, "venue_id")?,
            account_id: required_text(&value.account_id, "account_id")?,
            product_kind: match value.product_kind.as_str() {
                "prediction_market_binary" => ReservationProductKindV1::PredictionMarketBinary,
                other => anyhow::bail!("unsupported reservation product_kind `{other}`"),
            },
            collateral_currency: required_text(&value.collateral_currency, "collateral_currency")?,
            capital_pool_id: required_text(&value.capital_pool_id, "capital_pool_id")?,
            collateral_group_id: required_text(&value.collateral_group_id, "collateral_group_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            side: ReservationSideV1::try_from(value.side.as_str())?,
            submitted_quantity: canonical_decimal(
                &value.submitted_quantity,
                "submitted_quantity",
                DecimalRequirement::Positive,
            )?,
            liability_factor: canonical_decimal(
                &value.liability_factor,
                "liability_factor",
                DecimalRequirement::NonNegative,
            )?,
            additive_liability: canonical_decimal(
                &value.additive_liability,
                "additive_liability",
                DecimalRequirement::NonNegative,
            )?,
            reserved_liability: canonical_decimal(
                &value.reserved_liability,
                "reserved_liability",
                DecimalRequirement::NonNegative,
            )?,
            observed_at_ns: value.observed_at_ns,
            source: required_text(&value.source, "source")?,
        })
    }
}

impl TryFrom<&BoltV3CapitalAdmissionRebuildAuditEvidence> for CapitalAdmissionRebuildV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3CapitalAdmissionRebuildAuditEvidence) -> Result<Self> {
        ensure!(value.observed_at_ns > 0, "observed_at_ns must be positive");
        ensure!(
            value.accepted == value.reason.is_none(),
            "accepted capital-admission rebuild must have no rejection reason and rejected rebuild must have one"
        );
        ensure!(
            value.recovered_reservation_count <= value.attempted_reservation_count,
            "recovered_reservation_count cannot exceed attempted_reservation_count"
        );
        Ok(Self {
            observed_at_ns: value.observed_at_ns,
            source: required_text(&value.source, "source")?,
            observed_open_order_count: value.observed_open_order_count,
            all_open_orders_attributed: value.all_open_orders_attributed,
            accepted: value.accepted,
            reason: value
                .reason
                .map(CapitalAdmissionRebuildRejectionReasonV1::from),
            attempted_reservation_count: value.attempted_reservation_count,
            recovered_reservation_count: value.recovered_reservation_count,
            live_reserved_liability: canonical_decimal(
                &value.live_reserved_liability,
                "live_reserved_liability",
                DecimalRequirement::NonNegative,
            )?,
        })
    }
}

impl TryFrom<&BoltV3SubmitReservationFillEvidence> for SubmitReservationFillV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3SubmitReservationFillEvidence) -> Result<Self> {
        ensure!(value.observed_at_ns > 0, "observed_at_ns must be positive");
        Ok(Self {
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            submit_reservation_id: required_text(
                &value.submit_reservation_id,
                "submit_reservation_id",
            )?,
            trade_id: required_text(&value.trade_id, "trade_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            side: ReservationSideV1::try_from(value.side.as_str())?,
            fill_quantity: canonical_decimal(
                &value.fill_quantity,
                "fill_quantity",
                DecimalRequirement::Positive,
            )?,
            observed_at_ns: value.observed_at_ns,
            reconciliation: value.reconciliation,
            source: required_text(&value.source, "source")?,
        })
    }
}

impl TryFrom<&str> for ReservationSideV1 {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "buy" => Ok(Self::Buy),
            "sell" => Ok(Self::Sell),
            other => anyhow::bail!("unsupported reservation side `{other}`"),
        }
    }
}

impl From<ReservationProductKindV1> for ReservationProductKind {
    fn from(value: ReservationProductKindV1) -> Self {
        match value {
            ReservationProductKindV1::PredictionMarketBinary => Self::PredictionMarketBinary,
        }
    }
}

impl From<ReservationSideV1> for ReservationSide {
    fn from(value: ReservationSideV1) -> Self {
        match value {
            ReservationSideV1::Buy => Self::Buy,
            ReservationSideV1::Sell => Self::Sell,
        }
    }
}

impl From<ReservationRejectionReason> for CapitalAdmissionRebuildRejectionReasonV1 {
    fn from(value: ReservationRejectionReason) -> Self {
        match value {
            ReservationRejectionReason::MissingEvidence => Self::MissingEvidence,
            ReservationRejectionReason::StaleRequest => Self::StaleRequest,
            ReservationRejectionReason::PoolMismatch => Self::PoolMismatch,
            ReservationRejectionReason::OverBudget => Self::OverBudget,
            ReservationRejectionReason::InvalidRequest => Self::InvalidRequest,
            ReservationRejectionReason::CollateralGroupMismatch => Self::CollateralGroupMismatch,
            ReservationRejectionReason::DuplicateReservation => Self::DuplicateReservation,
            ReservationRejectionReason::UnknownReservation => Self::UnknownReservation,
            ReservationRejectionReason::UnknownRelease => Self::UnknownRelease,
            ReservationRejectionReason::ReconciliationRequired => Self::ReconciliationRequired,
        }
    }
}

impl From<CapitalAdmissionRebuildRejectionReasonV1> for CapitalAdmissionRebuildRejectionReason {
    fn from(value: CapitalAdmissionRebuildRejectionReasonV1) -> Self {
        match value {
            CapitalAdmissionRebuildRejectionReasonV1::MissingEvidence => Self::MissingEvidence,
            CapitalAdmissionRebuildRejectionReasonV1::StaleRequest => Self::StaleRequest,
            CapitalAdmissionRebuildRejectionReasonV1::PoolMismatch => Self::PoolMismatch,
            CapitalAdmissionRebuildRejectionReasonV1::OverBudget => Self::OverBudget,
            CapitalAdmissionRebuildRejectionReasonV1::InvalidRequest => Self::InvalidRequest,
            CapitalAdmissionRebuildRejectionReasonV1::CollateralGroupMismatch => {
                Self::CollateralGroupMismatch
            }
            CapitalAdmissionRebuildRejectionReasonV1::DuplicateReservation => {
                Self::DuplicateReservation
            }
            CapitalAdmissionRebuildRejectionReasonV1::UnknownReservation => {
                Self::UnknownReservation
            }
            CapitalAdmissionRebuildRejectionReasonV1::UnknownRelease => Self::UnknownRelease,
            CapitalAdmissionRebuildRejectionReasonV1::ReconciliationRequired => {
                Self::ReconciliationRequired
            }
        }
    }
}

pub(crate) fn decode_capital_admission_rebuild(line: &[u8]) -> Result<CapitalAdmissionRebuildFact> {
    let line: CapitalAdmissionRebuildV1Line = serde_json::from_slice(line)
        .context("failed to decode current capital-admission rebuild")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::CapitalAdmissionRebuild,
        "audit",
    )?;
    let audit = line.audit;
    ensure!(
        audit.accepted == audit.reason.is_none(),
        "accepted capital-admission rebuild must have no rejection reason and rejected rebuild must have one"
    );
    ensure!(
        audit.recovered_reservation_count <= audit.attempted_reservation_count,
        "recovered_reservation_count cannot exceed attempted_reservation_count"
    );
    Ok(CapitalAdmissionRebuildFact {
        observed_at_ns: positive_timestamp(audit.observed_at_ns, "observed_at_ns")?,
        source: required_text(&audit.source, "source")?,
        observed_open_order_count: audit.observed_open_order_count,
        all_open_orders_attributed: audit.all_open_orders_attributed,
        accepted: audit.accepted,
        reason: audit.reason.map(Into::into),
        attempted_reservation_count: audit.attempted_reservation_count,
        recovered_reservation_count: audit.recovered_reservation_count,
        live_reserved_liability: non_negative_decimal(
            &audit.live_reserved_liability,
            "live_reserved_liability",
        )?,
    })
}

pub(crate) fn decode_submit_reservation_metadata(
    line: &[u8],
) -> Result<SubmitReservationMetadataFact> {
    let line: SubmitReservationMetadataV1Line = serde_json::from_slice(line)
        .context("failed to decode current submit-reservation metadata")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::SubmitReservationMetadata,
        "metadata",
    )?;
    let metadata = line.metadata;
    Ok(SubmitReservationMetadataFact {
        client_order_id: required_text(&metadata.client_order_id, "client_order_id")?,
        submit_reservation_id: required_text(
            &metadata.submit_reservation_id,
            "submit_reservation_id",
        )?,
        venue_id: required_text(&metadata.venue_id, "venue_id")?,
        account_id: required_text(&metadata.account_id, "account_id")?,
        product_kind: metadata.product_kind.into(),
        collateral_currency: required_text(&metadata.collateral_currency, "collateral_currency")?,
        capital_pool_id: required_text(&metadata.capital_pool_id, "capital_pool_id")?,
        collateral_group_id: required_text(&metadata.collateral_group_id, "collateral_group_id")?,
        instrument_id: required_text(&metadata.instrument_id, "instrument_id")?,
        side: metadata.side.into(),
        submitted_quantity: positive_decimal(&metadata.submitted_quantity, "submitted_quantity")?,
        liability_factor: non_negative_decimal(&metadata.liability_factor, "liability_factor")?,
        additive_liability: non_negative_decimal(
            &metadata.additive_liability,
            "additive_liability",
        )?,
        reserved_liability: non_negative_decimal(
            &metadata.reserved_liability,
            "reserved_liability",
        )?,
        observed_at_ns: positive_timestamp(metadata.observed_at_ns, "observed_at_ns")?,
        source: required_text(&metadata.source, "source")?,
    })
}

pub(crate) fn decode_submit_reservation_fill(line: &[u8]) -> Result<SubmitReservationFillFact> {
    let line: SubmitReservationFillV1Line =
        serde_json::from_slice(line).context("failed to decode current submit-reservation fill")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::SubmitReservationFill,
        "fill",
    )?;
    let fill = line.fill;
    Ok(SubmitReservationFillFact {
        client_order_id: required_text(&fill.client_order_id, "client_order_id")?,
        submit_reservation_id: required_text(&fill.submit_reservation_id, "submit_reservation_id")?,
        trade_id: required_text(&fill.trade_id, "trade_id")?,
        instrument_id: required_text(&fill.instrument_id, "instrument_id")?,
        side: fill.side.into(),
        fill_quantity: positive_decimal(&fill.fill_quantity, "fill_quantity")?,
        observed_at_ns: positive_timestamp(fill.observed_at_ns, "observed_at_ns")?,
        reconciliation: fill.reconciliation,
        source: required_text(&fill.source, "source")?,
    })
}

#[derive(Clone, Copy)]
enum DecimalRequirement {
    Positive,
    NonNegative,
}

fn required_text(value: &str, field: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be non-empty and canonical"
    );
    Ok(value.to_string())
}

fn validate_current_header(
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &str,
    gate_version: &str,
    kind: &str,
    purpose: KnownPurpose,
    expected_payload_member: &str,
) -> Result<()> {
    let identity = current_identity_for_purpose(purpose);
    let (expected_kind, expected_schema_version, expected_gate_id, payload_member) =
        identity_metadata(identity);
    ensure!(
        payload_member == expected_payload_member,
        "identity payload member mismatch"
    );
    ensure!(
        schema_version == expected_schema_version,
        "schema_version mismatch"
    );
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    ensure!(gate_id == expected_gate_id, "gate_id mismatch");
    ensure!(
        !gate_version.is_empty() && gate_version.trim() == gate_version,
        "gate_version must be non-empty and canonical"
    );
    ensure!(kind == expected_kind, "kind mismatch");
    Ok(())
}

fn positive_timestamp(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}

fn positive_decimal(value: &str, field: &str) -> Result<Decimal> {
    let value = parse_decimal(value, field)?;
    ensure!(value > Decimal::ZERO, "`{field}` must be positive");
    Ok(value)
}

fn non_negative_decimal(value: &str, field: &str) -> Result<Decimal> {
    let value = parse_decimal(value, field)?;
    ensure!(value >= Decimal::ZERO, "`{field}` must be non-negative");
    Ok(value)
}

fn parse_decimal(value: &str, field: &str) -> Result<Decimal> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be canonical"
    );
    value
        .parse::<Decimal>()
        .with_context(|| format!("`{field}` must parse as decimal"))
}

fn canonical_decimal(value: &str, field: &str, requirement: DecimalRequirement) -> Result<String> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be canonical"
    );
    let value = value
        .parse::<Decimal>()
        .with_context(|| format!("`{field}` must parse as decimal"))?;
    match requirement {
        DecimalRequirement::Positive => {
            ensure!(value > Decimal::ZERO, "`{field}` must be positive")
        }
        DecimalRequirement::NonNegative => {
            ensure!(value >= Decimal::ZERO, "`{field}` must be non-negative")
        }
    }
    Ok(value.normalize().to_string())
}

fn positive_recorded_at_utc_ns() -> Result<i64> {
    let value = current_utc_ns();
    ensure!(value > 0, "recorded_at_utc_ns must be positive");
    Ok(value)
}

fn encode_record<T: Serialize>(
    line: &T,
    purpose: KnownPurpose,
    label: &str,
) -> Result<EncodedEvidenceRecord> {
    let mut bytes = serde_json::to_vec(line)
        .with_context(|| format!("failed to serialize current {label} evidence"))?;
    bytes.push(b'\n');
    EncodedEvidenceRecord::new(bytes, purpose).map_err(RecordError::into_anyhow)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn capital_rebuild() -> BoltV3CapitalAdmissionRebuildAuditEvidence {
        BoltV3CapitalAdmissionRebuildAuditEvidence {
            observed_at_ns: 3,
            source: "open_order_reconciliation".to_string(),
            observed_open_order_count: 3,
            all_open_orders_attributed: false,
            accepted: false,
            reason: Some(ReservationRejectionReason::DuplicateReservation),
            attempted_reservation_count: 3,
            recovered_reservation_count: 1,
            live_reserved_liability: "4.30".to_string(),
        }
    }

    fn metadata() -> BoltV3SubmitReservationMetadataEvidence {
        BoltV3SubmitReservationMetadataEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "client-order-1#1".to_string(),
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "PUSD".to_string(),
            capital_pool_id: "pool-a".to_string(),
            collateral_group_id: "condition-a".to_string(),
            instrument_id: "condition-a-yes.POLYMARKET".to_string(),
            side: "buy".to_string(),
            submitted_quantity: "10.00".to_string(),
            liability_factor: "0.40".to_string(),
            additive_liability: "0.30".to_string(),
            reserved_liability: "4.30".to_string(),
            observed_at_ns: 1,
            source: "submit_admission".to_string(),
        }
    }

    fn fill() -> BoltV3SubmitReservationFillEvidence {
        BoltV3SubmitReservationFillEvidence {
            client_order_id: "client-order-1".to_string(),
            submit_reservation_id: "client-order-1#1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "condition-a-yes.POLYMARKET".to_string(),
            side: "sell".to_string(),
            fill_quantity: "3.00".to_string(),
            observed_at_ns: 2,
            reconciliation: false,
            source: "nt_partial_fill_revalue".to_string(),
        }
    }

    #[test]
    fn current_reservation_encoders_use_fresh_registered_identities() {
        let metadata = encode_submit_reservation_metadata_at(&metadata(), 1)
            .expect("valid metadata should encode");
        let fill = encode_submit_reservation_fill_at(&fill(), 2).expect("valid fill should encode");

        assert_eq!(
            metadata.bytes(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_reservation_metadata_v1.jsonl"
            ))
        );
        assert_eq!(
            fill.bytes(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_reservation_fill_v1.jsonl"
            ))
        );

        let metadata: Value =
            serde_json::from_slice(metadata.bytes()).expect("metadata record should be JSON");
        let fill: Value = serde_json::from_slice(fill.bytes()).expect("fill record should be JSON");
        assert_eq!(metadata["kind"], "submit_reservation_metadata");
        assert_eq!(metadata["schema_version"], 16);
        assert_eq!(metadata["metadata"]["submitted_quantity"], "10");
        assert_eq!(metadata["metadata"]["reserved_liability"], "4.3");
        assert_eq!(fill["kind"], "submit_reservation_fill");
        assert_eq!(fill["schema_version"], 16);
        assert_eq!(fill["fill"]["fill_quantity"], "3");

        let decoded_metadata = crate::bolt_v3_decision_evidence::decode::decode_registered_line(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_reservation_metadata_v1.jsonl"
            )),
        )
        .expect("current metadata fixture should decode by exact identity");
        let decoded_fill = crate::bolt_v3_decision_evidence::decode::decode_registered_line(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_reservation_fill_v1.jsonl"
            )),
        )
        .expect("current fill fixture should decode by exact identity");
        assert!(matches!(
            decoded_metadata,
            crate::bolt_v3_decision_evidence::facts::DecodedFact::SubmitReservationMetadata(_)
        ));
        assert!(matches!(
            decoded_fill,
            crate::bolt_v3_decision_evidence::facts::DecodedFact::SubmitReservationFill(_)
        ));
    }

    #[test]
    fn current_capital_rebuild_codec_uses_fresh_registered_identity() {
        let encoded = encode_capital_admission_rebuild_at(&capital_rebuild(), 3)
            .expect("valid capital-admission rebuild should encode");
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/capital_admission_rebuild_v1.jsonl"
        ));
        assert_eq!(encoded.bytes(), fixture);

        let decoded = crate::bolt_v3_decision_evidence::decode::decode_registered_line(fixture)
            .expect("current capital-admission rebuild fixture should decode by exact identity");
        let crate::bolt_v3_decision_evidence::facts::DecodedFact::CapitalAdmissionRebuild(decoded) =
            decoded
        else {
            panic!("capital-admission rebuild fixture decoded to wrong fact");
        };
        assert_eq!(decoded.observed_at_ns, 3);
        assert!(!decoded.accepted);
        assert_eq!(
            decoded.reason,
            Some(CapitalAdmissionRebuildRejectionReason::DuplicateReservation)
        );
        assert_eq!(decoded.live_reserved_liability, Decimal::new(43, 1));
    }

    #[test]
    fn current_codecs_reject_invalid_semantic_input() {
        let mut metadata = metadata();
        metadata.product_kind = "other".to_string();
        assert!(encode_submit_reservation_metadata(&metadata).is_err());

        let mut fill = fill();
        fill.fill_quantity = "0".to_string();
        assert!(encode_submit_reservation_fill(&fill).is_err());

        let mut rebuild = capital_rebuild();
        rebuild.accepted = true;
        assert!(encode_capital_admission_rebuild(&rebuild).is_err());
    }

    #[test]
    fn current_codecs_reject_unknown_missing_or_inconsistent_fields() {
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_reservation_metadata_v1.jsonl"
        ));
        let mut value: Value =
            serde_json::from_slice(fixture).expect("current metadata fixture should parse");
        value
            .as_object_mut()
            .expect("current metadata line should be an object")
            .insert("unexpected".to_string(), serde_json::json!(true));
        let bytes = serde_json::to_vec(&value).expect("mutated current line should serialize");
        assert!(crate::bolt_v3_decision_evidence::decode::decode_registered_line(&bytes).is_err());

        let mut value: Value =
            serde_json::from_slice(fixture).expect("current metadata fixture should parse");
        value["metadata"]
            .as_object_mut()
            .expect("current metadata payload should be an object")
            .remove("client_order_id");
        let bytes = serde_json::to_vec(&value).expect("mutated current line should serialize");
        assert!(crate::bolt_v3_decision_evidence::decode::decode_registered_line(&bytes).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/capital_admission_rebuild_v1.jsonl"
        ));
        let mut value: Value =
            serde_json::from_slice(fixture).expect("current rebuild fixture should parse");
        value["audit"]["recovered_reservation_count"] = serde_json::json!(4);
        let bytes = serde_json::to_vec(&value).expect("mutated current line should serialize");
        assert!(crate::bolt_v3_decision_evidence::decode::decode_registered_line(&bytes).is_err());
    }
}
