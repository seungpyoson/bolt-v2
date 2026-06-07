use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_source_proof_scope::{
    BackfillSourceProofScopeReport, write_backfill_source_proof_scope_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Evaluate object-level source-proof scope over a backfill manifest")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_backfill_source_proof_scope_report_from_spec_file(&cli.spec)?;
    let report: BackfillSourceProofScopeReport =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "backfill_source_proof_scope_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!("matching_object_count = {}", report.matching_object_count);
    println!(
        "object_level_tranche_required = {}",
        report.object_level_tranche_required
    );
    Ok(())
}
