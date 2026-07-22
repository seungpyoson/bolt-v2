use crate::support;

use std::time::Duration;

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config, nautilus_startup_bound_secs},
    bolt_v3_live_node::{
        BoltV3LiveNodeError, build_bolt_v3_all_configured_client_mapping_live_node_with_summary,
        run_bolt_v3_live_node,
    },
};
use tokio::net::TcpListener;

fn chainlink_only_loaded_config(endpoint: String) -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.strategies.clear();
    loaded.root.strategy_files.clear();
    loaded.root.risk.capital_pools = None;
    loaded.root.reference_live_probe = None;
    loaded.root.realized_volatility_surfaces = None;
    loaded.root.gate_providers = None;
    loaded.root.outcome_group_sources = None;
    loaded.root.iv = None;
    loaded
        .root
        .clients
        .retain(|client_key, _| client_key == "chainlink_reference");
    loaded.root.nautilus.timeout_connection_secs = 1;
    loaded.root.nautilus.timeout_reconciliation_secs = 1;
    loaded.root.nautilus.timeout_portfolio_secs = 1;
    loaded.root.nautilus.timeout_disconnection_secs = 1;
    loaded.root.nautilus.delay_post_stop_secs = 0;
    loaded.root.nautilus.timeout_shutdown_secs = 1;
    let reconnect_timeout = chainlink_startup_bound(&loaded)
        .checked_add(Duration::from_secs(1))
        .expect("Chainlink startup reconnect timeout should fit");
    let reconnect_timeout_ms = i64::try_from(reconnect_timeout.as_millis())
        .expect("Chainlink startup reconnect timeout should fit TOML integer");

    let chainlink = loaded
        .root
        .clients
        .get_mut("chainlink_reference")
        .expect("fixture should configure chainlink_reference");
    let data = chainlink
        .data
        .as_mut()
        .and_then(toml::Value::as_table_mut)
        .expect("chainlink_reference fixture should configure data table");
    data.insert(
        "websocket_endpoint".to_string(),
        toml::Value::String(endpoint),
    );
    data.insert(
        "reconnect_timeout_ms".to_string(),
        toml::Value::Integer(reconnect_timeout_ms),
    );
    loaded
}

fn chainlink_startup_bound(loaded: &LoadedBoltV3Config) -> Duration {
    Duration::from_secs(
        nautilus_startup_bound_secs(&loaded.root.nautilus)
            .expect("Chainlink startup bound should fit"),
    )
}

#[test]
fn live_node_boot_fails_loudly_when_chainlink_reference_handshake_never_completes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build");
    let local = tokio::task::LocalSet::new();

    runtime.block_on(local.run_until(async {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("local hanging Chainlink listener should bind");
        let endpoint = format!(
            "wss://{}",
            listener
                .local_addr()
                .expect("local listener address should be available")
        );
        let server = tokio::task::spawn_local(async move {
            while let Ok((stream, _peer)) = listener.accept().await {
                tokio::task::spawn_local(async move {
                    let _held_open = stream;
                    tokio::time::sleep(Duration::from_secs(60)).await;
                });
            }
        });
        let catalog_dir = tempfile::tempdir().expect("catalog tempdir should create");
        let mut loaded = chainlink_only_loaded_config(endpoint);
        loaded.root.persistence.catalog_directory =
            catalog_dir.path().to_string_lossy().into_owned();
        support::current_evidence::prepare_current_evidence_generation(&loaded);
        let (mut node, summary) =
            build_bolt_v3_all_configured_client_mapping_live_node_with_summary(
                &loaded,
                |_| false,
                support::fake_bolt_v3_resolver,
            )
            .expect("Chainlink-only live node should build through production mapping");
        assert_eq!(summary.clients.len(), 1);
        let registered = summary
            .clients
            .get("chainlink_reference")
            .expect("Chainlink reference client should be registered");
        assert!(registered.data);
        assert!(!registered.execution);

        let startup_bound = chainlink_startup_bound(&loaded);
        let reconnect_timeout_ms = loaded
            .root
            .clients
            .get("chainlink_reference")
            .and_then(|client| client.data.as_ref())
            .and_then(toml::Value::as_table)
            .and_then(|data| data.get("reconnect_timeout_ms"))
            .and_then(toml::Value::as_integer)
            .expect("fixture Chainlink reconnect_timeout_ms should be configured");
        let reconnect_timeout_ms = u64::try_from(reconnect_timeout_ms)
            .expect("fixture Chainlink reconnect_timeout_ms should be positive");
        assert!(
            Duration::from_millis(reconnect_timeout_ms) > startup_bound,
            "smoke must keep Chainlink reconnect timeout above startup bound so the watchdog fires first"
        );
        let shutdown_grace_bound = Duration::from_secs(3);
        let expected_failure_bound =
            startup_bound + shutdown_grace_bound + Duration::from_secs(2);
        let smoke_guard = Duration::from_secs(30);
        let started = std::time::Instant::now();
        let error = tokio::time::timeout(smoke_guard, run_bolt_v3_live_node(&mut node, &loaded))
            .await
            .expect("live-node boot must fail loudly instead of hanging past the smoke-test guard")
            .expect_err("hanging Chainlink first-connect must fail startup");
        let elapsed = started.elapsed();

        server.abort();

        assert!(
            elapsed <= expected_failure_bound,
            "boot failure should stay within startup bound ({startup_bound:?}) plus shutdown grace ({shutdown_grace_bound:?}) and scheduler slack, elapsed {elapsed:?}"
        );

        match error {
            BoltV3LiveNodeError::LiveNodeStartupTimeout {
                timeout_secs,
                node_state,
                registered_client_labels,
            } => {
                assert_eq!(timeout_secs, startup_bound.as_secs());
                assert_eq!(node_state, "Starting");
                assert!(
                    registered_client_labels
                        .iter()
                        .any(|client| client == "data:chainlink_reference"),
                    "timeout must name the registered Chainlink startup client: {registered_client_labels:?}"
                );
            }
            other => panic!("expected LiveNodeStartupTimeout for hanging Chainlink boot, got {other}"),
        }
    }));
}
