use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        CapitalAdmissionRebuildFact, CapitalAdmissionRebuildOutcome,
        CapitalAdmissionRejectionReason,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_capital_rebuild(
    fact: CapitalAdmissionRebuildFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fact(&fact)?;
    let purpose = KnownPurpose::CapitalAdmissionRebuild;
    let descriptor = current_line_descriptor(purpose);
    let decision = CapitalAdmissionRebuildV1::from_fact(fact)?;
    encode_line(
        purpose,
        &CapitalAdmissionRebuildLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision,
        },
    )
}

pub(super) fn decode_capital_rebuild(
    line: &str,
    line_number: usize,
) -> Result<CapitalAdmissionRebuildFact> {
    let decoded: CapitalAdmissionRebuildLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::CapitalAdmissionRebuildV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.decision.into_fact()?;
    validate_fact(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_fact(fact: &CapitalAdmissionRebuildFact) -> Result<(), RecordFailure> {
    if fact.observed_at_ns == 0
        || fact.source.trim().is_empty()
        || fact.live_reserved_liability.trim().is_empty()
        || fact.recovered_reservation_count > fact.attempted_reservation_count
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "capital admission rebuild contains invalid or inconsistent fields"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapitalAdmissionRebuildLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: CapitalAdmissionRebuildV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapitalAdmissionRebuildV1 {
    observed_at_ns: u64,
    source: String,
    observed_open_order_count: u64,
    all_open_orders_attributed: bool,
    outcome: CapitalAdmissionRebuildOutcomeV1,
    attempted_reservation_count: u64,
    recovered_reservation_count: u64,
    live_reserved_liability: String,
}

impl CapitalAdmissionRebuildV1 {
    fn from_fact(fact: CapitalAdmissionRebuildFact) -> Result<Self, RecordFailure> {
        Ok(Self {
            observed_at_ns: fact.observed_at_ns,
            source: fact.source,
            observed_open_order_count: u64::try_from(fact.observed_open_order_count).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "observed_open_order_count cannot be encoded: {source}"
                    ))
                },
            )?,
            all_open_orders_attributed: fact.all_open_orders_attributed,
            outcome: CapitalAdmissionRebuildOutcomeV1::from_fact(fact.outcome),
            attempted_reservation_count: u64::try_from(fact.attempted_reservation_count).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "attempted_reservation_count cannot be encoded: {source}"
                    ))
                },
            )?,
            recovered_reservation_count: u64::try_from(fact.recovered_reservation_count).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "recovered_reservation_count cannot be encoded: {source}"
                    ))
                },
            )?,
            live_reserved_liability: fact.live_reserved_liability,
        })
    }

    fn into_fact(self) -> Result<CapitalAdmissionRebuildFact> {
        Ok(CapitalAdmissionRebuildFact {
            observed_at_ns: self.observed_at_ns,
            source: self.source,
            observed_open_order_count: usize::try_from(self.observed_open_order_count)
                .context("observed_open_order_count does not fit usize")?,
            all_open_orders_attributed: self.all_open_orders_attributed,
            outcome: self.outcome.into_fact(),
            attempted_reservation_count: usize::try_from(self.attempted_reservation_count)
                .context("attempted_reservation_count does not fit usize")?,
            recovered_reservation_count: usize::try_from(self.recovered_reservation_count)
                .context("recovered_reservation_count does not fit usize")?,
            live_reserved_liability: self.live_reserved_liability,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
enum CapitalAdmissionRebuildOutcomeV1 {
    Accepted,
    Rejected(CapitalAdmissionRejectionReasonV1),
}

impl CapitalAdmissionRebuildOutcomeV1 {
    fn from_fact(outcome: CapitalAdmissionRebuildOutcome) -> Self {
        match outcome {
            CapitalAdmissionRebuildOutcome::Accepted => Self::Accepted,
            CapitalAdmissionRebuildOutcome::Rejected(reason) => {
                Self::Rejected(CapitalAdmissionRejectionReasonV1::from_fact(reason))
            }
        }
    }

    fn into_fact(self) -> CapitalAdmissionRebuildOutcome {
        match self {
            Self::Accepted => CapitalAdmissionRebuildOutcome::Accepted,
            Self::Rejected(reason) => CapitalAdmissionRebuildOutcome::Rejected(reason.into_fact()),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CapitalAdmissionRejectionReasonV1 {
    MissingEvidence,
    StaleRequest,
    PoolMismatch,
    OverBudget,
    InvalidRequest,
    CollateralGroupMismatch,
    DuplicateReservation,
    UnknownReservation,
    UnknownRelease,
    ReconciliationRequired,
}

impl CapitalAdmissionRejectionReasonV1 {
    fn from_fact(reason: CapitalAdmissionRejectionReason) -> Self {
        match reason {
            CapitalAdmissionRejectionReason::MissingEvidence => Self::MissingEvidence,
            CapitalAdmissionRejectionReason::StaleRequest => Self::StaleRequest,
            CapitalAdmissionRejectionReason::PoolMismatch => Self::PoolMismatch,
            CapitalAdmissionRejectionReason::OverBudget => Self::OverBudget,
            CapitalAdmissionRejectionReason::InvalidRequest => Self::InvalidRequest,
            CapitalAdmissionRejectionReason::CollateralGroupMismatch => {
                Self::CollateralGroupMismatch
            }
            CapitalAdmissionRejectionReason::DuplicateReservation => Self::DuplicateReservation,
            CapitalAdmissionRejectionReason::UnknownReservation => Self::UnknownReservation,
            CapitalAdmissionRejectionReason::UnknownRelease => Self::UnknownRelease,
            CapitalAdmissionRejectionReason::ReconciliationRequired => Self::ReconciliationRequired,
        }
    }

    fn into_fact(self) -> CapitalAdmissionRejectionReason {
        match self {
            Self::MissingEvidence => CapitalAdmissionRejectionReason::MissingEvidence,
            Self::StaleRequest => CapitalAdmissionRejectionReason::StaleRequest,
            Self::PoolMismatch => CapitalAdmissionRejectionReason::PoolMismatch,
            Self::OverBudget => CapitalAdmissionRejectionReason::OverBudget,
            Self::InvalidRequest => CapitalAdmissionRejectionReason::InvalidRequest,
            Self::CollateralGroupMismatch => {
                CapitalAdmissionRejectionReason::CollateralGroupMismatch
            }
            Self::DuplicateReservation => CapitalAdmissionRejectionReason::DuplicateReservation,
            Self::UnknownReservation => CapitalAdmissionRejectionReason::UnknownReservation,
            Self::UnknownRelease => CapitalAdmissionRejectionReason::UnknownRelease,
            Self::ReconciliationRequired => CapitalAdmissionRejectionReason::ReconciliationRequired,
        }
    }
}
