mod support;

use std::{
    collections::BTreeMap,
    env,
    fmt::Display,
    fs,
    io::Write,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    str::FromStr,
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
    bolt_v3_decision_events::{
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE, BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY,
        BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON,
        BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
        BOLT_V3_HAS_SELECTED_MARKET_OPEN_ORDERS_FACT_KEY,
        BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON_FACT_KEY,
        BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON,
        BoltV3EntryEvaluationDecisionEvent,
    },
    bolt_v3_live_node::{make_bolt_v3_live_node_builder, make_live_node_config},
    bolt_v3_reference_actor_registration::register_bolt_v3_reference_actors,
    bolt_v3_release_identity::bolt_v3_compiled_nautilus_trader_revision,
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_strategy_registration::register_bolt_v3_strategies,
    platform::{
        reference::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind},
        resolution_basis::parse_ruleset_resolution_basis,
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState},
        runtime::runtime_selection_topic,
    },
};
use nautilus_common::msgbus::{publish_any, publish_deltas, switchboard};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_live::node::{LiveNode, NodeState};
use nautilus_model::{
    accounts::AccountAny,
    data::{BookOrder, OrderBookDelta, OrderBookDeltas},
    enums::{AccountType, AssetClass, BookAction, OrderSide, OrderStatus, OrderType, TimeInForce},
    events::AccountState,
    identifiers::{AccountId, ClientId, InstrumentId, StrategyId, Symbol, Venue, VenueOrderId},
    instruments::{InstrumentAny, binary_option::BinaryOption},
    reports::{ExecutionMassStatus, OrderStatusReport},
    types::{AccountBalance, Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde::Deserialize;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_exec_submissions, clear_mock_external_order_registrations,
    recorded_mock_exec_submissions, recorded_mock_external_order_registrations,
};
use tempfile::TempDir;
use tokio::time::sleep;

static RUNTIME_TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
const NANOSECONDS_PER_MILLISECOND: u64 = 1_000_000;
const RECONCILIATION_PROCESS_HELPER_ENV: &str = "BOLT_V3_RECONCILIATION_PROCESS_HELPER";
const RECONCILIATION_PROCESS_MODE_ENV: &str = "BOLT_V3_RECONCILIATION_PROCESS_MODE";
const RECONCILIATION_PROCESS_MARKER_PATH_ENV: &str = "BOLT_V3_RECONCILIATION_PROCESS_MARKER_PATH";

#[derive(Debug, Deserialize)]
struct OpenOrderFixture {
    load_state: bool,
    save_state: bool,
    delay_post_stop_seconds: u64,
    timeout_disconnection_seconds: u64,
    timeout_reconciliation_seconds: u64,
    reconciliation_startup_delay_seconds: u64,
    account_type: String,
    account_base_currency: String,
    account_total: String,
    account_locked: String,
    account_free: String,
    condition_id: String,
    up_token_id: String,
    down_token_id: String,
    price_increment: String,
    size_increment: String,
    asset_class: String,
    currency: String,
    order_side: String,
    order_type: String,
    time_in_force: String,
    order_status: String,
    order_price: String,
    order_quantity: String,
    filled_quantity: String,
    venue_order_id: String,
    activation_ts_ns: u64,
    expiration_ts_ns: u64,
    report_ts_ns: u64,
    question_id: String,
    selection_liquidity_num: f64,
    reference_initial_fair_value: f64,
    reference_initial_orderbook_bid: f64,
    reference_initial_orderbook_ask: f64,
    reference_next_fair_value: f64,
    reference_next_orderbook_bid: f64,
    reference_next_orderbook_ask: f64,
    signal_second_offset_ms: u64,
    signal_final_offset_ms: u64,
    signal_settle_milliseconds: u64,
    signal_poll_iterations: usize,
    signal_poll_interval_milliseconds: u64,
    fee_expected_requests: usize,
    process_helper_seed_mode: String,
    process_helper_restart_mode: String,
    process_exit_seed_status_code: i32,
    process_exit_seed_marker_file: String,
    process_exit_restart_marker_file: String,
    process_exit_seed_marker_text: String,
    process_exit_restart_marker_text: String,
    up_book_bid: String,
    up_book_ask: String,
    down_book_bid: String,
    down_book_ask: String,
    book_quantity: String,
}

#[test]
fn bolt_v3_maps_toml_reconciliation_settings_to_nt_live_config() {
    let loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    let config = make_live_node_config(&loaded);
    let expected = &loaded.root.nautilus.exec_engine;

    assert_eq!(config.exec_engine.reconciliation, expected.reconciliation);
    assert_eq!(
        config.exec_engine.reconciliation_startup_delay_secs,
        expected.reconciliation_startup_delay_seconds as f64
    );
    assert_eq!(
        config.exec_engine.filter_unclaimed_external_orders,
        expected.filter_unclaimed_external_orders
    );
    assert_eq!(
        config.exec_engine.generate_missing_orders,
        expected.generate_missing_orders
    );
}

#[test]
fn bolt_v3_startup_reconciliation_imports_external_open_order_into_nt_cache() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_external_order_registrations();
    let temp_dir = TempDir::new().unwrap();
    let open_order = open_order_fixture();
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    loaded.root.nautilus.load_state = open_order.load_state;
    loaded.root.nautilus.save_state = open_order.save_state;
    loaded.root.nautilus.delay_post_stop_seconds = open_order.delay_post_stop_seconds;
    loaded.root.nautilus.timeout_disconnection_seconds = open_order.timeout_disconnection_seconds;
    loaded.root.nautilus.timeout_reconciliation_seconds = open_order.timeout_reconciliation_seconds;
    loaded
        .root
        .nautilus
        .exec_engine
        .reconciliation_startup_delay_seconds = open_order.reconciliation_startup_delay_seconds;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let up = instrument_id(&open_order.condition_id, &open_order.up_token_id, &loaded);
    let down = instrument_id(&open_order.condition_id, &open_order.down_token_id, &loaded);
    let instruments = vec![
        binary_option(up, &open_order.up_token_id, &open_order),
        binary_option(down, &open_order.down_token_id, &open_order),
    ];
    let mass_status = external_open_order_mass_status(&loaded, up, &open_order);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) = register_bolt_v3_clients(
        builder,
        mock_client_configs_from_loaded(&loaded, instruments, mass_status),
    )
    .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    node.kernel()
        .cache()
        .borrow_mut()
        .add_account(account_from_fixture(&loaded, &open_order))
        .expect("mock account should seed NT cache before startup reconciliation");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            node.start()
                .await
                .expect("mock-only LiveNode start should run startup reconciliation");
            assert_eq!(node.state(), NodeState::Running);

            let registrations = recorded_mock_external_order_registrations();
            assert_eq!(registrations.len(), 1);
            assert_eq!(
                registrations[0].venue_order_id,
                VenueOrderId::from(open_order.venue_order_id.as_str())
            );
            assert_eq!(registrations[0].instrument_id, up);

            {
                let cache_handle = node.kernel().cache();
                let cache = cache_handle.borrow();
                let order_side = fixture_value("order_side", &open_order.order_side);
                let open_orders = cache.orders_open(None, Some(&up), None, None, Some(order_side));
                assert_eq!(
                    open_orders.len(),
                    1,
                    "startup reconciliation should import external open order into NT cache"
                );
            }

            node.stop()
                .await
                .expect("mock-only LiveNode stop should succeed");
        });
}

#[test]
fn bolt_v3_reconciled_open_order_blocks_duplicate_entry_submit() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_external_order_registrations();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let open_order = open_order_fixture();
    let (fee_base_url, fee_requests) = spawn_fee_rate_server(open_order.fee_expected_requests);
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    point_execution_http_to_local_fee_server(&mut loaded, fee_base_url);
    loaded.root.nautilus.load_state = open_order.load_state;
    loaded.root.nautilus.save_state = open_order.save_state;
    loaded.root.nautilus.delay_post_stop_seconds = open_order.delay_post_stop_seconds;
    loaded.root.nautilus.timeout_disconnection_seconds = open_order.timeout_disconnection_seconds;
    loaded.root.nautilus.timeout_reconciliation_seconds = open_order.timeout_reconciliation_seconds;
    loaded
        .root
        .nautilus
        .exec_engine
        .reconciliation_startup_delay_seconds = open_order.reconciliation_startup_delay_seconds;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());

    let up = instrument_id(&open_order.condition_id, &open_order.up_token_id, &loaded);
    let down = instrument_id(&open_order.condition_id, &open_order.down_token_id, &loaded);
    let instruments = vec![
        binary_option(up, &open_order.up_token_id, &open_order),
        binary_option(down, &open_order.down_token_id, &open_order),
    ];
    let mass_status = external_open_order_mass_status(&loaded, up, &open_order);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) = register_bolt_v3_clients(
        builder,
        mock_client_configs_from_loaded(&loaded, instruments, mass_status),
    )
    .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    node.kernel()
        .cache()
        .borrow_mut()
        .add_account(account_from_fixture(&loaded, &open_order))
        .expect("mock account should seed NT cache before startup reconciliation");

    let start_ts_ms = node_clock_timestamp_ms(&node);
    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let selection_topic = runtime_selection_topic(&strategy_id);
    let reference_topic = reference_publish_topic(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let loaded_for_control = loaded.clone();
    let fee_expected_requests = open_order.fee_expected_requests;
    let fee_request_timeout = Duration::from_secs(open_order.timeout_disconnection_seconds);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            node.start()
                .await
                .expect("mock-only LiveNode start should run startup reconciliation");
            assert_eq!(node.state(), NodeState::Running);

            publish_any(
                selection_topic.clone().into(),
                &selection_snapshot(&loaded_for_control, &open_order, start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(
                    &loaded_for_control,
                    &open_order,
                    start_ts_ms,
                    open_order.reference_initial_fair_value,
                    open_order.reference_initial_orderbook_bid,
                    open_order.reference_initial_orderbook_ask,
                ),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(
                    &loaded_for_control,
                    &open_order,
                    start_ts_ms + open_order.signal_second_offset_ms,
                    open_order.reference_next_fair_value,
                    open_order.reference_next_orderbook_bid,
                    open_order.reference_next_orderbook_ask,
                ),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(
                    up,
                    &open_order.up_book_bid,
                    &open_order.up_book_ask,
                    &open_order.book_quantity,
                ),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(
                    down,
                    &open_order.down_book_bid,
                    &open_order.down_book_ask,
                    &open_order.book_quantity,
                ),
            );
            sleep(Duration::from_millis(open_order.signal_settle_milliseconds)).await;
            tokio::task::spawn_blocking(move || {
                for _ in 0..fee_expected_requests {
                    fee_requests
                        .recv_timeout(fee_request_timeout)
                        .expect("local fee server should receive fee request");
                }
            })
            .await
            .expect("fee request waiter should join");

            publish_any(
                selection_topic.clone().into(),
                &selection_snapshot(&loaded_for_control, &open_order, start_ts_ms),
            );
            publish_any(
                reference_topic.clone().into(),
                &reference_snapshot(
                    &loaded_for_control,
                    &open_order,
                    start_ts_ms + open_order.signal_final_offset_ms,
                    open_order.reference_next_fair_value,
                    open_order.reference_next_orderbook_bid,
                    open_order.reference_next_orderbook_ask,
                ),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(up),
                &book_deltas(
                    up,
                    &open_order.up_book_bid,
                    &open_order.up_book_ask,
                    &open_order.book_quantity,
                ),
            );
            publish_deltas(
                switchboard::get_book_deltas_topic(down),
                &book_deltas(
                    down,
                    &open_order.down_book_bid,
                    &open_order.down_book_ask,
                    &open_order.book_quantity,
                ),
            );

            for _ in 0..open_order.signal_poll_iterations {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(
                    open_order.signal_poll_interval_milliseconds,
                ))
                .await;
            }

            node.stop()
                .await
                .expect("mock-only LiveNode stop should succeed");
        });

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "reconciled external open order should block duplicate entry submit"
    );
    assert_eq!(
        entry_submission_events(&catalog_dir, &target_id),
        0,
        "reconciled external open order should block duplicate entry order-submission evidence"
    );
    assert!(
        selected_open_order_entry_evaluation_events(&catalog_dir, &target_id) > 0,
        "reconciled external open order should persist selected-open-order entry no-action evidence"
    );
}

#[test]
fn bolt_v3_restarted_node_blocks_duplicate_entry_after_reconciliation() {
    let _guard = runtime_test_mutex().lock().unwrap();
    clear_mock_external_order_registrations();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let open_order = open_order_fixture();
    let (fee_base_url, fee_requests) = spawn_fee_rate_server(open_order.fee_expected_requests);
    let loaded = load_reconciliation_config(temp_dir.path(), &open_order, Some(fee_base_url));

    let up = instrument_id(&open_order.condition_id, &open_order.up_token_id, &loaded);
    let down = instrument_id(&open_order.condition_id, &open_order.down_token_id, &loaded);
    let mut first_node = build_reconciliation_node(&loaded, &open_order, up, down);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            first_node
                .start()
                .await
                .expect("first mock-only LiveNode start should run startup reconciliation");
            assert_eq!(first_node.state(), NodeState::Running);
            assert_reconciled_open_order(&first_node, up, &open_order);
            first_node
                .stop()
                .await
                .expect("first mock-only LiveNode stop should succeed");
        });
    drop(first_node);
    clear_mock_external_order_registrations();

    let mut restarted_node = build_reconciliation_node(&loaded, &open_order, up, down);
    let start_ts_ms = node_clock_timestamp_ms(&restarted_node);
    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let selection_topic = runtime_selection_topic(&strategy_id);
    let reference_topic = reference_publish_topic(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let loaded_for_control = loaded.clone();
    let fee_expected_requests = open_order.fee_expected_requests;
    let fee_request_timeout = Duration::from_secs(open_order.timeout_disconnection_seconds);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            restarted_node
                .start()
                .await
                .expect("restarted mock-only LiveNode start should run startup reconciliation");
            assert_eq!(restarted_node.state(), NodeState::Running);
            assert_reconciled_open_order(&restarted_node, up, &open_order);
            let registrations = recorded_mock_external_order_registrations();
            assert_eq!(
                registrations.len(),
                1,
                "restarted node should register the reconciled external order once"
            );

            publish_entry_signal(
                &loaded_for_control,
                &open_order,
                start_ts_ms,
                up,
                down,
                &selection_topic,
                &reference_topic,
                open_order.reference_initial_fair_value,
                open_order.reference_initial_orderbook_bid,
                open_order.reference_initial_orderbook_ask,
            );
            sleep(Duration::from_millis(open_order.signal_settle_milliseconds)).await;
            tokio::task::spawn_blocking(move || {
                for _ in 0..fee_expected_requests {
                    fee_requests
                        .recv_timeout(fee_request_timeout)
                        .expect("local fee server should receive fee request");
                }
            })
            .await
            .expect("fee request waiter should join");
            publish_entry_signal(
                &loaded_for_control,
                &open_order,
                start_ts_ms + open_order.signal_final_offset_ms,
                up,
                down,
                &selection_topic,
                &reference_topic,
                open_order.reference_next_fair_value,
                open_order.reference_next_orderbook_bid,
                open_order.reference_next_orderbook_ask,
            );

            for _ in 0..open_order.signal_poll_iterations {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(
                    open_order.signal_poll_interval_milliseconds,
                ))
                .await;
            }

            restarted_node
                .stop()
                .await
                .expect("restarted mock-only LiveNode stop should succeed");
        });

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "restarted reconciled external open order should block duplicate entry submit"
    );
    assert_eq!(
        entry_submission_events(&catalog_dir, &target_id),
        0,
        "restarted reconciled external open order should block duplicate entry order-submission evidence"
    );
    assert!(
        selected_open_order_entry_evaluation_events(&catalog_dir, &target_id) > 0,
        "restarted reconciled external open order should persist selected-open-order entry no-action evidence"
    );
}

#[test]
fn bolt_v3_process_death_restart_blocks_duplicate_entry_after_reconciliation() {
    let _guard = runtime_test_mutex().lock().unwrap();
    let temp_dir = TempDir::new().unwrap();
    let open_order = open_order_fixture();
    let seed_marker_path = temp_dir
        .path()
        .join(&open_order.process_exit_seed_marker_file);
    let restart_marker_path = temp_dir
        .path()
        .join(&open_order.process_exit_restart_marker_file);

    let seed_status =
        run_reconciliation_process_helper(&open_order.process_helper_seed_mode, &seed_marker_path);
    assert!(
        !seed_status.success(),
        "seed helper should terminate without orderly LiveNode stop; got {seed_status:?}"
    );
    assert_eq!(
        fs::read_to_string(&seed_marker_path).expect("seed marker should be written before exit"),
        open_order.process_exit_seed_marker_text
    );

    let restart_status = run_reconciliation_process_helper(
        &open_order.process_helper_restart_mode,
        &restart_marker_path,
    );
    assert!(
        restart_status.success(),
        "restart helper should prove duplicate-submit block after process death; got {restart_status:?}"
    );
    assert_eq!(
        fs::read_to_string(&restart_marker_path)
            .expect("restart marker should be written after duplicate-submit proof"),
        open_order.process_exit_restart_marker_text
    );
}

#[test]
fn bolt_v3_reconciliation_process_helper() {
    if env::var(RECONCILIATION_PROCESS_HELPER_ENV).is_err() {
        return;
    }
    let mode = env::var(RECONCILIATION_PROCESS_MODE_ENV)
        .expect("process helper mode env should be configured");
    let marker_path = PathBuf::from(
        env::var(RECONCILIATION_PROCESS_MARKER_PATH_ENV)
            .expect("process helper marker path env should be configured"),
    );
    let open_order = open_order_fixture();
    if mode == open_order.process_helper_seed_mode {
        run_reconciliation_seed_process_before_exit(&open_order, &marker_path);
    }
    if mode == open_order.process_helper_restart_mode {
        run_reconciliation_restart_process_duplicate_submit_proof(&open_order, &marker_path);
        return;
    }
    panic!("unknown reconciliation process helper mode: {mode}");
}

#[test]
fn pinned_nt_startup_reconciliation_registers_external_orders_with_execution_clients() {
    let nt_root = pinned_nt_checkout();
    let node_source = fs::read_to_string(nt_root.join("crates/live/src/node.rs"))
        .expect("pinned NT live node source should read");
    let reconciliation_block = node_source
        .split("async fn perform_startup_reconciliation")
        .nth(1)
        .expect("pinned NT live node should define startup reconciliation")
        .split("async fn run_reconciliation_checks")
        .next()
        .expect("startup reconciliation block should precede executor init");

    assert!(
        reconciliation_block.contains("reconcile_execution_mass_status"),
        "NT startup reconciliation should reconcile mass status through live manager"
    );
    assert!(
        reconciliation_block.contains("result.external_orders"),
        "NT startup reconciliation should surface external orders from reconciliation result"
    );
    assert!(
        reconciliation_block.contains("exec_engine.register_external_order"),
        "NT startup reconciliation should hand external orders back to execution clients"
    );
}

#[test]
fn pinned_nt_polymarket_can_generate_mass_status_but_does_not_track_external_orders() {
    let nt_root = pinned_nt_checkout();
    let source =
        fs::read_to_string(nt_root.join("crates/adapters/polymarket/src/execution/mod.rs"))
            .expect("pinned NT Polymarket execution source should read");
    let mass_status_method = source
        .split("async fn generate_mass_status")
        .nth(1)
        .expect("Polymarket execution client should implement mass status generation")
        .split("fn process_cancel_result")
        .next()
        .expect("mass status method should precede cancel helper");

    assert!(
        mass_status_method.contains("reconciliation::generate_mass_status"),
        "Polymarket adapter should delegate mass-status generation to its reconciliation module"
    );

    let register_method = source
        .split("fn register_external_order")
        .nth(1)
        .expect("Polymarket execution client should implement external-order registration hook")
        .split("fn on_instrument")
        .next()
        .expect("external-order registration hook should precede instrument callback");
    let register_body = register_method
        .split_once('{')
        .and_then(|(_, rest)| rest.rsplit_once('}').map(|(body, _)| body))
        .expect("external-order registration hook body should parse");
    let non_empty_lines = register_body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    assert!(
        non_empty_lines.is_empty(),
        "Polymarket external-order registration hook should currently be empty; \
         if upstream NT implements this, F10 blocker status must be re-evaluated: {non_empty_lines:?}"
    );
}

fn runtime_test_mutex() -> &'static Mutex<()> {
    RUNTIME_TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

fn run_reconciliation_process_helper(mode: &str, marker_path: &Path) -> ExitStatus {
    Command::new(env::current_exe().expect("current test binary should be available"))
        .arg("--exact")
        .arg("bolt_v3_reconciliation_process_helper")
        .arg("--nocapture")
        .env(RECONCILIATION_PROCESS_HELPER_ENV, mode)
        .env(RECONCILIATION_PROCESS_MODE_ENV, mode)
        .env(RECONCILIATION_PROCESS_MARKER_PATH_ENV, marker_path)
        .status()
        .expect("reconciliation process helper should run")
}

fn run_reconciliation_seed_process_before_exit(open_order: &OpenOrderFixture, marker_path: &Path) {
    clear_mock_external_order_registrations();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let loaded = load_reconciliation_config(temp_dir.path(), open_order, None);
    let up = instrument_id(&open_order.condition_id, &open_order.up_token_id, &loaded);
    let down = instrument_id(&open_order.condition_id, &open_order.down_token_id, &loaded);
    let mut node = build_reconciliation_node(&loaded, open_order, up, down);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            node.start()
                .await
                .expect("seed process LiveNode start should run startup reconciliation");
            assert_eq!(node.state(), NodeState::Running);
            assert_reconciled_open_order(&node, up, open_order);
            fs::write(marker_path, &open_order.process_exit_seed_marker_text)
                .expect("seed process marker should write");
        });

    std::process::exit(open_order.process_exit_seed_status_code);
}

fn run_reconciliation_restart_process_duplicate_submit_proof(
    open_order: &OpenOrderFixture,
    marker_path: &Path,
) {
    clear_mock_external_order_registrations();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let (fee_base_url, fee_requests) = spawn_fee_rate_server(open_order.fee_expected_requests);
    let loaded = load_reconciliation_config(temp_dir.path(), open_order, Some(fee_base_url));
    let up = instrument_id(&open_order.condition_id, &open_order.up_token_id, &loaded);
    let down = instrument_id(&open_order.condition_id, &open_order.down_token_id, &loaded);
    let mut node = build_reconciliation_node(&loaded, open_order, up, down);
    let start_ts_ms = node_clock_timestamp_ms(&node);
    let strategy_id = StrategyId::from(strategy_config(&loaded).strategy_instance_id.as_str());
    let selection_topic = runtime_selection_topic(&strategy_id);
    let reference_topic = reference_publish_topic(&loaded);
    let catalog_dir = PathBuf::from(loaded.root.persistence.catalog_directory.clone());
    let target_id = configured_target_id(&loaded);
    let fee_expected_requests = open_order.fee_expected_requests;
    let fee_request_timeout = Duration::from_secs(open_order.timeout_disconnection_seconds);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            node.start()
                .await
                .expect("restart process LiveNode start should run startup reconciliation");
            assert_eq!(node.state(), NodeState::Running);
            assert_reconciled_open_order(&node, up, open_order);
            publish_entry_signal(
                &loaded,
                open_order,
                start_ts_ms,
                up,
                down,
                &selection_topic,
                &reference_topic,
                open_order.reference_initial_fair_value,
                open_order.reference_initial_orderbook_bid,
                open_order.reference_initial_orderbook_ask,
            );
            sleep(Duration::from_millis(open_order.signal_settle_milliseconds)).await;
            tokio::task::spawn_blocking(move || {
                for _ in 0..fee_expected_requests {
                    fee_requests
                        .recv_timeout(fee_request_timeout)
                        .expect("local fee server should receive fee request");
                }
            })
            .await
            .expect("fee request waiter should join");
            publish_entry_signal(
                &loaded,
                open_order,
                start_ts_ms + open_order.signal_final_offset_ms,
                up,
                down,
                &selection_topic,
                &reference_topic,
                open_order.reference_next_fair_value,
                open_order.reference_next_orderbook_bid,
                open_order.reference_next_orderbook_ask,
            );

            for _ in 0..open_order.signal_poll_iterations {
                if !recorded_mock_exec_submissions().is_empty() {
                    break;
                }
                sleep(Duration::from_millis(
                    open_order.signal_poll_interval_milliseconds,
                ))
                .await;
            }

            node.stop()
                .await
                .expect("restart process LiveNode stop should succeed");
        });

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "process-restart reconciled external open order should block duplicate entry submit"
    );
    assert_eq!(
        entry_submission_events(&catalog_dir, &target_id),
        0,
        "process-restart reconciled external open order should block duplicate entry order-submission evidence"
    );
    assert!(
        selected_open_order_entry_evaluation_events(&catalog_dir, &target_id) > 0,
        "process-restart reconciled external open order should persist selected-open-order entry no-action evidence"
    );
    fs::write(marker_path, &open_order.process_exit_restart_marker_text)
        .expect("restart process marker should write");
}

fn open_order_fixture() -> OpenOrderFixture {
    let path = support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/reconciliation/open_order.toml",
    );
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should read: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()))
}

fn node_clock_timestamp_ms(node: &LiveNode) -> u64 {
    node.kernel().clock().borrow().timestamp_ns().as_u64() / NANOSECONDS_PER_MILLISECOND
}

fn load_reconciliation_config(
    temp_dir: &Path,
    open_order: &OpenOrderFixture,
    fee_base_url: Option<String>,
) -> LoadedBoltV3Config {
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root.toml",
    ))
    .expect("existing-strategy v3 TOML should load");
    if let Some(base_url) = fee_base_url {
        point_execution_http_to_local_fee_server(&mut loaded, base_url);
    }
    loaded.root.nautilus.load_state = open_order.load_state;
    loaded.root.nautilus.save_state = open_order.save_state;
    loaded.root.nautilus.delay_post_stop_seconds = open_order.delay_post_stop_seconds;
    loaded.root.nautilus.timeout_disconnection_seconds = open_order.timeout_disconnection_seconds;
    loaded.root.nautilus.timeout_reconciliation_seconds = open_order.timeout_reconciliation_seconds;
    loaded
        .root
        .nautilus
        .exec_engine
        .reconciliation_startup_delay_seconds = open_order.reconciliation_startup_delay_seconds;
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir);
    loaded
}

fn build_reconciliation_node(
    loaded: &LoadedBoltV3Config,
    open_order: &OpenOrderFixture,
    up: InstrumentId,
    down: InstrumentId,
) -> LiveNode {
    let instruments = vec![
        binary_option(up, &open_order.up_token_id, open_order),
        binary_option(down, &open_order.down_token_id, open_order),
    ];
    let mass_status = external_open_order_mass_status(loaded, up, open_order);
    let resolved = resolve_bolt_v3_secrets_with(loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let builder = make_bolt_v3_live_node_builder(loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) = register_bolt_v3_clients(
        builder,
        mock_client_configs_from_loaded(loaded, instruments, mass_status),
    )
    .expect("mock clients should register through v3 client boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, loaded, &resolved)
        .expect("existing strategy should register from v3 TOML");
    node.kernel()
        .cache()
        .borrow_mut()
        .add_account(account_from_fixture(loaded, open_order))
        .expect("mock account should seed NT cache before startup reconciliation");
    node
}

fn assert_reconciled_open_order(
    node: &LiveNode,
    instrument_id: InstrumentId,
    open_order: &OpenOrderFixture,
) {
    let cache_handle = node.kernel().cache();
    let cache = cache_handle.borrow();
    let order_side = fixture_value("order_side", &open_order.order_side);
    let open_orders = cache.orders_open(None, Some(&instrument_id), None, None, Some(order_side));
    assert_eq!(
        open_orders.len(),
        1,
        "startup reconciliation should import external open order into NT cache"
    );
}

fn publish_entry_signal(
    loaded: &LoadedBoltV3Config,
    open_order: &OpenOrderFixture,
    ts_ms: u64,
    up: InstrumentId,
    down: InstrumentId,
    selection_topic: &str,
    reference_topic: &str,
    fair_value: f64,
    orderbook_bid: f64,
    orderbook_ask: f64,
) {
    publish_any(
        selection_topic.to_string().into(),
        &selection_snapshot(loaded, open_order, ts_ms),
    );
    publish_any(
        reference_topic.to_string().into(),
        &reference_snapshot(
            loaded,
            open_order,
            ts_ms,
            fair_value,
            orderbook_bid,
            orderbook_ask,
        ),
    );
    publish_deltas(
        switchboard::get_book_deltas_topic(up),
        &book_deltas(
            up,
            &open_order.up_book_bid,
            &open_order.up_book_ask,
            &open_order.book_quantity,
        ),
    );
    publish_deltas(
        switchboard::get_book_deltas_topic(down),
        &book_deltas(
            down,
            &open_order.down_book_bid,
            &open_order.down_book_ask,
            &open_order.book_quantity,
        ),
    );
}

fn protocol_payload_fixture(filename: &str) -> String {
    std::fs::read_to_string(support::repo_path(&format!(
        "tests/fixtures/bolt_v3_protocol_payloads/{filename}",
    )))
    .unwrap_or_else(|error| panic!("protocol payload fixture {filename} should load: {error}"))
}

fn spawn_fee_rate_server(expected_requests: usize) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("local fee server should bind");
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    let body = protocol_payload_fixture("polymarket_fee_rate_zero.json");

    std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().expect("local fee server should accept");
            let request_text = support::read_local_http_request(&mut stream, "local fee server");
            tx.send(request_text)
                .expect("local fee server should record request");
            let response = support::local_http_json_response(support::LocalHttpStatus::Ok, &body);
            stream
                .write_all(response.as_bytes())
                .expect("local fee server should write response");
        }
    });

    (base_url, rx)
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

fn fixture_value<T>(field: &str, value: &str) -> T
where
    T: FromStr,
    T::Err: Display,
{
    value
        .parse()
        .unwrap_or_else(|error| panic!("fixture field {field}={value:?} should parse: {error}"))
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

fn execution_venue(loaded: &LoadedBoltV3Config) -> String {
    let client_id = execution_client_id(loaded);
    loaded
        .root
        .clients
        .get(&client_id)
        .unwrap_or_else(|| panic!("{client_id} should exist in root clients"))
        .venue
        .as_str()
        .to_string()
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

fn instrument_id(condition_id: &str, token_id: &str, loaded: &LoadedBoltV3Config) -> InstrumentId {
    InstrumentId::from(format!("{condition_id}-{token_id}.{}", execution_venue(loaded)).as_str())
}

fn selected_market(
    loaded: &LoadedBoltV3Config,
    fixture: &OpenOrderFixture,
    start_ts_ms: u64,
) -> CandidateMarket {
    let market_id = configured_target_id(loaded);
    CandidateMarket {
        market_id: market_id.clone(),
        market_slug: market_id.clone(),
        question_id: fixture.question_id.clone(),
        instrument_id: instrument_id(&fixture.condition_id, &fixture.up_token_id, loaded)
            .to_string(),
        condition_id: fixture.condition_id.clone(),
        up_token_id: fixture.up_token_id.clone(),
        down_token_id: fixture.down_token_id.clone(),
        selected_market_observed_ts_ms: start_ts_ms,
        price_to_beat: Some(fixture.reference_initial_fair_value),
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
        liquidity_num: fixture.selection_liquidity_num,
        seconds_to_end: strategy_config(loaded).target["cadence_seconds"]
            .as_integer()
            .expect("fixture target should include cadence_seconds") as u64,
    }
}

fn selection_snapshot(
    loaded: &LoadedBoltV3Config,
    fixture: &OpenOrderFixture,
    start_ts_ms: u64,
) -> RuntimeSelectionSnapshot {
    let market = selected_market(loaded, fixture, start_ts_ms);
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
    _fixture: &OpenOrderFixture,
    ts_ms: u64,
    fair_value: f64,
    orderbook_bid: f64,
    orderbook_ask: f64,
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
    let orderbook_mid = (orderbook_bid + orderbook_ask) / 2.0;
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
                VenueKind::Orderbook => orderbook_mid,
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
                observed_bid: (venue_kind == VenueKind::Orderbook).then_some(orderbook_bid),
                observed_ask: (venue_kind == VenueKind::Orderbook).then_some(orderbook_ask),
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

fn book_deltas(
    instrument_id: InstrumentId,
    bid: &str,
    ask: &str,
    quantity: &str,
) -> OrderBookDeltas {
    OrderBookDeltas::new(
        instrument_id,
        vec![
            OrderBookDelta::new(
                instrument_id,
                BookAction::Update,
                BookOrder::new(
                    OrderSide::Buy,
                    Price::from(bid),
                    Quantity::from(quantity),
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
                    Price::from(ask),
                    Quantity::from(quantity),
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

fn custom_events(
    catalog_dir: &Path,
    configured_target_id: &str,
    event_type: &str,
) -> Vec<nautilus_model::data::Data> {
    let ids = vec![configured_target_id.to_string()];
    let event_dir = catalog_dir
        .join("data")
        .join("custom")
        .join(event_type)
        .join(configured_target_id);
    if !event_dir.exists() {
        return Vec::new();
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
        .flat_map(|file| {
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
        })
        .collect()
}

fn submission_events(catalog_dir: &Path, configured_target_id: &str, event_type: &str) -> usize {
    custom_events(catalog_dir, configured_target_id, event_type).len()
}

fn entry_submission_events(catalog_dir: &Path, configured_target_id: &str) -> usize {
    submission_events(
        catalog_dir,
        configured_target_id,
        BOLT_V3_ENTRY_ORDER_SUBMISSION_DECISION_EVENT_TYPE,
    )
}

fn selected_open_order_entry_evaluation_events(
    catalog_dir: &Path,
    configured_target_id: &str,
) -> usize {
    custom_events(
        catalog_dir,
        configured_target_id,
        BOLT_V3_ENTRY_EVALUATION_DECISION_EVENT_TYPE,
    )
    .into_iter()
    .filter(|event| {
        let nautilus_model::data::Data::Custom(custom) = event else {
            return false;
        };
        let Some(decoded) = custom
            .data
            .as_any()
            .downcast_ref::<BoltV3EntryEvaluationDecisionEvent>()
        else {
            return false;
        };
        decoded
            .event_facts
            .get(BOLT_V3_ENTRY_NO_ACTION_REASON_FACT_KEY)
            .and_then(serde_json::Value::as_str)
            == Some(BOLT_V3_ENTRY_NO_ACTION_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON)
            && decoded
                .event_facts
                .get(BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_REASON_FACT_KEY)
                .and_then(serde_json::Value::as_str)
                == Some(BOLT_V3_UPDOWN_MARKET_MECHANICAL_REJECTION_SELECTED_OPEN_ORDERS_REASON)
            && decoded
                .event_facts
                .get(BOLT_V3_HAS_SELECTED_MARKET_OPEN_ORDERS_FACT_KEY)
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    })
    .count()
}

fn binary_option(
    instrument_id: InstrumentId,
    token_id: &str,
    fixture: &OpenOrderFixture,
) -> InstrumentAny {
    let price_increment = Price::from(fixture.price_increment.as_str());
    let size_increment = Quantity::from(fixture.size_increment.as_str());
    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        Symbol::new(token_id),
        fixture_value::<AssetClass>("asset_class", &fixture.asset_class),
        fixture_value::<Currency>("currency", &fixture.currency),
        UnixNanos::from(fixture.activation_ts_ns),
        UnixNanos::from(fixture.expiration_ts_ns),
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

fn external_open_order_mass_status(
    loaded: &LoadedBoltV3Config,
    instrument_id: InstrumentId,
    fixture: &OpenOrderFixture,
) -> ExecutionMassStatus {
    let client_id = execution_client_id(loaded);
    let account_id = execution_account_id(loaded, &client_id);
    let ts = UnixNanos::from(fixture.report_ts_ns);
    let mut mass_status = ExecutionMassStatus::new(
        ClientId::from(client_id.as_str()),
        AccountId::from(account_id.as_str()),
        Venue::from(execution_venue(loaded).as_str()),
        ts,
        None,
    );
    let order = OrderStatusReport::new(
        AccountId::from(account_id.as_str()),
        instrument_id,
        None,
        VenueOrderId::from(fixture.venue_order_id.as_str()),
        fixture_value::<OrderSide>("order_side", &fixture.order_side),
        fixture_value::<OrderType>("order_type", &fixture.order_type),
        fixture_value::<TimeInForce>("time_in_force", &fixture.time_in_force),
        fixture_value::<OrderStatus>("order_status", &fixture.order_status),
        Quantity::from(fixture.order_quantity.as_str()),
        Quantity::from(fixture.filled_quantity.as_str()),
        ts,
        ts,
        ts,
        None,
    )
    .with_price(Price::from(fixture.order_price.as_str()));
    mass_status.add_order_reports(vec![order]);
    mass_status
}

fn account_from_fixture(loaded: &LoadedBoltV3Config, fixture: &OpenOrderFixture) -> AccountAny {
    let client_id = execution_client_id(loaded);
    let account_id = execution_account_id(loaded, &client_id);
    let base_currency =
        fixture_value::<Currency>("account_base_currency", &fixture.account_base_currency);
    let account_balance = AccountBalance::new(
        Money::from(fixture.account_total.as_str()),
        Money::from(fixture.account_locked.as_str()),
        Money::from(fixture.account_free.as_str()),
    );
    AccountState::new(
        AccountId::from(account_id.as_str()),
        fixture_value::<AccountType>("account_type", &fixture.account_type),
        vec![account_balance],
        Vec::new(),
        true,
        UUID4::new(),
        UnixNanos::from(fixture.report_ts_ns),
        UnixNanos::from(fixture.report_ts_ns),
        Some(base_currency),
    )
    .into()
}

fn mock_client_configs_from_loaded(
    loaded: &LoadedBoltV3Config,
    startup_instruments: Vec<InstrumentAny>,
    mass_status: ExecutionMassStatus,
) -> BoltV3ClientConfigs {
    let execution_client_id = execution_client_id(loaded);
    let clients = loaded
        .root
        .clients
        .iter()
        .map(|(client_id, client)| {
            let venue = client.venue.as_str();
            let data_config = if *client_id == execution_client_id {
                MockDataClientConfig::new(client_id, venue)
                    .with_startup_instruments(startup_instruments.clone())
            } else {
                MockDataClientConfig::new(client_id, venue)
            };
            let data = client.data.as_ref().map(|_| BoltV3DataClientAdapterConfig {
                factory: Box::new(MockDataClientFactory),
                config: Box::new(data_config),
            });
            let execution = client.execution.as_ref().map(|_| {
                let config = MockExecClientConfig::new(
                    client_id,
                    execution_account_id(loaded, client_id).as_str(),
                    venue,
                )
                .with_mass_status(mass_status.clone());
                BoltV3ExecutionClientAdapterConfig {
                    factory: Box::new(MockExecutionClientFactory),
                    config: Box::new(config),
                }
            });
            (client_id.clone(), BoltV3ClientConfig { data, execution })
        })
        .collect::<BTreeMap<_, _>>();
    BoltV3ClientConfigs { clients }
}

fn pinned_nt_checkout() -> PathBuf {
    let revision =
        bolt_v3_compiled_nautilus_trader_revision().expect("Cargo.toml should pin one NT revision");
    let short_revision = revision
        .get(..7)
        .expect("NT revision should be at least 7 chars");
    let cargo_home = cargo_home();
    let checkouts = cargo_home.join("git/checkouts");

    for entry in fs::read_dir(&checkouts).expect("Cargo git checkouts dir should read") {
        let entry = entry.expect("Cargo git checkout entry should read");
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if !file_name.starts_with("nautilus_trader-") {
            continue;
        }
        let candidate = entry.path().join(short_revision);
        if candidate.is_dir() {
            return candidate;
        }
    }

    panic!(
        "pinned NT checkout {short_revision} not found under {}; run cargo fetch/test first",
        checkouts.display()
    );
}

fn cargo_home() -> PathBuf {
    if let Some(path) = env::var_os("CARGO_HOME") {
        return PathBuf::from(path);
    }

    let home = env::var_os("HOME").expect("HOME should be set when CARGO_HOME is unset");
    Path::new(&home).join(".cargo")
}
