use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use toml::Value;

use crate::{
    bolt_v3_config::LoadedBoltV3Config, bolt_v3_decision_event_context::BoltV3DecisionEventIdentity,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BoltV3ReleaseIdentityManifest {
    pub release_id: String,
    pub git_commit_sha: String,
    pub nautilus_trader_revision: String,
    pub binary_sha256: String,
    pub cargo_lock_sha256: String,
    pub config_hash: String,
    pub build_profile: String,
    pub artifact_sha256: BTreeMap<String, String>,
}

pub fn load_bolt_v3_release_identity(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3DecisionEventIdentity> {
    let manifest_path = Path::new(&loaded.root.release.identity_manifest_path);
    let manifest_text = fs::read_to_string(manifest_path).with_context(|| {
        format!(
            "failed to read release identity manifest {}",
            manifest_path.display()
        )
    })?;
    let manifest: BoltV3ReleaseIdentityManifest =
        toml::from_str(&manifest_text).with_context(|| {
            format!(
                "failed to parse release identity manifest {}",
                manifest_path.display()
            )
        })?;

    validate_manifest(&manifest)?;

    let compiled_revision = bolt_v3_compiled_nautilus_trader_revision()?;
    if manifest.nautilus_trader_revision != compiled_revision {
        bail!(
            "release identity manifest nautilus_trader_revision mismatch: manifest `{}` != Cargo.toml `{}`",
            manifest.nautilus_trader_revision,
            compiled_revision
        );
    }

    let computed_config_hash = bolt_v3_config_hash(loaded)?;
    if manifest.config_hash != computed_config_hash {
        bail!(
            "release identity manifest config_hash mismatch: manifest `{}` != computed `{}`",
            manifest.config_hash,
            computed_config_hash
        );
    }

    Ok(BoltV3DecisionEventIdentity {
        release_id: manifest.release_id,
        config_hash: manifest.config_hash,
        nautilus_trader_revision: manifest.nautilus_trader_revision,
    })
}

pub fn bolt_v3_config_hash(loaded: &LoadedBoltV3Config) -> Result<String> {
    let mut hasher = Sha256::new();
    update_hash_with_normalized_file(&mut hasher, &loaded.root_path)?;
    for strategy in &loaded.strategies {
        update_hash_with_normalized_file(&mut hasher, &strategy.config_path)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn bolt_v3_compiled_nautilus_trader_revision() -> Result<String> {
    let manifest: Value =
        toml::from_str(include_str!("../Cargo.toml")).context("failed to parse Cargo.toml")?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(Value::as_table)
        .context("Cargo.toml missing [dependencies]")?;
    let mut revisions = BTreeMap::new();

    for (name, dependency) in dependencies {
        if !name.starts_with("nautilus-") {
            continue;
        }
        let Some(revision) = dependency.get("rev").and_then(Value::as_str) else {
            bail!("Cargo.toml dependency `{name}` missing rev");
        };
        revisions.insert(name.as_str(), revision);
    }

    let Some(first_revision) = revisions.values().next().copied() else {
        bail!("Cargo.toml has no nautilus-* git dependencies");
    };
    for (name, revision) in &revisions {
        if *revision != first_revision {
            bail!(
                "Cargo.toml dependency `{name}` rev `{revision}` does not match `{first_revision}`"
            );
        }
    }
    Ok(first_revision.to_string())
}

fn update_hash_with_normalized_file(hasher: &mut Sha256, path: &Path) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut text = String::from_utf8(bytes)
        .with_context(|| format!("{} must be UTF-8 for config hashing", path.display()))?;
    if text.starts_with('\u{feff}') {
        text.replace_range(..'\u{feff}'.len_utf8(), "");
    }
    let mut normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    while normalized.ends_with('\n') {
        normalized.pop();
    }
    normalized.push('\n');
    hasher.update(normalized.as_bytes());
    Ok(())
}

fn validate_manifest(manifest: &BoltV3ReleaseIdentityManifest) -> Result<()> {
    validate_non_empty("release_id", &manifest.release_id)?;
    validate_non_empty("git_commit_sha", &manifest.git_commit_sha)?;
    validate_non_empty(
        "nautilus_trader_revision",
        &manifest.nautilus_trader_revision,
    )?;
    validate_non_empty("build_profile", &manifest.build_profile)?;
    validate_sha256("binary_sha256", &manifest.binary_sha256)?;
    validate_sha256("cargo_lock_sha256", &manifest.cargo_lock_sha256)?;
    validate_sha256("config_hash", &manifest.config_hash)?;
    if manifest.artifact_sha256.is_empty() {
        bail!("artifact_sha256 must contain at least one artifact digest");
    }
    for (name, digest) in &manifest.artifact_sha256 {
        validate_non_empty("artifact_sha256 artifact name", name)?;
        validate_sha256(&format!("artifact_sha256.{name}"), digest)?;
    }
    Ok(())
}

fn validate_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{field} must be non-empty");
    }
    Ok(())
}

fn validate_sha256(field: &str, value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("{field} must be a 64-character lowercase hexadecimal SHA-256 digest");
    }
    Ok(())
}
