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
    NT_DATA_TYPE_INDEX_PRICE_UPDATE, append_chainlink_index_prices_archive,
    canonical_rows_to_index_prices, chainlink_index_spec_from_table, project_chainlink_to_catalog,
    read_back_index_prices, read_chainlink_per_second_object,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

/// Fixture: a verbatim real staged BTC 5m-cycle object (one full 300-row cycle)
/// copied from `s3://bolt-parquet/backfill-staging/.../chainlink/btc-5m-cycles/`.
/// It is dictionary-encoded with an embedded `ARROW:schema` block exactly as the
/// production archive writes it, so the test exercises the real Arrow decode
/// (a `DictionaryArray`, not a plain `StringArray`).
const FIXTURE: &str = "tests/fixtures/chainlink/btc-updown-5m-sample.parquet";
const MARKET_SLUG: &str = "btc-updown-5m-1777420800";
/// Number of per-second rows in the fixture cycle.
const FIXTURE_ROWS: usize = 300;
const NT_INSTRUMENT_ID: &str = "BTCUSD.CHAINLINK";
/// Instrument id the *data-derived* bulk path builds from the slug's asset token
/// (`btc`) plus the venue suffix. The bulk path cannot invent the `USD` quote the
/// caller-supplied single-object spec carries, so the derived id has no quote.
const DERIVED_NT_INSTRUMENT_ID: &str = "BTC.CHAINLINK";
/// The feed's per-second prices carry up to 11 fractional digits; the
/// data-derived precision clamps to NautilusTrader's fixed precision, so the
/// single-object spec materializes at that same cap (9 fractional digits).
const PRICE_PRECISION: u8 = 9;

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
    assert_eq!(table.rows.len(), FIXTURE_ROWS);
    // Provenance tokens preserved and uniform across the cycle.
    for row in &table.rows {
        assert_eq!(row.resolution, CHAINLINK_RESOLUTION_PER_SECOND);
        assert_eq!(row.source, CHAINLINK_SOURCE_PER_SECOND);
        assert_eq!(row.market_slug, MARKET_SLUG);
    }
    // Unix-seconds source scaled to nanoseconds (first row = 1777420800 s).
    assert_eq!(table.rows[0].event_time_nanos, 1_777_420_800_000_000_000);
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
    assert_eq!(projection.update_count, FIXTURE_ROWS);
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

#[test]
fn chainlink_index_prices_data_derived_append_round_trips() {
    // The bulk path: derive the instrument identity AND price precision from the
    // object's own rows (Chainlink stages no instrument universe), append into a
    // shared catalog with no clean-root guard, and prove the NautilusTrader
    // round-trip is lossless.
    let object_bytes = std::fs::read(fixture_path()).expect("read fixture bytes");
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Independent expectation from the same source, via the data-derived spec.
    let table = read_chainlink_per_second_object(&fixture_path()).expect("read fixture");
    let derived = chainlink_index_spec_from_table(&table).expect("derive spec");
    // Identity is data-derived from the slug's asset token + venue suffix; no
    // `USD` quote is fabricated (the slug carries none).
    assert_eq!(derived.nt_instrument_id, DERIVED_NT_INSTRUMENT_ID);
    assert_eq!(derived.market_slug, MARKET_SLUG);
    // Precision is read from the data (the max fractional-digit count across the
    // cycle's prices), so it must not exceed the materialization precision the
    // single-object spec uses for the same feed, and must be representable.
    assert!(
        derived.price_precision <= PRICE_PRECISION,
        "data-derived precision {} exceeds the feed's known max {PRICE_PRECISION}",
        derived.price_precision
    );
    let expected = canonical_rows_to_index_prices(&table, &derived).expect("map to index prices");

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summary =
        append_chainlink_index_prices_archive(&object_bytes, &mut catalog).expect("append");
    assert_eq!(summary.nt_instrument_id, DERIVED_NT_INSTRUMENT_ID);
    assert_eq!(summary.record_count, table.rows.len());
    // Precision is read from the data and is self-consistent with the prices
    // built from the same derived spec — not a hardcoded assumption.
    assert_eq!(summary.price_precision, derived.price_precision);
    assert_eq!(summary.price_precision, expected[0].value.precision);
    // Provenance is honest: the append's object hash equals the SHA-256 the
    // reader computed over the same source bytes.
    assert_eq!(summary.source_object_hash, table.source_object_hash);
    assert!(!summary.source_object_hash.is_empty());

    let loaded = read_back_index_prices(dir.path(), DERIVED_NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), expected.len(), "round-tripped update count");
    assert!(
        loaded
            .windows(2)
            .all(|w| w[1].ts_init.as_u64() > w[0].ts_init.as_u64()),
        "loaded updates must be strictly ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived append must round-trip identically (count, ordering, payload, precision)"
    );
}
