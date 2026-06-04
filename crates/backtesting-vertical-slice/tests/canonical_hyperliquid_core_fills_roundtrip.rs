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
    HyperliquidCoreInstrumentSpec, NT_DATA_TYPE_TRADE_TICK, append_hyperliquid_core_fills_archive,
    decompress_lz4_frame, hyperliquid_core_fill_coins, hyperliquid_core_fills_spec_for_coin,
    is_core_perp_coin, project_trades_to_catalog, read_back_trade_ticks, read_lz4_jsonl,
    reconstruct_trades,
};
use nautilus_model::instruments::Instrument;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

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

/// Venue suffix and exotic coin codes the bulk-path proof asserts on. `BTC` is a
/// statically-registered NautilusTrader currency; `PUMP` is a real Hyperliquid
/// core perp that is NOT in NautilusTrader's static currency map (it must round
/// trip via the converter's `get_or_create_crypto` registration). `xyz:SILVER`
/// (HIP-3 DEX) and `@107` (spot index) are non-core markets that MUST be fenced.
const VENUE_SUFFIX: &str = ".HYPERLIQUID";
const REGISTERED_COIN: &str = "BTC";
const UNLISTED_CORE_COIN: &str = "PUMP";
const FOREIGN_HIP3_COIN: &str = "xyz:SILVER";
const FOREIGN_SPOT_COIN: &str = "@107";

#[test]
fn hyperliquid_core_fills_data_derived_append_round_trips() {
    // The bulk path: a single node_fills_by_block object multiplexes many coins.
    // Derive precision per coin from the object's own rows (Hyperliquid stages no
    // instrument universe), fence non-core markets, register unlisted coins, then
    // append into a shared catalog with no clean-root guard and prove the NT
    // round-trip is lossless.
    let lz4_bytes = std::fs::read(fixture_path()).expect("read lz4 fixture bytes");

    // Decompress once for the independent expectation (same source the append fn
    // decompresses internally).
    let jsonl = decompress_lz4_frame(&lz4_bytes).expect("decompress lz4 bytes");
    assert_eq!(
        jsonl,
        read_lz4_jsonl(&fixture_path()).expect("decompress via path"),
        "in-memory and path decompression agree"
    );

    // The enumerator fences HIP-3 DEX tokens and spot indices but keeps core perps.
    let coins = hyperliquid_core_fill_coins(&jsonl).expect("enumerate core coins");
    assert!(
        coins.iter().any(|c| c == REGISTERED_COIN),
        "core perp BTC is enumerated"
    );
    assert!(
        coins.iter().any(|c| c == UNLISTED_CORE_COIN),
        "unlisted core perp PUMP is enumerated"
    );
    assert!(
        !coins.iter().any(|c| c == FOREIGN_HIP3_COIN),
        "HIP-3 DEX token must be fenced out of the core family"
    );
    assert!(
        !coins.iter().any(|c| c == FOREIGN_SPOT_COIN),
        "spot index token must be fenced out of the core family"
    );
    assert!(
        coins.iter().all(|c| is_core_perp_coin(c)),
        "every enumerated coin classifies as a core perp"
    );

    // Independent expectation for BTC via the same data-derived spec the bulk path
    // builds.
    let btc_spec = hyperliquid_core_fills_spec_for_coin(REGISTERED_COIN).expect("btc spec");
    assert_eq!(
        btc_spec.nt_instrument_id,
        format!("{REGISTERED_COIN}{VENUE_SUFFIX}")
    );
    let expected_btc = reconstruct_trades(&jsonl, &btc_spec).expect("reconstruct BTC expectation");

    // Append the whole object into a freshly-opened (empty) catalog — no dirty-root
    // refusal. This registers unlisted coins (PUMP) as crypto currencies internally.
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_hyperliquid_core_fills_archive(&lz4_bytes, &mut catalog).expect("append fills");

    // One summary per enumerated core coin, no foreign markets.
    assert_eq!(
        summaries.len(),
        coins.len(),
        "one write summary per enumerated core coin"
    );
    assert!(
        summaries
            .iter()
            .all(|s| s.nt_instrument_id.ends_with(VENUE_SUFFIX)),
        "every written instrument carries the venue suffix"
    );
    assert!(
        !summaries.iter().any(|s| {
            s.nt_instrument_id.contains(FOREIGN_HIP3_COIN)
                || s.nt_instrument_id.contains(FOREIGN_SPOT_COIN)
        }),
        "no foreign (HIP-3 / spot) instrument is written"
    );

    let btc_summary = summaries
        .iter()
        .find(|s| s.nt_instrument_id == btc_spec.nt_instrument_id)
        .expect("BTC summary present");
    assert_eq!(btc_summary.record_count, expected_btc.trades.len());
    // Precision is read from the data, self-consistent with the ticks built from
    // the same derived spec — never a hardcoded assumption.
    assert_eq!(btc_summary.price_precision, expected_btc.price_precision);
    assert_eq!(btc_summary.size_precision, expected_btc.size_precision);
    assert_eq!(
        btc_summary.price_precision,
        expected_btc.trades[0].price.precision
    );

    // BTC (statically-registered currency) round-trips identically.
    let loaded_btc =
        read_back_trade_ticks(dir.path(), &btc_spec.nt_instrument_id).expect("read back BTC");
    assert_eq!(loaded_btc.len(), expected_btc.trades.len());
    assert!(
        loaded_btc.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded BTC ticks ascending"
    );
    assert_eq!(
        loaded_btc, expected_btc.trades,
        "data-derived BTC append round-trips identically (count, ordering, payload, precision)"
    );

    // PUMP (NOT in NautilusTrader's static currency map) also round-trips, proving
    // the converter's get_or_create_crypto registration admits unlisted core perps
    // rather than dropping them.
    let pump_spec = hyperliquid_core_fills_spec_for_coin(UNLISTED_CORE_COIN).expect("pump spec");
    let expected_pump =
        reconstruct_trades(&jsonl, &pump_spec).expect("reconstruct PUMP expectation");
    let loaded_pump =
        read_back_trade_ticks(dir.path(), &pump_spec.nt_instrument_id).expect("read back PUMP");
    assert_eq!(
        loaded_pump.len(),
        expected_pump.trades.len(),
        "unlisted core perp round-trips"
    );
    assert_eq!(
        loaded_pump, expected_pump.trades,
        "unlisted-coin append round-trips identically"
    );
}
