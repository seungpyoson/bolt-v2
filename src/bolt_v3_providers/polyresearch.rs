//! PolyResearch reference-price WebSocket authentication helpers and provider
//! binding.
//!
//! PRR auth is a query parameter named `key`. Bolt keeps the endpoint and
//! credential as separate SSM values, then constructs the credentialed URL once
//! at the provider edge.

use std::{any::Any, cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc, time::Duration};

use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
    messages::data::{SubscribeCustomData, UnsubscribeCustomData},
};
use nautilus_core::{Params, string::secret::REDACTED};
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
    bolt_v3_reference_price::{
        REFERENCE_PRICE_ASSET_PARAM, REFERENCE_PRICE_PROVIDER_PARAM,
        REFERENCE_PRICE_SOURCE_KEY_PARAM, REFERENCE_PRICE_SYMBOL_PARAM, ReferencePriceUpdate,
        ReferenceQuoteProvenance,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
};

const POLYRESEARCH_API_KEY_QUERY_FIELD: &str = "key";
const POLYRESEARCH_LEGACY_API_KEY_QUERY_FIELD: &str = "apiKey";
const POLYRESEARCH_PRICE_FEED_FRAME_TYPE: &str = "price_feed";
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
            subscriptions: BTreeMap::new(),
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
    subscriptions: BTreeMap<String, PolyResearchReferenceSubscription>,
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
        self.subscriptions.clear();
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        self.subscriptions.clear();
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

    fn subscribe(&mut self, cmd: SubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            polyresearch_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        self.subscriptions
            .insert(subscription.source_id.clone(), subscription);
        Ok(())
    }

    fn unsubscribe(&mut self, cmd: &UnsubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            polyresearch_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        self.subscriptions.remove(&subscription.source_id);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolyResearchReferenceSubscription {
    pub(crate) asset: String,
    pub(crate) source_id: String,
    pub(crate) symbol: String,
}

fn polyresearch_reference_subscription_from_command(
    data_type: &nautilus_model::data::DataType,
    params: Option<&Params>,
) -> Result<PolyResearchReferenceSubscription, String> {
    let data_type_owner = ReferenceSubscriptionFieldOwner::DataType;
    let params_owner = ReferenceSubscriptionFieldOwner::Params;
    let metadata = data_type.metadata().ok_or_else(|| {
        format!(
            "PolyResearch reference subscription missing {} metadata",
            data_type_owner.as_str()
        )
    })?;
    let asset = required_reference_field(metadata, REFERENCE_PRICE_ASSET_PARAM, data_type_owner)?;
    let source_id =
        required_reference_field(metadata, REFERENCE_PRICE_SOURCE_KEY_PARAM, data_type_owner)?;
    let provider =
        required_reference_field(metadata, REFERENCE_PRICE_PROVIDER_PARAM, data_type_owner)?;
    if provider != REFERENCE_PRICE_PROVIDER_KEY {
        return Err(format!(
            "PolyResearch reference subscription provider must be {REFERENCE_PRICE_PROVIDER_KEY}"
        ));
    }
    let expected_data_type =
        ReferencePriceUpdate::data_type_for(asset, source_id, REFERENCE_PRICE_PROVIDER_KEY)?;
    if &expected_data_type != data_type {
        return Err(format!(
            "PolyResearch reference subscription {} metadata is inconsistent",
            data_type_owner.as_str()
        ));
    }

    let params = params.ok_or_else(|| {
        format!(
            "PolyResearch reference subscription missing command {}",
            params_owner.as_str()
        )
    })?;
    require_matching_reference_param(params, REFERENCE_PRICE_ASSET_PARAM, asset)?;
    require_matching_reference_param(params, REFERENCE_PRICE_SOURCE_KEY_PARAM, source_id)?;
    require_matching_reference_param(params, REFERENCE_PRICE_PROVIDER_PARAM, provider)?;
    let symbol = required_reference_field(params, REFERENCE_PRICE_SYMBOL_PARAM, params_owner)?;

    Ok(PolyResearchReferenceSubscription {
        asset: asset.to_string(),
        source_id: source_id.to_string(),
        symbol: symbol.to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReferenceSubscriptionFieldOwner {
    DataType,
    Params,
}

impl ReferenceSubscriptionFieldOwner {
    const fn as_str(self) -> &'static str {
        match self {
            Self::DataType => stringify!(data_type),
            Self::Params => stringify!(params),
        }
    }
}

fn required_reference_field<'a>(
    params: &'a Params,
    key: &'static str,
    owner: ReferenceSubscriptionFieldOwner,
) -> Result<&'a str, String> {
    params.get_str(key).ok_or_else(|| {
        format!(
            "PolyResearch reference subscription missing {}.{key}",
            owner.as_str()
        )
    })
}

fn require_matching_reference_param(
    params: &Params,
    key: &'static str,
    expected: &str,
) -> Result<(), String> {
    let actual = required_reference_field(params, key, ReferenceSubscriptionFieldOwner::Params)?;
    if actual != expected {
        return Err(format!(
            "PolyResearch reference subscription {}.{key} does not match {} metadata",
            ReferenceSubscriptionFieldOwner::Params.as_str(),
            ReferenceSubscriptionFieldOwner::DataType.as_str()
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct PolyResearchPriceFrame {
    r#type: String,
    feed: Option<String>,
    timestamp: Option<u64>,
    data: Option<PolyResearchPriceData>,
}

#[derive(Debug, Deserialize)]
struct PolyResearchPriceData {
    feed: String,
    price: f64,
    bid: Option<f64>,
    ask: Option<f64>,
    timestamp: u64,
}

pub(crate) fn polyresearch_reference_update_from_price_frame(
    subscription: &PolyResearchReferenceSubscription,
    frame: &str,
    received_ts_ms: u64,
) -> Result<Option<ReferencePriceUpdate>, String> {
    let parsed = serde_json::from_str::<PolyResearchPriceFrame>(frame)
        .map_err(|error| format!("invalid PolyResearch price frame JSON: {error}"))?;
    if parsed.r#type != POLYRESEARCH_PRICE_FEED_FRAME_TYPE {
        return Ok(None);
    }

    let data = parsed
        .data
        .ok_or_else(|| "PolyResearch price_feed frame missing data".to_string())?;
    if parsed.feed.as_deref().is_some_and(|feed| feed != data.feed) {
        return Err("PolyResearch price_feed top-level feed does not match data.feed".to_string());
    }
    if data.feed != subscription.symbol {
        return Ok(None);
    }
    if parsed
        .timestamp
        .is_some_and(|timestamp| timestamp != data.timestamp)
    {
        return Err(
            "PolyResearch price_feed top-level timestamp does not match data.timestamp".to_string(),
        );
    }

    let observed_ts_ms = u64::try_from(Duration::from_secs(data.timestamp).as_millis())
        .map_err(|_| "PolyResearch price_feed timestamp overflows milliseconds".to_string())?;
    ReferencePriceUpdate::try_new_with_provenance(
        subscription.asset.as_str(),
        subscription.source_id.as_str(),
        REFERENCE_PRICE_PROVIDER_KEY,
        subscription.symbol.as_str(),
        data.price,
        data.bid,
        data.ask,
        observed_ts_ms,
        received_ts_ms,
        ReferenceQuoteProvenance::empty(),
    )
    .map(Some)
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
    use nautilus_core::{UUID4, UnixNanos};

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
            subscriptions: BTreeMap::new(),
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

    #[test]
    fn subscribe_custom_data_records_prr_reference_subscription() {
        let mut client = PolyResearchReferencePriceClient {
            client_id: ClientId::from("polyresearch_reference"),
            _config: fixture_config(),
            subscriptions: BTreeMap::new(),
            connected: false,
        };
        let mut params = Params::new();
        params.insert(
            REFERENCE_PRICE_ASSET_PARAM.to_string(),
            serde_json::json!("BTC"),
        );
        params.insert(
            REFERENCE_PRICE_SOURCE_KEY_PARAM.to_string(),
            serde_json::json!("polyresearch_primary"),
        );
        params.insert(
            REFERENCE_PRICE_PROVIDER_PARAM.to_string(),
            serde_json::json!(REFERENCE_PRICE_PROVIDER_KEY),
        );
        params.insert(
            REFERENCE_PRICE_SYMBOL_PARAM.to_string(),
            serde_json::json!("BTC/USD"),
        );

        client
            .subscribe(SubscribeCustomData::new(
                Some(ClientId::from("polyresearch_reference")),
                None,
                ReferencePriceUpdate::data_type_for(
                    "BTC",
                    "polyresearch_primary",
                    REFERENCE_PRICE_PROVIDER_KEY,
                )
                .expect("reference price data type should build"),
                UUID4::new(),
                UnixNanos::default(),
                None,
                Some(params),
            ))
            .expect("PRR reference subscription should be accepted");

        let subscription = client
            .subscriptions
            .get("polyresearch_primary")
            .expect("PRR reference subscription should be recorded");
        assert_eq!(subscription.asset, "BTC");
        assert_eq!(subscription.source_id, "polyresearch_primary");
        assert_eq!(subscription.symbol, "BTC/USD");
    }

    #[test]
    fn price_feed_frame_maps_to_reference_price_update() {
        let subscription = PolyResearchReferenceSubscription {
            asset: "BTC".to_string(),
            source_id: "polyresearch_primary".to_string(),
            symbol: "BTC/USD".to_string(),
        };

        let update = polyresearch_reference_update_from_price_frame(
            &subscription,
            r#"{"type":"price_feed","feed":"BTC/USD","timestamp":1774672588,"data":{"feed":"BTC/USD","price":66300.25,"bid":66299.5,"ask":66301.0,"timestamp":1774672588}}"#,
            1774672589123,
        )
        .expect("valid PRR price_feed frame should parse")
        .expect("matching PRR price_feed frame should emit an update");

        assert_eq!(update.asset(), "BTC");
        assert_eq!(update.source_id(), "polyresearch_primary");
        assert_eq!(update.provider(), REFERENCE_PRICE_PROVIDER_KEY);
        assert_eq!(update.provider_instrument(), "BTC/USD");
        assert_eq!(update.price(), 66300.25);
        assert_eq!(update.bid(), Some(66299.5));
        assert_eq!(update.ask(), Some(66301.0));
        assert_eq!(update.observed_ts_ms(), 1774672588000);
        assert_eq!(update.received_ts_ms(), 1774672589123);
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
