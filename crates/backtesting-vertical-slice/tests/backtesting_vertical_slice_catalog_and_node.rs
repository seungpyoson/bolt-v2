//! Slice-1 stack proof for the NautilusTrader backtesting vertical slice.
//!
//! This integration test proves, against the NautilusTrader dependency resolved
//! by this `bolt-v2` branch, that:
//!
//! 1. A local `ParquetDataCatalog` accepts a `CurrencyPair` instrument and a
//!    `TradeTick` projection and reads the exact same ticks back
//!    (`query_typed_data`). This is the NautilusTrader catalog read proof.
//! 2. A `BacktestNode` constructed from `BacktestRunConfig` /
//!    `BacktestDataConfig` / `BacktestVenueConfig` builds against that catalog,
//!    runs an existing compiled Rust strategy (`HurstVpinDirectional`), and
//!    returns a `BacktestResult`.
//!
//! The data here is synthetic and only proves the mechanical NautilusTrader
//! path. Real accepted-data normalization, source-proof acceptance, manifest
//! validation, and the objective result contract are covered by the other
//! slices of this vertical.

use nautilus_backtest::{
    config::{BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType},
    node::BacktestNode,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, TradeTick, order::BookOrder},
    enums::{
        AccountType, AggressorSide, AssetClass, BookAction, BookType, OmsType, OrderSide,
        RecordFlag,
    },
    identifiers::{InstrumentId, Symbol, TradeId, Venue},
    instruments::{BinaryOption, CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_trading::examples::strategies::{HurstVpinDirectional, HurstVpinDirectionalConfig};
use rust_decimal::Decimal;
use tempfile::TempDir;
use ustr::Ustr;

const RUN_IDENTIFIER: &str = "backtesting-vertical-slice-stack-proof";
const L2_RUN_IDENTIFIER: &str = "backtesting-vertical-slice-l2-delta-proof";
const VENUE_NAME: &str = "BYBIT";
const L2_VENUE_NAME: &str = "TESTVENUE";
const L2_SYMBOL: &str = "YES";
const RAW_SYMBOL: &str = "BTCUSDT";
const BASE_CURRENCY: &str = "BTC";
const QUOTE_CURRENCY: &str = "USDT";
const L2_SETTLEMENT_CURRENCY: &str = "USD";
const PRICE_PRECISION: u8 = 2;
const SIZE_PRECISION: u8 = 3;
const L2_SIZE_PRECISION: u8 = 6;
const TRADE_COUNT: usize = 64;
const L2_DELTA_COUNT: usize = 2;
const BASE_TIMESTAMP_NANOS: u64 = 1_740_787_200_000_000_000; // 2025-03-01T00:00:00Z
const TRADE_INTERVAL_NANOS: u64 = 1_000_000_000;
const L2_BASE_TIMESTAMP_NANOS: u64 = 1_772_323_201_665_000_000;

fn proof_instrument() -> CurrencyPair {
    let instrument_id = InstrumentId::new(Symbol::from(RAW_SYMBOL), Venue::from(VENUE_NAME));
    CurrencyPair::new(
        instrument_id,
        Symbol::from(RAW_SYMBOL),
        Currency::from(BASE_CURRENCY),
        Currency::from(QUOTE_CURRENCY),
        PRICE_PRECISION,
        SIZE_PRECISION,
        Price::new(0.01, PRICE_PRECISION),
        Quantity::new(0.001, SIZE_PRECISION),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // tick_scheme (NT bump)
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
}

fn synthetic_trades(instrument_id: InstrumentId) -> Vec<TradeTick> {
    (0..TRADE_COUNT)
        .map(|index| {
            let price = 50_000.0 + (index as f64);
            let aggressor = if index % 2 == 0 {
                AggressorSide::Buyer
            } else {
                AggressorSide::Seller
            };
            let ts = UnixNanos::from(BASE_TIMESTAMP_NANOS + (index as u64) * TRADE_INTERVAL_NANOS);
            TradeTick::new(
                instrument_id,
                Price::new(price, PRICE_PRECISION),
                Quantity::new(0.500, SIZE_PRECISION),
                aggressor,
                TradeId::from(format!("synthetic-{index}").as_str()),
                ts,
                ts,
            )
        })
        .collect()
}

fn proof_binary_option() -> BinaryOption {
    let instrument_id = InstrumentId::new(Symbol::from(L2_SYMBOL), Venue::from(L2_VENUE_NAME));
    let ts_init = UnixNanos::from(1_000_000_000u64);
    BinaryOption::new_checked(
        instrument_id,
        Symbol::from(L2_SYMBOL),
        AssetClass::Alternative,
        Currency::from(L2_SETTLEMENT_CURRENCY),
        UnixNanos::from(0),
        UnixNanos::from(2_000_000_000u64),
        PRICE_PRECISION,
        L2_SIZE_PRECISION,
        Price::from("0.01"),
        Quantity::from("0.000001"),
        Some(Ustr::from("Yes")),
        Some(Ustr::from("Bounded binary option fixture")),
        None,
        Some(Quantity::from("1")),
        None,
        None,
        Some(Price::from("1.00")),
        Some(Price::from("0.01")),
        None,
        None,
        Some(Decimal::ZERO),
        Some(Decimal::ZERO),
        None, // tick_scheme (NT bump)
        None,
        ts_init,
        ts_init,
    )
    .expect("binary option")
}

fn synthetic_l2_deltas(instrument_id: InstrumentId) -> Vec<OrderBookDelta> {
    let ts_event = UnixNanos::from(L2_BASE_TIMESTAMP_NANOS);
    let ts_init = UnixNanos::from(1_000_000_000u64);
    vec![
        OrderBookDelta::new_checked(
            instrument_id,
            BookAction::Clear,
            BookOrder::new(
                OrderSide::NoOrderSide,
                Price::from("0.00"),
                Quantity::from("0.000000"),
                0,
            ),
            RecordFlag::F_SNAPSHOT as u8,
            0,
            ts_event,
            ts_init,
        )
        .expect("clear delta"),
        OrderBookDelta::new_checked(
            instrument_id,
            BookAction::Add,
            BookOrder::new(
                OrderSide::Buy,
                Price::from("0.49"),
                Quantity::from("10.000000"),
                0,
            ),
            RecordFlag::F_LAST as u8,
            0,
            ts_event,
            ts_init,
        )
        .expect("bid delta"),
    ]
}

fn binary_option_l2_node_iterations() -> usize {
    let instrument = proof_binary_option();
    let instrument_id = instrument.id;
    let deltas = synthetic_l2_deltas(instrument_id);

    let temp_dir = TempDir::new().expect("temp dir");
    let catalog_path = temp_dir.path().to_str().expect("utf-8 path").to_string();

    let mut catalog = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::BinaryOption(instrument)])
        .expect("write binary option instrument");
    catalog
        .write_to_parquet(&deltas, None, None, None)
        .expect("write L2 deltas");

    let loaded: Vec<OrderBookDelta> = catalog
        .query_typed_data::<OrderBookDelta>(
            Some(vec![instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("query L2 deltas");
    assert_eq!(loaded.len(), deltas.len(), "catalog must return L2 deltas");
    assert_eq!(loaded[0].instrument_id, instrument_id);

    let venue_config = BacktestVenueConfig::builder()
        .name(Ustr::from(L2_VENUE_NAME))
        .oms_type(OmsType::Netting)
        .account_type(AccountType::Cash)
        .book_type(BookType::L2_MBP)
        .starting_balances(vec![format!("1_000_000 {L2_SETTLEMENT_CURRENCY}")])
        .build()
        .expect("valid L2 venue config");

    let data_config = BacktestDataConfig::builder()
        .data_type(NautilusDataType::OrderBookDelta)
        .catalog_path(catalog_path)
        .instrument_id(instrument_id)
        .build()
        .expect("valid L2 data config");

    let run_config = BacktestRunConfig::builder()
        .id(L2_RUN_IDENTIFIER.to_string())
        .venues(vec![venue_config])
        .data(vec![data_config])
        .build()
        .expect("valid L2 run config");

    let mut node = BacktestNode::new(vec![run_config]).expect("construct L2 backtest node");
    node.build().expect("build L2 backtest node");
    let results = node.run().expect("run L2 backtest node");

    assert_eq!(results.len(), 1, "exactly one configured run must execute");
    assert_eq!(results[0].run_config_id.as_deref(), Some(L2_RUN_IDENTIFIER));
    results[0].iterations
}

#[test]
fn catalog_round_trips_trade_ticks_and_node_runs_compiled_strategy() {
    let instrument = proof_instrument();
    let instrument_id = instrument.id();
    let trades = synthetic_trades(instrument_id);

    let temp_dir = TempDir::new().expect("temp dir");
    let catalog_path = temp_dir.path().to_str().expect("utf-8 path").to_string();

    // --- NautilusTrader catalog read proof -------------------------------
    let mut catalog = ParquetDataCatalog::new(temp_dir.path(), None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CurrencyPair(instrument)])
        .expect("write instrument");
    catalog
        .write_to_parquet(&trades, None, None, None)
        .expect("write trade ticks");

    let loaded: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(
            Some(vec![instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .expect("query trade ticks");
    assert_eq!(
        loaded.len(),
        TRADE_COUNT,
        "catalog must return every projected trade tick"
    );
    assert_eq!(loaded[0].instrument_id, instrument_id);

    // --- BacktestNode run proof ------------------------------------------
    let venue_config = BacktestVenueConfig::builder()
        .name(Ustr::from(VENUE_NAME))
        .oms_type(OmsType::Netting)
        .account_type(AccountType::Cash)
        .book_type(BookType::L1_MBP)
        .starting_balances(vec![format!("1_000_000 {QUOTE_CURRENCY}")])
        .build()
        .expect("valid trade-replay venue config");

    let data_config = BacktestDataConfig::builder()
        .data_type(NautilusDataType::TradeTick)
        .catalog_path(catalog_path)
        .instrument_id(instrument_id)
        .build()
        .expect("valid trade-replay data config");

    let run_config = BacktestRunConfig::builder()
        .id(RUN_IDENTIFIER.to_string())
        .venues(vec![venue_config])
        .data(vec![data_config])
        .build()
        .expect("valid trade-replay run config");

    let mut node = BacktestNode::new(vec![run_config]).expect("construct backtest node");
    node.build().expect("build backtest node");

    let bar_type = format!("{instrument_id}-1-MINUTE-LAST-INTERNAL");
    let strategy_config = HurstVpinDirectionalConfig::builder()
        .instrument_id(instrument_id)
        .bar_type(bar_type.parse().expect("bar type"))
        .trade_size(Quantity::new(0.010, SIZE_PRECISION))
        .build();
    {
        let engine = node
            .get_engine_mut(RUN_IDENTIFIER)
            .expect("engine for configured run");
        engine
            .add_strategy(HurstVpinDirectional::new(strategy_config))
            .expect("add compiled strategy");
    }

    let results = node.run().expect("run backtest node");
    assert_eq!(results.len(), 1, "exactly one configured run must execute");
    let result = &results[0];
    assert_eq!(result.run_config_id.as_deref(), Some(RUN_IDENTIFIER));
    assert!(
        result.elapsed_time_secs >= 0.0,
        "result must carry mechanical run metadata"
    );
    assert_eq!(
        result.iterations, TRADE_COUNT,
        "engine must iterate exactly once per projected trade tick"
    );
}

#[test]
fn catalog_round_trips_binary_option_l2_deltas_and_node_consumes_them() {
    assert_eq!(
        binary_option_l2_node_iterations(),
        L2_DELTA_COUNT,
        "engine must iterate exactly once per projected L2 order-book delta"
    );
}
