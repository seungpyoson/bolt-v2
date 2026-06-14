use std::path::{Path, PathBuf};

use backtesting_vertical_slice::research_reader::{
    CatalogQuerySpec, SqlBatchQuerySpec, query_catalog_typed, query_sql_arrow_batches,
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
        .write_to_parquet(trades, None, None, None)
        .expect("write trades");
    (temp_dir, instrument_id)
}

fn first_parquet_file(root: &Path, path_prefix: &str) -> PathBuf {
    let catalog = ParquetDataCatalog::new(root, None, None, None, None);
    let root = PathBuf::from(
        catalog
            .make_path(path_prefix, None)
            .expect("catalog data type path"),
    );
    for entry in std::fs::read_dir(&root).expect("read catalog data type dir") {
        let path = entry.expect("catalog entry").path();
        if path.is_dir() {
            if let Some(found) = first_parquet_file_if_any(&path) {
                return found;
            }
        } else if path.extension().is_some_and(|ext| ext == "parquet") {
            return path;
        }
    }
    panic!("catalog did not contain a parquet file under {path_prefix}")
}

fn first_parquet_file_if_any(root: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        if path.is_dir() {
            if let Some(found) = first_parquet_file_if_any(&path) {
                return Some(found);
            }
        } else if path.extension().is_some_and(|ext| ext == "parquet") {
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
