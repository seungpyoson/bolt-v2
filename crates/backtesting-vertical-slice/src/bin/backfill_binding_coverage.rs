use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_binding_coverage::{
    BackfillBindingCoverageReport, write_backfill_binding_coverage_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate source-binding coverage over a backfill ledger")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_binding_coverage_report_from_spec_file(&cli.spec)?;
    let report: BackfillBindingCoverageReport = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_binding_coverage_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!(
        "ledger_records_for_required_bindings = {}",
        report.ledger_records_for_required_bindings
    );
    Ok(())
}
