use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_execution_plan::{
    BackfillExecutionPlan, write_backfill_execution_plan_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a source-proof-bound backfill execution plan")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_execution_plan_from_spec_file(&cli.spec)?;
    let plan: BackfillExecutionPlan = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!("backfill_execution_plan = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", plan.status);
    println!("object_count = {}", plan.object_count);
    println!("accepted_bytes = {}", plan.accepted_bytes);
    println!("operator_run_id = {}", plan.operator_run_id);
    Ok(())
}
