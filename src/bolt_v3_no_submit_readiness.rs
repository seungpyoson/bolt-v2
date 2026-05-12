use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_node::{
        BoltV3BuiltLiveNode, BoltV3LiveNodeError, connect_bolt_v3_clients,
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
        std::fs::write(path, body)
    }
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
        Err(error) => {
            stages.push(failed(STAGE_CONTROLLED_CONNECT, error.to_string()));
            stages.push(skipped(
                STAGE_CONTROLLED_DISCONNECT,
                "controlled_disconnect skipped after controlled_connect failure",
            ));
            return Ok(BoltV3NoSubmitReadinessReport { stages });
        }
    }

    match disconnect_bolt_v3_clients(built.node_mut(), loaded).await {
        Ok(()) => stages.push(satisfied(STAGE_CONTROLLED_DISCONNECT)),
        Err(error) => stages.push(failed(STAGE_CONTROLLED_DISCONNECT, error.to_string())),
    }

    Ok(BoltV3NoSubmitReadinessReport { stages })
}

fn satisfied(stage: &'static str) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Satisfied,
        detail: SATISFIED_STATUS.to_string(),
    }
}

fn failed(stage: &'static str, detail: String) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Failed,
        detail,
    }
}

fn skipped(stage: &'static str, detail: &'static str) -> BoltV3NoSubmitReadinessStage {
    BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Skipped,
        detail: detail.to_string(),
    }
}
