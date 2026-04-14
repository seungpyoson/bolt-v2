use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use super::control::RegistryFile;
use super::evidence::{InventoryEntryArtifact, RegistryGapArtifact};

#[derive(Debug, Clone)]
pub struct SourceInventory {
    pub production: Vec<InventoryEntryArtifact>,
    pub test_support: Vec<InventoryEntryArtifact>,
    pub registry_gaps: Vec<RegistryGapArtifact>,
}

pub fn collect_source_inventory(
    repo_root: &Path,
    registry: &RegistryFile,
) -> Result<SourceInventory> {
    let mut production = Vec::new();
    let mut test_support = Vec::new();
    let mut registry_gaps = Vec::new();
    let seam_by_path = seam_owners_by_path(registry);

    for root in ["src", "tests"] {
        let absolute = repo_root.join(root);
        if !absolute.exists() {
            continue;
        }
        let files = collect_candidate_files(&absolute)?;
        for file in files {
            let relative = file
                .strip_prefix(repo_root)
                .expect("candidate file should be under repo root")
                .to_string_lossy()
                .replace('\\', "/");
            let contents = fs::read_to_string(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            let referenced_crates = detect_nautilus_references(&contents);
            if referenced_crates.is_empty() {
                continue;
            }
            let owned_by_seams = seam_by_path
                .get(&relative)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect::<Vec<_>>();
            let entry = InventoryEntryArtifact {
                path: relative.clone(),
                referenced_crates: referenced_crates.iter().cloned().collect(),
                owned_by_seams: owned_by_seams.clone(),
            };
            if root == "src" {
                production.push(entry);
            } else {
                test_support.push(entry);
            }
            if owned_by_seams.is_empty() {
                registry_gaps.push(RegistryGapArtifact {
                    path: relative,
                    section: if root == "src" {
                        "production".to_string()
                    } else {
                        "test-support".to_string()
                    },
                    referenced_crates: referenced_crates.into_iter().collect(),
                });
            }
        }
    }

    production.sort_by(|a, b| a.path.cmp(&b.path));
    test_support.sort_by(|a, b| a.path.cmp(&b.path));
    registry_gaps.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(SourceInventory {
        production,
        test_support,
        registry_gaps,
    })
}

fn seam_owners_by_path(registry: &RegistryFile) -> BTreeMap<String, BTreeSet<String>> {
    let mut mapping = BTreeMap::<String, BTreeSet<String>>::new();
    for seam in &registry.seams {
        for path in &seam.bolt_usage {
            mapping
                .entry(path.clone())
                .or_default()
                .insert(seam.name.clone());
        }
    }
    mapping
}

fn collect_candidate_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)
        .with_context(|| format!("failed to read directory {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_candidate_files(&path)?);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    Ok(files)
}

fn detect_nautilus_references(contents: &str) -> BTreeSet<String> {
    let mut references = BTreeSet::new();
    for token in contents
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .filter(|token| !token.is_empty())
    {
        if token.starts_with("nautilus_") || token.starts_with("nautilus-") {
            references.insert(token.to_string());
        }
    }
    references
}
