use bolt_v2::raw_capture_transport::{
    gamma_default_headers, gamma_markets_params, market_asset_id, market_ws_config,
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
fn builds_gamma_market_query_for_slug() {
    let params = gamma_markets_params("election-2028");

    assert_eq!(params.slug.as_deref(), Some("election-2028"));
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
fn extracts_market_asset_id_from_instrument_id() {
    let token_id = market_asset_id(
        "0xabc-12345678901234567890.POLYMARKET",
    )
    .unwrap();

    assert_eq!(token_id, "12345678901234567890");
}
