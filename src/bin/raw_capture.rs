use std::{collections::BTreeSet, path::PathBuf};

use anyhow::{Result, anyhow, bail, ensure};
use bolt_v2::{
    clients::polymarket::PolymarketDataClientInput,
    config::Config,
    raw_capture_transport::{
        build_gamma_http_client, build_gamma_instrument_client, gamma_events_params,
        gamma_events_url, market_subscribe_payload, market_token_ids_from_instruments,
        market_ws_config,
    },
    raw_types::{JsonlAppender, RawHttpResponse, RawWsMessage},
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

struct RawCaptureTargets {
    output_dir: String,
    timeout_connection_secs: u64,
    event_slugs: Vec<String>,
    subscribe_new_markets: bool,
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

fn current_ingest_date() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn collect_raw_capture_targets(cfg: &Config) -> Result<RawCaptureTargets> {
    let mut event_slugs = Vec::new();
    let mut subscribe_new_markets = false;

    for client in &cfg.data_clients {
        if client.kind != "polymarket" {
            continue;
        }

        let input: PolymarketDataClientInput = client.config.clone().try_into()?;
        subscribe_new_markets |= input.subscribe_new_markets;
        event_slugs.extend(input.event_slugs);
    }

    let event_slugs: Vec<String> = event_slugs
        .into_iter()
        .filter(|slug| !slug.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ensure!(
        !event_slugs.is_empty(),
        "raw capture requires at least one Polymarket event slug in data_clients"
    );

    Ok(RawCaptureTargets {
        output_dir: cfg.raw_capture.output_dir.clone(),
        timeout_connection_secs: cfg.node.timeout_connection_secs,
        event_slugs,
        subscribe_new_markets,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config).map_err(|e| anyhow!(e.to_string()))?;
    let targets = collect_raw_capture_targets(&cfg)?;
    let http_client = build_gamma_http_client(targets.timeout_connection_secs)?;
    let instrument_client = build_gamma_instrument_client(targets.timeout_connection_secs)?;
    let instruments = instrument_client
        .request_instruments_by_event_slugs(targets.event_slugs.clone())
        .await?;
    let token_ids = market_token_ids_from_instruments(&instruments);
    ensure!(
        !token_ids.is_empty() || targets.subscribe_new_markets,
        "raw capture could not resolve any Polymarket token IDs from event slugs"
    );

    let mut http_writer = JsonlAppender::new();
    for event_slug in &targets.event_slugs {
        let response = http_client
            .request_with_params(
                Method::GET,
                gamma_events_url(),
                Some(&gamma_events_params(event_slug)),
                None,
                None,
                None,
                None,
            )
            .await?;
        ensure!(
            response.status.is_success(),
            "Gamma events request failed with status {}",
            response.status.as_u16()
        );
        let body = String::from_utf8(response.body.to_vec())?;
        let ingest_date = current_ingest_date();
        let http_path = http_output_path(&targets.output_dir, &ingest_date);
        let http_row = RawHttpResponse {
            endpoint: "/events".to_string(),
            request_params_json: format!("{{\"slug\":\"{event_slug}\"}}"),
            received_ts: now_unix_nanos(),
            payload_json: body,
            source: "polymarket".to_string(),
            parser_version: "v1".to_string(),
            ingest_date: ingest_date.clone(),
        };
        http_writer.append(&http_path, &http_row)?;
    }
    http_writer.close()?;

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
    let subscribe_payload = market_subscribe_payload(token_ids, targets.subscribe_new_markets)?;
    ws_client.send_text(subscribe_payload.clone(), None).await?;

    let mut ws_writer = JsonlAppender::new();

    while let Some(message) = raw_rx.recv().await {
        if let Ok(text) = message.to_text() {
            if text == RECONNECTED {
                ws_client.send_text(subscribe_payload.clone(), None).await?;
                continue;
            }

            let ingest_date = current_ingest_date();
            let ws_path = ws_output_path(&targets.output_dir, &ingest_date);
            let row = RawWsMessage {
                stream_type: "market".to_string(),
                channel: "market".to_string(),
                market_id: None,
                instrument_id: None,
                received_ts: now_unix_nanos(),
                exchange_ts: None,
                payload_json: text.to_string(),
                source: "polymarket".to_string(),
                parser_version: "v1".to_string(),
                ingest_date: ingest_date.clone(),
            };
            ws_writer.append(&ws_path, &row)?;
        }
    }

    ws_writer.close()?;
    bail!("WebSocket message channel closed")
}
