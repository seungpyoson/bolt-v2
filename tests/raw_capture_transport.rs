use bolt_v2::raw_capture_transport::{
    gamma_default_headers, market_subscribe_payload, market_token_ids_from_instruments,
    market_ws_config,
};
use nautilus_core::UnixNanos;
use nautilus_polymarket::http::{
    models::GammaMarket,
    parse::{create_instrument_from_def, parse_gamma_market},
};

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
    let market = GammaMarket {
        id: "market-1".to_string(),
        condition_id: "0xcond1".to_string(),
        question_id: None,
        clob_token_ids: "[\"111\",\"222\"]".to_string(),
        outcomes: "[\"Yes\",\"No\"]".to_string(),
        question: "Q1".to_string(),
        description: None,
        start_date: None,
        end_date: None,
        active: Some(true),
        closed: Some(false),
        accepting_orders: Some(true),
        enable_order_book: None,
        order_price_min_tick_size: None,
        order_min_size: None,
        maker_base_fee: None,
        taker_base_fee: None,
        market_slug: Some("btc-updown-5m".to_string()),
        neg_risk: Some(false),
        liquidity_num: None,
        volume_num: None,
        volume_24hr: None,
        outcome_prices: None,
        best_bid: None,
        best_ask: None,
        spread: None,
        last_trade_price: None,
        one_day_price_change: None,
        one_week_price_change: None,
        volume_1wk: None,
        volume_1mo: None,
        volume_1yr: None,
        rewards_min_size: None,
        rewards_max_spread: None,
        competitive: None,
        category: None,
        neg_risk_market_id: None,
    };

    let defs = parse_gamma_market(&market).unwrap();
    let instruments = defs
        .iter()
        .map(|def| create_instrument_from_def(def, UnixNanos::from(1_u64)).unwrap())
        .collect::<Vec<_>>();

    let token_ids = market_token_ids_from_instruments(&instruments);

    assert_eq!(token_ids, vec!["111", "222"]);
}
