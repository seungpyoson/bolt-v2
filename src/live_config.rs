use std::path::Path;

use serde::{Deserialize, Serialize};

fn default_environment() -> String {
    "Live".to_string()
}

fn default_stdout_level() -> String {
    "Info".to_string()
}

fn default_file_level() -> String {
    "Debug".to_string()
}

fn default_timeout_connection_secs() -> u64 {
    60
}

fn default_timeout_reconciliation_secs() -> u64 {
    60
}

fn default_timeout_portfolio_secs() -> u64 {
    10
}

fn default_timeout_disconnection_secs() -> u64 {
    10
}

fn default_delay_post_stop_secs() -> u64 {
    5
}

fn default_delay_shutdown_secs() -> u64 {
    5
}

fn default_client_name() -> String {
    "POLYMARKET".to_string()
}

fn default_signature_type() -> u8 {
    2
}

fn default_update_instruments_interval_mins() -> u64 {
    60
}

fn default_ws_max_subscriptions() -> usize {
    200
}

fn default_strategy_id() -> String {
    "EXEC_TESTER-001".to_string()
}

fn default_order_qty() -> String {
    "5".to_string()
}

fn default_tob_offset_ticks() -> u64 {
    5
}

fn default_use_post_only() -> bool {
    true
}

fn default_region() -> String {
    "eu-west-1".to_string()
}

fn default_streaming_flush_interval_ms() -> u64 {
    1_000
}

fn default_raw_capture_output_dir() -> String {
    "var/raw".to_string()
}

#[derive(Debug, Deserialize)]
pub struct LiveLocalConfig {
    pub node: LiveNodeInput,
    #[serde(default)]
    pub logging: LiveLoggingInput,
    #[serde(default)]
    pub timeouts: LiveTimeoutsInput,
    pub polymarket: LivePolymarketInput,
    #[serde(default)]
    pub strategy: LiveStrategyInput,
    pub secrets: LiveSecretsInput,
    #[serde(default)]
    pub raw_capture: LiveRawCaptureInput,
    #[serde(default)]
    pub streaming: LiveStreamingInput,
}

#[derive(Debug, Deserialize)]
pub struct LiveNodeInput {
    pub name: String,
    pub trader_id: String,
    #[serde(default = "default_environment")]
    pub environment: String,
    #[serde(default)]
    pub load_state: bool,
    #[serde(default)]
    pub save_state: bool,
}

#[derive(Debug, Deserialize)]
pub struct LiveLoggingInput {
    #[serde(default = "default_stdout_level")]
    pub stdout_level: String,
    #[serde(default = "default_file_level")]
    pub file_level: String,
}

impl Default for LiveLoggingInput {
    fn default() -> Self {
        Self {
            stdout_level: default_stdout_level(),
            file_level: default_file_level(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LiveTimeoutsInput {
    #[serde(default = "default_timeout_connection_secs")]
    pub connection_secs: u64,
    #[serde(default = "default_timeout_reconciliation_secs")]
    pub reconciliation_secs: u64,
    #[serde(default = "default_timeout_portfolio_secs")]
    pub portfolio_secs: u64,
    #[serde(default = "default_timeout_disconnection_secs")]
    pub disconnection_secs: u64,
    #[serde(default = "default_delay_post_stop_secs")]
    pub post_stop_delay_secs: u64,
    #[serde(default = "default_delay_shutdown_secs")]
    pub shutdown_delay_secs: u64,
}

impl Default for LiveTimeoutsInput {
    fn default() -> Self {
        Self {
            connection_secs: default_timeout_connection_secs(),
            reconciliation_secs: default_timeout_reconciliation_secs(),
            portfolio_secs: default_timeout_portfolio_secs(),
            disconnection_secs: default_timeout_disconnection_secs(),
            post_stop_delay_secs: default_delay_post_stop_secs(),
            shutdown_delay_secs: default_delay_shutdown_secs(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LivePolymarketInput {
    #[serde(default = "default_client_name")]
    pub client_name: String,
    pub event_slug: String,
    pub instrument_id: String,
    pub account_id: String,
    pub funder: String,
    #[serde(default = "default_signature_type")]
    pub signature_type: u8,
    #[serde(default)]
    pub subscribe_new_markets: bool,
    #[serde(default = "default_update_instruments_interval_mins")]
    pub update_instruments_interval_mins: u64,
    #[serde(default = "default_ws_max_subscriptions")]
    pub ws_max_subscriptions: usize,
}

#[derive(Debug, Deserialize)]
pub struct LiveStrategyInput {
    #[serde(default = "default_strategy_id")]
    pub strategy_id: String,
    #[serde(default = "default_order_qty")]
    pub order_qty: String,
    #[serde(default)]
    pub log_data: bool,
    #[serde(default = "default_tob_offset_ticks")]
    pub tob_offset_ticks: u64,
    #[serde(default = "default_use_post_only")]
    pub use_post_only: bool,
    #[serde(default)]
    pub enable_limit_sells: bool,
    #[serde(default)]
    pub enable_stop_buys: bool,
    #[serde(default)]
    pub enable_stop_sells: bool,
}

impl Default for LiveStrategyInput {
    fn default() -> Self {
        Self {
            strategy_id: default_strategy_id(),
            order_qty: default_order_qty(),
            log_data: false,
            tob_offset_ticks: default_tob_offset_ticks(),
            use_post_only: default_use_post_only(),
            enable_limit_sells: false,
            enable_stop_buys: false,
            enable_stop_sells: false,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LiveSecretsInput {
    #[serde(default = "default_region")]
    pub region: String,
    pub pk: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
pub struct LiveRawCaptureInput {
    #[serde(default = "default_raw_capture_output_dir")]
    pub output_dir: String,
}

impl Default for LiveRawCaptureInput {
    fn default() -> Self {
        Self {
            output_dir: default_raw_capture_output_dir(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LiveStreamingInput {
    #[serde(default)]
    pub catalog_path: String,
    #[serde(default = "default_streaming_flush_interval_ms")]
    pub flush_interval_ms: u64,
}

impl Default for LiveStreamingInput {
    fn default() -> Self {
        Self {
            catalog_path: String::new(),
            flush_interval_ms: default_streaming_flush_interval_ms(),
        }
    }
}

#[derive(Serialize)]
struct RenderedConfig {
    node: RenderedNodeConfig,
    logging: RenderedLoggingConfig,
    raw_capture: RenderedRawCaptureConfig,
    data_clients: Vec<RenderedDataClientEntry>,
    exec_clients: Vec<RenderedExecClientEntry>,
    strategies: Vec<RenderedStrategyEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    streaming: Option<RenderedStreamingConfig>,
}

#[derive(Serialize)]
struct RenderedNodeConfig {
    name: String,
    trader_id: String,
    environment: String,
    load_state: bool,
    save_state: bool,
    timeout_connection_secs: u64,
    timeout_reconciliation_secs: u64,
    timeout_portfolio_secs: u64,
    timeout_disconnection_secs: u64,
    delay_post_stop_secs: u64,
    delay_shutdown_secs: u64,
}

#[derive(Serialize)]
struct RenderedLoggingConfig {
    stdout_level: String,
    file_level: String,
}

#[derive(Serialize)]
struct RenderedRawCaptureConfig {
    output_dir: String,
}

#[derive(Serialize)]
struct RenderedDataClientEntry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    config: RenderedDataClientConfig,
}

#[derive(Serialize)]
struct RenderedDataClientConfig {
    subscribe_new_markets: bool,
    update_instruments_interval_mins: u64,
    ws_max_subscriptions: usize,
    event_slugs: Vec<String>,
}

#[derive(Serialize)]
struct RenderedExecClientEntry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    config: RenderedExecClientConfig,
    secrets: RenderedSecretsConfig,
}

#[derive(Serialize)]
struct RenderedExecClientConfig {
    account_id: String,
    signature_type: u8,
    funder: String,
}

#[derive(Serialize)]
struct RenderedSecretsConfig {
    region: String,
    pk: String,
    api_key: String,
    api_secret: String,
    passphrase: String,
}

#[derive(Serialize)]
struct RenderedStrategyEntry {
    #[serde(rename = "type")]
    kind: String,
    config: RenderedStrategyConfig,
}

#[derive(Serialize)]
struct RenderedStrategyConfig {
    strategy_id: String,
    instrument_id: String,
    client_id: String,
    order_qty: String,
    log_data: bool,
    tob_offset_ticks: u64,
    use_post_only: bool,
    enable_limit_sells: bool,
    enable_stop_buys: bool,
    enable_stop_sells: bool,
}

#[derive(Serialize)]
struct RenderedStreamingConfig {
    catalog_path: String,
    flush_interval_ms: u64,
}

impl LiveLocalConfig {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
        let config: LiveLocalConfig = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse config file {}: {e}", path.display()))?;
        Ok(config)
    }
}

pub fn render_runtime_config(
    input: &LiveLocalConfig,
    source_path: &Path,
    output_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let rendered = RenderedConfig {
        node: RenderedNodeConfig {
            name: input.node.name.clone(),
            trader_id: input.node.trader_id.clone(),
            environment: input.node.environment.clone(),
            load_state: input.node.load_state,
            save_state: input.node.save_state,
            timeout_connection_secs: input.timeouts.connection_secs,
            timeout_reconciliation_secs: input.timeouts.reconciliation_secs,
            timeout_portfolio_secs: input.timeouts.portfolio_secs,
            timeout_disconnection_secs: input.timeouts.disconnection_secs,
            delay_post_stop_secs: input.timeouts.post_stop_delay_secs,
            delay_shutdown_secs: input.timeouts.shutdown_delay_secs,
        },
        logging: RenderedLoggingConfig {
            stdout_level: input.logging.stdout_level.clone(),
            file_level: input.logging.file_level.clone(),
        },
        raw_capture: RenderedRawCaptureConfig {
            output_dir: input.raw_capture.output_dir.clone(),
        },
        data_clients: vec![RenderedDataClientEntry {
            name: input.polymarket.client_name.clone(),
            kind: "polymarket".to_string(),
            config: RenderedDataClientConfig {
                subscribe_new_markets: input.polymarket.subscribe_new_markets,
                update_instruments_interval_mins: input.polymarket.update_instruments_interval_mins,
                ws_max_subscriptions: input.polymarket.ws_max_subscriptions,
                event_slugs: vec![input.polymarket.event_slug.clone()],
            },
        }],
        exec_clients: vec![RenderedExecClientEntry {
            name: input.polymarket.client_name.clone(),
            kind: "polymarket".to_string(),
            config: RenderedExecClientConfig {
                account_id: input.polymarket.account_id.clone(),
                signature_type: input.polymarket.signature_type,
                funder: input.polymarket.funder.clone(),
            },
            secrets: RenderedSecretsConfig {
                region: input.secrets.region.clone(),
                pk: input.secrets.pk.clone(),
                api_key: input.secrets.api_key.clone(),
                api_secret: input.secrets.api_secret.clone(),
                passphrase: input.secrets.passphrase.clone(),
            },
        }],
        strategies: vec![RenderedStrategyEntry {
            kind: "exec_tester".to_string(),
            config: RenderedStrategyConfig {
                strategy_id: input.strategy.strategy_id.clone(),
                instrument_id: input.polymarket.instrument_id.clone(),
                client_id: input.polymarket.client_name.clone(),
                order_qty: input.strategy.order_qty.clone(),
                log_data: input.strategy.log_data,
                tob_offset_ticks: input.strategy.tob_offset_ticks,
                use_post_only: input.strategy.use_post_only,
                enable_limit_sells: input.strategy.enable_limit_sells,
                enable_stop_buys: input.strategy.enable_stop_buys,
                enable_stop_sells: input.strategy.enable_stop_sells,
            },
        }],
        streaming: if input.streaming.catalog_path.trim().is_empty() {
            None
        } else {
            Some(RenderedStreamingConfig {
                catalog_path: input.streaming.catalog_path.clone(),
                flush_interval_ms: input.streaming.flush_interval_ms,
            })
        },
    };

    let body = toml::to_string_pretty(&rendered)?;
    Ok(format!(
        "# GENERATED FILE - DO NOT EDIT.\n# Source of truth: {}\n# Regenerate with: cargo run --bin render_live_config -- --input {} --output {}\n\n{body}",
        source_path.display(),
        source_path.display(),
        output_path.display()
    ))
}
