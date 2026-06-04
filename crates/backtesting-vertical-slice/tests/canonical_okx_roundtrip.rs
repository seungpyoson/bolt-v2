//! OKX order-book converter round-trip proof (venue slice of spec 023
//! `1-backtesting-engine`).
//!
//! Proves the venue's full-L2 archive is replayable from a NautilusTrader
//! catalog: parse the committed hermetic fixture -> build NautilusTrader
//! `OrderBookDelta`s -> `ParquetDataCatalog::write_to_parquet` into a temp
//! catalog -> `query_typed_data::<OrderBookDelta>` back -> assert the
//! round-tripped count, payloads, and ordering match.
//!
//! The fixture is a tiny downsampled slice of the smallest real OKX
//! `order_book_400` object, re-wrapped in the same gzip-of-ustar-tar envelope
//! the S3 archive uses, so the test exercises the full real extraction pipeline
//! (gunzip -> tar -> JSONL) without touching S3.

use std::fs;

use backtesting_vertical_slice::canonical_okx::{
    NT_DATA_TYPE_ORDER_BOOK_DELTA, OkxBookSpec, append_okx_book_archive,
    extract_jsonl_from_archive, okx_book_instruments, okx_book_messages_to_deltas,
    okx_book_spec_for_instrument, parse_okx_book_messages, project_okx_book_archive_to_catalog,
    read_back_order_book_deltas,
};
use nautilus_model::enums::BookAction;
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

/// Path to the committed hermetic fixture (real OKX `order_book_400` data,
/// downsampled).
const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/okx/okx_orderbook_400_BNB-USD_UM_XPERP.tar.gz"
);

/// The fixture's single instrument id, as it appears in the archive `instId`.
const VENUE_INST_ID: &str = "BNB-USD_UM_XPERP-310523";
/// The NautilusTrader instrument id used for the catalog projection.
const NT_INSTRUMENT_ID: &str = "BNB-USD_UM_XPERP-310523.OKX";

fn spec() -> OkxBookSpec {
    OkxBookSpec {
        nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        venue_inst_id: VENUE_INST_ID.to_string(),
    }
}

fn fixture_bytes() -> Vec<u8> {
    fs::read(FIXTURE).expect("read OKX order-book fixture")
}

#[test]
fn fixture_extracts_and_maps_to_expected_delta_shape() {
    let gz = fixture_bytes();
    let jsonl = extract_jsonl_from_archive(&gz).expect("extract JSONL from archive");
    let messages = parse_okx_book_messages(&jsonl, VENUE_INST_ID).expect("parse messages");
    // Fixture slice: 24 update messages + 1 populated snapshot message.
    assert_eq!(messages.len(), 25, "fixture message count");

    let deltas = okx_book_messages_to_deltas(&messages, &spec()).expect("map to deltas");

    // Derived deterministically from the fixture content:
    //   1 Clear (the lone snapshot) + 8 Add (snapshot levels) + 41 Update + 43 Delete.
    assert_eq!(deltas.len(), 93, "total delta count");
    let clears = deltas
        .iter()
        .filter(|d| d.action == BookAction::Clear)
        .count();
    let adds = deltas
        .iter()
        .filter(|d| d.action == BookAction::Add)
        .count();
    let updates = deltas
        .iter()
        .filter(|d| d.action == BookAction::Update)
        .count();
    let deletes = deltas
        .iter()
        .filter(|d| d.action == BookAction::Delete)
        .count();
    assert_eq!((clears, adds, updates, deletes), (1, 8, 41, 43));

    // Every delta is fenced to the one instrument and carries the L2 sentinel id.
    assert!(
        deltas
            .iter()
            .all(|d| d.instrument_id.to_string() == NT_INSTRUMENT_ID)
    );
    assert!(deltas.iter().all(|d| d.order.order_id == 0));

    // Timestamps are non-decreasing (the NautilusTrader write contract).
    assert!(
        deltas
            .windows(2)
            .all(|w| w[0].ts_init.as_u64() <= w[1].ts_init.as_u64())
    );
}

#[test]
fn okx_book_round_trips_through_nautilus_catalog() {
    let gz = fixture_bytes();
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Build the expected deltas independently so the round-trip compares against
    // a value derived from the same source but not from the catalog.
    let jsonl = extract_jsonl_from_archive(&gz).expect("extract JSONL");
    let messages = parse_okx_book_messages(&jsonl, VENUE_INST_ID).expect("parse messages");
    let expected = okx_book_messages_to_deltas(&messages, &spec()).expect("map to deltas");

    // Project the archive into a real NautilusTrader ParquetDataCatalog.
    let projection =
        project_okx_book_archive_to_catalog(&gz, &spec(), dir.path()).expect("project to catalog");
    assert_eq!(projection.delta_count, expected.len());
    assert_eq!(projection.data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
    assert_eq!(projection.nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(projection.price_precision, 1);
    assert_eq!(projection.size_precision, 0);
    assert!(!projection.catalog_hash.is_empty());

    // Read the deltas back with NautilusTrader's own typed query.
    let loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");

    // Count matches.
    assert_eq!(
        loaded.len(),
        expected.len(),
        "round-tripped delta count must match"
    );
    // Ordering + payloads survive the parquet round-trip exactly.
    assert_eq!(
        loaded, expected,
        "round-tripped deltas must be identical (count, ordering, and payload)"
    );

    // Spot-check that the catalog wrote to NautilusTrader's native
    // `order_book_deltas` tree.
    let has_delta_tree = walk(dir.path()).iter().any(|p| {
        p.to_string_lossy().contains("order_book_deltas")
            && p.extension().map(|e| e == "parquet").unwrap_or(false)
    });
    assert!(
        has_delta_tree,
        "catalog must contain a native order_book_deltas parquet file"
    );
}

#[test]
fn okx_book_data_derived_append_round_trips() {
    // The bulk path: enumerate the archive's instruments, derive precision from
    // each instrument's own levels (via resolve_precisions inside the converter),
    // append into a shared catalog with no clean-root guard, and prove the
    // NautilusTrader round-trip is lossless.
    let gz = fixture_bytes();
    let dir = tempfile::TempDir::new().expect("temp catalog root");

    // Independent expectation from the same source, via the data-derived spec.
    let jsonl = extract_jsonl_from_archive(&gz).expect("extract JSONL");
    assert_eq!(
        okx_book_instruments(&jsonl).expect("enumerate instruments"),
        vec![VENUE_INST_ID.to_string()],
        "fixture carries exactly one venue instrument"
    );
    let derived = okx_book_spec_for_instrument(VENUE_INST_ID);
    assert_eq!(derived.nt_instrument_id, NT_INSTRUMENT_ID);
    let messages = parse_okx_book_messages(&jsonl, VENUE_INST_ID).expect("parse messages");
    let expected = okx_book_messages_to_deltas(&messages, &derived).expect("map to deltas");

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries = append_okx_book_archive(&gz, &mut catalog).expect("append deltas");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].nt_instrument_id, NT_INSTRUMENT_ID);
    assert_eq!(summaries[0].record_count, expected.len());
    // Precision is read from the levels; BNB perp prints a 1-dp price and an
    // integer-contract size, and the summary matches the level-bearing deltas.
    assert_eq!(summaries[0].price_precision, 1);
    assert_eq!(summaries[0].size_precision, 0);
    let level_delta = expected
        .iter()
        .find(|d| d.action != BookAction::Clear)
        .expect("at least one level-bearing delta");
    assert_eq!(
        summaries[0].price_precision,
        level_delta.order.price.precision
    );
    assert_eq!(
        summaries[0].size_precision,
        level_delta.order.size.precision
    );

    let loaded = read_back_order_book_deltas(dir.path(), NT_INSTRUMENT_ID).expect("read back");
    assert_eq!(loaded.len(), expected.len(), "round-tripped delta count");
    assert!(
        loaded
            .windows(2)
            .all(|w| w[0].ts_init.as_u64() <= w[1].ts_init.as_u64()),
        "loaded deltas must be ascending"
    );
    assert_eq!(
        loaded, expected,
        "data-derived append must round-trip identically (count, ordering, payload, precision)"
    );
}

#[test]
fn projection_refuses_dirty_catalog_root() {
    let gz = fixture_bytes();
    let dir = tempfile::TempDir::new().expect("temp catalog root");
    fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
    let err = project_okx_book_archive_to_catalog(&gz, &spec(), dir.path())
        .expect_err("dirty catalog root must be refused");
    assert!(err.to_string().contains("not empty"), "{err}");
}

#[test]
fn catalog_hash_is_deterministic_across_roots() {
    let gz = fixture_bytes();
    let dir_a = tempfile::TempDir::new().unwrap();
    let dir_b = tempfile::TempDir::new().unwrap();
    let a = project_okx_book_archive_to_catalog(&gz, &spec(), dir_a.path()).unwrap();
    let b = project_okx_book_archive_to_catalog(&gz, &spec(), dir_b.path()).unwrap();
    assert_eq!(
        a.catalog_hash, b.catalog_hash,
        "same data must hash identically regardless of root"
    );
}

/// Recursively collect every file under `root`.
fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}
