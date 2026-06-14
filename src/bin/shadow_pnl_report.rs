use std::path::PathBuf;

use anyhow::Result;
use bolt_v2::shadow_pnl::{build_shadow_pnl_report, write_shadow_pnl_csv};
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[arg(long)]
    evidence_jsonl: PathBuf,
    #[arg(long)]
    settlements_jsonl: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let rows = build_shadow_pnl_report(&cli.evidence_jsonl, &cli.settlements_jsonl)?;
    let mut stdout = std::io::stdout().lock();
    write_shadow_pnl_csv(&rows, &mut stdout)?;
    Ok(())
}
