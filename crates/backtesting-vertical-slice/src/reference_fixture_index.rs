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
//!
//! This index is the single source of truth for the *fingerprints* (sha256 +
//! byte length) of each evicted blob. The *eviction scope* — which paths leave
//! the tree — is owned by this module's path predicates, asserted by the
//! eviction guard test, and kept from regrowing by `.gitignore`; those surfaces
//! move together whenever the scope changes. Record-`00000` of each execution
//! pack is always kept on disk (the acceptance test reads it); see
//! `GOLDEN_RECORD_DIR_PREFIX`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;

/// Committed location of the evicted-fixture index, relative to the repo root.
pub const INDEX_REPO_PATH: &str =
    "specs/023-nt-research-analytics-platform/reference/evicted-fixtures.index.json";

/// Repo-relative prefix every evicted fixture path must start with.
pub const REFERENCE_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/";

const EXECUTION_PACKS_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs/";
const EXECUTION_PACK_RUN_MARKER: &str = "/execution-pack/runs/";

const TIER1_CONVERSION_WORK_ORDERS_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-work-orders/";
const TIER1_BATCH_EXECUTION_REPORTS_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-batch-execution-reports/";
const TIER1_PMXT_CONVERSION_QUEUE_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/";
const TIER1_BYBIT_CONVERSION_RUN_PLAN_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/run-plan/";
pub const TIER1_PMXT_CONVERSION_QUEUE_PATH: &str = "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/source-universe-conversion-queue.json";
pub const TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH: &str = "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/run-plan/source-universe-conversion-run-plan.json";
const TIER1_BINANCE_SOURCE_UNIVERSE_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-universes/binance-data-vision-trades-2026-03-01-all-instruments/";
const TIER1_VENUE_SCALE_ACCEPTANCE_LEDGERS_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/";
const TIER1_PMXT_SOURCE_PROOFS_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/";
const PHASE3_CONVERSION_BATCHES_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/";
const PHASE3_CONVERSION_BATCH_PLAN_SUFFIX: &str = "/plan/backfill-conversion-batch-plan.json";
const BACKFILL_CONVERSION_COMPLETION_LEDGERS_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/backfill-conversion-completion-ledgers/";
const BACKFILL_CONVERSION_COMPLETION_LEDGER_SUFFIX: &str =
    "/ledger/backfill-conversion-completion-ledger.json";
const PMXT_OBJECT_MANIFESTS_PREFIX: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-";
const PMXT_AGGREGATE_OBJECT_MANIFEST_SUFFIX: &str =
    "/manifest/source-universe-object-manifest.json";
const PMXT_CATEGORY_OBJECT_MANIFEST_DIR: &str = "/category-manifests/";
const BINANCE_OPERATOR_INPUTS_PREFIX: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-operator-inputs/binance-";
const BINANCE_OPERATOR_INPUTS_SUFFIX: &str =
    "/operator-inputs/source-universe-operator-inputs.json";

/// Tier-1 subtree prefixes whose generated JSON artifacts are evicted.
pub const TIER1_EVICTED_SUBTREE_PREFIXES: &[&str] = &[
    TIER1_CONVERSION_WORK_ORDERS_PREFIX,
    TIER1_BATCH_EXECUTION_REPORTS_PREFIX,
    TIER1_PMXT_CONVERSION_QUEUE_PREFIX,
    TIER1_BYBIT_CONVERSION_RUN_PLAN_PREFIX,
    TIER1_BINANCE_SOURCE_UNIVERSE_PREFIX,
    TIER1_VENUE_SCALE_ACCEPTANCE_LEDGERS_PREFIX,
    TIER1_PMXT_SOURCE_PROOFS_PREFIX,
];

/// Tier-1 fixtures deliberately kept on disk: hand-authored `.toml` specs plus
/// the bybit source-universe JSON read by the backfill-gate reference tests.
pub const TIER1_KEPT_REFERENCE_PATHS: &[&str] = &[
    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-work-orders/binance-data-vision-trades-2026-03-01-all-instruments/source-universe-conversion-work-order.toml",
    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-work-orders/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-universe-conversion-work-order.toml",
    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/source-universe-conversion-queue.toml",
    "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-universe-conversion-run-plan.toml",
    "specs/023-nt-research-analytics-platform/reference/backfill-source-universes/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-source-universe.json",
    "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml",
    "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
];

/// Directory-name prefix of the one execution-pack record kept on disk per pack
/// (the `runs/00000-*` dir). The execution-pack acceptance test reads only this
/// golden record; every other `runs/<NNNNN-...>` dir is evicted. Single source of
/// truth shared with the eviction guard test (the `.gitignore` rule mirrors this
/// literal but cannot reference a Rust const).
pub const GOLDEN_RECORD_DIR_PREFIX: &str = "00000-";

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

    /// Load, parse, and structurally validate the index from a repo root. A
    /// successfully loaded index is always well-formed
    /// ([`Self::validate_structure`]), so callers never operate on a malformed
    /// index. Use [`Self::parse`] for the raw deserialize without validation.
    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = repo_root.join(INDEX_REPO_PATH);
        let bytes = std::fs::read(&path)
            .with_context(|| format!("read evicted-fixtures index {}", path.display()))?;
        let index = Self::parse(&bytes)?;
        index
            .validate_structure()
            .with_context(|| format!("validate evicted-fixtures index {}", path.display()))?;
        Ok(index)
    }

    /// Content-addressed object key for an entry: `<s3_artifact_root>/<sha256>`.
    pub fn object_key(&self, entry: &EvictedFixtureEntry) -> String {
        format!(
            "{}/{}",
            self.s3_artifact_root.trim_end_matches('/'),
            entry.sha256
        )
    }

    /// Return the indexed entry for an evicted repo-relative fixture path.
    pub fn entry_for(&self, path: &str) -> Option<&EvictedFixtureEntry> {
        self.entries.iter().find(|entry| entry.path == path)
    }

    /// Return the indexed SHA-256 for an evicted repo-relative fixture path.
    pub fn sha256_for(&self, path: &str) -> Option<&str> {
        self.entry_for(path).map(|entry| entry.sha256.as_str())
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

/// `true` iff `path` is in the declared #704 reference-fixture eviction scope.
pub fn is_evicted_reference_fixture_path(path: &str) -> bool {
    is_evicted_execution_pack_record_path(path)
        || is_tier1_evicted_reference_fixture_path(path)
        || is_phase3_conversion_batch_plan_path(path)
        || is_backfill_conversion_completion_ledger_path(path)
        || is_pmxt_source_universe_object_manifest_path(path)
        || is_binance_source_universe_operator_inputs_path(path)
}

/// `true` iff `path` is a Phase-3 generated conversion batch plan.
pub fn is_phase3_conversion_batch_plan_path(path: &str) -> bool {
    let Some(scope) = path.strip_prefix(PHASE3_CONVERSION_BATCHES_PREFIX) else {
        return false;
    };
    let Some(scope) = scope.strip_suffix(PHASE3_CONVERSION_BATCH_PLAN_SUFFIX) else {
        return false;
    };
    !scope.is_empty() && !scope.contains('/')
}

/// `true` iff `path` is a generated backfill conversion-completion ledger.
pub fn is_backfill_conversion_completion_ledger_path(path: &str) -> bool {
    let Some(scope) = path.strip_prefix(BACKFILL_CONVERSION_COMPLETION_LEDGERS_PREFIX) else {
        return false;
    };
    let Some(scope) = scope.strip_suffix(BACKFILL_CONVERSION_COMPLETION_LEDGER_SUFFIX) else {
        return false;
    };
    !scope.is_empty() && !scope.contains('/')
}

/// `true` iff `path` is a generated PMXT source-universe object manifest.
pub fn is_pmxt_source_universe_object_manifest_path(path: &str) -> bool {
    let Some(scoped_path) = path.strip_prefix(PMXT_OBJECT_MANIFESTS_PREFIX) else {
        return false;
    };
    if let Some(scope) = scoped_path.strip_suffix(PMXT_AGGREGATE_OBJECT_MANIFEST_SUFFIX) {
        return !scope.is_empty() && !scope.contains('/');
    }
    let Some((scope, file)) = scoped_path.split_once(PMXT_CATEGORY_OBJECT_MANIFEST_DIR) else {
        return false;
    };
    !scope.is_empty()
        && !scope.contains('/')
        && !file.contains('/')
        && file.ends_with(".json")
        && file.strip_suffix(".json").is_some_and(|stem| {
            stem.contains("-object-manifest-") && !stem.ends_with("-object-manifest-")
        })
}

/// `true` iff `path` is a generated Binance source-universe operator-inputs artifact.
pub fn is_binance_source_universe_operator_inputs_path(path: &str) -> bool {
    let Some(scope) = path.strip_prefix(BINANCE_OPERATOR_INPUTS_PREFIX) else {
        return false;
    };
    let Some(scope) = scope.strip_suffix(BINANCE_OPERATOR_INPUTS_SUFFIX) else {
        return false;
    };
    !scope.is_empty() && !scope.contains('/')
}

/// `true` iff `path` is a per-record (non-`00000`) execution-pack run artifact.
pub fn is_evicted_execution_pack_record_path(path: &str) -> bool {
    if !path.starts_with(EXECUTION_PACKS_PREFIX) {
        return false;
    }
    let Some(idx) = path.find(EXECUTION_PACK_RUN_MARKER) else {
        return false;
    };
    let run = path[idx + EXECUTION_PACK_RUN_MARKER.len()..]
        .split('/')
        .next()
        .unwrap_or("");
    !run.is_empty() && !run.starts_with(GOLDEN_RECORD_DIR_PREFIX)
}

/// `true` iff `path` belongs to the #704 Phase 2 Tier 1 generated JSON scope.
pub fn is_tier1_evicted_reference_fixture_path(path: &str) -> bool {
    is_conversion_work_order_json(path)
        || is_json_below(path, TIER1_BATCH_EXECUTION_REPORTS_PREFIX)
        || is_direct_json_below(path, TIER1_PMXT_CONVERSION_QUEUE_PREFIX)
        || is_direct_json_below(path, TIER1_BYBIT_CONVERSION_RUN_PLAN_PREFIX)
        || is_binance_source_universe_json(path)
        || is_venue_scale_acceptance_ledger_json(path)
        || is_direct_json_below(path, TIER1_PMXT_SOURCE_PROOFS_PREFIX)
}

fn is_binance_source_universe_json(path: &str) -> bool {
    path.strip_prefix(TIER1_BINANCE_SOURCE_UNIVERSE_PREFIX)
        .is_some_and(|file| is_direct_json_file(file) && file.ends_with("-source-universe.json"))
}

fn is_conversion_work_order_json(path: &str) -> bool {
    let Some(rest) = path.strip_prefix(TIER1_CONVERSION_WORK_ORDERS_PREFIX) else {
        return false;
    };
    let Some((scope, file)) = rest.split_once("/work-order/") else {
        return false;
    };
    !scope.is_empty() && !scope.contains('/') && is_direct_json_file(file)
}

fn is_venue_scale_acceptance_ledger_json(path: &str) -> bool {
    is_single_scope_child_json(
        path,
        TIER1_VENUE_SCALE_ACCEPTANCE_LEDGERS_PREFIX,
        "/ledger/",
    )
}

fn is_single_scope_child_json(path: &str, prefix: &str, marker: &str) -> bool {
    let Some(rest) = path.strip_prefix(prefix) else {
        return false;
    };
    let Some((scope, file)) = rest.split_once(marker) else {
        return false;
    };
    !scope.is_empty() && !scope.contains('/') && is_direct_json_file(file)
}

fn is_json_below(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty() && rest.ends_with(".json"))
}

fn is_direct_json_below(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix).is_some_and(is_direct_json_file)
}

fn is_direct_json_file(value: &str) -> bool {
    !value.is_empty() && !value.contains('/') && value.ends_with(".json")
}

/// `true` iff `value` is exactly 64 lowercase hex characters.
fn is_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Resolve the repo root from this crate's manifest dir (`<repo>/crates/<crate>`).
pub fn repo_root_from_manifest_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root")
        .to_path_buf()
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
    fn entry_and_sha_lookup_are_owned_by_index() {
        let index = sample_index();
        let path = format!("{REFERENCE_PREFIX}b.json");
        let expected_sha256 = "b".repeat(64);
        assert_eq!(index.entry_for(&path).expect("entry exists").bytes, 2);
        assert_eq!(index.sha256_for(&path), Some(expected_sha256.as_str()));
        assert_eq!(index.entry_for("missing.json"), None);
        assert_eq!(index.sha256_for("missing.json"), None);
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

    #[test]
    fn entry_for_file_hashes_bytes_at_repo_relative_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rel = "sub/dir/fixture.json";
        let abs = dir.path().join(rel);
        std::fs::create_dir_all(abs.parent().expect("fixture has a parent dir"))
            .expect("create parent dir");
        let payload = br#"{"k":"v"}"#;
        std::fs::write(&abs, payload).expect("write fixture");

        let entry = EvictedFixtureIndex::entry_for_file(dir.path(), rel)
            .expect("entry_for_file reads + hashes the file");
        assert_eq!(entry.path, rel);
        assert_eq!(entry.bytes, payload.len() as u64);
        assert_eq!(entry.sha256, sha256_hex(payload));
    }

    #[test]
    fn tier1_binance_source_universe_scope_matches_gitignore_glob() {
        let accepted = format!(
            "{TIER1_BINANCE_SOURCE_UNIVERSE_PREFIX}binance-data-vision-trades-source-universe.json"
        );
        let rejected_direct_json = format!("{TIER1_BINANCE_SOURCE_UNIVERSE_PREFIX}metadata.json");
        let rejected_nested = format!(
            "{TIER1_BINANCE_SOURCE_UNIVERSE_PREFIX}nested/binance-data-vision-trades-source-universe.json"
        );

        assert!(is_tier1_evicted_reference_fixture_path(&accepted));
        assert!(!is_tier1_evicted_reference_fixture_path(
            &rejected_direct_json
        ));
        assert!(!is_tier1_evicted_reference_fixture_path(&rejected_nested));
    }

    #[test]
    fn backfill_conversion_completion_ledger_scope_is_single_scope_generated_json() {
        let accepted = format!(
            "{BACKFILL_CONVERSION_COMPLETION_LEDGERS_PREFIX}example/ledger/backfill-conversion-completion-ledger.json"
        );
        let rejected_toml = format!(
            "{BACKFILL_CONVERSION_COMPLETION_LEDGERS_PREFIX}example/backfill-conversion-completion-ledger.toml"
        );
        let rejected_nested = format!(
            "{BACKFILL_CONVERSION_COMPLETION_LEDGERS_PREFIX}example/nested/ledger/backfill-conversion-completion-ledger.json"
        );
        let rejected_other_json =
            format!("{BACKFILL_CONVERSION_COMPLETION_LEDGERS_PREFIX}example/ledger/metadata.json");

        assert!(is_backfill_conversion_completion_ledger_path(&accepted));
        assert!(!is_backfill_conversion_completion_ledger_path(
            &rejected_toml
        ));
        assert!(!is_backfill_conversion_completion_ledger_path(
            &rejected_nested
        ));
        assert!(!is_backfill_conversion_completion_ledger_path(
            &rejected_other_json
        ));
    }

    #[test]
    fn pmxt_object_manifest_scope_accepts_only_family_shapes() {
        let accepted_aggregate = format!(
            "{PMXT_OBJECT_MANIFESTS_PREFIX}example/manifest/source-universe-object-manifest.json"
        );
        let accepted_category = format!(
            "{PMXT_OBJECT_MANIFESTS_PREFIX}example/category-manifests/example-object-manifest-category.json"
        );
        let rejected_non_manifest =
            format!("{PMXT_OBJECT_MANIFESTS_PREFIX}example/category-manifests/metadata.json");
        let rejected_nested = format!(
            "{PMXT_OBJECT_MANIFESTS_PREFIX}example/nested/manifest/source-universe-object-manifest.json"
        );
        let rejected_non_pmxt = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/example/manifest/source-universe-object-manifest.json";

        assert!(is_pmxt_source_universe_object_manifest_path(
            &accepted_aggregate
        ));
        assert!(is_pmxt_source_universe_object_manifest_path(
            &accepted_category
        ));
        assert!(!is_pmxt_source_universe_object_manifest_path(
            &rejected_non_manifest
        ));
        assert!(!is_pmxt_source_universe_object_manifest_path(
            &rejected_nested
        ));
        assert!(!is_pmxt_source_universe_object_manifest_path(
            rejected_non_pmxt
        ));
    }

    #[test]
    fn binance_operator_inputs_scope_accepts_only_family_shape() {
        let accepted = format!(
            "{BINANCE_OPERATOR_INPUTS_PREFIX}example/operator-inputs/source-universe-operator-inputs.json"
        );
        let rejected_bybit = "specs/023-nt-research-analytics-platform/reference/source-universe-operator-inputs/bybit-example/operator-inputs/source-universe-operator-inputs.json";
        let rejected_empty_scope = "specs/023-nt-research-analytics-platform/reference/source-universe-operator-inputs/binance-/operator-inputs/source-universe-operator-inputs.json";
        let rejected_nested = format!(
            "{BINANCE_OPERATOR_INPUTS_PREFIX}example/nested/operator-inputs/source-universe-operator-inputs.json"
        );
        let rejected_other_json =
            format!("{BINANCE_OPERATOR_INPUTS_PREFIX}example/operator-inputs/metadata.json");
        let rejected_toml =
            format!("{BINANCE_OPERATOR_INPUTS_PREFIX}example/source-universe-operator-inputs.toml");

        assert!(is_binance_source_universe_operator_inputs_path(&accepted));
        assert!(!is_binance_source_universe_operator_inputs_path(
            rejected_bybit
        ));
        assert!(!is_binance_source_universe_operator_inputs_path(
            rejected_empty_scope
        ));
        assert!(!is_binance_source_universe_operator_inputs_path(
            &rejected_nested
        ));
        assert!(!is_binance_source_universe_operator_inputs_path(
            &rejected_other_json
        ));
        assert!(!is_binance_source_universe_operator_inputs_path(
            &rejected_toml
        ));
    }
}
