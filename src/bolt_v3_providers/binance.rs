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
    common::credential::Ed25519Credential,
    common::enums::{
        BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
    },
    config::BinanceDataClientConfig,
    factories::BinanceDataClientFactory,
};
use nautilus_network::websocket::TransportBackend;
use serde::Deserialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderResolvedSecrets,
        ProviderSecretRequirement, ProviderSecretResolveContext, ResolvedClientSecrets,
        SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
};

pub const KEY: &str = "BINANCE";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Binance reference-data client",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_path", "api_secret_ssm_path"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_binance::common::credential"];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "BINANCE_ED25519_API_KEY",
    "BINANCE_ED25519_API_SECRET",
    "BINANCE_API_KEY",
    "BINANCE_API_SECRET",
];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceDataConfig {
    pub product_types: Vec<BinanceProductType>,
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
    pub instrument_status_poll_secs: u64,
    pub transport_backend: TransportBackend,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceProductType {
    Spot,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BinanceEnvironment {
    Mainnet,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceSecretsConfig {
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
}

#[derive(Clone)]
pub struct ResolvedBoltV3BinanceSecrets {
    pub api_key: String,
    pub api_secret: String,
}

struct RedactedDebug;

impl std::fmt::Debug for RedactedDebug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Debug for ResolvedBoltV3BinanceSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;
        f.debug_struct("ResolvedBoltV3BinanceSecrets")
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
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
            "clients.{key} (provider=BINANCE) is not allowed to declare an [execution] block in the current bolt-v3 scope"
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
                "clients.{key} (provider=BINANCE) declares [secrets] but no [data] block is configured; \
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
    let secrets_value = context
        .client
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "secrets".to_string(),
            ssm_path: String::new(),
            source: "missing [secrets] block".to_string(),
        })?;
    let secrets: BinanceSecretsConfig =
        secrets_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| BoltV3SecretError {
                client_key: context.client_key.to_string(),
                field: KEY.to_string(),
                ssm_path: String::new(),
                source: format!("invalid binance secrets schema: {error}"),
            })?;
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
        ssm_path: secrets.api_secret_ssm_path.clone(),
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
        api_key,
        api_secret,
    }))
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
    let product_types = cfg.product_types.into_iter().map(nt_product_type).collect();
    Ok(BinanceDataClientConfig {
        product_types,
        environment: nt_environment(cfg.environment),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        api_key: Some(secrets.api_key.clone()),
        api_secret: Some(secrets.api_secret.clone()),
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
    }
}

fn nt_environment(value: BinanceEnvironment) -> NtBinanceEnvironment {
    match value {
        BinanceEnvironment::Mainnet => NtBinanceEnvironment::Live,
    }
}
