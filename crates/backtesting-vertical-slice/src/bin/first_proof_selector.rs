//! Generate a first-proof selector report from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::first_proof_selector::{
    FirstProofSelectorReport, write_first_proof_selector_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a first-proof selector report from a TOML spec.")]
struct Cli {
    /// Path to the first-proof selector spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_first_proof_selector_report_from_spec_file(&cli.spec)?;
    let report: FirstProofSelectorReport = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    let status = serde_json::to_value(report.status)?
        .as_str()
        .unwrap_or_default()
        .to_string();
    println!("first_proof_selector_report = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {status}");
    println!("eligible_assets = {}", report.eligible_assets);
    println!("selected_asset_count = {}", artifact.selected_asset_count);
    println!(
        "selected_asset_ids_hash = {}",
        report.selected_asset_ids_hash
    );
    Ok(())
}
