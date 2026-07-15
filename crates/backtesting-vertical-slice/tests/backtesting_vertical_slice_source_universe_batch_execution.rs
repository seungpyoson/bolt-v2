use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use backtesting_vertical_slice::source_universe_batch_execution::{
    CachingSourceUniverseObjectFetcher, HttpSourceUniverseObjectFetcher,
    SourceUniverseBatchExecutionConfig, SourceUniverseBatchExecutionReport,
    SourceUniverseBatchExecutionReportStatus, SourceUniverseBatchExecutionRunOutput,
    SourceUniverseObjectFetcher, SourceUniverseOperatorRunner, execute_source_universe_batch,
    execute_source_universe_batch_with_config, execute_source_universe_batch_with_factories,
    write_source_universe_batch_execution_report,
};
use backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPackRecord;

#[test]
fn source_universe_batch_execution_fetches_verifies_and_runs_pack_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let accepted_tranche_path = temp_dir.path().join("accepted-tranche.json");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let object_sha256 = sha256_hex(object_bytes);

    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&accepted_tranche_path, "{}\n").expect("write accepted tranche");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");
    let run_spec_sha256 = sha256_hex(&fs::read(&run_spec_path).expect("read run spec"));
    let accepted_tranche_sha256 = sha256_hex(
        &fs::read(&accepted_tranche_path).expect("read accepted tranche"),
    );
    let execution_plan_sha256 =
        sha256_hex(&fs::read(&execution_plan_path).expect("read execution plan"));
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
      "run_spec_sha256": "{run_spec_sha256}",
      "accepted_tranche_path": "{accepted_tranche_path}",
      "accepted_tranche_sha256": "{accepted_tranche_sha256}",
      "execution_plan_path": "{execution_plan_path}",
      "execution_plan_sha256": "{execution_plan_sha256}"
    }}
  ],
  "blocking_reasons": []
}}"#,
            object_bytes_len = object_bytes.len(),
            object_sha256 = object_sha256,
            run_spec_path = run_spec_path.display(),
            run_spec_sha256 = run_spec_sha256,
            accepted_tranche_path = accepted_tranche_path.display(),
            accepted_tranche_sha256 = accepted_tranche_sha256,
            execution_plan_path = execution_plan_path.display(),
            execution_plan_sha256 = execution_plan_sha256,
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

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::Completed
    );
    assert_eq!(report.pack_id, "source-universe-execution-pack-synthetic");
    assert_eq!(report.selected_record_count, 1);
    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.failed_record_count, 0);
    assert_eq!(report.total_canonical_rows, 7);
    assert_eq!(report.total_nt_catalog_rows, 7);
    assert_eq!(fetcher.calls, 1);
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(
        runner.calls[0].operator_run_id,
        "source-universe-operator-run-synthetic-00000"
    );
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
    assert_eq!(written["batch_id"], "source-universe-batch-synthetic");
    assert_eq!(written["completed_record_count"], 1);
    assert_eq!(artifact.completed_record_count, 1);
}

#[test]
fn source_universe_batch_execution_respects_start_sequence() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    let output_dir = temp_dir.path().join("batch-output");
    let first_object_bytes = b"first accepted object bytes";
    let second_object_bytes = b"second accepted object bytes";

    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");
    write_two_record_pack(
        &pack_path,
        &run_spec_path,
        &execution_plan_path,
        first_object_bytes,
        second_object_bytes,
    );

    let mut fetcher = SequencedFetcher::from_objects(&[(1, second_object_bytes.to_vec())]);
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: Some(1),
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("batch executes selected sequence");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::Completed
    );
    assert_eq!(report.selected_record_count, 1);
    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.failed_record_count, 0);
    assert_eq!(report.records[0].sequence, 1);
    assert_eq!(report.records[0].symbol, "SYNTHETIC-BBB");
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(
        runner.calls[0].operator_run_id,
        "source-universe-operator-run-synthetic-00001"
    );
}

#[test]
fn source_universe_batch_execution_can_continue_after_record_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    let output_dir = temp_dir.path().join("batch-output");
    let first_object_bytes = b"first accepted object bytes";
    let second_object_bytes = b"second accepted object bytes";

    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");
    write_two_record_pack(
        &pack_path,
        &run_spec_path,
        &execution_plan_path,
        first_object_bytes,
        second_object_bytes,
    );

    let mut fetcher = SequencedFetcher::from_objects(&[
        (0, b"wrong bytes".to_vec()),
        (1, second_object_bytes.to_vec()),
    ]);
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(2),
            continue_on_error: true,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("batch records failure and continues");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    );
    assert_eq!(report.selected_record_count, 2);
    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.failed_record_count, 1);
    assert_eq!(report.records[0].sequence, 1);
    assert_eq!(report.failures[0].sequence, 0);
    assert_eq!(report.failures[0].failure_stage, "verify_object");
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(
        runner.calls[0].operator_run_id,
        "source-universe-operator-run-synthetic-00001"
    );
}

#[test]
fn http_source_universe_fetcher_rejects_zero_timeout() {
    match HttpSourceUniverseObjectFetcher::new(Some(0), None) {
        Ok(_) => panic!("zero timeout accepted"),
        Err(err) => assert!(
            err.to_string()
                .contains("fetch_timeout_seconds must be positive")
        ),
    }
}

#[test]
fn caching_fetcher_miss_then_hit_avoids_inner_and_persists_atomically() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let cache_dir = temp_dir.path().join("object-cache");
    let object_bytes = b"synthetic accepted object bytes";
    let record = synthetic_record(0, object_bytes, "https://synthetic.example/object-0");

    let inner = CountingFetcher::new(vec![(0, object_bytes.to_vec())]);
    let inner_calls = inner.calls();
    let mut fetcher = CachingSourceUniverseObjectFetcher::new(inner, &cache_dir);

    let first = fetcher.fetch(&record).expect("first fetch populates cache");
    assert_eq!(first, object_bytes);
    assert_eq!(
        inner_calls.load(Ordering::SeqCst),
        1,
        "miss calls inner once"
    );

    let cache_path = cache_dir.join(&record.selected_object_sha256);
    assert!(cache_path.exists(), "cache entry persisted under sha key");
    assert_eq!(
        fs::read(&cache_path).expect("read cache entry"),
        object_bytes,
        "cache entry holds verified bytes"
    );
    let stray_temp = fs::read_dir(&cache_dir)
        .expect("read cache dir")
        .filter_map(|entry| entry.ok())
        .any(|entry| entry.file_name() != std::ffi::OsStr::new(&record.selected_object_sha256));
    assert!(!stray_temp, "no temp files left behind after atomic rename");

    let second = fetcher.fetch(&record).expect("second fetch hits cache");
    assert_eq!(second, object_bytes);
    assert_eq!(
        inner_calls.load(Ordering::SeqCst),
        1,
        "cache hit does not call inner again"
    );
}

#[test]
fn caching_fetcher_corrupt_entry_is_deleted_and_repaired() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let cache_dir = temp_dir.path().join("object-cache");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    let object_bytes = b"synthetic accepted object bytes";
    let record = synthetic_record(0, object_bytes, "https://synthetic.example/object-0");

    // Plant a corrupt cache entry under the sha key (wrong length and bytes).
    let cache_path = cache_dir.join(&record.selected_object_sha256);
    fs::write(&cache_path, b"corrupt cached payload").expect("plant corrupt entry");

    let inner = CountingFetcher::new(vec![(0, object_bytes.to_vec())]);
    let inner_calls = inner.calls();
    let mut fetcher = CachingSourceUniverseObjectFetcher::new(inner, &cache_dir);

    let bytes = fetcher
        .fetch(&record)
        .expect("corrupt entry falls through to inner");
    assert_eq!(bytes, object_bytes);
    assert_eq!(
        inner_calls.load(Ordering::SeqCst),
        1,
        "corruption triggers a single inner refetch"
    );
    assert_eq!(
        fs::read(&cache_path).expect("read repaired entry"),
        object_bytes,
        "corrupt entry is overwritten with verified bytes"
    );
}

#[test]
fn caching_fetcher_unverified_inner_bytes_never_enter_cache() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let cache_dir = temp_dir.path().join("object-cache");
    let object_bytes = b"synthetic accepted object bytes";
    let record = synthetic_record(0, object_bytes, "https://synthetic.example/object-0");

    // Inner returns bytes that do not match the pinned sha/length.
    let inner = CountingFetcher::new(vec![(0, b"wrong inner bytes".to_vec())]);
    let mut fetcher = CachingSourceUniverseObjectFetcher::new(inner, &cache_dir);

    let result = fetcher.fetch(&record);
    assert!(result.is_err(), "unverified inner bytes fail the fetch");
    let cache_path = cache_dir.join(&record.selected_object_sha256);
    assert!(
        !cache_path.exists(),
        "unverified bytes must never be written to the cache"
    );
}

#[test]
fn resume_carries_forward_prior_clean_record_without_refetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![
        (0u64, b"synthetic object zero".to_vec()),
        (1u64, b"synthetic object one".to_vec()),
    ];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    // First run covers ONLY sequence 0 (record_limit = 1), so the prior report
    // we resume from is a genuine partial: sequence 0 succeeded, sequence 1 was
    // never reached. This is what makes the resume meaningful — on the second
    // run sequence 0 carries forward (sha matches) while sequence 1, absent from
    // the prior report, must still be fetched and run.
    let first_output = temp_dir.path().join("batch-output-first");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let mut first_report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &first_output,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("first run completes");
    assert_eq!(
        first_report.completed_record_count, 1,
        "prior report is a partial: only sequence 0 was processed"
    );

    // The carry-forward gate re-opens the carried record's prior OUTPUT catalog
    // and re-hashes it through `logical_catalog_hash`, carrying the record
    // forward without refetch only when that recomputed hash matches the
    // recorded `catalog_hash`. The in-test `RecordingRunner` double records a
    // synthetic hash but writes no real NT catalog under `output_dir`, so to
    // exercise the intended "clean record, prior output present and matching"
    // path the test plants a real, hash-matching NT catalog under the prior
    // record's `output_dir` before resuming. This mirrors the src-side unit
    // test `carried_output_verifies_against_intact_reference_catalog`, which
    // copies the committed PMXT reference catalog into the record's output and
    // pins the catalog's recorded hash. It is NOT a second verification seam:
    // the resume run still proves carry-forward through the one real gate.
    let prior_output_dir = first_report.records[0].output_dir.clone();
    copy_dir_all(
        &committed_reference_run_dir().join("nt-catalog"),
        &prior_output_dir.join("nt-catalog"),
    );
    // Pin the carried record's `catalog_hash` to the planted catalog's recorded
    // hash so the gate verifies. Never mutate the committed reference; only the
    // temp copy and the in-memory prior report are touched.
    first_report.records[0].catalog_hash = committed_reference_catalog_hash();

    let resume_report_path = first_output.join("prior-report.json");
    fs::write(
        &resume_report_path,
        serde_json::to_vec_pretty(&first_report).expect("serialize prior report"),
    )
    .expect("write prior report");

    // Second run resumes; sequence 0 carried forward, only sequence 1 fetched.
    let second_output = temp_dir.path().join("batch-output-second");
    let mut resume_fetcher = SequencedFetcher::from_objects(&objects);
    let resume_calls = resume_fetcher.calls();
    let mut resume_runner = RecordingRunner::default();
    let resume_report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &second_output,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(2),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report_path.clone()),
        },
        &mut resume_fetcher,
        &mut resume_runner,
    )
    .expect("resume run completes");

    assert_eq!(
        resume_report.completed_record_count, 2,
        "carried + reprocessed both succeed"
    );
    assert_eq!(
        resume_report.total_canonical_rows, 14,
        "totals include carried rows"
    );
    assert_eq!(resume_report.total_nt_catalog_rows, 14);
    assert_eq!(
        resume_report.records[0].sequence, 0,
        "carried record stays in order"
    );
    assert_eq!(resume_report.records[1].sequence, 1);
    assert_eq!(
        resume_report.records[0].output_dir, first_report.records[0].output_dir,
        "carried record keeps prior provenance verbatim"
    );
    assert_eq!(
        resume_calls.lock().expect("fetch log").as_slice(),
        &[1u64],
        "only the non-carried sequence was fetched"
    );
    assert_eq!(
        resume_runner.calls.len(),
        1,
        "carried record skips the runner"
    );
}

#[test]
fn resume_sha_mismatch_reprocesses_the_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    // Prior report claims a different sha for sequence 0 (pack regenerated).
    let prior_report = SourceUniverseBatchExecutionReport {
        schema_version:
            backtesting_vertical_slice::source_universe_batch_execution::SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION
                .to_string(),
        batch_id: "source-universe-batch-synthetic".to_string(),
        status: SourceUniverseBatchExecutionReportStatus::Completed,
        pack_id: "source-universe-execution-pack-synthetic".to_string(),
        universe_id: "backfill-source-universe-synthetic".to_string(),
        venue: "synthetic-venue".to_string(),
        selected_record_count: 1,
        completed_record_count: 1,
        failed_record_count: 0,
        total_canonical_rows: 7,
        total_nt_catalog_rows: 7,
        records: vec![carried_record_fixture(0, "stale-sha-does-not-match")],
        failures: vec![],
    };
    let resume_report_path = temp_dir.path().join("prior-report.json");
    fs::write(
        &resume_report_path,
        serde_json::to_vec_pretty(&prior_report).expect("serialize prior report"),
    )
    .expect("write prior report");

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();
    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report_path),
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("sha mismatch reprocesses");

    assert_eq!(report.completed_record_count, 1);
    assert_eq!(
        fetch_calls.lock().expect("fetch log").as_slice(),
        &[0u64],
        "sha mismatch forces a refetch"
    );
    assert_eq!(runner.calls.len(), 1, "sha mismatch forces a rerun");
}

#[test]
fn resume_pack_id_mismatch_fails_loud() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    let prior_report = SourceUniverseBatchExecutionReport {
        schema_version:
            backtesting_vertical_slice::source_universe_batch_execution::SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION
                .to_string(),
        batch_id: "source-universe-batch-synthetic".to_string(),
        status: SourceUniverseBatchExecutionReportStatus::Completed,
        pack_id: "different-pack-id".to_string(),
        universe_id: "backfill-source-universe-synthetic".to_string(),
        venue: "synthetic-venue".to_string(),
        selected_record_count: 1,
        completed_record_count: 1,
        failed_record_count: 0,
        total_canonical_rows: 7,
        total_nt_catalog_rows: 7,
        records: vec![carried_record_fixture(0, &sha256_hex(b"synthetic object zero"))],
        failures: vec![],
    };
    let resume_report_path = temp_dir.path().join("prior-report.json");
    fs::write(
        &resume_report_path,
        serde_json::to_vec_pretty(&prior_report).expect("serialize prior report"),
    )
    .expect("write prior report");

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let result = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report_path),
        },
        &mut fetcher,
        &mut runner,
    );
    let err = result.expect_err("pack_id mismatch must fail loud");
    assert!(
        err.to_string().contains("pack_id") || format!("{err:#}").contains("pack_id"),
        "error names the pack_id mismatch: {err:#}"
    );
}

#[test]
fn resume_does_not_carry_forward_prior_failure_entries() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    // Prior report has sequence 0 ONLY as a failure entry, not a success.
    let prior_report = SourceUniverseBatchExecutionReport {
        schema_version:
            backtesting_vertical_slice::source_universe_batch_execution::SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION
                .to_string(),
        batch_id: "source-universe-batch-synthetic".to_string(),
        status: SourceUniverseBatchExecutionReportStatus::Failed,
        pack_id: "source-universe-execution-pack-synthetic".to_string(),
        universe_id: "backfill-source-universe-synthetic".to_string(),
        venue: "synthetic-venue".to_string(),
        selected_record_count: 1,
        completed_record_count: 0,
        failed_record_count: 1,
        total_canonical_rows: 0,
        total_nt_catalog_rows: 0,
        records: vec![],
        failures: vec![failure_record_fixture(0, &sha256_hex(b"synthetic object zero"))],
    };
    let resume_report_path = temp_dir.path().join("prior-report.json");
    fs::write(
        &resume_report_path,
        serde_json::to_vec_pretty(&prior_report).expect("serialize prior report"),
    )
    .expect("write prior report");

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();
    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report_path),
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("prior failure is reprocessed");

    assert_eq!(
        report.completed_record_count, 1,
        "failure entry was reprocessed"
    );
    assert_eq!(
        fetch_calls.lock().expect("fetch log").as_slice(),
        &[0u64],
        "prior failure entry is not carried forward"
    );
}

#[test]
fn parallel_overlaps_and_matches_serial_report() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects: Vec<(u64, Vec<u8>)> = (0..8u64)
        .map(|sequence| {
            (
                sequence,
                format!("synthetic object {sequence}").into_bytes(),
            )
        })
        .collect();
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    // Serial baseline.
    let serial_output = temp_dir.path().join("batch-output-serial");
    let serial_objects = objects.clone();
    let serial_report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &serial_output,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(8),
            continue_on_error: false,
            max_concurrent_records: Some(1),
            resume_report: None,
        },
        {
            let objects = serial_objects.clone();
            move || Ok(SequencedFetcher::from_objects(&objects))
        },
        || Ok(ConcurrencyRunner::new(None)),
    )
    .expect("serial run completes");

    // Parallel run with a shared concurrency high-water mark.
    let parallel_output = temp_dir.path().join("batch-output-parallel");
    let high_water = std::sync::Arc::new(AtomicUsize::new(0));
    let active = std::sync::Arc::new(AtomicUsize::new(0));
    let parallel_objects = objects.clone();
    let parallel_report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &parallel_output,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(8),
            continue_on_error: false,
            max_concurrent_records: Some(4),
            resume_report: None,
        },
        {
            let objects = parallel_objects.clone();
            move || Ok(SequencedFetcher::from_objects(&objects))
        },
        {
            let high_water = std::sync::Arc::clone(&high_water);
            let active = std::sync::Arc::clone(&active);
            move || {
                Ok(ConcurrencyRunner::new(Some(ConcurrencyProbe {
                    high_water: std::sync::Arc::clone(&high_water),
                    active: std::sync::Arc::clone(&active),
                })))
            }
        },
    )
    .expect("parallel run completes");

    assert!(
        high_water.load(Ordering::SeqCst) > 1,
        "parallel run actually overlapped records (high water = {})",
        high_water.load(Ordering::SeqCst)
    );
    // Reports must be field-for-field identical except the per-record output_dir,
    // which embeds the (distinct) batch output root. Compare everything else.
    assert_eq!(parallel_report.status, serial_report.status);
    assert_eq!(
        parallel_report.completed_record_count,
        serial_report.completed_record_count
    );
    assert_eq!(
        parallel_report.failed_record_count,
        serial_report.failed_record_count
    );
    assert_eq!(
        parallel_report.total_canonical_rows,
        serial_report.total_canonical_rows
    );
    assert_eq!(
        parallel_report.total_nt_catalog_rows,
        serial_report.total_nt_catalog_rows
    );
    let serial_sequences: Vec<u64> = serial_report.records.iter().map(|r| r.sequence).collect();
    let parallel_sequences: Vec<u64> = parallel_report.records.iter().map(|r| r.sequence).collect();
    assert_eq!(
        parallel_sequences, serial_sequences,
        "parallel records assembled in original sequence order"
    );
    assert_eq!(
        parallel_sequences,
        (0..8u64).collect::<Vec<_>>(),
        "all sequences present in order"
    );
    // Check per-record content against sequence-derived expected values so the
    // assertions detect dropped, duplicated, or swapped records. parallel==serial
    // parity alone cannot distinguish those failure modes because both sides
    // would receive the same wrong value.
    for report_records in [&parallel_report.records, &serial_report.records] {
        for rec in report_records {
            assert_eq!(
                rec.canonical_rows,
                100 + rec.sequence,
                "canonical_rows for sequence {} must be {}",
                rec.sequence,
                100 + rec.sequence
            );
            assert_eq!(
                rec.nt_catalog_rows,
                200 + rec.sequence,
                "nt_catalog_rows for sequence {} must be {}",
                rec.sequence,
                200 + rec.sequence
            );
            assert_eq!(
                rec.catalog_hash,
                format!("catalog-hash-{}", rec.sequence),
                "catalog_hash for sequence {} must be catalog-hash-{}",
                rec.sequence,
                rec.sequence
            );
        }
    }
}

#[test]
fn parallel_stop_on_error_returns_lowest_sequence_error() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects: Vec<(u64, Vec<u8>)> = (0..6u64)
        .map(|sequence| {
            (
                sequence,
                format!("synthetic object {sequence}").into_bytes(),
            )
        })
        .collect();
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    // Fail sequences 2 and 4 in the runner; lowest-sequence error (2) must surface.
    let output_dir = temp_dir.path().join("batch-output");
    let result = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(6),
            continue_on_error: false,
            max_concurrent_records: Some(4),
            resume_report: None,
        },
        {
            let objects = objects.clone();
            move || Ok(SequencedFetcher::from_objects(&objects))
        },
        || Ok(FailingRunner::new(vec![2, 4])),
    );
    let err = result.expect_err("stop-on-error returns Err");
    assert!(
        format!("{err:#}").contains("sequence 2"),
        "lowest errored sequence surfaces: {err:#}"
    );
}

#[test]
fn parallel_continue_on_error_collects_failures() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects: Vec<(u64, Vec<u8>)> = (0..6u64)
        .map(|sequence| {
            (
                sequence,
                format!("synthetic object {sequence}").into_bytes(),
            )
        })
        .collect();
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    let output_dir = temp_dir.path().join("batch-output");
    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(6),
            continue_on_error: true,
            max_concurrent_records: Some(4),
            resume_report: None,
        },
        {
            let objects = objects.clone();
            move || Ok(SequencedFetcher::from_objects(&objects))
        },
        || Ok(FailingRunner::new(vec![2, 4])),
    )
    .expect("continue-on-error completes");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    );
    assert_eq!(report.completed_record_count, 4);
    assert_eq!(report.failed_record_count, 2);
    let failed_sequences: Vec<u64> = report.failures.iter().map(|f| f.sequence).collect();
    assert_eq!(failed_sequences, vec![2, 4], "failures in sequence order");
    let completed_sequences: Vec<u64> = report.records.iter().map(|r| r.sequence).collect();
    assert_eq!(
        completed_sequences,
        vec![0, 1, 3, 5],
        "remaining records completed in order"
    );
}

#[test]
fn parallel_duplicate_sha_records_share_cache_and_repair_corrupt_entry() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    // Sequences 0 and 1 pin IDENTICAL bytes — records are not deduplicated by
    // sha, so both map to the same cache entry path. A corrupt entry is
    // planted under that shared sha before the run: whichever worker loses
    // any repair race must still complete its record.
    let shared_bytes = b"shared synthetic object".to_vec();
    let objects = vec![
        (0u64, shared_bytes.clone()),
        (1u64, shared_bytes.clone()),
        (2u64, b"synthetic object two".to_vec()),
        (3u64, b"synthetic object three".to_vec()),
    ];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    let cache_dir = temp_dir.path().join("object-cache");
    fs::create_dir_all(&cache_dir).expect("create cache dir");
    let shared_sha = sha256_hex(&shared_bytes);
    fs::write(cache_dir.join(&shared_sha), b"corrupt cached payload").expect("plant corrupt entry");

    let output_dir = temp_dir.path().join("batch-output");
    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(4),
            continue_on_error: false,
            max_concurrent_records: Some(4),
            resume_report: None,
        },
        {
            let objects = objects.clone();
            let cache_dir = cache_dir.clone();
            move || {
                Ok(CachingSourceUniverseObjectFetcher::new(
                    SequencedFetcher::from_objects(&objects),
                    &cache_dir,
                ))
            }
        },
        || Ok(ConcurrencyRunner::new(None)),
    )
    .expect("duplicate-sha records complete regardless of cache repair races");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::Completed
    );
    assert_eq!(report.completed_record_count, 4);
    assert_eq!(report.failed_record_count, 0);
    assert_eq!(
        fs::read(cache_dir.join(&shared_sha)).expect("read repaired entry"),
        shared_bytes,
        "corrupt shared entry repaired with verified bytes"
    );
}

#[test]
fn resume_into_same_output_dir_fails_loud_before_any_fetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![
        (0u64, b"synthetic object zero".to_vec()),
        (1u64, b"synthetic object one".to_vec()),
    ];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    // Prior partial run writes its report into output_dir.
    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let first_report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("first partial run completes");
    let artifact = write_source_universe_batch_execution_report(&output_dir, &first_report)
        .expect("write prior report");

    // Resuming INTO the same output dir would collide with the clean-write
    // report guard at the end of the run; the contract is rejected up front,
    // before any fetch happens.
    let mut resume_fetcher = SequencedFetcher::from_objects(&objects);
    let resume_calls = resume_fetcher.calls();
    let mut resume_runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(2),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(artifact.path.clone()),
        },
        &mut resume_fetcher,
        &mut resume_runner,
    )
    .expect_err("in-place resume must be rejected");
    assert!(
        format!("{err:#}").contains("resume requires a fresh output dir"),
        "explicit contract error expected, got: {err:#}"
    );
    assert!(
        resume_calls.lock().expect("resume fetch log").is_empty(),
        "rejection happens before any fetch"
    );
}

// ── Pack-pinned control artifacts fail closed before external work ──

#[test]
fn prepare_batch_rejects_tampered_run_spec_before_external_work() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let expected_sha256 = sha256_hex(&fs::read(&fixture.run_spec_path).expect("read run spec"));
    fs::write(&fixture.run_spec_path, "run_id = \"tampered\"\n").expect("tamper run spec");
    let actual_sha256 = sha256_hex(&fs::read(&fixture.run_spec_path).expect("read tampered run spec"));

    let error = pack_preflight_error_before_external_work(&fixture);

    assert_control_artifact_mismatch(
        &error,
        "run_spec",
        &fixture.run_spec_path,
        &expected_sha256,
        &actual_sha256,
    );
}

#[test]
fn prepare_batch_rejects_tampered_accepted_tranche_before_external_work() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let expected_sha256 = sha256_hex(
        &fs::read(&fixture.accepted_tranche_path).expect("read accepted tranche"),
    );
    fs::write(&fixture.accepted_tranche_path, "{\"tampered\":true}\n")
        .expect("tamper accepted tranche");
    let actual_sha256 = sha256_hex(
        &fs::read(&fixture.accepted_tranche_path).expect("read tampered accepted tranche"),
    );

    let error = pack_preflight_error_before_external_work(&fixture);

    assert_control_artifact_mismatch(
        &error,
        "accepted_tranche",
        &fixture.accepted_tranche_path,
        &expected_sha256,
        &actual_sha256,
    );
}

#[test]
fn prepare_batch_rejects_tampered_execution_plan_before_external_work() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let expected_sha256 =
        sha256_hex(&fs::read(&fixture.execution_plan_path).expect("read execution plan"));
    fs::write(&fixture.execution_plan_path, "{\"tampered\":true}\n")
        .expect("tamper execution plan");
    let actual_sha256 = sha256_hex(
        &fs::read(&fixture.execution_plan_path).expect("read tampered execution plan"),
    );

    let error = pack_preflight_error_before_external_work(&fixture);

    assert_control_artifact_mismatch(
        &error,
        "execution_plan",
        &fixture.execution_plan_path,
        &expected_sha256,
        &actual_sha256,
    );
}

#[test]
fn prepare_batch_rejects_missing_control_artifact_before_external_work() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    fs::remove_file(&fixture.accepted_tranche_path).expect("remove accepted tranche");

    let error = pack_preflight_error_before_external_work(&fixture);

    assert!(error.contains("pack record 0"), "error names record: {error}");
    assert!(
        error.contains("source-universe-operator-run-synthetic-00000"),
        "error names operator run: {error}"
    );
    assert!(
        error.contains("accepted_tranche"),
        "error names artifact role: {error}"
    );
    assert!(
        error.contains(&fixture.accepted_tranche_path.display().to_string()),
        "error names artifact path: {error}"
    );
}

// ── Fix 1: path-traversal class — sha256 validation at the consume boundary ──

/// A pack record with a `../`-prefixed sha256 field must be rejected at pack
/// consumption, before any fetch or cache activity.
#[test]
fn prepare_batch_rejects_parent_dir_traversal_sha256() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(
        &pack_path,
        &run_spec_path,
        &execution_plan_path,
        "../etc/passwd",
    );

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect_err("parent-dir traversal sha256 must be rejected");
    assert!(
        format!("{err:#}").contains("selected_object_sha256"),
        "error names the field: {err:#}"
    );
    assert!(
        runner.calls.is_empty(),
        "rejection must happen before any run"
    );
}

/// A pack record with an absolute-path sha256 field must be rejected before
/// any fetch or cache activity.
#[test]
fn prepare_batch_rejects_absolute_path_sha256() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(
        &pack_path,
        &run_spec_path,
        &execution_plan_path,
        "/etc/shadow",
    );

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect_err("absolute-path sha256 must be rejected");
    assert!(
        format!("{err:#}").contains("selected_object_sha256"),
        "error names the field: {err:#}"
    );
    assert!(runner.calls.is_empty(), "rejection before any run");
}

/// A pack record with an uppercase-hex sha256 field must be rejected (the
/// digest produced by `hex::encode(Sha256::digest(...))` is always lowercase;
/// uppercase is a sign of a hand-crafted or tampered value).
#[test]
fn prepare_batch_rejects_uppercase_hex_sha256() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    // 64 chars but uppercase — valid hex encoding but not lowercase sha256.
    let uppercase_sha = "A".repeat(64);
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(
        &pack_path,
        &run_spec_path,
        &execution_plan_path,
        &uppercase_sha,
    );

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect_err("uppercase-hex sha256 must be rejected");
    assert!(
        format!("{err:#}").contains("selected_object_sha256"),
        "error names the field: {err:#}"
    );
    assert!(runner.calls.is_empty(), "rejection before any run");
}

/// A pack record with a 63-char hex sha256 (one char short) must be rejected.
#[test]
fn prepare_batch_rejects_short_hex_sha256() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let short_sha = "a".repeat(63);
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(&pack_path, &run_spec_path, &execution_plan_path, &short_sha);

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect_err("63-char hex sha256 must be rejected");
    assert!(
        format!("{err:#}").contains("selected_object_sha256"),
        "error names the field: {err:#}"
    );
    assert!(runner.calls.is_empty(), "rejection before any run");
}

// ── Fix 2: missing negative test for schema_version validation ──

#[test]
fn resume_schema_version_mismatch_fails_loud() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    let prior_report = SourceUniverseBatchExecutionReport {
        schema_version: "wrong-schema-version.v0".to_string(),
        batch_id: "source-universe-batch-synthetic".to_string(),
        status: SourceUniverseBatchExecutionReportStatus::Completed,
        pack_id: "source-universe-execution-pack-synthetic".to_string(),
        universe_id: "backfill-source-universe-synthetic".to_string(),
        venue: "synthetic-venue".to_string(),
        selected_record_count: 1,
        completed_record_count: 1,
        failed_record_count: 0,
        total_canonical_rows: 7,
        total_nt_catalog_rows: 7,
        records: vec![carried_record_fixture(
            0,
            &sha256_hex(b"synthetic object zero"),
        )],
        failures: vec![],
    };
    let resume_report_path = temp_dir.path().join("prior-report.json");
    fs::write(
        &resume_report_path,
        serde_json::to_vec_pretty(&prior_report).expect("serialize prior report"),
    )
    .expect("write prior report");

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let result = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report_path),
        },
        &mut fetcher,
        &mut runner,
    );
    let err = result.expect_err("schema_version mismatch must fail loud");
    assert!(
        err.to_string().contains("schema_version") || format!("{err:#}").contains("schema_version"),
        "error names the schema_version mismatch: {err:#}"
    );
}

// ── Fix 3: missing negative test for max_concurrent_records == 0 ──

#[test]
fn prepare_batch_rejects_zero_max_concurrent_records() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let run_spec_path = temp_dir.path().join("run-spec.toml");
    let execution_plan_path = temp_dir.path().join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, &objects);

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: Some(0),
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect_err("max_concurrent_records = 0 must be rejected");
    assert!(
        err.to_string().contains("max_concurrent_records")
            || format!("{err:#}").contains("max_concurrent_records"),
        "error names max_concurrent_records: {err:#}"
    );
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

struct SequencedFetcher {
    object_bytes_by_sequence: std::collections::BTreeMap<u64, Vec<u8>>,
    calls: std::sync::Arc<Mutex<Vec<u64>>>,
}

impl SequencedFetcher {
    fn from_objects(objects: &[(u64, Vec<u8>)]) -> Self {
        Self {
            object_bytes_by_sequence: objects.iter().cloned().collect(),
            calls: std::sync::Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> std::sync::Arc<Mutex<Vec<u64>>> {
        std::sync::Arc::clone(&self.calls)
    }
}

impl SourceUniverseObjectFetcher for SequencedFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        self.calls
            .lock()
            .expect("fetch call log")
            .push(record.sequence);
        self.object_bytes_by_sequence
            .get(&record.sequence)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing bytes for sequence {}", record.sequence))
    }
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

struct ValidSingleRecordPack {
    pack_path: PathBuf,
    run_spec_path: PathBuf,
    accepted_tranche_path: PathBuf,
    execution_plan_path: PathBuf,
    output_dir: PathBuf,
}

fn write_valid_single_record_pack(root: &Path) -> ValidSingleRecordPack {
    let pack_path = root.join("source-universe-execution-pack.json");
    let run_spec_path = root.join("run-spec.toml");
    let accepted_tranche_path = root.join("accepted-tranche.json");
    let execution_plan_path = root.join("execution-plan.json");
    fs::write(&run_spec_path, "run_id = \"synthetic-run\"\n").expect("write run spec");
    fs::write(&execution_plan_path, "{}\n").expect("write execution plan");
    write_n_record_pack(
        &pack_path,
        &run_spec_path,
        &execution_plan_path,
        &[(0, b"accepted object bytes".to_vec())],
    );
    ValidSingleRecordPack {
        pack_path,
        run_spec_path,
        accepted_tranche_path,
        execution_plan_path,
        output_dir: root.join("batch-output"),
    }
}

fn pack_preflight_error_before_external_work(fixture: &ValidSingleRecordPack) -> String {
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let error = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    )
    .expect_err("drifted pack-pinned artifact must be rejected");
    assert!(
        runner.calls.is_empty(),
        "pack preflight rejection must happen before operator execution"
    );
    format!("{error:#}")
}

fn assert_control_artifact_mismatch(
    error: &str,
    artifact_role: &str,
    artifact_path: &Path,
    expected_sha256: &str,
    actual_sha256: &str,
) {
    assert!(error.contains("pack record 0"), "error names record: {error}");
    assert!(
        error.contains("source-universe-operator-run-synthetic-00000"),
        "error names operator run: {error}"
    );
    assert!(
        error.contains(artifact_role),
        "error names artifact role: {error}"
    );
    assert!(
        error.contains(&artifact_path.display().to_string()),
        "error names artifact path: {error}"
    );
    assert!(
        error.contains(expected_sha256),
        "error names expected digest: {error}"
    );
    assert!(
        error.contains(actual_sha256),
        "error names actual digest: {error}"
    );
}

/// Repo-relative path to the committed PMXT reference NT catalog run, whose
/// `catalog-metadata.json` records the real `logical_catalog_hash`. Mirrors the
/// src-side `committed_reference_run_dir` so the resume carry-forward gate can be
/// exercised against a genuine catalog instead of a hand-built one.
fn committed_reference_run_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .join(
            "specs/023-nt-research-analytics-platform/reference/\
             pmxt-polymarket-selected-source-conversion/backtests/pmxt-run",
        )
}

/// Recorded logical catalog hash of the committed PMXT reference catalog.
fn committed_reference_catalog_hash() -> String {
    let metadata: serde_json::Value = serde_json::from_slice(
        &fs::read(committed_reference_run_dir().join("catalog-metadata.json"))
            .expect("read committed catalog metadata"),
    )
    .expect("parse committed catalog metadata");
    metadata["catalog_hash"]
        .as_str()
        .expect("catalog_hash present in committed metadata")
        .to_string()
}

/// Recursively copy `src` into `dst`, used to plant the committed reference
/// catalog under a resumed record's `output_dir` without touching the committed
/// fixture.
fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create copy dir");
    for entry in fs::read_dir(src).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir_all(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

fn write_two_record_pack(
    pack_path: &Path,
    run_spec_path: &Path,
    execution_plan_path: &Path,
    first_object_bytes: &[u8],
    second_object_bytes: &[u8],
) {
    write_n_record_pack(
        pack_path,
        run_spec_path,
        execution_plan_path,
        &[
            (0, first_object_bytes.to_vec()),
            (1, second_object_bytes.to_vec()),
        ],
    );
}

/// Synthetic per-sequence symbol so the two-record assertions keep working
/// (sequence 0 = `SYNTHETIC-AAA`, sequence 1 = `SYNTHETIC-BBB`, ...).
fn synthetic_symbol(sequence: u64) -> String {
    let letter = char::from(b'A' + (sequence % 26) as u8);
    format!("SYNTHETIC-{letter}{letter}{letter}")
}

fn write_n_record_pack(
    pack_path: &Path,
    run_spec_path: &Path,
    execution_plan_path: &Path,
    objects: &[(u64, Vec<u8>)],
) {
    let accepted_tranche_path = pack_path
        .parent()
        .expect("pack path has parent")
        .join("accepted-tranche.json");
    fs::write(&accepted_tranche_path, "{}\n").expect("write accepted tranche");
    let run_spec_sha256 = sha256_hex(&fs::read(run_spec_path).expect("read run spec"));
    let accepted_tranche_sha256 = sha256_hex(
        &fs::read(&accepted_tranche_path).expect("read accepted tranche"),
    );
    let execution_plan_sha256 =
        sha256_hex(&fs::read(execution_plan_path).expect("read execution plan"));
    let record_count = objects.len() as u64;
    let total_object_bytes_len: usize = objects.iter().map(|(_, bytes)| bytes.len()).sum();
    let records = objects
        .iter()
        .map(|(sequence, bytes)| {
            format!(
                r#"    {{
      "sequence": {sequence},
      "work_item_id": "synthetic-work-item-{sequence}",
      "operator_run_id": "source-universe-operator-run-synthetic-{sequence:05}",
      "source_binding": "synthetic-spot-tick-trades",
      "category": "spot",
      "symbol": "{symbol}",
      "archive_date": "2026-03-01",
      "source_uri": "s3://synthetic-bucket/raw/synthetic-{sequence}.csv.gz",
      "source_url": "https://public.synthetic.example/object-{sequence}.csv.gz",
      "selected_object_sha256": "{sha256}",
      "selected_object_bytes": {bytes_len},
      "source_proof_id": "source-proof-synthetic",
      "source_proof_version": 1,
      "accepted_tranche_id": "accepted-tranche-synthetic-{sequence}",
      "output_prefix": "s3://synthetic-bucket/nt-research-analytics/backtests/synthetic-{sequence}",
      "run_spec_path": "{run_spec_path}",
      "run_spec_sha256": "{run_spec_sha256}",
      "accepted_tranche_path": "{accepted_tranche_path}",
      "accepted_tranche_sha256": "{accepted_tranche_sha256}",
      "execution_plan_path": "{execution_plan_path}",
      "execution_plan_sha256": "{execution_plan_sha256}"
    }}"#,
                symbol = synthetic_symbol(*sequence),
                sha256 = sha256_hex(bytes),
                bytes_len = bytes.len(),
                run_spec_path = run_spec_path.display(),
                run_spec_sha256 = run_spec_sha256,
                accepted_tranche_path = accepted_tranche_path.display(),
                accepted_tranche_sha256 = accepted_tranche_sha256,
                execution_plan_path = execution_plan_path.display(),
                execution_plan_sha256 = execution_plan_sha256,
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    fs::write(
        pack_path,
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
  "venue": "synthetic-venue",
  "source": "public_archive",
  "family": "tick_trades",
  "table_family": "trades",
  "planned_object_count": {record_count},
  "executable_record_count": {record_count},
  "withheld_record_count": 0,
  "selected_record_count": {record_count},
  "materialized_record_count": {record_count},
  "skipped_executable_record_count": 0,
  "executable_source_bytes": {total_object_bytes_len},
  "materialized_source_bytes": {total_object_bytes_len},
  "artifact_refs": [],
  "records": [
{records}
  ],
  "blocking_reasons": []
}}"#,
        ),
    )
    .expect("write n-record pack");
}

/// Build a synthetic execution-pack record with the pinned sha/length for the
/// given object bytes. Used by the cache fetcher tests, which exercise the
/// fetcher directly without a full pack.
fn synthetic_record(
    sequence: u64,
    object_bytes: &[u8],
    source_url: &str,
) -> SourceUniverseExecutionPackRecord {
    SourceUniverseExecutionPackRecord {
        sequence,
        work_item_id: format!("synthetic-work-item-{sequence}"),
        operator_run_id: format!("source-universe-operator-run-synthetic-{sequence:05}"),
        source_binding: "synthetic-spot-tick-trades".to_string(),
        category: "spot".to_string(),
        symbol: synthetic_symbol(sequence),
        archive_date: "2026-03-01".to_string(),
        source_uri: format!("s3://synthetic-bucket/raw/synthetic-{sequence}.csv.gz"),
        source_url: source_url.to_string(),
        selected_object_sha256: sha256_hex(object_bytes),
        selected_object_bytes: object_bytes.len() as u64,
        source_proof_id: "source-proof-synthetic".to_string(),
        source_proof_version: 1,
        accepted_tranche_id: format!("accepted-tranche-synthetic-{sequence}"),
        output_prefix: format!(
            "s3://synthetic-bucket/nt-research-analytics/backtests/synthetic-{sequence}"
        ),
        run_spec_path: std::path::PathBuf::from("run-spec.toml"),
        run_spec_sha256: "run-spec-sha".to_string(),
        accepted_tranche_path: std::path::PathBuf::from("accepted-tranche.json"),
        accepted_tranche_sha256: "accepted-tranche-sha".to_string(),
        execution_plan_path: std::path::PathBuf::from("execution-plan.json"),
        execution_plan_sha256: "execution-plan-sha".to_string(),
    }
}

/// A prior successful report record for the resume tests, with a chosen sha so
/// the pack-regeneration guard can be exercised both ways.
fn carried_record_fixture(
    sequence: u64,
    selected_object_sha256: &str,
) -> backtesting_vertical_slice::source_universe_batch_execution::SourceUniverseBatchExecutionRecord
{
    backtesting_vertical_slice::source_universe_batch_execution::SourceUniverseBatchExecutionRecord {
        sequence,
        operator_run_id: format!("source-universe-operator-run-synthetic-{sequence:05}"),
        source_binding: "synthetic-spot-tick-trades".to_string(),
        category: "spot".to_string(),
        symbol: synthetic_symbol(sequence),
        archive_date: "2026-03-01".to_string(),
        selected_object_sha256: selected_object_sha256.to_string(),
        selected_object_bytes: 0,
        canonical_rows: 7,
        nt_catalog_rows: 7,
        catalog_hash: "carried-catalog-hash".to_string(),
        output_dir: std::path::PathBuf::from("prior-output-dir"),
    }
}

/// A prior failure entry for the resume tests, which must NOT be carried.
fn failure_record_fixture(
    sequence: u64,
    selected_object_sha256: &str,
) -> backtesting_vertical_slice::source_universe_batch_execution::SourceUniverseBatchExecutionFailureRecord{
    backtesting_vertical_slice::source_universe_batch_execution::SourceUniverseBatchExecutionFailureRecord {
        sequence,
        operator_run_id: format!("source-universe-operator-run-synthetic-{sequence:05}"),
        source_binding: "synthetic-spot-tick-trades".to_string(),
        category: "spot".to_string(),
        symbol: synthetic_symbol(sequence),
        archive_date: "2026-03-01".to_string(),
        selected_object_sha256: selected_object_sha256.to_string(),
        selected_object_bytes: 0,
        failure_stage: "run_operator".to_string(),
        error: "synthetic prior failure".to_string(),
    }
}

/// Inner fetcher for the caching tests: serves pinned bytes by sequence and
/// counts how many times it was actually invoked.
struct CountingFetcher {
    object_bytes_by_sequence: std::collections::BTreeMap<u64, Vec<u8>>,
    calls: std::sync::Arc<AtomicUsize>,
}

impl CountingFetcher {
    fn new(objects: Vec<(u64, Vec<u8>)>) -> Self {
        Self {
            object_bytes_by_sequence: objects.into_iter().collect(),
            calls: std::sync::Arc::new(AtomicUsize::new(0)),
        }
    }

    fn calls(&self) -> std::sync::Arc<AtomicUsize> {
        std::sync::Arc::clone(&self.calls)
    }
}

impl SourceUniverseObjectFetcher for CountingFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.object_bytes_by_sequence
            .get(&record.sequence)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing bytes for sequence {}", record.sequence))
    }
}

/// Shared concurrency probe: tracks active workers and the high-water mark.
struct ConcurrencyProbe {
    high_water: std::sync::Arc<AtomicUsize>,
    active: std::sync::Arc<AtomicUsize>,
}

/// Runner that optionally reports its observed concurrency. With a probe it
/// sleeps briefly so concurrent workers overlap; without one it is a plain
/// deterministic runner used for the serial baseline.
struct ConcurrencyRunner {
    probe: Option<ConcurrencyProbe>,
}

impl ConcurrencyRunner {
    fn new(probe: Option<ConcurrencyProbe>) -> Self {
        Self { probe }
    }
}

impl SourceUniverseOperatorRunner for ConcurrencyRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        _object_bytes: &[u8],
        _run_spec_path: &Path,
        _execution_plan_path: &Path,
        _output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        if let Some(probe) = &self.probe {
            let now = probe.active.fetch_add(1, Ordering::SeqCst) + 1;
            probe.high_water.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20));
            probe.active.fetch_sub(1, Ordering::SeqCst);
        }
        // Return per-record values derived from sequence so per-record assertions
        // are discriminating: swapping or duplicating records would be caught.
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 100 + record.sequence,
            nt_catalog_rows: 200 + record.sequence,
            catalog_hash: format!("catalog-hash-{}", record.sequence),
        })
    }
}

/// Runner that fails the named sequences and succeeds on every other one.
struct FailingRunner {
    failing_sequences: Vec<u64>,
}

impl FailingRunner {
    fn new(failing_sequences: Vec<u64>) -> Self {
        Self { failing_sequences }
    }
}

impl SourceUniverseOperatorRunner for FailingRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        _object_bytes: &[u8],
        _run_spec_path: &Path,
        _execution_plan_path: &Path,
        _output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        if self.failing_sequences.contains(&record.sequence) {
            anyhow::bail!("synthetic runner failure for sequence {}", record.sequence);
        }
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: "catalog-hash".to_string(),
        })
    }
}

/// Fetcher used by the sha256-validation rejection tests. Its `fetch` panics
/// if called, proving that validation fires before any fetch activity.
struct NeverFetcher;

impl SourceUniverseObjectFetcher for NeverFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        panic!(
            "NeverFetcher called for sequence {} — validation should have rejected the pack first",
            record.sequence
        );
    }
}

/// Write a single-record pack whose `selected_object_sha256` is set to the
/// given literal string (without computing a real digest). Used by the
/// sha256-field rejection tests to inject invalid values.
fn write_pack_with_sha256(
    pack_path: &Path,
    run_spec_path: &Path,
    execution_plan_path: &Path,
    sha256_literal: &str,
) {
    // Use a placeholder byte count; validation rejects the sha256 field before
    // byte-count checks are ever reached.
    fs::write(
        pack_path,
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
  "venue": "synthetic-venue",
  "source": "public_archive",
  "family": "tick_trades",
  "table_family": "trades",
  "planned_object_count": 1,
  "executable_record_count": 1,
  "withheld_record_count": 0,
  "selected_record_count": 1,
  "materialized_record_count": 1,
  "skipped_executable_record_count": 0,
  "executable_source_bytes": 1,
  "materialized_source_bytes": 1,
  "artifact_refs": [],
  "records": [
    {{
      "sequence": 0,
      "work_item_id": "synthetic-work-item-0",
      "operator_run_id": "source-universe-operator-run-synthetic-00000",
      "source_binding": "synthetic-spot-tick-trades",
      "category": "spot",
      "symbol": "SYNTHETIC-AAA",
      "archive_date": "2026-03-01",
      "source_uri": "s3://synthetic-bucket/raw/synthetic-0.csv.gz",
      "source_url": "https://public.synthetic.example/object-0.csv.gz",
      "selected_object_sha256": "{sha256_literal}",
      "selected_object_bytes": 1,
      "source_proof_id": "source-proof-synthetic",
      "source_proof_version": 1,
      "accepted_tranche_id": "accepted-tranche-synthetic-0",
      "output_prefix": "s3://synthetic-bucket/nt-research-analytics/backtests/synthetic-0",
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
            run_spec_path = run_spec_path.display(),
            execution_plan_path = execution_plan_path.display(),
        ),
    )
    .expect("write pack with literal sha256");
}
