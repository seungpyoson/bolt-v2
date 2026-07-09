//! Single-object raw backfill staging gate.
//!
//! This is the bounded ingress step before source-proof scope selection. It
//! verifies a locally supplied payload against config-pinned bytes/hash, writes
//! exactly one object with create-only semantics, and emits the existing
//! `payload_records` manifest shape consumed by the source-proof scope gate.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use bytes::Bytes;
use nautilus_persistence::parquet::create_object_store_from_path;
use object_store::{
    Error as ObjectStoreError, ObjectStore, ObjectStoreExt, PutMode, path::Path as ObjectPath,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    path_resolution::{resolve_existing_path, resolve_output_dir},
    run_manifest::{ManifestArtifactStore, ManifestError, artifact_store_storage_options_for_uri},
    source_proof::IngestManifestObjectRecord,
};

pub const BACKFILL_OBJECT_STAGING_SCHEMA_VERSION: &str = "backfill-object-staging-manifest.v1";
pub const BACKFILL_OBJECT_STAGING_MANIFEST_FILE: &str = "backfill-object-staging-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillObjectStagingSpec {
    pub staging_id: String,
    pub artifact_root: String,
    pub artifact_store: ManifestArtifactStore,
    pub local_object_path: PathBuf,
    pub output_object_uri: String,
    pub source_url: String,
    pub expected_sha256: String,
    pub expected_bytes: u64,
    pub archive_date: String,
    pub schema_columns: Vec<String>,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillObjectStagingStatus {
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillObjectStagingManifest {
    pub schema_version: String,
    pub manifest_id: String,
    pub status: BackfillObjectStagingStatus,
    pub artifact_root: String,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub payload_records: Vec<IngestManifestObjectRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillObjectStagingArtifact {
    pub manifest_path: PathBuf,
    pub manifest_hash: String,
    pub manifest_bytes: u64,
    pub object_uri: String,
    pub object_sha256: String,
    pub object_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackfillObjectStagingError {
    ReadSpec {
        path: String,
        error: String,
    },
    ParseSpecToml {
        path: String,
        error: String,
    },
    EmptyField(&'static str),
    EmptySchemaColumns,
    InvalidExpectedSha256(String),
    ArtifactRootMismatch {
        artifact_root: String,
        output_uri: String,
    },
    ReadLocalObject {
        path: String,
        error: String,
    },
    BytesMismatch {
        expected: u64,
        actual: u64,
    },
    Sha256Mismatch {
        expected: String,
        actual: String,
    },
    ArtifactStoreOptions(String),
    CreateLocalParent {
        path: String,
        error: String,
    },
    OpenObjectStore {
        uri: String,
        error: String,
    },
    OutputObjectAlreadyExists {
        uri: String,
    },
    CheckOutputObject {
        uri: String,
        error: String,
    },
    WriteOutputObject {
        uri: String,
        error: String,
    },
    CreateOutputDir {
        path: String,
        error: String,
    },
    ReadExistingManifest {
        path: String,
        error: String,
    },
    WriteManifest {
        path: String,
        error: String,
    },
    ExistingManifestMismatch {
        path: String,
    },
    Serialize(String),
}

impl fmt::Display for BackfillObjectStagingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read backfill object staging spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(f, "parse backfill object staging spec TOML {path}: {error}")
            }
            Self::EmptyField(field) => write!(f, "{field} must not be empty"),
            Self::EmptySchemaColumns => write!(f, "schema_columns must not be empty"),
            Self::InvalidExpectedSha256(value) => {
                write!(
                    f,
                    "expected_sha256 must be lowercase SHA-256 hex, got {value:?}"
                )
            }
            Self::ArtifactRootMismatch {
                artifact_root,
                output_uri,
            } => write!(
                f,
                "output_object_uri {output_uri:?} must be under artifact_root raw prefix {artifact_root:?}/raw/"
            ),
            Self::ReadLocalObject { path, error } => {
                write!(f, "read local object {path}: {error}")
            }
            Self::BytesMismatch { expected, actual } => {
                write!(
                    f,
                    "object bytes mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Sha256Mismatch { expected, actual } => {
                write!(
                    f,
                    "object SHA-256 mismatch: expected {expected}, got {actual}"
                )
            }
            Self::ArtifactStoreOptions(error) => {
                write!(f, "artifact-store options rejected: {error}")
            }
            Self::CreateLocalParent { path, error } => {
                write!(f, "create local output object parent {path}: {error}")
            }
            Self::OpenObjectStore { uri, error } => {
                write!(f, "open output object store for {uri:?}: {error}")
            }
            Self::OutputObjectAlreadyExists { uri } => {
                write!(f, "output object already exists: {uri}")
            }
            Self::CheckOutputObject { uri, error } => {
                write!(f, "check output object {uri}: {error}")
            }
            Self::WriteOutputObject { uri, error } => {
                write!(f, "write output object {uri}: {error}")
            }
            Self::CreateOutputDir { path, error } => {
                write!(f, "create staging manifest directory {path}: {error}")
            }
            Self::ReadExistingManifest { path, error } => {
                write!(f, "read existing staging manifest {path}: {error}")
            }
            Self::WriteManifest { path, error } => {
                write!(f, "write staging manifest {path}: {error}")
            }
            Self::ExistingManifestMismatch { path } => write!(
                f,
                "dirty backfill object staging manifest {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize backfill object staging manifest: {error}")
            }
        }
    }
}

impl Error for BackfillObjectStagingError {}

pub fn stage_backfill_object_from_spec_file_with_resolver<F>(
    spec_path: &Path,
    resolver: &mut F,
) -> Result<BackfillObjectStagingArtifact, BackfillObjectStagingError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let spec_path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| BackfillObjectStagingError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: BackfillObjectStagingSpec =
        toml::from_str(&spec_text).map_err(|error| BackfillObjectStagingError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    stage_backfill_object_with_resolver_and_base(&spec, resolver, base_dir)
}

pub fn stage_backfill_object_with_resolver<F>(
    spec: &BackfillObjectStagingSpec,
    resolver: &mut F,
) -> Result<BackfillObjectStagingArtifact, BackfillObjectStagingError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    stage_backfill_object_with_resolver_and_base(spec, resolver, Path::new("."))
}

fn stage_backfill_object_with_resolver_and_base<F>(
    spec: &BackfillObjectStagingSpec,
    resolver: &mut F,
    base_dir: &Path,
) -> Result<BackfillObjectStagingArtifact, BackfillObjectStagingError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    validate_spec(spec)?;
    let local_object_path = resolve_existing_path(base_dir, &spec.local_object_path);
    let actual_bytes = fs::metadata(&local_object_path)
        .map_err(|error| BackfillObjectStagingError::ReadLocalObject {
            path: spec.local_object_path.display().to_string(),
            error: error.to_string(),
        })?
        .len();
    if actual_bytes != spec.expected_bytes {
        return Err(BackfillObjectStagingError::BytesMismatch {
            expected: spec.expected_bytes,
            actual: actual_bytes,
        });
    }
    let object_bytes = fs::read(&local_object_path).map_err(|error| {
        BackfillObjectStagingError::ReadLocalObject {
            path: spec.local_object_path.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let read_bytes = object_bytes.len() as u64;
    if read_bytes != spec.expected_bytes {
        return Err(BackfillObjectStagingError::BytesMismatch {
            expected: spec.expected_bytes,
            actual: read_bytes,
        });
    }
    let actual_sha256 = hex::encode(Sha256::digest(&object_bytes));
    if actual_sha256 != spec.expected_sha256 {
        return Err(BackfillObjectStagingError::Sha256Mismatch {
            expected: spec.expected_sha256.clone(),
            actual: actual_sha256,
        });
    }

    let storage_options = artifact_store_storage_options_for_uri(
        &spec.output_object_uri,
        &spec.artifact_store,
        resolver,
    )
    .map_err(|error| BackfillObjectStagingError::ArtifactStoreOptions(error.to_string()))?;
    write_output_object(
        &spec.output_object_uri,
        storage_options.as_ref(),
        object_bytes,
    )?;
    let manifest = BackfillObjectStagingManifest {
        schema_version: BACKFILL_OBJECT_STAGING_SCHEMA_VERSION.to_string(),
        manifest_id: spec.staging_id.clone(),
        status: BackfillObjectStagingStatus::Completed,
        artifact_root: spec.artifact_root.clone(),
        object_count: 1,
        accepted_bytes: actual_bytes,
        payload_records: vec![IngestManifestObjectRecord {
            s3_uri: spec.output_object_uri.clone(),
            source_url: spec.source_url.clone(),
            sha256: spec.expected_sha256.clone(),
            bytes: actual_bytes,
            archive_date: spec.archive_date.clone(),
            schema_columns: spec.schema_columns.clone(),
        }],
    };
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    write_manifest(&output_dir, &manifest).map(|(manifest_path, manifest_hash, manifest_bytes)| {
        BackfillObjectStagingArtifact {
            manifest_path,
            manifest_hash,
            manifest_bytes,
            object_uri: spec.output_object_uri.clone(),
            object_sha256: spec.expected_sha256.clone(),
            object_bytes: actual_bytes,
        }
    })
}

fn validate_spec(spec: &BackfillObjectStagingSpec) -> Result<(), BackfillObjectStagingError> {
    for (field, value) in [
        ("staging_id", spec.staging_id.as_str()),
        ("artifact_root", spec.artifact_root.as_str()),
        ("output_object_uri", spec.output_object_uri.as_str()),
        ("source_url", spec.source_url.as_str()),
        ("archive_date", spec.archive_date.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(BackfillObjectStagingError::EmptyField(field));
        }
    }
    if spec
        .schema_columns
        .iter()
        .all(|column| column.trim().is_empty())
    {
        return Err(BackfillObjectStagingError::EmptySchemaColumns);
    }
    if !is_lower_sha256(&spec.expected_sha256) {
        return Err(BackfillObjectStagingError::InvalidExpectedSha256(
            spec.expected_sha256.clone(),
        ));
    }
    let raw_prefix = format!("{}/raw/", spec.artifact_root.trim_end_matches('/'));
    if !spec.output_object_uri.starts_with(&raw_prefix) {
        return Err(BackfillObjectStagingError::ArtifactRootMismatch {
            artifact_root: spec.artifact_root.clone(),
            output_uri: spec.output_object_uri.clone(),
        });
    }
    Ok(())
}

fn is_lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn write_output_object(
    output_object_uri: &str,
    storage_options: Option<&BTreeMap<String, String>>,
    object_bytes: Vec<u8>,
) -> Result<(), BackfillObjectStagingError> {
    ensure_local_parent_exists(output_object_uri)?;
    let (object_store, object_path) = object_store_target(output_object_uri, storage_options)
        .map_err(|error| BackfillObjectStagingError::OpenObjectStore {
            uri: output_object_uri.to_string(),
            error,
        })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| BackfillObjectStagingError::WriteOutputObject {
            uri: output_object_uri.to_string(),
            error: format!("build object-store runtime: {error}"),
        })?;
    match runtime.block_on(object_store.head(&object_path)) {
        Ok(_) => {
            return Err(BackfillObjectStagingError::OutputObjectAlreadyExists {
                uri: output_object_uri.to_string(),
            });
        }
        Err(ObjectStoreError::NotFound { .. }) => {}
        Err(error) => {
            return Err(BackfillObjectStagingError::CheckOutputObject {
                uri: output_object_uri.to_string(),
                error: error.to_string(),
            });
        }
    }
    runtime
        .block_on(object_store.put_opts(
            &object_path,
            Bytes::from(object_bytes).into(),
            PutMode::Create.into(),
        ))
        .map_err(|error| BackfillObjectStagingError::WriteOutputObject {
            uri: output_object_uri.to_string(),
            error: error.to_string(),
        })?;
    Ok(())
}

fn object_store_target(
    output_object_uri: &str,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<(Arc<dyn ObjectStore>, ObjectPath), String> {
    let object_store_options = storage_options
        .cloned()
        .map(|options| options.into_iter().collect());
    if let Some(path) = output_object_uri.strip_prefix("file://") {
        let path = Path::new(path);
        let parent = path
            .parent()
            .ok_or_else(|| "local output object URI has no parent directory".to_string())?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "local output object URI has no file name".to_string())?;
        let parent_uri = format!("file://{}", parent.display());
        let (object_store, _, _) = create_object_store_from_path(&parent_uri, object_store_options)
            .map_err(|error| error.to_string())?;
        return Ok((object_store, ObjectPath::from(file_name.to_string())));
    }
    let (object_store, base_path, _) =
        create_object_store_from_path(output_object_uri, object_store_options)
            .map_err(|error| error.to_string())?;
    Ok((object_store, ObjectPath::from(base_path)))
}

fn ensure_local_parent_exists(output_object_uri: &str) -> Result<(), BackfillObjectStagingError> {
    let Some(path) = output_object_uri.strip_prefix("file://") else {
        return Ok(());
    };
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).map_err(|error| {
            BackfillObjectStagingError::CreateLocalParent {
                path: parent.display().to_string(),
                error: error.to_string(),
            }
        })?;
    }
    Ok(())
}

fn write_manifest(
    output_dir: &Path,
    manifest: &BackfillObjectStagingManifest,
) -> Result<(PathBuf, String, u64), BackfillObjectStagingError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        BackfillObjectStagingError::CreateOutputDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(BACKFILL_OBJECT_STAGING_MANIFEST_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        BACKFILL_OBJECT_STAGING_MANIFEST_FILE,
        manifest,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: BackfillObjectStagingError::Serialize,
            read_existing_error: |path, error| BackfillObjectStagingError::ReadExistingManifest {
                path,
                error,
            },
            mismatch_error: |path| BackfillObjectStagingError::ExistingManifestMismatch { path },
            write_error: |path, error| BackfillObjectStagingError::WriteManifest { path, error },
        },
    )?;
    Ok((path, written.pin.sha256, written.bytes))
}

impl From<ManifestError> for BackfillObjectStagingError {
    fn from(error: ManifestError) -> Self {
        Self::ArtifactStoreOptions(error.to_string())
    }
}
