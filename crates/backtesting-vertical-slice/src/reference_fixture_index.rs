//! Single source of truth for source-universe reference fixtures evicted from
//! git into content-addressed S3 (issue #704).
//!
//! PR #696 committed ~230 MB / 27,644 generated JSON/TOML artifacts under
//! `specs/023-nt-research-analytics-platform/reference/`. The bulk of that is
//! per-record execution-pack output (run specs, execution plans, accepted-tranche
//! manifests) that no test reads: the execution-pack acceptance test only parses
//! each pack's summary plus its first record (`runs/00000-*`). Those per-record
//! artifacts are evicted from the working tree and recorded here by sha256 so the
//! provenance survives. The blobs also remain retrievable from git history (this
//! change performs no history rewrite) and are uploaded content-addressed to
//! `<s3_artifact_root>/<sha256>`.
//!
//! This module is the one owner of the evicted-fixture index schema, its
//! structural validation, and the on-disk eviction/regrowth invariants the CI
//! guard asserts. Hashes use the crate's single SHA-256 implementation
//! ([`crate::hashing::sha256_hex`]).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;

/// Committed location of the evicted-fixture index, relative to the repo root.
pub const INDEX_REPO_PATH: &str =
    "specs/023-nt-research-analytics-platform/reference/evicted-fixtures.index.json";

/// Repo-relative prefix every evicted fixture path must start with.
const REFERENCE_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/";

/// One evicted artifact: a repo-relative path plus the fingerprint of its bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictedFixtureEntry {
    /// Repo-relative path the artifact occupied before eviction.
    pub path: String,
    /// Lowercase-hex SHA-256 of the artifact bytes.
    pub sha256: String,
    /// Artifact size in bytes.
    pub bytes: u64,
}

/// The committed evicted-fixture index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictedFixtureIndex {
    /// Schema version; bump on any incompatible layout change.
    pub schema_version: u32,
    /// Tracking issue (informational).
    #[serde(default)]
    pub issue: String,
    /// Human-readable description (informational).
    #[serde(default)]
    pub description: String,
    /// Content-addressed S3 root holding the evicted blobs.
    pub s3_artifact_root: String,
    /// Whether objects are stored content-addressed (`<root>/<sha256>`).
    pub content_addressed: bool,
    /// Evicted artifacts, sorted strictly ascending by `path`.
    pub entries: Vec<EvictedFixtureEntry>,
}

impl EvictedFixtureIndex {
    /// The schema version this build understands.
    pub const CURRENT_SCHEMA_VERSION: u32 = 1;

    /// Parse the index from JSON bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("parse evicted-fixtures index JSON")
    }

    /// Load and parse the index from a repo root.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(INDEX_REPO_PATH);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read evicted-fixtures index {}", path.display()))?;
        Self::parse(&bytes)
    }

    /// Content-addressed object key for an entry: `<s3_artifact_root>/<sha256>`.
    pub fn object_key(&self, entry: &EvictedFixtureEntry) -> String {
        format!(
            "{}/{}",
            self.s3_artifact_root.trim_end_matches('/'),
            entry.sha256
        )
    }

    /// Validate the index is internally well-formed: known schema, a content-addressed
    /// `s3://` root, a non-empty entry list, repo-relative reference paths, valid
    /// lowercase-hex sha256, and strictly-ascending unique paths. Returns `Err` on the
    /// first violation so the CI guard fails loud rather than trusting a malformed index.
    pub fn validate_structure(&self) -> Result<()> {
        ensure!(
            self.schema_version == Self::CURRENT_SCHEMA_VERSION,
            "evicted-fixtures index schema_version {} != supported {}",
            self.schema_version,
            Self::CURRENT_SCHEMA_VERSION
        );
        ensure!(
            self.content_addressed,
            "evicted-fixtures index must be content_addressed (object key = <root>/<sha256>)"
        );
        ensure!(
            self.s3_artifact_root.starts_with("s3://")
                && self.s3_artifact_root.len() > "s3://".len(),
            "evicted-fixtures index s3_artifact_root must be a non-empty s3:// URI, got {:?}",
            self.s3_artifact_root
        );
        ensure!(
            !self.entries.is_empty(),
            "evicted-fixtures index must list at least one evicted artifact"
        );

        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            ensure!(
                entry.path.starts_with(REFERENCE_PREFIX),
                "evicted entry path {:?} must be repo-relative under {REFERENCE_PREFIX}",
                entry.path
            );
            ensure!(
                !entry.path.contains("..") && !entry.path.starts_with('/'),
                "evicted entry path {:?} must not be absolute or contain `..`",
                entry.path
            );
            ensure!(
                is_lowercase_sha256_hex(&entry.sha256),
                "evicted entry {:?} has malformed sha256 {:?}",
                entry.path,
                entry.sha256
            );
            if let Some(prev) = previous {
                ensure!(
                    prev < entry.path.as_str(),
                    "evicted entries must be strictly ascending + unique by path; \
                     {prev:?} is not < {:?}",
                    entry.path
                );
            }
            previous = Some(entry.path.as_str());
        }
        Ok(())
    }

    /// Assert every indexed artifact is absent from the working tree under `repo_root`.
    /// This is the eviction invariant: an indexed path that reappears on disk means the
    /// bulk corpus was re-committed and must be re-evicted.
    pub fn verify_evicted_absent(&self, repo_root: &Path) -> Result<()> {
        for entry in &self.entries {
            let on_disk = repo_root.join(&entry.path);
            ensure!(
                !on_disk.exists(),
                "evicted artifact {} reappeared in the working tree; it must stay evicted \
                 (in S3 + git history), not committed",
                entry.path
            );
        }
        Ok(())
    }

    /// Compute the index entry for a single file by reading + hashing its bytes.
    /// Used to (re)generate the index from a pre-eviction checkout.
    pub fn entry_for_file(repo_root: &Path, repo_relative: &str) -> Result<EvictedFixtureEntry> {
        let abs = repo_root.join(repo_relative);
        let bytes =
            std::fs::read(&abs).with_context(|| format!("read fixture {}", abs.display()))?;
        Ok(EvictedFixtureEntry {
            path: repo_relative.to_string(),
            sha256: sha256_hex(&bytes),
            bytes: bytes.len() as u64,
        })
    }
}

/// `true` iff `value` is exactly 64 lowercase hex characters.
fn is_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Resolve the repo root from this crate's manifest dir (`<repo>/crates/<crate>`).
/// Mirrors the `env!("CARGO_MANIFEST_DIR")/../..` convention used by the crate's tests;
/// the trailing `..` components are resolved by the OS at access time.
pub fn repo_root_from_manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_index() -> EvictedFixtureIndex {
        EvictedFixtureIndex {
            schema_version: 1,
            issue: String::new(),
            description: String::new(),
            s3_artifact_root: "s3://bucket/reference-fixtures".to_string(),
            content_addressed: true,
            entries: vec![
                EvictedFixtureEntry {
                    path: format!("{REFERENCE_PREFIX}a.json"),
                    sha256: "a".repeat(64),
                    bytes: 1,
                },
                EvictedFixtureEntry {
                    path: format!("{REFERENCE_PREFIX}b.json"),
                    sha256: "b".repeat(64),
                    bytes: 2,
                },
            ],
        }
    }

    #[test]
    fn valid_index_passes_structure_check() {
        sample_index()
            .validate_structure()
            .expect("sample index is valid");
    }

    #[test]
    fn object_key_is_content_addressed() {
        let index = sample_index();
        assert_eq!(
            index.object_key(&index.entries[0]),
            format!("s3://bucket/reference-fixtures/{}", "a".repeat(64))
        );
    }

    #[test]
    fn out_of_order_paths_are_rejected() {
        let mut index = sample_index();
        index.entries.reverse();
        assert!(index.validate_structure().is_err());
    }

    #[test]
    fn duplicate_paths_are_rejected() {
        let mut index = sample_index();
        index.entries[1].path = index.entries[0].path.clone();
        assert!(index.validate_structure().is_err());
    }

    #[test]
    fn uppercase_or_short_sha_is_rejected() {
        let mut index = sample_index();
        index.entries[0].sha256 = "A".repeat(64);
        assert!(index.validate_structure().is_err());
        index.entries[0].sha256 = "a".repeat(63);
        assert!(index.validate_structure().is_err());
    }

    #[test]
    fn non_reference_path_is_rejected() {
        let mut index = sample_index();
        index.entries[0].path = "crates/foo.json".to_string();
        assert!(index.validate_structure().is_err());
    }

    #[test]
    fn non_s3_root_is_rejected() {
        let mut index = sample_index();
        index.s3_artifact_root = "https://example/x".to_string();
        assert!(index.validate_structure().is_err());
    }

    #[test]
    fn empty_entries_rejected() {
        let mut index = sample_index();
        index.entries.clear();
        assert!(index.validate_structure().is_err());
    }
}
