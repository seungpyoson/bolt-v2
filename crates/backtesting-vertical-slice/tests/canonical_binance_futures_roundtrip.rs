//! Round-trip proofs for the Binance **futures** venue converters.
//!
//! Covers the futures market-data families that map onto NautilusTrader
//! tradable market-data types:
//!
//! - `aggTrades`       -> `TradeTick`
//! - `markPriceKlines` -> `Bar` (mark price, a tradable reference)
//!
//! `indexPriceKlines`, `premiumIndexKlines`, funding, open interest, and
//! metadata are NOT tradable market data and are deliberately kept as staged
//! Parquet (Python-native research), never converted to NT catalog types.
//!
//! Each test reads a committed decompressed-CSV fixture (the unzip is the ingest
//! step, matching the module contract), normalizes it, projects it into a
//! NautilusTrader `ParquetDataCatalog` via NautilusTrader's own
//! `write_to_parquet`, then reads it back with `query_typed_data`. Asserting the
//! round-tripped count, ascending `ts_event`, and per-record payload equality
//! proves the data is in an NT catalog NautilusTrader can replay.
//!
//! Hermetic: fixtures live under `tests/fixtures/binance/`; no test touches S3.

use std::{fs, path::PathBuf, str::FromStr};

use backtesting_vertical_slice::canonical_binance::{
    BinanceInstrumentIdentity, BinanceInstrumentSpec, BinanceProvenance, KlineBarSpec,
    NT_DATA_TYPE_BAR, NT_DATA_TYPE_TRADE_TICK, append_binance_futures_agg_trades_archive,
    append_binance_futures_mark_price_klines_archive, binance_bar_type_string,
    normalize_binance_agg_trades, normalize_binance_price_feed_klines, project_klines_to_catalog,
    project_trades_to_catalog, read_back_bars, read_back_trade_ticks,
};
use nautilus_model::{
    enums::{AggregationSource, AggressorSide, BarAggregation, PriceType},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

fn fixture(name: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "binance",
        name,
    ]
    .iter()
    .collect();
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

fn provenance(family: &str) -> BinanceProvenance {
    BinanceProvenance {
        ingest_run_id: "binance-futures-roundtrip-test".to_string(),
        source_binding: format!("binance-futures-{family}"),
        venue: "binance".to_string(),
        product_family: "futures_um".to_string(),
        product_category: "perpetual".to_string(),
        source_proof_id: format!("source-proof-binance-futures-{family}-fixture"),
        payload_hash: "fixture-decompressed-csv".to_string(),
        archive_date: "2026-03".to_string(),
    }
}

fn identity(symbol: &str) -> BinanceInstrumentIdentity {
    BinanceInstrumentIdentity {
        instrument_id: symbol.to_string(),
        venue_symbol: symbol.to_string(),
        nt_instrument_id: format!("{symbol}.BINANCE"),
    }
}

// Source price/qty carry up to 8 decimals; an 8-decimal increment represents
// them exactly. Built from the accepted instrument universe in production;
// literal here only because this is a test fixture.
fn spec(symbol: &str, base: &str, quote: &str) -> BinanceInstrumentSpec {
    BinanceInstrumentSpec {
        nt_instrument_id: format!("{symbol}.BINANCE"),
        raw_symbol: symbol.to_string(),
        base_currency: base.to_string(),
        quote_currency: quote.to_string(),
        price_increment: "0.00000001".to_string(),
        size_increment: "0.00000001".to_string(),
        min_quantity: "0.00000001".to_string(),
        max_quantity: "9000000.00000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "9000000".to_string(),
    }
}

#[test]
fn agg_trades_fixture_round_trips_through_nt_catalog() {
    let symbol = "ETHUSDT_260925";
    let nt_id = format!("{symbol}.BINANCE");
    let csv = fixture("ETHUSDT_260925-aggTrades-2026-03.csv");
    let table = normalize_binance_agg_trades(&provenance("aggTrades"), &identity(symbol), &csv)
        .expect("normalize aggTrades");
    assert!(!table.rows.is_empty(), "fixture must have rows");

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_trades_to_catalog(&table, &spec(symbol, "ETH", "USDT"), dir.path())
        .expect("project aggTrades");
    assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
    assert_eq!(projection.nt_instrument_id, nt_id);
    assert_eq!(projection.record_count, table.rows.len());

    let loaded = read_back_trade_ticks(dir.path(), &nt_id).expect("read back");
    assert_eq!(
        loaded.len(),
        table.rows.len(),
        "round-tripped trade count must match canonical row count"
    );

    let mut previous_ts = 0u64;
    for (i, tick) in loaded.iter().enumerate() {
        let row = &table.rows[i];
        assert_eq!(tick.instrument_id.to_string(), nt_id);
        assert_eq!(
            tick.trade_id.to_string(),
            row.trade_id,
            "trade id (agg_trade_id) order must be preserved at index {i}"
        );
        let expected_aggressor = match row.aggressor_side.as_str() {
            "BUYER" => AggressorSide::Buyer,
            "SELLER" => AggressorSide::Seller,
            other => panic!("unexpected aggressor {other}"),
        };
        assert_eq!(tick.aggressor_side, expected_aggressor, "aggressor at {i}");
        // Payload equality: price and size match the source values exactly
        // (compared numerically because `Price`/`Quantity` Display trims
        // trailing zeros).
        assert_eq!(
            tick.price,
            Price::from_str(&row.price).expect("parse price"),
            "price at {i}"
        );
        assert_eq!(
            tick.size,
            Quantity::from_str(&row.size).expect("parse size"),
            "size at {i}"
        );
        let ts = tick.ts_event.as_u64();
        assert_eq!(
            ts,
            u64::try_from(row.event_time).unwrap(),
            "event_time (ms->ns) at {i}"
        );
        assert!(ts >= previous_ts, "ascending order at {i}");
        previous_ts = ts;
    }
}

/// Body for the mark-price kline family. Asserts count, ascending `ts_event`,
/// and per-record OHLC payload equality through an NT catalog round trip.
fn assert_price_feed_klines_round_trip(
    fixture_name: &str,
    symbol: &str,
    base: &str,
    quote: &str,
    family: &str,
) {
    let nt_id = format!("{symbol}.BINANCE");
    let csv = fixture(fixture_name);
    let bar_spec = KlineBarSpec {
        step: 1,
        aggregation: BarAggregation::Minute,
    };
    let table =
        normalize_binance_price_feed_klines(&provenance(family), &identity(symbol), bar_spec, &csv)
            .unwrap_or_else(|e| panic!("normalize {family}: {e}"));
    assert!(!table.rows.is_empty(), "fixture must have rows");

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_klines_to_catalog(&table, &spec(symbol, base, quote), dir.path())
        .unwrap_or_else(|e| panic!("project {family}: {e}"));
    assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
    assert_eq!(projection.nt_instrument_id, nt_id);
    assert_eq!(projection.record_count, table.rows.len());

    let loaded = read_back_bars(dir.path(), &nt_id).expect("read back bars");
    assert_eq!(
        loaded.len(),
        table.rows.len(),
        "round-tripped bar count must match canonical row count"
    );

    let mut previous_ts = 0u64;
    for (i, bar) in loaded.iter().enumerate() {
        let row = &table.rows[i];
        assert_eq!(bar.instrument_id().to_string(), nt_id);
        // Payload equality: every OHLC value survives the round trip exactly at
        // instrument precision (compared numerically because `Price` Display
        // trims trailing zeros).
        assert_eq!(
            bar.open,
            Price::from_str(&row.open).expect("parse open"),
            "open at {i}"
        );
        assert_eq!(
            bar.high,
            Price::from_str(&row.high).expect("parse high"),
            "high at {i}"
        );
        assert_eq!(
            bar.low,
            Price::from_str(&row.low).expect("parse low"),
            "low at {i}"
        );
        assert_eq!(
            bar.close,
            Price::from_str(&row.close).expect("parse close"),
            "close at {i}"
        );
        assert_eq!(
            bar.volume,
            Quantity::from_str(&row.volume).expect("parse volume"),
            "volume at {i}"
        );
        assert!(bar.high >= bar.low, "OHLC ordering survives at {i}");
        let ts = bar.ts_event.as_u64();
        assert_eq!(
            ts,
            u64::try_from(row.close_time).unwrap(),
            "bar close_time (ms->ns) at {i}"
        );
        assert!(ts >= previous_ts, "ascending bar order at {i}");
        previous_ts = ts;
    }
}

#[test]
fn mark_price_klines_fixture_round_trips_through_nt_catalog() {
    assert_price_feed_klines_round_trip(
        "ETHUSDT_260925-markPriceKlines-1m-2026-03.csv",
        "ETHUSDT_260925",
        "ETH",
        "USDT",
        "markPriceKlines",
    );
}

// ---------------------------------------------------------------------------
// Bulk-append path (data-derived precision + key-derived identity/provenance)
// ---------------------------------------------------------------------------

const BULK_SYMBOL: &str = "ETHUSDT_260925";
const BULK_NT_INST: &str = "ETHUSDT_260925.BINANCE";
const BULK_RUN: &str = "binance-bulk-roundtrip-test";

/// A staged S3 object key in the real Binance staging layout (see
/// `scripts/backfill_binance_to_s3.py::s3_uri_for_payload`). The bulk path reads
/// the instrument symbol and provenance from this key, since the CSV rows carry
/// no instrument column. `<hash>` stands in for the object payload hash; only the
/// `symbol=`, `product=`, and `dt=` segments are load-bearing for identity and
/// provenance.
fn agg_trades_object_key() -> String {
    format!(
        "raw/v1/source=data.binance.vision/product=futures_um/frequency=monthly/\
         family=aggTrades/symbol={BULK_SYMBOL}/dt=2026-03/object=fixturehash.zip"
    )
}

fn mark_klines_object_key() -> String {
    format!(
        "raw/v1/source=data.binance.vision/product=futures_um/frequency=monthly/\
         family=markPriceKlines/symbol={BULK_SYMBOL}/interval=1m/dt=2026-03/object=fixturehash.zip"
    )
}

#[test]
fn agg_trades_data_derived_append_round_trips() {
    // The bulk path: identity from the object key (the CSV has no instrument
    // column), precision derived from the object's own rows, appended into a
    // shared catalog with no clean-root guard. Prove the NautilusTrader round
    // trip is lossless.
    let csv = fixture("ETHUSDT_260925-aggTrades-2026-03.csv");
    let key = agg_trades_object_key();
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Independent expectation from the same source, via the same normalize fn
    // and the same data-derived precision the append path uses (price: 2 dp,
    // qty: 3 dp in this fixture — read from the data, never assumed).
    let table =
        normalize_binance_agg_trades(&provenance("aggTrades"), &identity(BULK_SYMBOL), &csv)
            .expect("normalize aggTrades");
    assert!(!table.rows.is_empty(), "fixture must carry rows");

    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summary = append_binance_futures_agg_trades_archive(&csv, &key, BULK_RUN, &mut catalog)
        .expect("append aggTrades");
    assert_eq!(summary.nt_instrument_id, BULK_NT_INST);
    assert_eq!(summary.data_type, NT_DATA_TYPE_TRADE_TICK);
    assert_eq!(summary.record_count, table.rows.len());

    let loaded = read_back_trade_ticks(dir.path(), BULK_NT_INST).expect("read back ticks");
    assert_eq!(loaded.len(), table.rows.len(), "round-tripped tick count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    // Precision is whatever the source rendered, self-consistent with the loaded
    // ticks — not a hardcoded assumption.
    assert!(
        loaded
            .iter()
            .all(|t| t.price.precision == summary.price_precision)
    );
    assert!(
        loaded
            .iter()
            .all(|t| t.size.precision == summary.size_precision)
    );

    // Per-record payload equality against the canonical source rows.
    for (i, tick) in loaded.iter().enumerate() {
        let row = &table.rows[i];
        assert_eq!(tick.instrument_id.to_string(), BULK_NT_INST);
        assert_eq!(tick.trade_id.to_string(), row.trade_id, "trade id at {i}");
        let expected_aggressor = match row.aggressor_side.as_str() {
            "BUYER" => AggressorSide::Buyer,
            "SELLER" => AggressorSide::Seller,
            other => panic!("unexpected aggressor {other}"),
        };
        assert_eq!(tick.aggressor_side, expected_aggressor, "aggressor at {i}");
        assert_eq!(
            tick.price,
            Price::from_str(&row.price).expect("parse price"),
            "price at {i}"
        );
        assert_eq!(
            tick.size,
            Quantity::from_str(&row.size).expect("parse size"),
            "size at {i}"
        );
        assert_eq!(
            tick.ts_event.as_u64(),
            u64::try_from(row.event_time).unwrap(),
            "event_time at {i}"
        );
    }
}

#[test]
fn mark_price_klines_data_derived_append_round_trips() {
    // The bulk path for the mark-price kline family: identity + bar interval from
    // the object key, precision from the rows, no clean-root guard.
    let csv = fixture("ETHUSDT_260925-markPriceKlines-1m-2026-03.csv");
    let key = mark_klines_object_key();
    let bar_spec = KlineBarSpec {
        step: 1,
        aggregation: BarAggregation::Minute,
    };
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    let table = normalize_binance_price_feed_klines(
        &provenance("markPriceKlines"),
        &identity(BULK_SYMBOL),
        bar_spec,
        &csv,
    )
    .expect("normalize markPriceKlines");
    assert!(!table.rows.is_empty(), "fixture must carry rows");

    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summary = append_binance_futures_mark_price_klines_archive(
        &csv,
        &key,
        BULK_RUN,
        bar_spec,
        &mut catalog,
    )
    .expect("append markPriceKlines");
    assert_eq!(summary.nt_instrument_id, BULK_NT_INST);
    assert_eq!(summary.data_type, NT_DATA_TYPE_BAR);
    assert_eq!(summary.record_count, table.rows.len());

    let bar_type = binance_bar_type_string(BULK_NT_INST, bar_spec).expect("bar type string");
    let loaded = read_back_bars(dir.path(), &bar_type).expect("read back bars");
    assert_eq!(loaded.len(), table.rows.len(), "round-tripped bar count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded bars must be ascending"
    );
    assert!(
        loaded
            .iter()
            .all(|b| b.open.precision == summary.price_precision)
    );

    // Bar type is EXTERNAL-sourced LAST-price at the key-supplied step/unit.
    let bt = loaded[0].bar_type;
    assert_eq!(bt.instrument_id().to_string(), BULK_NT_INST);
    assert_eq!(bt.aggregation_source(), AggregationSource::External);
    assert_eq!(bt.spec().price_type, PriceType::Last);
    assert_eq!(bt.spec().aggregation, BarAggregation::Minute);

    // Per-record OHLCV payload equality against the canonical source rows.
    for (i, bar) in loaded.iter().enumerate() {
        let row = &table.rows[i];
        assert_eq!(
            bar.open,
            Price::from_str(&row.open).expect("parse open"),
            "open at {i}"
        );
        assert_eq!(
            bar.high,
            Price::from_str(&row.high).expect("parse high"),
            "high at {i}"
        );
        assert_eq!(
            bar.low,
            Price::from_str(&row.low).expect("parse low"),
            "low at {i}"
        );
        assert_eq!(
            bar.close,
            Price::from_str(&row.close).expect("parse close"),
            "close at {i}"
        );
        assert_eq!(
            bar.volume,
            Quantity::from_str(&row.volume).expect("parse volume"),
            "volume at {i}"
        );
        assert_eq!(
            bar.ts_event.as_u64(),
            u64::try_from(row.close_time).unwrap(),
            "close_time at {i}"
        );
    }
}
