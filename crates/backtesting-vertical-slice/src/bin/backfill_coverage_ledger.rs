//! Generate a backfill coverage ledger from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_coverage::{
    BackfillCoverageLedger, write_coverage_ledger_artifact_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a backfill coverage ledger from a TOML spec.")]
struct Cli {
    /// Path to the coverage spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_coverage_ledger_artifact_from_spec_file(&cli.spec)?;
    let ledger: BackfillCoverageLedger = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!("coverage_ledger = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!("accepted_records = {}", ledger.summary.accepted_records);
    println!("accepted_objects = {}", ledger.summary.accepted_objects);
    println!("accepted_bytes = {}", ledger.summary.accepted_bytes);
    println!("rejected_records = {}", ledger.summary.rejected_records);
    println!(
        "physical_only_records = {}",
        ledger.summary.physical_only_records
    );
    Ok(())
}
