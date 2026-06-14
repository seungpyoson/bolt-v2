//! Generate a venue/source-universe conversion acceptance ledger from a TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::venue_scale_conversion_acceptance::{
    VenueScaleConversionAcceptanceLedger,
    write_venue_scale_conversion_acceptance_ledger_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate venue-scale conversion acceptance from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)?;
    let ledger: VenueScaleConversionAcceptanceLedger =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "venue_scale_conversion_acceptance_ledger = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("venues = {}", artifact.venue_count);
    println!("universes = {}", artifact.universe_count);
    println!("status = {:?}", ledger.status);
    println!("converted_universes = {}", ledger.converted_universes);
    println!("source_only_universes = {}", ledger.source_only_universes);
    println!("blocked_universes = {}", ledger.blocked_universes);
    println!(
        "total_converted_canonical_rows = {}",
        ledger.total_converted_canonical_rows
    );
    println!(
        "total_converted_nt_catalog_rows = {}",
        ledger.total_converted_nt_catalog_rows
    );
    println!(
        "total_source_only_objects = {}",
        ledger.total_source_only_objects
    );
    Ok(())
}
