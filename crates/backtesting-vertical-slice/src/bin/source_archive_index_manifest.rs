//! Generate a source archive index manifest from a captured index snapshot.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::path_resolution::resolve_existing_input_path;
use backtesting_vertical_slice::source_archive_index_manifest::{
    SourceArchiveIndexManifest, write_source_archive_index_manifest_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate public archive index snapshot evidence from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_input_path(&cli.spec);
    let artifact = write_source_archive_index_manifest_from_spec_file(&spec_path)?;
    let manifest: SourceArchiveIndexManifest = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_archive_index_manifest = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", manifest.status);
    println!("manifest_id = {}", manifest.manifest_id);
    println!("snapshot_id = {}", manifest.snapshot_id);
    println!("objects = {}", manifest.object_count);
    println!("verified_heads = {}", manifest.verified_head_count);
    println!(
        "total_content_length_bytes = {}",
        manifest.total_content_length_bytes
    );
    Ok(())
}
