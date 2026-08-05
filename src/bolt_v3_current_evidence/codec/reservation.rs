use anyhow::Result;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        EvidenceOrderSide, ReservationAttribution, ReservationProductKind,
        SubmitReservationFillFact, SubmitReservationFillSource,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_nonempty,
    validate_recorded_at,
};

pub(super) fn validate_attribution(fact: &ReservationAttribution) -> Result<(), RecordFailure> {
    if fact.side == EvidenceOrderSide::Unspecified {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "submit reservation attribution side must be specified"
        )));
    }
    validate_nonempty(
        "submit reservation attribution",
        [
            fact.client_order_id.as_str(),
            fact.submit_reservation_id.as_str(),
            fact.venue_id.as_str(),
            fact.account_id.as_str(),
            fact.collateral_currency.as_str(),
            fact.capital_pool_id.as_str(),
            fact.collateral_group_id.as_str(),
            fact.instrument_id.as_str(),
            fact.submitted_quantity.as_str(),
            fact.liability_factor.as_str(),
            fact.additive_liability.as_str(),
            fact.reserved_liability.as_str(),
        ],
        fact.observed_at_ns,
    )
}

pub(super) fn encode_fill(
    fact: SubmitReservationFillFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fill(&fact)?;
    let purpose = KnownPurpose::SubmitReservationFill;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &FillLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            fill: FillV1::from_fact(fact),
        },
    )
}

fn validate_fill(fact: &SubmitReservationFillFact) -> Result<(), RecordFailure> {
    if fact.side == EvidenceOrderSide::Unspecified {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "submit reservation fill side must be specified"
        )));
    }
    validate_nonempty(
        "submit reservation fill",
        [
            fact.client_order_id.as_str(),
            fact.submit_reservation_id.as_str(),
            fact.trade_id.as_str(),
            fact.instrument_id.as_str(),
            fact.fill_quantity.as_str(),
        ],
        fact.observed_at_ns,
    )?;
    if !fact
        .fill_quantity
        .parse::<Decimal>()
        .is_ok_and(|quantity| quantity > Decimal::ZERO)
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "submit reservation fill quantity must be positive"
        )));
    }
    Ok(())
}

pub(super) fn decode_fill(line: &str, line_number: usize) -> Result<SubmitReservationFillFact> {
    let decoded: FillLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::SubmitReservationFillV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.fill.into_fact();
    validate_fill(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReservationAttributionV1 {
    client_order_id: String,
    submit_reservation_id: String,
    venue_id: String,
    account_id: String,
    product_kind: ReservationProductKindV1,
    collateral_currency: String,
    capital_pool_id: String,
    collateral_group_id: String,
    instrument_id: String,
    side: ReservationOrderSideV1,
    submitted_quantity: String,
    liability_factor: String,
    additive_liability: String,
    reserved_liability: String,
    observed_at_ns: u64,
}

impl ReservationAttributionV1 {
    pub(super) fn from_fact(fact: ReservationAttribution) -> Self {
        Self {
            client_order_id: fact.client_order_id,
            submit_reservation_id: fact.submit_reservation_id,
            venue_id: fact.venue_id,
            account_id: fact.account_id,
            product_kind: ReservationProductKindV1::from_fact(fact.product_kind),
            collateral_currency: fact.collateral_currency,
            capital_pool_id: fact.capital_pool_id,
            collateral_group_id: fact.collateral_group_id,
            instrument_id: fact.instrument_id,
            side: ReservationOrderSideV1::from_fact(fact.side),
            submitted_quantity: fact.submitted_quantity,
            liability_factor: fact.liability_factor,
            additive_liability: fact.additive_liability,
            reserved_liability: fact.reserved_liability,
            observed_at_ns: fact.observed_at_ns,
        }
    }

    pub(super) fn into_fact(self) -> ReservationAttribution {
        ReservationAttribution {
            client_order_id: self.client_order_id,
            submit_reservation_id: self.submit_reservation_id,
            venue_id: self.venue_id,
            account_id: self.account_id,
            product_kind: self.product_kind.into_fact(),
            collateral_currency: self.collateral_currency,
            capital_pool_id: self.capital_pool_id,
            collateral_group_id: self.collateral_group_id,
            instrument_id: self.instrument_id,
            side: self.side.into_fact(),
            submitted_quantity: self.submitted_quantity,
            liability_factor: self.liability_factor,
            additive_liability: self.additive_liability,
            reserved_liability: self.reserved_liability,
            observed_at_ns: self.observed_at_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReservationProductKindV1 {
    PredictionMarketBinary,
}

impl ReservationProductKindV1 {
    fn from_fact(value: ReservationProductKind) -> Self {
        match value {
            ReservationProductKind::PredictionMarketBinary => Self::PredictionMarketBinary,
        }
    }

    fn into_fact(self) -> ReservationProductKind {
        match self {
            Self::PredictionMarketBinary => ReservationProductKind::PredictionMarketBinary,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReservationOrderSideV1 {
    Unspecified,
    Buy,
    Sell,
}

impl ReservationOrderSideV1 {
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
#[serde(deny_unknown_fields)]
struct FillLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    fill: FillV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FillV1 {
    client_order_id: String,
    submit_reservation_id: String,
    trade_id: String,
    instrument_id: String,
    side: FillOrderSideV1,
    fill_quantity: String,
    observed_at_ns: u64,
    reconciliation: bool,
    source: FillSourceV1,
}

impl FillV1 {
    fn from_fact(fact: SubmitReservationFillFact) -> Self {
        Self {
            client_order_id: fact.client_order_id,
            submit_reservation_id: fact.submit_reservation_id,
            trade_id: fact.trade_id,
            instrument_id: fact.instrument_id,
            side: FillOrderSideV1::from_fact(fact.side),
            fill_quantity: fact.fill_quantity,
            observed_at_ns: fact.observed_at_ns,
            reconciliation: fact.reconciliation,
            source: FillSourceV1::from_fact(fact.source),
        }
    }

    fn into_fact(self) -> SubmitReservationFillFact {
        SubmitReservationFillFact {
            client_order_id: self.client_order_id,
            submit_reservation_id: self.submit_reservation_id,
            trade_id: self.trade_id,
            instrument_id: self.instrument_id,
            side: self.side.into_fact(),
            fill_quantity: self.fill_quantity,
            observed_at_ns: self.observed_at_ns,
            reconciliation: self.reconciliation,
            source: self.source.into_fact(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FillOrderSideV1 {
    Unspecified,
    Buy,
    Sell,
}

impl FillOrderSideV1 {
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
enum FillSourceV1 {
    NtOrderFill,
}

impl FillSourceV1 {
    fn from_fact(value: SubmitReservationFillSource) -> Self {
        match value {
            SubmitReservationFillSource::NtOrderFill => Self::NtOrderFill,
        }
    }

    fn into_fact(self) -> SubmitReservationFillSource {
        match self {
            Self::NtOrderFill => SubmitReservationFillSource::NtOrderFill,
        }
    }
}
