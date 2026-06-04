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
    append_okx_candlesticks_archive, append_okx_candlesticks_csv, append_okx_trades_archive,
    extract_csv_from_zip, okx_bar_spec_from_open_times, okx_bar_spec_from_rows,
    okx_bar_type_string, okx_candle_instruments, okx_candles_spec_from_rows,
    okx_candlesticks_to_bars, okx_trade_instruments, okx_trades_spec_from_rows,
    okx_trades_to_trade_ticks, parse_okx_candlesticks, parse_okx_trades,
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

// ===========================================================================
// Object-scoped bar-period derivation (regression guard for the RUN2
// okx/candlesticks EXIT=1: an illiquid single-minute OPTION strike in a
// multi-instrument option-chain object made the per-instrument period
// derivation bail and aborted the whole object). The bar period is a
// per-OBJECT property; an instrument carrying one bar inherits the object's
// proven period instead of being unprovable on its own.
// ===========================================================================

/// Candle CSV header, matching `OKX_CANDLES_HEADER`.
const CANDLES_CSV_HEADER: &str =
    "instrument_name,open,high,low,close,vol,vol_ccy,vol_quote,open_time,confirm";

/// A busy option strike that trades every minute (multi-bar stream).
const BUSY_STRIKE: &str = "BTC-USD-260308-58000-P";
/// An illiquid option strike that trades in a single minute (one-bar stream).
const SINGLE_BAR_STRIKE: &str = "BTC-USD-260308-77000-C";

/// First bar open (minute-aligned) in milliseconds, and the one-minute step.
const CANDLE_T0_MS: i64 = 1_772_323_200_000;
const ONE_MINUTE_MS: i64 = 60_000;

/// One confirmed 1-dp candle row. `vol_ccy`/`vol_quote` are unused by the parser
/// and carry placeholders only to satisfy the fixed column count.
fn candle_csv_row(inst: &str, open_time_ms: i64) -> String {
    format!("{inst},100.0,101.0,99.0,100.5,10,0,0,{open_time_ms},1")
}

/// A multi-instrument 1-minute option-chain candlesticks CSV in which one strike
/// is illiquid and carries exactly one bar (a single distinct open_time) while
/// the busy strike carries three — the object shape that aborted RUN2.
fn option_chain_csv_with_single_bar_strike() -> String {
    let mut lines = vec![CANDLES_CSV_HEADER.to_string()];
    lines.push(candle_csv_row(BUSY_STRIKE, CANDLE_T0_MS));
    lines.push(candle_csv_row(BUSY_STRIKE, CANDLE_T0_MS + ONE_MINUTE_MS));
    lines.push(candle_csv_row(
        BUSY_STRIKE,
        CANDLE_T0_MS + 2 * ONE_MINUTE_MS,
    ));
    lines.push(candle_csv_row(
        SINGLE_BAR_STRIKE,
        CANDLE_T0_MS + ONE_MINUTE_MS,
    ));
    lines.join("\n")
}

#[test]
fn okx_candlesticks_option_chain_with_single_bar_strike_appends_all_instruments() {
    // The bar period is a per-OBJECT property: the busy strike proves a
    // 1-minute period for the whole object, and the single-bar strike inherits
    // it instead of aborting the object (RUN2 EXIT=1 "cannot derive OKX candle
    // interval from fewer than two distinct bar-open times").
    let csv = option_chain_csv_with_single_bar_strike();
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);

    let summaries =
        append_okx_candlesticks_csv(&csv, &mut catalog).expect("append option-chain candles");

    assert_eq!(summaries.len(), 2, "one summary per distinct instrument");
    let single = summaries
        .iter()
        .find(|s| s.nt_instrument_id == format!("{SINGLE_BAR_STRIKE}.OKX"))
        .expect("single-bar strike must be appended, not dropped");
    assert_eq!(
        single.record_count, 1,
        "single-bar strike yields exactly one bar"
    );
    let busy = summaries
        .iter()
        .find(|s| s.nt_instrument_id == format!("{BUSY_STRIKE}.OKX"))
        .expect("busy strike must be appended");
    assert_eq!(busy.record_count, 3, "busy strike yields three bars");
}

#[test]
fn okx_object_bar_spec_derived_from_union_not_per_instrument() {
    // `open_time`s are nanoseconds (as `OkxCandleRow::open_time` is). The lone
    // strike alone (one open) cannot prove a period, but the object-level union
    // of every instrument's opens still resolves the 1-minute interval — the
    // derivation scope is the whole object, not a single instrument.
    const T0_NS: i64 = CANDLE_T0_MS * 1_000_000;
    const MIN_NS: i64 = ONE_MINUTE_MS * 1_000_000;
    let union = [T0_NS, T0_NS + MIN_NS, T0_NS + 2 * MIN_NS, T0_NS + MIN_NS];
    assert_eq!(
        okx_bar_spec_from_open_times(&union).expect("derive object period"),
        minute_bar(),
    );
    assert!(
        okx_bar_spec_from_open_times(&[T0_NS + MIN_NS]).is_err(),
        "a single open cannot prove a period on its own"
    );
}

#[test]
fn okx_single_bar_strike_roundtrips_at_object_period() {
    let csv = option_chain_csv_with_single_bar_strike();
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    append_okx_candlesticks_csv(&csv, &mut catalog).expect("append option-chain candles");

    // The single-bar strike inherits the object's 1-minute period. Read it back
    // by that bar-type identifier and prove it is exactly the one expected bar
    // (count, ordering, payload, precision) — the inherited period was applied,
    // not fabricated.
    let rows = parse_okx_candlesticks(&csv, SINGLE_BAR_STRIKE).expect("parse single strike");
    assert_eq!(rows.len(), 1, "single-bar strike has exactly one row");
    let spec = okx_candles_spec_from_rows(&rows, SINGLE_BAR_STRIKE).expect("derive spec");
    let expected = okx_candlesticks_to_bars(&rows, &spec, minute_bar()).expect("map to bars");
    let bar_type = okx_bar_type_string(&spec, minute_bar()).expect("bar type string");
    let loaded = read_back_bars(dir.path(), &bar_type).expect("read back single-strike bars");
    assert_eq!(loaded.len(), 1, "exactly one bar for the single-bar strike");
    assert_eq!(
        loaded, expected,
        "single-bar strike round-trips at the object's 1-minute period"
    );
}

#[test]
fn okx_object_with_only_one_distinct_open_across_all_instruments_still_fails_loud() {
    // Even with several rows, a single DISTINCT open across the whole object
    // cannot prove a period — the >=2-distinct guard must not be relaxed when
    // moving derivation to object scope.
    const T0_NS: i64 = CANDLE_T0_MS * 1_000_000;
    let err = okx_bar_spec_from_open_times(&[T0_NS, T0_NS, T0_NS])
        .expect_err("a single distinct open cannot prove a period");
    assert!(
        err.to_string()
            .contains("fewer than two distinct bar-open times"),
        "{err}"
    );
}
