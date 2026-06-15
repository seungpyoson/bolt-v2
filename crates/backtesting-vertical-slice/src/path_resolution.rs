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
//! 3. otherwise the working-directory-relative or `base_dir`-relative
//!    candidate wins.

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

const REPO_TOP_LEVEL_DIRS: [&str; 4] = ["specs", "crates", "docs", "scripts"];
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
    if path.exists() {
        return path.to_path_buf();
    }
    let base_candidate = base_dir.join(path);
    if base_candidate.exists() {
        return base_candidate;
    }
    resolve_from_known_anchors(path).unwrap_or_else(|| path.to_path_buf())
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
/// form so committed artifacts stay portable across checkouts; non-repo
/// paths pass through unchanged.
#[must_use]
pub fn portable_artifact_path(path: &Path) -> PathBuf {
    candidate_portable_artifact_path(path).unwrap_or_else(|| path.to_path_buf())
}

/// Rewrite an artifact path for serialization, failing if a repo-relative spec
/// input/output cannot be represented as a repo-relative committed path.
pub fn portable_artifact_path_for_spec(path: &Path, spec_path: &Path) -> Result<PathBuf> {
    let Some(portable) = candidate_portable_artifact_path(path) else {
        if looks_repo_relative(spec_path) {
            bail!(
                "repo-relative artifact path {} resolved to {} but could not be serialized portably",
                spec_path.display(),
                path.display()
            );
        }
        return Ok(path.to_path_buf());
    };
    Ok(portable)
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
    fn output_dirs_prefer_existing_base_parent_for_non_repo_paths() {
        let base = std::env::temp_dir();
        let resolved = resolve_output_dir(&base, Path::new("nested/out-dir"));
        assert_eq!(resolved, base.join("nested/out-dir"));
    }

    #[test]
    fn repo_relative_spec_paths_fail_when_they_cannot_be_made_portable() {
        let err = portable_artifact_path_for_spec(
            Path::new("/not/inside/this/repo/specs/reference/artifact.json"),
            Path::new("specs/reference/artifact.json"),
        )
        .expect_err("repo-relative spec path must fail loud if it stays absolute");
        assert!(
            err.to_string().contains("could not be serialized portably"),
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
            err.to_string().contains("could not be serialized portably"),
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
    fn non_repo_temp_paths_can_remain_absolute() {
        let path = std::env::temp_dir().join("path-resolution-temp-artifact.json");
        assert_eq!(
            portable_artifact_path_for_spec(&path, Path::new("temp-artifact.json"))
                .expect("non-repo temp artifact path may remain absolute"),
            path
        );
    }
}
