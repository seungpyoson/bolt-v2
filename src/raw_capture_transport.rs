use std::collections::HashMap;

use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_model::instruments::{Instrument, InstrumentAny};
use nautilus_network::{
    http::{HttpClient, HttpClientError, USER_AGENT},
    retry::RetryConfig,
    websocket::WebSocketConfig,
};
use nautilus_polymarket::{
    common::urls::gamma_api_url,
    http::{
        gamma::PolymarketGammaHttpClient, query::GetGammaEventsParams,
        rate_limits::POLYMARKET_GAMMA_REST_QUOTA,
    },
    websocket::messages::MarketInitialSubscribeRequest,
};

pub fn gamma_default_headers() -> HashMap<String, String> {
    HashMap::from([
        (USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string()),
        ("Content-Type".to_string(), "application/json".to_string()),
    ])
}

pub fn build_gamma_http_client(timeout_secs: u64) -> Result<HttpClient, HttpClientError> {
    HttpClient::new(
        gamma_default_headers(),
        vec![],
        vec![],
        Some(*POLYMARKET_GAMMA_REST_QUOTA),
        Some(timeout_secs),
        None,
    )
}

pub fn build_gamma_instrument_client(
    timeout_secs: u64,
) -> Result<PolymarketGammaHttpClient, HttpClientError> {
    PolymarketGammaHttpClient::new(None, timeout_secs, RetryConfig::default())
}

pub fn gamma_events_url() -> String {
    format!("{}/events", gamma_api_url())
}

pub fn gamma_events_params(event_slug: &str) -> GetGammaEventsParams {
    GetGammaEventsParams {
        slug: Some(event_slug.to_string()),
        ..Default::default()
    }
}

pub fn market_ws_config(url: String) -> WebSocketConfig {
    WebSocketConfig {
        url,
        headers: vec![],
        heartbeat: Some(30),
        heartbeat_msg: None,
        reconnect_timeout_ms: Some(15_000),
        reconnect_delay_initial_ms: Some(250),
        reconnect_delay_max_ms: Some(5_000),
        reconnect_backoff_factor: Some(2.0),
        reconnect_jitter_ms: Some(200),
        reconnect_max_attempts: None,
        idle_timeout_ms: None,
    }
}

pub fn market_subscribe_payload(
    token_ids: Vec<String>,
    subscribe_new_markets: bool,
) -> anyhow::Result<String> {
    serde_json::to_string(&MarketInitialSubscribeRequest {
        assets_ids: token_ids,
        msg_type: "market",
        custom_feature_enabled: subscribe_new_markets,
    })
    .map_err(Into::into)
}

pub fn market_token_ids_from_instruments(instruments: &[InstrumentAny]) -> Vec<String> {
    let mut token_ids: Vec<String> = instruments
        .iter()
        .map(|instrument| instrument.raw_symbol().as_str().to_string())
        .collect();
    token_ids.sort();
    token_ids.dedup();
    token_ids
}
