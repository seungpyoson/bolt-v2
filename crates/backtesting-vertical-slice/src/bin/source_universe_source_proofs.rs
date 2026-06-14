//! Generate accepted source proofs from source-universe category manifests.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::source_universe_source_proofs::{
    SourceUniverseSourceProofSet, write_source_universe_source_proof_set_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write source-universe source proofs from a TOML spec")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_source_universe_source_proof_set_from_spec_file(&cli.spec)?;
    let proof_set: SourceUniverseSourceProofSet =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_universe_source_proofs = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("proofs = {}", artifact.proof_count);
    println!("accepted_proofs = {}", proof_set.accepted_proof_count);
    println!(
        "total_completed_objects = {}",
        proof_set.total_completed_objects
    );
    println!("total_accepted_bytes = {}", proof_set.total_accepted_bytes);
    Ok(())
}
