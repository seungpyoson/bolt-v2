mod support;

use std::time::Duration;

use bolt_v2::{
    config::Config,
    live_node_setup::{
        DataClientRegistration, ExecClientRegistration, build_live_node, make_live_node_config,
    },
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::NodeState;
use nautilus_model::identifiers::TraderId;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
};

#[test]
fn explicit_live_node_config_path_runs_with_registered_clients() {
    let trader_id = TraderId::from("BOLT-001");
    let data_config = MockDataClientConfig::new("TEST", "TESTVENUE");
    let exec_config = MockExecClientConfig::new("TEST", "TEST-ACCOUNT", "TESTVENUE");
    let cfg: Config = toml::from_str(
        r#"
        [node]
        name = "TEST-RUN-NODE"
        trader_id = "BOLT-001"
        environment = "Live"
        load_state = false
        save_state = false
        timeout_connection_secs = 1
        timeout_reconciliation_secs = 30
        timeout_portfolio_secs = 10
        timeout_disconnection_secs = 1
        delay_post_stop_secs = 0
        delay_shutdown_secs = 0

        [logging]
        stdout_level = "Info"
        file_level = "Debug"

        [exec_engine]
        position_check_interval_secs = 0.25

        [[data_clients]]
        name = "TEST"
        type = "polymarket"
        [data_clients.config]
        subscribe_new_markets = false
        update_instruments_interval_mins = 60
        ws_max_subscriptions = 200
        event_slugs = ["btc-updown-5m"]

        [[exec_clients]]
        name = "TEST"
        type = "polymarket"
        [exec_clients.config]
        account_id = "POLYMARKET-001"
        signature_type = 2
        funder = "0xabc"
        [exec_clients.secrets]
        region = "us-east-1"
        pk = "/pk"
        api_key = "/key"
        api_secret = "/secret"
        passphrase = "/pass"
        "#,
    )
    .expect("config should parse");

    let live_node_config =
        make_live_node_config(&cfg, trader_id, Environment::Live, LoggerConfig::default());

    let data_clients: Vec<DataClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockDataClientFactory),
        Box::new(data_config),
    )];
    let exec_clients: Vec<ExecClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockExecutionClientFactory),
        Box::new(exec_config),
    )];
    let mut node = build_live_node(
        "TEST-RUN-NODE".to_string(),
        live_node_config,
        data_clients,
        exec_clients,
    )
    .expect("node should build from explicit config and register clients");

    let handle = node.handle();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    let state = runtime.block_on(async move {
        tokio::time::timeout(Duration::from_secs(1), async move {
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
        .expect("run should finish before timeout")
    });

    assert_eq!(state, NodeState::Stopped);
}

#[test]
fn sandbox_startup_path_preserves_environment_when_position_check_is_unset() {
    let trader_id = TraderId::from("BOLT-001");
    let data_config = MockDataClientConfig::new("TEST", "TESTVENUE");
    let exec_config = MockExecClientConfig::new("TEST", "TEST-ACCOUNT", "TESTVENUE");
    let cfg: Config = toml::from_str(
        r#"
        [node]
        name = "TEST-RUN-NODE"
        trader_id = "BOLT-001"
        environment = "Sandbox"
        load_state = false
        save_state = false
        timeout_connection_secs = 1
        timeout_reconciliation_secs = 30
        timeout_portfolio_secs = 10
        timeout_disconnection_secs = 1
        delay_post_stop_secs = 0
        delay_shutdown_secs = 0

        [logging]
        stdout_level = "Info"
        file_level = "Debug"

        [[data_clients]]
        name = "TEST"
        type = "polymarket"
        [data_clients.config]
        subscribe_new_markets = false
        update_instruments_interval_mins = 60
        ws_max_subscriptions = 200
        event_slugs = ["btc-updown-5m"]

        [[exec_clients]]
        name = "TEST"
        type = "polymarket"
        [exec_clients.config]
        account_id = "POLYMARKET-001"
        signature_type = 2
        funder = "0xabc"
        [exec_clients.secrets]
        region = "us-east-1"
        pk = "/pk"
        api_key = "/key"
        api_secret = "/secret"
        passphrase = "/pass"
        "#,
    )
    .expect("config should parse");

    let node_config = make_live_node_config(
        &cfg,
        trader_id,
        Environment::Sandbox,
        LoggerConfig::default(),
    );
    let data_clients: Vec<DataClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockDataClientFactory),
        Box::new(data_config),
    )];
    let exec_clients: Vec<ExecClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockExecutionClientFactory),
        Box::new(exec_config),
    )];
    let node = build_live_node(
        "TEST-RUN-NODE".to_string(),
        node_config,
        data_clients,
        exec_clients,
    )
    .expect("sandbox node should build through shared startup helper");

    assert_eq!(node.environment(), Environment::Sandbox);
}

#[test]
fn sandbox_startup_rejects_position_check_interval() {
    let trader_id = TraderId::from("BOLT-001");
    let data_config = MockDataClientConfig::new("TEST", "TESTVENUE");
    let exec_config = MockExecClientConfig::new("TEST", "TEST-ACCOUNT", "TESTVENUE");
    let cfg: Config = toml::from_str(
        r#"
        [node]
        name = "TEST-RUN-NODE"
        trader_id = "BOLT-001"
        environment = "Sandbox"
        load_state = false
        save_state = false
        timeout_connection_secs = 1
        timeout_reconciliation_secs = 30
        timeout_portfolio_secs = 10
        timeout_disconnection_secs = 1
        delay_post_stop_secs = 0
        delay_shutdown_secs = 0

        [logging]
        stdout_level = "Info"
        file_level = "Debug"

        [exec_engine]
        position_check_interval_secs = 0.25

        [[data_clients]]
        name = "TEST"
        type = "polymarket"
        [data_clients.config]
        subscribe_new_markets = false
        update_instruments_interval_mins = 60
        ws_max_subscriptions = 200
        event_slugs = ["btc-updown-5m"]

        [[exec_clients]]
        name = "TEST"
        type = "polymarket"
        [exec_clients.config]
        account_id = "POLYMARKET-001"
        signature_type = 2
        funder = "0xabc"
        [exec_clients.secrets]
        region = "us-east-1"
        pk = "/pk"
        api_key = "/key"
        api_secret = "/secret"
        passphrase = "/pass"
        "#,
    )
    .expect("config should parse");

    let node_config = make_live_node_config(
        &cfg,
        trader_id,
        Environment::Sandbox,
        LoggerConfig::default(),
    );
    let data_clients: Vec<DataClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockDataClientFactory),
        Box::new(data_config),
    )];
    let exec_clients: Vec<ExecClientRegistration> = vec![(
        Some("TEST".to_string()),
        Box::new(MockExecutionClientFactory),
        Box::new(exec_config),
    )];
    let error = build_live_node(
        "TEST-RUN-NODE".to_string(),
        node_config,
        data_clients,
        exec_clients,
    )
    .expect_err("sandbox should reject position check interval on shared startup helper");

    assert!(
        error
            .to_string()
            .contains("position_check_interval_secs is unsupported in Sandbox startup mode")
    );
}

#[test]
fn builds_bolt_v3_livenode_without_running_event_loop() {
    use bolt_v2::bolt_v3_live_node::build_bolt_v3_live_node_with;
    use nautilus_common::enums::Environment;
    use nautilus_live::node::NodeState;

    let (_tempdir, loaded) = support::load_bolt_v3_config_with_temp_catalog("live-node-run");

    // No forbidden env vars are set in the test predicate, and the fake
    // resolver supplies all configured SSM paths, so the build proceeds.
    let node = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("v3 LiveNode should build without entering the event loop");

    assert_eq!(node.environment(), Environment::Live);
    // node.run() is intentionally never called in this slice.
    assert_eq!(node.state(), NodeState::Idle);
}

#[test]
fn wires_runtime_capture_from_bolt_v3_persistence_config() {
    use bolt_v2::{
        bolt_v3_config::{
            BoltV3RootConfig, LoadedBoltV3Config, PersistenceBlock, StreamingBlock,
            load_bolt_v3_config,
        },
        bolt_v3_live_node::{build_bolt_v3_live_node_with, wire_bolt_v3_runtime_capture},
    };
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::task::LocalSet;

    let local = LocalSet::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build");

    runtime.block_on(local.run_until(async {
        let dir = tempdir().expect("tempdir for catalog should be created");
        let catalog_root = dir.path().join("catalog");
        std::fs::create_dir_all(&catalog_root).expect("catalog dir should be created");

        let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
        let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

        // Replace the production catalog_directory with the tempdir so the
        // wiring test can observe start/stop without touching /var/lib/bolt.
        let original_root = loaded.root.clone();
        let routed_root = BoltV3RootConfig {
            persistence: PersistenceBlock {
                catalog_directory: catalog_root.to_string_lossy().to_string(),
                decision_evidence: original_root.persistence.decision_evidence.clone(),
                streaming: StreamingBlock {
                    catalog_fs_protocol: original_root.persistence.streaming.catalog_fs_protocol,
                    flush_interval_milliseconds: original_root
                        .persistence
                        .streaming
                        .flush_interval_milliseconds,
                    replace_existing: original_root.persistence.streaming.replace_existing,
                    rotation_kind: original_root.persistence.streaming.rotation_kind,
                },
            },
            ..original_root
        };
        let routed_loaded = LoadedBoltV3Config {
            root_path: loaded.root_path.clone(),
            root: routed_root,
            strategies: loaded.strategies.clone(),
        };

        let node =
            build_bolt_v3_live_node_with(&routed_loaded, |_| false, support::fake_bolt_v3_resolver)
                .expect("v3 LiveNode should build for runtime-capture wiring");
        let handle = node.handle();
        let guards = wire_bolt_v3_runtime_capture(&node, handle.clone(), &routed_loaded)
            .expect("runtime capture should wire from v3 persistence config");

        // Give the spawned worker a chance to spin up, then shut it down. We
        // never invoke node.run(), so the worker stays in its startup-buffer
        // state and shutdown_all unsubscribes cleanly.
        tokio::time::sleep(Duration::from_millis(50)).await;
        guards
            .shutdown()
            .await
            .expect("runtime capture should shut down cleanly");

        let instance_root = catalog_root
            .join("live")
            .join(node.instance_id().to_string());
        assert!(
            instance_root.exists(),
            "wire should create the per-instance spool root at {}",
            instance_root.display()
        );
    }));
}
