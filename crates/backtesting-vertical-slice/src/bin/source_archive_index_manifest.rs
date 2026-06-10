//! Generate a source archive index manifest from a captured index snapshot.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
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
    let spec_path = resolve_existing_path(&cli.spec);
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
