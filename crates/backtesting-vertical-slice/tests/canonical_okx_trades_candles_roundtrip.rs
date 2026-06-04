//! OKX `trades` and `candlesticks` converter round-trip proofs (venue slice of
//! spec 023 `1-backtesting-engine`).
//!
//! Proves the two no-order-book OKX market-data families are replayable from a
//! NautilusTrader catalog: parse the committed hermetic ZIP fixture -> build the
//! NautilusTrader type (`TradeTick` / `Bar`) -> `write_to_parquet` into a temp
//! `ParquetDataCatalog` -> `query_typed_data` back -> assert the round-tripped
//! count, ascending timestamps, and per-record payload equality match.
//!
//! Each fixture is a tiny downsampled slice of the smallest real OKX object of
//! its family, re-wrapped in the same ZIP/deflate envelope the S3 archive uses,
//! so the test exercises the full real extraction pipeline (unzip -> CSV)
//! without touching S3.

use std::fs;

use backtesting_vertical_slice::canonical_okx::{
    NT_DATA_TYPE_BAR, NT_DATA_TYPE_TRADE_TICK, OkxBarSpec, OkxInstrumentSpec,
    append_okx_candlesticks_archive, append_okx_trades_archive, extract_csv_from_zip,
    okx_bar_spec_from_rows, okx_bar_type_string, okx_candle_instruments,
    okx_candles_spec_from_rows, okx_candlesticks_to_bars, okx_trade_instruments,
    okx_trades_spec_from_rows, okx_trades_to_trade_ticks, parse_okx_candlesticks, parse_okx_trades,
    project_okx_candlesticks_archive_to_catalog, project_okx_trades_archive_to_catalog,
    read_back_bars, read_back_trade_ticks,
};
use nautilus_model::enums::{AggregationSource, BarAggregation, PriceType};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

const TRADES_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/okx/okx_trades_DOGE-USD_UM_XPERP.zip"
);
const CANDLES_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/okx/okx_candlesticks_BNB-USD_UM_XPERP.zip"
);

/// Venue-native instrument id of the trades fixture, as it appears in the
/// `instrument_name` column.
const TRADES_VENUE_INST: &str = "DOGE-USD_UM_XPERP-310404";
const TRADES_NT_INST: &str = "DOGE-USD_UM_XPERP-310404.OKX";

/// Venue-native instrument id of the candlesticks fixture.
const CANDLES_VENUE_INST: &str = "BNB-USD_UM_XPERP-310523";
const CANDLES_NT_INST: &str = "BNB-USD_UM_XPERP-310523.OKX";

fn trades_spec() -> OkxInstrumentSpec {
    OkxInstrumentSpec {
        nt_instrument_id: TRADES_NT_INST.to_string(),
        venue_inst_id: TRADES_VENUE_INST.to_string(),
        // DOGE perp: 5-dp price tick, integer contract size.
        price_increment: "0.00001".to_string(),
        size_increment: "1".to_string(),
    }
}

fn candles_spec() -> OkxInstrumentSpec {
    OkxInstrumentSpec {
        nt_instrument_id: CANDLES_NT_INST.to_string(),
        venue_inst_id: CANDLES_VENUE_INST.to_string(),
        // BNB perp candles: 1-dp price, integer contract volume.
        price_increment: "0.1".to_string(),
        size_increment: "1".to_string(),
    }
}

fn minute_bar() -> OkxBarSpec {
    OkxBarSpec {
        step: 1,
        aggregation: BarAggregation::Minute,
    }
}

fn read(path: &str) -> Vec<u8> {
    fs::read(path).expect("read OKX fixture")
}

#[test]
fn trades_fixture_extracts_and_maps() {
    let zip = read(TRADES_FIXTURE);
    let csv = extract_csv_from_zip(&zip).expect("extract trades CSV");
    let rows = parse_okx_trades(&csv, TRADES_VENUE_INST).expect("parse trades");
    assert!(!rows.is_empty(), "fixture must carry trade rows");
    // Every row is fenced to the one instrument and timestamps are ascending.
    assert!(rows.windows(2).all(|w| w[0].event_time <= w[1].event_time));

    let ticks = okx_trades_to_trade_ticks(&rows, &trades_spec()).expect("map to ticks");
    assert_eq!(ticks.len(), rows.len());
    assert!(
        ticks
            .iter()
            .all(|t| t.instrument_id.to_string() == TRADES_NT_INST)
    );
    assert!(ticks.iter().all(|t| t.price.precision == 5));
    assert!(ticks.iter().all(|t| t.size.precision == 0));
    assert!(ticks.windows(2).all(|w| w[0].ts_init <= w[1].ts_init));
}

#[test]
fn okx_trades_round_trip_through_nautilus_catalog() {
    let zip = read(TRADES_FIXTURE);
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Build the expected ticks independently from the same source.
    let csv = extract_csv_from_zip(&zip).expect("extract trades CSV");
    let rows = parse_okx_trades(&csv, TRADES_VENUE_INST).expect("parse trades");
    let expected = okx_trades_to_trade_ticks(&rows, &trades_spec()).expect("map to ticks");

    let projection = project_okx_trades_archive_to_catalog(&zip, &trades_spec(), dir.path())
        .expect("project trades to catalog");
    assert_eq!(projection.record_count, expected.len());
    assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
    assert_eq!(projection.nt_identifier, TRADES_NT_INST);
    assert_eq!(projection.price_precision, 5);
    assert_eq!(projection.size_precision, 0);
    assert!(!projection.catalog_hash.is_empty());

    let loaded = read_back_trade_ticks(dir.path(), TRADES_NT_INST).expect("read back ticks");
    assert_eq!(loaded.len(), expected.len(), "round-tripped tick count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "round-tripped trade ticks must be identical (count, ordering, payload)"
    );

    // Spot-check the native trade-tick parquet tree exists.
    assert!(
        walk(dir.path()).iter().any(|p| {
            p.to_string_lossy().contains("trade")
                && p.extension().map(|e| e == "parquet").unwrap_or(false)
        }),
        "catalog must contain a native trade-tick parquet file"
    );
}

#[test]
fn candles_fixture_extracts_and_maps() {
    let zip = read(CANDLES_FIXTURE);
    let csv = extract_csv_from_zip(&zip).expect("extract candles CSV");
    let rows = parse_okx_candlesticks(&csv, CANDLES_VENUE_INST).expect("parse candles");
    assert!(!rows.is_empty(), "fixture must carry candle rows");
    assert!(rows.windows(2).all(|w| w[0].open_time < w[1].open_time));

    let bars = okx_candlesticks_to_bars(&rows, &candles_spec(), minute_bar()).expect("map to bars");
    assert_eq!(bars.len(), rows.len());
    let bar_type = bars[0].bar_type;
    assert_eq!(bar_type.instrument_id().to_string(), CANDLES_NT_INST);
    assert_eq!(bar_type.aggregation_source(), AggregationSource::External);
    assert_eq!(bar_type.spec().price_type, PriceType::Last);
    assert_eq!(bar_type.spec().aggregation, BarAggregation::Minute);
    assert!(bars.iter().all(|b| b.open.precision == 1));
    assert!(bars.windows(2).all(|w| w[0].ts_init <= w[1].ts_init));
}

#[test]
fn okx_candlesticks_round_trip_through_nautilus_catalog() {
    let zip = read(CANDLES_FIXTURE);
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    let csv = extract_csv_from_zip(&zip).expect("extract candles CSV");
    let rows = parse_okx_candlesticks(&csv, CANDLES_VENUE_INST).expect("parse candles");
    let expected = okx_candlesticks_to_bars(&rows, &candles_spec(), minute_bar()).expect("map");

    let projection = project_okx_candlesticks_archive_to_catalog(
        &zip,
        &candles_spec(),
        minute_bar(),
        dir.path(),
    )
    .expect("project candles to catalog");
    assert_eq!(projection.record_count, expected.len());
    assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
    assert_eq!(projection.price_precision, 1);
    assert_eq!(projection.size_precision, 0);
    assert!(!projection.catalog_hash.is_empty());

    let bar_type = okx_bar_type_string(&candles_spec(), minute_bar()).expect("bar type string");
    assert_eq!(projection.nt_identifier, bar_type);

    let loaded = read_back_bars(dir.path(), &bar_type).expect("read back bars");
    assert_eq!(loaded.len(), expected.len(), "round-tripped bar count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded bars must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "round-tripped bars must be identical (count, ordering, payload)"
    );

    assert!(
        walk(dir.path()).iter().any(|p| {
            p.to_string_lossy().contains("bar")
                && p.extension().map(|e| e == "parquet").unwrap_or(false)
        }),
        "catalog must contain a native bar parquet file"
    );
}

#[test]
fn okx_trades_data_derived_append_round_trips() {
    // The bulk path: derive precision from the object's own rows (OKX stages no
    // instrument universe), append into a shared catalog with no clean-root
    // guard, and prove the NautilusTrader round-trip is lossless.
    let zip = read(TRADES_FIXTURE);
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Independent expectation from the same source, via the data-derived spec.
    let csv = extract_csv_from_zip(&zip).expect("extract trades CSV");
    assert_eq!(
        okx_trade_instruments(&csv).expect("enumerate instruments"),
        vec![TRADES_VENUE_INST.to_string()],
        "fixture carries exactly one venue instrument"
    );
    let rows = parse_okx_trades(&csv, TRADES_VENUE_INST).expect("parse trades");
    let derived = okx_trades_spec_from_rows(&rows, TRADES_VENUE_INST).expect("derive spec");
    assert_eq!(derived.nt_instrument_id, TRADES_NT_INST);
    let expected = okx_trades_to_trade_ticks(&rows, &derived).expect("map to ticks");

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries = append_okx_trades_archive(&zip, &mut catalog).expect("append trades");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].nt_instrument_id, TRADES_NT_INST);
    assert_eq!(summaries[0].record_count, rows.len());
    // DOGE perp prints at a 5-dp price tick; precision is read from the data.
    assert_eq!(summaries[0].price_precision, 5);
    // Size precision is whatever the source rendered (self-consistent with the
    // ticks built from the same derived spec), not a hardcoded assumption.
    assert_eq!(summaries[0].price_precision, expected[0].price.precision);
    assert_eq!(summaries[0].size_precision, expected[0].size.precision);

    let loaded = read_back_trade_ticks(dir.path(), TRADES_NT_INST).expect("read back ticks");
    assert_eq!(loaded.len(), expected.len(), "round-tripped tick count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived append must round-trip identically (count, ordering, payload, precision)"
    );
}

#[test]
fn okx_candlesticks_data_derived_append_round_trips() {
    // The bulk path: derive both the price/size precision AND the bar period
    // from the object's own rows (OKX stages no instrument universe and no
    // interval in the key), append into a shared catalog with no clean-root
    // guard, and prove the NautilusTrader round-trip is lossless.
    let zip = read(CANDLES_FIXTURE);
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Independent expectation from the same source, via the data-derived spec
    // and the data-derived bar interval.
    let csv = extract_csv_from_zip(&zip).expect("extract candles CSV");
    assert_eq!(
        okx_candle_instruments(&csv).expect("enumerate instruments"),
        vec![CANDLES_VENUE_INST.to_string()],
        "fixture carries exactly one venue instrument"
    );
    let rows = parse_okx_candlesticks(&csv, CANDLES_VENUE_INST).expect("parse candles");
    let derived = okx_candles_spec_from_rows(&rows, CANDLES_VENUE_INST).expect("derive spec");
    assert_eq!(derived.nt_instrument_id, CANDLES_NT_INST);
    let derived_bar = okx_bar_spec_from_rows(&rows).expect("derive bar interval");
    // The fixture's open_time spacing is a 1-minute candle; the interval is
    // read from the data, not assumed.
    assert_eq!(derived_bar, minute_bar());
    let expected = okx_candlesticks_to_bars(&rows, &derived, derived_bar).expect("map to bars");

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries = append_okx_candlesticks_archive(&zip, &mut catalog).expect("append candles");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].nt_instrument_id, CANDLES_NT_INST);
    assert_eq!(summaries[0].record_count, rows.len());
    // Precision is read from the data and is self-consistent with the bars built
    // from the same derived spec — not a hardcoded assumption.
    assert_eq!(summaries[0].price_precision, expected[0].open.precision);
    assert_eq!(summaries[0].size_precision, expected[0].volume.precision);

    // Read back by the data-derived bar-type identifier.
    let bar_type = okx_bar_type_string(&derived, derived_bar).expect("bar type string");
    let loaded = read_back_bars(dir.path(), &bar_type).expect("read back bars");
    assert_eq!(loaded.len(), expected.len(), "round-tripped bar count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded bars must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived append must round-trip identically (count, ordering, payload, precision)"
    );
}

#[test]
fn projections_refuse_dirty_catalog_root() {
    let trades_zip = read(TRADES_FIXTURE);
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
    let err = project_okx_trades_archive_to_catalog(&trades_zip, &trades_spec(), dir.path())
        .expect_err("dirty catalog root must be refused");
    assert!(err.to_string().contains("not empty"), "{err}");
}

/// Recursively collect every file under `root`.
fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
