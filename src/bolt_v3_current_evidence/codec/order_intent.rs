use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        EntryOrderIntentFact, EvidenceOrderSide, EvidenceOrderType, EvidenceTimeInForce,
        EvidenceTrailingOffsetType, EvidenceTriggerType, OrderIntentClampNotEvaluatedReason,
        OrderIntentClampOutcome, OrderIntentDetails, OrderIntentOrderFields,
        RiskReducingExitOrderIntentFact,
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
        details.price.as_str(),
        details.quantity.as_str(),
    ];
    let optional = [
        details.order_fields.price.as_deref(),
        details.order_fields.trigger_price.as_deref(),
        details.order_fields.activation_price.as_deref(),
        details.order_fields.trigger_instrument_id.as_deref(),
        details.order_fields.trailing_offset.as_deref(),
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
    if details.order_side == EvidenceOrderSide::Unspecified
        || required.into_iter().any(|value| value.trim().is_empty())
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

macro_rules! identity_order_domain_v1 {
    (
        $side:ident,
        $order_type:ident,
        $time_in_force:ident,
        $trigger_type:ident,
        $trailing_offset_type:ident
    ) => {
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $side {
            Unspecified,
            Buy,
            Sell,
        }

        impl $side {
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
        enum $order_type {
            Market,
            Limit,
            StopMarket,
            StopLimit,
            MarketToLimit,
            MarketIfTouched,
            LimitIfTouched,
            TrailingStopMarket,
            TrailingStopLimit,
        }

        impl $order_type {
            fn from_fact(value: EvidenceOrderType) -> Self {
                match value {
                    EvidenceOrderType::Market => Self::Market,
                    EvidenceOrderType::Limit => Self::Limit,
                    EvidenceOrderType::StopMarket => Self::StopMarket,
                    EvidenceOrderType::StopLimit => Self::StopLimit,
                    EvidenceOrderType::MarketToLimit => Self::MarketToLimit,
                    EvidenceOrderType::MarketIfTouched => Self::MarketIfTouched,
                    EvidenceOrderType::LimitIfTouched => Self::LimitIfTouched,
                    EvidenceOrderType::TrailingStopMarket => Self::TrailingStopMarket,
                    EvidenceOrderType::TrailingStopLimit => Self::TrailingStopLimit,
                }
            }

            fn into_fact(self) -> EvidenceOrderType {
                match self {
                    Self::Market => EvidenceOrderType::Market,
                    Self::Limit => EvidenceOrderType::Limit,
                    Self::StopMarket => EvidenceOrderType::StopMarket,
                    Self::StopLimit => EvidenceOrderType::StopLimit,
                    Self::MarketToLimit => EvidenceOrderType::MarketToLimit,
                    Self::MarketIfTouched => EvidenceOrderType::MarketIfTouched,
                    Self::LimitIfTouched => EvidenceOrderType::LimitIfTouched,
                    Self::TrailingStopMarket => EvidenceOrderType::TrailingStopMarket,
                    Self::TrailingStopLimit => EvidenceOrderType::TrailingStopLimit,
                }
            }
        }

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $time_in_force {
            Gtc,
            Ioc,
            Fok,
            Gtd,
            Day,
            AtTheOpen,
            AtTheClose,
        }

        impl $time_in_force {
            fn from_fact(value: EvidenceTimeInForce) -> Self {
                match value {
                    EvidenceTimeInForce::Gtc => Self::Gtc,
                    EvidenceTimeInForce::Ioc => Self::Ioc,
                    EvidenceTimeInForce::Fok => Self::Fok,
                    EvidenceTimeInForce::Gtd => Self::Gtd,
                    EvidenceTimeInForce::Day => Self::Day,
                    EvidenceTimeInForce::AtTheOpen => Self::AtTheOpen,
                    EvidenceTimeInForce::AtTheClose => Self::AtTheClose,
                }
            }

            fn into_fact(self) -> EvidenceTimeInForce {
                match self {
                    Self::Gtc => EvidenceTimeInForce::Gtc,
                    Self::Ioc => EvidenceTimeInForce::Ioc,
                    Self::Fok => EvidenceTimeInForce::Fok,
                    Self::Gtd => EvidenceTimeInForce::Gtd,
                    Self::Day => EvidenceTimeInForce::Day,
                    Self::AtTheOpen => EvidenceTimeInForce::AtTheOpen,
                    Self::AtTheClose => EvidenceTimeInForce::AtTheClose,
                }
            }
        }

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $trigger_type {
            NoTrigger,
            Default,
            LastPrice,
            MarkPrice,
            IndexPrice,
            BidAsk,
            DoubleLast,
            DoubleBidAsk,
            LastOrBidAsk,
            MidPoint,
        }

        impl $trigger_type {
            fn from_fact(value: EvidenceTriggerType) -> Self {
                match value {
                    EvidenceTriggerType::NoTrigger => Self::NoTrigger,
                    EvidenceTriggerType::Default => Self::Default,
                    EvidenceTriggerType::LastPrice => Self::LastPrice,
                    EvidenceTriggerType::MarkPrice => Self::MarkPrice,
                    EvidenceTriggerType::IndexPrice => Self::IndexPrice,
                    EvidenceTriggerType::BidAsk => Self::BidAsk,
                    EvidenceTriggerType::DoubleLast => Self::DoubleLast,
                    EvidenceTriggerType::DoubleBidAsk => Self::DoubleBidAsk,
                    EvidenceTriggerType::LastOrBidAsk => Self::LastOrBidAsk,
                    EvidenceTriggerType::MidPoint => Self::MidPoint,
                }
            }

            fn into_fact(self) -> EvidenceTriggerType {
                match self {
                    Self::NoTrigger => EvidenceTriggerType::NoTrigger,
                    Self::Default => EvidenceTriggerType::Default,
                    Self::LastPrice => EvidenceTriggerType::LastPrice,
                    Self::MarkPrice => EvidenceTriggerType::MarkPrice,
                    Self::IndexPrice => EvidenceTriggerType::IndexPrice,
                    Self::BidAsk => EvidenceTriggerType::BidAsk,
                    Self::DoubleLast => EvidenceTriggerType::DoubleLast,
                    Self::DoubleBidAsk => EvidenceTriggerType::DoubleBidAsk,
                    Self::LastOrBidAsk => EvidenceTriggerType::LastOrBidAsk,
                    Self::MidPoint => EvidenceTriggerType::MidPoint,
                }
            }
        }

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $trailing_offset_type {
            NoTrailingOffset,
            Price,
            BasisPoints,
            Ticks,
            PriceTier,
        }

        impl $trailing_offset_type {
            fn from_fact(value: EvidenceTrailingOffsetType) -> Self {
                match value {
                    EvidenceTrailingOffsetType::NoTrailingOffset => Self::NoTrailingOffset,
                    EvidenceTrailingOffsetType::Price => Self::Price,
                    EvidenceTrailingOffsetType::BasisPoints => Self::BasisPoints,
                    EvidenceTrailingOffsetType::Ticks => Self::Ticks,
                    EvidenceTrailingOffsetType::PriceTier => Self::PriceTier,
                }
            }

            fn into_fact(self) -> EvidenceTrailingOffsetType {
                match self {
                    Self::NoTrailingOffset => EvidenceTrailingOffsetType::NoTrailingOffset,
                    Self::Price => EvidenceTrailingOffsetType::Price,
                    Self::BasisPoints => EvidenceTrailingOffsetType::BasisPoints,
                    Self::Ticks => EvidenceTrailingOffsetType::Ticks,
                    Self::PriceTier => EvidenceTrailingOffsetType::PriceTier,
                }
            }
        }
    };
}

identity_order_domain_v1!(
    EntryOrderSideV1,
    EntryOrderTypeV1,
    EntryTimeInForceV1,
    EntryTriggerTypeV1,
    EntryTrailingOffsetTypeV1
);
identity_order_domain_v1!(
    ExitOrderSideV1,
    ExitOrderTypeV1,
    ExitTimeInForceV1,
    ExitTriggerTypeV1,
    ExitTrailingOffsetTypeV1
);

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
    order_side: EntryOrderSideV1,
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
            order_side: EntryOrderSideV1::from_fact(details.order_side),
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
                order_side: self.order_side.into_fact(),
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
    order_type: EntryOrderTypeV1,
    time_in_force: EntryTimeInForceV1,
    price: Option<String>,
    trigger_price: Option<String>,
    activation_price: Option<String>,
    trigger_type: Option<EntryTriggerTypeV1>,
    trigger_instrument_id: Option<String>,
    trailing_offset: Option<String>,
    trailing_offset_type: Option<EntryTrailingOffsetTypeV1>,
    expire_time_unix_nanos: Option<String>,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
}

impl EntryOrderFieldsV1 {
    fn from_fact(fields: OrderIntentOrderFields) -> Self {
        Self {
            order_type: EntryOrderTypeV1::from_fact(fields.order_type),
            time_in_force: EntryTimeInForceV1::from_fact(fields.time_in_force),
            price: fields.price,
            trigger_price: fields.trigger_price,
            activation_price: fields.activation_price,
            trigger_type: fields.trigger_type.map(EntryTriggerTypeV1::from_fact),
            trigger_instrument_id: fields.trigger_instrument_id,
            trailing_offset: fields.trailing_offset,
            trailing_offset_type: fields
                .trailing_offset_type
                .map(EntryTrailingOffsetTypeV1::from_fact),
            expire_time_unix_nanos: fields.expire_time_unix_nanos,
            is_post_only: fields.is_post_only,
            is_reduce_only: fields.is_reduce_only,
            is_quote_quantity: fields.is_quote_quantity,
        }
    }

    fn into_fact(self) -> OrderIntentOrderFields {
        OrderIntentOrderFields {
            order_type: self.order_type.into_fact(),
            time_in_force: self.time_in_force.into_fact(),
            price: self.price,
            trigger_price: self.trigger_price,
            activation_price: self.activation_price,
            trigger_type: self.trigger_type.map(EntryTriggerTypeV1::into_fact),
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: self.trailing_offset,
            trailing_offset_type: self
                .trailing_offset_type
                .map(EntryTrailingOffsetTypeV1::into_fact),
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
    order_side: ExitOrderSideV1,
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
            order_side: ExitOrderSideV1::from_fact(details.order_side),
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
                order_side: self.order_side.into_fact(),
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
    order_type: ExitOrderTypeV1,
    time_in_force: ExitTimeInForceV1,
    price: Option<String>,
    trigger_price: Option<String>,
    activation_price: Option<String>,
    trigger_type: Option<ExitTriggerTypeV1>,
    trigger_instrument_id: Option<String>,
    trailing_offset: Option<String>,
    trailing_offset_type: Option<ExitTrailingOffsetTypeV1>,
    expire_time_unix_nanos: Option<String>,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
}

impl ExitOrderFieldsV1 {
    fn from_fact(fields: OrderIntentOrderFields) -> Self {
        Self {
            order_type: ExitOrderTypeV1::from_fact(fields.order_type),
            time_in_force: ExitTimeInForceV1::from_fact(fields.time_in_force),
            price: fields.price,
            trigger_price: fields.trigger_price,
            activation_price: fields.activation_price,
            trigger_type: fields.trigger_type.map(ExitTriggerTypeV1::from_fact),
            trigger_instrument_id: fields.trigger_instrument_id,
            trailing_offset: fields.trailing_offset,
            trailing_offset_type: fields
                .trailing_offset_type
                .map(ExitTrailingOffsetTypeV1::from_fact),
            expire_time_unix_nanos: fields.expire_time_unix_nanos,
            is_post_only: fields.is_post_only,
            is_reduce_only: fields.is_reduce_only,
            is_quote_quantity: fields.is_quote_quantity,
        }
    }

    fn into_fact(self) -> OrderIntentOrderFields {
        OrderIntentOrderFields {
            order_type: self.order_type.into_fact(),
            time_in_force: self.time_in_force.into_fact(),
            price: self.price,
            trigger_price: self.trigger_price,
            activation_price: self.activation_price,
            trigger_type: self.trigger_type.map(ExitTriggerTypeV1::into_fact),
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: self.trailing_offset,
            trailing_offset_type: self
                .trailing_offset_type
                .map(ExitTrailingOffsetTypeV1::into_fact),
            expire_time_unix_nanos: self.expire_time_unix_nanos,
            is_post_only: self.is_post_only,
            is_reduce_only: self.is_reduce_only,
            is_quote_quantity: self.is_quote_quantity,
        }
    }
}
