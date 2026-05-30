//! Backtesting Engine Gate-1 / Gate-2 / Gate-4 proof (spec 023, issue #438).
//!
//! Proves, empirically, against the exact NautilusTrader rev this repo pins
//! (`6e059dcbb59ac1e582132fc431a581936c216c3c`, NT v0.58.0), the foundational
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
//!     writes and reads back instrument, trade, and order-book-delta fixtures,
//!     and the `s3://` object-store backend is wired via the `cloud` feature.
//!   * **Gate 4 (BTE-029)** — `BacktestNode::run()` executes end-to-end over the
//!     catalog and emits a `BacktestResult`. The Rust `BacktestEngineConfig`
//!     carries no strategies (they are added imperatively, not via config), so
//!     this is a strategy-less run: the engine advances the clock through the
//!     catalog data and reports zero orders/positions. Results are pipeline-proof
//!     only — the data is synthetic, with no `SourceProofReport` (BTE-015), so
//!     they carry no market validity.
//!
//! Every runtime value (venue, currencies, increments, balances, the synthetic
//! data points) is bound through the TOML registry
//! `tests/fixtures/bte_market_families.toml` — the BTE-003 "venue/provider
//! selected only through TOML/registry bindings" requirement. This file holds no
//! venue/price/currency literals; the only structural choice it makes is which NT
//! instrument constructor a fixture's `kind` maps to.
//!
//! The two spec fixtures (`binary option`, `perps/spot`) are covered, plus
//! additional NT instrument families enabled for capability/round-trip coverage
//! (some have no bolt strategy yet — proving NT can carry the family, not that we
//! trade it). 10 families across 9 distinct NT instrument types:
//!
//! | Family | NT instrument | Venue | Account |
//! |--------|---------------|-------|---------|
//! | binary-option | `BinaryOption` | POLYMARKET | Cash |
//! | cex-spot | `CurrencyPair` | BINANCE | Cash |
//! | cex-perp | `CryptoPerpetual` | BINANCE | Margin |
//! | perp-dex | `CryptoPerpetual` | HYPERLIQUID | Margin |
//! | equity-perp | `PerpetualContract` | REPRESENTATIVE | Margin |
//! | betting-betfair | `BettingInstrument` | BETFAIR | Cash |
//! | crypto-future | `CryptoFuture` | DERIBIT | Margin |
//! | crypto-option | `CryptoOption` | DERIBIT | Margin |
//! | crypto-futures-spread | `CryptoFuturesSpread` | DERIBIT | Margin |
//! | crypto-option-spread | `CryptoOptionSpread` | DERIBIT | Margin |
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
//! Out of scope for this gate (tracked under later #438 / #439 tasks): the
//! `SourceProofReport` / Artifact Index contracts (BTE-005/015), a strategy-driven
//! run with fills, and a live-bucket S3 round-trip.
#![cfg(feature = "bte-gate-proof")]

use std::str::FromStr;

use nautilus_backtest::config::{
    BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType,
};
use nautilus_backtest::node::BacktestNode;
use nautilus_core::UnixNanos;
use nautilus_model::data::{BookOrder, OrderBookDelta, TradeTick};
use nautilus_model::enums::{
    AccountType, AggressorSide, AssetClass, BookAction, BookType, OmsType, OptionKind, OrderSide,
};
use nautilus_model::identifiers::{InstrumentId, Symbol, TradeId};
use nautilus_model::instruments::{
    BettingInstrument, BinaryOption, CryptoFuture, CryptoFuturesSpread, CryptoOption,
    CryptoOptionSpread, CryptoPerpetual, CurrencyPair, Instrument, InstrumentAny,
    PerpetualContract,
};
use nautilus_model::types::{Currency, Price, Quantity};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde::Deserialize;
use ustr::Ustr;

/// The fixture registry — single source of truth for every runtime value the
/// proof feeds NT. Embedded at compile time; the values live only in the TOML.
const FIXTURES_TOML: &str = include_str!("fixtures/bte_market_families.toml");

#[derive(Debug, Deserialize)]
struct FixtureRegistry {
    /// Gate-2 S3 interface target (synthetic bucket).
    s3_proof_uri: String,
    family: Vec<MarketFamily>,
}

/// One market-family fixture binding. `kind` selects the NT instrument
/// constructor; every other field is a pure data parameter — no venue, price,
/// or currency value is decided in Rust.
#[derive(Debug, Deserialize)]
struct MarketFamily {
    label: String,
    kind: String,
    instrument_id: String,
    symbol: String,
    base_currency: Option<String>,
    quote_currency: String,
    settlement_currency: Option<String>,
    asset_class: Option<String>,
    is_inverse: Option<bool>,
    /// Underlying for a generic `PerpetualContract` (a Ustr identifier, e.g.
    /// `NVDA`/`EURUSD`) or for the crypto derivatives (a currency code, e.g.
    /// `BTC`) — interpreted per `kind`.
    underlying: Option<String>,
    /// Option type (`call`/`put`) for `crypto_option`.
    option_kind: Option<String>,
    /// Strike price for `crypto_option`.
    strike_price: Option<String>,
    /// Spread strategy type (e.g. `CALENDAR`, `VERTICAL`) for the spread kinds.
    strategy_type: Option<String>,
    price_increment: String,
    size_increment: String,
    activation_ns: Option<u64>,
    expiration_ns: Option<u64>,
    outcome: Option<String>,
    /// Sports-betting taxonomy, present only for `kind = "betting"`.
    betting: Option<BettingSpec>,
    venue: String,
    account_type: String,
    venue_base_currency: Option<String>,
    starting_balance: String,
    trade_price: String,
    trade_size: String,
    book_bid: String,
    book_ask: String,
}

/// Betfair-style market/selection taxonomy for a `BettingInstrument`.
#[derive(Debug, Deserialize)]
struct BettingSpec {
    event_type_id: u64,
    event_type_name: String,
    competition_id: u64,
    competition_name: String,
    event_id: u64,
    event_name: String,
    event_country_code: String,
    event_open_date_ns: u64,
    betting_type: String,
    market_id: String,
    market_name: String,
    market_type: String,
    market_start_time_ns: u64,
    selection_id: u64,
    selection_name: String,
    selection_handicap: f64,
}

fn registry() -> FixtureRegistry {
    toml::from_str(FIXTURES_TOML).expect("parse tests/fixtures/bte_market_families.toml")
}

fn market_family(label: &str) -> MarketFamily {
    registry()
        .family
        .into_iter()
        .find(|f| f.label == label)
        .unwrap_or_else(|| panic!("no market family `{label}` in fixtures TOML"))
}

/// Resolve an NT [`Currency`] from its registered code (e.g. `USDC`).
fn currency(code: &str) -> Currency {
    Currency::from_str(code).unwrap_or_else(|e| panic!("unknown currency `{code}`: {e}"))
}

fn account_type(s: &str) -> AccountType {
    match s {
        "cash" => AccountType::Cash,
        "margin" => AccountType::Margin,
        other => panic!("unsupported account_type `{other}`"),
    }
}

fn option_kind(s: &str) -> OptionKind {
    match s {
        "call" => OptionKind::Call,
        "put" => OptionKind::Put,
        other => panic!("unsupported option_kind `{other}`"),
    }
}

fn asset_class(s: &str) -> AssetClass {
    match s {
        "fx" => AssetClass::FX,
        "equity" => AssetClass::Equity,
        "commodity" => AssetClass::Commodity,
        "debt" => AssetClass::Debt,
        "index" => AssetClass::Index,
        "cryptocurrency" => AssetClass::Cryptocurrency,
        "alternative" => AssetClass::Alternative,
        other => panic!("unsupported asset_class `{other}`"),
    }
}

/// Build the NT instrument for a family. The `kind` discriminant is the only
/// structural choice the Rust makes — all field values come from the TOML.
fn build_instrument(f: &MarketFamily) -> InstrumentAny {
    let id = InstrumentId::from_str(&f.instrument_id).unwrap();
    let symbol = Symbol::new(&f.symbol);
    let price_increment = Price::from(f.price_increment.as_str());
    let size_increment = Quantity::from(f.size_increment.as_str());
    let base = || currency(f.base_currency.as_deref().expect("base_currency required"));

    match f.kind.as_str() {
        "binary_option" => BinaryOption::new(
            id,
            symbol,
            asset_class(
                f.asset_class
                    .as_deref()
                    .expect("binary_option needs asset_class"),
            ),
            currency(&f.quote_currency),
            UnixNanos::from(f.activation_ns.expect("binary_option needs activation_ns")),
            UnixNanos::from(f.expiration_ns.expect("binary_option needs expiration_ns")),
            price_increment.precision,
            size_increment.precision,
            price_increment,
            size_increment,
            f.outcome.as_deref().map(Ustr::from),
            None, // description
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
        .into_any(),
        "currency_pair" => CurrencyPair::new(
            id,
            symbol,
            base(),
            currency(&f.quote_currency),
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
        .into_any(),
        "crypto_perpetual" => CryptoPerpetual::new(
            id,
            symbol,
            base(),
            currency(&f.quote_currency),
            currency(
                f.settlement_currency
                    .as_deref()
                    .expect("crypto_perpetual needs settlement_currency"),
            ),
            f.is_inverse.expect("crypto_perpetual needs is_inverse"),
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
        .into_any(),
        "perpetual_contract" => PerpetualContract::new(
            id,
            symbol,
            Ustr::from(
                f.underlying
                    .as_deref()
                    .expect("perpetual_contract needs underlying"),
            ),
            asset_class(
                f.asset_class
                    .as_deref()
                    .expect("perpetual_contract needs asset_class"),
            ),
            f.base_currency.as_deref().map(currency), // optional (set for FX/crypto underlyings)
            currency(&f.quote_currency),
            currency(
                f.settlement_currency
                    .as_deref()
                    .expect("perpetual_contract needs settlement_currency"),
            ),
            f.is_inverse.expect("perpetual_contract needs is_inverse"),
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
        .into_any(),
        "betting" => {
            let b = f
                .betting
                .as_ref()
                .expect("betting needs a [family.betting] table");
            BettingInstrument::new(
                id,
                symbol,
                b.event_type_id,
                Ustr::from(b.event_type_name.as_str()),
                b.competition_id,
                Ustr::from(b.competition_name.as_str()),
                b.event_id,
                Ustr::from(b.event_name.as_str()),
                Ustr::from(b.event_country_code.as_str()),
                UnixNanos::from(b.event_open_date_ns),
                Ustr::from(b.betting_type.as_str()),
                Ustr::from(b.market_id.as_str()),
                Ustr::from(b.market_name.as_str()),
                Ustr::from(b.market_type.as_str()),
                UnixNanos::from(b.market_start_time_ns),
                b.selection_id,
                Ustr::from(b.selection_name.as_str()),
                b.selection_handicap,
                currency(&f.quote_currency),
                price_increment.precision,
                size_increment.precision,
                price_increment,
                size_increment,
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
        "crypto_future" => CryptoFuture::new(
            id,
            symbol,
            currency(
                f.underlying
                    .as_deref()
                    .expect("crypto_future needs underlying"),
            ),
            currency(&f.quote_currency),
            currency(
                f.settlement_currency
                    .as_deref()
                    .expect("crypto_future needs settlement_currency"),
            ),
            f.is_inverse.expect("crypto_future needs is_inverse"),
            UnixNanos::from(f.activation_ns.expect("crypto_future needs activation_ns")),
            UnixNanos::from(f.expiration_ns.expect("crypto_future needs expiration_ns")),
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
        .into_any(),
        "crypto_option" => CryptoOption::new(
            id,
            symbol,
            currency(
                f.underlying
                    .as_deref()
                    .expect("crypto_option needs underlying"),
            ),
            currency(&f.quote_currency),
            currency(
                f.settlement_currency
                    .as_deref()
                    .expect("crypto_option needs settlement_currency"),
            ),
            f.is_inverse.expect("crypto_option needs is_inverse"),
            option_kind(
                f.option_kind
                    .as_deref()
                    .expect("crypto_option needs option_kind"),
            ),
            Price::from(
                f.strike_price
                    .as_deref()
                    .expect("crypto_option needs strike_price"),
            ),
            UnixNanos::from(f.activation_ns.expect("crypto_option needs activation_ns")),
            UnixNanos::from(f.expiration_ns.expect("crypto_option needs expiration_ns")),
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
        .into_any(),
        "crypto_futures_spread" => CryptoFuturesSpread::new(
            id,
            symbol,
            currency(
                f.underlying
                    .as_deref()
                    .expect("crypto_futures_spread needs underlying"),
            ),
            currency(&f.quote_currency),
            currency(
                f.settlement_currency
                    .as_deref()
                    .expect("crypto_futures_spread needs settlement_currency"),
            ),
            f.is_inverse
                .expect("crypto_futures_spread needs is_inverse"),
            Ustr::from(
                f.strategy_type
                    .as_deref()
                    .expect("crypto_futures_spread needs strategy_type"),
            ),
            UnixNanos::from(
                f.activation_ns
                    .expect("crypto_futures_spread needs activation_ns"),
            ),
            UnixNanos::from(
                f.expiration_ns
                    .expect("crypto_futures_spread needs expiration_ns"),
            ),
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
        .into_any(),
        "crypto_option_spread" => CryptoOptionSpread::new(
            id,
            symbol,
            currency(
                f.underlying
                    .as_deref()
                    .expect("crypto_option_spread needs underlying"),
            ),
            currency(&f.quote_currency),
            currency(
                f.settlement_currency
                    .as_deref()
                    .expect("crypto_option_spread needs settlement_currency"),
            ),
            f.is_inverse.expect("crypto_option_spread needs is_inverse"),
            Ustr::from(
                f.strategy_type
                    .as_deref()
                    .expect("crypto_option_spread needs strategy_type"),
            ),
            UnixNanos::from(
                f.activation_ns
                    .expect("crypto_option_spread needs activation_ns"),
            ),
            UnixNanos::from(
                f.expiration_ns
                    .expect("crypto_option_spread needs expiration_ns"),
            ),
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
        .into_any(),
        other => panic!("unsupported instrument kind `{other}`"),
    }
}

/// Drives all gates for one market family: write the instrument + trades + book
/// deltas to a temp `ParquetDataCatalog`, read them back identical (Gate 2
/// local), build a catalog-backed `BacktestNode` (Gate 1), then run it
/// end-to-end and assert the result shape (Gate 4). The venue is an `L2_MBP`
/// CLOB venue, which `BacktestNode` requires to have order-book data both at
/// construction (`node.rs:341-368`) and at run time, so the proof replays a real
/// book, not just trades.
fn prove_market_family(f: MarketFamily) {
    let label = f.label.as_str();
    let instrument = build_instrument(&f);
    let venue = f.venue.as_str();
    let account = account_type(&f.account_type);
    let venue_base_currency = f.venue_base_currency.as_deref().map(currency);
    let starting_balance = f.starting_balance.as_str();
    let trade_price = f.trade_price.as_str();
    let trade_size = f.trade_size.as_str();

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

    // Seed the L2 book: one bid Add + one ask Add. Without book data an `L2_MBP`
    // venue refuses to run, so this is the realistic L2_REPLAY data shape.
    let deltas: Vec<OrderBookDelta> = [
        (OrderSide::Buy, f.book_bid.as_str(), 1u64),
        (OrderSide::Sell, f.book_ask.as_str(), 2u64),
    ]
    .into_iter()
    .enumerate()
    .map(|(seq, (side, px, order_id))| {
        OrderBookDelta::new(
            id,
            BookAction::Add,
            BookOrder::new(side, Price::from(px), Quantity::from(trade_size), order_id),
            0, // flags
            seq as u64,
            UnixNanos::from(1),
            UnixNanos::from(1),
        )
    })
    .collect();
    catalog
        .write_to_parquet(deltas.clone(), None, None, None)
        .unwrap_or_else(|e| panic!("{label}: write book deltas: {e}"));

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
    assert_eq!(
        read_trades.len(),
        trades.len(),
        "{label}: all trades round-trip"
    );
    assert_eq!(
        read_trades, trades,
        "{label}: trade payloads are identical after the parquet round-trip"
    );
    let read_deltas: Vec<OrderBookDelta> = catalog
        .query_typed_data::<OrderBookDelta>(None, None, None, None, None, true)
        .unwrap_or_else(|e| panic!("{label}: read book deltas: {e}"));
    assert_eq!(
        read_deltas.len(),
        deltas.len(),
        "{label}: both book deltas round-trip"
    );
    assert_eq!(
        read_deltas, deltas,
        "{label}: book delta payloads are identical after the parquet round-trip"
    );

    // Gate 1: construct a catalog-backed BacktestNode.
    let catalog_path = tmp.path().to_string_lossy().to_string();
    let venue_cfg = BacktestVenueConfig::builder()
        .name(Ustr::from(venue))
        .oms_type(OmsType::Netting)
        .account_type(account)
        .book_type(BookType::L2_MBP)
        .starting_balances(vec![starting_balance.to_string()])
        .maybe_base_currency(venue_base_currency)
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

    let mut node = BacktestNode::new(vec![run_config])
        .unwrap_or_else(|e| panic!("{label}: BacktestNode construct: {e}"));
    assert_eq!(
        node.configs().len(),
        1,
        "{label}: exactly one run config (kernel MessageBus is a thread-local singleton)"
    );

    // Gate 4 (BTE-029): run the engine end-to-end over the catalog. Strategy-less,
    // so every catalog data point is iterated but no orders/positions are emitted.
    // The results are pipeline-proof only — synthetic data, no SourceProofReport.
    let results = node
        .run()
        .unwrap_or_else(|e| panic!("{label}: BacktestNode run: {e}"));
    assert_eq!(results.len(), 1, "{label}: one result per run config");
    let result = &results[0];
    let expected_iterations = trades.len() + deltas.len();
    assert_eq!(
        result.iterations, expected_iterations,
        "{label}: engine iterated every catalog data point (got {}, want {expected_iterations})",
        result.iterations
    );
    assert_eq!(
        result.total_orders, 0,
        "{label}: strategy-less run emits no orders"
    );
    assert_eq!(
        result.total_positions, 0,
        "{label}: strategy-less run opens no positions"
    );
    assert!(result.run_id.is_some(), "{label}: the run records a run id");
    assert!(
        result.backtest_start.is_some() && result.backtest_end.is_some(),
        "{label}: the run records a backtest time range"
    );
}

/// Fixture `binary option` — Polymarket-shaped `BinaryOption` on a Cash account.
#[test]
fn binary_option_polymarket() {
    prove_market_family(market_family("binary-option"));
}

/// Fixture `perps/spot` — CEX spot `CurrencyPair` (Binance BTCUSDT, Cash account).
#[test]
fn perps_spot_cex_spot_binance() {
    prove_market_family(market_family("cex-spot"));
}

/// Fixture `perps/spot` — CEX perp `CryptoPerpetual` (Binance USD-M, Margin).
#[test]
fn perps_spot_cex_perp_binance() {
    prove_market_family(market_family("cex-perp"));
}

/// Fixture `perps/spot` — perp DEX `CryptoPerpetual` (Hyperliquid, USDC-settled).
#[test]
fn perps_spot_perp_dex_hyperliquid() {
    prove_market_family(market_family("perp-dex"));
}

/// Generic `PerpetualContract` — an equity perpetual (non-crypto underlying), the
/// case `CryptoPerpetual` cannot express. Margin account.
#[test]
fn perpetual_contract_equity_perp() {
    prove_market_family(market_family("equity-perp"));
}

/// `BettingInstrument` — a Betfair-style sports market. Modeled, round-tripped,
/// and run end-to-end for capability coverage; no bolt strategy supports betting
/// yet, so this proves NT can carry the family, not that we trade it.
#[test]
fn betting_betfair_match_odds() {
    prove_market_family(market_family("betting-betfair"));
}

/// `CryptoFuture` — a dated (expiring) crypto future (Deribit/Binance-style).
#[test]
fn crypto_future_dated() {
    prove_market_family(market_family("crypto-future"));
}

/// `CryptoOption` — a crypto option (Deribit-style BTC call).
#[test]
fn crypto_option_btc_call() {
    prove_market_family(market_family("crypto-option"));
}

/// `CryptoFuturesSpread` — a calendar spread on crypto futures.
#[test]
fn crypto_futures_spread_calendar() {
    prove_market_family(market_family("crypto-futures-spread"));
}

/// `CryptoOptionSpread` — a vertical spread on crypto options.
#[test]
fn crypto_option_spread_vertical() {
    prove_market_family(market_family("crypto-option-spread"));
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
    // Synthetic fixture URI, bound from the TOML registry — never contacted.
    let uri = registry().s3_proof_uri;
    let result = ParquetDataCatalog::from_uri(&uri, None, None, None, None);
    assert!(
        result.is_ok(),
        "s3:// must construct the cloud object-store backend (lazy, no network) \
         with the `cloud` feature on, not hit the cloud-feature bail; got: {:?}",
        result.err()
    );
}
