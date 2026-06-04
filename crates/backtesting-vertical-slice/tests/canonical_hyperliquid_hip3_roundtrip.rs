//! Hyperliquid HIP-3 bars round-trip proof.
//!
//! Proves, against the NautilusTrader dependency resolved by this branch, that
//! the venue's only backtestable datum — the OHLCV candle — survives the full
//! path end to end:
//!
//! ```text
//! committed staged-bars fixture (real candleSnapshot JSONL, downsampled)
//!   -> normalize_hip3_bars  (canonical OHLCV table, invariant-validated)
//!   -> project_hip3_bars_to_catalog  (NautilusTrader Bar -> ParquetDataCatalog)
//!   -> read_back_hip3_bars  (NautilusTrader query_typed_data::<Bar>)
//!   -> assert count + ascending ts ordering + OHLCV values match
//! ```
//!
//! Hyperliquid HIP-3 has no order book and no trade ticks, so `Bar` is the
//! deliverable. The fixture is a hermetic, downsampled slice of a single real
//! `(instrument, interval)` series; the test never reaches S3.

use std::path::PathBuf;

use backtesting_vertical_slice::{
    canonical_hyperliquid_hip3::{
        Hip3BarProvenance, Hip3BarSelector, NT_DATA_TYPE_BAR, append_hyperliquid_hip3_bars_archive,
        hip3_bar_series, normalize_hip3_bars, project_hip3_bars_to_catalog, read_back_hip3_bars,
    },
    source_proof::SourceProofFidelityClass,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use tempfile::TempDir;

/// Committed downsampled fixture: one real HIP-3 `(instrument, interval)` series
/// of provider candles, captured from the staged `table=bars` object.
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hyperliquid-hip3/bars_hyna_btc_1h.jsonl")
}

/// The fixture holds exactly one instrument at one interval, so the selector is
/// derived from the fixture's own first row rather than hardcoded: we read the
/// instrument/interval out of the staged JSON itself.
fn selector_from_fixture(jsonl: &str) -> Hip3BarSelector {
    let first = jsonl
        .lines()
        .find(|l| !l.trim().is_empty())
        .expect("fixture has at least one row");
    let value: serde_json::Value = serde_json::from_str(first).expect("first row is json");
    Hip3BarSelector {
        instrument_name: value["instrument_name"]
            .as_str()
            .expect("instrument_name")
            .to_string(),
        interval: value["interval"].as_str().expect("interval").to_string(),
    }
}

fn provenance() -> Hip3BarProvenance {
    Hip3BarProvenance {
        ingest_run_id: "hip3-bars-roundtrip-proof".to_string(),
        source_proof_id: "source-proof-hyperliquid-hip3-bars".to_string(),
        source_proof_version: 1,
        // 64-hex placeholder hash; the gate-1 acceptance that binds the real
        // object hash is a separate slice. This proof is the catalog round-trip.
        payload_hash: "0".repeat(64),
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        forbidden_claims: vec![
            "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
        ],
    }
}

#[test]
fn hip3_bars_round_trip_through_nautilus_catalog() {
    let jsonl = std::fs::read_to_string(fixture_path()).expect("read committed fixture");
    let selector = selector_from_fixture(&jsonl);

    // Gate 2: parse + normalize the real staged candles into the canonical table.
    let table = normalize_hip3_bars(&jsonl, &selector, &provenance()).expect("normalize hip3 bars");
    assert!(
        table.rows.len() >= 2,
        "fixture must carry a multi-bar series, got {}",
        table.rows.len()
    );

    // Capture the expected NT bars before writing so the read-back is checked
    // against the exact projected values (not just a re-derivation of itself).
    let expected = table.to_nt_bars().expect("project to nt bars");
    assert_eq!(expected.len(), table.rows.len());

    // Gate 3: write Bars into a NautilusTrader ParquetDataCatalog via NT's own
    // writer, then read them back via NT's own query.
    let dir = TempDir::new().expect("temp catalog root");
    let projection =
        project_hip3_bars_to_catalog(&table, dir.path()).expect("project bars to catalog");
    assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
    assert_eq!(projection.bar_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, table.nt_instrument_id);

    let loaded = read_back_hip3_bars(dir.path(), &table.nt_bar_type).expect("read bars back");

    // Count must match.
    assert_eq!(
        loaded.len(),
        expected.len(),
        "round-tripped bar count must match projected count"
    );

    // Ordering must be ascending by event time, and must match the source order.
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_event <= window[1].ts_event,
            "round-tripped bars must be in ascending ts_event order"
        );
    }

    // Every field of every bar must survive the round-trip exactly, in order.
    for (i, (got, want)) in loaded.iter().zip(expected.iter()).enumerate() {
        assert_eq!(got.bar_type, want.bar_type, "bar {i}: bar_type");
        assert_eq!(got.open, want.open, "bar {i}: open");
        assert_eq!(got.high, want.high, "bar {i}: high");
        assert_eq!(got.low, want.low, "bar {i}: low");
        assert_eq!(got.close, want.close, "bar {i}: close");
        assert_eq!(got.volume, want.volume, "bar {i}: volume");
        assert_eq!(got.ts_event, want.ts_event, "bar {i}: ts_event");
        assert_eq!(
            got.bar_type.instrument_id().to_string(),
            table.nt_instrument_id,
            "bar {i}: instrument id"
        );
    }
}

#[test]
fn projection_refuses_dirty_catalog_root() {
    let jsonl = std::fs::read_to_string(fixture_path()).expect("read committed fixture");
    let selector = selector_from_fixture(&jsonl);
    let table = normalize_hip3_bars(&jsonl, &selector, &provenance()).expect("normalize");

    let dir = TempDir::new().expect("temp catalog root");
    std::fs::write(dir.path().join("stale.parquet"), b"stale").expect("seed stale file");
    let err = project_hip3_bars_to_catalog(&table, dir.path())
        .expect_err("dirty catalog root must be refused");
    assert!(err.to_string().contains("not empty"), "{err}");
}

/// Stable locator handed to the bulk-append path as the source object's S3 key.
/// It is data of the conversion (recorded as `ingest_run_id`), not an instrument
/// or price literal, and is shaped like the staged HIP-3 bars layout:
/// `staged/v1/table=bars/run={run_id}/part-000000.jsonl`.
const FIXTURE_OBJECT_KEY: &str = concat!(
    "s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip3/",
    "staged/v1/table=bars/run=run-fixture/part-000000.jsonl"
);

#[test]
fn hip3_bars_data_derived_append_round_trips() {
    // The bulk path: enumerate the object's own series, derive precision from each
    // series' own rows (HIP-3 stages no instrument universe), build honest
    // provenance from the object bytes + key, append into a shared catalog with
    // no clean-root guard, and prove the NautilusTrader round-trip is lossless.
    let jsonl = std::fs::read_to_string(fixture_path()).expect("read committed fixture");

    // The fixture carries exactly one (instrument, interval) series, discovered
    // from the data rather than hardcoded.
    let series = hip3_bar_series(&jsonl).expect("enumerate series");
    assert_eq!(
        series.len(),
        1,
        "fixture carries exactly one (instrument, interval) series"
    );
    let selector = selector_from_fixture(&jsonl);
    assert_eq!(series[0], selector);

    // Independent expectation from the same source. The projected `Bar` payload
    // (bar_type, OHLCV, ts) is provenance-independent, so building it through the
    // public normalize path with a valid provenance yields the exact bars the
    // append path writes.
    let table = normalize_hip3_bars(&jsonl, &selector, &provenance()).expect("normalize");
    assert!(
        table.rows.len() >= 2,
        "fixture must carry a multi-bar series, got {}",
        table.rows.len()
    );
    let expected = table.to_nt_bars().expect("project to nt bars");

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let dir = TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_hyperliquid_hip3_bars_archive(jsonl.as_bytes(), FIXTURE_OBJECT_KEY, &mut catalog)
            .expect("append hip3 bars");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].nt_instrument_id, table.nt_instrument_id);
    assert_eq!(summaries[0].nt_bar_type, table.nt_bar_type);
    assert_eq!(summaries[0].record_count, table.rows.len());
    // Precision is read from the data, self-consistent with the bars built from
    // the same series — not a hardcoded assumption.
    assert_eq!(summaries[0].price_precision, table.price_precision);
    assert_eq!(summaries[0].size_precision, table.size_precision);
    assert_eq!(summaries[0].price_precision, expected[0].open.precision);
    assert_eq!(summaries[0].size_precision, expected[0].volume.precision);

    let loaded = read_back_hip3_bars(dir.path(), &table.nt_bar_type).expect("read bars back");

    // Count must match.
    assert_eq!(
        loaded.len(),
        expected.len(),
        "round-tripped bar count must match projected count"
    );

    // Ordering must be ascending by event time.
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_event <= window[1].ts_event,
            "round-tripped bars must be in ascending ts_event order"
        );
    }

    // Every field of every bar must survive the round-trip exactly, in order.
    for (i, (got, want)) in loaded.iter().zip(expected.iter()).enumerate() {
        assert_eq!(got.bar_type, want.bar_type, "bar {i}: bar_type");
        assert_eq!(got.open, want.open, "bar {i}: open");
        assert_eq!(got.high, want.high, "bar {i}: high");
        assert_eq!(got.low, want.low, "bar {i}: low");
        assert_eq!(got.close, want.close, "bar {i}: close");
        assert_eq!(got.volume, want.volume, "bar {i}: volume");
        assert_eq!(got.ts_event, want.ts_event, "bar {i}: ts_event");
        assert_eq!(
            got.bar_type.instrument_id().to_string(),
            table.nt_instrument_id,
            "bar {i}: instrument id"
        );
    }
}
