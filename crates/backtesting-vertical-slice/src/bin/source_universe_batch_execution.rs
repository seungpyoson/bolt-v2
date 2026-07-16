use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use backtesting_vertical_slice::path_resolution::{
    resolve_existing_input_path, resolve_existing_path, resolve_output_dir,
};
use backtesting_vertical_slice::source_universe_batch_execution::{
    CachingSourceUniverseObjectFetcher, HttpSourceUniverseObjectFetcher,
    SOURCE_UNIVERSE_OPERATOR_WORKER_MODE, SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT,
    SourceUniverseBatchArtifactPin, SourceUniverseBatchBootstrapLimits,
    SourceUniverseBatchExecutionConfig, SourceUniverseBatchExecutionReportStatus,
    SourceUniverseBatchLaunchArtifacts, SourceUniverseCacheRunVerification,
    SourceUniverseObjectFetcher, execute_source_universe_batch_process_isolated,
    execute_source_universe_operator_worker, write_source_universe_batch_execution_report,
};
use backtesting_vertical_slice::source_universe_object_transport::StagedS3SourceUniverseObjectFetcher;
use clap::Parser;
use serde::Deserialize;

const SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION: &str =
    "source-universe-batch-launch-spec.v2";

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
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseBatchLaunchArtifactSpec {
    path: PathBuf,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseBatchLaunchSpec {
    schema_version: String,
    batch_id: String,
    execution_pack: SourceUniverseBatchLaunchArtifactSpec,
    output_dir: PathBuf,
    start_sequence: Option<u64>,
    record_limit: Option<u64>,
    continue_on_error: bool,
    fetch_timeout_seconds: u64,
    worker_termination_grace_seconds: u64,
    max_concurrent_records: u64,
    transport: SourceUniverseBatchTransportSpec,
    object_cache_dir: Option<PathBuf>,
    allow_partial: bool,
    bootstrap_limits: SourceUniverseBatchBootstrapLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceUniverseBatchTransportSpec {
    StagedS3,
    Https { http_user_agent: String },
}

impl SourceUniverseBatchLaunchSpec {
    fn from_toml_file(path: &Path) -> Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("read batch launch spec {}", path.display()))?;
        let spec: Self = toml::from_slice(&bytes)
            .with_context(|| format!("parse batch launch spec {}", path.display()))?;
        ensure!(
            spec.schema_version == SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
            "batch launch spec schema_version mismatch: expected {}, got {}",
            SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
            spec.schema_version
        );
        ensure!(
            spec.fetch_timeout_seconds > 0,
            "batch launch spec fetch_timeout_seconds must be positive"
        );
        ensure!(
            spec.worker_termination_grace_seconds > 0,
            "batch launch spec worker_termination_grace_seconds must be positive"
        );
        spec.transport.validate()?;
        spec.bootstrap_limits.validate()?;
        Ok(spec)
    }
}

impl SourceUniverseBatchTransportSpec {
    fn validate(&self) -> Result<()> {
        match self {
            Self::StagedS3 => Ok(()),
            Self::Https { http_user_agent } => validate_http_user_agent(http_user_agent),
        }
    }
}

fn validate_http_user_agent(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "batch launch spec http_user_agent must not be empty"
    );
    reqwest::header::HeaderValue::from_bytes(value.as_bytes())
        .context("batch launch spec http_user_agent must be a valid HTTP HeaderValue")?;
    Ok(())
}

#[derive(Debug, Parser)]
struct WorkerCli {
    #[arg(long)]
    request_archive_bytes: u64,
    #[arg(long)]
    request_manifest_bytes: u64,
    #[arg(long)]
    request_manifest_sha256: String,
    #[arg(long)]
    bootstrap_max_bytes: u64,
}

/// Object fetcher used by every batch worker: the HTTP fetcher, optionally
/// wrapped in the content-addressed cache when `object_cache_dir` is set.
/// One concrete type keeps the factory signature monomorphic across both modes.
enum BatchWorkerFetcher {
    DirectHttp(HttpSourceUniverseObjectFetcher),
    CachedHttp(CachingSourceUniverseObjectFetcher<HttpSourceUniverseObjectFetcher>),
    DirectStagedS3(StagedS3SourceUniverseObjectFetcher),
    CachedStagedS3(CachingSourceUniverseObjectFetcher<StagedS3SourceUniverseObjectFetcher>),
}

impl SourceUniverseObjectFetcher for BatchWorkerFetcher {
    fn fetch(
        &mut self,
        record: &backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPackRecord,
        run_spec: &backtesting_vertical_slice::operator::RunSpec,
        work_budget: &backtesting_vertical_slice::operator_work_budget::OperatorWorkBudgetGuard,
    ) -> Result<Vec<u8>> {
        match self {
            BatchWorkerFetcher::DirectHttp(fetcher) => {
                fetcher.fetch(record, run_spec, work_budget)
            }
            BatchWorkerFetcher::CachedHttp(fetcher) => {
                fetcher.fetch(record, run_spec, work_budget)
            }
            BatchWorkerFetcher::DirectStagedS3(fetcher) => {
                fetcher.fetch(record, run_spec, work_budget)
            }
            BatchWorkerFetcher::CachedStagedS3(fetcher) => {
                fetcher.fetch(record, run_spec, work_budget)
            }
        }
    }
}

fn build_batch_worker_fetcher(
    transport: &SourceUniverseBatchTransportSpec,
    fetch_timeout_seconds: u64,
    object_cache_dir: Option<&Path>,
    cache_run_verification: SourceUniverseCacheRunVerification,
) -> Result<BatchWorkerFetcher> {
    match transport {
        SourceUniverseBatchTransportSpec::Https { http_user_agent } => {
            let fetcher = HttpSourceUniverseObjectFetcher::new(
                Some(fetch_timeout_seconds),
                Some(http_user_agent.as_str()),
            )?;
            match object_cache_dir {
                Some(cache_dir) => Ok(BatchWorkerFetcher::CachedHttp(
                    CachingSourceUniverseObjectFetcher::for_run(
                        fetcher,
                        cache_dir,
                        cache_run_verification,
                    ),
                )),
                None => Ok(BatchWorkerFetcher::DirectHttp(fetcher)),
            }
        }
        SourceUniverseBatchTransportSpec::StagedS3 => {
            let fetcher = StagedS3SourceUniverseObjectFetcher::new(Some(fetch_timeout_seconds))?;
            match object_cache_dir {
                Some(cache_dir) => Ok(BatchWorkerFetcher::CachedStagedS3(
                    CachingSourceUniverseObjectFetcher::for_run(
                        fetcher,
                        cache_dir,
                        cache_run_verification,
                    ),
                )),
                None => Ok(BatchWorkerFetcher::DirectStagedS3(fetcher)),
            }
        }
    }
}

fn main() -> Result<()> {
    if std::env::args_os().nth(1).is_some_and(|argument| {
        argument == std::ffi::OsStr::new(SOURCE_UNIVERSE_OPERATOR_WORKER_MODE)
    }) {
        let worker = WorkerCli::parse_from(std::env::args_os().skip(1));
        return execute_source_universe_operator_worker(
            worker.request_archive_bytes,
            worker.request_manifest_bytes,
            &worker.request_manifest_sha256,
            worker.bootstrap_max_bytes,
        );
    }
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let spec = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)?;
    run_batch(&spec_path, spec)
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
    } = spec;
    ensure!(
        fetch_timeout_seconds > 0,
        "batch launch spec fetch_timeout_seconds must be positive"
    );
    ensure!(
        worker_termination_grace_seconds > 0,
        "batch launch spec worker_termination_grace_seconds must be positive"
    );
    transport.validate()?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let execution_pack_path = resolve_existing_path(base_dir, &execution_pack.path);
    let output_dir = resolve_output_dir(base_dir, &declared_output_dir);
    let object_cache_dir = declared_object_cache_dir
        .as_ref()
        .map(|path| resolve_output_dir(base_dir, path));
    let cache_run_verification = SourceUniverseCacheRunVerification::default();

    let fetcher_factory = move || -> Result<BatchWorkerFetcher> {
        build_batch_worker_fetcher(
            &transport,
            fetch_timeout_seconds,
            object_cache_dir.as_deref(),
            cache_run_verification.clone(),
        )
    };
    let absolute_output_dir = if output_dir.is_absolute() {
        output_dir.clone()
    } else {
        std::env::current_dir()
            .context("resolve current directory")?
            .join(&output_dir)
    };
    let request_root = absolute_output_dir.join(SOURCE_UNIVERSE_OPERATOR_WORKER_REQUEST_ROOT);
    let execution_pack = SourceUniverseBatchArtifactPin::try_new(
        execution_pack_path,
        execution_pack.bytes,
        execution_pack.sha256,
    )?;
    let launch_artifacts =
        SourceUniverseBatchLaunchArtifacts::try_new(execution_pack, bootstrap_limits)?;

    let report = execute_source_universe_batch_process_isolated(
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
    )?;
    let artifact = write_source_universe_batch_execution_report(&output_dir, &report)?;
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
        BatchWorkerFetcher, Cli, EXIT_PARTIAL_FAILURE,
        SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
        SourceUniverseBatchLaunchArtifactSpec, SourceUniverseBatchLaunchSpec,
        SourceUniverseBatchTransportSpec, build_batch_worker_fetcher, partial_failure_exit_code,
        run_batch,
    };
    use backtesting_vertical_slice::source_universe_batch_execution::{
        SourceUniverseBatchBootstrapLimits, SourceUniverseBatchExecutionReportStatus,
        SourceUniverseCacheRunVerification,
    };
    use clap::Parser;

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
                object_cache_dir: None,
                allow_partial: true,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
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
                object_cache_dir: None,
                allow_partial: false,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
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
                record_limit: None,
                continue_on_error: false,
                fetch_timeout_seconds: 1,
                worker_termination_grace_seconds: 1,
                max_concurrent_records: 1,
                transport: SourceUniverseBatchTransportSpec::Https {
                    http_user_agent: "invalid\r\nuser-agent".to_string(),
                },
                object_cache_dir: None,
                allow_partial: false,
                bootstrap_limits: SourceUniverseBatchBootstrapLimits {
                    max_launch_artifact_bytes: 1,
                    max_control_artifact_bytes: 1,
                    max_retained_control_input_bytes: 1,
                },
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
"#,
                "0".repeat(64)
            ),
        )
        .expect("write incomplete launch spec");

        let error = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
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
"#,
                "0".repeat(64)
            ),
        )
        .expect("write launch spec with unknown field");

        let error = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
            .expect_err("unknown runtime field must fail closed");
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
"#,
                "1".repeat(64),
                "0".repeat(64)
            ),
        )
        .expect("write launch spec with retired resume input");

        let error = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
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
"#,
                    "0".repeat(64)
                ),
            )
            .expect("write launch spec with invalid bootstrap limits");

            let error = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
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
        let error = Cli::try_parse_from([
            "source-universe-batch-execution",
            "--spec",
            "launch.toml",
            "--max-launch-artifact-bytes",
            "1024",
        ])
        .expect_err("the retired scalar bootstrap flag must not remain accepted");
        assert!(error.to_string().contains("unexpected argument"), "{error}");

        let parsed =
            Cli::try_parse_from(["source-universe-batch-execution", "--spec", "launch.toml"])
                .expect("one launch TOML path parses");
        assert_eq!(parsed.spec, PathBuf::from("launch.toml"));
        assert!(
            Cli::try_parse_from([
                "source-universe-batch-execution",
                "--spec",
                "first.toml",
                "--spec",
                "second.toml",
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
"#,
            "0".repeat(64)
        );
        fs::write(&spec_path, &valid).expect("write complete launch spec");
        let parsed = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
            .expect("complete launch spec parses");
        assert_eq!(parsed.start_sequence, Some(7));
        assert_eq!(parsed.record_limit, Some(3));

        fs::write(
            &spec_path,
            valid.replace(
                "max_retained_control_input_bytes = 4096",
                "max_retained_control_input_bytes = 4096\nuntracked_nested_limit = 1",
            ),
        )
        .expect("write launch spec with unknown nested limit");
        let error = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
            .expect_err("unknown nested bootstrap limit must fail closed");
        assert!(
            error.to_string().contains("parse batch launch spec"),
            "{error:#}"
        );
    }

    #[test]
    fn committed_one_record_launch_profiles_select_exact_staged_s3_packs() {
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        for relative_path in [
            "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs/binance-data-vision-trades-2026-03-01-all-instruments/source-universe-batch-launch.toml",
            "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-universe-batch-launch.toml",
        ] {
            let launch_path = repository_root.join(relative_path);
            let spec = SourceUniverseBatchLaunchSpec::from_toml_file(&launch_path)
                .unwrap_or_else(|error| panic!("parse {}: {error:#}", launch_path.display()));
            assert_eq!(spec.start_sequence, Some(0));
            assert_eq!(spec.record_limit, Some(1));
            assert_eq!(spec.max_concurrent_records, 1);
            assert!(!spec.continue_on_error);
            assert!(!spec.allow_partial);
            assert_eq!(spec.transport, SourceUniverseBatchTransportSpec::StagedS3);

            let pack_path = launch_path
                .parent()
                .expect("launch profile parent")
                .join(&spec.execution_pack.path);
            let pack_bytes = fs::read(&pack_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", pack_path.display()));
            assert_eq!(
                u64::try_from(pack_bytes.len()).expect("pack size fits u64"),
                spec.execution_pack.bytes
            );
            assert_eq!(
                backtesting_vertical_slice::hashing::sha256_hex(&pack_bytes),
                spec.execution_pack.sha256
            );
        }
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
"#,
                "0".repeat(64)
            ),
        )
        .expect("write conflicting launch spec");

        SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
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
"#,
                "0".repeat(64)
            ),
        )
        .expect("write zero-grace launch spec");

        let error = SourceUniverseBatchLaunchSpec::from_toml_file(&spec_path)
            .expect_err("zero termination grace must fail closed");
        assert!(
            error
                .to_string()
                .contains("worker_termination_grace_seconds must be positive"),
            "{error:#}"
        );
    }

    #[test]
    fn transport_selection_constructs_only_the_requested_implementation() {
        let staged = build_batch_worker_fetcher(
            &SourceUniverseBatchTransportSpec::StagedS3,
            1,
            None,
            SourceUniverseCacheRunVerification::default(),
        )
        .expect("construct staged-S3 transport");
        assert!(matches!(staged, BatchWorkerFetcher::DirectStagedS3(_)));

        let https = build_batch_worker_fetcher(
            &SourceUniverseBatchTransportSpec::Https {
                http_user_agent: "transport-selection-test".to_string(),
            },
            1,
            None,
            SourceUniverseCacheRunVerification::default(),
        )
        .expect("construct HTTPS transport");
        assert!(matches!(https, BatchWorkerFetcher::DirectHttp(_)));
    }
}
