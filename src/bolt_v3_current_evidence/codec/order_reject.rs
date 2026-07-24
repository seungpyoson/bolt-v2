use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{decode, encode_line, validate_envelope, validate_recorded_at};
use crate::bolt_v3_current_evidence::{
    facts::{
        AdmissionDecisionOutcome, AdmissionRejectionReason, EvidenceOrderSide, OrderRejectFact,
        OrderRejectReason, OrderRejectSource,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

pub(super) fn encode(
    fact: OrderRejectFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fact(&fact)?;
    let purpose = KnownPurpose::OrderReject;
    let descriptor = super::current_line_descriptor(purpose);
    encode_line(
        purpose,
        &OrderRejectLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            order_reject: OrderRejectWireV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_fact(line: &str, line_number: usize) -> Result<OrderRejectFact> {
    let decoded: OrderRejectLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::OrderRejectV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.order_reject.into_fact();
    validate_fact(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_fact(fact: &OrderRejectFact) -> Result<(), RecordFailure> {
    let optional_strings = [
        fact.raw_reason_text.as_deref(),
        fact.raw_price.as_deref(),
        fact.raw_quantity.as_deref(),
        fact.raw_maker_amount.as_deref(),
        fact.raw_taker_amount.as_deref(),
        fact.normalized_price.as_deref(),
        fact.normalized_quantity.as_deref(),
        fact.normalized_maker_amount.as_deref(),
        fact.normalized_taker_amount.as_deref(),
        fact.venue_min_notional.as_deref(),
        fact.prior_client_order_id.as_deref(),
    ];
    let admission_shape = matches!(fact.reject_source, OrderRejectSource::SubmitAdmission)
        && matches!(fact.reject_reason, OrderRejectReason::AdmissionRejected)
        && fact.admission_outcome.is_some();
    let observed_shape = !matches!(fact.reject_source, OrderRejectSource::SubmitAdmission)
        && !matches!(fact.reject_reason, OrderRejectReason::AdmissionRejected)
        && fact.admission_outcome.is_none();
    if fact.instrument_id.trim().is_empty()
        || fact.client_order_id.trim().is_empty()
        || fact.stable_episode_key.trim().is_empty()
        || fact.retry_count == 0
        || optional_strings
            .into_iter()
            .flatten()
            .any(|value| value.trim().is_empty())
        || fact.order_side == Some(EvidenceOrderSide::Unspecified)
        || !(admission_shape || observed_shape)
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "order reject contains an empty, invalid, or contradictory field"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderRejectLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    order_reject: OrderRejectWireV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderRejectWireV1 {
    reject_source: RejectSourceV1,
    reject_reason: RejectReasonV1,
    admission_outcome: Option<AdmissionOutcomeV1>,
    raw_reason_text: Option<String>,
    instrument_id: String,
    order_side: Option<OrderSideV1>,
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
    stable_episode_key: String,
    elapsed_ns: u64,
}

impl OrderRejectWireV1 {
    fn from_fact(fact: OrderRejectFact) -> Self {
        Self {
            reject_source: RejectSourceV1::from_fact(fact.reject_source),
            reject_reason: RejectReasonV1::from_fact(fact.reject_reason),
            admission_outcome: fact.admission_outcome.map(AdmissionOutcomeV1::from_fact),
            raw_reason_text: fact.raw_reason_text,
            instrument_id: fact.instrument_id,
            order_side: fact.order_side.map(OrderSideV1::from_fact),
            raw_price: fact.raw_price,
            raw_quantity: fact.raw_quantity,
            raw_maker_amount: fact.raw_maker_amount,
            raw_taker_amount: fact.raw_taker_amount,
            normalized_price: fact.normalized_price,
            normalized_quantity: fact.normalized_quantity,
            normalized_maker_amount: fact.normalized_maker_amount,
            normalized_taker_amount: fact.normalized_taker_amount,
            venue_price_precision: fact.venue_price_precision,
            venue_size_precision: fact.venue_size_precision,
            venue_min_notional: fact.venue_min_notional,
            prior_client_order_id: fact.prior_client_order_id,
            client_order_id: fact.client_order_id,
            retry_count: fact.retry_count,
            stable_episode_key: fact.stable_episode_key,
            elapsed_ns: fact.elapsed_ns,
        }
    }

    fn into_fact(self) -> OrderRejectFact {
        OrderRejectFact {
            reject_source: self.reject_source.into_fact(),
            reject_reason: self.reject_reason.into_fact(),
            admission_outcome: self.admission_outcome.map(AdmissionOutcomeV1::into_fact),
            raw_reason_text: self.raw_reason_text,
            instrument_id: self.instrument_id,
            order_side: self.order_side.map(OrderSideV1::into_fact),
            raw_price: self.raw_price,
            raw_quantity: self.raw_quantity,
            raw_maker_amount: self.raw_maker_amount,
            raw_taker_amount: self.raw_taker_amount,
            normalized_price: self.normalized_price,
            normalized_quantity: self.normalized_quantity,
            normalized_maker_amount: self.normalized_maker_amount,
            normalized_taker_amount: self.normalized_taker_amount,
            venue_price_precision: self.venue_price_precision,
            venue_size_precision: self.venue_size_precision,
            venue_min_notional: self.venue_min_notional,
            prior_client_order_id: self.prior_client_order_id,
            client_order_id: self.client_order_id,
            retry_count: self.retry_count,
            stable_episode_key: self.stable_episode_key,
            elapsed_ns: self.elapsed_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OrderSideV1 {
    Unspecified,
    Buy,
    Sell,
}

impl OrderSideV1 {
    fn from_fact(value: EvidenceOrderSide) -> Self {
        match value {
            EvidenceOrderSide::Unspecified => Self::Unspecified,
            EvidenceOrderSide::Buy => Self::Buy,
            EvidenceOrderSide::Sell => Self::Sell,
        }
    }

    fn into_fact(self) -> EvidenceOrderSide {
        match self {
            Self::Unspecified => EvidenceOrderSide::Unspecified,
            Self::Buy => EvidenceOrderSide::Buy,
            Self::Sell => EvidenceOrderSide::Sell,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RejectSourceV1 {
    SubmitAdmission,
    Venue,
    NtExecution,
    Internal,
}

impl RejectSourceV1 {
    fn from_fact(value: OrderRejectSource) -> Self {
        match value {
            OrderRejectSource::SubmitAdmission => Self::SubmitAdmission,
            OrderRejectSource::Venue => Self::Venue,
            OrderRejectSource::NtExecution => Self::NtExecution,
            OrderRejectSource::Internal => Self::Internal,
        }
    }

    fn into_fact(self) -> OrderRejectSource {
        match self {
            Self::SubmitAdmission => OrderRejectSource::SubmitAdmission,
            Self::Venue => OrderRejectSource::Venue,
            Self::NtExecution => OrderRejectSource::NtExecution,
            Self::Internal => OrderRejectSource::Internal,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RejectReasonV1 {
    AdmissionRejected,
    PrecisionRejected,
    MinSizeRejected,
    MinNotionalRejected,
    InsufficientBalance,
    DuplicateClientOrderId,
    Other,
}

impl RejectReasonV1 {
    fn from_fact(value: OrderRejectReason) -> Self {
        match value {
            OrderRejectReason::AdmissionRejected => Self::AdmissionRejected,
            OrderRejectReason::PrecisionRejected => Self::PrecisionRejected,
            OrderRejectReason::MinSizeRejected => Self::MinSizeRejected,
            OrderRejectReason::MinNotionalRejected => Self::MinNotionalRejected,
            OrderRejectReason::InsufficientBalance => Self::InsufficientBalance,
            OrderRejectReason::DuplicateClientOrderId => Self::DuplicateClientOrderId,
            OrderRejectReason::Other => Self::Other,
        }
    }

    fn into_fact(self) -> OrderRejectReason {
        match self {
            Self::AdmissionRejected => OrderRejectReason::AdmissionRejected,
            Self::PrecisionRejected => OrderRejectReason::PrecisionRejected,
            Self::MinSizeRejected => OrderRejectReason::MinSizeRejected,
            Self::MinNotionalRejected => OrderRejectReason::MinNotionalRejected,
            Self::InsufficientBalance => OrderRejectReason::InsufficientBalance,
            Self::DuplicateClientOrderId => OrderRejectReason::DuplicateClientOrderId,
            Self::Other => OrderRejectReason::Other,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
enum AdmissionOutcomeV1 {
    Admitted,
    Rejected(AdmissionRejectionV1),
}

impl AdmissionOutcomeV1 {
    fn from_fact(value: AdmissionDecisionOutcome) -> Self {
        match value {
            AdmissionDecisionOutcome::Admitted => Self::Admitted,
            AdmissionDecisionOutcome::Rejected(reason) => {
                Self::Rejected(AdmissionRejectionV1::from_fact(reason))
            }
        }
    }

    fn into_fact(self) -> AdmissionDecisionOutcome {
        match self {
            Self::Admitted => AdmissionDecisionOutcome::Admitted,
            Self::Rejected(reason) => AdmissionDecisionOutcome::Rejected(reason.into_fact()),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionRejectionV1 {
    KillSwitchLatched,
    LossGovernorHalted,
    NonPositiveNotional,
    NotionalCapExceeded,
    InvalidRiskReducingExitProof,
    CountCapExhausted,
    KillSwitchForcedReductionProofInvalid,
    KillSwitchForcedReductionCapExceeded,
    CapitalAdmission,
}

impl AdmissionRejectionV1 {
    fn from_fact(value: AdmissionRejectionReason) -> Self {
        match value {
            AdmissionRejectionReason::KillSwitchLatched => Self::KillSwitchLatched,
            AdmissionRejectionReason::LossGovernorHalted => Self::LossGovernorHalted,
            AdmissionRejectionReason::NonPositiveNotional => Self::NonPositiveNotional,
            AdmissionRejectionReason::NotionalCapExceeded => Self::NotionalCapExceeded,
            AdmissionRejectionReason::InvalidRiskReducingExitProof => {
                Self::InvalidRiskReducingExitProof
            }
            AdmissionRejectionReason::CountCapExhausted => Self::CountCapExhausted,
            AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid => {
                Self::KillSwitchForcedReductionProofInvalid
            }
            AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded => {
                Self::KillSwitchForcedReductionCapExceeded
            }
            AdmissionRejectionReason::CapitalAdmission => Self::CapitalAdmission,
        }
    }

    fn into_fact(self) -> AdmissionRejectionReason {
        match self {
            Self::KillSwitchLatched => AdmissionRejectionReason::KillSwitchLatched,
            Self::LossGovernorHalted => AdmissionRejectionReason::LossGovernorHalted,
            Self::NonPositiveNotional => AdmissionRejectionReason::NonPositiveNotional,
            Self::NotionalCapExceeded => AdmissionRejectionReason::NotionalCapExceeded,
            Self::InvalidRiskReducingExitProof => {
                AdmissionRejectionReason::InvalidRiskReducingExitProof
            }
            Self::CountCapExhausted => AdmissionRejectionReason::CountCapExhausted,
            Self::KillSwitchForcedReductionProofInvalid => {
                AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid
            }
            Self::KillSwitchForcedReductionCapExceeded => {
                AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded
            }
            Self::CapitalAdmission => AdmissionRejectionReason::CapitalAdmission,
        }
    }
}
