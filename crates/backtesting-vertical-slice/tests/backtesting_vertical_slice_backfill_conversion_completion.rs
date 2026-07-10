use crate::backtesting_vertical_slice_test_support::{
    BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH,
    BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
    PHASE3_BINANCE_BNBUSDC_CONVERSION_BATCH_PLAN_PATH,
    PHASE3_BYBIT_BNBUSDC_CONVERSION_BATCH_PLAN_PATH, generated_evicted_completion_ledger,
};
use backtesting_vertical_slice::backfill_conversion_completion::{
    BackfillConversionCompletionLedger, BackfillConversionCompletionStatus,
};
use std::path::Path;

fn generate_completion_ledger_with_temp_batch_plan(
    reference_root: &Path,
    scope: &str,
    evicted_batch_plan_path: &str,
) -> BackfillConversionCompletionLedger {
    let evicted_ledger_path = match scope {
        "binance-bnbusdc-2026-03-01-2026-05-31" => {
            BACKFILL_CONVERSION_COMPLETION_BINANCE_LEDGER_PATH
        }
        "bybit-bnbusdc-2026-03-01-2026-06-01" => BACKFILL_CONVERSION_COMPLETION_BYBIT_LEDGER_PATH,
        _ => panic!("unknown completion ledger scope {scope}"),
    };
    generated_evicted_completion_ledger(
        reference_root,
        scope,
        evicted_batch_plan_path,
        evicted_ledger_path,
    )
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
