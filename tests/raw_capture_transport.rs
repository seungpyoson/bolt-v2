use bolt_v2::raw_capture_transport::{
    gamma_default_headers, gamma_markets_params, market_asset_id, market_subscribe_payload,
    market_token_ids_from_gamma_events_json, market_ws_config,
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
    let token_id = market_asset_id("0xabc-12345678901234567890.POLYMARKET").unwrap();

    assert_eq!(token_id, "12345678901234567890");
}

#[test]
fn builds_market_subscribe_payload_for_multiple_assets() {
    let payload =
        market_subscribe_payload(vec!["111".to_string(), "222".to_string()], true).unwrap();

    assert!(payload.contains("\"assets_ids\":[\"111\",\"222\"]"));
    assert!(payload.contains("\"custom_feature_enabled\":true"));
}

#[test]
fn extracts_all_token_ids_from_gamma_events_payload() {
    let json = r#"
        [
          {
            "id": "event-1",
            "slug": "btc-updown-5m",
            "markets": [
              {
                "id": "market-1",
                "conditionId": "0xcond1",
                "clobTokenIds": "[\"111\",\"222\"]",
                "outcomes": "[\"Yes\",\"No\"]",
                "question": "Q1"
              },
              {
                "id": "market-2",
                "conditionId": "0xcond2",
                "clobTokenIds": "[\"333\",\"444\"]",
                "outcomes": "[\"Yes\",\"No\"]",
                "question": "Q2"
              }
            ]
          }
        ]
    "#;

    let token_ids = market_token_ids_from_gamma_events_json(json).unwrap();

    assert_eq!(token_ids, vec!["111", "222", "333", "444"]);
}
