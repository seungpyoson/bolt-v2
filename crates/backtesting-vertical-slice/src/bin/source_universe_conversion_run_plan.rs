//! Generate a bounded conversion run plan from source-universe object gates.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_universe_conversion_run_plan::{
    SourceUniverseConversionRunPlan, write_source_universe_conversion_run_plan_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize bounded conversion runs from source-universe object gates")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_universe_conversion_run_plan_from_spec_file(&spec_path)?;
    let plan: SourceUniverseConversionRunPlan = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_conversion_run_plan = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", plan.status);
    println!("gate_id = {}", plan.gate_id);
    println!("universe_id = {}", plan.universe_id);
    println!("runs = {}", plan.run_count);
    println!("objects = {}", plan.object_count);
    println!("planned_objects = {}", plan.planned_object_count);
    println!("planned_source_bytes = {}", plan.planned_source_bytes);
    Ok(())
}
