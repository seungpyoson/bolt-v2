//! Generate a venue-level backfill conversion batch plan from a TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_conversion_batch::{
    BackfillConversionBatchPlan, write_backfill_conversion_batch_plan_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a backfill conversion batch plan from a TOML spec.")]
struct Cli {
    /// Path to the conversion batch spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_conversion_batch_plan_from_spec_file(&cli.spec)?;
    let plan: BackfillConversionBatchPlan = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_conversion_batch_plan = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!("status = {:?}", plan.status);
    println!("total_accepted_objects = {}", plan.total_accepted_objects);
    println!("total_accepted_bytes = {}", plan.total_accepted_bytes);
    Ok(())
}
