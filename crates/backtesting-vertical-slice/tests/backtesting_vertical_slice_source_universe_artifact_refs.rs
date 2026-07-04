use std::{
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, repo_root_from_manifest_dir,
};
use serde_json::{Map, Value};

#[derive(Debug)]
struct ArtifactRefCandidate {
    role: String,
    path: String,
    sha256: String,
    location: String,
}

#[test]
fn committed_source_universe_artifact_refs_match_current_reference_bytes() {
    let repo_root = repo_root_from_manifest_dir();
    let reference_root = repo_root.join("specs/023-nt-research-analytics-platform/reference");
    let evicted_index = EvictedFixtureIndex::load(&repo_root).expect("load evicted fixture index");

    let mut mismatches = Vec::new();

    for json_path in committed_reference_json_files(&reference_root) {
        if json_path.file_name().and_then(|name| name.to_str())
            == Some("evicted-fixtures.index.json")
        {
            continue;
        }

        let json = read_json_value(&json_path);
        let mut artifact_refs = Vec::new();
        collect_artifact_refs(&json, "$", &mut artifact_refs);

        for artifact_ref in artifact_refs {
            check_artifact_ref(
                &repo_root,
                &evicted_index,
                &json_path,
                &artifact_ref.location,
                &artifact_ref.role,
                &artifact_ref.path,
                &artifact_ref.sha256,
                &mut mismatches,
            );
        }
    }

    assert!(
        mismatches.is_empty(),
        "committed reference JSON sha pins must match current fixture bytes:\n{}",
        mismatches.join("\n")
    );
}

fn collect_artifact_refs(
    value: &Value,
    location: &str,
    artifact_refs: &mut Vec<ArtifactRefCandidate>,
) {
    match value {
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                collect_artifact_refs(item, &format!("{location}[{index}]"), artifact_refs);
            }
        }
        Value::Object(object) => {
            if let Some(candidate) = artifact_ref_from_object(object, location) {
                artifact_refs.push(candidate);
            }
            if let Some(committed_input_hashes) = object.get("committed_input_hashes") {
                collect_committed_input_hash_refs(
                    committed_input_hashes,
                    &format!("{location}.committed_input_hashes"),
                    artifact_refs,
                );
            }
            for (key, child) in object {
                collect_artifact_refs(child, &format!("{location}.{key}"), artifact_refs);
            }
        }
        _ => {}
    }
}

fn artifact_ref_from_object(
    object: &Map<String, Value>,
    location: &str,
) -> Option<ArtifactRefCandidate> {
    Some(ArtifactRefCandidate {
        role: string_field(object, "role")?.to_string(),
        path: string_field(object, "path")?.to_string(),
        sha256: string_field(object, "sha256")?.to_string(),
        location: location.to_string(),
    })
}

fn collect_committed_input_hash_refs(
    value: &Value,
    location: &str,
    artifact_refs: &mut Vec<ArtifactRefCandidate>,
) {
    let Some(object) = value.as_object() else {
        return;
    };

    for (role, input_hash) in object {
        let Some(input_hash) = input_hash.as_object() else {
            continue;
        };
        let (Some(path), Some(sha256)) = (
            string_field(input_hash, "path"),
            string_field(input_hash, "sha256"),
        ) else {
            continue;
        };
        artifact_refs.push(ArtifactRefCandidate {
            role: format!("committed_input_hashes.{role}"),
            path: path.to_string(),
            sha256: sha256.to_string(),
            location: format!("{location}.{role}"),
        });
    }
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

fn check_artifact_ref(
    repo_root: &Path,
    evicted_index: &EvictedFixtureIndex,
    owner_path: &Path,
    location: &str,
    role: &str,
    artifact_path: &str,
    recorded_sha256: &str,
    mismatches: &mut Vec<String>,
) {
    let artifact_path = match normalize_repo_path(artifact_path) {
        Ok(path) => path,
        Err(err) => {
            mismatches.push(format!(
                "{} {location} role {role} path {artifact_path} recorded {recorded_sha256} actual <invalid path: {err}>",
                owner_path
                    .strip_prefix(repo_root)
                    .unwrap_or(owner_path)
                    .display()
            ));
            return;
        }
    };

    let actual_sha256 = actual_sha256(repo_root, evicted_index, &artifact_path);

    if recorded_sha256 != actual_sha256 {
        mismatches.push(format!(
            "{} {location} role {role} path {artifact_path} recorded {recorded_sha256} actual {actual_sha256}",
            owner_path
                .strip_prefix(repo_root)
                .unwrap_or(owner_path)
                .display()
        ));
    }
}

fn normalize_repo_path(path: &str) -> Result<String, String> {
    let path = path.strip_prefix("repo://").unwrap_or(path);
    let repo_path = Path::new(path);

    if path.is_empty() {
        return Err("path is empty".to_string());
    }
    if path.contains("://") {
        return Err("path is not repo-relative".to_string());
    }
    if repo_path.is_absolute() {
        return Err("path is absolute".to_string());
    }
    if repo_path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::RootDir
        )
    }) {
        return Err("path escapes repository root".to_string());
    }

    Ok(path.to_string())
}

fn actual_sha256(
    repo_root: &Path,
    evicted_index: &EvictedFixtureIndex,
    artifact_path: &str,
) -> String {
    if let Some(indexed) = evicted_index.sha256_for(artifact_path) {
        indexed.to_string()
    } else {
        sha256_file(&repo_root.join(artifact_path))
            .unwrap_or_else(|err| format!("<unreadable: {err}>"))
    }
}

fn committed_reference_json_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_json_files(root, &mut files);
    files.sort();
    files
}

fn collect_json_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| {
        panic!(
            "read directory {} while looking for reference JSON files: {err}",
            dir.display()
        )
    }) {
        let entry = entry.expect("directory entry is readable");
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, files);
        } else if path.extension().and_then(|name| name.to_str()) == Some("json") {
            files.push(path);
        }
    }
}

fn read_json_value(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|err| panic!("read JSON {}: {err}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|err| panic!("parse JSON {}: {err}", path.display()))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};

    let bytes = fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
