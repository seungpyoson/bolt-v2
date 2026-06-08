use backtesting_vertical_slice::{
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
        supported_data_paths: supported_data_paths(),
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
        supported_data_paths: supported_data_paths(),
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
        object_count: tranche.object_count,
        accepted_bytes: tranche.accepted_bytes,
        max_object_bytes: tranche.accepted_bytes,
        max_decoded_bytes: 4096,
        objects: vec![BackfillExecutionPlanObject {
            s3_uri: object.s3_uri,
            source_url: object.source_url,
            sha256: object.sha256,
            bytes: object.bytes,
            archive_date: object.archive_date,
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
