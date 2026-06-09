use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::backfill_run_spec_materialization::{
    write_backfill_run_spec_from_materialization_spec_file, BackfillRunSpecMaterializationSpec,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Materialize an operator run-spec from an accepted backfill tranche")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec: BackfillRunSpecMaterializationSpec =
        toml::from_str(&fs::read_to_string(&cli.spec)?)?;
    let artifact = write_backfill_run_spec_from_materialization_spec_file(&cli.spec)?;
    println!("backfill_run_spec = {}", artifact.path.display());
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("run_id = {}", spec.run_id);
    println!("output_prefix = {}", spec.output_prefix);
    Ok(())
}
