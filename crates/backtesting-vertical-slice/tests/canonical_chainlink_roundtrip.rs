//! Round-trip proof for the Chainlink per-second underlying price feed.
//!
//! Parses a committed fixture (a downsampled real staged object) into the
//! canonical price table, projects it into a NautilusTrader `ParquetDataCatalog`
//! as `IndexPriceUpdate` via NautilusTrader's own writer, then reads it back via
//! NautilusTrader's typed query and asserts the round-tripped count, ordering,
//! prices, and timestamps match the projection. This proves the venue's data is
//! in an NT-replayable catalog.
//!
//! Literal sample ids (the fixture path, market slug, and instrument id) appear
//! only here in the test, never in `src/`.

use std::path::PathBuf;

use backtesting_vertical_slice::canonical_chainlink::{
    CHAINLINK_RESOLUTION_PER_SECOND, CHAINLINK_SOURCE_PER_SECOND, ChainlinkIndexSpec,
    NT_DATA_TYPE_INDEX_PRICE_UPDATE, canonical_rows_to_index_prices, project_chainlink_to_catalog,
    read_back_index_prices, read_chainlink_per_second_object,
};

/// Fixture: 40 rows downsampled from the smallest real staged BTC 5m cycle
/// object under `s3://bolt-parquet/backfill-staging/.../chainlink/btc-5m-cycles/`.
const FIXTURE: &str = "tests/fixtures/chainlink/btc-updown-5m-sample.parquet";
const MARKET_SLUG: &str = "btc-updown-5m-1778380800";
const NT_INSTRUMENT_ID: &str = "BTCUSD.CHAINLINK";
/// Source feed carries up to 8 fractional digits; 8 fits NT fixed precision.
const PRICE_PRECISION: u8 = 8;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

fn spec() -> ChainlinkIndexSpec {
    ChainlinkIndexSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        price_precision: PRICE_PRECISION,
        market_slug: MARKET_SLUG.to_string(),
    }
}

#[test]
fn fixture_parses_to_canonical_table() {
    let table = read_chainlink_per_second_object(&fixture_path()).expect("read fixture");
    assert_eq!(table.market_slug, MARKET_SLUG);
    assert_eq!(table.rows.len(), 40);
    // Provenance tokens preserved and uniform across the cycle.
    for row in &table.rows {
        assert_eq!(row.resolution, CHAINLINK_RESOLUTION_PER_SECOND);
        assert_eq!(row.source, CHAINLINK_SOURCE_PER_SECOND);
        assert_eq!(row.market_slug, MARKET_SLUG);
    }
    // Unix-seconds source scaled to nanoseconds (first row = 1778380960 s).
    assert_eq!(table.rows[0].event_time_nanos, 1_778_380_960_000_000_000);
    // Strictly ascending (validate() enforced this).
    let times: Vec<i64> = table.rows.iter().map(|r| r.event_time_nanos).collect();
    assert!(times.windows(2).all(|w| w[1] > w[0]));
    assert!(!table.source_object_hash.is_empty());
}

#[test]
fn round_trips_index_prices_through_nautilus_catalog() {
    let table = read_chainlink_per_second_object(&fixture_path()).expect("read fixture");
    let projected =
        canonical_rows_to_index_prices(&table, &spec()).expect("project to index prices");
    assert_eq!(projected.len(), table.rows.len());

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_chainlink_to_catalog(&table, &spec(), dir.path()).expect("project to catalog");
    assert_eq!(projection.update_count, 40);
    assert_eq!(projection.data_type, NT_DATA_TYPE_INDEX_PRICE_UPDATE);
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert!(!projection.catalog_hash.is_empty());

    let loaded = read_back_index_prices(dir.path(), NT_INSTRUMENT_ID).expect("read back");

    // Count matches.
    assert_eq!(loaded.len(), projected.len());

    // Ordering + every field round-trips identically (NT replay sees the same
    // instrument, price at the projected precision, and event timestamp).
    for (index, (out, back)) in projected.iter().zip(loaded.iter()).enumerate() {
        assert_eq!(
            back.instrument_id.to_string(),
            NT_INSTRUMENT_ID,
            "row {index} instrument id"
        );
        assert_eq!(back.value, out.value, "row {index} price");
        assert_eq!(back.ts_event, out.ts_event, "row {index} ts_event");
        assert_eq!(back.ts_init, out.ts_init, "row {index} ts_init");
    }

    // Timestamps strictly ascending in the read-back stream.
    assert!(
        loaded
            .windows(2)
            .all(|w| w[1].ts_init.as_u64() > w[0].ts_init.as_u64()),
        "read-back stream must preserve strict ascending order"
    );
}

#[test]
fn projection_refuses_dirty_catalog_root() {
    let table = read_chainlink_per_second_object(&fixture_path()).expect("read fixture");
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
    let err = project_chainlink_to_catalog(&table, &spec(), dir.path())
        .expect_err("dirty catalog root must be refused");
    assert!(err.to_string().contains("not empty"), "{err}");
}

#[test]
fn spec_market_slug_mismatch_is_rejected() {
    let table = read_chainlink_per_second_object(&fixture_path()).expect("read fixture");
    let mut bad = spec();
    bad.market_slug = "eth-updown-5m-0".to_string();
    let err =
        canonical_rows_to_index_prices(&table, &bad).expect_err("slug mismatch must be rejected");
    assert!(
        err.to_string().contains("does not match spec slug"),
        "{err}"
    );
}
