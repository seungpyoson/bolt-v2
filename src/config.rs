use std::path::Path;

use serde::{Deserialize, Serialize};
use toml::Value;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub node: NodeConfig,
    pub logging: LoggingConfig,
    pub data_clients: Vec<DataClientEntry>,
    pub exec_clients: Vec<ExecClientEntry>,
    pub strategies: Vec<StrategyEntry>,
    #[serde(default)]
    pub raw_capture: RawCaptureConfig,
    #[serde(default)]
    pub streaming: StreamingCaptureConfig,
    #[serde(default)]
    pub reference: ReferenceConfig,
    #[serde(default)]
    pub rulesets: Vec<RulesetConfig>,
    #[serde(default)]
    pub audit: Option<AuditConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceVenueKind {
    Binance,
    Bybit,
    Deribit,
    Hyperliquid,
    Kraken,
    Okx,
    Polymarket,
    Chainlink,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RulesetVenueKind {
    Polymarket,
}

#[derive(Debug, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub trader_id: String,
    pub environment: String,
    pub load_state: bool,
    pub save_state: bool,
    pub timeout_connection_secs: u64,
    pub timeout_reconciliation_secs: u64,
    pub timeout_portfolio_secs: u64,
    pub timeout_disconnection_secs: u64,
    pub delay_post_stop_secs: u64,
    pub delay_shutdown_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    pub stdout_level: String,
    pub file_level: String,
}

#[derive(Debug, Deserialize)]
pub struct DataClientEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
}

#[derive(Debug, Deserialize)]
pub struct ExecClientSecrets {
    pub region: String,
    pub pk: Option<String>,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExecClientEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
    pub secrets: ExecClientSecrets,
}

#[derive(Debug, Deserialize)]
pub struct StrategyEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
}

#[derive(Debug, Deserialize)]
pub struct RawCaptureConfig {
    #[serde(default = "default_raw_capture_output_dir")]
    pub output_dir: String,
}

impl Default for RawCaptureConfig {
    fn default() -> Self {
        Self {
            output_dir: default_raw_capture_output_dir(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamingCaptureConfig {
    pub catalog_path: String,
    pub flush_interval_ms: u64,
    #[serde(default)]
    pub contract_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReferenceConfig {
    pub publish_topic: String,
    pub min_publish_interval_ms: u64,
    #[serde(default)]
    pub chainlink: Option<ChainlinkSharedConfig>,
    #[serde(default)]
    pub venues: Vec<ReferenceVenueEntry>,
}

impl Default for ReferenceConfig {
    fn default() -> Self {
        Self {
            publish_topic: String::new(),
            min_publish_interval_ms: 100,
            chainlink: None,
            venues: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReferenceVenueEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ReferenceVenueKind,
    pub instrument_id: String,
    pub base_weight: f64,
    pub stale_after_ms: u64,
    pub disable_after_ms: u64,
    #[serde(default)]
    pub chainlink: Option<ChainlinkReferenceConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkReferenceConfig {
    pub feed_id: String,
    pub price_scale: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkSharedConfig {
    pub region: String,
    pub api_key: String,
    pub api_secret: String,
    pub ws_url: String,
    pub ws_reconnect_alert_threshold: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RulesetConfig {
    pub id: String,
    pub venue: RulesetVenueKind,
    pub tag_slug: String,
    pub event_slug_prefix: String,
    pub resolution_basis: String,
    pub min_time_to_expiry_secs: u64,
    pub max_time_to_expiry_secs: u64,
    pub min_liquidity_num: f64,
    pub require_accepting_orders: bool,
    pub freeze_before_end_secs: u64,
    pub selector_poll_interval_ms: u64,
    pub candidate_load_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuditConfig {
    pub local_dir: String,
    pub s3_uri: String,
    pub ship_interval_secs: u64,
    pub upload_attempt_timeout_secs: u64,
    pub roll_max_bytes: u64,
    pub roll_max_secs: u64,
    pub max_local_backlog_bytes: u64,
}

pub(crate) fn default_raw_capture_output_dir() -> String {
    "var/raw".to_string()
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse config file {}: {e}", path.display()))?;

        let validation_errors = crate::validate::validate_runtime(&config);
        if !validation_errors.is_empty() {
            let details: Vec<String> = validation_errors
                .iter()
                .map(|e| format!("  - {e}"))
                .collect();
            return Err(format!(
                "Runtime config validation failed ({} error{}):\n{}",
                validation_errors.len(),
                if validation_errors.len() == 1 {
                    ""
                } else {
                    "s"
                },
                details.join("\n"),
            )
            .into());
        }

        Ok(config)
    }
}
