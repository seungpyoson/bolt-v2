//! Gate 4 — typed `BacktestingRunManifest` and NautilusTrader config mapping.
//!
//! The run manifest is the backtest recipe. It carries run intent plus the
//! fields needed to build the NautilusTrader `BacktestRunConfig`,
//! `BacktestDataConfig`, and `BacktestVenueConfig`, and it is validated to
//! reject inline strategy code, Python strategy paths, untracked config blobs,
//! and unaccepted data before any run.
//!
//! Strategy execution is restricted to existing compiled Rust strategies selected
//! by a registry key (see [`registered_strategies`]). The manifest records
//! whether the typed config was selected directly, human-authored, or generated
//! from a Research Analytics experiment result; it never
//! carries executable strategy code or a runtime path.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Debug,
    mem::size_of,
    str::FromStr,
};

use anyhow::{Result, bail};
use bolt_v2::{
    bolt_v3_config::{BacktestConfigOverride, RealizedVolatilitySourceSelector},
    bolt_v3_order_execution::BoltV3OrderExecutionMode,
    strategies::{
        binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder, binary_oracle_maker,
        production_strategy_registry, registry::StrategyBuilder,
    },
};
use nautilus_backtest::config::{
    BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType,
};
use nautilus_core::UnixNanos;
use nautilus_execution::models::{
    fee::{FeeModelAny, MakerTakerFeeModel},
    fill::{FillModelAny, ProbabilisticFillModel},
    latency::{LatencyModelAny, StaticLatencyModel},
};
use nautilus_model::{
    data::BarType,
    enums::{AccountType, BookType, OmsType, OtoTriggerMode},
    identifiers::{ClientId, InstrumentId},
    types::{Currency, Money, Quantity},
};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use super::{
    operator_work_budget::{
        OperatorWorkBudgetGuard, OperatorWorkBudgetStage, serialize_json_to_vec_guarded,
        sha256_json_guarded,
    },
    source_proof::{AcceptedDataset, FixtureType, SourceProofFidelityClass},
};

/// Registry key for the compiled Rust trade-driven example strategy.
pub const STRATEGY_HURST_VPIN_DIRECTIONAL: &str = "hurst_vpin_directional";
/// Registry key for Bolt's compiled Rust binary-oracle taker strategy.
pub const STRATEGY_BINARY_ORACLE_EDGE_TAKER: &str = "binary_oracle_edge_taker";
/// Registry key for Bolt's compiled Rust binary-oracle maker strategy.
pub const STRATEGY_BINARY_ORACLE_MAKER: &str = binary_oracle_maker::KEY;
/// Registry key for the bolt-owned mechanical trade-replay order-producing probe.
pub const STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE: &str = "mechanical_trade_replay_probe";
/// Strategy parameter key for the bar type.
pub const STRATEGY_PARAM_BAR_TYPE: &str = "bar_type";
/// Strategy parameter key for the trade size.
pub const STRATEGY_PARAM_TRADE_SIZE: &str = "trade_size";
/// Strategy parameter key for the normalized binary-oracle builder TOML.
pub const STRATEGY_PARAM_CONFIG_TOML: &str = "config_toml";
/// Strategy parameter key for the backtest fee-provider assumption.
pub const STRATEGY_PARAM_FEE_BPS: &str = "fee_bps";
/// Strategy parameter key for the Bolt-v3 order execution policy mode.
pub const STRATEGY_PARAM_ORDER_EXECUTION_MODE: &str = "order_execution_mode";
/// Strategy parameter key for the number of delivered trades before the entry order.
pub const STRATEGY_PARAM_ENTRY_AFTER_TRADES: &str = "entry_after_trades";
/// Strategy parameter key for the number of further delivered trades before the close.
pub const STRATEGY_PARAM_EXIT_AFTER_TRADES: &str = "exit_after_trades";
/// Strategy parameter key for the entry order side.
pub const STRATEGY_PARAM_SIDE: &str = "side";
/// Explicit manifest value for no catalog filesystem protocol.
pub const CATALOG_FS_PROTOCOL_NONE: &str = "NONE";
/// TOML selector for prediction-market fees resolved from instrument maker/taker rates.
pub const FEE_MODEL_PREDICTION_MARKET_MAKER_TAKER: &str = "prediction_market_maker_taker";
/// TOML selector for prediction-market fill realism backed by NT's probabilistic fill model.
pub const FILL_MODEL_PREDICTION_MARKET_PROBABILISTIC: &str = "prediction_market_probabilistic";
/// TOML selector for prediction-market venue latency backed by NT's static latency model.
pub const LATENCY_MODEL_PREDICTION_MARKET_STATIC: &str = "prediction_market_static";
/// TOML selector for the closed-position share domain metric.
pub const DOMAIN_METRIC_CLOSED_POSITION_RATIO: &str = "closed_position_ratio";
/// Artifact subpath for RA experiment-results artifacts that may carry GO-gated typed config refs.
const RESEARCH_ANALYTICS_EXPERIMENT_RESULT_PREFIX: &str =
    "research-analytics/v1/experiment-results";
/// NT venue-model surfaces declared in TOML but rejected until typed mappings exist.
pub const UNSUPPORTED_NT_VENUE_SURFACES: &[&str] =
    &["leverages", "margin_model", "modules", "settlement_prices"];
/// NT data-query surfaces declared in TOML but rejected until typed mappings are proven.
pub const UNSUPPORTED_NT_CATALOG_QUERY_SURFACES: &[(&str, &str, &str)] = &[
    (
        "catalog.metadata",
        "catalog_inputs.metadata",
        "BacktestDataConfig.metadata",
    ),
    (
        "catalog.bar_spec",
        "catalog_inputs.bar_spec",
        "BacktestDataConfig.bar_spec",
    ),
    (
        "catalog.bar_types",
        "catalog_inputs.bar_types",
        "BacktestDataConfig.bar_types",
    ),
];
/// Artifact-local manifest version written beside each backtest result.
pub const BACKTEST_RUN_MANIFEST_ARTIFACT_VERSION: &str = "backtest-run-manifest.v1";
/// Portable physical NT-catalog inventory sealed after projection and before execution.
pub const CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION: &str = "catalog-projection-manifest-v2";
/// Producer-minted authority binding a submitted manifest to exact catalog bytes.
pub const CATALOG_RUN_VIEW_AUTHORITY_SCHEMA_VERSION: &str = "catalog-run-view-authority.v1";
/// Canonical run artifact containing the unchanged producer-minted catalog authority.
pub const CATALOG_RUN_VIEW_AUTHORITY_FILE: &str = "catalog-run-view-authority.json";
/// Submitted run-manifest TOML schema version.
pub const BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION: &str = "backtesting-run-manifest.v2";

const CATALOG_STORAGE_OPTIONS_SHADOWED: &str =
    "cannot be combined with catalog_fs_rust_storage_options";
const S3_OPTION_CONDITIONAL_PUT: &str = "conditional_put";
const S3_CONDITIONAL_PUT_ETAG: &str = "etag";
const S3_CONDITIONAL_PUT_DISABLED: &str = "disabled";

/// Existing compiled Rust strategies selectable from a run manifest.
///
/// This is the single source of truth shared by manifest validation (gate 4)
/// and strategy instantiation (gate 5 runner).
#[must_use]
pub fn registered_strategies() -> &'static [&'static str] {
    &[
        STRATEGY_HURST_VPIN_DIRECTIONAL,
        STRATEGY_BINARY_ORACLE_EDGE_TAKER,
        STRATEGY_BINARY_ORACLE_MAKER,
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE,
    ]
}

#[must_use]
pub fn registered_strategy_parameters(registry_key: &str) -> Option<&'static [&'static str]> {
    match registry_key {
        STRATEGY_HURST_VPIN_DIRECTIONAL => {
            Some(&[STRATEGY_PARAM_BAR_TYPE, STRATEGY_PARAM_TRADE_SIZE])
        }
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => Some(&[
            STRATEGY_PARAM_CONFIG_TOML,
            STRATEGY_PARAM_FEE_BPS,
            STRATEGY_PARAM_ORDER_EXECUTION_MODE,
        ]),
        STRATEGY_BINARY_ORACLE_MAKER => Some(&[
            STRATEGY_PARAM_CONFIG_TOML,
            STRATEGY_PARAM_FEE_BPS,
            STRATEGY_PARAM_ORDER_EXECUTION_MODE,
        ]),
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE => Some(&[
            STRATEGY_PARAM_TRADE_SIZE,
            STRATEGY_PARAM_ENTRY_AFTER_TRADES,
            STRATEGY_PARAM_EXIT_AFTER_TRADES,
            STRATEGY_PARAM_SIDE,
        ]),
        _ => None,
    }
}

#[must_use]
pub fn registered_domain_metrics() -> &'static [&'static str] {
    &[DOMAIN_METRIC_CLOSED_POSITION_RATIO]
}

/// Market-structure fixture family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarketStructureFixture {
    BinaryOption,
    PerpsSpot,
}

/// Backtest run purpose. `Normal` runs cannot pin a non-latest source proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPurpose {
    Normal,
    Reproduction,
    Audit,
    Regression,
    Migration,
}

/// Classification vocabulary for NT/custom backtest extension surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NtSurfaceClassification {
    Defaulted,
    PassThrough,
    CustomOwned,
    UnsupportedForNow,
}

/// Resolved evidence for one NT/custom extension surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedNtSurface {
    pub surface: String,
    pub classification: NtSurfaceClassification,
    pub nt_field: String,
    pub resolved_value: String,
}

/// Currentness dimension whose exact admission rule is intentionally deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestCurrentnessDimension {
    NtVersion,
    StrategyConfigHash,
    CatalogHash,
    ManifestSchema,
    ExecutionModel,
}

/// Validation status for a manifest currentness dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestCurrentnessRuleStatus {
    Deferred,
}

/// Manifest schema slot for a future non-source-proof currentness rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestCurrentnessRuleSlot {
    pub dimension: ManifestCurrentnessDimension,
    pub status: ManifestCurrentnessRuleStatus,
}

/// Artifact-local backtest run manifest with resolved NT/default surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestRunManifestArtifact {
    pub manifest_version: String,
    pub submitted_manifest_hash: String,
    pub currentness_rule_slots: Vec<ManifestCurrentnessRuleSlot>,
    pub manifest: BacktestingRunManifest,
    pub resolved_nt_surfaces: Vec<ResolvedNtSurface>,
}

fn deferred_currentness_rule_slot(
    dimension: ManifestCurrentnessDimension,
) -> ManifestCurrentnessRuleSlot {
    ManifestCurrentnessRuleSlot {
        dimension,
        status: ManifestCurrentnessRuleStatus::Deferred,
    }
}

fn manifest_currentness_rule_slots() -> Vec<ManifestCurrentnessRuleSlot> {
    vec![
        deferred_currentness_rule_slot(ManifestCurrentnessDimension::NtVersion),
        deferred_currentness_rule_slot(ManifestCurrentnessDimension::StrategyConfigHash),
        deferred_currentness_rule_slot(ManifestCurrentnessDimension::ManifestSchema),
        deferred_currentness_rule_slot(ManifestCurrentnessDimension::ExecutionModel),
    ]
}

fn option_value<T: Debug>(value: Option<T>) -> String {
    match value {
        Some(value) => format!("{value:?}"),
        None => "None".to_string(),
    }
}

fn storage_option_keys_value<'a, I>(value: Option<I>) -> String
where
    I: IntoIterator<Item = (&'a String, &'a String)>,
{
    let Some(value) = value else {
        return "None".to_string();
    };
    let mut keys = value
        .into_iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>();
    keys.sort_unstable();
    format!("keys={keys:?}")
}

fn resolved_surface(
    surface: &str,
    classification: NtSurfaceClassification,
    nt_field: &str,
    resolved_value: impl Into<String>,
) -> ResolvedNtSurface {
    ResolvedNtSurface {
        surface: surface.to_string(),
        classification,
        nt_field: nt_field.to_string(),
        resolved_value: resolved_value.into(),
    }
}

fn manifest_time_to_nanos(field: &'static str, value: i64) -> Result<UnixNanos, ManifestError> {
    u64::try_from(value)
        .map(UnixNanos::from)
        .map_err(|_| ManifestError::NegativeTime { field, value })
}

/// Structured reason for pinning a non-latest accepted source proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofPinReasonCode {
    BaselineReproduction,
    PublishedResultReproduction,
    RegressionComparison,
    AuditOrInvestigation,
    MigrationValidation,
}

/// Source of the typed strategy config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySourceKind {
    /// Existing compiled Rust strategy selected directly from the registry.
    CompiledRustRegistry,
    /// Human-authored typed config with immutable artifact provenance.
    HumanTypedConfig,
    /// Typed config generated by a Research Analytics experiment result.
    ResearchAnalyticsExperimentResult,
}

/// Admissible strategy execution target plus typed-config provenance.
///
/// The executable strategy is always a registered compiled Rust strategy
/// selected by key, with typed string parameters. There is deliberately no
/// variant for inline code, notebook code, a Python path, or an untracked blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategySource {
    /// Provenance class for the typed strategy config.
    pub source_kind: StrategySourceKind,
    /// Key into [`registered_strategies`].
    pub registry_key: String,
    /// Typed parameters passed to the registered strategy constructor.
    pub parameters: BTreeMap<String, String>,
    /// Immutable typed config artifact URI, required for human/RA config sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_config_uri: Option<String>,
    /// SHA-256 of the typed config artifact, required for human/RA config sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_config_hash: Option<String>,
    /// Experiment-results artifact URI for RA-generated configs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_result_uri: Option<String>,
    /// SHA-256 of the experiment-results artifact for RA-generated configs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment_result_hash: Option<String>,
    /// Production config root plus one documented run-only overlay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_overlay: Option<StrategyConfigOverlaySource>,
}

/// Run-only strategy config overlay source.
///
/// The production root TOML is loaded unchanged, then the delta is applied in
/// memory for this backtest. The resolved delta is copied into the result
/// contract by the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StrategyConfigOverlaySource {
    pub production_root_config_path: String,
    pub override_delta: ManifestBacktestConfigOverride,
}

/// String-only backtest config delta encoded in the run manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBacktestConfigOverride {
    pub label: String,
    pub strategy_instance_id: String,
    pub signal_role: String,
    pub signal_data_client_id: String,
    pub signal_instrument_id: String,
    pub realized_volatility_surface_id: String,
    pub keep_realized_volatility_sources: Vec<ManifestRealizedVolatilitySourceSelector>,
}

/// String-only selector for an already-configured production RV source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRealizedVolatilitySourceSelector {
    pub data_client_id: String,
    pub instrument_id: String,
}

impl StrategyConfigOverlaySource {
    #[must_use]
    pub fn to_bolt_v3_override(&self) -> BacktestConfigOverride {
        BacktestConfigOverride {
            label: self.override_delta.label.clone(),
            strategy_instance_id: self.override_delta.strategy_instance_id.clone(),
            signal_role: self.override_delta.signal_role.clone(),
            signal_data_client_id: ClientId::from(
                self.override_delta.signal_data_client_id.as_str(),
            ),
            signal_instrument_id: InstrumentId::from(
                self.override_delta.signal_instrument_id.as_str(),
            ),
            realized_volatility_surface_id: self
                .override_delta
                .realized_volatility_surface_id
                .clone(),
            keep_realized_volatility_sources: self
                .override_delta
                .keep_realized_volatility_sources
                .iter()
                .map(|selector| RealizedVolatilitySourceSelector {
                    data_client_id: ClientId::from(selector.data_client_id.as_str()),
                    instrument_id: InstrumentId::from(selector.instrument_id.as_str()),
                })
                .collect(),
        }
    }
}

/// Simulated venue settings mapped into [`BacktestVenueConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestVenueConfig {
    /// NautilusTrader venue name.
    pub nt_venue: String,
    /// One of `NETTING` or `HEDGING`.
    pub oms_type: String,
    /// One of `CASH` or `MARGIN`.
    pub account_type: String,
    /// One of `L1_MBP`, `L2_MBP`, `L3_MBO`.
    pub book_type: String,
    /// Starting balances such as `["1_000_000 USDC"]`.
    pub starting_balances: Vec<String>,
    /// If multi-venue routing should be enabled for the execution client.
    pub routing: bool,
    /// If the account for this exchange is frozen.
    pub frozen_account: bool,
    /// If stop orders are rejected when trigger price is in the market.
    pub reject_stop_orders: bool,
    /// If GTD time-in-force orders are supported by the venue.
    pub support_gtd_orders: bool,
    /// If contingent orders are supported/respected by the venue.
    pub support_contingent_orders: bool,
    /// If venue position IDs are generated on fills.
    pub use_position_ids: bool,
    /// If venue order IDs and position IDs are random UUID4s.
    pub use_random_ids: bool,
    /// If reduce-only execution instructions are honored.
    pub use_reduce_only: bool,
    /// If bars should be processed by the matching engine.
    pub bar_execution: bool,
    /// If bar high/low ordering should use NT's adaptive heuristic.
    pub bar_adaptive_high_low_ordering: bool,
    /// If trades should be processed by the matching engine.
    pub trade_execution: bool,
    /// If market orders should emit `OrderAccepted` events.
    pub use_market_order_acks: bool,
    /// If order book liquidity consumption should be tracked per level.
    pub liquidity_consumption: bool,
    /// If negative cash balances are allowed.
    pub allow_cash_borrowing: bool,
    /// If limit order queue-position tracking is enabled.
    pub queue_position: bool,
    /// One of `PARTIAL` or `FULL`.
    pub oto_trigger_mode: String,
    /// Account base currency, or `NONE` for a multi-currency account.
    pub base_currency: String,
    /// Account default leverage as a decimal string.
    pub default_leverage: String,
    /// Exchange-calculated market-order price protection boundary in points.
    pub price_protection_points: u32,
    /// NT per-instrument leverage map. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leverages: Option<BTreeMap<String, String>>,
    /// NT margin model selector. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub margin_model: Option<String>,
    /// NT simulation module selectors. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modules: Option<Vec<String>>,
    /// Prediction-market fill-cost model registered with the NT simulated venue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_model: Option<ManifestFillModelConfig>,
    /// Prediction-market latency model registered with the NT simulated venue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_model: Option<ManifestLatencyModelConfig>,
    /// Prediction-market fee model registered with the NT simulated venue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_model: Option<ManifestFeeModelConfig>,
    /// NT settlement prices keyed by instrument id. Unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settlement_prices: Option<BTreeMap<String, String>>,
}

/// TOML-selected fill realism settings for a prediction-market backtest venue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFillModelConfig {
    pub kind: String,
    pub prob_fill_on_limit: String,
    pub prob_slippage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub random_seed: Option<u64>,
}

/// TOML-selected latency realism settings for a prediction-market backtest venue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestLatencyModelConfig {
    pub kind: String,
    pub base_latency_nanos: u64,
    pub insert_latency_nanos: u64,
    pub update_latency_nanos: u64,
    pub delete_latency_nanos: u64,
}

/// TOML-selected fee realism settings for a prediction-market backtest venue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestFeeModelConfig {
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestDomainMetricConfig {
    pub kind: String,
}

/// One immutable Parquet object in a portable physical catalog manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionManifestObject {
    /// Slash-separated path relative to the catalog root.
    pub relative_path: String,
    pub byte_len: u64,
    pub sha256: String,
}

/// Canonical, location-independent physical inventory for one NT catalog root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionManifestDocument {
    pub schema_version: String,
    pub objects: Vec<CatalogProjectionManifestObject>,
}

impl CatalogProjectionManifestDocument {
    /// Deterministic JSON bytes shared by local sealing and publication.
    pub fn canonical_bytes_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Vec<u8>> {
        self.validate_guarded(work_budget, stage)
            .map_err(|error| anyhow::anyhow!(error))?;
        serialize_json_to_vec_guarded(self, work_budget, stage)
    }

    /// SHA-256 of canonical JSON, streamed without materializing the JSON.
    pub fn manifest_sha256_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<String> {
        self.validate_guarded(work_budget, stage)
            .map_err(|error| anyhow::anyhow!(error))?;
        sha256_json_guarded(self, work_budget, stage)
    }

    fn budget_totals(&self) -> Result<(u64, u64, u64), ManifestError> {
        let object_count = u64::try_from(self.objects.len()).map_err(|_| {
            ManifestError::InvalidCatalogProjectionManifest {
                field: "catalog_run_view_authority.roots.physical_manifest.objects",
                message: "object count does not fit u64".to_string(),
            }
        })?;
        let schema_bytes = u64::try_from(self.schema_version.len()).map_err(|_| {
            ManifestError::InvalidCatalogProjectionManifest {
                field: "catalog_run_view_authority.roots.physical_manifest.schema_version",
                message: "schema metadata bytes do not fit u64".to_string(),
            }
        })?;
        let mut metadata_bytes = u64::try_from(size_of::<Self>())
            .ok()
            .and_then(|value| value.checked_add(schema_bytes))
            .ok_or_else(|| ManifestError::InvalidCatalogProjectionManifest {
                field: "catalog_run_view_authority.roots.physical_manifest",
                message: "metadata byte count overflow".to_string(),
            })?;
        let mut physical_bytes = 0_u64;
        for object in &self.objects {
            let record_bytes = size_of::<CatalogProjectionManifestObject>()
                .checked_add(object.relative_path.len())
                .and_then(|value| value.checked_add(object.sha256.len()))
                .ok_or_else(|| ManifestError::InvalidCatalogProjectionManifest {
                    field: "catalog_run_view_authority.roots.physical_manifest.objects",
                    message: "object metadata byte count overflow".to_string(),
                })?;
            metadata_bytes = metadata_bytes
                .checked_add(u64::try_from(record_bytes).map_err(|_| {
                    ManifestError::InvalidCatalogProjectionManifest {
                        field: "catalog_run_view_authority.roots.physical_manifest.objects",
                        message: "object metadata bytes do not fit u64".to_string(),
                    }
                })?)
                .ok_or_else(|| ManifestError::InvalidCatalogProjectionManifest {
                    field: "catalog_run_view_authority.roots.physical_manifest.objects",
                    message: "cumulative object metadata byte count overflow".to_string(),
                })?;
            physical_bytes = physical_bytes.checked_add(object.byte_len).ok_or_else(|| {
                ManifestError::InvalidCatalogProjectionManifest {
                    field: "catalog_run_view_authority.roots.physical_manifest.objects.byte_len",
                    message: "cumulative physical byte count overflow".to_string(),
                }
            })?;
        }
        Ok((object_count, metadata_bytes, physical_bytes))
    }

    pub fn validate_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<(), ManifestError> {
        let budget_error =
            |field, error: anyhow::Error| ManifestError::InvalidCatalogProjectionManifest {
                field,
                message: format!("work-budget rejection: {error:#}"),
            };
        work_budget
            .check_deadline(stage)
            .map_err(|error| budget_error("catalog_run_view_authority", error))?;
        let (object_count, metadata_bytes, physical_bytes) = self.budget_totals()?;
        work_budget
            .verify_actual_row_groups(object_count, stage)
            .map_err(|error| {
                budget_error(
                    "catalog_run_view_authority.roots.physical_manifest.objects",
                    error,
                )
            })?;
        work_budget
            .verify_decoded_bytes(metadata_bytes, stage)
            .map_err(|error| {
                budget_error(
                    "catalog_run_view_authority.roots.physical_manifest.metadata_bytes",
                    error,
                )
            })?;
        work_budget
            .verify_decoded_bytes(physical_bytes, stage)
            .map_err(|error| {
                budget_error(
                    "catalog_run_view_authority.roots.physical_manifest.physical_bytes",
                    error,
                )
            })?;
        work_budget
            .verify_decoded_bytes(
                metadata_bytes.checked_add(physical_bytes).ok_or_else(|| {
                    ManifestError::InvalidCatalogProjectionManifest {
                        field: "catalog_run_view_authority.roots.physical_manifest",
                        message: "physical plus metadata byte count overflow".to_string(),
                    }
                })?,
                stage,
            )
            .map_err(|error| budget_error("catalog_run_view_authority", error))?;
        if self.schema_version != CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::InvalidCatalogProjectionManifest {
                field: "catalog_run_view_authority.roots.physical_manifest.schema_version",
                message: format!("unsupported value {:?}", self.schema_version),
            });
        }
        if self.objects.is_empty() {
            return Err(ManifestError::MissingField(
                "catalog_run_view_authority.roots.physical_manifest.objects",
            ));
        }
        let mut previous_path: Option<&str> = None;
        for object in &self.objects {
            work_budget
                .check_deadline(stage)
                .map_err(|error| budget_error("catalog_run_view_authority", error))?;
            validate_catalog_projection_relative_path(&object.relative_path)?;
            if object.byte_len == 0 {
                return Err(ManifestError::InvalidCatalogProjectionManifest {
                    field: "catalog_run_view_authority.roots.physical_manifest.objects.byte_len",
                    message: format!("{} must have a positive byte length", object.relative_path),
                });
            }
            validate_strategy_source_hash(
                "catalog_run_view_authority.roots.physical_manifest.objects.sha256",
                &object.sha256,
            )?;
            if previous_path.is_some_and(|previous| previous >= object.relative_path.as_str()) {
                return Err(ManifestError::InvalidCatalogProjectionManifest {
                    field: "catalog_run_view_authority.roots.physical_manifest.objects.relative_path",
                    message: "objects must be strictly sorted by unique relative_path".to_string(),
                });
            }
            previous_path = Some(object.relative_path.as_str());
        }
        work_budget
            .check_deadline(stage)
            .map_err(|error| budget_error("catalog_run_view_authority", error))?;
        Ok(())
    }
}

/// Immutable catalog authority for one unique root used by one or more data inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRunViewRootAuthority {
    /// Sorted manifest inputs which resolve to this same physical root.
    pub catalog_inputs: Vec<CatalogRunViewInputAuthority>,
    /// Logical hash over decoded NT rows for the root.
    pub logical_catalog_hash: String,
    /// SHA-256 of `physical_manifest` canonical bytes.
    pub physical_manifest_sha256: String,
    /// Exact physical object set and content hashes authorized for the root.
    pub physical_manifest: CatalogProjectionManifestDocument,
}

/// Portable identity for one submitted catalog query, excluding hydrated path details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRunViewInputAuthority {
    pub catalog_input_index: u64,
    pub data_type: String,
    pub nt_instrument_id: String,
}

/// Validated immutable identity obtained from the submitted RunSpec, never a
/// path-rewritten runtime manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubmittedRunIdentity {
    run_id: String,
    manifest_hash: String,
    runtime_semantics_hash: String,
}

impl SubmittedRunIdentity {
    pub(crate) fn new(
        submitted_manifest: &BacktestingRunManifest,
        manifest_hash: &str,
    ) -> Result<Self, ManifestError> {
        if submitted_manifest.run_id.trim().is_empty() {
            return Err(ManifestError::MissingField("submitted_run_identity.run_id"));
        }
        validate_strategy_source_hash("submitted_run_identity.manifest_hash", manifest_hash)?;
        let actual_manifest_hash = submitted_manifest.manifest_hash();
        if actual_manifest_hash != manifest_hash {
            return Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "submitted_run_identity.manifest_hash",
                message: format!(
                    "declared {manifest_hash} does not match submitted manifest {actual_manifest_hash}"
                ),
            });
        }
        Ok(Self {
            run_id: submitted_manifest.run_id.clone(),
            manifest_hash: manifest_hash.to_string(),
            runtime_semantics_hash: runtime_manifest_semantics_hash(submitted_manifest),
        })
    }

    #[must_use]
    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    #[must_use]
    pub(crate) fn manifest_hash(&self) -> &str {
        &self.manifest_hash
    }

    fn validate_runtime_manifest(
        &self,
        runtime_manifest: &BacktestingRunManifest,
    ) -> Result<(), ManifestError> {
        if runtime_manifest.run_id != self.run_id {
            return Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.run_id",
                message: format!(
                    "trusted submitted run {:?} does not match runtime manifest {:?}",
                    self.run_id, runtime_manifest.run_id
                ),
            });
        }
        let runtime_semantics_hash = runtime_manifest_semantics_hash(runtime_manifest);
        if runtime_semantics_hash != self.runtime_semantics_hash {
            return Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.submitted_manifest_hash",
                message: "runtime manifest differs from submitted intent outside the allowed catalog location rewrite fields".to_string(),
            });
        }
        Ok(())
    }
}

fn runtime_manifest_semantics_hash(manifest: &BacktestingRunManifest) -> String {
    let mut normalized = manifest.clone();
    for input in &mut normalized.catalog_inputs {
        input.catalog_path.clear();
        input.catalog_fs_protocol.clear();
        input.catalog_fs_storage_options.clear();
        input.catalog_fs_rust_storage_options.clear();
    }
    normalized.manifest_hash()
}

/// Producer-minted, portable authority required by the sole BacktestNode path.
///
/// It is created only after catalog projection, binds to the exact submitted
/// run manifest, and is serialized unchanged for local execution, publication,
/// and later hydration. The submitted run manifest cannot authorize catalog
/// bytes which do not exist yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRunViewAuthority {
    pub schema_version: String,
    pub run_id: String,
    pub submitted_manifest_hash: String,
    pub roots: Vec<CatalogRunViewRootAuthority>,
}

impl CatalogRunViewAuthority {
    /// Deterministic bytes persisted and reused unchanged by publication/hydration.
    pub(crate) fn canonical_bytes_guarded(
        &self,
        runtime_manifest: &BacktestingRunManifest,
        submitted_identity: &SubmittedRunIdentity,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Vec<u8>> {
        self.validate_for_runtime_manifest(
            runtime_manifest,
            submitted_identity,
            work_budget,
            stage,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
        serialize_json_to_vec_guarded(self, work_budget, stage)
    }

    /// SHA-256 of canonical JSON, streamed without materializing the JSON.
    pub(crate) fn authority_sha256_guarded(
        &self,
        runtime_manifest: &BacktestingRunManifest,
        submitted_identity: &SubmittedRunIdentity,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<String> {
        self.validate_for_runtime_manifest(
            runtime_manifest,
            submitted_identity,
            work_budget,
            stage,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
        sha256_json_guarded(self, work_budget, stage)
    }

    /// Validate canonical structure and bind every catalog input exactly once.
    pub(crate) fn validate_for_runtime_manifest(
        &self,
        manifest: &BacktestingRunManifest,
        submitted_identity: &SubmittedRunIdentity,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<(), ManifestError> {
        let budget_error =
            |field, error: anyhow::Error| ManifestError::InvalidCatalogRunViewAuthority {
                field,
                message: format!("work-budget rejection: {error:#}"),
            };
        work_budget
            .check_deadline(stage)
            .map_err(|error| budget_error("catalog_run_view_authority", error))?;
        if self.schema_version != CATALOG_RUN_VIEW_AUTHORITY_SCHEMA_VERSION {
            return Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.schema_version",
                message: format!("unsupported value {:?}", self.schema_version),
            });
        }
        validate_strategy_source_hash(
            "catalog_run_view_authority.submitted_manifest_hash",
            &self.submitted_manifest_hash,
        )?;
        if self.run_id != submitted_identity.run_id
            || self.submitted_manifest_hash != submitted_identity.manifest_hash
        {
            return Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.submitted_manifest_hash",
                message: format!(
                    "authority ({:?}, {}) does not match trusted submitted identity ({:?}, {})",
                    self.run_id,
                    self.submitted_manifest_hash,
                    submitted_identity.run_id,
                    submitted_identity.manifest_hash
                ),
            });
        }
        submitted_identity.validate_runtime_manifest(manifest)?;
        if self.roots.is_empty() {
            return Err(ManifestError::MissingField(
                "catalog_run_view_authority.roots",
            ));
        }

        let mut cumulative_objects = 0_u64;
        let schema_bytes = u64::try_from(self.schema_version.len()).map_err(|_| {
            ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.schema_version",
                message: "schema metadata bytes do not fit u64".to_string(),
            }
        })?;
        let run_id_bytes = u64::try_from(self.run_id.len()).map_err(|_| {
            ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.run_id",
                message: "run-id metadata bytes do not fit u64".to_string(),
            }
        })?;
        let submitted_hash_bytes =
            u64::try_from(self.submitted_manifest_hash.len()).map_err(|_| {
                ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.submitted_manifest_hash",
                    message: "submitted-hash metadata bytes do not fit u64".to_string(),
                }
            })?;
        let mut cumulative_metadata = u64::try_from(size_of::<Self>())
            .ok()
            .and_then(|value| value.checked_add(schema_bytes))
            .and_then(|value| value.checked_add(run_id_bytes))
            .and_then(|value| value.checked_add(submitted_hash_bytes))
            .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority",
                message: "authority metadata byte count overflow".to_string(),
            })?;
        let mut cumulative_physical = 0_u64;

        let mut previous_first_index = None;
        for (root_index, root) in self.roots.iter().enumerate() {
            work_budget
                .check_deadline(stage)
                .map_err(|error| budget_error("catalog_run_view_authority", error))?;
            if root.catalog_inputs.is_empty() {
                return Err(ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: format!("roots[{root_index}] has no catalog input indexes"),
                });
            }
            let first_index = root.catalog_inputs[0].catalog_input_index;
            if previous_first_index.is_some_and(|previous| previous >= first_index) {
                return Err(ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: "roots must be strictly sorted by first catalog input index"
                        .to_string(),
                });
            }
            previous_first_index = Some(first_index);

            let mut previous_input_index = None;
            let mut expected_path: Option<&str> = None;
            for binding in &root.catalog_inputs {
                work_budget
                    .check_deadline(stage)
                    .map_err(|error| budget_error("catalog_run_view_authority", error))?;
                let declared_index = binding.catalog_input_index;
                if previous_input_index.is_some_and(|previous| previous >= declared_index) {
                    return Err(ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.catalog_inputs",
                        message: format!(
                            "roots[{root_index}] indexes must be strictly sorted and unique"
                        ),
                    });
                }
                let binding_bytes = size_of::<CatalogRunViewInputAuthority>()
                    .checked_add(binding.data_type.len())
                    .and_then(|value| value.checked_add(binding.nt_instrument_id.len()))
                    .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.catalog_inputs",
                        message: "catalog input binding metadata byte count overflow".to_string(),
                    })?;
                cumulative_metadata = cumulative_metadata
                    .checked_add(u64::try_from(binding_bytes).map_err(|_| {
                        ManifestError::InvalidCatalogRunViewAuthority {
                            field: "catalog_run_view_authority.roots.catalog_inputs",
                            message: "catalog input binding bytes do not fit u64".to_string(),
                        }
                    })?)
                    .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority",
                        message: "cumulative authority metadata byte count overflow".to_string(),
                    })?;
                work_budget
                    .verify_decoded_bytes(cumulative_metadata, stage)
                    .map_err(|error| budget_error("catalog_run_view_authority", error))?;
                previous_input_index = Some(declared_index);
                let index = usize::try_from(declared_index).map_err(|_| {
                    ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.catalog_inputs",
                        message: format!("catalog input index {declared_index} does not fit usize"),
                    }
                })?;
                let input = manifest.catalog_inputs.get(index).ok_or_else(|| {
                    ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.catalog_inputs",
                        message: format!(
                            "catalog input index {declared_index} is outside {} submitted inputs",
                            manifest.catalog_inputs.len()
                        ),
                    }
                })?;
                if binding.data_type != input.data_type
                    || binding.nt_instrument_id != input.nt_instrument_id
                {
                    return Err(ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.catalog_inputs",
                        message: format!(
                            "catalog input {declared_index} type/instrument ({:?}, {:?}) does not match runtime ({:?}, {:?})",
                            binding.data_type,
                            binding.nt_instrument_id,
                            input.data_type,
                            input.nt_instrument_id
                        ),
                    });
                }
                if let Some(path) = expected_path {
                    if path != input.catalog_path {
                        return Err(ManifestError::InvalidCatalogRunViewAuthority {
                            field: "catalog_run_view_authority.roots.catalog_inputs",
                            message: format!(
                                "roots[{root_index}] groups different catalog paths {path:?} and {:?}",
                                input.catalog_path
                            ),
                        });
                    }
                } else {
                    expected_path = Some(input.catalog_path.as_str());
                }
            }

            validate_strategy_source_hash(
                "catalog_run_view_authority.roots.logical_catalog_hash",
                &root.logical_catalog_hash,
            )?;
            validate_strategy_source_hash(
                "catalog_run_view_authority.roots.physical_manifest_sha256",
                &root.physical_manifest_sha256,
            )?;
            let (root_objects, root_metadata, root_physical) =
                root.physical_manifest.budget_totals()?;
            cumulative_objects = cumulative_objects
                .checked_add(root_objects)
                .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.physical_manifest.objects",
                    message: "cumulative physical object count overflow".to_string(),
                })?;
            let logical_hash_bytes =
                u64::try_from(root.logical_catalog_hash.len()).map_err(|_| {
                    ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.logical_catalog_hash",
                        message: "logical-hash metadata bytes do not fit u64".to_string(),
                    }
                })?;
            let physical_hash_bytes =
                u64::try_from(root.physical_manifest_sha256.len()).map_err(|_| {
                    ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.physical_manifest_sha256",
                        message: "physical-hash metadata bytes do not fit u64".to_string(),
                    }
                })?;
            cumulative_metadata = cumulative_metadata
                .checked_add(root_metadata)
                .and_then(|value| {
                    value.checked_add(u64::try_from(size_of::<CatalogRunViewRootAuthority>()).ok()?)
                })
                .and_then(|value| value.checked_add(logical_hash_bytes))
                .and_then(|value| value.checked_add(physical_hash_bytes))
                .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots",
                    message: "cumulative root metadata byte count overflow".to_string(),
                })?;
            cumulative_physical =
                cumulative_physical
                    .checked_add(root_physical)
                    .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                        field: "catalog_run_view_authority.roots.physical_manifest.physical_bytes",
                        message: "cumulative physical byte count overflow".to_string(),
                    })?;
            work_budget
                .verify_actual_row_groups(cumulative_objects, stage)
                .map_err(|error| {
                    budget_error(
                        "catalog_run_view_authority.roots.physical_manifest.objects",
                        error,
                    )
                })?;
            work_budget
                .verify_decoded_bytes(cumulative_metadata, stage)
                .map_err(|error| {
                    budget_error("catalog_run_view_authority.metadata_bytes", error)
                })?;
            work_budget
                .verify_decoded_bytes(cumulative_physical, stage)
                .map_err(|error| {
                    budget_error("catalog_run_view_authority.physical_bytes", error)
                })?;
            work_budget
                .verify_decoded_bytes(
                    cumulative_metadata
                        .checked_add(cumulative_physical)
                        .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                            field: "catalog_run_view_authority",
                            message: "cumulative authority byte count overflow".to_string(),
                        })?,
                    stage,
                )
                .map_err(|error| budget_error("catalog_run_view_authority", error))?;
            root.physical_manifest
                .validate_guarded(work_budget, stage)?;
            let actual_physical_hash = root
                .physical_manifest
                .manifest_sha256_guarded(work_budget, stage)
                .map_err(|error| {
                    budget_error(
                        "catalog_run_view_authority.roots.physical_manifest_sha256",
                        error,
                    )
                })?;
            if actual_physical_hash != root.physical_manifest_sha256 {
                return Err(ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.physical_manifest_sha256",
                    message: format!(
                        "declared {} does not match physical manifest {}",
                        root.physical_manifest_sha256, actual_physical_hash
                    ),
                });
            }
        }

        for input_index in 0..manifest.catalog_inputs.len() {
            work_budget
                .check_deadline(stage)
                .map_err(|error| budget_error("catalog_run_view_authority", error))?;
            let input_index = u64::try_from(input_index).map_err(|_| {
                ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: "submitted catalog input count does not fit u64".to_string(),
                }
            })?;
            let occurrences = self
                .roots
                .iter()
                .filter(|root| {
                    root.catalog_inputs
                        .binary_search_by_key(&input_index, |binding| binding.catalog_input_index)
                        .is_ok()
                })
                .count();
            if occurrences != 1 {
                return Err(ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: format!(
                        "catalog input index {input_index} must appear exactly once, found {occurrences}"
                    ),
                });
            }
        }
        for (left_index, left) in self.roots.iter().enumerate() {
            let left_manifest_index = usize::try_from(left.catalog_inputs[0].catalog_input_index)
                .map_err(|_| {
                ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: "validated catalog input index no longer fits usize".to_string(),
                }
            })?;
            let left_path = &manifest
                .catalog_inputs
                .get(left_manifest_index)
                .ok_or_else(|| ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: "validated catalog input index left runtime bounds".to_string(),
                })?
                .catalog_path;
            if self.roots.iter().skip(left_index + 1).any(|right| {
                usize::try_from(right.catalog_inputs[0].catalog_input_index)
                    .ok()
                    .and_then(|index| manifest.catalog_inputs.get(index))
                    .is_some_and(|input| input.catalog_path == *left_path)
            }) {
                return Err(ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.roots.catalog_inputs",
                    message: format!("catalog path {left_path:?} appears in more than one root"),
                });
            }
        }
        work_budget
            .check_deadline(stage)
            .map_err(|error| budget_error("catalog_run_view_authority", error))?;
        Ok(())
    }

    /// Terminal binding check against the immutable submitted run specification.
    #[cfg(test)]
    pub(crate) fn validate_submitted_manifest_identity(
        &self,
        submitted_identity: &SubmittedRunIdentity,
    ) -> Result<(), ManifestError> {
        if self.run_id != submitted_identity.run_id
            || self.submitted_manifest_hash != submitted_identity.manifest_hash
        {
            return Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.submitted_manifest_hash",
                message: format!(
                    "authority ({:?}, {}) does not bind submitted manifest ({:?}, {})",
                    self.run_id,
                    self.submitted_manifest_hash,
                    submitted_identity.run_id,
                    submitted_identity.manifest_hash
                ),
            });
        }
        Ok(())
    }
}

/// Catalog input mapped into [`BacktestDataConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestCatalogInput {
    pub catalog_path: String,
    /// Catalog filesystem protocol, or `NONE` when `catalog_path` is already complete.
    pub catalog_fs_protocol: String,
    /// NT filesystem storage options for Python/fsspec-compatible paths.
    pub catalog_fs_storage_options: BTreeMap<String, String>,
    /// NT Rust object-store options for cloud-backed catalog paths.
    pub catalog_fs_rust_storage_options: BTreeMap<String, String>,
    /// NautilusTrader data type.
    pub data_type: String,
    /// NautilusTrader instrument id, such as `SYMBOL.VENUE`.
    pub nt_instrument_id: String,
    /// NT multi-instrument query ids. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument_ids: Option<Vec<String>>,
    /// NT data-query start time. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<i64>,
    /// NT data-query end time. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_time: Option<i64>,
    /// NT catalog filter expression. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_expr: Option<String>,
    /// NT data client id for routing catalog data to named strategy subscriptions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// NT catalog query metadata. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, String>>,
    /// NT bar specification. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_spec: Option<String>,
    /// NT explicit bar type strings. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_types: Option<Vec<String>>,
    /// NT directory-based file loading optimization. Declared but unsupported until mapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimize_file_loading: Option<bool>,
}

/// Reconstructed reference-current-price custom data replayed into the
/// BacktestEngine data queue.
///
/// NT BacktestRunConfig does not expose CustomData as a catalog-loaded enum
/// variant, so these records are manifest-owned side inputs. The runner maps
/// them into the production `ReferencePriceUpdate` custom-data type before
/// `BacktestNode::run`, and result labels must keep them reconstructed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestReferenceCurrentPriceInput {
    pub client_id: String,
    pub asset: String,
    pub source_id: String,
    pub provider: String,
    pub provider_instrument: String,
    pub price: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
    pub observed_ts_ms: u64,
    pub received_ts_ms: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provenance: BTreeMap<String, String>,
}

/// Instrument settlement (resolution) side input.
///
/// Replayed as an NT `InstrumentClose` (`ContractExpired`) so a held position
/// redeems to its resolved value (binary outcome: winner `1.0`, loser `0.0`) and
/// books a realized P/L at end-of-market. The `close_price` is the REAL market
/// resolution observed in the replayed archive — it is not synthesized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestInstrumentSettlementInput {
    /// NT instrument id whose held position settles at resolution.
    pub nt_instrument_id: String,
    /// Resolution/redemption price in the instrument's own units (binary `0..=1`).
    pub close_price: String,
    /// Price precision (decimal places) the `close_price` is quoted to.
    pub price_precision: u8,
    /// UNIX timestamp (nanoseconds) the resolution occurred.
    pub ts_event_ns: u64,
    /// UNIX timestamp (nanoseconds) the settlement event was created.
    pub ts_init_ns: u64,
    /// Settlement (collateral) currency the held position redeems in. The
    /// settlement builder binds it to the holding venue's funded
    /// `starting_balances` so NautilusTrader cannot silently drop a realized PnL
    /// booked in a currency the account was never funded in. Required (not
    /// optional): a settlement cannot be injected without declaring its currency,
    /// so the funded-venue check is unconditional and can never be skipped by
    /// omitting the field.
    pub settlement_currency: String,
}

/// Artifact output store options used for publishing and published-catalog proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestArtifactStore {
    /// NT/object-store options for Python/fsspec-compatible artifact writes.
    pub storage_options: BTreeMap<String, String>,
    /// NT/object-store options for the Rust cloud-backed artifact path.
    pub rust_storage_options: BTreeMap<String, String>,
    /// SSM parameters resolved into the Rust object-store options at runtime.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ssm_parameters: Option<ManifestArtifactStoreSsmParameters>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactSubpath {
    Raw,
    NtCatalog,
    SourceProofs,
    Backtests,
    ArtifactIndex,
    ResearchAnalytics,
}

impl ArtifactSubpath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::NtCatalog => "nt-catalog",
            Self::SourceProofs => "source-proofs",
            Self::Backtests => "backtests",
            Self::ArtifactIndex => "artifact-index",
            Self::ResearchAnalytics => "research-analytics",
        }
    }
}

/// SSM parameter paths for artifact-store credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestArtifactStoreSsmParameters {
    /// AWS region used when resolving the configured SSM parameters.
    pub region: String,
    /// SSM parameter path whose decrypted value is the S3 access key id.
    pub access_key_id: String,
    /// SSM parameter path whose decrypted value is the S3 secret access key.
    pub secret_access_key: String,
    /// Optional SSM parameter path whose decrypted value is the S3 session token.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub session_token: Option<String>,
}

/// The typed backtest run manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestingRunManifest {
    pub manifest_schema_version: String,
    pub run_id: String,
    /// Target `bolt-v2` branch used to resolve this run's dependencies/config.
    pub target_bolt_v2_branch: String,
    /// Exact target `bolt-v2` ref or commit used for this run.
    pub target_bolt_v2_ref: String,
    /// Resolved NautilusTrader revision/version for this run.
    pub resolved_nt_version: String,
    pub market_structure_fixture: MarketStructureFixture,
    /// TOML/registry venue/provider binding key.
    pub venue_binding_key: String,
    pub run_purpose: RunPurpose,
    /// Source proof id/version governing the catalog input.
    pub source_proof_id: String,
    pub source_proof_version: u32,
    /// True when this manifest pins a non-latest accepted proof.
    pub pins_non_latest_proof: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proof_pin_reason_code: Option<ProofPinReasonCode>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub proof_pin_reason_detail: Option<String>,
    pub strategy: StrategySource,
    /// SHA-256 of the effective typed strategy config.
    pub strategy_config_hash: String,
    pub venue: ManifestVenueConfig,
    /// Additional simulated venues needed by non-execution data feeds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_venues: Vec<ManifestVenueConfig>,
    /// Positive TOML-owned NT streaming chunk size. A value is mandatory so
    /// BacktestNode never falls back to whole-catalog one-shot loading.
    pub nt_streaming_chunk_size: u64,
    pub catalog_inputs: Vec<ManifestCatalogInput>,
    /// Reconstructed reference-current-price custom data side input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconstructed_reference_current_price: Vec<ManifestReferenceCurrentPriceInput>,
    /// Instrument settlement (resolution) side inputs replayed as NT
    /// `InstrumentClose` so held positions redeem to their resolved value.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instrument_settlements: Vec<ManifestInstrumentSettlementInput>,
    /// Execution model selected for this run, for example `nt_backtest_node`.
    pub execution_model: String,
    /// Configured S3 artifact root (TOML/config-owned).
    pub artifact_root: String,
    /// Output prefix under `artifact_root/backtests/`.
    pub output_prefix: String,
    /// Artifact-store options for output publication and direct catalog proof.
    pub artifact_store: ManifestArtifactStore,
    /// Domain statistics registered with NT PortfolioAnalyzer for this run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub domain_metrics: Vec<ManifestDomainMetricConfig>,
    /// Optional inclusive start time (Unix nanos).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub start_time: Option<i64>,
    /// Optional inclusive end time (Unix nanos).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub end_time: Option<i64>,
}

/// Why a manifest is invalid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    MissingField(&'static str),
    InlineStrategyCode {
        registry_key: String,
    },
    NotebookRuntimeStrategy {
        registry_key: String,
    },
    PythonStrategyPath {
        registry_key: String,
    },
    UntrackedConfigBlob {
        registry_key: String,
    },
    UnknownStrategy {
        registry_key: String,
    },
    InvalidStrategyRegistryKey {
        registry_key: String,
    },
    UnknownStrategyParameter {
        registry_key: String,
        parameter: String,
    },
    InvalidStrategySourceHash {
        field: &'static str,
        value: String,
    },
    StrategySourceFieldNotAllowed {
        source_kind: StrategySourceKind,
        field: &'static str,
    },
    StrategySourceOutsideAllowedArtifactRoot {
        field: &'static str,
        uri: String,
        expected_prefix: String,
    },
    InvalidStrategyConfigOverlay {
        field: &'static str,
        message: String,
    },
    InvalidStartingBalance {
        balance: String,
    },
    InvalidBaseCurrency {
        currency: String,
    },
    InvalidDefaultLeverage {
        leverage: String,
    },
    InvalidVenueModelParameter {
        field: &'static str,
        value: String,
    },
    InvalidNtConfig {
        field: &'static str,
        message: String,
    },
    InvalidCatalogProjectionManifest {
        field: &'static str,
        message: String,
    },
    InvalidCatalogRunViewAuthority {
        field: &'static str,
        message: String,
    },
    InvalidInstrumentId {
        instrument_id: String,
    },
    UnsupportedInstrumentIdCharset {
        instrument_id: String,
    },
    UnacceptedData {
        manifest_proof: String,
        accepted_proof: String,
    },
    BindingMismatch {
        manifest_binding: String,
        accepted_binding: String,
    },
    FixtureMismatch {
        manifest_fixture: MarketStructureFixture,
        accepted_fixture: FixtureType,
    },
    DataTypeFidelityMismatch {
        data_type: String,
        fidelity_class: SourceProofFidelityClass,
    },
    OrderBookDeltaRequiresL2Mbp {
        book_type: String,
    },
    NonLatestProofPinForNormalRun,
    UnsupportedDataType {
        data_type: String,
    },
    UnsupportedEnum {
        field: &'static str,
        value: String,
    },
    UnsupportedNtSurface {
        field: &'static str,
    },
    OutputPrefixOutsideArtifactRoot,
    NegativeTime {
        field: &'static str,
        value: i64,
    },
    InvertedTimeWindow {
        start: i64,
        end: i64,
    },
    RawCredentialOption {
        field: &'static str,
        key: String,
    },
    ArtifactStoreS3CredentialsNotResolved,
    InvalidArtifactStoreSsmParameter {
        field: &'static str,
    },
    ArtifactStoreSecretResolution {
        field: &'static str,
        source: String,
    },
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::InlineStrategyCode { registry_key } => {
                write!(
                    f,
                    "inline strategy code is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::NotebookRuntimeStrategy { registry_key } => {
                write!(
                    f,
                    "notebook runtime code is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::PythonStrategyPath { registry_key } => {
                write!(
                    f,
                    "Python strategy path is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::UntrackedConfigBlob { registry_key } => {
                write!(
                    f,
                    "untracked config blob is not an accepted strategy source: {registry_key:?}"
                )
            }
            Self::UnknownStrategy { registry_key } => {
                write!(
                    f,
                    "strategy {registry_key:?} is not a registered compiled Rust strategy"
                )
            }
            Self::InvalidStrategyRegistryKey { registry_key } => {
                write!(
                    f,
                    "strategy registry key must contain only ASCII letters, digits, '_' or '-': {registry_key:?}"
                )
            }
            Self::UnknownStrategyParameter {
                registry_key,
                parameter,
            } => write!(
                f,
                "parameter {parameter:?} is not accepted for strategy {registry_key:?}"
            ),
            Self::InvalidStrategySourceHash { field, value } => {
                write!(f, "{field} must be lowercase sha256 hex, got {value:?}")
            }
            Self::StrategySourceFieldNotAllowed { source_kind, field } => write!(
                f,
                "{field} is not allowed for strategy source kind {source_kind:?}"
            ),
            Self::StrategySourceOutsideAllowedArtifactRoot {
                field,
                uri,
                expected_prefix,
            } => write!(
                f,
                "{field} {uri:?} is outside the allowed strategy source prefix {expected_prefix:?}"
            ),
            Self::InvalidStrategyConfigOverlay { field, message } => {
                write!(f, "invalid {field}: {message}")
            }
            Self::InvalidStartingBalance { balance } => {
                write!(f, "invalid starting balance: {balance:?}")
            }
            Self::InvalidBaseCurrency { currency } => {
                write!(f, "invalid base currency: {currency:?}")
            }
            Self::InvalidDefaultLeverage { leverage } => {
                write!(f, "invalid default leverage: {leverage:?}")
            }
            Self::InvalidVenueModelParameter { field, value } => {
                write!(f, "invalid venue model parameter {field}: {value:?}")
            }
            Self::InvalidNtConfig { field, message } => {
                write!(f, "invalid NautilusTrader {field} config: {message}")
            }
            Self::InvalidCatalogProjectionManifest { field, message } => {
                write!(f, "invalid {field}: {message}")
            }
            Self::InvalidCatalogRunViewAuthority { field, message } => {
                write!(f, "invalid {field}: {message}")
            }
            Self::InvalidInstrumentId { instrument_id } => {
                write!(f, "invalid instrument id: {instrument_id:?}")
            }
            Self::UnsupportedInstrumentIdCharset { instrument_id } => {
                write!(
                    f,
                    "instrument id {instrument_id:?} contains characters outside the \
                     catalog-directory-safe ASCII set (alphanumeric, '.', '_', '-'); such ids \
                     corrupt through the object-store percent-encoding layer and cannot be \
                     queried reliably from an NT catalog"
                )
            }
            Self::UnacceptedData {
                manifest_proof,
                accepted_proof,
            } => write!(
                f,
                "manifest source proof {manifest_proof:?} does not match accepted dataset proof {accepted_proof:?}"
            ),
            Self::BindingMismatch {
                manifest_binding,
                accepted_binding,
            } => write!(
                f,
                "manifest venue_binding_key {manifest_binding:?} does not match accepted source binding {accepted_binding:?}"
            ),
            Self::FixtureMismatch {
                manifest_fixture,
                accepted_fixture,
            } => write!(
                f,
                "manifest market_structure_fixture {manifest_fixture:?} does not match accepted.fixture_type {accepted_fixture:?}"
            ),
            Self::DataTypeFidelityMismatch {
                data_type,
                fidelity_class,
            } => write!(
                f,
                "catalog data type {data_type:?} is incompatible with accepted fidelity {fidelity_class:?}"
            ),
            Self::OrderBookDeltaRequiresL2Mbp { book_type } => write!(
                f,
                "order-book-delta catalog inputs require venue.book_type L2_MBP; bolt converters emit L2 (F_LAST) deltas with no per-order identity, so book_type {book_type:?} (e.g. L3_MBO) would collapse every level change onto order_id 0 and silently corrupt the book"
            ),
            Self::NonLatestProofPinForNormalRun => {
                write!(f, "normal runs cannot pin a non-latest source proof")
            }
            Self::UnsupportedDataType { data_type } => {
                write!(f, "unsupported catalog data type: {data_type:?}")
            }
            Self::UnsupportedEnum { field, value } => {
                write!(f, "unsupported value {value:?} for {field}")
            }
            Self::UnsupportedNtSurface { field } => write!(
                f,
                "unsupported NT surface {field}: add typed NT config mapping before use"
            ),
            Self::OutputPrefixOutsideArtifactRoot => {
                write!(f, "output prefix must live under artifact_root/backtests/")
            }
            Self::NegativeTime { field, value } => {
                write!(f, "{field} must not be negative: {value}")
            }
            Self::InvertedTimeWindow { start, end } => {
                write!(f, "start_time {start} must not be after end_time {end}")
            }
            Self::RawCredentialOption { field, key } => write!(
                f,
                "{field}.{key} contains raw credential material; credentials resolve from SSM at runtime and must never appear in a manifest"
            ),
            Self::ArtifactStoreS3CredentialsNotResolved => write!(
                f,
                "artifact_store.ssm_parameters must resolve access_key_id and secret_access_key before publishing to an s3 output_prefix"
            ),
            Self::InvalidArtifactStoreSsmParameter { field } => write!(
                f,
                "artifact_store.ssm_parameters.{field} must be an absolute SSM parameter path without whitespace"
            ),
            Self::ArtifactStoreSecretResolution { field, source } => write!(
                f,
                "artifact_store.ssm_parameters.{field} SSM resolution failed: {source}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

fn validate_strategy_source(
    strategy: &StrategySource,
    artifact_root: &str,
) -> Result<(), ManifestError> {
    let key = strategy.registry_key.trim();
    if key.is_empty() {
        return Err(ManifestError::MissingField("strategy.registry_key"));
    }
    // Reject executable code masquerading as a key.
    if key.contains(['{', '}', ';', '\n', '(', ')']) || key.contains("fn ") {
        return Err(ManifestError::InlineStrategyCode {
            registry_key: key.to_string(),
        });
    }
    // Reject notebook runtime paths before the generic filesystem-path guard.
    if key.ends_with(".ipynb") || key.contains(".ipynb:") {
        return Err(ManifestError::NotebookRuntimeStrategy {
            registry_key: key.to_string(),
        });
    }
    // Reject Python runtime paths.
    if key.ends_with(".py") || key.contains(".py:") {
        return Err(ManifestError::PythonStrategyPath {
            registry_key: key.to_string(),
        });
    }
    // Reject untracked config blobs / filesystem paths; registry keys are bare.
    if key.contains('/') || key.contains('\\') || key.contains("..") || key.ends_with(".toml") {
        return Err(ManifestError::UntrackedConfigBlob {
            registry_key: key.to_string(),
        });
    }
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(ManifestError::InvalidStrategyRegistryKey {
            registry_key: key.to_string(),
        });
    }
    if !registered_strategies().contains(&key) {
        return Err(ManifestError::UnknownStrategy {
            registry_key: key.to_string(),
        });
    }
    let allowed_parameters =
        registered_strategy_parameters(key).ok_or_else(|| ManifestError::UnknownStrategy {
            registry_key: key.to_string(),
        })?;
    for parameter in strategy.parameters.keys() {
        if !allowed_parameters.contains(&parameter.as_str()) {
            return Err(ManifestError::UnknownStrategyParameter {
                registry_key: key.to_string(),
                parameter: parameter.clone(),
            });
        }
    }
    match key {
        STRATEGY_HURST_VPIN_DIRECTIONAL => {
            for parameter in [STRATEGY_PARAM_BAR_TYPE, STRATEGY_PARAM_TRADE_SIZE] {
                if !strategy.parameters.contains_key(parameter) {
                    return Err(ManifestError::MissingField(match parameter {
                        STRATEGY_PARAM_BAR_TYPE => "strategy.parameters.bar_type",
                        STRATEGY_PARAM_TRADE_SIZE => "strategy.parameters.trade_size",
                        _ => unreachable!(),
                    }));
                }
            }
            let trade_size = strategy
                .parameters
                .get(STRATEGY_PARAM_TRADE_SIZE)
                .expect("presence checked above");
            Quantity::from_str(trade_size)
                .map_err(|_| ManifestError::MissingField("strategy.parameters.trade_size"))?;
            let bar_type = strategy
                .parameters
                .get(STRATEGY_PARAM_BAR_TYPE)
                .expect("presence checked above");
            bar_type
                .parse::<BarType>()
                .map_err(|_| ManifestError::MissingField("strategy.parameters.bar_type"))?;
        }
        STRATEGY_BINARY_ORACLE_EDGE_TAKER => {
            let has_config_overlay = strategy.config_overlay.is_some();
            if has_config_overlay && strategy.parameters.contains_key(STRATEGY_PARAM_CONFIG_TOML) {
                return Err(ManifestError::InvalidStrategyConfigOverlay {
                    field: "strategy.config_overlay",
                    message: "cannot be combined with strategy.parameters.config_toml".to_string(),
                });
            }
            let mut required_parameters =
                vec![STRATEGY_PARAM_FEE_BPS, STRATEGY_PARAM_ORDER_EXECUTION_MODE];
            if !has_config_overlay {
                required_parameters.push(STRATEGY_PARAM_CONFIG_TOML);
            }
            for parameter in required_parameters {
                if !strategy.parameters.contains_key(parameter) {
                    return Err(ManifestError::MissingField(match parameter {
                        STRATEGY_PARAM_CONFIG_TOML => "strategy.parameters.config_toml",
                        STRATEGY_PARAM_FEE_BPS => "strategy.parameters.fee_bps",
                        STRATEGY_PARAM_ORDER_EXECUTION_MODE => {
                            "strategy.parameters.order_execution_mode"
                        }
                        _ => unreachable!(),
                    }));
                }
            }
            let order_execution_mode = strategy
                .parameters
                .get(STRATEGY_PARAM_ORDER_EXECUTION_MODE)
                .expect("presence checked above");
            let _order_execution_mode: BoltV3OrderExecutionMode =
                toml::Value::String(order_execution_mode.clone())
                    .try_into()
                    .map_err(|_| {
                        ManifestError::MissingField("strategy.parameters.order_execution_mode")
                    })?;
            let fee_bps = strategy
                .parameters
                .get(STRATEGY_PARAM_FEE_BPS)
                .expect("presence checked above");
            let fee_bps = rust_decimal::Decimal::from_str(fee_bps)
                .map_err(|_| ManifestError::MissingField("strategy.parameters.fee_bps"))?;
            if fee_bps < rust_decimal::Decimal::ZERO {
                return Err(ManifestError::MissingField("strategy.parameters.fee_bps"));
            }
            if !has_config_overlay {
                let raw_config = strategy
                    .parameters
                    .get(STRATEGY_PARAM_CONFIG_TOML)
                    .expect("presence checked above");
                let raw_config = toml::from_str::<toml::Value>(raw_config)
                    .map_err(|_| ManifestError::MissingField("strategy.parameters.config_toml"))?;
                let registry =
                    production_strategy_registry().map_err(|_| ManifestError::UnknownStrategy {
                        registry_key: BinaryOracleEdgeTakerBuilder::kind().to_string(),
                    })?;
                let mut errors = Vec::new();
                registry.validate(
                    BinaryOracleEdgeTakerBuilder::kind(),
                    &raw_config,
                    "strategy.parameters.config_toml",
                    &mut errors,
                );
                if !errors.is_empty() {
                    return Err(ManifestError::MissingField(
                        "strategy.parameters.config_toml",
                    ));
                }
            }
        }
        STRATEGY_BINARY_ORACLE_MAKER => {
            if strategy.config_overlay.is_some() {
                return Err(ManifestError::InvalidStrategyConfigOverlay {
                    field: "strategy.config_overlay",
                    message: format!("not supported for strategy {key:?}"),
                });
            }
            for parameter in [
                STRATEGY_PARAM_CONFIG_TOML,
                STRATEGY_PARAM_FEE_BPS,
                STRATEGY_PARAM_ORDER_EXECUTION_MODE,
            ] {
                if !strategy.parameters.contains_key(parameter) {
                    return Err(ManifestError::MissingField(match parameter {
                        STRATEGY_PARAM_CONFIG_TOML => "strategy.parameters.config_toml",
                        STRATEGY_PARAM_FEE_BPS => "strategy.parameters.fee_bps",
                        STRATEGY_PARAM_ORDER_EXECUTION_MODE => {
                            "strategy.parameters.order_execution_mode"
                        }
                        _ => unreachable!(),
                    }));
                }
            }
            let order_execution_mode = strategy
                .parameters
                .get(STRATEGY_PARAM_ORDER_EXECUTION_MODE)
                .expect("presence checked above");
            let _order_execution_mode: BoltV3OrderExecutionMode =
                toml::Value::String(order_execution_mode.clone())
                    .try_into()
                    .map_err(|_| {
                        ManifestError::MissingField("strategy.parameters.order_execution_mode")
                    })?;
            let fee_bps = strategy
                .parameters
                .get(STRATEGY_PARAM_FEE_BPS)
                .expect("presence checked above");
            let fee_bps = rust_decimal::Decimal::from_str(fee_bps)
                .map_err(|_| ManifestError::MissingField("strategy.parameters.fee_bps"))?;
            if fee_bps < rust_decimal::Decimal::ZERO {
                return Err(ManifestError::MissingField("strategy.parameters.fee_bps"));
            }
            let raw_config = strategy
                .parameters
                .get(STRATEGY_PARAM_CONFIG_TOML)
                .expect("presence checked above");
            let raw_config = toml::from_str::<toml::Value>(raw_config)
                .map_err(|_| ManifestError::MissingField("strategy.parameters.config_toml"))?;
            let registry =
                production_strategy_registry().map_err(|_| ManifestError::UnknownStrategy {
                    registry_key: key.to_string(),
                })?;
            let mut errors = Vec::new();
            registry.validate(
                key,
                &raw_config,
                "strategy.parameters.config_toml",
                &mut errors,
            );
            if !errors.is_empty() {
                return Err(ManifestError::MissingField(
                    "strategy.parameters.config_toml",
                ));
            }
        }
        STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE => {
            for parameter in [
                STRATEGY_PARAM_TRADE_SIZE,
                STRATEGY_PARAM_ENTRY_AFTER_TRADES,
                STRATEGY_PARAM_EXIT_AFTER_TRADES,
                STRATEGY_PARAM_SIDE,
            ] {
                if !strategy.parameters.contains_key(parameter) {
                    return Err(ManifestError::MissingField(match parameter {
                        STRATEGY_PARAM_TRADE_SIZE => "strategy.parameters.trade_size",
                        STRATEGY_PARAM_ENTRY_AFTER_TRADES => {
                            "strategy.parameters.entry_after_trades"
                        }
                        STRATEGY_PARAM_EXIT_AFTER_TRADES => "strategy.parameters.exit_after_trades",
                        STRATEGY_PARAM_SIDE => "strategy.parameters.side",
                        _ => unreachable!(),
                    }));
                }
            }
            let trade_size = strategy
                .parameters
                .get(STRATEGY_PARAM_TRADE_SIZE)
                .expect("presence checked above");
            let parsed_trade_size = Quantity::from_str(trade_size)
                .map_err(|_| ManifestError::MissingField("strategy.parameters.trade_size"))?;
            if !parsed_trade_size.is_positive() {
                return Err(ManifestError::UnsupportedEnum {
                    field: "strategy.parameters.trade_size",
                    value: trade_size.to_string(),
                });
            }
            let entry_after_trades = strategy
                .parameters
                .get(STRATEGY_PARAM_ENTRY_AFTER_TRADES)
                .expect("presence checked above")
                .parse::<u64>()
                .map_err(|_| {
                    ManifestError::MissingField("strategy.parameters.entry_after_trades")
                })?;
            if entry_after_trades == 0 {
                return Err(ManifestError::UnsupportedEnum {
                    field: "strategy.parameters.entry_after_trades",
                    value: entry_after_trades.to_string(),
                });
            }
            let exit_after_trades = strategy
                .parameters
                .get(STRATEGY_PARAM_EXIT_AFTER_TRADES)
                .expect("presence checked above")
                .parse::<u64>()
                .map_err(|_| {
                    ManifestError::MissingField("strategy.parameters.exit_after_trades")
                })?;
            if exit_after_trades == 0 {
                return Err(ManifestError::UnsupportedEnum {
                    field: "strategy.parameters.exit_after_trades",
                    value: exit_after_trades.to_string(),
                });
            }
            let side = strategy
                .parameters
                .get(STRATEGY_PARAM_SIDE)
                .expect("presence checked above");
            if !matches!(side.as_str(), "buy" | "sell") {
                return Err(ManifestError::UnsupportedEnum {
                    field: "strategy.parameters.side",
                    value: side.clone(),
                });
            }
        }
        _ => unreachable!("registered strategy was already matched"),
    }
    validate_strategy_config_overlay(strategy, key)?;
    validate_strategy_source_provenance(strategy, artifact_root)?;
    Ok(())
}

fn validate_strategy_config_overlay(
    strategy: &StrategySource,
    registry_key: &str,
) -> Result<(), ManifestError> {
    let Some(overlay) = &strategy.config_overlay else {
        return Ok(());
    };
    if registry_key != STRATEGY_BINARY_ORACLE_EDGE_TAKER {
        return Err(ManifestError::InvalidStrategyConfigOverlay {
            field: "strategy.config_overlay",
            message: format!("not supported for strategy {registry_key:?}"),
        });
    }
    for (field, value) in [
        (
            "strategy.config_overlay.production_root_config_path",
            overlay.production_root_config_path.as_str(),
        ),
        (
            "strategy.config_overlay.override_delta.label",
            overlay.override_delta.label.as_str(),
        ),
        (
            "strategy.config_overlay.override_delta.strategy_instance_id",
            overlay.override_delta.strategy_instance_id.as_str(),
        ),
        (
            "strategy.config_overlay.override_delta.signal_role",
            overlay.override_delta.signal_role.as_str(),
        ),
        (
            "strategy.config_overlay.override_delta.signal_data_client_id",
            overlay.override_delta.signal_data_client_id.as_str(),
        ),
        (
            "strategy.config_overlay.override_delta.signal_instrument_id",
            overlay.override_delta.signal_instrument_id.as_str(),
        ),
        (
            "strategy.config_overlay.override_delta.realized_volatility_surface_id",
            overlay
                .override_delta
                .realized_volatility_surface_id
                .as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(ManifestError::MissingField(field));
        }
    }
    overlay
        .override_delta
        .signal_instrument_id
        .parse::<InstrumentId>()
        .map_err(|_| ManifestError::InvalidInstrumentId {
            instrument_id: overlay.override_delta.signal_instrument_id.clone(),
        })?;
    if overlay
        .override_delta
        .keep_realized_volatility_sources
        .is_empty()
    {
        return Err(ManifestError::MissingField(
            "strategy.config_overlay.override_delta.keep_realized_volatility_sources",
        ));
    }
    let mut seen = BTreeSet::new();
    for selector in &overlay.override_delta.keep_realized_volatility_sources {
        for (field, value) in [
            (
                "strategy.config_overlay.override_delta.keep_realized_volatility_sources.data_client_id",
                selector.data_client_id.as_str(),
            ),
            (
                "strategy.config_overlay.override_delta.keep_realized_volatility_sources.instrument_id",
                selector.instrument_id.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ManifestError::MissingField(field));
            }
        }
        selector
            .instrument_id
            .parse::<InstrumentId>()
            .map_err(|_| ManifestError::InvalidInstrumentId {
                instrument_id: selector.instrument_id.clone(),
            })?;
        let key = (&selector.data_client_id, &selector.instrument_id);
        if !seen.insert(key) {
            return Err(ManifestError::InvalidStrategyConfigOverlay {
                field: "strategy.config_overlay.override_delta.keep_realized_volatility_sources",
                message: format!(
                    "duplicate selector {}:{}",
                    selector.data_client_id, selector.instrument_id
                ),
            });
        }
    }
    Ok(())
}

fn validate_strategy_source_provenance(
    strategy: &StrategySource,
    artifact_root: &str,
) -> Result<(), ManifestError> {
    match strategy.source_kind {
        StrategySourceKind::CompiledRustRegistry => {
            reject_strategy_source_field(
                strategy.typed_config_uri.as_ref(),
                strategy.source_kind,
                "strategy.typed_config_uri",
            )?;
            reject_strategy_source_field(
                strategy.typed_config_hash.as_ref(),
                strategy.source_kind,
                "strategy.typed_config_hash",
            )?;
            reject_strategy_source_field(
                strategy.experiment_result_uri.as_ref(),
                strategy.source_kind,
                "strategy.experiment_result_uri",
            )?;
            reject_strategy_source_field(
                strategy.experiment_result_hash.as_ref(),
                strategy.source_kind,
                "strategy.experiment_result_hash",
            )
        }
        StrategySourceKind::HumanTypedConfig => {
            if strategy.config_overlay.is_some() {
                return Err(ManifestError::InvalidStrategyConfigOverlay {
                    field: "strategy.config_overlay",
                    message: format!(
                        "not allowed for strategy source kind {:?}",
                        strategy.source_kind
                    ),
                });
            }
            validate_strategy_artifact_ref(
                "strategy.typed_config_uri",
                "strategy.typed_config_hash",
                strategy.typed_config_uri.as_deref(),
                strategy.typed_config_hash.as_deref(),
                &format!("{}/", artifact_root.trim_end_matches('/')),
            )?;
            reject_strategy_source_field(
                strategy.experiment_result_uri.as_ref(),
                strategy.source_kind,
                "strategy.experiment_result_uri",
            )?;
            reject_strategy_source_field(
                strategy.experiment_result_hash.as_ref(),
                strategy.source_kind,
                "strategy.experiment_result_hash",
            )
        }
        StrategySourceKind::ResearchAnalyticsExperimentResult => {
            if strategy.config_overlay.is_some() {
                return Err(ManifestError::InvalidStrategyConfigOverlay {
                    field: "strategy.config_overlay",
                    message: format!(
                        "not allowed for strategy source kind {:?}",
                        strategy.source_kind
                    ),
                });
            }
            let experiment_result_prefix =
                research_analytics_experiment_result_prefix(artifact_root);
            validate_strategy_artifact_ref(
                "strategy.typed_config_uri",
                "strategy.typed_config_hash",
                strategy.typed_config_uri.as_deref(),
                strategy.typed_config_hash.as_deref(),
                &experiment_result_prefix,
            )?;
            validate_strategy_artifact_ref(
                "strategy.experiment_result_uri",
                "strategy.experiment_result_hash",
                strategy.experiment_result_uri.as_deref(),
                strategy.experiment_result_hash.as_deref(),
                &experiment_result_prefix,
            )
        }
    }
}

fn reject_strategy_source_field<T>(
    value: Option<&T>,
    source_kind: StrategySourceKind,
    field: &'static str,
) -> Result<(), ManifestError> {
    if value.is_some() {
        Err(ManifestError::StrategySourceFieldNotAllowed { source_kind, field })
    } else {
        Ok(())
    }
}

fn validate_strategy_artifact_ref(
    uri_field: &'static str,
    hash_field: &'static str,
    uri: Option<&str>,
    hash: Option<&str>,
    expected_prefix: &str,
) -> Result<(), ManifestError> {
    let uri = required_strategy_source_field(uri_field, uri)?;
    let hash = required_strategy_source_field(hash_field, hash)?;
    if !uri.starts_with(expected_prefix) {
        return Err(ManifestError::StrategySourceOutsideAllowedArtifactRoot {
            field: uri_field,
            uri: uri.to_string(),
            expected_prefix: expected_prefix.to_string(),
        });
    }
    validate_strategy_source_hash(hash_field, hash)
}

fn required_strategy_source_field<'a>(
    field: &'static str,
    value: Option<&'a str>,
) -> Result<&'a str, ManifestError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(ManifestError::MissingField(field))
}

fn validate_strategy_source_hash(field: &'static str, value: &str) -> Result<(), ManifestError> {
    let is_sha256 = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if is_sha256 {
        Ok(())
    } else {
        Err(ManifestError::InvalidStrategySourceHash {
            field,
            value: value.to_string(),
        })
    }
}

fn validate_catalog_projection_relative_path(value: &str) -> Result<(), ManifestError> {
    let valid = !value.is_empty()
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains('\\')
        && !value.bytes().any(|byte| byte.is_ascii_control())
        && value.ends_with(".parquet")
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..");
    if valid {
        Ok(())
    } else {
        Err(ManifestError::InvalidCatalogProjectionManifest {
            field: "catalog_run_view_authority.roots.physical_manifest.objects.relative_path",
            message: format!(
                "{value:?} must be a normalized slash-separated relative Parquet path without traversal"
            ),
        })
    }
}

fn research_analytics_experiment_result_prefix(artifact_root: &str) -> String {
    format!(
        "{}/{}/",
        artifact_root.trim_end_matches('/'),
        RESEARCH_ANALYTICS_EXPERIMENT_RESULT_PREFIX
    )
}

fn validate_starting_balances(balances: &[String]) -> Result<(), ManifestError> {
    if balances.is_empty() {
        return Err(ManifestError::MissingField("venue.starting_balances"));
    }
    for balance in balances {
        Money::from_str(balance).map_err(|_| ManifestError::InvalidStartingBalance {
            balance: balance.clone(),
        })?;
    }
    Ok(())
}

fn manifest_venue_to_nt_config(
    venue: &ManifestVenueConfig,
) -> Result<BacktestVenueConfig, ManifestError> {
    ensure_unsupported_nt_venue_surfaces_absent(venue)?;
    BacktestVenueConfig::builder()
        .name(Ustr::from(&venue.nt_venue))
        .oms_type(parse_oms_type(&venue.oms_type)?)
        .account_type(parse_account_type(&venue.account_type)?)
        .book_type(parse_book_type(&venue.book_type)?)
        .starting_balances(venue.starting_balances.clone())
        .routing(venue.routing)
        .frozen_account(venue.frozen_account)
        .reject_stop_orders(venue.reject_stop_orders)
        .support_gtd_orders(venue.support_gtd_orders)
        .support_contingent_orders(venue.support_contingent_orders)
        .use_position_ids(venue.use_position_ids)
        .use_random_ids(venue.use_random_ids)
        .use_reduce_only(venue.use_reduce_only)
        .bar_execution(venue.bar_execution)
        .bar_adaptive_high_low_ordering(venue.bar_adaptive_high_low_ordering)
        .trade_execution(venue.trade_execution)
        .use_market_order_acks(venue.use_market_order_acks)
        .liquidity_consumption(venue.liquidity_consumption)
        .allow_cash_borrowing(venue.allow_cash_borrowing)
        .queue_position(venue.queue_position)
        .oto_trigger_mode(parse_oto_trigger_mode(&venue.oto_trigger_mode)?)
        .maybe_base_currency(parse_base_currency(&venue.base_currency)?)
        .default_leverage(parse_default_leverage(&venue.default_leverage)?)
        .maybe_fill_model(resolve_fill_model(venue.fill_model.as_ref())?)
        .maybe_latency_model(resolve_latency_model(venue.latency_model.as_ref())?)
        .maybe_fee_model(resolve_fee_model(venue.fee_model.as_ref())?)
        .price_protection_points(venue.price_protection_points)
        .build()
        .map_err(|error| ManifestError::InvalidNtConfig {
            field: "venue",
            message: error.to_string(),
        })
}

impl BacktestingRunManifest {
    /// Deterministic SHA-256 over every typed manifest field that affects a run.
    #[must_use]
    pub fn manifest_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"backtesting-run-manifest.v2");
        hasher.update(
            serde_json::to_vec(self)
                .expect("BacktestingRunManifest JSON serialization must be infallible"),
        );
        hex::encode(hasher.finalize())
    }

    /// Build the artifact-local manifest that records submitted run intent plus
    /// resolved NT/default surface values.
    ///
    /// # Errors
    ///
    /// Returns an error if the submitted manifest cannot map to NT config.
    pub fn to_artifact_manifest(&self) -> Result<BacktestRunManifestArtifact, ManifestError> {
        Ok(BacktestRunManifestArtifact {
            manifest_version: BACKTEST_RUN_MANIFEST_ARTIFACT_VERSION.to_string(),
            submitted_manifest_hash: self.manifest_hash(),
            currentness_rule_slots: manifest_currentness_rule_slots(),
            manifest: self.clone(),
            resolved_nt_surfaces: self.resolved_nt_surfaces()?,
        })
    }

    /// Resolve the NT/custom extension-surface records for this manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot map to NT config.
    pub fn resolved_nt_surfaces(&self) -> Result<Vec<ResolvedNtSurface>, ManifestError> {
        let run_config = self.to_nt_run_config()?;
        let engine = run_config.engine();
        let venue = run_config
            .venues()
            .first()
            .expect("BacktestingRunManifest always builds one BacktestVenueConfig");
        let mut surfaces = vec![
            resolved_surface(
                "manifest.schema_version",
                NtSurfaceClassification::CustomOwned,
                "BacktestingRunManifest.manifest_schema_version",
                self.manifest_schema_version.clone(),
            ),
            resolved_surface(
                "target.bolt_v2_branch",
                NtSurfaceClassification::CustomOwned,
                "BacktestingRunManifest.target_bolt_v2_branch",
                self.target_bolt_v2_branch.clone(),
            ),
            resolved_surface(
                "target.bolt_v2_ref",
                NtSurfaceClassification::CustomOwned,
                "BacktestingRunManifest.target_bolt_v2_ref",
                self.target_bolt_v2_ref.clone(),
            ),
            resolved_surface(
                "manifest.resolved_nt_version",
                NtSurfaceClassification::CustomOwned,
                "BacktestingRunManifest.resolved_nt_version",
                self.resolved_nt_version.clone(),
            ),
            resolved_surface(
                "strategy.config_hash",
                NtSurfaceClassification::CustomOwned,
                "BacktestingRunManifest.strategy_config_hash",
                self.strategy_config_hash.clone(),
            ),
            resolved_surface(
                "execution.model",
                NtSurfaceClassification::CustomOwned,
                "BacktestingRunManifest.execution_model",
                self.execution_model.clone(),
            ),
            resolved_surface(
                "engine.config",
                NtSurfaceClassification::Defaulted,
                "BacktestRunConfig.engine",
                format!(
                    "BacktestEngineConfig::default(run_analysis={},bypass_logging={})",
                    engine.run_analysis, engine.bypass_logging
                ),
            ),
            resolved_surface(
                "run.chunk_size",
                NtSurfaceClassification::PassThrough,
                "BacktestingRunManifest.nt_streaming_chunk_size",
                option_value(run_config.chunk_size()),
            ),
            resolved_surface(
                "run.raise_exception",
                NtSurfaceClassification::Defaulted,
                "BacktestRunConfig.raise_exception",
                run_config.raise_exception().to_string(),
            ),
            resolved_surface(
                "run.dispose_on_completion",
                NtSurfaceClassification::Defaulted,
                "BacktestRunConfig.dispose_on_completion",
                run_config.dispose_on_completion().to_string(),
            ),
            resolved_surface(
                "run.id",
                NtSurfaceClassification::PassThrough,
                "BacktestRunConfig.id",
                run_config.id(),
            ),
            resolved_surface(
                "run.start",
                NtSurfaceClassification::PassThrough,
                "BacktestRunConfig.start",
                option_value(run_config.start()),
            ),
            resolved_surface(
                "run.end",
                NtSurfaceClassification::PassThrough,
                "BacktestRunConfig.end",
                option_value(run_config.end()),
            ),
            resolved_surface(
                "venue.name",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.name",
                venue.name().to_string(),
            ),
            resolved_surface(
                "venue.oms_type",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.oms_type",
                format!("{:?}", venue.oms_type()),
            ),
            resolved_surface(
                "venue.account_type",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.account_type",
                format!("{:?}", venue.account_type()),
            ),
            resolved_surface(
                "venue.book_type",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.book_type",
                format!("{:?}", venue.book_type()),
            ),
            resolved_surface(
                "venue.starting_balances",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.starting_balances",
                format!("{:?}", venue.starting_balances()),
            ),
            resolved_surface(
                "venue.routing",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.routing",
                venue.routing().to_string(),
            ),
            resolved_surface(
                "venue.frozen_account",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.frozen_account",
                venue.frozen_account().to_string(),
            ),
            resolved_surface(
                "venue.reject_stop_orders",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.reject_stop_orders",
                venue.reject_stop_orders().to_string(),
            ),
            resolved_surface(
                "venue.support_gtd_orders",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.support_gtd_orders",
                venue.support_gtd_orders().to_string(),
            ),
            resolved_surface(
                "venue.support_contingent_orders",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.support_contingent_orders",
                venue.support_contingent_orders().to_string(),
            ),
            resolved_surface(
                "venue.use_position_ids",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_position_ids",
                venue.use_position_ids().to_string(),
            ),
            resolved_surface(
                "venue.use_random_ids",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_random_ids",
                venue.use_random_ids().to_string(),
            ),
            resolved_surface(
                "venue.use_reduce_only",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_reduce_only",
                venue.use_reduce_only().to_string(),
            ),
            resolved_surface(
                "venue.bar_execution",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.bar_execution",
                venue.bar_execution().to_string(),
            ),
            resolved_surface(
                "venue.bar_adaptive_high_low_ordering",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.bar_adaptive_high_low_ordering",
                venue.bar_adaptive_high_low_ordering().to_string(),
            ),
            resolved_surface(
                "venue.trade_execution",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.trade_execution",
                venue.trade_execution().to_string(),
            ),
            resolved_surface(
                "venue.use_market_order_acks",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_market_order_acks",
                venue.use_market_order_acks().to_string(),
            ),
            resolved_surface(
                "venue.liquidity_consumption",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.liquidity_consumption",
                venue.liquidity_consumption().to_string(),
            ),
            resolved_surface(
                "venue.allow_cash_borrowing",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.allow_cash_borrowing",
                venue.allow_cash_borrowing().to_string(),
            ),
            resolved_surface(
                "venue.queue_position",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.queue_position",
                venue.queue_position().to_string(),
            ),
            resolved_surface(
                "venue.oto_trigger_mode",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.oto_trigger_mode",
                format!("{:?}", venue.oto_trigger_mode()),
            ),
            resolved_surface(
                "venue.base_currency",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.base_currency",
                option_value(venue.base_currency()),
            ),
            resolved_surface(
                "venue.default_leverage",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.default_leverage",
                venue.default_leverage().to_string(),
            ),
            resolved_surface(
                "venue.price_protection_points",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.price_protection_points",
                venue.price_protection_points().to_string(),
            ),
            resolved_surface(
                "venue.fill_model",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.fill_model",
                option_value(venue.fill_model()),
            ),
            resolved_surface(
                "venue.latency_model",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.latency_model",
                option_value(venue.latency_model()),
            ),
            resolved_surface(
                "venue.fee_model",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.fee_model",
                option_value(venue.fee_model()),
            ),
        ];
        for (index, metric) in self.domain_metrics.iter().enumerate() {
            surfaces.push(resolved_surface(
                &format!("domain_metrics[{index}].kind"),
                NtSurfaceClassification::CustomOwned,
                "PortfolioAnalyzer::register_statistic",
                metric.kind.clone(),
            ));
        }
        let data_configs = run_config.data();
        for (index, data) in data_configs.iter().enumerate() {
            let prefix = if data_configs.len() == 1 {
                "catalog".to_string()
            } else {
                format!("catalog_inputs[{index}]")
            };
            surfaces.extend([
                resolved_surface(
                    &format!("{prefix}.data_type"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.data_type",
                    format!("{:?}", data.data_type()),
                ),
                resolved_surface(
                    &format!("{prefix}.catalog_path"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.catalog_path",
                    data.catalog_path(),
                ),
                resolved_surface(
                    &format!("{prefix}.catalog_fs_protocol"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.catalog_fs_protocol",
                    option_value(data.catalog_fs_protocol()),
                ),
                resolved_surface(
                    &format!("{prefix}.catalog_fs_storage_options"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.catalog_fs_storage_options",
                    storage_option_keys_value(data.catalog_fs_storage_options()),
                ),
                resolved_surface(
                    &format!("{prefix}.catalog_fs_rust_storage_options"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.catalog_fs_rust_storage_options",
                    storage_option_keys_value(data.catalog_fs_rust_storage_options()),
                ),
                resolved_surface(
                    &format!("{prefix}.instrument_id"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.instrument_id",
                    option_value(data.instrument_id()),
                ),
                resolved_surface(
                    &format!("{prefix}.start_time"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.start_time",
                    option_value(data.start_time()),
                ),
                resolved_surface(
                    &format!("{prefix}.end_time"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.end_time",
                    option_value(data.end_time()),
                ),
                resolved_surface(
                    &format!("{prefix}.filter_expr"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.filter_expr",
                    option_value(data.filter_expr()),
                ),
                resolved_surface(
                    &format!("{prefix}.client_id"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.client_id",
                    option_value(data.client_id()),
                ),
                resolved_surface(
                    &format!("{prefix}.optimize_file_loading"),
                    NtSurfaceClassification::PassThrough,
                    "BacktestDataConfig.optimize_file_loading",
                    data.optimize_file_loading().to_string(),
                ),
            ]);
        }
        surfaces.extend(UNSUPPORTED_NT_VENUE_SURFACES.iter().map(|surface| {
            resolved_surface(
                &format!("venue.{surface}"),
                NtSurfaceClassification::UnsupportedForNow,
                &format!("BacktestVenueConfig.{surface}"),
                "requests_rejected_before_nt_config",
            )
        }));
        let bound_bar_specs = self
            .catalog_inputs
            .iter()
            .filter_map(|input| input.bar_spec.as_deref())
            .collect::<Vec<_>>();
        surfaces.extend(UNSUPPORTED_NT_CATALOG_QUERY_SURFACES.iter().map(
            |(surface, _, nt_field)| {
                // `catalog.bar_spec` is dual-classified: absent it remains the
                // rejected-before-NT-config surface (existing manifests stay
                // byte-identical); present it is the bolt-owned operator
                // catalog-binding surface that never reaches the NT config.
                if *surface == "catalog.bar_spec" && !bound_bar_specs.is_empty() {
                    resolved_surface(
                        surface,
                        NtSurfaceClassification::CustomOwned,
                        nt_field,
                        format!("operator_catalog_binding:{}", bound_bar_specs.join(",")),
                    )
                } else {
                    resolved_surface(
                        surface,
                        NtSurfaceClassification::UnsupportedForNow,
                        nt_field,
                        "requests_rejected_before_nt_config",
                    )
                }
            },
        ));
        Ok(surfaces)
    }

    /// Validate the manifest against gate-4 rules and bind it to an accepted
    /// dataset (the only admissible data source).
    ///
    /// # Errors
    ///
    /// Returns the first blocking [`ManifestError`].
    pub fn validate(&self, accepted: &AcceptedDataset) -> Result<(), ManifestError> {
        for (name, value) in [
            (
                "manifest_schema_version",
                self.manifest_schema_version.as_str(),
            ),
            ("run_id", self.run_id.as_str()),
            ("target_bolt_v2_branch", self.target_bolt_v2_branch.as_str()),
            ("target_bolt_v2_ref", self.target_bolt_v2_ref.as_str()),
            ("resolved_nt_version", self.resolved_nt_version.as_str()),
            ("venue_binding_key", self.venue_binding_key.as_str()),
            ("source_proof_id", self.source_proof_id.as_str()),
            ("strategy_config_hash", self.strategy_config_hash.as_str()),
            ("execution_model", self.execution_model.as_str()),
            ("artifact_root", self.artifact_root.as_str()),
            ("output_prefix", self.output_prefix.as_str()),
            ("venue.nt_venue", self.venue.nt_venue.as_str()),
            ("venue.oms_type", self.venue.oms_type.as_str()),
            ("venue.account_type", self.venue.account_type.as_str()),
            ("venue.book_type", self.venue.book_type.as_str()),
            (
                "venue.oto_trigger_mode",
                self.venue.oto_trigger_mode.as_str(),
            ),
            ("venue.base_currency", self.venue.base_currency.as_str()),
            (
                "venue.default_leverage",
                self.venue.default_leverage.as_str(),
            ),
        ] {
            if value.trim().is_empty() {
                return Err(ManifestError::MissingField(name));
            }
        }
        if self.manifest_schema_version != BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedEnum {
                field: "manifest_schema_version",
                value: self.manifest_schema_version.clone(),
            });
        }
        validate_strategy_source_hash("strategy_config_hash", &self.strategy_config_hash)?;
        if self.nt_streaming_chunk_size == 0
            || usize::try_from(self.nt_streaming_chunk_size).is_err()
        {
            return Err(ManifestError::InvalidNtConfig {
                field: "nt_streaming_chunk_size",
                message: format!(
                    "must be positive and fit usize, got {}",
                    self.nt_streaming_chunk_size
                ),
            });
        }
        if self.catalog_inputs.is_empty() {
            return Err(ManifestError::MissingField("catalog_inputs"));
        }
        if self.catalog_inputs.len() != 1 {
            return Err(ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                message: format!(
                    "pinned NautilusTrader streaming materializes all data for {} inputs; exactly one catalog input is required",
                    self.catalog_inputs.len()
                ),
            });
        }
        for venue in &self.additional_venues {
            for (name, value) in [
                ("additional_venues.nt_venue", venue.nt_venue.as_str()),
                ("additional_venues.oms_type", venue.oms_type.as_str()),
                (
                    "additional_venues.account_type",
                    venue.account_type.as_str(),
                ),
                ("additional_venues.book_type", venue.book_type.as_str()),
                (
                    "additional_venues.oto_trigger_mode",
                    venue.oto_trigger_mode.as_str(),
                ),
                (
                    "additional_venues.base_currency",
                    venue.base_currency.as_str(),
                ),
                (
                    "additional_venues.default_leverage",
                    venue.default_leverage.as_str(),
                ),
            ] {
                if value.trim().is_empty() {
                    return Err(ManifestError::MissingField(name));
                }
            }
        }
        for input in &self.catalog_inputs {
            for (name, value) in [
                ("catalog_inputs.catalog_path", input.catalog_path.as_str()),
                (
                    "catalog_inputs.catalog_fs_protocol",
                    input.catalog_fs_protocol.as_str(),
                ),
                (
                    "catalog_inputs.nt_instrument_id",
                    input.nt_instrument_id.as_str(),
                ),
            ] {
                if value.trim().is_empty() {
                    return Err(ManifestError::MissingField(name));
                }
            }
        }
        for input in &self.reconstructed_reference_current_price {
            for (name, value) in [
                (
                    "reconstructed_reference_current_price.client_id",
                    input.client_id.as_str(),
                ),
                (
                    "reconstructed_reference_current_price.asset",
                    input.asset.as_str(),
                ),
                (
                    "reconstructed_reference_current_price.source_id",
                    input.source_id.as_str(),
                ),
                (
                    "reconstructed_reference_current_price.provider",
                    input.provider.as_str(),
                ),
                (
                    "reconstructed_reference_current_price.provider_instrument",
                    input.provider_instrument.as_str(),
                ),
                (
                    "reconstructed_reference_current_price.price",
                    input.price.as_str(),
                ),
            ] {
                if value.trim().is_empty() {
                    return Err(ManifestError::MissingField(name));
                }
            }
        }
        for input in &self.instrument_settlements {
            for (name, value) in [
                (
                    "instrument_settlements.nt_instrument_id",
                    input.nt_instrument_id.as_str(),
                ),
                (
                    "instrument_settlements.close_price",
                    input.close_price.as_str(),
                ),
                (
                    "instrument_settlements.settlement_currency",
                    input.settlement_currency.as_str(),
                ),
            ] {
                if value.trim().is_empty() {
                    return Err(ManifestError::MissingField(name));
                }
            }
        }
        ensure_supported_enums(self)?;
        ensure_supported_domain_metrics(self)?;
        ensure_unsupported_nt_venue_surfaces_absent(&self.venue)?;
        for venue in &self.additional_venues {
            ensure_unsupported_nt_venue_surfaces_absent(venue)?;
        }
        for input in &self.catalog_inputs {
            ensure_unsupported_nt_catalog_query_surfaces_absent(input)?;
            ensure_supported_data_type(&input.data_type)?;
            let catalog_fs_protocol = parse_catalog_fs_protocol(&input.catalog_fs_protocol)?;
            validate_catalog_storage_options(
                catalog_fs_protocol.as_deref(),
                &input.catalog_fs_storage_options,
                &input.catalog_fs_rust_storage_options,
            )?;
            // Admission boundary: operator-authored manifests must never carry raw
            // credentials; the runtime-resolved published-catalog manifest bypasses
            // validate() (to_nt_run_config only), so SSM-resolved options stay legal.
            reject_raw_credential_options(
                "catalog_inputs.catalog_fs_storage_options",
                &input.catalog_fs_storage_options,
            )?;
            reject_raw_credential_options(
                "catalog_inputs.catalog_fs_rust_storage_options",
                &input.catalog_fs_rust_storage_options,
            )?;
            parse_and_validate_catalog_input_instrument_ids(input)?;
        }
        validate_catalog_storage_options(
            output_prefix_protocol(&self.output_prefix),
            &self.artifact_store.storage_options,
            &self.artifact_store.rust_storage_options,
        )?;
        validate_artifact_store_secrets(&self.artifact_store)?;
        ensure_artifact_store_conditional_put_enabled(
            output_prefix_protocol(&self.output_prefix),
            &self.artifact_store.storage_options,
            &self.artifact_store.rust_storage_options,
        )?;
        validate_artifact_root_protocol(&self.artifact_root)?;
        ensure_catalog_inputs_match_fidelity(&self.catalog_inputs, accepted.fidelity_class)?;
        ensure_order_book_delta_inputs_require_l2_mbp(&self.catalog_inputs, &self.venue.book_type)?;
        validate_strategy_source(&self.strategy, &self.artifact_root)?;
        validate_starting_balances(&self.venue.starting_balances)?;
        for venue in &self.additional_venues {
            validate_starting_balances(&venue.starting_balances)?;
        }

        // Data must be accepted: the only admissible input is the accepted
        // dataset, matched by source proof id and version.
        if self.source_proof_id != accepted.source_proof_id
            || self.source_proof_version != accepted.source_proof_version
        {
            return Err(ManifestError::UnacceptedData {
                manifest_proof: format!("{}@{}", self.source_proof_id, self.source_proof_version),
                accepted_proof: format!(
                    "{}@{}",
                    accepted.source_proof_id, accepted.source_proof_version
                ),
            });
        }
        if self.venue_binding_key != accepted.source_binding {
            return Err(ManifestError::BindingMismatch {
                manifest_binding: self.venue_binding_key.clone(),
                accepted_binding: accepted.source_binding.clone(),
            });
        }
        if !market_structure_fixture_matches_source_fixture(
            self.market_structure_fixture,
            accepted.fixture_type,
        ) {
            return Err(ManifestError::FixtureMismatch {
                manifest_fixture: self.market_structure_fixture,
                accepted_fixture: accepted.fixture_type,
            });
        }
        if self.run_purpose == RunPurpose::Normal && self.pins_non_latest_proof {
            return Err(ManifestError::NonLatestProofPinForNormalRun);
        }
        if self.pins_non_latest_proof && self.proof_pin_reason_code.is_none() {
            return Err(ManifestError::MissingField("proof_pin_reason_code"));
        }
        if self.proof_pin_reason_code == Some(ProofPinReasonCode::AuditOrInvestigation)
            && self
                .proof_pin_reason_detail
                .as_deref()
                .is_none_or(|detail| detail.trim().is_empty())
        {
            return Err(ManifestError::MissingField("proof_pin_reason_detail"));
        }
        for (field, value) in [("start_time", self.start_time), ("end_time", self.end_time)] {
            if let Some(value) = value
                && value < 0
            {
                return Err(ManifestError::NegativeTime { field, value });
            }
        }
        if let (Some(start), Some(end)) = (self.start_time, self.end_time)
            && start > end
        {
            return Err(ManifestError::InvertedTimeWindow { start, end });
        }
        if !self.output_prefix.starts_with(&format!(
            "{}/backtests/",
            self.artifact_root.trim_end_matches('/')
        )) {
            return Err(ManifestError::OutputPrefixOutsideArtifactRoot);
        }
        Ok(())
    }

    /// Resolve a typed artifact subpath under the configured artifact root.
    ///
    /// # Errors
    ///
    /// Returns an error when `artifact_root` uses an unsupported scheme.
    pub fn artifact_subpath_uri(&self, subpath: ArtifactSubpath) -> Result<String, ManifestError> {
        validate_artifact_root_protocol(&self.artifact_root)?;
        Ok(format!(
            "{}/{}",
            self.artifact_root.trim_end_matches('/'),
            subpath.as_str()
        ))
    }

    /// Map the venue settings into a NautilusTrader [`BacktestVenueConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if an enum value is unsupported.
    pub fn to_nt_venue_config(&self) -> Result<BacktestVenueConfig, ManifestError> {
        manifest_venue_to_nt_config(&self.venue)
    }

    /// Map all catalog inputs into NautilusTrader [`BacktestDataConfig`]s.
    ///
    /// # Errors
    ///
    /// Returns an error if the data type or instrument id is unsupported.
    pub fn to_nt_data_configs(&self) -> Result<Vec<BacktestDataConfig>, ManifestError> {
        if self.catalog_inputs.is_empty() {
            return Err(ManifestError::MissingField("catalog_inputs"));
        }
        self.catalog_inputs
            .iter()
            .map(catalog_input_to_nt_data_config)
            .collect()
    }

    /// Map a single-input manifest into NautilusTrader [`BacktestDataConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if this manifest contains zero or multiple catalog inputs.
    pub fn to_nt_data_config(&self) -> Result<BacktestDataConfig, ManifestError> {
        let [input] = self.catalog_inputs.as_slice() else {
            return Err(ManifestError::UnsupportedEnum {
                field: "catalog_inputs",
                value: format!(
                    "expected exactly one catalog input, got {}",
                    self.catalog_inputs.len()
                ),
            });
        };
        catalog_input_to_nt_data_config(input)
    }

    /// Return the only catalog input for code paths that intentionally support a
    /// single data family.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest contains zero or multiple inputs.
    pub fn single_catalog_input(&self) -> Result<&ManifestCatalogInput, ManifestError> {
        let [input] = self.catalog_inputs.as_slice() else {
            return Err(ManifestError::UnsupportedEnum {
                field: "catalog_inputs",
                value: format!(
                    "expected exactly one catalog input, got {}",
                    self.catalog_inputs.len()
                ),
            });
        };
        Ok(input)
    }

    /// Return the only catalog input mutably for code paths that intentionally
    /// support a single data family.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest contains zero or multiple inputs.
    pub fn single_catalog_input_mut(&mut self) -> Result<&mut ManifestCatalogInput, ManifestError> {
        let len = self.catalog_inputs.len();
        let [input] = self.catalog_inputs.as_mut_slice() else {
            return Err(ManifestError::UnsupportedEnum {
                field: "catalog_inputs",
                value: format!("expected exactly one catalog input, got {len}"),
            });
        };
        Ok(input)
    }

    /// Return the primary catalog input for strategy/instrument configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when no catalog input is configured.
    pub fn primary_catalog_input(&self) -> Result<&ManifestCatalogInput, ManifestError> {
        self.catalog_inputs
            .first()
            .ok_or(ManifestError::MissingField("catalog_inputs"))
    }

    /// Return the primary catalog input mutably.
    ///
    /// # Errors
    ///
    /// Returns an error when no catalog input is configured.
    pub fn primary_catalog_input_mut(
        &mut self,
    ) -> Result<&mut ManifestCatalogInput, ManifestError> {
        self.catalog_inputs
            .first_mut()
            .ok_or(ManifestError::MissingField("catalog_inputs"))
    }

    /// Map the manifest into a NautilusTrader [`BacktestRunConfig`].
    ///
    /// # Errors
    ///
    /// Returns an error if venue or data mapping fails.
    pub fn to_nt_run_config(&self) -> Result<BacktestRunConfig, ManifestError> {
        if self.catalog_inputs.len() != 1 {
            return Err(ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                message: format!(
                    "pinned NautilusTrader streaming materializes all data for {} inputs; exactly one catalog input is required",
                    self.catalog_inputs.len()
                ),
            });
        }
        let chunk_size = usize::try_from(self.nt_streaming_chunk_size).map_err(|_| {
            ManifestError::InvalidNtConfig {
                field: "nt_streaming_chunk_size",
                message: format!(
                    "must be positive and fit usize, got {}",
                    self.nt_streaming_chunk_size
                ),
            }
        })?;
        if chunk_size == 0 {
            return Err(ManifestError::InvalidNtConfig {
                field: "nt_streaming_chunk_size",
                message: "must be positive, got 0".to_string(),
            });
        }
        let mut venues = vec![self.to_nt_venue_config()?];
        for venue in &self.additional_venues {
            venues.push(manifest_venue_to_nt_config(venue)?);
        }
        let data = self.to_nt_data_configs()?;
        let start = self
            .start_time
            .map(|value| manifest_time_to_nanos("start_time", value))
            .transpose()?;
        let end = self
            .end_time
            .map(|value| manifest_time_to_nanos("end_time", value))
            .transpose()?;
        BacktestRunConfig::builder()
            .id(self.run_id.clone())
            .venues(venues)
            .data(data)
            .chunk_size(chunk_size)
            .maybe_start(start)
            .maybe_end(end)
            // Retain post-run engine state (orders, positions, account) so the
            // runner can build the result contract and the order-terminal proof
            // from the cache after `BacktestNode::run`. With the NautilusTrader
            // default (`true`), `run` disposes the engine and wipes the cache,
            // leaving no order terminal states to inspect. The `BacktestResult`
            // summary is computed before this branch either way, so the only
            // effect is that `clear_data` (free the data stream) runs instead of
            // `dispose` (free all state).
            .dispose_on_completion(false)
            .build()
            .map_err(|error| ManifestError::InvalidNtConfig {
                field: "run",
                message: error.to_string(),
            })
    }
}

fn catalog_input_to_nt_data_config(
    input: &ManifestCatalogInput,
) -> Result<BacktestDataConfig, ManifestError> {
    ensure_unsupported_nt_catalog_query_surfaces_absent(input)?;
    let start_time = input
        .start_time
        .map(|value| manifest_time_to_nanos("catalog_inputs.start_time", value))
        .transpose()?;
    let end_time = input
        .end_time
        .map(|value| manifest_time_to_nanos("catalog_inputs.end_time", value))
        .transpose()?;
    let data_type = parse_data_type_str(input.data_type.as_str())?;
    let (nt_instrument_id, instrument_ids) =
        parse_and_validate_catalog_input_instrument_ids(input)?;
    let instrument_id = if instrument_ids.is_some() {
        None
    } else {
        Some(nt_instrument_id)
    };
    let catalog_fs_protocol = parse_catalog_fs_protocol(&input.catalog_fs_protocol)?;
    validate_catalog_storage_options(
        catalog_fs_protocol.as_deref(),
        &input.catalog_fs_storage_options,
        &input.catalog_fs_rust_storage_options,
    )?;
    BacktestDataConfig::builder()
        .data_type(data_type)
        .catalog_path(input.catalog_path.clone())
        .maybe_catalog_fs_protocol(catalog_fs_protocol)
        .maybe_catalog_fs_storage_options(if input.catalog_fs_storage_options.is_empty() {
            None
        } else {
            Some(
                input
                    .catalog_fs_storage_options
                    .clone()
                    .into_iter()
                    .collect(),
            )
        })
        .maybe_catalog_fs_rust_storage_options(
            if input.catalog_fs_rust_storage_options.is_empty() {
                None
            } else {
                Some(
                    input
                        .catalog_fs_rust_storage_options
                        .clone()
                        .into_iter()
                        .collect(),
                )
            },
        )
        .maybe_instrument_id(instrument_id)
        .maybe_instrument_ids(instrument_ids)
        .maybe_start_time(start_time)
        .maybe_end_time(end_time)
        .maybe_filter_expr(input.filter_expr.clone())
        .maybe_client_id(input.client_id.as_deref().map(ClientId::from))
        .maybe_optimize_file_loading(input.optimize_file_loading)
        .build()
        .map_err(|error| ManifestError::InvalidNtConfig {
            field: "data",
            message: error.to_string(),
        })
}

/// Parse and charset-validate every instrument-id surface of a catalog input.
///
/// Shared by [`BacktestingRunManifest::validate`] (the Gate 4 preflight) and
/// [`catalog_input_to_nt_data_config`] so the preflight rejects exactly the
/// ids the NT config build would reject: an id defect must fail before any
/// derived canonical or catalog artifact is produced, not at NT-config
/// construction mid-run.
fn parse_and_validate_catalog_input_instrument_ids(
    input: &ManifestCatalogInput,
) -> Result<(InstrumentId, Option<Vec<InstrumentId>>), ManifestError> {
    let nt_instrument_id = input
        .nt_instrument_id
        .parse::<InstrumentId>()
        .map_err(|_| ManifestError::InvalidInstrumentId {
            instrument_id: input.nt_instrument_id.clone(),
        })?;
    let instrument_ids = input
        .instrument_ids
        .as_ref()
        .map(|ids| {
            ids.iter()
                .map(|id| {
                    id.parse::<InstrumentId>()
                        .map_err(|_| ManifestError::InvalidInstrumentId {
                            instrument_id: id.clone(),
                        })
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    validate_catalog_instrument_id_charset(&input.nt_instrument_id)?;
    if let Some(ids) = input.instrument_ids.as_ref() {
        for id in ids {
            validate_catalog_instrument_id_charset(id)?;
        }
    }
    Ok((nt_instrument_id, instrument_ids))
}

/// Reject instrument ids whose catalog directory name would be altered by the
/// object-store percent-encoding layer in a way no query path can survive.
///
/// The catalog directory name is the urisafe form of the id ('/' stripped,
/// '^' mapped to '_'). NautilusTrader requires every data config to carry an
/// instrument or bar selector, so non-ASCII ids cannot use the former
/// unfiltered-query fallback and must fail closed. ASCII characters that
/// object_store's path layer percent-encodes at write time likewise fail loud
/// here instead of producing an empty data feed downstream. The safe set
/// (ASCII alphanumeric, '.', '_', '-') is a conservative strict subset of the
/// ASCII object_store stores verbatim;
/// its INVALID encode set covers controls plus backslash, braces, caret,
/// percent, backtick, brackets, quote, angle brackets, tilde, hash, pipe,
/// asterisk, and question mark — note '~' IS encoded, so it is rejected here
/// despite being RFC 3986 unreserved.
/// Everything else outside the safe set is rejected conservatively even
/// though object_store stores other ASCII punctuation verbatim — admission
/// must not depend on per-character encode-set knowledge: an over-strict
/// early failure is recoverable, an admitted-but-encoded id is a guaranteed
/// late NT node-load failure.
fn validate_catalog_instrument_id_charset(instrument_id: &str) -> Result<(), ManifestError> {
    let urisafe = instrument_id.replace('/', "").replace('^', "_");
    let unsupported = urisafe
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-'));
    if unsupported {
        return Err(ManifestError::UnsupportedInstrumentIdCharset {
            instrument_id: instrument_id.to_string(),
        });
    }
    Ok(())
}

impl BacktestingRunManifest {
    /// Return the single effective artifact-store option map for publication.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured artifact-store options conflict with
    /// the output prefix protocol or use an unsupported S3 option key.
    pub fn artifact_store_storage_options(
        &self,
    ) -> Result<Option<BTreeMap<String, String>>, ManifestError> {
        if self.artifact_store.ssm_parameters.is_some() {
            return Err(ManifestError::UnsupportedEnum {
                field: "artifact_store.ssm_parameters",
                value: "requires SSM resolver".to_string(),
            });
        }
        let options = self
            .artifact_store_base_storage_options()?
            .unwrap_or_default();
        ensure_artifact_store_s3_credentials_resolved(
            output_prefix_protocol(&self.output_prefix),
            &options,
        )?;
        if options.is_empty() {
            Ok(None)
        } else {
            Ok(Some(options))
        }
    }

    pub fn artifact_store_storage_options_resolved<F>(
        &self,
        resolver: &mut F,
    ) -> Result<Option<BTreeMap<String, String>>, ManifestError>
    where
        F: FnMut(&str, &str) -> Result<String, String>,
    {
        artifact_store_storage_options_for_uri(&self.output_prefix, &self.artifact_store, resolver)
    }

    fn artifact_store_base_storage_options(
        &self,
    ) -> Result<Option<BTreeMap<String, String>>, ManifestError> {
        artifact_store_base_storage_options_for_uri(&self.output_prefix, &self.artifact_store)
    }
}

fn artifact_store_base_storage_options_for_uri(
    output_uri: &str,
    artifact_store: &ManifestArtifactStore,
) -> Result<Option<BTreeMap<String, String>>, ManifestError> {
    validate_catalog_storage_options(
        output_prefix_protocol(output_uri),
        &artifact_store.storage_options,
        &artifact_store.rust_storage_options,
    )?;
    validate_artifact_store_secrets(artifact_store)?;
    ensure_artifact_store_conditional_put_enabled(
        output_prefix_protocol(output_uri),
        &artifact_store.storage_options,
        &artifact_store.rust_storage_options,
    )?;
    if !artifact_store.rust_storage_options.is_empty() {
        Ok(Some(artifact_store.rust_storage_options.clone()))
    } else if !artifact_store.storage_options.is_empty() {
        Ok(Some(artifact_store.storage_options.clone()))
    } else {
        Ok(None)
    }
}

pub fn artifact_store_storage_options_for_uri<F>(
    output_uri: &str,
    artifact_store: &ManifestArtifactStore,
    resolver: &mut F,
) -> Result<Option<BTreeMap<String, String>>, ManifestError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let mut options = artifact_store_base_storage_options_for_uri(output_uri, artifact_store)?
        .unwrap_or_default();
    if let Some(parameters) = &artifact_store.ssm_parameters {
        validate_artifact_store_ssm_parameters(parameters)?;
        options.insert(
            "access_key_id".to_string(),
            resolve_artifact_store_secret(
                resolver,
                &parameters.region,
                "access_key_id",
                &parameters.access_key_id,
            )?,
        );
        options.insert(
            "secret_access_key".to_string(),
            resolve_artifact_store_secret(
                resolver,
                &parameters.region,
                "secret_access_key",
                &parameters.secret_access_key,
            )?,
        );
        if let Some(session_token) = &parameters.session_token {
            options.insert(
                "session_token".to_string(),
                resolve_artifact_store_secret(
                    resolver,
                    &parameters.region,
                    "session_token",
                    session_token,
                )?,
            );
        }
    }
    ensure_artifact_store_s3_credentials_resolved(output_prefix_protocol(output_uri), &options)?;
    if options.is_empty() {
        Ok(None)
    } else {
        Ok(Some(options))
    }
}

fn ensure_supported_enums(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    for venue in manifest
        .additional_venues
        .iter()
        .chain(std::iter::once(&manifest.venue))
    {
        parse_oms_type(&venue.oms_type)?;
        parse_account_type(&venue.account_type)?;
        parse_book_type(&venue.book_type)?;
        parse_oto_trigger_mode(&venue.oto_trigger_mode)?;
        parse_base_currency(&venue.base_currency)?;
        parse_default_leverage(&venue.default_leverage)?;
        resolve_fill_model(venue.fill_model.as_ref())?;
        resolve_latency_model(venue.latency_model.as_ref())?;
        resolve_fee_model(venue.fee_model.as_ref())?;
    }
    // Primary execution venue cost-realism anchor: the loop above already
    // validates manifest.venue (it is chained in), but the RA cost-realism fence
    // pins this exact spelling because fills settle against the primary venue.
    // Keep it explicit so the guarantee stays statically visible.
    resolve_fill_model(manifest.venue.fill_model.as_ref())?;
    Ok(())
}

fn ensure_supported_domain_metrics(manifest: &BacktestingRunManifest) -> Result<(), ManifestError> {
    for metric in &manifest.domain_metrics {
        if metric.kind.trim().is_empty() {
            return Err(ManifestError::MissingField("domain_metrics.kind"));
        }
        if !registered_domain_metrics().contains(&metric.kind.as_str()) {
            return Err(ManifestError::UnsupportedEnum {
                field: "domain_metrics.kind",
                value: metric.kind.clone(),
            });
        }
    }
    Ok(())
}

fn ensure_unsupported_nt_venue_surfaces_absent(
    venue: &ManifestVenueConfig,
) -> Result<(), ManifestError> {
    for (field, present) in [
        (UNSUPPORTED_NT_VENUE_SURFACES[0], venue.leverages.is_some()),
        (
            UNSUPPORTED_NT_VENUE_SURFACES[1],
            venue.margin_model.is_some(),
        ),
        (UNSUPPORTED_NT_VENUE_SURFACES[2], venue.modules.is_some()),
        (
            UNSUPPORTED_NT_VENUE_SURFACES[3],
            venue.settlement_prices.is_some(),
        ),
    ] {
        if present {
            return Err(ManifestError::UnsupportedNtSurface { field });
        }
    }
    Ok(())
}

fn ensure_unsupported_nt_catalog_query_surfaces_absent(
    catalog: &ManifestCatalogInput,
) -> Result<(), ManifestError> {
    for (field, present) in [
        (
            UNSUPPORTED_NT_CATALOG_QUERY_SURFACES[0].1,
            catalog.metadata.is_some(),
        ),
        (
            UNSUPPORTED_NT_CATALOG_QUERY_SURFACES[2].1,
            catalog.bar_types.is_some(),
        ),
    ] {
        if present {
            return Err(ManifestError::UnsupportedNtSurface { field });
        }
    }
    // `bar_spec` is an operator catalog-binding surface, not an NT query
    // surface: the operator binds each declared bar input to the projected
    // per-table catalog subroot that holds exactly one externally-aggregated
    // bar type, so the NT data config never needs a bar filter. It is only
    // admissible on `Bar` inputs and must name a concrete specification.
    if let Some(bar_spec) = catalog.bar_spec.as_deref() {
        if bar_spec.trim().is_empty() {
            return Err(ManifestError::UnsupportedEnum {
                field: "catalog_inputs.bar_spec",
                value: "bar_spec must not be blank".to_string(),
            });
        }
        if catalog.data_type != "Bar" {
            return Err(ManifestError::UnsupportedEnum {
                field: "catalog_inputs.bar_spec",
                value: format!(
                    "bar_spec is only admissible on Bar inputs, got data_type {:?}",
                    catalog.data_type
                ),
            });
        }
    }
    Ok(())
}

/// Role a data type plays in a fidelity class's admittance set.
///
/// `Primary` means the type satisfies the fidelity class's mandatory-presence
/// requirement (a run under that class must carry at least one input of the
/// primary type). `Auxiliary` means the type is admissible alongside a primary
/// but does not by itself satisfy the fidelity class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmittanceRole {
    Primary,
    Auxiliary,
}

/// One admissible (NT data type, fidelity class) pairing and its role.
struct AdmittanceRow {
    data_type: NautilusDataType,
    fidelity: SourceProofFidelityClass,
    role: AdmittanceRole,
}

/// Single source of truth for bolt's catalog-input admittance.
///
/// Every admittance decision in this module (supported-type check, type-string
/// parsing, per-input fidelity match, and the per-fidelity must-have-primary
/// check) derives from this table. There is no parallel admittance logic: a
/// `(data_type, fidelity)` pairing is admissible iff a row lists it, and a type
/// is supported iff any row names it. NT data types are the keys; every
/// data-type string flows through `NautilusDataType` `Display`/`FromStr`, so
/// there are no data-type string literals outside test bodies.
///
/// The `QuoteTick`/`IndexPriceUpdate`/`MarkPriceUpdate` primary rows admit those
/// streams (each its own primary fidelity class, per the v3 tier map: Tier A
/// quotes/index_prices/mark_prices). Their canonical tables and canonical->NT
/// projections landed in S3 (`project_canonical_{quotes,index,mark}_to_catalog`
/// and `read_back_{quotes,index,mark}`). Snapshot-seeded L2 quote archives now
/// populate `QuoteTick` through the seeded-L2 quote adapter; the older flat
/// snapshot-quote, index-price, and mark-price raw normalizers still fail loud
/// until their source-specific acquisition slices land (tracked by bolt-v2
/// #836/#437). Auxiliary status/close pairing for the three new classes is
/// deliberately omitted here — each new class carries a single primary row.
const ADMITTANCE_TABLE: &[AdmittanceRow] = &[
    // TradeReplay: native trade prints are primary; status/close auxiliary.
    AdmittanceRow {
        data_type: NautilusDataType::TradeTick,
        fidelity: SourceProofFidelityClass::TradeReplay,
        role: AdmittanceRole::Primary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::InstrumentStatus,
        fidelity: SourceProofFidelityClass::TradeReplay,
        role: AdmittanceRole::Auxiliary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::InstrumentClose,
        fidelity: SourceProofFidelityClass::TradeReplay,
        role: AdmittanceRole::Auxiliary,
    },
    // L2Replay: order-book deltas primary; trades + status/close auxiliary.
    AdmittanceRow {
        data_type: NautilusDataType::OrderBookDelta,
        fidelity: SourceProofFidelityClass::L2Replay,
        role: AdmittanceRole::Primary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::TradeTick,
        fidelity: SourceProofFidelityClass::L2Replay,
        role: AdmittanceRole::Auxiliary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::InstrumentStatus,
        fidelity: SourceProofFidelityClass::L2Replay,
        role: AdmittanceRole::Auxiliary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::InstrumentClose,
        fidelity: SourceProofFidelityClass::L2Replay,
        role: AdmittanceRole::Auxiliary,
    },
    // TradeBarReplay: externally-aggregated bars primary; trades + status/close auxiliary.
    AdmittanceRow {
        data_type: NautilusDataType::Bar,
        fidelity: SourceProofFidelityClass::TradeBarReplay,
        role: AdmittanceRole::Primary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::TradeTick,
        fidelity: SourceProofFidelityClass::TradeBarReplay,
        role: AdmittanceRole::Auxiliary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::InstrumentStatus,
        fidelity: SourceProofFidelityClass::TradeBarReplay,
        role: AdmittanceRole::Auxiliary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::InstrumentClose,
        fidelity: SourceProofFidelityClass::TradeBarReplay,
        role: AdmittanceRole::Auxiliary,
    },
    // QuoteReplay/IndexReplay/MarkReplay/FundingReplay: gate-capable point
    // streams with one NT-native primary row each.
    AdmittanceRow {
        data_type: NautilusDataType::QuoteTick,
        fidelity: SourceProofFidelityClass::QuoteReplay,
        role: AdmittanceRole::Primary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::IndexPriceUpdate,
        fidelity: SourceProofFidelityClass::IndexReplay,
        role: AdmittanceRole::Primary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::MarkPriceUpdate,
        fidelity: SourceProofFidelityClass::MarkReplay,
        role: AdmittanceRole::Primary,
    },
    AdmittanceRow {
        data_type: NautilusDataType::FundingRateUpdate,
        fidelity: SourceProofFidelityClass::FundingReplay,
        role: AdmittanceRole::Primary,
    },
];

/// True if any admittance row names this data type (bolt-supported type filter).
fn supported_data_type(data_type: NautilusDataType) -> bool {
    ADMITTANCE_TABLE
        .iter()
        .any(|row| row.data_type == data_type)
}

/// True if a `(data_type, fidelity)` pairing has an admittance row.
fn data_type_admissible_under(
    data_type: NautilusDataType,
    fidelity: SourceProofFidelityClass,
) -> bool {
    ADMITTANCE_TABLE
        .iter()
        .any(|row| row.data_type == data_type && row.fidelity == fidelity)
}

/// The data type whose presence is mandatory for a fidelity class, if any.
///
/// Returns `None` for fidelity classes with no primary row (e.g. `SignalOnly`,
/// `MetadataOnly`, `SnapshotReplay`, `ForwardCapturePending`), which keeps them
/// unrunnable as catalog-input fidelity classes.
fn fidelity_primary_type(fidelity: SourceProofFidelityClass) -> Option<NautilusDataType> {
    ADMITTANCE_TABLE
        .iter()
        .find(|row| row.fidelity == fidelity && row.role == AdmittanceRole::Primary)
        .map(|row| row.data_type)
}

/// Parse a catalog-input data-type string into a bolt-admitted [`NautilusDataType`].
///
/// String boundary for the admittance table: `NautilusDataType::from_str` admits
/// the pinned NT variants, then the table filters to bolt-supported types. A type
/// NT knows but the table omits (e.g. `OrderBookDepth10`) and pure junk both
/// surface the same [`ManifestError::UnsupportedDataType`] with the original
/// string payload.
fn parse_data_type_str(value: &str) -> Result<NautilusDataType, ManifestError> {
    let data_type =
        NautilusDataType::from_str(value).map_err(|_| ManifestError::UnsupportedDataType {
            data_type: value.to_string(),
        })?;
    if supported_data_type(data_type) {
        Ok(data_type)
    } else {
        Err(ManifestError::UnsupportedDataType {
            data_type: value.to_string(),
        })
    }
}

fn ensure_supported_data_type(value: &str) -> Result<(), ManifestError> {
    parse_data_type_str(value).map(|_| ())
}

fn parse_catalog_fs_protocol(value: &str) -> Result<Option<String>, ManifestError> {
    match value {
        CATALOG_FS_PROTOCOL_NONE => Ok(None),
        "s3" | "gs" | "gcs" | "az" | "abfs" | "http" | "https" => Ok(Some(value.to_string())),
        other => Err(ManifestError::UnsupportedEnum {
            field: "catalog_inputs.catalog_fs_protocol",
            value: other.to_string(),
        }),
    }
}

fn output_prefix_protocol(output_prefix: &str) -> Option<&str> {
    output_prefix
        .split_once("://")
        .map(|(protocol, _)| protocol)
}

fn validate_artifact_root_protocol(artifact_root: &str) -> Result<(), ManifestError> {
    match output_prefix_protocol(artifact_root) {
        Some("s3" | "file") => Ok(()),
        Some(other) => Err(ManifestError::UnsupportedEnum {
            field: "artifact_root",
            value: other.to_string(),
        }),
        None => Err(ManifestError::UnsupportedEnum {
            field: "artifact_root",
            value: CATALOG_FS_PROTOCOL_NONE.to_string(),
        }),
    }
}

fn validate_catalog_storage_options(
    protocol: Option<&str>,
    storage_options: &BTreeMap<String, String>,
    rust_storage_options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if !storage_options.is_empty() && !rust_storage_options.is_empty() {
        return Err(ManifestError::UnsupportedEnum {
            field: "catalog_inputs.catalog_fs_storage_options",
            value: CATALOG_STORAGE_OPTIONS_SHADOWED.to_string(),
        });
    }
    if protocol.is_none() && (!storage_options.is_empty() || !rust_storage_options.is_empty()) {
        return Err(ManifestError::UnsupportedEnum {
            field: "catalog_inputs.catalog_fs_protocol",
            value: format!("{CATALOG_FS_PROTOCOL_NONE} cannot carry storage options"),
        });
    }
    if protocol == Some("s3") {
        for (key, value) in storage_options {
            ensure_supported_s3_storage_option(
                "catalog_inputs.catalog_fs_storage_options",
                key,
                value,
            )?;
        }
        for (key, value) in rust_storage_options {
            ensure_supported_s3_storage_option(
                "catalog_inputs.catalog_fs_rust_storage_options",
                key,
                value,
            )?;
        }
    }
    Ok(())
}

fn ensure_supported_s3_storage_option(
    field: &'static str,
    key: &str,
    value: &str,
) -> Result<(), ManifestError> {
    match key {
        "endpoint_url" | "region" | "access_key_id" | "key" | "secret_access_key" | "secret"
        | "session_token" | "token" | "allow_http" => Ok(()),
        S3_OPTION_CONDITIONAL_PUT => validate_s3_conditional_put_value(field, value),
        other => Err(ManifestError::UnsupportedEnum {
            field,
            value: other.to_string(),
        }),
    }
}

fn validate_s3_conditional_put_value(
    field: &'static str,
    value: &str,
) -> Result<(), ManifestError> {
    match value {
        S3_CONDITIONAL_PUT_ETAG | S3_CONDITIONAL_PUT_DISABLED => Ok(()),
        other => Err(ManifestError::UnsupportedEnum {
            field,
            value: format!("{S3_OPTION_CONDITIONAL_PUT}={other}"),
        }),
    }
}

fn ensure_artifact_store_conditional_put_enabled(
    protocol: Option<&str>,
    storage_options: &BTreeMap<String, String>,
    rust_storage_options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if protocol != Some("s3") {
        return Ok(());
    }
    for options in [storage_options, rust_storage_options] {
        if options.get(S3_OPTION_CONDITIONAL_PUT).map(String::as_str)
            == Some(S3_CONDITIONAL_PUT_DISABLED)
        {
            return Err(ManifestError::UnsupportedEnum {
                field: "artifact_store.rust_storage_options.conditional_put",
                value: "disabled cannot support Artifact Index create-only event writes or conditional latest-pointer updates".to_string(),
            });
        }
    }
    Ok(())
}

fn validate_artifact_store_secrets(
    artifact_store: &ManifestArtifactStore,
) -> Result<(), ManifestError> {
    reject_raw_credential_options(
        "artifact_store.storage_options",
        &artifact_store.storage_options,
    )?;
    reject_raw_credential_options(
        "artifact_store.rust_storage_options",
        &artifact_store.rust_storage_options,
    )?;
    if let Some(parameters) = &artifact_store.ssm_parameters {
        validate_artifact_store_ssm_parameters(parameters)?;
    }
    Ok(())
}

fn ensure_artifact_store_s3_credentials_resolved(
    protocol: Option<&str>,
    options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    if protocol != Some("s3") {
        return Ok(());
    }
    let has_access_key = options.contains_key("access_key_id") || options.contains_key("key");
    let has_secret_key =
        options.contains_key("secret_access_key") || options.contains_key("secret");
    if has_access_key && has_secret_key {
        Ok(())
    } else {
        Err(ManifestError::ArtifactStoreS3CredentialsNotResolved)
    }
}

fn reject_raw_credential_options(
    field: &'static str,
    options: &BTreeMap<String, String>,
) -> Result<(), ManifestError> {
    for key in options.keys() {
        if is_s3_credential_option(key) {
            return Err(ManifestError::RawCredentialOption {
                field,
                key: key.clone(),
            });
        }
    }
    Ok(())
}

fn is_s3_credential_option(key: &str) -> bool {
    matches!(
        key,
        "access_key_id" | "key" | "secret_access_key" | "secret" | "session_token" | "token"
    )
}

fn validate_artifact_store_ssm_parameters(
    parameters: &ManifestArtifactStoreSsmParameters,
) -> Result<(), ManifestError> {
    if parameters.region.trim().is_empty() {
        return Err(ManifestError::MissingField(
            "artifact_store.ssm_parameters.region",
        ));
    }
    for (field, value) in [
        ("access_key_id", parameters.access_key_id.as_str()),
        ("secret_access_key", parameters.secret_access_key.as_str()),
    ] {
        validate_artifact_store_ssm_parameter(field, value)?;
    }
    if let Some(session_token) = &parameters.session_token {
        validate_artifact_store_ssm_parameter("session_token", session_token)?;
    }
    Ok(())
}

fn validate_artifact_store_ssm_parameter(
    field: &'static str,
    value: &str,
) -> Result<(), ManifestError> {
    if !value.starts_with('/') || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(ManifestError::InvalidArtifactStoreSsmParameter { field });
    }
    Ok(())
}

fn resolve_artifact_store_secret<F>(
    resolver: &mut F,
    region: &str,
    field: &'static str,
    path: &str,
) -> Result<String, ManifestError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let value = resolver(region, path)
        .map_err(|source| ManifestError::ArtifactStoreSecretResolution { field, source })?;
    if value.trim().is_empty() || value.trim() != value {
        return Err(ManifestError::ArtifactStoreSecretResolution {
            field,
            source: "resolved value must be non-empty and must not contain leading or trailing whitespace"
                .to_string(),
        });
    }
    Ok(value)
}

/// The single per-input admittance predicate: a data-type string is admissible
/// under a fidelity class iff [`ADMITTANCE_TABLE`] holds a matching row.
///
/// The error preserves the caller-supplied string: for NT-known types the
/// `Display` round-trips to the same name, and for table-unsupported types
/// `parse_data_type_str` already emits [`ManifestError::UnsupportedDataType`]
/// (so a fidelity mismatch only fires for supported-but-wrong-class types).
fn ensure_data_type_matches_fidelity(
    data_type: &str,
    fidelity_class: SourceProofFidelityClass,
) -> Result<(), ManifestError> {
    let parsed = parse_data_type_str(data_type)?;
    if data_type_admissible_under(parsed, fidelity_class) {
        Ok(())
    } else {
        Err(ManifestError::DataTypeFidelityMismatch {
            data_type: data_type.to_string(),
            fidelity_class,
        })
    }
}

/// Fully table-driven catalog-input fidelity gate.
///
/// Derived entirely from [`ADMITTANCE_TABLE`] — no per-class arms:
/// 1. The fidelity class must have a primary type ([`fidelity_primary_type`]).
///    A class with no primary (e.g. `SignalOnly`, `MetadataOnly`,
///    `SnapshotReplay`, `ForwardCapturePending`) is unrunnable as a
///    catalog-input class — this replaces the former catch-all `other =>` arm.
/// 2. At least one input must carry that primary type (must-have-presence).
/// 3. Every input must be admissible under the class, via the single per-input
///    predicate [`ensure_data_type_matches_fidelity`].
///
/// The must-have-presence error preserves the original "first input or `<none>`"
/// string payload so existing assertions hold. The presence probe parses each
/// input leniently (an unparseable/unsupported type is simply "not the primary");
/// the precise [`ManifestError::UnsupportedDataType`] for such an input is then
/// surfaced by the per-input predicate in step 3.
fn ensure_catalog_inputs_match_fidelity(
    inputs: &[ManifestCatalogInput],
    fidelity_class: SourceProofFidelityClass,
) -> Result<(), ManifestError> {
    let primary_data_type = fidelity_primary_type(fidelity_class);
    let has_primary = primary_data_type.is_some_and(|primary| {
        inputs.iter().any(|input| {
            parse_data_type_str(&input.data_type)
                .map(|parsed| parsed == primary)
                .unwrap_or(false)
        })
    });
    if !has_primary {
        return Err(ManifestError::DataTypeFidelityMismatch {
            data_type: inputs
                .first()
                .map(|input| input.data_type.clone())
                .unwrap_or_else(|| "<none>".to_string()),
            fidelity_class,
        });
    }
    for input in inputs {
        ensure_data_type_matches_fidelity(&input.data_type, fidelity_class)?;
    }
    Ok(())
}

/// Fail-loud fence coupling order-book-delta inputs to an L2 book type.
///
/// bolt's converter emits L2 (MBP) order-book deltas flagged `F_LAST` only, with
/// no per-order (`F_MBP`) identity — every level change carries `order_id == 0`.
/// Under NT's `BookType::L3_MBO` (or any non-L2 book type) those `order_id == 0`
/// UPDATE/DELETE rows collapse onto a single phantom order, silently corrupting
/// the book with nothing failing loud at run time. The `(data_type, fidelity)`
/// admittance table never couples `book_type` to the delta fidelity, so this is
/// the single place that rejects the mismatch: when any catalog input parses to
/// [`NautilusDataType::OrderBookDelta`], `venue.book_type` must be `L2_MBP`.
///
/// `book_type` is parsed through [`parse_book_type`] (the single book-type parser)
/// so the only admitted value is the typed [`BookType::L2_MBP`]; no book-type
/// string literal is duplicated here.
fn ensure_order_book_delta_inputs_require_l2_mbp(
    inputs: &[ManifestCatalogInput],
    book_type: &str,
) -> Result<(), ManifestError> {
    let has_order_book_delta = inputs.iter().any(|input| {
        parse_data_type_str(&input.data_type)
            .map(|parsed| parsed == NautilusDataType::OrderBookDelta)
            .unwrap_or(false)
    });
    if !has_order_book_delta {
        return Ok(());
    }
    if parse_book_type(book_type)? == BookType::L2_MBP {
        Ok(())
    } else {
        Err(ManifestError::OrderBookDeltaRequiresL2Mbp {
            book_type: book_type.to_string(),
        })
    }
}

fn market_structure_fixture_matches_source_fixture(
    manifest_fixture: MarketStructureFixture,
    accepted_fixture: FixtureType,
) -> bool {
    matches!(
        (manifest_fixture, accepted_fixture),
        (
            MarketStructureFixture::BinaryOption,
            FixtureType::BinaryOption | FixtureType::PredictionMarket
        ) | (MarketStructureFixture::PerpsSpot, FixtureType::PerpsSpot)
    )
}

fn parse_oms_type(value: &str) -> Result<OmsType, ManifestError> {
    match value {
        "NETTING" => Ok(OmsType::Netting),
        "HEDGING" => Ok(OmsType::Hedging),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.oms_type",
            value: other.to_string(),
        }),
    }
}

fn parse_account_type(value: &str) -> Result<AccountType, ManifestError> {
    match value {
        "CASH" => Ok(AccountType::Cash),
        "MARGIN" => Ok(AccountType::Margin),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.account_type",
            value: other.to_string(),
        }),
    }
}

fn parse_book_type(value: &str) -> Result<BookType, ManifestError> {
    match value {
        "L1_MBP" => Ok(BookType::L1_MBP),
        "L2_MBP" => Ok(BookType::L2_MBP),
        "L3_MBO" => Ok(BookType::L3_MBO),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.book_type",
            value: other.to_string(),
        }),
    }
}

fn parse_oto_trigger_mode(value: &str) -> Result<OtoTriggerMode, ManifestError> {
    match value {
        "PARTIAL" => Ok(OtoTriggerMode::Partial),
        "FULL" => Ok(OtoTriggerMode::Full),
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.oto_trigger_mode",
            value: other.to_string(),
        }),
    }
}

fn parse_base_currency(value: &str) -> Result<Option<Currency>, ManifestError> {
    if value == "NONE" {
        return Ok(None);
    }
    Currency::from_str(value)
        .map(Some)
        .map_err(|_| ManifestError::InvalidBaseCurrency {
            currency: value.to_string(),
        })
}

fn parse_default_leverage(value: &str) -> Result<Decimal, ManifestError> {
    let leverage = Decimal::from_str(value).map_err(|_| ManifestError::InvalidDefaultLeverage {
        leverage: value.to_string(),
    })?;
    if leverage <= Decimal::ZERO {
        return Err(ManifestError::InvalidDefaultLeverage {
            leverage: value.to_string(),
        });
    }
    Ok(leverage)
}

fn parse_probability(value: &str, field: &'static str) -> Result<f64, ManifestError> {
    let decimal =
        Decimal::from_str(value).map_err(|_| ManifestError::InvalidVenueModelParameter {
            field,
            value: value.to_string(),
        })?;
    if !(Decimal::ZERO..=Decimal::ONE).contains(&decimal) {
        return Err(ManifestError::InvalidVenueModelParameter {
            field,
            value: value.to_string(),
        });
    }
    decimal
        .to_f64()
        .ok_or_else(|| ManifestError::InvalidVenueModelParameter {
            field,
            value: value.to_string(),
        })
}

fn ensure_latency_component_sum(
    base: u64,
    component: u64,
    field: &'static str,
) -> Result<(), ManifestError> {
    base.checked_add(component).map(|_| ()).ok_or_else(|| {
        ManifestError::InvalidVenueModelParameter {
            field,
            value: component.to_string(),
        }
    })
}

fn resolve_fill_model(
    config: Option<&ManifestFillModelConfig>,
) -> Result<Option<FillModelAny>, ManifestError> {
    let Some(config) = config else {
        return Ok(None);
    };
    match config.kind.as_str() {
        FILL_MODEL_PREDICTION_MARKET_PROBABILISTIC => {
            let prob_fill_on_limit = parse_probability(
                &config.prob_fill_on_limit,
                "venue.fill_model.prob_fill_on_limit",
            )?;
            let prob_slippage =
                parse_probability(&config.prob_slippage, "venue.fill_model.prob_slippage")?;
            let random_seed = config
                .random_seed
                .ok_or(ManifestError::MissingField("venue.fill_model.random_seed"))?;
            let model =
                ProbabilisticFillModel::new(prob_fill_on_limit, prob_slippage, Some(random_seed))
                    .map_err(|_| ManifestError::InvalidVenueModelParameter {
                    field: "venue.fill_model",
                    value: config.kind.clone(),
                })?;
            Ok(Some(FillModelAny::Probabilistic(model)))
        }
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.fill_model.kind",
            value: other.to_string(),
        }),
    }
}

fn resolve_latency_model(
    config: Option<&ManifestLatencyModelConfig>,
) -> Result<Option<LatencyModelAny>, ManifestError> {
    let Some(config) = config else {
        return Ok(None);
    };
    match config.kind.as_str() {
        LATENCY_MODEL_PREDICTION_MARKET_STATIC => {
            ensure_latency_component_sum(
                config.base_latency_nanos,
                config.insert_latency_nanos,
                "venue.latency_model.insert_latency_nanos",
            )?;
            ensure_latency_component_sum(
                config.base_latency_nanos,
                config.update_latency_nanos,
                "venue.latency_model.update_latency_nanos",
            )?;
            ensure_latency_component_sum(
                config.base_latency_nanos,
                config.delete_latency_nanos,
                "venue.latency_model.delete_latency_nanos",
            )?;
            Ok(Some(LatencyModelAny::Static(StaticLatencyModel::new(
                UnixNanos::from(config.base_latency_nanos),
                UnixNanos::from(config.insert_latency_nanos),
                UnixNanos::from(config.update_latency_nanos),
                UnixNanos::from(config.delete_latency_nanos),
            ))))
        }
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.latency_model.kind",
            value: other.to_string(),
        }),
    }
}

fn resolve_fee_model(
    config: Option<&ManifestFeeModelConfig>,
) -> Result<Option<FeeModelAny>, ManifestError> {
    let Some(config) = config else {
        return Ok(None);
    };
    match config.kind.as_str() {
        FEE_MODEL_PREDICTION_MARKET_MAKER_TAKER => {
            Ok(Some(FeeModelAny::MakerTaker(MakerTakerFeeModel)))
        }
        other => Err(ManifestError::UnsupportedEnum {
            field: "venue.fee_model.kind",
            value: other.to_string(),
        }),
    }
}

/// Build the typed manifest from TOML text.
///
/// # Errors
///
/// Returns an error if the TOML cannot be parsed into the manifest schema.
pub fn parse_manifest_toml(text: &str) -> Result<BacktestingRunManifest> {
    let manifest: BacktestingRunManifest =
        toml::from_str(text).map_err(|error| anyhow::anyhow!("invalid manifest TOML: {error}"))?;
    if manifest.run_id.trim().is_empty() {
        bail!("manifest run_id must not be empty");
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_proof::{SourceProofFidelityClass, synthetic_accepted_dataset_for_tests};
    use nautilus_execution::models::latency::LatencyModel;

    const TEST_INSTRUMENT_ID: &str = "TESTPAIR.TESTVENUE";
    const TEST_BAR_TYPE: &str = "TESTPAIR.TESTVENUE-1-MINUTE-LAST-EXTERNAL";
    const TEST_RUN_ID: &str = "backtesting-vertical-slice-testpair-2026-03-01";
    const TEST_SOURCE_PROOF_ID: &str = "source-proof-synthetic-native-trades";
    const TEST_SOURCE_BINDING: &str = "synthetic-native-trades";
    const TEST_NT_VENUE: &str = "TESTVENUE";
    const TEST_SHA256_ZERO: &str =
        "0000000000000000000000000000000000000000000000000000000000000000";
    const TEST_SHA256_ONE: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    fn accepted_dataset() -> AcceptedDataset {
        synthetic_accepted_dataset_for_tests()
    }

    fn binary_option_accepted_dataset() -> AcceptedDataset {
        let mut accepted = accepted_dataset();
        accepted.product_family = "prediction-market".to_string();
        accepted.product_category = "binary-option".to_string();
        accepted.fixture_type = FixtureType::BinaryOption;
        accepted
    }

    fn valid_manifest() -> BacktestingRunManifest {
        BacktestingRunManifest {
            manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
            run_id: TEST_RUN_ID.to_string(),
            target_bolt_v2_branch: "main".to_string(),
            target_bolt_v2_ref: "refs/heads/main".to_string(),
            resolved_nt_version:
                crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
                    .expect("BVS NautilusTrader dependency provenance"),
            market_structure_fixture: MarketStructureFixture::PerpsSpot,
            venue_binding_key: TEST_SOURCE_BINDING.to_string(),
            run_purpose: RunPurpose::Normal,
            source_proof_id: TEST_SOURCE_PROOF_ID.to_string(),
            source_proof_version: 1,
            pins_non_latest_proof: false,
            proof_pin_reason_code: None,
            proof_pin_reason_detail: None,
            strategy: StrategySource {
                source_kind: StrategySourceKind::CompiledRustRegistry,
                registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
                parameters: BTreeMap::from([
                    ("trade_size".to_string(), "0.01".to_string()),
                    ("bar_type".to_string(), TEST_BAR_TYPE.to_string()),
                ]),
                typed_config_uri: None,
                typed_config_hash: None,
                experiment_result_uri: None,
                experiment_result_hash: None,
                config_overlay: None,
            },
            strategy_config_hash: TEST_SHA256_ZERO.to_string(),
            venue: ManifestVenueConfig {
                nt_venue: TEST_NT_VENUE.to_string(),
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
            },
            additional_venues: Vec::new(),
            nt_streaming_chunk_size: 128,
            catalog_inputs: vec![ManifestCatalogInput {
                catalog_path: "/tmp/catalog".to_string(),
                catalog_fs_protocol: CATALOG_FS_PROTOCOL_NONE.to_string(),
                catalog_fs_storage_options: BTreeMap::new(),
                catalog_fs_rust_storage_options: BTreeMap::new(),
                data_type: "TradeTick".to_string(),
                nt_instrument_id: TEST_INSTRUMENT_ID.to_string(),
                instrument_ids: None,
                start_time: None,
                end_time: None,
                filter_expr: None,
                client_id: None,
                metadata: None,
                bar_spec: None,
                bar_types: None,
                optimize_file_loading: None,
            }],
            reconstructed_reference_current_price: Vec::new(),
            instrument_settlements: Vec::new(),
            execution_model: "nt_backtest_node".to_string(),
            artifact_root: "s3://bolt-parquet/nt-research-analytics".to_string(),
            output_prefix: "s3://bolt-parquet/nt-research-analytics/backtests/testpair".to_string(),
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

    fn valid_catalog_run_view_authority(
        manifest: &BacktestingRunManifest,
    ) -> CatalogRunViewAuthority {
        let guard = OperatorWorkBudgetGuard::unbounded();
        let physical_manifest = CatalogProjectionManifestDocument {
            schema_version: CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION.to_string(),
            objects: vec![CatalogProjectionManifestObject {
                relative_path: "data/trade_tick/test.parquet".to_string(),
                byte_len: 4,
                sha256: TEST_SHA256_ZERO.to_string(),
            }],
        };
        CatalogRunViewAuthority {
            schema_version: CATALOG_RUN_VIEW_AUTHORITY_SCHEMA_VERSION.to_string(),
            run_id: manifest.run_id.clone(),
            submitted_manifest_hash: manifest.manifest_hash(),
            roots: vec![CatalogRunViewRootAuthority {
                catalog_inputs: vec![CatalogRunViewInputAuthority {
                    catalog_input_index: 0,
                    data_type: manifest.catalog_inputs[0].data_type.clone(),
                    nt_instrument_id: manifest.catalog_inputs[0].nt_instrument_id.clone(),
                }],
                logical_catalog_hash: TEST_SHA256_ONE.to_string(),
                physical_manifest_sha256: physical_manifest
                    .manifest_sha256_guarded(&guard, OperatorWorkBudgetStage::Backtest)
                    .expect("hash physical manifest"),
                physical_manifest,
            }],
        }
    }

    fn valid_submitted_run_identity(manifest: &BacktestingRunManifest) -> SubmittedRunIdentity {
        SubmittedRunIdentity::new(manifest, &manifest.manifest_hash())
            .expect("valid submitted run identity")
    }

    fn bounded_authority_guard(
        max_decoded_bytes: u64,
        max_projected_row_groups: u64,
    ) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::new(crate::operator_work_budget::OperatorWorkBudget::Backfill(
            crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                max_decoded_bytes,
                max_source_rows: u64::MAX,
                max_projected_row_groups,
                max_wall_seconds: 60,
                require_object_selection_metadata: true,
            },
        ))
        .expect("valid bounded authority work budget")
    }

    #[test]
    fn valid_manifest_passes_and_maps_to_nt_configs() {
        let manifest = valid_manifest();
        manifest.validate(&accepted_dataset()).expect("valid");
        let run = manifest.to_nt_run_config().expect("run config");
        assert_eq!(run.id(), TEST_RUN_ID);
        assert_eq!(run.venues().len(), 1);
        assert_eq!(run.data().len(), 1);
        assert_eq!(run.chunk_size(), Some(128));
    }

    #[test]
    fn run_config_rejects_zero_streaming_chunk_size() {
        let mut manifest = valid_manifest();
        manifest.nt_streaming_chunk_size = 0;

        assert!(matches!(
            manifest.to_nt_run_config(),
            Err(ManifestError::InvalidNtConfig {
                field: "nt_streaming_chunk_size",
                ..
            })
        ));
    }

    #[test]
    fn run_config_rejects_multiple_catalog_inputs_at_pinned_nt() {
        let mut manifest = valid_manifest();
        manifest
            .catalog_inputs
            .push(manifest.catalog_inputs[0].clone());

        assert!(matches!(
            manifest.to_nt_run_config(),
            Err(ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                ..
            })
        ));
    }

    #[test]
    fn catalog_run_view_authority_rejects_embedded_inventory_drift() {
        let manifest = valid_manifest();
        let submitted_identity = valid_submitted_run_identity(&manifest);
        let mut authority = valid_catalog_run_view_authority(&manifest);
        authority.roots[0].physical_manifest.objects[0].sha256 = TEST_SHA256_ONE.to_string();

        let error = authority
            .validate_for_runtime_manifest(
                &manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            )
            .expect_err("embedded inventory drift must invalidate its manifest pin");

        assert!(matches!(
            error,
            ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.roots.physical_manifest_sha256",
                ..
            }
        ));
    }

    #[test]
    fn catalog_run_view_authority_rejects_non_normal_relative_paths() {
        let manifest = valid_manifest();
        let submitted_identity = valid_submitted_run_identity(&manifest);
        let mut authority = valid_catalog_run_view_authority(&manifest);
        authority.roots[0].physical_manifest.objects[0].relative_path =
            "../stray.parquet".to_string();

        let error = authority
            .validate_for_runtime_manifest(
                &manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            )
            .expect_err("catalog traversal path must fail closed");

        assert!(matches!(
            error,
            ManifestError::InvalidCatalogProjectionManifest {
                field: "catalog_run_view_authority.roots.physical_manifest.objects.relative_path",
                ..
            }
        ));
    }

    #[test]
    fn catalog_run_view_authority_binds_exact_submitted_manifest_and_input_coverage() {
        let manifest = valid_manifest();
        let submitted_identity = valid_submitted_run_identity(&manifest);
        let authority = valid_catalog_run_view_authority(&manifest);
        authority
            .validate_for_runtime_manifest(
                &manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            )
            .expect("valid authority");

        let mut changed_manifest = manifest.clone();
        changed_manifest.catalog_inputs[0].catalog_path = "/tmp/hydrated-catalog".to_string();
        changed_manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        changed_manifest.catalog_inputs[0].catalog_fs_storage_options =
            BTreeMap::from([("region".to_string(), "local-test".to_string())]);
        changed_manifest.catalog_inputs[0].catalog_fs_rust_storage_options =
            BTreeMap::from([("endpoint".to_string(), "local-test".to_string())]);
        authority
            .validate_for_runtime_manifest(
                &changed_manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            )
            .expect("catalog location rewrite remains structurally bound");

        let mut changed_runtime_semantics = manifest.clone();
        changed_runtime_semantics.nt_streaming_chunk_size += 1;
        let mut changed_strategy = manifest.clone();
        changed_strategy
            .strategy
            .parameters
            .insert("trade_size".to_string(), "0.02".to_string());
        let mut changed_venue = manifest.clone();
        changed_venue.venue.trade_execution = false;
        let mut changed_time = manifest.clone();
        changed_time.catalog_inputs[0].start_time = Some(1);
        let mut changed_type = manifest.clone();
        changed_type.catalog_inputs[0].data_type = "QuoteTick".to_string();
        for changed in [
            &changed_runtime_semantics,
            &changed_strategy,
            &changed_venue,
            &changed_time,
            &changed_type,
        ] {
            assert!(matches!(
                authority.validate_for_runtime_manifest(
                    changed,
                    &submitted_identity,
                    &OperatorWorkBudgetGuard::unbounded(),
                    OperatorWorkBudgetStage::Backtest,
                ),
                Err(ManifestError::InvalidCatalogRunViewAuthority {
                    field: "catalog_run_view_authority.submitted_manifest_hash",
                    ..
                })
            ));
        }
        assert!(matches!(
            authority.validate_submitted_manifest_identity(
                &SubmittedRunIdentity::new(
                    &changed_runtime_semantics,
                    &changed_runtime_semantics.manifest_hash()
                )
                .expect("changed submitted identity")
            ),
            Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.submitted_manifest_hash",
                ..
            })
        ));

        let mut missing_input = authority.clone();
        missing_input.roots[0].catalog_inputs.clear();
        assert!(matches!(
            missing_input.validate_for_runtime_manifest(
                &manifest,
                &submitted_identity,
                &OperatorWorkBudgetGuard::unbounded(),
                OperatorWorkBudgetStage::Backtest,
            ),
            Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.roots.catalog_inputs",
                ..
            })
        ));

        assert!(matches!(
            SubmittedRunIdentity::new(&manifest, TEST_SHA256_ZERO),
            Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "submitted_run_identity.manifest_hash",
                ..
            })
        ));
    }

    #[test]
    fn catalog_run_view_authority_serialization_and_inventory_are_budget_bounded() {
        let manifest = valid_manifest();
        let submitted_identity = valid_submitted_run_identity(&manifest);
        let authority = valid_catalog_run_view_authority(&manifest);
        let unbounded = OperatorWorkBudgetGuard::unbounded();

        let bytes = authority
            .canonical_bytes_guarded(
                &manifest,
                &submitted_identity,
                &unbounded,
                OperatorWorkBudgetStage::Finalize,
            )
            .expect("serialize valid authority");
        let hash = authority
            .authority_sha256_guarded(
                &manifest,
                &submitted_identity,
                &unbounded,
                OperatorWorkBudgetStage::Finalize,
            )
            .expect("hash valid authority");
        assert_eq!(hash, crate::hashing::sha256_hex(&bytes));

        let tiny_bytes = bounded_authority_guard(32, 8);
        assert!(
            authority
                .canonical_bytes_guarded(
                    &manifest,
                    &submitted_identity,
                    &tiny_bytes,
                    OperatorWorkBudgetStage::Finalize,
                )
                .is_err(),
            "authority serialization must respect max_decoded_bytes"
        );

        let mut two_objects = authority.clone();
        let mut second = two_objects.roots[0].physical_manifest.objects[0].clone();
        second.relative_path = "data/trade_tick/z-test.parquet".to_string();
        two_objects.roots[0].physical_manifest.objects.push(second);
        two_objects.roots[0].physical_manifest_sha256 = two_objects.roots[0]
            .physical_manifest
            .manifest_sha256_guarded(&unbounded, OperatorWorkBudgetStage::Finalize)
            .expect("hash two-object physical inventory");
        let one_object_cap = bounded_authority_guard(1_000_000, 1);
        assert!(matches!(
            two_objects.validate_for_runtime_manifest(
                &manifest,
                &submitted_identity,
                &one_object_cap,
                OperatorWorkBudgetStage::Finalize,
            ),
            Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.roots.physical_manifest.objects",
                ..
            })
        ));

        let mut large_physical = authority.clone();
        large_physical.roots[0].physical_manifest.objects[0].byte_len = 10_000;
        large_physical.roots[0].physical_manifest_sha256 = large_physical.roots[0]
            .physical_manifest
            .manifest_sha256_guarded(&unbounded, OperatorWorkBudgetStage::Finalize)
            .expect("hash large physical inventory");
        let small_physical_cap = bounded_authority_guard(1_024, 8);
        assert!(matches!(
            large_physical.validate_for_runtime_manifest(
                &manifest,
                &submitted_identity,
                &small_physical_cap,
                OperatorWorkBudgetStage::Finalize,
            ),
            Err(ManifestError::InvalidCatalogRunViewAuthority {
                field: "catalog_run_view_authority.physical_bytes",
                ..
            })
        ));
    }

    #[test]
    fn additional_venues_map_to_nt_run_config() {
        let mut manifest = valid_manifest();
        let mut okx = manifest.venue.clone();
        okx.nt_venue = "OKX".to_string();
        okx.book_type = "L1_MBP".to_string();
        okx.starting_balances = vec!["1_000_000 USDT".to_string()];
        manifest.additional_venues.push(okx);

        manifest.validate(&accepted_dataset()).expect("valid");
        let run = manifest.to_nt_run_config().expect("run config");

        assert_eq!(run.venues().len(), 2);
        assert_eq!(run.venues()[0].name().as_str(), TEST_NT_VENUE);
        assert_eq!(run.venues()[1].name().as_str(), "OKX");
    }

    #[test]
    fn non_ascii_catalog_input_fails_closed_without_valid_nt_selector() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "币安人生USDC.BINANCE".to_string();

        let error = manifest
            .to_nt_data_config()
            .expect_err("non-ASCII catalog id must fail closed");

        assert!(matches!(
            error,
            ManifestError::UnsupportedInstrumentIdCharset { instrument_id }
                if instrument_id == "币安人生USDC.BINANCE"
        ));
    }

    #[test]
    fn slash_catalog_instrument_id_keeps_nt_instrument_filter() {
        // urisafe strips '/' before naming the catalog directory, so the
        // directory is plain ASCII and NT's filtered query path works; the
        // unfiltered fallback must NOT trigger for slash ids.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "BASE/QUOTE.TESTVENUE".to_string();

        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(
            data.instrument_id().map(|id| id.to_string()),
            Some("BASE/QUOTE.TESTVENUE".to_string())
        );
    }

    #[test]
    fn percent_catalog_instrument_id_fails_loud() {
        // A literal '%' survives urisafe unchanged and corrupts through every
        // percent-encode/decode layer (filtered, unfiltered, and node paths),
        // so the manifest must reject it instead of producing an empty feed.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "BASE%QUOTE.TESTVENUE".to_string();

        let error = manifest.to_nt_data_config().expect_err("charset rejected");

        assert!(matches!(
            error,
            ManifestError::UnsupportedInstrumentIdCharset { instrument_id }
                if instrument_id == "BASE%QUOTE.TESTVENUE"
        ));
    }

    #[test]
    fn tilde_catalog_instrument_id_fails_loud() {
        // '~' is RFC 3986 unreserved but object_store's INVALID encode set
        // percent-encodes it, so the on-disk directory becomes '%7E'-encoded
        // while the all-ASCII id keeps NT's filtered query path — the filter
        // can never match the encoded directory. Reject at manifest build
        // instead of failing late inside the NT node load.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "BASE~QUOTE.TESTVENUE".to_string();

        let error = manifest.to_nt_data_config().expect_err("charset rejected");

        assert!(matches!(
            error,
            ManifestError::UnsupportedInstrumentIdCharset { instrument_id }
                if instrument_id == "BASE~QUOTE.TESTVENUE"
        ));
    }

    #[test]
    fn validate_rejects_unsupported_charset_instrument_id_at_preflight() {
        // The Gate 4 preflight (validate + validate_run_spec_manifest_for_
        // object_hash) must reject the same ids the NT config build rejects;
        // otherwise the manifest is admitted, conversion and projection run,
        // and the id defect only surfaces at NT-config construction mid-run.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "BASE~QUOTE.TESTVENUE".to_string();

        let error = manifest
            .validate(&accepted_dataset())
            .expect_err("charset rejected at preflight");

        assert!(matches!(
            error,
            ManifestError::UnsupportedInstrumentIdCharset { instrument_id }
                if instrument_id == "BASE~QUOTE.TESTVENUE"
        ));
    }

    #[test]
    fn validate_rejects_unparseable_instrument_id_at_preflight() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "NOVENUESEPARATOR".to_string();

        let error = manifest
            .validate(&accepted_dataset())
            .expect_err("unparseable id rejected at preflight");

        assert!(matches!(
            error,
            ManifestError::InvalidInstrumentId { instrument_id }
                if instrument_id == "NOVENUESEPARATOR"
        ));
    }

    #[test]
    fn validate_rejects_unsupported_charset_in_instrument_ids_list_at_preflight() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].instrument_ids = Some(vec!["BASE~QUOTE.TESTVENUE".to_string()]);

        let error = manifest
            .validate(&accepted_dataset())
            .expect_err("charset rejected at preflight");

        assert!(matches!(
            error,
            ManifestError::UnsupportedInstrumentIdCharset { instrument_id }
                if instrument_id == "BASE~QUOTE.TESTVENUE"
        ));
    }

    #[test]
    fn venue_config_maps_explicit_nt_venue_controls() {
        let mut manifest = valid_manifest();
        manifest.venue.routing = true;
        manifest.venue.frozen_account = true;
        manifest.venue.reject_stop_orders = false;
        manifest.venue.support_gtd_orders = false;
        manifest.venue.support_contingent_orders = false;
        manifest.venue.use_position_ids = false;
        manifest.venue.use_random_ids = true;
        manifest.venue.use_reduce_only = false;
        manifest.venue.bar_execution = false;
        manifest.venue.bar_adaptive_high_low_ordering = true;
        manifest.venue.trade_execution = false;
        manifest.venue.use_market_order_acks = true;
        manifest.venue.liquidity_consumption = true;
        manifest.venue.allow_cash_borrowing = true;
        manifest.venue.queue_position = true;
        manifest.venue.oto_trigger_mode = "FULL".to_string();
        manifest.venue.base_currency = "USDC".to_string();
        manifest.venue.default_leverage = "2".to_string();
        manifest.venue.price_protection_points = 7;

        let venue = manifest.to_nt_venue_config().expect("venue config");
        assert!(venue.routing());
        assert!(venue.frozen_account());
        assert!(!venue.reject_stop_orders());
        assert!(!venue.support_gtd_orders());
        assert!(!venue.support_contingent_orders());
        assert!(!venue.use_position_ids());
        assert!(venue.use_random_ids());
        assert!(!venue.use_reduce_only());
        assert!(!venue.bar_execution());
        assert!(venue.bar_adaptive_high_low_ordering());
        assert!(!venue.trade_execution());
        assert!(venue.use_market_order_acks());
        assert!(venue.liquidity_consumption());
        assert!(venue.allow_cash_borrowing());
        assert!(venue.queue_position());
        assert_eq!(
            venue.oto_trigger_mode(),
            nautilus_model::enums::OtoTriggerMode::Full
        );
        assert_eq!(
            venue.base_currency().expect("base currency").to_string(),
            "USDC"
        );
        assert_eq!(venue.default_leverage(), rust_decimal::Decimal::from(2));
        assert_eq!(venue.price_protection_points(), 7);
    }

    #[test]
    fn venue_config_registers_polymarket_cost_realism_models_with_nt() {
        let mut manifest = valid_manifest();
        manifest.venue.fill_model = Some(ManifestFillModelConfig {
            kind: FILL_MODEL_PREDICTION_MARKET_PROBABILISTIC.to_string(),
            prob_fill_on_limit: "0.75".to_string(),
            prob_slippage: "0.04".to_string(),
            random_seed: Some(42),
        });
        manifest.venue.latency_model = Some(ManifestLatencyModelConfig {
            kind: LATENCY_MODEL_PREDICTION_MARKET_STATIC.to_string(),
            base_latency_nanos: 100,
            insert_latency_nanos: 500,
            update_latency_nanos: 700,
            delete_latency_nanos: 900,
        });
        manifest.venue.fee_model = Some(ManifestFeeModelConfig {
            kind: FEE_MODEL_PREDICTION_MARKET_MAKER_TAKER.to_string(),
        });

        manifest
            .validate(&accepted_dataset())
            .expect("cost realism manifest should validate");
        let venue = manifest.to_nt_venue_config().expect("venue config");

        assert!(matches!(
            venue.fill_model(),
            Some(FillModelAny::Probabilistic(_))
        ));
        assert!(matches!(
            venue.fee_model(),
            Some(FeeModelAny::MakerTaker(_))
        ));
        match venue.latency_model() {
            Some(LatencyModelAny::Static(model)) => {
                assert_eq!(model.get_base_latency(), UnixNanos::from(100));
                assert_eq!(model.get_insert_latency(), UnixNanos::from(600));
                assert_eq!(model.get_update_latency(), UnixNanos::from(800));
                assert_eq!(model.get_delete_latency(), UnixNanos::from(1000));
            }
            other => panic!("expected static latency model, got {other:?}"),
        }

        let surfaces = manifest.resolved_nt_surfaces().expect("surfaces");
        for surface in ["venue.fill_model", "venue.latency_model", "venue.fee_model"] {
            assert!(
                surfaces.iter().any(|resolved| resolved.surface == surface),
                "{surface} must be durably reported"
            );
        }
    }

    #[test]
    fn alternate_venue_provider_swap_is_toml_only() {
        let mut accepted = accepted_dataset();
        accepted.source_binding = "alt-native-trades".to_string();
        accepted.source_proof_id = "source-proof-alt-native-trades".to_string();
        accepted.venue = "altvenue".to_string();

        let mut manifest = valid_manifest();
        manifest.run_id = "backtesting-vertical-slice-altvenue-2026-03-01".to_string();
        manifest.venue_binding_key = accepted.source_binding.clone();
        manifest.source_proof_id = accepted.source_proof_id.clone();
        manifest.venue.nt_venue = "ALTVENUE".to_string();
        manifest.strategy.parameters.insert(
            "bar_type".to_string(),
            "ALTUSD.ALTVENUE-1-MINUTE-LAST-INTERNAL".to_string(),
        );
        manifest.catalog_inputs[0].nt_instrument_id = "ALTUSD.ALTVENUE".to_string();
        manifest.output_prefix =
            "s3://bolt-parquet/nt-research-analytics/backtests/altvenue".to_string();

        manifest
            .validate(&accepted)
            .expect("alternate TOML-only venue/provider binding");
        let venue = manifest.to_nt_venue_config().expect("venue config");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(venue.name().as_str(), "ALTVENUE");
        assert_eq!(
            data.instrument_id().expect("instrument id").to_string(),
            "ALTUSD.ALTVENUE"
        );
    }

    #[test]
    fn data_config_maps_catalog_cloud_options() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("allow_http".to_string(), "false".to_string()),
        ]);

        let data = manifest.to_nt_data_config().expect("data config");
        assert_eq!(data.catalog_path(), manifest.catalog_inputs[0].catalog_path);
        assert_eq!(data.catalog_fs_protocol(), Some("s3"));
        assert!(data.catalog_fs_storage_options().is_none());
        assert_eq!(
            data.catalog_fs_rust_storage_options()
                .expect("storage options")
                .get("region"),
            Some(&"us-east-1".to_string())
        );
        assert_eq!(
            data.catalog_fs_rust_storage_options()
                .expect("rust storage options")
                .get("allow_http"),
            Some(&"false".to_string())
        );
    }

    #[test]
    fn data_config_preserves_configured_object_store_conditional_put() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("conditional_put".to_string(), "etag".to_string()),
        ]);

        let data = manifest.to_nt_data_config().expect("data config");
        assert_eq!(
            data.catalog_fs_rust_storage_options()
                .expect("rust storage options")
                .get("conditional_put"),
            Some(&"etag".to_string())
        );
    }

    #[test]
    fn validate_rejects_raw_credentials_in_catalog_storage_options() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("access_key_id".to_string(), "AKIATEST".to_string()),
        ]);

        let error = manifest
            .validate(&accepted_dataset())
            .expect_err("raw catalog credentials must fail admission");
        assert!(matches!(
            error,
            ManifestError::RawCredentialOption { field, .. }
                if field == "catalog_inputs.catalog_fs_storage_options"
        ));
    }

    #[test]
    fn validate_rejects_raw_credentials_in_catalog_rust_storage_options() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("secret_access_key".to_string(), "not-from-ssm".to_string()),
        ]);

        let error = manifest
            .validate(&accepted_dataset())
            .expect_err("raw catalog credentials must fail admission");
        assert!(matches!(
            error,
            ManifestError::RawCredentialOption { field, .. }
                if field == "catalog_inputs.catalog_fs_rust_storage_options"
        ));
    }

    #[test]
    fn runtime_resolved_catalog_credentials_still_map_to_nt_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("access_key_id".to_string(), "AKIATEST".to_string()),
            ("secret_access_key".to_string(), "secret-value".to_string()),
        ]);

        let data = manifest
            .to_nt_data_config()
            .expect("SSM-resolved runtime credentials must stay valid for the NT config build");
        assert_eq!(data.catalog_fs_protocol(), Some("s3"));
    }

    #[test]
    fn artifact_store_options_are_toml_owned_for_publish_and_catalog_proof() {
        let mut manifest = valid_manifest();
        manifest.artifact_root = "file:///bolt-artifacts".to_string();
        manifest.output_prefix = "file:///bolt-artifacts/backtests/testpair".to_string();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("allow_http".to_string(), "false".to_string()),
        ]);

        let options = manifest
            .artifact_store_storage_options()
            .expect("artifact store options")
            .expect("rust storage options present");
        assert_eq!(options.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(options.get("allow_http"), Some(&"false".to_string()));
    }

    #[test]
    fn artifact_store_preserves_conditional_put_after_ssm_resolution() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("conditional_put".to_string(), "etag".to_string()),
        ]);
        manifest.artifact_store.ssm_parameters = Some(ManifestArtifactStoreSsmParameters {
            region: "us-east-1".to_string(),
            access_key_id: "/bolt/artifacts/access-key-id".to_string(),
            secret_access_key: "/bolt/artifacts/secret-access-key".to_string(),
            session_token: None,
        });

        let options = manifest
            .artifact_store_storage_options_resolved(&mut |_region, path| match path {
                "/bolt/artifacts/access-key-id" => Ok("AKIATEST".to_string()),
                "/bolt/artifacts/secret-access-key" => Ok("secret-value".to_string()),
                other => Err(format!("unexpected path {other}")),
            })
            .expect("resolved artifact store options")
            .expect("artifact store options");

        assert_eq!(options.get("conditional_put"), Some(&"etag".to_string()));
        assert_eq!(options.get("access_key_id"), Some(&"AKIATEST".to_string()));
        assert_eq!(
            options.get("secret_access_key"),
            Some(&"secret-value".to_string())
        );
    }

    #[test]
    fn artifact_store_rejects_disabled_conditional_put_for_s3_commit_path() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("conditional_put".to_string(), "disabled".to_string()),
        ]);

        let err = manifest
            .artifact_store_storage_options_resolved(&mut |_region, _path| {
                Ok("unused-secret".to_string())
            })
            .unwrap_err();

        assert!(
            err.to_string().contains("conditional_put")
                && err.to_string().contains("Artifact Index"),
            "{err}"
        );
    }

    #[test]
    fn artifact_store_rejects_s3_publish_without_resolved_ssm_credentials() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        let mut resolver_called = false;

        let err = manifest
            .artifact_store_storage_options_resolved(&mut |_region, _path| {
                resolver_called = true;
                Ok("unexpected-secret".to_string())
            })
            .unwrap_err();

        assert!(
            !resolver_called,
            "missing SSM parameter config must fail before any secret lookup"
        );
        assert!(
            err.to_string().contains("artifact_store.ssm_parameters")
                && err.to_string().contains("s3 output_prefix")
                && !err.to_string().contains("unexpected-secret"),
            "error must explain the missing SSM credential binding without exposing values: {err}"
        );
    }

    #[test]
    fn artifact_store_resolves_s3_credentials_from_ssm_parameters() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        manifest.artifact_store.ssm_parameters = Some(ManifestArtifactStoreSsmParameters {
            region: "us-east-1".to_string(),
            access_key_id: "/bolt/artifacts/access-key-id".to_string(),
            secret_access_key: "/bolt/artifacts/secret-access-key".to_string(),
            session_token: Some("/bolt/artifacts/session-token".to_string()),
        });

        let mut requested_paths = Vec::new();
        let options = manifest
            .artifact_store_storage_options_resolved(&mut |region, path| {
                assert_eq!(region, "us-east-1");
                requested_paths.push(path.to_string());
                match path {
                    "/bolt/artifacts/access-key-id" => Ok("AKIATEST".to_string()),
                    "/bolt/artifacts/secret-access-key" => Ok("secret-value".to_string()),
                    "/bolt/artifacts/session-token" => Ok("session-value".to_string()),
                    other => Err(format!("unexpected path {other}")),
                }
            })
            .expect("resolved artifact store options")
            .expect("s3 options");

        assert_eq!(
            requested_paths,
            vec![
                "/bolt/artifacts/access-key-id".to_string(),
                "/bolt/artifacts/secret-access-key".to_string(),
                "/bolt/artifacts/session-token".to_string(),
            ]
        );
        assert_eq!(options.get("region"), Some(&"us-east-1".to_string()));
        assert_eq!(options.get("access_key_id"), Some(&"AKIATEST".to_string()));
        assert_eq!(
            options.get("secret_access_key"),
            Some(&"secret-value".to_string())
        );
        assert_eq!(
            options.get("session_token"),
            Some(&"session-value".to_string())
        );
    }

    #[test]
    fn artifact_store_rejects_empty_or_whitespace_resolved_ssm_credentials() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        manifest.artifact_store.ssm_parameters = Some(ManifestArtifactStoreSsmParameters {
            region: "us-east-1".to_string(),
            access_key_id: "/bolt/artifacts/access-key-id".to_string(),
            secret_access_key: "/bolt/artifacts/secret-access-key".to_string(),
            session_token: Some("/bolt/artifacts/session-token".to_string()),
        });

        let empty_access_key_err = manifest
            .artifact_store_storage_options_resolved(&mut |_region, path| match path {
                "/bolt/artifacts/access-key-id" => Ok(String::new()),
                "/bolt/artifacts/secret-access-key" => Ok("secret-value".to_string()),
                "/bolt/artifacts/session-token" => Ok("session-value".to_string()),
                other => Err(format!("unexpected path {other}")),
            })
            .expect_err("empty resolved access key must fail closed");
        assert!(
            empty_access_key_err
                .to_string()
                .contains("artifact_store.ssm_parameters.access_key_id"),
            "{empty_access_key_err}"
        );

        let whitespace_secret_err = manifest
            .artifact_store_storage_options_resolved(&mut |_region, path| match path {
                "/bolt/artifacts/access-key-id" => Ok("AKIATEST".to_string()),
                "/bolt/artifacts/secret-access-key" => Ok(" secret-value ".to_string()),
                "/bolt/artifacts/session-token" => Ok("session-value".to_string()),
                other => Err(format!("unexpected path {other}")),
            })
            .expect_err("whitespace-padded resolved secret must fail closed");
        assert!(
            whitespace_secret_err
                .to_string()
                .contains("artifact_store.ssm_parameters.secret_access_key"),
            "{whitespace_secret_err}"
        );
    }

    #[test]
    fn artifact_store_rejects_raw_s3_credentials_in_toml() {
        let mut manifest = valid_manifest();
        manifest.artifact_store.rust_storage_options = BTreeMap::from([
            ("region".to_string(), "us-east-1".to_string()),
            ("secret_access_key".to_string(), "not-from-ssm".to_string()),
        ]);

        let err = manifest.artifact_store_storage_options().unwrap_err();

        assert!(
            err.to_string().contains("artifact_store")
                && err.to_string().contains("SSM")
                && !err.to_string().contains("not-from-ssm"),
            "error must reject raw credentials without rendering secret value: {err}"
        );
    }

    #[test]
    fn rejects_empty_starting_balances() {
        let mut manifest = valid_manifest();
        manifest.venue.starting_balances.clear();
        assert!(matches!(
            manifest.validate(&accepted_dataset()),
            Err(ManifestError::MissingField("venue.starting_balances"))
        ));
    }

    #[test]
    fn rejects_malformed_starting_balance() {
        let mut manifest = valid_manifest();
        manifest.venue.starting_balances = vec!["not money".to_string()];
        assert!(matches!(
            manifest.validate(&accepted_dataset()),
            Err(ManifestError::InvalidStartingBalance { balance }) if balance == "not money"
        ));
    }

    #[test]
    fn manifest_hash_is_stable_and_content_sensitive() {
        let manifest = valid_manifest();
        let round_tripped: BacktestingRunManifest =
            toml::from_str(&toml::to_string(&manifest).expect("serialize")).expect("parse");
        assert_eq!(manifest.manifest_hash(), round_tripped.manifest_hash());

        let mut changed = manifest.clone();
        changed.start_time = Some(1_772_323_200_000_000_000);
        assert_ne!(manifest.manifest_hash(), changed.manifest_hash());
    }

    #[test]
    fn manifest_hash_covers_venue_controls_and_catalog_cloud_fields() {
        fn assert_hash_changes(label: &str, mutate: impl FnOnce(&mut BacktestingRunManifest)) {
            let manifest = valid_manifest();
            let mut changed = manifest.clone();
            mutate(&mut changed);
            assert_ne!(
                manifest.manifest_hash(),
                changed.manifest_hash(),
                "{label} must affect the manifest hash"
            );
        }

        assert_hash_changes("venue.routing", |manifest| {
            manifest.venue.routing = true;
        });
        assert_hash_changes("venue.reject_stop_orders", |manifest| {
            manifest.venue.reject_stop_orders = false;
        });
        assert_hash_changes("venue.base_currency", |manifest| {
            manifest.venue.base_currency = "USDC".to_string();
        });
        assert_hash_changes("venue.price_protection_points", |manifest| {
            manifest.venue.price_protection_points = 7;
        });
        assert_hash_changes("venue.fill_model", |manifest| {
            manifest.venue.fill_model = Some(ManifestFillModelConfig {
                kind: FILL_MODEL_PREDICTION_MARKET_PROBABILISTIC.to_string(),
                prob_fill_on_limit: "0.75".to_string(),
                prob_slippage: "0.04".to_string(),
                random_seed: Some(42),
            });
        });
        assert_hash_changes("venue.latency_model", |manifest| {
            manifest.venue.latency_model = Some(ManifestLatencyModelConfig {
                kind: LATENCY_MODEL_PREDICTION_MARKET_STATIC.to_string(),
                base_latency_nanos: 100,
                insert_latency_nanos: 500,
                update_latency_nanos: 700,
                delete_latency_nanos: 900,
            });
        });
        assert_hash_changes("venue.fee_model", |manifest| {
            manifest.venue.fee_model = Some(ManifestFeeModelConfig {
                kind: FEE_MODEL_PREDICTION_MARKET_MAKER_TAKER.to_string(),
            });
        });
        assert_hash_changes("domain_metrics", |manifest| {
            manifest.domain_metrics.push(ManifestDomainMetricConfig {
                kind: DOMAIN_METRIC_CLOSED_POSITION_RATIO.to_string(),
            });
        });
        assert_hash_changes("nt_streaming_chunk_size", |manifest| {
            manifest.nt_streaming_chunk_size += 1;
        });
        assert_hash_changes("catalog_inputs.catalog_fs_protocol", |manifest| {
            manifest.catalog_inputs[0].catalog_path =
                "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
            manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        });
        assert_hash_changes(
            "catalog_inputs.catalog_fs_rust_storage_options",
            |manifest| {
                manifest.catalog_inputs[0].catalog_fs_rust_storage_options =
                    BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
            },
        );
        assert_hash_changes("manifest_schema_version", |manifest| {
            manifest.manifest_schema_version = "backtesting-run-manifest.v3".to_string();
        });
        assert_hash_changes("target_bolt_v2_branch", |manifest| {
            manifest.target_bolt_v2_branch = "release/backtesting".to_string();
        });
        assert_hash_changes("target_bolt_v2_ref", |manifest| {
            manifest.target_bolt_v2_ref = "refs/heads/release/backtesting".to_string();
        });
        assert_hash_changes("resolved_nt_version", |manifest| {
            manifest.resolved_nt_version = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        });
        assert_hash_changes("strategy_config_hash", |manifest| {
            manifest.strategy_config_hash =
                "2222222222222222222222222222222222222222222222222222222222222222".to_string();
        });
        assert_hash_changes("execution_model", |manifest| {
            manifest.execution_model = "alternate_nt_execution_model".to_string();
        });
    }

    #[test]
    fn artifact_manifest_records_deferred_currentness_rule_slots() {
        let artifact = valid_manifest()
            .to_artifact_manifest()
            .expect("build artifact manifest");

        assert_eq!(
            artifact.currentness_rule_slots,
            vec![
                ManifestCurrentnessRuleSlot {
                    dimension: ManifestCurrentnessDimension::NtVersion,
                    status: ManifestCurrentnessRuleStatus::Deferred,
                },
                ManifestCurrentnessRuleSlot {
                    dimension: ManifestCurrentnessDimension::StrategyConfigHash,
                    status: ManifestCurrentnessRuleStatus::Deferred,
                },
                ManifestCurrentnessRuleSlot {
                    dimension: ManifestCurrentnessDimension::ManifestSchema,
                    status: ManifestCurrentnessRuleStatus::Deferred,
                },
                ManifestCurrentnessRuleSlot {
                    dimension: ManifestCurrentnessDimension::ExecutionModel,
                    status: ManifestCurrentnessRuleStatus::Deferred,
                },
            ]
        );
    }

    #[test]
    fn manifest_toml_schema_records_currentness_dimensions() {
        let manifest = valid_manifest();
        let text = toml::to_string(&manifest).expect("serialize manifest");

        for field in [
            "manifest_schema_version",
            "target_bolt_v2_branch",
            "target_bolt_v2_ref",
            "resolved_nt_version",
            "strategy_config_hash",
            "execution_model",
        ] {
            assert!(
                text.contains(field),
                "manifest TOML should include required currentness/reproducibility field {field}"
            );
        }

        let parsed = parse_manifest_toml(&text).expect("parse manifest with currentness fields");
        assert_eq!(
            parsed.manifest_schema_version,
            manifest.manifest_schema_version
        );
        assert_eq!(parsed.target_bolt_v2_branch, manifest.target_bolt_v2_branch);
        assert_eq!(parsed.target_bolt_v2_ref, manifest.target_bolt_v2_ref);
        assert_eq!(parsed.resolved_nt_version, manifest.resolved_nt_version);
        assert_eq!(parsed.strategy_config_hash, manifest.strategy_config_hash);
        assert_eq!(parsed.execution_model, manifest.execution_model);
    }

    #[test]
    fn rejects_invalid_currentness_schema_dimensions() {
        let mut manifest = valid_manifest();
        manifest.manifest_schema_version = "backtesting-run-manifest.v0".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "manifest_schema_version",
                ..
            }
        ));

        let mut manifest = valid_manifest();
        manifest.strategy_config_hash = "not-sha256".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidStrategySourceHash {
                field: "strategy_config_hash",
                ..
            }
        ));
    }

    #[test]
    fn resolved_surfaces_record_custom_currentness_schema_dimensions() {
        let manifest = valid_manifest();
        let surfaces = manifest.resolved_nt_surfaces().expect("resolved surfaces");

        for (surface, nt_field, resolved_value) in [
            (
                "manifest.schema_version",
                "BacktestingRunManifest.manifest_schema_version",
                manifest.manifest_schema_version.as_str(),
            ),
            (
                "target.bolt_v2_branch",
                "BacktestingRunManifest.target_bolt_v2_branch",
                manifest.target_bolt_v2_branch.as_str(),
            ),
            (
                "target.bolt_v2_ref",
                "BacktestingRunManifest.target_bolt_v2_ref",
                manifest.target_bolt_v2_ref.as_str(),
            ),
            (
                "manifest.resolved_nt_version",
                "BacktestingRunManifest.resolved_nt_version",
                manifest.resolved_nt_version.as_str(),
            ),
            (
                "strategy.config_hash",
                "BacktestingRunManifest.strategy_config_hash",
                manifest.strategy_config_hash.as_str(),
            ),
            (
                "execution.model",
                "BacktestingRunManifest.execution_model",
                manifest.execution_model.as_str(),
            ),
        ] {
            assert!(
                surfaces.iter().any(|record| record.surface == surface
                    && record.classification == NtSurfaceClassification::CustomOwned
                    && record.nt_field == nt_field
                    && record.resolved_value == resolved_value),
                "missing custom-owned currentness schema surface {surface}"
            );
        }
    }

    #[test]
    fn resolved_nt_surfaces_record_supported_manifest_to_nt_mappings() {
        let manifest = valid_manifest();
        let surfaces = manifest.resolved_nt_surfaces().expect("resolved surfaces");

        for (surface, classification, nt_field) in [
            (
                "run.id",
                NtSurfaceClassification::PassThrough,
                "BacktestRunConfig.id",
            ),
            (
                "run.start",
                NtSurfaceClassification::PassThrough,
                "BacktestRunConfig.start",
            ),
            (
                "run.end",
                NtSurfaceClassification::PassThrough,
                "BacktestRunConfig.end",
            ),
            (
                "venue.name",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.name",
            ),
            (
                "venue.oms_type",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.oms_type",
            ),
            (
                "venue.account_type",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.account_type",
            ),
            (
                "venue.book_type",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.book_type",
            ),
            (
                "venue.starting_balances",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.starting_balances",
            ),
            (
                "venue.routing",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.routing",
            ),
            (
                "venue.frozen_account",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.frozen_account",
            ),
            (
                "venue.reject_stop_orders",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.reject_stop_orders",
            ),
            (
                "venue.support_gtd_orders",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.support_gtd_orders",
            ),
            (
                "venue.support_contingent_orders",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.support_contingent_orders",
            ),
            (
                "venue.use_position_ids",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_position_ids",
            ),
            (
                "venue.use_random_ids",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_random_ids",
            ),
            (
                "venue.use_reduce_only",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_reduce_only",
            ),
            (
                "venue.bar_execution",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.bar_execution",
            ),
            (
                "venue.bar_adaptive_high_low_ordering",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.bar_adaptive_high_low_ordering",
            ),
            (
                "venue.trade_execution",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.trade_execution",
            ),
            (
                "venue.use_market_order_acks",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.use_market_order_acks",
            ),
            (
                "venue.liquidity_consumption",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.liquidity_consumption",
            ),
            (
                "venue.allow_cash_borrowing",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.allow_cash_borrowing",
            ),
            (
                "venue.queue_position",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.queue_position",
            ),
            (
                "venue.oto_trigger_mode",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.oto_trigger_mode",
            ),
            (
                "venue.base_currency",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.base_currency",
            ),
            (
                "venue.default_leverage",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.default_leverage",
            ),
            (
                "venue.price_protection_points",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.price_protection_points",
            ),
            (
                "venue.fill_model",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.fill_model",
            ),
            (
                "venue.latency_model",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.latency_model",
            ),
            (
                "venue.fee_model",
                NtSurfaceClassification::PassThrough,
                "BacktestVenueConfig.fee_model",
            ),
            (
                "catalog.data_type",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.data_type",
            ),
            (
                "catalog.catalog_path",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.catalog_path",
            ),
            (
                "catalog.catalog_fs_protocol",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.catalog_fs_protocol",
            ),
            (
                "catalog.catalog_fs_storage_options",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.catalog_fs_storage_options",
            ),
            (
                "catalog.catalog_fs_rust_storage_options",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.catalog_fs_rust_storage_options",
            ),
            (
                "catalog.instrument_id",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.instrument_id",
            ),
            (
                "catalog.start_time",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.start_time",
            ),
            (
                "catalog.end_time",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.end_time",
            ),
            (
                "catalog.filter_expr",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.filter_expr",
            ),
            (
                "catalog.optimize_file_loading",
                NtSurfaceClassification::PassThrough,
                "BacktestDataConfig.optimize_file_loading",
            ),
        ] {
            assert!(
                surfaces.iter().any(|resolved| {
                    resolved.surface == surface
                        && resolved.classification == classification
                        && resolved.nt_field == nt_field
                }),
                "missing resolved NT surface {surface}"
            );
        }
    }

    #[test]
    fn resolved_nt_surfaces_record_domain_metric_registration() {
        let mut manifest = valid_manifest();
        manifest.domain_metrics.push(ManifestDomainMetricConfig {
            kind: DOMAIN_METRIC_CLOSED_POSITION_RATIO.to_string(),
        });

        let surfaces = manifest.resolved_nt_surfaces().expect("resolved surfaces");

        assert!(surfaces.iter().any(|resolved| {
            resolved.surface == "domain_metrics[0].kind"
                && resolved.classification == NtSurfaceClassification::CustomOwned
                && resolved.nt_field == "PortfolioAnalyzer::register_statistic"
                && resolved.resolved_value == DOMAIN_METRIC_CLOSED_POSITION_RATIO
        }));
    }

    #[test]
    fn rejects_unknown_domain_metric_selector() {
        let mut manifest = valid_manifest();
        manifest.domain_metrics.push(ManifestDomainMetricConfig {
            kind: "unknown_domain_metric".to_string(),
        });

        assert!(matches!(
            manifest.validate(&accepted_dataset()),
            Err(ManifestError::UnsupportedEnum {
                field: "domain_metrics.kind",
                value
            }) if value == "unknown_domain_metric"
        ));
    }

    fn binary_oracle_config_overlay() -> StrategyConfigOverlaySource {
        StrategyConfigOverlaySource {
            production_root_config_path: "config/root.toml".to_string(),
            override_delta: ManifestBacktestConfigOverride {
                label: "production config + documented multi-venue RV override".to_string(),
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
                        data_client_id: "synthetic_rv_data".to_string(),
                        instrument_id: "BTC-USDT.SYNTHETIC".to_string(),
                    },
                ],
            },
        }
    }

    fn binary_oracle_overlay_manifest() -> BacktestingRunManifest {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = STRATEGY_BINARY_ORACLE_EDGE_TAKER.to_string();
        manifest.strategy.parameters = BTreeMap::from([
            (STRATEGY_PARAM_FEE_BPS.to_string(), "0".to_string()),
            (
                STRATEGY_PARAM_ORDER_EXECUTION_MODE.to_string(),
                "shadow".to_string(),
            ),
        ]);
        manifest.strategy.config_overlay = Some(binary_oracle_config_overlay());
        manifest.strategy_config_hash =
            "2222222222222222222222222222222222222222222222222222222222222222".to_string();
        manifest
    }

    fn binary_oracle_maker_config_toml() -> String {
        r#"
        strategy_id = "binary_oracle_maker-backtest-001"
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

    fn binary_oracle_maker_manifest() -> BacktestingRunManifest {
        let mut manifest = valid_manifest();
        manifest.market_structure_fixture = MarketStructureFixture::BinaryOption;
        manifest.strategy.registry_key = STRATEGY_BINARY_ORACLE_MAKER.to_string();
        manifest.strategy.parameters = BTreeMap::from([
            (
                STRATEGY_PARAM_CONFIG_TOML.to_string(),
                binary_oracle_maker_config_toml(),
            ),
            (STRATEGY_PARAM_FEE_BPS.to_string(), "0".to_string()),
            (
                STRATEGY_PARAM_ORDER_EXECUTION_MODE.to_string(),
                "shadow".to_string(),
            ),
        ]);
        manifest.strategy_config_hash =
            "3333333333333333333333333333333333333333333333333333333333333333".to_string();
        manifest
    }

    #[test]
    fn binary_oracle_accepts_production_config_overlay_without_inline_config_toml() {
        let manifest = binary_oracle_overlay_manifest();

        manifest
            .validate(&accepted_dataset())
            .expect("binary-oracle production config overlay should validate");
    }

    #[test]
    fn binary_oracle_rejects_config_overlay_plus_inline_config_toml() {
        let mut manifest = binary_oracle_overlay_manifest();
        manifest
            .strategy
            .parameters
            .insert(STRATEGY_PARAM_CONFIG_TOML.to_string(), String::new());

        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidStrategyConfigOverlay {
                field: "strategy.config_overlay",
                ..
            }
        ));
    }

    #[test]
    fn binary_oracle_maker_accepts_inline_config_toml() {
        let manifest = binary_oracle_maker_manifest();

        manifest
            .validate(&binary_option_accepted_dataset())
            .expect("binary-oracle maker inline config should validate");
    }

    #[test]
    fn resolved_nt_surfaces_record_unsupported_catalog_query_mappings() {
        let manifest = valid_manifest();
        let surfaces = manifest.resolved_nt_surfaces().expect("resolved surfaces");

        for (surface, _, nt_field) in UNSUPPORTED_NT_CATALOG_QUERY_SURFACES {
            assert!(
                surfaces.iter().any(|resolved| {
                    resolved.surface == *surface
                        && resolved.classification == NtSurfaceClassification::UnsupportedForNow
                        && resolved.nt_field == *nt_field
                        && resolved.resolved_value == "requests_rejected_before_nt_config"
                }),
                "missing unsupported catalog query NT surface {surface}"
            );
        }
    }

    #[test]
    fn typed_supported_nt_catalog_query_surfaces_map_to_backtest_data_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].start_time = Some(1_772_323_200_000_000_000);
        manifest.catalog_inputs[0].end_time = Some(1_772_409_600_000_000_000);
        manifest.catalog_inputs[0].filter_expr = Some("price > 0".to_string());
        manifest.catalog_inputs[0].client_id = Some("TEST-CLIENT".to_string());
        manifest.catalog_inputs[0].optimize_file_loading = Some(true);

        let data = manifest
            .to_nt_data_config()
            .expect("supported catalog query fields map to NT data config");

        assert_eq!(
            data.start_time(),
            Some(UnixNanos::from(1_772_323_200_000_000_000))
        );
        assert_eq!(
            data.end_time(),
            Some(UnixNanos::from(1_772_409_600_000_000_000))
        );
        assert_eq!(data.filter_expr(), Some("price > 0"));
        assert_eq!(data.client_id(), Some(ClientId::from("TEST-CLIENT")));
        assert!(data.optimize_file_loading());
    }

    #[test]
    fn rejects_inline_strategy_code() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "fn on_trade(&mut self) { submit(); }".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InlineStrategyCode { .. }
        ));
    }

    #[test]
    fn rejects_python_strategy_path() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "strategies/my_strategy.py".to_string();
        // Filesystem path is caught first as a Python path.
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::PythonStrategyPath { .. }
        ));
    }

    #[test]
    fn rejects_untracked_config_blob() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "../untracked/config.toml".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UntrackedConfigBlob { .. }
        ));
    }

    #[test]
    fn accepts_human_typed_strategy_config_with_artifact_hash() {
        let mut manifest = valid_manifest();
        manifest.strategy.source_kind = StrategySourceKind::HumanTypedConfig;
        manifest.strategy.typed_config_uri = Some(
            "s3://bolt-parquet/nt-research-analytics/backtests/strategy-configs/hurst-vpin.toml"
                .to_string(),
        );
        manifest.strategy.typed_config_hash =
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());

        manifest
            .validate(&accepted_dataset())
            .expect("human typed config provenance should validate");
    }

    #[test]
    fn accepts_research_analytics_experiment_result_strategy_config() {
        let mut manifest = valid_manifest();
        manifest.strategy.source_kind = StrategySourceKind::ResearchAnalyticsExperimentResult;
        manifest.strategy.typed_config_uri = Some(
            "s3://bolt-parquet/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/runtime-config.toml"
                .to_string(),
        );
        manifest.strategy.typed_config_hash =
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string());
        manifest.strategy.experiment_result_uri = Some(
            "s3://bolt-parquet/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/experiment-result.json"
                .to_string(),
        );
        manifest.strategy.experiment_result_hash =
            Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string());

        manifest
            .validate(&accepted_dataset())
            .expect("RA experiment-result strategy config should validate");
    }

    #[test]
    fn rejects_research_analytics_strategy_config_without_experiment_result_ref() {
        let mut manifest = valid_manifest();
        manifest.strategy.source_kind = StrategySourceKind::ResearchAnalyticsExperimentResult;
        manifest.strategy.typed_config_uri = Some(
            "s3://bolt-parquet/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/runtime-config.toml"
                .to_string(),
        );
        manifest.strategy.typed_config_hash =
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string());

        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.experiment_result_uri")
        ));
    }

    #[test]
    fn rejects_research_analytics_strategy_config_outside_experiment_results_family() {
        let mut manifest = valid_manifest();
        manifest.strategy.source_kind = StrategySourceKind::ResearchAnalyticsExperimentResult;
        manifest.strategy.typed_config_uri = Some(
            "s3://bolt-parquet/nt-research-analytics/backtests/package-123/runtime-config.toml"
                .to_string(),
        );
        manifest.strategy.typed_config_hash =
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string());
        manifest.strategy.experiment_result_uri = Some(
            "s3://bolt-parquet/nt-research-analytics/research-analytics/v1/experiment-results/experiment-123/experiment-result.json"
                .to_string(),
        );
        manifest.strategy.experiment_result_hash =
            Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string());

        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::StrategySourceOutsideAllowedArtifactRoot { .. }
        ));
    }

    #[test]
    fn rejects_notebook_runtime_strategy_source() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "research/notebook.ipynb".to_string();

        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NotebookRuntimeStrategy { .. }
        ));
    }

    #[test]
    fn rejects_unknown_strategy() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "nonexistent_strategy".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnknownStrategy { .. }
        ));
    }

    #[test]
    fn rejects_invalid_strategy_registry_key_syntax() {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = "use process Command".to_string();
        let err = manifest.validate(&accepted_dataset()).unwrap_err();
        assert!(err.to_string().contains("registry key"), "{err}");
    }

    #[test]
    fn rejects_unknown_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest
            .strategy
            .parameters
            .insert("unknown_blob".to_string(), "x".to_string());
        let err = manifest.validate(&accepted_dataset()).unwrap_err();
        assert!(err.to_string().contains("parameter"), "{err}");
    }

    #[test]
    fn rejects_missing_required_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest
            .strategy
            .parameters
            .remove(STRATEGY_PARAM_TRADE_SIZE);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.trade_size")
        );
    }

    #[test]
    fn rejects_invalid_trade_size_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_TRADE_SIZE.to_string(),
            "not-a-size".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.trade_size")
        );
    }

    #[test]
    fn rejects_invalid_bar_type_strategy_parameter() {
        let mut manifest = valid_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_BAR_TYPE.to_string(),
            "not-a-bar-type".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.bar_type")
        );
    }

    fn probe_manifest() -> BacktestingRunManifest {
        let mut manifest = valid_manifest();
        manifest.strategy.registry_key = STRATEGY_MECHANICAL_TRADE_REPLAY_PROBE.to_string();
        manifest.strategy.parameters = BTreeMap::from([
            (STRATEGY_PARAM_TRADE_SIZE.to_string(), "0.01".to_string()),
            (
                STRATEGY_PARAM_ENTRY_AFTER_TRADES.to_string(),
                "1".to_string(),
            ),
            (
                STRATEGY_PARAM_EXIT_AFTER_TRADES.to_string(),
                "1".to_string(),
            ),
            (STRATEGY_PARAM_SIDE.to_string(), "buy".to_string()),
        ]);
        manifest
    }

    #[test]
    fn mechanical_trade_replay_probe_accepts_valid_params() {
        let manifest = probe_manifest();
        manifest
            .validate(&accepted_dataset())
            .expect("probe manifest with valid params must validate");
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_missing_threshold() {
        let mut manifest = probe_manifest();
        manifest
            .strategy
            .parameters
            .remove(STRATEGY_PARAM_EXIT_AFTER_TRADES);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.exit_after_trades")
        );
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_non_integer_threshold() {
        let mut manifest = probe_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_ENTRY_AFTER_TRADES.to_string(),
            "not-an-integer".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("strategy.parameters.entry_after_trades")
        );
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_zero_entry_after_trades() {
        let mut manifest = probe_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_ENTRY_AFTER_TRADES.to_string(),
            "0".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "strategy.parameters.entry_after_trades",
                value: "0".to_string(),
            }
        );
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_zero_exit_after_trades() {
        let mut manifest = probe_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_EXIT_AFTER_TRADES.to_string(),
            "0".to_string(),
        );
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "strategy.parameters.exit_after_trades",
                value: "0".to_string(),
            }
        );
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_zero_trade_size() {
        let mut manifest = probe_manifest();
        manifest
            .strategy
            .parameters
            .insert(STRATEGY_PARAM_TRADE_SIZE.to_string(), "0".to_string());
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "strategy.parameters.trade_size",
                value: "0".to_string(),
            }
        );
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_invalid_side() {
        let mut manifest = probe_manifest();
        manifest
            .strategy
            .parameters
            .insert(STRATEGY_PARAM_SIDE.to_string(), "sideways".to_string());
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "strategy.parameters.side",
                value: "sideways".to_string(),
            }
        );
    }

    #[test]
    fn mechanical_trade_replay_probe_rejects_stray_bar_type_param() {
        let mut manifest = probe_manifest();
        manifest.strategy.parameters.insert(
            STRATEGY_PARAM_BAR_TYPE.to_string(),
            TEST_BAR_TYPE.to_string(),
        );
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnknownStrategyParameter { .. }
        ));
    }

    #[test]
    fn rejects_unaccepted_data() {
        let mut manifest = valid_manifest();
        manifest.source_proof_id = "some-other-proof".to_string();
        assert!(matches!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnacceptedData { .. }
        ));
    }

    #[test]
    fn rejects_source_binding_mismatch() {
        let mut manifest = valid_manifest();
        manifest.venue_binding_key = "some-other-binding".to_string();
        let err = manifest.validate(&accepted_dataset()).unwrap_err();
        assert!(err.to_string().contains("binding"), "{err}");
    }

    #[test]
    fn rejects_fixture_mismatch() {
        let mut manifest = valid_manifest();
        manifest.market_structure_fixture = MarketStructureFixture::BinaryOption;
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::FixtureMismatch {
                manifest_fixture: MarketStructureFixture::BinaryOption,
                accepted_fixture: FixtureType::PerpsSpot,
            }
        );
    }

    #[test]
    fn binary_option_fixture_accepts_current_source_proof_fixture_type() {
        let mut manifest = valid_manifest();
        manifest.market_structure_fixture = MarketStructureFixture::BinaryOption;
        let mut accepted = accepted_dataset();
        accepted.fixture_type = FixtureType::BinaryOption;

        manifest
            .validate(&accepted)
            .expect("binary-option manifest accepts binary-option proof");
    }

    #[test]
    fn rejects_non_latest_proof_pin_for_normal_run() {
        let mut manifest = valid_manifest();
        manifest.pins_non_latest_proof = true;
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NonLatestProofPinForNormalRun
        );
    }

    #[test]
    fn rejects_non_latest_proof_pin_without_reason_code() {
        let mut manifest = valid_manifest();
        manifest.run_purpose = RunPurpose::Audit;
        manifest.pins_non_latest_proof = true;

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("proof_pin_reason_code")
        );
    }

    #[test]
    fn rejects_audit_non_latest_proof_pin_without_reason_detail() {
        let mut manifest = valid_manifest();
        manifest.run_purpose = RunPurpose::Audit;
        manifest.pins_non_latest_proof = true;
        manifest.proof_pin_reason_code = Some(ProofPinReasonCode::AuditOrInvestigation);

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("proof_pin_reason_detail")
        );
    }

    #[test]
    fn accepts_non_latest_reproduction_pin_with_reason_code() {
        let mut manifest = valid_manifest();
        manifest.run_purpose = RunPurpose::Reproduction;
        manifest.pins_non_latest_proof = true;
        manifest.proof_pin_reason_code = Some(ProofPinReasonCode::BaselineReproduction);

        manifest
            .validate(&accepted_dataset())
            .expect("reproduction pin with structured reason should validate");
    }

    #[test]
    fn accepts_all_configured_non_latest_proof_pin_reason_codes_from_toml() {
        for (run_purpose, reason_code) in [
            ("reproduction", "published_result_reproduction"),
            ("regression", "regression_comparison"),
        ] {
            let toml = toml::to_string(&valid_manifest())
                .expect("serialize manifest")
                .replace(
                    "run_purpose = \"normal\"",
                    &format!("run_purpose = \"{run_purpose}\""),
                )
                .replace(
                    "pins_non_latest_proof = false",
                    &format!(
                        "pins_non_latest_proof = true\nproof_pin_reason_code = \"{reason_code}\""
                    ),
                );
            let manifest: BacktestingRunManifest =
                toml::from_str(&toml).expect("parse allowed proof-pin reason code");

            manifest
                .validate(&accepted_dataset())
                .expect("allowed proof-pin reason code should validate");
        }
    }

    #[test]
    fn rejects_output_prefix_outside_artifact_root() {
        let mut manifest = valid_manifest();
        manifest.output_prefix = "s3://other-bucket/backtests/x".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::OutputPrefixOutsideArtifactRoot
        );
    }

    #[test]
    fn artifact_root_resolves_typed_subpaths_without_extra_root_knobs() {
        let manifest = valid_manifest();
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::Raw)
                .expect("raw subpath"),
            "s3://bolt-parquet/nt-research-analytics/raw"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::NtCatalog)
                .expect("nt catalog subpath"),
            "s3://bolt-parquet/nt-research-analytics/nt-catalog"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::SourceProofs)
                .expect("source proof subpath"),
            "s3://bolt-parquet/nt-research-analytics/source-proofs"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::Backtests)
                .expect("backtest subpath"),
            "s3://bolt-parquet/nt-research-analytics/backtests"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::ArtifactIndex)
                .expect("artifact index subpath"),
            "s3://bolt-parquet/nt-research-analytics/artifact-index"
        );
        assert_eq!(
            manifest
                .artifact_subpath_uri(ArtifactSubpath::ResearchAnalytics)
                .expect("research analytics subpath"),
            "s3://bolt-parquet/nt-research-analytics/research-analytics"
        );
    }

    #[test]
    fn rejects_unsupported_artifact_root_scheme() {
        let mut manifest = valid_manifest();
        manifest.artifact_root = "http://example.test/nt-research-analytics".to_string();
        manifest.output_prefix =
            "http://example.test/nt-research-analytics/backtests/run".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "artifact_root",
                value: "http".to_string(),
            }
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let manifest = valid_manifest();
        let text = toml::to_string(&manifest).expect("serialize");
        let parsed = parse_manifest_toml(&text).expect("parse");
        assert_eq!(parsed, manifest);
    }

    #[test]
    fn rejects_negative_start_time() {
        let mut manifest = valid_manifest();
        manifest.start_time = Some(-1);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NegativeTime {
                field: "start_time",
                value: -1,
            }
        );
    }

    #[test]
    fn rejects_negative_end_time() {
        let mut manifest = valid_manifest();
        manifest.end_time = Some(-42);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::NegativeTime {
                field: "end_time",
                value: -42,
            }
        );
    }

    #[test]
    fn rejects_start_after_end() {
        let mut manifest = valid_manifest();
        manifest.start_time = Some(200);
        manifest.end_time = Some(100);
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvertedTimeWindow {
                start: 200,
                end: 100,
            }
        );
    }

    #[test]
    fn rejects_unsupported_data_type() {
        // Two rejection paths must both surface UnsupportedDataType with the
        // original string payload:
        //   1. NT-known but bolt-unadmitted: OrderBookDepth10 is a valid
        //      NautilusDataType variant but has no ADMITTANCE_TABLE row.
        //   2. Pure junk: a string NT's FromStr rejects outright.
        let mut nt_known = valid_manifest();
        nt_known.catalog_inputs[0].data_type = "OrderBookDepth10".to_string();
        assert_eq!(
            nt_known.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedDataType {
                data_type: "OrderBookDepth10".to_string(),
            }
        );

        let mut junk = valid_manifest();
        junk.catalog_inputs[0].data_type = "Nonsense".to_string();
        assert_eq!(
            junk.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedDataType {
                data_type: "Nonsense".to_string(),
            }
        );
    }

    #[test]
    fn quote_replay_admits_quote_tick_data_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "QuoteTick".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::QuoteReplay;

        manifest
            .validate(&accepted)
            .expect("QuoteReplay source proof should allow QuoteTick catalog input");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(data.data_type(), NautilusDataType::QuoteTick);
    }

    #[test]
    fn index_replay_admits_index_price_update() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "IndexPriceUpdate".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::IndexReplay;

        manifest
            .validate(&accepted)
            .expect("IndexReplay source proof should allow IndexPriceUpdate catalog input");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(data.data_type(), NautilusDataType::IndexPriceUpdate);
    }

    #[test]
    fn mark_replay_admits_mark_price_update() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "MarkPriceUpdate".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::MarkReplay;

        manifest
            .validate(&accepted)
            .expect("MarkReplay source proof should allow MarkPriceUpdate catalog input");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(data.data_type(), NautilusDataType::MarkPriceUpdate);
    }

    #[test]
    fn funding_replay_admits_funding_rate_update() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "FundingRateUpdate".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::FundingReplay;

        manifest
            .validate(&accepted)
            .expect("FundingReplay source proof should allow FundingRateUpdate catalog input");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(data.data_type(), NautilusDataType::FundingRateUpdate);
    }

    #[test]
    fn quote_tick_rejected_under_trade_replay() {
        // The table must not over-admit across fidelity classes: a QuoteTick
        // input under the default TradeReplay proof has no admittance row.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "QuoteTick".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "QuoteTick".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeReplay,
            }
        );
    }

    #[test]
    fn index_price_update_rejected_under_trade_replay() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "IndexPriceUpdate".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "IndexPriceUpdate".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeReplay,
            }
        );
    }

    #[test]
    fn mark_price_update_rejected_under_trade_replay() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "MarkPriceUpdate".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "MarkPriceUpdate".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeReplay,
            }
        );
    }

    #[test]
    fn funding_rate_update_rejected_under_trade_replay() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "FundingRateUpdate".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "FundingRateUpdate".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeReplay,
            }
        );
    }

    #[test]
    fn quote_replay_rejects_when_no_quote() {
        // The must-have-primary check must still fire for the new classes: a
        // QuoteReplay proof carrying only a TradeTick input must fail closed.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "TradeTick".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::QuoteReplay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "TradeTick".to_string(),
                fidelity_class: SourceProofFidelityClass::QuoteReplay,
            }
        );
    }

    #[test]
    fn funding_replay_rejects_when_no_funding_rate_update() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "TradeTick".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::FundingReplay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "TradeTick".to_string(),
                fidelity_class: SourceProofFidelityClass::FundingReplay,
            }
        );
    }

    #[test]
    fn l2_replay_accepts_order_book_delta_data_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        manifest.venue.book_type = "L2_MBP".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        manifest
            .validate(&accepted)
            .expect("L2Replay source proof should allow OrderBookDelta catalog input");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(data.data_type(), NautilusDataType::OrderBookDelta);
    }

    #[test]
    fn order_book_delta_input_rejected_under_l3_mbo_book_type() {
        // bolt converters emit L2 (F_LAST) deltas with order_id 0; under
        // BookType::L3_MBO every level change would collapse onto a single
        // phantom order and silently corrupt the book. Pairing an
        // OrderBookDelta input with L3_MBO must fail loud at manifest validation.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        manifest.venue.book_type = "L3_MBO".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::OrderBookDeltaRequiresL2Mbp {
                book_type: "L3_MBO".to_string(),
            }
        );
    }

    #[test]
    fn order_book_delta_input_rejected_under_l1_mbp_book_type() {
        // Any non-L2 book type (here the default L1_MBP top-of-book) is
        // incompatible with full-depth L2 delta inputs and must fail loud.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        // valid_manifest() defaults book_type to L1_MBP; leave it as-is (not L2_MBP).
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::OrderBookDeltaRequiresL2Mbp {
                book_type: "L1_MBP".to_string(),
            }
        );
    }

    #[test]
    fn order_book_delta_input_accepted_under_l2_mbp_book_type() {
        // The single admissible pairing: an OrderBookDelta input with the
        // L2_MBP book type passes the fail-loud fence.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        manifest.venue.book_type = "L2_MBP".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        manifest
            .validate(&accepted)
            .expect("OrderBookDelta under L2_MBP must pass the book-type fence");
    }

    #[test]
    fn trade_bar_replay_accepts_bar_data_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "Bar".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::TradeBarReplay;

        manifest
            .validate(&accepted)
            .expect("TradeBarReplay source proof should allow Bar catalog input");
        let data = manifest.to_nt_data_config().expect("data config");

        assert_eq!(data.data_type(), NautilusDataType::Bar);
    }

    #[test]
    fn bar_data_type_rejected_under_trade_replay() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "Bar".to_string();
        // accepted_dataset() is TradeReplay; a Bar input must not be admissible.
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "Bar".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeReplay,
            }
        );
    }

    #[test]
    fn trade_bar_replay_rejects_when_no_bar() {
        // A TradeBarReplay source proof requires at least one Bar input; a
        // manifest carrying only TradeTick inputs must fail closed.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "TradeTick".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::TradeBarReplay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "TradeTick".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            }
        );
    }

    #[test]
    fn trade_bar_replay_rejects_mixed_input_containing_order_book_delta() {
        // The per-input guard loop inside ensure_catalog_inputs_match_fidelity
        // must reject any non-{Bar,TradeTick,InstrumentStatus,InstrumentClose}
        // entry even when the mandatory Bar is present.  Previously the only
        // tested path was "no Bar at all" (trade_bar_replay_rejects_when_no_bar);
        // that test cannot exercise the per-input loop.  This test supplies:
        //   input[0] = Bar         (valid, satisfies the "must have Bar" check)
        //   input[1] = OrderBookDelta (invalid under TradeBarReplay)
        // and asserts that the second input triggers the DataTypeFidelityMismatch
        // on "OrderBookDelta", not on "Bar".
        let mut manifest = valid_manifest();
        let bar_input = ManifestCatalogInput {
            data_type: "Bar".to_string(),
            ..manifest.catalog_inputs[0].clone()
        };
        let delta_input = ManifestCatalogInput {
            data_type: "OrderBookDelta".to_string(),
            ..manifest.catalog_inputs[0].clone()
        };
        manifest.catalog_inputs = vec![bar_input, delta_input];
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::TradeBarReplay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::DataTypeFidelityMismatch {
                data_type: "OrderBookDelta".to_string(),
                fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            }
        );
    }

    #[test]
    fn data_config_maps_configured_multi_instrument_ids() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        manifest.catalog_inputs[0].instrument_ids = Some(vec![
            "YES.TESTVENUE".to_string(),
            "NO.TESTVENUE".to_string(),
        ]);
        manifest.venue.book_type = "L2_MBP".to_string();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        manifest
            .validate(&accepted)
            .expect("configured instrument_ids should be supported for L2Replay");
        let data = manifest.to_nt_data_config().expect("data config");

        assert!(data.instrument_id().is_none());
        let instrument_ids = data.instrument_ids().expect("instrument ids");
        assert_eq!(instrument_ids.len(), 2);
        assert_eq!(instrument_ids[0].to_string(), "YES.TESTVENUE");
        assert_eq!(instrument_ids[1].to_string(), "NO.TESTVENUE");
    }

    #[test]
    fn l2_replay_manifest_rejects_multiple_catalog_inputs_for_pinned_nt_streaming() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        manifest.venue.book_type = "L2_MBP".to_string();
        let mut trade_input = manifest.catalog_inputs[0].clone();
        trade_input.data_type = "TradeTick".to_string();
        manifest.catalog_inputs.push(trade_input);
        let text = toml::to_string(&manifest).expect("serialize plural manifest");
        let parsed = parse_manifest_toml(&text).expect("parse plural catalog inputs");
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        assert_eq!(
            parsed.validate(&accepted).unwrap_err(),
            ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                message: "pinned NautilusTrader streaming materializes all data for 2 inputs; exactly one catalog input is required".to_string(),
            }
        );
    }

    #[test]
    fn l2_replay_manifest_rejects_auxiliary_catalog_inputs_for_pinned_nt_streaming() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "OrderBookDelta".to_string();
        manifest.venue.book_type = "L2_MBP".to_string();
        let mut status_input = manifest.catalog_inputs[0].clone();
        status_input.data_type = "InstrumentStatus".to_string();
        let mut close_input = manifest.catalog_inputs[0].clone();
        close_input.data_type = "InstrumentClose".to_string();
        manifest.catalog_inputs.push(status_input);
        manifest.catalog_inputs.push(close_input);
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;

        assert_eq!(
            manifest.validate(&accepted).unwrap_err(),
            ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                message: "pinned NautilusTrader streaming materializes all data for 3 inputs; exactly one catalog input is required".to_string(),
            }
        );
    }

    #[test]
    fn trade_replay_manifest_rejects_auxiliary_catalog_inputs_for_pinned_nt_streaming() {
        let mut manifest = valid_manifest();
        let mut status_input = manifest.catalog_inputs[0].clone();
        status_input.data_type = "InstrumentStatus".to_string();
        let mut close_input = manifest.catalog_inputs[0].clone();
        close_input.data_type = "InstrumentClose".to_string();
        manifest.catalog_inputs.push(status_input);
        manifest.catalog_inputs.push(close_input);

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                message: "pinned NautilusTrader streaming materializes all data for 3 inputs; exactly one catalog input is required".to_string(),
            }
        );
    }

    #[test]
    fn trade_replay_rejects_auxiliary_catalog_inputs_without_trade_ticks() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "InstrumentStatus".to_string();
        let mut close_input = manifest.catalog_inputs[0].clone();
        close_input.data_type = "InstrumentClose".to_string();
        manifest.catalog_inputs.push(close_input);

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidNtConfig {
                field: "catalog_inputs",
                message: "pinned NautilusTrader streaming materializes all data for 2 inputs; exactly one catalog input is required".to_string(),
            }
        );
    }

    #[test]
    fn rejects_unsupported_catalog_fs_protocol() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_fs_protocol = "ftp".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "catalog_inputs.catalog_fs_protocol",
                value: "ftp".to_string(),
            }
        );
    }

    #[test]
    fn rejects_shadowed_catalog_storage_options_before_nt_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_storage_options =
            BTreeMap::from([("region".to_string(), "us-east-1".to_string())]);
        manifest.catalog_inputs[0].catalog_fs_rust_storage_options =
            BTreeMap::from([("allow_http".to_string(), "false".to_string())]);

        let expected = ManifestError::UnsupportedEnum {
            field: "catalog_inputs.catalog_fs_storage_options",
            value: "cannot be combined with catalog_fs_rust_storage_options".to_string(),
        };
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            expected
        );
        assert_eq!(manifest.to_nt_data_config().unwrap_err(), expected);
    }

    #[test]
    fn rejects_unknown_s3_catalog_rust_storage_option_before_nt_config() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].catalog_path =
            "bolt-parquet/nt-research-analytics/backtests/run/nt-catalog".to_string();
        manifest.catalog_inputs[0].catalog_fs_protocol = "s3".to_string();
        manifest.catalog_inputs[0].catalog_fs_rust_storage_options = BTreeMap::from([(
            "aws_virtual_hosted_style_request".to_string(),
            "false".to_string(),
        )]);

        let expected = ManifestError::UnsupportedEnum {
            field: "catalog_inputs.catalog_fs_rust_storage_options",
            value: "aws_virtual_hosted_style_request".to_string(),
        };
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            expected
        );
        assert_eq!(manifest.to_nt_data_config().unwrap_err(), expected);
    }

    #[test]
    fn rejects_fidelity_data_type_mismatch() {
        let manifest = valid_manifest();
        let mut accepted = accepted_dataset();
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;
        let err = manifest.validate(&accepted).unwrap_err();
        assert!(err.to_string().contains("incompatible"), "{err}");
    }

    #[test]
    fn rejects_invalid_instrument_id_with_specific_error() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].nt_instrument_id = "not-an-instrument-id".to_string();
        assert!(matches!(
            manifest.to_nt_data_config(),
            Err(ManifestError::InvalidInstrumentId { instrument_id })
                if instrument_id == "not-an-instrument-id"
        ));
    }

    #[test]
    fn rejects_unsupported_oms_type() {
        let mut manifest = valid_manifest();
        manifest.venue.oms_type = "INVALID".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.oms_type",
                value: "INVALID".to_string(),
            }
        );
    }

    #[test]
    fn rejects_unsupported_oto_trigger_mode() {
        let mut manifest = valid_manifest();
        manifest.venue.oto_trigger_mode = "INVALID".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.oto_trigger_mode",
                value: "INVALID".to_string(),
            }
        );
    }

    #[test]
    fn rejects_invalid_base_currency() {
        let mut manifest = valid_manifest();
        manifest.venue.base_currency = "NOT_A_CURRENCY".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidBaseCurrency {
                currency: "NOT_A_CURRENCY".to_string(),
            }
        );
    }

    #[test]
    fn rejects_non_positive_default_leverage() {
        let mut manifest = valid_manifest();
        manifest.venue.default_leverage = "0".to_string();
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidDefaultLeverage {
                leverage: "0".to_string(),
            }
        );
    }

    #[test]
    fn rejects_malformed_manifest_toml() {
        assert!(parse_manifest_toml("this is not = valid = toml").is_err());
    }

    #[test]
    fn rejects_unknown_top_level_manifest_toml_fields() {
        let text = format!(
            "unknown_blob = \"x\"\n{}",
            toml::to_string(&valid_manifest()).expect("serialize")
        );
        assert!(parse_manifest_toml(&text).is_err());
    }

    #[test]
    fn rejects_unknown_nested_manifest_toml_fields() {
        let text = toml::to_string(&valid_manifest())
            .expect("serialize")
            .replace("[strategy]\n", "[strategy]\nunknown_blob = \"x\"\n");
        assert!(parse_manifest_toml(&text).is_err());
    }

    #[test]
    fn rejects_unsupported_nt_venue_model_surface_requests_before_nt_config() {
        let serialized = toml::to_string(&valid_manifest()).expect("serialize");
        for (field, value) in [
            ("leverages", "{}"),
            ("margin_model", "\"standard\""),
            ("modules", "[]"),
            ("settlement_prices", "{}"),
        ] {
            let text = serialized.replace("[venue]\n", &format!("[venue]\n{field} = {value}\n"));
            let manifest = parse_manifest_toml(&text)
                .expect("unsupported NT venue surface should be represented in schema");
            let err = manifest
                .validate(&accepted_dataset())
                .expect_err("unsupported NT venue surface must fail validation");
            assert!(
                matches!(err, ManifestError::UnsupportedNtSurface { field: actual } if actual == field),
                "unsupported venue surface {field:?} must fail fast, got {err}"
            );
        }
    }

    #[test]
    fn typed_unsupported_nt_venue_model_surfaces_parse_then_fail_before_nt_config() {
        let serialized = toml::to_string(&valid_manifest()).expect("serialize");
        let leverages = format!("{{ \"{TEST_INSTRUMENT_ID}\" = \"2\" }}");
        let settlement_prices = format!("{{ \"{TEST_INSTRUMENT_ID}\" = \"65000\" }}");
        for (field, value) in [
            ("leverages", leverages.as_str()),
            ("margin_model", "\"standard\""),
            ("modules", "[\"latency-probe\"]"),
            ("settlement_prices", settlement_prices.as_str()),
        ] {
            let text = serialized.replace("[venue]\n", &format!("[venue]\n{field} = {value}\n"));
            let manifest = parse_manifest_toml(&text)
                .expect("unsupported NT venue surface should be represented in schema");
            let err = manifest
                .to_nt_venue_config()
                .expect_err("unsupported NT venue surface must not reach NT config");
            assert!(
                matches!(err, ManifestError::UnsupportedNtSurface { field: actual } if actual == field),
                "unsupported venue surface {field:?} should fail with a structured error, got {err}"
            );
        }
    }

    #[test]
    fn rejects_unknown_polymarket_cost_realism_model_selectors() {
        let mut manifest = valid_manifest();
        manifest.venue.fill_model = Some(ManifestFillModelConfig {
            kind: "unknown-fill".to_string(),
            prob_fill_on_limit: "0.5".to_string(),
            prob_slippage: "0.0".to_string(),
            random_seed: None,
        });
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.fill_model.kind",
                value: "unknown-fill".to_string(),
            }
        );

        let mut manifest = valid_manifest();
        manifest.venue.latency_model = Some(ManifestLatencyModelConfig {
            kind: "unknown-latency".to_string(),
            base_latency_nanos: 0,
            insert_latency_nanos: 0,
            update_latency_nanos: 0,
            delete_latency_nanos: 0,
        });
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.latency_model.kind",
                value: "unknown-latency".to_string(),
            }
        );

        let mut manifest = valid_manifest();
        manifest.venue.fee_model = Some(ManifestFeeModelConfig {
            kind: "unknown-fee".to_string(),
        });
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::UnsupportedEnum {
                field: "venue.fee_model.kind",
                value: "unknown-fee".to_string(),
            }
        );
    }

    #[test]
    fn rejects_invalid_polymarket_cost_realism_parameters() {
        let mut manifest = valid_manifest();
        manifest.venue.fill_model = Some(ManifestFillModelConfig {
            kind: FILL_MODEL_PREDICTION_MARKET_PROBABILISTIC.to_string(),
            prob_fill_on_limit: "1.01".to_string(),
            prob_slippage: "0".to_string(),
            random_seed: None,
        });
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidVenueModelParameter {
                field: "venue.fill_model.prob_fill_on_limit",
                value: "1.01".to_string(),
            }
        );

        let mut manifest = valid_manifest();
        manifest.venue.latency_model = Some(ManifestLatencyModelConfig {
            kind: LATENCY_MODEL_PREDICTION_MARKET_STATIC.to_string(),
            base_latency_nanos: u64::MAX,
            insert_latency_nanos: 1,
            update_latency_nanos: 0,
            delete_latency_nanos: 0,
        });
        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::InvalidVenueModelParameter {
                field: "venue.latency_model.insert_latency_nanos",
                value: "1".to_string(),
            }
        );
    }

    #[test]
    fn rejects_probabilistic_fill_model_without_random_seed() {
        let mut manifest = valid_manifest();
        manifest.venue.fill_model = Some(ManifestFillModelConfig {
            kind: FILL_MODEL_PREDICTION_MARKET_PROBABILISTIC.to_string(),
            prob_fill_on_limit: "0.5".to_string(),
            prob_slippage: "0".to_string(),
            random_seed: None,
        });

        assert_eq!(
            manifest.validate(&accepted_dataset()).unwrap_err(),
            ManifestError::MissingField("venue.fill_model.random_seed")
        );
    }

    #[test]
    fn catalog_input_bar_spec_binds_for_operator_and_maps_to_nt_config() {
        // Present `bar_spec` is the operator catalog-binding surface: it parses,
        // passes validation on a Bar input, maps to an NT data config without a
        // bar filter, and resolves as a bolt-owned surface.
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "Bar".to_string();
        manifest.catalog_inputs[0].bar_spec = Some("1minute".to_string());
        manifest
            .to_nt_data_configs()
            .expect("bar_spec input maps to NT data config");
        let surfaces = manifest.resolved_nt_surfaces().expect("resolved surfaces");
        assert!(
            surfaces.iter().any(|resolved| {
                resolved.surface == "catalog.bar_spec"
                    && resolved.classification == NtSurfaceClassification::CustomOwned
                    && resolved.resolved_value == "operator_catalog_binding:1minute"
            }),
            "present bar_spec must resolve as the bolt-owned binding surface"
        );
    }

    #[test]
    fn catalog_input_bar_spec_on_non_bar_input_is_rejected() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].bar_spec = Some("1minute".to_string());
        let err = manifest
            .to_nt_data_configs()
            .expect_err("bar_spec on a TradeTick input must be rejected");
        assert!(
            matches!(
                &err,
                ManifestError::UnsupportedEnum { field, .. } if *field == "catalog_inputs.bar_spec"
            ),
            "unexpected error {err}"
        );
    }

    #[test]
    fn catalog_input_blank_bar_spec_is_rejected() {
        let mut manifest = valid_manifest();
        manifest.catalog_inputs[0].data_type = "Bar".to_string();
        manifest.catalog_inputs[0].bar_spec = Some(" ".to_string());
        let err = manifest
            .to_nt_data_configs()
            .expect_err("blank bar_spec must be rejected");
        assert!(
            matches!(
                &err,
                ManifestError::UnsupportedEnum { field, .. } if *field == "catalog_inputs.bar_spec"
            ),
            "unexpected error {err}"
        );
    }

    #[test]
    fn typed_unsupported_nt_catalog_query_surfaces_parse_then_fail_before_nt_config() {
        let serialized = toml::to_string(&valid_manifest()).expect("serialize");
        let bar_types = format!("[\"{TEST_BAR_TYPE}\"]");
        for (field, value) in [
            ("metadata", "{ source = \"proof\" }"),
            ("bar_types", bar_types.as_str()),
        ] {
            let text = serialized.replace(
                "[[catalog_inputs]]\n",
                &format!("[[catalog_inputs]]\n{field} = {value}\n"),
            );
            let manifest = parse_manifest_toml(&text)
                .expect("unsupported NT catalog query surface should be represented in schema");
            let err = manifest
                .to_nt_data_config()
                .expect_err("unsupported NT catalog query surface must not reach NT config");
            let expected = format!("catalog_inputs.{field}");
            assert!(
                matches!(err, ManifestError::UnsupportedNtSurface { field: actual } if actual == expected),
                "unsupported catalog query surface {field:?} should fail with a structured error, got {err}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_nt_engine_surface_requests_before_nt_config() {
        let text = format!(
            "[engine]\nrisk_engine = {{ bypass = true }}\n{}",
            toml::to_string(&valid_manifest()).expect("serialize")
        );
        let err = parse_manifest_toml(&text).unwrap_err().to_string();
        assert!(
            err.contains("engine"),
            "unsupported engine surface must fail fast, got {err}"
        );
    }
}
