use std::{
    env, fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};
use rust_decimal::Decimal;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_live_canary_gate::{BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_gate},
};

const PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const SUBMIT_ADMISSION_STATUS_ACCEPTED: &str = "accepted";
const SUBMIT_ADMISSION_STATUS_REJECTED: &str = "rejected";
const NT_ADAPTER_SUBMIT_PROVEN_REASON: &str = "nt_adapter_submit_proven";
const BLOCKED_BEFORE_LIVE_ORDER_REASON: &str = "blocked_before_live_order";
const BLOCKED_BEFORE_SUBMIT_REASON: &str = "blocked_before_submit";
const PHASE8_REQUIRED_LIVE_ORDER_CAP: u32 = 1;
const PHASE8_SHA256_BUFFER_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8CanaryPreflightStatus {
    Missing,
    AcceptedByGate,
    RejectedByGate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8StrategyInputAuditStatus {
    Approved,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8CanaryBlockReason {
    MissingNoSubmitReadinessReport,
    LiveCanaryGateRejected,
    StrategyInputSafetyAuditBlocked,
    LiveOrderCountCapNotOne,
    LiveProofCaptureUnavailable,
    NonPositiveRealizedVolatility,
    NonPositiveTimeToExpiry,
    DecisionEvidenceUnavailable,
    BlockedBeforeLiveOrder,
    RootConfigHashUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8StrategyInputSafetyAudit {
    status: Phase8StrategyInputAuditStatus,
    block_reasons: Vec<Phase8CanaryBlockReason>,
}

impl Phase8StrategyInputSafetyAudit {
    pub fn approved() -> Self {
        Self {
            status: Phase8StrategyInputAuditStatus::Approved,
            block_reasons: Vec::new(),
        }
    }

    pub fn blocked(block_reasons: Vec<Phase8CanaryBlockReason>) -> Self {
        Self {
            status: Phase8StrategyInputAuditStatus::Blocked,
            block_reasons,
        }
    }

    pub fn from_strategy_inputs(realized_volatility: Decimal, seconds_to_expiry: u64) -> Self {
        let mut block_reasons = Vec::new();
        if realized_volatility <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveRealizedVolatility);
        }
        if seconds_to_expiry == 0 {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveTimeToExpiry);
        }
        if block_reasons.is_empty() {
            Self::approved()
        } else {
            Self::blocked(block_reasons)
        }
    }

    pub fn is_approved(&self) -> bool {
        self.status == Phase8StrategyInputAuditStatus::Approved
    }

    pub fn block_reasons(&self) -> &[Phase8CanaryBlockReason] {
        &self.block_reasons
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8CanaryPreflight {
    pub head_sha: String,
    pub root_config_sha256: String,
    pub no_submit_report_status: Phase8CanaryPreflightStatus,
    pub strategy_input_audit_status: Phase8StrategyInputAuditStatus,
    pub max_live_order_count: Option<u32>,
    pub max_notional_per_order: Option<String>,
    pub block_reasons: Vec<Phase8CanaryBlockReason>,
}

impl Phase8CanaryPreflight {
    pub fn can_enter_live_runner(&self) -> bool {
        self.block_reasons.is_empty()
            && self.no_submit_report_status == Phase8CanaryPreflightStatus::AcceptedByGate
            && self.strategy_input_audit_status == Phase8StrategyInputAuditStatus::Approved
            && self.max_live_order_count == Some(PHASE8_REQUIRED_LIVE_ORDER_CAP)
    }
}

pub async fn evaluate_phase8_canary_preflight(
    loaded: &LoadedBoltV3Config,
    head_sha: &str,
    strategy_audit: Phase8StrategyInputSafetyAudit,
) -> Phase8CanaryPreflight {
    let live_canary = loaded.root.live_canary.as_ref();
    let mut block_reasons = strategy_audit.block_reasons.clone();
    let root_config_sha256 = match Phase8OperatorApprovalEnvelope::sha256_file(&loaded.root_path) {
        Ok(hash) => hash,
        Err(_) => {
            block_reasons.push(Phase8CanaryBlockReason::RootConfigHashUnavailable);
            String::new()
        }
    };

    let no_submit_report_status = match check_bolt_v3_live_canary_gate(loaded).await {
        Ok(_) => Phase8CanaryPreflightStatus::AcceptedByGate,
        Err(BoltV3LiveCanaryGateError::ReadinessReportRead { .. }) => {
            block_reasons.push(Phase8CanaryBlockReason::MissingNoSubmitReadinessReport);
            Phase8CanaryPreflightStatus::Missing
        }
        Err(_) => {
            block_reasons.push(Phase8CanaryBlockReason::LiveCanaryGateRejected);
            Phase8CanaryPreflightStatus::RejectedByGate
        }
    };
    if live_canary.is_some_and(|block| block.max_live_order_count != PHASE8_REQUIRED_LIVE_ORDER_CAP)
    {
        block_reasons.push(Phase8CanaryBlockReason::LiveOrderCountCapNotOne);
    }

    Phase8CanaryPreflight {
        head_sha: head_sha.trim().to_string(),
        root_config_sha256,
        no_submit_report_status,
        strategy_input_audit_status: strategy_audit.status,
        max_live_order_count: live_canary.map(|block| block.max_live_order_count),
        max_notional_per_order: live_canary.map(|block| block.max_notional_per_order.clone()),
        block_reasons,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8EvidenceRef {
    pub path_hash: String,
    pub record_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8SubmitAdmissionRef {
    pub status: String,
    pub admitted_order_count: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8RuntimeCaptureRef {
    pub spool_root_hash: String,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8NtLifecycleRef {
    pub kind: String,
    pub event_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8CanaryOutcome {
    DryNoSubmitProof,
    BlockedBeforeSubmit,
    LiveCanaryProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8LiveOrderRef {
    pub client_order_id_hash: String,
    pub venue_order_id_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8LiveCanaryResultRefs {
    pub nt_submit_event_ref: Phase8EvidenceRef,
    pub venue_order_state_ref: Phase8EvidenceRef,
    pub strategy_cancel_ref: Option<Phase8EvidenceRef>,
    pub restart_reconciliation_ref: Phase8EvidenceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8CanaryEvidence {
    pub schema_version: u32,
    pub head_sha: String,
    pub root_config_sha256: String,
    pub ssm_manifest_sha256: String,
    pub approval_id_hash: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
    pub decision_evidence_ref: Option<Phase8EvidenceRef>,
    pub submit_admission_ref: Phase8SubmitAdmissionRef,
    pub live_order_ref: Option<Phase8LiveOrderRef>,
    pub nt_submit_event_ref: Option<Phase8EvidenceRef>,
    pub venue_order_state_ref: Option<Phase8EvidenceRef>,
    pub strategy_cancel_ref: Option<Phase8EvidenceRef>,
    pub restart_reconciliation_ref: Option<Phase8EvidenceRef>,
    pub runtime_capture_ref: Phase8RuntimeCaptureRef,
    pub nt_lifecycle_refs: Vec<Phase8NtLifecycleRef>,
    pub outcome: Phase8CanaryOutcome,
    pub block_reasons: Vec<Phase8CanaryBlockReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8CanaryEvidenceInput {
    pub head_sha: String,
    pub root_config_sha256: String,
    pub ssm_manifest_sha256: String,
    pub approval_id: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: Decimal,
    pub runtime_capture_ref: Phase8RuntimeCaptureRef,
}

impl Phase8CanaryEvidence {
    pub fn dry_no_submit_proof(
        input: Phase8CanaryEvidenceInput,
        decision_evidence_ref: Phase8EvidenceRef,
    ) -> Self {
        Self {
            schema_version: PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: Some(decision_evidence_ref),
            submit_admission_ref: Phase8SubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_REJECTED.to_string(),
                admitted_order_count: 0,
                reason: BLOCKED_BEFORE_LIVE_ORDER_REASON.to_string(),
            },
            live_order_ref: None,
            nt_submit_event_ref: None,
            venue_order_state_ref: None,
            strategy_cancel_ref: None,
            restart_reconciliation_ref: None,
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: Phase8CanaryOutcome::DryNoSubmitProof,
            block_reasons: vec![Phase8CanaryBlockReason::BlockedBeforeLiveOrder],
        }
    }

    pub fn blocked_before_submit(
        input: Phase8CanaryEvidenceInput,
        block_reason: Phase8CanaryBlockReason,
    ) -> Self {
        Self {
            schema_version: PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: None,
            submit_admission_ref: Phase8SubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_REJECTED.to_string(),
                admitted_order_count: 0,
                reason: BLOCKED_BEFORE_SUBMIT_REASON.to_string(),
            },
            live_order_ref: None,
            nt_submit_event_ref: None,
            venue_order_state_ref: None,
            strategy_cancel_ref: None,
            restart_reconciliation_ref: None,
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: Phase8CanaryOutcome::BlockedBeforeSubmit,
            block_reasons: vec![block_reason],
        }
    }

    pub fn live_canary_proof(
        input: Phase8CanaryEvidenceInput,
        decision_evidence_ref: Phase8EvidenceRef,
        live_order_ref: Phase8LiveOrderRef,
        result_refs: Phase8LiveCanaryResultRefs,
    ) -> Self {
        Self {
            schema_version: PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: Some(decision_evidence_ref),
            submit_admission_ref: Phase8SubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_ACCEPTED.to_string(),
                admitted_order_count: 1,
                reason: NT_ADAPTER_SUBMIT_PROVEN_REASON.to_string(),
            },
            live_order_ref: Some(live_order_ref),
            nt_submit_event_ref: Some(result_refs.nt_submit_event_ref),
            venue_order_state_ref: Some(result_refs.venue_order_state_ref),
            strategy_cancel_ref: result_refs.strategy_cancel_ref,
            restart_reconciliation_ref: Some(result_refs.restart_reconciliation_ref),
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: Phase8CanaryOutcome::LiveCanaryProof,
            block_reasons: Vec::new(),
        }
    }

    pub fn write_json_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| {
                anyhow!(
                    "failed to create phase8 canary evidence directory `{}`: {source}",
                    parent.display()
                )
            })?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|source| anyhow!("failed to serialize phase8 canary evidence: {source}"))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::AlreadyExists => anyhow!(
                    "phase8 canary evidence `{}` already exists; refusing to overwrite",
                    path.display()
                ),
                _ => anyhow!(
                    "failed to create phase8 canary evidence `{}`: {source}",
                    path.display()
                ),
            })?;
        if let Err(source) = file.write_all(&bytes) {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to write phase8 canary evidence `{}`: {source}",
                path.display()
            ));
        }
        if let Err(source) = file.sync_all() {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to sync phase8 canary evidence `{}`: {source}",
                path.display()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8OperatorApprovalEnvelope {
    pub head_sha: String,
    pub root_toml_path: String,
    pub root_toml_sha256: String,
    pub ssm_manifest_sha256: String,
    pub operator_approval_id: String,
    pub canary_evidence_path: String,
}

impl Phase8OperatorApprovalEnvelope {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            head_sha: required_env("BOLT_V3_PHASE8_HEAD_SHA")?,
            root_toml_path: required_env("BOLT_V3_PHASE8_ROOT_TOML_PATH")?,
            root_toml_sha256: required_env("BOLT_V3_PHASE8_ROOT_TOML_SHA256")?,
            ssm_manifest_sha256: required_env("BOLT_V3_PHASE8_SSM_MANIFEST_SHA256")?,
            operator_approval_id: required_env("BOLT_V3_PHASE8_OPERATOR_APPROVAL_ID")?,
            canary_evidence_path: required_env("BOLT_V3_PHASE8_EVIDENCE_PATH")?,
        })
    }

    pub fn validate_against(
        &self,
        current_head_sha: &str,
        current_root_toml_sha256: &str,
        live_canary_approval_id: &str,
    ) -> Result<()> {
        if self.head_sha != current_head_sha {
            return Err(anyhow!(
                "phase8 operator approval head_sha does not match current head"
            ));
        }
        if self.root_toml_sha256 != current_root_toml_sha256 {
            return Err(anyhow!(
                "phase8 operator approval root_toml_sha256 does not match current root TOML"
            ));
        }
        if self.operator_approval_id != live_canary_approval_id {
            return Err(anyhow!(
                "phase8 operator approval id does not match `[live_canary]`"
            ));
        }
        Ok(())
    }

    pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        let file = fs::File::open(path).map_err(|source| {
            anyhow!(
                "failed to open phase8 sha256 input `{}`: {source}",
                path.display()
            )
        })?;
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut buffer = [0; PHASE8_SHA256_BUFFER_BYTES];
        loop {
            let length = reader.read(&mut buffer).map_err(|source| {
                anyhow!(
                    "failed to read phase8 sha256 input `{}`: {source}",
                    path.display()
                )
            })?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    pub fn root_path(&self) -> PathBuf {
        PathBuf::from(&self.root_toml_path)
    }
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).map_err(|_| anyhow!("missing required phase8 env `{name}`"))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("required phase8 env `{name}` is empty"));
    }
    Ok(trimmed.to_string())
}

fn sha256_text(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}
