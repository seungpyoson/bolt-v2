use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::config::default_raw_capture_output_dir;

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

pub(crate) const DEFAULT_BOOK_INTERVAL_MS: u64 = 1_000;

pub(crate) fn default_book_interval_ms() -> u64 {
    DEFAULT_BOOK_INTERVAL_MS
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

fn default_min_publish_interval_ms() -> u64 {
    100
}

fn default_live_raw_capture_output_dir() -> String {
    default_raw_capture_output_dir()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveLocalConfig {
    #[serde(default)]
    pub node: LiveNodeInput,
    #[serde(default)]
    pub logging: LiveLoggingInput,
    #[serde(default)]
    pub timeouts: LiveTimeoutsInput,
    #[serde(default)]
    pub polymarket: LivePolymarketInput,
    #[serde(default)]
    pub strategy: LiveStrategyInput,
    #[serde(default)]
    pub secrets: LiveSecretsInput,
    #[serde(default)]
    pub raw_capture: LiveRawCaptureInput,
    #[serde(default)]
    pub streaming: LiveStreamingInput,
    #[serde(default)]
    pub reference: LiveReferenceInput,
    #[serde(default)]
    pub rulesets: Vec<LiveRulesetInput>,
    #[serde(default)]
    pub audit: Option<LiveAuditInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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

impl Default for LiveNodeInput {
    fn default() -> Self {
        Self {
            name: String::new(),
            trader_id: String::new(),
            environment: default_environment(),
            load_state: false,
            save_state: false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct LivePolymarketInput {
    #[serde(default = "default_client_name")]
    pub client_name: String,
    #[serde(default)]
    pub event_slug: String,
    #[serde(default)]
    pub instrument_id: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
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

impl Default for LivePolymarketInput {
    fn default() -> Self {
        Self {
            client_name: default_client_name(),
            event_slug: String::new(),
            instrument_id: String::new(),
            account_id: String::new(),
            funder: String::new(),
            signature_type: default_signature_type(),
            subscribe_new_markets: false,
            update_instruments_interval_mins: default_update_instruments_interval_mins(),
            ws_max_subscriptions: default_ws_max_subscriptions(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveStrategyInput {
    #[serde(default = "default_strategy_id")]
    pub strategy_id: String,
    #[serde(default = "default_order_qty")]
    pub order_qty: String,
    #[serde(default)]
    pub log_data: bool,
    #[serde(default)]
    pub subscribe_book: bool,
    #[serde(default = "default_book_interval_ms")]
    pub book_interval_ms: u64,
    #[serde(default)]
    pub open_position_on_start_qty: Option<String>,
    #[serde(default)]
    pub open_position_time_in_force: Option<String>,
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
            subscribe_book: false,
            book_interval_ms: default_book_interval_ms(),
            open_position_on_start_qty: None,
            open_position_time_in_force: None,
            tob_offset_ticks: default_tob_offset_ticks(),
            use_post_only: default_use_post_only(),
            enable_limit_sells: false,
            enable_stop_buys: false,
            enable_stop_sells: false,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveSecretsInput {
    // Whole-section defaults intentionally differ from serde field defaults.
    // `Default::default()` here yields empty strings, while a present
    // `[secrets]` section can still rely on field-level serde defaults.
    #[serde(default = "default_region")]
    pub region: String,
    #[serde(default)]
    pub pk: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub passphrase: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRawCaptureInput {
    #[serde(default = "default_live_raw_capture_output_dir")]
    pub output_dir: String,
}

impl Default for LiveRawCaptureInput {
    fn default() -> Self {
        Self {
            output_dir: default_live_raw_capture_output_dir(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveStreamingInput {
    #[serde(default)]
    pub catalog_path: String,
    #[serde(default = "default_streaming_flush_interval_ms")]
    pub flush_interval_ms: u64,
    #[serde(default)]
    pub contract_path: Option<String>,
}

impl Default for LiveStreamingInput {
    fn default() -> Self {
        Self {
            catalog_path: String::new(),
            flush_interval_ms: default_streaming_flush_interval_ms(),
            contract_path: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LiveReferenceInput {
    #[serde(default)]
    pub publish_topic: String,
    #[serde(default = "default_min_publish_interval_ms")]
    pub min_publish_interval_ms: u64,
    #[serde(default)]
    pub chainlink: Option<crate::config::ChainlinkSharedConfig>,
    #[serde(default)]
    pub venues: Vec<LiveReferenceVenueInput>,
}

impl Default for LiveReferenceInput {
    fn default() -> Self {
        Self {
            publish_topic: String::new(),
            min_publish_interval_ms: default_min_publish_interval_ms(),
            chainlink: None,
            venues: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LiveReferenceVenueInput {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: crate::config::ReferenceVenueKind,
    pub instrument_id: String,
    pub base_weight: f64,
    pub stale_after_ms: u64,
    pub disable_after_ms: u64,
    #[serde(default)]
    pub chainlink: Option<crate::config::ChainlinkReferenceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct LiveRulesetInput {
    pub id: String,
    pub venue: crate::config::RulesetVenueKind,
    pub tag_slug: String,
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
pub struct LiveAuditInput {
    pub local_dir: String,
    pub s3_uri: String,
    pub ship_interval_secs: u64,
    pub upload_attempt_timeout_secs: u64,
    pub roll_max_bytes: u64,
    pub roll_max_secs: u64,
    pub max_local_backlog_bytes: u64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reference: Option<RenderedReferenceConfig>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rulesets: Vec<RenderedRulesetConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    audit: Option<RenderedAuditConfig>,
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
    subscribe_book: bool,
    book_interval_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    open_position_on_start_qty: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    open_position_time_in_force: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    contract_path: Option<String>,
}

#[derive(Serialize)]
struct RenderedReferenceConfig {
    publish_topic: String,
    min_publish_interval_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    chainlink: Option<crate::config::ChainlinkSharedConfig>,
    venues: Vec<RenderedReferenceVenueEntry>,
}

#[derive(Serialize)]
struct RenderedReferenceVenueEntry {
    name: String,
    #[serde(rename = "type")]
    kind: crate::config::ReferenceVenueKind,
    instrument_id: String,
    base_weight: f64,
    stale_after_ms: u64,
    disable_after_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    chainlink: Option<crate::config::ChainlinkReferenceConfig>,
}

#[derive(Serialize)]
struct RenderedRulesetConfig {
    id: String,
    venue: crate::config::RulesetVenueKind,
    tag_slug: String,
    resolution_basis: String,
    min_time_to_expiry_secs: u64,
    max_time_to_expiry_secs: u64,
    min_liquidity_num: f64,
    require_accepting_orders: bool,
    freeze_before_end_secs: u64,
    selector_poll_interval_ms: u64,
    candidate_load_timeout_secs: u64,
}

#[derive(Serialize)]
struct RenderedAuditConfig {
    local_dir: String,
    s3_uri: String,
    ship_interval_secs: u64,
    upload_attempt_timeout_secs: u64,
    roll_max_bytes: u64,
    roll_max_secs: u64,
    max_local_backlog_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterializationOutcome {
    Created,
    Updated,
    PermissionsRepaired,
    Unchanged,
}

impl LiveLocalConfig {
    fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = read_to_string_at(path, "read config file")
            .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))?;
        let config: LiveLocalConfig = toml::from_str(&contents)
            .map_err(|e| format!("Failed to parse config file {}: {e}", path.display()))?;
        Ok(config)
    }
}

pub fn materialize_live_config(
    input_path: &Path,
    output_path: &Path,
) -> Result<MaterializationOutcome, Box<dyn std::error::Error>> {
    let input = LiveLocalConfig::load(input_path)?;

    let validation_errors = crate::validate::validate_live_local(&input);
    if !validation_errors.is_empty() {
        let details: Vec<String> = validation_errors
            .iter()
            .map(|e| format!("  - {e}"))
            .collect();
        return Err(format!(
            "Config validation failed ({} error{}):\n{}",
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

    let rendered = render_runtime_config(&input, input_path)?;
    validate_rendered_runtime_config(&rendered)?;
    materialize_output(output_path, &rendered)
}

fn validate_rendered_runtime_config(rendered: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: crate::config::Config = toml::from_str(rendered)
        .map_err(|e| format!("Failed to parse rendered runtime config: {e}"))?;

    let validation_errors = crate::validate::validate_runtime(&config);
    if validation_errors.is_empty() {
        return Ok(());
    }

    let details: Vec<String> = validation_errors
        .iter()
        .map(|e| format!("  - {e}"))
        .collect();
    Err(format!(
        "Runtime config validation failed ({} error{}):\n{}",
        validation_errors.len(),
        if validation_errors.len() == 1 {
            ""
        } else {
            "s"
        },
        details.join("\n"),
    )
    .into())
}

fn render_runtime_config(
    input: &LiveLocalConfig,
    source_path: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let platform_enabled = !input.rulesets.is_empty();
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
                subscribe_book: input.strategy.subscribe_book,
                book_interval_ms: input.strategy.book_interval_ms,
                open_position_on_start_qty: input.strategy.open_position_on_start_qty.clone(),
                open_position_time_in_force: input
                    .strategy
                    .open_position_time_in_force
                    .clone(),
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
                contract_path: input
                    .streaming
                    .contract_path
                    .as_ref()
                    .filter(|path| !path.trim().is_empty())
                    .map(|p| resolve_rendered_contract_path(source_path, p))
                    .transpose()?,
            })
        },
        reference: platform_enabled.then(|| RenderedReferenceConfig {
            publish_topic: input.reference.publish_topic.clone(),
            min_publish_interval_ms: input.reference.min_publish_interval_ms,
            chainlink: input.reference.chainlink.clone(),
            venues: input
                .reference
                .venues
                .iter()
                .map(|venue| RenderedReferenceVenueEntry {
                    name: venue.name.clone(),
                    kind: venue.kind.clone(),
                    instrument_id: venue.instrument_id.clone(),
                    base_weight: venue.base_weight,
                    stale_after_ms: venue.stale_after_ms,
                    disable_after_ms: venue.disable_after_ms,
                    chainlink: venue.chainlink.clone(),
                })
                .collect(),
        }),
        rulesets: if platform_enabled {
            input
                .rulesets
                .iter()
                .map(|ruleset| RenderedRulesetConfig {
                    id: ruleset.id.clone(),
                    venue: ruleset.venue.clone(),
                    tag_slug: ruleset.tag_slug.clone(),
                    resolution_basis: ruleset.resolution_basis.clone(),
                    min_time_to_expiry_secs: ruleset.min_time_to_expiry_secs,
                    max_time_to_expiry_secs: ruleset.max_time_to_expiry_secs,
                    min_liquidity_num: ruleset.min_liquidity_num,
                    require_accepting_orders: ruleset.require_accepting_orders,
                    freeze_before_end_secs: ruleset.freeze_before_end_secs,
                    selector_poll_interval_ms: ruleset.selector_poll_interval_ms,
                    candidate_load_timeout_secs: ruleset.candidate_load_timeout_secs,
                })
                .collect()
        } else {
            Vec::new()
        },
        audit: if platform_enabled {
            input.audit.as_ref().map(|audit| RenderedAuditConfig {
                local_dir: audit.local_dir.clone(),
                s3_uri: audit.s3_uri.clone(),
                ship_interval_secs: audit.ship_interval_secs,
                upload_attempt_timeout_secs: audit.upload_attempt_timeout_secs,
                roll_max_bytes: audit.roll_max_bytes,
                roll_max_secs: audit.roll_max_secs,
                max_local_backlog_bytes: audit.max_local_backlog_bytes,
            })
        } else {
            None
        },
    };

    let body = toml::to_string_pretty(&rendered)?;
    Ok(format!(
        "# GENERATED FILE - DO NOT EDIT.\n# Source of truth: {}\n\n{body}",
        source_path.display(),
    ))
}

fn resolve_repo_root(source_path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let anchored = if source_path.is_absolute() {
        source_path.to_path_buf()
    } else {
        cwd.join(source_path)
    };

    let start = anchored.parent().unwrap_or(anchored.as_path());
    for candidate in start.ancestors() {
        if candidate.join("Cargo.toml").is_file() {
            return Ok(fs::canonicalize(candidate)?);
        }
    }

    for candidate in cwd.ancestors() {
        if candidate.join("Cargo.toml").is_file() {
            return Ok(fs::canonicalize(candidate)?);
        }
    }

    Err(std::io::Error::other(format!(
        "unable to determine repo root for {}",
        source_path.display()
    ))
    .into())
}

fn resolve_rendered_contract_path(
    source_path: &Path,
    raw_path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let path = Path::new(raw_path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let repo_root = resolve_repo_root(source_path)?;
        repo_root.join(path)
    };

    let normalized = crate::venue_contract::normalize_local_absolute_contract_path(&absolute)?;
    Ok(normalized.to_string_lossy().to_string())
}

fn materialize_output(
    path: &Path,
    contents: &str,
) -> Result<MaterializationOutcome, Box<dyn std::error::Error>> {
    ensure_parent_dir(path)?;

    if !path.exists() {
        write_output(path, contents)?;
        return Ok(MaterializationOutcome::Created);
    }

    let existing = read_to_string_at(path, "read existing config file").map_err(|e| {
        format!(
            "Failed to read existing config file {}: {e}",
            path.display()
        )
    })?;

    if existing != contents {
        write_output(path, contents)?;
        return Ok(MaterializationOutcome::Updated);
    }

    if !is_read_only(path)? {
        set_read_only(path)?;
        return Ok(MaterializationOutcome::PermissionsRepaired);
    }

    Ok(MaterializationOutcome::Unchanged)
}

fn ensure_parent_dir(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        create_dir_all_at(parent, "create output directory")?;
    }
    Ok(())
}

fn write_output(path: &Path, contents: &str) -> Result<(), Box<dyn std::error::Error>> {
    ensure_parent_dir(path)?;

    #[cfg(unix)]
    let target_mode = existing_read_only_mode(path)?;

    let staged = staged_output_path(path)?;
    write_contents_at(&staged, contents, "stage rendered config")?;
    #[cfg(unix)]
    set_staged_read_only(&staged, target_mode)?;
    #[cfg(not(unix))]
    set_read_only(&staged)?;

    #[cfg(windows)]
    if path.exists() {
        set_writable(path)?;
        remove_file_at(path, "replace existing output file")?;
    }

    rename_path(&staged, path, "promote staged config")?;
    Ok(())
}

fn staged_output_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let raw_parent = path
        .parent()
        .ok_or_else(|| format!("Output path has no parent: {}", path.display()))?;
    let parent = if raw_parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        raw_parent
    };
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Output path has no file name: {}", path.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(parent.join(format!(
        ".{}.tmp-{}-{}",
        filename,
        std::process::id(),
        stamp
    )))
}

#[cfg(windows)]
fn set_writable(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = metadata_at(path, "inspect file permissions")?.permissions();
    permissions.set_readonly(false);
    set_permissions_at(path, permissions, "mark file writable")?;
    Ok(())
}

#[cfg(unix)]
fn is_read_only(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = metadata_at(path, "inspect file permissions")?;
    Ok(metadata.permissions().mode() & 0o222 == 0)
}

#[cfg(not(unix))]
fn is_read_only(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let metadata = metadata_at(path, "inspect file permissions")?;
    Ok(metadata.permissions().readonly())
}

#[cfg(unix)]
fn existing_read_only_mode(path: &Path) -> Result<Option<u32>, Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    if path.exists() {
        let mode = metadata_at(path, "inspect existing output mode")?
            .permissions()
            .mode();
        return Ok(Some(mode & !0o222));
    }

    Ok(None)
}

#[cfg(unix)]
fn set_staged_read_only(
    path: &Path,
    target_mode: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = metadata_at(path, "inspect staged file permissions")?.permissions();
    let current_mode = permissions.mode();
    let read_only_mode = match target_mode {
        Some(mode) => mode,
        None => current_mode & !0o222,
    };

    permissions.set_mode(read_only_mode);
    set_permissions_at(path, permissions, "mark staged file read-only")?;
    Ok(())
}

#[cfg(unix)]
fn set_read_only(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = metadata_at(path, "inspect file permissions")?.permissions();
    let current_mode = permissions.mode();
    let read_only_mode = current_mode & !0o222;

    permissions.set_mode(read_only_mode);
    set_permissions_at(path, permissions, "mark file read-only")?;
    Ok(())
}

#[cfg(not(unix))]
fn set_read_only(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut permissions = metadata_at(path, "inspect file permissions")?.permissions();
    permissions.set_readonly(true);
    set_permissions_at(path, permissions, "mark file read-only")?;
    Ok(())
}

#[cfg(not(unix))]
fn set_staged_read_only(
    path: &Path,
    _target_mode: Option<u32>,
) -> Result<(), Box<dyn std::error::Error>> {
    set_read_only(path)
}

fn create_dir_all_at(path: &Path, action: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(path)
        .map_err(|e| format!("Failed to {action} {}: {e}", path.display()))?;
    Ok(())
}

fn read_to_string_at(path: &Path, action: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to {action} {}: {e}", path.display()))?)
}

fn write_contents_at(
    path: &Path,
    contents: &str,
    action: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::write(path, contents)
        .map_err(|e| format!("Failed to {action} {}: {e}", path.display()))?;
    Ok(())
}

fn metadata_at(path: &Path, action: &str) -> Result<std::fs::Metadata, Box<dyn std::error::Error>> {
    Ok(std::fs::metadata(path)
        .map_err(|e| format!("Failed to {action} {}: {e}", path.display()))?)
}

fn set_permissions_at(
    path: &Path,
    permissions: std::fs::Permissions,
    action: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::set_permissions(path, permissions)
        .map_err(|e| format!("Failed to {action} {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
fn remove_file_at(path: &Path, action: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::remove_file(path)
        .map_err(|e| format!("Failed to {action} {}: {e}", path.display()))?;
    Ok(())
}

fn rename_path(from: &Path, to: &Path, action: &str) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::rename(from, to).map_err(|e| {
        format!(
            "Failed to {action} {} -> {}: {e}",
            from.display(),
            to.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn repo_path(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn tracked_template_renders_expected_runtime_mapping() {
        let source_path = repo_path("config/live.local.example.toml");
        let input = LiveLocalConfig::load(&source_path).expect("tracked template should load");
        let rendered =
            render_runtime_config(&input, &source_path).expect("tracked template should render");
        let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");

        assert!(rendered.contains("# GENERATED FILE - DO NOT EDIT."));
        assert!(rendered.contains(&format!("# Source of truth: {}", source_path.display())));
        assert!(!rendered.contains("Regenerate with:"));

        assert_eq!(
            cfg.node.timeout_connection_secs,
            input.timeouts.connection_secs
        );
        assert_eq!(
            cfg.node.timeout_reconciliation_secs,
            input.timeouts.reconciliation_secs
        );
        assert_eq!(
            cfg.node.timeout_portfolio_secs,
            input.timeouts.portfolio_secs
        );
        assert_eq!(
            cfg.node.timeout_disconnection_secs,
            input.timeouts.disconnection_secs
        );
        assert_eq!(
            cfg.node.delay_post_stop_secs,
            input.timeouts.post_stop_delay_secs
        );
        assert_eq!(
            cfg.node.delay_shutdown_secs,
            input.timeouts.shutdown_delay_secs
        );

        let client_name = input.polymarket.client_name.as_str();
        assert_eq!(cfg.data_clients.len(), 1);
        assert_eq!(cfg.exec_clients.len(), 1);
        assert_eq!(cfg.strategies.len(), 1);
        assert_eq!(cfg.data_clients[0].name, client_name);
        assert_eq!(cfg.data_clients[0].kind, "polymarket");
        assert_eq!(cfg.exec_clients[0].name, client_name);
        assert_eq!(cfg.exec_clients[0].kind, "polymarket");
        assert_eq!(cfg.strategies[0].kind, "exec_tester");
        assert_eq!(
            cfg.strategies[0].config["client_id"]
                .as_str()
                .expect("strategy client_id should exist"),
            client_name
        );
        assert_eq!(
            cfg.data_clients[0].config["event_slugs"]
                .as_array()
                .expect("event slugs should exist"),
            &vec![toml::Value::String(input.polymarket.event_slug.clone())]
        );
        assert_eq!(
            cfg.strategies[0].config["instrument_id"]
                .as_str()
                .expect("instrument_id should exist"),
            input.polymarket.instrument_id
        );
        assert_eq!(
            cfg.exec_clients[0].config["signature_type"]
                .as_integer()
                .expect("signature_type should exist"),
            i64::from(input.polymarket.signature_type)
        );
        assert_eq!(
            cfg.exec_clients[0].config["funder"]
                .as_str()
                .expect("funder should exist"),
            input.polymarket.funder
        );
        assert_eq!(cfg.exec_clients[0].secrets.region, input.secrets.region);
        assert_eq!(
            cfg.exec_clients[0].secrets.pk.as_deref(),
            Some(input.secrets.pk.as_str())
        );
        assert_eq!(
            cfg.exec_clients[0].secrets.api_key.as_deref(),
            Some(input.secrets.api_key.as_str())
        );
        assert_eq!(
            cfg.exec_clients[0].secrets.api_secret.as_deref(),
            Some(input.secrets.api_secret.as_str())
        );
        assert_eq!(
            cfg.exec_clients[0].secrets.passphrase.as_deref(),
            Some(input.secrets.passphrase.as_str())
        );
    }

    #[test]
    fn minimal_operator_input_uses_defaults_and_renders_valid_runtime_config() {
        let raw = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"
"#;

        let input: LiveLocalConfig =
            toml::from_str(raw).expect("minimal operator config should parse");
        let rendered = render_runtime_config(&input, &repo_path("config/live.local.toml"))
            .expect("minimal operator config should render");
        let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");

        assert_eq!(cfg.node.environment, "Live");
        assert_eq!(cfg.logging.stdout_level, "Info");
        assert_eq!(cfg.logging.file_level, "Debug");
        assert_eq!(cfg.node.timeout_connection_secs, 60);
        assert_eq!(cfg.node.timeout_reconciliation_secs, 60);
        assert_eq!(cfg.node.timeout_portfolio_secs, 10);
        assert_eq!(cfg.node.timeout_disconnection_secs, 10);
        assert_eq!(cfg.node.delay_post_stop_secs, 5);
        assert_eq!(cfg.node.delay_shutdown_secs, 5);
        assert_eq!(cfg.data_clients[0].name, "POLYMARKET");
        assert_eq!(cfg.exec_clients[0].name, "POLYMARKET");
        assert_eq!(cfg.strategies[0].kind, "exec_tester");
        assert_eq!(
            cfg.strategies[0].config["strategy_id"]
                .as_str()
                .expect("strategy_id should exist"),
            "EXEC_TESTER-001"
        );
        assert_eq!(
            cfg.exec_clients[0].config["signature_type"]
                .as_integer()
                .expect("signature_type should exist"),
            2
        );
        assert_eq!(cfg.exec_clients[0].secrets.region, "eu-west-1");
    }

    #[test]
    fn relative_streaming_contract_path_resolves_from_repo_root() {
        let raw = r#"
[node]
name = "BOLT-V2-TEST"
trader_id = "BOLT-TEST"

[polymarket]
event_slug = "btc-updown-5m"
instrument_id = "0xabc-12345678901234567890.POLYMARKET"
account_id = "POLYMARKET-001"
funder = "0xabc"

[secrets]
pk = "/bolt/poly/pk"
api_key = "/bolt/poly/key"
api_secret = "/bolt/poly/secret"
passphrase = "/bolt/poly/passphrase"

[streaming]
catalog_path = "var/catalog"
contract_path = "contracts/polymarket.toml"
"#;

        let input: LiveLocalConfig =
            toml::from_str(raw).expect("minimal operator config should parse");
        let tempdir = tempdir().expect("tempdir should be created");
        std::fs::write(
            tempdir.path().join("Cargo.toml"),
            "[package]\nname = \"temp\"\n",
        )
        .expect("repo marker should exist");
        let source_dir = tempdir.path().join("config");
        std::fs::create_dir_all(&source_dir).expect("source dir should be created");
        let source_path = source_dir.join("live.local.toml");
        let rendered =
            render_runtime_config(&input, &source_path).expect("operator config should render");
        let cfg: Config = toml::from_str(&rendered).expect("rendered config should parse");
        let expected_root = std::fs::canonicalize(tempdir.path()).expect("tempdir should resolve");

        assert_eq!(
            cfg.streaming.contract_path.as_deref(),
            Some(
                expected_root
                    .join("contracts/polymarket.toml")
                    .to_str()
                    .expect("resolved contract path should be valid UTF-8")
            )
        );
    }
}
