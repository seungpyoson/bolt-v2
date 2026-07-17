use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use backtesting_vertical_slice::path_resolution::{
    resolve_existing_input_path, resolve_output_dir, resolve_pack_control_path,
};
use backtesting_vertical_slice::source_universe_batch_execution::{
    HttpSourceUniverseObjectFetcher, SOURCE_UNIVERSE_OPERATOR_WORKER_MODE,
    SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT, SourceUniverseBatchArtifactPin,
    SourceUniverseBatchExecutionConfig, SourceUniverseBatchExecutionReportStatus,
    SourceUniverseBatchLaunchArtifacts, SourceUniverseBatchResourceLimits,
    SourceUniverseObjectFetcher, VerifiedSourceObject, WriteThroughSourceUniverseObjectFetcher,
    execute_source_universe_batch_process_isolated, execute_source_universe_operator_worker,
    validate_process_isolated_batch_selection,
};
use backtesting_vertical_slice::source_universe_batch_launch::{
    SourceUniverseBatchLaunchSpec, SourceUniverseBatchTransportSpec,
};
use backtesting_vertical_slice::source_universe_local_storage::acquire_source_universe_local_storage;
use backtesting_vertical_slice::source_universe_object_transport::StagedS3SourceUniverseObjectFetcher;
use clap::Parser;

/// Process exit code when the batch completed but at least one record failed
/// (`continue_on_error` produced a `CompletedWithFailures`/`Failed` report)
/// and `allow_partial` was false. Distinct from anyhow's exit 1 (a hard
/// error before/while assembling the report) so automation can tell a partial
/// data outcome apart from a runner crash.
const EXIT_PARTIAL_FAILURE: i32 = 2;

#[derive(Debug, Parser)]
#[command(about = "Execute source-universe single-object operator runs from an execution pack")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
    #[arg(long)]
    spec_bytes: u64,
    #[arg(long)]
    spec_sha256: String,
}

#[derive(Debug, Parser)]
struct WorkerCli {
    #[arg(long)]
    workspace_owner_lock_fd: i32,
    #[arg(long)]
    request_archive_bytes: u64,
    #[arg(long)]
    request_manifest_bytes: u64,
    #[arg(long)]
    request_manifest_sha256: String,
    #[arg(long)]
    bootstrap_max_bytes: u64,
    #[arg(long)]
    worker_max_virtual_memory_bytes: u64,
}

/// Object fetcher used by every batch worker. Each selected transport fetches
/// its origin and then writes the verified bytes through to the mandatory
/// content-addressed evidence archive.
enum BatchWorkerFetcher {
    Http(WriteThroughSourceUniverseObjectFetcher<HttpSourceUniverseObjectFetcher>),
    StagedS3(WriteThroughSourceUniverseObjectFetcher<StagedS3SourceUniverseObjectFetcher>),
}

impl SourceUniverseObjectFetcher for BatchWorkerFetcher {
    fn fetch(
        &mut self,
        record: &backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPackRecord,
        run_spec: &backtesting_vertical_slice::operator::RunSpec,
        attempt_identity: &str,
        work_budget: &backtesting_vertical_slice::operator_work_budget::OperatorWorkBudgetGuard,
    ) -> Result<VerifiedSourceObject> {
        match self {
            BatchWorkerFetcher::Http(fetcher) => {
                fetcher.fetch(record, run_spec, attempt_identity, work_budget)
            }
            BatchWorkerFetcher::StagedS3(fetcher) => {
                fetcher.fetch(record, run_spec, attempt_identity, work_budget)
            }
        }
    }
}

fn build_batch_worker_fetcher(
    transport: &SourceUniverseBatchTransportSpec,
    fetch_timeout_seconds: u64,
    object_cache_dir: &Path,
) -> Result<BatchWorkerFetcher> {
    match transport {
        SourceUniverseBatchTransportSpec::Https { http_user_agent } => {
            let fetcher = HttpSourceUniverseObjectFetcher::new(
                fetch_timeout_seconds,
                http_user_agent.as_str(),
            )?;
            Ok(BatchWorkerFetcher::Http(
                WriteThroughSourceUniverseObjectFetcher::new(fetcher, object_cache_dir),
            ))
        }
        SourceUniverseBatchTransportSpec::StagedS3 => {
            let fetcher = StagedS3SourceUniverseObjectFetcher::new(fetch_timeout_seconds)?;
            Ok(BatchWorkerFetcher::StagedS3(
                WriteThroughSourceUniverseObjectFetcher::new(fetcher, object_cache_dir),
            ))
        }
    }
}

fn main() -> Result<()> {
    if std::env::args_os().nth(1).is_some_and(|argument| {
        argument == std::ffi::OsStr::new(SOURCE_UNIVERSE_OPERATOR_WORKER_MODE)
    }) {
        let worker = WorkerCli::parse_from(std::env::args_os().skip(1));
        return execute_source_universe_operator_worker(
            worker.workspace_owner_lock_fd,
            worker.request_archive_bytes,
            worker.request_manifest_bytes,
            &worker.request_manifest_sha256,
            worker.bootstrap_max_bytes,
            worker.worker_max_virtual_memory_bytes,
        );
    }
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let pinned = SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
        &spec_path,
        cli.spec_bytes,
        &cli.spec_sha256,
    )?;
    run_batch(&pinned.canonical_path, pinned.spec)
}

fn require_absolute_output_root(output_dir: &Path) -> Result<PathBuf> {
    ensure!(
        output_dir.is_absolute(),
        "resolved batch output root must be absolute: {}",
        output_dir.display()
    );
    Ok(output_dir.to_path_buf())
}

fn run_batch(spec_path: &Path, spec: SourceUniverseBatchLaunchSpec) -> Result<()> {
    let SourceUniverseBatchLaunchSpec {
        schema_version: _,
        batch_id,
        execution_pack,
        output_dir: declared_output_dir,
        start_sequence,
        record_limit,
        continue_on_error,
        fetch_timeout_seconds,
        worker_termination_grace_seconds,
        max_concurrent_records,
        transport,
        object_cache_dir: declared_object_cache_dir,
        allow_partial,
        bootstrap_limits,
        resource_limits,
        local_storage,
    } = spec;
    validate_process_isolated_batch_selection(record_limit, Some(max_concurrent_records))?;
    ensure!(
        fetch_timeout_seconds > 0,
        "batch launch spec fetch_timeout_seconds must be positive"
    );
    ensure!(
        worker_termination_grace_seconds > 0,
        "batch launch spec worker_termination_grace_seconds must be positive"
    );
    transport.validate()?;
    let base_dir = spec_path
        .parent()
        .context("batch launch spec path must have a parent directory")?;
    let execution_pack_path = resolve_execution_pack_path(spec_path, &execution_pack.path)?;
    let declared_output_dir = resolve_output_dir(base_dir, &declared_output_dir);
    let declared_object_cache_dir = resolve_output_dir(base_dir, &declared_object_cache_dir);
    let local_storage_lease = acquire_source_universe_local_storage(
        &local_storage,
        base_dir,
        &declared_output_dir,
        &declared_object_cache_dir,
    )?;
    let output_dir = local_storage_lease.output_root().to_path_buf();
    let object_cache_dir = local_storage_lease.cache_root().to_path_buf();
    let worker_cache_dir = object_cache_dir.clone();

    let fetcher_factory = move || -> Result<BatchWorkerFetcher> {
        build_batch_worker_fetcher(
            &transport,
            fetch_timeout_seconds,
            worker_cache_dir.as_path(),
        )
    };
    let absolute_output_dir = require_absolute_output_root(&output_dir)?;
    let request_root = absolute_output_dir.join(SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT);
    let execution_pack = SourceUniverseBatchArtifactPin::try_new(
        execution_pack_path,
        execution_pack.bytes,
        execution_pack.sha256,
    )?;
    let launch_artifacts =
        SourceUniverseBatchLaunchArtifacts::try_new(execution_pack, bootstrap_limits)?;

    let published = execute_source_universe_batch_process_isolated(
        &batch_id,
        &launch_artifacts,
        &output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence,
            record_limit,
            continue_on_error,
            max_concurrent_records: Some(max_concurrent_records),
        },
        fetcher_factory,
        request_root,
        worker_termination_grace_seconds,
        resource_limits,
        &local_storage,
        &local_storage_lease,
        local_storage.lifecycle_cleanup_limits(),
    )?;
    let report = published.report;
    let artifact = published.artifact;
    println!(
        "source_universe_batch_execution_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("pack_id = {}", report.pack_id);
    println!("selected_records = {}", report.selected_record_count);
    println!("completed_records = {}", report.completed_record_count);
    println!("failed_records = {}", report.failed_record_count);
    println!("total_canonical_rows = {}", report.total_canonical_rows);
    println!("total_nt_catalog_rows = {}", report.total_nt_catalog_rows);

    // The report is written before exiting non-zero, so a partial-failure run
    // still leaves its full evidence on disk. Derive the exit from the report
    // status (not an unconditional `Ok(())`) so `continue_on_error` cannot mask
    // a `CompletedWithFailures`/`Failed` outcome from automation.
    if let Some(exit_code) =
        partial_failure_exit_code(report.status, report.failed_record_count, allow_partial)
    {
        eprintln!(
            "batch completed with {} failed record(s); status {:?}. \
             Set allow_partial=true in the launch spec to exit zero.",
            report.failed_record_count, report.status
        );
        std::process::exit(exit_code);
    }
    Ok(())
}

fn resolve_execution_pack_path(spec_path: &Path, declared_path: &Path) -> Result<PathBuf> {
    let spec_parent = spec_path
        .parent()
        .context("batch launch spec path must have a parent directory")?;
    resolve_pack_control_path(spec_parent, declared_path).with_context(|| {
        format!(
            "resolve execution pack {} from launch spec parent {}",
            declared_path.display(),
            spec_parent.display()
        )
    })
}

/// Map a finished batch report to its process exit code. Returns
/// `Some(EXIT_PARTIAL_FAILURE)` when the run completed but any record failed and
/// `allow_partial` was false, otherwise `None` (a clean `Ok(())` exit).
///
/// `Completed` always maps to `None`; a non-`Completed` status OR a non-zero
/// failure count is a partial failure. Both signals are checked so a report can
/// never read as clean while still carrying failures.
fn partial_failure_exit_code(
    status: SourceUniverseBatchExecutionReportStatus,
    failed_record_count: u64,
    allow_partial: bool,
) -> Option<i32> {
    if allow_partial {
        return None;
    }
    let has_failures = failed_record_count > 0
        || !matches!(status, SourceUniverseBatchExecutionReportStatus::Completed);
    has_failures.then_some(EXIT_PARTIAL_FAILURE)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{
        BatchWorkerFetcher, Cli, EXIT_PARTIAL_FAILURE, SourceUniverseBatchLaunchSpec,
        SourceUniverseBatchTransportSpec, build_batch_worker_fetcher, partial_failure_exit_code,
        require_absolute_output_root, resolve_execution_pack_path, run_batch,
    };
    use backtesting_vertical_slice::hashing::sha256_hex;
    use backtesting_vertical_slice::source_universe_batch_execution::{
        SourceUniverseBatchBootstrapLimits, SourceUniverseBatchExecutionReportStatus,
        SourceUniverseBatchResourceLimits,
    };
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    use backtesting_vertical_slice::source_universe_batch_launch::{
        CommittedSourceUniverseExecutionPack, discover_committed_source_universe_execution_packs,
        inspect_worktree_source_universe_execution_pack_scope_names,
    };
    use backtesting_vertical_slice::source_universe_batch_launch::{
        SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION, SourceUniverseBatchLaunchArtifactSpec,
    };
    use backtesting_vertical_slice::source_universe_local_storage::SourceUniverseLocalStoragePolicy;
    use clap::Parser;

    fn test_bootstrap_limits() -> SourceUniverseBatchBootstrapLimits {
        SourceUniverseBatchBootstrapLimits {
            max_launch_artifact_bytes: 65_536,
            max_control_artifact_bytes: 65_536,
            max_retained_control_input_bytes: 262_144,
        }
    }

    #[test]
    fn batch_worker_rejects_a_relative_resolved_output_root() {
        let error = require_absolute_output_root(std::path::Path::new("relative/output"))
            .expect_err("a relative output root must not fall back to the current directory");
        assert!(
            error
                .to_string()
                .contains("resolved batch output root must be absolute"),
            "{error:#}"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn discover_test_packs(
        repository_root: &std::path::Path,
    ) -> anyhow::Result<Vec<CommittedSourceUniverseExecutionPack>> {
        let scope_names =
            inspect_worktree_source_universe_execution_pack_scope_names(repository_root, 64)?;
        discover_committed_source_universe_execution_packs(
            repository_root,
            &scope_names,
            test_bootstrap_limits(),
        )
    }

    fn read_test_launch_spec(
        path: &std::path::Path,
    ) -> anyhow::Result<SourceUniverseBatchLaunchSpec> {
        let bytes = fs::read(path)?;
        SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
            path,
            u64::try_from(bytes.len()).expect("test launch length fits u64"),
            &sha256_hex(&bytes),
        )
        .map(|pinned| pinned.spec)
    }

    fn test_local_storage_policy(root: &std::path::Path) -> SourceUniverseLocalStoragePolicy {
        let workspace_root = root.join("workspace");
        SourceUniverseLocalStoragePolicy {
            owner_lock_path: workspace_root.join("owner.lock"),
            workspace_root,
            max_workspace_bytes: 1 << 30,
            max_cache_bytes: 1 << 29,
            minimum_free_space_reserve_bytes: 1 << 20,
            one_record_worst_case_bytes: 1 << 20,
            cache_retention_age_seconds: 3600,
            candidate_retention_age_seconds: 3600,
            max_lifecycle_cleanup_entries: 10_000,
            max_lifecycle_cleanup_depth: 64,
        }
    }

    #[test]
    fn completed_report_exits_clean() {
        assert_eq!(
            partial_failure_exit_code(
                SourceUniverseBatchExecutionReportStatus::Completed,
                0,
                false
            ),
            None
        );
    }

    #[test]
    fn completed_with_failures_exits_nonzero_without_allow_partial() {
        // The regression this guards: `continue_on_error` produced a
        // `CompletedWithFailures` report, but `main` returned `Ok(())`, so a
        // partial failure exited zero and was hidden from automation.
        assert_eq!(
            partial_failure_exit_code(
                SourceUniverseBatchExecutionReportStatus::CompletedWithFailures,
                1,
                false,
            ),
            Some(EXIT_PARTIAL_FAILURE)
        );
    }

    #[test]
    fn failed_report_exits_nonzero_without_allow_partial() {
        assert_eq!(
            partial_failure_exit_code(SourceUniverseBatchExecutionReportStatus::Failed, 3, false),
            Some(EXIT_PARTIAL_FAILURE)
        );
    }

    #[test]
    fn allow_partial_overrides_partial_failure_to_clean_exit() {
        assert_eq!(
            partial_failure_exit_code(
                SourceUniverseBatchExecutionReportStatus::CompletedWithFailures,
                5,
                true,
            ),
            None
        );
    }

    #[test]
    fn nonzero_failure_count_alone_exits_nonzero() {
        // Defense in depth: even if status somehow read `Completed`, a non-zero
        // failure count must still surface as a partial failure.
        assert_eq!(
            partial_failure_exit_code(
                SourceUniverseBatchExecutionReportStatus::Completed,
                2,
                false
            ),
            Some(EXIT_PARTIAL_FAILURE)
        );
    }

    #[test]
    fn unsupported_process_parallelism_fails_before_pack_or_output_access() {
        let temp = tempfile::tempdir().expect("temporary output parent");
        let spec_path = temp.path().join("launch.toml");
        let output_dir = temp.path().join("must-not-be-created");
        let error = run_batch(
            &spec_path,
            SourceUniverseBatchLaunchSpec {
                schema_version: SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION.to_string(),
                batch_id: "process-concurrency-preflight".to_string(),
                execution_pack: SourceUniverseBatchLaunchArtifactSpec {
                    path: PathBuf::from("nonexistent-pack-must-not-be-read.json"),
                    bytes: 1,
                    sha256: "0".repeat(64),
                },
                output_dir: output_dir.clone(),
                start_sequence: None,
                record_limit: None,
                continue_on_error: true,
                fetch_timeout_seconds: 1,
                worker_termination_grace_seconds: 1,
                max_concurrent_records: 2,
                transport: SourceUniverseBatchTransportSpec::StagedS3,
                object_cache_dir: temp.path().join("workspace/cache"),
                allow_partial: true,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
                resource_limits: SourceUniverseBatchResourceLimits {
                    worker_max_virtual_memory_bytes: 1_073_741_824,
                    worker_reserved_overhead_bytes: 1,
                },
                local_storage: test_local_storage_policy(temp.path()),
            },
        )
        .expect_err("unsupported process parallelism must be a global configuration error");

        assert!(
            error
                .to_string()
                .contains("requires max_concurrent_records=1"),
            "{error:#}"
        );
        assert!(
            !output_dir.exists(),
            "configuration rejection must precede batch output creation"
        );
    }

    #[test]
    fn unbounded_process_selection_fails_before_pack_output_or_cache_access() {
        let temp = tempfile::tempdir().expect("temporary output parent");
        let spec_path = temp.path().join("launch.toml");
        let output_dir = temp.path().join("must-not-be-created");
        let cache_dir = temp.path().join("workspace/cache");
        let error = run_batch(
            &spec_path,
            SourceUniverseBatchLaunchSpec {
                schema_version: SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION.to_string(),
                batch_id: "unbounded-process-selection".to_string(),
                execution_pack: SourceUniverseBatchLaunchArtifactSpec {
                    path: PathBuf::from("nonexistent-pack-must-not-be-read.json"),
                    bytes: 1,
                    sha256: "0".repeat(64),
                },
                output_dir: output_dir.clone(),
                start_sequence: None,
                record_limit: None,
                continue_on_error: false,
                fetch_timeout_seconds: 1,
                worker_termination_grace_seconds: 1,
                max_concurrent_records: 1,
                transport: SourceUniverseBatchTransportSpec::StagedS3,
                object_cache_dir: cache_dir.clone(),
                allow_partial: false,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
                resource_limits: SourceUniverseBatchResourceLimits {
                    worker_max_virtual_memory_bytes: 1_073_741_824,
                    worker_reserved_overhead_bytes: 1,
                },
                local_storage: test_local_storage_policy(temp.path()),
            },
        )
        .expect_err("unbounded process selection must be rejected before artifact access");

        assert!(
            error.to_string().contains("requires record_limit=1"),
            "{error:#}"
        );
        assert!(!output_dir.exists() && !cache_dir.exists());
    }

    #[test]
    fn multi_record_process_selection_fails_before_pack_output_or_cache_access() {
        let temp = tempfile::tempdir().expect("temporary output parent");
        let spec_path = temp.path().join("launch.toml");
        let output_dir = temp.path().join("must-not-be-created");
        let cache_dir = temp.path().join("workspace/cache");
        let error = run_batch(
            &spec_path,
            SourceUniverseBatchLaunchSpec {
                schema_version: SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION.to_string(),
                batch_id: "multi-record-process-selection".to_string(),
                execution_pack: SourceUniverseBatchLaunchArtifactSpec {
                    path: PathBuf::from("nonexistent-pack-must-not-be-read.json"),
                    bytes: 1,
                    sha256: "0".repeat(64),
                },
                output_dir: output_dir.clone(),
                start_sequence: None,
                record_limit: Some(2),
                continue_on_error: false,
                fetch_timeout_seconds: 1,
                worker_termination_grace_seconds: 1,
                max_concurrent_records: 1,
                transport: SourceUniverseBatchTransportSpec::StagedS3,
                object_cache_dir: cache_dir.clone(),
                allow_partial: false,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
                resource_limits: SourceUniverseBatchResourceLimits {
                    worker_max_virtual_memory_bytes: 1_073_741_824,
                    worker_reserved_overhead_bytes: 1,
                },
                local_storage: test_local_storage_policy(temp.path()),
            },
        )
        .expect_err("multi-record process selection must be rejected before artifact access");

        assert!(
            error.to_string().contains("requires record_limit=1"),
            "{error:#}"
        );
        assert!(!output_dir.exists() && !cache_dir.exists());
    }

    #[test]
    fn zero_worker_termination_grace_fails_before_pack_or_output_access() {
        let temp = tempfile::tempdir().expect("temporary output parent");
        let spec_path = temp.path().join("launch.toml");
        let output_dir = temp.path().join("must-not-be-created");
        let error = run_batch(
            &spec_path,
            SourceUniverseBatchLaunchSpec {
                schema_version: SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION.to_string(),
                batch_id: "zero-worker-termination-grace".to_string(),
                execution_pack: SourceUniverseBatchLaunchArtifactSpec {
                    path: PathBuf::from("nonexistent-pack-must-not-be-read.json"),
                    bytes: 1,
                    sha256: "0".repeat(64),
                },
                output_dir: output_dir.clone(),
                start_sequence: None,
                record_limit: Some(1),
                continue_on_error: false,
                fetch_timeout_seconds: 1,
                worker_termination_grace_seconds: 0,
                max_concurrent_records: 1,
                transport: SourceUniverseBatchTransportSpec::StagedS3,
                object_cache_dir: temp.path().join("workspace/cache"),
                allow_partial: false,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
                resource_limits: SourceUniverseBatchResourceLimits {
                    worker_max_virtual_memory_bytes: 1_073_741_824,
                    worker_reserved_overhead_bytes: 1,
                },
                local_storage: test_local_storage_policy(temp.path()),
            },
        )
        .expect_err("zero worker termination grace must fail before artifact access");

        assert!(
            error
                .to_string()
                .contains("worker_termination_grace_seconds must be positive"),
            "{error:#}"
        );
        assert!(
            !output_dir.exists(),
            "termination-grace rejection must precede batch output creation"
        );
    }

    #[test]
    fn invalid_http_user_agent_fails_before_pack_or_output_creation() {
        let temp = tempfile::tempdir().expect("temporary output parent");
        let spec_path = temp.path().join("launch.toml");
        let output_dir = temp.path().join("must-not-be-created");
        let error = run_batch(
            &spec_path,
            SourceUniverseBatchLaunchSpec {
                schema_version: SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION.to_string(),
                batch_id: "http-header-preflight".to_string(),
                execution_pack: SourceUniverseBatchLaunchArtifactSpec {
                    path: PathBuf::from("nonexistent-pack-must-not-be-read.json"),
                    bytes: 1,
                    sha256: "0".repeat(64),
                },
                output_dir: output_dir.clone(),
                start_sequence: None,
                record_limit: Some(1),
                continue_on_error: false,
                fetch_timeout_seconds: 1,
                worker_termination_grace_seconds: 1,
                max_concurrent_records: 1,
                transport: SourceUniverseBatchTransportSpec::Https {
                    http_user_agent: "invalid\r\nuser-agent".to_string(),
                },
                object_cache_dir: temp.path().join("workspace/cache"),
                allow_partial: false,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
                resource_limits: SourceUniverseBatchResourceLimits {
                    worker_max_virtual_memory_bytes: 1_073_741_824,
                    worker_reserved_overhead_bytes: 1,
                },
                local_storage: test_local_storage_policy(temp.path()),
            },
        )
        .expect_err("invalid HTTP user-agent must fail launch preflight");

        assert!(error.to_string().contains("HeaderValue"), "{error:#}");
        assert!(
            !output_dir.exists(),
            "HTTP header rejection must precede batch output creation"
        );
    }

    #[test]
    fn launch_execution_pack_path_ignores_ambient_cwd_decoy() {
        let spec_root = tempfile::tempdir().expect("temporary launch spec root");
        let spec_parent = spec_root.path().join("scope");
        fs::create_dir(&spec_parent).expect("create launch spec parent");
        let spec_path = spec_parent.join("source-universe-batch-launch.toml");

        let current_dir = std::env::current_dir().expect("resolve ambient working directory");
        let cwd_decoy_root =
            tempfile::tempdir_in(&current_dir).expect("create ambient working-directory decoy");
        let decoy_component = cwd_decoy_root
            .path()
            .file_name()
            .expect("decoy directory has a file name");
        let declared_path = PathBuf::from(decoy_component).join("execution-pack.json");
        let cwd_decoy_path = current_dir.join(&declared_path);
        fs::write(&cwd_decoy_path, b"ambient decoy").expect("write ambient decoy pack");

        let authoritative_path = spec_parent.join(&declared_path);
        fs::create_dir_all(
            authoritative_path
                .parent()
                .expect("authoritative pack parent"),
        )
        .expect("create authoritative pack parent");
        fs::write(&authoritative_path, b"spec-parent-owned pack")
            .expect("write authoritative execution pack");

        let resolved = resolve_execution_pack_path(&spec_path, &declared_path)
            .expect("launch execution pack resolves");

        assert_eq!(
            resolved,
            authoritative_path
                .canonicalize()
                .expect("canonical authoritative execution pack")
        );
        assert_eq!(
            fs::read(resolved).expect("read resolved execution pack"),
            b"spec-parent-owned pack"
        );
    }

    #[test]
    fn launch_spec_requires_complete_toml_owned_bootstrap_limits() {
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let spec_path = temp.path().join("launch.toml");
        fs::write(
            &spec_path,
            format!(
                r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
continue_on_error = true
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "workspace/cache"
allow_partial = false

[transport]
kind = "https"
http_user_agent = "test-agent"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
max_launch_artifact_bytes = 1024
max_control_artifact_bytes = 1024

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                "0".repeat(64)
            ),
        )
        .expect("write incomplete launch spec");

        let error = read_test_launch_spec(&spec_path)
            .expect_err("missing aggregate bootstrap cap must fail closed");
        assert!(
            error.to_string().contains("parse batch launch spec"),
            "{error:#}"
        );
    }

    #[test]
    fn launch_spec_rejects_unknown_runtime_fields() {
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let spec_path = temp.path().join("launch.toml");
        fs::write(
            &spec_path,
            format!(
                r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
continue_on_error = true
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "workspace/cache"
allow_partial = false
untracked_runtime_switch = true

[transport]
kind = "https"
http_user_agent = "test-agent"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
max_launch_artifact_bytes = 1024
max_control_artifact_bytes = 1024
max_retained_control_input_bytes = 4096

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                "0".repeat(64)
            ),
        )
        .expect("write launch spec with unknown field");

        let error =
            read_test_launch_spec(&spec_path).expect_err("unknown runtime field must fail closed");
        assert!(
            error.to_string().contains("parse batch launch spec"),
            "{error:#}"
        );
    }

    #[test]
    fn launch_spec_rejects_retired_resume_report_input() {
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let spec_path = temp.path().join("launch.toml");
        fs::write(
            &spec_path,
            format!(
                r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
continue_on_error = true
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "workspace/cache"
allow_partial = false
resume_report = {{ path = "prior-report.json", bytes = 1, sha256 = "{}" }}

[transport]
kind = "https"
http_user_agent = "test-agent"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
max_launch_artifact_bytes = 1024
max_control_artifact_bytes = 1024
max_retained_control_input_bytes = 4096

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                "1".repeat(64),
                "0".repeat(64)
            ),
        )
        .expect("write launch spec with retired resume input");

        let error = read_test_launch_spec(&spec_path)
            .expect_err("retired resume_report input must fail closed");
        assert!(
            error.to_string().contains("parse batch launch spec"),
            "{error:#}"
        );
    }

    #[test]
    fn launch_spec_rejects_invalid_bootstrap_limit_values() {
        for (bootstrap_limits, expected_error) in [
            (
                "max_launch_artifact_bytes = 0\nmax_control_artifact_bytes = 1\nmax_retained_control_input_bytes = 1",
                "max_launch_artifact_bytes must be positive",
            ),
            (
                "max_launch_artifact_bytes = 1\nmax_control_artifact_bytes = 0\nmax_retained_control_input_bytes = 1",
                "max_control_artifact_bytes must be positive",
            ),
            (
                "max_launch_artifact_bytes = 1\nmax_control_artifact_bytes = 1\nmax_retained_control_input_bytes = 0",
                "max_retained_control_input_bytes must be positive",
            ),
            (
                "max_launch_artifact_bytes = 1\nmax_control_artifact_bytes = 2\nmax_retained_control_input_bytes = 1",
                "max_control_artifact_bytes must not exceed max_retained_control_input_bytes",
            ),
        ] {
            let temp = tempfile::tempdir().expect("temporary launch spec parent");
            let spec_path = temp.path().join("launch.toml");
            fs::write(
                &spec_path,
                format!(
                    r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
continue_on_error = true
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "workspace/cache"
allow_partial = false

[transport]
kind = "https"
http_user_agent = "test-agent"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
{bootstrap_limits}

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                    "0".repeat(64)
                ),
            )
            .expect("write launch spec with invalid bootstrap limits");

            let error = read_test_launch_spec(&spec_path)
                .expect_err("invalid bootstrap limit values must fail closed");
            assert!(error.to_string().contains(expected_error), "{error:#}");
        }
    }

    #[test]
    fn cli_requires_one_toml_spec_and_rejects_retired_scalar_flags() {
        assert!(
            Cli::try_parse_from(["source-universe-batch-execution"]).is_err(),
            "the launch TOML path must be required"
        );
        assert!(
            Cli::try_parse_from(["source-universe-batch-execution", "--spec", "launch.toml",])
                .is_err(),
            "the admitted launch byte length and SHA-256 must be required"
        );
        let error = Cli::try_parse_from([
            "source-universe-batch-execution",
            "--spec",
            "launch.toml",
            "--spec-bytes",
            "1",
            "--spec-sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
            "--max-launch-artifact-bytes",
            "1024",
        ])
        .expect_err("the retired scalar bootstrap flag must not remain accepted");
        assert!(error.to_string().contains("unexpected argument"), "{error}");

        let parsed = Cli::try_parse_from([
            "source-universe-batch-execution",
            "--spec",
            "launch.toml",
            "--spec-bytes",
            "1",
            "--spec-sha256",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ])
        .expect("one SHA-pinned launch TOML path parses");
        assert_eq!(parsed.spec, PathBuf::from("launch.toml"));
        assert_eq!(parsed.spec_bytes, 1);
        assert_eq!(
            parsed.spec_sha256,
            "0000000000000000000000000000000000000000000000000000000000000000"
        );
        assert!(
            Cli::try_parse_from([
                "source-universe-batch-execution",
                "--spec",
                "first.toml",
                "--spec",
                "second.toml",
                "--spec-bytes",
                "1",
                "--spec-sha256",
                "0000000000000000000000000000000000000000000000000000000000000000",
            ])
            .is_err(),
            "duplicate launch specs must fail"
        );
    }

    #[test]
    fn complete_launch_spec_parses_and_unknown_nested_limit_fails() {
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let spec_path = temp.path().join("launch.toml");
        let valid = format!(
            r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
start_sequence = 7
record_limit = 3
continue_on_error = true
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "cache"
allow_partial = false

[transport]
kind = "https"
http_user_agent = "test-agent"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
max_launch_artifact_bytes = 1024
max_control_artifact_bytes = 1024
max_retained_control_input_bytes = 4096

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
            "0".repeat(64)
        );
        fs::write(&spec_path, &valid).expect("write complete launch spec");
        let parsed = read_test_launch_spec(&spec_path).expect("complete launch spec parses");
        assert_eq!(parsed.start_sequence, Some(7));
        assert_eq!(parsed.record_limit, Some(3));

        fs::write(
            &spec_path,
            valid.replace(
                "worker_max_virtual_memory_bytes = 1073741824",
                "worker_max_virtual_memory_bytes = 0",
            ),
        )
        .expect("write launch spec with zero worker memory limit");
        let error = read_test_launch_spec(&spec_path)
            .expect_err("zero worker memory limit must fail closed");
        assert!(
            error
                .to_string()
                .contains("worker_max_virtual_memory_bytes must be positive"),
            "{error:#}"
        );

        fs::write(
            &spec_path,
            valid.replace(
                "max_retained_control_input_bytes = 4096",
                "max_retained_control_input_bytes = 4096\nuntracked_nested_limit = 1",
            ),
        )
        .expect("write launch spec with unknown nested limit");
        let error = read_test_launch_spec(&spec_path)
            .expect_err("unknown nested bootstrap limit must fail closed");
        assert!(
            error.to_string().contains("parse batch launch spec"),
            "{error:#}"
        );
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn committed_one_record_launch_profiles_select_exact_staged_s3_packs() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let committed_packs =
            discover_test_packs(&repository_root).expect("discover committed execution packs");
        for committed_pack in &committed_packs {
            let spec = &committed_pack.launch_spec;
            assert_eq!(spec.start_sequence, Some(0));
            assert_eq!(spec.record_limit, Some(1));
            assert_eq!(spec.max_concurrent_records, 1);
            assert!(!spec.continue_on_error);
            assert!(!spec.allow_partial);
            assert_eq!(spec.transport, SourceUniverseBatchTransportSpec::StagedS3);

            let pack_bytes = fs::read(&committed_pack.summary_path).unwrap_or_else(|error| {
                panic!(
                    "read {} discovered from {}: {error}",
                    committed_pack.summary_path.display(),
                    committed_pack.launch_path.display()
                )
            });
            assert_eq!(
                u64::try_from(pack_bytes.len()).expect("pack size fits u64"),
                spec.execution_pack.bytes
            );
            assert_eq!(sha256_hex(&pack_bytes), spec.execution_pack.sha256);
            let launch_bytes =
                fs::read(&committed_pack.launch_path).expect("read committed batch launch spec");
            assert_eq!(
                u64::try_from(launch_bytes.len()).expect("launch size fits u64"),
                committed_pack.launch_bytes
            );
            assert_eq!(sha256_hex(&launch_bytes), committed_pack.launch_sha256);
        }
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn sha_pinned_launch_reader_rejects_same_length_path_replacement_before_parse() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let committed =
            discover_test_packs(&repository_root).expect("discover committed execution packs");
        let pack = committed.last().expect("committed registry is nonempty");
        let original = fs::read(&pack.launch_path).expect("read committed launch bytes");
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let mutable_path = temp.path().join("source-universe-batch-launch.toml");
        fs::write(&mutable_path, &original).expect("write admitted launch bytes");
        SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
            &mutable_path,
            pack.launch_bytes,
            &pack.launch_sha256,
        )
        .expect("exact admitted launch bytes parse");
        let length_error = SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
            &mutable_path,
            pack.launch_bytes + 1,
            &pack.launch_sha256,
        )
        .expect_err("wrong admitted launch length must fail before reading");
        assert!(
            length_error.to_string().contains("byte length mismatch"),
            "{length_error:#}"
        );

        let mut replaced = original;
        let last = replaced.last_mut().expect("launch spec is nonempty");
        *last = if *last == b'\n' { b' ' } else { b'\n' };
        fs::write(&mutable_path, &replaced).expect("replace launch bytes at admitted path");
        let error = SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
            &mutable_path,
            pack.launch_bytes,
            &pack.launch_sha256,
        )
        .expect_err("same-length launch replacement must fail before parsing");
        assert!(error.to_string().contains("SHA-256 mismatch"), "{error:#}");
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn sha_pinned_launch_reader_rejects_leaf_and_parent_symlink_substitution() {
        use std::os::unix::fs::symlink;

        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let committed =
            discover_test_packs(&repository_root).expect("discover committed execution packs");
        let pack = committed.last().expect("committed registry is nonempty");
        let original = fs::read(&pack.launch_path).expect("read committed launch bytes");
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let real_parent = temp.path().join("real-parent");
        fs::create_dir(&real_parent).expect("create real launch parent");
        let real_launch = real_parent.join("source-universe-batch-launch.toml");
        fs::write(&real_launch, &original).expect("write real launch bytes");

        let leaf_link = real_parent.join("leaf-link.toml");
        symlink(&real_launch, &leaf_link).expect("create launch leaf symlink");
        let leaf_error = SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
            &leaf_link,
            pack.launch_bytes,
            &pack.launch_sha256,
        )
        .expect_err("launch leaf symlink must fail closed");
        let leaf_chain = format!("{leaf_error:#}");
        assert!(
            leaf_chain.contains("non-symlink regular file"),
            "{leaf_error:#}"
        );

        let parent_link = temp.path().join("parent-link");
        symlink(&real_parent, &parent_link).expect("create launch parent symlink");
        let parent_error = SourceUniverseBatchLaunchSpec::from_sha256_pinned_toml_file(
            &parent_link.join("source-universe-batch-launch.toml"),
            pack.launch_bytes,
            &pack.launch_sha256,
        )
        .expect_err("launch parent symlink must fail closed");
        let parent_chain = format!("{parent_error:#}");
        assert!(
            parent_chain.contains("must be a non-symlink directory"),
            "{parent_error:#}"
        );
    }

    #[test]
    fn staged_s3_transport_rejects_https_only_configuration() {
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let spec_path = temp.path().join("launch.toml");
        fs::write(
            &spec_path,
            format!(
                r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
record_limit = 1
continue_on_error = false
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 5
max_concurrent_records = 1
object_cache_dir = "workspace/cache"
allow_partial = false

[transport]
kind = "staged_s3"
http_user_agent = "must-not-be-accepted"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
max_launch_artifact_bytes = 1024
max_control_artifact_bytes = 1024
max_retained_control_input_bytes = 4096

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                "0".repeat(64)
            ),
        )
        .expect("write conflicting launch spec");

        read_test_launch_spec(&spec_path)
            .expect_err("staged S3 transport must not accept HTTPS-only configuration");
    }

    #[test]
    fn launch_spec_rejects_zero_worker_termination_grace() {
        let temp = tempfile::tempdir().expect("temporary launch spec parent");
        let spec_path = temp.path().join("launch.toml");
        fs::write(
            &spec_path,
            format!(
                r#"schema_version = "{SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION}"
batch_id = "test-batch"
output_dir = "output"
record_limit = 1
continue_on_error = false
fetch_timeout_seconds = 30
worker_termination_grace_seconds = 0
max_concurrent_records = 1
object_cache_dir = "workspace/cache"
allow_partial = false

[transport]
kind = "staged_s3"

[execution_pack]
path = "pack.json"
bytes = 1
sha256 = "{}"

[bootstrap_limits]
max_launch_artifact_bytes = 1024
max_control_artifact_bytes = 1024
max_retained_control_input_bytes = 4096

[resource_limits]
worker_max_virtual_memory_bytes = 1073741824
worker_reserved_overhead_bytes = 268435456

[local_storage]
workspace_root = "workspace"
owner_lock_path = "workspace/owner.lock"
max_workspace_bytes = 1073741824
max_cache_bytes = 536870912
minimum_free_space_reserve_bytes = 1048576
one_record_worst_case_bytes = 1048576
cache_retention_age_seconds = 3600
candidate_retention_age_seconds = 3600
max_lifecycle_cleanup_entries = 10000
max_lifecycle_cleanup_depth = 64
"#,
                "0".repeat(64)
            ),
        )
        .expect("write zero-grace launch spec");

        let error =
            read_test_launch_spec(&spec_path).expect_err("zero termination grace must fail closed");
        assert!(
            error
                .to_string()
                .contains("worker_termination_grace_seconds must be positive"),
            "{error:#}"
        );
    }

    #[test]
    fn transport_selection_constructs_only_the_requested_implementation() {
        let cache = tempfile::tempdir().expect("temporary source-object evidence archive");
        let staged = build_batch_worker_fetcher(
            &SourceUniverseBatchTransportSpec::StagedS3,
            1,
            cache.path(),
        )
        .expect("construct staged-S3 transport");
        assert!(matches!(staged, BatchWorkerFetcher::StagedS3(_)));

        let https = build_batch_worker_fetcher(
            &SourceUniverseBatchTransportSpec::Https {
                http_user_agent: "transport-selection-test".to_string(),
            },
            1,
            cache.path(),
        )
        .expect("construct HTTPS transport");
        assert!(matches!(https, BatchWorkerFetcher::Http(_)));
    }
}
