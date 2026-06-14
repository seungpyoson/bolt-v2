//! CI guard for the source-universe reference-fixture eviction (issue #704).
//!
//! Phase 1 evicts the per-record execution-pack artifacts (`runs/<non-00000>/...`)
//! from git into content-addressed S3, recording each blob's sha256 in
//! `evicted-fixtures.index.json`. These tests fail loud if:
//!   * the index is malformed,
//!   * an indexed (evicted) artifact reappears in the working tree,
//!   * the index drifts outside the declared phase-1 scope,
//!   * a new per-record run dir is (re-)committed (regrowth), or
//!   * the golden subset the execution-pack acceptance test reads is removed.

use std::fs;
use std::path::{Path, PathBuf};

use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, repo_root_from_manifest_dir,
};

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
    !run.is_empty() && !run.starts_with("00000-")
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
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    index
        .validate_structure()
        .expect("evicted-fixtures index must be well-formed");
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
                .map(|name| !name.starts_with("00000-"))
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
