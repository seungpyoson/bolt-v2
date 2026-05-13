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
use serde_json::Value;
use tokio::io::AsyncReadExt;

use crate::{
    bolt_v3_config::{LiveCanaryBlock, LoadedBoltV3Config},
    bolt_v3_no_submit_readiness_schema::{
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, LIVE_NODE_BUILD_STAGE, NAME_KEY,
        OPERATOR_APPROVAL_STAGE, REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE,
        SECRET_RESOLUTION_STAGE, STAGE_KEY, STAGES_KEY, STATUS_KEY, STATUS_SATISFIED,
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
    validate_no_submit_readiness_report(&report).map_err(|reasons| {
        BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport {
            path: report_path.clone(),
            reasons,
        }
    })?;

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

fn validate_no_submit_readiness_report(report: &Value) -> Result<(), Vec<String>> {
    let mut reasons = Vec::new();
    let report = match report.as_object() {
        Some(report) => report,
        None => {
            reasons.push(format!("expected JSON object, got {report}"));
            return Err(reasons);
        }
    };
    match report.get(STAGES_KEY) {
        None => reasons.push("stages array is missing".to_string()),
        Some(stages_value) => match stages_value.as_array() {
            None => reasons.push(format!("stages must be an array, got {stages_value}")),
            Some(stages) if stages.is_empty() => reasons.push("stages array is empty".to_string()),
            Some(stages) => {
                let mut satisfied_stage_names = std::collections::BTreeSet::new();
                for stage in stages {
                    let name = stage
                        .get(STAGE_KEY)
                        .or_else(|| stage.get(NAME_KEY))
                        .and_then(Value::as_str)
                        .unwrap_or("<unnamed>");
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
                    if !satisfied_stage_names.contains(*required_stage) {
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

fn matches_satisfied_status(status: Option<&str>) -> bool {
    matches!(status, Some(value) if value.eq_ignore_ascii_case(STATUS_SATISFIED))
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
            max_live_order_count: 1,
            max_notional_per_order: "1.00".to_string(),
        };

        assert_eq!(
            resolve_report_path(Path::new(""), &block),
            PathBuf::from(".").join("reports/no-submit-readiness.json")
        );
    }
}
