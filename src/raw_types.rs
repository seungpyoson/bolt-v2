use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawWsMessage {
    pub stream_type: String,
    pub channel: String,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub received_ts: u64,
    pub exchange_ts: Option<u64>,
    pub payload_json: String,
    pub source: String,
    pub parser_version: String,
    pub ingest_date: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawHttpResponse {
    pub endpoint: String,
    pub request_params_json: String,
    pub received_ts: u64,
    pub payload_json: String,
    pub source: String,
    pub parser_version: String,
    pub ingest_date: String,
}

pub fn append_jsonl<T: Serialize>(path: &Path, row: &T) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, row)?;
    file.write_all(b"\n")?;
    Ok(())
}
