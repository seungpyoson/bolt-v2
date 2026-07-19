//! NautilusTrader catalog read proof for the canonical order-book-delta
//! projection.
//!
//! Proves, against the NautilusTrader dependency resolved by this `bolt-v2`
//! branch, that a validated [`CanonicalOrderBookDeltasTable`] projects into a
//! local `ParquetDataCatalog` as `OrderBookDelta` data and reads the exact same
//! deltas back, including the snapshot-expansion flag contract, dirty-root
//! refusal, data-driven precision widening, and a stable logical catalog hash.
//!
//! The fixtures are synthetic and venue-free: the projection behaviour under
//! test is data-driven and must not be tied to any real venue, token, or
//! incident value (same precedent as the in-module `BASEQUOTE.TESTVENUE`
//! fixtures).

use backtesting_vertical_slice::{
    canonical_market_data::{
        CanonicalOrderBookDeltaRow, CanonicalOrderBookDeltasTable, DeltaAction, DeltaSide,
        NORMALIZED_SCHEMA_VERSION,
    },
    canonical_trades::TradesPartition,
    catalog_projection::{
        BinaryOptionInstrumentKind, BinaryOptionInstrumentSpec, SpotInstrumentSpec,
        project_canonical_order_book_deltas_to_catalog, read_back_order_book_deltas,
    },
    source_proof::SourceProofFidelityClass,
};
use nautilus_model::{
    enums::{BookAction, OrderSide, RecordFlag},
    instruments::InstrumentAny,
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

const NT_INSTRUMENT_ID: &str = "BASEQUOTE.TESTVENUE";
const INSTRUMENT_ID: &str = "BASEQUOTE";
const BASE_EVENT_TIME: i64 = 1_700_000_000_000_000_000;

fn test_catalog_encoding() -> backtesting_vertical_slice::artifact_store::CatalogEncodingConfig {
    backtesting_vertical_slice::artifact_store::CatalogEncodingConfig::new(
        5000,
        5000,
        backtesting_vertical_slice::artifact_store::CatalogCompression::Snappy,
    )
    .expect("positive test catalog encoding")
}

fn spec() -> SpotInstrumentSpec {
    SpotInstrumentSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: INSTRUMENT_ID.to_string(),
        base_currency: "BASE".to_string(),
        quote_currency: "QUOTE".to_string(),
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        min_quantity: "0.001".to_string(),
        max_quantity: "1000000".to_string(),
        min_notional: "1".to_string(),
        max_notional: "100000000".to_string(),
    }
}

/// A binary-option spec carrying the SAME nt_instrument_id as the spot spec, so
/// the same synthetic prediction-market deltas project through NT's
/// `BinaryOption` constructor via the generic catalog seam. Prediction-market
/// archives resolve to one settlement currency over a bounded epoch rather than
/// a base/quote pair.
fn binary_option_spec() -> BinaryOptionInstrumentSpec {
    BinaryOptionInstrumentSpec {
        instrument_kind: BinaryOptionInstrumentKind::BinaryOption,
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        raw_symbol: INSTRUMENT_ID.to_string(),
        asset_class: "ALTERNATIVE".to_string(),
        currency: "USDC".to_string(),
        activation_time_nanos: 1_700_000_000_000_000_000,
        expiration_time_nanos: 1_700_086_400_000_000_000,
        price_increment: "0.01".to_string(),
        size_increment: "0.001".to_string(),
        outcome: Some("Yes".to_string()),
        description: Some("Bounded binary option fixture".to_string()),
        max_quantity: Some("1000000".to_string()),
        min_quantity: Some("0.001".to_string()),
        // Optional risk and bound metadata is outside this delta fixture's scope.
        max_notional: None,
        min_notional: None,
        max_price: None,
        min_price: None,
        margin_init: None,
        margin_maint: None,
        maker_fee: Some("0".to_string()),
        taker_fee: Some("0".to_string()),
    }
}

fn delta_row(
    sequence: u64,
    event_time: i64,
    action: DeltaAction,
    side: &str,
    price: &str,
    size: &str,
    flags: u8,
) -> CanonicalOrderBookDeltaRow {
    CanonicalOrderBookDeltaRow {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        ingest_run_id: "ingest-run-test".to_string(),
        source_binding: "synthetic-archive".to_string(),
        venue: "testvenue".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary".to_string(),
        instrument_id: INSTRUMENT_ID.to_string(),
        canonical_instrument_key: "testvenue/prediction-market/BASEQUOTE".to_string(),
        venue_symbol: INSTRUMENT_ID.to_string(),
        nt_instrument_id: Some(NT_INSTRUMENT_ID.to_string()),
        event_time,
        capture_time: event_time,
        availability_time: None,
        source_sequence: Some(sequence.to_string()),
        raw_payload_id: "feedface".to_string(),
        source_proof_id: "source-proof-synthetic".to_string(),
        payload_hash: "feedface".to_string(),
        transform_hash: "0badc0de".to_string(),
        action: action.as_str().to_string(),
        side: side.to_string(),
        price: price.to_string(),
        size: size.to_string(),
        order_id: 0,
        flags,
        sequence,
    }
}

fn table_with_rows(rows: Vec<CanonicalOrderBookDeltaRow>) -> CanonicalOrderBookDeltasTable {
    CanonicalOrderBookDeltasTable {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        partition: TradesPartition {
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: INSTRUMENT_ID.to_string(),
            dt: "2026-05-22".to_string(),
        },
        source_proof_id: "source-proof-synthetic".to_string(),
        source_proof_version: 1,
        fidelity_class: SourceProofFidelityClass::L2Replay,
        forbidden_claims: vec!["No execution-quality claims.".to_string()],
        transform_hash: "0badc0de".to_string(),
        payload_hash: "feedface".to_string(),
        rows,
    }
}

/// One snapshot expansion (Clear + 2 bid/ask Adds) followed by one standalone
/// single-level Update and one standalone single-level Delete.
///
/// The Delete row (sequence 4) is included so that Fix 1 (action assertion for
/// UPDATE/DELETE) and Fix 2 (DELETE branch coverage) are both exercised by the
/// shared round-trip fixture.  DELETE carries a non-zero size in this fixture;
/// the canonical validation intentionally skips the positive-size check for
/// DELETE (level-removal may carry size 0), but a non-zero value is also valid
/// and avoids coupling the round-trip fixture to that edge case.
fn snapshot_then_delta_table() -> CanonicalOrderBookDeltasTable {
    let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
    let last = RecordFlag::F_LAST as u8;
    let mbp = RecordFlag::F_MBP as u8;
    let rows = vec![
        delta_row(
            0,
            BASE_EVENT_TIME,
            DeltaAction::Clear,
            "",
            "",
            "",
            snapshot_flags,
        ),
        delta_row(
            1,
            BASE_EVENT_TIME,
            DeltaAction::Add,
            DeltaSide::Buy.as_str(),
            "0.49",
            "10",
            snapshot_flags,
        ),
        delta_row(
            2,
            BASE_EVENT_TIME,
            DeltaAction::Add,
            DeltaSide::Sell.as_str(),
            "0.51",
            "12",
            snapshot_flags | last,
        ),
        delta_row(
            3,
            BASE_EVENT_TIME + 1,
            DeltaAction::Update,
            DeltaSide::Buy.as_str(),
            "0.48",
            "5",
            mbp | last,
        ),
        // Sequence 4: a standalone DELETE removes the sell-side level.  This
        // row exercises both the DELETE validation branch and the
        // DELETE -> BookAction::Delete conversion mapping in the round-trip.
        delta_row(
            4,
            BASE_EVENT_TIME + 2,
            DeltaAction::Delete,
            DeltaSide::Sell.as_str(),
            "0.51",
            "12",
            mbp | last,
        ),
    ];
    table_with_rows(rows)
}

#[test]
fn deltas_round_trip_through_nt_catalog() {
    let table = snapshot_then_delta_table();
    let dir = tempfile::TempDir::new().expect("temp dir");

    let projection = project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );
    assert!(!projection.catalog_hash.is_empty());

    let loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), table.rows.len());

    // Read-back is sorted by NautilusTrader; assert per-field equality against
    // the canonical rows keyed by sequence.
    let mut by_sequence = loaded.clone();
    by_sequence.sort_by_key(|delta| delta.sequence);
    for (delta, row) in by_sequence.iter().zip(table.rows.iter()) {
        assert_eq!(delta.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(delta.sequence, row.sequence);
        assert_eq!(delta.flags, row.flags);
        assert_eq!(delta.ts_event.as_u64(), row.event_time as u64);
        // Assert the round-tripped BookAction for every row — including
        // UPDATE and DELETE — so a wrong action mapping cannot survive.
        let expected_action = if row.action == DeltaAction::Clear.as_str() {
            BookAction::Clear
        } else if row.action == DeltaAction::Add.as_str() {
            BookAction::Add
        } else if row.action == DeltaAction::Update.as_str() {
            BookAction::Update
        } else {
            // DeltaAction::Delete
            BookAction::Delete
        };
        assert_eq!(
            delta.action, expected_action,
            "sequence {}: action mismatch (source {:?})",
            row.sequence, row.action
        );
        if row.action != DeltaAction::Clear.as_str() {
            // Compare numerically: `Price`/`Quantity` Display renders at the
            // instrument precision (trailing zeros), so compare the
            // round-tripped value against the source parsed at the same scale.
            assert_eq!(
                delta.order.price.as_decimal(),
                Price::from(row.price.as_str()).as_decimal()
            );
            assert_eq!(
                delta.order.size.as_decimal(),
                Quantity::from(row.size.as_str()).as_decimal()
            );
            let expected_side = if row.side == DeltaSide::Buy.as_str() {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };
            assert_eq!(delta.order.side, expected_side);
        }
    }
}

#[test]
fn deltas_round_trip_through_binary_option_spec() {
    // The same synthetic prediction-market deltas must project through the
    // generic catalog seam when bound to a BinaryOption instrument, proving the
    // binary-option family reaches the catalog exactly like spot/perp/future.
    let table = snapshot_then_delta_table();
    let dir = tempfile::TempDir::new().expect("temp dir");

    let projection = project_canonical_order_book_deltas_to_catalog(
        &table,
        &binary_option_spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project via binary option spec");
    assert_eq!(projection.trade_count, table.rows.len());
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert!(!projection.catalog_hash.is_empty());

    // The catalog instrument is an NT BinaryOption (not a CurrencyPair).
    let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let instruments = catalog
        .query_instruments(Some(&[NT_INSTRUMENT_ID.to_string()]))
        .expect("query instruments");
    assert_eq!(instruments.len(), 1);
    assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));

    let mut loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), table.rows.len());
    loaded.sort_by_key(|delta| delta.sequence);
    for (delta, row) in loaded.iter().zip(table.rows.iter()) {
        assert_eq!(delta.instrument_id.to_string(), NT_INSTRUMENT_ID);
        assert_eq!(delta.sequence, row.sequence);
        assert_eq!(delta.flags, row.flags);
        assert_eq!(delta.ts_event.as_u64(), row.event_time as u64);
        if row.action == DeltaAction::Clear.as_str() {
            assert_eq!(delta.action, BookAction::Clear);
        } else {
            assert_eq!(
                delta.order.price.as_decimal(),
                Price::from(row.price.as_str()).as_decimal()
            );
            assert_eq!(
                delta.order.size.as_decimal(),
                Quantity::from(row.size.as_str()).as_decimal()
            );
            let expected_side = if row.side == DeltaSide::Buy.as_str() {
                OrderSide::Buy
            } else {
                OrderSide::Sell
            };
            assert_eq!(delta.order.side, expected_side);
        }
    }
}

#[test]
fn zero_size_delete_round_trips_through_nt_catalog() {
    // Finding #8: a level-removal DELETE carries size "0" (the real event-stream
    // output for a level removal). Prove it survives NT's OrderBookDelta
    // new_checked at projection and reads back faithfully as BookAction::Delete
    // with a zero size — the existing round-trip fixture deliberately used a
    // non-zero DELETE size, leaving this exact shape uncovered end to end.
    let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
    let last = RecordFlag::F_LAST as u8;
    let mbp = RecordFlag::F_MBP as u8;
    let table = table_with_rows(vec![
        delta_row(
            0,
            BASE_EVENT_TIME,
            DeltaAction::Clear,
            "",
            "",
            "",
            snapshot_flags,
        ),
        delta_row(
            1,
            BASE_EVENT_TIME,
            DeltaAction::Add,
            DeltaSide::Sell.as_str(),
            "0.51",
            "12",
            snapshot_flags | last,
        ),
        // Standalone zero-size DELETE removing the sell-side level.
        delta_row(
            2,
            BASE_EVENT_TIME + 1,
            DeltaAction::Delete,
            DeltaSide::Sell.as_str(),
            "0.51",
            "0",
            mbp | last,
        ),
    ]);
    // The canonical contract permits a zero-size DELETE (positive-size is only
    // required for ADD/UPDATE).
    table.validate().expect("zero-size DELETE table validates");

    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project");

    let mut loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    loaded.sort_by_key(|delta| delta.sequence);
    assert_eq!(loaded.len(), 3);

    let delete = &loaded[2];
    assert_eq!(
        delete.action,
        BookAction::Delete,
        "zero-size DELETE must round-trip as BookAction::Delete"
    );
    assert_eq!(delete.order.side, OrderSide::Sell);
    assert_eq!(
        delete.order.size.as_decimal(),
        Quantity::from("0").as_decimal(),
        "DELETE size must read back as zero"
    );
    assert_eq!(
        delete.order.price.as_decimal(),
        Price::from("0.51").as_decimal()
    );
    assert_ne!(
        delete.flags & RecordFlag::F_LAST as u8,
        0,
        "standalone DELETE closes its own event"
    );
}

#[test]
fn snapshot_expands_to_clear_then_adds_with_f_last() {
    let table = snapshot_then_delta_table();
    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project");
    let mut loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    loaded.sort_by_key(|delta| delta.sequence);

    // The first delta is the snapshot Clear; the next two are Adds.
    assert_eq!(loaded[0].action, BookAction::Clear);
    assert_eq!(loaded[1].action, BookAction::Add);
    assert_eq!(loaded[2].action, BookAction::Add);
    // The final Add of the snapshot expansion (sequence 2) carries F_LAST.
    assert_ne!(loaded[2].flags & RecordFlag::F_LAST as u8, 0);
    // The Clear does NOT carry F_LAST (the expansion continues past it).
    assert_eq!(loaded[0].flags & RecordFlag::F_LAST as u8, 0);
    // Sequences are dense and 0-based; ts_init is non-strict ascending.
    let mut prev_ts = u64::MIN;
    for (index, delta) in loaded.iter().enumerate() {
        assert_eq!(delta.sequence, index as u64);
        assert!(delta.ts_init.as_u64() >= prev_ts);
        prev_ts = delta.ts_init.as_u64();
    }
}

#[test]
fn empty_book_snapshot_projects_to_single_clear_with_f_last() {
    // A market-open empty-book snapshot is a lone Clear carrying
    // F_SNAPSHOT|F_MBP|F_LAST and zero Adds; it must round-trip through the
    // catalog (proving write_to_parquet accepts a first-and-only Clear delta).
    let flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8 | RecordFlag::F_LAST as u8;
    let table = table_with_rows(vec![delta_row(
        0,
        BASE_EVENT_TIME,
        DeltaAction::Clear,
        "",
        "",
        "",
        flags,
    )]);
    let dir = tempfile::TempDir::new().expect("temp dir");
    let projection = project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("project");
    assert_eq!(projection.trade_count, 1);

    let loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].action, BookAction::Clear);
    let read_flags = loaded[0].flags;
    assert_ne!(
        read_flags & RecordFlag::F_LAST as u8,
        0,
        "Clear closes the event"
    );
    assert_ne!(
        read_flags & RecordFlag::F_SNAPSHOT as u8,
        0,
        "Clear is snapshot data"
    );
    assert_ne!(read_flags & RecordFlag::F_MBP as u8, 0, "Clear is MBP data");
}

#[test]
fn projection_refuses_dirty_catalog_root() {
    let table = snapshot_then_delta_table();
    let dir = tempfile::TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
    let err = project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect_err("dirty catalog root must be refused");
    assert!(format!("{err:#}").contains("unexpected entry"), "{err:#}");
}

#[test]
fn delta_precision_widens_when_data_finer_than_tick() {
    // The spec tick is 0.01 / size 0.001, but the snapshot carries finer prints
    // (price scale 3, size scale 4). The projection must widen the instrument to
    // the data's actual scale instead of rejecting the accepted object.
    let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
    let last = RecordFlag::F_LAST as u8;
    let table = table_with_rows(vec![
        delta_row(
            0,
            BASE_EVENT_TIME,
            DeltaAction::Clear,
            "",
            "",
            "",
            snapshot_flags,
        ),
        delta_row(
            1,
            BASE_EVENT_TIME,
            DeltaAction::Add,
            DeltaSide::Buy.as_str(),
            "0.491",
            "10.0001",
            snapshot_flags | last,
        ),
    ]);
    let dir = tempfile::TempDir::new().expect("temp dir");
    project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir.path(),
        &test_catalog_encoding(),
    )
    .expect("projection widens precision instead of rejecting accepted data");

    let mut loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    loaded.sort_by_key(|delta| delta.sequence);
    // Read-back preserves the exact archived values at the widened precision.
    assert_eq!(
        loaded[1].order.price.as_decimal(),
        Price::from("0.491").as_decimal()
    );
    assert_eq!(
        loaded[1].order.size.as_decimal(),
        Quantity::from("10.0001").as_decimal()
    );
}

#[test]
fn delta_catalog_hash_is_stable() {
    let table = snapshot_then_delta_table();
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let a = project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir_a.path(),
        &test_catalog_encoding(),
    )
    .unwrap();
    let b = project_canonical_order_book_deltas_to_catalog(
        &table,
        &spec(),
        dir_b.path(),
        &test_catalog_encoding(),
    )
    .unwrap();
    assert_eq!(
        a.catalog_hash, b.catalog_hash,
        "same data must hash identically regardless of root"
    );
}

#[test]
fn delta_catalog_hash_changes_with_content() {
    let table_a = snapshot_then_delta_table();
    let mut table_b = snapshot_then_delta_table();
    // Change one Add's price; the catalog hash must change.
    table_b.rows[1].price = "0.42".to_string();
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let a = project_canonical_order_book_deltas_to_catalog(
        &table_a,
        &spec(),
        dir_a.path(),
        &test_catalog_encoding(),
    )
    .unwrap();
    let b = project_canonical_order_book_deltas_to_catalog(
        &table_b,
        &spec(),
        dir_b.path(),
        &test_catalog_encoding(),
    )
    .unwrap();
    assert_ne!(
        a.catalog_hash, b.catalog_hash,
        "different delta data must change the catalog hash"
    );
}
