use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, project_from_wire, project_to_wire,
    validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3EntrySkipEvidence, BoltV3EntrySkipEvidenceWire,
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct EntrySkipObservationV1Wire(BoltV3EntrySkipEvidenceWire);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntrySkipObservationV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    entry_skip: EntrySkipObservationV1Wire,
}

pub fn encode_entry_skip_observation(
    evidence: &BoltV3EntrySkipEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_entry_skip_observation_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_entry_skip_observation_at(
    evidence: &BoltV3EntrySkipEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::EntrySkipObservation;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "entry_skip",
        "entry-skip identity has wrong payload member"
    );
    let line = EntrySkipObservationV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        entry_skip: EntrySkipObservationV1Wire(project_to_wire(evidence, "entry-skip")?),
    };
    encode_record(&line, purpose, "entry-skip observation")
}

pub(crate) fn decode_entry_skip_observation(line: &[u8]) -> Result<BoltV3EntrySkipEvidence> {
    let decoded: EntrySkipObservationV1Line =
        serde_json::from_slice(line).context("failed to decode current entry-skip observation")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        KnownPurpose::EntrySkipObservation,
        "entry_skip",
    )?;
    project_from_wire(&decoded.entry_skip.0, "entry-skip")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_skip_observation_is_byte_exact_and_rejects_wire_drift() {
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/entry_skip_observation_v1.jsonl"
        ));
        let line: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        let evidence: BoltV3EntrySkipEvidence =
            serde_json::from_value(line["entry_skip"].clone()).unwrap();
        assert_eq!(
            encode_entry_skip_observation_at(&evidence, 123)
                .unwrap()
                .bytes(),
            fixture
        );

        let mut drifted = line;
        drifted["entry_skip"]["unregistered_field"] = serde_json::json!(true);
        assert!(decode_entry_skip_observation(&serde_json::to_vec(&drifted).unwrap()).is_err());
    }
}
