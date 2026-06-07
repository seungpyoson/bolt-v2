use std::fs;

use backtesting_vertical_slice::{
    source_proof::{CONTRACT_VERSION, SOURCE_PROOF_SCHEMA_VERSION},
    source_proof_admissibility::{
        SOURCE_PROOF_ADMISSIBILITY_REPORT_FILE, SourceProofAdmissibilityIssue,
        SourceProofAdmissibilityJson, SourceProofAdmissibilityReport,
        SourceProofAdmissibilityStatus, SourceProofAdmissibilityWriteError,
        write_source_proof_admissibility_report,
        write_source_proof_admissibility_report_from_spec_file,
    },
};

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
        "retention_ref": "proof://synthetic/retention",
        "nt_mapping_status": "pending",
        "fidelity_class": "TRADE_REPLAY",
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
    assert!(
        record
            .missing_current_contract_fields
            .contains(&"source_binding".to_string())
    );
    assert!(
        record
            .missing_current_contract_fields
            .contains(&"product_category".to_string())
    );
    assert!(
        record
            .missing_current_contract_fields
            .contains(&"table_family".to_string())
    );
    assert!(
        record
            .missing_current_contract_fields
            .contains(&"acceptance_scope".to_string())
    );
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
    assert_eq!(
        first.content_hash,
        report.content_hash().expect("report content hash")
    );
    assert_eq!(first.record_count, 1);

    let parsed: SourceProofAdmissibilityReport =
        serde_json::from_slice(&fs::read(&first.path).expect("read report")).expect("parse report");
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
