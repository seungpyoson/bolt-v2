//! Generate a source archive discovery seed from a TOML spec.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use backtesting_vertical_slice::source_archive_discovery_seed::{
    SourceArchiveDiscoverySeed, write_source_archive_discovery_seed_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate source archive discovery seed evidence from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_path(&cli.spec);
    let artifact = write_source_archive_discovery_seed_from_spec_file(&spec_path)?;
    let seed: SourceArchiveDiscoverySeed = serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_archive_discovery_seed = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", seed.status);
    println!("discovery_id = {}", seed.discovery_id);
    println!("source_bindings = {}", seed.source_binding_count);
    println!(
        "representative_objects = {}",
        seed.representative_object_count
    );
    println!(
        "total_representative_object_bytes = {}",
        seed.total_representative_object_bytes
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
