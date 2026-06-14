//! Generate a legacy source-proof derivability report from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::source_proof_legacy_derivability::{
    SourceProofLegacyDerivabilityReport,
    write_source_proof_legacy_derivability_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a legacy source-proof derivability report from a TOML spec.")]
struct Cli {
    /// Path to the legacy source-proof derivability spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_source_proof_legacy_derivability_report_from_spec_file(&cli.spec)?;
    let report: SourceProofLegacyDerivabilityReport =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_proof_legacy_derivability_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!("s3_bound_records = {}", report.summary.s3_bound_records);
    println!(
        "single_table_family_records = {}",
        report.summary.single_table_family_records
    );
    println!(
        "acceptance_blocked_records = {}",
        report.summary.acceptance_blocked_records
    );
    Ok(())
}
