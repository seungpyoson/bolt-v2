use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

/// Repo-root manifest that is the single owner of the gated source-root list,
/// shared with `scripts/bolt_v3_source_roots.py`.
const GATED_SOURCE_ROOTS_MANIFEST: &str = "gated_source_roots.manifest";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for env_var in build_script_rerun_env_vars() {
        println!("cargo:rerun-if-env-changed={env_var}");
    }

    let manifest_dir = build_script_manifest_dir();
    emit_git_head_rerun_paths(&manifest_dir);
    emit_gated_source_roots(&manifest_dir);

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

/// Generate the `GATED_SOURCE_ROOTS` constant from the repo-root manifest into
/// `$OUT_DIR/gated_source_roots.rs`, which `src/source_canonicalization.rs`
/// includes. The manifest is the single source of the gated root list; Python
/// reads the same file. Build fails loud if the manifest is missing or malformed.
fn emit_gated_source_roots(manifest_dir: &Path) {
    let manifest_path = manifest_dir.join(GATED_SOURCE_ROOTS_MANIFEST);
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let text = fs::read_to_string(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "reading {} should succeed: {error}",
            manifest_path.display()
        )
    });
    let entries = parse_gated_source_roots(&text, &manifest_path);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo"));
    let out_path = out_dir.join("gated_source_roots.rs");
    fs::write(&out_path, render_gated_source_roots(&entries))
        .unwrap_or_else(|error| panic!("writing {} should succeed: {error}", out_path.display()));
}

/// Parse the manifest into `(key, roots)` entries in declaration order. Comments
/// (`#`) and blank lines are ignored; `[key]` starts a section; every other line
/// is a repo-relative root. Invalid roots (absolute, backslash, `.`/`..`/empty
/// components) and structural errors fail the build with a file:line message.
/// The manifest must declare exactly the four registry sections (`[strategy]`,
/// `[submit_admission]`, `[outcome_group]`, `[maker]`): a missing or unexpected
/// section fails the build. `scripts/bolt_v3_source_roots.py` enforces the same
/// set and mirrors the two Unicode-sensitive primitives used here: line
/// splitting on
/// `str::lines()` terminators (`\n`/`\r\n`, via Python `split("\n")` not
/// `splitlines()`) and whitespace trimming on the `str::trim()` `White_Space`
/// set (not bare `str.strip()`, which also strips U+001C–U+001F). Every other
/// step is an ASCII-literal check, so the two parsers are equivalent for all
/// inputs and a malformed manifest fails loudly on both.
fn parse_gated_source_roots(text: &str, manifest_path: &Path) -> Vec<(String, Vec<String>)> {
    let mut entries: Vec<(String, Vec<String>)> = Vec::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let location = format!("{}:{}", manifest_path.display(), index + 1);
        if let Some(rest) = line.strip_prefix('[') {
            let key = rest
                .strip_suffix(']')
                .unwrap_or_else(|| panic!("{location}: malformed section header `{line}`"))
                .trim();
            assert!(!key.is_empty(), "{location}: empty section key");
            assert!(
                !entries.iter().any(|(existing, _)| existing == key),
                "{location}: duplicate section `[{key}]`"
            );
            entries.push((key.to_string(), Vec::new()));
            continue;
        }
        let valid_root = !line.starts_with('/')
            && !line.contains('\\')
            && line
                .split('/')
                .all(|component| !component.is_empty() && component != "." && component != "..");
        assert!(
            valid_root,
            "{location}: invalid repo-relative root `{line}`"
        );
        let current = entries
            .last_mut()
            .unwrap_or_else(|| panic!("{location}: root `{line}` precedes any [section] header"));
        current.1.push(line.to_string());
    }
    assert!(
        !entries.is_empty(),
        "{}: no gated source roots defined",
        manifest_path.display()
    );
    for (key, roots) in &entries {
        assert!(
            !roots.is_empty(),
            "{}: section `[{key}]` has no roots",
            manifest_path.display()
        );
    }
    // The manifest must declare EXACTLY the four registry keys the crate consumes
    // (STRATEGY_KEY / SUBMIT_ADMISSION_KEY / OUTCOME_GROUP_KEY / MAKER_KEY in
    // `src/source_canonicalization.rs`). build.rs cannot import those crate consts,
    // so they are mirrored here; `scripts/bolt_v3_source_roots.py` enforces the
    // same set. Rejecting both missing AND unexpected sections means a typo'd
    // header (e.g. `[strategies]`) fails the build instead of silently dropping
    // roots from the gated set or panicking later at `registry_entry`.
    const REQUIRED_KEYS: [&str; 4] = [
        "strategy",
        "submit_admission",
        "outcome_group",
        "maker",
    ];
    let keys: Vec<&str> = entries.iter().map(|(key, _)| key.as_str()).collect();
    for required in REQUIRED_KEYS {
        assert!(
            keys.contains(&required),
            "{}: required section `[{required}]` is missing",
            manifest_path.display()
        );
    }
    for key in &keys {
        assert!(
            REQUIRED_KEYS.contains(key),
            "{}: unexpected section `[{key}]` (expected exactly {REQUIRED_KEYS:?})",
            manifest_path.display()
        );
    }
    entries
}

/// Render the parsed entries as a `GATED_SOURCE_ROOTS` constant. Keys and roots
/// are emitted with `{:?}` so they are valid, escaped Rust string literals.
fn render_gated_source_roots(entries: &[(String, Vec<String>)]) -> String {
    let mut out = String::new();
    out.push_str("// @generated by build.rs from gated_source_roots.manifest — do not edit.\n");
    out.push_str("pub const GATED_SOURCE_ROOTS: &[GatedSourceRoot] = &[\n");
    for (key, roots) in entries {
        out.push_str("    GatedSourceRoot {\n");
        out.push_str(&format!("        key: {key:?},\n"));
        out.push_str("        relative_roots: &[\n");
        for root in roots {
            out.push_str(&format!("            {root:?},\n"));
        }
        out.push_str("        ],\n");
        out.push_str("    },\n");
    }
    out.push_str("];\n");
    out
}

fn emit_git_head_rerun_paths(manifest_dir: &Path) {
    for path in git_head_rerun_paths(manifest_dir) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}

pub fn build_script_rerun_env_vars() -> &'static [&'static str] {
    &["CARGO_MANIFEST_DIR"]
}

pub fn build_script_manifest_dir() -> PathBuf {
    PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set by Cargo"),
    )
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
