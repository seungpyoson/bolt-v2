use backtesting_vertical_slice::backfill_conversion_completion::{
    BackfillConversionCompletionLedger, BackfillConversionCompletionStatus,
    write_backfill_conversion_completion_ledger_from_spec_file,
};
use std::path::Path;

#[test]
fn completion_ledger_proves_entire_binance_bnbusdc_venue_batch_is_published() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let ledger_root = reference_root
        .join("backfill-conversion-completion-ledgers/binance-bnbusdc-2026-03-01-2026-05-31");
    let spec_path = ledger_root.join("backfill-conversion-completion-ledger.toml");
    let artifact = write_backfill_conversion_completion_ledger_from_spec_file(&spec_path)
        .expect("completion ledger generation succeeds");
    let ledger: BackfillConversionCompletionLedger =
        serde_json::from_slice(&std::fs::read(&artifact.path).expect("read ledger"))
            .expect("ledger parses");

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
    let ledger_root = reference_root
        .join("backfill-conversion-completion-ledgers/bybit-bnbusdc-2026-03-01-2026-06-01");
    let spec_path = ledger_root.join("backfill-conversion-completion-ledger.toml");
    let artifact = write_backfill_conversion_completion_ledger_from_spec_file(&spec_path)
        .expect("completion ledger generation succeeds");
    let ledger: BackfillConversionCompletionLedger =
        serde_json::from_slice(&std::fs::read(&artifact.path).expect("read ledger"))
            .expect("ledger parses");

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
