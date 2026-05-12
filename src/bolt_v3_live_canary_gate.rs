use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use rust_decimal::Decimal;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_no_submit_readiness::{
        BOLT_V3_NO_SUBMIT_READINESS_REQUIRED_STAGES, BoltV3NoSubmitReadinessReport,
        BoltV3NoSubmitReadinessStage, BoltV3NoSubmitReadinessStatus,
    },
};

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct BoltV3LiveCanaryGateReport {
    pub approval_id: String,
    pub no_submit_readiness_report_path: PathBuf,
    pub report_path: PathBuf,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
}

#[derive(Debug)]
pub enum BoltV3LiveCanaryGateError {
    MissingConfig,
    ReadNoSubmitReadinessReport {
        path: PathBuf,
        source: std::io::Error,
    },
    ParseNoSubmitReadinessReport {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsatisfiedNoSubmitReadinessReport {
        path: PathBuf,
    },
    MissingNoSubmitReadinessStage {
        path: PathBuf,
        stage: BoltV3NoSubmitReadinessStage,
    },
    InvalidMaxNotional {
        value: String,
        reason: String,
    },
    CanaryNotionalExceedsRootRisk {
        canary: String,
        root: String,
    },
}

impl std::fmt::Display for BoltV3LiveCanaryGateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingConfig => write!(f, "live_canary config block is required"),
            Self::ReadNoSubmitReadinessReport { path, source } => write!(
                f,
                "failed to read no-submit readiness report {}: {source}",
                path.display()
            ),
            Self::ParseNoSubmitReadinessReport { path, source } => write!(
                f,
                "failed to parse no-submit readiness report {}: {source}",
                path.display()
            ),
            Self::UnsatisfiedNoSubmitReadinessReport { path } => write!(
                f,
                "no-submit readiness report contains failed or skipped facts: {}",
                path.display()
            ),
            Self::MissingNoSubmitReadinessStage { path, stage } => write!(
                f,
                "no-submit readiness report {} does not contain satisfied {stage:?} stage",
                path.display()
            ),
            Self::InvalidMaxNotional { value, reason } => write!(
                f,
                "live_canary.max_notional_per_order is not accepted ({reason}): `{value}`"
            ),
            Self::CanaryNotionalExceedsRootRisk { canary, root } => write!(
                f,
                "live_canary.max_notional_per_order exceeds root risk.default_max_notional_per_order: `{canary}` > `{root}`"
            ),
        }
    }
}

impl std::error::Error for BoltV3LiveCanaryGateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadNoSubmitReadinessReport { source, .. } => Some(source),
            Self::ParseNoSubmitReadinessReport { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn check_bolt_v3_live_canary_gate(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveCanaryGateReport, BoltV3LiveCanaryGateError> {
    let canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3LiveCanaryGateError::MissingConfig)?;
    let max_notional = Decimal::from_str(&canary.max_notional_per_order).map_err(|error| {
        BoltV3LiveCanaryGateError::InvalidMaxNotional {
            value: canary.max_notional_per_order.clone(),
            reason: error.to_string(),
        }
    })?;
    if max_notional <= Decimal::ZERO {
        return Err(BoltV3LiveCanaryGateError::InvalidMaxNotional {
            value: canary.max_notional_per_order.clone(),
            reason: "must be positive".to_string(),
        });
    }
    let root_max_notional = Decimal::from_str(&loaded.root.risk.default_max_notional_per_order)
        .map_err(|error| BoltV3LiveCanaryGateError::InvalidMaxNotional {
            value: canary.max_notional_per_order.clone(),
            reason: format!("root risk cap is invalid: {error}"),
        })?;
    if max_notional > root_max_notional {
        return Err(BoltV3LiveCanaryGateError::CanaryNotionalExceedsRootRisk {
            canary: canary.max_notional_per_order.clone(),
            root: loaded.root.risk.default_max_notional_per_order.clone(),
        });
    }

    let no_submit_report_path =
        resolve_config_relative_path(&loaded.root_path, &canary.no_submit_readiness_report_path);
    let report_text = std::fs::read_to_string(&no_submit_report_path).map_err(|source| {
        BoltV3LiveCanaryGateError::ReadNoSubmitReadinessReport {
            path: no_submit_report_path.clone(),
            source,
        }
    })?;
    let report: BoltV3NoSubmitReadinessReport =
        serde_json::from_str(&report_text).map_err(|source| {
            BoltV3LiveCanaryGateError::ParseNoSubmitReadinessReport {
                path: no_submit_report_path.clone(),
                source,
            }
        })?;

    require_satisfied_no_submit_report(&report, &no_submit_report_path)?;

    Ok(BoltV3LiveCanaryGateReport {
        approval_id: canary.approval_id.clone(),
        no_submit_readiness_report_path: no_submit_report_path,
        report_path: resolve_config_relative_path(&loaded.root_path, &canary.report_path),
        max_live_order_count: canary.max_live_order_count,
        max_notional_per_order: canary.max_notional_per_order.clone(),
    })
}

fn require_satisfied_no_submit_report(
    report: &BoltV3NoSubmitReadinessReport,
    path: &Path,
) -> Result<(), BoltV3LiveCanaryGateError> {
    if report
        .facts
        .iter()
        .any(|fact| fact.status != BoltV3NoSubmitReadinessStatus::Satisfied)
    {
        return Err(
            BoltV3LiveCanaryGateError::UnsatisfiedNoSubmitReadinessReport {
                path: path.to_path_buf(),
            },
        );
    }
    for stage in BOLT_V3_NO_SUBMIT_READINESS_REQUIRED_STAGES {
        if !report.facts.iter().any(|fact| {
            fact.stage == *stage && fact.status == BoltV3NoSubmitReadinessStatus::Satisfied
        }) {
            return Err(BoltV3LiveCanaryGateError::MissingNoSubmitReadinessStage {
                path: path.to_path_buf(),
                stage: *stage,
            });
        }
    }
    Ok(())
}

fn resolve_config_relative_path(root_path: &Path, configured_path: &str) -> PathBuf {
    let path = PathBuf::from(configured_path);
    if path.is_absolute() {
        return path;
    }
    root_path
        .parent()
        .map(|parent| parent.join(path.as_path()))
        .unwrap_or(path)
}
