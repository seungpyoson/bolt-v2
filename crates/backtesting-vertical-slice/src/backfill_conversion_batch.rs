//! Venue-level backfill conversion batch planning.
//!
//! This pre-payload artifact groups accepted coverage records with the concrete
//! execution plans and run-spec hashes that the durable operator will need. It
//! does not download payloads, convert rows, project catalogs, or run backtests.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    backfill_coverage::{BackfillCoverageLedger, BackfillCoverageRecord, BackfillCoverageStatus},
    backfill_execution_plan::{BackfillExecutionPlan, BackfillExecutionPlanStatus},
};

pub const BACKFILL_CONVERSION_BATCH_PLAN_SCHEMA_VERSION: &str = "backfill-conversion-batch-plan.v1";
pub const BACKFILL_CONVERSION_BATCH_PLAN_FILE: &str = "backfill-conversion-batch-plan.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionBatchSelection {
    pub max_records: u64,
    pub max_accepted_objects: u64,
    pub max_accepted_bytes: u64,
    pub require_uniform_source_binding: bool,
    pub allow_gaps: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillConversionBatchInput {
    pub record_id: String,
    pub run_spec_path: PathBuf,
    pub run_spec_hash: String,
    pub execution_plan_path: PathBuf,
    pub execution_plan_hash: String,
    pub execution_plan: BackfillExecutionPlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionBatchPlanSpec {
    pub batch_id: String,
    pub coverage_ledger_path: PathBuf,
    pub output_dir: PathBuf,
    pub selection: BackfillConversionBatchSelection,
    #[serde(rename = "input", default)]
    pub inputs: Vec<BackfillConversionBatchPlanSpecInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionBatchPlanSpecInput {
    pub record_id: String,
    pub run_spec_path: PathBuf,
    pub execution_plan_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillConversionBatchStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillConversionBatchBlockingIssue {
    EmptyBatchId,
    EmptyInputSet,
    InvalidRecordBudget,
    InvalidObjectBudget,
    InvalidByteBudget,
    DuplicateRecordInput,
    MissingCoverageRecord,
    CoverageRecordNotAccepted,
    CoverageRecordHasBlockingIssues,
    CoverageRecordHasGapsNotAllowed,
    CoverageRecordMissingBinding,
    ExecutionPlanNotReady,
    ExecutionPlanRecordMismatch,
    ExecutionPlanRunSpecHashMismatch,
    ExecutionPlanSourceProofMismatch,
    ExecutionPlanSourceBindingMismatch,
    ExecutionPlanTableFamilyMismatch,
    ExecutionPlanAcceptedObjectCountMismatch,
    ExecutionPlanAcceptedBytesMismatch,
    BatchRecordBudgetExceeded,
    BatchObjectBudgetExceeded,
    BatchByteBudgetExceeded,
    MixedSourceBinding,
    MixedTableFamily,
    MixedCoverageAxis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionBatchRecord {
    pub record_id: String,
    pub source_binding: String,
    pub table_family: String,
    pub coverage_axis: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub canonical_ready: bool,
    pub accepted_objects: u64,
    pub accepted_bytes: u64,
    pub run_spec_path: PathBuf,
    pub run_spec_hash: String,
    pub execution_plan_path: PathBuf,
    pub execution_plan_hash: String,
    pub operator_run_id: String,
    pub output_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionBatchPlan {
    pub schema_version: String,
    pub batch_id: String,
    pub coverage_ledger_id: String,
    pub status: BackfillConversionBatchStatus,
    pub selection: BackfillConversionBatchSelection,
    pub record_count: u64,
    pub total_accepted_objects: u64,
    pub total_accepted_bytes: u64,
    pub canonical_ready_records: u64,
    pub records: Vec<BackfillConversionBatchRecord>,
    pub blocking_issues: Vec<BackfillConversionBatchBlockingIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillConversionBatchPlanArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillConversionBatchPlanError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadCoverageLedger { path: String, error: String },
    ParseCoverageLedgerJson { path: String, error: String },
    ReadRunSpec { path: String, error: String },
    ReadExecutionPlan { path: String, error: String },
    ParseExecutionPlanJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillConversionBatchPlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill conversion-batch spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(
                    f,
                    "parse backfill conversion-batch spec TOML {path}: {error}"
                )
            }
            Self::ReadCoverageLedger { path, error } => {
                write!(f, "read backfill coverage ledger {path}: {error}")
            }
            Self::ParseCoverageLedgerJson { path, error } => {
                write!(f, "parse backfill coverage ledger JSON {path}: {error}")
            }
            Self::ReadRunSpec { path, error } => {
                write!(f, "read run-spec for conversion batch {path}: {error}")
            }
            Self::ReadExecutionPlan { path, error } => {
                write!(
                    f,
                    "read execution plan for conversion batch {path}: {error}"
                )
            }
            Self::ParseExecutionPlanJson { path, error } => {
                write!(f, "parse execution plan JSON {path}: {error}")
            }
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create backfill conversion-batch artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing backfill conversion-batch artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(
                    f,
                    "write backfill conversion-batch artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill conversion-batch artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize backfill conversion-batch artifact: {error}")
            }
        }
    }
}

impl Error for BackfillConversionBatchPlanError {}

#[must_use]
pub fn evaluate_backfill_conversion_batch_plan(
    batch_id: impl Into<String>,
    ledger: &BackfillCoverageLedger,
    selection: &BackfillConversionBatchSelection,
    inputs: Vec<BackfillConversionBatchInput>,
) -> BackfillConversionBatchPlan {
    let batch_id = batch_id.into();
    let mut blocking_issues = Vec::new();
    if batch_id.trim().is_empty() {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::EmptyBatchId);
    }
    if inputs.is_empty() {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::EmptyInputSet);
    }
    if selection.max_records == 0 {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::InvalidRecordBudget);
    }
    if selection.max_accepted_objects == 0 {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::InvalidObjectBudget);
    }
    if selection.max_accepted_bytes == 0 {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::InvalidByteBudget);
    }

    let records_by_id = ledger
        .records
        .iter()
        .map(|record| (record.record_id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut batch_records = Vec::new();

    for input in inputs {
        if !seen.insert(input.record_id.clone()) {
            blocking_issues.push(BackfillConversionBatchBlockingIssue::DuplicateRecordInput);
            continue;
        }
        let Some(record) = records_by_id.get(input.record_id.as_str()) else {
            blocking_issues.push(BackfillConversionBatchBlockingIssue::MissingCoverageRecord);
            continue;
        };
        validate_coverage_record(record, selection, &mut blocking_issues);
        validate_execution_plan(record, &input, &mut blocking_issues);
        if let Some(batch_record) = batch_record(record, &input) {
            batch_records.push(batch_record);
        }
    }

    let record_count = batch_records.len() as u64;
    let total_accepted_objects = batch_records
        .iter()
        .map(|record| record.accepted_objects)
        .sum::<u64>();
    let total_accepted_bytes = batch_records
        .iter()
        .map(|record| record.accepted_bytes)
        .sum::<u64>();
    let canonical_ready_records = batch_records
        .iter()
        .filter(|record| record.canonical_ready)
        .count() as u64;

    if record_count > selection.max_records {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::BatchRecordBudgetExceeded);
    }
    if total_accepted_objects > selection.max_accepted_objects {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::BatchObjectBudgetExceeded);
    }
    if total_accepted_bytes > selection.max_accepted_bytes {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::BatchByteBudgetExceeded);
    }
    if selection.require_uniform_source_binding {
        validate_uniform_batch_fields(&batch_records, &mut blocking_issues);
    }
    blocking_issues.sort_unstable_by_key(|issue| format!("{issue:?}"));
    blocking_issues.dedup();

    let status = if blocking_issues.is_empty() {
        BackfillConversionBatchStatus::Ready
    } else {
        BackfillConversionBatchStatus::Blocked
    };
    let records = if status == BackfillConversionBatchStatus::Ready {
        batch_records
    } else {
        Vec::new()
    };

    BackfillConversionBatchPlan {
        schema_version: BACKFILL_CONVERSION_BATCH_PLAN_SCHEMA_VERSION.to_string(),
        batch_id,
        coverage_ledger_id: ledger.ledger_id.clone(),
        status,
        selection: selection.clone(),
        record_count,
        total_accepted_objects,
        total_accepted_bytes,
        canonical_ready_records,
        records,
        blocking_issues,
    }
}

pub fn write_backfill_conversion_batch_plan_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillConversionBatchPlanArtifact, BackfillConversionBatchPlanError> {
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        BackfillConversionBatchPlanError::ReadSpec {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let spec: BackfillConversionBatchPlanSpec = toml::from_str(&spec_text).map_err(|error| {
        BackfillConversionBatchPlanError::ParseSpecToml {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let ledger_path = spec.coverage_ledger_path.display().to_string();
    let ledger_bytes = fs::read(&spec.coverage_ledger_path).map_err(|error| {
        BackfillConversionBatchPlanError::ReadCoverageLedger {
            path: ledger_path.clone(),
            error: error.to_string(),
        }
    })?;
    let ledger: BackfillCoverageLedger =
        serde_json::from_slice(&ledger_bytes).map_err(|error| {
            BackfillConversionBatchPlanError::ParseCoverageLedgerJson {
                path: ledger_path,
                error: error.to_string(),
            }
        })?;
    let mut inputs = Vec::new();
    for spec_input in spec.inputs {
        inputs.push(read_input(spec_input)?);
    }
    let plan =
        evaluate_backfill_conversion_batch_plan(spec.batch_id, &ledger, &spec.selection, inputs);
    write_backfill_conversion_batch_plan(&spec.output_dir, &plan)
}

pub fn write_backfill_conversion_batch_plan(
    output_dir: &Path,
    plan: &BackfillConversionBatchPlan,
) -> Result<BackfillConversionBatchPlanArtifact, BackfillConversionBatchPlanError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        BackfillConversionBatchPlanError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(BACKFILL_CONVERSION_BATCH_PLAN_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_CONVERSION_BATCH_PLAN_FILE,
        plan,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillConversionBatchPlanError::Serialize,
            read_existing_error: |path, error| BackfillConversionBatchPlanError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| {
                BackfillConversionBatchPlanError::ExistingArtifactMismatch { path }
            },
            write_error: |path, error| BackfillConversionBatchPlanError::Write { path, error },
        },
    )?;
    Ok(BackfillConversionBatchPlanArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        record_count: plan.record_count,
    })
}

fn read_input(
    spec_input: BackfillConversionBatchPlanSpecInput,
) -> Result<BackfillConversionBatchInput, BackfillConversionBatchPlanError> {
    let run_spec_hash = file_sha256(&spec_input.run_spec_path).map_err(|error| {
        BackfillConversionBatchPlanError::ReadRunSpec {
            path: spec_input.run_spec_path.display().to_string(),
            error,
        }
    })?;
    let execution_plan_path = spec_input.execution_plan_path.display().to_string();
    let execution_plan_bytes = fs::read(&spec_input.execution_plan_path).map_err(|error| {
        BackfillConversionBatchPlanError::ReadExecutionPlan {
            path: execution_plan_path.clone(),
            error: error.to_string(),
        }
    })?;
    let execution_plan_hash = format!("{:x}", Sha256::digest(&execution_plan_bytes));
    let execution_plan: BackfillExecutionPlan = serde_json::from_slice(&execution_plan_bytes)
        .map_err(
            |error| BackfillConversionBatchPlanError::ParseExecutionPlanJson {
                path: execution_plan_path,
                error: error.to_string(),
            },
        )?;
    Ok(BackfillConversionBatchInput {
        record_id: spec_input.record_id,
        run_spec_path: spec_input.run_spec_path,
        run_spec_hash,
        execution_plan_path: spec_input.execution_plan_path,
        execution_plan_hash,
        execution_plan,
    })
}

fn validate_coverage_record(
    record: &BackfillCoverageRecord,
    selection: &BackfillConversionBatchSelection,
    blocking_issues: &mut Vec<BackfillConversionBatchBlockingIssue>,
) {
    match record.status {
        BackfillCoverageStatus::Accepted => {}
        BackfillCoverageStatus::AcceptedWithGaps if selection.allow_gaps => {}
        BackfillCoverageStatus::AcceptedWithGaps => {
            blocking_issues
                .push(BackfillConversionBatchBlockingIssue::CoverageRecordHasGapsNotAllowed);
        }
        _ => {
            blocking_issues.push(BackfillConversionBatchBlockingIssue::CoverageRecordNotAccepted);
        }
    }
    if !record.blocking_issues.is_empty() {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::CoverageRecordHasBlockingIssues);
    }
    if required_record_string(record.source_binding.as_deref()).is_none()
        || required_record_string(record.table_family.as_deref()).is_none()
        || required_record_string(record.coverage_axis.as_deref()).is_none()
        || required_record_string(record.source_proof_id.as_deref()).is_none()
        || record.source_proof_version.is_none()
    {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::CoverageRecordMissingBinding);
    }
}

fn validate_execution_plan(
    record: &BackfillCoverageRecord,
    input: &BackfillConversionBatchInput,
    blocking_issues: &mut Vec<BackfillConversionBatchBlockingIssue>,
) {
    let execution_plan = &input.execution_plan;
    if execution_plan.status != BackfillExecutionPlanStatus::Ready {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::ExecutionPlanNotReady);
    }
    if execution_plan.accepted_tranche_id != input.record_id
        || execution_plan.accepted_tranche_id != record.record_id
    {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::ExecutionPlanRecordMismatch);
    }
    if execution_plan.run_spec_hash != input.run_spec_hash {
        blocking_issues
            .push(BackfillConversionBatchBlockingIssue::ExecutionPlanRunSpecHashMismatch);
    }
    if record.source_proof_id.as_deref() != Some(execution_plan.source_proof_id.as_str())
        || record.source_proof_version != Some(execution_plan.source_proof_version)
    {
        blocking_issues
            .push(BackfillConversionBatchBlockingIssue::ExecutionPlanSourceProofMismatch);
    }
    if record.source_binding.as_deref() != Some(execution_plan.source_binding.as_str()) {
        blocking_issues
            .push(BackfillConversionBatchBlockingIssue::ExecutionPlanSourceBindingMismatch);
    }
    if record.table_family.as_deref() != Some(execution_plan.table_family.as_str()) {
        blocking_issues
            .push(BackfillConversionBatchBlockingIssue::ExecutionPlanTableFamilyMismatch);
    }
    if record.accepted_objects != execution_plan.object_count {
        blocking_issues
            .push(BackfillConversionBatchBlockingIssue::ExecutionPlanAcceptedObjectCountMismatch);
    }
    if record.accepted_bytes != execution_plan.accepted_bytes {
        blocking_issues
            .push(BackfillConversionBatchBlockingIssue::ExecutionPlanAcceptedBytesMismatch);
    }
}

fn validate_uniform_batch_fields(
    records: &[BackfillConversionBatchRecord],
    blocking_issues: &mut Vec<BackfillConversionBatchBlockingIssue>,
) {
    if has_multiple(records.iter().map(|record| record.source_binding.as_str())) {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::MixedSourceBinding);
    }
    if has_multiple(records.iter().map(|record| record.table_family.as_str())) {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::MixedTableFamily);
    }
    if has_multiple(records.iter().map(|record| record.coverage_axis.as_str())) {
        blocking_issues.push(BackfillConversionBatchBlockingIssue::MixedCoverageAxis);
    }
}

fn batch_record(
    record: &BackfillCoverageRecord,
    input: &BackfillConversionBatchInput,
) -> Option<BackfillConversionBatchRecord> {
    Some(BackfillConversionBatchRecord {
        record_id: record.record_id.clone(),
        source_binding: required_record_string(record.source_binding.as_deref())?.to_string(),
        table_family: required_record_string(record.table_family.as_deref())?.to_string(),
        coverage_axis: required_record_string(record.coverage_axis.as_deref())?.to_string(),
        source_proof_id: required_record_string(record.source_proof_id.as_deref())?.to_string(),
        source_proof_version: record.source_proof_version?,
        canonical_ready: record.canonical_ready,
        accepted_objects: record.accepted_objects,
        accepted_bytes: record.accepted_bytes,
        run_spec_path: input.run_spec_path.clone(),
        run_spec_hash: input.run_spec_hash.clone(),
        execution_plan_path: input.execution_plan_path.clone(),
        execution_plan_hash: input.execution_plan_hash.clone(),
        operator_run_id: input.execution_plan.operator_run_id.clone(),
        output_prefix: input.execution_plan.output_prefix.clone(),
    })
}

fn has_multiple<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut distinct = BTreeSet::new();
    for value in values {
        distinct.insert(value);
        if distinct.len() > 1 {
            return true;
        }
    }
    false
}

fn required_record_string(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(value)
        }
    })
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
