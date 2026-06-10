//! Generate a conversion work queue from an accepted source-universe manifest.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use backtesting_vertical_slice::source_universe_conversion_queue::{
    SourceUniverseConversionQueue, write_source_universe_conversion_queue_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize a source-universe conversion work queue from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_path(&cli.spec);
    let artifact = write_source_universe_conversion_queue_from_spec_file(&spec_path)?;
    let queue: SourceUniverseConversionQueue = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_conversion_queue = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", queue.status);
    println!("manifest_id = {}", queue.manifest_id);
    println!("universe_id = {}", queue.universe_id);
    println!("work_items = {}", queue.work_item_count);
    println!(
        "pending_conversion_items = {}",
        queue.pending_conversion_items
    );
    println!("total_source_bytes = {}", queue.total_source_bytes);
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
