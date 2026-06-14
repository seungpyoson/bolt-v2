//! Generate a source-proof shortlist report from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::source_proof_shortlist::{
    SourceProofShortlistReport, write_source_proof_shortlist_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a source-proof shortlist report from a TOML spec.")]
struct Cli {
    /// Path to the source-proof shortlist spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_source_proof_shortlist_report_from_spec_file(&cli.spec)?;
    let report: SourceProofShortlistReport = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_proof_shortlist_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!(
        "eligible_candidate_count = {}",
        report.eligible_candidate_count
    );
    println!("candidate_count = {}", artifact.candidate_count);
    Ok(())
}
