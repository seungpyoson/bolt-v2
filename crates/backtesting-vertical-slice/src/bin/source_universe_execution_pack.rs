//! Generate source-universe operator execution inputs from a work order.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
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
    let spec_path = resolve_existing_path(&cli.spec);
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
