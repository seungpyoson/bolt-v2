use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        EntryOrderIntentFact, OrderIntentClampNotEvaluatedReason, OrderIntentClampOutcome,
        OrderIntentDetails, OrderIntentOrderFields, RiskReducingExitOrderIntentFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_entry(
    fact: EntryOrderIntentFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_details(&fact.details)?;
    let purpose = KnownPurpose::EntryOrderIntent;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &EntryOrderIntentLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            order_intent: EntryOrderIntentV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_entry(line: &str, line_number: usize) -> Result<EntryOrderIntentFact> {
    let decoded: EntryOrderIntentLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::EntryOrderIntentV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.order_intent.into_fact();
    validate_details(&fact.details).map_err(anyhow::Error::new)?;
    Ok(fact)
}

pub(super) fn encode_risk_reducing_exit(
    fact: RiskReducingExitOrderIntentFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_details(&fact.details)?;
    let purpose = KnownPurpose::RiskReducingExitOrderIntent;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &RiskReducingExitOrderIntentLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            order_intent: RiskReducingExitOrderIntentV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_risk_reducing_exit(
    line: &str,
    line_number: usize,
) -> Result<RiskReducingExitOrderIntentFact> {
    let decoded: RiskReducingExitOrderIntentLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::RiskReducingExitOrderIntentV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.order_intent.into_fact();
    validate_details(&fact.details).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_details(details: &OrderIntentDetails) -> Result<(), RecordFailure> {
    let required = [
        details.strategy_id.as_str(),
        details.instrument_id.as_str(),
        details.client_order_id.as_str(),
        details.order_side.as_str(),
        details.price.as_str(),
        details.quantity.as_str(),
        details.order_fields.order_type.as_str(),
        details.order_fields.time_in_force.as_str(),
    ];
    let optional = [
        details.order_fields.price.as_deref(),
        details.order_fields.trigger_price.as_deref(),
        details.order_fields.activation_price.as_deref(),
        details.order_fields.trigger_type.as_deref(),
        details.order_fields.trigger_instrument_id.as_deref(),
        details.order_fields.trailing_offset.as_deref(),
        details.order_fields.trailing_offset_type.as_deref(),
        details.order_fields.expire_time_unix_nanos.as_deref(),
    ];
    let clamped_original = match details.clamp_outcome.as_ref() {
        Some(OrderIntentClampOutcome::Clamped { original_quantity }) => {
            Some(original_quantity.as_str())
        }
        None
        | Some(
            OrderIntentClampOutcome::WithinBounds
            | OrderIntentClampOutcome::Rejected
            | OrderIntentClampOutcome::NotEvaluated { .. },
        ) => None,
    };
    if required.into_iter().any(|value| value.trim().is_empty())
        || optional
            .into_iter()
            .flatten()
            .chain(clamped_original)
            .any(|value| value.trim().is_empty())
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "order intent contains an empty required or present optional field"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryOrderIntentLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    order_intent: EntryOrderIntentV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryOrderIntentV1 {
    strategy_id: String,
    instrument_id: String,
    client_order_id: String,
    order_side: String,
    price: String,
    quantity: String,
    clamp_outcome: Option<EntryClampOutcomeV1>,
    order_fields: EntryOrderFieldsV1,
}

impl EntryOrderIntentV1 {
    fn from_fact(fact: EntryOrderIntentFact) -> Self {
        let details = fact.details;
        Self {
            strategy_id: details.strategy_id,
            instrument_id: details.instrument_id,
            client_order_id: details.client_order_id,
            order_side: details.order_side,
            price: details.price,
            quantity: details.quantity,
            clamp_outcome: details.clamp_outcome.map(EntryClampOutcomeV1::from_fact),
            order_fields: EntryOrderFieldsV1::from_fact(details.order_fields),
        }
    }

    fn into_fact(self) -> EntryOrderIntentFact {
        EntryOrderIntentFact {
            details: OrderIntentDetails {
                strategy_id: self.strategy_id,
                instrument_id: self.instrument_id,
                client_order_id: self.client_order_id,
                order_side: self.order_side,
                price: self.price,
                quantity: self.quantity,
                clamp_outcome: self.clamp_outcome.map(EntryClampOutcomeV1::into_fact),
                order_fields: self.order_fields.into_fact(),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum EntryClampOutcomeV1 {
    WithinBounds,
    Clamped { original_quantity: String },
    Rejected,
    NotEvaluated { reason: EntryClampReasonV1 },
}

impl EntryClampOutcomeV1 {
    fn from_fact(outcome: OrderIntentClampOutcome) -> Self {
        match outcome {
            OrderIntentClampOutcome::WithinBounds => Self::WithinBounds,
            OrderIntentClampOutcome::Clamped { original_quantity } => {
                Self::Clamped { original_quantity }
            }
            OrderIntentClampOutcome::Rejected => Self::Rejected,
            OrderIntentClampOutcome::NotEvaluated { reason } => Self::NotEvaluated {
                reason: EntryClampReasonV1::from_fact(reason),
            },
        }
    }

    fn into_fact(self) -> OrderIntentClampOutcome {
        match self {
            Self::WithinBounds => OrderIntentClampOutcome::WithinBounds,
            Self::Clamped { original_quantity } => {
                OrderIntentClampOutcome::Clamped { original_quantity }
            }
            Self::Rejected => OrderIntentClampOutcome::Rejected,
            Self::NotEvaluated { reason } => OrderIntentClampOutcome::NotEvaluated {
                reason: reason.into_fact(),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryClampReasonV1 {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

impl EntryClampReasonV1 {
    fn from_fact(reason: OrderIntentClampNotEvaluatedReason) -> Self {
        match reason {
            OrderIntentClampNotEvaluatedReason::NoVenueTruth => Self::NoVenueTruth,
            OrderIntentClampNotEvaluatedReason::ForeignInstrument => Self::ForeignInstrument,
            OrderIntentClampNotEvaluatedReason::NonSellOrderSide => Self::NonSellOrderSide,
        }
    }

    fn into_fact(self) -> OrderIntentClampNotEvaluatedReason {
        match self {
            Self::NoVenueTruth => OrderIntentClampNotEvaluatedReason::NoVenueTruth,
            Self::ForeignInstrument => OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            Self::NonSellOrderSide => OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryOrderFieldsV1 {
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

impl EntryOrderFieldsV1 {
    fn from_fact(fields: OrderIntentOrderFields) -> Self {
        Self {
            order_type: fields.order_type,
            time_in_force: fields.time_in_force,
            price: fields.price,
            trigger_price: fields.trigger_price,
            activation_price: fields.activation_price,
            trigger_type: fields.trigger_type,
            trigger_instrument_id: fields.trigger_instrument_id,
            trailing_offset: fields.trailing_offset,
            trailing_offset_type: fields.trailing_offset_type,
            expire_time_unix_nanos: fields.expire_time_unix_nanos,
            is_post_only: fields.is_post_only,
            is_reduce_only: fields.is_reduce_only,
            is_quote_quantity: fields.is_quote_quantity,
        }
    }

    fn into_fact(self) -> OrderIntentOrderFields {
        OrderIntentOrderFields {
            order_type: self.order_type,
            time_in_force: self.time_in_force,
            price: self.price,
            trigger_price: self.trigger_price,
            activation_price: self.activation_price,
            trigger_type: self.trigger_type,
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: self.trailing_offset,
            trailing_offset_type: self.trailing_offset_type,
            expire_time_unix_nanos: self.expire_time_unix_nanos,
            is_post_only: self.is_post_only,
            is_reduce_only: self.is_reduce_only,
            is_quote_quantity: self.is_quote_quantity,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RiskReducingExitOrderIntentLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    order_intent: RiskReducingExitOrderIntentV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RiskReducingExitOrderIntentV1 {
    strategy_id: String,
    instrument_id: String,
    client_order_id: String,
    order_side: String,
    price: String,
    quantity: String,
    clamp_outcome: Option<ExitClampOutcomeV1>,
    order_fields: ExitOrderFieldsV1,
}

impl RiskReducingExitOrderIntentV1 {
    fn from_fact(fact: RiskReducingExitOrderIntentFact) -> Self {
        let details = fact.details;
        Self {
            strategy_id: details.strategy_id,
            instrument_id: details.instrument_id,
            client_order_id: details.client_order_id,
            order_side: details.order_side,
            price: details.price,
            quantity: details.quantity,
            clamp_outcome: details.clamp_outcome.map(ExitClampOutcomeV1::from_fact),
            order_fields: ExitOrderFieldsV1::from_fact(details.order_fields),
        }
    }

    fn into_fact(self) -> RiskReducingExitOrderIntentFact {
        RiskReducingExitOrderIntentFact {
            details: OrderIntentDetails {
                strategy_id: self.strategy_id,
                instrument_id: self.instrument_id,
                client_order_id: self.client_order_id,
                order_side: self.order_side,
                price: self.price,
                quantity: self.quantity,
                clamp_outcome: self.clamp_outcome.map(ExitClampOutcomeV1::into_fact),
                order_fields: self.order_fields.into_fact(),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
enum ExitClampOutcomeV1 {
    WithinBounds,
    Clamped { original_quantity: String },
    Rejected,
    NotEvaluated { reason: ExitClampReasonV1 },
}

impl ExitClampOutcomeV1 {
    fn from_fact(outcome: OrderIntentClampOutcome) -> Self {
        match outcome {
            OrderIntentClampOutcome::WithinBounds => Self::WithinBounds,
            OrderIntentClampOutcome::Clamped { original_quantity } => {
                Self::Clamped { original_quantity }
            }
            OrderIntentClampOutcome::Rejected => Self::Rejected,
            OrderIntentClampOutcome::NotEvaluated { reason } => Self::NotEvaluated {
                reason: ExitClampReasonV1::from_fact(reason),
            },
        }
    }

    fn into_fact(self) -> OrderIntentClampOutcome {
        match self {
            Self::WithinBounds => OrderIntentClampOutcome::WithinBounds,
            Self::Clamped { original_quantity } => {
                OrderIntentClampOutcome::Clamped { original_quantity }
            }
            Self::Rejected => OrderIntentClampOutcome::Rejected,
            Self::NotEvaluated { reason } => OrderIntentClampOutcome::NotEvaluated {
                reason: reason.into_fact(),
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExitClampReasonV1 {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

impl ExitClampReasonV1 {
    fn from_fact(reason: OrderIntentClampNotEvaluatedReason) -> Self {
        match reason {
            OrderIntentClampNotEvaluatedReason::NoVenueTruth => Self::NoVenueTruth,
            OrderIntentClampNotEvaluatedReason::ForeignInstrument => Self::ForeignInstrument,
            OrderIntentClampNotEvaluatedReason::NonSellOrderSide => Self::NonSellOrderSide,
        }
    }

    fn into_fact(self) -> OrderIntentClampNotEvaluatedReason {
        match self {
            Self::NoVenueTruth => OrderIntentClampNotEvaluatedReason::NoVenueTruth,
            Self::ForeignInstrument => OrderIntentClampNotEvaluatedReason::ForeignInstrument,
            Self::NonSellOrderSide => OrderIntentClampNotEvaluatedReason::NonSellOrderSide,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitOrderFieldsV1 {
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

impl ExitOrderFieldsV1 {
    fn from_fact(fields: OrderIntentOrderFields) -> Self {
        Self {
            order_type: fields.order_type,
            time_in_force: fields.time_in_force,
            price: fields.price,
            trigger_price: fields.trigger_price,
            activation_price: fields.activation_price,
            trigger_type: fields.trigger_type,
            trigger_instrument_id: fields.trigger_instrument_id,
            trailing_offset: fields.trailing_offset,
            trailing_offset_type: fields.trailing_offset_type,
            expire_time_unix_nanos: fields.expire_time_unix_nanos,
            is_post_only: fields.is_post_only,
            is_reduce_only: fields.is_reduce_only,
            is_quote_quantity: fields.is_quote_quantity,
        }
    }

    fn into_fact(self) -> OrderIntentOrderFields {
        OrderIntentOrderFields {
            order_type: self.order_type,
            time_in_force: self.time_in_force,
            price: self.price,
            trigger_price: self.trigger_price,
            activation_price: self.activation_price,
            trigger_type: self.trigger_type,
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: self.trailing_offset,
            trailing_offset_type: self.trailing_offset_type,
            expire_time_unix_nanos: self.expire_time_unix_nanos,
            is_post_only: self.is_post_only,
            is_reduce_only: self.is_reduce_only,
            is_quote_quantity: self.is_quote_quantity,
        }
    }
}
