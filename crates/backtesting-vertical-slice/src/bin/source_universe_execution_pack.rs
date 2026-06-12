//! Generate source-universe operator execution inputs from a work order.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_universe_execution_pack::{
    SourceUniverseExecutionPack, write_source_universe_execution_pack_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize source-universe run specs and execution plans")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_universe_execution_pack_from_spec_file(&spec_path)?;
    let pack: SourceUniverseExecutionPack = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_execution_pack = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", pack.status);
    println!("universe_id = {}", pack.universe_id);
    println!("planned_object_count = {}", pack.planned_object_count);
    println!("executable_record_count = {}", pack.executable_record_count);
    println!("withheld_record_count = {}", pack.withheld_record_count);
    println!(
        "materialized_record_count = {}",
        pack.materialized_record_count
    );
    println!(
        "skipped_executable_record_count = {}",
        pack.skipped_executable_record_count
    );
    Ok(())
}
