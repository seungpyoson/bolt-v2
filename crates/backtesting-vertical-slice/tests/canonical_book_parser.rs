//! Parser proof for the canonical L2 order-book normalizer (gate 2, L2).
//!
//! Reads the committed hermetic Polymarket CLOB fixture (one `asset_id`,
//! shaped like the accepted streaming archive), decodes its rows into
//! [`RawClobEventRow`]s via the production Arrow reader, runs
//! [`normalize_polymarket_clob_book`], and asserts that the canonical event
//! counts match the source object's event mix and that a known full snapshot
//! decodes to the right levels. CI-safe: no network, no S3 — the fixture is the
//! only input.

use std::path::PathBuf;

use backtesting_vertical_slice::{
    canonical_book::{
        BookSide, CanonicalBookEvent, EVENT_TYPE_BOOK, EVENT_TYPE_LAST_TRADE_PRICE,
        EVENT_TYPE_PRICE_CHANGE, EVENT_TYPE_TICK_SIZE_CHANGE, RawClobEventRow,
        decode_polymarket_clob_parquet, normalize_polymarket_clob_book,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};

/// The single outcome token id in the committed fixture (test-only literal).
const FIXTURE_ASSET_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888";

/// SHA-256 of the committed accepted-schema fixture.
const FIXTURE_OBJECT_SHA256: &str =
    "852a6dabc415e0b73e5361db8b39d979291ee814ffa72fa8c287792979329ddc";

/// Event counts verified against the fixture with duckdb at build time.
const EXPECTED_BOOK_ROWS: usize = 1;
const EXPECTED_PRICE_CHANGE_ROWS: usize = 2;
const EXPECTED_TRADE_ROWS: usize = 2;
const EXPECTED_TICK_SIZE_CHANGE_ROWS: usize = 1;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/polymarket_clob_l2_slice.parquet")
}

fn accepted_dataset() -> AcceptedDataset {
    let checks = |evidence: &str| RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation://polymarket-archive"),
        schema: RequiredCheck::passed(
            "schema://timestamp_received,timestamp,event_type,asset_id,bids,asks",
        ),
        time_semantics: RequiredCheck::passed("timestamp_received_ms_to_unix_nanos"),
        instrument_universe: RequiredCheck::passed("universe://polymarket-outcomes"),
        coverage: RequiredCheck::passed(evidence),
        granularity: RequiredCheck::passed("full_depth_snapshot_plus_deltas"),
        completeness: RequiredCheck::passed(evidence),
        nt_mapping: RequiredCheck::passed("nt://OrderBookDelta"),
        storage: RequiredCheck::passed("s3://bolt-parquet/.../source-proofs/"),
    };
    let object = IngestManifestObjectRecord {
        s3_uri:
            "s3://bolt-parquet/backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/family=order_book_snapshots_fixed_depth/dt=2026-05-22/object=b32d8d.parquet"
                .to_string(),
        source_url: "https://polymarket-archive.example/clob/2026-05-22.parquet".to_string(),
        sha256: FIXTURE_OBJECT_SHA256.to_string(),
        bytes: 5379,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec![
            "timestamp_received".to_string(),
            "timestamp".to_string(),
            "market".to_string(),
            "event_type".to_string(),
            "asset_id".to_string(),
            "bids".to_string(),
            "asks".to_string(),
            "price".to_string(),
            "size".to_string(),
            "side".to_string(),
            "best_bid".to_string(),
            "best_ask".to_string(),
            "fee_rate_bps".to_string(),
            "transaction_hash".to_string(),
            "old_tick_size".to_string(),
            "new_tick_size".to_string(),
        ],
    };
    let proof = SourceProofReport {
        source_proof_id: "source-proof-polymarket-clob-l2".to_string(),
        source_proof_version: 1,
        contract_version: "backfill-table-contract.v1".to_string(),
        schema_version: "backfill-source-proof.v1".to_string(),
        status: SourceProofStatus::Pending,
        source_binding: "polymarket-parquet-archive-index".to_string(),
        venue: "polymarket".to_string(),
        product_family: "prediction-market".to_string(),
        product_category: "binary-outcome".to_string(),
        table_family: "order_book".to_string(),
        evidence_state: EvidenceState::OwnerArchiveBackfillable,
        fixture_type: FixtureType::PredictionMarket,
        requested_time_range: TimeRange {
            start_utc: "2026-05-01T00:00:00Z".to_string(),
            end_utc: "2026-06-01T00:00:00Z".to_string(),
        },
        coverage_time_range: TimeRange {
            start_utc: "2026-05-22T00:00:00Z".to_string(),
            end_utc: "2026-05-23T00:00:00Z".to_string(),
        },
        instrument_universe_id: "polymarket-outcomes-2026-05-22".to_string(),
        raw_sample_uri: "s3://bolt-parquet/.../object=b32d8d.parquet".to_string(),
        raw_sample_hash: FIXTURE_OBJECT_SHA256.to_string(),
        schema_sample_uri: "s3://bolt-parquet/.../schema.json".to_string(),
        schema_sample_hash: "bf26db".to_string(),
        license_ref: "https://polymarket-archive.example/ (attestation)".to_string(),
        retention_ref: "https://polymarket-archive.example/".to_string(),
        nt_mapping_status: NtMappingStatus::Accepted,
        fidelity_class: SourceProofFidelityClass::L2Replay,
        forbidden_claims: vec!["No fill claims beyond replayed top-of-book liquidity.".to_string()],
        gap_policy_id: String::new(),
        required_checks: checks("manifest://polymarket-clob-2026-05-22"),
        acceptance_mode: None,
        accepted_by: None,
        accepted_at: None,
        supersedes_source_proof_id: None,
    }
    .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
    .expect("accept proof");
    select_accepted_dataset(&proof, &object, FIXTURE_OBJECT_SHA256)
        .expect("select accepted dataset")
}

/// Decode the committed fixture Parquet into raw CLOB event rows.
fn read_fixture_rows() -> Vec<RawClobEventRow> {
    decode_polymarket_clob_parquet(&fixture_path()).expect("decode accepted-schema fixture")
}

#[test]
fn fixture_event_counts_match_source() {
    let raw_rows = read_fixture_rows();
    // The fixture is single-asset, so the raw count equals 1 + 2 + 2 + 1.
    assert_eq!(
        raw_rows.len(),
        EXPECTED_BOOK_ROWS
            + EXPECTED_PRICE_CHANGE_ROWS
            + EXPECTED_TRADE_ROWS
            + EXPECTED_TICK_SIZE_CHANGE_ROWS
    );
    let raw_books = raw_rows
        .iter()
        .filter(|r| r.event_type == EVENT_TYPE_BOOK)
        .count();
    let raw_price_changes = raw_rows
        .iter()
        .filter(|r| r.event_type == EVENT_TYPE_PRICE_CHANGE)
        .count();
    let raw_trades = raw_rows
        .iter()
        .filter(|r| r.event_type == EVENT_TYPE_LAST_TRADE_PRICE)
        .count();
    let raw_tick_size_changes = raw_rows
        .iter()
        .filter(|r| r.event_type == EVENT_TYPE_TICK_SIZE_CHANGE)
        .count();
    assert_eq!(raw_books, EXPECTED_BOOK_ROWS);
    assert_eq!(raw_price_changes, EXPECTED_PRICE_CHANGE_ROWS);
    assert_eq!(raw_trades, EXPECTED_TRADE_ROWS);
    assert_eq!(raw_tick_size_changes, EXPECTED_TICK_SIZE_CHANGE_ROWS);

    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &raw_rows,
        raw_rows[0].timestamp_received,
        "ingest-run-fixture",
    )
    .expect("normalize fixture");

    // Canonical counts match the duckdb-verified source mix; tick_size_change is
    // an accepted no-op and must not emit a book/trade row.
    assert_eq!(
        table.rows.len(),
        EXPECTED_BOOK_ROWS + EXPECTED_PRICE_CHANGE_ROWS + EXPECTED_TRADE_ROWS
    );
    assert_eq!(table.snapshot_count(), EXPECTED_BOOK_ROWS);
    assert_eq!(table.level_change_count(), EXPECTED_PRICE_CHANGE_ROWS);
    assert_eq!(table.trade_count(), EXPECTED_TRADE_ROWS);
    assert_eq!(table.instrument_id, FIXTURE_ASSET_ID);
    assert_eq!(table.fidelity_class, SourceProofFidelityClass::L2Replay);

    // Dense, 0-based, monotonic-nondecreasing capture time (validate() enforced it,
    // re-check the boundary here as a self-evident anchor).
    let mut prev = i64::MIN;
    for (i, row) in table.rows.iter().enumerate() {
        assert_eq!(row.source_sequence, i as u64);
        assert!(row.capture_time >= prev);
        assert_eq!(row.event_time, row.capture_time);
        prev = row.capture_time;
    }
}

#[test]
fn known_snapshot_decodes_to_expected_levels() {
    let raw_rows = read_fixture_rows();
    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &raw_rows,
        raw_rows[0].timestamp_received,
        "ingest-run-fixture",
    )
    .expect("normalize fixture");

    let snapshot = table
        .rows
        .iter()
        .find_map(|r| match &r.event {
            CanonicalBookEvent::Snapshot(s) => Some(s),
            _ => None,
        })
        .expect("fixture has one book snapshot");

    // Verified with duckdb json_array_length: 2 bid levels, 2 ask levels.
    assert_eq!(snapshot.bids.len(), 2);
    assert_eq!(snapshot.asks.len(), 2);
    // First bid/ask levels are preserved exactly as the source JSON strings.
    assert_eq!(snapshot.bids[0].price, "0.5000");
    assert_eq!(snapshot.bids[0].size, "10.000000");
    assert_eq!(snapshot.asks[0].price, "0.5100");
    assert_eq!(snapshot.asks[0].size, "8.000000");
}

#[test]
fn first_trade_decodes_with_side_and_tx_hash() {
    let raw_rows = read_fixture_rows();
    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &raw_rows,
        raw_rows[0].timestamp_received,
        "ingest-run-fixture",
    )
    .expect("normalize fixture");

    let trade = table
        .rows
        .iter()
        .find_map(|r| match &r.event {
            CanonicalBookEvent::Trade(t) => Some(t),
            _ => None,
        })
        .expect("fixture has trade prints");
    assert_eq!(trade.side, BookSide::Buy);
    assert_eq!(trade.price, "0.5100");
    assert!(!trade.transaction_hash.is_empty());
    assert!(trade.transaction_hash.starts_with("0x"));
}

#[test]
fn normalize_empty_book_snapshot_at_sequence_0_is_valid() {
    // A genuine market-open empty-book CLOB snapshot (bids="[]" AND asks="[]") is
    // the FIRST event for its asset; both sides are literal empty JSON arrays, not
    // null. The over-strict decode + validate guards rejected it as
    // "sequence 0: failed to decode CLOB event -> book row: snapshot has no
    // levels", aborting the whole object (the RUN2 polymarket failure). A
    // zero-level snapshot is a valid empty-book state and must normalize. This
    // exercises BOTH guards: normalize calls decode_event AND table.validate().
    let rows = vec![
        RawClobEventRow {
            asset_id: FIXTURE_ASSET_ID.to_string(),
            event_type: EVENT_TYPE_BOOK.to_string(),
            timestamp_received: 1_700_000_000_000_000_000,
            event_time: 1_700_000_000_000_000_000,
            source_row_index: 0,
            bids: "[]".to_string(),
            asks: "[]".to_string(),
            price: String::new(),
            size: String::new(),
            side: String::new(),
            transaction_hash: String::new(),
        },
        RawClobEventRow {
            asset_id: FIXTURE_ASSET_ID.to_string(),
            event_type: EVENT_TYPE_PRICE_CHANGE.to_string(),
            timestamp_received: 1_700_000_000_000_000_001,
            event_time: 1_700_000_000_000_000_001,
            source_row_index: 1,
            bids: String::new(),
            asks: String::new(),
            price: "0.5000".to_string(),
            size: "10.000000".to_string(),
            side: "buy".to_string(),
            transaction_hash: String::new(),
        },
    ];
    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &rows,
        rows[0].timestamp_received,
        "ingest-run-empty",
    )
    .expect("empty-book snapshot at sequence 0 must normalize");

    assert_eq!(table.snapshot_count(), 1);
    let snapshot = table
        .rows
        .iter()
        .find_map(|r| match &r.event {
            CanonicalBookEvent::Snapshot(s) => Some(s),
            _ => None,
        })
        .expect("one empty book snapshot");
    assert!(snapshot.bids.is_empty(), "market-open snapshot has no bids");
    assert!(snapshot.asks.is_empty(), "market-open snapshot has no asks");
}
