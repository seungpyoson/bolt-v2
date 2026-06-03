//! Round-trip proof for the Hyperliquid core (perp DEX) L2 converter.
//!
//! Hermetic: reads a committed, downsampled real `l2Book` fixture (8 snapshots of
//! one Hyperliquid core coin, captured 2025-06-01, lz4-decompressed to JSONL), then
//!
//!   fixture JSONL
//!     -> reconstruct snapshots into NautilusTrader `OrderBookDelta`s
//!        (Clear + Add-per-level, bids then asks)
//!     -> write_to_parquet into a temp NautilusTrader `ParquetDataCatalog`
//!     -> query_typed_data back
//!     -> assert the round-tripped count, ordering, and per-delta payload match.
//!
//! This proves Hyperliquid core's snapshot-only L2 data lands in an NT-replayable
//! catalog as `OrderBookDelta`, the venue's required NT type. No S3 access — the
//! fixture is committed under `tests/fixtures/hyperliquid-core/`.

use backtesting_vertical_slice::canonical_hyperliquid_core::{
    HyperliquidCoreInstrumentSpec, NT_DATA_TYPE_ORDER_BOOK_DELTA, project_books_to_catalog,
    read_back_order_book_deltas, reconstruct_books,
};
use nautilus_model::instruments::Instrument;

/// The committed, downsampled real Hyperliquid core `l2Book` capture (decompressed
/// JSONL). Embedded at compile time so the test never touches S3 or lz4.
const FIXTURE: &str = include_str!("fixtures/hyperliquid-core/bnb_l2book_sample.jsonl");

/// Instrument identity for the fixture coin. In production this is bound from the
/// accepted instrument universe; here it is a test literal matching the fixture's
/// `coin`. Hyperliquid core markets are USDC-settled crypto perpetuals.
fn spec() -> HyperliquidCoreInstrumentSpec {
    HyperliquidCoreInstrumentSpec {
        nt_instrument_id: "BNB.HYPERLIQUID".to_string(),
        raw_symbol: "BNB".to_string(),
        base_currency: "BNB".to_string(),
        quote_currency: "USDC".to_string(),
        settlement_currency: "USDC".to_string(),
    }
}

#[test]
fn hyperliquid_core_snapshots_round_trip_through_nt_catalog() {
    // Reconstruct deltas from the real fixture.
    let book = reconstruct_books(FIXTURE, &spec()).expect("reconstruct books from fixture");

    // The fixture is 8 full 20x20 photos: each snapshot -> 1 Clear + 20 bids + 20
    // asks = 41 deltas, so 8 * 41 = 328 deltas.
    assert_eq!(book.snapshot_count, 8, "fixture carries 8 snapshots");
    assert_eq!(
        book.deltas.len(),
        328,
        "8 snapshots * (1 Clear + 40 Adds) = 328 deltas"
    );
    assert_eq!(book.instrument.id().to_string(), "BNB.HYPERLIQUID");

    // Project into a fresh NautilusTrader ParquetDataCatalog (NT's own write path).
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let projection = project_books_to_catalog(&book, dir.path()).expect("project into NT catalog");
    assert_eq!(projection.delta_count, book.deltas.len());
    assert_eq!(projection.snapshot_count, book.snapshot_count);
    assert_eq!(projection.data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
    assert_eq!(projection.nt_instrument_id, "BNB.HYPERLIQUID");

    // Read the deltas back via NautilusTrader's typed query path.
    let loaded =
        read_back_order_book_deltas(dir.path(), "BNB.HYPERLIQUID").expect("read deltas back");

    // Count survives the round-trip.
    assert_eq!(
        loaded.len(),
        book.deltas.len(),
        "every reconstructed delta round-trips through the NT catalog"
    );

    // Full payload + ordering survive the round-trip. OrderBookDelta derives
    // PartialEq over all fields (action, flags, sequence, timestamps, and the
    // order's id), so this asserts byte-faithful, order-preserving replay.
    assert_eq!(
        loaded, book.deltas,
        "round-tripped deltas match the reconstructed deltas exactly, in order"
    );

    // Belt-and-braces: BookOrder::PartialEq compares only order_id, so assert the
    // price/size/side of a representative Add explicitly to prove the order payload
    // (not just the id) survived the catalog round-trip.
    let first_add = loaded
        .iter()
        .find(|d| d.action == nautilus_model::enums::BookAction::Add)
        .expect("at least one Add round-trips");
    let expected_add = book
        .deltas
        .iter()
        .find(|d| d.action == nautilus_model::enums::BookAction::Add)
        .expect("reconstructed Add present");
    assert_eq!(first_add.order.price, expected_add.order.price);
    assert_eq!(first_add.order.size, expected_add.order.size);
    assert_eq!(first_add.order.side, expected_add.order.side);

    // Monotonic non-decreasing ts_init (the ordering NT replay relies on).
    for window in loaded.windows(2) {
        assert!(
            window[0].ts_init <= window[1].ts_init,
            "deltas are emitted in non-decreasing ts_init order"
        );
    }
}
