use backtesting_vertical_slice::{
    backfill_accepted_tranche::{
        BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
        BackfillAcceptedTrancheObject, BackfillAcceptedTrancheStatus,
    },
    backfill_execution_plan::{
        BACKFILL_EXECUTION_PLAN_FILE, BackfillExecutionPlan, BackfillExecutionPlanIssue,
        BackfillExecutionPlanStatus, BackfillExecutionRunBinding, BackfillExecutionWorkBudget,
        evaluate_backfill_execution_plan, write_backfill_execution_plan,
    },
    source_proof::SourceProofUsageScope,
};
use sha2::{Digest, Sha256};

#[test]
fn execution_plan_is_ready_only_for_matching_accepted_tranche_and_run_spec_binding() {
    let tranche = accepted_tranche();
    let binding = matching_run_binding();

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &tranche,
        "synthetic-run-spec-hash",
        &binding,
        work_budget(),
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Ready);
    assert!(plan.blocking_issues.is_empty());
    assert_eq!(
        plan.accepted_tranche_manifest_hash,
        "synthetic-tranche-hash"
    );
    assert_eq!(plan.run_spec_hash, "synthetic-run-spec-hash");
    assert_eq!(plan.source_proof_id, tranche.source_proof_id);
    assert_eq!(plan.source_proof_version, tranche.source_proof_version);
    assert_eq!(plan.source_binding, tranche.source_binding);
    assert_eq!(plan.table_family, tranche.table_family);
    assert_eq!(plan.source_usage_scope, tranche.source_usage_scope);
    assert_eq!(plan.operator_run_id, binding.run_id);
    assert_eq!(plan.object_count, 1);
    assert_eq!(plan.accepted_bytes, 17);
    assert_eq!(plan.max_object_bytes, 17);
    assert_eq!(plan.max_decoded_bytes, 4096);
    assert_eq!(plan.max_source_rows, 128);
    assert_eq!(plan.max_projected_row_groups, 1);
    assert_eq!(plan.max_wall_seconds, 30);
    assert_eq!(plan.objects.len(), 1);
    assert_eq!(plan.objects[0].sha256, "synthetic-object-sha");
}

#[test]
fn execution_plan_carries_object_selection_metadata() {
    let mut tranche = accepted_tranche();
    tranche.objects[0].source_row_groups = vec![3, 5];
    tranche.objects[0].predicate_ref = Some("source-proof://synthetic/row-groups".to_string());

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &tranche,
        "synthetic-run-spec-hash",
        &matching_run_binding(),
        work_budget(),
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Ready);
    assert_eq!(plan.objects[0].source_row_groups, vec![3, 5]);
    assert_eq!(
        plan.objects[0].predicate_ref.as_deref(),
        Some("source-proof://synthetic/row-groups")
    );
}

#[test]
fn execution_plan_blocks_required_object_selection_metadata_before_payload_fetch() {
    let mut budget = work_budget();
    budget.require_object_selection_metadata = true;

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &accepted_tranche(),
        "synthetic-run-spec-hash",
        &matching_run_binding(),
        budget,
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Blocked);
    assert!(plan.objects.is_empty());
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::ExecutionPlanObjectSelectionMetadataMissing)
    );
}

#[test]
fn execution_plan_blocks_run_spec_usage_scope_mismatch_before_payload_fetch() {
    let tranche = accepted_tranche();
    let mut binding = matching_run_binding();
    binding.source_usage_scope = SourceProofUsageScope::OneOffBackfillData;

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &tranche,
        "synthetic-run-spec-hash",
        &binding,
        work_budget(),
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Blocked);
    assert!(plan.objects.is_empty());
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::RunSpecSourceUsageScopeMismatch)
    );
}

#[test]
fn execution_plan_blocks_run_spec_object_mismatch_before_payload_fetch() {
    let tranche = accepted_tranche();
    let mut binding = matching_run_binding();
    binding.accepted_object_sha256 = "different-object-sha".to_string();

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &tranche,
        "synthetic-run-spec-hash",
        &binding,
        work_budget(),
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Blocked);
    assert!(plan.objects.is_empty());
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::RunSpecObjectShaMismatch)
    );
}

#[test]
fn execution_plan_blocks_run_spec_table_family_mismatch_before_payload_fetch() {
    let mut tranche = accepted_tranche();
    let binding = matching_run_binding();
    tranche.table_family = format!("{}-mismatch", binding.table_family);

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &tranche,
        "synthetic-run-spec-hash",
        &binding,
        work_budget(),
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Blocked);
    assert!(plan.objects.is_empty());
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::RunSpecTableFamilyMismatch)
    );
}

#[test]
fn execution_plan_blocks_missing_work_budgets_before_payload_fetch() {
    let tranche = accepted_tranche();
    let binding = matching_run_binding();

    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &tranche,
        "synthetic-run-spec-hash",
        &binding,
        BackfillExecutionWorkBudget {
            max_decoded_bytes: u64::MAX,
            max_source_rows: 0,
            max_projected_row_groups: 0,
            max_wall_seconds: 0,
            require_object_selection_metadata: false,
        },
    );

    assert_eq!(plan.status, BackfillExecutionPlanStatus::Blocked);
    assert!(plan.objects.is_empty());
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::ExecutionPlanSourceRowBudgetMissing)
    );
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::ExecutionPlanProjectedRowGroupBudgetMissing)
    );
    assert!(
        plan.blocking_issues
            .contains(&BackfillExecutionPlanIssue::ExecutionPlanWallTimeBudgetMissing)
    );
}

#[test]
fn execution_plan_writer_is_idempotent_and_refuses_dirty_existing_artifact() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let plan = evaluate_backfill_execution_plan(
        "synthetic-plan",
        "synthetic-tranche-hash",
        &accepted_tranche(),
        "synthetic-run-spec-hash",
        &matching_run_binding(),
        work_budget(),
    );

    let first = write_backfill_execution_plan(dir.path(), &plan).expect("first write");
    let second = write_backfill_execution_plan(dir.path(), &plan).expect("second write");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(first.path, dir.path().join(BACKFILL_EXECUTION_PLAN_FILE));

    std::fs::write(&first.path, b"not the same plan").expect("dirty plan");
    let err = write_backfill_execution_plan(dir.path(), &plan).expect_err("dirty existing plan");
    assert!(err.to_string().contains("existing file content differs"));
}

#[test]
fn retained_execution_plan_artifact_hash_matches_written_file_bytes() {
    let retained_plan_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference")
        .join("source-universe-execution-packs")
        .join("binance-data-vision-trades-2026-03-01-all-instruments")
        .join("execution-pack/runs")
        .join(
            "00000-source-universe-operator-run-binance-data-vision-trades-2026-03-01-all-instruments-00000",
        )
        .join(BACKFILL_EXECUTION_PLAN_FILE);
    let plan_bytes = std::fs::read(retained_plan_path).expect("read retained execution plan");
    let plan: BackfillExecutionPlan =
        serde_json::from_slice(&plan_bytes).expect("parse retained execution plan");
    let dir = tempfile::TempDir::new().expect("temp dir");

    let artifact = write_backfill_execution_plan(dir.path(), &plan).expect("write retained plan");
    let written = std::fs::read(&artifact.path).expect("read written execution plan");

    assert_eq!(artifact.content_hash, hex::encode(Sha256::digest(&written)));
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
        objects: vec![BackfillAcceptedTrancheObject {
            s3_uri: "s3://synthetic-artifacts/raw/object=synthetic-object-sha.csv.gz".to_string(),
            source_url: "https://data.example.invalid/synthetic-object.csv.gz".to_string(),
            sha256: "synthetic-object-sha".to_string(),
            bytes: 17,
            archive_date: "2026-03-01".to_string(),
            source_row_groups: Vec::new(),
            predicate_ref: None,
        }],
        blocking_issues: Vec::new(),
    }
}

fn matching_run_binding() -> BackfillExecutionRunBinding {
    BackfillExecutionRunBinding {
        run_id: "synthetic-backtest-run".to_string(),
        output_prefix: "s3://synthetic-artifacts/backtests/synthetic-backtest-run".to_string(),
        source_proof_id: "source-proof-synthetic-native-trades".to_string(),
        source_proof_version: 3,
        source_binding: "synthetic-native-trades".to_string(),
        table_family: "trades".to_string(),
        source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        raw_sample_uri: "s3://synthetic-artifacts/raw/object=synthetic-object-sha.csv.gz"
            .to_string(),
        raw_sample_hash: "synthetic-object-sha".to_string(),
        accepted_object_s3_uri: "s3://synthetic-artifacts/raw/object=synthetic-object-sha.csv.gz"
            .to_string(),
        accepted_object_source_url: "https://data.example.invalid/synthetic-object.csv.gz"
            .to_string(),
        accepted_object_sha256: "synthetic-object-sha".to_string(),
        accepted_object_bytes: 17,
        accepted_object_archive_date: "2026-03-01".to_string(),
        max_object_bytes: 17,
        max_decoded_bytes: 4096,
    }
}

fn work_budget() -> BackfillExecutionWorkBudget {
    BackfillExecutionWorkBudget {
        max_decoded_bytes: u64::MAX,
        max_source_rows: 128,
        max_projected_row_groups: 1,
        max_wall_seconds: 30,
        require_object_selection_metadata: false,
    }
}
