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

use std::{path::Path, str::FromStr, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use bolt_v2::{
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3BasketAdmissionDecisionEvidence,
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3PositionSizerRebuildAuditEvidence, BoltV3StrategyInputEvidenceSnapshot,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
    },
    bolt_v3_order_execution::{BoltV3OrderExecutionMode, BoltV3OrderExecutionPolicy},
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    strategies::{
        binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder,
        registry::{FeeProvider, StrategyBuildContext},
    },
};
use futures_util::future::BoxFuture;
use nautilus_analysis::analyzer::PortfolioAnalyzer;
use nautilus_backtest::{engine::BacktestEngine, node::BacktestNode, result::BacktestResult};
use nautilus_model::{
    data::{
        Bar, BarSpecification, FundingRateUpdate, IndexPriceUpdate, MarkPriceUpdate,
        OrderBookDelta, QuoteTick, TradeTick,
    },
    enums::{AggregationSource, AggressorSide, BookAction, OrderSide, OrderStatus, PriceType},
    identifiers::{InstrumentId, Venue},
    orders::Order,
    types::Quantity,
};
use nautilus_trading::examples::strategies::{HurstVpinDirectional, HurstVpinDirectionalConfig};
use rust_decimal::Decimal;

use super::{
    canonical_trades::{
        CanonicalInstrumentIdentity, CanonicalTradeRow, CanonicalTradesTable, ConverterConfig,
        TradeAggressorSide, normalize_registered_trade_converter,
    },
    catalog_projection::{
        CatalogInstrumentSpecSource, CatalogProjection, project_canonical_trades_to_catalog,
        read_back_trade_ticks, ts_event_nanos, ts_init_nanos,
    },
    conversion_boundary::{
        ConversionCatalogMetadata, ConversionCheckpoint, ConversionFingerprint, ConversionManifest,
    },
    domain_metrics::{
        domain_statistics_from_analyzer, register_domain_statistics, resolve_domain_statistics,
    },
    mechanical_probe_strategy::{MechanicalTradeReplayProbe, MechanicalTradeReplayProbeConfig},
    result_contract::{
        BacktestResultContract, ResultArtifactUris, ResultContractInputs, build_result_contract,
    },
    run_manifest::{
        BacktestingRunManifest, NtSurfaceClassification, STRATEGY_BINARY_ORACLE_EDGE_TAKER,
        STRATEGY_HURST_VPIN_DIRECTIONAL, STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE,
        STRATEGY_PARAM_ORDER_EXECUTION_MODE,
    },
    source_proof::{AcceptedDataset, SourceProofFidelityClass},
};

/// Strategy parameter key for the bar type.
const PARAM_BAR_TYPE: &str = "bar_type";
/// Strategy parameter key for the trade size.
const PARAM_TRADE_SIZE: &str = "trade_size";
/// Strategy parameter key for the normalized binary-oracle builder TOML.
const PARAM_CONFIG_TOML: &str = "config_toml";
/// Strategy parameter key for the backtest fee-provider assumption.
const PARAM_FEE_BPS: &str = "fee_bps";

#[derive(Debug)]
struct BacktestDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for BacktestDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        Ok(())
    }

    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        Ok(())
    }

    fn record_admission_decision(&self, _decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct ManifestFeeProvider {
    fee_bps: Decimal,
}

impl FeeProvider for ManifestFeeProvider {
    fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
        Some(self.fee_bps)
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}

/// Strategy parameter key for the number of delivered trades before the entry order.
const PARAM_ENTRY_AFTER_TRADES: &str = "entry_after_trades";
/// Strategy parameter key for the number of further delivered trades before the close.
const PARAM_EXIT_AFTER_TRADES: &str = "exit_after_trades";
/// Strategy parameter key for the entry order side.
const PARAM_SIDE: &str = "side";

fn nt_surface_classification_label(classification: NtSurfaceClassification) -> &'static str {
    match classification {
        NtSurfaceClassification::Defaulted => "defaulted",
        NtSurfaceClassification::PassThrough => "pass_through",
        NtSurfaceClassification::CustomOwned => "custom_owned",
        NtSurfaceClassification::UnsupportedForNow => "unsupported_for_now",
    }
}

pub(crate) fn nt_extension_surface_claim_limits(
    manifest: &BacktestingRunManifest,
) -> Result<Vec<String>> {
    Ok(manifest
        .resolved_nt_surfaces()?
        .into_iter()
        .map(|surface| {
            format!(
                "NT {} surface {} nt_field={} resolved_value={}",
                nt_surface_classification_label(surface.classification),
                surface.surface,
                surface.nt_field,
                surface.resolved_value
            )
        })
        .collect())
}

pub(crate) fn result_contract_warnings(nt_result: &BacktestResult) -> Vec<String> {
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
    warnings
}

/// Inputs for one end-to-end backtest run over accepted data.
pub struct BacktestRunInputs<'a> {
    pub accepted: &'a AcceptedDataset,
    pub identity: &'a CanonicalInstrumentIdentity,
    pub instrument_spec: &'a dyn CatalogInstrumentSpecSource,
    /// Decompressed text of the accepted object (hash already verified).
    pub csv_text: &'a str,
    pub capture_time_nanos: i64,
    pub manifest: &'a BacktestingRunManifest,
    pub contract_manifest_hash: &'a str,
    pub converter: &'a ConverterConfig,
    /// Local path for the canonical normalized Parquet artifact.
    pub canonical_artifact_path: &'a Path,
    /// Local catalog projection root.
    pub catalog_root: &'a Path,
    pub selector_provenance: Option<BacktestSelectorProvenance<'a>>,
    pub created_at: &'a str,
    pub artifact_uris: ResultArtifactUris,
}

/// Source-selector provenance that must bind any L2 replay result contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BacktestSelectorProvenance<'a> {
    pub event_count_ledger_hash: &'a str,
    pub selected_asset_ids_hash: &'a str,
}

fn selector_provenance_hashes<'a>(
    fidelity_class: SourceProofFidelityClass,
    provenance: Option<BacktestSelectorProvenance<'a>>,
) -> Result<(Option<&'a str>, Option<&'a str>)> {
    if fidelity_class == SourceProofFidelityClass::L2Replay && provenance.is_none() {
        bail!("L2 replay result contract requires selector provenance");
    }
    Ok(match provenance {
        Some(provenance) => (
            Some(provenance.event_count_ledger_hash),
            Some(provenance.selected_asset_ids_hash),
        ),
        None => (None, None),
    })
}

/// All artifacts produced by an end-to-end run.
pub struct BacktestRunOutput {
    pub canonical_table: CanonicalTradesTable,
    pub projection: CatalogProjection,
    pub conversion_checkpoint: ConversionCheckpoint,
    pub conversion_manifest: ConversionManifest,
    pub conversion_catalog_metadata: ConversionCatalogMetadata,
    pub conversion_checkpoint_hash: String,
    pub conversion_manifest_hash: String,
    pub read_back_count: usize,
    pub nt_result: BacktestResult,
    /// Terminal state of every order the engine produced, captured from the
    /// post-run cache. Lets callers assert order-level outcomes (e.g. every
    /// submitted order reached `Filled`) with the per-order event trail that
    /// explains any non-fill terminal state.
    pub order_terminals: Vec<OrderTerminalRecord>,
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
            let catalog_input = manifest.primary_catalog_input().map_err(|error| {
                anyhow::anyhow!("strategy instrument requires catalog input: {error}")
            })?;
            let instrument_id: InstrumentId =
                catalog_input.nt_instrument_id.parse().with_context(|| {
                    format!("invalid instrument id {:?}", catalog_input.nt_instrument_id)
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
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => {
            let raw_config = strategy
                .parameters
                .get(PARAM_CONFIG_TOML)
                .with_context(|| format!("strategy parameter {PARAM_CONFIG_TOML} is required"))?;
            let raw_config = toml::from_str::<toml::Value>(raw_config)
                .with_context(|| format!("invalid {PARAM_CONFIG_TOML}"))?;
            let fee_bps_raw = strategy
                .parameters
                .get(PARAM_FEE_BPS)
                .with_context(|| format!("strategy parameter {PARAM_FEE_BPS} is required"))?;
            let fee_bps = Decimal::from_str(fee_bps_raw)
                .with_context(|| format!("invalid {PARAM_FEE_BPS} {fee_bps_raw:?}"))?;
            ensure!(
                fee_bps >= Decimal::ZERO,
                "strategy parameter {PARAM_FEE_BPS} must be non-negative"
            );
            let order_execution_mode_raw = strategy
                .parameters
                .get(STRATEGY_PARAM_ORDER_EXECUTION_MODE)
                .with_context(|| {
                    format!("strategy parameter {STRATEGY_PARAM_ORDER_EXECUTION_MODE} is required")
                })?;
            let order_execution_mode: BoltV3OrderExecutionMode = toml::Value::String(
                order_execution_mode_raw.clone(),
            )
            .try_into()
            .with_context(|| {
                format!(
                    "invalid {STRATEGY_PARAM_ORDER_EXECUTION_MODE} {order_execution_mode_raw:?}"
                )
            })?;
            let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> =
                Arc::new(BacktestDecisionEvidenceWriter);
            let submit_admission =
                Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence.clone()));
            let fee_provider: Arc<dyn FeeProvider> = Arc::new(ManifestFeeProvider { fee_bps });
            let build_context = StrategyBuildContext::new(
                fee_provider,
                decision_evidence,
                submit_admission,
                BoltV3OrderExecutionPolicy::from_mode(order_execution_mode),
                Venue::from(manifest.venue.nt_venue.as_str()),
            );
            let strategy =
                BinaryOracleEdgeTakerBuilder::build_strategy(&raw_config, &build_context)
                    .context("build binary_oracle_edge_taker strategy")?;
            let result = engine.add_strategy(strategy);
            result.context("add binary_oracle_edge_taker strategy")
        }
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE => {
            let catalog_input = manifest.primary_catalog_input().map_err(|error| {
                anyhow::anyhow!("strategy instrument requires catalog input: {error}")
            })?;
            let instrument_id: InstrumentId =
                catalog_input.nt_instrument_id.parse().with_context(|| {
                    format!("invalid instrument id {:?}", catalog_input.nt_instrument_id)
                })?;
            let trade_size_raw = strategy
                .parameters
                .get(PARAM_TRADE_SIZE)
                .with_context(|| format!("strategy parameter {PARAM_TRADE_SIZE} is required"))?;
            let trade_size = Quantity::from_str(trade_size_raw).map_err(|error| {
                anyhow::anyhow!("invalid {PARAM_TRADE_SIZE} {trade_size_raw:?}: {error}")
            })?;
            let entry_after_trades = strategy
                .parameters
                .get(PARAM_ENTRY_AFTER_TRADES)
                .with_context(|| {
                    format!("strategy parameter {PARAM_ENTRY_AFTER_TRADES} is required")
                })?
                .parse::<u64>()
                .with_context(|| format!("invalid {PARAM_ENTRY_AFTER_TRADES}"))?;
            let exit_after_trades = strategy
                .parameters
                .get(PARAM_EXIT_AFTER_TRADES)
                .with_context(|| {
                    format!("strategy parameter {PARAM_EXIT_AFTER_TRADES} is required")
                })?
                .parse::<u64>()
                .with_context(|| format!("invalid {PARAM_EXIT_AFTER_TRADES}"))?;
            let side_raw = strategy
                .parameters
                .get(PARAM_SIDE)
                .with_context(|| format!("strategy parameter {PARAM_SIDE} is required"))?;
            let side = match side_raw.as_str() {
                "buy" => OrderSide::Buy,
                "sell" => OrderSide::Sell,
                other => bail!("invalid {PARAM_SIDE} {other:?}"),
            };
            let config = MechanicalTradeReplayProbeConfig::new(
                instrument_id,
                trade_size,
                entry_after_trades,
                exit_after_trades,
                side,
            );
            engine
                .add_strategy(MechanicalTradeReplayProbe::new(config))
                .context("add MechanicalTradeReplayProbe strategy")
        }
        other => bail!("strategy {other:?} is not a registered compiled Rust strategy"),
    }
}

/// Terminal state of one order the engine produced, captured from the post-run
/// cache. Owned (not borrowed from the cache) so it survives the node lifetime,
/// and carries each order event's `Debug` rendering so a non-`Filled` terminal
/// state self-documents its denial/rejection/cancel reason in the failure output.
#[derive(Debug, Clone)]
pub struct OrderTerminalRecord {
    pub client_order_id: String,
    pub order_side: String,
    pub order_type: String,
    pub status: OrderStatus,
    pub quantity: String,
    pub filled_qty: String,
    pub events_debug: Vec<String>,
}

/// Result of one `BacktestNode` run: the NautilusTrader summary plus the
/// terminal state of every order in the post-run cache.
pub struct NtBacktestNodeRun {
    pub result: BacktestResult,
    pub order_terminals: Vec<OrderTerminalRecord>,
}

pub(crate) fn run_nt_backtest_node(manifest: &BacktestingRunManifest) -> Result<NtBacktestNodeRun> {
    let run_config = manifest
        .to_nt_run_config()
        .map_err(|error| anyhow::anyhow!("manifest to NautilusTrader config failed: {error}"))?;
    let domain_statistics = resolve_domain_statistics(&manifest.domain_metrics)?;
    let mut domain_analyzer = PortfolioAnalyzer::new();
    register_domain_statistics(&mut domain_analyzer, &domain_statistics);
    let mut node = BacktestNode::new(vec![run_config]).context("construct BacktestNode")?;
    node.build().context("build BacktestNode")?;
    {
        let engine = node
            .get_engine_mut(&manifest.run_id)
            .with_context(|| format!("no engine for run id {}", manifest.run_id))?;
        add_manifest_strategy(engine, manifest)?;
    }
    let mut results = node.run().context("run BacktestNode")?;
    ensure!(
        results.len() == 1,
        "expected exactly one backtest result, got {}",
        results.len()
    );
    // The run config sets `dispose_on_completion(false)`, so the engine still
    // holds its post-run cache here; capture each order's terminal state before
    // the node is dropped. A run that disposed (NautilusTrader default) would
    // leave this empty and the order-terminal proof would have nothing to check.
    let mut nt_result = results.remove(0);
    let order_terminals = {
        let engine = node
            .get_engine(&manifest.run_id)
            .with_context(|| format!("no engine for run id {} after run", manifest.run_id))?;
        let positions: Vec<_> = {
            let cache = engine.kernel().cache.borrow();
            cache
                .positions(None, None, None, None, None)
                .into_iter()
                .map(|position| position.cloned())
                .collect()
        };
        domain_analyzer.add_positions(&positions);
        for (name, value) in domain_statistics_from_analyzer(&domain_analyzer, &domain_statistics) {
            nt_result.stats_general.insert(name, value);
        }
        capture_order_terminals(engine)
    };
    Ok(NtBacktestNodeRun {
        result: nt_result,
        order_terminals,
    })
}

/// Capture the terminal state of every order in the engine's post-run cache.
fn capture_order_terminals(engine: &BacktestEngine) -> Vec<OrderTerminalRecord> {
    let cache = engine.kernel().cache.borrow();
    cache
        .orders(None, None, None, None, None)
        .into_iter()
        .map(|order| OrderTerminalRecord {
            client_order_id: order.client_order_id().to_string(),
            order_side: order.order_side().to_string(),
            order_type: order.order_type().to_string(),
            status: order.status(),
            quantity: order.quantity().to_string(),
            filled_qty: order.filled_qty().to_string(),
            events_debug: order
                .events()
                .iter()
                .map(|event| format!("{event:?}"))
                .collect(),
        })
        .collect()
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
    ensure!(
        !inputs.converter.version.trim().is_empty(),
        "converter_version must not be empty"
    );
    ensure!(
        !inputs.contract_manifest_hash.trim().is_empty(),
        "contract_manifest_hash must not be empty"
    );

    // Gate 4 preflight: reject unsupported NT/config surfaces before producing
    // derived canonical or catalog artifacts.
    inputs
        .manifest
        .validate(inputs.accepted)
        .map_err(|error| anyhow::anyhow!("manifest validation failed: {error}"))?;
    let catalog_root_str = inputs
        .catalog_root
        .to_str()
        .context("catalog root is not valid UTF-8")?;
    let manifest_catalog_input = inputs.manifest.single_catalog_input().map_err(|error| {
        anyhow::anyhow!("trade replay runner requires one catalog input: {error}")
    })?;
    ensure!(
        manifest_catalog_input.catalog_path == catalog_root_str,
        "manifest catalog_path {:?} does not match projection root {catalog_root_str:?}",
        manifest_catalog_input.catalog_path
    );

    // Gate 2: canonical normalization + canonical artifact.
    let canonical_table = normalize_registered_trade_converter(
        inputs.converter,
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

    // Gate 3: NautilusTrader catalog projection + read-back proof.
    let projection = project_canonical_trades_to_catalog(
        &canonical_table,
        inputs.instrument_spec,
        inputs.catalog_root,
    )
    .context("catalog projection failed")?;
    // Bind the manifest's catalog instrument id to the projected/read-back id.
    // The engine queries the catalog under the configured catalog input instrument id.
    // (gate 4 -> BacktestDataConfig); if that diverged from the id the read-back
    // proof verified, NautilusTrader would query a different (or empty) directory
    // and the run would silently execute over zero accepted trades.
    ensure!(
        manifest_catalog_input.nt_instrument_id == projection.nt_instrument_id,
        "manifest catalog_inputs.nt_instrument_id {:?} does not match projected instrument {:?}",
        manifest_catalog_input.nt_instrument_id,
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
    assert_read_back_matches(
        &read_back,
        &canonical_table.rows,
        &projection.nt_instrument_id,
    )?;
    // An optional manifest time window must overlap the accepted data's event
    // range, or the engine would filter out every accepted trade and still
    // "succeed" over zero data while stamping the accepted source/catalog hash.
    assert_time_window_overlaps_data(inputs.manifest, &canonical_table)?;

    // Gate 5: BacktestNode execution.
    let NtBacktestNodeRun {
        result: nt_result,
        order_terminals,
    } = run_nt_backtest_node(inputs.manifest)?;
    // The read-back proof above loads the catalog through one NautilusTrader code
    // path; the engine consumed it through another. Bind the two by asserting the
    // engine's own iteration count equals the number of accepted trades inside the
    // manifest's `[start_time, end_time]` window: NautilusTrader increments
    // `iterations` exactly once per data point delivered to the engine loop and
    // does not count data trimmed outside that window, so the expectation is the
    // windowed accepted-trade count (the whole accepted set when no window is set).
    // A run that silently processed zero (or a divergent count of) the in-window
    // accepted trades — while still stamping the accepted source/catalog hash — is
    // rejected here rather than producing a contract over data the engine never saw.
    let expected = expected_iterations(
        &canonical_table.rows,
        inputs.manifest.start_time,
        inputs.manifest.end_time,
    )
    .context("compute expected engine iterations")?;
    if let Some(reason) = iterations_mismatch(nt_result.iterations, expected) {
        bail!("backtest did not consume the accepted data: {reason}");
    }

    let conversion_fingerprint = ConversionFingerprint {
        source_proof_id: inputs.accepted.source_proof_id.clone(),
        source_proof_version: inputs.accepted.source_proof_version,
        accepted_object_sha256: inputs.accepted.accepted_object_sha256.clone(),
        converter_identity: inputs.converter.identity.clone(),
        converter_version: inputs.converter.version.clone(),
        converter_config_hash: inputs
            .converter
            .content_hash()
            .context("hash converter config")?,
    };
    let conversion_checkpoint = ConversionCheckpoint::completed(
        conversion_fingerprint.clone(),
        canonical_table.rows.len(),
        projection.catalog_hash.clone(),
        inputs.created_at,
    );
    let conversion_checkpoint_hash = conversion_checkpoint
        .content_hash()
        .context("hash conversion checkpoint")?;
    let conversion_manifest = ConversionManifest::completed(
        conversion_fingerprint,
        canonical_table.schema_version.clone(),
        projection.data_type.clone(),
        projection.nt_instrument_id.clone(),
        canonical_table.rows.len(),
        inputs.artifact_uris.nt_catalog_uri.clone(),
        projection.catalog_hash.clone(),
        conversion_checkpoint_hash.clone(),
        inputs.created_at,
    );
    let conversion_manifest_hash = conversion_manifest
        .content_hash()
        .context("hash conversion manifest")?;
    // execution_catalog_uri / direct_s3_catalog_access_proven keep the
    // deterministic defaults from `from_manifest` (the portable
    // output_catalog_uri; direct access = false): catalog-metadata.json is a
    // byte-deterministic artifact, so the transient local projection path must
    // never be stamped into it. The portable execution URI is recorded later by
    // the published-catalog proof path when (and only when) direct access is
    // actually proven.
    let conversion_catalog_metadata = ConversionCatalogMetadata::from_manifest(
        &conversion_manifest,
        conversion_manifest_hash.clone(),
        conversion_checkpoint_hash.clone(),
    );
    let conversion_catalog_metadata_hash = conversion_catalog_metadata
        .content_hash()
        .context("hash catalog metadata")?;

    // Gate 6: objective result contract.
    let warnings = result_contract_warnings(&nt_result);
    let mut claim_limits = inputs.accepted.result_contract_claim_limits();
    claim_limits.extend(nt_extension_surface_claim_limits(inputs.manifest)?);
    let (event_count_ledger_hash, selected_asset_ids_hash) =
        selector_provenance_hashes(canonical_table.fidelity_class, inputs.selector_provenance)?;
    let contract = build_result_contract(ResultContractInputs {
        run_id: &inputs.manifest.run_id,
        source_proof_id: &inputs.accepted.source_proof_id,
        source_proof_version: inputs.accepted.source_proof_version,
        manifest_hash: inputs.contract_manifest_hash,
        acceptance_mode: inputs.accepted.acceptance_mode,
        accepted_by: &inputs.accepted.accepted_by,
        accepted_at: &inputs.accepted.accepted_at,
        accepted_object_sha256: &inputs.accepted.accepted_object_sha256,
        converter_identity: &conversion_manifest.fingerprint.converter_identity,
        converter_version: &conversion_manifest.fingerprint.converter_version,
        converter_config_hash: &conversion_manifest.fingerprint.converter_config_hash,
        conversion_manifest_hash: &conversion_manifest_hash,
        conversion_checkpoint_hash: &conversion_checkpoint_hash,
        catalog_hash: &projection.catalog_hash,
        catalog_metadata_hash: &conversion_catalog_metadata_hash,
        event_count_ledger_hash,
        selected_asset_ids_hash,
        strategy: &inputs.manifest.strategy,
        execution_model: &inputs.manifest.execution_model,
        venue_queue_position: inputs.manifest.venue.queue_position,
        catalog_data_types: inputs
            .manifest
            .catalog_inputs
            .iter()
            .map(|input| input.data_type.clone())
            .collect(),
        run_purpose: run_purpose_label(inputs.manifest),
        market_structure_fixture: market_structure_label(inputs.manifest),
        fidelity_class: canonical_table.fidelity_class,
        claim_limits,
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
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash,
        conversion_manifest_hash,
        read_back_count: read_back.len(),
        nt_result,
        order_terminals,
        contract,
    })
}

pub(crate) fn run_purpose_label(manifest: &BacktestingRunManifest) -> &'static str {
    use super::run_manifest::RunPurpose;
    match manifest.run_purpose {
        RunPurpose::Normal => "normal",
        RunPurpose::Reproduction => "reproduction",
        RunPurpose::Audit => "audit",
        RunPurpose::Regression => "regression",
        RunPurpose::Migration => "migration",
    }
}

pub(crate) fn market_structure_label(manifest: &BacktestingRunManifest) -> &'static str {
    use super::run_manifest::MarketStructureFixture;
    match manifest.market_structure_fixture {
        MarketStructureFixture::BinaryOption => "binary-option",
        MarketStructureFixture::PerpsSpot => "perps-spot",
    }
}

/// Prove the catalog read-back is value-faithful, not just count-equal: every
/// read-back tick must carry the projected instrument id, and the set of trade
/// ids must equal the canonical table's, so a projection that silently dropped,
/// duplicated, or relabelled ticks cannot pass the gate.
pub(crate) fn assert_read_back_matches(
    read_back: &[TradeTick],
    canonical_rows: &[CanonicalTradeRow],
    expected_instrument_id: &str,
) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};
    let rows_by_id: BTreeMap<&str, &CanonicalTradeRow> = canonical_rows
        .iter()
        .map(|row| (row.trade_id.as_str(), row))
        .collect();
    ensure!(
        rows_by_id.len() == canonical_rows.len(),
        "canonical rows contain duplicate trade ids"
    );
    let expected_ids: BTreeSet<&str> = rows_by_id.keys().copied().collect();
    let mut actual_ids: BTreeSet<String> = BTreeSet::new();
    for tick in read_back {
        ensure!(
            tick.instrument_id.to_string() == expected_instrument_id,
            "catalog read-back tick instrument {} does not match projected {expected_instrument_id}",
            tick.instrument_id
        );
        let trade_id = tick.trade_id.to_string();
        let row = rows_by_id.get(trade_id.as_str()).with_context(|| {
            format!("catalog read-back trade id {trade_id} is absent from the canonical rows")
        })?;
        // Value faithfulness, not just identity: a projection that silently
        // corrupted a price, size, side, or timestamp must not pass the gate.
        let expected_price = Decimal::from_str(&row.price)
            .with_context(|| format!("canonical price {:?}", row.price))?;
        ensure!(
            tick.price.as_decimal() == expected_price,
            "catalog read-back price {} for trade {trade_id} does not match canonical {}",
            tick.price,
            row.price
        );
        let expected_size = Decimal::from_str(&row.size)
            .with_context(|| format!("canonical size {:?}", row.size))?;
        ensure!(
            tick.size.as_decimal() == expected_size,
            "catalog read-back size {} for trade {trade_id} does not match canonical {}",
            tick.size,
            row.size
        );
        ensure!(
            aggressor_label(tick.aggressor_side) == row.aggressor_side,
            "catalog read-back side {:?} for trade {trade_id} does not match canonical {}",
            tick.aggressor_side,
            row.aggressor_side
        );
        let label = format!("trade {}", row.trade_id);
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            tick.ts_event.as_u64() == expected_ts_event,
            "catalog read-back ts_event {} for trade {trade_id} does not match canonical {expected_ts_event}",
            tick.ts_event.as_u64()
        );
        // ts_init must equal the projection's availability-or-capture derivation,
        // not the event clock: NautilusTrader replays and windows by ts_init, so a
        // projection that stamped the wrong receipt clock must fail this gate. The
        // expectation is derived through the same shared owner the seam uses, so
        // the two cannot drift (NO DUAL PATHS).
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            tick.ts_init.as_u64() == expected_ts_init,
            "catalog read-back ts_init {} for trade {trade_id} does not match canonical {expected_ts_init}",
            tick.ts_init.as_u64()
        );
        actual_ids.insert(trade_id);
    }
    let actual_set: BTreeSet<&str> = actual_ids.iter().map(String::as_str).collect();
    ensure!(
        expected_ids == actual_set,
        "catalog read-back trade ids do not match the canonical rows"
    );
    Ok(())
}

/// Canonical aggressor-side label for a NautilusTrader [`AggressorSide`], so a
/// read-back tick's side can be compared to the canonical row's string.
fn aggressor_label(side: AggressorSide) -> &'static str {
    match side {
        AggressorSide::Buyer => TradeAggressorSide::Buyer.as_str(),
        AggressorSide::Seller => TradeAggressorSide::Seller.as_str(),
        AggressorSide::NoAggressor => "NO_AGGRESSOR",
    }
}

/// Prove a bar catalog read-back is value-faithful, mirroring
/// [`assert_read_back_matches`] for the bar family: every read-back bar must
/// carry the projected instrument id, the table's externally-aggregated bar
/// specification, and value-equal OHLCV and close-time fields, element-wise in
/// catalog order against the canonical rows (which `validate()` has already
/// proven time-monotonic).
pub(crate) fn assert_bar_read_back_matches(
    read_back: &[Bar],
    table: &super::canonical_market_data::CanonicalBarsTable,
    expected_instrument_id: &str,
) -> Result<()> {
    ensure!(
        read_back.len() == table.rows.len(),
        "bar catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    let expected_spec = BarSpecification::new_checked(
        table.bar_spec.step,
        table.bar_spec.aggregation,
        PriceType::Last,
    )
    .map_err(|error| anyhow::anyhow!("invalid canonical bar specification: {error}"))?;
    for (index, (bar, row)) in read_back.iter().zip(table.rows.iter()).enumerate() {
        ensure!(
            bar.bar_type.instrument_id().to_string() == expected_instrument_id,
            "bar read-back {index} instrument {} does not match projected {expected_instrument_id}",
            bar.bar_type.instrument_id()
        );
        ensure!(
            bar.bar_type.spec() == expected_spec,
            "bar read-back {index} spec {:?} does not match canonical {:?}",
            bar.bar_type.spec(),
            expected_spec
        );
        ensure!(
            bar.bar_type.aggregation_source() == AggregationSource::External,
            "bar read-back {index} must be externally aggregated"
        );
        for (label, actual, expected) in [
            ("open", bar.open.as_decimal(), &row.open),
            ("high", bar.high.as_decimal(), &row.high),
            ("low", bar.low.as_decimal(), &row.low),
            ("close", bar.close.as_decimal(), &row.close),
        ] {
            let expected = Decimal::from_str(expected)
                .with_context(|| format!("canonical {label} {expected:?}"))?;
            ensure!(
                actual == expected,
                "bar read-back {index} {label} {actual} does not match canonical {expected}"
            );
        }
        let expected_volume = Decimal::from_str(&row.volume)
            .with_context(|| format!("canonical volume {:?}", row.volume))?;
        ensure!(
            bar.volume.as_decimal() == expected_volume,
            "bar read-back {index} volume {} does not match canonical {expected_volume}",
            bar.volume
        );
        let label = format!("bar close_time {}", row.close_time);
        let expected_ts_event = ts_event_nanos(row.close_time, &label)?.as_u64();
        ensure!(
            bar.ts_event.as_u64() == expected_ts_event,
            "bar read-back {index} ts_event {} does not match canonical close_time {expected_ts_event}",
            bar.ts_event.as_u64()
        );
        // ts_init is the bar's availability-or-capture receipt clock (the clock
        // NautilusTrader replays by), derived through the shared projection owner.
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            bar.ts_init.as_u64() == expected_ts_init,
            "bar read-back {index} ts_init {} does not match canonical {expected_ts_init}",
            bar.ts_init.as_u64()
        );
    }
    Ok(())
}

/// Prove an order-book-delta catalog read-back is value-faithful, mirroring
/// [`assert_read_back_matches`] for the delta family: element-wise in catalog
/// order, every read-back delta must carry the projected instrument id and the
/// canonical action/side/price/size/order-id/flags/sequence/event-time values
/// (the canonical rows are dense-sequence validated, so positional comparison
/// plus sequence equality rejects drops, duplicates, and reorders).
pub(crate) fn assert_delta_read_back_matches(
    read_back: &[OrderBookDelta],
    table: &super::canonical_market_data::CanonicalOrderBookDeltasTable,
    expected_instrument_id: &str,
) -> Result<()> {
    use super::canonical_market_data::{DeltaAction, DeltaSide};
    ensure!(
        read_back.len() == table.rows.len(),
        "delta catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    for (index, (delta, row)) in read_back.iter().zip(table.rows.iter()).enumerate() {
        ensure!(
            delta.instrument_id.to_string() == expected_instrument_id,
            "delta read-back {index} instrument {} does not match projected {expected_instrument_id}",
            delta.instrument_id
        );
        ensure!(
            delta.sequence == row.sequence,
            "delta read-back {index} sequence {} does not match canonical {}",
            delta.sequence,
            row.sequence
        );
        ensure!(
            delta.flags == row.flags,
            "delta read-back {index} flags {} does not match canonical {}",
            delta.flags,
            row.flags
        );
        let label = format!("delta sequence {}", row.sequence);
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            delta.ts_event.as_u64() == expected_ts_event,
            "delta read-back {index} ts_event {} does not match canonical {expected_ts_event}",
            delta.ts_event.as_u64()
        );
        // CLEAR deltas carry ts_init too, so this gate covers them as well: the
        // expectation is the availability-or-capture receipt clock derived through
        // the shared projection owner (NautilusTrader replays by ts_init).
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            delta.ts_init.as_u64() == expected_ts_init,
            "delta read-back {index} ts_init {} does not match canonical {expected_ts_init}",
            delta.ts_init.as_u64()
        );
        let actual_action = match delta.action {
            BookAction::Clear => DeltaAction::Clear.as_str(),
            BookAction::Add => DeltaAction::Add.as_str(),
            BookAction::Update => DeltaAction::Update.as_str(),
            BookAction::Delete => DeltaAction::Delete.as_str(),
        };
        ensure!(
            actual_action == row.action,
            "delta read-back {index} action {actual_action} does not match canonical {}",
            row.action
        );
        if row.action == DeltaAction::Clear.as_str() {
            // CLEAR rows carry no side/price/size in the canonical vocabulary;
            // NautilusTrader represents them with a null book order.
            continue;
        }
        let actual_side = match delta.order.side {
            OrderSide::Buy => DeltaSide::Buy.as_str(),
            OrderSide::Sell => DeltaSide::Sell.as_str(),
            OrderSide::NoOrderSide => "NO_ORDER_SIDE",
        };
        ensure!(
            actual_side == row.side,
            "delta read-back {index} side {actual_side} does not match canonical {}",
            row.side
        );
        let expected_price = Decimal::from_str(&row.price)
            .with_context(|| format!("canonical price {:?}", row.price))?;
        ensure!(
            delta.order.price.as_decimal() == expected_price,
            "delta read-back {index} price {} does not match canonical {expected_price}",
            delta.order.price
        );
        let expected_size = Decimal::from_str(&row.size)
            .with_context(|| format!("canonical size {:?}", row.size))?;
        ensure!(
            delta.order.size.as_decimal() == expected_size,
            "delta read-back {index} size {} does not match canonical {expected_size}",
            delta.order.size
        );
        ensure!(
            delta.order.order_id == row.order_id,
            "delta read-back {index} order_id {} does not match canonical {}",
            delta.order.order_id,
            row.order_id
        );
    }
    Ok(())
}

/// Prove a top-of-book quote catalog read-back is value-faithful, mirroring
/// [`assert_read_back_matches`] for the quote family: element-wise in catalog
/// order, every read-back quote must carry the projected instrument id and the
/// canonical bid/ask/bid_size/ask_size decimals, with `ts_event` the event clock
/// and `ts_init` the availability-or-capture receipt clock derived through the
/// SAME shared projection owner the seam uses (NO DUAL PATHS) — this is the
/// load-bearing `ts_init == capture_time` proof for the quote family.
pub(crate) fn assert_quote_read_back_matches(
    read_back: &[QuoteTick],
    table: &super::canonical_market_data::CanonicalQuotesTable,
    expected_instrument_id: &str,
) -> Result<()> {
    ensure!(
        read_back.len() == table.rows.len(),
        "quote catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    for (index, (quote, row)) in read_back.iter().zip(table.rows.iter()).enumerate() {
        ensure!(
            quote.instrument_id.to_string() == expected_instrument_id,
            "quote read-back {index} instrument {} does not match projected {expected_instrument_id}",
            quote.instrument_id
        );
        for (label, actual, expected) in [
            ("bid", quote.bid_price.as_decimal(), &row.bid),
            ("ask", quote.ask_price.as_decimal(), &row.ask),
            ("bid_size", quote.bid_size.as_decimal(), &row.bid_size),
            ("ask_size", quote.ask_size.as_decimal(), &row.ask_size),
        ] {
            let expected = Decimal::from_str(expected)
                .with_context(|| format!("canonical {label} {expected:?}"))?;
            ensure!(
                actual == expected,
                "quote read-back {index} {label} {actual} does not match canonical {expected}"
            );
        }
        let label = format!("quote {}", row.event_time);
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            quote.ts_event.as_u64() == expected_ts_event,
            "quote read-back {index} ts_event {} does not match canonical {expected_ts_event}",
            quote.ts_event.as_u64()
        );
        // ts_init is the availability-or-capture receipt clock (the clock
        // NautilusTrader replays by), derived through the shared projection owner.
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            quote.ts_init.as_u64() == expected_ts_init,
            "quote read-back {index} ts_init {} does not match canonical {expected_ts_init}",
            quote.ts_init.as_u64()
        );
    }
    Ok(())
}

/// Prove an index-price catalog read-back is value-faithful, mirroring
/// [`assert_read_back_matches`] for the index family: element-wise in stable
/// read-back order, every read-back update must carry the projected instrument
/// id and the canonical `value` decimal, with `ts_event` the event clock and
/// `ts_init` the availability-or-capture receipt clock derived through the SAME
/// shared projection owner the seam uses (NO DUAL PATHS) — this is the
/// load-bearing `ts_init == capture_time` proof for the index family.
pub(crate) fn assert_index_read_back_matches(
    read_back: &[IndexPriceUpdate],
    table: &super::canonical_market_data::CanonicalIndexPricesTable,
    expected_instrument_id: &str,
) -> Result<()> {
    ensure!(
        read_back.len() == table.rows.len(),
        "index catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    for (index, (update, row)) in read_back.iter().zip(table.rows.iter()).enumerate() {
        ensure!(
            update.instrument_id.to_string() == expected_instrument_id,
            "index read-back {index} instrument {} does not match projected {expected_instrument_id}",
            update.instrument_id
        );
        let expected_value = Decimal::from_str(&row.value)
            .with_context(|| format!("canonical value {:?}", row.value))?;
        ensure!(
            update.value.as_decimal() == expected_value,
            "index read-back {index} value {} does not match canonical {expected_value}",
            update.value.as_decimal()
        );
        let label = format!("index price {}", row.event_time);
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            update.ts_event.as_u64() == expected_ts_event,
            "index read-back {index} ts_event {} does not match canonical {expected_ts_event}",
            update.ts_event.as_u64()
        );
        // ts_init is the availability-or-capture receipt clock (the clock
        // NautilusTrader replays by), derived through the shared projection owner.
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            update.ts_init.as_u64() == expected_ts_init,
            "index read-back {index} ts_init {} does not match canonical {expected_ts_init}",
            update.ts_init.as_u64()
        );
    }
    Ok(())
}

/// Prove a mark-price catalog read-back is value-faithful, mirroring
/// [`assert_read_back_matches`] for the mark family: element-wise in stable
/// read-back order, every read-back update must carry the projected instrument
/// id and the canonical `value` decimal, with `ts_event` the event clock and
/// `ts_init` the availability-or-capture receipt clock derived through the SAME
/// shared projection owner the seam uses (NO DUAL PATHS) — this is the
/// load-bearing `ts_init == capture_time` proof for the mark family.
pub(crate) fn assert_mark_read_back_matches(
    read_back: &[MarkPriceUpdate],
    table: &super::canonical_market_data::CanonicalMarkPricesTable,
    expected_instrument_id: &str,
) -> Result<()> {
    ensure!(
        read_back.len() == table.rows.len(),
        "mark catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    for (index, (update, row)) in read_back.iter().zip(table.rows.iter()).enumerate() {
        ensure!(
            update.instrument_id.to_string() == expected_instrument_id,
            "mark read-back {index} instrument {} does not match projected {expected_instrument_id}",
            update.instrument_id
        );
        let expected_value = Decimal::from_str(&row.value)
            .with_context(|| format!("canonical value {:?}", row.value))?;
        ensure!(
            update.value.as_decimal() == expected_value,
            "mark read-back {index} value {} does not match canonical {expected_value}",
            update.value.as_decimal()
        );
        let label = format!("mark price {}", row.event_time);
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            update.ts_event.as_u64() == expected_ts_event,
            "mark read-back {index} ts_event {} does not match canonical {expected_ts_event}",
            update.ts_event.as_u64()
        );
        // ts_init is the availability-or-capture receipt clock (the clock
        // NautilusTrader replays by), derived through the shared projection owner.
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            update.ts_init.as_u64() == expected_ts_init,
            "mark read-back {index} ts_init {} does not match canonical {expected_ts_init}",
            update.ts_init.as_u64()
        );
    }
    Ok(())
}

/// Prove a funding-rate catalog read-back is value-faithful, mirroring
/// [`assert_index_read_back_matches`] for the funding family: element-wise in
/// stable read-back order, every read-back update must carry the projected
/// instrument id, rate, interval, next funding timestamp, event clock, and
/// availability-or-capture receipt clock derived through the shared projection
/// owner.
pub(crate) fn assert_funding_read_back_matches(
    read_back: &[FundingRateUpdate],
    table: &super::canonical_market_data::CanonicalFundingRatesTable,
    expected_instrument_id: &str,
) -> Result<()> {
    ensure!(
        read_back.len() == table.rows.len(),
        "funding catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    let expected_id = InstrumentId::from_str(expected_instrument_id)
        .with_context(|| format!("invalid expected_instrument_id {expected_instrument_id:?}"))?;
    for (index, (update, row)) in read_back.iter().zip(table.rows.iter()).enumerate() {
        ensure!(
            update.instrument_id == expected_id,
            "funding read-back {index} instrument {} does not match projected {expected_instrument_id}",
            update.instrument_id
        );
        let expected_rate = Decimal::from_str(&row.rate)
            .with_context(|| format!("canonical rate {:?}", row.rate))?;
        ensure!(
            update.rate == expected_rate,
            "funding read-back {index} rate {} does not match canonical {expected_rate}",
            update.rate
        );
        ensure!(
            update.interval == row.interval_minutes,
            "funding read-back {index} interval {:?} does not match canonical {:?}",
            update.interval,
            row.interval_minutes
        );
        let expected_next = row
            .next_funding_time
            .map(u64::try_from)
            .transpose()
            .with_context(|| format!("canonical next_funding_time {:?}", row.next_funding_time))?;
        ensure!(
            update.next_funding_ns.map(|value| value.as_u64()) == expected_next,
            "funding read-back {index} next_funding_ns {:?} does not match canonical {:?}",
            update.next_funding_ns.map(|value| value.as_u64()),
            expected_next
        );
        let label = format!("funding rate {}", row.event_time);
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            update.ts_event.as_u64() == expected_ts_event,
            "funding read-back {index} ts_event {} does not match canonical {expected_ts_event}",
            update.ts_event.as_u64()
        );
        let expected_ts_init =
            ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
        ensure!(
            update.ts_init.as_u64() == expected_ts_init,
            "funding read-back {index} ts_init {} does not match canonical {expected_ts_init}",
            update.ts_init.as_u64()
        );
    }
    Ok(())
}

/// Reason the NautilusTrader engine did not process exactly the accepted data, or
/// `None` when its iteration count equals the accepted-trade count. NautilusTrader
/// increments `iterations` once per data point delivered to the engine loop.
pub(crate) fn iterations_mismatch(iterations: usize, expected: usize) -> Option<String> {
    if iterations == 0 {
        return Some(format!(
            "NautilusTrader engine iterated zero times; it processed none of the {expected} \
             accepted trades"
        ));
    }
    if iterations != expected {
        return Some(format!(
            "NautilusTrader engine processed {iterations} data points, expected {expected} \
             accepted trades"
        ));
    }
    None
}

/// Number of accepted trades the NautilusTrader engine will deliver under the
/// manifest's optional `[start, end]` window. NautilusTrader includes a data
/// point when `ts_init >= start_ns` (the skip-before-start loop breaks at the
/// first such point) and `ts_init <= end_ns` (the run loop breaks only once
/// `ts_init > end_ns`), so both bounds are inclusive. NautilusTrader windows by
/// `ts_init`, not the event clock, so the expectation is derived from each row's
/// availability-or-capture `ts_init` through the shared projection owner (the
/// same derivation the catalog seam used), never the canonical `event_time`.
/// With no bounds it is the whole accepted set, matching the read-back proof.
///
/// # Errors
///
/// Returns an error if a row's `ts_init` source clock is missing/non-positive,
/// or if a window bound is negative (mirroring the manifest's own
/// `manifest_time_to_nanos` rejection), so a malformed clock can never silently
/// admit or drop data from the engine's expected iteration count.
pub(crate) fn expected_iterations(
    rows: &[CanonicalTradeRow],
    start: Option<i64>,
    end: Option<i64>,
) -> Result<usize> {
    let start = window_bound_nanos("start_time", start)?;
    let end = window_bound_nanos("end_time", end)?;
    let mut count = 0usize;
    for row in rows {
        let ts_init = ts_init_nanos(
            row.availability_time,
            row.capture_time,
            &format!("trade {}", row.trade_id),
        )?
        .as_u64();
        if start.is_none_or(|start| ts_init >= start) && end.is_none_or(|end| ts_init <= end) {
            count += 1;
        }
    }
    Ok(count)
}

/// Convert an optional manifest window bound to nanos in the same domain the
/// engine compares `ts_init` against, rejecting a negative bound exactly as the
/// manifest's `manifest_time_to_nanos` does so the window math cannot diverge
/// from the configs NautilusTrader actually runs.
pub(crate) fn window_bound_nanos(field: &'static str, value: Option<i64>) -> Result<Option<u64>> {
    value
        .map(|value| {
            u64::try_from(value)
                .map_err(|_| anyhow::anyhow!("manifest {field} {value} is negative"))
        })
        .transpose()
}

/// Reject a manifest time window that excludes every accepted trade.
/// NautilusTrader admits a data point by `ts_init`, so the window must overlap
/// the accepted data's `ts_init` range — the min/max over the rows'
/// availability-or-capture clocks (the canonical rows are monotonic by
/// `event_time`, not `ts_init`, so the range is computed across all rows rather
/// than read off the first/last row). A `start_time` after the last available
/// trade (or an `end_time` before the first) would leave the engine with no data
/// while the run still reports the accepted source/catalog hash.
///
/// # Errors
///
/// Returns an error if a row's `ts_init` source clock is missing/non-positive,
/// if a window bound is negative, or if the window excludes all accepted data.
pub(crate) fn assert_time_window_overlaps_data(
    manifest: &BacktestingRunManifest,
    canonical_table: &CanonicalTradesTable,
) -> Result<()> {
    let mut range: Option<(u64, u64)> = None;
    for row in &canonical_table.rows {
        let ts_init = ts_init_nanos(
            row.availability_time,
            row.capture_time,
            &format!("trade {}", row.trade_id),
        )?
        .as_u64();
        range = Some(match range {
            Some((min, max)) => (min.min(ts_init), max.max(ts_init)),
            None => (ts_init, ts_init),
        });
    }
    let Some((first, last)) = range else {
        return Ok(());
    };
    let start = window_bound_nanos("start_time", manifest.start_time)?;
    let end = window_bound_nanos("end_time", manifest.end_time)?;
    match time_window_excludes_all_data(start, end, first, last) {
        None => Ok(()),
        Some("start_time") => bail!(
            "manifest start_time {:?} excludes all accepted data after ts_init {last}",
            manifest.start_time
        ),
        Some(_) => bail!(
            "manifest end_time {:?} excludes all accepted data before ts_init {first}",
            manifest.end_time
        ),
    }
}

/// Pure overlap test for a manifest `[start, end]` window against the accepted
/// data's `[first, last]` `ts_init` range (all in engine nanos). Returns the
/// name of the bound that excludes all data, or `None` when the window admits at
/// least one trade.
pub(crate) fn time_window_excludes_all_data(
    start: Option<u64>,
    end: Option<u64>,
    first: u64,
    last: u64,
) -> Option<&'static str> {
    if let Some(start) = start
        && start > last
    {
        return Some("start_time");
    }
    if let Some(end) = end
        && end < first
    {
        return Some("end_time");
    }
    None
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use nautilus_core::UnixNanos;
    use nautilus_model::{
        data::TradeTick,
        enums::AggressorSide,
        identifiers::{InstrumentId, TradeId},
        types::{Price, Quantity},
    };

    use super::{
        BacktestSelectorProvenance, assert_read_back_matches, expected_iterations,
        iterations_mismatch, selector_provenance_hashes, time_window_excludes_all_data,
    };
    use crate::canonical_trades::{CanonicalTradeRow, TradeAggressorSide};
    use crate::source_proof::SourceProofFidelityClass;

    const TEST_INSTRUMENT: &str = "BTCUSDT.BYBIT";

    fn canonical_row(
        trade_id: &str,
        price: &str,
        size: &str,
        side: TradeAggressorSide,
        event_time: i64,
    ) -> CanonicalTradeRow {
        CanonicalTradeRow {
            schema_version: String::new(),
            ingest_run_id: String::new(),
            source_binding: String::new(),
            venue: String::new(),
            product_family: String::new(),
            product_category: String::new(),
            instrument_id: String::new(),
            canonical_instrument_key: String::new(),
            venue_symbol: String::new(),
            nt_instrument_id: None,
            event_time,
            // The directly-built `tick()` fixtures stamp ts_init == ts_event ==
            // event_time, so the canonical row's receipt clock must derive the
            // same ts_init: availability absent, capture_time == event_time.
            capture_time: event_time,
            availability_time: None,
            source_sequence: None,
            raw_payload_id: String::new(),
            source_proof_id: String::new(),
            payload_hash: String::new(),
            transform_hash: String::new(),
            trade_source_type: String::new(),
            trade_id: trade_id.to_string(),
            aggressor_side: side.as_str().to_string(),
            price: price.to_string(),
            size: size.to_string(),
            notional: String::new(),
        }
    }

    fn tick(trade_id: &str, price: &str, size: &str, side: AggressorSide, ts: u64) -> TradeTick {
        TradeTick::new(
            InstrumentId::from_str(TEST_INSTRUMENT).expect("instrument id"),
            Price::from_str(price).expect("price"),
            Quantity::from_str(size).expect("size"),
            side,
            TradeId::from(trade_id),
            UnixNanos::from(ts),
            UnixNanos::from(ts),
        )
    }

    #[test]
    fn read_back_faithful_values_are_admitted() {
        let rows = vec![canonical_row(
            "t1",
            "100.5",
            "2.0",
            TradeAggressorSide::Buyer,
            1000,
        )];
        let ticks = vec![tick("t1", "100.5", "2.0", AggressorSide::Buyer, 1000)];
        assert_read_back_matches(&ticks, &rows, TEST_INSTRUMENT)
            .expect("faithful read-back must be admitted");
    }

    #[test]
    fn read_back_price_mismatch_is_rejected() {
        let rows = vec![canonical_row(
            "t1",
            "100.5",
            "2.0",
            TradeAggressorSide::Buyer,
            1000,
        )];
        let ticks = vec![tick("t1", "999.5", "2.0", AggressorSide::Buyer, 1000)];
        let err = assert_read_back_matches(&ticks, &rows, TEST_INSTRUMENT).unwrap_err();
        assert!(err.to_string().contains("price"), "{err}");
    }

    #[test]
    fn read_back_size_mismatch_is_rejected() {
        let rows = vec![canonical_row(
            "t1",
            "100.5",
            "2.0",
            TradeAggressorSide::Buyer,
            1000,
        )];
        let ticks = vec![tick("t1", "100.5", "9.0", AggressorSide::Buyer, 1000)];
        let err = assert_read_back_matches(&ticks, &rows, TEST_INSTRUMENT).unwrap_err();
        assert!(err.to_string().contains("size"), "{err}");
    }

    #[test]
    fn read_back_side_mismatch_is_rejected() {
        let rows = vec![canonical_row(
            "t1",
            "100.5",
            "2.0",
            TradeAggressorSide::Buyer,
            1000,
        )];
        let ticks = vec![tick("t1", "100.5", "2.0", AggressorSide::Seller, 1000)];
        let err = assert_read_back_matches(&ticks, &rows, TEST_INSTRUMENT).unwrap_err();
        assert!(err.to_string().contains("side"), "{err}");
    }

    #[test]
    fn read_back_timestamp_mismatch_is_rejected() {
        let rows = vec![canonical_row(
            "t1",
            "100.5",
            "2.0",
            TradeAggressorSide::Buyer,
            1000,
        )];
        let ticks = vec![tick("t1", "100.5", "2.0", AggressorSide::Buyer, 2000)];
        let err = assert_read_back_matches(&ticks, &rows, TEST_INSTRUMENT).unwrap_err();
        assert!(err.to_string().contains("ts_event"), "{err}");
    }

    #[test]
    fn iterations_zero_is_rejected() {
        assert!(iterations_mismatch(0, 3).is_some());
    }

    #[test]
    fn iterations_below_expected_is_rejected() {
        assert!(iterations_mismatch(2, 3).is_some());
    }

    #[test]
    fn iterations_matching_expected_is_admitted() {
        assert!(iterations_mismatch(3, 3).is_none());
    }

    #[test]
    fn l2_run_contract_provenance_requires_selector_hashes() {
        let err = selector_provenance_hashes(SourceProofFidelityClass::L2Replay, None).unwrap_err();
        assert!(
            err.to_string().contains("selector provenance"),
            "unexpected error: {err}"
        );

        let hashes = selector_provenance_hashes(
            SourceProofFidelityClass::L2Replay,
            Some(BacktestSelectorProvenance {
                event_count_ledger_hash:
                    "7777777777777777777777777777777777777777777777777777777777777777",
                selected_asset_ids_hash:
                    "8888888888888888888888888888888888888888888888888888888888888888",
            }),
        )
        .expect("selector provenance");
        assert_eq!(
            hashes,
            (
                Some("7777777777777777777777777777777777777777777777777777777777777777"),
                Some("8888888888888888888888888888888888888888888888888888888888888888")
            )
        );

        assert_eq!(
            selector_provenance_hashes(SourceProofFidelityClass::TradeReplay, None)
                .expect("trade replay selector provenance"),
            (None, None)
        );
    }

    fn windowed_rows() -> Vec<CanonicalTradeRow> {
        vec![
            canonical_row("t1", "1", "1", TradeAggressorSide::Buyer, 100),
            canonical_row("t2", "1", "1", TradeAggressorSide::Buyer, 200),
            canonical_row("t3", "1", "1", TradeAggressorSide::Buyer, 300),
        ]
    }

    #[test]
    fn expected_iterations_counts_all_rows_without_window() {
        // capture_time == event_time for these fixtures, so the ts_init the engine
        // windows by equals the event clock and the counts below are unchanged by S1.
        assert_eq!(
            expected_iterations(&windowed_rows(), None, None).expect("expected iterations"),
            3
        );
    }

    #[test]
    fn expected_iterations_excludes_trades_before_start() {
        // start is inclusive: the trades at ts_init 200 and 300 remain.
        assert_eq!(
            expected_iterations(&windowed_rows(), Some(200), None).expect("expected iterations"),
            2
        );
    }

    #[test]
    fn expected_iterations_excludes_trades_after_end() {
        // end is inclusive: only the trade at ts_init 100 remains.
        assert_eq!(
            expected_iterations(&windowed_rows(), None, Some(100)).expect("expected iterations"),
            1
        );
    }

    #[test]
    fn expected_iterations_counts_inclusive_boundary_and_interior_windows() {
        // Both bounds inclusive and exactly on the data's edges -> all three.
        assert_eq!(
            expected_iterations(&windowed_rows(), Some(100), Some(300))
                .expect("expected iterations"),
            3
        );
        // A window strictly inside the edges keeps only the middle trade.
        assert_eq!(
            expected_iterations(&windowed_rows(), Some(150), Some(250))
                .expect("expected iterations"),
            1
        );
    }

    #[test]
    fn expected_iterations_windows_by_ts_init_not_event_time() {
        // A row whose receipt clock (capture_time) differs from its event clock is
        // windowed by ts_init: event_time 100 but capture_time 500 falls OUTSIDE a
        // [100, 100] window (event-time math would wrongly include it) and INSIDE a
        // [500, 500] window.
        let mut rows = windowed_rows();
        rows[0].capture_time = 500;
        // Window on the event clock value -> excluded (ts_init 500 > 100).
        assert_eq!(
            expected_iterations(&rows, Some(100), Some(100)).expect("expected iterations"),
            0
        );
        // Window on the receipt clock value -> included.
        assert_eq!(
            expected_iterations(&rows, Some(500), Some(500)).expect("expected iterations"),
            1
        );
    }

    #[test]
    fn expected_iterations_fails_loud_on_invalid_ts_init_source() {
        let mut rows = windowed_rows();
        rows[0].capture_time = 0;
        rows[0].availability_time = None;
        let err = expected_iterations(&rows, None, None).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

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
        // A window covering exactly the data's ts_init range admits all of it.
        assert_eq!(
            time_window_excludes_all_data(Some(100), Some(200), 100, 200),
            None
        );
        // A start bound exactly at the last trade still admits that boundary trade.
        // (An inverted start>end window is rejected upstream by the manifest's
        // own validation, so it never reaches this pure overlap test.)
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
}
