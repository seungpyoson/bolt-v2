//! Machine-checkable NautilusTrader dependency proof.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

const WORKSPACE_CARGO_TOML: &str = include_str!("../Cargo.toml");
const WORKSPACE_CARGO_LOCK: &str = include_str!("../Cargo.lock");
const NT_GIT_URL: &str = "https://github.com/nautechsystems/nautilus_trader.git";
const REQUIRED_BACKTEST_FEATURES: [&str; 2] = ["examples", "streaming"];
const REQUIRED_PERSISTENCE_FEATURES: [&str; 1] = ["cloud"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtDependencyProof {
    pub nautilus_revision: String,
    pub nt_dependency_names: Vec<String>,
    pub nautilus_backtest_features: Vec<String>,
    pub nautilus_persistence_features: Vec<String>,
    pub lock_sources_all_resolve_to_revision: bool,
}

pub fn nt_dependency_proof_from_embedded_manifests()
-> Result<NtDependencyProof, NtDependencyProofError> {
    nt_dependency_proof_from_manifests(WORKSPACE_CARGO_TOML, WORKSPACE_CARGO_LOCK)
}

pub fn verified_nt_revision_from_embedded_manifests() -> Result<String, NtDependencyProofError> {
    verified_nt_revision_from_manifests(WORKSPACE_CARGO_TOML, WORKSPACE_CARGO_LOCK)
}

pub fn verified_nt_revision_from_manifests(
    cargo_toml: &str,
    cargo_lock: &str,
) -> Result<String, NtDependencyProofError> {
    let proof = nt_dependency_proof_from_manifests(cargo_toml, cargo_lock)?;
    if !proof.lock_sources_all_resolve_to_revision {
        return Err(NtDependencyProofError::LockRevisionMismatch {
            revision: proof.nautilus_revision,
        });
    }
    Ok(proof.nautilus_revision)
}

pub fn nt_dependency_proof_from_manifests(
    cargo_toml: &str,
    cargo_lock: &str,
) -> Result<NtDependencyProof, NtDependencyProofError> {
    let manifest = cargo_toml
        .parse::<toml::Table>()
        .map_err(|source| NtDependencyProofError::ParseToml(source.to_string()))?;
    let dependencies = manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or(NtDependencyProofError::MissingDependenciesTable)?;
    let mut nt_dependency_names = Vec::new();
    let mut nautilus_revision: Option<String> = None;
    for (name, value) in dependencies {
        if !name.starts_with("nautilus-") {
            continue;
        }
        let dependency = value
            .as_table()
            .ok_or_else(|| NtDependencyProofError::InvalidDependencyShape { name: name.clone() })?;
        let git = dependency
            .get("git")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| NtDependencyProofError::MissingGit { name: name.clone() })?;
        if git != NT_GIT_URL {
            return Err(NtDependencyProofError::UnexpectedGit {
                name: name.clone(),
                git: git.to_string(),
            });
        }
        let rev = dependency
            .get("rev")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| NtDependencyProofError::MissingRev { name: name.clone() })?;
        validate_git_sha(rev)?;
        if let Some(existing) = &nautilus_revision {
            if existing != rev {
                return Err(NtDependencyProofError::RevisionMismatch {
                    expected: existing.clone(),
                    actual: rev.to_string(),
                    name: name.clone(),
                });
            }
        } else {
            nautilus_revision = Some(rev.to_string());
        }
        nt_dependency_names.push(name.clone());
    }
    nt_dependency_names.sort();
    let nautilus_revision = nautilus_revision.ok_or(NtDependencyProofError::MissingNtDependency)?;
    let nautilus_backtest_features = sorted_features(dependencies, "nautilus-backtest")?;
    let nautilus_persistence_features = sorted_features(dependencies, "nautilus-persistence")?;
    require_features(
        "nautilus-backtest",
        &nautilus_backtest_features,
        &REQUIRED_BACKTEST_FEATURES,
    )?;
    require_features(
        "nautilus-persistence",
        &nautilus_persistence_features,
        &REQUIRED_PERSISTENCE_FEATURES,
    )?;
    Ok(NtDependencyProof {
        lock_sources_all_resolve_to_revision: lock_sources_resolve_to_revision(
            cargo_lock,
            &nautilus_revision,
        )?,
        nautilus_revision,
        nt_dependency_names,
        nautilus_backtest_features,
        nautilus_persistence_features,
    })
}

fn sorted_features(
    dependencies: &toml::map::Map<String, toml::Value>,
    name: &str,
) -> Result<Vec<String>, NtDependencyProofError> {
    let dependency = dependencies
        .get(name)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| NtDependencyProofError::MissingDependency {
            name: name.to_string(),
        })?;
    let mut features = dependency
        .get("features")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| NtDependencyProofError::MissingFeatures {
            name: name.to_string(),
        })?
        .iter()
        .map(|value| {
            value.as_str().map(ToString::to_string).ok_or_else(|| {
                NtDependencyProofError::InvalidFeature {
                    name: name.to_string(),
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    features.sort();
    Ok(features)
}

fn require_features(
    name: &str,
    actual: &[String],
    required: &[&str],
) -> Result<(), NtDependencyProofError> {
    for feature in required {
        if !actual.iter().any(|actual| actual == feature) {
            return Err(NtDependencyProofError::MissingRequiredFeature {
                name: name.to_string(),
                feature: (*feature).to_string(),
            });
        }
    }
    Ok(())
}

fn lock_sources_resolve_to_revision(
    cargo_lock: &str,
    revision: &str,
) -> Result<bool, NtDependencyProofError> {
    let lock = cargo_lock
        .parse::<toml::Table>()
        .map_err(|source| NtDependencyProofError::ParseLock(source.to_string()))?;
    let packages = lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or(NtDependencyProofError::MissingLockPackages)?;
    let mut saw_nt_package = false;
    for package in packages {
        let Some(package) = package.as_table() else {
            continue;
        };
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        if !name.starts_with("nautilus-") {
            continue;
        }
        let Some(source) = package.get("source").and_then(toml::Value::as_str) else {
            return Ok(false);
        };
        saw_nt_package = true;
        let expected_source = format!("git+{NT_GIT_URL}?rev={revision}#{revision}");
        if source != expected_source {
            return Ok(false);
        }
    }
    Ok(saw_nt_package)
}

fn validate_git_sha(rev: &str) -> Result<(), NtDependencyProofError> {
    if rev.len() == 40 && rev.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(NtDependencyProofError::InvalidRevision {
            rev: rev.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NtDependencyProofError {
    ParseToml(String),
    ParseLock(String),
    MissingDependenciesTable,
    MissingLockPackages,
    MissingNtDependency,
    MissingDependency {
        name: String,
    },
    InvalidDependencyShape {
        name: String,
    },
    MissingGit {
        name: String,
    },
    UnexpectedGit {
        name: String,
        git: String,
    },
    MissingRev {
        name: String,
    },
    InvalidRevision {
        rev: String,
    },
    RevisionMismatch {
        expected: String,
        actual: String,
        name: String,
    },
    MissingFeatures {
        name: String,
    },
    InvalidFeature {
        name: String,
    },
    MissingRequiredFeature {
        name: String,
        feature: String,
    },
    LockRevisionMismatch {
        revision: String,
    },
}

impl fmt::Display for NtDependencyProofError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseToml(source) => write!(f, "parse Cargo.toml: {source}"),
            Self::ParseLock(source) => write!(f, "parse Cargo.lock: {source}"),
            Self::MissingDependenciesTable => write!(f, "Cargo.toml is missing dependencies table"),
            Self::MissingLockPackages => write!(f, "Cargo.lock is missing package entries"),
            Self::MissingNtDependency => write!(f, "Cargo.toml has no nautilus-* dependencies"),
            Self::MissingDependency { name } => write!(f, "Cargo.toml is missing {name}"),
            Self::InvalidDependencyShape { name } => {
                write!(f, "Cargo.toml dependency {name} must be a table")
            }
            Self::MissingGit { name } => write!(f, "Cargo.toml dependency {name} is missing git"),
            Self::UnexpectedGit { name, git } => {
                write!(f, "Cargo.toml dependency {name} uses unexpected git {git}")
            }
            Self::MissingRev { name } => write!(f, "Cargo.toml dependency {name} is missing rev"),
            Self::InvalidRevision { rev } => write!(f, "invalid NautilusTrader git revision {rev}"),
            Self::RevisionMismatch {
                expected,
                actual,
                name,
            } => write!(
                f,
                "Cargo.toml dependency {name} uses rev {actual}, expected {expected}"
            ),
            Self::MissingFeatures { name } => {
                write!(f, "Cargo.toml dependency {name} is missing features")
            }
            Self::InvalidFeature { name } => {
                write!(f, "Cargo.toml dependency {name} has a non-string feature")
            }
            Self::MissingRequiredFeature { name, feature } => {
                write!(
                    f,
                    "Cargo.toml dependency {name} is missing required feature {feature}"
                )
            }
            Self::LockRevisionMismatch { revision } => write!(
                f,
                "Cargo.lock does not resolve NautilusTrader sources to declared revision {revision}"
            ),
        }
    }
}

impl Error for NtDependencyProofError {}

#[cfg(test)]
mod tests {
    use super::*;

    const REVISION: &str = "8160730c7c550480b0a439fb11086a4c4de15f0b";

    fn manifest(git: &str) -> String {
        format!(
            r#"[dependencies]
nautilus-backtest = {{ git = "{git}", rev = "{REVISION}", features = ["streaming", "examples"] }}
nautilus-persistence = {{ git = "{git}", rev = "{REVISION}", features = ["cloud"] }}
"#
        )
    }

    fn lock(source: &str) -> String {
        format!(
            r#"version = 4

[[package]]
name = "nautilus-backtest"
version = "0.60.0"
source = "{source}"

[[package]]
name = "nautilus-persistence"
version = "0.60.0"
source = "{source}"
"#
        )
    }

    #[test]
    fn official_exact_source_and_revision_produce_dependency_proof() {
        let source = format!("git+{NT_GIT_URL}?rev={REVISION}#{REVISION}");
        let proof = nt_dependency_proof_from_manifests(&manifest(NT_GIT_URL), &lock(&source))
            .expect("official exact source should verify");
        assert!(proof.lock_sources_all_resolve_to_revision);
        assert_eq!(
            proof.nautilus_backtest_features,
            vec!["examples".to_string(), "streaming".to_string()]
        );
        assert_eq!(
            proof.nautilus_persistence_features,
            vec!["cloud".to_string()]
        );
    }

    #[test]
    fn personal_fork_manifest_is_rejected() {
        let error = nt_dependency_proof_from_manifests(
            &manifest("https://github.com/seungpyoson/nautilus_trader.git"),
            &lock("unused"),
        )
        .expect_err("personal fork must not produce dependency proof");
        assert!(matches!(
            error,
            NtDependencyProofError::UnexpectedGit { .. }
        ));
    }

    #[test]
    fn noncanonical_lock_source_is_not_exact_revision_proof() {
        let source = format!("git+{NT_GIT_URL}?rev={REVISION}#{REVISION}-suffix");
        let proof = nt_dependency_proof_from_manifests(&manifest(NT_GIT_URL), &lock(&source))
            .expect("well-formed manifest should still return a negative lock proof");
        assert!(!proof.lock_sources_all_resolve_to_revision);
    }
}
