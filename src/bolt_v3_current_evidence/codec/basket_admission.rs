use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        BasketAdmissionDetails, BasketAdmissionGrantedFact, BasketAdmissionRejectedFact,
        BasketAdmissionRejectionReason,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_granted(
    fact: BasketAdmissionGrantedFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_details(&fact.details)?;
    let purpose = KnownPurpose::BasketAdmissionGranted;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &GrantedLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision: GrantedDecisionV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_granted(line: &str, line_number: usize) -> Result<BasketAdmissionGrantedFact> {
    let decoded: GrantedLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::BasketAdmissionGrantedV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.decision.into_fact();
    validate_details(&fact.details).map_err(anyhow::Error::new)?;
    Ok(fact)
}

pub(super) fn encode_rejected(
    fact: BasketAdmissionRejectedFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_details(&fact.details)?;
    let purpose = KnownPurpose::BasketAdmissionRejected;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &RejectedLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision: RejectedDecisionV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_rejected(
    line: &str,
    line_number: usize,
) -> Result<BasketAdmissionRejectedFact> {
    let decoded: RejectedLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::BasketAdmissionRejectedV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.decision.into_fact();
    validate_details(&fact.details).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_details(details: &BasketAdmissionDetails) -> Result<(), RecordFailure> {
    if details.strategy_id.trim().is_empty()
        || details.execution_client_id.trim().is_empty()
        || details.basket_id.trim().is_empty()
        || details.group_id.trim().is_empty()
        || details.total_notional.trim().is_empty()
        || details.leg_order_count == 0
        || details.leg_instrument_ids.is_empty()
        || details
            .leg_instrument_ids
            .iter()
            .any(|instrument| instrument.trim().is_empty())
        || usize::try_from(details.leg_order_count).ok() != Some(details.leg_instrument_ids.len())
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "basket admission contains invalid or inconsistent fields"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantedLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: GrantedDecisionV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantedDecisionV1 {
    strategy_id: String,
    execution_client_id: String,
    basket_id: String,
    group_id: String,
    leg_instrument_ids: Vec<String>,
    total_notional: String,
    leg_order_count: u32,
}

impl GrantedDecisionV1 {
    fn from_fact(fact: BasketAdmissionGrantedFact) -> Self {
        let details = fact.details;
        Self {
            strategy_id: details.strategy_id,
            execution_client_id: details.execution_client_id,
            basket_id: details.basket_id,
            group_id: details.group_id,
            leg_instrument_ids: details.leg_instrument_ids,
            total_notional: details.total_notional,
            leg_order_count: details.leg_order_count,
        }
    }

    fn into_fact(self) -> BasketAdmissionGrantedFact {
        BasketAdmissionGrantedFact {
            details: BasketAdmissionDetails {
                strategy_id: self.strategy_id,
                execution_client_id: self.execution_client_id,
                basket_id: self.basket_id,
                group_id: self.group_id,
                leg_instrument_ids: self.leg_instrument_ids,
                total_notional: self.total_notional,
                leg_order_count: self.leg_order_count,
            },
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectedLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: RejectedDecisionV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RejectedDecisionV1 {
    strategy_id: String,
    execution_client_id: String,
    basket_id: String,
    group_id: String,
    leg_instrument_ids: Vec<String>,
    total_notional: String,
    leg_order_count: u32,
    reason: RejectionReasonV1,
}

impl RejectedDecisionV1 {
    fn from_fact(fact: BasketAdmissionRejectedFact) -> Self {
        let details = fact.details;
        Self {
            strategy_id: details.strategy_id,
            execution_client_id: details.execution_client_id,
            basket_id: details.basket_id,
            group_id: details.group_id,
            leg_instrument_ids: details.leg_instrument_ids,
            total_notional: details.total_notional,
            leg_order_count: details.leg_order_count,
            reason: RejectionReasonV1::from_fact(fact.reason),
        }
    }

    fn into_fact(self) -> BasketAdmissionRejectedFact {
        BasketAdmissionRejectedFact {
            details: BasketAdmissionDetails {
                strategy_id: self.strategy_id,
                execution_client_id: self.execution_client_id,
                basket_id: self.basket_id,
                group_id: self.group_id,
                leg_instrument_ids: self.leg_instrument_ids,
                total_notional: self.total_notional,
                leg_order_count: self.leg_order_count,
            },
            reason: self.reason.into_fact(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RejectionReasonV1 {
    BasketNotionalCapExceeded,
    MaxOpenBasketCapExceeded,
    StaleScannerEvidence,
    StaleSubmitRecheck,
    NonPositiveCandidateCost,
    NonPositiveEdge,
    EdgeThreshold,
    MissingGroupingProof,
    MissingSettlementRules,
    RetryBudgetExceeded,
    SubmitSlots,
}

impl RejectionReasonV1 {
    fn from_fact(reason: BasketAdmissionRejectionReason) -> Self {
        match reason {
            BasketAdmissionRejectionReason::BasketNotionalCapExceeded => {
                Self::BasketNotionalCapExceeded
            }
            BasketAdmissionRejectionReason::MaxOpenBasketCapExceeded => {
                Self::MaxOpenBasketCapExceeded
            }
            BasketAdmissionRejectionReason::StaleScannerEvidence => Self::StaleScannerEvidence,
            BasketAdmissionRejectionReason::StaleSubmitRecheck => Self::StaleSubmitRecheck,
            BasketAdmissionRejectionReason::NonPositiveCandidateCost => {
                Self::NonPositiveCandidateCost
            }
            BasketAdmissionRejectionReason::NonPositiveEdge => Self::NonPositiveEdge,
            BasketAdmissionRejectionReason::EdgeThreshold => Self::EdgeThreshold,
            BasketAdmissionRejectionReason::MissingGroupingProof => Self::MissingGroupingProof,
            BasketAdmissionRejectionReason::MissingSettlementRules => Self::MissingSettlementRules,
            BasketAdmissionRejectionReason::RetryBudgetExceeded => Self::RetryBudgetExceeded,
            BasketAdmissionRejectionReason::SubmitSlots => Self::SubmitSlots,
        }
    }

    fn into_fact(self) -> BasketAdmissionRejectionReason {
        match self {
            Self::BasketNotionalCapExceeded => {
                BasketAdmissionRejectionReason::BasketNotionalCapExceeded
            }
            Self::MaxOpenBasketCapExceeded => {
                BasketAdmissionRejectionReason::MaxOpenBasketCapExceeded
            }
            Self::StaleScannerEvidence => BasketAdmissionRejectionReason::StaleScannerEvidence,
            Self::StaleSubmitRecheck => BasketAdmissionRejectionReason::StaleSubmitRecheck,
            Self::NonPositiveCandidateCost => {
                BasketAdmissionRejectionReason::NonPositiveCandidateCost
            }
            Self::NonPositiveEdge => BasketAdmissionRejectionReason::NonPositiveEdge,
            Self::EdgeThreshold => BasketAdmissionRejectionReason::EdgeThreshold,
            Self::MissingGroupingProof => BasketAdmissionRejectionReason::MissingGroupingProof,
            Self::MissingSettlementRules => BasketAdmissionRejectionReason::MissingSettlementRules,
            Self::RetryBudgetExceeded => BasketAdmissionRejectionReason::RetryBudgetExceeded,
            Self::SubmitSlots => BasketAdmissionRejectionReason::SubmitSlots,
        }
    }
}
