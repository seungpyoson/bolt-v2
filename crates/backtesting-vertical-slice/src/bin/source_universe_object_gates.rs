//! Generate source-universe object-gate materialization from a TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_universe_object_gates::{
    SourceUniverseObjectGateMaterialization,
    write_source_universe_object_gate_materialization_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate object gates for every source-universe conversion queue item")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_universe_object_gate_materialization_from_spec_file(&spec_path)?;
    let gates: SourceUniverseObjectGateMaterialization =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!("source_universe_object_gates = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", gates.status);
    println!("queue_id = {}", gates.queue_id);
    println!("universe_id = {}", gates.universe_id);
    println!("work_items = {}", gates.work_item_count);
    println!("accepted_gate_count = {}", gates.accepted_gate_count);
    println!("source_bindings = {}", gates.source_binding_count);
    println!("total_accepted_bytes = {}", gates.total_accepted_bytes);
    Ok(())
}
