//! Generate a source-universe conversion work order from operator inputs.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
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
    let spec_path = resolve_existing_path(&cli.spec);
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
