use std::path::PathBuf;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_canary_gate::resolve_report_path,
    bolt_v3_live_node::{
        BoltV3BuiltLiveNode, BoltV3LiveNodeError, build_bolt_v3_live_node, connect_bolt_v3_clients,
        disconnect_bolt_v3_clients,
    },
    bolt_v3_no_submit_readiness_schema::{
        DETAIL_KEY, FAILED_STATUS, SATISFIED_STATUS, SKIPPED_STATUS, STAGE_ADAPTER_MAPPING,
        STAGE_CONTROLLED_CONNECT, STAGE_CONTROLLED_DISCONNECT, STAGE_FORBIDDEN_CREDENTIAL_ENV,
        STAGE_KEY, STAGE_LIVE_NODE_BUILD, STAGE_SECRET_RESOLUTION, STAGES_KEY, STATUS_KEY,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoltV3NoSubmitReadinessStatus {
    Satisfied,
    Failed,
    Skipped,
}

impl BoltV3NoSubmitReadinessStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Satisfied => SATISFIED_STATUS,
            Self::Failed => FAILED_STATUS,
            Self::Skipped => SKIPPED_STATUS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3NoSubmitReadinessStage {
    pub stage: &'static str,
    pub status: BoltV3NoSubmitReadinessStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3NoSubmitReadinessReport {
    pub stages: Vec<BoltV3NoSubmitReadinessStage>,
}

#[derive(Debug)]
pub enum BoltV3NoSubmitReadinessError {
    MissingOperatorApprovalId,
    MissingLiveCanaryConfig,
    MissingReadinessReportPath,
    OperatorApprovalIdMismatch,
    LiveNode(BoltV3LiveNodeError),
    ReportWrite {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for BoltV3NoSubmitReadinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOperatorApprovalId => {
                write!(
                    f,
                    "bolt-v3 no-submit readiness operator approval id is empty"
                )
            }
            Self::MissingLiveCanaryConfig => {
                write!(
                    f,
                    "bolt-v3 no-submit readiness requires [live_canary] config"
                )
            }
            Self::MissingReadinessReportPath => write!(
                f,
                "bolt-v3 no-submit readiness live_canary report path is empty"
            ),
            Self::OperatorApprovalIdMismatch => write!(
                f,
                "bolt-v3 no-submit readiness operator approval id does not match [live_canary].approval_id"
            ),
            Self::LiveNode(error) => write!(f, "{error}"),
            Self::ReportWrite { path, source } => write!(
                f,
                "bolt-v3 no-submit readiness failed to write report {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for BoltV3NoSubmitReadinessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LiveNode(error) => Some(error),
            Self::ReportWrite { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl BoltV3NoSubmitReadinessReport {
    pub fn stage_status(&self, stage: &str) -> Vec<BoltV3NoSubmitReadinessStatus> {
        self.stages
            .iter()
            .filter(|fact| fact.stage == stage)
            .map(|fact| fact.status)
            .collect()
    }

    pub fn write_redacted_json(&self, path: &std::path::Path) -> std::io::Result<()> {
        let stages: Vec<serde_json::Value> = self
            .stages
            .iter()
            .map(|stage| {
                serde_json::json!({
                    STAGE_KEY: stage.stage,
                    STATUS_KEY: stage.status.as_str(),
                    DETAIL_KEY: stage.detail,
                })
            })
            .collect();
        let payload = serde_json::json!({
            STAGES_KEY: stages,
        });
        let body = serde_json::to_vec_pretty(&payload)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, body)
    }

    pub fn write_redacted_json_for_loaded_config(
        &self,
        loaded: &LoadedBoltV3Config,
    ) -> Result<PathBuf, BoltV3NoSubmitReadinessError> {
        let block = loaded
            .root
            .live_canary
            .as_ref()
            .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?;
        if block.no_submit_readiness_report_path.trim().is_empty() {
            return Err(BoltV3NoSubmitReadinessError::MissingReadinessReportPath);
        }
        let report_path = resolve_report_path(&loaded.root_path, block);
        self.write_redacted_json(&report_path).map_err(|source| {
            BoltV3NoSubmitReadinessError::ReportWrite {
                path: report_path.clone(),
                source,
            }
        })?;
        Ok(report_path)
    }
}

pub fn run_bolt_v3_no_submit_readiness(
    loaded: &LoadedBoltV3Config,
    operator_approval_id: &str,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3NoSubmitReadinessError> {
    let operator_approval_id = operator_approval_id.trim();
    if operator_approval_id.is_empty() {
        return Err(BoltV3NoSubmitReadinessError::MissingOperatorApprovalId);
    }
    let block = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?;
    if operator_approval_id != block.approval_id.trim() {
        return Err(BoltV3NoSubmitReadinessError::OperatorApprovalIdMismatch);
    }

    let mut built =
        build_bolt_v3_live_node(loaded).map_err(BoltV3NoSubmitReadinessError::LiveNode)?;
    run_bolt_v3_no_submit_readiness_on_built_node(&mut built, loaded)
        .map_err(BoltV3NoSubmitReadinessError::LiveNode)
}

pub fn run_bolt_v3_no_submit_readiness_on_built_node(
    built: &mut BoltV3BuiltLiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3LiveNodeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| BoltV3LiveNodeError::Build(anyhow::Error::new(error)))?;
    runtime.block_on(run_bolt_v3_no_submit_readiness_async_on_built_node(
        built, loaded,
    ))
}

async fn run_bolt_v3_no_submit_readiness_async_on_built_node(
    built: &mut BoltV3BuiltLiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3LiveNodeError> {
    let mut stages = vec![
        satisfied(STAGE_FORBIDDEN_CREDENTIAL_ENV),
        satisfied(STAGE_SECRET_RESOLUTION),
        satisfied(STAGE_ADAPTER_MAPPING),
        satisfied(STAGE_LIVE_NODE_BUILD),
    ];

    match connect_bolt_v3_clients(built.node_mut(), loaded).await {
        Ok(()) => stages.push(satisfied(STAGE_CONTROLLED_CONNECT)),
        Err(_) => {
            stages.push(failed(
                STAGE_CONTROLLED_CONNECT,
                "controlled_connect failed; inspect NT operator logs",
            ));
            record_controlled_disconnect_stage(&mut stages, built, loaded).await;
            return Ok(BoltV3NoSubmitReadinessReport { stages });
        }
    }

    record_controlled_disconnect_stage(&mut stages, built, loaded).await;

    Ok(BoltV3NoSubmitReadinessReport { stages })
}

async fn record_controlled_disconnect_stage(
    stages: &mut Vec<BoltV3NoSubmitReadinessStage>,
    built: &mut BoltV3BuiltLiveNode,
    loaded: &LoadedBoltV3Config,
) {
    match disconnect_bolt_v3_clients(built.node_mut(), loaded).await {
        Ok(()) => stages.push(satisfied(STAGE_CONTROLLED_DISCONNECT)),
        Err(_) => stages.push(failed(
            STAGE_CONTROLLED_DISCONNECT,
            "controlled_disconnect failed; inspect NT operator logs",
        )),
    }
}

fn satisfied(stage: &'static str) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Satisfied,
        detail: SATISFIED_STATUS.to_string(),
    }
}

fn failed(stage: &'static str, detail: &'static str) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Failed,
        detail: detail.to_string(),
    }
}
