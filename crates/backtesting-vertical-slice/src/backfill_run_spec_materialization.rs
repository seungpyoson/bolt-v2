//! Backfill run-spec materialization.
//!
//! This module copies an already accepted object-level tranche into a
//! configured operator run-spec template before any payload bytes are fetched.
//! The template owns converter, instrument, venue, and strategy details; the
//! accepted tranche owns only the source-proof/object facts that must not drift.

use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use toml::Value;

use crate::atomic_artifact_write::atomic_write;
use crate::hashing::sha256_hex;
use crate::{
    backfill_accepted_tranche::{
        BackfillAcceptedTrancheManifest, BackfillAcceptedTrancheObject,
        BackfillAcceptedTrancheStatus,
    },
    operator::RunSpec,
    retired_backfill_evidence::{
        active_backfill_runtime_output_path, read_active_backfill_runtime_input,
    },
};

pub const BACKFILL_RUN_SPEC_MATERIALIZED_FILE: &str = "backfill-run-spec.toml";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillRunSpecMaterializationSpec {
    pub materialization_id: String,
    pub accepted_tranche_manifest_path: PathBuf,
    pub run_spec_template_path: PathBuf,
    pub output_dir: PathBuf,
    pub run_id: String,
    pub output_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillRunSpecMaterializationArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillRunSpecMaterializationError {
    EmptyMaterializationId,
    EmptyRunId,
    EmptyOutputPrefix,
    AcceptedTrancheNotAccepted,
    AcceptedTrancheHasBlockingIssues,
    AcceptedTrancheObjectCountNotOne,
    AcceptedTrancheBytesMismatch,
    ObjectBytesExceedTomlInteger { bytes: u64 },
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadAcceptedTrancheManifest { path: String, error: String },
    ParseAcceptedTrancheManifestJson { path: String, error: String },
    ReadRunSpecTemplate { path: String, error: String },
    ParseRunSpecTemplateToml { path: String, error: String },
    MissingTemplateTable { path: &'static str },
    InvalidTemplateTable { path: &'static str },
    SerializeToml(String),
    ParseMaterializedRunSpecToml { error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
}

impl fmt::Display for BackfillRunSpecMaterializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMaterializationId => {
                write!(f, "backfill run-spec materialization id must not be empty")
            }
            Self::EmptyRunId => write!(f, "materialized run_id must not be empty"),
            Self::EmptyOutputPrefix => write!(f, "materialized output_prefix must not be empty"),
            Self::AcceptedTrancheNotAccepted => {
                write!(f, "accepted-tranche manifest is not accepted")
            }
            Self::AcceptedTrancheHasBlockingIssues => {
                write!(f, "accepted-tranche manifest has blocking issues")
            }
            Self::AcceptedTrancheObjectCountNotOne => {
                write!(
                    f,
                    "accepted-tranche manifest must contain exactly one object"
                )
            }
            Self::AcceptedTrancheBytesMismatch => {
                write!(
                    f,
                    "accepted-tranche accepted_bytes does not match its object"
                )
            }
            Self::ObjectBytesExceedTomlInteger { bytes } => {
                write!(f, "accepted object bytes {bytes} exceed TOML integer range")
            }
            Self::ReadSpec { path, error } => {
                write!(
                    f,
                    "read backfill run-spec materialization spec {path}: {error}"
                )
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse backfill run-spec materialization spec TOML {path}: {error}"
            ),
            Self::ReadAcceptedTrancheManifest { path, error } => {
                write!(f, "read accepted-tranche manifest {path}: {error}")
            }
            Self::ParseAcceptedTrancheManifestJson { path, error } => {
                write!(f, "parse accepted-tranche manifest JSON {path}: {error}")
            }
            Self::ReadRunSpecTemplate { path, error } => {
                write!(f, "read run-spec template {path}: {error}")
            }
            Self::ParseRunSpecTemplateToml { path, error } => {
                write!(f, "parse run-spec template TOML {path}: {error}")
            }
            Self::MissingTemplateTable { path } => {
                write!(f, "run-spec template is missing table {path}")
            }
            Self::InvalidTemplateTable { path } => {
                write!(f, "run-spec template field {path} is not a table")
            }
            Self::SerializeToml(error) => write!(f, "serialize materialized run-spec: {error}"),
            Self::ParseMaterializedRunSpecToml { error } => {
                write!(f, "parse materialized run-spec TOML: {error}")
            }
            Self::CreateDir { path, error } => {
                write!(f, "create materialized run-spec directory {path}: {error}")
            }
            Self::ReadExisting { path, error } => {
                write!(f, "read existing materialized run-spec {path}: {error}")
            }
            Self::Write { path, error } => {
                write!(f, "write materialized run-spec {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty materialized run-spec {path}: existing file content differs"
            ),
        }
    }
}

impl Error for BackfillRunSpecMaterializationError {}

pub fn write_backfill_run_spec_from_materialization_spec_file(
    spec_path: &Path,
) -> Result<BackfillRunSpecMaterializationArtifact, BackfillRunSpecMaterializationError> {
    let spec_bytes = read_active_backfill_runtime_input(None, spec_path).map_err(|error| {
        BackfillRunSpecMaterializationError::ReadSpec {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let spec_text = std::str::from_utf8(&spec_bytes).map_err(|error| {
        BackfillRunSpecMaterializationError::ReadSpec {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let spec: BackfillRunSpecMaterializationSpec = toml::from_str(spec_text).map_err(|error| {
        BackfillRunSpecMaterializationError::ParseSpecToml {
            path: spec_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    write_backfill_run_spec_from_materialization_spec(&spec)
}

pub fn write_backfill_run_spec_from_materialization_spec(
    spec: &BackfillRunSpecMaterializationSpec,
) -> Result<BackfillRunSpecMaterializationArtifact, BackfillRunSpecMaterializationError> {
    validate_materialization_spec(spec)?;
    let tranche = read_accepted_tranche_manifest(&spec.accepted_tranche_manifest_path)?;
    let template = read_run_spec_template(&spec.run_spec_template_path)?;
    let materialized = materialize_run_spec_template(template, &tranche, spec)?;
    let bytes = materialized.into_bytes();
    write_materialized_run_spec(&spec.output_dir, &bytes)
}

fn validate_materialization_spec(
    spec: &BackfillRunSpecMaterializationSpec,
) -> Result<(), BackfillRunSpecMaterializationError> {
    if spec.materialization_id.trim().is_empty() {
        return Err(BackfillRunSpecMaterializationError::EmptyMaterializationId);
    }
    if spec.run_id.trim().is_empty() {
        return Err(BackfillRunSpecMaterializationError::EmptyRunId);
    }
    if spec.output_prefix.trim().is_empty() {
        return Err(BackfillRunSpecMaterializationError::EmptyOutputPrefix);
    }
    Ok(())
}

fn read_accepted_tranche_manifest(
    path: &Path,
) -> Result<BackfillAcceptedTrancheManifest, BackfillRunSpecMaterializationError> {
    let bytes = read_active_backfill_runtime_input(None, path).map_err(|error| {
        BackfillRunSpecMaterializationError::ReadAcceptedTrancheManifest {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        BackfillRunSpecMaterializationError::ParseAcceptedTrancheManifestJson {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })
}

fn read_run_spec_template(path: &Path) -> Result<Value, BackfillRunSpecMaterializationError> {
    let bytes = read_active_backfill_runtime_input(None, path).map_err(|error| {
        BackfillRunSpecMaterializationError::ReadRunSpecTemplate {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        BackfillRunSpecMaterializationError::ReadRunSpecTemplate {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    toml::from_str(text).map_err(|error| {
        BackfillRunSpecMaterializationError::ParseRunSpecTemplateToml {
            path: path.display().to_string(),
            error: error.to_string(),
        }
    })
}

fn materialize_run_spec_template(
    mut template: Value,
    tranche: &BackfillAcceptedTrancheManifest,
    spec: &BackfillRunSpecMaterializationSpec,
) -> Result<String, BackfillRunSpecMaterializationError> {
    let object = accepted_tranche_object(tranche)?;
    let object_bytes = toml_integer(object.bytes)?;

    let accepted_object = required_table_mut(&mut template, &["accepted_object"])?;
    accepted_object.insert("s3_uri".to_string(), Value::String(object.s3_uri.clone()));
    accepted_object.insert(
        "source_url".to_string(),
        Value::String(object.source_url.clone()),
    );
    accepted_object.insert("sha256".to_string(), Value::String(object.sha256.clone()));
    accepted_object.insert("bytes".to_string(), Value::Integer(object_bytes));
    accepted_object.insert(
        "archive_date".to_string(),
        Value::String(object.archive_date.clone()),
    );

    let source_proof = required_table_mut(&mut template, &["source_proof"])?;
    source_proof.insert(
        "source_proof_id".to_string(),
        Value::String(tranche.source_proof_id.clone()),
    );
    source_proof.insert(
        "source_proof_version".to_string(),
        Value::Integer(i64::from(tranche.source_proof_version)),
    );
    source_proof.insert(
        "source_binding".to_string(),
        Value::String(tranche.source_binding.clone()),
    );
    source_proof.insert(
        "table_family".to_string(),
        Value::String(tranche.table_family.clone()),
    );
    source_proof.insert(
        "usage_scope".to_string(),
        Value::try_from(tranche.source_usage_scope).map_err(|error| {
            BackfillRunSpecMaterializationError::SerializeToml(error.to_string())
        })?,
    );
    source_proof.insert(
        "raw_sample_uri".to_string(),
        Value::String(object.s3_uri.clone()),
    );
    source_proof.insert(
        "raw_sample_hash".to_string(),
        Value::String(object.sha256.clone()),
    );

    let acceptance_scope =
        required_table_mut(&mut template, &["source_proof", "acceptance_scope"])?;
    acceptance_scope.insert(
        "planned_objects".to_string(),
        Value::Integer(toml_integer(tranche.object_count)?),
    );
    acceptance_scope.insert(
        "completed_objects".to_string(),
        Value::Integer(toml_integer(tranche.object_count)?),
    );
    acceptance_scope.insert("failed_objects".to_string(), Value::Integer(0));
    acceptance_scope.insert("skipped_objects".to_string(), Value::Integer(0));
    acceptance_scope.insert("accepted_bytes".to_string(), Value::Integer(object_bytes));

    let raw_payload = required_table_mut(&mut template, &["converter", "raw_payload"])?;
    raw_payload.insert("max_object_bytes".to_string(), Value::Integer(object_bytes));

    let manifest = required_table_mut(&mut template, &["manifest"])?;
    manifest.insert("run_id".to_string(), Value::String(spec.run_id.clone()));
    manifest.insert(
        "venue_binding_key".to_string(),
        Value::String(tranche.source_binding.clone()),
    );
    manifest.insert(
        "source_proof_id".to_string(),
        Value::String(tranche.source_proof_id.clone()),
    );
    manifest.insert(
        "source_proof_version".to_string(),
        Value::Integer(i64::from(tranche.source_proof_version)),
    );
    manifest.insert(
        "output_prefix".to_string(),
        Value::String(spec.output_prefix.clone()),
    );

    let materialized = toml::to_string_pretty(&template)
        .map_err(|error| BackfillRunSpecMaterializationError::SerializeToml(error.to_string()))?;
    let _: RunSpec = toml::from_str(&materialized).map_err(|error| {
        BackfillRunSpecMaterializationError::ParseMaterializedRunSpecToml {
            error: error.to_string(),
        }
    })?;
    Ok(materialized)
}

fn accepted_tranche_object(
    tranche: &BackfillAcceptedTrancheManifest,
) -> Result<&BackfillAcceptedTrancheObject, BackfillRunSpecMaterializationError> {
    if tranche.status != BackfillAcceptedTrancheStatus::Accepted {
        return Err(BackfillRunSpecMaterializationError::AcceptedTrancheNotAccepted);
    }
    if !tranche.blocking_issues.is_empty() {
        return Err(BackfillRunSpecMaterializationError::AcceptedTrancheHasBlockingIssues);
    }
    if tranche.object_count != 1 || tranche.objects.len() != 1 {
        return Err(BackfillRunSpecMaterializationError::AcceptedTrancheObjectCountNotOne);
    }
    let object = tranche
        .objects
        .first()
        .ok_or(BackfillRunSpecMaterializationError::AcceptedTrancheObjectCountNotOne)?;
    if object.bytes != tranche.accepted_bytes {
        return Err(BackfillRunSpecMaterializationError::AcceptedTrancheBytesMismatch);
    }
    Ok(object)
}

fn required_table_mut<'a>(
    value: &'a mut Value,
    path: &[&'static str],
) -> Result<&'a mut toml::Table, BackfillRunSpecMaterializationError> {
    let mut current = value;
    for key in path {
        current = current.get_mut(*key).ok_or(
            BackfillRunSpecMaterializationError::MissingTemplateTable {
                path: dotted_path(path),
            },
        )?;
    }
    current
        .as_table_mut()
        .ok_or(BackfillRunSpecMaterializationError::InvalidTemplateTable {
            path: dotted_path(path),
        })
}

fn dotted_path(path: &[&'static str]) -> &'static str {
    match path {
        ["accepted_object"] => "accepted_object",
        ["source_proof"] => "source_proof",
        ["source_proof", "acceptance_scope"] => "source_proof.acceptance_scope",
        ["converter", "raw_payload"] => "converter.raw_payload",
        ["manifest"] => "manifest",
        _ => "unknown",
    }
}

fn toml_integer(value: u64) -> Result<i64, BackfillRunSpecMaterializationError> {
    i64::try_from(value).map_err(|_| {
        BackfillRunSpecMaterializationError::ObjectBytesExceedTomlInteger { bytes: value }
    })
}

fn write_materialized_run_spec(
    output_dir: &Path,
    bytes: &[u8],
) -> Result<BackfillRunSpecMaterializationArtifact, BackfillRunSpecMaterializationError> {
    let path = active_backfill_runtime_output_path(
        output_dir,
        BACKFILL_RUN_SPEC_MATERIALIZED_FILE,
    )
    .map_err(|error| BackfillRunSpecMaterializationError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    fs::create_dir_all(output_dir).map_err(|error| {
        BackfillRunSpecMaterializationError::CreateDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    if path.exists() {
        let existing =
            fs::read(&path).map_err(|error| BackfillRunSpecMaterializationError::ReadExisting {
                path: path.display().to_string(),
                error: error.to_string(),
            })?;
        if existing != bytes {
            return Err(
                BackfillRunSpecMaterializationError::ExistingArtifactMismatch {
                    path: path.display().to_string(),
                },
            );
        }
    } else {
        atomic_write(&path, bytes).map_err(|error| BackfillRunSpecMaterializationError::Write {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
    }
    Ok(BackfillRunSpecMaterializationArtifact {
        path,
        content_hash: sha256_hex(bytes),
        bytes: bytes.len() as u64,
    })
}
