//! Combined backfill readiness gate.
//!
//! This report-only gate joins the coverage-ledger preflight with the
//! source-proof migration preflight so the next operator step cannot consider
//! a path ready unless both required preconditions are true.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    backfill_binding_coverage::{BackfillBindingCoverageReport, BackfillBindingCoverageStatus},
    backfill_preflight::{
        BackfillPreflightReport, BackfillPreflightSelectedRecord, BackfillPreflightStatus,
    },
    source_proof_migration_preflight::{
        SourceProofMigrationPreflightCandidate, SourceProofMigrationPreflightReport,
        SourceProofMigrationPreflightStatus,
    },
};

pub const BACKFILL_READINESS_SCHEMA_VERSION: &str = "backfill-readiness-report.v1";
pub const BACKFILL_READINESS_REPORT_FILE: &str = "backfill-readiness-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillReadinessSpec {
    pub readiness_id: String,
    pub backfill_preflight_report_path: PathBuf,
    pub source_proof_migration_preflight_report_path: PathBuf,
    pub backfill_binding_coverage_report_path: PathBuf,
    pub output_dir: PathBuf,
    pub required_table_family: String,
    pub required_nt_data_type: String,
    pub supported_data_paths: Vec<BackfillReadinessSupportedDataPath>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillReadinessSupportedDataPath {
    pub table_family: String,
    pub nt_data_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillReadinessStatus {
    Ready,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillReadinessBlocker {
    EmptyReadinessId,
    EmptyRequiredTableFamily,
    EmptyRequiredNtDataType,
    EmptySupportedDataPaths,
    UnsupportedRequiredNtDataType,
    UnsupportedRequiredTableFamilyDataType,
    BackfillPreflightBlocked,
    SourceProofMigrationPreflightBlocked,
    BackfillBindingCoverageBlocked,
    MissingSelectedBackfillRecord,
    MissingSelectedSourceProofCandidate,
    SourceProofTableFamilyMismatch,
    SelectedBackfillTableFamilyMismatch,
    SelectedSourceBindingMismatch,
    SelectedSourceBindingMissingFromCoverage,
    SelectedSourceProofMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillReadinessReport {
    pub schema_version: String,
    pub readiness_id: String,
    pub status: BackfillReadinessStatus,
    pub required_table_family: String,
    pub required_nt_data_type: String,
    pub supported_data_paths: Vec<BackfillReadinessSupportedDataPath>,
    pub backfill_preflight_id: String,
    pub backfill_preflight_status: BackfillPreflightStatus,
    pub source_proof_migration_preflight_id: String,
    pub source_proof_migration_preflight_status: SourceProofMigrationPreflightStatus,
    pub backfill_binding_coverage_id: String,
    pub backfill_binding_coverage_status: BackfillBindingCoverageStatus,
    pub selected_backfill_record: Option<BackfillPreflightSelectedRecord>,
    pub selected_source_proof_candidate: Option<SourceProofMigrationPreflightCandidate>,
    pub blockers: Vec<BackfillReadinessBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillReadinessArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillReadinessError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadBackfillPreflight { path: String, error: String },
    ParseBackfillPreflightJson { path: String, error: String },
    ReadSourceProofPreflight { path: String, error: String },
    ParseSourceProofPreflightJson { path: String, error: String },
    ReadBackfillBindingCoverage { path: String, error: String },
    ParseBackfillBindingCoverageJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillReadinessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill readiness spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(f, "parse backfill readiness spec TOML {path}: {error}")
            }
            Self::ReadBackfillPreflight { path, error } => {
                write!(f, "read backfill preflight report {path}: {error}")
            }
            Self::ParseBackfillPreflightJson { path, error } => {
                write!(f, "parse backfill preflight report JSON {path}: {error}")
            }
            Self::ReadSourceProofPreflight { path, error } => {
                write!(
                    f,
                    "read source-proof migration preflight report {path}: {error}"
                )
            }
            Self::ParseSourceProofPreflightJson { path, error } => write!(
                f,
                "parse source-proof migration preflight report JSON {path}: {error}"
            ),
            Self::ReadBackfillBindingCoverage { path, error } => {
                write!(f, "read backfill binding coverage report {path}: {error}")
            }
            Self::ParseBackfillBindingCoverageJson { path, error } => write!(
                f,
                "parse backfill binding coverage report JSON {path}: {error}"
            ),
            Self::CreateDir { path, error } => {
                write!(
                    f,
                    "create backfill readiness artifact directory {path}: {error}"
                )
            }
            Self::ReadExisting { path, error } => {
                write!(
                    f,
                    "read existing backfill readiness artifact {path}: {error}"
                )
            }
            Self::Write { path, error } => {
                write!(f, "write backfill readiness artifact {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill readiness artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => write!(f, "serialize backfill readiness artifact: {error}"),
        }
    }
}

impl Error for BackfillReadinessError {}

pub fn evaluate_backfill_readiness(
    readiness_id: impl Into<String>,
    backfill_preflight: BackfillPreflightReport,
    source_proof_preflight: SourceProofMigrationPreflightReport,
    binding_coverage: BackfillBindingCoverageReport,
    required_table_family: impl Into<String>,
    required_nt_data_type: impl Into<String>,
    supported_data_paths: Vec<BackfillReadinessSupportedDataPath>,
) -> BackfillReadinessReport {
    let readiness_id = readiness_id.into();
    let required_table_family = required_table_family.into();
    let required_nt_data_type = required_nt_data_type.into();
    let mut blockers = Vec::new();

    if readiness_id.trim().is_empty() {
        blockers.push(BackfillReadinessBlocker::EmptyReadinessId);
    }
    if required_table_family.trim().is_empty() {
        blockers.push(BackfillReadinessBlocker::EmptyRequiredTableFamily);
    }
    let required_table_family_trimmed = required_table_family.trim();
    let required_nt_data_type_trimmed = required_nt_data_type.trim();
    if supported_data_paths.is_empty() {
        blockers.push(BackfillReadinessBlocker::EmptySupportedDataPaths);
    }
    if required_nt_data_type_trimmed.is_empty() {
        blockers.push(BackfillReadinessBlocker::EmptyRequiredNtDataType);
    } else if !supported_data_paths
        .iter()
        .any(|path| path.nt_data_type.trim() == required_nt_data_type_trimmed)
    {
        blockers.push(BackfillReadinessBlocker::UnsupportedRequiredNtDataType);
    } else if !supported_data_paths.iter().any(|path| {
        path.table_family.trim() == required_table_family_trimmed
            && path.nt_data_type.trim() == required_nt_data_type_trimmed
    }) {
        blockers.push(BackfillReadinessBlocker::UnsupportedRequiredTableFamilyDataType);
    }
    if backfill_preflight.status != BackfillPreflightStatus::Go
        || !backfill_preflight.blocking_reasons.is_empty()
        || !backfill_preflight.selection.require_canonical_ready
    {
        blockers.push(BackfillReadinessBlocker::BackfillPreflightBlocked);
    }
    if source_proof_preflight.status != SourceProofMigrationPreflightStatus::CandidateFound
        || !source_proof_preflight.blocking_reasons.is_empty()
    {
        blockers.push(BackfillReadinessBlocker::SourceProofMigrationPreflightBlocked);
    }
    if binding_coverage.status != BackfillBindingCoverageStatus::Ready
        || !binding_coverage.blocking_issues.is_empty()
    {
        blockers.push(BackfillReadinessBlocker::BackfillBindingCoverageBlocked);
    }
    if backfill_preflight.selected_record.is_none() {
        blockers.push(BackfillReadinessBlocker::MissingSelectedBackfillRecord);
    }
    if backfill_preflight
        .selected_record
        .as_ref()
        .is_some_and(|record| !record.canonical_ready)
    {
        blockers.push(BackfillReadinessBlocker::BackfillPreflightBlocked);
    }
    if backfill_preflight
        .selected_record
        .as_ref()
        .is_some_and(|record| record.table_family != required_table_family_trimmed)
    {
        blockers.push(BackfillReadinessBlocker::SelectedBackfillTableFamilyMismatch);
    }
    match source_proof_preflight.selected_candidate.as_ref() {
        None => blockers.push(BackfillReadinessBlocker::MissingSelectedSourceProofCandidate),
        Some(candidate) if candidate.table_family != required_table_family => {
            blockers.push(BackfillReadinessBlocker::SourceProofTableFamilyMismatch);
        }
        Some(candidate) if !candidate.remaining_acceptance_blockers.is_empty() => {
            blockers.push(BackfillReadinessBlocker::SourceProofMigrationPreflightBlocked);
        }
        Some(_) => {}
    }
    let selected_backfill_binding = backfill_preflight
        .selected_record
        .as_ref()
        .map(|record| record.source_binding.as_str());
    let selected_source_proof_binding = source_proof_preflight
        .selected_candidate
        .as_ref()
        .map(|candidate| candidate.source_binding.as_str());
    if let (Some(backfill_binding), Some(source_proof_binding)) =
        (selected_backfill_binding, selected_source_proof_binding)
        && backfill_binding != source_proof_binding
    {
        blockers.push(BackfillReadinessBlocker::SelectedSourceBindingMismatch);
    }
    if let Some(source_binding) = selected_backfill_binding {
        let coverage_has_selected_binding = binding_coverage.bindings.iter().any(|binding| {
            binding.key == source_binding
                && binding
                    .table_families
                    .iter()
                    .any(|family| family == &required_table_family)
                && binding.ledger_record_count > 0
                && binding.canonical_ready_record_count > 0
                && binding.accepted_record_count > 0
        });
        if !coverage_has_selected_binding {
            blockers.push(BackfillReadinessBlocker::SelectedSourceBindingMissingFromCoverage);
        }
    }
    if let (Some(backfill_record), Some(source_proof_candidate)) = (
        backfill_preflight.selected_record.as_ref(),
        source_proof_preflight.selected_candidate.as_ref(),
    ) && (backfill_record.source_proof_id != source_proof_candidate.source_proof_id
        || backfill_record.source_proof_version != source_proof_candidate.source_proof_version)
    {
        blockers.push(BackfillReadinessBlocker::SelectedSourceProofMismatch);
    }

    let status = if blockers.is_empty() {
        BackfillReadinessStatus::Ready
    } else {
        BackfillReadinessStatus::Blocked
    };

    BackfillReadinessReport {
        schema_version: BACKFILL_READINESS_SCHEMA_VERSION.to_string(),
        readiness_id,
        status,
        required_table_family,
        required_nt_data_type,
        supported_data_paths,
        backfill_preflight_id: backfill_preflight.preflight_id.clone(),
        backfill_preflight_status: backfill_preflight.status,
        source_proof_migration_preflight_id: source_proof_preflight.preflight_id.clone(),
        source_proof_migration_preflight_status: source_proof_preflight.status,
        backfill_binding_coverage_id: binding_coverage.report_id.clone(),
        backfill_binding_coverage_status: binding_coverage.status,
        selected_backfill_record: backfill_preflight.selected_record,
        selected_source_proof_candidate: source_proof_preflight.selected_candidate,
        blockers,
    }
}

pub fn write_backfill_readiness_report(
    output_dir: &Path,
    report: &BackfillReadinessReport,
) -> Result<BackfillReadinessArtifact, BackfillReadinessError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillReadinessError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(BACKFILL_READINESS_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_READINESS_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillReadinessError::Serialize,
            read_existing_error: |path, error| BackfillReadinessError::ReadExisting { path, error },
            mismatch_error: |path| BackfillReadinessError::ExistingArtifactMismatch { path },
            write_error: |path, error| BackfillReadinessError::Write { path, error },
        },
    )?;
    Ok(BackfillReadinessArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

pub fn write_backfill_readiness_report_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillReadinessArtifact, BackfillReadinessError> {
    let path = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| BackfillReadinessError::ReadSpec {
            path: path.clone(),
            error: error.to_string(),
        })?;
    let spec: BackfillReadinessSpec =
        toml::from_str(&spec_text).map_err(|error| BackfillReadinessError::ParseSpecToml {
            path: path.clone(),
            error: error.to_string(),
        })?;
    let backfill_preflight = read_backfill_preflight(&spec.backfill_preflight_report_path)?;
    let source_proof_preflight =
        read_source_proof_preflight(&spec.source_proof_migration_preflight_report_path)?;
    let binding_coverage =
        read_backfill_binding_coverage(&spec.backfill_binding_coverage_report_path)?;
    let report = evaluate_backfill_readiness(
        spec.readiness_id,
        backfill_preflight,
        source_proof_preflight,
        binding_coverage,
        spec.required_table_family,
        spec.required_nt_data_type,
        spec.supported_data_paths,
    );
    write_backfill_readiness_report(&spec.output_dir, &report)
}

fn read_backfill_preflight(path: &Path) -> Result<BackfillPreflightReport, BackfillReadinessError> {
    let path_display = path.display().to_string();
    let bytes = fs::read(path).map_err(|error| BackfillReadinessError::ReadBackfillPreflight {
        path: path_display.clone(),
        error: error.to_string(),
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        BackfillReadinessError::ParseBackfillPreflightJson {
            path: path_display,
            error: error.to_string(),
        }
    })
}

fn read_source_proof_preflight(
    path: &Path,
) -> Result<SourceProofMigrationPreflightReport, BackfillReadinessError> {
    let path_display = path.display().to_string();
    let bytes =
        fs::read(path).map_err(|error| BackfillReadinessError::ReadSourceProofPreflight {
            path: path_display.clone(),
            error: error.to_string(),
        })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        BackfillReadinessError::ParseSourceProofPreflightJson {
            path: path_display,
            error: error.to_string(),
        }
    })
}

fn read_backfill_binding_coverage(
    path: &Path,
) -> Result<BackfillBindingCoverageReport, BackfillReadinessError> {
    let path_display = path.display().to_string();
    let bytes =
        fs::read(path).map_err(
            |error| BackfillReadinessError::ReadBackfillBindingCoverage {
                path: path_display.clone(),
                error: error.to_string(),
            },
        )?;
    serde_json::from_slice(&bytes).map_err(|error| {
        BackfillReadinessError::ParseBackfillBindingCoverageJson {
            path: path_display,
            error: error.to_string(),
        }
    })
}
