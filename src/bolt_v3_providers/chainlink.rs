//! Per-provider binding for `chainlink` reference-data venue config.
//!
//! This module owns the concrete shape of `[venues.<name>.data]` and
//! `[venues.<name>.secrets]` for any venue whose `kind = "chainlink"`
//! provider key is configured. Core config stays provider-neutral; this
//! binding translates validated bolt-v3 TOML into the existing NT-facing
//! Chainlink reference data client config.

use std::{any::Any, sync::Arc};

use serde::Deserialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3DataClientAdapterConfig, BoltV3VenueAdapterConfig,
    },
    bolt_v3_config::VenueBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderResolvedSecrets,
        ProviderSecretRequirement, ProviderSecretResolveContext, ReferenceCapability,
        ResolvedVenueSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    clients::chainlink::{
        ChainlinkReferenceClientConfig, ChainlinkReferenceDataClientFactory,
        ChainlinkSharedRuntimeConfig, chainlink_reference_feed_config, parse_chainlink_ws_origins,
    },
    secrets::ResolvedChainlinkSecrets,
};

pub const KEY: &str = "chainlink";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REFERENCE_CAPABILITIES: &[ReferenceCapability] = &[ReferenceCapability::Oracle];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Chainlink reference-data venue",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_path", "api_secret_ssm_path"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &[];
pub const FORBIDDEN_ENV_VARS: &[&str] = &["CHAINLINK_API_KEY", "CHAINLINK_API_SECRET"];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkDataConfig {
    pub instrument_id: String,
    pub feed_id: String,
    pub price_scale: u8,
    pub ws_url: String,
    pub ws_reconnect_alert_threshold: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkSecretsConfig {
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
}

#[derive(Clone)]
pub struct ResolvedBoltV3ChainlinkSecrets {
    pub api_key: String,
    pub api_secret: String,
}

struct RedactedDebug;

impl std::fmt::Debug for RedactedDebug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl std::fmt::Debug for ResolvedBoltV3ChainlinkSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;
        f.debug_struct("ResolvedBoltV3ChainlinkSecrets")
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3ChainlinkSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn validate_venue(key: &str, venue: &VenueBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if venue.execution.is_some() {
        errors.push(format!(
            "venues.{key} (kind=chainlink) is not allowed to declare an [execution] block"
        ));
    }
    if let Some(data) = &venue.data {
        match data.clone().try_into::<ChainlinkDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.data: {message}")),
        }
    }
    if let Some(secrets) = &venue.secrets {
        if venue.data.is_none() {
            errors.push(format!(
                "venues.{key} (kind=chainlink) declares [secrets] but no [data] block is configured; \
                 Chainlink [secrets] are only allowed alongside the data adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<ChainlinkSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &ChainlinkDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if data.instrument_id.trim().is_empty() {
        errors.push(format!("venues.{key}.data.instrument_id must not be empty"));
    }
    if data.ws_url.trim().is_empty() {
        errors.push(format!("venues.{key}.data.ws_url must not be empty"));
    } else if let Err(error) = parse_chainlink_ws_origins(&data.ws_url) {
        errors.push(format!("venues.{key}.data.ws_url: {error}"));
    }
    if data.ws_reconnect_alert_threshold == 0 {
        errors.push(format!(
            "venues.{key}.data.ws_reconnect_alert_threshold must be a positive integer"
        ));
    }
    if data.price_scale == 0 {
        errors.push(format!(
            "venues.{key}.data.price_scale must be a positive integer"
        ));
    }
    if data.price_scale > 18 {
        errors.push(format!(
            "venues.{key}.data.price_scale must be <= 18, got {}",
            data.price_scale
        ));
    }
    if let Err(error) =
        chainlink_reference_feed_config(key, &data.instrument_id, &data.feed_id, data.price_scale)
    {
        errors.push(format!("venues.{key}.data.feed_id: {error}"));
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &ChainlinkSecretsConfig) -> Vec<String> {
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
    let secrets: ChainlinkSecretsConfig =
        secrets_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| BoltV3SecretError {
                venue_key: context.venue_key.to_string(),
                field: KEY.to_string(),
                ssm_path: String::new(),
                source: format!("invalid chainlink secrets schema: {error}"),
            })?;
    let api_key = resolve_field(
        context.venue_key,
        "api_key_ssm_path",
        context.region,
        &secrets.api_key_ssm_path,
        resolver,
    )?;
    let api_secret = resolve_field(
        context.venue_key,
        "api_secret_ssm_path",
        context.region,
        &secrets.api_secret_ssm_path,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3ChainlinkSecrets {
        api_key,
        api_secret,
    }))
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.venue.data {
        Some(value) => {
            let secrets = secrets_for(context.venue_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(ChainlinkReferenceDataClientFactory),
                config: Box::new(map_data(context.venue_key, value, secrets)?),
            })
        }
        None => None,
    };
    Ok(BoltV3VenueAdapterConfig {
        data,
        execution: None,
    })
}

fn map_data(
    venue_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3ChainlinkSecrets,
) -> Result<ChainlinkReferenceClientConfig, BoltV3AdapterMappingError> {
    let cfg: ChainlinkDataConfig = value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            venue_key: venue_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })?;
    let ws_reconnect_alert_threshold =
        usize::try_from(cfg.ws_reconnect_alert_threshold).map_err(|error| {
            BoltV3AdapterMappingError::SchemaParse {
                venue_key: venue_key.to_string(),
                block: "data.ws_reconnect_alert_threshold",
                message: error.to_string(),
            }
        })?;
    let feed = chainlink_reference_feed_config(
        venue_key,
        &cfg.instrument_id,
        &cfg.feed_id,
        cfg.price_scale,
    )
    .map_err(|error| BoltV3AdapterMappingError::SchemaParse {
        venue_key: venue_key.to_string(),
        block: "data.feed_id",
        message: error.to_string(),
    })?;

    Ok(ChainlinkReferenceClientConfig {
        shared: ChainlinkSharedRuntimeConfig {
            ws_url: cfg.ws_url,
            ws_reconnect_alert_threshold,
            secrets: ResolvedChainlinkSecrets {
                api_key: secrets.api_key.clone(),
                api_secret: secrets.api_secret.clone(),
            },
        },
        feeds: vec![feed],
    })
}

fn secrets_for<'a>(
    venue_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3ChainlinkSecrets, BoltV3AdapterMappingError> {
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
