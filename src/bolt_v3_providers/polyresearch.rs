//! PolyResearch reference-price WebSocket authentication helpers and provider
//! binding.
//!
//! PRR auth is a query parameter named `key`. Bolt keeps the endpoint and
//! credential as separate SSM values, then constructs the credentialed URL once
//! at the provider edge.

use std::{any::Any, cell::RefCell, rc::Rc, sync::Arc};

use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
};
use nautilus_core::string::secret::REDACTED;
use nautilus_model::identifiers::{ClientId, Venue};
use nautilus_network::websocket::TransportBackend;
use serde::Deserialize;
use url::Url;
use zeroize::Zeroizing;

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
};

const POLYRESEARCH_API_KEY_QUERY_FIELD: &str = "key";
const POLYRESEARCH_LEGACY_API_KEY_QUERY_FIELD: &str = "apiKey";
pub const KEY: &str = "POLYRESEARCH_REFERENCE_PRICE";
pub const REFERENCE_PRICE_PROVIDER_KEY: &str = "polyresearch_ws";
pub const POLYRESEARCH_REFERENCE_PRICE_SUPPORTED_ASSETS: &[&str] = &["BTC", "ETH", "SOL", "XRP"];
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "PolyResearch reference-price source",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_parameter"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &[];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[];

const FACTORY_NAME: &str = KEY;
const CONFIG_TYPE: &str = "PolyResearchReferencePriceClientConfig";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PolyResearchReferencePriceDataConfig {
    pub websocket_endpoint: String,
    pub transport_backend: TransportBackend,
    pub heartbeat_secs: Option<u64>,
    pub heartbeat_message: Option<String>,
    pub reconnect_timeout_ms: u64,
    pub reconnect_delay_initial_ms: u64,
    pub reconnect_delay_max_ms: u64,
    pub reconnect_backoff_factor: f64,
    pub reconnect_jitter_ms: u64,
    pub idle_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolyResearchReferencePriceSecretsConfig {
    pub api_key_ssm_parameter: String,
}

#[derive(Clone)]
pub struct ResolvedBoltV3PolyResearchSecrets {
    pub api_key: Zeroizing<String>,
}

impl std::fmt::Debug for ResolvedBoltV3PolyResearchSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3PolyResearchSecrets")
            .field("api_key", &REDACTED)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3PolyResearchSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn redaction_values(&self) -> Vec<&str> {
        vec![self.api_key.as_str()]
    }
}

#[derive(Clone)]
pub struct PolyResearchReferencePriceClientConfig {
    pub websocket_endpoint: String,
    pub transport_backend: TransportBackend,
    pub heartbeat_secs: Option<u64>,
    pub heartbeat_message: Option<String>,
    pub reconnect_timeout_ms: u64,
    pub reconnect_delay_initial_ms: u64,
    pub reconnect_delay_max_ms: u64,
    pub reconnect_backoff_factor: f64,
    pub reconnect_jitter_ms: u64,
    pub idle_timeout_ms: u64,
    pub api_key: Zeroizing<String>,
}

impl std::fmt::Debug for PolyResearchReferencePriceClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolyResearchReferencePriceClientConfig")
            .field("websocket_endpoint", &self.websocket_endpoint)
            .field("transport_backend", &self.transport_backend)
            .field("heartbeat_secs", &self.heartbeat_secs)
            .field("heartbeat_message", &self.heartbeat_message)
            .field("reconnect_timeout_ms", &self.reconnect_timeout_ms)
            .field(
                "reconnect_delay_initial_ms",
                &self.reconnect_delay_initial_ms,
            )
            .field("reconnect_delay_max_ms", &self.reconnect_delay_max_ms)
            .field("reconnect_backoff_factor", &self.reconnect_backoff_factor)
            .field("reconnect_jitter_ms", &self.reconnect_jitter_ms)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("api_key", &REDACTED)
            .finish()
    }
}

impl ClientConfig for PolyResearchReferencePriceClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
pub struct PolyResearchReferencePriceClientFactory;

impl DataClientFactory for PolyResearchReferencePriceClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<PolyResearchReferencePriceClientConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!("PolyResearch reference factory received wrong config")
            })?;
        Ok(Box::new(PolyResearchReferencePriceClient {
            client_id: ClientId::from(name),
            _config: config.clone(),
            connected: false,
        }))
    }

    fn name(&self) -> &str {
        FACTORY_NAME
    }

    fn config_type(&self) -> &str {
        CONFIG_TYPE
    }
}

#[derive(Debug)]
struct PolyResearchReferencePriceClient {
    client_id: ClientId,
    _config: PolyResearchReferencePriceClientConfig,
    connected: bool,
}

#[async_trait::async_trait(?Send)]
impl DataClient for PolyResearchReferencePriceClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<Venue> {
        None
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn is_disconnected(&self) -> bool {
        !self.connected
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        Ok(())
    }
}

pub struct PolyResearchAuthConfig {
    pub websocket_endpoint: String,
    pub api_key: String,
}

pub fn polyresearch_websocket_url(config: &PolyResearchAuthConfig) -> Result<Url, String> {
    validate_secret_field("api_key", &config.api_key)?;
    let mut url = validate_websocket_endpoint(&config.websocket_endpoint)?;
    if let Some(field) = polyresearch_credential_query_field(&url) {
        return Err(format!(
            "polyresearch websocket_endpoint must not contain credential query `{field}`; configure api_key separately"
        ));
    }
    url.query_pairs_mut()
        .append_pair(POLYRESEARCH_API_KEY_QUERY_FIELD, &config.api_key);
    Ok(url)
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.execution.is_some() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) is data-only and must not declare an [execution] block"
        ));
    }
    if let Some(data) = &client.data {
        match data
            .clone()
            .try_into::<PolyResearchReferencePriceDataConfig>()
        {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(secrets) = &client.secrets {
        if client.data.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [secrets] but no [data] block is configured"
            ));
        }
        match secrets
            .clone()
            .try_into::<PolyResearchReferencePriceSecretsConfig>()
        {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &PolyResearchReferencePriceDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(message) = validate_websocket_endpoint(&data.websocket_endpoint) {
        errors.push(format!("clients.{key}.data.websocket_endpoint: {message}"));
    }
    if let Ok(url) = Url::parse(&data.websocket_endpoint)
        && let Some(field) = polyresearch_credential_query_field(&url)
    {
        errors.push(format!(
            "clients.{key}.data.websocket_endpoint must not contain credential query `{field}`; configure api_key separately"
        ));
    }
    validate_positive_optional_u64(
        &format!("clients.{key}.data.heartbeat_secs"),
        data.heartbeat_secs,
        &mut errors,
    );
    validate_positive_u64(
        &format!("clients.{key}.data.reconnect_timeout_ms"),
        data.reconnect_timeout_ms,
        &mut errors,
    );
    validate_positive_u64(
        &format!("clients.{key}.data.reconnect_delay_initial_ms"),
        data.reconnect_delay_initial_ms,
        &mut errors,
    );
    validate_positive_u64(
        &format!("clients.{key}.data.reconnect_delay_max_ms"),
        data.reconnect_delay_max_ms,
        &mut errors,
    );
    if !data.reconnect_backoff_factor.is_finite() || data.reconnect_backoff_factor <= 0.0 {
        errors.push(format!(
            "clients.{key}.data.reconnect_backoff_factor must be positive and finite"
        ));
    }
    validate_positive_u64(
        &format!("clients.{key}.data.reconnect_jitter_ms"),
        data.reconnect_jitter_ms,
        &mut errors,
    );
    validate_positive_u64(
        &format!("clients.{key}.data.idle_timeout_ms"),
        data.idle_timeout_ms,
        &mut errors,
    );
    errors
}

fn polyresearch_credential_query_field(url: &Url) -> Option<&'static str> {
    for (field, _) in url.query_pairs() {
        if field.eq_ignore_ascii_case(POLYRESEARCH_API_KEY_QUERY_FIELD) {
            return Some(POLYRESEARCH_API_KEY_QUERY_FIELD);
        }
        if field.eq_ignore_ascii_case(POLYRESEARCH_LEGACY_API_KEY_QUERY_FIELD) {
            return Some(POLYRESEARCH_LEGACY_API_KEY_QUERY_FIELD);
        }
    }
    None
}

fn validate_positive_optional_u64(field: &str, value: Option<u64>, errors: &mut Vec<String>) {
    if value.is_some_and(|value| value == 0) {
        errors.push(format!("{field} must be positive when configured"));
    }
}

fn validate_positive_u64(field: &str, value: u64, errors: &mut Vec<String>) {
    if value == 0 {
        errors.push(format!("{field} must be positive"));
    }
}

fn validate_secret_paths(
    key: &str,
    secrets: &PolyResearchReferencePriceSecretsConfig,
) -> Vec<String> {
    crate::bolt_v3_validate::validate_ssm_parameter_path(
        key,
        "api_key_ssm_parameter",
        &secrets.api_key_ssm_parameter,
    )
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
    Ok(Arc::new(ResolvedBoltV3PolyResearchSecrets {
        api_key: Zeroizing::new(api_key),
    }))
}

pub fn configured_secret_paths(
    context: ProviderSecretResolveContext<'_>,
) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    Ok(vec![ProviderSsmPathReference {
        field_name: "api_key_ssm_parameter",
        ssm_path: secrets.api_key_ssm_parameter,
    }])
}

fn parse_secrets_config(
    context: &ProviderSecretResolveContext<'_>,
) -> Result<PolyResearchReferencePriceSecretsConfig, BoltV3SecretError> {
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
            source: format!("invalid polyresearch secrets schema: {error}"),
        })
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(PolyResearchReferencePriceClientFactory),
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
    secrets: &ResolvedBoltV3PolyResearchSecrets,
) -> Result<PolyResearchReferencePriceClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolyResearchReferencePriceDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: client_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    if let Some(message) = validate_data_bounds(client_key, &cfg).into_iter().next() {
        return Err(BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message,
        });
    }
    Ok(PolyResearchReferencePriceClientConfig {
        websocket_endpoint: cfg.websocket_endpoint,
        transport_backend: cfg.transport_backend,
        heartbeat_secs: cfg.heartbeat_secs,
        heartbeat_message: cfg.heartbeat_message,
        reconnect_timeout_ms: cfg.reconnect_timeout_ms,
        reconnect_delay_initial_ms: cfg.reconnect_delay_initial_ms,
        reconnect_delay_max_ms: cfg.reconnect_delay_max_ms,
        reconnect_backoff_factor: cfg.reconnect_backoff_factor,
        reconnect_jitter_ms: cfg.reconnect_jitter_ms,
        idle_timeout_ms: cfg.idle_timeout_ms,
        api_key: secrets.api_key.clone(),
    })
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3PolyResearchSecrets, BoltV3AdapterMappingError> {
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
    use super::*;

    fn fixture_config() -> PolyResearchReferencePriceClientConfig {
        PolyResearchReferencePriceClientConfig {
            websocket_endpoint: "wss://ws.polyresearch.xyz/reference".to_string(),
            transport_backend: TransportBackend::Sockudo,
            heartbeat_secs: Some(5),
            heartbeat_message: Some("ping".to_string()),
            reconnect_timeout_ms: 5_000,
            reconnect_delay_initial_ms: 250,
            reconnect_delay_max_ms: 5_000,
            reconnect_backoff_factor: 1.5,
            reconnect_jitter_ms: 100,
            idle_timeout_ms: 10_000,
            api_key: Zeroizing::new("polyresearch-api-key".to_string()),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn connect_disconnect_update_nt_data_client_connected_state() {
        let mut client = PolyResearchReferencePriceClient {
            client_id: ClientId::from("polyresearch_reference"),
            _config: fixture_config(),
            connected: false,
        };

        assert!(client.is_disconnected());

        client
            .connect()
            .await
            .expect("polyresearch reference connect should succeed");
        assert!(client.is_connected());

        client
            .disconnect()
            .await
            .expect("polyresearch reference disconnect should succeed");
        assert!(client.is_disconnected());
    }
}

fn validate_secret_field(field: &'static str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err(format!("polyresearch {field} is invalid"));
    }
    Ok(())
}

fn validate_websocket_endpoint(value: &str) -> Result<Url, String> {
    if value.trim().is_empty() || value.trim() != value || value.chars().any(char::is_whitespace) {
        return Err("polyresearch websocket_endpoint must be a non-empty wss URL".to_string());
    }
    let url = Url::parse(value)
        .map_err(|_| "polyresearch websocket_endpoint must be a valid wss URL".to_string())?;
    if url.scheme() != "wss" || !url.has_host() || !value[url.scheme().len()..].starts_with("://") {
        return Err("polyresearch websocket_endpoint must be a valid wss URL".to_string());
    }
    Ok(url)
}
