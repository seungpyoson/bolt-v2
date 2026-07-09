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

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, GOLDEN_RECORD_DIR_PREFIX, PHASE3_EVICTED_REFERENCE_PATHS,
    TIER1_EVICTED_SUBTREE_PREFIXES, TIER1_KEPT_REFERENCE_PATHS,
    is_evicted_execution_pack_record_path, is_evicted_reference_fixture_path,
    is_phase3_evicted_reference_fixture_path, is_tier1_evicted_reference_fixture_path,
    repo_root_from_manifest_dir,
};
use backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPack;

const EXECUTION_PACKS_REL: &str =
    "specs/023-nt-research-analytics-platform/reference/source-universe-execution-packs";
const TIER1_EVICTED_FIXTURE_PATHS_REL: &str =
    "specs/023-nt-research-analytics-platform/reference/tier1-evicted-fixture-paths.txt";

fn execution_packs_root() -> PathBuf {
    repo_root_from_manifest_dir().join(EXECUTION_PACKS_REL)
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

/// All files under `dir` (empty if `dir` is absent).
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .flat_map(|path| {
            if path.is_dir() {
                files_under(&path)
            } else {
                vec![path]
            }
        })
        .collect()
}

fn repo_relative_path(repo_root: &Path, path: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or_else(|e| {
            panic!(
                "path {} should be under repo root {}: {e}",
                path.display(),
                repo_root.display()
            )
        })
        .to_str()
        .expect("repo-relative fixture path is UTF-8")
        .to_string()
}

fn read_tier1_evicted_path_manifest(repo_root: &Path) -> BTreeSet<String> {
    let manifest_path = repo_root.join(TIER1_EVICTED_FIXTURE_PATHS_REL);
    let manifest = fs::read_to_string(&manifest_path).unwrap_or_else(|e| {
        panic!(
            "read Tier 1 eviction path manifest {}: {e}",
            manifest_path.display()
        )
    });
    let mut paths = Vec::new();
    for (idx, raw) in manifest.lines().enumerate() {
        assert!(
            !raw.is_empty(),
            "Tier 1 eviction path manifest {} has an empty line at {}",
            manifest_path.display(),
            idx + 1
        );
        assert_eq!(
            raw.trim(),
            raw,
            "Tier 1 eviction path manifest {} line {} has surrounding whitespace",
            manifest_path.display(),
            idx + 1
        );
        paths.push(raw.to_string());
    }

    let sorted_unique: BTreeSet<String> = paths.iter().cloned().collect();
    assert!(
        !sorted_unique.is_empty(),
        "Tier 1 eviction path manifest {} must list at least one path",
        manifest_path.display()
    );
    assert_eq!(
        paths.len(),
        sorted_unique.len(),
        "Tier 1 eviction path manifest {} contains duplicate paths",
        manifest_path.display()
    );
    let sorted: Vec<String> = sorted_unique.iter().cloned().collect();
    assert_eq!(
        paths,
        sorted,
        "Tier 1 eviction path manifest {} must stay sorted",
        manifest_path.display()
    );
    sorted_unique
}

fn git_check_ignore(repo_root: &Path, path: &str) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("check-ignore")
        .arg("--quiet")
        .arg(path)
        .output()
        .expect("run git check-ignore");
    match output.status.code() {
        Some(0) => true,
        Some(1) => false,
        _ => panic!(
            "git check-ignore failed for {path}: status={:?}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ),
    }
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
fn every_indexed_entry_is_in_declared_eviction_scope() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    for entry in &index.entries {
        assert!(
            is_evicted_reference_fixture_path(&entry.path),
            "index entry {:?} is outside the declared reference-fixture eviction scope",
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
    let evicted: HashSet<&str> = index
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .filter(|path| is_evicted_execution_pack_record_path(path))
        .collect();

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

#[test]
fn tier1_index_entries_match_declared_eviction_scope() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    let manifest = read_tier1_evicted_path_manifest(&repo_root);
    let tier1_entries: BTreeSet<String> = index
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .filter(|path| is_tier1_evicted_reference_fixture_path(path))
        .collect();
    assert!(
        !tier1_entries.is_empty(),
        "expected Tier 1 reference-fixture entries in the eviction index"
    );

    for &subtree in TIER1_EVICTED_SUBTREE_PREFIXES {
        let subtree_entries: Vec<&str> = manifest
            .iter()
            .map(String::as_str)
            .filter(|path| path.starts_with(subtree))
            .collect();
        assert!(
            !subtree_entries.is_empty(),
            "Tier 1 subtree {subtree:?} must have at least one manifest-listed evicted path"
        );
    }

    for path in &manifest {
        assert!(
            is_tier1_evicted_reference_fixture_path(path),
            "Tier 1 manifest path {path:?} is outside the declared subtree scope"
        );
        assert!(
            !repo_root.join(path).exists(),
            "Tier 1 manifest path {path:?} must be absent from the working tree"
        );
    }

    let scoped: BTreeSet<String> = index
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .filter(|path| {
            TIER1_EVICTED_SUBTREE_PREFIXES
                .iter()
                .any(|&subtree| path.starts_with(subtree))
        })
        .collect();
    assert_eq!(
        tier1_entries,
        scoped,
        "Tier 1 index entries must equal the declared Tier 1 subtree scope; \
         outside declared scope: {:?}; declared scope but not accepted: {:?}",
        scoped.difference(&tier1_entries).collect::<Vec<_>>(),
        tier1_entries.difference(&scoped).collect::<Vec<_>>(),
    );
    assert_eq!(
        tier1_entries,
        manifest,
        "Tier 1 index entries must equal the independent evicted-path manifest; \
         indexed but not manifest-listed: {:?}; manifest-listed but not indexed: {:?}",
        tier1_entries.difference(&manifest).collect::<Vec<_>>(),
        manifest.difference(&tier1_entries).collect::<Vec<_>>(),
    );
}

#[test]
fn tier1_gitignore_patterns_match_eviction_predicates() {
    let repo_root = repo_root_from_manifest_dir();
    let cases = [
        (
            "conversion work order",
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-work-orders/example/work-order/source-universe-conversion-work-order.json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-work-orders/example/work-order/source-universe-conversion-work-order.toml",
        ),
        (
            "batch execution report direct json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-batch-execution-reports/example/declared-exclusions.json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-batch-execution-reports/example/declared-exclusions.toml",
        ),
        (
            "batch execution report nested json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-batch-execution-reports/example/chunk-00000-00099/retries/seq-00001/source-universe-batch-execution-report.json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-batch-execution-reports/example/chunk-00000-00099/retries/seq-00001/source-universe-batch-execution-report.txt",
        ),
        (
            "pmxt conversion queue",
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/source-universe-conversion-queue.json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/pmxt-polymarket-v2-current/queue/source-universe-conversion-queue.toml",
        ),
        (
            "bybit conversion run plan",
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/run-plan/source-universe-conversion-run-plan.json",
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/run-plan/source-universe-conversion-run-plan.toml",
        ),
        (
            "binance source universe",
            "specs/023-nt-research-analytics-platform/reference/backfill-source-universes/binance-data-vision-trades-2026-03-01-all-instruments/binance-data-vision-trades-source-universe.json",
            "specs/023-nt-research-analytics-platform/reference/backfill-source-universes/binance-data-vision-trades-2026-03-01-all-instruments/metadata.json",
        ),
        (
            "venue scale acceptance ledger",
            "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/example/ledger/venue-scale-conversion-acceptance-ledger.json",
            "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/example/ledger/venue-scale-conversion-acceptance-ledger.toml",
        ),
        (
            "pmxt source proof",
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proof-set.json",
            "specs/023-nt-research-analytics-platform/reference/backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
        ),
    ];

    for (label, evicted_json, non_evicted_sibling) in cases {
        assert!(
            is_tier1_evicted_reference_fixture_path(evicted_json),
            "{label}: representative generated JSON must be in the Tier 1 eviction predicate"
        );
        assert!(
            git_check_ignore(&repo_root, evicted_json),
            "{label}: representative generated JSON must be ignored by .gitignore"
        );
        assert!(
            !is_tier1_evicted_reference_fixture_path(non_evicted_sibling),
            "{label}: non-evicted sibling must stay outside the Tier 1 eviction predicate"
        );
        assert!(
            !git_check_ignore(&repo_root, non_evicted_sibling),
            "{label}: non-evicted sibling must not be ignored by .gitignore"
        );
    }
}

#[test]
fn tier1_evicted_scope_has_no_regrown_working_tree_artifacts() {
    let repo_root = repo_root_from_manifest_dir();
    let offenders: Vec<String> = TIER1_EVICTED_SUBTREE_PREFIXES
        .iter()
        .flat_map(|&subtree| files_under(&repo_root.join(subtree)))
        .map(|path| repo_relative_path(&repo_root, &path))
        .filter(|path| is_tier1_evicted_reference_fixture_path(path))
        .collect();
    assert!(
        offenders.is_empty(),
        "Tier 1 evicted reference artifacts must stay out of the working tree; \
         found {} committed/regrown artifacts: {offenders:?}",
        offenders.len()
    );
}

#[test]
fn tier1_keep_list_fixtures_are_present_and_not_indexed() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    let evicted: HashSet<&str> = index
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();

    for &path in TIER1_KEPT_REFERENCE_PATHS {
        assert!(
            repo_root.join(path).exists(),
            "Tier 1 keep-list fixture {path:?} must remain in the working tree"
        );
        assert!(
            !evicted.contains(path),
            "Tier 1 keep-list fixture {path:?} must not be recorded as evicted"
        );
    }
}

#[test]
fn phase3_index_entries_match_declared_exact_scope() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    let phase3_entries: BTreeSet<String> = index
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .filter(|path| is_phase3_evicted_reference_fixture_path(path))
        .collect();
    let declared: BTreeSet<String> = PHASE3_EVICTED_REFERENCE_PATHS
        .iter()
        .map(|path| (*path).to_string())
        .collect();

    assert_eq!(
        phase3_entries,
        declared,
        "Phase 3 index entries must exactly match the declared generated-reference eviction scope; \
         indexed but undeclared: {:?}; declared but unindexed: {:?}",
        phase3_entries.difference(&declared).collect::<Vec<_>>(),
        declared.difference(&phase3_entries).collect::<Vec<_>>(),
    );

    for path in &declared {
        assert!(
            !repo_root.join(path).exists(),
            "Phase 3 generated reference artifact {path:?} must be absent from the working tree"
        );
    }
}

#[test]
fn phase3_gitignore_patterns_match_eviction_predicates() {
    let repo_root = repo_root_from_manifest_dir();
    for &path in PHASE3_EVICTED_REFERENCE_PATHS {
        assert!(
            is_phase3_evicted_reference_fixture_path(path),
            "Phase 3 declared path {path:?} must be in the eviction predicate"
        );
        assert!(
            git_check_ignore(&repo_root, path),
            "Phase 3 declared path {path:?} must be ignored by .gitignore"
        );
    }
}
