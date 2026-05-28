//! Data-only provider bindings for NT venue adapters.
//!
//! These bindings are deliberately thin: Bolt validates that a configured
//! client is data-only, rejects direct credential material, then passes the
//! `[data]` TOML through to the pinned NautilusTrader data-client config
//! type and factory for that venue.

use nautilus_bybit::{config::BybitDataClientConfig, factories::BybitDataClientFactory};
use nautilus_coinbase::{config::CoinbaseDataClientConfig, factories::CoinbaseDataClientFactory};
use nautilus_common::factories::{ClientConfig, DataClientFactory};
use nautilus_deribit::{config::DeribitDataClientConfig, factories::DeribitDataClientFactory};
use nautilus_kraken::{config::KrakenDataClientConfig, factories::KrakenDataClientFactory};
use nautilus_okx::{config::OKXDataClientConfig, factories::OKXDataClientFactory};
use serde::de::DeserializeOwned;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderSecretRequirement, ProviderSecretResolveContext,
        ProviderSsmPathReference, ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::BoltV3SecretError,
};

pub const BYBIT_KEY: &str = "BYBIT";
pub const COINBASE_KEY: &str = "COINBASE";
pub const DERIBIT_KEY: &str = "DERIBIT";
pub const OKX_KEY: &str = "OKX";
pub const KRAKEN_KEY: &str = "KRAKEN";

pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[];
pub const SECRET_FIELD_NAMES: &[&str] = &[];

pub const BYBIT_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_bybit::common::credential"];
pub const COINBASE_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_coinbase::common::credential"];
pub const DERIBIT_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_deribit::common::credential"];
pub const OKX_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_okx::common::credential"];
pub const KRAKEN_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_kraken::common::credential"];

pub const BYBIT_FORBIDDEN_ENV_VARS: &[&str] = &[
    "BYBIT_DEMO_API_KEY",
    "BYBIT_DEMO_API_SECRET",
    "BYBIT_TESTNET_API_KEY",
    "BYBIT_TESTNET_API_SECRET",
    "BYBIT_API_KEY",
    "BYBIT_API_SECRET",
];
pub const COINBASE_FORBIDDEN_ENV_VARS: &[&str] = &["COINBASE_API_KEY", "COINBASE_API_SECRET"];
pub const DERIBIT_FORBIDDEN_ENV_VARS: &[&str] = &[
    "DERIBIT_TESTNET_API_KEY",
    "DERIBIT_TESTNET_API_SECRET",
    "DERIBIT_API_KEY",
    "DERIBIT_API_SECRET",
];
pub const OKX_FORBIDDEN_ENV_VARS: &[&str] =
    &["OKX_API_KEY", "OKX_API_SECRET", "OKX_API_PASSPHRASE"];
pub const KRAKEN_FORBIDDEN_ENV_VARS: &[&str] = &[
    "KRAKEN_SPOT_API_KEY",
    "KRAKEN_SPOT_API_SECRET",
    "KRAKEN_FUTURES_DEMO_API_KEY",
    "KRAKEN_FUTURES_DEMO_API_SECRET",
    "KRAKEN_FUTURES_API_KEY",
    "KRAKEN_FUTURES_API_SECRET",
];

const DIRECT_CREDENTIAL_FIELDS: &[&str] = &[
    "api_key",
    "api_secret",
    "api_passphrase",
    "private_key",
    "wallet_address",
];

pub fn validate_bybit_client(key: &str, client: &ClientBlock) -> Vec<String> {
    validate_data_only_client::<BybitDataClientConfig>(BYBIT_KEY, key, client)
}

pub fn validate_coinbase_client(key: &str, client: &ClientBlock) -> Vec<String> {
    validate_data_only_client::<CoinbaseDataClientConfig>(COINBASE_KEY, key, client)
}

pub fn validate_deribit_client(key: &str, client: &ClientBlock) -> Vec<String> {
    validate_data_only_client::<DeribitDataClientConfig>(DERIBIT_KEY, key, client)
}

pub fn validate_okx_client(key: &str, client: &ClientBlock) -> Vec<String> {
    validate_data_only_client::<OKXDataClientConfig>(OKX_KEY, key, client)
}

pub fn validate_kraken_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = validate_data_only_client::<KrakenDataClientConfig>(KRAKEN_KEY, key, client);
    if let Some(data) = &client.data {
        if let Ok(parsed) = data.clone().try_into::<KrakenDataClientConfig>() {
            if let Err(error) = parsed.validate() {
                errors.push(format!("clients.{key}.data: {error}"));
            }
        }
    }
    errors
}

fn validate_data_only_client<T>(
    provider_key: &'static str,
    key: &str,
    client: &ClientBlock,
) -> Vec<String>
where
    T: DeserializeOwned,
{
    let mut errors = Vec::new();
    if client.execution.is_some() {
        errors.push(format!(
            "clients.{key} (provider={provider_key}) is data-only in the current bolt-v3 scope and must not declare an [execution] block"
        ));
    }
    if client.secrets.is_some() {
        errors.push(format!(
            "clients.{key} (provider={provider_key}) is data-only in the current bolt-v3 scope and must not declare a [secrets] block; add an explicit SSM-backed provider binding before using credentials"
        ));
    }
    let Some(data) = &client.data else {
        errors.push(format!(
            "clients.{key} (provider={provider_key}) must declare a [data] block"
        ));
        return errors;
    };
    errors.extend(validate_no_direct_credential_fields(
        provider_key,
        key,
        data,
    ));
    match data.clone().try_into::<T>() {
        Ok(_) => {}
        Err(message) => errors.push(format!("clients.{key}.data: {message}")),
    }
    errors
}

fn validate_no_direct_credential_fields(
    provider_key: &'static str,
    key: &str,
    data: &toml::Value,
) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(table) = data.as_table() else {
        return errors;
    };
    for field in DIRECT_CREDENTIAL_FIELDS {
        if table.contains_key(*field) {
            errors.push(format!(
                "clients.{key}.data.{field} must not be configured directly for provider={provider_key}; credentials must use an explicit SSM-backed [secrets] binding"
            ));
        }
    }
    errors
}

pub fn resolve_unsupported_secrets(
    context: ProviderSecretResolveContext<'_>,
    _resolver: &mut dyn SsmSecretResolver,
) -> Result<ResolvedClientSecrets, BoltV3SecretError> {
    Err(BoltV3SecretError {
        client_key: context.client_key.to_string(),
        field: "secrets".to_string(),
        source: format!(
            "provider `{}` is data-only in this scope and does not support [secrets]",
            context.client.venue.as_str()
        ),
    })
}

pub fn configured_secret_paths(
    context: ProviderSecretResolveContext<'_>,
) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError> {
    if context.client.secrets.is_some() {
        Err(BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "secrets".to_string(),
            source: format!(
                "provider `{}` is data-only in this scope and does not support [secrets]",
                context.client.venue.as_str()
            ),
        })
    } else {
        Ok(Vec::new())
    }
}

pub fn map_bybit_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<BybitDataClientConfig, _>(context, BybitDataClientFactory::new())
}

pub fn map_coinbase_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<CoinbaseDataClientConfig, _>(context, CoinbaseDataClientFactory::new())
}

pub fn map_deribit_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<DeribitDataClientConfig, _>(context, DeribitDataClientFactory::new())
}

pub fn map_okx_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<OKXDataClientConfig, _>(context, OKXDataClientFactory::new())
}

pub fn map_kraken_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<KrakenDataClientConfig, _>(context, KrakenDataClientFactory::new())
}

fn map_data_only_adapters<T, F>(
    context: ProviderAdapterMapContext<'_>,
    factory: F,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError>
where
    T: ClientConfig + DeserializeOwned + 'static,
    F: DataClientFactory + 'static,
{
    let data = match &context.client.data {
        Some(value) => Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(factory),
            config: Box::new(parse_data_config::<T>(context.client_key, value)?),
        }),
        None => None,
    };
    Ok(BoltV3ClientAdapterConfig {
        data,
        execution: None,
    })
}

fn parse_data_config<T>(
    client_key: &str,
    value: &toml::Value,
) -> Result<T, BoltV3AdapterMappingError>
where
    T: DeserializeOwned,
{
    value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })
}
