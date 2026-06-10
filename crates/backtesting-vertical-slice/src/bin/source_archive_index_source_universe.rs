//! Generate a source-universe object manifest from a verified archive index.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use backtesting_vertical_slice::source_archive_index_source_universe::{
    SourceArchiveIndexSourceUniverseManifest,
    write_source_archive_index_source_universe_manifest_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize a source-universe object manifest from a source archive index")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_path(&cli.spec);
    let artifact = write_source_archive_index_source_universe_manifest_from_spec_file(&spec_path)?;
    let manifest: SourceArchiveIndexSourceUniverseManifest =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_archive_index_source_universe_manifest = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("manifest_id = {}", manifest.manifest_id);
    println!("universe_id = {}", manifest.universe_id);
    println!(
        "source_archive_index_manifest_id = {}",
        manifest.source_archive_index_manifest_id
    );
    println!("object_count = {}", artifact.object_count);
    println!("accepted_bytes = {}", artifact.accepted_bytes);
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
