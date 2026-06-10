use std::{fs, path::Path};

use backtesting_vertical_slice::source_universe_execution_acceptance::{
    SourceUniverseExecutionAcceptanceLedger, SourceUniverseExecutionAcceptanceLedgerStatus,
    SourceUniverseExecutionAcceptanceUniverseStatus,
    write_source_universe_execution_acceptance_ledger_from_spec_file,
};

#[test]
fn source_universe_execution_acceptance_reports_ready_and_blocked_universes_without_overclaiming() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let run_plan_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.json");
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
    assert_eq!(ledger.ready_for_conversion_universes, 1);
    assert_eq!(ledger.blocked_universes, 1);
    assert_eq!(ledger.total_planned_conversion_objects, 4);
    assert_eq!(ledger.total_required_single_object_operator_runs, 4);

    let binance = record(&ledger, "backfill-source-universe-binance-test");
    assert_eq!(
        binance.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::ReadyForConversionExecution
    );
    assert_eq!(binance.source_conversion_batch_count, 1);
    assert_eq!(binance.planned_conversion_objects, 2);
    assert_eq!(binance.remaining_conversion_objects, 2);
    assert!(binance.blocking_reasons.is_empty());

    let pmxt = record(&ledger, "backfill-source-universe-pmxt-test");
    assert_eq!(
        pmxt.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
    );
    assert_eq!(pmxt.planned_conversion_objects, 2);
    assert_eq!(pmxt.planned_source_bytes, 300);
    assert_eq!(pmxt.required_single_object_operator_runs, 2);
    assert!(
        pmxt.blocking_reasons
            .iter()
            .any(|reason| reason == "missing_source_universe_object_gates")
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
    assert_eq!(ledger.ready_for_conversion_universes, 2);
    assert_eq!(ledger.blocked_universes, 1);
    assert_eq!(ledger.total_planned_conversion_objects, 9_259);
    assert_eq!(ledger.total_required_single_object_operator_runs, 9_259);
    assert_eq!(ledger.total_remaining_conversion_objects, 9_259);

    let binance = record(
        &ledger,
        "backfill-source-universe-binance-data-vision-trades-2026-03-01-all-instruments",
    );
    assert_eq!(
        binance.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::ReadyForConversionExecution
    );
    assert_eq!(binance.source_gate_count, 2_051);
    assert_eq!(binance.source_conversion_batch_count, 8);
    assert_eq!(binance.planned_conversion_objects, 2_051);
    assert_eq!(binance.required_single_object_operator_runs, 2_051);
    assert!(binance.blocking_reasons.is_empty());

    let bybit = record(
        &ledger,
        "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
    );
    assert_eq!(
        bybit.status,
        SourceUniverseExecutionAcceptanceUniverseStatus::ReadyForConversionExecution
    );
    assert_eq!(bybit.source_gate_count, 5_857);
    assert_eq!(bybit.source_conversion_batch_count, 19);
    assert_eq!(bybit.planned_conversion_objects, 5_857);
    assert_eq!(bybit.required_single_object_operator_runs, 5_857);
    assert!(bybit.blocking_reasons.is_empty());

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
