//! Single owner of repo-relative input/output path resolution for the
//! backfill / source-universe pipeline modules and their CLI binaries.
//!
//! Pipeline specs reference committed inputs and output directories as
//! repo-relative paths (`specs/...`, `crates/...`), but the binaries run from
//! arbitrary working directories (repo root, the crate directory, a worktree).
//! Resolution priority, identical for every caller:
//!
//! 1. absolute paths pass through untouched;
//! 2. paths whose first component is a known repo top-level directory resolve
//!    against the enclosing repo root (an ancestor of the current directory or
//!    of this crate carrying the repo's `justfile` + `AGENTS.md` markers),
//!    falling back to the nearest ancestor that can contain them;
//! 3. repo scratch paths (`target/...`) resolve against the marker root that
//!    owns the referencing spec, without requiring `target/` to exist first;
//! 4. otherwise the working-directory-relative or `base_dir`-relative
//!    candidate wins.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

const REPO_TOP_LEVEL_DIRS: [&str; 4] = ["specs", "crates", "docs", "scripts"];
const REPO_SCRATCH_DIRS: [&str; 1] = ["target"];
const REPO_ROOT_MARKERS: [&str; 2] = ["justfile", "AGENTS.md"];

/// Resolve a path that must reference an existing input file or directory.
///
/// `base_dir` is the directory the referencing artifact lives in (spec file,
/// manifest, work order); relative paths that do not resolve from the repo
/// root or the working directory are tried against it.
#[must_use]
pub fn resolve_existing_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if looks_repo_relative(path)
        && let Some(candidate) = resolve_from_known_anchors(path)
    {
        return candidate;
    }
    if looks_repo_scratch_path(path) {
        if let Some(repo_root) = marker_repo_root_from_base_dir(base_dir) {
            return repo_root.join(path);
        }
        return base_dir.join(path);
    }
    if path.exists() {
        return path.to_path_buf();
    }
    let base_candidate = base_dir.join(path);
    if base_candidate.exists() {
        return base_candidate;
    }
    resolve_from_known_anchors(path).unwrap_or_else(|| path.to_path_buf())
}

/// Resolve a control artifact referenced by an execution pack.
///
/// Ordinary relative paths are owned by the pack directory and must not be
/// shadowed by an ambient working-directory file. Canonical repo-relative and
/// repo-scratch paths resolve below the marker-bearing repository root which
/// owns the pack. The returned path is canonical, so a symlink cannot conceal
/// an escape from either allowed root.
///
/// # Errors
///
/// Returns an error for absolute paths, parent traversal, missing inputs, a
/// repo-relative identity without an owning marker root, or a canonical path
/// outside the single authoritative root selected by the path class.
pub fn resolve_pack_control_path(pack_base_dir: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.is_absolute(),
        "pack control path {} must be relative",
        path.display()
    );
    ensure!(
        !has_parent_component(path),
        "pack control path {} must not contain parent traversal",
        path.display()
    );
    ensure!(
        path.components()
            .any(|component| matches!(component, Component::Normal(_))),
        "pack control path must contain a portable path component"
    );

    let canonical_pack_dir = pack_base_dir.canonicalize().with_context(|| {
        format!(
            "canonicalize execution-pack directory {}",
            pack_base_dir.display()
        )
    })?;
    let canonical_repo_root = marker_repo_root_from_base_dir(&canonical_pack_dir)
        .map(|root| {
            root.canonicalize()
                .with_context(|| format!("canonicalize repository root {}", root.display()))
        })
        .transpose()?;

    let candidate = if looks_repo_relative(path) || looks_repo_scratch_path(path) {
        canonical_repo_root
            .as_ref()
            .with_context(|| {
                format!(
                    "pack control path {} is repository-relative but pack directory {} has no marker-bearing repository root",
                    path.display(),
                    canonical_pack_dir.display()
                )
            })?
            .join(path)
    } else {
        canonical_pack_dir.join(path)
    };
    let canonical_candidate = candidate
        .canonicalize()
        .with_context(|| format!("canonicalize pack control path {}", candidate.display()))?;
    let expected_root = if looks_repo_relative(path) || looks_repo_scratch_path(path) {
        canonical_repo_root
            .as_ref()
            .expect("repo-relative branch established a marker root")
    } else {
        &canonical_pack_dir
    };
    ensure!(
        canonical_candidate.starts_with(expected_root),
        "pack control path {} resolves outside its authoritative root {}",
        path.display(),
        expected_root.display()
    );
    Ok(canonical_candidate)
}

/// Validate an operator-controlled output identity as one portable component.
///
/// # Errors
///
/// Returns an error when `value` is empty, `.`/`..`, or contains any character
/// outside the portable `[A-Za-z0-9._-]` set.
pub fn validate_portable_path_component(field: &str, value: &str) -> Result<()> {
    ensure!(!value.is_empty(), "{field} must not be empty");
    ensure!(
        value != "." && value != "..",
        "{field} must not be {value:?}"
    );
    ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')),
        "{field} {value:?} must be one portable [A-Za-z0-9._-]+ path component"
    );
    Ok(())
}

/// Resolve one validated child below an existing output root, rejecting an
/// already-present symlink or canonical escape.
///
/// # Errors
///
/// Returns an error for an invalid component, a missing output root, an
/// existing symlink, or an existing child outside the canonical output root.
pub fn resolve_contained_output_component(output_root: &Path, component: &str) -> Result<PathBuf> {
    validate_portable_path_component("operator_run_id", component)?;
    let canonical_root = output_root.canonicalize().with_context(|| {
        format!(
            "canonicalize batch output directory {}",
            output_root.display()
        )
    })?;
    let candidate = canonical_root.join(component);
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink(),
                "operator output {} must not be a symlink",
                candidate.display()
            );
            let canonical_candidate = candidate
                .canonicalize()
                .with_context(|| format!("canonicalize operator output {}", candidate.display()))?;
            ensure!(
                canonical_candidate.starts_with(&canonical_root),
                "operator output {} resolves outside batch output directory {}",
                candidate.display(),
                canonical_root.display()
            );
            Ok(canonical_candidate)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(candidate),
        Err(error) => {
            Err(error).with_context(|| format!("inspect operator output {}", candidate.display()))
        }
    }
}

/// Resolve a CLI-supplied path that must reference an existing input, with no
/// referencing-artifact directory to anchor against (the binary entrypoint
/// variant of [`resolve_existing_path`]).
#[must_use]
pub fn resolve_existing_input_path(path: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        return path.to_path_buf();
    }
    for anchor in anchor_dirs() {
        for ancestor in anchor.ancestors() {
            let candidate = ancestor.join(path);
            if candidate.exists() {
                return candidate;
            }
        }
    }
    path.to_path_buf()
}

/// Resolve an output directory that may not exist yet; its parent (or the
/// repo root for repo-relative paths) decides the placement.
#[must_use]
pub fn resolve_output_dir(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if looks_repo_relative(path)
        && let Some(candidate) = resolve_from_known_anchors(path)
    {
        return candidate;
    }
    if looks_repo_scratch_path(path) {
        if let Some(repo_root) = marker_repo_root_from_base_dir(base_dir) {
            return repo_root.join(path);
        }
        return base_dir.join(path);
    }
    let base_candidate = base_dir.join(path);
    if base_candidate
        .parent()
        .is_some_and(|parent| parent.exists())
    {
        return base_candidate;
    }
    resolve_from_known_anchors(path).unwrap_or(base_candidate)
}

/// Rewrite an absolute path under the repo root back to its repo-relative
/// form so committed artifacts stay portable across checkouts.
///
/// Serialization fails rather than embedding a machine-specific absolute path.
pub fn portable_artifact_path(path: &Path) -> Result<PathBuf> {
    candidate_portable_artifact_path(path).ok_or_else(|| {
        anyhow::anyhow!(
            "artifact path {} could not be serialized as a canonical repo-relative identity",
            path.display()
        )
    })
}

/// Rewrite an artifact path for serialization, failing if it cannot be
/// represented as a repo-relative committed identity.
pub fn portable_artifact_path_for_spec(path: &Path, spec_path: &Path) -> Result<PathBuf> {
    let Some(portable) = candidate_portable_artifact_path(path) else {
        bail!(
            "artifact path {} resolved from {} but could not be serialized as a canonical repo-relative identity",
            path.display(),
            spec_path.display()
        );
    };
    Ok(portable)
}

/// Choose the stable identity serialized for a materialized input artifact.
///
/// Specs that read from transient scratch storage may provide the artifact's
/// canonical committed or evicted repo path separately. Without an override,
/// preserve the existing portable-path behavior for ordinary inputs.
pub fn stable_artifact_identity_path_for_spec(
    resolved_path: &Path,
    materialization_path: &Path,
    artifact_identity_path: Option<&Path>,
) -> Result<PathBuf> {
    let Some(identity) = artifact_identity_path else {
        return portable_artifact_path_for_spec(resolved_path, materialization_path);
    };
    if !is_canonical_repo_relative(identity) {
        bail!(
            "artifact identity path {} must be a canonical repo-relative committed path",
            identity.display()
        );
    }
    Ok(identity.to_path_buf())
}

fn candidate_portable_artifact_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return Some(path.to_path_buf());
    }
    for repo_root in repo_root_dirs() {
        if let Ok(candidate) = path.strip_prefix(&repo_root)
            && is_canonical_repo_relative(candidate)
        {
            return Some(candidate.to_path_buf());
        }
        if let (Ok(canonical_path), Ok(canonical_root)) =
            (path.canonicalize(), repo_root.canonicalize())
            && let Ok(candidate) = canonical_path.strip_prefix(canonical_root)
            && is_canonical_repo_relative(candidate)
        {
            return Some(candidate.to_path_buf());
        }
    }
    None
}

fn repo_root_dirs() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for anchor in anchor_dirs() {
        for ancestor in anchor.ancestors() {
            if REPO_ROOT_MARKERS
                .iter()
                .all(|marker| ancestor.join(marker).exists())
                && !roots.iter().any(|root| root == ancestor)
            {
                roots.push(ancestor.to_path_buf());
            }
        }
    }
    roots
}

fn has_parent_component(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn is_canonical_repo_relative(path: &Path) -> bool {
    looks_repo_relative(path) && !has_parent_component(path)
}

/// Whether `path` starts with one of the repo's top-level directories and
/// should therefore resolve against the repo root rather than the working
/// directory.
#[must_use]
pub fn looks_repo_relative(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(component))
            if REPO_TOP_LEVEL_DIRS.iter().any(|dir| component == *dir)
    )
}

fn looks_repo_scratch_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Normal(component))
            if REPO_SCRATCH_DIRS.iter().any(|dir| component == *dir)
    ) && !has_parent_component(path)
}

fn marker_repo_root_from_base_dir(base_dir: &Path) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if base_dir.is_absolute() {
        candidates.push(base_dir.to_path_buf());
    } else {
        if let Ok(current_dir) = std::env::current_dir() {
            candidates.push(current_dir.join(base_dir));
        }
        candidates.push(base_dir.to_path_buf());
    }

    for candidate in candidates {
        let normalized = candidate.canonicalize().unwrap_or(candidate);
        for ancestor in normalized.ancestors() {
            if REPO_ROOT_MARKERS
                .iter()
                .all(|marker| ancestor.join(marker).exists())
            {
                return Some(ancestor.to_path_buf());
            }
        }
    }
    None
}

fn resolve_from_known_anchors(path: &Path) -> Option<PathBuf> {
    let anchors = anchor_dirs();
    // Strongest anchor first: the enclosing repo root itself, identified by
    // its marker files, so resolution never lands on an arbitrary ancestor
    // that merely happens to contain a matching directory name.
    for anchor in &anchors {
        for ancestor in anchor.ancestors() {
            if let Some(Component::Normal(component)) = path.components().next()
                && ancestor.join(component).exists()
                && REPO_ROOT_MARKERS
                    .iter()
                    .all(|marker| ancestor.join(marker).exists())
            {
                return Some(ancestor.join(path));
            }
        }
    }
    for anchor in &anchors {
        for ancestor in anchor.ancestors() {
            let candidate = ancestor.join(path);
            if candidate.parent().is_some_and(Path::exists) {
                return Some(candidate);
            }
        }
    }
    None
}

fn anchor_dirs() -> Vec<PathBuf> {
    let mut anchors = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        anchors.push(current_dir);
    }
    anchors.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    anchors
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn absolute_paths_pass_through() {
        let absolute = std::env::temp_dir().join("path-resolution-absolute");
        assert_eq!(
            resolve_existing_path(Path::new("."), &absolute),
            absolute.clone()
        );
        assert_eq!(resolve_existing_input_path(&absolute), absolute.clone());
        assert_eq!(resolve_output_dir(Path::new("."), &absolute), absolute);
    }

    #[test]
    fn repo_relative_paths_resolve_to_marker_carrying_repo_root() {
        let resolved = resolve_existing_path(Path::new("."), Path::new("crates"));
        assert!(resolved.is_absolute(), "resolved: {}", resolved.display());
        let root = resolved.parent().expect("repo root");
        for marker in REPO_ROOT_MARKERS {
            assert!(root.join(marker).exists(), "missing {marker}");
        }
    }

    #[test]
    fn pack_control_paths_are_canonical_and_pack_relative() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let declared = Path::new("control.json");
        fs::write(temp_dir.path().join(declared), "{}").expect("write control");

        assert_eq!(
            resolve_pack_control_path(temp_dir.path(), declared).expect("resolve pack control"),
            temp_dir.path().canonicalize().unwrap().join(declared)
        );
    }

    #[test]
    fn pack_control_paths_preserve_repo_relative_resolution() {
        let resolved = resolve_pack_control_path(Path::new("."), Path::new("crates"))
            .expect("resolve repo-relative control");
        assert!(resolved.is_absolute(), "resolved: {}", resolved.display());
        assert!(resolved.join("backtesting-vertical-slice").is_dir());
    }

    #[test]
    fn pack_control_paths_reject_missing_inputs() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let error = resolve_pack_control_path(temp_dir.path(), Path::new("missing-control.json"))
            .expect_err("missing pack control must fail closed");
        assert!(error.to_string().contains("canonicalize pack control path"));
    }

    #[test]
    fn markerless_pack_cannot_borrow_an_ambient_repo_root() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let error = resolve_pack_control_path(temp_dir.path(), Path::new("specs/control.json"))
            .expect_err("markerless pack must not borrow cwd repository authority");
        assert!(
            error
                .to_string()
                .contains("no marker-bearing repository root")
        );
    }

    #[test]
    fn output_dirs_prefer_existing_base_parent_for_non_repo_paths() {
        let base = std::env::temp_dir();
        let resolved = resolve_output_dir(&base, Path::new("nested/out-dir"));
        assert_eq!(resolved, base.join("nested/out-dir"));
    }

    #[test]
    fn target_output_dirs_anchor_to_marker_root_even_when_target_is_absent() {
        let temp_root = std::env::temp_dir().join(format!(
            "path-resolution-target-anchor-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        let spec_dir = temp_root.join("specs/reference/scope");
        fs::create_dir_all(&spec_dir).expect("create spec dir");
        fs::write(temp_root.join("justfile"), "").expect("write justfile marker");
        fs::write(temp_root.join("AGENTS.md"), "").expect("write AGENTS marker");
        let canonical_temp_root = temp_root.canonicalize().expect("canonical temp root");

        let resolved =
            resolve_output_dir(&spec_dir, Path::new("target/reference-regen/scope/plan"));

        assert_eq!(
            resolved,
            canonical_temp_root.join("target/reference-regen/scope/plan")
        );
        fs::remove_dir_all(temp_root).expect("remove marker-root temp dir");
    }

    #[test]
    fn target_output_dirs_without_repo_markers_remain_base_relative() {
        let temp_root = std::env::temp_dir().join(format!(
            "path-resolution-target-markerless-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        let spec_dir = temp_root.join("specs/reference/scope");
        fs::create_dir_all(&spec_dir).expect("create markerless spec dir");

        let resolved =
            resolve_output_dir(&spec_dir, Path::new("target/reference-regen/scope/plan"));

        assert_eq!(resolved, spec_dir.join("target/reference-regen/scope/plan"));
        fs::remove_dir_all(temp_root).expect("remove markerless temp dir");
    }

    #[test]
    fn target_input_paths_anchor_to_marker_root_before_cwd_decoys() {
        let temp_root = std::env::temp_dir().join(format!(
            "path-resolution-target-input-anchor-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        let spec_dir = temp_root.join("specs/reference/scope");
        fs::create_dir_all(&spec_dir).expect("create spec dir");
        fs::write(temp_root.join("justfile"), "").expect("write justfile marker");
        fs::write(temp_root.join("AGENTS.md"), "").expect("write AGENTS marker");
        let canonical_temp_root = temp_root.canonicalize().expect("canonical temp root");
        let relative = Path::new("target/path-resolution-input-decoy/plan.json");
        let anchored = canonical_temp_root.join(relative);
        fs::create_dir_all(anchored.parent().expect("anchored parent"))
            .expect("create anchored parent");
        fs::write(&anchored, b"anchored").expect("write anchored input");
        let cwd_decoy = std::env::current_dir().expect("current dir").join(relative);
        let _ = fs::remove_file(&cwd_decoy);
        fs::create_dir_all(cwd_decoy.parent().expect("cwd decoy parent"))
            .expect("create cwd decoy parent");
        fs::write(&cwd_decoy, b"decoy").expect("write cwd decoy");

        let resolved = resolve_existing_path(&spec_dir, relative);

        assert_eq!(resolved, anchored);
        assert_eq!(
            fs::read(&resolved).expect("read resolved input"),
            b"anchored"
        );
        fs::remove_file(cwd_decoy).expect("remove cwd decoy");
        fs::remove_dir_all(temp_root).expect("remove marker-root temp dir");
    }

    #[test]
    fn missing_target_input_paths_report_marker_root_location() {
        let temp_root = std::env::temp_dir().join(format!(
            "path-resolution-target-input-missing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp_root);
        let spec_dir = temp_root.join("specs/reference/scope");
        fs::create_dir_all(&spec_dir).expect("create spec dir");
        fs::write(temp_root.join("justfile"), "").expect("write justfile marker");
        fs::write(temp_root.join("AGENTS.md"), "").expect("write AGENTS marker");
        let canonical_temp_root = temp_root.canonicalize().expect("canonical temp root");
        let relative = Path::new("target/reference-regen/missing/plan.json");
        let anchored = canonical_temp_root.join(relative);

        let resolved = resolve_existing_path(&spec_dir, relative);
        let err = fs::read(&resolved)
            .map_err(|error| format!("read {}: {error}", resolved.display()))
            .expect_err("missing target input should fail at anchored path");

        assert_eq!(resolved, anchored);
        assert!(
            err.contains(&anchored.display().to_string()),
            "error did not name anchored path: {err}"
        );
        fs::remove_dir_all(temp_root).expect("remove marker-root temp dir");
    }

    #[test]
    fn target_output_dirs_do_not_expand_portable_artifact_prefixes() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root");
        let absolute_target_artifact = repo_root.join("target/reference-regen/scope/artifact.json");

        let err = portable_artifact_path_for_spec(
            &absolute_target_artifact,
            Path::new("target/reference-regen/scope/artifact.json"),
        )
        .expect_err("target scratch output requires a stable artifact identity");
        assert!(
            err.to_string().contains("canonical repo-relative identity"),
            "{err}"
        );
    }

    #[test]
    fn repo_relative_spec_paths_fail_when_they_cannot_be_made_portable() {
        let err = portable_artifact_path_for_spec(
            Path::new("/not/inside/this/repo/specs/reference/artifact.json"),
            Path::new("specs/reference/artifact.json"),
        )
        .expect_err("repo-relative spec path must fail loud if it stays absolute");
        assert!(
            err.to_string().contains("canonical repo-relative identity"),
            "{err}"
        );
    }

    #[test]
    fn markerless_absolute_specs_paths_are_not_misclassified_as_repo_relative() {
        let temp_root =
            std::env::temp_dir().join(format!("path-resolution-markerless-{}", std::process::id()));
        let artifact = temp_root.join("specs/reference/artifact.json");
        fs::create_dir_all(artifact.parent().expect("artifact parent"))
            .expect("create markerless specs dir");
        fs::write(&artifact, "{}").expect("write markerless artifact");

        let err =
            portable_artifact_path_for_spec(&artifact, Path::new("specs/reference/artifact.json"))
                .expect_err("markerless /specs path must not be treated as this repo");
        assert!(
            err.to_string().contains("canonical repo-relative identity"),
            "{err}"
        );

        fs::remove_dir_all(temp_root).expect("remove markerless specs dir");
    }

    #[test]
    fn portable_artifact_paths_canonicalize_parent_components() {
        let absolute_with_parent_components = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../specs/023-nt-research-analytics-platform/reference");
        assert_eq!(
            portable_artifact_path_for_spec(
                &absolute_with_parent_components,
                Path::new("specs/023-nt-research-analytics-platform/reference"),
            )
            .expect("repo path remains portable"),
            Path::new("specs/023-nt-research-analytics-platform/reference")
        );
    }

    #[test]
    fn non_repo_temp_paths_fail_instead_of_serializing_absolute_locations() {
        let path = std::env::temp_dir().join("path-resolution-temp-artifact.json");
        let err = portable_artifact_path_for_spec(&path, Path::new("temp-artifact.json"))
            .expect_err("non-repo temp artifact path must not be serialized");
        assert!(
            err.to_string().contains("canonical repo-relative identity"),
            "{err}"
        );
    }
}
