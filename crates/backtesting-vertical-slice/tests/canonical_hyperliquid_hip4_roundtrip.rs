//! Round-trip proof for the Hyperliquid HIP-4 fixed-depth snapshot converter.
//!
//! Parses a hermetic fixture downsampled from the real staged object
//! `s3://bolt-parquet/backfill-staging/2026-06-01/hyperliquid-hip4/staged/v1/`
//! `table=order_book_snapshots_fixed_depth/run=run-20260601T162956Z-9a92f0c96a37/`
//! `part-000000.jsonl` (source object sha256
//! d399e32ce2dd7680c3641a63ed36292045dd0db9dd92adf56a97033c8b5daea4; the kept
//! subset is committed verbatim under tests/fixtures/hyperliquid-hip4/), builds
//! NautilusTrader `OrderBookDelta`s, writes them into a temporary NautilusTrader
//! `ParquetDataCatalog`, queries them back, and asserts the round-tripped
//! count + ordering + payloads match. That proves HIP-4 snapshot data lands in an
//! NT catalog NautilusTrader can replay.

use std::collections::BTreeSet;

use backtesting_vertical_slice::canonical_hyperliquid_hip4::{
    Hip4InstrumentNaming, NT_DATA_TYPE_ORDER_BOOK_DELTA, parse_hip4_snapshots,
    project_hip4_snapshots_to_catalog, read_back_order_book_deltas, snapshots_to_deltas,
};
use nautilus_model::enums::{BookAction, RecordFlag};

/// The committed hermetic fixture (downsampled real HIP-4 snapshots).
const FIXTURE: &str =
    include_str!("fixtures/hyperliquid-hip4/order_book_snapshots_fixed_depth.jsonl");

/// Venue/identifier mapping for the fixture. Built here (the test owns the sample
/// ids); the converter takes naming as input so no instrument id is hardcoded in
/// the library.
fn naming() -> Hip4InstrumentNaming {
    Hip4InstrumentNaming {
        nt_venue_code: "HYPERLIQUID".to_string(),
        outcome_symbol_prefix: "OUTCOME-".to_string(),
        expected_venue: "hyperliquid".to_string(),
    }
}

#[test]
fn hip4_snapshot_fixture_round_trips_through_nt_catalog() {
    let table = parse_hip4_snapshots(FIXTURE, &naming()).expect("parse fixture");

    // The fixture holds 5 staged records across 4 distinct outcomes; outcome 101
    // appears twice (two book photos at different snapshot times) and outcome 100
    // is an empty book.
    assert_eq!(
        table.instruments.len(),
        4,
        "four distinct outcome instruments"
    );

    // Instruments are deterministically ordered by NautilusTrader instrument id.
    let ids: Vec<&str> = table
        .instruments
        .iter()
        .map(|inst| inst.nt_instrument_id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec![
            "OUTCOME-100.HYPERLIQUID",
            "OUTCOME-101.HYPERLIQUID",
            "OUTCOME-103.HYPERLIQUID",
            "OUTCOME-138.HYPERLIQUID",
        ],
    );

    // Per-instrument delta accounting: one Clear per photo + one Add per level.
    // outcome 100: 1 empty photo                 -> 1 delta
    // outcome 101: 2 photos, 52 levels total     -> 54 deltas
    // outcome 103: 1 photo, 19 levels            -> 20 deltas
    // outcome 138: 1 photo, 27 levels            -> 28 deltas
    let expected_deltas = [
        ("OUTCOME-100.HYPERLIQUID", 1usize),
        ("OUTCOME-101.HYPERLIQUID", 54),
        ("OUTCOME-103.HYPERLIQUID", 20),
        ("OUTCOME-138.HYPERLIQUID", 28),
    ];

    // Within-instrument snapshot times must be strictly non-descending (the
    // catalog writer rejects descending timestamps).
    for inst in &table.instruments {
        let mut last = None;
        for snapshot in &inst.snapshots {
            if let Some(prev) = last {
                assert!(
                    snapshot.ts_event >= prev,
                    "{}: snapshots must be ascending by time",
                    inst.nt_instrument_id
                );
            }
            last = Some(snapshot.ts_event);
        }
    }

    let dir = tempfile::TempDir::new().expect("temp catalog root");
    let projection = project_hip4_snapshots_to_catalog(&table, dir.path()).expect("project");

    assert_eq!(projection.data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
    assert_eq!(
        projection.total_delta_count, 103,
        "5 photos (5 clears) + 98 levels = 103 deltas"
    );
    assert_eq!(projection.instruments.len(), 4);

    // Read every instrument back from the NautilusTrader catalog and assert the
    // round-tripped deltas match the in-memory deltas exactly (count + ordering +
    // payload), proving NautilusTrader can replay what we wrote.
    for (id, expected_count) in expected_deltas {
        let inst = table
            .instruments
            .iter()
            .find(|inst| inst.nt_instrument_id == id)
            .expect("instrument present in table");
        let built = snapshots_to_deltas(inst).expect("build deltas");
        assert_eq!(built.len(), expected_count, "{id}: built delta count");

        let loaded = read_back_order_book_deltas(dir.path(), id).expect("read back");
        assert_eq!(loaded.len(), expected_count, "{id}: round-tripped count");
        assert_eq!(
            loaded, built,
            "{id}: round-tripped deltas identical to built deltas (ordering + payload)"
        );

        // Every photo begins with a Clear.
        assert_eq!(
            loaded[0].action,
            BookAction::Clear,
            "{id}: first delta is a Clear"
        );
        // Every Add carries the snapshot flag; the final delta of each photo
        // carries F_LAST.
        assert!(
            loaded
                .iter()
                .all(|d| RecordFlag::F_SNAPSHOT.matches(d.flags)),
            "{id}: every delta carries F_SNAPSHOT"
        );
        assert!(
            RecordFlag::F_LAST.matches(loaded.last().unwrap().flags),
            "{id}: final delta carries F_LAST"
        );
    }

    // Outcome 100 is the empty book: a single Clear with F_LAST, no Add.
    let empty = read_back_order_book_deltas(dir.path(), "OUTCOME-100.HYPERLIQUID")
        .expect("read empty outcome");
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].action, BookAction::Clear);
    assert!(RecordFlag::F_LAST.matches(empty[0].flags));

    // Outcome 101 carries two photos; assert both Clears survive and the two
    // photos sit at two distinct snapshot times in ascending order.
    let multi = read_back_order_book_deltas(dir.path(), "OUTCOME-101.HYPERLIQUID")
        .expect("read multi-photo outcome");
    let clears: Vec<_> = multi
        .iter()
        .filter(|d| d.action == BookAction::Clear)
        .collect();
    assert_eq!(clears.len(), 2, "two photos -> two clears");
    let times: BTreeSet<u64> = multi.iter().map(|d| d.ts_event.as_u64()).collect();
    assert_eq!(times.len(), 2, "two distinct snapshot times");
    let mut prev = 0u64;
    for d in &multi {
        assert!(
            d.ts_event.as_u64() >= prev,
            "deltas are emitted in ascending time order"
        );
        prev = d.ts_event.as_u64();
    }
}
