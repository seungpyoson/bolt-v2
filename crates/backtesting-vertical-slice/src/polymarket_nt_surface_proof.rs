//! Compile-checked proof that BTE can use NautilusTrader Polymarket surfaces.
//!
//! PMXT remains a one-off historical bootstrap source. The durable projection
//! boundary should reuse NT's Polymarket provider and websocket parsers rather
//! than rebuilding instrument/token lookup or market data parsing locally.

use std::{any::type_name, collections::HashMap};

use nautilus_backtest::config::NautilusDataType;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Data, InstrumentClose, InstrumentStatus, OrderBookDeltas, QuoteTick, TradeTick},
    identifiers::InstrumentId,
    instruments::InstrumentAny,
};
use nautilus_polymarket::{
    http::parse::rebuild_instrument_with_tick_size,
    providers::{PolymarketInstrumentProvider, build_gamma_params_from_hashmap},
    websocket::{
        messages::{
            MarketWsMessage, PolymarketBookSnapshot, PolymarketQuote, PolymarketQuotes,
            PolymarketTickSizeChange, PolymarketTrade,
        },
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
type TickSizeRebuilder =
    fn(&InstrumentAny, &str, UnixNanos, UnixNanos) -> anyhow::Result<InstrumentAny>;
type QuoteParser = fn(
    &PolymarketQuote,
    InstrumentId,
    u8,
    u8,
    bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BacktestInstrumentEpochReplaySupport {
    StaticCatalogInstrumentLoadOnly,
}

/// Witness-typed record of the NT dynamic-instrument-epoch boundary.
///
/// Every capability field carries the `type_name` of the NT surface that
/// proves it, bound at compile time in
/// [`prove_polymarket_dynamic_instrument_epoch_surfaces`]; the struct cannot
/// state a capability without naming the witness. The one `bool` field is a
/// documented NEGATIVE boundary (NT has no instrument-definition backtest
/// stream), not a capability claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketDynamicInstrumentEpochProof {
    pub live_tick_size_change_message_type: String,
    pub live_tick_size_rebuilder_type: &'static str,
    pub catalog_instrument_snapshot_type: String,
    pub catalog_auxiliary_status_close_data_types: [NautilusDataType; 2],
    pub backtest_data_config_instrument_definition_stream_supported: bool,
    pub backtest_instrument_epoch_replay_support: BacktestInstrumentEpochReplaySupport,
    pub nt_type_evidence: Vec<String>,
}

/// Compile-time witness that NT's market websocket envelope carries a
/// `TickSizeChange` arm with the payload the rebuilder consumes.
fn tick_size_change_payload(message: &MarketWsMessage) -> Option<&PolymarketTickSizeChange> {
    match message {
        MarketWsMessage::TickSizeChange(change) => Some(change),
        _ => None,
    }
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

#[must_use]
pub fn prove_polymarket_dynamic_instrument_epoch_surfaces() -> PolymarketDynamicInstrumentEpochProof
{
    let _: TickSizeRebuilder = rebuild_instrument_with_tick_size;
    let _: fn(&MarketWsMessage) -> Option<&PolymarketTickSizeChange> = tick_size_change_payload;
    let live_message_type = type_name::<MarketWsMessage>().to_string();
    let data_stream_type = type_name::<Data>().to_string();
    let instrument_type = type_name::<InstrumentAny>().to_string();

    PolymarketDynamicInstrumentEpochProof {
        live_tick_size_change_message_type: live_message_type.clone(),
        live_tick_size_rebuilder_type: type_name::<TickSizeRebuilder>(),
        catalog_instrument_snapshot_type: instrument_type.clone(),
        catalog_auxiliary_status_close_data_types: [
            NautilusDataType::InstrumentStatus,
            NautilusDataType::InstrumentClose,
        ],
        backtest_data_config_instrument_definition_stream_supported: false,
        backtest_instrument_epoch_replay_support:
            BacktestInstrumentEpochReplaySupport::StaticCatalogInstrumentLoadOnly,
        nt_type_evidence: vec![
            live_message_type,
            data_stream_type,
            instrument_type,
            type_name::<InstrumentStatus>().to_string(),
            type_name::<InstrumentClose>().to_string(),
        ],
    }
}
