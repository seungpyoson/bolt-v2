use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::bolt_v3_config::LoadedBoltV3Config;

pub trait BoltV3DecisionEvidenceWriter: std::fmt::Debug + Send + Sync {
    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentKind {
    Entry,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3OrderIntentEvidence {
    pub strategy_id: String,
    pub intent_kind: BoltV3OrderIntentKind,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
}

#[derive(Debug)]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    file: Mutex<std::fs::File>,
}

impl JsonlBoltV3DecisionEvidenceWriter {
    pub fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self> {
        let path = decision_evidence_path(loaded)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create decision evidence directory `{}`",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| {
                format!("failed to open decision evidence file `{}`", path.display())
            })?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        let line = encode_order_intent_line(intent)?;
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow!("decision evidence writer lock is poisoned"))?;
        file.write_all(&line)
            .context("failed to write decision evidence record")?;
        file.flush().context("failed to flush decision evidence")?;
        Ok(())
    }
}

pub fn decision_evidence_path(loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
    let relative = Path::new(
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path
            .trim(),
    );
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow!(
            "persistence.decision_evidence.order_intents_relative_path must be non-empty, relative, and stay under catalog_directory"
        ));
    }
    Ok(Path::new(&loaded.root.persistence.catalog_directory).join(relative))
}

fn encode_order_intent_line(intent: &BoltV3OrderIntentEvidence) -> Result<Vec<u8>> {
    let mut line = serde_json::to_vec(intent).context("failed to serialize decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_order_intent_line_serializes_before_appending_newline() {
        let intent = BoltV3OrderIntentEvidence {
            strategy_id: "strategy-one".to_string(),
            intent_kind: BoltV3OrderIntentKind::Entry,
            instrument_id: "instrument-one".to_string(),
            client_order_id: "client-order-one".to_string(),
            order_side: "Buy".to_string(),
            price: "0.42".to_string(),
            quantity: "1".to_string(),
        };

        let line = encode_order_intent_line(&intent).expect("intent should encode");

        assert!(line.ends_with(b"\n"));
        let json = std::str::from_utf8(&line[..line.len() - 1]).expect("line is utf8");
        let decoded: serde_json::Value = serde_json::from_str(json).expect("line is json");
        assert_eq!(decoded["strategy_id"], "strategy-one");
        assert_eq!(decoded["intent_kind"], "entry");
        assert_eq!(decoded["order_side"], "Buy");
    }
}
