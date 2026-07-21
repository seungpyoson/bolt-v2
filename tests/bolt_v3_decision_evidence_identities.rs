use std::fs;

use anyhow::Result;
use bolt_v2::bolt_v3_decision_evidence::{
    EvidenceConsumer, EvidenceDecodeAction, EvidenceRecordIdentity,
    identity_generator::{parse_registry, render_registry, validate_append_only_compatibility},
    read_submit_reservation_recovery_evidence, resolve_evidence_record_identity,
};

const REGISTRY: &str = include_str!("../config/decision-evidence-identities.toml");
const FROZEN_REGISTRY: &str = include_str!("../config/decision-evidence-identities-frozen.toml");
const GENERATED: &str = include_str!("../src/bolt_v3_decision_evidence/generated_identities.rs");

#[test]
fn identity_registry_is_closed_and_generated_rust_is_byte_exact() -> Result<()> {
    let registry = parse_registry(REGISTRY)?;
    let frozen = parse_registry(FROZEN_REGISTRY)?;
    validate_append_only_compatibility(&frozen, &registry)?;
    assert_eq!(render_registry(&registry)?, GENERATED);

    assert!(parse_registry(&format!("{REGISTRY}\nunknown = true\n")).is_err());
    assert!(
        parse_registry(&REGISTRY.replacen("schema_version = 1", "schema_version = 2", 1)).is_err()
    );
    assert!(
        parse_registry(&REGISTRY.replacen(
            "kind = \"entry_skip_complete_reason\"",
            "kind = \"entry_skip\"",
            1,
        ))
        .is_err()
    );
    assert!(
        parse_registry(&REGISTRY.replacen(
            "consumers = []",
            "consumers = [\"unknown_consumer\"]",
            1,
        ))
        .is_err()
    );

    let changed_gate = parse_registry(&REGISTRY.replacen(
        "gate_id = \"bolt_v3.entry_skip\"",
        "gate_id = \"bolt_v3.changed_entry_skip\"",
        1,
    ))?;
    assert!(validate_append_only_compatibility(&frozen, &changed_gate).is_err());

    let removed_pair = parse_registry(&REGISTRY.replacen(
        "schema_versions = [14, 15]",
        "schema_versions = [15]",
        1,
    ))?;
    assert!(validate_append_only_compatibility(&frozen, &removed_pair).is_err());

    let missing_encoder =
        parse_registry(&REGISTRY.replacen("current_encoder_version = 15\n", "", 1))?;
    assert!(validate_append_only_compatibility(&frozen, &missing_encoder).is_err());

    let historical_encoder = REGISTRY
        .replacen(
            "decode_action = \"entry_skip_v15\"\nconsumers = []",
            "decode_action = \"entry_skip_v15\"\ncurrent_encoder_version = 15\nconsumers = []",
            1,
        )
        .replacen(
            "decode_action = \"entry_skip_complete_reason\"\ncurrent_encoder_version = 15",
            "decode_action = \"entry_skip_complete_reason\"",
            1,
        );
    let historical_encoder = parse_registry(&historical_encoder)?;
    assert!(validate_append_only_compatibility(&frozen, &historical_encoder).is_err());
    Ok(())
}

#[test]
fn identities_are_exact_pairs_without_ordered_version_fallback() {
    assert_eq!(
        resolve_evidence_record_identity("entry_skip", 15).unwrap(),
        EvidenceRecordIdentity::EntrySkipLegacyV15
    );
    assert_eq!(
        resolve_evidence_record_identity("entry_skip_complete_reason", 15).unwrap(),
        EvidenceRecordIdentity::EntrySkipCompleteReasonV15
    );
    assert!(resolve_evidence_record_identity("entry_skip", 16).is_err());
    assert!(resolve_evidence_record_identity("unknown", 15).is_err());

    assert_eq!(
        EvidenceRecordIdentity::current_entry_skip(),
        EvidenceRecordIdentity::EntrySkipCompleteReasonV15
    );
    assert_eq!(
        EvidenceRecordIdentity::current_submit_reservation_metadata(),
        EvidenceRecordIdentity::SubmitReservationMetadataV15
    );
}

#[test]
fn recovery_consumer_membership_is_registered_before_payload_decode() {
    let entry_skip = EvidenceRecordIdentity::EntrySkipCompleteReasonV15;
    assert_eq!(
        entry_skip.decode_action_for(EvidenceConsumer::SubmitReservation),
        None
    );

    let metadata = EvidenceRecordIdentity::SubmitReservationMetadataV13;
    assert_eq!(
        metadata.decode_action_for(EvidenceConsumer::SubmitReservation),
        Some(EvidenceDecodeAction::SubmitReservationMetadata)
    );
    assert_eq!(
        metadata.decode_action_for(EvidenceConsumer::EntryDecisionChain),
        None
    );
}

#[test]
fn reservation_recovery_skips_registered_irrelevant_payload_before_decoding() {
    let fixture = fs::read_to_string(
        "tests/fixtures/bolt_v3/capital_admission_recovery/v13/decision-evidence.jsonl",
    )
    .unwrap();
    let irrelevant = r#"{"schema_version":15,"recorded_at_utc_ns":1,"gate_id":"bolt_v3.entry_skip","gate_version":"historical","kind":"entry_skip","entry_skip":{"not":"a valid entry skip"}}"#;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mixed.jsonl");
    fs::write(&path, format!("{irrelevant}\n{fixture}")).unwrap();

    let recovered = read_submit_reservation_recovery_evidence(&path, 100_000).unwrap();
    assert!(!recovered.metadata_by_client_order_id.is_empty());
}

#[test]
fn reservation_recovery_fails_closed_for_malformed_relevant_identity() {
    let relevant = r#"{"schema_version":15,"recorded_at_utc_ns":1,"gate_id":"bolt_v3.submit_admission","gate_version":"historical","kind":"submit_reservation_metadata","metadata":{"not":"valid"}}"#;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("relevant.jsonl");
    fs::write(&path, format!("{relevant}\n")).unwrap();

    assert!(read_submit_reservation_recovery_evidence(&path, 100_000).is_err());
}
