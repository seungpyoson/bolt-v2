use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, project_from_wire, project_to_wire,
    validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3ExitDecisionEvidence, BoltV3ExitDecisionEvidenceWire, BoltV3ExitDecisionOutcome,
    BoltV3ExitEvaluationEvidence, BoltV3ExitEvaluationEvidenceWire,
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct ExitSubmissionDecisionV1Wire(BoltV3ExitDecisionEvidenceWire);

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct ExitHoldDecisionV1Wire(BoltV3ExitDecisionEvidenceWire);

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct ExitEvaluationV1Wire(BoltV3ExitEvaluationEvidenceWire);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitDecisionV1Line<W> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_decision: W,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitEvaluationV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    exit_evaluation: ExitEvaluationV1Wire,
}

#[derive(Clone, Copy)]
enum ExitDecisionPurpose {
    Submission,
    Hold,
}

impl ExitDecisionPurpose {
    const fn known_purpose(self) -> KnownPurpose {
        match self {
            Self::Submission => KnownPurpose::ExitSubmissionDecision,
            Self::Hold => KnownPurpose::ExitHoldDecision,
        }
    }
}

pub fn encode_exit_submission_decision(
    decision: &BoltV3ExitDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_exit_decision(
        decision,
        ExitDecisionPurpose::Submission,
        positive_recorded_at_utc_ns()?,
    )
}

pub fn encode_exit_hold_decision(
    decision: &BoltV3ExitDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_exit_decision(
        decision,
        ExitDecisionPurpose::Hold,
        positive_recorded_at_utc_ns()?,
    )
}

fn encode_exit_decision(
    decision: &BoltV3ExitDecisionEvidence,
    exit_purpose: ExitDecisionPurpose,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    match exit_purpose {
        ExitDecisionPurpose::Submission => ensure!(
            matches!(
                decision.exit_decision,
                BoltV3ExitDecisionOutcome::Exit | BoltV3ExitDecisionOutcome::ExitFailClosed
            ),
            "exit-submission encoder requires an exit outcome"
        ),
        ExitDecisionPurpose::Hold => ensure!(
            matches!(
                decision.exit_decision,
                BoltV3ExitDecisionOutcome::Hold | BoltV3ExitDecisionOutcome::Blocked
            ),
            "exit-hold encoder requires a hold or blocked outcome"
        ),
    }
    let purpose = exit_purpose.known_purpose();
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "exit_decision",
        "exit-decision identity has wrong payload member"
    );
    let wire = project_to_wire(decision, "exit decision")?;
    match exit_purpose {
        ExitDecisionPurpose::Submission => encode_record(
            &ExitDecisionV1Line {
                schema_version,
                recorded_at_utc_ns,
                gate_id: gate_id.to_string(),
                gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
                kind: kind.to_string(),
                exit_decision: ExitSubmissionDecisionV1Wire(wire),
            },
            purpose,
            "exit submission decision",
        ),
        ExitDecisionPurpose::Hold => encode_record(
            &ExitDecisionV1Line {
                schema_version,
                recorded_at_utc_ns,
                gate_id: gate_id.to_string(),
                gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
                kind: kind.to_string(),
                exit_decision: ExitHoldDecisionV1Wire(wire),
            },
            purpose,
            "exit hold decision",
        ),
    }
}

pub fn encode_exit_evaluation(
    evaluation: &BoltV3ExitEvaluationEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_exit_evaluation_at(evaluation, positive_recorded_at_utc_ns()?)
}

fn encode_exit_evaluation_at(
    evaluation: &BoltV3ExitEvaluationEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    ensure!(
        !evaluation.exit_eval_now_ms.is_negative(),
        "exit_eval_now_ms must be non-negative"
    );
    let purpose = KnownPurpose::ExitEvaluation;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "exit_evaluation",
        "exit-evaluation identity has wrong payload member"
    );
    let line = ExitEvaluationV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        exit_evaluation: ExitEvaluationV1Wire(project_to_wire(evaluation, "exit evaluation")?),
    };
    encode_record(&line, purpose, "exit evaluation")
}

pub(crate) fn decode_exit_submission_decision(line: &[u8]) -> Result<BoltV3ExitDecisionEvidence> {
    decode_exit_decision(line, ExitDecisionPurpose::Submission)
}

pub(crate) fn decode_exit_hold_decision(line: &[u8]) -> Result<BoltV3ExitDecisionEvidence> {
    decode_exit_decision(line, ExitDecisionPurpose::Hold)
}

fn decode_exit_decision(
    line: &[u8],
    exit_purpose: ExitDecisionPurpose,
) -> Result<BoltV3ExitDecisionEvidence> {
    let purpose = exit_purpose.known_purpose();
    let decision: BoltV3ExitDecisionEvidence = match exit_purpose {
        ExitDecisionPurpose::Submission => {
            let decoded: ExitDecisionV1Line<ExitSubmissionDecisionV1Wire> =
                serde_json::from_slice(line)
                    .context("failed to decode current exit-submission decision")?;
            validate_current_header(
                decoded.schema_version,
                decoded.recorded_at_utc_ns,
                &decoded.gate_id,
                &decoded.gate_version,
                &decoded.kind,
                purpose,
                "exit_decision",
            )?;
            project_from_wire(&decoded.exit_decision.0, "exit submission decision")?
        }
        ExitDecisionPurpose::Hold => {
            let decoded: ExitDecisionV1Line<ExitHoldDecisionV1Wire> = serde_json::from_slice(line)
                .context("failed to decode current exit-hold decision")?;
            validate_current_header(
                decoded.schema_version,
                decoded.recorded_at_utc_ns,
                &decoded.gate_id,
                &decoded.gate_version,
                &decoded.kind,
                purpose,
                "exit_decision",
            )?;
            project_from_wire(&decoded.exit_decision.0, "exit hold decision")?
        }
    };
    match exit_purpose {
        ExitDecisionPurpose::Submission => ensure!(
            matches!(
                decision.exit_decision,
                BoltV3ExitDecisionOutcome::Exit | BoltV3ExitDecisionOutcome::ExitFailClosed
            ),
            "exit-submission identity requires an exit outcome"
        ),
        ExitDecisionPurpose::Hold => ensure!(
            matches!(
                decision.exit_decision,
                BoltV3ExitDecisionOutcome::Hold | BoltV3ExitDecisionOutcome::Blocked
            ),
            "exit-hold identity requires a hold or blocked outcome"
        ),
    }
    Ok(decision)
}

pub(crate) fn decode_exit_evaluation(line: &[u8]) -> Result<BoltV3ExitEvaluationEvidence> {
    let decoded: ExitEvaluationV1Line =
        serde_json::from_slice(line).context("failed to decode current exit evaluation")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        KnownPurpose::ExitEvaluation,
        "exit_evaluation",
    )?;
    let evaluation: BoltV3ExitEvaluationEvidence =
        project_from_wire(&decoded.exit_evaluation.0, "exit evaluation")?;
    ensure!(
        !evaluation.exit_eval_now_ms.is_negative(),
        "exit_eval_now_ms must be non-negative"
    );
    Ok(evaluation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_submission_decision_is_byte_exact_and_role_checked() {
        let submission_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/exit_submission_decision_v1.jsonl"
        ));
        let hold_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/exit_hold_decision_v1.jsonl"
        ));
        let evaluation_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/exit_evaluation_v1.jsonl"
        ));
        let submission_line: serde_json::Value =
            serde_json::from_slice(submission_fixture).unwrap();
        let hold_line: serde_json::Value = serde_json::from_slice(hold_fixture).unwrap();
        let evaluation_line: serde_json::Value =
            serde_json::from_slice(evaluation_fixture).unwrap();
        let submission: BoltV3ExitDecisionEvidence =
            serde_json::from_value(submission_line["exit_decision"].clone()).unwrap();
        let hold: BoltV3ExitDecisionEvidence =
            serde_json::from_value(hold_line["exit_decision"].clone()).unwrap();
        let evaluation: BoltV3ExitEvaluationEvidence =
            serde_json::from_value(evaluation_line["exit_evaluation"].clone()).unwrap();

        assert_eq!(
            encode_exit_decision(&submission, ExitDecisionPurpose::Submission, 123)
                .unwrap()
                .bytes(),
            submission_fixture
        );
        assert_eq!(
            encode_exit_decision(&hold, ExitDecisionPurpose::Hold, 123)
                .unwrap()
                .bytes(),
            hold_fixture
        );
        assert_eq!(
            encode_exit_evaluation_at(&evaluation, 123).unwrap().bytes(),
            evaluation_fixture
        );
        assert!(encode_exit_decision(&submission, ExitDecisionPurpose::Hold, 123).is_err());
        assert!(encode_exit_decision(&hold, ExitDecisionPurpose::Submission, 123).is_err());
        assert!(decode_exit_submission_decision(submission_fixture).is_ok());
        assert!(decode_exit_hold_decision(submission_fixture).is_err());
        assert!(decode_exit_hold_decision(hold_fixture).is_ok());
        assert!(decode_exit_evaluation(evaluation_fixture).is_ok());
    }
}
