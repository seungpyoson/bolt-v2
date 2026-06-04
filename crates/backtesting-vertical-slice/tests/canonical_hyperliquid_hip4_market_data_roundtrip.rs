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
    Hip4BarAggregation, Hip4MarketDataSpec, NT_DATA_TYPE_BAR, NT_DATA_TYPE_TRADE_TICK,
    normalize_hip4_bars, normalize_hip4_trades, project_hip4_bars_to_catalog,
    project_hip4_trades_to_catalog, read_back_bars, read_back_trade_ticks,
};

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
