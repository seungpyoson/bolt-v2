use nautilus_binance::{
    common::parse::parse_spot_instrument_sbe,
    spot::{
        http::models::{
            BinanceLotSizeFilterSbe, BinancePriceFilterSbe, BinanceSymbolFiltersSbe,
            BinanceSymbolSbe,
        },
        sbe::stream::BestBidAskStreamEvent,
        websocket::streams::parse::parse_bbo_event,
    },
};
use nautilus_core::UnixNanos;
use ustr::Ustr;

#[test]
fn sbe_bbo_preserves_unequal_event_and_adapter_initialization_stamps() {
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
    let instrument = parse_spot_instrument_sbe(&symbol, instrument_stamp, instrument_stamp)
        .expect("public Binance SBE instrument parser must construct BTCUSDT");
    let event_time_us = 1_700_000_000_000_000_i64;
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

    let quote = parse_bbo_event(&event, &instrument, adapter_ts_init);

    assert_ne!(
        UnixNanos::from_micros(event_time_us as u64),
        adapter_ts_init
    );
    assert_eq!(quote.ts_event, UnixNanos::from_micros(event_time_us as u64));
    assert_eq!(quote.ts_init, adapter_ts_init);
}
