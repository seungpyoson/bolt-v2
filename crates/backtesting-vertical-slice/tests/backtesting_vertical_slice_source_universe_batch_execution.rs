use std::{fs, path::Path};

use backtesting_vertical_slice::source_universe_batch_execution::{
    SourceUniverseBatchExecutionReportStatus, SourceUniverseBatchExecutionRunOutput,
    SourceUniverseObjectFetcher, SourceUniverseOperatorRunner, execute_source_universe_batch,
    write_source_universe_batch_execution_report,
};
use backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPackRecord;

#[test]
fn source_universe_batch_execution_fetches_verifies_and_runs_pack_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let object_sha256 = sha256_hex(object_bytes);

    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");
    fs::write(
        &pack_path,
        format!(
            r#"{{
  "schema_version": "source-universe-execution-pack.v1",
  "pack_id": "source-universe-execution-pack-synthetic",
  "status": "ready",
  "work_order_id": "source-universe-conversion-work-order-synthetic",
  "input_id": "source-universe-operator-inputs-synthetic",
  "gate_id": "source-universe-object-gates-synthetic",
  "conversion_run_plan_id": "source-universe-conversion-run-plan-synthetic",
  "universe_id": "backfill-source-universe-synthetic",
  "venue": "bybit",
  "source": "public_archive",
  "family": "tick_trades",
  "table_family": "trades",
  "planned_object_count": 1,
  "executable_record_count": 1,
  "withheld_record_count": 0,
  "selected_record_count": 1,
  "materialized_record_count": 1,
  "skipped_executable_record_count": 0,
  "executable_source_bytes": {object_bytes_len},
  "materialized_source_bytes": {object_bytes_len},
  "artifact_refs": [],
  "records": [
    {{
      "sequence": 0,
      "work_item_id": "synthetic-work-item",
      "operator_run_id": "source-universe-operator-run-synthetic-00000",
      "source_binding": "bybit-spot-tick-trades",
      "category": "spot",
      "symbol": "BTCUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://bolt-parquet/raw/synthetic.csv.gz",
      "source_url": "https://public.bybit.example/BTCUSDT2026-03-01.csv.gz",
      "selected_object_sha256": "{object_sha256}",
      "selected_object_bytes": {object_bytes_len},
      "source_proof_id": "source-proof-synthetic",
      "source_proof_version": 1,
      "accepted_tranche_id": "accepted-tranche-synthetic",
      "output_prefix": "s3://bolt-parquet/nt-research-analytics/backtests/synthetic",
      "run_spec_path": "{run_spec_path}",
      "run_spec_sha256": "run-spec-sha",
      "accepted_tranche_path": "accepted-tranche.json",
      "accepted_tranche_sha256": "accepted-tranche-sha",
      "execution_plan_path": "{execution_plan_path}",
      "execution_plan_sha256": "execution-plan-sha"
    }}
  ],
  "blocking_reasons": []
}}"#,
            object_bytes_len = object_bytes.len(),
            object_sha256 = object_sha256,
            run_spec_path = run_spec_path.display(),
            execution_plan_path = execution_plan_path.display(),
        ),
    )
    .expect("write pack");

    let mut fetcher = StaticFetcher {
        expected_source_url: "https://public.bybit.example/BTCUSDT2026-03-01.csv.gz".to_string(),
        object_bytes: object_bytes.to_vec(),
        calls: 0,
    };
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    )
    .expect("batch executes");

    assert_eq!(report.status, SourceUniverseBatchExecutionReportStatus::Completed);
    assert_eq!(report.pack_id, "source-universe-execution-pack-synthetic");
    assert_eq!(report.selected_record_count, 1);
    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.failed_record_count, 0);
    assert_eq!(report.total_canonical_rows, 7);
    assert_eq!(report.total_nt_catalog_rows, 7);
    assert_eq!(fetcher.calls, 1);
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(runner.calls[0].operator_run_id, "source-universe-operator-run-synthetic-00000");
    assert_eq!(runner.calls[0].object_bytes, object_bytes);
    assert_eq!(runner.calls[0].run_spec_path, run_spec_path);
    assert_eq!(runner.calls[0].execution_plan_path, execution_plan_path);
    assert_eq!(
        runner.calls[0].output_dir,
        output_dir.join("source-universe-operator-run-synthetic-00000")
    );

    let artifact = write_source_universe_batch_execution_report(&output_dir, &report)
        .expect("write batch report");
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read report"))
            .expect("parse report");
    assert_eq!(
        written["batch_id"],
        "source-universe-batch-synthetic"
    );
    assert_eq!(written["completed_record_count"], 1);
    assert_eq!(artifact.completed_record_count, 1);
}

struct StaticFetcher {
    expected_source_url: String,
    object_bytes: Vec<u8>,
    calls: usize,
}

impl SourceUniverseObjectFetcher for StaticFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        assert_eq!(record.source_url, self.expected_source_url);
        self.calls += 1;
        Ok(self.object_bytes.clone())
    }
}

#[derive(Default)]
struct RecordingRunner {
    calls: Vec<RunCall>,
}

struct RunCall {
    operator_run_id: String,
    object_bytes: Vec<u8>,
    run_spec_path: std::path::PathBuf,
    execution_plan_path: std::path::PathBuf,
    output_dir: std::path::PathBuf,
}

impl SourceUniverseOperatorRunner for RecordingRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        object_bytes: &[u8],
        run_spec_path: &Path,
        execution_plan_path: &Path,
        output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        self.calls.push(RunCall {
            operator_run_id: record.operator_run_id.clone(),
            object_bytes: object_bytes.to_vec(),
            run_spec_path: run_spec_path.to_path_buf(),
            execution_plan_path: execution_plan_path.to_path_buf(),
            output_dir: output_dir.to_path_buf(),
        });
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: "catalog-hash".to_string(),
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(bytes))
}
