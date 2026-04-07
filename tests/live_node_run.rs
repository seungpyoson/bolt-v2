mod support;

use std::time::Duration;

use bolt_v2::strategies::exec_tester;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::{LiveNode, NodeState};
use nautilus_model::identifiers::TraderId;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
};

#[test]
fn run_starts_and_stops_cleanly_with_test_clients_and_exec_tester() {
    let trader_id = TraderId::from("BOLT-001");
    let data_config = MockDataClientConfig::new("TEST", "TESTVENUE");
    let exec_config = MockExecClientConfig::new("TEST", "TEST-ACCOUNT", "TESTVENUE");

    let strategy = exec_tester::build_exec_tester(&toml::toml! {
        strategy_id = "EXEC_TESTER-001"
        instrument_id = "TOKEN.TESTVENUE"
        client_id = "TEST"
        order_qty = "5"
        log_data = false
        tob_offset_ticks = 5
        use_post_only = true
        enable_limit_sells = false
        enable_stop_buys = false
        enable_stop_sells = false
    }
    .into())
    .expect("strategy should translate");

    let mut node = LiveNode::builder(trader_id, Environment::Live)
        .expect("builder should construct")
        .with_name("TEST-RUN-NODE")
        .with_logging(LoggerConfig::default())
        .with_reconciliation(false)
        .with_timeout_connection(1)
        .with_timeout_disconnection_secs(1)
        .with_delay_post_stop_secs(0)
        .with_delay_shutdown_secs(0)
        .add_data_client(
            Some("TEST".to_string()),
            Box::new(MockDataClientFactory),
            Box::new(data_config),
        )
        .expect("data client should register")
        .add_exec_client(
            Some("TEST".to_string()),
            Box::new(MockExecutionClientFactory),
            Box::new(exec_config),
        )
        .expect("exec client should register")
        .build()
        .expect("node should build");

    node.add_strategy(strategy)
        .expect("strategy should register");

    let handle = node.handle();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    let state = runtime.block_on(async move {
        let run_result = tokio::time::timeout(Duration::from_secs(1), async move {
            let stop_after_startup = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                handle.stop();
            };

            let runner = async {
                node.run().await.expect("run should stop cleanly");
                node.state()
            };

            tokio::join!(stop_after_startup, runner).1
        })
        .await
        .expect("run should finish before timeout");

        run_result
    });

    assert_eq!(state, NodeState::Stopped);
}
