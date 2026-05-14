//! Bolt-v3 Phase 7 no-submit readiness report producer.
//!
//! This module owns report modeling, redaction, and sequencing. NT still
//! owns adapter behavior, connection dispatch, cache, lifecycle, order state,
//! reconciliation, and venue wire behavior.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{
        BoltV3LiveNodeError, BoltV3LiveNodeRuntime, build_bolt_v3_live_node,
        controlled_no_submit_readiness,
    },
    bolt_v3_no_submit_readiness_schema::{
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, LIVE_NODE_BUILD_STAGE,
        NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE, REDACTED_DETAIL_MARKER,
        REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SECRET_RESOLUTION_STAGE,
    },
};

#[derive(Debug)]
pub enum BoltV3NoSubmitReadinessError {
    MissingLiveCanaryConfig,
    MissingOperatorApprovalId,
    MissingHeadSha,
    OperatorApprovalIdMismatch,
    LiveNode {
        source: BoltV3LiveNodeError,
    },
    ReportTooLarge {
        path: PathBuf,
        length: usize,
        max_length: usize,
    },
    ReportParentCreate {
        path: PathBuf,
        source: std::io::Error,
    },
    ReportWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    ReportSerialize {
        source: serde_json::Error,
    },
    RootConfigChecksumRead {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for BoltV3NoSubmitReadinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingLiveCanaryConfig => {
                write!(f, "bolt-v3 no-submit readiness requires `[live_canary]`")
            }
            Self::MissingOperatorApprovalId => {
                write!(
                    f,
                    "bolt-v3 no-submit readiness operator approval id is empty"
                )
            }
            Self::MissingHeadSha => write!(f, "bolt-v3 no-submit readiness head SHA is empty"),
            Self::OperatorApprovalIdMismatch => write!(
                f,
                "bolt-v3 no-submit readiness operator approval id does not match `[live_canary]`"
            ),
            Self::LiveNode { source } => write!(
                f,
                "bolt-v3 no-submit readiness live-node operation failed: {source}"
            ),
            Self::ReportTooLarge {
                path,
                length,
                max_length,
            } => write!(
                f,
                "bolt-v3 no-submit readiness report {} is {length} bytes, exceeding configured limit {max_length}",
                path.display()
            ),
            Self::ReportParentCreate { path, source } => write!(
                f,
                "failed to create bolt-v3 no-submit readiness report parent for {}: {source}",
                path.display()
            ),
            Self::ReportWrite { path, source } => write!(
                f,
                "failed to write bolt-v3 no-submit readiness report {}: {source}",
                path.display()
            ),
            Self::ReportSerialize { source } => {
                write!(
                    f,
                    "failed to serialize bolt-v3 no-submit readiness report: {source}"
                )
            }
            Self::RootConfigChecksumRead { path, source } => write!(
                f,
                "failed to read bolt-v3 root TOML {} for no-submit checksum: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for BoltV3NoSubmitReadinessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LiveNode { source } => Some(source),
            Self::ReportParentCreate { source, .. } | Self::ReportWrite { source, .. } => {
                Some(source)
            }
            Self::ReportSerialize { source } => Some(source),
            Self::RootConfigChecksumRead { source, .. } => Some(source),
            Self::MissingLiveCanaryConfig
            | Self::MissingOperatorApprovalId
            | Self::MissingHeadSha
            | Self::OperatorApprovalIdMismatch
            | Self::ReportTooLarge { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3NoSubmitReadinessStatus {
    Satisfied,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3NoSubmitReadinessStage {
    pub stage: &'static str,
    pub status: BoltV3NoSubmitReadinessStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3NoSubmitReadinessReportMetadata {
    pub approval_id_hash: String,
    pub head_sha: String,
    pub config_checksum: String,
    pub report_path: String,
}

impl BoltV3NoSubmitReadinessReportMetadata {
    pub async fn from_loaded(
        loaded: &LoadedBoltV3Config,
        operator_approval_id: &str,
        head_sha: &str,
    ) -> Result<Self, BoltV3NoSubmitReadinessError> {
        let trimmed_head = head_sha.trim();
        if trimmed_head.is_empty() {
            return Err(BoltV3NoSubmitReadinessError::MissingHeadSha);
        }
        let approval_id_hash = validate_operator_approval(loaded, operator_approval_id)?;
        let block = loaded
            .root
            .live_canary
            .as_ref()
            .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?;
        Ok(Self {
            approval_id_hash,
            head_sha: trimmed_head.to_string(),
            config_checksum: root_config_checksum(loaded).await?,
            report_path: configured_report_path(loaded, &block.no_submit_readiness_report_path)
                .to_string_lossy()
                .to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3NoSubmitReadinessReport {
    pub schema_version: &'static str,
    pub approval_id_hash: String,
    pub head_sha: String,
    pub config_checksum: String,
    pub report_path: String,
    pub stages: Vec<BoltV3NoSubmitReadinessStage>,
}

impl BoltV3NoSubmitReadinessReport {
    pub fn stage_status(&self, stage: &str) -> Vec<BoltV3NoSubmitReadinessStatus> {
        self.stages
            .iter()
            .filter(|item| item.stage == stage)
            .map(|item| item.status)
            .collect()
    }

    pub fn write_redacted_json_with_max_bytes(
        &self,
        path: &Path,
        max_length: usize,
    ) -> Result<(), BoltV3NoSubmitReadinessError> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|source| BoltV3NoSubmitReadinessError::ReportSerialize { source })?;
        if bytes.len() > max_length {
            return Err(BoltV3NoSubmitReadinessError::ReportTooLarge {
                path: path.to_path_buf(),
                length: bytes.len(),
                max_length,
            });
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                BoltV3NoSubmitReadinessError::ReportParentCreate {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        }
        std::fs::write(path, bytes).map_err(|source| BoltV3NoSubmitReadinessError::ReportWrite {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn write_configured_redacted_json(
        &self,
        loaded: &LoadedBoltV3Config,
    ) -> Result<(), BoltV3NoSubmitReadinessError> {
        let block = loaded
            .root
            .live_canary
            .as_ref()
            .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?;
        self.write_redacted_json_with_max_bytes(
            &configured_report_path(loaded, &block.no_submit_readiness_report_path),
            block.max_no_submit_readiness_report_bytes as usize,
        )
    }
}

pub fn run_bolt_v3_no_submit_readiness_from_stage_results(
    metadata: BoltV3NoSubmitReadinessReportMetadata,
    controlled_connect: Result<(), String>,
    reference_readiness: Result<(), String>,
    controlled_disconnect: Result<(), String>,
    redacted_values: &[String],
) -> BoltV3NoSubmitReadinessReport {
    let mut stages = Vec::new();
    push_satisfied_stage(&mut stages, OPERATOR_APPROVAL_STAGE);
    push_satisfied_stage(&mut stages, SECRET_RESOLUTION_STAGE);
    push_satisfied_stage(&mut stages, LIVE_NODE_BUILD_STAGE);
    let connected = push_result_stage(
        &mut stages,
        CONTROLLED_CONNECT_STAGE,
        controlled_connect,
        redacted_values,
    );
    if connected {
        push_result_stage(
            &mut stages,
            REFERENCE_READINESS_STAGE,
            reference_readiness,
            redacted_values,
        );
    } else {
        stages.push(BoltV3NoSubmitReadinessStage {
            stage: REFERENCE_READINESS_STAGE,
            status: BoltV3NoSubmitReadinessStatus::Skipped,
            detail: Some("controlled connect failed".to_string()),
        });
    }
    push_result_stage(
        &mut stages,
        CONTROLLED_DISCONNECT_STAGE,
        controlled_disconnect,
        redacted_values,
    );
    push_satisfied_stage(&mut stages, REPORT_WRITE_STAGE);
    BoltV3NoSubmitReadinessReport {
        schema_version: NO_SUBMIT_READINESS_SCHEMA_VERSION,
        approval_id_hash: metadata.approval_id_hash,
        head_sha: metadata.head_sha,
        config_checksum: metadata.config_checksum,
        report_path: metadata.report_path,
        stages,
    }
}

pub fn reference_readiness_from_cached_instrument_ids<I, S>(
    loaded: &LoadedBoltV3Config,
    cached_instrument_ids: I,
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let cached = cached_instrument_ids
        .into_iter()
        .map(|instrument_id| instrument_id.as_ref().trim().to_string())
        .filter(|instrument_id| !instrument_id.is_empty())
        .collect::<BTreeSet<_>>();
    let missing = loaded
        .strategies
        .iter()
        .flat_map(|strategy| {
            strategy
                .config
                .reference_data
                .iter()
                .filter_map(|(role, reference)| {
                    let instrument_id = reference.instrument_id.trim();
                    (!cached.contains(instrument_id)).then(|| {
                        format!(
                            "{} reference_data.{role} instrument_id `{instrument_id}`",
                            strategy.relative_path
                        )
                    })
                })
        })
        .collect::<Vec<_>>();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "missing required reference instruments in NT cache: {}",
            missing.join(", ")
        ))
    }
}

pub async fn run_bolt_v3_no_submit_readiness_on_runtime(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    metadata: BoltV3NoSubmitReadinessReportMetadata,
    redacted_values: &[String],
) -> BoltV3NoSubmitReadinessReport {
    let (connect, reference, disconnect) =
        controlled_no_submit_readiness(runtime, loaded, |runtime| {
            reference_readiness_from_cached_instrument_ids(loaded, runtime.cached_instrument_ids())
        })
        .await;
    run_bolt_v3_no_submit_readiness_from_stage_results(
        metadata,
        connect.map_err(|error| error.to_string()),
        reference,
        disconnect.map_err(|error| error.to_string()),
        redacted_values,
    )
}

pub async fn run_bolt_v3_no_submit_readiness(
    loaded: &LoadedBoltV3Config,
    operator_approval_id: &str,
    head_sha: &str,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3NoSubmitReadinessError> {
    let metadata =
        BoltV3NoSubmitReadinessReportMetadata::from_loaded(loaded, operator_approval_id, head_sha)
            .await?;
    let mut runtime = build_bolt_v3_live_node(loaded)
        .map_err(|source| BoltV3NoSubmitReadinessError::LiveNode { source })?;
    let redacted_values = runtime.redaction_values().to_vec();
    Ok(
        run_bolt_v3_no_submit_readiness_on_runtime(
            &mut runtime,
            loaded,
            metadata,
            &redacted_values,
        )
        .await,
    )
}

fn validate_operator_approval(
    loaded: &LoadedBoltV3Config,
    operator_approval_id: &str,
) -> Result<String, BoltV3NoSubmitReadinessError> {
    let supplied = operator_approval_id.trim();
    if supplied.is_empty() {
        return Err(BoltV3NoSubmitReadinessError::MissingOperatorApprovalId);
    }
    let configured = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?
        .approval_id
        .trim();
    let supplied_hash = sha256_hex(supplied.as_bytes());
    let configured_hash = sha256_hex(configured.as_bytes());
    if supplied_hash != configured_hash {
        return Err(BoltV3NoSubmitReadinessError::OperatorApprovalIdMismatch);
    }
    Ok(supplied_hash)
}

fn configured_report_path(loaded: &LoadedBoltV3Config, configured: &str) -> PathBuf {
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        return path;
    }
    loaded
        .root_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path)
}

async fn root_config_checksum(
    loaded: &LoadedBoltV3Config,
) -> Result<String, BoltV3NoSubmitReadinessError> {
    let bytes = tokio::fs::read(&loaded.root_path).await.map_err(|source| {
        BoltV3NoSubmitReadinessError::RootConfigChecksumRead {
            path: loaded.root_path.clone(),
            source,
        }
    })?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn push_satisfied_stage(stages: &mut Vec<BoltV3NoSubmitReadinessStage>, stage: &'static str) {
    stages.push(BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Satisfied,
        detail: None,
    });
}

fn push_result_stage(
    stages: &mut Vec<BoltV3NoSubmitReadinessStage>,
    stage: &'static str,
    result: Result<(), String>,
    redacted_values: &[String],
) -> bool {
    match result {
        Ok(()) => {
            stages.push(BoltV3NoSubmitReadinessStage {
                stage,
                status: BoltV3NoSubmitReadinessStatus::Satisfied,
                detail: None,
            });
            true
        }
        Err(detail) => {
            stages.push(BoltV3NoSubmitReadinessStage {
                stage,
                status: BoltV3NoSubmitReadinessStatus::Failed,
                detail: Some(redact_detail(&detail, redacted_values)),
            });
            false
        }
    }
}

fn redact_detail(detail: &str, redacted_values: &[String]) -> String {
    let mut values = redacted_values
        .iter()
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    values.dedup();
    if values.is_empty() {
        return detail.to_string();
    }

    let mut occupied = vec![false; detail.len()];
    let mut ranges = Vec::new();
    for value in values {
        for (start, _) in detail.match_indices(value) {
            let end = start + value.len();
            if occupied[start..end].iter().any(|taken| *taken) {
                continue;
            }
            occupied[start..end].fill(true);
            ranges.push((start, end));
        }
    }
    if ranges.is_empty() {
        return detail.to_string();
    }

    ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut redacted = String::with_capacity(detail.len());
    let mut cursor = usize::default();
    for (start, end) in ranges {
        redacted.push_str(&detail[cursor..start]);
        redacted.push_str(REDACTED_DETAIL_MARKER);
        cursor = end;
    }
    redacted.push_str(&detail[cursor..]);
    redacted
}
