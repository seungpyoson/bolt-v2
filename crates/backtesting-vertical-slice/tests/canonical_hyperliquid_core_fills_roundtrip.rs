//! Round-trip proof for the Hyperliquid core `node_fills_by_block` -> `TradeTick`
//! converter.
//!
//! Hermetic: reads a committed, downsampled real `node_fills_by_block` fixture
//! (42 blocks of one real Hyperliquid object, captured 2026-03-01, kept in the
//! source LZ4-frame format), then
//!
//!   fixture .lz4  (LZ4 frame, real on-disk format)
//!     -> read_lz4_jsonl decompresses to JSONL
//!     -> reconstruct_trades: dedup by tid, keep the taker (crossed=true) leg,
//!        map side -> aggressor, derive precision, sort by event time
//!     -> write_to_parquet into a temp NautilusTrader `ParquetDataCatalog`
//!     -> query_typed_data back
//!     -> assert the round-tripped count, ascending ts, and per-tick payload match.
//!
//! This proves Hyperliquid core's per-block fills land in an NT-replayable catalog
//! as `TradeTick`, the venue's required NT type for this family. No S3 access — the
//! fixture is committed under `tests/fixtures/hyperliquid-core/`.

use std::path::PathBuf;

use backtesting_vertical_slice::canonical_hyperliquid_core::{
    HyperliquidCoreInstrumentSpec, NT_DATA_TYPE_TRADE_TICK, project_trades_to_catalog,
    read_back_trade_ticks, read_lz4_jsonl, reconstruct_trades,
};
use nautilus_model::instruments::Instrument;

/// Path to the committed LZ4-frame fixture (one real object's BTC fills, downsampled).
fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/hyperliquid-core/node_fills_by_block_btc.jsonl.lz4")
}

/// Instrument identity for the fixture coin. In production this is bound from the
/// accepted instrument universe; here it is a test literal matching a real coin in
/// the fixture. Hyperliquid core markets are USDC-settled crypto perpetuals.
fn spec() -> HyperliquidCoreInstrumentSpec {
    HyperliquidCoreInstrumentSpec {
        nt_instrument_id: "BTC.HYPERLIQUID".to_string(),
        raw_symbol: "BTC".to_string(),
        base_currency: "BTC".to_string(),
        quote_currency: "USDC".to_string(),
        settlement_currency: "USDC".to_string(),
    }
}

#[test]
fn hyperliquid_core_fills_round_trip_through_nt_catalog() {
    // Decompress the real LZ4-frame fixture, then reconstruct trades.
    let jsonl = read_lz4_jsonl(&fixture_path()).expect("decompress lz4 fixture");
    let reconstructed =
        reconstruct_trades(&jsonl, &spec()).expect("reconstruct trades from fixture");

    // 134 unique BTC trades (deduplicated by tid; maker legs and other coins
    // dropped). Precision derived from the data: px 1 dp, sz 5 dp.
    assert_eq!(
        reconstructed.trade_count, 134,
        "fixture carries 134 unique BTC taker trades"
    );
    assert_eq!(reconstructed.trades.len(), 134);
    assert_eq!(reconstructed.price_precision, 1);
    assert_eq!(reconstructed.size_precision, 5);
    assert_eq!(reconstructed.instrument.id().to_string(), "BTC.HYPERLIQUID");

    // Reconstructed trades are sorted ascending by ts_init (NT write contract).
    for window in reconstructed.trades.windows(2) {
        assert!(
            window[0].ts_init <= window[1].ts_init,
            "trades are emitted in non-decreasing ts_init order before writing"
        );
    }

    // Project into a fresh NautilusTrader ParquetDataCatalog (NT's own write path).
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let projection =
        project_trades_to_catalog(&reconstructed, dir.path()).expect("project into NT catalog");
    assert_eq!(projection.trade_count, reconstructed.trades.len());
    assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
    assert_eq!(projection.nt_instrument_id, "BTC.HYPERLIQUID");

    // Read the trades back via NautilusTrader's typed query path.
    let loaded =
        read_back_trade_ticks(dir.path(), "BTC.HYPERLIQUID").expect("read trade ticks back");

    // Count survives the round-trip.
    assert_eq!(
        loaded.len(),
        reconstructed.trades.len(),
        "every reconstructed trade round-trips through the NT catalog"
    );

    // Full payload + ordering survive the round-trip. TradeTick derives PartialEq
    // over instrument_id, price, size, aggressor_side, trade_id, ts_event, ts_init,
    // so this asserts byte-faithful, order-preserving replay.
    assert_eq!(
        loaded, reconstructed.trades,
        "round-tripped trades match the reconstructed trades exactly, in order"
    );

    // Belt-and-braces: assert the loaded stream is itself ascending by ts_event,
    // the ordering NT replay relies on.
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_event <= window[1].ts_event,
            "loaded trades replay in non-decreasing ts_event order"
        );
    }
}
