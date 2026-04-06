use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub node: NodeConfig,
    pub logging: LoggingConfig,
    pub timeouts: TimeoutsConfig,
    pub venue: VenueConfig,
    pub strategy: StrategyConfig,
    pub wallet: WalletConfig,
}

#[derive(Debug, Deserialize)]
pub struct NodeConfig {
    pub name: String,
    pub trader_id: String,
    pub account_id: String,
    pub client_id: String,
    pub environment: String,
    pub load_state: bool,
    pub save_state: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    pub stdout_level: String,
    pub file_level: String,
}

#[derive(Debug, Deserialize)]
pub struct TimeoutsConfig {
    pub connection_secs: u64,
    pub reconciliation_secs: u64,
    pub portfolio_secs: u64,
    pub disconnection_secs: u64,
    pub post_stop_delay_secs: u64,
    pub shutdown_delay_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct VenueConfig {
    pub event_slug: String,
    pub instrument_id: String,
    pub reconciliation_enabled: bool,
    pub reconciliation_lookback_mins: u32,
}

#[derive(Debug, Deserialize)]
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

/// All wallet-related config in one place. Change wallets = change this section.
#[derive(Debug, Deserialize)]
pub struct WalletConfig {
    pub signature_type_id: u8,
    pub funder: String,
    pub secrets: WalletSecretsConfig,
}

/// SSM-resolved credentials. Each field is an SSM parameter path.
#[derive(Debug, Deserialize)]
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
            "ssm", "get-parameter",
            "--region", region,
            "--name", ssm_path,
            "--with-decryption",
            "--query", "Parameter.Value",
            "--output", "text",
        ])
        .output()
        .map_err(|e| format!("Failed to run `aws ssm get-parameter --name {ssm_path}`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("aws ssm get-parameter --name {ssm_path} failed: {stderr}").into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Pad a base64 string to a multiple of 4 characters.
/// NT's Credential uses base64::URL_SAFE which requires padding.
/// Polymarket issues secrets without padding — add it before use.
fn pad_base64(mut secret: String) -> String {
    let pad_len = (4 - secret.len() % 4) % 4;
    secret.extend(std::iter::repeat_n('=', pad_len));
    secret
}

impl WalletConfig {
    /// Every env var NT needs, where each value comes from, and any transformation.
    /// This is the single source of truth for config→env var mapping.
    /// Must be called before tokio runtime is created — see main().
    fn resolve_env_vars(&self) -> Result<Vec<(&str, String)>, Box<dyn std::error::Error>> {
        let r = &self.secrets.region;
        Ok(vec![
            ("POLYMARKET_PK",         resolve_secret(r, &self.secrets.pk)?),
            ("POLYMARKET_API_KEY",    resolve_secret(r, &self.secrets.api_key)?),
            ("POLYMARKET_API_SECRET", pad_base64(resolve_secret(r, &self.secrets.api_secret)?)),
            ("POLYMARKET_PASSPHRASE", resolve_secret(r, &self.secrets.passphrase)?),
            ("POLYMARKET_FUNDER",     self.funder.clone()),
        ])
    }

    pub fn inject(&self) -> Result<(), Box<dyn std::error::Error>> {
        for (env_name, value) in self.resolve_env_vars()? {
            unsafe { std::env::set_var(env_name, value); }
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
