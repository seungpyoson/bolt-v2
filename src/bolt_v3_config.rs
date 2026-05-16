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

use serde::Deserialize;
use toml::Value;

use crate::bolt_v3_validate::{BoltV3ValidationError, validate_root_only, validate_strategies};

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
    pub live_canary: Option<LiveCanaryBlock>,
    pub aws: AwsBlock,
    pub venues: BTreeMap<String, VenueBlock>,
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
    Backtest,
    Sandbox,
    Live,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NautilusBlock {
    pub load_state: bool,
    pub save_state: bool,
    pub instance_id: NautilusComponentConfig,
    pub cache: NautilusComponentConfig,
    pub msgbus: NautilusComponentConfig,
    pub portfolio: NautilusComponentConfig,
    pub emulator: NautilusComponentConfig,
    pub streaming: NautilusComponentConfig,
    pub loop_debug: bool,
    pub timeout_connection_seconds: u64,
    pub timeout_reconciliation_seconds: u64,
    pub data_engine: NautilusDataEngineBlock,
    pub exec_engine: NautilusExecEngineBlock,
    pub timeout_portfolio_seconds: u64,
    pub timeout_disconnection_seconds: u64,
    pub delay_post_stop_seconds: u64,
    pub timeout_shutdown_seconds: u64,
}

pub type NautilusComponentConfig = Value;

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

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoggingBlock {
    pub standard_output_level: LogLevel,
    pub file_level: LogLevel,
    pub component_levels: BTreeMap<String, LogLevel>,
    pub module_levels: BTreeMap<String, LogLevel>,
    pub credential_module_level: LogLevel,
    pub log_components_only: bool,
    pub is_colored: bool,
    pub print_config: bool,
    pub use_tracing: bool,
    pub bypass_logging: bool,
    pub file_config: NautilusComponentConfig,
    pub clear_log_file: bool,
    pub stale_log_source_directory: String,
    pub stale_log_archive_directory: String,
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
    pub runtime_capture_start_poll_interval_milliseconds: u64,
    pub decision_evidence: DecisionEvidenceBlock,
    pub streaming: StreamingBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DecisionEvidenceBlock {
    pub order_intents_relative_path: String,
}

/// Operator approval and canary bounds required by the bolt-v3 live
/// canary gate before `run_bolt_v3_live_node` may enter NT's runner
/// loop. Field semantics are defined by the `[live_canary]` schema
/// section.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LiveCanaryBlock {
    pub approval_id: String,
    pub no_submit_readiness_report_path: String,
    pub max_no_submit_readiness_report_bytes: u64,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
    pub operator_evidence: Option<LiveCanaryOperatorEvidenceBlock>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LiveCanaryOperatorEvidenceBlock {
    pub approval_envelope_path: String,
    pub ssm_manifest_path: String,
    pub ssm_manifest_sha256: String,
    pub strategy_input_evidence_path: String,
    pub strategy_input_evidence_sha256: String,
    pub canary_evidence_path: String,
    pub approval_not_before_unix_seconds: i64,
    pub approval_not_after_unix_seconds: i64,
    pub approval_nonce_path: String,
    pub approval_nonce_sha256: String,
    pub approval_consumption_path: String,
    pub decision_evidence_path: String,
    pub client_order_id_hash: String,
    pub venue_order_id_hash: String,
    pub nt_submit_event_path: String,
    pub venue_order_state_path: String,
    pub strategy_cancel_path: Option<String>,
    pub restart_reconciliation_path: String,
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
pub struct VenueBlock {
    pub kind: ProviderKey,
    pub data: Option<toml::Value>,
    pub execution: Option<toml::Value>,
    pub secrets: Option<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct ProviderKey(String);

impl ProviderKey {
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
    pub use_uuid_client_order_ids: bool,
    pub use_hyphens_in_client_order_ids: bool,
    pub external_order_claims: Vec<String>,
    pub manage_contingent_orders: bool,
    pub manage_gtd_expiry: bool,
    pub manage_stop: bool,
    pub market_exit_interval_ms: u64,
    pub market_exit_max_attempts: u64,
    pub market_exit_time_in_force: String,
    pub market_exit_reduce_only: bool,
    pub log_events: bool,
    pub log_commands: bool,
    pub log_rejected_due_post_only_as_warning: bool,
    pub venue: String,
    /// Raw `[target]` envelope. The strategy envelope keeps the TOML
    /// field name `target` but its Rust type is a generic raw-TOML
    /// container so target-shape fields live in the per-family binding
    /// modules under `crate::bolt_v3_market_families`. Typed
    /// deserialization with `deny_unknown_fields` happens inside the
    /// matching family validator and inside the family instrument-filter
    /// code; the strategy envelope itself stores only the raw TOML value.
    pub target: toml::Value,
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
    pub venue: String,
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
    use std::fs;
    use tempfile::tempdir;

    fn minimal_root_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/root.toml")
    }

    fn minimal_strategy_toml() -> &'static str {
        include_str!("../tests/fixtures/bolt_v3/strategies/binary_oracle.toml")
    }

    fn oversized_config_text() -> String {
        let mut text = String::new();
        while text.len() as u64 <= crate::bounded_config_read::CONFIG_FILE_SIZE_LIMIT_BYTES {
            text.push_str("# oversized config\n");
        }
        text
    }

    #[test]
    fn parses_minimal_root_block() {
        let root: BoltV3RootConfig = toml::from_str(minimal_root_toml()).unwrap();
        assert_eq!(root.schema_version, 1);
        assert_eq!(root.trader_id, "BOLT-001");
        assert_eq!(root.runtime.mode, RuntimeMode::Live);
        assert!(root.venues.contains_key("polymarket_main"));
        assert!(root.venues.contains_key("binance_reference"));
        let polymarket = &root.venues["polymarket_main"];
        assert_eq!(polymarket.kind.as_str(), "polymarket");
        assert!(polymarket.execution.is_some());
        let binance = &root.venues["binance_reference"];
        assert_eq!(binance.kind.as_str(), "binance");
        assert!(binance.execution.is_none());
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
        assert!(strategy.reference_data.contains_key("spot"));
    }

    #[test]
    fn strategy_config_requires_explicit_reference_data_structure() {
        let strategy_without_reference_data = minimal_strategy_toml().replace(
            r#"[reference_data.spot]
venue = "binance_reference"
instrument_id = "BTCUSDT.BINANCE"

"#,
            "",
        );

        let error = toml::from_str::<BoltV3StrategyConfig>(&strategy_without_reference_data)
            .expect_err("strategy config must explicitly declare [reference_data]");

        assert!(
            error.message().contains("missing field `reference_data`"),
            "expected missing reference_data parse error, got: {error}"
        );
    }

    #[test]
    fn root_relative_path_resolves_against_root_parent() {
        assert_eq!(
            resolve_root_relative_path(
                Path::new("/srv/bolt/config/root.toml"),
                "strategies/binary_oracle.toml"
            ),
            PathBuf::from("/srv/bolt/config/strategies/binary_oracle.toml")
        );
    }

    #[test]
    fn root_relative_path_preserves_absolute_paths() {
        assert_eq!(
            resolve_root_relative_path(
                Path::new("/srv/bolt/config/root.toml"),
                "/srv/bolt/reports/no-submit.json"
            ),
            PathBuf::from("/srv/bolt/reports/no-submit.json")
        );
    }

    #[test]
    fn load_bolt_v3_config_rejects_oversized_root_before_parse() {
        let tempdir = tempdir().expect("temp dir should be created");
        let root_path = tempdir.path().join("root.toml");
        fs::write(&root_path, oversized_config_text()).expect("oversized root should be written");

        let error = load_bolt_v3_config(&root_path).expect_err("oversized root should fail closed");

        assert!(
            error.to_string().contains("exceeds config file size limit"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_bolt_v3_config_rejects_oversized_strategy_before_parse() {
        let tempdir = tempdir().expect("temp dir should be created");
        let strategy_dir = tempdir.path().join("strategies");
        fs::create_dir_all(&strategy_dir).expect("strategy dir should be created");
        let root_path = tempdir.path().join("root.toml");
        let strategy_path = strategy_dir.join("binary_oracle.toml");
        fs::write(&root_path, minimal_root_toml()).expect("root should be written");
        fs::write(&strategy_path, oversized_config_text())
            .expect("oversized strategy should be written");

        let error =
            load_bolt_v3_config(&root_path).expect_err("oversized strategy should fail closed");

        assert!(
            error.to_string().contains("exceeds config file size limit"),
            "unexpected error: {error}"
        );
    }
}
