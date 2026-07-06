//! Backfill preflight selection gate.
//!
//! This gate consumes an already-built coverage ledger and selects at most one
//! bounded canonical-ready tranche before any payload download, conversion,
//! catalog projection, or backtest work starts.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::backfill_coverage::{
    BackfillCoverageLedger, BackfillCoverageRecord, BackfillCoverageStatus,
};

pub const BACKFILL_PREFLIGHT_REPORT_SCHEMA_VERSION: &str = "backfill-preflight-report.v1";
pub const BACKFILL_PREFLIGHT_REPORT_FILE: &str = "backfill-preflight-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillPreflightSpec {
    pub preflight_id: String,
    pub coverage_ledger_path: PathBuf,
    pub output_dir: PathBuf,
    pub selection: BackfillPreflightSelection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillPreflightSelection {
    pub max_accepted_objects: u64,
    pub max_accepted_bytes: u64,
    pub require_canonical_ready: bool,
    pub allow_gaps: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillPreflightStatus {
    Go,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillPreflightBlockingReason {
    EmptyPreflightId,
    EmptyLedger,
    InvalidObjectBudget,
    InvalidByteBudget,
    NoAcceptedRecords,
    NoCanonicalReadyRecords,
    NoEligibleRecordsWithinBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillPreflightSelectedRecord {
    pub record_id: String,
    pub source_binding: String,
    pub table_family: String,
    pub coverage_axis: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub accepted_objects: u64,
    pub accepted_bytes: u64,
    pub skipped_objects: u64,
    pub canonical_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillPreflightReport {
    pub schema_version: String,
    pub preflight_id: String,
    pub coverage_ledger_id: String,
    pub status: BackfillPreflightStatus,
    pub selection: BackfillPreflightSelection,
    pub total_records: u64,
    pub accepted_records: u64,
    pub accepted_with_gaps_records: u64,
    pub canonical_ready_records: u64,
    pub eligible_record_count: u64,
    pub selected_record: Option<BackfillPreflightSelectedRecord>,
    pub blocking_reasons: Vec<BackfillPreflightBlockingReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillPreflightReportArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillPreflightError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadLedger { path: String, error: String },
    ParseLedgerJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillPreflightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill preflight spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(f, "parse backfill preflight spec TOML {path}: {error}")
            }
            Self::ReadLedger { path, error } => {
                write!(f, "read backfill coverage ledger {path}: {error}")
            }
            Self::ParseLedgerJson { path, error } => {
                write!(f, "parse backfill coverage ledger JSON {path}: {error}")
            }
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create backfill preflight artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing backfill preflight artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(f, "write backfill preflight artifact {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill preflight artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => write!(f, "serialize backfill preflight artifact: {error}"),
        }
    }
}

impl Error for BackfillPreflightError {}

pub fn evaluate_backfill_preflight(
    preflight_id: impl Into<String>,
    ledger: &BackfillCoverageLedger,
    selection: &BackfillPreflightSelection,
) -> BackfillPreflightReport {
    let preflight_id = preflight_id.into();
    let mut blocking_reasons = Vec::new();
    if preflight_id.trim().is_empty() {
        blocking_reasons.push(BackfillPreflightBlockingReason::EmptyPreflightId);
    }
    if ledger.records.is_empty() {
        blocking_reasons.push(BackfillPreflightBlockingReason::EmptyLedger);
    }
    if selection.max_accepted_objects == 0 {
        blocking_reasons.push(BackfillPreflightBlockingReason::InvalidObjectBudget);
    }
    if selection.max_accepted_bytes == 0 {
        blocking_reasons.push(BackfillPreflightBlockingReason::InvalidByteBudget);
    }
    if ledger.summary.accepted_records == 0 && ledger.summary.accepted_with_gaps_records == 0 {
        blocking_reasons.push(BackfillPreflightBlockingReason::NoAcceptedRecords);
    }
    if selection.require_canonical_ready && ledger.summary.canonical_ready_records == 0 {
        blocking_reasons.push(BackfillPreflightBlockingReason::NoCanonicalReadyRecords);
    }

    let mut eligible = ledger
        .records
        .iter()
        .filter(|record| is_eligible(record, selection))
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        left.accepted_bytes
            .cmp(&right.accepted_bytes)
            .then(left.accepted_objects.cmp(&right.accepted_objects))
            .then(left.record_id.cmp(&right.record_id))
    });

    if eligible.is_empty()
        && blocking_reasons.is_empty()
        && (ledger.summary.accepted_records > 0 || ledger.summary.accepted_with_gaps_records > 0)
    {
        blocking_reasons.push(BackfillPreflightBlockingReason::NoEligibleRecordsWithinBudget);
    }

    let selected_record = if blocking_reasons.is_empty() {
        eligible.first().map(|record| selected_record(record))
    } else {
        None
    };

    let status = if selected_record.is_some() {
        BackfillPreflightStatus::Go
    } else {
        BackfillPreflightStatus::Blocked
    };

    BackfillPreflightReport {
        schema_version: BACKFILL_PREFLIGHT_REPORT_SCHEMA_VERSION.to_string(),
        preflight_id,
        coverage_ledger_id: ledger.ledger_id.clone(),
        status,
        selection: selection.clone(),
        total_records: ledger.summary.total_records,
        accepted_records: ledger.summary.accepted_records,
        accepted_with_gaps_records: ledger.summary.accepted_with_gaps_records,
        canonical_ready_records: ledger.summary.canonical_ready_records,
        eligible_record_count: eligible.len() as u64,
        selected_record,
        blocking_reasons,
    }
}

pub fn write_backfill_preflight_report(
    output_dir: &Path,
    report: &BackfillPreflightReport,
) -> Result<BackfillPreflightReportArtifact, BackfillPreflightError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillPreflightError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(BACKFILL_PREFLIGHT_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_PREFLIGHT_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillPreflightError::Serialize,
            read_existing_error: |path, error| BackfillPreflightError::ReadExisting { path, error },
            mismatch_error: |path| BackfillPreflightError::ExistingArtifactMismatch { path },
            write_error: |path, error| BackfillPreflightError::Write { path, error },
        },
    )?;
    Ok(BackfillPreflightReportArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

pub fn write_backfill_preflight_report_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillPreflightReportArtifact, BackfillPreflightError> {
    let path = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| BackfillPreflightError::ReadSpec {
            path: path.clone(),
            error: error.to_string(),
        })?;
    let spec: BackfillPreflightSpec =
        toml::from_str(&spec_text).map_err(|error| BackfillPreflightError::ParseSpecToml {
            path: path.clone(),
            error: error.to_string(),
        })?;
    let ledger_path = spec.coverage_ledger_path.display().to_string();
    let ledger_bytes = fs::read(&spec.coverage_ledger_path).map_err(|error| {
        BackfillPreflightError::ReadLedger {
            path: ledger_path.clone(),
            error: error.to_string(),
        }
    })?;
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&ledger_bytes).map_err(|error| {
            BackfillPreflightError::ParseLedgerJson {
                path: ledger_path,
                error: error.to_string(),
            }
        })?;
    let report = evaluate_backfill_preflight(spec.preflight_id, &ledger, &spec.selection);
    write_backfill_preflight_report(&spec.output_dir, &report)
}

fn is_eligible(record: &BackfillCoverageRecord, selection: &BackfillPreflightSelection) -> bool {
    match record.status {
        BackfillCoverageStatus::Accepted => {}
        BackfillCoverageStatus::AcceptedWithGaps if selection.allow_gaps => {}
        _ => return false,
    }
    if selection.require_canonical_ready && !record.canonical_ready {
        return false;
    }
    if !record.blocking_issues.is_empty() {
        return false;
    }
    if record
        .source_binding
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
        || record
            .source_proof_id
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        || record
            .coverage_axis
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        || record.source_proof_version.is_none()
    {
        return false;
    }
    if record.accepted_objects == 0 || record.accepted_bytes == 0 {
        return false;
    }
    record.accepted_objects <= selection.max_accepted_objects
        && record.accepted_bytes <= selection.max_accepted_bytes
}

fn selected_record(record: &BackfillCoverageRecord) -> BackfillPreflightSelectedRecord {
    BackfillPreflightSelectedRecord {
        record_id: record.record_id.clone(),
        source_binding: record.source_binding.clone().unwrap_or_default(),
        table_family: record.table_family.clone().unwrap_or_default(),
        coverage_axis: record.coverage_axis.clone().unwrap_or_default(),
        source_proof_id: record.source_proof_id.clone().unwrap_or_default(),
        source_proof_version: record.source_proof_version.unwrap_or_default(),
        accepted_objects: record.accepted_objects,
        accepted_bytes: record.accepted_bytes,
        skipped_objects: record.skipped_objects,
        canonical_ready: record.canonical_ready,
    }
}
