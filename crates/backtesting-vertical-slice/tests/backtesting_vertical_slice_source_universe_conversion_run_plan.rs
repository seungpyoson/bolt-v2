use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::backtesting_vertical_slice_test_support::tempdir_in_repo_target;

use backtesting_vertical_slice::source_universe_conversion_run_plan::{
    SourceUniverseConversionRunPlan, SourceUniverseConversionRunPlanStatus,
    write_source_universe_conversion_run_plan_from_spec_file,
};

fn repo_relative_path(components: &[&str]) -> PathBuf {
    let mut path = PathBuf::new();
    for component in components {
        path.push(component);
    }
    path
}

#[test]
fn source_universe_conversion_run_plan_covers_every_bybit_object_gate() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let object_gates_path = reference_root.join(
        "source-universe-object-gates/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/gates/source-universe-object-gates.json",
    );
    let temp_dir = tempdir_in_repo_target();
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
    let expected_object_gates_path = repo_relative_path(&[
        "specs",
        "023-nt-research-analytics-platform",
        "reference",
        "source-universe-object-gates",
        "bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
        "gates",
        "source-universe-object-gates.json",
    ]);
    assert_eq!(plan.object_gates_path, expected_object_gates_path);
    assert!(
        plan.object_gates_path.is_relative(),
        "run-plan object_gates_path must be portable across checkouts"
    );
    let object_gates_artifact = plan
        .artifact_refs
        .iter()
        .find(|artifact_ref| artifact_ref.role == "source_universe_object_gates")
        .expect("run plan records the source-universe object-gates artifact");
    assert_eq!(object_gates_artifact.path, expected_object_gates_path);
    assert!(
        object_gates_artifact.path.is_relative(),
        "run-plan object-gates artifact ref path must be portable across checkouts"
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
fn committed_source_universe_conversion_run_plans_record_portable_object_gate_paths() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root exists");
    let expected_object_gates_prefix = repo_relative_path(&[
        "specs",
        "023-nt-research-analytics-platform",
        "reference",
        "source-universe-object-gates",
    ]);
    let mut checked = 0;

    for plan_path in source_universe_conversion_run_plan_json_paths(&repo_root) {
        let plan: SourceUniverseConversionRunPlan =
            serde_json::from_slice(&fs::read(&plan_path).expect("read committed run plan"))
                .expect("committed run plan parses");
        assert!(
            plan.object_gates_path.is_relative(),
            "{} object_gates_path must be repo-relative, got {}",
            plan_path.display(),
            plan.object_gates_path.display()
        );
        assert!(
            plan.object_gates_path
                .starts_with(&expected_object_gates_prefix),
            "{} object_gates_path must point at committed object gates, got {}",
            plan_path.display(),
            plan.object_gates_path.display()
        );

        let object_gates_artifact = plan
            .artifact_refs
            .iter()
            .find(|artifact_ref| artifact_ref.role == "source_universe_object_gates")
            .expect("run plan records the source-universe object-gates artifact");
        assert_eq!(
            object_gates_artifact.path,
            plan.object_gates_path,
            "{} object-gates artifact ref must match object_gates_path",
            plan_path.display()
        );
        assert!(
            object_gates_artifact.path.is_relative(),
            "{} object-gates artifact ref path must be repo-relative, got {}",
            plan_path.display(),
            object_gates_artifact.path.display()
        );
        assert!(
            object_gates_artifact
                .path
                .starts_with(&expected_object_gates_prefix),
            "{} object-gates artifact ref path must point at committed object gates, got {}",
            plan_path.display(),
            object_gates_artifact.path.display()
        );
        checked += 1;
    }

    assert!(
        checked > 0,
        "expected at least one committed source-universe conversion run-plan fixture"
    );
}

#[test]
fn source_universe_conversion_run_plan_splits_when_next_object_exceeds_budget() {
    let temp_dir = tempdir_in_repo_target();
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

fn source_universe_conversion_run_plan_json_paths(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("ls-files")
        .arg("specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans")
        .output()
        .expect("run git ls-files for committed source-universe conversion run-plan fixtures");
    assert!(
        output.status.success(),
        "git ls-files failed: status={:?}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("git ls-files output is UTF-8");
    let mut paths = stdout
        .lines()
        .filter(|path| path.ends_with("/run-plan/source-universe-conversion-run-plan.json"))
        .map(|path| root.join(path))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[test]
fn source_universe_conversion_run_plan_overwrites_existing_artifact_only_when_enabled() {
    let temp_dir = tempdir_in_repo_target();
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
