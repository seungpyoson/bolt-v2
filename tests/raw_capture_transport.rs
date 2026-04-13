use bolt_v2::raw_capture_transport::{
    gamma_default_headers, market_subscribe_payload, market_token_ids_from_instruments,
    market_ws_config,
};
use nautilus_core::UnixNanos;
use nautilus_polymarket::http::{
    models::GammaMarket,
    parse::{create_instrument_from_def, parse_gamma_market},
};
use serde_json::json;

#[test]
fn builds_market_ws_config_with_nt_polymarket_defaults() {
    let cfg = market_ws_config("wss://example.test/ws".to_string());

    assert_eq!(cfg.url, "wss://example.test/ws");
    assert_eq!(cfg.heartbeat, Some(30));
    assert_eq!(cfg.reconnect_timeout_ms, Some(15_000));
    assert_eq!(cfg.reconnect_delay_initial_ms, Some(250));
    assert_eq!(cfg.reconnect_delay_max_ms, Some(5_000));
    assert_eq!(cfg.reconnect_backoff_factor, Some(2.0));
    assert_eq!(cfg.reconnect_jitter_ms, Some(200));
    assert_eq!(cfg.reconnect_max_attempts, None);
    assert_eq!(cfg.idle_timeout_ms, None);
}

#[test]
fn builds_nt_gamma_default_headers() {
    let headers = gamma_default_headers();

    assert_eq!(
        headers.get("Content-Type").map(String::as_str),
        Some("application/json")
    );
    assert!(headers.contains_key("user-agent"));
}

#[test]
fn builds_market_subscribe_payload_for_multiple_assets() {
    let payload =
        market_subscribe_payload(vec!["111".to_string(), "222".to_string()], true).unwrap();

    assert!(payload.contains("\"assets_ids\":[\"111\",\"222\"]"));
    assert!(payload.contains("\"custom_feature_enabled\":true"));
}

#[test]
fn extracts_token_ids_from_bootstrapped_instruments() {
    let market: GammaMarket = serde_json::from_value(json!({
        "id": "market-1",
        "conditionId": "0xcond1",
        "clobTokenIds": "[\"111\",\"222\"]",
        "outcomes": "[\"Yes\",\"No\"]",
        "question": "Q1",
        "acceptingOrders": true,
        "active": true,
        "closed": false,
        "slug": "btc-updown-5m",
        "negRisk": false
    }))
    .expect("gamma market fixture should deserialize");

    let defs = parse_gamma_market(&market).unwrap();
    let instruments = defs
        .iter()
        .map(|def| create_instrument_from_def(def, UnixNanos::from(1_u64)).unwrap())
        .collect::<Vec<_>>();

    let token_ids = market_token_ids_from_instruments(&instruments);

    assert_eq!(token_ids, vec!["111", "222"]);
}
