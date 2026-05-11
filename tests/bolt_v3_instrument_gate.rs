mod support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::{
    bolt_v3_adapters::{BoltV3MarketSelectionNowFn, map_bolt_v3_clients_with_market_identity},
    bolt_v3_client_registration::register_bolt_v3_clients,
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_instrument_readiness::{
        BoltV3InstrumentReadinessStatus, check_bolt_v3_instrument_readiness_for_start,
    },
    bolt_v3_live_node::{build_bolt_v3_live_node_with_summary, make_bolt_v3_live_node_builder},
    bolt_v3_market_families::updown::{candidates_for_target, plan_market_identity},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use nautilus_core::{Params, UnixNanos};
use nautilus_live::node::NodeState;
use nautilus_model::{
    enums::AssetClass,
    identifiers::InstrumentId,
    instruments::{InstrumentAny, binary_option::BinaryOption},
    types::{Currency, Price, Quantity},
};
use serde_json::{Value, json};
use support::{MockDataClientConfig, MockDataClientFactory, UpdownSelectedMarketReadinessRole};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
use tokio_tungstenite::accept_async;
use ustr::Ustr;

struct LocalPolymarketInstrumentServer {
    http_base_url: String,
    ws_market_url: String,
    observed_http_requests: Arc<tokio::sync::Mutex<Vec<String>>>,
    http_task: JoinHandle<()>,
    ws_task: JoinHandle<()>,
}

impl Drop for LocalPolymarketInstrumentServer {
    fn drop(&mut self) {
        self.http_task.abort();
        self.ws_task.abort();
    }
}

async fn start_local_polymarket_instrument_server(
    selected_market_slug: String,
    selected_market: Value,
) -> LocalPolymarketInstrumentServer {
    let observed_http_requests = Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let http_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("local Gamma listener should bind");
    let http_addr = http_listener
        .local_addr()
        .expect("local Gamma listener should expose addr");
    let http_requests = Arc::clone(&observed_http_requests);
    let http_task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _peer)) = http_listener.accept().await else {
                return;
            };
            let slug = selected_market_slug.clone();
            let market = selected_market.clone();
            let requests = Arc::clone(&http_requests);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 4096];
                let Ok(read) = socket.read(&mut buffer).await else {
                    return;
                };
                let request = String::from_utf8_lossy(&buffer[..read]);
                let request_line = request.lines().next().unwrap_or_default().to_string();
                requests.lock().await.push(request_line.clone());
                let body = if request_line.contains(&format!("slug={slug}")) {
                    serde_json::to_string(&vec![market]).expect("selected market should encode")
                } else {
                    "[]".to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    let ws_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("local Polymarket WS listener should bind");
    let ws_addr = ws_listener
        .local_addr()
        .expect("local Polymarket WS listener should expose addr");
    let ws_task = tokio::spawn(async move {
        let Ok((stream, _peer)) = ws_listener.accept().await else {
            return;
        };
        let Ok(mut websocket) = accept_async(stream).await else {
            return;
        };
        while websocket.next().await.is_some() {}
    });

    LocalPolymarketInstrumentServer {
        http_base_url: format!("http://{http_addr}"),
        ws_market_url: format!("ws://{ws_addr}/ws/market"),
        observed_http_requests,
        http_task,
        ws_task,
    }
}

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

fn polymarket_updown_option_from_fixture(
    market: &support::UpdownSelectedMarketFixture,
    leg: &support::UpdownSelectedMarketLegFixture,
) -> InstrumentAny {
    polymarket_updown_option(
        leg.instrument_id.as_str(),
        leg.token_id.as_str(),
        market.condition_id.as_str(),
        market.question_id.as_str(),
        market.market_slug.as_str(),
        leg.outcome.as_str(),
        market.start_ms,
        market.end_ms,
    )
}

fn unix_seconds_to_gamma_iso(seconds: i64) -> String {
    DateTime::<Utc>::from_timestamp(seconds, 0)
        .expect("test period timestamp should fit chrono")
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn selected_gamma_market(
    market_slug: &str,
    current_start_unix_seconds: i64,
    next_start_unix_seconds: i64,
) -> Value {
    json!({
        "id": format!("gamma-{current_start_unix_seconds}"),
        "conditionId": format!("0x{:064x}", current_start_unix_seconds),
        "questionID": format!("question-{market_slug}"),
        "clobTokenIds": format!("[\"{}01\", \"{}02\"]", current_start_unix_seconds, current_start_unix_seconds),
        "outcomes": "[\"Up\", \"Down\"]",
        "outcomePrices": "[\"0.50\", \"0.50\"]",
        "question": format!("Test market for {market_slug}"),
        "description": "local test market",
        "startDate": unix_seconds_to_gamma_iso(current_start_unix_seconds),
        "endDate": unix_seconds_to_gamma_iso(next_start_unix_seconds),
        "active": true,
        "closed": false,
        "acceptingOrders": true,
        "enableOrderBook": true,
        "orderPriceMinTickSize": 0.001,
        "orderMinSize": 5.0,
        "makerBaseFee": 0,
        "takerBaseFee": 0,
        "slug": market_slug,
        "negRisk": false
    })
}

#[test]
fn live_node_instrument_gate_blocks_missing_cache_targets_before_start() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root_multi.toml");
    let temp_dir = TempDir::new().unwrap();
    let mut loaded = load_bolt_v3_config(&root_path).expect("multi-strategy fixture should load");
    support::attach_test_release_identity_manifest(&mut loaded, temp_dir.path());
    let expected_targets = loaded
        .strategies
        .iter()
        .map(|strategy| {
            (
                strategy.config.strategy_instance_id.as_str(),
                strategy.config.target["configured_target_id"]
                    .as_str()
                    .expect("fixture target should define configured_target_id"),
            )
        })
        .collect::<Vec<_>>();
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
        report
            .facts
            .iter()
            .any(|fact| fact.strategy_instance_id == expected_targets[0].0
                && fact.configured_target_id == expected_targets[0].1
                && fact.detail.contains("instruments_not_in_cache")),
        "5m target should block on missing NT cache instruments: {:#?}",
        report.facts
    );
    assert!(
        report
            .facts
            .iter()
            .any(|fact| fact.strategy_instance_id == expected_targets[1].0
                && fact.configured_target_id == expected_targets[1].1
                && fact.detail.contains("instruments_not_in_cache")),
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
    let current_market = support::bolt_v3_updown_readiness_selected_market_fixture(
        UpdownSelectedMarketReadinessRole::Current,
    );
    {
        let mut cache = cache.borrow_mut();
        for leg in &current_market.legs {
            cache
                .add_instrument(polymarket_updown_option_from_fixture(&current_market, leg))
                .unwrap();
        }
    }

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
    assert!(
        report.facts[0]
            .detail
            .contains(current_market.market_slug.as_str())
    );
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
fn live_node_start_loads_selected_market_instruments_through_real_polymarket_data_client() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for real Polymarket data-client proof");

    runtime.block_on(async {
        let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml");
        let mut loaded = load_bolt_v3_config(&root_path).expect("strategy fixture should load");
        loaded.root.nautilus.delay_post_stop_seconds = 0;
        loaded.root.nautilus.timeout_disconnection_seconds = 1;

        let plan = plan_market_identity(&loaded).expect("strategy target should plan from TOML");
        let target = plan
            .updown_targets
            .first()
            .expect("fixture should define one updown target");
        let data_client_id = target.client_id_key.clone();
        let market_selection_timestamp_seconds = target
            .cadence_seconds
            .checked_mul(2)
            .and_then(|timestamp| timestamp.checked_add(1))
            .expect("test market-selection timestamp should fit i64");
        let market_selection_timestamp_milliseconds = market_selection_timestamp_seconds * 1_000;
        loaded
            .root
            .clients
            .retain(|client_id, _client| client_id == &data_client_id);
        {
            let client = loaded
                .root
                .clients
                .get_mut(&data_client_id)
                .expect("strategy data client should exist in root TOML");
            client.execution = None;
            client.secrets = None;
        }

        let candidates = candidates_for_target(target, market_selection_timestamp_seconds)
            .expect("target candidates should derive from TOML");
        let server = start_local_polymarket_instrument_server(
            candidates.current_market_slug.clone(),
            selected_gamma_market(
                candidates.current_market_slug.as_str(),
                candidates.current_period_start_unix_seconds,
                candidates.next_period_start_unix_seconds,
            ),
        )
        .await;

        {
            let client = loaded
                .root
                .clients
                .get_mut(&data_client_id)
                .expect("strategy data client should exist in root TOML");
            let data = client
                .data
                .as_mut()
                .and_then(toml::Value::as_table_mut)
                .expect("Polymarket data block should be a TOML table");
            data.insert(
                "base_url_http".to_string(),
                toml::Value::String(server.http_base_url.clone()),
            );
            data.insert(
                "base_url_gamma".to_string(),
                toml::Value::String(server.http_base_url.clone()),
            );
            data.insert(
                "base_url_data_api".to_string(),
                toml::Value::String(server.http_base_url.clone()),
            );
            data.insert(
                "base_url_ws".to_string(),
                toml::Value::String(server.ws_market_url.clone()),
            );
            data.insert("http_timeout_seconds".to_string(), toml::Value::Integer(1));
            data.insert("ws_timeout_seconds".to_string(), toml::Value::Integer(1));
        }

        let clock: BoltV3MarketSelectionNowFn =
            Arc::new(move || market_selection_timestamp_seconds);
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = map_bolt_v3_clients_with_market_identity(&loaded, &resolved, &plan, clock)
            .expect("Polymarket data client should map with market-identity filters");
        let builder =
            make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct");
        let (builder, _summary) = register_bolt_v3_clients(builder, adapters)
            .expect("real Polymarket data client should register");
        let mut node = builder
            .build()
            .expect("LiveNode should build with real Polymarket data client");

        let before_start = check_bolt_v3_instrument_readiness_for_start(
            &node,
            &loaded,
            market_selection_timestamp_milliseconds,
        )
        .expect("readiness check before start should not fail");
        assert!(!before_start.is_ready());

        node.start()
            .await
            .expect("local Polymarket-data-only LiveNode start should succeed");
        assert_eq!(node.state(), NodeState::Running);

        let after_start = check_bolt_v3_instrument_readiness_for_start(
            &node,
            &loaded,
            market_selection_timestamp_milliseconds,
        )
        .expect("readiness check after real Polymarket data-client start should not fail");
        assert!(
            after_start.is_ready(),
            "real NT Polymarket data-client startup should populate selected-market instruments: {after_start:#?}"
        );

        let observed = server.observed_http_requests.lock().await.clone();
        assert!(
            observed.iter().any(|line| line.contains(&format!(
                "slug={}",
                candidates.current_market_slug
            ))),
            "Polymarket provider should request current slug through local Gamma: {observed:#?}"
        );
        assert!(
            observed
                .iter()
                .any(|line| line.contains(&format!("slug={}", candidates.next_market_slug))),
            "Polymarket provider should request next slug through local Gamma: {observed:#?}"
        );

        node.stop()
            .await
            .expect("local Polymarket-data-only LiveNode stop should succeed");
    });
}

#[test]
#[ignore = "external Polymarket public-data canary; no secrets, no execution client, no live orders"]
fn external_polymarket_start_loads_selected_market_instruments_through_nt_data_events() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime should build for external Polymarket data-client canary");

    runtime.block_on(async {
        let root_path = support::repo_path("tests/fixtures/bolt_v3_existing_strategy/root.toml");
        let mut loaded = load_bolt_v3_config(&root_path).expect("strategy fixture should load");
        loaded.root.nautilus.delay_post_stop_seconds = 0;

        let plan = plan_market_identity(&loaded).expect("strategy target should plan from TOML");
        let target = plan
            .updown_targets
            .first()
            .expect("fixture should define one updown target");
        let data_client_id = target.client_id_key.clone();
        let market_selection_time = Utc::now();
        let market_selection_timestamp_seconds = market_selection_time.timestamp();
        let market_selection_timestamp_milliseconds = market_selection_time.timestamp_millis();
        loaded
            .root
            .clients
            .retain(|client_id, _client| client_id == &data_client_id);
        {
            let client = loaded
                .root
                .clients
                .get_mut(&data_client_id)
                .expect("strategy data client should exist in root TOML");
            client.execution = None;
            client.secrets = None;
        }

        let clock: BoltV3MarketSelectionNowFn =
            Arc::new(move || market_selection_timestamp_seconds);
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = map_bolt_v3_clients_with_market_identity(&loaded, &resolved, &plan, clock)
            .expect("Polymarket data client should map with TOML endpoints and market filters");
        let builder =
            make_bolt_v3_live_node_builder(&loaded).expect("v3 builder should construct");
        let (builder, _summary) = register_bolt_v3_clients(builder, adapters)
            .expect("real Polymarket data client should register");
        let mut node = builder
            .build()
            .expect("LiveNode should build with external Polymarket data client");

        let before_start = check_bolt_v3_instrument_readiness_for_start(
            &node,
            &loaded,
            market_selection_timestamp_milliseconds,
        )
        .expect("readiness check before external start should not fail");
        assert!(!before_start.is_ready());

        node.start()
            .await
            .expect("external Polymarket-data-only LiveNode start should succeed");
        assert_eq!(node.state(), NodeState::Running);

        let after_start = check_bolt_v3_instrument_readiness_for_start(
            &node,
            &loaded,
            market_selection_timestamp_milliseconds,
        )
        .expect("readiness check after external Polymarket data-client start should not fail");
        assert!(
            after_start.is_ready(),
            "external NT Polymarket data-client startup should populate selected-market instruments: {after_start:#?}"
        );

        node.stop()
            .await
            .expect("external Polymarket-data-only LiveNode stop should succeed");
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
