//! Binary: run the NautilusTrader backtesting vertical slice over a real
//! accepted Bybit public-archive tick-trades object.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven run-spec TOML; the only
//! command-line inputs are filesystem paths. The orchestration itself lives in
//! [`backtesting_vertical_slice::operator`] so it is unit-testable; this binary
//! is a thin CLI shim that reads the inputs, runs the operator path, and prints
//! the produced artifacts.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use backtesting_vertical_slice::{
    nt_catalog_capability::NtCatalogSsmCredentialResolver,
    operator::{RunSpec, run_from_run_spec_with_artifact_store},
};

#[derive(Parser)]
#[command(about = "Run the NautilusTrader backtesting vertical slice over an accepted dataset.")]
struct Cli {
    /// Path to the run-spec TOML (dataset facts: object, source proof, manifest).
    #[arg(long)]
    run_spec: PathBuf,
    /// Local path to the accepted `.csv.gz` object whose SHA-256 the run-spec pins.
    #[arg(long)]
    object_gz: PathBuf,
    /// Output directory for produced artifacts.
    #[arg(long)]
    output_dir: PathBuf,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_text = fs::read_to_string(&cli.run_spec)
        .with_context(|| format!("read run-spec {}", cli.run_spec.display()))?;
    let spec: RunSpec = toml::from_str(&spec_text).context("parse run-spec TOML")?;
    let gz_bytes = fs::read(&cli.object_gz)
        .with_context(|| format!("read object {}", cli.object_gz.display()))?;

    let artifact_root = spec.artifact_store.resolve()?;
    let credential_resolver =
        NtCatalogSsmCredentialResolver::from_region(artifact_root.s3_region()).await?;
    let credentials = credential_resolver
        .resolve(&spec.nt_catalog_capability_proof.ssm_parameter_refs)
        .await?;
    let store = artifact_root.build_s3_object_store_with_credentials(&credentials)?;
    let artifacts =
        run_from_run_spec_with_artifact_store(&spec, &gz_bytes, &cli.output_dir, &store).await?;
    let output = &artifacts.output;

    println!("accepted_object_sha256 = {}", artifacts.verified_sha256);
    println!(
        "source_proof_id = {}",
        artifacts.accepted_source_proof.source_proof_id
    );
    println!(
        "canonical_trades_rows = {}",
        output.canonical_table.rows.len()
    );
    println!(
        "canonical_artifact = {}",
        artifacts.canonical_artifact_path.display()
    );
    if let Some(canonical_catalog_uri) = &artifacts.canonical_catalog_uri {
        println!("nt_catalog_uri = {canonical_catalog_uri}");
    }
    println!(
        "local_nt_catalog_root = {}",
        artifacts.catalog_root.display()
    );
    println!("catalog_hash = {}", output.projection.catalog_hash);
    println!("catalog_read_back_trade_ticks = {}", output.read_back_count);
    println!("nt_version = {}", output.contract.nt_version);
    println!(
        "strategy_config_hash = {}",
        output.contract.strategy_config_hash
    );
    println!(
        "backtest_run_config_id = {:?}",
        output.nt_result.run_config_id
    );
    println!(
        "nt_iterations = {} (market-data points processed by the engine)",
        output.nt_result.iterations
    );
    println!(
        "nt_backtest_start = {:?}, nt_backtest_end = {:?}",
        output.nt_result.backtest_start, output.nt_result.backtest_end
    );
    println!(
        "nt_total_events = {}, nt_total_orders = {}, nt_total_positions = {}",
        output.nt_result.total_events,
        output.nt_result.total_orders,
        output.nt_result.total_positions
    );
    println!("fidelity_class = {:?}", output.contract.fidelity_class);
    println!("result_contract = {}", artifacts.contract_path.display());
    println!("accepted_source_proof = {}", artifacts.proof_path.display());
    Ok(())
}
