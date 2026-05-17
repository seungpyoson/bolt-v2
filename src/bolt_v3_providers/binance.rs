//! Per-provider binding for `binance` venue config block shapes and
//! per-venue startup validation.
//!
//! Owns the concrete shape of `[venues.<name>.data]` and
//! `[venues.<name>.execution]`, and `[venues.<name>.secrets]` for
//! any venue whose `kind = "binance"` provider key is configured. Core
//! config in `crate::bolt_v3_config`
//! only owns the root/strategy envelope and raw provider-key field; the
//! provider-shaped block types and their
//! serde rules live here so provider-specific schema evolution does
//! not reach back into the envelope module.
//!
//! This module also owns the per-venue startup-validation policy for
//! Binance venues: typed deserialization of each present block,
//! cross-block presence rule ([secrets] is only allowed alongside a
//! configured adapter block), Binance data bounds, Binance execution
//! parse-time diagnostics plus current-scope fail-closed rejection, and
//! Binance secret-path ownership. The
//! cross-provider rule that [data] requires [secrets] is declared by
//! [`REQUIRED_SECRET_BLOCKS`] and enforced centrally in
//! `bolt_v3_providers::validate_venue_block`. Core startup validation in
//! `crate::bolt_v3_validate` dispatches into
//! `bolt_v3_providers::validate_venue_block`, which routes Binance
//! venues here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

use std::{any::Any, collections::BTreeMap, sync::Arc};

use nautilus_binance::{
    common::credential::Ed25519Credential,
    common::enums::{
        BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
    },
    config::BinanceDataClientConfig,
    factories::BinanceDataClientFactory,
};
use nautilus_core::string::secret::REDACTED;
use nautilus_network::websocket::TransportBackend;
use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3DataClientAdapterConfig, BoltV3VenueAdapterConfig,
    },
    bolt_v3_config::VenueBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderBinding, ProviderCredentialedBlock,
        ProviderResolvedSecrets, ProviderSecretRequirement, ProviderSecretResolveContext,
        ResolvedVenueSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
};

pub const KEY: &str = "binance";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[
    ProviderSecretRequirement {
        block: ProviderCredentialedBlock::Data,
        consumer: "Binance data venue",
    },
    ProviderSecretRequirement {
        block: ProviderCredentialedBlock::Execution,
        consumer: "Binance execution venue",
    },
];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_path", "api_secret_ssm_path"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_binance::common::credential"];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "BINANCE_ED25519_API_KEY",
    "BINANCE_ED25519_API_SECRET",
    "BINANCE_API_KEY",
    "BINANCE_API_SECRET",
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
    build_fee_provider: None,
};

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
    pub instrument_status_poll_seconds: u64,
    /// Required WebSocket transport backend passed through to NT so
    /// Bolt-v3 does not inherit the NT adapter default.
    pub transport_backend: TransportBackend,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceExecutionConfig {
    pub account_id: String,
    pub product_types: Vec<BinanceProductType>,
    pub environment: BinanceEnvironment,
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_ws_trading: String,
    pub use_ws_trading: bool,
    pub use_position_ids: bool,
    pub default_taker_fee: String,
    pub futures_leverages: BTreeMap<String, u32>,
    pub futures_margin_types: BTreeMap<String, BinanceMarginType>,
    pub treat_expired_as_canceled: bool,
    pub use_trade_lite: bool,
    pub transport_backend: TransportBackend,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BinanceProductType {
    Spot,
    UsdM,
    CoinM,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BinanceEnvironment {
    Mainnet,
    Testnet,
    Demo,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BinanceMarginType {
    Cross,
    Isolated,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BinanceSecretsConfig {
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedBoltV3BinanceSecrets {
    pub api_key: String,
    pub api_secret: String,
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

pub fn validate_venue(key: &str, venue: &VenueBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(data) = &venue.data {
        match data.clone().try_into::<BinanceDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.data: {message}")),
        }
    }
    if venue.execution.is_some() {
        errors.push(unsupported_binance_execution_message(key));
    }
    if let Some(secrets) = &venue.secrets {
        if venue.data.is_none() && venue.execution.is_none() {
            errors.push(format!(
                "venues.{key} (kind=binance) declares [secrets] but no [data] or [execution] block is configured; \
                 Binance [secrets] are only allowed alongside the adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<BinanceSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("venues.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &BinanceDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if data.product_types.is_empty() {
        errors.push(format!("venues.{key}.data.product_types must not be empty"));
    }
    let url_fields: &[(&str, &str)] = &[
        ("base_url_http", data.base_url_http.as_str()),
        ("base_url_ws", data.base_url_ws.as_str()),
    ];
    for (field, value) in url_fields {
        if value.trim().is_empty() {
            errors.push(format!("venues.{key}.data.{field} must be a non-empty URL"));
        }
    }
    // The bolt-v3 schema deliberately rejects `0` rather than treating
    // it as "polling disabled": NT's `BinanceDataClientConfig` consumes
    // this as a poll interval and a missing/zero value would leave NT
    // free to fall back to its own default cadence. Failing closed
    // here keeps the bolt-v3 instrument-status-poll cadence explicit.
    if data.instrument_status_poll_seconds == 0 {
        errors.push(format!(
            "venues.{key}.data.instrument_status_poll_seconds must be a positive integer"
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
    let secrets: BinanceSecretsConfig =
        secrets_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| BoltV3SecretError {
                venue_key: context.venue_key.to_string(),
                field: KEY.to_string(),
                ssm_path: String::new(),
                source: format!("invalid binance secrets schema: {error}"),
            })?;
    let api_secret = resolve_field(
        context.venue_key,
        "api_secret_ssm_path",
        context.region,
        &secrets.api_secret_ssm_path,
        resolver,
    )?;
    validate_api_secret_shape(context.venue_key, &api_secret).map_err(|reason| {
        BoltV3SecretError {
            venue_key: context.venue_key.to_string(),
            field: "api_secret_ssm_path".to_string(),
            ssm_path: secrets.api_secret_ssm_path.clone(),
            source: format!("resolved binance api_secret is not valid Ed25519 PKCS8 base64 key material accepted by the NautilusTrader binance adapter: {reason}"),
        }
    })?;
    let api_key = resolve_field(
        context.venue_key,
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

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.venue.data {
        Some(value) => {
            let secrets = secrets_for(context.venue_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(BinanceDataClientFactory::new()),
                config: Box::new(map_data(context.venue_key, value, secrets)?),
            })
        }
        None => None,
    };
    if context.venue.execution.is_some() {
        return Err(unsupported_binance_execution_error(context.venue_key));
    }
    let execution = None;
    Ok(BoltV3VenueAdapterConfig { data, execution })
}

fn map_data(
    venue_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3BinanceSecrets,
) -> Result<BinanceDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: BinanceDataConfig = value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            venue_key: venue_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })?;
    reject_empty_product_types(venue_key, "data.product_types", &cfg.product_types)?;
    let product_types = cfg.product_types.into_iter().map(nt_product_type).collect();
    Ok(BinanceDataClientConfig {
        product_types,
        environment: nt_environment(cfg.environment),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        api_key: Some(secrets.api_key.clone()),
        api_secret: Some(secrets.api_secret.clone()),
        instrument_status_poll_secs: cfg.instrument_status_poll_seconds,
        transport_backend: cfg.transport_backend,
    })
}

fn unsupported_binance_execution_error(venue_key: &str) -> BoltV3AdapterMappingError {
    BoltV3AdapterMappingError::ValidationInvariant {
        venue_key: venue_key.to_string(),
        field: "execution",
        message: "is not supported in the current Binance reference-data scope; Binance execution requires a separate approved runtime contract".to_string(),
    }
}

fn unsupported_binance_execution_message(venue_key: &str) -> String {
    format!(
        "venues.{venue_key}.execution {}",
        match unsupported_binance_execution_error(venue_key) {
            BoltV3AdapterMappingError::ValidationInvariant { message, .. } => message,
            _ => unreachable!("unsupported Binance execution helper only builds invariant errors"),
        }
    )
}

fn secrets_for<'a>(
    venue_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3BinanceSecrets, BoltV3AdapterMappingError> {
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

fn reject_empty_product_types(
    venue_key: &str,
    field: &'static str,
    product_types: &[BinanceProductType],
) -> Result<(), BoltV3AdapterMappingError> {
    if product_types.is_empty() {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field,
            message: "must not be empty".to_string(),
        });
    }
    Ok(())
}

fn validate_api_secret_shape(venue_key: &str, api_secret: &str) -> Result<(), String> {
    if api_secret.trim().is_empty() {
        return Err("resolved Binance api_secret is empty".to_string());
    }

    Ed25519Credential::new(venue_key.to_string(), api_secret)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "resolved Binance api_secret is not valid Ed25519 key material accepted by the NT Binance adapter: {error}"
            )
        })
}

fn nt_product_type(value: BinanceProductType) -> NtBinanceProductType {
    match value {
        BinanceProductType::Spot => NtBinanceProductType::Spot,
        BinanceProductType::UsdM => NtBinanceProductType::UsdM,
        BinanceProductType::CoinM => NtBinanceProductType::CoinM,
    }
}

fn nt_environment(value: BinanceEnvironment) -> NtBinanceEnvironment {
    match value {
        BinanceEnvironment::Mainnet => NtBinanceEnvironment::Mainnet,
        BinanceEnvironment::Testnet => NtBinanceEnvironment::Testnet,
        BinanceEnvironment::Demo => NtBinanceEnvironment::Demo,
    }
}

#[cfg(test)]
mod tests {
    use super::validate_api_secret_shape;
    use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

    fn synthetic_ed25519_pkcs8_base64() -> String {
        let mut der = vec![0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03];
        der.extend_from_slice(&[0x2B, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20]);
        der.extend(0_u8..32);
        BASE64_STANDARD.encode(der)
    }

    #[test]
    fn api_secret_shape_accepts_base64_pkcs8_ed25519() {
        let secret = synthetic_ed25519_pkcs8_base64();
        validate_api_secret_shape("test-binance-venue", &secret)
            .expect("synthetic ed25519 base64 should pass");
    }

    #[test]
    fn api_secret_shape_rejects_raw_32_byte_seed_base64() {
        let secret = BASE64_STANDARD.encode((0_u8..32).collect::<Vec<_>>());

        let error = validate_api_secret_shape("test-binance-venue", &secret)
            .expect_err("raw 32-byte ed25519 seed should fail");
        assert!(error.contains("Decoded key does not carry the Ed25519 PKCS#8 OID"));
    }

    #[test]
    fn api_secret_shape_accepts_pem_wrapped_pkcs8_ed25519() {
        let secret = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
            synthetic_ed25519_pkcs8_base64()
        );
        validate_api_secret_shape("test-binance-venue", &secret)
            .expect("synthetic ed25519 pem should pass");
    }

    #[test]
    fn api_secret_shape_rejects_short_base64_seed() {
        let secret = BASE64_STANDARD.encode((0_u8..31).collect::<Vec<_>>());

        let error = validate_api_secret_shape("test-binance-venue", &secret)
            .expect_err("short ed25519 seed should fail");
        assert!(error.contains("Decoded key does not carry the Ed25519 PKCS#8 OID"));
    }

    #[test]
    fn api_secret_shape_rejects_oid_only_false_positive() {
        let secret = BASE64_STANDARD.encode([0x2B, 0x65, 0x70]);

        let error = validate_api_secret_shape("test-binance-venue", &secret)
            .expect_err("short oid-bearing blob should fail");
        assert!(error.contains("Decoded key does not carry the Ed25519 PKCS#8 OID"));
    }

    #[test]
    fn api_secret_shape_rejects_non_key_material() {
        let error = validate_api_secret_shape("test-binance-venue", "not-a-valid-binance-secret")
            .expect_err("plain invalid string should fail");
        assert!(error.contains("valid Ed25519 key material"));
    }
}
