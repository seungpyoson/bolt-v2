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
};

use rust_decimal::Decimal;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config, resolve_root_relative_path},
    bolt_v3_no_submit_readiness_schema::{
        APPROVAL_ID_HASH_KEY, CONFIG_BUNDLE_CHECKSUM_KEY, CONTROLLED_CONNECT_STAGE,
        CONTROLLED_DISCONNECT_STAGE, EXECUTABLE_IDENTITY_KEY, LIVE_NODE_BUILD_STAGE,
        NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE, REFERENCE_READINESS_STAGE,
        REPORT_WRITE_STAGE, SCHEMA_VERSION_KEY, SECRET_RESOLUTION_STAGE, STAGE_KEY, STAGES_KEY,
        STATUS_KEY, STATUS_SATISFIED,
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
    UnsatisfiedNoSubmitReadinessReport {
        path: PathBuf,
        failures: Vec<NoSubmitReadinessReportFailure>,
    },
}

/// Typed per-report-shape failures aggregated under
/// [`BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport`].
///
/// The validator collects every failure mode it observes before
/// returning, so a single rejection carries the complete operator
/// triage picture rather than only the first failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoSubmitReadinessReportFailure {
    LinkageFieldMissing {
        field: &'static str,
    },
    LinkageFieldNotString {
        field: &'static str,
        kind: NoSubmitReadinessReportFieldKind,
    },
    LinkageFieldEmpty {
        field: &'static str,
    },
    ApprovalIdHashMismatch {
        expected: String,
        actual: String,
    },
    ExecutableIdentityMismatch {
        expected: String,
        actual: String,
    },
    ConfigBundleChecksumMismatch {
        expected: String,
        actual: String,
    },
    StagesMissing,
    StagesNotArray {
        kind: NoSubmitReadinessReportStagesNotArrayKind,
    },
    StagesEmpty,
    StageEntryMissingStageKey,
    StageStatusMissing {
        stage: String,
    },
    StageStatusNotSatisfied {
        stage: String,
        status: String,
    },
    RequiredStageMissingOrUnsatisfied {
        stage: String,
    },
}

impl std::fmt::Display for NoSubmitReadinessReportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoSubmitReadinessReportFailure::LinkageFieldMissing { field } => {
                write!(f, "linkage field `{field}` is missing")
            }
            NoSubmitReadinessReportFailure::LinkageFieldNotString { field, kind } => {
                write!(f, "linkage field `{field}` is not a string (got {kind})")
            }
            NoSubmitReadinessReportFailure::LinkageFieldEmpty { field } => {
                write!(f, "linkage field `{field}` is empty")
            }
            NoSubmitReadinessReportFailure::ApprovalIdHashMismatch { .. } => {
                write!(
                    f,
                    "linkage field `{APPROVAL_ID_HASH_KEY}` does not match loaded config"
                )
            }
            NoSubmitReadinessReportFailure::ExecutableIdentityMismatch { .. } => {
                write!(
                    f,
                    "linkage field `{EXECUTABLE_IDENTITY_KEY}` does not match current executable"
                )
            }
            NoSubmitReadinessReportFailure::ConfigBundleChecksumMismatch { .. } => {
                write!(
                    f,
                    "linkage field `{CONFIG_BUNDLE_CHECKSUM_KEY}` does not match loaded config bundle"
                )
            }
            NoSubmitReadinessReportFailure::StagesMissing => {
                write!(f, "stages array is missing")
            }
            NoSubmitReadinessReportFailure::StagesNotArray { kind } => {
                write!(f, "stages field is not an array (got {kind})")
            }
            NoSubmitReadinessReportFailure::StagesEmpty => {
                write!(f, "stages array is empty")
            }
            NoSubmitReadinessReportFailure::StageEntryMissingStageKey => {
                write!(f, "stage entry is missing `{STAGE_KEY}`")
            }
            NoSubmitReadinessReportFailure::StageStatusMissing { stage } => {
                write!(f, "stage `{stage}` status is missing")
            }
            NoSubmitReadinessReportFailure::StageStatusNotSatisfied { stage, status } => {
                write!(f, "stage `{stage}` status is `{status}`")
            }
            NoSubmitReadinessReportFailure::RequiredStageMissingOrUnsatisfied { stage } => {
                write!(f, "required stage `{stage}` is missing or unsatisfied")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoSubmitReadinessReportFieldKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl std::fmt::Display for NoSubmitReadinessReportFieldKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoSubmitReadinessReportFieldKind::Null => write!(f, "null"),
            NoSubmitReadinessReportFieldKind::Bool => write!(f, "bool"),
            NoSubmitReadinessReportFieldKind::Number => write!(f, "number"),
            NoSubmitReadinessReportFieldKind::String => write!(f, "string"),
            NoSubmitReadinessReportFieldKind::Array => write!(f, "array"),
            NoSubmitReadinessReportFieldKind::Object => write!(f, "object"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoSubmitReadinessReportStagesNotArrayKind {
    Null,
    Bool,
    Number,
    String,
    Object,
}

impl std::fmt::Display for NoSubmitReadinessReportStagesNotArrayKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NoSubmitReadinessReportStagesNotArrayKind::Null => write!(f, "null"),
            NoSubmitReadinessReportStagesNotArrayKind::Bool => write!(f, "bool"),
            NoSubmitReadinessReportStagesNotArrayKind::Number => write!(f, "number"),
            NoSubmitReadinessReportStagesNotArrayKind::String => write!(f, "string"),
            NoSubmitReadinessReportStagesNotArrayKind::Object => write!(f, "object"),
        }
    }
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
            BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport { path, failures } => {
                let joined = failures
                    .iter()
                    .map(|failure| failure.to_string())
                    .collect::<Vec<_>>()
                    .join("; ");
                write!(
                    f,
                    "bolt-v3 no-submit readiness report {} is not satisfied: {joined}",
                    path.display(),
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
    validate_no_submit_readiness_report(
        report_object,
        &expected_approval_id_hash,
        &expected_executable_identity,
        &loaded.config_bundle_checksum,
    )
    .map_err(
        |failures| BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport {
            path: report_path.clone(),
            failures,
        },
    )?;

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
    resolve_root_relative_path(root_path, &block.no_submit_readiness_report_path)
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

fn validate_no_submit_readiness_report(
    report: &Map<String, Value>,
    expected_approval_id_hash: &str,
    expected_executable_identity: &str,
    expected_config_bundle_checksum: &str,
) -> Result<(), Vec<NoSubmitReadinessReportFailure>> {
    let mut failures = Vec::new();
    validate_linkage_field(
        &mut failures,
        report,
        APPROVAL_ID_HASH_KEY,
        expected_approval_id_hash,
        |expected, actual| NoSubmitReadinessReportFailure::ApprovalIdHashMismatch {
            expected,
            actual,
        },
    );
    validate_linkage_field(
        &mut failures,
        report,
        EXECUTABLE_IDENTITY_KEY,
        expected_executable_identity,
        |expected, actual| NoSubmitReadinessReportFailure::ExecutableIdentityMismatch {
            expected,
            actual,
        },
    );
    validate_linkage_field(
        &mut failures,
        report,
        CONFIG_BUNDLE_CHECKSUM_KEY,
        expected_config_bundle_checksum,
        |expected, actual| NoSubmitReadinessReportFailure::ConfigBundleChecksumMismatch {
            expected,
            actual,
        },
    );
    match report.get(STAGES_KEY) {
        None => failures.push(NoSubmitReadinessReportFailure::StagesMissing),
        Some(serde_json::Value::Null) => {
            failures.push(NoSubmitReadinessReportFailure::StagesNotArray {
                kind: NoSubmitReadinessReportStagesNotArrayKind::Null,
            })
        }
        Some(serde_json::Value::Bool(_)) => {
            failures.push(NoSubmitReadinessReportFailure::StagesNotArray {
                kind: NoSubmitReadinessReportStagesNotArrayKind::Bool,
            })
        }
        Some(serde_json::Value::Number(_)) => {
            failures.push(NoSubmitReadinessReportFailure::StagesNotArray {
                kind: NoSubmitReadinessReportStagesNotArrayKind::Number,
            })
        }
        Some(serde_json::Value::String(_)) => {
            failures.push(NoSubmitReadinessReportFailure::StagesNotArray {
                kind: NoSubmitReadinessReportStagesNotArrayKind::String,
            })
        }
        Some(serde_json::Value::Object(_)) => {
            failures.push(NoSubmitReadinessReportFailure::StagesNotArray {
                kind: NoSubmitReadinessReportStagesNotArrayKind::Object,
            })
        }
        Some(serde_json::Value::Array(stages)) if stages.is_empty() => {
            failures.push(NoSubmitReadinessReportFailure::StagesEmpty)
        }
        Some(serde_json::Value::Array(stages)) => {
            let mut present_stage_names = std::collections::BTreeSet::new();
            let mut satisfied_stage_names = std::collections::BTreeSet::new();
            for stage in stages {
                let Some(name) = stage.get(STAGE_KEY).and_then(Value::as_str) else {
                    failures.push(NoSubmitReadinessReportFailure::StageEntryMissingStageKey);
                    continue;
                };
                present_stage_names.insert(name.to_string());
                match stage.get(STATUS_KEY).and_then(Value::as_str) {
                    None => failures.push(NoSubmitReadinessReportFailure::StageStatusMissing {
                        stage: name.to_string(),
                    }),
                    Some(status) if status == STATUS_SATISFIED => {
                        satisfied_stage_names.insert(name.to_string());
                    }
                    Some(status) => {
                        failures.push(NoSubmitReadinessReportFailure::StageStatusNotSatisfied {
                            stage: name.to_string(),
                            status: status.to_string(),
                        })
                    }
                }
            }
            for required_stage in REQUIRED_NO_SUBMIT_READINESS_STAGES {
                if !present_stage_names.contains(*required_stage)
                    && !satisfied_stage_names.contains(*required_stage)
                {
                    failures.push(
                        NoSubmitReadinessReportFailure::RequiredStageMissingOrUnsatisfied {
                            stage: (*required_stage).to_string(),
                        },
                    );
                }
            }
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

fn validate_linkage_field(
    failures: &mut Vec<NoSubmitReadinessReportFailure>,
    report: &Map<String, Value>,
    field: &'static str,
    expected: &str,
    mismatch: impl FnOnce(String, String) -> NoSubmitReadinessReportFailure,
) {
    let Some(value) = report.get(field) else {
        failures.push(NoSubmitReadinessReportFailure::LinkageFieldMissing { field });
        return;
    };
    let Some(actual) = value.as_str() else {
        failures.push(NoSubmitReadinessReportFailure::LinkageFieldNotString {
            field,
            kind: report_field_kind(value),
        });
        return;
    };
    if actual.trim().is_empty() {
        failures.push(NoSubmitReadinessReportFailure::LinkageFieldEmpty { field });
        return;
    }
    if actual != expected {
        failures.push(mismatch(expected.to_string(), actual.to_string()));
    }
}

fn report_field_kind(value: &Value) -> NoSubmitReadinessReportFieldKind {
    match value {
        Value::Null => NoSubmitReadinessReportFieldKind::Null,
        Value::Bool(_) => NoSubmitReadinessReportFieldKind::Bool,
        Value::Number(_) => NoSubmitReadinessReportFieldKind::Number,
        Value::String(_) => NoSubmitReadinessReportFieldKind::String,
        Value::Array(_) => NoSubmitReadinessReportFieldKind::Array,
        Value::Object(_) => NoSubmitReadinessReportFieldKind::Object,
    }
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
    fn relative_report_path_without_root_parent_uses_configured_relative_path() {
        let block = LiveCanaryBlock {
            approval_id: "operator-approved-canary-001".to_string(),
            no_submit_readiness_report_path: "reports/no-submit-readiness.json".to_string(),
            max_no_submit_readiness_report_bytes: 4096,
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
            operator_evidence: None,
        };

        assert_eq!(
            resolve_report_path(Path::new(""), &block),
            PathBuf::from("reports/no-submit-readiness.json")
        );
    }
}
