use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
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
    writer: Mutex<BufWriter<std::fs::File>>,
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
            writer: Mutex::new(BufWriter::new(file)),
        })
    }
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("decision evidence writer lock is poisoned"))?;
        serde_json::to_writer(&mut *writer, intent)
            .context("failed to serialize decision evidence")?;
        writer
            .write_all(b"\n")
            .context("failed to terminate decision evidence record")?;
        writer
            .flush()
            .context("failed to flush decision evidence")?;
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
