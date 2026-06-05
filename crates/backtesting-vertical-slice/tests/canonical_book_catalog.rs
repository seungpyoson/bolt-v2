//! NautilusTrader catalog read-back proof for the L2 order-book projection
//! (gate 3, L2).
//!
//! Reads the committed hermetic Polymarket CLOB fixture (one `asset_id`),
//! normalizes it via [`normalize_polymarket_clob_book`], projects the canonical
//! L2 book table into a NautilusTrader `ParquetDataCatalog` as both
//! `OrderBookDelta` (snapshot expansion + single-level deltas) and `TradeTick`
//! (trade prints), then proves the resolved NautilusTrader dependency reads
//! BOTH data types back from the same catalog. CI-safe: no network, no S3 — the
//! committed fixture is the only input.

use std::{fs::File, path::PathBuf, sync::Arc};

use arrow::{
    array::{ArrayRef, Decimal128Array, StringArray, TimestampMillisecondArray},
    datatypes::{DataType, Field, Schema, TimeUnit},
    record_batch::RecordBatch,
};

use backtesting_vertical_slice::{
    canonical_book::{
        CanonicalBookTable, POLYMARKET_VENUE, RawClobEventRow, append_polymarket_book_archive,
        append_polymarket_trades_archive, decode_polymarket_clob_parquet,
        decode_polymarket_clob_parquet_for_asset, normalize_polymarket_clob_book,
        polymarket_book_spec_from_table, polymarket_clob_assets_from_parquet,
    },
    catalog_projection::{
        BinaryOptionInstrumentSpec, NT_DATA_TYPE_ORDER_BOOK_DELTA, build_binary_option,
        canonical_rows_to_order_book_deltas, project_canonical_book_to_catalog,
        read_back_order_book_deltas, read_back_trade_ticks,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};
use nautilus_model::{
    enums::{AggressorSide, BookAction, OrderSide, RecordFlag},
    identifiers::InstrumentId,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use parquet::arrow::arrow_writer::ArrowWriter;
use tempfile::TempDir;

/// The single outcome token id in the committed fixture (test-only literal).
const FIXTURE_ASSET_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888";

/// SHA-256 of the committed accepted-schema fixture.
const FIXTURE_OBJECT_SHA256: &str =
    "852a6dabc415e0b73e5361db8b39d979291ee814ffa72fa8c287792979329ddc";

/// NautilusTrader instrument id for the fixture outcome on the Polymarket venue.
const FIXTURE_NT_INSTRUMENT_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888.POLYMARKET";

// Event counts verified against the fixture with duckdb at build time. The
// snapshot expands to one `Clear` plus one `Add` per level (2 bid + 2 ask
// levels = 4 adds), and each `price_change` maps to one delta.
const EXPECTED_SNAPSHOT_BID_LEVELS: usize = 2;
const EXPECTED_SNAPSHOT_ASK_LEVELS: usize = 2;
const EXPECTED_PRICE_CHANGE_ROWS: usize = 2;
const EXPECTED_TRADE_ROWS: usize = 2;
const EXPECTED_TICK_SIZE_CHANGE_ROWS: usize = 1;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/polymarket_clob_l2_slice.parquet")
}

struct TinyClobRow<'a> {
    asset_id: &'a str,
    event_type: &'a str,
    timestamp_ms: i64,
    price_scaled_4: Option<i128>,
    size_scaled_6: Option<i128>,
    side: &'a str,
    transaction_hash: &'a str,
}

fn tiny_price_change<'a>(
    asset_id: &'a str,
    timestamp_ms: i64,
    price_scaled_4: i128,
    size_scaled_6: i128,
    side: &'a str,
) -> TinyClobRow<'a> {
    TinyClobRow {
        asset_id,
        event_type: "price_change",
        timestamp_ms,
        price_scaled_4: Some(price_scaled_4),
        size_scaled_6: Some(size_scaled_6),
        side,
        transaction_hash: "",
    }
}

fn write_tiny_clob_parquet(dir: &TempDir, rows: &[TinyClobRow<'_>]) -> PathBuf {
    let path = dir.path().join("tiny-polymarket.parquet");
    let schema = Arc::new(Schema::new(vec![
        Field::new(
            "timestamp_received",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new("event_type", DataType::Utf8, true),
        Field::new("asset_id", DataType::Utf8, true),
        Field::new("bids", DataType::Utf8, true),
        Field::new("asks", DataType::Utf8, true),
        Field::new("price", DataType::Decimal128(18, 4), true),
        Field::new("size", DataType::Decimal128(18, 6), true),
        Field::new("side", DataType::Utf8, true),
        Field::new("transaction_hash", DataType::Utf8, true),
    ]));
    let timestamps: Vec<i64> = rows.iter().map(|row| row.timestamp_ms).collect();
    let strings = |values: Vec<&str>| Arc::new(StringArray::from(values)) as ArrayRef;
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(TimestampMillisecondArray::from(timestamps.clone())) as ArrayRef,
            Arc::new(TimestampMillisecondArray::from(timestamps)) as ArrayRef,
            strings(rows.iter().map(|row| row.event_type).collect()),
            strings(rows.iter().map(|row| row.asset_id).collect()),
            strings(vec![""; rows.len()]),
            strings(vec![""; rows.len()]),
            Arc::new(
                Decimal128Array::from(
                    rows.iter()
                        .map(|row| row.price_scaled_4)
                        .collect::<Vec<Option<i128>>>(),
                )
                .with_precision_and_scale(18, 4)
                .expect("price precision"),
            ) as ArrayRef,
            Arc::new(
                Decimal128Array::from(
                    rows.iter()
                        .map(|row| row.size_scaled_6)
                        .collect::<Vec<Option<i128>>>(),
                )
                .with_precision_and_scale(18, 6)
                .expect("size precision"),
            ) as ArrayRef,
            strings(rows.iter().map(|row| row.side).collect()),
            strings(rows.iter().map(|row| row.transaction_hash).collect()),
        ],
    )
    .expect("tiny CLOB record batch");
    let file = File::create(&path).expect("create tiny CLOB parquet");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("create parquet writer");
    writer.write(&batch).expect("write tiny CLOB batch");
    writer.close().expect("close tiny CLOB parquet");
    path
}

/// The Polymarket binary-outcome instrument spec for the fixture token.
///
/// Precision is derived from the source data, never hardcoded as a precision
/// integer: the price increment is the CLOB price tick (`0.01`, precision 2) and
/// the size increment is the source size-column granularity (the fixture's
/// `Decimal128(18,6)` share sizes resolve to `0.000001`, precision 6). The two
/// precisions are independent — Polymarket prices tick coarsely while share
/// sizes are fine-grained.
fn instrument_spec() -> BinaryOptionInstrumentSpec {
    BinaryOptionInstrumentSpec {
        nt_instrument_id: FIXTURE_NT_INSTRUMENT_ID.to_string(),
        raw_symbol: FIXTURE_ASSET_ID.to_string(),
        asset_class: "ALTERNATIVE".to_string(),
        quote_currency: "USDC".to_string(),
        outcome: "Up".to_string(),
        activation_ns: 1,
        expiration_ns: u64::MAX,
        price_increment: "0.01".to_string(),
        size_increment: "0.000001".to_string(),
    }
}

fn accepted_dataset() -> AcceptedDataset {
    let checks = RequiredChecks {
        source_access: RequiredCheck::passed("manifest://polymarket-clob-2026-05-22"),
        license: RequiredCheck::passed("attestation://polymarket-archive"),
        schema: RequiredCheck::passed(
            "schema://timestamp_received,timestamp,event_type,asset_id,bids,asks",
        ),
        time_semantics: RequiredCheck::passed("timestamp_received_ms_to_unix_nanos"),
        instrument_universe: RequiredCheck::passed("universe://polymarket-outcomes"),
        coverage: RequiredCheck::passed("manifest://polymarket-clob-2026-05-22"),
        granularity: RequiredCheck::passed("full_depth_snapshot_plus_deltas"),
        completeness: RequiredCheck::passed("manifest://polymarket-clob-2026-05-22"),
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
        required_checks: checks,
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

/// Decode the committed fixture Parquet into raw CLOB event rows, exactly as the
/// runner must.
fn read_fixture_rows() -> Vec<RawClobEventRow> {
    decode_polymarket_clob_parquet(&fixture_path()).expect("decode accepted-schema fixture")
}

#[test]
fn streaming_asset_discovery_matches_full_fixture_decode() {
    let rows = read_fixture_rows();
    let assets = polymarket_clob_assets_from_parquet(&fixture_path())
        .expect("stream asset ids from fixture parquet");

    assert_eq!(assets, vec![FIXTURE_ASSET_ID.to_string()]);
    assert!(rows.iter().all(|row| row.asset_id == FIXTURE_ASSET_ID));
}

#[test]
fn asset_filtered_parquet_decode_matches_full_decode_for_asset() {
    let rows = read_fixture_rows();
    let filtered = decode_polymarket_clob_parquet_for_asset(&fixture_path(), FIXTURE_ASSET_ID)
        .expect("decode one fixture asset");

    assert_eq!(filtered, rows);
    assert!(
        decode_polymarket_clob_parquet_for_asset(&fixture_path(), "missing-asset")
            .expect("decode missing asset")
            .is_empty()
    );
}

fn normalized_fixture() -> CanonicalBookTable {
    let accepted = accepted_dataset();
    let rows = read_fixture_rows();
    normalize_polymarket_clob_book(
        &accepted,
        FIXTURE_ASSET_ID,
        &rows,
        rows[0].timestamp_received,
        "ingest-run-fixture",
    )
    .expect("normalize fixture")
}

#[test]
fn book_deltas_expand_snapshot_then_deltas() {
    let table = normalized_fixture();
    let instrument = build_binary_option(&instrument_spec()).expect("build binary option");
    // Precision is derived from the increment strings: the `0.01` price tick is
    // precision 2, and the `0.000001` size granularity is precision 6.
    assert_eq!(instrument.price_precision, 2);
    assert_eq!(instrument.size_precision, 6);

    let deltas = canonical_rows_to_order_book_deltas(&table, &instrument).expect("deltas");

    // One snapshot -> 1 Clear + (2 bid + 2 ask) Adds; 2 price_changes -> 2
    // single-level deltas. Trades are routed to TradeTick, not here.
    let expected_adds = EXPECTED_SNAPSHOT_BID_LEVELS + EXPECTED_SNAPSHOT_ASK_LEVELS;
    let expected_total = 1 + expected_adds + EXPECTED_PRICE_CHANGE_ROWS;
    assert_eq!(deltas.len(), expected_total);

    // The first delta is the snapshot's Clear; the next 4 are Adds.
    assert_eq!(deltas[0].action, BookAction::Clear);
    for delta in &deltas[1..=expected_adds] {
        assert_eq!(delta.action, BookAction::Add);
    }

    // Sequences are dense and 0-based; ts_init is non-strict ascending (the
    // snapshot expansion shares one timestamp), matching NautilusTrader's
    // catalog write contract.
    let mut prev_ts = u64::MIN;
    for (i, delta) in deltas.iter().enumerate() {
        assert_eq!(delta.sequence, i as u64);
        assert!(delta.ts_init.as_u64() >= prev_ts);
        prev_ts = delta.ts_init.as_u64();
    }
}

#[test]
fn projects_and_reads_back_book_deltas_and_trades() {
    let table = normalized_fixture();
    let spec = instrument_spec();
    let dir = TempDir::new().expect("temp dir");

    let projection =
        project_canonical_book_to_catalog(&table, &spec, dir.path()).expect("project book");

    let expected_deltas = 1
        + EXPECTED_SNAPSHOT_BID_LEVELS
        + EXPECTED_SNAPSHOT_ASK_LEVELS
        + EXPECTED_PRICE_CHANGE_ROWS;
    assert_eq!(projection.delta_count, expected_deltas);
    assert_eq!(projection.trade_count, EXPECTED_TRADE_ROWS);
    assert_eq!(projection.nt_instrument_id, FIXTURE_NT_INSTRUMENT_ID);
    assert_eq!(
        projection.fidelity_class,
        SourceProofFidelityClass::L2Replay
    );
    assert!(!projection.catalog_hash.is_empty());

    // Read BOTH data types back from the same catalog (the NautilusTrader read
    // proof), proving the projection round-trips through `query_typed_data`.
    let read_deltas =
        read_back_order_book_deltas(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read deltas");
    assert_eq!(read_deltas.len(), expected_deltas);
    let id: InstrumentId = FIXTURE_NT_INSTRUMENT_ID.parse().expect("instrument id");
    for delta in &read_deltas {
        assert_eq!(delta.instrument_id, id);
    }

    let read_trades =
        read_back_trade_ticks(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read trades");
    assert_eq!(read_trades.len(), EXPECTED_TRADE_ROWS);
    for tick in &read_trades {
        assert_eq!(tick.instrument_id, id);
    }
}

#[test]
fn projection_refuses_dirty_catalog_root() {
    let table = normalized_fixture();
    let spec = instrument_spec();
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
    let err = project_canonical_book_to_catalog(&table, &spec, dir.path())
        .expect_err("dirty catalog root must be refused");
    assert!(err.to_string().contains("not empty"), "{err}");
}

#[test]
fn order_book_delta_data_type_label_is_stable() {
    assert_eq!(NT_DATA_TYPE_ORDER_BOOK_DELTA, "OrderBookDelta");
}

/// A staged object key in the accepted unified streaming archive layout.
const FIXTURE_BOOK_OBJECT_KEY: &str = "backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/family=order_book_snapshots_fixed_depth/dt=2026-05-22/object=852a6dabc415e0b73e5361db8b39d979291ee814ffa72fa8c287792979329ddc.parquet";
const FIXTURE_TRADES_OBJECT_KEY: &str = "backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/family=order_book_snapshots_fixed_depth/dt=2026-05-22/object=852a6dabc415e0b73e5361db8b39d979291ee814ffa72fa8c287792979329ddc.parquet";
const TINY_OBJECT_KEY: &str = "backfill-staging/2026-06-01/polymarket-pmxt-v2-streaming/raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/family=order_book_snapshots_fixed_depth/dt=2026-05-22/object=tiny.parquet";

#[test]
fn book_append_accepts_contiguous_multi_asset_runs() {
    let input_dir = TempDir::new().expect("temp parquet root");
    let object_path = write_tiny_clob_parquet(
        &input_dir,
        &[
            tiny_price_change(
                "1111111111111111111111111111111111111111",
                1_716_400_000_000,
                5100,
                1_000_000,
                "BUY",
            ),
            tiny_price_change(
                "1111111111111111111111111111111111111111",
                1_716_400_000_001,
                5200,
                2_000_000,
                "SELL",
            ),
            tiny_price_change(
                "2222222222222222222222222222222222222222",
                1_716_400_000_002,
                5300,
                3_000_000,
                "BUY",
            ),
        ],
    );
    let catalog_dir = TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(catalog_dir.path(), None, None, None, None);

    let summaries = append_polymarket_book_archive(&object_path, TINY_OBJECT_KEY, &mut catalog)
        .expect("append grouped tiny object");

    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].delta_count, 2);
    assert_eq!(summaries[0].trade_count, 0);
    assert_eq!(summaries[1].delta_count, 1);
    assert_eq!(summaries[1].trade_count, 0);
}

#[test]
fn book_append_rejects_non_contiguous_asset_runs() {
    let input_dir = TempDir::new().expect("temp parquet root");
    let object_path = write_tiny_clob_parquet(
        &input_dir,
        &[
            tiny_price_change(
                "1111111111111111111111111111111111111111",
                1_716_400_000_000,
                5100,
                1_000_000,
                "BUY",
            ),
            tiny_price_change(
                "2222222222222222222222222222222222222222",
                1_716_400_000_001,
                5200,
                2_000_000,
                "SELL",
            ),
            tiny_price_change(
                "1111111111111111111111111111111111111111",
                1_716_400_000_002,
                5300,
                3_000_000,
                "BUY",
            ),
        ],
    );
    let catalog_dir = TempDir::new().expect("temp catalog root");
    let mut catalog = ParquetDataCatalog::new(catalog_dir.path(), None, None, None, None);

    let error = append_polymarket_book_archive(&object_path, TINY_OBJECT_KEY, &mut catalog)
        .expect_err("non-contiguous asset run must be rejected");

    assert!(
        format!("{error:#}").contains("not grouped by asset_id"),
        "{error:#}"
    );
}

#[test]
fn book_data_derived_append_round_trips() {
    // The bulk book path: derive the binary-option precision from the object's
    // own rows (Polymarket stages no instrument universe), append into a shared
    // catalog with NO clean-root guard, and prove the NautilusTrader round-trip
    // is lossless for BOTH OrderBookDelta and TradeTick.
    let dir = TempDir::new().expect("temp catalog root");

    // Independent expectation from the same fixture, via the data-derived spec.
    let rows = decode_polymarket_clob_parquet(&fixture_path()).expect("decode fixture parquet");
    assert_eq!(
        rows.len(),
        EXPECTED_PRICE_CHANGE_ROWS + EXPECTED_TRADE_ROWS + EXPECTED_TICK_SIZE_CHANGE_ROWS + 1,
        "fixture row count (1 snapshot + 2 price_change + 2 trades + 1 tick_size_change)"
    );
    let table = normalized_fixture();
    let derived = polymarket_book_spec_from_table(&table).expect("derive spec");
    assert_eq!(derived.nt_instrument_id, FIXTURE_NT_INSTRUMENT_ID);
    assert_eq!(
        derived.nt_instrument_id,
        format!("{FIXTURE_ASSET_ID}.{POLYMARKET_VENUE}")
    );
    let instrument = build_binary_option(&derived).expect("build binary option");
    // Precision is read from the data (max decimals observed), not a hardcoded
    // literal: the staged `price` column renders at 4 decimals and the `size`
    // `Decimal128(_, 6)` column at 6.
    assert_eq!(instrument.price_precision, 4);
    assert_eq!(instrument.size_precision, 6);
    let expected_delta_count = 1
        + EXPECTED_SNAPSHOT_BID_LEVELS
        + EXPECTED_SNAPSHOT_ASK_LEVELS
        + EXPECTED_PRICE_CHANGE_ROWS;

    // Append into a freshly-opened (empty) catalog — no dirty-root refusal.
    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_polymarket_book_archive(&fixture_path(), FIXTURE_BOOK_OBJECT_KEY, &mut catalog)
            .expect("append book");
    assert_eq!(
        summaries.len(),
        1,
        "fixture carries exactly one outcome token"
    );
    assert_eq!(summaries[0].nt_instrument_id, FIXTURE_NT_INSTRUMENT_ID);
    assert_eq!(summaries[0].delta_count, expected_delta_count);
    assert_eq!(summaries[0].trade_count, EXPECTED_TRADE_ROWS);
    assert_eq!(summaries[0].price_precision, 4);
    assert_eq!(summaries[0].size_precision, 6);

    // Read BOTH data types back: count, ascending capture-clock ts, snapshot
    // flag protocol, Delete for zero-size price_change, and trade-id handling.
    let read_deltas =
        read_back_order_book_deltas(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read deltas");
    assert_eq!(
        read_deltas.len(),
        expected_delta_count,
        "round-tripped delta count"
    );
    assert!(
        read_deltas.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded deltas must be ascending"
    );
    assert_eq!(read_deltas[0].action, BookAction::Clear);
    assert!(RecordFlag::F_SNAPSHOT.matches(read_deltas[0].flags));
    assert!(RecordFlag::F_MBP.matches(read_deltas[0].flags));
    for delta in &read_deltas[1..=EXPECTED_SNAPSHOT_BID_LEVELS + EXPECTED_SNAPSHOT_ASK_LEVELS] {
        assert_eq!(delta.action, BookAction::Add);
        assert_eq!(delta.order.order_id, 0);
        assert!(RecordFlag::F_SNAPSHOT.matches(delta.flags));
        assert!(RecordFlag::F_MBP.matches(delta.flags));
    }
    assert!(RecordFlag::F_LAST.matches(read_deltas[4].flags));
    assert_eq!(read_deltas[5].action, BookAction::Update);
    assert_eq!(read_deltas[5].order.side, OrderSide::Buy);
    assert_eq!(read_deltas[5].order.order_id, 0);
    assert!(RecordFlag::F_MBP.matches(read_deltas[5].flags));
    assert_eq!(read_deltas[6].action, BookAction::Delete);
    assert_eq!(read_deltas[6].order.side, OrderSide::Sell);
    assert_eq!(read_deltas[6].order.order_id, 0);
    assert!(read_deltas[6].order.size.is_zero());
    assert!(RecordFlag::F_MBP.matches(read_deltas[6].flags));

    let read_ticks =
        read_back_trade_ticks(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read ticks");
    assert_eq!(
        read_ticks.len(),
        EXPECTED_TRADE_ROWS,
        "round-tripped tick count"
    );
    assert!(
        read_ticks.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(read_ticks[0].aggressor_side, AggressorSide::Buyer);
    assert_eq!(read_ticks[0].trade_id.to_string(), "0xabc123");
    assert_eq!(read_ticks[1].aggressor_side, AggressorSide::Seller);
    assert!(
        read_ticks[1].trade_id.to_string().starts_with("POLYCLOB-"),
        "{}",
        read_ticks[1].trade_id
    );
    assert_eq!(read_ticks[0].ts_event, read_ticks[0].ts_init);
    assert_eq!(read_ticks[1].ts_event, read_ticks[1].ts_init);
}

#[test]
fn trades_data_derived_append_round_trips() {
    // The bulk trades path: same decode/normalize/precision pipeline, but writes
    // ONLY the TradeTick projection (the `polymarket_trades/` family carries trade
    // prints). The committed CLOB fixture carries 2 trade prints, so it exercises
    // the trades family's full round-trip.
    let dir = TempDir::new().expect("temp catalog root");

    let expected_ticks = EXPECTED_TRADE_ROWS;

    let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
    let summaries =
        append_polymarket_trades_archive(&fixture_path(), FIXTURE_TRADES_OBJECT_KEY, &mut catalog)
            .expect("append trades");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].nt_instrument_id, FIXTURE_NT_INSTRUMENT_ID);
    assert_eq!(
        summaries[0].delta_count, 0,
        "trades family writes no deltas"
    );
    assert_eq!(summaries[0].trade_count, EXPECTED_TRADE_ROWS);

    // No OrderBookDelta stream is written for the trades family.
    let read_deltas =
        read_back_order_book_deltas(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read deltas");
    assert!(read_deltas.is_empty(), "trades family must write no deltas");

    let read_ticks =
        read_back_trade_ticks(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read ticks");
    assert_eq!(read_ticks.len(), expected_ticks, "round-tripped tick count");
    assert!(
        read_ticks.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(
        read_ticks[0].trade_id.to_string(),
        "0xabc123",
        "present transaction_hash must become the NT TradeId"
    );
}

#[test]
fn empty_book_snapshot_projects_to_single_clear_with_f_last() {
    // A genuine market-open empty-book snapshot (bids="[]" AND asks="[]") must
    // convert to NautilusTrader's empty-book delta: exactly one Clear carrying
    // F_SNAPSHOT|F_MBP|F_LAST and zero Adds (NT's own OrderBook::to_deltas
    // empty-book contract), and round-trip through the catalog — proving
    // write_to_parquet accepts a stream whose first and only delta is a Clear.
    let rows = vec![RawClobEventRow {
        asset_id: FIXTURE_ASSET_ID.to_string(),
        event_type: "book".to_string(),
        timestamp_received: 1_700_000_000_000_000_000,
        event_time: 1_700_000_000_000_000_000,
        source_row_index: 0,
        bids: "[]".to_string(),
        asks: "[]".to_string(),
        price: String::new(),
        size: String::new(),
        side: String::new(),
        transaction_hash: String::new(),
    }];
    let table = normalize_polymarket_clob_book(
        &accepted_dataset(),
        FIXTURE_ASSET_ID,
        &rows,
        rows[0].timestamp_received,
        "ingest-run-empty",
    )
    .expect("normalize empty-book snapshot");
    assert_eq!(table.snapshot_count(), 1);

    let instrument = build_binary_option(&instrument_spec()).expect("build binary option");
    let deltas = canonical_rows_to_order_book_deltas(&table, &instrument).expect("project deltas");
    assert_eq!(deltas.len(), 1, "empty snapshot expands to a lone Clear");
    assert_eq!(deltas[0].action, BookAction::Clear);
    let flags = deltas[0].flags;
    assert_ne!(
        flags & RecordFlag::F_LAST as u8,
        0,
        "the Clear closes the snapshot event (F_LAST)"
    );
    assert_ne!(
        flags & RecordFlag::F_SNAPSHOT as u8,
        0,
        "the Clear is replayed-snapshot data (F_SNAPSHOT)"
    );
    assert_ne!(
        flags & RecordFlag::F_MBP as u8,
        0,
        "the Clear is aggregated price-level data (F_MBP)"
    );

    // End-to-end: the lone-Clear stream writes and reads back through the
    // NautilusTrader catalog (closes the "does write_to_parquet accept a
    // first-and-only Clear delta?" question).
    let dir = TempDir::new().expect("temp dir");
    let projection = project_canonical_book_to_catalog(&table, &instrument_spec(), dir.path())
        .expect("project empty-book snapshot to catalog");
    assert_eq!(projection.delta_count, 1);
    assert_eq!(projection.trade_count, 0);
    let read_deltas = read_back_order_book_deltas(dir.path(), FIXTURE_NT_INSTRUMENT_ID)
        .expect("read deltas back");
    assert_eq!(read_deltas.len(), 1);
    assert_eq!(read_deltas[0].action, BookAction::Clear);
}
