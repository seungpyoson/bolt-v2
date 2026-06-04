//! Pure, dependency-light canonicalization of a *source root* (a single `.rs`
//! file OR a directory of `.rs` files) into a deterministic, layout-independent
//! byte stream, plus the ONE consolidated lowercase-hex SHA-256 primitive used
//! everywhere a source digest is needed.
//!
//! This file is the SINGLE TRANSCRIPTION of the walk + framing + hash logic. It
//! is compiled twice from one source: once by `build.rs` via a `#[path]` module
//! include (so the build script can re-emit the canonical bytes into `OUT_DIR`),
//! and once by the crate as a normal module re-exported through
//! [`crate::bolt_v3_source_integrity`]. Because both sides share this exact
//! source text, the build-time emission and the runtime digest can never drift.
//!
//! It depends ONLY on `std`, `sha2`, and `hex` so `build.rs` can compile it
//! standalone with `[build-dependencies] sha2 + hex` pinned to the same versions
//! as `[dependencies]`. It must NOT import anything from the rest of the crate.

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// The ONE consolidated lowercase-hex SHA-256 primitive. Every source-integrity
/// digest in the crate (verifier, producer, providers, tests) routes through
/// this. `hex::encode` and `format!("{digest:x}")` are byte-identical for a
/// 32-byte SHA-256 digest (both lowercase hex), so this is behavior-identical to
/// every helper it replaces.
pub fn sha256_hex_lower(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Read a regular file's raw bytes, erroring if it exceeds `max_bytes`.
///
/// Mirrors the bound semantics of the producer's `read_file_bounded`: read at
/// most `max_bytes + 1` and fail if the length exceeds the cap, so an
/// oversized file is rejected rather than silently truncated. This bounded
/// reader lives here (not borrowed from `bolt_v3_operator_artifacts`) because
/// the canonicalizer must be self-contained for `build.rs`.
fn read_file_bounded(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let length = bytes.len() as u64;
    if length > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source file exceeds max_source_bytes={max_bytes} bytes (length={length})"),
        ));
    }
    Ok(bytes)
}

/// Recursively collect every `*.rs` file under `dir`, sorted lexicographically
/// by relative-path raw UTF-8 bytes (locale/OS-independent), with path
/// components joined by `/` in the relative path used for ordering and framing.
/// Backslash bytes inside a component are rejected so Unix filenames like
/// `a\b.rs` cannot collide with `a/b.rs`. Returns
/// `(relative_path_bytes, absolute_path)` pairs in canonical order.
fn collect_rs_files_sorted(dir: &Path) -> io::Result<Vec<(Vec<u8>, PathBuf)>> {
    let mut out: Vec<(Vec<u8>, PathBuf)> = Vec::new();
    collect_rs_files_recursive(dir, dir, &mut out)?;
    // Sort on the relative-path raw UTF-8 bytes: deterministic and independent
    // of filesystem iteration order, locale, and OS.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect_rs_files_recursive(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(Vec<u8>, PathBuf)>,
) -> io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    // Sort raw paths for deterministic recursion order (final ordering is by
    // relative-path bytes; this only stabilizes traversal).
    entries.sort();
    for path in entries {
        let metadata = std::fs::symlink_metadata(&path)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            // Reject symlinks: a symlink could point outside the root and break
            // the layout-independence/tamper-evidence guarantees.
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("source root contains a symlink: {}", path.display()),
            ));
        }
        if file_type.is_dir() {
            collect_rs_files_recursive(root, &path, out)?;
        } else if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rs")
        {
            let relative = relative_path_bytes(root, &path)?;
            out.push((relative, path));
        }
    }
    Ok(())
}

/// Compute the relative path of `path` under `root`, as UTF-8 path components
/// joined by `/`. Errors if `path` is not under `root`, has non-UTF-8
/// components, or contains a backslash byte in any component.
fn relative_path_bytes(root: &Path, path: &Path) -> io::Result<Vec<u8>> {
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "source file {} is not under root {}",
                path.display(),
                root.display()
            ),
        )
    })?;
    let mut components = Vec::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "source relative path has unsupported component: {}",
                    relative.display()
                ),
            ));
        };
        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "source relative path is not valid UTF-8: {}",
                    relative.display()
                ),
            )
        })?;
        if name.contains('\\') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "source relative path component contains a backslash: {}",
                    relative.display()
                ),
            ));
        }
        components.push(name);
    }
    Ok(components.join("/").into_bytes())
}

/// Canonical byte stream for a source `root`.
///
/// - **IDENTITY case** (`root` is a regular file): the file's raw bytes verbatim
///   — no framing, no path prefix, no separator, no newline/BOM/trailing-newline
///   normalization. This is byte-identical to `include_str!(path).as_bytes()`
///   today, so the SHA-256 equals the currently-recorded digest (value-stable).
/// - **DIRECTORY case** (`root` is a directory): every `*.rs` file under it,
///   ordered lexicographically by relative-path raw UTF-8 bytes, each emitted as
///   a frame `relative_path_bytes + 0x00 + u64-LE(file_len) + file_raw_bytes`.
///   The path+NUL+length framing is collision-free and rename-sensitive. Per-file
///   reads obey `max_bytes`, and a running total-bytes ceiling (also `max_bytes`)
///   bounds the whole directory so it can never silently exceed today's cap.
pub fn canonical_source_bytes(root: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(root)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source root is a symlink: {}", root.display()),
        ));
    }
    if file_type.is_file() {
        // IDENTITY: verbatim raw bytes.
        return read_file_bounded(root, max_bytes);
    }
    if !file_type.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "source root is neither a regular file nor a directory: {}",
                root.display()
            ),
        ));
    }

    // DIRECTORY: framed concatenation in canonical order, with a running total
    // ceiling equal to the per-file cap.
    let files = collect_rs_files_sorted(root)?;
    let mut out: Vec<u8> = Vec::new();
    let mut total: u64 = 0;
    for (relative, path) in files {
        let file_bytes = read_file_bounded(&path, max_bytes)?;
        let file_len = file_bytes.len() as u64;
        total = total.saturating_add(relative.len() as u64);
        total = total.saturating_add(1); // NUL separator
        total = total.saturating_add(8); // u64-LE length frame
        total = total.saturating_add(file_len);
        if total > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "source directory canonical stream exceeds max_source_bytes={max_bytes} bytes"
                ),
            ));
        }
        out.extend_from_slice(&relative);
        out.push(0x00);
        out.extend_from_slice(&file_len.to_le_bytes());
        out.extend_from_slice(&file_bytes);
    }
    Ok(out)
}

/// Lowercase-hex SHA-256 of [`canonical_source_bytes`] for `root`.
pub fn canonical_source_digest(root: &Path, max_bytes: u64) -> io::Result<String> {
    Ok(sha256_hex_lower(&canonical_source_bytes(root, max_bytes)?))
}

/// Whole-module source text for a `root`, in the SAME canonicalization order as
/// the digest. IDENTITY case: the file's verbatim text. DIRECTORY case: the
/// framed-order concatenation of every file's UTF-8 text WITHOUT the binary
/// frame bytes (path/NUL/length) — i.e. just the file contents joined in
/// canonical order. There is exactly ONE order across the digest and the text
/// accessors.
pub fn module_source_text(root: &Path, max_bytes: u64) -> io::Result<String> {
    let metadata = std::fs::symlink_metadata(root)?;
    let file_type = metadata.file_type();
    if file_type.is_file() {
        let bytes = read_file_bounded(root, max_bytes)?;
        return utf8_string(bytes, root);
    }
    if !file_type.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "source root is neither a regular file nor a directory: {}",
                root.display()
            ),
        ));
    }
    let files = collect_rs_files_sorted(root)?;
    let mut text = String::new();
    for (_relative, path) in files {
        let bytes = read_file_bounded(&path, max_bytes)?;
        text.push_str(&utf8_string(bytes, &path)?);
    }
    Ok(text)
}

/// Production-only module source text for a `root`, in the SAME canonicalization
/// order as the digest, with the bottom `#[cfg(test)] mod tests` submodule
/// excluded.
///
/// This is the SINGLE definition of the production/test boundary for both the
/// IDENTITY and DIRECTORY cases.
///
/// - **IDENTITY case** (single file): the text up to (excluding) the FIRST
///   top-level [`TEST_MODULE_SPLIT_MARKER`], i.e. byte-for-byte the historical
///   `source.split("\n#[cfg(test)]\nmod tests").next()` output. A file with no
///   marker contributes its whole text. The ~37 earlier inline `#[cfg(test)]`
///   markers are retained (they are not the top-level test-module marker).
/// - **DIRECTORY case** (post-split, e.g. `{mod.rs, selection.rs}`): the
///   production half of EACH `*.rs` file — each split independently at its OWN
///   first top-level marker — concatenated in canonical (relative-path-byte)
///   order. A file owning the top-level `#[cfg(test)] mod tests` (e.g. `mod.rs`)
///   contributes only its production half; a file with no marker (e.g.
///   `selection.rs`, a production-only submodule) contributes its whole text.
///   This is NOT a `split_once` over the joined text — that would drop every
///   file sorted after the marker-owning file (`selection.rs` after `mod.rs`)
///   and silently shrink the production surface. Splitting per file keeps every
///   submodule's production code in scope while still excluding each file's own
///   test module. A future test-ONLY submodule file (whose entire content is a
///   test module) would be a separate concern; today no gated directory contains
///   one, and any such file would be split at its own marker like the rest.
pub fn production_module_source_text(root: &Path, max_bytes: u64) -> io::Result<String> {
    let metadata = std::fs::symlink_metadata(root)?;
    let file_type = metadata.file_type();
    if file_type.is_file() {
        let bytes = read_file_bounded(root, max_bytes)?;
        let text = utf8_string(bytes, root)?;
        return Ok(production_half(&text));
    }
    if !file_type.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "source root is neither a regular file nor a directory: {}",
                root.display()
            ),
        ));
    }
    let files = collect_rs_files_sorted(root)?;
    let mut text = String::new();
    for (_relative, path) in files {
        let bytes = read_file_bounded(&path, max_bytes)?;
        let file_text = utf8_string(bytes, &path)?;
        text.push_str(&production_half(&file_text));
    }
    Ok(text)
}

/// The production half of a single file's text: everything before the FIRST
/// top-level [`TEST_MODULE_SPLIT_MARKER`], or the whole text if the marker is
/// absent. LF checkout remains byte-for-byte equivalent to
/// `text.split(MARKER).next().unwrap()`. CRLF checkout normalizes to LF first so
/// the marker is OS-independent.
fn production_half(text: &str) -> String {
    let normalized = text.replace(CRLF_LINE_ENDING, LF_LINE_ENDING);
    match normalized.split_once(TEST_MODULE_SPLIT_MARKER) {
        Some((production, _rest)) => production.to_string(),
        None => normalized,
    }
}

fn utf8_string(bytes: Vec<u8>, path: &Path) -> io::Result<String> {
    String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source file is not valid UTF-8: {}", path.display()),
        )
    })
}

/// The exact 3-line marker that separates the top-level production code from the
/// bottom `#[cfg(test)] mod tests` module in a single-file Rust source. Defined
/// in exactly one place so the identity-case production boundary matches the
/// historical `.split(...).next()` convention byte-for-byte.
pub const TEST_MODULE_SPLIT_MARKER: &str = "\n#[cfg(test)]\nmod tests";
const CRLF_LINE_ENDING: &str = "\r\n";
const LF_LINE_ENDING: &str = "\n";

/// Stable registry key for the binary-oracle strategy source root.
pub const STRATEGY_KEY: &str = "strategy";
/// Stable registry key for the submit-admission source root.
pub const SUBMIT_ADMISSION_KEY: &str = "submit_admission";

/// One registry entry: a stable key + its repo-relative root path. The root may
/// resolve to a single `.rs` file (today) or a directory (after a split); the
/// canonicalizer decides at runtime.
pub struct GatedSourceRoot {
    pub key: &'static str,
    /// Repo-relative path from the crate manifest dir.
    pub relative_root: &'static str,
}

/// THE registry — the ONLY place the two gated source roots are named. Lives in
/// this `#[path]`-shared pure file so `build.rs` (which embeds the canonical
/// bytes) and the runtime integrity owner reference the SAME list with no
/// duplicated file list. `build.rs`, the verifier, the producer, and tests all
/// resolve roots through this registry.
pub const GATED_SOURCE_ROOTS: &[GatedSourceRoot] = &[
    GatedSourceRoot {
        key: STRATEGY_KEY,
        relative_root: "src/strategies/binary_oracle_edge_taker",
    },
    GatedSourceRoot {
        key: SUBMIT_ADMISSION_KEY,
        relative_root: "src/bolt_v3_submit_admission.rs",
    },
];

/// Look up a registry entry by key, panicking on an unknown key.
pub fn registry_entry(key: &str) -> &'static GatedSourceRoot {
    GATED_SOURCE_ROOTS
        .iter()
        .find(|entry| entry.key == key)
        .unwrap_or_else(|| panic!("unknown gated source registry key: {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "source_canon_{}_{}_{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn identity_branch_is_verbatim_raw_bytes() {
        let dir = temp_dir("identity");
        let file = dir.join("only.rs");
        let raw = b"fn a() {}\r\n\xEF\xBB\xBFno_trailing_newline".to_vec();
        fs::write(&file, &raw).unwrap();
        let canonical = canonical_source_bytes(&file, 1_000_000).unwrap();
        assert_eq!(canonical, raw, "identity branch must be verbatim raw bytes");
        assert_eq!(
            canonical_source_digest(&file, 1_000_000).unwrap(),
            sha256_hex_lower(&raw)
        );
    }

    #[test]
    fn directory_branch_is_deterministic_and_order_independent() {
        let dir = temp_dir("det");
        fs::write(dir.join("b.rs"), b"second").unwrap();
        fs::write(dir.join("a.rs"), b"first").unwrap();
        let d1 = canonical_source_digest(&dir, 1_000_000).unwrap();
        let d2 = canonical_source_digest(&dir, 1_000_000).unwrap();
        assert_eq!(d1, d2, "directory digest must be deterministic");

        // Order-independence: a second dir with the files created in the
        // opposite filesystem order must hash identically.
        let dir2 = temp_dir("det2");
        fs::write(dir2.join("a.rs"), b"first").unwrap();
        fs::write(dir2.join("b.rs"), b"second").unwrap();
        assert_eq!(
            d1,
            canonical_source_digest(&dir2, 1_000_000).unwrap(),
            "directory digest must be input-order-independent"
        );
    }

    #[test]
    fn directory_branch_detects_one_byte_change() {
        let dir = temp_dir("bytechange");
        fs::write(dir.join("a.rs"), b"first").unwrap();
        fs::write(dir.join("b.rs"), b"second").unwrap();
        let before = canonical_source_digest(&dir, 1_000_000).unwrap();
        fs::write(dir.join("b.rs"), b"sec0nd").unwrap();
        let after = canonical_source_digest(&dir, 1_000_000).unwrap();
        assert_ne!(before, after, "a 1-byte change must change the digest");
    }

    #[test]
    fn directory_branch_detects_file_rename() {
        let dir = temp_dir("rename");
        fs::write(dir.join("a.rs"), b"first").unwrap();
        fs::write(dir.join("b.rs"), b"second").unwrap();
        let before = canonical_source_digest(&dir, 1_000_000).unwrap();

        let dir2 = temp_dir("rename2");
        fs::write(dir2.join("a.rs"), b"first").unwrap();
        fs::write(dir2.join("c.rs"), b"second").unwrap(); // b.rs -> c.rs, same content
        let after = canonical_source_digest(&dir2, 1_000_000).unwrap();
        assert_ne!(
            before, after,
            "a file rename must change the digest (path is framed)"
        );
    }

    #[test]
    fn directory_branch_recurses_into_subdirs() {
        let dir = temp_dir("recurse");
        fs::write(dir.join("a.rs"), b"top").unwrap();
        let sub = dir.join("nested");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("z.rs"), b"deep").unwrap();
        // Non-.rs files are ignored.
        fs::write(dir.join("ignore.txt"), b"nope").unwrap();
        let digest = canonical_source_digest(&dir, 1_000_000).unwrap();

        // Build the expected stream by hand in canonical order:
        // "a.rs" then "nested/z.rs".
        let mut expected: Vec<u8> = Vec::new();
        for (rel, content) in [("a.rs", &b"top"[..]), ("nested/z.rs", &b"deep"[..])] {
            expected.extend_from_slice(rel.as_bytes());
            expected.push(0x00);
            expected.extend_from_slice(&(content.len() as u64).to_le_bytes());
            expected.extend_from_slice(content);
        }
        assert_eq!(digest, sha256_hex_lower(&expected));
    }

    #[test]
    #[cfg(unix)]
    fn directory_branch_rejects_backslash_path_component() {
        let dir = temp_dir("backslash");
        fs::write(dir.join("a\\b.rs"), b"same").unwrap();

        let error = canonical_source_bytes(&dir, 1_000_000).unwrap_err();

        assert!(
            error.to_string().contains("backslash"),
            "backslash path components must fail loudly, got: {error}"
        );
    }

    #[test]
    fn module_source_text_directory_joins_in_canonical_order() {
        let dir = temp_dir("text");
        fs::write(dir.join("b.rs"), b"BBB").unwrap();
        fs::write(dir.join("a.rs"), b"AAA").unwrap();
        let text = module_source_text(&dir, 1_000_000).unwrap();
        assert_eq!(text, "AAABBB", "text joins by relative-path byte order");
    }

    #[test]
    fn production_text_normalizes_crlf_before_test_module_split() {
        let dir = temp_dir("crlf");
        let file = dir.join("only.rs");
        fs::write(
            &file,
            b"fn production() {}\r\n#[cfg(test)]\r\nmod tests { fn ignored() {} }\r\n",
        )
        .unwrap();

        let production = production_module_source_text(&file, 1_000_000).unwrap();

        assert_eq!(production, "fn production() {}");
    }

    #[test]
    fn per_file_cap_rejects_oversized_file() {
        let dir = temp_dir("cap");
        let file = dir.join("big.rs");
        fs::write(&file, vec![b'x'; 100]).unwrap();
        assert!(canonical_source_bytes(&file, 50).is_err());
    }
}
