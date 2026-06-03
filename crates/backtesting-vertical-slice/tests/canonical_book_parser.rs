//! Parser proof for the canonical L2 order-book normalizer (gate 2, L2).
//!
//! Reads the committed hermetic Polymarket CLOB fixture (one `asset_id`,
//! downsampled from a real archive object), decodes its rows into
//! [`RawClobEventRow`]s via the NautilusTrader-independent Arrow reader, runs
//! [`normalize_polymarket_clob_book`], and asserts that the canonical event
//! counts match the source object's event mix and that a known full snapshot
//! decodes to the right levels. CI-safe: no network, no S3 — the fixture is the
//! only input.

use std::{fs::File, path::PathBuf};

use arrow::array::{Array, Decimal128Array, StringArray, TimestampMicrosecondArray};
use backtesting_vertical_slice::{
    canonical_book::{
        BookSide, CanonicalBookEvent, EVENT_TYPE_BOOK, EVENT_TYPE_LAST_TRADE_PRICE,
        EVENT_TYPE_PRICE_CHANGE, RawClobEventRow, normalize_polymarket_clob_book,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rust_decimal::Decimal;

/// The single outcome token id in the committed fixture (test-only literal).
const FIXTURE_ASSET_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888";

/// SHA-256 of the real archive object the fixture was downsampled from.
const FIXTURE_OBJECT_SHA256: &str =
    "b32d8dc1944c550191f62a79fc0a9bec25fa0c498705801998a4fd3adf279f19";

/// Event counts verified against the fixture with duckdb at build time.
const EXPECTED_BOOK_ROWS: usize = 1;
const EXPECTED_PRICE_CHANGE_ROWS: usize = 66;
const EXPECTED_TRADE_ROWS: usize = 2;

const NANOS_PER_MICROSECOND: i64 = 1_000;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/polymarket_clob_l2_slice.parquet")
}

fn accepted_dataset() -> AcceptedDataset {
    let checks = |evidence: &str| RequiredChecks {
        source_access: RequiredCheck::passed(evidence),
        license: RequiredCheck::passed("attestation://polymarket-archive"),
        schema: RequiredCheck::passed("schema://timestamp,event_type,asset_id,bids,asks"),
        time_semantics: RequiredCheck::passed("utc_micros_to_unix_nanos"),
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
        bytes: 9113,
        archive_date: "2026-05-22".to_string(),
        schema_columns: vec![
            "timestamp".to_string(),
            "event_type".to_string(),
            "asset_id".to_string(),
            "bids".to_string(),
            "asks".to_string(),
            "price".to_string(),
            "size".to_string(),
            "side".to_string(),
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

/// Format a `Decimal128` cell to a decimal string at the column's scale, or an
/// empty string when the cell is null (the column is null for `book` rows).
fn decimal_cell(array: &Decimal128Array, index: usize, scale: i8) -> String {
    if array.is_null(index) {
        return String::new();
    }
    let raw = array.value(index);
    Decimal::from_i128_with_scale(raw, u32::try_from(scale).expect("non-negative scale"))
        .to_string()
}

/// Read a UTF8 cell, mapping null to an empty string.
fn string_cell(array: &StringArray, index: usize) -> String {
    if array.is_null(index) {
        String::new()
    } else {
        array.value(index).to_string()
    }
}

/// Decode the committed fixture Parquet into raw CLOB event rows, ordered as
/// stored (already ordered by event time at fixture-build time).
fn read_fixture_rows() -> Vec<RawClobEventRow> {
    let file = File::open(fixture_path()).expect("open fixture parquet");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .expect("parquet reader builder")
        .build()
        .expect("parquet reader");

    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.expect("read record batch");
        let col = |name: &str| batch.column_by_name(name).expect("column present");

        let timestamp = col("timestamp")
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .expect("timestamp is micros");
        let asset_id = col("asset_id")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("asset_id utf8");
        let event_type = col("event_type")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("event_type utf8");
        let bids = col("bids")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("bids utf8");
        let asks = col("asks")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("asks utf8");
        let side = col("side")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("side utf8");
        let transaction_hash = col("transaction_hash")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("transaction_hash utf8");
        let price = col("price")
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .expect("price decimal");
        let size = col("size")
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .expect("size decimal");
        let price_scale = price.scale();
        let size_scale = size.scale();

        for i in 0..batch.num_rows() {
            rows.push(RawClobEventRow {
                asset_id: string_cell(asset_id, i),
                event_type: string_cell(event_type, i),
                event_time: timestamp.value(i) * NANOS_PER_MICROSECOND,
                bids: string_cell(bids, i),
                asks: string_cell(asks, i),
                price: decimal_cell(price, i, price_scale),
                size: decimal_cell(size, i, size_scale),
                side: string_cell(side, i),
                transaction_hash: string_cell(transaction_hash, i),
            });
        }
    }
    rows
}

#[test]
fn fixture_event_counts_match_source() {
    let raw_rows = read_fixture_rows();
    // The fixture is single-asset, so the raw count equals 1 + 66 + 2.
    assert_eq!(
        raw_rows.len(),
        EXPECTED_BOOK_ROWS + EXPECTED_PRICE_CHANGE_ROWS + EXPECTED_TRADE_ROWS
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
    assert_eq!(raw_books, EXPECTED_BOOK_ROWS);
    assert_eq!(raw_price_changes, EXPECTED_PRICE_CHANGE_ROWS);
    assert_eq!(raw_trades, EXPECTED_TRADE_ROWS);

    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &raw_rows,
        7,
        "ingest-run-fixture",
    )
    .expect("normalize fixture");

    // Canonical counts match the duckdb-verified source mix.
    assert_eq!(table.rows.len(), raw_rows.len());
    assert_eq!(table.snapshot_count(), EXPECTED_BOOK_ROWS);
    assert_eq!(table.level_change_count(), EXPECTED_PRICE_CHANGE_ROWS);
    assert_eq!(table.trade_count(), EXPECTED_TRADE_ROWS);
    assert_eq!(table.instrument_id, FIXTURE_ASSET_ID);
    assert_eq!(table.fidelity_class, SourceProofFidelityClass::L2Replay);

    // Dense, 0-based, monotonic-nondecreasing event time (validate() enforced it,
    // re-check the boundary here as a self-evident anchor).
    let mut prev = i64::MIN;
    for (i, row) in table.rows.iter().enumerate() {
        assert_eq!(row.source_sequence, i as u64);
        assert!(row.event_time >= prev);
        prev = row.event_time;
    }
}

#[test]
fn known_snapshot_decodes_to_expected_levels() {
    let raw_rows = read_fixture_rows();
    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &raw_rows,
        0,
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

    // Verified with duckdb json_array_length: 45 bid levels, 46 ask levels.
    assert_eq!(snapshot.bids.len(), 45);
    assert_eq!(snapshot.asks.len(), 46);
    // First bid/ask levels are preserved exactly as the source JSON strings.
    assert_eq!(snapshot.bids[0].price, "0.01");
    assert_eq!(snapshot.bids[0].size, "14926.03");
    assert_eq!(snapshot.asks[0].price, "0.99");
    assert_eq!(snapshot.asks[0].size, "14560.03");
}

#[test]
fn first_trade_decodes_with_side_and_tx_hash() {
    let raw_rows = read_fixture_rows();
    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &raw_rows,
        0,
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
    assert!(trade.price.starts_with("0.51"));
    assert!(!trade.transaction_hash.is_empty());
    assert!(trade.transaction_hash.starts_with("0x"));
}
