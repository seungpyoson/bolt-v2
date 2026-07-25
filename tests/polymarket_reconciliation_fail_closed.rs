use std::{cell::RefCell, net::SocketAddr, rc::Rc};

use nautilus_common::cache::Cache;
use nautilus_live::ExecutionClientCore;
use nautilus_model::{
    enums::{AccountType, OmsType},
    identifiers::{AccountId, TraderId},
};
use nautilus_polymarket::{
    common::consts::{POLYMARKET_CLIENT_ID, POLYMARKET_VENUE},
    config::PolymarketExecClientConfig,
    execution::PolymarketExecutionClient,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

const UNMAPPED_TOKEN: &str = "UNMAPPED-TOKEN";
const TEST_PRIVATE_KEY: &str = "0x4242424242424242424242424242424242424242424242424242424242424242";
const TEST_API_KEY: &str = "test-api-key";
const TEST_API_SECRET: &str = "YWJj";
const TEST_API_PASSPHRASE: &str = "test-passphrase";

async fn json_server(bodies: Vec<String>) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test HTTP listener should bind");
    let address = listener
        .local_addr()
        .expect("test HTTP listener should expose address");
    let handle = tokio::spawn(async move {
        for body in bodies {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("test HTTP listener should accept a request");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream
                    .read(&mut buffer)
                    .await
                    .expect("test HTTP listener should read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            assert!(
                !request.is_empty(),
                "Polymarket reconciliation should issue the expected HTTP request"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("test HTTP listener should write response");
        }
    });
    (address, handle)
}

fn execution_client(address: SocketAddr) -> PolymarketExecutionClient {
    let cache = Rc::new(RefCell::new(Cache::default()));
    let core = ExecutionClientCore::new(
        TraderId::from("TESTER-001"),
        *POLYMARKET_CLIENT_ID,
        *POLYMARKET_VENUE,
        OmsType::Netting,
        AccountId::from("POLYMARKET-001"),
        AccountType::Cash,
        None,
        cache,
    );
    let config = PolymarketExecClientConfig {
        private_key: Some(TEST_PRIVATE_KEY.to_string()),
        api_key: Some(TEST_API_KEY.to_string()),
        api_secret: Some(TEST_API_SECRET.to_string()),
        passphrase: Some(TEST_API_PASSPHRASE.to_string()),
        base_url_http: Some(format!("http://{address}")),
        base_url_ws: Some(format!("ws://{address}/ws")),
        base_url_data_api: Some(format!("http://{address}")),
        http_timeout_secs: 5,
        max_retries: 0,
        ..PolymarketExecClientConfig::default()
    };
    PolymarketExecutionClient::new(core, config)
        .expect("test Polymarket execution client should construct")
}

#[tokio::test]
async fn unmapped_venue_open_order_fails_startup_mass_status() {
    let body = serde_json::json!({
        "data": [{
            "associate_trades": [],
            "id": "0xaaaa000000000000000000000000000000000000000000000000000000000001",
            "status": "LIVE",
            "market": "0xdd22472e552920b8438158ea7238bfadfa4f736aa4cee91a6b86c39ead110917",
            "original_size": "1.0000",
            "outcome": "Yes",
            "maker_address": "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
            "owner": "test-api-key",
            "price": "0.5000",
            "side": "BUY",
            "size_matched": "0.0000",
            "asset_id": UNMAPPED_TOKEN,
            "expiration": null,
            "order_type": "GTC",
            "created_at": 1703875200
        }],
        "next_cursor": "LTE="
    })
    .to_string();
    let (address, server) = json_server(vec![body]).await;
    let client = execution_client(address);

    let error = client
        .generate_mass_status(None)
        .await
        .expect_err("unmapped venue open orders must fail NT startup mass status");
    server.await.expect("test HTTP server should finish");

    assert!(
        error
            .to_string()
            .contains("cannot map venue open order asset"),
        "{error}"
    );
}

#[tokio::test]
async fn unmapped_confirmed_fill_fails_startup_mass_status() {
    let body = serde_json::json!({
        "data": [{
            "id": "trade-1",
            "taker_order_id": "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef12",
            "market": "0xdd22472e552920b8438158ea7238bfadfa4f736aa4cee91a6b86c39ead110917",
            "asset_id": UNMAPPED_TOKEN,
            "side": "BUY",
            "size": "1.0000",
            "fee_rate_bps": "0",
            "price": "0.5000",
            "status": "CONFIRMED",
            "match_time": "2024-01-01T00:00:00Z",
            "last_update": "2024-01-01T00:01:00Z",
            "outcome": "Yes",
            "bucket_index": 0,
            "owner": "test-api-key",
            "maker_address": "0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266",
            "transaction_hash": "0xabcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
            "maker_orders": [],
            "trader_side": "TAKER"
        }],
        "next_cursor": "LTE="
    })
    .to_string();
    let empty_orders = serde_json::json!({
        "data": [],
        "next_cursor": "LTE="
    })
    .to_string();
    let (address, server) = json_server(vec![empty_orders, body]).await;
    let client = execution_client(address);

    let error = client
        .generate_mass_status(None)
        .await
        .expect_err("unmapped confirmed fills must fail NT startup mass status");
    server.await.expect("test HTTP server should finish");

    assert!(
        error
            .to_string()
            .contains("cannot map confirmed taker fill"),
        "{error}"
    );
}

#[tokio::test]
async fn unrepresentable_position_fails_startup_mass_status() {
    let body = serde_json::json!([{
        "asset": UNMAPPED_TOKEN,
        "conditionId": "0xcondition",
        "size": "1.0000",
        "avgPrice": "0.5000"
    }])
    .to_string();
    let empty_orders = serde_json::json!({
        "data": [],
        "next_cursor": "LTE="
    })
    .to_string();
    let empty_trades = serde_json::json!({
        "data": [],
        "next_cursor": "LTE="
    })
    .to_string();
    let (address, server) = json_server(vec![empty_orders, empty_trades, body]).await;
    let client = execution_client(address);

    let error = client
        .generate_mass_status(None)
        .await
        .expect_err("unrepresentable positions must fail NT startup mass status");
    server.await.expect("test HTTP server should finish");

    assert!(
        error.to_string().contains("cannot map Data API position"),
        "{error}"
    );
}
