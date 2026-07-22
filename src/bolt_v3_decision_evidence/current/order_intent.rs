use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3OrderIntentClampNotEvaluatedReason, BoltV3OrderIntentClampOutcome,
    BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields,
    facts::{
        OrderIntentClampNotEvaluatedReasonFact, OrderIntentClampOutcomeFact, OrderIntentFact,
        OrderIntentOrderFieldsFact,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderIntentV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    intent: OrderIntentV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderIntentV1Wire {
    strategy_id: String,
    instrument_id: String,
    client_order_id: String,
    order_side: String,
    price: String,
    quantity: String,
    clamp_outcome: Option<OrderIntentClampOutcomeV1>,
    order_fields: OrderIntentOrderFieldsV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum OrderIntentClampOutcomeV1 {
    WithinBounds,
    Clamped {
        original_quantity: String,
    },
    Rejected,
    NotEvaluated {
        reason: OrderIntentClampNotEvaluatedReasonV1,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrderIntentClampNotEvaluatedReasonV1 {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderIntentOrderFieldsV1 {
    order_type: String,
    time_in_force: String,
    price: Option<String>,
    trigger_price: Option<String>,
    activation_price: Option<String>,
    trigger_type: Option<String>,
    trigger_instrument_id: Option<String>,
    trailing_offset: Option<String>,
    trailing_offset_type: Option<String>,
    expire_time_unix_nanos: Option<String>,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
}

pub fn encode_entry_order_intent(
    evidence: &BoltV3OrderIntentEvidence,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.intent_kind == BoltV3OrderIntentKind::Entry,
        "entry-order-intent encoder requires an entry intent"
    );
    encode_order_intent_at(
        evidence,
        KnownPurpose::EntryOrderIntent,
        positive_recorded_at_utc_ns()?,
    )
}

pub fn encode_risk_reducing_exit_order_intent(
    evidence: &BoltV3OrderIntentEvidence,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.intent_kind == BoltV3OrderIntentKind::Exit,
        "risk-reducing-exit encoder requires an exit intent"
    );
    encode_order_intent_at(
        evidence,
        KnownPurpose::RiskReducingExitOrderIntent,
        positive_recorded_at_utc_ns()?,
    )
}

fn encode_order_intent_at(
    evidence: &BoltV3OrderIntentEvidence,
    purpose: KnownPurpose,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "intent",
        "order-intent identity has wrong payload member"
    );
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let line = OrderIntentV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        intent: OrderIntentV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "order intent")
}

pub(crate) fn decode_entry_order_intent(line: &[u8]) -> Result<OrderIntentFact> {
    decode_order_intent(line, KnownPurpose::EntryOrderIntent)
}

pub(crate) fn decode_risk_reducing_exit_order_intent(line: &[u8]) -> Result<OrderIntentFact> {
    decode_order_intent(line, KnownPurpose::RiskReducingExitOrderIntent)
}

fn decode_order_intent(line: &[u8], purpose: KnownPurpose) -> Result<OrderIntentFact> {
    let decoded: OrderIntentV1Line =
        serde_json::from_slice(line).context("failed to decode current order intent")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        purpose,
        "intent",
    )?;
    decoded.intent.try_into()
}

impl TryFrom<&BoltV3OrderIntentEvidence> for OrderIntentV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3OrderIntentEvidence) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            order_side: required_text(&value.order_side, "order_side")?,
            price: positive_decimal_text(&value.price, "price")?,
            quantity: positive_decimal_text(&value.quantity, "quantity")?,
            clamp_outcome: value
                .clamp_outcome
                .as_ref()
                .map(TryInto::try_into)
                .transpose()?,
            order_fields: (&value.order_fields).try_into()?,
        })
    }
}

impl TryFrom<&BoltV3OrderIntentClampOutcome> for OrderIntentClampOutcomeV1 {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3OrderIntentClampOutcome) -> Result<Self> {
        Ok(match value {
            BoltV3OrderIntentClampOutcome::WithinBounds => Self::WithinBounds,
            BoltV3OrderIntentClampOutcome::Clamped { original_quantity } => Self::Clamped {
                original_quantity: positive_decimal_text(original_quantity, "original_quantity")?,
            },
            BoltV3OrderIntentClampOutcome::Rejected => Self::Rejected,
            BoltV3OrderIntentClampOutcome::NotEvaluated { reason } => Self::NotEvaluated {
                reason: (*reason).into(),
            },
        })
    }
}

impl From<BoltV3OrderIntentClampNotEvaluatedReason> for OrderIntentClampNotEvaluatedReasonV1 {
    fn from(value: BoltV3OrderIntentClampNotEvaluatedReason) -> Self {
        match value {
            BoltV3OrderIntentClampNotEvaluatedReason::NoVenueTruth => Self::NoVenueTruth,
            BoltV3OrderIntentClampNotEvaluatedReason::ForeignInstrument => Self::ForeignInstrument,
            BoltV3OrderIntentClampNotEvaluatedReason::NonSellOrderSide => Self::NonSellOrderSide,
        }
    }
}

impl TryFrom<&BoltV3OrderIntentOrderFields> for OrderIntentOrderFieldsV1 {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3OrderIntentOrderFields) -> Result<Self> {
        Ok(Self {
            order_type: required_text(&value.order_type, "order_type")?,
            time_in_force: required_text(&value.time_in_force, "time_in_force")?,
            price: optional_positive_decimal_text(value.price.as_deref(), "order_fields.price")?,
            trigger_price: optional_positive_decimal_text(
                value.trigger_price.as_deref(),
                "order_fields.trigger_price",
            )?,
            activation_price: optional_positive_decimal_text(
                value.activation_price.as_deref(),
                "order_fields.activation_price",
            )?,
            trigger_type: optional_text(value.trigger_type.as_deref(), "trigger_type")?,
            trigger_instrument_id: optional_text(
                value.trigger_instrument_id.as_deref(),
                "trigger_instrument_id",
            )?,
            trailing_offset: optional_positive_decimal_text(
                value.trailing_offset.as_deref(),
                "trailing_offset",
            )?,
            trailing_offset_type: optional_text(
                value.trailing_offset_type.as_deref(),
                "trailing_offset_type",
            )?,
            expire_time_unix_nanos: optional_positive_u64_text(
                value.expire_time_unix_nanos.as_deref(),
                "expire_time_unix_nanos",
            )?,
            is_post_only: value.is_post_only,
            is_reduce_only: value.is_reduce_only,
            is_quote_quantity: value.is_quote_quantity,
        })
    }
}

impl TryFrom<OrderIntentV1Wire> for OrderIntentFact {
    type Error = anyhow::Error;

    fn try_from(value: OrderIntentV1Wire) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            order_side: required_text(&value.order_side, "order_side")?,
            price: positive_decimal(&value.price, "price")?,
            quantity: positive_decimal(&value.quantity, "quantity")?,
            clamp_outcome: value.clamp_outcome.map(TryInto::try_into).transpose()?,
            order_fields: value.order_fields.try_into()?,
        })
    }
}

impl TryFrom<OrderIntentClampOutcomeV1> for OrderIntentClampOutcomeFact {
    type Error = anyhow::Error;

    fn try_from(value: OrderIntentClampOutcomeV1) -> Result<Self> {
        Ok(match value {
            OrderIntentClampOutcomeV1::WithinBounds => Self::WithinBounds,
            OrderIntentClampOutcomeV1::Clamped { original_quantity } => Self::Clamped {
                original_quantity: positive_decimal(&original_quantity, "original_quantity")?,
            },
            OrderIntentClampOutcomeV1::Rejected => Self::Rejected,
            OrderIntentClampOutcomeV1::NotEvaluated { reason } => Self::NotEvaluated {
                reason: reason.into(),
            },
        })
    }
}

impl From<OrderIntentClampNotEvaluatedReasonV1> for OrderIntentClampNotEvaluatedReasonFact {
    fn from(value: OrderIntentClampNotEvaluatedReasonV1) -> Self {
        match value {
            OrderIntentClampNotEvaluatedReasonV1::NoVenueTruth => Self::NoVenueTruth,
            OrderIntentClampNotEvaluatedReasonV1::ForeignInstrument => Self::ForeignInstrument,
            OrderIntentClampNotEvaluatedReasonV1::NonSellOrderSide => Self::NonSellOrderSide,
        }
    }
}

impl TryFrom<OrderIntentOrderFieldsV1> for OrderIntentOrderFieldsFact {
    type Error = anyhow::Error;

    fn try_from(value: OrderIntentOrderFieldsV1) -> Result<Self> {
        Ok(Self {
            order_type: required_text(&value.order_type, "order_type")?,
            time_in_force: required_text(&value.time_in_force, "time_in_force")?,
            price: optional_positive_decimal(value.price.as_deref(), "order_fields.price")?,
            trigger_price: optional_positive_decimal(
                value.trigger_price.as_deref(),
                "order_fields.trigger_price",
            )?,
            activation_price: optional_positive_decimal(
                value.activation_price.as_deref(),
                "order_fields.activation_price",
            )?,
            trigger_type: optional_text(value.trigger_type.as_deref(), "trigger_type")?,
            trigger_instrument_id: optional_text(
                value.trigger_instrument_id.as_deref(),
                "trigger_instrument_id",
            )?,
            trailing_offset: optional_positive_decimal(
                value.trailing_offset.as_deref(),
                "trailing_offset",
            )?,
            trailing_offset_type: optional_text(
                value.trailing_offset_type.as_deref(),
                "trailing_offset_type",
            )?,
            expire_time_unix_nanos: value
                .expire_time_unix_nanos
                .as_deref()
                .map(|raw| parse_positive_u64(raw, "expire_time_unix_nanos"))
                .transpose()?,
            is_post_only: value.is_post_only,
            is_reduce_only: value.is_reduce_only,
            is_quote_quantity: value.is_quote_quantity,
        })
    }
}

fn positive_decimal_text(value: &str, field: &str) -> Result<String> {
    Ok(positive_decimal(value, field)?.normalize().to_string())
}

fn positive_decimal(value: &str, field: &str) -> Result<Decimal> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "{field} must be canonical"
    );
    let parsed = value
        .parse::<Decimal>()
        .with_context(|| format!("{field} must parse as decimal"))?;
    ensure!(parsed > Decimal::ZERO, "{field} must be positive");
    Ok(parsed)
}

fn optional_positive_decimal_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value
        .map(|raw| positive_decimal_text(raw, field))
        .transpose()
}

fn optional_positive_decimal(value: Option<&str>, field: &str) -> Result<Option<Decimal>> {
    value.map(|raw| positive_decimal(raw, field)).transpose()
}

fn optional_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value.map(|raw| required_text(raw, field)).transpose()
}

fn optional_positive_u64_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value
        .map(|raw| parse_positive_u64(raw, field).map(|parsed| parsed.to_string()))
        .transpose()
}

fn parse_positive_u64(value: &str, field: &str) -> Result<u64> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "{field} must be canonical"
    );
    let parsed = value
        .parse::<u64>()
        .with_context(|| format!("{field} must parse as u64"))?;
    ensure!(parsed > 0, "{field} must be positive");
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(intent_kind: BoltV3OrderIntentKind) -> BoltV3OrderIntentEvidence {
        BoltV3OrderIntentEvidence {
            strategy_id: "strategy".to_string(),
            intent_kind,
            instrument_id: "instrument".to_string(),
            client_order_id: "order".to_string(),
            order_side: "BUY".to_string(),
            price: "0.55".to_string(),
            quantity: "10".to_string(),
            clamp_outcome: Some(BoltV3OrderIntentClampOutcome::WithinBounds),
            order_fields: BoltV3OrderIntentOrderFields {
                order_type: "LIMIT".to_string(),
                time_in_force: "GTC".to_string(),
                price: Some("0.55".to_string()),
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                expire_time_unix_nanos: None,
                is_post_only: false,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
        }
    }

    #[test]
    fn entry_and_exit_intents_use_disjoint_current_identities() {
        let entry = encode_order_intent_at(
            &evidence(BoltV3OrderIntentKind::Entry),
            KnownPurpose::EntryOrderIntent,
            1,
        )
        .unwrap();
        let exit = encode_order_intent_at(
            &evidence(BoltV3OrderIntentKind::Exit),
            KnownPurpose::RiskReducingExitOrderIntent,
            1,
        )
        .unwrap();
        assert_eq!(
            entry.bytes(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/entry_order_intent_v1.jsonl"
            ))
        );
        assert_eq!(
            exit.bytes(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/risk_reducing_exit_order_intent_v1.jsonl"
            ))
        );
        assert_ne!(entry.bytes(), exit.bytes());
        assert!(matches!(
            decode_entry_order_intent(entry.bytes()).unwrap(),
            OrderIntentFact { .. }
        ));
        assert!(decode_entry_order_intent(exit.bytes()).is_err());
        assert!(decode_risk_reducing_exit_order_intent(entry.bytes()).is_err());
        assert!(decode_risk_reducing_exit_order_intent(exit.bytes()).is_ok());
    }
}
