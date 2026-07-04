use std::{
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, repo_root_from_manifest_dir,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ArtifactRef {
    role: String,
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Deserialize)]
struct ExecutionPackRefs {
    artifact_refs: Vec<ArtifactRef>,
}

#[derive(Debug, Deserialize)]
struct AcceptanceLedgerRefs {
    records: Vec<AcceptanceLedgerRecordRefs>,
}

#[derive(Debug, Deserialize)]
struct AcceptanceLedgerRecordRefs {
    universe_id: String,
    artifact_refs: Vec<ArtifactRef>,
}

#[test]
fn committed_source_universe_artifact_refs_match_current_reference_bytes() {
    let repo_root = repo_root_from_manifest_dir();
    let reference_root = repo_root.join("specs/023-nt-research-analytics-platform/reference");
    let evicted_index = EvictedFixtureIndex::load(&repo_root).expect("load evicted fixture index");

    let mut mismatches = Vec::new();

    for pack_path in committed_files_named(&reference_root, "source-universe-execution-pack.json") {
        let pack: ExecutionPackRefs = read_json(&pack_path);
        for artifact_ref in pack.artifact_refs {
            check_artifact_ref(
                &repo_root,
                &evicted_index,
                &pack_path,
                &artifact_ref.role,
                &artifact_ref.path,
                &artifact_ref.sha256,
                &mut mismatches,
            );
        }
    }

    for ledger_path in committed_files_named(
        &reference_root,
        "source-universe-execution-acceptance-ledger.json",
    ) {
        let ledger: AcceptanceLedgerRefs = read_json(&ledger_path);
        for record in ledger.records {
            for artifact_ref in record.artifact_refs {
                let role = format!("{}:{}", record.universe_id, artifact_ref.role);
                check_artifact_ref(
                    &repo_root,
                    &evicted_index,
                    &ledger_path,
                    &role,
                    &artifact_ref.path,
                    &artifact_ref.sha256,
                    &mut mismatches,
                );
            }
        }
    }

    assert!(
        mismatches.is_empty(),
        "committed source-universe artifact_refs must match current fixture bytes:\n{}",
        mismatches.join("\n")
    );
}

fn check_artifact_ref(
    repo_root: &Path,
    evicted_index: &EvictedFixtureIndex,
    owner_path: &Path,
    role: &str,
    artifact_path: &Path,
    recorded_sha256: &str,
    mismatches: &mut Vec<String>,
) {
    let artifact_path = artifact_path
        .to_str()
        .expect("artifact ref path must be UTF-8");
    let actual_sha256 = if let Some(indexed) = evicted_index
        .entries
        .iter()
        .find(|entry| entry.path == artifact_path)
    {
        indexed.sha256.clone()
    } else {
        sha256_file(&repo_root.join(artifact_path))
    };

    if recorded_sha256 != actual_sha256 {
        mismatches.push(format!(
            "{} role {role} path {artifact_path} recorded {recorded_sha256} actual {actual_sha256}",
            owner_path
                .strip_prefix(repo_root)
                .unwrap_or(owner_path)
                .display()
        ));
    }
}

fn committed_files_named(root: &Path, file_name: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_files_named(root, file_name, &mut files);
    files.sort();
    files
}

fn collect_files_named(dir: &Path, file_name: &str, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| {
        panic!(
            "read directory {} while looking for {file_name}: {err}",
            dir.display()
        )
    }) {
        let entry = entry.expect("directory entry is readable");
        let path = entry.path();
        if path.is_dir() {
            collect_files_named(&path, file_name, files);
        } else if path.file_name().and_then(|name| name.to_str()) == Some(file_name) {
            files.push(path);
        }
    }
}

fn read_json<T>(path: &Path) -> T
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).unwrap_or_else(|err| panic!("read JSON {}: {err}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|err| panic!("parse JSON {}: {err}", path.display()))
}

fn sha256_file(path: &Path) -> String {
    use sha2::{Digest, Sha256};

    format!(
        "{:x}",
        Sha256::digest(
            fs::read(path)
                .unwrap_or_else(|err| panic!("read file for hash {}: {err}", path.display()))
        )
    )
}
