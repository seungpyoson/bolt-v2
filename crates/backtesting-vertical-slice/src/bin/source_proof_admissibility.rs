//! Generate a source-proof admissibility report from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::source_proof_admissibility::{
    SourceProofAdmissibilityReport, write_source_proof_admissibility_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a source-proof admissibility report from a TOML spec.")]
struct Cli {
    /// Path to the source-proof admissibility spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_source_proof_admissibility_report_from_spec_file(&cli.spec)?;
    let report: SourceProofAdmissibilityReport =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_proof_admissibility_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!(
        "current_contract_records = {}",
        report.summary.current_contract_records
    );
    println!(
        "accept_ready_records = {}",
        report.summary.accept_ready_records
    );
    println!(
        "current_contract_rejected_records = {}",
        report.summary.current_contract_rejected_records
    );
    println!(
        "non_current_contract_records = {}",
        report.summary.non_current_contract_records
    );
    Ok(())
}
