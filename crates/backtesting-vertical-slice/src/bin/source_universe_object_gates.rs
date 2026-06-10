//! Generate source-universe object-gate materialization from a TOML spec.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
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
    let spec_path = resolve_existing_path(&cli.spec);
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

fn resolve_existing_path(path: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        return path.to_path_buf();
    }

    let mut anchors = Vec::new();
    if let Ok(current_dir) = env::current_dir() {
        anchors.push(current_dir);
    }
    anchors.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    for anchor in anchors {
        for ancestor in anchor.ancestors() {
            let candidate = ancestor.join(path);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    path.to_path_buf()
}
