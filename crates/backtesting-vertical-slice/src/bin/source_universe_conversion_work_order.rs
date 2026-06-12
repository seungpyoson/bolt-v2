//! Generate a source-universe conversion work order from operator inputs.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_universe_conversion_work_order::{
    SourceUniverseConversionWorkOrder, write_source_universe_conversion_work_order_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize executable source-universe conversion work from operator inputs")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_universe_conversion_work_order_from_spec_file(&spec_path)?;
    let work_order: SourceUniverseConversionWorkOrder =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_conversion_work_order = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", work_order.status);
    println!("universe_id = {}", work_order.universe_id);
    println!("planned_object_count = {}", work_order.planned_object_count);
    println!(
        "executable_record_count = {}",
        work_order.executable_record_count
    );
    println!(
        "withheld_record_count = {}",
        work_order.withheld_record_count
    );
    println!(
        "executable_source_bytes = {}",
        work_order.executable_source_bytes
    );
    println!(
        "withheld_source_bytes = {}",
        work_order.withheld_source_bytes
    );
    Ok(())
}
