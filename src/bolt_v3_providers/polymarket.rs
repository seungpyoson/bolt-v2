//! Per-provider binding for `polymarket` venue config block shapes
//! and per-venue startup validation.
//!
//! Owns the concrete shape of `[venues.<name>.data]`,
//! `[venues.<name>.execution]`, and `[venues.<name>.secrets]` for any
//! venue whose `kind = "polymarket"` provider key is configured. Core
//! config in `crate::bolt_v3_config` only owns the root/strategy envelope
//! and raw provider-key field; the provider-shaped block types and their
//! serde rules live here so provider-specific schema evolution does not
//! reach back into the envelope module.
//!
//! This module also owns the per-venue startup-validation policy for
//! Polymarket venues: typed deserialization of each present block,
//! cross-block presence rule ([secrets] is only allowed alongside
//! [execution]), Polymarket data/execution bounds, EVM funder-address
//! syntax, and Polymarket secret-path ownership. The cross-provider rule
//! that [execution] requires [secrets] is declared by
//! [`REQUIRED_SECRET_BLOCKS`] and enforced centrally in
//! `bolt_v3_providers::validate_venue_block`. Core startup validation in
//! `crate::bolt_v3_validate`
//! dispatches into `bolt_v3_providers::validate_venue_block`, which
//! routes Polymarket venues here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

mod fees;

use std::{any::Any, sync::Arc};

use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_network::websocket::TransportBackend;
use nautilus_polymarket::{
    common::credential::{EvmPrivateKey, Secrets as PolymarketSecrets},
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{InstrumentFilter, MarketSlugFilter, NewMarketPredicateFilter},
    http::clob::PolymarketClobHttpClient,
};
use serde::Deserialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig, BoltV3InstrumentFilterClockFn,
        BoltV3VenueAdapterConfig,
    },
    bolt_v3_config::VenueBlock,
    bolt_v3_instrument_filters::InstrumentFilterConfig,
    bolt_v3_market_families::updown::{self, updown_market_slug, updown_period_pair},
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderBinding, ProviderCredentialedBlock,
        ProviderResolvedSecrets, ProviderSecretRequirement, ProviderSecretResolveContext,
        ResolvedVenueSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    strategies::registry::FeeProvider,
};

use self::fees::PolymarketClobFeeProvider;

pub const KEY: &str = "polymarket";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[updown::KEY];
const URL_SAFE_BASE64_BLOCK_WIDTH: usize = 4;
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Execution,
    consumer: "Polymarket execution venue",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &[
    "private_key_ssm_path",
    "api_key_ssm_path",
    "api_secret_ssm_path",
    "passphrase_ssm_path",
];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_polymarket::common::credential"];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "POLYMARKET_PK",
    "POLYMARKET_FUNDER",
    "POLYMARKET_API_KEY",
    "POLYMARKET_API_SECRET",
    "POLYMARKET_PASSPHRASE",
];

pub const BINDING: ProviderBinding = ProviderBinding {
    key: KEY,
    validate_venue,
    supported_market_families: SUPPORTED_MARKET_FAMILIES,
    required_secret_blocks: REQUIRED_SECRET_BLOCKS,
    secret_field_names: SECRET_FIELD_NAMES,
    credential_log_modules: CREDENTIAL_LOG_MODULES,
    forbidden_env_vars: FORBIDDEN_ENV_VARS,
    resolve_secrets,
    map_adapters,
    build_fee_provider: Some(build_fee_provider),
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketDataConfig {
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_gamma: String,
    pub base_url_data_api: String,
    pub http_timeout_seconds: u64,
    pub ws_timeout_seconds: u64,
    pub subscribe_new_markets: bool,
    pub auto_load_missing_instruments: bool,
    pub update_instruments_interval_minutes: u64,
    pub websocket_max_subscriptions_per_connection: u64,
    pub auto_load_debounce_milliseconds: u64,
    /// Required WebSocket transport backend passed through to NT so
    /// Bolt-v3 does not inherit the NT adapter default.
    pub transport_backend: TransportBackend,
    pub new_market_filter: Option<PolymarketNewMarketFilterConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PolymarketNewMarketFilterConfig {
    Keyword { keyword: String },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketExecutionConfig {
    pub account_id: String,
    pub signature_type: PolymarketSignatureType,
    /// Public funder address. Required when `signature_type` is
    /// `poly_proxy` or `poly_gnosis_safe` (the proxy/safe routes the
    /// underlying funder wallet); permitted to be absent for `eoa`,
    /// where the EOA is itself the funder. Validation enforces this
    /// per-signature-type requirement and the EVM address syntax.
    pub funder_address: Option<String>,
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_data_api: String,
    pub http_timeout_seconds: u64,
    pub max_retries: u64,
    pub retry_delay_initial_milliseconds: u64,
    pub retry_delay_max_milliseconds: u64,
    pub ack_timeout_seconds: u64,
    pub fee_cache_ttl_seconds: u64,
    /// Required WebSocket transport backend passed through to NT so
    /// Bolt-v3 does not inherit the NT adapter default.
    pub transport_backend: TransportBackend,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolymarketSignatureType {
    Eoa,
    PolyProxy,
    PolyGnosisSafe,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketSecretsConfig {
    pub private_key_ssm_path: String,
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
    pub passphrase_ssm_path: String,
}

#[derive(Clone)]
pub struct ResolvedBoltV3PolymarketSecrets {
    pub private_key: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
}

struct RedactedDebug;

impl std::fmt::Debug for RedactedDebug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Debug for ResolvedBoltV3PolymarketSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;
        f.debug_struct("ResolvedBoltV3PolymarketSecrets")
            .field("private_key", &redacted)
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
            .field("passphrase", &redacted)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3PolymarketSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn redaction_values(&self) -> Vec<&str> {
        vec![
            self.private_key.as_str(),
            self.api_key.as_str(),
            self.api_secret.as_str(),
            self.passphrase.as_str(),
        ]
    }
}

pub fn validate_venue(key: &str, venue: &VenueBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(data) = &venue.data {
        match data.clone().try_into::<PolymarketDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.data: {message}")),
        }
    }
    if let Some(execution) = &venue.execution {
        match execution.clone().try_into::<PolymarketExecutionConfig>() {
            Ok(parsed) => {
                if parsed.account_id.trim().is_empty() {
                    errors.push(format!(
                        "venues.{key}.execution.account_id must be a non-empty string"
                    ));
                }
                errors.extend(validate_funder_address(key, &parsed));
                errors.extend(validate_execution_bounds(key, &parsed));
            }
            Err(message) => {
                errors.push(format!("venues.{key}.execution: {message}"));
            }
        }
    }
    if let Some(secrets) = &venue.secrets {
        // Only Polymarket execution consumes Polymarket credentials. A
        // data-only Polymarket venue with `[secrets]` would carry
        // credential paths that no adapter uses, which is a
        // misconfiguration rather than a silent no-op.
        if venue.execution.is_none() {
            errors.push(format!(
                "venues.{key} (kind=polymarket) declares [secrets] but no [execution] block is configured; \
                 Polymarket [secrets] are only allowed alongside the execution adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<PolymarketSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_funder_address(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(message) = funder_address_violation(execution) {
        errors.push(format!("venues.{key}.execution.funder_address {message}"));
    }
    errors
}

fn funder_address_violation(execution: &PolymarketExecutionConfig) -> Option<String> {
    let funder = execution
        .funder_address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let requires_funder = matches!(
        execution.signature_type,
        PolymarketSignatureType::PolyProxy | PolymarketSignatureType::PolyGnosisSafe
    );
    match (requires_funder, funder) {
        (true, None) => Some(
            "is required when signature_type is `poly_proxy` or `poly_gnosis_safe`".to_string(),
        ),
        (_, Some(value)) => check_evm_address_syntax(value)
            .err()
            .map(|message| format!("is not a valid EVM public address ({message}): `{value}`")),
        (false, None) => None,
    }
}

fn check_evm_address_syntax(value: &str) -> Result<(), &'static str> {
    let rest = value.strip_prefix("0x").ok_or("missing `0x` prefix")?;
    if rest.len() != 40 {
        return Err("must be 40 hex characters after `0x`");
    }
    if !rest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("must contain only hex characters after `0x`");
    }
    if rest.chars().all(|c| c == '0') {
        return Err("zero address is not allowed");
    }
    Ok(())
}

fn validate_data_bounds(key: &str, data: &PolymarketDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    push_empty_url_errors(
        key,
        "data",
        &[
            ("base_url_http", data.base_url_http.as_str()),
            ("base_url_ws", data.base_url_ws.as_str()),
            ("base_url_gamma", data.base_url_gamma.as_str()),
            ("base_url_data_api", data.base_url_data_api.as_str()),
        ],
        &mut errors,
    );
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_seconds", data.http_timeout_seconds),
        ("ws_timeout_seconds", data.ws_timeout_seconds),
        (
            "update_instruments_interval_minutes",
            data.update_instruments_interval_minutes,
        ),
        (
            "websocket_max_subscriptions_per_connection",
            data.websocket_max_subscriptions_per_connection,
        ),
        (
            "auto_load_debounce_milliseconds",
            data.auto_load_debounce_milliseconds,
        ),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "venues.{key}.data.{field} must be a positive integer"
            ));
        }
    }
    if data.subscribe_new_markets {
        errors.push(format!(
            "venues.{key}.data.subscribe_new_markets must be false in the current controlled-loading scope because pinned NT subscribes to all Polymarket markets when this flag is true"
        ));
    }
    if data.auto_load_missing_instruments {
        errors.push(format!(
            "venues.{key}.data.auto_load_missing_instruments must be false in the current controlled-loading scope because missing-instrument auto-load can trigger ad-hoc Polymarket instrument discovery"
        ));
    }
    if let Some(PolymarketNewMarketFilterConfig::Keyword { keyword }) = &data.new_market_filter
        && keyword.trim().is_empty()
    {
        errors.push(format!(
            "venues.{key}.data.new_market_filter.keyword must be non-empty"
        ));
    }
    errors
}

fn validate_execution_bounds(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    push_empty_url_errors(
        key,
        "execution",
        &[
            ("base_url_http", execution.base_url_http.as_str()),
            ("base_url_ws", execution.base_url_ws.as_str()),
            ("base_url_data_api", execution.base_url_data_api.as_str()),
        ],
        &mut errors,
    );
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_seconds", execution.http_timeout_seconds),
        ("max_retries", execution.max_retries),
        (
            "retry_delay_initial_milliseconds",
            execution.retry_delay_initial_milliseconds,
        ),
        (
            "retry_delay_max_milliseconds",
            execution.retry_delay_max_milliseconds,
        ),
        ("ack_timeout_seconds", execution.ack_timeout_seconds),
        ("fee_cache_ttl_seconds", execution.fee_cache_ttl_seconds),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "venues.{key}.execution.{field} must be a positive integer"
            ));
        }
    }
    if execution.max_retries > u64::from(u32::MAX) {
        errors.push(format!(
            "venues.{key}.execution.max_retries must fit in u32 expected by NT"
        ));
    }
    if let Some(message) = retry_delay_order_violation(
        execution.retry_delay_initial_milliseconds,
        execution.retry_delay_max_milliseconds,
    ) {
        errors.push(format!("venues.{key}.execution.{message}"));
    }
    errors
}

fn retry_delay_order_violation(initial_milliseconds: u64, max_milliseconds: u64) -> Option<String> {
    (initial_milliseconds > max_milliseconds).then(|| {
        format!(
            "retry_delay_initial_milliseconds ({initial_milliseconds}) must be <= retry_delay_max_milliseconds ({max_milliseconds})"
        )
    })
}

fn push_empty_url_errors(
    key: &str,
    block: &str,
    url_fields: &[(&str, &str)],
    errors: &mut Vec<String>,
) {
    for (field, value) in url_fields {
        if value.trim().is_empty() {
            errors.push(format!(
                "venues.{key}.{block}.{field} must be a non-empty URL"
            ));
        }
    }
}

fn validate_secret_paths(key: &str, secrets: &PolymarketSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("private_key_ssm_path", &secrets.private_key_ssm_path),
        ("api_key_ssm_path", &secrets.api_key_ssm_path),
        ("api_secret_ssm_path", &secrets.api_secret_ssm_path),
        ("passphrase_ssm_path", &secrets.passphrase_ssm_path),
    ];
    for (field, value) in path_fields {
        errors.extend(crate::bolt_v3_validate::validate_ssm_parameter_path(
            key, field, value,
        ));
    }
    errors
}

pub fn resolve_secrets(
    context: ProviderSecretResolveContext<'_>,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<ResolvedVenueSecrets, BoltV3SecretError> {
    let secrets_value = context
        .venue
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            venue_key: context.venue_key.to_string(),
            field: "secrets".to_string(),
            ssm_path: String::new(),
            source: "missing [secrets] block".to_string(),
        })?;
    let secrets: PolymarketSecretsConfig =
        secrets_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| BoltV3SecretError {
                venue_key: context.venue_key.to_string(),
                field: KEY.to_string(),
                ssm_path: String::new(),
                source: format!("invalid polymarket secrets schema: {error}"),
            })?;
    let private_key = resolve_field(
        context.venue_key,
        "private_key_ssm_path",
        context.region,
        &secrets.private_key_ssm_path,
        resolver,
    )?;
    if let Err(reason) = validate_private_key_shape(&private_key) {
        return Err(BoltV3SecretError {
            venue_key: context.venue_key.to_string(),
            field: "private_key_ssm_path".to_string(),
            ssm_path: secrets.private_key_ssm_path.clone(),
            source: format!(
                "resolved polymarket private_key is not valid EVM private key material accepted by the NautilusTrader polymarket adapter: {reason}"
            ),
        });
    }
    let api_key = resolve_field(
        context.venue_key,
        "api_key_ssm_path",
        context.region,
        &secrets.api_key_ssm_path,
        resolver,
    )?;
    let api_secret_raw = resolve_field(
        context.venue_key,
        "api_secret_ssm_path",
        context.region,
        &secrets.api_secret_ssm_path,
        resolver,
    )?;
    let api_secret = normalize_api_secret_padding(api_secret_raw);
    let passphrase = resolve_field(
        context.venue_key,
        "passphrase_ssm_path",
        context.region,
        &secrets.passphrase_ssm_path,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3PolymarketSecrets {
        private_key,
        api_key,
        api_secret,
        passphrase,
    }))
}

fn validate_private_key_shape(private_key: &str) -> Result<(), String> {
    EvmPrivateKey::new(private_key)
        .map(|_| ())
        .map_err(|source| source.to_string())
}

fn normalize_api_secret_padding(mut api_secret: String) -> String {
    let pad_len = (URL_SAFE_BASE64_BLOCK_WIDTH - api_secret.len() % URL_SAFE_BASE64_BLOCK_WIDTH)
        % URL_SAFE_BASE64_BLOCK_WIDTH;
    api_secret.extend(std::iter::repeat_n('=', pad_len));
    api_secret
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.venue.data {
        Some(value) => Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(PolymarketDataClientFactory),
            config: Box::new(map_data(
                context.venue_key,
                value,
                context.instrument_filters,
                context.clock,
            )?),
        }),
        None => None,
    };
    let execution = match &context.venue.execution {
        Some(value) => {
            let secrets = secrets_for(context.venue_key, context.resolved)?;
            Some(BoltV3ExecutionClientAdapterConfig {
                factory: Box::new(PolymarketExecutionClientFactory),
                config: Box::new(map_execution(
                    context.root,
                    context.venue_key,
                    value,
                    secrets,
                )?),
            })
        }
        None => None,
    };
    Ok(BoltV3VenueAdapterConfig { data, execution })
}

pub fn build_fee_provider(
    venue_key: &str,
    venue: &VenueBlock,
    resolved: &crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, BoltV3AdapterMappingError> {
    let value =
        venue
            .execution
            .as_ref()
            .ok_or_else(|| BoltV3AdapterMappingError::ValidationInvariant {
                venue_key: venue_key.to_string(),
                field: "execution",
                message: "is required before building the configured fee provider".to_string(),
            })?;
    let cfg = parse_execution_config(venue_key, value)?;
    reject_funder_address_violation(venue_key, &cfg)?;
    let secrets = secrets_for(venue_key, resolved)?;
    let secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.clone()),
        Some(secrets.api_secret.clone()),
        Some(secrets.passphrase.clone()),
        cfg.funder_address.clone(),
    )
    .map_err(|error| BoltV3AdapterMappingError::ValidationInvariant {
        venue_key: venue_key.to_string(),
        field: "execution",
        message: format!("failed to resolve Polymarket fee credentials: {error}"),
    })?;
    let client = PolymarketClobHttpClient::new(
        secrets.credential,
        secrets.address,
        Some(cfg.base_url_http),
        cfg.http_timeout_seconds,
    )
    .map_err(|error| BoltV3AdapterMappingError::ValidationInvariant {
        venue_key: venue_key.to_string(),
        field: "execution.base_url_http",
        message: format!("failed to create Polymarket fee HTTP client: {error}"),
    })?;

    Ok(Arc::new(PolymarketClobFeeProvider::new(
        client,
        std::time::Duration::from_secs(cfg.fee_cache_ttl_seconds),
    )))
}

fn parse_execution_config(
    venue_key: &str,
    value: &toml::Value,
) -> Result<PolymarketExecutionConfig, BoltV3AdapterMappingError> {
    value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            venue_key: venue_key.to_string(),
            block: "execution",
            message: error.to_string(),
        }
    })
}

fn map_data(
    venue_key: &str,
    value: &toml::Value,
    instrument_filters: &InstrumentFilterConfig,
    clock: Option<BoltV3InstrumentFilterClockFn>,
) -> Result<PolymarketDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                venue_key: venue_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    let ws_max_subscriptions = usize::try_from(cfg.websocket_max_subscriptions_per_connection)
        .map_err(|_| BoltV3AdapterMappingError::NumericRange {
            venue_key: venue_key.to_string(),
            field: "data.websocket_max_subscriptions_per_connection",
            message: format!(
                "value {} does not fit in usize on this target",
                cfg.websocket_max_subscriptions_per_connection
            ),
        })?;
    let filters = build_market_slug_filters_for_venue(instrument_filters, venue_key, clock)?;
    let new_market_filter = map_new_market_filter(venue_key, cfg.new_market_filter.as_ref())?;
    if cfg.subscribe_new_markets {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "data.subscribe_new_markets",
            message: "must be false in the current controlled-loading scope because pinned NT subscribes to all Polymarket markets when this flag is true"
                .to_string(),
        });
    }
    if cfg.auto_load_missing_instruments {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "data.auto_load_missing_instruments",
            message: "must be false in the current controlled-loading scope because missing-instrument auto-load can trigger ad-hoc Polymarket instrument discovery"
                .to_string(),
        });
    }
    Ok(PolymarketDataClientConfig {
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        base_url_gamma: Some(cfg.base_url_gamma),
        base_url_data_api: Some(cfg.base_url_data_api),
        http_timeout_secs: cfg.http_timeout_seconds,
        ws_timeout_secs: cfg.ws_timeout_seconds,
        ws_max_subscriptions,
        update_instruments_interval_mins: cfg.update_instruments_interval_minutes,
        subscribe_new_markets: cfg.subscribe_new_markets,
        auto_load_missing_instruments: cfg.auto_load_missing_instruments,
        auto_load_debounce_ms: cfg.auto_load_debounce_milliseconds,
        transport_backend: cfg.transport_backend,
        filters,
        new_market_filter,
    })
}

fn map_new_market_filter(
    venue_key: &str,
    filter: Option<&PolymarketNewMarketFilterConfig>,
) -> Result<Option<Arc<dyn InstrumentFilter>>, BoltV3AdapterMappingError> {
    match filter {
        Some(PolymarketNewMarketFilterConfig::Keyword { keyword }) => {
            let keyword = keyword.trim();
            if keyword.is_empty() {
                return Err(BoltV3AdapterMappingError::ValidationInvariant {
                    venue_key: venue_key.to_string(),
                    field: "data.new_market_filter.keyword",
                    message: "must be non-empty".to_string(),
                });
            }
            Ok(Some(Arc::new(NewMarketPredicateFilter::keyword(
                keyword.to_string(),
            ))))
        }
        None => Ok(None),
    }
}

fn build_market_slug_filters_for_venue(
    instrument_filters: &InstrumentFilterConfig,
    venue_key: &str,
    clock: Option<BoltV3InstrumentFilterClockFn>,
) -> Result<Vec<Arc<dyn InstrumentFilter>>, BoltV3AdapterMappingError> {
    let mut filters = Vec::new();
    for target in instrument_filters
        .target_refs()
        .filter(|target| SUPPORTED_MARKET_FAMILIES.contains(&target.family_key))
        .filter(|target| target.venue == venue_key)
    {
        let clock = clock.as_ref().cloned().ok_or_else(|| {
            BoltV3AdapterMappingError::ValidationInvariant {
                venue_key: venue_key.to_string(),
                field: "strategy.venue",
                message: format!(
                    "configured target `{}` requires a real instrument-filter clock",
                    target.configured_target_id
                ),
            }
        })?;
        filters.push(build_market_slug_filter(target, clock));
    }
    Ok(filters)
}

fn build_market_slug_filter(
    target: crate::bolt_v3_instrument_filters::InstrumentFilterTargetRef<'_>,
    clock: BoltV3InstrumentFilterClockFn,
) -> Arc<dyn InstrumentFilter> {
    let asset = target.underlying_asset.to_string();
    let token = target.cadence_slug_token.to_string();
    let cadence = target.cadence_seconds;
    Arc::new(MarketSlugFilter::new(move || {
        let now = (clock)();
        match updown_period_pair(cadence, now) {
            Ok((current, next)) => vec![
                updown_market_slug(&asset, &token, current),
                updown_market_slug(&asset, &token, next),
            ],
            Err(error) => {
                log::warn!(
                    "bolt-v3 provider binding: skipping updown filter cycle (cadence={cadence}, now_unix_seconds={now}): {error}"
                );
                Vec::new()
            }
        }
    }))
}

fn reject_funder_address_violation(
    venue_key: &str,
    cfg: &PolymarketExecutionConfig,
) -> Result<(), BoltV3AdapterMappingError> {
    if let Some(message) = funder_address_violation(cfg) {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "execution.funder_address",
            message,
        });
    }
    Ok(())
}

fn map_execution(
    root: &crate::bolt_v3_config::BoltV3RootConfig,
    venue_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3PolymarketSecrets,
) -> Result<PolymarketExecClientConfig, BoltV3AdapterMappingError> {
    let cfg = parse_execution_config(venue_key, value)?;
    reject_funder_address_violation(venue_key, &cfg)?;
    if let Some(message) = retry_delay_order_violation(
        cfg.retry_delay_initial_milliseconds,
        cfg.retry_delay_max_milliseconds,
    ) {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "execution.retry_delay_initial_milliseconds",
            message,
        });
    }
    let max_retries =
        u32::try_from(cfg.max_retries).map_err(|_| BoltV3AdapterMappingError::NumericRange {
            venue_key: venue_key.to_string(),
            field: "execution.max_retries",
            message: format!(
                "value {} does not fit in u32 expected by NT",
                cfg.max_retries
            ),
        })?;
    Ok(PolymarketExecClientConfig {
        trader_id: TraderId::from(root.trader_id.as_str()),
        account_id: AccountId::from(cfg.account_id.as_str()),
        private_key: Some(secrets.private_key.clone()),
        api_key: Some(secrets.api_key.clone()),
        api_secret: Some(secrets.api_secret.clone()),
        passphrase: Some(secrets.passphrase.clone()),
        funder: cfg.funder_address,
        signature_type: nt_signature_type(cfg.signature_type),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        base_url_data_api: Some(cfg.base_url_data_api),
        http_timeout_secs: cfg.http_timeout_seconds,
        max_retries,
        retry_delay_initial_ms: cfg.retry_delay_initial_milliseconds,
        retry_delay_max_ms: cfg.retry_delay_max_milliseconds,
        ack_timeout_secs: cfg.ack_timeout_seconds,
        transport_backend: cfg.transport_backend,
    })
}

fn secrets_for<'a>(
    venue_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3PolymarketSecrets, BoltV3AdapterMappingError> {
    match resolved.venues.get(venue_key) {
        Some(inner) => inner.as_any().downcast_ref().ok_or_else(|| {
            BoltV3AdapterMappingError::SecretKindMismatch {
                venue_key: venue_key.to_string(),
                expected_provider_key: KEY,
            }
        }),
        None => Err(BoltV3AdapterMappingError::MissingResolvedSecrets {
            venue_key: venue_key.to_string(),
            expected_provider_key: KEY,
        }),
    }
}

fn nt_signature_type(value: PolymarketSignatureType) -> NtPolymarketSignatureType {
    match value {
        PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
        PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
        PolymarketSignatureType::PolyGnosisSafe => NtPolymarketSignatureType::PolyGnosisSafe,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_api_secret_padding;

    #[test]
    fn polymarket_api_secret_padding_preserves_padded_shape() {
        assert_eq!(normalize_api_secret_padding("abcd".to_string()), "abcd");
        assert_eq!(normalize_api_secret_padding("abc".to_string()), "abc=");
        assert_eq!(normalize_api_secret_padding("ab".to_string()), "ab==");
    }
}
