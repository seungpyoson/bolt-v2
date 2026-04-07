use std::{path::PathBuf, time::Duration};

use bolt_v2::{
    config::Config,
    raw_types::{RawHttpResponse, RawWsMessage, append_jsonl},
};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use nautilus_cryptography::providers::install_cryptographic_provider;
use nautilus_model::identifiers::InstrumentId;
use nautilus_polymarket::{
    common::urls::{clob_ws_market_url, gamma_api_url},
    websocket::messages::MarketInitialSubscribeRequest,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};

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

fn market_asset_id(instrument_id: &str) -> anyhow::Result<String> {
    let instrument_id = InstrumentId::from_as_ref(instrument_id)?;
    let symbol = instrument_id.symbol.as_str();
    let (_, token_id) = symbol
        .rsplit_once('-')
        .ok_or_else(|| anyhow::anyhow!("Expected condition-token symbol in {symbol}"))?;
    Ok(token_id.to_string())
}

fn now_unix_nanos() -> u64 {
    chrono::Utc::now().timestamp_nanos_opt().unwrap() as u64
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config)?;
    let ingest_date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let token_id = market_asset_id(&cfg.venue.instrument_id)?;

    // Tokio Tungstenite's rustls path needs a process-default provider before opening wss://.
    install_cryptographic_provider();

    let http_client = reqwest::Client::new();
    let http_path = http_output_path(&cfg.raw_capture.output_dir, &ingest_date);
    let url = format!("{}/markets?slug={}", gamma_api_url(), cfg.venue.event_slug);
    let body = http_client.get(&url).send().await?.text().await?;
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

    let ws_path = ws_output_path(&cfg.raw_capture.output_dir, &ingest_date);
    loop {
        let (stream, _) = connect_async(clob_ws_market_url()).await?;
        let (mut write, mut read) = stream.split();

        let payload = serde_json::to_string(&MarketInitialSubscribeRequest {
            assets_ids: vec![token_id.clone()],
            msg_type: "market",
            custom_feature_enabled: cfg.venue.subscribe_new_markets,
        })?;
        write.send(Message::Text(payload)).await?;

        while let Some(message) = read.next().await {
            let message = message?;
            if let Message::Text(text) = message {
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

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}
