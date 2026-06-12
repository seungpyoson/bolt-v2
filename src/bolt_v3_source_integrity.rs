//! Single owner of source-integrity canonicalization, hashing, and text access
//! for the compile-time-embedded abort-plan gate sources.
//!
//! This module owns three things and is the ONLY place the two gated source
//! roots are named (the registry):
//!
//! 1. **The registry** — [`STRATEGY_KEY`] / [`SUBMIT_ADMISSION_KEY`] mapped to
//!    their repo-relative source root sets.
//! 2. The canonicalization + hash primitives, re-exported from the
//!    `#[path]`-shared [`crate::source_canonicalization`] walk module so the
//!    build-time emission (`build.rs`) and the runtime digest share exactly one
//!    transcription.
//! 3. The text accessors [`module_source_text`] (whole-module text) and
//!    [`production_module_source_text`] (test-submodule-free text), both in the
//!    SAME canonicalization order as the digest.
//!
//! `build.rs` emits compile-time canonical bytes (`$OUT_DIR/<key>.canonical`)
//! from the SAME walk. Tests and provider artifact helpers call the
//! registry-keyed digest / text accessors here.

use std::io;
use std::path::{Path, PathBuf};

pub use crate::source_canonicalization::{
    GATED_SOURCE_ROOTS, GatedSourceRoot, STRATEGY_KEY, SUBMIT_ADMISSION_KEY,
    TEST_MODULE_SPLIT_MARKER, TEST_ONLY_INNER_CFG_MARKER, canonical_source_bytes,
    canonical_source_digest, canonical_source_set_bytes, canonical_source_set_digest,
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
/// registry-keyed hashing/text helpers use every root in the set.
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

/// Lowercase-hex SHA-256 of the canonical bytes of a registry source set,
/// bounded by `max_bytes`.
pub fn registry_source_digest(key: &str, max_bytes: u64) -> io::Result<String> {
    canonical_source_set_digest(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        registry_relative_roots(key),
        max_bytes,
    )
}

/// Canonical bytes of a registry source set, bounded by `max_bytes`.
pub fn registry_source_bytes(key: &str, max_bytes: u64) -> io::Result<Vec<u8>> {
    canonical_source_set_bytes(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        registry_relative_roots(key),
        max_bytes,
    )
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
/// file and the strategy source set (strategy directory plus shared execution
/// sources, whose framed canonical stream is the raw content plus per-file
/// path/length frames). Used by the text accessors (whole module / production
/// text), where there is no operator-supplied cap.
///
/// Single source for the in-process text-accessor bound; the digest path uses
/// the operator-configured `max_source_bytes` instead.
const TEXT_ACCESSOR_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Whole-module source text for a registry key, in the same canonical order as
/// the digest.
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

    // Golden digests the compile-time abort-plan gate enforces.
    //
    // GOLDEN_STRATEGY_DIGEST is re-derived only when an accepted source move
    // changes the canonical strategy source set. A4 moves book state and
    // VWAP/slippage sizing out of the strategy wrapper, so the strategy source
    // set is now the strategy directory plus shared execution helper modules.
    //
    // Re-derived by A6 after moving the exposure state machine into
    // `src/strategies/binary_oracle_edge_taker/exposure.rs`; the dynamic source
    // root walk includes the new file in canonical order.
    //
    // Re-derived again by #581 after the binary-oracle runtime parser started
    // rejecting malformed configured instrument ids fail-loud. The value below
    // must match the live build-emitted `OUT_DIR/strategy.canonical` and the
    // independent hand-framed source-set stream in these tests.
    //
    // Re-derived again by A9 after the strategy wrapper delegates submit
    // admission request construction to `src/bolt_v3_submit_admission.rs`.
    // Re-derived once more after the exit-position input borrows identifiers
    // through the shared builder in response to exact-head PR review comments
    // and the follow-up clippy cleanup of that borrow bridge.
    //
    // Re-derived again after merging A9 with #581's configured-instrument
    // parser rejection in the same canonical strategy source set.
    //
    // Re-derived again by A10 after moving the binary-oracle-edge-taker embedded
    // test inventory into inner-cfg test-only split files under
    // `src/strategies/binary_oracle_edge_taker/tests/`.
    //
    // Re-derived again by #574 after scoped taker config and trade-flow
    // hardening changed the canonical strategy source set.
    //
    // Re-derived again by #574 review cleanup after replacing the remaining raw
    // positive integer guard in taker config parsing.
    //
    // Re-derived again by #574 external-review cleanup after unifying the
    // positive integer guard loop and aligning the test-only reference snapshot
    // helper with the production reference timestamp watermark.
    //
    // Re-derived again by #574 follow-up review cleanup after decoupling stale
    // snapshot fair-value rejection from test-only signal candidate processing.
    //
    // Re-derived again by #520 fresh post-#574 replacement after extracting the
    // signed trade-flow buffer into `src/bolt_v3_trade_flow.rs` while keeping
    // the strategy-local config projection in the canonical strategy source set.
    // Re-derived again by #579 after rebasing the retired evidence-gate removal
    // and review cleanup over #574's strategy source-set hardening.
    // Re-derived once more after the exact PR merge-head source-integrity check
    // exposed the stale intermediate digest from conflict resolution.
    // Re-derived again after removing vestigial retired gate arming constructor
    // terminology from submit-admission construction and its call sites.
    // Re-derived again after merging current main's trade-flow extraction into
    // the #579 retired-gate removal head.
    // Re-derived again after moving surfaced realized-volatility bindings and
    // source forwarding into the strategy source set.
    // Re-derived again after spelling the realized-volatility binding insertion
    // as an explicit entry match to satisfy both clippy and source-fence.
    // Re-derived again after refreshing surfaced realized-volatility snapshots
    // before entry/exit pricing so stale sources block instead of freezing RV.
    // Re-derived again after preserving signal-data bindings in surfaced RV
    // mode while keeping legacy strategy-owned RV knobs rejected.
    // Re-derived again after forwarding duplicate stream bindings to every
    // configured surfaced realized-volatility source ID.
    // Re-derived again after accepting zero-RV surfaced snapshots and exporting
    // unknown-source surfaced RV rejection counters in strategy input evidence.
    // Re-derived again after allowing zero-RV strategy evidence/position paths
    // and adding disabled-source surfaced RV diagnostics coverage.
    // Re-derived again after removing the strategy-owned internal realized
    // volatility estimator and requiring surfaced RV for taker pricing.
    // Re-derived again after recording RV-not-ready pricing blocks in strategy
    // input evidence and forwarding disabled surfaced RV sources for audit
    // without subscribing disabled-only source bindings.
    // Re-derived again after fail-closing rejected surfaced RV engine configs
    // without retaining source bindings.
    // Re-derived again after root-level surfaced RV readiness redesign moved
    // raw RV validation behind the shared ready snapshot accessor.
    // Re-derived again after removing the dead strategy evidence fallback RV
    // parameter, then after adding executable-edge cost breakdown to the gated
    // strategy source set and aligning its strategy test coverage.
    // Re-derived again after selected-side sized-notional executable-edge
    // re-evaluation started recomputing the uncertainty band from the sized
    // limit-price fee.
    // Re-derived after deriving VWAP assertion expectations from test inputs.
    // Re-derived after scaling the entry limit-notional float tolerance by
    // sized notional.
    // Re-derived after executable-edge review follow-ups tightened config
    // validation, reused exact-size VWAP probes, and surfaced edge-threshold
    // pricing blocks.
    // Re-derived after exact-head review follow-ups made negative threshold
    // checks authoritative and omitted unavailable executable-cost EV evidence.
    // Re-derived after moving notional float tolerance into the shared numeric
    // helper to remove duplicate tolerance definitions from the strategy source
    // set.
    // Re-derived after executable-edge review cleanup made non-positive net
    // edge untradeable regardless of a negative configured threshold.
    // Re-derived after exact-head review follow-ups blocked sell-side
    // executable-edge economics, single-sourced typed entry-shape checks, and
    // carried preliminary fee uncertainty into sized re-evaluation.
    // Re-derived after follow-up cleanup separated sized executable-edge
    // evidence, probed executable fees at VWAP, coupled slippage/depth config,
    // labeled executable-edge Debug output, and named notional tolerance.
    // Re-derived after splitting executable cost from binary outcome edge math
    // and moving entry limit-notional guarding into shared submit admission.
    // Re-derived after review cleanup single-sourced the cents-per-share unit
    // conversion in shared numeric helpers.
    // Re-derived after adding position-sizer submit admission fixture coverage
    // under the binary oracle edge taker strategy test source set.
    const GOLDEN_STRATEGY_DIGEST: &str =
        "078eeef3a5948fef94430c1ae6399efbd0e957048441b3c43f592d521a0db985";
    // GOLDEN_SUBMIT_ADMISSION_DIGEST is re-derived by A9 after moving submit
    // admission request construction and valuation out of the strategy wrapper,
    // then again after borrowing exit-position identifiers through the builder.
    // Re-derived again by #579 after deleting retired evidence-gate source.
    // Re-derived again after removing a stale deleted-executor comment.
    // Re-derived again after removing vestigial retired gate arming constructor
    // terminology from submit-admission construction.
    // Re-derived again after adding checked fee-inclusive admission arithmetic.
    // Re-derived after moving entry limit-notional guarding into shared submit
    // admission.
    // Re-derived after adding position-sizer submit admission state and evidence
    // to the shared submit admission module.
    const GOLDEN_SUBMIT_ADMISSION_DIGEST: &str =
        "d522f06088751957b9fad7872f90801ff27500c6a91480bef2b3c1e17ed04e18";

    // Bound comfortably above the strategy source-set canonical stream and the
    // submit_admission single file.
    const TEST_MAX_BYTES: u64 = 8 * 1024 * 1024;

    #[test]
    fn value_stability_strategy_digest_equals_golden_constant() {
        let digest = registry_source_digest(STRATEGY_KEY, TEST_MAX_BYTES).unwrap();
        assert_eq!(
            digest, GOLDEN_STRATEGY_DIGEST,
            "strategy canonical digest must equal the recorded golden constant (no regeneration)"
        );
    }

    #[test]
    fn value_stability_submit_admission_digest_equals_golden_constant() {
        let digest = registry_source_digest(SUBMIT_ADMISSION_KEY, TEST_MAX_BYTES).unwrap();
        assert_eq!(
            digest, GOLDEN_SUBMIT_ADMISSION_DIGEST,
            "submit_admission canonical digest must equal the recorded golden constant"
        );
    }

    /// The strategy source-set files, in strict repo-relative-path-byte order.
    /// Enumerated dynamically with the same fail-closed symlink/backslash policy
    /// as the production canonicalizer, so the invariant tracks accepted source
    /// moves without hardcoding a file list.
    fn strategy_source_files_in_canonical_order() -> Vec<(String, std::path::PathBuf)> {
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
        for relative_root in registry_relative_roots(STRATEGY_KEY) {
            let root = manifest_dir.join(relative_root);
            let root_type = std::fs::symlink_metadata(&root).unwrap().file_type();
            assert!(
                !root_type.is_symlink(),
                "strategy source helper must reject symlink roots: {}",
                root.display()
            );
            let root_label = normalized_path(std::path::Path::new(relative_root));
            if root_type.is_file() {
                files.push((root_label, root));
                continue;
            }
            assert!(
                root_type.is_dir(),
                "strategy source root must be a file or directory: {}",
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

    #[test]
    fn strategy_source_set_includes_wrapper_directory_and_shared_execution_modules() {
        assert_eq!(
            registry_relative_roots(STRATEGY_KEY),
            &[
                "src/strategies/binary_oracle_edge_taker",
                "src/bolt_v3_book_sizing.rs",
                "src/bolt_v3_binary_outcome_edge.rs",
                "src/bolt_v3_executable_cost.rs",
            ]
        );
    }

    #[test]
    fn source_set_digest_equals_hand_framed_canonical_over_strategy_files() {
        // Source-set invariant: the strategy digest must equal a SHA-256 over the
        // hand-built framed stream `repo_rel_path + 0x00 + u64-LE(len) + raw_bytes`
        // for every file in the strategy source set, in canonical order. This
        // pins the exact framing the gate hashes — not a tautology against the
        // accessor itself.
        let mut expected: Vec<u8> = Vec::new();
        for (relative, path) in strategy_source_files_in_canonical_order() {
            let raw = std::fs::read(&path).unwrap();
            expected.extend_from_slice(relative.as_bytes());
            expected.push(0x00);
            expected.extend_from_slice(&(raw.len() as u64).to_le_bytes());
            expected.extend_from_slice(&raw);
        }
        assert_eq!(
            registry_source_digest(STRATEGY_KEY, TEST_MAX_BYTES).unwrap(),
            sha256_hex_lower(&expected),
            "strategy source-set digest must equal the hand-framed canonical stream"
        );
    }

    #[test]
    fn one_byte_change_anywhere_in_source_set_changes_strategy_digest() {
        // Tamper-detection control for the directory case: flipping a single byte
        // in the hand-framed canonical stream of EACH file in turn must change the
        // digest away from the golden — proving every file in the source set is
        // covered, not just the first. (Operates on a copy of the framed bytes;
        // it never writes to the real source tree.)
        let files = strategy_source_files_in_canonical_order();
        assert!(
            files.len() >= 4,
            "expected current strategy directory plus shared execution source files"
        );

        // Build the framed stream and record each file's raw-byte span within it.
        let mut framed: Vec<u8> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for (relative, path) in &files {
            let raw = std::fs::read(path).unwrap();
            framed.extend_from_slice(relative.as_bytes());
            framed.push(0x00);
            framed.extend_from_slice(&(raw.len() as u64).to_le_bytes());
            let start = framed.len();
            framed.extend_from_slice(&raw);
            spans.push((start, framed.len()));
        }
        // Sanity: the unmodified framed stream reproduces the golden.
        assert_eq!(sha256_hex_lower(&framed), GOLDEN_STRATEGY_DIGEST);

        for (start, end) in spans {
            let mut tampered = framed.clone();
            tampered[start] ^= 0x01; // flip first byte of this file's content
            assert_ne!(
                sha256_hex_lower(&tampered),
                GOLDEN_STRATEGY_DIGEST,
                "a 1-byte change in the file spanning [{start}, {end}) must change the digest"
            );
        }
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
    fn registry_admits_current_strategy_directory_canonical_size() {
        // The producer cap must admit the current strategy source set. Compute
        // its exact length and assert the digest succeeds with a cap set to
        // exactly that size (and fails one byte below), proving the bound is
        // tight and meaningful.
        let canonical_len = registry_source_bytes(STRATEGY_KEY, TEST_MAX_BYTES)
            .unwrap()
            .len() as u64;
        let raw_len: u64 = strategy_source_files_in_canonical_order()
            .iter()
            .map(|(_relative, path)| std::fs::metadata(path).unwrap().len())
            .sum();
        assert!(
            canonical_len > raw_len,
            "source-set framing adds path/length frames over raw content"
        );
        assert!(registry_source_digest(STRATEGY_KEY, canonical_len).is_ok());
        assert!(
            registry_source_digest(STRATEGY_KEY, canonical_len - 1).is_err(),
            "cap one byte below the canonical length must reject"
        );
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
