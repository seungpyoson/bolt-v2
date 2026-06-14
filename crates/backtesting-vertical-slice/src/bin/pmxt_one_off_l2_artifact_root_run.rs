//! Run the PMXT one-off L2 artifact-root writer from a config-owned TOML spec.

use std::path::PathBuf;

use anyhow::Result;
use backtesting_vertical_slice::pmxt_one_off_backfill_projection::write_pmxt_one_off_l2_artifact_root_run_from_spec_file;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write PMXT one-off L2 catalog and result-contract artifacts from a TOML spec.")]
struct Cli {
    /// Path to the PMXT one-off artifact-root run spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_pmxt_one_off_l2_artifact_root_run_from_spec_file(&cli.spec)?;
    println!("output_dir = {}", artifact.output_dir.display());
    println!(
        "result_contract = {}",
        artifact.result_contract_path.display()
    );
    println!("result_contract_hash = {}", artifact.result_contract_hash);
    println!(
        "conversion_manifest_hash = {}",
        artifact.conversion_manifest_hash
    );
    println!("catalog_hash = {}", artifact.catalog_hash);
    println!(
        "selected_source_parquet_hash = {}",
        artifact.selected_source_parquet_hash
    );
    println!(
        "event_count_ledger_hash = {}",
        artifact.event_count_ledger_hash
    );
    println!(
        "selected_asset_ids_hash = {}",
        artifact.selected_asset_ids_hash
    );
    println!("projected_l2_rows = {}", artifact.projected_l2_rows);
    println!("nt_iterations = {}", artifact.nt_iterations);
    Ok(())
}
