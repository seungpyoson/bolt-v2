use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        BasketAdmissionDetails, BasketAdmissionGrantedFact, BasketAdmissionIntentKind,
        BasketAdmissionRejectedFact, BasketAdmissionRejectionReason, BasketAdmittedLeg,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::reservation::{ReservationAttributionV1, validate_attribution};
use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_granted(
    fact: BasketAdmissionGrantedFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_details(&fact.details)?;
    validate_admitted_legs(&fact)?;
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
    validate_admitted_legs(&fact).map_err(anyhow::Error::new)?;
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

fn validate_admitted_legs(fact: &BasketAdmissionGrantedFact) -> Result<(), RecordFailure> {
    if fact.admitted_legs.len() != fact.details.leg_instrument_ids.len() {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "basket admitted legs must match the declared leg count"
        )));
    }
    let mut client_order_ids = std::collections::BTreeSet::new();
    let mut reservation_ids = std::collections::BTreeSet::new();
    for (leg, expected_instrument_id) in fact
        .admitted_legs
        .iter()
        .zip(&fact.details.leg_instrument_ids)
    {
        if leg.client_order_id.trim().is_empty()
            || leg.instrument_id != *expected_instrument_id
            || !client_order_ids.insert(leg.client_order_id.as_str())
        {
            return Err(RecordFailure::Rejected(anyhow::anyhow!(
                "basket admitted leg correlation is invalid"
            )));
        }
        if let Some(reservation) = leg.reservation.as_ref() {
            if leg.intent_kind != BasketAdmissionIntentKind::Entry {
                return Err(RecordFailure::Rejected(anyhow::anyhow!(
                    "risk-reducing basket legs cannot carry capital reservation attribution"
                )));
            }
            validate_attribution(reservation)?;
            if reservation.client_order_id != leg.client_order_id
                || reservation.instrument_id != leg.instrument_id
                || !reservation_ids.insert(reservation.submit_reservation_id.as_str())
            {
                return Err(RecordFailure::Rejected(anyhow::anyhow!(
                    "basket reservation attribution is invalid or duplicated"
                )));
            }
        }
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
    admitted_legs: Vec<BasketAdmittedLegV1>,
}

impl GrantedDecisionV1 {
    fn from_fact(fact: BasketAdmissionGrantedFact) -> Self {
        let BasketAdmissionGrantedFact {
            details,
            admitted_legs,
        } = fact;
        Self {
            strategy_id: details.strategy_id,
            execution_client_id: details.execution_client_id,
            basket_id: details.basket_id,
            group_id: details.group_id,
            leg_instrument_ids: details.leg_instrument_ids,
            total_notional: details.total_notional,
            leg_order_count: details.leg_order_count,
            admitted_legs: admitted_legs
                .into_iter()
                .map(BasketAdmittedLegV1::from_fact)
                .collect(),
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
            admitted_legs: self
                .admitted_legs
                .into_iter()
                .map(BasketAdmittedLegV1::into_fact)
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasketAdmittedLegV1 {
    client_order_id: String,
    instrument_id: String,
    intent_kind: BasketAdmissionIntentKindV1,
    reservation: Option<ReservationAttributionV1>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BasketAdmissionIntentKindV1 {
    Entry,
    RiskReducingExit,
}

impl BasketAdmittedLegV1 {
    fn from_fact(fact: BasketAdmittedLeg) -> Self {
        Self {
            client_order_id: fact.client_order_id,
            instrument_id: fact.instrument_id,
            intent_kind: match fact.intent_kind {
                BasketAdmissionIntentKind::Entry => BasketAdmissionIntentKindV1::Entry,
                BasketAdmissionIntentKind::RiskReducingExit => {
                    BasketAdmissionIntentKindV1::RiskReducingExit
                }
            },
            reservation: fact.reservation.map(ReservationAttributionV1::from_fact),
        }
    }

    fn into_fact(self) -> BasketAdmittedLeg {
        BasketAdmittedLeg {
            client_order_id: self.client_order_id,
            instrument_id: self.instrument_id,
            intent_kind: match self.intent_kind {
                BasketAdmissionIntentKindV1::Entry => BasketAdmissionIntentKind::Entry,
                BasketAdmissionIntentKindV1::RiskReducingExit => {
                    BasketAdmissionIntentKind::RiskReducingExit
                }
            },
            reservation: self.reservation.map(ReservationAttributionV1::into_fact),
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
