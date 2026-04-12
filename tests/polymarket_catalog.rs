use std::{
    collections::HashMap,
    net::SocketAddr,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bolt_v2::{
    config::{RulesetConfig, RulesetVenueKind},
    platform::{
        polymarket_catalog::{
            load_candidate_markets_for_ruleset_with_gamma_client, polymarket_instrument_id,
        },
        resolution_basis::{
            CandleInterval, ResolutionBasis, ResolutionSourceKind, parse_declared_resolution_basis,
            parse_ruleset_resolution_basis,
        },
    },
};
use chrono::{Duration as ChronoDuration, Utc};
use nautilus_model::identifiers::InstrumentId;
use nautilus_polymarket::http::gamma::PolymarketGammaRawHttpClient;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use toml::Value as TomlValue;

fn polymarket_selector(tag_slug: &str) -> TomlValue {
    let mut selector = toml::map::Map::new();
    selector.insert("tag_slug".to_string(), TomlValue::String(tag_slug.to_string()));
    TomlValue::Table(selector)
}

fn polymarket_selector_with_prefix(tag_slug: &str, prefix: &str) -> TomlValue {
    let mut selector = toml::map::Map::new();
    selector.insert("tag_slug".to_string(), TomlValue::String(tag_slug.to_string()));
    selector.insert(
        "event_slug_prefix".to_string(),
        TomlValue::String(prefix.to_string()),
    );
    TomlValue::Table(selector)
}

fn exchange_candle(
    source: ResolutionSourceKind,
    pair: &str,
    interval: CandleInterval,
) -> ResolutionBasis {
    ResolutionBasis::ExchangeCandle {
        source,
        pair: pair.to_string(),
        interval,
    }
}

fn oracle_price_feed(source: ResolutionSourceKind, pair: &str) -> ResolutionBasis {
    ResolutionBasis::OraclePriceFeed {
        source,
        pair: pair.to_string(),
    }
}

#[derive(Clone)]
struct TestServerState {
    response_bodies: Arc<Vec<Value>>,
    request_count: Arc<AtomicUsize>,
}

fn ruleset() -> RulesetConfig {
    RulesetConfig {
        id: "btc-5m".to_string(),
        venue: RulesetVenueKind::Polymarket,
        selector: polymarket_selector("bitcoin"),
        resolution_basis: "binance_btcusdt_1m".to_string(),
        min_time_to_expiry_secs: 120,
        max_time_to_expiry_secs: 1_800,
        min_liquidity_num: 1_000.0,
        require_accepting_orders: true,
        freeze_before_end_secs: 300,
        selector_poll_interval_ms: 1_000,
        candidate_load_timeout_secs: 30,
    }
}

fn ruleset_with_prefix(prefix: &str) -> RulesetConfig {
    RulesetConfig {
        selector: polymarket_selector_with_prefix("bitcoin", prefix),
        ..ruleset()
    }
}

fn parse_request_target(request: &str) -> (&str, HashMap<String, String>) {
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("request line should include path");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params = query
        .split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect();
    (path, params)
}

async fn spawn_test_server(response_bodies: Vec<Value>) -> (SocketAddr, Arc<AtomicUsize>) {
    let request_count = Arc::new(AtomicUsize::new(0));
    let state = TestServerState {
        response_bodies: Arc::new(response_bodies),
        request_count: request_count.clone(),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let state = state.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]);
                let (path, params) = parse_request_target(&request);
                state.request_count.fetch_add(1, Ordering::Relaxed);

                let limit = params
                    .get("limit")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(100);
                let offset = params
                    .get("offset")
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(0);
                let page_index = offset / limit.max(1);
                let (status_line, body) = if path == "/events" {
                    if params.get("tag_slug").map(String::as_str) == Some("bitcoin") {
                        let body = state
                            .response_bodies
                            .get(page_index)
                            .cloned()
                            .unwrap_or_else(|| json!([]));
                        ("HTTP/1.1 200 OK", body.to_string())
                    } else if let Some(slug) = params.get("slug") {
                        let matching_events: Vec<Value> = state
                            .response_bodies
                            .iter()
                            .flat_map(|body| body.as_array().cloned().unwrap_or_default())
                            .filter(|event| event.get("slug").and_then(Value::as_str) == Some(slug.as_str()))
                            .collect();
                        ("HTTP/1.1 200 OK", Value::Array(matching_events).to_string())
                    } else {
                        (
                            "HTTP/1.1 400 Bad Request",
                            "expected /events?tag_slug=bitcoin or /events?slug=<event>".to_string(),
                        )
                    }
                } else {
                    ("HTTP/1.1 400 Bad Request", "expected /events".to_string())
                };
                let response = format!(
                    "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });

    (addr, request_count)
}

fn test_gamma_client(addr: SocketAddr) -> PolymarketGammaRawHttpClient {
    PolymarketGammaRawHttpClient::new(Some(format!("http://{addr}")), 5).unwrap()
}

async fn load_markets_from_event_markets(
    markets: Vec<Value>,
) -> (
    Vec<bolt_v2::platform::ruleset::CandidateMarket>,
    Arc<AtomicUsize>,
) {
    let (addr, request_count) =
        spawn_test_server(vec![json!([event_with_markets("event-1", markets)])]).await;
    let client = test_gamma_client(addr);
    let markets = load_candidate_markets_for_ruleset_with_gamma_client(&ruleset(), &client)
        .await
        .unwrap();

    (markets, request_count)
}

async fn load_markets_from_event_pages(
    event_pages: Vec<Vec<Value>>,
) -> (
    Vec<bolt_v2::platform::ruleset::CandidateMarket>,
    Arc<AtomicUsize>,
) {
    let response_bodies = event_pages.into_iter().map(Value::Array).collect();
    let (addr, request_count) = spawn_test_server(response_bodies).await;
    let client = test_gamma_client(addr);
    let markets = load_candidate_markets_for_ruleset_with_gamma_client(&ruleset(), &client)
        .await
        .unwrap();

    (markets, request_count)
}

async fn load_markets_from_ruleset_and_event_pages(
    ruleset: RulesetConfig,
    event_pages: Vec<Vec<Value>>,
) -> (
    Vec<bolt_v2::platform::ruleset::CandidateMarket>,
    Arc<AtomicUsize>,
) {
    let response_bodies = event_pages.into_iter().map(Value::Array).collect();
    let (addr, request_count) = spawn_test_server(response_bodies).await;
    let client = test_gamma_client(addr);
    let markets = load_candidate_markets_for_ruleset_with_gamma_client(&ruleset, &client)
        .await
        .unwrap();

    (markets, request_count)
}

fn event_with_markets(id: &str, markets: Vec<Value>) -> Value {
    json!({
        "id": id,
        "slug": "bitcoin",
        "title": "Bitcoin 5m",
        "markets": markets
    })
}

fn event_with_slug_and_markets(id: &str, slug: &str, title: &str, markets: Vec<Value>) -> Value {
    json!({
        "id": id,
        "slug": slug,
        "title": title,
        "markets": markets
    })
}

fn valid_market(end_date: String) -> Value {
    valid_market_with("market-good", "[\"111\",\"222\"]", end_date)
}

fn fixture_start_date() -> String {
    (Utc::now() - ChronoDuration::minutes(5)).to_rfc3339()
}

fn valid_market_with(id: &str, clob_token_ids: &str, end_date: String) -> Value {
    json!({
        "id": id,
        "questionID": "0xquestion1",
        "conditionId": "0xcondition1",
        "clobTokenIds": clob_token_ids,
        "outcomes": "[\"Up\",\"Down\"]",
        "question": "Will BTC finish green?",
        "description": "This market will resolve to \"Yes\" if the Binance 1 minute candle for BTCUSDT has a final close above the opening price. The resolution source for this market is Binance, specifically the BTCUSDT \"Close\" prices available with \"1m\" and \"Candles\" selected on the top bar.",
        "startDate": fixture_start_date(),
        "acceptingOrders": true,
        "liquidityNum": 4567.0,
        "endDate": end_date,
        "slug": id
    })
}

#[test]
fn parses_canonical_ruleset_resolution_bases() {
    assert_eq!(
        parse_ruleset_resolution_basis("binance_ethusdt_1m").unwrap(),
        exchange_candle(
            ResolutionSourceKind::Binance,
            "ethusdt",
            CandleInterval::OneMinute,
        )
    );
    assert_eq!(
        parse_ruleset_resolution_basis("chainlink_ethusd").unwrap(),
        oracle_price_feed(ResolutionSourceKind::Chainlink, "ethusd")
    );
}

#[test]
fn rejects_non_canonical_ruleset_resolution_bases() {
    for input in [
        "Binance_ethusdt_1m",
        "binance_eth/usdt_1m",
        "binance_ethusdt",
        "chainlink_ETHUSD",
        "chainlink_ethusd_1m",
    ] {
        assert!(
            parse_ruleset_resolution_basis(input).is_err(),
            "{input} should be rejected as non-canonical"
        );
    }
}

#[test]
fn resolution_basis_display_round_trips_canonical_strings() {
    for input in ["binance_ethusdt_1h", "chainlink_ethusd", "kraken_solusd_5m"] {
        let parsed = parse_ruleset_resolution_basis(input)
            .unwrap_or_else(|_| panic!("{input} should parse as canonical"));
        assert_eq!(parsed.to_string(), input);
    }
}

#[test]
fn parses_hourly_binance_basis_from_description() {
    let description = "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the ETH/USDT 1 hour candle that begins on the time and date specified in the title. The resolution source for this market is information from Binance, specifically the ETH/USDT pair. The relevant \"1H\" candle will be used once finalized.";
    assert_eq!(
        parse_declared_resolution_basis(Some(description)),
        Some(exchange_candle(
            ResolutionSourceKind::Binance,
            "ethusdt",
            CandleInterval::OneHour,
        ))
    );
}

#[test]
fn parses_chainlink_basis_from_description() {
    let description = "The resolution source for this market is information from Chainlink, specifically the ETH/USD data stream available at https://data.chain.link/streams/eth-usd.";
    assert_eq!(
        parse_declared_resolution_basis(Some(description)),
        Some(oracle_price_feed(ResolutionSourceKind::Chainlink, "ethusd"))
    );
}

#[test]
fn rejects_exchange_candle_description_without_interval() {
    assert_eq!(
        parse_declared_resolution_basis(Some(
            "The resolution source for this market is Binance, specifically the ETH/USDT pair."
        )),
        None
    );
}

#[test]
fn description_and_ruleset_parsers_land_on_same_canonical_basis() {
    let description = "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the ETH/USDT 1 hour candle that begins on the time and date specified in the title. The resolution source for this market is information from Binance, specifically the ETH/USDT pair. The relevant \"1H\" candle will be used once finalized.";
    assert_eq!(
        parse_declared_resolution_basis(Some(description)),
        Some(parse_ruleset_resolution_basis("binance_ethusdt_1h").unwrap())
    );
}

#[test]
fn chainlink_description_and_ruleset_parsers_land_on_same_canonical_basis() {
    let description = "The resolution source for this market is information from Chainlink, specifically the ETH/USD data stream available at https://data.chain.link/streams/eth-usd.";
    assert_eq!(
        parse_declared_resolution_basis(Some(description)),
        Some(parse_ruleset_resolution_basis("chainlink_ethusd").unwrap())
    );
}

#[test]
fn rejects_description_with_multiple_supported_sources() {
    let description = "The resolution source for this market references Binance ETH/USDT 1 hour candle data and Chainlink ETH/USD stream data.";
    assert_eq!(parse_declared_resolution_basis(Some(description)), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loads_candidate_markets_for_ruleset_and_translates_seconds_to_end() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_event_markets(vec![
        valid_market(end_date.clone()),
        json!({
            "id": "market-missing-basis",
            "questionID": "0xquestion2",
            "conditionId": "0xcondition2",
            "clobTokenIds": "[\"333\",\"444\"]",
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish red?",
            "description": "No known basis here.",
            "startDate": fixture_start_date(),
            "acceptingOrders": true,
            "liquidityNum": 9999.0,
            "endDate": end_date,
            "slug": "market-missing-basis"
        }),
    ])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert_eq!(markets.len(), 1);
    assert_eq!(markets[0].market_id, "market-good");
    assert_eq!(markets[0].instrument_id, "0xcondition1-111.POLYMARKET");
    assert_eq!(markets[0].condition_id, "0xcondition1");
    assert_eq!(markets[0].up_token_id, "111");
    assert_eq!(markets[0].down_token_id, "222");
    assert!(markets[0].start_ts_ms > 0);
    assert_eq!(
        markets[0].declared_resolution_basis,
        exchange_candle(
            ResolutionSourceKind::Binance,
            "btcusdt",
            CandleInterval::OneMinute,
        )
    );
    assert!(markets[0].accepting_orders);
    assert_eq!(markets[0].liquidity_num, 4567.0);
    assert!((1190..=1200).contains(&markets[0].seconds_to_end));
    assert_eq!(
        polymarket_instrument_id("0xcondition1", "111"),
        InstrumentId::from_str("0xcondition1-111.POLYMARKET").unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paginates_gamma_events_for_multi_page_tag_queries() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let mut first_page = Vec::with_capacity(100);
    first_page.push(event_with_markets(
        "event-page-1",
        vec![valid_market_with(
            "market-page-1",
            "[\"111\",\"222\"]",
            end_date.clone(),
        )],
    ));
    for index in 0..99 {
        first_page.push(event_with_markets(
            &format!("event-empty-{index}"),
            Vec::new(),
        ));
    }

    let second_page = vec![event_with_markets(
        "event-page-2",
        vec![valid_market_with(
            "market-page-2",
            "[\"333\",\"444\"]",
            end_date,
        )],
    )];

    let (markets, request_count) =
        load_markets_from_event_pages(vec![first_page, second_page]).await;

    assert_eq!(request_count.load(Ordering::Relaxed), 2);
    assert_eq!(markets.len(), 2);
    assert_eq!(
        markets
            .iter()
            .map(|market| market.market_id.as_str())
            .collect::<Vec<_>>(),
        vec!["market-page-1", "market-page-2"]
    );
    assert_eq!(
        markets
            .iter()
            .map(|market| market.instrument_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "0xcondition1-111.POLYMARKET",
            "0xcondition1-333.POLYMARKET"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prefix_selector_limits_catalog_candidates_to_matching_event_slugs() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_ruleset_and_event_pages(
        ruleset_with_prefix("bitcoin-5m"),
        vec![vec![
            event_with_slug_and_markets(
                "event-1",
                "bitcoin-5m-alpha",
                "Bitcoin 5m",
                vec![valid_market_with("market-prefix-match", "[\"111\",\"222\"]", end_date.clone())],
            ),
            event_with_slug_and_markets(
                "event-2",
                "bitcoin-15m-beta",
                "Bitcoin 15m",
                vec![valid_market_with("market-prefix-miss", "[\"333\",\"444\"]", end_date)],
            ),
        ]],
    )
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert_eq!(markets.len(), 1);
    assert_eq!(markets[0].market_id, "market-prefix-match");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_catalog_row_with_invalid_end_date() {
    let (markets, request_count) = load_markets_from_event_markets(vec![json!({
            "id": "market-invalid-end-date",
            "questionID": "0xquestion3",
            "conditionId": "0xcondition3",
            "clobTokenIds": "[\"111\",\"222\"]",
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish green?",
            "description": "The resolution source for this market is Binance spot BTC/USDT one-minute candles.",
            "startDate": fixture_start_date(),
            "acceptingOrders": true,
            "liquidityNum": 4567.0,
            "endDate": "not-a-date",
            "slug": "market-invalid-end-date"
    })])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert!(markets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_catalog_row_with_missing_accepting_orders() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_event_markets(vec![json!({
            "id": "market-missing-accepting-orders",
            "questionID": "0xquestion4",
            "conditionId": "0xcondition4",
            "clobTokenIds": "[\"111\",\"222\"]",
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish green?",
            "description": "The resolution source for this market is Binance spot BTC/USDT one-minute candles.",
            "startDate": fixture_start_date(),
            "liquidityNum": 4567.0,
            "endDate": end_date,
            "slug": "market-missing-accepting-orders"
        })])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert!(markets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_catalog_row_with_missing_liquidity_num() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_event_markets(vec![json!({
            "id": "market-missing-liquidity",
            "questionID": "0xquestion5",
            "conditionId": "0xcondition5",
            "clobTokenIds": "[\"111\",\"222\"]",
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish green?",
            "description": "The resolution source for this market is Binance spot BTC/USDT one-minute candles.",
            "startDate": fixture_start_date(),
            "acceptingOrders": true,
            "endDate": end_date,
            "slug": "market-missing-liquidity"
        })])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert!(markets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_catalog_row_with_malformed_clob_token_ids() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_event_markets(vec![json!({
            "id": "market-malformed-clob-token-ids",
            "questionID": "0xquestion6",
            "conditionId": "0xcondition6",
            "clobTokenIds": "not-json",
            "outcomes": "[\"Up\",\"Down\"]",
            "question": "Will BTC finish green?",
            "description": "The resolution source for this market is Binance spot BTC/USDT one-minute candles.",
            "startDate": fixture_start_date(),
            "acceptingOrders": true,
            "liquidityNum": 4567.0,
            "endDate": end_date,
            "slug": "market-malformed-clob-token-ids"
    })])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert!(markets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_catalog_row_with_unknown_basis() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_event_markets(vec![json!({
        "id": "market-unknown-basis",
        "questionID": "0xquestion7",
        "conditionId": "0xcondition7",
        "clobTokenIds": "[\"111\",\"222\"]",
        "outcomes": "[\"Up\",\"Down\"]",
        "question": "Will BTC finish green?",
        "description": "No known basis here.",
        "startDate": fixture_start_date(),
        "acceptingOrders": true,
        "liquidityNum": 4567.0,
        "endDate": end_date,
        "slug": "market-unknown-basis"
    })])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert!(markets.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_catalog_row_with_exchange_candle_description_missing_interval() {
    let end_date = (Utc::now() + ChronoDuration::minutes(20)).to_rfc3339();
    let (markets, request_count) = load_markets_from_event_markets(vec![json!({
        "id": "market-missing-interval",
        "questionID": "0xquestion8",
        "conditionId": "0xcondition8",
        "clobTokenIds": "[\"111\",\"222\"]",
        "outcomes": "[\"Up\",\"Down\"]",
        "question": "Will BTC finish green?",
        "description": "Resolution source: Binance spot BTC/USDT candles.",
        "startDate": fixture_start_date(),
        "acceptingOrders": true,
        "liquidityNum": 4567.0,
        "endDate": end_date,
        "slug": "market-missing-interval"
    })])
    .await;

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert!(markets.is_empty());
}
