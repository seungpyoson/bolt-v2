use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_accepted_tranche::{
    BackfillAcceptedTrancheManifest, write_backfill_accepted_tranche_manifest_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write an accepted object-level backfill tranche manifest")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_accepted_tranche_manifest_from_spec_file(&cli.spec)?;
    let manifest: BackfillAcceptedTrancheManifest =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_accepted_tranche_manifest = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", manifest.status);
    println!("object_count = {}", manifest.object_count);
    println!("accepted_bytes = {}", manifest.accepted_bytes);
    Ok(())
}
