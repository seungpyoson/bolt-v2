mod support;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_instrument_readiness::{
        BoltV3InstrumentReadinessStatus, check_bolt_v3_instrument_readiness_for_start,
    },
    bolt_v3_live_node::{build_bolt_v3_live_node_with_summary, make_bolt_v3_live_node_builder},
    bolt_v3_market_families::updown::{candidates_for_target, plan_market_identity},
};
use nautilus_core::{Params, UnixNanos};
use nautilus_live::node::NodeState;
use nautilus_model::{
    enums::AssetClass,
    identifiers::InstrumentId,
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Price, Quantity},
};
use serde_json::json;
use support::{MockDataClientConfig, MockDataClientFactory};
use tempfile::TempDir;
use ustr::Ustr;

fn polymarket_updown_option(
    instrument_id: &str,
    token_id: &str,
    condition_id: &str,
    question_id: &str,
    market_slug: &str,
    outcome: &str,
    start_ms: u64,
    end_ms: u64,
) -> InstrumentAny {
    let instrument_id = InstrumentId::from(instrument_id);
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    let mut info = Params::new();
    info.insert("token_id".to_string(), json!(token_id));
    info.insert("condition_id".to_string(), json!(condition_id));
    info.insert("question_id".to_string(), json!(question_id));
    info.insert("market_slug".to_string(), json!(market_slug));

    InstrumentAny::BinaryOption(BinaryOption::new(
        instrument_id,
        instrument_id.symbol,
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(start_ms * 1_000_000),
        UnixNanos::from(end_ms * 1_000_000),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        Some(Ustr::from(outcome)),
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
        Some(info),
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

#[test]
fn live_node_instrument_gate_blocks_missing_cache_targets_before_start() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml");
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&root_path).expect("multi-strategy fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register configured strategies");

    let report = check_bolt_v3_instrument_readiness_for_start(&node, &loaded, 601_000)
        .expect("readiness check should not fail on identity math");

    assert_eq!(node.state(), NodeState::Idle);
    assert!(!report.is_ready());
    assert_eq!(report.facts.len(), 2);
    assert!(
        report
            .facts
            .iter()
            .all(|fact| fact.status == BoltV3InstrumentReadinessStatus::Blocked),
        "empty NT cache must block every configured target: {:#?}",
        report.facts
    );
    assert!(
        report.facts.iter().any(
            |fact| fact.strategy_instance_id == "ETHCHAINLINKTAKER-V3-001"
                && fact.configured_target_id == "eth_updown_5m"
                && fact.detail.contains("instruments_not_in_cache")
        ),
        "5m target should block on missing NT cache instruments: {:#?}",
        report.facts
    );
    assert!(
        report.facts.iter().any(
            |fact| fact.strategy_instance_id == "ETHCHAINLINKTAKER-V3-015"
                && fact.configured_target_id == "eth_updown_15m"
                && fact.detail.contains("instruments_not_in_cache")
        ),
        "15m target should block on missing NT cache instruments: {:#?}",
        report.facts
    );
}

#[test]
fn live_node_instrument_gate_accepts_loaded_selected_market_before_start() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml");
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&root_path).expect("strategy fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let (node, _summary) =
        build_bolt_v3_live_node_with_summary(&loaded, |_| false, support::fake_bolt_v3_resolver)
            .expect("v3 LiveNode should build and register configured strategy");
    let cache = node.kernel().cache();
    cache
        .borrow_mut()
        .add_instrument(polymarket_updown_option(
            "0xcurrent-111.POLYMARKET",
            "111",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Up",
            600_000,
            900_000,
        ))
        .unwrap();
    cache
        .borrow_mut()
        .add_instrument(polymarket_updown_option(
            "0xcurrent-222.POLYMARKET",
            "222",
            "0xcurrent",
            "question-current",
            "eth-updown-5m-600",
            "Down",
            600_000,
            900_000,
        ))
        .unwrap();

    let report = check_bolt_v3_instrument_readiness_for_start(&node, &loaded, 601_000)
        .expect("readiness check should not fail on identity math");

    assert_eq!(node.state(), NodeState::Idle);
    assert!(
        report.is_ready(),
        "selected market should be ready: {report:#?}"
    );
    assert_eq!(report.facts.len(), 1);
    assert_eq!(
        report.facts[0].status,
        BoltV3InstrumentReadinessStatus::Ready
    );
    assert!(report.facts[0].detail.contains("selected_market"));
    assert!(report.facts[0].detail.contains("eth-updown-5m-600"));
}

#[test]
fn live_node_start_loads_selected_market_instruments_through_nt_data_events() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("strategy fixture should load");
    loaded.root.nautilus.delay_post_stop_seconds = 0;
    loaded.root.nautilus.timeout_disconnection_seconds = 1;
    let data_client_id = loaded.strategies[0].config.execution_client_id.clone();
    let venue = loaded
        .root
        .clients
        .get(&data_client_id)
        .expect("strategy data client should exist in root TOML")
        .venue
        .as_str()
        .to_string();
    let market_selection_timestamp_seconds = 601;
    let plan =
        plan_market_identity(&loaded).expect("strategy target should plan from fixture TOML");
    let target = plan
        .updown_targets
        .iter()
        .find(|target| target.client_id_key == data_client_id)
        .expect("strategy target should match configured data client");
    let candidates = candidates_for_target(target, market_selection_timestamp_seconds)
        .expect("target candidates should derive from fixture TOML");
    let current_start_milliseconds =
        u64::try_from(candidates.current_period_start_unix_seconds).unwrap() * 1_000;
    let next_start_milliseconds =
        u64::try_from(candidates.next_period_start_unix_seconds).unwrap() * 1_000;
    let condition_id = format!("condition-{}", candidates.current_market_slug);
    let up_token_id = format!("{}-UP", candidates.current_market_slug);
    let down_token_id = format!("{}-DOWN", candidates.current_market_slug);
    let instruments = vec![
        polymarket_updown_option(
            format!("{up_token_id}.{venue}").as_str(),
            up_token_id.as_str(),
            condition_id.as_str(),
            candidates.current_market_slug.as_str(),
            candidates.current_market_slug.as_str(),
            "Up",
            current_start_milliseconds,
            next_start_milliseconds,
        ),
        polymarket_updown_option(
            format!("{down_token_id}.{venue}").as_str(),
            down_token_id.as_str(),
            condition_id.as_str(),
            candidates.current_market_slug.as_str(),
            candidates.current_market_slug.as_str(),
            "Down",
            current_start_milliseconds,
            next_start_milliseconds,
        ),
    ];

    let mut node = make_bolt_v3_live_node_builder(&loaded)
        .expect("v3 builder should construct from fixture")
        .add_data_client(
            Some(data_client_id.clone()),
            Box::new(MockDataClientFactory),
            Box::new(
                MockDataClientConfig::new(data_client_id.as_str(), venue.as_str())
                    .with_startup_instruments(instruments),
            ),
        )
        .expect("mock Polymarket data client should register on builder")
        .build()
        .expect("LiveNode should build with mock Polymarket data client");

    let market_selection_timestamp_milliseconds = market_selection_timestamp_seconds * 1_000;
    let before_start = check_bolt_v3_instrument_readiness_for_start(
        &node,
        &loaded,
        market_selection_timestamp_milliseconds,
    )
    .expect("readiness check before start should not fail");
    assert!(!before_start.is_ready());

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for LiveNode start proof");

    runtime.block_on(async {
        node.start()
            .await
            .expect("mock-only LiveNode start should succeed");
        assert_eq!(node.state(), NodeState::Running);

        let after_start = check_bolt_v3_instrument_readiness_for_start(
            &node,
            &loaded,
            market_selection_timestamp_milliseconds,
        )
        .expect("readiness check after start should not fail");
        assert!(
            after_start.is_ready(),
            "NT startup data events should populate selected-market instruments: {after_start:#?}"
        );

        node.stop()
            .await
            .expect("mock-only LiveNode stop should succeed");
    });
}

#[test]
fn instrument_gate_wiring_has_no_start_run_order_or_subscription_calls() {
    let sources = [
        support::repo_path("src/bolt_v3_instrument_readiness.rs"),
        support::repo_path("src/bolt_v3_providers/polymarket.rs"),
    ]
    .map(|path| {
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()))
    });

    for forbidden in [
        ".start(",
        ".run(",
        "submit_order",
        "submit_order_list",
        "order_factory",
        "subscribe_book",
        "subscribe_quotes",
        "subscribe_trades",
        "subscribe_instruments",
    ] {
        assert!(
            sources.iter().all(|source| !source.contains(forbidden)),
            "bolt-v3 instrument readiness gate must not call `{forbidden}`"
        );
    }
}
