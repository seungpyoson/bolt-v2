//! Generate a bounded selected-source parquet from a config-owned TOML spec.

use std::path::PathBuf;

use anyhow::Result;
use backtesting_vertical_slice::selected_source_slice::write_selected_source_slice_from_spec_file;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a selected-source parquet and report from a TOML spec.")]
struct Cli {
    /// Path to the selected-source slice spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_selected_source_slice_from_spec_file(&cli.spec)?;
    println!(
        "selected_source_parquet = {}",
        artifact.output_parquet_path.display()
    );
    println!(
        "selected_source_report = {}",
        artifact.report_path.display()
    );
    println!("source_parquet_sha256 = {}", artifact.source_parquet_sha256);
    println!(
        "selector_report_sha256 = {}",
        artifact.selector_report_sha256
    );
    println!("output_parquet_sha256 = {}", artifact.output_parquet_sha256);
    println!("report_hash = {}", artifact.report_hash);
    println!("report_bytes = {}", artifact.report_bytes);
    println!("source_rows = {}", artifact.source_rows);
    println!("source_row_groups = {}", artifact.source_row_groups);
    println!("projected_row_groups = {}", artifact.projected_row_groups);
    println!("selected_rows = {}", artifact.selected_rows);
    println!("selected_asset_count = {}", artifact.selected_asset_count);
    println!(
        "selected_asset_ids_hash = {}",
        artifact.selected_asset_ids_hash
    );
    Ok(())
}
