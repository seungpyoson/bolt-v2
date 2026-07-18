//! Data-only provider bindings for NT venue adapters.
//!
//! These bindings are deliberately thin: Bolt validates that a configured
//! client is data-only, rejects direct credential material, then passes the
//! `[data]` TOML through to the pinned NautilusTrader data-client config
//! type and factory for that venue.

use nautilus_bitmex::{config::BitmexDataClientConfig, factories::BitmexDataClientFactory};
use nautilus_bybit::{config::BybitDataClientConfig, factories::BybitDataClientFactory};
use nautilus_coinbase::{config::CoinbaseDataClientConfig, factories::CoinbaseDataClientFactory};
use nautilus_common::factories::{ClientConfig, DataClientFactory};
use nautilus_deribit::{config::DeribitDataClientConfig, factories::DeribitDataClientFactory};
use nautilus_kraken::{config::KrakenDataClientConfig, factories::KrakenDataClientFactory};
use nautilus_okx::{config::OKXDataClientConfig, factories::OKXDataClientFactory};
use serde::{Deserialize, de::DeserializeOwned};

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

pub const BITMEX_KEY: &str = "BITMEX";
pub const BYBIT_KEY: &str = "BYBIT";
pub const COINBASE_KEY: &str = "COINBASE";
pub const DERIBIT_KEY: &str = "DERIBIT";
pub const OKX_KEY: &str = "OKX";
pub const KRAKEN_KEY: &str = "KRAKEN";

pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const NO_REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[];
pub const NO_SECRET_FIELD_NAMES: &[&str] = &[];

/// Compile-time anchor for the data-only `*_CREDENTIAL_LOG_MODULES` constants
/// below. Unlike the trading providers (polymarket/binance), these data-only
/// adapters expose no credential type that bolt code otherwise imports, so the
/// module-path strings would have no compile-time anchor and could silently
/// drift if an NT rev bump relocated the module. Taking each public credential
/// type at the exact path encoded in the matching constant makes the build fail
/// before the constant can go stale; the NT rev itself is pinned in
/// `Cargo.toml`, the single source of truth for the rev. The function is never
/// called — it exists only so the type paths are checked by the compiler.
#[allow(dead_code)]
fn _credential_log_module_paths_exist(
    _bitmex: &nautilus_bitmex::common::credential::Credential,
    _bybit: &nautilus_bybit::common::credential::Credential,
    _coinbase: &nautilus_coinbase::common::credential::CoinbaseCredential,
    _deribit: &nautilus_deribit::common::credential::Credential,
    _kraken: &nautilus_kraken::common::credential::KrakenCredential,
    _okx: &nautilus_okx::common::credential::Credential,
) {
}

pub const BITMEX_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_bitmex::common::credential"];
pub const BYBIT_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_bybit::common::credential"];
pub const COINBASE_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_coinbase::common::credential"];
pub const DERIBIT_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_deribit::common::credential"];
pub const OKX_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_okx::common::credential"];
pub const KRAKEN_CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_kraken::common::credential"];

pub const BITMEX_FORBIDDEN_ENV_VARS: &[&str] = &[
    "BITMEX_TESTNET_API_KEY",
    "BITMEX_TESTNET_API_SECRET",
    "BITMEX_API_KEY",
    "BITMEX_API_SECRET",
];
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
    "username",
    "password",
    "app_key",
    "wallet_address",
];

trait DataConfigBoundary: ClientConfig + Sized + 'static {
    fn parse(value: &toml::Value) -> Result<Self, String>;
}

#[derive(Debug, Deserialize)]
struct RequiredOkxBookHealthControls {
    book_stale_check_interval_secs: u64,
    book_stale_threshold_secs: u64,
    book_snapshot_timeout_secs: u64,
}

fn deserialize_data_config<T>(value: &toml::Value) -> Result<T, String>
where
    T: DeserializeOwned,
{
    value
        .clone()
        .try_into()
        .map_err(|error: toml::de::Error| error.to_string())
}

macro_rules! impl_direct_data_config_boundary {
    ($($config:ty),+ $(,)?) => {
        $(
            impl DataConfigBoundary for $config {
                fn parse(value: &toml::Value) -> Result<Self, String> {
                    deserialize_data_config(value)
                }
            }
        )+
    };
}

impl_direct_data_config_boundary!(
    BitmexDataClientConfig,
    BybitDataClientConfig,
    CoinbaseDataClientConfig,
    DeribitDataClientConfig,
);

impl DataConfigBoundary for OKXDataClientConfig {
    fn parse(value: &toml::Value) -> Result<Self, String> {
        let controls = deserialize_data_config::<RequiredOkxBookHealthControls>(value)?;
        let mut config = deserialize_data_config::<Self>(value)?;
        config.book_stale_check_interval_secs = controls.book_stale_check_interval_secs;
        config.book_stale_threshold_secs = controls.book_stale_threshold_secs;
        config.book_snapshot_timeout_secs = controls.book_snapshot_timeout_secs;
        Ok(config)
    }
}

impl DataConfigBoundary for KrakenDataClientConfig {
    fn parse(value: &toml::Value) -> Result<Self, String> {
        let config = deserialize_data_config::<Self>(value)?;
        config.validate().map_err(|error| error.to_string())?;
        Ok(config)
    }
}

pub fn validate_bitmex_client(key: &str, client: &ClientBlock) -> Vec<String> {
    validate_data_only_client::<BitmexDataClientConfig>(BITMEX_KEY, key, client)
}

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
    validate_data_only_client::<KrakenDataClientConfig>(KRAKEN_KEY, key, client)
}

fn validate_data_only_client<T>(
    provider_key: &'static str,
    key: &str,
    client: &ClientBlock,
) -> Vec<String>
where
    T: DataConfigBoundary,
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
    match parse_data_only_config::<T>(provider_key, key, data) {
        Ok(_) => {}
        Err(config_errors) => errors.extend(config_errors),
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

fn parse_data_only_config<T>(
    provider_key: &'static str,
    key: &str,
    value: &toml::Value,
) -> Result<T, Vec<String>>
where
    T: DataConfigBoundary,
{
    let mut errors = validate_no_direct_credential_fields(provider_key, key, value);
    match T::parse(value) {
        Ok(config) if errors.is_empty() => Ok(config),
        Ok(_) => Err(errors),
        Err(message) => {
            errors.push(format!(
                "clients.{key}.data: NT {provider_key} data-client config: {message}"
            ));
            Err(errors)
        }
    }
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

pub fn map_bitmex_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<BitmexDataClientConfig, _>(
        context,
        BitmexDataClientFactory::new(),
        BITMEX_KEY,
    )
}

pub fn map_bybit_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<BybitDataClientConfig, _>(
        context,
        BybitDataClientFactory::new(),
        BYBIT_KEY,
    )
}

pub fn map_coinbase_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<CoinbaseDataClientConfig, _>(
        context,
        CoinbaseDataClientFactory::new(),
        COINBASE_KEY,
    )
}

pub fn map_deribit_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<DeribitDataClientConfig, _>(
        context,
        DeribitDataClientFactory::new(),
        DERIBIT_KEY,
    )
}

pub fn map_okx_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<OKXDataClientConfig, _>(context, OKXDataClientFactory::new(), OKX_KEY)
}

pub fn map_kraken_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    map_data_only_adapters::<KrakenDataClientConfig, _>(
        context,
        KrakenDataClientFactory::new(),
        KRAKEN_KEY,
    )
}

fn map_data_only_adapters<T, F>(
    context: ProviderAdapterMapContext<'_>,
    factory: F,
    provider_key: &'static str,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError>
where
    T: DataConfigBoundary,
    F: DataClientFactory + 'static,
{
    let value =
        context
            .client
            .data
            .as_ref()
            .ok_or_else(|| BoltV3AdapterMappingError::SchemaParse {
                client_key: context.client_key.to_string(),
                block: "data",
                message: format!("provider {provider_key} requires a [data] block"),
            })?;
    Ok(BoltV3ClientAdapterConfig {
        data: Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(factory),
            config: Box::new(parse_data_config::<T>(
                context.client_key,
                provider_key,
                value,
            )?),
        }),
        execution: None,
    })
}

fn parse_data_config<T>(
    client_key: &str,
    provider_key: &'static str,
    value: &toml::Value,
) -> Result<T, BoltV3AdapterMappingError>
where
    T: DataConfigBoundary,
{
    parse_data_only_config::<T>(provider_key, client_key, value).map_err(|messages| {
        BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message: messages.join("; "),
        }
    })
}

#[cfg(test)]
mod forbidden_env_var_anchor_tests {
    //! Anchors the data-only `*_FORBIDDEN_ENV_VARS` blocklists to the env-var names the pinned
    //! NautilusTrader adapters actually read in their `credential_env_vars()` accessors. The
    //! `check_no_forbidden_credential_env_vars` gate fails closed only on the names listed in
    //! these constants; if an NT rev bump renamed or added a credential env var, the blocklist
    //! would silently miss it and a stray env var could feed credentials into a data-only client.
    //! This test fails in CI before that can happen. The NT rev is pinned in `Cargo.toml` (single
    //! source of truth). This is the runtime-name counterpart to the compile-time
    //! `_credential_log_module_paths_exist` anchor for the log-module-path strings.
    use super::{
        BITMEX_FORBIDDEN_ENV_VARS, BYBIT_FORBIDDEN_ENV_VARS, COINBASE_FORBIDDEN_ENV_VARS,
        DERIBIT_FORBIDDEN_ENV_VARS, KRAKEN_FORBIDDEN_ENV_VARS, OKX_FORBIDDEN_ENV_VARS,
    };

    fn assert_blocklist_covers(blocklist: &[&str], names: &[&str], adapter: &str) {
        for name in names {
            assert!(
                blocklist.contains(name),
                "{adapter}_FORBIDDEN_ENV_VARS is missing the pinned NautilusTrader credential env \
                 var `{name}`; the adapter reads it but the bolt blocklist does not fail closed on it"
            );
        }
    }

    #[test]
    fn forbidden_env_vars_cover_nt_credential_env_vars() {
        use nautilus_bitmex::common::{
            credential::credential_env_vars as bitmex_env_vars, enums::BitmexEnvironment,
        };
        use nautilus_bybit::common::{
            credential::credential_env_vars as bybit_env_vars, enums::BybitEnvironment,
        };
        use nautilus_coinbase::common::credential::credential_env_vars as coinbase_env_vars;
        use nautilus_deribit::common::{
            credential::credential_env_vars as deribit_env_vars, enums::DeribitEnvironment,
        };
        use nautilus_kraken::common::{
            credential::credential_env_vars as kraken_env_vars,
            enums::{KrakenEnvironment, KrakenProductType},
        };
        use nautilus_okx::common::credential::credential_env_vars as okx_env_vars;

        let mut bitmex = Vec::new();
        for env in [BitmexEnvironment::Testnet, BitmexEnvironment::Mainnet] {
            let (key, secret) = bitmex_env_vars(env);
            bitmex.extend([key, secret]);
        }
        assert_blocklist_covers(BITMEX_FORBIDDEN_ENV_VARS, &bitmex, "BITMEX");

        let mut bybit = Vec::new();
        for env in [
            BybitEnvironment::Demo,
            BybitEnvironment::Testnet,
            BybitEnvironment::Mainnet,
        ] {
            let (key, secret) = bybit_env_vars(env);
            bybit.extend([key, secret]);
        }
        assert_blocklist_covers(BYBIT_FORBIDDEN_ENV_VARS, &bybit, "BYBIT");

        let (ck, cs) = coinbase_env_vars();
        assert_blocklist_covers(COINBASE_FORBIDDEN_ENV_VARS, &[ck, cs], "COINBASE");

        let mut deribit = Vec::new();
        for env in [DeribitEnvironment::Testnet, DeribitEnvironment::Mainnet] {
            let (key, secret) = deribit_env_vars(env);
            deribit.extend([key, secret]);
        }
        assert_blocklist_covers(DERIBIT_FORBIDDEN_ENV_VARS, &deribit, "DERIBIT");

        let (ok, os, op) = okx_env_vars();
        assert_blocklist_covers(OKX_FORBIDDEN_ENV_VARS, &[ok, os, op], "OKX");

        let mut kraken = Vec::new();
        for product in [KrakenProductType::Spot, KrakenProductType::Futures] {
            for env in [KrakenEnvironment::Live, KrakenEnvironment::Demo] {
                let (key, secret) = kraken_env_vars(product, env);
                kraken.extend([key, secret]);
            }
        }
        assert_blocklist_covers(KRAKEN_FORBIDDEN_ENV_VARS, &kraken, "KRAKEN");
    }
}
