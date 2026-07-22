use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::facts::{
    VenueTruthCaptureFailureFact, VenueTruthDivergenceAlarm, VenueTruthDivergenceFact,
};
use crate::{
    bolt_v3_decision_evidence::generated_contract::current_identity_for_purpose,
    bolt_v3_venue_truth::{
        VenueTruthCaptureFailureEvidence, VenueTruthDivergenceAlarmClass,
        VenueTruthDivergenceEvidence,
    },
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VenueTruthCaptureFailureV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    venue_truth_capture_failure: VenueTruthCaptureFailureV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VenueTruthDivergenceV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    venue_truth_divergence: VenueTruthDivergenceV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VenueTruthCaptureFailureV1Wire {
    source: String,
    observed_at_ns: u64,
    endpoint: String,
    error_class: String,
    captures_missed: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VenueTruthDivergenceV1Wire {
    source: String,
    observed_at_ns: u64,
    account_id: String,
    field: String,
    venue_value: String,
    prior_accepted_value: String,
    missing_explanation: String,
    alarm_class: VenueTruthDivergenceAlarmV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum VenueTruthDivergenceAlarmV1 {
    TrueDivergence,
    OrderingViolation,
    SilentChannel,
}

pub fn encode_venue_truth_capture_failure(
    evidence: &VenueTruthCaptureFailureEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_venue_truth_capture_failure_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_venue_truth_capture_failure_at(
    evidence: &VenueTruthCaptureFailureEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let purpose = KnownPurpose::VenueTruthCaptureFailure;
    let (kind, schema_version, gate_id) =
        metadata(purpose, recorded_at_utc_ns, "venue_truth_capture_failure")?;
    encode_record(
        &VenueTruthCaptureFailureV1Line {
            schema_version,
            recorded_at_utc_ns,
            gate_id: gate_id.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: kind.to_string(),
            venue_truth_capture_failure: VenueTruthCaptureFailureV1Wire::try_from(evidence)?,
        },
        purpose,
        "venue truth capture failure",
    )
}

pub fn encode_venue_truth_divergence(
    evidence: &VenueTruthDivergenceEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_venue_truth_divergence_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_venue_truth_divergence_at(
    evidence: &VenueTruthDivergenceEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let purpose = KnownPurpose::VenueTruthDivergence;
    let (kind, schema_version, gate_id) =
        metadata(purpose, recorded_at_utc_ns, "venue_truth_divergence")?;
    encode_record(
        &VenueTruthDivergenceV1Line {
            schema_version,
            recorded_at_utc_ns,
            gate_id: gate_id.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: kind.to_string(),
            venue_truth_divergence: VenueTruthDivergenceV1Wire::try_from(evidence)?,
        },
        purpose,
        "venue truth divergence",
    )
}

fn metadata(
    purpose: KnownPurpose,
    recorded_at_utc_ns: i64,
    expected_payload_member: &str,
) -> Result<(&'static str, u32, &'static str)> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == expected_payload_member,
        "venue-truth identity has wrong payload member"
    );
    Ok((kind, schema_version, gate_id))
}

impl TryFrom<&VenueTruthCaptureFailureEvidence> for VenueTruthCaptureFailureV1Wire {
    type Error = anyhow::Error;
    fn try_from(value: &VenueTruthCaptureFailureEvidence) -> Result<Self> {
        Ok(Self {
            source: required_text(&value.source, "source")?,
            observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
            endpoint: required_text(&value.endpoint, "endpoint")?,
            error_class: required_text(&value.error_class, "error_class")?,
            captures_missed: positive(value.captures_missed, "captures_missed")?,
        })
    }
}

impl TryFrom<&VenueTruthDivergenceEvidence> for VenueTruthDivergenceV1Wire {
    type Error = anyhow::Error;
    fn try_from(value: &VenueTruthDivergenceEvidence) -> Result<Self> {
        Ok(Self {
            source: required_text(&value.source, "source")?,
            observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
            account_id: required_text(&value.account_id, "account_id")?,
            field: required_text(&value.field, "field")?,
            venue_value: required_text(&value.venue_value, "venue_value")?,
            prior_accepted_value: required_text(
                &value.prior_accepted_value,
                "prior_accepted_value",
            )?,
            missing_explanation: required_text(&value.missing_explanation, "missing_explanation")?,
            alarm_class: value.alarm_class.into(),
        })
    }
}

impl From<VenueTruthDivergenceAlarmClass> for VenueTruthDivergenceAlarmV1 {
    fn from(value: VenueTruthDivergenceAlarmClass) -> Self {
        match value {
            VenueTruthDivergenceAlarmClass::TrueDivergence => Self::TrueDivergence,
            VenueTruthDivergenceAlarmClass::OrderingViolation => Self::OrderingViolation,
            VenueTruthDivergenceAlarmClass::SilentChannel => Self::SilentChannel,
        }
    }
}

pub(crate) fn decode_venue_truth_capture_failure(
    line: &[u8],
) -> Result<VenueTruthCaptureFailureFact> {
    let line: VenueTruthCaptureFailureV1Line = serde_json::from_slice(line)
        .context("failed to decode current venue truth capture failure")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::VenueTruthCaptureFailure,
        "venue_truth_capture_failure",
    )?;
    let value = line.venue_truth_capture_failure;
    Ok(VenueTruthCaptureFailureFact {
        source: required_text(&value.source, "source")?,
        observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
        endpoint: required_text(&value.endpoint, "endpoint")?,
        error_class: required_text(&value.error_class, "error_class")?,
        captures_missed: positive(value.captures_missed, "captures_missed")?,
    })
}

pub(crate) fn decode_venue_truth_divergence(line: &[u8]) -> Result<VenueTruthDivergenceFact> {
    let line: VenueTruthDivergenceV1Line =
        serde_json::from_slice(line).context("failed to decode current venue truth divergence")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::VenueTruthDivergence,
        "venue_truth_divergence",
    )?;
    let value = line.venue_truth_divergence;
    Ok(VenueTruthDivergenceFact {
        source: required_text(&value.source, "source")?,
        observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
        account_id: required_text(&value.account_id, "account_id")?,
        field: required_text(&value.field, "field")?,
        venue_value: required_text(&value.venue_value, "venue_value")?,
        prior_accepted_value: required_text(&value.prior_accepted_value, "prior_accepted_value")?,
        missing_explanation: required_text(&value.missing_explanation, "missing_explanation")?,
        alarm_class: value.alarm_class.into(),
    })
}

impl From<VenueTruthDivergenceAlarmV1> for VenueTruthDivergenceAlarm {
    fn from(value: VenueTruthDivergenceAlarmV1) -> Self {
        match value {
            VenueTruthDivergenceAlarmV1::TrueDivergence => Self::TrueDivergence,
            VenueTruthDivergenceAlarmV1::OrderingViolation => Self::OrderingViolation,
            VenueTruthDivergenceAlarmV1::SilentChannel => Self::SilentChannel,
        }
    }
}

fn positive(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};

    fn capture_failure() -> VenueTruthCaptureFailureEvidence {
        VenueTruthCaptureFailureEvidence {
            source: "polymarket_venue_truth_rest".into(),
            observed_at_ns: 1_700_000_000_000_000_000,
            endpoint: "clob_balance_allowance".into(),
            error_class: "transport_or_decode".into(),
            captures_missed: 2,
        }
    }
    fn divergence() -> VenueTruthDivergenceEvidence {
        VenueTruthDivergenceEvidence {
            source: "polymarket_venue_truth_rest".into(),
            observed_at_ns: 1_700_000_000_000_000_100,
            account_id: "POLYMARKET-001".into(),
            field: "collateral_balance".into(),
            venue_value: "48.40".into(),
            prior_accepted_value: "50.00".into(),
            missing_explanation: "unexplained_collateral_delta".into(),
            alarm_class: VenueTruthDivergenceAlarmClass::TrueDivergence,
        }
    }

    #[test]
    fn current_venue_truth_codecs_are_byte_exact_and_decodable() {
        let capture = encode_venue_truth_capture_failure_at(&capture_failure(), 14).unwrap();
        let divergence = encode_venue_truth_divergence_at(&divergence(), 15).unwrap();
        let capture_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/venue_truth_capture_failure_v1.jsonl"
        ));
        let divergence_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/venue_truth_divergence_v1.jsonl"
        ));
        assert_eq!(capture.bytes(), capture_fixture);
        assert_eq!(divergence.bytes(), divergence_fixture);
        assert!(matches!(
            decode_registered_line(capture_fixture).unwrap(),
            DecodedFact::VenueTruthCaptureFailure(_)
        ));
        assert!(matches!(
            decode_registered_line(divergence_fixture).unwrap(),
            DecodedFact::VenueTruthDivergence(_)
        ));
    }

    #[test]
    fn current_venue_truth_codecs_reject_invalid_and_unknown_fields() {
        let mut invalid = capture_failure();
        invalid.captures_missed = 0;
        assert!(encode_venue_truth_capture_failure(&invalid).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/venue_truth_divergence_v1.jsonl"
        ));
        let mut value: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        value["venue_truth_divergence"]["unknown"] = serde_json::json!(true);
        assert!(decode_registered_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
