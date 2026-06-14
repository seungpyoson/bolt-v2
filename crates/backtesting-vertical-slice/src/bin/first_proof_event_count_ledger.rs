//! Generate a first-proof event-count ledger from a config-owned TOML spec.

use std::path::PathBuf;

use anyhow::Result;
use backtesting_vertical_slice::first_proof_selector::write_first_proof_event_count_ledger_from_spec_file;
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a first-proof event-count ledger from a TOML spec.")]
struct Cli {
    /// Path to the first-proof event-count ledger spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_first_proof_event_count_ledger_from_spec_file(&cli.spec)?;
    println!("event_count_ledger = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("source_rows = {}", artifact.source_rows);
    println!("event_count_rows = {}", artifact.event_count_rows);
    Ok(())
}
