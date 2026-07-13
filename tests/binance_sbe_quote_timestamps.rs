use ::nautilus_binance::spot::websocket::streams::parse as nt_binance_sbe_parse;
use nautilus_binance::{
    common::parse::parse_spot_instrument_sbe,
    spot::{
        http::models::{
            BinanceLotSizeFilterSbe, BinancePriceFilterSbe, BinanceSymbolFiltersSbe,
            BinanceSymbolSbe,
        },
        sbe::stream::{
            BestBidAskStreamEvent, DepthDiffStreamEvent, DepthSnapshotStreamEvent, PriceLevel,
            Trade, TradesStreamEvent,
        },
    },
};
use nautilus_core::UnixNanos;
use nautilus_model::{data::Data, instruments::InstrumentAny};
use ustr::Ustr;

fn sample_instrument() -> InstrumentAny {
    let symbol = BinanceSymbolSbe {
        symbol: "BTCUSDT".to_string(),
        base_asset: "BTC".to_string(),
        quote_asset: "USDT".to_string(),
        base_asset_precision: 8,
        quote_asset_precision: 8,
        status: 0,
        order_types: 0,
        iceberg_allowed: true,
        oco_allowed: true,
        oto_allowed: false,
        quote_order_qty_market_allowed: true,
        allow_trailing_stop: true,
        cancel_replace_allowed: true,
        amend_allowed: true,
        is_spot_trading_allowed: true,
        is_margin_trading_allowed: false,
        filters: BinanceSymbolFiltersSbe {
            price_filter: Some(BinancePriceFilterSbe {
                price_exponent: -2,
                min_price: 1,
                max_price: 100_000_000,
                tick_size: 1,
            }),
            lot_size_filter: Some(BinanceLotSizeFilterSbe {
                qty_exponent: -4,
                min_qty: 1,
                max_qty: 100_000_000,
                step_size: 1,
            }),
        },
        permissions: vec![vec!["SPOT".to_string()]],
    };
    let instrument_stamp = UnixNanos::from(1_600_000_000_000_000_000_u64);
    parse_spot_instrument_sbe(&symbol, instrument_stamp, instrument_stamp)
        .expect("public Binance SBE instrument parser must construct BTCUSDT")
}

#[test]
fn sbe_multi_trade_preserves_unequal_event_and_adapter_initialization_stamps() {
    let instrument = sample_instrument();
    let transact_time_us = 1_700_000_000_100_000_i64;
    let expected_ts_event = UnixNanos::from_micros(transact_time_us as u64);
    let adapter_ts_init = UnixNanos::from(1_800_000_000_000_000_000_u64);
    let event = TradesStreamEvent {
        event_time_us: 1_700_000_000_000_000_i64,
        transact_time_us,
        price_exponent: -2,
        qty_exponent: -4,
        trades: vec![
            Trade {
                id: 1,
                price_mantissa: 12_345,
                qty_mantissa: 25_000,
                is_buyer_maker: false,
            },
            Trade {
                id: 2,
                price_mantissa: 12_340,
                qty_mantissa: 10_000,
                is_buyer_maker: true,
            },
        ],
        symbol: Ustr::from("BTCUSDT"),
    };

    let trades = nt_binance_sbe_parse::parse_trades_event(&event, &instrument, adapter_ts_init);

    ::core::assert_ne!(expected_ts_event, adapter_ts_init);
    ::core::assert_eq!(trades.len(), 2);
    for data in trades {
        let Data::Trade(trade) = data else {
            panic!("expected every parsed item to be trade data");
        };
        ::core::assert_eq!(trade.ts_event, expected_ts_event);
        ::core::assert_eq!(trade.ts_init, adapter_ts_init);
    }
}

#[test]
fn sbe_bbo_preserves_unequal_event_and_adapter_initialization_stamps() {
    let instrument = sample_instrument();
    let event_time_us = 1_700_000_000_000_000_i64;
    let expected_ts_event = UnixNanos::from_micros(event_time_us as u64);
    let adapter_ts_init = UnixNanos::from(1_800_000_000_000_000_000_u64);
    let event = BestBidAskStreamEvent {
        event_time_us,
        book_update_id: 123,
        price_exponent: -2,
        qty_exponent: -4,
        bid_price_mantissa: 12_345,
        bid_qty_mantissa: 25_000,
        ask_price_mantissa: 12_350,
        ask_qty_mantissa: 30_000,
        symbol: Ustr::from("BTCUSDT"),
    };

    let quote = nt_binance_sbe_parse::parse_bbo_event(&event, &instrument, adapter_ts_init);

    ::core::assert_ne!(expected_ts_event, adapter_ts_init);
    ::core::assert_eq!(quote.ts_event, expected_ts_event);
    ::core::assert_eq!(quote.ts_init, adapter_ts_init);
}

#[test]
fn sbe_depth_snapshot_preserves_unequal_event_and_adapter_initialization_stamps() {
    let instrument = sample_instrument();
    let event_time_us = 1_700_000_000_000_000_i64;
    let expected_ts_event = UnixNanos::from_micros(event_time_us as u64);
    let adapter_ts_init = UnixNanos::from(1_800_000_000_000_000_000_u64);
    let event = DepthSnapshotStreamEvent {
        event_time_us,
        book_update_id: 123,
        price_exponent: -2,
        qty_exponent: -4,
        bids: vec![PriceLevel {
            price_mantissa: 12_345,
            qty_mantissa: 25_000,
        }],
        asks: vec![PriceLevel {
            price_mantissa: 12_350,
            qty_mantissa: 30_000,
        }],
        symbol: Ustr::from("BTCUSDT"),
    };

    let deltas = nt_binance_sbe_parse::parse_depth_snapshot(&event, &instrument, adapter_ts_init)
        .expect("non-empty SBE depth snapshot must produce deltas");

    ::core::assert_ne!(expected_ts_event, adapter_ts_init);
    ::core::assert_eq!(deltas.deltas.len(), 3);
    ::core::assert_eq!(deltas.ts_event, expected_ts_event);
    ::core::assert_eq!(deltas.ts_init, adapter_ts_init);
    ::core::assert!(
        deltas
            .deltas
            .iter()
            .all(|delta| delta.ts_event == expected_ts_event)
    );
    ::core::assert!(
        deltas
            .deltas
            .iter()
            .all(|delta| delta.ts_init == adapter_ts_init)
    );
}

#[test]
fn sbe_depth_diff_preserves_unequal_event_and_adapter_initialization_stamps() {
    let instrument = sample_instrument();
    let event_time_us = 1_700_000_000_000_000_i64;
    let expected_ts_event = UnixNanos::from_micros(event_time_us as u64);
    let adapter_ts_init = UnixNanos::from(1_800_000_000_000_000_000_u64);
    let event = DepthDiffStreamEvent {
        event_time_us,
        first_book_update_id: 100,
        last_book_update_id: 101,
        price_exponent: -2,
        qty_exponent: -4,
        bids: vec![
            PriceLevel {
                price_mantissa: 12_345,
                qty_mantissa: 25_000,
            },
            PriceLevel {
                price_mantissa: 12_340,
                qty_mantissa: 0,
            },
        ],
        asks: vec![PriceLevel {
            price_mantissa: 12_350,
            qty_mantissa: 30_000,
        }],
        symbol: Ustr::from("BTCUSDT"),
    };

    let deltas = nt_binance_sbe_parse::parse_depth_diff(&event, &instrument, adapter_ts_init)
        .expect("non-empty SBE depth diff must produce deltas");

    ::core::assert_ne!(expected_ts_event, adapter_ts_init);
    ::core::assert_eq!(deltas.deltas.len(), 3);
    ::core::assert_eq!(deltas.ts_event, expected_ts_event);
    ::core::assert_eq!(deltas.ts_init, adapter_ts_init);
    ::core::assert!(
        deltas
            .deltas
            .iter()
            .all(|delta| delta.ts_event == expected_ts_event)
    );
    ::core::assert!(
        deltas
            .deltas
            .iter()
            .all(|delta| delta.ts_init == adapter_ts_init)
    );
}
