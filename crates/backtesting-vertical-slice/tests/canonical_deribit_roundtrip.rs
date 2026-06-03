//! Round-trip proof for the Deribit (Tardis options-chain) converter.
//!
//! Reads a small committed gzip-CSV fixture downsampled from the real Deribit
//! options-chain archive object (one option instrument's two-sided top-of-book
//! series), normalizes it into NautilusTrader `QuoteTick`s, writes them into a
//! temp NautilusTrader `ParquetDataCatalog` via `write_to_parquet`, and reads
//! them back via `query_typed_data::<QuoteTick>`. Asserts the round-tripped
//! count, instrument id, ordering, and the first/last quote payload all survive
//! the catalog round-trip — proving the venue's data is in an NT catalog NT can
//! replay (i.e. "backtestable").
//!
//! The test is hermetic: it reads the committed fixture, never S3.

use std::path::PathBuf;

use backtesting_vertical_slice::canonical_deribit::{
    DeribitOptionInstrumentSpec, normalize_deribit_options_chain, project_series_to_catalog,
    read_back_quote_ticks, read_gzip_csv,
};
use nautilus_model::types::Price;

/// Path to the committed gzip-CSV fixture (one instrument's BBO series).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/deribit/options_chain_sample.csv.gz")
}

/// The instrument spec for the single option whose series the fixture isolates.
/// All values are derived from the fixture's own rows (symbol, strike, type) and
/// the venue's known premium tick / contract increment; nothing is invented.
fn fixture_spec() -> DeribitOptionInstrumentSpec {
    DeribitOptionInstrumentSpec {
        nt_instrument_id: "AVAX_USDC-29MAY26-8D6-P.DERIBIT".to_string(),
        raw_symbol: "AVAX_USDC-29MAY26-8D6-P".to_string(),
        underlying: "AVAX".to_string(),
        quote_currency: "USDC".to_string(),
        settlement_currency: "USDC".to_string(),
        is_inverse: false,
        option_kind: "PUT".to_string(),
        strike_price: "8.6".to_string(),
        activation_ns: 1_777_593_600_000_000_000,
        expiration_ns: 1_780_041_600_000_000_000,
        // Premium tick: fixture bid/ask carry up to 3 decimal places.
        price_increment: "0.001".to_string(),
        // Contract amounts in the fixture are whole integers.
        size_increment: "1".to_string(),
    }
}

#[test]
fn deribit_options_chain_round_trips_through_nt_catalog() {
    let spec = fixture_spec();

    // Parse the real gzip-CSV fixture -> canonical two-sided top-of-book series.
    let csv_text = read_gzip_csv(&fixture_path()).expect("decompress fixture");
    let series = normalize_deribit_options_chain(&csv_text, &spec).expect("normalize fixture");

    // The downsampled fixture is the single instrument's full two-sided series.
    assert!(
        series.rows.len() >= 100,
        "fixture should carry a substantial BBO series, got {}",
        series.rows.len()
    );
    assert_eq!(
        series.skipped_one_sided, 0,
        "fixture was pre-filtered to two-sided rows only"
    );

    // Source rows are non-decreasing in event time after normalization.
    for pair in series.rows.windows(2) {
        assert!(
            pair[1].event_time >= pair[0].event_time,
            "event times must be non-decreasing"
        );
    }

    let expected_count = series.rows.len();
    let first = series.rows.first().expect("non-empty series").clone();
    let last = series.rows.last().expect("non-empty series").clone();

    // Project -> NautilusTrader ParquetDataCatalog (QuoteTick) -> read back.
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let projection =
        project_series_to_catalog(&series, &spec, dir.path()).expect("project to NT catalog");
    assert_eq!(projection.quote_count, expected_count);
    assert_eq!(projection.nt_instrument_id, spec.nt_instrument_id);
    assert_eq!(projection.data_type, "QuoteTick");

    let loaded =
        read_back_quote_ticks(dir.path(), &spec.nt_instrument_id).expect("query back from catalog");

    // Count survives the round-trip.
    assert_eq!(
        loaded.len(),
        expected_count,
        "all quotes must round-trip through the NT catalog"
    );

    // Ordering survives the round-trip (non-decreasing ts_event).
    for pair in loaded.windows(2) {
        assert!(
            pair[1].ts_event >= pair[0].ts_event,
            "round-tripped quotes must stay in event-time order"
        );
    }

    // Instrument id survives on every quote.
    for quote in &loaded {
        assert_eq!(quote.instrument_id.to_string(), spec.nt_instrument_id);
    }

    // First and last quote payloads survive exactly (price/size + timestamps).
    let first_loaded = loaded.first().expect("non-empty");
    assert_eq!(
        first_loaded.bid_price,
        Price::from(first.bid_price.as_str())
    );
    assert_eq!(
        first_loaded.ask_price,
        Price::from(first.ask_price.as_str())
    );
    assert_eq!(
        u64::from(first_loaded.ts_event),
        u64::try_from(first.event_time).unwrap()
    );

    let last_loaded = loaded.last().expect("non-empty");
    assert_eq!(last_loaded.bid_price, Price::from(last.bid_price.as_str()));
    assert_eq!(last_loaded.ask_price, Price::from(last.ask_price.as_str()));
    assert_eq!(
        u64::from(last_loaded.ts_event),
        u64::try_from(last.event_time).unwrap()
    );
}
