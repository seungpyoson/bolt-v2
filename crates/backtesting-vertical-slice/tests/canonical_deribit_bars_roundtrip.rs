//! Round-trip proof for the Deribit 1m OHLC bars converter.
//!
//! Reads a small committed JSON fixture downsampled from a real Deribit
//! `get_tradingview_chart_data` 1m-bars archive object (the JSON-RPC envelope
//! and parallel OHLCV arrays are preserved), normalizes it into NautilusTrader
//! `Bar`s, writes them into a temp NautilusTrader `ParquetDataCatalog` via
//! `write_to_parquet`, and reads them back via `query_typed_data::<Bar>`.
//! Asserts the round-tripped count, ordering, and first/last bar payload all
//! survive — proving the venue's bar data is in an NT catalog NT can replay.
//!
//! The test is hermetic: it reads the committed fixture, never S3.

use std::{fs, path::PathBuf};

use backtesting_vertical_slice::canonical_deribit::{
    DeribitBarsInstrumentSpec, normalize_deribit_bars, project_bars_to_catalog, read_back_bars,
};
use nautilus_model::{enums::BarAggregation, types::Price};

/// Path to the committed 1m-bars JSON fixture.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/deribit/bars_1m_sample.json")
}

/// Spec for the instrument whose 1m candles the fixture carries. The fixture is
/// a real Deribit `bars_1m` object; the bar step/unit (1-minute) is the source
/// partition's contract and the precision is the venue's known tick.
fn fixture_spec() -> DeribitBarsInstrumentSpec {
    DeribitBarsInstrumentSpec {
        nt_instrument_id: "BTC_USDC-29MAY26-66000-C.DERIBIT".to_string(),
        raw_symbol: "BTC_USDC-29MAY26-66000-C".to_string(),
        underlying: "BTC".to_string(),
        quote_currency: "USDC".to_string(),
        settlement_currency: "USDC".to_string(),
        is_inverse: false,
        option_kind: "CALL".to_string(),
        strike_price: "66000".to_string(),
        activation_ns: 1_777_593_600_000_000_000,
        expiration_ns: 1_780_041_600_000_000_000,
        // Fixture OHLC prices carry up to 1 decimal place.
        price_increment: "0.1".to_string(),
        // Fixture volume carries up to 8 decimal places.
        size_increment: "0.00000001".to_string(),
        // 1m bars: step 1, MINUTE aggregation (the archive `bars_1m` partition).
        bar_step: 1,
        bar_aggregation: BarAggregation::Minute,
    }
}

#[test]
fn deribit_bars_round_trip_through_nt_catalog() {
    let spec = fixture_spec();

    // Parse the real JSON fixture -> canonical 1m OHLC series.
    let json_text = fs::read_to_string(fixture_path()).expect("read fixture");
    let series = normalize_deribit_bars(&json_text, &spec).expect("normalize fixture");

    assert_eq!(series.status, "ok");
    assert!(
        series.rows.len() >= 10,
        "fixture should carry a meaningful bar series, got {}",
        series.rows.len()
    );

    // Strictly increasing open times (1m spacing).
    for pair in series.rows.windows(2) {
        assert!(
            pair[1].open_time > pair[0].open_time,
            "bar open times must be strictly increasing"
        );
    }

    let expected_count = series.rows.len();
    let first = series.rows.first().expect("non-empty series").clone();
    let last = series.rows.last().expect("non-empty series").clone();

    // First/last bar open times in the fixture (captured from the real object,
    // milliseconds -> nanoseconds).
    assert_eq!(first.open_time, 1_772_323_200_000 * 1_000_000);
    assert_eq!(last.open_time, 1_772_324_340_000 * 1_000_000);

    // Project -> NautilusTrader ParquetDataCatalog (Bar) -> read back.
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let projection =
        project_bars_to_catalog(&series, &spec, dir.path()).expect("project to NT catalog");
    assert_eq!(projection.quote_count, expected_count);
    assert_eq!(projection.nt_instrument_id, spec.nt_instrument_id);
    assert_eq!(projection.data_type, "Bar");

    let loaded =
        read_back_bars(dir.path(), &spec.nt_instrument_id).expect("query back from catalog");

    // Count survives the round-trip.
    assert_eq!(
        loaded.len(),
        expected_count,
        "all bars must round-trip through the NT catalog"
    );

    // Ordering survives (non-decreasing ts_event).
    for pair in loaded.windows(2) {
        assert!(
            pair[1].ts_event >= pair[0].ts_event,
            "round-tripped bars must stay in event-time order"
        );
    }

    // Instrument id survives on every bar's bar_type.
    for bar in &loaded {
        assert_eq!(
            bar.bar_type.instrument_id().to_string(),
            spec.nt_instrument_id
        );
    }

    // First and last bar payloads survive exactly (OHLC + timestamp).
    let first_loaded = loaded.first().expect("non-empty");
    assert_eq!(first_loaded.open, Price::new(first.open, 1));
    assert_eq!(first_loaded.high, Price::new(first.high, 1));
    assert_eq!(first_loaded.low, Price::new(first.low, 1));
    assert_eq!(first_loaded.close, Price::new(first.close, 1));
    assert_eq!(
        u64::from(first_loaded.ts_event),
        u64::try_from(first.open_time).unwrap()
    );

    let last_loaded = loaded.last().expect("non-empty");
    assert_eq!(last_loaded.open, Price::new(last.open, 1));
    assert_eq!(last_loaded.close, Price::new(last.close, 1));
    assert_eq!(
        u64::from(last_loaded.ts_event),
        u64::try_from(last.open_time).unwrap()
    );
}
