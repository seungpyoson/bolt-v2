use std::{collections::BTreeSet, path::Path};

use anyhow::Result;

use super::{
    control::{
        CanaryEntry, MatchKind, RegistryFile, ReplayExpectedResult, ReplaySetFile, SafeListFile,
    },
    evidence::ChangedPathArtifact,
};

pub fn classify_changed_paths(
    changed_paths: &[String],
    registry: &RegistryFile,
    safe_list: &SafeListFile,
) -> Vec<ChangedPathArtifact> {
    let mut classifications = Vec::new();
    for path in changed_paths {
        let seams = registry
            .seams
            .iter()
            .filter(|seam| {
                seam.upstream_prefixes
                    .iter()
                    .any(|prefix| path_starts_with(path, prefix))
            })
            .map(|seam| seam.name.clone())
            .collect::<Vec<_>>();
        if !seams.is_empty() {
            let is_multi_seam = seams.len() > 1;
            classifications.push(ChangedPathArtifact {
                path: path.clone(),
                classification: if is_multi_seam {
                    "ambiguous".to_string()
                } else {
                    "seam".to_string()
                },
                seams,
                safe_list_entry: None,
                reason: if is_multi_seam {
                    Some("matched multiple seams".to_string())
                } else {
                    None
                },
            });
            continue;
        }

        let matching_safe_list = safe_list
            .entries
            .iter()
            .filter(|entry| safe_list_matches(path, entry.path.as_str(), entry.match_kind))
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        if matching_safe_list.len() == 1 {
            classifications.push(ChangedPathArtifact {
                path: path.clone(),
                classification: "safe_list".to_string(),
                seams: Vec::new(),
                safe_list_entry: matching_safe_list.into_iter().next(),
                reason: None,
            });
            continue;
        }

        classifications.push(ChangedPathArtifact {
            path: path.clone(),
            classification: "ambiguous".to_string(),
            seams: Vec::new(),
            safe_list_entry: None,
            reason: Some(if matching_safe_list.len() > 1 {
                "matched multiple safe-list entries".to_string()
            } else {
                "no seam or safe-list match".to_string()
            }),
        });
    }
    classifications
}

pub fn apply_replay_expectations(
    classifications: &mut [ChangedPathArtifact],
    replay_set: &ReplaySetFile,
) -> Vec<String> {
    let mut failures = Vec::new();
    for entry in &replay_set.entries {
        let changed_paths = entry.changed_paths.iter().collect::<BTreeSet<_>>();
        let overlaps = classifications
            .iter()
            .filter(|classification| changed_paths.contains(&classification.path))
            .count();
        if overlaps == 0 {
            continue;
        }
        match entry.expected_result {
            ReplayExpectedResult::Ambiguous => {
                for classification in classifications.iter_mut() {
                    if changed_paths.contains(&classification.path) {
                        classification.classification = "ambiguous".to_string();
                        classification.reason =
                            Some(format!("replay-set {} requires ambiguity", entry.id));
                    }
                }
                failures.push("ambiguous_upstream_path".to_string());
            }
            ReplayExpectedResult::Fail => failures.push("replay_fail".to_string()),
        }
    }
    failures.sort();
    failures.dedup();
    failures
}

pub fn required_seams(classifications: &[ChangedPathArtifact]) -> Vec<String> {
    let mut seams = BTreeSet::new();
    for classification in classifications {
        if classification.classification == "seam" {
            for seam in &classification.seams {
                seams.insert(seam.clone());
            }
        }
    }
    seams.into_iter().collect()
}

pub fn required_canaries(registry: &RegistryFile, seam_names: &[String]) -> Vec<CanaryEntry> {
    let seam_names = seam_names.iter().cloned().collect::<BTreeSet<_>>();
    let mut canaries = registry
        .seams
        .iter()
        .filter(|seam| seam_names.contains(&seam.name))
        .flat_map(|seam| seam.canaries.clone())
        .collect::<Vec<_>>();
    canaries.sort_by(|a, b| a.id.cmp(&b.id));
    canaries
}

pub fn registry_prefix_gaps(repo_root: &Path, registry: &RegistryFile) -> Result<Vec<String>> {
    let mut gaps = Vec::new();
    for seam in &registry.seams {
        for prefix in &seam.upstream_prefixes {
            if !repo_root.join(prefix).exists() {
                gaps.push(format!("{}::{}", seam.name, prefix));
            }
        }
    }
    gaps.sort();
    Ok(gaps)
}

fn path_starts_with(path: &str, prefix: &str) -> bool {
    let normalized_prefix = prefix.trim_end_matches('/');
    path == normalized_prefix
        || path.starts_with(prefix)
        || path.starts_with(&(normalized_prefix.to_string() + "/"))
}

fn safe_list_matches(path: &str, entry_path: &str, match_kind: MatchKind) -> bool {
    match match_kind {
        MatchKind::Exact => path == entry_path.trim_end_matches('/'),
        MatchKind::Prefix => path_starts_with(path, entry_path),
    }
}
