//! Integration tests for the bolt-v3 controlled-connect boundary.
//!
//! These tests guard the contract that `connect_bolt_v3_clients`:
//!   1. Drives `NautilusKernel::connect_data_clients` and
//!      `NautilusKernel::connect_exec_clients` against every NT data
//!      and execution client that the bolt-v3 client-registration
//!      boundary added to the LiveNode, through the pinned NT Rust API
//!      surface only.
//!   2. Returns without ever calling `LiveNode::start`, `LiveNode::run`,
//!      or `kernel.start_trader`. The post-call `NodeState` must remain
//!      `NodeState::Idle`, which is the public state machine evidence
//!      that no strategy was started and the runner loop never ran.
//!   3. Does not register strategies, select markets, construct orders,
//!      or submit orders. Orders submitted and market-data subscriptions
//!      issued via the test-support recording mocks must both be empty
//!      after the call returns.
//!   4. Source-level: `src/bolt_v3_live_node.rs` does NOT reference any
//!      strategy / market-selection / order-construction / submit
//!      identifier or the `node.run` / `start_trader` entrypoints.
//!
//! These tests use the existing `MockDataClient` / `MockExecutionClient`
//! infrastructure from `tests/support/mod.rs` so no production network
//! is touched. The bolt-v3 LiveNodeBuilder is constructed via the
//! production `make_bolt_v3_live_node_builder` factory so the bolt-v3
//! `LoggerConfig`, environment, trader id, and timeout values are the
//! same values the production binary would receive; only the venue
//! client factories are swapped for mocks.

mod support;

use std::sync::{Mutex, MutexGuard};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{
        BoltV3LiveNodeError, connect_bolt_v3_clients, disconnect_bolt_v3_clients,
        make_bolt_v3_live_node_builder, make_live_node_config,
    },
};
use nautilus_live::builder::LiveNodeBuilder;
use nautilus_live::node::NodeState;
use nautilus_model::identifiers::ClientId;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_data_subscriptions, clear_mock_exec_submissions, recorded_mock_data_subscriptions,
    recorded_mock_exec_submissions, repo_path,
};

static LIVE_NODE_TEST_LOCK: Mutex<()> = Mutex::new(());

fn live_node_test_guard() -> MutexGuard<'static, ()> {
    LIVE_NODE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fixture_loaded_with_timeouts(
    connection_timeout_secs: u64,
    disconnection_timeout_secs: u64,
) -> LoadedBoltV3Config {
    // Load the production fixture, then rewrite only the Nautilus
    // timeout fields this boundary owns so each test has explicit
    // bounds. Every other field stays exactly as production reads it
    // from disk.
    let root_path = repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.nautilus.timeout_connection_seconds = connection_timeout_secs;
    loaded.root.nautilus.timeout_disconnection_seconds = disconnection_timeout_secs;
    loaded
}

#[test]
fn builder_path_passes_explicit_exec_engine_to_nt_build() {
    let _guard = live_node_test_guard();

    let loaded = fixture_loaded_with_timeouts(30, 10);
    let mut cfg = make_live_node_config(&loaded);
    // Mutate after Bolt's config-load validation so this test proves the
    // production builder passes the exec engine config to NT's own build-time
    // validation, instead of constructing a fresh default engine config.
    cfg.exec_engine.reconciliation_startup_delay_secs = -1.0;

    let error = LiveNodeBuilder::from_config(cfg)
        .expect("v3 builder should construct from fixture")
        .build()
        .expect_err("NT build should validate the exec engine config passed through builder");

    assert!(
        error
            .to_string()
            .contains("invalid LiveExecEngineConfig.reconciliation_startup_delay_secs"),
        "expected NT exec-engine validation error, got: {error}"
    );
}

#[test]
fn builder_path_passes_explicit_data_engine_to_nt_build() {
    let _guard = live_node_test_guard();

    let mut loaded = fixture_loaded_with_timeouts(30, 10);
    // Mutate after Bolt's config-load validation so this test proves the
    // production builder passes the data engine config to NT's own build-time
    // validation, instead of constructing a fresh default engine config.
    loaded
        .root
        .nautilus
        .data_engine
        .time_bars_origins
        .insert("INVALID".to_string(), 1);

    let error = make_bolt_v3_live_node_builder(&loaded)
        .expect("v3 builder should construct from fixture")
        .build()
        .expect_err("NT build should validate the data engine config passed through builder");

    assert!(
        error
            .to_string()
            .contains("invalid LiveDataEngineConfig.time_bars_origins"),
        "expected NT data-engine validation error, got: {error}"
    );
}

#[test]
fn builder_path_passes_explicit_risk_engine_to_nt_build() {
    let _guard = live_node_test_guard();

    let mut loaded = fixture_loaded_with_timeouts(30, 10);
    // Mutate after Bolt's config-load validation so this test proves the
    // production builder passes the risk engine config to NT's own build-time
    // validation, instead of constructing a fresh default engine config.
    loaded.root.risk.nt_max_order_submit_rate = "not-a-rate-limit".to_string();

    let error = make_bolt_v3_live_node_builder(&loaded)
        .expect("v3 builder should construct from fixture")
        .build()
        .expect_err("NT build should validate the risk engine config passed through builder");

    assert!(
        error
            .to_string()
            .contains("invalid LiveRiskEngineConfig.max_order_submit_rate"),
        "expected NT risk-engine validation error, got: {error}"
    );
}

#[test]
fn controlled_connect_dispatches_engine_connect_on_mock_clients_without_starting_trader() {
    let _guard = live_node_test_guard();
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_timeouts(30, 10);

    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new("MOCK_DATA", "MOCKVENUE")),
        )
        .expect("mock data client should register on bolt-v3 builder");
    let builder = builder
        .add_exec_client(
            Some("MOCK_EXEC".to_string()),
            Box::new(MockExecutionClientFactory),
            Box::new(MockExecClientConfig::new(
                "MOCK_EXEC",
                "MOCK-ACCOUNT",
                "MOCKVENUE",
            )),
        )
        .expect("mock exec client should register on bolt-v3 builder");
    let mut node = builder.build().expect("LiveNode should build with mocks");

    assert_eq!(
        node.state(),
        NodeState::Idle,
        "node must start Idle before controlled-connect"
    );
    assert!(
        node.kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .contains(&ClientId::from("MOCK_DATA")),
        "data engine must have MOCK_DATA registered before connect"
    );
    assert!(
        node.kernel()
            .exec_engine
            .borrow()
            .client_ids()
            .contains(&ClientId::from("MOCK_EXEC")),
        "exec engine must have MOCK_EXEC registered before connect"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for controlled-connect test");
    runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect("controlled-connect must succeed against mock clients");

    // 1. NodeState is still Idle: connect_bolt_v3_clients never called
    //    LiveNode::start or LiveNode::run, which are the only public
    //    LiveNode methods that transition NodeState into Starting/Running.
    assert_eq!(
        node.state(),
        NodeState::Idle,
        "NodeState must stay Idle after controlled-connect (no start_trader, no run loop)"
    );

    // 2. Both engines now report their clients connected because each
    //    mock's `async fn connect(&mut self)` flipped its internal
    //    `connected` flag through NT's engine-level connect dispatcher.
    assert!(
        node.kernel().data_engine.borrow().check_connected(),
        "data engine must report all clients connected after controlled-connect"
    );
    assert!(
        node.kernel().exec_engine.borrow().check_connected(),
        "exec engine must report all clients connected after controlled-connect"
    );

    // 3. No orders submitted, no market-data subscriptions issued:
    //    the controlled-connect boundary is no-trade by construction.
    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "controlled-connect must not submit any orders; got {:?}",
        recorded_mock_exec_submissions()
    );
    assert!(
        recorded_mock_data_subscriptions().is_empty(),
        "controlled-connect must not subscribe to any market data; got {:?}",
        recorded_mock_data_subscriptions()
    );
}

#[test]
fn controlled_connect_returns_timeout_when_engine_connect_exceeds_configured_bound() {
    let _guard = live_node_test_guard();
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_timeouts(1, 10);
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("SLOW_MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(
                MockDataClientConfig::new("SLOW_MOCK_DATA", "MOCKVENUE")
                    .with_connect_delay_milliseconds(2_000),
            ),
        )
        .expect("slow mock data client should register on bolt-v3 builder");
    let mut node = builder
        .build()
        .expect("LiveNode should build with slow mock data client");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for controlled-connect timeout test");
    let error = runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect_err("controlled-connect must surface the configured timeout");

    match error {
        BoltV3LiveNodeError::ConnectTimeout { timeout_seconds } => {
            assert_eq!(timeout_seconds, 1);
        }
        other => panic!("expected ConnectTimeout, got {other}"),
    }
    assert_eq!(
        node.state(),
        NodeState::Idle,
        "timeout path must not start trader or runner loop"
    );
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

#[test]
fn controlled_connect_returns_incomplete_when_engine_swallows_client_connect_failure() {
    let _guard = live_node_test_guard();
    // Pinned NT engine-level connect dispatchers
    // (`DataEngine::connect`, `ExecutionEngine::connect`) swallow
    // individual client `connect()` errors and only log them (see
    // `nautilus_data::engine::DataEngine::connect` and
    // `nautilus_execution::engine::ExecutionEngine::connect`). The
    // bolt-v3 controlled-connect boundary must therefore consult NT's
    // `kernel.check_engines_connected()` after the dispatch returns:
    // if any registered NT client did not transition to `is_connected`
    // because its `connect()` returned `Err(...)`, the boundary must
    // surface a `ConnectIncomplete` error rather than `Ok(())`.
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_timeouts(30, 10);

    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("FAILING_MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(
                MockDataClientConfig::new("FAILING_MOCK_DATA", "MOCKVENUE")
                    .with_connect_failure("simulated bolt-v3 mock connect failure"),
            ),
        )
        .expect("failing mock data client should register on bolt-v3 builder");
    let mut node = builder
        .build()
        .expect("LiveNode should build with failing mock data client");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for controlled-connect failure test");
    let error = runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect_err("controlled-connect must surface incomplete-connect when a registered client fails connect");

    match error {
        BoltV3LiveNodeError::ConnectIncomplete => {}
        other => panic!("expected ConnectIncomplete, got {other}"),
    }
    assert!(
        !node.kernel().check_engines_connected(),
        "kernel.check_engines_connected() must remain false after a swallowed client connect failure"
    );
    assert_eq!(
        node.state(),
        NodeState::Idle,
        "incomplete-connect path must not start trader or runner loop"
    );
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

#[test]
fn controlled_disconnect_after_successful_connect_returns_ok_and_disconnects_engine_clients() {
    let _guard = live_node_test_guard();
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_timeouts(30, 10);
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(MockDataClientConfig::new("MOCK_DATA", "MOCKVENUE")),
        )
        .expect("mock data client should register on bolt-v3 builder");
    let builder = builder
        .add_exec_client(
            Some("MOCK_EXEC".to_string()),
            Box::new(MockExecutionClientFactory),
            Box::new(MockExecClientConfig::new(
                "MOCK_EXEC",
                "MOCK-ACCOUNT",
                "MOCKVENUE",
            )),
        )
        .expect("mock exec client should register on bolt-v3 builder");
    let mut node = builder.build().expect("LiveNode should build with mocks");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for controlled-disconnect test");
    runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect("controlled-connect must succeed against mock clients");
    assert!(
        node.kernel().check_engines_connected(),
        "engines must report connected after controlled-connect"
    );

    runtime
        .block_on(disconnect_bolt_v3_clients(&mut node, &loaded))
        .expect("controlled-disconnect must succeed against mock clients");

    assert!(
        node.kernel().check_engines_disconnected(),
        "kernel must report all engine clients disconnected after controlled-disconnect"
    );
    assert_eq!(
        node.state(),
        NodeState::Idle,
        "controlled-disconnect must not transition NodeState (no LiveNode::stop, no runner loop)"
    );
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

#[test]
fn controlled_disconnect_returns_timeout_when_engine_disconnect_exceeds_configured_bound() {
    let _guard = live_node_test_guard();
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    // Keep the connect bound generous and the disconnect bound short
    // so this test proves controlled-disconnect uses
    // `timeout_disconnection_seconds`, not the connection timeout.
    let loaded = fixture_loaded_with_timeouts(30, 1);
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("SLOW_DISCONNECT_MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(
                MockDataClientConfig::new("SLOW_DISCONNECT_MOCK_DATA", "MOCKVENUE")
                    .with_disconnect_delay_milliseconds(2_000),
            ),
        )
        .expect("slow-disconnect mock data client should register on bolt-v3 builder");
    let mut node = builder
        .build()
        .expect("LiveNode should build with slow-disconnect mock data client");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for controlled-disconnect timeout test");
    runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect("controlled-connect must succeed before timeout-disconnect test");

    let error = runtime
        .block_on(disconnect_bolt_v3_clients(&mut node, &loaded))
        .expect_err("controlled-disconnect must surface the configured timeout");

    match error {
        BoltV3LiveNodeError::DisconnectTimeout { timeout_seconds } => {
            assert_eq!(timeout_seconds, 1);
        }
        other => panic!("expected DisconnectTimeout, got {other}"),
    }
    assert_eq!(
        node.state(),
        NodeState::Idle,
        "disconnect-timeout path must not transition NodeState"
    );
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

#[test]
fn controlled_disconnect_propagates_engine_disconnect_failure() {
    let _guard = live_node_test_guard();
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_timeouts(30, 10);
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("FAILING_DISCONNECT_MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(
                MockDataClientConfig::new("FAILING_DISCONNECT_MOCK_DATA", "MOCKVENUE")
                    .with_disconnect_failure("simulated bolt-v3 mock disconnect failure"),
            ),
        )
        .expect("failing-disconnect mock data client should register on bolt-v3 builder");
    let mut node = builder
        .build()
        .expect("LiveNode should build with failing-disconnect mock data client");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for controlled-disconnect failure test");
    runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect("controlled-connect must succeed before disconnect-failure test");

    let error = runtime
        .block_on(disconnect_bolt_v3_clients(&mut node, &loaded))
        .expect_err("controlled-disconnect must propagate NT disconnect errors");

    match error {
        BoltV3LiveNodeError::DisconnectFailed(error) => {
            assert!(
                error
                    .to_string()
                    .contains("simulated bolt-v3 mock disconnect failure"),
                "DisconnectFailed must preserve the NT disconnect error, got: {error}"
            );
        }
        other => panic!("expected DisconnectFailed, got {other}"),
    }
    assert_eq!(
        node.state(),
        NodeState::Idle,
        "disconnect-failure path must not transition NodeState"
    );
    assert!(recorded_mock_exec_submissions().is_empty());
    assert!(recorded_mock_data_subscriptions().is_empty());
}

#[test]
fn controlled_disconnect_is_callable_after_connect_timeout_partial_state() {
    let _guard = live_node_test_guard();
    // After `connect_bolt_v3_clients` returns a `ConnectTimeout`, the
    // bolt-v3 LiveNode is left in a partially-connected state owned by
    // NT (the tokio::time::timeout dropped the awaiting future, but
    // any client whose `connect()` already finished still has its
    // `connected` flag set). The controlled-disconnect boundary must
    // remain callable in that state and clean up the surviving
    // connections under its own bound.
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_timeouts(1, 30);
    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct from fixture");
    let builder = builder
        .add_data_client(
            Some("SLOW_CONNECT_MOCK_DATA".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(
                MockDataClientConfig::new("SLOW_CONNECT_MOCK_DATA", "MOCKVENUE")
                    .with_connect_delay_milliseconds(2_000),
            ),
        )
        .expect("slow-connect mock data client should register on bolt-v3 builder");
    let mut node = builder
        .build()
        .expect("LiveNode should build with slow-connect mock data client");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for connect-timeout-then-disconnect test");
    let error = runtime
        .block_on(connect_bolt_v3_clients(&mut node, &loaded))
        .expect_err("controlled-connect must surface the configured timeout");
    match error {
        BoltV3LiveNodeError::ConnectTimeout { timeout_seconds } => {
            assert_eq!(timeout_seconds, 1);
        }
        other => panic!("expected ConnectTimeout, got {other}"),
    }

    // Disconnect must still be callable after a timeout-truncated
    // connect; with the mock's disconnect_delay at zero this completes
    // well within the bound.
    runtime
        .block_on(disconnect_bolt_v3_clients(&mut node, &loaded))
        .expect("controlled-disconnect must remain callable after a connect-timeout");
}

fn source_between<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let start_index = source
        .find(start)
        .unwrap_or_else(|| panic!("source should contain start marker {start}"));
    let end_index = source[start_index..]
        .find(end)
        .map(|offset| start_index + offset)
        .unwrap_or_else(|| panic!("source should contain end marker {end}"));
    &source[start_index..end_index]
}

#[test]
fn controlled_connect_and_disconnect_boundaries_remain_no_trade() {
    // Source-level inspection of only the controlled-connect and
    // controlled-disconnect boundaries. This file also owns the explicit
    // production runner wrapper, so scanning the whole module would blur
    // the two contracts.
    let source = include_str!("../src/bolt_v3_live_node.rs");
    let controlled_connect = source_between(
        source,
        "pub async fn connect_bolt_v3_clients",
        "pub async fn disconnect_bolt_v3_clients",
    );
    let controlled_disconnect = source_between(
        source,
        "pub async fn disconnect_bolt_v3_clients",
        "pub async fn run_bolt_v3_live_node",
    );
    let boundary_source = format!("{controlled_connect}\n{controlled_disconnect}");
    for forbidden in [
        ".run(",
        ".start(",
        "start_async",
        "kernel.start",
        "start_trader",
        "register_strategy",
        "register_actor",
        "select_market",
        "submit_order",
        "submit_order_list",
        "modify_order",
        "cancel_order",
        "OrderBuilder",
        "PolymarketOrderBuilder",
        "OrderSubmitter",
        "subscribe_quote_ticks",
        "subscribe_trade_ticks",
        "subscribe_order_book_deltas",
        "subscribe_order_book_snapshots",
        "subscribe_instruments",
        "subscribe_market",
        "ws_client.subscribe",
    ] {
        assert!(
            !boundary_source.contains(forbidden),
            "controlled-connect/disconnect must remain no-trade; \
             boundary source unexpectedly references `{forbidden}`"
        );
    }
}
