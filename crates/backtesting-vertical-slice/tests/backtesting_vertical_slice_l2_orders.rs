//! PRIMARY L2 proof (gate 5, L2): converted Polymarket L2 book data drives
//! NautilusTrader execution.
//!
//! This is the inverse of the trade-only end-to-end proof
//! (`backtesting_vertical_slice_end_to_end.rs`, which asserts
//! `total_orders == 0` because trade prints carry no quotes). Here the committed
//! hermetic CLOB fixture is normalized, projected into a NautilusTrader
//! `ParquetDataCatalog` as `OrderBookDelta` (+ `TradeTick`), and run through a
//! real `BacktestNode` with `emit_quotes_from_book = true`. A minimal,
//! test-only NautilusTrader strategy subscribes to the book and quotes and
//! submits exactly one market order on the first valid quote — isolating the
//! single fact under test: the converted L2 book reconstructs a top-of-book that
//! NautilusTrader's data engine turns into a quote, which drives strategy order
//! entry. The proof is `total_orders > 0`.
//!
//! CI-safe: no network, no S3 — the committed fixture is the only input. The
//! project's `HurstVpinDirectional` is deliberately NOT used here: it only
//! enters after Hurst/VPIN warm-up, which would confound the proof that L2 data
//! alone drives execution.

use std::{cell::Cell, fmt::Debug, path::PathBuf, rc::Rc};

use backtesting_vertical_slice::{
    canonical_book::{
        CanonicalBookEvent, CanonicalBookTable, RawClobEventRow, decode_polymarket_clob_parquet,
        normalize_polymarket_clob_book,
    },
    catalog_projection::{
        BinaryOptionInstrumentSpec, build_binary_option, canonical_rows_to_order_book_deltas,
        project_canonical_book_to_catalog,
    },
    source_proof::{
        AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType, IngestManifestObjectRecord,
        NtMappingStatus, RequiredCheck, RequiredChecks, SourceProofFidelityClass,
        SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
    },
};
use nautilus_backtest::{
    config::{
        BacktestDataConfig, BacktestEngineConfig, BacktestRunConfig, BacktestVenueConfig,
        NautilusDataType,
    },
    node::BacktestNode,
};
use nautilus_common::actor::DataActor;
use nautilus_data::engine::config::DataEngineConfig;
use nautilus_model::{
    data::QuoteTick,
    enums::{AccountType, BookType, OmsType, OrderSide},
    identifiers::{InstrumentId, StrategyId},
    instruments::Instrument,
    orderbook::OrderBook,
    types::{Price, Quantity},
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};
use tempfile::TempDir;
use ustr::Ustr;

/// The single outcome token id in the committed fixture (test-only literal).
const FIXTURE_ASSET_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888";

/// SHA-256 of the committed accepted-schema fixture.
const FIXTURE_OBJECT_SHA256: &str =
    "852a6dabc415e0b73e5361db8b39d979291ee814ffa72fa8c287792979329ddc";

/// NautilusTrader instrument id for the fixture outcome on the Polymarket venue.
const FIXTURE_NT_INSTRUMENT_ID: &str =
    "20419872418925958113466469406112781259698061446101840345505990534096167263888.POLYMARKET";

/// NautilusTrader venue name for the fixture outcome.
const FIXTURE_VENUE: &str = "POLYMARKET";

/// The run id used for both the manifest mapping and the configured engine.
const RUN_IDENTIFIER: &str = "backtesting-vertical-slice-l2-orders";

/// Best bid/ask of the fixture's `book` snapshot, verified with duckdb at build
/// time: the snapshot's highest bid price is `0.50` and lowest ask price `0.51`.
/// These are the source top-of-book the reconstructed NautilusTrader `OrderBook`
/// must reproduce (the L2 reconstruction-fidelity oracle).
const EXPECTED_SNAPSHOT_BEST_BID: &str = "0.50";
const EXPECTED_SNAPSHOT_BEST_ASK: &str = "0.51";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/polymarket_clob_l2_slice.parquet")
}

/// The Polymarket binary-outcome instrument spec for the fixture token.
///
/// Precision is derived from the source data, never hardcoded as a precision
/// integer: the price increment is the CLOB price tick (`0.01`, precision 2) and
/// the size increment is the source size-column granularity (the fixture's
/// `Decimal128(18,6)` share sizes resolve to `0.000001`, precision 6).
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

/// Shared one-shot order count, written by the strategy and read by the test.
///
/// `Cell<usize>` behind `Rc` lets the strategy (owned by the engine) and the
/// test observe the same counter. The strategy submits a single order, so this
/// also asserts the on-first-quote guard fires exactly once.
type SharedCounter = Rc<Cell<usize>>;

/// Minimal test-only NautilusTrader strategy that submits one market order on
/// the first valid quote.
///
/// It subscribes to the order book (which, with `emit_quotes_from_book = true`,
/// makes the data engine derive a `QuoteTick` from each book update) and to the
/// derived quotes. On the first quote it submits a single market buy and records
/// that it submitted, isolating the proof that converted L2 data drives order
/// entry. Order entry depends only on a valid quote, so a fill is not required —
/// `total_orders` counts submitted orders in the cache.
#[derive(Debug)]
struct FirstQuoteMarketBuy {
    core: StrategyCore,
    instrument_id: InstrumentId,
    trade_size: Quantity,
    submitted: bool,
    submit_count: SharedCounter,
}

impl FirstQuoteMarketBuy {
    fn new(instrument_id: InstrumentId, trade_size: Quantity, submit_count: SharedCounter) -> Self {
        let config = StrategyConfig {
            strategy_id: Some(StrategyId::from("FIRST-QUOTE-MARKET-BUY-001")),
            order_id_tag: Some("001".to_string()),
            ..Default::default()
        };
        Self {
            core: StrategyCore::new(config),
            instrument_id,
            trade_size,
            submitted: false,
            submit_count,
        }
    }
}

nautilus_strategy!(FirstQuoteMarketBuy);

impl DataActor for FirstQuoteMarketBuy {
    fn on_start(&mut self) -> anyhow::Result<()> {
        // A *managed* book-deltas subscription makes the data engine create the
        // cache `OrderBook` and the `BookUpdater` that, with
        // `emit_quotes_from_book`, derives a quote from each delta. (An
        // unmanaged subscription registers the handler but never sets up the
        // updater, so no quotes are derived.) Subscribing to quotes then routes
        // those derived quotes to `on_quote`.
        self.subscribe_book_deltas(self.instrument_id, BookType::L2_MBP, None, None, true, None);
        self.subscribe_quotes(self.instrument_id, None, None);
        Ok(())
    }

    fn on_quote(&mut self, _quote: &QuoteTick) -> anyhow::Result<()> {
        if self.submitted {
            return Ok(());
        }
        let order = self.core.order_factory().market(
            self.instrument_id,
            OrderSide::Buy,
            self.trade_size,
            None, // time_in_force
            None, // reduce_only
            None, // quote_quantity
            None, // exec_algorithm_id
            None, // exec_algorithm_params
            None, // tags
            None, // client_order_id
        );
        self.submit_order(order, None, None, None)?;
        self.submitted = true;
        self.submit_count.set(self.submit_count.get() + 1);
        Ok(())
    }
}

/// Build the NautilusTrader run config for the projected L2 catalog directly,
/// turning on book-derived quotes. This mirrors what
/// `BacktestingRunManifest::to_nt_run_config` produces for an `OrderBookDelta`
/// catalog input + `L2_MBP` venue, but is built inline so the test owns the
/// instrument id and catalog path without a full manifest fixture.
fn l2_run_config(catalog_path: String, instrument_id: InstrumentId) -> BacktestRunConfig {
    let venue = BacktestVenueConfig::builder()
        .name(Ustr::from(FIXTURE_VENUE))
        .oms_type(OmsType::Netting)
        .account_type(AccountType::Cash)
        .book_type(BookType::L2_MBP)
        .starting_balances(vec!["1_000_000 USDC".to_string()])
        .build();

    let data = BacktestDataConfig::builder()
        .data_type(NautilusDataType::OrderBookDelta)
        .catalog_path(catalog_path)
        .instrument_id(instrument_id)
        .build();

    let data_engine = DataEngineConfig::builder()
        .emit_quotes_from_book(true)
        .build();
    let engine = BacktestEngineConfig::builder()
        .data_engine(data_engine)
        .build();

    BacktestRunConfig::builder()
        .id(RUN_IDENTIFIER.to_string())
        .venues(vec![venue])
        .data(vec![data])
        .engine(engine)
        .build()
}

#[test]
fn converted_l2_book_drives_backtest_order_entry() {
    let table = normalized_fixture();
    let spec = instrument_spec();
    let dir = TempDir::new().expect("temp dir");
    let catalog_root = dir.path().join("nt-catalog");
    let catalog_path = catalog_root.to_str().expect("utf-8 path").to_string();

    // Convert the fixture into an NautilusTrader catalog (OrderBookDelta +
    // TradeTick) and confirm the L2 data is present.
    let projection =
        project_canonical_book_to_catalog(&table, &spec, &catalog_root).expect("project book");
    assert!(
        projection.delta_count > 0,
        "the projected catalog must carry order-book deltas"
    );
    assert_eq!(projection.nt_instrument_id, FIXTURE_NT_INSTRUMENT_ID);

    let instrument_id: InstrumentId = FIXTURE_NT_INSTRUMENT_ID.parse().expect("instrument id");
    let run_config = l2_run_config(catalog_path, instrument_id);

    let mut node = BacktestNode::new(vec![run_config]).expect("construct backtest node");
    node.build().expect("build backtest node");

    // One share at the instrument's size precision (6). The size only has to be
    // representable; order entry, not fill, is the fact under test.
    let trade_size = Quantity::from("1.000000");
    let submit_count: SharedCounter = Rc::new(Cell::new(0));
    {
        let engine = node
            .get_engine_mut(RUN_IDENTIFIER)
            .expect("engine for configured run");
        engine
            .add_strategy(FirstQuoteMarketBuy::new(
                instrument_id,
                trade_size,
                submit_count.clone(),
            ))
            .expect("add minimal L2 strategy");
    }

    let results = node.run().expect("run backtest node");
    assert_eq!(results.len(), 1, "exactly one configured run must execute");
    let result = &results[0];
    assert_eq!(result.run_config_id.as_deref(), Some(RUN_IDENTIFIER));

    // The whole point: converted L2 book data reconstructed a top-of-book, the
    // data engine derived a quote, and the strategy entered. This is the inverse
    // of the trade-only path (`total_orders == 0`).
    assert!(
        result.total_orders > 0,
        "converted L2 book data must drive at least one order; got total_orders={}",
        result.total_orders
    );
    // The strategy is one-shot: the derived quote fired `on_quote` and exactly
    // one order was submitted, proving the order flowed from a real quote.
    assert_eq!(
        submit_count.get(),
        1,
        "the one-shot strategy must submit exactly one order off the first derived quote"
    );
}

#[test]
fn reconstructed_book_top_matches_source_snapshot() {
    // Reconstruction fidelity: apply the projected snapshot deltas to a real
    // NautilusTrader `OrderBook` and assert the reconstructed top-of-book equals
    // the source snapshot's best bid/ask. NautilusTrader's `OrderBook` is the
    // oracle — the converter does not compute top-of-book itself.
    let table = normalized_fixture();
    let instrument = build_binary_option(&instrument_spec()).expect("build binary option");
    let deltas = canonical_rows_to_order_book_deltas(&table, &instrument).expect("deltas");

    let mut book = OrderBook::new(instrument.id(), BookType::L2_MBP);
    // Apply only the snapshot expansion (the leading Clear + per-level Adds);
    // stop before the first single-level `price_change` so the asserted
    // top-of-book is the source snapshot's, with no later mutation folded in.
    // The snapshot is the run's first event, so its deltas lead the sequence:
    // 1 Clear + (bid levels + ask levels) Adds.
    let CanonicalBookEvent::Snapshot(snapshot) = &table.rows[0].event else {
        panic!("the fixture's first event must be the full-depth snapshot");
    };
    let snapshot_delta_count = 1 + snapshot.bids.len() + snapshot.asks.len();
    for delta in deltas.iter().take(snapshot_delta_count) {
        book.apply_delta(delta).expect("apply snapshot delta");
    }

    let best_bid = book.best_bid_price().expect("reconstructed best bid");
    let best_ask = book.best_ask_price().expect("reconstructed best ask");
    assert_eq!(
        best_bid,
        Price::from(EXPECTED_SNAPSHOT_BEST_BID),
        "reconstructed best bid must match the source snapshot"
    );
    assert_eq!(
        best_ask,
        Price::from(EXPECTED_SNAPSHOT_BEST_ASK),
        "reconstructed best ask must match the source snapshot"
    );
}
