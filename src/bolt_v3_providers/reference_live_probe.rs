use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, anyhow};
use nautilus_network::mode::ConnectionMode;
use serde::Serialize;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_providers::{chainlink_reference, polyresearch},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
    bolt_v3_wire_boundary::{self, WireMessage, WireMessageHandler, WirePingHandler},
};

#[derive(Debug, Serialize)]
pub struct ReferenceLiveProbeReport {
    pub duration_secs: u64,
    pub chainlink: ChainlinkReferenceLiveProbeReport,
    pub polyresearch: PolyResearchReferenceLiveProbeReport,
}

#[derive(Debug, Serialize)]
pub struct ChainlinkReferenceLiveProbeReport {
    pub client_id: String,
    pub connection_mode: String,
    pub data_frames: u64,
    pub server_pings: u64,
    pub required_data_frames: u64,
}

#[derive(Debug, Serialize)]
pub struct PolyResearchReferenceLiveProbeReport {
    pub client_id: String,
    pub connection_mode: String,
    pub data_frames: u64,
    pub server_pings: u64,
}

pub async fn run_reference_live_probe(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> anyhow::Result<ReferenceLiveProbeReport> {
    let probe = loaded
        .root
        .reference_live_probe
        .as_ref()
        .ok_or_else(|| anyhow!("reference_live_probe must be configured"))?;
    if probe.duration_secs == 0 {
        return Err(anyhow!(
            "reference_live_probe.duration_secs must be positive"
        ));
    }
    if probe.min_chainlink_data_frames == 0 {
        return Err(anyhow!(
            "reference_live_probe.min_chainlink_data_frames must be positive"
        ));
    }
    let duration = Duration::from_secs(probe.duration_secs);
    let chainlink_config = chainlink_probe_config(loaded, resolved)?;
    let polyresearch_config = polyresearch_probe_config(loaded, resolved)?;

    let (chainlink, polyresearch) = tokio::try_join!(
        probe_chainlink_reference(
            probe.chainlink_client_id.as_str(),
            &chainlink_config,
            duration,
            probe.min_chainlink_data_frames,
        ),
        probe_polyresearch_reference(
            probe.polyresearch_client_id.as_str(),
            &polyresearch_config,
            duration,
        )
    )?;

    Ok(ReferenceLiveProbeReport {
        duration_secs: probe.duration_secs,
        chainlink,
        polyresearch,
    })
}

fn chainlink_probe_config(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> anyhow::Result<chainlink_reference::ChainlinkReferencePriceClientConfig> {
    let probe = loaded
        .root
        .reference_live_probe
        .as_ref()
        .ok_or_else(|| anyhow!("reference_live_probe must be configured"))?;
    let client = loaded
        .root
        .clients
        .get(probe.chainlink_client_id.as_str())
        .ok_or_else(|| {
            anyhow!(
                "reference_live_probe.chainlink_client_id `{}` is not configured",
                probe.chainlink_client_id
            )
        })?;
    chainlink_reference::reference_price_client_config(
        &loaded.root,
        probe.chainlink_client_id.as_str(),
        client,
        resolved,
    )
    .map_err(anyhow::Error::new)
}

fn polyresearch_probe_config(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> anyhow::Result<polyresearch::PolyResearchReferencePriceClientConfig> {
    let probe = loaded
        .root
        .reference_live_probe
        .as_ref()
        .ok_or_else(|| anyhow!("reference_live_probe must be configured"))?;
    let client = loaded
        .root
        .clients
        .get(probe.polyresearch_client_id.as_str())
        .ok_or_else(|| {
            anyhow!(
                "reference_live_probe.polyresearch_client_id `{}` is not configured",
                probe.polyresearch_client_id
            )
        })?;
    polyresearch::reference_price_client_config(
        probe.polyresearch_client_id.as_str(),
        client,
        resolved,
    )
    .map_err(anyhow::Error::new)
}

async fn probe_chainlink_reference(
    client_id: &str,
    config: &chainlink_reference::ChainlinkReferencePriceClientConfig,
    duration: Duration,
    required_data_frames: u64,
) -> anyhow::Result<ChainlinkReferenceLiveProbeReport> {
    let data_frames = Arc::new(AtomicU64::new(0));
    let server_pings = Arc::new(AtomicU64::new(0));
    let handler_data_frames = Arc::clone(&data_frames);
    let handler: WireMessageHandler = Arc::new(move |message| match message {
        WireMessage::Text(_) | WireMessage::Binary(_) => {
            handler_data_frames.fetch_add(1, Ordering::SeqCst);
        }
        WireMessage::Ping(_) | WireMessage::Pong(_) | WireMessage::Close => {}
    });
    let ping_counter = Arc::clone(&server_pings);
    let ping_handler: WirePingHandler = Arc::new(move |_| {
        ping_counter.fetch_add(1, Ordering::SeqCst);
    });
    let websocket_config = chainlink_reference::reference_price_websocket_config(config)
        .context("build Chainlink reference WebSocket config")?;
    let websocket = bolt_v3_wire_boundary::connect_websocket(
        websocket_config,
        Some(handler),
        Some(ping_handler),
        None,
        vec![],
        None,
    )
    .await
    .context("connect Chainlink reference WebSocket")?;
    tokio::time::sleep(duration).await;
    let connection_mode = websocket.connection_mode();
    let observed_data_frames = data_frames.load(Ordering::SeqCst);
    let observed_server_pings = server_pings.load(Ordering::SeqCst);
    websocket.disconnect().await;
    if connection_mode != ConnectionMode::Active {
        return Err(anyhow!(
            "Chainlink reference probe ended with connection_mode={connection_mode}"
        ));
    }
    if observed_data_frames < required_data_frames {
        return Err(anyhow!(
            "Chainlink reference probe observed {observed_data_frames} data frame(s), below reference_live_probe.min_chainlink_data_frames={required_data_frames}"
        ));
    }
    Ok(ChainlinkReferenceLiveProbeReport {
        client_id: client_id.to_string(),
        connection_mode: connection_mode.to_string(),
        data_frames: observed_data_frames,
        server_pings: observed_server_pings,
        required_data_frames,
    })
}

async fn probe_polyresearch_reference(
    client_id: &str,
    config: &polyresearch::PolyResearchReferencePriceClientConfig,
    duration: Duration,
) -> anyhow::Result<PolyResearchReferenceLiveProbeReport> {
    let data_frames = Arc::new(AtomicU64::new(0));
    let server_pings = Arc::new(AtomicU64::new(0));
    let handler_data_frames = Arc::clone(&data_frames);
    let handler: WireMessageHandler = Arc::new(move |message| match message {
        WireMessage::Text(_) | WireMessage::Binary(_) => {
            handler_data_frames.fetch_add(1, Ordering::SeqCst);
        }
        WireMessage::Ping(_) | WireMessage::Pong(_) | WireMessage::Close => {}
    });
    let ping_counter = Arc::clone(&server_pings);
    let ping_handler: WirePingHandler = Arc::new(move |_| {
        ping_counter.fetch_add(1, Ordering::SeqCst);
    });
    let websocket_config = polyresearch::reference_price_websocket_config(config)
        .map_err(anyhow::Error::msg)
        .context("build PolyResearch reference WebSocket config")?;
    let websocket = bolt_v3_wire_boundary::connect_websocket(
        websocket_config,
        Some(handler),
        Some(ping_handler),
        None,
        vec![],
        None,
    )
    .await
    .context("connect PolyResearch reference WebSocket")?;
    tokio::time::sleep(duration).await;
    let connection_mode = websocket.connection_mode();
    let observed_data_frames = data_frames.load(Ordering::SeqCst);
    let observed_server_pings = server_pings.load(Ordering::SeqCst);
    websocket.disconnect().await;
    if connection_mode != ConnectionMode::Active {
        return Err(anyhow!(
            "PolyResearch reference probe ended with connection_mode={connection_mode}"
        ));
    }
    Ok(PolyResearchReferenceLiveProbeReport {
        client_id: client_id.to_string(),
        connection_mode: connection_mode.to_string(),
        data_frames: observed_data_frames,
        server_pings: observed_server_pings,
    })
}
