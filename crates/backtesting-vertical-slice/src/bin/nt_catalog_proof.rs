use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::{
    artifact_store_secrets::ArtifactStoreSsmResolver,
    nt_catalog_proof::{NtCatalogProofReport, run_nt_catalog_proof_from_spec_file_with_resolver},
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Prove NautilusTrader multi-instrument catalog I/O against a configured URI")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut resolver = ArtifactStoreSsmResolver::new()?;
    let mut resolve_secret = |region: &str, path: &str| {
        resolver
            .resolve(region, path)
            .map_err(|error| error.to_string())
    };
    let artifact =
        run_nt_catalog_proof_from_spec_file_with_resolver(&cli.spec, &mut resolve_secret)?;
    let report: NtCatalogProofReport = serde_json::from_slice(&fs::read(&artifact.report_path)?)?;
    println!(
        "nt_catalog_proof_report = {}",
        artifact.report_path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("report_bytes = {}", artifact.report_bytes);
    println!("catalog_uri = {}", artifact.catalog_uri);
    println!("catalog_protocol = {}", report.catalog_protocol);
    println!("instrument_count = {}", report.nt_instrument_count);
    println!("trade_ticks = {}", report.nt_trade_ticks);
    println!("backtest_iterations = {}", report.nt_backtest_iterations);
    println!(
        "direct_s3_catalog_access_proven = {}",
        report.direct_s3_catalog_access_proven
    );
    Ok(())
}
