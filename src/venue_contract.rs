use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::lake_batch::supported_stream_classes;

const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Supported,
    Unsupported,
    Conditional,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Required,
    Optional,
    Disabled,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    #[default]
    Native,
    Derived,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StreamContract {
    pub capability: Capability,
    #[serde(default)]
    pub policy: Option<Policy>,
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub derived_from: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VenueContract {
    pub schema_version: u32,
    pub venue: String,
    pub adapter_version: String,
    pub streams: BTreeMap<String, StreamContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassReport {
    pub capability: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy: Option<String>,
    pub spool_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_converted: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_converted: Option<u64>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompletenessReport {
    pub schema_version: u32,
    pub venue: String,
    pub contract_version: u32,
    pub instance_id: String,
    pub outcome: String,
    pub classes: BTreeMap<String, ClassReport>,
}

impl VenueContract {
    pub fn load_and_validate(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read contract {}: {e}", path.display()))?;
        let contract: VenueContract = toml::from_str(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse contract {}: {e}", path.display()))?;
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported contract schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );

        for cls in supported_stream_classes() {
            ensure!(
                self.streams.contains_key(*cls),
                "contract missing required stream class: {cls}"
            );
        }

        for (name, stream) in &self.streams {
            ensure!(
                supported_stream_classes().contains(&name.as_str()),
                "adapter does not implement stream class: {name}"
            );
            match stream.capability {
                Capability::Unsupported => {
                    if let Some(ref policy) = stream.policy {
                        ensure!(
                            *policy == Policy::Disabled,
                            "stream {name}: unsupported capability cannot have \
                             policy {policy:?} (must be disabled or omitted)"
                        );
                    }
                }
                Capability::Supported | Capability::Conditional => {
                    if let Some(ref policy) = stream.policy {
                        ensure!(
                            *policy == Policy::Required
                                || *policy == Policy::Optional
                                || *policy == Policy::Disabled,
                            "stream {name}: supported capability has invalid \
                             policy {policy:?}"
                        );
                    }
                }
            }

            if stream.provenance == Provenance::Derived {
                let derived_from = stream.derived_from.as_ref();
                ensure!(
                    derived_from.is_some_and(|v| !v.is_empty()),
                    "stream {name}: derived provenance requires \
                     non-empty derived_from"
                );
                for source in derived_from.unwrap() {
                    let source_stream = self.streams.get(source);
                    ensure!(
                        source_stream.is_some_and(|s| s.capability == Capability::Supported),
                        "stream {name}: derived_from references {source} \
                         which is not supported"
                    );
                }
            }
        }

        Ok(())
    }

    pub fn effective_policy(&self, class: &str) -> Option<Policy> {
        self.streams.get(class).map(|s| {
            s.policy.clone().unwrap_or(match s.capability {
                Capability::Unsupported => Policy::Disabled,
                Capability::Supported | Capability::Conditional => Policy::Required,
            })
        })
    }
}

pub fn normalize_local_absolute_contract_path(path: &Path) -> Result<PathBuf> {
    let path_str = path.to_string_lossy();
    ensure!(
        !path_str.contains("://"),
        "contract_path must be a local absolute path, got `{}`",
        path.display()
    );
    ensure!(
        path.is_absolute(),
        "contract_path must be a local absolute path, got `{}`",
        path.display()
    );

    normalize_absolute_path(path)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return Ok(fs::canonicalize(path)?);
    }

    let mut tail = Vec::<OsString>::new();
    let mut cursor = path;
    while !cursor.exists() {
        let name = cursor.file_name().ok_or_else(|| {
            anyhow::anyhow!("unable to normalize path {}", path.display())
        })?;
        tail.push(name.to_os_string());
        cursor = cursor.parent().ok_or_else(|| {
            anyhow::anyhow!("unable to find existing ancestor for {}", path.display())
        })?;
    }

    let mut resolved = fs::canonicalize(cursor)?;
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}
