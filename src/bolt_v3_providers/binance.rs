//! Per-provider binding for `BINANCE` client config block shapes and
//! per-client startup validation.
//!
//! Owns the concrete shape of `[clients.<name>.data]` and
//! `[clients.<name>.secrets]` for any client whose `venue = "BINANCE"`
//! NT venue is configured. Core config in `crate::bolt_v3_config`
//! only owns the root/strategy envelope and raw NT venue field; the
//! provider-shaped block types and their
//! serde rules live here so provider-specific schema evolution does
//! not reach back into the envelope module.
//!
//! This module also owns the per-client startup-validation policy for
//! Binance clients: the no-execution rule for the current bolt-v3
//! scope, typed deserialization of each present block, cross-block
//! presence rule ([secrets] is only allowed alongside [data]),
//! Binance data bounds, and Binance secret-path ownership. The
//! cross-provider rule that [data] requires [secrets] is declared by
//! [`REQUIRED_SECRET_BLOCKS`] and enforced centrally in
//! `bolt_v3_providers::validate_client_block`. Core startup validation in
//! `crate::bolt_v3_validate` dispatches into
//! `bolt_v3_providers::validate_client_block`, which routes Binance
//! venues here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

use std::{any::Any, sync::Arc};

use nautilus_binance::{
    common::{
        consts::BINANCE_SPOT_WS_URL,
        credential::Ed25519Credential,
        enums::{
            BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
        },
    },
    config::{BinanceDataClientConfig, BinanceSpotMarketDataMode as NtBinanceSpotMarketDataMode},
    factories::BinanceDataClientFactory,
};
use nautilus_core::string::secret::REDACTED;
use serde::Deserialize;
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderResolvedSecrets,
        ProviderSecretRequirement, ProviderSecretResolveContext, ProviderSsmPathReference,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    bolt_v3_wire_boundary::TransportBackend,
    nautilus_source_capabilities::NAUTILUS_SOURCE_CAPABILITIES,
};

pub const KEY: &str = "BINANCE";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Binance reference-data client",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_path", "api_secret_ssm_path"];
/// NT module path(s) whose info-level logs can echo Binance credential
/// metadata; the live-node builder installs `WARN` filters for these so secret
/// material never reaches operator logs. The path is pinned to the NT revision
/// declared by `nautilus-binance` in `Cargo.toml` (single source of truth for
/// the rev) and is kept honest at compile time by the
/// `use nautilus_binance::common::credential::Ed25519Credential` import above:
/// if the NT rev moved this module, that import — and therefore the build —
/// would fail before this string could silently drift.
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_binance::common::credential"];
/// Every Binance credential environment variable that NT's
/// `resolve_credentials` (and the Spot WebSocket trading client) can read as a
/// secret fallback. Bolt always passes `Some(api_key)`/`Some(api_secret)` into
/// NT, so this env path is currently dead, but the blocklist is verified empty
/// at startup as defense-in-depth so a future regression that drops the
/// `Some(...)` wiring cannot silently let NT resolve a trading secret from the
/// operator's shell environment instead of SSM.
///
/// The set is verified against the pinned NT revision declared by
/// `nautilus-binance` in `Cargo.toml`, in
/// `nautilus_binance::common::credential::resolve_credentials`
/// (`crates/adapters/binance/src/common/credential.rs`). That function selects
/// the variable names by `BinanceEnvironment` and `BinanceProductType`:
/// - Live: standard `BINANCE_API_KEY`/`BINANCE_API_SECRET` plus the deprecated
///   `BINANCE_ED25519_*` pair (still read via `std::env::var` before the
///   deprecation error).
/// - Testnet (Spot/Margin/Options): `BINANCE_TESTNET_API_KEY`/`_API_SECRET`
///   plus deprecated `BINANCE_TESTNET_ED25519_*`.
/// - Futures testnet (UsdM/CoinM): `BINANCE_FUTURES_TESTNET_API_KEY`/`_API_SECRET`
///   plus deprecated `BINANCE_FUTURES_TESTNET_ED25519_*`.
/// - Demo (all product types): `BINANCE_DEMO_API_KEY`/`BINANCE_DEMO_API_SECRET`
///   (NT defines no Demo `ED25519` pair — the deprecated names are empty).
///
/// Every name NT can read is listed; under-listing would reopen the
/// defense-in-depth gap for the testnet/futures-testnet/demo environments.
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "BINANCE_ED25519_API_KEY",
    "BINANCE_ED25519_API_SECRET",
    "BINANCE_API_KEY",
    "BINANCE_API_SECRET",
    "BINANCE_TESTNET_API_KEY",
    "BINANCE_TESTNET_API_SECRET",
    "BINANCE_TESTNET_ED25519_API_KEY",
    "BINANCE_TESTNET_ED25519_API_SECRET",
    "BINANCE_FUTURES_TESTNET_API_KEY",
    "BINANCE_FUTURES_TESTNET_API_SECRET",
    "BINANCE_FUTURES_TESTNET_ED25519_API_KEY",
    "BINANCE_FUTURES_TESTNET_ED25519_API_SECRET",
    "BINANCE_DEMO_API_KEY",
    "BINANCE_DEMO_API_SECRET",
];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceDataConfig {
    pub product_type: BinanceProductType,
    pub environment: BinanceEnvironment,
    /// Required HTTP base URL passed through to
    /// `nautilus_binance::config::BinanceDataClientConfig.base_url_http`
    /// as `Some(...)` so NT does not silently fall back to the
    /// compiled-in default endpoint.
    pub base_url_http: String,
    /// Required WebSocket base URL passed through to
    /// `nautilus_binance::config::BinanceDataClientConfig.base_url_ws`
    /// as `Some(...)` so NT does not silently fall back to the
    /// compiled-in default endpoint.
    pub base_url_ws: String,
    pub spot_market_data_mode: BinanceSpotMarketDataMode,
    pub instrument_status_poll_secs: u64,
    pub transport_backend: TransportBackend,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceProductType {
    Spot,
    Margin,
    #[serde(rename = "usd_m")]
    UsdM,
    #[serde(rename = "coin_m")]
    CoinM,
    Options,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceSpotMarketDataMode {
    Sbe,
    Json,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceEnvironment {
    Mainnet,
    Live,
    Testnet,
    Demo,
}

pub(crate) fn new_risk_market_data_available(
    client_key: &str,
    client: &ClientBlock,
) -> Result<bool, String> {
    let Some(data) = client.data.as_ref() else {
        return Ok(true);
    };
    let config = data
        .clone()
        .try_into::<BinanceDataConfig>()
        .map_err(|error| {
            format!("clients.{client_key}.data could not parse Binance config: {error}")
        })?;
    Ok(config.product_type != BinanceProductType::Spot
        || config.spot_market_data_mode != BinanceSpotMarketDataMode::Sbe
        || NAUTILUS_SOURCE_CAPABILITIES.binance_spot_sbe_new_risk_quorum)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceSecretsConfig {
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedBoltV3BinanceSecrets {
    /// Wrapped in [`Zeroizing`] so the individual secret bytes are scrubbed on
    /// drop even when this field is moved out of the container — per-field
    /// zeroize in addition to the container-level `ZeroizeOnDrop`. Derefs to
    /// `String`; the redacting `Debug` impl below keeps it out of logs.
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
}

impl std::fmt::Debug for ResolvedBoltV3BinanceSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3BinanceSecrets")
            .field("api_key", &REDACTED)
            .field("api_secret", &REDACTED)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3BinanceSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn redaction_values(&self) -> Vec<&str> {
        vec![self.api_key.as_str(), self.api_secret.as_str()]
    }
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.execution.is_some() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) is not allowed to declare an [execution] block in the current bolt-v3 scope"
        ));
    }
    if let Some(data) = &client.data {
        match data.clone().try_into::<BinanceDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(secrets) = &client.secrets {
        if client.data.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [secrets] but no [data] block is configured; \
                 Binance [secrets] are only allowed alongside the data adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<BinanceSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &BinanceDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if !is_nt_binance_data_factory_supported(data.product_type) {
        errors.push(format!(
            "clients.{key}.data.product_type contains {:?}, but the pinned NT BinanceDataClientFactory only supports spot, usd_m, and coin_m data clients",
            data.product_type
        ));
    }
    let url_fields: &[(&str, &str)] = &[
        ("base_url_http", data.base_url_http.as_str()),
        ("base_url_ws", data.base_url_ws.as_str()),
    ];
    for (field, value) in url_fields {
        if value.trim().is_empty() {
            errors.push(format!(
                "clients.{key}.data.{field} must be a non-empty URL"
            ));
        }
    }
    if !data.base_url_ws.trim().is_empty() {
        errors.extend(validate_binance_websocket_endpoint(
            key,
            data.product_type,
            data.base_url_ws.as_str(),
        ));
    }
    // The bolt-v3 schema deliberately rejects `0` rather than treating
    // it as "polling disabled": NT's `BinanceDataClientConfig` consumes
    // this as a poll interval and a missing/zero value would leave NT
    // free to fall back to its own default cadence. Failing closed
    // here keeps the bolt-v3 instrument-status-poll cadence explicit.
    if data.instrument_status_poll_secs == 0 {
        errors.push(format!(
            "clients.{key}.data.instrument_status_poll_secs must be a positive integer"
        ));
    }
    errors
}

fn validate_binance_websocket_endpoint(
    key: &str,
    product_type: BinanceProductType,
    value: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let is_spot = product_type == BinanceProductType::Spot;
    let url_description = if is_spot {
        "Binance Spot WebSocket URL for NT subscribe_quotes"
    } else {
        "Binance WebSocket URL"
    };
    let Ok(configured) = Url::parse(value) else {
        errors.push(format!(
            "clients.{key}.data.base_url_ws must be a valid {url_description}"
        ));
        return errors;
    };
    if !matches!(configured.scheme(), "ws" | "wss") {
        errors.push(format!(
            "clients.{key}.data.base_url_ws must be a valid {url_description}"
        ));
        return errors;
    }
    if !value[configured.scheme().len()..].starts_with("://") || !configured.has_host() {
        errors.push(format!(
            "clients.{key}.data.base_url_ws must be a valid {url_description}"
        ));
        return errors;
    }
    if !is_spot {
        return errors;
    }

    let Ok(json_endpoint) = Url::parse(BINANCE_SPOT_WS_URL) else {
        errors.push(
            "nautilus_binance Spot JSON WebSocket URL constant failed URL parsing".to_string(),
        );
        return errors;
    };

    if configured.host_str() == json_endpoint.host_str() {
        errors.push(format!(
            "clients.{key}.data.base_url_ws must not use the Binance Spot JSON WebSocket host for NT subscribe_quotes (<symbol>@bestBidAsk); configure a Binance Spot SBE WebSocket endpoint or compatible SBE proxy so strategy-free reference quote readiness can observe QuoteTick data"
        ));
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &BinanceSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("api_key_ssm_path", &secrets.api_key_ssm_path),
        ("api_secret_ssm_path", &secrets.api_secret_ssm_path),
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
) -> Result<ResolvedClientSecrets, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    let api_secret = resolve_field(
        context.client_key,
        "api_secret_ssm_path",
        context.region,
        &secrets.api_secret_ssm_path,
        resolver,
    )?;
    validate_binance_api_secret_shape(&api_secret).map_err(|_| BoltV3SecretError {
        client_key: context.client_key.to_string(),
        field: "api_secret_ssm_path".to_string(),
        source: "resolved binance api_secret is not valid Ed25519 PKCS8 base64 key material accepted by the NautilusTrader binance adapter".to_string(),
    })?;
    let api_key = resolve_field(
        context.client_key,
        "api_key_ssm_path",
        context.region,
        &secrets.api_key_ssm_path,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3BinanceSecrets {
        api_key: Zeroizing::new(api_key),
        api_secret: Zeroizing::new(api_secret),
    }))
}

pub fn configured_secret_paths(
    context: ProviderSecretResolveContext<'_>,
) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    Ok(vec![
        ProviderSsmPathReference {
            field_name: "api_key_ssm_path",
            ssm_path: secrets.api_key_ssm_path,
        },
        ProviderSsmPathReference {
            field_name: "api_secret_ssm_path",
            ssm_path: secrets.api_secret_ssm_path,
        },
    ])
}

fn parse_secrets_config(
    context: &ProviderSecretResolveContext<'_>,
) -> Result<BinanceSecretsConfig, BoltV3SecretError> {
    let secrets_value = context
        .client
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "secrets".to_string(),
            source: "missing [secrets] block".to_string(),
        })?;
    secrets_value
        .clone()
        .try_into()
        .map_err(|error: toml::de::Error| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: KEY.to_string(),
            source: format!("invalid binance secrets schema: {error}"),
        })
}

fn validate_binance_api_secret_shape(api_secret: &str) -> Result<(), String> {
    if api_secret.trim().is_empty() {
        return Err("resolved Binance api_secret is empty".to_string());
    }

    Ed25519Credential::new("BINANCE-SHAPE-CHECK".to_string(), api_secret)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "resolved Binance api_secret is not valid Ed25519 key material accepted by the NT Binance adapter: {error}"
            )
        })
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(BinanceDataClientFactory::new()),
                config: Box::new(map_data(context.client_key, value, secrets)?),
            })
        }
        None => None,
    };
    Ok(BoltV3ClientAdapterConfig {
        data,
        execution: None,
    })
}

fn map_data(
    client_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3BinanceSecrets,
) -> Result<BinanceDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: BinanceDataConfig = value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })?;
    let validation_errors = validate_data_bounds(client_key, &cfg);
    if let Some(message) = validation_errors.into_iter().next() {
        return Err(BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message,
        });
    }
    Ok(BinanceDataClientConfig {
        product_type: nt_product_type(cfg.product_type),
        environment: nt_environment(cfg.environment),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        api_key: Some(secrets.api_key.as_str().to_owned()),
        api_secret: Some(secrets.api_secret.as_str().to_owned()),
        spot_market_data_mode: nt_spot_market_data_mode(cfg.spot_market_data_mode),
        instrument_status_poll_secs: cfg.instrument_status_poll_secs,
        transport_backend: cfg.transport_backend,
    })
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3BinanceSecrets, BoltV3AdapterMappingError> {
    match resolved.clients.get(client_key) {
        Some(inner) => inner.as_any().downcast_ref().ok_or_else(|| {
            BoltV3AdapterMappingError::SecretProviderMismatch {
                client_key: client_key.to_string(),
                expected_provider_key: KEY,
            }
        }),
        None => Err(BoltV3AdapterMappingError::MissingResolvedSecrets {
            client_key: client_key.to_string(),
            expected_provider_key: KEY,
        }),
    }
}

fn nt_product_type(value: BinanceProductType) -> NtBinanceProductType {
    match value {
        BinanceProductType::Spot => NtBinanceProductType::Spot,
        BinanceProductType::Margin => NtBinanceProductType::Margin,
        BinanceProductType::UsdM => NtBinanceProductType::UsdM,
        BinanceProductType::CoinM => NtBinanceProductType::CoinM,
        BinanceProductType::Options => NtBinanceProductType::Options,
    }
}

fn nt_spot_market_data_mode(value: BinanceSpotMarketDataMode) -> NtBinanceSpotMarketDataMode {
    match value {
        BinanceSpotMarketDataMode::Sbe => NtBinanceSpotMarketDataMode::Sbe,
        BinanceSpotMarketDataMode::Json => NtBinanceSpotMarketDataMode::Json,
    }
}

fn is_nt_binance_data_factory_supported(value: BinanceProductType) -> bool {
    matches!(
        value,
        BinanceProductType::Spot | BinanceProductType::UsdM | BinanceProductType::CoinM
    )
}

fn nt_environment(value: BinanceEnvironment) -> NtBinanceEnvironment {
    match value {
        BinanceEnvironment::Mainnet | BinanceEnvironment::Live => NtBinanceEnvironment::Live,
        BinanceEnvironment::Testnet => NtBinanceEnvironment::Testnet,
        BinanceEnvironment::Demo => NtBinanceEnvironment::Demo,
    }
}
