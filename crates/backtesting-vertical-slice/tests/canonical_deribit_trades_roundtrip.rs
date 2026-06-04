//! Round-trip proof for the Deribit RiveChen merged-trades converter.
//!
//! Reads a small committed Parquet fixture downsampled (single instrument, all
//! its prints) from the real RiveChen merged-trades archive object, normalizes
//! it into NautilusTrader `TradeTick`s, writes them into a temp NautilusTrader
//! `ParquetDataCatalog` via `write_to_parquet`, and reads them back via
//! `query_typed_data::<TradeTick>`. Asserts the round-tripped count, instrument
//! id, ordering, and the first/last trade payload all survive — proving the
//! venue's trade data is in an NT catalog NT can replay.
//!
//! The test is hermetic: it reads the committed fixture, never S3.

use std::{fs, path::PathBuf};

use backtesting_vertical_slice::canonical_deribit::{
    DeribitBarsInstrumentSpec, DeribitTradeAggressorSide, DeribitTradesInstrumentSpec,
    append_deribit_bars_archive, append_deribit_trades_archive, bars_to_bars,
    deribit_bars_spec_from_rows, deribit_trade_instruments, deribit_trades_spec_from_rows,
    normalize_deribit_bars, normalize_deribit_merged_trades, project_trades_to_catalog,
    read_back_bars, read_back_trade_ticks, trades_to_trade_ticks,
};
use nautilus_model::{
    enums::{AggressorSide, BarAggregation},
    instruments::Instrument,
    types::Price,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

/// Path to the committed 1m-bars JSON fixture (same object the bars converter's
/// own hermetic round-trip test consumes).
fn bars_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deribit/bars_1m_sample.json")
}

/// A staged `bars_1m` S3 object key carrying the fixture instrument in its
/// `instrument=` segment, exactly as `backfill_deribit_to_s3.py` lays it out
/// (`.../family=bars_1m/instrument=<SYMBOL>/...`). The bulk bars path reads
/// identity from this key because the `bars_1m` JSON payload has no
/// `instrument_name`.
fn bars_object_key() -> String {
    "backfill-staging/2026-06-01/deribit/raw/v1/run=deribit-3m-test/family=bars_1m/\
     instrument=BTC_USDC-29MAY26-66000-C/product_family=option/object=fixture.json"
        .to_string()
}

/// Path to the committed Parquet fixture (one option's full trade-print series).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/deribit/merged_trades_sample.parquet")
}

/// Spec for the single option whose trade series the fixture isolates. All
/// values derive from the fixture's own rows (instrument_name) and the venue's
/// known premium tick / contract increment; nothing is invented.
fn fixture_spec() -> DeribitTradesInstrumentSpec {
    DeribitTradesInstrumentSpec {
        nt_instrument_id: "XRP_USDC-29MAY26-1d45-P.DERIBIT".to_string(),
        raw_symbol: "XRP_USDC-29MAY26-1d45-P".to_string(),
        underlying: "XRP".to_string(),
        quote_currency: "USDC".to_string(),
        settlement_currency: "USDC".to_string(),
        is_inverse: false,
        option_kind: "PUT".to_string(),
        strike_price: "1.45".to_string(),
        activation_ns: 1_777_593_600_000_000_000,
        expiration_ns: 1_780_041_600_000_000_000,
        // Premium tick: fixture prices carry up to 4 decimal places.
        price_increment: "0.0001".to_string(),
        // Contract amounts in the fixture are whole integers.
        size_increment: "1".to_string(),
    }
}

#[test]
fn deribit_merged_trades_round_trip_through_nt_catalog() {
    let spec = fixture_spec();

    // Parse the real Parquet fixture -> canonical single-instrument trades.
    let series =
        normalize_deribit_merged_trades(&fixture_path(), &spec).expect("normalize fixture");

    // The fixture is one busy instrument's full print series.
    assert!(
        series.rows.len() >= 50,
        "fixture should carry a substantial trade series, got {}",
        series.rows.len()
    );
    // The source object mixes many instruments; everything else is skipped.
    assert!(
        series.skipped_other_symbol > 0,
        "fixture is a multi-instrument object; other symbols must be skipped"
    );

    // Non-decreasing event time after normalization (sort contract).
    for pair in series.rows.windows(2) {
        assert!(
            pair[1].event_time >= pair[0].event_time,
            "event times must be non-decreasing"
        );
    }

    let expected_count = series.rows.len();
    let first = series.rows.first().expect("non-empty series").clone();
    let last = series.rows.last().expect("non-empty series").clone();

    // First/last prints in the fixture (captured from the real object).
    assert_eq!(first.trade_id, "USDC-48938683");
    assert_eq!(first.aggressor_side, DeribitTradeAggressorSide::Seller);
    assert_eq!(last.trade_id, "USDC-49429632");
    assert_eq!(last.aggressor_side, DeribitTradeAggressorSide::Buyer);

    // Project -> NautilusTrader ParquetDataCatalog (TradeTick) -> read back.
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let projection =
        project_trades_to_catalog(&series, &spec, dir.path()).expect("project to NT catalog");
    assert_eq!(projection.quote_count, expected_count);
    assert_eq!(projection.nt_instrument_id, spec.nt_instrument_id);
    assert_eq!(projection.data_type, "TradeTick");

    let loaded =
        read_back_trade_ticks(dir.path(), &spec.nt_instrument_id).expect("query back from catalog");

    // Count survives the round-trip.
    assert_eq!(
        loaded.len(),
        expected_count,
        "all trades must round-trip through the NT catalog"
    );

    // Ordering survives (non-decreasing ts_event).
    for pair in loaded.windows(2) {
        assert!(
            pair[1].ts_event >= pair[0].ts_event,
            "round-tripped trades must stay in event-time order"
        );
    }

    // Instrument id survives on every trade.
    for tick in &loaded {
        assert_eq!(tick.instrument_id.to_string(), spec.nt_instrument_id);
    }

    // First and last trade payloads survive exactly.
    let first_loaded = loaded.first().expect("non-empty");
    assert_eq!(first_loaded.price, Price::new(first.price, 4));
    assert_eq!(first_loaded.aggressor_side, AggressorSide::Seller);
    assert_eq!(first_loaded.trade_id.to_string(), first.trade_id);
    assert_eq!(
        u64::from(first_loaded.ts_event),
        u64::try_from(first.event_time).unwrap()
    );

    let last_loaded = loaded.last().expect("non-empty");
    assert_eq!(last_loaded.price, Price::new(last.price, 4));
    assert_eq!(last_loaded.aggressor_side, AggressorSide::Buyer);
    assert_eq!(last_loaded.trade_id.to_string(), last.trade_id);
    assert_eq!(
        u64::from(last_loaded.ts_event),
        u64::try_from(last.event_time).unwrap()
    );
}

#[test]
fn deribit_trades_data_derived_append_round_trips() {
    // The bulk path: derive precision from the object's own rows and identity
    // from each instrument's Deribit symbol (Deribit stages no instrument
    // universe for this family), append into a shared catalog with no
    // clean-root guard, and prove the NautilusTrader round-trip is lossless.
    // Mirrors `canonical_okx::okx_trades_data_derived_append_round_trips`.
    let object_bytes = fs::read(fixture_path()).expect("read merged-trades fixture");
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // The object interleaves many instruments; the fixture's target must be one
    // of them. Identity is read off the symbol, never invented.
    let instruments =
        deribit_trade_instruments(&fixture_path()).expect("enumerate object instruments");
    let target = "XRP_USDC-29MAY26-1d45-P";
    assert!(
        instruments.iter().any(|s| s == target),
        "fixture must carry the target instrument {target:?}; found {instruments:?}"
    );

    // Independent expectation for the target, via the data-derived spec built
    // exactly the way the bulk append builds it (rows -> precision + identity).
    let probe = DeribitTradesInstrumentSpec {
        raw_symbol: target.to_string(),
        ..fixture_spec()
    };
    let series =
        normalize_deribit_merged_trades(&fixture_path(), &probe).expect("normalize target");
    let derived = deribit_trades_spec_from_rows(&series.rows, target).expect("derive spec");
    // Identity parsed from the symbol matches the hand-written committed spec.
    assert_eq!(derived.nt_instrument_id, fixture_spec().nt_instrument_id);
    assert_eq!(derived.underlying, "XRP");
    assert_eq!(derived.quote_currency, "USDC");
    assert_eq!(derived.option_kind, "PUT");
    assert_eq!(derived.strike_price, "1.45");
    let instrument = derived.build_instrument().expect("build instrument");
    // Premium tick: XRP_USDC prints carry up to 4 decimal places; whole-integer
    // contract amounts -> size precision 0. Read from the data, not assumed.
    assert_eq!(instrument.price_precision(), 4);
    assert_eq!(instrument.size_precision(), 0);
    let expected = trades_to_trade_ticks(&series, &instrument).expect("map to ticks");

    // Append the whole object into a freshly-opened (empty) catalog — no
    // dirty-root refusal. Every instrument in this object is an XRP_USDC option,
    // so identity parsing + currency lookup succeed for all of them.
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_deribit_trades_archive(&object_bytes, &mut catalog).expect("append trades");
    let target_summary = summaries
        .iter()
        .find(|s| s.nt_instrument_id == derived.nt_instrument_id)
        .expect("target instrument must be summarized");
    assert_eq!(target_summary.record_count, series.rows.len());
    assert_eq!(target_summary.price_precision, 4);
    assert_eq!(target_summary.price_precision, expected[0].price.precision);
    assert_eq!(target_summary.size_precision, expected[0].size.precision);

    let loaded =
        read_back_trade_ticks(dir.path(), &derived.nt_instrument_id).expect("read back ticks");
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
fn deribit_bars_data_derived_append_round_trips() {
    // Bars bulk path: the `bars_1m` JSON payload carries one instrument's
    // candles and no `instrument_name`, so identity comes from the staged object
    // key's `instrument=` segment and precision is derived from the OHLCV rows.
    // Append into a shared catalog with no clean-root guard and prove the
    // NautilusTrader round-trip is lossless.
    let object_bytes = fs::read(bars_fixture_path()).expect("read bars fixture");
    let object_key = bars_object_key();
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Independent expectation, built the same way the bulk append builds it:
    // normalize with a raw-symbol-only probe spec (normalization reads only
    // `raw_symbol`), then derive precision + identity from the resulting rows.
    let target = "BTC_USDC-29MAY26-66000-C";
    let json_text = String::from_utf8(object_bytes.clone()).expect("fixture is UTF-8");
    let probe = DeribitBarsInstrumentSpec {
        nt_instrument_id: String::new(),
        raw_symbol: target.to_string(),
        underlying: String::new(),
        quote_currency: String::new(),
        settlement_currency: String::new(),
        is_inverse: false,
        option_kind: "CALL".to_string(),
        strike_price: "1".to_string(),
        activation_ns: 0,
        expiration_ns: 0,
        price_increment: "1".to_string(),
        size_increment: "1".to_string(),
        bar_step: 1,
        bar_aggregation: BarAggregation::Minute,
    };
    let series = normalize_deribit_bars(&json_text, &probe).expect("normalize bars");
    let derived = deribit_bars_spec_from_rows(&series.rows, target).expect("derive bars spec");
    assert_eq!(derived.nt_instrument_id, format!("{target}.DERIBIT"));
    assert_eq!(derived.underlying, "BTC");
    assert_eq!(derived.quote_currency, "USDC");
    assert_eq!(derived.option_kind, "CALL");
    assert_eq!(derived.strike_price, "66000");
    let instrument = derived.build_instrument().expect("build instrument");
    // OHLC prices carry up to 1 dp; volume up to 8 dp. Read from the data.
    assert_eq!(instrument.price_precision(), 1);
    assert_eq!(instrument.size_precision(), 8);
    let expected = bars_to_bars(&series, &derived, &instrument).expect("map to bars");

    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summary =
        append_deribit_bars_archive(&object_bytes, &object_key, &mut catalog).expect("append bars");
    assert_eq!(summary.nt_instrument_id, derived.nt_instrument_id);
    assert_eq!(summary.record_count, series.rows.len());
    assert_eq!(summary.price_precision, 1);
    assert_eq!(summary.size_precision, 8);

    let loaded = read_back_bars(dir.path(), &derived.nt_instrument_id).expect("read back bars");
    assert_eq!(loaded.len(), expected.len(), "round-tripped bar count");
    assert!(
        loaded.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded bars must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived bars append must round-trip identically (count, ordering, payload, precision)"
    );
}
