//! Round-trip proof for the Hyperliquid HIP-4 market-data converters
//! (`info.recentTrades` -> `TradeTick`, `info.candleSnapshot` -> `Bar`).
//!
//! Parses hermetic fixtures downsampled from the real staged objects:
//!   - trades: `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip4/`
//!     `staged/v1/table=trades_recent/run=run-20260601T162956Z-9a92f0c96a37/`
//!     `part-000000.jsonl` (source sha256
//!     00056c087c64c344ace130734971d130847708e27b592cd541d77087058e700e)
//!   - bars: `.../table=bars/run=run-20260601T124449Z-93e6ce55b6be/part-000000.jsonl`
//!     (source sha256 8aa70c921100520b6b9b666b0ea65df92a5865aef02149ccce65ac0f897c9c8a)
//!
//! Each kept subset is committed verbatim (real JSONL) under
//! `tests/fixtures/hyperliquid-hip4/`. Each family is normalized, projected into
//! a temporary NautilusTrader `ParquetDataCatalog` via NautilusTrader's own
//! `write_to_parquet`, then read back with `query_typed_data`, asserting count,
//! ascending timestamps, and per-record payload equality (loaded == built). That
//! proves HIP-4 trade prints and candles land in an NT catalog NautilusTrader can
//! replay. Hermetic: the test never touches S3.

use std::{fs, path::PathBuf};

use backtesting_vertical_slice::canonical_hyperliquid_hip4::{
    Hip4BarAggregation, Hip4InstrumentNaming, Hip4MarketDataSpec, NT_DATA_TYPE_BAR,
    NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_TRADE_TICK, append_hip4_bars_archive,
    append_hip4_snapshots_archive, append_hip4_trades_archive, normalize_hip4_bars,
    normalize_hip4_trades, parse_hip4_snapshots, project_hip4_bars_to_catalog,
    project_hip4_trades_to_catalog, read_back_bars, read_back_order_book_deltas,
    read_back_trade_ticks, snapshots_to_deltas,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

fn fixture(name: &str) -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "tests",
        "fixtures",
        "hyperliquid-hip4",
        name,
    ]
    .iter()
    .collect();
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

// The fixture holds coins #1010 (outcome 101 Up leg) and #1011 (Down leg).
// Built here (the test owns the sample ids); the converter takes the spec as
// input so no instrument id or precision is hardcoded in the library. HIP-4
// prediction prices need 5 decimal places and sizes 1, derived from the fixture.
fn spec_up() -> Hip4MarketDataSpec {
    Hip4MarketDataSpec {
        expected_venue: "hyperliquid".to_string(),
        trade_coin: "#1010".to_string(),
        nt_instrument_id: "OUTCOME-101-UP.HYPERLIQUID".to_string(),
        price_increment: "0.00001".to_string(),
        size_increment: "0.1".to_string(),
        bar_step: 1,
        bar_aggregation: Hip4BarAggregation::Hour,
    }
}

fn spec_down() -> Hip4MarketDataSpec {
    Hip4MarketDataSpec {
        expected_venue: "hyperliquid".to_string(),
        trade_coin: "#1011".to_string(),
        nt_instrument_id: "OUTCOME-101-DOWN.HYPERLIQUID".to_string(),
        price_increment: "0.00001".to_string(),
        size_increment: "0.1".to_string(),
        bar_step: 1,
        bar_aggregation: Hip4BarAggregation::Hour,
    }
}

#[test]
fn hip4_trades_fixture_round_trips_through_nt_catalog() {
    let jsonl = fixture("trades_recent.jsonl");

    // Two coins share the interleaved object; each spec selects exactly one.
    for spec in [spec_up(), spec_down()] {
        let table = normalize_hip4_trades(&jsonl, &spec).expect("normalize trades");
        assert!(
            !table.rows.is_empty(),
            "fixture must have prints for {}",
            spec.trade_coin
        );

        let built = table.to_trade_ticks(&spec).expect("build ticks");

        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_hip4_trades_to_catalog(&table, &spec, dir.path()).expect("project trades");
        assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
        assert_eq!(projection.nt_identifier, spec.nt_instrument_id);
        assert_eq!(projection.record_count, table.rows.len());

        let loaded = read_back_trade_ticks(dir.path(), &spec.nt_instrument_id).expect("read back");
        assert_eq!(
            loaded.len(),
            table.rows.len(),
            "round-tripped trade count must match canonical row count"
        );

        // Ascending timestamps + exact payload equality.
        let mut previous_ts = 0u64;
        for tick in &loaded {
            assert_eq!(tick.instrument_id.to_string(), spec.nt_instrument_id);
            let ts = tick.ts_event.as_u64();
            assert!(ts >= previous_ts, "ascending trade order");
            previous_ts = ts;
        }
        assert_eq!(
            loaded, built,
            "round-tripped ticks identical to built ticks (ordering + payload)"
        );
    }
}

#[test]
fn hip4_bars_fixture_round_trips_through_nt_catalog() {
    let jsonl = fixture("bars.jsonl");

    for spec in [spec_up(), spec_down()] {
        let table = normalize_hip4_bars(&jsonl, &spec).expect("normalize bars");
        assert!(
            !table.rows.is_empty(),
            "fixture must have candles for {}",
            spec.trade_coin
        );

        let built = table.to_bars(&spec).expect("build bars");

        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_hip4_bars_to_catalog(&table, &spec, dir.path()).expect("project bars");
        assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
        assert_eq!(projection.record_count, table.rows.len());

        let bar_type = table.bar_type_string(&spec).expect("bar type string");
        // The bar-type id is a superstring of the instrument id.
        assert!(
            bar_type.contains(&spec.nt_instrument_id),
            "bar type carries instrument id"
        );
        assert_eq!(projection.nt_identifier, bar_type);

        let loaded = read_back_bars(dir.path(), &bar_type).expect("read back bars");
        assert_eq!(
            loaded.len(),
            table.rows.len(),
            "round-tripped bar count must match canonical row count"
        );

        // Ascending timestamps + exact payload equality.
        let mut previous_ts = 0u64;
        for bar in &loaded {
            assert_eq!(bar.instrument_id().to_string(), spec.nt_instrument_id);
            let ts = bar.ts_event.as_u64();
            assert!(ts >= previous_ts, "ascending bar order");
            previous_ts = ts;
        }
        assert_eq!(
            loaded, built,
            "round-tripped bars identical to built bars (ordering + payload)"
        );

        // Prove the fixture carries real price structure (at least one moving bar
        // survived the round trip with high > low).
        assert!(
            loaded.iter().any(|bar| bar.high > bar.low),
            "fixture must contain at least one moving bar with real OHLC structure"
        );
    }
}

// ===========================================================================
// Bulk-append path round-trip proofs (data-derived identity + precision).
//
// Mirror the OKX bulk-append proof: derive the spec/identity from the fixture's
// own rows (no caller-supplied instrument id, no instrument universe), append
// every instrument of one object into a single fresh temp catalog with no
// clean-root guard, read each instrument back, and assert count + ascending ts +
// identical payload against the records the pure converters build.
// ===========================================================================

/// Per-venue naming format constant (NT venue code + outcome-symbol prefix +
/// expected source venue) the L2 bulk append consumes; the numeric outcome ids
/// come from the object's own records.
fn bulk_naming() -> Hip4InstrumentNaming {
    Hip4InstrumentNaming {
        nt_venue_code: "HYPERLIQUID".to_string(),
        outcome_symbol_prefix: "OUTCOME-".to_string(),
        expected_venue: "hyperliquid".to_string(),
    }
}

#[test]
fn hip4_snapshots_data_derived_append_round_trips() {
    let jsonl = fixture("order_book_snapshots_fixed_depth.jsonl");
    let naming = bulk_naming();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_hip4_snapshots_archive(&jsonl, &naming, &mut catalog).expect("append snapshots");
    assert!(
        !summaries.is_empty(),
        "object must yield outcome instruments"
    );
    assert!(
        summaries
            .iter()
            .all(|s| s.data_type == NT_DATA_TYPE_ORDER_BOOK_DELTA),
        "every summary is an OrderBookDelta stream"
    );

    // The identity is data-derived from each record's own outcome id + the venue
    // format constant.
    let table = parse_hip4_snapshots(&jsonl, &naming).expect("parse for expected ids");
    assert_eq!(
        summaries.len(),
        table.instruments.len(),
        "one summary per distinct outcome instrument"
    );

    for instrument in &table.instruments {
        let built = snapshots_to_deltas(instrument).expect("build deltas");
        let summary = summaries
            .iter()
            .find(|s| s.nt_identifier == instrument.nt_instrument_id)
            .expect("summary present for instrument");
        assert_eq!(summary.record_count, built.len(), "summary count matches");

        let loaded = read_back_order_book_deltas(dir.path(), &instrument.nt_instrument_id)
            .expect("read back deltas");
        assert_eq!(
            loaded.len(),
            built.len(),
            "{}: round-tripped delta count",
            instrument.nt_instrument_id
        );

        let mut previous_ts = 0u64;
        for delta in &loaded {
            assert_eq!(delta.instrument_id.to_string(), instrument.nt_instrument_id);
            let ts = delta.ts_event.as_u64();
            assert!(ts >= previous_ts, "ascending delta order");
            previous_ts = ts;
        }
        assert_eq!(
            loaded, built,
            "{}: round-tripped deltas identical to built deltas",
            instrument.nt_instrument_id
        );
    }
}

/// The fixture's `(trade_coin, catalog instrument id)` mapping, owned by the test
/// (the test owns the sample data). The catalog id is the URI-safe
/// `<prefix><outcome>-<side>.<venue>` the append path derives from each record's
/// own `(outcome, side)`; it deliberately does NOT carry the raw `#`-prefixed HL
/// `trade_coin` handle (an id with `#` is a URI fragment that `ParquetDataCatalog`
/// mangles on read-back). `#1010` is outcome 101 side 0; `#1011` is outcome 101
/// side 1.
fn expected_coin_ids() -> [(&'static str, &'static str); 2] {
    [
        ("#1010", "OUTCOME-101-0.HYPERLIQUID"),
        ("#1011", "OUTCOME-101-1.HYPERLIQUID"),
    ]
}

#[test]
fn hip4_trades_data_derived_append_round_trips() {
    let jsonl = fixture("trades_recent.jsonl");
    let naming = bulk_naming();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_hip4_trades_archive(&jsonl, &naming, &mut catalog).expect("append trades");
    // The fixture interleaves coins #1010 and #1011.
    assert_eq!(summaries.len(), 2, "two distinct coins in the object");
    assert!(
        summaries
            .iter()
            .all(|s| s.data_type == NT_DATA_TYPE_TRADE_TICK),
        "every summary is a TradeTick stream"
    );

    for (trade_coin, expected_id) in expected_coin_ids() {
        let summary = summaries
            .iter()
            .find(|s| s.nt_identifier == expected_id)
            .unwrap_or_else(|| panic!("summary present for {expected_id}"));

        // The catalog id is the URI-safe outcome/side identity (no `#`), derived
        // from this coin's own records the same way the L2 snapshot family does.
        assert!(
            summary.nt_identifier.ends_with(".HYPERLIQUID"),
            "data-derived id carries the venue code: {}",
            summary.nt_identifier
        );
        assert!(
            summary.nt_identifier.starts_with("OUTCOME-"),
            "data-derived id uses the outcome-symbol prefix: {}",
            summary.nt_identifier
        );
        assert!(
            !summary.nt_identifier.contains('#'),
            "catalog id must be URI-safe (no `#`): {}",
            summary.nt_identifier
        );

        // Independently rebuild the expected ticks for this coin from the same
        // data-derived spec the append fn used (the `trade_coin` fence + the
        // catalog id + precision from this coin's rows).
        let spec = Hip4MarketDataSpec {
            expected_venue: "hyperliquid".to_string(),
            trade_coin: trade_coin.to_string(),
            nt_instrument_id: summary.nt_identifier.clone(),
            price_increment: increment_for(summary.price_precision),
            size_increment: increment_for(summary.size_precision),
            bar_step: 1,
            bar_aggregation: Hip4BarAggregation::Minute,
        };
        let table = normalize_hip4_trades(&jsonl, &spec).expect("normalize coin");
        let built = table.to_trade_ticks(&spec).expect("build ticks");
        assert_eq!(summary.record_count, built.len(), "summary count matches");

        let loaded =
            read_back_trade_ticks(dir.path(), &summary.nt_identifier).expect("read back ticks");
        assert_eq!(
            loaded.len(),
            built.len(),
            "{}: round-tripped tick count",
            summary.nt_identifier
        );

        let mut previous_ts = 0u64;
        for tick in &loaded {
            assert_eq!(tick.instrument_id.to_string(), summary.nt_identifier);
            let ts = tick.ts_event.as_u64();
            assert!(ts >= previous_ts, "ascending trade order");
            previous_ts = ts;
        }
        assert_eq!(
            loaded, built,
            "{}: round-tripped ticks identical to built ticks",
            summary.nt_identifier
        );
    }
}

#[test]
fn hip4_bars_data_derived_append_round_trips() {
    let jsonl = fixture("bars.jsonl");
    let naming = bulk_naming();

    let dir = tempfile::TempDir::new().expect("temp dir");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries = append_hip4_bars_archive(&jsonl, &naming, &mut catalog).expect("append bars");
    assert_eq!(summaries.len(), 2, "two distinct coins in the object");
    assert!(
        summaries.iter().all(|s| s.data_type == NT_DATA_TYPE_BAR),
        "every summary is a Bar stream"
    );

    // Each bar-type identifier is a superstring of the URI-safe instrument id the
    // append path derives from the coin's own `(outcome, side)`
    // (`<prefix><outcome>-<side>.HYPERLIQUID-<step>-<unit>-LAST-EXTERNAL`).
    for (_, expected_id) in expected_coin_ids() {
        assert!(
            summaries
                .iter()
                .any(|s| s.nt_identifier.starts_with(&format!("{expected_id}-"))),
            "a bar-type for {expected_id} must be present; got {:?}",
            summaries
                .iter()
                .map(|s| &s.nt_identifier)
                .collect::<Vec<_>>(),
        );
    }

    for summary in &summaries {
        assert!(
            summary.nt_identifier.contains(".HYPERLIQUID"),
            "bar-type carries the venue code: {}",
            summary.nt_identifier
        );
        assert!(
            summary.nt_identifier.starts_with("OUTCOME-"),
            "bar-type carries the URI-safe outcome/side instrument id: {}",
            summary.nt_identifier
        );
        assert!(
            !summary.nt_identifier.contains('#'),
            "bar-type id must be URI-safe (no `#`): {}",
            summary.nt_identifier
        );

        let loaded = read_back_bars(dir.path(), &summary.nt_identifier).expect("read back bars");
        assert_eq!(
            loaded.len(),
            summary.record_count,
            "{}: round-tripped bar count matches summary",
            summary.nt_identifier
        );

        let mut previous_ts = 0u64;
        let mut any_moving = false;
        for bar in &loaded {
            let ts = bar.ts_event.as_u64();
            assert!(ts >= previous_ts, "ascending bar order");
            previous_ts = ts;
            if bar.high > bar.low {
                any_moving = true;
            }
        }
        assert!(
            any_moving,
            "{}: at least one moving bar survived the round trip",
            summary.nt_identifier
        );
    }
}

/// Local copy of the converter's data-derived increment builder, used by the
/// trades round-trip proof to rebuild the expected ticks at the same precision
/// the append fn derived. Kept in the test so the proof depends only on the
/// public surface and the observed precision the summary reports.
fn increment_for(precision: u8) -> String {
    match precision {
        0 => "1".to_string(),
        n => format!("0.{}1", "0".repeat(usize::from(n) - 1)),
    }
}
