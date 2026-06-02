//! Gate 5 — `BacktestNode` execution and end-to-end orchestration.
//!
//! Ties the vertical slice together: an [`AcceptedDataset`] is normalized into a
//! canonical trades table, written as a canonical Parquet artifact, projected
//! into a NautilusTrader `ParquetDataCatalog`, proven readable, mapped through a
//! validated [`BacktestingRunManifest`] into NautilusTrader configs, executed by
//! a `BacktestNode` running an existing compiled Rust strategy, and reported as
//! an objective [`BacktestResultContract`].
//!
//! There is no custom simulation behaviour: NautilusTrader owns the engine,
//! catalog, fills, and results.

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use nautilus_backtest::{engine::BacktestEngine, node::BacktestNode, result::BacktestResult};
use nautilus_model::{identifiers::InstrumentId, types::Quantity};
use nautilus_trading::examples::strategies::{HurstVpinDirectional, HurstVpinDirectionalConfig};

use super::{
    canonical_trades::{
        CanonicalInstrumentIdentity, CanonicalTradesTable, normalize_bybit_spot_tick_trades,
    },
    catalog_projection::{
        BybitSpotInstrumentSpec, CatalogProjection, project_canonical_trades_to_catalog,
        read_back_trade_ticks,
    },
    result_contract::{
        BacktestResultContract, ResultArtifactUris, ResultContractInputs, build_result_contract,
    },
    run_manifest::{BacktestingRunManifest, STRATEGY_HURST_VPIN_DIRECTIONAL},
    source_proof::AcceptedDataset,
};

/// Strategy parameter key for the bar type.
const PARAM_BAR_TYPE: &str = "bar_type";
/// Strategy parameter key for the trade size.
const PARAM_TRADE_SIZE: &str = "trade_size";

/// Inputs for one end-to-end backtest run over accepted data.
pub struct BacktestRunInputs<'a> {
    pub accepted: &'a AcceptedDataset,
    pub identity: &'a CanonicalInstrumentIdentity,
    pub instrument_spec: &'a BybitSpotInstrumentSpec,
    /// Decompressed text of the accepted object (hash already verified).
    pub csv_text: &'a str,
    pub capture_time_nanos: i64,
    pub manifest: &'a BacktestingRunManifest,
    /// Local path for the canonical normalized Parquet artifact.
    pub canonical_artifact_path: &'a Path,
    /// Local catalog projection root.
    pub catalog_root: &'a Path,
    pub created_at: &'a str,
    pub artifact_uris: ResultArtifactUris,
}

/// All artifacts produced by an end-to-end run.
pub struct BacktestRunOutput {
    pub canonical_table: CanonicalTradesTable,
    pub projection: CatalogProjection,
    pub read_back_count: usize,
    pub nt_result: BacktestResult,
    pub contract: BacktestResultContract,
}

/// Add the manifest-selected compiled Rust strategy to the engine.
///
/// Only registered compiled Rust strategies are admissible; the manifest is
/// already validated, this is defence in depth.
fn add_manifest_strategy(
    engine: &mut BacktestEngine,
    manifest: &BacktestingRunManifest,
) -> Result<()> {
    let strategy = &manifest.strategy;
    match strategy.registry_key.as_str() {
        STRATEGY_HURST_VPIN_DIRECTIONAL => {
            let instrument_id: InstrumentId = manifest
                .catalog_input
                .nt_instrument_id
                .parse()
                .with_context(|| {
                    format!(
                        "invalid instrument id {:?}",
                        manifest.catalog_input.nt_instrument_id
                    )
                })?;
            let bar_type = strategy
                .parameters
                .get(PARAM_BAR_TYPE)
                .with_context(|| format!("strategy parameter {PARAM_BAR_TYPE} is required"))?
                .parse()
                .with_context(|| format!("invalid {PARAM_BAR_TYPE}"))?;
            let trade_size_raw = strategy
                .parameters
                .get(PARAM_TRADE_SIZE)
                .with_context(|| format!("strategy parameter {PARAM_TRADE_SIZE} is required"))?;
            let config = HurstVpinDirectionalConfig::new(
                instrument_id,
                bar_type,
                Quantity::from(trade_size_raw.as_str()),
            );
            engine
                .add_strategy(HurstVpinDirectional::new(config))
                .context("add HurstVpinDirectional strategy")
        }
        other => bail!("strategy {other:?} is not a registered compiled Rust strategy"),
    }
}

/// Run one minimal NautilusTrader `BacktestNode` backtest over accepted data and
/// return all produced artifacts plus the objective result contract.
///
/// # Errors
///
/// Returns an error at the first failed gate: normalization, manifest
/// validation, catalog projection, read-back proof, NautilusTrader execution, or
/// result-contract construction.
pub fn run_backtest(inputs: BacktestRunInputs<'_>) -> Result<BacktestRunOutput> {
    // Gate 2: canonical normalization + canonical artifact.
    let canonical_table = normalize_bybit_spot_tick_trades(
        inputs.accepted,
        inputs.identity,
        inputs.csv_text,
        inputs.capture_time_nanos,
    )
    .context("canonical normalization failed")?;
    canonical_table
        .write_parquet(inputs.canonical_artifact_path)
        .context("write canonical artifact failed")?;

    // Gate 4: manifest validation, bound to the accepted dataset.
    inputs
        .manifest
        .validate(inputs.accepted)
        .map_err(|error| anyhow::anyhow!("manifest validation failed: {error}"))?;
    let catalog_root_str = inputs
        .catalog_root
        .to_str()
        .context("catalog root is not valid UTF-8")?;
    ensure!(
        inputs.manifest.catalog_input.catalog_path == catalog_root_str,
        "manifest catalog_path {:?} does not match projection root {catalog_root_str:?}",
        inputs.manifest.catalog_input.catalog_path
    );

    // Gate 3: NautilusTrader catalog projection + read-back proof.
    let projection = project_canonical_trades_to_catalog(
        &canonical_table,
        inputs.instrument_spec,
        inputs.catalog_root,
    )
    .context("catalog projection failed")?;
    let read_back = read_back_trade_ticks(inputs.catalog_root, &projection.nt_instrument_id)
        .context("catalog read-back failed")?;
    ensure!(
        read_back.len() == canonical_table.rows.len(),
        "catalog read-back {} does not match projected {} trades",
        read_back.len(),
        canonical_table.rows.len()
    );

    // Gate 5: BacktestNode execution.
    let run_config = inputs
        .manifest
        .to_nt_run_config()
        .map_err(|error| anyhow::anyhow!("manifest to NautilusTrader config failed: {error}"))?;
    let mut node = BacktestNode::new(vec![run_config]).context("construct BacktestNode")?;
    node.build().context("build BacktestNode")?;
    {
        let engine = node
            .get_engine_mut(&inputs.manifest.run_id)
            .with_context(|| format!("no engine for run id {}", inputs.manifest.run_id))?;
        add_manifest_strategy(engine, inputs.manifest)?;
    }
    let mut results = node.run().context("run BacktestNode")?;
    ensure!(
        results.len() == 1,
        "expected exactly one backtest result, got {}",
        results.len()
    );
    let nt_result = results.remove(0);

    // Gate 6: objective result contract.
    let mut warnings = Vec::new();
    if nt_result.total_orders == 0 {
        warnings.push(
            "Trade-only accepted data: the strategy received no quotes or bars, so no orders \
             were placed. This reflects the TRADE_REPLAY fidelity of the source, not a defect."
                .to_string(),
        );
    }
    let contract = build_result_contract(ResultContractInputs {
        run_id: &inputs.manifest.run_id,
        source_proof_id: &inputs.accepted.source_proof_id,
        source_proof_version: inputs.accepted.source_proof_version,
        catalog_hash: &projection.catalog_hash,
        strategy: &inputs.manifest.strategy,
        run_purpose: run_purpose_label(inputs.manifest),
        market_structure_fixture: market_structure_label(inputs.manifest),
        fidelity_class: canonical_table.fidelity_class,
        claim_limits: canonical_table.forbidden_claims.clone(),
        warnings,
        mechanical_blockers: Vec::new(),
        nt_result: &nt_result,
        artifact_uris: inputs.artifact_uris,
        created_at: inputs.created_at,
    })
    .map_err(|error| anyhow::anyhow!("result contract construction failed: {error}"))?;

    Ok(BacktestRunOutput {
        canonical_table,
        projection,
        read_back_count: read_back.len(),
        nt_result,
        contract,
    })
}

fn run_purpose_label(manifest: &BacktestingRunManifest) -> &'static str {
    use super::run_manifest::RunPurpose;
    match manifest.run_purpose {
        RunPurpose::Normal => "normal",
        RunPurpose::Reproduction => "reproduction",
        RunPurpose::Audit => "audit",
        RunPurpose::Regression => "regression",
        RunPurpose::Migration => "migration",
    }
}

fn market_structure_label(manifest: &BacktestingRunManifest) -> &'static str {
    use super::run_manifest::MarketStructureFixture;
    match manifest.market_structure_fixture {
        MarketStructureFixture::BinaryOption => "binary-option",
        MarketStructureFixture::PerpsSpot => "perps-spot",
    }
}
