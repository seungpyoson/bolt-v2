use arrow::{
    array::{
        ArrayRef, BinaryArray, BinaryViewArray, Decimal128Array, FixedSizeBinaryArray,
        Float64Array, StringArray, TimestampNanosecondArray,
    },
    datatypes::{DataType, Field, Schema, TimeUnit},
    record_batch::RecordBatch,
};
use backtesting_vertical_slice::{
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        ConversionFingerprint, ConversionOutputState, inspect_conversion_output,
    },
    pmxt_one_off_backfill_projection::{
        NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_QUOTE_TICK, NT_DATA_TYPE_TRADE_TICK,
        PMXT_ONE_OFF_RESULT_CONTRACT_FILE, PmxtBookLevel, PmxtOneOffArtifactRootRunSpec,
        PmxtOneOffBacktestContractSpec, PmxtOneOffConversionProjectionSpec, PmxtOneOffNtProjection,
        PmxtOneOffProjectionRequest, PmxtOneOffSelectedRow, PmxtOneOffSnapshotRow,
        PmxtOneOffTickSide, PmxtOneOffTradeRow, PmxtPriceChangeRow,
        PmxtSelectedSourceProjectionSpec, PmxtSelectedSourceSchema,
        project_pmxt_one_off_rows_to_nt, project_pmxt_selected_source_parquet_to_nt,
        run_pmxt_one_off_l2_backtest_contract, write_pmxt_one_off_conversion_projection,
        write_pmxt_one_off_l2_artifact_root_run, write_pmxt_one_off_projection_to_catalog,
    },
    reference_fixture_index::repo_root_from_manifest_dir,
    result_contract::BacktestResultContract,
    result_contract::ResultArtifactUris,
    run_manifest::{
        BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION, BacktestingRunManifest, CATALOG_FS_PROTOCOL_NONE,
        ManifestArtifactStore, ManifestCatalogInput, ManifestVenueConfig, MarketStructureFixture,
        RunPurpose, STRATEGY_HURST_VPIN_DIRECTIONAL, STRATEGY_PARAM_BAR_TYPE,
        STRATEGY_PARAM_TRADE_SIZE, StrategySource, StrategySourceKind,
    },
    selected_source_slice::{SelectedSourceSliceReport, SelectedSourceSliceUsageScope},
    source_proof::{AcceptanceMode, SourceProofFidelityClass, SourceProofUsageScope},
};
use nautilus_backtest::{
    config::{BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType},
    node::BacktestNode,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, QuoteTick, TradeTick},
    enums::{AccountType, BookAction, BookType, OmsType, OrderSide},
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_polymarket::http::models::GammaMarket;
use parquet::arrow::ArrowWriter;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, path::PathBuf, process::Command, sync::Arc};
use ustr::Ustr;

const PMXT_TEST_EVENT_COUNT_LEDGER_HASH: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";
const PMXT_TEST_SELECTED_ASSET_IDS_HASH: &str =
    "2222222222222222222222222222222222222222222222222222222222222222";

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
    assert_eq!(projection.quote_ticks.len(), 1);
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
    assert!(projection.nt_surfaces_used.contains(
        &"nautilus_polymarket::websocket::parse::parse_quote_from_price_change".to_string()
    ));

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
    assert_eq!(projection.quote_ticks[0].instrument_id, instrument_id);
    assert_eq!(
        projection.quote_ticks[0].bid_price.as_decimal().to_string(),
        "0.49"
    );
    assert_eq!(
        projection.quote_ticks[0].ask_price.as_decimal().to_string(),
        "0.50"
    );
    assert_eq!(
        projection.quote_ticks[0].bid_size.as_decimal().to_string(),
        "12.000000"
    );
    assert_eq!(
        projection.quote_ticks[0].ask_size.as_decimal().to_string(),
        "0.000000"
    );
    assert_eq!(
        projection.quote_ticks[0].ts_event,
        UnixNanos::from(1_772_023_200_456_000_000)
    );
    assert_eq!(
        projection.quote_ticks[0].ts_init,
        UnixNanos::from(1_772_023_200_556_000_000)
    );
}

#[test]
fn pmxt_one_off_projection_projects_trade_ticks_with_transaction_hash_dedupe_and_sequences() {
    let projection = pmxt_trade_projection_fixture();

    assert!(projection.order_book_deltas.is_empty());
    assert!(projection.quote_ticks.is_empty());
    assert_eq!(projection.trade_ticks.len(), 2);
    assert_eq!(
        projection.trade_ticks[0].trade_id.to_string(),
        "000000000000000000abcdef-en-a-000000"
    );
    assert_eq!(
        projection.trade_ticks[1].trade_id.to_string(),
        "000000000000000000abcdef-en-a-000001"
    );
    assert_eq!(
        projection.trade_ticks[0].ts_event,
        UnixNanos::from(1_772_023_200_123_000_000)
    );
    assert_eq!(
        projection.trade_ticks[0].ts_init,
        UnixNanos::from(1_772_023_200_223_000_000)
    );
    assert_eq!(
        projection.trade_ticks[1].ts_event,
        UnixNanos::from(1_772_023_200_456_000_000)
    );
    assert_eq!(projection.trade_dedupe_provenance.len(), 1);
    assert_eq!(projection.trade_dedupe_provenance[0].duplicate_count, 2);
    assert_eq!(
        projection.trade_dedupe_provenance[0].max_ts_init,
        UnixNanos::from(1_772_023_200_323_000_000)
    );
    assert!(
        projection
            .nt_surfaces_used
            .contains(&"nautilus_model::data::TradeTick".to_string())
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
    assert_eq!(
        catalog_report.quote_tick_count,
        projection.quote_ticks.len() as u64
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
            false,
        )
        .expect("read back PMXT L2 deltas");
    assert_eq!(loaded.len(), projection.order_book_deltas.len());
    let loaded_quotes: Vec<QuoteTick> = catalog
        .query_typed_data::<QuoteTick>(
            Some(vec![instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            false,
        )
        .expect("read back PMXT quote ticks");
    assert_eq!(loaded_quotes.len(), projection.quote_ticks.len());
    assert_eq!(
        loaded_quotes[0].bid_price.as_decimal(),
        projection.quote_ticks[0].bid_price.as_decimal()
    );

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
fn pmxt_one_off_projection_writes_nt_catalog_and_reads_back_trade_ticks() {
    let projection = pmxt_trade_projection_fixture();
    let instrument_id = binary_option_instrument_id(&projection.instrument);
    let catalog_dir = tempfile::TempDir::new().expect("catalog dir");
    let catalog_report = write_pmxt_one_off_projection_to_catalog(catalog_dir.path(), &projection)
        .expect("write PMXT one-off TradeTick projection to catalog");

    assert_eq!(catalog_report.order_book_delta_count, 0);
    assert_eq!(catalog_report.quote_tick_count, 0);
    assert_eq!(catalog_report.trade_tick_count, 2);
    assert!(!catalog_report.catalog_hash.is_empty());

    let mut catalog = ParquetDataCatalog::new(catalog_dir.path(), None, None, None, None);
    let loaded: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(
            Some(vec![instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("read back PMXT TradeTicks");
    assert_eq!(loaded.len(), 2);
    assert_eq!(
        loaded[0].trade_id.to_string(),
        "000000000000000000abcdef-en-a-000000"
    );
    assert_eq!(
        loaded[1].trade_id.to_string(),
        "000000000000000000abcdef-en-a-000001"
    );
}

#[test]
fn pmxt_selected_source_parquet_projects_l2_rows_without_full_source_rescan() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    write_pmxt_selected_source_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );

    let selected = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        selected_source_parquet_path: selected_parquet_path.clone(),
        selected_source_report_path: selected_report_path.clone(),
        schema: pmxt_selected_source_schema(),
    })
    .expect("project selected-source parquet");

    assert_eq!(selected.selected_rows, 3);
    assert_eq!(selected.projected_l2_rows, 2);
    assert_eq!(selected.skipped_non_l2_rows, 1);
    assert_eq!(
        selected.event_count_ledger_hash,
        PMXT_TEST_EVENT_COUNT_LEDGER_HASH
    );
    assert_eq!(
        selected.selected_asset_ids_hash,
        PMXT_TEST_SELECTED_ASSET_IDS_HASH
    );
    assert_eq!(
        selected.projection.usage_scope,
        SourceProofUsageScope::OneOffBackfillData
    );
    assert_eq!(selected.projection.order_book_deltas.len(), 4);
    assert_eq!(selected.projection.quote_ticks.len(), 1);
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

#[test]
fn pmxt_selected_source_parquet_projects_trade_ticks_from_configured_trade_columns() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    write_pmxt_selected_source_trade_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );

    let selected = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        selected_source_parquet_path: selected_parquet_path,
        selected_source_report_path: selected_report_path,
        schema: pmxt_selected_source_schema_with_trades(),
    })
    .expect("project selected-source parquet with last_trade_price rows");

    assert_eq!(selected.selected_rows, 3);
    assert_eq!(selected.projected_l2_rows, 3);
    assert_eq!(selected.skipped_non_l2_rows, 0);
    assert!(selected.projection.order_book_deltas.is_empty());
    assert_eq!(selected.projection.trade_ticks.len(), 2);
    assert_eq!(
        selected.projection.trade_ticks[0].trade_id.to_string(),
        "000000000000000000abcdef-en-a-000000"
    );
    assert_eq!(selected.projection.trade_dedupe_provenance.len(), 1);
    assert_eq!(
        selected.projection.trade_dedupe_provenance[0].duplicate_count,
        2
    );
}

#[test]
fn pmxt_selected_source_parquet_projects_binary_view_market_column() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    write_pmxt_selected_source_binary_view_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );

    let selected = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        selected_source_parquet_path: selected_parquet_path,
        selected_source_report_path: selected_report_path,
        schema: pmxt_selected_source_schema(),
    })
    .expect("project selected-source parquet with BinaryView market column");

    assert_eq!(selected.selected_rows, 3);
    assert_eq!(selected.projected_l2_rows, 2);
    assert_eq!(selected.projection.order_book_deltas.len(), 4);
}

#[test]
fn pmxt_selected_source_parquet_projects_fixed_size_binary_market_column() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    let selected_condition_id =
        "0x92889d49761307073d461289d01208c3b19292d17da937c0f57501c7b7efa50d";
    write_pmxt_selected_source_fixed_size_binary_fixture(
        &selected_parquet_path,
        selected_condition_id,
    );
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );

    let selected = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: selected_condition_id.to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets_for_condition(selected_condition_id),
        selected_source_parquet_path: selected_parquet_path,
        selected_source_report_path: selected_report_path,
        schema: pmxt_selected_source_schema(),
    })
    .expect("project selected-source parquet with FixedSizeBinary market column");

    assert_eq!(selected.selected_rows, 3);
    assert_eq!(selected.projected_l2_rows, 2);
    assert_eq!(selected.projection.order_book_deltas.len(), 4);
}

#[test]
fn pmxt_selected_source_projection_rejects_ignored_tick_size_change_rows() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    write_pmxt_selected_source_with_tick_size_change_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        4,
    );

    let mut schema = pmxt_selected_source_schema();
    schema
        .ignored_event_types
        .push("tick_size_change".to_string());

    let error = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        selected_source_parquet_path: selected_parquet_path,
        selected_source_report_path: selected_report_path,
        schema,
    })
    .expect_err("tick_size_change rows must not be silently ignored");

    assert!(error.to_string().contains("cannot be ignored"), "{error:#}");
}

#[test]
fn pmxt_selected_source_projection_requires_selector_to_exclude_forbidden_event_types() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    write_pmxt_selected_source_fixture(&selected_parquet_path);
    write_selector_report_without_excluded_events_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );

    let error = project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        selected_source_parquet_path: selected_parquet_path,
        selected_source_report_path: selected_report_path,
        schema: pmxt_selected_source_schema(),
    })
    .expect_err("selector must prove forbidden event families are excluded");

    assert!(error.to_string().contains("selector report"), "{error:#}");
    assert!(error.to_string().contains("tick_size_change"), "{error:#}");
}

#[test]
fn pmxt_one_off_conversion_projection_writes_manifest_checkpoint_and_catalog_metadata() {
    let projection = pmxt_projection_fixture();
    let output_dir = tempfile::TempDir::new().expect("output dir");
    let catalog_root = output_dir.path().join("nt-catalog");
    let fingerprint = pmxt_conversion_fingerprint();

    let completed = write_pmxt_one_off_conversion_projection(PmxtOneOffConversionProjectionSpec {
        output_dir: output_dir.path().to_path_buf(),
        catalog_root: catalog_root.clone(),
        projection: projection.clone(),
        fingerprint: fingerprint.clone(),
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: catalog_root.display().to_string(),
        execution_catalog_uri: catalog_root.display().to_string(),
        direct_s3_catalog_access_proven: false,
        completed_at: "2026-06-08T00:00:00Z".to_string(),
    })
    .expect("write PMXT one-off conversion projection");

    assert!(output_dir.path().join(CONVERSION_CHECKPOINT_FILE).exists());
    assert!(output_dir.path().join(CONVERSION_MANIFEST_FILE).exists());
    assert!(output_dir.path().join(CATALOG_METADATA_FILE).exists());
    assert_eq!(
        completed.catalog_projection.order_book_delta_count,
        projection.order_book_deltas.len() as u64
    );
    assert_eq!(
        completed.catalog_projection.quote_tick_count,
        projection.quote_ticks.len() as u64
    );
    assert_eq!(completed.conversion_manifest.nt_data_type, "OrderBookDelta");
    assert_eq!(
        completed.conversion_manifest.canonical_rows,
        projection.order_book_deltas.len()
    );
    assert_eq!(
        completed.conversion_manifest.catalog_hash,
        completed.catalog_projection.catalog_hash
    );
    assert_eq!(
        completed
            .conversion_manifest
            .catalog_rows_by_nt_data_type
            .get(NT_DATA_TYPE_QUOTE_TICK),
        Some(&projection.quote_ticks.len())
    );
    assert!(
        completed
            .conversion_catalog_metadata
            .catalog_nt_data_types
            .contains(&NT_DATA_TYPE_QUOTE_TICK.to_string())
    );
    assert_eq!(
        inspect_conversion_output(output_dir.path(), &fingerprint).expect("inspect conversion"),
        ConversionOutputState::Complete {
            manifest_hash: completed.conversion_manifest_hash.clone(),
            checkpoint_hash: completed.conversion_checkpoint_hash.clone(),
            catalog_hash: completed.catalog_projection.catalog_hash.clone(),
        }
    );
}

#[test]
fn pmxt_one_off_conversion_projection_rerun_reuses_matching_complete_output() {
    let projection = pmxt_projection_fixture();
    let output_dir = tempfile::TempDir::new().expect("output dir");
    let catalog_root = output_dir.path().join("nt-catalog");
    let fingerprint = pmxt_conversion_fingerprint();
    let spec = PmxtOneOffConversionProjectionSpec {
        output_dir: output_dir.path().to_path_buf(),
        catalog_root: catalog_root.clone(),
        projection,
        fingerprint,
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: catalog_root.display().to_string(),
        execution_catalog_uri: catalog_root.display().to_string(),
        direct_s3_catalog_access_proven: false,
        completed_at: "2026-06-08T00:00:00Z".to_string(),
    };

    let first = write_pmxt_one_off_conversion_projection(spec.clone())
        .expect("write first PMXT one-off conversion projection");
    let second = write_pmxt_one_off_conversion_projection(spec)
        .expect("reuse matching PMXT one-off conversion projection");

    assert_eq!(
        second.conversion_manifest_hash,
        first.conversion_manifest_hash
    );
    assert_eq!(
        second.conversion_checkpoint_hash,
        first.conversion_checkpoint_hash
    );
    assert_eq!(
        second.conversion_catalog_metadata_hash,
        first.conversion_catalog_metadata_hash
    );
    assert_eq!(
        second.catalog_projection.catalog_hash,
        first.catalog_projection.catalog_hash
    );
}

#[test]
fn pmxt_one_off_l2_backtest_result_contract_binds_conversion_and_selector_provenance() {
    let projection = pmxt_projection_fixture();
    let output_dir = tempfile::TempDir::new().expect("output dir");
    let catalog_root = output_dir.path().join("nt-catalog");
    let fingerprint = pmxt_conversion_fingerprint();
    let completed = write_pmxt_one_off_conversion_projection(PmxtOneOffConversionProjectionSpec {
        output_dir: output_dir.path().to_path_buf(),
        catalog_root: catalog_root.clone(),
        projection: projection.clone(),
        fingerprint: fingerprint.clone(),
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: catalog_root.display().to_string(),
        execution_catalog_uri: catalog_root.display().to_string(),
        direct_s3_catalog_access_proven: false,
        completed_at: "2026-06-08T00:00:00Z".to_string(),
    })
    .expect("write PMXT one-off conversion projection");
    let manifest = pmxt_l2_manifest(&projection, &catalog_root, output_dir.path());
    let manifest_hash = manifest.manifest_hash();
    let artifact_uris = pmxt_result_artifact_uris(output_dir.path());

    let output = run_pmxt_one_off_l2_backtest_contract(PmxtOneOffBacktestContractSpec {
        completed: &completed,
        manifest: &manifest,
        manifest_hash: &manifest_hash,
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "source-proof-reviewer",
        accepted_at: "2026-06-08T00:00:00Z",
        event_count_ledger_hash: PMXT_TEST_EVENT_COUNT_LEDGER_HASH,
        selected_asset_ids_hash: PMXT_TEST_SELECTED_ASSET_IDS_HASH,
        artifact_uris,
        created_at: "2026-06-08T00:00:00Z",
        claim_limits: vec![
            "one-off PMXT L2 sample only".to_string(),
            "no dynamic tick-size replay claim".to_string(),
            "no expanded coverage claim".to_string(),
        ],
    })
    .expect("run PMXT one-off L2 BacktestNode and build result contract");

    assert_eq!(
        output.nt_result.iterations,
        projection.order_book_deltas.len()
    );
    assert_eq!(
        output.contract.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );
    assert_eq!(output.contract.source_proof_id, fingerprint.source_proof_id);
    assert_eq!(
        output.contract.source_proof_version,
        fingerprint.source_proof_version
    );
    assert_eq!(
        output.contract.accepted_object_sha256,
        fingerprint.accepted_object_sha256
    );
    assert_eq!(
        output.contract.converter_identity,
        fingerprint.converter_identity
    );
    assert_eq!(
        output.contract.conversion_manifest_hash,
        completed.conversion_manifest_hash
    );
    assert_eq!(
        output.contract.conversion_checkpoint_hash,
        completed.conversion_checkpoint_hash
    );
    assert_eq!(
        output.contract.catalog_hash,
        completed.catalog_projection.catalog_hash
    );
    assert_eq!(
        output.contract.catalog_metadata_hash,
        completed.conversion_catalog_metadata_hash
    );
    assert_eq!(
        output.contract.event_count_ledger_hash.as_deref(),
        Some(PMXT_TEST_EVENT_COUNT_LEDGER_HASH)
    );
    assert_eq!(
        output.contract.selected_asset_ids_hash.as_deref(),
        Some(PMXT_TEST_SELECTED_ASSET_IDS_HASH)
    );
    assert_eq!(output.contract.execution_model, manifest.execution_model);
    assert_eq!(
        output.contract.venue_queue_position,
        Some(manifest.venue.queue_position)
    );
    assert_eq!(
        output.contract.catalog_data_types,
        manifest
            .catalog_inputs
            .iter()
            .map(|input| input.data_type.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        output.contract.nt_result.run_config_id.as_deref(),
        Some("pmxt-one-off-l2-contract-proof")
    );
}

#[test]
fn pmxt_one_off_l2_backtest_result_contract_rejects_duplicate_manifest_data_types() {
    let projection = pmxt_projection_fixture();
    let output_dir = tempfile::TempDir::new().expect("output dir");
    let catalog_root = output_dir.path().join("nt-catalog");
    let fingerprint = pmxt_conversion_fingerprint();
    let completed = write_pmxt_one_off_conversion_projection(PmxtOneOffConversionProjectionSpec {
        output_dir: output_dir.path().to_path_buf(),
        catalog_root: catalog_root.clone(),
        projection: projection.clone(),
        fingerprint,
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: catalog_root.display().to_string(),
        execution_catalog_uri: catalog_root.display().to_string(),
        direct_s3_catalog_access_proven: false,
        completed_at: "2026-06-08T00:00:00Z".to_string(),
    })
    .expect("write PMXT one-off conversion projection");
    let mut manifest = pmxt_l2_manifest(&projection, &catalog_root, output_dir.path());
    manifest
        .catalog_inputs
        .push(manifest.catalog_inputs[0].clone());
    let manifest_hash = manifest.manifest_hash();

    let error = run_pmxt_one_off_l2_backtest_contract(PmxtOneOffBacktestContractSpec {
        completed: &completed,
        manifest: &manifest,
        manifest_hash: &manifest_hash,
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "source-proof-reviewer",
        accepted_at: "2026-06-08T00:00:00Z",
        event_count_ledger_hash: PMXT_TEST_EVENT_COUNT_LEDGER_HASH,
        selected_asset_ids_hash: PMXT_TEST_SELECTED_ASSET_IDS_HASH,
        artifact_uris: pmxt_result_artifact_uris(output_dir.path()),
        created_at: "2026-06-08T00:00:00Z",
        claim_limits: vec![
            "one-off PMXT L2 sample only".to_string(),
            "no dynamic tick-size replay claim".to_string(),
            "no expanded coverage claim".to_string(),
        ],
    })
    .expect_err("duplicate PMXT manifest data types must be rejected");

    assert!(
        error.to_string().contains("duplicates data_type"),
        "{error:#}"
    );
}

#[test]
fn pmxt_one_off_l2_artifact_root_run_writes_result_contract_from_selected_source_report_chain() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    let output_dir = dir
        .path()
        .join("artifact-root")
        .join("backtests")
        .join("pmxt-run");
    let catalog_root = output_dir.join("nt-catalog");
    write_pmxt_selected_source_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );
    let expected_projection =
        project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path.clone(),
            selected_source_report_path: selected_report_path.clone(),
            schema: pmxt_selected_source_schema(),
        })
        .expect("expected selected-source projection");
    let manifest = pmxt_l2_manifest(&expected_projection.projection, &catalog_root, &output_dir);
    let manifest_hash = manifest.manifest_hash();
    let selected_source_sha256 = sha256_file(&selected_parquet_path);

    let spec = PmxtOneOffArtifactRootRunSpec {
        selected_source: PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path.clone(),
            selected_source_report_path: selected_report_path.clone(),
            schema: pmxt_selected_source_schema(),
        },
        output_dir: output_dir.clone(),
        catalog_root: catalog_root.clone(),
        fingerprint: pmxt_conversion_fingerprint_for_hash(&selected_source_sha256),
        manifest,
        manifest_hash,
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: format!("file://{}", catalog_root.display()),
        execution_catalog_uri: catalog_root.display().to_string(),
        direct_s3_catalog_access_proven: false,
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "source-proof-reviewer".to_string(),
        accepted_at: "2026-06-08T00:00:00Z".to_string(),
        artifact_uris: pmxt_result_artifact_uris(&output_dir),
        created_at: "2026-06-08T00:00:00Z".to_string(),
        claim_limits: vec![
            "one-off PMXT L2 sample only".to_string(),
            "no dynamic tick-size replay claim".to_string(),
            "no expanded coverage claim".to_string(),
        ],
    };
    let run = write_pmxt_one_off_l2_artifact_root_run(spec.clone())
        .expect("write PMXT one-off artifact-root run");

    let contract_path = output_dir.join(PMXT_ONE_OFF_RESULT_CONTRACT_FILE);
    assert!(contract_path.exists(), "result contract must be written");
    assert!(output_dir.join(CONVERSION_CHECKPOINT_FILE).exists());
    assert!(output_dir.join(CONVERSION_MANIFEST_FILE).exists());
    assert!(output_dir.join(CATALOG_METADATA_FILE).exists());
    let written_contract: BacktestResultContract =
        serde_json::from_slice(&std::fs::read(&contract_path).expect("read contract"))
            .expect("parse contract");
    assert_eq!(written_contract, run.contract_output.contract);
    assert_eq!(run.selected_projection.selected_rows, 3);
    assert_eq!(run.selected_projection.projected_l2_rows, 2);
    assert_eq!(
        run.contract_output.nt_result.iterations,
        expected_projection.projection.order_book_deltas.len()
    );
    assert_eq!(
        run.contract_output
            .contract
            .event_count_ledger_hash
            .as_deref(),
        Some(PMXT_TEST_EVENT_COUNT_LEDGER_HASH)
    );
    assert_eq!(
        run.contract_output
            .contract
            .selected_asset_ids_hash
            .as_deref(),
        Some(PMXT_TEST_SELECTED_ASSET_IDS_HASH)
    );
    assert_eq!(
        run.contract_output.contract.accepted_object_sha256,
        selected_source_sha256
    );
    assert_eq!(
        run.contract_output.contract.conversion_manifest_hash,
        run.completed.conversion_manifest_hash
    );
    assert_eq!(
        run.contract_output.contract.catalog_hash,
        run.completed.catalog_projection.catalog_hash
    );

    let rerun = write_pmxt_one_off_l2_artifact_root_run(spec)
        .expect("rerun PMXT one-off artifact-root run idempotently");
    let contract_after_rerun: BacktestResultContract =
        serde_json::from_slice(&std::fs::read(&contract_path).expect("read rerun contract"))
            .expect("parse rerun contract");
    assert_eq!(contract_after_rerun, written_contract);
    assert_eq!(rerun.contract_output.contract, written_contract);
}

#[test]
fn pmxt_one_off_l2_artifact_root_run_resolves_repo_relative_catalog_root_for_runtime_io() {
    let repo_root = repo_root_from_manifest_dir();
    let scratch_root = PathBuf::from(format!(
        "target/pmxt-test-repo-relative-catalog-root-{}",
        std::process::id()
    ));
    let anchored_scratch_root = repo_root.join(&scratch_root);
    let _ = std::fs::remove_dir_all(&anchored_scratch_root);
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    let output_dir = scratch_root
        .join("artifact-root")
        .join("backtests")
        .join("pmxt-run");
    let catalog_root = output_dir.join("nt-catalog");
    let expected_catalog_root = catalog_root.display().to_string();
    let output_catalog_uri = format!("file://{expected_catalog_root}");
    write_pmxt_selected_source_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );
    let expected_projection =
        project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path.clone(),
            selected_source_report_path: selected_report_path.clone(),
            schema: pmxt_selected_source_schema(),
        })
        .expect("expected selected-source projection");
    let manifest = pmxt_l2_manifest(&expected_projection.projection, &catalog_root, &output_dir);
    let manifest_hash = manifest.manifest_hash();
    let selected_source_sha256 = sha256_file(&selected_parquet_path);

    let run = write_pmxt_one_off_l2_artifact_root_run(PmxtOneOffArtifactRootRunSpec {
        selected_source: PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path.clone(),
            selected_source_report_path: selected_report_path.clone(),
            schema: pmxt_selected_source_schema(),
        },
        output_dir: output_dir.clone(),
        catalog_root: catalog_root.clone(),
        fingerprint: pmxt_conversion_fingerprint_for_hash(&selected_source_sha256),
        manifest,
        manifest_hash,
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: output_catalog_uri.clone(),
        execution_catalog_uri: expected_catalog_root.clone(),
        direct_s3_catalog_access_proven: false,
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "source-proof-reviewer".to_string(),
        accepted_at: "2026-06-08T00:00:00Z".to_string(),
        artifact_uris: pmxt_result_artifact_uris(&output_dir),
        created_at: "2026-06-08T00:00:00Z".to_string(),
        claim_limits: vec![
            "one-off PMXT L2 sample only".to_string(),
            "no dynamic tick-size replay claim".to_string(),
            "no expanded coverage claim".to_string(),
        ],
    })
    .expect("write PMXT one-off artifact-root run with repo-relative catalog_root");

    let anchored_output_dir = repo_root.join(&output_dir);
    let anchored_catalog_root = repo_root.join(&catalog_root);
    assert!(
        anchored_catalog_root.exists(),
        "catalog root should be marker-root anchored at {}",
        anchored_catalog_root.display()
    );
    assert!(
        anchored_output_dir
            .join(CONVERSION_CHECKPOINT_FILE)
            .exists()
    );
    assert!(anchored_output_dir.join(CONVERSION_MANIFEST_FILE).exists());
    assert!(anchored_output_dir.join(CATALOG_METADATA_FILE).exists());
    assert!(
        anchored_output_dir
            .join(PMXT_ONE_OFF_RESULT_CONTRACT_FILE)
            .exists()
    );
    assert_eq!(run.completed.catalog_projection.catalog_root, catalog_root);
    assert_eq!(
        run.completed.conversion_manifest.output_catalog_uri,
        output_catalog_uri
    );
    assert_eq!(
        run.completed
            .conversion_catalog_metadata
            .execution_catalog_uri,
        expected_catalog_root
    );
    assert_eq!(
        run.contract_output.contract.artifact_uris.nt_catalog_uri,
        output_catalog_uri
    );
    let conversion_manifest_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(anchored_output_dir.join(CONVERSION_MANIFEST_FILE))
            .expect("read conversion manifest"),
    )
    .expect("parse conversion manifest");
    assert_eq!(
        conversion_manifest_json["output_catalog_uri"],
        output_catalog_uri
    );
    let catalog_metadata_json: serde_json::Value = serde_json::from_slice(
        &std::fs::read(anchored_output_dir.join(CATALOG_METADATA_FILE))
            .expect("read catalog metadata"),
    )
    .expect("parse catalog metadata");
    assert_eq!(
        catalog_metadata_json["execution_catalog_uri"],
        expected_catalog_root
    );
    assert_eq!(
        run.contract_output.nt_result.iterations,
        expected_projection.projection.order_book_deltas.len()
    );
    std::fs::remove_dir_all(anchored_scratch_root).expect("remove PMXT repo-relative scratch dir");
}

#[test]
fn pmxt_one_off_artifact_root_run_binds_mixed_l2_and_trade_tick_catalog() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    let output_dir = dir
        .path()
        .join("artifact-root")
        .join("backtests")
        .join("pmxt-run");
    let catalog_root = output_dir.join("nt-catalog");
    write_pmxt_selected_source_mixed_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        5,
    );
    let expected_projection =
        project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path.clone(),
            selected_source_report_path: selected_report_path.clone(),
            schema: pmxt_selected_source_schema_with_trades(),
        })
        .expect("expected mixed selected-source projection");
    assert_eq!(expected_projection.projection.order_book_deltas.len(), 4);
    assert_eq!(expected_projection.projection.quote_ticks.len(), 1);
    assert_eq!(expected_projection.projection.trade_ticks.len(), 2);

    let mut manifest =
        pmxt_l2_manifest(&expected_projection.projection, &catalog_root, &output_dir);
    let mut quote_input = manifest.catalog_inputs[0].clone();
    quote_input.data_type = NT_DATA_TYPE_QUOTE_TICK.to_string();
    manifest.catalog_inputs.push(quote_input);
    let mut trade_input = manifest.catalog_inputs[0].clone();
    trade_input.data_type = NT_DATA_TYPE_TRADE_TICK.to_string();
    manifest.catalog_inputs.push(trade_input);
    let manifest_hash = manifest.manifest_hash();
    let selected_source_sha256 = sha256_file(&selected_parquet_path);
    let catalog_probe_dir = tempfile::TempDir::new().expect("catalog probe dir");
    write_pmxt_one_off_projection_to_catalog(
        catalog_probe_dir.path(),
        &expected_projection.projection,
    )
    .expect("write PMXT mixed catalog probe");
    let mut catalog_probe = ParquetDataCatalog::from_uri(
        catalog_probe_dir
            .path()
            .to_str()
            .expect("catalog probe path is UTF-8"),
        None,
        None,
        None,
        None,
    )
    .expect("open PMXT mixed catalog probe");
    let loaded_trades: Vec<TradeTick> = catalog_probe
        .query_typed_data::<TradeTick>(
            Some(vec![
                binary_option_instrument_id(&expected_projection.projection.instrument).to_string(),
            ]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("read back PMXT mixed TradeTicks");
    assert_eq!(
        loaded_trades.len(),
        expected_projection.projection.trade_ticks.len()
    );
    let loaded_quotes: Vec<QuoteTick> = catalog_probe
        .query_typed_data::<QuoteTick>(
            Some(vec![
                binary_option_instrument_id(&expected_projection.projection.instrument).to_string(),
            ]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("read back PMXT mixed QuoteTicks");
    assert_eq!(
        loaded_quotes.len(),
        expected_projection.projection.quote_ticks.len()
    );
    let mut probe_manifest = manifest.clone();
    for catalog_input in &mut probe_manifest.catalog_inputs {
        catalog_input.catalog_path = catalog_probe_dir.path().display().to_string();
    }
    let run_config = probe_manifest.to_nt_run_config().expect("PMXT run config");
    assert_eq!(run_config.data().len(), 3);
    let loaded_data_counts = run_config
        .data()
        .iter()
        .map(|data_config| {
            BacktestNode::load_data_config(data_config, None, None)
                .expect("load PMXT data config")
                .len()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        loaded_data_counts,
        vec![
            expected_projection.projection.order_book_deltas.len(),
            expected_projection.projection.quote_ticks.len(),
            expected_projection.projection.trade_ticks.len()
        ]
    );
    let spec = PmxtOneOffArtifactRootRunSpec {
        selected_source: PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path,
            selected_source_report_path: selected_report_path,
            schema: pmxt_selected_source_schema_with_trades(),
        },
        output_dir: output_dir.clone(),
        catalog_root: catalog_root.clone(),
        fingerprint: pmxt_conversion_fingerprint_for_hash(&selected_source_sha256),
        manifest,
        manifest_hash,
        normalized_schema_version: "pmxt-selected-source-l2.v1".to_string(),
        output_catalog_uri: format!("file://{}", catalog_root.display()),
        execution_catalog_uri: catalog_root.display().to_string(),
        direct_s3_catalog_access_proven: false,
        acceptance_mode: AcceptanceMode::Manual,
        accepted_by: "source-proof-reviewer".to_string(),
        accepted_at: "2026-06-08T00:00:00Z".to_string(),
        artifact_uris: pmxt_result_artifact_uris(&output_dir),
        created_at: "2026-06-08T00:00:00Z".to_string(),
        claim_limits: vec![
            "one-off PMXT L2+QuoteTick+TradeTick sample only".to_string(),
            "no dynamic tick-size replay claim".to_string(),
            "no expanded coverage claim".to_string(),
        ],
    };

    let run = write_pmxt_one_off_l2_artifact_root_run(spec)
        .expect("write mixed PMXT one-off artifact-root run");

    assert_eq!(
        run.completed.catalog_projection.order_book_delta_count,
        expected_projection.projection.order_book_deltas.len() as u64
    );
    assert_eq!(
        run.completed.catalog_projection.quote_tick_count,
        expected_projection.projection.quote_ticks.len() as u64
    );
    assert_eq!(
        run.completed.catalog_projection.trade_tick_count,
        expected_projection.projection.trade_ticks.len() as u64
    );
    assert_eq!(
        run.completed.conversion_manifest.catalog_nt_data_types,
        vec![
            NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
            NT_DATA_TYPE_QUOTE_TICK.to_string(),
            NT_DATA_TYPE_TRADE_TICK.to_string()
        ]
    );
    assert_eq!(
        run.completed
            .conversion_manifest
            .catalog_rows_by_nt_data_type
            .get(NT_DATA_TYPE_ORDER_BOOK_DELTA),
        Some(&expected_projection.projection.order_book_deltas.len())
    );
    assert_eq!(
        run.completed
            .conversion_manifest
            .catalog_rows_by_nt_data_type
            .get(NT_DATA_TYPE_QUOTE_TICK),
        Some(&expected_projection.projection.quote_ticks.len())
    );
    assert_eq!(
        run.completed
            .conversion_manifest
            .catalog_rows_by_nt_data_type
            .get(NT_DATA_TYPE_TRADE_TICK),
        Some(&expected_projection.projection.trade_ticks.len())
    );
    assert_eq!(
        run.completed
            .conversion_catalog_metadata
            .catalog_nt_data_types,
        run.completed.conversion_manifest.catalog_nt_data_types
    );
    assert_eq!(
        run.completed
            .conversion_catalog_metadata
            .catalog_rows_by_nt_data_type,
        run.completed
            .conversion_manifest
            .catalog_rows_by_nt_data_type
    );
    assert_eq!(
        run.contract_output.nt_result.iterations,
        expected_projection.projection.order_book_deltas.len()
            + expected_projection.projection.quote_ticks.len()
            + expected_projection.projection.trade_ticks.len()
    );
    assert_eq!(
        run.contract_output.contract.conversion_manifest_hash,
        run.completed.conversion_manifest_hash
    );
    assert_eq!(
        run.contract_output.contract.catalog_metadata_hash,
        run.completed.conversion_catalog_metadata_hash
    );
}

#[test]
fn pmxt_one_off_l2_artifact_root_cli_writes_result_contract_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let selected_parquet_path = dir.path().join("selected-source.parquet");
    let selector_report_path = dir.path().join("first-proof-selector-report.json");
    let selected_report_path = dir.path().join("selected-source-report.json");
    let gamma_markets_path = dir.path().join("gamma-markets.json");
    let manifest_path = dir.path().join("manifest.toml");
    let spec_path = dir.path().join("pmxt-one-off-run.toml");
    let output_dir = dir
        .path()
        .join("artifact-root")
        .join("backtests")
        .join("pmxt-run");
    let catalog_root = output_dir.join("nt-catalog");
    write_pmxt_selected_source_fixture(&selected_parquet_path);
    write_selector_report_fixture(&selector_report_path);
    write_selected_source_report_with_selector(
        &selected_report_path,
        &selected_parquet_path,
        &selector_report_path,
        3,
    );
    std::fs::write(&gamma_markets_path, gamma_markets_json()).expect("write gamma markets");
    let expected_projection =
        project_pmxt_selected_source_parquet_to_nt(PmxtSelectedSourceProjectionSpec {
            source_binding: "synthetic-pmxt-one-off-source".to_string(),
            usage_scope: SourceProofUsageScope::OneOffBackfillData,
            selected_condition_id: "0xcondition".to_string(),
            selected_token_id: "token-a".to_string(),
            gamma_markets: gamma_markets(),
            selected_source_parquet_path: selected_parquet_path.clone(),
            selected_source_report_path: selected_report_path.clone(),
            schema: pmxt_selected_source_schema(),
        })
        .expect("expected selected-source projection");
    let manifest = pmxt_l2_manifest(&expected_projection.projection, &catalog_root, &output_dir);
    std::fs::write(
        &manifest_path,
        toml::to_string(&manifest).expect("serialize manifest"),
    )
    .expect("write manifest");
    let selected_source_sha256 = sha256_file(&selected_parquet_path);
    std::fs::write(
        &spec_path,
        format!(
            r#"
output_dir = "{output_dir}"
catalog_root = "{catalog_root}"
manifest_path = "{manifest_path}"
normalized_schema_version = "pmxt-selected-source-l2.v1"
direct_s3_catalog_access_proven = false
acceptance_mode = "manual"
accepted_by = "source-proof-reviewer"
accepted_at = "2026-06-08T00:00:00Z"
created_at = "2026-06-08T00:00:00Z"
claim_limits = [
  "one-off PMXT L2 sample only",
  "no dynamic tick-size replay claim",
  "no expanded coverage claim",
]

[selected_source]
source_binding = "synthetic-pmxt-one-off-source"
usage_scope = "one_off_backfill_data"
selected_condition_id = "0xcondition"
selected_token_id = "token-a"
gamma_markets_json_path = "{gamma_markets_path}"
selected_source_parquet_path = "{selected_parquet_path}"
selected_source_report_path = "{selected_report_path}"

[selected_source.schema]
timestamp_received_column = "timestamp_received"
timestamp_column = "timestamp"
market_column = "market"
event_type_column = "event_type"
asset_id_column = "asset_id"
bids_column = "bids"
asks_column = "asks"
price_column = "price"
size_column = "size"
side_column = "side"
best_bid_column = "best_bid"
best_ask_column = "best_ask"
buy_side = "BUY"
sell_side = "SELL"
book_event_type = "book"
price_change_event_type = "price_change"
ignored_event_types = ["last_trade_price"]
forbidden_ignored_event_types = ["tick_size_change"]

[fingerprint]
source_proof_id = "source-proof-pmxt-one-off"
source_proof_version = 1
accepted_object_sha256 = "{selected_source_sha256}"
converter_identity = "pmxt-one-off-selected-source-l2-to-nt.v1"
converter_version = "1"
converter_config_hash = "7c5ff8475a73c3aaf3e64cc09d803ff34de9cbc51345978406125fcc5147879a"

[artifact_uris]
source_proof_uri = "file://{output_dir}/accepted-source-proof.json"
canonical_table_uri = "file://{selected_parquet_path}"
nt_catalog_uri = "file://{catalog_root}"
catalog_metadata_uri = "file://{output_dir}/catalog-metadata.json"
result_contract_uri = "file://{output_dir}/backtest-result-contract.json"
"#,
            output_dir = output_dir.display(),
            catalog_root = catalog_root.display(),
            manifest_path = manifest_path.display(),
            gamma_markets_path = gamma_markets_path.display(),
            selected_parquet_path = selected_parquet_path.display(),
            selected_report_path = selected_report_path.display(),
        ),
    )
    .expect("write PMXT one-off run spec");

    let binary = std::env::var("CARGO_BIN_EXE_pmxt_one_off_l2_artifact_root_run")
        .expect("pmxt_one_off_l2_artifact_root_run binary path");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run PMXT one-off artifact-root CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("result_contract = "), "{stdout}");
    assert!(stdout.contains("projected_l2_rows = 2"), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "nt_iterations = {}",
            expected_projection.projection.order_book_deltas.len()
        )),
        "{stdout}"
    );

    let contract_path = output_dir.join(PMXT_ONE_OFF_RESULT_CONTRACT_FILE);
    let contract: BacktestResultContract =
        serde_json::from_slice(&std::fs::read(contract_path).expect("read contract"))
            .expect("parse contract");
    assert_eq!(contract.accepted_object_sha256, selected_source_sha256);
    assert_eq!(
        contract.event_count_ledger_hash.as_deref(),
        Some(PMXT_TEST_EVENT_COUNT_LEDGER_HASH)
    );
    assert_eq!(
        contract.selected_asset_ids_hash.as_deref(),
        Some(PMXT_TEST_SELECTED_ASSET_IDS_HASH)
    );
}

fn gamma_markets() -> Vec<GammaMarket> {
    serde_json::from_str(gamma_markets_json()).expect("Gamma fixture")
}

fn gamma_markets_for_condition(condition_id: &str) -> Vec<GammaMarket> {
    serde_json::from_str(&gamma_markets_json().replace("0xcondition", condition_id))
        .expect("Gamma fixture for condition")
}

fn gamma_markets_json() -> &'static str {
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
}]"#
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

fn pmxt_trade_projection_fixture() -> PmxtOneOffNtProjection {
    let same_hash = "0x000000000000000000000000000000000000000000000000000000000000abcdef";
    project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
        source_binding: "synthetic-pmxt-one-off-source".to_string(),
        usage_scope: SourceProofUsageScope::OneOffBackfillData,
        selected_condition_id: "0xcondition".to_string(),
        selected_token_id: "token-a".to_string(),
        gamma_markets: gamma_markets(),
        rows: vec![
            PmxtOneOffSelectedRow::LastTrade(PmxtOneOffTradeRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                transaction_hash: same_hash.to_string(),
                price: "0.4900".to_string(),
                side: PmxtOneOffTickSide::Buy,
                size: "2.000000".to_string(),
                fee_rate_bps: "0".to_string(),
                timestamp: UnixNanos::from(1_772_023_200_123_000_000),
                ts_init: UnixNanos::from(1_772_023_200_223_000_000),
            }),
            PmxtOneOffSelectedRow::LastTrade(PmxtOneOffTradeRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                transaction_hash: same_hash.to_string(),
                price: "0.4900".to_string(),
                side: PmxtOneOffTickSide::Buy,
                size: "2.000000".to_string(),
                fee_rate_bps: "0".to_string(),
                timestamp: UnixNanos::from(1_772_023_200_123_000_000),
                ts_init: UnixNanos::from(1_772_023_200_323_000_000),
            }),
            PmxtOneOffSelectedRow::LastTrade(PmxtOneOffTradeRow {
                market: "0xcondition".to_string(),
                asset_id: "token-a".to_string(),
                transaction_hash: same_hash.to_string(),
                price: "0.5000".to_string(),
                side: PmxtOneOffTickSide::Sell,
                size: "3.000000".to_string(),
                fee_rate_bps: "0".to_string(),
                timestamp: UnixNanos::from(1_772_023_200_456_000_000),
                ts_init: UnixNanos::from(1_772_023_200_556_000_000),
            }),
        ],
    })
    .expect("project PMXT one-off trade rows")
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

fn pmxt_conversion_fingerprint() -> ConversionFingerprint {
    pmxt_conversion_fingerprint_for_hash(
        "0102068effdcdbb308d9390746afa6a75dfda1b3ba8fc3239ecdb4c74d9ae99e",
    )
}

fn pmxt_conversion_fingerprint_for_hash(accepted_object_sha256: &str) -> ConversionFingerprint {
    ConversionFingerprint {
        source_proof_id: "source-proof-pmxt-one-off".to_string(),
        source_proof_version: 1,
        accepted_object_sha256: accepted_object_sha256.to_string(),
        converter_identity: "pmxt-one-off-selected-source-l2-to-nt.v1".to_string(),
        converter_version: "1".to_string(),
        converter_config_hash: "7c5ff8475a73c3aaf3e64cc09d803ff34de9cbc51345978406125fcc5147879a"
            .to_string(),
    }
}

fn pmxt_l2_manifest(
    projection: &backtesting_vertical_slice::pmxt_one_off_backfill_projection::PmxtOneOffNtProjection,
    catalog_root: &std::path::Path,
    output_dir: &std::path::Path,
) -> BacktestingRunManifest {
    let instrument_id = binary_option_instrument_id(&projection.instrument);
    let (venue_name, settlement_currency) =
        binary_option_venue_and_currency(&projection.instrument);
    BacktestingRunManifest {
        manifest_schema_version: BACKTESTING_RUN_MANIFEST_SCHEMA_VERSION.to_string(),
        run_id: "pmxt-one-off-l2-contract-proof".to_string(),
        target_bolt_v2_branch: "main".to_string(),
        target_bolt_v2_ref: "refs/heads/main".to_string(),
        resolved_nt_version: backtesting_vertical_slice::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .expect("BVS NautilusTrader dependency provenance"),
        market_structure_fixture: MarketStructureFixture::BinaryOption,
        venue_binding_key: "synthetic-pmxt-one-off-source".to_string(),
        run_purpose: RunPurpose::Audit,
        source_proof_id: "source-proof-pmxt-one-off".to_string(),
        source_proof_version: 1,
        pins_non_latest_proof: false,
        proof_pin_reason_code: None,
        proof_pin_reason_detail: None,
        strategy: StrategySource {
            source_kind: StrategySourceKind::CompiledRustRegistry,
            registry_key: STRATEGY_HURST_VPIN_DIRECTIONAL.to_string(),
            parameters: BTreeMap::from([
                (STRATEGY_PARAM_TRADE_SIZE.to_string(), "1".to_string()),
                (
                    STRATEGY_PARAM_BAR_TYPE.to_string(),
                    format!("{instrument_id}-1-MINUTE-LAST-EXTERNAL"),
                ),
            ]),
            typed_config_uri: None,
            typed_config_hash: None,
            experiment_result_uri: None,
            experiment_result_hash: None,
            config_overlay: None,
        },
        strategy_config_hash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        venue: ManifestVenueConfig {
            nt_venue: venue_name,
            oms_type: "NETTING".to_string(),
            account_type: "CASH".to_string(),
            book_type: "L2_MBP".to_string(),
            starting_balances: vec![format!("1_000_000 {settlement_currency}")],
            routing: false,
            frozen_account: false,
            reject_stop_orders: true,
            support_gtd_orders: true,
            support_contingent_orders: true,
            use_position_ids: true,
            use_random_ids: false,
            use_reduce_only: true,
            bar_execution: true,
            bar_adaptive_high_low_ordering: false,
            trade_execution: true,
            use_market_order_acks: false,
            liquidity_consumption: false,
            allow_cash_borrowing: false,
            queue_position: false,
            oto_trigger_mode: "PARTIAL".to_string(),
            base_currency: "NONE".to_string(),
            default_leverage: "1".to_string(),
            price_protection_points: 0,
            leverages: None,
            margin_model: None,
            modules: None,
            fill_model: None,
            latency_model: None,
            fee_model: None,
            settlement_prices: None,
        },
        additional_venues: Vec::new(),
        catalog_inputs: vec![ManifestCatalogInput {
            catalog_path: catalog_root.display().to_string(),
            catalog_fs_protocol: CATALOG_FS_PROTOCOL_NONE.to_string(),
            catalog_fs_storage_options: BTreeMap::new(),
            catalog_fs_rust_storage_options: BTreeMap::new(),
            data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
            nt_instrument_id: instrument_id.to_string(),
            instrument_ids: None,
            start_time: None,
            end_time: None,
            filter_expr: None,
            client_id: None,
            metadata: None,
            bar_spec: None,
            bar_types: None,
            optimize_file_loading: None,
        }],
        reconstructed_reference_current_price: Vec::new(),
        instrument_settlements: Vec::new(),
        catalog_hash: "1111111111111111111111111111111111111111111111111111111111111111"
            .to_string(),
        execution_model: "nt_backtest_node".to_string(),
        artifact_root: format!("file://{}", output_dir.display()),
        output_prefix: format!("file://{}", output_dir.display()),
        artifact_store: ManifestArtifactStore {
            storage_options: BTreeMap::new(),
            rust_storage_options: BTreeMap::new(),
            ssm_parameters: None,
        },
        domain_metrics: Vec::new(),
        start_time: None,
        end_time: None,
    }
}

fn pmxt_result_artifact_uris(output_dir: &std::path::Path) -> ResultArtifactUris {
    let uri = |file_name: &str| format!("file://{}", output_dir.join(file_name).display());
    ResultArtifactUris {
        source_proof_uri: uri("accepted-source-proof.json"),
        canonical_table_uri: uri("selected-source.parquet"),
        nt_catalog_uri: format!("file://{}", output_dir.join("nt-catalog").display()),
        nt_catalog_manifest_uri: None,
        catalog_metadata_uri: uri(CATALOG_METADATA_FILE),
        result_contract_uri: uri("backtest-result-contract.json"),
    }
}

fn pmxt_selected_source_schema() -> PmxtSelectedSourceSchema {
    PmxtSelectedSourceSchema {
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
        last_trade_price_event_type: None,
        transaction_hash_column: None,
        fee_rate_bps_column: None,
        ignored_event_types: vec!["last_trade_price".to_string()],
        forbidden_ignored_event_types: vec!["tick_size_change".to_string()],
    }
}

fn pmxt_selected_source_schema_with_trades() -> PmxtSelectedSourceSchema {
    PmxtSelectedSourceSchema {
        last_trade_price_event_type: Some("last_trade_price".to_string()),
        transaction_hash_column: Some("transaction_hash".to_string()),
        fee_rate_bps_column: Some("fee_rate_bps".to_string()),
        ignored_event_types: Vec::new(),
        ..pmxt_selected_source_schema()
    }
}

fn write_pmxt_selected_source_fixture(path: &std::path::Path) {
    write_pmxt_selected_source_fixture_with_market_array(
        path,
        Field::new("market", DataType::Binary, false),
        Arc::new(BinaryArray::from(vec![
            Some(b"0xcondition".as_slice()),
            Some(b"0xcondition".as_slice()),
            Some(b"0xcondition".as_slice()),
        ])) as ArrayRef,
    );
}

fn write_pmxt_selected_source_with_tick_size_change_fixture(path: &std::path::Path) {
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
                    1_772_023_200_600_000_000,
                    1_772_023_200_656_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_123_000_000,
                    1_772_023_200_456_000_000,
                    1_772_023_200_500_000_000,
                    1_772_023_200_556_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(BinaryArray::from(vec![
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "price_change",
                "tick_size_change",
                "last_trade_price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "token-a", "token-a", "token-a", "token-a",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some(r#"[["0.49","10.000000"]]"#),
                None,
                None,
                None,
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some(r#"[["0.50","11.000000"]]"#),
                None,
                None,
                None,
            ])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(4900), Some(100), Some(4900)])
                    .with_precision_and_scale(9, 4)
                    .expect("price decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(12_000_000), None, Some(2_000_000)])
                    .with_precision_and_scale(18, 6)
                    .expect("size decimal"),
            ) as ArrayRef,
            Arc::new(StringArray::from(vec![
                None,
                Some("BUY"),
                None,
                Some("SELL"),
            ])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(4900), None, None])
                    .with_precision_and_scale(9, 4)
                    .expect("best bid decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(5000), None, None])
                    .with_precision_and_scale(9, 4)
                    .expect("best ask decimal"),
            ) as ArrayRef,
        ],
    )
    .expect("selected-source batch with tick-size change");
    let file = File::create(path).expect("create selected source parquet");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("parquet writer");
    writer
        .write(&batch)
        .expect("write selected source parquet with tick-size change");
    writer.close().expect("close selected source parquet");
}

fn write_pmxt_selected_source_binary_view_fixture(path: &std::path::Path) {
    write_pmxt_selected_source_fixture_with_market_array(
        path,
        Field::new("market", DataType::BinaryView, false),
        Arc::new(BinaryViewArray::from_iter_values(vec![
            b"0xcondition".as_slice(),
            b"0xcondition".as_slice(),
            b"0xcondition".as_slice(),
        ])) as ArrayRef,
    );
}

fn write_pmxt_selected_source_fixed_size_binary_fixture(
    path: &std::path::Path,
    condition_id: &str,
) {
    let condition_bytes =
        hex::decode(condition_id.trim_start_matches("0x")).expect("condition id hex bytes");
    write_pmxt_selected_source_fixture_with_market_array(
        path,
        Field::new(
            "market",
            DataType::FixedSizeBinary(condition_bytes.len() as i32),
            false,
        ),
        Arc::new(FixedSizeBinaryArray::from(vec![
            condition_bytes.as_slice(),
            condition_bytes.as_slice(),
            condition_bytes.as_slice(),
        ])) as ArrayRef,
    );
}

fn write_pmxt_selected_source_fixture_with_market_array(
    path: &std::path::Path,
    market_field: Field,
    market_array: ArrayRef,
) {
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
        market_field,
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
            market_array,
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

fn write_pmxt_selected_source_trade_fixture(path: &std::path::Path) {
    let same_hash = "0x000000000000000000000000000000000000000000000000000000000000abcdef";
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
        Field::new("transaction_hash", DataType::Utf8, false),
        Field::new("fee_rate_bps", DataType::Float64, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_223_000_000,
                    1_772_023_200_323_000_000,
                    1_772_023_200_556_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_123_000_000,
                    1_772_023_200_123_000_000,
                    1_772_023_200_456_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(BinaryArray::from(vec![
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "last_trade_price",
                "last_trade_price",
                "last_trade_price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec!["token-a", "token-a", "token-a"])) as ArrayRef,
            Arc::new(StringArray::from(vec![None::<&str>, None, None])) as ArrayRef,
            Arc::new(StringArray::from(vec![None::<&str>, None, None])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![Some(4900), Some(4900), Some(5000)])
                    .with_precision_and_scale(9, 4)
                    .expect("trade price decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![Some(2_000_000), Some(2_000_000), Some(3_000_000)])
                    .with_precision_and_scale(18, 6)
                    .expect("trade size decimal"),
            ) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some("BUY"),
                Some("BUY"),
                Some("SELL"),
            ])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, None, None])
                    .with_precision_and_scale(9, 4)
                    .expect("best bid decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, None, None])
                    .with_precision_and_scale(9, 4)
                    .expect("best ask decimal"),
            ) as ArrayRef,
            Arc::new(StringArray::from(vec![same_hash, same_hash, same_hash])) as ArrayRef,
            Arc::new(Float64Array::from(vec![0.0, 0.0, 0.0])) as ArrayRef,
        ],
    )
    .expect("selected-source trade batch");
    let file = File::create(path).expect("create selected source parquet");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("parquet writer");
    writer
        .write(&batch)
        .expect("write selected source trade parquet");
    writer.close().expect("close selected source trade parquet");
}

fn write_pmxt_selected_source_mixed_fixture(path: &std::path::Path) {
    let same_hash = "0x000000000000000000000000000000000000000000000000000000000000abcdef";
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
        Field::new("transaction_hash", DataType::Utf8, true),
        Field::new("fee_rate_bps", DataType::Utf8, true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_223_000_000,
                    1_772_023_200_556_000_000,
                    1_772_023_200_623_000_000,
                    1_772_023_200_723_000_000,
                    1_772_023_200_856_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(
                TimestampNanosecondArray::from(vec![
                    1_772_023_200_123_000_000,
                    1_772_023_200_456_000_000,
                    1_772_023_200_523_000_000,
                    1_772_023_200_523_000_000,
                    1_772_023_200_756_000_000,
                ])
                .with_timezone_utc(),
            ) as ArrayRef,
            Arc::new(BinaryArray::from(vec![
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
                Some(b"0xcondition".as_slice()),
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "price_change",
                "last_trade_price",
                "last_trade_price",
                "last_trade_price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "token-a", "token-a", "token-a", "token-a", "token-a",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some(r#"[["0.49","10.000000"]]"#),
                None,
                None,
                None,
                None,
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                Some(r#"[["0.50","11.000000"]]"#),
                None,
                None,
                None,
                None,
            ])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(4900), Some(4900), Some(4900), Some(5000)])
                    .with_precision_and_scale(9, 4)
                    .expect("mixed price decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![
                    None,
                    Some(12_000_000),
                    Some(2_000_000),
                    Some(2_000_000),
                    Some(3_000_000),
                ])
                .with_precision_and_scale(18, 6)
                .expect("mixed size decimal"),
            ) as ArrayRef,
            Arc::new(StringArray::from(vec![
                None,
                Some("BUY"),
                Some("BUY"),
                Some("BUY"),
                Some("SELL"),
            ])) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(4900), None, None, None])
                    .with_precision_and_scale(9, 4)
                    .expect("mixed best bid decimal"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(vec![None, Some(5000), None, None, None])
                    .with_precision_and_scale(9, 4)
                    .expect("mixed best ask decimal"),
            ) as ArrayRef,
            Arc::new(StringArray::from(vec![
                None,
                None,
                Some(same_hash),
                Some(same_hash),
                Some(same_hash),
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                None,
                None,
                Some("0"),
                Some("0"),
                Some("0"),
            ])) as ArrayRef,
        ],
    )
    .expect("selected-source mixed batch");
    let file = File::create(path).expect("create selected source parquet");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("parquet writer");
    writer
        .write(&batch)
        .expect("write selected source mixed parquet");
    writer.close().expect("close selected source mixed parquet");
}

fn write_selector_report_fixture(path: &std::path::Path) {
    write_selector_report_with_excluded_events_fixture(path, &["tick_size_change"]);
}

fn write_selector_report_without_excluded_events_fixture(path: &std::path::Path) {
    write_selector_report_with_excluded_events_fixture(path, &[]);
}

fn write_selector_report_with_excluded_events_fixture(
    path: &std::path::Path,
    excluded_event_families: &[&str],
) {
    let report = serde_json::json!({
        "schema_version": "first-proof-selector-report.v1",
        "selector_id": "pmxt-one-off-selector",
        "status": "selected",
        "selection": {
            "required_event_families": ["book", "price_change", "last_trade_price"],
            "excluded_event_families": excluded_event_families,
            "row_budget": 10,
            "max_selected_assets": 1
        },
        "event_count_ledger_hash": PMXT_TEST_EVENT_COUNT_LEDGER_HASH,
        "total_assets": 1,
        "eligible_assets": 1,
        "selected_assets": [{
            "asset_id": "token-a",
            "replay_rows": 3
        }],
        "selected_asset_ids_hash": PMXT_TEST_SELECTED_ASSET_IDS_HASH,
        "excluded_event_asset_count": 0,
        "excluded_event_row_count": 0,
        "blocking_issues": []
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&report).expect("selector report json"),
    )
    .expect("write selector report");
}

fn write_selected_source_report_with_selector(
    path: &std::path::Path,
    parquet_path: &std::path::Path,
    selector_report_path: &std::path::Path,
    rows: u64,
) {
    let report = SelectedSourceSliceReport {
        schema_version: "selected-source-slice-report.v1".to_string(),
        source_parquet_path: "/source/full.parquet".to_string(),
        source_parquet_sha256: "source-sha".to_string(),
        selector_report_path: selector_report_path.display().to_string(),
        selector_report_sha256: sha256_file(selector_report_path),
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
        selected_asset_ids_hash: PMXT_TEST_SELECTED_ASSET_IDS_HASH.to_string(),
        output_parquet_sha256: sha256_file(parquet_path),
    };
    let bytes = serde_json::to_vec_pretty(&report).expect("report json");
    std::fs::write(path, bytes).expect("write selected source report");
}

fn sha256_file(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).expect("read sha file");
    hex::encode(Sha256::digest(bytes))
}
