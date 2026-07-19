use std::path::PathBuf;

use anyhow::Result;
use backtesting_vertical_slice::source_universe_batch_execution::{
    CachingSourceUniverseObjectFetcher, HttpSourceUniverseObjectFetcher,
    LocalSourceUniverseOperatorRunner, SourceUniverseBatchExecutionConfig,
    SourceUniverseBatchExecutionReportStatus, SourceUniverseObjectFetcher,
    execute_source_universe_batch_with_factories, load_source_universe_control_admission_policy,
    write_source_universe_batch_execution_report,
};
use clap::Parser;

/// Process exit code when the batch completed but at least one record failed
/// (`--continue-on-error` produced a `CompletedWithFailures`/`Failed` report)
/// and `--allow-partial` was not set. Distinct from anyhow's exit 1 (a hard
/// error before/while assembling the report) so automation can tell a partial
/// data outcome apart from a runner crash.
const EXIT_PARTIAL_FAILURE: i32 = 2;

#[derive(Debug, Parser)]
#[command(about = "Execute source-universe single-object operator runs from an execution pack")]
struct Cli {
    #[arg(long)]
    batch_id: String,
    #[arg(long)]
    execution_pack: PathBuf,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long)]
    control_policy: PathBuf,
    #[arg(long)]
    start_sequence: Option<u64>,
    #[arg(long)]
    record_limit: Option<u64>,
    #[arg(long)]
    continue_on_error: bool,
    #[arg(long)]
    fetch_timeout_seconds: Option<u64>,
    #[arg(long)]
    http_user_agent: Option<String>,
    #[arg(long)]
    max_concurrent_records: Option<u64>,
    #[arg(long)]
    object_cache_dir: Option<PathBuf>,
    #[arg(long)]
    resume_from_report: Option<PathBuf>,
    /// Treat a `CompletedWithFailures`/`Failed` report as a success exit. Without
    /// this flag the process exits with [`EXIT_PARTIAL_FAILURE`] whenever any
    /// record failed, so `--continue-on-error` cannot hide partial failure from
    /// automation.
    #[arg(long)]
    allow_partial: bool,
}

/// Object fetcher used by every batch worker: the HTTP fetcher, optionally
/// wrapped in the content-addressed cache when `--object-cache-dir` is set.
/// One concrete type keeps the factory signature monomorphic across both modes.
enum BatchWorkerFetcher {
    Direct(HttpSourceUniverseObjectFetcher),
    Cached(CachingSourceUniverseObjectFetcher<HttpSourceUniverseObjectFetcher>),
}

impl SourceUniverseObjectFetcher for BatchWorkerFetcher {
    fn fetch(
        &mut self,
        record: &backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPackRecord,
    ) -> Result<Vec<u8>> {
        match self {
            BatchWorkerFetcher::Direct(fetcher) => fetcher.fetch(record),
            BatchWorkerFetcher::Cached(fetcher) => fetcher.fetch(record),
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let control_admission = load_source_universe_control_admission_policy(&cli.control_policy)?;
    let fetch_timeout_seconds = cli.fetch_timeout_seconds;
    let http_user_agent = cli.http_user_agent.clone();
    let object_cache_dir = cli.object_cache_dir.clone();

    let fetcher_factory = move || -> Result<BatchWorkerFetcher> {
        let http_fetcher = HttpSourceUniverseObjectFetcher::new(
            fetch_timeout_seconds,
            http_user_agent.as_deref(),
        )?;
        match &object_cache_dir {
            Some(cache_dir) => Ok(BatchWorkerFetcher::Cached(
                CachingSourceUniverseObjectFetcher::new(http_fetcher, cache_dir),
            )),
            None => Ok(BatchWorkerFetcher::Direct(http_fetcher)),
        }
    };
    let runner_factory =
        || -> Result<LocalSourceUniverseOperatorRunner> { Ok(LocalSourceUniverseOperatorRunner) };

    let report = execute_source_universe_batch_with_factories(
        &cli.batch_id,
        &cli.execution_pack,
        &cli.output_dir,
        SourceUniverseBatchExecutionConfig {
            control_admission,
            start_sequence: cli.start_sequence,
            record_limit: cli.record_limit,
            continue_on_error: cli.continue_on_error,
            max_concurrent_records: cli.max_concurrent_records,
            resume_report: cli.resume_from_report.clone(),
        },
        fetcher_factory,
        runner_factory,
    )?;
    let artifact = write_source_universe_batch_execution_report(&cli.output_dir, &report)?;
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
    // status (not an unconditional `Ok(())`) so `--continue-on-error` cannot mask
    // a `CompletedWithFailures`/`Failed` outcome from automation.
    if let Some(exit_code) =
        partial_failure_exit_code(report.status, report.failed_record_count, cli.allow_partial)
    {
        eprintln!(
            "batch completed with {} failed record(s); status {:?}. \
             Pass --allow-partial to exit zero.",
            report.failed_record_count, report.status
        );
        std::process::exit(exit_code);
    }
    Ok(())
}

/// Map a finished batch report to its process exit code. Returns
/// `Some(EXIT_PARTIAL_FAILURE)` when the run completed but any record failed and
/// `--allow-partial` was not set, otherwise `None` (a clean `Ok(())` exit).
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
    use super::{Cli, EXIT_PARTIAL_FAILURE, partial_failure_exit_code};
    use backtesting_vertical_slice::source_universe_batch_execution::SourceUniverseBatchExecutionReportStatus;
    use clap::Parser;

    #[test]
    fn control_policy_is_a_required_cli_argument() {
        let error = Cli::try_parse_from([
            "source-universe-batch-execution",
            "--batch-id",
            "batch",
            "--execution-pack",
            "pack.json",
            "--output-dir",
            "output",
        ])
        .expect_err("missing --control-policy must fail CLI parsing");
        assert!(error.to_string().contains("--control-policy"));
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
        // The regression this guards: `--continue-on-error` produced a
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
}
