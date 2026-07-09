use std::{
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    backfill_conversion_batch::{
        BackfillConversionBatchPlan, write_backfill_conversion_batch_plan_from_spec_file,
    },
    hashing::sha256_hex,
    reference_fixture_index::{EvictedFixtureIndex, repo_root_from_manifest_dir},
};

pub fn tempdir_in_repo_target() -> tempfile::TempDir {
    let target_dir = repo_root_from_manifest_dir().join("target");
    fs::create_dir_all(&target_dir).unwrap_or_else(|error| {
        panic!("create target temp root {}: {error}", target_dir.display())
    });
    tempfile::tempdir_in(&target_dir)
        .unwrap_or_else(|error| panic!("create temp dir in {}: {error}", target_dir.display()))
}

pub fn rewrite_assignment(source: &str, key: &str, value: &Path) -> String {
    let prefix = format!("{key} = ");
    let replacement = format!("{key} = \"{}\"", value.display());
    let mut replacement_count = 0usize;
    let rewritten = source
        .lines()
        .map(|line| {
            if line.trim_start().starts_with(&prefix) {
                replacement_count += 1;
                replacement.as_str()
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    assert_eq!(
        replacement_count, 1,
        "expected exactly one assignment for {key:?}, found {replacement_count}"
    );
    rewritten
}

pub fn assert_generated_fixture_matches_index(repo_relative_path: &str, generated_path: &Path) {
    let index =
        EvictedFixtureIndex::load(&repo_root_from_manifest_dir()).expect("load eviction index");
    let entry = index
        .entry_for(repo_relative_path)
        .unwrap_or_else(|| panic!("eviction index must contain {repo_relative_path}"));
    let bytes = fs::read(generated_path).unwrap_or_else(|error| {
        panic!(
            "read generated fixture {}: {error}",
            generated_path.display()
        )
    });
    assert_eq!(
        bytes.len() as u64,
        entry.bytes,
        "generated fixture byte length must match eviction index for {repo_relative_path}"
    );
    assert_eq!(
        sha256_hex(&bytes),
        entry.sha256,
        "generated fixture sha256 must match eviction index for {repo_relative_path}"
    );
}

pub fn generate_evicted_batch_plan(
    batch_root: &Path,
    repo_relative_path: &str,
    temp_dir: &Path,
) -> PathBuf {
    let spec_path = batch_root.join("backfill-conversion-batch-plan.toml");
    let temp_spec_path = temp_dir.join("backfill-conversion-batch-plan.toml");
    let temp_output_dir = temp_dir.join("plan");
    let spec = fs::read_to_string(&spec_path)
        .unwrap_or_else(|error| panic!("read batch spec {}: {error}", spec_path.display()));
    fs::write(
        &temp_spec_path,
        rewrite_assignment(&spec, "output_dir", &temp_output_dir),
    )
    .expect("write temp batch spec");
    let artifact = write_backfill_conversion_batch_plan_from_spec_file(&temp_spec_path)
        .expect("batch plan generation succeeds");
    assert_generated_fixture_matches_index(repo_relative_path, &artifact.path);
    artifact.path
}

pub fn generated_evicted_conversion_batch_plan(
    reference_root: &Path,
    scope: &str,
    repo_relative_path: &str,
) -> BackfillConversionBatchPlan {
    let temp_dir = tempdir_in_repo_target();
    let batch_root = reference_root.join(format!("backfill-conversion-batches/{scope}"));
    let artifact_path = generate_evicted_batch_plan(&batch_root, repo_relative_path, temp_dir.path());
    let bytes = fs::read(&artifact_path)
        .unwrap_or_else(|error| panic!("read generated fixture {}: {error}", artifact_path.display()));
    serde_json::from_slice(&bytes).expect("conversion batch plan parses")
}
