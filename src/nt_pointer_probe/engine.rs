use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, ensure};
use clap::ValueEnum;
use tempfile::TempDir;
use toml::Value as TomlValue;

use super::{
    classify,
    control::{CanaryEntry, LoadedControlPlane},
    evidence::{CanaryArtifact, DryRunArtifact, sha256_file, sha256_string, write_artifact_atomic},
    inventory, upstream,
};

#[derive(Debug, Clone)]
pub struct DryRunRequest {
    pub repo_root: PathBuf,
    pub lane: ProbeLane,
    pub source_ref: String,
    pub artifact_out: PathBuf,
    pub upstream_repo_root: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ProbeLane {
    Develop,
    TaggedRelease,
}

impl ProbeLane {
    fn as_str(self) -> &'static str {
        match self {
            Self::Develop => "develop",
            Self::TaggedRelease => "tagged-release",
        }
    }
}

pub fn run_dry_run(request: DryRunRequest) -> Result<DryRunArtifact> {
    let workspace = TempProbeWorkspace::new(&request.repo_root)?;
    workspace.sync_from_working_tree()?;
    let loaded = LoadedControlPlane::load_from_repo_root(&workspace.path)?;
    let mut artifact = DryRunArtifact::new(request.lane.as_str(), &request.source_ref);
    let configured = loaded
        .control
        .nt_crates
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();

    artifact.registry.registry_digest = Some(sha256_file(
        &workspace.path.join(&loaded.control.paths.registry),
    )?);
    artifact.registry.safe_list_digest = Some(sha256_file(
        &workspace.path.join(&loaded.control.paths.safe_list),
    )?);
    artifact.registry.replay_set_digest = Some(sha256_file(
        &workspace.path.join(&loaded.control.paths.replay_set),
    )?);

    let current_sha = current_nt_revision(
        &workspace.path.join("Cargo.toml"),
        &configured,
        &mut artifact.inventory.manifest,
    )?;
    artifact.previous_nt_sha = Some(current_sha.clone());

    let resolved = match upstream::resolve_upstream_repo(
        &current_sha,
        &request.source_ref,
        request.upstream_repo_root.as_deref(),
    ) {
        Ok(resolved) => resolved,
        Err(err) => {
            artifact.push_failure("upstream_ref_resolution_failed");
            artifact.upstream_diff.identity = Some(sha256_string(&format!(
                "{}:{}",
                current_sha, request.source_ref
            )));
            artifact.finalize();
            write_artifact_atomic(&artifact, &request.artifact_out)?;
            return Err(err);
        }
    };
    artifact.resolved_nt_sha = Some(resolved.resolved_sha.clone());
    artifact.upstream_repo_root = Some(resolved.repo_root.display().to_string());

    let changed_paths =
        upstream::diff_changed_paths(&resolved.repo_root, &current_sha, &resolved.resolved_sha)?;
    artifact.upstream_diff.identity = Some(sha256_string(&format!(
        "{}:{}:{}",
        current_sha,
        resolved.resolved_sha,
        changed_paths.join("\n")
    )));
    artifact.upstream_diff.changed_paths =
        classify::classify_changed_paths(&changed_paths, &loaded.registry, &loaded.safe_list);
    for failure in classify::apply_replay_expectations(
        &mut artifact.upstream_diff.changed_paths,
        &loaded.replay_set,
    ) {
        artifact.push_failure(failure);
    }

    if artifact
        .upstream_diff
        .changed_paths
        .iter()
        .any(|entry| entry.classification == "ambiguous")
    {
        artifact.push_failure("ambiguous_upstream_path");
    }

    let inventory = inventory::collect_source_inventory(&workspace.path, &loaded.registry)?;
    artifact.inventory.production = inventory.production;
    artifact.inventory.test_support = inventory.test_support;
    artifact.inventory.registry_gaps = inventory.registry_gaps;
    if !artifact.inventory.registry_gaps.is_empty() {
        artifact.push_failure("registry_gap");
    }

    let prefix_gaps = classify::registry_prefix_gaps(&resolved.repo_root, &loaded.registry)?;
    if !prefix_gaps.is_empty() {
        artifact.registry.prefix_gaps = prefix_gaps.clone();
        artifact.push_failure("registry_prefix_gap");
    }

    artifact.required_seams = classify::required_seams(&artifact.upstream_diff.changed_paths);
    let required_canaries = classify::required_canaries(&loaded.registry, &artifact.required_seams);
    artifact.required_canaries = required_canaries
        .iter()
        .map(|canary| CanaryArtifact {
            id: canary.id.clone(),
            path: canary.path.clone(),
            coverage: canary.coverage.clone(),
            status: "pending".to_string(),
            details: None,
        })
        .collect();

    artifact.isolated_run.worktree_path = Some(workspace.path.display().to_string());
    match rewrite_manifest_to_revision(
        &workspace.path.join("Cargo.toml"),
        &configured,
        &resolved.resolved_sha,
        &mut artifact.isolated_run.updated_manifest_revs,
    ) {
        Ok(()) => {}
        Err(err) => {
            artifact.push_failure("manifest_update_failed");
            artifact.finalize();
            write_artifact_atomic(&artifact, &request.artifact_out)?;
            return Err(err);
        }
    }

    if let Err(err) = refresh_lockfile(&workspace.path) {
        artifact.push_failure("lockfile_refresh_failed");
        for canary in &mut artifact.required_canaries {
            canary.status = "skipped".to_string();
            canary.details = Some("lockfile refresh failed".to_string());
        }
        artifact.finalize();
        write_artifact_atomic(&artifact, &request.artifact_out)?;
        return Err(err);
    }
    let lock_path = workspace.path.join("Cargo.lock");
    if lock_path.exists() {
        artifact.isolated_run.cargo_lock_digest = Some(sha256_file(&lock_path)?);
    }

    let skip_canaries = !artifact.failures.is_empty();
    if skip_canaries {
        for canary in &mut artifact.required_canaries {
            canary.status = "skipped".to_string();
            canary.details = Some("pre-canary failures present".to_string());
        }
    } else {
        let mut canary_failed = false;
        for (artifact_canary, canary) in artifact
            .required_canaries
            .iter_mut()
            .zip(required_canaries.iter())
        {
            match run_canary(&workspace.path, canary) {
                Ok(output) => {
                    artifact_canary.status = "passed".to_string();
                    artifact_canary.details = Some(output);
                }
                Err(err) => {
                    artifact_canary.status = "failed".to_string();
                    artifact_canary.details = Some(err.to_string());
                    canary_failed = true;
                }
            }
        }
        if canary_failed {
            artifact.push_failure("canary_failed");
        }
    }

    artifact.finalize();
    write_artifact_atomic(&artifact, &request.artifact_out)?;
    Ok(artifact)
}

fn current_nt_revision(
    cargo_toml_path: &Path,
    configured: &BTreeSet<String>,
    manifest_inventory: &mut BTreeMap<String, String>,
) -> Result<String> {
    let contents = fs::read_to_string(cargo_toml_path)
        .with_context(|| format!("failed to read {}", cargo_toml_path.display()))?;
    let parsed: TomlValue =
        toml::from_str(&contents).context("failed to parse Cargo.toml for NT revision scan")?;
    let Some(table) = parsed.as_table() else {
        return Err(anyhow!("Cargo.toml root must be a table"));
    };
    let mut revs = BTreeSet::new();
    collect_nt_dependency_data(
        table,
        &mut Vec::new(),
        configured,
        &mut revs,
        manifest_inventory,
    );
    ensure!(
        !revs.is_empty(),
        "no configured NT dependency revisions found in Cargo.toml"
    );
    ensure!(
        revs.len() == 1,
        "configured NT crates must all share one revision, found {:?}",
        revs
    );
    Ok(revs
        .into_iter()
        .next()
        .expect("revs should contain one value"))
}

fn rewrite_manifest_to_revision(
    cargo_toml_path: &Path,
    configured: &BTreeSet<String>,
    revision: &str,
    updated_manifest_revs: &mut BTreeMap<String, String>,
) -> Result<()> {
    let contents = fs::read_to_string(cargo_toml_path)
        .with_context(|| format!("failed to read {}", cargo_toml_path.display()))?;
    let mut parsed: TomlValue =
        toml::from_str(&contents).context("failed to parse Cargo.toml for update")?;
    let Some(table) = parsed.as_table_mut() else {
        return Err(anyhow!("Cargo.toml root must be a table"));
    };
    update_nt_dependency_revisions(
        table,
        &mut Vec::new(),
        configured,
        revision,
        updated_manifest_revs,
    );
    let serialized =
        toml::to_string_pretty(&parsed).context("failed to serialize updated Cargo.toml")?;
    fs::write(cargo_toml_path, serialized)
        .with_context(|| format!("failed to write {}", cargo_toml_path.display()))?;
    Ok(())
}

fn collect_nt_dependency_data(
    table: &toml::map::Map<String, TomlValue>,
    path: &mut Vec<String>,
    configured: &BTreeSet<String>,
    revs: &mut BTreeSet<String>,
    manifest_inventory: &mut BTreeMap<String, String>,
) {
    for (key, value) in table {
        path.push(key.clone());
        if (is_dependency_table(path) || path.first().is_some_and(|segment| segment == "replace"))
            && let Some(dep_table) = value.as_table()
        {
            for (dependency_name, dependency_spec) in dep_table {
                if !configured.contains(dependency_name) {
                    continue;
                }
                if let Some(dependency_table) = dependency_spec.as_table()
                    && let Some(rev) = dependency_table.get("rev").and_then(TomlValue::as_str)
                {
                    revs.insert(rev.to_string());
                    manifest_inventory.insert(dependency_name.clone(), rev.to_string());
                }
            }
        }
        if let Some(nested) = value.as_table() {
            collect_nt_dependency_data(nested, path, configured, revs, manifest_inventory);
        }
        path.pop();
    }
}

fn update_nt_dependency_revisions(
    table: &mut toml::map::Map<String, TomlValue>,
    path: &mut Vec<String>,
    configured: &BTreeSet<String>,
    revision: &str,
    updated_manifest_revs: &mut BTreeMap<String, String>,
) {
    for (key, value) in table.iter_mut() {
        path.push(key.clone());
        if (is_dependency_table(path) || path.first().is_some_and(|segment| segment == "replace"))
            && let Some(dep_table) = value.as_table_mut()
        {
            for (dependency_name, dependency_spec) in dep_table.iter_mut() {
                if !configured.contains(dependency_name) {
                    continue;
                }
                if let Some(dependency_table) = dependency_spec.as_table_mut() {
                    dependency_table
                        .insert("rev".to_string(), TomlValue::String(revision.to_string()));
                    updated_manifest_revs.insert(dependency_name.clone(), revision.to_string());
                }
            }
        }
        if let Some(nested) = value.as_table_mut() {
            update_nt_dependency_revisions(
                nested,
                path,
                configured,
                revision,
                updated_manifest_revs,
            );
        }
        path.pop();
    }
}

fn is_dependency_table(path: &[String]) -> bool {
    matches!(
        path.last().map(String::as_str),
        Some("dependencies") | Some("dev-dependencies") | Some("build-dependencies")
    )
}

fn refresh_lockfile(worktree_path: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .args(["generate-lockfile"])
        .current_dir(worktree_path)
        .output()
        .with_context(|| {
            format!(
                "failed to run cargo generate-lockfile in {}",
                worktree_path.display()
            )
        })?;
    ensure!(
        output.status.success(),
        "cargo generate-lockfile failed in {}: {}",
        worktree_path.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

fn run_canary(worktree_path: &Path, canary: &CanaryEntry) -> Result<String> {
    let path = worktree_path.join(&canary.path);
    ensure!(
        path.exists(),
        "canary path {} does not exist",
        path.display()
    );
    let output = if canary.path.ends_with(".sh") {
        Command::new("bash")
            .arg(&path)
            .current_dir(worktree_path)
            .output()
            .with_context(|| format!("failed to execute shell canary {}", canary.id))?
    } else if canary.path.starts_with("tests/") {
        let test_name = Path::new(&canary.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| anyhow!("invalid canary test path {}", canary.path))?;
        let filter = canary
            .id
            .rsplit("::")
            .next()
            .ok_or_else(|| anyhow!("invalid canary id {}", canary.id))?;
        ensure_exact_test_match(
            worktree_path,
            &[
                "test", "--test", test_name, filter, "--", "--exact", "--list",
            ],
            &canary.id,
        )?;
        Command::new("cargo")
            .args([
                "test",
                "--test",
                test_name,
                filter,
                "--",
                "--exact",
                "--nocapture",
            ])
            .current_dir(worktree_path)
            .output()
            .with_context(|| format!("failed to execute integration canary {}", canary.id))?
    } else if canary.path.starts_with("src/") {
        let filter = lib_test_selector(canary)?;
        ensure_exact_test_match(
            worktree_path,
            &["test", "--lib", &filter, "--", "--exact", "--list"],
            &canary.id,
        )?;
        Command::new("cargo")
            .args(["test", "--lib", &filter, "--", "--exact", "--nocapture"])
            .current_dir(worktree_path)
            .output()
            .with_context(|| format!("failed to execute unit canary {}", canary.id))?
    } else {
        return Err(anyhow!("unsupported canary path {}", canary.path));
    };

    let stdout = String::from_utf8(output.stdout).context("canary stdout was not valid UTF-8")?;
    let stderr = String::from_utf8(output.stderr).context("canary stderr was not valid UTF-8")?;
    let detail = format_command_output(&stdout, &stderr);
    ensure!(
        output.status.success(),
        "canary {} failed: {}",
        canary.id,
        detail
    );
    Ok(detail)
}

struct TempProbeWorkspace {
    repo_root: PathBuf,
    _holder: TempDir,
    path: PathBuf,
}

impl TempProbeWorkspace {
    fn new(repo_root: &Path) -> Result<Self> {
        let holder = tempfile::tempdir().context("failed to create temporary probe workspace")?;
        let path = holder.path().join("worktree");
        let output = Command::new("git")
            .args(["worktree", "add", "--detach"])
            .arg(&path)
            .arg("HEAD")
            .current_dir(repo_root)
            .output()
            .with_context(|| format!("failed to add git worktree from {}", repo_root.display()))?;
        ensure!(
            output.status.success(),
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(Self {
            repo_root: repo_root.to_path_buf(),
            _holder: holder,
            path,
        })
    }

    fn sync_from_working_tree(&self) -> Result<()> {
        for relative in git_lines(&self.repo_root, &["ls-files", "-co", "--exclude-standard"])? {
            let source = self.repo_root.join(&relative);
            let destination = self.path.join(&relative);
            if source.is_dir() {
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "failed to create parent directory while syncing {}",
                        destination.display()
                    )
                })?;
            }
            if source.exists() {
                fs::copy(&source, &destination).with_context(|| {
                    format!(
                        "failed to copy working-tree snapshot from {} to {}",
                        source.display(),
                        destination.display()
                    )
                })?;
                let permissions = fs::metadata(&source)
                    .with_context(|| format!("failed to read metadata for {}", source.display()))?
                    .permissions();
                fs::set_permissions(&destination, permissions).with_context(|| {
                    format!(
                        "failed to copy permissions from {} to {}",
                        source.display(),
                        destination.display()
                    )
                })?;
            }
        }
        for relative in git_lines(&self.repo_root, &["ls-files", "-d"])? {
            let destination = self.path.join(relative);
            if destination.exists() {
                fs::remove_file(&destination).with_context(|| {
                    format!(
                        "failed to remove deleted working-tree file {}",
                        destination.display()
                    )
                })?;
            }
        }
        Ok(())
    }
}

impl Drop for TempProbeWorkspace {
    fn drop(&mut self) {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.path)
            .current_dir(&self.repo_root)
            .output();
    }
}

fn git_lines(repo_root: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("failed to run git {:?} in {}", args, repo_root.display()))?;
    ensure!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        repo_root.display(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)
        .context("git output was not valid UTF-8")?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .collect())
}

fn lib_test_selector(canary: &CanaryEntry) -> Result<String> {
    let suffix = canary
        .id
        .strip_prefix(&format!("{}::", canary.path))
        .ok_or_else(|| {
            anyhow!(
                "canary id {} does not align with path {}",
                canary.id,
                canary.path
            )
        })?;
    let module_path = canary
        .path
        .trim_start_matches("src/")
        .trim_end_matches(".rs")
        .replace('/', "::");
    if module_path == "lib" {
        Ok(suffix.to_string())
    } else {
        Ok(format!("{module_path}::{suffix}"))
    }
}

fn ensure_exact_test_match(worktree_path: &Path, args: &[&str], canary_id: &str) -> Result<()> {
    let output = Command::new("cargo")
        .args(args)
        .current_dir(worktree_path)
        .output()
        .with_context(|| format!("failed to list exact test selector for {}", canary_id))?;
    ensure!(
        output.status.success(),
        "cargo test list failed for {}: {}",
        canary_id,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let stdout =
        String::from_utf8(output.stdout).context("cargo test --list output was not valid UTF-8")?;
    let matches = stdout
        .lines()
        .filter(|line| line.trim_end().ends_with(": test"))
        .count();
    ensure!(
        matches == 1,
        "expected exactly one matching test for {}, found {}",
        canary_id,
        matches
    );
    Ok(())
}

fn format_command_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => "command produced no output".to_string(),
        (false, true) => stdout.trim().to_string(),
        (true, false) => stderr.trim().to_string(),
        (false, false) => format!("{}\n{}", stdout.trim(), stderr.trim()),
    }
}
