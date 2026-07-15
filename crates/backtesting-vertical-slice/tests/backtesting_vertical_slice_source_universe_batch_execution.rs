use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use backtesting_vertical_slice::{
    backfill_accepted_tranche::BackfillAcceptedTrancheManifest,
    backfill_execution_plan::{
        BackfillExecutionPlan, BackfillExecutionPlanStatus, BackfillExecutionRunBinding,
        BackfillExecutionWorkBudget, evaluate_backfill_execution_plan,
    },
    operator::RunSpec,
    source_proof::SourceBindingRegistry,
    source_universe_batch_execution::{
        CachingSourceUniverseObjectFetcher, HttpSourceUniverseObjectFetcher,
        LocalSourceUniverseOperatorRunner, SourceUniverseBatchExecutionConfig,
        SourceUniverseBatchExecutionReport, SourceUniverseBatchExecutionReportStatus,
        SourceUniverseBatchExecutionRunOutput, SourceUniverseObjectFetcher,
        SourceUniverseOperatorRunner, SourceUniverseVerifiedControlArtifacts,
        execute_source_universe_batch, execute_source_universe_batch_with_config,
        execute_source_universe_batch_with_factories, write_source_universe_batch_execution_report,
    },
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackRecord,
        SourceUniverseExecutionPackStatus,
    },
};
use flate2::{Compression, write::GzEncoder};

#[test]
fn source_universe_batch_execution_fetches_verifies_and_runs_pack_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let pack: SourceUniverseExecutionPack = serde_json::from_slice(
        &fs::read(&fixture.pack_path).expect("read typed execution pack fixture"),
    )
    .expect("parse typed execution pack fixture");
    let record = pack.records.first().expect("fixture has record zero");
    let run_spec_path = fixture.run_spec_path.clone();
    let accepted_tranche_path = fixture.accepted_tranche_path.clone();
    let execution_plan_path = fixture.execution_plan_path.clone();
    let output_dir = fixture.output_dir.clone();
    let object_bytes = b"accepted object bytes";
    let run_spec_sha256 = record.run_spec_sha256.clone();
    let accepted_tranche_sha256 = record.accepted_tranche_sha256.clone();
    let execution_plan_sha256 = record.execution_plan_sha256.clone();

    let mut fetcher = StaticFetcher {
        expected_source_url: record.source_url.clone(),
        object_bytes: object_bytes.to_vec(),
        calls: 0,
    };
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
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
    assert_eq!(report.records[0].run_spec_sha256, run_spec_sha256);
    assert_eq!(
        report.records[0].accepted_tranche_sha256,
        accepted_tranche_sha256
    );
    assert_eq!(
        report.records[0].execution_plan_sha256,
        execution_plan_sha256
    );
    assert_eq!(fetcher.calls, 1);
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(
        runner.calls[0].operator_run_id,
        "source-universe-operator-run-synthetic-00000"
    );
    assert_eq!(runner.calls[0].object_bytes, object_bytes);
    assert_eq!(runner.calls[0].run_spec_path, run_spec_path);
    assert_eq!(
        runner.calls[0].run_spec_bytes,
        fs::read(&run_spec_path).expect("read run spec assertion")
    );
    assert_eq!(runner.calls[0].accepted_tranche_path, accepted_tranche_path);
    assert_eq!(
        runner.calls[0].accepted_tranche_bytes,
        fs::read(&accepted_tranche_path).expect("read accepted tranche assertion")
    );
    assert_eq!(runner.calls[0].execution_plan_path, execution_plan_path);
    assert_eq!(
        runner.calls[0].execution_plan_bytes,
        fs::read(&execution_plan_path).expect("read execution plan assertion")
    );
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
    assert_eq!(
        report.records[0].symbol.as_str(),
        committed_record_zero_controls().record.symbol.as_str(),
        "windowing preserves the committed control identity"
    );
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
fn continue_on_error_records_control_preflight_failure_and_runs_later_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0u64, b"synthetic object zero".to_vec()),
        (1u64, b"synthetic object one".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "accepted_tranche_path",
        serde_json::Value::String("missing-record-zero-tranche.json".to_string()),
    );

    let mut fetcher = SequencedFetcher::from_objects(&[(1, objects[1].1.clone())]);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();
    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
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
    .expect("bad control artifact is isolated to its record");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    );
    assert_eq!(report.selected_record_count, 2);
    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.failed_record_count, 1);
    assert_eq!(report.failures[0].sequence, 0);
    assert_eq!(report.failures[0].failure_stage, "verify_control_artifacts");
    assert!(
        report.failures[0]
            .error
            .contains("missing-record-zero-tranche.json"),
        "failure retains the rejected control path: {}",
        report.failures[0].error
    );
    assert_eq!(
        fetch_calls.lock().expect("fetch log").as_slice(),
        &[1u64],
        "the bad record is not fetched and the later valid record still runs"
    );
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
        records: vec![carried_record_fixture(0, &"f".repeat(64))],
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
fn resume_rechecks_carried_catalog_after_an_earlier_record_mutates_it() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"fresh object zero".to_vec()),
        (1, b"initially carried object one".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let prior_output = temp_dir.path().join("prior-output");
    let mut prior_fetcher = SequencedFetcher::from_objects(&[(1, objects[1].1.clone())]);
    let mut prior_runner = RecordingRunner::default();
    let mut prior_report = execute_source_universe_batch_with_config(
        "source-universe-batch-prior",
        &fixture.pack_path,
        &prior_output,
        SourceUniverseBatchExecutionConfig {
            start_sequence: Some(1),
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut prior_fetcher,
        &mut prior_runner,
    )
    .expect("create prior sequence-one report");
    let carried_output_dir = prior_report.records[0].output_dir.clone();
    let carried_catalog_dir = carried_output_dir.join("nt-catalog");
    copy_dir_all(
        &committed_reference_run_dir().join("nt-catalog"),
        &carried_catalog_dir,
    );
    prior_report.records[0].catalog_hash = committed_reference_catalog_hash();
    let prior_artifact = write_source_universe_batch_execution_report(&prior_output, &prior_report)
        .expect("write prior sequence-one report");
    let catalog_file_to_mutate = first_regular_file(&carried_catalog_dir.join("data"));

    let mut resume_fetcher = SequencedFetcher::from_objects(&objects);
    let resume_calls = resume_fetcher.calls();
    let mut resume_runner = CatalogMutatingRunner {
        catalog_file_to_mutate,
        calls: Vec::new(),
    };
    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-resume",
        &fixture.pack_path,
        &temp_dir.path().join("resume-output"),
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(2),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(prior_artifact.path),
        },
        &mut resume_fetcher,
        &mut resume_runner,
    )
    .expect("drifted carried catalog falls through to fresh work");

    assert_eq!(report.completed_record_count, 2);
    assert_eq!(
        resume_calls.lock().expect("resume fetch calls").as_slice(),
        &[0, 1],
        "sequence one must be re-resolved after sequence zero mutates its carried catalog"
    );
    assert_eq!(resume_runner.calls, vec![0, 1]);
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
                sha256_hex(format!("catalog-hash-{}", rec.sequence).as_bytes()),
                "catalog_hash for sequence {} must bind its deterministic catalog bytes",
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
    let actual_sha256 =
        sha256_hex(&fs::read(&fixture.run_spec_path).expect("read tampered run spec"));

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
    let expected_sha256 =
        sha256_hex(&fs::read(&fixture.accepted_tranche_path).expect("read accepted tranche"));
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
    let actual_sha256 =
        sha256_hex(&fs::read(&fixture.execution_plan_path).expect("read tampered execution plan"));

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
fn operator_receives_verified_control_bytes_when_source_path_changes_during_fetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let verified_run_spec = fs::read(&fixture.run_spec_path).expect("read pinned run spec");
    let replacement_run_spec = b"run_id = \"mutated-during-fetch\"\n".to_vec();
    let mut fetcher = MutatingControlArtifactFetcher {
        object_bytes: b"accepted object bytes".to_vec(),
        artifact_path: fixture.run_spec_path.clone(),
        replacement_bytes: replacement_run_spec.clone(),
    };
    let mut runner = RecordingRunner::default();

    execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    )
    .expect("verified control bytes remain bound through source fetch");

    assert_eq!(runner.calls.len(), 1);
    assert_eq!(runner.calls[0].run_spec_bytes, verified_run_spec);
    assert_eq!(
        fs::read(&fixture.run_spec_path).expect("read mutated source path"),
        replacement_run_spec,
        "test proves the path changed after verification"
    );
}

#[test]
fn operator_uses_verified_source_registry_when_registry_path_changes_during_fetch() {
    let root = repo_root();
    let target_dir = root.join("target");
    fs::create_dir_all(&target_dir).expect("create repo target dir");
    let temp_dir = tempfile::Builder::new()
        .prefix("source-registry-mutation-")
        .tempdir_in(&target_dir)
        .expect("repo-relative temp dir");
    let object_bytes = valid_bybit_trade_object();
    let fixture = write_valid_pack(temp_dir.path(), &[(0, object_bytes.clone())]);
    let registry_path = temp_dir.path().join("source-bindings.toml");
    let committed_registry_bytes = fs::read(root.join(
        "specs/023-nt-research-analytics-platform/reference/\
         backfill-source-bindings.v1.toml",
    ))
    .expect("read committed source-binding registry");
    let distinct_source_binding = "test-retained-bybit-inverse-tick-trades";
    let retained_registry_bytes = source_binding_registry_with_distinct_key(
        &committed_registry_bytes,
        &committed_record_zero_controls()
            .run_spec
            .source_proof
            .source_binding,
        distinct_source_binding,
    );
    fs::write(&registry_path, &retained_registry_bytes)
        .expect("write distinct retained source-binding registry");
    let registry_identity = registry_path
        .strip_prefix(&root)
        .expect("registry temp path is repo-relative")
        .to_path_buf();
    rewrite_control_triple_and_regenerate_execution_plan(
        &fixture,
        0,
        |run_spec, accepted_tranche| {
            run_spec.source_bindings_path = registry_identity;
            run_spec.source_proof.source_binding = distinct_source_binding.to_string();
            run_spec.manifest.venue_binding_key = distinct_source_binding.to_string();
            accepted_tranche.source_binding = distinct_source_binding.to_string();
        },
    );
    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "source_binding",
        serde_json::Value::String(distinct_source_binding.to_string()),
    );
    let replacement_registry = committed_registry_bytes;
    let mut fetcher = MutatingControlArtifactFetcher {
        object_bytes,
        artifact_path: registry_path.clone(),
        replacement_bytes: replacement_registry.clone(),
    };
    let mut runner = LocalSourceUniverseOperatorRunner;

    let report = execute_source_universe_batch(
        "source-universe-batch-registry-mutation",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    )
    .expect("operator consumes the registry verified before source fetch");

    assert_eq!(report.completed_record_count, 1);
    assert_eq!(
        fs::read(&registry_path).expect("read mutated registry"),
        replacement_registry,
        "test proves the registry changed to valid bytes that do not authorize the distinct binding"
    );
}

#[test]
fn prepare_batch_rejects_missing_control_artifact_before_external_work() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let expected_sha256 =
        sha256_hex(&fs::read(&fixture.accepted_tranche_path).expect("read accepted tranche"));
    fs::remove_file(&fixture.accepted_tranche_path).expect("remove accepted tranche");

    let error = pack_preflight_error_before_external_work(&fixture);

    assert!(
        error.contains("pack record 0"),
        "error names record: {error}"
    );
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
    assert!(
        error.contains(&expected_sha256),
        "error names pinned expected digest: {error}"
    );
}

#[test]
fn record_limit_does_not_require_unselected_control_artifacts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let selected_object = b"selected object bytes".to_vec();
    let fixture = write_valid_pack(
        temp_dir.path(),
        &[
            (0, selected_object.clone()),
            (1, b"unselected object bytes".to_vec()),
        ],
    );
    rewrite_pack_record_field(
        &fixture.pack_path,
        1,
        "accepted_tranche_path",
        serde_json::Value::String("missing-unselected-tranche.json".to_string()),
    );

    let mut fetcher = SequencedFetcher::from_objects(&[(0, selected_object)]);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();
    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    )
    .expect("selected record executes without evicted unselected artifacts");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::Completed
    );
    assert_eq!(*fetch_calls.lock().expect("fetch calls"), vec![0]);
    assert_eq!(runner.calls.len(), 1);
}

#[test]
fn prepare_batch_rejects_malformed_control_artifact_sha256_before_external_work() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let malformed_sha256 = "A".repeat(64);
    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "run_spec_sha256",
        serde_json::Value::String(malformed_sha256.clone()),
    );

    let error = pack_preflight_error_before_external_work(&fixture);

    assert!(
        error.contains("pack record 0"),
        "error names record: {error}"
    );
    assert!(
        error.contains("source-universe-operator-run-synthetic-00000"),
        "error names operator run: {error}"
    );
    assert!(
        error.contains("run_spec_sha256"),
        "error names malformed field: {error}"
    );
    assert!(
        error.contains(&malformed_sha256),
        "error names malformed digest: {error}"
    );
}

#[test]
fn prepare_batch_rejects_malformed_typed_controls_before_fetch() {
    for (role, sha256_field, malformed_bytes) in [
        ("run_spec", "run_spec_sha256", b"[[[".as_slice()),
        (
            "accepted_tranche",
            "accepted_tranche_sha256",
            b"{}".as_slice(),
        ),
        ("execution_plan", "execution_plan_sha256", b"{}".as_slice()),
    ] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let fixture = write_valid_single_record_pack(temp_dir.path());
        let artifact_path = match role {
            "run_spec" => &fixture.run_spec_path,
            "accepted_tranche" => &fixture.accepted_tranche_path,
            "execution_plan" => &fixture.execution_plan_path,
            _ => unreachable!("all malformed control roles are enumerated"),
        };
        fs::write(artifact_path, malformed_bytes).expect("write malformed typed control");
        repin_pack_record_artifact(&fixture.pack_path, 0, sha256_field, artifact_path);
        let objects = vec![(0, b"accepted object bytes".to_vec())];
        let mut fetcher = SequencedFetcher::from_objects(&objects);
        let fetch_calls = fetcher.calls();
        let mut runner = RecordingRunner::default();

        let result = execute_source_universe_batch(
            "source-universe-batch-synthetic",
            &fixture.pack_path,
            &fixture.output_dir,
            Some(1),
            &mut fetcher,
            &mut runner,
        );

        assert!(
            fetch_calls.lock().expect("fetch calls").is_empty(),
            "malformed {role} must be rejected before fetch; result: {result:?}"
        );
        let error = result.expect_err("malformed typed control must fail preflight");
        assert!(
            format!("{error:#}").contains(role),
            "error names malformed {role}: {error:#}"
        );
    }
}

#[test]
fn prepare_batch_rejects_cross_artifact_control_drift_before_fetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let mut run_spec: RunSpec =
        toml::from_str(&fs::read_to_string(&fixture.run_spec_path).expect("read valid run spec"))
            .expect("parse valid run spec");
    run_spec.manifest.run_id = "semantically-drifted-operator-run".to_string();
    fs::write(
        &fixture.run_spec_path,
        toml::to_string_pretty(&run_spec).expect("serialize drifted run spec"),
    )
    .expect("write drifted run spec");
    repin_pack_record_artifact(
        &fixture.pack_path,
        0,
        "run_spec_sha256",
        &fixture.run_spec_path,
    );
    let objects = vec![(0, b"accepted object bytes".to_vec())];
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    );

    assert!(
        fetch_calls.lock().expect("fetch calls").is_empty(),
        "cross-artifact run-id/hash drift must be rejected before fetch; result: {result:?}"
    );
    result.expect_err("semantically inconsistent typed controls must fail preflight");
}

#[test]
fn prepare_batch_rejects_absolute_and_parent_control_path_escape_before_fetch() {
    for escape_kind in ["absolute", "parent"] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let pack_dir = temp_dir.path().join("pack");
        fs::create_dir_all(&pack_dir).expect("create pack dir");
        let fixture = write_valid_single_record_pack(&pack_dir);
        let outside_run_spec = temp_dir.path().join("outside-run-spec.toml");
        fs::copy(&fixture.run_spec_path, &outside_run_spec).expect("copy outside run spec");
        let declared_path = if escape_kind == "absolute" {
            outside_run_spec.display().to_string()
        } else {
            "../outside-run-spec.toml".to_string()
        };
        rewrite_pack_record_field(
            &fixture.pack_path,
            0,
            "run_spec_path",
            serde_json::Value::String(declared_path),
        );
        let objects = vec![(0, b"accepted object bytes".to_vec())];
        let mut fetcher = SequencedFetcher::from_objects(&objects);
        let fetch_calls = fetcher.calls();
        let mut runner = RecordingRunner::default();

        let result = execute_source_universe_batch(
            "source-universe-batch-synthetic",
            &fixture.pack_path,
            &fixture.output_dir,
            Some(1),
            &mut fetcher,
            &mut runner,
        );

        assert!(
            fetch_calls.lock().expect("fetch calls").is_empty(),
            "{escape_kind} control path escape must be rejected before fetch; result: {result:?}"
        );
        result.expect_err("escaped control path must fail preflight");
    }
}

#[cfg(unix)]
#[test]
fn prepare_batch_rejects_symlink_control_path_escape_before_fetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let pack_dir = temp_dir.path().join("pack");
    fs::create_dir_all(&pack_dir).expect("create pack dir");
    let fixture = write_valid_single_record_pack(&pack_dir);
    let outside_run_spec = temp_dir.path().join("outside-run-spec.toml");
    fs::copy(&fixture.run_spec_path, &outside_run_spec).expect("copy outside run spec");
    let symlink_path = pack_dir.join("linked-run-spec.toml");
    std::os::unix::fs::symlink(&outside_run_spec, &symlink_path)
        .expect("create escaping control symlink");
    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "run_spec_path",
        serde_json::Value::String("linked-run-spec.toml".to_string()),
    );
    let objects = vec![(0, b"accepted object bytes".to_vec())];
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    );

    assert!(
        fetch_calls.lock().expect("fetch calls").is_empty(),
        "symlink control path escape must be rejected before fetch; result: {result:?}"
    );
    result.expect_err("symlink-escaped control path must fail preflight");
}

#[test]
fn prepare_batch_rejects_escaping_operator_run_ids_before_fetch() {
    for operator_run_id in [
        "/tmp/absolute-operator-run".to_string(),
        "../parent-operator-run".to_string(),
    ] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let fixture = write_valid_single_record_pack(temp_dir.path());
        rewrite_control_triple_and_regenerate_execution_plan(&fixture, 0, |run_spec, _| {
            run_spec.manifest.run_id.clone_from(&operator_run_id);
        });
        rewrite_pack_record_field(
            &fixture.pack_path,
            0,
            "operator_run_id",
            serde_json::Value::String(operator_run_id.clone()),
        );
        let objects = vec![(0, b"accepted object bytes".to_vec())];
        let mut fetcher = SequencedFetcher::from_objects(&objects);
        let fetch_calls = fetcher.calls();
        let mut runner = RecordingRunner::default();

        let result = execute_source_universe_batch(
            "source-universe-batch-synthetic",
            &fixture.pack_path,
            &fixture.output_dir,
            Some(1),
            &mut fetcher,
            &mut runner,
        );

        assert!(
            fetch_calls.lock().expect("fetch calls").is_empty(),
            "escaping operator_run_id {operator_run_id:?} must be rejected before fetch; result: {result:?}"
        );
        result.expect_err("escaping operator_run_id must fail preflight");
    }
}

#[test]
fn prepare_batch_rejects_duplicate_operator_run_ids_before_fetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"accepted object zero".to_vec()),
        (1, b"accepted object one".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&fixture.pack_path).expect("read duplicate-id fixture"))
            .expect("parse duplicate-id fixture");
    let duplicate_operator_run_id = pack.records[0].operator_run_id.clone();
    rewrite_control_triple_and_regenerate_execution_plan(&fixture, 1, |run_spec, _| {
        run_spec
            .manifest
            .run_id
            .clone_from(&duplicate_operator_run_id);
    });
    rewrite_pack_record_field(
        &fixture.pack_path,
        1,
        "operator_run_id",
        serde_json::Value::String(duplicate_operator_run_id),
    );
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(2),
        &mut fetcher,
        &mut runner,
    );

    assert!(
        fetch_calls.lock().expect("fetch calls").is_empty(),
        "duplicate operator_run_id values must be rejected before fetch; result: {result:?}"
    );
    result.expect_err("duplicate operator_run_id values must fail preflight");
}

#[test]
fn prepare_batch_rejects_pack_scope_and_record_control_drift_before_fetch() {
    for (scope, mutate) in [
        ("schema", ("schema_version", "unsupported-pack-schema")),
        ("venue", ("venue", "different-venue")),
        ("universe", ("universe_id", "different-universe")),
        ("table_family", ("table_family", "different-family")),
    ] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let fixture = write_valid_single_record_pack(temp_dir.path());
        rewrite_pack_field(
            &fixture.pack_path,
            mutate.0,
            serde_json::Value::String(mutate.1.to_string()),
        );
        let objects = vec![(0, b"accepted object bytes".to_vec())];
        let mut fetcher = SequencedFetcher::from_objects(&objects);
        let fetch_calls = fetcher.calls();
        let mut runner = RecordingRunner::default();

        let result = execute_source_universe_batch(
            "source-universe-batch-synthetic",
            &fixture.pack_path,
            &fixture.output_dir,
            Some(1),
            &mut fetcher,
            &mut runner,
        );

        assert!(
            fetch_calls.lock().expect("fetch calls").is_empty(),
            "{scope} drift must reject before fetch; result: {result:?}"
        );
        result.expect_err("pack scope drift must fail preflight");
    }

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "selected_object_sha256",
        serde_json::Value::String("f".repeat(64)),
    );
    let objects = vec![(0, b"accepted object bytes".to_vec())];
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();
    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    );
    assert!(fetch_calls.lock().expect("fetch calls").is_empty());
    result.expect_err("record/control object drift must fail preflight");
}

#[cfg(unix)]
#[test]
fn prepare_batch_rejects_existing_symlinked_operator_output_before_fetch() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&fixture.pack_path).expect("read execution pack"))
            .expect("parse execution pack");
    fs::create_dir_all(&fixture.output_dir).expect("create output root");
    let outside = temp_dir.path().join("outside-output");
    fs::create_dir_all(&outside).expect("create outside output");
    std::os::unix::fs::symlink(
        &outside,
        fixture.output_dir.join(&pack.records[0].operator_run_id),
    )
    .expect("create escaping operator-output symlink");
    let objects = vec![(0, b"accepted object bytes".to_vec())];
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    );

    assert!(fetch_calls.lock().expect("fetch calls").is_empty());
    result.expect_err("existing symlinked operator output must fail preflight");
}

#[cfg(unix)]
#[test]
fn selected_output_preflight_failure_honors_continue_on_error() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"accepted object zero".to_vec()),
        (1, b"accepted object one".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&fixture.pack_path).expect("read execution pack"))
            .expect("parse execution pack");
    fs::create_dir_all(&fixture.output_dir).expect("create output root");
    let outside = temp_dir.path().join("outside-output");
    fs::create_dir_all(&outside).expect("create outside output");
    std::os::unix::fs::symlink(
        &outside,
        fixture.output_dir.join(&pack.records[0].operator_run_id),
    )
    .expect("create selected output symlink");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
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
    .expect("selected output preflight failure is recorded");

    assert_eq!(report.failed_record_count, 1);
    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.failures[0].sequence, 0);
    assert_eq!(*fetch_calls.lock().expect("fetch calls"), vec![1]);
}

#[cfg(unix)]
#[test]
fn output_symlink_swap_during_fetch_is_rejected_before_runner() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let object_bytes = b"accepted object bytes".to_vec();
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&fixture.pack_path).expect("read execution pack"))
            .expect("parse execution pack");
    let claimed_output = fixture.output_dir.join(&pack.records[0].operator_run_id);
    let outside = temp_dir.path().join("outside-output");
    fs::create_dir_all(&outside).expect("create outside output");
    let mut fetcher = OutputSymlinkSwapFetcher {
        object_bytes,
        claimed_output,
        outside,
    };
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    );

    result.expect_err("output symlink swap must reject before operator runner");
    assert!(runner.calls.is_empty());
}

#[cfg(unix)]
#[test]
fn output_root_replacement_during_fetch_is_rejected_before_runner() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let mut fetcher = OutputRootSwapFetcher {
        object_bytes: b"accepted object bytes".to_vec(),
        output_root: fixture.output_dir.clone(),
        displaced_root: temp_dir.path().join("displaced-output-root"),
    };
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut fetcher,
        &mut runner,
    );

    result.expect_err("output-root replacement must reject before operator runner");
    assert!(runner.calls.is_empty());
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

#[test]
fn prepare_batch_rejects_non_strict_full_pack_sequence_outside_selected_window() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"object zero".to_vec()),
        (2, b"selected object two".to_vec()),
        (1, b"out-of-order object one".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let mut fetcher = SequencedFetcher::from_objects(&[(2, objects[1].1.clone())]);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let error = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: Some(2),
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect_err("the complete pack sequence must be strict before window selection");

    assert!(
        format!("{error:#}").contains("sequence"),
        "error identifies the non-strict sequence: {error:#}"
    );
    assert!(
        fetch_calls.lock().expect("fetch calls").is_empty(),
        "global sequence validation happens before selected work is fetched"
    );
}

#[test]
fn prepare_batch_rejects_duplicate_sequence_outside_selected_window() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"selected object zero".to_vec()),
        (2, b"outside object two".to_vec()),
        (3, b"outside object three".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    rewrite_pack_record_field(
        &fixture.pack_path,
        2,
        "sequence",
        serde_json::Value::Number(2_u64.into()),
    );
    let mut fetcher = SequencedFetcher::from_objects(&[(0, objects[0].1.clone())]);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let error = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
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
    .expect_err("duplicate sequence outside the selected window must fail full-pack validation");

    assert!(
        format!("{error:#}").contains("sequence"),
        "error identifies the duplicate sequence: {error:#}"
    );
    assert!(
        fetch_calls.lock().expect("fetch calls").is_empty(),
        "duplicate full-pack sequence is rejected before selected work is fetched"
    );
}

#[test]
fn prepare_batch_allows_strict_sequence_gaps() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"object zero".to_vec()),
        (2, b"object two after a valid gap".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let fetch_calls = fetcher.calls();
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(2),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: None,
        },
        &mut fetcher,
        &mut runner,
    )
    .expect("strictly increasing sequence gaps are valid");

    assert_eq!(
        report
            .records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_eq!(fetch_calls.lock().expect("fetch calls").as_slice(), &[0, 2]);
}

#[test]
fn resume_reprocesses_when_an_unreported_pack_record_field_drifts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![(0, b"object zero".to_vec())];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let mut initial_fetcher = SequencedFetcher::from_objects(&objects);
    let mut initial_runner = RecordingRunner::default();
    let mut prior_report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut initial_fetcher,
        &mut initial_runner,
    )
    .expect("initial record completes");
    let prior_output_dir = prior_report.records[0].output_dir.clone();
    copy_dir_all(
        &committed_reference_run_dir().join("nt-catalog"),
        &prior_output_dir.join("nt-catalog"),
    );
    prior_report.records[0].catalog_hash = committed_reference_catalog_hash();
    let prior_report_artifact =
        write_source_universe_batch_execution_report(&fixture.output_dir, &prior_report)
            .expect("write prior report");

    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "work_item_id",
        serde_json::Value::String("drifted-work-item-id".to_string()),
    );

    let mut resume_fetcher = SequencedFetcher::from_objects(&objects);
    let resume_calls = resume_fetcher.calls();
    let mut resume_runner = RecordingRunner::default();
    execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic-resume",
        &fixture.pack_path,
        &temp_dir.path().join("resume-output"),
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(prior_report_artifact.path),
        },
        &mut resume_fetcher,
        &mut resume_runner,
    )
    .expect("record drift triggers fresh execution");

    assert_eq!(
        resume_calls.lock().expect("resume fetch calls").as_slice(),
        &[0],
        "a complete-record fingerprint must prevent carrying drifted pack metadata"
    );
    assert_eq!(resume_runner.calls.len(), 1);
}

#[test]
fn resume_does_not_carry_when_fingerprint_only_pack_metadata_drifts() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![(0, b"object zero".to_vec())];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let mut initial_fetcher = SequencedFetcher::from_objects(&objects);
    let mut initial_runner = RecordingRunner::default();
    let mut prior_report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut initial_fetcher,
        &mut initial_runner,
    )
    .expect("initial record completes");
    let prior_output_dir = prior_report.records[0].output_dir.clone();
    copy_dir_all(
        &committed_reference_run_dir().join("nt-catalog"),
        &prior_output_dir.join("nt-catalog"),
    );
    prior_report.records[0].catalog_hash = committed_reference_catalog_hash();
    let prior_report_artifact =
        write_source_universe_batch_execution_report(&fixture.output_dir, &prior_report)
            .expect("write prior report");
    rewrite_pack_field(
        &fixture.pack_path,
        "work_order_id",
        serde_json::Value::String("drifted-work-order".to_string()),
    );

    let mut resume_fetcher = SequencedFetcher::from_objects(&objects);
    let resume_calls = resume_fetcher.calls();
    let mut resume_runner = RecordingRunner::default();
    execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic-resume",
        &fixture.pack_path,
        &temp_dir.path().join("resume-output"),
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(prior_report_artifact.path),
        },
        &mut resume_fetcher,
        &mut resume_runner,
    )
    .expect("fingerprint-only pack drift reexecutes");

    assert_eq!(
        resume_calls.lock().expect("resume fetch calls").as_slice(),
        &[0],
        "fingerprint-only pack metadata drift must reprocess, never carry"
    );
    assert_eq!(resume_runner.calls.len(), 1);
}

#[test]
fn factory_entry_does_not_construct_dependencies_for_only_preflight_failures() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    rewrite_pack_record_field(
        &fixture.pack_path,
        0,
        "run_spec_path",
        serde_json::Value::String("missing-run-spec.toml".to_string()),
    );
    let fetcher_factory_calls = AtomicUsize::new(0);
    let runner_factory_calls = AtomicUsize::new(0);

    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: true,
            max_concurrent_records: Some(4),
            resume_report: None,
        },
        || -> anyhow::Result<NeverFetcher> {
            fetcher_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("fetcher factory must remain inert")
        },
        || -> anyhow::Result<RecordingRunner> {
            runner_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("runner factory must remain inert")
        },
    )
    .expect("inert preflight failure assembles a failure report");

    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::Failed
    );
    assert_eq!(fetcher_factory_calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner_factory_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn factory_entry_does_not_construct_dependencies_for_empty_selection() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let fetcher_factory_calls = AtomicUsize::new(0);
    let runner_factory_calls = AtomicUsize::new(0);

    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-empty-selection",
        &fixture.pack_path,
        &fixture.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: Some(1),
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: Some(4),
            resume_report: None,
        },
        || -> anyhow::Result<NeverFetcher> {
            fetcher_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("empty selection must not construct a fetcher")
        },
        || -> anyhow::Result<RecordingRunner> {
            runner_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("empty selection must not construct a runner")
        },
    )
    .expect("empty selection assembles without dependencies");

    assert_eq!(report.selected_record_count, 0);
    assert_eq!(report.completed_record_count, 0);
    assert_eq!(report.failed_record_count, 0);
    assert_eq!(fetcher_factory_calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner_factory_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn factory_entry_does_not_construct_dependencies_for_valid_carry() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![(0, b"carried object zero".to_vec())];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let mut initial_fetcher = SequencedFetcher::from_objects(&objects);
    let mut initial_runner = RecordingRunner::default();
    let mut prior_report = execute_source_universe_batch(
        "source-universe-batch-prior",
        &fixture.pack_path,
        &fixture.output_dir,
        Some(1),
        &mut initial_fetcher,
        &mut initial_runner,
    )
    .expect("initial record completes");
    let prior_record_output = prior_report.records[0].output_dir.clone();
    copy_dir_all(
        &committed_reference_run_dir().join("nt-catalog"),
        &prior_record_output.join("nt-catalog"),
    );
    prior_report.records[0].catalog_hash = committed_reference_catalog_hash();
    let prior_artifact =
        write_source_universe_batch_execution_report(&fixture.output_dir, &prior_report)
            .expect("write prior report");
    let fetcher_factory_calls = AtomicUsize::new(0);
    let runner_factory_calls = AtomicUsize::new(0);

    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-valid-carry",
        &fixture.pack_path,
        &temp_dir.path().join("resume-output"),
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: Some(4),
            resume_report: Some(prior_artifact.path),
        },
        || -> anyhow::Result<NeverFetcher> {
            fetcher_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("valid carry must not construct a fetcher")
        },
        || -> anyhow::Result<RecordingRunner> {
            runner_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("valid carry must not construct a runner")
        },
    )
    .expect("valid carry assembles without dependencies");

    assert_eq!(report.completed_record_count, 1);
    assert_eq!(report.records[0].output_dir, prior_record_output);
    assert_eq!(fetcher_factory_calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner_factory_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn factory_construction_error_is_a_record_failure_under_continue_on_error() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let fixture = write_valid_single_record_pack(temp_dir.path());
    let fetcher_factory_calls = AtomicUsize::new(0);

    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &fixture.pack_path,
        &fixture.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: true,
            max_concurrent_records: Some(1),
            resume_report: None,
        },
        || -> anyhow::Result<NeverFetcher> {
            fetcher_factory_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("synthetic fetcher factory failure")
        },
        || Ok(RecordingRunner::default()),
    )
    .expect("continue_on_error records dependency construction failure");

    assert_eq!(fetcher_factory_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::Failed
    );
    assert_eq!(report.failed_record_count, 1);
    assert_eq!(report.failures[0].sequence, 0);
    assert!(
        report.failures[0]
            .error
            .contains("synthetic fetcher factory failure"),
        "factory error is retained in the record failure: {:?}",
        report.failures[0]
    );
}

#[test]
fn factory_construction_retries_on_the_next_record_after_a_recorded_failure() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![
        (0, b"factory failure object zero".to_vec()),
        (1, b"factory retry object one".to_vec()),
    ];
    let fixture = write_valid_pack(temp_dir.path(), &objects);
    let fetcher_factory_calls = AtomicUsize::new(0);

    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-factory-retry",
        &fixture.pack_path,
        &fixture.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: None,
            record_limit: Some(2),
            continue_on_error: true,
            max_concurrent_records: Some(1),
            resume_report: None,
        },
        || -> anyhow::Result<SequencedFetcher> {
            if fetcher_factory_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                anyhow::bail!("synthetic first factory failure");
            }
            Ok(SequencedFetcher::from_objects(&objects))
        },
        || Ok(RecordingRunner::default()),
    )
    .expect("later record retries dependency construction");

    assert_eq!(fetcher_factory_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        report.status,
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    );
    assert_eq!(report.failures[0].sequence, 0);
    assert_eq!(report.records[0].sequence, 1);
}

struct StaticFetcher {
    expected_source_url: String,
    object_bytes: Vec<u8>,
    calls: usize,
}

struct MutatingControlArtifactFetcher {
    object_bytes: Vec<u8>,
    artifact_path: PathBuf,
    replacement_bytes: Vec<u8>,
}

impl SourceUniverseObjectFetcher for MutatingControlArtifactFetcher {
    fn fetch(&mut self, _record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        fs::write(&self.artifact_path, &self.replacement_bytes)
            .expect("mutate control artifact during fetch");
        Ok(self.object_bytes.clone())
    }
}

#[cfg(unix)]
struct OutputSymlinkSwapFetcher {
    object_bytes: Vec<u8>,
    claimed_output: PathBuf,
    outside: PathBuf,
}

#[cfg(unix)]
impl SourceUniverseObjectFetcher for OutputSymlinkSwapFetcher {
    fn fetch(&mut self, _record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        fs::remove_dir(&self.claimed_output).expect("remove atomically claimed output directory");
        std::os::unix::fs::symlink(&self.outside, &self.claimed_output)
            .expect("replace claimed output with symlink during fetch");
        Ok(self.object_bytes.clone())
    }
}

#[cfg(unix)]
struct OutputRootSwapFetcher {
    object_bytes: Vec<u8>,
    output_root: PathBuf,
    displaced_root: PathBuf,
}

#[cfg(unix)]
impl SourceUniverseObjectFetcher for OutputRootSwapFetcher {
    fn fetch(&mut self, _record: &SourceUniverseExecutionPackRecord) -> anyhow::Result<Vec<u8>> {
        fs::rename(&self.output_root, &self.displaced_root)
            .expect("displace leased output root during fetch");
        fs::create_dir(&self.output_root).expect("replace leased output root during fetch");
        Ok(self.object_bytes.clone())
    }
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
    run_spec_bytes: Vec<u8>,
    accepted_tranche_path: std::path::PathBuf,
    accepted_tranche_bytes: Vec<u8>,
    execution_plan_path: std::path::PathBuf,
    execution_plan_bytes: Vec<u8>,
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
        control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        self.calls.push(RunCall {
            operator_run_id: record.operator_run_id.clone(),
            object_bytes: object_bytes.to_vec(),
            run_spec_path: control_artifacts.run_spec_path.clone(),
            run_spec_bytes: control_artifacts.run_spec_bytes.to_vec(),
            accepted_tranche_path: control_artifacts.accepted_tranche_path.clone(),
            accepted_tranche_bytes: control_artifacts.accepted_tranche_bytes.to_vec(),
            execution_plan_path: control_artifacts.execution_plan_path.clone(),
            execution_plan_bytes: control_artifacts.execution_plan_bytes.to_vec(),
            output_dir: output_dir.to_path_buf(),
        });
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: sha256_hex(b"catalog-hash"),
        })
    }
}

struct CatalogMutatingRunner {
    catalog_file_to_mutate: PathBuf,
    calls: Vec<u64>,
}

impl SourceUniverseOperatorRunner for CatalogMutatingRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        _object_bytes: &[u8],
        _control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        _output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        self.calls.push(record.sequence);
        if record.sequence == 0 {
            fs::write(
                &self.catalog_file_to_mutate,
                b"mutated carried catalog bytes",
            )
            .expect("mutate later carried catalog");
        }
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: sha256_hex(format!("catalog-hash-{}", record.sequence).as_bytes()),
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(bytes))
}

fn valid_bybit_trade_object() -> Vec<u8> {
    let csv = "timestamp,symbol,side,size,price,tickDirection,trdMatchID,grossValue,homeNotional,foreignNotional,RPI\n\
               1748736000.125,AAVEUSD,Buy,1,100.00,PlusTick,synthetic-trade-1,100,1,100,false\n";
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(csv.as_bytes())
        .expect("write deterministic Bybit CSV gzip");
    encoder
        .finish()
        .expect("finish deterministic Bybit CSV gzip")
}

fn source_binding_registry_with_distinct_key(
    committed_registry_bytes: &[u8],
    committed_key: &str,
    distinct_key: &str,
) -> Vec<u8> {
    let committed_registry_text = std::str::from_utf8(committed_registry_bytes)
        .expect("committed source-binding registry is UTF-8");
    let committed_registry: toml::Table =
        toml::from_str(committed_registry_text).expect("parse committed source-binding registry");
    let committed_bindings = committed_registry
        .get("source_binding")
        .and_then(toml::Value::as_array)
        .expect("committed registry has source bindings");
    let mut distinct_binding = committed_bindings
        .iter()
        .find(|binding| binding.get("key").and_then(toml::Value::as_str) == Some(committed_key))
        .expect("committed record-zero source binding exists")
        .clone();
    distinct_binding
        .as_table_mut()
        .expect("source binding is a TOML table")
        .insert(
            "key".to_string(),
            toml::Value::String(distinct_key.to_string()),
        );
    let mut retained_registry = toml::Table::new();
    retained_registry.insert(
        "source_binding".to_string(),
        toml::Value::Array(vec![distinct_binding]),
    );
    let retained_registry_text =
        toml::to_string(&retained_registry).expect("serialize distinct source-binding registry");
    let retained = SourceBindingRegistry::from_toml_str(&retained_registry_text)
        .expect("distinct source-binding registry parses");
    let committed = SourceBindingRegistry::from_toml_str(committed_registry_text)
        .expect("committed source-binding registry parses");
    let venue = committed_record_zero_controls()
        .run_spec
        .source_proof
        .venue
        .as_str();
    assert!(
        retained
            .source_binding_metadata(distinct_key, venue)
            .is_some(),
        "retained registry authorizes the distinct binding"
    );
    assert!(
        committed
            .source_binding_metadata(distinct_key, venue)
            .is_none(),
        "committed fallback registry must not authorize the distinct binding"
    );
    retained_registry_text.into_bytes()
}

struct ValidSingleRecordPack {
    pack_path: PathBuf,
    run_spec_path: PathBuf,
    accepted_tranche_path: PathBuf,
    execution_plan_path: PathBuf,
    output_dir: PathBuf,
}

struct CommittedRecordZeroControls {
    pack_template: SourceUniverseExecutionPack,
    record: SourceUniverseExecutionPackRecord,
    run_spec: RunSpec,
    accepted_tranche: BackfillAcceptedTrancheManifest,
    execution_plan: BackfillExecutionPlan,
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
}

fn resolve_repo_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root().join(path)
    }
}

fn committed_record_zero_controls() -> &'static CommittedRecordZeroControls {
    static CONTROLS: OnceLock<CommittedRecordZeroControls> = OnceLock::new();

    CONTROLS.get_or_init(|| {
        let pack_path = repo_root().join(
            "specs/023-nt-research-analytics-platform/reference/\
             source-universe-execution-packs/\
             bybit-public-archive-tick-trades-2025-06-01-2026-06-01/\
             execution-pack/source-universe-execution-pack.json",
        );
        let mut pack_template: SourceUniverseExecutionPack = serde_json::from_slice(
            &fs::read(&pack_path).expect("read committed source-universe execution pack"),
        )
        .expect("parse committed source-universe execution pack");
        let record = pack_template
            .records
            .first()
            .expect("committed execution pack has record zero")
            .clone();
        let run_spec: RunSpec = toml::from_str(
            &fs::read_to_string(resolve_repo_path(&record.run_spec_path))
                .expect("read committed record-zero run spec"),
        )
        .expect("parse committed record-zero run spec");
        let accepted_tranche: BackfillAcceptedTrancheManifest = serde_json::from_slice(
            &fs::read(resolve_repo_path(&record.accepted_tranche_path))
                .expect("read committed record-zero accepted tranche"),
        )
        .expect("parse committed record-zero accepted tranche");
        let execution_plan: BackfillExecutionPlan = serde_json::from_slice(
            &fs::read(resolve_repo_path(&record.execution_plan_path))
                .expect("read committed record-zero execution plan"),
        )
        .expect("parse committed record-zero execution plan");
        pack_template.records = Vec::new();

        CommittedRecordZeroControls {
            pack_template,
            record,
            run_spec,
            accepted_tranche,
            execution_plan,
        }
    })
}

fn write_valid_single_record_pack(root: &Path) -> ValidSingleRecordPack {
    write_valid_pack(root, &[(0, b"accepted object bytes".to_vec())])
}

fn write_valid_pack(root: &Path, objects: &[(u64, Vec<u8>)]) -> ValidSingleRecordPack {
    let pack_path = root.join("source-universe-execution-pack.json");
    let run_spec_path = root.join("run-spec.toml");
    let accepted_tranche_path = root.join("accepted-tranche.json");
    let execution_plan_path = root.join("execution-plan.json");
    write_n_record_pack(&pack_path, &run_spec_path, &execution_plan_path, objects);
    ValidSingleRecordPack {
        pack_path,
        run_spec_path,
        accepted_tranche_path,
        execution_plan_path,
        output_dir: root.join("batch-output"),
    }
}

fn rewrite_pack_record_field(
    pack_path: &Path,
    record_index: usize,
    field: &str,
    value: serde_json::Value,
) {
    let mut pack: serde_json::Value =
        serde_json::from_slice(&fs::read(pack_path).expect("read pack")).expect("parse pack");
    pack["records"][record_index][field] = value;
    fs::write(
        pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize rewritten pack"),
    )
    .expect("rewrite pack");
}

fn rewrite_pack_field(pack_path: &Path, field: &str, value: serde_json::Value) {
    let mut pack: serde_json::Value =
        serde_json::from_slice(&fs::read(pack_path).expect("read pack")).expect("parse pack");
    pack[field] = value;
    fs::write(
        pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize rewritten pack"),
    )
    .expect("rewrite pack");
}

fn repin_pack_record_artifact(
    pack_path: &Path,
    record_index: usize,
    sha256_field: &str,
    artifact_path: &Path,
) {
    rewrite_pack_record_field(
        pack_path,
        record_index,
        sha256_field,
        serde_json::Value::String(sha256_hex(
            &fs::read(artifact_path).expect("read rewritten control artifact"),
        )),
    );
}

fn rewrite_control_triple_and_regenerate_execution_plan(
    fixture: &ValidSingleRecordPack,
    record_index: usize,
    mutate: impl FnOnce(&mut RunSpec, &mut BackfillAcceptedTrancheManifest),
) {
    let mut pack: SourceUniverseExecutionPack = serde_json::from_slice(
        &fs::read(&fixture.pack_path).expect("read execution pack for control rewrite"),
    )
    .expect("parse execution pack for control rewrite");
    let pack_dir = fixture.pack_path.parent().expect("pack path has parent");
    let record = pack
        .records
        .get_mut(record_index)
        .expect("fixture has requested record");
    let run_spec_path = pack_dir.join(&record.run_spec_path);
    let accepted_tranche_path = pack_dir.join(&record.accepted_tranche_path);
    let execution_plan_path = pack_dir.join(&record.execution_plan_path);
    let mut run_spec: RunSpec = toml::from_str(
        &fs::read_to_string(&run_spec_path).expect("read run spec for control rewrite"),
    )
    .expect("parse run spec for control rewrite");
    let original_accepted_tranche_bytes =
        fs::read(&accepted_tranche_path).expect("read accepted tranche for control rewrite");
    let mut accepted_tranche: BackfillAcceptedTrancheManifest =
        serde_json::from_slice(&original_accepted_tranche_bytes)
            .expect("parse accepted tranche for control rewrite");
    let previous_plan: BackfillExecutionPlan = serde_json::from_slice(
        &fs::read(&execution_plan_path).expect("read execution plan for control rewrite"),
    )
    .expect("parse execution plan for control rewrite");

    mutate(&mut run_spec, &mut accepted_tranche);
    let run_spec_bytes = toml::to_string_pretty(&run_spec)
        .expect("serialize rewritten run spec")
        .into_bytes();
    let accepted_tranche_bytes =
        serde_json::to_vec_pretty(&accepted_tranche).expect("serialize rewritten accepted tranche");
    let execution_plan = evaluate_backfill_execution_plan(
        previous_plan.plan_id.clone(),
        sha256_hex(&accepted_tranche_bytes),
        &accepted_tranche,
        sha256_hex(&run_spec_bytes),
        &BackfillExecutionRunBinding::from_run_spec(&run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: previous_plan.max_source_rows,
            max_projected_row_groups: previous_plan.max_projected_row_groups,
            max_wall_seconds: previous_plan.max_wall_seconds,
            require_object_selection_metadata: previous_plan.require_object_selection_metadata,
        },
    );
    assert_eq!(
        execution_plan.status,
        BackfillExecutionPlanStatus::Ready,
        "rewritten run spec must retain a ready execution plan"
    );
    let execution_plan_bytes =
        serde_json::to_vec_pretty(&execution_plan).expect("serialize regenerated execution plan");
    fs::write(&run_spec_path, &run_spec_bytes).expect("write rewritten run spec");
    fs::write(&accepted_tranche_path, &accepted_tranche_bytes)
        .expect("write rewritten accepted tranche");
    fs::write(&execution_plan_path, &execution_plan_bytes)
        .expect("write regenerated execution plan");
    record.run_spec_sha256 = sha256_hex(&run_spec_bytes);
    record.accepted_tranche_sha256 = sha256_hex(&accepted_tranche_bytes);
    record.execution_plan_sha256 = sha256_hex(&execution_plan_bytes);
    fs::write(
        &fixture.pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize repinned execution pack"),
    )
    .expect("write repinned execution pack");
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
    assert!(
        error.contains("pack record 0"),
        "error names record: {error}"
    );
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

fn first_regular_file(root: &Path) -> PathBuf {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        let mut entries = fs::read_dir(&dir)
            .expect("read catalog data dir")
            .map(|entry| entry.expect("catalog data entry"))
            .collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            if entry.file_type().expect("catalog data file type").is_dir() {
                pending.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .next()
        .expect("committed catalog contains a regular data file")
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

/// Synthetic per-sequence symbol for standalone record/report fixtures that do
/// not consume the committed typed control triple.
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
    let pack_dir = pack_path.parent().expect("pack path has parent");
    let committed = committed_record_zero_controls();
    let source_bindings_path = pack_dir.join("source-bindings.toml");
    fs::copy(
        repo_root().join(
            "specs/023-nt-research-analytics-platform/reference/\
             backfill-source-bindings.v1.toml",
        ),
        &source_bindings_path,
    )
    .expect("copy pack-local source-binding registry");
    let mut records = Vec::with_capacity(objects.len());

    for (record_index, (sequence, object_bytes)) in objects.iter().enumerate() {
        let control_dir = if record_index == 0 {
            pack_dir.to_path_buf()
        } else {
            pack_dir.join(format!("record-{sequence:05}"))
        };
        fs::create_dir_all(&control_dir).expect("create per-record control dir");
        let record_run_spec_path = if record_index == 0 {
            run_spec_path.to_path_buf()
        } else {
            control_dir.join("run-spec.toml")
        };
        let accepted_tranche_path = control_dir.join("accepted-tranche.json");
        let record_execution_plan_path = if record_index == 0 {
            execution_plan_path.to_path_buf()
        } else {
            control_dir.join("execution-plan.json")
        };

        let object_sha256 = sha256_hex(object_bytes);
        let operator_run_id = format!("source-universe-operator-run-synthetic-{sequence:05}");
        let accepted_tranche_id = format!("accepted-tranche-synthetic-{sequence}");

        let mut run_spec = committed.run_spec.clone();
        run_spec.source_bindings_path = PathBuf::from("source-bindings.toml");
        run_spec.manifest.run_id.clone_from(&operator_run_id);
        run_spec.manifest.output_prefix = format!(
            "{}-synthetic-{sequence}",
            committed.run_spec.manifest.output_prefix
        );
        run_spec.accepted_object.sha256.clone_from(&object_sha256);
        run_spec.accepted_object.bytes = object_bytes.len() as u64;
        run_spec
            .source_proof
            .raw_sample_hash
            .clone_from(&object_sha256);
        run_spec
            .source_proof
            .schema_sample_hash
            .clone_from(&object_sha256);
        if let Some(scope) = run_spec.source_proof.acceptance_scope.as_mut() {
            scope.accepted_bytes = object_bytes.len() as u64;
        }
        run_spec.converter.raw_payload.max_object_bytes = run_spec
            .converter
            .raw_payload
            .max_object_bytes
            .max(object_bytes.len() as u64);

        let mut accepted_tranche = committed.accepted_tranche.clone();
        accepted_tranche.tranche_id.clone_from(&accepted_tranche_id);
        accepted_tranche.accepted_bytes = object_bytes.len() as u64;
        let accepted_object = accepted_tranche
            .objects
            .first_mut()
            .expect("committed record-zero tranche has one object");
        accepted_object.sha256.clone_from(&object_sha256);
        accepted_object.bytes = object_bytes.len() as u64;

        let run_spec_bytes = toml::to_string_pretty(&run_spec)
            .expect("serialize typed per-record run spec")
            .into_bytes();
        let accepted_tranche_bytes = serde_json::to_vec_pretty(&accepted_tranche)
            .expect("serialize typed per-record accepted tranche");
        let execution_plan = evaluate_backfill_execution_plan(
            format!("backfill-execution-plan-synthetic-{sequence}"),
            sha256_hex(&accepted_tranche_bytes),
            &accepted_tranche,
            sha256_hex(&run_spec_bytes),
            &BackfillExecutionRunBinding::from_run_spec(&run_spec),
            BackfillExecutionWorkBudget {
                max_source_rows: committed.execution_plan.max_source_rows,
                max_projected_row_groups: committed.execution_plan.max_projected_row_groups,
                max_wall_seconds: committed.execution_plan.max_wall_seconds,
                require_object_selection_metadata: committed
                    .execution_plan
                    .require_object_selection_metadata,
            },
        );
        assert_eq!(
            execution_plan.status,
            BackfillExecutionPlanStatus::Ready,
            "derived per-record execution plan must be ready"
        );
        let execution_plan_bytes = serde_json::to_vec_pretty(&execution_plan)
            .expect("serialize typed per-record execution plan");

        fs::write(&record_run_spec_path, &run_spec_bytes).expect("write typed run spec");
        fs::write(&accepted_tranche_path, &accepted_tranche_bytes)
            .expect("write typed accepted tranche");
        fs::write(&record_execution_plan_path, &execution_plan_bytes)
            .expect("write typed execution plan");

        let mut record = committed.record.clone();
        record.sequence = *sequence;
        record.work_item_id = format!("synthetic-work-item-{sequence}");
        record.operator_run_id = operator_run_id;
        record.selected_object_sha256 = object_sha256;
        record.selected_object_bytes = object_bytes.len() as u64;
        record.accepted_tranche_id = accepted_tranche_id;
        record.output_prefix = run_spec.manifest.output_prefix.clone();
        record.run_spec_path = record_run_spec_path
            .strip_prefix(pack_dir)
            .expect("run spec is pack-relative")
            .to_path_buf();
        record.run_spec_sha256 = sha256_hex(&run_spec_bytes);
        record.accepted_tranche_path = accepted_tranche_path
            .strip_prefix(pack_dir)
            .expect("accepted tranche is pack-relative")
            .to_path_buf();
        record.accepted_tranche_sha256 = sha256_hex(&accepted_tranche_bytes);
        record.execution_plan_path = record_execution_plan_path
            .strip_prefix(pack_dir)
            .expect("execution plan is pack-relative")
            .to_path_buf();
        record.execution_plan_sha256 = sha256_hex(&execution_plan_bytes);
        records.push(record);
    }

    let record_count = objects.len() as u64;
    let total_object_bytes_len = objects.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    let mut pack = committed.pack_template.clone();
    pack.pack_id = "source-universe-execution-pack-synthetic".to_string();
    pack.status = SourceUniverseExecutionPackStatus::Ready;
    pack.planned_object_count = record_count;
    pack.executable_record_count = record_count;
    pack.withheld_record_count = 0;
    pack.selected_record_count = record_count;
    pack.materialized_record_count = record_count;
    pack.skipped_executable_record_count = 0;
    pack.executable_source_bytes = total_object_bytes_len;
    pack.materialized_source_bytes = total_object_bytes_len;
    pack.artifact_refs.clear();
    pack.records = records;
    pack.blocking_reasons.clear();
    fs::write(
        pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize typed execution pack fixture"),
    )
    .expect("write typed n-record pack");
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
        run_spec_sha256: sha256_hex(b"run_id = \"synthetic-run\"\n"),
        accepted_tranche_sha256: sha256_hex(b"{}\n"),
        execution_plan_sha256: sha256_hex(b"{}\n"),
        execution_record_sha256: sha256_hex(b"synthetic execution record"),
        source_bindings_sha256: sha256_hex(b"synthetic source bindings"),
        selected_object_bytes: 0,
        canonical_rows: 7,
        nt_catalog_rows: 7,
        catalog_hash: sha256_hex(b"carried catalog"),
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
        execution_record_sha256: sha256_hex(b"failed synthetic execution record"),
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
        _control_artifacts: &SourceUniverseVerifiedControlArtifacts,
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
            catalog_hash: sha256_hex(format!("catalog-hash-{}", record.sequence).as_bytes()),
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
        _control_artifacts: &SourceUniverseVerifiedControlArtifacts,
        _output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        if self.failing_sequences.contains(&record.sequence) {
            anyhow::bail!("synthetic runner failure for sequence {}", record.sequence);
        }
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: sha256_hex(b"catalog-hash"),
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
