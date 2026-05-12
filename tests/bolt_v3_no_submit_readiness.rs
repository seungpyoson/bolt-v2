mod support;

use std::{collections::BTreeSet, path::Path};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_no_submit_readiness::{
        BoltV3NoSubmitReadinessStage, BoltV3NoSubmitReadinessStatus,
        BoltV3NoSubmitReadinessSubject, build_bolt_v3_no_submit_live_node_with_summary,
        run_bolt_v3_no_submit_readiness, run_bolt_v3_no_submit_readiness_with,
    },
};
use nautilus_live::node::NodeState;

fn fixture_loaded() -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    load_bolt_v3_config(&root_path).expect("fixture v3 config should load")
}

fn first_secret_path(loaded: &LoadedBoltV3Config) -> (String, String) {
    let (client_id_key, secret_path) = loaded
        .root
        .clients
        .iter()
        .filter_map(|(client_id_key, client)| {
            let secrets = client.secrets.as_ref()?.as_table()?;
            let secret_path = secrets.values().find_map(toml::Value::as_str)?;
            Some((client_id_key.clone(), secret_path.to_string()))
        })
        .next()
        .expect("fixture should define at least one client secret path");
    (client_id_key, secret_path)
}

fn first_client_with_data_field(loaded: &LoadedBoltV3Config, field: &str) -> String {
    loaded
        .root
        .clients
        .iter()
        .find_map(|(client_id_key, client)| {
            client
                .data
                .as_ref()
                .and_then(toml::Value::as_table)
                .and_then(|data| data.contains_key(field).then_some(client_id_key.clone()))
        })
        .unwrap_or_else(|| panic!("fixture should define data field {field}"))
}

fn fixture_resolved_secret_values(loaded: &LoadedBoltV3Config) -> BTreeSet<String> {
    loaded
        .root
        .clients
        .values()
        .filter_map(|client| client.secrets.as_ref())
        .filter_map(toml::Value::as_table)
        .flat_map(|secrets| secrets.values())
        .filter_map(toml::Value::as_str)
        .filter_map(|path| support::fake_bolt_v3_resolver(&loaded.root.aws.region, path).ok())
        .collect()
}

fn assert_no_resolved_secret_values(loaded: &LoadedBoltV3Config, text: &str) {
    for secret_value in fixture_resolved_secret_values(loaded) {
        assert!(
            !text.contains(&secret_value),
            "report must not contain resolved secret value {secret_value}: {text}"
        );
    }
}

#[test]
fn no_submit_readiness_builds_client_only_idle_node_without_strategy_registration() {
    let loaded = fixture_loaded();
    assert!(
        !loaded.strategies.is_empty(),
        "fixture should include strategies so this proves client-only build skips strategy registration"
    );

    let (node, summary) = build_bolt_v3_no_submit_live_node_with_summary(
        &loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    )
    .expect("no-submit readiness should build an NT LiveNode from configured clients");

    assert_eq!(node.state(), NodeState::Idle);
    assert_eq!(summary.clients.len(), loaded.root.clients.len());
    assert_eq!(node.kernel().trader().borrow().strategy_count(), 0);
}

#[test]
fn no_submit_readiness_missing_secret_stops_before_mapping_build_and_connect() {
    let loaded = fixture_loaded();
    let (client_id_key, denied_path) = first_secret_path(&loaded);
    let resolver = move |region: &str, path: &str| -> Result<String, &'static str> {
        if path == denied_path {
            Err("resolver denied configured path")
        } else {
            support::fake_bolt_v3_resolver(region, path)
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for no-submit readiness test");
    let report = runtime.block_on(run_bolt_v3_no_submit_readiness_with(
        &loaded,
        |_| false,
        resolver,
    ));

    assert!(report.facts.iter().any(|fact| {
        fact.stage == BoltV3NoSubmitReadinessStage::SecretResolution
            && fact.subject == BoltV3NoSubmitReadinessSubject::Client(client_id_key.clone())
            && fact.status == BoltV3NoSubmitReadinessStatus::Failed
    }));
    for skipped_stage in [
        BoltV3NoSubmitReadinessStage::AdapterMapping,
        BoltV3NoSubmitReadinessStage::LiveNodeBuilder,
        BoltV3NoSubmitReadinessStage::ClientRegistration,
        BoltV3NoSubmitReadinessStage::LiveNodeBuild,
        BoltV3NoSubmitReadinessStage::Connect,
        BoltV3NoSubmitReadinessStage::Disconnect,
    ] {
        assert!(report.facts.iter().any(|fact| {
            fact.stage == skipped_stage
                && fact.subject
                    == BoltV3NoSubmitReadinessSubject::BlockedByStage(
                        BoltV3NoSubmitReadinessStage::SecretResolution,
                    )
                && fact.status == BoltV3NoSubmitReadinessStatus::Skipped
        }));
    }
}

#[test]
fn no_submit_readiness_adapter_mapping_failure_redacts_resolved_secrets() {
    let mut loaded = fixture_loaded();
    let field = "subscribe_new_markets";
    let client_id_key = first_client_with_data_field(&loaded, field);
    loaded
        .root
        .clients
        .get_mut(&client_id_key)
        .expect("fixture client should exist")
        .data
        .as_mut()
        .and_then(toml::Value::as_table_mut)
        .expect("fixture data block should be a TOML table")
        .insert(field.to_string(), toml::Value::Boolean(true));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for no-submit readiness test");
    let report = runtime.block_on(run_bolt_v3_no_submit_readiness_with(
        &loaded,
        |_| false,
        support::fake_bolt_v3_resolver,
    ));

    assert!(report.facts.iter().any(|fact| {
        fact.stage == BoltV3NoSubmitReadinessStage::AdapterMapping
            && fact.subject == BoltV3NoSubmitReadinessSubject::Client(client_id_key.clone())
            && fact.status == BoltV3NoSubmitReadinessStatus::Failed
    }));
    assert_no_resolved_secret_values(&loaded, &format!("{report:#?}"));
}

#[test]
fn no_submit_readiness_public_runner_uses_real_ssm_boundary() {
    let source = include_str!("../src/bolt_v3_no_submit_readiness.rs");
    let runner = source
        .split("pub fn run_bolt_v3_no_submit_readiness(")
        .nth(1)
        .expect("no-submit readiness module should expose a real-SSM public runner")
        .split("pub async fn run_bolt_v3_no_submit_readiness_with")
        .next()
        .expect("real-SSM runner should precede injected test helper");

    for required in [
        "check_no_forbidden_credential_env_vars(",
        "SsmResolverSession::new()",
        "resolve_bolt_v3_secrets(&session, loaded)",
        "build_no_submit_live_node_after_resolution(&mut report, loaded, &resolved)",
    ] {
        assert!(
            runner.contains(required),
            "real-SSM no-submit runner must use production startup boundary `{required}`"
        );
    }
    for forbidden in [
        "resolve_bolt_v3_secrets_with",
        "check_no_forbidden_credential_env_vars_with",
        "fake_bolt_v3_resolver",
        "std::env::var(",
        "std::env::var_os(",
    ] {
        assert!(
            !runner.contains(forbidden),
            "real-SSM no-submit runner must not use injected/env fallback boundary `{forbidden}`"
        );
    }

    let build_helper = source
        .split("fn build_no_submit_live_node_after_resolution(")
        .nth(1)
        .expect("no-submit readiness module should share one post-resolution build helper")
        .split("async fn connect_and_disconnect_for_no_submit_readiness")
        .next()
        .expect("post-resolution build helper should precede connect helper");
    assert!(
        build_helper.contains("map_bolt_v3_clients(loaded, resolved)"),
        "shared post-resolution helper must map clients through the production adapter mapper"
    );
}

#[test]
#[ignore = "requires explicit approval for real SSM secret resolution and authenticated Polymarket private execution connect; no submit/cancel path"]
fn external_polymarket_no_submit_readiness_uses_real_ssm_and_writes_redacted_report() {
    let config_path = std::env::var("BOLT_V3_NO_SUBMIT_READINESS_CONFIG_PATH")
        .expect("set BOLT_V3_NO_SUBMIT_READINESS_CONFIG_PATH to approved v3 root TOML");
    let report_path = std::env::var("BOLT_V3_NO_SUBMIT_READINESS_REPORT_PATH")
        .expect("set BOLT_V3_NO_SUBMIT_READINESS_REPORT_PATH to redacted report output path");
    let loaded = load_bolt_v3_config(Path::new(&config_path))
        .expect("approved no-submit v3 root TOML should load");

    let report = run_bolt_v3_no_submit_readiness(&loaded);
    let json =
        serde_json::to_string_pretty(&report).expect("no-submit readiness report should serialize");
    assert_no_resolved_secret_values(&loaded, &json);
    std::fs::write(&report_path, json).expect("redacted no-submit readiness report should write");
}

#[test]
fn no_submit_readiness_source_does_not_use_order_strategy_actor_or_runner_apis() {
    let source = include_str!("../src/bolt_v3_no_submit_readiness.rs");
    for forbidden in [
        ".run(",
        ".start(",
        "start_async",
        "kernel.start",
        "start_trader",
        "register_bolt_v3_strategies",
        "register_bolt_v3_reference_actors",
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
        "python",
        "PyO3",
        "maturin",
        "Command::new",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_no_submit_readiness.rs must stay no-submit; \
             source unexpectedly references `{forbidden}`"
        );
    }
}
