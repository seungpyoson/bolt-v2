use std::path::PathBuf;

use bolt_v2::{
    config::Config,
    raw_capture_transport::{
        build_gamma_http_client, gamma_markets_params, gamma_markets_url, market_asset_id,
        market_subscribe_payload, market_ws_config,
    },
    raw_types::{RawHttpResponse, RawWsMessage, append_jsonl},
};
use clap::Parser;
use nautilus_network::{
    RECONNECTED,
    http::Method,
    websocket::{WebSocketClient, channel_message_handler},
};
use nautilus_polymarket::common::urls::clob_ws_market_url;

#[derive(Parser)]
struct Cli {
    #[arg(short, long)]
    config: PathBuf,
}

fn ws_output_path(base: &str, ingest_date: &str) -> PathBuf {
    PathBuf::from(base)
        .join("ws")
        .join(ingest_date)
        .join("messages.jsonl")
}

fn http_output_path(base: &str, ingest_date: &str) -> PathBuf {
    PathBuf::from(base)
        .join("http")
        .join(ingest_date)
        .join("responses.jsonl")
}

fn now_unix_nanos() -> u64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap() as u64
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let ingest_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let token_id = market_asset_id(&cfg.venue.instrument_id)?;

    let http_client = build_gamma_http_client(cfg.timeouts.connection_secs)?;
    let http_path = http_output_path(&cfg.raw_capture.output_dir, &ingest_date);
    let response = http_client
        .request_with_params(
            Method::GET,
            gamma_markets_url(),
            Some(&gamma_markets_params(&cfg.venue.event_slug)),
            None,
            None,
            None,
            None,
        )
        .await?;
    anyhow::ensure!(
        response.status.is_success(),
        "Gamma markets request failed with status {}",
        response.status.as_u16()
    );
    let body = String::from_utf8(response.body.to_vec())?;
    let http_row = RawHttpResponse {
        endpoint: "/markets".to_string(),
        request_params_json: format!("{{\"slug\":\"{}\"}}", cfg.venue.event_slug),
        received_ts: now_unix_nanos(),
        payload_json: body,
        source: "polymarket".to_string(),
        parser_version: "v1".to_string(),
        ingest_date: ingest_date.clone(),
    };
    append_jsonl(&http_path, &http_row)?;

    let (message_handler, mut raw_rx) = channel_message_handler();
    let ws_client = WebSocketClient::connect(
        market_ws_config(clob_ws_market_url().to_string()),
        Some(message_handler),
        None,
        None,
        vec![],
        None,
    )
    .await?;
    let subscribe_payload =
        market_subscribe_payload(token_id.clone(), cfg.venue.subscribe_new_markets)?;
    ws_client.send_text(subscribe_payload.clone(), None).await?;

    let ws_path = ws_output_path(&cfg.raw_capture.output_dir, &ingest_date);
    while let Some(message) = raw_rx.recv().await {
        if let Ok(text) = message.to_text() {
            if text == RECONNECTED {
                ws_client.send_text(subscribe_payload.clone(), None).await?;
                continue;
            }

            let row = RawWsMessage {
                stream_type: "market".to_string(),
                channel: "market".to_string(),
                market_id: None,
                instrument_id: Some(cfg.venue.instrument_id.clone()),
                received_ts: now_unix_nanos(),
                exchange_ts: None,
                payload_json: text.to_string(),
                source: "polymarket".to_string(),
                parser_version: "v1".to_string(),
                ingest_date: ingest_date.clone(),
            };
            append_jsonl(&ws_path, &row)?;
        }
    }

    anyhow::bail!("WebSocket message channel closed")
}
