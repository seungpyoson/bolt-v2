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
    pub aws: AwsBlock,
    pub venues: BTreeMap<String, VenueBlock>,
}

// `[risk]` is intentionally narrow in the current bolt-v3 scope.
//
// The pinned NautilusTrader live-node API discards every
// `LiveRiskEngineConfig` field except `qsize` when constructing the
// runtime `RiskEngineConfig` (see `From<LiveRiskEngineConfig> for
// RiskEngineConfig` in the pinned `nautilus_live` crate). Carrying NT
// risk-engine knobs (rate limits, `bypass`) in the bolt-v3 schema while
// the build path drops them is a silent footgun: operators would see
// the keys validated and then have no effect on capital risk. So the
// only field this bolt-v3 slice owns under `[risk]` is the
// `default_max_notional_per_order` cap that bolt-v3 itself enforces in
// strategy validation. NautilusTrader-wired risk-engine knobs are
// re-introduced only when a future slice plumbs them through a real
// supported path.

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
    pub reconciliation_lookback_mins: u64,
    pub timeout_portfolio_seconds: u64,
    pub timeout_disconnection_seconds: u64,
    pub delay_post_stop_seconds: u64,
    pub timeout_shutdown_seconds: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RiskBlock {
    pub default_max_notional_per_order: String,
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
pub struct VenueBlock {
    pub kind: ProviderKey,
    #[serde(default)]
    pub data: Option<toml::Value>,
    #[serde(default)]
    pub execution: Option<toml::Value>,
    #[serde(default)]
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
    pub venue: String,
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
        assert!(strategy.reference_data.contains_key("primary"));
    }
}
