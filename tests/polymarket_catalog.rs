use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use bolt_v2::{
    config::{RulesetConfig, RulesetVenueKind},
    platform::{
        polymarket_catalog::load_candidate_markets_for_ruleset,
        resolution_basis::parse_declared_resolution_basis,
    },
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[derive(Clone)]
struct TestServerState {
    response_body: Arc<Value>,
    request_count: Arc<AtomicUsize>,
}

fn ruleset() -> RulesetConfig {
    RulesetConfig {
        id: "btc-5m".to_string(),
        venue: RulesetVenueKind::Polymarket,
        tag_slug: "bitcoin".to_string(),
        resolution_basis: "binance_btcusdt_1m".to_string(),
        min_time_to_expiry_secs: 120,
        max_time_to_expiry_secs: 1_800,
        min_liquidity_num: 1_000.0,
        require_accepting_orders: true,
        freeze_before_end_secs: 300,
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

async fn spawn_test_server(response_body: Value) -> (SocketAddr, Arc<AtomicUsize>) {
    let request_count = Arc::new(AtomicUsize::new(0));
    let state = TestServerState {
        response_body: Arc::new(response_body),
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

                let (status_line, body) = if path == "/events"
                    && params.get("slug").map(String::as_str) == Some("bitcoin")
                {
                    ("HTTP/1.1 200 OK", state.response_body.to_string())
                } else {
                    (
                        "HTTP/1.1 400 Bad Request",
                        "expected slug query".to_string(),
                    )
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

#[test]
fn parses_declared_basis_from_structured_resolution_source() {
    assert_eq!(
        parse_declared_resolution_basis(
            Some("https://www.chain.link/streams/btc-usd"),
            Some("ignored"),
        ),
        Some("chainlink_btcusd".to_string())
    );
}

#[test]
fn parses_declared_basis_from_known_description_patterns() {
    assert_eq!(
        parse_declared_resolution_basis(
            None,
            Some("The resolution source for this market is Binance spot BTC/USDT data."),
        ),
        Some("binance_btcusdt_1m".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loads_candidate_markets_for_ruleset_and_skips_invalid_rows() {
    let response_body = json!([
        {
            "id": "event-1",
            "slug": "bitcoin",
            "title": "Bitcoin 5m",
            "markets": [
                {
                    "id": "market-good",
                    "questionID": "0xquestion1",
                    "conditionId": "0xcondition1",
                    "clobTokenIds": "[\"111\",\"222\"]",
                    "outcomes": "[\"Yes\",\"No\"]",
                    "question": "Will BTC finish green?",
                    "description": "The resolution source for this market is Binance spot BTC/USDT data.",
                    "resolutionSource": "https://www.binance.com/en/trade/BTC_USDT",
                    "acceptingOrders": true,
                    "liquidityNum": 4567.0,
                    "endDate": "2099-01-01T00:20:00Z",
                    "slug": "market-good"
                },
                {
                    "id": "market-missing-basis",
                    "questionID": "0xquestion2",
                    "conditionId": "0xcondition2",
                    "clobTokenIds": "[\"333\",\"444\"]",
                    "outcomes": "[\"Yes\",\"No\"]",
                    "question": "Will BTC finish red?",
                    "description": "No known basis here.",
                    "acceptingOrders": true,
                    "liquidityNum": 9999.0,
                    "endDate": "2099-01-01T00:25:00Z",
                    "slug": "market-missing-basis"
                }
            ]
        }
    ]);
    let (addr, request_count) = spawn_test_server(response_body).await;
    unsafe {
        std::env::set_var("POLYMARKET_GAMMA_URL", format!("http://{addr}"));
    }

    let markets = load_candidate_markets_for_ruleset(&ruleset(), 5).unwrap();

    unsafe {
        std::env::remove_var("POLYMARKET_GAMMA_URL");
    }

    assert_eq!(request_count.load(Ordering::Relaxed), 1);
    assert_eq!(markets.len(), 1);
    assert_eq!(markets[0].market_id, "market-good");
    assert_eq!(markets[0].instrument_id, "111");
    assert_eq!(markets[0].tag_slug, "bitcoin");
    assert_eq!(
        markets[0].declared_resolution_basis,
        "binance_btcusdt_1m".to_string()
    );
    assert!(markets[0].accepting_orders);
    assert_eq!(markets[0].liquidity_num, 4567.0);
}
