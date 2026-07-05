//! Venue-level backfill conversion completion ledger.
//!
//! This post-conversion artifact binds a ready conversion batch to the concrete
//! publication and NT catalog-mapping evidence for every batch record. It does
//! not perform payload conversion or mutate S3.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    backfill_conversion_batch::{
        BackfillConversionBatchPlan, BackfillConversionBatchRecord, BackfillConversionBatchStatus,
    },
    backfill_execution_plan::BackfillExecutionPlan,
    source_catalog_mapping_readiness::SourceCatalogMappingStatusEntry,
    source_proof::SourceProofUsageScope,
    source_universe_batch_execution::{
        SourceUniverseBatchExecutionReport, SourceUniverseBatchExecutionReportStatus,
    },
};

pub const BACKFILL_CONVERSION_COMPLETION_SCHEMA_VERSION: &str =
    "backfill-conversion-completion-ledger.v1";
pub const BACKFILL_CONVERSION_COMPLETION_LEDGER_FILE: &str =
    "backfill-conversion-completion-ledger.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionCompletionLedgerSpec {
    pub ledger_id: String,
    pub batch_plan_path: PathBuf,
    pub output_dir: PathBuf,
    /// Optional path to the source-universe batch-execution report. When set,
    /// the completion ledger reconciles against the actual run evidence (the
    /// run must have Completed with zero failures and full planned coverage)
    /// before any Ready status is granted.
    #[serde(default)]
    pub batch_execution_report_path: Option<PathBuf>,
    pub requirements: BackfillConversionCompletionRequirements,
    #[serde(rename = "record", default)]
    pub records: Vec<BackfillConversionCompletionRecordSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionCompletionRequirements {
    pub scope_status: String,
    pub usage_scope: SourceProofUsageScope,
    pub current_bte_status: String,
    pub parquet_catalog_status: String,
    pub nt_data_type: String,
    pub fidelity_class: String,
    pub require_direct_s3_catalog_access: bool,
    pub require_publication_verification: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionCompletionRecordSpec {
    pub record_id: String,
    pub publication_evidence_path: PathBuf,
    pub catalog_mapping_evaluation_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillConversionCompletionInput {
    pub record_id: String,
    pub publication_evidence_path: PathBuf,
    pub publication_evidence_hash: String,
    publication_evidence: AcceptedPublicationEvidence,
    pub catalog_mapping_evaluation_path: PathBuf,
    pub catalog_mapping_evaluation_hash: String,
    mapping_entries: Vec<SourceCatalogMappingStatusEntry>,
    pub execution_plan_path: PathBuf,
    pub execution_plan_hash: String,
    pub execution_plan: BackfillExecutionPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillConversionCompletionStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillConversionCompletionBlockingIssue {
    EmptyLedgerId,
    EmptyRecordSet,
    DuplicateRecordInput,
    BatchPlanNotReady,
    MissingBatchRecord,
    UncoveredBatchRecord,
    BatchExecutionReportNotCompleted,
    BatchExecutionReportFailedRecords,
    BatchExecutionReportCompletedCountMismatch,
    ExecutionPlanRecordMismatch,
    ExecutionPlanObjectCountMismatch,
    PublicationScopeStatusMismatch,
    PublicationScopeNotPublished,
    PublicationDirectS3CatalogUnproven,
    PublicationVerificationFailed,
    PublicationRunMismatch,
    PublicationSourceProofMismatch,
    PublicationAcceptedObjectHashMismatch,
    PublicationRowsMismatch,
    PublicationFidelityMismatch,
    PublicationCatalogUriMismatch,
    MappingEntryMissing,
    MappingDuplicateEntries,
    MappingSourceProofMismatch,
    MappingUsageScopeMismatch,
    MappingCurrentBteStatusMismatch,
    MappingParquetCatalogStatusMismatch,
    MappingNtDataTypeMissing,
    MappingPublicationEvidenceMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionCompletionRecord {
    pub record_id: String,
    pub archive_date: String,
    pub source_binding: String,
    pub table_family: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub operator_run_id: String,
    pub output_prefix: String,
    pub accepted_bytes: u64,
    pub accepted_object_sha256: String,
    pub publication_evidence_path: PathBuf,
    pub publication_evidence_hash: String,
    pub catalog_mapping_evaluation_path: PathBuf,
    pub catalog_mapping_evaluation_hash: String,
    pub nt_data_type: String,
    pub fidelity_class: String,
    pub canonical_rows: u64,
    pub catalog_hash: String,
    pub catalog_read_back_trade_ticks: u64,
    pub published_catalog_uri: String,
    pub published_catalog_direct_s3: bool,
    pub published_catalog_expected_iterations: u64,
    pub published_catalog_nt_iterations: u64,
    pub mapping_current_bte_status: String,
    pub mapping_parquet_catalog_status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillConversionCompletionLedger {
    pub schema_version: String,
    pub ledger_id: String,
    pub batch_id: String,
    pub status: BackfillConversionCompletionStatus,
    pub requirements: BackfillConversionCompletionRequirements,
    pub record_count: u64,
    pub published_records: u64,
    pub mapping_proven_records: u64,
    pub total_accepted_bytes: u64,
    pub total_canonical_rows: u64,
    pub total_nt_iterations: u64,
    pub records: Vec<BackfillConversionCompletionRecord>,
    pub blocking_issues: Vec<BackfillConversionCompletionBlockingIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillConversionCompletionLedgerArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub record_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillConversionCompletionLedgerError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadBatchPlan { path: String, error: String },
    ParseBatchPlanJson { path: String, error: String },
    ReadBatchExecutionReport { path: String, error: String },
    ParseBatchExecutionReportJson { path: String, error: String },
    ReadPublicationEvidence { path: String, error: String },
    ParsePublicationEvidenceJson { path: String, error: String },
    ReadCatalogMappingEvaluation { path: String, error: String },
    ParseCatalogMappingEvaluationJson { path: String, error: String },
    ReadExecutionPlan { path: String, error: String },
    ParseExecutionPlanJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillConversionCompletionLedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(
                    f,
                    "read backfill conversion-completion spec {path}: {error}"
                )
            }
            Self::ParseSpecToml { path, error } => {
                write!(
                    f,
                    "parse backfill conversion-completion spec TOML {path}: {error}"
                )
            }
            Self::ReadBatchPlan { path, error } => {
                write!(f, "read backfill conversion-batch plan {path}: {error}")
            }
            Self::ParseBatchPlanJson { path, error } => {
                write!(
                    f,
                    "parse backfill conversion-batch plan JSON {path}: {error}"
                )
            }
            Self::ReadBatchExecutionReport { path, error } => {
                write!(
                    f,
                    "read source-universe batch-execution report {path}: {error}"
                )
            }
            Self::ParseBatchExecutionReportJson { path, error } => {
                write!(
                    f,
                    "parse source-universe batch-execution report JSON {path}: {error}"
                )
            }
            Self::ReadPublicationEvidence { path, error } => {
                write!(f, "read publication evidence {path}: {error}")
            }
            Self::ParsePublicationEvidenceJson { path, error } => {
                write!(f, "parse publication evidence JSON {path}: {error}")
            }
            Self::ReadCatalogMappingEvaluation { path, error } => {
                write!(f, "read catalog mapping evaluation {path}: {error}")
            }
            Self::ParseCatalogMappingEvaluationJson { path, error } => {
                write!(f, "parse catalog mapping evaluation JSON {path}: {error}")
            }
            Self::ReadExecutionPlan { path, error } => {
                write!(
                    f,
                    "read execution plan for conversion completion {path}: {error}"
                )
            }
            Self::ParseExecutionPlanJson { path, error } => {
                write!(f, "parse execution plan JSON {path}: {error}")
            }
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create backfill conversion-completion artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing backfill conversion-completion artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(
                    f,
                    "write backfill conversion-completion artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill conversion-completion artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(
                    f,
                    "serialize backfill conversion-completion artifact: {error}"
                )
            }
        }
    }
}

impl Error for BackfillConversionCompletionLedgerError {}

#[must_use]
pub fn evaluate_backfill_conversion_completion_ledger(
    ledger_id: impl Into<String>,
    batch: &BackfillConversionBatchPlan,
    requirements: &BackfillConversionCompletionRequirements,
    inputs: Vec<BackfillConversionCompletionInput>,
    batch_execution_report: Option<&SourceUniverseBatchExecutionReport>,
) -> BackfillConversionCompletionLedger {
    let ledger_id = ledger_id.into();
    let mut blocking_issues = Vec::new();
    if ledger_id.trim().is_empty() {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::EmptyLedgerId);
    }
    if inputs.is_empty() {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::EmptyRecordSet);
    }
    if batch.status != BackfillConversionBatchStatus::Ready {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::BatchPlanNotReady);
    }

    let batch_records = batch
        .records
        .iter()
        .map(|record| (record.record_id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    let mut completion_records = Vec::new();

    for input in inputs {
        if !seen.insert(input.record_id.clone()) {
            blocking_issues.push(BackfillConversionCompletionBlockingIssue::DuplicateRecordInput);
            continue;
        }
        let Some(batch_record) = batch_records.get(input.record_id.as_str()) else {
            blocking_issues.push(BackfillConversionCompletionBlockingIssue::MissingBatchRecord);
            continue;
        };
        validate_execution_plan(batch_record, &input, &mut blocking_issues);
        validate_publication_evidence(batch_record, requirements, &input, &mut blocking_issues);
        let mapping_entry =
            validate_mapping_evidence(batch_record, requirements, &input, &mut blocking_issues);
        if let Some(record) = completion_record(batch_record, requirements, &input, mapping_entry) {
            completion_records.push(record);
        }
    }

    // Coverage is COMPUTED, not assumed from the input count: a Ready ledger
    // must account for every batch record. Inputs covering only part of the
    // batch keyset (e.g. 10 inputs against a 92-record batch) are blocked, so
    // status can never derive from a partial input set.
    if batch_records
        .keys()
        .any(|record_id| !seen.contains(*record_id))
    {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::UncoveredBatchRecord);
    }

    // Reconcile against the actual batch-execution run evidence when present:
    // the run must have Completed with no failed records, and its completed
    // count must match the full planned batch keyset, before Ready is granted.
    // Without this, a CompletedWithFailures/Failed run (or a partial-scope run)
    // could still yield a Ready completion ledger.
    if let Some(report) = batch_execution_report {
        if report.status != SourceUniverseBatchExecutionReportStatus::Completed {
            blocking_issues
                .push(BackfillConversionCompletionBlockingIssue::BatchExecutionReportNotCompleted);
        }
        if report.failed_record_count != 0 {
            blocking_issues
                .push(BackfillConversionCompletionBlockingIssue::BatchExecutionReportFailedRecords);
        }
        if report.completed_record_count != batch_records.len() as u64 {
            blocking_issues.push(
                BackfillConversionCompletionBlockingIssue::BatchExecutionReportCompletedCountMismatch,
            );
        }
    }

    let record_count = completion_records.len() as u64;
    let published_records = completion_records
        .iter()
        .filter(|record| record.published_catalog_direct_s3)
        .count() as u64;
    let mapping_proven_records = completion_records
        .iter()
        .filter(|record| {
            record.mapping_current_bte_status == requirements.current_bte_status
                && record.mapping_parquet_catalog_status == requirements.parquet_catalog_status
        })
        .count() as u64;
    let total_accepted_bytes = completion_records
        .iter()
        .map(|record| record.accepted_bytes)
        .sum::<u64>();
    let total_canonical_rows = completion_records
        .iter()
        .map(|record| record.canonical_rows)
        .sum::<u64>();
    let total_nt_iterations = completion_records
        .iter()
        .map(|record| record.published_catalog_nt_iterations)
        .sum::<u64>();

    blocking_issues.sort_unstable_by_key(|issue| format!("{issue:?}"));
    blocking_issues.dedup();

    let status = if blocking_issues.is_empty() {
        BackfillConversionCompletionStatus::Ready
    } else {
        BackfillConversionCompletionStatus::Blocked
    };
    let records = if status == BackfillConversionCompletionStatus::Ready {
        completion_records
    } else {
        Vec::new()
    };

    BackfillConversionCompletionLedger {
        schema_version: BACKFILL_CONVERSION_COMPLETION_SCHEMA_VERSION.to_string(),
        ledger_id,
        batch_id: batch.batch_id.clone(),
        status,
        requirements: requirements.clone(),
        record_count,
        published_records,
        mapping_proven_records,
        total_accepted_bytes,
        total_canonical_rows,
        total_nt_iterations,
        records,
        blocking_issues,
    }
}

pub fn write_backfill_conversion_completion_ledger_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillConversionCompletionLedgerArtifact, BackfillConversionCompletionLedgerError> {
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        BackfillConversionCompletionLedgerError::ReadSpec {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let spec: BackfillConversionCompletionLedgerSpec =
        toml::from_str(&spec_text).map_err(|error| {
            BackfillConversionCompletionLedgerError::ParseSpecToml {
                path: spec_path.display().to_string(),
                error: error.to_string(),
            }
        })?;
    let path_base = path_resolution_base(spec_path, &spec.batch_plan_path);
    let batch_plan_path = resolve_path(&path_base, &spec.batch_plan_path);
    let batch_path = spec.batch_plan_path.display().to_string();
    let batch_bytes = fs::read(&batch_plan_path).map_err(|error| {
        BackfillConversionCompletionLedgerError::ReadBatchPlan {
            path: batch_path.clone(),
            error: error.to_string(),
        }
    })?;
    let batch: BackfillConversionBatchPlan =
        serde_json::from_slice(&batch_bytes).map_err(|error| {
            BackfillConversionCompletionLedgerError::ParseBatchPlanJson {
                path: batch_path,
                error: error.to_string(),
            }
        })?;
    let batch_records = batch
        .records
        .iter()
        .map(|record| (record.record_id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut inputs = Vec::new();
    for record in spec.records {
        let batch_record = batch_records.get(record.record_id.as_str());
        inputs.push(read_input(record, batch_record, &path_base)?);
    }
    let batch_execution_report = match &spec.batch_execution_report_path {
        Some(report_path) => {
            let report_display = report_path.display().to_string();
            let resolved_report_path = resolve_path(&path_base, report_path);
            let report_bytes = fs::read(&resolved_report_path).map_err(|error| {
                BackfillConversionCompletionLedgerError::ReadBatchExecutionReport {
                    path: report_display.clone(),
                    error: error.to_string(),
                }
            })?;
            let report: SourceUniverseBatchExecutionReport = serde_json::from_slice(&report_bytes)
                .map_err(|error| {
                    BackfillConversionCompletionLedgerError::ParseBatchExecutionReportJson {
                        path: report_display,
                        error: error.to_string(),
                    }
                })?;
            Some(report)
        }
        None => None,
    };
    let ledger = evaluate_backfill_conversion_completion_ledger(
        spec.ledger_id,
        &batch,
        &spec.requirements,
        inputs,
        batch_execution_report.as_ref(),
    );
    let output_dir = resolve_path(&path_base, &spec.output_dir);
    write_backfill_conversion_completion_ledger(&output_dir, &ledger)
}

pub fn write_backfill_conversion_completion_ledger(
    output_dir: &Path,
    ledger: &BackfillConversionCompletionLedger,
) -> Result<BackfillConversionCompletionLedgerArtifact, BackfillConversionCompletionLedgerError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        BackfillConversionCompletionLedgerError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(BACKFILL_CONVERSION_COMPLETION_LEDGER_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_CONVERSION_COMPLETION_LEDGER_FILE,
        ledger,
        BackfillConversionCompletionLedgerError::Serialize,
        |path, error| BackfillConversionCompletionLedgerError::ReadExisting { path, error },
        |path| BackfillConversionCompletionLedgerError::ExistingArtifactMismatch { path },
        |path, error| BackfillConversionCompletionLedgerError::Write { path, error },
    )?;
    Ok(BackfillConversionCompletionLedgerArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        record_count: ledger.record_count,
    })
}

fn read_input(
    spec: BackfillConversionCompletionRecordSpec,
    batch_record: Option<&&BackfillConversionBatchRecord>,
    path_base: &Path,
) -> Result<BackfillConversionCompletionInput, BackfillConversionCompletionLedgerError> {
    let publication_path = spec.publication_evidence_path.display().to_string();
    let resolved_publication_path = resolve_path(path_base, &spec.publication_evidence_path);
    let publication_bytes = fs::read(&resolved_publication_path).map_err(|error| {
        BackfillConversionCompletionLedgerError::ReadPublicationEvidence {
            path: publication_path.clone(),
            error: error.to_string(),
        }
    })?;
    let publication_evidence_hash = format!("{:x}", Sha256::digest(&publication_bytes));
    let publication_evidence: AcceptedPublicationEvidence =
        serde_json::from_slice(&publication_bytes).map_err(|error| {
            BackfillConversionCompletionLedgerError::ParsePublicationEvidenceJson {
                path: publication_path,
                error: error.to_string(),
            }
        })?;

    let mapping_path = spec.catalog_mapping_evaluation_path.display().to_string();
    let resolved_mapping_path = resolve_path(path_base, &spec.catalog_mapping_evaluation_path);
    let mapping_bytes = fs::read(&resolved_mapping_path).map_err(|error| {
        BackfillConversionCompletionLedgerError::ReadCatalogMappingEvaluation {
            path: mapping_path.clone(),
            error: error.to_string(),
        }
    })?;
    let catalog_mapping_evaluation_hash = format!("{:x}", Sha256::digest(&mapping_bytes));
    let mapping: SourceCatalogMappingEvaluation =
        serde_json::from_slice(&mapping_bytes).map_err(|error| {
            BackfillConversionCompletionLedgerError::ParseCatalogMappingEvaluationJson {
                path: mapping_path,
                error: error.to_string(),
            }
        })?;

    let execution_plan_path = batch_record
        .map(|record| record.execution_plan_path.clone())
        .unwrap_or_default();
    let execution_plan_path_display = execution_plan_path.display().to_string();
    let resolved_execution_plan_path = resolve_path(path_base, &execution_plan_path);
    let execution_plan_bytes = fs::read(&resolved_execution_plan_path).map_err(|error| {
        BackfillConversionCompletionLedgerError::ReadExecutionPlan {
            path: execution_plan_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let execution_plan_hash = format!("{:x}", Sha256::digest(&execution_plan_bytes));
    let execution_plan: BackfillExecutionPlan = serde_json::from_slice(&execution_plan_bytes)
        .map_err(
            |error| BackfillConversionCompletionLedgerError::ParseExecutionPlanJson {
                path: execution_plan_path_display,
                error: error.to_string(),
            },
        )?;

    Ok(BackfillConversionCompletionInput {
        record_id: spec.record_id,
        publication_evidence_path: spec.publication_evidence_path,
        publication_evidence_hash,
        publication_evidence,
        catalog_mapping_evaluation_path: spec.catalog_mapping_evaluation_path,
        catalog_mapping_evaluation_hash,
        mapping_entries: mapping.source_sample_mapping_status,
        execution_plan_path,
        execution_plan_hash,
        execution_plan,
    })
}

fn validate_execution_plan(
    batch_record: &BackfillConversionBatchRecord,
    input: &BackfillConversionCompletionInput,
    blocking_issues: &mut Vec<BackfillConversionCompletionBlockingIssue>,
) {
    if input.execution_plan.accepted_tranche_id != batch_record.record_id
        || input.execution_plan_hash != batch_record.execution_plan_hash
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::ExecutionPlanRecordMismatch);
    }
    if input.execution_plan.objects.len() != 1 {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::ExecutionPlanObjectCountMismatch);
    }
}

fn validate_publication_evidence(
    batch_record: &BackfillConversionBatchRecord,
    requirements: &BackfillConversionCompletionRequirements,
    input: &BackfillConversionCompletionInput,
    blocking_issues: &mut Vec<BackfillConversionCompletionBlockingIssue>,
) {
    let evidence = &input.publication_evidence;
    if evidence.scope.status != requirements.scope_status {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationScopeStatusMismatch);
    }
    if !evidence.scope.accepted_reference_gate_committed
        || !evidence.scope.staged_to_s3
        || !evidence.scope.published_to_s3
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationScopeNotPublished);
    }
    if requirements.require_direct_s3_catalog_access
        && (!evidence.scope.direct_s3_catalog_access_proven
            || !evidence
                .accepted_conversion_and_publication
                .published_catalog_direct_s3)
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationDirectS3CatalogUnproven);
    }
    if requirements.require_publication_verification
        && evidence
            .accepted_conversion_and_publication
            .verification_exit_code()
            != Some(0)
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationVerificationFailed);
    }
    if evidence.accepted_conversion_and_publication.run_id != batch_record.operator_run_id {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::PublicationRunMismatch);
    }
    if evidence.accepted_conversion_and_publication.source_proof_id != batch_record.source_proof_id
        || evidence
            .accepted_conversion_and_publication
            .source_proof_version
            != batch_record.source_proof_version
        || evidence.scope.source_binding != batch_record.source_binding
        || evidence.scope.table_family != batch_record.table_family
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationSourceProofMismatch);
    }
    let expected_hash = input
        .execution_plan
        .objects
        .first()
        .map(|object| object.sha256.as_str());
    if expected_hash
        != Some(
            evidence
                .accepted_conversion_and_publication
                .accepted_object_sha256
                .as_str(),
        )
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationAcceptedObjectHashMismatch);
    }
    if evidence
        .accepted_conversion_and_publication
        .canonical_trades_rows
        != evidence
            .accepted_conversion_and_publication
            .catalog_read_back_trade_ticks
        || evidence
            .accepted_conversion_and_publication
            .canonical_trades_rows
            != evidence
                .accepted_conversion_and_publication
                .published_catalog_expected_iterations
        || evidence
            .accepted_conversion_and_publication
            .canonical_trades_rows
            != evidence
                .accepted_conversion_and_publication
                .published_catalog_nt_iterations
        || evidence
            .accepted_conversion_and_publication
            .canonical_trades_rows
            != evidence
                .accepted_conversion_and_publication
                .nt_result
                .iterations
    {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::PublicationRowsMismatch);
    }
    if evidence.accepted_conversion_and_publication.fidelity_class != requirements.fidelity_class {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationFidelityMismatch);
    }
    let expected_catalog_uri = format!("{}/nt-catalog", batch_record.output_prefix);
    if evidence
        .accepted_conversion_and_publication
        .published_catalog_uri
        != expected_catalog_uri
    {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::PublicationCatalogUriMismatch);
    }
}

fn validate_mapping_evidence<'a>(
    batch_record: &BackfillConversionBatchRecord,
    requirements: &BackfillConversionCompletionRequirements,
    input: &'a BackfillConversionCompletionInput,
    blocking_issues: &mut Vec<BackfillConversionCompletionBlockingIssue>,
) -> Option<&'a SourceCatalogMappingStatusEntry> {
    let matches = input
        .mapping_entries
        .iter()
        .filter(|entry| {
            entry.source_proof_id.as_deref() == Some(batch_record.source_proof_id.as_str())
                && entry.source_proof_version == Some(batch_record.source_proof_version)
                && entry.source_binding == batch_record.source_binding
                && entry.table_family == batch_record.table_family
        })
        .collect::<Vec<_>>();
    let Some(entry) = matches.first().copied() else {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::MappingEntryMissing);
        return None;
    };
    if matches.len() > 1 {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::MappingDuplicateEntries);
    }
    if entry.source_proof_id.as_deref() != Some(batch_record.source_proof_id.as_str())
        || entry.source_proof_version != Some(batch_record.source_proof_version)
    {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::MappingSourceProofMismatch);
    }
    if entry.usage_scope != Some(requirements.usage_scope) {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::MappingUsageScopeMismatch);
    }
    if entry.current_bte_status != requirements.current_bte_status {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::MappingCurrentBteStatusMismatch);
    }
    if entry.parquet_catalog_status != requirements.parquet_catalog_status {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::MappingParquetCatalogStatusMismatch);
    }
    if !entry
        .candidate_nt_data_classes
        .iter()
        .any(|data_class| data_class == &requirements.nt_data_type)
    {
        blocking_issues.push(BackfillConversionCompletionBlockingIssue::MappingNtDataTypeMissing);
    }
    let publication_ref = repo_ref(&input.publication_evidence_path);
    let has_publication_ref = entry
        .nt_data_class_evidence_refs
        .get(&requirements.nt_data_type)
        .is_some_and(|refs| {
            refs.iter()
                .any(|evidence_ref| evidence_ref == &publication_ref)
        });
    if !has_publication_ref {
        blocking_issues
            .push(BackfillConversionCompletionBlockingIssue::MappingPublicationEvidenceMissing);
    }
    Some(entry)
}

fn completion_record(
    batch_record: &BackfillConversionBatchRecord,
    requirements: &BackfillConversionCompletionRequirements,
    input: &BackfillConversionCompletionInput,
    mapping_entry: Option<&SourceCatalogMappingStatusEntry>,
) -> Option<BackfillConversionCompletionRecord> {
    let mapping_entry = mapping_entry?;
    let evidence = &input.publication_evidence;
    Some(BackfillConversionCompletionRecord {
        record_id: batch_record.record_id.clone(),
        archive_date: evidence.scope.archive_date.clone(),
        source_binding: batch_record.source_binding.clone(),
        table_family: batch_record.table_family.clone(),
        source_proof_id: batch_record.source_proof_id.clone(),
        source_proof_version: batch_record.source_proof_version,
        operator_run_id: batch_record.operator_run_id.clone(),
        output_prefix: batch_record.output_prefix.clone(),
        accepted_bytes: batch_record.accepted_bytes,
        accepted_object_sha256: evidence
            .accepted_conversion_and_publication
            .accepted_object_sha256
            .clone(),
        publication_evidence_path: input.publication_evidence_path.clone(),
        publication_evidence_hash: input.publication_evidence_hash.clone(),
        catalog_mapping_evaluation_path: input.catalog_mapping_evaluation_path.clone(),
        catalog_mapping_evaluation_hash: input.catalog_mapping_evaluation_hash.clone(),
        nt_data_type: requirements.nt_data_type.clone(),
        fidelity_class: evidence
            .accepted_conversion_and_publication
            .fidelity_class
            .clone(),
        canonical_rows: evidence
            .accepted_conversion_and_publication
            .canonical_trades_rows,
        catalog_hash: evidence
            .accepted_conversion_and_publication
            .catalog_hash
            .clone(),
        catalog_read_back_trade_ticks: evidence
            .accepted_conversion_and_publication
            .catalog_read_back_trade_ticks,
        published_catalog_uri: evidence
            .accepted_conversion_and_publication
            .published_catalog_uri
            .clone(),
        published_catalog_direct_s3: evidence
            .accepted_conversion_and_publication
            .published_catalog_direct_s3,
        published_catalog_expected_iterations: evidence
            .accepted_conversion_and_publication
            .published_catalog_expected_iterations,
        published_catalog_nt_iterations: evidence
            .accepted_conversion_and_publication
            .published_catalog_nt_iterations,
        mapping_current_bte_status: mapping_entry.current_bte_status.clone(),
        mapping_parquet_catalog_status: mapping_entry.parquet_catalog_status.clone(),
    })
}

fn repo_ref(path: &Path) -> String {
    format!("repo://{}", path.display())
}

fn path_resolution_base(spec_path: &Path, probe_path: &Path) -> PathBuf {
    if probe_path.is_absolute() {
        return PathBuf::from(".");
    }
    if probe_path.exists() {
        return PathBuf::from(".");
    }
    let anchor = spec_path.parent().unwrap_or_else(|| Path::new("."));
    for ancestor in anchor.ancestors() {
        if ancestor.join(probe_path).exists() {
            return ancestor.to_path_buf();
        }
    }
    PathBuf::from(".")
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct SourceCatalogMappingEvaluation {
    source_sample_mapping_status: Vec<SourceCatalogMappingStatusEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AcceptedPublicationEvidence {
    scope: AcceptedPublicationScope,
    accepted_conversion_and_publication: AcceptedConversionAndPublication,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AcceptedPublicationScope {
    status: String,
    source_binding: String,
    table_family: String,
    archive_date: String,
    accepted_reference_gate_committed: bool,
    staged_to_s3: bool,
    published_to_s3: bool,
    direct_s3_catalog_access_proven: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AcceptedConversionAndPublication {
    publication_verification_exit_code: Option<i32>,
    exit_code: Option<i32>,
    run_id: String,
    source_proof_id: String,
    source_proof_version: u32,
    accepted_object_sha256: String,
    canonical_trades_rows: u64,
    catalog_hash: String,
    catalog_read_back_trade_ticks: u64,
    published_catalog_uri: String,
    published_catalog_direct_s3: bool,
    published_catalog_expected_iterations: u64,
    published_catalog_nt_iterations: u64,
    nt_result: AcceptedPublicationNtResult,
    fidelity_class: String,
}

impl AcceptedConversionAndPublication {
    fn verification_exit_code(&self) -> Option<i32> {
        self.publication_verification_exit_code.or(self.exit_code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct AcceptedPublicationNtResult {
    iterations: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backfill_conversion_batch::{
        BACKFILL_CONVERSION_BATCH_PLAN_SCHEMA_VERSION, BackfillConversionBatchSelection,
    };
    use crate::source_universe_batch_execution::{
        SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION, SourceUniverseBatchExecutionReport,
        SourceUniverseBatchExecutionReportStatus,
    };

    fn batch_record(record_id: &str) -> BackfillConversionBatchRecord {
        BackfillConversionBatchRecord {
            record_id: record_id.to_string(),
            source_binding: "test-source-binding".to_string(),
            table_family: "trades".to_string(),
            coverage_axis: "test-axis".to_string(),
            source_proof_id: "test-source-proof".to_string(),
            source_proof_version: 1,
            canonical_ready: true,
            accepted_objects: 1,
            accepted_bytes: 1,
            run_spec_path: PathBuf::from("run-spec.toml"),
            run_spec_hash: "test-run-spec-hash".to_string(),
            execution_plan_path: PathBuf::from("execution-plan.json"),
            execution_plan_hash: "test-execution-plan-hash".to_string(),
            operator_run_id: "test-operator-run".to_string(),
            output_prefix: "test/output/prefix".to_string(),
        }
    }

    fn ready_batch_plan(record_ids: &[&str]) -> BackfillConversionBatchPlan {
        let records: Vec<_> = record_ids.iter().map(|id| batch_record(id)).collect();
        BackfillConversionBatchPlan {
            schema_version: BACKFILL_CONVERSION_BATCH_PLAN_SCHEMA_VERSION.to_string(),
            batch_id: "test-batch".to_string(),
            coverage_ledger_id: "test-coverage-ledger".to_string(),
            status: BackfillConversionBatchStatus::Ready,
            selection: BackfillConversionBatchSelection {
                max_records: 100,
                max_accepted_objects: 100,
                max_accepted_bytes: 100,
                require_uniform_source_binding: true,
                allow_gaps: false,
            },
            record_count: records.len() as u64,
            total_accepted_objects: records.len() as u64,
            total_accepted_bytes: records.len() as u64,
            canonical_ready_records: records.len() as u64,
            records,
            blocking_issues: Vec::new(),
        }
    }

    fn requirements() -> BackfillConversionCompletionRequirements {
        BackfillConversionCompletionRequirements {
            scope_status: "accepted".to_string(),
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            current_bte_status: "accepted".to_string(),
            parquet_catalog_status: "proven".to_string(),
            nt_data_type: "TradeTick".to_string(),
            fidelity_class: "TRADE_REPLAY".to_string(),
            require_direct_s3_catalog_access: true,
            require_publication_verification: true,
        }
    }

    fn batch_execution_report(
        status: SourceUniverseBatchExecutionReportStatus,
        completed_record_count: u64,
        failed_record_count: u64,
    ) -> SourceUniverseBatchExecutionReport {
        SourceUniverseBatchExecutionReport {
            schema_version: SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION.to_string(),
            batch_id: "test-batch".to_string(),
            status,
            pack_id: "test-pack".to_string(),
            universe_id: "test-universe".to_string(),
            venue: "test-venue".to_string(),
            selected_record_count: completed_record_count + failed_record_count,
            completed_record_count,
            failed_record_count,
            total_canonical_rows: 0,
            total_nt_catalog_rows: 0,
            records: Vec::new(),
            failures: Vec::new(),
        }
    }

    // F3: inputs covering fewer records than the batch keyset must block on an
    // uncovered batch record; status must derive from coverage, not the input
    // count, so a partial input set can never yield Ready.
    #[test]
    fn partial_input_coverage_blocks_on_uncovered_batch_record() {
        let batch = ready_batch_plan(&["record-a", "record-b"]);
        let ledger = evaluate_backfill_conversion_completion_ledger(
            "test-ledger",
            &batch,
            &requirements(),
            Vec::new(),
            None,
        );
        assert!(
            ledger
                .blocking_issues
                .contains(&BackfillConversionCompletionBlockingIssue::UncoveredBatchRecord),
            "expected UncoveredBatchRecord blocker, got: {:?}",
            ledger.blocking_issues
        );
        assert_eq!(ledger.status, BackfillConversionCompletionStatus::Blocked);
    }

    // F18: a batch-execution run that did not cleanly complete (status other
    // than Completed, or any failed records) must block the completion ledger
    // before any Ready status is granted.
    #[test]
    fn completed_with_failures_report_blocks_completion_ledger() {
        let batch = ready_batch_plan(&["record-a"]);
        let report = batch_execution_report(
            SourceUniverseBatchExecutionReportStatus::CompletedWithFailures,
            1,
            1,
        );
        let ledger = evaluate_backfill_conversion_completion_ledger(
            "test-ledger",
            &batch,
            &requirements(),
            Vec::new(),
            Some(&report),
        );
        assert!(
            ledger.blocking_issues.contains(
                &BackfillConversionCompletionBlockingIssue::BatchExecutionReportNotCompleted
            ),
            "expected BatchExecutionReportNotCompleted blocker, got: {:?}",
            ledger.blocking_issues
        );
        assert!(
            ledger.blocking_issues.contains(
                &BackfillConversionCompletionBlockingIssue::BatchExecutionReportFailedRecords
            ),
            "expected BatchExecutionReportFailedRecords blocker, got: {:?}",
            ledger.blocking_issues
        );
        assert_eq!(ledger.status, BackfillConversionCompletionStatus::Blocked);
    }

    // F18 negative control: a clean Completed report covering every planned
    // batch record contributes no reconciliation blockers.
    #[test]
    fn completed_report_with_full_coverage_adds_no_reconciliation_blockers() {
        let batch = ready_batch_plan(&["record-a"]);
        let report =
            batch_execution_report(SourceUniverseBatchExecutionReportStatus::Completed, 1, 0);
        let ledger = evaluate_backfill_conversion_completion_ledger(
            "test-ledger",
            &batch,
            &requirements(),
            Vec::new(),
            Some(&report),
        );
        assert!(
            !ledger.blocking_issues.iter().any(|issue| matches!(
                issue,
                BackfillConversionCompletionBlockingIssue::BatchExecutionReportNotCompleted
                    | BackfillConversionCompletionBlockingIssue::BatchExecutionReportFailedRecords
                    | BackfillConversionCompletionBlockingIssue::BatchExecutionReportCompletedCountMismatch
            )),
            "clean report must add no reconciliation blockers, got: {:?}",
            ledger.blocking_issues
        );
    }
}
