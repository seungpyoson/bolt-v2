use std::path::{Path, PathBuf};

use backtesting_vertical_slice::research_reader::{
    AnalyticsSourceBinding, CatalogQuerySpec, CustomUiDecision, FeatureJoinSpec,
    NotebookBiSurfaceSpec, NotebookErgonomics, NotebookQueryEngine, SqlBatchQuerySpec,
    build_notebook_bi_surface, query_catalog_typed, query_sql_arrow_batches,
    validate_feature_join_bindings,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::AggressorSide,
    identifiers::{InstrumentId, Symbol, TradeId, Venue},
    instruments::{CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::{CatalogPathPrefix, ParquetDataCatalog};
use tempfile::TempDir;

const VENUE_NAME: &str = "SIM";
const RAW_SYMBOL: &str = "BTCUSDT";
const BASE_CURRENCY: &str = "BTC";
const QUOTE_CURRENCY: &str = "USDT";
const PRICE_PRECISION: u8 = 2;
const SIZE_PRECISION: u8 = 3;

fn proof_instrument() -> CurrencyPair {
    let instrument_id = InstrumentId::new(Symbol::from(RAW_SYMBOL), Venue::from(VENUE_NAME));
    CurrencyPair::new(
        instrument_id,
        Symbol::from(RAW_SYMBOL),
        Currency::from(BASE_CURRENCY),
        Currency::from(QUOTE_CURRENCY),
        PRICE_PRECISION,
        SIZE_PRECISION,
        Price::new(0.01, PRICE_PRECISION),
        Quantity::new(0.001, SIZE_PRECISION),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // tick_scheme (NT bump)
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
}

fn trade(instrument_id: InstrumentId, index: usize, ts: u64) -> TradeTick {
    let aggressor = if index % 2 == 0 {
        AggressorSide::Buyer
    } else {
        AggressorSide::Seller
    };
    let ts = UnixNanos::from(ts);
    TradeTick::new(
        instrument_id,
        Price::new(50_000.0 + index as f64, PRICE_PRECISION),
        Quantity::new(0.500, SIZE_PRECISION),
        aggressor,
        TradeId::from(format!("research-reader-{index}").as_str()),
        ts,
        ts,
    )
}

fn write_catalog() -> (TempDir, InstrumentId) {
    let temp_dir = TempDir::new().expect("temp dir");
    let instrument = proof_instrument();
    let instrument_id = instrument.id();
    let trades = vec![
        trade(instrument_id, 0, 1_000),
        trade(instrument_id, 1, 2_000),
        trade(instrument_id, 2, 3_000),
    ];
    let mut catalog = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CurrencyPair(instrument)])
        .expect("write instrument");
    catalog
        .write_to_parquet(&trades, None, None, None)
        .expect("write trades");
    (temp_dir, instrument_id)
}

fn first_parquet_file(root: &Path, path_prefix: &str) -> PathBuf {
    let catalog = ParquetDataCatalog::new(root, None, None, None, None);
    let preferred_root = PathBuf::from(
        catalog
            .make_path(path_prefix, None)
            .expect("catalog data type path"),
    );
    if let Some(found) = first_parquet_file_if_any(&preferred_root) {
        return found;
    }

    let data_kind_token = catalog_data_kind_token(path_prefix);
    if let Some(found) = first_parquet_file_matching(root, &data_kind_token) {
        return found;
    }

    panic!("catalog did not contain a parquet file under {path_prefix}")
}

fn catalog_data_kind_token(path_prefix: &str) -> String {
    path_prefix
        .split(['_', '-'])
        .find(|part| !part.is_empty())
        .unwrap_or(path_prefix)
        .to_ascii_lowercase()
}

fn sorted_dir_entries(root: &Path) -> Option<Vec<PathBuf>> {
    let mut entries = std::fs::read_dir(root)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Vec<_>>();
    entries.sort();
    Some(entries)
}

fn is_parquet(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "parquet")
}

fn first_parquet_file_matching(root: &Path, data_kind_token: &str) -> Option<PathBuf> {
    for path in sorted_dir_entries(root)? {
        if path.is_dir() {
            if let Some(found) = first_parquet_file_matching(&path, data_kind_token) {
                return Some(found);
            }
        } else if is_parquet(&path)
            && path
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(data_kind_token)
        {
            return Some(path);
        }
    }
    None
}

fn first_parquet_file_if_any(root: &Path) -> Option<PathBuf> {
    for path in sorted_dir_entries(root)? {
        if path.is_dir() {
            if let Some(found) = first_parquet_file_if_any(&path) {
                return Some(found);
            }
        } else if is_parquet(&path) {
            return Some(path);
        }
    }
    None
}

#[test]
fn typed_reader_delegates_to_nt_catalog_query() {
    let (catalog, instrument_id) = write_catalog();
    let spec = CatalogQuerySpec {
        catalog_uri: catalog.path().to_string_lossy().to_string(),
        storage_options: None,
        instrument_ids: Some(vec![instrument_id.to_string()]),
        start: None,
        end: None,
        where_clause: Some("ts_init >= 2000".to_string()),
        files: None,
        optimize_file_loading: true,
    };

    let rows: Vec<TradeTick> = query_catalog_typed(&spec).expect("query typed rows");

    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.instrument_id == instrument_id));
    assert!(rows.iter().all(|row| row.ts_init.as_u64() >= 2_000));
}

#[test]
fn sql_reader_delegates_to_data_backend_session_for_arrow_batches() {
    let (catalog, _instrument_id) = write_catalog();
    let parquet_file = first_parquet_file(catalog.path(), TradeTick::path_prefix());
    let spec = SqlBatchQuerySpec {
        table_name: "trade_ticks".to_string(),
        file_path: parquet_file,
        sql: Some("SELECT * FROM trade_ticks WHERE ts_init >= 2000 ORDER BY ts_init".to_string()),
        chunk_size: 16,
    };

    let batches = query_sql_arrow_batches(&spec).expect("query Arrow batches");
    let row_count: usize = batches.iter().map(|batch| batch.num_rows()).sum();

    assert_eq!(row_count, 2);
}

#[test]
fn notebook_bi_surface_exposes_duckdb_and_polars_over_nt_catalog_arrow_without_custom_ui() {
    let artifact_root = "s3://example-bucket/nt-research-analytics";
    let surface = build_notebook_bi_surface(NotebookBiSurfaceSpec {
        artifact_root: artifact_root.to_string(),
        nt_catalog_arrow_uri:
            "s3://example-bucket/nt-research-analytics/nt-catalog/v1/projection=research-proof/data/trade_tick/part-0.parquet"
                .to_string(),
        query_engines: vec![
            NotebookQueryEngine {
                engine_key: "duckdb".to_string(),
                reads_nt_catalog_arrow: true,
                read_only: true,
            },
            NotebookQueryEngine {
                engine_key: "polars".to_string(),
                reads_nt_catalog_arrow: true,
                read_only: true,
            },
        ],
        dashboard_product_refs: vec![
            "dashboard-product-gate:sql-bi:v1".to_string(),
            "dashboard-product-gate:notebook-adjacent:v1".to_string(),
        ],
        notebook: NotebookErgonomics {
            read_only: true,
            exposes_arrow_batches: true,
            exposes_sql_examples: true,
            mutation_actions_enabled: false,
        },
        custom_ui: CustomUiDecision::NotSelected,
    })
    .expect("BI surface should validate");

    assert_eq!(surface.artifact_root, artifact_root);
    assert_eq!(
        surface
            .query_engines
            .iter()
            .map(|engine| engine.engine_key.as_str())
            .collect::<Vec<_>>(),
        vec!["duckdb", "polars"]
    );
    assert_eq!(surface.custom_ui, CustomUiDecision::NotSelected);
    assert!(!surface.notebook.mutation_actions_enabled);
}

#[test]
fn notebook_bi_surface_requires_product_gate_before_custom_ui() {
    let mut spec = NotebookBiSurfaceSpec {
        artifact_root: "s3://example-bucket/nt-research-analytics".to_string(),
        nt_catalog_arrow_uri:
            "s3://example-bucket/nt-research-analytics/nt-catalog/v1/projection=research-proof/data/trade_tick/part-0.parquet"
                .to_string(),
        query_engines: vec![NotebookQueryEngine {
            engine_key: "duckdb".to_string(),
            reads_nt_catalog_arrow: true,
            read_only: true,
        }],
        dashboard_product_refs: vec!["dashboard-product-gate:sql-bi:v1".to_string()],
        notebook: NotebookErgonomics {
            read_only: true,
            exposes_arrow_batches: true,
            exposes_sql_examples: true,
            mutation_actions_enabled: false,
        },
        custom_ui: CustomUiDecision::AllowedAfterProductGate {
            confirmed_requirement_refs: Vec::new(),
            rejected_product_refs: vec!["dashboard-product-gate:sql-bi:v1".to_string()],
        },
    };

    let err = build_notebook_bi_surface(spec.clone())
        .expect_err("custom UI needs confirmed requirement evidence");
    assert!(err.to_string().contains("confirmed requirement"), "{err}");

    spec.custom_ui = CustomUiDecision::AllowedAfterProductGate {
        confirmed_requirement_refs: vec!["dashboard-requirement:non-tabular-visual:v1".to_string()],
        rejected_product_refs: Vec::new(),
    };
    let err =
        build_notebook_bi_surface(spec).expect_err("custom UI needs rejected product evidence");
    assert!(err.to_string().contains("product"), "{err}");
}

#[test]
fn analytics_feature_joins_use_source_binding_keys_not_venue_or_provider_literals() {
    let bindings = vec![
        AnalyticsSourceBinding {
            source_binding_key: "primary-market-trades".to_string(),
            venue_key: "venue-alpha".to_string(),
            provider_key: "provider-alpha".to_string(),
        },
        AnalyticsSourceBinding {
            source_binding_key: "reference-market-features".to_string(),
            venue_key: "venue-beta".to_string(),
            provider_key: "provider-beta".to_string(),
        },
    ];
    let joins = vec![FeatureJoinSpec {
        left_source_binding_key: "primary-market-trades".to_string(),
        right_source_binding_key: "reference-market-features".to_string(),
        as_of_column: "event_time".to_string(),
        freshness_column: "available_at".to_string(),
    }];

    validate_feature_join_bindings(&bindings, &joins)
        .expect("feature joins should resolve through source binding keys");

    let venue_literal_join = vec![FeatureJoinSpec {
        left_source_binding_key: "venue-alpha".to_string(),
        right_source_binding_key: "reference-market-features".to_string(),
        as_of_column: "event_time".to_string(),
        freshness_column: "available_at".to_string(),
    }];
    let err = validate_feature_join_bindings(&bindings, &venue_literal_join)
        .expect_err("join must not resolve through venue literals");
    assert!(err.to_string().contains("source_binding_key"), "{err}");
}
