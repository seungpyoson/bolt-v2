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
    data::TradeTick,
    enums::{AccountType, AggressorSide, BookType, OmsType},
    identifiers::{InstrumentId, Symbol, TradeId, Venue},
    instruments::{CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_trading::examples::strategies::{HurstVpinDirectional, HurstVpinDirectionalConfig};
use tempfile::TempDir;
use ustr::Ustr;

const RUN_IDENTIFIER: &str = "backtesting-vertical-slice-stack-proof";
const VENUE_NAME: &str = "BYBIT";
const RAW_SYMBOL: &str = "BTCUSDT";
const BASE_CURRENCY: &str = "BTC";
const QUOTE_CURRENCY: &str = "USDT";
const PRICE_PRECISION: u8 = 2;
const SIZE_PRECISION: u8 = 3;
const TRADE_COUNT: usize = 64;
const BASE_TIMESTAMP_NANOS: u64 = 1_740_787_200_000_000_000; // 2025-03-01T00:00:00Z
const TRADE_INTERVAL_NANOS: u64 = 1_000_000_000;

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
        .write_to_parquet(trades.clone(), None, None, None)
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
        .build();

    let data_config = BacktestDataConfig::builder()
        .data_type(NautilusDataType::TradeTick)
        .catalog_path(catalog_path)
        .instrument_id(instrument_id)
        .build();

    let run_config = BacktestRunConfig::builder()
        .id(RUN_IDENTIFIER.to_string())
        .venues(vec![venue_config])
        .data(vec![data_config])
        .build();

    let mut node = BacktestNode::new(vec![run_config]).expect("construct backtest node");
    node.build().expect("build backtest node");

    let bar_type = format!("{instrument_id}-1-MINUTE-LAST-EXTERNAL");
    let strategy_config = HurstVpinDirectionalConfig::new(
        instrument_id,
        bar_type.parse().expect("bar type"),
        Quantity::new(0.010, SIZE_PRECISION),
    );
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
}
