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
    path::{Path, PathBuf},
    str::FromStr,
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

use rust_decimal::Decimal;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    bolt_v3_config::{LiveCanaryBlock, LiveCanaryOperatorEvidenceBlock, LoadedBoltV3Config},
    bolt_v3_no_submit_readiness_schema::{
        APPROVAL_ID_HASH_KEY, CONFIG_BUNDLE_CHECKSUM_KEY, CONTROLLED_CONNECT_STAGE,
        CONTROLLED_DISCONNECT_STAGE, EXECUTABLE_IDENTITY_KEY, GENERATED_AT_UNIX_SECONDS_KEY,
        LIVE_NODE_BUILD_STAGE, NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE,
        REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY, SECRET_RESOLUTION_STAGE,
        STAGE_KEY, STAGES_KEY, STATUS_KEY, STATUS_SATISFIED,
    },
};

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
    InvalidMaxLiveOrderCount {
        value: u32,
    },
    InvalidReadinessReportSizeLimit {
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
            BoltV3LiveCanaryGateError::InvalidMaxLiveOrderCount { value } => write!(
                f,
                "bolt-v3 live canary max_live_order_count must be positive, got {value}"
            ),
            BoltV3LiveCanaryGateError::InvalidReadinessReportSizeLimit { value } => write!(
                f,
                "bolt-v3 live canary max_no_submit_readiness_report_bytes must be positive, got {value}"
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

#[doc(hidden)]
pub async fn check_bolt_v3_live_canary_gate_with_unix_seconds_for_test(
    loaded: &LoadedBoltV3Config,
    initial_unix_seconds: u64,
    late_unix_seconds: u64,
) -> Result<BoltV3LiveCanaryGateReport, BoltV3LiveCanaryGateError> {
    let mut calls = 0_u8;
    check_bolt_v3_live_canary_gate_with_clock(loaded, || {
        calls = calls.saturating_add(1);
        if calls == 1 {
            Ok(initial_unix_seconds)
        } else {
            Ok(late_unix_seconds)
        }
    })
    .await
}

async fn check_bolt_v3_live_canary_gate_with_clock(
    loaded: &LoadedBoltV3Config,
    mut unix_seconds: impl FnMut() -> Result<u64, BoltV3LiveCanaryGateError>,
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
    validate_operator_evidence(block, initial_unix_seconds)?;

    let report_path = resolve_report_path(&loaded.root_path, block);
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
    validate_operator_evidence(block, late_unix_seconds)?;

    Ok(BoltV3LiveCanaryGateReport {
        approval_id: approval_id.to_string(),
        no_submit_readiness_report_path: report_path,
        max_no_submit_readiness_report_bytes: block.max_no_submit_readiness_report_bytes,
        max_live_order_count: block.max_live_order_count,
        max_notional_per_order,
        root_max_notional_per_order,
    })
}

async fn read_report_bytes_with_limit(
    path: &Path,
    max_length: u64,
) -> Result<Vec<u8>, BoltV3LiveCanaryGateError> {
    let file = tokio::fs::File::open(path).await.map_err(|source| {
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

fn resolve_report_path(root_path: &Path, block: &LiveCanaryBlock) -> PathBuf {
    let configured = PathBuf::from(&block.no_submit_readiness_report_path);
    if configured.is_absolute() {
        return configured;
    }
    root_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&configured)
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

fn validate_operator_evidence(
    block: &LiveCanaryBlock,
    current_unix_seconds: u64,
) -> Result<(), BoltV3LiveCanaryGateError> {
    let evidence = block
        .operator_evidence
        .as_ref()
        .ok_or(BoltV3LiveCanaryGateError::MissingOperatorEvidence)?;

    for (field, value) in required_operator_evidence_fields(evidence) {
        if value.trim().is_empty() {
            return Err(BoltV3LiveCanaryGateError::MissingOperatorEvidenceField { field });
        }
    }
    if let Some(strategy_cancel_path) = &evidence.strategy_cancel_path {
        if strategy_cancel_path.trim().is_empty() {
            return Err(BoltV3LiveCanaryGateError::MissingOperatorEvidenceField {
                field: "strategy_cancel_path",
            });
        }
    }

    if evidence.approval_not_after_unix_seconds <= evidence.approval_not_before_unix_seconds {
        return Err(BoltV3LiveCanaryGateError::InvalidOperatorApprovalWindow {
            approval_not_before_unix_seconds: evidence.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: evidence.approval_not_after_unix_seconds,
        });
    }

    let current = i128::from(current_unix_seconds);
    let not_before = i128::from(evidence.approval_not_before_unix_seconds);
    let not_after = i128::from(evidence.approval_not_after_unix_seconds);
    if current < not_before || current > not_after {
        return Err(BoltV3LiveCanaryGateError::InactiveOperatorApprovalWindow {
            current_unix_seconds,
            approval_not_before_unix_seconds: evidence.approval_not_before_unix_seconds,
            approval_not_after_unix_seconds: evidence.approval_not_after_unix_seconds,
        });
    }

    Ok(())
}

fn required_operator_evidence_fields(
    evidence: &LiveCanaryOperatorEvidenceBlock,
) -> [(&'static str, &str); 22] {
    [
        ("approval_envelope_path", &evidence.approval_envelope_path),
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
        ("client_order_id_hash", &evidence.client_order_id_hash),
        ("venue_order_id_hash", &evidence.venue_order_id_hash),
        ("nt_submit_event_path", &evidence.nt_submit_event_path),
        ("venue_order_state_path", &evidence.venue_order_state_path),
        (
            "restart_reconciliation_path",
            &evidence.restart_reconciliation_path,
        ),
        ("post_run_hygiene_path", &evidence.post_run_hygiene_path),
    ]
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
                let mut satisfied_stage_names = std::collections::BTreeSet::new();
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
                    } else {
                        satisfied_stage_names.insert(name.to_string());
                    }
                }
                for required_stage in REQUIRED_NO_SUBMIT_READINESS_STAGES {
                    if !present_stage_names.contains(*required_stage)
                        && !satisfied_stage_names.contains(*required_stage)
                    {
                        reasons.push(format!(
                            "required stage `{required_stage}` is missing or unsatisfied"
                        ));
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
    use std::path::{Path, PathBuf};

    use crate::{bolt_v3_config::LiveCanaryBlock, bolt_v3_live_canary_gate::resolve_report_path};

    #[test]
    fn relative_report_path_without_root_parent_matches_config_loader_fallback() {
        let block = LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "reports/no-submit-readiness.json".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            readiness_report_max_age_seconds: 60,
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            operator_evidence: None,
        };

        assert_eq!(
            resolve_report_path(Path::new(""), &block),
            PathBuf::from(".").join("reports/no-submit-readiness.json")
        );
    }
}
