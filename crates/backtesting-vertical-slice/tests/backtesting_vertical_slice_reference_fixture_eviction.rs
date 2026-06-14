//! CI guard for the source-universe reference-fixture eviction (issue #704).
//!
//! Phase 1 evicts the per-record execution-pack artifacts (`runs/<non-00000>/...`)
//! from git into content-addressed S3, recording each blob's sha256 in
//! `evicted-fixtures.index.json`. These tests fail loud if:
//!   * the index is malformed,
//!   * an indexed (evicted) artifact reappears in the working tree,
//!   * the index drifts outside the declared phase-1 scope,
//!   * a new per-record run dir is (re-)committed (regrowth),
//!   * a committed pack's golden record-`00000` is missing or wrongly evicted, or
//!   * the summaries' advertised non-golden paths and the eviction index do not
//!     match exactly (an unindexed advertised path, or an orphaned index entry).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, GOLDEN_RECORD_DIR_PREFIX, repo_root_from_manifest_dir,
};
use backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPack;

const EXECUTION_PACKS_REL: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs";

fn execution_packs_root() -> PathBuf {
    repo_root_from_manifest_dir().join(EXECUTION_PACKS_REL)
}

/// `true` iff `path` is a per-record (non-`00000`) execution-pack run artifact —
/// the phase-1 eviction scope.
fn is_evicted_per_record_path(path: &str) -> bool {
    const MARKER: &str = "/execution-pack/runs/";
    if !path.contains("/source-universe-execution-packs/") {
        return false;
    }
    let Some(idx) = path.find(MARKER) else {
        return false;
    };
    let run = path[idx + MARKER.len()..].split('/').next().unwrap_or("");
    !run.is_empty() && !run.starts_with(GOLDEN_RECORD_DIR_PREFIX)
}

/// Immediate child directories of `dir` (empty if `dir` is absent).
fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect()
}

/// Every `runs/<run>` directory across all committed execution-pack scopes.
fn execution_pack_run_dirs(ep_root: &Path) -> Vec<PathBuf> {
    child_dirs(ep_root)
        .into_iter()
        .flat_map(|scope| child_dirs(&scope.join("execution-pack/runs")))
        .collect()
}

#[test]
fn evicted_fixtures_index_is_well_formed() {
    // `load` structurally validates before returning (see `EvictedFixtureIndex::load`),
    // so a successful load proves the committed index is well-formed — no separate
    // `validate_structure()` re-call needed.
    let repo_root = repo_root_from_manifest_dir();
    EvictedFixtureIndex::load(&repo_root)
        .expect("committed evicted-fixtures index loads + validates");
}

#[test]
fn evicted_fixtures_are_absent_from_working_tree() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    index
        .verify_evicted_absent(&repo_root)
        .expect("indexed artifacts must stay evicted (S3 + git history), not committed");
}

#[test]
fn every_indexed_entry_is_in_phase1_eviction_scope() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    for entry in &index.entries {
        assert!(
            is_evicted_per_record_path(&entry.path),
            "index entry {:?} is outside the phase-1 eviction scope \
             (execution-pack runs/<non-00000> per-record artifacts)",
            entry.path
        );
    }
}

#[test]
fn no_per_record_execution_pack_run_dirs_remain_in_git() {
    let offenders: Vec<PathBuf> = execution_pack_run_dirs(&execution_packs_root())
        .into_iter()
        .filter(|run_dir| {
            run_dir
                .file_name()
                .and_then(|n| n.to_str())
                .map(|name| !name.starts_with(GOLDEN_RECORD_DIR_PREFIX))
                .unwrap_or(false)
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "per-record execution-pack run dirs must be evicted (only record-00000 is kept); \
         found {} committed: {offenders:?}",
        offenders.len()
    );
}

#[test]
fn kept_execution_pack_summaries_are_present() {
    let ep_root = execution_packs_root();
    let scopes = child_dirs(&ep_root);
    assert!(
        !scopes.is_empty(),
        "expected committed execution-pack scopes under {}",
        ep_root.display()
    );
    for scope in scopes {
        let pack = scope.join("execution-pack/source-universe-execution-pack.json");
        assert!(
            pack.exists(),
            "kept execution-pack summary missing after eviction: {}",
            pack.display()
        );
    }
}

/// The keep/evict boundary, verified against every committed pack summary: each
/// pack's golden record-`00000` (the only record the execution-pack acceptance
/// test dereferences) is kept on disk and absent from the eviction index, while
/// every non-golden record the summary still advertises is absent on disk. The
/// set of advertised non-golden paths must equal the eviction index *exactly* (a
/// bijection, both directions), so the corpus cannot drift into either a summary
/// that points at a record neither present nor evicted, or an orphaned index
/// entry no summary advertises.
#[test]
fn committed_packs_keep_golden_record_and_evict_the_rest() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    let evicted: HashSet<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();

    let scopes = child_dirs(&execution_packs_root());
    assert!(
        !scopes.is_empty(),
        "expected committed execution-pack scopes to verify the keep/evict boundary"
    );

    let mut checked_packs = 0usize;
    let mut advertised_evicted: HashSet<String> = HashSet::new();
    for scope in scopes {
        let summary_path = scope.join("execution-pack/source-universe-execution-pack.json");
        let bytes = fs::read(&summary_path)
            .unwrap_or_else(|e| panic!("read pack summary {}: {e}", summary_path.display()));
        let pack: SourceUniverseExecutionPack = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("parse pack summary {}: {e}", summary_path.display()));
        checked_packs += 1;

        let golden = pack
            .records
            .first()
            .unwrap_or_else(|| panic!("pack {} has no records", summary_path.display()));
        for path in golden.artifact_paths() {
            let key = path.to_str().expect("golden record path is UTF-8");
            assert!(
                !evicted.contains(key),
                "golden record artifact {key} must NOT be evicted; the execution-pack \
                 acceptance test reads record-00000"
            );
            assert!(
                repo_root.join(path).exists(),
                "golden record artifact {key} must remain on disk after eviction"
            );
        }

        for record in pack.records.iter().skip(1) {
            for path in record.artifact_paths() {
                let key = path.to_str().expect("record path is UTF-8");
                assert!(
                    evicted.contains(key),
                    "non-golden record artifact {key} the summary advertises must be \
                     covered by the eviction index"
                );
                assert!(
                    !repo_root.join(path).exists(),
                    "non-golden record artifact {key} must be evicted from the working tree"
                );
                assert!(
                    advertised_evicted.insert(key.to_string()),
                    "summary advertises non-golden artifact {key} more than once; a \
                     duplicated path silently orphans the path it replaced in the index"
                );
            }
        }
    }

    // Full bijection, not just advertised ⊆ index: every advertised non-golden
    // path must be indexed AND every index entry must be advertised by some
    // committed summary. This catches the reverse drift the subset check misses —
    // an orphaned index entry (a stale fingerprint nobody points at), or a summary
    // path mutated to duplicate another. It also proves non-vacuity: the index is
    // non-empty (validate_structure), so equality means real evicted paths were
    // exercised, and `checked_packs` confirms a golden record was checked too.
    let advertised: HashSet<&str> = advertised_evicted.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        advertised,
        evicted,
        "advertised non-golden paths must equal the eviction index exactly; \
         orphan index entries (indexed, not advertised): {:?}; \
         advertised but unindexed: {:?}",
        evicted.difference(&advertised).collect::<Vec<_>>(),
        advertised.difference(&evicted).collect::<Vec<_>>(),
    );
    assert!(
        checked_packs >= 1,
        "no execution-pack summaries were checked"
    );
}
