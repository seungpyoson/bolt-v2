//! Generate a source-universe execution acceptance ledger from a TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_universe_execution_acceptance::{
    SourceUniverseExecutionAcceptanceLedger,
    write_source_universe_execution_acceptance_ledger_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate source-universe conversion execution acceptance from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_universe_execution_acceptance_ledger_from_spec_file(&spec_path)?;
    let ledger: SourceUniverseExecutionAcceptanceLedger =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_execution_acceptance_ledger = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("universes = {}", artifact.universe_count);
    println!("status = {:?}", ledger.status);
    println!("converted_universes = {}", ledger.converted_universes);
    println!(
        "ready_for_conversion_universes = {}",
        ledger.ready_for_conversion_universes
    );
    println!(
        "partially_ready_for_conversion_universes = {}",
        ledger.partially_ready_for_conversion_universes
    );
    println!("blocked_universes = {}", ledger.blocked_universes);
    println!(
        "total_required_single_object_operator_runs = {}",
        ledger.total_required_single_object_operator_runs
    );
    println!(
        "total_executable_single_object_operator_runs = {}",
        ledger.total_executable_single_object_operator_runs
    );
    println!(
        "total_withheld_conversion_objects = {}",
        ledger.total_withheld_conversion_objects
    );
    println!(
        "total_remaining_conversion_objects = {}",
        ledger.total_remaining_conversion_objects
    );
    Ok(())
}
