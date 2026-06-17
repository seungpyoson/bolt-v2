//! Pure, dependency-light canonicalization of a *source root* (a single `.rs`
//! file OR a directory of `.rs` files) into deterministic, layout-independent
//! source *text* (whole-module and production-only/test-stripped variants), plus
//! the ONE consolidated lowercase-hex SHA-256 primitive ([`sha256_hex_lower`])
//! still used by provider artifact code.
//!
//! This file is the SINGLE TRANSCRIPTION of the walk + text-extraction logic. It
//! is compiled as a normal crate module re-exported through
//! [`crate::bolt_v3_source_integrity`], which owns the registry-keyed text
//! accessors layered on top of it. (The binary framing + source-digest functions
//! this file used to own were removed with the golden-digest gate; only
//! [`sha256_hex_lower`] remains of the hashing surface.)
//!
//! It depends only on `std`, `sha2`, and `hex` and keeps its dependency surface
//! minimal so the canonicalization transcription stays isolated and easy to
//! audit. (It formerly avoided every `crate::` import because `build.rs`
//! compiled this file standalone via `#[path]`; build.rs now parses
//! `gated_source_roots.manifest` directly and no longer includes this module, so
//! that isolation is a design choice rather than a hard build constraint.)

use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

/// The ONE consolidated lowercase-hex SHA-256 primitive, used by the provider
/// artifact hashes (and tests). `hex::encode` and `format!("{digest:x}")` are
/// byte-identical for a 32-byte SHA-256 digest (both lowercase hex), so this is
/// behavior-identical to every helper it replaces.
pub fn sha256_hex_lower(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Read a regular file's raw bytes, erroring if it exceeds `max_bytes`.
///
/// Mirrors the bound semantics of the producer's `read_file_bounded`: read at
/// most `max_bytes + 1` and fail if the length exceeds the cap, so an
/// oversized file is rejected rather than silently truncated. This bounded
/// reader is a small local helper rather than a shared import from
/// `bolt_v3_operator_artifacts`, keeping this module's dependency surface
/// minimal.
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
/// components joined by `/` in the relative path used for ordering.
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

fn source_root_file_type(root: &Path) -> io::Result<std::fs::FileType> {
    let metadata = std::fs::symlink_metadata(root)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source root is a symlink: {}", root.display()),
        ));
    }
    Ok(file_type)
}

/// Whole-module source text for a `root`, in canonical file order. IDENTITY
/// case: the file's verbatim text. DIRECTORY case: every file's UTF-8 text
/// concatenated in canonical order (raw file contents, no separators).
pub fn module_source_text(root: &Path, max_bytes: u64) -> io::Result<String> {
    let file_type = source_root_file_type(root)?;
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

/// Whole-module source text for a registry-owned source set, in the canonical
/// repo-relative-path file order shared by every source-set accessor.
pub fn module_source_set_text(
    manifest_dir: &Path,
    relative_roots: &[&str],
    max_bytes: u64,
) -> io::Result<String> {
    if let Some(root) = single_source_set_root(manifest_dir, relative_roots)? {
        return module_source_text(&root, max_bytes);
    }

    let files = collect_source_set_files_sorted(manifest_dir, relative_roots)?;
    let mut text = String::new();
    for (_relative, path) in files {
        let bytes = read_file_bounded(&path, max_bytes)?;
        text.push_str(&utf8_string(bytes, &path)?);
    }
    Ok(text)
}

/// Production-only module source text for a `root`, in canonical file order,
/// with the bottom `#[cfg(test)] mod tests` submodule excluded.
///
/// This is the SINGLE definition of the production/test boundary for both the
/// IDENTITY and DIRECTORY cases.
///
/// - **IDENTITY case** (single file): the text up to (excluding) the FIRST
///   top-level [`TEST_MODULE_SPLIT_MARKER`], i.e. byte-for-byte the historical
///   `source.split("\n#[cfg(test)]\nmod tests").next()` output. A file with no
///   marker contributes its whole text. The ~37 earlier inline `#[cfg(test)]`
///   markers are retained (they are not the top-level test-module marker).
/// - **DIRECTORY case** (post-split, e.g. `{config.rs, mod.rs, selection.rs}`):
///   the production half of EACH `*.rs` file — each split independently at its
///   OWN first top-level marker — concatenated in canonical (relative-path-byte)
///   order. A file owning the top-level `#[cfg(test)] mod tests` (e.g. `mod.rs`)
///   contributes only its production half; a file with no marker (e.g.
///   `config.rs` or `selection.rs`, production-only submodules) contributes its
///   whole text.
///   This is NOT a `split_once` over the joined text — that would drop every
///   file sorted after the marker-owning file (`selection.rs` after `mod.rs`)
///   and silently shrink the production surface. Splitting per file keeps every
///   submodule's production code in scope while still excluding each file's own
///   test module. A test-only split file that starts with `#![cfg(test)]`
///   contributes empty production text, which keeps ownership-bucket test files
///   out of source-integrity and runtime-literal production scans.
pub fn production_module_source_text(root: &Path, max_bytes: u64) -> io::Result<String> {
    let file_type = source_root_file_type(root)?;
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

/// Production-only source text for a registry-owned source set, in the canonical
/// repo-relative-path file order shared by every source-set accessor.
pub fn production_source_set_text(
    manifest_dir: &Path,
    relative_roots: &[&str],
    max_bytes: u64,
) -> io::Result<String> {
    if let Some(root) = single_source_set_root(manifest_dir, relative_roots)? {
        return production_module_source_text(&root, max_bytes);
    }

    let files = collect_source_set_files_sorted(manifest_dir, relative_roots)?;
    let mut text = String::new();
    for (_relative, path) in files {
        let bytes = read_file_bounded(&path, max_bytes)?;
        let file_text = utf8_string(bytes, &path)?;
        text.push_str(&production_half(&file_text));
    }
    Ok(text)
}

fn single_source_set_root(
    manifest_dir: &Path,
    relative_roots: &[&str],
) -> io::Result<Option<PathBuf>> {
    if relative_roots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source set must contain at least one root",
        ));
    }
    if relative_roots.len() == 1 {
        source_set_relative_root_bytes(relative_roots[0])?;
        return Ok(Some(manifest_dir.join(relative_roots[0])));
    }
    Ok(None)
}

fn collect_source_set_files_sorted(
    manifest_dir: &Path,
    relative_roots: &[&str],
) -> io::Result<Vec<(Vec<u8>, PathBuf)>> {
    if relative_roots.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source set must contain at least one root",
        ));
    }

    let mut out = Vec::new();
    for relative_root in relative_roots {
        collect_source_set_root(manifest_dir, relative_root, &mut out)?;
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect_source_set_root(
    manifest_dir: &Path,
    relative_root: &str,
    out: &mut Vec<(Vec<u8>, PathBuf)>,
) -> io::Result<()> {
    let root_label = source_set_relative_root_bytes(relative_root)?;
    let root = manifest_dir.join(relative_root);
    let file_type = source_root_file_type(&root)?;
    if file_type.is_file() {
        out.push((root_label, root));
        return Ok(());
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

    for (relative, path) in collect_rs_files_sorted(&root)? {
        let mut set_relative = root_label.clone();
        set_relative.push(b'/');
        set_relative.extend_from_slice(&relative);
        out.push((set_relative, path));
    }
    Ok(())
}

fn source_set_relative_root_bytes(relative_root: &str) -> io::Result<Vec<u8>> {
    if relative_root.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "source set root must not be empty",
        ));
    }

    let path = Path::new(relative_root);
    if path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("source set root must be repo-relative: {relative_root}"),
        ));
    }

    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("source set root contains an unsupported component: {relative_root}"),
            ));
        };
        let name = name.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("source set root is not valid UTF-8: {relative_root}"),
            )
        })?;
        if name.contains('\\') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("source set root component contains a backslash: {relative_root}"),
            ));
        }
        parts.push(name);
    }
    Ok(parts.join("/").into_bytes())
}

/// The production half of a single file's text: empty for an inner-cfg test-only
/// file, otherwise everything before the FIRST top-level
/// [`TEST_MODULE_SPLIT_MARKER`] or the whole text if the marker is absent. LF
/// checkout remains byte-for-byte equivalent to
/// `text.split(MARKER).next().unwrap()`. CRLF checkout normalizes to LF first so
/// the marker is OS-independent.
fn production_half(text: &str) -> String {
    let normalized = text.replace(CRLF_LINE_ENDING, LF_LINE_ENDING);
    if normalized.starts_with(TEST_ONLY_INNER_CFG_MARKER) {
        return String::new();
    }
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
pub const TEST_ONLY_INNER_CFG_MARKER: &str = "#![cfg(test)]\n";
const CRLF_LINE_ENDING: &str = "\r\n";
const LF_LINE_ENDING: &str = "\n";

/// Stable registry key for the binary-oracle strategy source root.
pub const STRATEGY_KEY: &str = "strategy";
/// Stable registry key for the submit-admission source root.
pub const SUBMIT_ADMISSION_KEY: &str = "submit_admission";
/// Stable registry key for the shared outcome-group substrate source set.
pub const OUTCOME_GROUP_KEY: &str = "outcome_group";
/// Stable registry key for the binary-oracle maker strategy source root.
pub const MAKER_KEY: &str = "maker";

/// One registry entry: a stable key + its repo-relative source roots. A
/// one-element set preserves the old single-root semantics; a multi-root set is
/// ordered by full repo-relative file path.
pub struct GatedSourceRoot {
    pub key: &'static str,
    /// Repo-relative paths from the crate manifest dir.
    pub relative_roots: &'static [&'static str],
}

// THE registry — generated at build time from the repo-root
// `gated_source_roots.manifest` (the ONLY place gated source roots are named).
// `build.rs` parses that manifest and emits this `GATED_SOURCE_ROOTS` constant;
// `scripts/bolt_v3_source_roots.py` reads the same manifest, so the gated file
// list lives in exactly one place shared across both languages. The runtime
// integrity owner, the producer, and tests all resolve roots through this list.
// (Plain `//` comments: rustdoc cannot attach `///` docs to a macro invocation.)
include!(concat!(env!("OUT_DIR"), "/gated_source_roots.rs"));

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
    fn production_text_excludes_inner_cfg_test_only_split_files() {
        let root = temp_dir("test_only_split_file");
        fs::write(root.join("mod.rs"), "pub fn production() {}\n").unwrap();
        let tests = root.join("tests");
        fs::create_dir_all(&tests).unwrap();
        fs::write(
            tests.join("config.rs"),
            "#![cfg(test)]\nconst TEST_ONLY_SENTINEL: &str = \"must_not_enter_production\";\n",
        )
        .unwrap();

        let production = production_module_source_text(&root, 1_000_000).unwrap();

        assert!(
            production.contains("pub fn production() {}"),
            "production source text must keep production modules"
        );
        assert!(
            !production.contains("TEST_ONLY_SENTINEL")
                && !production.contains("must_not_enter_production"),
            "production source text must exclude inner-cfg test-only split files"
        );
    }

    #[test]
    fn source_set_rejects_empty_or_invalid_relative_roots() {
        let manifest = temp_dir("source_set_invalid");
        let absolute = manifest.join("absolute").to_string_lossy().to_string();

        for roots in [
            Vec::<&str>::new(),
            vec![""],
            vec![absolute.as_str()],
            vec!["../outside"],
            vec!["src\\strategy"],
        ] {
            assert!(
                module_source_set_text(&manifest, &roots, 1_000_000).is_err(),
                "invalid source set roots should fail: {roots:?}"
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn directory_branch_rejects_backslash_path_component() {
        let dir = temp_dir("backslash");
        fs::write(dir.join("a\\b.rs"), b"same").unwrap();

        let error = module_source_text(&dir, 1_000_000).unwrap_err();

        assert!(
            error.to_string().contains("backslash"),
            "backslash path components must fail loudly, got: {error}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn text_accessors_reject_symlink_root_explicitly() {
        let dir = temp_dir("symlink_root");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("a.rs"), b"fn a() {}").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        for error in [
            module_source_text(&link, 1_000_000).unwrap_err(),
            production_module_source_text(&link, 1_000_000).unwrap_err(),
        ] {
            assert!(
                error.to_string().contains("source root is a symlink"),
                "root symlink rejection should be explicit, got: {error}"
            );
        }
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
        assert!(module_source_text(&file, 50).is_err());
    }
}
