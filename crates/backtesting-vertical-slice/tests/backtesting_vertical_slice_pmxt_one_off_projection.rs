use arrow::{
    array::{ArrayRef, BinaryArray, Decimal128Array, StringArray, TimestampNanosecondArray},
    datatypes::{DataType, Field, Schema, TimeUnit},
    record_batch::RecordBatch,
};
use backtesting_vertical_slice::{
    pmxt_one_off_backfill_projection::{
        PmxtBookLevel, PmxtOneOffProjectionRequest, PmxtOneOffSelectedRow, PmxtOneOffSnapshotRow,
        PmxtOneOffTickSide, PmxtPriceChangeRow, PmxtSelectedSourceProjectionSpec,
        PmxtSelectedSourceSchema, project_pmxt_one_off_rows_to_nt,
        project_pmxt_selected_source_parquet_to_nt, write_pmxt_one_off_projection_to_catalog,
    },
    selected_source_slice::{SelectedSourceSliceReport, SelectedSourceSliceUsageScope},
    source_proof::SourceProofUsageScope,
};
use nautilus_backtest::{
    config::{BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType},
    node::BacktestNode,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::OrderBookDelta,
    enums::{AccountType, BookAction, BookType, OmsType, OrderSide},
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_polymarket::http::models::GammaMarket;
use parquet::arrow::ArrowWriter;
use sha2::{Digest, Sha256};
use std::{fs::File, sync::Arc};
use ustr::Ustr;

#[test]
fn pmxt_one_off_projection_uses_nt_polymarket_metadata_and_l2_parsers() {
    let projection = project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        rows: vec![
            PmxtOneOffSelectedRow::BookSnapshot(PmxtOneOffSnapshotRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                bids: vec![PmxtBookLevel {
                    price: "0.49".to_string(),
                    size: "10.000000".to_string(),
                }],
                asks: vec![PmxtBookLevel {
                    price: "0.50".to_string(),
                    size: "11.000000".to_string(),
                }],
                timestamp_ms: "1772023200123".to_string(),
                ts_init: UnixNanos::from(1_772_023_200_223_000_000),
            }),
            PmxtOneOffSelectedRow::PriceChange(PmxtPriceChangeRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                price: "0.49".to_string(),
                side: PmxtOneOffTickSide::Buy,
                size: "12.000000".to_string(),
                best_bid: Some("0.49".to_string()),
                best_ask: Some("0.50".to_string()),
                timestamp_ms: "1772023200456".to_string(),
                ts_init: UnixNanos::from(1_772_023_200_556_000_000),
            }),
        ],
    })
    .expect("project PMXT one-off rows");

    let instrument_id = match &projection.instrument {
        InstrumentAny::BinaryOption(instrument) => instrument.id(),
        other => panic!("expected BinaryOption instrument, got {other:?}"),
    };

    assert_eq!(
        projection.usage_scope,
        SourceProofUsageScope::OneOffBackfillData
    );
    assert_eq!(projection.source_binding, "synthetic-pmxt-one-off-source");
    assert_eq!(projection.order_book_deltas.len(), 4);
    assert!(projection.trade_ticks.is_empty());
    assert!(
        projection
            .nt_surfaces_used
            .contains(&"nautilus_polymarket::http::parse::create_instrument_from_def".to_string())
    );
    assert!(
        projection
            .nt_surfaces_used
            .contains(&"nautilus_polymarket::websocket::parse::parse_book_snapshot".to_string())
    );
    assert!(
        projection
            .nt_surfaces_used
            .contains(&"nautilus_polymarket::websocket::parse::parse_book_deltas".to_string())
    );

    assert_eq!(projection.order_book_deltas[0].instrument_id, instrument_id);
    assert_eq!(projection.order_book_deltas[0].action, BookAction::Clear);
    assert_eq!(projection.order_book_deltas[1].action, BookAction::Add);
    assert_eq!(projection.order_book_deltas[1].order.side, OrderSide::Buy);
    assert_eq!(projection.order_book_deltas[2].action, BookAction::Add);
    assert_eq!(projection.order_book_deltas[2].order.side, OrderSide::Sell);
    assert_eq!(projection.order_book_deltas[3].action, BookAction::Update);
    assert_eq!(projection.order_book_deltas[3].order.side, OrderSide::Buy);
    assert_eq!(
        projection.order_book_deltas[3].ts_event,
        UnixNanos::from(1_772_023_200_456_000_000)
    );
    assert_eq!(
        projection.order_book_deltas[3].ts_init,
        UnixNanos::from(1_772_023_200_556_000_000)
    );
}

#[test]
fn pmxt_one_off_projection_rejects_canonical_usage_scope() {
    let error = project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        rows: Vec::new(),
    })
    .expect_err("canonical scope must be rejected");

    assert!(
        error
            .to_string()
            .contains("only accepts one_off_backfill_data"),
        "{error}"
    );
}

#[test]
fn pmxt_one_off_projection_writes_nt_catalog_and_backtest_node_consumes_l2() {
    let projection = pmxt_projection_fixture();
    let instrument_id = binary_option_instrument_id(&projection.instrument);
    let (venue_name, settlement_currency) =
        binary_option_venue_and_currency(&projection.instrument);
    let catalog_dir = tempfile::TempDir::new().expect("catalog dir");
    let catalog_report = write_pmxt_one_off_projection_to_catalog(catalog_dir.path(), &projection)
        .expect("write PMXT one-off projection to catalog");

    assert_eq!(
        catalog_report.usage_scope,
        SourceProofUsageScope::OneOffBackfillData
    );
    assert_eq!(catalog_report.nt_instrument_id, instrument_id.to_string());
    assert_eq!(
        catalog_report.order_book_delta_count,
        projection.order_book_deltas.len() as u64
    );
    assert_eq!(catalog_report.trade_tick_count, 0);
    assert!(!catalog_report.catalog_hash.is_empty());

    let mut catalog = ParquetDataCatalog::new(catalog_dir.path(), None, None, None, None);
    let loaded: Vec<OrderBookDelta> = catalog
        .query_typed_data::<OrderBookDelta>(
            Some(vec![instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("read back PMXT L2 deltas");
    assert_eq!(loaded.len(), projection.order_book_deltas.len());

    let data_config = BacktestDataConfig::builder()
        .data_type(NautilusDataType::OrderBookDelta)
        .catalog_path(catalog_dir.path().to_str().expect("utf-8 path").to_string())
        .instrument_id(instrument_id)
        .build();
    let venue_config = BacktestVenueConfig::builder()
        .name(Ustr::from(venue_name.as_str()))
        .oms_type(OmsType::Netting)
        .account_type(AccountType::Cash)
        .book_type(BookType::L2_MBP)
        .starting_balances(vec![format!("1_000_000 {settlement_currency}")])
        .build();
    let run_config = BacktestRunConfig::builder()
        .id("pmxt-one-off-l2-catalog-proof".to_string())
        .venues(vec![venue_config])
        .data(vec![data_config])
        .build();

    let mut node = BacktestNode::new(vec![run_config]).expect("construct BacktestNode");
    node.build().expect("build BacktestNode");
    let results = node.run().expect("run BacktestNode");

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].iterations,
        projection.order_book_deltas.len(),
        "BacktestNode must consume the PMXT-projected L2 catalog rows"
    );
}

#[test]
fn pmxt_selected_source_parquet_projects_l2_rows_without_full_source_rescan() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selected_report_path = dir.path().join("selected-source-report.json");
    write_pmxt_selected_source_fixture(&selected_parquet_path);
    write_selected_source_report(&selected_report_path, &selected_parquet_path, 3);

    let selected = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        selected_source_parquet_path: selected_parquet_path.clone(),
        selected_source_report_path: selected_report_path.clone(),
        schema: PmxtSelectedSourceSchema {
            timestamp_received_column: "timestamp_received".to_string(),
            timestamp_column: "timestamp".to_string(),
            market_column: "market".to_string(),
            event_type_column: "event_type".to_string(),
            asset_id_column: "asset_id".to_string(),
            bids_column: "bids".to_string(),
            asks_column: "asks".to_string(),
            price_column: "price".to_string(),
            size_column: "size".to_string(),
            side_column: "side".to_string(),
            best_bid_column: "best_bid".to_string(),
            best_ask_column: "best_ask".to_string(),
            buy_side: "BUY".to_string(),
            sell_side: "SELL".to_string(),
            book_event_type: "book".to_string(),
            price_change_event_type: "price_change".to_string(),
            ignored_event_types: vec!["last_trade_price".to_string()],
        },
    })
    .expect("project selected-source parquet");

    assert_eq!(selected.selected_rows, 3);
    assert_eq!(selected.projected_l2_rows, 2);
    assert_eq!(selected.skipped_non_l2_rows, 1);
    assert_eq!(selected.selected_asset_ids_hash, "selected-assets-hash");
    assert_eq!(
        selected.projection.usage_scope,
        SourceProofUsageScope::OneOffBackfillData
    );
    assert_eq!(selected.projection.order_book_deltas.len(), 4);
    assert!(selected.projection.trade_ticks.is_empty());
    assert_eq!(
        selected.projection.order_book_deltas[3].ts_event,
        UnixNanos::from(1_772_023_200_456_000_000)
    );
    assert_eq!(
        selected.projection.order_book_deltas[3].ts_init,
        UnixNanos::from(1_772_023_200_556_000_000)
    );
}

fn gamma_markets() -> Vec<GammaMarket> {
    serde_json::from_str(
        r#"[{
  "id": "market-1",
  "conditionId": "0xcondition",
  "questionID": "0xquestion",
  "clobTokenIds": "[\"token-a\", \"token-b\"]",
  "outcomes": "[\"Yes\", \"No\"]",
  "question": "Synthetic source-backed question?",
  "description": "Synthetic description",
  "startDate": "2026-05-20T20:00:00Z",
  "endDate": "2026-05-21T20:00:00Z",
  "active": true,
  "closed": false,
  "acceptingOrders": true,
  "enableOrderBook": true,
  "orderPriceMinTickSize": 0.01,
  "orderMinSize": 5,
  "feeSchedule": {
    "exponent": 1,
    "rate": 0.03,
    "takerOnly": true,
    "rebateRate": 0
  }
}]"#,
    )
    .expect("Gamma fixture")
}

fn pmxt_projection_fixture()
-> backtesting_vertical_slice::pmxt_one_off_backfill_projection::PmxtOneOffNtProjection {
    project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        rows: vec![
            PmxtOneOffSelectedRow::BookSnapshot(PmxtOneOffSnapshotRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                bids: vec![PmxtBookLevel {
                    price: "0.49".to_string(),
                    size: "10.000000".to_string(),
                }],
                asks: vec![PmxtBookLevel {
                    price: "0.50".to_string(),
                    size: "11.000000".to_string(),
                }],
                timestamp_ms: "1772023200123".to_string(),
                ts_init: UnixNanos::from(1_772_023_200_223_000_000),
            }),
            PmxtOneOffSelectedRow::PriceChange(PmxtPriceChangeRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                price: "0.49".to_string(),
                side: PmxtOneOffTickSide::Buy,
                size: "12.000000".to_string(),
                best_bid: Some("0.49".to_string()),
                best_ask: Some("0.50".to_string()),
                timestamp_ms: "1772023200456".to_string(),
                ts_init: UnixNanos::from(1_772_023_200_556_000_000),
            }),
        ],
    })
    .expect("project PMXT one-off rows")
}

fn binary_option_instrument_id(instrument: &InstrumentAny) -> InstrumentId {
    match instrument {
        InstrumentAny::BinaryOption(instrument) => instrument.id(),
        other => panic!("expected BinaryOption instrument, got {other:?}"),
    }
}

fn binary_option_venue_and_currency(instrument: &InstrumentAny) -> (String, String) {
    match instrument {
        InstrumentAny::BinaryOption(instrument) => (
            instrument.id.venue.to_string(),
            instrument.currency.to_string(),
        ),
        other => panic!("expected BinaryOption instrument, got {other:?}"),
    }
}

fn write_pmxt_selected_source_fixture(path: &std::path::Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp_received",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Nanosecond, Some("+00:00".into())),
            false,
        ),
        Field::new("market", DataType::Binary, false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("asset_id", DataType::Utf8, false),
        Field::new("bids", DataType::Utf8, true),
        Field::new("asks", DataType::Utf8, true),
        Field::new("price", DataType::Decimal128(9, 4), true),
        Field::new("size", DataType::Decimal128(18, 6), true),
        Field::new("side", DataType::Utf8, true),
        Field::new("best_bid", DataType::Decimal128(9, 4), true),
        Field::new("best_ask", DataType::Decimal128(9, 4), true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_223_000_000,
                    1_772_023_200_556_000_000,
                    1_772_023_200_556_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_123_000_000,
                    1_772_023_200_456_000_000,
                    1_772_023_200_500_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(BinaryArray::from(vec![
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "price_change",
                "last_trade_price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec!["token-a", "token-a", "token-a"])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some(r#"[["0.49","10.000000"]]"#),
                None,
                None,
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some(r#"[["0.50","11.000000"]]"#),
                None,
                None,
            ])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(4900), Some(4900)])
                    .with_precision_and_scale(9, 4)
                    .expect("price decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(12_000_000), Some(2_000_000)])
                    .with_precision_and_scale(18, 6)
                    .expect("size decimal"),
            ) as ArrayRef,
            Arc::new(StringArray::from(vec![None, Some("BUY"), Some("SELL")])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(4900), None])
                    .with_precision_and_scale(9, 4)
                    .expect("best bid decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(5000), None])
                    .with_precision_and_scale(9, 4)
                    .expect("best ask decimal"),
            ) as ArrayRef,
        ],
    )
    .expect("selected-source batch");
    let file = File::create(path).expect("create selected source parquet");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("parquet writer");
    writer.write(&batch).expect("write selected source parquet");
    writer.close().expect("close selected source parquet");
}

fn write_selected_source_report(path: &std::path::Path, parquet_path: &std::path::Path, rows: u64) {
    let report = SelectedSourceSliceReport {
        schema_version: "selected-source-slice-report.v1".to_string(),
        source_parquet_path: "/source/full.parquet".to_string(),
        source_parquet_sha256: "source-sha".to_string(),
        selector_report_path: "/selector/report.json".to_string(),
        selector_report_sha256: "selector-sha".to_string(),
        output_parquet_path: parquet_path.display().to_string(),
        asset_id_column: "asset_id".to_string(),
        usage_scope: SelectedSourceSliceUsageScope::OneOffBackfillData,
        projected_columns: vec![
            "timestamp_received".to_string(),
            "timestamp".to_string(),
            "market".to_string(),
            "event_type".to_string(),
            "asset_id".to_string(),
            "bids".to_string(),
            "asks".to_string(),
            "price".to_string(),
            "size".to_string(),
            "side".to_string(),
            "best_bid".to_string(),
            "best_ask".to_string(),
        ],
        source_rows: rows,
        source_row_groups: 1,
        projected_row_groups: 1,
        selected_rows: rows,
        selected_asset_count: 1,
        selected_asset_ids_hash: "selected-assets-hash".to_string(),
        output_parquet_sha256: sha256_file(parquet_path),
    };
    let bytes = serde_json::to_vec_pretty(&report).expect("report json");
    std::fs::write(path, bytes).expect("write selected source report");
}

fn sha256_file(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).expect("read sha file");
    hex::encode(Sha256::digest(bytes))
}
