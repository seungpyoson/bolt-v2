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
    module_source_text as canonical_module_text, registry_entry, sha256_hex_lower,
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

/// A bound large enough to admit either gated root in the current single-file
/// layout (strategy is 732_776 bytes today). Used by the text accessors (whole
/// module / production text), where there is no operator-supplied cap.
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
/// with the bottom `#[cfg(test)] mod tests` submodule excluded.
///
/// IDENTITY case (single file today): reproduces the historical
/// `source.split("\n#[cfg(test)]\nmod tests").next()` output byte-for-byte — it
/// strips ONLY at the FIRST occurrence of the top-level test-module marker, so
/// the ~37 earlier inline `#[cfg(test)]` markers are retained (value-stability).
///
/// DIRECTORY case (post-split): the production/test boundary is "exclude the
/// file owning the top-level `#[cfg(test)] mod tests` and any file under a
/// test-only submodule" — NOT a blanket "drop test files". Today no gated root
/// is a directory, so that branch is exercised only by tests with synthetic
/// fixtures; the migrating split slice discharges the directory boundary
/// definition.
pub fn production_module_source_text(key: &str) -> String {
    let whole = module_source_text(key);
    match whole.split_once(TEST_MODULE_SPLIT_MARKER) {
        Some((production, _rest)) => production.to_string(),
        None => whole,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Golden digests captured live from `origin/main` raw bytes
    // (`git show origin/main:<path> | shasum -a 256`). These are exactly the
    // values the compile-time abort-plan gate enforces today; the new mechanism
    // MUST reproduce them with NO regeneration of any committed fixture.
    const GOLDEN_STRATEGY_DIGEST: &str =
        "0694f0a3830520bc01e7e354d8397d7aaaf5ad7c9243d10a2b5599b6d7fa9ac0";
    const GOLDEN_SUBMIT_ADMISSION_DIGEST: &str =
        "61428e39d55fa78d21f98414c083efc30e0ca737c90055f41d81523c96b2d4e9";

    // Bound comfortably above the strategy file size (732_776 bytes today).
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

    #[test]
    fn identity_digest_equals_raw_file_sha256() {
        // The single-file identity branch must equal a plain SHA-256 of the
        // file's raw bytes.
        let raw = std::fs::read(registry_root_path(STRATEGY_KEY)).unwrap();
        assert_eq!(
            registry_source_digest(STRATEGY_KEY, TEST_MAX_BYTES).unwrap(),
            sha256_hex_lower(&raw)
        );
    }

    #[test]
    fn one_byte_change_changes_strategy_digest() {
        // Control: hashing the raw bytes with a single byte flipped must differ
        // from the golden digest (the gate still detects tampering).
        let mut raw = std::fs::read(registry_root_path(STRATEGY_KEY)).unwrap();
        raw[0] ^= 0x01;
        assert_ne!(sha256_hex_lower(&raw), GOLDEN_STRATEGY_DIGEST);
    }

    #[test]
    fn production_text_reproduces_historical_split_for_strategy() {
        // Golden-TEXT: `production_module_source_text` in the identity case must
        // reproduce the current `.split("\n#[cfg(test)]\nmod tests").next()`
        // output byte-for-byte.
        let raw = std::fs::read_to_string(registry_root_path(STRATEGY_KEY)).unwrap();
        let expected = raw
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .unwrap()
            .to_string();
        assert_eq!(production_module_source_text(STRATEGY_KEY), expected);
    }

    #[test]
    fn whole_module_text_equals_full_file_for_strategy() {
        let raw = std::fs::read_to_string(registry_root_path(STRATEGY_KEY)).unwrap();
        assert_eq!(module_source_text(STRATEGY_KEY), raw);
    }

    #[test]
    fn registry_admits_current_strategy_file_size() {
        // The producer cap must admit the current single strategy file.
        const STRATEGY_FILE_BYTES: u64 = 732_776;
        const { assert!(TEST_MAX_BYTES >= STRATEGY_FILE_BYTES) };
        assert!(registry_source_digest(STRATEGY_KEY, STRATEGY_FILE_BYTES).is_ok());
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
        // The single KNOWN-AND-DEFERRED exception is the strategy file's own
        // in-file self-`include_str!("binary_oracle_edge_taker.rs")` in its
        // `#[cfg(test)] mod tests` block: removing it would change the gated
        // strategy bytes and break A0's value-stability guarantee, so the
        // self-reference is removed by A3 (which legitimately re-derives the
        // strategy digest when it splits the file). This test asserts that the
        // only remaining monolith-root `include_str!` is exactly that one
        // self-reference — nothing scattered re-creeps back.
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
