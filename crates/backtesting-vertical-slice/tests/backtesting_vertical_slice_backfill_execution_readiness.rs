use std::collections::BTreeMap;

use backtesting_vertical_slice::{
    artifact_index::ArtifactKind,
    artifact_index_commit_proof::{
        ARTIFACT_INDEX_COMMIT_PROOF_SCHEMA_VERSION, ArtifactIndexCommitProofReport,
    },
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
        BackfillAcceptedTrancheObject, BackfillAcceptedTrancheStatus,
    },
    backfill_execution_plan::{
        BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION, BackfillExecutionPlan, BackfillExecutionPlanObject,
        BackfillExecutionPlanStatus,
    },
    backfill_execution_readiness::{
        BACKFILL_EXECUTION_READINESS_REPORT_FILE, BackfillExecutionReadinessBlocker,
        BackfillExecutionReadinessInput, BackfillExecutionReadinessStatus,
        BackfillExecutionReadinessSupportedDataPath, evaluate_backfill_execution_readiness,
        write_backfill_execution_readiness_report_from_spec_file,
    },
    source_catalog_mapping_readiness::{
        SOURCE_CATALOG_MAPPING_READINESS_SCHEMA_VERSION, SourceCatalogMappingReadinessReport,
        SourceCatalogMappingReadinessStatus,
    },
    source_proof::{
        FixtureType, SourceProofFidelityClass, SourceProofUsageScope, SourceSelectionStatus,
    },
    source_selection_readiness::{
        SOURCE_SELECTION_READINESS_SCHEMA_VERSION, SourceSelectionReadinessBlocker,
        SourceSelectionReadinessReport, SourceSelectionReadinessStatus,
    },
};

#[test]
fn execution_readiness_is_ready_for_matching_accepted_tranche_and_ready_plan() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Ready);
    assert!(report.blockers.is_empty());
    assert_eq!(report.accepted_tranche_id, tranche.tranche_id);
    assert_eq!(report.execution_plan_id, plan.plan_id);
    assert_eq!(report.source_proof_id, tranche.source_proof_id);
    assert_eq!(report.source_proof_version, tranche.source_proof_version);
    assert_eq!(report.source_binding, tranche.source_binding);
    assert_eq!(report.table_family, tranche.table_family);
    assert_eq!(report.object_count, 1);
    assert_eq!(report.accepted_bytes, tranche.accepted_bytes);
}

#[test]
fn execution_readiness_blocks_when_plan_is_not_bound_to_the_tranche_file_hash() {
    let tranche = accepted_tranche();
    let mut plan = matching_execution_plan(&tranche);
    plan.accepted_tranche_manifest_hash = "different-tranche-file-hash".to_string();

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillExecutionReadinessBlocker::ExecutionPlanAcceptedTrancheHashMismatch)
    );
}

#[test]
fn execution_readiness_writer_reads_toml_spec_and_writes_report_idempotently() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let tranche_path = dir.path().join("accepted-tranche.json");
    let plan_path = dir.path().join("execution-plan.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("readiness.toml");
    let tranche = accepted_tranche();
    let mut plan = matching_execution_plan(&tranche);
    let tranche_bytes = serde_json::to_vec_pretty(&tranche).expect("tranche json");
    let tranche_hash = sha256_bytes(&tranche_bytes);
    plan.accepted_tranche_manifest_hash = tranche_hash;

    std::fs::write(&tranche_path, tranche_bytes).expect("write tranche");
    std::fs::write(
        &plan_path,
        serde_json::to_vec_pretty(&plan).expect("plan json"),
    )
    .expect("write plan");
    std::fs::write(
        &spec_path,
        format!(
            r#"readiness_id = "synthetic-readiness"
accepted_tranche_manifest_path = "{}"
execution_plan_path = "{}"
output_dir = "{}"
required_table_family = "trades"
required_nt_data_type = "TradeTick"
required_source_usage_scope = "canonical_backfill_input"

[[supported_data_paths]]
table_family = "trades"
nt_data_type = "TradeTick"
"#,
            tranche_path.display(),
            plan_path.display(),
            output_dir.display()
        ),
    )
    .expect("write spec");

    let first =
        write_backfill_execution_readiness_report_from_spec_file(&spec_path).expect("first");
    let second =
        write_backfill_execution_readiness_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(BACKFILL_EXECUTION_READINESS_REPORT_FILE)
    );
}

#[test]
fn execution_readiness_blocks_index_backfill_when_producer_iam_scope_is_unproven() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let proof_report = artifact_index_commit_proof_report(false);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: true,
        required_artifact_index_kind: Some(ArtifactKind::Backtests),
        artifact_index_commit_proof_report_hash: Some("synthetic-artifact-index-proof-hash"),
        artifact_index_commit_proof_report: Some(&proof_report),
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert_eq!(
        report.artifact_index_commit_proof_id.as_deref(),
        Some("synthetic-artifact-index-proof")
    );
    assert_eq!(
        report.artifact_index_commit_proof_hash.as_deref(),
        Some("synthetic-artifact-index-proof-hash")
    );
    assert_eq!(
        report.required_artifact_index_kind,
        Some(ArtifactKind::Backtests)
    );
    assert!(
        report
            .blockers
            .contains(&BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven)
    );
}

#[test]
fn execution_readiness_blocks_index_backfill_when_artifact_index_root_mismatches_plan_output() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut proof_report = artifact_index_commit_proof_report(true);
    proof_report.artifact_root = "s3://different-artifacts".to_string();

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: true,
        required_artifact_index_kind: Some(ArtifactKind::Backtests),
        artifact_index_commit_proof_report_hash: Some("synthetic-artifact-index-proof-hash"),
        artifact_index_commit_proof_report: Some(&proof_report),
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(report.blockers.contains(
        &BackfillExecutionReadinessBlocker::ArtifactIndexCommitProofArtifactRootMismatch
    ));
}

#[test]
fn execution_readiness_accepts_artifact_index_proof_sandbox_under_plan_artifact_root() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut proof_report = artifact_index_commit_proof_report(true);
    proof_report.artifact_root =
        "s3://synthetic-artifacts/artifact-index/proofs/synthetic-artifact-index-proof".to_string();

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: true,
        required_artifact_index_kind: Some(ArtifactKind::Backtests),
        artifact_index_commit_proof_report_hash: Some("synthetic-artifact-index-proof-hash"),
        artifact_index_commit_proof_report: Some(&proof_report),
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Ready);
    assert!(!report.blockers.contains(
        &BackfillExecutionReadinessBlocker::ArtifactIndexCommitProofArtifactRootMismatch
    ));
}

#[test]
fn execution_readiness_is_ready_when_required_source_selection_readiness_matches() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let source_selection = source_selection_readiness_report(&tranche);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: true,
        source_selection_readiness_report_hash: Some("synthetic-source-selection-hash"),
        source_selection_readiness_report: Some(&source_selection),
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Ready);
    assert_eq!(
        report.source_selection_readiness_id.as_deref(),
        Some("synthetic-source-selection")
    );
    assert_eq!(
        report.source_selection_readiness_hash.as_deref(),
        Some("synthetic-source-selection-hash")
    );
    assert_eq!(
        report.source_selection_readiness_status,
        Some(SourceSelectionReadinessStatus::Ready)
    );
}

#[test]
fn execution_readiness_blocks_when_source_selection_readiness_proof_boolean_is_false() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut source_selection = source_selection_readiness_report(&tranche);
    source_selection.source_proof_accepted = false;

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: true,
        source_selection_readiness_report_hash: Some("synthetic-source-selection-hash"),
        source_selection_readiness_report: Some(&source_selection),
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillExecutionReadinessBlocker::SourceSelectionReadinessNotProven)
    );
}

#[test]
fn execution_readiness_blocks_when_source_selection_readiness_usage_scope_mismatches() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut source_selection = source_selection_readiness_report(&tranche);
    source_selection.usage_scope = SourceProofUsageScope::OneOffBackfillData;

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: true,
        source_selection_readiness_report_hash: Some("synthetic-source-selection-hash"),
        source_selection_readiness_report: Some(&source_selection),
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(
        report.blockers.contains(
            &BackfillExecutionReadinessBlocker::SourceSelectionReadinessUsageScopeMismatch
        )
    );
}

#[test]
fn execution_readiness_blocks_when_source_selection_readiness_is_required_but_missing() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: true,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: false,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(
        report.blockers.contains(
            &BackfillExecutionReadinessBlocker::SourceSelectionReadinessRequiredButMissing
        )
    );
}

#[test]
fn execution_readiness_is_ready_when_required_source_catalog_mapping_readiness_matches() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let catalog_mapping = source_catalog_mapping_readiness_report(&tranche);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some("synthetic-catalog-mapping-hash"),
        source_catalog_mapping_readiness_report: Some(&catalog_mapping),
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Ready);
    assert_eq!(
        report.source_catalog_mapping_readiness_id.as_deref(),
        Some("synthetic-catalog-mapping")
    );
    assert_eq!(
        report.source_catalog_mapping_readiness_hash.as_deref(),
        Some("synthetic-catalog-mapping-hash")
    );
    assert_eq!(
        report.source_catalog_mapping_readiness_status,
        Some(SourceCatalogMappingReadinessStatus::Ready)
    );
}

#[test]
fn execution_readiness_blocks_when_source_catalog_mapping_readiness_is_required_but_missing() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: None,
        source_catalog_mapping_readiness_report: None,
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(report.blockers.contains(
        &BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessRequiredButMissing
    ));
}

#[test]
fn execution_readiness_blocks_when_source_catalog_mapping_readiness_source_proof_mismatches() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut catalog_mapping = source_catalog_mapping_readiness_report(&tranche);
    catalog_mapping.source_proof_id = "source-proof-different".to_string();
    catalog_mapping.source_proof_version = tranche.source_proof_version + 1;

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some("synthetic-catalog-mapping-hash"),
        source_catalog_mapping_readiness_report: Some(&catalog_mapping),
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(report.blockers.contains(
        &BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessSourceProofMismatch
    ));
}

#[test]
fn execution_readiness_blocks_when_source_catalog_mapping_readiness_usage_scope_mismatches() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut catalog_mapping = source_catalog_mapping_readiness_report(&tranche);
    catalog_mapping.allowed_usage_scopes = vec![SourceProofUsageScope::OneOffBackfillData];
    catalog_mapping.observed_usage_scope = Some(SourceProofUsageScope::OneOffBackfillData);

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some("synthetic-catalog-mapping-hash"),
        source_catalog_mapping_readiness_report: Some(&catalog_mapping),
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(report.blockers.contains(
        &BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessUsageScopeMismatch
    ));
}

#[test]
fn execution_readiness_blocks_when_source_catalog_mapping_readiness_proof_boolean_is_false() {
    let tranche = accepted_tranche();
    let plan = matching_execution_plan(&tranche);
    let mut catalog_mapping = source_catalog_mapping_readiness_report(&tranche);
    catalog_mapping.nt_catalog_mapping_proven = false;

    let report = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "synthetic-readiness",
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash",
        tranche: &tranche,
        execution_plan_hash: "synthetic-plan-file-hash",
        plan: &plan,
        required_table_family: "trades",
        required_nt_data_type: "TradeTick",
        required_source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        supported_data_paths: supported_data_paths(),
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some("synthetic-catalog-mapping-hash"),
        source_catalog_mapping_readiness_report: Some(&catalog_mapping),
    });

    assert_eq!(report.status, BackfillExecutionReadinessStatus::Blocked);
    assert!(
        report
            .blockers
            .contains(&BackfillExecutionReadinessBlocker::SourceCatalogMappingReadinessNotProven)
    );
}

fn accepted_tranche() -> BackfillAcceptedTrancheManifest {
    BackfillAcceptedTrancheManifest {
        schema_version: BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION.to_string(),
        tranche_id: "synthetic-tranche".to_string(),
        status: BackfillAcceptedTrancheStatus::Accepted,
        source_proof_scope_report_id: "synthetic-scope-report".to_string(),
        source_proof_scope_report_hash: "synthetic-scope-report-hash".to_string(),
        source_proof_id: "source-proof-synthetic-native-trades".to_string(),
        source_proof_version: 3,
        source_binding: "synthetic-native-trades".to_string(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        parent_manifest_id: "synthetic-parent-manifest".to_string(),
        object_level_tranche_required: true,
        object_count: 1,
        accepted_bytes: 17,
        objects: vec![accepted_object()],
        blocking_issues: Vec::new(),
    }
}

fn matching_execution_plan(tranche: &BackfillAcceptedTrancheManifest) -> BackfillExecutionPlan {
    let object = tranche.objects[0].clone();
    BackfillExecutionPlan {
        schema_version: BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION.to_string(),
        plan_id: "synthetic-plan".to_string(),
        status: BackfillExecutionPlanStatus::Ready,
        accepted_tranche_id: tranche.tranche_id.clone(),
        accepted_tranche_manifest_hash: "synthetic-tranche-file-hash".to_string(),
        run_spec_hash: "synthetic-run-spec-hash".to_string(),
        operator_run_id: "synthetic-run".to_string(),
        output_prefix: "s3://synthetic-artifacts/backtests/synthetic-run".to_string(),
        source_proof_id: tranche.source_proof_id.clone(),
        source_proof_version: tranche.source_proof_version,
        source_binding: tranche.source_binding.clone(),
        table_family: tranche.table_family.clone(),
        source_usage_scope: tranche.source_usage_scope,
        object_count: tranche.object_count,
        accepted_bytes: tranche.accepted_bytes,
        max_object_bytes: tranche.accepted_bytes,
        max_decoded_bytes: 4096,
        max_source_rows: 128,
        max_projected_row_groups: 1,
        max_wall_seconds: 30,
        require_object_selection_metadata: false,
        objects: vec![BackfillExecutionPlanObject {
            s3_uri: object.s3_uri,
            source_url: object.source_url,
            sha256: object.sha256,
            bytes: object.bytes,
            archive_date: object.archive_date,
            source_row_groups: object.source_row_groups,
            predicate_ref: object.predicate_ref,
        }],
        blocking_issues: Vec::new(),
    }
}

fn accepted_object() -> BackfillAcceptedTrancheObject {
    BackfillAcceptedTrancheObject {
        s3_uri: "s3://synthetic-artifacts/raw/object=synthetic-object-sha.csv.gz".to_string(),
        source_url: "https://data.example.invalid/synthetic-object.csv.gz".to_string(),
        sha256: "synthetic-object-sha".to_string(),
        bytes: 17,
        archive_date: "2026-03-01".to_string(),
        source_row_groups: Vec::new(),
        predicate_ref: None,
    }
}

fn artifact_index_commit_proof_report(
    producer_iam_scope_proven: bool,
) -> ArtifactIndexCommitProofReport {
    ArtifactIndexCommitProofReport {
        schema_version: ARTIFACT_INDEX_COMMIT_PROOF_SCHEMA_VERSION.to_string(),
        proof_id: "synthetic-artifact-index-proof".to_string(),
        artifact_root: "s3://synthetic-artifacts".to_string(),
        artifact_protocol: "s3".to_string(),
        artifact_kind: ArtifactKind::Backtests,
        producer_project: "synthetic-producer".to_string(),
        writer_id: "synthetic-writer".to_string(),
        storage_option_keys: vec!["conditional_put".to_string()],
        event_uris: vec!["s3://synthetic-artifacts/artifact-index/v1/events/kind=backtests/event.json".to_string()],
        snapshot_uris: vec!["s3://synthetic-artifacts/artifact-index/v1/snapshots/kind=backtests/snapshot.json".to_string()],
        latest_pointer_uri: "s3://synthetic-artifacts/artifact-index/v1/pointers/kind=backtests/latest.json".to_string(),
        audit_epoch_uris: vec!["s3://synthetic-artifacts/artifact-index/v1/audit/epochs/synthetic.json".to_string()],
        first_pointer_precondition:
            backtesting_vertical_slice::artifact_index::ArtifactIndexPointerPrecondition::IfNoneMatchAny,
        second_pointer_precondition:
            backtesting_vertical_slice::artifact_index::ArtifactIndexPointerPrecondition::IfMatch {
                etag: "synthetic-etag".to_string(),
            },
        prior_pointer_etag_observed: true,
        final_pointer_etag_observed: true,
        event_create_only_proven: true,
        snapshot_create_only_proven: true,
        audit_epoch_create_only_proven: true,
        latest_pointer_create_only_proven: true,
        latest_pointer_update_if_match_proven: true,
        stale_etag_update_rejected: true,
        latest_pointer_readback_proven: true,
        snapshot_readback_proven: true,
        resolved_snapshot_id: "synthetic-snapshot-b".to_string(),
        final_snapshot_id: "synthetic-snapshot-b".to_string(),
        final_snapshot_content_hash: "synthetic-snapshot-content-hash".to_string(),
        persisted_final_snapshot_json_sha256: "synthetic-snapshot-json-hash".to_string(),
        direct_s3_commit_proven: true,
        producer_iam_scope_proven,
        producer_iam_scope_denied_kinds: vec![ArtifactKind::ResearchAnalytics],
        producer_iam_scope_denied_write_attempts: 3,
        producer_iam_scope_denied_write_rejections: if producer_iam_scope_proven { 3 } else { 0 },
        producer_iam_scope_violation_count: if producer_iam_scope_proven { 0 } else { 3 },
        producer_iam_scope_violation_uris: Vec::new(),
    }
}

fn source_selection_readiness_report(
    tranche: &BackfillAcceptedTrancheManifest,
) -> SourceSelectionReadinessReport {
    SourceSelectionReadinessReport {
        schema_version: SOURCE_SELECTION_READINESS_SCHEMA_VERSION.to_string(),
        selection_id: "synthetic-source-selection".to_string(),
        status: SourceSelectionReadinessStatus::Ready,
        source_proof_id: tranche.source_proof_id.clone(),
        source_proof_version: tranche.source_proof_version,
        source_proof_hash: "synthetic-source-proof-hash".to_string(),
        source_binding: tranche.source_binding.clone(),
        venue: "synthetic-venue".to_string(),
        fixture_type: FixtureType::PerpsSpot,
        table_family: tranche.table_family.clone(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        source_selection_status: SourceSelectionStatus::AcceptedForRequiredFidelity,
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        required_fixture_type: FixtureType::PerpsSpot,
        required_table_family: tranche.table_family.clone(),
        allowed_fidelity_classes: vec![SourceProofFidelityClass::TradeReplay],
        allow_lower_fidelity: false,
        source_proof_accepted: true,
        canonical_usage_scope_proven: true,
        source_access_proven: true,
        license_proven: true,
        sample_schema_proven: true,
        time_semantics_proven: true,
        instrument_universe_proven: true,
        coverage_proven: true,
        retention_freshness_proven: true,
        granularity_proven: true,
        completeness_proven: true,
        nt_mapping_proven: true,
        cost_proven: true,
        storage_proven: true,
        claim_limits_recorded: true,
        source_proof_acceptance_error: None,
        unmet_required_checks: Vec::new(),
        blockers: Vec::<SourceSelectionReadinessBlocker>::new(),
    }
}

fn source_catalog_mapping_readiness_report(
    tranche: &BackfillAcceptedTrancheManifest,
) -> SourceCatalogMappingReadinessReport {
    SourceCatalogMappingReadinessReport {
        schema_version: SOURCE_CATALOG_MAPPING_READINESS_SCHEMA_VERSION.to_string(),
        readiness_id: "synthetic-catalog-mapping".to_string(),
        status: SourceCatalogMappingReadinessStatus::Ready,
        catalog_mapping_evaluation_hash: "synthetic-catalog-mapping-evaluation-hash".to_string(),
        source_proof_id: tranche.source_proof_id.clone(),
        source_proof_version: tranche.source_proof_version,
        source_binding: tranche.source_binding.clone(),
        required_table_family: tranche.table_family.clone(),
        required_nt_data_types: vec!["TradeTick".to_string()],
        required_claim_evidence_refs: Vec::new(),
        allowed_current_bte_statuses: vec!["accepted".to_string()],
        allowed_parquet_catalog_statuses: vec!["proven".to_string()],
        allowed_usage_scopes: vec![SourceProofUsageScope::CanonicalBackfillInput],
        observed_source_proof_id: Some(tranche.source_proof_id.clone()),
        observed_source_proof_version: Some(tranche.source_proof_version),
        observed_source_binding: Some(tranche.source_binding.clone()),
        observed_table_family: Some(tranche.table_family.clone()),
        observed_usage_scope: Some(SourceProofUsageScope::CanonicalBackfillInput),
        observed_nt_data_types: vec!["TradeTick".to_string()],
        observed_nt_data_type_evidence_refs: BTreeMap::from([(
            "TradeTick".to_string(),
            vec!["repo://synthetic/trade-tick-catalog-proof.json".to_string()],
        )]),
        observed_claim_evidence_refs: BTreeMap::new(),
        observed_current_bte_status: Some("accepted".to_string()),
        observed_parquet_catalog_status: Some("proven".to_string()),
        nt_catalog_mapping_proven: true,
        blockers: Vec::new(),
    }
}

fn supported_data_paths() -> Vec<BackfillExecutionReadinessSupportedDataPath> {
    vec![BackfillExecutionReadinessSupportedDataPath {
        table_family: "trades".to_string(),
        nt_data_type: "TradeTick".to_string(),
    }]
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    format!("{:x}", Sha256::digest(bytes))
}
