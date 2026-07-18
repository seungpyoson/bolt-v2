use std::{
    collections::BTreeSet,
    env,
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

/// Repo-root manifest that is the single owner of the gated source-root list,
/// shared with `scripts/bolt_v3_source_roots.py`.
const GATED_SOURCE_ROOTS_MANIFEST: &str = "gated_source_roots.manifest";
const NAUTILUS_SOURCE_CAPABILITIES_MANIFEST: &str = "ci/nautilus-source-capabilities.toml";
const OFFICIAL_NAUTILUS_REPOSITORY: &str = "https://github.com/nautechsystems/nautilus_trader.git";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    for env_var in build_script_rerun_env_vars() {
        println!("cargo:rerun-if-env-changed={env_var}");
    }

    let manifest_dir = build_script_manifest_dir();
    emit_git_head_rerun_paths(&manifest_dir);
    emit_gated_source_roots(&manifest_dir);
    emit_nautilus_source_capabilities(&manifest_dir);

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct NautilusSourceCapabilities {
    revision: String,
    binance_spot_sbe_schema_3_5: bool,
    binance_adapter_receive_timestamps: bool,
    evidence: Vec<NautilusSourceCapabilityEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NautilusSourceCapabilityEvidence {
    capability: String,
    cargo_test_target: String,
    path: String,
}

/// Generate immutable Nautilus source-capability facts from the governed CI
/// manifest. The manifest is code-review policy, not runtime/operator config.
/// This source slice has one direct Binance path, so either required fact being
/// false fails the build until an issue-bound admission-blocking implementation
/// replaces that direct path.
fn emit_nautilus_source_capabilities(manifest_dir: &Path) {
    let capability_path = manifest_dir.join(NAUTILUS_SOURCE_CAPABILITIES_MANIFEST);
    let cargo_path = manifest_dir.join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", capability_path.display());
    println!("cargo:rerun-if-changed={}", cargo_path.display());

    let capability_text = fs::read_to_string(&capability_path).unwrap_or_else(|error| {
        panic!(
            "reading {} should succeed: {error}",
            capability_path.display()
        )
    });
    let capabilities = parse_nautilus_source_capabilities(&capability_text, &capability_path);
    validate_nautilus_manifest_binding(&capabilities, &cargo_path);
    for evidence in &capabilities.evidence {
        println!("cargo:rerun-if-changed={}", evidence.path);
    }
    assert!(
        capabilities.binance_spot_sbe_schema_3_5,
        "{}: the direct Binance runtime path requires official schema 3:5 support; a false fact requires an explicit affected-new-risk admission blocker",
        capability_path.display()
    );
    assert!(
        capabilities.binance_adapter_receive_timestamps,
        "{}: the direct Binance runtime path requires adapter receive timestamps; a false fact requires an explicit affected-new-risk admission blocker",
        capability_path.display()
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR should be set by Cargo"));
    let out_path = out_dir.join("nautilus_source_capabilities.rs");
    fs::write(
        &out_path,
        render_nautilus_source_capabilities(&capabilities),
    )
    .unwrap_or_else(|error| panic!("writing {} should succeed: {error}", out_path.display()));
}

fn parse_nautilus_source_capabilities(text: &str, path: &Path) -> NautilusSourceCapabilities {
    let document = toml::from_str::<toml::Table>(text)
        .unwrap_or_else(|error| panic!("parsing {} should succeed: {error}", path.display()));
    assert_exact_keys(
        &document,
        &["revision", "binance_spot", "evidence"],
        path,
        "root",
    );

    let revision = required_toml_string(&document, "revision", path, "root");
    assert!(
        is_git_head_sha(&revision),
        "{}: revision must be one immutable lowercase 40-character commit",
        path.display()
    );
    let binance_spot = document
        .get("binance_spot")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| panic!("{}: binance_spot must be a TOML table", path.display()));
    assert_exact_keys(
        binance_spot,
        &["sbe_schema_3_5", "adapter_receive_timestamps"],
        path,
        "binance_spot",
    );
    let evidence = document
        .get("evidence")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("{}: evidence must be an array of tables", path.display()))
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let owner = format!("evidence[{index}]");
            let table = value
                .as_table()
                .unwrap_or_else(|| panic!("{}: {owner} must be a TOML table", path.display()));
            assert_exact_keys(
                table,
                &["capability", "cargo_test_target", "path", "sha256"],
                path,
                &owner,
            );
            let artifact_path = required_toml_string(table, "path", path, &owner);
            assert!(
                is_safe_repo_relative_path(&artifact_path),
                "{}: {owner}.path must be a safe repository-relative path",
                path.display()
            );
            let sha256 = required_toml_string(table, "sha256", path, &owner);
            assert!(
                is_lower_hex_sha256(&sha256),
                "{}: {owner}.sha256 must be lowercase 64-character hex",
                path.display()
            );
            NautilusSourceCapabilityEvidence {
                capability: required_toml_string(table, "capability", path, &owner),
                cargo_test_target: required_toml_string(table, "cargo_test_target", path, &owner),
                path: artifact_path,
            }
        })
        .collect::<Vec<_>>();
    let expected_evidence_capabilities = binance_spot
        .keys()
        .map(|key| format!("binance_spot.{key}"))
        .collect::<BTreeSet<_>>();
    let actual_evidence_capabilities = evidence
        .iter()
        .map(|entry| entry.capability.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        evidence.len(),
        actual_evidence_capabilities.len(),
        "{}: evidence capability keys must be unique",
        path.display()
    );
    assert_eq!(
        actual_evidence_capabilities,
        expected_evidence_capabilities,
        "{}: evidence must bind every declared source capability exactly once",
        path.display()
    );

    NautilusSourceCapabilities {
        revision,
        binance_spot_sbe_schema_3_5: required_toml_bool(
            binance_spot,
            "sbe_schema_3_5",
            path,
            "binance_spot",
        ),
        binance_adapter_receive_timestamps: required_toml_bool(
            binance_spot,
            "adapter_receive_timestamps",
            path,
            "binance_spot",
        ),
        evidence,
    }
}

fn validate_nautilus_manifest_binding(
    capabilities: &NautilusSourceCapabilities,
    cargo_path: &Path,
) {
    let cargo_text = fs::read_to_string(cargo_path)
        .unwrap_or_else(|error| panic!("reading {} should succeed: {error}", cargo_path.display()));
    let cargo = toml::from_str::<toml::Table>(&cargo_text)
        .unwrap_or_else(|error| panic!("parsing {} should succeed: {error}", cargo_path.display()));
    let dependencies = cargo
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| {
            panic!(
                "{}: dependencies must be a TOML table",
                cargo_path.display()
            )
        });
    let binance = dependencies
        .get("nautilus-binance")
        .and_then(toml::Value::as_table)
        .unwrap_or_else(|| {
            panic!(
                "{}: dependencies.nautilus-binance must be a TOML table",
                cargo_path.display()
            )
        });
    let git = required_toml_string(binance, "git", cargo_path, "dependencies.nautilus-binance");
    let revision =
        required_toml_string(binance, "rev", cargo_path, "dependencies.nautilus-binance");
    assert_eq!(
        git,
        OFFICIAL_NAUTILUS_REPOSITORY,
        "{}: dependencies.nautilus-binance must use the official repository",
        cargo_path.display()
    );
    assert_eq!(
        revision,
        capabilities.revision,
        "{}: revision must equal dependencies.nautilus-binance.rev in {}",
        NAUTILUS_SOURCE_CAPABILITIES_MANIFEST,
        cargo_path.display()
    );
    let test_targets = cargo
        .get("test")
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("{}: test targets must be an array", cargo_path.display()));
    for evidence in &capabilities.evidence {
        let registrations = test_targets
            .iter()
            .filter_map(toml::Value::as_table)
            .filter(|target| {
                target.get("name").and_then(toml::Value::as_str)
                    == Some(evidence.cargo_test_target.as_str())
                    && target.get("path").and_then(toml::Value::as_str)
                        == Some(evidence.path.as_str())
            })
            .count();
        assert_eq!(
            registrations,
            1,
            "{}: capability {} evidence must bind exactly one Cargo test target {} at {}",
            cargo_path.display(),
            evidence.capability,
            evidence.cargo_test_target,
            evidence.path
        );
        assert!(
            cargo_path
                .parent()
                .expect("Cargo.toml must have a parent")
                .join(&evidence.path)
                .is_file(),
            "{}: capability {} evidence artifact {} must exist",
            cargo_path.display(),
            evidence.capability,
            evidence.path
        );
    }
}

fn assert_exact_keys(table: &toml::Table, expected: &[&str], path: &Path, owner: &str) {
    let actual = table.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        actual,
        expected,
        "{}: {owner} must contain exactly {expected:?}",
        path.display()
    );
}

fn required_toml_string(table: &toml::Table, key: &str, path: &Path, owner: &str) -> String {
    table
        .get(key)
        .and_then(toml::Value::as_str)
        .unwrap_or_else(|| panic!("{}: {owner}.{key} must be a string", path.display()))
        .to_string()
}

fn required_toml_bool(table: &toml::Table, key: &str, path: &Path, owner: &str) -> bool {
    table
        .get(key)
        .and_then(toml::Value::as_bool)
        .unwrap_or_else(|| panic!("{}: {owner}.{key} must be a boolean", path.display()))
}

fn render_nautilus_source_capabilities(capabilities: &NautilusSourceCapabilities) -> String {
    format!(
        "// @generated by build.rs from ci/nautilus-source-capabilities.toml — do not edit.\n\
pub const NAUTILUS_SOURCE_CAPABILITIES: NautilusSourceCapabilityRegistry =\n\
    NautilusSourceCapabilityRegistry {{\n\
        revision: {:?},\n\
        binance_spot_sbe_schema_3_5: {},\n\
        binance_adapter_receive_timestamps: {},\n\
    }};\n",
        capabilities.revision,
        capabilities.binance_spot_sbe_schema_3_5,
        capabilities.binance_adapter_receive_timestamps,
    )
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
    const REQUIRED_KEYS: [&str; 4] = ["strategy", "submit_admission", "outcome_group", "maker"];
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

/// Paths whose change means `HEAD` now points at a different commit.
///
/// Every path returned must exist. Cargo treats a `rerun-if-changed` path that
/// does not exist as permanently dirty, which re-runs this script and
/// recompiles the crate on every single invocation.
///
/// Empty when there is no `HEAD` file to read: a git dir without one names no
/// commit, so there is nothing to watch. `<git_dir>` is not a fallback, because
/// the index and the logs beneath it change without `HEAD` moving.
pub fn git_head_rerun_paths(manifest_dir: &Path) -> Vec<PathBuf> {
    let Some(git_dir) = git_dir_from_manifest(manifest_dir) else {
        return Vec::new();
    };

    // `git_dir` may still be absent (a `.git` file can name a git dir that was
    // deleted), and `HEAD` may be missing or not a file. Naming it regardless
    // would emit the very kind of nonexistent path this function exists to keep
    // out.
    let head_path = git_dir.join("HEAD");
    if !head_path.is_file() {
        return Vec::new();
    }
    let mut paths = vec![head_path.clone()];

    let Ok(head_content) = fs::read_to_string(&head_path) else {
        return paths;
    };
    let Some(head_ref) = head_content.strip_prefix("ref:").map(str::trim) else {
        return paths;
    };

    // `HEAD` may name a ref in either base: a linked worktree resolves
    // `refs/heads/*` against the common dir, but keeps its per-worktree refs
    // (`refs/bisect/*`, `refs/worktree/*`, `refs/rewritten/*`) in its own git
    // dir. Only one base holds this `HEAD`'s ref; the other finds no loose file
    // and falls back to its `refs` dir, which over-triggers harmlessly. Outside
    // a linked worktree the two bases are the same directory.
    let common_dir = git_common_dir(&git_dir);
    for base in [&common_dir, &git_dir] {
        if let Some(path) = ref_watch_path(base, head_ref) {
            push_unique(&mut paths, path);
        }
    }
    let packed_refs = common_dir.join("packed-refs");
    if packed_refs.is_file() {
        push_unique(&mut paths, packed_refs);
    }
    paths
}

/// Watch `<base>/<head_ref>` when the ref is stored loosely, otherwise watch the
/// deepest existing directory above it.
///
/// A branch ref may be stored loosely, packed, or both at once: `git pack-refs`
/// deletes the loose file but leaves its packed entry behind, which a later
/// commit shadows by writing the loose file again. Either storage can therefore
/// be absent, and an absent path may not be named. Naming the enclosing
/// directory instead covers the transitions in both directions, because Cargo
/// reports a directory as changed when an entry anywhere beneath it is added,
/// modified, or removed: packing the ref deletes the loose file, and committing
/// on a packed branch creates one.
///
/// `<base>` itself is never watched. It holds the index and the logs, which are
/// rewritten by ordinary git commands that never move `HEAD`.
fn ref_watch_path(base: &Path, head_ref: &str) -> Option<PathBuf> {
    let head_ref = refs_relative_path(head_ref)?;
    let refs_root = base.join("refs");
    let loose_ref = base.join(head_ref);
    if loose_ref.is_file() {
        return Some(loose_ref);
    }

    let mut current = loose_ref.parent()?;
    while current.starts_with(&refs_root) {
        if current.is_dir() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
    None
}

/// `head_ref` as a relative path under `refs/`, or `None` when it is anything
/// else.
///
/// `Path::starts_with` matches components literally rather than resolving them,
/// so the walk above stays inside `<base>/refs` only while `head_ref` holds no
/// `..`. A `HEAD` reading `ref: refs/../..` would otherwise resolve the deepest
/// existing directory to the working tree root, and watching that re-runs this
/// build script on every source edit.
fn refs_relative_path(head_ref: &str) -> Option<&Path> {
    let head_ref = Path::new(head_ref);
    let mut components = head_ref.components();
    if components.next() != Some(Component::Normal(OsStr::new("refs"))) {
        return None;
    }
    components
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(head_ref)
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

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_safe_repo_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}
