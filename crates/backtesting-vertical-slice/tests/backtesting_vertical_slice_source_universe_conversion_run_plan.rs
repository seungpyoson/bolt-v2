use std::{fs, path::Path};

use backtesting_vertical_slice::source_universe_conversion_run_plan::{
    SourceUniverseConversionRunPlan, SourceUniverseConversionRunPlanStatus,
    write_source_universe_conversion_run_plan_from_spec_file,
};

#[test]
fn source_universe_conversion_run_plan_covers_every_bybit_object_gate() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let object_gates_path = reference_root.join(
        "source-universe-object-gates/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/gates/source-universe-object-gates.json",
    );
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("run-plan");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.toml");

    fs::write(
        &spec_path,
        format!(
            r#"
plan_id = "source-universe-conversion-run-plan-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
source_universe_object_gates_path = "{object_gates_path}"
output_dir = "{output_dir}"
max_objects_per_run = 500
max_source_bytes_per_run = 2000000000
"#,
            object_gates_path = object_gates_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_conversion_run_plan_from_spec_file(&spec_path)
        .expect("run plan generation succeeds");
    let plan: SourceUniverseConversionRunPlan =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read run plan"))
            .expect("run plan parses");

    assert_eq!(
        plan.schema_version,
        "source-universe-conversion-run-plan.v1"
    );
    assert_eq!(plan.status, SourceUniverseConversionRunPlanStatus::Ready);
    assert_eq!(
        plan.gate_id,
        "source-universe-object-gates-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
    );
    assert_eq!(plan.object_count, 5_857);
    assert_eq!(plan.planned_object_count, 5_857);
    assert_eq!(plan.source_binding_count, 3);
    assert_eq!(plan.total_source_bytes, 20_309_079_098);
    assert_eq!(plan.planned_source_bytes, 20_309_079_098);
    assert!(plan.run_count > 3, "budgeted plan must split category runs");
    assert_eq!(plan.runs.len(), plan.run_count as usize);
    assert!(
        plan.runs
            .iter()
            .all(|run| run.object_count <= 500 && run.source_bytes <= 2_000_000_000),
        "every generated run must obey configured object and byte budgets"
    );
    assert!(
        plan.runs
            .iter()
            .all(|run| run.work_item_ids.len() == run.object_count as usize),
        "each run must carry every gated work item id it schedules"
    );

    let planned_ids = plan
        .runs
        .iter()
        .flat_map(|run| run.work_item_ids.iter())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        planned_ids.len(),
        5_857,
        "every source-object gate must be assigned to exactly one run"
    );
    assert_eq!(plan.category_summaries.len(), 3);
    assert_eq!(plan.category_summaries[0].category, "inverse");
    assert_eq!(plan.category_summaries[0].object_count, 702);
    assert_eq!(plan.category_summaries[1].category, "linear");
    assert_eq!(plan.category_summaries[1].object_count, 1_851);
    assert_eq!(plan.category_summaries[2].category, "spot");
    assert_eq!(plan.category_summaries[2].object_count, 3_304);
}

#[test]
fn source_universe_conversion_run_plan_splits_when_next_object_exceeds_budget() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let output_dir = temp_dir.path().join("run-plan");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.toml");

    fs::write(
        &gates_path,
        r#"{
  "schema_version": "source-universe-object-gates.v1",
  "gate_id": "synthetic-source-universe-object-gates",
  "status": "ready",
  "queue_id": "synthetic-queue",
  "manifest_id": "synthetic-manifest",
  "universe_id": "synthetic-universe",
  "venue": "synthetic-venue",
  "source": "synthetic-source",
  "family": "synthetic-family",
  "table_family": "trades",
  "queue_path": "synthetic-queue.json",
  "queue_hash": "queue-hash",
  "work_item_count": 4,
  "accepted_gate_count": 4,
  "source_binding_count": 1,
  "total_accepted_bytes": 101,
  "source_binding_summaries": [],
  "artifact_refs": [],
  "records": [
    {
      "work_item_id": "item-1",
      "gate_status": "ready",
      "source_binding": "synthetic-binding",
      "table_family": "trades",
      "category": "linear",
      "symbol": "AAA",
      "archive_date": "2026-03-01",
      "source_uri": "s3://synthetic/item-1",
      "source_url": "https://example.test/item-1.csv.gz",
      "selected_object_sha256": "hash-1",
      "selected_object_bytes": 40,
      "source_proof_id": "proof",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "manifest-linear",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-1",
      "accepted_tranche_id": "tranche-1",
      "output_prefix": "s3://synthetic/out-1"
    },
    {
      "work_item_id": "item-2",
      "gate_status": "ready",
      "source_binding": "synthetic-binding",
      "table_family": "trades",
      "category": "linear",
      "symbol": "BBB",
      "archive_date": "2026-03-02",
      "source_uri": "s3://synthetic/item-2",
      "source_url": "https://example.test/item-2.csv.gz",
      "selected_object_sha256": "hash-2",
      "selected_object_bytes": 40,
      "source_proof_id": "proof",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "manifest-linear",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-2",
      "accepted_tranche_id": "tranche-2",
      "output_prefix": "s3://synthetic/out-2"
    },
    {
      "work_item_id": "item-3",
      "gate_status": "ready",
      "source_binding": "synthetic-binding",
      "table_family": "trades",
      "category": "linear",
      "symbol": "CCC",
      "archive_date": "2026-03-03",
      "source_uri": "s3://synthetic/item-3",
      "source_url": "https://example.test/item-3.csv.gz",
      "selected_object_sha256": "hash-3",
      "selected_object_bytes": 20,
      "source_proof_id": "proof",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "manifest-linear",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-3",
      "accepted_tranche_id": "tranche-3",
      "output_prefix": "s3://synthetic/out-3"
    },
    {
      "work_item_id": "item-4",
      "gate_status": "ready",
      "source_binding": "synthetic-binding",
      "table_family": "trades",
      "category": "linear",
      "symbol": "DDD",
      "archive_date": "2026-03-04",
      "source_uri": "s3://synthetic/item-4",
      "source_url": "https://example.test/item-4.csv.gz",
      "selected_object_sha256": "hash-4",
      "selected_object_bytes": 1,
      "source_proof_id": "proof",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "manifest-linear",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-4",
      "accepted_tranche_id": "tranche-4",
      "output_prefix": "s3://synthetic/out-4"
    }
  ]
}"#,
    )
    .expect("write gates");
    fs::write(
        &spec_path,
        format!(
            r#"
plan_id = "synthetic-run-plan"
source_universe_object_gates_path = "{gates_path}"
output_dir = "{output_dir}"
max_objects_per_run = 2
max_source_bytes_per_run = 60
"#,
            gates_path = gates_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_conversion_run_plan_from_spec_file(&spec_path)
        .expect("run plan generation succeeds");
    let plan: SourceUniverseConversionRunPlan =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read run plan"))
            .expect("run plan parses");

    assert_eq!(plan.run_count, 3);
    assert_eq!(plan.runs[0].work_item_ids, vec!["item-1"]);
    assert_eq!(plan.runs[0].source_bytes, 40);
    assert_eq!(plan.runs[1].work_item_ids, vec!["item-2", "item-3"]);
    assert_eq!(plan.runs[1].source_bytes, 60);
    assert_eq!(plan.runs[2].work_item_ids, vec!["item-4"]);
    assert_eq!(plan.runs[2].source_bytes, 1);
}

#[test]
fn source_universe_conversion_run_plan_overwrites_existing_artifact_only_when_enabled() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let output_dir = temp_dir.path().join("run-plan");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.toml");

    fs::write(
        &gates_path,
        r#"{
  "schema_version": "source-universe-object-gates.v1",
  "gate_id": "synthetic-source-universe-object-gates",
  "status": "ready",
  "queue_id": "synthetic-queue",
  "manifest_id": "synthetic-manifest",
  "universe_id": "synthetic-universe",
  "venue": "synthetic-venue",
  "source": "synthetic-source",
  "family": "synthetic-family",
  "table_family": "trades",
  "queue_path": "synthetic-queue.json",
  "queue_hash": "queue-hash",
  "work_item_count": 0,
  "accepted_gate_count": 0,
  "source_binding_count": 0,
  "total_accepted_bytes": 0,
  "source_binding_summaries": [],
  "artifact_refs": [],
  "records": []
}"#,
    )
    .expect("write gates");
    fs::create_dir_all(&output_dir).expect("create output dir");
    fs::write(
        output_dir.join("source-universe-conversion-run-plan.json"),
        br#"{"stale":true}"#,
    )
    .expect("write stale output");

    fs::write(
        &spec_path,
        format!(
            r#"
plan_id = "synthetic-run-plan"
source_universe_object_gates_path = "{gates_path}"
output_dir = "{output_dir}"
max_objects_per_run = 2
max_source_bytes_per_run = 60
"#,
            gates_path = gates_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let err = write_source_universe_conversion_run_plan_from_spec_file(&spec_path)
        .expect_err("dirty output is protected by default");
    assert!(
        err.to_string()
            .contains("dirty source-universe conversion run-plan")
    );

    fs::write(
        &spec_path,
        format!(
            r#"
plan_id = "synthetic-run-plan"
source_universe_object_gates_path = "{gates_path}"
output_dir = "{output_dir}"
max_objects_per_run = 2
max_source_bytes_per_run = 60
overwrite_existing_artifacts = true
"#,
            gates_path = gates_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write overwrite spec");

    let artifact = write_source_universe_conversion_run_plan_from_spec_file(&spec_path)
        .expect("overwrite enabled");
    let plan: SourceUniverseConversionRunPlan =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read run plan"))
            .expect("run plan parses");
    assert_eq!(plan.runs.len(), 0);
}
