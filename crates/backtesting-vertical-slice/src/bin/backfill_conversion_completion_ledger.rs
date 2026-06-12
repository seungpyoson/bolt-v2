//! Generate a venue-level backfill conversion completion ledger from a TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_conversion_completion::{
    BackfillConversionCompletionLedger, write_backfill_conversion_completion_ledger_from_spec_file,
};
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate conversion publication completion from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_backfill_conversion_completion_ledger_from_spec_file(&spec_path)?;
    let ledger: BackfillConversionCompletionLedger =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_conversion_completion_ledger = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!("status = {:?}", ledger.status);
    println!("published_records = {}", ledger.published_records);
    println!("mapping_proven_records = {}", ledger.mapping_proven_records);
    println!("total_canonical_rows = {}", ledger.total_canonical_rows);
    println!("total_nt_iterations = {}", ledger.total_nt_iterations);
    Ok(())
}
