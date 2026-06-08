//! Compile-checked proof that BTE can use NautilusTrader Polymarket surfaces.
//!
//! PMXT remains a one-off historical bootstrap source. The durable projection
//! boundary should reuse NT's Polymarket provider and websocket parsers rather
//! than rebuilding instrument/token lookup or market data parsing locally.

use std::{any::type_name, collections::HashMap};

use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDeltas, QuoteTick, TradeTick},
    identifiers::InstrumentId,
};
use nautilus_polymarket::{
    providers::{PolymarketInstrumentProvider, build_gamma_params_from_hashmap},
    websocket::{
        messages::{PolymarketBookSnapshot, PolymarketQuote, PolymarketQuotes, PolymarketTrade},
        parse::{
            parse_book_deltas, parse_book_snapshot, parse_quote_from_price_change, parse_trade_tick,
        },
    },
};

type GammaQueryBuilder =
    fn(&HashMap<String, String>) -> nautilus_polymarket::http::query::GetGammaMarketsParams;
type BookSnapshotParser =
    fn(&PolymarketBookSnapshot, InstrumentId, u8, u8, UnixNanos) -> anyhow::Result<OrderBookDeltas>;
type BookDeltaParser =
    fn(&PolymarketQuotes, InstrumentId, u8, u8, UnixNanos) -> anyhow::Result<OrderBookDeltas>;
type TradeParser =
    fn(&PolymarketTrade, InstrumentId, u8, u8, UnixNanos) -> anyhow::Result<TradeTick>;
type QuoteParser = fn(
    &PolymarketQuote,
    InstrumentId,
    u8,
    u8,
    Option<&QuoteTick>,
    UnixNanos,
    UnixNanos,
) -> anyhow::Result<Option<QuoteTick>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolymarketNtSurface {
    InstrumentProviderTokenMap,
    GammaQueryBuilder,
    WebsocketBookSnapshotParser,
    WebsocketBookDeltaParser,
    WebsocketTradeParser,
    WebsocketQuoteParser,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketNtSurfaceProof {
    pub provider_type: String,
    pub surfaces: Vec<PolymarketNtSurface>,
}

#[must_use]
pub fn prove_polymarket_nt_public_surfaces() -> PolymarketNtSurfaceProof {
    let provider_type = type_name::<PolymarketInstrumentProvider>().to_string();
    let _: GammaQueryBuilder = build_gamma_params_from_hashmap;
    let _: BookSnapshotParser = parse_book_snapshot;
    let _: BookDeltaParser = parse_book_deltas;
    let _: TradeParser = parse_trade_tick;
    let _: QuoteParser = parse_quote_from_price_change;

    PolymarketNtSurfaceProof {
        provider_type,
        surfaces: vec![
            PolymarketNtSurface::InstrumentProviderTokenMap,
            PolymarketNtSurface::GammaQueryBuilder,
            PolymarketNtSurface::WebsocketBookSnapshotParser,
            PolymarketNtSurface::WebsocketBookDeltaParser,
            PolymarketNtSurface::WebsocketTradeParser,
            PolymarketNtSurface::WebsocketQuoteParser,
        ],
    }
}
