//! Bolt-v3 root and strategy TOML configuration types and loading.
//!
//! Schema: docs/bolt-v3/2026-04-25-bolt-v3-schema.md
//! Runtime contracts: docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md
//!
//! This module is intentionally a no-trade boundary. It only parses and
//! validates configuration; it does not register strategies, build
//! clients, perform market selection, or construct orders.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::bolt_v3_validate::{BoltV3ValidationError, validate_root_only, validate_strategies};

pub const REFERENCE_STREAM_ID_PARAMETER: &str = "reference_stream_id";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoltV3RootConfig {
    pub schema_version: u32,
    pub trader_id: String,
    pub strategy_files: Vec<String>,
    pub runtime: RuntimeBlock,
    pub nautilus: NautilusBlock,
    pub risk: RiskBlock,
    pub logging: LoggingBlock,
    pub persistence: PersistenceBlock,
    pub aws: AwsBlock,
    #[serde(default)]
    pub reference_streams: BTreeMap<String, ReferenceStreamBlock>,
    pub clients: BTreeMap<String, ClientBlock>,
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
    pub mode: RuntimeMode,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeMode {
    Live,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NautilusBlock {
    pub load_state: bool,
    pub save_state: bool,
    pub timeout_connection_seconds: u64,
    pub timeout_reconciliation_seconds: u64,
    pub data_engine: NautilusDataEngineBlock,
    pub exec_engine: NautilusExecEngineBlock,
    pub timeout_portfolio_seconds: u64,
    pub timeout_disconnection_seconds: u64,
    pub delay_post_stop_seconds: u64,
    pub timeout_shutdown_seconds: u64,
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
    pub external_client_ids: Vec<String>,
    pub debug: bool,
    pub graceful_shutdown_on_error: bool,
    pub qsize: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NautilusExecEngineBlock {
    pub load_cache: bool,
    pub snapshot_orders: bool,
    pub snapshot_positions: bool,
    pub snapshot_positions_interval_seconds: u64,
    pub external_client_ids: Vec<String>,
    pub debug: bool,
    pub reconciliation: bool,
    pub reconciliation_startup_delay_seconds: u64,
    pub reconciliation_lookback_mins: u32,
    pub reconciliation_instrument_ids: Vec<String>,
    pub filter_unclaimed_external_orders: bool,
    pub filter_position_reports: bool,
    pub filtered_client_order_ids: Vec<String>,
    pub generate_missing_orders: bool,
    pub inflight_check_interval_milliseconds: u32,
    pub inflight_check_threshold_milliseconds: u32,
    pub inflight_check_retries: u32,
    pub open_check_interval_seconds: u64,
    pub open_check_lookback_mins: u32,
    pub open_check_threshold_milliseconds: u32,
    pub open_check_missing_retries: u32,
    pub open_check_open_only: bool,
    pub max_single_order_queries_per_cycle: u32,
    pub single_order_query_delay_milliseconds: u32,
    pub position_check_interval_seconds: u64,
    pub position_check_lookback_mins: u32,
    pub position_check_threshold_milliseconds: u32,
    pub position_check_retries: u32,
    pub purge_closed_orders_interval_mins: u32,
    pub purge_closed_orders_buffer_mins: u32,
    pub purge_closed_positions_interval_mins: u32,
    pub purge_closed_positions_buffer_mins: u32,
    pub purge_account_events_interval_mins: u32,
    pub purge_account_events_lookback_mins: u32,
    pub purge_from_database: bool,
    pub own_books_audit_interval_seconds: u64,
    pub graceful_shutdown_on_error: bool,
    pub qsize: u32,
    pub allow_overfills: bool,
    pub manage_own_order_books: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RiskBlock {
    pub default_max_notional_per_order: String,
    pub nt_bypass: bool,
    pub nt_max_order_submit_rate: String,
    pub nt_max_order_modify_rate: String,
    pub nt_max_notional_per_order: BTreeMap<String, String>,
    pub nt_debug: bool,
    pub nt_graceful_shutdown_on_error: bool,
    pub nt_qsize: u32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LoggingBlock {
    pub standard_output_level: LogLevel,
    pub file_level: LogLevel,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Off,
}

impl LogLevel {
    pub fn to_level_filter(self) -> log::LevelFilter {
        match self {
            LogLevel::Trace => log::LevelFilter::Trace,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Off => log::LevelFilter::Off,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PersistenceBlock {
    pub catalog_directory: String,
    pub streaming: StreamingBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StreamingBlock {
    pub catalog_fs_protocol: CatalogFsProtocol,
    pub flush_interval_milliseconds: u64,
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
#[serde(deny_unknown_fields)]
pub struct ReferenceStreamBlock {
    pub publish_topic: String,
    pub min_publish_interval_milliseconds: u64,
    pub inputs: Vec<ReferenceStreamInputBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceStreamInputBlock {
    pub source_id: String,
    pub source_type: ReferenceSourceType,
    #[serde(default)]
    pub data_client_id: Option<String>,
    pub instrument_id: String,
    pub base_weight: f64,
    pub stale_after_milliseconds: u64,
    pub disable_after_milliseconds: u64,
    #[serde(default)]
    pub chainlink: Option<ReferenceStreamChainlinkInputBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceStreamChainlinkInputBlock {
    pub feed_id: String,
    pub price_scale: u8,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceSourceType {
    Oracle,
    Orderbook,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClientBlock {
    pub venue: VenueKey,
    #[serde(default)]
    pub data: Option<toml::Value>,
    #[serde(default)]
    pub execution: Option<toml::Value>,
    #[serde(default)]
    pub secrets: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct VenueKey(String);

impl VenueKey {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoltV3StrategyConfig {
    pub schema_version: u32,
    pub strategy_instance_id: String,
    pub strategy_archetype: StrategyArchetypeKey,
    pub order_id_tag: String,
    pub oms_type: OmsType,
    pub execution_client_id: String,
    /// Raw `[target]` envelope. The strategy envelope keeps the TOML
    /// field name `target` but its Rust type is a generic raw-TOML
    /// container so target-shape fields live in the per-family binding
    /// modules under `crate::bolt_v3_market_families`. Typed
    /// deserialization with `deny_unknown_fields` happens inside the
    /// matching family validator and inside the family planner; the
    /// strategy envelope itself is target-shape-neutral.
    pub target: toml::Value,
    #[serde(default)]
    pub reference_data: BTreeMap<String, ReferenceDataBlock>,
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OmsType {
    Netting,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReferenceDataBlock {
    pub data_client_id: String,
    pub instrument_id: String,
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
    pub root: BoltV3RootConfig,
    pub strategies: Vec<LoadedStrategy>,
}

#[derive(Debug)]
pub enum BoltV3ConfigError {
    FileRead {
        path: PathBuf,
        source: std::io::Error,
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
            BoltV3ConfigError::FileRead { source, .. } => Some(source),
            BoltV3ConfigError::Validation(error) => Some(error),
            _ => None,
        }
    }
}

pub fn load_bolt_v3_config(root_path: &Path) -> Result<LoadedBoltV3Config, BoltV3ConfigError> {
    let root_text =
        std::fs::read_to_string(root_path).map_err(|source| BoltV3ConfigError::FileRead {
            path: root_path.to_path_buf(),
            source,
        })?;
    let root: BoltV3RootConfig =
        toml::from_str(&root_text).map_err(|error| BoltV3ConfigError::Parse {
            path: root_path.to_path_buf(),
            message: error.to_string(),
        })?;

    let root_dir = root_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut strategies = Vec::with_capacity(root.strategy_files.len());
    let mut seen_paths = HashSet::new();
    let mut path_errors: Vec<String> = Vec::new();

    for relative in &root.strategy_files {
        if !seen_paths.insert(relative.clone()) {
            path_errors.push(format!(
                "strategy_files contains duplicate entry `{relative}`"
            ));
            continue;
        }
        let absolute = root_dir.join(relative);
        if !absolute.exists() {
            path_errors.push(format!(
                "strategy file `{relative}` does not exist at {}",
                absolute.display()
            ));
            continue;
        }
        let text =
            std::fs::read_to_string(&absolute).map_err(|source| BoltV3ConfigError::FileRead {
                path: absolute.clone(),
                source,
            })?;
        let strategy: BoltV3StrategyConfig =
            toml::from_str(&text).map_err(|error| BoltV3ConfigError::Parse {
                path: absolute.clone(),
                message: error.to_string(),
            })?;
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
        root,
        strategies,
    })
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

    #[test]
    fn parses_minimal_root_block() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        assert_eq!(root.schema_version, 1);
        assert_eq!(root.trader_id, "BOLT-001");
        assert_eq!(root.runtime.mode, RuntimeMode::Live);
        assert!(root.clients.contains_key("polymarket_main"));
        assert!(root.clients.contains_key("binance_reference"));
        let polymarket = &root.clients["polymarket_main"];
        assert_eq!(polymarket.venue.as_str(), "POLYMARKET");
        assert!(polymarket.execution.is_some());
        let binance = &root.clients["binance_reference"];
        assert_eq!(binance.venue.as_str(), "BINANCE");
        assert!(binance.execution.is_none());
    }

    #[test]
    fn parses_minimal_strategy_block() {
        let strategy: BoltV3StrategyConfig = toml::from_str(minimal_strategy_toml()).unwrap();
        assert!(!strategy.strategy_archetype.as_str().is_empty());
        assert_eq!(strategy.execution_client_id, "polymarket_main");
        // The strategy envelope keeps `target` as raw TOML. Verify the
        // raw envelope here only at the structural level.
        let target_table = strategy
            .target
            .as_table()
            .expect("[target] should parse into a table");
        assert!(!target_table.is_empty());
        assert!(strategy.reference_data.contains_key("primary"));
    }
}
