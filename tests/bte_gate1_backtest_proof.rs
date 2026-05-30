//! Backtesting Engine Gate-1 / Gate-2 proof (spec 023, issue #438).
//!
//! Proves, empirically, against the exact NautilusTrader rev this repo pins
//! (`6e059dcbb59ac1e582132fc431a581936c216c3c`, NT v0.58.0), the two foundational
//! Implementation Gates from
//! `specs/023-nt-research-analytics-platform/1-backtesting-engine/plan.md`,
//! across **both** spec market-structure fixtures (BTE-003): `binary option`
//! and `perps/spot`.
//!
//!   * **Gate 1 (BTE-001)** — `nautilus-backtest` compiles in bolt-v2 with the
//!     `streaming` feature that gates `BacktestNode` and the catalog-driven API,
//!     pure Rust, no pyo3/python; `BacktestNode` constructs from a catalog-backed
//!     run config.
//!   * **Gate 2 (BTE-007)** — `nautilus-persistence`'s `ParquetDataCatalog`
//!     writes and reads back instrument + trade fixtures, and the `s3://`
//!     object-store backend is wired via the `cloud` feature.
//!
//! Both gates are exercised for four market families spanning the two fixtures,
//! each with a venue selected only as a config/fixture parameter (no hardcoded
//! venue branch in engine logic — spec 023 BTE-003):
//!
//! | Family | Fixture | NT instrument | Venue (example) | Account |
//! |--------|---------|---------------|-----------------|---------|
//! | binary option | `binary option` | `BinaryOption` | POLYMARKET | Cash |
//! | CEX spot | `perps/spot` | `CurrencyPair` | BINANCE | Cash |
//! | CEX perp | `perps/spot` | `CryptoPerpetual` | BINANCE | Margin |
//! | perp DEX | `perps/spot` | `CryptoPerpetual` | HYPERLIQUID | Margin |
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
use nautilus_model::instruments::{
    BinaryOption, CryptoPerpetual, CurrencyPair, Instrument, InstrumentAny,
};
use nautilus_model::types::{Currency, Price, Quantity};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use ustr::Ustr;

/// A binary option shaped like a Polymarket outcome token (fixture: `binary
/// option`). `price_precision`/`size_precision` are taken from the increments so
/// `BinaryOption::new`'s internal precision checks pass.
fn polymarket_binary_option() -> InstrumentAny {
    let price_increment = Price::from("0.001");
    let size_increment = Quantity::from("0.01");
    BinaryOption::new(
        InstrumentId::from_str("0xPROOF-OUTCOME-YES.POLYMARKET").unwrap(),
        Symbol::new("0xPROOF-OUTCOME-YES"),
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
    .into_any()
}

/// A spot pair shaped like Binance BTCUSDT (fixture: `perps/spot`, CEX spot).
fn cex_spot_currency_pair() -> InstrumentAny {
    let price_increment = Price::from("0.01");
    let size_increment = Quantity::from("0.000001");
    CurrencyPair::new(
        InstrumentId::from_str("BTCUSDT.BINANCE").unwrap(),
        Symbol::new("BTCUSDT"),
        Currency::BTC(),  // base
        Currency::USDT(), // quote
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        None, // multiplier
        None, // lot_size
        None, // max_quantity
        None, // min_quantity
        None, // max_notional
        None, // min_notional
        None, // max_price
        None, // min_price
        None, // margin_init
        None, // margin_maint
        None, // maker_fee
        None, // taker_fee
        None, // info
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .into_any()
}

/// A linear crypto perpetual (fixture: `perps/spot`, perp). Used for both the
/// CEX-perp (Binance USD-M) and perp-DEX (Hyperliquid) families — the venue and
/// settlement currency are the only differences.
fn crypto_perpetual(
    id: &str,
    symbol: &str,
    quote: Currency,
    settlement: Currency,
) -> InstrumentAny {
    let price_increment = Price::from("0.1");
    let size_increment = Quantity::from("0.001");
    CryptoPerpetual::new(
        InstrumentId::from_str(id).unwrap(),
        Symbol::new(symbol),
        Currency::BTC(), // base
        quote,
        settlement,
        false, // is_inverse (linear)
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        None, // multiplier
        None, // lot_size
        None, // max_quantity
        None, // min_quantity
        None, // max_notional
        None, // min_notional
        None, // max_price
        None, // min_price
        None, // margin_init
        None, // margin_maint
        None, // maker_fee
        None, // taker_fee
        None, // info
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .into_any()
}

/// One market family to prove (a fixture binding — venue/currencies are config
/// parameters, never branched on in engine logic).
struct MarketFamily {
    label: &'static str,
    instrument: InstrumentAny,
    venue: &'static str,
    account_type: AccountType,
    base_currency: Option<Currency>,
    starting_balance: &'static str,
    trade_price: &'static str,
    trade_size: &'static str,
}

/// Drives both gates for one market family: write the instrument + three trades
/// to a temp `ParquetDataCatalog`, read both back identical (Gate 2 local), then
/// build a catalog-backed `BacktestNode` (Gate 1). The venue is an `L2_MBP` CLOB
/// venue, so `BacktestNode::new`'s "L2 needs order-book data" invariant
/// (`node.rs:341-368`) is satisfied with an `OrderBookDelta` + `TradeTick` config.
fn prove_market_family(f: MarketFamily) {
    let MarketFamily {
        label,
        instrument,
        venue,
        account_type,
        base_currency,
        starting_balance,
        trade_price,
        trade_size,
    } = f;
    let tmp = tempfile::tempdir().expect("temp artifact_root");
    let mut catalog = ParquetDataCatalog::new(tmp.path(), None, Some(5000), None, None);
    let id = instrument.id();

    // Instruments use the dedicated write/read path (bypasses DataFusion).
    catalog
        .write_instruments(vec![instrument.clone()])
        .unwrap_or_else(|e| panic!("{label}: write instrument: {e}"));

    let trades: Vec<TradeTick> = (1..=3u64)
        .map(|i| {
            TradeTick::new(
                id,
                Price::from(trade_price),
                Quantity::from(trade_size),
                AggressorSide::Buyer,
                TradeId::new(format!("{label}-{i}")),
                UnixNanos::from(i),
                UnixNanos::from(i),
            )
        })
        .collect();
    catalog
        .write_to_parquet(trades.clone(), None, None, None)
        .unwrap_or_else(|e| panic!("{label}: write trades: {e}"));

    // Gate 2 (local): round-trip.
    let read_instruments = catalog
        .query_instruments(None)
        .unwrap_or_else(|e| panic!("{label}: read instruments: {e}"));
    assert_eq!(
        read_instruments.len(),
        1,
        "{label}: one instrument round-trips"
    );
    assert_eq!(
        read_instruments[0].id(),
        id,
        "{label}: instrument id survives the round-trip"
    );
    let read_trades: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(None, None, None, None, None, true)
        .unwrap_or_else(|e| panic!("{label}: read trades: {e}"));
    assert_eq!(read_trades.len(), 3, "{label}: all three trades round-trip");
    assert_eq!(
        read_trades, trades,
        "{label}: trade payloads are identical after the parquet round-trip"
    );

    // Gate 1: construct a catalog-backed BacktestNode.
    let catalog_path = tmp.path().to_string_lossy().to_string();
    let venue_cfg = BacktestVenueConfig::builder()
        .name(Ustr::from(venue))
        .oms_type(OmsType::Netting)
        .account_type(account_type)
        .book_type(BookType::L2_MBP)
        .starting_balances(vec![starting_balance.to_string()])
        .maybe_base_currency(base_currency)
        .build();

    let book_data = BacktestDataConfig::builder()
        .data_type(NautilusDataType::OrderBookDelta)
        .catalog_path(catalog_path.clone())
        .instrument_id(id)
        .build();
    let trade_data = BacktestDataConfig::builder()
        .data_type(NautilusDataType::TradeTick)
        .catalog_path(catalog_path)
        .instrument_id(id)
        .build();
    let run_config = BacktestRunConfig::builder()
        .venues(vec![venue_cfg])
        .data(vec![book_data, trade_data])
        .build();

    let node = BacktestNode::new(vec![run_config])
        .unwrap_or_else(|e| panic!("{label}: BacktestNode construct: {e}"));
    assert_eq!(
        node.configs().len(),
        1,
        "{label}: exactly one run config (kernel MessageBus is a thread-local singleton)"
    );
}

/// Fixture `binary option` — Polymarket-shaped `BinaryOption` on a Cash account.
#[test]
fn binary_option_polymarket() {
    prove_market_family(MarketFamily {
        label: "binary-option",
        instrument: polymarket_binary_option(),
        venue: "POLYMARKET",
        account_type: AccountType::Cash,
        base_currency: Some(Currency::USDC()),
        starting_balance: "1000000 USDC",
        trade_price: "0.450",
        trade_size: "10.00",
    });
}

/// Fixture `perps/spot` — CEX spot `CurrencyPair` (Binance BTCUSDT, Cash account).
#[test]
fn perps_spot_cex_spot_binance() {
    prove_market_family(MarketFamily {
        label: "cex-spot",
        instrument: cex_spot_currency_pair(),
        venue: "BINANCE",
        account_type: AccountType::Cash,
        base_currency: None, // multi-currency spot account
        starting_balance: "1000000 USDT",
        trade_price: "60000.00",
        trade_size: "0.000100",
    });
}

/// Fixture `perps/spot` — CEX perp `CryptoPerpetual` (Binance USD-M, Margin).
#[test]
fn perps_spot_cex_perp_binance() {
    prove_market_family(MarketFamily {
        label: "cex-perp",
        instrument: crypto_perpetual(
            "BTCUSDT-PERP.BINANCE",
            "BTCUSDT-PERP",
            Currency::USDT(),
            Currency::USDT(),
        ),
        venue: "BINANCE",
        account_type: AccountType::Margin,
        base_currency: Some(Currency::USDT()),
        starting_balance: "1000000 USDT",
        trade_price: "60000.0",
        trade_size: "0.001",
    });
}

/// Fixture `perps/spot` — perp DEX `CryptoPerpetual` (Hyperliquid, USDC-settled,
/// Margin).
#[test]
fn perps_spot_perp_dex_hyperliquid() {
    prove_market_family(MarketFamily {
        label: "perp-dex",
        instrument: crypto_perpetual(
            "BTC-PERP.HYPERLIQUID",
            "BTC-PERP",
            Currency::USDC(),
            Currency::USDC(),
        ),
        venue: "HYPERLIQUID",
        account_type: AccountType::Margin,
        base_currency: Some(Currency::USDC()),
        starting_balance: "1000000 USDC",
        trade_price: "60000.0",
        trade_size: "0.001",
    });
}

/// Gate 2 (BTE-007) — the `s3://` object-store backend is compiled in.
///
/// With the `cloud` feature on, `from_uri("s3://...")` constructs the real S3
/// object-store backend rather than hitting the "Cloud storage support requires
/// the cloud feature" bail. The builder is lazy — `object_store`'s
/// `AmazonS3Builder::build` defaults the region and resolves the
/// instance-credential provider at request time (`object_store-0.13.2`
/// `aws/builder.rs:1086,1164`), so construction returns `Ok` without touching
/// the network. The positive `is_ok` path is therefore the primary assertion,
/// and it also rules out the cloud-feature bail (which is an `Err`). A
/// live-bucket round-trip is deferred per #438.
#[test]
fn gate2_s3_object_store_backend_is_wired() {
    // Synthetic fixture bucket — never provisioned, never contacted.
    let result =
        ParquetDataCatalog::from_uri("s3://example-bte-proof/nt-catalog", None, None, None, None);
    assert!(
        result.is_ok(),
        "s3:// must construct the cloud object-store backend (lazy, no network) \
         with the `cloud` feature on, not hit the cloud-feature bail; got: {:?}",
        result.err()
    );
}
