use crate::backtesting_vertical_slice_test_support::{
    PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH, generate_evicted_batch_plan,
    rewrite_assignment, tempdir_in_repo_target,
};
use backtesting_vertical_slice::{
    backfill_conversion_completion::{
        BackfillConversionCompletionLedger, BackfillConversionCompletionStatus,
        write_backfill_conversion_completion_ledger_from_spec_file,
    },
    reference_fixture_index::repo_root_from_manifest_dir,
};
use std::{fs, path::Path};

fn generate_completion_ledger_with_temp_batch_plan(
    reference_root: &Path,
    scope: &str,
    evicted_batch_plan_path: &str,
) -> BackfillConversionCompletionLedger {
    let temp_dir = tempdir_in_repo_target();
    let batch_root = reference_root.join(format!("backfill-conversion-batches/{scope}"));
    let batch_plan_path =
        generate_evicted_batch_plan(&batch_root, evicted_batch_plan_path, temp_dir.path());
    let repo_root = repo_root_from_manifest_dir();
    let batch_plan_path = batch_plan_path
        .strip_prefix(&repo_root)
        .unwrap_or(&batch_plan_path)
        .to_path_buf();

    let ledger_root =
        reference_root.join(format!("backfill-conversion-completion-ledgers/{scope}"));
    let ledger_spec_path = ledger_root.join("backfill-conversion-completion-ledger.toml");
    let temp_ledger_spec_path = temp_dir
        .path()
        .join("backfill-conversion-completion-ledger.toml");
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
        &temp_dir.path().join("completion-ledger"),
    );
    fs::write(&temp_ledger_spec_path, ledger_spec).expect("write temp completion ledger spec");

    let artifact =
        write_backfill_conversion_completion_ledger_from_spec_file(&temp_ledger_spec_path)
            .expect("completion ledger generation succeeds");
    serde_json::from_slice(&fs::read(&artifact.path).expect("read ledger")).expect("ledger parses")
}

#[test]
fn completion_ledger_proves_entire_binance_bnbusdc_venue_batch_is_published() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let ledger = generate_completion_ledger_with_temp_batch_plan(
        &reference_root,
        "binance-bnbusdc-2026-03-01-2026-05-31",
        PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    );

    assert_eq!(
        ledger.ledger_id,
        "backfill-conversion-completion-ledger-binance-bnbusdc-2026-03-01-2026-05-31"
    );
    assert_eq!(
        ledger.batch_id,
        "backfill-conversion-batch-binance-bnbusdc-2026-03-01-2026-05-31"
    );
    assert_eq!(ledger.status, BackfillConversionCompletionStatus::Ready);
    assert_eq!(ledger.record_count, 92);
    assert_eq!(ledger.published_records, 92);
    assert_eq!(ledger.mapping_proven_records, 92);
    assert_eq!(ledger.total_accepted_bytes, 66_451_476);
    assert_eq!(ledger.total_canonical_rows, 4_470_719);
    assert_eq!(ledger.total_nt_iterations, 4_470_719);
    assert!(ledger.blocking_issues.is_empty());
    assert!(
        ledger.records.iter().all(|record| {
            record.source_binding == "binance-spot-native-trades"
                && record.table_family == "trades"
                && record.nt_data_type == "TradeTick"
                && record.fidelity_class == "TRADE_REPLAY"
                && record.published_catalog_direct_s3
                && record.mapping_current_bte_status == "accepted"
                && record.mapping_parquet_catalog_status == "proven"
                && record.canonical_rows == record.catalog_read_back_trade_ticks
                && record.canonical_rows == record.published_catalog_nt_iterations
        }),
        "all venue records must be published, proven TradeTick catalog inputs"
    );
    assert_eq!(
        ledger
            .records
            .first()
            .map(|record| record.archive_date.as_str()),
        Some("2026-03-01")
    );
    assert_eq!(
        ledger
            .records
            .last()
            .map(|record| record.archive_date.as_str()),
        Some("2026-05-31")
    );
}

#[test]
fn completion_ledger_proves_entire_bybit_bnbusdc_venue_batch_is_published() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let ledger = generate_completion_ledger_with_temp_batch_plan(
        &reference_root,
        "bybit-bnbusdc-2026-03-01-2026-06-01",
        PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    );

    assert_eq!(
        ledger.ledger_id,
        "backfill-conversion-completion-ledger-bybit-bnbusdc-2026-03-01-2026-06-01"
    );
    assert_eq!(
        ledger.batch_id,
        "backfill-conversion-batch-bybit-bnbusdc-2026-03-01-2026-06-01"
    );
    assert_eq!(ledger.status, BackfillConversionCompletionStatus::Ready);
    assert_eq!(ledger.record_count, 93);
    assert_eq!(ledger.published_records, 93);
    assert_eq!(ledger.mapping_proven_records, 93);
    assert_eq!(ledger.total_accepted_bytes, 1_156_784);
    assert_eq!(ledger.total_canonical_rows, 131_635);
    assert_eq!(ledger.total_nt_iterations, 131_635);
    assert!(ledger.blocking_issues.is_empty());
    assert!(
        ledger.records.iter().all(|record| {
            record.source_binding == "bybit-spot-tick-trades"
                && record.table_family == "trades"
                && record.nt_data_type == "TradeTick"
                && record.fidelity_class == "TRADE_REPLAY"
                && record.published_catalog_direct_s3
                && record.mapping_current_bte_status == "accepted"
                && record.mapping_parquet_catalog_status == "proven"
                && record.canonical_rows == record.catalog_read_back_trade_ticks
                && record.canonical_rows == record.published_catalog_nt_iterations
        }),
        "all venue records must be published, proven TradeTick catalog inputs"
    );
    assert_eq!(
        ledger
            .records
            .first()
            .map(|record| record.archive_date.as_str()),
        Some("2026-03-01")
    );
    assert_eq!(
        ledger
            .records
            .last()
            .map(|record| record.archive_date.as_str()),
        Some("2026-06-01")
    );
}
