mod reservation;
mod settlement;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    facts::{
        RecoveryFact, SettlementBookingErrorFact, SettlementFact, SubmitReservationFillFact,
        SubmitReservationMetadataFact, TerminalSettlementFact,
    },
    generated_contract::{
        ConsumerDisposition, IdentityDescriptor, KnownConsumer, KnownIdentity, KnownPurpose,
        current_identity_for_purpose, descriptor_for_identity, disposition_for, fact_for_identity,
    },
    record::{EncodedEvidenceRecord, RecordFailure},
};

pub(crate) fn encode_reservation_metadata(
    fact: SubmitReservationMetadataFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    reservation::encode_metadata(fact)
}

pub(crate) fn encode_reservation_fill(
    fact: SubmitReservationFillFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    reservation::encode_fill(fact)
}

pub(crate) fn encode_settlement(
    fact: SettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    settlement::encode_settlement(fact)
}

pub(crate) fn encode_settlement_booking_error(
    fact: SettlementBookingErrorFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    settlement::encode_booking_error(fact)
}

pub(crate) fn encode_terminal_settlement(
    fact: TerminalSettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    settlement::encode_terminal(fact)
}

pub(crate) fn decode_startup_recovery_fact(
    identity: KnownIdentity,
    line: &str,
    line_number: usize,
) -> Result<Option<RecoveryFact>> {
    if !is_startup_relevant(identity) {
        return Ok(None);
    }
    let fact = match identity {
        KnownIdentity::SubmitReservationMetadataV1 => {
            reservation::decode_metadata(line, line_number)?
        }
        KnownIdentity::SubmitReservationFillV1 => reservation::decode_fill(line, line_number)?,
        KnownIdentity::SettlementV1 => settlement::decode_settlement(line, line_number)?,
        KnownIdentity::SettlementBookingErrorV1 => {
            settlement::decode_booking_error(line, line_number)?
        }
        KnownIdentity::TerminalSettlementV1 => settlement::decode_terminal(line, line_number)?,
        KnownIdentity::BlockedStrategyInputObservationV1
        | KnownIdentity::SubmitLinkedStrategyInputSnapshotV1
        | KnownIdentity::EntryOrderIntentV1
        | KnownIdentity::RiskReducingExitOrderIntentV1
        | KnownIdentity::AdmittedEntryAdmissionV1
        | KnownIdentity::RejectedEntryAdmissionV1
        | KnownIdentity::RiskReducingExitAdmissionV1
        | KnownIdentity::ForcedReductionAdmissionV1
        | KnownIdentity::BasketAdmissionGrantedV1
        | KnownIdentity::BasketAdmissionRejectedV1
        | KnownIdentity::CapitalAdmissionRebuildV1
        | KnownIdentity::EntrySkipObservationV1
        | KnownIdentity::ExitSubmissionDecisionV1
        | KnownIdentity::ExitHoldDecisionV1
        | KnownIdentity::ExitEvaluationV1
        | KnownIdentity::LossGovernorHaltV1
        | KnownIdentity::OrderRejectV1
        | KnownIdentity::OrderLifecycleV1
        | KnownIdentity::RequoteThrottleObservationV1
        | KnownIdentity::VenueTruthCaptureFailureV1
        | KnownIdentity::VenueTruthDivergenceV1 => {
            unreachable!("startup-irrelevant identity returned relevant disposition")
        }
    };
    Ok(Some(fact))
}

fn is_startup_relevant(identity: KnownIdentity) -> bool {
    let fact = fact_for_identity(identity);
    [
        KnownConsumer::ReservationRecoveryV1,
        KnownConsumer::SettlementRecoveryV1,
        KnownConsumer::BookingRecoveryV1,
    ]
    .into_iter()
    .any(|consumer| {
        matches!(
            disposition_for(fact, consumer),
            ConsumerDisposition::Relevant(_)
        )
    })
}

fn current_line_descriptor(purpose: KnownPurpose) -> IdentityDescriptor {
    descriptor_for_identity(current_identity_for_purpose(purpose))
}

fn current_utc_ns() -> Result<i64, RecordFailure> {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            RecordFailure::Rejected(anyhow::anyhow!(
                "current UTC timestamp is outside the supported positive i64 domain"
            ))
        })
}

fn encode_line(
    purpose: KnownPurpose,
    line: &impl Serialize,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    let mut bytes = serde_json::to_vec(line).map_err(|error| {
        RecordFailure::Rejected(anyhow::anyhow!(
            "current evidence serialization failed: {error}"
        ))
    })?;
    bytes.push(b'\n');
    EncodedEvidenceRecord::try_new(purpose, bytes)
}

fn validate_nonempty<'a>(
    label: &str,
    values: impl IntoIterator<Item = &'a str>,
    observed_at_ns: u64,
) -> Result<(), RecordFailure> {
    if values.into_iter().any(|value| value.trim().is_empty()) || observed_at_ns == 0 {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "{label} fields must be nonempty and observed_at_ns must be positive"
        )));
    }
    Ok(())
}

fn decode<'a, T: Deserialize<'a>>(line: &'a str, line_number: usize) -> Result<T> {
    serde_json::from_str(line).with_context(|| {
        format!("malformed relevant payload at machine evidence line {line_number}")
    })
}

fn validate_envelope(
    identity: KnownIdentity,
    kind: &str,
    schema_version: u32,
    gate_id: &str,
    recorded_at_utc_ns: i64,
    line_number: usize,
) -> Result<()> {
    let descriptor = descriptor_for_identity(identity);
    ensure!(
        kind == descriptor.kind && schema_version == descriptor.schema_version,
        "identity mismatch at machine evidence line {line_number}"
    );
    ensure!(
        gate_id == descriptor.gate_id,
        "wrong gate_id at machine evidence line {line_number}"
    );
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive at machine evidence line {line_number}"
    );
    Ok(())
}
