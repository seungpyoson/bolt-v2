use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{SubmitReservationFillFact, SubmitReservationMetadataFact},
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_nonempty,
    validate_recorded_at,
};

pub(super) fn encode_metadata(
    fact: SubmitReservationMetadataFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_metadata(&fact)?;
    let purpose = KnownPurpose::SubmitReservationMetadata;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &MetadataLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            metadata: MetadataV1::from_fact(fact),
        },
    )
}

fn validate_metadata(fact: &SubmitReservationMetadataFact) -> Result<(), RecordFailure> {
    validate_nonempty(
        "submit reservation metadata",
        [
            fact.client_order_id.as_str(),
            fact.submit_reservation_id.as_str(),
            fact.venue_id.as_str(),
            fact.account_id.as_str(),
            fact.product_kind.as_str(),
            fact.collateral_currency.as_str(),
            fact.capital_pool_id.as_str(),
            fact.collateral_group_id.as_str(),
            fact.instrument_id.as_str(),
            fact.side.as_str(),
            fact.submitted_quantity.as_str(),
            fact.liability_factor.as_str(),
            fact.additive_liability.as_str(),
            fact.reserved_liability.as_str(),
            fact.source.as_str(),
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
    validate_nonempty(
        "submit reservation fill",
        [
            fact.client_order_id.as_str(),
            fact.submit_reservation_id.as_str(),
            fact.trade_id.as_str(),
            fact.instrument_id.as_str(),
            fact.side.as_str(),
            fact.fill_quantity.as_str(),
            fact.source.as_str(),
        ],
        fact.observed_at_ns,
    )
}

pub(super) fn decode_metadata(
    line: &str,
    line_number: usize,
) -> Result<SubmitReservationMetadataFact> {
    let decoded: MetadataLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::SubmitReservationMetadataV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.metadata.into_fact();
    validate_metadata(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
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
struct MetadataLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    metadata: MetadataV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MetadataV1 {
    client_order_id: String,
    submit_reservation_id: String,
    venue_id: String,
    account_id: String,
    product_kind: String,
    collateral_currency: String,
    capital_pool_id: String,
    collateral_group_id: String,
    instrument_id: String,
    side: String,
    submitted_quantity: String,
    liability_factor: String,
    additive_liability: String,
    reserved_liability: String,
    observed_at_ns: u64,
    source: String,
}

impl MetadataV1 {
    fn from_fact(fact: SubmitReservationMetadataFact) -> Self {
        Self {
            client_order_id: fact.client_order_id,
            submit_reservation_id: fact.submit_reservation_id,
            venue_id: fact.venue_id,
            account_id: fact.account_id,
            product_kind: fact.product_kind,
            collateral_currency: fact.collateral_currency,
            capital_pool_id: fact.capital_pool_id,
            collateral_group_id: fact.collateral_group_id,
            instrument_id: fact.instrument_id,
            side: fact.side,
            submitted_quantity: fact.submitted_quantity,
            liability_factor: fact.liability_factor,
            additive_liability: fact.additive_liability,
            reserved_liability: fact.reserved_liability,
            observed_at_ns: fact.observed_at_ns,
            source: fact.source,
        }
    }

    fn into_fact(self) -> SubmitReservationMetadataFact {
        SubmitReservationMetadataFact {
            client_order_id: self.client_order_id,
            submit_reservation_id: self.submit_reservation_id,
            venue_id: self.venue_id,
            account_id: self.account_id,
            product_kind: self.product_kind,
            collateral_currency: self.collateral_currency,
            capital_pool_id: self.capital_pool_id,
            collateral_group_id: self.collateral_group_id,
            instrument_id: self.instrument_id,
            side: self.side,
            submitted_quantity: self.submitted_quantity,
            liability_factor: self.liability_factor,
            additive_liability: self.additive_liability,
            reserved_liability: self.reserved_liability,
            observed_at_ns: self.observed_at_ns,
            source: self.source,
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
    side: String,
    fill_quantity: String,
    observed_at_ns: u64,
    reconciliation: bool,
    source: String,
}

impl FillV1 {
    fn from_fact(fact: SubmitReservationFillFact) -> Self {
        Self {
            client_order_id: fact.client_order_id,
            submit_reservation_id: fact.submit_reservation_id,
            trade_id: fact.trade_id,
            instrument_id: fact.instrument_id,
            side: fact.side,
            fill_quantity: fact.fill_quantity,
            observed_at_ns: fact.observed_at_ns,
            reconciliation: fact.reconciliation,
            source: fact.source,
        }
    }

    fn into_fact(self) -> SubmitReservationFillFact {
        SubmitReservationFillFact {
            client_order_id: self.client_order_id,
            submit_reservation_id: self.submit_reservation_id,
            trade_id: self.trade_id,
            instrument_id: self.instrument_id,
            side: self.side,
            fill_quantity: self.fill_quantity,
            observed_at_ns: self.observed_at_ns,
            reconciliation: self.reconciliation,
            source: self.source,
        }
    }
}
