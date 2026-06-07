use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::source_proof_migration_preflight::{
    SourceProofMigrationPreflightReport,
    write_source_proof_migration_preflight_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate source-proof migration candidates from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_source_proof_migration_preflight_report_from_spec_file(&cli.spec)?;
    let report: SourceProofMigrationPreflightReport =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_proof_migration_preflight_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!(
        "eligible_candidate_count = {}",
        report.eligible_candidate_count
    );
    Ok(())
}
