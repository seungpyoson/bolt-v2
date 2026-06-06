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

use std::{path::Path, str::FromStr};

use anyhow::{Context, Result, bail, ensure};
use nautilus_backtest::{engine::BacktestEngine, node::BacktestNode, result::BacktestResult};
use nautilus_model::{data::TradeTick, identifiers::InstrumentId, types::Quantity};
use nautilus_trading::examples::strategies::{HurstVpinDirectional, HurstVpinDirectionalConfig};

use super::{
    canonical_trades::{
        CanonicalInstrumentIdentity, CanonicalTradesTable, normalize_bybit_spot_tick_trades,
    },
    catalog_projection::{
        CatalogProjection, SpotInstrumentSpec, project_canonical_trades_to_catalog,
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
    pub instrument_spec: &'a SpotInstrumentSpec,
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
            let trade_size = Quantity::from_str(trade_size_raw).map_err(|error| {
                anyhow::anyhow!("invalid {PARAM_TRADE_SIZE} {trade_size_raw:?}: {error}")
            })?;
            let config = HurstVpinDirectionalConfig::new(instrument_id, bar_type, trade_size);
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
        &inputs.manifest.run_id,
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
    // Bind the manifest's catalog instrument id to the projected/read-back id.
    // The engine queries the catalog under `manifest.catalog_input.nt_instrument_id`
    // (gate 4 -> BacktestDataConfig); if that diverged from the id the read-back
    // proof verified, NautilusTrader would query a different (or empty) directory
    // and the run would silently execute over zero accepted trades.
    ensure!(
        inputs.manifest.catalog_input.nt_instrument_id == projection.nt_instrument_id,
        "manifest catalog_input.nt_instrument_id {:?} does not match projected instrument {:?}",
        inputs.manifest.catalog_input.nt_instrument_id,
        projection.nt_instrument_id
    );
    let read_back = read_back_trade_ticks(inputs.catalog_root, &projection.nt_instrument_id)
        .context("catalog read-back failed")?;
    ensure!(
        read_back.len() == canonical_table.rows.len(),
        "catalog read-back {} does not match projected {} trades",
        read_back.len(),
        canonical_table.rows.len()
    );
    assert_read_back_matches(&read_back, &canonical_table, &projection.nt_instrument_id)?;
    // An optional manifest time window must overlap the accepted data's event
    // range, or the engine would filter out every accepted trade and still
    // "succeed" over zero data while stamping the accepted source/catalog hash.
    assert_time_window_overlaps_data(inputs.manifest, &canonical_table)?;

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
            "No orders were placed: the accepted data is trade-only and carries no quote ticks, \
             and the configured strategy's order entry is quote-driven. NautilusTrader still \
             aggregated the accepted trades into bars and ran the strategy's signal logic. This \
             reflects the TRADE_REPLAY fidelity of the source, not a defect."
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

#[cfg(test)]
mod tests {
    use super::time_window_excludes_all_data;

    #[test]
    fn time_window_without_bounds_admits_data() {
        assert_eq!(time_window_excludes_all_data(None, None, 100, 200), None);
    }

    #[test]
    fn time_window_overlapping_data_admits_data() {
        assert_eq!(
            time_window_excludes_all_data(Some(150), Some(180), 100, 200),
            None
        );
        // The start bound is inclusive, so a start exactly at the last event
        // still admits that boundary trade.
        assert_eq!(
            time_window_excludes_all_data(Some(200), None, 100, 200),
            None
        );
    }

    #[test]
    fn time_window_start_after_last_excludes_all() {
        assert_eq!(
            time_window_excludes_all_data(Some(201), None, 100, 200),
            Some("start_time")
        );
    }

    #[test]
    fn time_window_end_before_first_excludes_all() {
        assert_eq!(
            time_window_excludes_all_data(None, Some(99), 100, 200),
            Some("end_time")
        );
    }

    #[test]
    fn time_window_end_equal_to_first_excludes_all() {
        assert_eq!(
            time_window_excludes_all_data(None, Some(100), 100, 200),
            Some("end_time")
        );
    }
}

/// Prove the catalog read-back is value-faithful, not just count-equal: every
/// read-back tick must carry the projected instrument id, and the set of trade
/// ids must equal the canonical table's, so a projection that silently dropped,
/// duplicated, or relabelled ticks cannot pass the gate.
fn assert_read_back_matches(
    read_back: &[TradeTick],
    canonical_table: &CanonicalTradesTable,
    expected_instrument_id: &str,
) -> Result<()> {
    use std::collections::BTreeSet;
    for tick in read_back {
        ensure!(
            tick.instrument_id.to_string() == expected_instrument_id,
            "catalog read-back tick instrument {} does not match projected {expected_instrument_id}",
            tick.instrument_id
        );
    }
    let expected_ids: BTreeSet<&str> = canonical_table
        .rows
        .iter()
        .map(|row| row.trade_id.as_str())
        .collect();
    let actual_ids: Vec<String> = read_back.iter().map(|t| t.trade_id.to_string()).collect();
    let actual_set: BTreeSet<&str> = actual_ids.iter().map(String::as_str).collect();
    ensure!(
        expected_ids == actual_set,
        "catalog read-back trade ids do not match the canonical table"
    );
    Ok(())
}

/// Reject a manifest time window that excludes every accepted trade. The
/// canonical rows are validated monotonic by `event_time`, so the first and last
/// rows bound the accepted data's event range; a `start_time` after the last
/// trade (or an `end_time` at/ before the first) would leave the engine with no
/// data while the run still reports the accepted source/catalog hash.
fn assert_time_window_overlaps_data(
    manifest: &BacktestingRunManifest,
    canonical_table: &CanonicalTradesTable,
) -> Result<()> {
    let (Some(first), Some(last)) = (
        canonical_table.rows.first().map(|row| row.event_time),
        canonical_table.rows.last().map(|row| row.event_time),
    ) else {
        return Ok(());
    };
    match time_window_excludes_all_data(manifest.start_time, manifest.end_time, first, last) {
        None => Ok(()),
        Some("start_time") => bail!(
            "manifest start_time {:?} excludes all accepted data after event_time {last}",
            manifest.start_time
        ),
        Some(_) => bail!(
            "manifest end_time {:?} excludes all accepted data before event_time {first}",
            manifest.end_time
        ),
    }
}

/// Pure overlap test for a manifest `[start, end]` window against the accepted
/// data's `[first, last]` event range. Returns the name of the bound that
/// excludes all data, or `None` when the window admits at least one trade.
fn time_window_excludes_all_data(
    start: Option<i64>,
    end: Option<i64>,
    first: i64,
    last: i64,
) -> Option<&'static str> {
    if let Some(start) = start
        && start > last
    {
        return Some("start_time");
    }
    if let Some(end) = end
        && end <= first
    {
        return Some("end_time");
    }
    None
}
