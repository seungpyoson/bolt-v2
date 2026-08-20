use std::{fs, process::Command};

use backtesting_vertical_slice::{
    source_proof::{
        CONTRACT_VERSION, RESEARCH_SOURCE_POLICY_CANDIDATE_SCHEMA_VERSION,
        ResearchSourcePolicyCandidate, SOURCE_PROOF_SCHEMA_VERSION, SourceBindingRegistry,
        SourceCandidateClass, SourceEvidenceState, SourceProofReport, StagedResearchSourceStatus,
        stage_research_source_registration, validate_source_evidence_transition,
    },
    source_proof_admissibility::{
        SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE, SourceProofAdmissibilityIssue,
        SourceProofAdmissibilityJson, SourceProofAdmissibilityReport,
        SourceProofAdmissibilityStatus, SourceProofAdmissibilityWriteError,
        write_source_proof_admissibility_report,
        write_source_proof_admissibility_report_from_spec_file,
    },
};
use sha2::{Digest, Sha256};

fn legacy_source_proof_json() -> serde_json::Value {
    serde_json::json!({
        "source_proof_id": "source-proof-synthetic-legacy",
        "source_proof_version": 1,
        "contract_version": CONTRACT_VERSION,
        "schema_version": "source-proof-v3.legacy",
        "status": "pending",
        "source_binding_key": "synthetic-native-trades",
        "venue": "synthetic",
        "product_family": "native_trades",
        "table_families": ["trades"],
        "raw_payload_records": [
            {
                "s3_uri": "s3://synthetic-staging/raw/object.csv.gz",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        ],
        "required_checks": {
            "source_access": "pending",
            "license": "pending",
            "schema": "pending"
        }
    })
}

fn current_rejected_source_proof_json() -> serde_json::Value {
    serde_json::json!({
        "source_proof_id": "source-proof-synthetic-rejected",
        "source_proof_version": 1,
        "contract_version": CONTRACT_VERSION,
        "schema_version": SOURCE_PROOF_SCHEMA_VERSION,
        "status": "rejected",
        "source_binding": "synthetic-native-trades",
        "venue": "synthetic",
        "product_family": "native_trades",
        "product_category": "spot",
        "table_family": "trades",
        "evidence_state": "directly_backfillable",
        "source_candidate_class": "official_free",
        "source_selection_status": "REJECTED",
        "usage_scope": "canonical_backfill_input",
        "fixture_type": "mixed",
        "requested_time_range": {
            "start_utc": "2026-01-01T00:00:00Z",
            "end_utc": "2026-01-02T00:00:00Z"
        },
        "coverage_time_range": {
            "start_utc": "2026-01-01T00:00:00Z",
            "end_utc": "2026-01-02T00:00:00Z"
        },
        "instrument_universe_id": "synthetic-universe",
        "raw_sample_uri": "s3://synthetic-staging/raw/object.csv.gz",
        "raw_sample_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "schema_sample_uri": "s3://synthetic-staging/schema/object.json",
        "schema_sample_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "license_ref": "proof://synthetic/license",
        "license_scope": "unknown",
        "retention_ref": "proof://synthetic/retention",
        "cost_ref": "proof://synthetic/cost",
        "nt_mapping_status": "pending",
        "fidelity_class": "TRADE_REPLAY",
        "l2_replay_evidence": {},
        "forbidden_claims": [],
        "claim_limits": [],
        "acceptance_scope": {
            "planned_objects": 1,
            "completed_objects": 1,
            "failed_objects": 0,
            "skipped_objects": 0,
            "accepted_bytes": 100,
            "selector_scope_violations": 0
        },
        "gap_policy_id": "",
        "required_checks": {
            "source_access": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/source-access"
            },
            "license": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/license"
            },
            "schema": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/schema"
            },
            "time_semantics": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/time-semantics"
            },
            "instrument_universe": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/instrument-universe"
            },
            "coverage": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/coverage"
            },
            "retention_freshness": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/retention"
            },
            "granularity": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/granularity"
            },
            "completeness": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/completeness"
            },
            "nt_mapping": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/nt-mapping"
            },
            "cost": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/cost"
            },
            "storage": {
                "outcome": "pending",
                "evidence_ref": "proof://synthetic/storage"
            }
        }
    })
}

#[test]
fn source_proof_admissibility_reports_legacy_shape_without_provider_inference() {
    let report = SourceProofAdmissibilityReport::from_json_values(
        "source-proof-admissibility-synthetic",
        vec![SourceProofAdmissibilityJson {
            proof_uri: "proof://synthetic/legacy-source-proof.json".to_string(),
            proof: legacy_source_proof_json(),
        }],
    )
    .expect("admissibility report builds");

    assert_eq!(report.summary.total_records, 1);
    assert_eq!(report.summary.non_current_contract_records, 1);
    assert_eq!(report.summary.current_contract_records, 0);
    assert_eq!(report.summary.accept_ready_records, 0);

    let record = report.records.first().expect("record");
    assert_eq!(
        record.status,
        SourceProofAdmissibilityStatus::NonCurrentContract
    );
    assert_eq!(
        record.source_proof_id.as_deref(),
        Some("source-proof-synthetic-legacy")
    );
    assert_eq!(record.source_proof_version, Some(1));
    assert_eq!(
        record.source_binding.as_deref(),
        Some("synthetic-native-trades")
    );
    assert!(record.acceptance_error.is_none());
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::MissingCurrentContractField)
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::LegacySourceBindingKeyField)
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::LegacyTableFamiliesField)
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::LegacyRawPayloadRecordsField)
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::LegacyScalarRequiredChecks)
    );
    for field in [
        "source_binding",
        "product_category",
        "table_family",
        "source_candidate_class",
        "source_selection_status",
        "usage_scope",
        "l2_replay_evidence",
        "acceptance_scope",
    ] {
        assert!(
            record
                .missing_current_contract_fields
                .contains(&field.to_string()),
            "legacy proof must report missing current-contract field {field}: {record:?}"
        );
    }
}

#[test]
fn source_proof_admissibility_uses_current_contract_acceptance_logic() {
    let report = SourceProofAdmissibilityReport::from_json_values(
        "source-proof-admissibility-current",
        vec![SourceProofAdmissibilityJson {
            proof_uri: "proof://synthetic/current-source-proof.json".to_string(),
            proof: current_rejected_source_proof_json(),
        }],
    )
    .expect("admissibility report builds");

    assert_eq!(report.summary.total_records, 1);
    assert_eq!(report.summary.current_contract_records, 1);
    assert_eq!(report.summary.current_contract_rejected_records, 1);
    assert_eq!(report.summary.accept_ready_records, 0);

    let record = report.records.first().expect("record");
    assert_eq!(
        record.status,
        SourceProofAdmissibilityStatus::CurrentContractRejected
    );
    assert_eq!(
        record.source_binding.as_deref(),
        Some("synthetic-native-trades")
    );
    assert!(record.missing_current_contract_fields.is_empty());
    assert_eq!(
        record.blocking_issues,
        vec![SourceProofAdmissibilityIssue::AcceptanceFailed]
    );
    assert_eq!(
        record.acceptance_error.as_deref(),
        Some("rejected source proof cannot be accepted")
    );
}

#[test]
fn source_proof_admissibility_reports_missing_l2_replay_evidence_as_contract_gap() {
    let mut proof = current_l2_source_proof_json();
    serde_json::from_value::<SourceProofReport>(proof.clone())
        .expect("complete L2 fixture must deserialize as current contract before field removal");
    proof
        .as_object_mut()
        .expect("proof object")
        .remove("l2_replay_evidence");

    let report = SourceProofAdmissibilityReport::from_json_values(
        "source-proof-admissibility-missing-l2",
        vec![SourceProofAdmissibilityJson {
            proof_uri: "proof://synthetic/missing-l2-source-proof.json".to_string(),
            proof,
        }],
    )
    .expect("admissibility report builds");

    assert_eq!(report.summary.total_records, 1);
    assert_eq!(report.summary.non_current_contract_records, 1);

    let record = report.records.first().expect("record");
    assert_eq!(
        record.status,
        SourceProofAdmissibilityStatus::NonCurrentContract
    );
    assert!(!record.current_contract_deserializes);
    assert!(
        record
            .missing_current_contract_fields
            .contains(&"l2_replay_evidence".to_string()),
        "missing current-contract fields must name the L2 replay evidence/tick-size policy field: {record:?}"
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::MissingCurrentContractField)
    );
    assert!(
        record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::CurrentContractDeserializeFailed)
    );
}

#[test]
fn source_proof_admissibility_reports_defaulted_usage_scope_without_acceptance_failure() {
    let mut proof = accept_ready_l2_source_proof_json();
    let parsed = serde_json::from_value::<SourceProofReport>(proof.clone())
        .expect("complete L2 fixture must deserialize as current contract before field removal");
    parsed
        .evaluate_acceptance_with_registry(&accept_ready_l2_source_binding_registry())
        .expect("complete L2 fixture must be acceptance-ready before field removal");
    proof
        .as_object_mut()
        .expect("proof object")
        .remove("usage_scope");

    let report = SourceProofAdmissibilityReport::from_json_values_with_registry(
        "source-proof-admissibility-missing-usage-scope",
        vec![SourceProofAdmissibilityJson {
            proof_uri: "proof://synthetic/missing-usage-scope-source-proof.json".to_string(),
            proof,
        }],
        &accept_ready_l2_source_binding_registry(),
    )
    .expect("admissibility report builds");

    assert_eq!(report.summary.total_records, 1);
    assert_eq!(report.summary.current_contract_records, 1);
    assert_eq!(report.summary.current_contract_rejected_records, 1);
    assert_eq!(report.summary.accept_ready_records, 0);

    let record = report.records.first().expect("record");
    assert_eq!(
        record.status,
        SourceProofAdmissibilityStatus::CurrentContractRejected
    );
    assert!(record.current_contract_deserializes);
    assert_eq!(
        record.missing_current_contract_fields,
        vec!["usage_scope".to_string()]
    );
    assert_eq!(
        record.blocking_issues,
        vec![SourceProofAdmissibilityIssue::MissingCurrentContractField]
    );
    assert!(
        !record
            .blocking_issues
            .contains(&SourceProofAdmissibilityIssue::AcceptanceFailed)
    );
    assert!(record.acceptance_error.is_none());
}

#[test]
fn source_proof_admissibility_writer_is_idempotent_and_rejects_dirty_artifact() {
    let report = SourceProofAdmissibilityReport::from_json_values(
        "source-proof-admissibility-writer",
        vec![SourceProofAdmissibilityJson {
            proof_uri: "proof://synthetic/legacy-source-proof.json".to_string(),
            proof: legacy_source_proof_json(),
        }],
    )
    .expect("admissibility report builds");
    let dir = tempfile::TempDir::new().expect("temp dir");

    let first = write_source_proof_admissibility_report(dir.path(), &report).expect("write report");
    let second =
        write_source_proof_admissibility_report(dir.path(), &report).expect("same report rerun");

    assert_eq!(first, second);
    assert_eq!(
        first.path,
        dir.path().join(SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE)
    );
    let written = fs::read(&first.path).expect("read report");
    assert_eq!(first.content_hash, hex::encode(Sha256::digest(&written)));
    assert_eq!(
        first.content_hash,
        report.content_hash().expect("report content hash")
    );
    assert_eq!(first.record_count, 1);

    let parsed: SourceProofAdmissibilityReport =
        serde_json::from_slice(&written).expect("parse report");
    assert_eq!(parsed, report);

    let dirty_dir = tempfile::TempDir::new().expect("dirty temp dir");
    let dirty_path = dirty_dir
        .path()
        .join(SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE);
    fs::write(&dirty_path, br#"{"schema_version":"wrong"}"#).expect("seed dirty report");

    let err = write_source_proof_admissibility_report(dirty_dir.path(), &report).unwrap_err();

    assert_eq!(
        err,
        SourceProofAdmissibilityWriteError::ExistingArtifactMismatch {
            path: dirty_path.display().to_string()
        }
    );
}

fn accept_ready_l2_source_binding_registry() -> SourceBindingRegistry {
    SourceBindingRegistry::from_toml_str(
        r#"
[[source_binding]]
key = "synthetic-l2-order-book-deltas"
venue = "polymarket"
product_family = "prediction_market_outcome"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.example/polymarket/{market}/{date}.parquet"
evidence_state = "directly_backfillable"
table_families = ["order_book_snapshot_deltas"]
"#,
    )
    .expect("synthetic source binding registry parses")
}

fn accept_ready_l2_source_proof_json() -> serde_json::Value {
    let mut proof = current_l2_source_proof_json();
    let object = proof.as_object_mut().expect("proof object");
    object.insert(
        "source_binding".to_string(),
        serde_json::json!("synthetic-l2-order-book-deltas"),
    );
    object.insert(
        "evidence_state".to_string(),
        serde_json::json!("directly_backfillable"),
    );
    object.insert(
        "source_selection_status".to_string(),
        serde_json::json!("ACCEPTED_FOR_REQUIRED_FIDELITY"),
    );
    proof
}

fn current_l2_source_proof_json() -> serde_json::Value {
    serde_json::json!({
        "source_proof_id": "source-proof-synthetic-l2",
        "source_proof_version": 1,
        "contract_version": CONTRACT_VERSION,
        "schema_version": SOURCE_PROOF_SCHEMA_VERSION,
        "status": "pending",
        "source_binding": "polymarket-parquet-archive-index",
        "venue": "polymarket",
        "product_family": "prediction_market_outcome",
        "product_category": "binary",
        "table_family": "order_book_snapshot_deltas",
        "evidence_state": "pending_source_proof",
        "source_candidate_class": "official_free",
        "source_selection_status": "PENDING_MORE_PROOF",
        "usage_scope": "canonical_backfill_input",
        "fixture_type": "binary-option",
        "requested_time_range": {
            "start_utc": "2026-01-01T00:00:00Z",
            "end_utc": "2026-01-02T00:00:00Z"
        },
        "coverage_time_range": {
            "start_utc": "2026-01-01T00:00:00Z",
            "end_utc": "2026-01-02T00:00:00Z"
        },
        "instrument_universe_id": "polymarket-parquet-archive-index-universe",
        "raw_sample_uri": "s3://synthetic-artifacts/raw/object.parquet",
        "raw_sample_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "schema_sample_uri": "s3://synthetic-artifacts/schema/object.json",
        "schema_sample_hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "license_ref": "proof://synthetic/license",
        "license_scope": "public",
        "retention_ref": "proof://synthetic/retention",
        "cost_ref": "proof://synthetic/cost",
        "nt_mapping_status": "accepted",
        "fidelity_class": "L2_REPLAY",
        "l2_replay_evidence": {
            "order_book_delta_ref": "proof://synthetic/order-book-delta",
            "no_tick_size_change_universe_ref": "proof://synthetic/no-tick-size-change-universe"
        },
        "forbidden_claims": [],
        "claim_limits": [],
        "acceptance_scope": {
            "planned_objects": 1,
            "completed_objects": 1,
            "failed_objects": 0,
            "skipped_objects": 0,
            "accepted_bytes": 100,
            "selector_scope_violations": 0
        },
        "gap_policy_id": "",
        "required_checks": {
            "source_access": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/source-access"
            },
            "license": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/license"
            },
            "schema": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/schema"
            },
            "time_semantics": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/time-semantics"
            },
            "instrument_universe": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/instrument-universe"
            },
            "coverage": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/coverage"
            },
            "retention_freshness": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/retention"
            },
            "granularity": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/granularity"
            },
            "completeness": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/completeness"
            },
            "nt_mapping": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/nt-mapping"
            },
            "cost": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/cost"
            },
            "storage": {
                "outcome": "passed",
                "evidence_ref": "proof://synthetic/storage"
            }
        }
    })
}

#[test]
fn source_proof_admissibility_reads_toml_spec_and_writes_report() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let proof_path = dir.path().join("source-proof.json");
    fs::write(
        &proof_path,
        serde_json::to_vec(&legacy_source_proof_json()).expect("serialize proof"),
    )
    .expect("write proof");
    let output_dir = dir.path().join("source-proof-admissibility");
    let spec_path = dir.path().join("source-proof-admissibility.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
report_id = "source-proof-admissibility-spec"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[source_proof]]
proof_uri = "proof://synthetic/source-proof.json"
path = "{}"
"#,
            output_dir.display(),
            proof_path.display()
        ),
    )
    .expect("write spec");

    let artifact = write_source_proof_admissibility_report_from_spec_file(&spec_path)
        .expect("write source proof admissibility report from spec");

    assert_eq!(artifact.record_count, 1);
    let report: SourceProofAdmissibilityReport =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read report"))
            .expect("parse report");
    assert_eq!(report.report_id, "source-proof-admissibility-spec");
    assert_eq!(report.summary.non_current_contract_records, 1);
}

fn research_source_candidate_json(candidate_class: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": RESEARCH_SOURCE_POLICY_CANDIDATE_SCHEMA_VERSION,
        "source_candidate_id": "research-source-a",
        "registry_binding": "pump-research-source-register",
        "dataset_product_version": {
            "dataset_id": "dataset-a",
            "product_name": "historical-market-data",
            "version": "2025-01"
        },
        "source_candidate_class": candidate_class,
        "upstream_provenance": {
            "upstream_sources": ["venue-public-archive"],
            "transformations": ["lossless-normalization-v1"]
        },
        "query_and_fields": {
            "query_contract": "archive-object-list-v1",
            "fields": ["event_time", "instrument_id", "price", "quantity"]
        },
        "requested_coverage": {
            "venue_keys": ["venue-a"],
            "instrument_scope": "enumerated-roster-r",
            "start_time": "2025-01-01T00:00:00Z",
            "end_time": "2025-03-01T00:00:00Z",
            "gap_semantics": "missing-object-is-unknown-coverage"
        },
        "rights_packet": {
            "query": {"status": "permitted", "evidence_ref": "rights://query"},
            "download": {"status": "permitted", "evidence_ref": "rights://download"},
            "cache": {"status": "permitted", "evidence_ref": "rights://cache"},
            "post_termination_retention": {"status": "permitted", "evidence_ref": "rights://retention"},
            "derived_data": {"status": "permitted", "evidence_ref": "rights://derived"},
            "collaboration": {"status": "permitted", "evidence_ref": "rights://collaboration"},
            "publication": {"status": "permitted", "evidence_ref": "rights://publication"},
            "attribution": {"status": "required", "evidence_ref": "rights://attribution"},
            "upstream_rights": {"status": "permitted", "evidence_ref": "rights://upstream"}
        },
        "fidelity_requirements": [
            "timestamp_semantics",
            "sequence_continuity",
            "snapshot_reset_semantics",
            "disconnect_semantics",
            "raw_payload_replay",
            "completeness_measurement",
            "correction_tracking",
            "nautilus_mapping"
        ],
        "cost_status": {
            "kind": "zero_cost_verified",
            "evidence_ref": "cost://zero"
        },
        "reviewer": "source-reviewer",
        "decision_time": "2024-12-01T00:00:00Z",
        "expiry_time": "2025-04-01T00:00:00Z",
        "allowed_claims": ["retrospective_episode_detection"],
        "forbidden_claims": ["execution_quality"]
    })
}

fn research_source_candidate(candidate_class: &str) -> ResearchSourcePolicyCandidate {
    serde_json::from_value(research_source_candidate_json(candidate_class))
        .expect("strict research source policy candidate")
}

#[test]
fn free_and_paid_sources_face_the_same_rights_fidelity_and_expiry_gates() {
    let mut errors = Vec::new();
    for candidate_class in ["official_free", "paid_vendor"] {
        let mut value = research_source_candidate_json(candidate_class);
        value["rights_packet"]["download"]["evidence_ref"] = serde_json::json!("");
        let candidate: ResearchSourcePolicyCandidate =
            serde_json::from_value(value).expect("strict source candidate");
        errors.push(
            stage_research_source_registration(&candidate)
                .expect_err("missing rights evidence must fail")
                .to_string(),
        );
    }
    assert_eq!(errors[0], errors[1]);
}

#[test]
fn nominally_free_pilots_require_separate_authorization_and_never_spend() {
    let mut value = research_source_candidate_json("official_free");
    value["cost_status"] = serde_json::json!({
        "kind": "nominally_free_pilot",
        "evidence_ref": "cost://nominally-free",
        "authorization_ref": null
    });
    let candidate: ResearchSourcePolicyCandidate =
        serde_json::from_value(value).expect("source candidate");
    assert!(
        stage_research_source_registration(&candidate)
            .unwrap_err()
            .to_string()
            .contains("authorization")
    );

    let staged = stage_research_source_registration(&research_source_candidate("official_free"))
        .expect("zero-cost source policy");
    assert_eq!(staged.provider_calls(), 0);
    assert_eq!(staged.incremental_provider_spend_usd(), "0");
    assert_eq!(
        staged.status(),
        StagedResearchSourceStatus::PolicyOnlyPendingGenesis
    );
    assert!(!staged.confirmatory_eligible());
}

#[test]
fn pre_genesis_policy_wire_cannot_carry_e0_metadata_or_claim_admission() {
    let mut candidate = research_source_candidate_json("official_free");
    candidate["retained_artifacts"] = serde_json::json!([{
        "artifact_uri": "s3://forbidden/e0.bin",
        "content_hash": "a".repeat(64),
        "representation": "exact_raw"
    }]);
    assert!(serde_json::from_value::<ResearchSourcePolicyCandidate>(candidate).is_err());

    let mut measured_coverage = research_source_candidate_json("official_free");
    measured_coverage["requested_coverage"]["gap_evidence_refs"] =
        serde_json::json!(["evidence://prospective-gap"]);
    assert!(serde_json::from_value::<ResearchSourcePolicyCandidate>(measured_coverage).is_err());

    let mut measured_fidelity = research_source_candidate_json("official_free");
    measured_fidelity["fidelity_requirements"] = serde_json::json!({
        "timestamp_evidence_ref": "fidelity://prospective-timestamp"
    });
    assert!(serde_json::from_value::<ResearchSourcePolicyCandidate>(measured_fidelity).is_err());

    let staged = stage_research_source_registration(&research_source_candidate("official_free"))
        .expect("policy staging");
    assert_eq!(
        staged.status(),
        StagedResearchSourceStatus::PolicyOnlyPendingGenesis
    );
    assert!(!staged.confirmatory_eligible());
}

#[test]
fn source_evidence_lifecycle_is_one_way_and_separate_from_storage_lifecycle() {
    validate_source_evidence_transition(
        SourceEvidenceState::Active,
        SourceEvidenceState::Quarantined,
    )
    .expect("quarantine");
    validate_source_evidence_transition(
        SourceEvidenceState::Quarantined,
        SourceEvidenceState::Revoked,
    )
    .expect("revoke");
    assert!(
        validate_source_evidence_transition(
            SourceEvidenceState::Quarantined,
            SourceEvidenceState::Active
        )
        .is_err()
    );
    assert!(
        validate_source_evidence_transition(
            SourceEvidenceState::Expired,
            SourceEvidenceState::Active
        )
        .is_err()
    );
}

#[test]
fn staged_registration_rejects_e0_bytes_and_metadata_before_genesis() {
    let summary = stage_research_source_registration(&research_source_candidate("official_free"))
        .expect("policy-only staging");
    assert_eq!(summary.provider_calls(), 0);
    assert_eq!(summary.incremental_provider_spend_usd(), "0");
    assert_eq!(summary.source_candidate_id(), "research-source-a");
    assert_eq!(
        summary.source_candidate_class(),
        SourceCandidateClass::OfficialFree
    );
}

#[test]
fn staged_source_hash_canonicalizes_sets_but_preserves_transform_order() {
    let candidate = research_source_candidate("official_free");
    let first = stage_research_source_registration(&candidate).expect("source staging");
    let mut reordered = candidate.clone();
    reordered.query_and_fields.fields.reverse();
    reordered.upstream_provenance.upstream_sources.reverse();
    reordered.allowed_claims.reverse();
    let second = stage_research_source_registration(&reordered).expect("source staging");
    assert_eq!(first.semantic_hash(), second.semantic_hash());
    let mut reordered_transforms = candidate;
    reordered_transforms.upstream_provenance.transformations =
        vec!["second".to_string(), "first".to_string()];
    let second_order =
        stage_research_source_registration(&reordered_transforms).expect("ordered transformations");
    reordered_transforms
        .upstream_provenance
        .transformations
        .reverse();
    let first_order =
        stage_research_source_registration(&reordered_transforms).expect("ordered transformations");
    assert_ne!(first_order.semantic_hash(), second_order.semantic_hash());
}

#[test]
fn register_source_cli_stages_only_policy_evidence_with_zero_provider_activity() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let entry_path = dir.path().join("research-source.toml");
    fs::write(
        &entry_path,
        toml::to_string(&research_source_candidate("official_free")).expect("source TOML"),
    )
    .expect("write source entry");
    let output = Command::new(env!("CARGO_BIN_EXE_pump_research"))
        .args([
            "register-source",
            "--source-entry",
            entry_path.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("register-source command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("source summary");
    assert_eq!(summary["source_candidate_id"], "research-source-a");
    assert_eq!(summary["status"], "policy_only_pending_genesis");
    assert_eq!(summary["provider_calls"], 0);
    assert_eq!(summary["incremental_provider_spend_usd"], "0");
}

#[test]
fn research_source_schema_and_version_chain_fail_closed() {
    let mut unknown = research_source_candidate_json("official_free");
    unknown["provider_api_key"] = serde_json::json!("forbidden");
    assert!(serde_json::from_value::<ResearchSourcePolicyCandidate>(unknown).is_err());

    let mut bad_version = research_source_candidate_json("official_free");
    bad_version["schema_version"] = serde_json::json!("pump-research-source-policy-candidate.v2");
    let bad_version: ResearchSourcePolicyCandidate =
        serde_json::from_value(bad_version).expect("strict source candidate");
    assert!(stage_research_source_registration(&bad_version).is_err());
}
