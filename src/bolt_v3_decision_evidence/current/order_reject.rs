use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionOutcome, BoltV3OrderRejectEvidence, BoltV3OrderRejectReason, BoltV3RejectSource,
    facts::{AdmissionOutcomeFact, OrderRejectFact, OrderRejectReasonFact, RejectSourceFact},
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderRejectV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    order_reject: OrderRejectV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderRejectV1Wire {
    reject_source: RejectSourceV1,
    reject_reason: OrderRejectReasonV1,
    admission_outcome: Option<AdmissionOutcomeV1>,
    raw_reason_text: Option<String>,
    instrument_id: String,
    order_side: Option<String>,
    raw_price: Option<String>,
    raw_quantity: Option<String>,
    raw_maker_amount: Option<String>,
    raw_taker_amount: Option<String>,
    normalized_price: Option<String>,
    normalized_quantity: Option<String>,
    normalized_maker_amount: Option<String>,
    normalized_taker_amount: Option<String>,
    venue_price_precision: Option<u32>,
    venue_size_precision: Option<u32>,
    venue_min_notional: Option<String>,
    prior_client_order_id: Option<String>,
    client_order_id: String,
    retry_count: u32,
    backoff_cooldown_state: Option<String>,
    stable_episode_key: String,
    elapsed_ns: u64,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RejectSourceV1 {
    SubmitAdmission,
    Venue,
    NtExecution,
    Internal,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrderRejectReasonV1 {
    AdmissionRejected,
    PrecisionRejected,
    MinSizeRejected,
    MinNotionalRejected,
    InsufficientBalance,
    DuplicateClientOrderId,
    Other,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionOutcomeV1 {
    Admitted,
    RejectedKillSwitchLatched,
    RejectedSubmitLifecycleDisallowed,
    RejectedLossGovernorHalted,
    RejectedNonPositiveNotional,
    RejectedNotionalCapExceeded,
    RejectedInvalidRiskReducingExitProof,
    RejectedCountCapExhausted,
    RejectedKillSwitchForcedReductionProofInvalid,
    RejectedKillSwitchForcedReductionCapExceeded,
    RejectedCapitalAdmission,
}

pub fn encode_order_reject(evidence: &BoltV3OrderRejectEvidence) -> Result<EncodedEvidenceRecord> {
    encode_order_reject_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_order_reject_at(
    evidence: &BoltV3OrderRejectEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::OrderReject;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "order_reject",
        "order-reject payload mismatch"
    );
    let line = OrderRejectV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        order_reject: OrderRejectV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "order reject")
}

pub(crate) fn decode_order_reject(line: &[u8]) -> Result<OrderRejectFact> {
    let decoded: OrderRejectV1Line =
        serde_json::from_slice(line).context("failed to decode order-reject v1")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        KnownPurpose::OrderReject,
        "order_reject",
    )?;
    decoded.order_reject.try_into()
}

impl TryFrom<&BoltV3OrderRejectEvidence> for OrderRejectV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3OrderRejectEvidence) -> Result<Self> {
        let wire = Self {
            reject_source: value.reject_source.into(),
            reject_reason: value.reject_reason.into(),
            admission_outcome: value.admission_outcome.clone().map(Into::into),
            raw_reason_text: optional_text(value.raw_reason_text.clone(), "raw_reason_text")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            order_side: optional_text(value.order_side.clone(), "order_side")?,
            raw_price: optional_decimal_text(value.raw_price.as_deref(), "raw_price")?,
            raw_quantity: optional_decimal_text(value.raw_quantity.as_deref(), "raw_quantity")?,
            raw_maker_amount: optional_decimal_text(
                value.raw_maker_amount.as_deref(),
                "raw_maker_amount",
            )?,
            raw_taker_amount: optional_decimal_text(
                value.raw_taker_amount.as_deref(),
                "raw_taker_amount",
            )?,
            normalized_price: optional_decimal_text(
                value.normalized_price.as_deref(),
                "normalized_price",
            )?,
            normalized_quantity: optional_decimal_text(
                value.normalized_quantity.as_deref(),
                "normalized_quantity",
            )?,
            normalized_maker_amount: optional_decimal_text(
                value.normalized_maker_amount.as_deref(),
                "normalized_maker_amount",
            )?,
            normalized_taker_amount: optional_decimal_text(
                value.normalized_taker_amount.as_deref(),
                "normalized_taker_amount",
            )?,
            venue_price_precision: value.venue_price_precision,
            venue_size_precision: value.venue_size_precision,
            venue_min_notional: optional_decimal_text(
                value.venue_min_notional.as_deref(),
                "venue_min_notional",
            )?,
            prior_client_order_id: optional_text(
                value.prior_client_order_id.clone(),
                "prior_client_order_id",
            )?,
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            retry_count: value.retry_count,
            backoff_cooldown_state: optional_text(
                value.backoff_cooldown_state.clone(),
                "backoff_cooldown_state",
            )?,
            stable_episode_key: required_text(&value.stable_episode_key, "stable_episode_key")?,
            elapsed_ns: value.elapsed_ns,
        };
        wire.validate()?;
        Ok(wire)
    }
}

impl TryFrom<OrderRejectV1Wire> for OrderRejectFact {
    type Error = anyhow::Error;

    fn try_from(value: OrderRejectV1Wire) -> Result<Self> {
        value.validate()?;
        Ok(Self {
            reject_source: value.reject_source.into(),
            reject_reason: value.reject_reason.into(),
            admission_outcome: value.admission_outcome.map(Into::into),
            raw_reason_text: optional_text(value.raw_reason_text, "raw_reason_text")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            order_side: optional_text(value.order_side, "order_side")?,
            raw_price: optional_decimal(value.raw_price.as_deref(), "raw_price")?,
            raw_quantity: optional_decimal(value.raw_quantity.as_deref(), "raw_quantity")?,
            raw_maker_amount: optional_decimal(
                value.raw_maker_amount.as_deref(),
                "raw_maker_amount",
            )?,
            raw_taker_amount: optional_decimal(
                value.raw_taker_amount.as_deref(),
                "raw_taker_amount",
            )?,
            normalized_price: optional_decimal(
                value.normalized_price.as_deref(),
                "normalized_price",
            )?,
            normalized_quantity: optional_decimal(
                value.normalized_quantity.as_deref(),
                "normalized_quantity",
            )?,
            normalized_maker_amount: optional_decimal(
                value.normalized_maker_amount.as_deref(),
                "normalized_maker_amount",
            )?,
            normalized_taker_amount: optional_decimal(
                value.normalized_taker_amount.as_deref(),
                "normalized_taker_amount",
            )?,
            venue_price_precision: value.venue_price_precision,
            venue_size_precision: value.venue_size_precision,
            venue_min_notional: optional_decimal(
                value.venue_min_notional.as_deref(),
                "venue_min_notional",
            )?,
            prior_client_order_id: optional_text(
                value.prior_client_order_id,
                "prior_client_order_id",
            )?,
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            retry_count: value.retry_count,
            backoff_cooldown_state: optional_text(
                value.backoff_cooldown_state,
                "backoff_cooldown_state",
            )?,
            stable_episode_key: required_text(&value.stable_episode_key, "stable_episode_key")?,
            elapsed_ns: value.elapsed_ns,
        })
    }
}

impl OrderRejectV1Wire {
    fn validate(&self) -> Result<()> {
        ensure!(
            !matches!(self.admission_outcome, Some(AdmissionOutcomeV1::Admitted)),
            "order rejection cannot carry an admitted admission outcome"
        );
        Ok(())
    }
}

fn optional_text(value: Option<String>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_text(&value, field)).transpose()
}

fn optional_decimal_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    optional_decimal(value, field).map(|value| value.map(|value| value.normalize().to_string()))
}

fn optional_decimal(value: Option<&str>, field: &str) -> Result<Option<Decimal>> {
    value
        .map(|value| positive_decimal(value, field))
        .transpose()
}

fn positive_decimal(value: &str, field: &str) -> Result<Decimal> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "{field} must be canonical"
    );
    let value = value
        .parse::<Decimal>()
        .with_context(|| format!("{field} must parse as decimal"))?;
    ensure!(value > Decimal::ZERO, "{field} must be positive");
    Ok(value)
}

macro_rules! map_enum {
    ($from:ty => $to:ty, $($variant:ident),+ $(,)?) => {
        impl From<$from> for $to {
            fn from(value: $from) -> Self {
                match value { $(<$from>::$variant => Self::$variant,)+ }
            }
        }
    };
}

map_enum!(BoltV3RejectSource => RejectSourceV1,
    SubmitAdmission, Venue, NtExecution, Internal);
map_enum!(RejectSourceV1 => RejectSourceFact,
    SubmitAdmission, Venue, NtExecution, Internal);
map_enum!(BoltV3OrderRejectReason => OrderRejectReasonV1,
    AdmissionRejected, PrecisionRejected, MinSizeRejected, MinNotionalRejected,
    InsufficientBalance, DuplicateClientOrderId, Other);
map_enum!(OrderRejectReasonV1 => OrderRejectReasonFact,
    AdmissionRejected, PrecisionRejected, MinSizeRejected, MinNotionalRejected,
    InsufficientBalance, DuplicateClientOrderId, Other);
map_enum!(BoltV3AdmissionOutcome => AdmissionOutcomeV1,
    Admitted, RejectedKillSwitchLatched, RejectedSubmitLifecycleDisallowed,
    RejectedLossGovernorHalted, RejectedNonPositiveNotional, RejectedNotionalCapExceeded,
    RejectedInvalidRiskReducingExitProof, RejectedCountCapExhausted,
    RejectedKillSwitchForcedReductionProofInvalid,
    RejectedKillSwitchForcedReductionCapExceeded, RejectedCapitalAdmission);
map_enum!(AdmissionOutcomeV1 => AdmissionOutcomeFact,
    Admitted, RejectedKillSwitchLatched, RejectedSubmitLifecycleDisallowed,
    RejectedLossGovernorHalted, RejectedNonPositiveNotional, RejectedNotionalCapExceeded,
    RejectedInvalidRiskReducingExitProof, RejectedCountCapExhausted,
    RejectedKillSwitchForcedReductionProofInvalid,
    RejectedKillSwitchForcedReductionCapExceeded, RejectedCapitalAdmission);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_order_reject_round_trips_under_its_exact_identity() {
        let evidence = BoltV3OrderRejectEvidence {
            reject_source: BoltV3RejectSource::Venue,
            reject_reason: BoltV3OrderRejectReason::MinNotionalRejected,
            admission_outcome: None,
            raw_reason_text: Some("below minimum".into()),
            instrument_id: "instrument".into(),
            order_side: Some("BUY".into()),
            raw_price: Some("0.50".into()),
            raw_quantity: Some("2".into()),
            raw_maker_amount: None,
            raw_taker_amount: None,
            normalized_price: Some("0.5".into()),
            normalized_quantity: Some("2".into()),
            normalized_maker_amount: None,
            normalized_taker_amount: None,
            venue_price_precision: Some(2),
            venue_size_precision: Some(2),
            venue_min_notional: Some("5".into()),
            prior_client_order_id: None,
            client_order_id: "order".into(),
            retry_count: 1,
            backoff_cooldown_state: Some("active".into()),
            stable_episode_key: "episode".into(),
            elapsed_ns: 10,
        };
        let encoded = encode_order_reject_at(&evidence, 1).unwrap();
        assert_eq!(
            encoded.bytes(),
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/order_reject_v1.jsonl"
            ))
        );
        let decoded = decode_order_reject(encoded.bytes()).unwrap();
        assert_eq!(
            decoded.reject_reason,
            OrderRejectReasonFact::MinNotionalRejected
        );
        assert_eq!(decoded.normalized_price, Some(Decimal::new(5, 1)));
    }
}
