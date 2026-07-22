use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3OrderLifecycleEvidence, BoltV3OrderLifecycleOutcome, BoltV3OrderLifecycleTransition,
    BoltV3SettlementBookingErrorEvidence, BoltV3SettlementBookingErrorReason,
    BoltV3TerminalSettlementEvidence,
    facts::{
        OrderLifecycleFact, OrderLifecycleOutcome, OrderLifecycleSide, OrderLifecycleTransition,
        SettlementBookingErrorFact, SettlementBookingErrorReason, TerminalSettlementFact,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalSettlementV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    terminal_settlement: TerminalSettlementV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalSettlementV1Wire {
    settlement_key: String,
    booking_error: Option<TerminalBookingErrorV1Wire>,
    lifecycle: TerminalLifecycleV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalBookingErrorV1Wire {
    strategy_id: String,
    settlement_key: String,
    market_id: Option<String>,
    position_id: Option<String>,
    instrument_id: Option<String>,
    resolution_instrument_id: Option<String>,
    reason: TerminalBookingErrorReasonV1,
    detail: String,
    observed_at_ns: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerminalLifecycleV1Wire {
    strategy_id: String,
    transition: TerminalLifecycleTransitionV1,
    outcome: TerminalLifecycleOutcomeV1,
    source: String,
    market_id: Option<String>,
    instrument_id: Option<String>,
    position_id: Option<String>,
    client_order_id: Option<String>,
    prior_client_order_id: Option<String>,
    raw_reason_text: Option<String>,
    order_side: Option<TerminalLifecycleSideV1>,
    filled_quantity: Option<String>,
    residual_quantity: Option<String>,
    ts_event_ns: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalBookingErrorReasonV1 {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalLifecycleTransitionV1 {
    SettlementBookingTerminal,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalLifecycleOutcomeV1 {
    Flat,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalLifecycleSideV1 {
    Buy,
    Sell,
}

pub fn encode_terminal_settlement(
    evidence: &BoltV3TerminalSettlementEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_terminal_settlement_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_terminal_settlement_at(
    evidence: &BoltV3TerminalSettlementEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::TerminalSettlement;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "terminal_settlement",
        "terminal-settlement identity has wrong payload member"
    );
    let wire = TerminalSettlementV1Wire::try_from(evidence)?;
    validate_relationships(&wire)?;
    encode_record(
        &TerminalSettlementV1Line {
            schema_version,
            recorded_at_utc_ns,
            gate_id: gate_id.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: kind.to_string(),
            terminal_settlement: wire,
        },
        purpose,
        "terminal settlement",
    )
}

impl TryFrom<&BoltV3TerminalSettlementEvidence> for TerminalSettlementV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3TerminalSettlementEvidence) -> Result<Self> {
        Ok(Self {
            settlement_key: required_text(&value.settlement_key, "settlement_key")?,
            booking_error: value
                .booking_error
                .as_ref()
                .map(TerminalBookingErrorV1Wire::try_from)
                .transpose()?,
            lifecycle: TerminalLifecycleV1Wire::try_from(&value.lifecycle)?,
        })
    }
}

impl TryFrom<&BoltV3SettlementBookingErrorEvidence> for TerminalBookingErrorV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3SettlementBookingErrorEvidence) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            settlement_key: required_text(&value.settlement_key, "booking_error.settlement_key")?,
            market_id: optional_text(value.market_id.as_deref(), "booking_error.market_id")?,
            position_id: optional_text(value.position_id.as_deref(), "booking_error.position_id")?,
            instrument_id: optional_text(
                value.instrument_id.as_deref(),
                "booking_error.instrument_id",
            )?,
            resolution_instrument_id: optional_text(
                value.resolution_instrument_id.as_deref(),
                "booking_error.resolution_instrument_id",
            )?,
            reason: value.reason.into(),
            detail: required_text(&value.detail, "booking_error.detail")?,
            observed_at_ns: positive(value.observed_at_ns, "booking_error.observed_at_ns")?,
        })
    }
}

impl TryFrom<&BoltV3OrderLifecycleEvidence> for TerminalLifecycleV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3OrderLifecycleEvidence) -> Result<Self> {
        ensure!(
            value.transition == BoltV3OrderLifecycleTransition::SettlementBookingTerminal,
            "terminal settlement requires settlement-booking-terminal lifecycle"
        );
        ensure!(
            value.outcome == BoltV3OrderLifecycleOutcome::Flat,
            "terminal settlement requires flat lifecycle outcome"
        );
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "lifecycle.strategy_id")?,
            transition: TerminalLifecycleTransitionV1::SettlementBookingTerminal,
            outcome: TerminalLifecycleOutcomeV1::Flat,
            source: required_text(&value.source, "lifecycle.source")?,
            market_id: optional_text(value.market_id.as_deref(), "lifecycle.market_id")?,
            instrument_id: optional_text(
                value.instrument_id.as_deref(),
                "lifecycle.instrument_id",
            )?,
            position_id: optional_text(value.position_id.as_deref(), "lifecycle.position_id")?,
            client_order_id: optional_text(
                value.client_order_id.as_deref(),
                "lifecycle.client_order_id",
            )?,
            prior_client_order_id: optional_text(
                value.prior_client_order_id.as_deref(),
                "lifecycle.prior_client_order_id",
            )?,
            raw_reason_text: optional_text(
                value.raw_reason_text.as_deref(),
                "lifecycle.raw_reason_text",
            )?,
            order_side: value
                .order_side
                .as_deref()
                .map(TerminalLifecycleSideV1::try_from)
                .transpose()?,
            filled_quantity: optional_positive_decimal(
                value.filled_quantity.as_deref(),
                "lifecycle.filled_quantity",
            )?,
            residual_quantity: optional_positive_decimal(
                value.residual_quantity.as_deref(),
                "lifecycle.residual_quantity",
            )?,
            ts_event_ns: value
                .ts_event_ns
                .map(|value| positive(value, "lifecycle.ts_event_ns"))
                .transpose()?,
        })
    }
}

impl From<BoltV3SettlementBookingErrorReason> for TerminalBookingErrorReasonV1 {
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

impl TryFrom<&str> for TerminalLifecycleSideV1 {
    type Error = anyhow::Error;
    fn try_from(value: &str) -> Result<Self> {
        match value {
            "Buy" => Ok(Self::Buy),
            "Sell" => Ok(Self::Sell),
            _ => anyhow::bail!("lifecycle.order_side must be Buy or Sell"),
        }
    }
}

pub(crate) fn decode_terminal_settlement(line: &[u8]) -> Result<TerminalSettlementFact> {
    let line: TerminalSettlementV1Line =
        serde_json::from_slice(line).context("failed to decode current terminal settlement")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::TerminalSettlement,
        "terminal_settlement",
    )?;
    validate_relationships(&line.terminal_settlement)?;
    line.terminal_settlement.decode()
}

impl TerminalSettlementV1Wire {
    fn decode(self) -> Result<TerminalSettlementFact> {
        Ok(TerminalSettlementFact {
            settlement_key: required_text(&self.settlement_key, "settlement_key")?,
            booking_error: self
                .booking_error
                .map(TerminalBookingErrorV1Wire::decode)
                .transpose()?,
            lifecycle: self.lifecycle.decode()?,
        })
    }
}

impl TerminalBookingErrorV1Wire {
    fn decode(self) -> Result<SettlementBookingErrorFact> {
        Ok(SettlementBookingErrorFact {
            strategy_id: required_text(&self.strategy_id, "booking_error.strategy_id")?,
            settlement_key: required_text(&self.settlement_key, "booking_error.settlement_key")?,
            market_id: optional_text(self.market_id.as_deref(), "booking_error.market_id")?,
            position_id: optional_text(self.position_id.as_deref(), "booking_error.position_id")?,
            instrument_id: optional_text(
                self.instrument_id.as_deref(),
                "booking_error.instrument_id",
            )?,
            resolution_instrument_id: optional_text(
                self.resolution_instrument_id.as_deref(),
                "booking_error.resolution_instrument_id",
            )?,
            reason: self.reason.into(),
            detail: required_text(&self.detail, "booking_error.detail")?,
            observed_at_ns: positive(self.observed_at_ns, "booking_error.observed_at_ns")?,
        })
    }
}

impl TerminalLifecycleV1Wire {
    fn decode(self) -> Result<OrderLifecycleFact> {
        Ok(OrderLifecycleFact {
            strategy_id: required_text(&self.strategy_id, "lifecycle.strategy_id")?,
            transition: match self.transition {
                TerminalLifecycleTransitionV1::SettlementBookingTerminal => {
                    OrderLifecycleTransition::SettlementBookingTerminal
                }
            },
            outcome: match self.outcome {
                TerminalLifecycleOutcomeV1::Flat => OrderLifecycleOutcome::Flat,
            },
            source: required_text(&self.source, "lifecycle.source")?,
            market_id: optional_text(self.market_id.as_deref(), "lifecycle.market_id")?,
            instrument_id: optional_text(self.instrument_id.as_deref(), "lifecycle.instrument_id")?,
            position_id: optional_text(self.position_id.as_deref(), "lifecycle.position_id")?,
            client_order_id: optional_text(
                self.client_order_id.as_deref(),
                "lifecycle.client_order_id",
            )?,
            prior_client_order_id: optional_text(
                self.prior_client_order_id.as_deref(),
                "lifecycle.prior_client_order_id",
            )?,
            raw_reason_text: optional_text(
                self.raw_reason_text.as_deref(),
                "lifecycle.raw_reason_text",
            )?,
            order_side: self.order_side.map(Into::into),
            filled_quantity: optional_positive_decimal_value(
                self.filled_quantity.as_deref(),
                "lifecycle.filled_quantity",
            )?,
            residual_quantity: optional_positive_decimal_value(
                self.residual_quantity.as_deref(),
                "lifecycle.residual_quantity",
            )?,
            ts_event_ns: self
                .ts_event_ns
                .map(|value| positive(value, "lifecycle.ts_event_ns"))
                .transpose()?,
        })
    }
}

impl From<TerminalBookingErrorReasonV1> for SettlementBookingErrorReason {
    fn from(value: TerminalBookingErrorReasonV1) -> Self {
        match value {
            TerminalBookingErrorReasonV1::ResolutionFeedMissing => Self::ResolutionFeedMissing,
            TerminalBookingErrorReasonV1::SettlementAlreadyBooked => Self::SettlementAlreadyBooked,
            TerminalBookingErrorReasonV1::SettlementInputInvalid => Self::SettlementInputInvalid,
            TerminalBookingErrorReasonV1::SettlementBlocked => Self::SettlementBlocked,
        }
    }
}
impl From<TerminalLifecycleSideV1> for OrderLifecycleSide {
    fn from(value: TerminalLifecycleSideV1) -> Self {
        match value {
            TerminalLifecycleSideV1::Buy => Self::Buy,
            TerminalLifecycleSideV1::Sell => Self::Sell,
        }
    }
}

fn validate_relationships(value: &TerminalSettlementV1Wire) -> Result<()> {
    let settlement_key = required_text(&value.settlement_key, "settlement_key")?;
    if let Some(error) = value.booking_error.as_ref() {
        ensure!(
            error.settlement_key == settlement_key,
            "terminal settlement booking-error key does not match canonical key"
        );
        ensure!(
            error.strategy_id == value.lifecycle.strategy_id,
            "terminal settlement strategy IDs do not match"
        );
        matching_optional(&error.market_id, &value.lifecycle.market_id, "market_id")?;
        matching_optional(
            &error.position_id,
            &value.lifecycle.position_id,
            "position_id",
        )?;
        matching_optional(
            &error.instrument_id,
            &value.lifecycle.instrument_id,
            "instrument_id",
        )?;
    }
    Ok(())
}

fn matching_optional(left: &Option<String>, right: &Option<String>, field: &str) -> Result<()> {
    if let (Some(left), Some(right)) = (left, right) {
        ensure!(
            left == right,
            "terminal settlement {field} values do not match"
        );
    }
    Ok(())
}
fn optional_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_text(value, field)).transpose()
}
fn positive(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}
fn optional_positive_decimal(value: Option<&str>, field: &str) -> Result<Option<String>> {
    optional_positive_decimal_value(value, field)
        .map(|value| value.map(|value| value.normalize().to_string()))
}
fn optional_positive_decimal_value(value: Option<&str>, field: &str) -> Result<Option<Decimal>> {
    value
        .map(|value| {
            ensure!(
                !value.is_empty() && value.trim() == value,
                "`{field}` must be canonical"
            );
            let value = value
                .parse::<Decimal>()
                .with_context(|| format!("`{field}` must parse as decimal"))?;
            ensure!(value > Decimal::ZERO, "`{field}` must be positive");
            Ok(value)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};

    fn lifecycle() -> BoltV3OrderLifecycleEvidence {
        BoltV3OrderLifecycleEvidence {
            strategy_id: "BINARYORACLEEDGETAKER-001".into(),
            transition: BoltV3OrderLifecycleTransition::SettlementBookingTerminal,
            outcome: BoltV3OrderLifecycleOutcome::Flat,
            source: "settlement_booking_terminal".into(),
            market_id: Some("MKT-1".into()),
            instrument_id: Some("condition-MKT-1-UP.POLYMARKET".into()),
            position_id: Some("P-1".into()),
            client_order_id: None,
            prior_client_order_id: None,
            raw_reason_text: Some("settlement booking terminal".into()),
            order_side: Some("Buy".into()),
            filled_quantity: None,
            residual_quantity: Some("10.00".into()),
            ts_event_ns: Some(1_300_000_000),
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
    fn evidence() -> BoltV3TerminalSettlementEvidence {
        BoltV3TerminalSettlementEvidence {
            settlement_key: "settlement-1".into(),
            booking_error: Some(booking_error()),
            lifecycle: lifecycle(),
        }
    }

    #[test]
    fn current_terminal_settlement_codec_is_byte_exact_and_decodable() {
        let encoded = encode_terminal_settlement_at(&evidence(), 10)
            .expect("terminal settlement should encode");
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/terminal_settlement_v1.jsonl"
        ));
        assert_eq!(encoded.bytes(), fixture);
        let DecodedFact::TerminalSettlement(decoded) =
            decode_registered_line(fixture).expect("fixture should decode")
        else {
            panic!("terminal settlement decoded to wrong fact");
        };
        assert_eq!(decoded.settlement_key, "settlement-1");
        assert_eq!(
            decoded.lifecycle.transition,
            OrderLifecycleTransition::SettlementBookingTerminal
        );
        assert_eq!(decoded.lifecycle.outcome, OrderLifecycleOutcome::Flat);
    }

    #[test]
    fn current_terminal_settlement_without_booking_error_is_byte_exact() {
        let mut evidence = evidence();
        evidence.booking_error = None;
        let encoded = encode_terminal_settlement_at(&evidence, 11)
            .expect("terminal settlement without booking error should encode");
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/terminal_settlement_without_booking_error_v1.jsonl"
        ));
        assert_eq!(encoded.bytes(), fixture);
        let DecodedFact::TerminalSettlement(decoded) =
            decode_registered_line(fixture).expect("fixture should decode")
        else {
            panic!("terminal settlement decoded to wrong fact");
        };
        assert_eq!(decoded.booking_error, None);
    }

    #[test]
    fn current_terminal_settlement_rejects_noncanonical_lifecycle_and_mismatched_keys() {
        let mut invalid = evidence();
        invalid.lifecycle.outcome = BoltV3OrderLifecycleOutcome::Managed;
        assert!(encode_terminal_settlement(&invalid).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/terminal_settlement_v1.jsonl"
        ));
        let mut value: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        value["terminal_settlement"]["booking_error"]["settlement_key"] =
            serde_json::json!("other");
        assert!(decode_registered_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
