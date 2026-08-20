//! Bounded Artifact Index commit proof.
//!
//! This proves the selected object-store path can execute the commit sequence
//! required by the Artifact Index contract: immutable event/snapshot writes,
//! pre-CAS audit intents, first latest-pointer creation, ETag-guarded
//! latest-pointer update, stale ETag rejection, and readback resolution.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, ensure};
use bytes::Bytes;
use nautilus_persistence::parquet::create_object_store_from_path;
use object_store::{
    Error as ObjectStoreError, ObjectStore, ObjectStoreExt, PutMode, UpdateVersion,
    path::Path as ObjectPath,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::hashing::sha256_hex;
use crate::{
    artifact_index::{
        ArtifactIndexEventObject, ArtifactIndexLatestPointer, ArtifactIndexLineageRef,
        ArtifactIndexObservedPointer, ArtifactIndexPointerPrecondition, ArtifactIndexRecord,
        ArtifactIndexSnapshotManifest, ArtifactKind, ResearchAnalyticsSubfamily,
        artifact_index_audit_kind, plan_index_event_create, plan_latest_pointer_update,
        resolve_committed_snapshot,
    },
    run_manifest::{ManifestArtifactStore, artifact_store_storage_options_for_uri},
};

pub const ARTIFACT_INDEX_COMMIT_PROOF_SCHEMA_VERSION: &str = "artifact-index-commit-proof.v2";
pub const ARTIFACT_INDEX_COMMIT_PROOF_V1_SCHEMA_VERSION: &str = "artifact-index-commit-proof.v1";
pub const ARTIFACT_INDEX_COMMIT_PROOF_REPORT_FILE: &str = "artifact-index-commit-proof-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexCommitProofSpec {
    pub proof_id: String,
    pub artifact_root: String,
    pub output_dir: PathBuf,
    pub artifact_store: ManifestArtifactStore,
    pub artifact_kind: ArtifactKind,
    pub producer_project: String,
    pub writer_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub research_analytics_subfamily: Option<ResearchAnalyticsSubfamily>,
    #[serde(default)]
    pub denied_artifact_kinds: Vec<ArtifactKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexCommitProofReport {
    pub schema_version: String,
    pub proof_id: String,
    pub artifact_root: String,
    pub artifact_protocol: String,
    pub artifact_kind: ArtifactKind,
    pub producer_project: String,
    pub writer_id: String,
    pub storage_option_keys: Vec<String>,
    pub event_uris: Vec<String>,
    pub snapshot_uris: Vec<String>,
    pub latest_pointer_uri: String,
    pub audit_intent_uris: Vec<String>,
    pub first_pointer_precondition: ArtifactIndexPointerPrecondition,
    pub second_pointer_precondition: ArtifactIndexPointerPrecondition,
    pub prior_pointer_etag_observed: bool,
    pub final_pointer_etag_observed: bool,
    pub event_create_only_proven: bool,
    pub snapshot_create_only_proven: bool,
    pub audit_intent_create_only_proven: bool,
    pub latest_pointer_create_only_proven: bool,
    pub latest_pointer_update_if_match_proven: bool,
    pub stale_etag_update_rejected: bool,
    pub latest_pointer_readback_proven: bool,
    pub snapshot_readback_proven: bool,
    pub resolved_snapshot_id: String,
    pub final_snapshot_id: String,
    pub final_snapshot_content_hash: String,
    pub persisted_final_snapshot_json_sha256: String,
    pub direct_s3_commit_proven: bool,
    pub producer_iam_scope_proven: bool,
    pub producer_iam_scope_denied_kinds: Vec<ArtifactKind>,
    pub producer_iam_scope_denied_write_attempts: usize,
    pub producer_iam_scope_denied_write_rejections: usize,
    pub producer_iam_scope_violation_count: usize,
    pub producer_iam_scope_violation_uris: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexCommitProofReportV1 {
    pub schema_version: String,
    pub proof_id: String,
    pub artifact_root: String,
    pub artifact_protocol: String,
    pub artifact_kind: ArtifactKind,
    pub producer_project: String,
    pub writer_id: String,
    pub storage_option_keys: Vec<String>,
    pub event_uris: Vec<String>,
    pub snapshot_uris: Vec<String>,
    pub latest_pointer_uri: String,
    pub audit_epoch_uris: Vec<String>,
    pub first_pointer_precondition: ArtifactIndexPointerPrecondition,
    pub second_pointer_precondition: ArtifactIndexPointerPrecondition,
    pub prior_pointer_etag_observed: bool,
    pub final_pointer_etag_observed: bool,
    pub event_create_only_proven: bool,
    pub snapshot_create_only_proven: bool,
    pub audit_epoch_create_only_proven: bool,
    pub latest_pointer_create_only_proven: bool,
    pub latest_pointer_update_if_match_proven: bool,
    pub stale_etag_update_rejected: bool,
    pub latest_pointer_readback_proven: bool,
    pub snapshot_readback_proven: bool,
    pub resolved_snapshot_id: String,
    pub final_snapshot_id: String,
    pub final_snapshot_content_hash: String,
    pub persisted_final_snapshot_json_sha256: String,
    pub direct_s3_commit_proven: bool,
    pub producer_iam_scope_proven: bool,
    pub producer_iam_scope_denied_kinds: Vec<ArtifactKind>,
    pub producer_iam_scope_denied_write_attempts: usize,
    pub producer_iam_scope_denied_write_rejections: usize,
    pub producer_iam_scope_violation_count: usize,
    pub producer_iam_scope_violation_uris: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactIndexCommitProofEvidence {
    V1(ArtifactIndexCommitProofReportV1),
    V2(ArtifactIndexCommitProofReport),
}

impl ArtifactIndexCommitProofEvidence {
    #[must_use]
    pub fn proof_id(&self) -> &str {
        match self {
            Self::V1(report) => &report.proof_id,
            Self::V2(report) => &report.proof_id,
        }
    }

    #[must_use]
    pub const fn artifact_kind(&self) -> ArtifactKind {
        match self {
            Self::V1(report) => report.artifact_kind,
            Self::V2(report) => report.artifact_kind,
        }
    }

    #[must_use]
    pub fn artifact_root(&self) -> &str {
        match self {
            Self::V1(report) => &report.artifact_root,
            Self::V2(report) => &report.artifact_root,
        }
    }

    #[must_use]
    pub const fn direct_s3_commit_proven(&self) -> bool {
        match self {
            Self::V1(report) => report.direct_s3_commit_proven,
            Self::V2(report) => report.direct_s3_commit_proven,
        }
    }

    #[must_use]
    pub const fn producer_iam_scope_proven(&self) -> bool {
        match self {
            Self::V1(report) => report.producer_iam_scope_proven,
            Self::V2(report) => report.producer_iam_scope_proven,
        }
    }

    #[must_use]
    pub const fn as_v2(&self) -> Option<&ArtifactIndexCommitProofReport> {
        match self {
            Self::V1(_) => None,
            Self::V2(report) => Some(report),
        }
    }
}

impl From<ArtifactIndexCommitProofReport> for ArtifactIndexCommitProofEvidence {
    fn from(report: ArtifactIndexCommitProofReport) -> Self {
        Self::V2(report)
    }
}

impl<'de> Deserialize<'de> for ArtifactIndexCommitProofEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let schema_version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::custom("missing string schema_version"))?;
        match schema_version {
            ARTIFACT_INDEX_COMMIT_PROOF_V1_SCHEMA_VERSION => serde_json::from_value(value)
                .map(Self::V1)
                .map_err(serde::de::Error::custom),
            ARTIFACT_INDEX_COMMIT_PROOF_SCHEMA_VERSION => serde_json::from_value(value)
                .map(Self::V2)
                .map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!(
                "unsupported Artifact Index commit proof schema {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIndexCommitProofArtifact {
    pub report_path: PathBuf,
    pub content_hash: String,
    pub report_bytes: u64,
    pub artifact_root: String,
    pub latest_pointer_uri: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactIndexIamProbeObject {
    schema_version: String,
    proof_id: String,
    writer_id: String,
    denied_artifact_kind: ArtifactKind,
    probe_path_kind: ArtifactIndexIamProbePathKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactIndexIamProbePathKind {
    Event,
    Snapshot,
    LatestPointer,
    AuditIntent,
}

impl ArtifactIndexIamProbePathKind {
    /// Every probe path kind, in emission order. `denied_probe_uris` emits
    /// exactly one URI per kind via an exhaustive match, and the IAM plan's
    /// `expected_denied_write_attempts` derives from this slice's length, so
    /// adding a kind updates both sides or fails to compile.
    pub(crate) const ALL: [Self; 4] = [
        Self::Event,
        Self::Snapshot,
        Self::LatestPointer,
        Self::AuditIntent,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArtifactIndexIamScopeProbeSummary {
    denied_write_attempts: usize,
    denied_write_rejections: usize,
    violation_uris: Vec<String>,
}

pub fn run_artifact_index_commit_proof_from_spec_file_with_resolver<F>(
    spec_path: &Path,
    resolver: &mut F,
) -> Result<ArtifactIndexCommitProofArtifact>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read Artifact Index commit proof spec {}",
            spec_path.display()
        )
    })?;
    let spec: ArtifactIndexCommitProofSpec = toml::from_slice(&bytes).with_context(|| {
        format!(
            "parse Artifact Index commit proof spec TOML {}",
            spec_path.display()
        )
    })?;
    run_artifact_index_commit_proof_with_resolver(&spec, resolver)
}

pub fn run_artifact_index_commit_proof_with_resolver<F>(
    spec: &ArtifactIndexCommitProofSpec,
    resolver: &mut F,
) -> Result<ArtifactIndexCommitProofArtifact>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    validate_spec(spec)?;
    ensure!(
        artifact_protocol(&spec.artifact_root) == "s3",
        "configured Artifact Index commit proof requires an s3 artifact_root"
    );
    let storage_options =
        artifact_store_storage_options_for_uri(&spec.artifact_root, &spec.artifact_store, resolver)
            .map_err(|error| anyhow!("resolve artifact-store options: {error}"))?;
    let storage_option_keys = storage_options
        .as_ref()
        .map(|options| options.keys().cloned().collect())
        .unwrap_or_default();
    let object_store_options = storage_options.map(|options| options.into_iter().collect());
    let (object_store, base_path, _) =
        create_object_store_from_path(&spec.artifact_root, object_store_options)
            .with_context(|| format!("open artifact_root {}", spec.artifact_root))?;
    run_artifact_index_commit_proof_with_object_store(
        spec,
        storage_option_keys,
        object_store,
        ObjectPath::from(base_path),
        true,
    )
}

pub fn run_artifact_index_commit_proof_with_object_store(
    spec: &ArtifactIndexCommitProofSpec,
    storage_option_keys: Vec<String>,
    object_store: Arc<dyn ObjectStore>,
    artifact_root_object_path: ObjectPath,
    direct_s3_store: bool,
) -> Result<ArtifactIndexCommitProofArtifact> {
    validate_spec(spec)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build Artifact Index commit proof runtime")?;
    let report = runtime.block_on(execute_commit_sequence(
        spec,
        storage_option_keys,
        object_store,
        artifact_root_object_path,
        direct_s3_store,
    ))?;
    write_report(&spec.output_dir, &report).map(|(report_path, content_hash, report_bytes)| {
        ArtifactIndexCommitProofArtifact {
            report_path,
            content_hash,
            report_bytes,
            artifact_root: spec.artifact_root.clone(),
            latest_pointer_uri: report.latest_pointer_uri,
        }
    })
}

async fn execute_commit_sequence(
    spec: &ArtifactIndexCommitProofSpec,
    storage_option_keys: Vec<String>,
    object_store: Arc<dyn ObjectStore>,
    artifact_root_object_path: ObjectPath,
    direct_s3_store: bool,
) -> Result<ArtifactIndexCommitProofReport> {
    let first_snapshot_id = format!("{}-snapshot-a", spec.proof_id);
    let second_snapshot_id = format!("{}-snapshot-b", spec.proof_id);
    let first_staged = staged_record(spec, "artifact-a")?;
    let second_staged = staged_record(spec, "artifact-b")?;
    let first_snapshot_uri =
        snapshot_uri(&spec.artifact_root, spec.artifact_kind, &first_snapshot_id);
    let second_snapshot_uri =
        snapshot_uri(&spec.artifact_root, spec.artifact_kind, &second_snapshot_id);
    let first_committed =
        first_staged.committed_for_snapshot(&first_snapshot_id, &first_snapshot_uri)?;
    let second_committed =
        second_staged.committed_for_snapshot(&second_snapshot_id, &second_snapshot_uri)?;
    let first_snapshot = ArtifactIndexSnapshotManifest::new_with_computed_hash(
        &spec.artifact_root,
        spec.artifact_kind,
        &first_snapshot_id,
        vec![first_committed],
    )?;
    let second_snapshot = ArtifactIndexSnapshotManifest::new_with_computed_hash(
        &spec.artifact_root,
        spec.artifact_kind,
        &second_snapshot_id,
        vec![second_committed],
    )?;
    let first_pointer =
        ArtifactIndexLatestPointer::from_snapshot(&spec.artifact_root, &first_snapshot)?;
    let second_pointer =
        ArtifactIndexLatestPointer::from_snapshot(&spec.artifact_root, &second_snapshot)?;
    let first_plan =
        plan_latest_pointer_update(&spec.artifact_root, &spec.writer_id, None, &first_pointer)?;

    let first_event = ArtifactIndexEventObject::from_record(&first_staged)?;
    let second_event = ArtifactIndexEventObject::from_record(&second_staged)?;
    ensure!(
        matches!(
            plan_index_event_create(&spec.producer_project, &first_event, None)?,
            crate::artifact_index::ArtifactIndexEventWriteDecision::CreateWithIfNoneMatchAny
        ),
        "first event plan must be create-only"
    );
    ensure!(
        matches!(
            plan_index_event_create(&spec.producer_project, &second_event, None)?,
            crate::artifact_index::ArtifactIndexEventWriteDecision::CreateWithIfNoneMatchAny
        ),
        "second event plan must be create-only"
    );

    put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &first_event.event_uri,
        )?,
        &first_event,
    )
    .await
    .with_context(|| format!("create first index event {}", first_event.event_uri))?;
    put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &first_snapshot.snapshot_uri,
        )?,
        &first_snapshot,
    )
    .await
    .with_context(|| format!("create first snapshot {}", first_snapshot.snapshot_uri))?;
    put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &first_plan.audit_intent_uri,
        )?,
        &first_plan.audit_intent,
    )
    .await
    .with_context(|| format!("create first audit intent {}", first_plan.audit_intent_uri))?;
    let first_pointer_result = put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &first_pointer.latest_pointer_uri,
        )?,
        &first_pointer,
    )
    .await
    .with_context(|| {
        format!(
            "create first latest pointer {}",
            first_pointer.latest_pointer_uri
        )
    })?;
    let first_etag = if let Some(etag) = first_pointer_result.e_tag {
        etag
    } else {
        object_store
            .head(&path_for_uri(
                &spec.artifact_root,
                &artifact_root_object_path,
                &first_pointer.latest_pointer_uri,
            )?)
            .await
            .context("head first latest pointer for ETag")?
            .e_tag
            .ok_or_else(|| anyhow!("first pointer create did not return an ETag"))?
    };
    let observed_first = ArtifactIndexObservedPointer::new(first_pointer, first_etag.clone())?;
    let second_plan = plan_latest_pointer_update(
        &spec.artifact_root,
        &spec.writer_id,
        Some(&observed_first),
        &second_pointer,
    )?;
    put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &second_event.event_uri,
        )?,
        &second_event,
    )
    .await
    .with_context(|| format!("create second index event {}", second_event.event_uri))?;
    put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &second_snapshot.snapshot_uri,
        )?,
        &second_snapshot,
    )
    .await
    .with_context(|| format!("create second snapshot {}", second_snapshot.snapshot_uri))?;
    put_create_json(
        object_store.as_ref(),
        &path_for_uri(
            &spec.artifact_root,
            &artifact_root_object_path,
            &second_plan.audit_intent_uri,
        )?,
        &second_plan.audit_intent,
    )
    .await
    .with_context(|| {
        format!(
            "create second audit intent {}",
            second_plan.audit_intent_uri
        )
    })?;
    let pointer_path = path_for_uri(
        &spec.artifact_root,
        &artifact_root_object_path,
        &second_pointer.latest_pointer_uri,
    )?;
    let second_pointer_result = put_update_json_if_match(
        object_store.as_ref(),
        &pointer_path,
        first_etag.clone(),
        &second_pointer,
    )
    .await
    .with_context(|| {
        format!(
            "update latest pointer {}",
            second_pointer.latest_pointer_uri
        )
    })?;
    let second_etag = if let Some(etag) = second_pointer_result.e_tag {
        etag
    } else {
        object_store
            .head(&pointer_path)
            .await
            .context("head second latest pointer for ETag")?
            .e_tag
            .ok_or_else(|| anyhow!("second pointer update did not return an ETag"))?
    };
    let stale_update = put_update_json_if_match(
        object_store.as_ref(),
        &pointer_path,
        first_etag,
        &observed_first.pointer,
    )
    .await;
    let stale_etag_update_rejected = match stale_update {
        Ok(_) => false,
        Err(error) if is_conditional_rejection(&error) => true,
        Err(error) => return Err(error).context("stale pointer ETag update failed unexpectedly"),
    };
    ensure!(
        stale_etag_update_rejected,
        "stale pointer ETag update unexpectedly succeeded"
    );

    let read_pointer: ArtifactIndexLatestPointer =
        get_json(object_store.as_ref(), &pointer_path).await?;
    let read_snapshot_path = path_for_uri(
        &spec.artifact_root,
        &artifact_root_object_path,
        &read_pointer.snapshot_uri,
    )?;
    let read_snapshot: ArtifactIndexSnapshotManifest =
        get_json(object_store.as_ref(), &read_snapshot_path).await?;
    let resolved = resolve_committed_snapshot(&read_pointer, &read_snapshot)?;
    let snapshot_bytes = crate::reference_artifact::canonical_json_bytes(&read_snapshot)
        .context("serialize readback snapshot")?;
    let iam_scope_probe =
        probe_producer_iam_scope(spec, object_store.as_ref(), &artifact_root_object_path).await?;
    let producer_iam_scope_proven = !spec.denied_artifact_kinds.is_empty()
        && iam_scope_probe.denied_write_attempts == iam_scope_probe.denied_write_rejections
        && iam_scope_probe.violation_uris.is_empty();

    Ok(ArtifactIndexCommitProofReport {
        schema_version: ARTIFACT_INDEX_COMMIT_PROOF_SCHEMA_VERSION.to_string(),
        proof_id: spec.proof_id.clone(),
        artifact_root: spec.artifact_root.clone(),
        artifact_protocol: artifact_protocol(&spec.artifact_root).to_string(),
        artifact_kind: spec.artifact_kind,
        producer_project: spec.producer_project.clone(),
        writer_id: spec.writer_id.clone(),
        storage_option_keys,
        event_uris: vec![first_event.event_uri, second_event.event_uri],
        snapshot_uris: vec![first_snapshot.snapshot_uri, second_snapshot.snapshot_uri],
        latest_pointer_uri: read_pointer.latest_pointer_uri.clone(),
        audit_intent_uris: vec![first_plan.audit_intent_uri, second_plan.audit_intent_uri],
        first_pointer_precondition: first_plan.precondition,
        second_pointer_precondition: second_plan.precondition,
        prior_pointer_etag_observed: true,
        final_pointer_etag_observed: !second_etag.is_empty(),
        event_create_only_proven: true,
        snapshot_create_only_proven: true,
        audit_intent_create_only_proven: true,
        latest_pointer_create_only_proven: true,
        latest_pointer_update_if_match_proven: true,
        stale_etag_update_rejected,
        latest_pointer_readback_proven: true,
        snapshot_readback_proven: true,
        resolved_snapshot_id: resolved.snapshot_id.clone(),
        final_snapshot_id: read_snapshot.snapshot_id,
        final_snapshot_content_hash: read_snapshot.snapshot_content_hash,
        persisted_final_snapshot_json_sha256: sha256_hex(&snapshot_bytes),
        direct_s3_commit_proven: direct_s3_store && artifact_protocol(&spec.artifact_root) == "s3",
        producer_iam_scope_proven,
        producer_iam_scope_denied_kinds: spec.denied_artifact_kinds.clone(),
        producer_iam_scope_denied_write_attempts: iam_scope_probe.denied_write_attempts,
        producer_iam_scope_denied_write_rejections: iam_scope_probe.denied_write_rejections,
        producer_iam_scope_violation_count: iam_scope_probe.violation_uris.len(),
        producer_iam_scope_violation_uris: iam_scope_probe.violation_uris,
    })
}

async fn probe_producer_iam_scope(
    spec: &ArtifactIndexCommitProofSpec,
    object_store: &dyn ObjectStore,
    artifact_root_object_path: &ObjectPath,
) -> Result<ArtifactIndexIamScopeProbeSummary> {
    let mut denied_write_attempts = 0;
    let mut denied_write_rejections = 0;
    let mut violation_uris = Vec::new();
    for denied_kind in &spec.denied_artifact_kinds {
        for (path_kind, uri) in denied_probe_uris(spec, *denied_kind) {
            denied_write_attempts += 1;
            let object_path = path_for_uri(&spec.artifact_root, artifact_root_object_path, &uri)?;
            let probe = ArtifactIndexIamProbeObject {
                schema_version: "artifact-index-iam-scope-probe.v1".to_string(),
                proof_id: spec.proof_id.clone(),
                writer_id: spec.writer_id.clone(),
                denied_artifact_kind: *denied_kind,
                probe_path_kind: path_kind,
            };
            match put_create_json(object_store, &object_path, &probe).await {
                Ok(_) => violation_uris.push(uri),
                Err(error) if is_permission_rejection(&error) => denied_write_rejections += 1,
                Err(error) if is_existing_object(&error) => violation_uris.push(uri),
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("probe denied Artifact Index path {uri}"));
                }
            }
        }
    }
    Ok(ArtifactIndexIamScopeProbeSummary {
        denied_write_attempts,
        denied_write_rejections,
        violation_uris,
    })
}

fn denied_probe_uris(
    spec: &ArtifactIndexCommitProofSpec,
    denied_kind: ArtifactKind,
) -> Vec<(ArtifactIndexIamProbePathKind, String)> {
    let root = spec.artifact_root.trim_end_matches('/');
    let kind = denied_kind.as_str();
    let audit_kind = artifact_index_audit_kind(denied_kind).as_str();
    let hash = sha256_hex(format!("{}:{kind}:iam-scope-probe", spec.proof_id).as_bytes());
    ArtifactIndexIamProbePathKind::ALL
        .into_iter()
        .map(|path_kind| {
            let uri = match path_kind {
                ArtifactIndexIamProbePathKind::Event => format!(
                    "{root}/artifact-index/v1/events/kind={kind}/artifact_id={}-denied-{kind}/hash={hash}.json",
                    spec.proof_id
                ),
                ArtifactIndexIamProbePathKind::Snapshot => format!(
                    "{root}/artifact-index/v1/snapshots/kind={kind}/snapshot_id={}-denied-{kind}/manifest.json",
                    spec.proof_id
                ),
                ArtifactIndexIamProbePathKind::LatestPointer => {
                    format!("{root}/artifact-index/v1/pointers/kind={kind}/latest.json")
                }
                ArtifactIndexIamProbePathKind::AuditIntent => format!(
                    "{root}/artifact-index/v1/audit/intents/v1/kind={audit_kind}/{hash}.json"
                ),
            };
            (path_kind, uri)
        })
        .collect()
}

fn staged_record(spec: &ArtifactIndexCommitProofSpec, suffix: &str) -> Result<ArtifactIndexRecord> {
    let artifact_id = format!("{}-{suffix}", spec.proof_id);
    let content_hash = sha256_hex(format!("{}:{suffix}:artifact", spec.proof_id).as_bytes());
    let lineage_hash = sha256_hex(format!("{}:{suffix}:lineage", spec.proof_id).as_bytes());
    let manifest_uri = manifest_uri(spec, suffix);
    let lineage = vec![ArtifactIndexLineageRef::new(
        format!("{}-{suffix}-parent", spec.proof_id),
        Some(1),
        lineage_hash,
    )];
    if spec.artifact_kind == ArtifactKind::ResearchAnalytics {
        let subfamily = spec
            .research_analytics_subfamily
            .ok_or_else(|| anyhow!("research_analytics_subfamily is required for RA proofs"))?;
        return ArtifactIndexRecord::new_research_analytics_staged(
            &spec.artifact_root,
            subfamily,
            artifact_id,
            &spec.producer_project,
            manifest_uri,
            &content_hash,
            lineage,
        )
        .map_err(Into::into);
    }
    ArtifactIndexRecord::new_staged(
        &spec.artifact_root,
        spec.artifact_kind,
        artifact_id,
        &spec.producer_project,
        manifest_uri,
        &content_hash,
        lineage,
    )
    .map_err(Into::into)
}

fn manifest_uri(spec: &ArtifactIndexCommitProofSpec, suffix: &str) -> String {
    let root = spec.artifact_root.trim_end_matches('/');
    if spec.artifact_kind == ArtifactKind::ResearchAnalytics {
        let subfamily = spec
            .research_analytics_subfamily
            .map(|value| value.as_str())
            .unwrap_or("datasets");
        return format!(
            "{root}/research-analytics/v1/{subfamily}/proofs/{}/{}.manifest.json",
            spec.proof_id, suffix
        );
    }
    format!(
        "{root}/{}/proofs/{}/{}.manifest.json",
        artifact_kind_subpath(spec.artifact_kind),
        spec.proof_id,
        suffix
    )
}

async fn put_create_json<T>(
    object_store: &dyn ObjectStore,
    path: &ObjectPath,
    value: &T,
) -> Result<object_store::PutResult>
where
    T: Serialize,
{
    let bytes = crate::reference_artifact::canonical_json_bytes(value)
        .context("serialize create-only object")?;
    object_store
        .put_opts(path, Bytes::from(bytes).into(), PutMode::Create.into())
        .await
        .map_err(Into::into)
}

async fn put_update_json_if_match<T>(
    object_store: &dyn ObjectStore,
    path: &ObjectPath,
    etag: String,
    value: &T,
) -> Result<object_store::PutResult>
where
    T: Serialize,
{
    let bytes = crate::reference_artifact::canonical_json_bytes(value)
        .context("serialize conditional update object")?;
    object_store
        .put_opts(
            path,
            Bytes::from(bytes).into(),
            PutMode::Update(UpdateVersion {
                e_tag: Some(etag),
                version: None,
            })
            .into(),
        )
        .await
        .map_err(Into::into)
}

async fn get_json<T>(object_store: &dyn ObjectStore, path: &ObjectPath) -> Result<T>
where
    T: DeserializeOwned,
{
    let bytes = object_store
        .get(path)
        .await
        .with_context(|| format!("get {path}"))?
        .bytes()
        .await
        .with_context(|| format!("read {path}"))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse JSON {path}"))
}

fn path_for_uri(
    artifact_root: &str,
    artifact_root_object_path: &ObjectPath,
    uri: &str,
) -> Result<ObjectPath> {
    let root = artifact_root.trim_end_matches('/');
    let suffix = uri.strip_prefix(root).ok_or_else(|| {
        anyhow!(
            "URI {uri} is outside artifact_root {}",
            artifact_root.trim_end_matches('/')
        )
    })?;
    let relative = suffix.trim_start_matches('/');
    ensure!(
        !relative.is_empty(),
        "URI {uri} does not identify an object"
    );
    let root_path = artifact_root_object_path.to_string();
    let object_path = if root_path.is_empty() {
        relative.to_string()
    } else {
        format!("{root_path}/{relative}")
    };
    Ok(ObjectPath::from(object_path))
}

fn snapshot_uri(artifact_root: &str, artifact_kind: ArtifactKind, snapshot_id: &str) -> String {
    format!(
        "{}/artifact-index/v1/snapshots/kind={}/snapshot_id={snapshot_id}/manifest.json",
        artifact_root.trim_end_matches('/'),
        artifact_kind.as_str()
    )
}

fn artifact_kind_subpath(artifact_kind: ArtifactKind) -> &'static str {
    match artifact_kind {
        ArtifactKind::Raw => "raw",
        ArtifactKind::NtCatalog => "nt-catalog",
        ArtifactKind::SourceProofs => "source-proofs",
        ArtifactKind::Backtests => "backtests",
        ArtifactKind::ArtifactIndex => "artifact-index",
        ArtifactKind::ResearchAnalytics => "research-analytics",
    }
}

fn validate_spec(spec: &ArtifactIndexCommitProofSpec) -> Result<()> {
    validate_path_component("proof_id", &spec.proof_id)?;
    validate_path_component("producer_project", &spec.producer_project)?;
    validate_path_component("writer_id", &spec.writer_id)?;
    ensure!(
        !spec.artifact_root.trim().is_empty(),
        "artifact_root must not be empty"
    );
    ensure!(
        matches!(artifact_protocol(&spec.artifact_root), "file" | "s3"),
        "artifact_root protocol must be file or s3"
    );
    if spec.artifact_kind == ArtifactKind::ResearchAnalytics {
        ensure!(
            spec.research_analytics_subfamily.is_some(),
            "research_analytics_subfamily is required for research_analytics"
        );
    } else {
        ensure!(
            spec.research_analytics_subfamily.is_none(),
            "research_analytics_subfamily is only valid for research_analytics"
        );
    }
    ensure!(
        !spec.denied_artifact_kinds.contains(&spec.artifact_kind),
        "denied_artifact_kinds must not include artifact_kind"
    );
    Ok(())
}

fn validate_path_component(field: &'static str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    ensure!(
        value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')),
        "{field} may contain only ASCII letters, digits, hyphen, underscore, or dot"
    );
    Ok(())
}

fn artifact_protocol(uri: &str) -> &str {
    uri.split_once("://")
        .map(|(protocol, _)| protocol)
        .unwrap_or("")
}

fn is_conditional_rejection(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ObjectStoreError>()
        .is_some_and(|error| matches!(error, ObjectStoreError::Precondition { .. }))
}

fn is_permission_rejection(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ObjectStoreError>()
        .is_some_and(|error| {
            matches!(
                error,
                ObjectStoreError::PermissionDenied { .. }
                    | ObjectStoreError::Unauthenticated { .. }
            )
        })
}

fn is_existing_object(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ObjectStoreError>()
        .is_some_and(|error| matches!(error, ObjectStoreError::AlreadyExists { .. }))
}

fn write_report(
    output_dir: &Path,
    report: &ArtifactIndexCommitProofReport,
) -> Result<(PathBuf, String, u64)> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "create Artifact Index commit proof output dir {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(ARTIFACT_INDEX_COMMIT_PROOF_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        ARTIFACT_INDEX_COMMIT_PROOF_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| {
        format!(
            "write Artifact Index commit proof report {}",
            path.display()
        )
    })?;
    Ok((path, written.pin.sha256, written.bytes))
}
