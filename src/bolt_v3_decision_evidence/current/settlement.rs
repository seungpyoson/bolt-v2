use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3OutcomeSide, BoltV3SettlementBookingErrorEvidence, BoltV3SettlementBookingErrorReason,
    BoltV3SettlementEvidence,
    facts::{
        SettlementBookingErrorFact, SettlementBookingErrorReason, SettlementFact,
        SettlementOrderSide, SettlementOutcomeSide,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    settlement: SettlementV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementBookingErrorV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    settlement_booking_error: SettlementBookingErrorV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementV1Wire {
    strategy_id: String,
    settlement_key: String,
    market_id: String,
    position_id: String,
    instrument_id: String,
    product_id: String,
    outcome_side: SettlementOutcomeSideV1,
    entry_order_side: SettlementOrderSideV1,
    quantity: String,
    entry_price: String,
    family_key: String,
    strike_price: String,
    resolution_instrument_id: String,
    resolution_ts_event_ns: u64,
    reference_close_price: String,
    payout_per_share: String,
    terminal_value: String,
    realized_pnl: String,
    settlement_currency: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettlementBookingErrorV1Wire {
    strategy_id: String,
    settlement_key: String,
    market_id: Option<String>,
    position_id: Option<String>,
    instrument_id: Option<String>,
    resolution_instrument_id: Option<String>,
    reason: SettlementBookingErrorReasonV1,
    detail: String,
    observed_at_ns: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettlementOutcomeSideV1 {
    Up,
    Down,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettlementOrderSideV1 {
    Buy,
    Sell,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettlementBookingErrorReasonV1 {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

pub fn encode_settlement(evidence: &BoltV3SettlementEvidence) -> Result<EncodedEvidenceRecord> {
    encode_settlement_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_settlement_at(
    evidence: &BoltV3SettlementEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let purpose = KnownPurpose::Settlement;
    let (kind, schema_version, gate_id) =
        current_line_metadata(purpose, recorded_at_utc_ns, "settlement")?;
    encode_record(
        &SettlementV1Line {
            schema_version,
            recorded_at_utc_ns,
            gate_id: gate_id.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: kind.to_string(),
            settlement: SettlementV1Wire::try_from(evidence)?,
        },
        purpose,
        "settlement",
    )
}

pub fn encode_settlement_booking_error(
    evidence: &BoltV3SettlementBookingErrorEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_settlement_booking_error_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_settlement_booking_error_at(
    evidence: &BoltV3SettlementBookingErrorEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let purpose = KnownPurpose::SettlementBookingError;
    let (kind, schema_version, gate_id) =
        current_line_metadata(purpose, recorded_at_utc_ns, "settlement_booking_error")?;
    encode_record(
        &SettlementBookingErrorV1Line {
            schema_version,
            recorded_at_utc_ns,
            gate_id: gate_id.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: kind.to_string(),
            settlement_booking_error: SettlementBookingErrorV1Wire::try_from(evidence)?,
        },
        purpose,
        "settlement booking error",
    )
}

fn current_line_metadata(
    purpose: KnownPurpose,
    recorded_at_utc_ns: i64,
    expected_payload_member: &str,
) -> Result<(&'static str, u32, &'static str)> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == expected_payload_member,
        "settlement identity has wrong payload member"
    );
    Ok((kind, schema_version, gate_id))
}

impl TryFrom<&BoltV3SettlementEvidence> for SettlementV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3SettlementEvidence) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            settlement_key: required_text(&value.settlement_key, "settlement_key")?,
            market_id: required_text(&value.market_id, "market_id")?,
            position_id: required_text(&value.position_id, "position_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            product_id: required_text(&value.product_id, "product_id")?,
            outcome_side: value.outcome_side.into(),
            entry_order_side: SettlementOrderSideV1::try_from(value.entry_order_side.as_str())?,
            quantity: decimal(&value.quantity, "quantity", DecimalRule::Positive)?,
            entry_price: decimal(&value.entry_price, "entry_price", DecimalRule::NonNegative)?,
            family_key: required_text(&value.family_key, "family_key")?,
            strike_price: decimal(&value.strike_price, "strike_price", DecimalRule::Positive)?,
            resolution_instrument_id: required_text(
                &value.resolution_instrument_id,
                "resolution_instrument_id",
            )?,
            resolution_ts_event_ns: positive(
                value.resolution_ts_event_ns,
                "resolution_ts_event_ns",
            )?,
            reference_close_price: decimal(
                &value.reference_close_price,
                "reference_close_price",
                DecimalRule::Positive,
            )?,
            payout_per_share: decimal(
                &value.payout_per_share,
                "payout_per_share",
                DecimalRule::NonNegative,
            )?,
            terminal_value: decimal(
                &value.terminal_value,
                "terminal_value",
                DecimalRule::NonNegative,
            )?,
            realized_pnl: decimal(&value.realized_pnl, "realized_pnl", DecimalRule::Any)?,
            settlement_currency: required_text(&value.settlement_currency, "settlement_currency")?,
        })
    }
}

impl TryFrom<&BoltV3SettlementBookingErrorEvidence> for SettlementBookingErrorV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3SettlementBookingErrorEvidence) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            settlement_key: required_text(&value.settlement_key, "settlement_key")?,
            market_id: optional_text(value.market_id.as_deref(), "market_id")?,
            position_id: optional_text(value.position_id.as_deref(), "position_id")?,
            instrument_id: optional_text(value.instrument_id.as_deref(), "instrument_id")?,
            resolution_instrument_id: optional_text(
                value.resolution_instrument_id.as_deref(),
                "resolution_instrument_id",
            )?,
            reason: value.reason.into(),
            detail: required_text(&value.detail, "detail")?,
            observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
        })
    }
}

impl From<BoltV3OutcomeSide> for SettlementOutcomeSideV1 {
    fn from(value: BoltV3OutcomeSide) -> Self {
        match value {
            BoltV3OutcomeSide::Up => Self::Up,
            BoltV3OutcomeSide::Down => Self::Down,
        }
    }
}

impl TryFrom<&str> for SettlementOrderSideV1 {
    type Error = anyhow::Error;
    fn try_from(value: &str) -> Result<Self> {
        match value {
            "Buy" => Ok(Self::Buy),
            "Sell" => Ok(Self::Sell),
            _ => anyhow::bail!("entry_order_side must be Buy or Sell"),
        }
    }
}

impl From<BoltV3SettlementBookingErrorReason> for SettlementBookingErrorReasonV1 {
    fn from(value: BoltV3SettlementBookingErrorReason) -> Self {
        match value {
            BoltV3SettlementBookingErrorReason::ResolutionFeedMissing => {
                Self::ResolutionFeedMissing
            }
            BoltV3SettlementBookingErrorReason::SettlementAlreadyBooked => {
                Self::SettlementAlreadyBooked
            }
            BoltV3SettlementBookingErrorReason::SettlementInputInvalid => {
                Self::SettlementInputInvalid
            }
            BoltV3SettlementBookingErrorReason::SettlementBlocked => Self::SettlementBlocked,
        }
    }
}

pub(crate) fn decode_settlement(line: &[u8]) -> Result<SettlementFact> {
    let line: SettlementV1Line =
        serde_json::from_slice(line).context("failed to decode current settlement")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::Settlement,
        "settlement",
    )?;
    line.settlement.decode()
}

pub(crate) fn decode_settlement_booking_error(line: &[u8]) -> Result<SettlementBookingErrorFact> {
    let line: SettlementBookingErrorV1Line = serde_json::from_slice(line)
        .context("failed to decode current settlement booking error")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::SettlementBookingError,
        "settlement_booking_error",
    )?;
    line.settlement_booking_error.decode()
}

impl SettlementV1Wire {
    fn decode(self) -> Result<SettlementFact> {
        Ok(SettlementFact {
            strategy_id: required_text(&self.strategy_id, "strategy_id")?,
            settlement_key: required_text(&self.settlement_key, "settlement_key")?,
            market_id: required_text(&self.market_id, "market_id")?,
            position_id: required_text(&self.position_id, "position_id")?,
            instrument_id: required_text(&self.instrument_id, "instrument_id")?,
            product_id: required_text(&self.product_id, "product_id")?,
            outcome_side: self.outcome_side.into(),
            entry_order_side: self.entry_order_side.into(),
            quantity: decimal_value(&self.quantity, "quantity", DecimalRule::Positive)?,
            entry_price: decimal_value(&self.entry_price, "entry_price", DecimalRule::NonNegative)?,
            family_key: required_text(&self.family_key, "family_key")?,
            strike_price: decimal_value(&self.strike_price, "strike_price", DecimalRule::Positive)?,
            resolution_instrument_id: required_text(
                &self.resolution_instrument_id,
                "resolution_instrument_id",
            )?,
            resolution_ts_event_ns: positive(
                self.resolution_ts_event_ns,
                "resolution_ts_event_ns",
            )?,
            reference_close_price: decimal_value(
                &self.reference_close_price,
                "reference_close_price",
                DecimalRule::Positive,
            )?,
            payout_per_share: decimal_value(
                &self.payout_per_share,
                "payout_per_share",
                DecimalRule::NonNegative,
            )?,
            terminal_value: decimal_value(
                &self.terminal_value,
                "terminal_value",
                DecimalRule::NonNegative,
            )?,
            realized_pnl: decimal_value(&self.realized_pnl, "realized_pnl", DecimalRule::Any)?,
            settlement_currency: required_text(&self.settlement_currency, "settlement_currency")?,
        })
    }
}

impl SettlementBookingErrorV1Wire {
    fn decode(self) -> Result<SettlementBookingErrorFact> {
        Ok(SettlementBookingErrorFact {
            strategy_id: required_text(&self.strategy_id, "strategy_id")?,
            settlement_key: required_text(&self.settlement_key, "settlement_key")?,
            market_id: optional_text(self.market_id.as_deref(), "market_id")?,
            position_id: optional_text(self.position_id.as_deref(), "position_id")?,
            instrument_id: optional_text(self.instrument_id.as_deref(), "instrument_id")?,
            resolution_instrument_id: optional_text(
                self.resolution_instrument_id.as_deref(),
                "resolution_instrument_id",
            )?,
            reason: self.reason.into(),
            detail: required_text(&self.detail, "detail")?,
            observed_at_ns: positive(self.observed_at_ns, "observed_at_ns")?,
        })
    }
}

impl From<SettlementOutcomeSideV1> for SettlementOutcomeSide {
    fn from(value: SettlementOutcomeSideV1) -> Self {
        match value {
            SettlementOutcomeSideV1::Up => Self::Up,
            SettlementOutcomeSideV1::Down => Self::Down,
        }
    }
}
impl From<SettlementOrderSideV1> for SettlementOrderSide {
    fn from(value: SettlementOrderSideV1) -> Self {
        match value {
            SettlementOrderSideV1::Buy => Self::Buy,
            SettlementOrderSideV1::Sell => Self::Sell,
        }
    }
}
impl From<SettlementBookingErrorReasonV1> for SettlementBookingErrorReason {
    fn from(value: SettlementBookingErrorReasonV1) -> Self {
        match value {
            SettlementBookingErrorReasonV1::ResolutionFeedMissing => Self::ResolutionFeedMissing,
            SettlementBookingErrorReasonV1::SettlementAlreadyBooked => {
                Self::SettlementAlreadyBooked
            }
            SettlementBookingErrorReasonV1::SettlementInputInvalid => Self::SettlementInputInvalid,
            SettlementBookingErrorReasonV1::SettlementBlocked => Self::SettlementBlocked,
        }
    }
}

#[derive(Clone, Copy)]
enum DecimalRule {
    Any,
    NonNegative,
    Positive,
}

fn decimal(value: &str, field: &str, rule: DecimalRule) -> Result<String> {
    decimal_value(value, field, rule).map(|value| value.normalize().to_string())
}
fn decimal_value(value: &str, field: &str, rule: DecimalRule) -> Result<Decimal> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be canonical"
    );
    let value = value
        .parse::<Decimal>()
        .with_context(|| format!("`{field}` must parse as decimal"))?;
    match rule {
        DecimalRule::Any => {}
        DecimalRule::NonNegative => {
            ensure!(value >= Decimal::ZERO, "`{field}` must be non-negative")
        }
        DecimalRule::Positive => ensure!(value > Decimal::ZERO, "`{field}` must be positive"),
    }
    Ok(value)
}
fn positive(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}
fn optional_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_text(value, field)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};

    fn settlement() -> BoltV3SettlementEvidence {
        BoltV3SettlementEvidence {
            strategy_id: "BINARYORACLEEDGETAKER-001".into(),
            settlement_key: "settlement-1".into(),
            market_id: "MKT-1".into(),
            position_id: "P-1".into(),
            instrument_id: "condition-MKT-1-UP.POLYMARKET".into(),
            product_id: "condition-MKT-1-UP".into(),
            outcome_side: BoltV3OutcomeSide::Up,
            entry_order_side: "Buy".into(),
            quantity: "10.00".into(),
            entry_price: "0.45".into(),
            family_key: "updown".into(),
            strike_price: "3100.0".into(),
            resolution_instrument_id: "RESOLUTION.SOURCE".into(),
            resolution_ts_event_ns: 1_300_000_000,
            reference_close_price: "3101.0".into(),
            payout_per_share: "1".into(),
            terminal_value: "10".into(),
            realized_pnl: "5.5".into(),
            settlement_currency: "USDC".into(),
        }
    }
    fn booking_error() -> BoltV3SettlementBookingErrorEvidence {
        BoltV3SettlementBookingErrorEvidence {
            strategy_id: "BINARYORACLEEDGETAKER-001".into(),
            settlement_key: "settlement-1".into(),
            market_id: Some("MKT-1".into()),
            position_id: Some("P-1".into()),
            instrument_id: Some("condition-MKT-1-UP.POLYMARKET".into()),
            resolution_instrument_id: Some("RESOLUTION.SOURCE".into()),
            reason: BoltV3SettlementBookingErrorReason::ResolutionFeedMissing,
            detail: "resolution feed missing at market end; settlement not booked".into(),
            observed_at_ns: 1_300_000_000,
        }
    }

    #[test]
    fn current_settlement_codecs_are_byte_exact_and_decodable() {
        let settlement = encode_settlement_at(&settlement(), 8).expect("settlement should encode");
        let error = encode_settlement_booking_error_at(&booking_error(), 9)
            .expect("booking error should encode");
        let settlement_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/settlement_v1.jsonl"
        ));
        let error_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/settlement_booking_error_v1.jsonl"
        ));
        assert_eq!(settlement.bytes(), settlement_fixture);
        assert_eq!(error.bytes(), error_fixture);
        assert!(matches!(
            decode_registered_line(settlement_fixture).unwrap(),
            DecodedFact::Settlement(_)
        ));
        assert!(matches!(
            decode_registered_line(error_fixture).unwrap(),
            DecodedFact::SettlementBookingError(_)
        ));
    }

    #[test]
    fn current_booking_error_rejects_unknown_fields() {
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/settlement_booking_error_v1.jsonl"
        ));
        let mut value: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        value["settlement_booking_error"]["terminal_lifecycle"] = serde_json::json!({});
        assert!(decode_registered_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
