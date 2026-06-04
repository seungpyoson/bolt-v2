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

use std::path::PathBuf;

use backtesting_vertical_slice::canonical_deribit::{
    DeribitTradeAggressorSide, DeribitTradesInstrumentSpec, normalize_deribit_merged_trades,
    project_trades_to_catalog, read_back_trade_ticks,
};
use nautilus_model::{enums::AggressorSide, types::Price};

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
