use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        VenueTruthCaptureFailureFact, VenueTruthDivergenceAlarmClass, VenueTruthDivergenceFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_capture_failure(
    fact: VenueTruthCaptureFailureFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_capture_failure(&fact)?;
    let purpose = KnownPurpose::VenueTruthCaptureFailure;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &CaptureFailureLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            capture_failure: CaptureFailureV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_capture_failure(
    line: &str,
    line_number: usize,
) -> Result<VenueTruthCaptureFailureFact> {
    let decoded: CaptureFailureLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::VenueTruthCaptureFailureV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.capture_failure.into_fact();
    validate_capture_failure(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

pub(super) fn encode_divergence(
    fact: VenueTruthDivergenceFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_divergence(&fact)?;
    let purpose = KnownPurpose::VenueTruthDivergence;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &DivergenceLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            divergence: DivergenceV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_divergence(
    line: &str,
    line_number: usize,
) -> Result<VenueTruthDivergenceFact> {
    let decoded: DivergenceLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::VenueTruthDivergenceV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.divergence.into_fact();
    validate_divergence(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_capture_failure(fact: &VenueTruthCaptureFailureFact) -> Result<(), RecordFailure> {
    if fact.source.trim().is_empty()
        || fact.endpoint.trim().is_empty()
        || fact.error_class.trim().is_empty()
        || fact.observed_at_ns == 0
        || fact.captures_missed == 0
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "venue-truth capture failure contains an empty or invalid field"
        )));
    }
    Ok(())
}

fn validate_divergence(fact: &VenueTruthDivergenceFact) -> Result<(), RecordFailure> {
    if fact.source.trim().is_empty()
        || fact.account_id.trim().is_empty()
        || fact.field.trim().is_empty()
        || fact.venue_value.trim().is_empty()
        || fact.prior_accepted_value.trim().is_empty()
        || fact.missing_explanation.trim().is_empty()
        || fact.observed_at_ns == 0
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "venue-truth divergence contains an empty or invalid field"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureFailureLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    capture_failure: CaptureFailureV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureFailureV1 {
    source: String,
    observed_at_ns: u64,
    endpoint: String,
    error_class: String,
    captures_missed: u64,
}

impl CaptureFailureV1 {
    fn from_fact(fact: VenueTruthCaptureFailureFact) -> Self {
        Self {
            source: fact.source,
            observed_at_ns: fact.observed_at_ns,
            endpoint: fact.endpoint,
            error_class: fact.error_class,
            captures_missed: fact.captures_missed,
        }
    }

    fn into_fact(self) -> VenueTruthCaptureFailureFact {
        VenueTruthCaptureFailureFact {
            source: self.source,
            observed_at_ns: self.observed_at_ns,
            endpoint: self.endpoint,
            error_class: self.error_class,
            captures_missed: self.captures_missed,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DivergenceLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    divergence: DivergenceV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DivergenceV1 {
    source: String,
    observed_at_ns: u64,
    account_id: String,
    field: String,
    venue_value: String,
    prior_accepted_value: String,
    missing_explanation: String,
    alarm_class: AlarmClassV1,
}

impl DivergenceV1 {
    fn from_fact(fact: VenueTruthDivergenceFact) -> Self {
        Self {
            source: fact.source,
            observed_at_ns: fact.observed_at_ns,
            account_id: fact.account_id,
            field: fact.field,
            venue_value: fact.venue_value,
            prior_accepted_value: fact.prior_accepted_value,
            missing_explanation: fact.missing_explanation,
            alarm_class: AlarmClassV1::from_fact(fact.alarm_class),
        }
    }

    fn into_fact(self) -> VenueTruthDivergenceFact {
        VenueTruthDivergenceFact {
            source: self.source,
            observed_at_ns: self.observed_at_ns,
            account_id: self.account_id,
            field: self.field,
            venue_value: self.venue_value,
            prior_accepted_value: self.prior_accepted_value,
            missing_explanation: self.missing_explanation,
            alarm_class: self.alarm_class.into_fact(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AlarmClassV1 {
    TrueDivergence,
    OrderingViolation,
    SilentChannel,
}

impl AlarmClassV1 {
    fn from_fact(value: VenueTruthDivergenceAlarmClass) -> Self {
        match value {
            VenueTruthDivergenceAlarmClass::TrueDivergence => Self::TrueDivergence,
            VenueTruthDivergenceAlarmClass::OrderingViolation => Self::OrderingViolation,
            VenueTruthDivergenceAlarmClass::SilentChannel => Self::SilentChannel,
        }
    }

    fn into_fact(self) -> VenueTruthDivergenceAlarmClass {
        match self {
            Self::TrueDivergence => VenueTruthDivergenceAlarmClass::TrueDivergence,
            Self::OrderingViolation => VenueTruthDivergenceAlarmClass::OrderingViolation,
            Self::SilentChannel => VenueTruthDivergenceAlarmClass::SilentChannel,
        }
    }
}
