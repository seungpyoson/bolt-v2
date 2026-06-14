//! Single owner of source-text canonicalization and registry-keyed text access
//! for the abort-plan gate sources.
//!
//! This module owns two things:
//!
//! 1. **The registry surface** — [`STRATEGY_KEY`] / [`SUBMIT_ADMISSION_KEY`] /
//!    [`OUTCOME_GROUP_KEY`] and the [`GATED_SOURCE_ROOTS`] table mapping each key
//!    to its repo-relative source roots. The roots are named in exactly one place
//!    — the repo-root `gated_source_roots.manifest`, which `build.rs` compiles
//!    into [`GATED_SOURCE_ROOTS`]; this module re-exports that generated table.
//! 2. The text accessors [`module_source_text`] (whole-module text) and
//!    [`production_module_source_text`] (test-submodule-free text), both in the
//!    SAME canonical file order. These are thin wrappers over the
//!    [`crate::source_canonicalization`] walk module's text functions
//!    (delegating with a fixed byte cap), so there is exactly one transcription
//!    of the walk + text-extraction logic.
//!
//! The lowercase-hex SHA-256 helper [`sha256_hex_lower`] is re-exported here for
//! provider artifact code. The binary source-digest/framing primitives this
//! module used to own were removed with the golden-digest gate.
//!
//! Tests and provider artifact helpers call the registry-keyed text accessors
//! here.

use std::io;
use std::path::{Path, PathBuf};

pub use crate::source_canonicalization::{
    GATED_SOURCE_ROOTS, GatedSourceRoot, OUTCOME_GROUP_KEY, STRATEGY_KEY, SUBMIT_ADMISSION_KEY,
    TEST_MODULE_SPLIT_MARKER, TEST_ONLY_INNER_CFG_MARKER,
    module_source_set_text as canonical_module_source_set_text,
    module_source_text as canonical_module_text,
    production_module_source_text as canonical_production_module_text,
    production_source_set_text as canonical_production_source_set_text, registry_entry,
    sha256_hex_lower,
};

/// Repo-relative source roots for a registry key.
pub fn registry_relative_roots(key: &str) -> &'static [&'static str] {
    registry_entry(key).relative_roots
}

/// Primary repo-relative root path for a registry key. For source-set entries,
/// this remains the strategy directory used by older path-based collector tests;
/// registry-keyed text helpers use every root in the set.
pub fn registry_relative_root(key: &str) -> &'static str {
    registry_relative_roots(key)
        .first()
        .copied()
        .unwrap_or_else(|| panic!("gated source registry key `{key}` has no source roots"))
}

/// Primary absolute repo path for a registry key, rooted at the crate manifest
/// dir.
pub fn registry_root_path(key: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(registry_relative_root(key))
}

/// Absolute repo paths for every root in a registry-keyed source set.
pub fn registry_root_paths(key: &str) -> Vec<PathBuf> {
    registry_relative_roots(key)
        .iter()
        .map(|relative| Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
        .collect()
}

/// Whole-module source text for a registry source set, bounded by `max_bytes`.
pub fn registry_module_source_text(key: &str, max_bytes: u64) -> io::Result<String> {
    canonical_module_source_set_text(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        registry_relative_roots(key),
        max_bytes,
    )
}

/// A bound large enough to admit either gated root: the submit_admission single
/// file and the strategy source set (the strategy directory plus the shared
/// execution sources). Used by the text accessors (whole module / production
/// text), where there is no operator-supplied cap; it is the single source for
/// that in-process text-accessor bound.
const TEXT_ACCESSOR_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Whole-module source text for a registry key, in canonical file order.
pub fn module_source_text(key: &str) -> String {
    canonical_module_source_set_text(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        registry_relative_roots(key),
        TEXT_ACCESSOR_MAX_BYTES,
    )
    .unwrap_or_else(|error| panic!("module source text for `{key}` should read: {error}"))
}

/// Production-only module source text for a registry key: the whole-module text
/// with each file's bottom `#[cfg(test)] mod tests` submodule excluded.
///
/// Delegates to the SINGLE production/test boundary defined in
/// [`crate::source_canonicalization::production_source_set_text`].
///
/// IDENTITY case (e.g. `submit_admission`, a single file): reproduces the
/// historical `source.split("\n#[cfg(test)]\nmod tests").next()` output
/// byte-for-byte — strips ONLY at the FIRST top-level test-module marker, so the
/// earlier inline `#[cfg(test)]` markers are retained (value-stability).
///
/// SOURCE-SET case (e.g. the strategy directory plus shared execution helpers):
/// the production half of EACH file — split independently at its own first
/// top-level marker — concatenated in canonical order. This keeps every
/// production file in scope rather than dropping every file sorted after the
/// marker-owning file.
pub fn production_module_source_text(key: &str) -> String {
    canonical_production_source_set_text(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        registry_relative_roots(key),
        TEXT_ACCESSOR_MAX_BYTES,
    )
    .unwrap_or_else(|error| {
        panic!("production module source text for `{key}` should read: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strategy source-set files, in strict repo-relative-path-byte order.
    /// Enumerated dynamically with the same fail-closed symlink/backslash policy
    /// as the production canonicalizer, so the invariant tracks accepted source
    /// moves without hardcoding a file list.
    fn source_files_in_canonical_order(
        key: &str,
        label: &str,
    ) -> Vec<(String, std::path::PathBuf)> {
        fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let file_type = std::fs::symlink_metadata(&path).unwrap().file_type();
                assert!(
                    !file_type.is_symlink(),
                    "strategy source helper must reject symlinks: {}",
                    path.display()
                );
                if file_type.is_dir() {
                    collect(&path, out);
                } else if file_type.is_file()
                    && path.extension().and_then(|ext| ext.to_str()) == Some("rs")
                {
                    out.push(path);
                }
            }
        }
        fn normalized_path(path: &std::path::Path) -> String {
            let mut parts = Vec::new();
            for component in path.components() {
                let std::path::Component::Normal(name) = component else {
                    panic!(
                        "strategy source helper found unsupported path component: {}",
                        path.display()
                    );
                };
                let name = name.to_str().unwrap_or_else(|| {
                    panic!(
                        "strategy source helper found non-UTF-8 path: {}",
                        path.display()
                    )
                });
                assert!(
                    !name.contains('\\'),
                    "strategy source helper must reject backslash components: {}",
                    path.display()
                );
                parts.push(name.to_owned());
            }
            parts.join("/")
        }

        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for relative_root in registry_relative_roots(key) {
            let root = manifest_dir.join(relative_root);
            let root_type = std::fs::symlink_metadata(&root).unwrap().file_type();
            assert!(
                !root_type.is_symlink(),
                "{label} source helper must reject symlink roots: {}",
                root.display()
            );
            let root_label = normalized_path(std::path::Path::new(relative_root));
            if root_type.is_file() {
                files.push((root_label, root));
                continue;
            }
            assert!(
                root_type.is_dir(),
                "{label} source root must be a file or directory: {}",
                root.display()
            );

            let mut root_files = Vec::new();
            collect(&root, &mut root_files);
            for path in root_files {
                let relative = normalized_path(path.strip_prefix(&root).unwrap());
                files.push((format!("{root_label}/{relative}"), path));
            }
        }
        files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        files
    }

    fn strategy_source_files_in_canonical_order() -> Vec<(String, std::path::PathBuf)> {
        source_files_in_canonical_order(STRATEGY_KEY, "strategy")
    }

    fn outcome_group_source_files_in_canonical_order() -> Vec<(String, std::path::PathBuf)> {
        source_files_in_canonical_order(OUTCOME_GROUP_KEY, "outcome_group")
    }

    #[test]
    fn strategy_source_set_includes_wrapper_directory_and_shared_execution_modules() {
        assert_eq!(
            registry_relative_roots(STRATEGY_KEY),
            &[
                "src/strategies/binary_oracle_edge_taker",
                "src/strategies/complete_set_arbitrage",
                "src/bolt_v3_archetypes/binary_oracle_edge_taker.rs",
                "src/bolt_v3_archetypes/complete_set_arbitrage.rs",
                "src/bolt_v3_order_execution.rs",
                "src/bolt_v3_book_sizing.rs",
                "src/bolt_v3_binary_outcome_edge.rs",
                "src/bolt_v3_executable_cost.rs",
                "src/bolt_v3_sizing.rs",
                "src/bolt_v3_taker_updown_signal.rs",
            ]
        );
    }

    #[test]
    fn submit_admission_source_set_is_the_single_admission_module() {
        // Pins the generated constant for the third registry key, so a manifest
        // change to `[submit_admission]` fails this test the same way the strategy
        // and outcome-group sets are pinned (rather than only surfacing later as a
        // `registry_entry` panic).
        assert_eq!(
            registry_relative_roots(SUBMIT_ADMISSION_KEY),
            &["src/bolt_v3_submit_admission.rs"]
        );
    }

    #[test]
    fn outcome_group_source_set_includes_registered_outcome_group_roots() {
        assert_eq!(
            registry_relative_roots(OUTCOME_GROUP_KEY),
            &[
                "src/bolt_v3_atomic_io.rs",
                "src/bolt_v3_outcome_groups.rs",
                "src/bolt_v3_outcome_group_sources.rs",
                "src/bolt_v3_outcome_group_polymarket.rs",
                "src/bolt_v3_outcome_group_hyperliquid.rs",
                "src/bolt_v3_outcome_group_scanner.rs",
                "src/bolt_v3_basket_admission.rs",
                "src/bolt_v3_basket_execution.rs",
                "src/bolt_v3_basket_store.rs",
                "src/bolt_v3_archetypes/complete_set_arbitrage.rs",
                "src/bolt_v3_market_families/outcome_group.rs",
                "src/strategy_bindings.rs",
                "src/strategies/complete_set_arbitrage",
            ]
        );
    }

    #[test]
    fn complete_set_strategy_shell_overlap_is_intentional_and_pinned() {
        let complete_set_root = "src/strategies/complete_set_arbitrage";
        assert!(
            registry_relative_roots(STRATEGY_KEY).contains(&complete_set_root),
            "complete-set shell is a production-registered strategy and must stay under strategy source integrity"
        );
        assert!(
            registry_relative_roots(OUTCOME_GROUP_KEY).contains(&complete_set_root),
            "complete-set shell is the first outcome-group consumer and must stay under outcome-group source integrity"
        );
        let complete_set_archetype = "src/bolt_v3_archetypes/complete_set_arbitrage.rs";
        assert!(
            registry_relative_roots(STRATEGY_KEY).contains(&complete_set_archetype),
            "complete-set archetype produces registered strategy runtime config and must stay under strategy source integrity"
        );
        assert!(
            registry_relative_roots(OUTCOME_GROUP_KEY).contains(&complete_set_archetype),
            "complete-set archetype owns outcome-group runtime parameters and must stay under outcome-group source integrity"
        );
    }

    #[test]
    fn outcome_group_source_set_has_exact_first_slice_file_membership() {
        let files: Vec<String> = outcome_group_source_files_in_canonical_order()
            .into_iter()
            .map(|(relative, _path)| relative)
            .collect();
        assert_eq!(
            files,
            vec![
                "src/bolt_v3_archetypes/complete_set_arbitrage.rs".to_string(),
                "src/bolt_v3_atomic_io.rs".to_string(),
                "src/bolt_v3_basket_admission.rs".to_string(),
                "src/bolt_v3_basket_execution.rs".to_string(),
                "src/bolt_v3_basket_store.rs".to_string(),
                "src/bolt_v3_market_families/outcome_group.rs".to_string(),
                "src/bolt_v3_outcome_group_hyperliquid.rs".to_string(),
                "src/bolt_v3_outcome_group_polymarket.rs".to_string(),
                "src/bolt_v3_outcome_group_scanner.rs".to_string(),
                "src/bolt_v3_outcome_group_sources.rs".to_string(),
                "src/bolt_v3_outcome_groups.rs".to_string(),
                "src/strategies/complete_set_arbitrage/mod.rs".to_string(),
                "src/strategy_bindings.rs".to_string(),
            ],
            "Task 11 covers the HIP-4 normalizer root alongside shared outcome-group roots"
        );
    }

    #[test]
    fn production_text_for_strategy_directory_excludes_test_module_and_includes_selection() {
        // Source-set production-text boundary: the production text must equal the
        // per-file concatenation of every strategy source-set file's production
        // half, each empty when inner-cfg test-only or split at its own first
        // top-level test-module marker.
        let expected: String = strategy_source_files_in_canonical_order()
            .iter()
            .map(|(_relative, path)| {
                let text = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
                if text.starts_with(TEST_ONLY_INNER_CFG_MARKER) {
                    return String::new();
                }
                text.split(TEST_MODULE_SPLIT_MARKER)
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(production_module_source_text(STRATEGY_KEY), expected);
    }

    #[test]
    fn production_text_for_strategy_contains_relocated_selection_production_code() {
        // Regression guard for the directory-boundary bug: a naive
        // `split_once(MARKER)` over the JOINED text would drop everything sorted
        // after the marker-owning `mod.rs` — i.e. ALL of `selection.rs`. Assert a
        // production symbol that ONLY exists in `selection.rs` is present in the
        // production text, AND that `mod.rs`'s test module is still excluded.
        let production = production_module_source_text(STRATEGY_KEY);
        assert!(
            production.contains("fn selection_snapshot_from_instruments"),
            "production text must include selection.rs production code (relocated by A3)"
        );
        assert!(
            production.contains("fn outcome_on_execution_venue"),
            "production text must include the relocated venue-routing predicate"
        );
        assert!(
            production.contains("struct OutcomeBookState"),
            "production text must include the shared book-sizing state"
        );
        assert!(
            !production.contains("\n#[cfg(test)]\nmod tests"),
            "production text must exclude each file's top-level test module"
        );
    }

    #[test]
    fn whole_module_text_equals_concatenated_strategy_files() {
        // Source-set whole-text invariant: the whole-module text must equal every
        // strategy source-set file's full UTF-8 text concatenated in canonical
        // order, with the test modules retained.
        let expected: String = strategy_source_files_in_canonical_order()
            .iter()
            .map(|(_relative, path)| std::fs::read_to_string(path).unwrap())
            .collect();
        assert_eq!(module_source_text(STRATEGY_KEY), expected);
    }

    #[test]
    fn no_external_monolith_root_include_str_remains() {
        // No `src/` or `tests/` file may `include_str!` either monolith root via
        // an EXTERNAL path (e.g. "strategies/binary_oracle_edge_taker.rs",
        // "../src/strategies/binary_oracle_edge_taker.rs",
        // "../src/bolt_v3_submit_admission.rs").
        //
        // A0's KNOWN-AND-DEFERRED exception (the strategy file's own
        // self-`include_str!("binary_oracle_edge_taker.rs")`) is now RESOLVED by
        // A3/A8: the single file became the strategy directory module, so that
        // bare self-reference no longer exists. The strategy's in-module
        // outcome-suffix guard now uses `production_module_source_text`, so the
        // source boundary stays layout-independent through the registry. This
        // test therefore asserts NO monolith-root `include_str!` remains
        // anywhere — nothing scattered re-creeps back, and the deferred
        // self-reference is gone.
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let needles = [
            "include_str!(\"strategies/binary_oracle_edge_taker.rs\")",
            "include_str!(\"../src/strategies/binary_oracle_edge_taker.rs\")",
            "include_str!(\"bolt_v3_submit_admission.rs\")",
            "include_str!(\"../src/bolt_v3_submit_admission.rs\")",
        ];
        let mut offenders: Vec<String> = Vec::new();
        for dir in ["src", "tests"] {
            collect_offending_includes(&manifest.join(dir), &needles, &mut offenders);
        }
        assert!(
            offenders.is_empty(),
            "scattered monolith-root include_str! must not return: {offenders:?}"
        );
    }

    fn collect_offending_includes(
        dir: &std::path::Path,
        needles: &[&str],
        offenders: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_offending_includes(&path, needles, offenders);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for needle in needles {
                    if text.contains(needle) {
                        offenders.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
    }
}
