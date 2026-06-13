use std::path::PathBuf;

use anyhow::Result;
use backtesting_vertical_slice::source_universe_batch_execution::{
    CachingSourceUniverseObjectFetcher, HttpSourceUniverseObjectFetcher,
    LocalSourceUniverseOperatorRunner, SourceUniverseBatchExecutionConfig,
    SourceUniverseObjectFetcher, execute_source_universe_batch_with_factories,
    write_source_universe_batch_execution_report,
};
use clap::Parser;

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
    Ok(())
}
