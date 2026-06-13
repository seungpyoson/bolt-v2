use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use object_store::{ObjectStore, ObjectStoreExt, PutMode, UpdateVersion, path::Path as ObjectPath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::run_manifest::MarketStructureFixture;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStoreConfig {
    pub artifact_root: String,
    pub subpaths: ArtifactSubpaths,
    pub lifecycle: ArtifactLifecycleConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSubpaths {
    pub raw: String,
    pub nt_catalog: String,
    pub source_proofs: String,
    pub backtests: String,
    pub artifact_index: String,
    pub research_analytics: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactLifecycleConfig {
    pub retention: String,
    pub default_delete_expiration: String,
    pub storage_profiles: Vec<ArtifactStorageProfile>,
    pub quiet_window_seconds: ArtifactQuietWindowSeconds,
    pub hot_index: ArtifactIndexHotLifecycleConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactQuietWindowSeconds {
    pub raw: u64,
    pub nt_catalog: u64,
    pub source_proofs: u64,
    pub backtests: u64,
    pub artifact_index: u64,
    pub research_analytics: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexHotLifecycleConfig {
    pub latest_pointer_storage_profile: ArtifactStorageProfile,
    pub current_snapshot_storage_profile: ArtifactStorageProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactLifecyclePolicy {
    quiet_window_seconds: ArtifactQuietWindowSeconds,
    hot_index: ArtifactIndexHotLifecycleConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStorageProfile {
    Active,
    Archive,
    DeepArchive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactLifecycleState {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactKind {
    Raw,
    NtCatalog,
    SourceProofs,
    Backtests,
    ArtifactIndex,
    ResearchAnalytics,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArtifactRoot {
    artifact_root: String,
    subpaths: ArtifactSubpaths,
    lifecycle: ArtifactLifecyclePolicy,
}

impl ArtifactStoreConfig {
    /// # Errors
    ///
    /// Returns an error when the configured canonical root or subpaths are not
    /// valid artifact-store paths.
    pub fn resolve(&self) -> Result<ResolvedArtifactRoot> {
        let artifact_root = normalize_artifact_root(&self.artifact_root)?;
        let subpaths = ArtifactSubpaths {
            raw: normalize_subpath("subpaths.raw", &self.subpaths.raw)?,
            nt_catalog: normalize_subpath("subpaths.nt_catalog", &self.subpaths.nt_catalog)?,
            source_proofs: normalize_subpath(
                "subpaths.source_proofs",
                &self.subpaths.source_proofs,
            )?,
            backtests: normalize_subpath("subpaths.backtests", &self.subpaths.backtests)?,
            artifact_index: normalize_subpath(
                "subpaths.artifact_index",
                &self.subpaths.artifact_index,
            )?,
            research_analytics: normalize_subpath(
                "subpaths.research_analytics",
                &self.subpaths.research_analytics,
            )?,
        };
        ensure_unique_subpaths(&subpaths)?;
        Ok(ResolvedArtifactRoot {
            artifact_root,
            subpaths,
            lifecycle: self.lifecycle.resolve()?,
        })
    }
}

impl ResolvedArtifactRoot {
    #[must_use]
    pub fn lifecycle_policy(&self) -> &ArtifactLifecyclePolicy {
        &self.lifecycle
    }

    #[must_use]
    pub fn typed_root(&self, kind: ArtifactKind) -> String {
        self.join([self.subpath(kind), "v1"])
    }

    #[must_use]
    pub fn nt_catalog_projection_root(&self, catalog_projection_id: &str) -> String {
        self.join([
            self.subpaths.nt_catalog.as_str(),
            "v1",
            &format!("projection={catalog_projection_id}"),
            "",
        ])
    }

    #[must_use]
    pub fn backtest_run_root(&self, fixture: MarketStructureFixture, run_id: &str) -> String {
        self.join([
            self.subpaths.backtests.as_str(),
            "v1",
            &format!("fixture={}", fixture_label(fixture)),
            &format!("run={run_id}"),
            "",
        ])
    }

    #[must_use]
    pub fn latest_pointer(&self, kind: ArtifactKind) -> String {
        self.join([
            self.subpaths.artifact_index.as_str(),
            "v1",
            "pointers",
            &format!("kind={}", kind.index_label()),
            "latest.json",
        ])
    }

    fn index_event_uri(&self, kind: ArtifactKind, event_id: &str) -> String {
        self.join([
            self.subpaths.artifact_index.as_str(),
            "v1",
            "events",
            &format!("kind={}", kind.index_label()),
            &format!("event={event_id}.json"),
        ])
    }

    fn index_snapshot_uri(&self, kind: ArtifactKind, snapshot_id: &str) -> String {
        self.join([
            self.subpaths.artifact_index.as_str(),
            "v1",
            "snapshots",
            &format!("kind={}", kind.index_label()),
            &format!("snapshot={snapshot_id}.json"),
        ])
    }

    fn index_audit_epoch_uri(&self, audit_epoch_id: &str) -> String {
        self.join([
            self.subpaths.artifact_index.as_str(),
            "v1",
            "audit",
            "epochs",
            &format!("{audit_epoch_id}.json"),
        ])
    }

    /// # Errors
    ///
    /// Returns an error if `uri` is not under this artifact root.
    pub fn object_path_for_uri(&self, uri: &str) -> Result<ObjectPath> {
        let uri = uri.trim();
        let expected_prefix = format!("{}/", self.artifact_root);
        ensure!(
            uri.starts_with(&expected_prefix),
            "artifact URI {uri:?} is outside configured artifact_root"
        );
        let without_scheme = uri
            .strip_prefix("s3://")
            .context("artifact URI must be an s3:// URI")?;
        let Some((_bucket, object_path)) = without_scheme.split_once('/') else {
            bail!("artifact URI must include an S3 bucket and object path");
        };
        ensure_path_token("artifact_uri", object_path, PathTokenMode::AllowEquals)?;
        Ok(ObjectPath::from(object_path))
    }

    fn subpath(&self, kind: ArtifactKind) -> &str {
        match kind {
            ArtifactKind::Raw => &self.subpaths.raw,
            ArtifactKind::NtCatalog => &self.subpaths.nt_catalog,
            ArtifactKind::SourceProofs => &self.subpaths.source_proofs,
            ArtifactKind::Backtests => &self.subpaths.backtests,
            ArtifactKind::ArtifactIndex => &self.subpaths.artifact_index,
            ArtifactKind::ResearchAnalytics => &self.subpaths.research_analytics,
        }
    }

    fn join<const N: usize>(&self, parts: [&str; N]) -> String {
        let mut uri = self.artifact_root.clone();
        for part in parts {
            if part.is_empty() {
                uri.push('/');
            } else {
                uri.push('/');
                uri.push_str(part.trim_matches('/'));
            }
        }
        uri
    }
}

impl ArtifactLifecycleConfig {
    fn resolve(&self) -> Result<ArtifactLifecyclePolicy> {
        ensure!(
            self.retention == "forever",
            "artifact lifecycle retention must be forever"
        );
        ensure!(
            self.default_delete_expiration == "disabled",
            "artifact lifecycle delete/expiration must be disabled"
        );
        let mut profiles = BTreeSet::new();
        for profile in &self.storage_profiles {
            ensure!(
                profiles.insert(*profile),
                "artifact lifecycle storage_profiles must be unique"
            );
        }
        for required in [
            ArtifactStorageProfile::Active,
            ArtifactStorageProfile::Archive,
            ArtifactStorageProfile::DeepArchive,
        ] {
            ensure!(
                profiles.contains(&required),
                "artifact lifecycle storage_profiles must include {}",
                required.config_label()
            );
        }
        ensure!(
            self.hot_index.latest_pointer_storage_profile == ArtifactStorageProfile::Active,
            "artifact index latest pointer must remain in active storage"
        );
        ensure!(
            self.hot_index.current_snapshot_storage_profile == ArtifactStorageProfile::Active,
            "artifact index current snapshot must remain in active storage"
        );
        Ok(ArtifactLifecyclePolicy {
            quiet_window_seconds: self.quiet_window_seconds.clone(),
            hot_index: self.hot_index.clone(),
        })
    }
}

impl ArtifactLifecyclePolicy {
    #[must_use]
    pub fn state_after_quiet_window(
        &self,
        kind: ArtifactKind,
        elapsed_seconds: u64,
    ) -> ArtifactLifecycleState {
        if elapsed_seconds >= self.quiet_window_seconds(kind) {
            ArtifactLifecycleState::Inactive
        } else {
            ArtifactLifecycleState::Active
        }
    }

    #[must_use]
    pub fn hot_index_latest_pointer_storage_profile(&self) -> ArtifactStorageProfile {
        self.hot_index.latest_pointer_storage_profile
    }

    #[must_use]
    pub fn hot_index_current_snapshot_storage_profile(&self) -> ArtifactStorageProfile {
        self.hot_index.current_snapshot_storage_profile
    }

    fn quiet_window_seconds(&self, kind: ArtifactKind) -> u64 {
        match kind {
            ArtifactKind::Raw => self.quiet_window_seconds.raw,
            ArtifactKind::NtCatalog => self.quiet_window_seconds.nt_catalog,
            ArtifactKind::SourceProofs => self.quiet_window_seconds.source_proofs,
            ArtifactKind::Backtests => self.quiet_window_seconds.backtests,
            ArtifactKind::ArtifactIndex => self.quiet_window_seconds.artifact_index,
            ArtifactKind::ResearchAnalytics => self.quiet_window_seconds.research_analytics,
        }
    }
}

impl ArtifactStorageProfile {
    fn config_label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archive => "archive",
            Self::DeepArchive => "deep_archive",
        }
    }
}

impl ArtifactKind {
    fn index_label(self) -> &'static str {
        match self {
            ArtifactKind::Raw => "raw",
            ArtifactKind::NtCatalog => "nt-catalog",
            ArtifactKind::SourceProofs => "source-proofs",
            ArtifactKind::Backtests => "backtests",
            ArtifactKind::ArtifactIndex => "artifact-index",
            ArtifactKind::ResearchAnalytics => "research-analytics",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogDispatchConfig {
    pub bindings: Vec<CatalogProjectionBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionBinding {
    pub source_binding: String,
    pub market_structure_fixture: MarketStructureFixture,
    pub catalog_projection_id: String,
}

impl CatalogDispatchConfig {
    /// # Errors
    ///
    /// Returns an error if the source binding is missing, ambiguous, or resolves
    /// to an invalid projection id.
    pub fn catalog_root_for(
        &self,
        source_binding: &str,
        artifact_root: &ResolvedArtifactRoot,
    ) -> Result<String> {
        let mut matches = self
            .bindings
            .iter()
            .filter(|binding| binding.source_binding == source_binding);
        let binding = matches
            .next()
            .with_context(|| format!("no catalog projection binding for {source_binding:?}"))?;
        ensure!(
            matches.next().is_none(),
            "multiple catalog projection bindings for {source_binding:?}"
        );
        ensure_path_token(
            "catalog_projection_id",
            &binding.catalog_projection_id,
            PathTokenMode::AllowEquals,
        )?;
        Ok(artifact_root.nt_catalog_projection_root(&binding.catalog_projection_id))
    }
}

pub struct CreateOnlyArtifactWriter<'a> {
    store: &'a dyn ObjectStore,
}

impl<'a> CreateOnlyArtifactWriter<'a> {
    #[must_use]
    pub fn new(store: &'a dyn ObjectStore) -> Self {
        Self { store }
    }

    /// # Errors
    ///
    /// Returns an error if the object already exists or the object store rejects
    /// create-only semantics.
    pub async fn put_create(&self, path: &ObjectPath, payload: Vec<u8>) -> Result<()> {
        self.store
            .put_opts(path, payload.into(), PutMode::Create.into())
            .await
            .with_context(|| format!("create-only put {path}"))?;
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the object exists with different bytes or the object
    /// store rejects create-only semantics.
    pub async fn put_create_idempotent(
        &self,
        path: &ObjectPath,
        payload: Vec<u8>,
    ) -> Result<UpdateVersion> {
        match self
            .store
            .put_opts(path, payload.clone().into(), PutMode::Create.into())
            .await
        {
            Ok(result) => Ok(result.into()),
            Err(object_store::Error::AlreadyExists { .. }) => {
                let existing = self
                    .store
                    .get(path)
                    .await
                    .with_context(|| format!("read existing create-only object {path}"))?;
                let version = UpdateVersion {
                    e_tag: existing.meta.e_tag.clone(),
                    version: existing.meta.version.clone(),
                };
                let existing_bytes = existing
                    .bytes()
                    .await
                    .with_context(|| format!("read existing create-only bytes {path}"))?;
                ensure!(
                    existing_bytes.as_ref() == payload.as_slice(),
                    "create-only object {path} already exists with different payload"
                );
                Ok(version)
            }
            Err(err) => Err(err).with_context(|| format!("create-only put {path}")),
        }
    }

    /// # Errors
    ///
    /// Returns an error if `uri` is outside `artifact_root`, the object already
    /// exists, or the object store rejects create-only semantics.
    pub async fn put_create_uri(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        uri: &str,
        payload: Vec<u8>,
    ) -> Result<()> {
        let path = artifact_root.object_path_for_uri(uri)?;
        self.put_create(&path, payload).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactIndexCommitState {
    Staged,
    Committed,
    Orphan,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactLineageRef {
    pub artifact_kind: ArtifactKind,
    pub artifact_id: String,
    pub version: Option<String>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexEvent {
    pub schema_version: String,
    pub created_at: String,
    pub event_id: String,
    pub artifact_kind: ArtifactKind,
    pub artifact_id: String,
    pub artifact_uri: String,
    pub manifest_uri: String,
    pub producer_project: String,
    pub owner_project: String,
    pub content_sha256: String,
    pub lifecycle_state: ArtifactLifecycleState,
    pub storage_profile: ArtifactStorageProfile,
    pub parent_lineage: Vec<ArtifactLineageRef>,
    pub commit_state: ArtifactIndexCommitState,
}

impl ArtifactIndexEvent {
    fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        ensure_path_token(
            "schema_version",
            &self.schema_version,
            PathTokenMode::AllowEquals,
        )?;
        ensure_utc_timestamp("created_at", &self.created_at)?;
        ensure_path_token("event_id", &self.event_id, PathTokenMode::AllowEquals)?;
        validate_artifact_id(&self.artifact_id)?;
        validate_artifact_id(&self.producer_project)?;
        validate_artifact_id(&self.owner_project)?;
        artifact_root.object_path_for_uri(&self.artifact_uri)?;
        artifact_root.object_path_for_uri(&self.manifest_uri)?;
        ensure_sha256("content_sha256", &self.content_sha256)?;
        ensure!(
            !self.parent_lineage.is_empty(),
            "artifact index event must declare parent lineage"
        );
        for parent in &self.parent_lineage {
            parent.validate()?;
        }
        Ok(())
    }

    fn event_uri(&self, artifact_root: &ResolvedArtifactRoot) -> Result<String> {
        self.validate(artifact_root)?;
        Ok(artifact_root.index_event_uri(self.artifact_kind, &self.event_id))
    }
}

impl ArtifactLineageRef {
    fn validate(&self) -> Result<()> {
        validate_artifact_id(&self.artifact_id)?;
        if let Some(version) = &self.version {
            validate_artifact_id(version)?;
        }
        ensure_sha256("lineage sha256", &self.sha256)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexSnapshotRow {
    pub schema_version: String,
    pub created_at: String,
    pub artifact_kind: ArtifactKind,
    pub artifact_id: String,
    pub artifact_uri: String,
    pub manifest_uri: String,
    pub producer_project: String,
    pub owner_project: String,
    pub content_sha256: String,
    pub lifecycle_state: ArtifactLifecycleState,
    pub storage_profile: ArtifactStorageProfile,
    pub parent_lineage: Vec<ArtifactLineageRef>,
    pub commit_state: ArtifactIndexCommitState,
}

impl ArtifactIndexSnapshotRow {
    #[must_use]
    pub fn from_event(event: &ArtifactIndexEvent, commit_state: ArtifactIndexCommitState) -> Self {
        Self {
            schema_version: event.schema_version.clone(),
            created_at: event.created_at.clone(),
            artifact_kind: event.artifact_kind,
            artifact_id: event.artifact_id.clone(),
            artifact_uri: event.artifact_uri.clone(),
            manifest_uri: event.manifest_uri.clone(),
            producer_project: event.producer_project.clone(),
            owner_project: event.owner_project.clone(),
            content_sha256: event.content_sha256.clone(),
            lifecycle_state: event.lifecycle_state,
            storage_profile: event.storage_profile,
            parent_lineage: event.parent_lineage.clone(),
            commit_state,
        }
    }

    fn validate(
        &self,
        snapshot_kind: ArtifactKind,
        artifact_root: &ResolvedArtifactRoot,
    ) -> Result<()> {
        ensure!(
            self.artifact_kind == snapshot_kind,
            "snapshot row kind does not match snapshot kind"
        );
        ensure_path_token(
            "schema_version",
            &self.schema_version,
            PathTokenMode::AllowEquals,
        )?;
        ensure_utc_timestamp("created_at", &self.created_at)?;
        validate_artifact_id(&self.artifact_id)?;
        validate_artifact_id(&self.producer_project)?;
        validate_artifact_id(&self.owner_project)?;
        artifact_root.object_path_for_uri(&self.artifact_uri)?;
        artifact_root.object_path_for_uri(&self.manifest_uri)?;
        ensure_sha256("content_sha256", &self.content_sha256)?;
        ensure!(
            !self.parent_lineage.is_empty(),
            "snapshot row must declare parent lineage"
        );
        for parent in &self.parent_lineage {
            parent.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexSnapshot {
    pub snapshot_id: String,
    pub artifact_kind: ArtifactKind,
    pub rows: Vec<ArtifactIndexSnapshotRow>,
}

impl ArtifactIndexSnapshot {
    /// # Errors
    ///
    /// Returns an error if the snapshot id or rows are invalid.
    pub fn new(
        snapshot_id: impl Into<String>,
        artifact_kind: ArtifactKind,
        rows: Vec<ArtifactIndexSnapshotRow>,
    ) -> Result<Self> {
        let snapshot = Self {
            snapshot_id: snapshot_id.into(),
            artifact_kind,
            rows,
        };
        ensure_path_token(
            "snapshot_id",
            &snapshot.snapshot_id,
            PathTokenMode::AllowEquals,
        )?;
        ensure!(!snapshot.rows.is_empty(), "snapshot must contain rows");
        let mut unique = BTreeSet::new();
        for row in &snapshot.rows {
            ensure!(
                row.artifact_kind == artifact_kind,
                "snapshot row kind does not match snapshot kind"
            );
            ensure!(
                row.commit_state == ArtifactIndexCommitState::Committed,
                "snapshot must contain committed rows"
            );
            ensure!(
                unique.insert(row.artifact_id.as_str()),
                "snapshot rows must be unique by artifact_id"
            );
        }
        Ok(snapshot)
    }

    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context("serialize artifact index snapshot")
    }

    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn sha256(&self) -> Result<String> {
        Ok(sha256_bytes(&self.bytes()?))
    }

    fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        ensure_path_token("snapshot_id", &self.snapshot_id, PathTokenMode::AllowEquals)?;
        ensure!(!self.rows.is_empty(), "snapshot must contain rows");
        let mut unique = BTreeSet::new();
        for row in &self.rows {
            row.validate(self.artifact_kind, artifact_root)?;
            ensure!(
                row.commit_state == ArtifactIndexCommitState::Committed,
                "snapshot must contain committed rows"
            );
            ensure!(
                unique.insert(row.artifact_id.as_str()),
                "snapshot rows must be unique by artifact_id"
            );
        }
        Ok(())
    }

    fn snapshot_uri(&self, artifact_root: &ResolvedArtifactRoot) -> Result<String> {
        self.validate(artifact_root)?;
        Ok(artifact_root.index_snapshot_uri(self.artifact_kind, &self.snapshot_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexPointer {
    pub artifact_kind: ArtifactKind,
    pub snapshot_id: String,
    pub snapshot_uri: String,
    pub snapshot_sha256: String,
}

impl ArtifactIndexPointer {
    /// # Errors
    ///
    /// Returns an error if the snapshot cannot be serialized or addressed under
    /// the configured artifact root.
    pub fn from_snapshot(
        artifact_root: &ResolvedArtifactRoot,
        snapshot: &ArtifactIndexSnapshot,
    ) -> Result<Self> {
        Ok(Self {
            artifact_kind: snapshot.artifact_kind,
            snapshot_id: snapshot.snapshot_id.clone(),
            snapshot_uri: snapshot.snapshot_uri(artifact_root)?,
            snapshot_sha256: snapshot.sha256()?,
        })
    }

    fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        ensure_path_token("snapshot_id", &self.snapshot_id, PathTokenMode::AllowEquals)?;
        artifact_root.object_path_for_uri(&self.snapshot_uri)?;
        ensure_sha256("snapshot_sha256", &self.snapshot_sha256)?;
        Ok(())
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context("serialize artifact index pointer")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredArtifactIndexPointer {
    pub pointer: ArtifactIndexPointer,
    pub version: UpdateVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIndexAuditEpoch {
    pub audit_epoch_id: String,
    pub artifact_kind: ArtifactKind,
    pub prior_snapshot_id: Option<String>,
    pub new_snapshot_id: String,
    pub writer_id: String,
    pub prior_pointer_e_tag: Option<String>,
    pub new_pointer_e_tag: Option<String>,
}

impl ArtifactIndexAuditEpoch {
    fn audit_uri(&self, artifact_root: &ResolvedArtifactRoot) -> Result<String> {
        self.validate()?;
        Ok(artifact_root.index_audit_epoch_uri(&self.audit_epoch_id))
    }

    fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).context("serialize artifact index audit epoch")
    }

    fn validate(&self) -> Result<()> {
        ensure_path_token(
            "audit_epoch_id",
            &self.audit_epoch_id,
            PathTokenMode::AllowEquals,
        )?;
        if let Some(prior_snapshot_id) = &self.prior_snapshot_id {
            ensure_path_token(
                "prior_snapshot_id",
                prior_snapshot_id,
                PathTokenMode::AllowEquals,
            )?;
        }
        ensure_path_token(
            "new_snapshot_id",
            &self.new_snapshot_id,
            PathTokenMode::AllowEquals,
        )?;
        validate_artifact_id(&self.writer_id)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIndexCommitPlan {
    pub event: ArtifactIndexEvent,
    pub snapshot_ids: Vec<String>,
    pub audit_epoch_ids: Vec<String>,
    pub writer_id: String,
}

impl ArtifactIndexCommitPlan {
    fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        self.event.validate(artifact_root)?;
        ensure!(
            !self.snapshot_ids.is_empty(),
            "commit plan must include snapshot ids"
        );
        ensure!(
            !self.audit_epoch_ids.is_empty(),
            "commit plan must include audit epoch ids"
        );
        validate_artifact_id(&self.writer_id)?;
        let mut snapshot_ids = BTreeSet::new();
        for snapshot_id in &self.snapshot_ids {
            ensure_path_token("snapshot_id", snapshot_id, PathTokenMode::AllowEquals)?;
            ensure!(
                snapshot_ids.insert(snapshot_id.as_str()),
                "commit plan snapshot ids must be unique"
            );
        }
        let mut audit_epoch_ids = BTreeSet::new();
        for audit_epoch_id in &self.audit_epoch_ids {
            ensure_path_token("audit_epoch_id", audit_epoch_id, PathTokenMode::AllowEquals)?;
            ensure!(
                audit_epoch_ids.insert(audit_epoch_id.as_str()),
                "commit plan audit epoch ids must be unique"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIndexCommitOutcome {
    pub snapshot_id: String,
    pub pointer_attempts: usize,
    pub prior_snapshot_id: Option<String>,
    pub audit_epoch_uri: String,
    pub audit_epoch: ArtifactIndexAuditEpoch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactIndexWriteAuthority {
    writer_id: String,
    owned_kinds: BTreeSet<ArtifactKind>,
}

impl ArtifactIndexWriteAuthority {
    /// # Errors
    ///
    /// Returns an error if the writer id is invalid or no artifact kinds are
    /// owned by the writer.
    pub fn new(
        writer_id: impl Into<String>,
        owned_kinds: impl IntoIterator<Item = ArtifactKind>,
    ) -> Result<Self> {
        let writer_id = writer_id.into();
        validate_artifact_id(&writer_id)?;
        let owned_kinds = owned_kinds.into_iter().collect::<BTreeSet<_>>();
        ensure!(
            !owned_kinds.is_empty(),
            "artifact index writer authority must own at least one kind"
        );
        Ok(Self {
            writer_id,
            owned_kinds,
        })
    }

    fn authorize_kind(&self, kind: ArtifactKind) -> Result<()> {
        ensure!(
            self.owned_kinds.contains(&kind),
            "artifact index writer {:?} is not authorized to write {kind:?}",
            self.writer_id
        );
        Ok(())
    }

    fn authorize_commit(&self, writer_id: &str, kind: ArtifactKind) -> Result<()> {
        self.authorize_kind(kind)?;
        ensure!(
            writer_id == self.writer_id,
            "commit writer_id {:?} does not match configured artifact index writer {:?}",
            writer_id,
            self.writer_id
        );
        Ok(())
    }
}

pub struct ArtifactIndexWriter<'a> {
    store: &'a dyn ObjectStore,
    create_only: CreateOnlyArtifactWriter<'a>,
    authority: Option<ArtifactIndexWriteAuthority>,
}

impl<'a> ArtifactIndexWriter<'a> {
    #[must_use]
    pub fn new(store: &'a dyn ObjectStore) -> Self {
        Self {
            store,
            create_only: CreateOnlyArtifactWriter::new(store),
            authority: None,
        }
    }

    #[must_use]
    pub fn with_authority(
        store: &'a dyn ObjectStore,
        authority: ArtifactIndexWriteAuthority,
    ) -> Self {
        Self {
            store,
            create_only: CreateOnlyArtifactWriter::new(store),
            authority: Some(authority),
        }
    }

    /// # Errors
    ///
    /// Returns an error if the event is invalid, conflicts with an existing
    /// different event payload, or the object store rejects the create.
    pub async fn put_event(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        event: &ArtifactIndexEvent,
    ) -> Result<UpdateVersion> {
        self.authorize_kind(event.artifact_kind)?;
        let uri = event.event_uri(artifact_root)?;
        let path = artifact_root.object_path_for_uri(&uri)?;
        let payload = serde_json::to_vec(event).context("serialize artifact index event")?;
        self.create_only.put_create_idempotent(&path, payload).await
    }

    /// # Errors
    ///
    /// Returns an error if the snapshot is invalid, conflicts with an existing
    /// different snapshot payload, or the object store rejects the create.
    pub async fn put_snapshot(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        snapshot: &ArtifactIndexSnapshot,
    ) -> Result<UpdateVersion> {
        self.authorize_kind(snapshot.artifact_kind)?;
        let uri = snapshot.snapshot_uri(artifact_root)?;
        let path = artifact_root.object_path_for_uri(&uri)?;
        self.create_only
            .put_create_idempotent(&path, snapshot.bytes()?)
            .await
    }

    /// # Errors
    ///
    /// Returns an error if the commit cannot write immutable records, cannot
    /// advance the generated latest pointer, or exhausts supplied snapshot ids.
    pub async fn commit_event(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        plan: ArtifactIndexCommitPlan,
    ) -> Result<ArtifactIndexCommitOutcome> {
        let observed = self
            .read_latest_pointer(artifact_root, plan.event.artifact_kind)
            .await?;
        self.commit_event_from_observed_latest(artifact_root, plan, observed)
            .await
    }

    /// # Errors
    ///
    /// Returns an error if the supplied observed latest is stale and the commit
    /// cannot rebase within the supplied snapshot ids.
    pub async fn commit_event_from_observed_latest(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        plan: ArtifactIndexCommitPlan,
        observed: Option<StoredArtifactIndexPointer>,
    ) -> Result<ArtifactIndexCommitOutcome> {
        plan.validate(artifact_root)?;
        self.authorize_commit(&plan.writer_id, plan.event.artifact_kind)?;
        self.put_event(artifact_root, &plan.event).await?;
        let mut observed = observed;
        for (attempt, snapshot_id) in plan.snapshot_ids.iter().enumerate() {
            let prior_snapshot = match &observed {
                Some(latest) => Some(
                    self.read_snapshot_for_pointer(artifact_root, &latest.pointer)
                        .await?,
                ),
                None => None,
            };
            let snapshot = build_rebased_snapshot(snapshot_id, plan.event.clone(), prior_snapshot)?;
            self.put_snapshot(artifact_root, &snapshot).await?;
            let pointer = ArtifactIndexPointer::from_snapshot(artifact_root, &snapshot)?;
            let prior_snapshot_id = observed
                .as_ref()
                .map(|latest| latest.pointer.snapshot_id.clone());
            let prior_pointer_e_tag = observed
                .as_ref()
                .and_then(|latest| latest.version.e_tag.clone());
            let pointer_result = match &observed {
                Some(latest) => {
                    self.update_latest_pointer(artifact_root, &pointer, latest.version.clone())
                        .await
                }
                None => self.create_latest_pointer(artifact_root, &pointer).await,
            };
            match pointer_result {
                Ok(new_pointer_version) => {
                    let audit_epoch_id = plan
                        .audit_epoch_ids
                        .get(attempt)
                        .or_else(|| plan.audit_epoch_ids.last())
                        .context("commit plan must include audit epoch ids")?
                        .clone();
                    let audit_epoch = ArtifactIndexAuditEpoch {
                        audit_epoch_id,
                        artifact_kind: snapshot.artifact_kind,
                        prior_snapshot_id,
                        new_snapshot_id: snapshot.snapshot_id.clone(),
                        writer_id: plan.writer_id.clone(),
                        prior_pointer_e_tag,
                        new_pointer_e_tag: new_pointer_version.e_tag,
                    };
                    let audit_epoch_uri =
                        self.append_audit_epoch(artifact_root, &audit_epoch).await?;
                    let outcome_prior_snapshot_id = audit_epoch.prior_snapshot_id.clone();
                    return Ok(ArtifactIndexCommitOutcome {
                        snapshot_id: snapshot.snapshot_id,
                        pointer_attempts: attempt + 1,
                        prior_snapshot_id: outcome_prior_snapshot_id,
                        audit_epoch_uri,
                        audit_epoch,
                    });
                }
                Err(err) if is_pointer_commit_conflict(&err) => {
                    observed = self
                        .read_latest_pointer(artifact_root, plan.event.artifact_kind)
                        .await?;
                    ensure!(
                        observed.is_some(),
                        "artifact index pointer conflict left no latest pointer"
                    );
                }
                Err(err) => return Err(err),
            }
        }
        bail!("artifact index commit exhausted supplied snapshot ids")
    }

    /// # Errors
    ///
    /// Returns an error if the audit epoch is invalid, already exists with
    /// different bytes, or the object store rejects create-only semantics.
    pub async fn append_audit_epoch(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        audit_epoch: &ArtifactIndexAuditEpoch,
    ) -> Result<String> {
        self.authorize_kind(audit_epoch.artifact_kind)?;
        let uri = audit_epoch.audit_uri(artifact_root)?;
        let path = artifact_root.object_path_for_uri(&uri)?;
        self.create_only
            .put_create_idempotent(&path, audit_epoch.bytes()?)
            .await?;
        Ok(uri)
    }

    /// # Errors
    ///
    /// Returns an error if the pointer is invalid, already exists, or the object
    /// store rejects create-only semantics.
    pub async fn create_latest_pointer(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        pointer: &ArtifactIndexPointer,
    ) -> Result<UpdateVersion> {
        self.authorize_kind(pointer.artifact_kind)?;
        pointer.validate(artifact_root)?;
        let path = latest_pointer_path(artifact_root, pointer.artifact_kind)?;
        let result = self
            .store
            .put_opts(&path, pointer.bytes()?.into(), PutMode::Create.into())
            .await
            .with_context(|| format!("create latest artifact index pointer {path}"))?;
        Ok(result.into())
    }

    /// # Errors
    ///
    /// Returns an error if the pointer is invalid, the expected object version
    /// does not match, or the object store rejects conditional updates.
    pub async fn update_latest_pointer(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        pointer: &ArtifactIndexPointer,
        expected: UpdateVersion,
    ) -> Result<UpdateVersion> {
        self.authorize_kind(pointer.artifact_kind)?;
        pointer.validate(artifact_root)?;
        let path = latest_pointer_path(artifact_root, pointer.artifact_kind)?;
        let result = self
            .store
            .put_opts(
                &path,
                pointer.bytes()?.into(),
                PutMode::Update(expected).into(),
            )
            .await
            .with_context(|| {
                format!("conditional precondition update artifact index pointer {path}")
            })?;
        Ok(result.into())
    }

    /// # Errors
    ///
    /// Returns an error if the pointer object cannot be read or decoded.
    pub async fn read_latest_pointer(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        kind: ArtifactKind,
    ) -> Result<Option<StoredArtifactIndexPointer>> {
        let path = latest_pointer_path(artifact_root, kind)?;
        let object = match self.store.get(&path).await {
            Ok(object) => object,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("read artifact index pointer {path}"));
            }
        };
        let version = UpdateVersion {
            e_tag: object.meta.e_tag.clone(),
            version: object.meta.version.clone(),
        };
        let bytes = object
            .bytes()
            .await
            .with_context(|| format!("read artifact index pointer bytes {path}"))?;
        let pointer: ArtifactIndexPointer =
            serde_json::from_slice(bytes.as_ref()).context("decode artifact index pointer")?;
        ensure!(
            pointer.artifact_kind == kind,
            "artifact index pointer kind does not match requested kind"
        );
        pointer.validate(artifact_root)?;
        Ok(Some(StoredArtifactIndexPointer { pointer, version }))
    }

    /// # Errors
    ///
    /// Returns an error if the event object exists but cannot be read, decoded,
    /// or validated against the requested kind and event id.
    pub async fn read_event(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        kind: ArtifactKind,
        event_id: &str,
    ) -> Result<Option<ArtifactIndexEvent>> {
        ensure_path_token("event_id", event_id, PathTokenMode::AllowEquals)?;
        let uri = artifact_root.index_event_uri(kind, event_id);
        let path = artifact_root.object_path_for_uri(&uri)?;
        let object = match self.store.get(&path).await {
            Ok(object) => object,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("read artifact index event {path}"));
            }
        };
        let bytes = object
            .bytes()
            .await
            .with_context(|| format!("read artifact index event bytes {path}"))?;
        let event: ArtifactIndexEvent =
            serde_json::from_slice(bytes.as_ref()).context("decode artifact index event")?;
        ensure!(
            event.artifact_kind == kind,
            "artifact index event kind does not match requested kind"
        );
        ensure!(
            event.event_id == event_id,
            "artifact index event id does not match requested id"
        );
        event.validate(artifact_root)?;
        Ok(Some(event))
    }

    /// # Errors
    ///
    /// Returns an error if the latest snapshot cannot be read or verified.
    pub async fn read_committed_row(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        kind: ArtifactKind,
        artifact_id: &str,
    ) -> Result<Option<ArtifactIndexSnapshotRow>> {
        validate_artifact_id(artifact_id)?;
        let Some(latest) = self.read_latest_pointer(artifact_root, kind).await? else {
            return Ok(None);
        };
        let snapshot = self
            .read_snapshot_for_pointer(artifact_root, &latest.pointer)
            .await?;
        Ok(snapshot
            .rows
            .into_iter()
            .find(|row| row.artifact_id == artifact_id))
    }

    /// # Errors
    ///
    /// Returns an error if the child row exists but does not declare the parent,
    /// or if the declared parent is missing or hash-mismatched.
    pub async fn read_declared_parent_row(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        child_kind: ArtifactKind,
        child_artifact_id: &str,
        parent_kind: ArtifactKind,
        parent_artifact_id: &str,
    ) -> Result<Option<ArtifactIndexSnapshotRow>> {
        let Some(child) = self
            .read_committed_row(artifact_root, child_kind, child_artifact_id)
            .await?
        else {
            return Ok(None);
        };
        let declared_parent = child
            .parent_lineage
            .iter()
            .find(|parent| {
                parent.artifact_kind == parent_kind && parent.artifact_id == parent_artifact_id
            })
            .with_context(|| {
                format!(
                    "artifact {child_artifact_id:?} does not declare lineage to {parent_kind:?}/{parent_artifact_id:?}"
                )
            })?;
        let parent = self
            .read_committed_row(artifact_root, parent_kind, parent_artifact_id)
            .await?
            .with_context(|| {
                format!("declared parent {parent_kind:?}/{parent_artifact_id:?} is not committed")
            })?;
        ensure!(
            parent.content_sha256 == declared_parent.sha256,
            "declared parent content hash mismatch for {parent_kind:?}/{parent_artifact_id:?}"
        );
        Ok(Some(parent))
    }

    /// # Errors
    ///
    /// Returns an error if latest is missing, hash-invalid, or points outside
    /// the configured artifact root.
    pub async fn read_verified_latest_snapshot(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        kind: ArtifactKind,
    ) -> Result<ArtifactIndexSnapshot> {
        let latest = self
            .read_latest_pointer(artifact_root, kind)
            .await?
            .with_context(|| format!("missing latest artifact index pointer for {kind:?}"))?;
        self.read_snapshot_for_pointer(artifact_root, &latest.pointer)
            .await
    }

    async fn read_snapshot_for_pointer(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        pointer: &ArtifactIndexPointer,
    ) -> Result<ArtifactIndexSnapshot> {
        pointer.validate(artifact_root)?;
        let path = artifact_root.object_path_for_uri(&pointer.snapshot_uri)?;
        let object = self
            .store
            .get(&path)
            .await
            .with_context(|| format!("read artifact index snapshot {path}"))?;
        let bytes = object
            .bytes()
            .await
            .with_context(|| format!("read artifact index snapshot bytes {path}"))?;
        let actual_hash = sha256_bytes(bytes.as_ref());
        ensure!(
            actual_hash == pointer.snapshot_sha256,
            "snapshot hash mismatch for latest artifact index pointer"
        );
        let snapshot: ArtifactIndexSnapshot =
            serde_json::from_slice(bytes.as_ref()).context("decode artifact index snapshot")?;
        ensure!(
            snapshot.artifact_kind == pointer.artifact_kind,
            "snapshot kind does not match pointer kind"
        );
        ensure!(
            snapshot.snapshot_id == pointer.snapshot_id,
            "snapshot id does not match pointer"
        );
        snapshot.validate(artifact_root)?;
        Ok(snapshot)
    }

    fn authorize_kind(&self, kind: ArtifactKind) -> Result<()> {
        if let Some(authority) = &self.authority {
            authority.authorize_kind(kind)?;
        }
        Ok(())
    }

    fn authorize_commit(&self, writer_id: &str, kind: ArtifactKind) -> Result<()> {
        if let Some(authority) = &self.authority {
            authority.authorize_commit(writer_id, kind)?;
        }
        Ok(())
    }
}

fn latest_pointer_path(
    artifact_root: &ResolvedArtifactRoot,
    kind: ArtifactKind,
) -> Result<ObjectPath> {
    artifact_root.object_path_for_uri(&artifact_root.latest_pointer(kind))
}

fn build_rebased_snapshot(
    snapshot_id: &str,
    event: ArtifactIndexEvent,
    prior_snapshot: Option<ArtifactIndexSnapshot>,
) -> Result<ArtifactIndexSnapshot> {
    let mut rows = prior_snapshot.map_or_else(Vec::new, |snapshot| snapshot.rows);
    let new_row = ArtifactIndexSnapshotRow::from_event(&event, ArtifactIndexCommitState::Committed);
    match rows
        .iter()
        .position(|row| row.artifact_id == new_row.artifact_id)
    {
        Some(index) if rows[index] == new_row => {}
        Some(_) => bail!(
            "artifact index snapshot already contains artifact_id {:?} with different content",
            new_row.artifact_id
        ),
        None => rows.push(new_row),
    }
    ArtifactIndexSnapshot::new(snapshot_id, event.artifact_kind, rows)
}

fn is_pointer_commit_conflict(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<object_store::Error>()
            .is_some_and(|object_error| {
                matches!(
                    object_error,
                    object_store::Error::AlreadyExists { .. }
                        | object_store::Error::Precondition { .. }
                )
            })
    })
}

fn normalize_artifact_root(value: &str) -> Result<String> {
    let root = value.trim().trim_end_matches('/');
    ensure!(
        root.starts_with("s3://"),
        "canonical artifact_root must be an s3:// URI"
    );
    let Some((bucket, prefix)) = root
        .strip_prefix("s3://")
        .and_then(|without_scheme| without_scheme.split_once('/'))
    else {
        bail!("artifact_root must include an S3 bucket and prefix");
    };
    ensure!(
        !bucket.trim().is_empty(),
        "artifact_root S3 bucket is empty"
    );
    ensure!(
        !prefix.trim_matches('/').is_empty(),
        "artifact_root S3 prefix is empty"
    );
    Ok(root.to_string())
}

fn normalize_subpath(field: &'static str, value: &str) -> Result<String> {
    let token = value.trim().trim_matches('/');
    ensure_path_token(field, token, PathTokenMode::NoEquals)?;
    Ok(token.to_string())
}

fn ensure_unique_subpaths(subpaths: &ArtifactSubpaths) -> Result<()> {
    let values = [
        subpaths.raw.as_str(),
        subpaths.nt_catalog.as_str(),
        subpaths.source_proofs.as_str(),
        subpaths.backtests.as_str(),
        subpaths.artifact_index.as_str(),
        subpaths.research_analytics.as_str(),
    ];
    let unique = values.iter().copied().collect::<BTreeSet<_>>();
    ensure!(
        unique.len() == values.len(),
        "artifact subpaths must be unique"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathTokenMode {
    NoEquals,
    AllowEquals,
}

fn ensure_path_token(field: &'static str, token: &str, mode: PathTokenMode) -> Result<()> {
    ensure!(!token.is_empty(), "{field} must not be empty");
    ensure!(
        !token.contains("://"),
        "{field} must be a relative artifact path token"
    );
    ensure!(
        !token.split('/').any(|part| matches!(part, "." | ".." | "")),
        "{field} must not contain empty, current, or parent path segments"
    );
    if mode == PathTokenMode::NoEquals {
        ensure!(!token.contains('='), "{field} must not contain '='");
    }
    Ok(())
}

fn validate_artifact_id(value: &str) -> Result<()> {
    ensure_path_token("artifact_id", value, PathTokenMode::AllowEquals)
}

fn ensure_utc_timestamp(field: &'static str, value: &str) -> Result<()> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("{field} must be an RFC3339 timestamp"))?;
    ensure!(
        timestamp.offset().local_minus_utc() == 0,
        "{field} must be UTC"
    );
    Ok(())
}

fn ensure_sha256(field: &'static str, value: &str) -> Result<()> {
    ensure!(value.len() == 64, "{field} must be a sha256 hex digest");
    ensure!(
        value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{field} must be a lowercase sha256 hex digest"
    );
    Ok(())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fixture_label(fixture: MarketStructureFixture) -> &'static str {
    match fixture {
        MarketStructureFixture::BinaryOption => "binary-option",
        MarketStructureFixture::PerpsSpot => "perps-spot",
    }
}
