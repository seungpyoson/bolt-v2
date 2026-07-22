use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3OrderLifecycleEvidence, BoltV3OrderLifecycleOutcome, BoltV3OrderLifecycleTransition,
    facts::{
        OrderLifecycleFact, OrderLifecycleOutcome, OrderLifecycleSide, OrderLifecycleTransition,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderLifecycleV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    order_lifecycle: OrderLifecycleV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderLifecycleV1Wire {
    strategy_id: String,
    transition: OrderLifecycleTransitionV1,
    outcome: OrderLifecycleOutcomeV1,
    source: String,
    market_id: Option<String>,
    instrument_id: Option<String>,
    position_id: Option<String>,
    client_order_id: Option<String>,
    prior_client_order_id: Option<String>,
    raw_reason_text: Option<String>,
    order_side: Option<OrderLifecycleSideV1>,
    filled_quantity: Option<String>,
    residual_quantity: Option<String>,
    ts_event_ns: Option<u64>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrderLifecycleTransitionV1 {
    BoundaryReclassification,
    EntryFillMaterialized,
    EntryReconcilePending,
    PositionTruthRematerialized,
    PositionClosed,
    ResidualRemanaged,
    RestartOpenOrderAdopted,
    RestartOpenOrderRecoveryBlocked,
    SettlementEvidenceRecoveryBlocked,
    SettlementBookingTerminal,
    OrderDenied,
    OrderRejected,
    OrderCanceled,
    OrderExpired,
    OrderFilled,
    ReconcileQueryFailed,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrderLifecycleOutcomeV1 {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrderLifecycleSideV1 {
    Buy,
    Sell,
}

pub fn encode_order_lifecycle(
    evidence: &BoltV3OrderLifecycleEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_order_lifecycle_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_order_lifecycle_at(
    evidence: &BoltV3OrderLifecycleEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::OrderLifecycle;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "order_lifecycle",
        "order-lifecycle identity has wrong payload member"
    );
    let line = OrderLifecycleV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        order_lifecycle: OrderLifecycleV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "order lifecycle")
}

impl TryFrom<&BoltV3OrderLifecycleEvidence> for OrderLifecycleV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3OrderLifecycleEvidence) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            transition: value.transition.into(),
            outcome: value.outcome.into(),
            source: required_text(&value.source, "source")?,
            market_id: optional_text(value.market_id.as_deref(), "market_id")?,
            instrument_id: optional_text(value.instrument_id.as_deref(), "instrument_id")?,
            position_id: optional_text(value.position_id.as_deref(), "position_id")?,
            client_order_id: optional_text(value.client_order_id.as_deref(), "client_order_id")?,
            prior_client_order_id: optional_text(
                value.prior_client_order_id.as_deref(),
                "prior_client_order_id",
            )?,
            raw_reason_text: optional_text(value.raw_reason_text.as_deref(), "raw_reason_text")?,
            order_side: value
                .order_side
                .as_deref()
                .map(OrderLifecycleSideV1::try_from)
                .transpose()?,
            filled_quantity: optional_positive_decimal(
                value.filled_quantity.as_deref(),
                "filled_quantity",
            )?,
            residual_quantity: optional_positive_decimal(
                value.residual_quantity.as_deref(),
                "residual_quantity",
            )?,
            ts_event_ns: optional_positive_timestamp(value.ts_event_ns, "ts_event_ns")?,
        })
    }
}

impl From<BoltV3OrderLifecycleTransition> for OrderLifecycleTransitionV1 {
    fn from(value: BoltV3OrderLifecycleTransition) -> Self {
        match value {
            BoltV3OrderLifecycleTransition::BoundaryReclassification => {
                Self::BoundaryReclassification
            }
            BoltV3OrderLifecycleTransition::EntryFillMaterialized => Self::EntryFillMaterialized,
            BoltV3OrderLifecycleTransition::EntryReconcilePending => Self::EntryReconcilePending,
            BoltV3OrderLifecycleTransition::PositionTruthRematerialized => {
                Self::PositionTruthRematerialized
            }
            BoltV3OrderLifecycleTransition::PositionClosed => Self::PositionClosed,
            BoltV3OrderLifecycleTransition::ResidualRemanaged => Self::ResidualRemanaged,
            BoltV3OrderLifecycleTransition::RestartOpenOrderAdopted => {
                Self::RestartOpenOrderAdopted
            }
            BoltV3OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked => {
                Self::RestartOpenOrderRecoveryBlocked
            }
            BoltV3OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked => {
                Self::SettlementEvidenceRecoveryBlocked
            }
            BoltV3OrderLifecycleTransition::SettlementBookingTerminal => {
                Self::SettlementBookingTerminal
            }
            BoltV3OrderLifecycleTransition::OrderDenied => Self::OrderDenied,
            BoltV3OrderLifecycleTransition::OrderRejected => Self::OrderRejected,
            BoltV3OrderLifecycleTransition::OrderCanceled => Self::OrderCanceled,
            BoltV3OrderLifecycleTransition::OrderExpired => Self::OrderExpired,
            BoltV3OrderLifecycleTransition::OrderFilled => Self::OrderFilled,
            BoltV3OrderLifecycleTransition::ReconcileQueryFailed => Self::ReconcileQueryFailed,
        }
    }
}

impl From<BoltV3OrderLifecycleOutcome> for OrderLifecycleOutcomeV1 {
    fn from(value: BoltV3OrderLifecycleOutcome) -> Self {
        match value {
            BoltV3OrderLifecycleOutcome::PendingEntry => Self::PendingEntry,
            BoltV3OrderLifecycleOutcome::Managed => Self::Managed,
            BoltV3OrderLifecycleOutcome::ExitPending => Self::ExitPending,
            BoltV3OrderLifecycleOutcome::EntryReconcilePending => Self::EntryReconcilePending,
            BoltV3OrderLifecycleOutcome::UnsupportedObserved => Self::UnsupportedObserved,
            BoltV3OrderLifecycleOutcome::BlindRecovery => Self::BlindRecovery,
            BoltV3OrderLifecycleOutcome::Flat => Self::Flat,
        }
    }
}

impl TryFrom<&str> for OrderLifecycleSideV1 {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self> {
        match value {
            "Buy" => Ok(Self::Buy),
            "Sell" => Ok(Self::Sell),
            _ => anyhow::bail!("order_side must be Buy or Sell"),
        }
    }
}

pub(crate) fn decode_order_lifecycle(line: &[u8]) -> Result<OrderLifecycleFact> {
    let line: OrderLifecycleV1Line =
        serde_json::from_slice(line).context("failed to decode current order lifecycle")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::OrderLifecycle,
        "order_lifecycle",
    )?;
    let value = line.order_lifecycle;
    Ok(OrderLifecycleFact {
        strategy_id: required_text(&value.strategy_id, "strategy_id")?,
        transition: value.transition.into(),
        outcome: value.outcome.into(),
        source: required_text(&value.source, "source")?,
        market_id: optional_text(value.market_id.as_deref(), "market_id")?,
        instrument_id: optional_text(value.instrument_id.as_deref(), "instrument_id")?,
        position_id: optional_text(value.position_id.as_deref(), "position_id")?,
        client_order_id: optional_text(value.client_order_id.as_deref(), "client_order_id")?,
        prior_client_order_id: optional_text(
            value.prior_client_order_id.as_deref(),
            "prior_client_order_id",
        )?,
        raw_reason_text: optional_text(value.raw_reason_text.as_deref(), "raw_reason_text")?,
        order_side: value.order_side.map(Into::into),
        filled_quantity: optional_positive_decimal_value(
            value.filled_quantity.as_deref(),
            "filled_quantity",
        )?,
        residual_quantity: optional_positive_decimal_value(
            value.residual_quantity.as_deref(),
            "residual_quantity",
        )?,
        ts_event_ns: optional_positive_timestamp(value.ts_event_ns, "ts_event_ns")?,
    })
}

impl From<OrderLifecycleTransitionV1> for OrderLifecycleTransition {
    fn from(value: OrderLifecycleTransitionV1) -> Self {
        match value {
            OrderLifecycleTransitionV1::BoundaryReclassification => Self::BoundaryReclassification,
            OrderLifecycleTransitionV1::EntryFillMaterialized => Self::EntryFillMaterialized,
            OrderLifecycleTransitionV1::EntryReconcilePending => Self::EntryReconcilePending,
            OrderLifecycleTransitionV1::PositionTruthRematerialized => {
                Self::PositionTruthRematerialized
            }
            OrderLifecycleTransitionV1::PositionClosed => Self::PositionClosed,
            OrderLifecycleTransitionV1::ResidualRemanaged => Self::ResidualRemanaged,
            OrderLifecycleTransitionV1::RestartOpenOrderAdopted => Self::RestartOpenOrderAdopted,
            OrderLifecycleTransitionV1::RestartOpenOrderRecoveryBlocked => {
                Self::RestartOpenOrderRecoveryBlocked
            }
            OrderLifecycleTransitionV1::SettlementEvidenceRecoveryBlocked => {
                Self::SettlementEvidenceRecoveryBlocked
            }
            OrderLifecycleTransitionV1::SettlementBookingTerminal => {
                Self::SettlementBookingTerminal
            }
            OrderLifecycleTransitionV1::OrderDenied => Self::OrderDenied,
            OrderLifecycleTransitionV1::OrderRejected => Self::OrderRejected,
            OrderLifecycleTransitionV1::OrderCanceled => Self::OrderCanceled,
            OrderLifecycleTransitionV1::OrderExpired => Self::OrderExpired,
            OrderLifecycleTransitionV1::OrderFilled => Self::OrderFilled,
            OrderLifecycleTransitionV1::ReconcileQueryFailed => Self::ReconcileQueryFailed,
        }
    }
}

impl From<OrderLifecycleOutcomeV1> for OrderLifecycleOutcome {
    fn from(value: OrderLifecycleOutcomeV1) -> Self {
        match value {
            OrderLifecycleOutcomeV1::PendingEntry => Self::PendingEntry,
            OrderLifecycleOutcomeV1::Managed => Self::Managed,
            OrderLifecycleOutcomeV1::ExitPending => Self::ExitPending,
            OrderLifecycleOutcomeV1::EntryReconcilePending => Self::EntryReconcilePending,
            OrderLifecycleOutcomeV1::UnsupportedObserved => Self::UnsupportedObserved,
            OrderLifecycleOutcomeV1::BlindRecovery => Self::BlindRecovery,
            OrderLifecycleOutcomeV1::Flat => Self::Flat,
        }
    }
}

impl From<OrderLifecycleSideV1> for OrderLifecycleSide {
    fn from(value: OrderLifecycleSideV1) -> Self {
        match value {
            OrderLifecycleSideV1::Buy => Self::Buy,
            OrderLifecycleSideV1::Sell => Self::Sell,
        }
    }
}

fn optional_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_text(value, field)).transpose()
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

fn optional_positive_timestamp(value: Option<u64>, field: &str) -> Result<Option<u64>> {
    value
        .map(|value| {
            ensure!(value > 0, "`{field}` must be positive");
            Ok(value)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};

    fn evidence() -> BoltV3OrderLifecycleEvidence {
        BoltV3OrderLifecycleEvidence {
            strategy_id: "BINARYORACLEEDGETAKER-001".to_string(),
            transition: BoltV3OrderLifecycleTransition::SettlementBookingTerminal,
            outcome: BoltV3OrderLifecycleOutcome::Flat,
            source: "settlement_booking_terminal".to_string(),
            market_id: Some("MKT-1".to_string()),
            instrument_id: Some("condition-MKT-1-UP.POLYMARKET".to_string()),
            position_id: Some("P-1".to_string()),
            client_order_id: None,
            prior_client_order_id: None,
            raw_reason_text: Some("settlement booking terminal".to_string()),
            order_side: Some("Buy".to_string()),
            filled_quantity: None,
            residual_quantity: Some("10.00".to_string()),
            ts_event_ns: Some(1_300_000_000),
        }
    }

    #[test]
    fn current_order_lifecycle_codec_is_byte_exact_and_decodable() {
        let encoded =
            encode_order_lifecycle_at(&evidence(), 6).expect("valid order lifecycle should encode");
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/order_lifecycle_v1.jsonl"
        ));
        assert_eq!(encoded.bytes(), fixture);
        let decoded = decode_registered_line(fixture).expect("fixture should decode");
        let DecodedFact::OrderLifecycle(decoded) = decoded else {
            panic!("order-lifecycle fixture decoded to wrong fact");
        };
        assert_eq!(
            decoded.transition,
            OrderLifecycleTransition::SettlementBookingTerminal
        );
        assert_eq!(decoded.outcome, OrderLifecycleOutcome::Flat);
        assert_eq!(decoded.order_side, Some(OrderLifecycleSide::Buy));
        assert_eq!(decoded.residual_quantity, Some(Decimal::TEN));
    }

    #[test]
    fn current_order_lifecycle_codec_rejects_domain_widening() {
        let mut invalid = evidence();
        invalid.order_side = Some("Other".to_string());
        assert!(encode_order_lifecycle(&invalid).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/order_lifecycle_v1.jsonl"
        ));
        let mut value: serde_json::Value =
            serde_json::from_slice(fixture).expect("fixture should parse");
        value["order_lifecycle"]["extra"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&value).expect("mutated fixture should serialize");
        assert!(decode_registered_line(&bytes).is_err());
    }
}
