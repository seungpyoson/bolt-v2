//! Accepted object-level tranche manifest.
//!
//! This artifact is the narrow, source-proof-bound unit that a converter may
//! consume. It intentionally does not promote a broader parent run manifest.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::backfill_source_proof_scope::{
    BackfillSourceProofScopeReport, BackfillSourceProofScopeStatus,
};
use crate::reference_artifact::{resolve_spec_path, spec_path_resolution_base};
use crate::source_proof::SourceProofUsageScope;

pub const BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION: &str = "backfill-accepted-tranche-manifest.v1";
pub const BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE: &str = "backfill-accepted-tranche-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillAcceptedTrancheSpec {
    pub tranche_id: String,
    pub source_proof_scope_report_path: PathBuf,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillAcceptedTrancheStatus {
    Accepted,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillAcceptedTrancheIssue {
    EmptyTrancheId,
    SourceProofScopeNotCandidate,
    SourceProofScopeHasBlockingIssues,
    MissingSelectedObject,
    MatchingObjectCountNotOne,
    AcceptedScopeNotSingleObject,
    AcceptedBytesMismatchSelectedObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillAcceptedTrancheObject {
    pub s3_uri: String,
    pub source_url: String,
    pub sha256: String,
    pub bytes: u64,
    pub archive_date: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_row_groups: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillAcceptedTrancheManifest {
    pub schema_version: String,
    pub tranche_id: String,
    pub status: BackfillAcceptedTrancheStatus,
    pub source_proof_scope_report_id: String,
    pub source_proof_scope_report_hash: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub table_family: String,
    #[serde(
        default = "default_source_usage_scope",
        skip_serializing_if = "is_canonical_backfill_input"
    )]
    pub source_usage_scope: SourceProofUsageScope,
    pub parent_manifest_id: String,
    pub object_level_tranche_required: bool,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub objects: Vec<BackfillAcceptedTrancheObject>,
    pub blocking_issues: Vec<BackfillAcceptedTrancheIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillAcceptedTrancheArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillAcceptedTrancheError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadSourceProofScopeReport { path: String, error: String },
    ParseSourceProofScopeReportJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillAcceptedTrancheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill accepted-tranche spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse backfill accepted-tranche spec TOML {path}: {error}"
            ),
            Self::ReadSourceProofScopeReport { path, error } => {
                write!(f, "read backfill source-proof scope report {path}: {error}")
            }
            Self::ParseSourceProofScopeReportJson { path, error } => write!(
                f,
                "parse backfill source-proof scope report JSON {path}: {error}"
            ),
            Self::CreateDir { path, error } => write!(
                f,
                "create backfill accepted-tranche artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing backfill accepted-tranche artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(
                    f,
                    "write backfill accepted-tranche artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill accepted-tranche artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize backfill accepted-tranche artifact: {error}")
            }
        }
    }
}

impl Error for BackfillAcceptedTrancheError {}

pub fn evaluate_backfill_accepted_tranche(
    tranche_id: impl Into<String>,
    source_proof_scope: &BackfillSourceProofScopeReport,
    source_proof_scope_report_hash: &str,
) -> Result<BackfillAcceptedTrancheManifest, BackfillAcceptedTrancheError> {
    Ok(evaluate_backfill_accepted_tranche_report(
        tranche_id.into(),
        source_proof_scope,
        source_proof_scope_report_hash.to_string(),
    ))
}

pub fn write_backfill_accepted_tranche_manifest_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillAcceptedTrancheArtifact, BackfillAcceptedTrancheError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| BackfillAcceptedTrancheError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: BackfillAcceptedTrancheSpec = toml::from_str(&spec_text).map_err(|error| {
        BackfillAcceptedTrancheError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        }
    })?;
    let path_base = spec_path_resolution_base(spec_path, &spec.source_proof_scope_report_path);
    let report_path = spec.source_proof_scope_report_path.display().to_string();
    let resolved_report_path = resolve_spec_path(&path_base, &spec.source_proof_scope_report_path);
    let report_bytes = fs::read(&resolved_report_path).map_err(|error| {
        BackfillAcceptedTrancheError::ReadSourceProofScopeReport {
            path: report_path.clone(),
            error: error.to_string(),
        }
    })?;
    let report: BackfillSourceProofScopeReport =
        serde_json::from_slice(&report_bytes).map_err(|error| {
            BackfillAcceptedTrancheError::ParseSourceProofScopeReportJson {
                path: report_path,
                error: error.to_string(),
            }
        })?;
    let report_hash = format!("{:x}", Sha256::digest(&report_bytes));
    let manifest = evaluate_backfill_accepted_tranche_report(spec.tranche_id, &report, report_hash);
    let output_dir = resolve_spec_path(&path_base, &spec.output_dir);
    write_backfill_accepted_tranche_manifest(&output_dir, &manifest)
}

pub fn write_backfill_accepted_tranche_manifest(
    output_dir: &Path,
    manifest: &BackfillAcceptedTrancheManifest,
) -> Result<BackfillAcceptedTrancheArtifact, BackfillAcceptedTrancheError> {
    fs::create_dir_all(output_dir).map_err(|error| BackfillAcceptedTrancheError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_ACCEPTED_TRANCHE_MANIFEST_FILE,
        manifest,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillAcceptedTrancheError::Serialize,
            read_existing_error: |path, error| BackfillAcceptedTrancheError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| BackfillAcceptedTrancheError::ExistingArtifactMismatch { path },
            write_error: |path, error| BackfillAcceptedTrancheError::Write { path, error },
        },
    )?;
    Ok(BackfillAcceptedTrancheArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

fn evaluate_backfill_accepted_tranche_report(
    tranche_id: String,
    source_proof_scope: &BackfillSourceProofScopeReport,
    source_proof_scope_report_hash: String,
) -> BackfillAcceptedTrancheManifest {
    let selected = source_proof_scope.selected_object.as_ref();
    let mut blocking_issues = Vec::new();
    if tranche_id.trim().is_empty() {
        blocking_issues.push(BackfillAcceptedTrancheIssue::EmptyTrancheId);
    }
    if source_proof_scope.status != BackfillSourceProofScopeStatus::CandidateFound {
        blocking_issues.push(BackfillAcceptedTrancheIssue::SourceProofScopeNotCandidate);
    }
    if !source_proof_scope.blocking_issues.is_empty() {
        blocking_issues.push(BackfillAcceptedTrancheIssue::SourceProofScopeHasBlockingIssues);
    }
    if selected.is_none() {
        blocking_issues.push(BackfillAcceptedTrancheIssue::MissingSelectedObject);
    }
    if source_proof_scope.matching_object_count != 1 {
        blocking_issues.push(BackfillAcceptedTrancheIssue::MatchingObjectCountNotOne);
    }
    if !source_proof_scope.object_level_tranche_required
        && source_proof_scope.accepted_scope_completed_objects != 1
    {
        blocking_issues.push(BackfillAcceptedTrancheIssue::AcceptedScopeNotSingleObject);
    }
    if !source_proof_scope.object_level_tranche_required
        && selected
            .is_some_and(|object| object.bytes != source_proof_scope.accepted_scope_accepted_bytes)
    {
        blocking_issues.push(BackfillAcceptedTrancheIssue::AcceptedBytesMismatchSelectedObject);
    }

    let status = if blocking_issues.is_empty() {
        BackfillAcceptedTrancheStatus::Accepted
    } else {
        BackfillAcceptedTrancheStatus::Blocked
    };
    let objects = if status == BackfillAcceptedTrancheStatus::Accepted {
        selected
            .map(|object| {
                vec![BackfillAcceptedTrancheObject {
                    s3_uri: object.s3_uri.clone(),
                    source_url: object.source_url.clone(),
                    sha256: object.sha256.clone(),
                    bytes: object.bytes,
                    archive_date: object.archive_date.clone(),
                    source_row_groups: object.source_row_groups.clone(),
                    predicate_ref: object.predicate_ref.clone(),
                }]
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    BackfillAcceptedTrancheManifest {
        schema_version: BACKFILL_ACCEPTED_TRANCHE_SCHEMA_VERSION.to_string(),
        tranche_id,
        status,
        source_proof_scope_report_id: source_proof_scope.report_id.clone(),
        source_proof_scope_report_hash,
        source_proof_id: source_proof_scope.source_proof_id.clone(),
        source_proof_version: source_proof_scope.source_proof_version,
        source_binding: source_proof_scope.source_binding.clone(),
        table_family: source_proof_scope.table_family.clone(),
        source_usage_scope: source_proof_scope.source_usage_scope,
        parent_manifest_id: source_proof_scope.manifest_id.clone(),
        object_level_tranche_required: source_proof_scope.object_level_tranche_required,
        object_count: objects.len() as u64,
        accepted_bytes: objects.iter().map(|object| object.bytes).sum(),
        objects,
        blocking_issues,
    }
}

fn default_source_usage_scope() -> SourceProofUsageScope {
    SourceProofUsageScope::CanonicalBackfillInput
}

fn is_canonical_backfill_input(value: &SourceProofUsageScope) -> bool {
    *value == SourceProofUsageScope::CanonicalBackfillInput
}
