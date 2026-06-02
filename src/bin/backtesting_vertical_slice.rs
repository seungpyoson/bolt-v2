//! Binary: run the NautilusTrader backtesting vertical slice over a real
//! accepted Bybit public-archive tick-trades object.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven run-spec TOML; the only
//! command-line inputs are filesystem paths. The accepted object's SHA-256 is
//! re-verified against the run-spec before any normalization, so raw staged data
//! can never reach the backtest without passing source-proof acceptance.

use std::{fs, io::Read, path::PathBuf};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use bolt_v2::backtesting_vertical_slice::{
    canonical_trades::CanonicalInstrumentIdentity,
    catalog_projection::BybitSpotInstrumentSpec,
    result_contract::ResultArtifactUris,
    run_manifest::BacktestingRunManifest,
    runner::{BacktestRunInputs, run_backtest},
    source_proof::{
        AcceptanceMode, IngestManifestObjectRecord, SourceProofReport, select_accepted_dataset,
    },
};

const CANONICAL_ARTIFACT_FILE: &str = "canonical-trades.parquet";
const CATALOG_DIR: &str = "nt-catalog";
const RESULT_CONTRACT_FILE: &str = "backtest-result-contract.json";
const ACCEPTED_SOURCE_PROOF_FILE: &str = "accepted-source-proof.json";

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

#[derive(Deserialize)]
struct RunSpec {
    capture_time_utc: String,
    created_at_utc: String,
    accepted_by: String,
    accepted_at_utc: String,
    accepted_object: IngestManifestObjectRecord,
    source_proof: SourceProofReport,
    instrument_spec: BybitSpotInstrumentSpec,
    identity: CanonicalInstrumentIdentity,
    manifest: BacktestingRunManifest,
}

fn rfc3339_to_nanos(value: &str) -> Result<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("invalid RFC3339 timestamp {value:?}"))?
        .timestamp_nanos_opt()
        .context("timestamp out of representable nanosecond range")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_text = fs::read_to_string(&cli.run_spec)
        .with_context(|| format!("read run-spec {}", cli.run_spec.display()))?;
    let spec: RunSpec = toml::from_str(&spec_text).context("parse run-spec TOML")?;

    // Re-verify the accepted object content hash against the run-spec.
    let gz_bytes = fs::read(&cli.object_gz)
        .with_context(|| format!("read object {}", cli.object_gz.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&gz_bytes);
    let verified_sha256 = hex::encode(hasher.finalize());
    ensure!(
        verified_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {verified_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );

    // Decompress to CSV text.
    let mut csv_text = String::new();
    GzDecoder::new(&gz_bytes[..])
        .read_to_string(&mut csv_text)
        .context("decompress gzip object")?;

    // Gate 1: accept the source proof and bind the object via the ledger.
    let accepted_proof = spec
        .source_proof
        .clone()
        .accept(
            AcceptanceMode::Manual,
            spec.accepted_by.clone(),
            spec.accepted_at_utc.clone(),
        )
        .map_err(|error| anyhow::anyhow!("source-proof acceptance failed: {error}"))?;
    let accepted =
        select_accepted_dataset(&accepted_proof, &spec.accepted_object, &verified_sha256)
            .map_err(|error| anyhow::anyhow!("accepted-data ledger rejected object: {error}"))?;

    fs::create_dir_all(&cli.output_dir)
        .with_context(|| format!("create output dir {}", cli.output_dir.display()))?;
    let canonical_path = cli.output_dir.join(CANONICAL_ARTIFACT_FILE);
    let catalog_root = cli.output_dir.join(CATALOG_DIR);
    let catalog_path = catalog_root
        .to_str()
        .context("catalog path is not valid UTF-8")?
        .to_string();
    let contract_path = cli.output_dir.join(RESULT_CONTRACT_FILE);
    let proof_path = cli.output_dir.join(ACCEPTED_SOURCE_PROOF_FILE);

    // Bind the manifest catalog input to the local projection root.
    let mut manifest = spec.manifest.clone();
    manifest.catalog_input.catalog_path = catalog_path.clone();

    let output = run_backtest(BacktestRunInputs {
        accepted: &accepted,
        identity: &spec.identity,
        instrument_spec: &spec.instrument_spec,
        csv_text: &csv_text,
        capture_time_nanos: rfc3339_to_nanos(&spec.capture_time_utc)?,
        manifest: &manifest,
        canonical_artifact_path: &canonical_path,
        catalog_root: &catalog_root,
        created_at: &spec.created_at_utc,
        artifact_uris: ResultArtifactUris {
            source_proof_uri: proof_path.to_string_lossy().into_owned(),
            canonical_table_uri: canonical_path.to_string_lossy().into_owned(),
            nt_catalog_uri: catalog_path,
            result_contract_uri: contract_path.to_string_lossy().into_owned(),
        },
    })?;

    fs::write(
        &proof_path,
        serde_json::to_string_pretty(&accepted_proof).context("serialize accepted source proof")?,
    )
    .with_context(|| format!("write {}", proof_path.display()))?;
    fs::write(
        &contract_path,
        serde_json::to_string_pretty(&output.contract).context("serialize result contract")?,
    )
    .with_context(|| format!("write {}", contract_path.display()))?;

    println!("accepted_object_sha256 = {verified_sha256}");
    println!("source_proof_id = {}", accepted.source_proof_id);
    println!(
        "canonical_trades_rows = {}",
        output.canonical_table.rows.len()
    );
    println!("canonical_artifact = {}", canonical_path.display());
    println!("nt_catalog_root = {}", catalog_root.display());
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
    println!("result_contract = {}", contract_path.display());
    println!("accepted_source_proof = {}", proof_path.display());
    Ok(())
}
