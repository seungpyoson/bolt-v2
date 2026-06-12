//! Generate source-universe operator inputs from accepted object gates.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_universe_operator_inputs::{
    SourceUniverseOperatorInputs, write_source_universe_operator_inputs_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize source-universe operator inputs from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_universe_operator_inputs_from_spec_file(&spec_path)?;
    let inputs: SourceUniverseOperatorInputs = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_operator_inputs = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!("status = {:?}", inputs.status);
    println!("planned_object_count = {}", inputs.planned_object_count);
    println!("ready_input_count = {}", inputs.ready_input_count);
    println!("blocked_input_count = {}", inputs.blocked_input_count);
    println!("instrument_spec_count = {}", inputs.instrument_spec_count);
    println!(
        "converter_mapping_count = {}",
        inputs.converter_mapping_count
    );
    Ok(())
}
