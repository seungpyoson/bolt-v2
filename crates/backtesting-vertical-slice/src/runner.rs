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

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use bolt_v2::{
    ReferencePriceUpdate, ReferenceQuoteProvenance,
    bolt_v3_config::{
        BacktestConfigOverrideReport, LoadedStrategy, apply_backtest_config_override,
        load_bolt_v3_config,
    },
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome,
        BoltV3BasketAdmissionDecisionEvidence, BoltV3CapitalAdmissionRebuildAuditEvidence,
        BoltV3DecisionEvidenceWriter, BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence,
        BoltV3ExitEvaluationEvidence, BoltV3LossGovernorHaltEvidence, BoltV3OrderIntentEvidence,
        BoltV3OrderRejectEvidence, BoltV3RequoteThrottleEvidence,
        BoltV3SettlementBookingErrorEvidence, BoltV3SettlementEvidence,
        BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitReservationFillEvidence,
        BoltV3SubmitReservationMetadataEvidence,
    },
    bolt_v3_operator_artifacts::json_artifact_bytes,
    bolt_v3_order_execution::{BoltV3OrderExecutionMode, BoltV3OrderExecutionPolicy},
    bolt_v3_providers::FeeProvider,
    bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime,
    bolt_v3_strategy_context::StrategyBuildContext,
    bolt_v3_strategy_registration::{
        StrategyPreparationConfig, prepare_strategy_client_routes, register_prepared_strategy_batch,
    },
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    strategies::binary_oracle_edge_taker::archetype::raw_taker_config,
    strategies::production_strategy_registry,
};
use futures_util::future::BoxFuture;
use nautilus_analysis::analyzer::PortfolioAnalyzer;
use nautilus_backtest::{engine::BacktestEngine, node::BacktestNode, result::BacktestResult};
use nautilus_core::UnixNanos;
#[cfg(test)]
use nautilus_model::orderbook::OrderBook;
use nautilus_model::{
    data::{
        Bar, BarSpecification, Data, FundingRateUpdate, IndexPriceUpdate, InstrumentClose,
        MarkPriceUpdate, OrderBookDelta, QuoteTick, TradeTick,
    },
    enums::{
        AggregationSource, AggressorSide, BookAction, InstrumentCloseType, OrderSide, OrderStatus,
        PriceType,
    },
    events::OrderEventAny,
    identifiers::{ClientId, InstrumentId, Venue},
    orders::Order,
    position::Position,
    types::{AccountBalance, Price, Quantity},
};
use nautilus_trading::examples::strategies::{HurstVpinDirectional, HurstVpinDirectionalConfig};
use rust_decimal::Decimal;
use serde::Serialize;

use crate::hashing::sha256_hex;

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
    path_resolution::resolve_existing_input_path,
    result_contract::{
        BacktestFeedLabel, BacktestResultContract, BacktestRunGuardReport, ResultArtifactUris,
        ResultContractInputs, build_result_contract,
    },
    run_manifest::{
        BacktestingRunManifest, NtSurfaceClassification, STRATEGY_BINARY_ORACLE_EDGE_TAKER,
        STRATEGY_BINARY_ORACLE_MAKER, STRATEGY_HURST_VPIN_DIRECTIONAL,
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE, STRATEGY_PARAM_ORDER_EXECUTION_MODE,
        StrategySource,
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

#[derive(Debug, Default)]
struct BacktestDecisionEvidenceState {
    strategy_input_snapshot_count: u64,
    order_intent_count: u64,
    admission_decision_count: u64,
    admitted_order_count: u64,
    submit_reservation_count: u64,
    submit_fill_count: u64,
    entry_skip_count: u64,
    exit_decision_count: u64,
    loss_governor_halt_count: u64,
    requote_throttle_count: u64,
    latest_strategy_input_snapshot: Option<BoltV3StrategyInputEvidenceSnapshot>,
}

#[derive(Debug, Default)]
struct BacktestDecisionEvidenceWriter {
    state: Mutex<BacktestDecisionEvidenceState>,
}

impl BacktestDecisionEvidenceWriter {
    fn run_guard_report(&self, result: &BacktestResult) -> Result<BacktestRunGuardReport> {
        let state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("backtest decision evidence state mutex poisoned"))?;
        let latest = state.latest_strategy_input_snapshot.as_ref();
        let signal_quote_received =
            latest.is_some_and(|snapshot| positive_decimal_text(&snapshot.spot_price));
        let realized_volatility_ready = latest.is_some_and(|snapshot| {
            positive_decimal_text(&snapshot.realized_volatility)
                && snapshot.realized_volatility_as_of_ms.is_some()
                && snapshot.realized_volatility_blockers.is_empty()
        });
        let price_to_beat_received =
            latest.is_some_and(|snapshot| positive_decimal_text(&snapshot.price_to_beat_value));
        let reference_fresh = latest.is_some_and(|snapshot| {
            snapshot
                .reference_current_price
                .as_deref()
                .is_some_and(positive_decimal_text)
        });
        let armed = signal_quote_received
            && realized_volatility_ready
            && price_to_beat_received
            && reference_fresh;
        let traded = result.total_orders > 0 && result.total_positions > 0;
        let did_not_arm_reason = did_not_arm_reason(DidNotArmReasonInputs {
            latest,
            signal_quote_received,
            realized_volatility_ready,
            price_to_beat_received,
            reference_fresh,
            order_intent_count: state.order_intent_count,
            admitted_order_count: state.admitted_order_count,
            total_orders: result.total_orders as u64,
            total_positions: result.total_positions as u64,
        });

        Ok(BacktestRunGuardReport {
            strategy_input_snapshot_count: state.strategy_input_snapshot_count,
            order_intent_count: state.order_intent_count,
            admission_decision_count: state.admission_decision_count,
            admitted_order_count: state.admitted_order_count,
            submit_reservation_count: state.submit_reservation_count,
            submit_fill_count: state.submit_fill_count,
            entry_skip_count: state.entry_skip_count,
            exit_decision_count: state.exit_decision_count,
            loss_governor_halt_count: state.loss_governor_halt_count,
            requote_throttle_count: state.requote_throttle_count,
            signal_quote_received,
            realized_volatility_ready,
            price_to_beat_received,
            reference_fresh,
            armed,
            traded,
            latest_market_id: latest.and_then(|snapshot| snapshot.market_id.clone()),
            latest_spot_price: latest.map(|snapshot| snapshot.spot_price.clone()),
            latest_reference_current_price: latest
                .and_then(|snapshot| snapshot.reference_current_price.clone()),
            latest_reference_current_price_source_id: latest
                .and_then(|snapshot| snapshot.reference_current_price_source_id.clone()),
            latest_price_to_beat_value: latest.map(|snapshot| snapshot.price_to_beat_value.clone()),
            latest_realized_volatility_as_of_ms: latest
                .and_then(|snapshot| snapshot.realized_volatility_as_of_ms),
            latest_realized_volatility_sources_used: latest.map_or_else(Vec::new, |snapshot| {
                snapshot.realized_volatility_sources_used.clone()
            }),
            latest_realized_volatility_blockers: latest.map_or_else(Vec::new, |snapshot| {
                snapshot.realized_volatility_blockers.clone()
            }),
            did_not_arm_reason,
        })
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut BacktestDecisionEvidenceState) -> R) -> Result<R> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("backtest decision evidence state mutex poisoned"))?;
        Ok(f(&mut state))
    }
}

fn positive_decimal_text(value: &str) -> bool {
    value
        .trim()
        .parse::<Decimal>()
        .is_ok_and(|value| value > Decimal::ZERO)
}

struct DidNotArmReasonInputs<'a> {
    latest: Option<&'a BoltV3StrategyInputEvidenceSnapshot>,
    signal_quote_received: bool,
    realized_volatility_ready: bool,
    price_to_beat_received: bool,
    reference_fresh: bool,
    order_intent_count: u64,
    admitted_order_count: u64,
    total_orders: u64,
    total_positions: u64,
}

fn did_not_arm_reason(inputs: DidNotArmReasonInputs<'_>) -> Option<String> {
    let DidNotArmReasonInputs {
        latest,
        signal_quote_received,
        realized_volatility_ready,
        price_to_beat_received,
        reference_fresh,
        order_intent_count,
        admitted_order_count,
        total_orders,
        total_positions,
    } = inputs;
    if total_orders > 0 && total_positions > 0 {
        return None;
    }
    let Some(snapshot) = latest else {
        return Some("did NOT arm — feed strategy input evidence missing/stale".to_string());
    };
    if !signal_quote_received {
        return Some("did NOT arm — feed signal quote missing/stale".to_string());
    }
    if !realized_volatility_ready {
        let blockers = snapshot.realized_volatility_blockers.join(",");
        return Some(if blockers.is_empty() {
            "did NOT arm — feed realized volatility missing/stale".to_string()
        } else {
            format!("did NOT arm — feed realized volatility missing/stale ({blockers})")
        });
    }
    if !price_to_beat_received {
        return Some("did NOT arm — feed strike reconstruction missing/stale".to_string());
    }
    if !reference_fresh {
        return Some("did NOT arm — feed reference reconstruction missing/stale".to_string());
    }
    if order_intent_count == 0 {
        return Some("did NOT arm — no order intent emitted after feeds were ready".to_string());
    }
    if admitted_order_count == 0 {
        return Some("did NOT arm — submit admission rejected order intents".to_string());
    }
    if total_orders == 0 {
        return Some("did NOT arm — Nautilus total_orders stayed zero after admission".to_string());
    }
    if total_positions == 0 {
        return Some("did NOT arm — feed tradable book/fill path missing/stale".to_string());
    }
    None
}

impl BoltV3DecisionEvidenceWriter for BacktestDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        self.with_state(|state| {
            state.strategy_input_snapshot_count += 1;
            state.latest_strategy_input_snapshot = Some(snapshot.clone());
        })?;
        Ok(())
    }

    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        self.with_state(|state| {
            state.order_intent_count += 1;
        })?;
        Ok(())
    }

    fn record_admission_decision(&self, decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        self.with_state(|state| {
            state.admission_decision_count += 1;
            if decision.outcome == BoltV3AdmissionOutcome::Admitted {
                state.admitted_order_count += 1;
            }
        })?;
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        self.with_state(|state| {
            state.submit_reservation_count += 1;
        })?;
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        self.with_state(|state| {
            state.submit_fill_count += 1;
        })?;
        Ok(())
    }

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> Result<()> {
        self.with_state(|state| {
            state.entry_skip_count += 1;
        })?;
        Ok(())
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        self.with_state(|state| {
            state.exit_decision_count += 1;
        })?;
        Ok(())
    }

    fn record_loss_governor_halt(&self, _halt: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        self.with_state(|state| {
            state.loss_governor_halt_count += 1;
        })?;
        Ok(())
    }

    fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        self.with_state(|state| {
            state.requote_throttle_count += 1;
        })?;
        Ok(())
    }

    fn record_exit_evaluation(&self, _evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
        Ok(())
    }

    fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
        Ok(())
    }

    fn record_settlement(&self, _evidence: &BoltV3SettlementEvidence) -> Result<()> {
        Ok(())
    }

    fn record_settlement_booking_error(
        &self,
        _evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn drain_shutdown(&self) -> Result<()> {
        // Deliberate no-op: the BVS run guard writer keeps in-memory counters only.
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

pub(crate) fn result_contract_warnings(
    nt_result: &BacktestResult,
    fidelity_class: SourceProofFidelityClass,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if nt_result.total_orders == 0 {
        // Two honest, mutually exclusive reasons for a zero-order run, keyed off
        // the source fidelity. Trade-only sources carry no quote ticks, so a
        // quote-driven strategy structurally cannot enter — expected fidelity,
        // not a defect. Any quote-bearing source had ticks available, so zero
        // orders means the strategy never armed; point the operator at the run
        // guard report instead. One source of truth, shared by every run path.
        let message = if fidelity_class == SourceProofFidelityClass::TradeReplay {
            "No orders were placed: the accepted data is trade-only and carries no quote ticks, \
             and the configured strategy's order entry is quote-driven. NautilusTrader still \
             aggregated the accepted trades into bars and ran the strategy's signal logic. This \
             reflects the TRADE_REPLAY fidelity of the source, not a defect."
        } else {
            "No orders were placed. Treat P/L as non-armed unless the run_guard_report shows \
             armed=true and traded=true; inspect run_guard_report.did_not_arm_reason for the \
             missing or stale feed."
        };
        warnings.push(message.to_string());
    }
    warnings
}

pub(crate) fn result_contract_feed_labels(
    manifest: &BacktestingRunManifest,
) -> Vec<BacktestFeedLabel> {
    let mut labels = manifest
        .catalog_inputs
        .iter()
        .enumerate()
        .map(|(index, input)| BacktestFeedLabel {
            feed_id: input
                .client_id
                .clone()
                .unwrap_or_else(|| format!("catalog_input_{index}")),
            source_class: "real".to_string(),
            data_type: input.data_type.clone(),
            instrument_id: input.nt_instrument_id.clone(),
            label: format!(
                "real catalog feed: data_type={} instrument={}",
                input.data_type, input.nt_instrument_id
            ),
        })
        .collect::<Vec<_>>();
    if manifest.strategy.registry_key == STRATEGY_BINARY_ORACLE_EDGE_TAKER {
        labels.push(BacktestFeedLabel {
            feed_id: "binary_oracle_price_to_beat".to_string(),
            source_class: "reconstructed".to_string(),
            data_type: "ChainlinkStrikeReference".to_string(),
            instrument_id: "price_to_beat".to_string(),
            label: "Chainlink strike/reference reconstruction, not raw".to_string(),
        });
        labels.push(BacktestFeedLabel {
            feed_id: "binary_oracle_reference_current_price".to_string(),
            source_class: "reconstructed".to_string(),
            data_type: "ChainlinkReferencePrice".to_string(),
            instrument_id: "reference_current_price".to_string(),
            label: "Chainlink reference-current-price reconstruction, not raw".to_string(),
        });
    }
    labels
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

#[derive(Default)]
struct AddedManifestStrategy {
    config_override_report: Option<BacktestConfigOverrideReport>,
    run_guard_writer: Option<Arc<BacktestDecisionEvidenceWriter>>,
    resolved_config_hash: Option<String>,
    resolved_config_bytes: Option<Vec<u8>>,
}

#[derive(Serialize)]
struct ResolvedTakerConfigIdentity<'a> {
    schema_version: &'static str,
    production_config_bundle_checksum: Option<&'a str>,
    applied_override: Option<&'a bolt_v2::bolt_v3_config::BacktestConfigOverride>,
    raw_strategy_config: &'a toml::Value,
}

fn canonical_resolved_taker_config_bytes(
    raw_strategy_config: &toml::Value,
    production_config_bundle_checksum: Option<&str>,
    applied_override: Option<&bolt_v2::bolt_v3_config::BacktestConfigOverride>,
) -> Result<Vec<u8>> {
    json_artifact_bytes(&ResolvedTakerConfigIdentity {
        schema_version: "backtest-resolved-taker-config.v1",
        production_config_bundle_checksum,
        applied_override,
        raw_strategy_config,
    })
    .map_err(|error| anyhow::anyhow!(error))
    .context("serialize canonical production-resolved taker configuration")
}

fn register_backtest_data_clients(
    engine: &mut BacktestEngine,
    client_ids: impl IntoIterator<Item = ClientId>,
) {
    for client_id in client_ids {
        engine.add_data_client_if_not_exists(client_id);
    }
}

fn manifest_backtest_data_client_ids(manifest: &BacktestingRunManifest) -> BTreeSet<ClientId> {
    let mut client_ids = BTreeSet::new();
    for input in &manifest.catalog_inputs {
        if let Some(client_id) = input.client_id.as_deref() {
            client_ids.insert(ClientId::from(client_id));
        }
    }
    for input in &manifest.reconstructed_reference_current_price {
        client_ids.insert(ClientId::from(input.client_id.as_str()));
    }
    client_ids
}

fn effective_taker_subscription_data_client_ids(
    strategy: &LoadedStrategy,
    realized_volatility_runtime: &RealizedVolSurfaceRuntime,
) -> BTreeSet<ClientId> {
    let mut client_ids = BTreeSet::new();
    for signal in strategy.config.signal_data.values() {
        client_ids.insert(ClientId::from(signal.data_client_id.as_str()));
    }
    if let Some(resolution_data) = &strategy.config.resolution_data {
        client_ids.insert(ClientId::from(resolution_data.data_client_id.as_str()));
    }
    if let Some(reference_current_price) = &strategy.config.reference_current_price {
        for source_id in &reference_current_price.source_order {
            let Some(source) = reference_current_price.sources.get(source_id) else {
                continue;
            };
            if source.enabled {
                client_ids.insert(ClientId::from(source.client_id.as_str()));
            }
        }
    }
    if let Some(surface_id) = strategy.config.realized_volatility_surface_id.as_deref() {
        for (_, client_id) in realized_volatility_runtime
            .quote_subscription_requests_for_surface(surface_id)
            .into_iter()
            .chain(realized_volatility_runtime.trade_subscription_requests_for_surface(surface_id))
            .chain(realized_volatility_runtime.index_subscription_requests_for_surface(surface_id))
        {
            if let Some(client_id) = client_id {
                client_ids.insert(client_id);
            }
        }
    }
    client_ids
}

fn manifest_binary_oracle_execution_controls(
    strategy: &StrategySource,
) -> Result<(Decimal, BoltV3OrderExecutionMode)> {
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
    let order_execution_mode: BoltV3OrderExecutionMode =
        toml::Value::String(order_execution_mode_raw.clone())
            .try_into()
            .with_context(|| {
                format!(
                    "invalid {STRATEGY_PARAM_ORDER_EXECUTION_MODE} {order_execution_mode_raw:?}"
                )
            })?;
    Ok((fee_bps, order_execution_mode))
}

fn inline_manifest_strategy_config(strategy: &StrategySource) -> Result<toml::Value> {
    let raw_config = strategy
        .parameters
        .get(PARAM_CONFIG_TOML)
        .with_context(|| format!("strategy parameter {PARAM_CONFIG_TOML} is required"))?;
    toml::from_str::<toml::Value>(raw_config)
        .with_context(|| format!("invalid {PARAM_CONFIG_TOML}"))
}

fn register_manifest_binary_oracle_strategy(
    engine: &mut BacktestEngine,
    manifest: &BacktestingRunManifest,
    registry_key: &str,
    raw_config: &toml::Value,
    fee_bps: Decimal,
    order_execution_mode: BoltV3OrderExecutionMode,
    realized_volatility_runtime: Option<Arc<Mutex<RealizedVolSurfaceRuntime>>>,
) -> Result<Arc<BacktestDecisionEvidenceWriter>> {
    let run_guard_writer = Arc::new(BacktestDecisionEvidenceWriter::default());
    let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> = run_guard_writer.clone();
    let submit_admission = Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence.clone()));
    let fee_provider: Arc<dyn FeeProvider> = Arc::new(ManifestFeeProvider { fee_bps });
    let mut build_context = StrategyBuildContext::new(
        fee_provider,
        decision_evidence,
        submit_admission,
        BoltV3OrderExecutionPolicy::from_mode(order_execution_mode),
        Venue::from(manifest.venue.nt_venue.as_str()),
    );
    if let Some(runtime) = realized_volatility_runtime {
        build_context = build_context.with_realized_volatility_runtime(runtime);
    }
    let registry = production_strategy_registry().context("build production strategy registry")?;
    let prepared = registry
        .prepare_strategy(registry_key, raw_config, &build_context)
        .with_context(|| format!("prepare {registry_key} strategy through production registry"))?;
    register_prepared_strategy_batch(engine.kernel().trader(), vec![prepared])
        .with_context(|| format!("register {registry_key} prepared strategy batch"))?;
    Ok(run_guard_writer)
}

/// Add the manifest-selected compiled Rust strategy to the engine.
///
/// Only registered compiled Rust strategies are admissible; the manifest is
/// already validated, this is defence in depth.
fn add_manifest_strategy(
    engine: &mut BacktestEngine,
    manifest: &BacktestingRunManifest,
) -> Result<AddedManifestStrategy> {
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
            let config = HurstVpinDirectionalConfig::builder()
                .instrument_id(instrument_id)
                .bar_type(bar_type)
                .trade_size(trade_size)
                .build();
            engine
                .add_strategy(HurstVpinDirectional::new(config))
                .context("add HurstVpinDirectional strategy")?;
            Ok(AddedManifestStrategy::default())
        }
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => {
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
            let (
                raw_config,
                config_override_report,
                realized_volatility_runtime,
                resolved_config_bytes,
            ) = if let Some(overlay) = &strategy.config_overlay {
                let production_root_config_path =
                    resolve_existing_input_path(Path::new(&overlay.production_root_config_path));
                let loaded =
                    load_bolt_v3_config(&production_root_config_path).with_context(|| {
                        format!(
                            "load production config root {}",
                            overlay.production_root_config_path
                        )
                    })?;
                let override_spec = overlay.to_bolt_v3_override();
                let (loaded, report) = apply_backtest_config_override(loaded, &override_spec)
                    .with_context(|| {
                        format!(
                            "apply backtest config overlay {}",
                            overlay.override_delta.label
                        )
                    })?;
                let loaded_strategy = loaded
                    .strategies
                    .iter()
                    .find(|loaded_strategy| {
                        loaded_strategy.config.strategy_instance_id
                            == overlay.override_delta.strategy_instance_id
                    })
                    .with_context(|| {
                        format!(
                            "overlay strategy_instance_id {} was not present after load",
                            overlay.override_delta.strategy_instance_id
                        )
                    })?;
                let preparation_config = StrategyPreparationConfig::from_root(&loaded.root);
                let client_routes = prepare_strategy_client_routes(&loaded, loaded_strategy)
                    .context("prepare configured strategy client routes")?;
                let raw_config =
                    raw_taker_config(loaded_strategy, &preparation_config, &client_routes)
                        .context("build raw taker config from overlaid production config")?;
                let runtime = RealizedVolSurfaceRuntime::from_loaded_config(&loaded)
                    .map_err(|error| anyhow::anyhow!("{error}"))
                    .context("build realized-volatility runtime from overlaid config")?;
                register_backtest_data_clients(
                    engine,
                    effective_taker_subscription_data_client_ids(loaded_strategy, &runtime),
                );
                let runtime = Arc::new(Mutex::new(runtime));
                let resolved_config_bytes = canonical_resolved_taker_config_bytes(
                    &raw_config,
                    Some(&loaded.config_bundle_checksum),
                    Some(&override_spec),
                )?;
                (
                    raw_config,
                    Some(report),
                    Some(runtime),
                    resolved_config_bytes,
                )
            } else {
                let raw_config = strategy
                    .parameters
                    .get(PARAM_CONFIG_TOML)
                    .with_context(|| {
                        format!("strategy parameter {PARAM_CONFIG_TOML} is required")
                    })?;
                let raw_config = toml::from_str::<toml::Value>(raw_config)
                    .with_context(|| format!("invalid {PARAM_CONFIG_TOML}"))?;
                let resolved_config_bytes =
                    canonical_resolved_taker_config_bytes(&raw_config, None, None)?;
                (raw_config, None, None, resolved_config_bytes)
            };
            let run_guard_writer = Arc::new(BacktestDecisionEvidenceWriter::default());
            let resolved_config_hash = sha256_hex(&resolved_config_bytes);
            let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> = run_guard_writer.clone();
            let submit_admission =
                Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence.clone()));
            let fee_provider: Arc<dyn FeeProvider> = Arc::new(ManifestFeeProvider { fee_bps });
            let mut build_context = StrategyBuildContext::new(
                fee_provider,
                decision_evidence,
                submit_admission,
                BoltV3OrderExecutionPolicy::from_mode(order_execution_mode),
                Venue::from(manifest.venue.nt_venue.as_str()),
            );
            if let Some(runtime) = realized_volatility_runtime {
                build_context = build_context.with_realized_volatility_runtime(runtime);
            }
            let registry =
                production_strategy_registry().context("build production strategy registry")?;
            let prepared = registry
                .prepare_strategy(
                    STRATEGY_BINARY_ORACLE_EDGE_TAKER,
                    &raw_config,
                    &build_context,
                )
                .context("prepare binary_oracle_edge_taker strategy through production registry")?;
            register_prepared_strategy_batch(engine.kernel().trader(), vec![prepared])
                .context("register binary_oracle_edge_taker prepared strategy batch")?;
            Ok(AddedManifestStrategy {
                config_override_report,
                run_guard_writer: Some(run_guard_writer),
                resolved_config_hash: Some(resolved_config_hash),
                resolved_config_bytes: Some(resolved_config_bytes),
            })
        }
        STRATEGY_BINARY_ORACLE_MAKER => {
            ensure!(
                strategy.config_overlay.is_none(),
                "strategy.config_overlay is not supported for strategy {STRATEGY_BINARY_ORACLE_MAKER:?}"
            );
            let (fee_bps, order_execution_mode) =
                manifest_binary_oracle_execution_controls(strategy)?;
            let raw_config = inline_manifest_strategy_config(strategy)?;
            let run_guard_writer = register_manifest_binary_oracle_strategy(
                engine,
                manifest,
                STRATEGY_BINARY_ORACLE_MAKER,
                &raw_config,
                fee_bps,
                order_execution_mode,
                None,
            )?;
            Ok(AddedManifestStrategy {
                config_override_report: None,
                run_guard_writer: Some(run_guard_writer),
                resolved_config_hash: None,
                resolved_config_bytes: None,
            })
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
                .context("add MechanicalTradeReplayProbe strategy")?;
            Ok(AddedManifestStrategy::default())
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
    pub initialized_quantity: Quantity,
    pub initialized_quote_quantity: bool,
    pub effective_quantity: Quantity,
    pub submission_timestamp: Option<UnixNanos>,
    pub fills: Vec<nautilus_model::events::OrderFilled>,
    pub events_debug: Vec<String>,
}

/// Result of one `BacktestNode` run: the NautilusTrader summary plus the
/// terminal state of every order in the post-run cache.
pub struct NtBacktestNodeRun {
    pub result: BacktestResult,
    pub order_terminals: Vec<OrderTerminalRecord>,
    pub config_override_report: Option<BacktestConfigOverrideReport>,
    pub run_guard_report: Option<BacktestRunGuardReport>,
    pub positions: Vec<Position>,
    pub account_balances: Vec<AccountBalance>,
    pub resolved_config_hash: Option<String>,
    pub resolved_config_bytes: Option<Vec<u8>>,
    pub execution_contract_report: Option<crate::execution_contract::ExecutionContractReport>,
}

fn reconstructed_reference_current_price_data(
    manifest: &BacktestingRunManifest,
) -> Result<BTreeMap<String, Vec<Data>>> {
    let mut by_client_id = BTreeMap::<String, Vec<Data>>::new();
    for (index, row) in manifest
        .reconstructed_reference_current_price
        .iter()
        .enumerate()
    {
        let price = parse_reconstructed_price_float(index, "price", &row.price)?;
        let bid = row
            .bid
            .as_deref()
            .map(|value| parse_reconstructed_price_float(index, "bid", value))
            .transpose()?;
        let ask = row
            .ask
            .as_deref()
            .map(|value| parse_reconstructed_price_float(index, "ask", value))
            .transpose()?;
        let provenance = ReferenceQuoteProvenance::try_from_fields(row.provenance.clone())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| {
                format!("invalid reconstructed_reference_current_price[{index}] provenance")
            })?;
        let update = ReferencePriceUpdate::try_new_with_provenance(
            row.asset.as_str(),
            row.source_id.as_str(),
            row.provider.as_str(),
            row.provider_instrument.as_str(),
            price,
            bid,
            ask,
            row.observed_ts_ms,
            row.received_ts_ms,
            provenance,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))
        .with_context(|| {
            format!("invalid reconstructed_reference_current_price[{index}] update")
        })?;
        by_client_id
            .entry(row.client_id.clone())
            .or_default()
            .push(Data::Custom(update.to_custom_data()));
    }
    Ok(by_client_id)
}

fn parse_reconstructed_price_float(index: usize, field: &str, value: &str) -> Result<f64> {
    value.trim().parse::<f64>().with_context(|| {
        format!("parse reconstructed_reference_current_price[{index}].{field} {value:?}")
    })
}

/// Builds NT `InstrumentClose` settlement events from the manifest's instrument
/// settlement inputs. When replayed, each `ContractExpired` close redeems a held
/// position to its resolved value (binary: winner `1.0`, loser `0.0`) and books a
/// realized P/L. The `close_price` is the real market resolution carried on the
/// manifest — it is not synthesized here.
fn instrument_settlement_data(manifest: &BacktestingRunManifest) -> Result<Vec<Data>> {
    let mut closes = Vec::with_capacity(manifest.instrument_settlements.len());
    for (index, row) in manifest.instrument_settlements.iter().enumerate() {
        let instrument_id = InstrumentId::from_str(row.nt_instrument_id.as_str())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| {
                format!(
                    "parse instrument_settlements[{index}].nt_instrument_id {:?}",
                    row.nt_instrument_id
                )
            })?;
        // The settlement close must route to a declared backtest venue, else NT
        // sends it to no exchange and the held position silently never redeems.
        let settlement_venue = instrument_id.venue.to_string();
        let venue_config = std::iter::once(&manifest.venue)
            .chain(manifest.additional_venues.iter())
            .find(|venue| venue.nt_venue == settlement_venue)
            .with_context(|| {
                format!(
                    "instrument_settlements[{index}] {} settles on venue {settlement_venue}, which is not a declared backtest venue",
                    row.nt_instrument_id
                )
            })?;
        // The holding venue MUST be funded in the settlement currency. NT's
        // multi-currency portfolio silently drops a realized PnL booked in a
        // currency the account was never funded in (the loss vanishes from
        // stats_pnls), so this binding is the difference between a real P/L and a
        // green run that lost the number. settlement_currency is a required
        // manifest field, so this check is unconditional — it can never be
        // skipped by omitting it.
        ensure_settlement_currency_funded(
            index,
            &row.nt_instrument_id,
            &settlement_venue,
            &row.settlement_currency,
            &venue_config.starting_balances,
        )?;
        let close_value = Decimal::from_str(row.close_price.trim()).with_context(|| {
            format!(
                "parse instrument_settlements[{index}].close_price {:?}",
                row.close_price
            )
        })?;
        // Binary options redeem at a payoff in [0,1]; a value outside that range is
        // a malformed resolution that would book a nonsensical multiple-of-stake P/L
        // while still passing as the "real market resolution".
        ensure!(
            (Decimal::ZERO..=Decimal::ONE).contains(&close_value),
            "instrument_settlements[{index}] {} close_price {close_value} is outside the binary [0,1] redemption range",
            row.nt_instrument_id
        );
        let close_price = Price::from_str(row.close_price.trim())
            .map_err(|error| anyhow::anyhow!("{error}"))
            .with_context(|| {
                format!("invalid close_price for instrument_settlements[{index}]: {close_value}")
            })?;
        ensure!(
            close_price.precision == row.price_precision,
            "instrument_settlements[{index}] close_price precision {} does not match declared {}",
            close_price.precision,
            row.price_precision
        );
        closes.push(Data::InstrumentClose(InstrumentClose::new(
            instrument_id,
            close_price,
            InstrumentCloseType::ContractExpired,
            UnixNanos::from(row.ts_event_ns),
            UnixNanos::from(row.ts_init_ns),
        )));
    }
    Ok(closes)
}

/// The holding venue MUST be funded in the settlement currency, else NT's
/// multi-currency portfolio silently drops the realized PnL booked in a currency
/// the account was never funded in — turning a real loss into a green run that
/// lost the number. `settlement_currency` is a required manifest field, so the
/// settlement builder calls this for every injected settlement: the funded-venue
/// check is unconditional and cannot be skipped by omitting the field.
fn ensure_settlement_currency_funded(
    index: usize,
    nt_instrument_id: &str,
    settlement_venue: &str,
    settlement_currency: &str,
    starting_balances: &[String],
) -> Result<()> {
    let funded = starting_balances.iter().any(|balance| {
        balance
            .split_whitespace()
            .last()
            .is_some_and(|funded_currency| funded_currency == settlement_currency)
    });
    ensure!(
        funded,
        "instrument_settlements[{index}] {nt_instrument_id} settles in {settlement_currency} but venue {settlement_venue} is not funded in it; NautilusTrader would silently drop the realized PnL"
    );
    Ok(())
}

pub(crate) fn run_nt_backtest_node(manifest: &BacktestingRunManifest) -> Result<NtBacktestNodeRun> {
    let run_config = manifest
        .to_nt_run_config()
        .map_err(|error| anyhow::anyhow!("manifest to NautilusTrader config failed: {error}"))?;
    let reconstructed_reference_current_price_data =
        reconstructed_reference_current_price_data(manifest)?;
    let domain_statistics = resolve_domain_statistics(&manifest.domain_metrics)?;
    let mut domain_analyzer = PortfolioAnalyzer::new();
    register_domain_statistics(&mut domain_analyzer, &domain_statistics);
    let mut node = BacktestNode::new(vec![run_config]).context("construct BacktestNode")?;
    node.build().context("build BacktestNode")?;
    let added_strategy = {
        let engine = node
            .get_engine_mut(&manifest.run_id)
            .with_context(|| format!("no engine for run id {}", manifest.run_id))?;
        register_backtest_data_clients(engine, manifest_backtest_data_client_ids(manifest));
        add_manifest_strategy(engine, manifest)?
    };
    if !reconstructed_reference_current_price_data.is_empty() {
        let engine = node
            .get_engine_mut(&manifest.run_id)
            .with_context(|| format!("no engine for run id {}", manifest.run_id))?;
        for (client_id, data) in reconstructed_reference_current_price_data {
            engine
                .add_data(data, Some(ClientId::from(client_id.as_str())), false, false)
                .with_context(|| {
                    format!(
                        "add reconstructed reference-current-price custom data for client {client_id}"
                    )
                })?;
        }
    }
    let instrument_settlement_data = instrument_settlement_data(manifest)?;
    if !instrument_settlement_data.is_empty() {
        let engine = node
            .get_engine_mut(&manifest.run_id)
            .with_context(|| format!("no engine for run id {}", manifest.run_id))?;
        engine
            .add_data(instrument_settlement_data, None, false, true)
            .context("add instrument settlement close events")?;
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
    let (order_terminals, positions, account_balances) = {
        let engine = node
            .get_engine(&manifest.run_id)
            .with_context(|| format!("no engine for run id {} after run", manifest.run_id))?;
        let (positions, account_balances): (Vec<_>, Vec<_>) = {
            let cache = engine.kernel().cache.borrow();
            let positions = cache
                .positions(None, None, None, None, None)
                .into_iter()
                .map(|position| position.cloned())
                .collect();
            let account_balances = std::iter::once(&manifest.venue)
                .chain(manifest.additional_venues.iter())
                .filter_map(|venue| cache.account_for_venue(&Venue::from(venue.nt_venue.as_str())))
                .flat_map(|account| account.balances().into_values())
                .collect();
            (positions, account_balances)
        };
        domain_analyzer.add_positions(&positions);
        for (name, value) in domain_statistics_from_analyzer(&domain_analyzer, &domain_statistics) {
            nt_result.stats_general.insert(name, value);
        }
        (
            capture_order_terminals(engine)?,
            positions,
            account_balances,
        )
    };
    let run_guard_report = added_strategy
        .run_guard_writer
        .as_ref()
        .map(|writer| writer.run_guard_report(&nt_result))
        .transpose()?;
    Ok(NtBacktestNodeRun {
        result: nt_result,
        order_terminals,
        config_override_report: added_strategy.config_override_report,
        run_guard_report,
        positions,
        account_balances,
        resolved_config_hash: added_strategy.resolved_config_hash,
        resolved_config_bytes: added_strategy.resolved_config_bytes,
        execution_contract_report: None,
    })
}

/// Capture the terminal state of every order in the engine's post-run cache.
fn capture_order_terminals(engine: &BacktestEngine) -> Result<Vec<OrderTerminalRecord>> {
    let cache = engine.kernel().cache.borrow();
    cache
        .orders(None, None, None, None, None)
        .into_iter()
        .map(|order| -> Result<OrderTerminalRecord> {
            let initialized = order
                .events()
                .iter()
                .find_map(|event| match event {
                    OrderEventAny::Initialized(initialized) => Some(initialized),
                    _ => None,
                })
                .context("cached order has no initialization event")?;
            Ok(OrderTerminalRecord {
                client_order_id: order.client_order_id().to_string(),
                order_side: order.order_side().to_string(),
                order_type: order.order_type().to_string(),
                status: order.status(),
                quantity: order.quantity().to_string(),
                filled_qty: order.filled_qty().to_string(),
                initialized_quantity: initialized.quantity,
                initialized_quote_quantity: initialized.quote_quantity,
                effective_quantity: order.quantity(),
                submission_timestamp: order.events().iter().find_map(|event| match event {
                    OrderEventAny::Submitted(submitted) => Some(submitted.ts_event),
                    _ => None,
                }),
                fills: order
                    .events()
                    .iter()
                    .filter_map(|event| match event {
                        OrderEventAny::Filled(fill) => Some(fill.clone()),
                        _ => None,
                    })
                    .collect(),
                events_debug: order
                    .events()
                    .iter()
                    .map(|event| format!("{event:?}"))
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
fn run_nt_backtest_node_with_execution_contract<F>(
    manifest: &BacktestingRunManifest,
    validator: F,
) -> Result<NtBacktestNodeRun>
where
    F: FnOnce(&NtBacktestNodeRun) -> Result<crate::execution_contract::ExecutionContractReport>,
{
    let mut output = run_nt_backtest_node(manifest)?;
    output.execution_contract_report = Some(validator(&output)?);
    Ok(output)
}

#[cfg(test)]
fn replay_executable_book_at_submission(
    instrument_id: InstrumentId,
    deltas: &[OrderBookDelta],
    submission_timestamp: UnixNanos,
) -> Result<OrderBook> {
    let mut book = OrderBook::new(instrument_id, nautilus_model::enums::BookType::L2_MBP);
    for delta in deltas
        .iter()
        .filter(|delta| delta.ts_init <= submission_timestamp)
    {
        book.apply_delta(delta)
            .map_err(|error| anyhow::anyhow!(error))
            .context("replay executable book with NautilusTrader")?;
    }
    Ok(book)
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
        config_override_report,
        run_guard_report,
        ..
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
    let warnings = result_contract_warnings(&nt_result, canonical_table.fidelity_class);
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
        config_override_report: config_override_report.as_ref(),
        run_guard_report: run_guard_report.as_ref(),
        feed_labels: result_contract_feed_labels(inputs.manifest),
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
/// [`assert_index_read_back_matches`] for the funding family: every read-back
/// update must carry the projected instrument id, rate, interval, next funding
/// timestamp, event clock, and availability-or-capture receipt clock derived
/// through the shared projection owner.
///
/// The comparison is order-INDEPENDENT: both sides are sorted here by the SAME
/// key the projection's `read_back_funding_rates` sorts the read-back by
/// (`(ts_event, rate, rate.scale(), interval, next_funding_ns, ts_init)`;
/// instrument id is constant for one instrument so it is omitted from the
/// discriminator but still checked per element) before the element-wise pass,
/// so correctness can never silently depend on the canonical table's stored
/// order matching the read-back's stable sort order.
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

    // Comparable sort key derived from BOTH sides, mirroring the projection's
    // `read_back_funding_rates` sort order so neither input's stored order can
    // influence the pairing. `instrument_id` (the projection's 2nd sort key) is
    // intentionally omitted: each read-back is single-instrument, so it is
    // constant on both sides and cannot affect ordering. If this assertion is
    // ever reused for a multi-instrument read-back, restore instrument_id here.
    type FundingSortKey = (u64, Decimal, u32, Option<u16>, Option<u64>, u64);

    // Read-back side: fields are already typed; the key is infallible.
    let mut sorted_read_back: Vec<(FundingSortKey, &FundingRateUpdate)> = read_back
        .iter()
        .map(|update| {
            let key: FundingSortKey = (
                update.ts_event.as_u64(),
                update.rate,
                update.rate.scale(),
                update.interval,
                update.next_funding_ns.map(|value| value.as_u64()),
                update.ts_init.as_u64(),
            );
            (key, update)
        })
        .collect();
    sorted_read_back.sort_by_key(|entry| entry.0);

    // Canonical side: key derivation is fallible (timestamp helpers, rate parse,
    // next-funding cast), so pre-compute keys in a fallible pass before sorting
    // on the infallible precomputed key.
    let mut sorted_rows: Vec<(
        FundingSortKey,
        &super::canonical_market_data::CanonicalFundingRateRow,
    )> = table
        .rows
        .iter()
        .map(|row| {
            let label = format!("funding rate {}", row.event_time);
            let ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
            let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?.as_u64();
            let rate = Decimal::from_str(&row.rate)
                .with_context(|| format!("canonical rate {:?}", row.rate))?;
            let next_funding_ns = row
                .next_funding_time
                .map(u64::try_from)
                .transpose()
                .with_context(|| {
                    format!("canonical next_funding_time {:?}", row.next_funding_time)
                })?;
            let key: FundingSortKey = (
                ts_event,
                rate,
                rate.scale(),
                row.interval_minutes,
                next_funding_ns,
                ts_init,
            );
            Ok((key, row))
        })
        .collect::<Result<Vec<_>>>()?;
    sorted_rows.sort_by_key(|entry| entry.0);

    for (index, ((_, update), (_, row))) in
        sorted_read_back.iter().zip(sorted_rows.iter()).enumerate()
    {
        ensure!(
            update.instrument_id == expected_id,
            "funding read-back {index} instrument {} does not match projected {expected_instrument_id}",
            update.instrument_id
        );
        let expected_rate = Decimal::from_str(&row.rate)
            .with_context(|| format!("canonical rate {:?}", row.rate))?;
        ensure!(
            update.rate == expected_rate && update.rate.scale() == expected_rate.scale(),
            "funding read-back {index} rate {} (scale {}) does not match canonical {expected_rate} (scale {})",
            update.rate,
            update.rate.scale(),
            expected_rate.scale()
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
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use anyhow::{Context, Result, ensure};
    use nautilus_core::{Params, UnixNanos};
    use nautilus_model::{
        data::{OrderBookDelta, TradeTick},
        enums::{AggressorSide, AssetClass, BookAction, OrderSide},
        identifiers::{InstrumentId, Symbol, TradeId},
        instruments::{BinaryOption, Instrument, InstrumentAny},
        types::{Currency, Money, Price, Quantity},
    };
    use nautilus_persistence::backend::catalog::ParquetDataCatalog;
    use nautilus_polymarket::http::models::GammaMarket;
    use rust_decimal::Decimal;
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::{
        BacktestDecisionEvidenceWriter, BacktestSelectorProvenance, BoltV3DecisionEvidenceWriter,
        StrategyPreparationConfig, apply_backtest_config_override, assert_read_back_matches,
        canonical_resolved_taker_config_bytes, ensure_settlement_currency_funded,
        expected_iterations, iterations_mismatch, load_bolt_v3_config,
        prepare_strategy_client_routes, raw_taker_config, replay_executable_book_at_submission,
        resolve_existing_input_path, run_nt_backtest_node,
        run_nt_backtest_node_with_execution_contract, selector_provenance_hashes,
        time_window_excludes_all_data,
    };
    use crate::canonical_market_data::{
        CanonicalIndexPriceRow, CanonicalIndexPricesTable, CanonicalQuotesTable,
        NORMALIZED_SCHEMA_VERSION,
    };
    use crate::canonical_trades::{
        CanonicalTradeRow, CsvTimestampUnit, TradeAggressorSide, TradesPartition,
    };
    use crate::catalog_projection::{
        SpotInstrumentSpec, project_canonical_index_to_catalog, project_canonical_quotes_to_catalog,
    };
    use crate::pmxt_one_off_backfill_projection::{
        PmxtBookLevel, PmxtOneOffProjectionRequest, PmxtOneOffSelectedRow, PmxtOneOffSnapshotRow,
        PmxtOneOffTickSide, PmxtOneOffTradeRow, PmxtPriceChangeRow,
        project_pmxt_one_off_rows_to_nt, write_pmxt_one_off_projection_to_catalog,
    };
    use crate::run_manifest::{
        BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION, BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE,
        ManifestArtifactStore, ManifestBacktestConfigOverride, ManifestCatalogInput,
        ManifestInstrumentSettlementInput, ManifestRealizedVolatilitySourceSelector,
        ManifestReferenceCurrentPriceInput, ManifestVenueConfig, MarketStructureFixture,
        RunPurpose, STRATEGY_BINARY_ORACLE_EDGE_TAKER, STRATEGY_BINARY_ORACLE_MAKER,
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE, STRATEGY_PARAM_FEE_BPS,
        STRATEGY_PARAM_ORDER_EXECUTION_MODE, StrategyConfigOverlaySource, StrategySource,
        StrategySourceKind,
    };
    use crate::seeded_l2_quotes::{
        SeededL2QuoteAction, SeededL2QuoteMappingConfig, SeededL2QuoteProvenance,
        normalize_seeded_l2_events, parse_seeded_l2_jsonl, seeded_l2_quote_transform_hash,
    };
    use crate::source_proof::{SourceProofFidelityClass, SourceProofUsageScope};

    const TEST_INSTRUMENT: &str = "BTCUSDT.BYBIT";
    const MAKER_SMOKE_VENUE: &str = "POLYMARKET";
    const MAKER_SMOKE_YES_INSTRUMENT: &str = "SAMPLE-EVENT-YES.POLYMARKET";
    const MAKER_SMOKE_NO_INSTRUMENT: &str = "SAMPLE-EVENT-NO.POLYMARKET";
    const MAKER_SMOKE_MARKET_SLUG: &str = "will-sample-event-resolve-yes";
    const MAKER_SMOKE_CONDITION_ID: &str = "condition-sample-event";
    const MAKER_SMOKE_QUESTION_ID: &str = "question-sample-event";
    const MAKER_SMOKE_CLIENT_ID: &str = "maker_execution_client";
    const MAKER_SMOKE_RUN_ID: &str = "binary-oracle-maker-backtest-smoke";
    const MAKER_SMOKE_TS_NS: u64 = 1_772_323_201_665_000_000;

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
    fn backtest_decision_evidence_writer_drain_shutdown_is_noop() -> Result<()> {
        let writer = BacktestDecisionEvidenceWriter::default();
        writer.drain_shutdown()
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
    fn settlement_currency_must_be_funded_at_holding_venue() -> Result<()> {
        // A settlement in the funded collateral currency passes: the realized PnL
        // has an account to book against.
        ensure_settlement_currency_funded(
            0,
            "BTC-up.POLYMARKET",
            "POLYMARKET",
            "pUSD",
            &["1000 pUSD".to_string()],
        )?;

        // Differential: settle in a currency the venue was NEVER funded in. NT
        // would silently drop the realized PnL (the loss vanishes from
        // stats_pnls); the guard must instead fail loud so the green run can't
        // hide the lost number.
        let err = ensure_settlement_currency_funded(
            1,
            "BTC-up.POLYMARKET",
            "POLYMARKET",
            "pUSD",
            &["1000 USDC".to_string()],
        )
        .expect_err("a settlement in an unfunded currency must fail loud, not drop the PnL");
        let message = err.to_string();
        ensure!(
            message.contains("is not funded in it"),
            "unexpected error message: {message}"
        );
        Ok(())
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

    fn maker_smoke_binary_option(instrument_id: &str, outcome: &str) -> InstrumentAny {
        let mut info = Params::new();
        for (key, value) in [
            ("market_slug", MAKER_SMOKE_MARKET_SLUG),
            ("market_id", "sample-event-yes-no"),
            ("condition_id", MAKER_SMOKE_CONDITION_ID),
            ("question_id", MAKER_SMOKE_QUESTION_ID),
        ] {
            info.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (MAKER_SMOKE_TS_NS - 1_000_000_000).into(),
            (MAKER_SMOKE_TS_NS + 60_000_000_000).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
            None,
            None,
            None,
            None,
            None,
            Some(Price::from("0.999")),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(info),
            1.into(),
            1.into(),
        ))
    }

    fn maker_smoke_trade(
        instrument_id: InstrumentId,
        trade_id: &str,
        side: AggressorSide,
        ts_ns: u64,
    ) -> TradeTick {
        let ts = UnixNanos::from(ts_ns);
        TradeTick::new(
            instrument_id,
            Price::from("0.500"),
            Quantity::from("1.00"),
            side,
            TradeId::from(trade_id),
            ts,
            ts,
        )
    }

    fn write_maker_smoke_catalog(catalog_root: &Path) -> Result<()> {
        let yes = maker_smoke_binary_option(MAKER_SMOKE_YES_INSTRUMENT, "Yes");
        let no = maker_smoke_binary_option(MAKER_SMOKE_NO_INSTRUMENT, "No");
        let yes_id = yes.id();
        let no_id = no.id();
        let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
        catalog
            .write_instruments(vec![yes, no])
            .context("write maker smoke instruments")?;
        catalog
            .write_to_parquet(
                &[maker_smoke_trade(
                    yes_id,
                    "maker-smoke-yes-1",
                    AggressorSide::Buyer,
                    MAKER_SMOKE_TS_NS,
                )],
                None,
                None,
                None,
            )
            .context("write maker smoke YES trade tick")?;
        catalog
            .write_to_parquet(
                &[maker_smoke_trade(
                    no_id,
                    "maker-smoke-no-1",
                    AggressorSide::Seller,
                    MAKER_SMOKE_TS_NS + 1_000_000,
                )],
                None,
                None,
                None,
            )
            .context("write maker smoke NO trade tick")?;
        Ok(())
    }

    fn write_execution_contract_smoke_catalog(catalog_root: &Path) -> Result<()> {
        let instrument = maker_smoke_binary_option(MAKER_SMOKE_YES_INSTRUMENT, "Yes");
        let instrument_id = instrument.id();
        let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
        catalog
            .write_instruments(vec![instrument])
            .context("write execution-contract smoke instrument")?;
        catalog
            .write_to_parquet(
                &[
                    maker_smoke_trade(
                        instrument_id,
                        "execution-contract-entry",
                        AggressorSide::Buyer,
                        MAKER_SMOKE_TS_NS,
                    ),
                    maker_smoke_trade(
                        instrument_id,
                        "execution-contract-exit",
                        AggressorSide::Seller,
                        MAKER_SMOKE_TS_NS + 1_000_000,
                    ),
                ],
                None,
                None,
                None,
            )
            .context("write execution-contract smoke trade ticks")?;
        Ok(())
    }

    fn maker_smoke_config_toml() -> String {
        r#"
        strategy_id = "binary_oracle_maker-backtest-smoke"
        order_id_tag = "001"
        oms_type = "netting"
        client_id = "maker_execution_client"
        trade_flow_window_secs = 600
        trade_flow_max_samples = 1000
        mu_min_classified_samples = 4
        mu_stale_window_ms = 60000
        mu_min_floor = 0.05
        requote_min_interval_ms = 500
        quote_interval_ms = 1000
        market_portfolio_max_active_markets = 1
        market_portfolio_total_bankroll_notional = 1500.0
        market_portfolio_min_slot_notional = 100.0
        markets_config_digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

        [[markets]]
        market_key = "sample-event"
        family_key = "static_binary_event"
        underlying_asset = "ETH"
        cadence_seconds = 60
        cadence_slug_token = "will-sample-event-resolve-yes"
        static_condition_id = "condition-sample-event"
        static_yes_outcome = "Yes"
        static_no_outcome = "No"
        "#
        .to_string()
    }

    fn maker_smoke_venue() -> ManifestVenueConfig {
        ManifestVenueConfig {
            nt_venue: MAKER_SMOKE_VENUE.to_string(),
            oms_type: "NETTING".to_string(),
            account_type: "CASH".to_string(),
            book_type: "L1_MBP".to_string(),
            starting_balances: vec!["1_000_000 USDC".to_string()],
            routing: false,
            frozen_account: false,
            reject_stop_orders: true,
            support_gtd_orders: true,
            support_contingent_orders: true,
            use_position_ids: true,
            use_random_ids: false,
            use_reduce_only: true,
            bar_execution: true,
            bar_adaptive_high_low_ordering: false,
            trade_execution: true,
            use_market_order_acks: false,
            liquidity_consumption: false,
            allow_cash_borrowing: false,
            queue_position: false,
            oto_trigger_mode: "PARTIAL".to_string(),
            base_currency: "NONE".to_string(),
            default_leverage: "1".to_string(),
            price_protection_points: 0,
            leverages: None,
            margin_model: None,
            modules: None,
            fill_model: None,
            latency_model: None,
            fee_model: None,
            settlement_prices: None,
        }
    }

    fn maker_smoke_catalog_input(catalog_path: &str, instrument_id: &str) -> ManifestCatalogInput {
        ManifestCatalogInput {
            catalog_path: catalog_path.to_string(),
            catalog_fs_protocol: CATALOG_FS_PROTOCOL_NONE.to_string(),
            catalog_fs_storage_options: BTreeMap::new(),
            catalog_fs_rust_storage_options: BTreeMap::new(),
            data_type: "TradeTick".to_string(),
            nt_instrument_id: instrument_id.to_string(),
            instrument_ids: None,
            start_time: None,
            end_time: None,
            filter_expr: None,
            client_id: Some(MAKER_SMOKE_CLIENT_ID.to_string()),
            metadata: None,
            bar_spec: None,
            bar_types: None,
            optimize_file_loading: None,
        }
    }

    fn maker_smoke_manifest(catalog_root: &Path) -> BacktestingRunManifest {
        let catalog_path = catalog_root.to_str().expect("catalog path is UTF-8");
        BacktestingRunManifest {
            manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
            run_id: MAKER_SMOKE_RUN_ID.to_string(),
            target_bolt_v2_branch: "codex/437-maker-backtest-allowlist".to_string(),
            target_bolt_v2_ref: "worktree".to_string(),
            resolved_nt_version:
                crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
                    .expect("BVS NautilusTrader dependency provenance"),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            venue_binding_key: "maker-smoke-static-binary-event".to_string(),
            run_purpose: RunPurpose::Normal,
            source_proof_id: "maker-smoke-source-proof".to_string(),
            source_proof_version: 1,
            pins_non_latest_proof: false,
            proof_pin_reason_code: None,
            proof_pin_reason_detail: None,
            strategy: StrategySource {
                source_kind: StrategySourceKind::CompiledRustRegistry,
                registry_key: STRATEGY_BINARY_ORACLE_MAKER.to_string(),
                parameters: BTreeMap::from([
                    ("config_toml".to_string(), maker_smoke_config_toml()),
                    (STRATEGY_PARAM_FEE_BPS.to_string(), "0".to_string()),
                    (
                        STRATEGY_PARAM_ORDER_EXECUTION_MODE.to_string(),
                        "shadow".to_string(),
                    ),
                ]),
                typed_config_uri: None,
                typed_config_hash: None,
                experiment_result_uri: None,
                experiment_result_hash: None,
                config_overlay: None,
            },
            strategy_config_hash: sha256_hex(maker_smoke_config_toml().as_bytes()),
            venue: maker_smoke_venue(),
            additional_venues: Vec::new(),
            catalog_inputs: vec![
                maker_smoke_catalog_input(catalog_path, MAKER_SMOKE_YES_INSTRUMENT),
                maker_smoke_catalog_input(catalog_path, MAKER_SMOKE_NO_INSTRUMENT),
            ],
            reconstructed_reference_current_price: Vec::new(),
            instrument_settlements: Vec::new(),
            catalog_hash: sha256_hex(catalog_path.as_bytes()),
            execution_model: "nt_backtest_node".to_string(),
            artifact_root: "memory://maker-smoke".to_string(),
            output_prefix: "maker-smoke".to_string(),
            artifact_store: ManifestArtifactStore {
                storage_options: BTreeMap::new(),
                rust_storage_options: BTreeMap::new(),
                ssm_parameters: None,
            },
            domain_metrics: Vec::new(),
            start_time: None,
            end_time: None,
        }
    }

    #[test]
    fn binary_oracle_maker_manifest_runs_through_backtest_node() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create maker smoke catalog root")?;
        write_maker_smoke_catalog(tempdir.path())?;
        let manifest = maker_smoke_manifest(tempdir.path());

        let output = run_nt_backtest_node(&manifest)
            .context("run binary-oracle maker manifest through BacktestNode")?;

        ensure!(
            output.result.run_config_id.as_deref() == Some(MAKER_SMOKE_RUN_ID),
            "maker smoke returned unexpected run id {:?}",
            output.result.run_config_id
        );
        ensure!(
            output.result.iterations == 2,
            "maker smoke must iterate the two catalog trade ticks, got {}",
            output.result.iterations
        );
        let guard = output
            .run_guard_report
            .as_ref()
            .context("missing binary-oracle maker run guard report")?;
        ensure!(
            !guard.armed,
            "maker smoke must remain not armed until the quote cycle is wired; guard={guard:?}"
        );
        ensure!(
            output.result.total_orders == 0,
            "unwired maker quote cycle must not submit orders, got {}",
            output.result.total_orders
        );
        ensure!(
            guard
                .did_not_arm_reason
                .as_deref()
                .is_some_and(|reason| !reason.trim().is_empty()),
            "unwired maker quote cycle must report why it did not arm; guard={guard:?}"
        );
        Ok(())
    }

    #[test]
    fn runner_propagates_execution_contract_validator_failure() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create execution smoke catalog root")?;
        write_execution_contract_smoke_catalog(tempdir.path())?;
        let mut manifest = maker_smoke_manifest(tempdir.path());
        manifest.run_id = "execution-contract-runner-smoke".to_string();
        manifest.strategy.registry_key = STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE.to_string();
        manifest.strategy.parameters = BTreeMap::from([
            ("trade_size".to_string(), "1.00".to_string()),
            ("entry_after_trades".to_string(), "1".to_string()),
            ("exit_after_trades".to_string(), "1".to_string()),
            ("side".to_string(), "buy".to_string()),
        ]);
        manifest.catalog_inputs.truncate(1);
        let result = run_nt_backtest_node_with_execution_contract(&manifest, |_| {
            anyhow::bail!("execution-contract-validator-sentinel")
        });
        let error = match result {
            Ok(_) => anyhow::bail!("runner ignored the execution-contract validator failure"),
            Err(error) => error,
        };
        ensure!(
            format!("{error:#}").contains("execution-contract-validator-sentinel"),
            "runner did not propagate the execution-contract validator failure: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn execution_contract_excludes_late_arriving_book_deltas() -> Result<()> {
        let instrument_id = InstrumentId::from(MAKER_SMOKE_YES_INSTRUMENT);
        let timely = OrderBookDelta::new(
            instrument_id,
            BookAction::Add,
            nautilus_model::data::BookOrder::new(
                OrderSide::Sell,
                Price::from("0.420"),
                Quantity::from("21.52"),
                1,
            ),
            0,
            1,
            UnixNanos::from(10),
            UnixNanos::from(100),
        );
        let late = OrderBookDelta::new(
            instrument_id,
            BookAction::Add,
            nautilus_model::data::BookOrder::new(
                OrderSide::Sell,
                Price::from("0.410"),
                Quantity::from("50.00"),
                2,
            ),
            0,
            2,
            UnixNanos::from(9),
            UnixNanos::from(101),
        );

        let book = replay_executable_book_at_submission(
            instrument_id,
            &[timely, late],
            UnixNanos::from(100),
        )?;

        ensure!(
            book.best_ask_price() == Some(Price::from("0.420")),
            "late-arriving delta leaked into the executable book"
        );
        Ok(())
    }

    #[test]
    fn execution_contract_config_identity_covers_applied_rv_source_filter() -> Result<()> {
        let raw_config = toml::from_str::<toml::Value>(&maker_smoke_config_toml())?;
        let mut override_spec = StrategyConfigOverlaySource {
            production_root_config_path: "config/root.toml".to_string(),
            override_delta: ManifestBacktestConfigOverride {
                label: "test override".to_string(),
                strategy_instance_id: "binary_oracle_btc".to_string(),
                signal_role: "primary".to_string(),
                signal_data_client_id: "okx_data".to_string(),
                signal_instrument_id: "BTC-USDT.OKX".to_string(),
                realized_volatility_surface_id: "btc_usdt_midpoint_rv".to_string(),
                keep_realized_volatility_sources: vec![
                    ManifestRealizedVolatilitySourceSelector {
                        data_client_id: "okx_data".to_string(),
                        instrument_id: "BTC-USDT.OKX".to_string(),
                    },
                    ManifestRealizedVolatilitySourceSelector {
                        data_client_id: "bybit_data".to_string(),
                        instrument_id: "BTCUSDT-SPOT.BYBIT".to_string(),
                    },
                ],
            },
        }
        .to_bolt_v3_override();
        let production_checksum = "a".repeat(64);
        let both_sources = canonical_resolved_taker_config_bytes(
            &raw_config,
            Some(&production_checksum),
            Some(&override_spec),
        )?;
        override_spec.keep_realized_volatility_sources.pop();
        let okx_only = canonical_resolved_taker_config_bytes(
            &raw_config,
            Some(&production_checksum),
            Some(&override_spec),
        )?;

        ensure!(
            sha256_hex(&both_sources) != sha256_hex(&okx_only),
            "RV source filter changed without changing resolved-config provenance"
        );
        Ok(())
    }

    #[test]
    fn unknown_manifest_strategy_still_fails_allowlist() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create unknown-strategy catalog root")?;
        write_maker_smoke_catalog(tempdir.path())?;
        let mut manifest = maker_smoke_manifest(tempdir.path());
        manifest.strategy.registry_key = "binary_oracle_maker_typo".to_string();

        let error = run_nt_backtest_node(&manifest)
            .err()
            .context("unknown strategy unexpectedly ran")?;

        ensure!(
            error
                .to_string()
                .contains("not a registered compiled Rust strategy"),
            "unexpected unknown-strategy error: {error:#}"
        );
        Ok(())
    }

    const ISSUE_789_START_MS: u64 = 1_776_816_000_000;
    const ISSUE_789_END_MS: u64 = 1_776_816_300_000;
    const ISSUE_789_START_NS: i64 = 1_776_816_000_000_000_000;
    const ISSUE_789_END_NS: i64 = 1_776_816_300_000_000_000;
    const ISSUE_789_RESULT_ARTIFACT_ROLE: &str = "issue-789-result-artifact.v1";
    const ISSUE_789_CONDITION_ID: &str =
        "0xb98f764c4d5dd36580c8c9903bc75ddcb631428d84e9c1e532f0da236f77054c";
    const ISSUE_789_UP_TOKEN: &str =
        "70185630899601185587604849909583851214968263628583846260964185007520683306835";
    const ISSUE_789_DOWN_TOKEN: &str =
        "39327110184724906690545821148183414832224062782460969169826610548819991310639";
    const ISSUE_789_MARKET_SLUG: &str = "btc-updown-5m-1776816000";

    #[test]
    fn issue_789_first_real_free_data_taker_pl() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create issue #789 temp catalog root")?;
        let okx_quotes = seeded_quote_table(
            &gunzip_pinned_fixture(
                include_bytes!(
                    "../tests/fixtures/issue_789_first_pl/okx_btc_usdt_l2_20260422_000000_000300.jsonl.gz"
                ),
                ISSUE_789_OKX_FIXTURE_SHA256,
                "okx",
            )?,
            okx_seeded_l2_mapping(),
            QuoteTableSpec {
                source_binding: "okx-official-historical-l2-400lv",
                venue: "OKX",
                instrument_id: "BTC-USDT",
                venue_symbol: "BTC-USDT",
                nt_instrument_id: "BTC-USDT.OKX",
                payload_id: "https://static.okx.com/cdn/okx/match/orderbook/L2/400lv/daily/20260422/BTC-USDT-L2orderbook-400lv-2026-04-22.tar.gz",
            },
        )?;
        let bybit_quotes = seeded_quote_table(
            &gunzip_pinned_fixture(
                include_bytes!(
                    "../tests/fixtures/issue_789_first_pl/bybit_btc_usdt_l2_20260422_000000_000300.jsonl.gz"
                ),
                ISSUE_789_BYBIT_FIXTURE_SHA256,
                "bybit",
            )?,
            bybit_seeded_l2_mapping(),
            QuoteTableSpec {
                source_binding: "bybit-quote-saver-ob200",
                venue: "BYBIT",
                instrument_id: "BTCUSDT",
                venue_symbol: "BTCUSDT",
                nt_instrument_id: "BTCUSDT-SPOT.BYBIT",
                payload_id: "https://quote-saver.bycsi.com/orderbook/spot/BTCUSDT/2026-04-22_BTCUSDT_ob200.data.zip",
            },
        )?;

        let okx_catalog = tempdir.path().join("okx_btc_usdt_quotes");
        let okx_projection = project_canonical_quotes_to_catalog(
            &okx_quotes,
            &spot_spec(
                "BTC-USDT.OKX",
                "BTC-USDT",
                "BTC",
                "USDT",
                "0.1",
                "0.00000001",
            ),
            &okx_catalog,
        )
        .context("project OKX seeded L2 BBO quotes")?;
        let bybit_catalog = tempdir.path().join("bybit_btc_usdt_quotes");
        let bybit_projection = project_canonical_quotes_to_catalog(
            &bybit_quotes,
            &spot_spec(
                "BTCUSDT-SPOT.BYBIT",
                "BTCUSDT",
                "BTC",
                "USDT",
                "0.1",
                "0.000001",
            ),
            &bybit_catalog,
        )
        .context("project Bybit seeded L2 BBO quotes")?;

        let gamma_markets = issue_789_gamma_markets()?;
        let pmxt_rows = issue_789_pmxt_rows()?;
        let up_projection = project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
            source_binding: "pmxt-free-r2-archive".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            drop_quotes_missing_side: true,
            selected_condition_id: ISSUE_789_CONDITION_ID.to_string(),
            selected_token_id: ISSUE_789_UP_TOKEN.to_string(),
            gamma_markets: gamma_markets.clone(),
            rows: pmxt_rows_for_token(&pmxt_rows, ISSUE_789_UP_TOKEN)?,
        })
        .context("project PMXT Up book/trades")?;
        let down_projection = project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
            source_binding: "pmxt-free-r2-archive".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            drop_quotes_missing_side: true,
            selected_condition_id: ISSUE_789_CONDITION_ID.to_string(),
            selected_token_id: ISSUE_789_DOWN_TOKEN.to_string(),
            gamma_markets,
            rows: pmxt_rows_for_token(&pmxt_rows, ISSUE_789_DOWN_TOKEN)?,
        })
        .context("project PMXT Down book/trades")?;
        let up_instrument_id = up_projection.instrument.id().to_string();
        let down_instrument_id = down_projection.instrument.id().to_string();
        let up_catalog = tempdir.path().join("pmxt_up_catalog");
        let down_catalog = tempdir.path().join("pmxt_down_catalog");
        let up_catalog_report =
            write_pmxt_one_off_projection_to_catalog(&up_catalog, &up_projection)
                .context("write PMXT Up catalog")?;
        let down_catalog_report =
            write_pmxt_one_off_projection_to_catalog(&down_catalog, &down_projection)
                .context("write PMXT Down catalog")?;

        let chainlink_catalog = tempdir.path().join("chainlink_price_to_beat");
        let chainlink_table = reconstructed_chainlink_price_to_beat_table(&okx_quotes)?;
        let chainlink_projection = project_canonical_index_to_catalog(
            &chainlink_table,
            &spot_spec(
                "BTC-USD.CHAINLINK",
                "BTC-USD",
                "BTC",
                "USD",
                "0.000001",
                "0.000001",
            ),
            &chainlink_catalog,
        )
        .context("project reconstructed Chainlink price-to-beat index updates")?;

        // Faithful settlement: replay the REAL market resolution observed in the
        // archive. At resolution the winning binary token's order book converges
        // to ~1.0 and the loser's to ~0.0; redeem the winner at 1.0 and the loser
        // at 0.0 so a held position books its true realized P/L. The winner is read
        // from the terminal best-bid of the SAME PMXT rows that drive the backtest
        // (single source of truth — not synthesized).
        let up_terminal_bid = issue_789_terminal_best_bid(&pmxt_rows, ISSUE_789_UP_TOKEN)?;
        let down_terminal_bid = issue_789_terminal_best_bid(&pmxt_rows, ISSUE_789_DOWN_TOKEN)?;
        let binary_midpoint = Decimal::new(5, 1);
        ensure!(
            (up_terminal_bid > binary_midpoint) ^ (down_terminal_bid > binary_midpoint),
            "ambiguous #789 resolution: up_terminal_bid={up_terminal_bid} down_terminal_bid={down_terminal_bid}"
        );
        let (up_close, down_close) = if up_terminal_bid > down_terminal_bid {
            ("1", "0")
        } else {
            ("0", "1")
        };
        let up_precision = up_projection.instrument.price_precision();
        let down_precision = down_projection.instrument.price_precision();
        let instrument_settlements = vec![
            ManifestInstrumentSettlementInput {
                nt_instrument_id: up_instrument_id.clone(),
                close_price: format!(
                    "{up_close}.{zeros}",
                    zeros = "0".repeat(up_precision as usize)
                ),
                price_precision: up_precision,
                ts_event_ns: ISSUE_789_END_NS as u64,
                ts_init_ns: ISSUE_789_END_NS as u64,
                // The binary settles in pUSD (NT Polymarket collateral); bind it so a
                // mis-funded venue fails loud instead of silently dropping the loss.
                settlement_currency: "pUSD".to_string(),
            },
            ManifestInstrumentSettlementInput {
                nt_instrument_id: down_instrument_id.clone(),
                close_price: format!(
                    "{down_close}.{zeros}",
                    zeros = "0".repeat(down_precision as usize)
                ),
                price_precision: down_precision,
                ts_event_ns: ISSUE_789_END_NS as u64,
                ts_init_ns: ISSUE_789_END_NS as u64,
                settlement_currency: "pUSD".to_string(),
            },
        ];

        let manifest = issue_789_manifest(Issue789Catalogs {
            okx_catalog,
            okx_catalog_hash: okx_projection.catalog_hash.clone(),
            bybit_catalog,
            bybit_catalog_hash: bybit_projection.catalog_hash.clone(),
            chainlink_catalog,
            chainlink_catalog_hash: chainlink_projection.catalog_hash.clone(),
            up_catalog,
            up_catalog_hash: up_catalog_report.catalog_hash.clone(),
            up_instrument_id,
            down_catalog,
            down_catalog_hash: down_catalog_report.catalog_hash.clone(),
            down_instrument_id,
            reference_rows: reconstructed_reference_rows_from_okx(&okx_quotes)?,
            instrument_settlements,
        })?;

        let output = run_nt_backtest_node_with_execution_contract(&manifest, |output| {
            validate_issue_789_execution_contract(
                output,
                &manifest,
                &up_projection,
                &down_projection,
            )
        })
        .context("run issue #789 first real free-data taker P/L slice")?;
        let guard = output
            .run_guard_report
            .as_ref()
            .context("missing binary-oracle run guard report")?;
        let did_not_arm = || {
            guard.did_not_arm_reason.clone().unwrap_or_else(|| {
                "did NOT arm — guard did not provide a feed-specific reason".to_string()
            })
        };

        println!("issue_789_result_label=production config + documented OKX/Bybit override");
        println!(
            "issue_789_override_report={:?}",
            output.config_override_report
        );
        println!(
            "issue_789_feed_labels=signal:OKX real snapshot-seeded L2 BBO; rv:OKX real snapshot-seeded L2 BBO; rv:Bybit real snapshot-seeded L2 BBO; tradable:PMXT real R2 archive book/price_change/trades WITH converter-synthesized uncross deltas (not byte-faithful); strike/reference:reconstructed-from-spot not raw Chainlink; fidelity:ZERO-LATENCY single-clock replay (spot/reference on exchange event-time, fast-venue age pinned 0; ~120ms live spot->PM lead NOT modeled) — the P/L is a reconstructed-replay figure, not latency-aware"
        );
        println!(
            "issue_789_guard total_orders={} total_positions={} armed={} traded={} signal_quote_received={} rv_ready={} price_to_beat_received={} reference_fresh={} latest_market_id={:?} latest_spot_price={:?} latest_price_to_beat={:?} latest_reference={:?} rv_sources={:?} rv_blockers={:?}",
            output.result.total_orders,
            output.result.total_positions,
            guard.armed,
            guard.traded,
            guard.signal_quote_received,
            guard.realized_volatility_ready,
            guard.price_to_beat_received,
            guard.reference_fresh,
            guard.latest_market_id,
            guard.latest_spot_price,
            guard.latest_price_to_beat_value,
            guard.latest_reference_current_price,
            guard.latest_realized_volatility_sources_used,
            guard.latest_realized_volatility_blockers,
        );
        println!("issue_789_stats_pnls={:?}", output.result.stats_pnls);
        println!("issue_789_stats_returns={:?}", output.result.stats_returns);
        write_issue_789_result_artifact(&output, guard)?;

        ensure!(guard.signal_quote_received, "{}", did_not_arm());
        ensure!(guard.realized_volatility_ready, "{}", did_not_arm());
        ensure!(guard.price_to_beat_received, "{}", did_not_arm());
        ensure!(guard.reference_fresh, "{}", did_not_arm());
        ensure!(guard.armed, "{}", did_not_arm());
        ensure!(output.result.total_orders > 0, "{}", did_not_arm());
        ensure!(output.result.total_positions > 0, "{}", did_not_arm());
        ensure!(guard.traded, "{}", did_not_arm());
        // The override keeps OKX + Bybit RV sources, but min_ready_sources=1 means
        // OKX alone satisfies readiness — so rv_ready does NOT prove Bybit fed the
        // surface. Assert Bybit actually contributed, else a silent Bybit routing
        // drop (e.g. an instrument-id drift) would still report "OKX+Bybit RV".
        ensure!(
            guard
                .latest_realized_volatility_sources_used
                .iter()
                .any(|source| source.to_ascii_lowercase().contains("bybit")),
            "issue #789 RV claims OKX+Bybit but the Bybit source never contributed (likely a routing/id drift); sources_used={:?}",
            guard.latest_realized_volatility_sources_used
        );
        ensure!(
            !output.result.stats_pnls.is_empty(),
            "issue #789 run traded but stats_pnls was empty"
        );
        ensure!(
            output.execution_contract_report.is_some(),
            "issue #789 runner omitted the required execution contract report"
        );
        Ok(())
    }

    fn validate_issue_789_execution_contract(
        output: &super::NtBacktestNodeRun,
        manifest: &BacktestingRunManifest,
        up_projection: &crate::pmxt_one_off_backfill_projection::PmxtOneOffNtProjection,
        down_projection: &crate::pmxt_one_off_backfill_projection::PmxtOneOffNtProjection,
    ) -> Result<crate::execution_contract::ExecutionContractReport> {
        let settlement_ts = UnixNanos::from(ISSUE_789_END_NS as u64);
        let entry_orders: Vec<_> = output
            .order_terminals
            .iter()
            .filter(|order| order.fills.iter().any(|fill| fill.ts_event < settlement_ts))
            .collect();
        ensure!(
            entry_orders.len() == 1,
            "issue #789 requires exactly one pre-settlement entry order, got {}",
            entry_orders.len()
        );
        let entry_order = entry_orders[0];
        let entry_fills: Vec<_> = entry_order
            .fills
            .iter()
            .filter(|fill| fill.ts_event < settlement_ts)
            .cloned()
            .collect();
        ensure!(
            !entry_fills.is_empty(),
            "issue #789 entry order has no executable-book fills"
        );
        let instrument_id = entry_fills[0].instrument_id;
        let projection = if up_projection.instrument.id() == instrument_id {
            up_projection
        } else if down_projection.instrument.id() == instrument_id {
            down_projection
        } else {
            anyhow::bail!("issue #789 fill instrument {instrument_id} has no PMXT projection")
        };
        let submission_timestamp = entry_order
            .submission_timestamp
            .context("issue #789 entry order has no submission timestamp")?;
        let executable_book = replay_executable_book_at_submission(
            instrument_id,
            &projection.order_book_deltas,
            submission_timestamp,
        )?;
        let positions: Vec<_> = output
            .positions
            .iter()
            .filter(|position| position.instrument_id == instrument_id)
            .collect();
        ensure!(
            positions.len() == 1,
            "issue #789 requires exactly one position for {instrument_id}, got {}",
            positions.len()
        );
        let position = positions[0];
        let realized_pnl = position
            .realized_pnl
            .context("issue #789 position has no realized PnL")?;
        let matching_balances: Vec<_> = output
            .account_balances
            .iter()
            .filter(|balance| balance.currency == realized_pnl.currency)
            .collect();
        ensure!(
            matching_balances.len() == 1,
            "issue #789 requires exactly one terminal {} balance, got {}",
            realized_pnl.currency,
            matching_balances.len()
        );
        let terminal_cash = matching_balances[0].total;
        let initial_balances: Vec<Money> = manifest
            .venue
            .starting_balances
            .iter()
            .map(|balance| {
                Money::from_str(&balance.replace('_', ""))
                    .map_err(anyhow::Error::msg)
                    .context("parse issue #789 exact initial balance")
            })
            .collect::<Result<_>>()?;
        let matching_initial_balances: Vec<_> = initial_balances
            .iter()
            .filter(|balance| balance.currency == realized_pnl.currency)
            .collect();
        ensure!(
            matching_initial_balances.len() == 1,
            "issue #789 requires exactly one initial {} balance, got {}",
            realized_pnl.currency,
            matching_initial_balances.len()
        );
        let initial_cash = *matching_initial_balances[0];
        let position_commission = position
            .commissions
            .get(&realized_pnl.currency)
            .copied()
            .unwrap_or_else(|| Money::zero(realized_pnl.currency));
        let settlement = manifest
            .instrument_settlements
            .iter()
            .find(|settlement| settlement.nt_instrument_id == instrument_id.to_string())
            .context("issue #789 fill instrument has no settlement")?;
        let settlement_price = Price::from_str(&settlement.close_price)
            .map_err(|error| anyhow::anyhow!(error))
            .context("parse issue #789 exact settlement price")?;
        let resolved_config_bytes = output
            .resolved_config_bytes
            .as_deref()
            .context("issue #789 runner omitted resolved config bytes")?;

        crate::execution_contract::validate_execution_contract(
            &crate::execution_contract::ExecutionContractTrace {
                instrument: &projection.instrument,
                executable_book: &executable_book,
                order_side: entry_fills[0].order_side,
                submitted_quantity: entry_order.initialized_quantity,
                quote_quantity: entry_order.initialized_quote_quantity,
                effective_base_quantity: entry_order.effective_quantity,
                fills: &entry_fills,
                position_fills: &position.events,
                settlement_price,
                initial_cash,
                terminal_cash,
                realized_pnl,
                position_commission,
                expected_fill_commission: Money::zero(realized_pnl.currency),
                canonical_resolved_config_bytes: resolved_config_bytes,
                canonical_resolved_config_sha256: &manifest.strategy_config_hash,
            },
        )
    }

    fn write_issue_789_result_artifact(
        output: &super::NtBacktestNodeRun,
        guard: &crate::result_contract::BacktestRunGuardReport,
    ) -> Result<()> {
        let Ok(path) = std::env::var("BOLT_ISSUE_789_RESULT_PATH") else {
            return Ok(());
        };
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create issue #789 result dir {}", parent.display()))?;
        }
        let payload = serde_json::json!({
            "result_label": "production config + documented OKX/Bybit override",
            "feed_labels": {
                "signal": "OKX real snapshot-seeded L2 BBO",
                "rv_okx": "OKX real snapshot-seeded L2 BBO",
                "rv_bybit": "Bybit real snapshot-seeded L2 BBO",
                "tradable": "PMXT real R2 archive book/price_change/trades, with converter-synthesized uncross deltas (not byte-faithful)",
                "strike": "reconstructed-from-spot, not raw Chainlink",
                "reference": "reconstructed-from-spot, not raw Chainlink"
            },
            "fidelity": {
                "inter_feed_latency_collapsed": true,
                "clock_model": "single-clock zero-latency replay: spot/reference/strike on exchange event-time, fast-venue age pinned to 0; the ~120ms live spot->PM lead is NOT modeled",
                "tradable_book": "real R2 archive with converter-synthesized uncross Delete deltas (pruned crossings the archive omitted), not a byte-faithful raw replay",
                "note": "the realized P/L is a zero-latency reconstructed-replay figure, not a latency-aware live-faithful P/L"
            },
            "total_orders": output.result.total_orders,
            "total_positions": output.result.total_positions,
            "resolved_config_sha256": output.resolved_config_hash,
            "execution_contract_validated_fill_count": output
                .execution_contract_report
                .as_ref()
                .map(|report| report.validated_fill_count),
            "stats_pnls_debug": format!("{:?}", output.result.stats_pnls),
            "stats_returns_debug": format!("{:?}", output.result.stats_returns),
            "guard": {
                "strategy_input_snapshot_count": guard.strategy_input_snapshot_count,
                "order_intent_count": guard.order_intent_count,
                "admission_decision_count": guard.admission_decision_count,
                "admitted_order_count": guard.admitted_order_count,
                "submit_reservation_count": guard.submit_reservation_count,
                "submit_fill_count": guard.submit_fill_count,
                "entry_skip_count": guard.entry_skip_count,
                "exit_decision_count": guard.exit_decision_count,
                "loss_governor_halt_count": guard.loss_governor_halt_count,
                "requote_throttle_count": guard.requote_throttle_count,
                "signal_quote_received": guard.signal_quote_received,
                "realized_volatility_ready": guard.realized_volatility_ready,
                "price_to_beat_received": guard.price_to_beat_received,
                "reference_fresh": guard.reference_fresh,
                "armed": guard.armed,
                "traded": guard.traded,
                "latest_market_id": guard.latest_market_id.clone(),
                "latest_spot_price": guard.latest_spot_price.clone(),
                "latest_reference_current_price": guard.latest_reference_current_price.clone(),
                "latest_reference_current_price_source_id": guard.latest_reference_current_price_source_id.clone(),
                "latest_price_to_beat_value": guard.latest_price_to_beat_value.clone(),
                "latest_realized_volatility_as_of_ms": guard.latest_realized_volatility_as_of_ms,
                "latest_realized_volatility_sources_used": guard.latest_realized_volatility_sources_used.clone(),
                "latest_realized_volatility_blockers": guard.latest_realized_volatility_blockers.clone(),
                "did_not_arm_reason": guard.did_not_arm_reason.clone()
            }
        });
        crate::reference_artifact::write_reference_artifact_with_len(
            &path,
            ISSUE_789_RESULT_ARTIFACT_ROLE,
            &payload,
            crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
        )
        .with_context(|| format!("write issue #789 result artifact {}", path.display()))?;
        Ok(())
    }

    struct QuoteTableSpec<'a> {
        source_binding: &'a str,
        venue: &'a str,
        instrument_id: &'a str,
        venue_symbol: &'a str,
        nt_instrument_id: &'a str,
        payload_id: &'a str,
    }

    /// Decompress a gzip-embedded issue #789 fixture into its plaintext form.
    /// The real OKX/Bybit L2 and PMXT R2 windows are large, so they are committed
    /// gzip-compressed and embedded via `include_bytes!`; this keeps the test
    /// hermetic without bloating the source tree with tens of MB of plaintext.
    fn gunzip_fixture(bytes: &[u8]) -> Result<String> {
        use std::io::Read as _;
        let mut text = String::new();
        flate2::read::GzDecoder::new(bytes)
            .read_to_string(&mut text)
            .context("gunzip issue #789 fixture")?;
        Ok(text)
    }

    /// sha256 of each issue #789 fixture's DECOMPRESSED content, pinning the
    /// committed `.gz` bytes to the exact rows replayed. The whole −$0.95 P/L
    /// derives from these three archives; without a pin a single edited price or
    /// size would change the result undetectably (the gzip CRC only catches
    /// accidental corruption, not a deliberate edit). On intentional regeneration
    /// from the public archives named in each `payload_id`, recompute with
    /// `gunzip -c <fixture>.gz | shasum -a 256` and update the matching constant.
    const ISSUE_789_OKX_FIXTURE_SHA256: &str =
        "36f749da9e40ff88cbdabff2b070c8086102d5f136168e3c648f1ec7dd8651d0";
    const ISSUE_789_BYBIT_FIXTURE_SHA256: &str =
        "4df60a46aaf424222c42a2fb2ea42b43b599065c0709854f8e89c9b92646dfea";
    const ISSUE_789_PMXT_FIXTURE_SHA256: &str =
        "5b108b1fb82701173a05aac734089f3cddf6f133fbadc4abad47fda40b92dffb";

    /// Decompress a committed fixture and fail loud unless its content hash matches
    /// the pinned value, so any post-commit edit to the replayed data is caught.
    fn gunzip_pinned_fixture(bytes: &[u8], expected_sha256: &str, name: &str) -> Result<String> {
        let text = gunzip_fixture(bytes)?;
        let actual = sha256_hex(text.as_bytes());
        ensure!(
            actual == expected_sha256,
            "issue #789 fixture {name} content hash {actual} != pinned {expected_sha256}: the committed bytes were altered or regenerated. Re-confirm provenance against the documented public archive before trusting any P/L derived from it."
        );
        Ok(text)
    }

    fn seeded_quote_table(
        jsonl: &str,
        mapping: SeededL2QuoteMappingConfig,
        spec: QuoteTableSpec<'_>,
    ) -> Result<CanonicalQuotesTable> {
        let events = parse_seeded_l2_jsonl(&mapping, jsonl)
            .with_context(|| format!("parse {} seeded L2 rows", spec.venue))?;
        ensure!(
            events
                .first()
                .is_some_and(|event| event.action == SeededL2QuoteAction::Snapshot),
            "{} seeded L2 fixture must start with a snapshot row",
            spec.venue
        );
        let provenance = SeededL2QuoteProvenance {
            ingest_run_id: "issue-789-first-real-pl".to_string(),
            source_binding: spec.source_binding.to_string(),
            venue: spec.venue.to_string(),
            product_family: "spot".to_string(),
            product_category: "l2_orderbook".to_string(),
            instrument_id: spec.instrument_id.to_string(),
            canonical_instrument_key: format!("spot/{}", spec.venue_symbol),
            venue_symbol: spec.venue_symbol.to_string(),
            nt_instrument_id: Some(spec.nt_instrument_id.to_string()),
            partition_dt: "2026-04-22".to_string(),
            source_proof_id: format!(
                "issue-789-{}-snapshot-seeded-l2",
                spec.venue.to_ascii_lowercase()
            ),
            source_proof_version: 1,
            forbidden_claims: vec!["raw-bbo-claim-without-snapshot-seeded-replay".to_string()],
            raw_payload_id: spec.payload_id.to_string(),
            payload_hash: sha256_hex(jsonl.as_bytes()),
            transform_hash: seeded_l2_quote_transform_hash(),
            default_capture_time: events[0].event_time,
        };
        normalize_seeded_l2_events(&provenance, &events)
            .with_context(|| format!("normalize {} seeded L2 BBO quotes", spec.venue))
    }

    fn okx_seeded_l2_mapping() -> SeededL2QuoteMappingConfig {
        SeededL2QuoteMappingConfig {
            action_path: vec!["action".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["bids".to_string()],
            asks_path: vec!["asks".to_string()],
            level_price_index: 0,
            level_size_index: 1,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["update".to_string()],
            source_sequence_path: None,
        }
    }

    fn bybit_seeded_l2_mapping() -> SeededL2QuoteMappingConfig {
        SeededL2QuoteMappingConfig {
            action_path: vec!["type".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["data".to_string(), "b".to_string()],
            asks_path: vec!["data".to_string(), "a".to_string()],
            level_price_index: 0,
            level_size_index: 1,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["delta".to_string()],
            source_sequence_path: Some(vec!["data".to_string(), "seq".to_string()]),
        }
    }

    fn spot_spec(
        nt_instrument_id: &str,
        raw_symbol: &str,
        base_currency: &str,
        quote_currency: &str,
        price_increment: &str,
        size_increment: &str,
    ) -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            nt_instrument_id: nt_instrument_id.to_string(),
            raw_symbol: raw_symbol.to_string(),
            base_currency: base_currency.to_string(),
            quote_currency: quote_currency.to_string(),
            price_increment: price_increment.to_string(),
            size_increment: size_increment.to_string(),
            min_quantity: size_increment.to_string(),
            max_quantity: "1000000".to_string(),
            min_notional: "0".to_string(),
            max_notional: "1000000000".to_string(),
        }
    }

    #[derive(Debug, Deserialize)]
    struct Issue789PmxtCsvRow {
        event_type: String,
        asset_id: String,
        bids: Option<String>,
        asks: Option<String>,
        price: Option<String>,
        size: Option<String>,
        side: Option<String>,
        best_bid: Option<String>,
        best_ask: Option<String>,
        transaction_hash: Option<String>,
        fee_rate_bps: Option<String>,
        timestamp_ms: String,
        ts_init_ns: u64,
    }

    /// The binary resolves AT market end; its outcome is read from the terminal
    /// best-bid of the same PMXT rows that drive the backtest (single source of
    /// truth). The terminal tick lands just after the half-open window end — the
    /// resolution is observed at the close — so the read is BOUNDED to this
    /// immediate post-close window rather than accepting an arbitrary future row.
    const ISSUE_789_RESOLUTION_OBSERVATION_TOLERANCE_NS: i64 = 1_000_000_000;

    /// Terminal (latest-`ts_init`) best-bid for a PMXT token, used to read the
    /// real binary resolution: the winning outcome's book converges to ~1.0.
    fn issue_789_terminal_best_bid(rows: &[Issue789PmxtCsvRow], token_id: &str) -> Result<Decimal> {
        let (terminal_ts, terminal_bid) = rows
            .iter()
            .filter(|row| row.asset_id == token_id)
            .filter_map(|row| row.best_bid.as_deref().map(|bid| (row.ts_init_ns, bid)))
            .max_by_key(|(ts, _)| *ts)
            .with_context(|| format!("no terminal best_bid for token {token_id}"))?;
        let end_ns = ISSUE_789_END_NS as u64;
        let max_resolution_ns =
            end_ns.saturating_add(ISSUE_789_RESOLUTION_OBSERVATION_TOLERANCE_NS as u64);
        ensure!(
            (end_ns..=max_resolution_ns).contains(&terminal_ts),
            "issue #789 resolution read for token {token_id} at ts {terminal_ts} is outside the post-close resolution bound [{end_ns}, {max_resolution_ns}]; the terminal tick must be the immediate at-/post-close observation, not arbitrary future data"
        );
        terminal_bid
            .trim()
            .parse::<Decimal>()
            .with_context(|| format!("parse terminal best_bid for token {token_id}"))
    }

    fn issue_789_pmxt_rows() -> Result<Vec<Issue789PmxtCsvRow>> {
        let csv_text = gunzip_pinned_fixture(
            include_bytes!(
                "../tests/fixtures/issue_789_first_pl/pmxt_btc_updown_5m_1776816000_rows.csv.gz"
            ),
            ISSUE_789_PMXT_FIXTURE_SHA256,
            "pmxt",
        )?;
        csv::Reader::from_reader(csv_text.as_bytes())
            .deserialize()
            .collect::<std::result::Result<Vec<Issue789PmxtCsvRow>, csv::Error>>()
            .context("parse issue #789 PMXT CSV fixture")
    }

    fn pmxt_rows_for_token(
        rows: &[Issue789PmxtCsvRow],
        token_id: &str,
    ) -> Result<Vec<PmxtOneOffSelectedRow>> {
        let mut selected = Vec::new();
        for row in rows.iter().filter(|row| row.asset_id == token_id) {
            match row.event_type.as_str() {
                "book" => {
                    selected.push(PmxtOneOffSelectedRow::BookSnapshot(PmxtOneOffSnapshotRow {
                        market: ISSUE_789_CONDITION_ID.to_string(),
                        asset_id: row.asset_id.clone(),
                        bids: parse_pmxt_book_levels(row.bids.as_deref().context("book bids")?)?,
                        asks: parse_pmxt_book_levels(row.asks.as_deref().context("book asks")?)?,
                        timestamp_ms: row.timestamp_ms.clone(),
                        ts_init: UnixNanos::from(row.ts_init_ns),
                    }))
                }
                "price_change" => {
                    selected.push(PmxtOneOffSelectedRow::PriceChange(PmxtPriceChangeRow {
                        market: ISSUE_789_CONDITION_ID.to_string(),
                        asset_id: row.asset_id.clone(),
                        price: row.price.clone().context("price_change price")?,
                        side: parse_pmxt_side(row.side.as_deref().context("price_change side")?)?,
                        size: row.size.clone().context("price_change size")?,
                        best_bid: row.best_bid.clone(),
                        best_ask: row.best_ask.clone(),
                        timestamp_ms: row.timestamp_ms.clone(),
                        ts_init: UnixNanos::from(row.ts_init_ns),
                    }))
                }
                "last_trade_price" => {
                    selected.push(PmxtOneOffSelectedRow::LastTrade(PmxtOneOffTradeRow {
                        market: ISSUE_789_CONDITION_ID.to_string(),
                        asset_id: row.asset_id.clone(),
                        transaction_hash: row
                            .transaction_hash
                            .clone()
                            .context("last_trade_price transaction_hash")?,
                        price: row.price.clone().context("last_trade_price price")?,
                        side: parse_pmxt_side(
                            row.side.as_deref().context("last_trade_price side")?,
                        )?,
                        size: row.size.clone().context("last_trade_price size")?,
                        fee_rate_bps: row
                            .fee_rate_bps
                            .clone()
                            .context("last_trade_price fee_rate_bps")?,
                        timestamp: UnixNanos::from(
                            row.timestamp_ms
                                .parse::<u64>()
                                .context("parse PMXT timestamp_ms")?
                                .checked_mul(1_000_000)
                                .context("PMXT timestamp_ms overflow")?,
                        ),
                        ts_init: UnixNanos::from(row.ts_init_ns),
                    }))
                }
                other => anyhow::bail!("unsupported issue #789 PMXT event_type {other:?}"),
            }
        }
        ensure!(
            !selected.is_empty(),
            "no PMXT rows selected for token {token_id}"
        );
        Ok(selected)
    }

    fn parse_pmxt_book_levels(raw: &str) -> Result<Vec<PmxtBookLevel>> {
        serde_json::from_str::<Vec<Vec<String>>>(raw)
            .context("parse PMXT book levels JSON")?
            .into_iter()
            .map(|level| {
                ensure!(
                    level.len() >= 2,
                    "PMXT book level must carry price and size, got {level:?}"
                );
                Ok(PmxtBookLevel {
                    price: level[0].clone(),
                    size: level[1].clone(),
                })
            })
            .collect()
    }

    fn parse_pmxt_side(raw: &str) -> Result<PmxtOneOffTickSide> {
        match raw {
            "BUY" => Ok(PmxtOneOffTickSide::Buy),
            "SELL" => Ok(PmxtOneOffTickSide::Sell),
            other => anyhow::bail!("unsupported PMXT side {other:?}"),
        }
    }

    /// Regression guard for the #789 crossed-book reconstruction defect.
    ///
    /// The raw PMXT archive's `best_bid`/`best_ask` are always uncrossed, but a
    /// naive level-delta replay of the snapshot-sparse archive accumulates stale
    /// opposite-side levels and the rebuilt book crosses on the large majority of
    /// ticks, which trips the taker's `BookCrossed` entry gate and blocks every
    /// trade. The projection now prunes levels that cross the venue's
    /// authoritative inside market. This applies the projected deltas through the
    /// same book maintenance the strategy uses (`OutcomeBookState::update_from_deltas`)
    /// and asserts the book stays uncrossed at every atomic update boundary while
    /// still becoming two-sided — so the guard is not vacuous.
    #[test]
    fn issue_789_reconstructed_pmxt_book_never_crosses() -> Result<()> {
        use std::collections::BTreeMap;

        use nautilus_model::{
            enums::{BookAction, OrderSide, RecordFlag},
            types::Price,
        };

        let gamma_markets = issue_789_gamma_markets()?;
        let pmxt_rows = issue_789_pmxt_rows()?;
        let last_flag = RecordFlag::F_LAST as u8;

        for token in [ISSUE_789_UP_TOKEN, ISSUE_789_DOWN_TOKEN] {
            let projection = project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
                source_binding: "pmxt-free-r2-archive".to_string(),
                usage_scope: SourceProofUsageScope::OneOffBackfillData,
                drop_quotes_missing_side: true,
                selected_condition_id: ISSUE_789_CONDITION_ID.to_string(),
                selected_token_id: token.to_string(),
                gamma_markets: gamma_markets.clone(),
                rows: pmxt_rows_for_token(&pmxt_rows, token)?,
            })
            .with_context(|| format!("project PMXT token {token}"))?;

            let mut bids: BTreeMap<Price, f64> = BTreeMap::new();
            let mut asks: BTreeMap<Price, f64> = BTreeMap::new();
            let mut became_priced = false;

            for delta in &projection.order_book_deltas {
                // Mirror OutcomeBookState::update_from_deltas exactly.
                match delta.action {
                    BookAction::Clear => {
                        bids.clear();
                        asks.clear();
                    }
                    BookAction::Delete => match delta.order.side {
                        OrderSide::Buy => {
                            bids.remove(&delta.order.price);
                        }
                        OrderSide::Sell => {
                            asks.remove(&delta.order.price);
                        }
                        _ => {}
                    },
                    BookAction::Add | BookAction::Update => {
                        let levels = match delta.order.side {
                            OrderSide::Buy => Some(&mut bids),
                            OrderSide::Sell => Some(&mut asks),
                            _ => None,
                        };
                        if let Some(levels) = levels {
                            if delta.order.size.is_zero() {
                                levels.remove(&delta.order.price);
                            } else {
                                levels.insert(delta.order.price, delta.order.size.as_f64());
                            }
                        }
                    }
                }
                if (delta.flags & last_flag) == 0 {
                    continue;
                }
                if let (Some((best_bid, _)), Some((best_ask, _))) =
                    (bids.iter().next_back(), asks.iter().next())
                {
                    became_priced = true;
                    assert!(
                        best_bid <= best_ask,
                        "token {token}: reconstructed book crossed (best_bid {best_bid} > best_ask {best_ask})"
                    );
                }
            }

            assert!(
                became_priced,
                "token {token}: reconstructed book never became two-sided; guard is vacuous"
            );
        }

        Ok(())
    }

    fn issue_789_gamma_markets() -> Result<Vec<GammaMarket>> {
        serde_json::from_str(&format!(
            r#"[{{
  "id": "2039137",
  "conditionId": "{condition}",
  "questionID": "0xd5167b495ac67974886a5875266d6a2c43b245f56bae3cd52d5756abe7b11c4b",
  "clobTokenIds": "[\"{up}\", \"{down}\"]",
  "outcomes": "[\"Up\", \"Down\"]",
  "question": "Bitcoin Up or Down - April 21, 8:00PM-8:05PM ET",
  "description": "Issue #789 real PMXT fixture market metadata reconstructed from Gamma for replay-time open state",
  "startDate": "2026-04-22T00:00:00Z",
  "endDate": "2026-04-22T00:05:00Z",
  "active": true,
  "closed": false,
  "acceptingOrders": true,
  "enableOrderBook": true,
  "orderPriceMinTickSize": 0.001,
  "orderMinSize": 5,
  "slug": "{slug}"
}}]"#,
            condition = ISSUE_789_CONDITION_ID,
            up = ISSUE_789_UP_TOKEN,
            down = ISSUE_789_DOWN_TOKEN,
            slug = ISSUE_789_MARKET_SLUG,
        ))
        .context("parse issue #789 Gamma market metadata")
    }

    fn reconstructed_chainlink_price_to_beat_table(
        okx_quotes: &CanonicalQuotesTable,
    ) -> Result<CanonicalIndexPricesTable> {
        // The price-to-beat (resolution strike) is the underlying price at the
        // market's interval open, fixed for the interval. Reconstruct it from
        // the real OKX BBO midpoint at the earliest in-window sample — never a
        // hardcoded literal. Mirrors `reconstructed_reference_rows_from_okx`.
        let open_quote = okx_quotes
            .rows
            .iter()
            .filter(|row| {
                u64::try_from(row.event_time / 1_000_000)
                    .map(|ms| (ISSUE_789_START_MS..ISSUE_789_END_MS).contains(&ms))
                    .unwrap_or(false)
            })
            .min_by_key(|row| row.event_time)
            .context("OKX fixture carries no quote at the issue #789 interval open")?;
        let open_bid = open_quote
            .bid
            .parse::<Decimal>()
            .context("parse OKX interval-open bid")?;
        let open_ask = open_quote
            .ask
            .parse::<Decimal>()
            .context("parse OKX interval-open ask")?;
        let price_to_beat = ((open_bid + open_ask) / Decimal::from(2))
            .normalize()
            .to_string();
        // Emit one strike row per second across the whole replay window so the
        // price-to-beat feed never ages out before the market resolves. The value
        // is constant (the interval-open strike); only the timestamps advance.
        let window_seconds = ((ISSUE_789_END_MS - ISSUE_789_START_MS) / 1_000) as i32;
        let rows = (0..window_seconds)
            .map(|second| {
                let ts = ISSUE_789_START_NS + i64::from(second) * 1_000_000_000;
                CanonicalIndexPriceRow {
                    schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
                    ingest_run_id: "issue-789-first-real-pl".to_string(),
                    source_binding:
                        "chainlink-price-to-beat-reconstructed-from-okx-interval-open-bbo"
                            .to_string(),
                    venue: "CHAINLINK".to_string(),
                    product_family: "index".to_string(),
                    product_category: "price_to_beat".to_string(),
                    instrument_id: "BTC-USD".to_string(),
                    canonical_instrument_key: "CHAINLINK/index/BTC-USD".to_string(),
                    venue_symbol: "BTC-USD".to_string(),
                    nt_instrument_id: Some("BTC-USD.CHAINLINK".to_string()),
                    event_time: ts,
                    capture_time: ts,
                    availability_time: None,
                    source_sequence: Some(format!("issue-789-reconstructed-strike-{second}")),
                    raw_payload_id: "issue-789-chainlink-reconstruction".to_string(),
                    source_proof_id: "issue-789-chainlink-reconstructed-strike".to_string(),
                    payload_hash: sha256_hex(format!("{price_to_beat}-{second}").as_bytes()),
                    transform_hash: sha256_hex(b"canonical-chainlink-reconstruction.v1"),
                    value: price_to_beat.clone(),
                }
            })
            .collect();
        Ok(CanonicalIndexPricesTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: TradesPartition {
                venue: "CHAINLINK".to_string(),
                product_family: "index".to_string(),
                product_category: "price_to_beat".to_string(),
                instrument_id: "BTC-USD".to_string(),
                dt: "2026-04-22".to_string(),
            },
            source_proof_id: "issue-789-chainlink-reconstructed-strike".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::IndexReplay,
            forbidden_claims: vec!["raw_chainlink_data_streams_replay".to_string()],
            transform_hash: sha256_hex(b"canonical-chainlink-reconstruction.v1"),
            payload_hash: sha256_hex(price_to_beat.as_bytes()),
            rows,
        })
    }

    fn reconstructed_reference_rows_from_okx(
        okx_quotes: &CanonicalQuotesTable,
    ) -> Result<Vec<ManifestReferenceCurrentPriceInput>> {
        let mut by_second = BTreeMap::new();
        for row in &okx_quotes.rows {
            let observed_ms = u64::try_from(row.event_time / 1_000_000)
                .context("OKX quote event_time before Unix epoch")?;
            if !(ISSUE_789_START_MS..ISSUE_789_END_MS).contains(&observed_ms) {
                continue;
            }
            by_second
                .entry((observed_ms - ISSUE_789_START_MS) / 1_000)
                .or_insert(row);
        }
        ensure!(
            by_second.len() >= 10,
            "OKX reference reconstruction needs at least 10 one-second samples"
        );
        by_second
            .into_values()
            .map(|row| {
                let bid = row.bid.parse::<Decimal>().context("parse OKX bid")?;
                let ask = row.ask.parse::<Decimal>().context("parse OKX ask")?;
                let midpoint = (bid + ask) / Decimal::from(2);
                let observed_ms = u64::try_from(row.event_time / 1_000_000)
                    .context("OKX reference observed_ms before Unix epoch")?;
                Ok(ManifestReferenceCurrentPriceInput {
                    client_id: "chainlink_reference".to_string(),
                    asset: "BTC".to_string(),
                    source_id: "chainlink_primary".to_string(),
                    provider: "chainlink_ws".to_string(),
                    provider_instrument: "BTC-USD.CHAINLINK".to_string(),
                    price: midpoint.normalize().to_string(),
                    bid: Some(row.bid.clone()),
                    ask: Some(row.ask.clone()),
                    observed_ts_ms: observed_ms,
                    received_ts_ms: observed_ms,
                    provenance: BTreeMap::from([
                        (
                            "fidelity".to_string(),
                            "reconstructed_from_okx_snapshot_seeded_l2_bbo".to_string(),
                        ),
                        ("raw_chainlink".to_string(), "false".to_string()),
                    ]),
                })
            })
            .collect()
    }

    struct Issue789Catalogs {
        okx_catalog: PathBuf,
        okx_catalog_hash: String,
        bybit_catalog: PathBuf,
        bybit_catalog_hash: String,
        chainlink_catalog: PathBuf,
        chainlink_catalog_hash: String,
        up_catalog: PathBuf,
        up_catalog_hash: String,
        up_instrument_id: String,
        down_catalog: PathBuf,
        down_catalog_hash: String,
        down_instrument_id: String,
        reference_rows: Vec<ManifestReferenceCurrentPriceInput>,
        instrument_settlements: Vec<ManifestInstrumentSettlementInput>,
    }

    fn issue_789_manifest(catalogs: Issue789Catalogs) -> Result<BacktestingRunManifest> {
        let catalog_hash = sha256_hex(
            format!(
                "{}{}{}{}{}",
                catalogs.okx_catalog_hash,
                catalogs.bybit_catalog_hash,
                catalogs.chainlink_catalog_hash,
                catalogs.up_catalog_hash,
                catalogs.down_catalog_hash
            )
            .as_bytes(),
        );
        let mut manifest = BacktestingRunManifest {
            manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
            run_id: "issue-789-first-real-free-data-taker-pl".to_string(),
            target_bolt_v2_branch: "codex/789-first-faithful-taker-pl".to_string(),
            target_bolt_v2_ref: "worktree".to_string(),
            resolved_nt_version:
                crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
                    .context("resolve BVS NautilusTrader dependency provenance")?,
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            venue_binding_key: "issue-789-pmxt-okx-bybit-chainlink".to_string(),
            run_purpose: RunPurpose::Normal,
            source_proof_id: "issue-789-first-real-free-data-slice".to_string(),
            source_proof_version: 1,
            pins_non_latest_proof: false,
            proof_pin_reason_code: None,
            proof_pin_reason_detail: None,
            strategy: StrategySource {
                source_kind: StrategySourceKind::CompiledRustRegistry,
                registry_key: STRATEGY_BINARY_ORACLE_EDGE_TAKER.to_string(),
                parameters: BTreeMap::from([
                    (STRATEGY_PARAM_FEE_BPS.to_string(), "0".to_string()),
                    (
                        STRATEGY_PARAM_ORDER_EXECUTION_MODE.to_string(),
                        "live".to_string(),
                    ),
                ]),
                typed_config_uri: None,
                typed_config_hash: None,
                experiment_result_uri: None,
                experiment_result_hash: None,
                config_overlay: Some(StrategyConfigOverlaySource {
                    production_root_config_path: "config/root.toml".to_string(),
                    override_delta: ManifestBacktestConfigOverride {
                        label: "production config + documented OKX/Bybit override".to_string(),
                        strategy_instance_id: "binary_oracle_btc".to_string(),
                        signal_role: "primary".to_string(),
                        signal_data_client_id: "okx_data".to_string(),
                        signal_instrument_id: "BTC-USDT.OKX".to_string(),
                        realized_volatility_surface_id: "btc_usdt_midpoint_rv".to_string(),
                        keep_realized_volatility_sources: vec![
                            ManifestRealizedVolatilitySourceSelector {
                                data_client_id: "okx_data".to_string(),
                                instrument_id: "BTC-USDT.OKX".to_string(),
                            },
                            ManifestRealizedVolatilitySourceSelector {
                                data_client_id: "bybit_data".to_string(),
                                instrument_id: "BTCUSDT-SPOT.BYBIT".to_string(),
                            },
                        ],
                    },
                }),
            },
            strategy_config_hash: "0".repeat(64),
            // POLYMARKET must be funded in the binary's settlement currency
            // (pUSD — the NT Polymarket adapter's collateral currency), not
            // USDC. NT's multi-currency portfolio manager refuses to auto-create
            // a balance for a negative realized PnL, so settling a held loser in
            // a currency the account was never funded in silently drops the P/L
            // from stats_pnls. The instrument's settlement currency owns this.
            venue: issue_789_venue("POLYMARKET", "pUSD", "L2_MBP", true, true),
            additional_venues: vec![
                issue_789_venue("OKX", "USDT", "L1_MBP", false, false),
                issue_789_venue("BYBIT", "USDT", "L1_MBP", false, false),
                issue_789_venue("CHAINLINK", "USD", "L1_MBP", false, false),
            ],
            catalog_inputs: vec![
                catalog_input(
                    &catalogs.okx_catalog,
                    "QuoteTick",
                    "BTC-USDT.OKX",
                    Some("okx_data"),
                ),
                catalog_input(
                    &catalogs.bybit_catalog,
                    "QuoteTick",
                    "BTCUSDT-SPOT.BYBIT",
                    Some("bybit_data"),
                ),
                catalog_input(
                    &catalogs.chainlink_catalog,
                    "IndexPriceUpdate",
                    "BTC-USD.CHAINLINK",
                    Some("chainlink_strike"),
                ),
                catalog_input(
                    &catalogs.up_catalog,
                    "OrderBookDelta",
                    &catalogs.up_instrument_id,
                    None,
                ),
                catalog_input(
                    &catalogs.up_catalog,
                    "TradeTick",
                    &catalogs.up_instrument_id,
                    None,
                ),
                catalog_input(
                    &catalogs.down_catalog,
                    "OrderBookDelta",
                    &catalogs.down_instrument_id,
                    None,
                ),
                catalog_input(
                    &catalogs.down_catalog,
                    "TradeTick",
                    &catalogs.down_instrument_id,
                    None,
                ),
            ],
            reconstructed_reference_current_price: catalogs.reference_rows,
            instrument_settlements: catalogs.instrument_settlements,
            catalog_hash,
            execution_model: "nt_backtest_node".to_string(),
            artifact_root: "memory://issue-789".to_string(),
            output_prefix: "issue-789-first-real-free-data-taker-pl".to_string(),
            artifact_store: ManifestArtifactStore {
                storage_options: BTreeMap::new(),
                rust_storage_options: BTreeMap::new(),
                ssm_parameters: None,
            },
            domain_metrics: Vec::new(),
            start_time: Some(ISSUE_789_START_NS),
            end_time: Some(ISSUE_789_END_NS),
        };
        let overlay = manifest
            .strategy
            .config_overlay
            .as_ref()
            .context("issue #789 manifest must carry its production config override")?;
        let production_root_config_path =
            resolve_existing_input_path(Path::new(&overlay.production_root_config_path));
        let loaded = load_bolt_v3_config(&production_root_config_path)
            .context("load issue #789 production config for canonical provenance")?;
        let override_spec = overlay.to_bolt_v3_override();
        let (loaded, _) = apply_backtest_config_override(loaded, &override_spec)
            .context("apply issue #789 override for canonical provenance")?;
        let loaded_strategy = loaded
            .strategies
            .iter()
            .find(|strategy| {
                strategy.config.strategy_instance_id == overlay.override_delta.strategy_instance_id
            })
            .context("issue #789 overlaid strategy is missing")?;
        let preparation_config = StrategyPreparationConfig::from_root(&loaded.root);
        let client_routes = prepare_strategy_client_routes(&loaded, loaded_strategy)
            .context("prepare configured strategy client routes")?;
        let raw = raw_taker_config(loaded_strategy, &preparation_config, &client_routes)
            .context("resolve issue #789 canonical taker config")?;
        let resolved_config_bytes = canonical_resolved_taker_config_bytes(
            &raw,
            Some(&loaded.config_bundle_checksum),
            Some(&override_spec),
        )?;
        manifest.strategy_config_hash = sha256_hex(&resolved_config_bytes);
        Ok(manifest)
    }

    fn issue_789_venue(
        nt_venue: &str,
        balance_currency: &str,
        book_type: &str,
        trade_execution: bool,
        liquidity_consumption: bool,
    ) -> ManifestVenueConfig {
        ManifestVenueConfig {
            nt_venue: nt_venue.to_string(),
            oms_type: "NETTING".to_string(),
            account_type: "CASH".to_string(),
            book_type: book_type.to_string(),
            starting_balances: vec![format!("1_000_000 {balance_currency}")],
            routing: false,
            frozen_account: false,
            reject_stop_orders: true,
            support_gtd_orders: true,
            support_contingent_orders: true,
            use_position_ids: true,
            use_random_ids: false,
            use_reduce_only: true,
            bar_execution: false,
            bar_adaptive_high_low_ordering: false,
            trade_execution,
            use_market_order_acks: false,
            liquidity_consumption,
            allow_cash_borrowing: false,
            queue_position: false,
            oto_trigger_mode: "PARTIAL".to_string(),
            base_currency: "NONE".to_string(),
            default_leverage: "1".to_string(),
            price_protection_points: 0,
            leverages: None,
            margin_model: None,
            modules: None,
            fill_model: None,
            latency_model: None,
            // This diagnostic fixture explicitly assumes zero commission. It is
            // not a claim that production Polymarket economics are zero-fee;
            // dynamic fee/rebate parity remains tracked by #843 item 10.
            fee_model: None,
            settlement_prices: None,
        }
    }

    fn catalog_input(
        catalog_path: &std::path::Path,
        data_type: &str,
        nt_instrument_id: &str,
        client_id: Option<&str>,
    ) -> ManifestCatalogInput {
        ManifestCatalogInput {
            catalog_path: catalog_path.display().to_string(),
            catalog_fs_protocol: CATALOG_FS_PROTOCOL_NONE.to_string(),
            catalog_fs_storage_options: BTreeMap::new(),
            catalog_fs_rust_storage_options: BTreeMap::new(),
            data_type: data_type.to_string(),
            nt_instrument_id: nt_instrument_id.to_string(),
            instrument_ids: None,
            start_time: Some(ISSUE_789_START_NS),
            end_time: Some(ISSUE_789_END_NS),
            filter_expr: None,
            client_id: client_id.map(str::to_string),
            metadata: None,
            bar_spec: None,
            bar_types: None,
            optimize_file_loading: None,
        }
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hex::encode(hasher.finalize())
    }
}
