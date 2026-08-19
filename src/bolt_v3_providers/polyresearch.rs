//! PolyResearch reference-price WebSocket authentication helpers and provider
//! binding.
//!
//! PRR auth is a query parameter named `apiKey`. Bolt keeps the endpoint and
//! credential as separate SSM values, then constructs the credentialed URL once
//! at the provider edge.

use std::{
    any::Any,
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

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
use nautilus_core::{Params, string::secret::REDACTED};
use nautilus_model::{
    data::Data,
    identifiers::{ClientId, Venue},
};
use nautilus_network::mode::ConnectionMode;
use serde::{Deserialize, Serialize};
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
    bolt_v3_wire_boundary::{
        self, BoundaryWebSocket, TransportBackend, WebSocketConfig, WireMessage, WireMessageHandler,
    },
};

const POLYRESEARCH_API_KEY_QUERY_FIELD: &str = "apiKey";
const POLYRESEARCH_PRICE_FEED_FRAME_TYPE: &str = "price_feed";
const POLYRESEARCH_SUBSCRIBED_FRAME_TYPE: &str = "subscribed";
const POLYRESEARCH_SUBSCRIBE_ACTION: &str = "subscribe";
const POLYRESEARCH_UNSUBSCRIBE_ACTION: &str = "unsubscribe";
const POLYRESEARCH_REFERENCE_SUBSCRIPTION_TYPE: &str = "chainlink";
pub const KEY: &str = "POLYRESEARCH_REFERENCE_PRICE";
pub const REFERENCE_PRICE_PROVIDER_KEY: &str = "polyresearch_ws";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "PolyResearch reference-price source",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_parameter"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_network::websocket::client"];
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
    pub reconnect_max_attempts: PolyResearchReconnectMaxAttempts,
    pub subscribe_ack_timeout_ms: u64,
    pub idle_timeout_ms: u64,
}

pub(crate) fn reconnect_timeout_ms_for_nt_connect_budget(
    data: &toml::Value,
) -> Result<u64, toml::de::Error> {
    data.clone()
        .try_into::<PolyResearchReferencePriceDataConfig>()
        .map(|data| data.reconnect_timeout_ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolyResearchReconnectMaxAttempts {
    Unlimited,
    Limited(u32),
}

impl PolyResearchReconnectMaxAttempts {
    fn as_websocket_config(self) -> Option<u32> {
        match self {
            Self::Unlimited => None,
            Self::Limited(max_attempts) => Some(max_attempts),
        }
    }
}

impl<'de> Deserialize<'de> for PolyResearchReconnectMaxAttempts {
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
    pub reconnect_max_attempts: PolyResearchReconnectMaxAttempts,
    pub subscribe_ack_timeout_ms: u64,
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
            .field("reconnect_max_attempts", &self.reconnect_max_attempts)
            .field("subscribe_ack_timeout_ms", &self.subscribe_ack_timeout_ms)
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
            config: config.clone(),
            subscriptions: Arc::new(Mutex::new(BTreeMap::new())),
            pending_provider_subscriptions: Arc::new(Mutex::new(VecDeque::new())),
            provider_subscription_ids: Arc::new(Mutex::new(BTreeMap::new())),
            provider_subscription_generation: Arc::new(AtomicU64::new(0)),
            outbound: None,
            connection_mode: None,
            data_sender: get_data_event_sender(),
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
    config: PolyResearchReferencePriceClientConfig,
    subscriptions: Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_ids: Arc<Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, String>>>,
    provider_subscription_generation: Arc<AtomicU64>,
    outbound: Option<PolyResearchReferenceOutboundHandle>,
    connection_mode: Option<Arc<AtomicU8>>,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    connected: bool,
}

#[derive(Debug, Clone)]
struct PolyResearchReferenceOutboundHandle {
    sender: tokio::sync::mpsc::UnboundedSender<PolyResearchReferenceOutboundCommand>,
    #[cfg(test)]
    send_failures_remaining: Arc<AtomicUsize>,
}

impl PolyResearchReferenceOutboundHandle {
    fn send_text(&self, frame: String) -> anyhow::Result<()> {
        #[cfg(test)]
        if self
            .send_failures_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            let (sender, receiver) =
                tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
            drop(receiver);
            return sender
                .send(PolyResearchReferenceOutboundCommand::SendText(frame))
                .map_err(|error| {
                    anyhow::anyhow!("PolyResearch reference outbound task unavailable: {error}")
                });
        }
        self.sender
            .send(PolyResearchReferenceOutboundCommand::SendText(frame))
            .map_err(|error| {
                anyhow::anyhow!("PolyResearch reference outbound task unavailable: {error}")
            })
    }

    async fn disconnect(&self) -> anyhow::Result<()> {
        let (complete_sender, complete_receiver) = tokio::sync::oneshot::channel();
        self.sender
            .send(PolyResearchReferenceOutboundCommand::Disconnect(Some(
                complete_sender,
            )))
            .map_err(|error| {
                anyhow::anyhow!("PolyResearch reference outbound task unavailable: {error}")
            })?;
        complete_receiver
            .await
            .map_err(|error| anyhow::anyhow!("PolyResearch reference disconnect failed: {error}"))
    }

    fn spawn_disconnect(&self) -> anyhow::Result<()> {
        self.sender
            .send(PolyResearchReferenceOutboundCommand::Disconnect(None))
            .map_err(|error| {
                anyhow::anyhow!("PolyResearch reference outbound task unavailable: {error}")
            })
    }

    #[cfg(test)]
    fn from_sender(
        sender: tokio::sync::mpsc::UnboundedSender<PolyResearchReferenceOutboundCommand>,
    ) -> Self {
        Self {
            sender,
            send_failures_remaining: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cfg(test)]
    fn from_sender_with_send_failures(
        sender: tokio::sync::mpsc::UnboundedSender<PolyResearchReferenceOutboundCommand>,
        send_failures: usize,
    ) -> Self {
        Self {
            sender,
            send_failures_remaining: Arc::new(AtomicUsize::new(send_failures)),
        }
    }
}

#[derive(Debug)]
enum PolyResearchReferenceOutboundCommand {
    SendText(String),
    Disconnect(Option<tokio::sync::oneshot::Sender<()>>),
}

impl PolyResearchReferencePriceClient {
    fn clear_subscriptions(&self) -> anyhow::Result<()> {
        self.subscriptions
            .lock()
            .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
            .clear();
        self.pending_provider_subscriptions
            .lock()
            .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
            .clear();
        self.provider_subscription_ids
            .lock()
            .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
            .clear();
        self.advance_provider_subscription_generation();
        Ok(())
    }

    fn clear_provider_subscription_state(&self) -> anyhow::Result<()> {
        self.pending_provider_subscriptions
            .lock()
            .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
            .clear();
        self.provider_subscription_ids
            .lock()
            .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
            .clear();
        self.advance_provider_subscription_generation();
        Ok(())
    }

    fn advance_provider_subscription_generation(&self) {
        self.provider_subscription_generation
            .fetch_add(1, Ordering::SeqCst);
    }

    fn spawn_disconnect(&mut self) {
        if let Some(outbound) = self.outbound.take()
            && let Err(error) = outbound.spawn_disconnect()
        {
            log::warn!("PolyResearch reference disconnect dropped: {error}");
        }
        self.connection_mode = None;
    }
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
        self.spawn_disconnect();
        self.connected = false;
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.spawn_disconnect();
        self.connected = false;
        self.clear_subscriptions()
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.spawn_disconnect();
        self.connected = false;
        self.clear_subscriptions()
    }

    fn is_connected(&self) -> bool {
        polyresearch_reference_transport_connected(
            self.connected,
            self.connection_mode
                .as_ref()
                .map(|mode| ConnectionMode::from_u8(mode.load(Ordering::SeqCst))),
        )
    }

    fn is_disconnected(&self) -> bool {
        !self.is_connected()
    }

    async fn connect(&mut self) -> anyhow::Result<()> {
        if self.outbound.is_some() {
            self.disconnect().await?;
        }
        let url = polyresearch_websocket_url(&PolyResearchAuthConfig {
            websocket_endpoint: self.config.websocket_endpoint.clone(),
            api_key: self.config.api_key.to_string(),
        })
        .map_err(anyhow::Error::msg)?;
        self.clear_provider_subscription_state()?;
        let config = polyresearch_websocket_client_config(&self.config, url);
        let (outbound, outbound_receiver) = polyresearch_reference_outbound_channel();
        let message_handler = polyresearch_reference_message_handler(
            Arc::clone(&self.subscriptions),
            Arc::clone(&self.pending_provider_subscriptions),
            Arc::clone(&self.provider_subscription_ids),
            Arc::clone(&self.provider_subscription_generation),
            Some(outbound.clone()),
            self.config.subscribe_ack_timeout_ms,
            self.data_sender.clone(),
        );
        let post_reconnection = polyresearch_reference_post_reconnection_handler(
            Arc::clone(&self.subscriptions),
            Arc::clone(&self.pending_provider_subscriptions),
            Arc::clone(&self.provider_subscription_ids),
            Arc::clone(&self.provider_subscription_generation),
            outbound.clone(),
            self.config.subscribe_ack_timeout_ms,
        );
        let websocket = bolt_v3_wire_boundary::connect_websocket(
            config,
            Some(message_handler),
            None,
            Some(post_reconnection),
            vec![],
            None,
        )
        .await?;
        self.connection_mode = Some(websocket.connection_mode_atomic());
        polyresearch_reference_spawn_outbound_task(websocket, outbound_receiver);
        replay_polyresearch_reference_subscriptions(
            &self.subscriptions,
            &self.pending_provider_subscriptions,
            &self.provider_subscription_generation,
            &outbound,
            self.config.subscribe_ack_timeout_ms,
        )?;
        self.outbound = Some(outbound);
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        if let Some(outbound) = self.outbound.take() {
            outbound.disconnect().await?;
        }
        self.connection_mode = None;
        self.clear_provider_subscription_state()?;
        self.connected = false;
        Ok(())
    }

    fn subscribe(&mut self, cmd: SubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            polyresearch_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        let subscription_key =
            PolyResearchReferenceSubscriptionKey::from_subscription(&subscription);
        let inserted = {
            let mut subscriptions = self.subscriptions.lock().map_err(|error| {
                anyhow::anyhow!("PolyResearch subscription state poisoned: {error}")
            })?;
            if let std::collections::btree_map::Entry::Vacant(entry) =
                subscriptions.entry(subscription_key.clone())
            {
                entry.insert(subscription.clone());
                true
            } else {
                false
            }
        };
        if inserted
            && let Some(outbound) = self.outbound.as_ref()
            && let Err(error) = queue_polyresearch_reference_subscribe(
                &subscription,
                &self.pending_provider_subscriptions,
                &self.provider_subscription_generation,
                outbound,
                self.config.subscribe_ack_timeout_ms,
            )
        {
            self.subscriptions
                .lock()
                .map_err(|lock_error| {
                    anyhow::anyhow!("PolyResearch subscription state poisoned: {lock_error}")
                })?
                .remove(&subscription_key);
            return Err(error);
        }
        Ok(())
    }

    fn unsubscribe(&mut self, cmd: &UnsubscribeCustomData) -> anyhow::Result<()> {
        let subscription =
            polyresearch_reference_subscription_from_command(&cmd.data_type, cmd.params.as_ref())
                .map_err(anyhow::Error::msg)?;
        let subscription_key =
            PolyResearchReferenceSubscriptionKey::from_subscription(&subscription);
        let (removed, subscriptions_empty) = {
            let mut subscriptions = self.subscriptions.lock().map_err(|error| {
                anyhow::anyhow!("PolyResearch subscription state poisoned: {error}")
            })?;
            let removed = subscriptions.remove(&subscription_key).is_some();
            (removed, subscriptions.is_empty())
        };
        if !removed {
            return Ok(());
        }
        let provider_subscription_id = self
            .provider_subscription_ids
            .lock()
            .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
            .remove(&subscription_key);
        if let Some(outbound) = self.outbound.as_ref() {
            if let Some(provider_subscription_id) = provider_subscription_id {
                outbound.send_text(polyresearch_reference_unsubscribe_frame(Some(
                    provider_subscription_id.as_str(),
                ))?)?;
            } else if subscriptions_empty {
                outbound.send_text(polyresearch_reference_unsubscribe_frame(None)?)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolyResearchReferenceSubscription {
    pub(crate) asset: String,
    pub(crate) source_id: String,
    pub(crate) symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PolyResearchReferenceSubscriptionKey {
    asset: String,
    source_id: String,
    symbol: String,
}

impl PolyResearchReferenceSubscriptionKey {
    fn from_subscription(subscription: &PolyResearchReferenceSubscription) -> Self {
        Self {
            asset: subscription.asset.clone(),
            source_id: subscription.source_id.clone(),
            symbol: subscription.symbol.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
struct PolyResearchReferenceSubscribeFrame<'a> {
    action: &'static str,
    r#type: &'static str,
    filters: PolyResearchReferenceSubscribeFilters<'a>,
}

#[derive(Debug, Serialize)]
struct PolyResearchReferenceUnsubscribeFrame<'a> {
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    subscription_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct PolyResearchReferenceSubscribeFilters<'a> {
    feeds: Vec<&'a str>,
}

fn polyresearch_reference_subscribe_frame(
    subscription: &PolyResearchReferenceSubscription,
) -> anyhow::Result<String> {
    let frame = PolyResearchReferenceSubscribeFrame {
        action: POLYRESEARCH_SUBSCRIBE_ACTION,
        r#type: POLYRESEARCH_REFERENCE_SUBSCRIPTION_TYPE,
        filters: PolyResearchReferenceSubscribeFilters {
            feeds: vec![subscription.symbol.as_str()],
        },
    };
    serde_json::to_string(&frame).map_err(|error| {
        anyhow::anyhow!("PolyResearch subscribe frame serialization failed: {error}")
    })
}

fn polyresearch_reference_unsubscribe_frame(
    provider_subscription_id: Option<&str>,
) -> anyhow::Result<String> {
    let frame = PolyResearchReferenceUnsubscribeFrame {
        action: POLYRESEARCH_UNSUBSCRIBE_ACTION,
        subscription_id: provider_subscription_id,
    };
    serde_json::to_string(&frame).map_err(|error| {
        anyhow::anyhow!("PolyResearch unsubscribe frame serialization failed: {error}")
    })
}

fn polyresearch_reference_outbound_channel() -> (
    PolyResearchReferenceOutboundHandle,
    tokio::sync::mpsc::UnboundedReceiver<PolyResearchReferenceOutboundCommand>,
) {
    let (sender, receiver) =
        tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
    (
        PolyResearchReferenceOutboundHandle {
            sender,
            #[cfg(test)]
            send_failures_remaining: Arc::new(AtomicUsize::new(0)),
        },
        receiver,
    )
}

fn polyresearch_reference_spawn_outbound_task(
    websocket: BoundaryWebSocket,
    mut receiver: tokio::sync::mpsc::UnboundedReceiver<PolyResearchReferenceOutboundCommand>,
) {
    get_runtime().spawn(async move {
        while let Some(command) = receiver.recv().await {
            match command {
                PolyResearchReferenceOutboundCommand::SendText(frame) => {
                    if let Err(error) = websocket.send_text(frame).await {
                        log::warn!("PolyResearch reference outbound frame dropped: {error}");
                    }
                }
                PolyResearchReferenceOutboundCommand::Disconnect(complete_sender) => {
                    websocket.disconnect().await;
                    if let Some(complete_sender) = complete_sender {
                        let _ = complete_sender.send(());
                    }
                    break;
                }
            }
        }
    });
}

fn polyresearch_reference_post_reconnection_handler(
    subscriptions: Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_ids: Arc<Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, String>>>,
    provider_subscription_generation: Arc<AtomicU64>,
    outbound: PolyResearchReferenceOutboundHandle,
    subscribe_ack_timeout_ms: u64,
) -> Arc<dyn Fn() + Send + Sync> {
    Arc::new(move || {
        if let Err(error) = clear_polyresearch_provider_subscription_state(
            &pending_provider_subscriptions,
            &provider_subscription_ids,
            &provider_subscription_generation,
        )
        .and_then(|_| {
            replay_polyresearch_reference_subscriptions(
                &subscriptions,
                &pending_provider_subscriptions,
                &provider_subscription_generation,
                &outbound,
                subscribe_ack_timeout_ms,
            )
        }) {
            log::warn!(
                "PolyResearch reference subscription replay after reconnect failed: {error}"
            );
        }
    })
}

fn replay_polyresearch_reference_subscriptions(
    subscriptions: &Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_generation: &Arc<AtomicU64>,
    outbound: &PolyResearchReferenceOutboundHandle,
    subscribe_ack_timeout_ms: u64,
) -> anyhow::Result<()> {
    let subscriptions = subscriptions
        .lock()
        .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
        .values()
        .cloned()
        .collect::<Vec<_>>();
    for subscription in &subscriptions {
        if let Err(error) = queue_polyresearch_reference_subscribe(
            subscription,
            pending_provider_subscriptions,
            provider_subscription_generation,
            outbound,
            subscribe_ack_timeout_ms,
        ) {
            let _ = outbound.spawn_disconnect();
            return Err(error);
        }
    }
    Ok(())
}

fn clear_polyresearch_provider_subscription_state(
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_ids: &Arc<Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, String>>>,
    provider_subscription_generation: &Arc<AtomicU64>,
) -> anyhow::Result<()> {
    pending_provider_subscriptions
        .lock()
        .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
        .clear();
    provider_subscription_ids
        .lock()
        .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?
        .clear();
    provider_subscription_generation.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

fn queue_polyresearch_reference_subscribe(
    subscription: &PolyResearchReferenceSubscription,
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_generation: &Arc<AtomicU64>,
    outbound: &PolyResearchReferenceOutboundHandle,
    subscribe_ack_timeout_ms: u64,
) -> anyhow::Result<()> {
    let provider_frame = polyresearch_reference_subscribe_frame(subscription)?;
    let subscription_key = PolyResearchReferenceSubscriptionKey::from_subscription(subscription);
    let expected_generation = provider_subscription_generation.load(Ordering::SeqCst);
    let should_send = {
        let mut pending = pending_provider_subscriptions.lock().map_err(|error| {
            anyhow::anyhow!("PolyResearch subscription state poisoned: {error}")
        })?;
        let should_send = pending.is_empty();
        pending.push_back(subscription_key.clone());
        should_send
    };
    if should_send && let Err(error) = outbound.send_text(provider_frame) {
        remove_polyresearch_pending_subscription(
            pending_provider_subscriptions,
            &subscription_key,
        )?;
        return Err(error);
    }
    if should_send {
        spawn_polyresearch_reference_subscribe_ack_timeout(
            Arc::clone(pending_provider_subscriptions),
            Arc::clone(provider_subscription_generation),
            outbound.clone(),
            subscription_key,
            expected_generation,
            subscribe_ack_timeout_ms,
        );
    }
    Ok(())
}

fn spawn_polyresearch_reference_subscribe_ack_timeout(
    pending_provider_subscriptions: Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_generation: Arc<AtomicU64>,
    outbound: PolyResearchReferenceOutboundHandle,
    subscription_key: PolyResearchReferenceSubscriptionKey,
    expected_generation: u64,
    subscribe_ack_timeout_ms: u64,
) {
    get_runtime().spawn(async move {
        tokio::time::sleep(Duration::from_millis(subscribe_ack_timeout_ms)).await;
        if let Err(error) = polyresearch_reference_handle_subscribe_ack_timeout(
            &pending_provider_subscriptions,
            &provider_subscription_generation,
            &outbound,
            &subscription_key,
            expected_generation,
        ) {
            log::warn!("PolyResearch provider subscribe ack timeout handling failed: {error}");
        }
    });
}

fn polyresearch_reference_handle_subscribe_ack_timeout(
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_generation: &Arc<AtomicU64>,
    outbound: &PolyResearchReferenceOutboundHandle,
    subscription_key: &PolyResearchReferenceSubscriptionKey,
    expected_generation: u64,
) -> Result<(), String> {
    if provider_subscription_generation.load(Ordering::SeqCst) != expected_generation {
        return Ok(());
    }
    let timed_out = pending_provider_subscriptions
        .lock()
        .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?
        .front()
        == Some(subscription_key);
    if timed_out {
        log::warn!(
            "PolyResearch provider subscribe ack timed out; disconnecting reference stream for replay"
        );
        outbound
            .spawn_disconnect()
            .map_err(|error| format!("PolyResearch reference disconnect failed: {error}"))?;
    }
    Ok(())
}

fn remove_polyresearch_pending_subscription(
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    subscription_key: &PolyResearchReferenceSubscriptionKey,
) -> anyhow::Result<()> {
    let mut pending = pending_provider_subscriptions
        .lock()
        .map_err(|error| anyhow::anyhow!("PolyResearch subscription state poisoned: {error}"))?;
    if let Some(index) = pending
        .iter()
        .position(|pending_key| pending_key == subscription_key)
    {
        pending.remove(index);
    }
    Ok(())
}

fn polyresearch_reference_transport_connected(
    started: bool,
    transport_mode: Option<ConnectionMode>,
) -> bool {
    started && transport_mode.is_some_and(|mode| mode.is_active())
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
    r#type: Option<String>,
    subscription_id: Option<String>,
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

#[cfg(test)]
pub(crate) fn polyresearch_reference_update_from_price_frame(
    subscription: &PolyResearchReferenceSubscription,
    frame: &str,
    received_ts_ms: u64,
) -> Result<Option<ReferencePriceUpdate>, String> {
    let parsed = serde_json::from_str::<PolyResearchPriceFrame>(frame)
        .map_err(|error| format!("invalid PolyResearch price frame JSON: {error}"))?;
    polyresearch_reference_update_from_parsed_price_frame(subscription, &parsed, received_ts_ms)
}

fn polyresearch_reference_update_from_parsed_price_frame(
    subscription: &PolyResearchReferenceSubscription,
    parsed: &PolyResearchPriceFrame,
    received_ts_ms: u64,
) -> Result<Option<ReferencePriceUpdate>, String> {
    if parsed.r#type.as_deref() != Some(POLYRESEARCH_PRICE_FEED_FRAME_TYPE) {
        return Ok(None);
    }

    let data = parsed
        .data
        .as_ref()
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

fn polyresearch_reference_updates_from_parsed_price_frame(
    subscriptions: &BTreeMap<
        PolyResearchReferenceSubscriptionKey,
        PolyResearchReferenceSubscription,
    >,
    parsed: &PolyResearchPriceFrame,
    received_ts_ms: u64,
) -> Result<Vec<ReferencePriceUpdate>, String> {
    let mut updates = Vec::new();
    for subscription in subscriptions.values() {
        if let Some(update) = polyresearch_reference_update_from_parsed_price_frame(
            subscription,
            parsed,
            received_ts_ms,
        )? {
            updates.push(update);
        }
    }
    Ok(updates)
}

#[cfg(test)]
fn polyresearch_reference_record_subscription_ack(
    subscriptions: &Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_ids: &Arc<Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, String>>>,
    outbound: Option<&PolyResearchReferenceOutboundHandle>,
    subscribe_ack_timeout_ms: u64,
    frame: &str,
) -> Result<bool, String> {
    let parsed = serde_json::from_str::<PolyResearchPriceFrame>(frame)
        .map_err(|error| format!("invalid PolyResearch control frame JSON: {error}"))?;
    let provider_subscription_generation = Arc::new(AtomicU64::new(0));
    polyresearch_reference_record_subscription_ack_from_parsed(
        subscriptions,
        pending_provider_subscriptions,
        provider_subscription_ids,
        &provider_subscription_generation,
        outbound,
        subscribe_ack_timeout_ms,
        &parsed,
    )
}

fn polyresearch_reference_record_subscription_ack_from_parsed(
    subscriptions: &Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_ids: &Arc<Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, String>>>,
    provider_subscription_generation: &Arc<AtomicU64>,
    outbound: Option<&PolyResearchReferenceOutboundHandle>,
    subscribe_ack_timeout_ms: u64,
    parsed: &PolyResearchPriceFrame,
) -> Result<bool, String> {
    if parsed.r#type.as_deref() != Some(POLYRESEARCH_SUBSCRIBED_FRAME_TYPE) {
        return Ok(false);
    }
    let provider_subscription_id = parsed
        .subscription_id
        .as_ref()
        .ok_or_else(|| "PolyResearch subscribed frame missing subscription_id".to_string())?;
    let subscription_key = pending_provider_subscriptions
        .lock()
        .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?
        .pop_front()
        .ok_or_else(|| "PolyResearch subscribed frame has no pending subscription".to_string())?;
    let subscription_active = subscriptions
        .lock()
        .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?
        .contains_key(&subscription_key);
    if !subscription_active {
        let unsubscribe_result = if let Some(outbound) = outbound {
            let frame = polyresearch_reference_unsubscribe_frame(Some(provider_subscription_id))
                .map_err(|error| error.to_string())?;
            outbound
                .send_text(frame)
                .map_err(|error| format!("PolyResearch unsubscribe frame send failed: {error}"))
        } else {
            Ok(())
        };
        send_next_polyresearch_pending_subscription(
            subscriptions,
            pending_provider_subscriptions,
            provider_subscription_generation,
            outbound,
            subscribe_ack_timeout_ms,
        )?;
        unsubscribe_result?;
        return Ok(true);
    }
    provider_subscription_ids
        .lock()
        .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?
        .insert(subscription_key, provider_subscription_id.clone());
    send_next_polyresearch_pending_subscription(
        subscriptions,
        pending_provider_subscriptions,
        provider_subscription_generation,
        outbound,
        subscribe_ack_timeout_ms,
    )?;
    Ok(true)
}

fn send_next_polyresearch_pending_subscription(
    subscriptions: &Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: &Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_generation: &Arc<AtomicU64>,
    outbound: Option<&PolyResearchReferenceOutboundHandle>,
    subscribe_ack_timeout_ms: u64,
) -> Result<(), String> {
    let Some(outbound) = outbound else {
        return Ok(());
    };
    loop {
        let Some(subscription_key) = pending_provider_subscriptions
            .lock()
            .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?
            .front()
            .cloned()
        else {
            return Ok(());
        };
        let subscription = subscriptions
            .lock()
            .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?
            .get(&subscription_key)
            .cloned();
        if let Some(subscription) = subscription {
            let frame = polyresearch_reference_subscribe_frame(&subscription).map_err(|error| {
                format!("PolyResearch subscribe frame serialization failed: {error}")
            })?;
            let expected_generation = provider_subscription_generation.load(Ordering::SeqCst);
            outbound
                .send_text(frame)
                .map_err(|error| format!("PolyResearch subscribe frame send failed: {error}"))?;
            spawn_polyresearch_reference_subscribe_ack_timeout(
                Arc::clone(pending_provider_subscriptions),
                Arc::clone(provider_subscription_generation),
                (*outbound).clone(),
                subscription_key,
                expected_generation,
                subscribe_ack_timeout_ms,
            );
            return Ok(());
        }
        let mut pending = pending_provider_subscriptions
            .lock()
            .map_err(|error| format!("PolyResearch subscription state poisoned: {error}"))?;
        if pending.front() == Some(&subscription_key) {
            pending.pop_front();
        }
    }
}

fn polyresearch_reference_message_handler(
    subscriptions: Arc<
        Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, PolyResearchReferenceSubscription>>,
    >,
    pending_provider_subscriptions: Arc<Mutex<VecDeque<PolyResearchReferenceSubscriptionKey>>>,
    provider_subscription_ids: Arc<Mutex<BTreeMap<PolyResearchReferenceSubscriptionKey, String>>>,
    provider_subscription_generation: Arc<AtomicU64>,
    outbound: Option<PolyResearchReferenceOutboundHandle>,
    subscribe_ack_timeout_ms: u64,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
) -> WireMessageHandler {
    Arc::new(move |message: WireMessage| {
        let frame_bytes = match message {
            WireMessage::Text(bytes) | WireMessage::Binary(bytes) => bytes,
            _ => return,
        };
        let frame = match std::str::from_utf8(frame_bytes.as_ref()) {
            Ok(frame) => frame,
            Err(error) => {
                log::warn!("PolyResearch reference frame dropped: invalid UTF-8: {error}");
                return;
            }
        };
        let received_ts_ms = match current_unix_timestamp_ms() {
            Ok(value) => value,
            Err(error) => {
                log::warn!("PolyResearch reference frame dropped: {error}");
                return;
            }
        };
        let parsed = match serde_json::from_str::<PolyResearchPriceFrame>(frame) {
            Ok(parsed) => parsed,
            Err(error) => {
                log::warn!("PolyResearch reference frame dropped: invalid JSON: {error}");
                return;
            }
        };
        match polyresearch_reference_record_subscription_ack_from_parsed(
            &subscriptions,
            &pending_provider_subscriptions,
            &provider_subscription_ids,
            &provider_subscription_generation,
            outbound.as_ref(),
            subscribe_ack_timeout_ms,
            &parsed,
        ) {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                log::warn!("PolyResearch reference frame dropped: {error}");
                return;
            }
        }
        let updates = match subscriptions.lock() {
            Ok(subscriptions) => polyresearch_reference_updates_from_parsed_price_frame(
                &subscriptions,
                &parsed,
                received_ts_ms,
            ),
            Err(error) => Err(format!("PolyResearch subscription state poisoned: {error}")),
        };
        match updates {
            Ok(updates) => {
                for update in updates {
                    if let Err(error) =
                        data_sender.send(DataEvent::Data(Data::Custom(update.to_custom_data())))
                    {
                        log::warn!("PolyResearch reference update dropped: {error}");
                    }
                }
            }
            Err(error) => log::warn!("PolyResearch reference frame dropped: {error}"),
        }
    })
}

pub(crate) fn polyresearch_websocket_client_config(
    config: &PolyResearchReferencePriceClientConfig,
    url: Url,
) -> WebSocketConfig {
    WebSocketConfig {
        url: url.to_string(),
        headers: vec![],
        heartbeat_interval_secs: config.heartbeat_secs,
        heartbeat_payload: config.heartbeat_message.clone(),
        connect_timeout_ms: Some(config.reconnect_timeout_ms),
        reconnect_delay_initial_ms: Some(config.reconnect_delay_initial_ms),
        reconnect_delay_max_ms: Some(config.reconnect_delay_max_ms),
        reconnect_backoff_factor: Some(config.reconnect_backoff_factor),
        reconnect_jitter_ms: Some(config.reconnect_jitter_ms),
        reconnect_max_attempts: config.reconnect_max_attempts.as_websocket_config(),
        heartbeat_timeout_secs: None,
        idle_timeout_ms: Some(config.idle_timeout_ms),
        backend: config.transport_backend,
        proxy_url: None,
    }
}

fn current_unix_timestamp_ms() -> anyhow::Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| anyhow::anyhow!("system clock before UNIX epoch: {error}"))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| anyhow::anyhow!("system clock timestamp exceeds supported range"))
}

pub struct PolyResearchAuthConfig {
    pub websocket_endpoint: String,
    pub api_key: String,
}

pub fn polyresearch_websocket_url(config: &PolyResearchAuthConfig) -> Result<Url, String> {
    validate_secret_field("api_key", &config.api_key)?;
    let mut url = validate_websocket_endpoint(&config.websocket_endpoint)?;
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
    if matches!(
        data.reconnect_max_attempts,
        PolyResearchReconnectMaxAttempts::Limited(0)
    ) {
        errors.push(format!(
            "clients.{key}.data.reconnect_max_attempts must be positive or \"unlimited\""
        ));
    }
    validate_positive_u64(
        &format!("clients.{key}.data.subscribe_ack_timeout_ms"),
        data.subscribe_ack_timeout_ms,
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

pub(crate) fn reference_price_client_config(
    client_key: &str,
    client: &ClientBlock,
    resolved: &crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<PolyResearchReferencePriceClientConfig, BoltV3AdapterMappingError> {
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
    map_data(client_key, value, secrets)
}

pub(crate) fn reference_price_websocket_config(
    config: &PolyResearchReferencePriceClientConfig,
) -> Result<WebSocketConfig, String> {
    let url = polyresearch_websocket_url(&PolyResearchAuthConfig {
        websocket_endpoint: config.websocket_endpoint.clone(),
        api_key: config.api_key.as_str().to_string(),
    })?;
    Ok(polyresearch_websocket_client_config(config, url))
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
        reconnect_max_attempts: cfg.reconnect_max_attempts,
        subscribe_ack_timeout_ms: cfg.subscribe_ack_timeout_ms,
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
            reconnect_max_attempts: PolyResearchReconnectMaxAttempts::Unlimited,
            subscribe_ack_timeout_ms: 2_000,
            idle_timeout_ms: 10_000,
            api_key: Zeroizing::new("polyresearch-api-key".to_string()),
        }
    }

    #[test]
    fn websocket_config_carries_toml_owned_reconnect_max_attempts() {
        let mut config = fixture_config();
        config.reconnect_max_attempts = PolyResearchReconnectMaxAttempts::Limited(3);
        let websocket = polyresearch_websocket_client_config(
            &config,
            Url::parse("wss://ws.polyresearch.xyz/reference?apiKey=test")
                .expect("fixture URL should parse"),
        );
        assert_eq!(websocket.reconnect_max_attempts, Some(3));
        assert_eq!(websocket.heartbeat_interval_secs, Some(5));
        assert_eq!(websocket.heartbeat_payload.as_deref(), Some("ping"));
        assert_eq!(websocket.heartbeat_timeout_secs, None);
        assert_eq!(websocket.connect_timeout_ms, Some(5_000));

        config.reconnect_max_attempts = PolyResearchReconnectMaxAttempts::Unlimited;
        let websocket = polyresearch_websocket_client_config(
            &config,
            Url::parse("wss://ws.polyresearch.xyz/reference?apiKey=test")
                .expect("fixture URL should parse"),
        );
        assert_eq!(websocket.reconnect_max_attempts, None);
    }

    fn fixture_client() -> (
        PolyResearchReferencePriceClient,
        tokio::sync::mpsc::UnboundedReceiver<DataEvent>,
    ) {
        let (data_sender, data_receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            PolyResearchReferencePriceClient {
                client_id: ClientId::from("polyresearch_reference"),
                config: fixture_config(),
                subscriptions: Arc::new(Mutex::new(BTreeMap::new())),
                pending_provider_subscriptions: Arc::new(Mutex::new(VecDeque::new())),
                provider_subscription_ids: Arc::new(Mutex::new(BTreeMap::new())),
                provider_subscription_generation: Arc::new(AtomicU64::new(0)),
                outbound: None,
                connection_mode: None,
                data_sender,
                connected: false,
            },
            data_receiver,
        )
    }

    fn subscription_key(
        asset: &str,
        source_id: &str,
        symbol: &str,
    ) -> PolyResearchReferenceSubscriptionKey {
        PolyResearchReferenceSubscriptionKey {
            asset: asset.to_string(),
            source_id: source_id.to_string(),
            symbol: symbol.to_string(),
        }
    }

    fn subscription(
        asset: &str,
        source_id: &str,
        symbol: &str,
    ) -> PolyResearchReferenceSubscription {
        PolyResearchReferenceSubscription {
            asset: asset.to_string(),
            source_id: source_id.to_string(),
            symbol: symbol.to_string(),
        }
    }

    #[test]
    fn start_stop_update_nt_data_client_connected_state() {
        let (mut client, _data_receiver) = fixture_client();

        assert!(client.is_disconnected());

        client
            .start()
            .expect("polyresearch reference start should succeed");
        assert!(
            client.is_disconnected(),
            "start without a WebSocket transport must not mask a later connect failure"
        );

        client
            .stop()
            .expect("polyresearch reference stop should succeed");
        assert!(client.is_disconnected());
    }

    #[test]
    fn transport_closed_state_reports_data_client_disconnected() {
        assert!(
            !polyresearch_reference_transport_connected(true, Some(ConnectionMode::Closed)),
            "closed PRR transport must fail closed instead of reporting stale connected state"
        );
        assert!(
            !polyresearch_reference_transport_connected(true, Some(ConnectionMode::Reconnect)),
            "reconnecting PRR transport must not report healthy connected state"
        );
        assert!(
            !polyresearch_reference_transport_connected(true, None),
            "missing PRR transport state must fail closed instead of reporting connected"
        );
        assert!(polyresearch_reference_transport_connected(
            true,
            Some(ConnectionMode::Active)
        ));
    }

    #[test]
    fn price_feed_frame_for_active_subscription_emits_custom_reference_update() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::from([(
            subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
            subscription("BTC", "polyresearch_primary", "BTC/USD"),
        )])));
        let (data_sender, mut data_receiver) = tokio::sync::mpsc::unbounded_channel();
        let handler = polyresearch_reference_message_handler(
            subscriptions,
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(BTreeMap::new())),
            Arc::new(AtomicU64::new(0)),
            None,
            2_000,
            data_sender,
        );

        handler(WireMessage::text(
            r#"{"type":"price_feed","feed":"BTC/USD","timestamp":1774672588,"data":{"feed":"BTC/USD","price":66300.25,"bid":66299.5,"ask":66301.0,"timestamp":1774672588}}"#,
        ));

        let event = data_receiver
            .try_recv()
            .expect("matched PRR price_feed frame should emit one data event");
        let DataEvent::Data(Data::Custom(custom)) = event else {
            panic!("matched PRR price_feed frame should emit custom data, got {event:?}");
        };
        let update = ReferencePriceUpdate::from_custom_data(&custom)
            .expect("custom data should contain a reference price update");

        assert_eq!(update.asset(), "BTC");
        assert_eq!(update.source_id(), "polyresearch_primary");
        assert_eq!(update.provider(), REFERENCE_PRICE_PROVIDER_KEY);
        assert_eq!(update.provider_instrument(), "BTC/USD");
        assert_eq!(update.price(), 66300.25);
        assert_eq!(update.observed_ts_ms(), 1774672588000);
    }

    #[test]
    fn binary_price_feed_frame_for_active_subscription_emits_custom_reference_update() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::from([(
            subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
            subscription("BTC", "polyresearch_primary", "BTC/USD"),
        )])));
        let (data_sender, mut data_receiver) = tokio::sync::mpsc::unbounded_channel();
        let handler = polyresearch_reference_message_handler(
            subscriptions,
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(BTreeMap::new())),
            Arc::new(AtomicU64::new(0)),
            None,
            2_000,
            data_sender,
        );

        handler(WireMessage::binary(
            r#"{"type":"price_feed","feed":"BTC/USD","timestamp":1774672588,"data":{"feed":"BTC/USD","price":66300.25,"bid":66299.5,"ask":66301.0,"timestamp":1774672588}}"#,
        ));

        let event = data_receiver
            .try_recv()
            .expect("matched binary PRR price_feed frame should emit one data event");
        let DataEvent::Data(Data::Custom(custom)) = event else {
            panic!("matched binary PRR price_feed frame should emit custom data, got {event:?}");
        };
        let update = ReferencePriceUpdate::from_custom_data(&custom)
            .expect("custom data should contain a reference price update");

        assert_eq!(update.asset(), "BTC");
        assert_eq!(update.source_id(), "polyresearch_primary");
        assert_eq!(update.provider(), REFERENCE_PRICE_PROVIDER_KEY);
        assert_eq!(update.provider_instrument(), "BTC/USD");
        assert_eq!(update.price(), 66300.25);
        assert_eq!(update.observed_ts_ms(), 1774672588000);
    }

    #[test]
    fn invalid_utf8_binary_frame_emits_no_custom_data() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::from([(
            subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
            subscription("BTC", "polyresearch_primary", "BTC/USD"),
        )])));
        let (data_sender, mut data_receiver) = tokio::sync::mpsc::unbounded_channel();
        let handler = polyresearch_reference_message_handler(
            subscriptions,
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(BTreeMap::new())),
            Arc::new(AtomicU64::new(0)),
            None,
            2_000,
            data_sender,
        );

        handler(WireMessage::binary(vec![0xff, 0xfe, 0xfd]));

        let error = data_receiver
            .try_recv()
            .expect_err("invalid UTF-8 binary PRR frame must not emit data");
        assert!(
            matches!(error, tokio::sync::mpsc::error::TryRecvError::Empty),
            "invalid UTF-8 binary PRR frame should leave the data channel open and empty, got {error:?}"
        );
    }

    #[test]
    fn price_feed_frame_for_unsubscribed_symbol_emits_no_custom_data() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::from([(
            subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
            subscription("BTC", "polyresearch_primary", "BTC/USD"),
        )])));
        let (data_sender, mut data_receiver) = tokio::sync::mpsc::unbounded_channel();
        let handler = polyresearch_reference_message_handler(
            subscriptions,
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(BTreeMap::new())),
            Arc::new(AtomicU64::new(0)),
            None,
            2_000,
            data_sender,
        );

        handler(WireMessage::text(
            r#"{"type":"price_feed","feed":"ETH/USD","timestamp":1774672588,"data":{"feed":"ETH/USD","price":3000.25,"bid":2999.5,"ask":3001.0,"timestamp":1774672588}}"#,
        ));

        assert!(
            data_receiver.try_recv().is_err(),
            "unsubscribed PRR price_feed frame should not emit a data event"
        );
    }

    #[test]
    fn subscribe_custom_data_records_prr_reference_subscription() {
        let (mut client, _data_receiver) = fixture_client();

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("PRR reference subscription should be accepted");

        let subscriptions = client
            .subscriptions
            .lock()
            .expect("PRR reference subscriptions should not be poisoned");
        let subscription = subscriptions
            .get(&subscription_key("BTC", "polyresearch_primary", "BTC/USD"))
            .expect("PRR reference subscription should be recorded");
        assert_eq!(subscription.asset, "BTC");
        assert_eq!(subscription.source_id, "polyresearch_primary");
        assert_eq!(subscription.symbol, "BTC/USD");
    }

    #[test]
    fn same_source_id_subscriptions_remain_asset_scoped() {
        let (mut client, _data_receiver) = fixture_client();

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("BTC PRR reference subscription should be accepted");
        client
            .subscribe(reference_price_subscribe_cmd(
                "ETH",
                "polyresearch_primary",
                "ETH/USD",
            ))
            .expect("ETH PRR reference subscription should be accepted");

        let subscriptions = client
            .subscriptions
            .lock()
            .expect("PRR reference subscriptions should not be poisoned");
        assert_eq!(
            subscriptions.len(),
            2,
            "same source_id across assets must not overwrite active WebSocket subscriptions"
        );
        assert!(subscriptions.contains_key(&subscription_key(
            "BTC",
            "polyresearch_primary",
            "BTC/USD",
        )));
        assert!(subscriptions.contains_key(&subscription_key(
            "ETH",
            "polyresearch_primary",
            "ETH/USD",
        )));
    }

    #[test]
    fn subscribe_custom_data_sends_prr_provider_subscribe_frame() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        client.outbound = Some(PolyResearchReferenceOutboundHandle::from_sender(
            outbound_sender,
        ));
        assert!(client.outbound.is_some());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("PRR reference subscription should be accepted");

        let outbound_command = outbound_receiver
            .try_recv()
            .expect("PRR subscription should send a provider subscribe frame");
        let PolyResearchReferenceOutboundCommand::SendText(frame) = outbound_command else {
            panic!("PRR subscription should send text, got {outbound_command:?}");
        };
        assert_eq!(
            frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );
    }

    #[test]
    fn failed_prr_provider_subscribe_enqueue_rolls_back_local_subscription() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        client.outbound = Some(
            PolyResearchReferenceOutboundHandle::from_sender_with_send_failures(outbound_sender, 1),
        );

        let error = client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect_err("failed provider enqueue should reject subscription");

        assert!(
            error
                .to_string()
                .contains("PolyResearch reference outbound task unavailable"),
            "unexpected subscription error: {error}"
        );
        assert!(
            !client
                .subscriptions
                .lock()
                .expect("PRR reference subscriptions should not be poisoned")
                .contains_key(&subscription_key("BTC", "polyresearch_primary", "BTC/USD")),
            "failed provider enqueue must roll back the local subscription"
        );
        assert!(
            client
                .pending_provider_subscriptions
                .lock()
                .expect("provider subscription queue should not be poisoned")
                .is_empty(),
            "failed provider enqueue must not leave stale pending state"
        );
        assert!(
            outbound_receiver.try_recv().is_err(),
            "failed provider enqueue must not emit a provider subscribe frame"
        );
    }

    #[test]
    fn prr_provider_subscribe_frames_are_single_flight_until_ack() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("first PRR reference subscription should be accepted");
        client
            .subscribe(reference_price_subscribe_cmd(
                "ETH",
                "polyresearch_secondary",
                "ETH/USD",
            ))
            .expect("second PRR reference subscription should be accepted");

        let PolyResearchReferenceOutboundCommand::SendText(first_frame) = outbound_receiver
            .try_recv()
            .expect("first PRR subscription should send provider subscribe")
        else {
            panic!("first PRR subscription should send text");
        };
        assert_eq!(
            first_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );
        assert!(
            outbound_receiver.try_recv().is_err(),
            "second PRR provider subscribe must wait until the first subscription is acked"
        );

        let handled = polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            Some(&outbound),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        )
        .expect("first PRR subscribed ack should be handled");
        assert!(handled);

        let PolyResearchReferenceOutboundCommand::SendText(second_frame) = outbound_receiver
            .try_recv()
            .expect("second PRR subscription should send only after first ack")
        else {
            panic!("second PRR subscription should send text after first ack");
        };
        assert_eq!(
            second_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["ETH/USD"]}}"#
        );
    }

    #[test]
    fn prr_provider_second_subscribe_ack_timeout_disconnects_after_first_ack() {
        let (mut client, _data_receiver) = fixture_client();
        client.config.subscribe_ack_timeout_ms = 10;
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("first PRR reference subscription should be accepted");
        client
            .subscribe(reference_price_subscribe_cmd(
                "ETH",
                "polyresearch_secondary",
                "ETH/USD",
            ))
            .expect("second PRR reference subscription should be accepted");

        let PolyResearchReferenceOutboundCommand::SendText(_) = outbound_receiver
            .try_recv()
            .expect("first PRR subscription should send provider subscribe")
        else {
            panic!("first PRR subscription should send text");
        };
        assert!(
            outbound_receiver.try_recv().is_err(),
            "second PRR provider subscribe must wait until the first subscription is acked"
        );

        polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            Some(&outbound),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        )
        .expect("first PRR subscribed ack should be handled");

        let PolyResearchReferenceOutboundCommand::SendText(_) = outbound_receiver
            .try_recv()
            .expect("second PRR subscription should send after first ack")
        else {
            panic!("second PRR subscription should send text after first ack");
        };

        std::thread::sleep(Duration::from_millis(50));

        let PolyResearchReferenceOutboundCommand::Disconnect(None) = outbound_receiver
            .try_recv()
            .expect("second in-flight PRR subscribe should disconnect on missing ack")
        else {
            panic!("second in-flight PRR subscribe should send a disconnect command");
        };
    }

    #[test]
    fn prr_provider_subscribe_ack_timeout_disconnects_without_advancing_queue() {
        let btc_key = subscription_key("BTC", "polyresearch_primary", "BTC/USD");
        let eth_key = subscription_key("ETH", "polyresearch_secondary", "ETH/USD");
        let pending_provider_subscriptions = Arc::new(Mutex::new(VecDeque::from([
            btc_key.clone(),
            eth_key.clone(),
        ])));
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        let provider_subscription_generation = Arc::new(AtomicU64::new(0));

        polyresearch_reference_handle_subscribe_ack_timeout(
            &pending_provider_subscriptions,
            &provider_subscription_generation,
            &outbound,
            &btc_key,
            0,
        )
        .expect("subscribe ack timeout handling should not fail");

        let PolyResearchReferenceOutboundCommand::Disconnect(None) = outbound_receiver
            .try_recv()
            .expect("timed-out PRR provider subscribe should disconnect")
        else {
            panic!("timed-out PRR provider subscribe should send a disconnect command");
        };
        assert_eq!(
            pending_provider_subscriptions
                .lock()
                .expect("provider subscription queue should not be poisoned")
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
            vec![btc_key, eth_key],
            "timeout must not pop the queue because late subscribed acks are FIFO-only"
        );
    }

    #[test]
    fn duplicate_prr_provider_subscribe_does_not_queue_duplicate_provider_frame() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("first PRR reference subscription should be accepted");
        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("duplicate PRR reference subscription should be accepted");

        let PolyResearchReferenceOutboundCommand::SendText(first_frame) = outbound_receiver
            .try_recv()
            .expect("first PRR subscription should send provider subscribe")
        else {
            panic!("first PRR subscription should send text");
        };
        assert_eq!(
            first_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );
        assert!(
            outbound_receiver.try_recv().is_err(),
            "duplicate PRR provider subscribe must not send before the first ack"
        );

        let handled = polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            Some(&outbound),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        )
        .expect("first PRR subscribed ack should be handled");
        assert!(handled);
        assert!(
            outbound_receiver.try_recv().is_err(),
            "duplicate PRR provider subscribe must not send after the first ack"
        );
    }

    #[test]
    fn post_reconnection_replays_prr_reference_subscriptions() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::new()));
        subscriptions
            .lock()
            .expect("test subscription state should lock")
            .insert(
                subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
                PolyResearchReferenceSubscription {
                    asset: "BTC".to_string(),
                    source_id: "polyresearch_primary".to_string(),
                    symbol: "BTC/USD".to_string(),
                },
            );
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);

        let handler = polyresearch_reference_post_reconnection_handler(
            Arc::clone(&subscriptions),
            Arc::new(Mutex::new(VecDeque::new())),
            Arc::new(Mutex::new(BTreeMap::new())),
            Arc::new(AtomicU64::new(0)),
            outbound,
            2_000,
        );
        handler();

        let outbound_command = outbound_receiver
            .try_recv()
            .expect("post-reconnect replay should send a provider subscribe frame");
        let PolyResearchReferenceOutboundCommand::SendText(frame) = outbound_command else {
            panic!("post-reconnect replay should send text, got {outbound_command:?}");
        };
        assert_eq!(
            frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );
    }

    #[test]
    fn stale_prr_provider_subscribe_ack_timeout_after_reconnect_does_not_disconnect_replay() {
        let (mut client, _data_receiver) = fixture_client();
        client.config.subscribe_ack_timeout_ms = 100;
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("PRR reference subscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(_) = outbound_receiver
            .try_recv()
            .expect("initial PRR subscription should send provider subscribe")
        else {
            panic!("initial PRR subscription should send text");
        };

        std::thread::sleep(Duration::from_millis(70));
        let handler = polyresearch_reference_post_reconnection_handler(
            Arc::clone(&client.subscriptions),
            Arc::clone(&client.pending_provider_subscriptions),
            Arc::clone(&client.provider_subscription_ids),
            Arc::clone(&client.provider_subscription_generation),
            outbound,
            client.config.subscribe_ack_timeout_ms,
        );
        handler();
        let PolyResearchReferenceOutboundCommand::SendText(_) = outbound_receiver
            .try_recv()
            .expect("post-reconnect replay should send provider subscribe")
        else {
            panic!("post-reconnect replay should send text");
        };

        std::thread::sleep(Duration::from_millis(50));
        assert!(
            outbound_receiver.try_recv().is_err(),
            "stale pre-reconnect timeout must not disconnect a freshly replayed subscription"
        );
    }

    #[test]
    fn failed_prr_replay_disconnects_outbound_task() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::new()));
        subscriptions
            .lock()
            .expect("test subscription state should lock")
            .insert(
                subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
                PolyResearchReferenceSubscription {
                    asset: "BTC".to_string(),
                    source_id: "polyresearch_primary".to_string(),
                    symbol: "BTC/USD".to_string(),
                },
            );
        let pending_provider_subscriptions = Arc::new(Mutex::new(VecDeque::new()));
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound =
            PolyResearchReferenceOutboundHandle::from_sender_with_send_failures(outbound_sender, 1);

        let error = replay_polyresearch_reference_subscriptions(
            &subscriptions,
            &pending_provider_subscriptions,
            &Arc::new(AtomicU64::new(0)),
            &outbound,
            2_000,
        )
        .expect_err("failed PRR replay should be surfaced");

        assert!(
            error
                .to_string()
                .contains("PolyResearch reference outbound task unavailable"),
            "unexpected replay error: {error}"
        );
        let outbound_command = outbound_receiver
            .try_recv()
            .expect("failed PRR replay should disconnect the outbound task");
        let PolyResearchReferenceOutboundCommand::Disconnect(None) = outbound_command else {
            panic!("failed replay should send a best-effort disconnect, got {outbound_command:?}");
        };
    }

    #[test]
    fn unsubscribe_custom_data_removes_prr_reference_subscription() {
        let (mut client, _data_receiver) = fixture_client();
        client
            .subscriptions
            .lock()
            .expect("PRR reference subscriptions should not be poisoned")
            .insert(
                subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
                subscription("BTC", "polyresearch_primary", "BTC/USD"),
            );
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
            .unsubscribe(&UnsubscribeCustomData::new(
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
            .expect("PRR reference unsubscription should be accepted");

        assert!(
            !client
                .subscriptions
                .lock()
                .expect("PRR reference subscriptions should not be poisoned")
                .contains_key(&subscription_key("BTC", "polyresearch_primary", "BTC/USD",)),
            "PRR reference unsubscription should remove the active subscription"
        );
    }

    #[test]
    fn subscribed_ack_allows_unsubscribe_to_send_prr_provider_subscription_id() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        client.outbound = Some(PolyResearchReferenceOutboundHandle::from_sender(
            outbound_sender,
        ));

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("PRR reference subscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("PRR subscription should send provider subscribe")
        else {
            panic!("PRR subscription should send text");
        };
        assert_eq!(
            subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );

        let handled = polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            client.outbound.as_ref(),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:1"}"#,
        )
        .expect("subscribed ack should record provider subscription id");
        assert!(handled, "subscribed ack must be handled as a control frame");

        client
            .unsubscribe(&reference_price_unsubscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("PRR reference unsubscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(unsubscribe_frame) = outbound_receiver
            .try_recv()
            .expect("PRR unsubscription should send provider unsubscribe")
        else {
            panic!("PRR unsubscription should send text");
        };
        assert_eq!(
            unsubscribe_frame,
            r#"{"action":"unsubscribe","subscription_id":"chainlink:1"}"#
        );
    }

    #[test]
    fn canceled_pending_subscription_unsubscribes_provider_after_late_ack() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());
        let (data_sender, _data_receiver) = tokio::sync::mpsc::unbounded_channel();
        let handler = polyresearch_reference_message_handler(
            Arc::clone(&client.subscriptions),
            Arc::clone(&client.pending_provider_subscriptions),
            Arc::clone(&client.provider_subscription_ids),
            Arc::clone(&client.provider_subscription_generation),
            Some(outbound),
            client.config.subscribe_ack_timeout_ms,
            data_sender,
        );

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("BTC PRR reference subscription should be accepted");
        client
            .subscribe(reference_price_subscribe_cmd(
                "ETH",
                "polyresearch_secondary",
                "ETH/USD",
            ))
            .expect("ETH PRR reference subscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(frame) = outbound_receiver
            .try_recv()
            .expect("first PRR subscription should send provider subscribe")
        else {
            panic!("first PRR subscription should send text");
        };
        assert_eq!(
            frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );
        assert!(
            outbound_receiver.try_recv().is_err(),
            "second PRR provider subscribe must wait behind the in-flight provider subscribe"
        );

        client
            .unsubscribe(&reference_price_unsubscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("pre-ack PRR reference unsubscription should be accepted");
        assert!(
            outbound_receiver.try_recv().is_err(),
            "pre-ack partial unsubscribe should wait for provider subscription_id"
        );

        handler(WireMessage::text(
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        ));
        let PolyResearchReferenceOutboundCommand::SendText(unsubscribe_frame) = outbound_receiver
            .try_recv()
            .expect("late ack for canceled PRR subscription should send provider unsubscribe")
        else {
            panic!("PRR late ack should send text");
        };
        assert_eq!(
            unsubscribe_frame,
            r#"{"action":"unsubscribe","subscription_id":"chainlink:btc"}"#
        );
        let PolyResearchReferenceOutboundCommand::SendText(eth_subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("second PRR subscription should send after first late ack is drained")
        else {
            panic!("second PRR subscription should send text after first late ack");
        };
        assert_eq!(
            eth_subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["ETH/USD"]}}"#
        );

        handler(WireMessage::text(
            r#"{"type":"subscribed","subscription_id":"chainlink:eth"}"#,
        ));
        assert_eq!(
            client
                .provider_subscription_ids
                .lock()
                .expect("provider subscription ids should not be poisoned")
                .get(&subscription_key(
                    "ETH",
                    "polyresearch_secondary",
                    "ETH/USD"
                ))
                .map(String::as_str),
            Some("chainlink:eth")
        );
    }

    #[test]
    fn failed_late_cancel_unsubscribe_still_advances_prr_pending_queue() {
        let subscriptions = Arc::new(Mutex::new(BTreeMap::new()));
        subscriptions
            .lock()
            .expect("test subscription state should lock")
            .insert(
                subscription_key("ETH", "polyresearch_secondary", "ETH/USD"),
                subscription("ETH", "polyresearch_secondary", "ETH/USD"),
            );
        let pending_provider_subscriptions = Arc::new(Mutex::new(VecDeque::from([
            subscription_key("BTC", "polyresearch_primary", "BTC/USD"),
            subscription_key("ETH", "polyresearch_secondary", "ETH/USD"),
        ])));
        let provider_subscription_ids = Arc::new(Mutex::new(BTreeMap::new()));
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound =
            PolyResearchReferenceOutboundHandle::from_sender_with_send_failures(outbound_sender, 1);

        let error = polyresearch_reference_record_subscription_ack(
            &subscriptions,
            &pending_provider_subscriptions,
            &provider_subscription_ids,
            Some(&outbound),
            2_000,
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        )
        .expect_err("failed late-cancel provider unsubscribe should be surfaced");

        assert!(
            error.contains("PolyResearch unsubscribe frame send failed"),
            "unexpected late-cancel error: {error}"
        );
        let PolyResearchReferenceOutboundCommand::SendText(eth_subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("next PRR subscription must send even after late-cancel cleanup fails")
        else {
            panic!("next PRR subscription should send text after failed late-cancel cleanup");
        };
        assert_eq!(
            eth_subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["ETH/USD"]}}"#
        );
        assert_eq!(
            pending_provider_subscriptions
                .lock()
                .expect("provider subscription queue should not be poisoned")
                .front()
                .cloned(),
            Some(subscription_key("ETH", "polyresearch_secondary", "ETH/USD")),
            "failed late-cancel cleanup must not leave the canceled subscription at the queue head"
        );
    }

    #[test]
    fn canceled_only_pending_subscription_waits_for_late_ack_before_next_subscribe() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("BTC PRR reference subscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(btc_subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("first PRR subscription should send provider subscribe")
        else {
            panic!("first PRR subscription should send text");
        };
        assert_eq!(
            btc_subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );

        client
            .unsubscribe(&reference_price_unsubscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("pre-ack PRR reference unsubscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(unsubscribe_frame) = outbound_receiver
            .try_recv()
            .expect("canceling the only pending PRR subscription should send provider unsubscribe")
        else {
            panic!("PRR unsubscription should send text");
        };
        assert_eq!(unsubscribe_frame, r#"{"action":"unsubscribe"}"#);

        client
            .subscribe(reference_price_subscribe_cmd(
                "ETH",
                "polyresearch_secondary",
                "ETH/USD",
            ))
            .expect("ETH PRR reference subscription should be accepted");
        assert!(
            outbound_receiver.try_recv().is_err(),
            "new PRR subscription must wait behind the canceled in-flight provider subscribe"
        );

        let handled = polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            Some(&outbound),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        )
        .expect("late ack for canceled PRR subscription should drain provider subscribe");
        assert!(handled, "late subscribed ack must be handled");
        let PolyResearchReferenceOutboundCommand::SendText(provider_unsubscribe_frame) =
            outbound_receiver
                .try_recv()
                .expect("late ack for canceled PRR subscription should send provider unsubscribe")
        else {
            panic!("PRR late ack should send provider unsubscribe");
        };
        assert_eq!(
            provider_unsubscribe_frame,
            r#"{"action":"unsubscribe","subscription_id":"chainlink:btc"}"#
        );
        let PolyResearchReferenceOutboundCommand::SendText(eth_subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("new PRR subscription should send after canceled ack drains")
        else {
            panic!("new PRR subscription should send text after canceled ack drains");
        };
        assert_eq!(
            eth_subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["ETH/USD"]}}"#
        );

        let handled = polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            Some(&outbound),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:eth"}"#,
        )
        .expect("ETH ack should record active provider subscription id");
        assert!(handled, "ETH subscribed ack must be handled");
        assert_eq!(
            client
                .provider_subscription_ids
                .lock()
                .expect("provider subscription ids should not be poisoned")
                .get(&subscription_key(
                    "ETH",
                    "polyresearch_secondary",
                    "ETH/USD"
                ))
                .map(String::as_str),
            Some("chainlink:eth"),
            "late BTC ack must not be recorded as ETH provider subscription id"
        );
    }

    #[test]
    fn failed_bare_unsubscribe_keeps_canceled_prr_pending_until_late_ack() {
        let (mut client, _data_receiver) = fixture_client();
        let (outbound_sender, mut outbound_receiver) =
            tokio::sync::mpsc::unbounded_channel::<PolyResearchReferenceOutboundCommand>();
        let outbound = PolyResearchReferenceOutboundHandle::from_sender(outbound_sender);
        client.outbound = Some(outbound.clone());

        client
            .subscribe(reference_price_subscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect("BTC PRR reference subscription should be accepted");
        let PolyResearchReferenceOutboundCommand::SendText(btc_subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("first PRR subscription should send provider subscribe")
        else {
            panic!("first PRR subscription should send text");
        };
        assert_eq!(
            btc_subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["BTC/USD"]}}"#
        );

        client
            .outbound
            .as_ref()
            .expect("test outbound should be installed")
            .send_failures_remaining
            .store(1, Ordering::SeqCst);
        let error = client
            .unsubscribe(&reference_price_unsubscribe_cmd(
                "BTC",
                "polyresearch_primary",
                "BTC/USD",
            ))
            .expect_err("failed bare provider unsubscribe should be surfaced");

        assert!(
            error
                .to_string()
                .contains("PolyResearch reference outbound task unavailable"),
            "unexpected bare unsubscribe error: {error}"
        );
        assert_eq!(
            client
                .pending_provider_subscriptions
                .lock()
                .expect("provider subscription queue should not be poisoned")
                .front()
                .cloned(),
            Some(subscription_key("BTC", "polyresearch_primary", "BTC/USD")),
            "failed bare unsubscribe must preserve the canceled in-flight subscription until its ack drains"
        );

        client
            .subscribe(reference_price_subscribe_cmd(
                "ETH",
                "polyresearch_secondary",
                "ETH/USD",
            ))
            .expect("ETH PRR reference subscription should be accepted after cleanup failure");
        assert!(
            outbound_receiver.try_recv().is_err(),
            "new PRR subscription must wait behind the canceled in-flight provider subscribe"
        );

        let handled = polyresearch_reference_record_subscription_ack(
            &client.subscriptions,
            &client.pending_provider_subscriptions,
            &client.provider_subscription_ids,
            Some(&outbound),
            client.config.subscribe_ack_timeout_ms,
            r#"{"type":"subscribed","subscription_id":"chainlink:btc"}"#,
        )
        .expect("late ack for failed bare unsubscribe should drain canceled provider subscribe");
        assert!(handled, "late subscribed ack must be handled");
        let PolyResearchReferenceOutboundCommand::SendText(unsubscribe_frame) = outbound_receiver
            .try_recv()
            .expect("late ack for canceled PRR subscription should send provider unsubscribe")
        else {
            panic!("PRR late ack should send provider unsubscribe");
        };
        assert_eq!(
            unsubscribe_frame,
            r#"{"action":"unsubscribe","subscription_id":"chainlink:btc"}"#
        );
        let PolyResearchReferenceOutboundCommand::SendText(eth_subscribe_frame) = outbound_receiver
            .try_recv()
            .expect("new PRR subscription should send after canceled ack drains")
        else {
            panic!("new PRR subscription should send text after canceled ack drains");
        };
        assert_eq!(
            eth_subscribe_frame,
            r#"{"action":"subscribe","type":"chainlink","filters":{"feeds":["ETH/USD"]}}"#
        );
        assert_eq!(
            client
                .provider_subscription_ids
                .lock()
                .expect("provider subscription ids should not be poisoned")
                .get(&subscription_key(
                    "ETH",
                    "polyresearch_secondary",
                    "ETH/USD"
                ))
                .map(String::as_str),
            None,
            "late BTC ack must not be recorded as ETH provider subscription id"
        );
    }

    #[test]
    fn price_feed_frame_maps_to_reference_price_update() {
        const ASSET: &str = "BTC";
        const SOURCE_ID: &str = "polyresearch_primary";
        const PROVIDER_INSTRUMENT: &str = "BTC/USD";
        const FRAME_OBSERVED_TS_SEC: u64 = 1_774_672_588;
        const EXPECTED_OBSERVED_TS_MS: u64 = 1_774_672_588_000;
        const RECEIVED_TS_MS: u64 = 1_774_672_589_123;
        const EXPECTED_PRICE: f64 = 66_300.25;
        const EXPECTED_BID: f64 = 66_299.5;
        const EXPECTED_ASK: f64 = 66_301.0;

        let subscription = PolyResearchReferenceSubscription {
            asset: ASSET.to_string(),
            source_id: SOURCE_ID.to_string(),
            symbol: PROVIDER_INSTRUMENT.to_string(),
        };
        let frame = format!(
            r#"{{"type":"price_feed","feed":"{PROVIDER_INSTRUMENT}","timestamp":{FRAME_OBSERVED_TS_SEC},"data":{{"feed":"{PROVIDER_INSTRUMENT}","price":{EXPECTED_PRICE},"bid":{EXPECTED_BID},"ask":{EXPECTED_ASK},"timestamp":{FRAME_OBSERVED_TS_SEC}}}}}"#
        );

        let update =
            polyresearch_reference_update_from_price_frame(&subscription, &frame, RECEIVED_TS_MS)
                .expect("valid PRR price_feed frame should parse")
                .expect("matching PRR price_feed frame should emit an update");

        assert_eq!(update.asset(), ASSET);
        assert_eq!(update.source_id(), SOURCE_ID);
        assert_eq!(update.provider(), REFERENCE_PRICE_PROVIDER_KEY);
        assert_eq!(update.provider_instrument(), PROVIDER_INSTRUMENT);
        assert_eq!(update.price(), EXPECTED_PRICE);
        let quote = update
            .to_reference_quote()
            .expect("PRR reference update should convert to quote");
        assert_eq!(quote.bid(), Some(EXPECTED_BID));
        assert_eq!(quote.ask(), Some(EXPECTED_ASK));
        assert_eq!(update.observed_ts_ms(), EXPECTED_OBSERVED_TS_MS);
        assert_eq!(update.received_ts_ms(), RECEIVED_TS_MS);
    }

    #[test]
    fn untyped_control_frame_is_ignored() {
        let subscription = PolyResearchReferenceSubscription {
            asset: "BTC".to_string(),
            source_id: "polyresearch_primary".to_string(),
            symbol: "BTC/USD".to_string(),
        };

        let update = polyresearch_reference_update_from_price_frame(
            &subscription,
            r#"{"action":"pong"}"#,
            1_774_672_589_123,
        )
        .expect("untyped PRR control frames should be ignored without parse errors");

        assert!(
            update.is_none(),
            "untyped PRR control frames must not emit reference updates"
        );
    }

    fn reference_price_subscribe_cmd(
        asset: &str,
        source_id: &str,
        symbol: &str,
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
            REFERENCE_PRICE_SYMBOL_PARAM.to_string(),
            serde_json::json!(symbol),
        );

        SubscribeCustomData::new(
            Some(ClientId::from("polyresearch_reference")),
            None,
            ReferencePriceUpdate::data_type_for(asset, source_id, REFERENCE_PRICE_PROVIDER_KEY)
                .expect("reference price data type should build"),
            UUID4::new(),
            UnixNanos::default(),
            None,
            Some(params),
        )
    }

    fn reference_price_unsubscribe_cmd(
        asset: &str,
        source_id: &str,
        symbol: &str,
    ) -> UnsubscribeCustomData {
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
            REFERENCE_PRICE_SYMBOL_PARAM.to_string(),
            serde_json::json!(symbol),
        );

        UnsubscribeCustomData::new(
            Some(ClientId::from("polyresearch_reference")),
            None,
            ReferencePriceUpdate::data_type_for(asset, source_id, REFERENCE_PRICE_PROVIDER_KEY)
                .expect("reference price data type should build"),
            UUID4::new(),
            UnixNanos::default(),
            None,
            Some(params),
        )
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
    if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
        return Err(
            "polyresearch websocket_endpoint must be a credential-free wss URL".to_string(),
        );
    }
    if let Some(field) = polyresearch_credential_query_field(&url) {
        return Err(format!(
            "polyresearch websocket_endpoint must not contain credential query `{field}`; configure api_key separately"
        ));
    }
    if url.query().is_some() {
        return Err(
            "polyresearch websocket_endpoint must be a credential-free wss URL".to_string(),
        );
    }
    Ok(url)
}
