use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_preflight::{
    BackfillPreflightReport, write_backfill_preflight_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate a backfill preflight gate from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_preflight_report_from_spec_file(&cli.spec)?;
    let report: BackfillPreflightReport = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!("backfill_preflight_report = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!("eligible_record_count = {}", report.eligible_record_count);
    Ok(())
}
