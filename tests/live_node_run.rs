mod support;

use std::time::Duration;

use bolt_v2::{
    clients::chainlink::build_chainlink_reference_data_client_with_secrets,
    config::Config,
    live_node_setup::{
        DataClientRegistration, ExecClientRegistration, build_live_node, make_live_node_config,
    },
    secrets::ResolvedChainlinkSecrets,
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::NodeState;
use nautilus_model::identifiers::{ClientId, TraderId};
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
};

const CORRECT_ETH_TESTNET_FEED_ID: &str =
    "0x000359843a543ee2fe414dc14c7e7920ef10f4372990b79d6361cdc0dd1ba782";

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
fn explicit_live_node_config_path_registers_chainlink_client_after_sender_init() {
    let trader_id = TraderId::from("BOLT-001");
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

    let reference = bolt_v2::config::ReferenceConfig {
        publish_topic: "platform.reference.test.chainlink".into(),
        min_publish_interval_ms: 100,
        binance: None,
        chainlink: Some(bolt_v2::config::ChainlinkSharedConfig {
            region: "us-east-1".into(),
            api_key: "/bolt/chainlink/api_key".into(),
            api_secret: "/bolt/chainlink/api_secret".into(),
            ws_url: "wss://ws.testnet-dataengine.chain.link".into(),
            ws_reconnect_alert_threshold: 5,
        }),
        venues: vec![bolt_v2::config::ReferenceVenueEntry {
            name: "CHAINLINK-ETH".into(),
            kind: bolt_v2::config::ReferenceVenueKind::Chainlink,
            instrument_id: "ETHUSD.CHAINLINK".into(),
            base_weight: 1.0,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
            chainlink: Some(bolt_v2::config::ChainlinkReferenceConfig {
                feed_id: CORRECT_ETH_TESTNET_FEED_ID.into(),
                price_scale: 18,
            }),
        }],
    };

    let (factory, client_config) = build_chainlink_reference_data_client_with_secrets(
        &reference,
        ResolvedChainlinkSecrets {
            api_key: "placeholder-api-key".into(),
            api_secret: "placeholder-api-secret".into(),
        },
    )
    .expect("chainlink client config should build");

    let node = build_live_node(
        "TEST-RUN-NODE".to_string(),
        live_node_config,
        vec![(Some("CHAINLINK".to_string()), factory, client_config)],
        vec![],
    )
    .expect("chainlink client should register after live sender initialization");

    assert_eq!(
        node.kernel().data_engine().registered_clients(),
        vec![ClientId::from("CHAINLINK")]
    );
}
