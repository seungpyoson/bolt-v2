use std::{
    fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, resolve_root_relative_path},
    bolt_v3_live_canary_gate::{BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_gate},
};

const TINY_CANARY_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const SUBMIT_ADMISSION_STATUS_ACCEPTED: &str = "accepted";
const SUBMIT_ADMISSION_STATUS_REJECTED: &str = "rejected";
const NT_ADAPTER_SUBMIT_PROVEN_REASON: &str = "nt_adapter_submit_proven";
const BLOCKED_BEFORE_LIVE_ORDER_REASON: &str = "blocked_before_live_order";
const BLOCKED_BEFORE_SUBMIT_REASON: &str = "blocked_before_submit";
const TINY_CANARY_SHA256_BUFFER_BYTES: usize = 8 * 1024;
const TINY_CANARY_APPROVAL_CONSUMPTION_SCHEMA_VERSION: u32 = 1;
const TINY_CANARY_APPROVAL_CONSUMPTION_RECORD_KIND: &str =
    "tiny_canary_operator_approval_consumption";
pub const TINY_CANARY_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID: &str =
    "tiny-canary-blocked-before-live-runner";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TinyCanaryPreflightStatus {
    Missing,
    AcceptedByGate,
    RejectedByGate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TinyCanaryStrategyInputAuditStatus {
    Approved,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TinyCanaryBlockReason {
    MissingNoSubmitReadinessReport,
    LiveCanaryGateRejected,
    StrategyInputSafetyAuditBlocked,
    NonPositiveRealizedVolatility,
    NonPositiveTimeToExpiry,
    DecisionEvidenceUnavailable,
    BlockedBeforeLiveOrder,
    RootConfigHashUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanaryStrategyInputSafetyAudit {
    status: TinyCanaryStrategyInputAuditStatus,
    block_reasons: Vec<TinyCanaryBlockReason>,
}

impl TinyCanaryStrategyInputSafetyAudit {
    pub fn approved() -> Self {
        Self {
            status: TinyCanaryStrategyInputAuditStatus::Approved,
            block_reasons: Vec::new(),
        }
    }

    pub fn blocked(block_reasons: Vec<TinyCanaryBlockReason>) -> Self {
        Self {
            status: TinyCanaryStrategyInputAuditStatus::Blocked,
            block_reasons,
        }
    }

    pub fn from_strategy_inputs(realized_volatility: Decimal, seconds_to_expiry: u64) -> Self {
        let mut block_reasons = Vec::new();
        if realized_volatility <= Decimal::ZERO {
            block_reasons.push(TinyCanaryBlockReason::NonPositiveRealizedVolatility);
        }
        if seconds_to_expiry == 0 {
            block_reasons.push(TinyCanaryBlockReason::NonPositiveTimeToExpiry);
        }
        if block_reasons.is_empty() {
            Self::approved()
        } else {
            Self::blocked(block_reasons)
        }
    }

    pub fn from_evidence_file(
        path: impl AsRef<Path>,
        expected_sha256: impl AsRef<str>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let expected_sha256 = expected_sha256.as_ref().trim();
        if expected_sha256.is_empty() {
            return Err(anyhow!(
                "required tiny canary strategy input evidence sha256 is empty"
            ));
        }
        let bytes = fs::read(path).map_err(|source| {
            anyhow!(
                "failed to read tiny canary strategy input evidence `{}`: {source}",
                path.display()
            )
        })?;
        let current_sha256 = sha256_bytes(&bytes);
        if current_sha256 != expected_sha256 {
            return Err(anyhow!(
                "tiny canary strategy input evidence sha256 does not match current evidence"
            ));
        }
        let raw: TinyCanaryStrategyInputEvidenceFile =
            serde_json::from_slice(&bytes).map_err(|source| {
                anyhow!(
                    "failed to parse tiny canary strategy input evidence `{}`: {source}",
                    path.display()
                )
            })?;
        let realized_volatility =
            Decimal::from_str_exact(raw.realized_volatility.trim()).map_err(|source| {
                anyhow!("failed to parse tiny canary strategy input realized_volatility: {source}")
            })?;
        Ok(Self::from_strategy_inputs(
            realized_volatility,
            raw.seconds_to_expiry,
        ))
    }

    pub fn is_approved(&self) -> bool {
        self.status == TinyCanaryStrategyInputAuditStatus::Approved
    }

    pub fn block_reasons(&self) -> &[TinyCanaryBlockReason] {
        &self.block_reasons
    }
}

#[derive(Debug, Deserialize)]
struct TinyCanaryStrategyInputEvidenceFile {
    realized_volatility: String,
    seconds_to_expiry: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanaryPreflight {
    pub head_sha: String,
    pub root_config_sha256: String,
    pub no_submit_report_status: TinyCanaryPreflightStatus,
    pub strategy_input_audit_status: TinyCanaryStrategyInputAuditStatus,
    pub max_live_order_count: Option<u32>,
    pub max_notional_per_order: Option<String>,
    pub block_reasons: Vec<TinyCanaryBlockReason>,
}

impl TinyCanaryPreflight {
    pub fn can_enter_live_runner(&self) -> bool {
        self.block_reasons.is_empty()
            && self.no_submit_report_status == TinyCanaryPreflightStatus::AcceptedByGate
            && self.strategy_input_audit_status == TinyCanaryStrategyInputAuditStatus::Approved
            && self.max_live_order_count.is_some()
    }
}

pub async fn evaluate_tiny_canary_preflight(
    loaded: &LoadedBoltV3Config,
    head_sha: &str,
    strategy_audit: TinyCanaryStrategyInputSafetyAudit,
) -> TinyCanaryPreflight {
    let live_canary = loaded.root.live_canary.as_ref();
    let mut block_reasons = strategy_audit.block_reasons.clone();
    let root_config_sha256 =
        match TinyCanaryOperatorApprovalEnvelope::sha256_file(&loaded.root_path) {
            Ok(hash) => hash,
            Err(_) => {
                block_reasons.push(TinyCanaryBlockReason::RootConfigHashUnavailable);
                String::new()
            }
        };

    let no_submit_report_status = match check_bolt_v3_live_canary_gate(loaded).await {
        Ok(_) => TinyCanaryPreflightStatus::AcceptedByGate,
        Err(BoltV3LiveCanaryGateError::ReadinessReportRead { .. }) => {
            block_reasons.push(TinyCanaryBlockReason::MissingNoSubmitReadinessReport);
            TinyCanaryPreflightStatus::Missing
        }
        Err(_) => {
            block_reasons.push(TinyCanaryBlockReason::LiveCanaryGateRejected);
            TinyCanaryPreflightStatus::RejectedByGate
        }
    };
    TinyCanaryPreflight {
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
pub struct TinyCanaryEvidenceRef {
    pub path_hash: String,
    pub record_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanarySubmitAdmissionRef {
    pub status: String,
    pub admitted_order_count: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanaryRuntimeCaptureRef {
    pub spool_root_hash: String,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanaryNtLifecycleRef {
    pub kind: String,
    pub event_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TinyCanaryOutcome {
    DryNoSubmitProof,
    BlockedBeforeSubmit,
    LiveCanaryProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanaryLiveOrderRef {
    pub client_order_id_hash: String,
    pub venue_order_id_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TinyCanaryLiveCanaryResultRefs {
    pub nt_submit_event_ref: TinyCanaryEvidenceRef,
    pub venue_order_state_ref: TinyCanaryEvidenceRef,
    pub strategy_cancel_ref: Option<TinyCanaryEvidenceRef>,
    pub restart_reconciliation_ref: TinyCanaryEvidenceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TinyCanaryEvidence {
    pub schema_version: u32,
    pub head_sha: String,
    pub root_config_sha256: String,
    pub ssm_manifest_sha256: String,
    pub ssm_manifest_ref: TinyCanaryEvidenceRef,
    pub strategy_input_evidence_ref: TinyCanaryEvidenceRef,
    pub approval_id_hash: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
    pub decision_evidence_ref: Option<TinyCanaryEvidenceRef>,
    pub submit_admission_ref: TinyCanarySubmitAdmissionRef,
    pub live_order_ref: Option<TinyCanaryLiveOrderRef>,
    pub nt_submit_event_ref: Option<TinyCanaryEvidenceRef>,
    pub venue_order_state_ref: Option<TinyCanaryEvidenceRef>,
    pub strategy_cancel_ref: Option<TinyCanaryEvidenceRef>,
    pub restart_reconciliation_ref: Option<TinyCanaryEvidenceRef>,
    pub runtime_capture_ref: TinyCanaryRuntimeCaptureRef,
    pub nt_lifecycle_refs: Vec<TinyCanaryNtLifecycleRef>,
    pub outcome: TinyCanaryOutcome,
    pub block_reasons: Vec<TinyCanaryBlockReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TinyCanaryEvidenceInput {
    pub head_sha: String,
    pub root_config_sha256: String,
    pub ssm_manifest_sha256: String,
    pub ssm_manifest_ref: TinyCanaryEvidenceRef,
    pub strategy_input_evidence_ref: TinyCanaryEvidenceRef,
    pub approval_id: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: Decimal,
    pub runtime_capture_ref: TinyCanaryRuntimeCaptureRef,
}

impl TinyCanaryEvidence {
    pub fn dry_no_submit_proof(
        input: TinyCanaryEvidenceInput,
        decision_evidence_ref: TinyCanaryEvidenceRef,
    ) -> Self {
        Self {
            schema_version: TINY_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            ssm_manifest_ref: input.ssm_manifest_ref,
            strategy_input_evidence_ref: input.strategy_input_evidence_ref,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: Some(decision_evidence_ref),
            submit_admission_ref: TinyCanarySubmitAdmissionRef {
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
            outcome: TinyCanaryOutcome::DryNoSubmitProof,
            block_reasons: vec![TinyCanaryBlockReason::BlockedBeforeLiveOrder],
        }
    }

    pub fn blocked_before_submit(
        input: TinyCanaryEvidenceInput,
        block_reason: TinyCanaryBlockReason,
    ) -> Self {
        Self {
            schema_version: TINY_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            ssm_manifest_ref: input.ssm_manifest_ref,
            strategy_input_evidence_ref: input.strategy_input_evidence_ref,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: None,
            submit_admission_ref: TinyCanarySubmitAdmissionRef {
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
            outcome: TinyCanaryOutcome::BlockedBeforeSubmit,
            block_reasons: vec![block_reason],
        }
    }

    pub fn live_canary_proof(
        input: TinyCanaryEvidenceInput,
        decision_evidence_ref: TinyCanaryEvidenceRef,
        live_order_ref: TinyCanaryLiveOrderRef,
        result_refs: TinyCanaryLiveCanaryResultRefs,
        admitted_order_count: u32,
    ) -> Result<Self> {
        if admitted_order_count == 0 {
            return Err(anyhow!(
                "tiny canary live canary proof admitted_order_count must be positive"
            ));
        }
        if admitted_order_count > input.max_live_order_count {
            return Err(anyhow!(
                "tiny canary live canary proof admitted_order_count must be at most configured max_live_order_count {} got {admitted_order_count}",
                input.max_live_order_count
            ));
        }
        Ok(Self {
            schema_version: TINY_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            ssm_manifest_ref: input.ssm_manifest_ref,
            strategy_input_evidence_ref: input.strategy_input_evidence_ref,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: Some(decision_evidence_ref),
            submit_admission_ref: TinyCanarySubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_ACCEPTED.to_string(),
                admitted_order_count,
                reason: NT_ADAPTER_SUBMIT_PROVEN_REASON.to_string(),
            },
            live_order_ref: Some(live_order_ref),
            nt_submit_event_ref: Some(result_refs.nt_submit_event_ref),
            venue_order_state_ref: Some(result_refs.venue_order_state_ref),
            strategy_cancel_ref: result_refs.strategy_cancel_ref,
            restart_reconciliation_ref: Some(result_refs.restart_reconciliation_ref),
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: TinyCanaryOutcome::LiveCanaryProof,
            block_reasons: Vec::new(),
        })
    }

    pub fn write_json_file(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| {
                anyhow!(
                    "failed to create tiny canary evidence directory `{}`: {source}",
                    parent.display()
                )
            })?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|source| anyhow!("failed to serialize tiny canary evidence: {source}"))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::AlreadyExists => anyhow!(
                    "tiny canary evidence `{}` already exists; refusing to overwrite",
                    path.display()
                ),
                _ => anyhow!(
                    "failed to create tiny canary evidence `{}`: {source}",
                    path.display()
                ),
            })?;
        if let Err(source) = file.write_all(&bytes) {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to write tiny canary evidence `{}`: {source}",
                path.display()
            ));
        }
        if let Err(source) = file.sync_all() {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to sync tiny canary evidence `{}`: {source}",
                path.display()
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TinyCanaryOperatorApprovalEnvelope {
    pub head_sha: String,
    pub root_toml_path: String,
    pub root_toml_sha256: String,
    pub ssm_manifest_path: String,
    pub ssm_manifest_sha256: String,
    pub strategy_input_evidence_path: String,
    pub strategy_input_evidence_sha256: String,
    pub operator_approval_id: String,
    pub approval_not_before_unix_seconds: i64,
    pub approval_not_after_unix_seconds: i64,
    pub approval_nonce_path: String,
    pub approval_nonce_sha256: String,
    pub approval_consumption_path: String,
    pub canary_evidence_path: String,
}

impl TinyCanaryOperatorApprovalEnvelope {
    pub fn from_config(
        loaded: &LoadedBoltV3Config,
        current_head_sha: &str,
        current_root_toml_sha256: &str,
    ) -> Result<Self> {
        let live_canary = loaded
            .root
            .live_canary
            .as_ref()
            .ok_or_else(|| anyhow!("tiny canary operator approval requires `[live_canary]`"))?;
        let operator_evidence = live_canary.operator_evidence.as_ref().ok_or_else(|| {
            anyhow!("tiny canary operator approval requires `[live_canary.operator_evidence]`")
        })?;
        Ok(Self {
            head_sha: current_head_sha.to_string(),
            root_toml_path: loaded.root_path.to_string_lossy().to_string(),
            root_toml_sha256: current_root_toml_sha256.to_string(),
            ssm_manifest_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.ssm_manifest_path,
                "[live_canary.operator_evidence].ssm_manifest_path",
            )?,
            ssm_manifest_sha256: required_config_value(
                &operator_evidence.ssm_manifest_sha256,
                "[live_canary.operator_evidence].ssm_manifest_sha256",
            )?,
            strategy_input_evidence_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.strategy_input_evidence_path,
                "[live_canary.operator_evidence].strategy_input_evidence_path",
            )?,
            strategy_input_evidence_sha256: required_config_value(
                &operator_evidence.strategy_input_evidence_sha256,
                "[live_canary.operator_evidence].strategy_input_evidence_sha256",
            )?,
            operator_approval_id: required_config_value(
                &live_canary.approval_id,
                "[live_canary].approval_id",
            )?,
            canary_evidence_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.canary_evidence_path,
                "[live_canary.operator_evidence].canary_evidence_path",
            )?,
            approval_not_before_unix_seconds: operator_evidence.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: operator_evidence.approval_not_after_unix_seconds,
            approval_nonce_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.approval_nonce_path,
                "[live_canary.operator_evidence].approval_nonce_path",
            )?,
            approval_nonce_sha256: required_config_value(
                &operator_evidence.approval_nonce_sha256,
                "[live_canary.operator_evidence].approval_nonce_sha256",
            )?,
            approval_consumption_path: required_operator_path(
                &loaded.root_path,
                &operator_evidence.approval_consumption_path,
                "[live_canary.operator_evidence].approval_consumption_path",
            )?,
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
                "tiny canary operator approval head_sha does not match current head"
            ));
        }
        if self.root_toml_sha256 != current_root_toml_sha256 {
            return Err(anyhow!(
                "tiny canary operator approval root_toml_sha256 does not match current root TOML"
            ));
        }
        let current_ssm_manifest_sha256 = Self::sha256_file(&self.ssm_manifest_path)?;
        if self.ssm_manifest_sha256 != current_ssm_manifest_sha256 {
            return Err(anyhow!(
                "tiny canary operator approval ssm_manifest_sha256 does not match current SSM manifest"
            ));
        }
        let current_strategy_input_evidence_sha256 =
            Self::sha256_file(&self.strategy_input_evidence_path)?;
        if self.strategy_input_evidence_sha256 != current_strategy_input_evidence_sha256 {
            return Err(anyhow!(
                "tiny canary operator approval strategy_input_evidence_sha256 does not match current strategy input evidence"
            ));
        }
        if self.operator_approval_id != live_canary_approval_id {
            return Err(anyhow!(
                "tiny canary operator approval id does not match `[live_canary]`"
            ));
        }
        Ok(())
    }

    pub fn validate_and_consume_against(
        &self,
        current_head_sha: &str,
        current_root_toml_sha256: &str,
        live_canary_approval_id: &str,
        current_unix_seconds: i64,
    ) -> Result<()> {
        self.validate_against(
            current_head_sha,
            current_root_toml_sha256,
            live_canary_approval_id,
        )?;
        self.validate_approval_window(current_unix_seconds)?;
        let current_nonce_sha256 = Self::sha256_file(&self.approval_nonce_path)?;
        if self.approval_nonce_sha256 != current_nonce_sha256 {
            return Err(anyhow!(
                "tiny canary operator approval nonce sha256 does not match current nonce evidence"
            ));
        }
        self.write_approval_consumption_evidence(current_unix_seconds)
    }

    fn validate_approval_window(&self, current_unix_seconds: i64) -> Result<()> {
        if self.approval_not_after_unix_seconds < self.approval_not_before_unix_seconds {
            return Err(anyhow!(
                "tiny canary operator approval not_after is before not_before"
            ));
        }
        if current_unix_seconds < self.approval_not_before_unix_seconds {
            return Err(anyhow!("tiny canary operator approval is not yet valid"));
        }
        if current_unix_seconds > self.approval_not_after_unix_seconds {
            return Err(anyhow!("tiny canary operator approval is expired"));
        }
        Ok(())
    }

    fn write_approval_consumption_evidence(&self, current_unix_seconds: i64) -> Result<()> {
        let path = Path::new(&self.approval_consumption_path);
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| {
                anyhow!(
                    "failed to create tiny canary approval consumption evidence directory `{}`: {source}",
                    parent.display()
                )
            })?;
        }
        let evidence = TinyCanaryApprovalConsumptionEvidence {
            schema_version: TINY_CANARY_APPROVAL_CONSUMPTION_SCHEMA_VERSION,
            record_kind: TINY_CANARY_APPROVAL_CONSUMPTION_RECORD_KIND,
            head_sha: &self.head_sha,
            root_toml_sha256: &self.root_toml_sha256,
            ssm_manifest_sha256: &self.ssm_manifest_sha256,
            strategy_input_evidence_sha256: &self.strategy_input_evidence_sha256,
            approval_id_hash: sha256_text(&self.operator_approval_id),
            approval_nonce_sha256: &self.approval_nonce_sha256,
            approval_not_before_unix_seconds: self.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: self.approval_not_after_unix_seconds,
            canary_evidence_path_hash: sha256_text(&self.canary_evidence_path),
            consumed_unix_seconds: current_unix_seconds,
        };
        let bytes = serde_json::to_vec_pretty(&evidence).map_err(|source| {
            anyhow!("failed to serialize tiny canary approval consumption evidence: {source}")
        })?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::AlreadyExists => anyhow!(
                    "tiny canary operator approval consumption `{}` already consumed; refusing to replay",
                    path.display()
                ),
                _ => anyhow!(
                    "failed to create tiny canary operator approval consumption `{}`: {source}",
                    path.display()
                ),
            })?;
        if let Err(source) = file.write_all(&bytes) {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to write tiny canary operator approval consumption `{}`: {source}",
                path.display()
            ));
        }
        if let Err(source) = file.sync_all() {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to sync tiny canary operator approval consumption `{}`: {source}",
                path.display()
            ));
        }
        Ok(())
    }

    pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        let file = fs::File::open(path).map_err(|source| {
            anyhow!(
                "failed to open tiny canary sha256 input `{}`: {source}",
                path.display()
            )
        })?;
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut buffer = [0; TINY_CANARY_SHA256_BUFFER_BYTES];
        loop {
            let length = reader.read(&mut buffer).map_err(|source| {
                anyhow!(
                    "failed to read tiny canary sha256 input `{}`: {source}",
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

#[derive(Serialize)]
struct TinyCanaryApprovalConsumptionEvidence<'a> {
    schema_version: u32,
    record_kind: &'static str,
    head_sha: &'a str,
    root_toml_sha256: &'a str,
    ssm_manifest_sha256: &'a str,
    strategy_input_evidence_sha256: &'a str,
    approval_id_hash: String,
    approval_nonce_sha256: &'a str,
    approval_not_before_unix_seconds: i64,
    approval_not_after_unix_seconds: i64,
    canary_evidence_path_hash: String,
    consumed_unix_seconds: i64,
}

fn required_config_value(value: &str, field: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!(
            "required tiny canary config field `{field}` is empty"
        ));
    }
    Ok(trimmed.to_string())
}

fn required_operator_path(root_path: &Path, value: &str, field: &str) -> Result<String> {
    let trimmed = required_config_value(value, field)?;
    Ok(resolve_root_relative_path(root_path, trimmed)
        .to_string_lossy()
        .to_string())
}

fn sha256_text(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

pub fn tiny_canary_sha256_text(value: &str) -> String {
    sha256_text(value)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}
