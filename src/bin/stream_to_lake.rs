use std::path::PathBuf;

use anyhow::Result;
use bolt_v2::lake_batch::convert_live_spool_to_parquet;
use bolt_v2::venue_contract::{VenueContract, normalize_local_absolute_contract_path};
use clap::Parser;

#[derive(Parser)]
#[command(name = "stream_to_lake")]
struct Cli {
    #[arg(long)]
    catalog_path: PathBuf,
    #[arg(long)]
    instance_id: String,
    #[arg(long)]
    output_root: PathBuf,
    #[arg(long)]
    contract: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let contract_path = normalize_local_absolute_contract_path(&cli.contract)?;
    let contract = VenueContract::load_and_validate(&contract_path)?;

    let report = convert_live_spool_to_parquet(
        &cli.catalog_path,
        &cli.instance_id,
        &cli.output_root,
        &contract,
    )?;

    println!(
        "Contract validation: {} ({} venue, {} classes)",
        report.completeness.outcome,
        report.completeness.venue,
        report.completeness.classes.len()
    );

    println!(
        "Converted {} live stream classes for instance {} into {}",
        report.converted_classes.len(),
        report.instance_id,
        report.output_root.display()
    );

    Ok(())
}
