//! Generate source-universe operator inputs from accepted object gates.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
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
    let spec_path = resolve_existing_path(&cli.spec);
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
