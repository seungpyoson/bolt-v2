//! Local bolt-v3 order-lifecycle tracer.
//!
//! Scope: v3 TOML -> mock NT LiveNode -> existing strategy -> mock order-event capture.
//! This is not venue live-readiness, cancel, or reconciliation proof.

mod support;

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::Duration,
};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3ClientConfig, BoltV3ClientConfigs, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig, map_bolt_v3_clients,
    },
    bolt_v3_client_registration::register_bolt_v3_clients,
    bolt_v3_config::{
        LoadedBoltV3Config, REFERENCE_STREAM_ID_PARAMETER, ReferenceSourceType, load_bolt_v3_config,
    },
    bolt_v3_decision_events::{
        BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
        BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    },
    bolt_v3_live_node::make_bolt_v3_live_node_builder,
    bolt_v3_reference_actor_registration::register_bolt_v3_reference_actors,
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_strategy_registration::register_bolt_v3_strategies,
    platform::{
        reference::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind},
        resolution_basis::parse_ruleset_resolution_basis,
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState},
        runtime::runtime_selection_topic,
    },
};
use futures_util::StreamExt;
use nautilus_common::{
    messages::execution::{CancelAllOrders, TradingCommand},
    msgbus::switchboard::MessagingSwitchboard,
    msgbus::{self, publish_any, publish_deltas, switchboard},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, OrderBookDeltas},
    enums::{AssetClass, BookAction, LiquiditySide, OrderSide},
    events::{OrderAccepted, OrderCanceled, OrderEventAny, OrderFilled, OrderRejected},
    identifiers::{
        AccountId, ClientId, InstrumentId, PositionId, StrategyId, Symbol, TradeId, TraderId,
        VenueOrderId,
    },
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde::Deserialize;
use support::{
    LocalHttpStatus, MockDataClientConfig, MockDataClientFactory, MockExecClientConfig,
    MockExecutionClientFactory, RecordedSubmitOrder, clear_mock_exec_submissions,
    recorded_mock_exec_submissions,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener as TokioTcpListener,
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
    time::sleep,
};
use tokio_tungstenite::accept_async;
use ustr::Ustr;

static RUNTIME_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
static ORDER_LIFECYCLE_TRACER_FIXTURE: OnceLock<OrderLifecycleTracerFixture> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct OrderLifecycleTracerFixture {
    local_polymarket: LocalPolymarketFixture,
    selected_binary_option: SelectedBinaryOptionFixture,
    market_snapshot: MarketSnapshotFixture,
    scenario_prices: ScenarioPricesFixture,
    test_timing: TestTimingFixture,
    timeline_offsets_milliseconds: TimelineOffsetsMillisecondsFixture,
}

#[derive(Debug, Deserialize)]
struct LocalPolymarketFixture {
    accepted_order_id: String,
    up_token_id: String,
    down_token_id: String,
    fee_requests_per_binary_market: usize,
    http_timeout_seconds: i64,
    ack_timeout_seconds: i64,
    bind_addr: String,
    balance_allowance_method: String,
    balance_allowance_path: String,
    orders_data_method: String,
    orders_data_path: String,
    trades_data_method: String,
    trades_data_path: String,
    positions_method: String,
    positions_path: String,
    fee_rate_method: String,
    fee_rate_path: String,
    fee_rate_query_prefix: String,
    submit_order_method: String,
    submit_order_path: String,
    cancel_orders_method: String,
    cancel_orders_path: String,
    cancel_order_method: String,
    cancel_order_path: String,
}

#[derive(Debug, Deserialize)]
struct SelectedBinaryOptionFixture {
    price_increment: String,
    size_increment: String,
    book_level_quantity: String,
    created_ts_nanos: u64,
    updated_ts_nanos: u64,
}

#[derive(Debug, Deserialize)]
struct MarketSnapshotFixture {
    price_to_beat: f64,
    liquidity_num: f64,
    reference_orderbook_half_spread: f64,
}

#[derive(Debug, Deserialize)]
struct ScenarioPricesFixture {
    opening_fast_price: f64,
    entry_fair_value: f64,
    entry_fast_price: f64,
    exit_fair_value: f64,
    exit_fast_price: f64,
    entry_up_bid: f64,
    entry_up_ask: f64,
    exit_up_bid: f64,
    exit_up_ask: f64,
    down_bid: f64,
    down_ask: f64,
}

#[derive(Debug, Deserialize)]
struct TestTimingFixture {
    local_clob_wait_timeout_seconds: u64,
    poll_interval_milliseconds: u64,
    post_order_cancel_delay_milliseconds: u64,
    post_initial_market_delay_milliseconds: u64,
    fee_request_recv_timeout_seconds: u64,
    submit_poll_attempts: usize,
    real_execution_run_timeout_margin_seconds: u64,
    mock_lifecycle_run_timeout_margin_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct TimelineOffsetsMillisecondsFixture {
    real_execution_cancel_command: u64,
    initial_reference_snapshot: u64,
    entry_rejected_event: u64,
    entry_reference_snapshot: u64,
    entry_accepted_event: u64,
    entry_rejected_retry_snapshot: u64,
    entry_filled_event: u64,
    freeze_snapshot: u64,
    exit_accepted_event: u64,
    exit_canceled_event: u64,
    replacement_freeze_snapshot: u64,
}

fn runtime_test_mutex() -> &'static Mutex<()> {
    RUNTIME_TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

fn order_lifecycle_tracer_fixture() -> &'static OrderLifecycleTracerFixture {
    ORDER_LIFECYCLE_TRACER_FIXTURE.get_or_init(|| {
        let path = support::repo_path(
            "tests/fixtures/bolt_v3_existing_strategy/order_lifecycle_tracer.toml",
        );
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
        toml::from_str(&text)
            .unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()))
    })
}

fn local_polymarket_fixture() -> &'static LocalPolymarketFixture {
    &order_lifecycle_tracer_fixture().local_polymarket
}

fn local_polymarket_order_id() -> &'static str {
    local_polymarket_fixture().accepted_order_id.as_str()
}

fn local_polymarket_fee_requests_per_binary_market() -> usize {
    local_polymarket_fixture().fee_requests_per_binary_market
}

fn local_polymarket_up_token_id() -> String {
    local_polymarket_fixture().up_token_id.clone()
}

fn local_polymarket_down_token_id() -> String {
    local_polymarket_fixture().down_token_id.clone()
}

fn local_polymarket_http_timeout_seconds() -> i64 {
    local_polymarket_fixture().http_timeout_seconds
}

fn local_polymarket_ack_timeout_seconds() -> i64 {
    local_polymarket_fixture().ack_timeout_seconds
}

fn local_polymarket_bind_addr() -> &'static str {
    local_polymarket_fixture().bind_addr.as_str()
}

fn local_polymarket_balance_allowance_method() -> &'static str {
    local_polymarket_fixture().balance_allowance_method.as_str()
}

fn local_polymarket_balance_allowance_path() -> &'static str {
    local_polymarket_fixture().balance_allowance_path.as_str()
}

fn local_polymarket_orders_data_method() -> &'static str {
    local_polymarket_fixture().orders_data_method.as_str()
}

fn local_polymarket_orders_data_path() -> &'static str {
    local_polymarket_fixture().orders_data_path.as_str()
}

fn local_polymarket_trades_data_method() -> &'static str {
    local_polymarket_fixture().trades_data_method.as_str()
}

fn local_polymarket_trades_data_path() -> &'static str {
    local_polymarket_fixture().trades_data_path.as_str()
}

fn local_polymarket_positions_method() -> &'static str {
    local_polymarket_fixture().positions_method.as_str()
}

fn local_polymarket_positions_path() -> &'static str {
    local_polymarket_fixture().positions_path.as_str()
}

fn local_polymarket_fee_rate_method() -> &'static str {
    local_polymarket_fixture().fee_rate_method.as_str()
}

fn local_polymarket_fee_rate_path() -> &'static str {
    local_polymarket_fixture().fee_rate_path.as_str()
}

fn local_polymarket_fee_rate_query_prefix() -> &'static str {
    local_polymarket_fixture().fee_rate_query_prefix.as_str()
}

fn local_polymarket_submit_order_method() -> &'static str {
    local_polymarket_fixture().submit_order_method.as_str()
}

fn local_polymarket_submit_order_path() -> &'static str {
    local_polymarket_fixture().submit_order_path.as_str()
}

fn local_polymarket_cancel_orders_method() -> &'static str {
    local_polymarket_fixture().cancel_orders_method.as_str()
}

fn local_polymarket_cancel_orders_path() -> &'static str {
    local_polymarket_fixture().cancel_orders_path.as_str()
}

fn local_polymarket_cancel_order_method() -> &'static str {
    local_polymarket_fixture().cancel_order_method.as_str()
}

fn local_polymarket_cancel_order_path() -> &'static str {
    local_polymarket_fixture().cancel_order_path.as_str()
}

fn selected_binary_option_fixture() -> &'static SelectedBinaryOptionFixture {
    &order_lifecycle_tracer_fixture().selected_binary_option
}

fn selected_binary_option_price_increment() -> &'static str {
    selected_binary_option_fixture().price_increment.as_str()
}

fn selected_binary_option_price_precision() -> u8 {
    Price::from(selected_binary_option_price_increment()).precision
}

fn selected_binary_option_size_increment() -> &'static str {
    selected_binary_option_fixture().size_increment.as_str()
}

fn selected_binary_option_book_level_quantity() -> &'static str {
    selected_binary_option_fixture()
        .book_level_quantity
        .as_str()
}

fn selected_binary_option_created_ts() -> UnixNanos {
    UnixNanos::from(selected_binary_option_fixture().created_ts_nanos)
}

fn selected_binary_option_updated_ts() -> UnixNanos {
    UnixNanos::from(selected_binary_option_fixture().updated_ts_nanos)
}

fn market_snapshot_fixture() -> &'static MarketSnapshotFixture {
    &order_lifecycle_tracer_fixture().market_snapshot
}

fn market_snapshot_price_to_beat() -> f64 {
    market_snapshot_fixture().price_to_beat
}

fn market_snapshot_liquidity_num() -> f64 {
    market_snapshot_fixture().liquidity_num
}

fn reference_orderbook_half_spread() -> f64 {
    market_snapshot_fixture().reference_orderbook_half_spread
}

fn scenario_prices() -> &'static ScenarioPricesFixture {
    &order_lifecycle_tracer_fixture().scenario_prices
}

fn test_timing_fixture() -> &'static TestTimingFixture {
    &order_lifecycle_tracer_fixture().test_timing
}

fn order_lifecycle_local_clob_wait_timeout() -> Duration {
    Duration::from_secs(test_timing_fixture().local_clob_wait_timeout_seconds)
}

fn order_lifecycle_poll_interval() -> Duration {
    Duration::from_millis(test_timing_fixture().poll_interval_milliseconds)
}

fn order_lifecycle_post_order_cancel_delay() -> Duration {
    Duration::from_millis(test_timing_fixture().post_order_cancel_delay_milliseconds)
}

fn order_lifecycle_post_initial_market_delay() -> Duration {
    Duration::from_millis(test_timing_fixture().post_initial_market_delay_milliseconds)
}

fn order_lifecycle_fee_request_recv_timeout() -> Duration {
    Duration::from_secs(test_timing_fixture().fee_request_recv_timeout_seconds)
}

fn order_lifecycle_submit_poll_attempts() -> usize {
    test_timing_fixture().submit_poll_attempts
}

fn order_lifecycle_real_execution_run_timeout(loaded: &LoadedBoltV3Config) -> Duration {
    Duration::from_secs(
        loaded.root.nautilus.timeout_shutdown_seconds
            + loaded.root.nautilus.timeout_disconnection_seconds
            + test_timing_fixture().real_execution_run_timeout_margin_seconds,
    )
}

fn order_lifecycle_mock_run_timeout(loaded: &LoadedBoltV3Config) -> Duration {
    Duration::from_secs(
        loaded.root.nautilus.delay_post_stop_seconds
            + loaded.root.nautilus.timeout_shutdown_seconds
            + test_timing_fixture().mock_lifecycle_run_timeout_margin_seconds,
    )
}

fn timeline_offsets_milliseconds() -> &'static TimelineOffsetsMillisecondsFixture {
    &order_lifecycle_tracer_fixture().timeline_offsets_milliseconds
}

fn offset_ts_ms(start_ts_ms: u64, offset_milliseconds: u64) -> u64 {
    start_ts_ms + offset_milliseconds
}

fn existing_strategy_root_fixture() -> PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

fn protocol_payload_fixture(filename: &str) -> String {
    std::fs::read_to_string(support::repo_path(&format!(
        "tests/fixtures/bolt_v3_protocol_payloads/{filename}",
    )))
    .unwrap_or_else(|error| panic!("protocol payload fixture {filename} should load: {error}"))
}

fn protocol_payload_template(filename: &str, substitutions: &[(&str, &str)]) -> String {
    let mut body = protocol_payload_fixture(filename);
    for (key, value) in substitutions {
        body = body.replace(&format!("{{{key}}}"), value);
    }
    body
}

fn strategy_config(loaded: &LoadedBoltV3Config) -> &bolt_v2::bolt_v3_config::BoltV3StrategyConfig {
    &loaded
        .strategies
        .first()
        .expect("fixture should load one strategy")
        .config
}

fn target_field<'a>(loaded: &'a LoadedBoltV3Config, field: &str) -> &'a str {
    strategy_config(loaded)
        .target
        .get(field)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("fixture target should include {field}"))
}

fn configured_target_id(loaded: &LoadedBoltV3Config) -> String {
    target_field(loaded, "configured_target_id").to_string()
}

fn underlying_asset(loaded: &LoadedBoltV3Config) -> String {
    target_field(loaded, "underlying_asset").to_string()
}

fn execution_client_id(loaded: &LoadedBoltV3Config) -> String {
    strategy_config(loaded).execution_client_id.clone()
}

fn execution_account_id(loaded: &LoadedBoltV3Config, client_id: &str) -> String {
    loaded
        .root
        .clients
        .get(client_id)
        .and_then(|client| client.execution.as_ref())
        .and_then(toml::Value::as_table)
        .and_then(|table| table.get("account_id"))
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("{client_id} execution config should include account_id"))
        .to_string()
}

fn reference_publish_topic(loaded: &LoadedBoltV3Config) -> String {
    let stream_id = strategy_config(loaded)
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("fixture strategy should select reference stream");
    loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist")
        .publish_topic
        .clone()
}

fn point_execution_http_to_local_fee_server(
    loaded: &mut LoadedBoltV3Config,
    base_url_http: String,
) {
    let client_id = execution_client_id(loaded);
    loaded
        .root
        .clients
        .get_mut(&client_id)
        .and_then(|client| client.execution.as_mut())
        .and_then(toml::Value::as_table_mut)
        .unwrap_or_else(|| panic!("{client_id} execution config should be a TOML table"))
        .insert(
            "base_url_http".to_string(),
            toml::Value::String(base_url_http),
        );
}

fn mock_client_configs_from_loaded(loaded: &LoadedBoltV3Config) -> BoltV3ClientConfigs {
    let clients = loaded
        .root
        .clients
        .iter()
        .map(|(client_id, client)| {
            let venue = client.venue.as_str();
            let data = client.data.as_ref().map(|_| BoltV3DataClientAdapterConfig {
                factory: Box::new(MockDataClientFactory),
                config: Box::new(MockDataClientConfig::new(client_id, venue)),
            });
            let execution = client
                .execution
                .as_ref()
                .map(|_| BoltV3ExecutionClientAdapterConfig {
                    factory: Box::new(MockExecutionClientFactory),
                    config: Box::new(MockExecClientConfig::new(
                        client_id,
                        execution_account_id(loaded, client_id).as_str(),
                        venue,
                    )),
                });
            (client_id.clone(), BoltV3ClientConfig { data, execution })
        })
        .collect::<BTreeMap<_, _>>();
    BoltV3ClientConfigs { clients }
}

fn spawn_fee_rate_server(expected_requests: usize) -> (String, mpsc::Receiver<String>) {
    let listener =
        TcpListener::bind(local_polymarket_bind_addr()).expect("local fee server should bind");
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let body = protocol_payload_fixture("polymarket_fee_rate_zero.json");

    std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().expect("local fee server should accept");
            let mut request = Vec::new();
            loop {
                let mut buffer = [0_u8; 512];
                let read = stream
                    .read(&mut buffer)
                    .expect("local fee server should read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request).into_owned();
            tx.send(request)
                .expect("local fee server should record request");

            write!(
                stream,
                "{}",
                support::local_http_json_response(LocalHttpStatus::Ok, &body)
            )
            .expect("local fee server should write response");
        }
    });

    (base_url, rx)
}

#[derive(Debug, Clone)]
struct RecordedPolymarketRequest {
    method: String,
    target: String,
    body: String,
}

struct LocalPolymarketExecutionServer {
    http_base_url: String,
    ws_user_url: String,
    requests: Arc<AsyncMutex<Vec<RecordedPolymarketRequest>>>,
    http_task: JoinHandle<()>,
    ws_task: JoinHandle<()>,
}

impl Drop for LocalPolymarketExecutionServer {
    fn drop(&mut self) {
        self.http_task.abort();
        self.ws_task.abort();
    }
}

async fn start_local_polymarket_execution_server() -> LocalPolymarketExecutionServer {
    let requests = Arc::new(AsyncMutex::new(Vec::<RecordedPolymarketRequest>::new()));
    let http_listener = TokioTcpListener::bind(local_polymarket_bind_addr())
        .await
        .expect("local CLOB HTTP listener should bind");
    let http_base_url = format!(
        "http://{}",
        http_listener
            .local_addr()
            .expect("local CLOB HTTP listener should expose addr")
    );
    let recorded_http_requests = Arc::clone(&requests);
    let balance_allowance_body = Arc::new(protocol_payload_fixture(
        "polymarket_balance_allowance_high.json",
    ));
    let empty_cursor_page_body = Arc::new(protocol_payload_fixture(
        "polymarket_empty_cursor_page.json",
    ));
    let positions_body = Arc::new(protocol_payload_fixture("polymarket_positions_empty.json"));
    let fee_rate_body = Arc::new(protocol_payload_fixture("polymarket_fee_rate_zero.json"));
    let unexpected_request_body = Arc::new(protocol_payload_fixture(
        "polymarket_unexpected_request_error.json",
    ));
    let http_task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _peer)) = http_listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded_http_requests);
            let balance_allowance_body = Arc::clone(&balance_allowance_body);
            let empty_cursor_page_body = Arc::clone(&empty_cursor_page_body);
            let positions_body = Arc::clone(&positions_body);
            let fee_rate_body = Arc::clone(&fee_rate_body);
            let unexpected_request_body = Arc::clone(&unexpected_request_body);
            tokio::spawn(async move {
                let Some(request) = read_http_request(&mut socket).await else {
                    return;
                };
                let request_line = request.lines().next().unwrap_or_default();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let body = request
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body.to_string())
                    .unwrap_or_default();
                recorded.lock().await.push(RecordedPolymarketRequest {
                    method: method.clone(),
                    target: target.clone(),
                    body,
                });
                let path = target.split('?').next().unwrap_or_default();
                let (status, response_body) = if method
                    == local_polymarket_balance_allowance_method()
                    && path == local_polymarket_balance_allowance_path()
                {
                    (LocalHttpStatus::Ok, balance_allowance_body.as_ref().clone())
                } else if (method == local_polymarket_orders_data_method()
                    && path == local_polymarket_orders_data_path())
                    || (method == local_polymarket_trades_data_method()
                        && path == local_polymarket_trades_data_path())
                {
                    (LocalHttpStatus::Ok, empty_cursor_page_body.as_ref().clone())
                } else if method == local_polymarket_positions_method()
                    && path == local_polymarket_positions_path()
                {
                    (LocalHttpStatus::Ok, positions_body.as_ref().clone())
                } else if method == local_polymarket_fee_rate_method()
                    && path == local_polymarket_fee_rate_path()
                {
                    (LocalHttpStatus::Ok, fee_rate_body.as_ref().clone())
                } else if method == local_polymarket_submit_order_method()
                    && path == local_polymarket_submit_order_path()
                {
                    (
                        LocalHttpStatus::Ok,
                        protocol_payload_template(
                            "polymarket_order_success_template.json",
                            &[("order_id", local_polymarket_order_id())],
                        ),
                    )
                } else if method == local_polymarket_cancel_orders_method()
                    && path == local_polymarket_cancel_orders_path()
                {
                    (
                        LocalHttpStatus::Ok,
                        protocol_payload_template(
                            "polymarket_cancel_success_template.json",
                            &[("order_id", local_polymarket_order_id())],
                        ),
                    )
                } else if method == local_polymarket_cancel_order_method()
                    && path == local_polymarket_cancel_order_path()
                {
                    (
                        LocalHttpStatus::Ok,
                        protocol_payload_template(
                            "polymarket_cancel_success_template.json",
                            &[("order_id", local_polymarket_order_id())],
                        ),
                    )
                } else {
                    (
                        LocalHttpStatus::NotFound,
                        unexpected_request_body.as_ref().clone(),
                    )
                };
                let response = support::local_http_json_response(status, &response_body);
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    let ws_listener = TokioTcpListener::bind(local_polymarket_bind_addr())
        .await
        .expect("local user WS listener should bind");
    let ws_user_url = format!(
        "ws://{}",
        ws_listener
            .local_addr()
            .expect("local user WS listener should expose addr")
    );
    let ws_task = tokio::spawn(async move {
        loop {
            let Ok((stream, _peer)) = ws_listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let Ok(mut websocket) = accept_async(stream).await else {
                    return;
                };
                while websocket.next().await.is_some() {}
            });
        }
    });

    LocalPolymarketExecutionServer {
        http_base_url,
        ws_user_url,
        requests,
        http_task,
        ws_task,
    }
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> Option<String> {
    let mut request = Vec::new();
    loop {
        let mut buffer = [0_u8; 1024];
        let read = socket.read(&mut buffer).await.ok()?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if request_has_complete_body(&request) {
            break;
        }
    }
    String::from_utf8(request).ok()
}

fn request_has_complete_body(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    request.len() >= header_end + 4 + content_length
}

fn point_execution_to_local_polymarket_server(
    loaded: &mut LoadedBoltV3Config,
    server: &LocalPolymarketExecutionServer,
) {
    let client_id = execution_client_id(loaded);
    let execution = loaded
        .root
        .clients
        .get_mut(&client_id)
        .and_then(|client| client.execution.as_mut())
        .and_then(toml::Value::as_table_mut)
        .unwrap_or_else(|| panic!("{client_id} execution config should be a TOML table"));
    execution.insert(
        "base_url_http".to_string(),
        toml::Value::String(server.http_base_url.clone()),
    );
    execution.insert(
        "base_url_ws".to_string(),
        toml::Value::String(server.ws_user_url.clone()),
    );
    execution.insert(
        "base_url_data_api".to_string(),
        toml::Value::String(server.http_base_url.clone()),
    );
    execution.insert(
        "http_timeout_seconds".to_string(),
        toml::Value::Integer(local_polymarket_http_timeout_seconds()),
    );
    execution.insert(
        "ack_timeout_seconds".to_string(),
        toml::Value::Integer(local_polymarket_ack_timeout_seconds()),
    );
}

fn client_configs_with_real_polymarket_execution(
    loaded: &LoadedBoltV3Config,
    resolved: &bolt_v2::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> BoltV3ClientConfigs {
    let mut configs = mock_client_configs_from_loaded(loaded);
    let mut real_configs =
        map_bolt_v3_clients(loaded, resolved).expect("real v3 client configs should map");
    let client_id = execution_client_id(loaded);
    let real_execution = real_configs
        .clients
        .remove(&client_id)
        .and_then(|client| client.execution)
        .expect("real Polymarket execution config should map from v3 TOML");
    configs
        .clients
        .get_mut(&client_id)
        .expect("mock client configs should include strategy execution client")
        .execution = Some(real_execution);
    configs
}

async fn wait_for_local_clob_request(
    server: &LocalPolymarketExecutionServer,
    method: &str,
    path: &str,
) -> RecordedPolymarketRequest {
    let deadline = tokio::time::Instant::now() + order_lifecycle_local_clob_wait_timeout();
    loop {
        if let Some(request) = server
            .requests
            .lock()
            .await
            .iter()
            .find(|request| {
                request.method == method && request.target.split('?').next() == Some(path)
            })
            .cloned()
        {
            return request;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for local CLOB {method} {path}; observed {:#?}",
            server.requests.lock().await
        );
        sleep(order_lifecycle_poll_interval()).await;
    }
}

async fn wait_for_local_clob_request_count(
    server: &LocalPolymarketExecutionServer,
    method: &str,
    path: &str,
    count: usize,
) {
    let deadline = tokio::time::Instant::now() + order_lifecycle_local_clob_wait_timeout();
    loop {
        let observed = server
            .requests
            .lock()
            .await
            .iter()
            .filter(|request| {
                request.method == method && request.target.split('?').next() == Some(path)
            })
            .count();
        if observed >= count {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {count} local CLOB {method} {path} requests; observed {:#?}",
            server.requests.lock().await
        );
        sleep(order_lifecycle_poll_interval()).await;
    }
}

fn selected_market(loaded: &LoadedBoltV3Config, start_ts_ms: u64) -> CandidateMarket {
    let market_id = configured_target_id(loaded);
    let underlying = underlying_asset(loaded);
    let condition_id = format!("condition-{}", underlying.to_ascii_lowercase());
    let up_token_id = local_polymarket_up_token_id();
    let down_token_id = local_polymarket_down_token_id();
    CandidateMarket {
        market_id: market_id.clone(),
        market_slug: market_id.clone(),
        question_id: format!("question-{market_id}"),
        instrument_id: instrument_id(&condition_id, &up_token_id, loaded).to_string(),
        condition_id,
        up_token_id,
        down_token_id,
        selected_market_observed_ts_ms: start_ts_ms,
        price_to_beat: Some(price_to_beat_value()),
        price_to_beat_source: Some(reference_publish_topic(loaded)),
        price_to_beat_observed_ts_ms: Some(start_ts_ms),
        start_ts_ms,
        end_ts_ms: start_ts_ms
            + strategy_config(loaded).target["cadence_seconds"]
                .as_integer()
                .expect("fixture target should include cadence_seconds") as u64
                * 1_000,
        declared_resolution_basis: parse_ruleset_resolution_basis(&declared_resolution_basis_key(
            loaded,
        ))
        .expect("fixture resolution basis should parse"),
        accepting_orders: true,
        liquidity_num: market_snapshot_liquidity_num(),
        seconds_to_end: strategy_config(loaded).target["cadence_seconds"]
            .as_integer()
            .expect("fixture target should include cadence_seconds") as u64,
    }
}

fn declared_resolution_basis_key(loaded: &LoadedBoltV3Config) -> String {
    let stream_id = strategy_config(loaded)
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("fixture strategy should select reference stream");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist");
    let oracle_input = stream
        .inputs
        .iter()
        .find(|input| input.source_type == ReferenceSourceType::Oracle)
        .expect("selected reference stream should include oracle input");
    let oracle_client_id = oracle_input
        .data_client_id
        .as_deref()
        .expect("oracle input should reference data client");
    let oracle_client = loaded
        .root
        .clients
        .get(oracle_client_id)
        .expect("oracle data client should exist");
    let symbol = oracle_input
        .instrument_id
        .split('.')
        .next()
        .expect("oracle instrument should include symbol");
    format!(
        "{}_{}",
        oracle_client.venue.as_str().to_ascii_lowercase(),
        symbol.to_ascii_lowercase()
    )
}

fn price_to_beat_value() -> f64 {
    market_snapshot_price_to_beat()
}

fn selection_snapshot(loaded: &LoadedBoltV3Config, start_ts_ms: u64) -> RuntimeSelectionSnapshot {
    let market = selected_market(loaded, start_ts_ms);
    RuntimeSelectionSnapshot {
        ruleset_id: configured_target_id(loaded),
        decision: SelectionDecision {
            ruleset_id: configured_target_id(loaded),
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms: start_ts_ms,
    }
}

fn freeze_selection_snapshot(
    loaded: &LoadedBoltV3Config,
    start_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let market = selected_market(loaded, start_ts_ms);
    RuntimeSelectionSnapshot {
        ruleset_id: configured_target_id(loaded),
        decision: SelectionDecision {
            ruleset_id: configured_target_id(loaded),
            state: SelectionState::Freeze {
                market: market.clone(),
                reason: format!("{}-freeze", configured_target_id(loaded)),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms: start_ts_ms,
    }
}

fn reference_snapshot(
    loaded: &LoadedBoltV3Config,
    ts_ms: u64,
    fair_value: f64,
    fast_price: f64,
) -> ReferenceSnapshot {
    let stream_id = strategy_config(loaded)
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("fixture strategy should select reference stream");
    let stream = loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist");
    let venues = stream
        .inputs
        .iter()
        .map(|input| {
            let venue_kind = match input.source_type {
                ReferenceSourceType::Oracle => VenueKind::Oracle,
                ReferenceSourceType::Orderbook => VenueKind::Orderbook,
            };
            let observed_price = match venue_kind {
                VenueKind::Oracle => fair_value,
                VenueKind::Orderbook => fast_price,
            };
            EffectiveVenueState {
                venue_name: input.source_id.clone(),
                base_weight: input.base_weight,
                effective_weight: input.base_weight,
                stale: false,
                health: VenueHealth::Healthy,
                observed_ts_ms: Some(ts_ms),
                venue_kind,
                observed_price: Some(observed_price),
                observed_bid: (venue_kind == VenueKind::Orderbook)
                    .then_some(fast_price - reference_orderbook_half_spread()),
                observed_ask: (venue_kind == VenueKind::Orderbook)
                    .then_some(fast_price + reference_orderbook_half_spread()),
            }
        })
        .collect();
    ReferenceSnapshot {
        ts_ms,
        topic: stream.publish_topic.clone(),
        fair_value: Some(fair_value),
        confidence: 1.0,
        venues,
    }
}

fn opening_reference_snapshot(loaded: &LoadedBoltV3Config, ts_ms: u64) -> ReferenceSnapshot {
    reference_snapshot(
        loaded,
        ts_ms,
        price_to_beat_value(),
        scenario_prices().opening_fast_price,
    )
}

fn entry_reference_snapshot(loaded: &LoadedBoltV3Config, ts_ms: u64) -> ReferenceSnapshot {
    reference_snapshot(
        loaded,
        ts_ms,
        scenario_prices().entry_fair_value,
        scenario_prices().entry_fast_price,
    )
}

fn exit_reference_snapshot(loaded: &LoadedBoltV3Config, ts_ms: u64) -> ReferenceSnapshot {
    reference_snapshot(
        loaded,
        ts_ms,
        scenario_prices().exit_fair_value,
        scenario_prices().exit_fast_price,
    )
}

fn instrument_id(condition_id: &str, token_id: &str, loaded: &LoadedBoltV3Config) -> InstrumentId {
    let execution_client_id = execution_client_id(loaded);
    let venue = loaded
        .root
        .clients
        .get(&execution_client_id)
        .expect("strategy execution client should exist")
        .venue
        .as_str();
    InstrumentId::from(format!("{condition_id}-{token_id}.{venue}").as_str())
}

fn selected_instruments(loaded: &LoadedBoltV3Config) -> (InstrumentId, InstrumentId) {
    let market = selected_market(loaded, 0);
    (
        instrument_id(&market.condition_id, &market.up_token_id, loaded),
        instrument_id(&market.condition_id, &market.down_token_id, loaded),
    )
}

fn binary_option(instrument_id: InstrumentId) -> InstrumentAny {
    let price_increment = Price::from(selected_binary_option_price_increment());
    let size_increment = Quantity::from(selected_binary_option_size_increment());
    let token_id = instrument_id
        .symbol
        .as_str()
        .rsplit_once('-')
        .map(|(_, token_id)| token_id)
        .unwrap_or_else(|| {
            panic!("polymarket fixture instrument id should include token id: {instrument_id}")
        });
    let raw_symbol = Symbol::new(token_id);
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        raw_symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        selected_binary_option_created_ts(),
        selected_binary_option_updated_ts(),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn add_selected_instruments(node: &mut LiveNode, loaded: &LoadedBoltV3Config) {
    let (up, down) = selected_instruments(loaded);
    let cache_handle = node.kernel().cache();
    let mut cache = cache_handle.borrow_mut();
    cache.add_instrument(binary_option(up)).unwrap();
    cache.add_instrument(binary_option(down)).unwrap();
}

fn book_deltas(instrument_id: InstrumentId, bid: f64, ask: f64) -> OrderBookDeltas {
    OrderBookDeltas::new(
        instrument_id,
        vec![
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(
                    OrderSide::Buy,
                    Price::new(bid, selected_binary_option_price_precision()),
                    Quantity::from(selected_binary_option_book_level_quantity()),
                    0,
                ),
                0,
                1,
                UnixNanos::default(),
                UnixNanos::default(),
            ),
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(
                    OrderSide::Sell,
                    Price::new(ask, selected_binary_option_price_precision()),
                    Quantity::from(selected_binary_option_book_level_quantity()),
                    0,
                ),
                0,
                2,
                UnixNanos::default(),
                UnixNanos::default(),
            ),
        ],
    )
}

fn entry_up_book_deltas(instrument_id: InstrumentId) -> OrderBookDeltas {
    book_deltas(
        instrument_id,
        scenario_prices().entry_up_bid,
        scenario_prices().entry_up_ask,
    )
}

fn down_book_deltas(instrument_id: InstrumentId) -> OrderBookDeltas {
    book_deltas(
        instrument_id,
        scenario_prices().down_bid,
        scenario_prices().down_ask,
    )
}

fn exit_up_book_deltas(instrument_id: InstrumentId) -> OrderBookDeltas {
    book_deltas(
        instrument_id,
        scenario_prices().exit_up_bid,
        scenario_prices().exit_up_ask,
    )
}

async fn wait_for_running(handle: &LiveNodeHandle) {
    while !handle.is_running() {
        sleep(order_lifecycle_poll_interval()).await;
    }
}

fn unix_nanos_from_milliseconds(ts_ms: u64) -> UnixNanos {
    let ts_nanos = Duration::from_millis(ts_ms).as_nanos();
    UnixNanos::from(u64::try_from(ts_nanos).expect("millisecond timestamp should fit UnixNanos"))
}

fn unix_milliseconds_from_nanos(ts_nanos: u64) -> u64 {
    let ts_millis = Duration::from_nanos(ts_nanos).as_millis();
    u64::try_from(ts_millis).expect("nanosecond timestamp should fit milliseconds")
}

fn send_rejected_to_exec_engine(
    loaded: &LoadedBoltV3Config,
    submission: &RecordedSubmitOrder,
    ts_event_ms: u64,
) {
    let account_id = execution_account_id(loaded, execution_client_id(loaded).as_str());
    let event = OrderRejected::new(
        TraderId::from(loaded.root.trader_id.as_str()),
        submission.strategy_id,
        submission.instrument_id,
        submission.client_order_id,
        AccountId::from(account_id.as_str()),
        Ustr::from("mock_execution_rejected"),
        UUID4::new(),
        unix_nanos_from_milliseconds(ts_event_ms),
        unix_nanos_from_milliseconds(ts_event_ms),
        false,
        false,
    );
    msgbus::send_order_event(
        MessagingSwitchboard::exec_engine_process(),
        OrderEventAny::Rejected(event),
    );
}

fn venue_order_id_for(submission: &RecordedSubmitOrder) -> VenueOrderId {
    let id = submission.client_order_id.to_string();
    VenueOrderId::from(id.as_str())
}

fn send_accepted_to_exec_engine(
    loaded: &LoadedBoltV3Config,
    submission: &RecordedSubmitOrder,
    ts_event_ms: u64,
) -> VenueOrderId {
    let account_id = execution_account_id(loaded, execution_client_id(loaded).as_str());
    let venue_order_id = venue_order_id_for(submission);
    let event = OrderAccepted::new(
        submission.trader_id,
        submission.strategy_id,
        submission.instrument_id,
        submission.client_order_id,
        venue_order_id,
        AccountId::from(account_id.as_str()),
        UUID4::new(),
        unix_nanos_from_milliseconds(ts_event_ms),
        unix_nanos_from_milliseconds(ts_event_ms),
        false,
    );
    msgbus::send_order_event(
        MessagingSwitchboard::exec_engine_process(),
        OrderEventAny::Accepted(event),
    );
    venue_order_id
}

fn send_filled_to_exec_engine(
    loaded: &LoadedBoltV3Config,
    submission: &RecordedSubmitOrder,
    venue_order_id: VenueOrderId,
    ts_event_ms: u64,
) {
    let account_id = execution_account_id(loaded, execution_client_id(loaded).as_str());
    let trade_id = submission.client_order_id.to_string();
    let position_id = submission.client_order_id.to_string();
    let event = OrderFilled::new(
        submission.trader_id,
        submission.strategy_id,
        submission.instrument_id,
        submission.client_order_id,
        venue_order_id,
        AccountId::from(account_id.as_str()),
        TradeId::from(trade_id.as_str()),
        submission.order_side,
        submission.order_type,
        submission.quantity,
        submission
            .price
            .expect("submitted entry order should carry limit price"),
        Currency::USDC(),
        LiquiditySide::Taker,
        UUID4::new(),
        unix_nanos_from_milliseconds(ts_event_ms),
        unix_nanos_from_milliseconds(ts_event_ms),
        false,
        Some(PositionId::from(position_id.as_str())),
        None,
    );
    msgbus::send_order_event(
        MessagingSwitchboard::exec_engine_process(),
        OrderEventAny::Filled(event),
    );
}

fn send_canceled_to_exec_engine(
    loaded: &LoadedBoltV3Config,
    submission: &RecordedSubmitOrder,
    venue_order_id: VenueOrderId,
    ts_event_ms: u64,
) {
    let account_id = execution_account_id(loaded, execution_client_id(loaded).as_str());
    let event = OrderCanceled::new(
        submission.trader_id,
        submission.strategy_id,
        submission.instrument_id,
        submission.client_order_id,
        UUID4::new(),
        unix_nanos_from_milliseconds(ts_event_ms),
        unix_nanos_from_milliseconds(ts_event_ms),
        false,
        Some(venue_order_id),
        Some(AccountId::from(account_id.as_str())),
    );
    msgbus::send_order_event(
        MessagingSwitchboard::exec_engine_process(),
        OrderEventAny::Canceled(event),
    );
}

fn submission_events(catalog_dir: &Path, configured_target_id: &str, event_type: &str) -> usize {
    let ids = vec![configured_target_id.to_string()];
    let event_dir = catalog_dir
        .join("data")
        .join("custom")
        .join(event_type)
        .join(configured_target_id);
    if !event_dir.exists() {
        return 0;
    }

    let mut files = std::fs::read_dir(event_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "parquet")
        })
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();

    files
        .into_iter()
        .map(|file| {
            ParquetDataCatalog::new(catalog_dir, None, None, None, None)
                .query_custom_data_dynamic(
                    event_type,
                    Some(&ids),
                    None,
                    None,
                    None,
                    Some(vec![file]),
                    true,
                )
                .unwrap_or_default()
                .len()
        })
        .sum()
}

fn entry_submission_events(catalog_dir: &Path, configured_target_id: &str) -> usize {
    submission_events(
        catalog_dir,
        configured_target_id,
        BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    )
}

fn exit_submission_events(catalog_dir: &Path, configured_target_id: &str) -> usize {
    submission_events(
        catalog_dir,
        configured_target_id,
        BOLT_V3_EXIT_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    )
}

#[test]
fn bolt_v3_existing_strategy_reaches_real_polymarket_submit_and_cancel_http_through_nt_livenode_run()
 {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    loaded.root.nautilus.delay_post_stop_seconds = 0;
    loaded.root.nautilus.timeout_disconnection_seconds = 1;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            let server = start_local_polymarket_execution_server().await;
            point_execution_to_local_polymarket_server(&mut loaded, &server);

            let strategy_id =
                StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
            let execution_client_id = ClientId::from(execution_client_id(&loaded).as_str());
            let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
                .expect("fixture secrets should resolve through fake SSM");
            let builder =
                make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
            let (builder, _summary) = register_bolt_v3_clients(
                builder,
                client_configs_with_real_polymarket_execution(&loaded, &resolved),
            )
            .expect("mock data plus real Polymarket execution should register through v3 boundary");
            let mut node = builder
                .build()
                .expect("mixed real-exec LiveNode should build");
            register_bolt_v3_reference_actors(&mut node, &loaded)
                .expect("v3 reference actors should register on mixed LiveNode");
            register_bolt_v3_strategies(&mut node, &loaded, &resolved)
                .expect("existing strategy should register from v3 TOML");
            add_selected_instruments(&mut node, &loaded);

            let handle = node.handle();
            let start_ts_ms = unix_milliseconds_from_nanos(
                node.kernel().clock().borrow().timestamp_ns().as_u64(),
            );
            let reference_topic = reference_publish_topic(&loaded);
            let (up, down) = selected_instruments(&loaded);
            let loaded_for_control = loaded.clone();
            let run_timeout = order_lifecycle_real_execution_run_timeout(&loaded);

            let (post_order, cancel_orders) = tokio::time::timeout(run_timeout, async move {
                let control = async {
                    wait_for_running(&handle).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &opening_reference_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().initial_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );
                    wait_for_local_clob_request_count(
                        &server,
                        local_polymarket_fee_rate_method(),
                        local_polymarket_fee_rate_path(),
                        local_polymarket_fee_requests_per_binary_market(),
                    )
                    .await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    let post_order = wait_for_local_clob_request(
                        &server,
                        local_polymarket_submit_order_method(),
                        local_polymarket_submit_order_path(),
                    )
                    .await;
                    sleep(order_lifecycle_post_order_cancel_delay()).await;
                    let cancel = CancelAllOrders::new(
                        TraderId::from(loaded_for_control.root.trader_id.as_str()),
                        Some(execution_client_id),
                        strategy_id,
                        up,
                        OrderSide::Buy,
                        UUID4::new(),
                        unix_nanos_from_milliseconds(offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().real_execution_cancel_command,
                        )),
                        None,
                    );
                    msgbus::send_trading_command(
                        MessagingSwitchboard::exec_engine_execute(),
                        TradingCommand::CancelAllOrders(cancel),
                    );
                    let cancel_orders = wait_for_local_clob_request(
                        &server,
                        local_polymarket_cancel_orders_method(),
                        local_polymarket_cancel_orders_path(),
                    )
                    .await;

                    handle.stop();
                    (post_order, cancel_orders)
                };
                let runner = async {
                    node.run()
                        .await
                        .expect("mixed real-exec node should stop cleanly");
                };
                let (observed, _) = tokio::join!(control, runner);
                observed
            })
            .await
            .expect("mixed real-exec LiveNode run should finish before timeout");

            assert_eq!(post_order.target, local_polymarket_submit_order_path());
            assert!(
                post_order.body.contains("\"owner\""),
                "real NT Polymarket submitter should send signed order body through local CLOB: {}",
                post_order.body
            );
            assert_eq!(cancel_orders.target, local_polymarket_cancel_orders_path());
            assert!(
                cancel_orders.body.contains(local_polymarket_order_id()),
                "real NT Polymarket cancel should reference accepted venue order id: {}",
                cancel_orders.body
            );
            assert!(recorded_mock_exec_submissions().is_empty());
        });
}

#[test]
fn bolt_v3_existing_strategy_reaches_mock_submit_through_nt_livenode_run() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) =
        spawn_fee_rate_server(local_polymarket_fee_requests_per_binary_market());
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let execution_client_id = ClientId::from(execution_client_id(&loaded).as_str());
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) =
        register_bolt_v3_clients(builder, mock_client_configs_from_loaded(&loaded))
            .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    add_selected_instruments(&mut node, &loaded);

    let handle = node.handle();
    let start_ts_ms =
        unix_milliseconds_from_nanos(node.kernel().clock().borrow().timestamp_ns().as_u64());
    let reference_topic = reference_publish_topic(&loaded);
    let (up, down) = selected_instruments(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let run_timeout = order_lifecycle_mock_run_timeout(&loaded);
    let loaded_for_control = loaded.clone();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tokio::time::timeout(run_timeout, async move {
                let control = async {
                    wait_for_running(&handle).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &opening_reference_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().initial_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );
                    sleep(order_lifecycle_post_initial_market_delay()).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if !recorded_mock_exec_submissions().is_empty() {
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    handle.stop();
                };

                let runner = async {
                    node.run().await.expect("mock node should stop cleanly");
                };

                tokio::join!(control, runner);
            })
            .await
            .expect("mock LiveNode run should finish before timeout");
        });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 1, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(execution_client_id));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert_eq!(entry_submission_events(&catalog_dir, &target_id), 1);
    for _ in 0..local_polymarket_fee_requests_per_binary_market() {
        let request = fee_requests
            .recv_timeout(order_lifecycle_fee_request_recv_timeout())
            .expect("local fee server should receive fee request");
        assert!(
            request
                .split_ascii_whitespace()
                .nth(1)
                .unwrap_or_default()
                .starts_with(local_polymarket_fee_rate_query_prefix()),
            "unexpected fee request path: {request:?}"
        );
    }
}

#[test]
fn bolt_v3_existing_strategy_recovers_after_nt_order_reject_event() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) =
        spawn_fee_rate_server(local_polymarket_fee_requests_per_binary_market());
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let execution_client_id = ClientId::from(execution_client_id(&loaded).as_str());
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) =
        register_bolt_v3_clients(builder, mock_client_configs_from_loaded(&loaded))
            .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    add_selected_instruments(&mut node, &loaded);

    let handle = node.handle();
    let start_ts_ms =
        unix_milliseconds_from_nanos(node.kernel().clock().borrow().timestamp_ns().as_u64());
    let reference_topic = reference_publish_topic(&loaded);
    let (up, down) = selected_instruments(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let run_timeout = order_lifecycle_mock_run_timeout(&loaded);
    let loaded_for_control = loaded.clone();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tokio::time::timeout(run_timeout, async move {
                let control = async {
                    wait_for_running(&handle).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &opening_reference_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().initial_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );
                    sleep(order_lifecycle_post_initial_market_delay()).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if let Some(submission) = recorded_mock_exec_submissions().first().cloned()
                        {
                            send_rejected_to_exec_engine(
                                &loaded_for_control,
                                &submission,
                                offset_ts_ms(
                                    start_ts_ms,
                                    timeline_offsets_milliseconds().entry_rejected_event,
                                ),
                            );
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }

                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_rejected_retry_snapshot,
                            ),
                        ),
                    );
                    publish_any(
                        reference_topic.into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_rejected_retry_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if recorded_mock_exec_submissions().len() >= 2 {
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    handle.stop();
                };

                let runner = async {
                    node.run().await.expect("mock node should stop cleanly");
                };

                tokio::join!(control, runner);
            })
            .await
            .expect("mock LiveNode run should finish before timeout");
        });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 2, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(execution_client_id));
    assert_eq!(submissions[1].client_id, Some(execution_client_id));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[1].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert_eq!(submissions[1].instrument_id, up);
    assert_ne!(
        submissions[0].client_order_id, submissions[1].client_order_id,
        "{submissions:?}"
    );
    assert_eq!(entry_submission_events(&catalog_dir, &target_id), 2);
    for _ in 0..local_polymarket_fee_requests_per_binary_market() {
        let request = fee_requests
            .recv_timeout(order_lifecycle_fee_request_recv_timeout())
            .expect("local fee server should receive fee request");
        assert!(
            request
                .split_ascii_whitespace()
                .nth(1)
                .unwrap_or_default()
                .starts_with(local_polymarket_fee_rate_query_prefix()),
            "unexpected fee request path: {request:?}"
        );
    }
}

#[test]
fn bolt_v3_existing_strategy_exits_after_nt_order_fill_event() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) =
        spawn_fee_rate_server(local_polymarket_fee_requests_per_binary_market());
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let execution_client_id = ClientId::from(execution_client_id(&loaded).as_str());
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) =
        register_bolt_v3_clients(builder, mock_client_configs_from_loaded(&loaded))
            .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    add_selected_instruments(&mut node, &loaded);

    let handle = node.handle();
    let start_ts_ms =
        unix_milliseconds_from_nanos(node.kernel().clock().borrow().timestamp_ns().as_u64());
    let reference_topic = reference_publish_topic(&loaded);
    let (up, down) = selected_instruments(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let run_timeout = order_lifecycle_mock_run_timeout(&loaded);
    let loaded_for_control = loaded.clone();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tokio::time::timeout(run_timeout, async move {
                let control = async {
                    wait_for_running(&handle).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &opening_reference_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().initial_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );
                    sleep(order_lifecycle_post_initial_market_delay()).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    let mut entry_submission = None;
                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if let Some(submission) = recorded_mock_exec_submissions().first().cloned()
                        {
                            entry_submission = Some(submission);
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    let entry_submission =
                        entry_submission.expect("entry submit should happen before fill event");
                    let venue_order_id = send_accepted_to_exec_engine(
                        &loaded_for_control,
                        &entry_submission,
                        offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().entry_accepted_event,
                        ),
                    );
                    send_filled_to_exec_engine(
                        &loaded_for_control,
                        &entry_submission,
                        venue_order_id,
                        offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().entry_filled_event,
                        ),
                    );

                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &freeze_selection_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().freeze_snapshot,
                            ),
                        ),
                    );
                    publish_any(
                        reference_topic.into(),
                        &exit_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().freeze_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &exit_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if recorded_mock_exec_submissions().len() >= 2 {
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    handle.stop();
                };

                let runner = async {
                    node.run().await.expect("mock node should stop cleanly");
                };

                tokio::join!(control, runner);
            })
            .await
            .expect("mock LiveNode run should finish before timeout");
        });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 2, "{submissions:?}");
    assert_eq!(submissions[0].client_id, Some(execution_client_id));
    assert_eq!(submissions[1].client_id, Some(execution_client_id));
    assert_eq!(submissions[0].strategy_id, strategy_id);
    assert_eq!(submissions[1].strategy_id, strategy_id);
    assert_eq!(submissions[0].instrument_id, up);
    assert_eq!(submissions[1].instrument_id, up);
    assert_eq!(submissions[0].order_side, OrderSide::Buy);
    assert_eq!(submissions[1].order_side, OrderSide::Sell);
    assert_eq!(submissions[1].quantity, submissions[0].quantity);
    assert_eq!(entry_submission_events(&catalog_dir, &target_id), 1);
    assert_eq!(exit_submission_events(&catalog_dir, &target_id), 1);
    for _ in 0..local_polymarket_fee_requests_per_binary_market() {
        let request = fee_requests
            .recv_timeout(order_lifecycle_fee_request_recv_timeout())
            .expect("local fee server should receive fee request");
        assert!(
            request
                .split_ascii_whitespace()
                .nth(1)
                .unwrap_or_default()
                .starts_with(local_polymarket_fee_rate_query_prefix()),
            "unexpected fee request path: {request:?}"
        );
    }
}

#[test]
fn bolt_v3_existing_strategy_resubmits_exit_after_nt_cancel_event() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) =
        spawn_fee_rate_server(local_polymarket_fee_requests_per_binary_market());
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let execution_client_id = ClientId::from(execution_client_id(&loaded).as_str());
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) =
        register_bolt_v3_clients(builder, mock_client_configs_from_loaded(&loaded))
            .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    add_selected_instruments(&mut node, &loaded);

    let handle = node.handle();
    let start_ts_ms =
        unix_milliseconds_from_nanos(node.kernel().clock().borrow().timestamp_ns().as_u64());
    let reference_topic = reference_publish_topic(&loaded);
    let (up, down) = selected_instruments(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let run_timeout = order_lifecycle_mock_run_timeout(&loaded);
    let loaded_for_control = loaded.clone();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            tokio::time::timeout(run_timeout, async move {
                let control = async {
                    wait_for_running(&handle).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &opening_reference_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().initial_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );
                    sleep(order_lifecycle_post_initial_market_delay()).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &entry_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().entry_reference_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &entry_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    let mut entry_submission = None;
                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if let Some(submission) = recorded_mock_exec_submissions().first().cloned()
                        {
                            entry_submission = Some(submission);
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    let entry_submission =
                        entry_submission.expect("entry submit should happen before fill event");
                    let entry_venue_order_id = send_accepted_to_exec_engine(
                        &loaded_for_control,
                        &entry_submission,
                        offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().entry_accepted_event,
                        ),
                    );
                    send_filled_to_exec_engine(
                        &loaded_for_control,
                        &entry_submission,
                        entry_venue_order_id,
                        offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().entry_filled_event,
                        ),
                    );

                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &freeze_selection_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().freeze_snapshot,
                            ),
                        ),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &exit_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().freeze_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &exit_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    let mut exit_submission = None;
                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        let submissions = recorded_mock_exec_submissions();
                        if submissions.len() >= 2 {
                            exit_submission = submissions.get(1).cloned();
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    let exit_submission =
                        exit_submission.expect("exit submit should happen before cancel event");
                    let exit_venue_order_id = send_accepted_to_exec_engine(
                        &loaded_for_control,
                        &exit_submission,
                        offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().exit_accepted_event,
                        ),
                    );
                    send_canceled_to_exec_engine(
                        &loaded_for_control,
                        &exit_submission,
                        exit_venue_order_id,
                        offset_ts_ms(
                            start_ts_ms,
                            timeline_offsets_milliseconds().exit_canceled_event,
                        ),
                    );

                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &freeze_selection_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().replacement_freeze_snapshot,
                            ),
                        ),
                    );
                    publish_any(
                        reference_topic.into(),
                        &exit_reference_snapshot(
                            &loaded_for_control,
                            offset_ts_ms(
                                start_ts_ms,
                                timeline_offsets_milliseconds().replacement_freeze_snapshot,
                            ),
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &exit_up_book_deltas(up),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &down_book_deltas(down),
                    );

                    for _ in 0..order_lifecycle_submit_poll_attempts() {
                        if recorded_mock_exec_submissions().len() >= 3 {
                            break;
                        }
                        sleep(order_lifecycle_poll_interval()).await;
                    }
                    handle.stop();
                };

                let runner = async {
                    node.run().await.expect("mock node should stop cleanly");
                };

                tokio::join!(control, runner);
            })
            .await
            .expect("mock LiveNode run should finish before timeout");
        });

    let submissions = recorded_mock_exec_submissions();
    assert_eq!(submissions.len(), 3, "{submissions:?}");
    for submission in &submissions {
        assert_eq!(submission.client_id, Some(execution_client_id));
        assert_eq!(submission.strategy_id, strategy_id);
        assert_eq!(submission.instrument_id, up);
    }
    assert_eq!(submissions[0].order_side, OrderSide::Buy);
    assert_eq!(submissions[1].order_side, OrderSide::Sell);
    assert_eq!(submissions[2].order_side, OrderSide::Sell);
    assert_eq!(submissions[1].quantity, submissions[0].quantity);
    assert_eq!(submissions[2].quantity, submissions[0].quantity);
    assert_ne!(
        submissions[1].client_order_id, submissions[2].client_order_id,
        "{submissions:?}"
    );
    assert_eq!(entry_submission_events(&catalog_dir, &target_id), 1);
    assert_eq!(exit_submission_events(&catalog_dir, &target_id), 2);
    for _ in 0..local_polymarket_fee_requests_per_binary_market() {
        let request = fee_requests
            .recv_timeout(order_lifecycle_fee_request_recv_timeout())
            .expect("local fee server should receive fee request");
        assert!(
            request
                .split_ascii_whitespace()
                .nth(1)
                .unwrap_or_default()
                .starts_with(local_polymarket_fee_rate_query_prefix()),
            "unexpected fee request path: {request:?}"
        );
    }
}
