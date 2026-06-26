use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use nautilus_network::{transport::Message, websocket::MessageHandler};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::LoadedBoltV3Config, bolt_v3_secrets::ResolvedBoltV3Secrets,
    bolt_v3_wire_boundary,
};

use super::chainlink_reference;

const CHAINLINK_REFERENCE_CAPTURE_SCHEMA_VERSION: u32 = 1;
const CHAINLINK_REFERENCE_CAPTURE_KIND: &str = "chainlink-reference-fixture-capture";
const CHAINLINK_REFERENCE_CAPTURE_FRAME_KIND: &str = "binary";
const CHAINLINK_REFERENCE_CAPTURE_FRAME_FILENAME: &str = "chainlink-reference-frame.bin";
const CHAINLINK_REFERENCE_CAPTURE_PROVENANCE_FILENAME: &str = "ci-provenance.json";

#[derive(Debug, Clone)]
pub struct BoundaryFixtureCaptureRequest {
    pub client_key: String,
    pub output_dir: PathBuf,
    pub wait_timeout: Duration,
    pub provenance: BoundaryFixtureCaptureProvenance,
}

#[derive(Debug, Clone)]
pub struct BoundaryFixtureCaptureProvenance {
    pub repository: String,
    pub workflow_path: String,
    pub workflow_digest: String,
    pub provenance_config_digest: String,
    pub head_sha: String,
    pub head_branch: String,
    pub run_id: u64,
    pub run_attempt: u64,
    pub check_suite_id: u64,
    pub event: String,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct BoundaryFixtureCaptureReport {
    pub fixture_frame_path: String,
    pub fixture_frame_sha256: String,
    pub provenance_path: String,
    pub observed_binary_frames: usize,
    pub observed_text_frames: usize,
}

#[derive(Debug, Serialize)]
struct ChainlinkReferenceFixtureCaptureRecord<'a> {
    schema_version: u32,
    kind: &'static str,
    repository: &'a str,
    workflow_path: &'a str,
    workflow_digest: &'a str,
    provenance_config_digest: &'a str,
    head_sha: &'a str,
    tested_sha: &'a str,
    run_id: u64,
    run_attempt: u64,
    check_suite_id: u64,
    event: &'a str,
    head_branch: &'a str,
    pull_request: serde_json::Value,
    required_jobs: serde_json::Value,
    conditional_jobs: serde_json::Value,
    nextest_fingerprint: Option<String>,
    created_at: &'a str,
    capture: ChainlinkReferenceFixtureCaptureFields<'a>,
}

#[derive(Debug, Serialize)]
struct ChainlinkReferenceFixtureCaptureFields<'a> {
    record_kind: &'static str,
    adapter_id: &'a str,
    client_key: &'a str,
    frame_kind: &'static str,
    signature_verified: bool,
    fixture_filename: &'static str,
    fixture_sha256: &'a str,
    observed_binary_frames: usize,
    observed_text_frames: usize,
}

pub async fn capture_reference_boundary_fixture(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    request: BoundaryFixtureCaptureRequest,
) -> Result<BoundaryFixtureCaptureReport> {
    let client = loaded
        .root
        .clients
        .get(&request.client_key)
        .ok_or_else(|| anyhow::anyhow!("capture client_key is not configured"))?;
    let config = chainlink_reference::reference_price_client_config(
        &loaded.root,
        &request.client_key,
        client,
        resolved,
    )?;
    let websocket_config = chainlink_reference::reference_price_websocket_config(&config)?;

    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let observed_binary_frames = Arc::new(AtomicUsize::new(0));
    let observed_text_frames = Arc::new(AtomicUsize::new(0));
    let handler = chainlink_reference_capture_handler(
        sender,
        Arc::clone(&observed_binary_frames),
        Arc::clone(&observed_text_frames),
    );

    let websocket = bolt_v3_wire_boundary::connect_websocket(
        websocket_config,
        Some(handler),
        None,
        None,
        vec![],
        None,
    )
    .await?;
    let frame_result = tokio::time::timeout(request.wait_timeout, receiver.recv()).await;
    websocket.disconnect().await;
    let frame = frame_result
        .map_err(|_| anyhow::anyhow!("timed out waiting for Chainlink binary frame"))?
        .ok_or_else(|| anyhow::anyhow!("Chainlink binary frame channel closed before capture"))?;

    std::fs::create_dir_all(&request.output_dir)?;
    let frame_path = request
        .output_dir
        .join(CHAINLINK_REFERENCE_CAPTURE_FRAME_FILENAME);
    std::fs::write(&frame_path, &frame)?;
    let fixture_frame_sha256 = sha256_bytes(&frame);

    let provenance_path = request
        .output_dir
        .join(CHAINLINK_REFERENCE_CAPTURE_PROVENANCE_FILENAME);
    let record = ChainlinkReferenceFixtureCaptureRecord {
        schema_version: CHAINLINK_REFERENCE_CAPTURE_SCHEMA_VERSION,
        kind: "full-ci",
        repository: &request.provenance.repository,
        workflow_path: &request.provenance.workflow_path,
        workflow_digest: &request.provenance.workflow_digest,
        provenance_config_digest: &request.provenance.provenance_config_digest,
        head_sha: &request.provenance.head_sha,
        tested_sha: &request.provenance.head_sha,
        run_id: request.provenance.run_id,
        run_attempt: request.provenance.run_attempt,
        check_suite_id: request.provenance.check_suite_id,
        event: &request.provenance.event,
        head_branch: &request.provenance.head_branch,
        pull_request: serde_json::json!({
            "number": null,
            "base_sha": null,
        }),
        required_jobs: serde_json::json!({ "capture": "success" }),
        conditional_jobs: serde_json::json!({}),
        nextest_fingerprint: None,
        created_at: &request.provenance.created_at,
        capture: ChainlinkReferenceFixtureCaptureFields {
            record_kind: CHAINLINK_REFERENCE_CAPTURE_KIND,
            adapter_id: chainlink_reference::KEY,
            client_key: &request.client_key,
            frame_kind: CHAINLINK_REFERENCE_CAPTURE_FRAME_KIND,
            signature_verified: false,
            fixture_filename: CHAINLINK_REFERENCE_CAPTURE_FRAME_FILENAME,
            fixture_sha256: &fixture_frame_sha256,
            observed_binary_frames: observed_binary_frames.load(Ordering::SeqCst),
            observed_text_frames: observed_text_frames.load(Ordering::SeqCst),
        },
    };
    let record = serde_json::to_vec_pretty(&record)?;
    std::fs::write(&provenance_path, record)?;

    Ok(BoundaryFixtureCaptureReport {
        fixture_frame_path: display_path(&frame_path),
        fixture_frame_sha256,
        provenance_path: display_path(&provenance_path),
        observed_binary_frames: observed_binary_frames.load(Ordering::SeqCst),
        observed_text_frames: observed_text_frames.load(Ordering::SeqCst),
    })
}

fn chainlink_reference_capture_handler(
    sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    observed_binary_frames: Arc<AtomicUsize>,
    observed_text_frames: Arc<AtomicUsize>,
) -> MessageHandler {
    Arc::new(move |message: Message| match message {
        Message::Binary(bytes) => {
            observed_binary_frames.fetch_add(1, Ordering::SeqCst);
            let _ = sender.send(bytes.to_vec());
        }
        Message::Text(_) => {
            observed_text_frames.fetch_add(1, Ordering::SeqCst);
        }
        _ => {}
    })
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}
