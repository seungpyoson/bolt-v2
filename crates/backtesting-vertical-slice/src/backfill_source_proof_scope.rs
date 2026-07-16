//! Object-level source-proof scope over a backfill manifest.
//!
//! This report-only gate prevents a run-level manifest from being bound to an
//! accepted proof when the proof authorizes only a narrower object tranche.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::source_proof::{
    AcceptanceScope, SourceBindingRegistry, SourceProofReport, SourceProofStatus,
    SourceProofUsageScope,
};
use crate::{
    path_resolution::resolve_output_dir,
    retired_backfill_evidence::{
        active_backfill_runtime_output_path, read_active_backfill_runtime_input,
    },
};

pub const BACKFILL_SOURCE_PROOF_SCOPE_SCHEMA_VERSION: &str =
    "backfill-source-proof-scope-report.v1";
pub const BACKFILL_SOURCE_PROOF_SCOPE_REPORT_FILE: &str = "backfill-source-proof-scope-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillSourceProofScopeSpec {
    pub report_id: String,
    pub source_bindings_path: PathBuf,
    pub source_proof_path: PathBuf,
    pub manifest_path: PathBuf,
    #[serde(default)]
    pub selected_object_uri: Option<String>,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillSourceProofScopeStatus {
    CandidateFound,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillSourceProofScopeIssue {
    EmptyReportId,
    SourceProofNotAccepted,
    SourceProofAcceptanceFailed,
    MissingAcceptanceScope,
    NoManifestPayloadObjects,
    NoMatchingManifestObject,
    MultipleMatchingManifestObjects,
    AcceptanceScopeDoesNotCoverManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillSourceProofScopeObject {
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
pub struct BackfillSourceProofScopeReport {
    pub schema_version: String,
    pub report_id: String,
    pub status: BackfillSourceProofScopeStatus,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_binding: String,
    pub table_family: String,
    #[serde(
        default = "default_source_usage_scope",
        skip_serializing_if = "is_canonical_backfill_input"
    )]
    pub source_usage_scope: SourceProofUsageScope,
    pub manifest_id: String,
    pub accepted_scope_completed_objects: u64,
    pub accepted_scope_accepted_bytes: u64,
    pub manifest_payload_object_count: u64,
    pub matching_object_count: u64,
    pub object_level_tranche_required: bool,
    pub selected_object: Option<BackfillSourceProofScopeObject>,
    pub source_proof_acceptance_error: Option<String>,
    pub blocking_issues: Vec<BackfillSourceProofScopeIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillSourceProofScopeArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillSourceProofScopeError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadSourceProof { path: String, error: String },
    ParseSourceProofJson { path: String, error: String },
    ReadManifest { path: String, error: String },
    ParseManifestJson { path: String, error: String },
    ReadSourceBindings { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Serialize(String),
}

impl fmt::Display for BackfillSourceProofScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill source-proof scope spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse backfill source-proof scope spec TOML {path}: {error}"
            ),
            Self::ReadSourceProof { path, error } => {
                write!(f, "read source proof {path}: {error}")
            }
            Self::ParseSourceProofJson { path, error } => {
                write!(f, "parse source proof JSON {path}: {error}")
            }
            Self::ReadManifest { path, error } => {
                write!(f, "read backfill manifest {path}: {error}")
            }
            Self::ParseManifestJson { path, error } => {
                write!(f, "parse backfill manifest JSON {path}: {error}")
            }
            Self::ReadSourceBindings { path, error } => {
                write!(f, "read source-bindings registry {path}: {error}")
            }
            Self::CreateDir { path, error } => write!(
                f,
                "create backfill source-proof scope artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing backfill source-proof scope artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(
                    f,
                    "write backfill source-proof scope artifact {path}: {error}"
                )
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty backfill source-proof scope artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize backfill source-proof scope artifact: {error}")
            }
        }
    }
}

impl Error for BackfillSourceProofScopeError {}

pub fn evaluate_backfill_source_proof_scope(
    report_id: impl Into<String>,
    source_proof_json: &str,
    manifest_json: &str,
) -> Result<BackfillSourceProofScopeReport, BackfillSourceProofScopeError> {
    let proof: SourceProofReport = serde_json::from_str(source_proof_json).map_err(|error| {
        BackfillSourceProofScopeError::ParseSourceProofJson {
            path: "inline".to_string(),
            error: error.to_string(),
        }
    })?;
    let manifest: Value = serde_json::from_str(manifest_json).map_err(|error| {
        BackfillSourceProofScopeError::ParseManifestJson {
            path: "inline".to_string(),
            error: error.to_string(),
        }
    })?;
    Ok(evaluate_backfill_source_proof_scope_from_values(
        report_id.into(),
        proof,
        manifest,
        &crate::source_proof::committed_source_binding_registry(),
        None,
    ))
}

pub fn evaluate_backfill_source_proof_scope_for_selected_object(
    report_id: impl Into<String>,
    source_proof_json: &str,
    manifest_json: &str,
    selected_object_uri: &str,
) -> Result<BackfillSourceProofScopeReport, BackfillSourceProofScopeError> {
    let proof: SourceProofReport = serde_json::from_str(source_proof_json).map_err(|error| {
        BackfillSourceProofScopeError::ParseSourceProofJson {
            path: "inline".to_string(),
            error: error.to_string(),
        }
    })?;
    let manifest: Value = serde_json::from_str(manifest_json).map_err(|error| {
        BackfillSourceProofScopeError::ParseManifestJson {
            path: "inline".to_string(),
            error: error.to_string(),
        }
    })?;
    Ok(evaluate_backfill_source_proof_scope_from_values(
        report_id.into(),
        proof,
        manifest,
        &crate::source_proof::committed_source_binding_registry(),
        selected_object_uri_from_config(Some(selected_object_uri.to_string())),
    ))
}

pub fn write_backfill_source_proof_scope_report_from_spec_file(
    spec_path: &Path,
) -> Result<BackfillSourceProofScopeArtifact, BackfillSourceProofScopeError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_bytes = read_active_backfill_runtime_input(None, spec_path).map_err(|error| {
        BackfillSourceProofScopeError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let spec_text = std::str::from_utf8(&spec_bytes).map_err(|error| {
        BackfillSourceProofScopeError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let spec: BackfillSourceProofScopeSpec = toml::from_str(spec_text).map_err(|error| {
        BackfillSourceProofScopeError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        }
    })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let source_bindings_path = spec.source_bindings_path.display().to_string();
    let source_bindings_bytes =
        read_active_backfill_runtime_input(Some(base_dir), &spec.source_bindings_path).map_err(
            |error| BackfillSourceProofScopeError::ReadSourceBindings {
                path: source_bindings_path.clone(),
                error: error.to_string(),
            },
        )?;
    let source_bindings_text = std::str::from_utf8(&source_bindings_bytes).map_err(|error| {
        BackfillSourceProofScopeError::ReadSourceBindings {
            path: source_bindings_path.clone(),
            error: error.to_string(),
        }
    })?;
    let source_bindings_registry = SourceBindingRegistry::from_toml_str(source_bindings_text)
        .map_err(|error| BackfillSourceProofScopeError::ReadSourceBindings {
            path: source_bindings_path,
            error: error.to_string(),
        })?;
    let source_proof_path = spec.source_proof_path.display().to_string();
    let source_proof_bytes =
        read_active_backfill_runtime_input(Some(base_dir), &spec.source_proof_path).map_err(
            |error| BackfillSourceProofScopeError::ReadSourceProof {
                path: source_proof_path.clone(),
                error: error.to_string(),
            },
        )?;
    let source_proof_text = std::str::from_utf8(&source_proof_bytes).map_err(|error| {
        BackfillSourceProofScopeError::ReadSourceProof {
            path: source_proof_path.clone(),
            error: error.to_string(),
        }
    })?;
    let proof: SourceProofReport = serde_json::from_str(source_proof_text).map_err(|error| {
        BackfillSourceProofScopeError::ParseSourceProofJson {
            path: source_proof_path,
            error: error.to_string(),
        }
    })?;
    let manifest_path = spec.manifest_path.display().to_string();
    let manifest_bytes = read_active_backfill_runtime_input(Some(base_dir), &spec.manifest_path)
        .map_err(|error| BackfillSourceProofScopeError::ReadManifest {
            path: manifest_path.clone(),
            error: error.to_string(),
        })?;
    let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|error| {
        BackfillSourceProofScopeError::ReadManifest {
            path: manifest_path.clone(),
            error: error.to_string(),
        }
    })?;
    let manifest: Value = serde_json::from_str(manifest_text).map_err(|error| {
        BackfillSourceProofScopeError::ParseManifestJson {
            path: manifest_path,
            error: error.to_string(),
        }
    })?;
    let report = evaluate_backfill_source_proof_scope_from_values(
        spec.report_id,
        proof,
        manifest,
        &source_bindings_registry,
        selected_object_uri_from_config(spec.selected_object_uri),
    );
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    write_backfill_source_proof_scope_report(&output_dir, &report)
}

pub fn write_backfill_source_proof_scope_report(
    output_dir: &Path,
    report: &BackfillSourceProofScopeReport,
) -> Result<BackfillSourceProofScopeArtifact, BackfillSourceProofScopeError> {
    let path =
        active_backfill_runtime_output_path(output_dir, BACKFILL_SOURCE_PROOF_SCOPE_REPORT_FILE)
            .map_err(|error| BackfillSourceProofScopeError::CreateDir {
                path: output_dir.display().to_string(),
                error: error.to_string(),
            })?;
    fs::create_dir_all(output_dir).map_err(|error| BackfillSourceProofScopeError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_SOURCE_PROOF_SCOPE_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillSourceProofScopeError::Serialize,
            read_existing_error: |path, error| BackfillSourceProofScopeError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| BackfillSourceProofScopeError::ExistingArtifactMismatch { path },
            write_error: |path, error| BackfillSourceProofScopeError::Write { path, error },
        },
    )?;
    Ok(BackfillSourceProofScopeArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
    })
}

fn evaluate_backfill_source_proof_scope_from_values(
    report_id: String,
    proof: SourceProofReport,
    manifest: Value,
    source_bindings_registry: &SourceBindingRegistry,
    selected_object_uri: Option<String>,
) -> BackfillSourceProofScopeReport {
    let manifest_id = manifest_id(&manifest);
    let acceptance_error = if proof.status == SourceProofStatus::Accepted {
        proof
            .evaluate_acceptance_with_registry(source_bindings_registry)
            .err()
            .map(|error| error.to_string())
    } else {
        None
    };
    let accepted_scope = proof.acceptance_scope.clone().unwrap_or(AcceptanceScope {
        planned_objects: 0,
        completed_objects: 0,
        failed_objects: 0,
        skipped_objects: 0,
        accepted_bytes: 0,
        selector_scope_violations: 0,
    });
    let payload_objects = manifest_payload_objects(&manifest);
    let manifest_payload_object_count = payload_objects.len() as u64;
    let manifest_payload_bytes = payload_objects
        .iter()
        .map(|object| object.bytes)
        .sum::<u64>();
    let raw_sample_present = payload_objects.iter().any(|object| {
        object.s3_uri.trim() == proof.raw_sample_uri.trim()
            && object.sha256 == proof.raw_sample_hash
    });
    let acceptance_scope_covers_manifest = accepted_scope.completed_objects
        == manifest_payload_object_count
        && accepted_scope.accepted_bytes == manifest_payload_bytes;
    let matching_objects = payload_objects
        .iter()
        .filter(|object| {
            if let Some(selected_uri) = selected_object_uri.as_deref() {
                object.s3_uri.trim() == selected_uri
                    && object.bytes <= accepted_scope.accepted_bytes
            } else {
                object.s3_uri.trim() == proof.raw_sample_uri.trim()
                    && object.sha256 == proof.raw_sample_hash
                    && object.bytes <= accepted_scope.accepted_bytes
            }
        })
        .cloned()
        .collect::<Vec<_>>();
    let selected_object = if matching_objects.len() == 1 {
        matching_objects.first().cloned()
    } else {
        None
    };

    let mut blocking_issues = Vec::new();
    if report_id.trim().is_empty() {
        blocking_issues.push(BackfillSourceProofScopeIssue::EmptyReportId);
    }
    if proof.status != SourceProofStatus::Accepted {
        blocking_issues.push(BackfillSourceProofScopeIssue::SourceProofNotAccepted);
    }
    if acceptance_error.is_some() {
        blocking_issues.push(BackfillSourceProofScopeIssue::SourceProofAcceptanceFailed);
    }
    if proof.acceptance_scope.is_none() {
        blocking_issues.push(BackfillSourceProofScopeIssue::MissingAcceptanceScope);
    }
    if payload_objects.is_empty() {
        blocking_issues.push(BackfillSourceProofScopeIssue::NoManifestPayloadObjects);
    }
    if selected_object_uri.is_some() && (!acceptance_scope_covers_manifest || !raw_sample_present) {
        blocking_issues.push(BackfillSourceProofScopeIssue::AcceptanceScopeDoesNotCoverManifest);
    }
    if matching_objects.is_empty() {
        blocking_issues.push(BackfillSourceProofScopeIssue::NoMatchingManifestObject);
    } else if matching_objects.len() > 1 {
        blocking_issues.push(BackfillSourceProofScopeIssue::MultipleMatchingManifestObjects);
    }

    let status = if blocking_issues.is_empty() {
        BackfillSourceProofScopeStatus::CandidateFound
    } else {
        BackfillSourceProofScopeStatus::Blocked
    };
    let object_level_tranche_required = manifest_payload_object_count > 1
        || manifest_payload_object_count > accepted_scope.completed_objects
        || manifest_payload_bytes > accepted_scope.accepted_bytes;

    BackfillSourceProofScopeReport {
        schema_version: BACKFILL_SOURCE_PROOF_SCOPE_SCHEMA_VERSION.to_string(),
        report_id,
        status,
        source_proof_id: proof.source_proof_id,
        source_proof_version: proof.source_proof_version,
        source_binding: proof.source_binding,
        table_family: proof.table_family,
        source_usage_scope: proof.usage_scope,
        manifest_id,
        accepted_scope_completed_objects: accepted_scope.completed_objects,
        accepted_scope_accepted_bytes: accepted_scope.accepted_bytes,
        manifest_payload_object_count,
        matching_object_count: matching_objects.len() as u64,
        object_level_tranche_required,
        selected_object,
        source_proof_acceptance_error: acceptance_error,
        blocking_issues,
    }
}

fn selected_object_uri_from_config(selected_object_uri: Option<String>) -> Option<String> {
    selected_object_uri
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn manifest_id(manifest: &Value) -> String {
    string_at(manifest, &["manifest_id"])
        .or_else(|| string_at(manifest, &["run_id"]))
        .unwrap_or_default()
}

fn manifest_payload_objects(manifest: &Value) -> Vec<BackfillSourceProofScopeObject> {
    manifest
        .get("payload_records")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(manifest_payload_object)
        .collect()
}

fn manifest_payload_object(value: &Value) -> Option<BackfillSourceProofScopeObject> {
    let s3_uri = string_at(value, &["s3_uri"])?;
    let source_url = string_at(value, &["source_url"]).unwrap_or_default();
    let sha256 = string_at(value, &["sha256"])?;
    let bytes = value.get("bytes")?.as_u64()?;
    let archive_date = string_at(value, &["archive_date"])
        .or_else(|| string_at(value, &["attrs", "dt"]))
        .unwrap_or_default();
    Some(BackfillSourceProofScopeObject {
        s3_uri,
        source_url,
        sha256,
        bytes,
        archive_date,
        source_row_groups: u64_array_at(value, &["source_row_groups"])
            .or_else(|| u64_array_at(value, &["row_groups"]))
            .unwrap_or_default(),
        predicate_ref: string_at(value, &["predicate_ref"]),
    })
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn u64_array_at(value: &Value, path: &[&str]) -> Option<Vec<u64>> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    let values = current.as_array()?;
    values.iter().map(Value::as_u64).collect()
}

fn default_source_usage_scope() -> SourceProofUsageScope {
    SourceProofUsageScope::CanonicalBackfillInput
}

fn is_canonical_backfill_input(value: &SourceProofUsageScope) -> bool {
    *value == SourceProofUsageScope::CanonicalBackfillInput
}
