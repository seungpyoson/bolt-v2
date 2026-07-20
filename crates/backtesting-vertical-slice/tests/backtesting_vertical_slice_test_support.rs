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
    reference_fixture_index::{
        EvictedFixtureIndex, TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH, repo_root_from_manifest_dir,
    },
    source_archive_index_source_universe::write_source_archive_index_source_universe_manifest_from_spec_file,
    source_universe_conversion_run_plan::write_source_universe_conversion_run_plan_from_spec_file,
    source_universe_operator_inputs::{
        SourceUniverseOperatorInputs, write_source_universe_operator_inputs_from_spec_file,
    },
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

pub const BYBIT_SOURCE_UNIVERSE_OPERATOR_INPUTS_PATH: &str = "specs/023-nt-research-analytics-platform/reference/source-universe-operator-inputs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/operator-inputs/source-universe-operator-inputs.json";
pub const BYBIT_OPERATOR_INPUTS_EVICTED_REFERENCE_PATHS: &[&str] =
    &[BYBIT_SOURCE_UNIVERSE_OPERATOR_INPUTS_PATH];

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

/// Regenerates the evicted Bybit operator inputs and its already-evicted run-plan input.
///
/// The generated operator-input bytes are normalized to the run plan's stable
/// evicted repo identity before the index assertion and before downstream tests
/// consume them.
pub fn generate_evicted_bybit_operator_inputs(
    reference_root: &Path,
    temp_dir: &Path,
) -> (PathBuf, PathBuf) {
    let run_plan_spec_path = reference_root.join(
        "source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-universe-conversion-run-plan.toml",
    );
    let temp_run_plan_spec_path = temp_dir.join("bybit-source-universe-conversion-run-plan.toml");
    let run_plan_spec = fs::read_to_string(&run_plan_spec_path).unwrap_or_else(|error| {
        panic!(
            "read Bybit run-plan spec {}: {error}",
            run_plan_spec_path.display()
        )
    });
    let run_plan_spec = rewrite_assignment(
        &run_plan_spec,
        "output_dir",
        &temp_dir.join("bybit-source-universe-conversion-run-plan"),
    );
    fs::write(&temp_run_plan_spec_path, run_plan_spec).expect("write temp Bybit run-plan spec");
    let run_plan_artifact =
        write_source_universe_conversion_run_plan_from_spec_file(&temp_run_plan_spec_path)
            .expect("Bybit run plan is reproducible");
    assert_generated_fixture_matches_index(
        TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH,
        &run_plan_artifact.path,
    );

    let operator_inputs_spec_path = reference_root.join(
        "source-universe-operator-inputs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-universe-operator-inputs.toml",
    );
    let temp_operator_inputs_spec_path =
        temp_dir.join("bybit-source-universe-operator-inputs.toml");
    let operator_inputs_spec =
        fs::read_to_string(&operator_inputs_spec_path).unwrap_or_else(|error| {
            panic!(
                "read Bybit operator-inputs spec {}: {error}",
                operator_inputs_spec_path.display()
            )
        });
    let operator_inputs_spec = rewrite_assignment(
        &operator_inputs_spec,
        "source_universe_conversion_run_plan_path",
        &run_plan_artifact.path,
    );
    let operator_inputs_spec = rewrite_assignment(
        &operator_inputs_spec,
        "output_dir",
        &temp_dir.join("bybit-source-universe-operator-inputs"),
    );
    fs::write(&temp_operator_inputs_spec_path, operator_inputs_spec)
        .expect("write temp Bybit operator-inputs spec");
    let operator_inputs_artifact =
        write_source_universe_operator_inputs_from_spec_file(&temp_operator_inputs_spec_path)
            .expect("Bybit operator inputs are reproducible");

    let bytes = fs::read(&operator_inputs_artifact.path).unwrap_or_else(|error| {
        panic!(
            "read generated Bybit operator inputs {}: {error}",
            operator_inputs_artifact.path.display()
        )
    });
    let mut operator_inputs: SourceUniverseOperatorInputs =
        serde_json::from_slice(&bytes).expect("generated Bybit operator inputs parse");
    let run_plan_ref = operator_inputs
        .artifact_refs
        .iter_mut()
        .find(|artifact_ref| artifact_ref.role == "source_universe_conversion_run_plan")
        .expect("Bybit operator inputs contain the run-plan artifact ref");
    run_plan_ref.path = Path::new(TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH).to_path_buf();
    let normalized =
        serde_json::to_vec_pretty(&operator_inputs).expect("serialize normalized operator inputs");
    assert_generated_fixture_bytes_match_index(
        BYBIT_SOURCE_UNIVERSE_OPERATOR_INPUTS_PATH,
        &normalized,
    );
    fs::write(&operator_inputs_artifact.path, normalized)
        .expect("write normalized Bybit operator inputs");

    (operator_inputs_artifact.path, run_plan_artifact.path)
}
