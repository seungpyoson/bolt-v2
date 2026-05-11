//! Local bolt-v3 order-lifecycle tracer.
//!
//! Scope: v3 TOML -> mock NT LiveNode -> existing strategy -> mock submit capture.
//! This is not venue live-readiness, fill/reject/cancel, or reconciliation proof.

mod support;

use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
    time::Duration,
};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3ClientConfig, BoltV3ClientConfigs, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig,
    },
    bolt_v3_client_registration::register_bolt_v3_clients,
    bolt_v3_config::{
        LoadedBoltV3Config, REFERENCE_STREAM_ID_PARAMETER, ReferenceSourceType, load_bolt_v3_config,
    },
    bolt_v3_decision_events::BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    bolt_v3_live_node::make_bolt_v3_live_node_builder,
    bolt_v3_reference_actor_registration::register_bolt_v3_reference_actors,
    bolt_v3_release_identity::{bolt_v3_compiled_nautilus_trader_revision, bolt_v3_config_hash},
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_strategy_registration::register_bolt_v3_strategies,
    platform::{
        reference::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind},
        resolution_basis::parse_ruleset_resolution_basis,
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState},
        runtime::runtime_selection_topic,
    },
};
use nautilus_common::{
    msgbus::switchboard::MessagingSwitchboard,
    msgbus::{self, publish_any, publish_deltas, switchboard},
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::{LiveNode, LiveNodeHandle};
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, OrderBookDeltas},
    enums::{AssetClass, BookAction, OrderSide},
    events::{OrderEventAny, OrderRejected},
    identifiers::{AccountId, ClientId, InstrumentId, StrategyId, TraderId},
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    RecordedSubmitOrder, clear_mock_exec_submissions, recorded_mock_exec_submissions,
};
use tempfile::TempDir;
use tokio::time::sleep;
use ustr::Ustr;

static RUNTIME_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn runtime_test_mutex() -> &'static Mutex<()> {
    RUNTIME_TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

fn existing_strategy_root_fixture() -> PathBuf {
    support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml")
}

fn attach_release_identity_manifest(loaded: &mut LoadedBoltV3Config, temp_dir: &TempDir) {
    let config_hash = bolt_v3_config_hash(loaded).expect("fixture config hash should compute");
    let nt_revision = bolt_v3_compiled_nautilus_trader_revision()
        .expect("fixture NT revision should resolve from Cargo.toml");
    let manifest_path = temp_dir.path().join("release-identity.toml");
    std::fs::write(
        &manifest_path,
        format!(
            r#"
release_id = "test-release"
git_commit_sha = "test-git-sha"
nautilus_trader_revision = "{nt_revision}"
binary_sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
cargo_lock_sha256 = "2222222222222222222222222222222222222222222222222222222222222222"
config_hash = "{config_hash}"
build_profile = "test"

[artifact_sha256]
bolt_v2 = "3333333333333333333333333333333333333333333333333333333333333333"
"#,
        ),
    )
    .expect("release identity manifest should write");
    loaded.root.release.identity_manifest_path = manifest_path.to_string_lossy().into_owned();
    let catalog_dir = temp_dir.path().join("catalog");
    std::fs::create_dir_all(&catalog_dir).expect("catalog dir should create");
    loaded.root.persistence.catalog_directory = catalog_dir.to_string_lossy().into_owned();
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
    let listener = TcpListener::bind("127.0.0.1:0").expect("local fee server should bind");
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();

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

            let body = r#"{"base_fee":"0"}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("local fee server should write response");
        }
    });

    (base_url, rx)
}

fn selected_market(loaded: &LoadedBoltV3Config, start_ts_ms: u64) -> CandidateMarket {
    let market_id = configured_target_id(loaded);
    let underlying = underlying_asset(loaded);
    let condition_id = format!("condition-{}", underlying.to_ascii_lowercase());
    let up_token_id = format!("{market_id}-UP");
    let down_token_id = format!("{market_id}-DOWN");
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
        liquidity_num: 1000.0,
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
    3_100.0
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
                observed_bid: (venue_kind == VenueKind::Orderbook).then_some(fast_price - 0.5),
                observed_ask: (venue_kind == VenueKind::Orderbook).then_some(fast_price + 0.5),
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
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        instrument_id.symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(1_u64),
        UnixNanos::from(2_u64),
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
                BookOrder::new(OrderSide::Buy, Price::new(bid, 3), Quantity::from("100"), 0),
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
                    Price::new(ask, 3),
                    Quantity::from("100"),
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

async fn wait_for_running(handle: &LiveNodeHandle) {
    while !handle.is_running() {
        sleep(Duration::from_millis(10)).await;
    }
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
        UnixNanos::from(ts_event_ms * 1_000_000),
        UnixNanos::from(ts_event_ms * 1_000_000),
        false,
        false,
    );
    msgbus::send_order_event(
        MessagingSwitchboard::exec_engine_process(),
        OrderEventAny::Rejected(event),
    );
}

fn submission_events(catalog_dir: &Path, configured_target_id: &str) -> usize {
    let ids = vec![configured_target_id.to_string()];
    let event_dir = catalog_dir
        .join("data")
        .join("custom")
        .join(BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE)
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
                    BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
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

#[test]
fn bolt_v3_existing_strategy_reaches_mock_submit_through_nt_livenode_run() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) = spawn_fee_rate_server(2);
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    attach_release_identity_manifest(&mut loaded, &temp_dir);

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
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let reference_topic = reference_publish_topic(&loaded);
    let (up, down) = selected_instruments(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let run_timeout = Duration::from_secs(
        loaded.root.nautilus.delay_post_stop_seconds
            + loaded.root.nautilus.timeout_shutdown_seconds
            + 1,
    );
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
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms,
                            price_to_beat_value(),
                            3_102.0,
                        ),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms + 200,
                            3_101.0,
                            3_105.0,
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &book_deltas(up, 0.430, 0.450),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &book_deltas(down, 0.480, 0.490),
                    );
                    sleep(Duration::from_millis(200)).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.into(),
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms + 400,
                            3_101.0,
                            3_105.0,
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &book_deltas(up, 0.430, 0.450),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &book_deltas(down, 0.480, 0.490),
                    );

                    for _ in 0..50 {
                        if !recorded_mock_exec_submissions().is_empty() {
                            break;
                        }
                        sleep(Duration::from_millis(10)).await;
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
    assert_eq!(submission_events(&catalog_dir, &target_id), 1);
    for _ in 0..2 {
        let request = fee_requests
            .recv_timeout(Duration::from_secs(1))
            .expect("local fee server should receive fee request");
        assert!(
            request
                .split_ascii_whitespace()
                .nth(1)
                .unwrap_or_default()
                .starts_with("/fee-rate?token_id="),
            "unexpected fee request path: {request:?}"
        );
    }
}

#[test]
fn bolt_v3_existing_strategy_recovers_after_nt_order_reject_event() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) = spawn_fee_rate_server(2);
    let mut loaded =
        load_bolt_v3_config(&existing_strategy_root_fixture()).expect("v3 fixture should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = false;
    loaded.root.nautilus.save_state = false;
    attach_release_identity_manifest(&mut loaded, &temp_dir);

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
    let start_ts_ms = node.kernel().clock().borrow().timestamp_ns().as_u64() / 1_000_000;
    let reference_topic = reference_publish_topic(&loaded);
    let (up, down) = selected_instruments(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let run_timeout = Duration::from_secs(
        loaded.root.nautilus.delay_post_stop_seconds
            + loaded.root.nautilus.timeout_shutdown_seconds
            + 1,
    );
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
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms,
                            price_to_beat_value(),
                            3_102.0,
                        ),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms + 200,
                            3_101.0,
                            3_105.0,
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &book_deltas(up, 0.430, 0.450),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &book_deltas(down, 0.480, 0.490),
                    );
                    sleep(Duration::from_millis(200)).await;
                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms),
                    );
                    publish_any(
                        reference_topic.clone().into(),
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms + 400,
                            3_101.0,
                            3_105.0,
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &book_deltas(up, 0.430, 0.450),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &book_deltas(down, 0.480, 0.490),
                    );

                    for _ in 0..50 {
                        if let Some(submission) = recorded_mock_exec_submissions().first().cloned()
                        {
                            send_rejected_to_exec_engine(
                                &loaded_for_control,
                                &submission,
                                start_ts_ms + 300,
                            );
                            break;
                        }
                        sleep(Duration::from_millis(10)).await;
                    }

                    publish_any(
                        runtime_selection_topic(&strategy_id).into(),
                        &selection_snapshot(&loaded_for_control, start_ts_ms + 600),
                    );
                    publish_any(
                        reference_topic.into(),
                        &reference_snapshot(
                            &loaded_for_control,
                            start_ts_ms + 600,
                            3_101.0,
                            3_105.0,
                        ),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(up),
                        &book_deltas(up, 0.430, 0.450),
                    );
                    publish_deltas(
                        switchboard::get_book_deltas_topic(down),
                        &book_deltas(down, 0.480, 0.490),
                    );

                    for _ in 0..50 {
                        if recorded_mock_exec_submissions().len() >= 2 {
                            break;
                        }
                        sleep(Duration::from_millis(10)).await;
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
    assert_eq!(submission_events(&catalog_dir, &target_id), 2);
    for _ in 0..2 {
        let request = fee_requests
            .recv_timeout(Duration::from_secs(1))
            .expect("local fee server should receive fee request");
        assert!(
            request
                .split_ascii_whitespace()
                .nth(1)
                .unwrap_or_default()
                .starts_with("/fee-rate?token_id="),
            "unexpected fee request path: {request:?}"
        );
    }
}
