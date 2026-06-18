//! Thin Artifact Index contract helpers for BTE-produced artifacts.
//!
//! This module intentionally does not commit index events or update latest
//! pointers. The configured artifact store still needs create-only and
//! conditional pointer-update proof before BTE can rely on the commit path.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
#[serde(rename_all = "kebab-case")]
pub enum ResearchAnalyticsSubfamily {
    Datasets,
    FeatureTables,
    ExperimentResults,
}

impl ResearchAnalyticsSubfamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Datasets => "datasets",
            Self::FeatureTables => "feature-tables",
            Self::ExperimentResults => "experiment-results",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageProfile {
    Active,
    Archive,
    DeepArchive,
}

impl StorageProfile {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archive => "archive",
            Self::DeepArchive => "deep_archive",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactLifecycleConfig {
    pub retain_forever: bool,
    pub default_delete_after_seconds: Option<u64>,
    pub default_expire_after_seconds: Option<u64>,
    pub quiet_window_seconds: u64,
    pub storage_profiles: Vec<StorageProfile>,
}

impl ArtifactLifecycleConfig {
    pub fn retain_forever(quiet_window_seconds: u64) -> Self {
        Self {
            retain_forever: true,
            default_delete_after_seconds: None,
            default_expire_after_seconds: None,
            quiet_window_seconds,
            storage_profiles: vec![
                StorageProfile::Active,
                StorageProfile::Archive,
                StorageProfile::DeepArchive,
            ],
        }
    }

    pub fn validate(&self) -> Result<(), ArtifactIndexError> {
        if !self.retain_forever {
            return Err(ArtifactIndexError::LifecyclePolicy(
                "canonical artifacts must retain forever by default".to_string(),
            ));
        }
        if self.default_delete_after_seconds.is_some() {
            return Err(ArtifactIndexError::LifecyclePolicy(
                "default delete lifecycle rules are disabled".to_string(),
            ));
        }
        if self.default_expire_after_seconds.is_some() {
            return Err(ArtifactIndexError::LifecyclePolicy(
                "default expiration lifecycle rules are disabled".to_string(),
            ));
        }
        if self.quiet_window_seconds == 0 {
            return Err(ArtifactIndexError::LifecyclePolicy(
                "quiet_window_seconds must be configured as a positive value".to_string(),
            ));
        }
        for required in [
            StorageProfile::Active,
            StorageProfile::Archive,
            StorageProfile::DeepArchive,
        ] {
            if !self.storage_profiles.contains(&required) {
                return Err(ArtifactIndexError::LifecyclePolicy(format!(
                    "storage profile {} is required",
                    required.as_str()
                )));
            }
        }
        Ok(())
    }

    pub fn lifecycle_state_at(
        &self,
        created_at_seconds: u64,
        observed_at_seconds: u64,
    ) -> Result<LifecycleState, ArtifactIndexError> {
        self.validate()?;
        if observed_at_seconds < created_at_seconds {
            return Err(ArtifactIndexError::LifecyclePolicy(
                "observed_at_seconds must not be before created_at_seconds".to_string(),
            ));
        }
        if observed_at_seconds - created_at_seconds >= self.quiet_window_seconds {
            Ok(LifecycleState::Inactive)
        } else {
            Ok(LifecycleState::Active)
        }
    }
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

struct ArtifactIndexRecordDraft {
    artifact_kind: ArtifactKind,
    artifact_subfamily: Option<String>,
    artifact_id: String,
    producer_project: String,
    manifest_uri: String,
    content_hash: String,
    lineage_ids: Vec<ArtifactIndexLineageRef>,
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
        if artifact_kind == ArtifactKind::ResearchAnalytics {
            return Err(ArtifactIndexError::ResearchAnalyticsSubfamilyRequired);
        }
        Self::new_staged_from_draft(
            artifact_root,
            ArtifactIndexRecordDraft {
                artifact_kind,
                artifact_subfamily: None,
                artifact_id: artifact_id.into(),
                producer_project: producer_project.into(),
                manifest_uri: manifest_uri.into(),
                content_hash: content_hash.to_string(),
                lineage_ids,
            },
        )
    }

    pub fn new_research_analytics_staged(
        artifact_root: &str,
        artifact_subfamily: ResearchAnalyticsSubfamily,
        artifact_id: impl Into<String>,
        producer_project: impl Into<String>,
        manifest_uri: impl Into<String>,
        content_hash: &str,
        lineage_ids: Vec<ArtifactIndexLineageRef>,
    ) -> Result<Self, ArtifactIndexError> {
        Self::new_staged_from_draft(
            artifact_root,
            ArtifactIndexRecordDraft {
                artifact_kind: ArtifactKind::ResearchAnalytics,
                artifact_subfamily: Some(artifact_subfamily.as_str().to_string()),
                artifact_id: artifact_id.into(),
                producer_project: producer_project.into(),
                manifest_uri: manifest_uri.into(),
                content_hash: content_hash.to_string(),
                lineage_ids,
            },
        )
    }

    fn new_staged_from_draft(
        artifact_root: &str,
        draft: ArtifactIndexRecordDraft,
    ) -> Result<Self, ArtifactIndexError> {
        let ArtifactIndexRecordDraft {
            artifact_kind,
            artifact_subfamily,
            artifact_id,
            producer_project,
            manifest_uri,
            content_hash,
            lineage_ids,
        } = draft;

        validate_artifact_root(artifact_root)?;
        validate_sha256(&content_hash)?;
        validate_lineage(&lineage_ids)?;

        validate_non_empty("artifact_id", &artifact_id)?;
        validate_non_empty("producer_project", &producer_project)?;
        validate_manifest_uri(
            artifact_root,
            artifact_kind,
            artifact_subfamily.as_deref(),
            &manifest_uri,
        )?;

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
            artifact_subfamily,
            producer_project,
            manifest_uri,
            event_uri,
            snapshot_id: None,
            snapshot_uri: None,
            latest_pointer_uri,
            content_hash,
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

    pub fn committed_for_snapshot(
        &self,
        snapshot_id: &str,
        snapshot_uri: &str,
    ) -> Result<Self, ArtifactIndexError> {
        validate_non_empty("snapshot_id", snapshot_id)?;
        validate_non_empty("snapshot_uri", snapshot_uri)?;

        let mut record = self.clone();
        record.snapshot_id = Some(snapshot_id.to_string());
        record.snapshot_uri = Some(snapshot_uri.to_string());
        record.commit_state = CommitState::Committed;
        Ok(record)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexEventObject {
    pub artifact_kind: ArtifactKind,
    pub artifact_id: String,
    pub producer_project: String,
    pub event_uri: String,
    pub payload_hash: String,
}

impl ArtifactIndexEventObject {
    pub fn from_record(record: &ArtifactIndexRecord) -> Result<Self, ArtifactIndexError> {
        record.validate_write_authority(&record.producer_project)?;
        validate_non_empty("event_uri", &record.event_uri)?;

        Ok(Self {
            artifact_kind: record.artifact_kind,
            artifact_id: record.artifact_id.clone(),
            producer_project: record.producer_project.clone(),
            event_uri: record.event_uri.clone(),
            payload_hash: event_payload_hash(record)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactIndexEventWriteDecision {
    CreateWithIfNoneMatchAny,
    AlreadyExistsSamePayload,
}

pub fn plan_index_event_create(
    requester_project: &str,
    event: &ArtifactIndexEventObject,
    existing: Option<&ArtifactIndexEventObject>,
) -> Result<ArtifactIndexEventWriteDecision, ArtifactIndexError> {
    validate_non_empty("requester_project", requester_project)?;
    validate_non_empty("event_uri", &event.event_uri)?;
    validate_sha256_field("event.payload_hash", &event.payload_hash)?;
    if event.producer_project != requester_project {
        return Err(ArtifactIndexError::ReadOnlyConsumer {
            artifact_id: event.artifact_id.clone(),
            requester_project: requester_project.to_string(),
            producer_project: event.producer_project.clone(),
        });
    }

    let Some(existing) = existing else {
        return Ok(ArtifactIndexEventWriteDecision::CreateWithIfNoneMatchAny);
    };
    if existing.event_uri != event.event_uri {
        return Err(ArtifactIndexError::EventUriMismatch {
            expected_uri: event.event_uri.clone(),
            actual_uri: existing.event_uri.clone(),
        });
    }
    if existing.payload_hash == event.payload_hash {
        Ok(ArtifactIndexEventWriteDecision::AlreadyExistsSamePayload)
    } else {
        Err(ArtifactIndexError::ImmutableEventOverwrite {
            event_uri: event.event_uri.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexSnapshotManifest {
    pub artifact_kind: ArtifactKind,
    pub snapshot_id: String,
    pub snapshot_uri: String,
    pub snapshot_content_hash: String,
    pub records: Vec<ArtifactIndexRecord>,
    pub lifecycle_state: LifecycleState,
}

impl ArtifactIndexSnapshotManifest {
    pub fn new(
        artifact_root: &str,
        artifact_kind: ArtifactKind,
        snapshot_id: impl Into<String>,
        snapshot_content_hash: &str,
        records: Vec<ArtifactIndexRecord>,
    ) -> Result<Self, ArtifactIndexError> {
        validate_artifact_root(artifact_root)?;
        validate_sha256_field("snapshot_content_hash", snapshot_content_hash)?;

        let snapshot_id = snapshot_id.into();
        validate_non_empty("snapshot_id", &snapshot_id)?;
        if records.is_empty() {
            return Err(ArtifactIndexError::SnapshotWithoutRecords);
        }

        let snapshot_uri = expected_snapshot_uri(artifact_root, artifact_kind, &snapshot_id);
        for record in &records {
            validate_snapshot_record(record, artifact_kind, &snapshot_id, &snapshot_uri)?;
        }

        Ok(Self {
            artifact_kind,
            snapshot_id,
            snapshot_uri,
            snapshot_content_hash: snapshot_content_hash.to_string(),
            records,
            lifecycle_state: LifecycleState::Active,
        })
    }

    pub fn new_with_computed_hash(
        artifact_root: &str,
        artifact_kind: ArtifactKind,
        snapshot_id: impl Into<String>,
        mut records: Vec<ArtifactIndexRecord>,
    ) -> Result<Self, ArtifactIndexError> {
        let snapshot_id = snapshot_id.into();
        sort_snapshot_records(&mut records);
        let snapshot_content_hash =
            computed_snapshot_content_hash(artifact_root, artifact_kind, &snapshot_id, &records)?;
        Self::new(
            artifact_root,
            artifact_kind,
            snapshot_id,
            &snapshot_content_hash,
            records,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexLatestPointer {
    pub artifact_kind: ArtifactKind,
    pub latest_pointer_uri: String,
    pub snapshot_id: String,
    pub snapshot_uri: String,
    pub snapshot_content_hash: String,
    pub lifecycle_state: LifecycleState,
}

impl ArtifactIndexLatestPointer {
    pub fn from_snapshot(
        artifact_root: &str,
        snapshot: &ArtifactIndexSnapshotManifest,
    ) -> Result<Self, ArtifactIndexError> {
        validate_artifact_root(artifact_root)?;
        if snapshot.lifecycle_state != LifecycleState::Active {
            return Err(ArtifactIndexError::HotIndexMetadataNotActive);
        }

        Ok(Self {
            artifact_kind: snapshot.artifact_kind,
            latest_pointer_uri: expected_latest_pointer_uri(artifact_root, snapshot.artifact_kind),
            snapshot_id: snapshot.snapshot_id.clone(),
            snapshot_uri: snapshot.snapshot_uri.clone(),
            snapshot_content_hash: snapshot.snapshot_content_hash.clone(),
            lifecycle_state: LifecycleState::Active,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexObservedPointer {
    pub pointer: ArtifactIndexLatestPointer,
    pub etag: String,
}

impl ArtifactIndexObservedPointer {
    pub fn new(
        pointer: ArtifactIndexLatestPointer,
        etag: impl Into<String>,
    ) -> Result<Self, ArtifactIndexError> {
        let etag = etag.into();
        validate_non_empty("etag", &etag)?;
        Ok(Self { pointer, etag })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactIndexPointerPrecondition {
    IfNoneMatchAny,
    IfMatch { etag: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIndexLatestPointerUpdatePlan {
    pub artifact_kind: ArtifactKind,
    pub latest_pointer_uri: String,
    pub new_snapshot_id: String,
    pub new_snapshot_uri: String,
    pub new_snapshot_content_hash: String,
    pub prior_snapshot_id: Option<String>,
    pub precondition: ArtifactIndexPointerPrecondition,
    pub writer_id: String,
    pub audit_epoch_uri: String,
}

impl ArtifactIndexLatestPointerUpdatePlan {
    pub const fn requires_retry_rebase_after_conditional_failure(&self) -> bool {
        true
    }
}

pub fn resolve_committed_snapshot<'a>(
    pointer: &ArtifactIndexLatestPointer,
    snapshot: &'a ArtifactIndexSnapshotManifest,
) -> Result<&'a ArtifactIndexSnapshotManifest, ArtifactIndexError> {
    if pointer.lifecycle_state != LifecycleState::Active
        || snapshot.lifecycle_state != LifecycleState::Active
    {
        return Err(ArtifactIndexError::HotIndexMetadataNotActive);
    }
    if pointer.artifact_kind != snapshot.artifact_kind
        || pointer.snapshot_id != snapshot.snapshot_id
        || pointer.snapshot_uri != snapshot.snapshot_uri
    {
        return Err(ArtifactIndexError::StaleLatestPointer {
            pointer_snapshot_id: pointer.snapshot_id.clone(),
            snapshot_id: snapshot.snapshot_id.clone(),
        });
    }
    if pointer.snapshot_content_hash != snapshot.snapshot_content_hash {
        return Err(ArtifactIndexError::SnapshotHashMismatch {
            pointer_hash: pointer.snapshot_content_hash.clone(),
            snapshot_hash: snapshot.snapshot_content_hash.clone(),
        });
    }
    Ok(snapshot)
}

pub fn resolve_lineage_parent<'a>(
    child: &ArtifactIndexRecord,
    parent: &'a ArtifactIndexRecord,
) -> Result<&'a ArtifactIndexRecord, ArtifactIndexError> {
    validate_lineage(&child.lineage_ids)?;
    validate_sha256(&parent.content_hash)?;

    if child.lineage_ids.iter().any(|lineage| {
        lineage.artifact_id == parent.artifact_id && lineage.content_hash == parent.content_hash
    }) {
        Ok(parent)
    } else {
        Err(ArtifactIndexError::LineageParentMismatch {
            child_artifact_id: child.artifact_id.clone(),
            parent_artifact_id: parent.artifact_id.clone(),
        })
    }
}

pub fn plan_latest_pointer_update(
    artifact_root: &str,
    writer_id: impl Into<String>,
    observed_prior: Option<&ArtifactIndexObservedPointer>,
    new_pointer: &ArtifactIndexLatestPointer,
    audit_epoch_id: impl Into<String>,
) -> Result<ArtifactIndexLatestPointerUpdatePlan, ArtifactIndexError> {
    validate_artifact_root(artifact_root)?;
    let writer_id = writer_id.into();
    validate_non_empty("writer_id", &writer_id)?;
    let audit_epoch_id = audit_epoch_id.into();
    validate_non_empty("audit_epoch_id", &audit_epoch_id)?;

    let expected_pointer_uri =
        expected_latest_pointer_uri(artifact_root, new_pointer.artifact_kind);
    if new_pointer.latest_pointer_uri != expected_pointer_uri {
        return Err(ArtifactIndexError::StaleLatestPointer {
            pointer_snapshot_id: new_pointer.snapshot_id.clone(),
            snapshot_id: expected_pointer_uri,
        });
    }

    let (prior_snapshot_id, precondition) = match observed_prior {
        Some(prior) => {
            if prior.pointer.artifact_kind != new_pointer.artifact_kind
                || prior.pointer.latest_pointer_uri != new_pointer.latest_pointer_uri
            {
                return Err(ArtifactIndexError::StaleLatestPointer {
                    pointer_snapshot_id: prior.pointer.snapshot_id.clone(),
                    snapshot_id: new_pointer.snapshot_id.clone(),
                });
            }
            (
                Some(prior.pointer.snapshot_id.clone()),
                ArtifactIndexPointerPrecondition::IfMatch {
                    etag: prior.etag.clone(),
                },
            )
        }
        None => (None, ArtifactIndexPointerPrecondition::IfNoneMatchAny),
    };

    Ok(ArtifactIndexLatestPointerUpdatePlan {
        artifact_kind: new_pointer.artifact_kind,
        latest_pointer_uri: new_pointer.latest_pointer_uri.clone(),
        new_snapshot_id: new_pointer.snapshot_id.clone(),
        new_snapshot_uri: new_pointer.snapshot_uri.clone(),
        new_snapshot_content_hash: new_pointer.snapshot_content_hash.clone(),
        prior_snapshot_id,
        precondition,
        writer_id,
        audit_epoch_uri: format!(
            "{}/artifact-index/v1/audit/epochs/{audit_epoch_id}.json",
            artifact_root.trim_end_matches('/')
        ),
    })
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
    LifecyclePolicy(String),
    SnapshotWithoutRecords,
    SnapshotRecordKindMismatch {
        artifact_id: String,
        expected_kind: ArtifactKind,
        actual_kind: ArtifactKind,
    },
    SnapshotRecordNotCommitted {
        artifact_id: String,
        commit_state: CommitState,
    },
    SnapshotRecordOutsideSnapshot {
        artifact_id: String,
    },
    StaleLatestPointer {
        pointer_snapshot_id: String,
        snapshot_id: String,
    },
    SnapshotHashMismatch {
        pointer_hash: String,
        snapshot_hash: String,
    },
    HotIndexMetadataNotActive,
    LineageParentMismatch {
        child_artifact_id: String,
        parent_artifact_id: String,
    },
    EventPayloadSerialization(String),
    EventUriMismatch {
        expected_uri: String,
        actual_uri: String,
    },
    ImmutableEventOverwrite {
        event_uri: String,
    },
    ResearchAnalyticsSubfamilyRequired,
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
            Self::LifecyclePolicy(message) => {
                write!(f, "artifact lifecycle policy rejected: {message}")
            }
            Self::SnapshotWithoutRecords => {
                write!(f, "artifact index snapshot requires at least one record")
            }
            Self::SnapshotRecordKindMismatch {
                artifact_id,
                expected_kind,
                actual_kind,
            } => write!(
                f,
                "artifact index snapshot record {artifact_id:?} has kind {actual_kind:?}, expected {expected_kind:?}"
            ),
            Self::SnapshotRecordNotCommitted {
                artifact_id,
                commit_state,
            } => write!(
                f,
                "artifact index snapshot record {artifact_id:?} must be committed, not {commit_state:?}"
            ),
            Self::SnapshotRecordOutsideSnapshot { artifact_id } => write!(
                f,
                "artifact index snapshot record {artifact_id:?} must reference the snapshot id and URI"
            ),
            Self::StaleLatestPointer {
                pointer_snapshot_id,
                snapshot_id,
            } => write!(
                f,
                "artifact index latest pointer is stale: pointer references {pointer_snapshot_id:?}, expected {snapshot_id:?}"
            ),
            Self::SnapshotHashMismatch {
                pointer_hash,
                snapshot_hash,
            } => write!(
                f,
                "artifact index latest pointer snapshot hash {pointer_hash:?} does not match snapshot hash {snapshot_hash:?}"
            ),
            Self::HotIndexMetadataNotActive => write!(
                f,
                "artifact index latest pointer and current snapshot metadata must remain in active storage"
            ),
            Self::LineageParentMismatch {
                child_artifact_id,
                parent_artifact_id,
            } => write!(
                f,
                "artifact index parent {parent_artifact_id:?} does not match manifest lineage ids and sha256 hashes for child {child_artifact_id:?}"
            ),
            Self::EventPayloadSerialization(message) => {
                write!(
                    f,
                    "artifact index event payload serialization failed: {message}"
                )
            }
            Self::EventUriMismatch {
                expected_uri,
                actual_uri,
            } => write!(
                f,
                "artifact index event URI mismatch: expected {expected_uri:?}, got {actual_uri:?}"
            ),
            Self::ImmutableEventOverwrite { event_uri } => write!(
                f,
                "artifact index immutable event overwrite rejected for {event_uri:?}"
            ),
            Self::ResearchAnalyticsSubfamilyRequired => write!(
                f,
                "research analytics artifact index records require a typed subfamily"
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
    artifact_subfamily: Option<&str>,
    manifest_uri: &str,
) -> Result<(), ArtifactIndexError> {
    let root = artifact_root.trim_end_matches('/');
    let required_prefix = if artifact_kind == ArtifactKind::ResearchAnalytics {
        let subfamily =
            artifact_subfamily.ok_or(ArtifactIndexError::ResearchAnalyticsSubfamilyRequired)?;
        format!(
            "{root}/{}/v1/{subfamily}/",
            artifact_kind.artifact_subpath()
        )
    } else {
        format!("{root}/{}/", artifact_kind.artifact_subpath())
    };
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

fn validate_snapshot_record(
    record: &ArtifactIndexRecord,
    artifact_kind: ArtifactKind,
    snapshot_id: &str,
    snapshot_uri: &str,
) -> Result<(), ArtifactIndexError> {
    if record.artifact_kind != artifact_kind {
        return Err(ArtifactIndexError::SnapshotRecordKindMismatch {
            artifact_id: record.artifact_id.clone(),
            expected_kind: artifact_kind,
            actual_kind: record.artifact_kind,
        });
    }
    if record.commit_state != CommitState::Committed {
        return Err(ArtifactIndexError::SnapshotRecordNotCommitted {
            artifact_id: record.artifact_id.clone(),
            commit_state: record.commit_state,
        });
    }
    if record.snapshot_id.as_deref() != Some(snapshot_id)
        || record.snapshot_uri.as_deref() != Some(snapshot_uri)
    {
        return Err(ArtifactIndexError::SnapshotRecordOutsideSnapshot {
            artifact_id: record.artifact_id.clone(),
        });
    }
    validate_sha256(&record.content_hash)?;
    validate_lineage(&record.lineage_ids)?;
    Ok(())
}

pub fn computed_snapshot_content_hash(
    artifact_root: &str,
    artifact_kind: ArtifactKind,
    snapshot_id: &str,
    records: &[ArtifactIndexRecord],
) -> Result<String, ArtifactIndexError> {
    validate_artifact_root(artifact_root)?;
    validate_non_empty("snapshot_id", snapshot_id)?;
    if records.is_empty() {
        return Err(ArtifactIndexError::SnapshotWithoutRecords);
    }
    let snapshot_uri = expected_snapshot_uri(artifact_root, artifact_kind, snapshot_id);
    let mut sorted_records = records.to_vec();
    sort_snapshot_records(&mut sorted_records);
    for record in &sorted_records {
        validate_snapshot_record(record, artifact_kind, snapshot_id, &snapshot_uri)?;
    }
    let payload = ArtifactIndexSnapshotHashPayload {
        schema_version: "artifact-index-snapshot-content-hash.v1",
        artifact_kind,
        snapshot_id,
        snapshot_uri: snapshot_uri.as_str(),
        lifecycle_state: LifecycleState::Active,
        records: &sorted_records,
    };
    let bytes = serde_json::to_vec(&payload)
        .map_err(|err| ArtifactIndexError::EventPayloadSerialization(err.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}

#[derive(Serialize)]
struct ArtifactIndexSnapshotHashPayload<'a> {
    schema_version: &'static str,
    artifact_kind: ArtifactKind,
    snapshot_id: &'a str,
    snapshot_uri: &'a str,
    lifecycle_state: LifecycleState,
    records: &'a [ArtifactIndexRecord],
}

fn sort_snapshot_records(records: &mut [ArtifactIndexRecord]) {
    records.sort_by(|left, right| {
        (
            left.artifact_kind.as_str(),
            left.artifact_subfamily.as_deref().unwrap_or(""),
            left.artifact_id.as_str(),
            left.content_hash.as_str(),
            left.manifest_uri.as_str(),
        )
            .cmp(&(
                right.artifact_kind.as_str(),
                right.artifact_subfamily.as_deref().unwrap_or(""),
                right.artifact_id.as_str(),
                right.content_hash.as_str(),
                right.manifest_uri.as_str(),
            ))
    });
}

fn expected_latest_pointer_uri(artifact_root: &str, artifact_kind: ArtifactKind) -> String {
    format!(
        "{}/artifact-index/v1/pointers/kind={}/latest.json",
        artifact_root.trim_end_matches('/'),
        artifact_kind.as_str()
    )
}

fn expected_snapshot_uri(
    artifact_root: &str,
    artifact_kind: ArtifactKind,
    snapshot_id: &str,
) -> String {
    format!(
        "{}/artifact-index/v1/snapshots/kind={}/snapshot_id={snapshot_id}/manifest.json",
        artifact_root.trim_end_matches('/'),
        artifact_kind.as_str()
    )
}

fn event_payload_hash(record: &ArtifactIndexRecord) -> Result<String, ArtifactIndexError> {
    let bytes = serde_json::to_vec(record)
        .map_err(|err| ArtifactIndexError::EventPayloadSerialization(err.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
}
