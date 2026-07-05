use backtesting_vertical_slice::{
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE, BackfillAcceptedTrancheIssue,
        BackfillAcceptedTrancheStatus, evaluate_backfill_accepted_tranche,
        write_backfill_accepted_tranche_manifest_from_spec_file,
    },
    backfill_source_proof_scope::{
        BACKFILL_SOURCE_PROOF_SCOPE_SCHEMA_VERSION, BackfillSourceProofScopeObject,
        BackfillSourceProofScopeReport, BackfillSourceProofScopeStatus,
    },
    source_proof::SourceProofUsageScope,
};
use sha2::{Digest, Sha256};

#[test]
fn accepted_tranche_manifest_contains_only_selected_scope_object() {
    let scope = candidate_scope_report();
    let scope_hash = scope_hash(&scope);

    let manifest = evaluate_backfill_accepted_tranche("synthetic-tranche", &scope, &scope_hash)
        .expect("manifest");

    assert_eq!(manifest.status, BackfillAcceptedTrancheStatus::Accepted);
    assert!(manifest.blocking_issues.is_empty());
    assert_eq!(manifest.tranche_id, "synthetic-tranche");
    assert_eq!(manifest.source_proof_scope_report_hash, scope_hash);
    assert_eq!(manifest.source_proof_id, scope.source_proof_id);
    assert_eq!(manifest.source_proof_version, scope.source_proof_version);
    assert_eq!(manifest.source_binding, scope.source_binding);
    assert_eq!(manifest.table_family, scope.table_family);
    assert_eq!(manifest.source_usage_scope, scope.source_usage_scope);
    assert_eq!(manifest.parent_manifest_id, scope.manifest_id);
    assert_eq!(manifest.object_count, 1);
    assert_eq!(manifest.accepted_bytes, 11);
    assert_eq!(manifest.objects.len(), 1);
    assert_eq!(manifest.objects[0].sha256, "selected-object");
}

#[test]
fn accepted_tranche_manifest_accepts_selected_object_from_broader_object_scope() {
    let mut scope = candidate_scope_report();
    scope.accepted_scope_completed_objects = 2;
    scope.accepted_scope_accepted_bytes = 28;
    scope.manifest_payload_object_count = 2;
    scope.object_level_tranche_required = true;
    let selected = scope.selected_object.as_mut().expect("selected object");
    selected.sha256 = "selected-object-from-broader-scope".to_string();
    selected.bytes = 17;
    let scope_hash = scope_hash(&scope);

    let manifest = evaluate_backfill_accepted_tranche("synthetic-tranche", &scope, &scope_hash)
        .expect("manifest");

    assert_eq!(manifest.status, BackfillAcceptedTrancheStatus::Accepted);
    assert!(manifest.blocking_issues.is_empty());
    assert!(manifest.object_level_tranche_required);
    assert_eq!(manifest.object_count, 1);
    assert_eq!(manifest.accepted_bytes, 17);
    assert_eq!(
        manifest.objects[0].sha256,
        "selected-object-from-broader-scope"
    );
}

#[test]
fn accepted_tranche_carries_object_selection_metadata() {
    let mut scope = candidate_scope_report();
    let selected = scope.selected_object.as_mut().expect("selected object");
    selected.source_row_groups = vec![3, 5];
    selected.predicate_ref = Some("source-proof://synthetic/row-groups".to_string());
    let scope_hash = scope_hash(&scope);

    let manifest = evaluate_backfill_accepted_tranche("synthetic-tranche", &scope, &scope_hash)
        .expect("manifest");

    assert_eq!(manifest.status, BackfillAcceptedTrancheStatus::Accepted);
    assert_eq!(manifest.objects[0].source_row_groups, vec![3, 5]);
    assert_eq!(
        manifest.objects[0].predicate_ref.as_deref(),
        Some("source-proof://synthetic/row-groups")
    );
}

#[test]
fn accepted_tranche_blocks_when_scope_has_no_selected_object() {
    let mut scope = candidate_scope_report();
    scope.selected_object = None;
    let scope_hash = scope_hash(&scope);

    let manifest = evaluate_backfill_accepted_tranche("synthetic-tranche", &scope, &scope_hash)
        .expect("manifest");

    assert_eq!(manifest.status, BackfillAcceptedTrancheStatus::Blocked);
    assert!(manifest.objects.is_empty());
    assert!(
        manifest
            .blocking_issues
            .contains(&BackfillAcceptedTrancheIssue::MissingSelectedObject)
    );
}

#[test]
fn accepted_tranche_writer_is_idempotent_and_hash_bound() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let scope_path = dir.path().join("source-proof-scope-report.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("accepted-tranche.toml");
    let scope = candidate_scope_report();
    std::fs::write(
        &scope_path,
        serde_json::to_vec_pretty(&scope).expect("scope json"),
    )
    .expect("write scope");
    std::fs::write(
        &spec_path,
        format!(
            r#"tranche_id = "synthetic-tranche"
source_proof_scope_report_path = "{}"
output_dir = "{}"
"#,
            scope_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first = write_backfill_accepted_tranche_manifest_from_spec_file(&spec_path).expect("first");
    let second =
        write_backfill_accepted_tranche_manifest_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE)
    );
    let manifest: backtesting_vertical_slice::backfill_accepted_tranche::BackfillAcceptedTrancheManifest =
        serde_json::from_slice(&std::fs::read(first.path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(
        manifest.source_proof_scope_report_hash,
        file_hash(&scope_path)
    );
}

fn candidate_scope_report() -> BackfillSourceProofScopeReport {
    BackfillSourceProofScopeReport {
        schema_version: BACKFILL_SOURCE_PROOF_SCOPE_SCHEMA_VERSION.to_string(),
        report_id: "synthetic-source-proof-scope".to_string(),
        status: BackfillSourceProofScopeStatus::CandidateFound,
        source_proof_id: "source-proof-synthetic".to_string(),
        source_proof_version: 1,
        source_binding: "synthetic-native-trades".to_string(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        manifest_id: "synthetic-parent-manifest".to_string(),
        accepted_scope_completed_objects: 1,
        accepted_scope_accepted_bytes: 11,
        manifest_payload_object_count: 2,
        matching_object_count: 1,
        object_level_tranche_required: true,
        selected_object: Some(BackfillSourceProofScopeObject {
            s3_uri: "s3://synthetic-artifacts/raw/v1/object=selected-object.csv.gz".to_string(),
            source_url: "https://example.invalid/synthetic.csv.gz".to_string(),
            sha256: "selected-object".to_string(),
            bytes: 11,
            archive_date: "2026-03-01".to_string(),
            source_row_groups: Vec::new(),
            predicate_ref: None,
        }),
        source_proof_acceptance_error: None,
        blocking_issues: Vec::new(),
    }
}

fn scope_hash(scope: &BackfillSourceProofScopeReport) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(scope).expect("scope json"))
    )
}

fn file_hash(path: &std::path::Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(std::fs::read(path).expect("artifact bytes"))
    )
}
