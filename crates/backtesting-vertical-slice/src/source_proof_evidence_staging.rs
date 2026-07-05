//! Source-proof evidence artifact staging.
//!
//! This stages small proof evidence files such as schema samples, license
//! attestations, retention notes, and instrument-universe snapshots under the
//! configured `source-proofs/` artifact family. It does not accept a source
//! proof; it only creates durable URI/hash evidence that the acceptance gate can
//! reference.

use std::{
    collections::{BTreeMap, BTreeSet},
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

use crate::run_manifest::{ManifestArtifactStore, artifact_store_storage_options_for_uri};

pub const SOURCE_PROOF_EVIDENCE_STAGING_SCHEMA_VERSION: &str =
    "source-proof-evidence-staging-manifest.v1";
pub const SOURCE_PROOF_EVIDENCE_STAGING_MANIFEST_FILE: &str =
    "source-proof-evidence-staging-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofEvidenceStagingFile {
    pub evidence_kind: String,
    pub local_path: PathBuf,
    pub output_uri: String,
    pub expected_sha256: String,
    pub expected_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofEvidenceStagingSpec {
    pub staging_id: String,
    pub artifact_root: String,
    pub artifact_store: ManifestArtifactStore,
    pub evidence_files: Vec<SourceProofEvidenceStagingFile>,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceProofEvidenceStagingStatus {
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofEvidenceRecord {
    pub evidence_kind: String,
    pub uri: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofEvidenceStagingManifest {
    pub schema_version: String,
    pub manifest_id: String,
    pub status: SourceProofEvidenceStagingStatus,
    pub artifact_root: String,
    pub record_count: u64,
    pub total_bytes: u64,
    pub evidence_records: Vec<SourceProofEvidenceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProofEvidenceStagingArtifact {
    pub manifest_path: PathBuf,
    pub manifest_hash: String,
    pub manifest_bytes: u64,
    pub record_count: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceProofEvidenceStagingError {
    ReadSpec {
        path: String,
        error: String,
    },
    ParseSpecToml {
        path: String,
        error: String,
    },
    EmptyField(&'static str),
    NoEvidenceFiles,
    EmptyEvidenceKind,
    DuplicateEvidenceKind(String),
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

impl fmt::Display for SourceProofEvidenceStagingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read source-proof evidence staging spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => write!(
                f,
                "parse source-proof evidence staging spec TOML {path}: {error}"
            ),
            Self::EmptyField(field) => write!(f, "{field} must not be empty"),
            Self::NoEvidenceFiles => write!(f, "evidence_files must not be empty"),
            Self::EmptyEvidenceKind => write!(f, "evidence_kind must not be empty"),
            Self::DuplicateEvidenceKind(kind) => {
                write!(f, "duplicate evidence_kind {kind:?}")
            }
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
                "output_uri {output_uri:?} must be under artifact_root source-proofs prefix {artifact_root:?}/source-proofs/"
            ),
            Self::ReadLocalObject { path, error } => {
                write!(f, "read local evidence object {path}: {error}")
            }
            Self::BytesMismatch { expected, actual } => {
                write!(
                    f,
                    "evidence bytes mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Sha256Mismatch { expected, actual } => {
                write!(
                    f,
                    "evidence SHA-256 mismatch: expected {expected}, got {actual}"
                )
            }
            Self::ArtifactStoreOptions(error) => {
                write!(f, "artifact-store options rejected: {error}")
            }
            Self::CreateLocalParent { path, error } => {
                write!(f, "create local evidence parent {path}: {error}")
            }
            Self::OpenObjectStore { uri, error } => {
                write!(f, "open evidence object store for {uri:?}: {error}")
            }
            Self::OutputObjectAlreadyExists { uri } => {
                write!(f, "evidence output object already exists: {uri}")
            }
            Self::CheckOutputObject { uri, error } => {
                write!(f, "check evidence output object {uri}: {error}")
            }
            Self::WriteOutputObject { uri, error } => {
                write!(f, "write evidence output object {uri}: {error}")
            }
            Self::CreateOutputDir { path, error } => {
                write!(
                    f,
                    "create source-proof evidence manifest directory {path}: {error}"
                )
            }
            Self::ReadExistingManifest { path, error } => {
                write!(
                    f,
                    "read existing source-proof evidence manifest {path}: {error}"
                )
            }
            Self::WriteManifest { path, error } => {
                write!(f, "write source-proof evidence manifest {path}: {error}")
            }
            Self::ExistingManifestMismatch { path } => write!(
                f,
                "dirty source-proof evidence staging manifest {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(
                    f,
                    "serialize source-proof evidence staging manifest: {error}"
                )
            }
        }
    }
}

impl Error for SourceProofEvidenceStagingError {}

pub fn stage_source_proof_evidence_from_spec_file_with_resolver<F>(
    spec_path: &Path,
    resolver: &mut F,
) -> Result<SourceProofEvidenceStagingArtifact, SourceProofEvidenceStagingError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let spec_path_display = spec_path.display().to_string();
    let spec_text = fs::read_to_string(spec_path).map_err(|error| {
        SourceProofEvidenceStagingError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let spec: SourceProofEvidenceStagingSpec = toml::from_str(&spec_text).map_err(|error| {
        SourceProofEvidenceStagingError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        }
    })?;
    stage_source_proof_evidence_with_resolver(&spec, resolver)
}

pub fn stage_source_proof_evidence_with_resolver<F>(
    spec: &SourceProofEvidenceStagingSpec,
    resolver: &mut F,
) -> Result<SourceProofEvidenceStagingArtifact, SourceProofEvidenceStagingError>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    validate_spec(spec)?;
    let mut pending_writes = Vec::with_capacity(spec.evidence_files.len());
    for file in &spec.evidence_files {
        let bytes = fs::read(&file.local_path).map_err(|error| {
            SourceProofEvidenceStagingError::ReadLocalObject {
                path: file.local_path.display().to_string(),
                error: error.to_string(),
            }
        })?;
        let actual_bytes = bytes.len() as u64;
        if actual_bytes != file.expected_bytes {
            return Err(SourceProofEvidenceStagingError::BytesMismatch {
                expected: file.expected_bytes,
                actual: actual_bytes,
            });
        }
        let actual_sha256 = hex::encode(Sha256::digest(&bytes));
        if actual_sha256 != file.expected_sha256 {
            return Err(SourceProofEvidenceStagingError::Sha256Mismatch {
                expected: file.expected_sha256.clone(),
                actual: actual_sha256,
            });
        }
        pending_writes.push((file, bytes));
    }

    for (file, bytes) in pending_writes {
        let storage_options = artifact_store_storage_options_for_uri(
            &file.output_uri,
            &spec.artifact_store,
            resolver,
        )
        .map_err(|error| {
            SourceProofEvidenceStagingError::ArtifactStoreOptions(error.to_string())
        })?;
        write_output_object(&file.output_uri, storage_options.as_ref(), bytes)?;
    }

    let evidence_records = spec
        .evidence_files
        .iter()
        .map(|file| SourceProofEvidenceRecord {
            evidence_kind: file.evidence_kind.clone(),
            uri: file.output_uri.clone(),
            sha256: file.expected_sha256.clone(),
            bytes: file.expected_bytes,
        })
        .collect::<Vec<_>>();
    let total_bytes = evidence_records
        .iter()
        .map(|record| record.bytes)
        .sum::<u64>();
    let manifest = SourceProofEvidenceStagingManifest {
        schema_version: SOURCE_PROOF_EVIDENCE_STAGING_SCHEMA_VERSION.to_string(),
        manifest_id: spec.staging_id.clone(),
        status: SourceProofEvidenceStagingStatus::Completed,
        artifact_root: spec.artifact_root.clone(),
        record_count: evidence_records.len() as u64,
        total_bytes,
        evidence_records,
    };
    write_manifest(&spec.output_dir, &manifest).map(
        |(manifest_path, manifest_hash, manifest_bytes)| SourceProofEvidenceStagingArtifact {
            manifest_path,
            manifest_hash,
            manifest_bytes,
            record_count: manifest.record_count,
            total_bytes: manifest.total_bytes,
        },
    )
}

fn validate_spec(
    spec: &SourceProofEvidenceStagingSpec,
) -> Result<(), SourceProofEvidenceStagingError> {
    for (field, value) in [
        ("staging_id", spec.staging_id.as_str()),
        ("artifact_root", spec.artifact_root.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(SourceProofEvidenceStagingError::EmptyField(field));
        }
    }
    if spec.evidence_files.is_empty() {
        return Err(SourceProofEvidenceStagingError::NoEvidenceFiles);
    }
    let mut evidence_kinds = BTreeSet::new();
    for file in &spec.evidence_files {
        if file.evidence_kind.trim().is_empty() {
            return Err(SourceProofEvidenceStagingError::EmptyEvidenceKind);
        }
        if !evidence_kinds.insert(file.evidence_kind.as_str()) {
            return Err(SourceProofEvidenceStagingError::DuplicateEvidenceKind(
                file.evidence_kind.clone(),
            ));
        }
        if !is_lower_sha256(&file.expected_sha256) {
            return Err(SourceProofEvidenceStagingError::InvalidExpectedSha256(
                file.expected_sha256.clone(),
            ));
        }
        let source_proofs_prefix = format!(
            "{}/source-proofs/",
            spec.artifact_root.trim_end_matches('/')
        );
        if !file.output_uri.starts_with(&source_proofs_prefix) {
            return Err(SourceProofEvidenceStagingError::ArtifactRootMismatch {
                artifact_root: spec.artifact_root.clone(),
                output_uri: file.output_uri.clone(),
            });
        }
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
    output_uri: &str,
    storage_options: Option<&BTreeMap<String, String>>,
    object_bytes: Vec<u8>,
) -> Result<(), SourceProofEvidenceStagingError> {
    ensure_local_parent_exists(output_uri)?;
    let (object_store, object_path) =
        object_store_target(output_uri, storage_options).map_err(|error| {
            SourceProofEvidenceStagingError::OpenObjectStore {
                uri: output_uri.to_string(),
                error,
            }
        })?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| SourceProofEvidenceStagingError::WriteOutputObject {
            uri: output_uri.to_string(),
            error: format!("build object-store runtime: {error}"),
        })?;
    match runtime.block_on(object_store.head(&object_path)) {
        Ok(_) => {
            return Err(SourceProofEvidenceStagingError::OutputObjectAlreadyExists {
                uri: output_uri.to_string(),
            });
        }
        Err(ObjectStoreError::NotFound { .. }) => {}
        Err(error) => {
            return Err(SourceProofEvidenceStagingError::CheckOutputObject {
                uri: output_uri.to_string(),
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
        .map_err(|error| SourceProofEvidenceStagingError::WriteOutputObject {
            uri: output_uri.to_string(),
            error: error.to_string(),
        })?;
    Ok(())
}

fn ensure_local_parent_exists(output_uri: &str) -> Result<(), SourceProofEvidenceStagingError> {
    let Some(path) = output_uri.strip_prefix("file://") else {
        return Ok(());
    };
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).map_err(|error| {
            SourceProofEvidenceStagingError::CreateLocalParent {
                path: parent.display().to_string(),
                error: error.to_string(),
            }
        })?;
    }
    Ok(())
}

fn object_store_target(
    output_uri: &str,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<(Arc<dyn ObjectStore>, ObjectPath), String> {
    let object_store_options = storage_options
        .cloned()
        .map(|options| options.into_iter().collect());
    if let Some(path) = output_uri.strip_prefix("file://") {
        let path = Path::new(path);
        let parent = path
            .parent()
            .ok_or_else(|| "local output evidence URI has no parent directory".to_string())?;
        let file_name = path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "local output evidence URI has no file name".to_string())?;
        let parent_uri = format!("file://{}", parent.display());
        let (object_store, _, _) = create_object_store_from_path(&parent_uri, object_store_options)
            .map_err(|error| error.to_string())?;
        return Ok((object_store, ObjectPath::from(file_name.to_string())));
    }
    let (object_store, base_path, _) =
        create_object_store_from_path(output_uri, object_store_options)
            .map_err(|error| error.to_string())?;
    Ok((object_store, ObjectPath::from(base_path)))
}

fn write_manifest(
    output_dir: &Path,
    manifest: &SourceProofEvidenceStagingManifest,
) -> Result<(PathBuf, String, u64), SourceProofEvidenceStagingError> {
    fs::create_dir_all(output_dir).map_err(|error| {
        SourceProofEvidenceStagingError::CreateOutputDir {
            path: output_dir.display().to_string(),
            error: error.to_string(),
        }
    })?;
    let path = output_dir.join(SOURCE_PROOF_EVIDENCE_STAGING_MANIFEST_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_PROOF_EVIDENCE_STAGING_MANIFEST_FILE,
        manifest,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: SourceProofEvidenceStagingError::Serialize,
            read_existing_error: |path, error| {
                SourceProofEvidenceStagingError::ReadExistingManifest { path, error }
            },
            mismatch_error: |path| {
                SourceProofEvidenceStagingError::ExistingManifestMismatch { path }
            },
            write_error: |path, error| SourceProofEvidenceStagingError::WriteManifest {
                path,
                error,
            },
        },
    )?;
    Ok((path, written.pin.sha256, written.bytes))
}
