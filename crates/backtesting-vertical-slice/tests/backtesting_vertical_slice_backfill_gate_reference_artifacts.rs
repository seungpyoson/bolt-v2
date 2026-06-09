use backtesting_vertical_slice::{
    artifact_index::ArtifactKind,
    artifact_index_commit_proof::ArtifactIndexCommitProofReport,
    backfill_accepted_tranche::{
        BackfillAcceptedTrancheManifest, evaluate_backfill_accepted_tranche,
    },
    backfill_execution_plan::{
        BackfillExecutionPlan, BackfillExecutionRunBinding, BackfillExecutionWorkBudget,
        evaluate_backfill_execution_plan,
    },
    backfill_execution_readiness::{
        BackfillExecutionReadinessBlocker, BackfillExecutionReadinessInput,
        BackfillExecutionReadinessReport, BackfillExecutionReadinessStatus,
        BackfillExecutionReadinessSupportedDataPath, evaluate_backfill_execution_readiness,
    },
    backfill_source_proof_scope::{
        BackfillSourceProofScopeReport, evaluate_backfill_source_proof_scope,
    },
    operator::RunSpec,
    source_catalog_mapping_readiness::{
        SourceCatalogMappingReadinessInput, SourceCatalogMappingReadinessReport,
        SourceCatalogMappingReadinessStatus, SourceCatalogMappingStatusEntry,
        evaluate_source_catalog_mapping_readiness,
    },
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

const SOURCE_PROOF: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-accepted-source-proof.binance-bnbusdc-2026-03-01.json"
);
const RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml"
);
const RUN_SPEC_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml"
);
const OBJECT_STAGING_MANIFEST: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/object-staging/backfill-object-staging-manifest.json"
);
const SOURCE_PROOF_SCOPE_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/source-proof-scope/backfill-source-proof-scope-report.json"
);
const ACCEPTED_TRANCHE_MANIFEST: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/accepted-tranche/backfill-accepted-tranche-manifest.json"
);
const ACCEPTED_TRANCHE_MANIFEST_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/accepted-tranche/backfill-accepted-tranche-manifest.json"
);
const EXECUTION_PLAN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/backfill-execution-plan.toml"
);
const EXECUTION_PLAN: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json"
);
const EXECUTION_PLAN_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-plan/backfill-execution-plan.json"
);
const CATALOG_MAPPING_EVALUATION: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const CATALOG_MAPPING_EVALUATION_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-proof-nt-catalog-mapping-evaluation.backtesting-engine.2026-06-08.json"
);
const SOURCE_CATALOG_MAPPING_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json"
);
const SOURCE_CATALOG_MAPPING_READINESS_REPORT_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/source-catalog-mapping-readiness/source-catalog-mapping-readiness-report.json"
);
const BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/source-catalog-mapping-readiness/polymarket-parquet-archive-index-canonical/source-catalog-mapping-readiness-report.json"
);
const EXECUTION_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-readiness/backfill-execution-readiness-report.json"
);
const ARTIFACT_INDEX_COMMIT_STATUS: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof-status.backtesting-engine-006.2026-06-08.json"
);
const ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-direct-s3.2026-06-08.json"
);
const ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-direct-s3.2026-06-08.json"
);
const ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-iam-scope.2026-06-08.json"
);
const ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES: &[u8] = include_bytes!(
    "../../../specs/023-nt-research-analytics-platform/reference/artifact-index-commit-proof/artifact-index-commit-proof-report.backtesting-engine-006-iam-scope.2026-06-08.json"
);
const ARTIFACT_INDEX_REQUIRED_EXECUTION_READINESS_REPORT: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01/execution-readiness-artifact-index-required/backfill-execution-readiness-report.json"
);
const BINANCE_GATE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/023-nt-research-analytics-platform/reference/backfill-gates/binance-bnbusdc-2026-03-01"
);

#[test]
fn binance_backfill_gate_reference_artifacts_match_generic_evaluators() {
    let expected_scope: BackfillSourceProofScopeReport =
        serde_json::from_str(SOURCE_PROOF_SCOPE_REPORT).expect("scope report parses");
    let actual_scope = evaluate_backfill_source_proof_scope(
        expected_scope.report_id.clone(),
        SOURCE_PROOF,
        OBJECT_STAGING_MANIFEST,
    )
    .expect("source-proof scope evaluates");

    assert_eq!(actual_scope, expected_scope);

    let expected_tranche: BackfillAcceptedTrancheManifest =
        serde_json::from_str(ACCEPTED_TRANCHE_MANIFEST).expect("accepted tranche parses");
    let actual_tranche = evaluate_backfill_accepted_tranche(
        expected_tranche.tranche_id.clone(),
        &actual_scope,
        &source_proof_scope_hash(&actual_scope),
    )
    .expect("accepted tranche evaluates");

    assert_eq!(actual_tranche, expected_tranche);

    let expected_plan: BackfillExecutionPlan =
        serde_json::from_str(EXECUTION_PLAN).expect("execution plan parses");
    let run_spec: RunSpec = toml::from_str(RUN_SPEC).expect("run spec parses");
    let actual_plan = evaluate_backfill_execution_plan(
        expected_plan.plan_id.clone(),
        sha256_hex(ACCEPTED_TRANCHE_MANIFEST_BYTES),
        &actual_tranche,
        sha256_hex(RUN_SPEC_BYTES),
        &BackfillExecutionRunBinding::from_run_spec(&run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: expected_plan.max_source_rows,
            max_projected_row_groups: expected_plan.max_projected_row_groups,
            max_wall_seconds: expected_plan.max_wall_seconds,
        },
    );

    assert_eq!(actual_plan, expected_plan);
    assert!(actual_plan.blocking_issues.is_empty());

    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_str(CATALOG_MAPPING_EVALUATION).expect("mapping evaluation parses");
    let manifest_exposure = &mapping_evaluation
        .nt_surface_evidence
        .current_bte_manifest_exposure;
    assert!(
        manifest_exposure
            .accepted_data_classes
            .iter()
            .any(|data_class| data_class == "TradeTick")
    );
    assert!(
        manifest_exposure
            .accepted_data_classes
            .iter()
            .any(|data_class| data_class == "OrderBookDelta")
    );
    assert!(
        !manifest_exposure
            .rejected_for_now
            .iter()
            .any(|data_class| data_class == "OrderBookDelta")
    );
    assert!(
        manifest_exposure
            .rejected_for_now
            .iter()
            .any(|data_class| data_class == "QuoteTick")
    );
    let expected_catalog_mapping_readiness: SourceCatalogMappingReadinessReport =
        serde_json::from_str(SOURCE_CATALOG_MAPPING_READINESS_REPORT)
            .expect("source catalog-mapping readiness parses");
    let actual_catalog_mapping_readiness =
        evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
            readiness_id: &expected_catalog_mapping_readiness.readiness_id,
            catalog_mapping_evaluation_hash: &sha256_hex(CATALOG_MAPPING_EVALUATION_BYTES),
            source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
            source_proof_id: &expected_catalog_mapping_readiness.source_proof_id,
            source_proof_version: expected_catalog_mapping_readiness.source_proof_version,
            source_binding: &expected_catalog_mapping_readiness.source_binding,
            required_table_family: &expected_catalog_mapping_readiness.required_table_family,
            required_nt_data_types: expected_catalog_mapping_readiness
                .required_nt_data_types
                .clone(),
            allowed_current_bte_statuses: expected_catalog_mapping_readiness
                .allowed_current_bte_statuses
                .clone(),
            allowed_parquet_catalog_statuses: expected_catalog_mapping_readiness
                .allowed_parquet_catalog_statuses
                .clone(),
        });

    assert_eq!(
        actual_catalog_mapping_readiness,
        expected_catalog_mapping_readiness
    );

    let expected_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(EXECUTION_READINESS_REPORT).expect("execution readiness parses");
    let accepted_tranche_manifest_hash = sha256_hex(ACCEPTED_TRANCHE_MANIFEST_BYTES);
    let execution_plan_hash = sha256_hex(EXECUTION_PLAN_BYTES);
    let source_catalog_mapping_readiness_hash =
        sha256_hex(SOURCE_CATALOG_MAPPING_READINESS_REPORT_BYTES);
    let readiness = evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
        readiness_id: "binance-bnbusdc-2026-03-01-reference-readiness",
        accepted_tranche_manifest_hash: &accepted_tranche_manifest_hash,
        tranche: &actual_tranche,
        execution_plan_hash: &execution_plan_hash,
        plan: &actual_plan,
        required_table_family: &actual_plan.table_family,
        required_nt_data_type: "TradeTick",
        supported_data_paths: vec![BackfillExecutionReadinessSupportedDataPath {
            table_family: actual_plan.table_family.clone(),
            nt_data_type: "TradeTick".to_string(),
        }],
        artifact_index_commit_required: false,
        required_artifact_index_kind: None,
        artifact_index_commit_proof_report_hash: None,
        artifact_index_commit_proof_report: None,
        source_selection_readiness_required: false,
        source_selection_readiness_report_hash: None,
        source_selection_readiness_report: None,
        source_catalog_mapping_readiness_required: true,
        source_catalog_mapping_readiness_report_hash: Some(&source_catalog_mapping_readiness_hash),
        source_catalog_mapping_readiness_report: Some(&actual_catalog_mapping_readiness),
    });

    assert_eq!(readiness, expected_readiness);
    assert_eq!(readiness.status, BackfillExecutionReadinessStatus::Ready);
    assert!(readiness.blockers.is_empty());

    let artifact_index_proof: ArtifactIndexCommitProofReport =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("Artifact Index IAM-scope proof report parses");
    let expected_artifact_index_required_readiness: BackfillExecutionReadinessReport =
        serde_json::from_str(ARTIFACT_INDEX_REQUIRED_EXECUTION_READINESS_REPORT)
            .expect("Artifact Index-required execution readiness parses");
    let artifact_index_required_readiness =
        evaluate_backfill_execution_readiness(BackfillExecutionReadinessInput {
            readiness_id: "binance-bnbusdc-2026-03-01-artifact-index-required-readiness",
            accepted_tranche_manifest_hash: &accepted_tranche_manifest_hash,
            tranche: &actual_tranche,
            execution_plan_hash: &execution_plan_hash,
            plan: &actual_plan,
            required_table_family: &actual_plan.table_family,
            required_nt_data_type: "TradeTick",
            supported_data_paths: vec![BackfillExecutionReadinessSupportedDataPath {
                table_family: actual_plan.table_family.clone(),
                nt_data_type: "TradeTick".to_string(),
            }],
            artifact_index_commit_required: true,
            required_artifact_index_kind: Some(ArtifactKind::Backtests),
            artifact_index_commit_proof_report_hash: Some(&sha256_hex(
                ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES,
            )),
            artifact_index_commit_proof_report: Some(&artifact_index_proof),
            source_selection_readiness_required: false,
            source_selection_readiness_report_hash: None,
            source_selection_readiness_report: None,
            source_catalog_mapping_readiness_required: true,
            source_catalog_mapping_readiness_report_hash: Some(
                &source_catalog_mapping_readiness_hash,
            ),
            source_catalog_mapping_readiness_report: Some(&actual_catalog_mapping_readiness),
        });

    assert_eq!(
        artifact_index_required_readiness,
        expected_artifact_index_required_readiness
    );
    assert_eq!(
        artifact_index_required_readiness.status,
        BackfillExecutionReadinessStatus::Blocked
    );
    assert_eq!(
        artifact_index_required_readiness.blockers,
        vec![BackfillExecutionReadinessBlocker::ArtifactIndexProducerIamScopeUnproven]
    );
}

#[test]
fn binance_backfill_gate_commits_materialized_run_spec_before_execution_plan() {
    let materialization_spec_path =
        Path::new(BINANCE_GATE_ROOT).join("backfill-run-spec-materialization.toml");
    let materialized_run_spec_path = Path::new(BINANCE_GATE_ROOT)
        .join("materialized-run-spec")
        .join("backfill-run-spec.toml");

    assert!(
        materialization_spec_path.exists(),
        "Binance reference gate must commit the run-spec materialization spec"
    );
    assert!(
        materialized_run_spec_path.exists(),
        "Binance reference gate must commit the materialized run spec used by the execution plan"
    );
    assert!(
        EXECUTION_PLAN_SPEC.contains(
            "backfill-gates/binance-bnbusdc-2026-03-01/materialized-run-spec/backfill-run-spec.toml"
        ),
        "Binance reference execution plan must consume the materialized run spec"
    );
}

#[test]
fn blocked_canonical_source_catalog_mapping_reference_artifact_matches_generic_evaluator() {
    let mapping_evaluation: SourceCatalogMappingEvaluation =
        serde_json::from_str(CATALOG_MAPPING_EVALUATION).expect("mapping evaluation parses");
    let expected_blocked_readiness: SourceCatalogMappingReadinessReport =
        serde_json::from_str(BLOCKED_SOURCE_CATALOG_MAPPING_READINESS_REPORT)
            .expect("blocked source catalog-mapping readiness parses");

    let actual_blocked_readiness =
        evaluate_source_catalog_mapping_readiness(SourceCatalogMappingReadinessInput {
            readiness_id: &expected_blocked_readiness.readiness_id,
            catalog_mapping_evaluation_hash: &sha256_hex(CATALOG_MAPPING_EVALUATION_BYTES),
            source_sample_mapping_status: &mapping_evaluation.source_sample_mapping_status,
            source_proof_id: &expected_blocked_readiness.source_proof_id,
            source_proof_version: expected_blocked_readiness.source_proof_version,
            source_binding: &expected_blocked_readiness.source_binding,
            required_table_family: &expected_blocked_readiness.required_table_family,
            required_nt_data_types: expected_blocked_readiness.required_nt_data_types.clone(),
            allowed_current_bte_statuses: expected_blocked_readiness
                .allowed_current_bte_statuses
                .clone(),
            allowed_parquet_catalog_statuses: expected_blocked_readiness
                .allowed_parquet_catalog_statuses
                .clone(),
        });

    assert_eq!(actual_blocked_readiness, expected_blocked_readiness);
    assert_eq!(
        actual_blocked_readiness.status,
        SourceCatalogMappingReadinessStatus::Blocked
    );
    assert!(!actual_blocked_readiness.blockers.is_empty());
}

#[test]
fn artifact_index_commit_status_references_committed_proof_reports() {
    let status: serde_json::Value =
        serde_json::from_str(ARTIFACT_INDEX_COMMIT_STATUS).expect("Artifact Index status parses");
    let direct_s3_proof: ArtifactIndexCommitProofReport =
        serde_json::from_str(ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT)
            .expect("direct S3 proof report parses");
    let iam_scope_proof: ArtifactIndexCommitProofReport =
        serde_json::from_str(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT)
            .expect("IAM-scope proof report parses");

    assert!(direct_s3_proof.direct_s3_commit_proven);
    assert!(direct_s3_proof.event_create_only_proven);
    assert!(direct_s3_proof.latest_pointer_update_if_match_proven);
    assert!(!iam_scope_proof.producer_iam_scope_proven);
    assert_eq!(iam_scope_proof.producer_iam_scope_denied_write_attempts, 3);
    assert_eq!(
        iam_scope_proof.producer_iam_scope_denied_write_rejections,
        0
    );
    assert_eq!(iam_scope_proof.producer_iam_scope_violation_count, 3);

    assert_eq!(
        status["store_commit_mechanics"]["report_content_hash"]
            .as_str()
            .expect("store commit proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_DIRECT_S3_PROOF_REPORT_BYTES)
    );
    assert_eq!(
        status["producer_iam_scope"]["report_content_hash"]
            .as_str()
            .expect("producer IAM proof hash is a string"),
        sha256_hex(ARTIFACT_INDEX_IAM_SCOPE_PROOF_REPORT_BYTES)
    );
    assert_eq!(status["bte_006_can_close"].as_bool(), Some(false));
}

fn source_proof_scope_hash(report: &BackfillSourceProofScopeReport) -> String {
    let bytes = serde_json::to_vec(report).expect("scope report serializes");
    sha256_hex(&bytes)
}

#[derive(Debug, Deserialize)]
struct SourceCatalogMappingEvaluation {
    nt_surface_evidence: NtSurfaceEvidence,
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}

#[derive(Debug, Deserialize)]
struct NtSurfaceEvidence {
    current_bte_manifest_exposure: CurrentBteManifestExposure,
}

#[derive(Debug, Deserialize)]
struct CurrentBteManifestExposure {
    accepted_data_classes: Vec<String>,
    rejected_for_now: Vec<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
