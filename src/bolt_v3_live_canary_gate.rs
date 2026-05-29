//! Bolt-v3 live canary approval gate.
//!
//! This module is intentionally an admission boundary only. It reads
//! operator approval and a prior no-submit readiness report from the
//! loaded TOML contract, but it does not connect, subscribe, submit,
//! cancel, or mutate NT state.
//!
//! The gate validates the configured live-canary bounds before the NT
//! runner starts. Submit-time admission remains the boundary that must
//! independently consume validated bounds from this gate before live
//! order submission is enabled.

use std::{
    path::{Component, Path, PathBuf},
    str::FromStr,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

use nautilus_model::identifiers::ActorId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::{
    bolt_v3_canary_proof_policy::{CANARY_PROOF_CLAIM, CanaryProofOrderIntentArtifact},
    bolt_v3_config::{
        DECISION_REFERENCE_GATE_ROLE, LiveCanaryBlock, LiveCanaryOperatorEvidenceBlock,
        LoadedBoltV3Config,
    },
    bolt_v3_decision_evidence::{
        BoltV3ReadinessGateEvidenceSnapshot, read_latest_entry_decision_evidence_chain,
        validate_readiness_gate_evidence_snapshot,
    },
    bolt_v3_no_submit_readiness_schema::{
        APPROVAL_CONSUMPTION_RECORD_KIND, APPROVAL_CONSUMPTION_SCHEMA_VERSION,
        APPROVAL_ID_HASH_KEY, CONFIG_BUNDLE_CHECKSUM_KEY, CONTROLLED_CONNECT_STAGE,
        CONTROLLED_DISCONNECT_STAGE, EXECUTABLE_IDENTITY_KEY, GENERATED_AT_UNIX_SECONDS_KEY,
        LIVE_NODE_BUILD_STAGE, NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE,
        REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY, SECRET_RESOLUTION_STAGE,
        STAGE_KEY, STAGES_KEY, STATUS_KEY, STATUS_SATISFIED,
    },
    bolt_v3_operator_artifacts::EntryReadinessGateSession,
};

pub const APPROVAL_ENVELOPE_SCHEMA_VERSION: i64 = 1;
pub const APPROVAL_ENVELOPE_RECORD_KIND: &str = "phase8_operator_approval_envelope";
const MILLIS_PER_SECOND_U64: u64 = 1_000;

/// Successful live canary gate evaluation.
///
/// The report carries the validated operator approval id, resolved
/// no-submit readiness report path, approved canary order-count bound,
/// approved per-order notional bound, and root risk notional bound.
/// Submit-time admission must consume these validated bounds before
/// any live canary order is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3LiveCanaryGateReport {
    approval_id: String,
    no_submit_readiness_report_path: PathBuf,
    max_no_submit_readiness_report_bytes: u64,
    readiness_report_max_age_seconds: u64,
    max_live_order_count: u32,
    max_notional_per_order: Decimal,
    root_max_notional_per_order: Decimal,
}

impl BoltV3LiveCanaryGateReport {
    pub fn approval_id(&self) -> &str {
        &self.approval_id
    }

    pub fn no_submit_readiness_report_path(&self) -> &Path {
        &self.no_submit_readiness_report_path
    }

    pub fn max_no_submit_readiness_report_bytes(&self) -> u64 {
        self.max_no_submit_readiness_report_bytes
    }

    pub fn readiness_report_max_age_seconds(&self) -> u64 {
        self.readiness_report_max_age_seconds
    }

    pub fn max_live_order_count(&self) -> u32 {
        self.max_live_order_count
    }

    pub fn max_notional_per_order(&self) -> Decimal {
        self.max_notional_per_order
    }

    pub fn root_max_notional_per_order(&self) -> Decimal {
        self.root_max_notional_per_order
    }

    #[cfg(test)]
    pub(crate) fn for_test(max_live_order_count: u32, max_notional_per_order: Decimal) -> Self {
        Self {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: PathBuf::from("no-submit-readiness.json"),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: 60,
            max_live_order_count,
            max_notional_per_order,
            root_max_notional_per_order: max_notional_per_order,
        }
    }
}

/// Fail-closed reasons returned by the bolt-v3 live canary gate before
/// NT's runner loop is entered.
#[derive(Debug)]
pub enum BoltV3LiveCanaryGateError {
    MissingConfig,
    MissingApprovalId,
    MissingReadinessReportPath,
    InvalidConfiguredPath {
        field: &'static str,
        value: String,
    },
    InvalidMaxLiveOrderCount {
        value: u32,
    },
    InvalidReadinessReportSizeLimit {
        value: u64,
    },
    InvalidReadinessReportMaxAge {
        value: u64,
    },
    InvalidReferenceQuoteMaxAge {
        value: u64,
    },
    InvalidReferenceQuoteWaitTimeout {
        value: u64,
    },
    InvalidReferenceQuoteProbeActorId {
        value: String,
        reason: String,
    },
    InvalidOperatorEvidenceSizeLimit {
        value: u64,
    },
    InvalidApprovalConsumptionMaxAge {
        value: u64,
    },
    InvalidMaxNotional {
        field: &'static str,
        value: String,
        reason: String,
    },
    MaxNotionalExceedsRootRisk {
        max_notional_per_order: Decimal,
        root_max_notional_per_order: Decimal,
    },
    MissingOperatorEvidence,
    MissingOperatorEvidenceField {
        field: &'static str,
    },
    InvalidOperatorEvidenceHeadShaShape {
        field: &'static str,
    },
    BuildHeadShaUnavailable,
    OperatorEvidenceHeadShaMismatch {
        expected: &'static str,
        actual: String,
    },
    InvalidOperatorEvidenceHashShape {
        field: &'static str,
    },
    OperatorEvidenceRead {
        field: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorEvidenceHashMismatch {
        field: &'static str,
        path: PathBuf,
    },
    OperatorGateSessionParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    OperatorGateSessionInvalid {
        path: PathBuf,
        reason: String,
    },
    OperatorStrategyInputEvidenceParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    OperatorStrategyInputEvidenceInvalid {
        path: PathBuf,
        reason: String,
    },
    OperatorDecisionEvidenceInvalid {
        path: PathBuf,
        reason: String,
    },
    OperatorApprovalEnvelopeRead {
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorApprovalEnvelopeParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    OperatorApprovalEnvelopeMismatch {
        path: PathBuf,
        field: &'static str,
    },
    OperatorApprovalConsumptionRead {
        path: PathBuf,
        source: std::io::Error,
    },
    OperatorApprovalConsumptionAlreadyExistsBeforeRunner {
        path: PathBuf,
    },
    OperatorApprovalConsumptionParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    OperatorApprovalConsumptionMalformed {
        path: PathBuf,
        reason: String,
    },
    OperatorApprovalConsumptionMismatch {
        path: PathBuf,
        field: &'static str,
    },
    OperatorApprovalConsumptionStale {
        path: PathBuf,
        consumed_unix_secs: i64,
        approval_not_before_unix_seconds: i64,
        approval_not_after_unix_seconds: i64,
        current_unix_seconds: u64,
        approval_consumption_max_age_seconds: u64,
    },
    RootTomlRead {
        path: PathBuf,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    InvalidOperatorApprovalWindow {
        approval_not_before_unix_seconds: i64,
        approval_not_after_unix_seconds: i64,
    },
    InactiveOperatorApprovalWindow {
        current_unix_seconds: u64,
        approval_not_before_unix_seconds: i64,
        approval_not_after_unix_seconds: i64,
    },
    ReadinessReportRead {
        path: PathBuf,
        source: std::io::Error,
    },
    ReadinessReportTooLarge {
        path: PathBuf,
        length: u64,
        max_length: u64,
    },
    ReadinessReportParse {
        path: PathBuf,
        source: serde_json::Error,
    },
    ReadinessReportSchemaVersionMismatch {
        path: PathBuf,
        expected: &'static str,
        actual: Option<String>,
    },
    CurrentExecutablePath {
        source: std::io::Error,
    },
    ExecutableIdentityRead {
        path: PathBuf,
        source: std::io::Error,
    },
    SystemTimeBeforeUnixEpoch {
        source: SystemTimeError,
    },
    UnsatisfiedNoSubmitReadinessReport {
        path: PathBuf,
        reasons: Vec<String>,
    },
}

impl std::fmt::Display for BoltV3LiveCanaryGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveCanaryGateError::MissingConfig => {
                write!(f, "bolt-v3 live canary gate is missing `[live_canary]`")
            }
            BoltV3LiveCanaryGateError::MissingApprovalId => {
                write!(f, "bolt-v3 live canary approval_id is empty")
            }
            BoltV3LiveCanaryGateError::MissingReadinessReportPath => {
                write!(
                    f,
                    "bolt-v3 live canary no_submit_readiness_report_path is empty"
                )
            }
            BoltV3LiveCanaryGateError::InvalidConfiguredPath { field, value } => write!(
                f,
                "bolt-v3 live canary configured path `{field}` must not contain parent directory traversal: `{value}`"
            ),
            BoltV3LiveCanaryGateError::InvalidMaxLiveOrderCount { value } => write!(
                f,
                "bolt-v3 live canary max_live_order_count must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidReadinessReportSizeLimit { value } => write!(
                f,
                "bolt-v3 live canary max_no_submit_readiness_report_bytes must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidReadinessReportMaxAge { value } => write!(
                f,
                "bolt-v3 live canary readiness_report_max_age_seconds must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteMaxAge { value } => write!(
                f,
                "bolt-v3 live canary reference_quote_max_age_seconds must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteWaitTimeout { value } => write!(
                f,
                "bolt-v3 live canary reference_quote_wait_timeout_seconds must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteProbeActorId { value, reason } => {
                write!(
                    f,
                    "bolt-v3 live canary reference_quote_probe_actor_id `{value}` is invalid: {reason}"
                )
            }
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceSizeLimit { value } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.max_operator_evidence_file_bytes` must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidApprovalConsumptionMaxAge { value } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.approval_consumption_max_age_seconds` must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidMaxNotional {
                field,
                value,
                reason,
            } => write!(
                f,
                "bolt-v3 live canary {field} is not a valid positive decimal ({reason}): `{value}`"
            ),
            BoltV3LiveCanaryGateError::MaxNotionalExceedsRootRisk {
                max_notional_per_order,
                root_max_notional_per_order,
            } => write!(
                f,
                "bolt-v3 live canary max_notional_per_order ({max_notional_per_order}) exceeds \
                 risk.default_max_notional_per_order ({root_max_notional_per_order})"
            ),
            BoltV3LiveCanaryGateError::MissingOperatorEvidence => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence` is required"
            ),
            BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.{field}` is empty"
            ),
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHeadShaShape { field } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.{field}` must be a 40-character lowercase git head sha"
            ),
            BoltV3LiveCanaryGateError::BuildHeadShaUnavailable => write!(
                f,
                "bolt-v3 live canary build head_sha is unavailable or invalid"
            ),
            BoltV3LiveCanaryGateError::OperatorEvidenceHeadShaMismatch { expected, actual } => {
                write!(
                    f,
                    "bolt-v3 live canary `[live_canary].operator_evidence.head_sha` ({actual}) does not match build head_sha ({expected})"
                )
            }
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHashShape { field } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.{field}` must be a lowercase sha256 hex string"
            ),
            BoltV3LiveCanaryGateError::OperatorEvidenceRead {
                field,
                path,
                source,
            } => write!(
                f,
                "failed to read bolt-v3 live canary operator evidence for {field} at {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorEvidenceHashMismatch { field, path } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.{field}` does not match {}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorGateSessionParse { path, source } => write!(
                f,
                "failed to parse bolt-v3 live canary gate session {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorGateSessionInvalid { path, reason } => write!(
                f,
                "bolt-v3 live canary gate session {} is invalid: {reason}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceParse { path, source } => {
                write!(
                    f,
                    "failed to parse bolt-v3 live canary strategy_input_evidence {}: {source}",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid { path, reason } => {
                write!(
                    f,
                    "bolt-v3 live canary strategy_input_evidence {} is invalid: {reason}",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::OperatorDecisionEvidenceInvalid { path, reason } => {
                write!(
                    f,
                    "bolt-v3 live canary decision_evidence {} is invalid: {reason}",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeRead { path, source } => write!(
                f,
                "failed to read bolt-v3 live canary approval envelope {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeParse { path, source } => write!(
                f,
                "failed to parse bolt-v3 live canary approval envelope {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch { path, field } => write!(
                f,
                "bolt-v3 live canary approval envelope {} field `{field}` does not match configured operator evidence",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionRead { path, source } => write!(
                f,
                "failed to read bolt-v3 live canary approval consumption proof {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionAlreadyExistsBeforeRunner {
                path,
            } => write!(
                f,
                "bolt-v3 live canary approval consumption proof {} already exists before live runner entry validation",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionParse { path, source } => write!(
                f,
                "failed to parse bolt-v3 live canary approval consumption proof {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed { path, reason } => {
                write!(
                    f,
                    "bolt-v3 live canary approval consumption proof {} is malformed: {reason}",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch { path, field } => {
                write!(
                    f,
                    "bolt-v3 live canary approval consumption proof {} field `{field}` does not match configured operator evidence",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionStale {
                path,
                consumed_unix_secs,
                approval_not_before_unix_seconds,
                approval_not_after_unix_seconds,
                current_unix_seconds,
                approval_consumption_max_age_seconds,
            } => write!(
                f,
                "bolt-v3 live canary approval consumption proof {} is stale: consumed_unix_secs={consumed_unix_secs}, current_unix_seconds={current_unix_seconds}, approval window [{approval_not_before_unix_seconds}, {approval_not_after_unix_seconds}], approval_consumption_max_age_seconds={approval_consumption_max_age_seconds}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::RootTomlRead { path, source } => write!(
                f,
                "failed to read bolt-v3 live canary root TOML {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::InvalidOperatorApprovalWindow {
                approval_not_before_unix_seconds,
                approval_not_after_unix_seconds,
            } => write!(
                f,
                "bolt-v3 live canary `[live_canary].operator_evidence.approval_not_after_unix_seconds` \
                 must be greater than approval_not_before_unix_seconds \
                 ({approval_not_before_unix_seconds}), got {approval_not_after_unix_seconds}"
            ),
            BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
                current_unix_seconds,
                approval_not_before_unix_seconds,
                approval_not_after_unix_seconds,
            } => write!(
                f,
                "bolt-v3 live canary operator approval window is not active: current time \
                 {current_unix_seconds} is outside \
                 [{approval_not_before_unix_seconds}, {approval_not_after_unix_seconds}]"
            ),
            BoltV3LiveCanaryGateError::ReadinessReportRead { path, source } => {
                write!(
                    f,
                    "failed to read bolt-v3 no-submit readiness report {}: {source}",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::ReadinessReportTooLarge {
                path,
                length,
                max_length,
            } => write!(
                f,
                "bolt-v3 no-submit readiness report {} is {length} bytes, exceeding configured limit {max_length}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::ReadinessReportParse { path, source } => {
                write!(
                    f,
                    "failed to parse bolt-v3 no-submit readiness report {}: {source}",
                    path.display()
                )
            }
            BoltV3LiveCanaryGateError::ReadinessReportSchemaVersionMismatch {
                path,
                expected,
                actual,
            } => match actual {
                Some(actual) => write!(
                    f,
                    "bolt-v3 no-submit readiness report {} schema_version is `{actual}`, expected `{expected}`",
                    path.display()
                ),
                None => write!(
                    f,
                    "bolt-v3 no-submit readiness report {} schema_version is missing or not a string, expected `{expected}`",
                    path.display()
                ),
            },
            BoltV3LiveCanaryGateError::CurrentExecutablePath { source } => write!(
                f,
                "failed to resolve bolt-v3 live canary gate executable path: {source}"
            ),
            BoltV3LiveCanaryGateError::ExecutableIdentityRead { path, source } => write!(
                f,
                "failed to read bolt-v3 live canary gate executable {}: {source}",
                path.display()
            ),
            BoltV3LiveCanaryGateError::SystemTimeBeforeUnixEpoch { source } => write!(
                f,
                "failed to timestamp bolt-v3 live canary gate evaluation: {source}"
            ),
            BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { path, reasons } => {
                write!(
                    f,
                    "bolt-v3 no-submit readiness report {} is not satisfied: {}",
                    path.display(),
                    reasons.join("; ")
                )
            }
        }
    }
}

impl std::error::Error for BoltV3LiveCanaryGateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveCanaryGateError::ReadinessReportRead { source, .. } => Some(source),
            BoltV3LiveCanaryGateError::ReadinessReportParse { source, .. } => Some(source),
            BoltV3LiveCanaryGateError::OperatorEvidenceRead { source, .. } => Some(source),
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeRead { source, .. } => Some(source),
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeParse { source, .. } => Some(source),
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionRead { source, .. } => {
                Some(source)
            }
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionParse { source, .. } => {
                Some(source)
            }
            BoltV3LiveCanaryGateError::RootTomlRead { source, .. } => Some(source.as_ref()),
            BoltV3LiveCanaryGateError::CurrentExecutablePath { source } => Some(source),
            BoltV3LiveCanaryGateError::ExecutableIdentityRead { source, .. } => Some(source),
            BoltV3LiveCanaryGateError::SystemTimeBeforeUnixEpoch { source } => Some(source),
            _ => None,
        }
    }
}

/// Validate the loaded config's `[live_canary]` section and referenced
/// no-submit readiness report before NT's runner loop is entered.
///
/// The gate is read-only: it does not connect, subscribe, submit,
/// cancel, or mutate NT state. Relative readiness report paths resolve
/// from the root TOML directory.
pub async fn check_bolt_v3_live_canary_gate(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveCanaryGateReport, BoltV3LiveCanaryGateError> {
    check_bolt_v3_live_canary_gate_with_clock(loaded, current_unix_seconds).await
}

pub async fn check_bolt_v3_live_canary_pre_consumption_gate(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveCanaryGateReport, BoltV3LiveCanaryGateError> {
    check_bolt_v3_live_canary_gate_with_clock_and_approval_consumption(
        loaded,
        current_unix_seconds,
        ApprovalConsumptionExpectation::DeferredUntilLiveRunnerEntry,
    )
    .await
}

async fn check_bolt_v3_live_canary_gate_with_clock(
    loaded: &LoadedBoltV3Config,
    mut unix_seconds: impl FnMut() -> Result<u64, BoltV3LiveCanaryGateError>,
) -> Result<BoltV3LiveCanaryGateReport, BoltV3LiveCanaryGateError> {
    check_bolt_v3_live_canary_gate_with_clock_and_approval_consumption(
        loaded,
        &mut unix_seconds,
        ApprovalConsumptionExpectation::MustExistAndBeValid,
    )
    .await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalConsumptionExpectation {
    MustExistAndBeValid,
    DeferredUntilLiveRunnerEntry,
}

async fn check_bolt_v3_live_canary_gate_with_clock_and_approval_consumption(
    loaded: &LoadedBoltV3Config,
    mut unix_seconds: impl FnMut() -> Result<u64, BoltV3LiveCanaryGateError>,
    approval_consumption_expectation: ApprovalConsumptionExpectation,
) -> Result<BoltV3LiveCanaryGateReport, BoltV3LiveCanaryGateError> {
    let block = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3LiveCanaryGateError::MissingConfig)?;
    let approval_id = block.approval_id.trim();
    if approval_id.is_empty() {
        return Err(BoltV3LiveCanaryGateError::MissingApprovalId);
    }
    if block.no_submit_readiness_report_path.trim().is_empty() {
        return Err(BoltV3LiveCanaryGateError::MissingReadinessReportPath);
    }
    if block.max_live_order_count == 0 {
        return Err(BoltV3LiveCanaryGateError::InvalidMaxLiveOrderCount {
            value: block.max_live_order_count,
        });
    }
    if block.max_no_submit_readiness_report_bytes == 0 {
        return Err(BoltV3LiveCanaryGateError::InvalidReadinessReportSizeLimit {
            value: block.max_no_submit_readiness_report_bytes,
        });
    }
    if block.readiness_report_max_age_seconds == 0 {
        return Err(BoltV3LiveCanaryGateError::InvalidReadinessReportMaxAge {
            value: block.readiness_report_max_age_seconds,
        });
    }
    if block.reference_quote_max_age_seconds == 0 {
        return Err(BoltV3LiveCanaryGateError::InvalidReferenceQuoteMaxAge {
            value: block.reference_quote_max_age_seconds,
        });
    }
    if block.reference_quote_wait_timeout_seconds == 0 {
        return Err(
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteWaitTimeout {
                value: block.reference_quote_wait_timeout_seconds,
            },
        );
    }
    let reference_quote_probe_actor_id = block.reference_quote_probe_actor_id.as_str();
    if reference_quote_probe_actor_id.trim().is_empty()
        || reference_quote_probe_actor_id.trim() != reference_quote_probe_actor_id
    {
        return Err(
            BoltV3LiveCanaryGateError::InvalidReferenceQuoteProbeActorId {
                value: block.reference_quote_probe_actor_id.clone(),
                reason: "must be non-empty without surrounding whitespace".to_string(),
            },
        );
    }
    ActorId::new_checked(reference_quote_probe_actor_id).map_err(|error| {
        BoltV3LiveCanaryGateError::InvalidReferenceQuoteProbeActorId {
            value: block.reference_quote_probe_actor_id.clone(),
            reason: error.to_string(),
        }
    })?;

    let max_notional_per_order = parse_positive_decimal(
        "max_notional_per_order",
        block.max_notional_per_order.as_str(),
    )?;
    // Keep the run boundary fail-closed even if a caller constructs
    // LoadedBoltV3Config outside the normal validation path.
    let root_max_notional_per_order = parse_positive_decimal(
        "risk.default_max_notional_per_order",
        loaded.root.risk.default_max_notional_per_order.as_str(),
    )?;
    if max_notional_per_order > root_max_notional_per_order {
        return Err(BoltV3LiveCanaryGateError::MaxNotionalExceedsRootRisk {
            max_notional_per_order,
            root_max_notional_per_order,
        });
    }

    let initial_unix_seconds = unix_seconds()?;
    validate_operator_evidence(
        loaded,
        block,
        approval_id,
        max_notional_per_order,
        initial_unix_seconds,
        initial_unix_seconds,
        approval_consumption_expectation,
    )
    .await?;

    let report_path = resolve_report_path(&loaded.root_path, block)?;
    let report_bytes =
        read_report_bytes_with_limit(&report_path, block.max_no_submit_readiness_report_bytes)
            .await?;
    let report: Value = serde_json::from_slice(&report_bytes).map_err(|source| {
        BoltV3LiveCanaryGateError::ReadinessReportParse {
            path: report_path.clone(),
            source,
        }
    })?;
    let Some(report_object) = report.as_object() else {
        return Err(
            BoltV3LiveCanaryGateError::ReadinessReportSchemaVersionMismatch {
                path: report_path.clone(),
                expected: NO_SUBMIT_READINESS_SCHEMA_VERSION,
                actual: None,
            },
        );
    };
    let observed_schema_version = report_object
        .get(SCHEMA_VERSION_KEY)
        .and_then(Value::as_str);
    if observed_schema_version != Some(NO_SUBMIT_READINESS_SCHEMA_VERSION) {
        return Err(
            BoltV3LiveCanaryGateError::ReadinessReportSchemaVersionMismatch {
                path: report_path.clone(),
                expected: NO_SUBMIT_READINESS_SCHEMA_VERSION,
                actual: observed_schema_version.map(str::to_string),
            },
        );
    }
    let expected_approval_id_hash = sha256_hex(approval_id.as_bytes());
    let expected_executable_identity = executable_identity().await?;
    let late_unix_seconds = unix_seconds()?;
    validate_no_submit_readiness_report(
        report_object,
        &expected_approval_id_hash,
        &expected_executable_identity,
        &loaded.config_bundle_checksum,
        block.readiness_report_max_age_seconds,
        late_unix_seconds,
    )
    .map_err(
        |reasons| BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport {
            path: report_path.clone(),
            reasons,
        },
    )?;
    // Re-read and re-hash operator evidence after report validation so any
    // between-check artifact mutation fails closed. Operators must size the
    // approval window to cover both evidence validation rounds plus report I/O.
    validate_operator_evidence(
        loaded,
        block,
        approval_id,
        max_notional_per_order,
        late_unix_seconds,
        initial_unix_seconds,
        approval_consumption_expectation,
    )
    .await?;

    Ok(BoltV3LiveCanaryGateReport {
        approval_id: approval_id.to_string(),
        no_submit_readiness_report_path: report_path,
        max_no_submit_readiness_report_bytes: block.max_no_submit_readiness_report_bytes,
        readiness_report_max_age_seconds: block.readiness_report_max_age_seconds,
        max_live_order_count: block.max_live_order_count,
        max_notional_per_order,
        root_max_notional_per_order,
    })
}

async fn read_report_bytes_with_limit(
    path: &Path,
    max_length: u64,
) -> Result<Vec<u8>, BoltV3LiveCanaryGateError> {
    let file = open_regular_file(path).await.map_err(|source| {
        BoltV3LiveCanaryGateError::ReadinessReportRead {
            path: path.to_path_buf(),
            source,
        }
    })?;
    let mut bytes = Vec::new();
    file.take(max_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::ReadinessReportRead {
            path: path.to_path_buf(),
            source,
        })?;
    let length = bytes.len() as u64;
    if length > max_length {
        return Err(BoltV3LiveCanaryGateError::ReadinessReportTooLarge {
            path: path.to_path_buf(),
            length,
            max_length,
        });
    }
    Ok(bytes)
}

fn resolve_report_path(
    root_path: &Path,
    block: &LiveCanaryBlock,
) -> Result<PathBuf, BoltV3LiveCanaryGateError> {
    validate_configured_path_shape(
        "no_submit_readiness_report_path",
        &block.no_submit_readiness_report_path,
    )?;
    let configured = PathBuf::from(block.no_submit_readiness_report_path.trim());
    if configured.is_absolute() {
        return Ok(configured);
    }
    Ok(root_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&configured))
}

fn parse_positive_decimal(
    field: &'static str,
    value: &str,
) -> Result<Decimal, BoltV3LiveCanaryGateError> {
    let trimmed = value.trim();
    let decimal = Decimal::from_str(trimmed).map_err(|error| {
        BoltV3LiveCanaryGateError::InvalidMaxNotional {
            field,
            value: trimmed.to_string(),
            reason: error.to_string(),
        }
    })?;
    if decimal <= Decimal::ZERO {
        return Err(BoltV3LiveCanaryGateError::InvalidMaxNotional {
            field,
            value: trimmed.to_string(),
            reason: "value must be positive".to_string(),
        });
    }
    Ok(decimal)
}

async fn validate_operator_evidence(
    loaded: &LoadedBoltV3Config,
    block: &LiveCanaryBlock,
    approval_id: &str,
    max_notional_per_order: Decimal,
    approval_window_unix_seconds: u64,
    approval_consumption_freshness_unix_seconds: u64,
    approval_consumption_expectation: ApprovalConsumptionExpectation,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let root_path = &loaded.root_path;
    let evidence = block
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3LiveCanaryGateError::MissingOperatorEvidence)?;

    for (field, value) in required_operator_evidence_fields(evidence) {
        if value.trim().is_empty() {
            return Err(BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field });
        }
    }
    validate_operator_evidence_head_sha(evidence)?;
    if evidence
        .strategy_cancel_path
        .as_ref()
        .is_some_and(|strategy_cancel_path| strategy_cancel_path.trim().is_empty())
    {
        return Err(BoltV3LiveCanaryGateError::MissingOperatorEvidenceField {
            field: "strategy_cancel_path",
        });
    }
    validate_operator_evidence_paths(evidence)?;
    for (field, value) in operator_evidence_hash_fields(evidence) {
        if !is_sha256_hex(value) {
            return Err(BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHashShape { field });
        }
    }
    if evidence.max_operator_evidence_file_bytes == 0 {
        return Err(
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceSizeLimit {
                value: evidence.max_operator_evidence_file_bytes,
            },
        );
    }
    if evidence.approval_consumption_max_age_seconds == 0 {
        return Err(
            BoltV3LiveCanaryGateError::InvalidApprovalConsumptionMaxAge {
                value: evidence.approval_consumption_max_age_seconds,
            },
        );
    }

    if evidence.approval_not_after_unix_seconds <= evidence.approval_not_before_unix_seconds {
        return Err(BoltV3LiveCanaryGateError::InvalidOperatorApprovalWindow {
            approval_not_before_unix_seconds: evidence.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: evidence.approval_not_after_unix_seconds,
        });
    }

    let current = i128::from(approval_window_unix_seconds);
    let not_before = i128::from(evidence.approval_not_before_unix_seconds);
    let not_after = i128::from(evidence.approval_not_after_unix_seconds);
    if current < not_before || current > not_after {
        return Err(BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
            current_unix_seconds: approval_window_unix_seconds,
            approval_not_before_unix_seconds: evidence.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: evidence.approval_not_after_unix_seconds,
        });
    }

    validate_operator_evidence_file_hashes(root_path, evidence).await?;
    let gate_session = validate_operator_gate_session_binding(loaded, root_path, evidence).await?;
    if live_canary_proof_policy_enabled(block) {
        validate_operator_gate_session_freshness(
            &gate_session,
            block.reference_quote_max_age_seconds,
            approval_window_unix_seconds,
        )?;
        validate_operator_canary_proof_order_intent(
            root_path,
            block,
            evidence,
            &gate_session,
            max_notional_per_order,
        )
        .await?;
    } else {
        validate_operator_strategy_input_freshness(
            loaded,
            root_path,
            evidence,
            block.reference_quote_max_age_seconds,
            approval_window_unix_seconds,
        )
        .await?;
        validate_operator_decision_notional_within_canary_cap(
            root_path,
            evidence,
            max_notional_per_order,
        )?;
    }
    validate_operator_approval_envelope(root_path, evidence, approval_id).await?;
    validate_operator_approval_consumption(
        root_path,
        evidence,
        approval_id,
        approval_window_unix_seconds,
        approval_consumption_freshness_unix_seconds,
        approval_consumption_expectation,
    )
    .await?;
    Ok(())
}

fn live_canary_proof_policy_enabled(block: &LiveCanaryBlock) -> bool {
    block
        .proof_policy
        .as_ref()
        .is_some_and(|proof_policy| proof_policy.enabled)
}

fn validate_operator_decision_notional_within_canary_cap(
    root_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    max_notional_per_order: Decimal,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let path = resolve_configured_path(
        root_path,
        "decision_evidence_path",
        &evidence.decision_evidence_path,
    )?;
    let chain =
        read_latest_entry_decision_evidence_chain(&path, evidence.max_operator_evidence_file_bytes)
            .map_err(
                |source| BoltV3LiveCanaryGateError::OperatorDecisionEvidenceInvalid {
                    path: path.clone(),
                    reason: source.to_string(),
                },
            )?;
    let notional = Decimal::from_str(chain.admission.notional.trim()).map_err(|source| {
        BoltV3LiveCanaryGateError::OperatorDecisionEvidenceInvalid {
            path: path.clone(),
            reason: format!("entry order notional is not a decimal: {source}"),
        }
    })?;
    if notional <= Decimal::ZERO {
        return Err(BoltV3LiveCanaryGateError::OperatorDecisionEvidenceInvalid {
            path,
            reason: "entry order notional must be positive".to_string(),
        });
    }
    if notional > max_notional_per_order {
        return Err(BoltV3LiveCanaryGateError::OperatorDecisionEvidenceInvalid {
            path,
            reason: format!(
                "source-owned entry order notional {notional} exceeds [live_canary].max_notional_per_order={max_notional_per_order}"
            ),
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct OperatorStrategyInputFreshness {
    reference_quote_ts_event: Option<u64>,
}

async fn validate_operator_strategy_input_freshness(
    loaded: &LoadedBoltV3Config,
    root_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    reference_quote_max_age_seconds: u64,
    current_unix_seconds: u64,
) -> Result<(), BoltV3LiveCanaryGateError> {
    if !loaded_has_source_owned_decision_reference(loaded) {
        return Ok(());
    }
    let path = resolve_configured_path(
        root_path,
        "strategy_input_evidence_path",
        &evidence.strategy_input_evidence_path,
    )?;
    let bytes = read_regular_file_bounded(&path, evidence.max_operator_evidence_file_bytes)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::OperatorEvidenceRead {
            field: "strategy_input_evidence_sha256",
            path: path.clone(),
            source,
        })?;
    let input: OperatorStrategyInputFreshness =
        serde_json::from_slice(&bytes).map_err(|source| {
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceParse {
                path: path.clone(),
                source,
            }
        })?;
    let Some(reference_quote_ts_event) = input.reference_quote_ts_event else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "reference_quote_ts_event is missing".to_string(),
            },
        );
    };
    if reference_quote_ts_event == 0 {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "reference_quote_ts_event is invalid".to_string(),
            },
        );
    }
    let Some(current_unix_ms) = current_unix_seconds.checked_mul(MILLIS_PER_SECOND_U64) else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "current_unix_seconds is invalid: overflows milliseconds".to_string(),
            },
        );
    };
    if reference_quote_ts_event > current_unix_ms {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: format!(
                    "reference_quote_ts_event is invalid: {reference_quote_ts_event} is in the future relative to current_unix_ms {current_unix_ms}"
                ),
            },
        );
    }
    let Some(max_age_ms) = reference_quote_max_age_seconds.checked_mul(MILLIS_PER_SECOND_U64)
    else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: format!(
                    "reference_quote_max_age_seconds is invalid: {reference_quote_max_age_seconds} overflows milliseconds"
                ),
            },
        );
    };
    let age_ms = current_unix_ms.saturating_sub(reference_quote_ts_event);
    if age_ms > max_age_ms {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: format!(
                    "reference_quote_ts_event is invalid: age_ms={age_ms} exceeds [live_canary].reference_quote_max_age_seconds={reference_quote_max_age_seconds}"
                ),
            },
        );
    }
    Ok(())
}

fn loaded_has_source_owned_decision_reference(loaded: &LoadedBoltV3Config) -> bool {
    loaded.strategies.iter().any(|strategy| {
        strategy
            .config
            .target
            .as_table()
            .and_then(|target| target.get("gate_subscriptions"))
            .and_then(toml::Value::as_table)
            .is_some_and(|subscriptions| subscriptions.contains_key(DECISION_REFERENCE_GATE_ROLE))
    })
}

async fn validate_operator_gate_session_binding(
    loaded: &LoadedBoltV3Config,
    root_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<EntryReadinessGateSession, BoltV3LiveCanaryGateError> {
    let gate_session_path = required_optional_operator_evidence_field(
        "gate_session_path",
        evidence.gate_session_path.as_deref(),
    )?;
    let expected_gate_session_sha256 = required_optional_operator_evidence_field(
        "expected_gate_session_sha256",
        evidence.expected_gate_session_sha256.as_deref(),
    )?;
    if !is_sha256_hex(expected_gate_session_sha256) {
        return Err(
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHashShape {
                field: "expected_gate_session_sha256",
            },
        );
    }

    let path = resolve_configured_path(root_path, "gate_session_path", gate_session_path)?;
    let bytes = read_regular_file_bounded(&path, evidence.max_operator_evidence_file_bytes)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::OperatorEvidenceRead {
            field: "gate_session_path",
            path: path.clone(),
            source,
        })?;
    if sha256_hex(&bytes) != expected_gate_session_sha256 {
        return Err(BoltV3LiveCanaryGateError::OperatorEvidenceHashMismatch {
            field: "expected_gate_session_sha256",
            path,
        });
    }
    let session: EntryReadinessGateSession = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3LiveCanaryGateError::OperatorGateSessionParse {
            path: path.clone(),
            source,
        }
    })?;
    validate_live_canary_gate_session(loaded, &session).map_err(|reason| {
        BoltV3LiveCanaryGateError::OperatorGateSessionInvalid {
            path: path.clone(),
            reason,
        }
    })?;
    Ok(session)
}

fn validate_operator_gate_session_freshness(
    session: &EntryReadinessGateSession,
    reference_quote_max_age_seconds: u64,
    current_unix_seconds: u64,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let Some(current_unix_ms) = current_unix_seconds.checked_mul(MILLIS_PER_SECOND_U64) else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path: PathBuf::from("gate_session_path"),
                reason: "current_unix_seconds is invalid: overflows milliseconds".to_string(),
            },
        );
    };
    if session.created_at_ms > current_unix_ms {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path: PathBuf::from("gate_session_path"),
                reason: format!(
                    "gate session created_at_ms is invalid: {} is in the future relative to current_unix_ms {current_unix_ms}",
                    session.created_at_ms
                ),
            },
        );
    }
    let Some(max_age_ms) = reference_quote_max_age_seconds.checked_mul(MILLIS_PER_SECOND_U64)
    else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path: PathBuf::from("gate_session_path"),
                reason: format!(
                    "reference_quote_max_age_seconds is invalid: {reference_quote_max_age_seconds} overflows milliseconds"
                ),
            },
        );
    };
    let age_ms = current_unix_ms.saturating_sub(session.created_at_ms);
    if age_ms > max_age_ms {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path: PathBuf::from("gate_session_path"),
                reason: format!(
                    "gate session created_at_ms is invalid: age_ms={age_ms} exceeds [live_canary].reference_quote_max_age_seconds={reference_quote_max_age_seconds}"
                ),
            },
        );
    }
    Ok(())
}

async fn validate_operator_canary_proof_order_intent(
    root_path: &Path,
    block: &LiveCanaryBlock,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    gate_session: &EntryReadinessGateSession,
    max_notional_per_order: Decimal,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let proof_policy = block.proof_policy.as_ref().ok_or(
        BoltV3LiveCanaryGateError::MissingOperatorEvidenceField {
            field: "live_canary.proof_policy",
        },
    )?;
    let path_value = required_optional_operator_evidence_field(
        "canary_proof_order_intent_path",
        evidence.canary_proof_order_intent_path.as_deref(),
    )?;
    let expected_sha256 = required_optional_operator_evidence_field(
        "canary_proof_order_intent_sha256",
        evidence.canary_proof_order_intent_sha256.as_deref(),
    )?;
    validate_configured_path_shape("canary_proof_order_intent_path", path_value)?;
    if !is_sha256_hex(expected_sha256) {
        return Err(
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHashShape {
                field: "canary_proof_order_intent_sha256",
            },
        );
    }
    let path = resolve_configured_path(root_path, "canary_proof_order_intent_path", path_value)?;
    let bytes = read_regular_file_bounded(&path, evidence.max_operator_evidence_file_bytes)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::OperatorEvidenceRead {
            field: "canary_proof_order_intent_sha256",
            path: path.clone(),
            source,
        })?;
    if sha256_hex(&bytes) != expected_sha256 {
        return Err(BoltV3LiveCanaryGateError::OperatorEvidenceHashMismatch {
            field: "canary_proof_order_intent_sha256",
            path,
        });
    }
    let order_intent: CanaryProofOrderIntentArtifact =
        serde_json::from_slice(&bytes).map_err(|source| {
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceParse {
                path: path.clone(),
                source,
            }
        })?;
    if order_intent.proof_claim != CANARY_PROOF_CLAIM
        || order_intent.proof_claim != proof_policy.proof_claim
    {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "canary proof order intent proof_claim is invalid".to_string(),
            },
        );
    }
    if order_intent.strategy_instance_id != gate_session.strategy_instance_id
        || order_intent.strategy_instance_id != proof_policy.strategy_instance_id
    {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "canary proof order intent strategy_instance_id is invalid".to_string(),
            },
        );
    }
    if order_intent.execution_client_id != proof_policy.execution_client_id {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "canary proof order intent execution_client_id is invalid".to_string(),
            },
        );
    }
    if !gate_session
        .selected_market
        .instrument_ids
        .contains(&order_intent.instrument_id)
    {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "canary proof order intent instrument_id is outside selected market"
                    .to_string(),
            },
        );
    }
    if !order_intent
        .source_refs
        .contains(&gate_session.session_hash)
    {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "canary proof order intent source_refs does not bind gate session"
                    .to_string(),
            },
        );
    }
    if order_intent.notional <= Decimal::ZERO || order_intent.quantity <= Decimal::ZERO {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: "canary proof order intent notional and quantity must be positive"
                    .to_string(),
            },
        );
    }
    if order_intent.notional > max_notional_per_order {
        return Err(
            BoltV3LiveCanaryGateError::OperatorStrategyInputEvidenceInvalid {
                path,
                reason: format!(
                    "canary proof order intent notional {} exceeds [live_canary].max_notional_per_order={max_notional_per_order}",
                    order_intent.notional
                ),
            },
        );
    }
    Ok(())
}

fn required_optional_operator_evidence_field<'a>(
    field: &'static str,
    value: Option<&'a str>,
) -> Result<&'a str, BoltV3LiveCanaryGateError> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field })
}

fn validate_live_canary_gate_session(
    loaded: &LoadedBoltV3Config,
    session: &EntryReadinessGateSession,
) -> Result<(), String> {
    let snapshot = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(session);
    validate_readiness_gate_evidence_snapshot(&snapshot).map_err(|error| error.to_string())?;

    let strategy = loaded
        .strategies
        .iter()
        .find(|strategy| strategy.config.strategy_instance_id == session.strategy_instance_id)
        .ok_or_else(|| {
            "gate session strategy_instance_id does not match loaded config".to_string()
        })?;
    let target = strategy
        .config
        .target
        .as_table()
        .ok_or_else(|| "gate session strategy target is not a table".to_string())?;
    let configured_target_id = target
        .get("configured_target_id")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| {
            "gate session strategy target configured_target_id is missing".to_string()
        })?;
    if configured_target_id != session.configured_target_id
        || configured_target_id != session.selected_market.configured_target_id
    {
        return Err(
            "gate session configured_target_id does not match loaded strategy target".to_string(),
        );
    }
    Ok(())
}

fn validate_operator_evidence_head_sha(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3LiveCanaryGateError> {
    if !is_git_head_sha(&evidence.head_sha) {
        return Err(
            BoltV3LiveCanaryGateError::InvalidOperatorEvidenceHeadShaShape { field: "head_sha" },
        );
    }

    let build_head_sha =
        current_build_head_sha().ok_or(BoltV3LiveCanaryGateError::BuildHeadShaUnavailable)?;
    if evidence.head_sha != build_head_sha {
        return Err(BoltV3LiveCanaryGateError::OperatorEvidenceHeadShaMismatch {
            expected: build_head_sha,
            actual: evidence.head_sha.clone(),
        });
    }

    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Phase8OperatorApprovalEnvelopeFile {
    pub schema_version: i64,
    pub record_kind: String,
    pub head_sha: String,
    pub ssm_manifest_sha256: String,
    pub strategy_input_evidence_sha256: String,
    pub financial_envelope_sha256: String,
    pub pre_run_state_sha256: String,
    pub abort_plan_sha256: String,
    pub approval_id_hash: String,
    pub approval_nonce_sha256: String,
    pub approval_not_before_unix_secs: i64,
    pub approval_not_after_unix_secs: i64,
    pub canary_evidence_path_hash: String,
    pub strategy_cancel_path_hash: Option<String>,
}

async fn validate_operator_evidence_file_hashes(
    root_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3LiveCanaryGateError> {
    for binding in operator_evidence_file_hash_bindings(evidence) {
        let path = resolve_configured_path(root_path, binding.path_field, binding.path)?;
        let actual = sha256_file(
            &path,
            binding.hash_field,
            evidence.max_operator_evidence_file_bytes,
        )
        .await?;
        if actual != binding.expected_sha256 {
            return Err(BoltV3LiveCanaryGateError::OperatorEvidenceHashMismatch {
                field: binding.hash_field,
                path,
            });
        }
    }
    Ok(())
}

async fn validate_operator_approval_envelope(
    root_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let path = resolve_configured_path(
        root_path,
        "approval_envelope_path",
        &evidence.approval_envelope_path,
    )?;
    let bytes = read_regular_file_bounded(&path, evidence.max_operator_evidence_file_bytes)
        .await
        .map_err(
            |source| BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeRead {
                path: path.clone(),
                source,
            },
        )?;
    let envelope: Phase8OperatorApprovalEnvelopeFile =
        serde_json::from_slice(&bytes).map_err(|source| {
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeParse {
                path: path.clone(),
                source,
            }
        })?;

    validate_approval_envelope_i64_field(
        &path,
        "schema_version",
        envelope.schema_version,
        APPROVAL_ENVELOPE_SCHEMA_VERSION,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "record_kind",
        &envelope.record_kind,
        APPROVAL_ENVELOPE_RECORD_KIND,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "head_sha",
        &envelope.head_sha,
        &evidence.head_sha,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "ssm_manifest_sha256",
        &envelope.ssm_manifest_sha256,
        &evidence.ssm_manifest_sha256,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "strategy_input_evidence_sha256",
        &envelope.strategy_input_evidence_sha256,
        &evidence.strategy_input_evidence_sha256,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "financial_envelope_sha256",
        &envelope.financial_envelope_sha256,
        &evidence.financial_envelope_sha256,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "pre_run_state_sha256",
        &envelope.pre_run_state_sha256,
        &evidence.pre_run_state_sha256,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "abort_plan_sha256",
        &envelope.abort_plan_sha256,
        &evidence.abort_plan_sha256,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "approval_id_hash",
        &envelope.approval_id_hash,
        &sha256_hex(approval_id.as_bytes()),
    )?;
    validate_approval_envelope_string_field(
        &path,
        "approval_nonce_sha256",
        &envelope.approval_nonce_sha256,
        &evidence.approval_nonce_sha256,
    )?;
    validate_approval_envelope_string_field(
        &path,
        "canary_evidence_path_hash",
        &envelope.canary_evidence_path_hash,
        &sha256_hex(evidence.canary_evidence_path.as_bytes()),
    )?;
    validate_approval_envelope_i64_field(
        &path,
        "approval_not_before_unix_secs",
        envelope.approval_not_before_unix_secs,
        evidence.approval_not_before_unix_seconds,
    )?;
    validate_approval_envelope_i64_field(
        &path,
        "approval_not_after_unix_secs",
        envelope.approval_not_after_unix_secs,
        evidence.approval_not_after_unix_seconds,
    )?;

    match (
        &evidence.strategy_cancel_path,
        &envelope.strategy_cancel_path_hash,
    ) {
        (Some(strategy_cancel_path), Some(actual)) => validate_approval_envelope_string_field(
            &path,
            "strategy_cancel_path_hash",
            actual,
            &sha256_hex(strategy_cancel_path.as_bytes()),
        )?,
        (Some(_), None) | (None, Some(_)) => {
            return Err(
                BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch {
                    path,
                    field: "strategy_cancel_path_hash",
                },
            );
        }
        (None, None) => {}
    }

    Ok(())
}

fn validate_approval_envelope_string_field(
    path: &Path,
    field: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), BoltV3LiveCanaryGateError> {
    if actual != expected {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch {
                path: path.to_path_buf(),
                field,
            },
        );
    }
    Ok(())
}

fn validate_approval_envelope_i64_field(
    path: &Path,
    field: &'static str,
    actual: i64,
    expected: i64,
) -> Result<(), BoltV3LiveCanaryGateError> {
    if actual != expected {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalEnvelopeMismatch {
                path: path.to_path_buf(),
                field,
            },
        );
    }
    Ok(())
}

async fn validate_operator_approval_consumption(
    root_path: &Path,
    evidence: &LiveCanaryOperatorEvidenceBlock,
    approval_id: &str,
    approval_window_unix_seconds: u64,
    approval_consumption_freshness_unix_seconds: u64,
    approval_consumption_expectation: ApprovalConsumptionExpectation,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let path = resolve_configured_path(
        root_path,
        "approval_consumption_path",
        &evidence.approval_consumption_path,
    )?;
    let bytes =
        match read_regular_file_bounded(&path, evidence.max_operator_evidence_file_bytes).await {
            Ok(bytes) => bytes,
            Err(source)
                if approval_consumption_expectation
                    == ApprovalConsumptionExpectation::DeferredUntilLiveRunnerEntry
                    && source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(source) => {
                return Err(BoltV3LiveCanaryGateError::OperatorApprovalConsumptionRead {
                    path: path.clone(),
                    source,
                });
            }
        };
    if approval_consumption_expectation
        == ApprovalConsumptionExpectation::DeferredUntilLiveRunnerEntry
    {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionAlreadyExistsBeforeRunner {
                path,
            },
        );
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|source| {
        BoltV3LiveCanaryGateError::OperatorApprovalConsumptionParse {
            path: path.clone(),
            source,
        }
    })?;
    let object = value.as_object().ok_or_else(|| {
        BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed {
            path: path.clone(),
            reason: "proof must be a JSON object".to_string(),
        }
    })?;

    validate_consumption_i64_field(
        &path,
        object,
        "schema_version",
        APPROVAL_CONSUMPTION_SCHEMA_VERSION,
    )?;
    validate_consumption_string_field(
        &path,
        object,
        "record_kind",
        APPROVAL_CONSUMPTION_RECORD_KIND,
    )?;
    let approval_id_hash = sha256_hex(approval_id.as_bytes());
    let canary_evidence_path_hash = sha256_hex(evidence.canary_evidence_path.as_bytes());
    let root_toml_sha256 = root_toml_sha256(root_path).await?;
    for (field, expected) in [
        ("head_sha", evidence.head_sha.as_str()),
        ("root_toml_sha256", root_toml_sha256.as_str()),
        (
            "approval_envelope_sha256",
            evidence.approval_envelope_sha256.as_str(),
        ),
        ("ssm_manifest_sha256", evidence.ssm_manifest_sha256.as_str()),
        (
            "strategy_input_evidence_sha256",
            evidence.strategy_input_evidence_sha256.as_str(),
        ),
        (
            "financial_envelope_sha256",
            evidence.financial_envelope_sha256.as_str(),
        ),
        (
            "pre_run_state_sha256",
            evidence.pre_run_state_sha256.as_str(),
        ),
        ("abort_plan_sha256", evidence.abort_plan_sha256.as_str()),
        (
            "approval_nonce_sha256",
            evidence.approval_nonce_sha256.as_str(),
        ),
        ("approval_id_hash", approval_id_hash.as_str()),
        (
            "canary_evidence_path_hash",
            canary_evidence_path_hash.as_str(),
        ),
    ] {
        validate_consumption_string_field(&path, object, field, expected)?;
    }
    if let Some(strategy_cancel_path) = &evidence.strategy_cancel_path {
        let strategy_cancel_path_hash = sha256_hex(strategy_cancel_path.as_bytes());
        validate_consumption_string_field(
            &path,
            object,
            "strategy_cancel_path_hash",
            &strategy_cancel_path_hash,
        )?;
    }
    validate_consumption_i64_field(
        &path,
        object,
        "approval_not_before_unix_secs",
        evidence.approval_not_before_unix_seconds,
    )?;
    validate_consumption_i64_field(
        &path,
        object,
        "approval_not_after_unix_secs",
        evidence.approval_not_after_unix_seconds,
    )?;
    let consumed_unix_secs = consumption_i64_field(&path, object, "consumed_unix_secs")?;
    let consumed = i128::from(consumed_unix_secs);
    let consumption_freshness_current = i128::from(approval_consumption_freshness_unix_seconds);
    let not_before = i128::from(evidence.approval_not_before_unix_seconds);
    let not_after = i128::from(evidence.approval_not_after_unix_seconds);
    let max_age = i128::from(evidence.approval_consumption_max_age_seconds);
    if consumed < not_before
        || consumed > not_after
        || consumed > consumption_freshness_current
        || consumption_freshness_current - consumed > max_age
    {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionStale {
                path,
                consumed_unix_secs,
                approval_not_before_unix_seconds: evidence.approval_not_before_unix_seconds,
                approval_not_after_unix_seconds: evidence.approval_not_after_unix_seconds,
                current_unix_seconds: approval_consumption_freshness_unix_seconds,
                approval_consumption_max_age_seconds: evidence.approval_consumption_max_age_seconds,
            },
        );
    }

    let approval_window_current = i128::from(approval_window_unix_seconds);
    // Standalone guard for direct approval-consumption validation callers.
    // The normal gate path checks the same window before delegation.
    if approval_window_current < not_before || approval_window_current > not_after {
        return Err(BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
            current_unix_seconds: approval_window_unix_seconds,
            approval_not_before_unix_seconds: evidence.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: evidence.approval_not_after_unix_seconds,
        });
    }

    Ok(())
}

fn validate_consumption_string_field(
    path: &Path,
    object: &Map<String, Value>,
    field: &'static str,
    expected: &str,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let Some(value) = object.get(field) else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed {
                path: path.to_path_buf(),
                reason: format!("field `{field}` is missing"),
            },
        );
    };
    let Some(actual) = value.as_str() else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed {
                path: path.to_path_buf(),
                reason: format!("field `{field}` must be a string"),
            },
        );
    };
    if actual != expected {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch {
                path: path.to_path_buf(),
                field,
            },
        );
    }
    Ok(())
}

fn validate_consumption_i64_field(
    path: &Path,
    object: &Map<String, Value>,
    field: &'static str,
    expected: i64,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let actual = consumption_i64_field(path, object, field)?;
    if actual != expected {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMismatch {
                path: path.to_path_buf(),
                field,
            },
        );
    }
    Ok(())
}

fn consumption_i64_field(
    path: &Path,
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<i64, BoltV3LiveCanaryGateError> {
    let Some(value) = object.get(field) else {
        return Err(
            BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed {
                path: path.to_path_buf(),
                reason: format!("field `{field}` is missing"),
            },
        );
    };
    value.as_i64().ok_or_else(
        || BoltV3LiveCanaryGateError::OperatorApprovalConsumptionMalformed {
            path: path.to_path_buf(),
            reason: format!("field `{field}` must be an integer"),
        },
    )
}

async fn sha256_file(
    path: &Path,
    field: &'static str,
    max_bytes: u64,
) -> Result<String, BoltV3LiveCanaryGateError> {
    let mut file = open_regular_file_bounded(path, max_bytes)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::OperatorEvidenceRead {
            field,
            path: path.to_path_buf(),
            source,
        })?;
    let bytes = read_to_vec_with_cap(&mut file, max_bytes)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::OperatorEvidenceRead {
            field,
            path: path.to_path_buf(),
            source,
        })?;
    Ok(sha256_hex(&bytes))
}

async fn read_regular_file_bounded(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut file = open_regular_file_bounded(path, max_bytes).await?;
    read_to_vec_with_cap(&mut file, max_bytes).await
}

async fn open_regular_file_bounded(
    path: &Path,
    max_bytes: u64,
) -> std::io::Result<tokio::fs::File> {
    let file = open_regular_file(path).await?;
    let opened_metadata = file.metadata().await?;
    validate_regular_file_metadata(path, &opened_metadata, max_bytes)?;
    Ok(file)
}

async fn open_regular_file(path: &Path) -> std::io::Result<tokio::fs::File> {
    let pre_open_metadata = tokio::fs::symlink_metadata(path).await?;
    validate_regular_file_type(path, &pre_open_metadata)?;
    let file = tokio::fs::File::open(path).await?;
    let opened_metadata = file.metadata().await?;
    validate_regular_file_type(path, &opened_metadata)?;
    let post_open_path_metadata = tokio::fs::symlink_metadata(path).await?;
    validate_regular_file_type(path, &post_open_path_metadata)?;
    Ok(file)
}

async fn read_to_vec_with_cap<R>(mut reader: R, max_bytes: u64) -> std::io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::new();
    let mut observed = 0_u64;
    let mut buffer = [0_u8; 8192];
    loop {
        let remaining_to_detect = max_bytes.saturating_sub(observed).saturating_add(1);
        let read_len = if remaining_to_detect > buffer.len() as u64 {
            buffer.len()
        } else {
            remaining_to_detect as usize
        };
        let length = reader.read(&mut buffer[..read_len]).await?;
        if length == 0 {
            break;
        }
        observed = observed.saturating_add(length as u64);
        if observed > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("read exceeds cap of {max_bytes} bytes"),
            ));
        }
        bytes.extend_from_slice(&buffer[..length]);
    }
    Ok(bytes)
}

fn validate_regular_file_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    max_bytes: u64,
) -> std::io::Result<()> {
    validate_regular_file_type(path, metadata)?;
    let length = metadata.len();
    if length > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} exceeds max_operator_evidence_file_bytes={max_bytes} bytes (length={length})",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn validate_regular_file_type(path: &Path, metadata: &std::fs::Metadata) -> std::io::Result<()> {
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a regular file", path.display()),
        ));
    }
    Ok(())
}

async fn root_toml_sha256(root_path: &Path) -> Result<String, BoltV3LiveCanaryGateError> {
    let root_text = crate::bounded_config_read::read_to_string_async(root_path)
        .await
        .map_err(|source| BoltV3LiveCanaryGateError::RootTomlRead {
            path: root_path.to_path_buf(),
            source: Box::new(source),
        })?;
    Ok(sha256_hex(root_text.as_bytes()))
}

fn required_operator_evidence_fields(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> [(&'static str, &str); 22] {
    [
        ("head_sha", &evidence.head_sha),
        ("approval_envelope_path", &evidence.approval_envelope_path),
        (
            "approval_envelope_sha256",
            &evidence.approval_envelope_sha256,
        ),
        ("ssm_manifest_path", &evidence.ssm_manifest_path),
        ("ssm_manifest_sha256", &evidence.ssm_manifest_sha256),
        (
            "strategy_input_evidence_path",
            &evidence.strategy_input_evidence_path,
        ),
        (
            "strategy_input_evidence_sha256",
            &evidence.strategy_input_evidence_sha256,
        ),
        ("financial_envelope_path", &evidence.financial_envelope_path),
        (
            "financial_envelope_sha256",
            &evidence.financial_envelope_sha256,
        ),
        ("pre_run_state_path", &evidence.pre_run_state_path),
        ("pre_run_state_sha256", &evidence.pre_run_state_sha256),
        ("abort_plan_path", &evidence.abort_plan_path),
        ("abort_plan_sha256", &evidence.abort_plan_sha256),
        ("canary_evidence_path", &evidence.canary_evidence_path),
        ("approval_nonce_path", &evidence.approval_nonce_path),
        ("approval_nonce_sha256", &evidence.approval_nonce_sha256),
        (
            "approval_consumption_path",
            &evidence.approval_consumption_path,
        ),
        ("decision_evidence_path", &evidence.decision_evidence_path),
        ("nt_submit_event_path", &evidence.nt_submit_event_path),
        ("venue_order_state_path", &evidence.venue_order_state_path),
        (
            "restart_reconciliation_path",
            &evidence.restart_reconciliation_path,
        ),
        ("post_run_hygiene_path", &evidence.post_run_hygiene_path),
    ]
}

fn validate_operator_evidence_paths(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> Result<(), BoltV3LiveCanaryGateError> {
    for (field, value) in operator_evidence_path_fields(evidence) {
        validate_configured_path_shape(field, value)?;
    }
    if let Some(strategy_cancel_path) = evidence.strategy_cancel_path.as_deref() {
        validate_configured_path_shape("strategy_cancel_path", strategy_cancel_path)?;
    }
    Ok(())
}

fn operator_evidence_path_fields(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> [(&'static str, &str); 14] {
    [
        ("approval_envelope_path", &evidence.approval_envelope_path),
        ("ssm_manifest_path", &evidence.ssm_manifest_path),
        (
            "strategy_input_evidence_path",
            &evidence.strategy_input_evidence_path,
        ),
        ("financial_envelope_path", &evidence.financial_envelope_path),
        ("pre_run_state_path", &evidence.pre_run_state_path),
        ("abort_plan_path", &evidence.abort_plan_path),
        ("canary_evidence_path", &evidence.canary_evidence_path),
        ("approval_nonce_path", &evidence.approval_nonce_path),
        (
            "approval_consumption_path",
            &evidence.approval_consumption_path,
        ),
        ("decision_evidence_path", &evidence.decision_evidence_path),
        ("nt_submit_event_path", &evidence.nt_submit_event_path),
        ("venue_order_state_path", &evidence.venue_order_state_path),
        (
            "restart_reconciliation_path",
            &evidence.restart_reconciliation_path,
        ),
        ("post_run_hygiene_path", &evidence.post_run_hygiene_path),
    ]
}

fn operator_evidence_hash_fields(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> [(&'static str, &str); 7] {
    [
        (
            "approval_envelope_sha256",
            &evidence.approval_envelope_sha256,
        ),
        ("ssm_manifest_sha256", &evidence.ssm_manifest_sha256),
        (
            "strategy_input_evidence_sha256",
            &evidence.strategy_input_evidence_sha256,
        ),
        (
            "financial_envelope_sha256",
            &evidence.financial_envelope_sha256,
        ),
        ("pre_run_state_sha256", &evidence.pre_run_state_sha256),
        ("abort_plan_sha256", &evidence.abort_plan_sha256),
        ("approval_nonce_sha256", &evidence.approval_nonce_sha256),
    ]
}

struct OperatorEvidenceFileHashBinding<'a> {
    path_field: &'static str,
    path: &'a str,
    hash_field: &'static str,
    expected_sha256: &'a str,
}

fn operator_evidence_file_hash_bindings(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> [OperatorEvidenceFileHashBinding<'_>; 7] {
    [
        OperatorEvidenceFileHashBinding {
            path_field: "approval_envelope_path",
            path: &evidence.approval_envelope_path,
            hash_field: "approval_envelope_sha256",
            expected_sha256: &evidence.approval_envelope_sha256,
        },
        OperatorEvidenceFileHashBinding {
            path_field: "ssm_manifest_path",
            path: &evidence.ssm_manifest_path,
            hash_field: "ssm_manifest_sha256",
            expected_sha256: &evidence.ssm_manifest_sha256,
        },
        OperatorEvidenceFileHashBinding {
            path_field: "strategy_input_evidence_path",
            path: &evidence.strategy_input_evidence_path,
            hash_field: "strategy_input_evidence_sha256",
            expected_sha256: &evidence.strategy_input_evidence_sha256,
        },
        OperatorEvidenceFileHashBinding {
            path_field: "financial_envelope_path",
            path: &evidence.financial_envelope_path,
            hash_field: "financial_envelope_sha256",
            expected_sha256: &evidence.financial_envelope_sha256,
        },
        OperatorEvidenceFileHashBinding {
            path_field: "pre_run_state_path",
            path: &evidence.pre_run_state_path,
            hash_field: "pre_run_state_sha256",
            expected_sha256: &evidence.pre_run_state_sha256,
        },
        OperatorEvidenceFileHashBinding {
            path_field: "abort_plan_path",
            path: &evidence.abort_plan_path,
            hash_field: "abort_plan_sha256",
            expected_sha256: &evidence.abort_plan_sha256,
        },
        OperatorEvidenceFileHashBinding {
            path_field: "approval_nonce_path",
            path: &evidence.approval_nonce_path,
            hash_field: "approval_nonce_sha256",
            expected_sha256: &evidence.approval_nonce_sha256,
        },
    ]
}

fn resolve_configured_path(
    root_path: &Path,
    field: &'static str,
    configured: &str,
) -> Result<PathBuf, BoltV3LiveCanaryGateError> {
    validate_configured_path_shape(field, configured)?;
    let path = PathBuf::from(configured.trim());
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(root_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(path))
}

fn validate_configured_path_shape(
    field: &'static str,
    configured: &str,
) -> Result<(), BoltV3LiveCanaryGateError> {
    if Path::new(configured.trim())
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(BoltV3LiveCanaryGateError::InvalidConfiguredPath {
            field,
            value: configured.to_string(),
        });
    }
    Ok(())
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn current_build_head_sha() -> Option<&'static str> {
    option_env!("BOLT_V3_BUILD_HEAD_SHA").filter(|value| is_git_head_sha(value))
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_no_submit_readiness_report(
    report: &Map<String, Value>,
    expected_approval_id_hash: &str,
    expected_executable_identity: &str,
    expected_config_bundle_checksum: &str,
    readiness_report_max_age_seconds: u64,
    current_unix_seconds: u64,
) -> Result<(), Vec<String>> {
    let mut reasons = Vec::new();
    validate_linkage_field(
        &mut reasons,
        report,
        APPROVAL_ID_HASH_KEY,
        expected_approval_id_hash,
    );
    validate_linkage_field(
        &mut reasons,
        report,
        EXECUTABLE_IDENTITY_KEY,
        expected_executable_identity,
    );
    validate_linkage_field(
        &mut reasons,
        report,
        CONFIG_BUNDLE_CHECKSUM_KEY,
        expected_config_bundle_checksum,
    );
    validate_report_generated_at(
        &mut reasons,
        report,
        readiness_report_max_age_seconds,
        current_unix_seconds,
    );
    match report.get(STAGES_KEY) {
        None => reasons.push("stages array is missing".to_string()),
        Some(stages_value) => match stages_value.as_array() {
            None => reasons.push(format!("stages must be an array, got {stages_value}")),
            Some(stages) if stages.is_empty() => reasons.push("stages array is empty".to_string()),
            Some(stages) => {
                let mut present_stage_names = std::collections::BTreeSet::new();
                for stage in stages {
                    let name = stage
                        .get(STAGE_KEY)
                        .and_then(Value::as_str)
                        .unwrap_or("<unnamed>");
                    present_stage_names.insert(name.to_string());
                    let status = stage.get(STATUS_KEY).and_then(Value::as_str);
                    if !matches_satisfied_status(status) {
                        reasons.push(format!(
                            "stage `{name}` status is `{}`",
                            status.unwrap_or("<missing>")
                        ));
                    }
                }
                for required_stage in REQUIRED_NO_SUBMIT_READINESS_STAGES {
                    if !present_stage_names.contains(*required_stage) {
                        reasons.push(format!("required stage `{required_stage}` is missing"));
                    }
                }
            }
        },
    }

    if reasons.is_empty() {
        Ok(())
    } else {
        Err(reasons)
    }
}

fn validate_report_generated_at(
    reasons: &mut Vec<String>,
    report: &Map<String, Value>,
    readiness_report_max_age_seconds: u64,
    current_unix_seconds: u64,
) {
    let Some(value) = report.get(GENERATED_AT_UNIX_SECONDS_KEY) else {
        reasons.push(format!("{GENERATED_AT_UNIX_SECONDS_KEY} is missing"));
        return;
    };
    let Some(generated_at_unix_seconds) = value.as_u64() else {
        reasons.push(format!(
            "{GENERATED_AT_UNIX_SECONDS_KEY} must be an unsigned integer (got {})",
            report_field_kind(value)
        ));
        return;
    };
    let Some(age_seconds) = current_unix_seconds.checked_sub(generated_at_unix_seconds) else {
        reasons.push(format!(
            "{GENERATED_AT_UNIX_SECONDS_KEY} is in the future ({generated_at_unix_seconds} > {current_unix_seconds})"
        ));
        return;
    };
    if age_seconds > readiness_report_max_age_seconds {
        reasons.push(format!(
            "{GENERATED_AT_UNIX_SECONDS_KEY} expired: age_seconds={age_seconds} exceeds \
             [live_canary].readiness_report_max_age_seconds={readiness_report_max_age_seconds}"
        ));
    }
}

fn validate_linkage_field(
    reasons: &mut Vec<String>,
    report: &Map<String, Value>,
    field: &'static str,
    expected: &str,
) {
    let Some(value) = report.get(field) else {
        reasons.push(format!("linkage field `{field}` is missing"));
        return;
    };
    let Some(actual) = value.as_str() else {
        reasons.push(format!(
            "linkage field `{field}` is not a string (got {})",
            report_field_kind(value)
        ));
        return;
    };
    if actual.trim().is_empty() {
        reasons.push(format!("linkage field `{field}` is empty"));
        return;
    }
    if actual != expected {
        reasons.push(format!(
            "linkage field `{field}` does not match expected value"
        ));
    }
}

fn report_field_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn matches_satisfied_status(status: Option<&str>) -> bool {
    matches!(status, Some(value) if value.eq_ignore_ascii_case(STATUS_SATISFIED))
}

async fn executable_identity() -> Result<String, BoltV3LiveCanaryGateError> {
    let path = std::env::current_exe()
        .map_err(|source| BoltV3LiveCanaryGateError::CurrentExecutablePath { source })?;
    let bytes = tokio::fs::read(&path).await.map_err(|source| {
        BoltV3LiveCanaryGateError::ExecutableIdentityRead {
            path: path.clone(),
            source,
        }
    })?;
    Ok(sha256_hex(&bytes))
}

fn current_unix_seconds() -> Result<u64, BoltV3LiveCanaryGateError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| BoltV3LiveCanaryGateError::SystemTimeBeforeUnixEpoch { source })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

const REQUIRED_NO_SUBMIT_READINESS_STAGES: &[&str] = &[
    OPERATOR_APPROVAL_STAGE,
    SECRET_RESOLUTION_STAGE,
    LIVE_NODE_BUILD_STAGE,
    CONTROLLED_CONNECT_STAGE,
    REFERENCE_READINESS_STAGE,
    CONTROLLED_DISCONNECT_STAGE,
    REPORT_WRITE_STAGE,
];

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    use crate::{
        bolt_v3_config::{
            DECISION_REFERENCE_GATE_ROLE, LiveCanaryBlock, LiveCanaryOperatorEvidenceBlock,
            LoadedBoltV3Config, load_bolt_v3_config,
        },
        bolt_v3_decision_evidence::{
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
            BOLT_V3_ORDER_INTENT_GATE_ID, BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT,
            BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID, BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
            BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3GateEvidenceIdentity,
            BoltV3OrderIntentEvidence, BoltV3OrderIntentKind, BoltV3OrderIntentOrderFields,
            BoltV3StrategyInputEvidenceSnapshot, BoltV3SubmitIntentKind,
        },
        bolt_v3_live_canary_gate::{
            APPROVAL_CONSUMPTION_RECORD_KIND, APPROVAL_CONSUMPTION_SCHEMA_VERSION,
            APPROVAL_ENVELOPE_RECORD_KIND, APPROVAL_ENVELOPE_SCHEMA_VERSION, APPROVAL_ID_HASH_KEY,
            ApprovalConsumptionExpectation, BoltV3LiveCanaryGateError, CONFIG_BUNDLE_CHECKSUM_KEY,
            CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, EXECUTABLE_IDENTITY_KEY,
            GENERATED_AT_UNIX_SECONDS_KEY, LIVE_NODE_BUILD_STAGE, MILLIS_PER_SECOND_U64,
            NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE, REFERENCE_READINESS_STAGE,
            REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY, SECRET_RESOLUTION_STAGE, STAGE_KEY, STAGES_KEY,
            STATUS_KEY, STATUS_SATISFIED, check_bolt_v3_live_canary_gate_with_clock,
            current_build_head_sha, executable_identity, read_to_vec_with_cap, resolve_report_path,
            sha256_hex, validate_operator_approval_consumption,
        },
    };

    #[test]
    fn relative_report_path_without_root_parent_matches_config_loader_fallback() {
        let block = LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "reports/no-submit-readiness.json".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: 60,
            reference_quote_max_age_seconds: 10,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: None,
        };

        assert_eq!(
            resolve_report_path(Path::new(""), &block)
                .expect("relative report path should resolve"),
            PathBuf::from(".").join("reports/no-submit-readiness.json")
        );
    }

    #[test]
    fn relative_report_path_trims_configured_whitespace() {
        let block = LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: " reports/no-submit-readiness.json ".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: 60,
            reference_quote_max_age_seconds: 10,
            reference_quote_wait_timeout_seconds: 10,
            reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
            reference_quote_probe_log_events: true,
            reference_quote_probe_log_commands: true,
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            egress_identity_observed_path: None,
            egress_identity_observed_max_bytes: None,
            approved_egress_identity_sha256: None,
            proof_policy: None,
            operator_evidence: None,
        };

        assert_eq!(
            resolve_report_path(Path::new(""), &block)
                .expect("relative report path should resolve"),
            PathBuf::from(".").join("reports/no-submit-readiness.json")
        );
    }

    #[test]
    fn bounded_reader_rejects_when_actual_stream_exceeds_cap() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build");
        let error = runtime.block_on(async {
            let reader = tokio::io::repeat(b'x');
            read_to_vec_with_cap(reader, 8)
                .await
                .expect_err("reader must fail closed once max plus one byte is observed")
        });

        assert_eq!(error.to_string(), "read exceeds cap of 8 bytes");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn approval_consumption_freshness_uses_first_observed_timestamp() {
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let root_path = tempdir.path().join("root.toml");
        fs::write(&root_path, "root").expect("root TOML should be written");
        let root_toml_sha256 = sha256_hex(b"root");
        let approval_consumption_path = tempdir.path().join("approval-consumption.json");
        let consumed_unix_secs = 1_000_i64;
        let initial_unix_seconds = 1_010_u64;
        let late_unix_seconds = 1_200_u64;
        let approval_id = "operator-approved-canary-001";
        let approval_id_hash = sha256_hex(approval_id.as_bytes());
        let canary_evidence_path = tempdir
            .path()
            .join("canary-evidence.json")
            .to_string_lossy()
            .to_string();
        let canary_evidence_path_hash = sha256_hex(canary_evidence_path.as_bytes());
        let evidence = LiveCanaryOperatorEvidenceBlock {
            head_sha: "0123456789abcdef0123456789abcdef01234567".to_string(),
            max_operator_evidence_file_bytes: 4096,
            approval_consumption_max_age_seconds: 60,
            approval_envelope_path: tempdir
                .path()
                .join("approval-envelope.json")
                .to_string_lossy()
                .to_string(),
            approval_envelope_sha256: "9".repeat(64),
            ssm_manifest_path: tempdir
                .path()
                .join("ssm-manifest.json")
                .to_string_lossy()
                .to_string(),
            ssm_manifest_sha256: "a".repeat(64),
            strategy_input_evidence_path: tempdir
                .path()
                .join("strategy-input.json")
                .to_string_lossy()
                .to_string(),
            strategy_input_evidence_sha256: "b".repeat(64),
            gate_session_path: None,
            expected_gate_session_sha256: None,
            financial_envelope_path: tempdir
                .path()
                .join("financial-envelope.json")
                .to_string_lossy()
                .to_string(),
            financial_envelope_sha256: "c".repeat(64),
            pre_run_state_path: tempdir
                .path()
                .join("pre-run-state.json")
                .to_string_lossy()
                .to_string(),
            pre_run_state_sha256: "d".repeat(64),
            abort_plan_path: tempdir
                .path()
                .join("abort-plan.json")
                .to_string_lossy()
                .to_string(),
            abort_plan_sha256: "e".repeat(64),
            canary_proof_candidate_source_path: None,
            canary_proof_candidate_source_sha256: None,
            canary_proof_order_intent_path: None,
            canary_proof_order_intent_sha256: None,
            canary_evidence_path,
            approval_not_before_unix_seconds: 900,
            approval_not_after_unix_seconds: 1_300,
            approval_nonce_path: tempdir
                .path()
                .join("approval-nonce.json")
                .to_string_lossy()
                .to_string(),
            approval_nonce_sha256: "f".repeat(64),
            approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
            decision_evidence_path: tempdir
                .path()
                .join("decision-evidence.jsonl")
                .to_string_lossy()
                .to_string(),
            nt_submit_event_path: tempdir
                .path()
                .join("nt-submit-event.json")
                .to_string_lossy()
                .to_string(),
            venue_order_state_path: tempdir
                .path()
                .join("venue-order-state.json")
                .to_string_lossy()
                .to_string(),
            strategy_cancel_path: None,
            restart_reconciliation_path: tempdir
                .path()
                .join("restart-reconciliation.json")
                .to_string_lossy()
                .to_string(),
            post_run_hygiene_path: tempdir
                .path()
                .join("post-run-hygiene.json")
                .to_string_lossy()
                .to_string(),
        };
        fs::write(
            &approval_consumption_path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": APPROVAL_CONSUMPTION_SCHEMA_VERSION,
                "record_kind": APPROVAL_CONSUMPTION_RECORD_KIND,
                "head_sha": evidence.head_sha.as_str(),
                "root_toml_sha256": root_toml_sha256,
                "approval_envelope_sha256": evidence.approval_envelope_sha256.as_str(),
                "ssm_manifest_sha256": evidence.ssm_manifest_sha256.as_str(),
                "strategy_input_evidence_sha256": evidence.strategy_input_evidence_sha256.as_str(),
                "financial_envelope_sha256": evidence.financial_envelope_sha256.as_str(),
                "pre_run_state_sha256": evidence.pre_run_state_sha256.as_str(),
                "abort_plan_sha256": evidence.abort_plan_sha256.as_str(),
                "approval_nonce_sha256": evidence.approval_nonce_sha256.as_str(),
                "approval_id_hash": approval_id_hash,
                "canary_evidence_path_hash": canary_evidence_path_hash,
                "approval_not_before_unix_secs": evidence.approval_not_before_unix_seconds,
                "approval_not_after_unix_secs": evidence.approval_not_after_unix_seconds,
                "consumed_unix_secs": consumed_unix_secs,
            }))
            .expect("approval proof should encode"),
        )
        .expect("approval proof should be written");

        validate_operator_approval_consumption(
            &root_path,
            &evidence,
            approval_id,
            late_unix_seconds,
            initial_unix_seconds,
            ApprovalConsumptionExpectation::MustExistAndBeValid,
        )
        .await
        .expect("late revalidation must not re-age an initially fresh approval consumption proof");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn gate_late_revalidation_rejects_expired_approval_window_without_reaging_consumption() {
        let fixture_root_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bolt_v3/root.toml");
        let loaded = load_bolt_v3_config(&fixture_root_path)
            .expect("fixture bolt-v3 root config should load");
        let tempdir = tempfile::tempdir().expect("tempdir should be created");
        let approval_id = "operator-approved-canary-001";
        let initial_unix_seconds = 1_000_u64;
        let late_unix_seconds = 1_200_u64;
        let report_path = tempdir.path().join("no-submit-readiness.json");
        let operator_evidence =
            live_canary_operator_evidence_for_test(LiveCanaryOperatorEvidenceFixture {
                dir: tempdir.path(),
                root_path: &fixture_root_path,
                approval_id,
                approval_not_before_unix_seconds: 900,
                approval_not_after_unix_seconds: 1_100,
                reference_quote_unix_seconds: initial_unix_seconds,
                consumed_unix_secs: initial_unix_seconds as i64,
                approval_consumption_max_age_seconds: 500,
            });
        write_no_submit_readiness_report_for_test(
            &report_path,
            approval_id,
            &executable_identity()
                .await
                .expect("test executable identity should resolve"),
            &loaded.config_bundle_checksum,
            initial_unix_seconds,
        );
        let loaded = loaded_with_live_canary_for_test(
            loaded,
            LiveCanaryBlock {
                approval_id: approval_id.to_string(),
                no_submit_readiness_report_path: report_path.to_string_lossy().to_string(),
                max_live_order_count: 1,
                max_notional_per_order: "1.00".to_string(),
                max_no_submit_readiness_report_bytes: 4096,
                readiness_report_max_age_seconds: 500,
                reference_quote_max_age_seconds: 10,
                reference_quote_wait_timeout_seconds: 10,
                reference_quote_probe_actor_id: "no-submit-reference-quote-probe".to_string(),
                reference_quote_probe_log_events: true,
                reference_quote_probe_log_commands: true,
                egress_identity_observed_path: None,
                egress_identity_observed_max_bytes: None,
                approved_egress_identity_sha256: None,
                proof_policy: None,
                operator_evidence: Some(operator_evidence),
            },
        );
        let mut ticks = [initial_unix_seconds, late_unix_seconds].into_iter();

        let error = check_bolt_v3_live_canary_gate_with_clock(&loaded, || {
            Ok(ticks.next().unwrap_or(late_unix_seconds))
        })
        .await
        .expect_err("late gate revalidation must reject approval window expiry");

        match error {
            BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
                current_unix_seconds,
                approval_not_before_unix_seconds,
                approval_not_after_unix_seconds,
            } => {
                assert_eq!(current_unix_seconds, late_unix_seconds);
                assert_eq!(approval_not_before_unix_seconds, 900);
                assert_eq!(approval_not_after_unix_seconds, 1_100);
            }
            other => panic!("expected late approval-window rejection, got {other:?}"),
        }
    }

    fn loaded_with_live_canary_for_test(
        loaded: LoadedBoltV3Config,
        live_canary: LiveCanaryBlock,
    ) -> LoadedBoltV3Config {
        let mut root = loaded.root;
        root.live_canary = Some(live_canary);
        LoadedBoltV3Config { root, ..loaded }
    }

    struct LiveCanaryOperatorEvidenceFixture<'a> {
        dir: &'a Path,
        root_path: &'a Path,
        approval_id: &'a str,
        approval_not_before_unix_seconds: i64,
        approval_not_after_unix_seconds: i64,
        reference_quote_unix_seconds: u64,
        consumed_unix_secs: i64,
        approval_consumption_max_age_seconds: u64,
    }

    fn live_canary_operator_evidence_for_test(
        fixture: LiveCanaryOperatorEvidenceFixture<'_>,
    ) -> LiveCanaryOperatorEvidenceBlock {
        let LiveCanaryOperatorEvidenceFixture {
            dir,
            root_path,
            approval_id,
            approval_not_before_unix_seconds,
            approval_not_after_unix_seconds,
            reference_quote_unix_seconds,
            consumed_unix_secs,
            approval_consumption_max_age_seconds,
        } = fixture;
        let approval_envelope_path = dir
            .join("approval-envelope.json")
            .to_string_lossy()
            .to_string();
        let (ssm_manifest_path, ssm_manifest_sha256) =
            write_json_file_with_sha256(dir, "ssm-manifest.json", "ssm_manifest");
        let (strategy_input_evidence_path, strategy_input_evidence_sha256) =
            write_strategy_input_file_with_sha256(dir, reference_quote_unix_seconds);
        let (gate_session_path, expected_gate_session_sha256) =
            write_gate_session_file_with_sha256(dir);
        let (financial_envelope_path, financial_envelope_sha256) =
            write_json_file_with_sha256(dir, "financial-envelope.json", "financial_envelope");
        let (pre_run_state_path, pre_run_state_sha256) =
            write_json_file_with_sha256(dir, "pre-run-state.json", "pre_run_state");
        let (abort_plan_path, abort_plan_sha256) =
            write_json_file_with_sha256(dir, "abort-plan.json", "abort_plan");
        let (approval_nonce_path, approval_nonce_sha256) =
            write_json_file_with_sha256(dir, "approval-nonce.json", "approval_nonce");
        let approval_consumption_path = dir.join("approval-consumption.json");
        let canary_evidence_path = dir
            .join("canary-evidence.json")
            .to_string_lossy()
            .to_string();
        let decision_evidence_path =
            write_entry_decision_evidence_file_for_test(dir, reference_quote_unix_seconds);
        let mut evidence = LiveCanaryOperatorEvidenceBlock {
            head_sha: current_build_head_sha()
                .expect("build head sha should be compiled for gate tests")
                .to_string(),
            max_operator_evidence_file_bytes: 4096,
            approval_consumption_max_age_seconds,
            approval_envelope_path,
            approval_envelope_sha256: String::new(),
            ssm_manifest_path,
            ssm_manifest_sha256,
            strategy_input_evidence_path,
            strategy_input_evidence_sha256,
            gate_session_path: Some(gate_session_path),
            expected_gate_session_sha256: Some(expected_gate_session_sha256),
            financial_envelope_path,
            financial_envelope_sha256,
            pre_run_state_path,
            pre_run_state_sha256,
            abort_plan_path,
            abort_plan_sha256,
            canary_proof_candidate_source_path: None,
            canary_proof_candidate_source_sha256: None,
            canary_proof_order_intent_path: None,
            canary_proof_order_intent_sha256: None,
            canary_evidence_path: canary_evidence_path.clone(),
            approval_not_before_unix_seconds,
            approval_not_after_unix_seconds,
            approval_nonce_path,
            approval_nonce_sha256,
            approval_consumption_path: approval_consumption_path.to_string_lossy().to_string(),
            decision_evidence_path,
            nt_submit_event_path: dir
                .join("nt-submit-event.json")
                .to_string_lossy()
                .to_string(),
            venue_order_state_path: dir
                .join("venue-order-state.json")
                .to_string_lossy()
                .to_string(),
            strategy_cancel_path: None,
            restart_reconciliation_path: dir
                .join("restart-reconciliation.json")
                .to_string_lossy()
                .to_string(),
            post_run_hygiene_path: dir
                .join("post-run-hygiene.json")
                .to_string_lossy()
                .to_string(),
        };
        let approval_envelope = approval_envelope_value_for_test(&evidence, approval_id);
        let approval_envelope_bytes =
            serde_json::to_vec(&approval_envelope).expect("approval envelope should serialize");
        fs::write(&evidence.approval_envelope_path, &approval_envelope_bytes)
            .expect("approval envelope should be written");
        evidence.approval_envelope_sha256 = sha256_hex(&approval_envelope_bytes);
        let root_toml_bytes = fs::read(root_path).expect("fixture root TOML should be readable");
        write_json_value(
            &approval_consumption_path,
            serde_json::json!({
                "schema_version": APPROVAL_CONSUMPTION_SCHEMA_VERSION,
                "record_kind": APPROVAL_CONSUMPTION_RECORD_KIND,
                "head_sha": evidence.head_sha,
                "root_toml_sha256": sha256_hex(&root_toml_bytes),
                "approval_envelope_sha256": evidence.approval_envelope_sha256,
                "ssm_manifest_sha256": evidence.ssm_manifest_sha256,
                "strategy_input_evidence_sha256": evidence.strategy_input_evidence_sha256,
                "financial_envelope_sha256": evidence.financial_envelope_sha256,
                "pre_run_state_sha256": evidence.pre_run_state_sha256,
                "abort_plan_sha256": evidence.abort_plan_sha256,
                "approval_nonce_sha256": evidence.approval_nonce_sha256,
                "approval_id_hash": sha256_hex(approval_id.as_bytes()),
                "canary_evidence_path_hash": sha256_hex(canary_evidence_path.as_bytes()),
                "approval_not_before_unix_secs": evidence.approval_not_before_unix_seconds,
                "approval_not_after_unix_secs": evidence.approval_not_after_unix_seconds,
                "consumed_unix_secs": consumed_unix_secs,
            }),
        );
        evidence
    }

    fn approval_envelope_value_for_test(
        evidence: &LiveCanaryOperatorEvidenceBlock,
        approval_id: &str,
    ) -> serde_json::Value {
        let mut envelope = serde_json::json!({
            "schema_version": APPROVAL_ENVELOPE_SCHEMA_VERSION,
            "record_kind": APPROVAL_ENVELOPE_RECORD_KIND,
            "head_sha": evidence.head_sha,
            "ssm_manifest_sha256": evidence.ssm_manifest_sha256,
            "strategy_input_evidence_sha256": evidence.strategy_input_evidence_sha256,
            "financial_envelope_sha256": evidence.financial_envelope_sha256,
            "pre_run_state_sha256": evidence.pre_run_state_sha256,
            "abort_plan_sha256": evidence.abort_plan_sha256,
            "approval_id_hash": sha256_hex(approval_id.as_bytes()),
            "approval_nonce_sha256": evidence.approval_nonce_sha256,
            "approval_not_before_unix_secs": evidence.approval_not_before_unix_seconds,
            "approval_not_after_unix_secs": evidence.approval_not_after_unix_seconds,
            "canary_evidence_path_hash": sha256_hex(evidence.canary_evidence_path.as_bytes()),
        });
        if let Some(strategy_cancel_path) = &evidence.strategy_cancel_path {
            envelope
                .as_object_mut()
                .expect("approval envelope should be an object")
                .insert(
                    "strategy_cancel_path_hash".to_string(),
                    serde_json::json!(sha256_hex(strategy_cancel_path.as_bytes())),
                );
        }
        envelope
    }

    fn write_json_file_with_sha256(
        dir: &Path,
        filename: &str,
        record_kind: &str,
    ) -> (String, String) {
        let path = dir.join(filename);
        let value = serde_json::json!({ "record_kind": record_kind });
        let bytes = serde_json::to_vec(&value).expect("test evidence should serialize");
        fs::write(&path, &bytes).expect("test evidence should be written");
        (path.to_string_lossy().to_string(), sha256_hex(&bytes))
    }

    fn write_strategy_input_file_with_sha256(
        dir: &Path,
        reference_quote_unix_seconds: u64,
    ) -> (String, String) {
        let path = dir.join("strategy-input.json");
        let reference_quote_ts_event = reference_quote_unix_seconds
            .checked_mul(MILLIS_PER_SECOND_U64)
            .expect("test reference quote timestamp should fit in milliseconds");
        let value = serde_json::json!({
            "record_kind": "strategy_input",
            "reference_quote_ts_event": reference_quote_ts_event,
        });
        let bytes = serde_json::to_vec(&value).expect("test evidence should serialize");
        fs::write(&path, &bytes).expect("test evidence should be written");
        (path.to_string_lossy().to_string(), sha256_hex(&bytes))
    }

    fn write_entry_decision_evidence_file_for_test(
        dir: &Path,
        reference_quote_unix_seconds: u64,
    ) -> String {
        let path = dir.join("decision-evidence.jsonl");
        let reference_quote_ts_event = reference_quote_unix_seconds
            .checked_mul(MILLIS_PER_SECOND_U64)
            .expect("test reference quote timestamp should fit in milliseconds");
        let snapshot_recorded_at_utc_ns = i64::try_from(reference_quote_ts_event)
            .expect("test reference quote timestamp should fit in i64 nanosecond field");
        let intent_recorded_at_utc_ns = snapshot_recorded_at_utc_ns
            .checked_add(1)
            .expect("test order intent timestamp should fit in i64");
        let admission_recorded_at_utc_ns = intent_recorded_at_utc_ns
            .checked_add(1)
            .expect("test admission timestamp should fit in i64");
        let strategy_id = "operator-canary-strategy".to_string();
        let client_order_id = "operator-canary-entry-001".to_string();
        let instrument_id = "operator-canary-instrument".to_string();
        let selected_market_key = "operator-canary-market-key".to_string();
        let submission_order_side = "BUY".to_string();
        let submission_price = "0.50".to_string();
        let submission_quantity = "1".to_string();
        let mut gate_evidence = BTreeMap::new();
        gate_evidence.insert(
            DECISION_REFERENCE_GATE_ROLE.to_string(),
            BoltV3GateEvidenceIdentity {
                satisfaction_kind: "no_resolution".to_string(),
                selected_market_key: selected_market_key.clone(),
                provider_id: None,
                provider_kind: None,
                value_kind: None,
                normalized_value_sha256: None,
                provider_provenance_sha256: None,
                artifact_sha256s: Vec::new(),
                resolution_identity: Some("operator-canary-resolution".to_string()),
            },
        );
        let snapshot = BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: strategy_id.clone(),
            configured_target_id: "operator-canary-target".to_string(),
            market_selection_ruleset_id: "operator-canary-ruleset".to_string(),
            gate_session_hash: "operator-canary-gate-session".to_string(),
            selected_market_key: selected_market_key.clone(),
            gate_evidence,
            market_selection_outcome: BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT
                .to_string(),
            market_id: None,
            polymarket_condition_id: None,
            polymarket_market_slug: None,
            polymarket_question_id: None,
            up_instrument_id: None,
            down_instrument_id: None,
            market_selection_timestamp_ms: Some(reference_quote_ts_event),
            selected_market_observed_timestamp_ms: Some(reference_quote_ts_event),
            polymarket_market_start_timestamp_ms: None,
            polymarket_market_end_timestamp_ms: None,
            price_to_beat_source: "operator-canary-reference".to_string(),
            price_to_beat_value: submission_price.clone(),
            reference_quote_ts_event,
            spot_price: submission_price.clone(),
            reference_fair_value: None,
            realized_volatility: "0".to_string(),
            seconds_to_market_end: 1,
            pricing_kurtosis: "0".to_string(),
            theta_decay_factor: "1".to_string(),
            theta_scaled_min_edge_bps: "1".to_string(),
            fair_probability_up: "0.50".to_string(),
            uncertainty_band_probability: "0.01".to_string(),
            expected_edge_basis_points: "1".to_string(),
            worst_case_edge_basis_points: "1".to_string(),
            fee_rate_basis_points: "0".to_string(),
            selected_side: Some(submission_order_side.clone()),
            submission_instrument_id: instrument_id.clone(),
            submission_order_side: submission_order_side.clone(),
            submission_price: submission_price.clone(),
            submission_quantity: submission_quantity.clone(),
            client_order_id: client_order_id.clone(),
        };
        let intent = BoltV3OrderIntentEvidence {
            strategy_id: strategy_id.clone(),
            intent_kind: BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.clone(),
            client_order_id: client_order_id.clone(),
            order_side: submission_order_side,
            price: submission_price.clone(),
            quantity: submission_quantity,
            canary_proof_claim: None,
            order_fields: BoltV3OrderIntentOrderFields {
                order_type: "LIMIT".to_string(),
                time_in_force: "GTC".to_string(),
                price: Some(submission_price.clone()),
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                expire_time_unix_nanos: None,
                is_post_only: true,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
        };
        let admission = BoltV3AdmissionDecisionEvidence {
            strategy_id,
            client_order_id,
            instrument_id,
            notional: "0.50".to_string(),
            intent_kind: BoltV3SubmitIntentKind::Entry,
            outcome: BoltV3AdmissionOutcome::RejectedNotArmed,
        };
        let lines = [
            serde_json::json!({
                "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                "recorded_at_utc_ns": snapshot_recorded_at_utc_ns,
                "gate_id": BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "strategy_input_snapshot",
                "snapshot": snapshot,
            }),
            serde_json::json!({
                "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                "recorded_at_utc_ns": intent_recorded_at_utc_ns,
                "gate_id": BOLT_V3_ORDER_INTENT_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "order_intent",
                "intent": intent,
            }),
            serde_json::json!({
                "schema_version": BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
                "recorded_at_utc_ns": admission_recorded_at_utc_ns,
                "gate_id": BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                "gate_version": BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
                "kind": "admission_decision",
                "decision": admission,
            }),
        ];
        let mut jsonl = String::new();
        for line in lines {
            jsonl.push_str(&serde_json::to_string(&line).expect("decision evidence should encode"));
            jsonl.push('\n');
        }
        fs::write(&path, jsonl).expect("decision evidence should write");
        path.to_string_lossy().to_string()
    }

    fn write_gate_session_file_with_sha256(dir: &Path) -> (String, String) {
        let path = dir.join("entry-readiness-gate-session.json");
        let selected_market_key = "b".repeat(64);
        let value = serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.entry_readiness_gate_session.v1",
            "strategy_instance_id": "configured_updown_main",
            "configured_target_id": "configured_updown_target",
            "selected_market": {
                "configured_target_id": "configured_updown_target",
                "venue": "polymarket",
                "family_key": "updown",
                "market_id": "configured-condition",
                "instrument_ids": ["configured-condition-DOWN.POLYMARKET", "configured-condition-UP.POLYMARKET"],
                "market_class": "binary_option",
                "resolution_kind": "price",
                "resolution_identity": "configured-reference-price",
                "value_kind": "scalar_price",
                "metadata_provenance_sha256": "f".repeat(64),
                "selected_market_key": selected_market_key,
                "selected_at_ms": 1234567890_u64
            },
            "created_at_ms": 1234567890_u64,
            "satisfied_roles": {
                "resolution": {
                    "satisfaction_kind": "no_resolution",
                    "selected_market_key": selected_market_key,
                    "resolution_identity": "configured-reference-price"
                }
            },
            "session_hash": "a".repeat(64),
            "artifact_refs": []
        });
        let bytes = serde_json::to_vec(&value).expect("test gate session should serialize");
        fs::write(&path, &bytes).expect("test gate session should be written");
        (path.to_string_lossy().to_string(), sha256_hex(&bytes))
    }

    fn write_no_submit_readiness_report_for_test(
        path: &Path,
        approval_id: &str,
        executable_identity: &str,
        config_bundle_checksum: &str,
        generated_at_unix_seconds: u64,
    ) {
        let stages = [
            OPERATOR_APPROVAL_STAGE,
            SECRET_RESOLUTION_STAGE,
            LIVE_NODE_BUILD_STAGE,
            CONTROLLED_CONNECT_STAGE,
            REFERENCE_READINESS_STAGE,
            CONTROLLED_DISCONNECT_STAGE,
            REPORT_WRITE_STAGE,
        ]
        .into_iter()
        .map(|stage| serde_json::json!({ STAGE_KEY: stage, STATUS_KEY: STATUS_SATISFIED }))
        .collect::<Vec<_>>();
        write_json_value(
            path,
            serde_json::json!({
                SCHEMA_VERSION_KEY: NO_SUBMIT_READINESS_SCHEMA_VERSION,
                APPROVAL_ID_HASH_KEY: sha256_hex(approval_id.as_bytes()),
                EXECUTABLE_IDENTITY_KEY: executable_identity,
                CONFIG_BUNDLE_CHECKSUM_KEY: config_bundle_checksum,
                GENERATED_AT_UNIX_SECONDS_KEY: generated_at_unix_seconds,
                STAGES_KEY: stages,
            }),
        );
    }

    fn write_json_value(path: &Path, value: serde_json::Value) {
        fs::write(
            path,
            serde_json::to_vec_pretty(&value).expect("test JSON should serialize"),
        )
        .expect("test JSON should be written");
    }
}
