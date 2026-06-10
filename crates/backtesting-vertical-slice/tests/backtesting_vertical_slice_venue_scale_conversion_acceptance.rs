use std::{fs, path::Path};

use backtesting_vertical_slice::venue_scale_conversion_acceptance::{
    VenueScaleConversionAcceptanceLedger, VenueScaleConversionAcceptanceStatus,
    write_venue_scale_conversion_acceptance_ledger_from_spec_file,
};

#[test]
fn venue_scale_acceptance_ledger_reports_current_binance_bybit_pmxt_scope_without_overclaiming() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_dir = temp_dir.path().join("acceptance-ledger");
    let spec_path = temp_dir
        .path()
        .join("venue-scale-conversion-acceptance-ledger.toml");

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
source_universe_object_gates_path = "{bybit_object_gates}"

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
selected_source_report_path = "{pmxt_selected_source_report}"
blocking_issues = [
  "missing_accepted_pmxt_source_universe",
  "missing_full_file_tick_size_epoch_policy",
  "selected_source_only_not_full_pmxt",
]
"#,
            output_dir = output_dir.display(),
            binance_completion_ledger = reference_root
                .join("backfill-conversion-completion-ledgers/binance-bnbusdc-2026-03-01-2026-05-31/ledger/backfill-conversion-completion-ledger.json")
                .display(),
            bybit_completion_ledger = reference_root
                .join("backfill-conversion-completion-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01/ledger/backfill-conversion-completion-ledger.json")
                .display(),
            bybit_source_manifest = reference_root
                .join("backfill-source-universe-object-manifests/bybit-public-archive-tick-trades-2025-06-01-2026-06-01/bybit-public-archive-tick-trades-object-manifest.json")
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
    assert_eq!(ledger.universe_count, 5);
    assert_eq!(ledger.converted_universes, 3);
    assert_eq!(ledger.source_only_universes, 1);
    assert_eq!(ledger.blocked_universes, 1);
    assert_eq!(ledger.total_converted_canonical_rows, 4_602_457);
    assert_eq!(ledger.total_converted_nt_catalog_rows, 4_602_458);
    assert_eq!(ledger.total_source_only_objects, 5_857);
    assert_eq!(ledger.total_source_only_object_gates, 5_857);
    assert_eq!(ledger.total_source_only_accepted_bytes, 20_309_079_098);

    let binance = ledger
        .venues
        .iter()
        .find(|venue| venue.venue == "binance")
        .expect("binance venue");
    assert_eq!(
        binance.status,
        VenueScaleConversionAcceptanceStatus::Converted
    );
    assert_eq!(binance.total_converted_canonical_rows, 4_470_719);

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
    assert_eq!(bybit_source_only.source_object_gate_count, 5_857);
    assert_eq!(bybit_source_only.source_object_gate_source_binding_count, 3);
    assert!(
        bybit_source_only
            .artifact_refs
            .iter()
            .any(|artifact| artifact.role == "source_universe_object_gates")
    );

    let pmxt = ledger
        .venues
        .iter()
        .find(|venue| venue.venue == "pmxt")
        .expect("pmxt venue");
    assert_eq!(pmxt.status, VenueScaleConversionAcceptanceStatus::Blocked);
    assert_eq!(pmxt.converted_universes, 1);
    assert_eq!(pmxt.blocked_universes, 1);

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
