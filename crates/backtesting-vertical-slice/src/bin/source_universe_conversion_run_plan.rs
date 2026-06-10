//! Generate a bounded conversion run plan from source-universe object gates.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
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
    let spec_path = resolve_existing_path(&cli.spec);
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
