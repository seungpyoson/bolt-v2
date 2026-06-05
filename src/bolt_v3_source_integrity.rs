//! Single owner of source-integrity canonicalization, hashing, and text access
//! for the compile-time-embedded abort-plan gate sources.
//!
//! This module owns three things and is the ONLY place the two gated source
//! roots are named (the registry):
//!
//! 1. **The registry** — [`STRATEGY_KEY`] / [`SUBMIT_ADMISSION_KEY`] mapped to
//!    their repo-relative root paths.
//! 2. The canonicalization + hash primitives, re-exported from the
//!    `#[path]`-shared [`crate::source_canonicalization`] walk module so the
//!    build-time emission (`build.rs`) and the runtime digest share exactly one
//!    transcription.
//! 3. The text accessors [`module_source_text`] (whole-module text) and
//!    [`production_module_source_text`] (test-submodule-free text), both in the
//!    SAME canonicalization order as the digest.
//!
//! The verifier ([`crate::bolt_v3_tiny_canary_evidence`]) hashes the
//! compile-time-embedded canonical bytes (`$OUT_DIR/<key>.canonical`, produced
//! by `build.rs` from the SAME walk) — tamper-evidence preserved. The producer
//! ([`crate::bolt_v3_operator_artifacts`]) and every test call the registry-keyed
//! digest / text accessors here.

use std::io;
use std::path::{Path, PathBuf};

pub use crate::source_canonicalization::{
    GATED_SOURCE_ROOTS, GatedSourceRoot, STRATEGY_KEY, SUBMIT_ADMISSION_KEY,
    TEST_MODULE_SPLIT_MARKER, canonical_source_bytes, canonical_source_digest,
    module_source_text as canonical_module_text,
    production_module_source_text as canonical_production_module_text, registry_entry,
    sha256_hex_lower,
};

/// Repo-relative root path for a registry key (e.g. for test feeds / CLI args).
pub fn registry_relative_root(key: &str) -> &'static str {
    registry_entry(key).relative_root
}

/// Absolute repo path for a registry key, rooted at the crate manifest dir.
pub fn registry_root_path(key: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(registry_entry(key).relative_root)
}

/// Lowercase-hex SHA-256 of the canonical bytes of a registry root (file or
/// directory), bounded by `max_bytes`.
pub fn registry_source_digest(key: &str, max_bytes: u64) -> io::Result<String> {
    canonical_source_digest(&registry_root_path(key), max_bytes)
}

/// Canonical bytes of a registry root, bounded by `max_bytes`.
pub fn registry_source_bytes(key: &str, max_bytes: u64) -> io::Result<Vec<u8>> {
    canonical_source_bytes(&registry_root_path(key), max_bytes)
}

/// A bound large enough to admit either gated root: the submit_admission single
/// file and the strategy DIRECTORY (`{config.rs, mod.rs, selection.rs}`, whose
/// framed canonical stream is the raw content plus per-file path/length frames).
/// Used by the text accessors (whole module / production text), where there is
/// no operator-supplied cap.
///
/// Single source for the in-process text-accessor bound; the digest path uses
/// the operator-configured `max_source_bytes` instead.
const TEXT_ACCESSOR_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Whole-module source text for a registry key, in the same canonical order as
/// the digest.
pub fn module_source_text(key: &str) -> String {
    canonical_module_text(&registry_root_path(key), TEXT_ACCESSOR_MAX_BYTES)
        .unwrap_or_else(|error| panic!("module source text for `{key}` should read: {error}"))
}

/// Production-only module source text for a registry key: the whole-module text
/// with each file's bottom `#[cfg(test)] mod tests` submodule excluded.
///
/// Delegates to the SINGLE production/test boundary defined in
/// [`crate::source_canonicalization::production_module_source_text`].
///
/// IDENTITY case (e.g. `submit_admission`, a single file): reproduces the
/// historical `source.split("\n#[cfg(test)]\nmod tests").next()` output
/// byte-for-byte — strips ONLY at the FIRST top-level test-module marker, so the
/// earlier inline `#[cfg(test)]` markers are retained (value-stability).
///
/// DIRECTORY case (e.g. the strategy `{config.rs, mod.rs, selection.rs}` after
/// slice A8): the production half of EACH file — split independently at its own
/// first top-level marker — concatenated in canonical order. `mod.rs` contributes
/// its production half (its test module is excluded); `config.rs` and
/// `selection.rs` (production-only) contribute their whole text. This keeps every
/// submodule's production code in scope rather than dropping every file sorted
/// after the marker-owning file.
pub fn production_module_source_text(key: &str) -> String {
    canonical_production_module_text(&registry_root_path(key), TEXT_ACCESSOR_MAX_BYTES)
        .unwrap_or_else(|error| {
            panic!("production module source text for `{key}` should read: {error}")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden digests the compile-time abort-plan gate enforces.
    //
    // GOLDEN_SUBMIT_ADMISSION_DIGEST is unchanged from A0: it is the single-file
    // identity digest captured live from `origin/main` raw bytes
    // (`git show origin/main:<path> | shasum -a 256`); that root did not move.
    //
    // GOLDEN_STRATEGY_DIGEST was RE-DERIVED again by slice A5 after pricing
    // state moved out of `mod.rs`: the strategy source remains a directory
    // whose framed DIRECTORY concatenation is over
    // `{config.rs, mod.rs, selection.rs}` (sorted by relative path). This is a
    // legitimate behavior-preserving source move, not a fixture regeneration:
    // the value is re-derived from the live build-emitted
    // `OUT_DIR/strategy.canonical` and independently confirmed by hand-framing
    // the live source files (`shasum -a256 OUT_DIR/strategy.canonical` == this
    // constant).
    const GOLDEN_STRATEGY_DIGEST: &str =
        "8391ba33183078aa3c9139b786d9104346b7a11a52c413575b39afa7e483c319";
    const GOLDEN_SUBMIT_ADMISSION_DIGEST: &str =
        "61428e39d55fa78d21f98414c083efc30e0ca737c90055f41d81523c96b2d4e9";

    // Bound comfortably above the strategy directory canonical stream and the
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

    /// The `*.rs` files the strategy directory root resolves to, in strict
    /// canonical (relative-path-byte) order. Enumerated DYNAMICALLY with the
    /// same fail-closed symlink/backslash policy as `canonical_source_bytes`, so
    /// the invariant tracks the module as each slice adds a file (A3
    /// `selection.rs`, A8 `config.rs`, …) — no slice should ever edit this list.
    /// Current order: `config.rs` < `mod.rs` < `selection.rs` (by relative-path
    /// bytes).
    fn strategy_dir_files_in_canonical_order() -> Vec<std::path::PathBuf> {
        let root = registry_root_path(STRATEGY_KEY);
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
        fn relative_bytes(root: &std::path::Path, path: &std::path::Path) -> Vec<u8> {
            let relative = path.strip_prefix(root).unwrap();
            let mut parts = Vec::new();
            for component in relative.components() {
                let std::path::Component::Normal(name) = component else {
                    panic!(
                        "strategy source helper found unsupported path component: {}",
                        relative.display()
                    );
                };
                let name = name.to_str().unwrap_or_else(|| {
                    panic!(
                        "strategy source helper found non-UTF-8 path: {}",
                        relative.display()
                    )
                });
                assert!(
                    !name.contains('\\'),
                    "strategy source helper must reject backslash components: {}",
                    relative.display()
                );
                parts.push(name.to_owned());
            }
            parts.join("/").into_bytes()
        }
        let mut files = Vec::new();
        collect(&root, &mut files);
        files.sort_by(|a, b| relative_bytes(&root, a).cmp(&relative_bytes(&root, b)));
        files
    }

    #[test]
    fn strategy_root_is_a_directory() {
        // A3 converted the single strategy file into a directory module. The
        // registry must now point at a directory so the canonicalizer takes the
        // framed DIRECTORY branch (binary path+NUL+length frames), not the
        // verbatim IDENTITY branch.
        let root = registry_root_path(STRATEGY_KEY);
        assert!(
            root.is_dir(),
            "strategy registry root must be a directory after A3: {}",
            root.display()
        );
    }

    #[test]
    fn directory_digest_equals_hand_framed_canonical_over_strategy_files() {
        // DIRECTORY invariant (replaces the old single-file identity equality):
        // the strategy digest must equal a SHA-256 over the hand-built framed
        // stream `rel_path + 0x00 + u64-LE(len) + raw_bytes` for every `*.rs`
        // file under the directory, in canonical order. This pins the exact
        // framing the gate hashes — not a tautology against the accessor itself.
        let root = registry_root_path(STRATEGY_KEY);
        let mut expected: Vec<u8> = Vec::new();
        for path in strategy_dir_files_in_canonical_order() {
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            let raw = std::fs::read(&path).unwrap();
            expected.extend_from_slice(relative.as_bytes());
            expected.push(0x00);
            expected.extend_from_slice(&(raw.len() as u64).to_le_bytes());
            expected.extend_from_slice(&raw);
        }
        assert_eq!(
            registry_source_digest(STRATEGY_KEY, TEST_MAX_BYTES).unwrap(),
            sha256_hex_lower(&expected),
            "strategy directory digest must equal the hand-framed canonical stream"
        );
    }

    #[test]
    fn one_byte_change_anywhere_under_directory_changes_strategy_digest() {
        // Tamper-detection control for the directory case: flipping a single byte
        // in the hand-framed canonical stream of EACH file in turn must change the
        // digest away from the golden — proving every file under the directory is
        // covered, not just the first. (Operates on a copy of the framed bytes;
        // it never writes to the real source tree.)
        let root = registry_root_path(STRATEGY_KEY);
        let files = strategy_dir_files_in_canonical_order();
        assert!(
            files.len() >= 3,
            "expected current strategy directory source files"
        );

        // Build the framed stream and record each file's raw-byte span within it.
        let mut framed: Vec<u8> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for path in &files {
            let relative = path
                .strip_prefix(&root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
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
        // DIRECTORY production-text boundary (replaces the old single-file split
        // reproduction): the production text must equal the per-file concatenation
        // of every strategy file's production half (each split at its OWN first
        // top-level test-module marker), in canonical order. This pins the exact
        // boundary and proves `selection.rs` (a production-only file with no test
        // module) is INCLUDED whole, while `mod.rs`'s test module is excluded.
        let expected: String = strategy_dir_files_in_canonical_order()
            .iter()
            .map(|path| {
                let text = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
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
            !production.contains("\n#[cfg(test)]\nmod tests"),
            "production text must exclude each file's top-level test module"
        );
    }

    #[test]
    fn whole_module_text_equals_concatenated_strategy_files() {
        // DIRECTORY whole-text invariant (replaces the old single-file equality):
        // the whole-module text must equal every strategy file's full UTF-8 text
        // concatenated in canonical order, with the test modules retained.
        let expected: String = strategy_dir_files_in_canonical_order()
            .iter()
            .map(|path| std::fs::read_to_string(path).unwrap())
            .collect();
        assert_eq!(module_source_text(STRATEGY_KEY), expected);
    }

    #[test]
    fn registry_admits_current_strategy_directory_canonical_size() {
        // The producer cap must admit the current strategy DIRECTORY: the framed
        // canonical stream over {config.rs, mod.rs, selection.rs}. Compute its
        // exact length and assert the digest succeeds with a cap set to exactly
        // that size (and fails one byte below), proving the bound is tight and
        // meaningful.
        let canonical_len = registry_source_bytes(STRATEGY_KEY, TEST_MAX_BYTES)
            .unwrap()
            .len() as u64;
        let raw_len: u64 = strategy_dir_files_in_canonical_order()
            .iter()
            .map(|path| std::fs::metadata(path).unwrap().len())
            .sum();
        assert!(
            canonical_len > raw_len,
            "directory framing adds path/length frames over raw content"
        );
        assert!(registry_source_digest(STRATEGY_KEY, canonical_len).is_ok());
        assert!(
            registry_source_digest(STRATEGY_KEY, canonical_len - 1).is_err(),
            "cap one byte below the canonical length must reject"
        );
    }

    #[test]
    fn verifier_no_longer_self_includes_monolith_roots() {
        // The production verifier must NOT `include_str!` either monolith root
        // directly — it embeds the build-emitted canonical bytes instead. This
        // is the core no-dual-path regression guard for the gate itself.
        let verifier = std::fs::read_to_string(repo_path_from_manifest(
            "src/bolt_v3_tiny_canary_evidence.rs",
        ))
        .unwrap();
        assert!(
            !verifier.contains("include_str!(\"strategies/binary_oracle_edge_taker.rs\")"),
            "verifier must not self-include the strategy monolith root"
        );
        assert!(
            !verifier.contains("include_str!(\"bolt_v3_submit_admission.rs\")"),
            "verifier must not self-include the submit_admission monolith root"
        );
        assert!(
            verifier.contains("/strategy.canonical")
                && verifier.contains("/submit_admission.canonical"),
            "verifier must embed the build-emitted OUT_DIR canonical bytes"
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

    fn repo_path_from_manifest(relative: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
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
