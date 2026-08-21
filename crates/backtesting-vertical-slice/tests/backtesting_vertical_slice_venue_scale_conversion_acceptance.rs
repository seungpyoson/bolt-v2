use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::{
    BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
    BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
    PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH, assert_generated_fixture_bytes_match_index,
    assert_generated_fixture_matches_index, generate_evicted_completion_ledger,
    generate_evicted_pmxt_object_manifests, rewrite_assignment, tempdir_in_repo_target,
};
use backtesting_vertical_slice::source_universe_conversion_queue::write_source_universe_conversion_queue_from_spec_file;
use backtesting_vertical_slice::source_universe_conversion_run_plan::write_source_universe_conversion_run_plan_from_spec_file;
use backtesting_vertical_slice::source_universe_source_proofs::write_source_universe_source_proof_set_from_spec_file;
use backtesting_vertical_slice::venue_scale_conversion_acceptance::{
    VENUE_SCALE_CONVERSION_ACCEPTANCE_SCHEMA_VERSION, VenueScaleConversionAcceptanceLedger,
    VenueScaleConversionAcceptanceStatus,
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
completion_ledger_artifact_path = "specs/test-generated/venue-scale/binance-completion-ledger.json"

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
source_archive_discovery_seed_artifact_path = "specs/test-generated/venue-scale/binance-archive-seed.json"
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
completion_ledger_artifact_path = "specs/test-generated/venue-scale/bybit-completion-ledger.json"

[[venue.universe]]
universe_id = "bybit-public-archive-tick-trades-2025-06-01-2026-06-01"
scope_label = "Bybit public archive tick trades all staged categories"
status = "source_only"
source_universe_manifest_path = "{bybit_source_manifest}"
source_universe_conversion_queue_path = "{bybit_conversion_queue}"
source_universe_object_gates_path = "{bybit_object_gates}"
source_universe_conversion_run_plan_path = "{bybit_conversion_run_plan}"
source_universe_conversion_run_plan_artifact_path = "specs/test-generated/venue-scale/bybit-conversion-run-plan.json"

[[venue]]
venue_id = "pmxt-current-reference"
venue = "pmxt"

[[venue.universe]]
universe_id = "pmxt-polymarket-selected-source-2026-05-20"
scope_label = "Polymarket selected source one binary option"
status = "blocked"
selected_source_report_path = "{pmxt_selected_source_report}"
blocking_issues = [
  "historical_v1_delta_conversion_requires_v2_recertification",
]

[[venue.universe]]
universe_id = "pmxt-polymarket-full-current-data"
scope_label = "Polymarket full current local/archive data"
status = "blocked"
source_archive_discovery_seed_path = "{pmxt_archive_seed}"
source_archive_discovery_seed_artifact_path = "specs/test-generated/venue-scale/pmxt-archive-seed.json"
source_archive_index_manifest_path = "{pmxt_archive_index_manifest}"
source_archive_index_manifest_artifact_path = "specs/test-generated/venue-scale/pmxt-archive-index-manifest.json"
source_universe_manifest_path = "{pmxt_source_manifest}"
source_universe_manifest_artifact_path = "specs/test-generated/venue-scale/pmxt-source-manifest.json"
source_universe_conversion_queue_path = "{pmxt_conversion_queue}"
source_universe_conversion_queue_artifact_path = "specs/test-generated/venue-scale/pmxt-conversion-queue.json"
source_universe_source_proof_set_path = "{pmxt_source_proof_set}"
source_universe_source_proof_set_artifact_path = "specs/test-generated/venue-scale/pmxt-source-proof-set.json"
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
        ledger.schema_version,
        VENUE_SCALE_CONVERSION_ACCEPTANCE_SCHEMA_VERSION
    );
    assert_eq!(
        ledger.ledger_id,
        "venue-scale-conversion-acceptance-ledger-binance-bybit-pmxt-current"
    );
    assert_eq!(ledger.status, VenueScaleConversionAcceptanceStatus::Blocked);
    assert_eq!(ledger.venue_count, 3);
    assert_eq!(ledger.universe_count, 7);
    assert_eq!(ledger.converted_universes, 2);
    assert_eq!(ledger.source_only_universes, 2);
    assert_eq!(ledger.blocked_universes, 3);
    assert_eq!(ledger.total_converted_canonical_rows, 4_602_354);
    assert_eq!(ledger.total_converted_nt_catalog_rows, 4_602_354);
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
    assert_eq!(pmxt.converted_universes, 0);
    assert_eq!(pmxt.source_only_universes, 0);
    assert_eq!(pmxt.blocked_universes, 2);
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
    assert_eq!(
        pmxt_selected.status,
        VenueScaleConversionAcceptanceStatus::Blocked
    );
    assert_eq!(
        pmxt_selected.blocking_issues,
        vec!["historical_v1_delta_conversion_requires_v2_recertification"]
    );
    assert_eq!(pmxt_selected.converted_record_count, 0);
    assert_eq!(pmxt_selected.converted_canonical_rows, 0);
    assert_eq!(pmxt_selected.converted_nt_catalog_rows, 0);
}

#[test]
fn venue_scale_ledger_uses_stable_manifest_identity_across_materialization_roots() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let queue_spec = fs::read_to_string(reference_root.join(
        "source-universe-conversion-queues/pmxt-polymarket-v2-current/source-universe-conversion-queue.toml",
    ))
    .expect("read committed queue spec");
    let proof_spec = fs::read_to_string(reference_root.join(
        "backfill-source-proofs/pmxt-polymarket-v2-current/source-universe-source-proofs.toml",
    ))
    .expect("read committed proof spec");
    let venue_spec = fs::read_to_string(reference_root.join(
        "venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/venue-scale-conversion-acceptance-ledger.toml",
    ))
    .expect("read committed venue-scale spec");
    let mut generated = Vec::new();

    for root_name in ["first-root", "second-root-with-a-different-length"] {
        let temp_dir = tempdir_in_repo_target();
        let materialization_root = temp_dir.path().join(root_name);
        fs::create_dir_all(&materialization_root).expect("create materialization root");
        let binance_completion_root = materialization_root.join("binance-completion");
        let bybit_completion_root = materialization_root.join("bybit-completion");
        fs::create_dir_all(&binance_completion_root).expect("create binance completion root");
        fs::create_dir_all(&bybit_completion_root).expect("create bybit completion root");
        let binance_completion_ledger = generate_evicted_completion_ledger(
            &reference_root,
            "binance-bnbusdc-2026-03-01-2026-05-31",
            PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
            BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
            &binance_completion_root,
        );
        let bybit_completion_ledger = generate_evicted_completion_ledger(
            &reference_root,
            "bybit-bnbusdc-2026-03-01-2026-06-01",
            PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
            BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
            &bybit_completion_root,
        );
        let manifest_root = materialization_root.join("object-manifests");
        fs::create_dir_all(&manifest_root).expect("create object-manifest root");
        let (manifest_path, category_manifest_path) =
            generate_evicted_pmxt_object_manifests(&reference_root, &manifest_root);

        let queue_root = materialization_root.join("conversion-queue");
        fs::create_dir_all(&queue_root).expect("create queue root");
        let queue_spec_path = queue_root.join("source-universe-conversion-queue.toml");
        let rewritten_queue =
            rewrite_assignment(&queue_spec, "source_universe_manifest_path", &manifest_path);
        let rewritten_queue =
            rewrite_assignment(&rewritten_queue, "output_dir", &queue_root.join("output"));
        fs::write(&queue_spec_path, rewritten_queue).expect("write differential queue spec");
        let queue_artifact =
            write_source_universe_conversion_queue_from_spec_file(&queue_spec_path)
                .expect("generate differential queue");

        let proof_root = materialization_root.join("source-proofs");
        fs::create_dir_all(&proof_root).expect("create proof root");
        let proof_spec_path = proof_root.join("source-universe-source-proofs.toml");
        let rewritten_proof =
            rewrite_assignment(&proof_spec, "output_dir", &proof_root.join("output"));
        let rewritten_proof = rewrite_assignment(
            &rewritten_proof,
            "category_manifest_path",
            &category_manifest_path,
        );
        fs::write(&proof_spec_path, rewritten_proof).expect("write differential proof spec");
        let proof_artifact =
            write_source_universe_source_proof_set_from_spec_file(&proof_spec_path)
                .expect("generate differential proof set");

        let run_plan_root = materialization_root.join("bybit-run-plan");
        fs::create_dir_all(&run_plan_root).expect("create bybit run-plan root");
        let committed_run_plan_spec_path = reference_root.join(
            "source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/source-universe-conversion-run-plan.toml",
        );
        let committed_run_plan_spec = fs::read_to_string(&committed_run_plan_spec_path)
            .expect("read committed bybit run-plan spec");
        let run_plan_spec_path = run_plan_root.join("source-universe-conversion-run-plan.toml");
        fs::write(
            &run_plan_spec_path,
            rewrite_assignment(
                &committed_run_plan_spec,
                "output_dir",
                &run_plan_root.join("output"),
            ),
        )
        .expect("write differential bybit run-plan spec");
        let run_plan_artifact =
            write_source_universe_conversion_run_plan_from_spec_file(&run_plan_spec_path)
                .expect("generate differential bybit run plan");
        assert_generated_fixture_matches_index(
            "specs/023-nt-research-analytics-platform/reference/source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/run-plan/source-universe-conversion-run-plan.json",
            &run_plan_artifact.path,
        );

        let venue_root = materialization_root.join("venue-scale");
        fs::create_dir_all(&venue_root).expect("create venue-scale root");
        let venue_spec_path = venue_root.join("venue-scale-conversion-acceptance-ledger.toml");
        let mut rewritten_venue =
            rewrite_assignment(&venue_spec, "output_dir", &venue_root.join("output"));
        for (committed_materialization, actual_materialization) in [
            (
                "target/reference-regen/binance-bnbusdc-2026-03-01-2026-05-31/ledger/backfill-conversion-completion-ledger.json",
                binance_completion_ledger.as_path(),
            ),
            (
                "target/reference-regen/bybit-bnbusdc-2026-03-01-2026-06-01/ledger/backfill-conversion-completion-ledger.json",
                bybit_completion_ledger.as_path(),
            ),
            (
                "target/reference-regen/pmxt-polymarket-v2-current/manifest/source-universe-object-manifest.json",
                manifest_path.as_path(),
            ),
            (
                "target/reference-regen/pmxt-polymarket-v2-current/conversion-queue/source-universe-conversion-queue.json",
                queue_artifact.path.as_path(),
            ),
            (
                "target/reference-regen/pmxt-polymarket-v2-current/source-proofs/source-universe-source-proof-set.json",
                proof_artifact.path.as_path(),
            ),
            (
                "target/reference-regen/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/run-plan/source-universe-conversion-run-plan.json",
                run_plan_artifact.path.as_path(),
            ),
        ] {
            let needle = format!("= \"{committed_materialization}\"");
            let replacement = format!("= \"{}\"", actual_materialization.display());
            assert!(rewritten_venue.contains(&needle));
            rewritten_venue = rewritten_venue.replacen(&needle, &replacement, 1);
        }
        fs::write(&venue_spec_path, rewritten_venue).expect("write differential venue-scale spec");

        let artifact =
            write_venue_scale_conversion_acceptance_ledger_from_spec_file(&venue_spec_path)
                .expect("venue-scale generation succeeds");
        let bytes = fs::read(&artifact.path).expect("read generated venue-scale ledger");
        generated.push(bytes);
    }

    assert_eq!(generated[0], generated[1]);
    assert_generated_fixture_bytes_match_index(
        "specs/023-nt-research-analytics-platform/reference/venue-scale-conversion-acceptance-ledgers/binance-bybit-pmxt-current/ledger/venue-scale-conversion-acceptance-ledger.json",
        &generated[0],
    );
}

/// A selected conversion manifest is historical conversion evidence, not the
/// current venue-scale completion authority. Current `Converted` status must
/// come through the ready completion-ledger path so a stale logical catalog
/// hash cannot be silently re-certified by non-empty manifest fields.
#[test]
fn selected_conversion_manifest_cannot_grant_current_converted_status() {
    let temp_dir = tempdir_in_repo_target();
    let output_dir = temp_dir.path().join("acceptance-ledger");
    let manifest_path = temp_dir.path().join("historical-conversion-manifest.json");
    fs::write(
        &manifest_path,
        r#"{
  "canonical_rows": 42,
  "catalog_rows_by_nt_data_type": {"OrderBookDelta": 42},
  "catalog_hash": "0000000000000000000000000000000000000000000000000000000000000000",
  "output_catalog_uri": "file:///tmp/historical",
  "completed_at": "2026-06-10T14:30:00Z"
}"#,
    )
    .expect("write historical conversion manifest");
    let spec_path = temp_dir.path().join("selected-manifest-spec.toml");
    fs::write(
        &spec_path,
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
        ),
    )
    .expect("write selected-manifest spec");
    let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)
        .expect_err("historical selected manifest must not grant current converted status");
    assert!(
        format!("{error:#}").contains("selected_conversion_manifest_path"),
        "expected unknown selected-manifest authority rejection, got: {error:#}"
    );
}

#[test]
fn fabricated_completion_summary_cannot_grant_current_converted_status() {
    let temp_dir = tempdir_in_repo_target();
    let output_dir = temp_dir.path().join("acceptance-ledger");
    let completion_ledger_path = temp_dir.path().join("fabricated-completion-ledger.json");
    fs::write(
        &completion_ledger_path,
        r#"{
  "ledger_id": "fabricated-completion-ledger",
  "status": "ready",
  "record_count": 1,
  "total_canonical_rows": 1,
  "total_nt_iterations": 1
}"#,
    )
    .expect("write fabricated completion summary");
    let spec_path = temp_dir.path().join("fabricated-completion-spec.toml");
    fs::write(
        &spec_path,
        format!(
            r#"ledger_id = "venue-scale-conversion-acceptance-ledger-fabricated"
output_dir = "{output_dir}"

[[venue]]
venue_id = "fabricated-venue"
venue = "fabricated"

[[venue.universe]]
universe_id = "fabricated-universe"
scope_label = "fabricated completion authority"
status = "converted"
completion_ledger_path = "{completion_ledger_path}"
"#,
            output_dir = output_dir.display(),
            completion_ledger_path = completion_ledger_path.display(),
        ),
    )
    .expect("write fabricated completion spec");

    let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)
        .expect_err("a loose ready summary must not grant current converted authority");
    assert!(
        format!("{error:#}").contains("schema_version"),
        "expected typed completion-ledger schema rejection, got: {error:#}"
    );
}

#[test]
fn completion_ledger_with_inconsistent_totals_cannot_grant_converted_status() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let completion_root = temp_dir.path().join("completion-ledger");
    fs::create_dir_all(&completion_root).expect("create completion-ledger temp dir");
    let completion_ledger_path = generate_evicted_completion_ledger(
        &reference_root,
        "binance-bnbusdc-2026-03-01-2026-05-31",
        PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
        BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
        &completion_root,
    );
    let mut completion_ledger: serde_json::Value = serde_json::from_slice(
        &fs::read(&completion_ledger_path).expect("read generated completion ledger"),
    )
    .expect("parse generated completion ledger");
    let declared_rows = completion_ledger["total_canonical_rows"]
        .as_u64()
        .expect("completion ledger declares total_canonical_rows");
    completion_ledger["total_canonical_rows"] = serde_json::json!(declared_rows + 1);
    fs::write(
        &completion_ledger_path,
        serde_json::to_vec_pretty(&completion_ledger).expect("serialize inconsistent ledger"),
    )
    .expect("write inconsistent completion ledger");

    let output_dir = temp_dir.path().join("acceptance-ledger");
    let spec_path = temp_dir.path().join("inconsistent-completion-spec.toml");
    fs::write(
        &spec_path,
        format!(
            r#"ledger_id = "venue-scale-conversion-acceptance-ledger-inconsistent"
output_dir = "{output_dir}"

[[venue]]
venue_id = "inconsistent-venue"
venue = "inconsistent"

[[venue.universe]]
universe_id = "inconsistent-universe"
scope_label = "inconsistent completion authority"
status = "converted"
completion_ledger_path = "{completion_ledger_path}"
"#,
            output_dir = output_dir.display(),
            completion_ledger_path = completion_ledger_path.display(),
        ),
    )
    .expect("write inconsistent completion spec");

    let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)
        .expect_err("an internally inconsistent ready ledger must not grant converted authority");
    assert!(
        format!("{error:#}").contains("total_canonical_rows mismatch"),
        "expected completion total mismatch rejection, got: {error:#}"
    );
}

#[test]
fn completion_ledger_without_required_record_coverage_cannot_grant_converted_status() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let completion_root = temp_dir.path().join("completion-ledger");
    fs::create_dir_all(&completion_root).expect("create completion-ledger temp dir");
    let generated_completion_ledger = generate_evicted_completion_ledger(
        &reference_root,
        "binance-bnbusdc-2026-03-01-2026-05-31",
        PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
        BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
        &completion_root,
    );
    let completion_ledger: serde_json::Value = serde_json::from_slice(
        &fs::read(&generated_completion_ledger).expect("read generated completion ledger"),
    )
    .expect("parse generated completion ledger");

    let assert_rejected =
        |case: &str, ledger: &serde_json::Value, expected_error: &str| {
            let case_root = temp_dir.path().join(case);
            fs::create_dir_all(&case_root).expect("create coverage case directory");
            let completion_ledger_path = case_root.join("completion-ledger.json");
            fs::write(
                &completion_ledger_path,
                serde_json::to_vec_pretty(ledger).expect("serialize incomplete ledger"),
            )
            .expect("write incomplete completion ledger");
            let output_dir = case_root.join("acceptance-ledger");
            let spec_path = case_root.join("acceptance-spec.toml");
            fs::write(
                &spec_path,
                format!(
                    r#"ledger_id = "venue-scale-conversion-acceptance-ledger-{case}"
output_dir = "{output_dir}"

[[venue]]
venue_id = "{case}-venue"
venue = "{case}"

[[venue.universe]]
universe_id = "{case}-universe"
scope_label = "incomplete completion-ledger coverage"
status = "converted"
completion_ledger_path = "{completion_ledger_path}"
"#,
                    output_dir = output_dir.display(),
                    completion_ledger_path = completion_ledger_path.display(),
                ),
            )
            .expect("write coverage case spec");

            let error = write_venue_scale_conversion_acceptance_ledger_from_spec_file(&spec_path)
                .expect_err("incomplete ready-ledger coverage must not grant converted authority");
            assert!(
                format!("{error:#}").contains(expected_error),
                "expected {expected_error:?} rejection for {case}, got: {error:#}"
            );
        };

    let mut incomplete_mapping = completion_ledger.clone();
    let mapped = incomplete_mapping["mapping_proven_records"]
        .as_u64()
        .expect("completion ledger declares mapping_proven_records");
    incomplete_mapping["records"][0]["mapping_current_bte_status"] =
        serde_json::json!("not-current");
    incomplete_mapping["mapping_proven_records"] = serde_json::json!(mapped - 1);
    assert_rejected(
        "incomplete-mapping",
        &incomplete_mapping,
        "does not prove catalog mapping for every record",
    );

    let mut incomplete_publication = completion_ledger.clone();
    let published = incomplete_publication["published_records"]
        .as_u64()
        .expect("completion ledger declares published_records");
    incomplete_publication["records"][0]["published_catalog_direct_s3"] =
        serde_json::json!(false);
    incomplete_publication["published_records"] = serde_json::json!(published - 1);
    assert_rejected(
        "incomplete-publication",
        &incomplete_publication,
        "does not prove direct S3 publication for every record",
    );

    let mut empty_ledger_identity = completion_ledger.clone();
    empty_ledger_identity["ledger_id"] = serde_json::json!("   ");
    assert_rejected(
        "empty-ledger-identity",
        &empty_ledger_identity,
        "contains an empty ledger_id",
    );

    let mut empty_batch_identity = completion_ledger.clone();
    empty_batch_identity["batch_id"] = serde_json::json!("   ");
    assert_rejected(
        "empty-batch-identity",
        &empty_batch_identity,
        "contains an empty batch_id",
    );

    let mut empty_requirement_identity = completion_ledger.clone();
    empty_requirement_identity["requirements"]["nt_data_type"] = serde_json::json!("   ");
    for record in empty_requirement_identity["records"]
        .as_array_mut()
        .expect("completion ledger records are an array")
    {
        record["nt_data_type"] = serde_json::json!("   ");
    }
    assert_rejected(
        "empty-requirement-identity",
        &empty_requirement_identity,
        "contains empty requirement nt_data_type",
    );

    for (case, field) in [
        ("empty-archive-date", "archive_date"),
        ("empty-source-binding", "source_binding"),
        ("empty-table-family", "table_family"),
        ("empty-source-proof-id", "source_proof_id"),
        ("empty-operator-run-id", "operator_run_id"),
        ("empty-output-prefix", "output_prefix"),
        ("empty-published-catalog-uri", "published_catalog_uri"),
    ] {
        let mut empty_record_authority = completion_ledger.clone();
        empty_record_authority["records"][0][field] = serde_json::json!("   ");
        assert_rejected(
            case,
            &empty_record_authority,
            &format!("contains empty record {field}"),
        );
    }

    for (case, field) in [
        ("invalid-accepted-object-hash", "accepted_object_sha256"),
        (
            "invalid-publication-evidence-hash",
            "publication_evidence_hash",
        ),
        (
            "invalid-catalog-mapping-hash",
            "catalog_mapping_evaluation_hash",
        ),
        ("invalid-catalog-hash", "catalog_hash"),
    ] {
        let mut invalid_record_hash = completion_ledger.clone();
        invalid_record_hash["records"][0][field] = serde_json::json!("not-a-sha256");
        assert_rejected(
            case,
            &invalid_record_hash,
            &format!("contains invalid record {field}"),
        );
    }

    for (case, field) in [
        ("empty-publication-evidence-path", "publication_evidence_path"),
        (
            "empty-catalog-mapping-path",
            "catalog_mapping_evaluation_path",
        ),
    ] {
        let mut empty_record_path = completion_ledger.clone();
        empty_record_path["records"][0][field] = serde_json::json!("");
        assert_rejected(
            case,
            &empty_record_path,
            &format!("contains empty record {field}"),
        );
    }

    let mut zero_source_proof_version = completion_ledger.clone();
    zero_source_proof_version["records"][0]["source_proof_version"] = serde_json::json!(0);
    assert_rejected(
        "zero-source-proof-version",
        &zero_source_proof_version,
        "contains zero record source_proof_version",
    );

    let mut zero_accepted_bytes = completion_ledger.clone();
    let accepted_bytes = zero_accepted_bytes["records"][0]["accepted_bytes"]
        .as_u64()
        .expect("completion record declares accepted_bytes");
    let total_accepted_bytes = zero_accepted_bytes["total_accepted_bytes"]
        .as_u64()
        .expect("completion ledger declares total_accepted_bytes");
    zero_accepted_bytes["records"][0]["accepted_bytes"] = serde_json::json!(0);
    zero_accepted_bytes["total_accepted_bytes"] =
        serde_json::json!(total_accepted_bytes - accepted_bytes);
    assert_rejected(
        "zero-accepted-bytes",
        &zero_accepted_bytes,
        "contains zero record accepted_bytes",
    );

    let mut zero_canonical_rows = completion_ledger.clone();
    let canonical_rows = zero_canonical_rows["records"][0]["canonical_rows"]
        .as_u64()
        .expect("completion record declares canonical_rows");
    let total_canonical_rows = zero_canonical_rows["total_canonical_rows"]
        .as_u64()
        .expect("completion ledger declares total_canonical_rows");
    let total_nt_iterations = zero_canonical_rows["total_nt_iterations"]
        .as_u64()
        .expect("completion ledger declares total_nt_iterations");
    for field in [
        "canonical_rows",
        "catalog_read_back_trade_ticks",
        "published_catalog_expected_iterations",
        "published_catalog_nt_iterations",
    ] {
        zero_canonical_rows["records"][0][field] = serde_json::json!(0);
    }
    zero_canonical_rows["total_canonical_rows"] =
        serde_json::json!(total_canonical_rows - canonical_rows);
    zero_canonical_rows["total_nt_iterations"] =
        serde_json::json!(total_nt_iterations - canonical_rows);
    assert_rejected(
        "zero-canonical-rows",
        &zero_canonical_rows,
        "contains zero record canonical_rows",
    );

    let mut duplicate_record_identity = completion_ledger;
    let duplicate_record_id = duplicate_record_identity["records"][0]["record_id"].clone();
    duplicate_record_identity["records"][1]["record_id"] = duplicate_record_id;
    assert_rejected(
        "duplicate-record-identity",
        &duplicate_record_identity,
        "contains duplicate record_id",
    );
}

#[test]
fn source_proof_set_rejects_accepted_count_above_total_count() {
    let temp_dir = tempdir_in_repo_target();
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
source_universe_manifest_artifact_path = "specs/test-generated/venue-scale/source-manifest.json"
source_universe_source_proof_set_path = "{}"
source_universe_source_proof_set_artifact_path = "specs/test-generated/venue-scale/source-proof-set.json"
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
