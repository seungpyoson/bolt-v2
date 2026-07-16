use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

use crate::source_universe_batch_execution::SourceUniverseBatchBootstrapLimits;

pub const SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION: &str =
    "source-universe-batch-launch-spec.v2";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchLaunchArtifactSpec {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchLaunchSpec {
    pub schema_version: String,
    pub batch_id: String,
    pub execution_pack: SourceUniverseBatchLaunchArtifactSpec,
    pub output_dir: PathBuf,
    pub start_sequence: Option<u64>,
    pub record_limit: Option<u64>,
    pub continue_on_error: bool,
    pub fetch_timeout_seconds: u64,
    pub worker_termination_grace_seconds: u64,
    pub max_concurrent_records: u64,
    pub transport: SourceUniverseBatchTransportSpec,
    pub object_cache_dir: Option<PathBuf>,
    pub allow_partial: bool,
    pub bootstrap_limits: SourceUniverseBatchBootstrapLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceUniverseBatchTransportSpec {
    StagedS3,
    Https { http_user_agent: String },
}

impl SourceUniverseBatchLaunchSpec {
    pub fn from_toml_file(path: &Path) -> Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("read batch launch spec {}", path.display()))?;
        let spec: Self = toml::from_slice(&bytes)
            .with_context(|| format!("parse batch launch spec {}", path.display()))?;
        ensure!(
            spec.schema_version == SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
            "batch launch spec schema_version mismatch: expected {}, got {}",
            SOURCE_UNIVERSE_BATCH_LAUNCH_SPEC_SCHEMA_VERSION,
            spec.schema_version
        );
        ensure!(
            spec.fetch_timeout_seconds > 0,
            "batch launch spec fetch_timeout_seconds must be positive"
        );
        ensure!(
            spec.worker_termination_grace_seconds > 0,
            "batch launch spec worker_termination_grace_seconds must be positive"
        );
        spec.transport.validate()?;
        spec.bootstrap_limits.validate()?;
        Ok(spec)
    }
}

impl SourceUniverseBatchTransportSpec {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::StagedS3 => Ok(()),
            Self::Https { http_user_agent } => validate_http_user_agent(http_user_agent),
        }
    }
}

fn validate_http_user_agent(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty(),
        "batch launch spec http_user_agent must not be empty"
    );
    reqwest::header::HeaderValue::from_bytes(value.as_bytes())
        .context("batch launch spec http_user_agent must be a valid HTTP HeaderValue")?;
    Ok(())
}
