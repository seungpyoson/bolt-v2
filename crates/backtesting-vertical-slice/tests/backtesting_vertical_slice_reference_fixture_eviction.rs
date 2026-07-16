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
//!   * the eviction index does not retain exactly three controls for every
//!     executable record skipped by a pilot-sized pack summary.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::backtesting_vertical_slice_test_support::PMXT_OBJECT_MANIFEST_EVICTED_REFERENCE_PATHS;
use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, GOLDEN_RECORD_DIR_PREFIX, TIER1_EVICTED_SUBTREE_PREFIXES,
    TIER1_KEPT_REFERENCE_PATHS, is_evicted_execution_pack_record_path,
    is_evicted_reference_fixture_path, is_pmxt_source_universe_object_manifest_path,
    is_tier1_evicted_reference_fixture_path, repo_root_from_manifest_dir,
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

/// The keep/evict boundary, verified against every committed pilot pack. Each
/// summary advertises only its materialized golden record-`00000`; those three
/// controls stay on disk and stay out of the eviction index. The index remains
/// authoritative for the skipped executable corpus and must contain exactly
/// three conventionally named controls for every skipped run owned by the same
/// venue/pack scope.
#[test]
fn committed_packs_keep_golden_record_and_evict_the_rest() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    let evicted: BTreeSet<&str> = index
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
    let mut validated_evicted = BTreeSet::new();
    for scope in scopes {
        let scope_name = scope
            .file_name()
            .and_then(|name| name.to_str())
            .expect("execution-pack scope name is UTF-8");
        let summary_path = scope.join("execution-pack/source-universe-execution-pack.json");
        let bytes = fs::read(&summary_path)
            .unwrap_or_else(|e| panic!("read pack summary {}: {e}", summary_path.display()));
        let pack: SourceUniverseExecutionPack = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("parse pack summary {}: {e}", summary_path.display()));
        checked_packs += 1;
        assert_eq!(
            pack.pack_id,
            format!("source-universe-execution-pack-{scope_name}"),
            "execution-pack summary must be owned by its containing scope"
        );
        assert!(
            scope_name.starts_with(&format!("{}-", pack.venue.to_ascii_lowercase())),
            "execution-pack scope {scope_name} must be owned by venue {}",
            pack.venue
        );
        assert_eq!(pack.selected_record_count, 1, "committed packs are pilots");
        assert_eq!(
            pack.materialized_record_count, 1,
            "committed packs materialize one golden pilot"
        );
        assert_eq!(
            pack.records.len(),
            1,
            "pilot summary must not advertise the independently indexed skipped corpus"
        );

        let golden = pack
            .records
            .first()
            .unwrap_or_else(|| panic!("pack {} has no records", summary_path.display()));
        assert_eq!(golden.sequence, 0, "pilot record must be sequence zero");
        let golden_operator_run_id = format!("source-universe-operator-run-{scope_name}-00000");
        assert_eq!(golden.operator_run_id, golden_operator_run_id);
        let golden_run_prefix = format!(
            "{EXECUTION_PACKS_REL}/{scope_name}/execution-pack/runs/00000-{golden_operator_run_id}/"
        );
        let golden_paths: BTreeSet<String> = golden
            .artifact_paths()
            .iter()
            .map(|path| {
                path.to_str()
                    .expect("golden record path is UTF-8")
                    .to_string()
            })
            .collect();
        let expected_golden_paths = [
            "run-spec.toml",
            "backfill-accepted-tranche-manifest.json",
            "backfill-execution-plan.json",
        ]
        .map(|file| format!("{golden_run_prefix}{file}"))
        .into_iter()
        .collect::<BTreeSet<_>>();
        assert_eq!(
            golden_paths, expected_golden_paths,
            "golden controls must obey their venue/pack/run ownership path"
        );
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

        let pack_run_prefix = format!("{EXECUTION_PACKS_REL}/{scope_name}/execution-pack/runs/");
        let owned_evicted: BTreeSet<&str> = evicted
            .iter()
            .copied()
            .filter(|path| path.starts_with(&pack_run_prefix))
            .collect();
        assert_eq!(
            owned_evicted.len() as u64,
            pack.skipped_executable_record_count * 3,
            "eviction index must retain exactly three controls per skipped executable run for {scope_name}"
        );

        let expected_control_files = [
            "backfill-accepted-tranche-manifest.json",
            "backfill-execution-plan.json",
            "run-spec.toml",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        let operator_run_prefix = format!("source-universe-operator-run-{scope_name}-");
        let mut controls_by_run = BTreeMap::<&str, BTreeSet<&str>>::new();
        for path in owned_evicted {
            let relative = path
                .strip_prefix(&pack_run_prefix)
                .expect("owned eviction path has the pack prefix");
            let (run_dir, control_file) = relative
                .split_once('/')
                .expect("evicted execution control has a run directory");
            assert!(
                !control_file.contains('/'),
                "evicted control {path} must be directly inside its run directory"
            );
            let (work_order_sequence, operator_run_id) = run_dir
                .split_once('-')
                .expect("evicted run directory has a sequence prefix");
            assert!(
                work_order_sequence.len() == 5
                    && work_order_sequence.chars().all(|ch| ch.is_ascii_digit())
                    && work_order_sequence != "00000",
                "evicted control {path} must belong to a non-golden five-digit work-order sequence"
            );
            let operator_sequence = operator_run_id
                .strip_prefix(&operator_run_prefix)
                .unwrap_or_else(|| {
                    panic!(
                        "evicted control {path} must be owned by operator-run prefix {operator_run_prefix}"
                    )
                });
            assert!(
                operator_sequence.len() == 5
                    && operator_sequence.chars().all(|ch| ch.is_ascii_digit())
                    && operator_sequence != "00000",
                "evicted control {path} must belong to a non-golden five-digit operator sequence"
            );
            assert!(
                expected_control_files.contains(control_file),
                "evicted run {run_dir} contains unexpected control {control_file}"
            );
            assert!(
                controls_by_run
                    .entry(run_dir)
                    .or_default()
                    .insert(control_file),
                "evicted run {run_dir} repeats control {control_file}"
            );
            assert!(
                !repo_root.join(path).exists(),
                "skipped execution control {path} must remain evicted from the working tree"
            );
            assert!(validated_evicted.insert(path.to_string()));
        }
        assert_eq!(
            controls_by_run.len() as u64,
            pack.skipped_executable_record_count,
            "eviction index must own one run directory per skipped executable record for {scope_name}"
        );
        for (run_dir, control_files) in controls_by_run {
            assert_eq!(
                control_files, expected_control_files,
                "evicted run {run_dir} must retain exactly the three generator controls"
            );
        }
    }

    let evicted = evicted
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        evicted, validated_evicted,
        "every execution-pack eviction entry must validate under one committed pilot pack's ownership rules"
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
fn pmxt_object_manifest_index_entries_match_declared_exact_scope() {
    let repo_root = repo_root_from_manifest_dir();
    let index = EvictedFixtureIndex::load(&repo_root).expect("load evicted-fixtures index");
    let indexed: BTreeSet<String> = index
        .entries
        .iter()
        .map(|entry| entry.path.clone())
        .filter(|path| is_pmxt_source_universe_object_manifest_path(path))
        .collect();
    let declared: BTreeSet<String> = PMXT_OBJECT_MANIFEST_EVICTED_REFERENCE_PATHS
        .iter()
        .map(|path| (*path).to_string())
        .collect();

    assert_eq!(
        indexed,
        declared,
        "PMXT object-manifest index entries must exactly match the declared generated-reference eviction scope; \
         indexed but undeclared: {:?}; declared but unindexed: {:?}",
        indexed.difference(&declared).collect::<Vec<_>>(),
        declared.difference(&indexed).collect::<Vec<_>>(),
    );

    for path in &declared {
        assert!(
            !repo_root.join(path).exists(),
            "PMXT object-manifest artifact {path:?} must be absent from the working tree"
        );
    }
}

#[test]
fn pmxt_object_manifest_scope_has_no_regrown_working_tree_artifacts() {
    let repo_root = repo_root_from_manifest_dir();
    let manifest_root = repo_root.join(
        "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests",
    );
    for path in files_under(&manifest_root) {
        let repo_relative = repo_relative_path(&repo_root, &path);
        assert!(
            !is_pmxt_source_universe_object_manifest_path(&repo_relative),
            "PMXT object-manifest artifact {repo_relative:?} must remain evicted from the working tree"
        );
    }
}

#[test]
fn pmxt_object_manifest_gitignore_patterns_match_eviction_predicates() {
    let repo_root = repo_root_from_manifest_dir();
    for &path in PMXT_OBJECT_MANIFEST_EVICTED_REFERENCE_PATHS {
        assert!(
            is_pmxt_source_universe_object_manifest_path(path),
            "PMXT object-manifest path {path:?} must be in the eviction predicate"
        );
        assert!(
            git_check_ignore(&repo_root, path),
            "PMXT object-manifest path {path:?} must be ignored by .gitignore"
        );
    }

    let hypothetical_aggregate = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-hypothetical/manifest/source-universe-object-manifest.json";
    let hypothetical_category = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-hypothetical/category-manifests/hypothetical-object-manifest-category.json";
    for path in [hypothetical_aggregate, hypothetical_category] {
        assert!(
            is_pmxt_source_universe_object_manifest_path(path),
            "hypothetical PMXT object-manifest path must be in the eviction predicate: {path}"
        );
        assert!(
            git_check_ignore(&repo_root, path),
            "hypothetical PMXT object-manifest path must be ignored by .gitignore: {path}"
        );
    }

    let non_manifest_json = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-hypothetical/category-manifests/metadata.json";
    assert!(!is_pmxt_source_universe_object_manifest_path(
        non_manifest_json
    ));
    assert!(!git_check_ignore(&repo_root, non_manifest_json));

    let nested_scope = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-hypothetical/nested/manifest/source-universe-object-manifest.json";
    assert!(!is_pmxt_source_universe_object_manifest_path(nested_scope));
    assert!(!git_check_ignore(&repo_root, nested_scope));

    let non_pmxt_scope = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/hypothetical/manifest/source-universe-object-manifest.json";
    assert!(!is_pmxt_source_universe_object_manifest_path(
        non_pmxt_scope
    ));
    assert!(!git_check_ignore(&repo_root, non_pmxt_scope));
}
