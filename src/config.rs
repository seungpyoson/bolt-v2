use std::path::Path;

use serde::Deserialize;
use toml::Value;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub node: NodeConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub data_clients: Vec<DataClientEntry>,
    #[serde(default)]
    pub exec_clients: Vec<ExecClientEntry>,
    #[serde(default)]
    pub strategies: Vec<StrategyEntry>,
    #[serde(default)]
    pub timeouts: TimeoutsConfig,
    #[serde(default)]
    pub venue: VenueConfig,
    #[serde(default)]
    pub strategy: StrategyConfig,
    #[serde(default)]
    pub wallet: WalletConfig,
    #[serde(default)]
    pub raw_capture: RawCaptureConfig,
    #[serde(default)]
    pub streaming: StreamingCaptureConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub trader_id: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub client_id: String,
    pub environment: String,
    pub load_state: bool,
    pub save_state: bool,
    #[serde(default)]
    pub timeout_connection_secs: u64,
    #[serde(default)]
    pub timeout_reconciliation_secs: u64,
    #[serde(default)]
    pub timeout_portfolio_secs: u64,
    #[serde(default)]
    pub timeout_disconnection_secs: u64,
    #[serde(default)]
    pub delay_post_stop_secs: u64,
    #[serde(default)]
    pub delay_shutdown_secs: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct LoggingConfig {
    pub stdout_level: String,
    pub file_level: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct DataClientEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
}

#[derive(Debug, Default, Deserialize)]
pub struct ExecClientSecrets {
    pub region: String,
    pub pk: Option<String>,
    pub api_key: Option<String>,
    pub api_secret: Option<String>,
    pub passphrase: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ExecClientEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
    pub secrets: ExecClientSecrets,
}

#[derive(Debug, Default, Deserialize)]
pub struct StrategyEntry {
    #[serde(rename = "type")]
    pub kind: String,
    pub config: Value,
}

#[derive(Debug, Default, Deserialize)]
pub struct TimeoutsConfig {
    pub connection_secs: u64,
    pub reconciliation_secs: u64,
    pub portfolio_secs: u64,
    pub disconnection_secs: u64,
    pub post_stop_delay_secs: u64,
    pub shutdown_delay_secs: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct VenueConfig {
    pub event_slug: String,
    pub instrument_id: String,
    pub reconciliation_enabled: bool,
    pub reconciliation_lookback_mins: u32,
    #[serde(default)]
    pub subscribe_new_markets: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct RawCaptureConfig {
    pub output_dir: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct StreamingCaptureConfig {
    pub catalog_path: String,
    pub flush_interval_ms: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct StrategyConfig {
    pub strategy_id: String,
    pub log_data: bool,
    pub order_qty: String,
    pub tob_offset_ticks: u64,
    pub use_post_only: bool,
    #[serde(default)]
    pub enable_limit_sells: bool,
    #[serde(default)]
    pub enable_stop_buys: bool,
    #[serde(default)]
    pub enable_stop_sells: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct WalletConfig {
    pub signature_type_id: u8,
    pub funder: String,
    #[serde(default)]
    pub secrets: WalletSecretsConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct WalletSecretsConfig {
    pub region: String,
    pub pk: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
}

fn resolve_secret(region: &str, ssm_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let output = std::process::Command::new("aws")
        .args([
            "ssm",
            "get-parameter",
            "--region",
            region,
            "--name",
            ssm_path,
            "--with-decryption",
            "--query",
            "Parameter.Value",
            "--output",
            "text",
        ])
        .output()
        .map_err(|e| format!("Failed to run `aws ssm get-parameter --name {ssm_path}`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("aws ssm get-parameter --name {ssm_path} failed: {stderr}").into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

fn pad_base64(mut secret: String) -> String {
    let pad_len = (4 - secret.len() % 4) % 4;
    secret.extend(std::iter::repeat_n('=', pad_len));
    secret
}

impl WalletConfig {
    fn resolve_env_vars(&self) -> Result<Vec<(&str, String)>, Box<dyn std::error::Error>> {
        let r = &self.secrets.region;
        Ok(vec![
            ("POLYMARKET_PK", resolve_secret(r, &self.secrets.pk)?),
            (
                "POLYMARKET_API_KEY",
                resolve_secret(r, &self.secrets.api_key)?,
            ),
            (
                "POLYMARKET_API_SECRET",
                pad_base64(resolve_secret(r, &self.secrets.api_secret)?),
            ),
            (
                "POLYMARKET_PASSPHRASE",
                resolve_secret(r, &self.secrets.passphrase)?,
            ),
            ("POLYMARKET_FUNDER", self.funder.clone()),
        ])
    }

    pub fn inject(&self) -> Result<(), Box<dyn std::error::Error>> {
        for (env_name, value) in self.resolve_env_vars()? {
            unsafe {
                std::env::set_var(env_name, value);
            }
        }
        Ok(())
    }
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
