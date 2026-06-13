use std::collections::BTreeSet;

use anyhow::{Context, Result, bail, ensure};
use object_store::{ObjectStore, PutMode, path::Path as ObjectPath};
use serde::{Deserialize, Serialize};

use crate::run_manifest::MarketStructureFixture;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactStoreConfig {
    pub artifact_root: String,
    pub subpaths: ArtifactSubpaths,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        })
    }
}

impl ResolvedArtifactRoot {
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

fn fixture_label(fixture: MarketStructureFixture) -> &'static str {
    match fixture {
        MarketStructureFixture::BinaryOption => "binary-option",
        MarketStructureFixture::PerpsSpot => "perps-spot",
    }
}
