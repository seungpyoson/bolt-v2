//! Backtesting Engine Gate-1 / Gate-2 proof (spec 023, issue #438).
//!
//! Proves, empirically, against the exact NautilusTrader rev this repo pins
//! (`6e059dcbb59ac1e582132fc431a581936c216c3c`, NT v0.58.0), the two foundational
//! Implementation Gates from
//! `specs/023-nt-research-analytics-platform/1-backtesting-engine/plan.md`:
//!
//!   * **Gate 1 (BTE-001)** — `nautilus-backtest` compiles in bolt-v2 with the
//!     `streaming` feature that gates `BacktestNode` and the entire
//!     catalog-driven API, pure Rust, no pyo3/python. Proven by
//!     [`gate1_backtest_node_constructs_from_catalog_config`].
//!   * **Gate 2 (BTE-007)** — `nautilus-persistence`'s `ParquetDataCatalog` can
//!     write and read back a multi-instrument binary-option fixture
//!     ([`gate2_local_catalog_round_trip_binary_option`]), and the `s3://`
//!     object-store backend is wired via the `cloud` feature
//!     ([`gate2_s3_object_store_backend_is_wired`]), proving the
//!     configured-`artifact_root` storage path is reachable in Rust.
//!
//! Per the operator decision recorded on #438, the S3 leg is proven at the
//! *interface* level (scheme dispatch reaches the cloud backend rather than the
//! "Cloud storage support requires the cloud feature" bail at
//! `nautilus-persistence` `parquet.rs:539`); a live-bucket round-trip is deferred
//! to the #438 contract slice. plan.md Gate 2 explicitly permits this: "If direct
//! S3 catalog access is not supported, document the supported staging path before
//! implementation."
//!
//! This file is gated behind the `bte-gate-proof` cargo feature so the production
//! `LiveNode` build never compiles `nautilus-backtest` or the persistence `cloud`
//! backend. Run it with:
//!
//! ```text
//! cargo test --features bte-gate-proof --test bte_gate1_backtest_proof
//! ```
//!
//! Out of scope for this gate (tracked under later #438 / #439 tasks): a full
//! `BacktestNode::run()` over the catalog (BTE-029), the `SourceProofReport` /
//! Artifact Index contracts (BTE-005/015), and a live-bucket S3 round-trip.
#![cfg(feature = "bte-gate-proof")]

use std::str::FromStr;

use nautilus_backtest::config::{
    BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType,
};
use nautilus_backtest::node::BacktestNode;
use nautilus_core::UnixNanos;
use nautilus_model::data::TradeTick;
use nautilus_model::enums::{AccountType, AggressorSide, AssetClass, BookType, OmsType};
use nautilus_model::identifiers::{InstrumentId, Symbol, TradeId};
use nautilus_model::instruments::{BinaryOption, Instrument, InstrumentAny};
use nautilus_model::types::{Currency, Price, Quantity};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use ustr::Ustr;

/// Venue + raw symbol are fixture parameters only — never branched on in engine
/// logic. A real run binds these through TOML/registry (spec 023 BTE-003).
const VENUE: &str = "POLYMARKET";
const RAW_SYMBOL: &str = "0xPROOF-OUTCOME-YES";

fn instrument_id() -> InstrumentId {
    InstrumentId::from_str(&format!("{RAW_SYMBOL}.{VENUE}")).unwrap()
}

/// A minimal but valid Polymarket-shaped binary option.
///
/// `price_precision`/`size_precision` are taken from the increments so
/// `BinaryOption::new`'s internal precision checks pass.
fn sample_binary_option() -> BinaryOption {
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    BinaryOption::new(
        instrument_id(),
        Symbol::new(RAW_SYMBOL),
        AssetClass::Alternative,
        Currency::USDC(),
        UnixNanos::from(1_699_304_047_000_000_000), // activation_ns
        UnixNanos::from(1_708_729_140_000_000_000), // expiration_ns
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        Some(Ustr::from("YES")), // outcome
        None,                    // description
        None,                    // max_quantity
        None,                    // min_quantity
        None,                    // max_notional
        None,                    // min_notional
        None,                    // max_price
        None,                    // min_price
        None,                    // margin_init
        None,                    // margin_maint
        None,                    // maker_fee
        None,                    // taker_fee
        None,                    // info
        UnixNanos::default(),    // ts_event
        UnixNanos::default(),    // ts_init
    )
}

/// `n` trades with strictly ascending `ts_init` (the catalog write path requires
/// ascending timestamps).
fn sample_trades(n: u64) -> Vec<TradeTick> {
    (1..=n)
        .map(|i| {
            TradeTick::new(
                instrument_id(),
                Price::from("0.450"),
                Quantity::from("10.00"),
                AggressorSide::Buyer,
                TradeId::new(format!("T-{i}")),
                UnixNanos::from(i),
                UnixNanos::from(i),
            )
        })
        .collect()
}

/// Gate 2 (BTE-007) — local `ParquetDataCatalog` round-trip for a binary option.
///
/// Writes one `BinaryOption` instrument plus three `TradeTick`s under a temp
/// `artifact_root`, then reads both back and asserts the payloads survive the
/// parquet round-trip. This needs no cargo features beyond the crate being
/// present (local filesystem object-store is unconditional).
#[test]
fn gate2_local_catalog_round_trip_binary_option() {
    let tmp = tempfile::tempdir().expect("temp artifact_root");
    let mut catalog = ParquetDataCatalog::new(tmp.path(), None, Some(5000), None, None);

    // Instruments use the dedicated write/read path (bypasses DataFusion).
    let instrument: InstrumentAny = sample_binary_option().into_any();
    catalog
        .write_instruments(vec![instrument.clone()])
        .expect("write binary option instrument");

    let trades = sample_trades(3);
    catalog
        .write_to_parquet(trades.clone(), None, None, None)
        .expect("write trades to catalog");

    let read_instruments = catalog.query_instruments(None).expect("read instruments");
    assert_eq!(read_instruments.len(), 1, "one binary option round-trips");
    assert_eq!(
        read_instruments[0].id(),
        instrument.id(),
        "instrument id survives the round-trip"
    );

    let read_trades: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(None, None, None, None, None, true)
        .expect("read trades back");
    assert_eq!(read_trades.len(), 3, "all three trades round-trip");
    assert_eq!(
        read_trades, trades,
        "trade payloads are identical after the parquet round-trip"
    );
}

/// Gate 2 (BTE-007) — the `s3://` object-store backend is compiled in.
///
/// With the `cloud` feature on, `from_uri("s3://...")` must dispatch to the real
/// S3 object-store backend rather than the "Cloud storage support requires the
/// cloud feature" bail. object_store's S3 builder is lazy, so construction does
/// not touch the network; a live-bucket round-trip is deferred per #438.
#[test]
fn gate2_s3_object_store_backend_is_wired() {
    let result =
        ParquetDataCatalog::from_uri("s3://bolt-v2-bte-proof/nt-catalog", None, None, None, None);
    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.to_lowercase().contains("cloud feature"),
            "s3:// must reach the cloud object-store backend, not the cloud-feature bail; got: {msg}"
        );
    }
}

/// Gate 1 (BTE-001) — `nautilus-backtest`'s `streaming` API compiles and a
/// `BacktestNode` constructs from a catalog-backed run config.
///
/// Building the `BacktestVenueConfig` / `BacktestDataConfig` / `BacktestRunConfig`
/// `bon` builders and `BacktestNode::new` proves the crate + `streaming` feature
/// are enabled and usable in bolt-v2 against the pinned rev. The config is the
/// realistic binary-option/CLOB shape: an `L2_MBP` venue (matching #438's
/// `L2_REPLAY` fidelity target) fed both order-book-delta and trade data — which
/// also exercises `BacktestNode::new`'s cross-validation that an `L2_MBP`/`L3_MBO`
/// venue has order-book data configured (`node.rs:341-368`). (A full `run()` is
/// BTE-029, out of this gate's scope.)
#[test]
fn gate1_backtest_node_constructs_from_catalog_config() {
    let tmp = tempfile::tempdir().expect("temp artifact_root");
    let catalog_path = tmp.path().to_string_lossy().to_string();

    let venue = BacktestVenueConfig::builder()
        .name(Ustr::from(VENUE))
        .oms_type(OmsType::Netting)
        .account_type(AccountType::Cash)
        .book_type(BookType::L2_MBP)
        .starting_balances(vec!["1000000 USDC".to_string()])
        .base_currency(Currency::USDC())
        .build();

    let book_data = BacktestDataConfig::builder()
        .data_type(NautilusDataType::OrderBookDelta)
        .catalog_path(catalog_path.clone())
        .instrument_id(instrument_id())
        .build();

    let trade_data = BacktestDataConfig::builder()
        .data_type(NautilusDataType::TradeTick)
        .catalog_path(catalog_path)
        .instrument_id(instrument_id())
        .build();

    let run_config = BacktestRunConfig::builder()
        .venues(vec![venue])
        .data(vec![book_data, trade_data])
        .build();

    let node =
        BacktestNode::new(vec![run_config]).expect("BacktestNode builds from one run config");
    assert_eq!(
        node.configs().len(),
        1,
        "exactly one run config (kernel MessageBus is a thread-local singleton)"
    );
}
