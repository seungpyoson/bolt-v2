//! Chainlink Data Streams strike (price-to-beat) source for bolt-v3.
//!
//! This is a point-in-time NT [`DataClient`] — NOT a continuous stream. Its
//! only job is to deliver the Chainlink Data Streams benchmark price for a
//! binary up/down window: the strike at the window-open Unix timestamp and the
//! settlement reference at the window-close Unix timestamp, each fetched ONCE
//! per window via the timestamped REST endpoint and delivered as a single NT
//! [`IndexPriceUpdate`] on the resolution instrument.
//!
//! The window timestamp is an INPUT supplied by the strategy via the standard
//! NT subscribe-command `params` map (`window_open_unix_seconds` for strike,
//! `window_close_unix_seconds` for settlement); it is never a string literal in
//! this code. The feed-id <-> instrument-id mapping comes from TOML
//! (`feed_bindings`). Credentials are resolved from SSM by the provider binding
//! and embedded into [`ChainlinkStrikeSourceConfig`] as zeroizing material; this
//! module never logs or prints secret values.
//!
//! Protocol logic (HMAC auth, signed request URL, V3 `fullReport` decode) is
//! reused from the sibling [`super::auth`] / [`super::report`] modules.

use std::{
    any::Any,
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use nautilus_common::{
    cache::CacheView,
    clients::DataClient,
    clock::Clock,
    factories::{ClientConfig, DataClientFactory},
    live::runner::get_data_event_sender,
    live::runtime::get_runtime,
    messages::{
        DataEvent,
        data::{SubscribeIndexPrices, UnsubscribeIndexPrices},
    },
};
use nautilus_core::{Params, UnixNanos, consts::NAUTILUS_USER_AGENT};
use nautilus_model::{
    data::{Data, IndexPriceUpdate},
    identifiers::{ClientId, InstrumentId},
    types::Price,
};
use nautilus_network::http::{HttpClient, USER_AGENT};
use zeroize::Zeroizing;

use super::{
    CHAINLINK_REPORT_MILLISECONDS_PER_SECOND, ChainlinkDataStreamsReportApiResponse,
    DecodedPriceToBeatReport, PriceToBeatReportBinding, chainlink_data_streams_auth_headers,
    chainlink_data_streams_credentials, chainlink_data_streams_report_request_url,
    decode_price_to_beat_report,
};

/// NT subscribe-command `params` key carrying the window-open Unix timestamp
/// (seconds) for the point-in-time strike lookup.
pub const STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM: &str = "window_open_unix_seconds";
/// NT subscribe-command `params` key carrying the window-close Unix timestamp
/// (seconds) for the point-in-time settlement reference lookup.
pub const SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM: &str = "window_close_unix_seconds";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainlinkReportBoundaryKind {
    WindowOpenStrike,
    WindowCloseSettlement,
}

impl ChainlinkReportBoundaryKind {
    fn label(self) -> &'static str {
        match self {
            Self::WindowOpenStrike => "window-open strike",
            Self::WindowCloseSettlement => "window-close settlement",
        }
    }

    fn boundary_label(self) -> &'static str {
        match self {
            Self::WindowOpenStrike => "window-open boundary",
            Self::WindowCloseSettlement => "window-close boundary",
        }
    }

    fn param_key(self) -> &'static str {
        match self {
            Self::WindowOpenStrike => STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM,
            Self::WindowCloseSettlement => SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChainlinkReportBoundary {
    kind: ChainlinkReportBoundaryKind,
    unix_seconds: u64,
}

impl ChainlinkReportBoundary {
    fn new(kind: ChainlinkReportBoundaryKind, unix_seconds: u64) -> Self {
        Self { kind, unix_seconds }
    }
}

const CHAINLINK_STRIKE_SOURCE_FACTORY_NAME: &str = "CHAINLINK_DATA_STREAMS";
const CHAINLINK_STRIKE_SOURCE_CONFIG_TYPE: &str = "ChainlinkStrikeSourceConfig";

/// One TOML-driven feed binding: a Chainlink Data Streams feed id mapped to
/// the NT resolution instrument the strike is published on, plus the V3 report
/// schema/scale and the NT price precision used to build the [`Price`].
#[derive(Clone)]
pub struct ChainlinkStrikeFeedBinding {
    pub feed_id: String,
    pub instrument_id: InstrumentId,
    pub report_schema_version: u64,
    pub report_decimal_scale: u64,
    pub price_precision: u8,
}

impl std::fmt::Debug for ChainlinkStrikeFeedBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainlinkStrikeFeedBinding")
            .field("feed_id", &self.feed_id)
            .field("instrument_id", &self.instrument_id)
            .field("report_schema_version", &self.report_schema_version)
            .field("report_decimal_scale", &self.report_decimal_scale)
            .field("price_precision", &self.price_precision)
            .finish()
    }
}

/// Resolved runtime configuration for the strike source.
///
/// Holds the REST endpoint shape, per-feed bindings, and the SSM-resolved
/// credentials. The credentials are wrapped in [`Zeroizing`] and excluded from
/// the [`std::fmt::Debug`] output so secret bytes are scrubbed on drop and
/// never reach logs.
#[derive(Clone)]
pub struct ChainlinkStrikeSourceConfig {
    pub rest_base_url: String,
    pub report_endpoint_path: String,
    pub http_timeout_secs: u64,
    pub feed_bindings: Vec<ChainlinkStrikeFeedBinding>,
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
}

impl std::fmt::Debug for ChainlinkStrikeSourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChainlinkStrikeSourceConfig")
            .field("rest_base_url", &self.rest_base_url)
            .field("report_endpoint_path", &self.report_endpoint_path)
            .field("http_timeout_secs", &self.http_timeout_secs)
            .field("feed_bindings", &self.feed_bindings)
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .finish()
    }
}

impl ClientConfig for ChainlinkStrikeSourceConfig {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// NT [`DataClientFactory`] that constructs the Chainlink strike source.
///
/// A unit struct constructed directly at the provider binding
/// (`Box::new(ChainlinkStrikeSourceFactory)`); it deliberately does not derive
/// `Default` so the bolt-v3 production surface stays clear of the legacy
/// default-construction fence. The `DataClientFactory` trait only requires
/// `Debug`.
#[derive(Debug)]
pub struct ChainlinkStrikeSourceFactory;

impl DataClientFactory for ChainlinkStrikeSourceFactory {
    fn create(
        &self,
        name: &str,
        config: &dyn ClientConfig,
        _cache: CacheView,
        _clock: Rc<RefCell<dyn Clock>>,
    ) -> anyhow::Result<Box<dyn DataClient>> {
        let config = config
            .as_any()
            .downcast_ref::<ChainlinkStrikeSourceConfig>()
            .ok_or_else(|| {
                anyhow::anyhow!("ChainlinkStrikeSourceFactory received wrong config type")
            })?;
        Ok(Box::new(ChainlinkStrikeSourceClient::new(
            ClientId::from(name),
            config.clone(),
        )?))
    }

    fn name(&self) -> &str {
        CHAINLINK_STRIKE_SOURCE_FACTORY_NAME
    }

    fn config_type(&self) -> &str {
        CHAINLINK_STRIKE_SOURCE_CONFIG_TYPE
    }
}

/// Point-in-time strike source. Implements NT [`DataClient`]; on
/// `subscribe_index_prices` it performs ONE timestamped fetch for the
/// requested resolution instrument and emits a single [`IndexPriceUpdate`].
/// There is no background stream, no reconnect loop, and no unsubscribe-side
/// teardown beyond marking the client disconnected.
#[derive(Debug)]
struct ChainlinkStrikeSourceClient {
    client_id: ClientId,
    config: ChainlinkStrikeSourceConfig,
    connected: bool,
    data_sender: tokio::sync::mpsc::UnboundedSender<DataEvent>,
    /// Resolution instruments whose strike fetch is currently in flight. The
    /// strategy re-issues the strike subscribe on every selection-retry tick
    /// (unsubscribe-then-resubscribe to defeat NT's per-instrument index-price
    /// dedup), so without this guard a stalled REST call would let those retries
    /// stack concurrent fetches against the live endpoint. At most one fetch per
    /// instrument runs until it finishes; the spawned task clears its entry on
    /// completion. Shared across the spawned task, hence `Arc<Mutex<..>>`.
    in_flight: Arc<Mutex<HashSet<InstrumentId>>>,
}

impl ChainlinkStrikeSourceClient {
    fn new(client_id: ClientId, config: ChainlinkStrikeSourceConfig) -> anyhow::Result<Self> {
        Ok(Self {
            client_id,
            config,
            connected: false,
            data_sender: get_data_event_sender(),
            in_flight: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    fn feed_binding_for(&self, instrument_id: InstrumentId) -> Option<&ChainlinkStrikeFeedBinding> {
        self.config
            .feed_bindings
            .iter()
            .find(|binding| binding.instrument_id == instrument_id)
    }

    /// Admits a strike fetch for `instrument_id` only when none is already in
    /// flight, recording it as in flight. Returns `true` when the caller may
    /// spawn the fetch, `false` when one is already running (skip — the bounded
    /// selection-retry cadence re-issues after it finishes). Fails closed
    /// (returns `false`, no fetch) if the shared guard lock is poisoned.
    fn begin_strike_fetch_if_idle(
        in_flight: &Arc<Mutex<HashSet<InstrumentId>>>,
        instrument_id: InstrumentId,
    ) -> bool {
        match in_flight.lock() {
            Ok(mut in_flight) => in_flight.insert(instrument_id),
            Err(_) => false,
        }
    }

    /// Clears the in-flight marker for `instrument_id` once its fetch completes
    /// (success or failure), so the next retry tick may re-issue.
    fn finish_strike_fetch(
        in_flight: &Arc<Mutex<HashSet<InstrumentId>>>,
        instrument_id: InstrumentId,
    ) {
        if let Ok(mut in_flight) = in_flight.lock() {
            in_flight.remove(&instrument_id);
        }
    }
}

#[async_trait::async_trait(?Send)]
impl DataClient for ChainlinkStrikeSourceClient {
    fn client_id(&self) -> ClientId {
        self.client_id
    }

    fn venue(&self) -> Option<nautilus_model::identifiers::Venue> {
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

    fn subscribe_index_prices(&mut self, cmd: SubscribeIndexPrices) -> anyhow::Result<()> {
        let binding = self.feed_binding_for(cmd.instrument_id).ok_or_else(|| {
            anyhow::anyhow!(
                "Chainlink strike source has no feed binding for resolution instrument {}",
                cmd.instrument_id
            )
        })?;
        let report_boundary = requested_report_boundary(cmd.params.as_ref(), cmd.instrument_id)?;
        log::info!(
            "Chainlink strike source {} received {} subscribe for {} at {}={}",
            self.client_id,
            report_boundary.kind.label(),
            cmd.instrument_id,
            report_boundary.kind.param_key(),
            report_boundary.unix_seconds
        );

        let request = StrikeFetchRequest {
            rest_base_url: self.config.rest_base_url.clone(),
            report_endpoint_path: self.config.report_endpoint_path.clone(),
            http_timeout_secs: self.config.http_timeout_secs,
            api_key: self.config.api_key.clone(),
            api_secret: self.config.api_secret.clone(),
            feed_id: binding.feed_id.clone(),
            instrument_id: binding.instrument_id,
            report_schema_version: binding.report_schema_version,
            report_decimal_scale: binding.report_decimal_scale,
            price_precision: binding.price_precision,
            report_boundary,
        };
        // Admit at most one in-flight fetch per resolution instrument: the
        // strategy re-issues this subscribe on every retry tick while the strike
        // is unbound, so a stalled REST call must not stack concurrent requests.
        if !Self::begin_strike_fetch_if_idle(&self.in_flight, binding.instrument_id) {
            log::warn!(
                "Chainlink strike source {} skipping {} subscribe for {} at {}={}: a fetch is already in flight",
                self.client_id,
                report_boundary.kind.label(),
                cmd.instrument_id,
                report_boundary.kind.param_key(),
                report_boundary.unix_seconds
            );
            return Ok(());
        }
        log::info!(
            "Chainlink strike source {} starting {} fetch for {} at {}={}",
            self.client_id,
            report_boundary.kind.label(),
            cmd.instrument_id,
            report_boundary.kind.param_key(),
            report_boundary.unix_seconds
        );
        let sender = self.data_sender.clone();
        let client_id = self.client_id;
        let in_flight = Arc::clone(&self.in_flight);
        let fetch_instrument_id = binding.instrument_id;

        get_runtime().spawn(async move {
            match fetch_chainlink_report_index_price(&request).await {
                Ok(index_price) => {
                    log::info!(
                        "Chainlink strike source {client_id} fetched {} for {} at {}={}: value={} ts_event={}",
                        request.report_boundary.kind.label(),
                        request.instrument_id,
                        request.report_boundary.kind.param_key(),
                        request.report_boundary.unix_seconds,
                        index_price.value,
                        index_price.ts_event
                    );
                    if sender
                        .send(DataEvent::Data(Data::IndexPriceUpdate(index_price)))
                        .is_err()
                    {
                        log::error!(
                            "Chainlink strike source {client_id} could not deliver {} for {}: data channel closed",
                            request.report_boundary.kind.label(),
                            request.instrument_id
                        );
                    } else {
                        log::info!(
                            "Chainlink strike source {client_id} delivered {} for {} at {}={}",
                            request.report_boundary.kind.label(),
                            request.instrument_id,
                            request.report_boundary.kind.param_key(),
                            request.report_boundary.unix_seconds
                        );
                    }
                }
                Err(error) => {
                    log::error!(
                        "Chainlink strike source {client_id} {} fetch failed for {} at {}={}: {error:#}",
                        request.report_boundary.kind.label(),
                        request.instrument_id,
                        request.report_boundary.kind.param_key(),
                        request.report_boundary.unix_seconds
                    );
                }
            }
            ChainlinkStrikeSourceClient::finish_strike_fetch(&in_flight, fetch_instrument_id);
            log::info!(
                "Chainlink strike source {client_id} cleared in-flight {} fetch for {} at {}={}",
                request.report_boundary.kind.label(),
                request.instrument_id,
                request.report_boundary.kind.param_key(),
                request.report_boundary.unix_seconds
            );
        });
        Ok(())
    }

    fn unsubscribe_index_prices(&mut self, cmd: &UnsubscribeIndexPrices) -> anyhow::Result<()> {
        // Point-in-time source: each subscribe emits one strike and nothing
        // persists, so unsubscribe is a no-op beyond acknowledging.
        log::info!(
            "Chainlink strike source {} unsubscribed index prices for {}",
            self.client_id,
            cmd.instrument_id
        );
        Ok(())
    }
}

/// All inputs required for one strike fetch, owned so the fetch can run on the
/// async runtime without borrowing the client.
struct StrikeFetchRequest {
    rest_base_url: String,
    report_endpoint_path: String,
    http_timeout_secs: u64,
    api_key: Zeroizing<String>,
    api_secret: Zeroizing<String>,
    feed_id: String,
    instrument_id: InstrumentId,
    report_schema_version: u64,
    report_decimal_scale: u64,
    price_precision: u8,
    report_boundary: ChainlinkReportBoundary,
}

fn requested_report_boundary(
    params: Option<&Params>,
    instrument_id: InstrumentId,
) -> anyhow::Result<ChainlinkReportBoundary> {
    let window_open = params
        .and_then(|params| params.get_u64(STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM))
        .filter(|seconds| *seconds != 0);
    let window_close = params
        .and_then(|params| params.get_u64(SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM))
        .filter(|seconds| *seconds != 0);
    match (window_open, window_close) {
        (Some(unix_seconds), None) => Ok(ChainlinkReportBoundary::new(
            ChainlinkReportBoundaryKind::WindowOpenStrike,
            unix_seconds,
        )),
        (None, Some(unix_seconds)) => Ok(ChainlinkReportBoundary::new(
            ChainlinkReportBoundaryKind::WindowCloseSettlement,
            unix_seconds,
        )),
        (Some(_), Some(_)) => anyhow::bail!(
            "Chainlink strike subscribe for {} must provide exactly one positive timestamp param: `{}` or `{}`, but both were set",
            instrument_id,
            STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM,
            SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM
        ),
        (None, None) => anyhow::bail!(
            "Chainlink strike subscribe for {} is missing a positive timestamp param: `{}` or `{}`",
            instrument_id,
            STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM,
            SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM
        ),
    }
}

/// Fetches the Chainlink Data Streams report AT a requested boundary timestamp
/// and returns the benchmark price as an NT [`IndexPriceUpdate`] on the
/// resolution instrument. Reuses the extracted auth/url/decode core and mirrors
/// the offline timestamped fetch pattern (signed GET, byte-bounded decode).
async fn fetch_chainlink_report_index_price(
    request: &StrikeFetchRequest,
) -> anyhow::Result<IndexPriceUpdate> {
    let credentials = chainlink_data_streams_credentials(&request.api_key, &request.api_secret)
        .map_err(|error| anyhow::anyhow!("Chainlink strike credentials invalid: {error}"))?;
    let (url, path_with_query) = chainlink_data_streams_report_request_url(
        &request.rest_base_url,
        &request.report_endpoint_path,
        &request.feed_id,
        request.report_boundary.unix_seconds,
    )
    .map_err(|error| anyhow::anyhow!("Chainlink strike request URL invalid: {error}"))?;
    let authorization_timestamp_ms = current_unix_timestamp_ms()?;
    let headers = chainlink_data_streams_auth_headers(
        &credentials,
        &path_with_query,
        authorization_timestamp_ms,
    );

    let client = HttpClient::new(
        HashMap::from([(USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string())]),
        Vec::new(),
        Vec::new(),
        None,
        Some(request.http_timeout_secs),
        None,
    )
    .map_err(|error| anyhow::anyhow!("Chainlink strike HTTP client could not be built: {error}"))?;
    let response = client
        .get(
            url,
            None,
            Some(headers),
            Some(request.http_timeout_secs),
            None,
        )
        .await
        .map_err(|error| anyhow::anyhow!("Chainlink strike report fetch failed: {error}"))?;
    if !response.status.is_success() {
        anyhow::bail!(
            "Chainlink strike report fetch failed with HTTP status {}",
            response.status.as_u16()
        );
    }

    let api_response: ChainlinkDataStreamsReportApiResponse =
        serde_json::from_slice(&response.body).map_err(|_| {
            anyhow::anyhow!("Chainlink strike report response is not a valid report JSON payload")
        })?;
    let report_bytes = serde_json::to_vec_pretty(&api_response.report)
        .map_err(|_| anyhow::anyhow!("Chainlink strike report source could not serialize"))?;

    let binding = PriceToBeatReportBinding {
        feed_id: request.feed_id.clone(),
        schema_version: request.report_schema_version,
        decimal_scale: request.report_decimal_scale,
    };
    let decoded = decode_price_to_beat_report(&report_bytes, &binding)
        .map_err(|error| anyhow::anyhow!("Chainlink strike report decode failed: {error}"))?;

    let ts_init = UnixNanos::from(current_unix_timestamp_ms()? * 1_000_000);
    match request.report_boundary.kind {
        ChainlinkReportBoundaryKind::WindowOpenStrike => build_strike_index_price(
            request.instrument_id,
            &decoded,
            request.price_precision,
            request.report_boundary.unix_seconds,
            ts_init,
        ),
        ChainlinkReportBoundaryKind::WindowCloseSettlement => build_settlement_close_index_price(
            request.instrument_id,
            &decoded,
            request.price_precision,
            request.report_boundary.unix_seconds,
            ts_init,
        ),
    }
}

/// Maps a decoded strike report to the NT [`IndexPriceUpdate`] on the
/// resolution instrument, with `ts_event` pinned to the window-open boundary
/// (the strike instant). Pure (no network, no clock read): the caller supplies
/// `ts_init`. Split out of [`fetch_chainlink_report_index_price`] so the
/// value/timestamp mapping is unit-testable from a decoded fixture without an
/// HTTP round-trip.
///
/// Fail-closed interval-open binding (F2): the Chainlink "report at T" REST
/// endpoint returns the report *active at* T (i.e. `validFrom <= T`), not
/// necessarily the report whose `validFrom == T`. A strike is the price-to-beat
/// only if it is the report that opened the interval, so this rejects any
/// decoded report whose `validFrom` does not equal the requested window-open
/// boundary. Without this check a stale-instant strike would be silently bound
/// as the price-to-beat and systematically misprice live entries.
pub(crate) fn build_strike_index_price(
    instrument_id: InstrumentId,
    decoded: &DecodedPriceToBeatReport,
    price_precision: u8,
    window_open_unix_seconds: u64,
    ts_init: UnixNanos,
) -> anyhow::Result<IndexPriceUpdate> {
    build_bound_report_index_price(
        instrument_id,
        decoded,
        price_precision,
        ChainlinkReportBoundary::new(
            ChainlinkReportBoundaryKind::WindowOpenStrike,
            window_open_unix_seconds,
        ),
        ts_init,
    )
}

pub(crate) fn build_settlement_close_index_price(
    instrument_id: InstrumentId,
    decoded: &DecodedPriceToBeatReport,
    price_precision: u8,
    window_close_unix_seconds: u64,
    ts_init: UnixNanos,
) -> anyhow::Result<IndexPriceUpdate> {
    build_bound_report_index_price(
        instrument_id,
        decoded,
        price_precision,
        ChainlinkReportBoundary::new(
            ChainlinkReportBoundaryKind::WindowCloseSettlement,
            window_close_unix_seconds,
        ),
        ts_init,
    )
}

fn build_bound_report_index_price(
    instrument_id: InstrumentId,
    decoded: &DecodedPriceToBeatReport,
    price_precision: u8,
    report_boundary: ChainlinkReportBoundary,
    ts_init: UnixNanos,
) -> anyhow::Result<IndexPriceUpdate> {
    let boundary_unix_millis = report_boundary
        .unix_seconds
        .checked_mul(CHAINLINK_REPORT_MILLISECONDS_PER_SECOND)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Chainlink {} timestamp exceeds milliseconds range",
                report_boundary.kind.label()
            )
        })?;
    if decoded.valid_from_timestamp_ms != boundary_unix_millis {
        anyhow::bail!(
            "Chainlink {} report is not bound to the {}: report validFrom is {} ms but the boundary is {} ms",
            report_boundary.kind.label(),
            report_boundary.kind.boundary_label(),
            decoded.valid_from_timestamp_ms,
            boundary_unix_millis
        );
    }
    let value = Price::new_checked(decoded.benchmark_price, price_precision).map_err(|error| {
        anyhow::anyhow!(
            "Chainlink {} benchmark price is not a valid NT Price: {error}",
            report_boundary.kind.label()
        )
    })?;
    // ts_event = requested boundary. The report timestamp is published in
    // seconds; convert to nanos for the NT IndexPriceUpdate.
    let ts_event = boundary_unix_nanos(report_boundary)?;
    Ok(IndexPriceUpdate::new(
        instrument_id,
        value,
        ts_event,
        ts_init,
    ))
}

fn boundary_unix_nanos(report_boundary: ChainlinkReportBoundary) -> anyhow::Result<UnixNanos> {
    report_boundary
        .unix_seconds
        .checked_mul(1_000_000_000)
        .map(UnixNanos::from)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Chainlink {} timestamp exceeds nanos range",
                report_boundary.kind.label()
            )
        })
}

fn current_unix_timestamp_ms() -> anyhow::Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow::anyhow!("system clock is before Unix epoch"))?
        .as_millis();
    u64::try_from(millis)
        .map_err(|_| anyhow::anyhow!("system clock timestamp exceeds supported range"))
}

/// Parses a TOML feed-binding table into a [`ChainlinkStrikeFeedBinding`].
///
/// Shared between the provider binding's `map_adapters` (build path) and
/// `validate_client` (startup-validation path) so the `feed_bindings` schema
/// has a single owner. Returns a human-readable error string per the bolt-v3
/// validation convention.
pub fn parse_feed_binding(
    client_key: &str,
    index: usize,
    value: &toml::Value,
) -> Result<ChainlinkStrikeFeedBinding, String> {
    let table = value.as_table().ok_or_else(|| {
        format!("clients.{client_key}.data.feed_bindings[{index}] must be a table")
    })?;
    let feed_id = required_str(client_key, index, table, "feed_id")?;
    if !super::is_lowercase_chainlink_feed_id(&feed_id) {
        return Err(format!(
            "clients.{client_key}.data.feed_bindings[{index}].feed_id must be a 0x-prefixed lowercase 64-hex Chainlink Data Streams feed id"
        ));
    }
    let instrument_id_raw = required_str(client_key, index, table, "instrument_id")?;
    let instrument_id = InstrumentId::from_str(&instrument_id_raw).map_err(|error| {
        format!(
            "clients.{client_key}.data.feed_bindings[{index}].instrument_id is not a valid NT InstrumentId: {error}"
        )
    })?;
    let report_schema_version =
        required_positive_u64(client_key, index, table, "report_schema_version")?;
    let report_decimal_scale =
        required_positive_u64(client_key, index, table, "report_decimal_scale")?;
    let price_precision = required_u8(client_key, index, table, "price_precision")?;
    Ok(ChainlinkStrikeFeedBinding {
        feed_id,
        instrument_id,
        report_schema_version,
        report_decimal_scale,
        price_precision,
    })
}

fn required_str(
    client_key: &str,
    index: usize,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Result<String, String> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            format!("clients.{client_key}.data.feed_bindings[{index}].{field} must be a non-empty string")
        })
}

fn required_positive_u64(
    client_key: &str,
    index: usize,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Result<u64, String> {
    table
        .get(field)
        .and_then(toml::Value::as_integer)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            format!("clients.{client_key}.data.feed_bindings[{index}].{field} must be a positive integer")
        })
}

fn required_u8(
    client_key: &str,
    index: usize,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Result<u8, String> {
    table
        .get(field)
        .and_then(toml::Value::as_integer)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| {
            format!(
                "clients.{client_key}.data.feed_bindings[{index}].{field} must be an integer in 0..=255"
            )
        })
}

#[cfg(test)]
mod tests {
    //! Strike/settlement-mapping unit tests (NO network): a decoded V3 report
    //! fixture for a (feed_id, boundary timestamp) is mapped to the configured
    //! resolution instrument and delivered as an NT [`IndexPriceUpdate`] whose
    //! value equals the decoded benchmark price and whose `ts_event` is the
    //! requested boundary in nanos. The report-blob fixture mirrors the ABI
    //! layout used by `super::report`'s decode tests and the offline
    //! materializer tests.

    use rust_decimal::{Decimal, prelude::ToPrimitive};

    use super::*;

    const TEST_FEED_ID: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const TEST_INSTRUMENT_ID: &str = "BTC-USD-UP.BOLT";
    const TEST_DECIMAL_SCALE: u64 = 18;
    const TEST_PRICE_PRECISION: u8 = 2;
    const TEST_BENCHMARK_PRICE: f64 = 3300.5;
    const TEST_WINDOW_OPEN_UNIX_SECONDS: u64 = 1_700_000_000;
    const TEST_WINDOW_CLOSE_UNIX_SECONDS: u64 = 1_700_000_900;
    const TEST_TS_INIT_NANOS: u64 = 1_700_000_500_000_000_000;
    const NANOS_PER_SECOND: u64 = 1_000_000_000;

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

    fn scaled_price(benchmark_price: f64, decimal_scale: u64) -> i128 {
        let scale = 10_i128
            .checked_pow(u32::try_from(decimal_scale).expect("scale should fit u32"))
            .expect("scale should fit i128");
        let price = Decimal::from_str_exact(&benchmark_price.to_string())
            .expect("benchmark price should be decimal");
        (price * Decimal::from(scale))
            .round()
            .to_i128()
            .expect("scaled price should fit i128")
    }

    fn report_source_json(
        feed_id: &str,
        valid_from_seconds: u32,
        observations_seconds: u32,
        benchmark_price: f64,
        decimal_scale: u64,
    ) -> Vec<u8> {
        let benchmark_word = abi_i192_word(scaled_price(benchmark_price, decimal_scale));
        let mut blob = Vec::new();
        blob.extend_from_slice(&feed_id_bytes(feed_id));
        blob.extend_from_slice(&abi_u32_word(valid_from_seconds));
        blob.extend_from_slice(&abi_u32_word(observations_seconds));
        blob.extend_from_slice(&abi_zero_word());
        blob.extend_from_slice(&abi_zero_word());
        blob.extend_from_slice(&abi_u32_word(observations_seconds + 60));
        blob.extend_from_slice(&benchmark_word);
        blob.extend_from_slice(&benchmark_word);
        blob.extend_from_slice(&benchmark_word);

        let mut payload = Vec::new();
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_usize_word(128));
        payload.extend_from_slice(&abi_usize_word(blob.len()));
        payload.extend_from_slice(&blob);

        serde_json::to_vec_pretty(&serde_json::json!({
            "feedID": feed_id,
            "validFromTimestamp": valid_from_seconds,
            "observationsTimestamp": observations_seconds,
            "fullReport": format!("0x{}", hex::encode(payload)),
        }))
        .expect("report source JSON should serialize")
    }

    fn observations_after(valid_from_seconds: u32) -> u32 {
        valid_from_seconds
            .checked_add(1)
            .expect("test observation timestamp should fit u32")
    }

    #[test]
    fn decoded_report_maps_to_index_price_on_resolution_instrument_at_window_open() {
        let instrument_id = InstrumentId::from_str(TEST_INSTRUMENT_ID)
            .expect("test resolution instrument id should parse");
        // Window-open ts is the strike instant; validFrom is pinned to it.
        let valid_from_seconds =
            u32::try_from(TEST_WINDOW_OPEN_UNIX_SECONDS).expect("window-open ts should fit u32");
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            valid_from_seconds,
            observations_after(valid_from_seconds),
            TEST_BENCHMARK_PRICE,
            TEST_DECIMAL_SCALE,
        );
        let binding = PriceToBeatReportBinding {
            feed_id: TEST_FEED_ID.to_string(),
            schema_version: 3,
            decimal_scale: TEST_DECIMAL_SCALE,
        };
        let decoded = decode_price_to_beat_report(&report_bytes, &binding)
            .expect("the fixture V3 report should decode");

        let index_price = build_strike_index_price(
            instrument_id,
            &decoded,
            TEST_PRICE_PRECISION,
            TEST_WINDOW_OPEN_UNIX_SECONDS,
            UnixNanos::from(TEST_TS_INIT_NANOS),
        )
        .expect("decoded strike should map to an IndexPriceUpdate");

        assert_eq!(
            index_price.instrument_id, instrument_id,
            "strike must publish on the configured resolution instrument"
        );
        assert!(
            (index_price.value.as_f64() - TEST_BENCHMARK_PRICE).abs() < 1e-6,
            "strike value must equal the decoded benchmark price, got {}",
            index_price.value
        );
        assert_eq!(
            index_price.ts_event.as_u64(),
            TEST_WINDOW_OPEN_UNIX_SECONDS * NANOS_PER_SECOND,
            "ts_event must be the window-open boundary in nanos"
        );
        assert_eq!(
            index_price.ts_init.as_u64(),
            TEST_TS_INIT_NANOS,
            "ts_init must carry the supplied initialization timestamp"
        );
    }

    #[test]
    fn decoded_report_maps_to_index_price_on_resolution_instrument_at_window_close() {
        let instrument_id = InstrumentId::from_str(TEST_INSTRUMENT_ID)
            .expect("test resolution instrument id should parse");
        let valid_from_seconds =
            u32::try_from(TEST_WINDOW_CLOSE_UNIX_SECONDS).expect("window-close ts should fit u32");
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            valid_from_seconds,
            observations_after(valid_from_seconds),
            TEST_BENCHMARK_PRICE,
            TEST_DECIMAL_SCALE,
        );
        let binding = PriceToBeatReportBinding {
            feed_id: TEST_FEED_ID.to_string(),
            schema_version: 3,
            decimal_scale: TEST_DECIMAL_SCALE,
        };
        let decoded = decode_price_to_beat_report(&report_bytes, &binding)
            .expect("the fixture V3 report should decode");

        let index_price = build_settlement_close_index_price(
            instrument_id,
            &decoded,
            TEST_PRICE_PRECISION,
            TEST_WINDOW_CLOSE_UNIX_SECONDS,
            UnixNanos::from(TEST_TS_INIT_NANOS),
        )
        .expect("decoded close report should map to an IndexPriceUpdate");

        assert_eq!(
            index_price.instrument_id, instrument_id,
            "settlement close must publish on the configured resolution instrument"
        );
        assert!(
            (index_price.value.as_f64() - TEST_BENCHMARK_PRICE).abs() < 1e-6,
            "settlement close value must equal the decoded benchmark price, got {}",
            index_price.value
        );
        assert_eq!(
            index_price.ts_event.as_u64(),
            TEST_WINDOW_CLOSE_UNIX_SECONDS * NANOS_PER_SECOND,
            "ts_event must be the window-close boundary in nanos"
        );
        assert_eq!(
            index_price.ts_init.as_u64(),
            TEST_TS_INIT_NANOS,
            "ts_init must carry the supplied initialization timestamp"
        );
    }

    #[test]
    fn strike_mapping_rejects_window_open_timestamp_beyond_supported_range() {
        let instrument_id = InstrumentId::from_str(TEST_INSTRUMENT_ID)
            .expect("test resolution instrument id should parse");
        // A window-open ts that overflows the timestamp range must fail closed
        // before any strike is emitted (the milliseconds guard trips first).
        let decoded = DecodedPriceToBeatReport {
            feed_id: TEST_FEED_ID.to_string(),
            valid_from_timestamp_ms: u64::MAX,
            observations_timestamp_ms: u64::MAX,
            benchmark_price: TEST_BENCHMARK_PRICE,
            bid_price: TEST_BENCHMARK_PRICE,
            ask_price: TEST_BENCHMARK_PRICE,
        };
        build_strike_index_price(
            instrument_id,
            &decoded,
            TEST_PRICE_PRECISION,
            u64::MAX,
            UnixNanos::from(TEST_TS_INIT_NANOS),
        )
        .expect_err("a window-open ts that overflows the timestamp range must fail closed");
    }

    #[test]
    fn strike_mapping_rejects_report_not_bound_to_window_open() {
        let instrument_id = InstrumentId::from_str(TEST_INSTRUMENT_ID)
            .expect("test resolution instrument id should parse");
        // The REST "report at T" endpoint can return a report whose validFrom is
        // BEFORE the requested window-open (the report active at T). Such a
        // report is not the interval-open strike and must be rejected (F2).
        let stale_valid_from_seconds = u32::try_from(TEST_WINDOW_OPEN_UNIX_SECONDS)
            .expect("window-open ts should fit u32")
            - 60;
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            stale_valid_from_seconds,
            observations_after(stale_valid_from_seconds),
            TEST_BENCHMARK_PRICE,
            TEST_DECIMAL_SCALE,
        );
        let binding = PriceToBeatReportBinding {
            feed_id: TEST_FEED_ID.to_string(),
            schema_version: 3,
            decimal_scale: TEST_DECIMAL_SCALE,
        };
        let decoded = decode_price_to_beat_report(&report_bytes, &binding)
            .expect("the fixture V3 report should decode");

        build_strike_index_price(
            instrument_id,
            &decoded,
            TEST_PRICE_PRECISION,
            TEST_WINDOW_OPEN_UNIX_SECONDS,
            UnixNanos::from(TEST_TS_INIT_NANOS),
        )
        .expect_err("a report whose validFrom is not the window-open boundary must fail closed");
    }

    #[test]
    fn settlement_close_mapping_rejects_report_not_bound_to_window_close() {
        let instrument_id = InstrumentId::from_str(TEST_INSTRUMENT_ID)
            .expect("test resolution instrument id should parse");
        let stale_valid_from_seconds = u32::try_from(TEST_WINDOW_CLOSE_UNIX_SECONDS)
            .expect("window-close ts should fit u32")
            - 60;
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            stale_valid_from_seconds,
            observations_after(stale_valid_from_seconds),
            TEST_BENCHMARK_PRICE,
            TEST_DECIMAL_SCALE,
        );
        let binding = PriceToBeatReportBinding {
            feed_id: TEST_FEED_ID.to_string(),
            schema_version: 3,
            decimal_scale: TEST_DECIMAL_SCALE,
        };
        let decoded = decode_price_to_beat_report(&report_bytes, &binding)
            .expect("the fixture V3 report should decode");

        build_settlement_close_index_price(
            instrument_id,
            &decoded,
            TEST_PRICE_PRECISION,
            TEST_WINDOW_CLOSE_UNIX_SECONDS,
            UnixNanos::from(TEST_TS_INIT_NANOS),
        )
        .expect_err("a report whose validFrom is not the window-close boundary must fail closed");
    }

    #[test]
    fn report_boundary_params_accept_one_close_timestamp() {
        let instrument_id =
            InstrumentId::from_str(TEST_INSTRUMENT_ID).expect("resolution instrument id parses");
        let mut params = Params::new();
        params.insert(
            SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM.to_string(),
            serde_json::json!(TEST_WINDOW_CLOSE_UNIX_SECONDS),
        );

        let boundary = requested_report_boundary(Some(&params), instrument_id)
            .expect("one close timestamp should select settlement close");

        assert_eq!(
            boundary,
            ChainlinkReportBoundary::new(
                ChainlinkReportBoundaryKind::WindowCloseSettlement,
                TEST_WINDOW_CLOSE_UNIX_SECONDS
            )
        );
    }

    #[test]
    fn report_boundary_params_reject_both_open_and_close_timestamps() {
        let instrument_id =
            InstrumentId::from_str(TEST_INSTRUMENT_ID).expect("resolution instrument id parses");
        let mut params = Params::new();
        params.insert(
            STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM.to_string(),
            serde_json::json!(TEST_WINDOW_OPEN_UNIX_SECONDS),
        );
        params.insert(
            SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM.to_string(),
            serde_json::json!(TEST_WINDOW_CLOSE_UNIX_SECONDS),
        );

        let error = requested_report_boundary(Some(&params), instrument_id)
            .expect_err("ambiguous timestamp params must fail closed");

        assert!(
            error.to_string().contains("exactly one positive timestamp"),
            "error should explain the single-timestamp contract, got: {error:#}"
        );
    }

    #[test]
    fn report_boundary_params_reject_missing_timestamps() {
        let instrument_id =
            InstrumentId::from_str(TEST_INSTRUMENT_ID).expect("resolution instrument id parses");

        let error = requested_report_boundary(None, instrument_id)
            .expect_err("missing timestamp params must fail closed");

        assert!(
            error
                .to_string()
                .contains(STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM)
                && error
                    .to_string()
                    .contains(SETTLEMENT_WINDOW_CLOSE_UNIX_SECONDS_PARAM),
            "error should name both accepted params, got: {error:#}"
        );
    }

    #[test]
    fn in_flight_guard_admits_one_fetch_per_instrument_until_finished() {
        // After the strategy's unsubscribe-before-subscribe re-arm reaches the source
        // on every retry tick, a stalled REST call must not let retries stack
        // concurrent fetches against the live endpoint. The in-flight guard admits at
        // most one fetch per resolution instrument until the prior one finishes.
        let in_flight: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<InstrumentId>>> =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        let instrument_id =
            InstrumentId::from_str(TEST_INSTRUMENT_ID).expect("resolution instrument id parses");
        assert!(
            ChainlinkStrikeSourceClient::begin_strike_fetch_if_idle(&in_flight, instrument_id),
            "the first fetch for an idle resolution instrument must be admitted",
        );
        assert!(
            !ChainlinkStrikeSourceClient::begin_strike_fetch_if_idle(&in_flight, instrument_id),
            "a second fetch must be skipped while one is already in flight",
        );
        ChainlinkStrikeSourceClient::finish_strike_fetch(&in_flight, instrument_id);
        assert!(
            ChainlinkStrikeSourceClient::begin_strike_fetch_if_idle(&in_flight, instrument_id),
            "after the in-flight fetch finishes, a new fetch may be admitted",
        );
    }
}
