mod support;

use std::{collections::BTreeMap, path::Path, time::Duration};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3ClientConfig, BoltV3ClientConfigs, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig,
    },
    bolt_v3_client_registration::register_bolt_v3_clients,
    bolt_v3_config::{
        LoadedBoltV3Config, LoadedStrategy, REFERENCE_STREAM_ID_PARAMETER, ReferenceSourceType,
        load_bolt_v3_config,
    },
    bolt_v3_decision_events::{
        BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE, register_bolt_v3_decision_event_types,
    },
    bolt_v3_live_node::make_bolt_v3_live_node_builder,
    bolt_v3_reference_actor_registration::register_bolt_v3_reference_actors,
    bolt_v3_secrets::resolve_bolt_v3_secrets_with,
    bolt_v3_strategy_registration::register_bolt_v3_strategies,
    platform::{
        resolution_basis::parse_ruleset_resolution_basis,
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState},
        runtime::runtime_selection_topic,
    },
};
use nautilus_common::msgbus::publish_any;
use nautilus_model::{data::Data, identifiers::StrategyId};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde::Deserialize;
use support::{
    MockDataClientConfig, MockDataClientFactory, MockExecClientConfig, MockExecutionClientFactory,
    clear_mock_exec_submissions, recorded_mock_exec_submissions,
};
use tempfile::TempDir;
use tokio::time::sleep;

const NANOSECONDS_PER_MILLISECOND: u64 = 1_000_000;
const MILLISECONDS_PER_SECOND: u64 = 1_000;

#[derive(Debug, Deserialize)]
struct ScaleProcessScenario {
    event_settle_milliseconds: u64,
    delay_post_stop_seconds: u64,
    timeout_disconnection_seconds: u64,
    condition_suffix: String,
    up_token_suffix: String,
    down_token_suffix: String,
    question_suffix: String,
    price_to_beat: f64,
    accepting_orders: bool,
    liquidity_num: f64,
}

#[test]
fn multi_strategy_v3_node_routes_selection_to_only_addressed_strategy_topic() {
    register_bolt_v3_decision_event_types();
    clear_mock_exec_submissions();
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/root_multi.toml",
    ))
    .expect("multi-strategy v3 TOML should load");
    let scenario = scale_process_scenario();
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    loaded.root.nautilus.delay_post_stop_seconds = scenario.delay_post_stop_seconds;
    loaded.root.nautilus.timeout_disconnection_seconds = scenario.timeout_disconnection_seconds;
    let resolved = resolve_bolt_v3_secrets_with(&loaded, support::fake_bolt_v3_resolver)
        .expect("fixture secrets should resolve through fake SSM");
    let first_strategy = loaded
        .strategies
        .first()
        .expect("multi-strategy fixture should include first strategy");
    let second_strategy = loaded
        .strategies
        .get(1)
        .expect("multi-strategy fixture should include second strategy");
    let first_strategy_id = StrategyId::from(first_strategy.config.strategy_instance_id.as_str());
    let first_target_id = configured_target_id(first_strategy);
    let second_target_id = configured_target_id(second_strategy);
    let catalog_dir = Path::new(&loaded.root.persistence.catalog_directory).to_path_buf();

    let builder =
        make_bolt_v3_live_node_builder(&loaded).expect("v3 LiveNode builder should build");
    let (builder, _summary) = register_bolt_v3_clients(builder, mock_client_configs(&loaded))
        .expect("mock clients should register through v3 boundary");
    let mut node = builder.build().expect("mock LiveNode should build");
    register_bolt_v3_reference_actors(&mut node, &loaded)
        .expect("v3 reference actors should register on mock LiveNode");
    register_bolt_v3_strategies(&mut node, &loaded, &resolved)
        .expect("configured strategies should register from v3 TOML");

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build")
        .block_on(async {
            node.start()
                .await
                .expect("mock-only multi-strategy LiveNode should start");
            let published_at_ms = node.kernel().clock().borrow().timestamp_ns().as_u64()
                / NANOSECONDS_PER_MILLISECOND;
            publish_any(
                runtime_selection_topic(&first_strategy_id).into(),
                &selection_snapshot(&loaded, first_strategy, &scenario, published_at_ms),
            );
            sleep(Duration::from_millis(scenario.event_settle_milliseconds)).await;
            node.stop()
                .await
                .expect("mock-only multi-strategy LiveNode should stop");
        });

    assert!(
        recorded_mock_exec_submissions().is_empty(),
        "selection-topic routing proof must remain no-trade"
    );
    assert_eq!(
        market_selection_event_count(&catalog_dir, &first_target_id),
        1,
        "addressed strategy should persist one market-selection decision"
    );
    assert_eq!(
        market_selection_event_count(&catalog_dir, &second_target_id),
        0,
        "unaddressed strategy should not receive first strategy selection topic"
    );
}

fn mock_client_configs(loaded: &LoadedBoltV3Config) -> BoltV3ClientConfigs {
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

fn selection_snapshot(
    loaded: &LoadedBoltV3Config,
    strategy: &LoadedStrategy,
    scenario: &ScaleProcessScenario,
    published_at_ms: u64,
) -> RuntimeSelectionSnapshot {
    let market = candidate_market(loaded, strategy, scenario, published_at_ms);
    RuntimeSelectionSnapshot {
        ruleset_id: configured_target_id(strategy),
        decision: SelectionDecision {
            ruleset_id: configured_target_id(strategy),
            state: SelectionState::Active {
                market: market.clone(),
            },
        },
        eligible_candidates: vec![market],
        published_at_ms,
    }
}

fn candidate_market(
    loaded: &LoadedBoltV3Config,
    strategy: &LoadedStrategy,
    scenario: &ScaleProcessScenario,
    start_ts_ms: u64,
) -> CandidateMarket {
    let target_id = configured_target_id(strategy);
    let cadence_seconds = strategy.config.target["cadence_seconds"]
        .as_integer()
        .expect("fixture target should define cadence seconds")
        .try_into()
        .expect("cadence seconds should fit u64");
    let execution_client = loaded
        .root
        .clients
        .get(&strategy.config.execution_client_id)
        .expect("strategy execution client should exist");
    let venue = execution_client.venue.as_str();
    let condition_id = format!("{}-{}", target_id, scenario.condition_suffix);
    let up_token_id = format!("{}-{}", target_id, scenario.up_token_suffix);
    let down_token_id = format!("{}-{}", target_id, scenario.down_token_suffix);
    let up_instrument_id = format!("{condition_id}-{up_token_id}.{venue}");
    CandidateMarket {
        market_id: target_id.clone(),
        market_slug: target_id.clone(),
        question_id: format!("{}-{}", target_id, scenario.question_suffix),
        instrument_id: up_instrument_id,
        condition_id,
        up_token_id,
        down_token_id,
        selected_market_observed_ts_ms: start_ts_ms,
        price_to_beat: Some(scenario.price_to_beat),
        price_to_beat_source: Some(reference_publish_topic(loaded, strategy)),
        price_to_beat_observed_ts_ms: Some(start_ts_ms),
        start_ts_ms,
        end_ts_ms: start_ts_ms + cadence_seconds * MILLISECONDS_PER_SECOND,
        declared_resolution_basis: parse_ruleset_resolution_basis(&resolution_basis_key(
            loaded, strategy,
        ))
        .expect("selected reference stream resolution basis should parse"),
        accepting_orders: scenario.accepting_orders,
        liquidity_num: scenario.liquidity_num,
        seconds_to_end: cadence_seconds,
    }
}

fn scale_process_scenario() -> ScaleProcessScenario {
    let path = support::repo_path(
        "tests/fixtures/bolt_v3_existing_strategy/scale_process_selection_topic_isolation.toml",
    );
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} should be readable: {error}", path.display()));
    toml::from_str(&text).unwrap_or_else(|error| panic!("{} should parse: {error}", path.display()))
}

fn selected_reference_stream<'a>(
    loaded: &'a LoadedBoltV3Config,
    strategy: &LoadedStrategy,
) -> &'a bolt_v2::bolt_v3_config::ReferenceStreamBlock {
    let stream_id = strategy
        .config
        .parameters
        .get(REFERENCE_STREAM_ID_PARAMETER)
        .and_then(toml::Value::as_str)
        .expect("strategy should select reference stream");
    loaded
        .root
        .reference_streams
        .get(stream_id)
        .expect("selected reference stream should exist")
}

fn reference_publish_topic(loaded: &LoadedBoltV3Config, strategy: &LoadedStrategy) -> String {
    selected_reference_stream(loaded, strategy)
        .publish_topic
        .clone()
}

fn resolution_basis_key(loaded: &LoadedBoltV3Config, strategy: &LoadedStrategy) -> String {
    let stream = loaded
        .root
        .reference_streams
        .get(
            strategy
                .config
                .parameters
                .get(REFERENCE_STREAM_ID_PARAMETER)
                .and_then(toml::Value::as_str)
                .expect("strategy should select reference stream"),
        )
        .expect("selected reference stream should exist");
    let oracle_input = stream
        .inputs
        .iter()
        .find(|input| input.source_type == ReferenceSourceType::Oracle)
        .expect("fixture reference stream should define oracle input");
    let client_id = oracle_input
        .data_client_id
        .as_deref()
        .expect("oracle input should declare data client");
    let client = loaded
        .root
        .clients
        .get(client_id)
        .expect("oracle data client should exist");
    let symbol = oracle_input
        .instrument_id
        .split('.')
        .next()
        .expect("oracle instrument should include symbol");
    format!(
        "{}_{}",
        client.venue.as_str().to_ascii_lowercase(),
        symbol.to_ascii_lowercase()
    )
}

fn configured_target_id(strategy: &LoadedStrategy) -> String {
    strategy.config.target["configured_target_id"]
        .as_str()
        .expect("fixture target should define configured_target_id")
        .to_string()
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
        .expect("execution client should define account_id")
        .to_string()
}

fn market_selection_event_count(catalog_dir: &Path, configured_target_id: &str) -> usize {
    let ids = vec![configured_target_id.to_string()];
    ParquetDataCatalog::new(catalog_dir, None, None, None, None)
        .query_custom_data_dynamic(
            BOLT_V3_MARKET_SELECTION_DECISION_EVENT_TYPE,
            Some(&ids),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("market-selection events should query")
        .into_iter()
        .filter(|event| matches!(event, Data::Custom(_)))
        .count()
}
