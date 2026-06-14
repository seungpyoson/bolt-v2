use std::path::PathBuf;

use backtesting_vertical_slice::{
    backfill_conversion_batch::{
        BackfillConversionBatchInput, BackfillConversionBatchSelection,
        BackfillConversionBatchStatus, evaluate_backfill_conversion_batch_plan,
    },
    backfill_coverage::{
        BackfillCoverageLedger, BackfillCoverageRecord, BackfillCoverageStatus,
        BackfillCoverageSummary,
    },
    backfill_execution_plan::{
        BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION, BackfillExecutionPlan, BackfillExecutionPlanStatus,
    },
    source_proof::SourceProofUsageScope,
};

#[test]
fn conversion_batch_plan_groups_accepted_staging_records_without_canonical_ready() {
    let ledger = BackfillCoverageLedger {
        schema_version: "backfill-coverage-ledger.v1".to_string(),
        ledger_id: "synthetic-ledger".to_string(),
        records: vec![
            accepted_record("synthetic-tranche-2026-03-01", 100),
            accepted_record("synthetic-tranche-2026-03-02", 200),
        ],
        summary: BackfillCoverageSummary {
            total_records: 2,
            accepted_records: 2,
            accepted_with_gaps_records: 0,
            rejected_records: 0,
            physical_only_records: 0,
            canonical_ready_records: 0,
            accepted_objects: 2,
            accepted_bytes: 300,
            skipped_objects: 0,
            physical_only_objects: 0,
            physical_only_bytes: 0,
            blocking_issue_count: 0,
        },
    };

    let plan = evaluate_backfill_conversion_batch_plan(
        "synthetic-venue-batch",
        &ledger,
        &BackfillConversionBatchSelection {
            max_records: 2,
            max_accepted_objects: 2,
            max_accepted_bytes: 300,
            require_uniform_source_binding: true,
            allow_gaps: false,
        },
        vec![
            input_for("synthetic-tranche-2026-03-01", "run-spec-a-hash"),
            input_for("synthetic-tranche-2026-03-02", "run-spec-b-hash"),
        ],
    );

    assert_eq!(plan.status, BackfillConversionBatchStatus::Ready);
    assert!(plan.blocking_issues.is_empty());
    assert_eq!(plan.coverage_ledger_id, "synthetic-ledger");
    assert_eq!(plan.record_count, 2);
    assert_eq!(plan.total_accepted_objects, 2);
    assert_eq!(plan.total_accepted_bytes, 300);
    assert_eq!(plan.canonical_ready_records, 0);
    assert_eq!(plan.records.len(), 2);
    assert_eq!(plan.records[0].record_id, "synthetic-tranche-2026-03-01");
    assert_eq!(plan.records[0].source_binding, "synthetic-source-binding");
    assert_eq!(plan.records[0].table_family, "trades");
    assert_eq!(plan.records[0].run_spec_hash, "run-spec-a-hash");
    assert_eq!(
        plan.records[0].run_spec_path,
        PathBuf::from("run-spec-a.toml")
    );
    assert_eq!(
        plan.records[0].execution_plan_path,
        PathBuf::from("execution-plan-synthetic-tranche-2026-03-01.json")
    );
}

fn input_for(record_id: &str, run_spec_hash: &str) -> BackfillConversionBatchInput {
    BackfillConversionBatchInput {
        record_id: record_id.to_string(),
        run_spec_path: PathBuf::from(format!("run-spec-{}.toml", &run_spec_hash[9..10])),
        run_spec_hash: run_spec_hash.to_string(),
        execution_plan_path: PathBuf::from(format!("execution-plan-{record_id}.json")),
        execution_plan_hash: format!("execution-plan-hash-{record_id}"),
        execution_plan: BackfillExecutionPlan {
            schema_version: BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION.to_string(),
            plan_id: format!("execution-plan-{record_id}"),
            status: BackfillExecutionPlanStatus::Ready,
            accepted_tranche_id: record_id.to_string(),
            accepted_tranche_manifest_hash: format!("manifest-hash-{record_id}"),
            run_spec_hash: run_spec_hash.to_string(),
            operator_run_id: format!("operator-run-{record_id}"),
            output_prefix: format!("reference://synthetic/{record_id}"),
            source_proof_id: format!("source-proof-{record_id}"),
            source_proof_version: 1,
            source_binding: "synthetic-source-binding".to_string(),
            table_family: "trades".to_string(),
            source_usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            object_count: 1,
            accepted_bytes: if record_id.ends_with("01") { 100 } else { 200 },
            max_object_bytes: 1_000,
            max_decoded_bytes: 10_000,
            max_source_rows: 1_000,
            max_projected_row_groups: 1,
            max_wall_seconds: 30,
            require_object_selection_metadata: false,
            objects: Vec::new(),
            blocking_issues: Vec::new(),
        },
    }
}

fn accepted_record(record_id: &str, accepted_bytes: u64) -> BackfillCoverageRecord {
    BackfillCoverageRecord {
        record_id: record_id.to_string(),
        status: BackfillCoverageStatus::Accepted,
        source_binding: Some("synthetic-source-binding".to_string()),
        table_family: Some("trades".to_string()),
        coverage_axis: Some("archive_date".to_string()),
        source_proof_id: Some(format!("source-proof-{record_id}")),
        source_proof_version: Some(1),
        canonical_ready: false,
        accepted_objects: 1,
        accepted_bytes,
        skipped_objects: 0,
        physical_only_objects: 0,
        physical_only_bytes: 0,
        blocking_issues: Vec::new(),
    }
}
