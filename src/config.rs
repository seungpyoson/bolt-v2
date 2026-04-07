use std::path::Path;

use serde::Deserialize;
use toml::Value;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub node: NodeConfig,
    pub logging: LoggingConfig,
    pub data_clients: Vec<DataClientEntry>,
    pub exec_clients: Vec<ExecClientEntry>,
    pub strategies: Vec<StrategyEntry>,
    #[serde(default)]
    pub streaming: StreamingCaptureConfig,
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

#[derive(Debug, Default, Deserialize)]
pub struct StreamingCaptureConfig {
    pub catalog_path: String,
    pub flush_interval_ms: u64,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
        let config: Config = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse config file {}: {e}", path.display()))?;
        Ok(config)
    }
}
