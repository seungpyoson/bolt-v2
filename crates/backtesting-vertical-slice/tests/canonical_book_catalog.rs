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

use std::{fs::File, path::PathBuf};

use arrow::array::{Array, Decimal128Array, StringArray, TimestampMicrosecondArray};
use backtesting_vertical_slice::{
    canonical_book::{
        CanonicalBookTable, POLYMARKET_VENUE, RawClobEventRow, append_polymarket_book_archive,
        append_polymarket_trades_archive, decode_polymarket_clob_parquet,
        normalize_polymarket_clob_book, polymarket_book_spec_from_table,
    },
    catalog_projection::{
        BinaryOptionInstrumentSpec, NT_DATA_TYPE_ORDER_BOOK_DELTA, build_binary_option,
        canonical_book_rows_to_trade_ticks, canonical_rows_to_order_book_deltas,
        project_canonical_book_to_catalog, read_back_order_book_deltas, read_back_trade_ticks,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};
use nautilus_model::{enums::BookAction, identifiers::InstrumentId};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rust_decimal::Decimal;
use tempfile::TempDir;

/// The single outcome token id in the committed fixture (test-only literal).
const FIXTURE_ASSET_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888";

/// SHA-256 of the real archive object the fixture was downsampled from.
const FIXTURE_OBJECT_SHA256: &str =
    "b32d8dc1944c550191f62a79fc0a9bec25fa0c498705801998a4fd3adf279f19";

/// NautilusTrader instrument id for the fixture outcome on the Polymarket venue.
const FIXTURE_NT_INSTRUMENT_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888.POLYMARKET";

// Event counts verified against the fixture with duckdb at build time. The
// snapshot expands to one `Clear` plus one `Add` per level (45 bid + 46 ask
// levels = 91 adds), and each `price_change` maps to one delta.
const EXPECTED_SNAPSHOT_BID_LEVELS: usize = 45;
const EXPECTED_SNAPSHOT_ASK_LEVELS: usize = 46;
const EXPECTED_PRICE_CHANGE_ROWS: usize = 66;
const EXPECTED_TRADE_ROWS: usize = 2;

const NANOS_PER_MICROSECOND: i64 = 1_000;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/polymarket_clob_l2_slice.parquet")
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
        schema: RequiredCheck::passed("schema://timestamp,event_type,asset_id,bids,asks"),
        time_semantics: RequiredCheck::passed("utc_micros_to_unix_nanos"),
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

/// Decode the committed fixture Parquet into raw CLOB event rows, exactly as the
/// runner must (handoff decision: the runner decodes the accepted Parquet object
/// into `RawClobEventRow` identically to this helper).
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

fn normalized_fixture() -> CanonicalBookTable {
    let accepted = accepted_dataset();
    let rows = read_fixture_rows();
    normalize_polymarket_clob_book(&accepted, FIXTURE_ASSET_ID, &rows, 7, "ingest-run-fixture")
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

    // One snapshot -> 1 Clear + (45 bid + 46 ask) Adds; 66 price_changes -> 66
    // single-level deltas. Trades are routed to TradeTick, not here.
    let expected_adds = EXPECTED_SNAPSHOT_BID_LEVELS + EXPECTED_SNAPSHOT_ASK_LEVELS;
    let expected_total = 1 + expected_adds + EXPECTED_PRICE_CHANGE_ROWS;
    assert_eq!(deltas.len(), expected_total);

    // The first delta is the snapshot's Clear; the next 91 are Adds.
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
fn book_trade_ticks_carry_sequence_trade_ids() {
    let table = normalized_fixture();
    let instrument = build_binary_option(&instrument_spec()).expect("build binary option");
    let ticks = canonical_book_rows_to_trade_ticks(&table, &instrument).expect("ticks");
    assert_eq!(ticks.len(), EXPECTED_TRADE_ROWS);
    // The NautilusTrader TradeId is the dense canonical sequence under a stable
    // prefix (the 66-char on-chain tx hash exceeds NautilusTrader's 36-char
    // TradeId limit). Ids fit the limit and are unique per print.
    let mut ids = std::collections::BTreeSet::new();
    for tick in &ticks {
        let id = tick.trade_id.to_string();
        assert!(id.starts_with("POLYCLOB-"), "{id}");
        assert!(id.len() <= 36, "trade id {id} exceeds NT 36-char limit");
        assert!(ids.insert(id), "trade ids must be unique per print");
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

/// A staged object key in the bulk layout (top-level `polymarket_parquet/`
/// prefix, NOT under backfill-staging) carrying the `dt=` segment the bulk path
/// parses for the honest archive date. The `dt` matches the fixture's coverage.
const FIXTURE_BOOK_OBJECT_KEY: &str =
    "polymarket_parquet/polymarket_book/dt=2026-05-22/object=b32d8d.parquet";
const FIXTURE_TRADES_OBJECT_KEY: &str =
    "polymarket_parquet/polymarket_trades/dt=2026-05-22/object=b32d8d.parquet";

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
        EXPECTED_PRICE_CHANGE_ROWS + EXPECTED_TRADE_ROWS + 1,
        "fixture row count (1 snapshot + 66 price_change + 2 trades)"
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
    let expected_deltas = canonical_rows_to_order_book_deltas(&table, &instrument).expect("deltas");
    let expected_ticks = canonical_book_rows_to_trade_ticks(&table, &instrument).expect("ticks");

    let expected_delta_count = 1
        + EXPECTED_SNAPSHOT_BID_LEVELS
        + EXPECTED_SNAPSHOT_ASK_LEVELS
        + EXPECTED_PRICE_CHANGE_ROWS;
    assert_eq!(expected_deltas.len(), expected_delta_count);
    assert_eq!(expected_ticks.len(), EXPECTED_TRADE_ROWS);

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

    // Read BOTH data types back: count, ascending ts, identical payload.
    let read_deltas =
        read_back_order_book_deltas(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read deltas");
    assert_eq!(
        read_deltas.len(),
        expected_deltas.len(),
        "round-tripped delta count"
    );
    assert!(
        read_deltas.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded deltas must be ascending"
    );
    assert_eq!(
        read_deltas, expected_deltas,
        "data-derived book append must round-trip deltas identically"
    );

    let read_ticks =
        read_back_trade_ticks(dir.path(), FIXTURE_NT_INSTRUMENT_ID).expect("read ticks");
    assert_eq!(
        read_ticks.len(),
        expected_ticks.len(),
        "round-tripped tick count"
    );
    assert!(
        read_ticks.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(
        read_ticks, expected_ticks,
        "data-derived book append must round-trip trade prints identically"
    );
}

#[test]
fn trades_data_derived_append_round_trips() {
    // The bulk trades path: same decode/normalize/precision pipeline, but writes
    // ONLY the TradeTick projection (the `polymarket_trades/` family carries trade
    // prints). The committed CLOB fixture carries 2 trade prints, so it exercises
    // the trades family's full round-trip.
    let dir = TempDir::new().expect("temp catalog root");

    let table = normalized_fixture();
    let derived = polymarket_book_spec_from_table(&table).expect("derive spec");
    let instrument = build_binary_option(&derived).expect("build binary option");
    let expected_ticks = canonical_book_rows_to_trade_ticks(&table, &instrument).expect("ticks");
    assert_eq!(expected_ticks.len(), EXPECTED_TRADE_ROWS);

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
    assert_eq!(
        read_ticks.len(),
        expected_ticks.len(),
        "round-tripped tick count"
    );
    assert!(
        read_ticks.windows(2).all(|w| w[0].ts_init <= w[1].ts_init),
        "loaded ticks must be ascending"
    );
    assert_eq!(
        read_ticks, expected_ticks,
        "data-derived trades append must round-trip identically"
    );
}
