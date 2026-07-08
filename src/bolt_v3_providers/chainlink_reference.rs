//! Data-only Chainlink reference-price WebSocket provider binding.
//!
//! This provider is intentionally distinct from `CHAINLINK_DATA_STREAMS`, which
//! remains the point-in-time REST strike source that emits `IndexPriceUpdate`.
//! The reference-price provider is a custom-data source only.

use std::{
    any::Any,
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
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
use nautilus_network::{http::USER_AGENT, mode::ConnectionMode};
use serde::Deserialize;
use tokio::task::JoinHandle;
use url::Url;
use zeroize::Zeroizing;

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterConfigs, BoltV3AdapterMappingError, BoltV3ClientAdapterConfig,
        BoltV3DataClientAdapterConfig,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_operator_health::{
        BoltV3InputHealthSourceTransition, BoltV3InputHealthTransitionEmitter,
        BoltV3MissingInputSource,
    },
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
    bolt_v3_wire_boundary::{
        self, BoundaryWebSocket, TransportBackend, WebSocketConfig, WireMessage, WireMessageHandler,
    },
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
const CHAINLINK_REFERENCE_TRANSPORT_RECONNECT_MAX_ATTEMPTS: u32 = 0;
const CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE: &str =
    "chainlink reference stream stale before provider reconnect";
const CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_EXHAUSTED: &str =
    "chainlink reference provider reconnect attempts exhausted";
const CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED: &str =
    "chainlink reference stream recovered after provider reconnect";
const CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_SUBSCRIPTION_UNAVAILABLE: &str =
    "chainlink reference subscription state unavailable during liveness transition";

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
    pub reconnect_max_attempts: ChainlinkReferenceReconnectMaxAttempts,
    pub idle_timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainlinkReferenceReconnectMaxAttempts {
    Unlimited,
    Limited(u32),
}

impl ChainlinkReferenceReconnectMaxAttempts {
    fn permits_attempt(self, attempted_reconnects: u32) -> bool {
        match self {
            Self::Unlimited => true,
            Self::Limited(max_attempts) => attempted_reconnects < max_attempts,
        }
    }
}

impl<'de> Deserialize<'de> for ChainlinkReferenceReconnectMaxAttempts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Text(String),
            Number(u32),
        }

        match Wire::deserialize(deserializer)? {
            Wire::Text(value) if value == "unlimited" => Ok(Self::Unlimited),
            Wire::Text(value) => Err(serde::de::Error::custom(format!(
                "expected \"unlimited\" or a positive integer, got {value:?}"
            ))),
            Wire::Number(max_attempts) => Ok(Self::Limited(max_attempts)),
        }
    }
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
    pub reconnect_max_attempts: ChainlinkReferenceReconnectMaxAttempts,
    pub idle_timeout_ms: u64,
    pub feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    pub input_health_transition_emitter: Option<BoltV3InputHealthTransitionEmitter>,
    pub input_health_sources: Vec<BoltV3MissingInputSource>,
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
            .field(
                "input_health_transition_emitter_configured",
                &self.input_health_transition_emitter.is_some(),
            )
            .field("input_health_sources", &self.input_health_sources)
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
            websocket: Arc::new(Mutex::new(None)),
            last_report_unix_ms: Arc::new(AtomicU64::new(0)),
            input_health_report_liveness: Arc::new(Mutex::new(BTreeMap::new())),
            input_health_missing_sources: Arc::new(Mutex::new(BTreeSet::new())),
            liveness_task: None,
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
    websocket: Arc<Mutex<Option<BoundaryWebSocket>>>,
    last_report_unix_ms: Arc<AtomicU64>,
    input_health_report_liveness: ChainlinkReferenceInputHealthReportLiveness,
    input_health_missing_sources: ChainlinkReferenceInputHealthMissingSources,
    liveness_task: Option<JoinHandle<()>>,
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

type ChainlinkReferenceInputHealthMissingSources =
    Arc<Mutex<BTreeSet<ChainlinkReferenceInputHealthSourceKey>>>;
type ChainlinkReferenceInputHealthReportLiveness =
    Arc<Mutex<BTreeMap<ChainlinkReferenceInputHealthSourceKey, u64>>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ChainlinkReferenceInputHealthSourceKey {
    strategy_instance_id: String,
    source_id: String,
    asset: String,
    provider: String,
    provider_instrument: String,
}

impl ChainlinkReferenceInputHealthSourceKey {
    fn from_source(source: &BoltV3MissingInputSource) -> Self {
        Self {
            strategy_instance_id: source.strategy_instance_id.clone(),
            source_id: source.source_id.clone(),
            asset: source.asset.clone(),
            provider: source.provider.clone(),
            provider_instrument: source.provider_instrument.clone(),
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
            chainlink_reference_current_transport_mode(&self.websocket),
        )
    }

    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if let Some(task) = self.liveness_task.take() {
            task.abort();
        }
        if chainlink_reference_current_transport_mode(&self.websocket).is_some() {
            self.disconnect().await?;
        }
        let connection_epoch_ms = current_unix_timestamp_ms()?;
        let websocket = chainlink_reference_connect_websocket(
            &self.config,
            Arc::clone(&self.subscriptions),
            self.data_sender.clone(),
            Arc::clone(&self.last_report_unix_ms),
            Arc::clone(&self.input_health_report_liveness),
            Arc::clone(&self.input_health_missing_sources),
        )
        .await?;
        chainlink_reference_store_transport(&self.websocket, websocket)?;
        self.liveness_task = Some(spawn_chainlink_reference_liveness_supervisor(
            ChainlinkReferenceLivenessSupervisorContext {
                client_id: self.client_id,
                config: self.config.clone(),
                subscriptions: Arc::clone(&self.subscriptions),
                websocket: Arc::clone(&self.websocket),
                data_sender: self.data_sender.clone(),
                last_report_unix_ms: Arc::clone(&self.last_report_unix_ms),
                input_health_report_liveness: Arc::clone(&self.input_health_report_liveness),
                input_health_missing_sources: Arc::clone(&self.input_health_missing_sources),
                connection_epoch_ms,
            },
        ));
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if let Some(task) = self.liveness_task.take() {
            task.abort();
        }
        if let Some(websocket) = chainlink_reference_take_transport(&self.websocket)? {
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
                subscription.clone(),
            );
        match current_unix_timestamp_ms() {
            Ok(now_ms) => chainlink_reference_seed_input_health_report_liveness_for_subscription(
                &self.config,
                &self.input_health_report_liveness,
                &subscription,
                now_ms,
            ),
            Err(error) => {
                log::warn!("Chainlink reference subscription liveness seed skipped: {error}");
            }
        }
        Ok(())
    }

    fn unsubscribe(&mut self, cmd: &UnsubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            chainlink_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        chainlink_reference_remove_input_health_report_liveness_for_subscription(
            &self.config,
            &self.input_health_report_liveness,
            &subscription,
        );
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
        if let Some(task) = self.liveness_task.take() {
            task.abort();
        }
        let websocket = match chainlink_reference_take_transport(&self.websocket) {
            Ok(websocket) => websocket,
            Err(error) => {
                log::warn!("Chainlink reference disconnect dropped: {error}");
                None
            }
        };
        if let Some(websocket) = websocket {
            get_runtime().spawn(async move {
                websocket.disconnect().await;
            });
        }
    }
}

fn chainlink_reference_current_transport_mode(
    websocket: &Arc<Mutex<Option<BoundaryWebSocket>>>,
) -> Option<ConnectionMode> {
    websocket
        .lock()
        .ok()
        .and_then(|websocket| websocket.as_ref().map(BoundaryWebSocket::connection_mode))
}

fn chainlink_reference_take_transport(
    websocket: &Arc<Mutex<Option<BoundaryWebSocket>>>,
) -> anyhow::Result<Option<BoundaryWebSocket>> {
    websocket
        .lock()
        .map(|mut websocket| websocket.take())
        .map_err(|error| anyhow::anyhow!("Chainlink reference transport state poisoned: {error}"))
}

fn chainlink_reference_store_transport(
    websocket: &Arc<Mutex<Option<BoundaryWebSocket>>>,
    next: BoundaryWebSocket,
) -> anyhow::Result<()> {
    let mut websocket = websocket.lock().map_err(|error| {
        anyhow::anyhow!("Chainlink reference transport state poisoned: {error}")
    })?;
    *websocket = Some(next);
    Ok(())
}

fn chainlink_reference_transport_connected(
    started: bool,
    transport_mode: Option<ConnectionMode>,
) -> bool {
    started && transport_mode.is_some_and(|mode| mode.is_active())
}

#[derive(Debug)]
struct ChainlinkReferenceLivenessSupervisorState {
    attempted_reconnects: u32,
    last_budget_reset_report_ms: u64,
    source_reconnect_attempted: BTreeSet<ChainlinkReferenceInputHealthSourceKey>,
}

impl ChainlinkReferenceLivenessSupervisorState {
    fn new() -> Self {
        Self {
            attempted_reconnects: u32::MIN,
            last_budget_reset_report_ms: u64::MIN,
            source_reconnect_attempted: BTreeSet::new(),
        }
    }
}

struct ChainlinkReferenceLivenessTickContext<'a> {
    config: &'a ChainlinkReferencePriceClientConfig,
    subscriptions:
        &'a Arc<Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>>,
    input_health_report_liveness: &'a ChainlinkReferenceInputHealthReportLiveness,
    input_health_missing_sources: &'a ChainlinkReferenceInputHealthMissingSources,
    last_report_unix_ms: &'a AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChainlinkReferenceLivenessTickOutcome {
    reconnect: bool,
    exhausted: bool,
    silence_ms: u64,
    stream_stale: bool,
    source_stale: bool,
    source_reconnect: bool,
    transport_dead: bool,
}

fn chainlink_reference_liveness_supervisor_tick(
    context: ChainlinkReferenceLivenessTickContext<'_>,
    connection_epoch_ms: u64,
    state: &mut ChainlinkReferenceLivenessSupervisorState,
    now_ms: u64,
    mode: Option<ConnectionMode>,
) -> ChainlinkReferenceLivenessTickOutcome {
    let config = context.config;
    let last_report_ms = context.last_report_unix_ms.load(Ordering::SeqCst);
    if last_report_ms > connection_epoch_ms && last_report_ms > state.last_budget_reset_report_ms {
        state.attempted_reconnects = 0;
        state.last_budget_reset_report_ms = last_report_ms;
    }

    let stream_liveness_ms = last_report_ms.max(connection_epoch_ms);
    let silence_ms = now_ms.saturating_sub(stream_liveness_ms);
    let stream_stale = silence_ms > config.idle_timeout_ms;
    let stale_sources = chainlink_reference_stale_input_health_sources(
        config,
        context.subscriptions,
        context.input_health_report_liveness,
        now_ms,
        config.idle_timeout_ms,
        CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE,
    );
    let stale_source_keys = stale_sources
        .iter()
        .map(ChainlinkReferenceInputHealthSourceKey::from_source)
        .collect::<BTreeSet<_>>();
    state
        .source_reconnect_attempted
        .retain(|key| stale_source_keys.contains(key));
    let source_stale = !stale_source_keys.is_empty();
    let source_reconnect = stale_source_keys
        .iter()
        .any(|key| !state.source_reconnect_attempted.contains(key));
    let transport_dead = !chainlink_reference_transport_connected(true, mode);

    if source_stale {
        chainlink_reference_emit_missing_input_health_sources(
            config,
            stale_sources,
            context.input_health_missing_sources,
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE,
            false,
        );
    }
    if stream_stale || transport_dead {
        chainlink_reference_emit_missing_input_health_transition(
            config,
            context.subscriptions,
            context.input_health_missing_sources,
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE,
            false,
        );
    }

    let reconnect = stream_stale || source_reconnect || transport_dead;
    if !reconnect {
        return ChainlinkReferenceLivenessTickOutcome {
            reconnect: false,
            exhausted: false,
            silence_ms,
            stream_stale,
            source_stale,
            source_reconnect,
            transport_dead,
        };
    }

    if !config
        .reconnect_max_attempts
        .permits_attempt(state.attempted_reconnects)
    {
        let exhausted_sources = chainlink_reference_stale_input_health_sources(
            config,
            context.subscriptions,
            context.input_health_report_liveness,
            now_ms,
            config.idle_timeout_ms,
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_EXHAUSTED,
        );
        if !exhausted_sources.is_empty() {
            chainlink_reference_emit_missing_input_health_sources(
                config,
                exhausted_sources,
                context.input_health_missing_sources,
                CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_EXHAUSTED,
                true,
            );
        }
        if stream_stale || transport_dead {
            chainlink_reference_emit_missing_input_health_transition(
                config,
                context.subscriptions,
                context.input_health_missing_sources,
                CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_EXHAUSTED,
                true,
            );
        }
        return ChainlinkReferenceLivenessTickOutcome {
            reconnect: false,
            exhausted: true,
            silence_ms,
            stream_stale,
            source_stale,
            source_reconnect,
            transport_dead,
        };
    }

    state.attempted_reconnects = state.attempted_reconnects.saturating_add(1);
    if source_reconnect {
        state.source_reconnect_attempted.extend(stale_source_keys);
    }
    ChainlinkReferenceLivenessTickOutcome {
        reconnect: true,
        exhausted: false,
        silence_ms,
        stream_stale,
        source_stale,
        source_reconnect,
        transport_dead,
    }
}

async fn chainlink_reference_connect_websocket(
    config: &ChainlinkReferencePriceClientConfig,
    subscriptions: Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    last_report_unix_ms: Arc<AtomicU64>,
    input_health_report_liveness: ChainlinkReferenceInputHealthReportLiveness,
    input_health_missing_sources: ChainlinkReferenceInputHealthMissingSources,
) -> anyhow::Result<BoundaryWebSocket> {
    let websocket_config = reference_price_websocket_config(config)?;
    let handler = chainlink_reference_message_handler_with_input_health_recovery(
        config.feed_bindings.clone(),
        subscriptions,
        data_sender,
        last_report_unix_ms,
        Some(ChainlinkReferenceInputHealthRecovery {
            config: config.clone(),
            input_health_report_liveness,
            input_health_missing_sources,
        }),
    );
    bolt_v3_wire_boundary::connect_websocket(
        websocket_config,
        Some(handler),
        None,
        None,
        vec![],
        None,
    )
    .await
    .map_err(anyhow::Error::from)
}

struct ChainlinkReferenceLivenessSupervisorContext {
    client_id: ClientId,
    config: ChainlinkReferencePriceClientConfig,
    subscriptions:
        Arc<Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>>,
    websocket: Arc<Mutex<Option<BoundaryWebSocket>>>,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    last_report_unix_ms: Arc<AtomicU64>,
    input_health_report_liveness: ChainlinkReferenceInputHealthReportLiveness,
    input_health_missing_sources: ChainlinkReferenceInputHealthMissingSources,
    connection_epoch_ms: u64,
}

fn spawn_chainlink_reference_liveness_supervisor(
    context: ChainlinkReferenceLivenessSupervisorContext,
) -> JoinHandle<()> {
    get_runtime().spawn(async move {
        let ChainlinkReferenceLivenessSupervisorContext {
            client_id,
            config,
            subscriptions,
            websocket,
            data_sender,
            last_report_unix_ms,
            input_health_report_liveness,
            input_health_missing_sources,
            mut connection_epoch_ms,
        } = context;
        let mut supervisor_state = ChainlinkReferenceLivenessSupervisorState::new();
        let mut last_logged_mode = chainlink_reference_current_transport_mode(&websocket);
        loop {
            tokio::time::sleep(Duration::from_millis(config.idle_timeout_ms)).await;
            let now_ms = match current_unix_timestamp_ms() {
                Ok(value) => value,
                Err(error) => {
                    log::error!(
                        "Chainlink reference liveness check skipped for client_id={client_id}: {error}"
                    );
                    continue;
                }
            };
            let mode = chainlink_reference_current_transport_mode(&websocket);
            if mode != last_logged_mode {
                log::warn!(
                    "Chainlink reference transport state transition for client_id={client_id}: {:?} -> {:?}",
                    last_logged_mode,
                    mode
                );
                last_logged_mode = mode;
            }
            let tick = chainlink_reference_liveness_supervisor_tick(
                ChainlinkReferenceLivenessTickContext {
                    config: &config,
                    subscriptions: &subscriptions,
                    input_health_report_liveness: &input_health_report_liveness,
                    input_health_missing_sources: &input_health_missing_sources,
                    last_report_unix_ms: last_report_unix_ms.as_ref(),
                },
                connection_epoch_ms,
                &mut supervisor_state,
                now_ms,
                mode,
            );
            if tick.exhausted {
                log::error!(
                    "Chainlink reference reconnect attempts exhausted for client_id={client_id}; transport remains unhealthy"
                );
                break;
            }
            if !tick.reconnect {
                continue;
            }
            log::error!(
                "Chainlink reference liveness unhealthy for client_id={client_id}: silence_ms={} idle_timeout_ms={} transport_mode={:?} stream_stale={} source_stale={} source_reconnect={} transport_dead={}; reconnecting with fresh Data Streams auth headers",
                tick.silence_ms,
                config.idle_timeout_ms,
                mode,
                tick.stream_stale,
                tick.source_stale,
                tick.source_reconnect,
                tick.transport_dead
            );
            match chainlink_reference_take_transport(&websocket) {
                Ok(Some(existing)) => existing.disconnect().await,
                Ok(None) => {}
                Err(error) => {
                    log::error!(
                        "Chainlink reference reconnect could not take old transport for client_id={client_id}: {error}"
                    );
                    continue;
                }
            }
            let next_connection_epoch_ms = match current_unix_timestamp_ms() {
                Ok(value) => value,
                Err(error) => {
                    log::error!(
                        "Chainlink reference reconnect could not read connection epoch for client_id={client_id}: {error}"
                    );
                    now_ms
                }
            };
            match chainlink_reference_connect_websocket(
                &config,
                Arc::clone(&subscriptions),
                data_sender.clone(),
                Arc::clone(&last_report_unix_ms),
                Arc::clone(&input_health_report_liveness),
                Arc::clone(&input_health_missing_sources),
            )
            .await
            {
                Ok(next) => {
                    let mode = next.connection_mode();
                    if let Err(error) = chainlink_reference_store_transport(&websocket, next) {
                        log::error!(
                            "Chainlink reference reconnect could not store new transport for client_id={client_id}: {error}"
                        );
                        continue;
                    }
                    connection_epoch_ms = next_connection_epoch_ms;
                    let subscription_count = chainlink_reference_replayed_subscription_count(
                        &subscriptions,
                        client_id,
                    );
                    log::warn!(
                        "Chainlink reference DataClient reconnect completed for client_id={client_id}; transport_mode={mode:?} replayed_subscription_count={subscription_count:?}"
                    );
                    last_logged_mode = Some(mode);
                }
                Err(error) => {
                    log::error!(
                        "Chainlink reference DataClient reconnect failed for client_id={client_id}: {error}"
                    );
                }
            }
        }
    })
}

fn chainlink_reference_replayed_subscription_count(
    subscriptions: &Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    client_id: ClientId,
) -> Option<usize> {
    match subscriptions.lock() {
        Ok(subscriptions) => Some(subscriptions.len()),
        Err(error) => {
            log::error!(
                "Chainlink reference reconnect could not read subscription count for client_id={client_id}: {error}"
            );
            None
        }
    }
}

fn chainlink_reference_emit_missing_input_health_transition(
    config: &ChainlinkReferencePriceClientConfig,
    subscriptions: &Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    missing_sources: &ChainlinkReferenceInputHealthMissingSources,
    reason: &'static str,
    emit_existing: bool,
) {
    let sources =
        chainlink_reference_input_health_sources_for_transition(config, subscriptions, reason);
    chainlink_reference_emit_missing_input_health_sources(
        config,
        sources,
        missing_sources,
        reason,
        emit_existing,
    );
}

fn chainlink_reference_emit_missing_input_health_sources(
    config: &ChainlinkReferencePriceClientConfig,
    sources: Vec<BoltV3MissingInputSource>,
    missing_sources: &ChainlinkReferenceInputHealthMissingSources,
    reason: &'static str,
    emit_existing: bool,
) {
    let Some(emitter) = config.input_health_transition_emitter.as_ref() else {
        return;
    };
    let mut missing_sources = match missing_sources.lock() {
        Ok(missing_sources) => missing_sources,
        Err(error) => {
            log::error!("Chainlink reference input-health missing-source state poisoned: {error}");
            return;
        }
    };
    for source in sources {
        let key = ChainlinkReferenceInputHealthSourceKey::from_source(&source);
        let inserted = missing_sources.insert(key);
        if inserted || emit_existing {
            emitter(
                reason,
                BoltV3InputHealthSourceTransition {
                    source,
                    missing: true,
                },
            );
        }
    }
}

fn chainlink_reference_input_health_sources_for_transition(
    config: &ChainlinkReferencePriceClientConfig,
    subscriptions: &Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    reason: &'static str,
) -> Vec<BoltV3MissingInputSource> {
    if config.input_health_sources.is_empty() {
        return Vec::new();
    }
    let active_subscriptions = match subscriptions.lock() {
        Ok(subscriptions) => subscriptions.values().cloned().collect::<Vec<_>>(),
        Err(error) => {
            log::error!(
                "Chainlink reference input-health transition could not read subscriptions: {error}"
            );
            return config
                .input_health_sources
                .iter()
                .map(|source| {
                    chainlink_reference_input_health_source_with_reason(
                        source,
                        CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_SUBSCRIPTION_UNAVAILABLE,
                    )
                })
                .collect();
        }
    };
    let mut sources = if active_subscriptions.is_empty() {
        config.input_health_sources.clone()
    } else {
        config
            .input_health_sources
            .iter()
            .filter(|source| {
                active_subscriptions.iter().any(|subscription| {
                    subscription.asset == source.asset
                        && subscription.source_id == source.source_id
                        && subscription.instrument_id == source.provider_instrument
                })
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    if sources.is_empty() {
        sources = config.input_health_sources.clone();
    }
    sources
        .iter()
        .map(|source| chainlink_reference_input_health_source_with_reason(source, reason))
        .collect()
}

fn chainlink_reference_input_health_source_with_reason(
    source: &BoltV3MissingInputSource,
    reason: &'static str,
) -> BoltV3MissingInputSource {
    let mut source = source.clone();
    source.reason = reason.to_string();
    source
}

fn chainlink_reference_input_health_sources_for_subscription(
    config: &ChainlinkReferencePriceClientConfig,
    subscription: &ChainlinkReferenceSubscription,
    reason: &'static str,
) -> Vec<BoltV3MissingInputSource> {
    config
        .input_health_sources
        .iter()
        .filter(|source| {
            source.asset == subscription.asset
                && source.source_id == subscription.source_id
                && source.provider_instrument == subscription.instrument_id
        })
        .map(|source| chainlink_reference_input_health_source_with_reason(source, reason))
        .collect()
}

fn chainlink_reference_seed_input_health_report_liveness_for_subscription(
    config: &ChainlinkReferencePriceClientConfig,
    report_liveness: &ChainlinkReferenceInputHealthReportLiveness,
    subscription: &ChainlinkReferenceSubscription,
    now_ms: u64,
) {
    let sources = chainlink_reference_input_health_sources_for_subscription(
        config,
        subscription,
        CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED,
    );
    chainlink_reference_seed_input_health_report_liveness_for_sources(
        report_liveness,
        sources.iter(),
        now_ms,
    );
}

fn chainlink_reference_seed_input_health_report_liveness_for_sources<'a>(
    report_liveness: &ChainlinkReferenceInputHealthReportLiveness,
    sources: impl IntoIterator<Item = &'a BoltV3MissingInputSource>,
    now_ms: u64,
) {
    let mut report_liveness = match report_liveness.lock() {
        Ok(report_liveness) => report_liveness,
        Err(error) => {
            log::error!("Chainlink reference report-liveness state poisoned: {error}");
            return;
        }
    };
    for source in sources {
        report_liveness.insert(
            ChainlinkReferenceInputHealthSourceKey::from_source(source),
            now_ms,
        );
    }
}

fn chainlink_reference_remove_input_health_report_liveness_for_subscription(
    config: &ChainlinkReferencePriceClientConfig,
    report_liveness: &ChainlinkReferenceInputHealthReportLiveness,
    subscription: &ChainlinkReferenceSubscription,
) {
    let sources = chainlink_reference_input_health_sources_for_subscription(
        config,
        subscription,
        CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED,
    );
    let mut report_liveness = match report_liveness.lock() {
        Ok(report_liveness) => report_liveness,
        Err(error) => {
            log::error!("Chainlink reference report-liveness state poisoned: {error}");
            return;
        }
    };
    for source in sources {
        report_liveness.remove(&ChainlinkReferenceInputHealthSourceKey::from_source(
            &source,
        ));
    }
}

fn chainlink_reference_refresh_input_health_report_liveness(
    config: &ChainlinkReferencePriceClientConfig,
    report_liveness: &ChainlinkReferenceInputHealthReportLiveness,
    updates: &[ReferencePriceUpdate],
    received_ts_ms: u64,
) {
    let sources = chainlink_reference_recovered_input_health_sources(config, updates);
    chainlink_reference_seed_input_health_report_liveness_for_sources(
        report_liveness,
        sources.iter(),
        received_ts_ms,
    );
}

fn chainlink_reference_stale_input_health_sources(
    config: &ChainlinkReferencePriceClientConfig,
    subscriptions: &Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    report_liveness: &ChainlinkReferenceInputHealthReportLiveness,
    now_ms: u64,
    idle_timeout_ms: u64,
    reason: &'static str,
) -> Vec<BoltV3MissingInputSource> {
    let sources =
        chainlink_reference_input_health_sources_for_transition(config, subscriptions, reason);
    if sources.is_empty() {
        return Vec::new();
    }
    let report_liveness = match report_liveness.lock() {
        Ok(report_liveness) => report_liveness,
        Err(error) => {
            log::error!("Chainlink reference report-liveness state poisoned: {error}");
            return sources;
        }
    };
    sources
        .into_iter()
        .filter(|source| {
            let key = ChainlinkReferenceInputHealthSourceKey::from_source(source);
            let Some(last_report_ms) = report_liveness.get(&key).copied() else {
                return true;
            };
            now_ms.saturating_sub(last_report_ms) > idle_timeout_ms
        })
        .collect()
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
    #[cfg(test)]
    if chainlink_reference_test_loopback_endpoint_is_valid(value, &url) {
        return Ok(url);
    }
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

#[cfg(test)]
fn chainlink_reference_test_loopback_endpoint_is_valid(value: &str, url: &Url) -> bool {
    value.trim() == value
        && url.scheme() == "ws"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && {
            let path = url.path();
            path.is_empty() || path == "/"
        }
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
        reconnect_max_attempts: Some(CHAINLINK_REFERENCE_TRANSPORT_RECONNECT_MAX_ATTEMPTS),
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

#[cfg(test)]
fn chainlink_reference_message_handler(
    feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    subscriptions: Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
) -> WireMessageHandler {
    chainlink_reference_message_handler_with_input_health_recovery(
        feed_bindings,
        subscriptions,
        data_sender,
        Arc::new(AtomicU64::new(0)),
        None,
    )
}

#[derive(Clone)]
struct ChainlinkReferenceInputHealthRecovery {
    config: ChainlinkReferencePriceClientConfig,
    input_health_report_liveness: ChainlinkReferenceInputHealthReportLiveness,
    input_health_missing_sources: ChainlinkReferenceInputHealthMissingSources,
}

fn chainlink_reference_message_handler_with_input_health_recovery(
    feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    subscriptions: Arc<
        Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
    >,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    last_report_unix_ms: Arc<AtomicU64>,
    input_health_recovery: Option<ChainlinkReferenceInputHealthRecovery>,
) -> WireMessageHandler {
    Arc::new(move |message: WireMessage| {
        let received_ts_ms = match current_unix_timestamp_ms() {
            Ok(value) => value,
            Err(error) => {
                log::warn!("Chainlink reference frame dropped: {error}");
                return;
            }
        };
        let frame_bytes = match message {
            WireMessage::Text(bytes) | WireMessage::Binary(bytes) => bytes,
            WireMessage::Ping(bytes) => {
                log::info!(
                    "Chainlink reference WebSocket ping received: payload_bytes={}",
                    bytes.len()
                );
                return;
            }
            WireMessage::Pong(bytes) => {
                log::info!(
                    "Chainlink reference WebSocket pong received: payload_bytes={}",
                    bytes.len()
                );
                return;
            }
            WireMessage::Close => {
                log::warn!("Chainlink reference WebSocket close frame received");
                return;
            }
        };
        let frame = match std::str::from_utf8(frame_bytes.as_ref()) {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!("Chainlink reference frame dropped: invalid UTF-8: {error}");
                return;
            }
        };
        let frame_updates = match subscriptions.lock() {
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
        match frame_updates {
            Ok(frame_updates) => {
                let updates = frame_updates.updates;
                if frame_updates.report_observed {
                    last_report_unix_ms.store(received_ts_ms, Ordering::SeqCst);
                }
                if !updates.is_empty() {
                    if let Some(recovery) = &input_health_recovery {
                        chainlink_reference_refresh_input_health_report_liveness(
                            &recovery.config,
                            &recovery.input_health_report_liveness,
                            &updates,
                            received_ts_ms,
                        );
                    }
                }
                if let Some(recovery) = &input_health_recovery {
                    chainlink_reference_emit_recovered_input_health_for_updates(recovery, &updates);
                }
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

fn chainlink_reference_emit_recovered_input_health_for_updates(
    recovery: &ChainlinkReferenceInputHealthRecovery,
    updates: &[ReferencePriceUpdate],
) {
    if updates.is_empty() {
        return;
    }
    let Some(emitter) = recovery.config.input_health_transition_emitter.as_ref() else {
        return;
    };
    let sources = chainlink_reference_recovered_input_health_sources(&recovery.config, updates);
    if sources.is_empty() {
        return;
    }
    let recovered_sources = {
        let mut missing_sources = match recovery.input_health_missing_sources.lock() {
            Ok(missing_sources) => missing_sources,
            Err(error) => {
                log::error!(
                    "Chainlink reference recovered input-health could not read missing-source state: {error}"
                );
                return;
            }
        };
        sources
            .into_iter()
            .filter(|source| {
                let key = ChainlinkReferenceInputHealthSourceKey::from_source(source);
                missing_sources.remove(&key)
            })
            .collect::<Vec<_>>()
    };
    for source in recovered_sources {
        emitter(
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED,
            BoltV3InputHealthSourceTransition {
                source,
                missing: false,
            },
        );
    }
}

fn chainlink_reference_recovered_input_health_sources(
    config: &ChainlinkReferencePriceClientConfig,
    updates: &[ReferencePriceUpdate],
) -> Vec<BoltV3MissingInputSource> {
    let mut sources = BTreeMap::new();
    for update in updates {
        for source in &config.input_health_sources {
            if source.asset == update.asset()
                && source.source_id == update.source_id()
                && source.provider == update.provider()
                && source.provider_instrument == update.provider_instrument()
            {
                let mut recovered = source.clone();
                recovered.reason = CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED.to_string();
                sources.insert(
                    (
                        recovered.strategy_instance_id.clone(),
                        recovered.source_id.clone(),
                        recovered.asset.clone(),
                        recovered.provider.clone(),
                        recovered.provider_instrument.clone(),
                    ),
                    recovered,
                );
            }
        }
    }
    sources.into_values().collect()
}

struct ChainlinkReferenceReportFrameUpdates {
    report_observed: bool,
    updates: Vec<ReferencePriceUpdate>,
}

fn chainlink_reference_updates_from_report_frame(
    feed_bindings: &[ChainlinkStrikeFeedBinding],
    subscriptions: &BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>,
    frame: &str,
    received_ts_ms: u64,
) -> Result<ChainlinkReferenceReportFrameUpdates, String> {
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
    let updates = subscriptions
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
            .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ChainlinkReferenceReportFrameUpdates {
        report_observed: true,
        updates,
    })
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
    if matches!(
        data.reconnect_max_attempts,
        ChainlinkReferenceReconnectMaxAttempts::Limited(0)
    ) {
        errors.push(format!(
            "clients.{key}.data.reconnect_max_attempts must be positive or \"unlimited\"; Chainlink reference reconnects are provider-supervised so Data Streams auth headers are regenerated on every reconnect"
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

pub(crate) fn attach_live_input_health_transition_emitter(
    adapters: &mut BoltV3AdapterConfigs,
    input_health_transition_emitter: BoltV3InputHealthTransitionEmitter,
    input_health_sources_by_client: &BTreeMap<String, Vec<BoltV3MissingInputSource>>,
) {
    for (client_key, client_config) in &mut adapters.clients {
        let Some(data) = client_config.data.as_mut() else {
            continue;
        };
        let Some(config) = data
            .config
            .as_any()
            .downcast_ref::<ChainlinkReferencePriceClientConfig>()
        else {
            continue;
        };
        let input_health_sources = input_health_sources_by_client
            .get(client_key)
            .into_iter()
            .flat_map(|sources| sources.iter())
            .filter(|source| source.provider == REFERENCE_PRICE_PROVIDER_KEY)
            .cloned()
            .collect::<Vec<_>>();
        if input_health_sources.is_empty() {
            continue;
        }
        let mut updated = config.clone();
        updated.input_health_transition_emitter = Some(input_health_transition_emitter.clone());
        updated.input_health_sources = input_health_sources;
        data.config = Box::new(updated);
    }
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
    let authorization_timestamp_ms = current_unix_timestamp_ms()?;
    chainlink_reference_websocket_config_at(config, authorization_timestamp_ms)
}

fn chainlink_reference_websocket_config_at(
    config: &ChainlinkReferencePriceClientConfig,
    authorization_timestamp_ms: u64,
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
        input_health_transition_emitter: None,
        input_health_sources: Vec::new(),
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
    use futures_util::SinkExt;
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
    // Chainlink Data Streams V3 reports in the shared feed catalog are scaled
    // by 18 decimals; the committed capture proves origin and the catalog scale
    // proves the production decode path remains structurally usable.
    const CAPTURED_REFERENCE_REPORT_DECIMAL_SCALE: u64 = 18;

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
            reconnect_max_attempts: ChainlinkReferenceReconnectMaxAttempts::Unlimited,
            idle_timeout_ms: 10_000,
            feed_bindings: vec![fixture_feed_binding()],
            input_health_transition_emitter: None,
            input_health_sources: Vec::new(),
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
                websocket: Arc::new(Mutex::new(None)),
                last_report_unix_ms: Arc::new(AtomicU64::new(0)),
                input_health_report_liveness: Arc::new(Mutex::new(BTreeMap::new())),
                input_health_missing_sources: Arc::new(Mutex::new(BTreeSet::new())),
                liveness_task: None,
                connected: false,
            },
            data_receiver,
        )
    }

    fn test_last_report_clock() -> Arc<AtomicU64> {
        Arc::new(AtomicU64::new(0))
    }

    fn input_health_source(
        asset: &str,
        source_id: &str,
        provider_instrument: &str,
    ) -> BoltV3MissingInputSource {
        BoltV3MissingInputSource {
            strategy_instance_id: "binary-edge-taker".to_string(),
            source_id: source_id.to_string(),
            asset: asset.to_string(),
            provider: REFERENCE_PRICE_PROVIDER_KEY.to_string(),
            provider_instrument: provider_instrument.to_string(),
            reason: "test".to_string(),
        }
    }

    fn input_health_missing_sources_with(
        sources: Vec<&BoltV3MissingInputSource>,
    ) -> ChainlinkReferenceInputHealthMissingSources {
        let mut missing_sources = BTreeSet::new();
        for source in sources {
            missing_sources.insert(ChainlinkReferenceInputHealthSourceKey::from_source(source));
        }
        Arc::new(Mutex::new(missing_sources))
    }

    #[derive(Debug)]
    struct ChainlinkReferenceHandshakeProbe {
        uri: String,
        has_authorization: bool,
        has_authorization_timestamp: bool,
        has_authorization_signature: bool,
        has_user_agent: bool,
    }

    fn reference_price_update(
        asset: &str,
        source_id: &str,
        provider_instrument: &str,
    ) -> ReferencePriceUpdate {
        ReferencePriceUpdate::try_new(
            asset,
            source_id,
            REFERENCE_PRICE_PROVIDER_KEY,
            provider_instrument,
            TEST_BENCHMARK_PRICE,
            Some(TEST_BID_PRICE),
            Some(TEST_ASK_PRICE),
            TEST_OBSERVATIONS_SECONDS.into(),
            TEST_OBSERVATIONS_SECONDS.into(),
        )
        .expect("test reference price update should be valid")
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
    fn attach_live_input_health_transition_emitter_reboxes_chainlink_reference_config() {
        let mut adapters = BoltV3AdapterConfigs {
            clients: BTreeMap::from([(
                "chainlink_reference".to_string(),
                BoltV3ClientAdapterConfig {
                    data: Some(BoltV3DataClientAdapterConfig {
                        factory: Box::new(ChainlinkReferencePriceClientFactory),
                        config: Box::new(fixture_config()),
                    }),
                    execution: None,
                },
            )]),
        };
        let emitter: BoltV3InputHealthTransitionEmitter = Arc::new(|_, _| {});
        let source = BoltV3MissingInputSource {
            strategy_instance_id: "binary-edge-taker".to_string(),
            source_id: "chainlink_primary".to_string(),
            asset: "BTC".to_string(),
            provider: REFERENCE_PRICE_PROVIDER_KEY.to_string(),
            provider_instrument: TEST_INSTRUMENT_ID.to_string(),
            reason: "test".to_string(),
        };
        let sources_by_client =
            BTreeMap::from([("chainlink_reference".to_string(), vec![source.clone()])]);

        attach_live_input_health_transition_emitter(&mut adapters, emitter, &sources_by_client);

        let config = adapters
            .clients
            .get("chainlink_reference")
            .and_then(|client| client.data.as_ref())
            .and_then(|data| data.config_as::<ChainlinkReferencePriceClientConfig>())
            .expect("Chainlink reference config should remain downcastable after rebox");
        assert!(config.input_health_transition_emitter.is_some());
        assert_eq!(config.input_health_sources, vec![source]);
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
    fn websocket_config_disables_transport_reconnect_for_provider_level_resign() {
        let mut config = fixture_config();
        config.reconnect_max_attempts = ChainlinkReferenceReconnectMaxAttempts::Limited(3);

        let websocket = chainlink_reference_websocket_config_at(&config, 1_700_000_000_000)
            .expect("Chainlink WebSocket config should build");

        assert_eq!(
            websocket.reconnect_max_attempts,
            Some(CHAINLINK_REFERENCE_TRANSPORT_RECONNECT_MAX_ATTEMPTS),
            "transport reconnect must stay disabled so the provider supervisor rebuilds signed headers"
        );
    }

    #[test]
    fn reconnect_websocket_config_resigns_chainlink_auth_headers() {
        let config = fixture_config();

        let first = chainlink_reference_websocket_config_at(&config, 1_700_000_000_000)
            .expect("first signed WebSocket config should build");
        let second = chainlink_reference_websocket_config_at(&config, 1_700_000_001_000)
            .expect("second signed WebSocket config should build");

        fn header<'a>(config: &'a WebSocketConfig, name: &str) -> &'a str {
            config
                .headers
                .iter()
                .find_map(|(key, value)| (key == name).then_some(value.as_str()))
                .expect("signed Chainlink header should be present")
        }
        assert_eq!(header(&first, "X-Authorization-Timestamp"), "1700000000000");
        assert_eq!(
            header(&second, "X-Authorization-Timestamp"),
            "1700000001000"
        );
        assert_ne!(
            header(&first, "X-Authorization-Signature-SHA256"),
            header(&second, "X-Authorization-Signature-SHA256")
        );
    }

    #[test]
    fn chainlink_reference_reconnect_attempt_budget_is_provider_level() {
        assert!(ChainlinkReferenceReconnectMaxAttempts::Unlimited.permits_attempt(u32::MAX));
        assert!(ChainlinkReferenceReconnectMaxAttempts::Limited(2).permits_attempt(1));
        assert!(!ChainlinkReferenceReconnectMaxAttempts::Limited(2).permits_attempt(2));
        assert!(!ChainlinkReferenceReconnectMaxAttempts::Limited(0).permits_attempt(0));
    }

    #[test]
    fn replayed_subscription_count_returns_none_for_poisoned_state() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::new()));
        let poisoned = Arc::clone(&subscriptions);
        let _ = std::panic::catch_unwind(move || {
            let _guard = poisoned
                .lock()
                .expect("subscription state should be lockable before poisoning");
            panic!("poison Chainlink subscription state for reconnect logging test");
        });

        assert_eq!(
            chainlink_reference_replayed_subscription_count(
                &subscriptions,
                ClientId::from("chainlink_reference"),
            ),
            None
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
        let last_report = test_last_report_clock();
        let handler = chainlink_reference_message_handler_with_input_health_recovery(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            Arc::clone(&last_report),
            None,
        );

        handler(WireMessage::text(chainlink_report_frame_json()));
        assert!(
            last_report.load(Ordering::SeqCst) > 0,
            "matched Chainlink report frames should refresh report-data liveness"
        );

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

    #[test]
    fn unsubscribed_report_frame_refreshes_report_liveness_without_emitting_data() {
        let (client, mut data_receiver) = fixture_client();
        let last_report = test_last_report_clock();
        let handler = chainlink_reference_message_handler_with_input_health_recovery(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            Arc::clone(&last_report),
            None,
        );

        handler(WireMessage::text(chainlink_report_frame_json()));

        assert!(
            last_report.load(Ordering::SeqCst) > 0,
            "valid configured Chainlink reports must refresh stream liveness even before NT subscriptions exist"
        );
        let error = data_receiver
            .try_recv()
            .expect_err("unsubscribed Chainlink report frame must not emit data");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "unsubscribed report should leave the data channel open and empty, got {error:?}"
        );
    }

    #[tokio::test]
    async fn runtime_connect_path_refreshes_source_liveness_before_subscription() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("local Chainlink WebSocket fixture should bind");
        let local_addr = listener
            .local_addr()
            .expect("local Chainlink WebSocket fixture should expose its address");
        let report_frame = chainlink_report_frame_json();
        let (handshake_sender, handshake_receiver) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .expect("runtime Chainlink client should connect to the local fixture");
            let mut websocket = tokio_tungstenite::accept_hdr_async(
                stream,
                move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                      response| {
                    let headers = request.headers();
                    let probe = ChainlinkReferenceHandshakeProbe {
                        uri: request.uri().to_string(),
                        has_authorization: headers.contains_key("authorization"),
                        has_authorization_timestamp: headers
                            .contains_key("x-authorization-timestamp"),
                        has_authorization_signature: headers
                            .contains_key("x-authorization-signature-sha256"),
                        has_user_agent: headers.contains_key("user-agent"),
                    };
                    let _ = handshake_sender.send(probe);
                    Ok(response)
                },
            )
            .await
            .expect("runtime Chainlink client should complete the WebSocket handshake");
            websocket
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    report_frame.into(),
                ))
                .await
                .expect("local fixture should send a Chainlink report frame");
            tokio::time::sleep(Duration::from_millis(25)).await;
        });

        let (client, mut data_receiver) = fixture_client();
        let btc_source = input_health_source(TEST_ASSET, TEST_SOURCE_ID, TEST_INSTRUMENT_ID);
        let btc_key = ChainlinkReferenceInputHealthSourceKey::from_source(&btc_source);
        let mut config = client.config.clone();
        config.websocket_endpoint = format!("ws://{local_addr}");
        config.transport_backend = TransportBackend::Tungstenite;
        config.idle_timeout_ms = 60_000;
        config.input_health_sources = vec![btc_source.clone()];
        let last_report = test_last_report_clock();
        let report_liveness = Arc::new(Mutex::new(BTreeMap::new()));
        let missing_sources = Arc::new(Mutex::new(BTreeSet::new()));

        let websocket = chainlink_reference_connect_websocket(
            &config,
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            Arc::clone(&last_report),
            Arc::clone(&report_liveness),
            Arc::clone(&missing_sources),
        )
        .await
        .expect("runtime Chainlink connect path should complete against the local fixture");
        assert_eq!(websocket.connection_mode(), ConnectionMode::Active);
        let handshake = handshake_receiver
            .await
            .expect("local fixture should capture the Chainlink handshake");
        assert_eq!(handshake.uri, format!("/api/v1/ws?feedIDs={TEST_FEED_ID}"));
        assert!(handshake.has_authorization);
        assert!(handshake.has_authorization_timestamp);
        assert!(handshake.has_authorization_signature);
        assert!(handshake.has_user_agent);

        let report_ms = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let report_ms = last_report.load(Ordering::SeqCst);
                if report_ms > 0 {
                    break report_ms;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("valid configured report should refresh client-level liveness");
        {
            let report_liveness = report_liveness
                .lock()
                .expect("report liveness should be available");
            assert_eq!(
                report_liveness.get(&btc_key).copied(),
                Some(report_ms),
                "valid configured reports must refresh source liveness before NT subscriptions exist"
            );
        }
        let error = data_receiver
            .try_recv()
            .expect_err("pre-subscription Chainlink report must not emit data");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "pre-subscription report should leave the data channel open and empty, got {error:?}"
        );

        let mut supervisor_state = ChainlinkReferenceLivenessSupervisorState::new();
        let tick = chainlink_reference_liveness_supervisor_tick(
            ChainlinkReferenceLivenessTickContext {
                config: &config,
                subscriptions: &client.subscriptions,
                input_health_report_liveness: &report_liveness,
                input_health_missing_sources: &missing_sources,
                last_report_unix_ms: last_report.as_ref(),
            },
            report_ms.saturating_sub(1),
            &mut supervisor_state,
            report_ms + 1,
            Some(ConnectionMode::Active),
        );

        assert!(!tick.reconnect);
        assert!(!tick.stream_stale);
        assert!(!tick.source_stale);
        assert!(!tick.source_reconnect);
        assert!(!tick.transport_dead);
        assert!(
            missing_sources
                .lock()
                .expect("missing source set should be available")
                .is_empty()
        );

        websocket.disconnect().await;
        server
            .await
            .expect("local Chainlink WebSocket fixture should finish cleanly");
    }

    #[test]
    fn unknown_feed_report_does_not_refresh_report_liveness() {
        let (client, mut data_receiver) =
            fixture_client_with_bindings(vec![fixture_eth_feed_binding()]);
        let last_report = test_last_report_clock();
        let handler = chainlink_reference_message_handler_with_input_health_recovery(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            Arc::clone(&last_report),
            None,
        );

        handler(WireMessage::text(chainlink_report_frame_json()));

        assert_eq!(
            last_report.load(Ordering::SeqCst),
            0,
            "unknown Chainlink feed frames must not refresh report-data liveness"
        );
        let error = data_receiver
            .try_recv()
            .expect_err("unknown Chainlink report frame must not emit data");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "unknown feed should leave the data channel open and empty, got {error:?}"
        );
    }

    #[test]
    fn binary_report_frame_for_active_subscription_emits_custom_reference_update() {
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

        handler(WireMessage::binary(chainlink_report_frame_json()));

        assert_chainlink_reference_update(
            data_receiver
                .try_recv()
                .expect("matched binary Chainlink report frame should emit one data event"),
        );
    }

    #[test]
    fn committed_real_capture_frame_decodes_through_production_handler() {
        let frame_bytes = include_bytes!(
            "../../tests/fixtures/bolt_v3/boundary_evidence/chainlink-reference-frame.bin"
        );
        let envelope: ChainlinkDataStreamsReportApiResponse = serde_json::from_slice(frame_bytes)
            .expect("committed Chainlink capture frame should be the report envelope");
        let captured_feed_id = envelope.report.feed_id().to_string();
        let captured_instrument_id = "CAPTURED-REFERENCE.CHAINLINK";
        let (mut client, mut data_receiver) =
            fixture_client_with_bindings(vec![ChainlinkStrikeFeedBinding {
                feed_id: captured_feed_id,
                instrument_id: InstrumentId::from_str(captured_instrument_id)
                    .expect("captured Chainlink instrument id should parse"),
                report_schema_version: TEST_REPORT_SCHEMA_VERSION,
                report_decimal_scale: CAPTURED_REFERENCE_REPORT_DECIMAL_SCALE,
                price_precision: 8,
            }]);
        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                captured_instrument_id,
            ))
            .expect("captured Chainlink reference subscription should be accepted");
        let handler = chainlink_reference_message_handler(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
        );

        handler(WireMessage::binary(frame_bytes.to_vec()));

        let event = data_receiver
            .try_recv()
            .expect("committed Chainlink capture frame should emit one data event");
        let DataEvent::Data(Data::Custom(custom)) = event else {
            panic!("committed Chainlink capture frame should emit custom data, got {event:?}");
        };
        let update = ReferencePriceUpdate::from_custom_data(&custom)
            .expect("committed Chainlink capture should decode to a reference price update");
        assert_eq!(update.provider(), REFERENCE_PRICE_PROVIDER_KEY);
        assert!(
            update.price().is_finite() && update.price() > 0.0,
            "committed Chainlink capture should decode to a finite positive price, got {}",
            update.price()
        );
        let error = data_receiver
            .try_recv()
            .expect_err("committed Chainlink capture frame should emit exactly one data event");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "committed Chainlink capture should leave the data channel open after one event, got {error:?}"
        );
    }

    #[test]
    fn invalid_utf8_binary_report_frame_emits_no_custom_data() {
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

        handler(WireMessage::binary(vec![0xff, 0xfe, 0xfd]));

        let error = data_receiver
            .try_recv()
            .expect_err("invalid UTF-8 binary Chainlink frame must not emit data");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "invalid UTF-8 binary Chainlink frame should leave the data channel open and empty, got {error:?}"
        );
    }

    #[test]
    fn control_frames_do_not_refresh_report_liveness() {
        let (client, mut data_receiver) = fixture_client();
        let last_report = test_last_report_clock();
        let handler = chainlink_reference_message_handler_with_input_health_recovery(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            Arc::clone(&last_report),
            None,
        );

        handler(WireMessage::Ping(vec![1]));
        assert_eq!(
            last_report.load(Ordering::SeqCst),
            0,
            "Chainlink Ping control frames must not refresh report-data liveness"
        );
        handler(WireMessage::Pong(vec![2]));
        handler(WireMessage::Close);

        let error = data_receiver
            .try_recv()
            .expect_err("Chainlink control frames must not emit data");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "control frames should leave the data channel open and empty, got {error:?}"
        );
    }

    #[test]
    fn recovered_input_health_waits_for_matching_report_after_missing() {
        let (mut client, _data_receiver) = fixture_client();
        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                TEST_INSTRUMENT_ID,
            ))
            .expect("catalog-backed Chainlink reference subscription should be accepted");
        let source = BoltV3MissingInputSource {
            strategy_instance_id: "binary-edge-taker".to_string(),
            source_id: TEST_SOURCE_ID.to_string(),
            asset: TEST_ASSET.to_string(),
            provider: REFERENCE_PRICE_PROVIDER_KEY.to_string(),
            provider_instrument: TEST_INSTRUMENT_ID.to_string(),
            reason: "test".to_string(),
        };
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&transitions);
        let emitter: BoltV3InputHealthTransitionEmitter = Arc::new(move |reason, transition| {
            recorded
                .lock()
                .expect("transition recorder should be available")
                .push((reason, transition));
        });
        let mut config = client.config.clone();
        config.input_health_transition_emitter = Some(emitter);
        config.input_health_sources = vec![source.clone()];
        let input_health_missing_sources = input_health_missing_sources_with(vec![&source]);
        let handler = chainlink_reference_message_handler_with_input_health_recovery(
            config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            test_last_report_clock(),
            Some(ChainlinkReferenceInputHealthRecovery {
                config,
                input_health_report_liveness: Arc::new(Mutex::new(BTreeMap::new())),
                input_health_missing_sources: Arc::clone(&input_health_missing_sources),
            }),
        );

        handler(WireMessage::Ping(vec![1]));

        assert!(
            transitions
                .lock()
                .expect("transition recorder should be available")
                .is_empty(),
            "control frames must not mark a missing reference input recovered"
        );
        assert!(
            input_health_missing_sources
                .lock()
                .expect("missing source set should be available")
                .contains(&ChainlinkReferenceInputHealthSourceKey::from_source(
                    &source,
                ))
        );

        handler(WireMessage::text(chainlink_report_frame_json()));

        let recorded = transitions
            .lock()
            .expect("transition recorder should be available");
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].0,
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED
        );
        assert!(!recorded[0].1.missing);
        assert_eq!(recorded[0].1.source.source_id.as_str(), TEST_SOURCE_ID);
        assert!(
            input_health_missing_sources
                .lock()
                .expect("missing source set should be available")
                .is_empty()
        );
    }

    #[test]
    fn recovered_input_health_clears_only_matching_missing_source() {
        let btc_source = input_health_source(TEST_ASSET, TEST_SOURCE_ID, TEST_INSTRUMENT_ID);
        let eth_source =
            input_health_source(TEST_ETH_ASSET, TEST_SOURCE_ID, TEST_ETH_INSTRUMENT_ID);
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&transitions);
        let emitter: BoltV3InputHealthTransitionEmitter = Arc::new(move |reason, transition| {
            recorded
                .lock()
                .expect("transition recorder should be available")
                .push((reason, transition));
        });
        let mut config = fixture_config();
        config.input_health_transition_emitter = Some(emitter);
        config.input_health_sources = vec![btc_source.clone(), eth_source.clone()];
        let input_health_missing_sources =
            input_health_missing_sources_with(vec![&btc_source, &eth_source]);
        let recovery = ChainlinkReferenceInputHealthRecovery {
            config,
            input_health_report_liveness: Arc::new(Mutex::new(BTreeMap::new())),
            input_health_missing_sources: Arc::clone(&input_health_missing_sources),
        };
        let btc_update = reference_price_update(TEST_ASSET, TEST_SOURCE_ID, TEST_INSTRUMENT_ID);
        let eth_update =
            reference_price_update(TEST_ETH_ASSET, TEST_SOURCE_ID, TEST_ETH_INSTRUMENT_ID);

        chainlink_reference_emit_recovered_input_health_for_updates(&recovery, &[btc_update]);

        {
            let recorded = transitions
                .lock()
                .expect("transition recorder should be available");
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0].1.source.asset.as_str(), TEST_ASSET);
        }
        {
            let missing_sources = input_health_missing_sources
                .lock()
                .expect("missing source set should be available");
            assert!(!missing_sources.contains(
                &ChainlinkReferenceInputHealthSourceKey::from_source(&btc_source)
            ));
            assert!(missing_sources.contains(
                &ChainlinkReferenceInputHealthSourceKey::from_source(&eth_source)
            ));
        }

        chainlink_reference_emit_recovered_input_health_for_updates(&recovery, &[eth_update]);

        let recorded = transitions
            .lock()
            .expect("transition recorder should be available");
        assert_eq!(recorded.len(), 2);
        assert_eq!(recorded[1].1.source.asset.as_str(), TEST_ETH_ASSET);
        assert!(
            input_health_missing_sources
                .lock()
                .expect("missing source set should be available")
                .is_empty()
        );
    }

    #[test]
    fn stale_input_health_marks_only_source_with_stale_report_clock() {
        let btc_source = input_health_source(TEST_ASSET, TEST_SOURCE_ID, TEST_INSTRUMENT_ID);
        let eth_source =
            input_health_source(TEST_ETH_ASSET, TEST_SOURCE_ID, TEST_ETH_INSTRUMENT_ID);
        let transitions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&transitions);
        let emitter: BoltV3InputHealthTransitionEmitter = Arc::new(move |reason, transition| {
            recorded
                .lock()
                .expect("transition recorder should be available")
                .push((reason, transition));
        });
        let mut config = fixture_config();
        config.input_health_transition_emitter = Some(emitter);
        config.input_health_sources = vec![btc_source.clone(), eth_source.clone()];
        let subscriptions = Arc::new(Mutex::new(BTreeMap::from([
            (
                ChainlinkReferenceSubscriptionKey {
                    asset: TEST_ASSET.to_string(),
                    source_id: TEST_SOURCE_ID.to_string(),
                    instrument_id: TEST_INSTRUMENT_ID.to_string(),
                },
                ChainlinkReferenceSubscription {
                    asset: TEST_ASSET.to_string(),
                    source_id: TEST_SOURCE_ID.to_string(),
                    instrument_id: TEST_INSTRUMENT_ID.to_string(),
                },
            ),
            (
                ChainlinkReferenceSubscriptionKey {
                    asset: TEST_ETH_ASSET.to_string(),
                    source_id: TEST_SOURCE_ID.to_string(),
                    instrument_id: TEST_ETH_INSTRUMENT_ID.to_string(),
                },
                ChainlinkReferenceSubscription {
                    asset: TEST_ETH_ASSET.to_string(),
                    source_id: TEST_SOURCE_ID.to_string(),
                    instrument_id: TEST_ETH_INSTRUMENT_ID.to_string(),
                },
            ),
        ])));
        let report_liveness = Arc::new(Mutex::new(BTreeMap::from([
            (
                ChainlinkReferenceInputHealthSourceKey::from_source(&btc_source),
                20_000,
            ),
            (
                ChainlinkReferenceInputHealthSourceKey::from_source(&eth_source),
                5_000,
            ),
        ])));
        let input_health_missing_sources = Arc::new(Mutex::new(BTreeSet::new()));

        let stale_sources = chainlink_reference_stale_input_health_sources(
            &config,
            &subscriptions,
            &report_liveness,
            20_001,
            10_000,
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE,
        );
        chainlink_reference_emit_missing_input_health_sources(
            &config,
            stale_sources,
            &input_health_missing_sources,
            CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE,
            false,
        );

        let recorded = transitions
            .lock()
            .expect("transition recorder should be available");
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].0, CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE);
        assert!(recorded[0].1.missing);
        assert_eq!(recorded[0].1.source.asset.as_str(), TEST_ETH_ASSET);
        let missing_sources = input_health_missing_sources
            .lock()
            .expect("missing source set should be available");
        assert!(
            !missing_sources.contains(&ChainlinkReferenceInputHealthSourceKey::from_source(
                &btc_source
            ))
        );
        assert!(
            missing_sources.contains(&ChainlinkReferenceInputHealthSourceKey::from_source(
                &eth_source
            ))
        );
    }

    #[test]
    fn liveness_supervisor_resets_budget_only_after_post_reconnect_report() {
        let btc_source = input_health_source(TEST_ASSET, TEST_SOURCE_ID, TEST_INSTRUMENT_ID);
        let eth_source =
            input_health_source(TEST_ETH_ASSET, TEST_SOURCE_ID, TEST_ETH_INSTRUMENT_ID);
        let btc_key = ChainlinkReferenceInputHealthSourceKey::from_source(&btc_source);
        let eth_key = ChainlinkReferenceInputHealthSourceKey::from_source(&eth_source);
        let (mut client, mut data_receiver) =
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

        let transitions = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&transitions);
        let emitter: BoltV3InputHealthTransitionEmitter = Arc::new(move |reason, transition| {
            recorded
                .lock()
                .expect("transition recorder should be available")
                .push((reason, transition));
        });
        let mut config = client.config.clone();
        config.idle_timeout_ms = 60_000;
        config.reconnect_max_attempts = ChainlinkReferenceReconnectMaxAttempts::Limited(2);
        config.input_health_transition_emitter = Some(emitter);
        config.input_health_sources = vec![btc_source.clone(), eth_source.clone()];
        let last_report = test_last_report_clock();
        let report_liveness = Arc::new(Mutex::new(BTreeMap::from([
            (btc_key.clone(), 1),
            (eth_key.clone(), 1),
        ])));
        let input_health_missing_sources = Arc::new(Mutex::new(BTreeSet::new()));
        let handler = chainlink_reference_message_handler_with_input_health_recovery(
            config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
            Arc::clone(&last_report),
            Some(ChainlinkReferenceInputHealthRecovery {
                config: config.clone(),
                input_health_report_liveness: Arc::clone(&report_liveness),
                input_health_missing_sources: Arc::clone(&input_health_missing_sources),
            }),
        );

        handler(WireMessage::text(chainlink_report_frame_json()));

        let btc_report_ms = last_report.load(Ordering::SeqCst);
        assert!(
            btc_report_ms > 1,
            "matched BTC report should refresh the client-level report clock"
        );
        {
            let report_liveness = report_liveness
                .lock()
                .expect("report liveness should be available");
            assert_eq!(report_liveness.get(&btc_key).copied(), Some(btc_report_ms));
            assert_eq!(
                report_liveness.get(&eth_key).copied(),
                Some(1),
                "BTC reports must not refresh ETH source liveness"
            );
        }
        let _ = data_receiver
            .try_recv()
            .expect("BTC report should emit a data event");

        let mut supervisor_state = ChainlinkReferenceLivenessSupervisorState::new();
        let connection_epoch_ms = btc_report_ms.saturating_sub(1);
        let first_tick = chainlink_reference_liveness_supervisor_tick(
            ChainlinkReferenceLivenessTickContext {
                config: &config,
                subscriptions: &client.subscriptions,
                input_health_report_liveness: &report_liveness,
                input_health_missing_sources: &input_health_missing_sources,
                last_report_unix_ms: last_report.as_ref(),
            },
            connection_epoch_ms,
            &mut supervisor_state,
            btc_report_ms + 1,
            Some(ConnectionMode::Active),
        );

        assert!(first_tick.reconnect);
        assert!(!first_tick.stream_stale);
        assert!(first_tick.source_stale);
        assert!(first_tick.source_reconnect);
        assert!(!first_tick.transport_dead);
        assert_eq!(supervisor_state.attempted_reconnects, 1);
        assert_eq!(supervisor_state.last_budget_reset_report_ms, btc_report_ms);
        assert!(
            supervisor_state
                .source_reconnect_attempted
                .contains(&eth_key)
        );
        {
            let missing_sources = input_health_missing_sources
                .lock()
                .expect("missing source set should be available");
            assert!(!missing_sources.contains(&btc_key));
            assert!(missing_sources.contains(&eth_key));
        }
        {
            let recorded = transitions
                .lock()
                .expect("transition recorder should be available");
            assert_eq!(recorded.len(), 1);
            assert_eq!(recorded[0].0, CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_STALE);
            assert_eq!(recorded[0].1.source.asset.as_str(), TEST_ETH_ASSET);
        }

        let reconnected_epoch_ms = btc_report_ms;
        let second_tick = chainlink_reference_liveness_supervisor_tick(
            ChainlinkReferenceLivenessTickContext {
                config: &config,
                subscriptions: &client.subscriptions,
                input_health_report_liveness: &report_liveness,
                input_health_missing_sources: &input_health_missing_sources,
                last_report_unix_ms: last_report.as_ref(),
            },
            reconnected_epoch_ms,
            &mut supervisor_state,
            reconnected_epoch_ms + 1,
            Some(ConnectionMode::Active),
        );

        assert!(!second_tick.reconnect);
        assert!(!second_tick.stream_stale);
        assert!(second_tick.source_stale);
        assert!(!second_tick.source_reconnect);
        assert_eq!(
            supervisor_state.attempted_reconnects, 1,
            "a successful handshake without a new matched report must not reset the budget"
        );
        assert!(
            input_health_missing_sources
                .lock()
                .expect("missing source set should be available")
                .contains(&eth_key),
            "connection grace must not clear source-level missing state"
        );
        assert_eq!(
            transitions
                .lock()
                .expect("transition recorder should be available")
                .len(),
            1,
            "a persistent stale source must not trigger repeated missing transitions"
        );

        let mut clock_advanced = current_unix_timestamp_ms()
            .expect("test clock should be readable")
            > reconnected_epoch_ms;
        for _ in u64::MIN..config.idle_timeout_ms {
            if clock_advanced {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
            clock_advanced = current_unix_timestamp_ms().expect("test clock should be readable")
                > reconnected_epoch_ms;
        }
        assert!(
            clock_advanced,
            "test clock must advance past the simulated reconnect epoch before the recovery report"
        );
        handler(WireMessage::text(chainlink_report_frame_json_for_feed(
            TEST_ETH_FEED_ID,
        )));

        let eth_report_ms = last_report.load(Ordering::SeqCst);
        assert!(
            eth_report_ms > reconnected_epoch_ms,
            "ETH report must postdate the simulated reconnect epoch"
        );
        {
            let report_liveness = report_liveness
                .lock()
                .expect("report liveness should be available");
            assert_eq!(report_liveness.get(&btc_key).copied(), Some(btc_report_ms));
            assert_eq!(report_liveness.get(&eth_key).copied(), Some(eth_report_ms));
        }
        let _ = data_receiver
            .try_recv()
            .expect("ETH report should emit a data event");
        {
            let recorded = transitions
                .lock()
                .expect("transition recorder should be available");
            assert_eq!(recorded.len(), 2);
            assert_eq!(
                recorded[1].0,
                CHAINLINK_REFERENCE_INPUT_HEALTH_REASON_RECOVERED
            );
            assert_eq!(recorded[1].1.source.asset.as_str(), TEST_ETH_ASSET);
        }
        assert!(
            input_health_missing_sources
                .lock()
                .expect("missing source set should be available")
                .is_empty()
        );

        let recovered_tick = chainlink_reference_liveness_supervisor_tick(
            ChainlinkReferenceLivenessTickContext {
                config: &config,
                subscriptions: &client.subscriptions,
                input_health_report_liveness: &report_liveness,
                input_health_missing_sources: &input_health_missing_sources,
                last_report_unix_ms: last_report.as_ref(),
            },
            reconnected_epoch_ms,
            &mut supervisor_state,
            eth_report_ms + 1,
            Some(ConnectionMode::Active),
        );

        assert!(!recovered_tick.reconnect);
        assert!(!recovered_tick.stream_stale);
        assert!(!recovered_tick.source_stale);
        assert_eq!(
            supervisor_state.attempted_reconnects, 0,
            "a matched report after the reconnect epoch is the only reconnect-budget reset"
        );
        assert_eq!(supervisor_state.last_budget_reset_report_ms, eth_report_ms);
        assert!(
            supervisor_state.source_reconnect_attempted.is_empty(),
            "source reconnect edge state should clear after the source becomes healthy"
        );
    }

    #[test]
    fn binary_report_frame_through_text_only_handler_emits_no_custom_data() {
        let (mut client, mut data_receiver) = fixture_client();
        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                TEST_INSTRUMENT_ID,
            ))
            .expect("catalog-backed Chainlink reference subscription should be accepted");
        let handler = chainlink_reference_text_only_mutation_handler(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
        );

        handler(WireMessage::binary(chainlink_report_frame_json()));

        let error = data_receiver.try_recv().expect_err(
            "text-only Chainlink handler mutation must drop the provider's binary frame",
        );
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "text-only mutation should leave the data channel open and empty, got {error:?}"
        );
    }

    #[test]
    fn planted_drop_binary_arm_mutation_would_fail_the_binary_observation_test() {
        let (mut client, mut data_receiver) = fixture_client();
        client
            .subscribe(reference_price_subscribe_cmd(
                TEST_ASSET,
                TEST_SOURCE_ID,
                TEST_INSTRUMENT_ID,
            ))
            .expect("catalog-backed Chainlink reference subscription should be accepted");
        let handler = chainlink_reference_text_only_mutation_handler(
            client.config.feed_bindings.clone(),
            Arc::clone(&client.subscriptions),
            client.data_sender.clone(),
        );

        handler(WireMessage::binary(chainlink_report_frame_json()));

        assert!(
            data_receiver.try_recv().is_err(),
            "dropping the Chainlink Binary arm must make the binary observation path fail"
        );
    }

    fn assert_chainlink_reference_update(event: DataEvent) {
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

    fn chainlink_reference_text_only_mutation_handler(
        feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
        subscriptions: Arc<
            Mutex<BTreeMap<ChainlinkReferenceSubscriptionKey, ChainlinkReferenceSubscription>>,
        >,
        data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    ) -> WireMessageHandler {
        Arc::new(move |message: WireMessage| {
            let WireMessage::Text(bytes) = message else {
                return;
            };
            let frame = match std::str::from_utf8(bytes.as_ref()) {
                Ok(frame) => frame,
                Err(_) => return,
            };
            let frame_updates = match subscriptions.lock() {
                Ok(subscriptions) => chainlink_reference_updates_from_report_frame(
                    &feed_bindings,
                    &subscriptions,
                    frame,
                    u64::from(TEST_OBSERVATIONS_SECONDS) * 1_000,
                ),
                Err(error) => Err(format!(
                    "Chainlink reference subscription state poisoned: {error}"
                )),
            };
            if let Ok(frame_updates) = frame_updates {
                for update in frame_updates.updates {
                    let _ =
                        data_sender.send(DataEvent::Data(Data::Custom(update.to_custom_data())));
                }
            }
        })
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
        chainlink_report_frame_json_for_feed(TEST_FEED_ID)
    }

    fn chainlink_report_frame_json_for_feed(feed_id: &str) -> String {
        let report_source = report_source_json(
            feed_id,
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
