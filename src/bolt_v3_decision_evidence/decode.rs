use std::{io::Read, path::Path};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;

use super::{
    current,
    facts::DecodedFact,
    generated_contract::{
        KnownIdentity, KnownSink, facts_for_identity, resolve_identity, sink_for_identity,
    },
    open_regular_decision_evidence_file,
};

#[derive(Deserialize)]
struct EnvelopeIdentity {
    kind: String,
    schema_version: u32,
}

#[cfg(test)]
pub(crate) fn decode_registered_line(line: &[u8]) -> Result<DecodedFact> {
    let envelope: EnvelopeIdentity = serde_json::from_slice(line)
        .context("failed to parse current decision-evidence envelope identity")?;
    let identity = resolve_identity(&envelope.kind, envelope.schema_version)?;
    decode_known_identity(identity, line)
}

pub fn read_registered_facts(path: &Path, max_bytes: u64) -> Result<Vec<DecodedFact>> {
    Ok(read_registered_stream(path, max_bytes)?.facts)
}

pub(crate) struct DecodedStream {
    pub(crate) facts: Vec<DecodedFact>,
    pub(crate) bytes: u64,
}

pub(crate) fn read_registered_stream(path: &Path, max_bytes: u64) -> Result<DecodedStream> {
    let mut file = open_regular_decision_evidence_file(path)
        .context("failed to open current decision-evidence machine stream")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("failed to read current decision-evidence machine stream")?;
    if bytes.len() as u64 > max_bytes {
        return Err(anyhow!(
            "current decision-evidence machine stream exceeds max_bytes={max_bytes}"
        ));
    }
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Err(anyhow!(
            "current decision-evidence machine stream has a torn final record"
        ));
    }

    if bytes.is_empty() {
        return Ok(DecodedStream {
            facts: Vec::new(),
            bytes: 0,
        });
    }

    let facts = bytes[..bytes.len() - 1]
        .split(|byte| *byte == b'\n')
        .enumerate()
        .map(|(index, line)| {
            if line.is_empty() {
                return Err(anyhow!(
                    "current decision-evidence machine stream has a blank record at line index {index}"
                ));
            }
            let envelope: EnvelopeIdentity = serde_json::from_slice(line).with_context(|| {
                format!("failed to parse current decision evidence at line index {index}")
            })?;
            let identity = resolve_identity(&envelope.kind, envelope.schema_version).with_context(
                || format!("failed to resolve current decision evidence at line index {index}"),
            )?;
            ensure!(
                sink_for_identity(identity) == KnownSink::Machine,
                "current machine decision-evidence stream contains observation identity at line index {index}"
            );
            decode_known_identity(identity, line).with_context(|| {
                format!("failed to decode current decision evidence at line index {index}")
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(DecodedStream {
        facts,
        bytes: bytes.len() as u64,
    })
}

fn decode_known_identity(identity: KnownIdentity, line: &[u8]) -> Result<DecodedFact> {
    let fact = match identity {
        KnownIdentity::BlockedStrategyInputObservationV1 => {
            DecodedFact::BlockedStrategyInputObservation(Box::new(
                current::decode_blocked_strategy_input_observation(line)?,
            ))
        }
        KnownIdentity::SubmitLinkedStrategyInputSnapshotV1 => {
            DecodedFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                current::decode_submit_linked_strategy_input_snapshot(line)?,
            ))
        }
        KnownIdentity::EntryOrderIntentV1 => {
            DecodedFact::EntryOrderIntent(current::decode_entry_order_intent(line)?)
        }
        KnownIdentity::RiskReducingExitOrderIntentV1 => DecodedFact::RiskReducingExitOrderIntent(
            current::decode_risk_reducing_exit_order_intent(line)?,
        ),
        KnownIdentity::AdmittedEntryAdmissionV1 => {
            DecodedFact::AdmittedEntryAdmission(current::decode_admitted_entry_admission(line)?)
        }
        KnownIdentity::RejectedEntryAdmissionV1 => {
            DecodedFact::RejectedEntryAdmission(current::decode_rejected_entry_admission(line)?)
        }
        KnownIdentity::RiskReducingExitAdmissionV1 => DecodedFact::RiskReducingExitAdmission(
            current::decode_risk_reducing_exit_admission(line)?,
        ),
        KnownIdentity::ForcedReductionAdmissionV1 => {
            DecodedFact::ForcedReductionAdmission(current::decode_forced_reduction_admission(line)?)
        }
        KnownIdentity::BasketAdmissionGrantedV1 => {
            DecodedFact::BasketAdmissionGranted(current::decode_basket_admission_granted(line)?)
        }
        KnownIdentity::BasketAdmissionRejectedV1 => {
            DecodedFact::BasketAdmissionRejected(current::decode_basket_admission_rejected(line)?)
        }
        KnownIdentity::CapitalAdmissionRebuildV1 => {
            DecodedFact::CapitalAdmissionRebuild(current::decode_capital_admission_rebuild(line)?)
        }
        KnownIdentity::SubmitReservationMetadataV1 => DecodedFact::SubmitReservationMetadata(
            current::decode_submit_reservation_metadata(line)?,
        ),
        KnownIdentity::SubmitReservationFillV1 => {
            DecodedFact::SubmitReservationFill(current::decode_submit_reservation_fill(line)?)
        }
        KnownIdentity::EntrySkipObservationV1 => DecodedFact::EntrySkipObservation(Box::new(
            current::decode_entry_skip_observation(line)?,
        )),
        KnownIdentity::ExitSubmissionDecisionV1 => DecodedFact::ExitSubmissionDecision(Box::new(
            current::decode_exit_submission_decision(line)?,
        )),
        KnownIdentity::ExitHoldDecisionV1 => {
            DecodedFact::ExitHoldDecision(Box::new(current::decode_exit_hold_decision(line)?))
        }
        KnownIdentity::ExitEvaluationV1 => {
            DecodedFact::ExitEvaluation(Box::new(current::decode_exit_evaluation(line)?))
        }
        KnownIdentity::LossGovernorHaltV1 => {
            DecodedFact::LossGovernorHalt(current::decode_loss_governor_halt(line)?)
        }
        KnownIdentity::OrderRejectV1 => {
            DecodedFact::OrderReject(current::decode_order_reject(line)?)
        }
        KnownIdentity::OrderLifecycleV1 => {
            DecodedFact::OrderLifecycle(current::decode_order_lifecycle(line)?)
        }
        KnownIdentity::RequoteThrottleObservationV1 => {
            DecodedFact::RequoteThrottle(current::decode_requote_throttle(line)?)
        }
        KnownIdentity::SettlementV1 => DecodedFact::Settlement(current::decode_settlement(line)?),
        KnownIdentity::SettlementBookingErrorV1 => {
            DecodedFact::SettlementBookingError(current::decode_settlement_booking_error(line)?)
        }
        KnownIdentity::TerminalSettlementV1 => {
            DecodedFact::TerminalSettlement(current::decode_terminal_settlement(line)?)
        }
        KnownIdentity::VenueTruthCaptureFailureV1 => DecodedFact::VenueTruthCaptureFailure(
            current::decode_venue_truth_capture_failure(line)?,
        ),
        KnownIdentity::VenueTruthDivergenceV1 => {
            DecodedFact::VenueTruthDivergence(current::decode_venue_truth_divergence(line)?)
        }
    };
    ensure!(
        facts_for_identity(identity) == [fact.id()],
        "decision-evidence identity decoded to an unregistered fact"
    );
    Ok(fact)
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/",
                $name,
                ".jsonl"
            )) as &[u8]
        };
    }

    const CURRENT_IDENTITY_FIXTURES: &[&[u8]] = &[
        fixture!("blocked_strategy_input_observation_v1"),
        fixture!("submit_linked_strategy_input_snapshot_v1"),
        fixture!("entry_order_intent_v1"),
        fixture!("risk_reducing_exit_order_intent_v1"),
        fixture!("admitted_entry_admission_v1"),
        fixture!("rejected_entry_admission_v1"),
        fixture!("risk_reducing_exit_admission_v1"),
        fixture!("forced_reduction_admission_v1"),
        fixture!("basket_admission_granted_v1"),
        fixture!("basket_admission_rejected_v1"),
        fixture!("capital_admission_rebuild_v1"),
        fixture!("submit_reservation_metadata_v1"),
        fixture!("submit_reservation_fill_v1"),
        fixture!("entry_skip_observation_v1"),
        fixture!("exit_submission_decision_v1"),
        fixture!("exit_hold_decision_v1"),
        fixture!("exit_evaluation_v1"),
        fixture!("loss_governor_halt_present_v1"),
        fixture!("order_reject_v1"),
        fixture!("order_lifecycle_v1"),
        fixture!("requote_throttle_observation_v1"),
        fixture!("settlement_v1"),
        fixture!("settlement_booking_error_v1"),
        fixture!("terminal_settlement_v1"),
        fixture!("venue_truth_capture_failure_v1"),
        fixture!("venue_truth_divergence_v1"),
    ];

    fn encoded(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn every_current_identity_has_strict_positive_and_negative_wire_evidence() {
        const ENVELOPE_FIELDS: [&str; 5] = [
            "schema_version",
            "recorded_at_utc_ns",
            "gate_id",
            "gate_version",
            "kind",
        ];

        for fixture in CURRENT_IDENTITY_FIXTURES {
            decode_registered_line(fixture).expect("positive current fixture must decode");
            let value: serde_json::Value = serde_json::from_slice(fixture).unwrap();
            let payload_member = value
                .as_object()
                .unwrap()
                .keys()
                .find(|field| !ENVELOPE_FIELDS.contains(&field.as_str()))
                .cloned()
                .expect("current fixture must have one payload member");

            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove("gate_id");
            assert!(decode_registered_line(&encoded(&missing)).is_err());

            let mut wrong_type = value.clone();
            wrong_type["schema_version"] = serde_json::json!("1");
            assert!(decode_registered_line(&encoded(&wrong_type)).is_err());

            let mut unknown_envelope_field = value.clone();
            unknown_envelope_field["unregistered_envelope_field"] = serde_json::json!(true);
            assert!(decode_registered_line(&encoded(&unknown_envelope_field)).is_err());

            let mut wrong_gate = value.clone();
            wrong_gate["gate_id"] = serde_json::json!("bolt_v3.unregistered");
            assert!(decode_registered_line(&encoded(&wrong_gate)).is_err());

            let mut old_or_unknown_identity = value.clone();
            old_or_unknown_identity["schema_version"] = serde_json::json!(999);
            assert!(decode_registered_line(&encoded(&old_or_unknown_identity)).is_err());

            let mut wrong_payload_type = value.clone();
            wrong_payload_type[&payload_member] = serde_json::json!("not-an-object");
            assert!(decode_registered_line(&encoded(&wrong_payload_type)).is_err());

            let mut unknown_payload_field = value;
            unknown_payload_field[&payload_member]["unregistered_payload_field"] =
                serde_json::json!(true);
            assert!(decode_registered_line(&encoded(&unknown_payload_field)).is_err());
        }
    }
}
