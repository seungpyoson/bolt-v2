//! Generate a venue-level backfill conversion completion ledger from a TOML spec.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use backtesting_vertical_slice::backfill_conversion_completion::{
    BackfillConversionCompletionLedger, write_backfill_conversion_completion_ledger_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate conversion publication completion from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_path = resolve_existing_path(&cli.spec);
    let artifact = write_backfill_conversion_completion_ledger_from_spec_file(&spec_path)?;
    let ledger: BackfillConversionCompletionLedger =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_conversion_completion_ledger = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("records = {}", artifact.record_count);
    println!("status = {:?}", ledger.status);
    println!("published_records = {}", ledger.published_records);
    println!("mapping_proven_records = {}", ledger.mapping_proven_records);
    println!("total_canonical_rows = {}", ledger.total_canonical_rows);
    println!("total_nt_iterations = {}", ledger.total_nt_iterations);
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
