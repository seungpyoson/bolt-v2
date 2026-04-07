use std::collections::HashMap;

use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_model::identifiers::InstrumentId;
use nautilus_network::{
    http::{HttpClient, HttpClientError, USER_AGENT},
    websocket::WebSocketConfig,
};
use nautilus_polymarket::{
    common::urls::gamma_api_url,
    http::{query::GetGammaMarketsParams, rate_limits::POLYMARKET_GAMMA_REST_QUOTA},
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

pub fn gamma_markets_url() -> String {
    format!("{}/markets", gamma_api_url())
}

pub fn gamma_markets_params(event_slug: &str) -> GetGammaMarketsParams {
    GetGammaMarketsParams {
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

pub fn market_asset_id(instrument_id: &str) -> anyhow::Result<String> {
    let instrument_id = InstrumentId::from_as_ref(instrument_id)?;
    let symbol = instrument_id.symbol.as_str();
    let (_, token_id) = symbol
        .rsplit_once('-')
        .ok_or_else(|| anyhow::anyhow!("Expected condition-token symbol in {symbol}"))?;
    Ok(token_id.to_string())
}

pub fn market_subscribe_payload(
    token_id: String,
    subscribe_new_markets: bool,
) -> anyhow::Result<String> {
    serde_json::to_string(&MarketInitialSubscribeRequest {
        assets_ids: vec![token_id],
        msg_type: "market",
        custom_feature_enabled: subscribe_new_markets,
    })
    .map_err(Into::into)
}
