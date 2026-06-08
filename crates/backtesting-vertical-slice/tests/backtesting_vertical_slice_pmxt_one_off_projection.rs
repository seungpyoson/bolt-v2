use backtesting_vertical_slice::{
    pmxt_one_off_backfill_projection::{
        PmxtBookLevel, PmxtOneOffProjectionRequest, PmxtOneOffSelectedRow, PmxtOneOffSnapshotRow,
        PmxtOneOffTickSide, PmxtPriceChangeRow, project_pmxt_one_off_rows_to_nt,
        write_pmxt_one_off_projection_to_catalog,
    },
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
