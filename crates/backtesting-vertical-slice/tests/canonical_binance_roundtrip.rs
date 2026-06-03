//! Round-trip proof for the Binance venue converter.
//!
//! Reads the committed Binance public-archive fixtures (decompressed CSV from
//! `data.binance.vision`), normalizes them, projects them into a NautilusTrader
//! `ParquetDataCatalog` via NautilusTrader's own `write_to_parquet`, then reads
//! them back with `query_typed_data`. Asserting the round-tripped count and
//! ordering proves the data is in an NT catalog NautilusTrader can replay
//! ("backtestable"):
//!
//! - `trades` family -> `TradeTick`
//! - `klines` family -> `Bar`
//!
//! Hermetic: the fixtures are committed under `tests/fixtures/binance/`; this
//! test never touches S3.

use std::{fs, path::PathBuf, str::FromStr};

use backtesting_vertical_slice::canonical_binance::{
    BinanceInstrumentIdentity, BinanceInstrumentSpec, BinanceProvenance, KlineBarSpec,
    NT_DATA_TYPE_BAR, NT_DATA_TYPE_TRADE_TICK, normalize_binance_klines, normalize_binance_trades,
    project_klines_to_catalog, project_trades_to_catalog, read_back_bars, read_back_trade_ticks,
};
use nautilus_model::{
    enums::{AggressorSide, BarAggregation},
    types::Price,
};

const NT_INSTRUMENT_ID: &str = "XRPTUSD.BINANCE";

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

fn provenance() -> BinanceProvenance {
    BinanceProvenance {
        ingest_run_id: "binance-roundtrip-test".to_string(),
        source_binding: "binance-spot".to_string(),
        venue: "binance".to_string(),
        product_family: "spot".to_string(),
        product_category: "spot".to_string(),
        source_proof_id: "source-proof-binance-spot-fixture".to_string(),
        payload_hash: "119eff2919bb79abe02c51221734ffd96bd3a01a5657a3cc3a7ee1e7d12c46d6"
            .to_string(),
        archive_date: "2026-04".to_string(),
    }
}

fn identity() -> BinanceInstrumentIdentity {
    BinanceInstrumentIdentity {
        instrument_id: "XRPTUSD".to_string(),
        venue_symbol: "XRPTUSD".to_string(),
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
    }
}

// Source price/qty carry 8 decimals; an 8-decimal increment represents them
// exactly. Built from the accepted instrument universe in production; literal
// here only because this is a test fixture.
fn spec() -> BinanceInstrumentSpec {
    BinanceInstrumentSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: "XRPTUSD".to_string(),
        base_currency: "XRP".to_string(),
        quote_currency: "TUSD".to_string(),
        price_increment: "0.00000001".to_string(),
        size_increment: "0.00000001".to_string(),
        min_quantity: "0.00000001".to_string(),
        max_quantity: "9000000.00000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "9000000".to_string(),
    }
}

#[test]
fn trades_fixture_round_trips_through_nt_catalog() {
    let csv = fixture("XRPTUSD-trades-2026-04.csv");
    let table = normalize_binance_trades(&provenance(), &identity(), &csv).expect("normalize");
    assert!(!table.rows.is_empty(), "fixture must have rows");

    // The expected NautilusTrader ticks, computed from the canonical rows, are
    // the ground truth for the round-trip comparison.
    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_trades_to_catalog(&table, &spec(), dir.path()).expect("project trades");
    assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(projection.record_count, table.rows.len());

    let loaded = read_back_trade_ticks(dir.path(), NT_INSTRUMENT_ID).expect("read back");

    // Count must match exactly.
    assert_eq!(
        loaded.len(),
        table.rows.len(),
        "round-tripped trade count must match canonical row count"
    );

    // Ordering + per-tick identity must match canonical order.
    let mut previous_ts = 0u64;
    for (i, tick) in loaded.iter().enumerate() {
        assert_eq!(tick.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(
            tick.trade_id.to_string(),
            table.rows[i].trade_id,
            "trade id order must be preserved at index {i}"
        );
        let expected_aggressor = match table.rows[i].aggressor_side.as_str() {
            "BUYER" => AggressorSide::Buyer,
            "SELLER" => AggressorSide::Seller,
            other => panic!("unexpected aggressor {other}"),
        };
        assert_eq!(tick.aggressor_side, expected_aggressor, "aggressor at {i}");
        let ts = tick.ts_event.as_u64();
        assert_eq!(
            ts,
            u64::try_from(table.rows[i].event_time).unwrap(),
            "event_time at {i}"
        );
        assert!(ts >= previous_ts, "ascending order at {i}");
        previous_ts = ts;
    }
}

#[test]
fn klines_fixture_round_trips_through_nt_catalog() {
    let csv = fixture("XRPTUSD-1m-2026-04.csv");
    let bar_spec = KlineBarSpec {
        step: 1,
        aggregation: BarAggregation::Minute,
    };
    let table =
        normalize_binance_klines(&provenance(), &identity(), bar_spec, &csv).expect("normalize");
    assert!(!table.rows.is_empty(), "fixture must have rows");

    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection =
        project_klines_to_catalog(&table, &spec(), dir.path()).expect("project klines");
    assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(projection.record_count, table.rows.len());

    let loaded = read_back_bars(dir.path(), NT_INSTRUMENT_ID).expect("read back");

    // Count must match exactly.
    assert_eq!(
        loaded.len(),
        table.rows.len(),
        "round-tripped bar count must match canonical row count"
    );

    // Ordering + per-bar timestamps must match canonical order.
    let mut previous_ts = 0u64;
    for (i, bar) in loaded.iter().enumerate() {
        assert_eq!(bar.instrument_id().to_string(), NT_INSTRUMENT_ID);
        let ts = bar.ts_event.as_u64();
        assert_eq!(
            ts,
            u64::try_from(table.rows[i].close_time).unwrap(),
            "bar close_time at {i}"
        );
        // Bars are emitted in ascending time order.
        assert!(ts >= previous_ts, "ascending bar order at {i}");
        previous_ts = ts;
    }

    // Spot-check that OHLC survived the round trip on a bar with movement
    // (high != low), proving the projection carries real price structure.
    let moving = loaded
        .iter()
        .zip(table.rows.iter())
        .find(|(_, row)| row.high != row.low)
        .expect("fixture must contain at least one moving bar");
    let (bar, row) = moving;
    // Compare numerically: `Price` Display trims trailing zeros, so compare the
    // round-tripped price against the source value parsed at the same precision.
    assert_eq!(
        bar.high,
        Price::from_str(&row.high).expect("parse high"),
        "high preserved"
    );
    assert_eq!(
        bar.low,
        Price::from_str(&row.low).expect("parse low"),
        "low preserved"
    );
    assert!(bar.high > bar.low, "moving bar must have high > low");
}
