use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use serde_json::Value;

const PATH_FIELD_NAMES: &[&str] = &[
    "accepted_tranche_path",
    "execution_pack_path",
    "execution_plan_path",
    "object_gates_path",
    "operator_inputs_path",
    "path",
    "proof_path",
    "queue_path",
    "run_plan_path",
    "run_spec_path",
    "source_manifest_path",
];

#[test]
fn committed_source_universe_artifacts_record_portable_paths() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root exists");
    let artifact_paths = committed_source_universe_json_paths(&repo_root);
    assert!(
        !artifact_paths.is_empty(),
        "expected committed source-universe JSON artifacts"
    );

    let mut failures = Vec::new();
    for artifact_path in artifact_paths {
        let relative_artifact_path = artifact_path
            .strip_prefix(&repo_root)
            .expect("artifact path is under repo root");
        let json: Value = serde_json::from_slice(
            &fs::read(&artifact_path).expect("read committed source-universe artifact"),
        )
        .unwrap_or_else(|err| {
            panic!(
                "parse committed source-universe artifact {}: {err}",
                relative_artifact_path.display()
            )
        });
        collect_absolute_path_fields(
            &json,
            relative_artifact_path,
            &mut Vec::new(),
            &mut failures,
        );
    }

    assert!(
        failures.is_empty(),
        "committed source-universe artifacts must not record checkout-absolute paths:\n{}",
        failures.join("\n")
    );
}

fn committed_source_universe_json_paths(repo_root: &Path) -> Vec<PathBuf> {
    let specs_dir = Path::new("specs")
        .join("023-nt-research-analytics-platform")
        .join("reference");
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("ls-files")
        .arg(&specs_dir)
        .output();

    let mut paths = if let Ok(output) = output
        && output.status.success()
    {
        let stdout = String::from_utf8(output.stdout).expect("git ls-files output is UTF-8");
        stdout
            .lines()
            .filter(|path| is_source_universe_json_path(Path::new(path)))
            .map(|path| repo_root.join(path))
            .collect::<Vec<_>>()
    } else {
        collect_source_universe_json_paths(&repo_root.join(specs_dir))
    };
    paths.sort();
    paths
}

fn collect_source_universe_json_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut pending_dirs = vec![root.to_path_buf()];
    while let Some(dir) = pending_dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending_dirs.push(path);
            } else if is_source_universe_json_path(&path) {
                paths.push(path);
            }
        }
    }
    paths
}

fn is_source_universe_json_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "json")
        && path.components().any(|component| {
            component
                .as_os_str()
                .to_string_lossy()
                .contains("source-universe")
        })
}

fn collect_absolute_path_fields(
    value: &Value,
    artifact_path: &Path,
    field_path: &mut Vec<String>,
    failures: &mut Vec<String>,
) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                field_path.push(key.clone());
                collect_absolute_path_fields(value, artifact_path, field_path, failures);
                field_path.pop();
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                field_path.push(index.to_string());
                collect_absolute_path_fields(value, artifact_path, field_path, failures);
                field_path.pop();
            }
        }
        Value::String(path) if is_path_field(field_path) && is_nonportable_local_path(path) => {
            failures.push(format!(
                "{}:{} = {}",
                artifact_path.display(),
                field_path.join("."),
                path
            ));
        }
        _ => {}
    }
}

fn is_path_field(field_path: &[String]) -> bool {
    field_path
        .last()
        .is_some_and(|field| PATH_FIELD_NAMES.contains(&field.as_str()))
}

fn is_nonportable_local_path(path: &str) -> bool {
    is_checkout_absolute_path(path) || is_noncanonical_repo_relative_path(path)
}

fn is_checkout_absolute_path(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with('\\')
        || path.contains(".worktrees")
        || has_windows_drive_prefix(path)
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn is_noncanonical_repo_relative_path(path: &str) -> bool {
    if path.contains("://") || is_checkout_absolute_path(path) {
        return false;
    }
    let path = Path::new(path);
    matches!(
        path.components().next(),
        Some(Component::Normal(component))
            if matches!(
                component.to_str(),
                Some("specs" | "crates" | "docs" | "scripts")
            )
    ) && path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
}

#[test]
fn path_guard_rejects_parent_dir_in_repo_relative_paths() {
    assert!(is_nonportable_local_path(
        "specs/../specs/reference/artifact.json"
    ));
    assert!(is_nonportable_local_path(
        "crates/backtesting-vertical-slice/../artifact.json"
    ));
}

#[test]
fn path_guard_preserves_external_uri_paths_with_parent_segments() {
    assert!(!is_nonportable_local_path(
        "s3://bucket/source-universe=abc/../object.json"
    ));
    assert!(!is_nonportable_local_path(
        "https://example.invalid/source-universe/../object.json"
    ));
}
