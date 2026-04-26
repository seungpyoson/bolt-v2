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

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{
        BoltV3LiveNodeError, connect_bolt_v3_clients, make_bolt_v3_live_node_builder,
    },
};
use nautilus_live::node::NodeState;
use nautilus_model::identifiers::ClientId;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_data_subscriptions, clear_mock_exec_submissions, recorded_mock_data_subscriptions,
    recorded_mock_exec_submissions, repo_path,
};

fn fixture_loaded_with_connection_timeout(timeout_secs: u64) -> LoadedBoltV3Config {
    // Load the production fixture, then rewrite only
    // `nautilus.timeout_connection_seconds` so the controlled-connect
    // boundary has an explicit per-test bound. Every other field stays
    // exactly as production reads it from disk.
    let root_path = repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.nautilus.timeout_connection_seconds = timeout_secs;
    loaded
}

#[test]
fn controlled_connect_dispatches_engine_connect_on_mock_clients_without_starting_trader() {
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_connection_timeout(30);

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
    clear_mock_data_subscriptions();
    clear_mock_exec_submissions();

    let loaded = fixture_loaded_with_connection_timeout(1);
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
fn live_node_module_remains_no_trade_boundary_after_controlled_connect_addition() {
    // Source-level inspection of `src/bolt_v3_live_node.rs`. The module
    // is allowed to reference NT's `connect_data_clients` and
    // `connect_exec_clients` (the pinned controlled-connect API), but it
    // must never call `node.run`, `kernel.start_trader`, register a
    // strategy actor, select a market, construct an order, or submit
    // one. The forbidden token list lives in this integration test (not
    // in the module's own source) so the assertion does not self-trip.
    let source = include_str!("../src/bolt_v3_live_node.rs");
    for forbidden in [
        ".run(",
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
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_live_node.rs must remain a no-trade boundary; \
             source unexpectedly references `{forbidden}`"
        );
    }
}
