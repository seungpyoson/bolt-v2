use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::{
    BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
    BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
    PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH, generate_evicted_completion_ledger,
    tempdir_in_repo_target,
};
use backtesting_vertical_slice::venue_scale_conversion_acceptance::{
    VenueScaleConversionAcceptanceLedger, VenueScaleConversionAcceptanceStatus,
    write_venue_scale_conversion_acceptance_ledger_from_spec_file,
};

#[test]
fn venue_scale_acceptance_ledger_reports_current_binance_bybit_pmxt_scope_without_overclaiming() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let output_dir = temp_dir.path().join("acceptance-ledger");
    let spec_path = temp_dir
        .path()
        .join("venue-scale-conversion-acceptance-ledger.toml");
    let binance_completion_ledger_dir = temp_dir.path().join("binance-completion-ledger");
    let bybit_completion_ledger_dir = temp_dir.path().join("bybit-completion-ledger");
    fs::create_dir_all(&binance_completion_ledger_dir).expect("create binance ledger temp dir");
    fs::create_dir_all(&bybit_completion_ledger_dir).expect("create bybit ledger temp dir");
    let binance_completion_ledger = generate_evicted_completion_ledger(
        &reference_root,
        "binance-bnbusdc-2026-03-01-2026-05-31",
        PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
        BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
        &binance_completion_ledger_dir,
    );
    let bybit_completion_ledger = generate_evicted_completion_ledger(
        &reference_root,
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
        BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
        &bybit_completion_ledger_dir,
    );
    let binance_archive_seed = temp_dir.path().join("binance-archive-discovery-seed.json");
    let bybit_conversion_run_plan = temp_dir.path().join("bybit-conversion-run-plan.json");
    let pmxt_archive_seed = temp_dir.path().join("pmxt-archive-discovery-seed.json");
    let pmxt_archive_index_manifest = temp_dir.path().join("pmxt-archive-index-manifest.json");
    let pmxt_source_manifest = temp_dir.path().join("pmxt-source-universe-manifest.json");
    let pmxt_conversion_queue = temp_dir.path().join("pmxt-conversion-queue.json");
    let pmxt_source_proof_set = temp_dir.path().join("pmxt-source-proof-set.json");

    fs::write(
        &binance_archive_seed,
        r#"{
  "schema_version": "source-archive-discovery-seed.v1",
  "discovery_id": "source-archive-discovery-seed-binance-data-vision-trades-current",
  "status": "ready",
  "venue": "binance",
  "source": "data-vision",
  "window_start": "2026-03-01",
  "window_end": "2026-03-01",
  "source_binding_count": 5,
  "representative_object_count": 5,
  "total_representative_object_bytes": 52653072,
  "product_families": ["coin_m_delivery", "coin_m_perpetual", "spot", "usd_m_delivery", "usd_m_perpetual"],
  "table_families": ["native_trades"],
  "bindings": []
}"#,
    )
    .expect("write binance discovery seed");
    fs::write(
        &bybit_conversion_run_plan,
        r#"{
  "schema_version": "source-universe-conversion-run-plan.v1",
  "plan_id": "source-universe-conversion-run-plan-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
  "status": "ready",
  "gate_id": "source-universe-object-gates-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
  "queue_id": "source-universe-conversion-queue-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
  "manifest_id": "backfill-source-universe-object-manifest-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
  "universe_id": "backfill-source-universe-bybit-public-archive-tick-trades-2025-06-01-2026-06-01",
  "venue": "bybit",
  "source": "public_archive",
  "family": "tick_trades",
  "table_family": "trades",
  "object_gates_path": "source-universe-object-gates.json",
  "object_gates_hash": "object-gates-hash",
  "max_objects_per_run": 500,
  "max_source_bytes_per_run": 2000000000,
  "source_binding_count": 3,
  "object_count": 5857,
  "planned_object_count": 5857,
  "total_source_bytes": 20309079098,
  "planned_source_bytes": 20309079098,
  "run_count": 19,
  "category_summaries": [],
  "artifact_refs": [],
  "runs": []
}"#,
    )
    .expect("write bybit conversion run plan");
    fs::write(
        &pmxt_archive_seed,
        r#"{
  "schema_version": "source-archive-discovery-seed.v1",
  "discovery_id": "source-archive-discovery-seed-pmxt-polymarket-v2-current",
  "status": "ready",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "window_start": "2026-05-20",
  "window_end": "2026-06-08",
  "source_binding_count": 1,
  "representative_object_count": 1,
  "total_representative_object_bytes": 361365244,
  "product_families": ["prediction_market_outcome"],
  "table_families": ["order_book_snapshot_deltas"],
  "bindings": []
}"#,
    )
    .expect("write pmxt discovery seed");
    fs::write(
        &pmxt_archive_index_manifest,
        r#"{
  "schema_version": "source-archive-index-manifest.v1",
  "manifest_id": "source-archive-index-manifest-pmxt-polymarket-v2-current",
  "status": "ready",
  "snapshot_id": "source-archive-index-snapshot-pmxt-polymarket-v2-current-2026-06-10T15",
  "fetched_at_utc": "2026-06-10T16:40:00Z",
  "venue": "pmxt",
  "source": "polymarket-v2-archive",
  "family": "orderbook",
  "table_family": "order_book_snapshot_deltas",
  "index_url": "https://archive.example.test/Polymarket/v2",
  "page_count": 28,
  "object_count": 1351,
  "verified_head_count": 1351,
  "total_content_length_bytes": 686000000000,
  "first_archive_hour_utc": "2026-04-13T19:00:00Z",
  "last_archive_hour_utc": "2026-06-10T15:00:00Z",
  "artifact_refs": [],
  "records": []
}"#,
    )
    .expect("write pmxt archive index manifest");
    fs::write(
        &pmxt_source_manifest,
        r#"{
  "schema_version": "backfill-source-universe-object-manifest.v1",
  "manifest_id": "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current",
  "universe_id": "backfill-source-universe-pmxt-polymarket-v2-current",
  "object_count": 1351,
  "accepted_bytes": 557815904970,
  "category_summaries": [
    {
      "category": "orderbook",
      "source_binding": "polymarket-parquet-archive-index",
      "instrument_count": 1,
      "object_count": 1351,
      "compressed_bytes": 557815904970
    }
  ]
}"#,
    )
    .expect("write pmxt source-universe manifest");
    fs::write(
        &pmxt_conversion_queue,
        r#"{
  "schema_version": "source-universe-conversion-queue.v1",
  "queue_id": "source-universe-conversion-queue-pmxt-polymarket-v2-current",
  "status": "ready",
  "manifest_id": "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current",
  "universe_id": "backfill-source-universe-pmxt-polymarket-v2-current",
  "work_item_count": 1351,
  "pending_conversion_items": 1351,
  "total_source_bytes": 557815904970
}"#,
    )
    .expect("write pmxt conversion queue");
    fs::write(
        &pmxt_source_proof_set,
        r#"{
  "schema_version": "source-universe-source-proof-set.v1",
  "proof_set_id": "source-universe-source-proofs-pmxt-polymarket-v2-current",
  "proof_count": 1,
  "accepted_proof_count": 0,
  "total_completed_objects": 1351,
  "total_accepted_bytes": 557815904970,
  "proofs": []
}"#,
    )
    .expect("write pmxt source proof set");

    fs::write(
        &spec_path,
        format!(
            r#"
ledger_id = "venue-scale-conversion-acceptance-ledger-binance-bybit-pmxt-current"
output_dir = "{output_dir}"

[[venue]]
venue_id = "binance-current-reference"
venue = "binance"

[[venue.universe]]
universe_id = "binance-bnbusdc-spot-2026-03-01-2026-05-31"
scope_label = "BNBUSDC spot daily trades"
status = "converted"
completion_ledger_path = "{binance_completion_ledger}"

[[venue.universe]]
universe_id = "binance-data-vision-trades-2026-03-01-all-instruments"
scope_label = "Binance Data Vision daily trades all instruments 2026-03-01"
status = "source_only"
source_universe_manifest_path = "{binance_source_manifest}"
source_universe_conversion_queue_path = "{binance_conversion_queue}"
source_universe_object_gates_path = "{binance_object_gates}"
source_universe_conversion_run_plan_path = "{binance_conversion_run_plan}"

[[venue.universe]]
universe_id = "binance-public-archive-full-current-data"
scope_label = "Binance public archive full current data"
status = "blocked"
source_archive_discovery_seed_path = "{binance_archive_seed}"
blocking_issues = [
  "missing_binance_full_source_universe_manifest",
  "missing_binance_full_source_universe_conversion_queue",
  "bnbusdc_slice_only_not_full_binance",
]

[[venue]]
venue_id = "bybit-current-reference"
venue = "bybit"

[[venue.universe]]
universe_id = "bybit-bnbusdc-spot-2026-03-01-2026-06-01"
scope_label = "BNBUSDC spot tick trades"
status = "converted"
completion_ledger_path = "{bybit_completion_ledger}"

[[venue.universe]]
universe_id = "bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
scope_label = "Bybit public archive tick trades all staged categories"
status = "source_only"
source_universe_manifest_path = "{bybit_source_manifest}"
source_universe_conversion_queue_path = "{bybit_conversion_queue}"
source_universe_object_gates_path = "{bybit_object_gates}"
source_universe_conversion_run_plan_path = "{bybit_conversion_run_plan}"

[[venue]]
venue_id = "pmxt-current-reference"
venue = "pmxt"

[[venue.universe]]
universe_id = "pmxt-polymarket-selected-source-2026-05-20"
scope_label = "Polymarket selected source one binary option"
status = "converted"
selected_conversion_manifest_path = "{pmxt_conversion_manifest}"
selected_source_report_path = "{pmxt_selected_source_report}"

[[venue.universe]]
universe_id = "pmxt-polymarket-full-current-data"
scope_label = "Polymarket full current local/archive data"
status = "blocked"
source_archive_discovery_seed_path = "{pmxt_archive_seed}"
source_archive_index_manifest_path = "{pmxt_archive_index_manifest}"
source_universe_manifest_path = "{pmxt_source_manifest}"
source_universe_conversion_queue_path = "{pmxt_conversion_queue}"
source_universe_source_proof_set_path = "{pmxt_source_proof_set}"
selected_source_report_path = "{pmxt_selected_source_report}"
blocking_issues = [
  "missing_accepted_source_proof",
  "missing_source_universe_object_gates",
  "missing_source_universe_conversion_run_plan",
  "missing_pmxt_l2_tick_size_epoch_policy",
]
"#,
            output_dir = output_dir.display(),
            binance_archive_seed = binance_archive_seed.display(),
            bybit_conversion_run_plan = bybit_conversion_run_plan.display(),
            pmxt_archive_seed = pmxt_archive_seed.display(),
            pmxt_archive_index_manifest = pmxt_archive_index_manifest.display(),
            pmxt_source_manifest = pmxt_source_manifest.display(),
            pmxt_conversion_queue = pmxt_conversion_queue.display(),
            pmxt_source_proof_set = pmxt_source_proof_set.display(),
            binance_completion_ledger = binance_completion_ledger.display(),
            binance_source_manifest = reference_root
                .join("backfill-source-universe-object-manifests/binance-data-vision-trades-2026-03-01-all-instruments/binance-data-vision-trades-object-manifest.json")
                .display(),
            binance_conversion_queue = reference_root
                .join("source-universe-conversion-queues/binance-data-vision-trades-2026-03-01-all-instruments/queue/source-universe-conversion-queue.json")
                .display(),
            binance_object_gates = reference_root
                .join("source-universe-object-gates/binance-data-vision-trades-2026-03-01-all-instruments/gates/source-universe-object-gates.json")
                .display(),
            binance_conversion_run_plan = reference_root
                .join("source-universe-conversion-run-plans/binance-data-vision-trades-2026-03-01-all-instruments/run-plan/source-universe-conversion-run-plan.json")
                .display(),
            bybit_completion_ledger = bybit_completion_ledger.display(),
            bybit_source_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json")
                .display(),
            bybit_conversion_queue = reference_root
                .join("source-universe-conversion-queues/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/queue/source-universe-conversion-queue.json")
                .display(),
            bybit_object_gates = reference_root
                .join("source-universe-object-gates/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/gates/source-universe-object-gates.json")
                .display(),
            pmxt_conversion_manifest = reference_root
                .join("pmxt-polymarket-selected-source-conversion/backtests/pmxt-run/conversion-manifest.json")
                .display(),
            pmxt_selected_source_report = reference_root
                .join("pmxt-polymarket-selected-source-conversion/selected-source/selected-source-report.json")
                .display(),
        ),
    )
    .expect("write spec");

    let artifact = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)
        .expect("venue scale acceptance ledger generation succeeds");
    let ledger: VenueScaleConversionAcceptanceLedger =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger"))
            .expect("ledger parses");

    assert_eq!(
        ledger.ledger_id,
        "venue-scale-conversion-acceptance-ledger-binance-bybit-pmxt-current"
    );
    assert_eq!(ledger.status, VenueScaleConversionAcceptanceStatus::Blocked);
    assert_eq!(ledger.venue_count, 3);
    assert_eq!(ledger.universe_count, 7);
    assert_eq!(ledger.converted_universes, 3);
    assert_eq!(ledger.source_only_universes, 2);
    assert_eq!(ledger.blocked_universes, 2);
    assert_eq!(ledger.total_converted_canonical_rows, 4_602_457);
    assert_eq!(ledger.total_converted_nt_catalog_rows, 4_602_458);
    assert_eq!(ledger.total_source_only_objects, 7_908);
    assert_eq!(ledger.total_source_only_object_gates, 7_908);
    assert_eq!(ledger.total_source_only_accepted_bytes, 22_057_801_068);

    let binance = ledger
        .venues
        .iter()
        .find(|venue| venue.venue == "binance")
        .expect("binance venue");
    assert_eq!(
        binance.status,
        VenueScaleConversionAcceptanceStatus::Blocked
    );
    assert_eq!(binance.converted_universes, 1);
    assert_eq!(binance.source_only_universes, 1);
    assert_eq!(binance.blocked_universes, 1);
    assert_eq!(binance.total_converted_canonical_rows, 4_470_719);
    assert_eq!(binance.total_source_only_objects, 2_051);
    assert_eq!(binance.total_source_only_object_gates, 2_051);
    assert_eq!(binance.total_source_only_accepted_bytes, 1_748_721_970);
    let binance_source_only = binance
        .universes
        .iter()
        .find(|universe| {
            universe.universe_id == "binance-data-vision-trades-2026-03-01-all-instruments"
        })
        .expect("binance source-only universe");
    assert_eq!(
        binance_source_only.status,
        VenueScaleConversionAcceptanceStatus::SourceOnly
    );
    assert_eq!(
        binance_source_only.source_manifest_id.as_deref(),
        Some(
            "backfill-source-universe-object-manifest-binance-data-vision-trades-2026-03-01-all-instruments"
        )
    );
    assert_eq!(
        binance_source_only.source_conversion_queue_id.as_deref(),
        Some(
            "source-universe-conversion-queue-binance-data-vision-trades-2026-03-01-all-instruments"
        )
    );
    assert_eq!(binance_source_only.source_object_count, 2_051);
    assert_eq!(binance_source_only.source_accepted_bytes, 1_748_721_970);
    assert_eq!(
        binance_source_only.source_conversion_queue_work_item_count,
        2_051
    );
    assert_eq!(
        binance_source_only.source_conversion_queue_pending_items,
        2_051
    );
    assert_eq!(
        binance_source_only.source_conversion_queue_total_bytes,
        1_748_721_970
    );
    assert_eq!(
        binance_source_only.source_object_gate_id.as_deref(),
        Some("source-universe-object-gates-binance-data-vision-trades-2026-03-01-all-instruments")
    );
    assert_eq!(binance_source_only.source_object_gate_count, 2_051);
    assert_eq!(
        binance_source_only.source_object_gate_source_binding_count,
        5
    );
    assert_eq!(
        binance_source_only.source_conversion_run_plan_id.as_deref(),
        Some(
            "source-universe-conversion-run-plan-binance-data-vision-trades-2026-03-01-all-instruments"
        )
    );
    assert_eq!(binance_source_only.source_conversion_run_count, 8);
    assert_eq!(
        binance_source_only.source_conversion_run_object_count,
        2_051
    );
    assert_eq!(
        binance_source_only.source_conversion_run_planned_bytes,
        1_748_721_970
    );
    assert!(
        binance_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_manifest")
    );
    assert!(
        binance_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_conversion_queue")
    );
    assert!(
        binance_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_object_gates")
    );
    assert!(
        binance_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_conversion_run_plan")
    );
    let binance_full = binance
        .universes
        .iter()
        .find(|universe| universe.universe_id == "binance-public-archive-full-current-data")
        .expect("binance full current universe");
    assert_eq!(
        binance_full.status,
        VenueScaleConversionAcceptanceStatus::Blocked
    );
    assert_eq!(
        binance_full.blocking_issues,
        vec![
            "missing_binance_full_source_universe_manifest",
            "missing_binance_full_source_universe_conversion_queue",
            "bnbusdc_slice_only_not_full_binance",
        ]
    );
    assert_eq!(
        binance_full.source_archive_discovery_seed_id.as_deref(),
        Some("source-archive-discovery-seed-binance-data-vision-trades-current")
    );
    assert_eq!(
        binance_full.source_archive_discovery_seed_source_binding_count,
        5
    );
    assert_eq!(
        binance_full.source_archive_discovery_seed_representative_object_count,
        5
    );
    assert!(
        binance_full
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_archive_discovery_seed")
    );

    let bybit = ledger
        .venues
        .iter()
        .find(|venue| venue.venue == "bybit")
        .expect("bybit venue");
    assert_eq!(
        bybit.status,
        VenueScaleConversionAcceptanceStatus::PartiallyConverted
    );
    assert_eq!(bybit.source_only_universes, 1);
    assert_eq!(bybit.total_source_only_objects, 5_857);
    assert_eq!(bybit.total_source_only_object_gates, 5_857);
    let bybit_source_only = bybit
        .universes
        .iter()
        .find(|universe| {
            universe.universe_id == "bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
        })
        .expect("bybit source-only universe");
    assert_eq!(
        bybit_source_only.source_object_gate_id.as_deref(),
        Some("source-universe-object-gates-bybit-public-archive-tick-trades-2025-06-01-2026-06-01")
    );
    assert_eq!(
        bybit_source_only.source_conversion_queue_id.as_deref(),
        Some(
            "source-universe-conversion-queue-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
        )
    );
    assert_eq!(
        bybit_source_only.source_conversion_queue_work_item_count,
        5_857
    );
    assert_eq!(
        bybit_source_only.source_conversion_queue_pending_items,
        5_857
    );
    assert_eq!(
        bybit_source_only.source_conversion_queue_total_bytes,
        20_309_079_098
    );
    assert_eq!(bybit_source_only.source_object_gate_count, 5_857);
    assert_eq!(bybit_source_only.source_object_gate_source_binding_count, 3);
    assert_eq!(
        bybit_source_only.source_conversion_run_plan_id.as_deref(),
        Some(
            "source-universe-conversion-run-plan-bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
        )
    );
    assert_eq!(bybit_source_only.source_conversion_run_count, 19);
    assert_eq!(bybit_source_only.source_conversion_run_object_count, 5_857);
    assert_eq!(
        bybit_source_only.source_conversion_run_planned_bytes,
        20_309_079_098
    );
    assert!(
        bybit_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_conversion_queue")
    );
    assert!(
        bybit_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_object_gates")
    );
    assert!(
        bybit_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_conversion_run_plan")
    );

    let pmxt = ledger
        .venues
        .iter()
        .find(|venue| venue.venue == "pmxt")
        .expect("pmxt venue");
    assert_eq!(pmxt.status, VenueScaleConversionAcceptanceStatus::Blocked);
    assert_eq!(pmxt.converted_universes, 1);
    assert_eq!(pmxt.source_only_universes, 0);
    assert_eq!(pmxt.blocked_universes, 1);
    assert_eq!(pmxt.total_source_only_objects, 0);
    assert_eq!(pmxt.total_source_only_object_gates, 0);
    assert_eq!(pmxt.total_source_only_accepted_bytes, 0);
    let pmxt_full = pmxt
        .universes
        .iter()
        .find(|universe| universe.universe_id == "pmxt-polymarket-full-current-data")
        .expect("pmxt full current universe");
    assert_eq!(
        pmxt_full.status,
        VenueScaleConversionAcceptanceStatus::Blocked
    );
    assert_eq!(
        pmxt_full.blocking_issues,
        vec![
            "missing_accepted_source_proof",
            "missing_source_universe_object_gates",
            "missing_source_universe_conversion_run_plan",
            "missing_pmxt_l2_tick_size_epoch_policy",
        ]
    );
    assert_eq!(
        pmxt_full.source_archive_discovery_seed_id.as_deref(),
        Some("source-archive-discovery-seed-pmxt-polymarket-v2-current")
    );
    assert_eq!(
        pmxt_full.source_archive_discovery_seed_source_binding_count,
        1
    );
    assert_eq!(
        pmxt_full.source_archive_discovery_seed_representative_object_count,
        1
    );
    assert_eq!(
        pmxt_full.source_archive_index_manifest_id.as_deref(),
        Some("source-archive-index-manifest-pmxt-polymarket-v2-current")
    );
    assert_eq!(
        pmxt_full.source_archive_index_snapshot_id.as_deref(),
        Some("source-archive-index-snapshot-pmxt-polymarket-v2-current-2026-06-10T15")
    );
    assert_eq!(pmxt_full.source_archive_index_object_count, 1_351);
    assert_eq!(pmxt_full.source_archive_index_verified_head_count, 1_351);
    assert_eq!(
        pmxt_full.source_archive_index_total_content_length_bytes,
        686_000_000_000
    );
    assert!(
        pmxt_full
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_archive_index_manifest")
    );
    assert_eq!(
        pmxt_full.source_manifest_id.as_deref(),
        Some("backfill-source-universe-object-manifest-pmxt-polymarket-v2-current")
    );
    assert_eq!(
        pmxt_full.source_conversion_queue_id.as_deref(),
        Some("source-universe-conversion-queue-pmxt-polymarket-v2-current")
    );
    assert_eq!(pmxt_full.source_object_count, 1_351);
    assert_eq!(pmxt_full.source_accepted_bytes, 557_815_904_970);
    assert_eq!(pmxt_full.source_conversion_queue_work_item_count, 1_351);
    assert_eq!(pmxt_full.source_conversion_queue_pending_items, 1_351);
    assert_eq!(
        pmxt_full.source_conversion_queue_total_bytes,
        557_815_904_970
    );
    let pmxt_full_value = serde_json::to_value(pmxt_full).expect("serialize pmxt full universe");
    assert_eq!(
        pmxt_full_value["source_proof_set_id"],
        "source-universe-source-proofs-pmxt-polymarket-v2-current"
    );
    assert_eq!(pmxt_full_value["source_proof_count"], 1);
    assert_eq!(pmxt_full_value["source_accepted_proof_count"], 0);
    assert!(
        pmxt_full
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_manifest")
    );
    assert!(
        pmxt_full
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_conversion_queue")
    );
    assert!(
        pmxt_full
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_source_proof_set")
    );

    let pmxt_selected = pmxt
        .universes
        .iter()
        .find(|universe| universe.universe_id == "pmxt-polymarket-selected-source-2026-05-20")
        .expect("pmxt selected universe");
    assert_eq!(pmxt_selected.converted_canonical_rows, 103);
    assert_eq!(
        pmxt_selected
            .catalog_rows_by_nt_data_type
            .get("OrderBookDelta"),
        Some(&103)
    );
    assert_eq!(
        pmxt_selected.catalog_rows_by_nt_data_type.get("TradeTick"),
        Some(&1)
    );
}

#[test]
fn venue_scale_ledger_uses_stable_manifest_identity_across_materialization_roots() {
    let stable_manifest_identity = "specs/023-nt-research-analytics-platform/reference/backfill-source-universe-object-manifests/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json";
    let manifest = r#"{
  "schema_version": "backfill-source-universe-object-manifest.v1",
  "manifest_id": "backfill-source-universe-object-manifest-pmxt-polymarket-v2-current",
  "universe_id": "backfill-source-universe-pmxt-polymarket-v2-current",
  "object_count": 1351,
  "accepted_bytes": 557815904970,
  "category_summaries": []
}"#;
    let mut generated = Vec::new();

    for root_name in ["first-root", "second-root-with-a-different-length"] {
        let temp_dir = tempdir_in_repo_target();
        let materialization_root = temp_dir.path().join(root_name);
        let manifest_path = materialization_root.join("source-universe-object-manifest.json");
        let output_dir = materialization_root.join("ledger");
        let spec_path = materialization_root.join("venue-scale-ledger.toml");
        fs::create_dir_all(&materialization_root).expect("create materialization root");
        fs::write(&manifest_path, manifest).expect("write materialized manifest");
        fs::write(
            &spec_path,
            format!(
                r#"
ledger_id = "venue-scale-conversion-acceptance-ledger-pmxt-location-independent"
output_dir = "{output_dir}"

[[venue]]
venue_id = "pmxt-current-reference"
venue = "pmxt"

[[venue.universe]]
universe_id = "pmxt-polymarket-full-current-data"
scope_label = "Polymarket full current local/archive data"
status = "blocked"
source_universe_manifest_path = "{manifest_path}"
source_universe_manifest_artifact_path = "{stable_manifest_identity}"
blocking_issues = ["missing_accepted_source_proof"]
"#,
                output_dir = output_dir.display(),
                manifest_path = manifest_path.display(),
            ),
        )
        .expect("write differential venue-scale spec");

        let artifact = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)
            .expect("venue-scale generation succeeds");
        let bytes = fs::read(&artifact.path).expect("read generated venue-scale ledger");
        let ledger: VenueScaleConversionAcceptanceLedger =
            serde_json::from_slice(&bytes).expect("venue-scale ledger parses");
        let manifest_ref = ledger.venues[0].universes[0]
            .artifact_refs
            .iter()
            .find(|artifact_ref| artifact_ref.role == "source_universe_manifest")
            .expect("manifest artifact ref");
        assert_eq!(manifest_ref.path, Path::new(stable_manifest_identity));
        generated.push(bytes);
    }

    assert_eq!(generated[0], generated[1]);
}

/// Regression for the selected-conversion-manifest completion-proof gate: a
/// manifest that parses but does not attest a *finalized* conversion (zero
/// canonical rows, or a blank catalog hash) must be rejected. Without the gate,
/// such a stub satisfied the Converted coverage requirement through the
/// planned == 0 path on parseability alone — the same false-positive class the
/// ledger path closes via `status == "ready"`.
#[test]
fn converted_universe_via_selected_manifest_rejects_unfinalized_manifest() {
    fn spec_for(manifest_path: &Path, output_dir: &Path) -> String {
        format!(
            r#"ledger_id = "venue-scale-conversion-acceptance-ledger-degenerate"
output_dir = "{output_dir}"

[[venue]]
venue_id = "pmxt-current-reference"
venue = "pmxt"

[[venue.universe]]
universe_id = "pmxt-degenerate-converted"
scope_label = "degenerate converted universe"
status = "converted"
selected_conversion_manifest_path = "{manifest_path}"
"#,
            output_dir = output_dir.display(),
            manifest_path = manifest_path.display(),
        )
    }

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("acceptance-ledger");

    // Zero canonical rows: parses (all required fields present) but is not a
    // finalized conversion.
    let zero_rows_manifest = temp_dir.path().join("zero-rows-manifest.json");
    fs::write(
        &zero_rows_manifest,
        r#"{
  "canonical_rows": 0,
  "catalog_rows_by_nt_data_type": {},
  "catalog_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "output_catalog_uri": "file:///tmp/none",
  "completed_at": "2026-06-10T14:30:00Z"
}"#,
    )
    .expect("write zero-rows manifest");
    let zero_spec = temp_dir.path().join("zero-rows-spec.toml");
    fs::write(&zero_spec, spec_for(&zero_rows_manifest, &output_dir)).expect("write zero spec");
    let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&zero_spec)
        .expect_err("zero-canonical-row manifest must be rejected as a completion proof");
    assert!(
        format!("{error:#}").contains("zero canonical rows"),
        "expected zero-canonical-rows rejection, got: {error:#}"
    );

    // Blank catalog hash: a manifest with rows but no catalog hash never wrote a
    // catalog and is not a finalized conversion.
    let blank_hash_manifest = temp_dir.path().join("blank-hash-manifest.json");
    fs::write(
        &blank_hash_manifest,
        r#"{
  "canonical_rows": 42,
  "catalog_rows_by_nt_data_type": {"TradeTick": 42},
  "catalog_hash": "   ",
  "output_catalog_uri": "file:///tmp/none",
  "completed_at": "2026-06-10T14:30:00Z"
}"#,
    )
    .expect("write blank-hash manifest");
    let blank_spec = temp_dir.path().join("blank-hash-spec.toml");
    fs::write(&blank_spec, spec_for(&blank_hash_manifest, &output_dir)).expect("write blank spec");
    let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&blank_spec)
        .expect_err("blank-catalog-hash manifest must be rejected as a completion proof");
    assert!(
        format!("{error:#}").contains("catalog hash"),
        "expected missing-catalog-hash rejection, got: {error:#}"
    );
}

#[test]
fn source_proof_set_rejects_accepted_count_above_total_count() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("acceptance-ledger");
    let source_manifest = temp_dir.path().join("source-manifest.json");
    let source_proof_set = temp_dir.path().join("source-proof-set.json");
    fs::write(
        &source_manifest,
        r#"{
  "schema_version": "backfill-source-universe-object-manifest.v1",
  "manifest_id": "backfill-source-universe-object-manifest-test",
  "universe_id": "backfill-source-universe-test",
  "object_count": 1,
  "accepted_bytes": 10
}"#,
    )
    .expect("write source manifest");
    fs::write(
        &source_proof_set,
        r#"{
  "schema_version": "source-universe-source-proof-set.v1",
  "proof_set_id": "source-universe-source-proofs-test",
  "proof_count": 1,
  "accepted_proof_count": 2,
  "total_completed_objects": 1,
  "total_accepted_bytes": 10
}"#,
    )
    .expect("write source proof set");
    let spec = temp_dir.path().join("source-proof-set-spec.toml");
    fs::write(
        &spec,
        format!(
            r#"ledger_id = "venue-scale-conversion-acceptance-ledger-inconsistent-proof-set"
output_dir = "{}"

[[venue]]
venue_id = "test-current-reference"
venue = "test"

[[venue.universe]]
universe_id = "test-source-only"
scope_label = "test source-only universe"
status = "source_only"
source_universe_manifest_path = "{}"
source_universe_source_proof_set_path = "{}"
"#,
            output_dir.display(),
            source_manifest.display(),
            source_proof_set.display(),
        ),
    )
    .expect("write spec");
    let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec)
        .expect_err("accepted proof count above total proof count must be rejected");
    assert!(
        format!("{error:#}").contains("accepted proof count exceeds proof count"),
        "expected proof-count consistency rejection, got: {error:#}"
    );
}
