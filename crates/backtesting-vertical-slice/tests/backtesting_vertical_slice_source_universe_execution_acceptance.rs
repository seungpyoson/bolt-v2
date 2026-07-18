use std::{fs, path::Path};

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use std::collections::BTreeSet;

use crate::backtesting_vertical_slice_test_support::{
    PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH, materialize_evicted_pmxt_object_manifests,
    tempdir_in_repo_target,
};
use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH, TIER1_PMXT_CONVERSION_QUEUE_PATH,
    repo_root_from_manifest_dir,
};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use backtesting_vertical_slice::source_universe_batch_execution::SourceUniverseBatchBootstrapLimits;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use backtesting_vertical_slice::source_universe_batch_launch::{
    discover_committed_source_universe_execution_packs,
    inspect_worktree_source_universe_execution_pack_scope_names,
};
use backtesting_vertical_slice::source_universe_conversion_queue::write_source_universe_conversion_queue_from_spec_file;
use backtesting_vertical_slice::source_universe_conversion_run_plan::write_source_universe_conversion_run_plan_from_spec_file;
use backtesting_vertical_slice::source_universe_execution_acceptance::{
    SourceUniverseExecutionAcceptanceLedger, SourceUniverseExecutionAcceptanceLedgerSpec,
    SourceUniverseExecutionAcceptanceLedgerStatus, SourceUniverseExecutionAcceptanceUniverseStatus,
    evaluate_source_universe_execution_acceptance_ledger,
    write_source_universe_execution_acceptance_ledger_from_spec_file,
};
use backtesting_vertical_slice::source_universe_operator_inputs::SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn test_registry_bootstrap_limits() -> SourceUniverseBatchBootstrapLimits {
    SourceUniverseBatchBootstrapLimits {
        max_launch_artifact_bytes: 65_536,
        max_control_artifact_bytes: 65_536,
        max_retained_control_input_bytes: 262_144,
    }
}

fn copy_spec_with_output_dir(source_spec: &Path, target_spec: &Path, output_dir: &Path) {
    let spec = fs::read_to_string(source_spec).expect("read committed source-universe spec");
    let mut replaced = false;
    let updated = spec
        .lines()
        .map(|line| {
            if line.starts_with("output_dir = ") {
                replaced = true;
                format!("output_dir = \"{}\"", output_dir.display())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(replaced, "committed source-universe spec has output_dir");
    fs::write(target_spec, format!("{updated}\n")).expect("write temp source-universe spec");
}

fn replace_spec_path(spec_text: &str, committed_path: &str, temp_path: &Path) -> String {
    assert!(
        spec_text.contains(committed_path),
        "committed spec contains {committed_path}"
    );
    spec_text.replace(committed_path, &temp_path.display().to_string())
}

fn normalize_artifact_ref_path(
    ledger: &mut SourceUniverseExecutionAcceptanceLedger,
    universe_id: &str,
    role: &str,
    path: &str,
    sha256: &str,
) {
    let record = ledger
        .records
        .iter_mut()
        .find(|record| record.universe_id == universe_id)
        .expect("record exists");
    let artifact_ref = record
        .artifact_refs
        .iter_mut()
        .find(|artifact_ref| artifact_ref.role == role)
        .expect("artifact ref exists");
    artifact_ref.path = Path::new(path).to_path_buf();
    artifact_ref.sha256 = sha256.to_string();
}

fn artifact_ref_sha256(
    ledger: &SourceUniverseExecutionAcceptanceLedger,
    universe_id: &str,
    role: &str,
) -> String {
    ledger
        .records
        .iter()
        .find(|record| record.universe_id == universe_id)
        .expect("record exists")
        .artifact_refs
        .iter()
        .find(|artifact_ref| artifact_ref.role == role)
        .expect("artifact ref exists")
        .sha256
        .clone()
}

#[test]
fn source_universe_execution_acceptance_reports_ready_and_blocked_universes_without_overclaiming() {
    let temp_dir = tempdir_in_repo_target();
    let gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let run_plan_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.json");
    let operator_inputs_path = temp_dir.path().join("source-universe-operator-inputs.json");
    let manifest_path = temp_dir.path().join("pmxt-source-universe-manifest.json");
    let queue_path = temp_dir.path().join("pmxt-conversion-queue.json");
    let output_dir = temp_dir.path().join("execution-ledger");
    let spec_path = temp_dir
        .path()
        .join("source-universe-execution-acceptance-ledger.toml");

    fs::write(
        &gates_path,
        r#"{
  "schema_version": "source-universe-object-gates.v1",
  "gate_id": "source-universe-object-gates-binance-test",
  "status": "ready",
  "queue_id": "source-universe-conversion-queue-binance-test",
  "manifest_id": "backfill-source-universe-object-manifest-binance-test",
  "universe_id": "backfill-source-universe-binance-test",
  "venue": "binance",
  "source": "data_vision",
  "family": "trades",
  "table_family": "trades",
  "queue_path": "queue.json",
  "queue_hash": "queue-hash",
  "work_item_count": 2,
  "accepted_gate_count": 2,
  "source_binding_count": 1,
  "total_accepted_bytes": 300,
  "source_binding_summaries": [],
  "artifact_refs": [],
  "records": [
    {
      "work_item_id": "binance:BTCUSDT:2026-03-01:hash-a",
      "gate_status": "ready",
      "source_binding": "binance-spot-native-trades",
      "table_family": "trades",
      "category": "spot",
      "symbol": "BTCUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-a.zip",
      "source_url": "https://data.example/BTCUSDT.zip",
      "selected_object_sha256": "hash-a",
      "selected_object_bytes": 100,
      "source_proof_id": "source-proof-binance-test",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "category-manifest-binance-test",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-a",
      "accepted_tranche_id": "tranche-a",
      "output_prefix": "s3://example/backtests/a"
    },
    {
      "work_item_id": "binance:ETHUSDT:2026-03-01:hash-b",
      "gate_status": "ready",
      "source_binding": "binance-spot-native-trades",
      "table_family": "trades",
      "category": "spot",
      "symbol": "ETHUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-b.zip",
      "source_url": "https://data.example/ETHUSDT.zip",
      "selected_object_sha256": "hash-b",
      "selected_object_bytes": 200,
      "source_proof_id": "source-proof-binance-test",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "category-manifest-binance-test",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-b",
      "accepted_tranche_id": "tranche-b",
      "output_prefix": "s3://example/backtests/b"
    }
  ]
}"#,
    )
    .expect("write gates");
    fs::write(
        &run_plan_path,
        r#"{
  "schema_version": "source-universe-conversion-run-plan.v1",
  "plan_id": "source-universe-conversion-run-plan-binance-test",
  "status": "ready",
  "gate_id": "source-universe-object-gates-binance-test",
  "queue_id": "source-universe-conversion-queue-binance-test",
  "manifest_id": "backfill-source-universe-object-manifest-binance-test",
  "universe_id": "backfill-source-universe-binance-test",
  "venue": "binance",
  "source": "data_vision",
  "family": "trades",
  "table_family": "trades",
  "object_gates_path": "source-universe-object-gates.json",
  "object_gates_hash": "gates-hash",
  "max_objects_per_run": 500,
  "max_source_bytes_per_run": 1000,
  "source_binding_count": 1,
  "object_count": 2,
  "planned_object_count": 2,
  "total_source_bytes": 300,
  "planned_source_bytes": 300,
  "run_count": 1,
  "category_summaries": [],
  "artifact_refs": [],
  "runs": [
    {
      "run_id": "source-universe-conversion-run-plan-binance-test:run-00001",
      "run_index": 1,
      "source_binding": "binance-spot-native-trades",
      "table_family": "trades",
      "category": "spot",
      "first_archive_date": "2026-03-01",
      "last_archive_date": "2026-03-01",
      "object_count": 2,
      "source_bytes": 300,
      "work_item_ids": [
        "binance:BTCUSDT:2026-03-01:hash-a",
        "binance:ETHUSDT:2026-03-01:hash-b"
      ],
      "accepted_tranche_ids": ["tranche-a", "tranche-b"],
      "output_prefixes": ["s3://example/backtests/a", "s3://example/backtests/b"]
    }
  ]
}"#,
    )
    .expect("write run plan");
    fs::write(
        &operator_inputs_path,
        format!(
            "{}{}{}",
            "{\n  \"schema_version\": \"",
            SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION,
            r#"",
  "input_id": "source-universe-operator-inputs-binance-test",
  "status": "blocked",
  "gate_id": "source-universe-object-gates-binance-test",
  "conversion_run_plan_id": "source-universe-conversion-run-plan-binance-test",
  "universe_id": "backfill-source-universe-binance-test",
  "venue": "binance",
  "source": "data_vision",
  "family": "trades",
  "table_family": "trades",
  "operator_run_id_prefix": "source-universe-operator-run-binance-test",
  "nt_venue": "BINANCE",
  "converter_identity": "csv-native-trades-to-canonical-trades.v1",
  "converter_version": "1",
  "raw_payload_container": "single_csv_zip",
  "max_decoded_bytes": 268435456,
  "max_source_rows": 1000000,
  "max_projected_row_groups": 128,
  "max_wall_seconds": 1800,
  "planned_object_count": 2,
  "planned_source_bytes": 300,
  "conversion_run_count": 1,
  "instrument_spec_count": 1,
  "converter_mapping_count": 1,
  "ready_input_count": 1,
  "blocked_input_count": 1,
  "artifact_refs": [],
  "converter_mappings": [],
  "instrument_specs": [],
  "records": [
    {
      "work_item_id": "binance:BTCUSDT:2026-03-01:hash-a",
      "status": "ready",
      "operator_run_id": "source-universe-operator-run-binance-test-00000",
      "source_binding": "binance-spot-native-trades",
      "category": "spot",
      "symbol": "BTCUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-a.zip",
      "source_url": "https://data.example/BTCUSDT.zip",
      "selected_object_sha256": "hash-a",
      "selected_object_bytes": 100,
      "source_proof_id": "source-proof-binance-test",
      "source_proof_version": 1,
      "accepted_tranche_id": "tranche-a",
      "output_prefix": "s3://example/backtests/a",
      "instrument_key": "binance-spot-native-trades:spot:BTCUSDT",
      "converter_identity": "csv-native-trades-to-canonical-trades.v1",
      "converter_version": "1",
      "raw_payload_container": "single_csv_zip",
      "zip_member": "BTCUSDT-trades-2026-03-01.csv",
      "max_decoded_bytes": 268435456,
      "max_source_rows": 1000000,
      "max_projected_row_groups": 128,
      "max_wall_seconds": 1800,
      "schema_columns": null,
      "converter_csv": null,
      "blocking_reasons": []
    },
    {
      "work_item_id": "binance:ETHUSDT:2026-03-01:hash-b",
      "status": "blocked",
      "operator_run_id": "source-universe-operator-run-binance-test-00001",
      "source_binding": "binance-spot-native-trades",
      "category": "spot",
      "symbol": "ETHUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-b.zip",
      "source_url": "https://data.example/ETHUSDT.zip",
      "selected_object_sha256": "hash-b",
      "selected_object_bytes": 200,
      "source_proof_id": "source-proof-binance-test",
      "source_proof_version": 1,
      "accepted_tranche_id": "tranche-b",
      "output_prefix": "s3://example/backtests/b",
      "instrument_key": "binance-spot-native-trades:spot:ETHUSDT",
      "converter_identity": "csv-native-trades-to-canonical-trades.v1",
      "converter_version": "1",
      "raw_payload_container": "single_csv_zip",
      "zip_member": "ETHUSDT-trades-2026-03-01.csv",
      "max_decoded_bytes": 268435456,
      "max_source_rows": 1000000,
      "max_projected_row_groups": 128,
      "max_wall_seconds": 1800,
      "schema_columns": null,
      "converter_csv": null,
      "blocking_reasons": ["missing_instrument_metadata"]
    }
  ],
  "blocking_reasons": ["blocked_operator_input_records"]
}"#,
        ),
    )
    .expect("write operator inputs");
    fs::write(&manifest_path, b"{}").expect("write pmxt manifest");
    fs::write(
        &queue_path,
        r#"{
  "schema_version": "source-universe-conversion-queue.v1",
  "queue_id": "source-universe-conversion-queue-pmxt-test",
  "status": "ready",
  "manifest_id": "backfill-source-universe-object-manifest-pmxt-test",
  "universe_id": "backfill-source-universe-pmxt-test",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "family": "orderbook",
  "table_family": "orderbook",
  "source_manifest_path": "pmxt-source-universe-manifest.json",
  "source_manifest_hash": "manifest-hash",
  "output_prefix_template": "s3://example/pmxt/{symbol}",
  "work_item_count": 2,
  "pending_conversion_items": 2,
  "total_source_bytes": 300,
  "category_summaries": [],
  "artifact_refs": [],
  "work_items": [
    {
      "work_item_id": "pmxt:orderbook:POLYMARKET:2026-06-10T15:00:00Z:hash-a",
      "work_state": "pending_conversion",
      "source_binding": "polymarket-parquet-archive-index",
      "table_family": "orderbook",
      "category": "orderbook",
      "symbol": "POLYMARKET",
      "archive_date": "2026-06-10T15:00:00Z",
      "source_uri": "s3://example/pmxt/a.parquet",
      "source_url": "https://example.invalid/pmxt/a.parquet",
      "source_hash_algorithm": "etag",
      "source_hash": "hash-a",
      "source_bytes": 100,
      "schema_columns": ["timestamp", "market", "asset_id", "bids", "asks"],
      "output_prefix": "s3://example/pmxt/a"
    },
    {
      "work_item_id": "pmxt:orderbook:POLYMARKET:2026-06-10T16:00:00Z:hash-b",
      "work_state": "pending_conversion",
      "source_binding": "polymarket-parquet-archive-index",
      "table_family": "orderbook",
      "category": "orderbook",
      "symbol": "POLYMARKET",
      "archive_date": "2026-06-10T16:00:00Z",
      "source_uri": "s3://example/pmxt/b.parquet",
      "source_url": "https://example.invalid/pmxt/b.parquet",
      "source_hash_algorithm": "etag",
      "source_hash": "hash-b",
      "source_bytes": 200,
      "schema_columns": ["timestamp", "market", "asset_id", "bids", "asks"],
      "output_prefix": "s3://example/pmxt/b"
    }
  ]
}"#,
    )
    .expect("write pmxt queue");
    fs::write(
        &spec_path,
        format!(
            r#"ledger_id = "source-universe-execution-acceptance-ledger-test"
output_dir = "{output_dir}"

[[universe]]
universe_id = "backfill-source-universe-binance-test"
venue = "binance"
source = "data_vision"
family = "trades"
source_universe_object_gates_path = "{gates_path}"
source_universe_conversion_run_plan_path = "{run_plan_path}"
source_universe_operator_inputs_path = "{operator_inputs_path}"

[[universe]]
universe_id = "backfill-source-universe-pmxt-test"
venue = "pmxt"
source = "polymarket-v2-archive"
family = "orderbook"
source_universe_manifest_path = "{manifest_path}"
source_universe_conversion_queue_path = "{queue_path}"
blocking_reasons = [
  "missing_pmxt_l2_tick_size_epoch_policy",
]
"#,
            output_dir = output_dir.display(),
            gates_path = gates_path.display(),
            run_plan_path = run_plan_path.display(),
            operator_inputs_path = operator_inputs_path.display(),
            manifest_path = manifest_path.display(),
            queue_path = queue_path.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_execution_acceptance_ledger_from_spec_file(&spec_path)
        .expect("write execution acceptance ledger");
    let ledger: SourceUniverseExecutionAcceptanceLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("ledger bytes"))
            .expect("ledger parses");

    assert_eq!(
        ledger.status,
        SourceUniverseExecutionAcceptanceLedgerStatus::Incomplete
    );
    assert_eq!(ledger.ready_for_conversion_universes, 0);
    assert_eq!(ledger.partially_ready_for_conversion_universes, 1);
    assert_eq!(ledger.blocked_universes, 1);
    assert_eq!(ledger.total_planned_conversion_objects, 4);
    assert_eq!(ledger.total_required_single_object_operator_runs, 4);
    assert_eq!(ledger.total_executable_single_object_operator_runs, 1);
    assert_eq!(ledger.total_materialized_single_object_operator_runs, 0);
    assert_eq!(ledger.total_withheld_conversion_objects, 3);

    let binance = record(&ledger, "backfill-source-universe-binance-test");
    assert_eq!(
        binance.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::PartiallyReadyForConversionExecution
    );
    assert_eq!(binance.source_conversion_batch_count, 1);
    assert_eq!(binance.planned_conversion_objects, 2);
    assert_eq!(binance.ready_operator_input_count, 1);
    assert_eq!(binance.blocked_operator_input_count, 1);
    assert_eq!(binance.executable_single_object_operator_runs, 1);
    assert_eq!(binance.materialized_single_object_operator_runs, 0);
    assert_eq!(binance.withheld_conversion_objects, 1);
    assert_eq!(binance.remaining_conversion_objects, 2);
    assert!(
        binance
            .blocking_reasons
            .iter()
            .any(|reason| reason == "missing_instrument_metadata")
    );

    let pmxt = record(&ledger, "backfill-source-universe-pmxt-test");
    assert_eq!(
        pmxt.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
    );
    assert_eq!(pmxt.planned_conversion_objects, 2);
    assert_eq!(pmxt.planned_source_bytes, 300);
    assert_eq!(pmxt.required_single_object_operator_runs, 2);
    assert_eq!(pmxt.executable_single_object_operator_runs, 0);
    assert_eq!(pmxt.materialized_single_object_operator_runs, 0);
    assert_eq!(pmxt.withheld_conversion_objects, 2);
    assert!(
        pmxt.blocking_reasons
            .iter()
            .any(|reason| reason == "missing_source_universe_object_gates")
    );
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn committed_execution_pack_registry_and_acceptance_ledger_are_an_exact_set() {
    let repo_root = repo_root_from_manifest_dir()
        .canonicalize()
        .expect("repository root canonicalizes");
    let scope_names = inspect_worktree_source_universe_execution_pack_scope_names(&repo_root, 64)
        .expect("inspect committed execution-pack registry");
    let registry_summary_paths = discover_committed_source_universe_execution_packs(
        &repo_root,
        &scope_names,
        test_registry_bootstrap_limits(),
    )
    .expect("discover committed execution-pack registry")
    .into_iter()
    .map(|pack| pack.summary_path)
    .collect::<BTreeSet<_>>();

    let acceptance_ledgers_root = repo_root.join(
        "specs/023-nt-research-analytics-platform/reference/source-universe-execution-acceptance-ledgers",
    );
    let mut ledger_spec_paths = fs::read_dir(&acceptance_ledgers_root)
        .expect("read committed execution-acceptance ledger root")
        .map(|entry| {
            let entry = entry.expect("read committed execution-acceptance ledger entry");
            assert!(
                entry.file_type().expect("stat ledger entry").is_dir(),
                "committed execution-acceptance ledger entry must be a directory: {}",
                entry.path().display()
            );
            let spec_path = entry
                .path()
                .join("source-universe-execution-acceptance-ledger.toml");
            assert!(
                spec_path.is_file(),
                "committed execution-acceptance ledger entry must contain {}",
                spec_path.display()
            );
            spec_path
        })
        .collect::<Vec<_>>();
    ledger_spec_paths.sort();
    assert_eq!(
        ledger_spec_paths.len(),
        1,
        "exactly one committed execution-acceptance ledger must be authoritative"
    );
    let ledger_spec: SourceUniverseExecutionAcceptanceLedgerSpec = toml::from_slice(
        &fs::read(&ledger_spec_paths[0]).expect("read authoritative execution-acceptance ledger"),
    )
    .expect("parse authoritative execution-acceptance ledger");

    let mut ledger_summary_paths = BTreeSet::new();
    for summary_path in ledger_spec
        .universes
        .iter()
        .filter_map(|universe| universe.source_universe_execution_pack_path.as_ref())
    {
        assert!(
            !summary_path.is_absolute(),
            "execution-acceptance ledger summary path must be repository-relative: {}",
            summary_path.display()
        );
        let canonical_summary_path =
            repo_root
                .join(summary_path)
                .canonicalize()
                .unwrap_or_else(|error| {
                    panic!(
                        "canonicalize execution-acceptance ledger summary {}: {error}",
                        summary_path.display()
                    )
                });
        assert!(
            ledger_summary_paths.insert(canonical_summary_path.clone()),
            "duplicate execution-pack summary reference in acceptance ledger: {}",
            canonical_summary_path.display()
        );
    }

    let registry_only = registry_summary_paths
        .difference(&ledger_summary_paths)
        .cloned()
        .collect::<Vec<_>>();
    let ledger_only = ledger_summary_paths
        .difference(&registry_summary_paths)
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        registry_only.is_empty() && ledger_only.is_empty(),
        "committed execution-pack registry and acceptance ledger must cover each other exactly; registry_only={registry_only:?}, ledger_only={ledger_only:?}"
    );
}

#[test]
fn committed_source_universe_execution_acceptance_ledger_tracks_current_venue_scale_state() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference")
        .canonicalize()
        .expect("reference root canonicalizes");
    let ledger_path = reference_root
        .join("source-universe-execution-acceptance-ledgers/binance-bybit-pmxt-current")
        .join("ledger/source-universe-execution-acceptance-ledger.json");

    let ledger: SourceUniverseExecutionAcceptanceLedger =
        serde_json::from_slice(&fs::read(&ledger_path).expect("ledger bytes"))
            .expect("ledger parses");

    assert_eq!(
        ledger.status,
        SourceUniverseExecutionAcceptanceLedgerStatus::Incomplete
    );
    assert_eq!(ledger.universe_count, 3);
    assert_eq!(ledger.converted_universes, 0);
    assert_eq!(ledger.ready_for_conversion_universes, 0);
    assert_eq!(ledger.partially_ready_for_conversion_universes, 0);
    assert_eq!(ledger.blocked_universes, 3);
    assert_eq!(ledger.total_planned_conversion_objects, 9_259);
    assert_eq!(ledger.total_required_single_object_operator_runs, 9_259);
    assert_eq!(ledger.total_executable_single_object_operator_runs, 0);
    assert_eq!(ledger.total_materialized_single_object_operator_runs, 2);
    assert_eq!(ledger.total_withheld_conversion_objects, 9_259);
    assert_eq!(ledger.total_remaining_conversion_objects, 9_259);

    let binance = record(
        &ledger,
        "backfill-source-universe-binance-data-vision-trades-2026-03-01-all-instruments",
    );
    assert_eq!(
        binance.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
    );
    assert_eq!(binance.source_gate_count, 2_051);
    assert_eq!(binance.source_conversion_batch_count, 8);
    assert_eq!(binance.planned_conversion_objects, 2_051);
    assert_eq!(binance.ready_operator_input_count, 2_035);
    assert_eq!(binance.blocked_operator_input_count, 16);
    assert_eq!(binance.required_single_object_operator_runs, 2_051);
    assert_eq!(binance.executable_single_object_operator_runs, 0);
    assert_eq!(binance.materialized_single_object_operator_runs, 1);
    assert_eq!(binance.withheld_conversion_objects, 2_051);
    assert!(
        binance
            .blocking_reasons
            .iter()
            .any(|reason| reason == "missing_instrument_metadata")
    );
    assert!(
        binance
            .blocking_reasons
            .iter()
            .any(|reason| reason == "source_universe_execution_pack_skipped_executable_records")
    );

    let bybit = record(
        &ledger,
        "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
    );
    assert_eq!(
        bybit.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
    );
    assert_eq!(bybit.source_gate_count, 5_857);
    assert_eq!(bybit.source_conversion_batch_count, 19);
    assert_eq!(bybit.planned_conversion_objects, 5_857);
    assert_eq!(bybit.ready_operator_input_count, 5_857);
    assert_eq!(bybit.blocked_operator_input_count, 0);
    assert_eq!(bybit.required_single_object_operator_runs, 5_857);
    assert_eq!(bybit.executable_single_object_operator_runs, 0);
    assert_eq!(bybit.materialized_single_object_operator_runs, 1);
    assert_eq!(bybit.withheld_conversion_objects, 5_857);
    assert_eq!(
        bybit.blocking_reasons,
        ["source_universe_execution_pack_skipped_executable_records"]
    );

    let pmxt = record(
        &ledger,
        "backfill-source-universe-pmxt-polymarket-v2-current",
    );
    assert_eq!(
        pmxt.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
    );
    assert_eq!(pmxt.planned_conversion_objects, 1_351);
    assert_eq!(pmxt.planned_source_bytes, 557_815_904_970);
    assert_eq!(pmxt.required_single_object_operator_runs, 1_351);
    assert_eq!(pmxt.executable_single_object_operator_runs, 0);
    assert_eq!(pmxt.materialized_single_object_operator_runs, 0);
    assert_eq!(pmxt.withheld_conversion_objects, 1_351);
    assert!(
        pmxt.blocking_reasons
            .iter()
            .any(|reason| reason == "missing_pmxt_l2_tick_size_epoch_policy")
    );
    assert!(
        pmxt.blocking_reasons
            .iter()
            .any(|reason| reason == "missing_source_universe_object_gates")
    );
    assert!(
        pmxt.blocking_reasons
            .iter()
            .any(|reason| reason == "missing_source_universe_conversion_run_plan")
    );
}

#[test]
fn committed_source_universe_execution_acceptance_ledger_round_trips_through_evaluator() {
    // Read the committed spec TOML and the committed output JSON.
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    materialize_evicted_pmxt_object_manifests(&reference_root);
    let spec_path = reference_root
        .join("source-universe-execution-acceptance-ledgers/binance-bybit-pmxt-current")
        .join("source-universe-execution-acceptance-ledger.toml");
    let spec_path = spec_path
        .canonicalize()
        .expect("committed spec path must exist");

    let committed_ledger_path = spec_path
        .parent()
        .expect("spec parent")
        .join("ledger/source-universe-execution-acceptance-ledger.json");
    let committed_bytes = fs::read(&committed_ledger_path).expect("read committed ledger");
    let committed_ledger: SourceUniverseExecutionAcceptanceLedger =
        serde_json::from_slice(&committed_bytes).expect("parse committed ledger");
    let evicted_index =
        EvictedFixtureIndex::load(&repo_root_from_manifest_dir()).expect("load eviction index");

    let temp_dir = tempdir_in_repo_target();
    let bybit_run_plan_spec = temp_dir
        .path()
        .join("bybit-source-universe-conversion-run-plan.toml");
    copy_spec_with_output_dir(
        &reference_root
            .join("source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01")
            .join("source-universe-conversion-run-plan.toml"),
        &bybit_run_plan_spec,
        &temp_dir.path().join("bybit-run-plan"),
    );
    let bybit_run_plan_artifact =
        write_source_universe_conversion_run_plan_from_spec_file(&bybit_run_plan_spec)
            .expect("Bybit run plan is reproducible");
    let bybit_run_plan_sha256 = evicted_index
        .sha256_for(TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH)
        .unwrap_or_else(|| {
            panic!("evicted fixture index does not contain {TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH}")
        });
    assert_eq!(
        bybit_run_plan_artifact.content_hash, bybit_run_plan_sha256,
        "regenerated Bybit run-plan bytes must match the evicted fixture index"
    );

    let pmxt_queue_spec = temp_dir
        .path()
        .join("pmxt-source-universe-conversion-queue.toml");
    copy_spec_with_output_dir(
        &reference_root
            .join("source-universe-conversion-queues/pmxt-polymarket-v2-current")
            .join("source-universe-conversion-queue.toml"),
        &pmxt_queue_spec,
        &temp_dir.path().join("pmxt-conversion-queue"),
    );
    let pmxt_queue_artifact =
        write_source_universe_conversion_queue_from_spec_file(&pmxt_queue_spec)
            .expect("PMXT queue is reproducible");
    let pmxt_queue_sha256 = evicted_index
        .sha256_for(TIER1_PMXT_CONVERSION_QUEUE_PATH)
        .unwrap_or_else(|| {
            panic!("evicted fixture index does not contain {TIER1_PMXT_CONVERSION_QUEUE_PATH}")
        });
    assert_eq!(
        pmxt_queue_artifact.content_hash, pmxt_queue_sha256,
        "regenerated PMXT queue bytes must match the evicted fixture index"
    );

    let spec_text = fs::read_to_string(&spec_path).expect("read committed spec");
    let spec_text = replace_spec_path(
        &spec_text,
        TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH,
        &bybit_run_plan_artifact.path,
    );
    let spec_text = replace_spec_path(
        &spec_text,
        TIER1_PMXT_CONVERSION_QUEUE_PATH,
        &pmxt_queue_artifact.path,
    );
    let spec: SourceUniverseExecutionAcceptanceLedgerSpec =
        toml::from_str(&spec_text).expect("parse temp committed spec TOML");
    let base_dir = spec_path.parent().expect("spec parent");
    let mut evaluated = evaluate_source_universe_execution_acceptance_ledger(&spec, base_dir)
        .expect("evaluate committed spec");
    let evaluated_bybit_run_plan_sha256 = artifact_ref_sha256(
        &evaluated,
        "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
        "source_universe_conversion_run_plan",
    );
    let evaluated_pmxt_queue_sha256 = artifact_ref_sha256(
        &evaluated,
        "backfill-source-universe-pmxt-polymarket-v2-current",
        "source_universe_conversion_queue",
    );
    let pmxt_manifest_sha256 = evicted_index
        .sha256_for(PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH)
        .expect("evicted fixture index contains PMXT source-universe manifest");
    assert_eq!(
        evaluated_bybit_run_plan_sha256, bybit_run_plan_artifact.content_hash,
        "evaluator must hash the freshly regenerated Bybit run-plan artifact before ledger normalization"
    );
    assert_eq!(
        evaluated_pmxt_queue_sha256, pmxt_queue_artifact.content_hash,
        "evaluator must hash the freshly regenerated PMXT queue artifact before ledger normalization"
    );
    normalize_artifact_ref_path(
        &mut evaluated,
        "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
        "source_universe_conversion_run_plan",
        TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH,
        &bybit_run_plan_sha256,
    );
    normalize_artifact_ref_path(
        &mut evaluated,
        "backfill-source-universe-pmxt-polymarket-v2-current",
        "source_universe_manifest",
        PMXT_SOURCE_UNIVERSE_OBJECT_MANIFEST_PATH,
        &pmxt_manifest_sha256,
    );
    normalize_artifact_ref_path(
        &mut evaluated,
        "backfill-source-universe-pmxt-polymarket-v2-current",
        "source_universe_conversion_queue",
        TIER1_PMXT_CONVERSION_QUEUE_PATH,
        &pmxt_queue_sha256,
    );

    assert_eq!(
        evaluated, committed_ledger,
        "evaluator output must match the committed ledger; \
         if input artifacts changed, regenerate the committed ledger from the spec"
    );

    // The struct `PartialEq` above can silently miss an added optional/default
    // field that round-trips to a default. Compare the serialized bytes too: the
    // writer emits `serde_json::to_vec_pretty` with no trailing newline, so the
    // re-serialized evaluator output must be byte-identical to the committed file.
    let reserialized = serde_json::to_vec_pretty(&evaluated).expect("serialize evaluated ledger");
    let first_difference = reserialized
        .iter()
        .zip(&committed_bytes)
        .position(|(generated, committed)| generated != committed);
    assert!(
        reserialized == committed_bytes,
        "re-serialized evaluator output must be byte-identical to the committed ledger JSON; \
         a mismatch signals serialized field drift the struct PartialEq cannot see; \
         generated_bytes={}, committed_bytes={}, first_difference={first_difference:?}",
        reserialized.len(),
        committed_bytes.len(),
    );
}

#[test]
fn source_universe_execution_acceptance_blocks_on_spec_vs_artifact_family_mismatch() {
    // Negative regression for the spec-vs-artifact family check: when a loaded
    // artifact's `family` disagrees with the spec's declared `family`, the
    // evaluator must push a `source_universe_spec_family_mismatch_<role>` blocking
    // reason and mark the universe Blocked. This mirrors the pmxt-universe
    // construction in the round-trip-free fixture test above (a conversion-queue
    // artifact), but deliberately declares a spec family that disagrees with the
    // artifact's `orderbook` family.
    let temp_dir = tempdir_in_repo_target();
    let manifest_path = temp_dir.path().join("pmxt-source-universe-manifest.json");
    let queue_path = temp_dir.path().join("pmxt-conversion-queue.json");
    let output_dir = temp_dir.path().join("execution-ledger");
    let spec_path = temp_dir
        .path()
        .join("source-universe-execution-acceptance-ledger.toml");

    fs::write(&manifest_path, b"{}").expect("write pmxt manifest");
    // The conversion-queue artifact carries `family: "orderbook"`.
    fs::write(
        &queue_path,
        r#"{
  "schema_version": "source-universe-conversion-queue.v1",
  "queue_id": "source-universe-conversion-queue-pmxt-mismatch-test",
  "status": "ready",
  "manifest_id": "backfill-source-universe-object-manifest-pmxt-mismatch-test",
  "universe_id": "backfill-source-universe-pmxt-mismatch-test",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "family": "orderbook",
  "table_family": "orderbook",
  "source_manifest_path": "pmxt-source-universe-manifest.json",
  "source_manifest_hash": "manifest-hash",
  "output_prefix_template": "s3://example/pmxt/{symbol}",
  "work_item_count": 1,
  "pending_conversion_items": 1,
  "total_source_bytes": 100,
  "category_summaries": [],
  "artifact_refs": [],
  "work_items": [
    {
      "work_item_id": "pmxt:orderbook:POLYMARKET:2026-06-10T15:00:00Z:hash-a",
      "work_state": "pending_conversion",
      "source_binding": "polymarket-parquet-archive-index",
      "table_family": "orderbook",
      "category": "orderbook",
      "symbol": "POLYMARKET",
      "archive_date": "2026-06-10T15:00:00Z",
      "source_uri": "s3://example/pmxt/a.parquet",
      "source_url": "https://example.invalid/pmxt/a.parquet",
      "source_hash_algorithm": "etag",
      "source_hash": "hash-a",
      "source_bytes": 100,
      "schema_columns": ["timestamp", "market", "asset_id", "bids", "asks"],
      "output_prefix": "s3://example/pmxt/a"
    }
  ]
}"#,
    )
    .expect("write pmxt queue");
    // The spec declares `family = "trades"`, which disagrees with the queue
    // artifact's `orderbook` family above — the mismatch the check must catch.
    fs::write(
        &spec_path,
        format!(
            r#"ledger_id = "source-universe-execution-acceptance-ledger-mismatch-test"
output_dir = "{output_dir}"

[[universe]]
universe_id = "backfill-source-universe-pmxt-mismatch-test"
venue = "pmxt"
source = "polymarket-v2-archive"
family = "trades"
source_universe_manifest_path = "{manifest_path}"
source_universe_conversion_queue_path = "{queue_path}"
"#,
            output_dir = output_dir.display(),
            manifest_path = manifest_path.display(),
            queue_path = queue_path.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_execution_acceptance_ledger_from_spec_file(&spec_path)
        .expect("write execution acceptance ledger");
    let ledger: SourceUniverseExecutionAcceptanceLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("ledger bytes"))
            .expect("ledger parses");

    let pmxt = record(&ledger, "backfill-source-universe-pmxt-mismatch-test");
    assert_eq!(
        pmxt.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked,
        "a spec-vs-artifact family mismatch must mark the universe Blocked"
    );
    assert!(
        pmxt.blocking_reasons
            .iter()
            .any(|reason| reason.starts_with("source_universe_spec_family_mismatch_")),
        "expected a source_universe_spec_family_mismatch_<role> blocking reason, got: {:?}",
        pmxt.blocking_reasons
    );
}

fn record<'a>(
    ledger: &'a SourceUniverseExecutionAcceptanceLedger,
    universe_id: &str,
) -> &'a backtesting_vertical_slice::source_universe_execution_acceptance::SourceUniverseExecutionAcceptanceRecord
{
    ledger
        .records
        .iter()
        .find(|record| record.universe_id == universe_id)
        .unwrap_or_else(|| panic!("missing record {universe_id}"))
}
