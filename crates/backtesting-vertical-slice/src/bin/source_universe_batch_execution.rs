use std::path::PathBuf;

use anyhow::Result;
use backtesting_vertical_slice::source_universe_batch_execution::{
    HttpSourceUniverseObjectFetcher, LocalSourceUniverseOperatorRunner,
    SourceUniverseBatchExecutionConfig,
    execute_source_universe_batch_with_config, write_source_universe_batch_execution_report,
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut fetcher = HttpSourceUniverseObjectFetcher::new()?;
    let mut runner = LocalSourceUniverseOperatorRunner;
    let report = execute_source_universe_batch_with_config(
        &cli.batch_id,
        &cli.execution_pack,
        &cli.output_dir,
        SourceUniverseBatchExecutionConfig {
            start_sequence: cli.start_sequence,
            record_limit: cli.record_limit,
            continue_on_error: cli.continue_on_error,
        },
        &mut fetcher,
        &mut runner,
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
