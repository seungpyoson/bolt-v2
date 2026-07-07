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

const DATED_SOURCE_ATTESTATION_OWNERS: &[&str] = &[
    "specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json",
];

/// Gate policy: committed reference JSON can contain both live artifact pins and
/// historical source attestations. Reference-tree targets remain live pins
/// checked against current bytes or the evicted-fixture index. Source-tree
/// targets inside registered dated source attestations are delegated to the
/// dedicated verifier for that attestation, so this matcher avoids duplicate
/// enforcement instead of treating stale source pins as valid.
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
                &artifact_ref,
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
            collect_flat_sibling_hash_refs(object, location, artifact_refs);
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
    let role = string_field(object, "role").map(str::to_string);
    let path = string_field(object, "path")?;
    if role.is_none() && normalize_repo_path(path).is_err() {
        return None;
    }

    Some(ArtifactRefCandidate {
        role: role.unwrap_or_else(|| inferred_role_from_location(location)),
        path: path.to_string(),
        sha256: string_field(object, "sha256")?.to_string(),
        location: location.to_string(),
    })
}

fn collect_flat_sibling_hash_refs(
    object: &Map<String, Value>,
    location: &str,
    artifact_refs: &mut Vec<ArtifactRefCandidate>,
) {
    for (key, path) in object {
        let Some(role) = key.strip_suffix("_path") else {
            continue;
        };
        if role.is_empty() {
            continue;
        }
        let Some(path) = path.as_str() else {
            continue;
        };
        if normalize_repo_path(path).is_err() {
            continue;
        }
        let sha256_key = format!("{role}_sha256");
        let hash_key = format!("{role}_hash");
        let Some((hash_key, sha256)) = string_field(object, &sha256_key)
            .map(|sha256| (sha256_key.as_str(), sha256))
            .or_else(|| string_field(object, &hash_key).map(|sha256| (hash_key.as_str(), sha256)))
        else {
            continue;
        };
        artifact_refs.push(ArtifactRefCandidate {
            role: role.to_string(),
            path: path.to_string(),
            sha256: sha256.to_string(),
            location: format!("{location}.{hash_key}"),
        });
    }
}

fn inferred_role_from_location(location: &str) -> String {
    location
        .strip_prefix("$.")
        .and_then(|location| location.strip_prefix("committed_input_hashes."))
        .map(|role| format!("committed_input_hashes.{role}"))
        .unwrap_or_else(|| {
            location
                .rsplit_once('.')
                .map(|(_, role)| role)
                .unwrap_or(location)
                .to_string()
        })
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key)?.as_str()
}

#[test]
fn artifact_ref_collector_treats_named_path_sha256_objects_as_pins() {
    let json = serde_json::json!({
        "source_proof_admissibility_status": {
            "admissibility_contract": {
                "path": "repo://specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json",
                "sha256": "d1fb1504af910203751eb15ba22300b241d4272e48dbfd27de432e737b5cc59d"
            }
        }
    });
    let mut artifact_refs = Vec::new();

    collect_artifact_refs(&json, "$", &mut artifact_refs);

    assert!(
        artifact_refs.iter().any(|candidate| {
            candidate.role == "admissibility_contract"
                && candidate.location
                    == "$.source_proof_admissibility_status.admissibility_contract"
                && candidate.path
                    == "repo://specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
                && candidate.sha256
                    == "d1fb1504af910203751eb15ba22300b241d4272e48dbfd27de432e737b5cc59d"
        }),
        "collector should include named-key path/sha256 pins: {artifact_refs:?}"
    );
}

#[test]
fn dated_status_source_path_pins_are_point_in_time_attestations() {
    let dated_status = Path::new(
        "specs/023-nt-research-analytics-platform/reference/source-proof-pmxt-durable-source-selection-status.2026-06-16.json",
    );

    assert!(!should_enforce_artifact_ref(
        dated_status,
        "crates/backtesting-vertical-slice/src/source_proof.rs"
    ));
    assert!(!should_enforce_artifact_ref(
        dated_status,
        "src/source_proof.rs"
    ));
    assert!(!should_enforce_artifact_ref(
        dated_status,
        "scripts/verify_source_proof.py"
    ));
    assert!(should_enforce_artifact_ref(
        dated_status,
        "specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
    ));
    assert!(should_enforce_artifact_ref(
        Path::new(
            "specs/023-nt-research-analytics-platform/reference/source-proof-policy-hardcode-audit.2026-06-09.json",
        ),
        "crates/backtesting-vertical-slice/src/source_proof.rs"
    ));
}

#[test]
fn dated_source_attestation_exceptions_are_registered_by_owner_path() {
    let renamed_date_like_status = Path::new(
        "specs/023-nt-research-analytics-platform/reference/renamed-status.2026-06-16.json",
    );

    assert!(should_enforce_artifact_ref(
        renamed_date_like_status,
        "crates/backtesting-vertical-slice/src/source_proof.rs"
    ));
}

#[test]
fn repo_path_normalization_removes_current_directory_prefixes() {
    assert_eq!(
        normalize_repo_path("./specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json")
            .expect("normalize ./ path"),
        "specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
    );
    assert_eq!(
        normalize_repo_path("repo://./specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json")
            .expect("normalize repo://./ path"),
        "specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
    );
}

#[test]
fn repo_path_normalization_rejects_backslash_parent_traversal() {
    assert!(normalize_repo_path(r"..\outside.json").is_err());
    assert!(normalize_repo_path(r"repo://..\outside.json").is_err());
    assert!(normalize_repo_path(r"repo://specs\..\outside.json").is_err());
    assert_eq!(
        normalize_repo_path(r"repo://specs\023-nt-research-analytics-platform\reference\source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json")
            .expect("normalize backslash path separators"),
        "specs/023-nt-research-analytics-platform/reference/source-proof-fixture.binary-option.polymarket-pmxt-official-free-pending.v1.json"
    );
}

#[test]
fn artifact_ref_collector_treats_flat_path_hash_siblings_as_pins() {
    let json = serde_json::json!({
        "object_gates_path": "specs/023-nt-research-analytics-platform/reference/source-universe-object-gates/binance-data-vision-trades-2026-03-01-all-instruments/gates/source-universe-object-gates.json",
        "object_gates_hash": "8e5f68118c05e12f5f533305694fdd071140748b5aeb7db6afaa67937e2c148e",
        "conversion_queue_path": "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/binance-data-vision-trades-2026-03-01-all-instruments/queue/source-universe-conversion-queue.json",
        "conversion_queue_sha256": "3c6b000f781712930c90189a2131f419cf2822d3ec06083fdf06c53c67c59d77"
    });
    let mut artifact_refs = Vec::new();

    collect_artifact_refs(&json, "$", &mut artifact_refs);

    assert!(
        artifact_refs.iter().any(|candidate| {
            candidate.role == "object_gates"
                && candidate.location == "$.object_gates_hash"
                && candidate.path
                    == "specs/023-nt-research-analytics-platform/reference/source-universe-object-gates/binance-data-vision-trades-2026-03-01-all-instruments/gates/source-universe-object-gates.json"
                && candidate.sha256
                    == "8e5f68118c05e12f5f533305694fdd071140748b5aeb7db6afaa67937e2c148e"
        }),
        "collector should include flat sibling path/hash pins: {artifact_refs:?}"
    );
    assert!(
        artifact_refs.iter().any(|candidate| {
            candidate.role == "conversion_queue"
                && candidate.location == "$.conversion_queue_sha256"
                && candidate.path
                    == "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-queues/binance-data-vision-trades-2026-03-01-all-instruments/queue/source-universe-conversion-queue.json"
                && candidate.sha256
                    == "3c6b000f781712930c90189a2131f419cf2822d3ec06083fdf06c53c67c59d77"
        }),
        "collector should include flat sibling path/sha256 pins: {artifact_refs:?}"
    );
}

fn check_artifact_ref(
    repo_root: &Path,
    evicted_index: &EvictedFixtureIndex,
    owner_path: &Path,
    artifact_ref: &ArtifactRefCandidate,
    mismatches: &mut Vec<String>,
) {
    let artifact_path = match normalize_repo_path(&artifact_ref.path) {
        Ok(path) => path,
        Err(err) => {
            mismatches.push(format!(
                "{} {} role {} path {} recorded {} actual <invalid path: {err}>",
                owner_path
                    .strip_prefix(repo_root)
                    .unwrap_or(owner_path)
                    .display(),
                artifact_ref.location,
                artifact_ref.role,
                artifact_ref.path,
                artifact_ref.sha256
            ));
            return;
        }
    };

    if !should_enforce_artifact_ref(owner_path, &artifact_path) {
        return;
    }

    let actual_sha256 = actual_sha256(repo_root, evicted_index, &artifact_path);

    if artifact_ref.sha256 != actual_sha256 {
        mismatches.push(format!(
            "{} {} role {} path {artifact_path} recorded {} actual {actual_sha256}",
            owner_path
                .strip_prefix(repo_root)
                .unwrap_or(owner_path)
                .display(),
            artifact_ref.location,
            artifact_ref.role,
            artifact_ref.sha256
        ));
    }
}

fn should_enforce_artifact_ref(owner_path: &Path, artifact_path: &str) -> bool {
    !(is_registered_dated_source_attestation(owner_path) && is_source_tree_path(artifact_path))
}

fn is_registered_dated_source_attestation(path: &Path) -> bool {
    DATED_SOURCE_ATTESTATION_OWNERS.iter().any(|owner| {
        is_dated_status_file(Path::new(owner)) && path_matches_repo_suffix(path, owner)
    })
}

fn path_matches_repo_suffix(path: &Path, repo_suffix: &str) -> bool {
    let path = path.to_string_lossy().replace('\\', "/");
    path == repo_suffix || path.ends_with(&format!("/{repo_suffix}"))
}

fn is_dated_status_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let Some(date_start) = file_name
        .strip_suffix(".json")
        .and_then(|name| name.rfind("-status."))
    else {
        return false;
    };
    let date = &file_name[date_start + "-status.".len()..file_name.len() - ".json".len()];
    date.len() == "YYYY-MM-DD".len()
        && date.chars().enumerate().all(|(index, character)| {
            if matches!(index, 4 | 7) {
                character == '-'
            } else {
                character.is_ascii_digit()
            }
        })
}

fn is_source_tree_path(path: &str) -> bool {
    path.starts_with("crates/") || path.starts_with("src/") || path.starts_with("scripts/")
}

fn normalize_repo_path(path: &str) -> Result<String, String> {
    let path = path.strip_prefix("repo://").unwrap_or(path);
    let path = path.replace('\\', "/");
    let repo_path = Path::new(&path);

    if path.is_empty() {
        return Err("path is empty".to_string());
    }
    if path.contains("://") {
        return Err("path is not repo-relative".to_string());
    }
    if repo_path.is_absolute() {
        return Err("path is absolute".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in repo_path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err("path escapes repository root".to_string());
            }
        }
    }
    let normalized = normalized.to_string_lossy().replace('\\', "/");
    if normalized.is_empty() {
        return Err("path is empty".to_string());
    }

    Ok(normalized)
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
