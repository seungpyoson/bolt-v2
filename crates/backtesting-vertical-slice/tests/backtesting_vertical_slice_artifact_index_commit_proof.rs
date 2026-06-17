use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    artifact_index::ArtifactKind,
    artifact_index_commit_proof::{
        ARTIFACT_INDEX_COMMIT_PROOF_REPORT_FILE, ArtifactIndexCommitProofReport,
        ArtifactIndexCommitProofSpec, run_artifact_index_commit_proof_with_object_store,
    },
    hashing::sha256_hex,
    run_manifest::ManifestArtifactStore,
};
use object_store::{memory::InMemory, path::Path as ObjectPath};

#[test]
fn artifact_index_commit_proof_executes_pointer_swap_and_stale_etag_rejection() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let artifact_root = format!("s3://example-bucket/{}", "artifact-index-proof-001-root");
    let output_dir = temp_dir.path().join("output");
    let spec = ArtifactIndexCommitProofSpec {
        proof_id: "artifact-index-proof-001".to_string(),
        artifact_root,
        output_dir: output_dir.clone(),
        artifact_store: ManifestArtifactStore {
            storage_options: BTreeMap::new(),
            rust_storage_options: BTreeMap::new(),
            ssm_parameters: None,
        },
        artifact_kind: ArtifactKind::Backtests,
        producer_project: "backtesting-engine".to_string(),
        writer_id: "backtesting-engine".to_string(),
        research_analytics_subfamily: None,
        denied_artifact_kinds: vec![ArtifactKind::ResearchAnalytics],
    };
    let object_store = InMemory::new();

    let artifact = run_artifact_index_commit_proof_with_object_store(
        &spec,
        Vec::new(),
        std::sync::Arc::new(object_store),
        ObjectPath::from("artifact-index-proof-001-root"),
        false,
    )
    .expect("proof run");

    assert_eq!(
        artifact.report_path,
        output_dir.join(ARTIFACT_INDEX_COMMIT_PROOF_REPORT_FILE)
    );
    assert!(artifact.report_bytes > 0);
    assert_eq!(artifact.content_hash.len(), 64);

    let report_bytes = std::fs::read(&artifact.report_path).expect("report bytes");
    assert_eq!(
        artifact.content_hash,
        sha256_hex(&report_bytes),
        "commit proof hash must bind the exact report bytes written to disk"
    );
    let report: ArtifactIndexCommitProofReport =
        serde_json::from_slice(&report_bytes).expect("report json");
    assert_eq!(report.proof_id, spec.proof_id);
    assert_eq!(report.artifact_kind, ArtifactKind::Backtests);
    assert!(report.event_create_only_proven);
    assert!(report.snapshot_create_only_proven);
    assert!(report.latest_pointer_create_only_proven);
    assert!(report.latest_pointer_update_if_match_proven);
    assert!(report.stale_etag_update_rejected);
    assert!(!report.direct_s3_commit_proven);
    assert!(!report.producer_iam_scope_proven);
    assert_eq!(
        report.producer_iam_scope_denied_kinds,
        vec![ArtifactKind::ResearchAnalytics]
    );
    assert_eq!(report.producer_iam_scope_violation_count, 3);
    assert_eq!(report.resolved_snapshot_id, report.final_snapshot_id);
}
