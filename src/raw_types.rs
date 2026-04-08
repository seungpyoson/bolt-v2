use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
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

pub struct JsonlAppender {
    current_path: Option<PathBuf>,
    writer: Option<BufWriter<File>>,
}

impl JsonlAppender {
    pub fn new() -> Self {
        Self {
            current_path: None,
            writer: None,
        }
    }

    pub fn append<T: Serialize>(&mut self, path: &Path, row: &T) -> anyhow::Result<()> {
        self.ensure_path(path)?;

        let writer = self
            .writer
            .as_mut()
            .expect("JsonlAppender writer must exist after ensure_path");
        serde_json::to_writer(&mut *writer, row)?;
        writer.write_all(b"\n")?;
        Ok(())
    }

    pub fn close(&mut self) -> anyhow::Result<()> {
        self.current_path = None;
        if let Some(mut writer) = self.writer.take() {
            writer.flush()?;
        }
        Ok(())
    }

    fn ensure_path(&mut self, path: &Path) -> anyhow::Result<()> {
        if self.current_path.as_deref() == Some(path) {
            return Ok(());
        }

        self.close()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(path)?;
        self.current_path = Some(path.to_path_buf());
        self.writer = Some(BufWriter::new(file));
        Ok(())
    }
}

impl Default for JsonlAppender {
    fn default() -> Self {
        Self::new()
    }
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
