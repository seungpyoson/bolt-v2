//! Generate a conversion work queue from an accepted source-universe manifest.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
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
    let spec_path = resolve_existing_input_path(&cli.spec);
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
