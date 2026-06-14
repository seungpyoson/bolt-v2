use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_execution_readiness::{
    BackfillExecutionReadinessReport, write_backfill_execution_readiness_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate accepted-tranche execution readiness from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_execution_readiness_report_from_spec_file(&cli.spec)?;
    let report: BackfillExecutionReadinessReport =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_execution_readiness_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!("blockers = {}", report.blockers.len());
    Ok(())
}
