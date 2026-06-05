use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

// SINGLE TRANSCRIPTION of the canonicalization walk + framing + hash. This is
// the exact same source file the crate compiles as
// `crate::source_canonicalization`; including it here via `#[path]` guarantees
// the build-time canonical bytes and the runtime digest can never drift. It
// depends only on `std` + `sha2` + `hex` (declared in `[build-dependencies]`
// pinned to the same versions as `[dependencies]`).
// The build script only consumes `GATED_SOURCE_ROOTS` + `canonical_source_bytes`
// from this shared module; the digest/text accessors are used by the lib, not
// here, so `dead_code` is expected in the build-script compilation view.
#[allow(dead_code)]
#[path = "src/source_canonicalization.rs"]
mod source_canonicalization;

// Generous in-build cap. The strategy root is now the directory
// `src/strategies/binary_oracle_edge_taker/` ({config.rs, mod.rs,
// selection.rs}), whose framed canonical stream is well under this cap; the
// runtime digest path applies the operator-configured `max_source_bytes` instead.
// This only bounds the bytes embedded into the binary at build time.
const BUILD_CANONICAL_MAX_BYTES: u64 = 8 * 1024 * 1024;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set by Cargo"),
    );
    emit_git_head_rerun_paths(&manifest_dir);
    emit_canonical_source_artifacts(&manifest_dir);

    match Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&manifest_dir)
        .output()
    {
        Ok(output) if output.status.success() => {
            let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if is_git_head_sha(&head) {
                println!("cargo:rustc-env=BOLT_V3_BUILD_HEAD_SHA={head}");
            } else {
                println!("cargo:warning=git rev-parse HEAD returned invalid head shape");
            }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            println!("cargo:warning=git rev-parse HEAD failed: {stderr}");
        }
        Err(error) => {
            println!("cargo:warning=failed to run git rev-parse HEAD: {error}");
        }
    }
}

fn emit_git_head_rerun_paths(manifest_dir: &Path) {
    for path in git_head_rerun_paths(manifest_dir) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

/// Re-emit the canonical bytes of every gated source root into
/// `$OUT_DIR/<key>.canonical`, using the SAME walk/framing the runtime digest
/// uses. The verifier embeds these via `include_bytes!(concat!(env!("OUT_DIR"),
/// "/<key>.canonical"))` and hashes them at runtime — compile-time
/// tamper-evidence preserved, layout-independent. Emits `rerun-if-changed` per
/// root so a modify/add/remove (including nested subdirs after a split)
/// re-triggers the build.
fn emit_canonical_source_artifacts(manifest_dir: &Path) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo"));
    for entry in source_canonicalization::GATED_SOURCE_ROOTS {
        let root = manifest_dir.join(entry.relative_root);
        println!("cargo:rerun-if-changed={}", root.display());
        let canonical =
            source_canonicalization::canonical_source_bytes(&root, BUILD_CANONICAL_MAX_BYTES)
                .unwrap_or_else(|error| {
                    panic!(
                        "canonical source bytes for `{}` ({}) should emit: {error}",
                        entry.key,
                        root.display()
                    )
                });
        let out_path = out_dir.join(format!("{}.canonical", entry.key));
        fs::write(&out_path, &canonical).unwrap_or_else(|error| {
            panic!("writing {} should succeed: {error}", out_path.display())
        });
    }
}

pub fn git_head_rerun_paths(manifest_dir: &Path) -> Vec<PathBuf> {
    let Some(git_dir) = git_dir_from_manifest(manifest_dir) else {
        return Vec::new();
    };

    let head_path = git_dir.join("HEAD");
    let mut paths = vec![head_path.clone()];

    let Ok(head_content) = fs::read_to_string(&head_path) else {
        return paths;
    };
    let Some(head_ref) = head_content.strip_prefix("ref:").map(str::trim) else {
        return paths;
    };

    let common_dir = git_common_dir(&git_dir);
    push_unique(&mut paths, common_dir.join(head_ref));
    push_unique(&mut paths, git_dir.join(head_ref));
    push_unique(&mut paths, common_dir.join("packed-refs"));
    paths
}

fn git_dir_from_manifest(manifest_dir: &Path) -> Option<PathBuf> {
    let dot_git = manifest_dir.join(".git");
    if dot_git.is_dir() {
        return Some(canonicalize_existing(dot_git));
    }

    let dot_git_content = fs::read_to_string(&dot_git).ok()?;
    let git_dir = dot_git_content.strip_prefix("gitdir:").map(str::trim)?;
    let git_dir = PathBuf::from(git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        manifest_dir.join(git_dir)
    };
    Some(canonicalize_existing(git_dir))
}

fn git_common_dir(git_dir: &Path) -> PathBuf {
    let Ok(common_dir) = fs::read_to_string(git_dir.join("commondir")) else {
        return git_dir.to_path_buf();
    };
    let common_dir = PathBuf::from(common_dir.trim());
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        git_dir.join(common_dir)
    };
    canonicalize_existing(common_dir)
}

fn canonicalize_existing(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
