use std::{fs, path::PathBuf};

use backtesting_vertical_slice::{
    backfill_coverage::{
        BACKFILL_COVERAGE_LEDGER_FILE, BackfillCoverageIssue, BackfillCoverageLedger,
        BackfillCoverageLedgerError, BackfillCoverageManifestEvidence,
        BackfillCoverageManifestFile, BackfillCoverageManifestFileError,
        BackfillCoverageManifestJson, BackfillCoverageParseError, BackfillCoverageStatus,
        BackfillCoverageWriteError, BackfillPhysicalInventory, BackfillWriteMode,
        classify_manifest_coverage, classify_physical_inventory, write_coverage_ledger_artifact,
        write_coverage_ledger_artifact_from_manifest_files,
        write_coverage_ledger_artifact_from_spec_file,
    },
    source_proof::{
        CONTRACT_VERSION, SOURCE_PROOF_SCHEMA_VERSION, SourceProofStatus,
        committed_source_binding_registry,
    },
};

fn completed_manifest() -> BackfillCoverageManifestEvidence {
    BackfillCoverageManifestEvidence {
        manifest_id: "manifest-synthetic-complete".to_string(),
        source_binding: "synthetic-native-trades".to_string(),
        table_family: Some("trades".to_string()),
        source_proof_id: Some("source-proof-synthetic-native-trades".to_string()),
        source_proof_version: Some(1),
        source_proof_status: Some(SourceProofStatus::Accepted),
        write_mode: BackfillWriteMode::S3Staging,
        canonical_s3_write: false,
        planned_objects: 3,
        completed_objects: 3,
        failed_objects: 0,
        skipped_objects: 0,
        completed_bytes: 900,
        selector_scope_violations: 0,
        gap_policy_id: None,
        coverage_axis: Some("synthetic_archive_hour".to_string()),
    }
}

#[test]
fn coverage_ledger_accepts_completed_manifest_without_venue_specific_fields() {
    let record = classify_manifest_coverage(&completed_manifest(), None);

    assert_eq!(record.status, BackfillCoverageStatus::Accepted);
    assert!(!record.canonical_ready);
    assert_eq!(record.accepted_objects, 3);
    assert_eq!(record.accepted_bytes, 900);
    assert!(record.blocking_issues.is_empty(), "{record:?}");
}

#[test]
fn coverage_ledger_rejects_completed_manifest_without_coverage_axis() {
    let mut manifest = completed_manifest();
    manifest.coverage_axis = None;

    let record = classify_manifest_coverage(&manifest, None);

    assert_eq!(record.status, BackfillCoverageStatus::Rejected);
    assert!(
        record
            .blocking_issues
            .contains(&BackfillCoverageIssue::MissingCoverageAxis),
        "{record:?}"
    );
}

#[test]
fn coverage_ledger_preserves_configured_coverage_axis_from_manifest_json() {
    let summary = serde_json::json!({
        "run_id": "manifest-synthetic-axis",
        "source_binding": "synthetic-native-trades",
        "source_proof_id": "source-proof-synthetic-native-trades",
        "source_proof_version": 1,
        "coverage_axis": "synthetic_ingest_time",
        "write_mode": "s3_staging",
        "canonical_s3_write": false,
        "planned_payload_object_count": 4,
        "completed_payload_object_count": 4,
        "completed_payload_bytes": 1_200,
        "errors": [],
        "selector_scope": {
            "payload_selector_scope_violations": []
        }
    });

    let manifest = BackfillCoverageManifestEvidence::from_manifest_json(
        &summary,
        Some(SourceProofStatus::Accepted),
    )
    .expect("manifest with coverage axis parses");
    let record = classify_manifest_coverage(&manifest, None);

    assert_eq!(record.status, BackfillCoverageStatus::Accepted);
    assert_eq!(
        serde_json::to_value(&record).expect("record json")["coverage_axis"],
        "synthetic_ingest_time"
    );
}

#[test]
fn coverage_ledger_binds_source_proof_metadata_from_report_path() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("manifest.json");
    let source_proof_path = dir.path().join("source-proof.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "run_id": "manifest-synthetic-source-proof-bound",
            "coverage_axis": "synthetic_ingest_time",
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "planned_payload_object_count": 2,
            "completed_payload_object_count": 2,
            "completed_payload_bytes": 600,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&synthetic_pending_source_proof(
            "synthetic-archive-index",
            "synthetic_order_book_deltas",
        ))
        .expect("serialize source proof"),
    )
    .expect("write source proof");
    let spec_path = dir.path().join("coverage-ledger.toml");
    let output_dir = dir.path().join("coverage-ledger");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-source-proof-bound"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "manifest://synthetic/source-proof-bound.json"
path = "{}"
source_proof_path = "{}"
"#,
            output_dir.display(),
            manifest_path.display(),
            source_proof_path.display()
        ),
    )
    .expect("write coverage ledger spec");

    let artifact =
        write_coverage_ledger_artifact_from_spec_file(&spec_path).expect("write coverage ledger");
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("parse ledger");
    let record = ledger.records.first().expect("ledger record");

    assert_eq!(
        record.source_binding.as_deref(),
        Some("synthetic-archive-index")
    );
    assert_eq!(
        record.table_family.as_deref(),
        Some("synthetic_order_book_deltas")
    );
    assert_eq!(
        record.source_proof_id.as_deref(),
        Some("source-proof-synthetic-archive-index")
    );
    assert_eq!(record.source_proof_version, Some(1));
    assert_eq!(
        record.blocking_issues,
        vec![BackfillCoverageIssue::SourceProofNotAccepted]
    );
}

#[test]
fn coverage_ledger_rejects_metadata_conflicting_with_source_proof_path() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("manifest.json");
    let source_proof_path = dir.path().join("source-proof.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "run_id": "manifest-synthetic-source-proof-conflict",
            "coverage_axis": "synthetic_ingest_time",
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "planned_payload_object_count": 2,
            "completed_payload_object_count": 2,
            "completed_payload_bytes": 600,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&synthetic_pending_source_proof(
            "synthetic-archive-index",
            "synthetic_order_book_deltas",
        ))
        .expect("serialize source proof"),
    )
    .expect("write source proof");
    let spec_path = dir.path().join("coverage-ledger.toml");
    let output_dir = dir.path().join("coverage-ledger");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-source-proof-conflict"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "manifest://synthetic/source-proof-conflict.json"
path = "{}"
source_proof_path = "{}"
table_family = "synthetic_wrong_family"
"#,
            output_dir.display(),
            manifest_path.display(),
            source_proof_path.display()
        ),
    )
    .expect("write coverage ledger spec");

    let err = write_coverage_ledger_artifact_from_spec_file(&spec_path)
        .expect_err("conflicting metadata is rejected");

    assert!(matches!(
        err,
        BackfillCoverageManifestFileError::SourceProofMetadataMismatch {
            field: "table_family",
            ..
        }
    ));
}

#[test]
fn coverage_ledger_rejects_source_proof_path_with_invalid_accepted_proof() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("manifest.json");
    let source_proof_path = dir.path().join("source-proof.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "run_id": "manifest-synthetic-invalid-accepted-proof",
            "coverage_axis": "synthetic_ingest_time",
            "write_mode": "canonical_s3",
            "canonical_s3_write": true,
            "planned_payload_object_count": 1,
            "completed_payload_object_count": 1,
            "completed_payload_bytes": 500,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    let mut source_proof =
        synthetic_pending_source_proof("synthetic-archive-index", "synthetic_order_book_deltas");
    let source_proof_object = source_proof
        .as_object_mut()
        .expect("synthetic source proof object");
    source_proof_object.insert("status".to_string(), serde_json::json!("accepted"));
    source_proof_object.insert(
        "source_selection_status".to_string(),
        serde_json::json!("ACCEPTED_FOR_REQUIRED_FIDELITY"),
    );
    source_proof_object.insert("acceptance_mode".to_string(), serde_json::json!("manual"));
    source_proof_object.insert("accepted_by".to_string(), serde_json::json!("operator"));
    source_proof_object.insert(
        "accepted_at".to_string(),
        serde_json::json!("2026-06-09T00:00:00Z"),
    );
    source_proof_object.insert(
        "acceptance_scope".to_string(),
        serde_json::json!({
            "planned_objects": 1,
            "completed_objects": 1,
            "failed_objects": 0,
            "skipped_objects": 0,
            "accepted_bytes": 500,
            "selector_scope_violations": 0
        }),
    );
    fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&source_proof).expect("serialize source proof"),
    )
    .expect("write source proof");
    let spec_path = dir.path().join("coverage-ledger.toml");
    let output_dir = dir.path().join("coverage-ledger");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-invalid-accepted-proof"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "manifest://synthetic/invalid-accepted-proof.json"
path = "{}"
source_proof_path = "{}"
"#,
            output_dir.display(),
            manifest_path.display(),
            source_proof_path.display()
        ),
    )
    .expect("write coverage ledger spec");

    let err = write_coverage_ledger_artifact_from_spec_file(&spec_path)
        .expect_err("invalid accepted source proof must not reach coverage output");

    assert!(err.to_string().contains("source proof acceptance"), "{err}");
}

#[test]
fn coverage_ledger_classifies_inventory_without_manifest_as_physical_only() {
    let inventory = BackfillPhysicalInventory {
        inventory_id: "s3-prefix-synthetic-raw".to_string(),
        object_count: 11,
        byte_count: 1_100,
    };

    let record = classify_physical_inventory(&inventory);

    assert_eq!(record.status, BackfillCoverageStatus::PhysicalOnly);
    assert_eq!(record.physical_only_objects, 11);
    assert_eq!(record.physical_only_bytes, 1_100);
    assert_eq!(
        record.blocking_issues,
        vec![BackfillCoverageIssue::MissingManifest]
    );
}

fn synthetic_pending_source_proof(source_binding: &str, table_family: &str) -> serde_json::Value {
    serde_json::json!({
        "source_proof_id": format!("source-proof-{source_binding}"),
        "source_proof_version": 1,
        "contract_version": CONTRACT_VERSION,
        "schema_version": SOURCE_PROOF_SCHEMA_VERSION,
        "status": "pending",
        "source_binding": source_binding,
        "venue": "synthetic-venue",
        "product_family": "synthetic-product",
        "product_category": "synthetic-category",
        "table_family": table_family,
        "evidence_state": "pending_source_proof",
        "source_candidate_class": "official_free",
        "source_selection_status": "PENDING_MORE_PROOF",
        "usage_scope": "one_off_backfill_data",
        "fixture_type": "binary-option",
        "requested_time_range": {
            "start_utc": "2026-03-01T00:00:00Z",
            "end_utc": "2026-03-02T00:00:00Z"
        },
        "coverage_time_range": {
            "start_utc": "2026-03-01T00:00:00Z",
            "end_utc": "2026-03-02T00:00:00Z"
        },
        "instrument_universe_id": "synthetic-instrument-universe",
        "raw_sample_uri": "s3://artifact-root/raw/synthetic/object.parquet",
        "raw_sample_hash": "synthetic-raw-sample-hash",
        "schema_sample_uri": "s3://artifact-root/source-proofs/synthetic/schema.json",
        "schema_sample_hash": "synthetic-schema-sample-hash",
        "license_ref": "s3://artifact-root/source-proofs/synthetic/license.txt",
        "license_scope": "public",
        "retention_ref": "s3://artifact-root/source-proofs/synthetic/retention.json",
        "cost_ref": "s3://artifact-root/source-proofs/synthetic/cost.json",
        "nt_mapping_status": "accepted",
        "fidelity_class": "L2_REPLAY",
        "l2_replay_evidence": {
            "order_book_delta_ref": "s3://artifact-root/source-proofs/synthetic/nt-mapping.json"
        },
        "forbidden_claims": [
            "No broad canonical use until coverage, cost, retention, and storage checks pass."
        ],
        "gap_policy_id": "",
        "required_checks": synthetic_pending_required_checks()
    })
}

fn synthetic_pending_required_checks() -> serde_json::Value {
    let pending = |name: &str| {
        serde_json::json!({
            "outcome": "pending",
            "evidence_ref": format!("pending://synthetic/{name}")
        })
    };
    serde_json::json!({
        "source_access": pending("source_access"),
        "license": pending("license"),
        "schema": pending("schema"),
        "time_semantics": pending("time_semantics"),
        "instrument_universe": pending("instrument_universe"),
        "coverage": pending("coverage"),
        "retention_freshness": pending("retention_freshness"),
        "granularity": pending("granularity"),
        "completeness": pending("completeness"),
        "nt_mapping": pending("nt_mapping"),
        "cost": pending("cost"),
        "storage": pending("storage")
    })
}

#[test]
fn coverage_ledger_rejects_failed_or_unbounded_selector_manifest_before_download() {
    let mut manifest = completed_manifest();
    manifest.failed_objects = 1;
    manifest.selector_scope_violations = 2;

    let record = classify_manifest_coverage(&manifest, None);

    assert_eq!(record.status, BackfillCoverageStatus::Rejected);
    assert_eq!(record.accepted_objects, 0);
    assert!(
        record
            .blocking_issues
            .contains(&BackfillCoverageIssue::FailedObjectsPresent)
    );
    assert!(
        record
            .blocking_issues
            .contains(&BackfillCoverageIssue::SelectorScopeViolationsPresent)
    );
}

#[test]
fn coverage_ledger_requires_gap_policy_for_skipped_objects() {
    let mut missing_policy = completed_manifest();
    missing_policy.planned_objects = 4;
    missing_policy.skipped_objects = 1;

    let rejected = classify_manifest_coverage(&missing_policy, None);

    assert_eq!(rejected.status, BackfillCoverageStatus::Rejected);
    assert_eq!(
        rejected.blocking_issues,
        vec![BackfillCoverageIssue::SkippedObjectsWithoutGapPolicy]
    );

    let mut with_policy = missing_policy;
    with_policy.gap_policy_id = Some("gap-policy-synthetic-hourly".to_string());

    let accepted = classify_manifest_coverage(&with_policy, None);

    assert_eq!(accepted.status, BackfillCoverageStatus::AcceptedWithGaps);
    assert_eq!(accepted.accepted_objects, 3);
    assert_eq!(accepted.skipped_objects, 1);
    assert!(accepted.blocking_issues.is_empty(), "{accepted:?}");
}

#[test]
fn coverage_ledger_reports_unmanifested_inventory_delta_without_rejecting_manifest_scope() {
    let inventory = BackfillPhysicalInventory {
        inventory_id: "s3-prefix-synthetic-raw".to_string(),
        object_count: 5,
        byte_count: 1_500,
    };

    let record = classify_manifest_coverage(&completed_manifest(), Some(&inventory));

    assert_eq!(record.status, BackfillCoverageStatus::Accepted);
    assert_eq!(record.accepted_objects, 3);
    assert_eq!(record.accepted_bytes, 900);
    assert_eq!(record.physical_only_objects, 2);
    assert_eq!(record.physical_only_bytes, 600);
    assert!(record.blocking_issues.is_empty(), "{record:?}");
}

#[test]
fn coverage_manifest_evidence_parses_payload_count_aliases_from_json_summary() {
    let summary = serde_json::json!({
        "run_id": "manifest-synthetic-json",
        "source_binding": "synthetic-native-trades",
        "source_proof_id": "source-proof-synthetic-native-trades",
        "source_proof_version": 1,
        "coverage_axis": "synthetic_archive_hour",
        "write_mode": "s3_staging",
        "canonical_s3_write": false,
        "planned_payload_object_count": 4,
        "completed_payload_object_count": 4,
        "completed_payload_bytes": 1_200,
        "errors": [],
        "selector_scope": {
            "payload_selector_scope_violations": []
        }
    });

    let manifest = BackfillCoverageManifestEvidence::from_manifest_json(
        &summary,
        Some(SourceProofStatus::Accepted),
    )
    .expect("generic payload aliases parse");

    assert_eq!(manifest.manifest_id, "manifest-synthetic-json");
    assert_eq!(manifest.write_mode, BackfillWriteMode::S3Staging);
    assert_eq!(manifest.planned_objects, 4);
    assert_eq!(manifest.completed_objects, 4);
    assert_eq!(manifest.failed_objects, 0);
    assert_eq!(manifest.selector_scope_violations, 0);
    assert_eq!(
        classify_manifest_coverage(&manifest, None).status,
        BackfillCoverageStatus::Accepted
    );
}

#[test]
fn coverage_manifest_evidence_parses_nested_count_aliases_from_json_summary() {
    let summary = serde_json::json!({
        "run_id": "manifest-synthetic-nested",
        "source_binding": "synthetic-native-trades",
        "source_proof_id": "source-proof-synthetic-native-trades",
        "source_proof_version": 1,
        "coverage_axis": "synthetic_archive_hour",
        "write_mode": "s3_staging",
        "canonical_s3_write": false,
        "counts": {
            "payload_object_count": 7,
            "payload_bytes": 1_700,
            "error_count": 0
        },
        "selector_scope": {
            "payload_selector_scope_violations": []
        }
    });

    let manifest = BackfillCoverageManifestEvidence::from_manifest_json(
        &summary,
        Some(SourceProofStatus::Accepted),
    )
    .expect("nested count aliases parse");

    assert_eq!(manifest.planned_objects, 7);
    assert_eq!(manifest.completed_objects, 7);
    assert_eq!(manifest.completed_bytes, 1_700);
    assert_eq!(
        classify_manifest_coverage(&manifest, None).status,
        BackfillCoverageStatus::Accepted
    );
}

#[test]
fn coverage_manifest_evidence_rejects_unknown_write_mode() {
    let summary = serde_json::json!({
        "run_id": "manifest-synthetic-bad-mode",
        "write_mode": "download_then_guess",
        "planned_objects": 1,
        "completed_objects": 1,
        "completed_bytes": 1
    });

    let err = BackfillCoverageManifestEvidence::from_manifest_json(
        &summary,
        Some(SourceProofStatus::Accepted),
    )
    .unwrap_err();

    assert_eq!(
        err,
        BackfillCoverageParseError::UnknownWriteMode("download_then_guess".to_string())
    );
}

#[test]
fn coverage_manifest_evidence_parses_staging_only_and_manifest_exclusion_aliases() {
    let summary = serde_json::json!({
        "run_id": "manifest-synthetic-staging-only",
        "source_binding": "synthetic-native-trades",
        "source_proof_id": "source-proof-synthetic-native-trades",
        "source_proof_version": 1,
        "coverage_axis": "synthetic_archive_hour",
        "write_mode": "s3_staging_only",
        "canonical_s3_write": false,
        "object_count_excluding_manifest": 6,
        "bytes_excluding_manifest": 1_800,
        "errors": []
    });

    let manifest = BackfillCoverageManifestEvidence::from_manifest_json(
        &summary,
        Some(SourceProofStatus::Accepted),
    )
    .expect("manifest-exclusion aliases parse");

    assert_eq!(manifest.write_mode, BackfillWriteMode::S3Staging);
    assert_eq!(manifest.planned_objects, 6);
    assert_eq!(manifest.completed_objects, 6);
    assert_eq!(manifest.completed_bytes, 1_800);
    assert_eq!(
        classify_manifest_coverage(&manifest, None).status,
        BackfillCoverageStatus::Accepted
    );
}

#[test]
fn coverage_ledger_builds_from_manifest_json_batch_without_payload_downloads() {
    let first_summary = serde_json::json!({
        "run_id": "manifest-synthetic-batch-a",
        "source_binding": "synthetic-native-trades",
        "source_proof_id": "source-proof-synthetic-native-trades",
        "source_proof_version": 1,
        "coverage_axis": "synthetic_archive_hour",
        "write_mode": "s3_staging",
        "canonical_s3_write": false,
        "planned_payload_object_count": 2,
        "completed_payload_object_count": 2,
        "completed_payload_bytes": 500,
        "errors": []
    });
    let second_summary = serde_json::json!({
        "run_id": "manifest-synthetic-batch-b",
        "source_binding": "synthetic-native-trades",
        "source_proof_id": "source-proof-synthetic-native-trades",
        "source_proof_version": 1,
        "coverage_axis": "synthetic_archive_hour",
        "write_mode": "s3_staging",
        "canonical_s3_write": false,
        "counts": {
            "payload_object_count": 3,
            "payload_bytes": 700,
            "error_count": 0
        }
    });

    let ledger = BackfillCoverageLedger::from_manifest_json_summaries(
        "ledger-synthetic-batch",
        vec![
            BackfillCoverageManifestJson {
                manifest_uri: "manifest://synthetic/batch-a.json".to_string(),
                summary: first_summary,
                source_proof_status: Some(SourceProofStatus::Accepted),
            },
            BackfillCoverageManifestJson {
                manifest_uri: "manifest://synthetic/batch-b.json".to_string(),
                summary: second_summary,
                source_proof_status: Some(SourceProofStatus::Accepted),
            },
        ],
        vec![],
    )
    .expect("batch ledger builds");

    assert_eq!(ledger.records.len(), 2);
    assert_eq!(ledger.summary.accepted_records, 2);
    assert_eq!(ledger.summary.accepted_objects, 5);
    assert_eq!(ledger.summary.accepted_bytes, 1_200);
    assert_eq!(ledger.summary.blocking_issue_count, 0);
}

#[test]
fn coverage_ledger_records_unsupported_manifest_schema_instead_of_aborting_batch() {
    let unsupported_summary = serde_json::json!({
        "run_id": "manifest-synthetic-batch-bad",
        "completed_objects": 1,
        "completed_bytes": 100
    });

    let ledger = BackfillCoverageLedger::from_manifest_json_summaries(
        "ledger-synthetic-batch",
        vec![BackfillCoverageManifestJson {
            manifest_uri: "manifest://synthetic/batch-bad.json".to_string(),
            summary: unsupported_summary,
            source_proof_status: Some(SourceProofStatus::Accepted),
        }],
        vec![],
    )
    .expect("ledger records unsupported manifest");

    assert_eq!(ledger.records.len(), 1);
    assert_eq!(ledger.summary.rejected_records, 1);
    assert_eq!(ledger.summary.blocking_issue_count, 1);
    let record = ledger.records.first().expect("unsupported record");
    assert_eq!(record.record_id, "manifest://synthetic/batch-bad.json");
    assert_eq!(record.status, BackfillCoverageStatus::Rejected);
    assert_eq!(
        record.blocking_issues,
        vec![BackfillCoverageIssue::UnsupportedManifestSchema]
    );
}

#[test]
fn coverage_ledger_artifact_aggregates_manifest_records_and_physical_only_inventory() {
    let accepted = completed_manifest();
    let mut rejected = completed_manifest();
    rejected.manifest_id = "manifest-synthetic-rejected".to_string();
    rejected.planned_objects = 4;
    rejected.failed_objects = 1;

    let matched_inventory = BackfillPhysicalInventory {
        inventory_id: accepted.manifest_id.clone(),
        object_count: 5,
        byte_count: 1_500,
    };
    let orphan_inventory = BackfillPhysicalInventory {
        inventory_id: "manifest-synthetic-orphan".to_string(),
        object_count: 2,
        byte_count: 200,
    };

    let ledger = BackfillCoverageLedger::from_evidence(
        "ledger-synthetic",
        vec![accepted, rejected],
        vec![matched_inventory, orphan_inventory],
    )
    .expect("ledger aggregate builds from normalized evidence");

    assert_eq!(ledger.ledger_id, "ledger-synthetic");
    assert_eq!(ledger.records.len(), 3);
    assert_eq!(ledger.summary.accepted_records, 1);
    assert_eq!(ledger.summary.rejected_records, 1);
    assert_eq!(ledger.summary.physical_only_records, 1);
    assert_eq!(ledger.summary.accepted_objects, 3);
    assert_eq!(ledger.summary.accepted_bytes, 900);
    assert_eq!(ledger.summary.physical_only_objects, 4);
    assert_eq!(ledger.summary.physical_only_bytes, 800);
    assert_eq!(ledger.summary.blocking_issue_count, 2);

    let hash = ledger.content_hash().expect("ledger content hashes");
    assert_eq!(hash.len(), 64);
    assert!(hash.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let json = serde_json::to_value(&ledger).expect("ledger serializes");
    assert_eq!(json["schema_version"], "backfill-coverage-ledger.v1");
    assert_eq!(
        json["records"]
            .as_array()
            .expect("records array")
            .iter()
            .filter(|record| record["source_proof_id"].is_string())
            .count(),
        2
    );
}

#[test]
fn coverage_ledger_artifact_rejects_duplicate_manifest_ids_before_summary() {
    let manifest = completed_manifest();

    let err = BackfillCoverageLedger::from_evidence(
        "ledger-synthetic",
        vec![manifest.clone(), manifest],
        vec![],
    )
    .unwrap_err();

    assert_eq!(
        err,
        BackfillCoverageLedgerError::DuplicateManifestId("manifest-synthetic-complete".to_string())
    );
}

#[test]
fn coverage_ledger_artifact_rejects_duplicate_inventory_ids_before_summary() {
    let inventory = BackfillPhysicalInventory {
        inventory_id: "manifest-synthetic-physical".to_string(),
        object_count: 1,
        byte_count: 100,
    };

    let err = BackfillCoverageLedger::from_evidence(
        "ledger-synthetic",
        vec![],
        vec![inventory.clone(), inventory],
    )
    .unwrap_err();

    assert_eq!(
        err,
        BackfillCoverageLedgerError::DuplicateInventoryId(
            "manifest-synthetic-physical".to_string()
        )
    );
}

#[test]
fn coverage_ledger_writer_creates_deterministic_local_artifact_and_allows_same_rerun() {
    let ledger = BackfillCoverageLedger::from_evidence(
        "ledger-synthetic",
        vec![completed_manifest()],
        vec![],
    )
    .expect("ledger builds");
    let dir = tempfile::TempDir::new().expect("temp dir");

    let first = write_coverage_ledger_artifact(dir.path(), &ledger).expect("write ledger");
    let second = write_coverage_ledger_artifact(dir.path(), &ledger).expect("same ledger rerun");

    assert_eq!(first, second);
    assert_eq!(first.path, dir.path().join(BACKFILL_COVERAGE_LEDGER_FILE));
    assert_eq!(
        first.content_hash,
        ledger.content_hash().expect("ledger content hash")
    );
    assert!(first.bytes > 0);
    assert_eq!(first.record_count, 1);

    let written = fs::read(&first.path).expect("read written ledger");
    assert_eq!(written.len() as u64, first.bytes);
    let parsed: BackfillCoverageLedger =
        serde_json::from_slice(&written).expect("parse written ledger");
    assert_eq!(parsed, ledger);
}

#[test]
fn coverage_ledger_writer_rejects_existing_mismatched_artifact_without_overwrite() {
    let ledger = BackfillCoverageLedger::from_evidence(
        "ledger-synthetic",
        vec![completed_manifest()],
        vec![],
    )
    .expect("ledger builds");
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join(BACKFILL_COVERAGE_LEDGER_FILE);
    fs::write(&path, br#"{"schema_version":"wrong"}"#).expect("seed dirty artifact");

    let err = write_coverage_ledger_artifact(dir.path(), &ledger).unwrap_err();

    assert_eq!(
        err,
        BackfillCoverageWriteError::ExistingArtifactMismatch {
            path: path.display().to_string()
        }
    );
    assert_eq!(
        fs::read_to_string(&path).expect("dirty artifact remains"),
        r#"{"schema_version":"wrong"}"#
    );
}

#[test]
fn coverage_ledger_writer_reads_manifest_json_files_and_writes_artifact() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_dir = dir.path().join("manifests");
    fs::create_dir_all(&manifest_dir).expect("manifest dir");
    let first_path = manifest_dir.join("first.json");
    let second_path = manifest_dir.join("second.json");
    fs::write(
        &first_path,
        serde_json::to_vec(&serde_json::json!({
            "run_id": "manifest-synthetic-file-a",
            "source_binding": "synthetic-native-trades",
            "source_proof_id": "source-proof-synthetic-native-trades",
            "source_proof_version": 1,
            "coverage_axis": "synthetic_archive_hour",
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "completed_objects": 2,
            "completed_bytes": 600,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    fs::write(
        &second_path,
        serde_json::to_vec(&serde_json::json!({
            "run_id": "manifest-synthetic-file-b",
            "source_binding": "synthetic-native-trades",
            "source_proof_id": "source-proof-synthetic-native-trades",
            "source_proof_version": 1,
            "coverage_axis": "synthetic_archive_hour",
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "counts": {
                "payload_object_count": 4,
                "payload_bytes": 900,
                "error_count": 0
            }
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");

    let output_dir = dir.path().join("coverage-ledger");
    let artifact = write_coverage_ledger_artifact_from_manifest_files(
        &output_dir,
        "ledger-synthetic-files",
        vec![
            manifest_file("manifest://synthetic/file-a.json", first_path),
            manifest_file("manifest://synthetic/file-b.json", second_path),
        ],
        vec![],
        &committed_source_binding_registry(),
    )
    .expect("write coverage ledger from manifest files");

    assert_eq!(artifact.record_count, 2);
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("parse ledger");
    assert_eq!(ledger.summary.accepted_records, 2);
    assert_eq!(ledger.summary.accepted_objects, 6);
    assert_eq!(ledger.summary.accepted_bytes, 1_500);
}

#[test]
fn coverage_ledger_writer_binds_coverage_axis_from_manifest_file_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "run_id": "manifest-synthetic-file-axis",
            "source_binding": "synthetic-native-trades",
            "source_proof_id": "source-proof-synthetic-native-trades",
            "source_proof_version": 1,
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "completed_objects": 2,
            "completed_bytes": 600,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    let spec_path = dir.path().join("coverage-ledger.toml");
    let output_dir = dir.path().join("coverage-ledger");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-file-axis"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "manifest://synthetic/file-axis.json"
path = "{}"
coverage_axis = "timestamp_received"
source_proof_status = "accepted"
"#,
            output_dir.display(),
            manifest_path.display()
        ),
    )
    .expect("write coverage ledger spec");

    let artifact =
        write_coverage_ledger_artifact_from_spec_file(&spec_path).expect("write coverage ledger");
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("parse ledger");

    assert_eq!(ledger.summary.accepted_records, 1);
    assert_eq!(
        ledger.records[0].coverage_axis.as_deref(),
        Some("timestamp_received")
    );
}

#[test]
fn coverage_ledger_accepts_accepted_tranche_manifest_with_spec_owned_write_mode() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("accepted-tranche.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": "backfill-accepted-tranche-manifest.v1",
            "tranche_id": "backfill-accepted-tranche-synthetic-native-trades-2026-05-31",
            "status": "accepted",
            "source_proof_id": "source-proof-synthetic-native-trades-2026-05-31",
            "source_proof_version": 1,
            "source_binding": "synthetic-native-trades",
            "table_family": "trades",
            "object_count": 1,
            "accepted_bytes": 1_809_563,
            "blocking_issues": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    let spec_path = dir.path().join("coverage-ledger.toml");
    let output_dir = dir.path().join("coverage-ledger");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-accepted-tranche"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "repo://synthetic/accepted-tranche.json"
path = "{}"
coverage_axis = "archive_date"
source_proof_status = "accepted"
write_mode = "s3_staging"
canonical_s3_write = false
"#,
            output_dir.display(),
            manifest_path.display()
        ),
    )
    .expect("write coverage ledger spec");

    let artifact =
        write_coverage_ledger_artifact_from_spec_file(&spec_path).expect("write coverage ledger");
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("parse ledger");

    assert_eq!(ledger.summary.accepted_records, 1);
    assert_eq!(ledger.summary.accepted_objects, 1);
    assert_eq!(ledger.summary.accepted_bytes, 1_809_563);
    assert_eq!(
        ledger.records[0].record_id,
        "backfill-accepted-tranche-synthetic-native-trades-2026-05-31"
    );
    assert_eq!(
        ledger.records[0].coverage_axis.as_deref(),
        Some("archive_date")
    );
    assert!(!ledger.records[0].canonical_ready);
}

#[test]
fn coverage_ledger_writer_reports_manifest_uri_for_invalid_json_file() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("bad.json");
    fs::write(&manifest_path, b"{not-json").expect("write invalid manifest");

    let err = write_coverage_ledger_artifact_from_manifest_files(
        &dir.path().join("coverage-ledger"),
        "ledger-synthetic-files",
        vec![manifest_file(
            "manifest://synthetic/bad.json",
            manifest_path.clone(),
        )],
        vec![],
        &committed_source_binding_registry(),
    )
    .unwrap_err();

    match err {
        BackfillCoverageManifestFileError::ParseManifestJson {
            manifest_uri,
            path,
            error,
        } => {
            assert_eq!(manifest_uri, "manifest://synthetic/bad.json");
            assert_eq!(path, manifest_path.display().to_string());
            assert!(error.contains("key must be a string"), "{error}");
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn coverage_ledger_writer_reads_toml_spec_and_writes_artifact() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("summary.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "run_id": "manifest-synthetic-spec-a",
            "source_binding": "synthetic-native-trades",
            "source_proof_id": "source-proof-synthetic-native-trades",
            "source_proof_version": 1,
            "coverage_axis": "synthetic_archive_hour",
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "completed_objects": 8,
            "completed_bytes": 2_400,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    let spec_path = dir.path().join("coverage.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-spec"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "manifest://synthetic/spec-a.json"
path = "{}"
source_proof_status = "accepted"
"#,
            dir.path().join("coverage-ledger").display(),
            manifest_path.display()
        ),
    )
    .expect("write spec");

    let artifact = write_coverage_ledger_artifact_from_spec_file(&spec_path)
        .expect("write coverage ledger from spec file");

    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("parse ledger");
    assert_eq!(ledger.ledger_id, "ledger-synthetic-spec");
    assert_eq!(ledger.summary.accepted_records, 1);
    assert_eq!(ledger.summary.accepted_objects, 8);
    assert_eq!(ledger.summary.accepted_bytes, 2_400);
}

#[test]
fn coverage_ledger_spec_binds_source_proof_metadata_when_manifest_lacks_it() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let manifest_path = dir.path().join("summary.json");
    fs::write(
        &manifest_path,
        serde_json::to_vec(&serde_json::json!({
            "run_id": "manifest-synthetic-bound-proof",
            "coverage_axis": "synthetic_archive_hour",
            "write_mode": "s3_staging",
            "canonical_s3_write": false,
            "completed_objects": 5,
            "completed_bytes": 1_500,
            "errors": []
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
    let spec_path = dir.path().join("coverage.toml");
    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "ledger-synthetic-bound-proof"
output_dir = "{}"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[[manifest]]
manifest_uri = "manifest://synthetic/bound-proof.json"
path = "{}"
source_binding = "synthetic-native-trades"
table_family = "trades"
source_proof_id = "source-proof-synthetic-native-trades"
source_proof_version = 3
source_proof_status = "pending"
"#,
            dir.path().join("coverage-ledger").display(),
            manifest_path.display()
        ),
    )
    .expect("write spec");

    let artifact = write_coverage_ledger_artifact_from_spec_file(&spec_path)
        .expect("write coverage ledger from spec file");

    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("parse ledger");
    assert_eq!(ledger.summary.rejected_records, 1);
    let record = ledger.records.first().expect("record");
    assert_eq!(
        record.source_binding.as_deref(),
        Some("synthetic-native-trades")
    );
    assert_eq!(record.table_family.as_deref(), Some("trades"));
    assert_eq!(
        record.source_proof_id.as_deref(),
        Some("source-proof-synthetic-native-trades")
    );
    assert_eq!(record.source_proof_version, Some(3));
    assert_eq!(
        record.blocking_issues,
        vec![BackfillCoverageIssue::SourceProofNotAccepted]
    );
}

fn manifest_file(manifest_uri: &str, path: PathBuf) -> BackfillCoverageManifestFile {
    BackfillCoverageManifestFile {
        manifest_uri: manifest_uri.to_string(),
        path,
        source_proof_path: None,
        source_binding: None,
        table_family: None,
        coverage_axis: None,
        source_proof_id: None,
        source_proof_version: None,
        source_proof_status: Some(SourceProofStatus::Accepted),
        write_mode: None,
        canonical_s3_write: None,
    }
}
