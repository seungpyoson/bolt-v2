use backtesting_vertical_slice::{
    backfill_coverage::{
        BackfillCoverageIssue, BackfillCoverageManifestEvidence, BackfillCoverageStatus,
        BackfillPhysicalInventory, BackfillWriteMode, classify_manifest_coverage,
        classify_physical_inventory,
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
