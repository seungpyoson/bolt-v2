use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize)]
pub struct DryRunArtifact {
    pub schema_version: u32,
    pub mode: String,
    pub lane: String,
    pub requested_ref: String,
    pub resolved_nt_sha: Option<String>,
    pub previous_nt_sha: Option<String>,
    pub upstream_repo_root: Option<String>,
    pub upstream_diff: UpstreamDiffArtifact,
    pub inventory: InventoryArtifact,
    pub registry: RegistryArtifact,
    pub required_seams: Vec<String>,
    pub required_canaries: Vec<CanaryArtifact>,
    pub isolated_run: IsolatedRunArtifact,
    pub result: String,
    pub failures: Vec<String>,
    pub toolchain: ToolchainArtifact,
    pub timestamps: TimestampArtifact,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpstreamDiffArtifact {
    pub identity: Option<String>,
    pub changed_paths: Vec<ChangedPathArtifact>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedPathArtifact {
    pub path: String,
    pub classification: String,
    pub seams: Vec<String>,
    pub safe_list_entry: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct InventoryArtifact {
    pub manifest: BTreeMap<String, String>,
    pub production: Vec<InventoryEntryArtifact>,
    pub test_support: Vec<InventoryEntryArtifact>,
    pub registry_gaps: Vec<RegistryGapArtifact>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InventoryEntryArtifact {
    pub path: String,
    pub referenced_crates: Vec<String>,
    pub owned_by_seams: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistryGapArtifact {
    pub path: String,
    pub section: String,
    pub referenced_crates: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RegistryArtifact {
    pub registry_digest: Option<String>,
    pub safe_list_digest: Option<String>,
    pub replay_set_digest: Option<String>,
    pub prefix_gaps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanaryArtifact {
    pub id: String,
    pub path: String,
    pub coverage: String,
    pub status: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IsolatedRunArtifact {
    pub worktree_path: Option<String>,
    pub cargo_lock_digest: Option<String>,
    pub updated_manifest_revs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolchainArtifact {
    pub rust_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimestampArtifact {
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

impl DryRunArtifact {
    pub fn new(lane: &str, requested_ref: &str) -> Self {
        Self {
            schema_version: 1,
            mode: "dry-run".to_string(),
            lane: lane.to_string(),
            requested_ref: requested_ref.to_string(),
            resolved_nt_sha: None,
            previous_nt_sha: None,
            upstream_repo_root: None,
            upstream_diff: UpstreamDiffArtifact::default(),
            inventory: InventoryArtifact::default(),
            registry: RegistryArtifact::default(),
            required_seams: Vec::new(),
            required_canaries: Vec::new(),
            isolated_run: IsolatedRunArtifact::default(),
            result: "fail".to_string(),
            failures: Vec::new(),
            toolchain: ToolchainArtifact {
                rust_version: rust_version().ok(),
            },
            timestamps: TimestampArtifact {
                started_at: Utc::now(),
                finished_at: None,
            },
        }
    }

    pub fn push_failure(&mut self, failure: impl Into<String>) {
        let failure = failure.into();
        if !self.failures.iter().any(|existing| existing == &failure) {
            self.failures.push(failure);
        }
    }

    pub fn finalize(&mut self) {
        self.failures.sort();
        self.required_seams.sort();
        self.required_seams.dedup();
        self.timestamps.finished_at = Some(Utc::now());
        self.result = if self.failures.is_empty() {
            "pass".to_string()
        } else {
            "fail".to_string()
        };
    }
}

pub fn write_artifact_atomic(artifact: &DryRunArtifact, output_path: &Path) -> Result<()> {
    let serialized =
        serde_json::to_vec_pretty(artifact).context("failed to serialize dry-run artifact")?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create parent directory for artifact {}",
                output_path.display()
            )
        })?;
    }
    let temp_path = temp_artifact_path(output_path);
    fs::write(&temp_path, serialized).with_context(|| {
        format!(
            "failed to write temporary artifact file {}",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, output_path).with_context(|| {
        format!(
            "failed to atomically publish artifact {}",
            output_path.display()
        )
    })?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let contents =
        fs::read(path).with_context(|| format!("failed to read {} for sha256", path.display()))?;
    Ok(sha256_bytes(&contents))
}

pub fn sha256_string(contents: &str) -> String {
    sha256_bytes(contents.as_bytes())
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn temp_artifact_path(output_path: &Path) -> PathBuf {
    let mut temp_name = output_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "artifact.json".to_string());
    temp_name.push_str(".tmp");
    output_path.with_file_name(temp_name)
}

fn rust_version() -> Result<String> {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .context("failed to execute rustc --version")?;
    let version =
        String::from_utf8(output.stdout).context("rustc --version output was not valid UTF-8")?;
    Ok(version.trim().to_string())
}
