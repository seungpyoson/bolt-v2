//! Per-provider binding for the `CHAINLINK_DATA_STREAMS` client block shape
//! and per-client startup validation.
//!
//! Owns the concrete shape of `[clients.<name>.data]` and
//! `[clients.<name>.secrets]` for any client whose `venue = "CHAINLINK_DATA_STREAMS"`
//! NT venue is configured. Core config in `crate::bolt_v3_config` only owns the
//! root/strategy envelope and raw NT venue field; the provider-shaped block
//! types and their serde rules live here so provider-specific schema evolution
//! does not reach back into the envelope module.
//!
//! The Chainlink client is a point-in-time strike (price-to-beat) source: it
//! fetches the Data Streams report AT a window-open timestamp and delivers it
//! as one NT `IndexPriceUpdate`. It is NOT a continuous stream and declares no
//! `[execution]` block. The bolt-owned HMAC request signer never routes
//! credentials through an NT adapter, so this binding contributes no
//! credential-log modules and no forbidden environment variables.

use std::{any::Any, sync::Arc};

use nautilus_core::string::secret::REDACTED;
use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
    },
    bolt_v3_chainlink::{
        ChainlinkStrikeFeedBinding, ChainlinkStrikeSourceConfig, ChainlinkStrikeSourceFactory,
        parse_feed_binding,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderResolvedSecrets,
        ProviderSecretRequirement, ProviderSecretResolveContext, ProviderSsmPathReference,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
};

pub const KEY: &str = "CHAINLINK_DATA_STREAMS";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Chainlink Data Streams strike source",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_parameter", "api_secret_ssm_parameter"];
/// Chainlink credentials are consumed by the bolt-owned HMAC request signer in
/// `crate::bolt_v3_chainlink`, never by an NT adapter, so no NT module can echo
/// them at info level.
pub const CREDENTIAL_LOG_MODULES: &[&str] = &[];
/// The bolt-owned Chainlink signer resolves credentials only from SSM; it never
/// reads any environment variable as a secret fallback.
pub const FORBIDDEN_ENV_VARS: &[&str] = &[];

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkDataConfig {
    pub rest_base_url: String,
    pub report_endpoint_path: String,
    pub http_timeout_secs: u64,
    pub feed_bindings: Vec<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkSecretsConfig {
    pub api_key_ssm_parameter: String,
    pub api_secret_ssm_parameter: String,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedBoltV3ChainlinkSecrets {
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
}

impl std::fmt::Debug for ResolvedBoltV3ChainlinkSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3ChainlinkSecrets")
            .field("api_key", &REDACTED)
            .field("api_secret", &REDACTED)
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

    fn redaction_values(&self) -> Vec<&str> {
        vec![self.api_key.as_str(), self.api_secret.as_str()]
    }
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.execution.is_some() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) is not allowed to declare an [execution] block; the Chainlink Data Streams strike source is data-only"
        ));
    }
    if let Some(data) = &client.data {
        match data.clone().try_into::<ChainlinkDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(secrets) = &client.secrets {
        if client.data.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [secrets] but no [data] block is configured; \
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
    if data.rest_base_url.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.data.rest_base_url must be a non-empty URL"
        ));
    } else if url::Url::parse(&data.rest_base_url).is_err() {
        errors.push(format!(
            "clients.{key}.data.rest_base_url must be a valid URL"
        ));
    }
    if data.report_endpoint_path.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.data.report_endpoint_path must be a non-empty path"
        ));
    }
    if data.http_timeout_secs == 0 {
        errors.push(format!(
            "clients.{key}.data.http_timeout_secs must be a positive integer"
        ));
    }
    if data.feed_bindings.is_empty() {
        errors.push(format!(
            "clients.{key}.data.feed_bindings must declare at least one feed-to-instrument binding"
        ));
    }
    let mut seen_feed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_instrument_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (index, binding) in data.feed_bindings.iter().enumerate() {
        match parse_feed_binding(key, index, binding) {
            Ok(parsed) => {
                if !seen_feed_ids.insert(parsed.feed_id.clone()) {
                    errors.push(format!(
                        "clients.{key}.data.feed_bindings[{index}].feed_id duplicates an earlier binding; each feed_id must map to exactly one instrument_id"
                    ));
                }
                let instrument_id = parsed.instrument_id.to_string();
                if !seen_instrument_ids.insert(instrument_id) {
                    errors.push(format!(
                        "clients.{key}.data.feed_bindings[{index}].instrument_id duplicates an earlier binding; each instrument_id must map to exactly one feed_id"
                    ));
                }
            }
            Err(message) => errors.push(message),
        }
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &ChainlinkSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("api_key_ssm_parameter", &secrets.api_key_ssm_parameter),
        (
            "api_secret_ssm_parameter",
            &secrets.api_secret_ssm_parameter,
        ),
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
    let api_key = resolve_field(
        context.client_key,
        "api_key_ssm_parameter",
        context.region,
        &secrets.api_key_ssm_parameter,
        resolver,
    )?;
    let api_secret = resolve_field(
        context.client_key,
        "api_secret_ssm_parameter",
        context.region,
        &secrets.api_secret_ssm_parameter,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3ChainlinkSecrets {
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
            field_name: "api_key_ssm_parameter",
            ssm_path: secrets.api_key_ssm_parameter,
        },
        ProviderSsmPathReference {
            field_name: "api_secret_ssm_parameter",
            ssm_path: secrets.api_secret_ssm_parameter,
        },
    ])
}

fn parse_secrets_config(
    context: &ProviderSecretResolveContext<'_>,
) -> Result<ChainlinkSecretsConfig, BoltV3SecretError> {
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
            source: format!("invalid chainlink secrets schema: {error}"),
        })
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(ChainlinkStrikeSourceFactory),
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
    secrets: &ResolvedBoltV3ChainlinkSecrets,
) -> Result<ChainlinkStrikeSourceConfig, BoltV3AdapterMappingError> {
    let cfg: ChainlinkDataConfig = value.clone().try_into().map_err(|error: toml::de::Error| {
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
    let feed_bindings = cfg
        .feed_bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| parse_feed_binding(client_key, index, binding))
        .collect::<Result<Vec<ChainlinkStrikeFeedBinding>, String>>()
        .map_err(|message| BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message,
        })?;
    Ok(ChainlinkStrikeSourceConfig {
        rest_base_url: cfg.rest_base_url,
        report_endpoint_path: cfg.report_endpoint_path,
        http_timeout_secs: cfg.http_timeout_secs,
        feed_bindings,
        api_key: secrets.api_key.clone(),
        api_secret: secrets.api_secret.clone(),
    })
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3ChainlinkSecrets, BoltV3AdapterMappingError> {
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

#[cfg(test)]
mod tests {
    //! Feed-binding uniqueness validation.
    //!
    //! The `[clients.<id>.data].feed_bindings` table maps each Chainlink Data
    //! Streams `feed_id` to exactly one NT resolution `instrument_id`. The live
    //! strike lookup in `bolt_v3_chainlink::strike_source` resolves a binding by
    //! `.find(|b| b.instrument_id == instrument_id)` (first match wins), so a
    //! duplicate `feed_id` or `instrument_id` silently shadows the second entry
    //! — a misconfiguration that must fail closed at config validation rather
    //! than mapping live money onto the wrong feed.

    use super::*;

    // Two distinct valid Chainlink Data Streams feed ids (0x + 64 lowercase hex)
    // and two distinct valid NT instrument ids, so that each fixture varies only
    // the dimension under test (duplicate feed_id XOR duplicate instrument_id).
    const FEED_ID_A: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const FEED_ID_B: &str = "0x0003111111111111111111111111111111111111111111111111111111111111";
    const INSTRUMENT_ID_A: &str = "BTC-USD-UP.BOLT";
    const INSTRUMENT_ID_B: &str = "BTC-USD-DOWN.BOLT";

    fn data_config_from_bindings(bindings_toml: &str) -> ChainlinkDataConfig {
        let toml_src = format!(
            r#"
rest_base_url = "https://example.invalid"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 5
{bindings_toml}
"#
        );
        toml::from_str::<ChainlinkDataConfig>(&toml_src).expect("fixture data config must parse")
    }

    fn binding_table(feed_id: &str, instrument_id: &str) -> String {
        format!(
            r#"
[[feed_bindings]]
feed_id = "{feed_id}"
instrument_id = "{instrument_id}"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 2
"#
        )
    }

    #[test]
    fn rejects_duplicate_feed_id_in_feed_bindings() {
        // Same feed_id on two bindings (distinct instrument_ids): ambiguous which
        // instrument a single feed's strike resolves onto.
        let bindings = format!(
            "{}{}",
            binding_table(FEED_ID_A, INSTRUMENT_ID_A),
            binding_table(FEED_ID_A, INSTRUMENT_ID_B),
        );
        let data = data_config_from_bindings(&bindings);

        let errors = validate_data_bounds("chainlink_strike", &data);

        assert!(
            !errors.is_empty(),
            "duplicate feed_id across feed_bindings must be rejected at validation; got no errors"
        );
        assert!(
            errors.iter().any(|e| e.contains("feed_id")),
            "expected a duplicate-feed_id error mentioning `feed_id`, got: {errors:?}"
        );
    }

    #[test]
    fn rejects_duplicate_instrument_id_in_feed_bindings() {
        // Same instrument_id on two bindings (distinct feed_ids): the
        // first-match-wins lookup silently ignores the second feed.
        let bindings = format!(
            "{}{}",
            binding_table(FEED_ID_A, INSTRUMENT_ID_A),
            binding_table(FEED_ID_B, INSTRUMENT_ID_A),
        );
        let data = data_config_from_bindings(&bindings);

        let errors = validate_data_bounds("chainlink_strike", &data);

        assert!(
            !errors.is_empty(),
            "duplicate instrument_id across feed_bindings must be rejected at validation; got no errors"
        );
        assert!(
            errors.iter().any(|e| e.contains("instrument_id")),
            "expected a duplicate-instrument_id error mentioning `instrument_id`, got: {errors:?}"
        );
    }
}
