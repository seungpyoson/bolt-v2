use std::{
    any::Any,
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use chainlink_data_streams_report::{
    feed_id::ID,
    report::{Report, decode_full_report, v3::ReportDataV3},
};
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use nautilus_common::{
    cache::Cache,
    clients::DataClient,
    clock::Clock,
    msgbus::{publish_any, switchboard::get_custom_topic},
};
use nautilus_core::{Params, UnixNanos};
use nautilus_model::{
    data::{CustomData, CustomDataTrait, DataType, HasTsInit, ensure_custom_data_json_registered},
    identifiers::ClientId,
};
use nautilus_system::factories::{ClientConfig, DataClientFactory};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tokio::{
    net::TcpStream,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;

use crate::{
    clients::ReferenceDataClientParts,
    config::{ReferenceConfig, ReferenceVenueKind},
    secrets::{ResolvedChainlinkSecrets, SsmResolverSession, resolve_chainlink},
};

const CHAINLINK_CLIENT_NAME: &str = "CHAINLINK";
const CHAINLINK_VENUE_NAME_KEY: &str = "venue_name";
const CHAINLINK_INSTRUMENT_ID_KEY: &str = "instrument_id";
const CHAINLINK_WS_PATH: &str = "/api/v1/ws";
const CHAINLINK_FEED_VERSION_V3: u16 = 3;
const DEFAULT_WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MIN_WS_RECONNECT_INTERVAL: Duration = Duration::from_millis(1_000);
const MAX_WS_RECONNECT_INTERVAL: Duration = Duration::from_millis(10_000);
type ChainlinkWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ChainlinkWebSocketReport {
    report: Report,
}

fn chainlink_feed_version(feed_id: ID) -> u16 {
    u16::from_be_bytes([feed_id.0[0], feed_id.0[1]])
}

#[derive(Clone)]
pub struct ChainlinkReferenceClientConfig {
    pub shared: ChainlinkSharedRuntimeConfig,
    pub feeds: Vec<ChainlinkReferenceFeedConfig>,
}

impl std::fmt::Debug for ChainlinkReferenceClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainlinkReferenceClientConfig")
            .field("shared", &self.shared)
            .field("feeds", &self.feeds)
            .finish()
    }
}

impl ClientConfig for ChainlinkReferenceClientConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[derive(Debug, Clone)]
pub struct ChainlinkSharedRuntimeConfig {
    pub ws_url: String,
    pub ws_reconnect_alert_threshold: usize,
    pub secrets: ResolvedChainlinkSecrets,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainlinkReferenceFeedConfig {
    pub venue_name: String,
    pub instrument_id: String,
    pub feed_id: ID,
    pub price_scale: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainlinkOracleUpdate {
    pub venue_name: String,
    pub instrument_id: String,
    pub price: f64,
    pub round_id: String,
    pub updated_at_ms: u64,
    pub ts_init: UnixNanos,
}

impl HasTsInit for ChainlinkOracleUpdate {
    fn ts_init(&self) -> UnixNanos {
        self.ts_init
    }
}

impl CustomDataTrait for ChainlinkOracleUpdate {
    fn type_name(&self) -> &'static str {
        Self::type_name_static()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn ts_event(&self) -> UnixNanos {
        self.ts_init
    }

    fn to_json(&self) -> anyhow::Result<String> {
        serde_json::to_string(self).map_err(Into::into)
    }

    fn clone_arc(&self) -> Arc<dyn CustomDataTrait> {
        Arc::new(self.clone())
    }

    fn eq_arc(&self, other: &dyn CustomDataTrait) -> bool {
        other.as_any().downcast_ref::<Self>() == Some(self)
    }

    fn type_name_static() -> &'static str
    where
        Self: Sized,
    {
        "ChainlinkOracleUpdate"
    }

    fn from_json(value: Value) -> anyhow::Result<Arc<dyn CustomDataTrait>>
    where
        Self: Sized,
    {
        Ok(Arc::new(serde_json::from_value::<Self>(value)?))
    }
}

#[derive(Debug, Default)]
pub struct ChainlinkReferenceDataClientFactory;

impl DataClientFactory for ChainlinkReferenceDataClientFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        _cache: Rc<RefCell<Cache>>,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<ChainlinkReferenceClientConfig>()
            .context("ChainlinkReferenceDataClientFactory received wrong config type")?;

        Ok(Box::new(ChainlinkReferenceDataClient::new(
            ClientId::from(name),
            config.clone(),
        )))
    }

    fn name(&self) -> &str {
        CHAINLINK_CLIENT_NAME
    }

    fn config_type(&self) -> &str {
        "ChainlinkReferenceClientConfig"
    }
}

pub fn build_chainlink_reference_data_client(
    session: &SsmResolverSession,
    reference: &ReferenceConfig,
) -> Result<ReferenceDataClientParts, Box<dyn std::error::Error>> {
    let shared = reference.chainlink.as_ref().ok_or_else(|| {
        anyhow!("missing shared chainlink config for configured chainlink reference venues")
    })?;
    let secrets = resolve_chainlink(session, &shared.region, &shared.api_key, &shared.api_secret)?;
    build_chainlink_reference_data_client_with_secrets(reference, secrets)
}

pub fn build_chainlink_reference_data_client_with_secrets(
    reference: &ReferenceConfig,
    secrets: ResolvedChainlinkSecrets,
) -> Result<ReferenceDataClientParts, Box<dyn std::error::Error>> {
    ensure_custom_data_json_registered::<ChainlinkOracleUpdate>()?;

    let shared = reference.chainlink.as_ref().ok_or_else(|| {
        anyhow!("missing shared chainlink config for configured chainlink reference venues")
    })?;
    let ws_reconnect_alert_threshold = usize::try_from(shared.ws_reconnect_alert_threshold)
        .context("chainlink ws_reconnect_alert_threshold does not fit in usize")?;

    let feeds = reference
        .venues
        .iter()
        .filter(|venue| venue.kind == ReferenceVenueKind::Chainlink)
        .map(|venue| {
            let chainlink = venue.chainlink.as_ref().ok_or_else(|| {
                anyhow!(
                    "missing chainlink config for reference venue {} ({})",
                    venue.name,
                    venue.instrument_id
                )
            })?;

            let feed_id = ID::from_hex_str(&chainlink.feed_id)
                .with_context(|| format!("invalid chainlink feed_id for {}", venue.name))?;
            let version = chainlink_feed_version(feed_id);
            if version != CHAINLINK_FEED_VERSION_V3 {
                return Err(anyhow!(
                    "unsupported Chainlink Data Streams feed version {} for {} ({})",
                    version,
                    venue.name,
                    venue.instrument_id
                ));
            }

            Ok(ChainlinkReferenceFeedConfig {
                venue_name: venue.name.clone(),
                instrument_id: venue.instrument_id.clone(),
                feed_id,
                price_scale: chainlink.price_scale,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if feeds.is_empty() {
        return Err(anyhow!("no chainlink reference venues configured").into());
    }

    Ok((
        Box::new(ChainlinkReferenceDataClientFactory),
        Box::new(ChainlinkReferenceClientConfig {
            shared: ChainlinkSharedRuntimeConfig {
                ws_url: shared.ws_url.clone(),
                ws_reconnect_alert_threshold,
                secrets,
            },
            feeds,
        }),
    ))
}

pub fn chainlink_data_type_for_venue(venue_name: &str, instrument_id: &str) -> DataType {
    let _ = ensure_custom_data_json_registered::<ChainlinkOracleUpdate>();

    let mut metadata = Params::new();
    metadata.insert(
        CHAINLINK_VENUE_NAME_KEY.to_string(),
        Value::String(venue_name.to_string()),
    );
    metadata.insert(
        CHAINLINK_INSTRUMENT_ID_KEY.to_string(),
        Value::String(instrument_id.to_string()),
    );

    DataType::new(
        ChainlinkOracleUpdate::type_name_static(),
        Some(metadata),
        None,
    )
}

pub fn chainlink_topic_for_venue(
    venue_name: &str,
    instrument_id: &str,
) -> nautilus_common::msgbus::mstr::MStr<nautilus_common::msgbus::mstr::Topic> {
    let data_type = chainlink_data_type_for_venue(venue_name, instrument_id);
    get_custom_topic(&data_type)
}

#[derive(Debug)]
struct ChainlinkReferenceDataClient {
    client_id: ClientId,
    shared: ChainlinkSharedRuntimeConfig,
    connected: bool,
    subscriptions: BTreeMap<String, DataType>,
    feed_configs_by_topic: BTreeMap<String, ChainlinkReferenceFeedConfig>,
    worker: Option<ChainlinkWorkerHandle>,
}

#[derive(Debug)]
struct ChainlinkWorkerHandle {
    cancellation: CancellationToken,
    task: JoinHandle<()>,
}

impl ChainlinkReferenceDataClient {
    fn new(client_id: ClientId, config: ChainlinkReferenceClientConfig) -> Self {
        let feed_configs_by_topic = config
            .feeds
            .iter()
            .map(|feed| {
                let data_type =
                    chainlink_data_type_for_venue(&feed.venue_name, &feed.instrument_id);
                (data_type.topic().to_string(), feed.clone())
            })
            .collect();

        Self {
            client_id,
            shared: config.shared,
            connected: false,
            subscriptions: BTreeMap::new(),
            feed_configs_by_topic,
            worker: None,
        }
    }

    fn selected_feeds(&self) -> Vec<(ChainlinkReferenceFeedConfig, DataType)> {
        self.subscriptions
            .iter()
            .filter_map(|(topic, data_type)| {
                self.feed_configs_by_topic
                    .get(topic)
                    .cloned()
                    .map(|feed| (feed, data_type.clone()))
            })
            .collect()
    }

    fn restart_worker(&mut self) -> Result<()> {
        self.stop_worker();
        if !self.connected {
            return Ok(());
        }

        let selected_feeds = self.selected_feeds();
        if selected_feeds.is_empty() {
            return Ok(());
        }

        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_chainlink_stream_worker(
            self.shared.clone(),
            selected_feeds,
            cancellation.clone(),
        ));
        self.worker = Some(ChainlinkWorkerHandle { cancellation, task });
        Ok(())
    }

    fn stop_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.cancellation.cancel();
            worker.task.abort();
        }
    }
}

#[async_trait(?Send)]
impl DataClient for ChainlinkReferenceDataClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<nautilus_model::identifiers::Venue> {
        None
    }

    fn start(&mut self) -> anyhow::Result<()> {
        self.connected = true;
        self.restart_worker()
    }

    fn stop(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        self.stop_worker();
        Ok(())
    }

    fn reset(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        self.stop_worker();
        Ok(())
    }

    fn dispose(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        self.stop_worker();
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
        self.restart_worker()
    }

    async fn disconnect(&mut self) -> anyhow::Result<()> {
        self.connected = false;
        self.stop_worker();
        Ok(())
    }

    fn subscribe(
        &mut self,
        cmd: &nautilus_common::messages::data::SubscribeCustomData,
    ) -> anyhow::Result<()> {
        let topic = cmd.data_type.topic().to_string();
        self.subscriptions.insert(topic, cmd.data_type.clone());
        self.restart_worker()
    }

    fn unsubscribe(
        &mut self,
        cmd: &nautilus_common::messages::data::UnsubscribeCustomData,
    ) -> anyhow::Result<()> {
        let topic = cmd.data_type.topic().to_string();
        self.subscriptions.remove(&topic);
        self.restart_worker()
    }
}

async fn run_chainlink_stream_worker(
    shared: ChainlinkSharedRuntimeConfig,
    selected_feeds: Vec<(ChainlinkReferenceFeedConfig, DataType)>,
    cancellation: CancellationToken,
) {
    let mut feed_routes = BTreeMap::new();
    let mut feed_ids = Vec::with_capacity(selected_feeds.len());
    for (feed, data_type) in selected_feeds {
        let feed_key = feed.feed_id.to_hex_string();
        feed_routes.insert(feed_key.clone(), (feed.clone(), data_type));
        feed_ids.push(feed.feed_id);
    }

    run_chainlink_stream_loop(
        feed_ids,
        cancellation,
        shared.ws_reconnect_alert_threshold,
        MIN_WS_RECONNECT_INTERVAL,
        {
            let shared = shared.clone();
            move |feed_ids| {
                let shared = shared.clone();
                async move { connect_chainlink_stream_session(&shared, &feed_ids).await }
            }
        },
        |report| publish_chainlink_report(&feed_routes, report),
    )
    .await;
}

#[async_trait]
trait ChainlinkStreamSession: Send {
    async fn read_report(&mut self) -> Result<ChainlinkWebSocketReport>;
    async fn close_session(&mut self);
}

struct RawChainlinkStreamSession {
    stream: ChainlinkWsStream,
}

#[async_trait]
impl ChainlinkStreamSession for RawChainlinkStreamSession {
    async fn read_report(&mut self) -> Result<ChainlinkWebSocketReport> {
        loop {
            match self.stream.next().await {
                Some(Ok(Message::Binary(data))) => {
                    return serde_json::from_slice::<ChainlinkWebSocketReport>(&data)
                        .context("failed to parse Chainlink websocket binary report");
                }
                Some(Ok(Message::Text(_))) | Some(Ok(Message::Pong(_))) => continue,
                Some(Ok(Message::Ping(payload))) => {
                    self.stream
                        .send(Message::Pong(payload))
                        .await
                        .context("failed to send websocket pong")?;
                }
                Some(Ok(Message::Close(frame))) => {
                    return Err(anyhow!("chainlink websocket closed: {frame:?}"));
                }
                Some(Ok(_)) => continue,
                Some(Err(error)) => {
                    return Err(anyhow!("chainlink websocket read failed: {error}"));
                }
                None => return Err(anyhow!("chainlink websocket stream ended")),
            }
        }
    }

    async fn close_session(&mut self) {
        let _ = self.stream.close(None).await;
    }
}

async fn run_chainlink_stream_loop<Connect, ConnectFut, Session, Handle>(
    feed_ids: Vec<ID>,
    cancellation: CancellationToken,
    reconnect_alert_threshold: usize,
    base_reconnect_delay: Duration,
    mut connect_session: Connect,
    mut handle_report: Handle,
) where
    Connect: FnMut(Vec<ID>) -> ConnectFut + Send,
    ConnectFut: std::future::Future<Output = Result<Session>> + Send,
    Session: ChainlinkStreamSession + 'static,
    Handle: FnMut(ChainlinkWebSocketReport) -> Result<()> + Send,
{
    let reconnect_alert_threshold = reconnect_alert_threshold.max(1);
    let mut consecutive_failures = 0usize;
    let mut reconnect_delay = base_reconnect_delay;
    let mut last_full_reports = BTreeMap::<String, String>::new();

    loop {
        if cancellation.is_cancelled() {
            return;
        }

        let mut session = match connect_session(feed_ids.clone()).await {
            Ok(session) => {
                consecutive_failures = 0;
                reconnect_delay = base_reconnect_delay;
                session
            }
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                log::warn!("failed to connect Chainlink Data Streams websocket: {error:#}");
                log_reconnect_alert_threshold_exceeded(
                    consecutive_failures,
                    reconnect_alert_threshold,
                );
                if wait_for_reconnect(reconnect_delay, &cancellation).await {
                    return;
                }
                reconnect_delay =
                    std::cmp::min(reconnect_delay.saturating_mul(2), MAX_WS_RECONNECT_INTERVAL);
                continue;
            }
        };

        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    session.close_session().await;
                    return;
                }
                result = session.read_report() => {
                    match result {
                        Ok(report) => {
                            let feed_key = report.report.feed_id.to_hex_string();
                            if last_full_reports
                                .get(&feed_key)
                                .is_some_and(|last| last == &report.report.full_report)
                            {
                                continue;
                            }
                            last_full_reports
                                .insert(feed_key, report.report.full_report.clone());
                            if let Err(error) = handle_report(report) {
                                log::warn!("failed to process Chainlink Data Streams report: {error:#}");
                            }
                        }
                        Err(error) => {
                            log::warn!("Chainlink Data Streams session failed: {error:#}");
                            session.close_session().await;
                            break;
                        }
                    }
                }
            }
        }

        consecutive_failures = consecutive_failures.saturating_add(1);
        log_reconnect_alert_threshold_exceeded(consecutive_failures, reconnect_alert_threshold);
        if wait_for_reconnect(reconnect_delay, &cancellation).await {
            return;
        }
        reconnect_delay =
            std::cmp::min(reconnect_delay.saturating_mul(2), MAX_WS_RECONNECT_INTERVAL);
    }
}

fn log_reconnect_alert_threshold_exceeded(
    consecutive_failures: usize,
    reconnect_alert_threshold: usize,
) {
    // The Chainlink lane intentionally reconnects indefinitely; this threshold only
    // controls when repeated failures escalate from warn-level noise to error-level noise.
    if consecutive_failures >= reconnect_alert_threshold {
        log::error!(
            "Chainlink Data Streams hit {} consecutive connection failure(s); continuing reconnect loop",
            consecutive_failures
        );
    }
}

async fn wait_for_reconnect(delay: Duration, cancellation: &CancellationToken) -> bool {
    tokio::select! {
        _ = cancellation.cancelled() => true,
        _ = sleep(delay) => false,
    }
}

async fn connect_chainlink_stream_session(
    shared: &ChainlinkSharedRuntimeConfig,
    feed_ids: &[ID],
) -> Result<RawChainlinkStreamSession> {
    let origins = parse_chainlink_ws_origins(&shared.ws_url)?;
    let feed_ids_joined = feed_ids
        .iter()
        .map(|feed_id| feed_id.to_hex_string())
        .collect::<Vec<_>>()
        .join(",");
    let path = format!("{CHAINLINK_WS_PATH}?feedIDs={feed_ids_joined}");
    let mut last_error = None;

    for origin in origins {
        let timestamp = current_timestamp_ms()?;
        let mut request = format!("{origin}{path}")
            .into_client_request()
            .with_context(|| format!("failed to build Chainlink websocket request for {origin}"))?;
        let headers = generate_chainlink_auth_headers(
            "GET",
            &path,
            b"",
            &shared.secrets.api_key,
            &shared.secrets.api_secret,
            timestamp,
        )?;
        for (name, value) in &headers {
            request.headers_mut().insert(name, value.clone());
        }

        match timeout(DEFAULT_WS_CONNECT_TIMEOUT, connect_async(request)).await {
            Ok(Ok((stream, _))) => return Ok(RawChainlinkStreamSession { stream }),
            Ok(Err(error)) => {
                last_error = Some(anyhow!(
                    "failed to connect Chainlink websocket origin {origin}: {error}"
                ));
            }
            Err(_) => {
                last_error = Some(anyhow!(
                    "timed out connecting Chainlink websocket origin {origin} after {:?}",
                    DEFAULT_WS_CONNECT_TIMEOUT
                ));
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("failed to connect any Chainlink websocket origin")))
}

pub(crate) fn parse_chainlink_ws_origins(ws_url: &str) -> Result<Vec<String>> {
    let origins = ws_url
        .split(',')
        .map(str::trim)
        .filter(|origin| !origin.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if origins.is_empty() {
        return Err(anyhow!("chainlink ws_url must contain at least one origin"));
    }

    for origin in &origins {
        let request = origin
            .as_str()
            .into_client_request()
            .with_context(|| format!("invalid Chainlink websocket origin \"{origin}\""))?;
        if request.uri().scheme_str() != Some("wss") {
            return Err(anyhow!(
                "each origin must start with wss://, got \"{origin}\""
            ));
        }
        if request.uri().authority().is_none() {
            return Err(anyhow!("each origin must include a host, got \"{origin}\""));
        }
    }

    Ok(origins)
}

fn generate_chainlink_auth_headers(
    method: &str,
    path: &str,
    body: &[u8],
    api_key: &str,
    api_secret: &str,
    timestamp: u128,
) -> Result<
    Vec<(
        tokio_tungstenite::tungstenite::http::header::HeaderName,
        tokio_tungstenite::tungstenite::http::HeaderValue,
    )>,
> {
    let body_hash = hex::encode(Sha256::digest(body));
    let signing_payload = format!("{method} {path} {body_hash} {api_key} {timestamp}");
    let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes())
        .context("invalid Chainlink API secret for HMAC")?;
    mac.update(signing_payload.as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());

    Ok(vec![
        (
            tokio_tungstenite::tungstenite::http::header::HeaderName::from_static("authorization"),
            tokio_tungstenite::tungstenite::http::HeaderValue::from_str(api_key)
                .context("invalid Chainlink authorization header value")?,
        ),
        (
            tokio_tungstenite::tungstenite::http::header::HeaderName::from_static(
                "x-authorization-timestamp",
            ),
            tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&timestamp.to_string())
                .context("invalid Chainlink authorization timestamp header value")?,
        ),
        (
            tokio_tungstenite::tungstenite::http::header::HeaderName::from_static(
                "x-authorization-signature-sha256",
            ),
            tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&signature)
                .context("invalid Chainlink authorization signature header value")?,
        ),
    ])
}

fn current_timestamp_ms() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before UNIX_EPOCH")?
        .as_millis())
}

fn publish_chainlink_report(
    feed_routes: &BTreeMap<String, (ChainlinkReferenceFeedConfig, DataType)>,
    report: ChainlinkWebSocketReport,
) -> Result<()> {
    let feed_id = report.report.feed_id.to_hex_string();
    let (feed, data_type) = match feed_routes.get(&feed_id) {
        Some(route) => route,
        None => return Ok(()),
    };

    let payload_hex = report.report.full_report.trim_start_matches("0x");
    let payload = hex::decode(payload_hex)
        .with_context(|| format!("invalid fullReport hex for {feed_id}"))?;
    let (report_context, report_blob) = decode_full_report(&payload)
        .with_context(|| format!("invalid fullReport payload for {feed_id}"))?;
    let epoch_and_round = extract_epoch_and_round(&report_context)
        .with_context(|| format!("invalid Chainlink reportContext for {feed_id}"))?;

    match chainlink_feed_version(feed.feed_id) {
        CHAINLINK_FEED_VERSION_V3 => publish_v3_report(
            feed,
            data_type,
            &report.report,
            &report_blob,
            epoch_and_round,
        ),
        version => Err(anyhow!(
            "unsupported Chainlink Data Streams feed version {} for {} ({})",
            version,
            feed.venue_name,
            feed.instrument_id
        )),
    }
}

fn extract_epoch_and_round(report_context: &[[u8; 32]]) -> Result<u64> {
    let epoch_and_round_bytes = report_context
        .get(1)
        .ok_or_else(|| anyhow!("reportContext must include epoch-and-round slot"))?;
    let mut encoded = [0_u8; 8];
    encoded[2..].copy_from_slice(&epoch_and_round_bytes[26..32]);
    Ok(u64::from_be_bytes(encoded))
}

fn publish_v3_report(
    feed: &ChainlinkReferenceFeedConfig,
    data_type: &DataType,
    report: &chainlink_data_streams_report::report::Report,
    report_blob: &[u8],
    epoch_and_round: u64,
) -> Result<()> {
    let report_data = ReportDataV3::decode(report_blob)
        .with_context(|| format!("failed to decode v3 report for {}", feed.venue_name))?;
    let updated_at_ms = u64::try_from(report.observations_timestamp)
        .context("observationsTimestamp does not fit in u64")?
        .saturating_mul(1_000);
    let observed_ts_ms = u64::try_from(current_timestamp_ms()?)
        .context("local observed timestamp does not fit in u64")?;
    let price =
        scale_decimal_string_to_f64(&report_data.benchmark_price.to_string(), feed.price_scale)?;

    let custom = CustomData::new(
        Arc::new(ChainlinkOracleUpdate {
            venue_name: feed.venue_name.clone(),
            instrument_id: feed.instrument_id.clone(),
            price,
            round_id: epoch_and_round.to_string(),
            updated_at_ms,
            ts_init: UnixNanos::from(observed_ts_ms.saturating_mul(1_000_000)),
        }),
        data_type.clone(),
    );
    publish_any(get_custom_topic(data_type), &custom);
    Ok(())
}

fn scale_decimal_string_to_f64(raw: &str, scale: u8) -> Result<f64> {
    let (negative, digits) = raw
        .strip_prefix('-')
        .map(|trimmed| (true, trimmed))
        .unwrap_or((false, raw));
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(anyhow!("invalid decimal digits: {raw}"));
    }

    let scale = usize::from(scale);
    let scaled = if scale == 0 {
        digits.to_string()
    } else if digits.len() <= scale {
        format!("0.{}{}", "0".repeat(scale - digits.len()), digits)
    } else {
        format!(
            "{}.{}",
            &digits[..digits.len() - scale],
            &digits[digits.len() - scale..]
        )
    };

    let signed = if negative {
        format!("-{scaled}")
    } else {
        scaled
    };
    signed
        .parse::<f64>()
        .with_context(|| format!("failed to parse scaled decimal {signed}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::{
        collections::{BTreeSet, VecDeque},
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio::time::timeout;

    struct FakeSession {
        events: VecDeque<Result<ChainlinkWebSocketReport>>,
    }

    #[async_trait]
    impl ChainlinkStreamSession for FakeSession {
        async fn read_report(&mut self) -> Result<ChainlinkWebSocketReport> {
            self.events
                .pop_front()
                .unwrap_or_else(|| Err(anyhow!("no more fake events")))
        }

        async fn close_session(&mut self) {}
    }

    fn sample_report(feed_id: ID, observations_timestamp: usize) -> ChainlinkWebSocketReport {
        ChainlinkWebSocketReport {
            report: Report {
                feed_id,
                valid_from_timestamp: observations_timestamp,
                observations_timestamp,
                full_report: "0x00".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn stream_loop_reconnects_after_terminal_read_error() {
        let feed_id =
            ID::from_hex_str("0x00037da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439")
                .unwrap();
        let sessions = Arc::new(Mutex::new(VecDeque::from([
            Ok(FakeSession {
                events: VecDeque::from([Err(anyhow!("first session failed"))]),
            }),
            Ok(FakeSession {
                events: VecDeque::from([
                    Ok(sample_report(feed_id, 1)),
                    Err(anyhow!("stop after report")),
                ]),
            }),
        ])));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let cancellation = CancellationToken::new();

        run_chainlink_stream_loop(
            vec![feed_id],
            cancellation.clone(),
            1,
            Duration::from_millis(0),
            {
                let sessions = Arc::clone(&sessions);
                move |_| {
                    let sessions = Arc::clone(&sessions);
                    async move {
                        sessions
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or_else(|| Err(anyhow!("no more fake sessions")))
                    }
                }
            },
            {
                let seen = Arc::clone(&seen);
                move |report| {
                    seen.lock()
                        .unwrap()
                        .push(report.report.feed_id.to_hex_string());
                    cancellation.cancel();
                    Ok(())
                }
            },
        )
        .await;

        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn stream_loop_drops_identical_report_replays() {
        let feed_id =
            ID::from_hex_str("0x00037da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439")
                .unwrap();
        let full_report = "0xdeadbeef".to_string();
        let sessions = Arc::new(Mutex::new(VecDeque::from([Ok(FakeSession {
            events: VecDeque::from([
                Ok(ChainlinkWebSocketReport {
                    report: Report {
                        feed_id,
                        valid_from_timestamp: 1,
                        observations_timestamp: 1,
                        full_report: full_report.clone(),
                    },
                }),
                Ok(ChainlinkWebSocketReport {
                    report: Report {
                        feed_id,
                        valid_from_timestamp: 1,
                        observations_timestamp: 1,
                        full_report: full_report.clone(),
                    },
                }),
                Err(anyhow!("stop after duplicate replay")),
            ]),
        })])));
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let cancellation = CancellationToken::new();

        run_chainlink_stream_loop(
            vec![feed_id],
            cancellation.clone(),
            1,
            Duration::from_millis(0),
            {
                let sessions = Arc::clone(&sessions);
                move |_| {
                    let sessions = Arc::clone(&sessions);
                    async move {
                        sessions
                            .lock()
                            .unwrap()
                            .pop_front()
                            .unwrap_or_else(|| Err(anyhow!("no more fake sessions")))
                    }
                }
            },
            {
                let delivered = Arc::clone(&delivered);
                move |report| {
                    delivered
                        .lock()
                        .unwrap()
                        .push(report.report.full_report.clone());
                    cancellation.cancel();
                    Ok(())
                }
            },
        )
        .await;

        assert_eq!(delivered.lock().unwrap().len(), 1);
    }

    #[test]
    fn extracts_epoch_and_round_from_report_context() {
        let mut slot = [0_u8; 32];
        slot[26..28].copy_from_slice(&0x0102_u16.to_be_bytes());
        slot[28..32].copy_from_slice(&0x03040506_u32.to_be_bytes());

        let value = extract_epoch_and_round(&[[0_u8; 32], slot, [0_u8; 32]]).unwrap();

        assert_eq!(value, 0x010203040506);
    }

    #[test]
    fn scales_decimal_string_to_f64_with_zero_scale() {
        let value = scale_decimal_string_to_f64("12345", 0).unwrap();
        assert_eq!(value, 12_345.0);
    }

    #[test]
    fn scales_decimal_string_to_f64_when_digits_are_shorter_than_scale() {
        let value = scale_decimal_string_to_f64("123", 5).unwrap();
        assert!(
            (value - 0.00123).abs() < 1e-12,
            "unexpected scaled value: {value}"
        );
    }

    #[test]
    fn scales_decimal_string_to_f64_for_negative_values() {
        let value = scale_decimal_string_to_f64("-12345", 2).unwrap();
        assert!(
            (value + 123.45).abs() < 1e-12,
            "unexpected scaled value: {value}"
        );
    }

    #[test]
    fn rejects_invalid_decimal_strings() {
        let error = scale_decimal_string_to_f64("12.34", 2)
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid decimal digits"));
    }

    #[tokio::test]
    #[ignore = "requires config/live.toml with resolvable Chainlink testnet credentials"]
    async fn live_chainlink_stream_smoke_works_with_generated_runtime_config() {
        let config_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/live.toml");
        let config = crate::config::Config::load(&config_path).expect("runtime config should load");
        let shared = config
            .reference
            .chainlink
            .as_ref()
            .expect("runtime config should include reference.chainlink");
        let session = crate::secrets::SsmResolverSession::new()
            .expect("SsmResolverSession should build for chainlink integration smoke");
        let resolved = crate::secrets::resolve_chainlink(
            &session,
            &shared.region,
            &shared.api_key,
            &shared.api_secret,
        )
        .expect("chainlink secrets should resolve");
        let (_, client_config_any) =
            build_chainlink_reference_data_client_with_secrets(&config.reference, resolved)
                .expect("chainlink client config should build");
        let client_config = client_config_any
            .as_any()
            .downcast_ref::<ChainlinkReferenceClientConfig>()
            .expect("chainlink client config should downcast");
        let expected_reports = client_config.feeds.len();
        let seen: Arc<Mutex<BTreeSet<String>>> = Arc::new(Mutex::new(BTreeSet::new()));
        let cancellation = CancellationToken::new();

        timeout(
            Duration::from_secs(20),
            run_chainlink_stream_loop(
                client_config
                    .feeds
                    .iter()
                    .map(|feed| feed.feed_id)
                    .collect(),
                cancellation.clone(),
                client_config.shared.ws_reconnect_alert_threshold,
                MIN_WS_RECONNECT_INTERVAL,
                {
                    let shared = client_config.shared.clone();
                    move |feed_ids| {
                        let shared = shared.clone();
                        async move { connect_chainlink_stream_session(&shared, &feed_ids).await }
                    }
                },
                {
                    let seen = Arc::clone(&seen);
                    move |report| {
                        let mut seen_guard = seen.lock().unwrap();
                        seen_guard.insert(report.report.feed_id.to_hex_string());
                        if seen_guard.len() >= expected_reports {
                            cancellation.cancel();
                        }
                        Ok(())
                    }
                },
            ),
        )
        .await
        .expect("live chainlink smoke should finish within timeout");

        assert_eq!(seen.lock().unwrap().len(), expected_reports);
    }
}
