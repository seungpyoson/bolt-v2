//! Data-only Chainlink reference-price WebSocket provider binding.
//!
//! This provider is intentionally distinct from `CHAINLINK_DATA_STREAMS`, which
//! remains the point-in-time REST strike source that emits `IndexPriceUpdate`.
//! The reference-price provider is a custom-data source only.

use std::{
    any::Any,
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
    live::{runner::get_data_event_sender, runtime::get_runtime},
    messages::{
        DataEvent,
        data::{SubscribeCustomData, UnsubscribeCustomData},
    },
};
use nautilus_core::{Params, consts::NAUTILUS_USER_AGENT, string::secret::REDACTED};
use nautilus_model::{
    data::Data,
    identifiers::{ClientId, Venue},
};
use nautilus_network::{
    http::USER_AGENT,
    mode::ConnectionMode,
    transport::Message,
    websocket::{MessageHandler, TransportBackend, WebSocketClient, WebSocketConfig},
};
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
        chainlink::{
            ChainlinkDataStreamsReportApiResponse, ChainlinkStrikeFeedBinding,
            PriceToBeatReportBinding, chainlink_data_streams_auth_headers,
            chainlink_data_streams_credentials, decode_price_to_beat_report, parse_feed_binding,
        },
    },
    bolt_v3_reference_price::{
        REFERENCE_PRICE_ASSET_PARAM, REFERENCE_PRICE_INSTRUMENT_ID_PARAM,
        REFERENCE_PRICE_PROVIDER_PARAM, REFERENCE_PRICE_SOURCE_KEY_PARAM, ReferencePriceUpdate,
        ReferenceQuoteProvenance,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
};

pub const KEY: &str = "CHAINLINK_REFERENCE_PRICE";
pub const REFERENCE_PRICE_PROVIDER_KEY: &str = "chainlink_ws";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Chainlink reference-price source",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_parameter", "api_secret_ssm_parameter"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &[];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[];

const FACTORY_NAME: &str = KEY;
const CONFIG_TYPE: &str = "ChainlinkReferencePriceClientConfig";
const CHAINLINK_REFERENCE_FEED_IDS_QUERY_FIELD: &str = "feedIDs";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkReferencePriceDataConfig {
    pub websocket_endpoint: String,
    pub websocket_path: String,
    pub transport_backend: TransportBackend,
    pub heartbeat_secs: Option<u64>,
    pub heartbeat_message: Option<String>,
    pub reconnect_timeout_ms: u64,
    pub reconnect_delay_initial_ms: u64,
    pub reconnect_delay_max_ms: u64,
    pub reconnect_backoff_factor: f64,
    pub reconnect_jitter_ms: u64,
    pub reconnect_max_attempts: Option<u32>,
    pub idle_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkReferencePriceSecretsConfig {
    pub api_key_ssm_parameter: String,
    pub api_secret_ssm_parameter: String,
}

#[derive(Clone)]
pub struct ResolvedBoltV3ChainlinkReferenceSecrets {
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
}

impl std::fmt::Debug for ResolvedBoltV3ChainlinkReferenceSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3ChainlinkReferenceSecrets")
            .field("api_key", &REDACTED)
            .field("api_secret", &REDACTED)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3ChainlinkReferenceSecrets {
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

#[derive(Clone)]
pub struct ChainlinkReferencePriceClientConfig {
    pub websocket_endpoint: String,
    pub websocket_path: String,
    pub transport_backend: TransportBackend,
    pub heartbeat_secs: Option<u64>,
    pub heartbeat_message: Option<String>,
    pub reconnect_timeout_ms: u64,
    pub reconnect_delay_initial_ms: u64,
    pub reconnect_delay_max_ms: u64,
    pub reconnect_backoff_factor: f64,
    pub reconnect_jitter_ms: u64,
    pub reconnect_max_attempts: Option<u32>,
    pub idle_timeout_ms: u64,
    pub feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
}

impl std::fmt::Debug for ChainlinkReferencePriceClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainlinkReferencePriceClientConfig")
            .field("websocket_endpoint", &self.websocket_endpoint)
            .field("websocket_path", &self.websocket_path)
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
            .field("reconnect_max_attempts", &self.reconnect_max_attempts)
            .field("idle_timeout_ms", &self.idle_timeout_ms)
            .field("feed_bindings", &self.feed_bindings)
            .field("api_key", &REDACTED)
            .field("api_secret", &REDACTED)
            .finish()
    }
}

impl ClientConfig for ChainlinkReferencePriceClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug)]
pub struct ChainlinkReferencePriceClientFactory;

impl DataClientFactory for ChainlinkReferencePriceClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<ChainlinkReferencePriceClientConfig>()
            .ok_or_else(|| anyhow::anyhow!("Chainlink reference factory received wrong config"))?;
        Ok(Box::new(ChainlinkReferencePriceClient {
            client_id: ClientId::from(name),
            config: config.clone(),
            data_sender: get_data_event_sender(),
            subscriptions: Arc::new(Mutex::new(BTreeMap::new())),
            websocket: None,
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
struct ChainlinkReferencePriceClient {
    client_id: ClientId,
    config: ChainlinkReferencePriceClientConfig,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    subscriptions:
        Arc<Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>>,
    websocket: Option<WebSocketClient>,
    connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainlinkReferenceSubscription {
    asset: String,
    source_id: String,
    instrument_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ChainlinkReferenceSubscriptionKey {
    asset: String,
    source_id: String,
    instrument_id: String,
}

impl ChainlinkReferenceSubscriptionKey {
    fn from_subscription(subscription: &ChainlinkReferenceSubscription) -> Self {
        Self {
            asset: subscription.asset.clone(),
            source_id: subscription.source_id.clone(),
            instrument_id: subscription.instrument_id.clone(),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl DataClient for ChainlinkReferencePriceClient {
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
        self.spawn_disconnect();
        self.connected = false;
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.spawn_disconnect();
        self.connected = false;
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.spawn_disconnect();
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        chainlink_reference_transport_connected(
            self.connected,
            self.websocket
                .as_ref()
                .map(WebSocketClient::connection_mode),
        )
    }

    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.websocket.is_some() {
            self.disconnect().await?;
        }
        let feed_ids = self
            .config
            .feed_bindings
            .iter()
            .map(|binding| binding.feed_id.clone())
            .collect::<Vec<_>>();
        let (url, path_with_query) = chainlink_reference_websocket_url(
            &self.config.websocket_endpoint,
            &self.config.websocket_path,
            &feed_ids,
        )?;
        let credentials =
            chainlink_data_streams_credentials(&self.config.api_key, &self.config.api_secret)
                .map_err(|error| {
                    anyhow::anyhow!("Chainlink reference credentials invalid: {error}")
                })?;
        let authorization_timestamp_ms = current_unix_timestamp_ms()?;
        let headers = chainlink_reference_websocket_headers(chainlink_data_streams_auth_headers(
            &credentials,
            &path_with_query,
            authorization_timestamp_ms,
        ));
        let websocket_config =
            chainlink_reference_websocket_client_config(&self.config, url, headers);
        let handler = chainlink_reference_message_handler(
            self.config.feed_bindings.clone(),
            Arc::clone(&self.subscriptions),
            self.data_sender.clone(),
        );
        let websocket =
            WebSocketClient::connect(websocket_config, Some(handler), None, None, vec![], None)
                .await?;
        self.websocket = Some(websocket);
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if let Some(websocket) = self.websocket.take() {
            websocket.disconnect().await;
        }
        self.connected = false;
        Ok(())
    }

    fn subscribe(&mut self, cmd: SubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            chainlink_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        if !self
            .config
            .feed_bindings
            .iter()
            .any(|binding| binding.instrument_id.to_string() == subscription.instrument_id)
        {
            anyhow::bail!(
                "Chainlink reference subscription instrument {} is not present in the shared Chainlink feed catalog",
                subscription.instrument_id
            );
        }
        self.subscriptions
            .lock()
            .map_err(|error| {
                anyhow::anyhow!("Chainlink reference subscription state poisoned: {error}")
            })?
            .insert(
                ChainlinkReferenceSubscriptionKey::from_subscription(&subscription),
                subscription,
            );
        Ok(())
    }

    fn unsubscribe(&mut self, cmd: &UnsubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            chainlink_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        self.subscriptions
            .lock()
            .map_err(|error| {
                anyhow::anyhow!("Chainlink reference subscription state poisoned: {error}")
            })?
            .remove(&ChainlinkReferenceSubscriptionKey::from_subscription(
                &subscription,
            ));
        Ok(())
    }
}

impl ChainlinkReferencePriceClient {
    fn spawn_disconnect(&mut self) {
        if let Some(websocket) = self.websocket.take() {
            get_runtime().spawn(async move {
                websocket.disconnect().await;
            });
        }
    }
}

fn chainlink_reference_transport_connected(
    started: bool,
    transport_mode: Option<ConnectionMode>,
) -> bool {
    started && transport_mode.is_some_and(|mode| mode.is_active())
}

fn chainlink_reference_websocket_url(
    websocket_endpoint: &str,
    websocket_path: &str,
    feed_ids: &[String],
) -> anyhow::Result<(Url, String)> {
    if feed_ids.is_empty() {
        anyhow::bail!("Chainlink reference WebSocket requires at least one feed id");
    }
    let mut url = validate_chainlink_reference_websocket_endpoint(websocket_endpoint)?;
    validate_chainlink_reference_websocket_path(websocket_path).map_err(anyhow::Error::msg)?;
    url.set_path(websocket_path);
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut().append_pair(
        CHAINLINK_REFERENCE_FEED_IDS_QUERY_FIELD,
        &feed_ids.join(","),
    );
    let mut path_with_query = url.path().to_string();
    if let Some(query) = url.query() {
        path_with_query.push('?');
        path_with_query.push_str(query);
    }
    Ok((url, path_with_query))
}

fn validate_chainlink_reference_websocket_path(value: &str) -> Result<(), String> {
    if value.trim() != value
        || value == "/"
        || !value.starts_with('/')
        || value.starts_with("//")
        || value.contains('?')
        || value.contains('#')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(
            "Chainlink reference websocket_path must be a rooted credential-free path".to_string(),
        );
    }
    Ok(())
}

fn validate_chainlink_reference_websocket_endpoint(value: &str) -> anyhow::Result<Url> {
    let url = Url::parse(value).map_err(|_| {
        anyhow::anyhow!("Chainlink reference websocket_endpoint must be a valid wss URL")
    })?;
    let path = url.path();
    if value.trim() != value
        || url.scheme() != "wss"
        || !url.has_host()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !(path.is_empty() || path == "/")
    {
        anyhow::bail!(
            "Chainlink reference websocket_endpoint must be a credential-free wss origin"
        );
    }
    Ok(url)
}

fn chainlink_reference_websocket_headers(
    auth_headers: std::collections::HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut headers = auth_headers
        .into_iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .collect::<Vec<_>>();
    headers.push((USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string()));
    headers
}

pub(crate) fn chainlink_reference_websocket_client_config(
    config: &ChainlinkReferencePriceClientConfig,
    url: Url,
    headers: Vec<(String, String)>,
) -> WebSocketConfig {
    WebSocketConfig {
        url: url.to_string(),
        headers,
        heartbeat: config.heartbeat_secs,
        heartbeat_msg: config.heartbeat_message.clone(),
        reconnect_timeout_ms: Some(config.reconnect_timeout_ms),
        reconnect_delay_initial_ms: Some(config.reconnect_delay_initial_ms),
        reconnect_delay_max_ms: Some(config.reconnect_delay_max_ms),
        reconnect_backoff_factor: Some(config.reconnect_backoff_factor),
        reconnect_jitter_ms: Some(config.reconnect_jitter_ms),
        reconnect_max_attempts: config.reconnect_max_attempts,
        idle_timeout_ms: Some(config.idle_timeout_ms),
        backend: config.transport_backend,
        proxy_url: None,
    }
}

fn chainlink_reference_subscription_from_command(
    data_type: &nautilus_model::data::DataType,
    params: Option<&Params>,
) -> Result<ChainlinkReferenceSubscription, String> {
    let metadata = data_type
        .metadata()
        .ok_or_else(|| "Chainlink reference subscription missing data_type metadata".to_string())?;
    let asset = required_reference_field(metadata, REFERENCE_PRICE_ASSET_PARAM, "data_type")?;
    let source_id =
        required_reference_field(metadata, REFERENCE_PRICE_SOURCE_KEY_PARAM, "data_type")?;
    let provider = required_reference_field(metadata, REFERENCE_PRICE_PROVIDER_PARAM, "data_type")?;
    if provider != REFERENCE_PRICE_PROVIDER_KEY {
        return Err(format!(
            "Chainlink reference subscription provider must be {REFERENCE_PRICE_PROVIDER_KEY}"
        ));
    }
    let expected_data_type =
        ReferencePriceUpdate::data_type_for(asset, source_id, REFERENCE_PRICE_PROVIDER_KEY)?;
    if &expected_data_type != data_type {
        return Err(
            "Chainlink reference subscription data_type metadata is inconsistent".to_string(),
        );
    }

    let params = params
        .ok_or_else(|| "Chainlink reference subscription missing command params".to_string())?;
    require_matching_reference_param(params, REFERENCE_PRICE_ASSET_PARAM, asset)?;
    require_matching_reference_param(params, REFERENCE_PRICE_SOURCE_KEY_PARAM, source_id)?;
    require_matching_reference_param(params, REFERENCE_PRICE_PROVIDER_PARAM, provider)?;
    let instrument_id =
        required_reference_field(params, REFERENCE_PRICE_INSTRUMENT_ID_PARAM, "params")?;

    Ok(ChainlinkReferenceSubscription {
        asset: asset.to_string(),
        source_id: source_id.to_string(),
        instrument_id: instrument_id.to_string(),
    })
}

fn required_reference_field<'a>(
    params: &'a Params,
    key: &'static str,
    owner: &'static str,
) -> Result<&'a str, String> {
    params
        .get_str(key)
        .ok_or_else(|| format!("Chainlink reference subscription missing {owner}.{key}"))
}

fn require_matching_reference_param(
    params: &Params,
    key: &'static str,
    expected: &str,
) -> Result<(), String> {
    let actual = required_reference_field(params, key, "params")?;
    if actual != expected {
        return Err(format!(
            "Chainlink reference subscription params.{key} does not match data_type metadata"
        ));
    }
    Ok(())
}

fn chainlink_reference_message_handler(
    feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    subscriptions: Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
) -> MessageHandler {
    Arc::new(move |message: Message| {
        let frame_bytes = match message {
            Message::Text(bytes) | Message::Binary(bytes) => bytes,
            _ => return,
        };
        let frame = match std::str::from_utf8(frame_bytes.as_ref()) {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!("Chainlink reference frame dropped: invalid UTF-8: {error}");
                return;
            }
        };
        let received_ts_ms = match current_unix_timestamp_ms() {
            Ok(value) => value,
            Err(error) => {
                log::warn!("Chainlink reference frame dropped: {error}");
                return;
            }
        };
        let updates = match subscriptions.lock() {
            Ok(subscriptions) => chainlink_reference_updates_from_report_frame(
                &feed_bindings,
                &subscriptions,
                frame,
                received_ts_ms,
            ),
            Err(error) => Err(format!(
                "Chainlink reference subscription state poisoned: {error}"
            )),
        };
        match updates {
            Ok(updates) => {
                for update in updates {
                    if let Err(error) =
                        data_sender.send(DataEvent::Data(Data::Custom(update.to_custom_data())))
                    {
                        log::warn!("Chainlink reference update dropped: {error}");
                    }
                }
            }
            Err(error) => log::warn!("Chainlink reference frame dropped: {error}"),
        }
    })
}

fn chainlink_reference_updates_from_report_frame(
    feed_bindings: &[ChainlinkStrikeFeedBinding],
    subscriptions: &BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>,
    frame: &str,
    received_ts_ms: u64,
) -> Result<Vec<ReferencePriceUpdate>, String> {
    let envelope = serde_json::from_str::<ChainlinkDataStreamsReportApiResponse>(frame)
        .map_err(|error| format!("invalid Chainlink reference report JSON: {error}"))?;
    let binding = feed_bindings
        .iter()
        .find(|binding| binding.feed_id == envelope.report.feed_id())
        .ok_or_else(|| {
            format!(
                "Chainlink reference report feed {} is not present in the shared Chainlink feed catalog",
                envelope.report.feed_id()
            )
        })?;
    let report_bytes = serde_json::to_vec_pretty(&envelope.report)
        .map_err(|error| format!("Chainlink reference report could not serialize: {error}"))?;
    let decoded = decode_price_to_beat_report(
        &report_bytes,
        &PriceToBeatReportBinding {
            feed_id: binding.feed_id.clone(),
            schema_version: binding.report_schema_version,
            decimal_scale: binding.report_decimal_scale,
        },
    )
    .map_err(|error| format!("Chainlink reference report decode failed: {error}"))?;
    let instrument_id = binding.instrument_id.to_string();
    subscriptions
        .values()
        .filter(|subscription| subscription.instrument_id == instrument_id)
        .map(|subscription| {
            ReferencePriceUpdate::try_new_with_provenance(
                subscription.asset.as_str(),
                subscription.source_id.as_str(),
                REFERENCE_PRICE_PROVIDER_KEY,
                instrument_id.as_str(),
                decoded.benchmark_price,
                Some(decoded.bid_price),
                Some(decoded.ask_price),
                decoded.observations_timestamp_ms,
                received_ts_ms,
                ReferenceQuoteProvenance::empty(),
            )
        })
        .collect()
}

fn current_unix_timestamp_ms() -> anyhow::Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| anyhow::anyhow!("system clock before UNIX epoch: {error}"))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| anyhow::anyhow!("system clock timestamp exceeds supported range"))
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.execution.is_some() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) is data-only and must not declare an [execution] block"
        ));
    }
    if let Some(data) = &client.data {
        match data.clone().try_into::<ChainlinkReferencePriceDataConfig>() {
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
            .try_into::<ChainlinkReferencePriceSecretsConfig>()
        {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &ChainlinkReferencePriceDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    validate_wss_endpoint(
        &format!("clients.{key}.data.websocket_endpoint"),
        &data.websocket_endpoint,
        &mut errors,
    );
    if let Err(message) = validate_chainlink_reference_websocket_path(&data.websocket_path) {
        errors.push(format!("clients.{key}.data.websocket_path: {message}"));
    }
    validate_positive_optional_u64(
        &format!("clients.{key}.data.heartbeat_secs"),
        data.heartbeat_secs,
        &mut errors,
    );
    if data.heartbeat_message.is_some() {
        errors.push(format!(
            "clients.{key}.data.heartbeat_message must be omitted so Chainlink receives WebSocket protocol Ping frames instead of text messages"
        ));
    }
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
    if data.reconnect_max_attempts != Some(0) {
        errors.push(format!(
            "clients.{key}.data.reconnect_max_attempts must be explicitly set to 0 because Chainlink reference WebSocket auth headers are regenerated only on DataClient connect"
        ));
    }
    validate_positive_u64(
        &format!("clients.{key}.data.idle_timeout_ms"),
        data.idle_timeout_ms,
        &mut errors,
    );
    errors
}

fn validate_wss_endpoint(field: &str, value: &str, errors: &mut Vec<String>) {
    if validate_chainlink_reference_websocket_endpoint(value).is_err() {
        errors.push(format!("{field} must be a credential-free wss origin"));
    }
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

fn validate_secret_paths(key: &str, secrets: &ChainlinkReferencePriceSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    for (field, value) in [
        ("api_key_ssm_parameter", &secrets.api_key_ssm_parameter),
        (
            "api_secret_ssm_parameter",
            &secrets.api_secret_ssm_parameter,
        ),
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
    Ok(Arc::new(ResolvedBoltV3ChainlinkReferenceSecrets {
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
) -> Result<ChainlinkReferencePriceSecretsConfig, BoltV3SecretError> {
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
            source: format!("invalid chainlink reference secrets schema: {error}"),
        })
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(ChainlinkReferencePriceClientFactory),
                config: Box::new(map_data(context.root, context.client_key, value, secrets)?),
            })
        }
        None => None,
    };
    Ok(BoltV3ClientAdapterConfig {
        data,
        execution: None,
    })
}

pub(crate) fn reference_price_client_config(
    root: &crate::bolt_v3_config::BoltV3RootConfig,
    client_key: &str,
    client: &ClientBlock,
    resolved: &crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<ChainlinkReferencePriceClientConfig, BoltV3AdapterMappingError> {
    let value =
        client
            .data
            .as_ref()
            .ok_or_else(|| BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.to_string(),
                field: "data",
                message: format!("clients.{client_key}.data must be configured"),
            })?;
    let secrets = secrets_for(client_key, resolved)?;
    map_data(root, client_key, value, secrets)
}

pub(crate) fn reference_price_websocket_config(
    config: &ChainlinkReferencePriceClientConfig,
) -> anyhow::Result<WebSocketConfig> {
    let feed_ids = config
        .feed_bindings
        .iter()
        .map(|binding| binding.feed_id.clone())
        .collect::<Vec<_>>();
    let (url, path_with_query) = chainlink_reference_websocket_url(
        &config.websocket_endpoint,
        &config.websocket_path,
        &feed_ids,
    )?;
    let authorization_timestamp_ms = current_unix_timestamp_ms()?;
    let credentials = chainlink_data_streams_credentials(&config.api_key, &config.api_secret)?;
    let headers = chainlink_reference_websocket_headers(chainlink_data_streams_auth_headers(
        &credentials,
        &path_with_query,
        authorization_timestamp_ms,
    ));
    Ok(chainlink_reference_websocket_client_config(
        config, url, headers,
    ))
}

fn map_data(
    root: &crate::bolt_v3_config::BoltV3RootConfig,
    client_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3ChainlinkReferenceSecrets,
) -> Result<ChainlinkReferencePriceClientConfig, BoltV3AdapterMappingError> {
    let cfg: ChainlinkReferencePriceDataConfig =
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
    let feed_bindings = root
        .chainlink_data_streams
        .as_ref()
        .ok_or_else(|| BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "chainlink_data_streams.feed_bindings",
            message: format!(
                "chainlink_data_streams.feed_bindings must be configured for clients.{client_key}"
            ),
        })?
        .feed_bindings
        .as_slice();
    if feed_bindings.is_empty() {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "chainlink_data_streams.feed_bindings",
            message: format!(
                "chainlink_data_streams.feed_bindings must contain one or more resolution feed bindings for clients.{client_key}"
            ),
        });
    }
    let feed_bindings = feed_bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| parse_feed_binding(client_key, index, binding))
        .collect::<Result<Vec<ChainlinkStrikeFeedBinding>, String>>()
        .map_err(|message| BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "chainlink_data_streams.feed_bindings",
            message,
        })?;
    Ok(ChainlinkReferencePriceClientConfig {
        websocket_endpoint: cfg.websocket_endpoint,
        websocket_path: cfg.websocket_path,
        transport_backend: cfg.transport_backend,
        heartbeat_secs: cfg.heartbeat_secs,
        heartbeat_message: cfg.heartbeat_message,
        reconnect_timeout_ms: cfg.reconnect_timeout_ms,
        reconnect_delay_initial_ms: cfg.reconnect_delay_initial_ms,
        reconnect_delay_max_ms: cfg.reconnect_delay_max_ms,
        reconnect_backoff_factor: cfg.reconnect_backoff_factor,
        reconnect_jitter_ms: cfg.reconnect_jitter_ms,
        reconnect_max_attempts: cfg.reconnect_max_attempts,
        idle_timeout_ms: cfg.idle_timeout_ms,
        feed_bindings,
        api_key: secrets.api_key.clone(),
        api_secret: secrets.api_secret.clone(),
    })
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3ChainlinkReferenceSecrets, BoltV3AdapterMappingError> {
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
    use std::str::FromStr;

    use super::*;
    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::identifiers::InstrumentId;
    use rust_decimal::{Decimal, prelude::ToPrimitive};

    const TEST_FEED_ID: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const TEST_ETH_FEED_ID: &str =
        "0x000462205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const TEST_INSTRUMENT_ID: &str = "BTC-USD.CHAINLINK";
    const TEST_ETH_INSTRUMENT_ID: &str = "ETH-USD.CHAINLINK";
    const TEST_ASSET: &str = "BTC";
    const TEST_ETH_ASSET: &str = "ETH";
    const TEST_SOURCE_ID: &str = "chainlink_primary";
    const TEST_DECIMAL_SCALE: u64 = 18;
    const TEST_REPORT_SCHEMA_VERSION: u64 = 3;
    const TEST_VALID_FROM_SECONDS: u32 = 600;
    const TEST_OBSERVATIONS_SECONDS: u32 = 601;
    const TEST_BENCHMARK_PRICE: f64 = 66_300.25;
    const TEST_BID_PRICE: f64 = 66_299.50;
    const TEST_ASK_PRICE: f64 = 66_301.00;
    const TEST_PRICE_TOLERANCE: f64 = 1e-6;

    fn fixture_config() -> ChainlinkReferencePriceClientConfig {
        ChainlinkReferencePriceClientConfig {
            websocket_endpoint: "wss://ws.testnet-dataengine.chain.link".to_string(),
            websocket_path: "/api/v1/ws".to_string(),
            transport_backend: TransportBackend::Sockudo,
            heartbeat_secs: Some(5),
            heartbeat_message: None,
            reconnect_timeout_ms: 5_000,
            reconnect_delay_initial_ms: 250,
            reconnect_delay_max_ms: 5_000,
            reconnect_backoff_factor: 1.5,
            reconnect_jitter_ms: 100,
            reconnect_max_attempts: Some(0),
            idle_timeout_ms: 10_000,
            feed_bindings: vec![fixture_feed_binding()],
            api_key: Zeroizing::new("chainlink-api-key".to_string()),
            api_secret: Zeroizing::new("chainlink-api-secret".to_string()),
        }
    }

    fn fixture_root_config() -> crate::bolt_v3_config::BoltV3RootConfig {
        toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture root config should parse")
    }

    fn fixture_resolved_secrets() -> ResolvedBoltV3ChainlinkReferenceSecrets {
        ResolvedBoltV3ChainlinkReferenceSecrets {
            api_key: Zeroizing::new("chainlink-api-key".to_string()),
            api_secret: Zeroizing::new("chainlink-api-secret".to_string()),
        }
    }

    fn fixture_feed_binding() -> ChainlinkStrikeFeedBinding {
        ChainlinkStrikeFeedBinding {
            feed_id: TEST_FEED_ID.to_string(),
            instrument_id: InstrumentId::from_str(TEST_INSTRUMENT_ID)
                .expect("test Chainlink instrument id should parse"),
            report_schema_version: TEST_REPORT_SCHEMA_VERSION,
            report_decimal_scale: TEST_DECIMAL_SCALE,
            price_precision: 8,
        }
    }

    fn fixture_eth_feed_binding() -> ChainlinkStrikeFeedBinding {
        ChainlinkStrikeFeedBinding {
            feed_id: TEST_ETH_FEED_ID.to_string(),
            instrument_id: InstrumentId::from_str(TEST_ETH_INSTRUMENT_ID)
                .expect("test Chainlink ETH instrument id should parse"),
            report_schema_version: TEST_REPORT_SCHEMA_VERSION,
            report_decimal_scale: TEST_DECIMAL_SCALE,
            price_precision: 8,
        }
    }

    fn fixture_client() -> (
        ChainlinkReferencePriceClient,
        tokio::sync::mpsc::UnboundedReceiver<DataEvent>,
    ) {
        fixture_client_with_bindings(vec![fixture_feed_binding()])
    }

    fn fixture_client_with_bindings(
        feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    ) -> (
        ChainlinkReferencePriceClient,
        tokio::sync::mpsc::UnboundedReceiver<DataEvent>,
    ) {
        let (data_sender, data_receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut config = fixture_config();
        config.feed_bindings = feed_bindings;
        (
            ChainlinkReferencePriceClient {
                client_id: ClientId::from("chainlink_reference"),
                config,
                data_sender,
                subscriptions: Arc::new(Mutex::new(BTreeMap::new())),
                websocket: None,
                connected: false,
            },
            data_receiver,
        )
    }

    #[test]
    fn map_data_rejects_empty_root_feed_bindings() {
        let mut root = fixture_root_config();
        root.chainlink_data_streams
            .as_mut()
            .expect("fixture root should have Chainlink feed bindings")
            .feed_bindings
            .clear();
        let data = root
            .clients
            .get("chainlink_reference")
            .and_then(|client| client.data.as_ref())
            .expect("fixture Chainlink reference client should have data");
        let secrets = fixture_resolved_secrets();

        let error = map_data(&root, "chainlink_reference", data, &secrets)
            .expect_err("empty root feed bindings should be rejected by adapter mapping");

        assert!(
            error
                .to_string()
                .contains("chainlink_data_streams.feed_bindings must contain one or more"),
            "unexpected empty feed binding error: {error}"
        );
    }

    #[test]
    fn websocket_url_uses_configured_chainlink_ws_path_and_shared_feed_ids() {
        let (url, path_with_query) = chainlink_reference_websocket_url(
            "wss://ws.testnet-dataengine.chain.link",
            "/custom/ws",
            &[TEST_FEED_ID.to_string()],
        )
        .expect("valid Chainlink WS origin and feed id should build");

        assert_eq!(
            url.as_str(),
            format!("wss://ws.testnet-dataengine.chain.link/custom/ws?feedIDs={TEST_FEED_ID}")
        );
        assert_eq!(
            path_with_query,
            format!("/custom/ws?feedIDs={TEST_FEED_ID}")
        );
    }

    #[test]
    fn websocket_config_carries_hmac_auth_headers_and_user_agent() {
        let (_url, path_with_query) = chainlink_reference_websocket_url(
            "wss://ws.testnet-dataengine.chain.link",
            "/api/v1/ws",
            &[TEST_FEED_ID.to_string()],
        )
        .expect("valid Chainlink WS origin and feed id should build");
        let credentials =
            chainlink_data_streams_credentials("chainlink-api-key", "chainlink-secret")
                .expect("test Chainlink credentials should validate");
        let headers = chainlink_reference_websocket_headers(chainlink_data_streams_auth_headers(
            &credentials,
            &path_with_query,
            1_700_000_000_000,
        ));
        let user_agent_header = USER_AGENT.to_string();

        assert!(
            headers
                .iter()
                .any(|(key, value)| key == &user_agent_header
                    && value.as_str() == NAUTILUS_USER_AGENT)
        );
        assert!(
            headers
                .iter()
                .any(|(key, value)| key == "Authorization" && value == "chainlink-api-key")
        );
        assert!(
            headers
                .iter()
                .any(|(key, value)| key == "X-Authorization-Timestamp" && value == "1700000000000")
        );
        assert!(
            headers
                .iter()
                .any(|(key, value)| key == "X-Authorization-Signature-SHA256" && value.len() == 64)
        );
    }

    #[test]
    fn start_stop_update_nt_data_client_connected_state_without_network() {
        let (mut client, _data_receiver) = fixture_client();

        assert!(client.is_disconnected());

        client
            .start()
            .expect("chainlink reference start should succeed");
        assert!(
            client.is_disconnected(),
            "start without a WebSocket transport must not mask a later connect failure"
        );

        client
            .stop()
            .expect("chainlink reference stop should succeed");
        assert!(client.is_disconnected());
    }

    #[test]
    fn transport_closed_state_reports_data_client_disconnected() {
        assert!(
            !chainlink_reference_transport_connected(true, Some(ConnectionMode::Closed)),
            "closed Chainlink transport must fail closed instead of reporting stale connected state"
        );
        assert!(
            !chainlink_reference_transport_connected(true, Some(ConnectionMode::Reconnect)),
            "reconnecting Chainlink transport has not minted fresh auth headers and must not report connected"
        );
        assert!(
            !chainlink_reference_transport_connected(true, None),
            "missing Chainlink transport state must fail closed instead of reporting connected"
        );
        assert!(chainlink_reference_transport_connected(
            true,
            Some(ConnectionMode::Active)
        ));
    }

    #[test]
    fn subscribe_custom_data_records_catalog_backed_chainlink_reference_subscription() {
        let (mut client, _data_receiver) = fixture_client();

        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                TEST_INSTRUMENT_ID,
            ))
            .expect("catalog-backed Chainlink reference subscription should be accepted");

        let subscriptions = client
            .subscriptions
            .lock()
            .expect("subscription state should be available");
        let key = ChainlinkReferenceSubscriptionKey {
            asset: TEST_ASSET.to_string(),
            source_id: TEST_SOURCE_ID.to_string(),
            instrument_id: TEST_INSTRUMENT_ID.to_string(),
        };
        assert_eq!(
            subscriptions.get(&key),
            Some(&ChainlinkReferenceSubscription {
                asset: TEST_ASSET.to_string(),
                source_id: TEST_SOURCE_ID.to_string(),
                instrument_id: TEST_INSTRUMENT_ID.to_string(),
            })
        );
    }

    #[test]
    fn same_source_id_subscriptions_remain_asset_scoped() {
        let (mut client, _data_receiver) =
            fixture_client_with_bindings(vec![fixture_feed_binding(), fixture_eth_feed_binding()]);

        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                TEST_INSTRUMENT_ID,
            ))
            .expect("BTC Chainlink reference subscription should be accepted");
        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ETH_ASSET,
                TEST_SOURCE_ID,
                TEST_ETH_INSTRUMENT_ID,
            ))
            .expect("ETH Chainlink reference subscription should be accepted");

        let subscriptions = client
            .subscriptions
            .lock()
            .expect("subscription state should be available");
        assert_eq!(
            subscriptions.len(),
            2,
            "same source_id across assets must not overwrite active WebSocket subscriptions"
        );
        assert!(
            subscriptions.contains_key(&ChainlinkReferenceSubscriptionKey {
                asset: TEST_ASSET.to_string(),
                source_id: TEST_SOURCE_ID.to_string(),
                instrument_id: TEST_INSTRUMENT_ID.to_string(),
            })
        );
        assert!(
            subscriptions.contains_key(&ChainlinkReferenceSubscriptionKey {
                asset: TEST_ETH_ASSET.to_string(),
                source_id: TEST_SOURCE_ID.to_string(),
                instrument_id: TEST_ETH_INSTRUMENT_ID.to_string(),
            })
        );
    }

    #[test]
    fn report_frame_for_active_subscription_emits_custom_reference_update() {
        let (mut client, mut data_receiver) = fixture_client();
        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                TEST_INSTRUMENT_ID,
            ))
            .expect("catalog-backed Chainlink reference subscription should be accepted");
        let handler = chainlink_reference_message_handler(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
        );

        handler(Message::text(chainlink_report_frame_json()));

        let event = data_receiver
            .try_recv()
            .expect("matched Chainlink report frame should emit one data event");
        let DataEvent::Data(Data::Custom(custom)) = event else {
            panic!("matched Chainlink report frame should emit custom data, got {event:?}");
        };
        let update = ReferencePriceUpdate::from_custom_data(&custom)
            .expect("custom data should contain a reference price update");
        assert_eq!(update.asset(), TEST_ASSET);
        assert_eq!(update.source_id(), TEST_SOURCE_ID);
        assert_eq!(update.provider(), REFERENCE_PRICE_PROVIDER_KEY);
        assert_eq!(update.provider_instrument(), TEST_INSTRUMENT_ID);
        assert!(
            (update.price() - TEST_BENCHMARK_PRICE).abs() < TEST_PRICE_TOLERANCE,
            "benchmark price should round-trip, got {}",
            update.price()
        );
        let quote = update
            .to_reference_quote()
            .expect("Chainlink update should convert to a reference quote");
        assert!(
            (quote.bid().expect("Chainlink quote should carry bid") - TEST_BID_PRICE).abs()
                < TEST_PRICE_TOLERANCE,
            "bid price should round-trip, got {:?}",
            quote.bid()
        );
        assert!(
            (quote.ask().expect("Chainlink quote should carry ask") - TEST_ASK_PRICE).abs()
                < TEST_PRICE_TOLERANCE,
            "ask price should round-trip, got {:?}",
            quote.ask()
        );
        assert_eq!(
            update.observed_ts_ms(),
            u64::from(TEST_OBSERVATIONS_SECONDS) * 1_000
        );
    }

    fn reference_price_subscribe_cmd(
        asset: &str,
        source_id: &str,
        instrument_id: &str,
    ) -> SubscribeCustomData {
        let mut params = Params::new();
        params.insert(
            REFERENCE_PRICE_ASSET_PARAM.to_string(),
            serde_json::json!(asset),
        );
        params.insert(
            REFERENCE_PRICE_SOURCE_KEY_PARAM.to_string(),
            serde_json::json!(source_id),
        );
        params.insert(
            REFERENCE_PRICE_PROVIDER_PARAM.to_string(),
            serde_json::json!(REFERENCE_PRICE_PROVIDER_KEY),
        );
        params.insert(
            REFERENCE_PRICE_INSTRUMENT_ID_PARAM.to_string(),
            serde_json::json!(instrument_id),
        );

        SubscribeCustomData::new(
            Some(ClientId::from("chainlink_reference")),
            None,
            ReferencePriceUpdate::data_type_for(asset, source_id, REFERENCE_PRICE_PROVIDER_KEY)
                .expect("reference price data type should build"),
            UUID4::new(),
            UnixNanos::default(),
            None,
            Some(params),
        )
    }

    fn chainlink_report_frame_json() -> String {
        let report_source = report_source_json(
            TEST_FEED_ID,
            TEST_VALID_FROM_SECONDS,
            TEST_OBSERVATIONS_SECONDS,
            abi_i192_word(scaled_price(TEST_BENCHMARK_PRICE, TEST_DECIMAL_SCALE)),
            abi_i192_word(scaled_price(TEST_BID_PRICE, TEST_DECIMAL_SCALE)),
            abi_i192_word(scaled_price(TEST_ASK_PRICE, TEST_DECIMAL_SCALE)),
        );
        let report_source: serde_json::Value =
            serde_json::from_slice(&report_source).expect("report source should parse as JSON");
        serde_json::json!({ "report": report_source }).to_string()
    }

    fn report_source_json(
        feed_id: &str,
        valid_from_seconds: u32,
        observations_seconds: u32,
        benchmark_word: [u8; 32],
        bid_word: [u8; 32],
        ask_word: [u8; 32],
    ) -> Vec<u8> {
        serde_json::to_vec_pretty(&serde_json::json!({
            "feedID": feed_id,
            "validFromTimestamp": valid_from_seconds,
            "observationsTimestamp": observations_seconds,
            "fullReport": format!(
                "0x{}",
                hex::encode(full_report_payload(
                    feed_id,
                    valid_from_seconds,
                    observations_seconds,
                    benchmark_word,
                    bid_word,
                    ask_word,
                ))
            ),
        }))
        .expect("report source JSON should serialize")
    }

    fn full_report_payload(
        feed_id: &str,
        valid_from_seconds: u32,
        observations_seconds: u32,
        benchmark_word: [u8; 32],
        bid_word: [u8; 32],
        ask_word: [u8; 32],
    ) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(&feed_id_bytes(feed_id));
        blob.extend_from_slice(&abi_u32_word(valid_from_seconds));
        blob.extend_from_slice(&abi_u32_word(observations_seconds));
        blob.extend_from_slice(&abi_zero_word());
        blob.extend_from_slice(&abi_zero_word());
        blob.extend_from_slice(&abi_u32_word(observations_seconds + 60));
        blob.extend_from_slice(&benchmark_word);
        blob.extend_from_slice(&bid_word);
        blob.extend_from_slice(&ask_word);

        let mut payload = Vec::new();
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_usize_word(128));
        payload.extend_from_slice(&abi_usize_word(blob.len()));
        payload.extend_from_slice(&blob);
        payload
    }

    fn abi_zero_word() -> [u8; 32] {
        [0_u8; 32]
    }

    fn abi_u32_word(value: u32) -> [u8; 32] {
        let mut word = [0_u8; 32];
        word[28..32].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn abi_usize_word(value: usize) -> [u8; 32] {
        let mut word = [0_u8; 32];
        word[24..32].copy_from_slice(&(value as u64).to_be_bytes());
        word
    }

    fn abi_i192_word(value: i128) -> [u8; 32] {
        let mut word = if value < 0 { [0xff_u8; 32] } else { [0_u8; 32] };
        word[16..32].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn feed_id_bytes(feed_id: &str) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        let decoded = hex::decode(feed_id.strip_prefix("0x").expect("feed id should have 0x"))
            .expect("feed id should decode");
        bytes.copy_from_slice(&decoded);
        bytes
    }

    fn scaled_price(price: f64, decimal_scale: u64) -> i128 {
        let scale = 10_i128
            .checked_pow(u32::try_from(decimal_scale).expect("scale should fit u32"))
            .expect("scale should fit i128");
        let price = Decimal::from_str_exact(&price.to_string()).expect("price should be decimal");
        (price * Decimal::from(scale))
            .round()
            .to_i128()
            .expect("scaled price should fit i128")
    }
}
