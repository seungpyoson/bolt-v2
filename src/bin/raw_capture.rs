use std::{collections::BTreeSet, path::PathBuf};

use anyhow::{Result, anyhow, bail, ensure};
use bolt_v2::{
    clients::polymarket::PolymarketDataClientInput,
    config::Config,
    raw_capture_transport::{
        build_gamma_http_client, gamma_markets_params, gamma_markets_url, market_asset_id,
        market_subscribe_payload, market_ws_config,
    },
    raw_types::{RawHttpResponse, RawWsMessage, append_jsonl},
    strategies::exec_tester::ExecTesterInput,
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
    instrument_ids: Vec<String>,
    token_ids: Vec<String>,
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

    let mut instrument_ids = Vec::new();
    for strategy in &cfg.strategies {
        if strategy.kind != "exec_tester" {
            continue;
        }

        let input: ExecTesterInput = strategy.config.clone().try_into()?;
        instrument_ids.push(input.instrument_id);
    }

    let instrument_ids: Vec<String> = instrument_ids
        .into_iter()
        .filter(|instrument_id| !instrument_id.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    ensure!(
        !instrument_ids.is_empty(),
        "raw capture requires at least one exec_tester instrument_id in strategies"
    );

    let mut token_ids = Vec::with_capacity(instrument_ids.len());
    for instrument_id in &instrument_ids {
        token_ids.push(market_asset_id(instrument_id)?);
    }

    Ok(RawCaptureTargets {
        output_dir: cfg.raw_capture.output_dir.clone(),
        timeout_connection_secs: cfg.node.timeout_connection_secs,
        event_slugs,
        instrument_ids,
        token_ids,
        subscribe_new_markets,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load(&cli.config).map_err(|e| anyhow!(e.to_string()))?;
    let targets = collect_raw_capture_targets(&cfg)?;
    let ingest_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let http_client = build_gamma_http_client(targets.timeout_connection_secs)?;
    let http_path = http_output_path(&targets.output_dir, &ingest_date);

    for event_slug in &targets.event_slugs {
        let response = http_client
            .request_with_params(
                Method::GET,
                gamma_markets_url(),
                Some(&gamma_markets_params(event_slug)),
                None,
                None,
                None,
                None,
            )
            .await?;
        ensure!(
            response.status.is_success(),
            "Gamma markets request failed with status {}",
            response.status.as_u16()
        );
        let body = String::from_utf8(response.body.to_vec())?;
        let http_row = RawHttpResponse {
            endpoint: "/markets".to_string(),
            request_params_json: format!("{{\"slug\":\"{event_slug}\"}}"),
            received_ts: now_unix_nanos(),
            payload_json: body,
            source: "polymarket".to_string(),
            parser_version: "v1".to_string(),
            ingest_date: ingest_date.clone(),
        };
        append_jsonl(&http_path, &http_row)?;
    }

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
        market_subscribe_payload(targets.token_ids.clone(), targets.subscribe_new_markets)?;
    ws_client.send_text(subscribe_payload.clone(), None).await?;

    let ws_path = ws_output_path(&targets.output_dir, &ingest_date);
    let single_instrument_id = if targets.instrument_ids.len() == 1 {
        Some(targets.instrument_ids[0].clone())
    } else {
        None
    };

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
                instrument_id: single_instrument_id.clone(),
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

    bail!("WebSocket message channel closed")
}
