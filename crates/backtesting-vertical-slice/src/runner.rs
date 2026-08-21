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
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, bail, ensure};
use bolt_v2::{
    ReferencePriceUpdate, ReferenceQuoteProvenance,
    bolt_v3_config::{
        BacktestConfigOverrideReport, LoadedBoltV3Config, LoadedStrategy,
        apply_backtest_config_override, load_bolt_v3_config,
    },
    bolt_v3_current_evidence::{
        AdmissionDecisionOutcome, BacktestRunGuardEvent, CurrentEvidenceStream,
        OfflineDecisionEvidenceRuntime, PositiveFiniteEvidenceReadCap, StrategyInputDetails,
        StrategyInputRvState, SubmitAdmissionEvidence, read_backtest_run_guard_events,
    },
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        authoritative_economics_input_from_replay, bind_execution_economics,
        configured_provider_exact_replay_from_unit,
    },
    bolt_v3_operator_artifacts::json_artifact_bytes,
    bolt_v3_order_execution::{
        BoltV3OrderEconomicsHandle, BoltV3OrderExecutionMode, BoltV3OrderExecutionPolicy,
    },
    bolt_v3_position_authority_feed::BoltV3PositionAuthorityRuntime,
    bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime,
    bolt_v3_strategy_context::{StrategyBuildContext, StrategyDecisionEvidence},
    bolt_v3_strategy_registration::{
        StrategyPreparationConfig, bind_position_authority_capability,
        prepare_position_authority_runtime, prepare_strategy_client_routes,
        register_prepared_strategy_batch,
    },
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    economics::{CurrencyId, SnapshotId, SourceIdentity},
    strategies::binary_oracle_edge_taker::archetype::raw_taker_config,
    strategies::production_strategy_registry,
};
use nautilus_analysis::analyzer::PortfolioAnalyzer;
use nautilus_backtest::{engine::BacktestEngine, node::BacktestNode, result::BacktestResult};
use nautilus_core::{UUID4, UnixNanos};
#[cfg(test)]
use nautilus_model::orderbook::OrderBook;
use nautilus_model::{
    accounts::AccountAny,
    data::{
        Bar, BarSpecification, Data, FundingRateUpdate, IndexPriceUpdate, InstrumentClose,
        MarkPriceUpdate, OrderBookDelta, QuoteTick, TradeTick,
    },
    enums::{
        AccountType, AggregationSource, AggressorSide, BookAction, InstrumentCloseType,
        LiquiditySide, OrderSide, OrderStatus, OrderType, PriceType,
    },
    events::OrderEventAny,
    identifiers::{
        AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, TradeId,
        TraderId, Venue, VenueOrderId,
    },
    orders::Order,
    position::Position,
    types::{AccountBalance, Currency, MarginBalance, Money, Price, Quantity},
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
        CatalogInstrumentSpecSource, CatalogProjection, DeltaReplayClock,
        native_order_book_sequence, order_book_delta_replay_times,
        project_canonical_trades_to_catalog, read_back_trade_ticks, ts_event_nanos, ts_init_nanos,
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
        BacktestingRunManifest, ManifestEconomicsAuthorityValue,
        ManifestEconomicsValuationObservation, NtSurfaceClassification,
        STRATEGY_BINARY_ORACLE_EDGE_TAKER, STRATEGY_BINARY_ORACLE_MAKER,
        STRATEGY_HURST_VPIN_DIRECTIONAL, STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE,
        STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES, STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT,
        STRATEGY_PARAM_ORDER_EXECUTION_MODE, StrategySource,
    },
    seeded_l2_quote_bridge::{
        SeededL2QuoteBridgePlan, SeededL2QuoteBridgeReport, install_seeded_l2_quote_bridge,
    },
    source_proof::{AcceptedDataset, SourceProofFidelityClass},
};

/// Strategy parameter key for the bar type.
const PARAM_BAR_TYPE: &str = "bar_type";
/// Strategy parameter key for the trade size.
const PARAM_TRADE_SIZE: &str = "trade_size";
/// Strategy parameter key for the normalized binary-oracle builder TOML.
const PARAM_CONFIG_TOML: &str = "config_toml";

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
    latest_strategy_input_snapshot: Option<BacktestStrategyInputSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BacktestStrategyInputSnapshot {
    recorded_at_utc_ns: i64,
    purpose: BacktestStrategyInputPurpose,
    market_id: Option<String>,
    spot_price: Option<String>,
    reference_current_price: Option<String>,
    reference_current_price_source_id: Option<String>,
    price_to_beat_value: Option<String>,
    realized_volatility: StrategyInputRvState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BacktestStrategyInputPurpose {
    BlockedObservation,
    SubmitLinked,
}

trait IntoOptionalEvidenceText {
    fn into_optional_evidence_text(self) -> Option<String>;
}

impl IntoOptionalEvidenceText for String {
    fn into_optional_evidence_text(self) -> Option<String> {
        Some(self)
    }
}

impl IntoOptionalEvidenceText for Option<String> {
    fn into_optional_evidence_text(self) -> Option<String> {
        self
    }
}

impl BacktestDecisionEvidenceState {
    fn observe_strategy_input<PurposeNumeric>(
        &mut self,
        recorded_at_utc_ns: i64,
        purpose: BacktestStrategyInputPurpose,
        details: StrategyInputDetails<PurposeNumeric>,
    ) -> Result<()>
    where
        PurposeNumeric: IntoOptionalEvidenceText,
    {
        self.strategy_input_snapshot_count += 1;
        let candidate = BacktestStrategyInputSnapshot {
            recorded_at_utc_ns,
            purpose,
            market_id: details.market_id,
            spot_price: details.spot_price.into_optional_evidence_text(),
            reference_current_price: details.reference_current_price,
            reference_current_price_source_id: details.reference_current_price_source_id,
            price_to_beat_value: details.price_to_beat_value.into_optional_evidence_text(),
            realized_volatility: details.realized_volatility,
        };
        match &self.latest_strategy_input_snapshot {
            None => self.latest_strategy_input_snapshot = Some(candidate),
            Some(current) => match candidate
                .recorded_at_utc_ns
                .cmp(&current.recorded_at_utc_ns)
            {
                std::cmp::Ordering::Less => {}
                std::cmp::Ordering::Equal => {
                    ensure!(
                        current == &candidate,
                        "conflicting strategy-input facts share recorded_at_utc_ns {}",
                        candidate.recorded_at_utc_ns
                    );
                }
                std::cmp::Ordering::Greater => {
                    self.latest_strategy_input_snapshot = Some(candidate);
                }
            },
        }
        Ok(())
    }
}

#[derive(Debug)]
struct BacktestDecisionEvidenceWriter {
    runtime: OfflineDecisionEvidenceRuntime,
    machine: PathBuf,
    observation: PathBuf,
    read_max_bytes: PositiveFiniteEvidenceReadCap,
    _catalog: tempfile::TempDir,
}

const BACKTEST_MACHINE_EVIDENCE_FILE: &str = "machine.jsonl";
const BACKTEST_OBSERVATION_EVIDENCE_FILE: &str = "observation.jsonl";

impl BacktestDecisionEvidenceWriter {
    fn new(
        reject_episode_max_count: usize,
        read_max_bytes: PositiveFiniteEvidenceReadCap,
    ) -> Result<Self> {
        ensure!(
            reject_episode_max_count > 0,
            "strategy parameter {STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT} must be positive"
        );
        let catalog =
            tempfile::tempdir().context("create isolated backtest current-evidence catalog")?;
        let canonical_catalog = std::fs::canonicalize(catalog.path())
            .context("canonicalize isolated backtest current-evidence catalog")?;
        let machine_relative = BACKTEST_MACHINE_EVIDENCE_FILE;
        let observation_relative = BACKTEST_OBSERVATION_EVIDENCE_FILE;
        let machine = canonical_catalog.join(machine_relative);
        let observation = canonical_catalog.join(observation_relative);
        let runtime = OfflineDecisionEvidenceRuntime::open_isolated(
            &canonical_catalog,
            machine_relative,
            observation_relative,
            read_max_bytes,
            reject_episode_max_count,
        )?;
        Ok(Self {
            runtime,
            machine,
            observation,
            read_max_bytes,
            _catalog: catalog,
        })
    }

    fn submit_admission_evidence(&self) -> SubmitAdmissionEvidence {
        self.runtime.submit_admission_evidence()
    }

    fn strategy_evidence(&self, registry_key: &str) -> Result<StrategyDecisionEvidence> {
        match registry_key {
            STRATEGY_BINARY_ORACLE_EDGE_TAKER => Ok(StrategyDecisionEvidence::edge_taker(
                self.runtime.edge_taker_evidence(),
                self.runtime.order_execution_evidence(),
            )),
            STRATEGY_BINARY_ORACLE_MAKER => Ok(StrategyDecisionEvidence::maker(
                self.runtime.maker_evidence(),
                self.runtime.order_execution_evidence(),
            )),
            _ => bail!("strategy {registry_key:?} owns no decision-evidence capability"),
        }
    }

    fn state(&self) -> Result<BacktestDecisionEvidenceState> {
        let mut state = BacktestDecisionEvidenceState::default();
        for (stream, kind) in [
            (&self.machine, CurrentEvidenceStream::Machine),
            (&self.observation, CurrentEvidenceStream::Observation),
        ] {
            for record in read_backtest_run_guard_events(stream, self.read_max_bytes, kind)? {
                match record.event {
                    BacktestRunGuardEvent::BlockedStrategyInputObservation(fact) => {
                        state.observe_strategy_input(
                            record.recorded_at_utc_ns,
                            BacktestStrategyInputPurpose::BlockedObservation,
                            fact.details,
                        )?;
                    }
                    BacktestRunGuardEvent::SubmitLinkedStrategyInputSnapshot(fact) => {
                        state.observe_strategy_input(
                            record.recorded_at_utc_ns,
                            BacktestStrategyInputPurpose::SubmitLinked,
                            fact.details,
                        )?;
                    }
                    BacktestRunGuardEvent::EntryOrderIntent(_) => state.order_intent_count += 1,
                    BacktestRunGuardEvent::AdmittedEntryAdmission(fact) => {
                        state.admission_decision_count += 1;
                        state.admitted_order_count += 1;
                        state.submit_reservation_count += u64::from(fact.reservation.is_some());
                    }
                    BacktestRunGuardEvent::RejectedEntryAdmission(_) => {
                        state.admission_decision_count += 1;
                    }
                    BacktestRunGuardEvent::RiskReducingExitAdmission(fact) => {
                        state.admission_decision_count += 1;
                        if matches!(fact.outcome, AdmissionDecisionOutcome::Admitted) {
                            state.admitted_order_count += 1;
                        }
                    }
                    BacktestRunGuardEvent::ForcedReductionAdmission(fact) => {
                        state.admission_decision_count += 1;
                        if matches!(fact.outcome, AdmissionDecisionOutcome::Admitted) {
                            state.admitted_order_count += 1;
                        }
                    }
                    BacktestRunGuardEvent::BasketAdmissionGranted(fact) => {
                        state.submit_reservation_count += u64::try_from(
                            fact.admitted_legs
                                .iter()
                                .filter(|leg| leg.reservation.is_some())
                                .count(),
                        )
                        .context("basket reservation count must fit u64")?;
                    }
                    BacktestRunGuardEvent::SubmitReservationFill(_) => {
                        state.submit_fill_count += 1;
                    }
                    BacktestRunGuardEvent::EntrySkipObservation(_) => state.entry_skip_count += 1,
                    BacktestRunGuardEvent::ExitIntentDecision(_)
                    | BacktestRunGuardEvent::ExitHoldDecision(_) => {
                        state.exit_decision_count += 1;
                    }
                    BacktestRunGuardEvent::ExitPreparedOrder(_) => {}
                    BacktestRunGuardEvent::LossGovernorHalt(_) => {
                        state.loss_governor_halt_count += 1;
                    }
                    BacktestRunGuardEvent::RequoteThrottleObservation(_) => {
                        state.requote_throttle_count += 1;
                    }
                }
            }
        }
        Ok(state)
    }

    fn run_guard_report(&self, result: &BacktestResult) -> Result<BacktestRunGuardReport> {
        let state = self.state()?;
        let latest = state.latest_strategy_input_snapshot.as_ref();
        let signal_quote_received = latest.is_some_and(|snapshot| {
            snapshot
                .spot_price
                .as_deref()
                .is_some_and(positive_decimal_text)
        });
        let realized_volatility_ready =
            latest.is_some_and(|snapshot| match &snapshot.realized_volatility {
                StrategyInputRvState::Absent { .. } => false,
                StrategyInputRvState::Present {
                    selected_annualized_decimal,
                    snapshot,
                    ..
                } => {
                    selected_annualized_decimal
                        .as_deref()
                        .is_some_and(positive_decimal_text)
                        && snapshot.as_of_ms.is_some()
                        && snapshot.blockers.is_empty()
                }
            });
        let price_to_beat_received = latest.is_some_and(|snapshot| {
            snapshot
                .price_to_beat_value
                .as_deref()
                .is_some_and(positive_decimal_text)
        });
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
            latest_spot_price: latest.and_then(|snapshot| snapshot.spot_price.clone()),
            latest_reference_current_price: latest
                .and_then(|snapshot| snapshot.reference_current_price.clone()),
            latest_reference_current_price_source_id: latest
                .and_then(|snapshot| snapshot.reference_current_price_source_id.clone()),
            latest_price_to_beat_value: latest
                .and_then(|snapshot| snapshot.price_to_beat_value.clone()),
            latest_realized_volatility_as_of_ms: latest.and_then(|snapshot| {
                match &snapshot.realized_volatility {
                    StrategyInputRvState::Absent { .. } => None,
                    StrategyInputRvState::Present { snapshot, .. } => snapshot.as_of_ms,
                }
            }),
            latest_realized_volatility_sources_used: latest.map_or_else(Vec::new, |snapshot| {
                match &snapshot.realized_volatility {
                    StrategyInputRvState::Absent { .. } => Vec::new(),
                    StrategyInputRvState::Present { snapshot, .. } => snapshot.sources_used.clone(),
                }
            }),
            latest_realized_volatility_blockers: latest.map_or_else(Vec::new, |snapshot| {
                match &snapshot.realized_volatility {
                    StrategyInputRvState::Absent { .. } => Vec::new(),
                    StrategyInputRvState::Present { snapshot, .. } => snapshot
                        .blockers
                        .iter()
                        .map(|blocker| format!("{blocker:?}"))
                        .collect(),
                }
            }),
            did_not_arm_reason,
        })
    }
}

fn positive_decimal_text(value: &str) -> bool {
    value
        .trim()
        .parse::<Decimal>()
        .is_ok_and(|value| value > Decimal::ZERO)
}

struct DidNotArmReasonInputs<'a> {
    latest: Option<&'a BacktestStrategyInputSnapshot>,
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
        let blockers = match &snapshot.realized_volatility {
            StrategyInputRvState::Absent { .. } => String::new(),
            StrategyInputRvState::Present { snapshot, .. } => snapshot
                .blockers
                .iter()
                .map(|blocker| format!("{blocker:?}"))
                .collect::<Vec<_>>()
                .join(","),
        };
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
    _position_authority_runtime: Option<BoltV3PositionAuthorityRuntime>,
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
) -> Result<BoltV3OrderExecutionMode> {
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
    Ok(order_execution_mode)
}

fn load_manifest_economics_config(manifest: &BacktestingRunManifest) -> Result<LoadedBoltV3Config> {
    let economics = manifest
        .economics
        .as_ref()
        .context("binary-oracle replay requires manifest economics authority")?;
    let production_root_config_path =
        resolve_existing_input_path(Path::new(&economics.production_root_config_path));
    let loaded = load_bolt_v3_config(&production_root_config_path).with_context(|| {
        format!(
            "load production economics config root {}",
            economics.production_root_config_path
        )
    })?;
    anyhow::ensure!(
        loaded.config_bundle_checksum == economics.production_config_bundle_checksum,
        "production economics config bundle checksum mismatch: expected={} actual={} path={}",
        economics.production_config_bundle_checksum,
        loaded.config_bundle_checksum,
        economics.production_root_config_path
    );
    Ok(loaded)
}

fn raw_execution_client_id(raw_config: &toml::Value) -> Result<&str> {
    raw_config
        .get("client_id")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .context("binary-oracle config client_id is required for execution economics")
}

fn raw_oms_type(raw_config: &toml::Value) -> Result<nautilus_model::enums::OmsType> {
    raw_config
        .get("oms_type")
        .and_then(toml::Value::as_str)
        .context("binary-oracle config oms_type is required for position authority")?
        .parse()
        .context("binary-oracle config oms_type is invalid for position authority")
}

fn manifest_order_economics(
    manifest: &BacktestingRunManifest,
    loaded: &LoadedBoltV3Config,
    raw_config: &toml::Value,
) -> Result<BoltV3OrderEconomicsHandle> {
    let economics = manifest
        .economics
        .as_ref()
        .context("binary-oracle replay requires manifest economics authority")?;
    let execution_client_id = raw_execution_client_id(raw_config)?;
    let inputs = economics
        .inputs
        .iter()
        .map(|input| {
            let authority = manifest_authority_table(&input.authority);
            let authority = authoritative_economics_input_from_replay(
                loaded,
                execution_client_id,
                &input.instrument_id,
                &input.product_surface_id,
                &authority,
            )
            .map_err(anyhow::Error::from)?;
            let observations = input
                .valuation_observations
                .iter()
                .map(|observation| {
                    manifest_valuation_observation(loaded, execution_client_id, observation)
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(authority.with_valuation_observations(observations))
        })
        .collect::<Result<Vec<_>>>()?;
    let inputs = AuthoritativeEconomicsInputStore::try_new(inputs)
        .map_err(anyhow::Error::from)
        .context("build replay authoritative economics store")?;
    let economics = bind_execution_economics(loaded, execution_client_id, &inputs)
        .map_err(anyhow::Error::from)
        .context("bind replay execution economics")?;
    Ok(BoltV3OrderEconomicsHandle::new(economics))
}

fn attach_position_authority(
    build_context: StrategyBuildContext,
    loaded: &LoadedBoltV3Config,
    raw_config: &toml::Value,
    engine: &BacktestEngine,
) -> Result<(StrategyBuildContext, BoltV3PositionAuthorityRuntime)> {
    let runtime = prepare_position_authority_runtime(loaded, engine.kernel().cache())
        .context("prepare shared position-authority runtime")?;
    let execution_client_id = ClientId::from(raw_execution_client_id(raw_config)?);
    let capability = bind_position_authority_capability(
        loaded,
        &runtime,
        execution_client_id,
        raw_oms_type(raw_config)?,
    )
    .context("bind replay strategy position-authority capability")?;
    Ok((build_context.with_position_authority(capability), runtime))
}

fn manifest_authority_table(
    table: &BTreeMap<String, ManifestEconomicsAuthorityValue>,
) -> toml::Value {
    toml::Value::Table(
        table
            .iter()
            .map(|(key, value)| (key.clone(), manifest_authority_value(value)))
            .collect(),
    )
}

fn manifest_authority_value(value: &ManifestEconomicsAuthorityValue) -> toml::Value {
    match value {
        ManifestEconomicsAuthorityValue::String(value) => toml::Value::String(value.clone()),
        ManifestEconomicsAuthorityValue::Integer(value) => toml::Value::Integer(*value),
        ManifestEconomicsAuthorityValue::Boolean(value) => toml::Value::Boolean(*value),
        ManifestEconomicsAuthorityValue::Array(values) => {
            toml::Value::Array(values.iter().map(manifest_authority_value).collect())
        }
        ManifestEconomicsAuthorityValue::Table(values) => toml::Value::Table(
            values
                .iter()
                .map(|(key, value)| (key.clone(), manifest_authority_value(value)))
                .collect(),
        ),
    }
}

fn manifest_valuation_observation(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
    observation: &ManifestEconomicsValuationObservation,
) -> Result<AuthoritativeValuationObservation> {
    match observation {
        ManifestEconomicsValuationObservation::MarketQuote {
            client_id,
            instrument_id,
            base_currency,
            quote_currency,
            price,
            snapshot_id,
            observed_at_ns,
            fetched_at_ns,
            valid_until_ns,
        } => Ok(AuthoritativeValuationObservation::MarketQuote {
            client_id: client_id.clone(),
            instrument_id: instrument_id.clone(),
            base_currency: CurrencyId::try_new(base_currency.clone())
                .map_err(anyhow::Error::from)?,
            quote_currency: CurrencyId::try_new(quote_currency.clone())
                .map_err(anyhow::Error::from)?,
            price: Decimal::from_str(price)
                .with_context(|| format!("invalid economics valuation price {price:?}"))?,
            snapshot_id: SnapshotId::try_new(snapshot_id.clone()).map_err(anyhow::Error::from)?,
            observed_at_ns: *observed_at_ns,
            fetched_at_ns: *fetched_at_ns,
            valid_until_ns: *valid_until_ns,
        }),
        ManifestEconomicsValuationObservation::ProviderExactConversion {
            source_id,
            from_unit,
            to_unit,
            snapshot_id,
            observed_at_ns,
            fetched_at_ns,
            valid_until_ns,
        } => Ok(AuthoritativeValuationObservation::ProviderExactConversion {
            source_id: SourceIdentity::try_new(source_id.clone()).map_err(anyhow::Error::from)?,
            from_unit: configured_provider_exact_replay_from_unit(
                loaded,
                execution_client_id,
                source_id,
                from_unit,
                to_unit,
            )
            .map_err(anyhow::Error::from)?,
            to_unit: CurrencyId::try_new(to_unit.clone()).map_err(anyhow::Error::from)?,
            snapshot_id: SnapshotId::try_new(snapshot_id.clone()).map_err(anyhow::Error::from)?,
            observed_at_ns: *observed_at_ns,
            fetched_at_ns: *fetched_at_ns,
            valid_until_ns: *valid_until_ns,
        }),
    }
}

fn manifest_evidence_reject_episode_max_count(strategy: &StrategySource) -> Result<usize> {
    let raw = strategy
        .parameters
        .get(STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT)
        .with_context(|| {
            format!(
                "strategy parameter {STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT} is required"
            )
        })?;
    let value = raw.parse::<usize>().with_context(|| {
        format!("invalid {STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT} {raw:?}")
    })?;
    ensure!(
        value > 0,
        "strategy parameter {STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT} must be positive"
    );
    Ok(value)
}

fn manifest_evidence_read_max_bytes(
    strategy: &StrategySource,
) -> Result<PositiveFiniteEvidenceReadCap> {
    let raw = strategy
        .parameters
        .get(STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES)
        .with_context(|| {
            format!("strategy parameter {STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES} is required")
        })?;
    let value = raw
        .parse::<u64>()
        .with_context(|| format!("invalid {STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES} {raw:?}"))?;
    PositiveFiniteEvidenceReadCap::new(value).map_err(|message| {
        anyhow::Error::msg(format!(
            "strategy parameter {STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES} {message}"
        ))
    })
}

fn inline_manifest_strategy_config(strategy: &StrategySource) -> Result<toml::Value> {
    let raw_config = strategy
        .parameters
        .get(PARAM_CONFIG_TOML)
        .with_context(|| format!("strategy parameter {PARAM_CONFIG_TOML} is required"))?;
    toml::from_str::<toml::Value>(raw_config)
        .with_context(|| format!("invalid {PARAM_CONFIG_TOML}"))
}

fn register_manifest_binary_oracle_maker(
    engine: &mut BacktestEngine,
    manifest: &BacktestingRunManifest,
    loaded: &LoadedBoltV3Config,
    raw_config: &toml::Value,
    order_economics: BoltV3OrderEconomicsHandle,
    order_execution_mode: BoltV3OrderExecutionMode,
) -> Result<(
    Arc<BacktestDecisionEvidenceWriter>,
    BoltV3PositionAuthorityRuntime,
)> {
    let run_guard_writer = Arc::new(BacktestDecisionEvidenceWriter::new(
        manifest_evidence_reject_episode_max_count(&manifest.strategy)?,
        manifest_evidence_read_max_bytes(&manifest.strategy)?,
    )?);
    let submit_admission = Arc::new(BoltV3SubmitAdmissionState::new(
        run_guard_writer.submit_admission_evidence(),
    ));
    let build_context = StrategyBuildContext::new(
        order_economics,
        run_guard_writer.strategy_evidence(STRATEGY_BINARY_ORACLE_MAKER)?,
        submit_admission,
        BoltV3OrderExecutionPolicy::from_mode(order_execution_mode),
        Venue::from(manifest.venue.nt_venue.as_str()),
    );
    let (build_context, position_authority_runtime) =
        attach_position_authority(build_context, loaded, raw_config, engine)?;
    let registry = production_strategy_registry().context("build production strategy registry")?;
    let prepared = registry
        .prepare_strategy(STRATEGY_BINARY_ORACLE_MAKER, raw_config, &build_context)
        .context("prepare binary_oracle_maker strategy through production registry")?;
    register_prepared_strategy_batch(engine.kernel().trader(), vec![prepared])
        .context("register binary_oracle_maker prepared strategy batch")?;
    Ok((run_guard_writer, position_authority_runtime))
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
            let order_execution_mode = manifest_binary_oracle_execution_controls(strategy)?;
            let loaded = load_manifest_economics_config(manifest)?;
            let (
                loaded,
                raw_config,
                config_override_report,
                realized_volatility_runtime,
                resolved_config_bytes,
            ) = if let Some(overlay) = &strategy.config_overlay {
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
                    loaded,
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
                (loaded, raw_config, None, None, resolved_config_bytes)
            };
            let run_guard_writer = Arc::new(BacktestDecisionEvidenceWriter::new(
                manifest_evidence_reject_episode_max_count(strategy)?,
                manifest_evidence_read_max_bytes(strategy)?,
            )?);
            let resolved_config_hash = sha256_hex(&resolved_config_bytes);
            let submit_admission = Arc::new(BoltV3SubmitAdmissionState::new(
                run_guard_writer.submit_admission_evidence(),
            ));
            let order_economics = manifest_order_economics(manifest, &loaded, &raw_config)?;
            let build_context = StrategyBuildContext::new(
                order_economics,
                run_guard_writer.strategy_evidence(STRATEGY_BINARY_ORACLE_EDGE_TAKER)?,
                submit_admission,
                BoltV3OrderExecutionPolicy::from_mode(order_execution_mode),
                Venue::from(manifest.venue.nt_venue.as_str()),
            );
            let (mut build_context, position_authority_runtime) =
                attach_position_authority(build_context, &loaded, &raw_config, engine)?;
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
                _position_authority_runtime: Some(position_authority_runtime),
            })
        }
        STRATEGY_BINARY_ORACLE_MAKER => {
            ensure!(
                strategy.config_overlay.is_none(),
                "strategy.config_overlay is not supported for strategy {STRATEGY_BINARY_ORACLE_MAKER:?}"
            );
            let order_execution_mode = manifest_binary_oracle_execution_controls(strategy)?;
            let raw_config = inline_manifest_strategy_config(strategy)?;
            let loaded = load_manifest_economics_config(manifest)?;
            let order_economics = manifest_order_economics(manifest, &loaded, &raw_config)?;
            let (run_guard_writer, position_authority_runtime) =
                register_manifest_binary_oracle_maker(
                    engine,
                    manifest,
                    &loaded,
                    &raw_config,
                    order_economics,
                    order_execution_mode,
                )?;
            Ok(AddedManifestStrategy {
                config_override_report: None,
                run_guard_writer: Some(run_guard_writer),
                resolved_config_hash: None,
                resolved_config_bytes: None,
                _position_authority_runtime: Some(position_authority_runtime),
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
    pub trader_id: TraderId,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub client_order_id: ClientOrderId,
    pub account_id: Option<AccountId>,
    pub venue_order_id: Option<VenueOrderId>,
    pub position_id: Option<PositionId>,
    pub order_side: OrderSide,
    pub order_type: OrderType,
    pub status: OrderStatus,
    pub quantity: Quantity,
    pub filled_qty: Quantity,
    pub leaves_qty: Quantity,
    pub initialized_quantity: Quantity,
    pub initialized_quote_quantity: bool,
    pub current_quote_quantity: bool,
    pub trade_ids: Vec<TradeId>,
    pub commissions: Vec<(Currency, Money)>,
    pub fills: Vec<Issue789ProofFill>,
    pub events_debug: Vec<String>,
}

/// Proof-relevant `OrderFilled` fields. Transport timestamps, capture-time
/// position attribution, opaque info, and causation metadata are deliberately
/// outside the economic proof boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue789ProofFill {
    pub trader_id: TraderId,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub client_order_id: ClientOrderId,
    pub venue_order_id: VenueOrderId,
    pub account_id: AccountId,
    pub trade_id: TradeId,
    pub order_side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub price: Price,
    pub currency: Currency,
    pub liquidity_side: LiquiditySide,
    pub event_id: UUID4,
    pub reconciliation: bool,
    pub commission: Option<Money>,
}

fn issue_789_proof_fill(fill: &nautilus_model::events::OrderFilled) -> Issue789ProofFill {
    Issue789ProofFill {
        trader_id: fill.trader_id,
        strategy_id: fill.strategy_id,
        instrument_id: fill.instrument_id,
        client_order_id: fill.client_order_id,
        venue_order_id: fill.venue_order_id,
        account_id: fill.account_id,
        trade_id: fill.trade_id,
        order_side: fill.order_side,
        order_type: fill.order_type,
        quantity: fill.last_qty,
        price: fill.last_px,
        currency: fill.currency,
        liquidity_side: fill.liquidity_side,
        event_id: fill.event_id,
        reconciliation: fill.reconciliation,
        commission: fill.commission,
    }
}

/// Current account state captured directly from the post-run cache.
///
/// This is deliberately not built from `AccountAny::last_event`: the final
/// event-store entry and the live cache are independent completeness surfaces.
#[derive(Debug, Clone)]
pub struct AccountTerminalRecord {
    pub account_id: AccountId,
    pub account_type: AccountType,
    pub base_currency: Option<Currency>,
    pub balances: Vec<AccountBalance>,
    pub cash_locks: Vec<(InstrumentId, Currency, Money)>,
    pub margins: Vec<MarginBalance>,
}

/// Result of one `BacktestNode` run: the NautilusTrader summary plus the
/// terminal state of every order in the post-run cache.
pub struct NtBacktestNodeRun {
    pub result: BacktestResult,
    pub order_terminals: Vec<OrderTerminalRecord>,
    pub config_override_report: Option<BacktestConfigOverrideReport>,
    pub run_guard_report: Option<BacktestRunGuardReport>,
    pub positions: Vec<Position>,
    pub configured_execution_account_id: AccountId,
    pub account_terminals: Vec<AccountTerminalRecord>,
    pub resolved_config_hash: Option<String>,
    pub resolved_config_bytes: Option<Vec<u8>>,
    pub execution_contract_report: Option<crate::execution_contract::ExecutionContractReport>,
}

fn require_pre_run_configured_account(
    account_id: Option<AccountId>,
    venue: Venue,
) -> Result<AccountId> {
    account_id.with_context(|| {
        format!("built backtest engine has no configured execution account for {venue}")
    })
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
    run_nt_backtest_node_with_hooks(manifest, |_| Ok(()), |_| Ok(())).map(|(output, ())| output)
}

pub(crate) fn run_nt_backtest_node_with_optional_seeded_l2_quote_bridge(
    manifest: &BacktestingRunManifest,
    plan: Option<SeededL2QuoteBridgePlan>,
) -> Result<(NtBacktestNodeRun, Option<SeededL2QuoteBridgeReport>)> {
    run_nt_backtest_node_with_hooks(
        manifest,
        |engine| {
            plan.map(|plan| install_seeded_l2_quote_bridge(engine, plan))
                .transpose()
        },
        |capture| capture.map(|capture| capture.finalize()).transpose(),
    )
}

fn run_nt_backtest_node_with_hooks<C, E, B, A>(
    manifest: &BacktestingRunManifest,
    before_run: B,
    after_run: A,
) -> Result<(NtBacktestNodeRun, E)>
where
    B: FnOnce(&mut BacktestEngine) -> Result<C>,
    A: FnOnce(C) -> Result<E>,
{
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
    let configured_execution_account_id = {
        let engine = node
            .get_engine(&manifest.run_id)
            .with_context(|| format!("no engine for run id {} before run", manifest.run_id))?;
        let venue = Venue::from(manifest.venue.nt_venue.as_str());
        let exec_engine = engine.kernel().exec_engine.borrow();
        let matching_account_ids = exec_engine
            .get_all_clients()
            .into_iter()
            .filter(|client| client.venue() == venue)
            .map(|client| client.account_id())
            .collect::<Vec<_>>();
        ensure!(
            matching_account_ids.len() <= 1,
            "built backtest engine has multiple configured execution accounts for {venue}"
        );
        require_pre_run_configured_account(matching_account_ids.first().copied(), venue)?
    };
    let capture = {
        let engine = node
            .get_engine_mut(&manifest.run_id)
            .with_context(|| format!("no engine for run id {} before run", manifest.run_id))?;
        before_run(engine)?
    };
    let run_result = node.run();
    let evidence_result = after_run(capture);
    let mut results = run_result.context("run BacktestNode")?;
    let evidence = evidence_result.context("finalize BacktestNode execution evidence")?;
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
    let (order_terminals, positions, account_terminals) = {
        let engine = node
            .get_engine(&manifest.run_id)
            .with_context(|| format!("no engine for run id {} after run", manifest.run_id))?;
        let (positions, account_terminals): (Vec<_>, Vec<_>) = {
            let cache = engine.kernel().cache.borrow();
            let positions = cache
                .positions(None, None, None, None, None)
                .into_iter()
                .map(|position| position.cloned())
                .collect();
            let account_terminals = std::iter::once(&manifest.venue)
                .chain(manifest.additional_venues.iter())
                .filter_map(|venue| cache.account_for_venue(&Venue::from(venue.nt_venue.as_str())))
                .map(|account| capture_account_terminal(account.as_ref()))
                .collect();
            (positions, account_terminals)
        };
        domain_analyzer.add_positions(&positions);
        for (name, value) in domain_statistics_from_analyzer(&domain_analyzer, &domain_statistics) {
            nt_result.stats_general.insert(name, value);
        }
        (
            capture_order_terminals(engine)?,
            positions,
            account_terminals,
        )
    };
    let run_guard_report = added_strategy
        .run_guard_writer
        .as_ref()
        .map(|writer| writer.run_guard_report(&nt_result))
        .transpose()?;
    Ok((
        NtBacktestNodeRun {
            result: nt_result,
            order_terminals,
            config_override_report: added_strategy.config_override_report,
            run_guard_report,
            positions,
            configured_execution_account_id,
            account_terminals,
            resolved_config_hash: added_strategy.resolved_config_hash,
            resolved_config_bytes: added_strategy.resolved_config_bytes,
            execution_contract_report: None,
        },
        evidence,
    ))
}

fn capture_account_terminal(account: &AccountAny) -> AccountTerminalRecord {
    let (account_type, mut cash_locks, mut margins) = match account {
        AccountAny::Cash(account) => (
            AccountType::Cash,
            account
                .balances_locked
                .iter()
                .map(|((instrument_id, currency), money)| (*instrument_id, *currency, *money))
                .collect(),
            Vec::new(),
        ),
        AccountAny::Margin(account) => (
            AccountType::Margin,
            Vec::new(),
            account
                .margins
                .values()
                .chain(account.account_margins.values())
                .copied()
                .collect(),
        ),
        AccountAny::Betting(_) => (AccountType::Betting, Vec::new(), Vec::new()),
        AccountAny::Wallet(account) => (
            AccountType::Wallet,
            account
                .balances_locked
                .iter()
                .map(|((instrument_id, currency), money)| (*instrument_id, *currency, *money))
                .collect(),
            Vec::new(),
        ),
    };
    let mut balances = account.balances().into_values().collect::<Vec<_>>();
    balances.sort_by_key(|balance| balance.currency.to_string());
    cash_locks.sort_by_key(|(instrument_id, currency, _)| {
        (instrument_id.to_string(), currency.to_string())
    });
    margins.sort_by_key(|margin| {
        (
            margin
                .instrument_id
                .map(|instrument_id| instrument_id.to_string()),
            margin.currency.to_string(),
        )
    });
    AccountTerminalRecord {
        account_id: account.id(),
        account_type,
        base_currency: account.base_currency(),
        balances,
        cash_locks,
        margins,
    }
}

#[cfg(test)]
pub(crate) fn run_nt_backtest_node_capturing_evidence(
    manifest: &BacktestingRunManifest,
) -> Result<(
    NtBacktestNodeRun,
    crate::execution_evidence::ExecutionEvidence,
)> {
    run_nt_backtest_node_with_hooks(
        manifest,
        |engine| crate::execution_evidence::ExecutionEvidenceCapture::start(engine, manifest),
        crate::execution_evidence::ExecutionEvidenceCapture::finish,
    )
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
                trader_id: order.trader_id(),
                strategy_id: order.strategy_id(),
                instrument_id: order.instrument_id(),
                client_order_id: order.client_order_id(),
                account_id: order.account_id(),
                venue_order_id: order.venue_order_id(),
                position_id: order.position_id(),
                order_side: order.order_side(),
                order_type: order.order_type(),
                status: order.status(),
                quantity: order.quantity(),
                filled_qty: order.filled_qty(),
                leaves_qty: order.leaves_qty(),
                initialized_quantity: initialized.quantity,
                initialized_quote_quantity: initialized.quote_quantity,
                current_quote_quantity: order.is_quote_quantity(),
                trade_ids: order.trade_ids().into_iter().copied().collect(),
                commissions: {
                    let mut commissions = order
                        .commissions()
                        .iter()
                        .map(|(currency, money)| (*currency, *money))
                        .collect::<Vec<_>>();
                    commissions.sort_by_key(|(currency, _)| currency.to_string());
                    commissions
                },
                fills: order
                    .events()
                    .iter()
                    .filter_map(|event| match event {
                        OrderEventAny::Filled(fill) => Some(issue_789_proof_fill(fill)),
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
    F: FnOnce(
        &NtBacktestNodeRun,
        &crate::execution_evidence::ExecutionEvidence,
    ) -> Result<crate::execution_contract::ExecutionContractReport>,
{
    let (mut output, evidence) =
        crate::execution_evidence::run_nt_backtest_node_with_evidence(manifest)?;
    output.execution_contract_report = Some(validator(&output, &evidence)?);
    Ok(output)
}

#[cfg(test)]
fn run_nt_backtest_node_with_execution_contract_and_seeded_l2_quote_bridge<F>(
    manifest: &BacktestingRunManifest,
    plan: SeededL2QuoteBridgePlan,
    validator: F,
) -> Result<(NtBacktestNodeRun, SeededL2QuoteBridgeReport)>
where
    F: FnOnce(
        &NtBacktestNodeRun,
        &crate::execution_evidence::ExecutionEvidence,
    ) -> Result<crate::execution_contract::ExecutionContractReport>,
{
    let (mut output, (evidence, bridge_report)) = run_nt_backtest_node_with_hooks(
        manifest,
        |engine| {
            Ok((
                crate::execution_evidence::ExecutionEvidenceCapture::start(engine, manifest)?,
                install_seeded_l2_quote_bridge(engine, plan)?,
            ))
        },
        |(evidence, bridge)| Ok((evidence.finish()?, bridge.finalize()?)),
    )?;
    output.execution_contract_report = Some(validator(&output, &evidence)?);
    Ok((output, bridge_report))
}

#[cfg(test)]
fn replay_executable_book_at_cursor(
    instrument_id: InstrumentId,
    deltas: &[OrderBookDelta],
    event_count: usize,
) -> Result<OrderBook> {
    ensure!(event_count > 0, "event-store book cursor is empty");
    let last_flag = nautilus_model::enums::RecordFlag::F_LAST as u8;
    let mut book = OrderBook::new(instrument_id, nautilus_model::enums::BookType::L2_MBP);
    let mut replayed_events = 0usize;
    // The marker counts `OrderBookDeltas` publishes. Replay complete catalog
    // events through the Nth `F_LAST`, rather than mistaking that event ordinal
    // for an individual-delta row count.
    for delta in deltas {
        book.apply_delta(delta)
            .map_err(|error| anyhow::anyhow!(error))
            .context("replay executable book to event-store cursor")?;
        if delta.flags & last_flag != 0 {
            replayed_events += 1;
            if replayed_events == event_count {
                break;
            }
        }
    }
    ensure!(
        replayed_events == event_count,
        "event-store book cursor {event_count} exceeds {replayed_events} replayable hash-bound catalog events"
    );
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
        seeded_l2_quote_bridge_report: None,
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
        AggressorSide::Buy => TradeAggressorSide::Buyer.as_str(),
        AggressorSide::Sell => TradeAggressorSide::Seller.as_str(),
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
/// canonical action/side/price/size/order-id/flags/native-sequence/event-time
/// values. Positional comparison against the dense canonical row order rejects
/// drops, duplicates, and reorders; NT's sequence field carries only the native
/// source sequence, or zero when the source has none.
pub(crate) fn assert_delta_read_back_matches(
    read_back: &[OrderBookDelta],
    table: &super::canonical_market_data::CanonicalOrderBookDeltasTable,
    expected_instrument_id: &str,
    replay_clock: DeltaReplayClock,
) -> Result<()> {
    use super::canonical_market_data::{DeltaAction, DeltaSide};
    ensure!(
        read_back.len() == table.rows.len(),
        "delta catalog read-back count {} does not match canonical rows {}",
        read_back.len(),
        table.rows.len()
    );
    let expected_replay_times = order_book_delta_replay_times(table, replay_clock)?;
    for (index, ((delta, row), expected_ts_init)) in read_back
        .iter()
        .zip(table.rows.iter())
        .zip(expected_replay_times)
        .enumerate()
    {
        ensure!(
            delta.instrument_id.to_string() == expected_instrument_id,
            "delta read-back {index} instrument {} does not match projected {expected_instrument_id}",
            delta.instrument_id
        );
        let label = format!("delta sequence {}", row.sequence);
        let expected_sequence = native_order_book_sequence(row, &label)?;
        ensure!(
            delta.sequence == expected_sequence,
            "delta read-back {index} native sequence {} does not match expected {expected_sequence}",
            delta.sequence
        );
        ensure!(
            delta.flags == row.flags,
            "delta read-back {index} flags {} does not match canonical {}",
            delta.flags,
            row.flags
        );
        let expected_ts_event = ts_event_nanos(row.event_time, &label)?.as_u64();
        ensure!(
            delta.ts_event.as_u64() == expected_ts_event,
            "delta read-back {index} ts_event {} does not match canonical {expected_ts_event}",
            delta.ts_event.as_u64()
        );
        ensure!(
            delta.ts_init.as_u64() == expected_ts_init,
            "delta read-back {index} ts_init {} does not match the bound replay clock {expected_ts_init}",
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
        collections::{BTreeMap, BTreeSet},
        fs,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use anyhow::{Context, Result, bail, ensure};
    use bolt_v2::economics::NativeUnitId;
    use nautilus_backtest::node::BacktestNode;
    use nautilus_core::{Params, UUID4, UnixNanos};
    use nautilus_model::{
        data::{BookOrder, Data, InstrumentClose, OrderBookDelta, OrderBookDeltas, TradeTick},
        enums::{
            AccountType, AggressorSide, AssetClass, BookAction, InstrumentCloseType, OrderSide,
            OrderStatus, OrderType, PositionAdjustmentType, PositionSide, RecordFlag,
        },
        events::{AccountState, PositionAdjusted},
        identifiers::{
            AccountId, ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, Symbol,
            TradeId, TraderId, Venue,
        },
        instruments::{BinaryOption, Instrument, InstrumentAny},
        position::{PositionFillVoid, PositionReplayEvent},
        types::{AccountBalance, Currency, MarginBalance, Money, Price, Quantity},
    };
    use nautilus_persistence::backend::catalog::ParquetDataCatalog;
    use nautilus_polymarket::http::models::GammaMarket;
    use rust_decimal::Decimal;
    use serde::Deserialize;
    use sha2::{Digest, Sha256};

    use super::{
        AuthoritativeValuationObservation, BacktestDecisionEvidenceWriter,
        BacktestSelectorProvenance, OrderBook, OrderTerminalRecord, Position,
        PositiveFiniteEvidenceReadCap, StrategyPreparationConfig, apply_backtest_config_override,
        assert_delta_read_back_matches, assert_read_back_matches,
        canonical_resolved_taker_config_bytes, ensure_settlement_currency_funded,
        expected_iterations, issue_789_proof_fill, iterations_mismatch, load_bolt_v3_config,
        manifest_valuation_observation, prepare_strategy_client_routes, raw_taker_config,
        replay_executable_book_at_cursor, require_pre_run_configured_account,
        resolve_existing_input_path, run_nt_backtest_node,
        run_nt_backtest_node_with_execution_contract,
        run_nt_backtest_node_with_execution_contract_and_seeded_l2_quote_bridge,
        selector_provenance_hashes, time_window_excludes_all_data,
    };
    use crate::canonical_market_data::{
        CanonicalIndexPriceRow, CanonicalIndexPricesTable, CanonicalQuotesTable,
        NORMALIZED_SCHEMA_VERSION,
    };
    use crate::canonical_trades::JsonlStreamConfig;
    use crate::canonical_trades::{
        CanonicalInstrumentIdentity, CanonicalTradeRow, CsvTimestampUnit, RawPayloadConfig,
        RawPayloadContainer, TradeAggressorSide, TradesPartition,
    };
    use crate::catalog_projection::{
        CatalogInstrumentSpec, DeltaReplayClock, NT_DATA_TYPE_ORDER_BOOK_DELTA,
        NT_DATA_TYPE_QUOTE_TICK, SpotInstrumentSpec, build_catalog_instrument,
        order_book_delta_replay_times, project_canonical_index_to_catalog,
        project_canonical_order_book_deltas_to_catalog, project_canonical_quotes_to_catalog,
        read_back_order_book_deltas, read_back_quotes,
    };
    use crate::conversion_boundary::{
        ConversionCheckpoint, ConversionFingerprint, ConversionManifest, SeededL2QuotePlanV1,
    };
    use crate::pmxt_one_off_backfill_projection::{
        PmxtBookLevel, PmxtOneOffProjectionRequest, PmxtOneOffSelectedRow, PmxtOneOffSnapshotRow,
        PmxtOneOffTickSide, PmxtOneOffTradeRow, PmxtPriceChangeRow,
        project_pmxt_one_off_rows_to_nt, write_pmxt_one_off_projection_to_catalog,
    };
    use crate::run_manifest::{
        BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION, BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE,
        ManifestArtifactStore, ManifestBacktestConfigOverride, ManifestCatalogInput,
        ManifestEconomicsAuthorityValue, ManifestEconomicsInput, ManifestEconomicsSource,
        ManifestEconomicsValuationObservation, ManifestInstrumentSettlementInput,
        ManifestRealizedVolatilitySourceSelector, ManifestReferenceCurrentPriceInput,
        ManifestVenueConfig, MarketStructureFixture, RunPurpose, STRATEGY_BINARY_ORACLE_EDGE_TAKER,
        STRATEGY_BINARY_ORACLE_MAKER, STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE,
        STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES, STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT,
        STRATEGY_PARAM_ORDER_EXECUTION_MODE, StrategyConfigOverlaySource, StrategySource,
        StrategySourceKind, resolved_manifest_book_type,
    };
    use crate::seeded_l2_quote_bridge::{
        SeededL2QuoteBridgePlanInput, SeededL2QuoteBridgeReport,
        compile_seeded_l2_quote_bridge_plan,
    };
    use crate::seeded_level_set_deltas::{
        OrderCountPolicy, SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY,
        SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION, SeededLevelSetCompileInput,
        SeededLevelSetMappingConfig, SeededLevelSetOutputLimits, SeededLevelSetWindow,
        SeededLevelSetWindowBounds, SourceSequencePolicy, normalize_seeded_level_set_window,
    };

    type TerminalOrderMutation = Box<dyn Fn(&mut OrderTerminalRecord)>;
    type SettlementWitnessMutation = Box<
        dyn Fn(
            &mut nautilus_model::events::OrderInitialized,
            &mut nautilus_model::events::OrderAccepted,
        ),
    >;
    use crate::source_proof::{
        SourceProofFidelityClass, SourceProofUsageScope, synthetic_accepted_dataset_for_tests,
    };

    const TEST_INSTRUMENT: &str = "BTCUSDT.BYBIT";
    const MAKER_SMOKE_VENUE: &str = "POLYMARKET";
    const MAKER_SMOKE_YES_INSTRUMENT: &str = "SAMPLE-EVENT-YES.POLYMARKET";
    const MAKER_SMOKE_NO_INSTRUMENT: &str = "SAMPLE-EVENT-NO.POLYMARKET";
    const MAKER_SMOKE_MARKET_SLUG: &str = "will-sample-event-resolve-yes";

    #[test]
    fn manifest_exact_conversion_uses_checksummed_configured_origin_kind() -> Result<()> {
        let loaded =
            load_bolt_v3_config(&resolve_existing_input_path(Path::new("config/root.toml")))?;
        let observation = |from_unit: &str| {
            manifest_valuation_observation(
                &loaded,
                "polymarket_main",
                &ManifestEconomicsValuationObservation::ProviderExactConversion {
                    source_id: "collateral".to_string(),
                    from_unit: from_unit.to_string(),
                    to_unit: "USDC.e".to_string(),
                    snapshot_id: "snapshot".to_string(),
                    observed_at_ns: 1,
                    fetched_at_ns: 1,
                    valid_until_ns: 2,
                },
            )
        };

        let currency = observation("pUSD")?;
        assert!(matches!(
            currency,
            AuthoritativeValuationObservation::ProviderExactConversion {
                from_unit: NativeUnitId::Currency(unit),
                ..
            } if unit.as_str() == "pUSD"
        ));
        Ok(())
    }

    fn polymarket_replay_economics(
        instruments: &[(&str, &str)],
        condition_id: &str,
        source_at_ns: u64,
        valid_until_ns: u64,
    ) -> ManifestEconomicsSource {
        let market_info_json = serde_json::json!({
            "r": {},
            "t": instruments
                .iter()
                .enumerate()
                .map(|(index, (_, provider_instrument_id))| serde_json::json!({
                    "t": provider_instrument_id,
                    "o": if index == 0 { "Yes" } else { "No" },
                }))
                .collect::<Vec<_>>(),
            "c": condition_id,
            "mos": "1",
            "mts": "1",
            // The replay fixture's provider snapshot is the zero-fee authority;
            // production config must not manufacture this value when `fd` is absent.
            "fd": { "r": 0, "e": 1, "to": true },
        })
        .to_string();
        let valuation_observations = vec![
            ManifestEconomicsValuationObservation::ProviderExactConversion {
                source_id: "collateral".to_string(),
                from_unit: "pUSD".to_string(),
                to_unit: "USDC.e".to_string(),
                snapshot_id: format!("{condition_id}-collateral"),
                observed_at_ns: source_at_ns,
                fetched_at_ns: source_at_ns,
                valid_until_ns,
            },
            ManifestEconomicsValuationObservation::MarketQuote {
                client_id: "coinbase_data".to_string(),
                instrument_id: "USDC-USD.COINBASE".to_string(),
                base_currency: "USDC".to_string(),
                quote_currency: "USD".to_string(),
                price: "1".to_string(),
                snapshot_id: format!("{condition_id}-usdc-usd"),
                observed_at_ns: source_at_ns,
                fetched_at_ns: source_at_ns,
                valid_until_ns,
            },
        ];
        ManifestEconomicsSource {
            production_root_config_path: "config/root.toml".to_string(),
            production_config_bundle_checksum: load_bolt_v3_config(&resolve_existing_input_path(
                Path::new("config/root.toml"),
            ))
            .expect("replay fixture production config must load")
            .config_bundle_checksum,
            inputs: instruments
                .iter()
                .map(|(instrument_id, provider_instrument_id)| {
                    let authority = BTreeMap::from([
                        (
                            "provider_instrument_id".to_string(),
                            ManifestEconomicsAuthorityValue::String(
                                (*provider_instrument_id).to_string(),
                            ),
                        ),
                        (
                            "snapshot_id".to_string(),
                            ManifestEconomicsAuthorityValue::String(format!(
                                "{condition_id}-market-info"
                            )),
                        ),
                        (
                            "source_at_ns".to_string(),
                            ManifestEconomicsAuthorityValue::Integer(
                                i64::try_from(source_at_ns).expect("fixture timestamp fits i64"),
                            ),
                        ),
                        (
                            "fetched_at_ns".to_string(),
                            ManifestEconomicsAuthorityValue::Integer(
                                i64::try_from(source_at_ns).expect("fixture timestamp fits i64"),
                            ),
                        ),
                        (
                            "valid_until_ns".to_string(),
                            ManifestEconomicsAuthorityValue::Integer(
                                i64::try_from(valid_until_ns).expect("fixture timestamp fits i64"),
                            ),
                        ),
                        (
                            "market_info_json".to_string(),
                            ManifestEconomicsAuthorityValue::String(market_info_json.clone()),
                        ),
                    ]);
                    ManifestEconomicsInput {
                        instrument_id: (*instrument_id).to_string(),
                        product_surface_id: "binary_outcome".to_string(),
                        authority,
                        valuation_observations: valuation_observations.clone(),
                    }
                })
                .collect(),
        }
    }
    const MAKER_SMOKE_CONDITION_ID: &str = "condition-sample-event";
    const MAKER_SMOKE_QUESTION_ID: &str = "question-sample-event";
    const MAKER_SMOKE_CLIENT_ID: &str = "maker_execution_client";
    const MAKER_SMOKE_RUN_ID: &str = "binary-oracle-maker-backtest-smoke";
    const MAKER_SMOKE_TS_NS: u64 = 1_772_323_201_665_000_000;

    fn strategy_input_fixture(
        fixture: &str,
        baseline_recorded_at_utc_ns: i64,
        recorded_at_utc_ns: i64,
        spot_price: &str,
    ) -> Vec<u8> {
        let baseline = fixture.lines().next().expect("fixture must contain a line");
        let timestamp_anchor = format!("\"recorded_at_utc_ns\":{baseline_recorded_at_utc_ns}");
        assert_eq!(
            baseline.matches(&timestamp_anchor).count(),
            1,
            "fixture must contain exactly one declared baseline timestamp"
        );
        let timestamp = format!("\"recorded_at_utc_ns\":{recorded_at_utc_ns}");
        let line = baseline.replacen(&timestamp_anchor, &timestamp, 1);
        let spot_anchor = "\"spot_price\":\"100\"";
        assert_eq!(
            line.matches(spot_anchor).count(),
            1,
            "fixture must contain exactly one baseline spot price"
        );
        let line = line.replacen(spot_anchor, &format!("\"spot_price\":\"{spot_price}\""), 1);
        format!("{line}\n").into_bytes()
    }

    #[test]
    fn backtest_guard_selects_latest_strategy_input_across_separate_streams() -> Result<()> {
        let writer = BacktestDecisionEvidenceWriter::new(
            4,
            PositiveFiniteEvidenceReadCap::new(1_048_576)
                .expect("test evidence cap must be positive and finite"),
        )?;
        fs::write(
            writer.machine.as_path(),
            strategy_input_fixture(
                include_str!(
                    "../../../tests/fixtures/bolt_v3/current_evidence/positive/submit_linked_strategy_input_snapshot.jsonl"
                ),
                33,
                200,
                "200",
            ),
        )?;
        fs::write(
            writer.observation.as_path(),
            strategy_input_fixture(
                include_str!(
                    "../../../tests/fixtures/bolt_v3/current_evidence/positive/blocked_strategy_input_observation.jsonl"
                ),
                32,
                100,
                "100",
            ),
        )?;

        let state = writer.state()?;
        assert_eq!(state.strategy_input_snapshot_count, 2);
        assert_eq!(
            state
                .latest_strategy_input_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.spot_price.as_deref()),
            Some("200")
        );
        Ok(())
    }

    #[test]
    fn backtest_guard_counts_exit_intent_once_without_counting_preparation() -> Result<()> {
        let writer = BacktestDecisionEvidenceWriter::new(
            4,
            PositiveFiniteEvidenceReadCap::new(1_048_576)
                .expect("test evidence cap must be positive and finite"),
        )?;
        let intent = include_str!(
            "../../../tests/fixtures/bolt_v3/current_evidence/positive/exit_intent_decision.jsonl"
        )
        .lines()
        .next()
        .expect("exit-intent fixture must contain a line");
        let prepared = include_str!(
            "../../../tests/fixtures/bolt_v3/current_evidence/positive/exit_prepared_order.jsonl"
        )
        .lines()
        .next()
        .expect("exit-prepared fixture must contain a line");
        fs::write(writer.machine.as_path(), format!("{intent}\n{prepared}\n"))?;

        let state = writer.state()?;
        assert_eq!(state.exit_decision_count, 1);
        Ok(())
    }

    #[test]
    fn backtest_guard_rejects_conflicting_strategy_inputs_at_the_same_timestamp() -> Result<()> {
        let writer = BacktestDecisionEvidenceWriter::new(
            4,
            PositiveFiniteEvidenceReadCap::new(1_048_576)
                .expect("test evidence cap must be positive and finite"),
        )?;
        fs::write(
            writer.machine.as_path(),
            strategy_input_fixture(
                include_str!(
                    "../../../tests/fixtures/bolt_v3/current_evidence/positive/submit_linked_strategy_input_snapshot.jsonl"
                ),
                33,
                200,
                "200",
            ),
        )?;
        fs::write(
            writer.observation.as_path(),
            strategy_input_fixture(
                include_str!(
                    "../../../tests/fixtures/bolt_v3/current_evidence/positive/blocked_strategy_input_observation.jsonl"
                ),
                32,
                200,
                "200",
            ),
        )?;

        let error = writer
            .state()
            .expect_err("same-timestamp conflicting facts must not select an arbitrary winner");
        assert!(
            error
                .to_string()
                .contains("conflicting strategy-input facts share recorded_at_utc_ns 200")
        );
        Ok(())
    }

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
        let ticks = vec![tick("t1", "100.5", "2.0", AggressorSide::Buy, 1000)];
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
        let ticks = vec![tick("t1", "999.5", "2.0", AggressorSide::Buy, 1000)];
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
        let ticks = vec![tick("t1", "100.5", "9.0", AggressorSide::Buy, 1000)];
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
        let ticks = vec![tick("t1", "100.5", "2.0", AggressorSide::Sell, 1000)];
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
        let ticks = vec![tick("t1", "100.5", "2.0", AggressorSide::Buy, 2000)];
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
                    AggressorSide::Buy,
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
                    AggressorSide::Sell,
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
                        AggressorSide::Buy,
                        MAKER_SMOKE_TS_NS,
                    ),
                    maker_smoke_trade(
                        instrument_id,
                        "execution-contract-exit",
                        AggressorSide::Sell,
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
        client_id = "polymarket_main"
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
                    (
                        STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES.to_string(),
                        "1048576".to_string(),
                    ),
                    (
                        STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT.to_string(),
                        "4096".to_string(),
                    ),
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
            economics: Some(polymarket_replay_economics(
                &[
                    (MAKER_SMOKE_YES_INSTRUMENT, "SAMPLE-EVENT-YES"),
                    (MAKER_SMOKE_NO_INSTRUMENT, "SAMPLE-EVENT-NO"),
                ],
                "condition-sample-event",
                MAKER_SMOKE_TS_NS,
                MAKER_SMOKE_TS_NS + 60_000_000_000,
            )),
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
    fn binary_oracle_replay_rejects_malformed_provider_economics() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create maker smoke catalog root")?;
        write_maker_smoke_catalog(tempdir.path())?;
        let mut manifest = maker_smoke_manifest(tempdir.path());
        let input = manifest
            .economics
            .as_mut()
            .and_then(|economics| economics.inputs.first_mut())
            .context("maker smoke economics input")?;
        input.authority.insert(
            "market_info_json".to_string(),
            ManifestEconomicsAuthorityValue::String("{}".to_string()),
        );

        let error = match run_nt_backtest_node(&manifest) {
            Ok(_) => anyhow::bail!("malformed provider economics unexpectedly ran"),
            Err(error) => error,
        };
        ensure!(
            format!("{error:#}").contains("Polymarket market-info is invalid"),
            "unexpected malformed-economics error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn binary_oracle_replay_rejects_changed_production_economics_config() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create replay manifest root")?;
        let mut manifest = maker_smoke_manifest(tempdir.path());
        manifest
            .economics
            .as_mut()
            .context("maker smoke economics source")?
            .production_config_bundle_checksum = "0".repeat(64);

        let error = super::load_manifest_economics_config(&manifest)
            .expect_err("a changed production economics config must fail closed");
        ensure!(
            format!("{error:#}").contains("production economics config bundle checksum mismatch"),
            "unexpected config-integrity error: {error:#}"
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
        manifest.economics = None;
        manifest.catalog_inputs.truncate(1);
        let result = run_nt_backtest_node_with_execution_contract(&manifest, |_, _| {
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
    fn execution_contract_rejects_followup_delayed_past_next_fill() {
        let error = ensure_one_causal_followup_per_fill(
            "position mutation",
            &[10, 20, 30],
            &[21, 22, 31],
            40,
        )
        .expect_err("first mutation after the next fill must fail closed");
        assert!(error.to_string().contains("outside its causal interval"));
    }

    #[test]
    fn execution_contract_rejects_terminal_followup_after_settlement_receipt() {
        let error = ensure_one_causal_followup_per_fill(
            "AccountState transition",
            &[10, 20, 30],
            &[11, 21, 41],
            40,
        )
        .expect_err("terminal account transition after close receipt must fail closed");
        assert!(error.to_string().contains("outside its causal interval"));
    }

    #[test]
    fn execution_contract_config_identity_covers_applied_rv_source_filter() -> Result<()> {
        let raw_config = toml::from_str::<toml::Value>(&maker_smoke_config_toml())?;
        let mut override_spec = StrategyConfigOverlaySource {
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
    const ISSUE_789_ITERATIONS: usize = 587_788;
    const ISSUE_789_CONDITION_ID: &str =
        "0xb98f764c4d5dd36580c8c9903bc75ddcb631428d84e9c1e532f0da236f77054c";
    const ISSUE_789_UP_TOKEN: &str =
        "70185630899601185587604849909583851214968263628583846260964185007520683306835";
    const ISSUE_789_DOWN_TOKEN: &str =
        "39327110184724906690545821148183414832224062782460969169826610548819991310639";
    const ISSUE_789_MARKET_SLUG: &str = "btc-updown-5m-1776816000";
    // Canonical pretty-JSON hashes over the ordered BBO semantic tuple rows below.
    const ISSUE_789_OKX_BBO_ROWS: usize = 15_148;
    const ISSUE_789_OKX_BBO_SHA256: &str =
        "40c097a000361e032f431ee0aa05672ae994b54c99070f055644407f31e56333";
    const ISSUE_789_BYBIT_BBO_ROWS: usize = 2_719;
    const ISSUE_789_BYBIT_BBO_SHA256: &str =
        "4db2659ee879bbf220d2725cc89984aadd4943e78a04590e759315954ea8b336";

    fn assert_issue_789_bbo_semantics(
        name: &str,
        table: &CanonicalQuotesTable,
        expected_rows: usize,
        expected_sha256: &str,
    ) -> Result<()> {
        let semantic_rows = table
            .rows
            .iter()
            .map(|row| {
                (
                    row.nt_instrument_id.as_deref(),
                    row.bid.as_str(),
                    row.ask.as_str(),
                    row.bid_size.as_str(),
                    row.ask_size.as_str(),
                    row.event_time,
                    row.availability_time,
                    row.source_sequence.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        ensure!(
            semantic_rows.len() == expected_rows,
            "issue #789 {name} full-depth-derived BBO row count {} != source golden {expected_rows}",
            semantic_rows.len()
        );
        let actual_sha256 = crate::reference_artifact::canonical_json_sha256(&semantic_rows)?;
        ensure!(
            actual_sha256 == expected_sha256,
            "issue #789 {name} BBO hash {actual_sha256} != golden {expected_sha256}"
        );
        Ok(())
    }

    fn assert_issue_789_delta_catalog_replay_matches_quotes(
        name: &str,
        window: &SeededLevelSetWindow,
        instrument_spec: &SpotInstrumentSpec,
        catalog_root: &Path,
    ) -> Result<crate::catalog_projection::CatalogProjection> {
        let quotes = window
            .quotes
            .as_ref()
            .with_context(|| format!("issue #789 {name} emitted no derived BBO"))?;
        let projection = project_canonical_order_book_deltas_to_catalog(
            &window.deltas,
            instrument_spec,
            DeltaReplayClock::StrictEncounterOrder,
            catalog_root,
        )
        .with_context(|| format!("project issue #789 {name} full-depth deltas"))?;
        let read_back = read_back_order_book_deltas(catalog_root, &projection.nt_instrument_id)
            .with_context(|| format!("read back issue #789 {name} full-depth deltas"))?;
        assert_delta_read_back_matches(
            &read_back,
            &window.deltas,
            &projection.nt_instrument_id,
            DeltaReplayClock::StrictEncounterOrder,
        )?;

        let instrument_id = InstrumentId::from_str(&projection.nt_instrument_id)
            .with_context(|| format!("parse issue #789 {name} instrument"))?;
        let mut book = OrderBook::new(instrument_id, nautilus_model::enums::BookType::L2_MBP);
        let mut event = Vec::new();
        let mut quote_index = 0usize;
        for delta in read_back {
            let is_last = delta.flags & RecordFlag::F_LAST as u8 != 0;
            event.push(delta);
            if !is_last {
                continue;
            }
            let terminal = *event.last().context("closed delta event is empty")?;
            let batch = OrderBookDeltas::new_checked(instrument_id, std::mem::take(&mut event))
                .context("construct issue #789 replay event")?;
            book.apply_deltas(&batch)
                .map_err(|error| anyhow::anyhow!(error))
                .with_context(|| format!("apply issue #789 {name} replay event"))?;
            let (Some(bid), Some(ask), Some(bid_size), Some(ask_size)) = (
                book.best_bid_price(),
                book.best_ask_price(),
                book.best_bid_size(),
                book.best_ask_size(),
            ) else {
                continue;
            };
            let expected = quotes
                .rows
                .get(quote_index)
                .with_context(|| format!("issue #789 {name} replay emitted an unexpected BBO"))?;
            for (field, actual, expected) in [
                ("bid", bid.as_decimal(), expected.bid.as_str()),
                ("ask", ask.as_decimal(), expected.ask.as_str()),
                (
                    "bid_size",
                    bid_size.as_decimal(),
                    expected.bid_size.as_str(),
                ),
                (
                    "ask_size",
                    ask_size.as_decimal(),
                    expected.ask_size.as_str(),
                ),
            ] {
                ensure!(
                    actual == Decimal::from_str(expected)?,
                    "issue #789 {name} replay quote {quote_index} {field} {actual} != derived {expected}"
                );
            }
            ensure!(
                terminal.ts_event.as_u64() == expected.event_time as u64,
                "issue #789 {name} replay quote {quote_index} event timestamp drift"
            );
            ensure!(
                expected.availability_time == Some(expected.event_time),
                "issue #789 {name} audit quote {quote_index} does not retain source availability"
            );
            ensure!(
                terminal.ts_init.as_u64() >= terminal.ts_event.as_u64(),
                "issue #789 {name} replay quote {quote_index} precedes its source event"
            );
            quote_index += 1;
        }
        ensure!(event.is_empty(), "issue #789 {name} ended without F_LAST");
        ensure!(
            quote_index == quotes.rows.len(),
            "issue #789 {name} delta replay produced {quote_index} BBO rows for {} derived rows",
            quotes.rows.len()
        );
        Ok(projection)
    }

    fn issue_789_seeded_conversion_manifest(
        name: &str,
        accepted_object_sha256: &str,
        window: &SeededLevelSetWindow,
        projection: &crate::catalog_projection::CatalogProjection,
    ) -> Result<ConversionManifest> {
        let fingerprint = ConversionFingerprint {
            source_proof_id: format!("issue-789-{name}-full-depth-l2"),
            source_proof_version: 1,
            accepted_object_sha256: accepted_object_sha256.to_string(),
            converter_identity: SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY.to_string(),
            converter_version: SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION.to_string(),
            converter_config_hash: sha256_hex(name.as_bytes()),
        };
        let checkpoint = ConversionCheckpoint::completed(
            fingerprint.clone(),
            window.deltas.rows.len(),
            projection.catalog_hash.clone(),
            "2026-04-22T00:05:00Z",
        );
        let checkpoint_hash = checkpoint.content_hash()?;
        ConversionManifest::completed(
            fingerprint,
            NORMALIZED_SCHEMA_VERSION,
            "OrderBookDelta",
            projection.nt_instrument_id.clone(),
            window.deltas.rows.len(),
            format!("memory://issue-789/{name}/deltas"),
            projection.catalog_hash.clone(),
            checkpoint_hash,
            "2026-04-22T00:05:00Z",
        )
        .with_catalog_rows_by_nt_data_type(BTreeMap::from([
            (
                NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
                window.deltas.rows.len(),
            ),
            (
                NT_DATA_TYPE_QUOTE_TICK.to_string(),
                window.quotes.as_ref().map_or(0, |quotes| quotes.rows.len()),
            ),
        ]))
        .with_seeded_l2_quote_plan(issue_789_seeded_l2_quote_plan(window)?)
    }

    fn issue_789_seeded_l2_quote_plan(
        window: &SeededLevelSetWindow,
    ) -> Result<SeededL2QuotePlanV1> {
        let replay_times =
            order_book_delta_replay_times(&window.deltas, DeltaReplayClock::StrictEncounterOrder)?;
        Ok(SeededL2QuotePlanV1 {
            synthetic_seed_batches: window.synthetic_seed_batches,
            selected_source_events: window.selected_events,
            replay_start_time: i64::try_from(
                *replay_times
                    .first()
                    .context("issue #789 seeded L2 replay has no first timestamp")?,
            )
            .context("issue #789 first replay timestamp exceeds i64")?,
            replay_end_time: i64::try_from(
                *replay_times
                    .last()
                    .context("issue #789 seeded L2 replay has no terminal timestamp")?,
            )
            .context("issue #789 terminal replay timestamp exceeds i64")?,
        })
    }

    #[test]
    fn issue_789_first_real_free_data_taker_pl() -> Result<()> {
        let tempdir = tempfile::TempDir::new().context("create issue #789 temp catalog root")?;
        let okx_spot = spot_spec(
            "BTC-USDT.OKX",
            "BTC-USDT",
            "BTC",
            "USDT",
            "0.1",
            "0.00000001",
        );
        let okx_window = seeded_window(
            &gunzip_pinned_fixture(
                include_bytes!(
                    "../tests/fixtures/issue_789_first_pl/okx_btc_usdt_l2_20260422_000000_000300.jsonl.gz"
                ),
                ISSUE_789_OKX_FIXTURE_SHA256,
                "okx",
            )?,
            okx_seeded_level_set_mapping(),
            &okx_spot,
            QuoteTableSpec {
                source_binding: "okx-official-historical-l2-400lv",
                venue: "OKX",
                instrument_id: "BTC-USDT",
                venue_symbol: "BTC-USDT",
                nt_instrument_id: "BTC-USDT.OKX",
                payload_id: "https://static.okx.com/cdn/okx/match/orderbook/L2/400lv/daily/20260422/BTC-USDT-L2orderbook-400lv-2026-04-22.tar.gz",
            },
        )?;
        let bybit_spot = spot_spec(
            "BTCUSDT-SPOT.BYBIT",
            "BTCUSDT",
            "BTC",
            "USDT",
            "0.1",
            "0.000001",
        );
        let bybit_window = seeded_window(
            &gunzip_pinned_fixture(
                include_bytes!(
                    "../tests/fixtures/issue_789_first_pl/bybit_btc_usdt_l2_20260422_000000_000300.jsonl.gz"
                ),
                ISSUE_789_BYBIT_FIXTURE_SHA256,
                "bybit",
            )?,
            bybit_seeded_level_set_mapping(),
            &bybit_spot,
            QuoteTableSpec {
                source_binding: "bybit-quote-saver-ob200",
                venue: "BYBIT",
                instrument_id: "BTCUSDT",
                venue_symbol: "BTCUSDT",
                nt_instrument_id: "BTCUSDT-SPOT.BYBIT",
                payload_id: "https://quote-saver.bycsi.com/orderbook/spot/BTCUSDT/2026-04-22_BTCUSDT_ob200.data.zip",
            },
        )?;
        let okx_quotes = okx_window
            .quotes
            .as_ref()
            .context("issue #789 OKX fixture emitted no derived BBO")?;
        let bybit_quotes = bybit_window
            .quotes
            .as_ref()
            .context("issue #789 Bybit fixture emitted no derived BBO")?;
        let okx_delta_catalog = tempdir.path().join("okx_btc_usdt_deltas");
        let okx_delta_projection = assert_issue_789_delta_catalog_replay_matches_quotes(
            "OKX",
            &okx_window,
            &okx_spot,
            &okx_delta_catalog,
        )?;
        let bybit_delta_catalog = tempdir.path().join("bybit_btc_usdt_deltas");
        let bybit_delta_projection = assert_issue_789_delta_catalog_replay_matches_quotes(
            "Bybit",
            &bybit_window,
            &bybit_spot,
            &bybit_delta_catalog,
        )?;
        assert_issue_789_bbo_semantics(
            "OKX",
            okx_quotes,
            ISSUE_789_OKX_BBO_ROWS,
            ISSUE_789_OKX_BBO_SHA256,
        )?;
        assert_issue_789_bbo_semantics(
            "Bybit",
            bybit_quotes,
            ISSUE_789_BYBIT_BBO_ROWS,
            ISSUE_789_BYBIT_BBO_SHA256,
        )?;

        let okx_audit_catalog = tempdir.path().join("okx_btc_usdt_audit_quotes");
        project_canonical_quotes_to_catalog(okx_quotes, &okx_spot, &okx_audit_catalog)
            .context("project OKX full-depth-derived audit BBO quotes")?;
        let bybit_audit_catalog = tempdir.path().join("bybit_btc_usdt_audit_quotes");
        project_canonical_quotes_to_catalog(bybit_quotes, &bybit_spot, &bybit_audit_catalog)
            .context("project Bybit full-depth-derived audit BBO quotes")?;
        let okx_delta_read_back = read_back_order_book_deltas(&okx_delta_catalog, "BTC-USDT.OKX")?;
        let bybit_delta_read_back =
            read_back_order_book_deltas(&bybit_delta_catalog, "BTCUSDT-SPOT.BYBIT")?;
        let okx_audit_read_back = read_back_quotes(&okx_audit_catalog, "BTC-USDT.OKX")?;
        let bybit_audit_read_back = read_back_quotes(&bybit_audit_catalog, "BTCUSDT-SPOT.BYBIT")?;
        let okx_conversion_manifest = issue_789_seeded_conversion_manifest(
            "OKX",
            ISSUE_789_OKX_FIXTURE_SHA256,
            &okx_window,
            &okx_delta_projection,
        )?;
        let bybit_conversion_manifest = issue_789_seeded_conversion_manifest(
            "Bybit",
            ISSUE_789_BYBIT_FIXTURE_SHA256,
            &bybit_window,
            &bybit_delta_projection,
        )?;
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
        let chainlink_table = reconstructed_chainlink_price_to_beat_table(okx_quotes)?;
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
            okx_catalog: okx_delta_catalog,
            okx_catalog_hash: okx_delta_projection.catalog_hash.clone(),
            bybit_catalog: bybit_delta_catalog,
            bybit_catalog_hash: bybit_delta_projection.catalog_hash.clone(),
            chainlink_catalog,
            chainlink_catalog_hash: chainlink_projection.catalog_hash.clone(),
            up_catalog,
            up_catalog_hash: up_catalog_report.catalog_hash.clone(),
            up_instrument_id,
            down_catalog,
            down_catalog_hash: down_catalog_report.catalog_hash.clone(),
            down_instrument_id,
            reference_rows: reconstructed_reference_rows_from_okx(okx_quotes)?,
            instrument_settlements,
        })?;
        let okx_instrument_id = InstrumentId::from("BTC-USDT.OKX");
        let bybit_instrument_id = InstrumentId::from("BTCUSDT-SPOT.BYBIT");
        let bridge_plan = compile_seeded_l2_quote_bridge_plan(vec![
            SeededL2QuoteBridgePlanInput {
                conversion_manifest: &okx_conversion_manifest,
                client_id: Some(ClientId::from("okx_data")),
                book_type: resolved_manifest_book_type(&manifest, okx_instrument_id)
                    .map_err(|error| anyhow::anyhow!(error))?,
                deltas: &okx_delta_read_back,
                audit_quotes: &okx_audit_read_back,
            },
            SeededL2QuoteBridgePlanInput {
                conversion_manifest: &bybit_conversion_manifest,
                client_id: Some(ClientId::from("bybit_data")),
                book_type: resolved_manifest_book_type(&manifest, bybit_instrument_id)
                    .map_err(|error| anyhow::anyhow!(error))?,
                deltas: &bybit_delta_read_back,
                audit_quotes: &bybit_audit_read_back,
            },
        ])?;

        let run_config = manifest
            .to_nt_run_config()
            .map_err(|error| anyhow::anyhow!("issue #789 manifest conversion failed: {error}"))?;
        for (instrument_id, expected) in [
            (InstrumentId::from("BTC-USDT.OKX"), &okx_delta_read_back),
            (
                InstrumentId::from("BTCUSDT-SPOT.BYBIT"),
                &bybit_delta_read_back,
            ),
        ] {
            let data_config = run_config
                .data()
                .iter()
                .find(|config| {
                    config
                        .get_instrument_ids()
                        .is_ok_and(|ids| ids.contains(&instrument_id))
                })
                .with_context(|| format!("missing issue #789 data config for {instrument_id}"))?;
            let actual =
                BacktestNode::load_data_config(data_config, run_config.start(), run_config.end())?
                    .into_iter()
                    .filter_map(|data| match data {
                        Data::Delta(delta) => Some(delta),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
            ensure!(
                actual.len() == expected.len(),
                "issue #789 runtime query returned {} {instrument_id} deltas, expected {}",
                actual.len(),
                expected.len()
            );
            let mismatch = actual
                .iter()
                .zip(expected.iter())
                .position(|(actual, expected)| actual != expected);
            ensure!(
                mismatch.is_none(),
                "issue #789 runtime query reordered {instrument_id} at row {}",
                mismatch.unwrap_or_default()
            );
        }

        ensure_issue_789_venue_shape(&manifest.venue)?;
        let expected_binary_settlements = ensure_issue_789_binary_settlement_domain(
            &manifest.instrument_settlements,
            &up_projection.instrument,
            &down_projection.instrument,
        )?;
        validate_issue_789_book_domain(
            &up_projection.instrument,
            &up_projection.order_book_deltas,
        )?;
        validate_issue_789_book_domain(
            &down_projection.instrument,
            &down_projection.order_book_deltas,
        )?;

        let (output, bridge_report) =
            run_nt_backtest_node_with_execution_contract_and_seeded_l2_quote_bridge(
                &manifest,
                bridge_plan,
                |output, evidence| {
                    evidence.ensure_issue_789_causal_surface(&manifest)?;
                    validate_issue_789_execution_contract(
                        output,
                        evidence,
                        &manifest,
                        &up_projection,
                        &down_projection,
                        &expected_binary_settlements,
                    )
                },
            )
            .context("run issue #789 first real free-data taker P/L slice")?;
        ensure!(
            bridge_report.instruments.len() == 2,
            "issue #789 causal bridge did not prove both seeded instruments"
        );
        for (instrument_id, expected_quotes) in [
            ("BTC-USDT.OKX", ISSUE_789_OKX_BBO_ROWS),
            ("BTCUSDT-SPOT.BYBIT", ISSUE_789_BYBIT_BBO_ROWS),
        ] {
            let report = bridge_report
                .instruments
                .iter()
                .find(|report| report.nt_instrument_id == instrument_id)
                .with_context(|| format!("issue #789 bridge omitted {instrument_id}"))?;
            ensure!(
                report.emitted_quotes == expected_quotes as u64,
                "issue #789 bridge emitted {} {instrument_id} quotes, expected {expected_quotes}",
                report.emitted_quotes
            );
        }
        let guard = output
            .run_guard_report
            .as_ref()
            .context("missing binary-oracle run guard report")?;
        let did_not_arm = || {
            guard.did_not_arm_reason.clone().unwrap_or_else(|| {
                "did NOT arm — guard did not provide a feed-specific reason".to_string()
            })
        };

        ensure!(
            output.result.iterations == ISSUE_789_ITERATIONS,
            "issue #789 backtest consumed {} elements, expected {ISSUE_789_ITERATIONS}",
            output.result.iterations
        );

        println!("issue_789_result_label=production config + documented OKX/Bybit override");
        println!(
            "issue_789_override_report={:?}",
            output.config_override_report
        );
        println!(
            "issue_789_feed_labels=signal:OKX BBO derived from real full-depth L2; rv:OKX BBO derived from real full-depth L2; rv:Bybit BBO derived from real full-depth L2; tradable:PMXT real R2 archive book/price_change/trades WITH converter-synthesized uncross deltas (not byte-faithful); strike/reference:reconstructed-from-spot not raw Chainlink; fidelity:ZERO-LATENCY single-clock replay (spot/reference on exchange event-time, fast-venue age pinned 0; ~120ms live spot->PM lead NOT modeled) — the P/L is a reconstructed-replay figure, not latency-aware"
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
        write_issue_789_result_artifact(&output, guard, &bridge_report)?;

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

    fn issue_789_commission_projection(
        fills: &[nautilus_model::events::OrderFilled],
    ) -> Result<Vec<(Currency, Money)>> {
        let mut totals = BTreeMap::<String, (Currency, Decimal)>::new();
        for fill in fills {
            let commission = fill
                .commission
                .with_context(|| format!("fill {} has no commission", fill.trade_id))?;
            let entry = totals
                .entry(commission.currency.to_string())
                .or_insert((commission.currency, Decimal::ZERO));
            entry.1 += commission.as_decimal();
        }
        totals
            .into_values()
            .map(|(currency, amount)| {
                Money::from_decimal(amount, currency)
                    .map_err(anyhow::Error::msg)
                    .map(|money| (currency, money))
            })
            .collect()
    }

    #[derive(Clone, Copy)]
    enum Issue789ExecutionQuantityPolicy {
        Exact,
        ReducibleIocBound,
    }

    impl Issue789ExecutionQuantityPolicy {
        fn accepts(self, requested: Quantity, effective: Quantity) -> bool {
            match self {
                Self::Exact => effective == requested,
                Self::ReducibleIocBound => effective <= requested,
            }
        }
    }

    fn ensure_issue_789_terminal_order_matches(
        terminal: &super::OrderTerminalRecord,
        initialized: &nautilus_model::events::OrderInitialized,
        configured_account_id: AccountId,
        position_id: PositionId,
        requested_effective_quantity: Quantity,
        quantity_policy: Issue789ExecutionQuantityPolicy,
        fills: &[nautilus_model::events::OrderFilled],
    ) -> Result<()> {
        let filled_decimal = fills
            .iter()
            .map(|fill| fill.last_qty.as_decimal())
            .sum::<Decimal>();
        let expected_filled_quantity =
            Quantity::from_decimal_dp(filled_decimal, terminal.quantity.precision)
                .map_err(anyhow::Error::msg)
                .context("#789 terminal filled quantity is not representable")?;
        let first_fill = fills
            .first()
            .context("#789 terminal order has no causal fills")?;
        let expected_trade_ids = fills.iter().map(|fill| fill.trade_id).collect::<Vec<_>>();
        let expected_commissions = issue_789_commission_projection(fills)?;
        let expected_fills = fills.iter().map(issue_789_proof_fill).collect::<Vec<_>>();
        ensure!(
            quantity_policy.accepts(requested_effective_quantity, terminal.quantity),
            "terminal order {} has invalid execution-time quantity: requested={} effective={}",
            initialized.client_order_id,
            requested_effective_quantity,
            terminal.quantity,
        );
        ensure!(
            terminal.trader_id == initialized.trader_id
                && terminal.strategy_id == initialized.strategy_id
                && terminal.instrument_id == initialized.instrument_id
                && terminal.client_order_id == initialized.client_order_id
                && terminal.account_id == Some(configured_account_id)
                && terminal.venue_order_id == Some(first_fill.venue_order_id)
                && terminal.position_id == Some(position_id)
                && terminal.order_side == initialized.order_side
                && terminal.order_type == initialized.order_type
                && terminal.status == OrderStatus::Filled
                && terminal.filled_qty == expected_filled_quantity
                && terminal.quantity == expected_filled_quantity
                && terminal.leaves_qty.is_zero()
                && terminal.initialized_quantity == initialized.quantity
                && terminal.initialized_quote_quantity == initialized.quote_quantity
                && !terminal.current_quote_quantity
                && terminal.trade_ids == expected_trade_ids
                && terminal.commissions == expected_commissions
                && terminal
                    .commissions
                    .iter()
                    .all(|(currency, money)| *currency == money.currency)
                && terminal.fills == expected_fills,
            "terminal order {} diverges from its complete causal projection",
            initialized.client_order_id
        );
        Ok(())
    }

    fn ensure_one_causal_followup_per_fill(
        label: &str,
        fill_sequences: &[u64],
        followup_sequences: &[u64],
        terminal_bound: u64,
    ) -> Result<()> {
        ensure!(
            fill_sequences.len() == followup_sequences.len(),
            "#789 recorded {} {label} events for {} fills",
            followup_sequences.len(),
            fill_sequences.len()
        );
        for (index, (&fill_seq, &followup_seq)) in
            fill_sequences.iter().zip(followup_sequences).enumerate()
        {
            let upper_bound = fill_sequences
                .get(index + 1)
                .copied()
                .unwrap_or(terminal_bound);
            ensure!(
                fill_seq < followup_seq && followup_seq < upper_bound,
                "#789 {label} event for fill sequence {fill_seq} is outside its causal interval ({fill_seq}, {upper_bound})"
            );
        }
        Ok(())
    }

    fn record_issue_789_semantic_identity(
        seen: &mut Vec<(UUID4, &'static str)>,
        identity: UUID4,
        label: &'static str,
    ) -> Result<()> {
        if let Some((_, previous_label)) = seen.iter().find(|(seen, _)| *seen == identity) {
            anyhow::bail!(
                "duplicate {label} identity {identity} in #789 evidence; already used by {previous_label}"
            );
        }
        seen.push((identity, label));
        Ok(())
    }

    fn issue_789_submitted_order_trace(
        command: &nautilus_common::messages::execution::SubmitOrder,
        submitted: &nautilus_model::events::OrderSubmitted,
    ) -> Result<crate::execution_contract::SubmittedOrderTrace> {
        let initialized = &command.order_init;
        ensure!(
            command.trader_id == initialized.trader_id
                && command.strategy_id == initialized.strategy_id
                && command.instrument_id == initialized.instrument_id
                && command.client_order_id == initialized.client_order_id
                && command.exec_algorithm_id == initialized.exec_algorithm_id,
            "#789 SubmitOrder envelope diverges from its embedded initialized-order semantics"
        );
        ensure!(
            submitted.trader_id == initialized.trader_id
                && submitted.strategy_id == initialized.strategy_id
                && submitted.instrument_id == initialized.instrument_id
                && submitted.client_order_id == initialized.client_order_id,
            "#789 OrderSubmitted diverges from its causal SubmitOrder"
        );
        Ok(crate::execution_contract::SubmittedOrderTrace {
            trader_id: initialized.trader_id,
            strategy_id: initialized.strategy_id,
            instrument_id: initialized.instrument_id,
            client_order_id: initialized.client_order_id,
            account_id: submitted.account_id,
            order_side: initialized.order_side,
            order_type: initialized.order_type,
            quantity: initialized.quantity,
            quote_quantity: initialized.quote_quantity,
            post_only: initialized.post_only,
            reconciliation: initialized.reconciliation,
        })
    }

    fn issue_789_bind_normal_submission(
        submit_seq: u64,
        command: &nautilus_common::messages::execution::SubmitOrder,
        initialized: Option<&(u64, nautilus_model::events::OrderInitialized)>,
        submitted: Option<&(u64, nautilus_model::events::OrderSubmitted)>,
        accepted: Option<&(u64, nautilus_model::events::OrderAccepted)>,
        first_fill_seq: u64,
    ) -> Result<(u64, crate::execution_contract::SubmittedOrderTrace)> {
        let (initialized_seq, initialized) = initialized.with_context(|| {
            format!(
                "#789 normal order {} lacks OrderInitialized evidence",
                command.client_order_id
            )
        })?;
        let (submitted_seq, submitted) = submitted.with_context(|| {
            format!(
                "#789 normal order {} lacks OrderSubmitted evidence",
                command.client_order_id
            )
        })?;
        ensure!(
            *initialized_seq < submit_seq
                && submit_seq < *submitted_seq
                && *submitted_seq < first_fill_seq,
            "#789 normal order is outside its Initialized-to-SubmitOrder-to-Submitted-to-fill causal order"
        );
        ensure!(
            initialized == &command.order_init,
            "#789 OrderInitialized evidence diverges from its embedded SubmitOrder"
        );
        ensure!(
            accepted.is_none(),
            "#789 normal order {} has unexpected OrderAccepted evidence",
            command.client_order_id
        );
        Ok((
            *submitted_seq,
            issue_789_submitted_order_trace(command, submitted)?,
        ))
    }

    fn issue_789_quote_conversion_in_interval(
        submit_seq: u64,
        first_fill_seq: u64,
        update: Option<&(u64, nautilus_model::events::OrderUpdated)>,
    ) -> Result<Option<nautilus_model::events::OrderUpdated>> {
        update
            .map(|(update_seq, update)| {
                ensure!(
                    submit_seq < *update_seq && *update_seq < first_fill_seq,
                    "#789 OrderUpdated is outside its SubmitOrder-to-fill causal interval"
                );
                Ok(*update)
            })
            .transpose()
    }

    fn record_issue_789_order_update(
        updates: &mut BTreeMap<String, (u64, nautilus_model::events::OrderUpdated)>,
        seq: u64,
        event: nautilus_model::events::OrderUpdated,
    ) -> Result<()> {
        ensure!(
            updates
                .insert(event.client_order_id.to_string(), (seq, event))
                .is_none(),
            "duplicate OrderUpdated client-order identity in #789 event store"
        );
        Ok(())
    }

    fn validate_issue_789_book_domain(
        instrument: &InstrumentAny,
        deltas: &[OrderBookDelta],
    ) -> Result<()> {
        let instrument_id = instrument.id();
        let price_increment = instrument.price_increment().as_decimal();
        let size_increment = instrument.size_increment().as_decimal();
        for (index, delta) in deltas.iter().enumerate() {
            ensure!(
                delta.instrument_id == instrument_id,
                "#789 book delta {index} instrument {} diverges from {instrument_id}",
                delta.instrument_id
            );
            if delta.action == BookAction::Clear {
                continue;
            }
            ensure!(
                matches!(delta.order.side, OrderSide::Buy | OrderSide::Sell),
                "#789 non-Clear book delta {index} has no executable side"
            );

            let price = delta.order.price;
            ensure!(
                price.as_decimal() > Decimal::ZERO
                    && price.precision == instrument.price_precision()
                    && (price.as_decimal() % price_increment).is_zero(),
                "#789 book delta {index} price {price} violates instrument precision/increment"
            );
            if let Some(min_price) = instrument.min_price() {
                ensure!(
                    price >= min_price,
                    "#789 book delta {index} price {price} is below instrument minimum {min_price}"
                );
            }
            if let Some(max_price) = instrument.max_price() {
                ensure!(
                    price <= max_price,
                    "#789 book delta {index} price {price} exceeds instrument maximum {max_price}"
                );
            }

            let size = delta.order.size;
            ensure!(
                size.precision == instrument.size_precision()
                    && (size.as_decimal() % size_increment).is_zero(),
                "#789 book delta {index} size {size} violates instrument precision/increment"
            );
            if matches!(delta.action, BookAction::Add | BookAction::Update) {
                ensure!(
                    size.as_decimal() > Decimal::ZERO,
                    "#789 book delta {index} executable size must be positive"
                );
            }
        }
        Ok(())
    }

    fn ensure_issue_789_venue_shape(venue: &ManifestVenueConfig) -> Result<()> {
        ensure!(
            venue.oms_type == "NETTING"
                && venue.account_type == "CASH"
                && venue.book_type == "L2_MBP"
                && venue.liquidity_consumption
                && !venue.use_market_order_acks
                && venue.fill_model.is_none()
                && venue.latency_model.is_none()
                && venue.fee_model.is_none(),
            "#789 lifecycle evidence is restricted to NETTING/CASH, L2 liquidity consumption, disabled market-order acknowledgements, and deterministic default fill/latency/fee models"
        );
        Ok(())
    }

    fn ensure_issue_789_binary_settlement_domain(
        settlements: &[ManifestInstrumentSettlementInput],
        up_instrument: &InstrumentAny,
        down_instrument: &InstrumentAny,
    ) -> Result<BTreeMap<InstrumentId, Price>> {
        ensure!(
            matches!(up_instrument, InstrumentAny::BinaryOption(_))
                && matches!(down_instrument, InstrumentAny::BinaryOption(_)),
            "#789 settlement projections must both be binary options"
        );
        let expected_instruments = BTreeMap::from([
            (up_instrument.id(), up_instrument),
            (down_instrument.id(), down_instrument),
        ]);
        let expected_ids = expected_instruments
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        ensure!(
            expected_ids.len() == 2,
            "#789 settlement projections must identify two distinct binary legs"
        );
        for instrument in expected_instruments.values() {
            ensure!(
                instrument.quote_currency() == instrument.settlement_currency(),
                "#789 projected binary instrument {} has divergent quote and settlement currencies",
                instrument.id()
            );
            ensure!(
                instrument.taker_fee().is_zero(),
                "#789 projected binary instrument {} must have zero taker fee before NT runs",
                instrument.id()
            );
        }
        ensure!(
            up_instrument.settlement_currency() == down_instrument.settlement_currency(),
            "#789 projected binary legs must share one lifecycle currency"
        );
        let mut bound = BTreeMap::new();
        let mut payoffs = BTreeSet::new();
        for settlement in settlements {
            let instrument_id = InstrumentId::from_str(&settlement.nt_instrument_id)
                .map_err(|error| anyhow::anyhow!(error))
                .context("parse #789 binary settlement instrument")?;
            ensure!(
                expected_ids.contains(&instrument_id),
                "#789 settlement instrument {instrument_id} is not one of the two projected binary legs"
            );
            let instrument = expected_instruments
                .get(&instrument_id)
                .context("resolve #789 projected settlement instrument")?;
            ensure!(
                settlement.settlement_currency == instrument.settlement_currency().to_string(),
                "#789 settlement instrument {instrument_id} currency {} differs from instrument currency {}",
                settlement.settlement_currency,
                instrument.settlement_currency()
            );
            let payoff = Decimal::from_str(&settlement.close_price)
                .context("parse #789 binary settlement payoff")?;
            ensure!(
                payoff == Decimal::ZERO || payoff == Decimal::ONE,
                "#789 binary settlement payoff must be exactly 0 or 1"
            );
            let price = Price::from_str(&settlement.close_price)
                .map_err(|error| anyhow::anyhow!(error))
                .context("parse #789 binary settlement price")?;
            ensure!(
                price.precision == settlement.price_precision,
                "#789 binary settlement price precision diverges from its declaration"
            );
            ensure!(
                bound.insert(instrument_id, price).is_none(),
                "#789 binary settlement contains a duplicate projected leg"
            );
            payoffs.insert(payoff);
        }
        ensure!(
            bound.keys().copied().collect::<BTreeSet<_>>() == expected_ids,
            "#789 binary settlement must exactly cover both projected legs"
        );
        ensure!(
            payoffs == BTreeSet::from([Decimal::ZERO, Decimal::ONE]),
            "#789 paired binary settlements must contain complementary 0 and 1 payoffs"
        );
        Ok(bound)
    }

    fn issue_789_terminal_position<'a>(
        positions: &'a [Position],
        instrument_id: InstrumentId,
    ) -> Result<&'a Position> {
        ensure!(
            positions.len() == 1 && positions[0].instrument_id == instrument_id,
            "issue #789 requires exactly one terminal position for {instrument_id}, got {} total",
            positions.len()
        );
        let position = &positions[0];
        ensure!(
            position.is_closed()
                && position.side == PositionSide::Flat
                && position.quantity.is_zero()
                && position.signed_decimal_qty().is_zero(),
            "issue #789 terminal position must be closed, flat, and zero quantity"
        );
        Ok(position)
    }

    fn ensure_issue_789_terminal_position_matches(
        position: &Position,
        configured_account_id: AccountId,
        expected_position_id: PositionId,
        fills: &[nautilus_model::events::OrderFilled],
    ) -> Result<()> {
        let first_fill = fills
            .first()
            .context("#789 terminal position has no causal entry fill")?;
        let last_fill = fills
            .last()
            .context("#789 terminal position has no causal settlement fill")?;
        let expected_fills = fills.iter().map(issue_789_proof_fill).collect::<Vec<_>>();
        let terminal_fills = position
            .events
            .iter()
            .map(issue_789_proof_fill)
            .collect::<Vec<_>>();
        let replay_fills = position
            .replay_events
            .iter()
            .map(|event| match event {
                PositionReplayEvent::Filled(fill) => Ok(issue_789_proof_fill(fill)),
                PositionReplayEvent::Adjusted(_) => {
                    bail!("terminal position replay contains an unsupported adjustment")
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let expected_trade_ids = fills
            .iter()
            .map(|fill| fill.trade_id)
            .collect::<BTreeSet<_>>();
        let expected_commissions = issue_789_commission_projection(fills)?;
        let mut terminal_commissions = position
            .commissions
            .iter()
            .map(|(currency, money)| (*currency, *money))
            .collect::<Vec<_>>();
        terminal_commissions.sort_by_key(|(currency, _)| currency.to_string());

        ensure!(
            position.trader_id == first_fill.trader_id
                && position.strategy_id == first_fill.strategy_id
                && position.instrument_id == first_fill.instrument_id
                && position.id == expected_position_id
                && position.account_id == configured_account_id
                && position.opening_order_id == first_fill.client_order_id
                && position.closing_order_id == Some(last_fill.client_order_id)
                && position.entry == first_fill.order_side,
            "terminal position causal identity diverges from the lifecycle evidence"
        );
        ensure!(
            position.events.iter().all(|fill| {
                fill.position_id == Some(expected_position_id)
                    && fill.account_id == configured_account_id
            }) && terminal_fills == expected_fills
                && replay_fills == expected_fills
                && position.trade_ids.len() == expected_trade_ids.len()
                && expected_trade_ids
                    .iter()
                    .all(|trade_id| position.trade_ids.contains(trade_id)),
            "terminal position fills or trade identities diverge from causal evidence"
        );
        ensure!(
            position
                .commissions
                .iter()
                .all(|(currency, money)| *currency == money.currency)
                && terminal_commissions == expected_commissions,
            "terminal position keyed commissions diverge from causal fill commissions"
        );
        ensure!(
            position.adjustments.is_empty() && position.fill_voids.is_empty(),
            "terminal position contains unsupported adjustment or fill-void state"
        );
        ensure!(
            position.is_closed()
                && position.side == PositionSide::Flat
                && position.quantity.is_zero()
                && position.signed_decimal_qty().is_zero(),
            "issue #789 terminal position must be closed, flat, and zero quantity"
        );
        Ok(())
    }

    fn ensure_issue_789_payload_type_is_admitted(payload_type: &str) -> Result<()> {
        ensure!(
            matches!(
                payload_type,
                "RunStarted"
                    | "RunEnded"
                    | "SubscribeCommand"
                    | "UnsubscribeCommand"
                    | "TimeEvent"
                    | "SubmitOrder"
                    | "OrderInitialized"
                    | "OrderSubmitted"
                    | "OrderAccepted"
                    | "OrderUpdated"
                    | "OrderFilled"
                    | "InstrumentClose"
                    | "AccountState"
                    | "PositionOpened"
                    | "PositionChanged"
                    | "PositionClosed"
            ),
            "#789 event store contains unsupported payload type {payload_type:?}"
        );
        Ok(())
    }

    fn ensure_issue_789_run_envelope(entries: &[(u64, &str)]) -> Result<()> {
        let started = entries
            .iter()
            .filter(|(_, payload_type)| *payload_type == "RunStarted")
            .count();
        let ended = entries
            .iter()
            .filter(|(_, payload_type)| *payload_type == "RunEnded")
            .count();
        ensure!(
            started == 1 && ended == 1,
            "#789 sealed stream requires exactly one RunStarted and one RunEnded envelope"
        );
        ensure!(
            entries.first().map(|(_, payload_type)| *payload_type) == Some("RunStarted")
                && entries.last().map(|(_, payload_type)| *payload_type) == Some("RunEnded"),
            "#789 store lifecycle envelopes do not bound the complete sealed stream"
        );
        Ok(())
    }

    #[expect(clippy::too_many_arguments)]
    fn ensure_issue_789_settlement_witness(
        client_order_id: &str,
        first_fill_seq: u64,
        fills: &[nautilus_model::events::OrderFilled],
        close_seq: u64,
        close: &nautilus_model::data::InstrumentClose,
        initialized: Option<&(u64, nautilus_model::events::OrderInitialized)>,
        submitted: Option<&(u64, nautilus_model::events::OrderSubmitted)>,
        accepted: Option<&(u64, nautilus_model::events::OrderAccepted)>,
    ) -> Result<()> {
        let first_fill = fills
            .first()
            .with_context(|| format!("#789 settlement order {client_order_id} has no fills"))?;
        ensure!(
            close.close_type == InstrumentCloseType::ContractExpired
                && close.instrument_id == first_fill.instrument_id
                && close_seq > first_fill_seq,
            "#789 settlement fill is not followed by its ContractExpired receipt"
        );
        let (initialized_seq, initialized) = initialized.with_context(|| {
            format!(
                "#789 fill {client_order_id} lacks synthetic expiration OrderInitialized evidence"
            )
        })?;
        let (accepted_seq, accepted) = accepted.with_context(|| {
            format!("#789 fill {client_order_id} lacks synthetic expiration OrderAccepted evidence")
        })?;
        ensure!(
            submitted.is_none(),
            "#789 synthetic expiration order {client_order_id} must not have a normal OrderSubmitted path"
        );
        ensure!(
            *initialized_seq < *accepted_seq
                && *accepted_seq < first_fill_seq
                && first_fill_seq < close_seq,
            "#789 synthetic expiration lifecycle is not ordered Initialized -> Accepted -> Filled -> ContractExpired receipt"
        );
        let expiration_prefix = format!("EXPIRATION-{}-", first_fill.instrument_id.venue);
        let expiration_suffix = client_order_id
            .strip_prefix(&expiration_prefix)
            .with_context(|| {
                format!(
                    "#789 settlement client order ID {client_order_id} lacks prefix {expiration_prefix}"
                )
            })?;
        UUID4::from_str(expiration_suffix)
            .map_err(anyhow::Error::msg)
            .context("#789 settlement client order ID suffix is not a UUID4")?;
        let expected_tag = format!("EXPIRATION_{}_CLOSE", first_fill.instrument_id.venue);
        let filled_quantity = fills
            .iter()
            .map(|fill| fill.last_qty.as_decimal())
            .sum::<Decimal>();
        ensure!(
            initialized.client_order_id == first_fill.client_order_id
                && initialized.instrument_id == first_fill.instrument_id
                && initialized.trader_id == first_fill.trader_id
                && initialized.strategy_id == first_fill.strategy_id
                && initialized.order_side == first_fill.order_side
                && initialized.order_type == nautilus_model::enums::OrderType::Market
                && initialized.time_in_force == nautilus_model::enums::TimeInForce::Gtc
                && initialized.quantity.as_decimal() == filled_quantity
                && initialized.quantity.precision == first_fill.last_qty.precision
                && initialized.reduce_only
                && !initialized.post_only
                && !initialized.quote_quantity
                && !initialized.reconciliation
                && initialized.price.is_none()
                && initialized.activation_price.is_none()
                && initialized.trigger_price.is_none()
                && initialized.trigger_type == Some(nautilus_model::enums::TriggerType::NoTrigger)
                && initialized.limit_offset.is_none()
                && initialized.trailing_offset.is_none()
                && initialized.trailing_offset_type.is_none()
                && initialized.expire_time.is_none()
                && initialized.display_qty.is_none()
                && initialized.emulation_trigger.is_none()
                && initialized.trigger_instrument_id.is_none()
                && initialized.contingency_type.is_none()
                && initialized.order_list_id.is_none()
                && initialized.linked_order_ids.is_none()
                && initialized.parent_order_id.is_none()
                && initialized.exec_algorithm_id.is_none()
                && initialized.exec_algorithm_params.is_none()
                && initialized.exec_spawn_id.is_none()
                && initialized
                    .tags
                    .as_ref()
                    .is_some_and(|tags| { tags.len() == 1 && tags[0].as_str() == expected_tag }),
            "#789 fill {client_order_id} does not match the pinned synthetic expiration OrderInitialized shape"
        );
        ensure!(
            accepted.client_order_id == first_fill.client_order_id
                && accepted.instrument_id == first_fill.instrument_id
                && accepted.trader_id == first_fill.trader_id
                && accepted.strategy_id == first_fill.strategy_id
                && accepted.account_id == first_fill.account_id
                && accepted.venue_order_id == first_fill.venue_order_id
                && !accepted.reconciliation,
            "#789 fill {client_order_id} does not match its synthetic expiration OrderAccepted evidence"
        );
        Ok(())
    }

    #[test]
    fn issue_789_rejects_reused_position_event_identity() {
        let identity = UUID4::new();
        let mut seen = Vec::new();
        record_issue_789_semantic_identity(&mut seen, identity, "PositionOpened")
            .expect("first position event identity is unique");

        let error = record_issue_789_semantic_identity(&mut seen, identity, "PositionChanged")
            .expect_err("position event identities must be globally unique");

        assert!(
            error
                .to_string()
                .contains("duplicate PositionChanged identity")
        );
    }

    fn test_issue_789_submit_order() -> nautilus_common::messages::execution::SubmitOrder {
        let initialized =
            nautilus_model::events::order::spec::OrderInitializedSpec::builder().build();
        nautilus_common::messages::execution::SubmitOrder::new(
            initialized.trader_id,
            None,
            initialized.strategy_id,
            initialized.instrument_id,
            initialized.client_order_id,
            initialized,
            None,
            None,
            None,
            UUID4::new(),
            UnixNanos::default(),
            None,
        )
    }

    fn test_issue_789_order_submitted(
        command: &nautilus_common::messages::execution::SubmitOrder,
    ) -> nautilus_model::events::OrderSubmitted {
        nautilus_model::events::OrderSubmitted::new(
            command.trader_id,
            command.strategy_id,
            command.instrument_id,
            command.client_order_id,
            AccountId::from("POLYMARKET-001"),
            UUID4::new(),
            UnixNanos::default(),
            UnixNanos::default(),
        )
    }

    #[test]
    fn issue_789_rejects_submit_envelope_identity_drift() {
        let mut command = test_issue_789_submit_order();
        let submitted = test_issue_789_order_submitted(&command);
        command.strategy_id = nautilus_model::identifiers::StrategyId::from("OTHER-001");

        let error = issue_789_submitted_order_trace(&command, &submitted)
            .expect_err("SubmitOrder envelope drift must fail closed");
        assert!(error.to_string().contains("envelope diverges"));
    }

    #[test]
    fn issue_789_rejects_embedded_submit_identity_drift() {
        let mut command = test_issue_789_submit_order();
        let submitted = test_issue_789_order_submitted(&command);
        command.order_init.instrument_id = InstrumentId::from("OTHER.POLYMARKET");

        let error = issue_789_submitted_order_trace(&command, &submitted)
            .expect_err("embedded initialized-order drift must fail closed");
        assert!(error.to_string().contains("envelope diverges"));
    }

    #[test]
    fn issue_789_normal_submission_binding_rejects_missing_and_reordered_evidence() {
        let command = test_issue_789_submit_order();
        let submitted = test_issue_789_order_submitted(&command);
        let initialized = command.order_init.clone();

        let missing_initialized =
            issue_789_bind_normal_submission(10, &command, None, Some(&(15, submitted)), None, 20)
                .expect_err("a normal order requires OrderInitialized evidence");
        assert!(
            missing_initialized
                .to_string()
                .contains("lacks OrderInitialized")
        );

        let missing_submitted = issue_789_bind_normal_submission(
            10,
            &command,
            Some(&(5, initialized.clone())),
            None,
            None,
            20,
        )
        .expect_err("a normal order requires OrderSubmitted evidence");
        assert!(
            missing_submitted
                .to_string()
                .contains("lacks OrderSubmitted")
        );

        let reordered_initialized = issue_789_bind_normal_submission(
            10,
            &command,
            Some(&(10, initialized.clone())),
            Some(&(15, submitted)),
            None,
            20,
        )
        .expect_err("OrderInitialized must precede SubmitOrder");
        assert!(reordered_initialized.to_string().contains("causal order"));

        for submitted_seq in [10, 20] {
            let error = issue_789_bind_normal_submission(
                10,
                &command,
                Some(&(5, initialized.clone())),
                Some(&(submitted_seq, submitted)),
                None,
                20,
            )
            .expect_err("OrderSubmitted must remain inside its causal interval");
            assert!(error.to_string().contains("causal order"));
        }
    }

    fn ensure_issue_789_order_event_sets(
        normal_order_ids: &BTreeSet<String>,
        settlement_order_ids: &BTreeSet<String>,
        submit_command_ids: &BTreeSet<String>,
        submitted_event_ids: &BTreeSet<String>,
        accepted_event_ids: &BTreeSet<String>,
        initialized_event_ids: &BTreeSet<String>,
        updated_event_ids: &BTreeSet<String>,
    ) -> Result<()> {
        let all_order_ids = normal_order_ids
            .union(settlement_order_ids)
            .cloned()
            .collect::<BTreeSet<_>>();
        ensure!(
            submit_command_ids == normal_order_ids && submitted_event_ids == normal_order_ids,
            "#789 normal SubmitOrder, OrderSubmitted, and fill identities are not complete in both directions"
        );
        ensure!(
            accepted_event_ids == settlement_order_ids,
            "#789 OrderAccepted evidence does not exactly identify the synthetic settlement"
        );
        ensure!(
            initialized_event_ids == &all_order_ids,
            "#789 OrderInitialized evidence is missing or unrelated to the frozen lifecycle"
        );
        ensure!(
            updated_event_ids.is_subset(normal_order_ids),
            "#789 OrderUpdated evidence is not bound to a normal submitted order"
        );
        Ok(())
    }

    fn ensure_issue_789_instrument_close_set(
        closes: &[(u64, InstrumentClose)],
        expected: &BTreeMap<InstrumentId, Price>,
    ) -> Result<()> {
        ensure!(
            closes.len() == expected.len()
                && closes.iter().all(|(_, close)| {
                    close.close_type == InstrumentCloseType::ContractExpired
                        && expected.get(&close.instrument_id) == Some(&close.close_price)
                })
                && closes
                    .iter()
                    .map(|(_, close)| close.instrument_id)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == closes.len(),
            "#789 InstrumentClose evidence does not exactly match configured binary settlements"
        );
        Ok(())
    }

    fn group_issue_789_fills(
        sequenced_fills: Vec<(u64, nautilus_model::events::OrderFilled)>,
    ) -> Result<Vec<(String, u64, Vec<nautilus_model::events::OrderFilled>)>> {
        let mut grouped = Vec::<(String, u64, Vec<nautilus_model::events::OrderFilled>)>::new();
        for (seq, fill) in sequenced_fills {
            let client_order_id = fill.client_order_id.to_string();
            match grouped.last_mut() {
                Some((current_id, _, fills)) if *current_id == client_order_id => fills.push(fill),
                Some((_, _, _)) => {
                    ensure!(
                        !grouped
                            .iter()
                            .any(|(existing, _, _)| *existing == client_order_id),
                        "#789 event store contains a non-contiguous order fill sequence"
                    );
                    grouped.push((client_order_id, seq, vec![fill]));
                }
                None => grouped.push((client_order_id, seq, vec![fill])),
            }
        }
        Ok(grouped)
    }

    #[test]
    fn issue_789_normal_submission_binding_rejects_identity_drift_and_acceptance() {
        let command = test_issue_789_submit_order();
        let mut submitted = test_issue_789_order_submitted(&command);
        submitted.strategy_id = nautilus_model::identifiers::StrategyId::from("OTHER-001");
        let drift = issue_789_bind_normal_submission(
            10,
            &command,
            Some(&(5, command.order_init.clone())),
            Some(&(15, submitted)),
            None,
            20,
        )
        .expect_err("OrderSubmitted identity drift must fail closed");
        assert!(drift.to_string().contains("OrderSubmitted diverges"));

        let submitted = test_issue_789_order_submitted(&command);
        let accepted = nautilus_model::events::order::spec::OrderAcceptedSpec::builder().build();
        let unexpected = issue_789_bind_normal_submission(
            10,
            &command,
            Some(&(5, command.order_init.clone())),
            Some(&(15, submitted)),
            Some(&(16, accepted)),
            20,
        )
        .expect_err("market-order acknowledgements are outside the frozen #789 shape");
        assert!(unexpected.to_string().contains("unexpected OrderAccepted"));
    }

    #[test]
    fn issue_789_quote_conversion_requires_submit_to_fill_interval() {
        let update = nautilus_model::events::order::spec::OrderUpdatedSpec::builder().build();
        for update_seq in [9, 20] {
            let error = issue_789_quote_conversion_in_interval(10, 20, Some(&(update_seq, update)))
                .expect_err("quote conversion outside the causal interval must fail closed");
            assert!(error.to_string().contains("causal interval"));
        }
    }

    #[test]
    fn issue_789_quote_conversion_accepts_causal_interval() {
        let update = nautilus_model::events::order::spec::OrderUpdatedSpec::builder().build();
        let bound = issue_789_quote_conversion_in_interval(10, 20, Some(&(15, update)))
            .expect("quote conversion inside the causal interval must bind")
            .expect("quote conversion must be retained");
        assert_eq!(bound.event_id, update.event_id);
    }

    #[test]
    fn issue_789_rejects_duplicate_quote_conversion_witnesses() {
        let first = nautilus_model::events::order::spec::OrderUpdatedSpec::builder().build();
        let mut second = first;
        second.event_id = UUID4::new();
        let mut updates = BTreeMap::new();
        record_issue_789_order_update(&mut updates, 10, first)
            .expect("first quote-conversion witness must be recorded");
        let error = record_issue_789_order_update(&mut updates, 11, second)
            .expect_err("a second witness for one order must fail closed");
        assert!(error.to_string().contains("duplicate OrderUpdated"));
    }

    fn issue_789_admission_delta(
        instrument_id: InstrumentId,
        action: BookAction,
        price: &str,
        size: &str,
    ) -> OrderBookDelta {
        OrderBookDelta {
            instrument_id,
            action,
            order: BookOrder::new(OrderSide::Buy, Price::from(price), Quantity::from(size), 1),
            flags: 0,
            sequence: 0,
            ts_event: UnixNanos::from(1),
            ts_init: UnixNanos::from(1),
        }
    }

    fn issue_789_admission_instrument() -> InstrumentAny {
        let mut instrument = nautilus_model::instruments::stubs::binary_option();
        instrument.min_price = Some(Price::from("0.001"));
        instrument.max_price = Some(Price::from("0.999"));
        InstrumentAny::BinaryOption(instrument)
    }

    #[test]
    fn issue_789_replays_marker_cursor_as_book_delta_events() -> Result<()> {
        let instrument_id = issue_789_admission_instrument().id();
        let first = issue_789_admission_delta(instrument_id, BookAction::Add, "0.500", "1.00");
        let mut second = issue_789_admission_delta(instrument_id, BookAction::Add, "0.600", "2.00");
        second.flags = RecordFlag::F_LAST as u8;
        let mut third =
            issue_789_admission_delta(instrument_id, BookAction::Delete, "0.600", "0.00");
        third.flags = RecordFlag::F_LAST as u8;

        let first_event =
            replay_executable_book_at_cursor(instrument_id, &[first, second, third], 1)?;
        assert_eq!(first_event.best_bid_price(), Some(Price::from("0.600")));

        let second_event =
            replay_executable_book_at_cursor(instrument_id, &[first, second, third], 2)?;
        assert_eq!(second_event.best_bid_price(), Some(Price::from("0.500")));
        Ok(())
    }

    #[test]
    fn issue_789_static_admission_rejects_invalid_executable_book_values() {
        let instrument = issue_789_admission_instrument();
        let instrument_id = instrument.id();
        let mutations = [
            (
                "zero price",
                issue_789_admission_delta(instrument_id, BookAction::Add, "0.000", "1.00"),
            ),
            (
                "out-of-range price",
                issue_789_admission_delta(instrument_id, BookAction::Add, "1.500", "1.00"),
            ),
            (
                "price precision",
                issue_789_admission_delta(instrument_id, BookAction::Add, "0.4210", "1.00"),
            ),
            (
                "size precision",
                issue_789_admission_delta(instrument_id, BookAction::Add, "0.421", "0.001"),
            ),
        ];
        for (label, delta) in mutations {
            assert!(
                validate_issue_789_book_domain(&instrument, std::slice::from_ref(&delta)).is_err(),
                "{label} must fail before NT runs"
            );
        }
    }

    #[test]
    fn issue_789_static_admission_accepts_structural_clear_and_zero_size_delete() {
        let instrument = issue_789_admission_instrument();
        let instrument_id = instrument.id();
        let deltas = [
            OrderBookDelta::clear(instrument_id, 0, UnixNanos::from(1), UnixNanos::from(1)),
            issue_789_admission_delta(instrument_id, BookAction::Delete, "0.421", "0.00"),
        ];
        validate_issue_789_book_domain(&instrument, &deltas)
            .expect("structural clear and zero-size delete must remain admissible");
    }

    #[test]
    fn issue_789_binary_settlement_requires_payoff_endpoints() {
        let settlement = |close_price: &str| ManifestInstrumentSettlementInput {
            nt_instrument_id: "YES.POLYMARKET".to_string(),
            close_price: close_price.to_string(),
            price_precision: 3,
            ts_event_ns: 1,
            ts_init_ns: 1,
            settlement_currency: "USDC".to_string(),
        };

        let mut yes = nautilus_model::instruments::stubs::binary_option();
        yes.id = InstrumentId::from("YES.POLYMARKET");
        let mut no = yes.clone();
        no.id = InstrumentId::from("NO.POLYMARKET");
        let yes = InstrumentAny::BinaryOption(yes);
        let no = InstrumentAny::BinaryOption(no);

        ensure_issue_789_binary_settlement_domain(
            &[settlement("0.000"), {
                let mut row = settlement("1.000");
                row.nt_instrument_id = "NO.POLYMARKET".to_string();
                row
            }],
            &yes,
            &no,
        )
        .expect("binary settlement endpoints must be admitted");
        ensure_issue_789_binary_settlement_domain(&[settlement("0.500")], &yes, &no)
            .expect_err("a fractional binary payoff must fail closed");

        for invalid in [
            vec![settlement("0.000")],
            vec![settlement("0.000"), settlement("1.000")],
            vec![settlement("0.000"), {
                let mut row = settlement("1.000");
                row.nt_instrument_id = "OTHER.POLYMARKET".to_string();
                row
            }],
            vec![settlement("1.000"), {
                let mut row = settlement("1.000");
                row.nt_instrument_id = "NO.POLYMARKET".to_string();
                row
            }],
        ] {
            ensure_issue_789_binary_settlement_domain(&invalid, &yes, &no)
                .expect_err("missing, duplicate, unrelated, or non-complementary legs must fail");
        }
    }

    #[test]
    fn issue_789_binary_settlement_rejects_currency_drift_for_each_leg() {
        let mut yes = nautilus_model::instruments::stubs::binary_option();
        yes.id = InstrumentId::from("YES.POLYMARKET");
        let mut no = yes.clone();
        no.id = InstrumentId::from("NO.POLYMARKET");
        let yes = InstrumentAny::BinaryOption(yes);
        let no = InstrumentAny::BinaryOption(no);
        let settlements = [
            ManifestInstrumentSettlementInput {
                nt_instrument_id: yes.id().to_string(),
                close_price: "0.000".to_string(),
                price_precision: 3,
                ts_event_ns: 1,
                ts_init_ns: 1,
                settlement_currency: yes.settlement_currency().to_string(),
            },
            ManifestInstrumentSettlementInput {
                nt_instrument_id: no.id().to_string(),
                close_price: "1.000".to_string(),
                price_precision: 3,
                ts_event_ns: 1,
                ts_init_ns: 1,
                settlement_currency: no.settlement_currency().to_string(),
            },
        ];

        for index in 0..settlements.len() {
            let mut drifted = settlements.clone();
            drifted[index].settlement_currency = "EUR".to_string();
            let error = ensure_issue_789_binary_settlement_domain(&drifted, &yes, &no)
                .expect_err("each settlement row must use its instrument currency");
            assert!(error.to_string().contains("instrument currency"));
        }
    }

    #[test]
    fn issue_789_binary_settlement_rejects_correlated_instrument_currency_drift() {
        let mut yes = nautilus_model::instruments::stubs::binary_option();
        yes.id = InstrumentId::from("YES.POLYMARKET");
        let mut no = yes.clone();
        no.id = InstrumentId::from("NO.POLYMARKET");

        for mutate_yes in [true, false] {
            let mut drifted_yes = yes.clone();
            let mut drifted_no = no.clone();
            let drifted_id = if mutate_yes {
                drifted_yes.currency = Currency::EUR();
                drifted_yes.id
            } else {
                drifted_no.currency = Currency::EUR();
                drifted_no.id
            };
            let settlements = [
                ManifestInstrumentSettlementInput {
                    nt_instrument_id: drifted_yes.id.to_string(),
                    close_price: "0.000".to_string(),
                    price_precision: 3,
                    ts_event_ns: 1,
                    ts_init_ns: 1,
                    settlement_currency: drifted_yes.currency.to_string(),
                },
                ManifestInstrumentSettlementInput {
                    nt_instrument_id: drifted_no.id.to_string(),
                    close_price: "1.000".to_string(),
                    price_precision: 3,
                    ts_event_ns: 1,
                    ts_init_ns: 1,
                    settlement_currency: drifted_no.currency.to_string(),
                },
            ];

            let error = ensure_issue_789_binary_settlement_domain(
                &settlements,
                &InstrumentAny::BinaryOption(drifted_yes),
                &InstrumentAny::BinaryOption(drifted_no),
            )
            .expect_err("both projected legs must share one lifecycle currency");
            assert!(error.to_string().contains("one lifecycle currency"));
            assert!(settlements.iter().any(|row| {
                row.nt_instrument_id == drifted_id.to_string()
                    && row.settlement_currency == Currency::EUR().to_string()
            }));
        }
    }

    #[test]
    fn issue_789_binary_settlement_rejects_nonzero_taker_fee_for_each_leg() {
        let mut yes = nautilus_model::instruments::stubs::binary_option();
        yes.id = InstrumentId::from("YES.POLYMARKET");
        let mut no = yes.clone();
        no.id = InstrumentId::from("NO.POLYMARKET");
        let settlements = [
            ManifestInstrumentSettlementInput {
                nt_instrument_id: yes.id.to_string(),
                close_price: "0.000".to_string(),
                price_precision: 3,
                ts_event_ns: 1,
                ts_init_ns: 1,
                settlement_currency: yes.currency.to_string(),
            },
            ManifestInstrumentSettlementInput {
                nt_instrument_id: no.id.to_string(),
                close_price: "1.000".to_string(),
                price_precision: 3,
                ts_event_ns: 1,
                ts_init_ns: 1,
                settlement_currency: no.currency.to_string(),
            },
        ];

        for mutate_yes in [true, false] {
            let mut drifted_yes = yes.clone();
            let mut drifted_no = no.clone();
            if mutate_yes {
                drifted_yes.taker_fee = Decimal::new(1, 2);
            } else {
                drifted_no.taker_fee = Decimal::new(1, 2);
            }
            let error = ensure_issue_789_binary_settlement_domain(
                &settlements,
                &InstrumentAny::BinaryOption(drifted_yes),
                &InstrumentAny::BinaryOption(drifted_no),
            )
            .expect_err("both projected legs must have zero taker fees before NT runs");
            assert!(error.to_string().contains("zero taker fee"));
        }
    }

    #[test]
    fn issue_789_terminal_position_requires_complete_closed_flat_projection() {
        let instrument = issue_789_admission_instrument();
        let instrument_id = instrument.id();
        let mut fill = nautilus_model::events::order::spec::OrderFilledSpec::builder().build();
        fill.instrument_id = instrument_id;
        fill.position_id = Some(PositionId::from("P-789"));
        fill.commission = Some(Money::from("0.00 USD"));
        let mut closed = Position::new(&instrument, fill.clone());
        closed.side = PositionSide::Flat;
        closed.quantity = Quantity::zero(instrument.size_precision());
        closed.signed_qty = 0.0;
        closed.ts_closed = Some(UnixNanos::from(2));
        closed.closing_order_id = Some(fill.client_order_id);

        issue_789_terminal_position(std::slice::from_ref(&closed), instrument_id)
            .expect("one closed flat terminal position must be admitted");
        ensure_issue_789_terminal_position_matches(
            &closed,
            fill.account_id,
            PositionId::from("P-789"),
            std::slice::from_ref(&fill),
        )
        .expect("finite terminal position projection must match causal evidence");

        let mutations: Vec<Box<dyn Fn(&mut Position)>> = vec![
            Box::new(|position| position.trader_id = TraderId::from("OTHER-001")),
            Box::new(|position| position.strategy_id = StrategyId::from("OTHER-001")),
            Box::new(|position| position.opening_order_id = ClientOrderId::from("OTHER-001")),
            Box::new(|position| position.closing_order_id = Some(ClientOrderId::from("OTHER-001"))),
            Box::new(|position| position.entry = OrderSide::Sell),
            Box::new(|position| position.trade_ids.clear()),
            Box::new(|position| position.events[0].last_px = Price::from("0.990")),
            Box::new(|position| {
                let money = *position
                    .commissions
                    .values()
                    .next()
                    .expect("test commission");
                position.commissions.clear();
                position.commissions.insert(Currency::EUR(), money);
            }),
            Box::new(|position| {
                position.adjustments.push(PositionAdjusted::new(
                    position.trader_id,
                    position.strategy_id,
                    position.instrument_id,
                    position.id,
                    position.account_id,
                    PositionAdjustmentType::Funding,
                    None,
                    None,
                    None,
                    UUID4::new(),
                    UnixNanos::from(3),
                    UnixNanos::from(3),
                ));
            }),
            Box::new(|position| {
                position
                    .replay_events
                    .push(PositionReplayEvent::Filled(position.events[0].clone()));
            }),
            Box::new(|position| {
                position.fill_voids.push(PositionFillVoid {
                    event: nautilus_model::events::order::spec::OrderFillVoidedSpec::builder()
                        .build(),
                    voided_qty: Quantity::from("1.00"),
                    commission_voided: None,
                });
            }),
        ];
        for mutate in mutations {
            let mut candidate = closed.clone();
            mutate(&mut candidate);
            ensure_issue_789_terminal_position_matches(
                &candidate,
                fill.account_id,
                PositionId::from("P-789"),
                std::slice::from_ref(&fill),
            )
            .expect_err("terminal position projection drift must fail closed");
        }

        let mut nonflat = closed.clone();
        nonflat.side = PositionSide::Long;
        nonflat.quantity = Quantity::from("1.00");
        nonflat.signed_qty = 1.0;
        nonflat.ts_closed = None;
        issue_789_terminal_position(&[nonflat], instrument_id)
            .expect_err("a nonflat terminal position must fail closed");

        let mut unrelated = closed.clone();
        unrelated.instrument_id = InstrumentId::from("OTHER.POLYMARKET");
        issue_789_terminal_position(&[closed, unrelated], instrument_id)
            .expect_err("an extra terminal position must fail closed");
    }

    #[test]
    fn issue_789_static_admission_rejects_correct_precision_wrong_increment() {
        let mut instrument = nautilus_model::instruments::stubs::binary_option();
        instrument.price_increment = Price::from("0.002");
        instrument.size_increment = Quantity::from("0.02");
        instrument.min_price = Some(Price::from("0.001"));
        instrument.max_price = Some(Price::from("0.999"));
        let instrument = InstrumentAny::BinaryOption(instrument);
        let instrument_id = instrument.id();

        for delta in [
            issue_789_admission_delta(instrument_id, BookAction::Add, "0.421", "1.00"),
            issue_789_admission_delta(instrument_id, BookAction::Add, "0.420", "1.01"),
        ] {
            validate_issue_789_book_domain(&instrument, &[delta])
                .expect_err("correct precision with wrong increment must fail before NT runs");
        }
    }

    #[test]
    fn issue_789_static_admission_rejects_unsupported_run_shape() {
        let mut venue = issue_789_venue("POLYMARKET", "pUSD", "L2_MBP", true, true);
        ensure_issue_789_venue_shape(&venue).expect("declared #789 venue shape must be admitted");

        venue.oms_type = "HEDGING".to_string();
        let error = ensure_issue_789_venue_shape(&venue)
            .expect_err("unsupported run shape must fail before NT runs");
        assert!(error.to_string().contains("restricted to NETTING/CASH"));
    }

    #[test]
    fn issue_789_static_admission_rejects_market_order_acknowledgements() {
        let mut venue = issue_789_venue("POLYMARKET", "pUSD", "L2_MBP", true, true);
        venue.use_market_order_acks = true;

        let error = ensure_issue_789_venue_shape(&venue)
            .expect_err("market-order acknowledgements change the frozen normal-order grammar");
        assert!(error.to_string().contains("market-order acknowledgements"));
    }

    #[test]
    fn issue_789_static_admission_rejects_no_side_non_clear_delta() {
        let instrument = issue_789_admission_instrument();
        let mut delta =
            issue_789_admission_delta(instrument.id(), BookAction::Update, "0.421", "1.00");
        delta.order.side = OrderSide::NoOrderSide;

        let error = validate_issue_789_book_domain(&instrument, &[delta])
            .expect_err("non-Clear deltas without an executable side must fail before NT runs");
        assert!(error.to_string().contains("executable side"));
    }

    #[test]
    fn issue_789_requires_configured_execution_account_before_run() {
        let venue = Venue::from("POLYMARKET");
        let account_id = AccountId::from("POLYMARKET-001");
        assert_eq!(
            require_pre_run_configured_account(Some(account_id), venue)
                .expect("configured account must be retained"),
            account_id
        );
        require_pre_run_configured_account(None, venue)
            .expect_err("missing configured account must fail before BacktestNode::run");
    }

    #[test]
    fn issue_789_closed_intake_rejects_unknown_payload_type() {
        let error = ensure_issue_789_payload_type_is_admitted("OrderRejected")
            .expect_err("out-of-scope lifecycle payload must fail closed");
        assert!(error.to_string().contains("unsupported payload type"));
    }

    #[test]
    fn issue_789_closed_intake_admits_store_lifecycle_envelopes() {
        for payload_type in [
            "RunStarted",
            "RunEnded",
            "SubscribeCommand",
            "UnsubscribeCommand",
            "TimeEvent",
        ] {
            ensure_issue_789_payload_type_is_admitted(payload_type)
                .expect("store lifecycle envelope must be admitted for integrity validation");
        }
    }

    #[test]
    fn issue_789_run_envelope_requires_unique_stream_boundaries() {
        ensure_issue_789_run_envelope(&[(1, "RunStarted"), (2, "TimeEvent"), (3, "RunEnded")])
            .expect("unique boundary envelopes must contain the sealed stream");

        for invalid in [
            vec![(1, "TimeEvent"), (2, "RunEnded")],
            vec![(1, "RunStarted"), (2, "TimeEvent")],
            vec![(1, "RunStarted"), (2, "RunStarted"), (3, "RunEnded")],
            vec![(1, "RunEnded"), (2, "TimeEvent"), (3, "RunStarted")],
            vec![(1, "TimeEvent"), (2, "RunStarted"), (3, "RunEnded")],
            vec![(1, "RunStarted"), (2, "RunEnded"), (3, "TimeEvent")],
        ] {
            ensure_issue_789_run_envelope(&invalid)
                .expect_err("missing, duplicate, reversed, or non-boundary envelopes must fail");
        }
    }

    #[test]
    fn issue_789_order_event_sets_reject_missing_and_unexplained_evidence() {
        let normal = BTreeSet::from(["ENTRY".to_string(), "EXIT".to_string()]);
        let settlement = BTreeSet::from(["SETTLEMENT".to_string()]);
        let all = BTreeSet::from([
            "ENTRY".to_string(),
            "EXIT".to_string(),
            "SETTLEMENT".to_string(),
        ]);
        let accepted = settlement.clone();
        let updated = BTreeSet::from(["ENTRY".to_string()]);
        ensure_issue_789_order_event_sets(
            &normal,
            &settlement,
            &normal,
            &normal,
            &accepted,
            &all,
            &updated,
        )
        .expect("complete frozen order grammar must pass");

        let cases = [
            (
                BTreeSet::from(["ENTRY".to_string()]),
                normal.clone(),
                accepted.clone(),
                all.clone(),
                updated.clone(),
            ),
            (
                normal.clone(),
                BTreeSet::from(["ENTRY".to_string()]),
                accepted.clone(),
                all.clone(),
                updated.clone(),
            ),
            (
                normal.clone(),
                normal.clone(),
                BTreeSet::from(["OTHER".to_string()]),
                all.clone(),
                updated.clone(),
            ),
            (
                normal.clone(),
                normal.clone(),
                accepted.clone(),
                BTreeSet::from(["ENTRY".to_string(), "EXIT".to_string()]),
                updated.clone(),
            ),
            (
                normal.clone(),
                normal.clone(),
                accepted,
                all,
                BTreeSet::from(["OTHER".to_string()]),
            ),
        ];
        for (submits, submitted, accepted, initialized, updated) in cases {
            ensure_issue_789_order_event_sets(
                &normal,
                &settlement,
                &submits,
                &submitted,
                &accepted,
                &initialized,
                &updated,
            )
            .expect_err("missing or unexplained order evidence must fail closed");
        }
    }

    #[test]
    fn issue_789_instrument_close_set_binds_both_binary_legs() {
        let first_id = InstrumentId::from("YES.POLYMARKET");
        let second_id = InstrumentId::from("NO.POLYMARKET");
        let expected = BTreeMap::from([
            (first_id, Price::from("1.000")),
            (second_id, Price::from("0.000")),
        ]);
        let closes = vec![
            (
                10,
                InstrumentClose::new(
                    first_id,
                    Price::from("1.000"),
                    InstrumentCloseType::ContractExpired,
                    UnixNanos::from(10),
                    UnixNanos::from(10),
                ),
            ),
            (
                11,
                InstrumentClose::new(
                    second_id,
                    Price::from("0.000"),
                    InstrumentCloseType::ContractExpired,
                    UnixNanos::from(11),
                    UnixNanos::from(11),
                ),
            ),
        ];
        ensure_issue_789_instrument_close_set(&closes, &expected)
            .expect("both configured binary close receipts must be assigned");

        ensure_issue_789_instrument_close_set(&closes[..1], &expected)
            .expect_err("a missing paired-market close receipt must fail closed");
    }

    #[test]
    fn issue_789_rejects_non_contiguous_fill_groups() {
        let mut first = nautilus_model::events::order::spec::OrderFilledSpec::builder().build();
        first.client_order_id = nautilus_model::identifiers::ClientOrderId::from("ORDER-A");
        let mut second = first.clone();
        second.client_order_id = nautilus_model::identifiers::ClientOrderId::from("ORDER-B");
        let third = first.clone();

        let error = group_issue_789_fills(vec![(1, first), (2, second), (3, third)])
            .expect_err("an order's fills must form one contiguous causal group");
        assert!(error.to_string().contains("non-contiguous"));
    }

    fn test_issue_789_terminal_order() -> (
        OrderTerminalRecord,
        nautilus_model::events::OrderInitialized,
        Vec<nautilus_model::events::OrderFilled>,
    ) {
        let mut fill = nautilus_model::events::order::spec::OrderFilledSpec::builder().build();
        fill.position_id = Some(PositionId::from("P-789"));
        fill.commission = Some(Money::from("0.00 USD"));
        let mut initialized =
            nautilus_model::events::order::spec::OrderInitializedSpec::builder().build();
        initialized.trader_id = fill.trader_id;
        initialized.strategy_id = fill.strategy_id;
        initialized.instrument_id = fill.instrument_id;
        initialized.client_order_id = fill.client_order_id;
        initialized.order_side = fill.order_side;
        initialized.order_type = fill.order_type;
        initialized.quantity = fill.last_qty;
        initialized.quote_quantity = false;
        let terminal = OrderTerminalRecord {
            trader_id: fill.trader_id,
            strategy_id: fill.strategy_id,
            instrument_id: fill.instrument_id,
            client_order_id: fill.client_order_id,
            account_id: Some(fill.account_id),
            venue_order_id: Some(fill.venue_order_id),
            position_id: fill.position_id,
            order_side: fill.order_side,
            order_type: fill.order_type,
            status: OrderStatus::Filled,
            quantity: fill.last_qty,
            filled_qty: fill.last_qty,
            leaves_qty: Quantity::zero(fill.last_qty.precision),
            initialized_quantity: initialized.quantity,
            initialized_quote_quantity: initialized.quote_quantity,
            current_quote_quantity: false,
            trade_ids: vec![fill.trade_id],
            commissions: vec![(
                fill.currency,
                fill.commission.expect("test fill commission"),
            )],
            fills: vec![issue_789_proof_fill(&fill)],
            events_debug: Vec::new(),
        };
        (terminal, initialized, vec![fill])
    }

    #[test]
    fn issue_789_terminal_order_accepts_complete_projection() {
        let (terminal, initialized, fills) = test_issue_789_terminal_order();
        ensure_issue_789_terminal_order_matches(
            &terminal,
            &initialized,
            fills[0].account_id,
            fills[0].position_id.expect("test fill position"),
            initialized.quantity,
            Issue789ExecutionQuantityPolicy::Exact,
            &fills,
        )
        .expect("complete terminal order projection must match");
    }

    #[test]
    fn issue_789_canonical_fill_excludes_transport_metadata() {
        let fill = nautilus_model::events::order::spec::OrderFilledSpec::builder().build();
        let mut transport_variant = fill.clone();
        transport_variant.position_id = Some(PositionId::from("P-OTHER"));
        transport_variant.ts_event = UnixNanos::from(99);
        transport_variant.ts_init = UnixNanos::from(100);
        transport_variant.info = Some(Default::default());
        transport_variant.causation_id = Some(UUID4::new());

        assert_eq!(
            issue_789_proof_fill(&fill),
            issue_789_proof_fill(&transport_variant)
        );
    }

    #[test]
    fn issue_789_terminal_order_rejects_metadata_and_quantity_drift() {
        let (terminal, initialized, fills) = test_issue_789_terminal_order();
        let mutations: Vec<TerminalOrderMutation> = vec![
            Box::new(|terminal| terminal.trader_id = TraderId::from("OTHER-001")),
            Box::new(|terminal| terminal.strategy_id = StrategyId::from("OTHER-001")),
            Box::new(|terminal| terminal.instrument_id = InstrumentId::from("OTHER.POLYMARKET")),
            Box::new(|terminal| terminal.client_order_id = ClientOrderId::from("OTHER-001")),
            Box::new(|terminal| terminal.account_id = None),
            Box::new(|terminal| terminal.venue_order_id = None),
            Box::new(|terminal| terminal.position_id = None),
            Box::new(|terminal| terminal.order_side = OrderSide::Sell),
            Box::new(|terminal| terminal.order_type = OrderType::Limit),
            Box::new(|terminal| terminal.status = OrderStatus::Canceled),
            Box::new(|terminal| terminal.quantity = Quantity::from("2.00")),
            Box::new(|terminal| terminal.filled_qty = Quantity::from("2.00")),
            Box::new(|terminal| terminal.leaves_qty = Quantity::from("1.00")),
            Box::new(|terminal| terminal.initialized_quantity = Quantity::from("2.00")),
            Box::new(|terminal| terminal.initialized_quote_quantity = true),
            Box::new(|terminal| terminal.current_quote_quantity = true),
            Box::new(|terminal| terminal.trade_ids.clear()),
            Box::new(|terminal| {
                terminal.commissions[0].0 = Currency::EUR();
            }),
            Box::new(|terminal| {
                terminal.fills.clear();
            }),
            Box::new(|terminal| {
                terminal.fills[0].price = Price::from("0.990");
            }),
            Box::new(|terminal| terminal.fills[0].event_id = UUID4::new()),
        ];
        for mutate in mutations {
            let mut candidate = terminal.clone();
            mutate(&mut candidate);
            ensure_issue_789_terminal_order_matches(
                &candidate,
                &initialized,
                fills[0].account_id,
                fills[0].position_id.expect("test fill position"),
                initialized.quantity,
                Issue789ExecutionQuantityPolicy::Exact,
                &fills,
            )
            .expect_err("terminal order projection drift must fail closed");
        }
    }

    #[test]
    fn issue_789_terminal_order_rejects_filled_status_with_partial_quantity() {
        let (mut terminal, initialized, fills) = test_issue_789_terminal_order();
        terminal.quantity = Quantity::from("2.00");

        let error = ensure_issue_789_terminal_order_matches(
            &terminal,
            &initialized,
            fills[0].account_id,
            fills[0].position_id.expect("test fill position"),
            terminal.quantity,
            Issue789ExecutionQuantityPolicy::Exact,
            &fills,
        )
        .expect_err("Filled requires the complete effective quantity");
        assert!(error.to_string().contains("complete causal projection"));
    }

    #[test]
    fn issue_789_rejects_orphan_normal_close_as_settlement() {
        let fill = nautilus_model::events::order::spec::OrderFilledSpec::builder().build();
        let close = InstrumentClose::new(
            fill.instrument_id,
            fill.last_px,
            InstrumentCloseType::ContractExpired,
            UnixNanos::from(20),
            UnixNanos::from(20),
        );
        let client_order_id = fill.client_order_id.to_string();

        let error = ensure_issue_789_settlement_witness(
            &client_order_id,
            10,
            &[fill],
            20,
            &close,
            None,
            None,
            None,
        )
        .expect_err("a fill without synthetic expiration-order evidence is not settlement");

        assert!(error.to_string().contains("synthetic expiration"));
    }

    fn test_issue_789_settlement_witness() -> (
        String,
        nautilus_model::events::OrderFilled,
        InstrumentClose,
        nautilus_model::events::OrderInitialized,
        nautilus_model::events::OrderAccepted,
    ) {
        let mut fill = nautilus_model::events::order::spec::OrderFilledSpec::builder().build();
        let client_order_id = format!("EXPIRATION-{}-{}", fill.instrument_id.venue, UUID4::new());
        fill.client_order_id =
            nautilus_model::identifiers::ClientOrderId::from(client_order_id.as_str());
        let mut initialized =
            nautilus_model::events::order::spec::OrderInitializedSpec::builder().build();
        initialized.trader_id = fill.trader_id;
        initialized.strategy_id = fill.strategy_id;
        initialized.instrument_id = fill.instrument_id;
        initialized.client_order_id = fill.client_order_id;
        initialized.order_side = fill.order_side;
        initialized.order_type = OrderType::Market;
        initialized.time_in_force = nautilus_model::enums::TimeInForce::Gtc;
        initialized.quantity = fill.last_qty;
        initialized.reduce_only = true;
        initialized.post_only = false;
        initialized.quote_quantity = false;
        initialized.reconciliation = false;
        initialized.trigger_type = Some(nautilus_model::enums::TriggerType::NoTrigger);
        initialized.tags = Some(vec![ustr::Ustr::from(
            format!("EXPIRATION_{}_CLOSE", fill.instrument_id.venue).as_str(),
        )]);
        let mut accepted =
            nautilus_model::events::order::spec::OrderAcceptedSpec::builder().build();
        accepted.trader_id = fill.trader_id;
        accepted.strategy_id = fill.strategy_id;
        accepted.instrument_id = fill.instrument_id;
        accepted.client_order_id = fill.client_order_id;
        accepted.venue_order_id = fill.venue_order_id;
        accepted.account_id = fill.account_id;
        accepted.reconciliation = false;
        let close = InstrumentClose::new(
            fill.instrument_id,
            fill.last_px,
            InstrumentCloseType::ContractExpired,
            UnixNanos::from(20),
            UnixNanos::from(20),
        );
        (client_order_id, fill, close, initialized, accepted)
    }

    #[test]
    fn issue_789_accepts_the_pinned_synthetic_settlement_witness() {
        let (client_order_id, fill, close, initialized, accepted) =
            test_issue_789_settlement_witness();
        ensure_issue_789_settlement_witness(
            &client_order_id,
            10,
            &[fill],
            20,
            &close,
            Some(&(5, initialized)),
            None,
            Some(&(8, accepted)),
        )
        .expect("pinned synthetic expiration evidence must be admitted");
    }

    #[test]
    fn issue_789_rejects_settlement_origin_shape_and_account_drift() {
        let mutations: Vec<SettlementWitnessMutation> = vec![
            Box::new(|initialized, _| initialized.reduce_only = false),
            Box::new(|initialized, _| initialized.tags = None),
            Box::new(|initialized, _| initialized.quantity = Quantity::from("1.00")),
            Box::new(|initialized, _| initialized.price = Some(Price::from("0.500"))),
            Box::new(|_, accepted| accepted.account_id = AccountId::from("OTHER-001")),
            Box::new(|_, accepted| accepted.reconciliation = true),
        ];
        for mutate in mutations {
            let (client_order_id, fill, close, mut initialized, mut accepted) =
                test_issue_789_settlement_witness();
            mutate(&mut initialized, &mut accepted);
            ensure_issue_789_settlement_witness(
                &client_order_id,
                10,
                &[fill],
                20,
                &close,
                Some(&(5, initialized)),
                None,
                Some(&(8, accepted)),
            )
            .expect_err("synthetic settlement witness drift must fail closed");
        }
    }

    fn bind_issue_789_account_states<'a>(
        fill_sequences: &[u64],
        position_effect_sequences: &[u64],
        account_states: &'a [(u64, AccountState)],
        terminal_bound: u64,
    ) -> Result<Vec<&'a (u64, AccountState)>> {
        ensure!(
            !fill_sequences.is_empty(),
            "#789 cannot bind AccountState evidence without fills"
        );
        ensure_one_causal_followup_per_fill(
            "position mutation",
            fill_sequences,
            position_effect_sequences,
            terminal_bound,
        )?;
        ensure!(
            account_states
                .windows(2)
                .all(|states| states[0].0 < states[1].0),
            "#789 AccountState evidence is not strictly sequence ordered"
        );

        let same_projection = |left: &AccountState, right: &AccountState| {
            left.account_id == right.account_id
                && left.account_type == right.account_type
                && left.base_currency == right.base_currency
                && left.has_same_balances_and_margins(right)
        };
        let first_fill = fill_sequences[0];
        let initial_states = account_states
            .iter()
            .take_while(|(seq, _)| *seq < first_fill)
            .collect::<Vec<_>>();
        ensure!(
            !initial_states.is_empty(),
            "#789 lifecycle has no AccountState before its first fill"
        );
        ensure!(
            initial_states
                .iter()
                .all(|state| same_projection(&initial_states[0].1, &state.1)),
            "#789 has conflicting initial AccountState evidence: {:?}",
            initial_states
                .iter()
                .map(|(seq, state)| (
                    seq,
                    state.account_type,
                    state.base_currency,
                    state.is_reported,
                    &state.balances,
                    &state.margins,
                ))
                .collect::<Vec<_>>()
        );

        let mut bound = vec![
            *initial_states
                .last()
                .context("#789 initial AccountState sequence unexpectedly empty")?,
        ];
        let mut assigned_count = initial_states.len();
        let mut seen_event_ids = initial_states
            .iter()
            .map(|(_, state)| state.event_id)
            .collect::<Vec<_>>();
        for (index, (&fill_seq, &position_effect_seq)) in fill_sequences
            .iter()
            .zip(position_effect_sequences)
            .enumerate()
        {
            let upper_bound = fill_sequences
                .get(index + 1)
                .copied()
                .unwrap_or(terminal_bound);
            let candidates = account_states
                .iter()
                .filter(|(seq, _)| position_effect_seq < *seq && *seq < upper_bound)
                .collect::<Vec<_>>();
            ensure!(
                !candidates.is_empty(),
                "#789 fill sequence {fill_seq} has no AccountState after its position mutation {position_effect_seq} before {upper_bound}"
            );
            ensure!(
                candidates
                    .iter()
                    .all(|state| same_projection(&candidates[0].1, &state.1)),
                "#789 fill sequence {fill_seq} has conflicting AccountState evidence in its causal interval"
            );
            ensure!(
                candidates
                    .iter()
                    .all(|(_, state)| !seen_event_ids.contains(&state.event_id)),
                "#789 fill sequence {fill_seq} contains a replayed AccountState event identity"
            );
            for (_, state) in &candidates {
                seen_event_ids.push(state.event_id);
            }
            assigned_count += candidates.len();
            bound.push(
                *candidates
                    .last()
                    .context("#789 AccountState causal interval unexpectedly empty")?,
            );
        }
        ensure!(
            assigned_count == account_states.len(),
            "#789 contains AccountState evidence outside the ordered fill -> position mutation -> account transition lifecycle"
        );
        Ok(bound)
    }

    fn ensure_issue_789_account_state_scope(
        account_states: &[(u64, AccountState)],
        lifecycle_account_id: AccountId,
        first_submit_seq: u64,
    ) -> Result<()> {
        ensure!(
            account_states.iter().all(|(seq, state)| {
                state.account_id == lifecycle_account_id || *seq < first_submit_seq
            }),
            "#789 event store contains unrelated AccountState evidence outside the pre-submission initialization prefix"
        );
        Ok(())
    }

    fn test_issue_789_account_state(balances: Vec<AccountBalance>) -> AccountState {
        AccountState::new(
            AccountId::from("POLYMARKET-001"),
            AccountType::Cash,
            balances,
            Vec::new(),
            true,
            UUID4::default(),
            UnixNanos::from(1),
            UnixNanos::from(1),
            Some(Currency::USDC()),
        )
    }

    fn test_issue_789_account_state_with_cash(cash: &str) -> AccountState {
        test_issue_789_account_state(vec![AccountBalance::new(
            Money::from(cash),
            Money::from("0.00000000 USDC"),
            Money::from(cash),
        )])
    }

    #[test]
    fn issue_789_account_binding_preserves_zero_cash_delta_settlement() {
        let states = vec![
            (
                1,
                test_issue_789_account_state_with_cash("100.00000000 USDC"),
            ),
            (
                12,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
            (
                22,
                test_issue_789_account_state_with_cash("95.00000000 USDC"),
            ),
            (
                32,
                test_issue_789_account_state_with_cash("95.00000000 USDC"),
            ),
        ];

        let bound = bind_issue_789_account_states(&[10, 20, 30], &[11, 21, 31], &states, 40)
            .expect("zero-value settlement still has a distinct causal AccountState");

        assert_eq!(bound.len(), 4);
        assert_eq!(bound[3].0, 32);
    }

    #[test]
    fn issue_789_account_scope_limits_unrelated_state_to_initialization_prefix() {
        let lifecycle_account = AccountId::from("POLYMARKET-001");
        let mut unrelated = test_issue_789_account_state_with_cash("100.00000000 USDC");
        unrelated.account_id = AccountId::from("OTHER-001");
        ensure_issue_789_account_state_scope(&[(9, unrelated.clone())], lifecycle_account, 10)
            .expect("unrelated node-initialization state may precede the first submission");
        for seq in [10, 11] {
            let error = ensure_issue_789_account_state_scope(
                &[(seq, unrelated.clone())],
                lifecycle_account,
                10,
            )
            .expect_err("unrelated account evidence at or after submission must fail closed");
            assert!(error.to_string().contains("unrelated AccountState"));
        }
    }

    #[test]
    fn issue_789_account_binding_accepts_reported_to_calculated_initial_republish() {
        let reported = test_issue_789_account_state_with_cash("100.00000000 USDC");
        let mut calculated = reported.clone();
        calculated.is_reported = false;
        let states = vec![
            (1, reported),
            (2, calculated),
            (
                12,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
        ];

        let bound = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect("NT may republish the same initial map as calculated state");

        assert_eq!(bound.len(), 2);
        assert_eq!(bound[0].0, 2);
    }

    #[test]
    fn issue_789_account_binding_rejects_account_before_position_effect() {
        let states = vec![
            (
                1,
                test_issue_789_account_state_with_cash("100.00000000 USDC"),
            ),
            (
                11,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
            (
                22,
                test_issue_789_account_state_with_cash("95.00000000 USDC"),
            ),
            (
                32,
                test_issue_789_account_state_with_cash("96.00000000 USDC"),
            ),
        ];

        let error = bind_issue_789_account_states(&[10, 20, 30], &[12, 21, 31], &states, 40)
            .expect_err("AccountState before its position effect must fail closed");

        assert!(error.to_string().contains("after its position mutation"));
    }

    #[test]
    fn issue_789_account_binding_rejects_replayed_prior_event_identity() {
        let initial = test_issue_789_account_state_with_cash("100.00000000 USDC");
        let states = vec![(1, initial.clone()), (12, initial)];

        let error = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect_err("a replayed pre-fill AccountState is not a post-fill event");

        assert!(
            error
                .to_string()
                .contains("replayed AccountState event identity")
        );
    }

    #[test]
    fn issue_789_account_binding_rejects_replay_mixed_with_fresh_event() {
        let initial = test_issue_789_account_state_with_cash("100.00000000 USDC");
        let states = vec![
            (1, initial.clone()),
            (12, initial),
            (
                13,
                test_issue_789_account_state_with_cash("100.00000000 USDC"),
            ),
        ];

        let error = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect_err("every AccountState candidate must have a fresh identity");

        assert!(
            error
                .to_string()
                .contains("replayed AccountState event identity")
        );
    }

    #[test]
    fn issue_789_account_binding_rejects_missing_transition() {
        let states = vec![(
            1,
            test_issue_789_account_state_with_cash("100.00000000 USDC"),
        )];

        let error = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect_err("a fill without AccountState evidence must fail closed");

        assert!(error.to_string().contains("has no AccountState"));
    }

    #[test]
    fn issue_789_account_binding_rejects_conflicting_initial_evidence() {
        let states = vec![
            (
                1,
                test_issue_789_account_state_with_cash("100.00000000 USDC"),
            ),
            (
                2,
                test_issue_789_account_state_with_cash("99.00000000 USDC"),
            ),
            (
                12,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
        ];

        let error = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect_err("conflicting initial AccountState evidence must fail closed");

        assert!(
            error
                .to_string()
                .contains("conflicting initial AccountState")
        );
    }

    #[test]
    fn issue_789_account_binding_rejects_conflicting_interval_evidence() {
        let states = vec![
            (
                1,
                test_issue_789_account_state_with_cash("100.00000000 USDC"),
            ),
            (
                12,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
            (
                13,
                test_issue_789_account_state_with_cash("91.00000000 USDC"),
            ),
        ];

        let error = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect_err("conflicting AccountStates inside one fill interval must fail closed");

        assert!(
            error
                .to_string()
                .contains("conflicting AccountState evidence")
        );
    }

    #[test]
    fn issue_789_account_binding_rejects_unexplained_trailing_state() {
        let states = vec![
            (
                1,
                test_issue_789_account_state_with_cash("100.00000000 USDC"),
            ),
            (
                12,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
            (
                21,
                test_issue_789_account_state_with_cash("90.00000000 USDC"),
            ),
        ];

        let error = bind_issue_789_account_states(&[10], &[11], &states, 20)
            .expect_err("AccountState evidence after the terminal bound must fail closed");

        assert!(error.to_string().contains("outside the ordered"));
    }

    #[test]
    fn issue_789_account_cash_rejects_extra_currency_balance() {
        let state = test_issue_789_account_state(vec![
            AccountBalance::new(
                Money::from("100.00000000 USDC"),
                Money::from("0.00000000 USDC"),
                Money::from("100.00000000 USDC"),
            ),
            AccountBalance::new(
                Money::from("1.00000000 BTC"),
                Money::from("0.00000000 BTC"),
                Money::from("1.00000000 BTC"),
            ),
        ]);

        let error = issue_789_account_cash(&state, Currency::USDC())
            .expect_err("#789 must reject an extra account currency");
        assert!(error.to_string().contains("single-currency"));
    }

    #[test]
    fn issue_789_account_cash_rejects_margin_state() {
        let mut state = test_issue_789_account_state(vec![AccountBalance::new(
            Money::from("100.00000000 USDC"),
            Money::from("0.00000000 USDC"),
            Money::from("100.00000000 USDC"),
        )]);
        state.margins.push(MarginBalance::new(
            Money::from("1.00000000 USDC"),
            Money::from("1.00000000 USDC"),
            None,
        ));

        let error = issue_789_account_cash(&state, Currency::USDC())
            .expect_err("#789 CASH evidence must reject margin balances");
        assert!(error.to_string().contains("must not contain margin"));
    }

    #[test]
    fn issue_789_account_cash_rejects_locked_cash() {
        let state = test_issue_789_account_state(vec![AccountBalance::new(
            Money::from("100.00000000 USDC"),
            Money::from("1.00000000 USDC"),
            Money::from("99.00000000 USDC"),
        )]);

        let error = issue_789_account_cash(&state, Currency::USDC())
            .expect_err("#789 immediate-market evidence must reject locked cash");
        assert!(error.to_string().contains("zero locked cash"));
    }

    #[test]
    fn issue_789_terminal_account_rejects_complete_map_drift() {
        let stored = test_issue_789_account_state(vec![AccountBalance::new(
            Money::from("100.00000000 USDC"),
            Money::from("0.00000000 USDC"),
            Money::from("100.00000000 USDC"),
        )]);
        let terminal = super::AccountTerminalRecord {
            account_id: stored.account_id,
            account_type: stored.account_type,
            base_currency: stored.base_currency,
            balances: vec![AccountBalance::new(
                Money::from("99.00000000 USDC"),
                Money::from("0.00000000 USDC"),
                Money::from("99.00000000 USDC"),
            )],
            cash_locks: Vec::new(),
            margins: Vec::new(),
        };

        let error =
            ensure_issue_789_terminal_account_matches(&stored, &terminal, stored.base_currency)
                .expect_err("#789 must reject complete terminal account-map drift");
        assert!(error.to_string().contains("complete final AccountState"));
    }

    #[test]
    fn issue_789_terminal_account_rejects_hidden_locked_cash() {
        let terminal = super::AccountTerminalRecord {
            account_id: AccountId::from("POLYMARKET-001"),
            account_type: AccountType::Cash,
            base_currency: Some(Currency::USDC()),
            balances: vec![AccountBalance::new(
                Money::from("100.00000000 USDC"),
                Money::from("0.00000000 USDC"),
                Money::from("100.00000000 USDC"),
            )],
            cash_locks: vec![(
                InstrumentId::from("YES.POLYMARKET"),
                Currency::USDC(),
                Money::from("1.00000000 USDC"),
            )],
            margins: Vec::new(),
        };

        let error = issue_789_terminal_account_cash(&terminal, Currency::USDC())
            .expect_err("current account locks absent from its last event must fail closed");
        assert!(error.to_string().contains("locked cash"));
    }

    #[test]
    fn issue_789_terminal_capture_preserves_cash_account_transient_locks() {
        let state = test_issue_789_account_state_with_cash("100.00000000 USDC");
        let mut account = nautilus_model::accounts::CashAccount::new(state, true, false);
        account.balances_locked.insert(
            (InstrumentId::from("YES.POLYMARKET"), Currency::BTC()),
            Money::from("0.00000000 BTC"),
        );
        let terminal =
            super::capture_account_terminal(&nautilus_model::accounts::AccountAny::Cash(account));

        assert_eq!(terminal.cash_locks.len(), 1);
        let error = issue_789_terminal_account_cash(&terminal, Currency::USDC())
            .expect_err("a stale transient CashAccount lock must fail closed");
        assert!(error.to_string().contains("locked cash"));
    }

    #[test]
    fn terminal_capture_preserves_wallet_account_identity_balances_and_locks() {
        let state = AccountState::new(
            AccountId::from("WALLET-001"),
            AccountType::Wallet,
            vec![AccountBalance::new(
                Money::from("2.00000000 USDC"),
                Money::from("0.00000000 USDC"),
                Money::from("2.00000000 USDC"),
            )],
            Vec::new(),
            true,
            UUID4::default(),
            UnixNanos::from(1),
            UnixNanos::from(1),
            None,
        );
        let mut account = nautilus_model::accounts::WalletAccount::new(state, true);
        account.balances_locked.insert(
            (InstrumentId::from("YES.POLYMARKET"), Currency::USDC()),
            Money::from("0.50000000 USDC"),
        );

        let terminal =
            super::capture_account_terminal(&nautilus_model::accounts::AccountAny::Wallet(account));

        assert_eq!(terminal.account_type, AccountType::Wallet);
        assert_eq!(terminal.base_currency, None);
        assert_eq!(terminal.balances.len(), 1);
        assert_eq!(terminal.cash_locks.len(), 1);
        assert!(terminal.margins.is_empty());
    }

    #[test]
    fn issue_789_terminal_account_rejects_extra_locked_currency() {
        let terminal = super::AccountTerminalRecord {
            account_id: AccountId::from("POLYMARKET-001"),
            account_type: AccountType::Cash,
            base_currency: Some(Currency::USDC()),
            balances: vec![AccountBalance::new(
                Money::from("100.00000000 USDC"),
                Money::from("0.00000000 USDC"),
                Money::from("100.00000000 USDC"),
            )],
            cash_locks: vec![(
                InstrumentId::from("YES.POLYMARKET"),
                Currency::BTC(),
                Money::from("0.00000000 BTC"),
            )],
            margins: Vec::new(),
        };

        let error = issue_789_terminal_account_cash(&terminal, Currency::USDC())
            .expect_err("an extra current locked-balance currency must fail closed");
        assert!(error.to_string().contains("locked cash"));
    }

    #[test]
    fn issue_789_terminal_account_rejects_base_currency_drift() {
        let stored = test_issue_789_account_state_with_cash("100.00000000 USDC");
        let terminal = super::AccountTerminalRecord {
            account_id: stored.account_id,
            account_type: stored.account_type,
            base_currency: None,
            balances: stored.balances.clone(),
            cash_locks: Vec::new(),
            margins: Vec::new(),
        };

        let error =
            ensure_issue_789_terminal_account_matches(&stored, &terminal, stored.base_currency)
                .expect_err("current account metadata drift must fail closed");
        assert!(error.to_string().contains("complete final AccountState"));
    }

    #[test]
    fn issue_789_terminal_account_rejects_correlated_manifest_base_currency_drift() {
        let stored = test_issue_789_account_state_with_cash("100.00000000 USDC");
        let terminal = super::AccountTerminalRecord {
            account_id: stored.account_id,
            account_type: stored.account_type,
            base_currency: stored.base_currency,
            balances: stored.balances.clone(),
            cash_locks: Vec::new(),
            margins: Vec::new(),
        };

        let error = ensure_issue_789_terminal_account_matches(&stored, &terminal, None)
            .expect_err("stored and terminal base currency must match the manifest NONE setting");
        assert!(error.to_string().contains("manifest base currency"));
    }

    fn issue_789_account_cash(
        state: &nautilus_model::events::AccountState,
        currency: Currency,
    ) -> Result<Money> {
        ensure!(
            state.account_type == AccountType::Cash,
            "#789 lifecycle AccountState must remain CASH"
        );
        ensure!(
            state.margins.is_empty(),
            "#789 CASH AccountState must not contain margin balances"
        );
        ensure!(
            state.balances.len() == 1,
            "#789 lifecycle is restricted to a single-currency AccountState, got {} balances",
            state.balances.len()
        );
        let balance = &state.balances[0];
        ensure!(
            balance.currency == currency,
            "#789 AccountState balance currency {} differs from lifecycle currency {currency}",
            balance.currency
        );
        ensure!(
            balance.locked == Money::zero(currency) && balance.free == balance.total,
            "#789 immediate-market CASH lifecycle requires zero locked cash and free cash equal to total"
        );
        Ok(balance.total)
    }

    fn ensure_issue_789_terminal_account_matches(
        stored: &nautilus_model::events::AccountState,
        terminal: &super::AccountTerminalRecord,
        expected_base_currency: Option<Currency>,
    ) -> Result<()> {
        let mut stored_balances = stored.balances.clone();
        stored_balances.sort_by_key(|balance| balance.currency.to_string());
        let mut stored_margins = stored.margins.clone();
        stored_margins.sort_by_key(|margin| {
            (
                margin
                    .instrument_id
                    .map(|instrument_id| instrument_id.to_string()),
                margin.currency.to_string(),
            )
        });
        ensure!(
            terminal.account_id == stored.account_id
                && terminal.account_type == stored.account_type
                && stored.base_currency == expected_base_currency
                && terminal.base_currency == expected_base_currency
                && terminal.balances == stored_balances
                && terminal.margins == stored_margins,
            "terminal account projection diverges from the complete final AccountState or manifest base currency"
        );
        Ok(())
    }

    fn issue_789_terminal_account_cash(
        terminal: &super::AccountTerminalRecord,
        currency: Currency,
    ) -> Result<Money> {
        ensure!(
            terminal.account_type == AccountType::Cash,
            "#789 terminal account must remain CASH"
        );
        ensure!(
            terminal.margins.is_empty(),
            "#789 terminal CASH account must not contain margin balances"
        );
        ensure!(
            terminal.balances.len() == 1,
            "#789 terminal account is restricted to one currency, got {} balances",
            terminal.balances.len()
        );
        let balance = &terminal.balances[0];
        ensure!(
            balance.currency == currency,
            "#789 terminal account balance currency {} differs from lifecycle currency {currency}",
            balance.currency
        );
        ensure!(
            balance.locked == Money::zero(currency)
                && balance.free == balance.total
                && terminal.cash_locks.is_empty(),
            "#789 terminal immediate-market CASH account contains locked cash"
        );
        Ok(balance.total)
    }

    fn validate_issue_789_execution_contract(
        output: &super::NtBacktestNodeRun,
        evidence: &crate::execution_evidence::ExecutionEvidence,
        manifest: &BacktestingRunManifest,
        up_projection: &crate::pmxt_one_off_backfill_projection::PmxtOneOffNtProjection,
        down_projection: &crate::pmxt_one_off_backfill_projection::PmxtOneOffNtProjection,
        expected_binary_settlements: &BTreeMap<InstrumentId, Price>,
    ) -> Result<crate::execution_contract::ExecutionContractReport> {
        let mut submit_orders = BTreeMap::new();
        let mut order_initializations = BTreeMap::new();
        let mut order_submissions = BTreeMap::new();
        let mut order_acceptances = BTreeMap::new();
        let mut order_updates = BTreeMap::new();
        let mut closes = Vec::new();
        let mut sequenced_fills = Vec::new();
        let mut position_effects = Vec::new();
        let mut account_states = Vec::new();
        let mut semantic_identities = Vec::new();
        let unsupported_payload_types = evidence
            .entries
            .iter()
            .map(|entry| entry.payload_type.as_str())
            .filter(|payload_type| ensure_issue_789_payload_type_is_admitted(payload_type).is_err())
            .collect::<BTreeSet<_>>();
        ensure!(
            unsupported_payload_types.is_empty(),
            "#789 event store contains unsupported payload types {unsupported_payload_types:?}"
        );
        let run_envelope = evidence
            .entries
            .iter()
            .map(|entry| (entry.seq, entry.payload_type.as_str()))
            .collect::<Vec<_>>();
        ensure_issue_789_run_envelope(&run_envelope)?;
        for entry in &evidence.entries {
            match entry.payload_type.as_str() {
                "RunStarted" | "RunEnded" => {}
                "SubmitOrder" => {
                    let command: nautilus_common::messages::execution::SubmitOrder =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 SubmitOrder evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        command.command_id,
                        "SubmitOrder command",
                    )?;
                    ensure!(
                        submit_orders
                            .insert(
                                command.client_order_id.to_string(),
                                (entry.seq, entry.ts_init, command),
                            )
                            .is_none(),
                        "duplicate SubmitOrder identity in #789 event store"
                    );
                }
                "OrderInitialized" => {
                    let event: nautilus_model::events::OrderInitialized =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 OrderInitialized evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        event.event_id,
                        "OrderInitialized",
                    )?;
                    ensure!(
                        order_initializations
                            .insert(event.client_order_id.to_string(), (entry.seq, event))
                            .is_none(),
                        "duplicate OrderInitialized client-order identity in #789 event store"
                    );
                }
                "OrderSubmitted" => {
                    let event: nautilus_model::events::OrderSubmitted =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 OrderSubmitted evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        event.event_id,
                        "OrderSubmitted",
                    )?;
                    ensure!(
                        order_submissions
                            .insert(event.client_order_id.to_string(), (entry.seq, event))
                            .is_none(),
                        "duplicate OrderSubmitted client-order identity in #789 event store"
                    );
                }
                "OrderAccepted" => {
                    let event: nautilus_model::events::OrderAccepted =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 OrderAccepted evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        event.event_id,
                        "OrderAccepted",
                    )?;
                    ensure!(
                        order_acceptances
                            .insert(event.client_order_id.to_string(), (entry.seq, event))
                            .is_none(),
                        "duplicate OrderAccepted client-order identity in #789 event store"
                    );
                }
                "OrderUpdated" => {
                    let event: nautilus_model::events::OrderUpdated =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 OrderUpdated evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        event.event_id,
                        "OrderUpdated",
                    )?;
                    record_issue_789_order_update(&mut order_updates, entry.seq, event)?;
                }
                "OrderFilled" => {
                    let fill: nautilus_model::events::OrderFilled =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 OrderFilled evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        fill.event_id,
                        "OrderFilled",
                    )?;
                    sequenced_fills.push((entry.seq, fill));
                }
                "InstrumentClose" => {
                    let close: nautilus_model::data::InstrumentClose =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 InstrumentClose evidence")?;
                    closes.push((entry.seq, close));
                }
                "AccountState" => {
                    let state: nautilus_model::events::AccountState =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 AccountState evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        state.event_id,
                        "AccountState",
                    )?;
                    account_states.push((entry.seq, state));
                }
                "PositionOpened" => {
                    let effect: nautilus_model::events::PositionOpened =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 PositionOpened evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        effect.event_id,
                        "PositionOpened",
                    )?;
                    position_effects.push((
                        entry.seq,
                        crate::execution_contract::PositionEffectTrace {
                            kind: crate::execution_contract::PositionEffectKind::Opened,
                            trader_id: effect.trader_id,
                            strategy_id: effect.strategy_id,
                            position_id: effect.position_id,
                            instrument_id: effect.instrument_id,
                            account_id: effect.account_id,
                            opening_order_id: effect.opening_order_id,
                            closing_order_id: None,
                            entry: effect.entry,
                            side: effect.side,
                            signed_quantity: effect.signed_qty,
                            quantity: effect.quantity,
                            last_quantity: effect.last_qty,
                            last_price: effect.last_px,
                            currency: effect.currency,
                            realized_pnl: None,
                        },
                    ));
                }
                "PositionChanged" => {
                    let effect: nautilus_model::events::PositionChanged =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 PositionChanged evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        effect.event_id,
                        "PositionChanged",
                    )?;
                    position_effects.push((
                        entry.seq,
                        crate::execution_contract::PositionEffectTrace {
                            kind: crate::execution_contract::PositionEffectKind::Changed,
                            trader_id: effect.trader_id,
                            strategy_id: effect.strategy_id,
                            position_id: effect.position_id,
                            instrument_id: effect.instrument_id,
                            account_id: effect.account_id,
                            opening_order_id: effect.opening_order_id,
                            closing_order_id: None,
                            entry: effect.entry,
                            side: effect.side,
                            signed_quantity: effect.signed_qty,
                            quantity: effect.quantity,
                            last_quantity: effect.last_qty,
                            last_price: effect.last_px,
                            currency: effect.currency,
                            realized_pnl: effect.realized_pnl,
                        },
                    ));
                }
                "PositionClosed" => {
                    let effect: nautilus_model::events::PositionClosed =
                        rmp_serde::from_slice(&entry.payload)
                            .context("decode #789 PositionClosed evidence")?;
                    record_issue_789_semantic_identity(
                        &mut semantic_identities,
                        effect.event_id,
                        "PositionClosed",
                    )?;
                    position_effects.push((
                        entry.seq,
                        crate::execution_contract::PositionEffectTrace {
                            kind: crate::execution_contract::PositionEffectKind::Closed,
                            trader_id: effect.trader_id,
                            strategy_id: effect.strategy_id,
                            position_id: effect.position_id,
                            instrument_id: effect.instrument_id,
                            account_id: effect.account_id,
                            opening_order_id: effect.opening_order_id,
                            closing_order_id: effect.closing_order_id,
                            entry: effect.entry,
                            side: effect.side,
                            signed_quantity: effect.signed_qty,
                            quantity: effect.quantity,
                            last_quantity: effect.last_qty,
                            last_price: effect.last_px,
                            currency: effect.currency,
                            realized_pnl: effect.realized_pnl,
                        },
                    ));
                }
                "PositionAdjusted" => {
                    anyhow::bail!("#789 does not admit position-adjustment evidence")
                }
                "SubscribeCommand" | "UnsubscribeCommand" | "TimeEvent" => {
                    // Explicit control-plane waiver: these records drive data/time
                    // delivery but carry no order, fill, position, or account claim.
                    // Their bytes remain covered by the sealed-store verifier.
                }
                other => {
                    ensure_issue_789_payload_type_is_admitted(other)?;
                    anyhow::bail!(
                        "#789 admitted payload type {other:?} lacks an explicit lifecycle handler"
                    )
                }
            }
        }
        ensure!(
            !sequenced_fills.is_empty(),
            "issue #789 event store contains no fills"
        );
        let instrument_id = sequenced_fills[0].1.instrument_id;
        ensure!(
            sequenced_fills
                .iter()
                .all(|(_, fill)| fill.instrument_id == instrument_id),
            "issue #789 event-store fills span multiple instruments"
        );
        let fill_sequences = sequenced_fills
            .iter()
            .map(|(seq, _)| *seq)
            .collect::<Vec<_>>();
        let projection = if up_projection.instrument.id() == instrument_id {
            up_projection
        } else if down_projection.instrument.id() == instrument_id {
            down_projection
        } else {
            anyhow::bail!("issue #789 fill instrument {instrument_id} has no PMXT projection")
        };

        let grouped_fills = group_issue_789_fills(sequenced_fills)?;

        let mut orders = Vec::with_capacity(grouped_fills.len());
        let mut ordered_fills = Vec::new();
        let mut settlement_receipt_seq = None;
        for (client_order_id, first_fill_seq, fills) in grouped_fills {
            let cause = if let Some((submit_seq, submit_ts_init, submit)) =
                submit_orders.get(&client_order_id)
            {
                ensure!(
                    *submit_seq < first_fill_seq
                        && submit.instrument_id == instrument_id
                        && submit.order_init.client_order_id.to_string() == client_order_id,
                    "#789 normal fill is not causally preceded by its matching SubmitOrder"
                );
                let (submitted_seq, submitted_order) = issue_789_bind_normal_submission(
                    *submit_seq,
                    submit,
                    order_initializations.get(&client_order_id),
                    order_submissions.get(&client_order_id),
                    order_acceptances.get(&client_order_id),
                    first_fill_seq,
                )?;
                let event_count = evidence.book_delta_event_count_at(
                    *submit_seq,
                    *submit_ts_init,
                    &instrument_id.to_string(),
                    &projection.order_book_deltas,
                )?;
                let executable_book = replay_executable_book_at_cursor(
                    instrument_id,
                    &projection.order_book_deltas,
                    event_count,
                )?;
                crate::execution_contract::ExecutionOrderCause::Submitted {
                    executable_book: Box::new(executable_book),
                    submitted_order,
                    quote_conversion: issue_789_quote_conversion_in_interval(
                        submitted_seq,
                        first_fill_seq,
                        order_updates.get(&client_order_id),
                    )?
                    .map(Box::new),
                }
            } else {
                let matching_closes = closes
                    .iter()
                    .filter(|(_, close)| close.instrument_id == instrument_id)
                    .collect::<Vec<_>>();
                ensure!(
                    matching_closes.len() == 1,
                    "#789 fill {client_order_id} has neither SubmitOrder nor exactly one matching InstrumentClose receipt"
                );
                let close = matching_closes[0];
                // BacktestEngine routes InstrumentClose into the exchange before
                // publishing the same data through DataEngine. Settlement is
                // therefore generated before the BusTap can record the close.
                // Pin that observed boundary explicitly so an upstream ordering
                // change fails closed instead of silently changing the evidence
                // interpretation. Position effect, not this sequence relation,
                // classifies the fill as the terminal settlement below.
                ensure_issue_789_settlement_witness(
                    &client_order_id,
                    first_fill_seq,
                    &fills,
                    close.0,
                    &close.1,
                    order_initializations.get(&client_order_id),
                    order_submissions.get(&client_order_id),
                    order_acceptances.get(&client_order_id),
                )?;
                ensure!(
                    settlement_receipt_seq.replace(close.0).is_none(),
                    "#789 lifecycle contains multiple settlement receipts"
                );
                crate::execution_contract::ExecutionOrderCause::Settlement {
                    declared_price: close.1.close_price,
                }
            };
            let effective_quantity = output
                .order_terminals
                .iter()
                .find(|terminal| terminal.client_order_id == fills[0].client_order_id)
                .map(|terminal| terminal.quantity)
                .with_context(|| {
                    format!(
                        "missing terminal effective quantity for {}",
                        fills[0].client_order_id
                    )
                })?;
            ordered_fills.extend(fills.iter().cloned());
            orders.push(crate::execution_contract::ExecutionOrderTrace {
                cause,
                effective_quantity,
                fills,
            });
        }
        let settlement_receipt_seq = settlement_receipt_seq
            .context("#789 lifecycle has no terminal InstrumentClose receipt")?;
        let position_effect_sequences = position_effects
            .iter()
            .map(|(seq, _)| *seq)
            .collect::<Vec<_>>();
        let normal_order_ids = orders
            .iter()
            .filter(|order| {
                matches!(
                    &order.cause,
                    crate::execution_contract::ExecutionOrderCause::Submitted { .. }
                )
            })
            .map(|order| order.fills[0].client_order_id.to_string())
            .collect::<BTreeSet<_>>();
        let settlement_order_ids = orders
            .iter()
            .filter(|order| {
                matches!(
                    &order.cause,
                    crate::execution_contract::ExecutionOrderCause::Settlement { .. }
                )
            })
            .map(|order| order.fills[0].client_order_id.to_string())
            .collect::<BTreeSet<_>>();
        let submit_command_ids = submit_orders.keys().cloned().collect::<BTreeSet<_>>();
        let submitted_event_ids = order_submissions.keys().cloned().collect::<BTreeSet<_>>();
        let accepted_event_ids = order_acceptances.keys().cloned().collect::<BTreeSet<_>>();
        let initialized_event_ids = order_initializations
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let updated_ids = order_updates.keys().cloned().collect::<BTreeSet<_>>();
        ensure_issue_789_order_event_sets(
            &normal_order_ids,
            &settlement_order_ids,
            &submit_command_ids,
            &submitted_event_ids,
            &accepted_event_ids,
            &initialized_event_ids,
            &updated_ids,
        )?;
        ensure_issue_789_instrument_close_set(&closes, expected_binary_settlements)?;
        ensure!(
            output.result.total_orders == orders.len()
                && output.order_terminals.len() == orders.len(),
            "#789 terminal order projections are incomplete for the captured lifecycle"
        );
        ensure!(
            output.result.total_positions == 1,
            "#789 lifecycle evidence is restricted to exactly one position"
        );

        let position = issue_789_terminal_position(&output.positions, instrument_id)?;
        let configured_account_id = output.configured_execution_account_id;
        let lifecycle_account_id = orders
            .iter()
            .find_map(|order| match &order.cause {
                crate::execution_contract::ExecutionOrderCause::Submitted {
                    submitted_order,
                    ..
                } => Some(submitted_order.account_id),
                crate::execution_contract::ExecutionOrderCause::Settlement { .. } => None,
            })
            .context("#789 lifecycle has no submitted account anchor")?;
        ensure!(
            lifecycle_account_id == configured_account_id,
            "submitted account anchor diverges from the pre-run configured account"
        );
        let expected_position_id = position_effects
            .first()
            .map(|(_, effect)| effect.position_id)
            .context("#789 lifecycle contains no position effects")?;
        ensure!(
            position_effects.iter().all(|(_, effect)| {
                effect.position_id == expected_position_id
                    && effect.account_id == lifecycle_account_id
            }),
            "position effects diverge from the lifecycle position/account identity"
        );
        ensure_issue_789_terminal_position_matches(
            position,
            lifecycle_account_id,
            expected_position_id,
            &ordered_fills,
        )?;
        for order in &orders {
            let client_order_id = order.fills[0].client_order_id;
            let client_order_id_text = client_order_id.to_string();
            let terminal = output
                .order_terminals
                .iter()
                .find(|terminal| terminal.client_order_id == client_order_id)
                .with_context(|| format!("missing terminal order projection {client_order_id}"))?;
            let (initialized, requested_effective_quantity, quantity_policy) = match &order.cause {
                crate::execution_contract::ExecutionOrderCause::Submitted {
                    submitted_order,
                    quote_conversion,
                    ..
                } => {
                    let submit = &submit_orders
                        .get(&client_order_id_text)
                        .with_context(|| {
                            format!("missing SubmitOrder projection {client_order_id}")
                        })?
                        .2;
                    let effective_quantity = quote_conversion
                        .as_ref()
                        .map_or(submitted_order.quantity, |update| update.quantity);
                    (
                        &submit.order_init,
                        effective_quantity,
                        if submitted_order.quote_quantity {
                            Issue789ExecutionQuantityPolicy::Exact
                        } else {
                            Issue789ExecutionQuantityPolicy::ReducibleIocBound
                        },
                    )
                }
                crate::execution_contract::ExecutionOrderCause::Settlement { .. } => {
                    let initialized = &order_initializations
                        .get(&client_order_id_text)
                        .with_context(|| {
                            format!("missing settlement OrderInitialized {client_order_id}")
                        })?
                        .1;
                    (
                        initialized,
                        initialized.quantity,
                        Issue789ExecutionQuantityPolicy::Exact,
                    )
                }
            };
            ensure_issue_789_terminal_order_matches(
                terminal,
                initialized,
                configured_account_id,
                position.id,
                requested_effective_quantity,
                quantity_policy,
                &order.fills,
            )?;
        }
        let realized_pnl = position
            .realized_pnl
            .context("issue #789 position has no realized PnL")?;
        let matching_terminal_accounts: Vec<_> = output
            .account_terminals
            .iter()
            .filter(|account| account.account_id == position.account_id)
            .collect();
        ensure!(
            matching_terminal_accounts.len() == 1,
            "issue #789 requires exactly one terminal account projection for {}, got {}",
            position.account_id,
            matching_terminal_accounts.len()
        );
        let terminal_account = matching_terminal_accounts[0];
        let terminal_cash =
            issue_789_terminal_account_cash(terminal_account, realized_pnl.currency)?;
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
        ensure!(
            initial_balances.len() == 1 && initial_balances[0].currency == realized_pnl.currency,
            "issue #789 requires exactly one initial {} balance, got {:?}",
            realized_pnl.currency,
            initial_balances
        );
        let manifest_initial_cash = initial_balances[0];
        let expected_base_currency = manifest
            .to_nt_venue_config()
            .context("map issue #789 manifest venue configuration")?
            .base_currency();
        let first_submit_seq = submit_orders
            .values()
            .map(|(seq, _, _)| *seq)
            .min()
            .context("#789 lifecycle contains no normal SubmitOrder evidence")?;
        ensure_issue_789_account_state_scope(
            &account_states,
            lifecycle_account_id,
            first_submit_seq,
        )?;
        let raw_lifecycle_account_states = account_states
            .iter()
            .filter(|(_, state)| state.account_id == position.account_id)
            .cloned()
            .collect::<Vec<_>>();
        ensure!(
            !raw_lifecycle_account_states.is_empty(),
            "issue #789 event store contains no lifecycle AccountState evidence"
        );
        ensure!(
            raw_lifecycle_account_states
                .iter()
                .all(|(_, state)| state.base_currency == expected_base_currency),
            "#789 lifecycle AccountState evidence diverges from the manifest base currency"
        );
        let lifecycle_account_states = bind_issue_789_account_states(
            &fill_sequences,
            &position_effect_sequences,
            &raw_lifecycle_account_states,
            settlement_receipt_seq,
        )?;
        let initial_cash =
            issue_789_account_cash(&lifecycle_account_states[0].1, realized_pnl.currency)?;
        let stored_terminal_account = &lifecycle_account_states
            .last()
            .context("issue #789 AccountState sequence unexpectedly empty")?
            .1;
        let stored_terminal_cash =
            issue_789_account_cash(stored_terminal_account, realized_pnl.currency)?;
        ensure!(
            initial_cash == manifest_initial_cash,
            "initial AccountState does not equal the manifest starting balance"
        );
        ensure_issue_789_terminal_account_matches(
            stored_terminal_account,
            terminal_account,
            expected_base_currency,
        )?;
        ensure!(
            stored_terminal_cash == terminal_cash,
            "terminal account cash diverges from the final AccountState"
        );
        let account_cash_after_fills = lifecycle_account_states
            .iter()
            .skip(1)
            .map(|(_, state)| issue_789_account_cash(state, realized_pnl.currency))
            .collect::<Result<Vec<_>>>()?;
        let position_commissions = position.commissions.values().copied().collect();
        let settlement = manifest
            .instrument_settlements
            .iter()
            .find(|settlement| settlement.nt_instrument_id == instrument_id.to_string())
            .context("issue #789 fill instrument has no settlement")?;
        let declared_settlement_price = Price::from_str(&settlement.close_price)
            .map_err(|error| anyhow::anyhow!(error))
            .context("parse issue #789 exact settlement price")?;
        ensure!(
            orders.iter().any(|order| matches!(
                &order.cause,
                crate::execution_contract::ExecutionOrderCause::Settlement { declared_price }
                    if *declared_price == declared_settlement_price
            )),
            "captured InstrumentClose does not equal the manifest settlement"
        );
        let resolved_config_bytes = output
            .resolved_config_bytes
            .as_deref()
            .context("issue #789 runner omitted resolved config bytes")?;

        let report = crate::execution_contract::validate_execution_contract(
            &crate::execution_contract::ExecutionContractTrace {
                instrument: &projection.instrument,
                configured_account_id,
                orders,
                position_effects: position_effects
                    .into_iter()
                    .map(|(_, effect)| effect)
                    .collect(),
                initial_cash,
                account_cash_after_fills,
                terminal_cash: stored_terminal_cash,
                realized_pnl,
                position_commissions,
                canonical_resolved_config_bytes: resolved_config_bytes,
                canonical_resolved_config_sha256: &manifest.strategy_config_hash,
            },
        )?;
        ensure!(
            report.normal_exit_fill_count > 0,
            "issue #789 lifecycle did not contain a validated normal exit before settlement"
        );
        Ok(report)
    }

    fn write_issue_789_result_artifact(
        output: &super::NtBacktestNodeRun,
        guard: &crate::result_contract::BacktestRunGuardReport,
        bridge_report: &SeededL2QuoteBridgeReport,
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
                "signal": "OKX BBO derived from real full-depth L2",
                "rv_okx": "OKX BBO derived from real full-depth L2",
                "rv_bybit": "Bybit BBO derived from real full-depth L2",
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
            "iterations": output.result.iterations,
            "seeded_l2_quote_bridge": bridge_report,
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

    fn seeded_window(
        jsonl: &str,
        mapping: SeededLevelSetMappingConfig,
        instrument_spec: &SpotInstrumentSpec,
        spec: QuoteTableSpec<'_>,
    ) -> Result<SeededLevelSetWindow> {
        let mut accepted = synthetic_accepted_dataset_for_tests();
        let payload_hash = sha256_hex(jsonl.as_bytes());
        accepted.source_binding = spec.source_binding.to_string();
        accepted.venue = spec.venue.to_string();
        accepted.product_family = "spot".to_string();
        accepted.product_category = "l2_orderbook".to_string();
        accepted.table_family = "order_book_snapshot_deltas".to_string();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;
        accepted.source_proof_id = format!(
            "issue-789-{}-snapshot-seeded-l2",
            spec.venue.to_ascii_lowercase()
        );
        accepted.forbidden_claims =
            vec!["raw-bbo-claim-without-snapshot-seeded-replay".to_string()];
        accepted.object.source_url = spec.payload_id.to_string();
        accepted.object.sha256.clone_from(&payload_hash);
        accepted.object.bytes = jsonl.len() as u64;
        accepted.object.archive_date = "2026-04-22".to_string();
        accepted.accepted_object_sha256 = payload_hash;
        let identity = CanonicalInstrumentIdentity {
            instrument_id: spec.instrument_id.to_string(),
            venue_symbol: spec.venue_symbol.to_string(),
            nt_instrument_id: spec.nt_instrument_id.to_string(),
        };
        let instrument =
            build_catalog_instrument(&CatalogInstrumentSpec::Spot(instrument_spec.clone()))?;
        let raw_payload = RawPayloadConfig {
            container: RawPayloadContainer::JsonlText,
            max_object_bytes: jsonl.len() as u64,
            max_decoded_bytes: jsonl.len() as u64,
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
            jsonl_stream: Some(JsonlStreamConfig {
                max_members: 1,
                max_record_bytes: 100_000,
                max_records: 20_000,
            }),
        };
        normalize_seeded_level_set_window(
            SeededLevelSetCompileInput {
                accepted: &accepted,
                identity: &identity,
                instrument: &instrument,
                window: SeededLevelSetWindowBounds {
                    start_time: None,
                    end_time: None,
                },
                raw_bytes: jsonl.as_bytes(),
                capture_time: ISSUE_789_START_NS,
                ingest_run_id: "issue-789-first-real-pl",
            },
            &raw_payload,
            &mapping,
        )
        .with_context(|| format!("normalize {} full-depth L2 and derived BBO", spec.venue))
    }

    fn issue_789_seeded_output_limits() -> SeededLevelSetOutputLimits {
        SeededLevelSetOutputLimits {
            max_levels_per_event: 1_024,
            max_active_levels_per_side: 512,
            max_selected_events: 20_000,
            max_selected_delta_rows: 1_000_000,
            max_emitted_bytes: 1_000_000_000,
            max_published_bytes: 2_000_000_000,
        }
    }

    fn okx_seeded_level_set_mapping() -> SeededLevelSetMappingConfig {
        SeededLevelSetMappingConfig {
            record_identity_path: vec!["instId".to_string()],
            action_path: vec!["action".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["bids".to_string()],
            asks_path: vec!["asks".to_string()],
            level_arity: 3,
            level_price_index: 0,
            level_size_index: 1,
            order_count: OrderCountPolicy::ValidateNonNegativeAndDrop { index: 2 },
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["update".to_string()],
            source_sequence: SourceSequencePolicy::Unavailable,
            output: issue_789_seeded_output_limits(),
        }
    }

    fn bybit_seeded_level_set_mapping() -> SeededLevelSetMappingConfig {
        SeededLevelSetMappingConfig {
            record_identity_path: vec!["data".to_string(), "s".to_string()],
            action_path: vec!["type".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["data".to_string(), "b".to_string()],
            asks_path: vec!["data".to_string(), "a".to_string()],
            level_arity: 2,
            level_price_index: 0,
            level_size_index: 1,
            order_count: OrderCountPolicy::Absent,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["delta".to_string()],
            source_sequence: SourceSequencePolicy::Native {
                path: vec!["data".to_string(), "seq".to_string()],
            },
            output: issue_789_seeded_output_limits(),
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
  "negRisk": false,
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
                            "reconstructed_from_okx_full_depth_l2_bbo".to_string(),
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
        let up_provider_instrument_id = catalogs
            .up_instrument_id
            .split_once('.')
            .map_or(catalogs.up_instrument_id.as_str(), |(symbol, _)| symbol);
        let down_provider_instrument_id = catalogs
            .down_instrument_id
            .split_once('.')
            .map_or(catalogs.down_instrument_id.as_str(), |(symbol, _)| symbol);
        let economics = polymarket_replay_economics(
            &[
                (&catalogs.up_instrument_id, up_provider_instrument_id),
                (&catalogs.down_instrument_id, down_provider_instrument_id),
            ],
            "issue-789-condition",
            ISSUE_789_START_NS as u64,
            ISSUE_789_END_NS as u64,
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
                    (
                        STRATEGY_PARAM_EVIDENCE_READ_MAX_BYTES.to_string(),
                        "67108864".to_string(),
                    ),
                    (
                        STRATEGY_PARAM_EVIDENCE_REJECT_EPISODE_MAX_COUNT.to_string(),
                        "4096".to_string(),
                    ),
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
            economics: Some(economics),
            strategy_config_hash: "0".repeat(64),
            // POLYMARKET must be funded in the binary's settlement currency
            // (pUSD — the NT Polymarket adapter's collateral currency), not
            // USDC. NT's multi-currency portfolio manager refuses to auto-create
            // a balance for a negative realized PnL, so settling a held loser in
            // a currency the account was never funded in silently drops the P/L
            // from stats_pnls. The instrument's settlement currency owns this.
            venue: issue_789_venue("POLYMARKET", "pUSD", "L2_MBP", true, true),
            additional_venues: vec![
                issue_789_venue("OKX", "USDT", "L2_MBP", false, false),
                issue_789_venue("BYBIT", "USDT", "L2_MBP", false, false),
                issue_789_venue("CHAINLINK", "USD", "L1_MBP", false, false),
            ],
            catalog_inputs: vec![
                catalog_input(
                    &catalogs.okx_catalog,
                    "OrderBookDelta",
                    "BTC-USDT.OKX",
                    Some("okx_data"),
                ),
                catalog_input(
                    &catalogs.bybit_catalog,
                    "OrderBookDelta",
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
        let economics = manifest
            .economics
            .as_ref()
            .context("issue #789 manifest must carry economics authority")?;
        let production_root_config_path =
            resolve_existing_input_path(Path::new(&economics.production_root_config_path));
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
