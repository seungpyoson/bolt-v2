//! Per-provider binding for Chainlink reference data clients.

use std::{any::Any, sync::Arc};

use serde::Deserialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3ClientConfig, BoltV3ClientMappingError, BoltV3DataClientAdapterConfig,
    },
    bolt_v3_config::{ClientBlock, ReferenceSourceType, ReferenceStreamInputBlock},
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderReferenceInputContext,
        ProviderResolvedSecrets, ProviderSecretRequirement, ProviderSecretResolveContext,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    clients::chainlink::build_chainlink_reference_data_client_with_secrets,
    config::{
        ChainlinkReferenceConfig, ChainlinkSharedConfig, ReferenceConfig, ReferenceVenueEntry,
        ReferenceVenueKind,
    },
    secrets::ResolvedChainlinkSecrets,
};

pub const KEY: &str = "CHAINLINK";
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Chainlink reference-data client",
}];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &[];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkDataConfig {
    pub region: String,
    pub ws_url: String,
    pub ws_reconnect_alert_threshold: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkSecretsConfig {
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkReferenceInputConfig {
    pub feed_id: String,
    pub price_scale: u8,
}

#[derive(Clone)]
pub struct ResolvedBoltV3ChainlinkSecrets {
    pub api_key: String,
    pub api_secret: String,
}

struct RedactedDebug;

impl std::fmt::Debug for RedactedDebug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
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
    fn venue_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn build_reference_venue_entry(
    context: ProviderReferenceInputContext<'_>,
) -> Result<ReferenceVenueEntry, String> {
    if context.input.source_type != ReferenceSourceType::Oracle {
        return Err(format!(
            "reference_streams.{}.inputs[{}].source_type `orderbook` is not supported for client venue `{}`",
            context.stream_id, context.input_index, KEY
        ));
    }
    let chainlink = parse_reference_input_config(context.input).map_err(|error| {
        format!(
            "reference_streams.{}.inputs[{}].provider_config: {error}",
            context.stream_id, context.input_index
        )
    })?;
    Ok(ReferenceVenueEntry {
        name: context.input.source_id.clone(),
        kind: ReferenceVenueKind::Chainlink,
        instrument_id: context.input.instrument_id.clone(),
        base_weight: context.input.base_weight,
        stale_after_ms: context.input.stale_after_milliseconds,
        disable_after_ms: context.input.disable_after_milliseconds,
        chainlink: Some(ChainlinkReferenceConfig {
            feed_id: chainlink.feed_id,
            price_scale: chainlink.price_scale,
        }),
    })
}

fn parse_reference_input_config(
    input: &ReferenceStreamInputBlock,
) -> Result<ChainlinkReferenceInputConfig, String> {
    input
        .provider_config
        .clone()
        .ok_or_else(|| "feed config is required for Chainlink producer inputs".to_string())?
        .try_into()
        .map_err(|error: toml::de::Error| error.to_string())
}

pub fn validate_client_id(key: &str, client_id: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client_id.execution.is_some() {
        errors.push(format!(
            "clients.{key} (venue=CHAINLINK) is not allowed to declare an [execution] block"
        ));
    }
    if let Some(data) = &client_id.data {
        match data.clone().try_into::<ChainlinkDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(secrets) = &client_id.secrets {
        if client_id.data.is_none() {
            errors.push(format!(
                "clients.{key} (venue=CHAINLINK) declares [secrets] but no [data] block is configured; \
                 Chainlink [secrets] are only allowed alongside the data adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<ChainlinkSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &ChainlinkDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if data.region.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.data.region must be a non-empty string"
        ));
    }
    if data.ws_url.trim().is_empty() {
        errors.push(format!("clients.{key}.data.ws_url must be a non-empty URL"));
    }
    if data.ws_reconnect_alert_threshold == 0 {
        errors.push(format!(
            "clients.{key}.data.ws_reconnect_alert_threshold must be a positive integer"
        ));
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &ChainlinkSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    for (field, value) in [
        ("api_key_ssm_path", secrets.api_key_ssm_path.as_str()),
        ("api_secret_ssm_path", secrets.api_secret_ssm_path.as_str()),
    ] {
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
        .client_id
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            client_id_key: context.client_id_key.to_string(),
            field: "secrets".to_string(),
            ssm_path: String::new(),
            source: "missing [secrets] block".to_string(),
        })?;
    let secrets: ChainlinkSecretsConfig =
        secrets_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| BoltV3SecretError {
                client_id_key: context.client_id_key.to_string(),
                field: "secrets".to_string(),
                ssm_path: String::new(),
                source: error.to_string(),
            })?;
    let api_key = resolve_field(
        context.client_id_key,
        "api_key_ssm_path",
        context.region,
        &secrets.api_key_ssm_path,
        resolver,
    )?;
    let api_secret = resolve_field(
        context.client_id_key,
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
) -> Result<BoltV3ClientConfig, BoltV3ClientMappingError> {
    let data = match &context.client_id.data {
        Some(value) => {
            let secrets = secrets_for(context.client_id_key, context.resolved)?;
            let data_config = map_data(context.client_id_key, value, context, secrets)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: data_config.0,
                config: data_config.1,
            })
        }
        None => None,
    };
    Ok(BoltV3ClientConfig {
        data,
        execution: None,
    })
}

fn map_data(
    client_id_key: &str,
    value: &toml::Value,
    context: ProviderAdapterMapContext<'_>,
    secrets: &ResolvedBoltV3ChainlinkSecrets,
) -> Result<crate::clients::ReferenceDataClientParts, BoltV3ClientMappingError> {
    let cfg: ChainlinkDataConfig = value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3ClientMappingError::SchemaParse {
            client_id_key: client_id_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })?;
    let reference = reference_config_for_client(client_id_key, &cfg, context)?;
    build_chainlink_reference_data_client_with_secrets(
        &reference,
        ResolvedChainlinkSecrets {
            api_key: secrets.api_key.clone(),
            api_secret: secrets.api_secret.clone(),
        },
    )
    .map_err(|error| BoltV3ClientMappingError::ValidationInvariant {
        client_id_key: client_id_key.to_string(),
        field: "reference_streams",
        message: error.to_string(),
    })
}

fn reference_config_for_client(
    client_id_key: &str,
    data: &ChainlinkDataConfig,
    context: ProviderAdapterMapContext<'_>,
) -> Result<ReferenceConfig, BoltV3ClientMappingError> {
    let secrets: ChainlinkSecretsConfig = context
        .client_id
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3ClientMappingError::MissingResolvedSecrets {
            client_id_key: client_id_key.to_string(),
            expected_venue: KEY,
        })?
        .clone()
        .try_into()
        .map_err(
            |error: toml::de::Error| BoltV3ClientMappingError::SchemaParse {
                client_id_key: client_id_key.to_string(),
                block: "secrets",
                message: error.to_string(),
            },
        )?;
    let mut venues = Vec::new();
    for stream in context.root.reference_streams.values() {
        for input in &stream.inputs {
            if input.data_client_id.as_deref() != Some(client_id_key) {
                continue;
            }
            if input.source_type != ReferenceSourceType::Oracle {
                return Err(BoltV3ClientMappingError::ValidationInvariant {
                    client_id_key: client_id_key.to_string(),
                    field: "reference_streams",
                    message: format!(
                        "reference input `{}` uses Chainlink client but source_type is not `oracle`",
                        input.source_id
                    ),
                });
            }
            let chainlink =
                parse_reference_input_config(input).map_err(|error| {
                    BoltV3ClientMappingError::ValidationInvariant {
                        client_id_key: client_id_key.to_string(),
                        field: "reference_streams",
                        message: format!(
                            "reference input `{}` uses Chainlink client but has invalid provider_config: {error}",
                            input.source_id
                        ),
                    }
                })?;
            venues.push(ReferenceVenueEntry {
                name: input.source_id.clone(),
                kind: ReferenceVenueKind::Chainlink,
                instrument_id: input.instrument_id.clone(),
                base_weight: input.base_weight,
                stale_after_ms: input.stale_after_milliseconds,
                disable_after_ms: input.disable_after_milliseconds,
                chainlink: Some(ChainlinkReferenceConfig {
                    feed_id: chainlink.feed_id,
                    price_scale: chainlink.price_scale,
                }),
            });
        }
    }
    if venues.is_empty() {
        return Err(BoltV3ClientMappingError::ValidationInvariant {
            client_id_key: client_id_key.to_string(),
            field: "reference_streams",
            message: "no reference stream inputs use this Chainlink client".to_string(),
        });
    }

    Ok(ReferenceConfig {
        publish_topic: String::new(),
        min_publish_interval_ms: 1,
        binance: None,
        chainlink: Some(ChainlinkSharedConfig {
            region: data.region.clone(),
            api_key: secrets.api_key_ssm_path,
            api_secret: secrets.api_secret_ssm_path,
            ws_url: data.ws_url.clone(),
            ws_reconnect_alert_threshold: data.ws_reconnect_alert_threshold,
        }),
        venues,
    })
}

fn secrets_for<'a>(
    client_id_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3ChainlinkSecrets, BoltV3ClientMappingError> {
    match resolved.clients.get(client_id_key) {
        Some(inner) => inner.as_any().downcast_ref().ok_or_else(|| {
            BoltV3ClientMappingError::SecretVenueMismatch {
                client_id_key: client_id_key.to_string(),
                expected_venue: KEY,
            }
        }),
        None => Err(BoltV3ClientMappingError::MissingResolvedSecrets {
            client_id_key: client_id_key.to_string(),
            expected_venue: KEY,
        }),
    }
}
