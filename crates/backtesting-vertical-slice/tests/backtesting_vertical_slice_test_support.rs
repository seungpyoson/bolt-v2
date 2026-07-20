use std::{
    fs,
    path::{Path, PathBuf},
};

use backtesting_vertical_slice::{
    backfill_conversion_batch::{
        BackfillConversionBatchPlan, write_backfill_conversion_batch_plan_from_spec_file,
    },
    backfill_conversion_completion::{
        BackfillConversionCompletionLedger,
        write_backfill_conversion_completion_ledger_from_spec_file,
    },
    hashing::sha256_hex,
    reference_fixture_index::{EvictedFixtureIndex, repo_root_from_manifest_dir},
    source_archive_index_source_universe::write_source_archive_index_source_universe_manifest_from_spec_file,
    source_universe_operator_inputs::write_source_universe_operator_inputs_from_spec_file,
};

pub const PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/binance-bnbusdc-2026-03-01-2026-05-31/plan/backfill-conversion-batch-plan.json";
pub const PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-conversion-batches/bybit-bnbusdc-2026-03-01-2026-06-01/plan/backfill-conversion-batch-plan.json";

pub const PHASE3_EVICTED_REFERENCE_PATHS: &[&str] = &[
    PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
];

pub const BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-conversion-completion-ledgers/binance-bnbusdc-2026-03-01-2026-05-31/ledger/backfill-conversion-completion-ledger.json";
pub const BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-conversion-completion-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01/ledger/backfill-conversion-completion-ledger.json";

pub const BACKFILL_CONVERSION_COMPLETION_LEDGER_EVICTED_REFERENCE_PATHS: &[&str] = &[
    BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
    BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
];

pub const PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json";
pub const PMXT_CATEGORY_OBJECT_MANIFEST_PATH: &str = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/category-manifests/pmxt-polymarket-v2-object-manifest-orderbook.json";
pub const PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_REGEN_PATH: &str = "target/reference-regen/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json";
pub const PMXT_OBJECT_MANIFEST_EVICTED_REFERENCE_PATHS: &[&str] = &[
    PMXT_CATEGORY_OBJECT_MANIFEST_PATH,
    PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH,
];

pub const BINANCE_SOURCE_UNIVERSE_OPERATOR_INPUTS_PATH: &str = "specs/023-nt-research-analytics-platform/reference/source-universe-operator-inputs/binance-data-vision-trades-2026-03-01-all-instruments/operator-inputs/source-universe-operator-inputs.json";
pub const BINANCE_OPERATOR_INPUTS_EVICTED_REFERENCE_PATHS: &[&str] =
    &[BINANCE_SOURCE_UNIVERSE_OPERATOR_INPUTS_PATH];

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
    let artifact_path =
        generate_evicted_batch_plan(&batch_root, repo_relative_path, temp_dir.path());
    let bytes = fs::read(&artifact_path).unwrap_or_else(|error| {
        panic!(
            "read generated fixture {}: {error}",
            artifact_path.display()
        )
    });
    serde_json::from_slice(&bytes).expect("conversion batch plan parses")
}

/// Regenerates one evicted completion-ledger family and returns its artifact path.
///
/// The caller owns `temp_dir`, which must be dedicated to one family because the
/// temporary spec and output names are fixed within that directory.
pub fn generate_evicted_completion_ledger(
    reference_root: &Path,
    scope: &str,
    evicted_batch_plan_path: &str,
    evicted_ledger_path: &str,
    temp_dir: &Path,
) -> PathBuf {
    let batch_root = reference_root.join(format!("backfill-conversion-batches/{scope}"));
    let batch_plan_path =
        generate_evicted_batch_plan(&batch_root, evicted_batch_plan_path, temp_dir);
    let repo_root = repo_root_from_manifest_dir();
    let batch_plan_path = batch_plan_path
        .strip_prefix(&repo_root)
        .unwrap_or(&batch_plan_path)
        .to_path_buf();

    let ledger_root =
        reference_root.join(format!("backfill-conversion-completion-ledgers/{scope}"));
    let ledger_spec_path = ledger_root.join("backfill-conversion-completion-ledger.toml");
    let temp_ledger_spec_path = temp_dir.join("backfill-conversion-completion-ledger.toml");
    let ledger_spec = fs::read_to_string(&ledger_spec_path).unwrap_or_else(|error| {
        panic!(
            "read completion ledger spec {}: {error}",
            ledger_spec_path.display()
        )
    });
    let ledger_spec = rewrite_assignment(&ledger_spec, "batch_plan_path", &batch_plan_path);
    let ledger_spec = rewrite_assignment(
        &ledger_spec,
        "output_dir",
        &temp_dir.join("completion-ledger"),
    );
    fs::write(&temp_ledger_spec_path, ledger_spec).expect("write temp completion ledger spec");

    let artifact =
        write_backfill_conversion_completion_ledger_from_spec_file(&temp_ledger_spec_path)
            .expect("completion ledger generation succeeds");
    assert_generated_fixture_matches_index(evicted_ledger_path, &artifact.path);
    artifact.path
}

/// Regenerates and parses one evicted completion ledger in an owned temporary directory.
pub fn generated_evicted_completion_ledger(
    reference_root: &Path,
    scope: &str,
    evicted_batch_plan_path: &str,
    evicted_ledger_path: &str,
) -> BackfillConversionCompletionLedger {
    let temp_dir = tempdir_in_repo_target();
    let artifact_path = generate_evicted_completion_ledger(
        reference_root,
        scope,
        evicted_batch_plan_path,
        evicted_ledger_path,
        temp_dir.path(),
    );
    let bytes = fs::read(&artifact_path).unwrap_or_else(|error| {
        panic!(
            "read generated fixture {}: {error}",
            artifact_path.display()
        )
    });
    serde_json::from_slice(&bytes).expect("completion ledger parses")
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

/// Regenerates the evicted Binance operator inputs into caller-owned scratch space.
pub fn generate_evicted_binance_operator_inputs(reference_root: &Path, temp_dir: &Path) -> PathBuf {
    let source_spec_path = reference_root.join(
        "source-universe-operator-inputs/binance-data-vision-trades-2026-03-01-all-instruments/source-universe-operator-inputs.toml",
    );
    let temp_spec_path = temp_dir.join("binance-source-universe-operator-inputs.toml");
    let output_dir = temp_dir.join("binance-source-universe-operator-inputs");
    let spec = fs::read_to_string(&source_spec_path).unwrap_or_else(|error| {
        panic!(
            "read Binance operator-inputs spec {}: {error}",
            source_spec_path.display()
        )
    });
    let spec = rewrite_assignment(&spec, "output_dir", &output_dir);
    fs::write(&temp_spec_path, spec).expect("write temp Binance operator-inputs spec");

    let artifact = write_source_universe_operator_inputs_from_spec_file(&temp_spec_path)
        .expect("Binance operator inputs are reproducible");
    assert_generated_fixture_matches_index(
        BINANCE_SOURCE_UNIVERSE_OPERATOR_INPUTS_PATH,
        &artifact.path,
    );
    artifact.path
}
