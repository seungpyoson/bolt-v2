//! Thin Artifact Index contract helpers for BTE-produced artifacts.
//!
//! This module intentionally does not commit index events or update latest
//! pointers. The configured artifact store still needs create-only and
//! conditional pointer-update proof before BTE can rely on the commit path.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Raw,
    NtCatalog,
    SourceProofs,
    Backtests,
    ArtifactIndex,
    ResearchAnalytics,
}

impl ArtifactKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::NtCatalog => "nt_catalog",
            Self::SourceProofs => "source_proofs",
            Self::Backtests => "backtests",
            Self::ArtifactIndex => "artifact_index",
            Self::ResearchAnalytics => "research_analytics",
        }
    }

    const fn artifact_subpath(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::NtCatalog => "nt-catalog",
            Self::SourceProofs => "source-proofs",
            Self::Backtests => "backtests",
            Self::ArtifactIndex => "artifact-index",
            Self::ResearchAnalytics => "research-analytics",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteAuthority {
    ProducerOwned,
    ReadOnlyConsumer,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitState {
    Staged,
    Committed,
    Orphan,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Active,
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexLineageRef {
    pub artifact_id: String,
    pub artifact_version: Option<u64>,
    pub content_hash: String,
}

impl ArtifactIndexLineageRef {
    pub fn new(
        artifact_id: impl Into<String>,
        artifact_version: Option<u64>,
        content_hash: impl Into<String>,
    ) -> Self {
        Self {
            artifact_id: artifact_id.into(),
            artifact_version,
            content_hash: content_hash.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexRecord {
    pub artifact_id: String,
    pub artifact_kind: ArtifactKind,
    pub artifact_subfamily: Option<String>,
    pub producer_project: String,
    pub manifest_uri: String,
    pub event_uri: String,
    pub snapshot_id: Option<String>,
    pub snapshot_uri: Option<String>,
    pub latest_pointer_uri: String,
    pub content_hash: String,
    pub lineage_ids: Vec<ArtifactIndexLineageRef>,
    pub write_authority: WriteAuthority,
    pub commit_state: CommitState,
    pub lifecycle_state: LifecycleState,
}

impl ArtifactIndexRecord {
    pub fn new_staged(
        artifact_root: &str,
        artifact_kind: ArtifactKind,
        artifact_id: impl Into<String>,
        producer_project: impl Into<String>,
        manifest_uri: impl Into<String>,
        content_hash: &str,
        lineage_ids: Vec<ArtifactIndexLineageRef>,
    ) -> Result<Self, ArtifactIndexError> {
        validate_artifact_root(artifact_root)?;
        validate_sha256(content_hash)?;
        validate_lineage(&lineage_ids)?;

        let artifact_id = artifact_id.into();
        validate_non_empty("artifact_id", &artifact_id)?;
        let producer_project = producer_project.into();
        validate_non_empty("producer_project", &producer_project)?;
        let manifest_uri = manifest_uri.into();
        validate_manifest_uri(artifact_root, artifact_kind, &manifest_uri)?;

        let root = artifact_root.trim_end_matches('/');
        let kind = artifact_kind.as_str();
        let event_uri = format!(
            "{root}/artifact-index/v1/events/kind={kind}/artifact_id={artifact_id}/hash={content_hash}.json"
        );
        let latest_pointer_uri =
            format!("{root}/artifact-index/v1/pointers/kind={kind}/latest.json");

        Ok(Self {
            artifact_id,
            artifact_kind,
            artifact_subfamily: None,
            producer_project,
            manifest_uri,
            event_uri,
            snapshot_id: None,
            snapshot_uri: None,
            latest_pointer_uri,
            content_hash: content_hash.to_string(),
            lineage_ids,
            write_authority: WriteAuthority::ProducerOwned,
            commit_state: CommitState::Staged,
            lifecycle_state: LifecycleState::Active,
        })
    }

    pub fn validate_write_authority(
        &self,
        requester_project: &str,
    ) -> Result<(), ArtifactIndexError> {
        if self.write_authority == WriteAuthority::ProducerOwned
            && self.producer_project == requester_project
        {
            return Ok(());
        }
        Err(ArtifactIndexError::ReadOnlyConsumer {
            artifact_id: self.artifact_id.clone(),
            requester_project: requester_project.to_string(),
            producer_project: self.producer_project.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactIndexError {
    EmptyField {
        field: &'static str,
    },
    UnsupportedArtifactRoot {
        artifact_root: String,
    },
    ManifestOutsideArtifactRoot {
        manifest_uri: String,
        required_prefix: String,
    },
    InvalidSha256 {
        field: &'static str,
        value: String,
    },
    MissingLineage,
    ReadOnlyConsumer {
        artifact_id: String,
        requester_project: String,
        producer_project: String,
    },
}

impl fmt::Display for ArtifactIndexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => write!(f, "artifact index {field} must not be empty"),
            Self::UnsupportedArtifactRoot { artifact_root } => {
                write!(
                    f,
                    "artifact index artifact_root must be an s3 URI: {artifact_root:?}"
                )
            }
            Self::ManifestOutsideArtifactRoot {
                manifest_uri,
                required_prefix,
            } => write!(
                f,
                "artifact index manifest_uri {manifest_uri:?} must live under {required_prefix:?}"
            ),
            Self::InvalidSha256 { field, value } => {
                write!(
                    f,
                    "artifact index {field} must be a sha256 hex value: {value:?}"
                )
            }
            Self::MissingLineage => write!(f, "artifact index record requires parent lineage ids"),
            Self::ReadOnlyConsumer {
                artifact_id,
                requester_project,
                producer_project,
            } => write!(
                f,
                "artifact index record {artifact_id:?} is producer-owned by {producer_project:?}; {requester_project:?} is a read-only consumer"
            ),
        }
    }
}

impl Error for ArtifactIndexError {}

fn validate_artifact_root(artifact_root: &str) -> Result<(), ArtifactIndexError> {
    if artifact_root.starts_with("s3://") {
        Ok(())
    } else {
        Err(ArtifactIndexError::UnsupportedArtifactRoot {
            artifact_root: artifact_root.to_string(),
        })
    }
}

fn validate_manifest_uri(
    artifact_root: &str,
    artifact_kind: ArtifactKind,
    manifest_uri: &str,
) -> Result<(), ArtifactIndexError> {
    let required_prefix = format!(
        "{}/{}/",
        artifact_root.trim_end_matches('/'),
        artifact_kind.artifact_subpath()
    );
    if manifest_uri.starts_with(&required_prefix) {
        Ok(())
    } else {
        Err(ArtifactIndexError::ManifestOutsideArtifactRoot {
            manifest_uri: manifest_uri.to_string(),
            required_prefix,
        })
    }
}

fn validate_lineage(lineage_ids: &[ArtifactIndexLineageRef]) -> Result<(), ArtifactIndexError> {
    if lineage_ids.is_empty() {
        return Err(ArtifactIndexError::MissingLineage);
    }
    for lineage in lineage_ids {
        validate_non_empty("lineage.artifact_id", &lineage.artifact_id)?;
        validate_sha256_field("lineage.content_hash", &lineage.content_hash)?;
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), ArtifactIndexError> {
    validate_sha256_field("content_hash", value)
}

fn validate_sha256_field(field: &'static str, value: &str) -> Result<(), ArtifactIndexError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ArtifactIndexError::InvalidSha256 {
            field,
            value: value.to_string(),
        })
    }
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), ArtifactIndexError> {
    if value.trim().is_empty() {
        Err(ArtifactIndexError::EmptyField { field })
    } else {
        Ok(())
    }
}
