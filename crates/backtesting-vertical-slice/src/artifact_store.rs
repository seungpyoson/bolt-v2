use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::{
    os::fd::{AsRawFd, FromRawFd},
    os::unix::ffi::OsStrExt,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
};

use ahash::AHashMap;
use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use futures_util::StreamExt;
use object_store::aws::{AmazonS3, AmazonS3Builder, S3ConditionalPut, S3CopyIfNotExists};
use object_store::{
    GetOptions, ObjectStore, ObjectStoreExt, PutMode, UpdateVersion, path::Path as ObjectPath,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{
    operator_work_budget::{
        ExactSizedObjectBuffer, OperatorWorkBudgetCommitPermit, OperatorWorkBudgetGuard,
        OperatorWorkBudgetStage, deserialize_json_with_budget, guarded_async_operation_outcome,
        guarded_operation_outcome, read_exact_sized_hashed_pinned_file_guarded,
        serialize_json_to_vec_guarded, sha256_hex_with_budget, sha256_json_guarded,
    },
    run_manifest::{
        CATALOG_RUN_VIEW_AUTHORITY_FILE, CatalogProjectionManifestDocument, MarketStructureFixture,
    },
    runner::seal_trusted_local_catalog_permissions_guarded,
};

#[cfg(test)]
use crate::atomic_artifact_write::validate_pinned_regular_file_identity;

pub const CATALOG_PROJECTION_PUBLICATION_RECEIPT_SCHEMA_VERSION: &str =
    "catalog-projection-publication-receipt-v2";
/// Amazon S3's documented 5 GB ceiling for one `PutObject` request, expressed
/// in binary bytes.
pub const S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES: u64 = 5 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStoreConfig {
    pub artifact_root: String,
    /// Protocol and peak retained-payload-memory ceiling. The pinned
    /// `object_store` single-PUT path materializes one complete object, so this
    /// is intentionally both an S3 size cap and a per-object memory cap.
    pub max_final_object_bytes: u64,
    pub s3: S3ArtifactStoreConfig,
    pub create_only_probe: CreateOnlyProbeConfig,
    pub catalog_projection_manifest_object: String,
    pub subpaths: ArtifactSubpaths,
    pub lifecycle: ArtifactLifecycleConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3ArtifactStoreConfig {
    pub region: String,
    pub conditional_put: S3ConditionalPutMode,
    pub copy_if_not_exists: S3CopyIfNotExistsMode,
    /// Hard tail bound for one terminal create plus exact-version
    /// reconciliation. Expiry is reported as an indeterminate commit, never as
    /// proof that no object was created.
    pub terminal_commit_timeout_seconds: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct S3ArtifactStoreCredentials {
    access_key_id: String,
    secret_access_key: String,
    session_token: Option<String>,
}

/// Opaque proof that the configured S3 bucket reported versioning `Enabled`
/// before the durable publisher was entered.
///
/// The fields are deliberately private: production code can obtain this value
/// only from [`ResolvedArtifactRoot::verify_bucket_versioning_enabled`]. Each
/// individual create still has to return a non-`null` version ID, which closes
/// the race where versioning is suspended after this read-only preflight.
#[derive(Debug, PartialEq, Eq)]
pub struct BucketVersioningEnabled {
    bucket: String,
    region: String,
}

fn ensure_bucket_versioning_status_enabled(
    status: Option<&aws_sdk_s3::types::BucketVersioningStatus>,
) -> Result<()> {
    ensure!(
        status == Some(&aws_sdk_s3::types::BucketVersioningStatus::Enabled),
        "artifact bucket versioning must be Enabled before durable publication; reported {status:?}"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum S3ConditionalPutMode {
    #[serde(rename = "etag")]
    Etag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum S3CopyIfNotExistsMode {
    Multipart,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOnlyProbeConfig {
    pub prefix: String,
    pub object_name: String,
    pub copy_source_object_name: String,
    pub copy_dest_object_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSubpaths {
    pub raw: String,
    pub nt_catalog: String,
    pub nt_catalog_synthetic_proof: String,
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

const RESEARCH_ANALYTICS_DATASETS_SUBFAMILY: &str = "datasets";
const RESEARCH_ANALYTICS_FEATURE_TABLES_SUBFAMILY: &str = "feature-tables";
const RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY: &str = "experiment-results";
const RESEARCH_ANALYTICS_ARTIFACT_FAMILIES: &[&str] = &[
    RESEARCH_ANALYTICS_DATASETS_SUBFAMILY,
    RESEARCH_ANALYTICS_FEATURE_TABLES_SUBFAMILY,
    RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArtifactRoot {
    artifact_root: String,
    max_final_object_bytes: u64,
    s3: S3ArtifactStoreConfig,
    create_only_probe: CreateOnlyProbeConfig,
    catalog_projection_manifest_object: String,
    subpaths: ArtifactSubpaths,
    lifecycle: ArtifactLifecyclePolicy,
}

fn enforce_final_object_byte_cap(
    object_label: &str,
    object_bytes: u64,
    max_final_object_bytes: u64,
) -> Result<()> {
    ensure!(
        max_final_object_bytes > 0,
        "artifact_store.max_final_object_bytes must be positive"
    );
    ensure!(
        max_final_object_bytes < S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES,
        "artifact_store.max_final_object_bytes {max_final_object_bytes} must be strictly below \
         S3 single-PUT protocol ceiling {S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES} bytes; multipart \
         publication is prohibited"
    );
    ensure!(
        object_bytes <= max_final_object_bytes,
        "{object_label} is {object_bytes} bytes, exceeding artifact_store.max_final_object_bytes \
         {max_final_object_bytes}; multipart publication is prohibited"
    );
    Ok(())
}

impl ArtifactStoreConfig {
    /// # Errors
    ///
    /// Returns an error when the final-object byte cap is invalid or the
    /// configured canonical root or subpaths are not valid artifact-store
    /// paths.
    pub fn resolve(&self) -> Result<ResolvedArtifactRoot> {
        let artifact_root = normalize_artifact_root(&self.artifact_root)?;
        enforce_final_object_byte_cap(
            "artifact-store configuration",
            0,
            self.max_final_object_bytes,
        )?;
        let s3 = self.s3.resolve()?;
        let create_only_probe = CreateOnlyProbeConfig {
            prefix: normalize_subpath("create_only_probe.prefix", &self.create_only_probe.prefix)?,
            object_name: normalize_subpath(
                "create_only_probe.object_name",
                &self.create_only_probe.object_name,
            )?,
            copy_source_object_name: normalize_subpath(
                "create_only_probe.copy_source_object_name",
                &self.create_only_probe.copy_source_object_name,
            )?,
            copy_dest_object_name: normalize_subpath(
                "create_only_probe.copy_dest_object_name",
                &self.create_only_probe.copy_dest_object_name,
            )?,
        };
        let catalog_projection_manifest_object = normalize_subpath(
            "catalog_projection_manifest_object",
            &self.catalog_projection_manifest_object,
        )?;
        let subpaths = ArtifactSubpaths {
            raw: normalize_subpath("subpaths.raw", &self.subpaths.raw)?,
            nt_catalog: normalize_subpath("subpaths.nt_catalog", &self.subpaths.nt_catalog)?,
            nt_catalog_synthetic_proof: normalize_subpath(
                "subpaths.nt_catalog_synthetic_proof",
                &self.subpaths.nt_catalog_synthetic_proof,
            )?,
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
        ensure_probe_prefix_is_private(&create_only_probe, &subpaths)?;
        Ok(ResolvedArtifactRoot {
            artifact_root,
            max_final_object_bytes: self.max_final_object_bytes,
            s3,
            create_only_probe,
            catalog_projection_manifest_object,
            subpaths,
            lifecycle: self.lifecycle.resolve()?,
        })
    }

    /// # Errors
    ///
    /// Returns an error when the artifact root or S3 capability config is invalid.
    pub fn build_s3_object_store_with_credentials(
        &self,
        credentials: &S3ArtifactStoreCredentials,
    ) -> Result<AmazonS3> {
        let resolved = self.resolve()?;
        resolved.build_s3_object_store_with_credentials(credentials)
    }

    /// # Errors
    ///
    /// Returns an error when the artifact-store S3 config is invalid.
    pub fn nt_catalog_storage_options(&self) -> Result<AHashMap<String, String>> {
        Ok(self.resolve()?.nt_catalog_storage_options())
    }

    /// # Errors
    ///
    /// Returns an error when the artifact-store config is invalid or the
    /// resolved S3 credentials are empty.
    pub fn nt_catalog_storage_options_with_credentials(
        &self,
        credentials: &S3ArtifactStoreCredentials,
    ) -> Result<AHashMap<String, String>> {
        Ok(self
            .resolve()?
            .nt_catalog_storage_options_with_credentials(credentials))
    }
}

impl S3ArtifactStoreConfig {
    fn resolve(&self) -> Result<Self> {
        let region = self.region.trim();
        ensure_path_token("s3.region", region, PathTokenMode::NoEquals)?;
        ensure!(
            region == self.region,
            "s3.region must not contain leading or trailing whitespace"
        );
        ensure!(
            self.terminal_commit_timeout_seconds > 0,
            "s3.terminal_commit_timeout_seconds must be positive"
        );
        Ok(Self {
            region: region.to_string(),
            conditional_put: self.conditional_put,
            copy_if_not_exists: self.copy_if_not_exists,
            terminal_commit_timeout_seconds: self.terminal_commit_timeout_seconds,
        })
    }
}

impl S3ArtifactStoreCredentials {
    /// # Errors
    ///
    /// Returns an error if any resolved credential value is empty or includes
    /// surrounding whitespace.
    pub fn new(
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    ) -> Result<Self> {
        Ok(Self {
            access_key_id: ensure_resolved_credential_value(
                "artifact_store.s3.access_key_id",
                access_key_id,
            )?,
            secret_access_key: ensure_resolved_credential_value(
                "artifact_store.s3.secret_access_key",
                secret_access_key,
            )?,
            session_token: session_token
                .map(|value| {
                    ensure_resolved_credential_value("artifact_store.s3.session_token", value)
                })
                .transpose()?,
        })
    }

    #[must_use]
    pub fn access_key_id(&self) -> &str {
        &self.access_key_id
    }

    #[must_use]
    pub fn secret_access_key(&self) -> &str {
        &self.secret_access_key
    }

    #[must_use]
    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref()
    }
}

impl ResolvedArtifactRoot {
    #[must_use]
    pub fn artifact_root_uri(&self) -> &str {
        &self.artifact_root
    }

    #[must_use]
    pub const fn max_final_object_bytes(&self) -> u64 {
        self.max_final_object_bytes
    }

    #[must_use]
    pub fn s3_region(&self) -> &str {
        &self.s3.region
    }

    #[must_use]
    pub fn lifecycle_policy(&self) -> &ArtifactLifecyclePolicy {
        &self.lifecycle
    }

    /// # Errors
    ///
    /// Returns an error when the configured S3 object store cannot be built.
    pub fn build_s3_object_store_with_credentials(
        &self,
        credentials: &S3ArtifactStoreCredentials,
    ) -> Result<AmazonS3> {
        let bucket_name = artifact_bucket_name(&self.artifact_root)?;
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(bucket_name)
            .with_region(self.s3.region.as_str())
            .with_access_key_id(credentials.access_key_id())
            .with_secret_access_key(credentials.secret_access_key())
            .with_conditional_put(match self.s3.conditional_put {
                S3ConditionalPutMode::Etag => S3ConditionalPut::ETagMatch,
            })
            .with_copy_if_not_exists(match self.s3.copy_if_not_exists {
                S3CopyIfNotExistsMode::Multipart => S3CopyIfNotExists::Multipart,
            });
        if let Some(token) = credentials.session_token() {
            builder = builder.with_token(token);
        }
        builder
            .build()
            .context("build artifact_root S3 object store")
    }

    /// Perform the read-only S3 control-plane check which gates every durable
    /// publication run.
    ///
    /// # Errors
    ///
    /// Returns an error when `GetBucketVersioning` fails or the bucket reports
    /// any state other than `Enabled` (including an absent status).
    pub async fn verify_bucket_versioning_enabled(
        &self,
        credentials: &S3ArtifactStoreCredentials,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<BucketVersioningEnabled> {
        work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        let bucket = artifact_bucket_name(&self.artifact_root)?.to_string();
        let sdk_credentials = aws_sdk_s3::config::Credentials::new(
            credentials.access_key_id().to_string(),
            credentials.secret_access_key().to_string(),
            credentials.session_token().map(str::to_string),
            None,
            "bolt-v2-ssm",
        );
        let sdk_config = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(self.s3.region.clone()))
            .credentials_provider(sdk_credentials)
            .build();
        let client = aws_sdk_s3::Client::from_conf(sdk_config);
        let response = guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            client.get_bucket_versioning().bucket(&bucket).send(),
        )
        .await?
        .map_err(|error| {
            anyhow::anyhow!(
                "AWS S3 GetBucketVersioning failed for configured artifact bucket: {}",
                aws_sdk_s3::error::DisplayErrorContext(&error)
            )
        })?;
        ensure_bucket_versioning_status_enabled(response.status())?;
        work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
        Ok(BucketVersioningEnabled {
            bucket,
            region: self.s3.region.clone(),
        })
    }

    /// Construct the same opaque preflight proof for debug-only object-store
    /// contract tests. This symbol is absent from release binaries; production
    /// callers must use the AWS control-plane check above.
    #[cfg(debug_assertions)]
    #[must_use]
    pub fn emulate_bucket_versioning_enabled_for_contract_test(&self) -> BucketVersioningEnabled {
        BucketVersioningEnabled {
            bucket: artifact_bucket_name(&self.artifact_root)
                .expect("resolved artifact root must contain a bucket")
                .to_string(),
            region: self.s3.region.clone(),
        }
    }

    /// Require that an opaque preflight proof belongs to this exact resolved
    /// bucket and region.
    pub fn validate_bucket_versioning_capability(
        &self,
        capability: &BucketVersioningEnabled,
    ) -> Result<()> {
        let bucket = artifact_bucket_name(&self.artifact_root)?;
        ensure!(
            capability.bucket == bucket && capability.region == self.s3.region,
            "bucket-versioning capability does not match the configured artifact root"
        );
        Ok(())
    }

    #[must_use]
    pub fn nt_catalog_storage_options(&self) -> AHashMap<String, String> {
        self.s3.nt_catalog_storage_options()
    }

    #[must_use]
    pub fn nt_catalog_storage_options_with_credentials(
        &self,
        credentials: &S3ArtifactStoreCredentials,
    ) -> AHashMap<String, String> {
        let mut options = self.nt_catalog_storage_options();
        options.insert(
            "access_key_id".to_string(),
            credentials.access_key_id().to_string(),
        );
        options.insert(
            "secret_access_key".to_string(),
            credentials.secret_access_key().to_string(),
        );
        if let Some(token) = credentials.session_token() {
            options.insert("session_token".to_string(), token.to_string());
        }
        options
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
    pub fn catalog_projection_manifest_object_uri(&self, catalog_projection_id: &str) -> String {
        format!(
            "{}{}",
            self.nt_catalog_projection_root(catalog_projection_id),
            self.catalog_projection_manifest_object.as_str()
        )
    }

    /// # Errors
    ///
    /// Returns an error if `proof_run_id` is not a valid artifact path token.
    pub fn nt_catalog_synthetic_proof_root(&self, proof_run_id: &str) -> Result<String> {
        ensure_path_token(
            "nt_catalog_synthetic_proof_run_id",
            proof_run_id,
            PathTokenMode::NoEquals,
        )?;
        ensure!(
            !proof_run_id.contains('/'),
            "nt_catalog_synthetic_proof_run_id must be a single artifact path token"
        );
        Ok(self.join([
            self.subpaths.nt_catalog_synthetic_proof.as_str(),
            "v1",
            &format!("proof={proof_run_id}"),
            "",
        ]))
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

    #[must_use]
    pub fn create_only_probe_uri(&self, probe_id: &str) -> String {
        self.join([
            self.create_only_probe.prefix.as_str(),
            &format!("probe={probe_id}"),
            self.create_only_probe.object_name.as_str(),
        ])
    }

    #[must_use]
    pub fn create_only_probe_copy_source_uri(&self, probe_id: &str) -> String {
        self.join([
            self.create_only_probe.prefix.as_str(),
            &format!("probe={probe_id}"),
            self.create_only_probe.copy_source_object_name.as_str(),
        ])
    }

    #[must_use]
    pub fn create_only_probe_copy_dest_uri(&self, probe_id: &str) -> String {
        self.join([
            self.create_only_probe.prefix.as_str(),
            &format!("probe={probe_id}"),
            self.create_only_probe.copy_dest_object_name.as_str(),
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

    /// Resolve an S3 object path anywhere in the configured artifact bucket.
    ///
    /// This is intentionally broader than [`Self::object_path_for_uri`] only
    /// in prefix, not in store ownership: staged raw inputs can live beside
    /// the canonical artifact root, but a different bucket is rejected before
    /// credentials or network access are attempted.
    pub(crate) fn object_path_for_same_bucket_uri(&self, uri: &str) -> Result<ObjectPath> {
        let trimmed = uri.trim();
        ensure!(
            uri == trimmed,
            "artifact-bucket URI must not contain surrounding whitespace"
        );
        let uri = trimmed;
        let without_scheme = uri
            .strip_prefix("s3://")
            .context("artifact-bucket URI must be an s3:// URI")?;
        let Some((bucket, object_path)) = without_scheme.split_once('/') else {
            bail!("artifact-bucket URI must include an S3 bucket and object path");
        };
        ensure!(
            bucket == artifact_bucket_name(&self.artifact_root)?,
            "artifact-bucket URI bucket {bucket:?} differs from the configured artifact bucket"
        );
        ensure_path_token(
            "artifact_bucket_uri",
            object_path,
            PathTokenMode::AllowEquals,
        )?;
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
            let trimmed = part.trim_matches('/');
            if !trimmed.is_empty() {
                if !uri.ends_with('/') {
                    uri.push('/');
                }
                uri.push_str(trimmed);
            } else if !uri.ends_with('/') {
                uri.push('/');
            }
        }
        uri
    }
}

impl S3ArtifactStoreConfig {
    #[must_use]
    pub fn nt_catalog_storage_options(&self) -> AHashMap<String, String> {
        let mut options = AHashMap::new();
        options.insert("region".to_string(), self.region.clone());
        options
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
    pub encoding: CatalogEncodingConfig,
    pub bindings: Vec<CatalogProjectionBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "CatalogEncodingConfigWire")]
pub struct CatalogEncodingConfig {
    batch_size: usize,
    max_row_group_size: usize,
    compression: CatalogCompression,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogEncodingConfigWire {
    batch_size: usize,
    max_row_group_size: usize,
    compression: CatalogCompression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogCompression {
    Snappy,
}

impl CatalogEncodingConfig {
    /// Build an explicit NautilusTrader catalog encoding configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when either configured row count is zero.
    pub fn new(
        batch_size: usize,
        max_row_group_size: usize,
        compression: CatalogCompression,
    ) -> Result<Self> {
        ensure!(
            batch_size > 0,
            "catalog encoding batch_size must be positive"
        );
        ensure!(
            max_row_group_size > 0,
            "catalog encoding max_row_group_size must be positive"
        );
        Ok(Self {
            batch_size,
            max_row_group_size,
            compression,
        })
    }

    /// Hash the exact explicit encoding values used to build the NT catalog.
    ///
    /// # Errors
    ///
    /// Returns an error when the encoding identity cannot be serialized.
    pub fn content_hash(&self) -> Result<String> {
        crate::reference_artifact::canonical_json_sha256(self)
            .context("hash exact catalog encoding config")
    }

    #[must_use]
    pub(crate) const fn batch_size(&self) -> usize {
        self.batch_size
    }

    #[must_use]
    pub(crate) const fn max_row_group_size(&self) -> usize {
        self.max_row_group_size
    }

    #[must_use]
    pub(crate) const fn compression(&self) -> CatalogCompression {
        self.compression
    }
}

impl TryFrom<CatalogEncodingConfigWire> for CatalogEncodingConfig {
    type Error = String;

    fn try_from(value: CatalogEncodingConfigWire) -> std::result::Result<Self, Self::Error> {
        Self::new(
            value.batch_size,
            value.max_row_group_size,
            value.compression,
        )
        .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionBinding {
    pub source_binding: String,
    pub market_structure_fixture: MarketStructureFixture,
    pub catalog_projection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PersistedCatalogProjectionObject {
    pub relative_path: String,
    pub uri: String,
    pub sha256: String,
    pub byte_len: u64,
    pub version_id: String,
    pub e_tag: String,
    #[serde(skip_serializing)]
    pub create_only_write: CreateOnlyWriteDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedCatalogProjection {
    pub catalog_root_uri: String,
    pub receipt_uri: String,
    pub receipt_byte_len: u64,
    pub physical_manifest_sha256: String,
    pub receipt_sha256: String,
    pub receipt_version_id: String,
    pub receipt_e_tag: String,
    pub receipt_create_only_write: CreateOnlyWriteDisposition,
    pub binding: CatalogProjectionBinding,
    pub objects: Vec<PersistedCatalogProjectionObject>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProjectionPublicationReceiptLocator {
    pub receipt_uri: String,
    pub receipt_sha256: String,
    pub receipt_version_id: String,
    pub receipt_e_tag: String,
}

impl PersistedCatalogProjection {
    #[must_use]
    pub fn receipt_locator(&self) -> CatalogProjectionPublicationReceiptLocator {
        CatalogProjectionPublicationReceiptLocator {
            receipt_uri: self.receipt_uri.clone(),
            receipt_sha256: self.receipt_sha256.clone(),
            receipt_version_id: self.receipt_version_id.clone(),
            receipt_e_tag: self.receipt_e_tag.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionPublicationObject {
    pub relative_path: String,
    pub uri: String,
    pub sha256: String,
    pub byte_len: u64,
    pub version_id: String,
    pub e_tag: String,
}

/// Immutable read authority for one published catalog projection.
///
/// S3 `If-None-Match` is collision protection, not the integrity authority: on
/// a versioned bucket it may create a new current version when the prior
/// current version is a delete marker. Readers therefore bind every object to
/// this receipt's exact version ID, ETag, byte length, and SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProjectionPublicationReceipt {
    pub schema_version: String,
    pub catalog_root_uri: String,
    pub physical_manifest_sha256: String,
    pub physical_manifest: CatalogProjectionManifestDocument,
    pub binding: CatalogProjectionBinding,
    pub objects: Vec<CatalogProjectionPublicationObject>,
}

#[derive(Serialize)]
struct CatalogProjectionPublicationReceiptRef<'a> {
    schema_version: &'static str,
    catalog_root_uri: &'a str,
    physical_manifest_sha256: &'a str,
    physical_manifest: &'a CatalogProjectionManifestDocument,
    binding: &'a CatalogProjectionBinding,
    objects: &'a [PersistedCatalogProjectionObject],
}

impl CatalogProjectionPublicationReceiptRef<'_> {
    fn validate_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        validate_catalog_projection_publication_receipt_parts(
            CatalogProjectionPublicationReceiptValidation {
                schema_version: self.schema_version,
                catalog_root_uri: self.catalog_root_uri,
                physical_manifest_sha256: self.physical_manifest_sha256,
                physical_manifest: self.physical_manifest,
                binding: self.binding,
                work_budget,
                stage,
            },
            self.objects.len(),
            self.objects
                .iter()
                .map(CatalogProjectionPublicationObjectView::from),
        )
    }

    fn canonical_bytes_guarded(
        &self,
        retained_publication_metadata_bytes: u64,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Vec<u8>> {
        self.validate_guarded(work_budget, stage)?;
        let serialized_bytes = serialized_json_len_guarded(self, work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication metadata plus canonical receipt serialization",
            &[retained_publication_metadata_bytes, serialized_bytes],
            work_budget,
            stage,
        )?;
        let bytes = serialize_json_to_vec_guarded(self, work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication metadata plus canonical receipt payload",
            &[
                retained_publication_metadata_bytes,
                u64::try_from(bytes.capacity())
                    .context("canonical receipt capacity does not fit u64")?,
            ],
            work_budget,
            stage,
        )?;
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
struct CatalogProjectionPublicationObjectView<'a> {
    relative_path: &'a str,
    uri: &'a str,
    sha256: &'a str,
    byte_len: u64,
    version_id: &'a str,
    e_tag: &'a str,
}

impl<'a> From<&'a CatalogProjectionPublicationObject>
    for CatalogProjectionPublicationObjectView<'a>
{
    fn from(object: &'a CatalogProjectionPublicationObject) -> Self {
        Self {
            relative_path: &object.relative_path,
            uri: &object.uri,
            sha256: &object.sha256,
            byte_len: object.byte_len,
            version_id: &object.version_id,
            e_tag: &object.e_tag,
        }
    }
}

impl<'a> From<&'a PersistedCatalogProjectionObject> for CatalogProjectionPublicationObjectView<'a> {
    fn from(object: &'a PersistedCatalogProjectionObject) -> Self {
        Self {
            relative_path: &object.relative_path,
            uri: &object.uri,
            sha256: &object.sha256,
            byte_len: object.byte_len,
            version_id: &object.version_id,
            e_tag: &object.e_tag,
        }
    }
}

struct CatalogProjectionPublicationReceiptValidation<'a> {
    schema_version: &'a str,
    catalog_root_uri: &'a str,
    physical_manifest_sha256: &'a str,
    physical_manifest: &'a CatalogProjectionManifestDocument,
    binding: &'a CatalogProjectionBinding,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

fn validate_catalog_projection_publication_receipt_parts<'a>(
    validation: CatalogProjectionPublicationReceiptValidation<'_>,
    object_count: usize,
    objects: impl Iterator<Item = CatalogProjectionPublicationObjectView<'a>>,
) -> Result<()> {
    let CatalogProjectionPublicationReceiptValidation {
        schema_version,
        catalog_root_uri,
        physical_manifest_sha256,
        physical_manifest,
        binding,
        work_budget,
        stage,
    } = validation;
    work_budget.check_deadline(stage)?;
    ensure!(
        schema_version == CATALOG_PROJECTION_PUBLICATION_RECEIPT_SCHEMA_VERSION,
        "unsupported catalog projection publication receipt schema_version {schema_version:?}"
    );
    ensure!(
        catalog_root_uri.starts_with("s3://") && catalog_root_uri.ends_with('/'),
        "catalog publication receipt catalog_root_uri must be an S3 directory URI"
    );
    ensure_path_token(
        "catalog_projection_id",
        &binding.catalog_projection_id,
        PathTokenMode::AllowEquals,
    )?;
    ensure!(
        !binding.source_binding.trim().is_empty(),
        "catalog publication receipt source_binding must not be blank"
    );
    physical_manifest
        .validate_guarded(work_budget, stage)
        .map_err(anyhow::Error::from)
        .context("validate receipt physical manifest")?;
    ensure_sha256(
        "catalog publication receipt physical_manifest_sha256",
        physical_manifest_sha256,
    )?;
    let actual_physical_manifest_sha256 = physical_manifest
        .manifest_sha256_guarded(work_budget, stage)
        .context("hash receipt physical manifest")?;
    ensure!(
        actual_physical_manifest_sha256 == physical_manifest_sha256,
        "catalog publication receipt physical_manifest_sha256 mismatch: expected {physical_manifest_sha256}, got {actual_physical_manifest_sha256}"
    );
    ensure!(
        object_count == physical_manifest.objects.len(),
        "catalog publication receipt object count {object_count} does not match physical manifest {}",
        physical_manifest.objects.len()
    );
    work_budget.verify_actual_row_groups(
        u64::try_from(object_count)
            .context("catalog publication receipt object count does not fit u64")?,
        stage,
    )?;

    let mut metadata_bytes =
        u64::try_from(std::mem::size_of::<CatalogProjectionPublicationReceipt>())
            .context("catalog publication receipt metadata size does not fit u64")?;
    for (receipt_object, physical_object) in objects.zip(&physical_manifest.objects) {
        work_budget.check_deadline(stage)?;
        ensure!(
            receipt_object.relative_path == physical_object.relative_path,
            "catalog publication receipt relative_path {:?} does not match physical manifest {:?}",
            receipt_object.relative_path,
            physical_object.relative_path
        );
        ensure!(
            receipt_object.byte_len == physical_object.byte_len,
            "catalog publication receipt byte_len for {} does not match physical manifest",
            physical_object.relative_path
        );
        ensure!(
            receipt_object.sha256 == physical_object.sha256,
            "catalog publication receipt SHA-256 for {} does not match physical manifest",
            physical_object.relative_path
        );
        ensure_immutable_s3_version_id(
            &format!(
                "catalog publication receipt version_id for {}",
                physical_object.relative_path
            ),
            receipt_object.version_id,
        )?;
        ensure!(
            !receipt_object.e_tag.trim().is_empty(),
            "catalog publication receipt ETag for {} must not be empty",
            physical_object.relative_path
        );
        ensure!(
            receipt_object
                .uri
                .strip_prefix(catalog_root_uri)
                .is_some_and(|suffix| suffix == physical_object.relative_path.as_str()),
            "catalog publication receipt URI {:?} is not the exact root-relative locator for {}",
            receipt_object.uri,
            physical_object.relative_path
        );
        let object_metadata_bytes = receipt_object
            .relative_path
            .len()
            .checked_add(receipt_object.uri.len())
            .and_then(|value| value.checked_add(receipt_object.sha256.len()))
            .and_then(|value| value.checked_add(receipt_object.version_id.len()))
            .and_then(|value| value.checked_add(receipt_object.e_tag.len()))
            .and_then(|value| {
                value.checked_add(std::mem::size_of::<CatalogProjectionPublicationObject>())
            })
            .context("catalog publication receipt object metadata size overflow")?;
        metadata_bytes = metadata_bytes
            .checked_add(
                u64::try_from(object_metadata_bytes)
                    .context("catalog publication receipt object metadata does not fit u64")?,
            )
            .context("catalog publication receipt metadata total overflow")?;
    }
    metadata_bytes = metadata_bytes
        .checked_add(
            u64::try_from(
                schema_version
                    .len()
                    .checked_add(catalog_root_uri.len())
                    .and_then(|value| value.checked_add(physical_manifest_sha256.len()))
                    .and_then(|value| value.checked_add(binding.source_binding.len()))
                    .and_then(|value| value.checked_add(binding.catalog_projection_id.len()))
                    .context("catalog publication receipt header metadata size overflow")?,
            )
            .context("catalog publication receipt header metadata does not fit u64")?,
        )
        .context("catalog publication receipt metadata total overflow")?;
    work_budget.verify_decoded_bytes(metadata_bytes, stage)?;
    work_budget.check_deadline(stage)
}

impl CatalogProjectionPublicationReceipt {
    fn retained_memory_bytes(&self) -> Result<u64> {
        let mut retained = u64::try_from(std::mem::size_of::<Self>())
            .context("catalog publication receipt retained size does not fit u64")?;
        let string_capacity = |value: &String| -> Result<u64> {
            u64::try_from(value.capacity())
                .context("catalog publication receipt string capacity does not fit u64")
        };
        for value in [
            &self.schema_version,
            &self.catalog_root_uri,
            &self.physical_manifest_sha256,
            &self.binding.source_binding,
            &self.binding.catalog_projection_id,
            &self.physical_manifest.schema_version,
        ] {
            retained = retained
                .checked_add(string_capacity(value)?)
                .context("catalog publication receipt retained string total overflow")?;
        }
        retained = retained
            .checked_add(
                u64::try_from(
                    self.physical_manifest
                        .objects
                        .capacity()
                        .checked_mul(std::mem::size_of::<
                            crate::run_manifest::CatalogProjectionManifestObject,
                        >())
                        .context("physical manifest retained vector capacity overflow")?,
                )
                .context("physical manifest retained vector bytes do not fit u64")?,
            )
            .context("catalog publication receipt retained total overflow")?;
        for object in &self.physical_manifest.objects {
            retained = retained
                .checked_add(string_capacity(&object.relative_path)?)
                .context("physical manifest retained path total overflow")?;
            retained = retained
                .checked_add(string_capacity(&object.sha256)?)
                .context("physical manifest retained hash total overflow")?;
        }
        retained = retained
            .checked_add(
                u64::try_from(
                    self.objects
                        .capacity()
                        .checked_mul(std::mem::size_of::<CatalogProjectionPublicationObject>())
                        .context("publication receipt object vector capacity overflow")?,
                )
                .context("publication receipt object vector bytes do not fit u64")?,
            )
            .context("catalog publication receipt retained total overflow")?;
        for object in &self.objects {
            for value in [
                &object.relative_path,
                &object.uri,
                &object.sha256,
                &object.version_id,
            ] {
                retained = retained
                    .checked_add(string_capacity(value)?)
                    .context("publication receipt retained object string total overflow")?;
            }
            retained = retained
                .checked_add(string_capacity(&object.e_tag)?)
                .context("publication receipt retained ETag total overflow")?;
        }
        Ok(retained)
    }

    /// Validate that this receipt is only locator/version evidence for the
    /// embedded shared physical manifest, never a second content authority.
    pub fn validate_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<()> {
        validate_catalog_projection_publication_receipt_parts(
            CatalogProjectionPublicationReceiptValidation {
                schema_version: &self.schema_version,
                catalog_root_uri: &self.catalog_root_uri,
                physical_manifest_sha256: &self.physical_manifest_sha256,
                physical_manifest: &self.physical_manifest,
                binding: &self.binding,
                work_budget,
                stage,
            },
            self.objects.len(),
            self.objects
                .iter()
                .map(CatalogProjectionPublicationObjectView::from),
        )
    }

    /// Canonical compact JSON bytes for immutable publication and replay.
    pub fn canonical_bytes_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Vec<u8>> {
        self.validate_guarded(work_budget, stage)?;
        let retained_receipt_bytes = self.retained_memory_bytes()?;
        let serialized_bytes = serialized_json_len_guarded(self, work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication receipt plus canonical serialization",
            &[retained_receipt_bytes, serialized_bytes],
            work_budget,
            stage,
        )?;
        let bytes = serialize_json_to_vec_guarded(self, work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication receipt plus canonical serialization",
            &[
                retained_receipt_bytes,
                u64::try_from(bytes.capacity())
                    .context("canonical receipt capacity does not fit u64")?,
            ],
            work_budget,
            stage,
        )?;
        Ok(bytes)
    }

    /// SHA-256 of the canonical receipt JSON without materializing it.
    pub fn receipt_sha256_guarded(
        &self,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<String> {
        self.validate_guarded(work_budget, stage)?;
        sha256_json_guarded(self, work_budget, stage)
    }

    /// Parse a committed receipt, require exact canonical bytes, and bind the
    /// caller-supplied immutable receipt hash before any hydration is allowed.
    pub fn parse_and_validate_guarded(
        bytes: &[u8],
        expected_receipt_sha256: &str,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        let input_retained_bytes = u64::try_from(bytes.len())
            .context("catalog publication receipt byte length does not fit u64")?;
        Self::parse_and_validate_with_input_retained_guarded(
            bytes,
            input_retained_bytes,
            expected_receipt_sha256,
            work_budget,
            stage,
        )
    }

    fn parse_and_validate_with_input_retained_guarded(
        bytes: &[u8],
        input_retained_bytes: u64,
        expected_receipt_sha256: &str,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        ensure_sha256(
            "expected catalog publication receipt SHA-256",
            expected_receipt_sha256,
        )?;
        let wire_bytes = u64::try_from(bytes.len())
            .context("catalog publication receipt wire bytes do not fit u64")?;
        ensure!(
            input_retained_bytes >= wire_bytes,
            "catalog publication receipt retained input bytes cannot be smaller than wire bytes"
        );
        work_budget.verify_decoded_bytes(input_retained_bytes, stage)?;
        let actual_receipt_sha256 = sha256_hex_with_budget(bytes, work_budget, stage)?;
        ensure!(
            actual_receipt_sha256 == expected_receipt_sha256,
            "catalog publication receipt SHA-256 mismatch: expected {expected_receipt_sha256}, got {actual_receipt_sha256}"
        );
        let conservative_parsed_bytes =
            conservative_parsed_receipt_retained_upper_bound(bytes, work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication receipt wire plus parsed-memory upper bound",
            &[input_retained_bytes, conservative_parsed_bytes],
            work_budget,
            stage,
        )?;
        let receipt: Self = deserialize_json_with_budget(bytes, work_budget, stage)
            .context("parse catalog projection publication receipt")?;
        receipt.validate_guarded(work_budget, stage)?;
        let retained_receipt_bytes = receipt.retained_memory_bytes()?;
        verify_cumulative_retained_bytes(
            "catalog publication receipt wire plus parsed document",
            &[input_retained_bytes, retained_receipt_bytes],
            work_budget,
            stage,
        )?;
        let canonical_bytes_len = serialized_json_len_guarded(&receipt, work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication receipt wire, parsed document, and canonical serialization",
            &[
                input_retained_bytes,
                retained_receipt_bytes,
                canonical_bytes_len,
            ],
            work_budget,
            stage,
        )?;
        let canonical_bytes = receipt.canonical_bytes_guarded(work_budget, stage)?;
        verify_cumulative_retained_bytes(
            "catalog publication receipt wire, parsed document, and canonical serialization",
            &[
                input_retained_bytes,
                retained_receipt_bytes,
                u64::try_from(canonical_bytes.capacity())
                    .context("canonical receipt capacity does not fit u64")?,
            ],
            work_budget,
            stage,
        )?;
        ensure!(
            canonical_bytes == bytes,
            "catalog projection publication receipt bytes are not canonical"
        );
        let canonical_sha256 = receipt.receipt_sha256_guarded(work_budget, stage)?;
        ensure!(
            canonical_sha256 == expected_receipt_sha256,
            "canonical catalog publication receipt SHA-256 mismatch: expected {expected_receipt_sha256}, got {canonical_sha256}"
        );
        Ok(receipt)
    }
}

struct PrivateCatalogRootLease {
    path: PathBuf,
    directory: fs::File,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl std::fmt::Debug for PrivateCatalogRootLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrivateCatalogRootLease")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl PrivateCatalogRootLease {
    fn open_empty(path: &Path) -> Result<Self> {
        let path_metadata = fs::symlink_metadata(path)
            .with_context(|| format!("inspect private hydration root {}", path.display()))?;
        ensure!(
            path_metadata.file_type().is_dir(),
            "private hydration root {} must be a real directory",
            path.display()
        );
        validate_private_catalog_root_permissions(path, &path_metadata)?;
        let mut entries = fs::read_dir(path)
            .with_context(|| format!("read private hydration root {}", path.display()))?;
        ensure!(
            entries.next().transpose()?.is_none(),
            "private hydration root {} must be empty",
            path.display()
        );
        #[cfg(unix)]
        let directory = {
            let mut options = fs::OpenOptions::new();
            options
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
            options
                .open(path)
                .with_context(|| format!("open private hydration root {}", path.display()))?
        };
        #[cfg(not(unix))]
        let directory = fs::File::open(path)
            .with_context(|| format!("open private hydration root {}", path.display()))?;
        let handle_metadata = directory
            .metadata()
            .with_context(|| format!("fstat private hydration root {}", path.display()))?;
        ensure!(
            handle_metadata.file_type().is_dir(),
            "private hydration root handle {} is not a directory",
            path.display()
        );
        #[cfg(unix)]
        ensure!(
            path_metadata.dev() == handle_metadata.dev()
                && path_metadata.ino() == handle_metadata.ino(),
            "private hydration root {} changed identity while opening",
            path.display()
        );
        let final_path_metadata = fs::symlink_metadata(path).with_context(|| {
            format!(
                "reinspect private hydration root {} after open",
                path.display()
            )
        })?;
        ensure!(
            final_path_metadata.file_type().is_dir(),
            "private hydration root {} changed type while opening",
            path.display()
        );
        validate_private_catalog_root_permissions(path, &final_path_metadata)?;
        #[cfg(unix)]
        ensure!(
            final_path_metadata.dev() == handle_metadata.dev()
                && final_path_metadata.ino() == handle_metadata.ino(),
            "private hydration root {} changed namespace identity while opening",
            path.display()
        );
        Ok(Self {
            path: path.to_path_buf(),
            directory,
            #[cfg(unix)]
            device: handle_metadata.dev(),
            #[cfg(unix)]
            inode: handle_metadata.ino(),
        })
    }

    fn revalidate(&self) -> Result<()> {
        let path_metadata = fs::symlink_metadata(&self.path)
            .with_context(|| format!("reinspect private hydration root {}", self.path.display()))?;
        ensure!(
            path_metadata.file_type().is_dir(),
            "private hydration root {} is no longer a real directory",
            self.path.display()
        );
        validate_private_catalog_root_permissions(&self.path, &path_metadata)?;
        let handle_metadata = self
            .directory
            .metadata()
            .with_context(|| format!("re-fstat private hydration root {}", self.path.display()))?;
        ensure!(
            handle_metadata.file_type().is_dir(),
            "private hydration root handle {} is no longer a directory",
            self.path.display()
        );
        #[cfg(unix)]
        ensure!(
            path_metadata.dev() == self.device
                && path_metadata.ino() == self.inode
                && handle_metadata.dev() == self.device
                && handle_metadata.ino() == self.inode,
            "private hydration root {} changed identity",
            self.path.display()
        );
        let final_path_metadata = fs::symlink_metadata(&self.path).with_context(|| {
            format!(
                "reinspect private hydration root {} after fstat",
                self.path.display()
            )
        })?;
        ensure!(
            final_path_metadata.file_type().is_dir(),
            "private hydration root {} changed type during revalidation",
            self.path.display()
        );
        validate_private_catalog_root_permissions(&self.path, &final_path_metadata)?;
        #[cfg(unix)]
        ensure!(
            final_path_metadata.dev() == self.device && final_path_metadata.ino() == self.inode,
            "private hydration root {} changed namespace identity during revalidation",
            self.path.display()
        );
        Ok(())
    }
}

/// A verified local catalog view whose private root descriptor must remain
/// alive until the runner seals the same shared physical manifest.
#[derive(Debug)]
pub struct HydratedCatalogProjection {
    root_lease: PrivateCatalogRootLease,
    pub catalog_root_uri: String,
    pub binding: CatalogProjectionBinding,
    pub physical_manifest_sha256: String,
    pub receipt_sha256: String,
    pub receipt_version_id: String,
    pub receipt_e_tag: String,
    pub object_count: usize,
}

impl HydratedCatalogProjection {
    #[must_use]
    pub fn local_catalog_root(&self) -> &Path {
        &self.root_lease.path
    }

    /// Re-pin the retained root and re-check the exact manifest set immediately
    /// before runner sealing. The runner owns its content pre/post hashes;
    /// dropping this lease discards hydration authority.
    pub fn revalidate_for_runner_seal_guarded(
        &self,
        expected_physical_manifest: &CatalogProjectionManifestDocument,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<()> {
        self.root_lease.revalidate()?;
        verify_local_catalog_projection_exact_set_guarded(
            &self.root_lease.path,
            expected_physical_manifest,
            work_budget,
            OperatorWorkBudgetStage::Backtest,
        )?;
        self.root_lease.revalidate()
    }
}

#[cfg(unix)]
fn validate_private_catalog_root_permissions(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    ensure!(
        metadata.permissions().mode() & 0o777 == 0o700,
        "private hydration root {} must have mode 0700",
        path.display()
    );
    Ok(())
}

#[cfg(not(unix))]
fn validate_private_catalog_root_permissions(_path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateOnlyWriteDisposition {
    Created,
}

/// One non-cloneable terminal create prepared completely before the caller
/// consumes its one-use commit permit.
pub(crate) struct PreparedTerminalCreate {
    path: ObjectPath,
    payload: Bytes,
    object_label: String,
}

/// Fully acknowledged identity for one freshly created terminal object.
///
/// Only the direct create response can construct this value. A conflict,
/// missing version ID, or missing ETag is never reconciled into success.
pub(crate) struct CreatedTerminalObject {
    pub(crate) version_id: String,
    pub(crate) e_tag: String,
}

/// The create request may have committed, but its direct acknowledgement did
/// not prove a complete immutable identity. Callers must stop; automatic
/// retry, discovery-based success, or cleanup is forbidden.
#[derive(Debug)]
pub struct TerminalCreateIndeterminate {
    detail: String,
}

impl std::fmt::Display for TerminalCreateIndeterminate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "terminal create is indeterminate: {}",
            self.detail
        )
    }
}

impl std::error::Error for TerminalCreateIndeterminate {}

#[must_use]
pub fn is_terminal_create_indeterminate(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<TerminalCreateIndeterminate>()
            .is_some()
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateOnlyProbeTranscript {
    pub probe_uri: String,
    pub copy_source_uri: String,
    pub copy_dest_uri: String,
    pub first_create_succeeded: bool,
    pub duplicate_create_rejected: bool,
    pub first_copy_succeeded: bool,
    pub duplicate_copy_rejected: bool,
}

impl CatalogDispatchConfig {
    fn binding_for(
        &self,
        source_binding: &str,
        expected_market_structure_fixture: MarketStructureFixture,
    ) -> Result<&CatalogProjectionBinding> {
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
        ensure!(
            binding.market_structure_fixture == expected_market_structure_fixture,
            "catalog dispatch market_structure_fixture mismatch for {source_binding:?}: expected {:?}, configured {:?}",
            expected_market_structure_fixture,
            binding.market_structure_fixture
        );
        Ok(binding)
    }

    /// # Errors
    ///
    /// Returns an error if the source binding is missing, ambiguous, or resolves
    /// to an invalid projection id.
    pub fn catalog_root_for(
        &self,
        source_binding: &str,
        expected_market_structure_fixture: MarketStructureFixture,
        artifact_root: &ResolvedArtifactRoot,
    ) -> Result<String> {
        let binding = self.binding_for(source_binding, expected_market_structure_fixture)?;
        Ok(artifact_root.nt_catalog_projection_root(&binding.catalog_projection_id))
    }
}

/// Persist one catalog projection while checking the shared operator deadline
/// before and after every immutable object write.
///
/// # Errors
///
/// Returns an error if dispatch, local projection validation, immutable object
/// publication, or the explicit operator work budget fails.
// Keep the independently validated store, versioning capability, dispatch
// authority, source identity, physical manifest, and budget explicit at this
// public security boundary.
#[allow(clippy::too_many_arguments)]
pub async fn persist_catalog_projection_for_source_binding_guarded(
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    versioning_enabled: &BucketVersioningEnabled,
    dispatch: &CatalogDispatchConfig,
    source_binding: &str,
    expected_market_structure_fixture: MarketStructureFixture,
    local_catalog_root: &Path,
    expected_physical_manifest: &CatalogProjectionManifestDocument,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<PersistedCatalogProjection> {
    artifact_root.validate_bucket_versioning_capability(versioning_enabled)?;
    catalog_projection_for_source_binding_guarded(
        CatalogProjectionPublicationRequest {
            store,
            artifact_root,
            dispatch,
            source_binding,
            expected_market_structure_fixture,
            expected_physical_manifest,
            work_budget,
        },
        local_catalog_root,
    )
    .await
}

struct CatalogProjectionPublicationRequest<'a> {
    store: &'a dyn ObjectStore,
    artifact_root: &'a ResolvedArtifactRoot,
    dispatch: &'a CatalogDispatchConfig,
    source_binding: &'a str,
    expected_market_structure_fixture: MarketStructureFixture,
    expected_physical_manifest: &'a CatalogProjectionManifestDocument,
    work_budget: &'a OperatorWorkBudgetGuard,
}

async fn catalog_projection_for_source_binding_guarded(
    request: CatalogProjectionPublicationRequest<'_>,
    local_catalog_root: &Path,
) -> Result<PersistedCatalogProjection> {
    let CatalogProjectionPublicationRequest {
        store,
        artifact_root,
        dispatch,
        source_binding,
        expected_market_structure_fixture,
        expected_physical_manifest,
        work_budget,
    } = request;
    let (binding, catalog_root_uri, physical_manifest_sha256) = guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Publish,
        || -> Result<_> {
            let root_metadata = fs::symlink_metadata(local_catalog_root).with_context(|| {
                format!(
                    "inspect local catalog projection root {}",
                    local_catalog_root.display()
                )
            })?;
            ensure!(
                root_metadata.file_type().is_dir(),
                "local catalog projection root {} must be a real directory",
                local_catalog_root.display()
            );
            expected_physical_manifest
                .validate_guarded(work_budget, OperatorWorkBudgetStage::Publish)
                .map_err(anyhow::Error::from)
                .context("validate producer-minted catalog physical manifest")?;
            ensure!(
                artifact_root.catalog_projection_manifest_object.as_str()
                    != CATALOG_RUN_VIEW_AUTHORITY_FILE,
                "catalog publication receipt object must not overwrite the catalog run-view authority file"
            );
            for object in &expected_physical_manifest.objects {
                work_budget.check_deadline(OperatorWorkBudgetStage::Publish)?;
                ensure!(
                    object.relative_path
                        != artifact_root.catalog_projection_manifest_object.as_str(),
                    "catalog physical manifest contains configured publication receipt object {}",
                    object.relative_path
                );
                ensure!(
                    object.relative_path != CATALOG_RUN_VIEW_AUTHORITY_FILE,
                    "catalog physical manifest contains reserved run-view authority object {}",
                    object.relative_path
                );
            }
            let configured_binding =
                dispatch.binding_for(source_binding, expected_market_structure_fixture)?;
            let binding = CatalogProjectionBinding {
                source_binding: clone_string_guarded(
                    &configured_binding.source_binding,
                    "catalog publication source binding",
                    work_budget,
                    OperatorWorkBudgetStage::Publish,
                )?,
                market_structure_fixture: configured_binding.market_structure_fixture,
                catalog_projection_id: clone_string_guarded(
                    &configured_binding.catalog_projection_id,
                    "catalog publication projection ID",
                    work_budget,
                    OperatorWorkBudgetStage::Publish,
                )?,
            };
            let catalog_root_uri =
                artifact_root.nt_catalog_projection_root(&binding.catalog_projection_id);
            let physical_manifest_sha256 = expected_physical_manifest
                .manifest_sha256_guarded(work_budget, OperatorWorkBudgetStage::Publish)
                .context("hash producer-minted catalog physical manifest")?;
            Ok((binding, catalog_root_uri, physical_manifest_sha256))
        },
    )??;

    verify_local_catalog_projection_exact_set_guarded(
        local_catalog_root,
        expected_physical_manifest,
        work_budget,
        OperatorWorkBudgetStage::Publish,
    )?;
    let planned_publication_peak = preflight_catalog_publication_retained_peak(
        expected_physical_manifest,
        &binding,
        &catalog_root_uri,
        &physical_manifest_sha256,
        artifact_root.max_final_object_bytes,
        work_budget,
        OperatorWorkBudgetStage::Publish,
    )?;
    let maximum_catalog_payload_bytes = expected_physical_manifest
        .objects
        .iter()
        .map(|object| object.byte_len)
        .max()
        .context("catalog physical manifest must contain an object")?;

    let writer = CreateOnlyArtifactWriter::new(store, artifact_root);
    let mut objects = Vec::new();
    objects
        .try_reserve_exact(expected_physical_manifest.objects.len())
        .context("reserve persisted catalog publication objects")?;
    let excess_object_slots = objects
        .capacity()
        .checked_sub(expected_physical_manifest.objects.len())
        .context("persisted catalog object vector capacity regressed below reservation")?;
    let excess_vector_bytes = u64::try_from(
        excess_object_slots
            .checked_mul(std::mem::size_of::<PersistedCatalogProjectionObject>())
            .context("persisted catalog excess vector capacity overflow")?,
    )
    .context("persisted catalog excess vector bytes do not fit u64")?;
    verify_cumulative_retained_bytes(
        "catalog publication planned peak plus allocator vector slack",
        &[planned_publication_peak, excess_vector_bytes],
        work_budget,
        OperatorWorkBudgetStage::Publish,
    )?;
    for expected_object in &expected_physical_manifest.objects {
        let (file_path, uri, object_path) = guarded_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            || -> Result<_> {
                let file_path = contained_catalog_object_path(
                    local_catalog_root,
                    &expected_object.relative_path,
                    work_budget,
                    OperatorWorkBudgetStage::Publish,
                )?;
                enforce_final_object_byte_cap(
                    &format!("catalog projection file {}", expected_object.relative_path),
                    expected_object.byte_len,
                    artifact_root.max_final_object_bytes,
                )?;
                let uri = catalog_object_uri_guarded(
                    &catalog_root_uri,
                    &expected_object.relative_path,
                    work_budget,
                    OperatorWorkBudgetStage::Publish,
                );
                let uri = uri?;
                let object_path = artifact_root.object_path_for_uri(&uri)?;
                Ok((file_path, uri, object_path))
            },
        )??;
        let (payload, identity) = read_exact_sized_hashed_pinned_file_guarded(
            &file_path,
            expected_object.byte_len,
            &expected_object.sha256,
            work_budget,
            OperatorWorkBudgetStage::Publish,
        )
        .with_context(|| {
            format!(
                "read producer-authorized catalog object {}",
                expected_object.relative_path
            )
        })?;
        verify_retained_vec_capacity(
            &format!(
                "single-PUT catalog projection payload {}",
                expected_object.relative_path
            ),
            payload.capacity(),
            work_budget,
            OperatorWorkBudgetStage::Publish,
        )?;
        let created = writer
            .put_create_strict_guarded(&object_path, payload, work_budget)
            .await
            .with_context(|| format!("persist catalog object {uri}"))?;
        identity.revalidate_path(&file_path).with_context(|| {
            format!(
                "revalidate local catalog object {} after remote create",
                expected_object.relative_path
            )
        })?;
        let version_id = created.version_id;
        let e_tag = created.e_tag;
        let current_retained_metadata = catalog_publication_retained_metadata_bytes(
            expected_physical_manifest,
            &binding,
            &catalog_root_uri,
            &physical_manifest_sha256,
            &objects,
        )?;
        let e_tag_capacity = string_capacity_bytes(&e_tag, "catalog object ETag")?;
        let relative_path_bytes = u64::try_from(expected_object.relative_path.len())
            .context("catalog object relative_path length does not fit u64")?;
        let sha256_bytes = u64::try_from(expected_object.sha256.len())
            .context("catalog object SHA-256 length does not fit u64")?;
        let prospective_object_strings = string_capacity_bytes(&uri, "catalog object URI")?
            .checked_add(string_capacity_bytes(
                &version_id,
                "catalog object version ID",
            )?)
            .and_then(|value| value.checked_add(e_tag_capacity))
            .and_then(|value| value.checked_add(relative_path_bytes))
            .and_then(|value| value.checked_add(sha256_bytes))
            .context("prospective catalog object string allocation overflow")?;
        verify_cumulative_retained_bytes(
            "catalog publication retained metadata, returned version evidence, owned strings, and next payload",
            &[
                current_retained_metadata,
                prospective_object_strings,
                maximum_catalog_payload_bytes,
            ],
            work_budget,
            OperatorWorkBudgetStage::Publish,
        )?;
        objects.push(PersistedCatalogProjectionObject {
            relative_path: clone_string_guarded(
                &expected_object.relative_path,
                "persisted catalog relative_path",
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?,
            uri,
            sha256: clone_string_guarded(
                &expected_object.sha256,
                "persisted catalog SHA-256",
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?,
            byte_len: expected_object.byte_len,
            version_id,
            e_tag,
            create_only_write: CreateOnlyWriteDisposition::Created,
        });
        let retained_metadata = catalog_publication_retained_metadata_bytes(
            expected_physical_manifest,
            &binding,
            &catalog_root_uri,
            &physical_manifest_sha256,
            &objects,
        )?;
        verify_cumulative_retained_bytes(
            "catalog publication live metadata plus next sequential single-PUT payload",
            &[retained_metadata, maximum_catalog_payload_bytes],
            work_budget,
            OperatorWorkBudgetStage::Publish,
        )?;
    }
    verify_local_catalog_projection_exact_set_guarded(
        local_catalog_root,
        expected_physical_manifest,
        work_budget,
        OperatorWorkBudgetStage::Publish,
    )?;

    let receipt = CatalogProjectionPublicationReceiptRef {
        schema_version: CATALOG_PROJECTION_PUBLICATION_RECEIPT_SCHEMA_VERSION,
        catalog_root_uri: &catalog_root_uri,
        physical_manifest_sha256: &physical_manifest_sha256,
        physical_manifest: expected_physical_manifest,
        binding: &binding,
        objects: &objects,
    };

    let (receipt_uri, receipt_path) = guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Publish,
        || -> Result<_> {
            let receipt_uri = artifact_root
                .catalog_projection_manifest_object_uri(&binding.catalog_projection_id);
            let receipt_path = artifact_root.object_path_for_uri(&receipt_uri)?;
            Ok((receipt_uri, receipt_path))
        },
    )??;
    let receipt_path_bytes = u64::try_from(receipt_path.as_ref().len())
        .context("catalog publication receipt object path length does not fit u64")?;
    let retained_publication_metadata = catalog_publication_retained_metadata_bytes(
        expected_physical_manifest,
        &binding,
        &catalog_root_uri,
        &physical_manifest_sha256,
        &objects,
    )?
    .checked_add(string_capacity_bytes(
        &receipt_uri,
        "catalog publication receipt URI",
    )?)
    .and_then(|value| value.checked_add(receipt_path_bytes))
    .context("catalog publication retained receipt locator total overflow")?;
    let receipt_payload = receipt
        .canonical_bytes_guarded(
            retained_publication_metadata,
            work_budget,
            OperatorWorkBudgetStage::Publish,
        )
        .context("serialize catalog projection publication receipt")?;
    let receipt_payload_bytes = u64::try_from(receipt_payload.len())
        .context("catalog projection publication receipt length does not fit u64")?;
    enforce_final_object_byte_cap(
        "catalog projection publication receipt payload",
        receipt_payload_bytes,
        artifact_root.max_final_object_bytes,
    )?;
    let receipt_sha256 = sha256_hex_with_budget(
        &receipt_payload,
        work_budget,
        OperatorWorkBudgetStage::Publish,
    )
    .context("hash canonical catalog projection publication receipt")?;
    let prepared_receipt = writer.prepare_terminal_create_uri(
        artifact_root,
        &receipt_uri,
        receipt_payload,
        format!("catalog projection publication receipt {receipt_uri}"),
    )?;
    let permit = work_budget.authorize_commit(OperatorWorkBudgetStage::Publish)?;
    let created_receipt = writer
        .create_terminal_strict(prepared_receipt, permit)
        .await
        .with_context(|| format!("persist catalog projection publication receipt {receipt_uri}"))?;
    Ok(PersistedCatalogProjection {
        catalog_root_uri,
        receipt_uri,
        receipt_byte_len: receipt_payload_bytes,
        physical_manifest_sha256,
        receipt_sha256,
        receipt_version_id: created_receipt.version_id,
        receipt_e_tag: created_receipt.e_tag,
        receipt_create_only_write: CreateOnlyWriteDisposition::Created,
        binding,
        objects,
    })
}

/// Hydrate one restart-safe local NT catalog solely from the exact receipt and
/// S3 object versions. The retained private-root lease is the only successful
/// return path and must live through runner sealing.
///
/// # Errors
///
/// Returns an error on any receipt/version/ETag/content mismatch, non-private
/// or non-empty local root, unexpected local entry, cap breach, or deadline.
pub async fn hydrate_catalog_projection_from_receipt_guarded(
    store: &dyn ObjectStore,
    artifact_root: &ResolvedArtifactRoot,
    locator: &CatalogProjectionPublicationReceiptLocator,
    expected_physical_manifest: &CatalogProjectionManifestDocument,
    private_local_catalog_root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<HydratedCatalogProjection> {
    let stage = OperatorWorkBudgetStage::ObjectVerification;
    work_budget.check_deadline(stage)?;
    ensure_sha256(
        "catalog publication receipt locator SHA-256",
        &locator.receipt_sha256,
    )?;
    ensure_immutable_s3_version_id(
        "catalog publication receipt locator version ID",
        &locator.receipt_version_id,
    )?;
    ensure!(
        !locator.receipt_e_tag.trim().is_empty(),
        "catalog publication receipt locator ETag must not be empty"
    );
    expected_physical_manifest
        .validate_guarded(work_budget, stage)
        .map_err(anyhow::Error::from)
        .context("validate caller-expected hydration physical manifest")?;
    let expected_physical_manifest_sha256 = expected_physical_manifest
        .manifest_sha256_guarded(work_budget, stage)
        .context("hash caller-expected hydration physical manifest")?;

    let receipt_path = artifact_root.object_path_for_uri(&locator.receipt_uri)?;
    let receipt_get = get_exact_version_guarded(
        store,
        &receipt_path,
        &locator.receipt_version_id,
        &locator.receipt_e_tag,
        work_budget,
        stage,
        "catalog publication receipt",
    )
    .await?;
    let receipt_byte_len = receipt_get.meta.size;
    ensure!(
        receipt_byte_len > 0,
        "catalog publication receipt returned an empty payload"
    );
    enforce_final_object_byte_cap(
        "catalog publication receipt hydration payload",
        receipt_byte_len,
        artifact_root.max_final_object_bytes,
    )?;
    work_budget.verify_decoded_bytes(receipt_byte_len, stage)?;
    validate_versioned_get_metadata(
        &receipt_get,
        &receipt_path,
        receipt_byte_len,
        &locator.receipt_version_id,
        &locator.receipt_e_tag,
        "catalog publication receipt",
    )?;
    let receipt_bytes = collect_exact_get_result_guarded(
        receipt_get,
        receipt_byte_len,
        work_budget,
        stage,
        "catalog publication receipt",
    )
    .await?;
    verify_retained_vec_capacity(
        "hydrated catalog publication receipt",
        receipt_bytes.capacity(),
        work_budget,
        stage,
    )?;
    let receipt =
        CatalogProjectionPublicationReceipt::parse_and_validate_with_input_retained_guarded(
            &receipt_bytes,
            u64::try_from(receipt_bytes.capacity())
                .context("hydrated receipt retained capacity does not fit u64")?,
            &locator.receipt_sha256,
            work_budget,
            stage,
        )?;
    ensure!(
        receipt.physical_manifest == *expected_physical_manifest,
        "catalog publication receipt physical manifest does not exactly match caller authority"
    );
    ensure!(
        receipt.physical_manifest_sha256 == expected_physical_manifest_sha256,
        "catalog publication receipt physical manifest hash does not match caller authority"
    );
    let expected_catalog_root_uri =
        artifact_root.nt_catalog_projection_root(&receipt.binding.catalog_projection_id);
    ensure!(
        receipt.catalog_root_uri == expected_catalog_root_uri,
        "catalog publication receipt root {:?} does not match configured projection root {:?}",
        receipt.catalog_root_uri,
        expected_catalog_root_uri
    );
    let expected_receipt_uri = artifact_root
        .catalog_projection_manifest_object_uri(&receipt.binding.catalog_projection_id);
    ensure!(
        locator.receipt_uri == expected_receipt_uri,
        "catalog publication receipt URI {:?} does not match configured receipt URI {:?}",
        locator.receipt_uri,
        expected_receipt_uri
    );

    let root_lease = PrivateCatalogRootLease::open_empty(private_local_catalog_root)?;
    root_lease.revalidate()?;
    for (receipt_object, physical_object) in receipt
        .objects
        .iter()
        .zip(&expected_physical_manifest.objects)
    {
        work_budget.check_deadline(stage)?;
        root_lease.revalidate()?;
        let expected_uri = format!(
            "{}{}",
            receipt.catalog_root_uri, physical_object.relative_path
        );
        ensure!(
            receipt_object.uri == expected_uri,
            "catalog receipt object URI is not derived from its root and canonical relative path"
        );
        enforce_final_object_byte_cap(
            &format!("hydrated catalog object {}", physical_object.relative_path),
            physical_object.byte_len,
            artifact_root.max_final_object_bytes,
        )?;
        work_budget.verify_decoded_bytes(physical_object.byte_len, stage)?;
        let remote_path = artifact_root.object_path_for_uri(&expected_uri)?;
        let object_get = get_exact_version_guarded(
            store,
            &remote_path,
            &receipt_object.version_id,
            &receipt_object.e_tag,
            work_budget,
            stage,
            &format!("catalog object {}", physical_object.relative_path),
        )
        .await?;
        validate_versioned_get_metadata(
            &object_get,
            &remote_path,
            physical_object.byte_len,
            &receipt_object.version_id,
            &receipt_object.e_tag,
            &format!("catalog object {}", physical_object.relative_path),
        )?;
        let (local_path, local_file) = create_private_hydration_file(
            &root_lease,
            &physical_object.relative_path,
            work_budget,
            stage,
        )?;
        stream_versioned_object_to_local_file_guarded(
            object_get,
            local_file,
            &local_path,
            physical_object.byte_len,
            &physical_object.sha256,
            work_budget,
            stage,
        )
        .await?;
        root_lease.revalidate()?;
    }

    seal_trusted_local_catalog_permissions_guarded(
        private_local_catalog_root,
        expected_physical_manifest,
        work_budget,
        stage,
    )?;
    verify_local_catalog_projection_exact_set_guarded(
        private_local_catalog_root,
        expected_physical_manifest,
        work_budget,
        stage,
    )?;
    root_lease.revalidate()?;
    Ok(HydratedCatalogProjection {
        root_lease,
        catalog_root_uri: receipt.catalog_root_uri,
        binding: receipt.binding,
        physical_manifest_sha256: expected_physical_manifest_sha256,
        receipt_sha256: locator.receipt_sha256.clone(),
        receipt_version_id: locator.receipt_version_id.clone(),
        receipt_e_tag: locator.receipt_e_tag.clone(),
        object_count: receipt.objects.len(),
    })
}

async fn get_exact_version_guarded(
    store: &dyn ObjectStore,
    object_path: &ObjectPath,
    version_id: &str,
    e_tag: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    object_label: &str,
) -> Result<object_store::GetResult> {
    let options = object_store::GetOptions {
        version: Some(version_id.to_string()),
        if_match: Some(e_tag.to_string()),
        ..object_store::GetOptions::default()
    };
    let outcome =
        guarded_async_operation_outcome(work_budget, stage, store.get_opts(object_path, options))
            .await?;
    outcome.with_context(|| format!("get exact version of {object_label} at {object_path}"))
}

fn validate_versioned_get_metadata(
    result: &object_store::GetResult,
    expected_path: &ObjectPath,
    expected_byte_len: u64,
    expected_version_id: &str,
    expected_e_tag: &str,
    object_label: &str,
) -> Result<()> {
    ensure!(
        result.meta.location == *expected_path,
        "{object_label} returned location {} instead of {expected_path}",
        result.meta.location
    );
    ensure!(
        result.meta.size == expected_byte_len,
        "{object_label} returned {} bytes instead of exact expected {expected_byte_len}",
        result.meta.size
    );
    ensure!(
        result.range.start == 0 && result.range.end == expected_byte_len,
        "{object_label} response range {:?} is not exact 0..{expected_byte_len}",
        result.range
    );
    ensure!(
        result.meta.version.as_deref() == Some(expected_version_id),
        "{object_label} returned version {:?} instead of exact receipt version {expected_version_id:?}",
        result.meta.version
    );
    ensure!(
        result.meta.e_tag.as_deref() == Some(expected_e_tag),
        "{object_label} returned ETag {:?} instead of exact receipt ETag {expected_e_tag:?}",
        result.meta.e_tag
    );
    Ok(())
}

async fn collect_exact_get_result_guarded(
    result: object_store::GetResult,
    expected_byte_len: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
    object_label: &str,
) -> Result<Vec<u8>> {
    let mut buffer = ExactSizedObjectBuffer::new(expected_byte_len)?;
    let mut stream = result.into_stream();
    loop {
        let outcome = guarded_async_operation_outcome(work_budget, stage, async {
            stream.next().await.transpose()
        })
        .await?;
        let Some(chunk) = outcome.with_context(|| format!("stream {object_label}"))? else {
            break;
        };
        work_budget.verify_decoded_bytes(
            u64::try_from(chunk.len())
                .with_context(|| format!("{object_label} chunk length does not fit u64"))?,
            stage,
        )?;
        buffer.push(&chunk, work_budget, stage)?;
    }
    buffer.finish(work_budget, stage)
}

#[cfg(unix)]
fn create_private_hydration_file(
    root_lease: &PrivateCatalogRootLease,
    relative_path: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<(PathBuf, fs::File)> {
    work_budget.check_deadline(stage)?;
    let relative_path_bytes = relative_path.as_bytes();
    visit_bytes_cooperatively(relative_path_bytes, work_budget, stage)?;
    let mut maximum_component_bytes = 0_usize;
    let mut component_count = 0_usize;
    for component in relative_path.split('/') {
        let component_bytes = component.as_bytes();
        work_budget.check_deadline(stage)?;
        ensure!(
            !component.is_empty() && component != "." && component != "..",
            "hydration relative path {relative_path:?} is not canonical"
        );
        ensure!(
            !component_bytes.contains(&0),
            "hydration path component contains an interior NUL"
        );
        work_budget.check_deadline(stage)?;
        maximum_component_bytes = maximum_component_bytes.max(component_bytes.len());
        component_count = component_count
            .checked_add(1)
            .context("hydration path component count overflow")?;
    }
    ensure!(
        component_count > 0,
        "hydration relative path must contain a final file component"
    );
    let display_path_capacity = root_lease
        .path
        .as_os_str()
        .as_bytes()
        .len()
        .checked_add(1)
        .and_then(|value| value.checked_add(relative_path_bytes.len()))
        .context("private hydration display path capacity overflow")?;
    let component_c_string_capacity = maximum_component_bytes
        .checked_add(1)
        .context("private hydration component C string capacity overflow")?;
    verify_cumulative_retained_bytes(
        "private hydration display path and component control allocation",
        &[
            u64::try_from(display_path_capacity)
                .context("private hydration display path capacity does not fit u64")?,
            u64::try_from(component_c_string_capacity)
                .context("private hydration component capacity does not fit u64")?,
        ],
        work_budget,
        stage,
    )?;

    let mut display_path = PathBuf::new();
    display_path
        .try_reserve_exact(display_path_capacity)
        .context("reserve private hydration display path")?;
    verify_cumulative_retained_bytes(
        "private hydration retained display path and component control allocation",
        &[
            u64::try_from(display_path.capacity())
                .context("private hydration retained display path does not fit u64")?,
            u64::try_from(component_c_string_capacity)
                .context("private hydration component capacity does not fit u64")?,
        ],
        work_budget,
        stage,
    )?;
    display_path.push(&root_lease.path);
    root_lease.revalidate()?;
    work_budget.check_deadline(stage)?;
    let mut directory = root_lease
        .directory
        .try_clone()
        .context("duplicate private hydration root descriptor")?;
    work_budget.check_deadline(stage)?;
    let mut components = relative_path.split('/').peekable();
    while let Some(component) = components.next() {
        work_budget.check_deadline(stage)?;
        let component_capacity = component
            .len()
            .checked_add(1)
            .context("hydration component C string capacity overflow")?;
        let mut component_bytes = Vec::new();
        component_bytes
            .try_reserve_exact(component_capacity)
            .context("reserve hydration component C string")?;
        component_bytes.extend_from_slice(component.as_bytes());
        work_budget.check_deadline(stage)?;
        // SAFETY: the complete preflight above rejected every interior NUL;
        // capacity also includes the terminator added by CString.
        let component_c = unsafe { std::ffi::CString::from_vec_unchecked(component_bytes) };
        display_path.push(component);
        if components.peek().is_some() {
            let next_directory = match openat_hydration_directory(&directory, &component_c) {
                Ok(directory) => directory,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // SAFETY: `directory` is a retained directory descriptor and
                    // `component_c` is one validated NUL-free path component.
                    let created = unsafe {
                        libc::mkdirat(directory.as_raw_fd(), component_c.as_ptr(), 0o700)
                    };
                    if created != 0 {
                        let create_error = std::io::Error::last_os_error();
                        if create_error.kind() != std::io::ErrorKind::AlreadyExists {
                            return Err(create_error).with_context(|| {
                                format!(
                                    "mkdirat private hydration directory {}",
                                    display_path.display()
                                )
                            });
                        }
                    }
                    openat_hydration_directory(&directory, &component_c).with_context(|| {
                        format!(
                            "openat newly created hydration directory {}",
                            display_path.display()
                        )
                    })?
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "openat hydration directory {} without following symlinks",
                            display_path.display()
                        )
                    });
                }
            };
            let metadata = next_directory
                .metadata()
                .with_context(|| format!("fstat hydration directory {}", display_path.display()))?;
            ensure!(
                metadata.file_type().is_dir(),
                "hydration path component {} is not a directory",
                display_path.display()
            );
            ensure!(
                metadata.permissions().mode() & 0o777 == 0o700,
                "hydration directory {} must have mode 0700",
                display_path.display()
            );
            directory = next_directory;
            root_lease.revalidate()?;
        } else {
            // SAFETY: `directory` is a retained directory descriptor,
            // `component_c` is one validated NUL-free final component, and the
            // returned descriptor is immediately owned by `File` on success.
            let raw_fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    component_c.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if raw_fd < 0 {
                return Err(std::io::Error::last_os_error()).with_context(|| {
                    format!(
                        "openat create-new hydration object {}",
                        display_path.display()
                    )
                });
            }
            // SAFETY: successful `openat` returned one new owned descriptor.
            let file = unsafe { fs::File::from_raw_fd(raw_fd) };
            let metadata = file.metadata().with_context(|| {
                format!("fstat new hydration object {}", display_path.display())
            })?;
            ensure!(
                metadata.file_type().is_file(),
                "new hydration object {} is not a regular file",
                display_path.display()
            );
            ensure!(
                metadata.permissions().mode() & 0o777 == 0o600,
                "new hydration object {} must have mode 0600",
                display_path.display()
            );
            root_lease.revalidate()?;
            return Ok((display_path, file));
        }
    }
    bail!("hydration relative path must contain a final file component")
}

#[cfg(unix)]
fn openat_hydration_directory(
    parent: &fs::File,
    component: &std::ffi::CStr,
) -> std::io::Result<fs::File> {
    // SAFETY: `parent` remains a live directory descriptor for this call and
    // `component` is one NUL-terminated relative component. O_NOFOLLOW and
    // O_DIRECTORY reject both symlink substitution and non-directory entries.
    let raw_fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            component.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if raw_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful `openat` returned one new owned descriptor.
    Ok(unsafe { fs::File::from_raw_fd(raw_fd) })
}

#[cfg(not(unix))]
fn create_private_hydration_file(
    _root_lease: &PrivateCatalogRootLease,
    _relative_path: &str,
    _work_budget: &OperatorWorkBudgetGuard,
    _stage: OperatorWorkBudgetStage,
) -> Result<(PathBuf, fs::File)> {
    bail!("race-free catalog hydration requires Unix fd-relative filesystem operations")
}

async fn stream_versioned_object_to_local_file_guarded(
    result: object_store::GetResult,
    file: fs::File,
    local_path: &Path,
    expected_byte_len: u64,
    expected_sha256: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let mut file = tokio::fs::File::from_std(file);
    let mut stream = result.into_stream();
    let mut observed_byte_len = 0_u64;
    let mut hasher = Sha256::new();
    loop {
        let outcome = guarded_async_operation_outcome(work_budget, stage, async {
            stream.next().await.transpose()
        })
        .await?;
        let Some(chunk) = outcome
            .with_context(|| format!("stream catalog object into {}", local_path.display()))?
        else {
            break;
        };
        work_budget.verify_decoded_bytes(
            u64::try_from(chunk.len())
                .context("catalog hydration chunk length does not fit u64")?,
            stage,
        )?;
        work_budget.check_deadline(stage)?;
        observed_byte_len = observed_byte_len
            .checked_add(
                u64::try_from(chunk.len())
                    .context("catalog hydration write length does not fit u64")?,
            )
            .context("catalog hydration observed byte length overflow")?;
        ensure!(
            observed_byte_len <= expected_byte_len,
            "hydrated catalog object {} exceeded exact expected byte length {expected_byte_len}",
            local_path.display()
        );
        file.write_all(&chunk)
            .await
            .with_context(|| format!("write hydration object {}", local_path.display()))?;
        hasher.update(&chunk);
        work_budget.check_deadline(stage)?;
    }
    ensure!(
        observed_byte_len == expected_byte_len,
        "hydrated catalog object {} has {observed_byte_len} bytes instead of exact expected {expected_byte_len}",
        local_path.display()
    );
    file.flush()
        .await
        .with_context(|| format!("flush hydration object {}", local_path.display()))?;
    file.sync_all()
        .await
        .with_context(|| format!("sync hydration object {}", local_path.display()))?;
    let metadata = file
        .metadata()
        .await
        .with_context(|| format!("fstat hydrated object {}", local_path.display()))?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() == expected_byte_len,
        "hydrated catalog object {} changed type or length while writing",
        local_path.display()
    );
    let actual_sha256 = sha256_digest_hex_guarded(hasher.finalize(), work_budget, stage)?;
    ensure!(
        actual_sha256 == expected_sha256,
        "hydrated catalog object {} SHA-256 mismatch: expected {expected_sha256}, got {actual_sha256}",
        local_path.display()
    );
    work_budget.check_deadline(stage)
}

fn sha256_digest_hex_guarded(
    digest: impl AsRef<[u8]>,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    work_budget.check_deadline(stage)?;
    let bytes = digest.as_ref();
    let capacity = bytes
        .len()
        .checked_mul(2)
        .context("SHA-256 hex capacity overflow")?;
    let mut output = String::new();
    output
        .try_reserve_exact(capacity)
        .context("reserve SHA-256 hex output")?;
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    work_budget.check_deadline(stage)?;
    Ok(output)
}

pub(crate) fn required_versioned_create_result(
    version: UpdateVersion,
    object_label: &str,
) -> Result<(String, String)> {
    let version_id = version.version.with_context(|| {
        format!("{object_label} did not return an S3 version ID; versioned publication is required")
    })?;
    ensure_immutable_s3_version_id(&format!("{object_label} S3 version ID"), &version_id)?;
    let e_tag = version
        .e_tag
        .filter(|value| !value.trim().is_empty())
        .with_context(|| {
            format!("{object_label} did not return a nonempty ETag; conditional exact reads are required")
        })?;
    Ok((version_id, e_tag))
}

pub(crate) fn ensure_immutable_s3_version_id(label: &str, version_id: &str) -> Result<()> {
    ensure!(!version_id.is_empty(), "{label} must not be empty");
    ensure!(
        version_id != "null",
        "{label} is the S3 null version; bucket versioning must remain Enabled"
    );
    Ok(())
}

struct BudgetJsonLengthWriter<'a> {
    bytes_written: u64,
    work_budget: &'a OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
}

impl Write for BudgetJsonLengthWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        self.bytes_written = self
            .bytes_written
            .checked_add(
                u64::try_from(buffer.len())
                    .map_err(|_| std::io::Error::other("JSON write length does not fit u64"))?,
            )
            .ok_or_else(|| std::io::Error::other("JSON serialized length overflow"))?;
        self.work_budget
            .verify_decoded_bytes(self.bytes_written, self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.work_budget
            .check_deadline(self.stage)
            .map_err(|error| std::io::Error::other(format!("{error:#}")))
    }
}

fn serialized_json_len_guarded<T: Serialize>(
    value: &T,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<u64> {
    work_budget.check_deadline(stage)?;
    let mut writer = BudgetJsonLengthWriter {
        bytes_written: 0,
        work_budget,
        stage,
    };
    serde_json::to_writer(&mut writer, value).context("measure guarded canonical JSON")?;
    writer.flush().context("flush guarded JSON length writer")?;
    work_budget.check_deadline(stage)?;
    Ok(writer.bytes_written)
}

fn visit_bytes_cooperatively(
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    work_budget.verify_decoded_bytes(
        u64::try_from(bytes.len()).context("byte slice length does not fit u64")?,
        stage,
    )?;
    work_budget.check_deadline(stage)?;
    work_budget.check_deadline(stage)
}

fn conservative_parsed_receipt_retained_upper_bound(
    bytes: &[u8],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<u64> {
    // The strict receipt has six header strings, two strings per physical
    // manifest object, and five strings per publication object. One
    // next-power-of-two wire allowance per field family covers decoded string
    // bytes plus geometric String growth. Each JSON array element consumes at
    // least one wire byte, so the same rounded wire count bounds the logical
    // capacity of both Vecs. This intentionally derives the pre-parse bound
    // from the wire shape and concrete Rust record sizes instead of applying an
    // unexplained scalar multiplier.
    const HEADER_STRING_FIELD_FAMILIES: u64 = 6;
    const PHYSICAL_OBJECT_STRING_FIELD_FAMILIES: u64 = 2;
    const PUBLICATION_OBJECT_STRING_FIELD_FAMILIES: u64 = 5;

    visit_bytes_cooperatively(bytes, work_budget, stage)?;
    let wire_bytes = u64::try_from(bytes.len())
        .context("catalog publication receipt wire length does not fit u64")?;
    let rounded_wire_slots = wire_bytes
        .max(1)
        .checked_next_power_of_two()
        .context("catalog publication receipt rounded wire-slot bound overflow")?;
    let vector_slot_bytes = u64::try_from(
        std::mem::size_of::<crate::run_manifest::CatalogProjectionManifestObject>()
            .checked_add(std::mem::size_of::<CatalogProjectionPublicationObject>())
            .context("catalog publication receipt vector slot size overflow")?,
    )
    .context("catalog publication receipt vector slot size does not fit u64")?;
    let vector_storage = rounded_wire_slots
        .checked_mul(vector_slot_bytes)
        .context("catalog publication receipt vector storage bound overflow")?;
    let string_field_families = HEADER_STRING_FIELD_FAMILIES
        .checked_add(PHYSICAL_OBJECT_STRING_FIELD_FAMILIES)
        .and_then(|value| value.checked_add(PUBLICATION_OBJECT_STRING_FIELD_FAMILIES))
        .context("catalog publication receipt string-field family count overflow")?;
    let string_storage = rounded_wire_slots
        .checked_mul(string_field_families)
        .context("catalog publication receipt string storage bound overflow")?;
    u64::try_from(std::mem::size_of::<CatalogProjectionPublicationReceipt>())
        .context("catalog publication receipt root size does not fit u64")?
        .checked_add(vector_storage)
        .and_then(|value| value.checked_add(string_storage))
        .context("catalog publication receipt parsed-memory bound overflow")
}

fn clone_string_guarded(
    value: &str,
    allocation_label: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    visit_bytes_cooperatively(value.as_bytes(), work_budget, stage)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(value.len())
        .with_context(|| format!("reserve {allocation_label}"))?;
    bytes.extend_from_slice(value.as_bytes());
    work_budget.check_deadline(stage)?;
    // SAFETY: every byte came, in order, from the already-valid UTF-8 `value`.
    Ok(unsafe { String::from_utf8_unchecked(bytes) })
}

fn catalog_object_uri_guarded(
    catalog_root_uri: &str,
    relative_path: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<String> {
    let root = catalog_root_uri.trim_end_matches('/');
    visit_bytes_cooperatively(root.as_bytes(), work_budget, stage)?;
    visit_bytes_cooperatively(relative_path.as_bytes(), work_budget, stage)?;
    let uri_len = root
        .len()
        .checked_add(1)
        .and_then(|value| value.checked_add(relative_path.len()))
        .context("catalog object URI length overflow")?;
    work_budget.verify_decoded_bytes(
        u64::try_from(uri_len).context("catalog object URI length does not fit u64")?,
        stage,
    )?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(uri_len)
        .context("reserve catalog object URI")?;
    for part in [root.as_bytes(), b"/", relative_path.as_bytes()] {
        work_budget.check_deadline(stage)?;
        bytes.extend_from_slice(part);
    }
    work_budget.check_deadline(stage)?;
    // SAFETY: the root and relative path are valid UTF-8 and the separator is
    // ASCII, so their concatenation is valid UTF-8.
    Ok(unsafe { String::from_utf8_unchecked(bytes) })
}

fn string_capacity_bytes(value: &String, allocation_label: &str) -> Result<u64> {
    u64::try_from(value.capacity())
        .with_context(|| format!("{allocation_label} string capacity does not fit u64"))
}

fn catalog_projection_manifest_retained_bytes(
    manifest: &CatalogProjectionManifestDocument,
) -> Result<u64> {
    let mut retained = u64::try_from(std::mem::size_of::<CatalogProjectionManifestDocument>())
        .context("catalog physical manifest root size does not fit u64")?;
    retained = retained
        .checked_add(string_capacity_bytes(
            &manifest.schema_version,
            "catalog physical manifest schema_version",
        )?)
        .context("catalog physical manifest retained byte total overflow")?;
    retained = retained
        .checked_add(
            u64::try_from(
                manifest
                    .objects
                    .capacity()
                    .checked_mul(std::mem::size_of::<
                        crate::run_manifest::CatalogProjectionManifestObject,
                    >())
                    .context("catalog physical manifest vector capacity overflow")?,
            )
            .context("catalog physical manifest vector bytes do not fit u64")?,
        )
        .context("catalog physical manifest retained byte total overflow")?;
    for object in &manifest.objects {
        retained = retained
            .checked_add(string_capacity_bytes(
                &object.relative_path,
                "catalog physical manifest relative_path",
            )?)
            .context("catalog physical manifest retained byte total overflow")?;
        retained = retained
            .checked_add(string_capacity_bytes(
                &object.sha256,
                "catalog physical manifest SHA-256",
            )?)
            .context("catalog physical manifest retained byte total overflow")?;
    }
    Ok(retained)
}

fn persisted_catalog_projection_objects_retained_bytes(
    objects: &Vec<PersistedCatalogProjectionObject>,
) -> Result<u64> {
    let mut retained = u64::try_from(
        objects
            .capacity()
            .checked_mul(std::mem::size_of::<PersistedCatalogProjectionObject>())
            .context("persisted catalog object vector capacity overflow")?,
    )
    .context("persisted catalog object vector bytes do not fit u64")?;
    for object in objects {
        for (value, label) in [
            (&object.relative_path, "persisted catalog relative_path"),
            (&object.uri, "persisted catalog URI"),
            (&object.sha256, "persisted catalog SHA-256"),
            (&object.version_id, "persisted catalog version ID"),
        ] {
            retained = retained
                .checked_add(string_capacity_bytes(value, label)?)
                .context("persisted catalog object retained byte total overflow")?;
        }
        retained = retained
            .checked_add(string_capacity_bytes(
                &object.e_tag,
                "persisted catalog ETag",
            )?)
            .context("persisted catalog object retained byte total overflow")?;
    }
    Ok(retained)
}

fn catalog_publication_header_retained_bytes(
    binding: &CatalogProjectionBinding,
    catalog_root_uri: &String,
    physical_manifest_sha256: &String,
) -> Result<u64> {
    let mut retained = u64::try_from(std::mem::size_of::<CatalogProjectionBinding>())
        .context("catalog publication binding size does not fit u64")?;
    for (value, label) in [
        (
            &binding.source_binding,
            "catalog publication source binding",
        ),
        (
            &binding.catalog_projection_id,
            "catalog publication projection ID",
        ),
        (catalog_root_uri, "catalog publication root URI"),
        (
            physical_manifest_sha256,
            "catalog publication physical manifest SHA-256",
        ),
    ] {
        retained = retained
            .checked_add(string_capacity_bytes(value, label)?)
            .context("catalog publication header retained byte total overflow")?;
    }
    Ok(retained)
}

fn catalog_publication_retained_metadata_bytes(
    expected_physical_manifest: &CatalogProjectionManifestDocument,
    binding: &CatalogProjectionBinding,
    catalog_root_uri: &String,
    physical_manifest_sha256: &String,
    objects: &Vec<PersistedCatalogProjectionObject>,
) -> Result<u64> {
    let retained = catalog_projection_manifest_retained_bytes(expected_physical_manifest)?
        .checked_add(catalog_publication_header_retained_bytes(
            binding,
            catalog_root_uri,
            physical_manifest_sha256,
        )?)
        .context("catalog publication retained metadata byte total overflow")?;
    retained
        .checked_add(persisted_catalog_projection_objects_retained_bytes(
            objects,
        )?)
        .context("catalog publication retained metadata byte total overflow")
}

fn preflight_catalog_publication_retained_peak(
    expected_physical_manifest: &CatalogProjectionManifestDocument,
    binding: &CatalogProjectionBinding,
    catalog_root_uri: &String,
    physical_manifest_sha256: &String,
    max_final_object_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<u64> {
    let manifest_retained = catalog_projection_manifest_retained_bytes(expected_physical_manifest)?;
    let header_retained = catalog_publication_header_retained_bytes(
        binding,
        catalog_root_uri,
        physical_manifest_sha256,
    )?;
    let mut planned_objects_retained = u64::try_from(
        expected_physical_manifest
            .objects
            .len()
            .checked_mul(std::mem::size_of::<PersistedCatalogProjectionObject>())
            .context("planned persisted catalog object vector size overflow")?,
    )
    .context("planned persisted catalog object vector bytes do not fit u64")?;
    let root_len = catalog_root_uri.trim_end_matches('/').len();
    let mut maximum_payload_bytes = 0_u64;
    for object in &expected_physical_manifest.objects {
        work_budget.check_deadline(stage)?;
        enforce_final_object_byte_cap(
            "catalog projection physical-manifest object",
            object.byte_len,
            max_final_object_bytes,
        )
        .with_context(|| format!("validate catalog projection file {}", object.relative_path))?;
        visit_bytes_cooperatively(object.relative_path.as_bytes(), work_budget, stage)?;
        visit_bytes_cooperatively(object.sha256.as_bytes(), work_budget, stage)?;
        let uri_len = root_len
            .checked_add(1)
            .and_then(|value| value.checked_add(object.relative_path.len()))
            .context("planned catalog object URI length overflow")?;
        let owned_string_bytes = object
            .relative_path
            .len()
            .checked_add(object.sha256.len())
            .and_then(|value| value.checked_add(uri_len))
            .context("planned persisted catalog object string bytes overflow")?;
        planned_objects_retained = planned_objects_retained
            .checked_add(
                u64::try_from(owned_string_bytes)
                    .context("planned persisted catalog object strings do not fit u64")?,
            )
            .context("planned persisted catalog object retained byte total overflow")?;
        maximum_payload_bytes = maximum_payload_bytes.max(object.byte_len);
    }
    let planned_peak = manifest_retained
        .checked_add(header_retained)
        .and_then(|value| value.checked_add(planned_objects_retained))
        .and_then(|value| value.checked_add(maximum_payload_bytes))
        .context("catalog publication planned retained peak overflow")?;
    verify_cumulative_retained_bytes(
        "catalog publication manifest, output metadata, and sequential single-PUT payload",
        &[planned_peak],
        work_budget,
        stage,
    )?;
    Ok(planned_peak)
}

fn verify_retained_vec_capacity(
    allocation_label: &str,
    retained_capacity: usize,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let retained_bytes = u64::try_from(retained_capacity)
        .with_context(|| format!("{allocation_label} retained capacity does not fit u64"))?;
    work_budget
        .verify_decoded_bytes(retained_bytes, stage)
        .with_context(|| {
            format!(
                "{allocation_label} retained capacity {retained_bytes} exceeds the work-budget memory envelope"
            )
        })
}

fn verify_cumulative_retained_bytes(
    allocation_label: &str,
    retained_components: &[u64],
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let mut retained_total = 0_u64;
    for component in retained_components {
        retained_total = retained_total
            .checked_add(*component)
            .with_context(|| format!("{allocation_label} retained byte total overflow"))?;
    }
    work_budget
        .verify_decoded_bytes(retained_total, stage)
        .with_context(|| {
            format!(
                "{allocation_label} cumulative retained memory {retained_total} exceeds the work-budget memory envelope"
            )
        })
}

fn verify_local_catalog_projection_exact_set_guarded(
    local_catalog_root: &Path,
    expected_physical_manifest: &CatalogProjectionManifestDocument,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let root_metadata = fs::symlink_metadata(local_catalog_root).with_context(|| {
        format!(
            "inspect local catalog projection root {}",
            local_catalog_root.display()
        )
    })?;
    ensure!(
        root_metadata.file_type().is_dir(),
        "local catalog projection root {} must remain a real directory",
        local_catalog_root.display()
    );

    let mut expected_directories = BTreeSet::new();
    for object in &expected_physical_manifest.objects {
        work_budget.check_deadline(stage)?;
        for (index, byte) in object.relative_path.bytes().enumerate() {
            if byte == b'/' {
                expected_directories.insert(&object.relative_path[..index]);
            }
        }
    }

    let stack_capacity = expected_directories
        .len()
        .checked_add(1)
        .context("catalog exact-set directory count overflow")?;
    let mut stack = Vec::new();
    stack
        .try_reserve_exact(stack_capacity)
        .context("reserve catalog exact-set directory stack")?;
    stack.push(local_catalog_root.to_path_buf());
    let mut seen_objects = Vec::new();
    seen_objects
        .try_reserve_exact(expected_physical_manifest.objects.len())
        .context("reserve catalog exact-set object bitmap")?;
    seen_objects.resize(expected_physical_manifest.objects.len(), false);

    while let Some(directory) = stack.pop() {
        work_budget.check_deadline(stage)?;
        let entries = fs::read_dir(&directory)
            .with_context(|| format!("read catalog directory {}", directory.display()))?;
        for entry in entries {
            work_budget.check_deadline(stage)?;
            let entry = entry.with_context(|| {
                format!("read catalog directory entry under {}", directory.display())
            })?;
            let path = entry.path();
            let relative_path = path.strip_prefix(local_catalog_root).with_context(|| {
                format!(
                    "derive catalog path relative to {}",
                    local_catalog_root.display()
                )
            })?;
            let relative_key = catalog_relative_path_key(relative_path)?;
            let metadata = fs::symlink_metadata(&path)
                .with_context(|| format!("inspect catalog entry {}", path.display()))?;
            if metadata.file_type().is_dir() {
                ensure!(
                    expected_directories.contains(relative_key.as_str()),
                    "local catalog projection contains unexpected directory {relative_key}"
                );
                stack.push(path);
            } else if metadata.file_type().is_file() {
                let object_index = expected_physical_manifest
                    .objects
                    .binary_search_by(|object| {
                        object.relative_path.as_str().cmp(relative_key.as_str())
                    })
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "local catalog projection contains unexpected object {relative_key}"
                        )
                    })?;
                ensure!(
                    !seen_objects[object_index],
                    "local catalog projection contains duplicate object {relative_key}"
                );
                let expected_object = &expected_physical_manifest.objects[object_index];
                ensure!(
                    metadata.len() == expected_object.byte_len,
                    "local catalog object {relative_key} byte length {} does not match producer-minted physical manifest {}",
                    metadata.len(),
                    expected_object.byte_len
                );
                seen_objects[object_index] = true;
            } else {
                bail!("local catalog projection contains non-regular entry {relative_key}");
            }
        }
    }

    if let Some((index, _)) = seen_objects.iter().enumerate().find(|(_, seen)| !**seen) {
        bail!(
            "local catalog projection is missing producer-authorized object {}",
            expected_physical_manifest.objects[index].relative_path
        );
    }
    work_budget.check_deadline(stage)
}

fn contained_catalog_object_path(
    local_catalog_root: &Path,
    relative_path: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<PathBuf> {
    let root_metadata = fs::symlink_metadata(local_catalog_root).with_context(|| {
        format!(
            "inspect local catalog projection root {}",
            local_catalog_root.display()
        )
    })?;
    ensure!(
        root_metadata.file_type().is_dir(),
        "local catalog projection root {} must remain a real directory",
        local_catalog_root.display()
    );
    let mut path = local_catalog_root.to_path_buf();
    let mut components = relative_path.split('/').peekable();
    while let Some(component) = components.next() {
        work_budget.check_deadline(stage)?;
        ensure!(
            !component.is_empty() && component != "." && component != "..",
            "catalog object path {relative_path:?} is not a normalized relative path"
        );
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("inspect catalog path component {}", path.display()))?;
        if components.peek().is_some() {
            ensure!(
                metadata.file_type().is_dir(),
                "catalog object path component {} must be a real directory",
                path.display()
            );
        } else {
            ensure!(
                metadata.file_type().is_file(),
                "catalog object {} must be a real regular file",
                path.display()
            );
        }
    }
    let canonical_root = fs::canonicalize(local_catalog_root).with_context(|| {
        format!(
            "canonicalize local catalog projection root {}",
            local_catalog_root.display()
        )
    })?;
    let canonical_path = fs::canonicalize(&path)
        .with_context(|| format!("canonicalize catalog object {}", path.display()))?;
    ensure!(
        canonical_path.starts_with(&canonical_root),
        "catalog object {} resolves outside local catalog root {}",
        path.display(),
        local_catalog_root.display()
    );
    work_budget.check_deadline(stage)?;
    Ok(path)
}

fn catalog_relative_path_key(path: &Path) -> Result<String> {
    let mut key = String::new();
    key.try_reserve(path.as_os_str().len())
        .context("reserve catalog relative path key")?;
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            bail!("catalog object path must be relative: {}", path.display());
        };
        let component = component.to_str().with_context(|| {
            format!("catalog object path is not valid UTF-8: {}", path.display())
        })?;
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(component);
    }
    ensure!(
        !key.is_empty(),
        "catalog object path must not be empty for {}",
        path.display()
    );
    Ok(key)
}

pub struct CreateOnlyArtifactWriter<'a> {
    store: &'a dyn ObjectStore,
    artifact_root: ResolvedArtifactRoot,
}

impl<'a> CreateOnlyArtifactWriter<'a> {
    #[must_use]
    pub fn new(store: &'a dyn ObjectStore, artifact_root: &ResolvedArtifactRoot) -> Self {
        Self {
            store,
            artifact_root: artifact_root.clone(),
        }
    }

    fn ensure_bound_artifact_root(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        ensure!(
            self.artifact_root == *artifact_root,
            "create-only writer is not bound to the supplied artifact root or its terminal commit policy: writer root {:?}, supplied root {:?}",
            self.artifact_root.artifact_root_uri(),
            artifact_root.artifact_root_uri(),
        );
        Ok(())
    }

    fn enforce_payload_cap(&self, object_label: &str, payload: &[u8]) -> Result<()> {
        let payload_bytes = u64::try_from(payload.len())
            .with_context(|| format!("{object_label} byte length does not fit u64"))?;
        enforce_final_object_byte_cap(
            object_label,
            payload_bytes,
            self.artifact_root.max_final_object_bytes(),
        )
    }

    /// Freeze a terminal create key and exact payload before commit authority
    /// is consumed. No remote operation occurs here.
    fn prepare_terminal_create(
        &self,
        path: &ObjectPath,
        payload: Vec<u8>,
        object_label: impl Into<String>,
    ) -> Result<PreparedTerminalCreate> {
        let object_label = object_label.into();
        ensure!(
            !object_label.trim().is_empty(),
            "terminal create object label must not be empty"
        );
        self.enforce_payload_cap(&object_label, &payload)?;
        Ok(PreparedTerminalCreate {
            path: path.clone(),
            payload: Bytes::from(payload),
            object_label,
        })
    }

    /// Bind one terminal key to the same resolved TOML root which constructed
    /// this writer. This is the sole terminal-preparation entry point exposed
    /// to other modules; raw object paths cannot bypass the root/URI check.
    pub(crate) fn prepare_terminal_create_uri(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        uri: &str,
        payload: Vec<u8>,
        object_label: impl Into<String>,
    ) -> Result<PreparedTerminalCreate> {
        self.ensure_bound_artifact_root(artifact_root)?;
        let path = artifact_root.object_path_for_uri(uri)?;
        self.prepare_terminal_create(&path, payload, object_label)
    }

    /// Issue exactly one create-only terminal mutation.
    ///
    /// Success requires a complete immutable identity in the direct create
    /// response. An occupied key is a conflict, and any other failed or
    /// incomplete acknowledgement is indeterminate. This write path never
    /// discovers or reuses a current object.
    pub(crate) async fn create_terminal_strict(
        &self,
        prepared: PreparedTerminalCreate,
        permit: OperatorWorkBudgetCommitPermit,
    ) -> Result<CreatedTerminalObject> {
        ensure!(
            permit.stage() == OperatorWorkBudgetStage::Publish,
            "terminal object create requires a publish-stage commit permit"
        );
        let configured_timeout =
            std::time::Duration::from_secs(self.artifact_root.s3.terminal_commit_timeout_seconds);
        let remaining_wall_time = permit
            .remaining_wall_time_at_consumption()?
            .map_or(configured_timeout, |remaining| {
                remaining.min(configured_timeout)
            });
        match tokio::time::timeout(
            remaining_wall_time,
            self.create_terminal_strict_inner(&prepared),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => Err(TerminalCreateIndeterminate {
                detail: format!(
                    "{} at {}: create acknowledgement did not finish within the effective terminal timeout {remaining_wall_time:?}; the request may have committed",
                    prepared.object_label, prepared.path
                ),
            }
            .into()),
        }
    }

    async fn create_terminal_strict_inner(
        &self,
        prepared: &PreparedTerminalCreate,
    ) -> Result<CreatedTerminalObject> {
        match self
            .store
            .put_opts(
                &prepared.path,
                prepared.payload.clone().into(),
                PutMode::Create.into(),
            )
            .await
        {
            Ok(result) => {
                let version: UpdateVersion = result.into();
                match required_versioned_create_result(version, &prepared.object_label) {
                    Ok((version_id, e_tag)) => Ok(CreatedTerminalObject { version_id, e_tag }),
                    Err(error) => Err(TerminalCreateIndeterminate {
                        detail: format!(
                            "{} at {}: create succeeded with an unusable immutable identity ({error:#})",
                            prepared.object_label, prepared.path
                        ),
                    }
                    .into()),
                }
            }
            Err(error) if is_object_store_create_only_conflict(&error) => Err(error)
                .with_context(|| {
                    format!(
                        "{} conflicts with an occupied terminal key",
                        prepared.object_label
                    )
                }),
            Err(error) => Err(TerminalCreateIndeterminate {
                detail: format!(
                    "{} at {}: create request failed and may have committed ({error})",
                    prepared.object_label, prepared.path
                ),
            }
            .into()),
        }
    }

    /// # Errors
    ///
    /// Returns an error if the object already exists or the object store rejects
    /// create-only semantics.
    async fn put_create(&self, path: &ObjectPath, payload: Vec<u8>) -> Result<()> {
        self.enforce_payload_cap(&format!("create-only object {path}"), &payload)?;
        self.store
            .put_opts(path, payload.into(), PutMode::Create.into())
            .await
            .with_context(|| format!("create-only put {path}"))?;
        Ok(())
    }

    /// Perform one strict create-only publication and accept only the direct
    /// response's complete immutable identity. No occupied object is read or
    /// reused, even when it contains identical bytes.
    pub(crate) async fn put_create_strict_guarded(
        &self,
        path: &ObjectPath,
        payload: Vec<u8>,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<CreatedTerminalObject> {
        let object_label = format!("strict create-only object {path}");
        self.enforce_payload_cap(&object_label, &payload)?;
        let put_outcome = guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            self.store
                .put_opts(path, payload.into(), PutMode::Create.into()),
        )
        .await
        .map_err(|error| TerminalCreateIndeterminate {
            detail: format!("{object_label}: create outcome is unknown ({error:#})"),
        })?;
        match put_outcome {
            Ok(result) => {
                let version: UpdateVersion = result.into();
                let (version_id, e_tag) = required_versioned_create_result(version, &object_label)
                    .map_err(|error| TerminalCreateIndeterminate {
                        detail: format!(
                            "{object_label}: create succeeded with an unusable immutable identity ({error:#})"
                        ),
                    })?;
                Ok(CreatedTerminalObject { version_id, e_tag })
            }
            Err(error) if is_object_store_create_only_conflict(&error) => Err(error)
                .with_context(|| format!("{object_label} conflicts with an occupied key")),
            Err(error) => Err(TerminalCreateIndeterminate {
                detail: format!("{object_label}: create request failed and may have committed ({error})"),
            }
            .into()),
        }
    }

    /// Execute the create-only capability probe under the shared operator
    /// deadline. Every remote create/copy/read is fenced independently.
    ///
    /// # Errors
    ///
    /// Returns an error if the probe object cannot be created exactly once, a
    /// duplicate create/copy is accepted, or the explicit budget expires.
    pub async fn probe_create_only_guarded(
        &self,
        artifact_root: &ResolvedArtifactRoot,
        probe_id: &str,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<CreateOnlyProbeTranscript> {
        self.ensure_bound_artifact_root(artifact_root)?;
        let (probe_uri, path, payload) = guarded_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            || -> Result<_> {
                ensure_path_token("create_only_probe_id", probe_id, PathTokenMode::AllowEquals)?;
                let probe_uri = artifact_root.create_only_probe_uri(probe_id);
                let path = artifact_root.object_path_for_uri(&probe_uri)?;
                Ok((probe_uri, path, probe_id.as_bytes().to_vec()))
            },
        )??;
        self.put_create_strict_guarded(&path, payload.clone(), work_budget)
            .await
            .with_context(|| format!("create-only probe setup write {probe_uri}"))?;

        match guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            async { self.put_create(&path, payload.clone()).await },
        )
        .await?
        {
            Ok(()) => bail!("create-only probe accepted duplicate write to {probe_uri}"),
            Err(err) if is_create_only_conflict(&err) => {
                self.verify_existing_probe_payload(
                    &path,
                    &payload,
                    Some(work_budget),
                    &format!("create-only probe object {probe_uri}"),
                )
                .await?;
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("create-only probe duplicate write failed unexpectedly for {probe_uri}")
                });
            }
        }

        let (copy_source_uri, copy_dest_uri, copy_source_path, copy_dest_path) =
            guarded_operation_outcome(
                work_budget,
                OperatorWorkBudgetStage::Publish,
                || -> Result<_> {
                    let copy_source_uri = artifact_root.create_only_probe_copy_source_uri(probe_id);
                    let copy_dest_uri = artifact_root.create_only_probe_copy_dest_uri(probe_id);
                    let copy_source_path = artifact_root.object_path_for_uri(&copy_source_uri)?;
                    let copy_dest_path = artifact_root.object_path_for_uri(&copy_dest_uri)?;
                    Ok((
                        copy_source_uri,
                        copy_dest_uri,
                        copy_source_path,
                        copy_dest_path,
                    ))
                },
            )??;
        self.put_create_strict_guarded(&copy_source_path, payload.clone(), work_budget)
            .await
            .with_context(|| format!("create-only probe copy source setup {copy_source_uri}"))?;
        self.copy_if_not_exists_strict(
            &copy_source_path,
            &copy_dest_path,
            payload.as_slice(),
            &copy_source_uri,
            &copy_dest_uri,
            work_budget,
        )
        .await?;

        match guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            self.store
                .copy_if_not_exists(&copy_source_path, &copy_dest_path),
        )
        .await?
        {
            Ok(()) => bail!("create-only probe accepted duplicate copy to {copy_dest_uri}"),
            Err(err) if is_object_store_create_only_conflict(&err) => {
                self.verify_existing_probe_payload(
                    &copy_dest_path,
                    &payload,
                    Some(work_budget),
                    &format!("create-only probe copy destination {copy_dest_uri}"),
                )
                .await?;
                Ok(CreateOnlyProbeTranscript {
                    probe_uri,
                    copy_source_uri,
                    copy_dest_uri,
                    first_create_succeeded: true,
                    duplicate_create_rejected: true,
                    first_copy_succeeded: true,
                    duplicate_copy_rejected: true,
                })
            }
            Err(err) => Err(err).with_context(|| {
                format!(
                    "create-only probe duplicate copy-if-not-exists failed unexpectedly for {copy_dest_uri}"
                )
            }),
        }
    }

    async fn copy_if_not_exists_strict(
        &self,
        copy_source_path: &ObjectPath,
        copy_dest_path: &ObjectPath,
        expected_payload: &[u8],
        copy_source_uri: &str,
        copy_dest_uri: &str,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<()> {
        self.enforce_payload_cap(
            &format!("create-only copy destination {copy_dest_uri}"),
            expected_payload,
        )?;
        guarded_async_operation_outcome(
            work_budget,
            OperatorWorkBudgetStage::Publish,
            self.store
                .copy_if_not_exists(copy_source_path, copy_dest_path),
        )
        .await?
        .with_context(|| {
            format!(
                "strict create-only probe copy setup {copy_source_uri} -> {copy_dest_uri}"
            )
        })
    }

    /// # Errors
    ///
    /// Returns an error if the object already exists or the object store
    /// rejects create-only semantics.
    async fn put_create_strict(
        &self,
        path: &ObjectPath,
        payload: Vec<u8>,
    ) -> Result<UpdateVersion> {
        self.put_create_strict_inner(path, payload, None).await
    }

    pub(crate) async fn put_create_strict_guarded(
        &self,
        path: &ObjectPath,
        payload: Vec<u8>,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<UpdateVersion> {
        self.put_create_strict_inner(path, payload, Some(work_budget))
            .await
    }

    async fn put_create_strict_inner(
        &self,
        path: &ObjectPath,
        payload: Vec<u8>,
        work_budget: Option<&OperatorWorkBudgetGuard>,
    ) -> Result<UpdateVersion> {
        self.enforce_payload_cap(&format!("create-only object {path}"), &payload)?;
        // Convert once before the remote operation. Cloning `Bytes` for the
        // create attempt is O(1), including on the terminal manifest path
        // where the one-use commit permit has already been consumed.
        let payload = Bytes::from(payload);
        let put_outcome = if let Some(work_budget) = work_budget {
            guarded_async_operation_outcome(work_budget, OperatorWorkBudgetStage::Publish, async {
                self.store
                    .put_opts(path, payload.clone().into(), PutMode::Create.into())
                    .await
            })
            .await?
        } else {
            self.store
                .put_opts(path, payload.clone().into(), PutMode::Create.into())
                .await
        };
        put_outcome
            .map(Into::into)
            .with_context(|| format!("strict create-only put {path}"))
    }

    async fn verify_existing_probe_payload(
        &self,
        path: &ObjectPath,
        expected_payload: &[u8],
        work_budget: Option<&OperatorWorkBudgetGuard>,
        object_label: &str,
    ) -> Result<UpdateVersion> {
        let existing = if let Some(work_budget) = work_budget {
            guarded_async_operation_outcome(
                work_budget,
                OperatorWorkBudgetStage::Publish,
                self.store.get(path),
            )
            .await?
            .with_context(|| format!("read existing {object_label}"))?
        } else {
            self.store
                .get(path)
                .await
                .with_context(|| format!("read existing {object_label}"))?
        };
        verify_create_conflict_payload(existing, expected_payload, work_budget, object_label).await
    }
}

async fn verify_create_conflict_payload(
    existing: object_store::GetResult,
    expected_payload: &[u8],
    work_budget: Option<&OperatorWorkBudgetGuard>,
    object_label: &str,
) -> Result<UpdateVersion> {
    let expected_bytes = u64::try_from(expected_payload.len())
        .with_context(|| format!("{object_label} expected byte length does not fit u64"))?;
    let (version, existing_bytes) =
        read_exact_get_result_payload(existing, expected_bytes, work_budget, object_label).await?;
    ensure!(
        existing_bytes.as_slice() == expected_payload,
        "{object_label} already exists with different payload"
    );
    Ok(version)
}

async fn read_exact_get_result_payload(
    existing: object_store::GetResult,
    expected_bytes: u64,
    work_budget: Option<&OperatorWorkBudgetGuard>,
    object_label: &str,
) -> Result<(UpdateVersion, Vec<u8>)> {
    ensure!(
        existing.meta.size == expected_bytes,
        "{object_label} Content-Length {} does not match exact expected {expected_bytes}",
        existing.meta.size
    );
    ensure!(
        existing.range.start == 0 && existing.range.end == expected_bytes,
        "{object_label} response range {:?} does not cover exact expected bytes 0..{expected_bytes}",
        existing.range
    );
    let version = UpdateVersion {
        e_tag: existing.meta.e_tag.clone(),
        version: existing.meta.version.clone(),
    };
    // A terminal create conflict can occur after its one-use commit permit is
    // consumed, and standalone index reads have no execution-plan deadline.
    // Those paths receive an unbounded guard only after response metadata and
    // range pass the allocation-free size check.
    let unbounded;
    let work_budget = if let Some(work_budget) = work_budget {
        work_budget
    } else {
        unbounded = OperatorWorkBudgetGuard::unbounded();
        &unbounded
    };
    let mut existing_bytes = ExactSizedObjectBuffer::new(expected_bytes)?;
    let mut stream = existing.into_stream();
    loop {
        let chunk =
            guarded_async_operation_outcome(work_budget, OperatorWorkBudgetStage::Publish, async {
                stream.next().await.transpose()
            })
            .await?
            .with_context(|| format!("stream existing {object_label} body"))?;
        let Some(chunk) = chunk else { break };
        existing_bytes.push(&chunk, work_budget, OperatorWorkBudgetStage::Publish)?;
    }
    let existing_bytes = existing_bytes.finish(work_budget, OperatorWorkBudgetStage::Publish)?;
    Ok((version, existing_bytes))
}

async fn read_capped_artifact_index_payload(
    existing: object_store::GetResult,
    artifact_root: &ResolvedArtifactRoot,
    object_label: &str,
) -> Result<(UpdateVersion, Vec<u8>)> {
    let expected_bytes = existing.meta.size;
    enforce_final_object_byte_cap(
        object_label,
        expected_bytes,
        artifact_root.max_final_object_bytes(),
    )?;
    read_exact_get_result_payload(existing, expected_bytes, None, object_label).await
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
        validate_artifact_uri_for_kind(
            artifact_root,
            self.artifact_kind,
            "artifact_uri",
            &self.artifact_uri,
        )?;
        validate_artifact_uri_for_kind(
            artifact_root,
            self.artifact_kind,
            "manifest_uri",
            &self.manifest_uri,
        )?;
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
        validate_artifact_uri_for_kind(
            artifact_root,
            self.artifact_kind,
            "artifact_uri",
            &self.artifact_uri,
        )?;
        validate_artifact_uri_for_kind(
            artifact_root,
            self.artifact_kind,
            "manifest_uri",
            &self.manifest_uri,
        )?;
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
    authority: Option<ArtifactIndexWriteAuthority>,
}

impl<'a> ArtifactIndexWriter<'a> {
    #[must_use]
    pub fn new(store: &'a dyn ObjectStore) -> Self {
        Self {
            store,
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
        CreateOnlyArtifactWriter::new(self.store, artifact_root)
            .put_create_strict(&path, payload)
            .await
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
        CreateOnlyArtifactWriter::new(self.store, artifact_root)
            .put_create_strict(&path, snapshot.bytes()?)
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
        CreateOnlyArtifactWriter::new(self.store, artifact_root)
            .put_create_strict(&path, audit_epoch.bytes()?)
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
        let payload = pointer.bytes()?;
        CreateOnlyArtifactWriter::new(self.store, artifact_root)
            .enforce_payload_cap(&format!("artifact index latest pointer {path}"), &payload)?;
        let result = self
            .store
            .put_opts(&path, payload.into(), PutMode::Create.into())
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
        let payload = pointer.bytes()?;
        CreateOnlyArtifactWriter::new(self.store, artifact_root)
            .enforce_payload_cap(&format!("artifact index latest pointer {path}"), &payload)?;
        let result = self
            .store
            .put_opts(&path, payload.into(), PutMode::Update(expected).into())
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
        let (version, bytes) = read_capped_artifact_index_payload(
            object,
            artifact_root,
            &format!("artifact index pointer {path}"),
        )
        .await?;
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
        let (_version, bytes) = read_capped_artifact_index_payload(
            object,
            artifact_root,
            &format!("artifact index event {path}"),
        )
        .await?;
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
        let (_version, bytes) = read_capped_artifact_index_payload(
            object,
            artifact_root,
            &format!("artifact index snapshot {path}"),
        )
        .await?;
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

fn validate_artifact_uri_for_kind(
    artifact_root: &ResolvedArtifactRoot,
    kind: ArtifactKind,
    field: &str,
    uri: &str,
) -> Result<()> {
    artifact_root.object_path_for_uri(uri)?;
    let typed_root = artifact_root.typed_root(kind);
    ensure!(
        uri.starts_with(&format!("{typed_root}/")),
        "{field} for {kind:?} artifact must live under {typed_root}/"
    );
    if kind != ArtifactKind::ResearchAnalytics {
        return Ok(());
    }

    let in_ra_family = RESEARCH_ANALYTICS_ARTIFACT_FAMILIES
        .iter()
        .any(|family| uri.starts_with(&format!("{typed_root}/{family}/")));
    ensure!(
        in_ra_family,
        "{field} for research analytics artifact must live under {typed_root}/<{}>/",
        RESEARCH_ANALYTICS_ARTIFACT_FAMILIES.join("|")
    );
    Ok(())
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
    is_create_only_conflict(err)
}

fn is_create_only_conflict(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<object_store::Error>()
            .is_some_and(is_object_store_create_only_conflict)
    })
}

fn is_object_store_create_only_conflict(err: &object_store::Error) -> bool {
    matches!(
        err,
        object_store::Error::AlreadyExists { .. } | object_store::Error::Precondition { .. }
    )
}

/// # Errors
///
/// Returns an error when `value` is not a canonical s3 artifact root with a
/// bucket and prefix.
pub fn validate_artifact_root(value: &str) -> Result<String> {
    normalize_artifact_root(value)
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
    ensure_path_token("artifact_root prefix", prefix, PathTokenMode::AllowEquals)?;
    Ok(root.to_string())
}

fn artifact_bucket_name(artifact_root: &str) -> Result<&str> {
    let Some((bucket, _prefix)) = artifact_root
        .strip_prefix("s3://")
        .and_then(|without_scheme| without_scheme.split_once('/'))
    else {
        bail!("artifact_root must include an S3 bucket and prefix");
    };
    ensure!(
        !bucket.trim().is_empty(),
        "artifact_root S3 bucket is empty"
    );
    Ok(bucket)
}

fn ensure_resolved_credential_value(field: &'static str, value: String) -> Result<String> {
    let trimmed = value.trim();
    ensure!(
        !trimmed.is_empty(),
        "{field} must resolve to a non-empty value"
    );
    ensure!(
        trimmed == value,
        "{field} must not contain leading or trailing whitespace"
    );
    Ok(value)
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
        subpaths.nt_catalog_synthetic_proof.as_str(),
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

fn ensure_probe_prefix_is_private(
    create_only_probe: &CreateOnlyProbeConfig,
    subpaths: &ArtifactSubpaths,
) -> Result<()> {
    let values = [
        subpaths.raw.as_str(),
        subpaths.nt_catalog.as_str(),
        subpaths.nt_catalog_synthetic_proof.as_str(),
        subpaths.source_proofs.as_str(),
        subpaths.backtests.as_str(),
        subpaths.artifact_index.as_str(),
        subpaths.research_analytics.as_str(),
    ];
    ensure!(
        !values.contains(&create_only_probe.prefix.as_str()),
        "create_only_probe.prefix must not reuse an artifact subpath"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backfill_execution_plan::BackfillExecutionWorkBudget,
        operator_work_budget::OperatorWorkBudget,
        run_manifest::{
            CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION, CatalogProjectionManifestObject,
        },
    };

    fn catalog_dispatch_toml(encoding: &str) -> String {
        format!(
            r#"
{encoding}

[[bindings]]
source_binding = "binary-official"
market_structure_fixture = "binary-option"
catalog_projection_id = "binary-projection-1"
"#
        )
    }

    #[test]
    fn catalog_dispatch_rejects_missing_encoding() {
        let error = toml::from_str::<CatalogDispatchConfig>(&catalog_dispatch_toml(""))
            .expect_err("catalog encoding must be explicit");

        assert!(error.to_string().contains("encoding"), "{error}");
    }

    #[test]
    fn catalog_encoding_rejects_zero_batch_size() {
        let error = toml::from_str::<CatalogDispatchConfig>(&catalog_dispatch_toml(
            r#"
[encoding]
batch_size = 0
max_row_group_size = 5000
compression = "snappy"
"#,
        ))
        .expect_err("catalog batch_size must be positive");

        assert!(error.to_string().contains("batch_size"), "{error}");
    }

    #[test]
    fn catalog_encoding_rejects_zero_max_row_group_size() {
        let error = toml::from_str::<CatalogDispatchConfig>(&catalog_dispatch_toml(
            r#"
[encoding]
batch_size = 5000
max_row_group_size = 0
compression = "snappy"
"#,
        ))
        .expect_err("catalog max_row_group_size must be positive");

        assert!(error.to_string().contains("max_row_group_size"), "{error}");
    }

    #[test]
    fn catalog_encoding_rejects_unknown_compression() {
        let error = toml::from_str::<CatalogDispatchConfig>(&catalog_dispatch_toml(
            r#"
[encoding]
batch_size = 5000
max_row_group_size = 5000
compression = "implicit-default"
"#,
        ))
        .expect_err("catalog compression must map to an explicit supported NT value");

        assert!(error.to_string().contains("compression"), "{error}");
    }

    #[test]
    fn catalog_encoding_hash_binds_every_explicit_encoding_value() {
        let baseline = CatalogEncodingConfig::new(5_000, 5_000, CatalogCompression::Snappy)
            .expect("valid baseline encoding");
        let changed_batch = CatalogEncodingConfig::new(5_001, 5_000, CatalogCompression::Snappy)
            .expect("valid changed batch encoding");
        let changed_row_group =
            CatalogEncodingConfig::new(5_000, 5_001, CatalogCompression::Snappy)
                .expect("valid changed row-group encoding");

        let baseline_hash = baseline.content_hash().expect("hash catalog encoding");
        assert!(crate::hashing::is_lowercase_sha256_hex(&baseline_hash));
        assert_eq!(
            baseline_hash,
            baseline
                .content_hash()
                .expect("repeat catalog encoding hash")
        );
        assert_ne!(
            baseline_hash,
            changed_batch.content_hash().expect("hash changed batch")
        );
        assert_ne!(
            baseline_hash,
            changed_row_group
                .content_hash()
                .expect("hash changed row group")
        );
    }

    #[cfg(unix)]
    #[test]
    fn catalog_projection_file_identity_rejects_a_swapped_open_handle() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let selected_path = temp.path().join("selected.parquet");
        let foreign_path = temp.path().join("foreign.parquet");
        fs::write(&selected_path, b"selected").expect("selected file");
        fs::write(&foreign_path, b"foreign!").expect("foreign file");
        let selected_metadata = fs::symlink_metadata(&selected_path).expect("selected lstat");
        let foreign_file = fs::File::open(&foreign_path).expect("foreign open");
        let foreign_metadata = foreign_file.metadata().expect("foreign fstat");

        let error = validate_pinned_regular_file_identity(
            &selected_path,
            &selected_metadata,
            &foreign_metadata,
        )
        .expect_err("path-to-handle identity swap must fail closed");

        assert!(error.to_string().contains("device/inode"), "{error:#}");
    }

    #[test]
    fn catalog_publication_rejects_missing_version_id() {
        let error = required_versioned_create_result(
            UpdateVersion {
                e_tag: Some("etag".to_string()),
                version: None,
            },
            "catalog object",
        )
        .expect_err("versioned publication requires a version ID");

        assert!(error.to_string().contains("version ID"), "{error:#}");
    }

    #[test]
    fn catalog_publication_rejects_null_version_id() {
        let error = required_versioned_create_result(
            UpdateVersion {
                e_tag: Some("etag".to_string()),
                version: Some("null".to_string()),
            },
            "catalog object",
        )
        .expect_err("S3's null version is not immutable authority");

        assert!(error.to_string().contains("null"), "{error:#}");
    }

    #[test]
    fn catalog_publication_rejects_missing_or_empty_etag() {
        for e_tag in [None, Some(String::new()), Some("   ".to_string())] {
            let error = required_versioned_create_result(
                UpdateVersion {
                    e_tag,
                    version: Some("version-1".to_string()),
                },
                "catalog object",
            )
            .expect_err("versioned publication requires a nonempty ETag");

            assert!(error.to_string().contains("ETag"), "{error:#}");
        }
    }

    #[test]
    fn durable_preflight_rejects_suspended_or_absent_bucket_versioning() {
        for status in [
            None,
            Some(&aws_sdk_s3::types::BucketVersioningStatus::Suspended),
        ] {
            let error = ensure_bucket_versioning_status_enabled(status)
                .expect_err("only Enabled may mint the opaque capability");
            assert!(error.to_string().contains("Enabled"), "{error:#}");
        }
        ensure_bucket_versioning_status_enabled(Some(
            &aws_sdk_s3::types::BucketVersioningStatus::Enabled,
        ))
        .expect("Enabled versioning passes preflight");
    }

    #[cfg(unix)]
    #[test]
    fn hydration_path_budget_rejects_before_namespace_mutation() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let root = temp.path().join("private-catalog");
        fs::create_dir(&root).expect("private catalog root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("private root mode");
        let root_lease = PrivateCatalogRootLease::open_empty(&root).expect("root lease");
        let relative_path = format!("{}.parquet", "a".repeat(180));
        let max_decoded_bytes = u64::try_from(relative_path.len() + 1).expect("path budget");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(
            BackfillExecutionWorkBudget {
                max_decoded_bytes,
                max_source_rows: 1,
                max_projected_row_groups: 1,
                max_wall_seconds: 60,
                require_object_selection_metadata: false,
            },
        ))
        .expect("path allocation guard");

        let error = create_private_hydration_file(
            &root_lease,
            &relative_path,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("combined display/component allocation must fail before openat");

        assert!(
            format!("{error:#}").contains("cumulative retained memory"),
            "{error:#}"
        );
        assert_eq!(fs::read_dir(&root).expect("read private root").count(), 0);
    }

    #[test]
    fn broad_catalog_preflight_rejects_cumulative_metadata_before_allocation() {
        let mut manifest_objects = Vec::new();
        for index in 0..64 {
            manifest_objects.push(CatalogProjectionManifestObject {
                relative_path: format!("data/trades/instrument={index:04}/part-000.parquet"),
                byte_len: 1,
                sha256: sha256_bytes(&[u8::try_from(index).expect("test index")]),
            });
        }
        let physical_manifest = CatalogProjectionManifestDocument {
            schema_version: CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION.to_string(),
            objects: manifest_objects,
        };
        let binding = CatalogProjectionBinding {
            source_binding: "source".to_string(),
            market_structure_fixture: MarketStructureFixture::BinaryOption,
            catalog_projection_id: "broad".to_string(),
        };
        let catalog_root_uri = "s3://catalog/projection=broad/".to_string();
        let physical_manifest_sha256 = sha256_bytes(b"manifest");
        let input_retained = catalog_projection_manifest_retained_bytes(&physical_manifest)
            .expect("manifest retained")
            .checked_add(
                catalog_publication_header_retained_bytes(
                    &binding,
                    &catalog_root_uri,
                    &physical_manifest_sha256,
                )
                .expect("header retained"),
            )
            .expect("input retained total");
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(
            BackfillExecutionWorkBudget {
                max_decoded_bytes: input_retained,
                max_source_rows: 1,
                max_projected_row_groups: 64,
                max_wall_seconds: 60,
                require_object_selection_metadata: false,
            },
        ))
        .expect("broad publication guard");

        let error = preflight_catalog_publication_retained_peak(
            &physical_manifest,
            &binding,
            &catalog_root_uri,
            &physical_manifest_sha256,
            S3_SINGLE_PUT_PROTOCOL_CEILING_BYTES - 1,
            &guard,
            OperatorWorkBudgetStage::Publish,
        )
        .expect_err("output metadata must not fit beside the retained broad manifest");

        assert!(
            format!("{error:#}").contains("cumulative retained memory"),
            "{error:#}"
        );
    }

    #[test]
    fn receipt_parse_rejects_cumulative_peak_when_each_live_component_fits() {
        let unbounded = OperatorWorkBudgetGuard::unbounded();
        let physical_manifest = CatalogProjectionManifestDocument {
            schema_version: CATALOG_PROJECTION_MANIFEST_SCHEMA_VERSION.to_string(),
            objects: vec![CatalogProjectionManifestObject {
                relative_path: "part-000.parquet".to_string(),
                byte_len: 1,
                sha256: sha256_bytes(b"x"),
            }],
        };
        let physical_manifest_sha256 = physical_manifest
            .manifest_sha256_guarded(&unbounded, OperatorWorkBudgetStage::Publish)
            .expect("physical manifest hash");
        let receipt = CatalogProjectionPublicationReceipt {
            schema_version: CATALOG_PROJECTION_PUBLICATION_RECEIPT_SCHEMA_VERSION.to_string(),
            catalog_root_uri: "s3://catalog/projection=test/".to_string(),
            physical_manifest_sha256,
            physical_manifest,
            binding: CatalogProjectionBinding {
                source_binding: "source".to_string(),
                market_structure_fixture: MarketStructureFixture::BinaryOption,
                catalog_projection_id: "test".to_string(),
            },
            objects: vec![CatalogProjectionPublicationObject {
                relative_path: "part-000.parquet".to_string(),
                uri: "s3://catalog/projection=test/part-000.parquet".to_string(),
                sha256: sha256_bytes(b"x"),
                byte_len: 1,
                version_id: "version-1".to_string(),
                e_tag: "etag-1".to_string(),
            }],
        };
        let bytes = receipt
            .canonical_bytes_guarded(&unbounded, OperatorWorkBudgetStage::Publish)
            .expect("canonical receipt");
        let receipt_sha256 = receipt
            .receipt_sha256_guarded(&unbounded, OperatorWorkBudgetStage::Publish)
            .expect("receipt hash");
        let wire_bytes = u64::try_from(bytes.len()).expect("wire bytes");
        let retained_receipt_bytes = receipt.retained_memory_bytes().expect("retained receipt");
        let individual_limit = wire_bytes.max(retained_receipt_bytes);
        assert!(wire_bytes <= individual_limit);
        assert!(retained_receipt_bytes <= individual_limit);
        let guard = OperatorWorkBudgetGuard::new(OperatorWorkBudget::Backfill(
            BackfillExecutionWorkBudget {
                max_decoded_bytes: individual_limit,
                max_source_rows: 1,
                max_projected_row_groups: 4,
                max_wall_seconds: 60,
                require_object_selection_metadata: false,
            },
        ))
        .expect("cumulative memory guard");

        let error = CatalogProjectionPublicationReceipt::parse_and_validate_guarded(
            &bytes,
            &receipt_sha256,
            &guard,
            OperatorWorkBudgetStage::ObjectVerification,
        )
        .expect_err("combined live receipt memory must exceed the envelope");

        assert!(
            format!("{error:#}").contains("cumulative retained memory"),
            "{error:#}"
        );
    }
}
