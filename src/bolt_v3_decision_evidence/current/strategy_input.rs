use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, project_from_wire, project_to_wire,
    validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3StrategyInputEvidenceSnapshot, BoltV3StrategyInputEvidenceSnapshotWire,
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct BlockedStrategyInputObservationV1Wire(BoltV3StrategyInputEvidenceSnapshotWire);

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct SubmitLinkedStrategyInputV1Wire(BoltV3StrategyInputEvidenceSnapshotWire);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StrategyInputObservationV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    blocked_strategy_input_observation: BlockedStrategyInputObservationV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitLinkedStrategyInputV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    snapshot: SubmitLinkedStrategyInputV1Wire,
}

pub fn encode_blocked_strategy_input_observation(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
) -> Result<EncodedEvidenceRecord> {
    encode_blocked_strategy_input_observation_at(snapshot, positive_recorded_at_utc_ns()?)
}

fn encode_blocked_strategy_input_observation_at(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    validate_blocked_linkage(snapshot)?;
    let purpose = KnownPurpose::BlockedStrategyInputObservation;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "blocked_strategy_input_observation",
        "blocked strategy-input identity has wrong payload member"
    );
    let line = StrategyInputObservationV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        blocked_strategy_input_observation: BlockedStrategyInputObservationV1Wire(project_to_wire(
            snapshot,
            "blocked strategy-input observation",
        )?),
    };
    encode_record(&line, purpose, "blocked strategy-input observation")
}

pub fn encode_submit_linked_strategy_input_snapshot(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
) -> Result<EncodedEvidenceRecord> {
    encode_submit_linked_strategy_input_snapshot_at(snapshot, positive_recorded_at_utc_ns()?)
}

fn encode_submit_linked_strategy_input_snapshot_at(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    validate_submit_linkage(snapshot)?;
    let purpose = KnownPurpose::SubmitLinkedStrategyInputSnapshot;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "snapshot",
        "submit-linked strategy-input identity has wrong payload member"
    );
    let line = SubmitLinkedStrategyInputV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        snapshot: SubmitLinkedStrategyInputV1Wire(project_to_wire(
            snapshot,
            "submit-linked strategy-input snapshot",
        )?),
    };
    encode_record(&line, purpose, "submit-linked strategy-input snapshot")
}

pub(crate) fn decode_blocked_strategy_input_observation(
    line: &[u8],
) -> Result<BoltV3StrategyInputEvidenceSnapshot> {
    let decoded: StrategyInputObservationV1Line = serde_json::from_slice(line)
        .context("failed to decode current blocked strategy-input observation")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        KnownPurpose::BlockedStrategyInputObservation,
        "blocked_strategy_input_observation",
    )?;
    let snapshot = project_from_wire(
        &decoded.blocked_strategy_input_observation.0,
        "blocked strategy-input observation",
    )?;
    validate_blocked_linkage(&snapshot)?;
    Ok(snapshot)
}

pub(crate) fn decode_submit_linked_strategy_input_snapshot(
    line: &[u8],
) -> Result<BoltV3StrategyInputEvidenceSnapshot> {
    let decoded: SubmitLinkedStrategyInputV1Line = serde_json::from_slice(line)
        .context("failed to decode current submit-linked strategy-input snapshot")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        KnownPurpose::SubmitLinkedStrategyInputSnapshot,
        "snapshot",
    )?;
    let snapshot = project_from_wire(&decoded.snapshot.0, "submit-linked strategy-input snapshot")?;
    validate_submit_linkage(&snapshot)?;
    Ok(snapshot)
}

fn validate_blocked_linkage(snapshot: &BoltV3StrategyInputEvidenceSnapshot) -> Result<()> {
    for (field, value) in submission_linkage(snapshot) {
        ensure!(
            value.is_empty(),
            "blocked observation `{field}` must be empty"
        );
    }
    Ok(())
}

fn validate_submit_linkage(snapshot: &BoltV3StrategyInputEvidenceSnapshot) -> Result<()> {
    for (field, value) in submission_linkage(snapshot) {
        ensure!(
            !value.is_empty() && value.trim() == value,
            "submit-linked snapshot `{field}` must be non-empty and canonical"
        );
    }
    Ok(())
}

fn submission_linkage(snapshot: &BoltV3StrategyInputEvidenceSnapshot) -> [(&'static str, &str); 5] {
    [
        (
            "submission_instrument_id",
            &snapshot.submission_instrument_id,
        ),
        ("submission_order_side", &snapshot.submission_order_side),
        ("submission_price", &snapshot.submission_price),
        ("submission_quantity", &snapshot.submission_quantity),
        ("client_order_id", &snapshot.client_order_id),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(fixture: &[u8], payload_member: &str) -> BoltV3StrategyInputEvidenceSnapshot {
        let line: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        serde_json::from_value(line[payload_member].clone()).unwrap()
    }

    #[test]
    fn blocked_and_submit_snapshots_are_byte_exact_and_disjoint() {
        let blocked_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/blocked_strategy_input_observation_v1.jsonl"
        ));
        let submit_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/submit_linked_strategy_input_snapshot_v1.jsonl"
        ));
        let blocked = snapshot(blocked_fixture, "blocked_strategy_input_observation");
        let submit = snapshot(submit_fixture, "snapshot");

        assert_eq!(
            encode_blocked_strategy_input_observation_at(&blocked, 123)
                .unwrap()
                .bytes(),
            blocked_fixture
        );
        assert_eq!(
            encode_submit_linked_strategy_input_snapshot_at(&submit, 123)
                .unwrap()
                .bytes(),
            submit_fixture
        );
        assert!(encode_blocked_strategy_input_observation_at(&submit, 123).is_err());
        assert!(encode_submit_linked_strategy_input_snapshot_at(&blocked, 123).is_err());
    }
}
