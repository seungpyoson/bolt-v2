//! Bolt-v3 root and strategy TOML configuration types and loading.
//!
//! Schema: docs/bolt-v3/2026-04-25-bolt-v3-schema.md
//! Runtime contracts: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
//!
//! This module is intentionally a no-trade boundary. It only parses and
//! validates configuration; it does not register strategies, build venue
//! adapters, perform market selection, or construct orders.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use nautilus_common::enums::{Environment, LogLevel};
use nautilus_model::{
    enums::{OmsType, OrderType, TimeInForce},
    identifiers::{AccountId, ClientId, InstrumentId, TraderId, Venue},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_iv::config::IvRootConfig,
    bolt_v3_loss_halt_actions::{LossGovernorRecoveryMode, LossGovernorTradingStateAction},
    bolt_v3_outcome_group_sources::{BasketExecutionRiskBlock, OutcomeGroupSourceConfig},
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolCoarserGridPolicy, RealizedVolEngineConfig,
        RealizedVolEstimatorConfig, RealizedVolJumpConfig, RealizedVolJumpPolicy,
        RealizedVolNoiseConfig, RealizedVolNoiseMethod, RealizedVolPricingComponent,
        RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceConfig,
    },
    bolt_v3_validate::{BoltV3ValidationError, validate_root_only, validate_strategies},
};

pub const TEST_DOUBLE_PROVIDER_KIND: &str = "test_double";
// The `chainlink_data_streams` provider-kind literal is owned by the provider
// binding (`crate::bolt_v3_providers::chainlink`) and re-exported here under its
// legacy core name, so core config keeps a single import site without declaring
// the provider-key literal itself.
pub use crate::bolt_v3_providers::RESOLUTION_ORACLE_PROVIDER_KIND as CHAINLINK_DATA_STREAMS_PROVIDER_KIND;
pub const NO_RESOLUTION_KIND: &str = "no_resolution";
pub const NO_RESOLUTION_VALUE_KIND: &str = "none";
pub const RESOLUTION_GATE_ROLE: &str = "resolution";
pub const DECISION_REFERENCE_GATE_ROLE: &str = "decision_reference";
pub const PRICE_GATE_VALUE_KIND: &str = "price";
pub const GATE_PROVIDER_KINDS: &[&str] = &[
    CHAINLINK_DATA_STREAMS_PROVIDER_KIND,
    "pyth",
    "exchange_index",
    "venue_native",
    "hyperliquid_hip4",
    "deribit_index",
    "outcome_oracle",
    TEST_DOUBLE_PROVIDER_KIND,
];
pub const GATE_PROVIDER_CAPABILITIES: &[&str] =
    &["resolution_value", "reference_value", "market_metadata"];
pub const GATE_ROLES: &[&str] = &[RESOLUTION_GATE_ROLE, DECISION_REFERENCE_GATE_ROLE];
pub const GATE_VALUE_KINDS: &[&str] = &[
    PRICE_GATE_VALUE_KIND,
    "index",
    "outcome",
    "metadata",
    NO_RESOLUTION_VALUE_KIND,
];
pub const SSM_CREDENTIAL_PARAMETER_FIELD: &str = "ssm_credential_parameter";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoltV3RootConfig {
    pub schema_version: u32,
    pub trader_id: TraderId,
    pub strategy_files: Vec<String>,
    pub runtime: RuntimeBlock,
    pub nautilus: NautilusBlock,
    pub risk: RiskBlock,
    pub logging: LoggingBlock,
    pub persistence: PersistenceBlock,
    pub aws: AwsBlock,
    pub chainlink_data_streams: Option<RootFeedBindingCatalog>,
    pub clients: BTreeMap<String, ClientBlock>,
    pub realized_volatility_surfaces: Option<BTreeMap<String, RealizedVolatilitySurfaceBlock>>,
    pub gate_providers: Option<BTreeMap<String, GateProviderBlock>>,
    pub outcome_group_sources: Option<Vec<OutcomeGroupSourceConfig>>,
    pub iv: Option<IvRootConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RootFeedBindingCatalog {
    pub feed_bindings: Vec<toml::Value>,
}

// `[risk]` owns Bolt-v3 strategy-sizing limits and the explicit
// NautilusTrader live risk-engine fields that affect runtime
// behavior. `default_max_notional_per_order` is enforced by Bolt-v3
// strategy validation and is not automatically expanded into NT's
// per-instrument map; use `nt_max_notional_per_order` for intentional
// NT instrument-level caps. The `nt_*` fields map into
// `LiveRiskEngineConfig`.

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeBlock {
    pub mode: Environment,
    pub order_execution_mode: crate::bolt_v3_order_execution::BoltV3OrderExecutionMode,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NautilusBlock {
    pub load_state: bool,
    pub save_state: bool,
    pub shutdown_on_error: bool,
    pub timeout_connection_secs: u64,
    pub timeout_reconciliation_secs: u64,
    pub data_engine: NautilusDataEngineBlock,
    pub exec_engine: NautilusExecEngineBlock,
    pub timeout_portfolio_secs: u64,
    pub timeout_disconnection_secs: u64,
    pub delay_post_stop_secs: u64,
    pub timeout_shutdown_secs: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NautilusDataEngineBlock {
    pub time_bars_build_with_no_updates: bool,
    pub time_bars_timestamp_on_close: bool,
    pub time_bars_skip_first_non_full_bar: bool,
    pub time_bars_interval_type: String,
    pub time_bars_build_delay: u64,
    pub time_bars_origins: BTreeMap<String, u64>,
    pub validate_data_sequence: bool,
    pub buffer_deltas: bool,
    pub emit_quotes_from_book: bool,
    pub emit_quotes_from_book_depths: bool,
    pub external_clients: Vec<ClientId>,
    pub debug: bool,
    pub qsize: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NautilusExecEngineBlock {
    pub load_cache: bool,
    pub snapshot_orders: bool,
    pub snapshot_positions: bool,
    pub snapshot_positions_interval_secs: u64,
    pub external_clients: Vec<ClientId>,
    pub debug: bool,
    pub reconciliation: bool,
    pub reconciliation_startup_delay_secs: u64,
    pub reconciliation_lookback_mins: u32,
    pub reconciliation_instrument_ids: Vec<String>,
    pub filter_unclaimed_external_orders: bool,
    pub filter_position_reports: bool,
    pub filtered_client_order_ids: Vec<String>,
    pub generate_missing_orders: bool,
    pub inflight_check_interval_ms: u32,
    pub inflight_check_threshold_ms: u32,
    pub inflight_check_retries: u32,
    pub open_check_interval_secs: u64,
    pub open_check_lookback_mins: u32,
    pub open_check_threshold_ms: u32,
    pub open_check_missing_retries: u32,
    pub open_check_open_only: bool,
    pub max_single_order_queries_per_cycle: u32,
    pub single_order_query_delay_ms: u32,
    pub position_check_interval_secs: u64,
    pub position_check_lookback_mins: u32,
    pub position_check_threshold_ms: u32,
    pub position_check_retries: u32,
    pub purge_closed_orders_interval_mins: u32,
    pub purge_closed_orders_buffer_mins: u32,
    pub purge_closed_positions_interval_mins: u32,
    pub purge_closed_positions_buffer_mins: u32,
    pub purge_account_events_interval_mins: u32,
    pub purge_account_events_lookback_mins: u32,
    pub purge_from_database: bool,
    pub own_books_audit_interval_secs: u64,
    pub qsize: u32,
    pub allow_overfills: bool,
    pub manage_own_order_books: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RiskBlock {
    pub default_max_notional_per_order: String,
    pub loss_governor: Option<LossGovernorBlock>,
    pub capital_pools: Option<Vec<CapitalPoolBlock>>,
    pub nautilus: NautilusRiskBlock,
    pub kill_switch: Option<KillSwitchConfigBlock>,
    pub basket_execution: Option<BasketExecutionRiskBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LossGovernorBlock {
    pub enabled: bool,
    pub account_id: AccountId,
    pub max_snapshot_age_ns: u64,
    pub rolling_window_ns: u64,
    pub active_position_pnl_max_entries: Option<usize>,
    pub on_loss_breach_trading_state: Option<LossGovernorTradingStateAction>,
    pub on_untrusted_snapshot_trading_state: Option<LossGovernorTradingStateAction>,
    pub recovery_mode: Option<LossGovernorRecoveryMode>,
    pub manual_recovery_evidence_max_path_bytes: Option<usize>,
    pub max_per_trade_loss: Option<String>,
    pub max_daily_loss: Option<String>,
    pub max_rolling_loss: Option<String>,
    pub max_drawdown: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapitalPoolBlock {
    pub pool_id: String,
    pub venue_id: String,
    pub account_id: AccountId,
    pub collateral_currency: String,
    pub product_kind: String,
    pub enforce_submit_admission: bool,
    pub max_pool_liability: String,
    pub max_snapshot_age_ns: u64,
    pub dedupe_retention_ns: u64,
    pub venue_spendability_source_path: Option<String>,
    pub venue_spendability_source_sha256: Option<String>,
    pub venue_spendability_source_max_bytes: Option<u64>,
    pub prediction_market_binary: Option<PredictionMarketBinaryProductBlock>,
    pub sizing_policy: CapitalPoolSizingPolicyBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PredictionMarketBinaryProductBlock {
    pub yes_instrument_id: InstrumentId,
    pub no_instrument_id: InstrumentId,
    pub collateral_coupled_group_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapitalPoolSizingPolicyBlock {
    pub min_remaining_pool_balance: Option<String>,
    pub fee_slippage: FeeSlippagePolicyBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FeeSlippagePolicyBlock {
    pub max_fee_liability: String,
    pub max_slippage_liability: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NautilusRiskBlock {
    pub max_order_submit_rate: String,
    pub max_order_modify_rate: String,
    pub max_notional_per_order: BTreeMap<String, String>,
    pub debug: bool,
    pub qsize: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KillSwitchConfigBlock {
    pub enabled: bool,
    pub state_path: String,
    pub max_state_file_bytes: u64,
    pub max_utc_daily_realized_loss: String,
    pub flatten_open_positions_on_breach: bool,
    pub action_retry_interval_ms: u64,
    pub action_retry_timeout_ms: u64,
    pub mandatory_proof_max_age_ms: u64,
    pub manual_reset_evidence_max_age_ms: u64,
    pub forced_reduction_policy_sha256: String,
    pub forced_reduction_max_live_order_count: u32,
    pub forced_reduction_max_notional_per_order: String,
    pub authorized_operator_ids: Vec<String>,
    pub account_ids: Vec<String>,
    pub instrument_ids: Vec<String>,
    pub cancel: Option<KillSwitchCancelConfigBlock>,
    pub flatten: Option<KillSwitchFlattenConfigBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KillSwitchCancelConfigBlock {
    pub enabled: bool,
    pub retry_max_attempts: u32,
    pub retry_timeout_ms: u64,
    pub retry_backoff_ms: u64,
    pub source_freshness_max_age_ms: u64,
    pub mandatory_surfaces: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KillSwitchFlattenConfigBlock {
    pub enabled: bool,
    pub retry_max_attempts: u32,
    pub retry_timeout_ms: u64,
    pub retry_backoff_ms: u64,
    pub source_freshness_max_age_ms: u64,
    pub max_position_proof_age_ms: u64,
    pub route_kind: KillSwitchFlattenRouteKindConfig,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
    pub order_type: OrderType,
    pub time_in_force: TimeInForce,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KillSwitchFlattenRouteKindConfig {
    PerStrategyActionPort,
    LiveNodeCommandRouter,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LoggingBlock {
    pub stdout_level: LogLevel,
    pub fileout_level: LogLevel,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PersistenceBlock {
    pub catalog_directory: String,
    pub required_catalog_prefix: Option<String>,
    pub min_free_bytes: Option<u64>,
    pub runtime_capture_start_poll_interval_ms: u64,
    pub decision_evidence: DecisionEvidenceBlock,
    pub streaming: StreamingBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionEvidenceBlock {
    pub order_intents_relative_path: String,
    /// Byte cap applied when the live-node startup driver reads this same
    /// decision-evidence file to recover known submit-reservation metadata
    /// after a restart. The path is owned by this block
    /// (`order_intents_relative_path` via `decision_evidence_path`), so its
    /// read bound lives here too. `None` opts startup reservation recovery
    /// out: the position sizer then fails closed if any open orders exist at
    /// boot (it cannot attribute them without recovered metadata).
    pub recovery_evidence_max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StreamingBlock {
    pub catalog_fs_protocol: CatalogFsProtocol,
    pub flush_interval_ms: u64,
    pub replace_existing: bool,
    pub rotation_kind: RotationKind,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CatalogFsProtocol {
    File,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RotationKind {
    None,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AwsBlock {
    pub region: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct GateProviderBlock {
    pub provider_kind: Option<String>,
    pub capabilities: Option<Vec<String>>,
    pub client_id: Option<ClientId>,
    pub freshness: Option<GateProviderFreshnessBlock>,
    #[serde(flatten)]
    pub provider_config: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateProviderFreshnessBlock {
    pub max_age_ms: Option<u64>,
    pub max_clock_skew_ms: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilitySurfaceBlock {
    pub canonical_base_asset: String,
    pub canonical_quote_asset: String,
    pub policy: RealizedVolatilityPolicyBlock,
    pub estimator: Option<RealizedVolatilityEstimatorBlock>,
    pub sources: Vec<RealizedVolatilitySourceBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilityPolicyBlock {
    pub window_ms: u64,
    pub sampling_interval_ms: u64,
    pub min_ready_sources: usize,
    pub max_source_age_ms: u64,
    pub max_event_receive_lag_ms: u64,
    pub max_inter_sample_gap_ms: u64,
    pub min_coverage_ratio: f64,
    pub max_cross_source_dispersion: f64,
    pub seconds_per_annum: f64,
    pub aggregation: RealizedVolatilityAggregationBlock,
    pub upper_quantile: f64,
    pub trim_fraction: Option<f64>,
    pub guard_weight: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilityAggregationBlock {
    UpperQuantile,
    Median,
    TrimmedMean,
    MedianWithUpperQuantileGuard,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilityEstimatorBlock {
    pub noise_robust_method: Option<RealizedVolatilityNoiseMethodBlock>,
    pub subsamples: Option<usize>,
    pub min_ready_subsamples: Option<usize>,
    pub coarse_sampling_interval_ms: Option<u64>,
    pub coarser_grid_policy: Option<RealizedVolatilityCoarserGridPolicyBlock>,
    pub jump_policy: Option<RealizedVolatilityJumpPolicyBlock>,
    pub pricing_component: Option<RealizedVolatilityPricingComponentBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilityNoiseMethodBlock {
    None,
    CoarserGrid,
    Subsampled,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilityCoarserGridPolicyBlock {
    CoarseOnly,
    MinBaseCoarse,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilityJumpPolicyBlock {
    None,
    Separate,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilityPricingComponentBlock {
    Measured,
    NoiseRobust,
    Continuous,
    Forecast,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RealizedVolatilitySourceBlock {
    pub source_id: String,
    pub data_client_id: ClientId,
    pub instrument_id: InstrumentId,
    pub source_class: RealizedVolatilitySourceClassBlock,
    pub sample_kind: RealizedVolatilitySampleKindBlock,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub canonical_base_asset: String,
    pub canonical_quote_asset: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilitySourceClassBlock {
    SpotQuote,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RealizedVolatilitySampleKindBlock {
    Midpoint,
    Trade,
    Mark,
    Index,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClientBlock {
    pub venue: Venue,
    pub data: Option<toml::Value>,
    pub execution: Option<toml::Value>,
    pub secrets: Option<toml::Value>,
    pub readiness_probe: Option<DataClientReadinessProbeBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DataClientReadinessProbeBlock {
    pub market_data_kind: DataClientReadinessProbeMarketDataKind,
    pub book_type: Option<DataClientReadinessProbeBookType>,
    pub quote_target_source: DataClientReadinessProbeQuoteTargetSource,
    pub max_metadata_quote_targets: Option<usize>,
    pub allow_metadata_target_sampling: Option<bool>,
    /// Minimum number of sampled readiness-probe targets that must produce a
    /// fresh quote/book/trade observation for the probe to pass. When unset the probe
    /// requires every sampled target (strict, fail-closed default). Configuring
    /// a value lets broad metadata universes prove adapter data-path behaviour
    /// without requiring every illiquid or un-streamable sampled instrument to
    /// tick within the configured wait. Must be >= 1 and <= the sampled count.
    /// For a trade chunk-count probe (`market_data_kind = "trade"` with
    /// `quote_target_source = "metadata_response"`) this is `m`: the number of
    /// distinct markets that must produce a trade across the chunk walk for the
    /// probe to pass, and it is required (there is no fixed sample to fall back
    /// on).
    pub min_observed_targets: Option<usize>,
    /// Maximum number of instruments a trade chunk-count probe subscribes to at
    /// once (`n`). The probe walks the venue's full instrument universe in
    /// chunks of this size — never subscribing to more than `chunk_size`
    /// channels concurrently — to stay below the venue's silent delivery
    /// ceiling. Required (and only valid) when `market_data_kind = "trade"`
    /// and `quote_target_source = "metadata_response"`; must be >= 1.
    pub chunk_size: Option<usize>,
    /// How long a trade chunk-count probe watches each chunk for trades before
    /// moving to the next chunk, in seconds. Required (and only valid) when
    /// `market_data_kind = "trade"` and `quote_target_source =
    /// "metadata_response"`; must be >= 1.
    pub chunk_observation_window_seconds: Option<u64>,
    pub quote_targets: Option<BTreeMap<String, DataClientReadinessProbeQuoteTargetBlock>>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataClientReadinessProbeMarketDataKind {
    Quote,
    Book,
    Trade,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataClientReadinessProbeBookType {
    L1Mbp,
    L2Mbp,
    L3Mbo,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataClientReadinessProbeQuoteTargetSource {
    Configured,
    MetadataResponse,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DataClientReadinessProbeQuoteTargetBlock {
    pub instrument_id: InstrumentId,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoltV3StrategyConfig {
    pub schema_version: u32,
    pub strategy_instance_id: String,
    pub strategy_archetype: StrategyArchetypeKey,
    pub order_id_tag: String,
    pub oms_type: OmsType,
    pub use_uuid_client_order_ids: bool,
    pub use_hyphens_in_client_order_ids: bool,
    pub external_order_claims: Vec<String>,
    pub manage_contingent_orders: bool,
    pub manage_gtd_expiry: bool,
    pub manage_stop: bool,
    pub market_exit_interval_ms: u64,
    pub market_exit_max_attempts: u64,
    pub market_exit_reduce_only: Option<bool>,
    pub log_events: bool,
    pub log_commands: bool,
    pub log_rejected_due_post_only_as_warning: bool,
    pub execution_client_id: ClientId,
    /// Raw `[target]` envelope. The strategy envelope keeps the TOML
    /// field name `target` but its Rust type is a generic raw-TOML
    /// container so target-shape fields live in the per-family binding
    /// modules under `crate::bolt_v3_market_families`. Typed
    /// deserialization with `deny_unknown_fields` happens inside the
    /// matching family validator and inside the family planner; the
    /// strategy envelope itself is target-shape-neutral.
    pub target: toml::Value,
    pub realized_volatility_surface_id: Option<String>,
    pub signal_data: BTreeMap<String, DataInstrumentBlock>,
    /// Optional live resolution-strike (price-to-beat) data source. Uses the
    /// data-instrument shape (`data_client_id` + `instrument_id`) but is
    /// a single block rather than a role-keyed map, matching the strategy's
    /// singular `resolution_client_id` / `resolution_instrument_id` runtime
    /// fields. When absent, the live strike simply does not subscribe.
    pub resolution_data: Option<DataInstrumentBlock>,
    pub reference_current_price: Option<ReferencePriceBlock>,
    pub parameters: toml::Value,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct StrategyArchetypeKey(String);

impl StrategyArchetypeKey {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DataInstrumentBlock {
    pub data_client_id: ClientId,
    pub instrument_id: InstrumentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePriceBlock {
    pub asset: String,
    pub source_order: Vec<String>,
    pub min_valid_sources: usize,
    pub selection_policy: ReferencePriceSelectionPolicy,
    pub max_source_age_ms: u64,
    pub max_source_drift_bps: u32,
    pub drift_policy: ReferencePriceDriftPolicy,
    pub stale_policy: ReferencePriceStalePolicy,
    pub sources: BTreeMap<String, ReferencePriceSourceBlock>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferencePriceBlockWire {
    asset: String,
    #[serde(rename = "sources")]
    source_order: Vec<String>,
    min_valid_sources: usize,
    selection_policy: ReferencePriceSelectionPolicy,
    max_source_age_ms: u64,
    max_source_drift_bps: u32,
    drift_policy: ReferencePriceDriftPolicy,
    stale_policy: ReferencePriceStalePolicy,
    #[serde(rename = "source")]
    sources: BTreeMap<String, ReferencePriceSourceBlock>,
}

impl<'de> Deserialize<'de> for ReferencePriceBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ReferencePriceBlockWire::deserialize(deserializer)?;
        Ok(Self {
            asset: wire.asset,
            source_order: wire.source_order,
            min_valid_sources: wire.min_valid_sources,
            selection_policy: wire.selection_policy,
            max_source_age_ms: wire.max_source_age_ms,
            max_source_drift_bps: wire.max_source_drift_bps,
            drift_policy: wire.drift_policy,
            stale_policy: wire.stale_policy,
            sources: wire.sources,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePriceSourceBlock {
    pub provider: ReferencePriceProvider,
    pub enabled: bool,
    pub required: bool,
    pub client_id: ClientId,
    pub instrument_id: Option<String>,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferencePriceSourceBlockWire {
    provider: ReferencePriceProvider,
    enabled: bool,
    required: bool,
    client_id: ClientId,
    instrument_id: Option<String>,
    symbol: Option<String>,
}

impl<'de> Deserialize<'de> for ReferencePriceSourceBlock {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ReferencePriceSourceBlockWire::deserialize(deserializer)?;
        Ok(Self {
            provider: wire.provider,
            enabled: wire.enabled,
            required: wire.required,
            client_id: wire.client_id,
            instrument_id: wire.instrument_id,
            symbol: wire.symbol,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferencePriceProvider(String);

impl ReferencePriceProvider {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.trim().is_empty()
            || value.trim() != value
            || value.chars().any(char::is_whitespace)
        {
            return Err("reference_price provider is invalid".to_string());
        }
        Ok(Self(value))
    }

    pub fn from_serialized(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for ReferencePriceProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferencePriceDriftPolicy {
    Observe,
    Block,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferencePriceSelectionPolicy {
    FirstValidPerInterval,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferencePriceStalePolicy {
    Block,
}

pub fn realized_volatility_engine_config(
    surface_id: &str,
    surface: &RealizedVolatilitySurfaceBlock,
) -> Result<RealizedVolEngineConfig, String> {
    let aggregation = realized_volatility_aggregation(surface)?;
    Ok(RealizedVolEngineConfig {
        surface_id: surface_id.to_string(),
        window_ms: surface.policy.window_ms,
        sampling_interval_ms: surface.policy.sampling_interval_ms,
        min_ready_sources: surface.policy.min_ready_sources,
        max_source_age_ms: surface.policy.max_source_age_ms,
        max_event_receive_lag_ms: surface.policy.max_event_receive_lag_ms,
        max_inter_sample_gap_ms: surface.policy.max_inter_sample_gap_ms,
        min_coverage_ratio: surface.policy.min_coverage_ratio,
        max_cross_source_dispersion: surface.policy.max_cross_source_dispersion,
        seconds_per_annum: surface.policy.seconds_per_annum,
        aggregation,
        estimator: realized_volatility_estimator_config(surface)?,
        sources: surface
            .sources
            .iter()
            .map(|source| RealizedVolSourceConfig {
                source_id: source.source_id.clone(),
                data_client_id: source.data_client_id.to_string(),
                instrument_id: source.instrument_id.to_string(),
                source_class: match source.source_class {
                    RealizedVolatilitySourceClassBlock::SpotQuote => {
                        RealizedVolSourceClass::SpotQuote
                    }
                    RealizedVolatilitySourceClassBlock::Trade => RealizedVolSourceClass::Trade,
                    RealizedVolatilitySourceClassBlock::Mark => RealizedVolSourceClass::Mark,
                    RealizedVolatilitySourceClassBlock::Index => RealizedVolSourceClass::Index,
                },
                sample_kind: match source.sample_kind {
                    RealizedVolatilitySampleKindBlock::Midpoint => RealizedVolSampleKind::Midpoint,
                    RealizedVolatilitySampleKindBlock::Trade => RealizedVolSampleKind::Trade,
                    RealizedVolatilitySampleKindBlock::Mark => RealizedVolSampleKind::Mark,
                    RealizedVolatilitySampleKindBlock::Index => RealizedVolSampleKind::Index,
                },
                enabled: source.enabled,
                counts_toward_quorum: source.counts_toward_quorum,
                canonical_base_asset: source.canonical_base_asset.clone(),
                canonical_quote_asset: source.canonical_quote_asset.clone(),
            })
            .collect(),
    })
}

fn realized_volatility_aggregation(
    surface: &RealizedVolatilitySurfaceBlock,
) -> Result<RealizedVolAggregation, String> {
    Ok(match surface.policy.aggregation {
        RealizedVolatilityAggregationBlock::UpperQuantile => {
            RealizedVolAggregation::UpperQuantile {
                quantile: surface.policy.upper_quantile,
            }
        }
        RealizedVolatilityAggregationBlock::Median => RealizedVolAggregation::Median,
        RealizedVolatilityAggregationBlock::TrimmedMean => RealizedVolAggregation::TrimmedMean {
            trim_fraction: surface
                .policy
                .trim_fraction
                .ok_or_else(|| "trimmed_mean aggregation requires trim_fraction".to_string())?,
        },
        RealizedVolatilityAggregationBlock::MedianWithUpperQuantileGuard => {
            RealizedVolAggregation::MedianWithUpperQuantileGuard {
                upper_quantile: surface.policy.upper_quantile,
                guard_weight: surface.policy.guard_weight.ok_or_else(|| {
                    "median_with_upper_quantile_guard aggregation requires guard_weight".to_string()
                })?,
            }
        }
    })
}

fn realized_volatility_estimator_config(
    surface: &RealizedVolatilitySurfaceBlock,
) -> Result<RealizedVolEstimatorConfig, String> {
    let Some(estimator) = surface.estimator.as_ref() else {
        return Ok(RealizedVolEstimatorConfig::measured());
    };
    let noise_method = match estimator.noise_robust_method.ok_or_else(|| {
        "estimator.noise_robust_method must be set when estimator is configured".to_string()
    })? {
        RealizedVolatilityNoiseMethodBlock::None => RealizedVolNoiseMethod::None,
        RealizedVolatilityNoiseMethodBlock::CoarserGrid => RealizedVolNoiseMethod::CoarserGrid {
            coarse_sampling_interval_ms: estimator.coarse_sampling_interval_ms.ok_or_else(
                || {
                    "estimator.coarse_sampling_interval_ms must be set for coarser_grid RV"
                        .to_string()
                },
            )?,
            policy: match estimator.coarser_grid_policy.ok_or_else(|| {
                "estimator.coarser_grid_policy must be set for coarser_grid RV".to_string()
            })? {
                RealizedVolatilityCoarserGridPolicyBlock::CoarseOnly => {
                    RealizedVolCoarserGridPolicy::CoarseOnly
                }
                RealizedVolatilityCoarserGridPolicyBlock::MinBaseCoarse => {
                    RealizedVolCoarserGridPolicy::MinBaseCoarse
                }
            },
        },
        RealizedVolatilityNoiseMethodBlock::Subsampled => RealizedVolNoiseMethod::Subsampled {
            subsamples: estimator
                .subsamples
                .ok_or_else(|| "estimator.subsamples must be set for subsampled RV".to_string())?,
            min_ready_subsamples: estimator.min_ready_subsamples.ok_or_else(|| {
                "estimator.min_ready_subsamples must be set for subsampled RV".to_string()
            })?,
        },
    };
    Ok(RealizedVolEstimatorConfig {
        horizons: Vec::new(),
        horizon_policy: crate::bolt_v3_realized_volatility::RealizedVolHorizonPolicy::Measured,
        noise: RealizedVolNoiseConfig {
            method: noise_method,
        },
        jump: RealizedVolJumpConfig {
            policy: match estimator.jump_policy.ok_or_else(|| {
                "estimator.jump_policy must be set when estimator is configured".to_string()
            })? {
                RealizedVolatilityJumpPolicyBlock::None => RealizedVolJumpPolicy::None,
                RealizedVolatilityJumpPolicyBlock::Separate => RealizedVolJumpPolicy::Separate,
            },
        },
        pricing_component: match estimator.pricing_component.ok_or_else(|| {
            "estimator.pricing_component must be set when estimator is configured".to_string()
        })? {
            RealizedVolatilityPricingComponentBlock::Measured => {
                RealizedVolPricingComponent::Measured
            }
            RealizedVolatilityPricingComponentBlock::NoiseRobust => {
                RealizedVolPricingComponent::NoiseRobust
            }
            RealizedVolatilityPricingComponentBlock::Continuous => {
                RealizedVolPricingComponent::Continuous
            }
            RealizedVolatilityPricingComponentBlock::Forecast => {
                RealizedVolPricingComponent::Forecast
            }
        },
        forecast: crate::bolt_v3_realized_volatility::RealizedVolForecastConfig::none(),
    })
}

#[derive(Debug, Clone)]
pub struct LoadedStrategy {
    pub config_path: PathBuf,
    pub relative_path: String,
    pub config: BoltV3StrategyConfig,
}

#[derive(Debug, Clone)]
pub struct LoadedBoltV3Config {
    pub root_path: PathBuf,
    pub config_bundle_checksum: String,
    pub root: BoltV3RootConfig,
    pub strategies: Vec<LoadedStrategy>,
}

#[derive(Debug)]
pub enum BoltV3ConfigError {
    FileRead {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    Validation(BoltV3ValidationError),
}

impl std::fmt::Display for BoltV3ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3ConfigError::FileRead { path, source } => {
                write!(f, "failed to read {}: {source}", path.display())
            }
            BoltV3ConfigError::Parse { path, message } => {
                write!(f, "failed to parse {}: {message}", path.display())
            }
            BoltV3ConfigError::Validation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for BoltV3ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3ConfigError::FileRead { source, .. } => Some(source.as_ref()),
            BoltV3ConfigError::Validation(error) => Some(error),
            _ => None,
        }
    }
}

pub fn load_bolt_v3_config(root_path: &Path) -> Result<LoadedBoltV3Config, BoltV3ConfigError> {
    let root_text = crate::bounded_config_read::read_to_string(root_path).map_err(|source| {
        BoltV3ConfigError::FileRead {
            path: root_path.to_path_buf(),
            source: Box::new(source),
        }
    })?;
    let root: BoltV3RootConfig =
        toml::from_str(&root_text).map_err(|error| BoltV3ConfigError::Parse {
            path: root_path.to_path_buf(),
            message: error.to_string(),
        })?;

    let mut strategies = Vec::with_capacity(root.strategy_files.len());
    let mut strategy_texts = Vec::with_capacity(root.strategy_files.len());
    let mut seen_paths = HashSet::new();
    let mut path_errors: Vec<String> = Vec::new();

    for relative in &root.strategy_files {
        if !seen_paths.insert(relative.clone()) {
            path_errors.push(format!(
                "strategy_files contains duplicate entry `{relative}`"
            ));
            continue;
        }
        let absolute = resolve_root_relative_path(root_path, relative);
        if !absolute.exists() {
            path_errors.push(format!(
                "strategy file `{relative}` does not exist at {}",
                absolute.display()
            ));
            continue;
        }
        let text = crate::bounded_config_read::read_to_string(&absolute).map_err(|source| {
            BoltV3ConfigError::FileRead {
                path: absolute.clone(),
                source: Box::new(source),
            }
        })?;
        let strategy: BoltV3StrategyConfig =
            toml::from_str(&text).map_err(|error| BoltV3ConfigError::Parse {
                path: absolute.clone(),
                message: error.to_string(),
            })?;
        strategy_texts.push((relative.clone(), text));
        strategies.push(LoadedStrategy {
            config_path: absolute,
            relative_path: relative.clone(),
            config: strategy,
        });
    }

    let mut validation_messages = path_errors;
    validation_messages.extend(validate_root_only(&root));
    validation_messages.extend(validate_strategies(&root, &strategies));

    if !validation_messages.is_empty() {
        return Err(BoltV3ConfigError::Validation(BoltV3ValidationError::new(
            validation_messages,
        )));
    }

    Ok(LoadedBoltV3Config {
        root_path: root_path.to_path_buf(),
        config_bundle_checksum: config_bundle_checksum(&root_text, &strategy_texts),
        root,
        strategies,
    })
}

const CONFIG_BUNDLE_CHECKSUM_DOMAIN: &[u8] = b"bolt-v3.config-bundle.v1\n";

fn config_bundle_checksum(root_text: &str, strategy_texts: &[(String, String)]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CONFIG_BUNDLE_CHECKSUM_DOMAIN);
    hasher.update((strategy_texts.len() as u32 + 1).to_be_bytes());
    update_config_bundle_entry(&mut hasher, 0, "root", root_text.as_bytes());

    let mut sorted_strategy_texts: Vec<_> = strategy_texts.iter().collect();
    sorted_strategy_texts.sort_by(|left, right| left.0.cmp(&right.0));
    for (relative_path, text) in sorted_strategy_texts {
        update_config_bundle_entry(&mut hasher, 1, relative_path, text.as_bytes());
    }

    hex::encode(hasher.finalize())
}

fn update_config_bundle_entry(hasher: &mut Sha256, kind: u8, key: &str, content: &[u8]) {
    hasher.update([kind]);
    hasher.update((key.len() as u32).to_be_bytes());
    hasher.update(key.as_bytes());
    hasher.update((content.len() as u64).to_be_bytes());
    hasher.update(content);
}

pub(crate) fn resolve_root_relative_path(
    root_path: &Path,
    configured_path: impl AsRef<Path>,
) -> PathBuf {
    let configured_path = configured_path.as_ref();
    if configured_path.is_absolute() {
        return configured_path.to_path_buf();
    }
    match root_path.parent() {
        Some(root_parent) => root_parent.join(configured_path),
        None => configured_path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_root_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/root.toml")
    }

    fn minimal_strategy_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/strategies/binary_oracle.toml")
    }

    fn root_toml_with_realized_volatility_surface(upper_quantile: f64) -> String {
        format!(
            r#"{}

[clients."<DATA_CLIENT_ID>"]
venue = "<DATA_CLIENT_VENUE>"

[clients."<DATA_CLIENT_ID>".data]

[realized_volatility_surfaces."<surface_id>"]
canonical_base_asset = "<BASE_ASSET>"
canonical_quote_asset = "<QUOTE_ASSET>"

[realized_volatility_surfaces."<surface_id>".policy]
window_ms = 4000
sampling_interval_ms = 1000
min_ready_sources = 1
max_source_age_ms = 500
max_event_receive_lag_ms = 250
max_inter_sample_gap_ms = 2000
min_coverage_ratio = 0.75
max_cross_source_dispersion = 0.50
seconds_per_annum = 31536000.0
aggregation = "upper_quantile"
upper_quantile = {upper_quantile}

[[realized_volatility_surfaces."<surface_id>".sources]]
source_id = "<SOURCE_ID_A>"
data_client_id = "<DATA_CLIENT_ID>"
instrument_id = "<INSTRUMENT_ID_A>.<DATA_CLIENT_ID>"
source_class = "spot_quote"
sample_kind = "midpoint"
enabled = true
counts_toward_quorum = true
canonical_base_asset = "<BASE_ASSET>"
canonical_quote_asset = "<QUOTE_ASSET>"
"#,
            minimal_root_toml()
        )
    }

    #[test]
    fn parses_minimal_root_block() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        assert_eq!(root.schema_version, 1);
        assert_eq!(root.trader_id, TraderId::from("BOLT-001"));
        assert_eq!(root.runtime.mode, Environment::Live);
        assert!(root.clients.contains_key("polymarket_main"));
        let polymarket = &root.clients["polymarket_main"];
        assert_eq!(polymarket.venue, Venue::from("POLYMARKET"));
        assert!(polymarket.execution.is_some());
        assert!(!root.clients.contains_key("binance_reference"));
    }

    #[test]
    fn parses_realized_volatility_surfaces_from_root_config() {
        let raw = root_toml_with_realized_volatility_surface(1.0);

        let config: BoltV3RootConfig = toml::from_str(&raw).expect("root config should parse");
        let surface = config
            .realized_volatility_surfaces
            .as_ref()
            .and_then(|surfaces| surfaces.get("<surface_id>"))
            .unwrap();
        assert_eq!(
            surface.policy.aggregation,
            RealizedVolatilityAggregationBlock::UpperQuantile
        );
        assert_eq!(surface.sources[0].source_id, "<SOURCE_ID_A>");
    }

    #[test]
    fn realized_volatility_engine_config_carries_toml_upper_quantile() {
        let raw = root_toml_with_realized_volatility_surface(0.75);
        let config: BoltV3RootConfig = toml::from_str(&raw).expect("root config should parse");
        let surface = config
            .realized_volatility_surfaces
            .as_ref()
            .and_then(|surfaces| surfaces.get("<surface_id>"))
            .unwrap();

        let engine_config = realized_volatility_engine_config("<surface_id>", surface)
            .expect("validated surface should map to engine config");

        assert_eq!(
            engine_config.aggregation,
            crate::bolt_v3_realized_volatility::RealizedVolAggregation::UpperQuantile {
                quantile: 0.75
            }
        );
    }

    #[test]
    fn realized_volatility_engine_config_carries_source_binding_fields() {
        let raw = root_toml_with_realized_volatility_surface(1.0);
        let config: BoltV3RootConfig = toml::from_str(&raw).expect("root config should parse");
        let surface = config
            .realized_volatility_surfaces
            .as_ref()
            .and_then(|surfaces| surfaces.get("<surface_id>"))
            .unwrap();

        let engine_config = realized_volatility_engine_config("<surface_id>", surface)
            .expect("validated surface should map to engine config");

        let source = engine_config
            .sources
            .first()
            .expect("fixture should include source");
        let parsed_source = &surface.sources[0];
        assert_eq!(source.source_id, parsed_source.source_id);
        assert_eq!(
            source.data_client_id,
            parsed_source.data_client_id.to_string()
        );
        assert_eq!(
            source.instrument_id,
            parsed_source.instrument_id.to_string()
        );
        assert_eq!(
            source.canonical_base_asset,
            parsed_source.canonical_base_asset
        );
        assert_eq!(
            source.canonical_quote_asset,
            parsed_source.canonical_quote_asset
        );
    }

    #[test]
    fn parses_minimal_strategy_block() {
        let strategy: BoltV3StrategyConfig = toml::from_str(minimal_strategy_toml()).unwrap();
        assert!(!strategy.strategy_archetype.as_str().is_empty());
        // The strategy envelope keeps `target` as raw TOML. Verify the
        // raw envelope here only at the structural level.
        let target_table = strategy
            .target
            .as_table()
            .expect("[target] should parse into a table");
        assert!(!target_table.is_empty());
        assert!(!strategy.signal_data.is_empty());
    }

    #[test]
    fn risk_default_max_notional_must_be_positive_decimal() {
        let mut root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        for bad in ["0.00", "-1.00"] {
            root.risk.default_max_notional_per_order = bad.to_string();
            let errors = crate::bolt_v3_validate::validate_root_only(&root);
            assert!(
                errors.iter().any(|e| e.contains(
                    "risk.default_max_notional_per_order must be a positive decimal string"
                )),
                "default_max_notional_per_order={bad} must fail the positive check; got: {errors:?}"
            );
        }
        // A malformed decimal keeps the existing syntax error rather than the
        // positivity message.
        root.risk.default_max_notional_per_order = "not-a-decimal".to_string();
        let errors = crate::bolt_v3_validate::validate_root_only(&root);
        assert!(
            errors
                .iter()
                .any(|e| e
                    .contains("risk.default_max_notional_per_order is not a valid decimal string")),
            "malformed default_max_notional must keep the syntax error; got: {errors:?}"
        );
    }
}
