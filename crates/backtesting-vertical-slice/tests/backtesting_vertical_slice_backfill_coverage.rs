use std::fs;

use backtesting_vertical_slice::{
    backfill_coverage::{
        BACKFILL_COVERAGE_LEDGER_FILE, BackfillCoverageIssue, BackfillCoverageLedger,
        BackfillCoverageLedgerError, BackfillCoverageManifestEvidence, BackfillCoverageParseError,
        BackfillCoverageStatus, BackfillCoverageWriteError, BackfillPhysicalInventory,
        BackfillWriteMode, classify_manifest_coverage, classify_physical_inventory,
        write_coverage_ledger_artifact,
    },
    source_proof::SourceProofStatus,
};

fn completed_manifest() -> BackfillCoverageManifestEvidence {
    BackfillCoverageManifestEvidence {
        manifest_id: "manifest-synthetic-complete".to_string(),
        source_binding: "synthetic-native-trades".to_string(),
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
