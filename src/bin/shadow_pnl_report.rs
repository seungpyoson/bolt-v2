use std::path::PathBuf;

use anyhow::Result;
use bolt_v2::bolt_v3_config::load_bolt_v3_config;
use bolt_v2::shadow_pnl::{build_shadow_pnl_report, write_shadow_pnl_csv};
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    evidence_jsonl: PathBuf,
    #[arg(long)]
    settlements_jsonl: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let loaded = load_bolt_v3_config(&cli.config)?;
    let rows = build_shadow_pnl_report(
        &cli.evidence_jsonl,
        &cli.settlements_jsonl,
        loaded
            .root
            .persistence
            .decision_evidence
            .recovery_evidence_max_bytes,
    )?;
    let mut stdout = std::io::stdout().lock();
    write_shadow_pnl_csv(&rows, &mut stdout)?;
    Ok(())
}
