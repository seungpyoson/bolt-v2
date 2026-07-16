use std::{
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    hashing::sha256_hex,
    reference_fixture_index::{EvictedFixtureIndex, repo_root_from_manifest_dir},
    source_archive_index_source_universe::write_source_archive_index_source_universe_manifest_from_spec_file,
};

pub const PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json";
pub const PMXT_CATEGORY_OBJECT_MANIFEST_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json";
pub const PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_REGEN_PATH: &str = "target/reference-regen/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json";
pub const PMXT_OBJECT_MANIFEST_EVICTED_REFERENCE_PATHS: &[&str] = &[
    PMXT_CATEGORY_OBJECT_MANIFEST_PATH,
    PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH,
];

pub fn tempdir_in_repo_target() -> tempfile::TempDir {
    let target_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
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
    let bytes = fs::read(generated_path).unwrap_or_else(|error| {
        panic!(
            "read generated fixture {}: {error}",
            generated_path.display()
        )
    });
    assert_generated_fixture_bytes_match_index(repo_relative_path, &bytes);
}

pub fn assert_generated_fixture_bytes_match_index(repo_relative_path: &str, bytes: &[u8]) {
    let index =
        EvictedFixtureIndex::load(&repo_root_from_manifest_dir()).expect("load eviction index");
    let entry = index
        .entry_for(repo_relative_path)
        .unwrap_or_else(|| panic!("eviction index must contain {repo_relative_path}"));
    assert_eq!(
        (bytes.len() as u64, sha256_hex(&bytes)),
        (entry.bytes, entry.sha256.clone()),
        "generated fixture bytes and sha256 must match eviction index for {repo_relative_path}"
    );
}

/// Regenerates both evicted PMXT object manifests into caller-owned scratch space.
pub fn generate_evicted_pmxt_object_manifests(
    reference_root: &Path,
    temp_dir: &Path,
) -> (PathBuf, PathBuf) {
    let source_spec_path = reference_root.join(
        "backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/source-archive-index-source-universe.toml",
    );
    let temp_spec_path = temp_dir.join("source-archive-index-source-universe.toml");
    let aggregate_dir = temp_dir.join("manifest");
    let category_path =
        temp_dir.join("category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json");
    let spec = fs::read_to_string(&source_spec_path).unwrap_or_else(|error| {
        panic!(
            "read PMXT source-universe object-manifest spec {}: {error}",
            source_spec_path.display()
        )
    });
    let spec = rewrite_assignment(&spec, "output_dir", &aggregate_dir);
    let spec = rewrite_assignment(&spec, "category_manifest_path", &category_path);
    fs::write(&temp_spec_path, spec).expect("write temp PMXT object-manifest spec");

    let artifact =
        write_source_archive_index_source_universe_manifest_from_spec_file(&temp_spec_path)
            .expect("PMXT source-universe object-manifest generation succeeds");
    assert_generated_fixture_matches_index(
        PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH,
        &artifact.path,
    );
    assert_generated_fixture_matches_index(PMXT_CATEGORY_OBJECT_MANIFEST_PATH, &category_path);
    (artifact.path, category_path)
}

/// Materializes both evicted PMXT object manifests at the committed scratch paths.
pub fn materialize_evicted_pmxt_object_manifests(reference_root: &Path) -> (PathBuf, PathBuf) {
    let spec_path = reference_root.join(
        "backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/source-archive-index-source-universe.toml",
    );
    let artifact = write_source_archive_index_source_universe_manifest_from_spec_file(&spec_path)
        .expect("PMXT object manifests materialize at committed scratch paths");
    let category_path = repo_root_from_manifest_dir().join(
        "target/reference-regen/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json",
    );
    assert_generated_fixture_matches_index(
        PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH,
        &artifact.path,
    );
    assert_generated_fixture_matches_index(PMXT_CATEGORY_OBJECT_MANIFEST_PATH, &category_path);
    (artifact.path, category_path)
}
