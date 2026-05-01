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
//! cross-block presence rules ([execution] requires [secrets]; [secrets]
//! is only allowed alongside [execution]), Polymarket data/execution
//! bounds, EVM funder-address syntax, and Polymarket secret-path
//! ownership. Core startup validation in `crate::bolt_v3_validate`
//! dispatches into `bolt_v3_providers::validate_venue_block`, which
//! routes Polymarket venues here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

use std::{any::Any, sync::Arc};

use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_polymarket::{
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{InstrumentFilter, MarketSlugFilter},
};
use serde::Deserialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig, BoltV3UpdownNowFn, BoltV3VenueAdapterConfig,
    },
    bolt_v3_config::VenueBlock,
    bolt_v3_market_families::updown::{
        MarketIdentityPlan, UpdownTargetPlan, updown_market_slug, updown_period_pair,
    },
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderResolvedSecrets, ProviderSecretResolveContext,
        ResolvedVenueSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    secrets::pad_base64,
};

pub const KEY: &str = "polymarket";
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_polymarket::common::credential"];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "POLYMARKET_PK",
    "POLYMARKET_FUNDER",
    "POLYMARKET_API_KEY",
    "POLYMARKET_API_SECRET",
    "POLYMARKET_PASSPHRASE",
];

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
    pub update_instruments_interval_minutes: u64,
    pub websocket_max_subscriptions_per_connection: u64,
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
    #[serde(default)]
    pub funder_address: Option<String>,
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_data_api: String,
    pub http_timeout_seconds: u64,
    pub max_retries: u64,
    pub retry_delay_initial_milliseconds: u64,
    pub retry_delay_max_milliseconds: u64,
    pub ack_timeout_seconds: u64,
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
        // Per docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md Section 3,
        // every Polymarket execution venue must resolve credentials through
        // SSM. The secret-block requirement is what guarantees the env-var
        // blocklist actually fires for that venue at startup; without it an
        // operator could declare [execution] alone and silently bypass the
        // SSM-only invariant by reading the legacy POLYMARKET_* env vars.
        if venue.secrets.is_none() {
            errors.push(format!(
                "venues.{key} (kind=polymarket) declares [execution] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every Polymarket execution venue"
            ));
        }
    }
    if let Some(secrets) = &venue.secrets {
        // Only Polymarket execution consumes Polymarket credentials in
        // this slice. A data-only Polymarket venue with `[secrets]`
        // would carry credential paths that no adapter uses, which is a
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
        (true, None) => errors.push(format!(
            "venues.{key}.execution.funder_address is required when signature_type is `poly_proxy` or `poly_gnosis_safe`"
        )),
        (_, Some(value)) => {
            if let Err(message) = check_evm_address_syntax(value) {
                errors.push(format!(
                    "venues.{key}.execution.funder_address is not a valid EVM public address ({message}): `{value}`"
                ));
            }
        }
        (false, None) => {}
    }
    errors
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
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "venues.{key}.data.{field} must be a positive integer"
            ));
        }
    }
    // The pinned NautilusTrader Polymarket data client (`nautilus_polymarket::data`)
    // calls `ws_client.subscribe_market(vec![])` from inside its `connect()`
    // implementation when `subscribe_new_markets = true`, which is effectively
    // an all-markets subscription and violates the bolt-v3 controlled-connect
    // boundary. The flag is forced false in the current bolt-v3 scope until
    // the market-subscription slice owns the controlled-subscribe path; failing
    // closed here keeps that invariant honest.
    if data.subscribe_new_markets {
        errors.push(format!(
            "venues.{key}.data.subscribe_new_markets must be false in the current bolt-v3 scope; \
             the pinned NT Polymarket data client subscribes to all markets via \
             `ws_client.subscribe_market(vec![])` during connect when this flag is true, \
             which violates the bolt-v3 controlled-connect boundary until the \
             market-subscription slice owns it"
        ));
    }
    errors
}

fn validate_execution_bounds(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
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
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "venues.{key}.execution.{field} must be a positive integer"
            ));
        }
    }
    if execution.retry_delay_initial_milliseconds > execution.retry_delay_max_milliseconds {
        errors.push(format!(
            "venues.{key}.execution.retry_delay_initial_milliseconds ({}) must be <= retry_delay_max_milliseconds ({})",
            execution.retry_delay_initial_milliseconds, execution.retry_delay_max_milliseconds
        ));
    }
    errors
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
    let api_secret = pad_base64(api_secret_raw);
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

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.venue.data {
        Some(value) => Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(PolymarketDataClientFactory),
            config: Box::new(map_data(
                context.venue_key,
                value,
                context.plan,
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

fn map_data(
    venue_key: &str,
    value: &toml::Value,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
) -> Result<PolymarketDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                venue_key: venue_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    if cfg.subscribe_new_markets {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "data.subscribe_new_markets",
            message: "must be false before mapping to NT because pinned NT subscribes to all Polymarket markets when this flag is true".to_string(),
        });
    }
    let ws_max_subscriptions = usize::try_from(cfg.websocket_max_subscriptions_per_connection)
        .map_err(|_| BoltV3AdapterMappingError::NumericRange {
            venue_key: venue_key.to_string(),
            field: "data.websocket_max_subscriptions_per_connection",
            message: format!(
                "value {} does not fit in usize on this target",
                cfg.websocket_max_subscriptions_per_connection
            ),
        })?;
    let filters = build_market_slug_filters_for_venue(plan, venue_key, clock);
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
        auto_load_missing_instruments: false,
        auto_load_debounce_ms: 100,
        transport_backend: Default::default(),
        filters,
        new_market_filter: None,
    })
}

fn build_market_slug_filters_for_venue(
    plan: &MarketIdentityPlan,
    venue_key: &str,
    clock: BoltV3UpdownNowFn,
) -> Vec<Arc<dyn InstrumentFilter>> {
    plan.updown_targets
        .iter()
        .filter(|target| target.venue_config_key == venue_key)
        .map(|target| build_market_slug_filter(target, clock.clone()))
        .collect()
}

fn build_market_slug_filter(
    target: &UpdownTargetPlan,
    clock: BoltV3UpdownNowFn,
) -> Arc<dyn InstrumentFilter> {
    let asset = target.underlying_asset.clone();
    let token = target.cadence_slug_token.clone();
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

fn map_execution(
    root: &crate::bolt_v3_config::BoltV3RootConfig,
    venue_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3PolymarketSecrets,
) -> Result<PolymarketExecClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                venue_key: venue_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
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
        transport_backend: Default::default(),
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
