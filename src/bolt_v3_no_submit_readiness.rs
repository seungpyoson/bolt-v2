//! Bolt-v3 Phase 7 no-submit readiness report producer.
//!
//! This module owns report modeling, redaction, and sequencing. NT still
//! owns adapter behavior, connection dispatch, cache, lifecycle, order state,
//! reconciliation, and venue wire behavior.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{
        BoltV3LiveNodeError, BoltV3LiveNodeRuntime, build_bolt_v3_live_node,
        controlled_no_submit_readiness,
    },
};

#[derive(Debug)]
pub enum BoltV3NoSubmitReadinessError {
    MissingLiveCanaryConfig,
    MissingOperatorApprovalId,
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
            Self::MissingLiveCanaryConfig
            | Self::MissingOperatorApprovalId
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
pub struct BoltV3NoSubmitReadinessReport {
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
    controlled_connect: Result<(), String>,
    reference_readiness: Result<(), String>,
    controlled_disconnect: Result<(), String>,
    redacted_values: &[String],
) -> BoltV3NoSubmitReadinessReport {
    let mut stages = Vec::new();
    let connected = push_result_stage(
        &mut stages,
        "controlled_connect",
        controlled_connect,
        redacted_values,
    );
    if connected {
        push_result_stage(
            &mut stages,
            "reference_readiness",
            reference_readiness,
            redacted_values,
        );
    } else {
        stages.push(BoltV3NoSubmitReadinessStage {
            stage: "reference_readiness",
            status: BoltV3NoSubmitReadinessStatus::Skipped,
            detail: Some("controlled connect failed".to_string()),
        });
    }
    push_result_stage(
        &mut stages,
        "controlled_disconnect",
        controlled_disconnect,
        redacted_values,
    );
    BoltV3NoSubmitReadinessReport { stages }
}

pub async fn run_bolt_v3_no_submit_readiness_on_runtime(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    redacted_values: &[String],
) -> BoltV3NoSubmitReadinessReport {
    let (connect, disconnect) = controlled_no_submit_readiness(runtime, loaded).await;
    let reference = if connect.is_ok() {
        current_main_reference_readiness()
    } else {
        Err("controlled connect failed".to_string())
    };
    run_bolt_v3_no_submit_readiness_from_stage_results(
        connect.map_err(|error| error.to_string()),
        reference,
        disconnect.map_err(|error| error.to_string()),
        redacted_values,
    )
}

pub async fn run_bolt_v3_no_submit_readiness(
    loaded: &LoadedBoltV3Config,
    operator_approval_id: &str,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3NoSubmitReadinessError> {
    validate_operator_approval(loaded, operator_approval_id)?;
    let mut runtime = build_bolt_v3_live_node(loaded)
        .map_err(|source| BoltV3NoSubmitReadinessError::LiveNode { source })?;
    Ok(run_bolt_v3_no_submit_readiness_on_runtime(&mut runtime, loaded, &[]).await)
}

fn validate_operator_approval(
    loaded: &LoadedBoltV3Config,
    operator_approval_id: &str,
) -> Result<(), BoltV3NoSubmitReadinessError> {
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
    if supplied != configured {
        return Err(BoltV3NoSubmitReadinessError::OperatorApprovalIdMismatch);
    }
    Ok(())
}

fn current_main_reference_readiness() -> Result<(), String> {
    Err(
        "reference readiness cannot be satisfied through the current no-run bolt-v3 NT boundary"
            .to_string(),
    )
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
    redacted_values
        .iter()
        .filter(|value| !value.is_empty())
        .fold(detail.to_string(), |acc, value| {
            acc.replace(value, "[redacted]")
        })
}
