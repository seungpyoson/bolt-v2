use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use backtesting_vertical_slice::backfill_accepted_tranche::{
    BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION, BackfillAcceptedTrancheManifest,
    BackfillAcceptedTrancheObject, BackfillAcceptedTrancheStatus,
};
use backtesting_vertical_slice::backfill_execution_plan::{
    BackfillExecutionPlan, BackfillExecutionPlanStatus, BackfillExecutionRunBinding,
    BackfillExecutionWorkBudget, evaluate_backfill_execution_plan,
};
use backtesting_vertical_slice::operator::{
    CANONICAL_ARTIFACT_FILE, CATALOG_DIR, RunSpec, RunSpecInstrumentIdentities,
};
use backtesting_vertical_slice::source_proof::read_source_binding_registry_from_path;
use backtesting_vertical_slice::source_universe_batch_execution::{
    CachingSourceUniverseObjectFetcher, HttpSourceUniverseObjectFetcher,
    LocalSourceUniverseOperatorRunner, SourceUniverseAdmittedControls,
    SourceUniverseBatchExecutionConfig, SourceUniverseBatchExecutionReport,
    SourceUniverseBatchExecutionReportStatus, SourceUniverseBatchExecutionRunOutput,
    SourceUniverseControlAdmissionPolicy, SourceUniverseObjectFetcher,
    SourceUniverseOperatorRunner, execute_source_universe_batch,
    execute_source_universe_batch_with_config, execute_source_universe_batch_with_factories,
    load_source_universe_control_admission_policy, write_source_universe_batch_execution_report,
};
use backtesting_vertical_slice::source_universe_execution_pack::{
    SourceUniverseExecutionPack, SourceUniverseExecutionPackRecord,
    SourceUniverseExecutionPackStatus,
};

#[cfg(unix)]
struct OneShotFifoWriter {
    opened: std::sync::mpsc::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
}

#[cfg(unix)]
impl OneShotFifoWriter {
    fn start(path: &Path, bytes: Vec<u8>) -> Self {
        let path = path.to_path_buf();
        let (opened_tx, opened) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let mut writer = fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open FIFO writer");
            opened_tx.send(()).expect("report opened FIFO writer");
            std::io::Write::write_all(&mut writer, &bytes).expect("write FIFO payload");
        });
        Self { opened, thread }
    }

    fn finish(self, path: &Path) {
        let cleanup_reader = match self.opened.try_recv() {
            Ok(()) => None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let cleanup_reader = fs::OpenOptions::new()
                    .read(true)
                    .open(path)
                    .expect("open FIFO cleanup reader");
                self.opened
                    .recv_timeout(Duration::from_secs(1))
                    .expect("FIFO writer opens for cleanup");
                Some(cleanup_reader)
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("FIFO writer disconnected before opening")
            }
        };
        self.thread.join().expect("FIFO writer exits");
        drop(cleanup_reader);
    }
}

#[test]
fn source_universe_batch_execution_fetches_verifies_and_runs_pack_record() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let (pack_path, run_spec_path, accepted_tranche_path, execution_plan_path) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);

    let mut fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes: object_bytes.to_vec(),
        calls: 0,
    };
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
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
    assert_eq!(
        runner.calls[0].run_spec_bytes,
        fs::read(run_spec_path).unwrap()
    );
    assert_eq!(
        runner.calls[0].accepted_tranche_bytes,
        fs::read(accepted_tranche_path).unwrap()
    );
    assert_eq!(
        runner.calls[0].execution_plan_bytes,
        fs::read(execution_plan_path).unwrap()
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
fn needs_work_replaces_the_record_directory_before_any_runner_observes_it() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);
    let record_output_dir = output_dir.join("source-universe-operator-run-synthetic-00000");
    fs::create_dir_all(&record_output_dir).expect("create stale record output");
    fs::write(record_output_dir.join("unverified-prior-output"), b"stale")
        .expect("write stale record output");

    let mut fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes: object_bytes.to_vec(),
        calls: 0,
    };
    let mut runner = EmptyDirectoryRunner;

    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut fetcher,
        &mut runner,
    )
    .expect("NeedsWork must run against a fresh record directory");

    assert_eq!(report.completed_record_count, 1);
    assert!(record_output_dir.is_dir());
    assert!(
        !record_output_dir.join("unverified-prior-output").exists(),
        "unverified prior output must not survive a NeedsWork transition"
    );
}

#[cfg(unix)]
#[test]
fn needs_work_unlinks_a_special_canonical_artifact_before_running() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);
    let record_output_dir = output_dir.join("source-universe-operator-run-synthetic-00000");
    fs::create_dir_all(&record_output_dir).expect("create stale record output");
    let canonical_path = record_output_dir.join(CANONICAL_ARTIFACT_FILE);
    let status = std::process::Command::new("mkfifo")
        .arg(&canonical_path)
        .status()
        .expect("create canonical-artifact FIFO");
    assert!(status.success(), "mkfifo must create the test FIFO");

    let mut fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes: object_bytes.to_vec(),
        calls: 0,
    };
    let mut runner = EmptyDirectoryRunner;

    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut fetcher,
        &mut runner,
    )
    .expect("NeedsWork must unlink a special canonical artifact without opening it");

    assert_eq!(report.completed_record_count, 1);
    assert!(
        !canonical_path.exists(),
        "the special canonical artifact must be gone before runner invocation"
    );
}

#[cfg(unix)]
#[test]
fn needs_work_replaces_a_record_directory_symlink_without_touching_its_target() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    fs::create_dir(&output_dir).expect("create batch output");
    let outside_dir = temp_dir.path().join("outside-record-output");
    fs::create_dir(&outside_dir).expect("create outside record output");
    let outside_sentinel = outside_dir.join("must-survive");
    fs::write(&outside_sentinel, b"outside").expect("write outside sentinel");
    let record_output_dir = output_dir.join("source-universe-operator-run-synthetic-00000");
    std::os::unix::fs::symlink(&outside_dir, &record_output_dir)
        .expect("plant record-directory symlink");

    let object_bytes = b"accepted object bytes";
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);
    let mut fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes: object_bytes.to_vec(),
        calls: 0,
    };
    let mut runner = EmptyDirectoryRunner;

    execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut fetcher,
        &mut runner,
    )
    .expect("NeedsWork must replace the record-directory symlink itself");

    let metadata = fs::symlink_metadata(&record_output_dir).expect("record output metadata");
    assert!(metadata.file_type().is_dir());
    assert!(!metadata.file_type().is_symlink());
    assert_eq!(
        fs::read(&outside_sentinel).expect("outside sentinel survives"),
        b"outside"
    );
}

#[cfg(unix)]
#[test]
fn batch_output_root_symlink_rejects_before_fetch_or_runner_invocation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let outside_dir = temp_dir.path().join("outside-batch-output");
    fs::create_dir(&outside_dir).expect("create outside batch output");
    let outside_sentinel = outside_dir.join("must-survive");
    fs::write(&outside_sentinel, b"outside").expect("write outside sentinel");
    let output_dir = temp_dir.path().join("batch-output");
    std::os::unix::fs::symlink(&outside_dir, &output_dir).expect("plant batch-output symlink");
    let object_bytes = b"accepted object bytes";
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);
    let mut runner = RecordingRunner::default();

    let error = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut NeverFetcher,
        &mut runner,
    )
    .expect_err("a symlinked batch output root must reject before execution");

    assert!(
        format!("{error:#}").contains("batch output dir"),
        "{error:#}"
    );
    assert!(runner.calls.is_empty());
    assert_eq!(
        fs::read(&outside_sentinel).expect("outside sentinel survives"),
        b"outside"
    );
}

#[test]
fn needs_work_executes_the_real_local_runner_against_a_fresh_directory() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = reference_binance_zip_object();
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.clone())]);
    let record_output_dir = output_dir.join("source-universe-operator-run-synthetic-00000");
    fs::create_dir_all(&record_output_dir).expect("create stale record output");
    let stale_path = record_output_dir.join("unverified-prior-output");
    fs::write(&stale_path, b"stale").expect("write stale record output");
    let mut fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes,
        calls: 0,
    };
    let mut runner = LocalSourceUniverseOperatorRunner;

    let report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut fetcher,
        &mut runner,
    )
    .expect("real local runner executes fresh work");

    assert_eq!(report.completed_record_count, 1);
    assert!(!stale_path.exists());
    assert!(record_output_dir.join(CANONICAL_ARTIFACT_FILE).is_file());
}

#[cfg(unix)]
#[test]
fn unverifiable_resume_output_is_rebuilt_by_the_real_local_runner() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = reference_binance_zip_object();
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.clone())]);
    let mut initial_fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes: object_bytes.clone(),
        calls: 0,
    };
    let mut initial_runner = LocalSourceUniverseOperatorRunner;
    let prior_report = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut initial_fetcher,
        &mut initial_runner,
    )
    .expect("initial real run completes");
    let prior_report_path = temp_dir.path().join("prior-report.json");
    fs::write(
        &prior_report_path,
        serde_json::to_vec_pretty(&prior_report).expect("serialize prior report"),
    )
    .expect("write prior report outside the batch output root");

    let outside_target = temp_dir.path().join("outside-catalog-target");
    fs::write(&outside_target, b"outside").expect("write outside target");
    let record_output_dir = prior_report.records[0].output_dir.clone();
    let planted_symlink = record_output_dir.join(CATALOG_DIR).join("ignored-special-entry");
    std::os::unix::fs::symlink(&outside_target, &planted_symlink)
        .expect("plant an unverifiable catalog descendant");

    let mut resumed_fetcher = StaticFetcher {
        expected_source_url: "https://public.synthetic.example/object-0.csv.gz".to_string(),
        object_bytes,
        calls: 0,
    };
    let mut resumed_runner = LocalSourceUniverseOperatorRunner;
    let resumed_report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
            start_sequence: None,
            record_limit: Some(1),
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(prior_report_path),
        },
        &mut resumed_fetcher,
        &mut resumed_runner,
    )
    .expect("unverifiable prior output is rebuilt fresh");

    assert_eq!(resumed_report.completed_record_count, 1);
    assert_eq!(resumed_fetcher.calls, 1, "NeedsWork must refetch the object");
    assert!(
        fs::symlink_metadata(&planted_symlink).is_err(),
        "fresh execution must remove the unverifiable prior entry"
    );
    assert_eq!(
        fs::read(&outside_target).expect("outside target survives"),
        b"outside"
    );
    assert!(record_output_dir.join(CANONICAL_ARTIFACT_FILE).is_file());
}

#[test]
fn control_hash_mismatch_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let (pack_path, run_spec_path, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);
    fs::write(&run_spec_path, "run_id = \"tampered\"\n").expect("tamper run spec");

    let error = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut NeverFetcher,
        &mut RecordingRunner::default(),
    )
    .expect_err("tampered control must reject before fetch");

    assert!(
        format!("{error:#}").contains("pinned run_spec sha256 mismatch"),
        "full error chain must identify the rejected control: {error:#}"
    );
    assert!(
        !output_dir.exists(),
        "invalid admission must create no output"
    );
}

#[test]
fn malformed_control_sha256_fields_reject_before_fetch_or_output_creation() {
    for field in [
        "run_spec_sha256",
        "accepted_tranche_sha256",
        "execution_plan_sha256",
    ] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (pack_path, _, _, _) = write_control_admission_fixture(
            temp_dir.path(),
            &[(0, b"accepted object bytes".to_vec())],
        );
        let mut pack: serde_json::Value = serde_json::from_slice(
            &fs::read(&pack_path).expect("read execution pack for mutation"),
        )
        .expect("parse execution pack for mutation");
        pack["records"][0][field] = serde_json::Value::String("A".repeat(64));
        fs::write(
            &pack_path,
            serde_json::to_vec_pretty(&pack).expect("serialize mutated execution pack"),
        )
        .expect("write mutated execution pack");

        assert_control_admission_rejects_before_side_effects(
            &pack_path,
            &temp_dir.path().join("batch-output"),
            &format!("invalid {field}"),
        );
    }
}

#[test]
fn missing_control_file_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, execution_plan_path) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    fs::remove_file(execution_plan_path).expect("remove execution plan");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("batch-output"),
        "open pinned execution_plan",
    );
}

#[test]
fn non_regular_control_file_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, accepted_tranche_path, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    fs::remove_file(&accepted_tranche_path).expect("remove accepted tranche file");
    fs::create_dir(&accepted_tranche_path).expect("replace accepted tranche with directory");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("batch-output"),
        "pinned accepted_tranche",
    );
}

#[cfg(unix)]
#[test]
fn fifo_control_file_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, accepted_tranche_path, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    fs::remove_file(&accepted_tranche_path).expect("remove accepted tranche file");
    let status = std::process::Command::new("mkfifo")
        .arg(&accepted_tranche_path)
        .status()
        .expect("create accepted tranche FIFO");
    assert!(status.success(), "mkfifo must create the test FIFO");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("batch-output"),
        "pinned accepted_tranche",
    );
}

#[cfg(unix)]
#[test]
fn fifo_execution_pack_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    let policy = test_control_admission_policy(temp_dir.path());
    fs::remove_file(&pack_path).expect("remove execution pack");
    let status = std::process::Command::new("mkfifo")
        .arg(&pack_path)
        .status()
        .expect("create execution-pack FIFO");
    assert!(status.success(), "mkfifo must create the test FIFO");
    let fifo_writer = OneShotFifoWriter::start(&pack_path, b"not-json".to_vec());
    let output_dir = temp_dir.path().join("batch-output");
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        None,
        policy,
        &mut NeverFetcher,
        &mut runner,
    );
    fifo_writer.finish(&pack_path);
    let error = result.expect_err("execution-pack FIFO must reject before parsing");
    let error = format!("{error:#}");

    assert!(error.contains("execution pack"), "{error}");
    assert!(error.contains("is not a regular file"), "{error}");
    assert!(!output_dir.exists(), "rejection must create no output");
    assert!(runner.calls.is_empty(), "rejection must not run");
}

#[cfg(unix)]
#[test]
fn fifo_resume_report_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    let resume_report = temp_dir.path().join("prior-report.json");
    let status = std::process::Command::new("mkfifo")
        .arg(&resume_report)
        .status()
        .expect("create resume-report FIFO");
    assert!(status.success(), "mkfifo must create the test FIFO");
    let fifo_writer = OneShotFifoWriter::start(&resume_report, b"not-json".to_vec());
    let output_dir = temp_dir.path().join("batch-output");
    let mut runner = RecordingRunner::default();

    let result = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report.clone()),
        },
        &mut NeverFetcher,
        &mut runner,
    );
    fifo_writer.finish(&resume_report);
    let error = result.expect_err("resume-report FIFO must reject before parsing");
    let error = format!("{error:#}");

    assert!(error.contains("resume report"), "{error}");
    assert!(error.contains("is not a regular file"), "{error}");
    assert!(!output_dir.exists(), "rejection must create no output");
    assert!(runner.calls.is_empty(), "rejection must not run");
}

#[cfg(unix)]
#[test]
fn standalone_registry_loader_rejects_fifo_before_parsing() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let registry_path = temp_dir.path().join("source-bindings.toml");
    let status = std::process::Command::new("mkfifo")
        .arg(&registry_path)
        .status()
        .expect("create source-bindings FIFO");
    assert!(status.success(), "mkfifo must create the test FIFO");
    let fifo_writer = OneShotFifoWriter::start(&registry_path, b"not-toml".to_vec());

    let result = read_source_binding_registry_from_path(&registry_path);
    fifo_writer.finish(&registry_path);
    let error = result.expect_err("source-bindings FIFO must reject before parsing");
    let error = format!("{error:#}");

    assert!(error.contains("source-binding registry"), "{error}");
    assert!(error.contains("is not a regular file"), "{error}");
}

#[test]
fn missing_outer_inputs_retain_path_specific_error_context() {
    let pack_temp_dir = tempfile::tempdir().expect("pack temp dir");
    let (pack_path, _, _, _) = write_control_admission_fixture(
        pack_temp_dir.path(),
        &[(0, b"accepted object bytes".to_vec())],
    );
    let policy = test_control_admission_policy(pack_temp_dir.path());
    fs::remove_file(&pack_path).expect("remove execution pack");
    let pack_error = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &pack_temp_dir.path().join("batch-output"),
        None,
        policy,
        &mut NeverFetcher,
        &mut RecordingRunner::default(),
    )
    .expect_err("missing execution pack must reject");
    let pack_error = format!("{pack_error:#}");
    assert!(pack_error.contains("open execution pack"), "{pack_error}");
    assert!(
        pack_error.contains(pack_path.to_string_lossy().as_ref()),
        "{pack_error}"
    );

    let resume_temp_dir = tempfile::tempdir().expect("resume temp dir");
    let (pack_path, _, _, _) = write_control_admission_fixture(
        resume_temp_dir.path(),
        &[(0, b"accepted object bytes".to_vec())],
    );
    let resume_report = resume_temp_dir.path().join("missing-resume-report.json");
    let resume_error = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &resume_temp_dir.path().join("batch-output"),
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(resume_temp_dir.path()),
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(resume_report.clone()),
        },
        &mut NeverFetcher,
        &mut RecordingRunner::default(),
    )
    .expect_err("missing resume report must reject");
    let resume_error = format!("{resume_error:#}");
    assert!(
        resume_error.contains("open resume report"),
        "{resume_error}"
    );
    assert!(
        resume_error.contains(resume_report.to_string_lossy().as_ref()),
        "{resume_error}"
    );

    let registry_path = resume_temp_dir.path().join("missing-source-bindings.toml");
    let registry_error = read_source_binding_registry_from_path(&registry_path)
        .expect_err("missing standalone registry must reject");
    let registry_error = format!("{registry_error:#}");
    assert!(
        registry_error.contains("open source-binding registry"),
        "{registry_error}"
    );
    assert!(
        registry_error.contains(registry_path.to_string_lossy().as_ref()),
        "{registry_error}"
    );
}

#[test]
fn duplicate_selected_sequence_rejects_before_fetch_or_output_creation() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, _) = write_control_admission_fixture(
        temp_dir.path(),
        &[
            (0, b"first accepted object bytes".to_vec()),
            (1, b"second accepted object bytes".to_vec()),
        ],
    );
    let mut pack: serde_json::Value = serde_json::from_slice(
        &fs::read(&pack_path).expect("read execution pack for duplicate mutation"),
    )
    .expect("parse execution pack for duplicate mutation");
    pack["records"][1]["sequence"] = serde_json::json!(0);
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize duplicate execution pack"),
    )
    .expect("write duplicate execution pack");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("batch-output"),
        "duplicate record sequence 0",
    );
}

#[test]
fn runner_consumes_admitted_control_and_registry_snapshot_after_source_path_changes() {
    struct MutatingFetcher {
        control_replacements: Vec<(PathBuf, Vec<u8>)>,
        object_bytes: Vec<u8>,
    }

    impl SourceUniverseObjectFetcher for MutatingFetcher {
        fn fetch(
            &mut self,
            _record: &SourceUniverseExecutionPackRecord,
        ) -> anyhow::Result<Vec<u8>> {
            for (path, replacement) in &self.control_replacements {
                fs::write(path, replacement)?;
            }
            Ok(self.object_bytes.clone())
        }
    }

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("batch-output");
    let object_bytes = b"accepted object bytes";
    let (pack_path, run_spec_path, accepted_tranche_path, execution_plan_path) =
        write_control_admission_fixture(temp_dir.path(), &[(0, object_bytes.to_vec())]);
    let admitted_run_spec = fs::read(&run_spec_path).expect("read admitted run spec");
    let admitted_tranche =
        fs::read(&accepted_tranche_path).expect("read admitted accepted tranche");
    let admitted_plan = fs::read(&execution_plan_path).expect("read admitted execution plan");
    let admitted_run_spec_value: RunSpec =
        toml::from_slice(&admitted_run_spec).expect("parse admitted run spec");
    let admitted_run_id = admitted_run_spec_value.manifest.run_id.clone();
    let mut changed_run_spec = admitted_run_spec_value;
    changed_run_spec.manifest.run_id = "changed-after-admission".to_string();
    let changed_run_spec_bytes = toml::to_string_pretty(&changed_run_spec)
        .expect("serialize changed run spec")
        .into_bytes();

    let mut fetcher = MutatingFetcher {
        control_replacements: vec![
            (run_spec_path, changed_run_spec_bytes),
            (accepted_tranche_path, b"changed-after-admission".to_vec()),
            (execution_plan_path, b"changed-after-admission".to_vec()),
            (
                temp_dir.path().join("source-bindings.toml"),
                b"changed-after-admission".to_vec(),
            ),
        ],
        object_bytes: object_bytes.to_vec(),
    };
    let mut runner = RecordingRunner::default();
    execute_source_universe_batch(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        Some(1),
        test_control_admission_policy(temp_dir.path()),
        &mut fetcher,
        &mut runner,
    )
    .expect("batch uses admitted control snapshot");

    assert_eq!(runner.calls[0].run_spec_bytes, admitted_run_spec);
    assert_eq!(runner.calls[0].accepted_tranche_bytes, admitted_tranche);
    assert_eq!(runner.calls[0].execution_plan_bytes, admitted_plan);
    assert_eq!(runner.calls[0].run_spec_run_id, admitted_run_id);
}

#[test]
fn cross_binding_rejects_a_hash_valid_splice_before_side_effects() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, _) = write_control_admission_fixture(
        temp_dir.path(),
        &[
            (0, b"first accepted object bytes".to_vec()),
            (1, b"second accepted object bytes".to_vec()),
        ],
    );
    let mut pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&pack_path).expect("read execution pack for splice"))
            .expect("parse execution pack for splice");
    let spliced_run_spec_path = pack.records[1].run_spec_path.clone();
    let spliced_run_spec_sha256 = pack.records[1].run_spec_sha256.clone();
    pack.records[0].run_spec_path = spliced_run_spec_path;
    pack.records[0].run_spec_sha256 = spliced_run_spec_sha256;
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize spliced execution pack"),
    )
    .expect("write spliced execution pack");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("batch-output"),
        "execution_plan does not match the admitted run_spec and accepted_tranche",
    );
}

#[test]
fn record_and_pack_identity_mismatches_reject_before_side_effects() {
    let cases = [
        (
            "/records/0/operator_run_id",
            serde_json::json!("different-operator-run"),
            "operator_run_id mismatch",
        ),
        (
            "/records/0/accepted_tranche_id",
            serde_json::json!("different-tranche"),
            "accepted_tranche_id mismatch",
        ),
        (
            "/records/0/source_proof_id",
            serde_json::json!("different-proof"),
            "source proof identity mismatch",
        ),
        (
            "/records/0/source_binding",
            serde_json::json!("different-binding"),
            "source_binding mismatch",
        ),
        (
            "/records/0/output_prefix",
            serde_json::json!("s3://different/output"),
            "output_prefix mismatch",
        ),
        (
            "/records/0/category",
            serde_json::json!("different-category"),
            "category mismatch",
        ),
        (
            "/records/0/symbol",
            serde_json::json!("DIFFERENT-SYMBOL"),
            "symbol mismatch",
        ),
        (
            "/records/0/selected_object_sha256",
            serde_json::json!("b".repeat(64)),
            "selected_object_sha256 mismatch",
        ),
        (
            "/records/0/selected_object_bytes",
            serde_json::json!(999),
            "selected_object_bytes mismatch",
        ),
        (
            "/records/0/source_uri",
            serde_json::json!("s3://different/object"),
            "source_uri mismatch",
        ),
        (
            "/records/0/source_url",
            serde_json::json!("https://different.invalid/object"),
            "source_url mismatch",
        ),
        (
            "/records/0/archive_date",
            serde_json::json!("2026-04-01"),
            "archive_date mismatch",
        ),
        (
            "/venue",
            serde_json::json!("different-neutral-venue"),
            "venue mismatch",
        ),
        (
            "/table_family",
            serde_json::json!("different-table-family"),
            "table_family mismatch",
        ),
    ];

    for (pointer, replacement, expected_error) in cases {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (pack_path, _, _, _) = write_control_admission_fixture(
            temp_dir.path(),
            &[(0, b"accepted object bytes".to_vec())],
        );
        let mut pack: serde_json::Value = serde_json::from_slice(
            &fs::read(&pack_path).expect("read execution pack for identity mutation"),
        )
        .expect("parse execution pack for identity mutation");
        *pack
            .pointer_mut(pointer)
            .expect("identity mutation pointer must exist") = replacement;
        if pointer == "/records/0/selected_object_bytes" {
            pack["executable_source_bytes"] = serde_json::json!(999);
            pack["materialized_source_bytes"] = serde_json::json!(999);
        }
        fs::write(
            &pack_path,
            serde_json::to_vec_pretty(&pack).expect("serialize identity-mutated pack"),
        )
        .expect("write identity-mutated pack");

        assert_control_admission_rejects_before_side_effects(
            &pack_path,
            &temp_dir.path().join("batch-output"),
            expected_error,
        );
    }
}

#[test]
fn missing_identities_in_each_control_reject_before_side_effects() {
    for role in ["run_spec", "accepted_tranche", "execution_plan"] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (pack_path, run_spec_path, tranche_path, plan_path) = write_control_admission_fixture(
            temp_dir.path(),
            &[(0, b"accepted object bytes".to_vec())],
        );
        let (path, bytes, pin_field, expected_error) = match role {
            "run_spec" => {
                let mut value: toml::Value = toml::from_slice(
                    &fs::read(&run_spec_path).expect("read run spec for missing identity"),
                )
                .expect("parse run spec for missing identity");
                value
                    .get_mut("manifest")
                    .and_then(toml::Value::as_table_mut)
                    .expect("run spec manifest table")
                    .remove("run_id");
                (
                    run_spec_path,
                    toml::to_string_pretty(&value)
                        .expect("serialize run spec without run_id")
                        .into_bytes(),
                    "run_spec_sha256",
                    "run_id",
                )
            }
            "accepted_tranche" => {
                let mut value: serde_json::Value = serde_json::from_slice(
                    &fs::read(&tranche_path).expect("read tranche for missing identity"),
                )
                .expect("parse tranche for missing identity");
                value
                    .as_object_mut()
                    .expect("tranche object")
                    .remove("tranche_id");
                (
                    tranche_path,
                    serde_json::to_vec_pretty(&value)
                        .expect("serialize tranche without tranche_id"),
                    "accepted_tranche_sha256",
                    "tranche_id",
                )
            }
            "execution_plan" => {
                let mut value: serde_json::Value = serde_json::from_slice(
                    &fs::read(&plan_path).expect("read plan for missing identity"),
                )
                .expect("parse plan for missing identity");
                value
                    .as_object_mut()
                    .expect("plan object")
                    .remove("operator_run_id");
                (
                    plan_path,
                    serde_json::to_vec_pretty(&value)
                        .expect("serialize plan without operator_run_id"),
                    "execution_plan_sha256",
                    "operator_run_id",
                )
            }
            _ => unreachable!(),
        };
        fs::write(&path, &bytes).expect("write control without identity");
        let mut pack: serde_json::Value =
            serde_json::from_slice(&fs::read(&pack_path).expect("read pack for control repin"))
                .expect("parse pack for control repin");
        pack["records"][0][pin_field] = serde_json::json!(sha256_hex(&bytes));
        fs::write(
            &pack_path,
            serde_json::to_vec_pretty(&pack).expect("serialize repinned pack"),
        )
        .expect("write repinned pack");

        assert_control_admission_rejects_before_side_effects(
            &pack_path,
            &temp_dir.path().join("batch-output"),
            expected_error,
        );
    }
}

#[test]
fn malformed_bytes_in_each_control_reject_before_side_effects() {
    for (role, malformed, expected_error) in [
        ("run_spec", b"not = [valid toml".as_slice(), "run_spec TOML"),
        ("accepted_tranche", b"{".as_slice(), "accepted_tranche JSON"),
        ("execution_plan", b"[".as_slice(), "execution_plan JSON"),
    ] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (pack_path, run_spec_path, tranche_path, plan_path) = write_control_admission_fixture(
            temp_dir.path(),
            &[(0, b"accepted object bytes".to_vec())],
        );
        let (path, pin_field) = match role {
            "run_spec" => (run_spec_path, "run_spec_sha256"),
            "accepted_tranche" => (tranche_path, "accepted_tranche_sha256"),
            "execution_plan" => (plan_path, "execution_plan_sha256"),
            _ => unreachable!(),
        };
        fs::write(&path, malformed).expect("write malformed control");
        let mut pack: serde_json::Value = serde_json::from_slice(
            &fs::read(&pack_path).expect("read pack for malformed control repin"),
        )
        .expect("parse pack for malformed control repin");
        pack["records"][0][pin_field] = serde_json::json!(sha256_hex(malformed));
        fs::write(
            &pack_path,
            serde_json::to_vec_pretty(&pack).expect("serialize malformed-control pack"),
        )
        .expect("write malformed-control pack");

        assert_control_admission_rejects_before_side_effects(
            &pack_path,
            &temp_dir.path().join("batch-output"),
            expected_error,
        );
    }
}

#[test]
fn coherent_but_malformed_control_values_reject_before_side_effects() {
    #[derive(Clone, Copy)]
    enum Case {
        ParentPathOperatorRunId,
        AbsolutePathOperatorRunId,
        NulOperatorRunId,
        RegistryVenueCaseMismatch,
        RegistrySourceBindingWhitespaceMismatch,
        PendingSourceProof,
        InvalidSourceUrl,
        InvalidArchiveDate,
        InvalidConverterPayload,
        EmptySourceProofId,
        ZeroSourceProofVersion,
        RunSpecSourceProofIdentityMismatch,
        RunSpecSourceBindingMismatch,
        EmptySourceBinding,
        EmptyTableFamily,
        EmptyVenue,
        EmptyCategory,
        EmptySymbol,
        EmptyOutputPrefix,
        EmptySourceUri,
        EmptySourceUrl,
        EmptyArchiveDate,
        ZeroObjectBytes,
        EmptyScopeReportId,
        MalformedScopeReportHash,
        EmptyParentManifestId,
        ObjectLevelTrancheNotRequired,
        DuplicateSourceRowGroups,
        EmptyPredicateRef,
        ZeroDecodedByteBudget,
    }

    let cases = [
        (Case::ParentPathOperatorRunId, "parent-path-operator-run-id"),
        (
            Case::AbsolutePathOperatorRunId,
            "absolute-path-operator-run-id",
        ),
        (Case::NulOperatorRunId, "nul-operator-run-id"),
        (
            Case::RegistryVenueCaseMismatch,
            "registry-venue-case-mismatch",
        ),
        (
            Case::RegistrySourceBindingWhitespaceMismatch,
            "registry-source-binding-whitespace-mismatch",
        ),
        (Case::PendingSourceProof, "pending-source-proof"),
        (Case::InvalidSourceUrl, "invalid-source-url"),
        (Case::InvalidArchiveDate, "invalid-archive-date"),
        (Case::InvalidConverterPayload, "invalid-converter-payload"),
        (Case::EmptySourceProofId, "empty-source-proof-id"),
        (Case::ZeroSourceProofVersion, "zero-source-proof-version"),
        (
            Case::RunSpecSourceProofIdentityMismatch,
            "run-spec-source-proof-identity-mismatch",
        ),
        (
            Case::RunSpecSourceBindingMismatch,
            "run-spec-source-binding-mismatch",
        ),
        (Case::EmptySourceBinding, "empty-source-binding"),
        (Case::EmptyTableFamily, "empty-table-family"),
        (Case::EmptyVenue, "empty-venue"),
        (Case::EmptyCategory, "empty-category"),
        (Case::EmptySymbol, "empty-symbol"),
        (Case::EmptyOutputPrefix, "empty-output-prefix"),
        (Case::EmptySourceUri, "empty-source-uri"),
        (Case::EmptySourceUrl, "empty-source-url"),
        (Case::EmptyArchiveDate, "empty-archive-date"),
        (Case::ZeroObjectBytes, "zero-object-bytes"),
        (Case::EmptyScopeReportId, "empty-scope-report-id"),
        (
            Case::MalformedScopeReportHash,
            "malformed-scope-report-hash",
        ),
        (Case::EmptyParentManifestId, "empty-parent-manifest-id"),
        (
            Case::ObjectLevelTrancheNotRequired,
            "object-level-tranche-not-required",
        ),
        (
            Case::DuplicateSourceRowGroups,
            "duplicate-source-row-groups",
        ),
        (Case::EmptyPredicateRef, "empty-predicate-ref"),
        (Case::ZeroDecodedByteBudget, "zero-decoded-byte-budget"),
    ];

    for (case, name) in cases {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let (pack_path, run_spec_path, tranche_path, plan_path) = write_control_admission_fixture(
            temp_dir.path(),
            &[(0, b"accepted object bytes".to_vec())],
        );
        let mut pack: SourceUniverseExecutionPack = serde_json::from_slice(
            &fs::read(&pack_path).expect("read pack for coherent malformed control"),
        )
        .expect("parse pack for coherent malformed control");
        let mut run_spec: RunSpec = toml::from_slice(
            &fs::read(&run_spec_path).expect("read run spec for coherent malformed control"),
        )
        .expect("parse run spec for coherent malformed control");
        let mut tranche: BackfillAcceptedTrancheManifest = serde_json::from_slice(
            &fs::read(&tranche_path).expect("read tranche for coherent malformed control"),
        )
        .expect("parse tranche for coherent malformed control");
        let plan_template: BackfillExecutionPlan = serde_json::from_slice(
            &fs::read(&plan_path).expect("read plan for coherent malformed control"),
        )
        .expect("parse plan for coherent malformed control");

        let expected_error = match case {
            Case::ParentPathOperatorRunId => {
                let operator_run_id = "../escaped-output".to_string();
                pack.records[0].operator_run_id.clone_from(&operator_run_id);
                run_spec.manifest.run_id.clone_from(&operator_run_id);
                "operator_run_id must be a single normal path component"
            }
            Case::AbsolutePathOperatorRunId => {
                let operator_run_id = "/tmp/escaped-output".to_string();
                pack.records[0].operator_run_id.clone_from(&operator_run_id);
                run_spec.manifest.run_id.clone_from(&operator_run_id);
                "operator_run_id must be a single normal path component"
            }
            Case::NulOperatorRunId => {
                let operator_run_id = "escaped\0output".to_string();
                pack.records[0].operator_run_id.clone_from(&operator_run_id);
                run_spec.manifest.run_id.clone_from(&operator_run_id);
                "operator_run_id must be a single normal path component"
            }
            Case::RegistryVenueCaseMismatch => {
                let venue = "SYNTHETIC-VENUE".to_string();
                pack.venue.clone_from(&venue);
                run_spec.source_proof.venue.clone_from(&venue);
                "is not configured in the registry"
            }
            Case::RegistrySourceBindingWhitespaceMismatch => {
                let source_binding = "synthetic-spot-tick-trades ".to_string();
                pack.records[0].source_binding.clone_from(&source_binding);
                run_spec
                    .manifest
                    .venue_binding_key
                    .clone_from(&source_binding);
                run_spec
                    .source_proof
                    .source_binding
                    .clone_from(&source_binding);
                tranche.source_binding.clone_from(&source_binding);
                "is not configured in the registry"
            }
            Case::PendingSourceProof => {
                run_spec.source_proof.status =
                    backtesting_vertical_slice::source_proof::SourceProofStatus::Pending;
                "source proof is not accepted"
            }
            Case::InvalidSourceUrl => {
                let source_url = "http://public.synthetic.example/object-0.csv.gz".to_string();
                pack.records[0].source_url.clone_from(&source_url);
                run_spec.accepted_object.source_url.clone_from(&source_url);
                tranche.objects[0].source_url.clone_from(&source_url);
                "does not reference proof venue"
            }
            Case::InvalidArchiveDate => {
                let archive_date = "not-a-date".to_string();
                pack.records[0].archive_date.clone_from(&archive_date);
                run_spec
                    .accepted_object
                    .archive_date
                    .clone_from(&archive_date);
                tranche.objects[0].archive_date.clone_from(&archive_date);
                "object archive_date"
            }
            Case::InvalidConverterPayload => {
                run_spec.converter.raw_payload.zip_member = None;
                "zip_member"
            }
            Case::EmptySourceProofId => {
                pack.records[0].source_proof_id.clear();
                run_spec.manifest.source_proof_id.clear();
                run_spec.source_proof.source_proof_id.clear();
                tranche.source_proof_id.clear();
                "execution pack record source_proof_id must not be empty"
            }
            Case::ZeroSourceProofVersion => {
                pack.records[0].source_proof_version = 0;
                run_spec.manifest.source_proof_version = 0;
                run_spec.source_proof.source_proof_version = 0;
                tranche.source_proof_version = 0;
                "execution pack record source_proof_version must be positive"
            }
            Case::RunSpecSourceProofIdentityMismatch => {
                run_spec.source_proof.source_proof_id = "different-source-proof".to_string();
                "run_spec source proof identity mismatch"
            }
            Case::RunSpecSourceBindingMismatch => {
                run_spec.source_proof.source_binding = "different-source-binding".to_string();
                "run_spec source_binding mismatch"
            }
            Case::EmptySourceBinding => {
                pack.records[0].source_binding.clear();
                run_spec.manifest.venue_binding_key.clear();
                run_spec.source_proof.source_binding.clear();
                tranche.source_binding.clear();
                "source_binding must not be empty"
            }
            Case::EmptyTableFamily => {
                pack.table_family.clear();
                run_spec.source_proof.table_family.clear();
                tranche.table_family.clear();
                "table_family must not be empty"
            }
            Case::EmptyVenue => {
                pack.venue.clear();
                run_spec.source_proof.venue.clear();
                "venue must not be empty"
            }
            Case::EmptyCategory => {
                pack.records[0].category.clear();
                run_spec.source_proof.product_category.clear();
                "category must not be empty"
            }
            Case::EmptySymbol => {
                pack.records[0].symbol.clear();
                match &mut run_spec.identity {
                    RunSpecInstrumentIdentities::Single(identity) => {
                        identity.instrument_id.clear();
                        identity.venue_symbol.clear();
                    }
                    RunSpecInstrumentIdentities::Keyed(_) => {
                        panic!("synthetic run-spec must be single")
                    }
                }
                "symbol must not be empty"
            }
            Case::EmptyOutputPrefix => {
                pack.records[0].output_prefix.clear();
                run_spec.manifest.output_prefix.clear();
                "output_prefix must not be empty"
            }
            Case::EmptySourceUri => {
                pack.records[0].source_uri.clear();
                run_spec.source_proof.raw_sample_uri.clear();
                run_spec.accepted_object.s3_uri.clear();
                tranche.objects[0].s3_uri.clear();
                "source_uri must not be empty"
            }
            Case::EmptySourceUrl => {
                pack.records[0].source_url.clear();
                run_spec.accepted_object.source_url.clear();
                tranche.objects[0].source_url.clear();
                "source_url must not be empty"
            }
            Case::EmptyArchiveDate => {
                pack.records[0].archive_date.clear();
                run_spec.accepted_object.archive_date.clear();
                tranche.objects[0].archive_date.clear();
                "archive_date must not be empty"
            }
            Case::ZeroObjectBytes => {
                pack.records[0].selected_object_bytes = 0;
                pack.executable_source_bytes = 0;
                pack.materialized_source_bytes = 0;
                run_spec.accepted_object.bytes = 0;
                run_spec.converter.raw_payload.max_object_bytes = 0;
                tranche.accepted_bytes = 0;
                tranche.objects[0].bytes = 0;
                "selected_object_bytes must be positive"
            }
            Case::EmptyScopeReportId => {
                tranche.source_proof_scope_report_id.clear();
                "source_proof_scope_report_id must not be empty"
            }
            Case::MalformedScopeReportHash => {
                tranche.source_proof_scope_report_hash = "not-a-sha256".to_string();
                "source_proof_scope_report_hash"
            }
            Case::EmptyParentManifestId => {
                tranche.parent_manifest_id.clear();
                "parent_manifest_id must not be empty"
            }
            Case::ObjectLevelTrancheNotRequired => {
                tranche.object_level_tranche_required = false;
                "object_level_tranche_required must be true"
            }
            Case::DuplicateSourceRowGroups => {
                tranche.objects[0].source_row_groups = vec![1, 1];
                "source_row_groups must be strictly increasing"
            }
            Case::EmptyPredicateRef => {
                tranche.objects[0].predicate_ref = Some(" ".to_string());
                "predicate_ref must not be empty"
            }
            Case::ZeroDecodedByteBudget => {
                run_spec.converter.raw_payload.max_decoded_bytes = 0;
                "max_decoded_bytes must be positive"
            }
        };

        let run_spec_bytes = toml::to_string_pretty(&run_spec)
            .expect("serialize coherent malformed run spec")
            .into_bytes();
        let accepted_tranche_bytes =
            serde_json::to_vec_pretty(&tranche).expect("serialize coherent malformed tranche");
        let run_spec_sha256 = sha256_hex(&run_spec_bytes);
        let accepted_tranche_sha256 = sha256_hex(&accepted_tranche_bytes);
        let execution_plan = evaluate_backfill_execution_plan(
            plan_template.plan_id,
            accepted_tranche_sha256.clone(),
            &tranche,
            run_spec_sha256.clone(),
            &BackfillExecutionRunBinding::from_run_spec(&run_spec),
            BackfillExecutionWorkBudget {
                max_source_rows: plan_template.max_source_rows,
                max_projected_row_groups: plan_template.max_projected_row_groups,
                max_wall_seconds: plan_template.max_wall_seconds,
                require_object_selection_metadata: plan_template.require_object_selection_metadata,
            },
        );
        assert_eq!(
            execution_plan.status,
            BackfillExecutionPlanStatus::Ready,
            "{name} must remain evaluator-consistent so the self-validity gate is decisive"
        );
        let execution_plan_bytes = serde_json::to_vec_pretty(&execution_plan)
            .expect("serialize coherent malformed execution plan");
        fs::write(&run_spec_path, &run_spec_bytes).expect("write coherent malformed run spec");
        fs::write(&tranche_path, &accepted_tranche_bytes)
            .expect("write coherent malformed tranche");
        fs::write(&plan_path, &execution_plan_bytes).expect("write coherent malformed plan");
        pack.records[0].run_spec_sha256 = run_spec_sha256;
        pack.records[0].accepted_tranche_sha256 = accepted_tranche_sha256;
        pack.records[0].execution_plan_sha256 = sha256_hex(&execution_plan_bytes);
        fs::write(
            &pack_path,
            serde_json::to_vec_pretty(&pack).expect("serialize coherent malformed pack"),
        )
        .expect("write coherent malformed pack");

        assert_control_admission_rejects_before_side_effects(
            &pack_path,
            &temp_dir.path().join(format!("{name}-output")),
            expected_error,
        );
    }
}

#[test]
fn malformed_execution_pack_declarations_reject_before_side_effects() {
    enum Case {
        SchemaVersion,
        EmptyUniverse,
        EmptyFamily,
        EmptyWorkItemId,
        MaterializedCount,
        MaterializedBytes,
        ExecutableBytesWithoutSkippedRecords,
        ExecutableBytesWithSkippedRecords,
        DuplicateSequence,
        DuplicateOperatorRunId,
    }

    for (case, expected_error) in [
        (
            Case::SchemaVersion,
            "execution pack schema_version mismatch",
        ),
        (
            Case::EmptyUniverse,
            "execution pack universe_id must not be empty",
        ),
        (Case::EmptyFamily, "execution pack family must not be empty"),
        (
            Case::EmptyWorkItemId,
            "execution pack record work_item_id must not be empty",
        ),
        (Case::MaterializedCount, "materialized_record_count"),
        (Case::MaterializedBytes, "materialized_source_bytes"),
        (
            Case::ExecutableBytesWithoutSkippedRecords,
            "executable_source_bytes must equal materialized_source_bytes when no executable records were skipped",
        ),
        (
            Case::ExecutableBytesWithSkippedRecords,
            "executable_source_bytes must exceed materialized_source_bytes when executable records were skipped",
        ),
        (Case::DuplicateSequence, "duplicate record sequence"),
        (Case::DuplicateOperatorRunId, "duplicate operator_run_id"),
    ] {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let objects = vec![
            (0, b"accepted object zero".to_vec()),
            (1, b"accepted object one".to_vec()),
        ];
        let (pack_path, _, _, _) = write_control_admission_fixture(temp_dir.path(), &objects);
        let mut pack: SourceUniverseExecutionPack = serde_json::from_slice(
            &fs::read(&pack_path).expect("read pack for declaration mutation"),
        )
        .expect("parse pack for declaration mutation");
        match case {
            Case::SchemaVersion => pack.schema_version = "unknown-pack-schema".to_string(),
            Case::EmptyUniverse => pack.universe_id.clear(),
            Case::EmptyFamily => pack.family.clear(),
            Case::EmptyWorkItemId => pack.records[0].work_item_id.clear(),
            Case::MaterializedCount => pack.materialized_record_count = 1,
            Case::MaterializedBytes => pack.materialized_source_bytes += 1,
            Case::ExecutableBytesWithoutSkippedRecords => pack.executable_source_bytes += 1,
            Case::ExecutableBytesWithSkippedRecords => {
                pack.status = SourceUniverseExecutionPackStatus::PartiallyReady;
                pack.executable_record_count += 1;
                pack.planned_object_count += 1;
                pack.skipped_executable_record_count = 1;
                pack.executable_source_bytes = pack.materialized_source_bytes;
                pack.blocking_reasons = vec!["one executable record was skipped".to_string()];
            }
            Case::DuplicateSequence => pack.records[1].sequence = pack.records[0].sequence,
            Case::DuplicateOperatorRunId => {
                pack.records[1].operator_run_id = pack.records[0].operator_run_id.clone();
            }
        }
        fs::write(
            &pack_path,
            serde_json::to_vec_pretty(&pack).expect("serialize mutated pack"),
        )
        .expect("write mutated pack");

        assert_control_admission_rejects_before_side_effects(
            &pack_path,
            &temp_dir.path().join("batch-output"),
            expected_error,
        );
    }
}

#[test]
fn internally_drifted_execution_plan_rejects_after_repinning() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, execution_plan_path) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    let mut plan: BackfillExecutionPlan = serde_json::from_slice(
        &fs::read(&execution_plan_path).expect("read execution plan for drift"),
    )
    .expect("parse execution plan for drift");
    plan.accepted_bytes = plan
        .accepted_bytes
        .checked_add(1)
        .expect("accepted bytes add");
    let plan_bytes = serde_json::to_vec_pretty(&plan).expect("serialize drifted plan");
    fs::write(&execution_plan_path, &plan_bytes).expect("write drifted plan");
    let mut pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&pack_path).expect("read pack for plan repin"))
            .expect("parse pack for plan repin");
    pack.records[0].execution_plan_sha256 = sha256_hex(&plan_bytes);
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize plan-repinned pack"),
    )
    .expect("write plan-repinned pack");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("batch-output"),
        "execution_plan does not match the admitted run_spec and accepted_tranche",
    );
}

#[test]
fn venue_identity_is_bound_opaquely_without_venue_enumeration() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![(0, b"accepted object bytes".to_vec())];
    let (pack_path, run_spec_path, _, execution_plan_path) =
        write_control_admission_fixture(temp_dir.path(), &objects);
    let mut run_spec: RunSpec =
        toml::from_slice(&fs::read(&run_spec_path).expect("read run spec for venue mutation"))
            .expect("parse run spec for venue mutation");
    run_spec.source_proof.venue = "second-neutral-venue".to_string();
    let run_spec_bytes = toml::to_string_pretty(&run_spec)
        .expect("serialize venue-mutated run spec")
        .into_bytes();
    fs::write(&run_spec_path, &run_spec_bytes).expect("write venue-mutated run spec");
    let run_spec_sha256 = sha256_hex(&run_spec_bytes);
    let mut plan: BackfillExecutionPlan = serde_json::from_slice(
        &fs::read(&execution_plan_path).expect("read plan for run-spec repin"),
    )
    .expect("parse plan for run-spec repin");
    plan.run_spec_hash.clone_from(&run_spec_sha256);
    let plan_bytes = serde_json::to_vec_pretty(&plan).expect("serialize run-spec-repinned plan");
    fs::write(&execution_plan_path, &plan_bytes).expect("write run-spec-repinned plan");
    let mut pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&pack_path).expect("read pack for venue repin"))
            .expect("parse pack for venue repin");
    pack.venue = "second-neutral-venue".to_string();
    pack.records[0].run_spec_sha256 = run_spec_sha256;
    pack.records[0].execution_plan_sha256 = sha256_hex(&plan_bytes);
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize venue-repinned pack"),
    )
    .expect("write venue-repinned pack");
    write_source_binding_registry(
        &temp_dir.path().join("source-bindings.toml"),
        "second-neutral-venue",
    );

    let mut fetcher = SequencedFetcher::from_objects(&objects);
    execute_source_universe_batch(
        "source-universe-batch-second-neutral-venue",
        &pack_path,
        &temp_dir.path().join("batch-output"),
        None,
        test_control_admission_policy(temp_dir.path()),
        &mut fetcher,
        &mut RecordingRunner::default(),
    )
    .expect("a second opaque venue identity follows the same admission path");
}

#[test]
fn admission_rejection_leaves_preexisting_output_unchanged() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, _) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    let mut pack: serde_json::Value =
        serde_json::from_slice(&fs::read(&pack_path).expect("read pack for mismatch"))
            .expect("parse pack for mismatch");
    pack["records"][0]["operator_run_id"] = serde_json::json!("different-run");
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize mismatched pack"),
    )
    .expect("write mismatched pack");
    let output_dir = temp_dir.path().join("preexisting-output");
    fs::create_dir_all(&output_dir).expect("create preexisting output");
    let sentinel_path = output_dir.join("sentinel");
    fs::write(&sentinel_path, b"unchanged").expect("write output sentinel");
    let mut runner = RecordingRunner::default();

    execute_source_universe_batch(
        "source-universe-batch-preexisting-output",
        &pack_path,
        &output_dir,
        None,
        test_control_admission_policy(temp_dir.path()),
        &mut NeverFetcher,
        &mut runner,
    )
    .expect_err("identity mismatch must reject before output mutation");
    assert_eq!(
        fs::read(&sentinel_path).expect("read unchanged sentinel"),
        b"unchanged"
    );
    assert!(
        !output_dir
            .join("source-universe-batch-execution-report.json")
            .exists()
    );
    assert!(runner.calls.is_empty());
}

#[test]
fn control_and_batch_byte_ceilings_enforce_exact_boundaries() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let objects = vec![(0, b"accepted object bytes".to_vec())];
    let (pack_path, run_spec_path, tranche_path, plan_path) =
        write_control_admission_fixture(temp_dir.path(), &objects);
    let run_bytes = u64::try_from(fs::read(&run_spec_path).expect("read run spec").len())
        .expect("run-spec length fits u64");
    let tranche_bytes = u64::try_from(fs::read(&tranche_path).expect("read tranche").len())
        .expect("tranche length fits u64");
    let plan_bytes = u64::try_from(fs::read(&plan_path).expect("read plan").len())
        .expect("plan length fits u64");
    let registry_bytes = fs::metadata(temp_dir.path().join("source-bindings.toml"))
        .expect("source-bindings metadata")
        .len();
    let total_bytes = registry_bytes
        .checked_add(run_bytes)
        .and_then(|total| total.checked_add(tranche_bytes))
        .and_then(|total| total.checked_add(plan_bytes))
        .expect("control total fits u64");
    let exact_policy = test_control_admission_policy_with_limits(
        temp_dir.path(),
        run_bytes,
        tranche_bytes,
        plan_bytes,
        total_bytes,
    );
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    execute_source_universe_batch(
        "source-universe-batch-exact-control-limits",
        &pack_path,
        &temp_dir.path().join("exact-output"),
        None,
        exact_policy,
        &mut fetcher,
        &mut runner,
    )
    .expect("exact control and total ceilings admit");

    for (role, limits) in [
        (
            "run_spec",
            (run_bytes - 1, tranche_bytes, plan_bytes, total_bytes),
        ),
        (
            "accepted_tranche",
            (run_bytes, tranche_bytes - 1, plan_bytes, total_bytes),
        ),
        (
            "execution_plan",
            (run_bytes, tranche_bytes, plan_bytes - 1, total_bytes),
        ),
    ] {
        let policy = test_control_admission_policy_with_limits(
            temp_dir.path(),
            limits.0,
            limits.1,
            limits.2,
            limits.3,
        );
        assert_control_admission_rejects_with_policy(
            &pack_path,
            &temp_dir.path().join(format!("over-{role}-output")),
            policy,
            "exceeds configured byte limit",
        );
    }

    let aggregate_temp = tempfile::tempdir().expect("aggregate temp dir");
    let aggregate_objects = vec![
        (0, b"first accepted object bytes".to_vec()),
        (1, b"second accepted object bytes".to_vec()),
    ];
    let aggregate_pack_path = aggregate_temp
        .path()
        .join("source-universe-execution-pack.json");
    write_n_record_pack(&aggregate_pack_path, &aggregate_objects);
    let pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&aggregate_pack_path).expect("read aggregate pack"))
            .expect("parse aggregate pack");
    let mut role_max = [0_u64; 3];
    let mut aggregate_total = fs::metadata(aggregate_temp.path().join("source-bindings.toml"))
        .expect("aggregate source-bindings metadata")
        .len();
    for record in &pack.records {
        for (index, path) in record.artifact_paths().into_iter().enumerate() {
            let length = fs::metadata(path).expect("control metadata").len();
            role_max[index] = role_max[index].max(length);
            aggregate_total = aggregate_total
                .checked_add(length)
                .expect("aggregate control bytes fit u64");
        }
    }
    let aggregate_policy = test_control_admission_policy_with_limits(
        aggregate_temp.path(),
        role_max[0],
        role_max[1],
        role_max[2],
        aggregate_total,
    );
    let mut aggregate_fetcher = SequencedFetcher::from_objects(&aggregate_objects);
    execute_source_universe_batch(
        "source-universe-batch-exact-aggregate-limit",
        &aggregate_pack_path,
        &aggregate_temp.path().join("exact-aggregate-output"),
        None,
        aggregate_policy,
        &mut aggregate_fetcher,
        &mut RecordingRunner::default(),
    )
    .expect("exact batch aggregate ceiling admits");
    let over_policy = test_control_admission_policy_with_limits(
        aggregate_temp.path(),
        role_max[0],
        role_max[1],
        role_max[2],
        aggregate_total - 1,
    );
    assert_control_admission_rejects_with_policy(
        &aggregate_pack_path,
        &aggregate_temp.path().join("over-aggregate-output"),
        over_policy,
        "exceed configured max_total_control_bytes",
    );

    let first_record_total = pack.records[0]
        .artifact_paths()
        .into_iter()
        .try_fold(0_u64, |total, path| {
            total.checked_add(fs::metadata(path).expect("first control metadata").len())
        })
        .expect("first record control total fits u64");
    let second_run_spec_bytes = fs::metadata(&pack.records[1].run_spec_path)
        .expect("second run-spec metadata")
        .len();
    let during_read_limit = fs::metadata(aggregate_temp.path().join("source-bindings.toml"))
        .expect("aggregate source-bindings metadata")
        .len()
        .checked_add(first_record_total)
        .and_then(|total| total.checked_add(second_run_spec_bytes))
        .and_then(|total| total.checked_sub(1))
        .expect("during-read aggregate limit fits u64");
    assert!(
        role_max
            .into_iter()
            .all(|individual| during_read_limit >= individual),
        "during-read aggregate policy must remain structurally valid"
    );
    fs::remove_file(&pack.records[1].accepted_tranche_path)
        .expect("remove later control to prove it is never opened");
    let during_read_policy = test_control_admission_policy_with_limits(
        aggregate_temp.path(),
        role_max[0],
        role_max[1],
        role_max[2],
        during_read_limit,
    );
    assert_control_admission_rejects_with_policy(
        &aggregate_pack_path,
        &aggregate_temp.path().join("during-read-aggregate-output"),
        during_read_policy,
        "exceed configured max_total_control_bytes while reading run_spec",
    );
}

#[test]
fn malformed_control_policies_fail_closed() {
    let cases = [
        (
            "missing-key",
            "[control_admission]\nmax_run_spec_bytes = 1\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
        (
            "missing-source-bindings-path",
            "[control_admission]\nmax_run_spec_bytes = 1\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\n",
        ),
        (
            "unknown-key",
            "[control_admission]\nmax_run_spec_bytes = 1\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\nextra = 1\n",
        ),
        (
            "zero",
            "[control_admission]\nmax_run_spec_bytes = 0\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
        (
            "zero-source-bindings-limit",
            "[control_admission]\nmax_run_spec_bytes = 1\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 0\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
        (
            "malformed",
            "[control_admission]\nmax_run_spec_bytes = \"large\"\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
        (
            "negative",
            "[control_admission]\nmax_run_spec_bytes = -1\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
        (
            "out-of-range",
            "[control_admission]\nmax_run_spec_bytes = 999999999999999999999999\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
        (
            "contradictory",
            "[control_admission]\nmax_run_spec_bytes = 2\nmax_accepted_tranche_bytes = 1\nmax_execution_plan_bytes = 1\nmax_source_bindings_bytes = 1\nmax_total_control_bytes = 1\nsource_bindings_path = \"source-bindings.toml\"\n",
        ),
    ];
    for (name, toml) in cases {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let policy_path = temp_dir.path().join(format!("{name}.toml"));
        fs::write(&policy_path, toml).expect("write malformed policy");
        load_source_universe_control_admission_policy(&policy_path)
            .expect_err("malformed policy must reject");
    }
}

#[test]
fn oversized_source_bindings_registry_fails_policy_load() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    let registry_bytes = fs::metadata(temp_dir.path().join("source-bindings.toml"))
        .expect("source-bindings metadata")
        .len();
    let policy_path = temp_dir.path().join("oversized-registry-policy.toml");
    fs::write(
        &policy_path,
        format!(
            "[control_admission]\n\
             max_run_spec_bytes = 1000000\n\
             max_accepted_tranche_bytes = 1000000\n\
             max_execution_plan_bytes = 1000000\n\
             max_source_bindings_bytes = {}\n\
             max_total_control_bytes = 10000000\n\
             source_bindings_path = \"source-bindings.toml\"\n",
            registry_bytes - 1
        ),
    )
    .expect("write oversized-registry policy");

    let error = load_source_universe_control_admission_policy(&policy_path)
        .expect_err("registry over its trusted ceiling must reject");
    assert!(
        format!("{error:#}").contains("source_bindings exceeds configured byte limit"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn run_spec_cannot_select_a_different_source_bindings_registry() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, run_spec_path, _, plan_path) =
        write_control_admission_fixture(temp_dir.path(), &[(0, b"accepted object bytes".to_vec())]);
    let alternate_registry_path = temp_dir.path().join("alternate-source-bindings.toml");
    fs::copy(
        temp_dir.path().join("source-bindings.toml"),
        &alternate_registry_path,
    )
    .expect("copy alternate source-bindings registry");
    let mut run_spec: RunSpec =
        toml::from_slice(&fs::read(&run_spec_path).expect("read run spec for registry mutation"))
            .expect("parse run spec for registry mutation");
    run_spec.source_bindings_path = alternate_registry_path;
    let run_spec_bytes = toml::to_string_pretty(&run_spec)
        .expect("serialize registry-mutated run spec")
        .into_bytes();
    let run_spec_sha256 = sha256_hex(&run_spec_bytes);
    fs::write(&run_spec_path, &run_spec_bytes).expect("write registry-mutated run spec");
    let mut plan: BackfillExecutionPlan = serde_json::from_slice(
        &fs::read(&plan_path).expect("read execution plan for registry mutation"),
    )
    .expect("parse execution plan for registry mutation");
    plan.run_spec_hash.clone_from(&run_spec_sha256);
    let plan_bytes = serde_json::to_vec_pretty(&plan).expect("serialize registry-mutated plan");
    fs::write(&plan_path, &plan_bytes).expect("write registry-mutated plan");
    let mut pack: SourceUniverseExecutionPack = serde_json::from_slice(
        &fs::read(&pack_path).expect("read execution pack for registry mutation"),
    )
    .expect("parse execution pack for registry mutation");
    pack.records[0].run_spec_sha256 = run_spec_sha256;
    pack.records[0].execution_plan_sha256 = sha256_hex(&plan_bytes);
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize registry-mutated pack"),
    )
    .expect("write registry-mutated pack");

    assert_control_admission_rejects_before_side_effects(
        &pack_path,
        &temp_dir.path().join("registry-mismatch-output"),
        "does not match trusted control policy registry",
    );
}

#[test]
fn parallel_and_resume_paths_cannot_bypass_cross_binding() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let (pack_path, _, _, _) = write_control_admission_fixture(
        temp_dir.path(),
        &[
            (0, b"first accepted object bytes".to_vec()),
            (1, b"second accepted object bytes".to_vec()),
        ],
    );
    let mut pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&pack_path).expect("read execution pack for splice"))
            .expect("parse execution pack for splice");
    let spliced_run_spec_path = pack.records[1].run_spec_path.clone();
    let spliced_run_spec_sha256 = pack.records[1].run_spec_sha256.clone();
    pack.records[0].run_spec_path = spliced_run_spec_path;
    pack.records[0].run_spec_sha256 = spliced_run_spec_sha256;
    fs::write(
        &pack_path,
        serde_json::to_vec_pretty(&pack).expect("serialize spliced execution pack"),
    )
    .expect("write spliced execution pack");

    let parallel_output = temp_dir.path().join("parallel-output");
    let parallel_error = execute_source_universe_batch_with_factories(
        "source-universe-batch-parallel-admission",
        &pack_path,
        &parallel_output,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: Some(2),
            resume_report: None,
        },
        || Ok(NeverFetcher),
        || Ok(RecordingRunner::default()),
    )
    .expect_err("parallel path must reject the splice at admission");
    assert!(
        format!("{parallel_error:#}").contains("execution_plan does not match"),
        "parallel error must identify cross-binding: {parallel_error:#}"
    );
    assert!(!parallel_output.exists());

    let resume_output = temp_dir.path().join("resume-output");
    let mut runner = RecordingRunner::default();
    let resume_error = execute_source_universe_batch_with_config(
        "source-universe-batch-resume-admission",
        &pack_path,
        &resume_output,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
            start_sequence: None,
            record_limit: None,
            continue_on_error: false,
            max_concurrent_records: None,
            resume_report: Some(temp_dir.path().join("unread-resume-report.json")),
        },
        &mut NeverFetcher,
        &mut runner,
    )
    .expect_err("resume path must reject the splice before reading resume state");
    assert!(
        format!("{resume_error:#}").contains("execution_plan does not match"),
        "resume error must identify cross-binding: {resume_error:#}"
    );
    assert!(!resume_output.exists());
    assert!(runner.calls.is_empty());
}

#[test]
fn source_universe_batch_execution_respects_start_sequence() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    let output_dir = temp_dir.path().join("batch-output");
    let first_object_bytes = b"first accepted object bytes";
    let second_object_bytes = b"second accepted object bytes";

    write_two_record_pack(&pack_path, first_object_bytes, second_object_bytes);

    let mut fetcher = SequencedFetcher::from_objects(&[(1, second_object_bytes.to_vec())]);
    let mut runner = RecordingRunner::default();

    let report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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
    let output_dir = temp_dir.path().join("batch-output");
    let first_object_bytes = b"first accepted object bytes";
    let second_object_bytes = b"second accepted object bytes";

    write_two_record_pack(&pack_path, first_object_bytes, second_object_bytes);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects = vec![
        (0u64, b"synthetic object zero".to_vec()),
        (1u64, b"synthetic object one".to_vec()),
    ];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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
    fs::copy(
        committed_reference_run_dir().join("catalog-metadata.json"),
        prior_output_dir.join("catalog-metadata.json"),
    )
    .expect("copy committed catalog metadata");
    first_report.records[0].catalog_metadata_sha256 = sha256_hex(
        &fs::read(prior_output_dir.join("catalog-metadata.json"))
            .expect("read copied catalog metadata"),
    );
    first_report.records[0].operator_run_id = "forged-prior-run".to_string();
    first_report.records[0].source_binding = "forged-prior-binding".to_string();
    first_report.records[0].symbol = "FORGED".to_string();
    first_report.records[0].archive_date = "1900-01-01".to_string();
    first_report.records[0].selected_object_bytes = u64::MAX;
    first_report.records[0].canonical_rows = u64::MAX;
    first_report.records[0].nt_catalog_rows = u64::MAX;

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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
        resume_report.total_canonical_rows, 110,
        "totals derive carried rows from verified catalog metadata"
    );
    assert_eq!(resume_report.total_nt_catalog_rows, 111);
    assert_eq!(
        resume_report.records[0].sequence, 0,
        "carried record stays in order"
    );
    assert_eq!(resume_report.records[1].sequence, 1);
    assert_eq!(
        resume_report.records[0].operator_run_id,
        "source-universe-operator-run-synthetic-00000"
    );
    assert_eq!(
        resume_report.records[0].source_binding,
        "synthetic-spot-tick-trades"
    );
    assert_eq!(resume_report.records[0].symbol, "SYNTHETIC-AAA");
    assert_eq!(resume_report.records[0].archive_date, "2026-03-01");
    assert_eq!(
        resume_report.records[0].selected_object_bytes,
        u64::try_from(objects[0].1.len()).expect("object length")
    );
    assert_eq!(resume_report.records[0].canonical_rows, 103);
    assert_eq!(resume_report.records[0].nt_catalog_rows, 104);
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

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects: Vec<(u64, Vec<u8>)> = (0..8u64)
        .map(|sequence| {
            (
                sequence,
                format!("synthetic object {sequence}").into_bytes(),
            )
        })
        .collect();
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

    // Serial baseline.
    let serial_output = temp_dir.path().join("batch-output-serial");
    let serial_objects = objects.clone();
    let serial_report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &serial_output,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects: Vec<(u64, Vec<u8>)> = (0..6u64)
        .map(|sequence| {
            (
                sequence,
                format!("synthetic object {sequence}").into_bytes(),
            )
        })
        .collect();
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

    // Fail sequences 2 and 4 in the runner; lowest-sequence error (2) must surface.
    let output_dir = temp_dir.path().join("batch-output");
    let result = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects: Vec<(u64, Vec<u8>)> = (0..6u64)
        .map(|sequence| {
            (
                sequence,
                format!("synthetic object {sequence}").into_bytes(),
            )
        })
        .collect();
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

    let output_dir = temp_dir.path().join("batch-output");
    let report = execute_source_universe_batch_with_factories(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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
    write_n_record_pack(&pack_path, &objects);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects = vec![
        (0u64, b"synthetic object zero".to_vec()),
        (1u64, b"synthetic object one".to_vec()),
    ];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

    // Prior partial run writes its report into output_dir.
    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let first_report = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

// ── Fix 1: path-traversal class — sha256 validation at the consume boundary ──

/// A pack record with a `../`-prefixed sha256 field must be rejected at pack
/// consumption, before any fetch or cache activity.
#[test]
fn prepare_batch_rejects_parent_dir_traversal_sha256() {
    let temp_dir = tempfile::tempdir().expect("temp dir");

    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(&pack_path, "../etc/passwd");

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(&pack_path, "/etc/shadow");

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    // 64 chars but uppercase — valid hex encoding but not lowercase sha256.
    let uppercase_sha = "A".repeat(64);
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(&pack_path, &uppercase_sha);

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let short_sha = "a".repeat(63);
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_pack_with_sha256(&pack_path, &short_sha);

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = NeverFetcher;
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

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
            control_admission: test_control_admission_policy(temp_dir.path()),
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

    let objects = vec![(0u64, b"synthetic object zero".to_vec())];
    let pack_path = temp_dir.path().join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, &objects);

    let output_dir = temp_dir.path().join("batch-output");
    let mut fetcher = SequencedFetcher::from_objects(&objects);
    let mut runner = RecordingRunner::default();
    let err = execute_source_universe_batch_with_config(
        "source-universe-batch-synthetic",
        &pack_path,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission: test_control_admission_policy(temp_dir.path()),
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

struct EmptyDirectoryRunner;

impl SourceUniverseOperatorRunner for EmptyDirectoryRunner {
    fn run(
        &mut self,
        _record: &SourceUniverseExecutionPackRecord,
        _object_bytes: &[u8],
        _controls: &SourceUniverseAdmittedControls,
        output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        anyhow::ensure!(
            output_dir.is_dir(),
            "runner output directory must be a real directory"
        );
        anyhow::ensure!(
            fs::read_dir(output_dir)?.next().is_none(),
            "runner output directory must be empty"
        );
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: "catalog-hash".to_string(),
            catalog_metadata_sha256: "b".repeat(64),
        })
    }
}

struct RunCall {
    operator_run_id: String,
    object_bytes: Vec<u8>,
    run_spec_bytes: Vec<u8>,
    accepted_tranche_bytes: Vec<u8>,
    execution_plan_bytes: Vec<u8>,
    run_spec_run_id: String,
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
        controls: &SourceUniverseAdmittedControls,
        output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        self.calls.push(RunCall {
            operator_run_id: record.operator_run_id.clone(),
            object_bytes: object_bytes.to_vec(),
            run_spec_bytes: controls.run_spec_bytes().to_vec(),
            accepted_tranche_bytes: controls.accepted_tranche_bytes().to_vec(),
            execution_plan_bytes: controls.execution_plan_bytes().to_vec(),
            run_spec_run_id: controls.run_spec().manifest.run_id.clone(),
            output_dir: output_dir.to_path_buf(),
        });
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: "catalog-hash".to_string(),
            catalog_metadata_sha256: "b".repeat(64),
        })
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    hex::encode(Sha256::digest(bytes))
}

fn reference_binance_zip_object() -> Vec<u8> {
    const MEMBER: &str = "0GTRY-trades-2026-03-01.csv";
    const CSV: &str = "101735393,617.34000000,1.61900000,999.47346000,1772323201711256,True,True\n\
        101735394,617.34000000,0.07200000,44.44848000,1772323201815330,False,True\n";
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    writer
        .start_file(MEMBER, zip::write::FileOptions::default())
        .expect("start reference ZIP member");
    writer
        .write_all(CSV.as_bytes())
        .expect("write reference CSV");
    writer.finish().expect("finish reference ZIP").into_inner()
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

fn write_two_record_pack(pack_path: &Path, first_object_bytes: &[u8], second_object_bytes: &[u8]) {
    write_n_record_pack(
        pack_path,
        &[
            (0, first_object_bytes.to_vec()),
            (1, second_object_bytes.to_vec()),
        ],
    );
}

fn write_control_admission_fixture(
    root: &Path,
    objects: &[(u64, Vec<u8>)],
) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let pack_path = root.join("source-universe-execution-pack.json");
    write_n_record_pack(&pack_path, objects);
    let (run_spec_path, accepted_tranche_path, execution_plan_path) =
        synthetic_control_paths(root, objects[0].0);
    (
        pack_path,
        run_spec_path,
        accepted_tranche_path,
        execution_plan_path,
    )
}

fn assert_control_admission_rejects_before_side_effects(
    pack_path: &Path,
    output_dir: &Path,
    expected_error: &str,
) {
    let policy = test_control_admission_policy(
        pack_path
            .parent()
            .expect("execution pack parent for policy"),
    );
    assert_control_admission_rejects_with_policy(pack_path, output_dir, policy, expected_error);
}

fn assert_control_admission_rejects_with_policy(
    pack_path: &Path,
    output_dir: &Path,
    policy: SourceUniverseControlAdmissionPolicy,
    expected_error: &str,
) {
    let mut runner = RecordingRunner::default();
    let error = execute_source_universe_batch(
        "source-universe-batch-synthetic",
        pack_path,
        output_dir,
        None,
        policy,
        &mut NeverFetcher,
        &mut runner,
    )
    .expect_err("invalid controls must reject before fetch");
    assert!(
        format!("{error:#}").contains(expected_error),
        "full error chain must contain {expected_error:?}: {error:#}"
    );
    assert!(
        !output_dir.exists(),
        "invalid admission must create no output"
    );
    assert!(runner.calls.is_empty(), "invalid admission must not run");
}

/// Synthetic per-sequence symbol so the two-record assertions keep working
/// (sequence 0 = `SYNTHETIC-AAA`, sequence 1 = `SYNTHETIC-BBB`, ...).
fn synthetic_symbol(sequence: u64) -> String {
    let letter = char::from(b'A' + (sequence % 26) as u8);
    format!("SYNTHETIC-{letter}{letter}{letter}")
}

fn write_n_record_pack(pack_path: &Path, objects: &[(u64, Vec<u8>)]) {
    let root = pack_path.parent().expect("pack parent");
    let source_bindings_path = root.join("source-bindings.toml");
    write_source_binding_registry(&source_bindings_path, "synthetic-venue");
    let record_count = u64::try_from(objects.len()).expect("record count fits u64");
    let total_object_bytes_len = objects
        .iter()
        .try_fold(0_u64, |total, (_, bytes)| {
            total.checked_add(u64::try_from(bytes.len()).expect("object length fits u64"))
        })
        .expect("total object bytes fit u64");
    let records = objects
        .iter()
        .map(|(sequence, bytes)| {
            write_synthetic_controls(root, &source_bindings_path, *sequence, bytes)
        })
        .collect::<Vec<_>>();

    let mut pack = committed_execution_pack_template();
    pack.pack_id = "source-universe-execution-pack-synthetic".to_string();
    pack.status = SourceUniverseExecutionPackStatus::Ready;
    pack.work_order_id = "source-universe-conversion-work-order-synthetic".to_string();
    pack.input_id = "source-universe-operator-inputs-synthetic".to_string();
    pack.gate_id = "source-universe-object-gates-synthetic".to_string();
    pack.conversion_run_plan_id = "source-universe-conversion-run-plan-synthetic".to_string();
    pack.universe_id = "backfill-source-universe-synthetic".to_string();
    pack.venue = "synthetic-venue".to_string();
    pack.source = "public_archive".to_string();
    pack.family = "tick_trades".to_string();
    pack.table_family = "trades".to_string();
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
        serde_json::to_vec_pretty(&pack).expect("serialize pack"),
    )
    .expect("write n-record pack");
}

fn write_source_binding_registry(path: &Path, venue: &str) {
    let registry = format!(
        r#"
[[source_binding]]
key = "synthetic-spot-tick-trades"
venue = "{venue}"
product_family = "spot"
market_structure_fixture = "perps-spot"
source_uri = "https://public.synthetic.example/object-{{sequence}}.csv.gz"
evidence_state = "directly_backfillable"
table_families = ["trades"]
"#
    );
    fs::write(path, registry).expect("write synthetic source-binding registry");
}

fn synthetic_control_paths(root: &Path, sequence: u64) -> (PathBuf, PathBuf, PathBuf) {
    (
        root.join(format!("run-spec-{sequence}.toml")),
        root.join(format!("accepted-tranche-{sequence}.json")),
        root.join(format!("execution-plan-{sequence}.json")),
    )
}

fn committed_execution_pack_template() -> SourceUniverseExecutionPack {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .join(
            "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs/\
             binance-data-vision-trades-2026-03-01-all-instruments/execution-pack/\
             source-universe-execution-pack.json",
        );
    serde_json::from_slice(&fs::read(path).expect("read committed execution-pack template"))
        .expect("parse committed execution-pack template")
}

fn committed_control_templates() -> (
    RunSpec,
    BackfillAcceptedTrancheManifest,
    BackfillExecutionPlan,
) {
    let pack = committed_execution_pack_template();
    let record = pack.records.first().expect("committed pack record");
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root");
    let run_spec_text = fs::read_to_string(repo_root.join(&record.run_spec_path))
        .expect("read committed run-spec template");
    let run_spec = toml::from_str(&run_spec_text).expect("parse committed run-spec template");
    let tranche = serde_json::from_slice(
        &fs::read(repo_root.join(&record.accepted_tranche_path))
            .expect("read committed accepted-tranche template"),
    )
    .expect("parse committed accepted-tranche template");
    let plan = serde_json::from_slice(
        &fs::read(repo_root.join(&record.execution_plan_path))
            .expect("read committed execution-plan template"),
    )
    .expect("parse committed execution-plan template");
    (run_spec, tranche, plan)
}

fn write_synthetic_controls(
    root: &Path,
    source_bindings_path: &Path,
    sequence: u64,
    object_bytes: &[u8],
) -> SourceUniverseExecutionPackRecord {
    let (mut run_spec, mut tranche, plan_template) = committed_control_templates();
    let operator_run_id = format!("source-universe-operator-run-synthetic-{sequence:05}");
    let tranche_id = format!("accepted-tranche-synthetic-{sequence}");
    let symbol = synthetic_symbol(sequence);
    let object_sha256 = sha256_hex(object_bytes);
    let object_bytes_len = u64::try_from(object_bytes.len()).expect("object bytes fit u64");
    let source_uri = format!("s3://synthetic-bucket/raw/synthetic-{sequence}.csv.gz");
    let source_url = format!("https://public.synthetic.example/object-{sequence}.csv.gz");
    let output_prefix = format!(
        "{}/backtests/synthetic-{sequence}",
        run_spec.manifest.artifact_root.trim_end_matches('/')
    );

    run_spec.manifest.run_id.clone_from(&operator_run_id);
    run_spec.source_bindings_path = source_bindings_path.to_path_buf();
    run_spec.manifest.output_prefix.clone_from(&output_prefix);
    run_spec.manifest.source_proof_id = "source-proof-synthetic".to_string();
    run_spec.manifest.source_proof_version = 1;
    run_spec.manifest.venue_binding_key = "synthetic-spot-tick-trades".to_string();
    run_spec.source_proof.source_proof_id = "source-proof-synthetic".to_string();
    run_spec.source_proof.source_proof_version = 1;
    run_spec.source_proof.source_binding = "synthetic-spot-tick-trades".to_string();
    run_spec.source_proof.venue = "synthetic-venue".to_string();
    run_spec.source_proof.product_category = "spot".to_string();
    run_spec.source_proof.table_family = "trades".to_string();
    run_spec.source_proof.raw_sample_uri.clone_from(&source_uri);
    run_spec
        .source_proof
        .raw_sample_hash
        .clone_from(&object_sha256);
    run_spec.accepted_object.s3_uri.clone_from(&source_uri);
    run_spec.accepted_object.source_url.clone_from(&source_url);
    run_spec.accepted_object.sha256.clone_from(&object_sha256);
    run_spec.accepted_object.bytes = object_bytes_len;
    run_spec.accepted_object.archive_date = "2026-03-01".to_string();
    run_spec.converter.raw_payload.max_object_bytes = object_bytes_len;
    match &mut run_spec.identity {
        RunSpecInstrumentIdentities::Single(identity) => {
            identity.instrument_id.clone_from(&symbol);
            identity.venue_symbol.clone_from(&symbol);
        }
        RunSpecInstrumentIdentities::Keyed(_) => panic!("committed run-spec must be single"),
    }

    let tranche_object = BackfillAcceptedTrancheObject {
        s3_uri: source_uri.clone(),
        source_url: source_url.clone(),
        sha256: object_sha256.clone(),
        bytes: object_bytes_len,
        archive_date: "2026-03-01".to_string(),
        source_row_groups: Vec::new(),
        predicate_ref: None,
    };
    tranche.schema_version = BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION.to_string();
    tranche.tranche_id.clone_from(&tranche_id);
    tranche.status = BackfillAcceptedTrancheStatus::Accepted;
    tranche.source_proof_id = "source-proof-synthetic".to_string();
    tranche.source_proof_version = 1;
    tranche.source_binding = "synthetic-spot-tick-trades".to_string();
    tranche.table_family = "trades".to_string();
    tranche.source_usage_scope = run_spec.source_proof.usage_scope;
    tranche.object_count = 1;
    tranche.accepted_bytes = object_bytes_len;
    tranche.objects = vec![tranche_object];
    tranche.blocking_issues.clear();

    let run_spec_bytes = toml::to_string_pretty(&run_spec)
        .expect("serialize synthetic run-spec")
        .into_bytes();
    let accepted_tranche_bytes =
        serde_json::to_vec_pretty(&tranche).expect("serialize synthetic accepted tranche");
    let run_spec_sha256 = sha256_hex(&run_spec_bytes);
    let accepted_tranche_sha256 = sha256_hex(&accepted_tranche_bytes);
    let execution_plan = evaluate_backfill_execution_plan(
        format!("{operator_run_id}:execution-plan"),
        accepted_tranche_sha256.clone(),
        &tranche,
        run_spec_sha256.clone(),
        &BackfillExecutionRunBinding::from_run_spec(&run_spec),
        BackfillExecutionWorkBudget {
            max_source_rows: plan_template.max_source_rows,
            max_projected_row_groups: plan_template.max_projected_row_groups,
            max_wall_seconds: plan_template.max_wall_seconds,
            require_object_selection_metadata: false,
        },
    );
    let execution_plan_bytes =
        serde_json::to_vec_pretty(&execution_plan).expect("serialize synthetic execution plan");
    let (run_spec_path, accepted_tranche_path, execution_plan_path) =
        synthetic_control_paths(root, sequence);
    fs::write(&run_spec_path, &run_spec_bytes).expect("write synthetic run spec");
    fs::write(&accepted_tranche_path, &accepted_tranche_bytes)
        .expect("write synthetic accepted tranche");
    fs::write(&execution_plan_path, &execution_plan_bytes).expect("write synthetic execution plan");

    SourceUniverseExecutionPackRecord {
        sequence,
        work_item_id: format!("synthetic-work-item-{sequence}"),
        operator_run_id,
        source_binding: "synthetic-spot-tick-trades".to_string(),
        category: "spot".to_string(),
        symbol,
        archive_date: "2026-03-01".to_string(),
        source_uri,
        source_url,
        selected_object_sha256: object_sha256,
        selected_object_bytes: object_bytes_len,
        source_proof_id: "source-proof-synthetic".to_string(),
        source_proof_version: 1,
        accepted_tranche_id: tranche_id,
        output_prefix,
        run_spec_path,
        run_spec_sha256,
        accepted_tranche_path,
        accepted_tranche_sha256,
        execution_plan_path,
        execution_plan_sha256: sha256_hex(&execution_plan_bytes),
    }
}

fn test_control_admission_policy(root: &Path) -> SourceUniverseControlAdmissionPolicy {
    test_control_admission_policy_with_limits(root, 1_000_000, 1_000_000, 1_000_000, 10_000_000)
}

fn test_control_admission_policy_with_limits(
    root: &Path,
    max_run_spec_bytes: u64,
    max_accepted_tranche_bytes: u64,
    max_execution_plan_bytes: u64,
    max_total_control_bytes: u64,
) -> SourceUniverseControlAdmissionPolicy {
    let max_source_bindings_bytes = fs::metadata(root.join("source-bindings.toml"))
        .expect("source-bindings registry exists before policy construction")
        .len();
    let path = root.join(format!(
        "control-admission-{max_run_spec_bytes}-{max_accepted_tranche_bytes}-{max_execution_plan_bytes}-{max_total_control_bytes}.toml"
    ));
    fs::write(
        &path,
        format!(
            "[control_admission]\n\
             max_run_spec_bytes = {max_run_spec_bytes}\n\
             max_accepted_tranche_bytes = {max_accepted_tranche_bytes}\n\
             max_execution_plan_bytes = {max_execution_plan_bytes}\n\
             max_source_bindings_bytes = {max_source_bindings_bytes}\n\
             max_total_control_bytes = {max_total_control_bytes}\n\
             source_bindings_path = \"source-bindings.toml\"\n"
        ),
    )
    .expect("write test control-admission policy");
    load_source_universe_control_admission_policy(&path)
        .expect("load test control-admission policy")
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
        execution_plan_sha256: "a".repeat(64),
        catalog_metadata_sha256: "b".repeat(64),
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
        _controls: &SourceUniverseAdmittedControls,
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
            catalog_metadata_sha256: sha256_hex(
                format!("metadata-hash-{}", record.sequence).as_bytes(),
            ),
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
        _controls: &SourceUniverseAdmittedControls,
        _output_dir: &Path,
    ) -> anyhow::Result<SourceUniverseBatchExecutionRunOutput> {
        if self.failing_sequences.contains(&record.sequence) {
            anyhow::bail!("synthetic runner failure for sequence {}", record.sequence);
        }
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: 7,
            nt_catalog_rows: 7,
            catalog_hash: "catalog-hash".to_string(),
            catalog_metadata_sha256: "b".repeat(64),
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
fn write_pack_with_sha256(pack_path: &Path, sha256_literal: &str) {
    write_source_binding_registry(
        &pack_path
            .parent()
            .expect("literal-sha pack path has parent")
            .join("source-bindings.toml"),
        "synthetic-venue",
    );
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
      "run_spec_path": "run-spec.toml",
      "run_spec_sha256": "run-spec-sha",
      "accepted_tranche_path": "accepted-tranche.json",
      "accepted_tranche_sha256": "accepted-tranche-sha",
      "execution_plan_path": "execution-plan.json",
      "execution_plan_sha256": "execution-plan-sha"
    }}
  ],
  "blocking_reasons": []
}}"#,
        ),
    )
    .expect("write pack with literal sha256");
}
