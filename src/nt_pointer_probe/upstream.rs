use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, anyhow, ensure};

#[derive(Debug, Clone)]
pub struct ResolvedUpstream {
    pub repo_root: PathBuf,
    pub resolved_sha: String,
}

pub fn resolve_upstream_repo(
    current_sha: &str,
    source_ref: &str,
    upstream_repo_root: Option<&Path>,
) -> Result<ResolvedUpstream> {
    if upstream_repo_root.is_none() && !looks_like_full_sha(source_ref) {
        return Err(anyhow!(
            "non-SHA source ref {} requires --upstream-repo-root for deterministic local dry-run",
            source_ref
        ));
    }
    let repo_root = match upstream_repo_root {
        Some(path) => path.to_path_buf(),
        None => select_cached_checkout(current_sha, source_ref)?,
    };
    ensure!(
        repo_root.join(".git").exists(),
        "upstream repo root {} does not look like a git checkout",
        repo_root.display()
    );
    let resolved_sha = git_stdout(
        &repo_root,
        &["rev-parse", &format!("{source_ref}^{{commit}}")],
    )
    .with_context(|| {
        format!(
            "failed to resolve source ref {} in upstream repo {}",
            source_ref,
            repo_root.display()
        )
    })?;
    git_stdout(
        &repo_root,
        &["rev-parse", &format!("{current_sha}^{{commit}}")],
    )
    .with_context(|| {
        format!(
            "current pinned SHA {} not found in upstream repo {}",
            current_sha,
            repo_root.display()
        )
    })?;
    Ok(ResolvedUpstream {
        repo_root,
        resolved_sha,
    })
}

pub fn diff_changed_paths(repo_root: &Path, base_sha: &str, head_sha: &str) -> Result<Vec<String>> {
    let output = git_stdout(repo_root, &["diff", "--name-only", base_sha, head_sha])?;
    let mut paths = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn select_cached_checkout(current_sha: &str, source_ref: &str) -> Result<PathBuf> {
    let home = env::var("HOME").context("HOME must be set to scan cached NT checkouts")?;
    let checkouts_root = Path::new(&home).join(".cargo/git/checkouts");
    let mut candidates = Vec::new();

    for root_entry in fs::read_dir(&checkouts_root).with_context(|| {
        format!(
            "failed to read cached NT checkout directory {}",
            checkouts_root.display()
        )
    })? {
        let root_entry = root_entry?;
        let root_path = root_entry.path();
        let Some(name) = root_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("nautilus_trader-") {
            continue;
        }
        for repo_entry in fs::read_dir(&root_path)
            .with_context(|| format!("failed to inspect cached checkout {}", root_path.display()))?
        {
            let repo_entry = repo_entry?;
            let repo_path = repo_entry.path();
            if !repo_path.join(".git").exists() {
                continue;
            }
            let resolves_current = git_stdout(
                &repo_path,
                &["rev-parse", &format!("{current_sha}^{{commit}}")],
            )
            .is_ok();
            let resolves_source = git_stdout(
                &repo_path,
                &["rev-parse", &format!("{source_ref}^{{commit}}")],
            )
            .is_ok();
            if resolves_current && resolves_source {
                candidates.push(repo_path);
            }
        }
    }

    candidates.sort();
    candidates.into_iter().next().ok_or_else(|| {
        anyhow!(
            "no cached NT checkout resolves both {} and {}",
            current_sha,
            source_ref
        )
    })
}

fn looks_like_full_sha(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn git_stdout(repo_root: &Path, args: &[&str]) -> Result<String> {
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
        .trim()
        .to_string())
}
