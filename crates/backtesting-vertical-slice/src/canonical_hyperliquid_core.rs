//! Hyperliquid core (perp DEX) L2 snapshot -> NautilusTrader `OrderBookDelta`.
//!
//! Hyperliquid core publishes **full L2 book photos** on the `l2Book` channel: no
//! native incremental deltas exist. Each captured line is a complete snapshot of
//! the aggregated book (up to 20 price levels per side, ~0.5s apart). This module
//! reconstructs the standard NautilusTrader snapshot-as-deltas encoding from those
//! photos: every snapshot becomes a `BookAction::Clear` followed by one
//! `BookAction::Add` per level (bids from `levels[0]`, asks from `levels[1]`). The
//! reconstructed deltas are written into a NautilusTrader `ParquetDataCatalog` via
//! the native `write_to_parquet::<OrderBookDelta>` path and read back with
//! `query_typed_data::<OrderBookDelta>`, proving the venue's data lands in an
//! NT-replayable catalog (fidelity class `SNAPSHOT_REPLAY`).
//!
//! No execution-quality or queue-position claims follow from this fidelity: the
//! venue exposes only periodic aggregated book photos, not the per-order event
//! stream, so anything finer than top-of-book/level liquidity replay is forbidden.
//!
//! NO HARDCODES: the instrument identity (NT id, venue symbol, currencies) is
//! supplied by the caller via [`HyperliquidCoreInstrumentSpec`]; the price/size
//! precision is derived from the data itself (the maximum decimal places observed
//! across all levels), never a code literal. The only structural fact this module
//! encodes is Hyperliquid's wire shape (`raw.data.levels[0]` = bids,
//! `levels[1]` = asks, `data.time` in milliseconds).

use std::{collections::HashMap, io::Read, path::Path, str::FromStr};

use anyhow::{Context, Result, bail, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, TradeTick},
    enums::{AggressorSide, BookAction, OrderSide, RecordFlag},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{CryptoPerpetual, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// NautilusTrader data type written for the `node_fills_by_block` projection.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// Milliseconds-to-nanoseconds scale for the venue event timestamp.
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// Venue wire channel the snapshots are captured from.
const HYPERLIQUID_L2_CHANNEL: &str = "l2Book";

/// One level of an L2 book photo: aggregated price/size plus order count `n`.
#[derive(Debug, Clone, Deserialize)]
struct WireLevel {
    /// Aggregated price at this level, exact source decimal string.
    px: String,
    /// Aggregated size resting at this level, exact source decimal string.
    sz: String,
    /// Number of orders aggregated into this level (carried for provenance only).
    #[allow(dead_code)]
    n: u64,
}

/// The `raw.data` body of an `l2Book` snapshot.
#[derive(Debug, Clone, Deserialize)]
struct WireBookData {
    coin: String,
    /// Exchange event timestamp in milliseconds.
    time: i64,
    /// `[bids, asks]`; each side is ordered best-first by the venue.
    levels: [Vec<WireLevel>; 2],
}

/// The `raw` envelope identifying the channel.
#[derive(Debug, Clone, Deserialize)]
struct WireRaw {
    channel: String,
    data: WireBookData,
}

/// One captured Hyperliquid `l2Book` line.
#[derive(Debug, Clone, Deserialize)]
struct WireSnapshot {
    /// Capture (worker-receipt) timestamp, ISO-8601 with nanosecond precision.
    time: String,
    raw: WireRaw,
}

/// Caller-supplied instrument identity for the reconstructed book.
///
/// Built from accepted instrument-universe metadata; this module never decides a
/// venue, symbol, or currency itself. Hyperliquid core markets are USDC-settled
/// crypto perpetuals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyperliquidCoreInstrumentSpec {
    /// NautilusTrader instrument id, for example `BNB.HYPERLIQUID`.
    pub nt_instrument_id: String,
    /// Venue-native symbol / coin code, for example `BNB`.
    pub raw_symbol: String,
    /// Base currency code (the coin), for example `BNB`.
    pub base_currency: String,
    /// Quote currency code, for example `USDC`.
    pub quote_currency: String,
    /// Settlement currency code, for example `USDC`.
    pub settlement_currency: String,
}

/// A reconstructed book ready for the NautilusTrader catalog.
#[derive(Debug, Clone)]
pub struct ReconstructedBook {
    /// The NT instrument carrying the book.
    pub instrument: InstrumentAny,
    /// Number of source snapshots consumed.
    pub snapshot_count: usize,
    /// Price precision derived from the data (max decimal places observed).
    pub price_precision: u8,
    /// Size precision derived from the data (max decimal places observed).
    pub size_precision: u8,
    /// Reconstructed deltas in capture order (`Clear` + `Add`* per snapshot).
    pub deltas: Vec<OrderBookDelta>,
}

/// Result of projecting reconstructed deltas into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProjection {
    pub nt_instrument_id: String,
    pub data_type: String,
    pub delta_count: usize,
    pub snapshot_count: usize,
}

/// Decimal places implied by a decimal string (`657.85` -> 2, `658` -> 0).
fn decimal_places(value: &str) -> u8 {
    match value.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    }
}

/// Rescale a source decimal string to a fixed precision, failing if it carries
/// more precision than the instrument allows (it never should — the precision is
/// derived as the per-file maximum — but this fails loud if a caller passes a
/// smaller precision).
fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Parse the JSONL into typed snapshots, validating the channel, coin, and
/// monotonic event time.
fn parse_snapshots(jsonl: &str, expected_coin: &str) -> Result<Vec<WireSnapshot>> {
    let mut snapshots = Vec::new();
    let mut previous_event_ms = i64::MIN;
    for (index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let snapshot: WireSnapshot = serde_json::from_str(line)
            .with_context(|| format!("line {index}: malformed l2Book JSON"))?;
        ensure!(
            snapshot.raw.channel == HYPERLIQUID_L2_CHANNEL,
            "line {index}: unexpected channel {:?} (want {HYPERLIQUID_L2_CHANNEL:?})",
            snapshot.raw.channel
        );
        ensure!(
            snapshot.raw.data.coin == expected_coin,
            "line {index}: coin {:?} does not match expected {expected_coin:?}",
            snapshot.raw.data.coin
        );
        ensure!(
            !snapshot.time.trim().is_empty(),
            "line {index}: empty capture time"
        );
        let event_ms = snapshot.raw.data.time;
        ensure!(
            event_ms > 0,
            "line {index}: non-positive event time {event_ms}"
        );
        ensure!(
            event_ms >= previous_event_ms,
            "line {index}: event time {event_ms} precedes previous {previous_event_ms}"
        );
        previous_event_ms = event_ms;
        snapshots.push(snapshot);
    }
    ensure!(!snapshots.is_empty(), "no l2Book snapshots found in input");
    Ok(snapshots)
}

/// Build the USDC-settled `CryptoPerpetual` from the caller spec at the
/// data-derived precision.
fn build_instrument(
    spec: &HyperliquidCoreInstrumentSpec,
    price_precision: u8,
    size_precision: u8,
) -> Result<CryptoPerpetual> {
    let id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let base = Currency::from_str(&spec.base_currency)
        .with_context(|| format!("invalid base_currency {:?}", spec.base_currency))?;
    let quote = Currency::from_str(&spec.quote_currency)
        .with_context(|| format!("invalid quote_currency {:?}", spec.quote_currency))?;
    let settlement = Currency::from_str(&spec.settlement_currency)
        .with_context(|| format!("invalid settlement_currency {:?}", spec.settlement_currency))?;
    // Price/size increments are the smallest representable step at the derived
    // precision (10^-precision), so the instrument's increment agrees with the
    // precision the deltas are encoded at — never a hardcoded tick size.
    let price_increment = Price::new(10f64.powi(-i32::from(price_precision)), price_precision);
    let size_increment = Quantity::new(10f64.powi(-i32::from(size_precision)), size_precision);
    Ok(CryptoPerpetual::new(
        id,
        Symbol::new(&spec.raw_symbol),
        base,
        quote,
        settlement,
        false, // is_inverse: Hyperliquid core perps are linear (USDC-settled)
        price_precision,
        size_precision,
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
    ))
}

/// Reconstruct NautilusTrader `OrderBookDelta`s from Hyperliquid core L2 snapshots.
///
/// Each snapshot is encoded as a `Clear` (flagged `F_SNAPSHOT`) followed by one
/// `Add` per level, bids first then asks. The final delta of each snapshot bundle
/// carries `F_LAST` so a NautilusTrader `OrderBook` applies the photo as one atomic
/// replacement. The sequence number is a per-file running counter; the price/size
/// precision is the maximum decimal places observed across all levels.
///
/// # Errors
///
/// Returns an error if the input is empty/malformed, the channel/coin mismatch, the
/// event time is non-monotonic, or a level price/size cannot be represented.
pub fn reconstruct_books(
    jsonl: &str,
    spec: &HyperliquidCoreInstrumentSpec,
) -> Result<ReconstructedBook> {
    let snapshots = parse_snapshots(jsonl, &spec.raw_symbol)?;

    // Derive a single instrument precision from the data: the maximum decimal
    // places seen on any level price/size. NT encodes one precision per instrument,
    // so every delta is rescaled to this shared precision.
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for snapshot in &snapshots {
        for side in &snapshot.raw.data.levels {
            for level in side {
                price_precision = price_precision.max(decimal_places(&level.px));
                size_precision = size_precision.max(decimal_places(&level.sz));
            }
        }
    }

    let instrument = build_instrument(spec, price_precision, size_precision)?;
    let instrument_id = instrument.id();

    let mut deltas = Vec::new();
    let mut sequence: u64 = 0;
    for snapshot in &snapshots {
        let event_ns = u64::try_from(
            snapshot
                .raw
                .data
                .time
                .checked_mul(NANOS_PER_MILLISECOND)
                .context("event time overflow")?,
        )
        .context("negative event time")?;
        let ts_event = UnixNanos::from(event_ns);
        // The capture timestamp is the worker-receipt time; the event timestamp is
        // the exchange book time. Replay orders on ts_init, so both are set to the
        // event time to keep deltas of one photo atomic and monotonic.
        let ts_init = ts_event;

        let bids = snapshot.raw.data.levels.first().expect("levels[0] present");
        let asks = snapshot.raw.data.levels.get(1).expect("levels[1] present");
        let level_count = bids.len() + asks.len();

        // 1) Clear the book for this photo (F_SNAPSHOT). If there are no levels at
        //    all, the Clear is itself the last record of the bundle.
        let clear_flags = if level_count == 0 {
            (RecordFlag::F_SNAPSHOT as u8) | (RecordFlag::F_LAST as u8)
        } else {
            RecordFlag::F_SNAPSHOT as u8
        };
        deltas.push(OrderBookDelta::new(
            instrument_id,
            BookAction::Clear,
            BookOrder::default(),
            clear_flags,
            sequence,
            ts_event,
            ts_init,
        ));
        sequence += 1;

        // 2) Add each level. order_id is a synthetic per-delta id (the sequence)
        //    because Hyperliquid aggregates levels and exposes no per-order ids.
        let mut emitted = 0usize;
        for (side, levels) in [(OrderSide::Buy, bids), (OrderSide::Sell, asks)] {
            for level in levels {
                emitted += 1;
                let is_last = emitted == level_count;
                let flags = if is_last {
                    (RecordFlag::F_SNAPSHOT as u8) | (RecordFlag::F_LAST as u8)
                } else {
                    RecordFlag::F_SNAPSHOT as u8
                };
                let price = Price::from_str(&rescaled(&level.px, price_precision)?)
                    .map_err(|e| anyhow::anyhow!("invalid price {:?}: {e}", level.px))?;
                let size = Quantity::from_str(&rescaled(&level.sz, size_precision)?)
                    .map_err(|e| anyhow::anyhow!("invalid size {:?}: {e}", level.sz))?;
                ensure!(
                    size.is_positive(),
                    "level size {:?} is not positive (Add requires > 0)",
                    level.sz
                );
                deltas.push(OrderBookDelta::new(
                    instrument_id,
                    BookAction::Add,
                    BookOrder::new(side, price, size, sequence),
                    flags,
                    sequence,
                    ts_event,
                    ts_init,
                ));
                sequence += 1;
            }
        }
    }

    Ok(ReconstructedBook {
        instrument: instrument.into_any(),
        snapshot_count: snapshots.len(),
        price_precision,
        size_precision,
        deltas,
    })
}

/// Project reconstructed Hyperliquid core books into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Writes the venue instrument and the `OrderBookDelta` stream under `catalog_root`
/// using NautilusTrader's own write path, then returns a [`CatalogProjection`].
/// Fails closed on a non-empty `catalog_root` (NT's writer skips existing files and
/// would otherwise read back stale data).
///
/// # Errors
///
/// Returns an error if reconstruction, instrument construction, or catalog writes
/// fail.
pub fn project_books_to_catalog(
    book: &ReconstructedBook,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    ensure!(!book.deltas.is_empty(), "no deltas to project");
    if catalog_root.exists() {
        let mut entries = std::fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    std::fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;

    let instrument_id = book.instrument.id().to_string();
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![book.instrument.clone()])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(book.deltas.clone(), None, None, None)
        .context("write order book deltas to catalog")?;

    Ok(CatalogProjection {
        nt_instrument_id: instrument_id,
        data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
        delta_count: book.deltas.len(),
        snapshot_count: book.snapshot_count,
    })
}

/// Read the projected `OrderBookDelta` stream back from `catalog_root`, proving the
/// resolved NautilusTrader dependency can replay it.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_order_book_deltas(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<OrderBookDelta>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<OrderBookDelta>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query order book deltas from catalog")
}

// ===========================================================================
// node_fills_by_block -> NautilusTrader `TradeTick`
// ===========================================================================
//
// Hyperliquid's node operator captures one JSONL line per chain block. Each line
// is `{local_time, block_time, block_number, events}` where `events` is an array
// of `[user_address, fill]` pairs. Every executed trade emits TWO fill records in
// the SAME block sharing one `tid`: the resting **maker** leg (`crossed:false`)
// and the aggressing **taker** leg (`crossed:true`). Both legs carry identical
// `px`/`sz`/`time`; only `side`, `crossed`, the user, and per-side accounting
// fields differ. (Verified across 92,308 tid-pairs in a real object: every tid
// occurs exactly twice with opposite `side`, opposite `crossed`, equal px/sz/time.)
//
// A NautilusTrader `TradeTick` is one market print per trade, so this projection
// **deduplicates by `tid` and keeps the taker (`crossed:true`) leg**. The taker's
// `side` is the aggressor: Hyperliquid `side:"B"` = the taker bought
// (`AggressorSide::Buyer`), `side:"A"` = the taker sold (`AggressorSide::Seller`).
// The fill `tid` becomes the NT `TradeId`. The fill `time` (exchange match time in
// milliseconds) becomes both `ts_event` and `ts_init`.
//
// NO HARDCODES: the coin to project, the NT instrument identity, and the
// currencies all come from the caller-supplied [`HyperliquidCoreInstrumentSpec`]
// (reused from the L2 path). Price/size precision is derived from the data (the
// max decimal places observed across the selected coin's fills), never a literal.

/// LZ4-frame magic number (`0x184D2204`, little-endian on disk) prefixing the raw
/// Hyperliquid node objects. Carried as a fact for the doc/decompression contract.
const LZ4_FRAME_MAGIC: [u8; 4] = [0x04, 0x22, 0x4D, 0x18];

/// Hyperliquid taker `side` token for a buy (the aggressor lifted the offer).
const HL_SIDE_BUY: &str = "B";

/// Hyperliquid taker `side` token for a sell (the aggressor hit the bid).
const HL_SIDE_SELL: &str = "A";

/// NautilusTrader venue code appended to a coin to form the `nt_instrument_id`
/// (`BTC` -> `BTC.HYPERLIQUID`). The per-venue format constant, mirroring OKX's
/// `OKX` suffix — not a runtime instrument id.
pub const HYPERLIQUID_VENUE: &str = "HYPERLIQUID";

/// Quote and settlement currency for Hyperliquid core perpetuals. These markets
/// are USDC-settled linear perps (the same fact `build_instrument` encodes with
/// `is_inverse = false`); it is a venue-shape constant, not a per-instrument
/// runtime value, so the bulk path derives the spec's currencies from it without
/// an instrument universe (Hyperliquid stages none).
const HYPERLIQUID_CORE_QUOTE_CCY: &str = "USDC";

/// Namespace separator the venue uses for builder-deployed (HIP-3) DEX markets:
/// a coin token of the form `<dex>:<local>` (for example `xyz:SILVER`). These are
/// NOT top-level Hyperliquid core perps — they belong to the HIP-3 family (whose
/// own converter/backfill owns them, splitting on this exact separator) — so the
/// core converter fences them out of the shared `node_fills_by_block` object.
const HYPERLIQUID_HIP3_NAMESPACE_SEP: char = ':';

/// Prefix the venue uses for spot pair index tokens (for example `@107`). Spot
/// markets are not USDC-settled core perps, so the core converter fences any
/// `@`-prefixed token out of the shared object.
const HYPERLIQUID_SPOT_INDEX_PREFIX: char = '@';

/// Separator the venue uses for the one spot pair it names by symbol rather than
/// by `@`-index: `BASE/QUOTE` (for example `PURR/USDC`). Like the `@`-indexed
/// spot markets, it is not a USDC-settled core perp, so the core converter
/// fences any token containing this separator out of the shared object.
const HYPERLIQUID_SPOT_PAIR_SEP: char = '/';

/// One fill record as it appears inside an `events` pair's second element.
///
/// Only the fields this projection needs are deserialized; the rest of the rich
/// fill payload (`startPosition`, `dir`, `closedPnl`, `hash`, `oid`, `fee`,
/// `feeToken`, `cloid`, `twapId`, ...) is ignored — `TradeTick` carries only the
/// market print.
#[derive(Debug, Clone, Deserialize)]
struct WireFill {
    /// Venue coin code (for example `BTC`). Selects the instrument to project.
    coin: String,
    /// Exact source fill price string.
    px: String,
    /// Exact source fill size string.
    sz: String,
    /// `"B"` = this leg bought, `"A"` = this leg sold.
    side: String,
    /// Exchange match timestamp in milliseconds.
    time: i64,
    /// Whether this leg crossed the spread (the aggressor/taker leg).
    crossed: bool,
    /// Trade id shared by the maker and taker legs of one match.
    tid: u64,
}

/// One captured block: a `local_time`/`block_time`/`block_number` header plus the
/// per-block `events`. Each event is `[user_address, fill]`.
#[derive(Debug, Clone, Deserialize)]
struct WireBlock {
    events: Vec<(String, WireFill)>,
}

/// A taker fill selected for projection, parsed and validated.
#[derive(Debug, Clone)]
struct TakerFill {
    tid: u64,
    aggressor: AggressorSide,
    price: String,
    size: String,
    /// Exchange match time in Unix nanoseconds.
    event_ns: u64,
}

/// Reconstructed trade stream ready for the NautilusTrader catalog.
#[derive(Debug, Clone)]
pub struct ReconstructedTrades {
    /// The NT instrument carrying the trades.
    pub instrument: InstrumentAny,
    /// Number of unique trades (deduplicated by `tid`) projected.
    pub trade_count: usize,
    /// Price precision derived from the data (max decimal places observed).
    pub price_precision: u8,
    /// Size precision derived from the data (max decimal places observed).
    pub size_precision: u8,
    /// Trade ticks sorted by ascending event time (NT write contract).
    pub trades: Vec<TradeTick>,
}

/// Result of projecting reconstructed trades into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradesCatalogProjection {
    pub nt_instrument_id: String,
    pub data_type: String,
    pub trade_count: usize,
}

/// Decompress an LZ4-frame-compressed Hyperliquid node object into JSONL text.
///
/// The raw S3 objects are stored in the standard LZ4 frame format (magic
/// `0x184D2204`); this mirrors [`super::canonical_deribit::read_gzip_csv`] for the
/// gzip case — the converter module owns its decompression step.
///
/// # Errors
///
/// Returns an error if the file cannot be read, lacks the LZ4 frame magic, or is
/// not valid LZ4 / UTF-8.
pub fn read_lz4_jsonl(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read lz4 object {}", path.display()))?;
    ensure!(
        bytes.len() >= 4 && bytes[..4] == LZ4_FRAME_MAGIC,
        "object {} is not an LZ4 frame (bad magic)",
        path.display()
    );
    let mut decoder = lz4_flex::frame::FrameDecoder::new(bytes.as_slice());
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .with_context(|| format!("decompress lz4 object {}", path.display()))?;
    Ok(text)
}

/// Map a Hyperliquid taker `side` token to the NautilusTrader aggressor side.
///
/// `"B"` -> the taker bought (`Buyer`); `"A"` -> the taker sold (`Seller`).
fn taker_side_to_aggressor(side: &str) -> Result<AggressorSide> {
    match side {
        HL_SIDE_BUY => Ok(AggressorSide::Buyer),
        HL_SIDE_SELL => Ok(AggressorSide::Seller),
        other => {
            bail!("unknown taker side token {other:?} (want {HL_SIDE_BUY:?} or {HL_SIDE_SELL:?})")
        }
    }
}

/// Reconstruct NautilusTrader `TradeTick`s from Hyperliquid `node_fills_by_block`
/// JSONL for one coin.
///
/// Each block's `events` are scanned; only fills whose `coin` equals
/// `spec.raw_symbol` are considered. Trades are deduplicated by `tid`, keeping the
/// taker (`crossed:true`) leg as the canonical print, and the result is sorted by
/// ascending event time to satisfy NautilusTrader's non-decreasing `ts_init` write
/// contract (the source is block-ordered, but fills inside a block are not sorted
/// by match time, so a global sort is required and is not an error).
///
/// Price/size precision is the maximum number of decimal places observed across
/// the selected coin's fills.
///
/// # Errors
///
/// Returns an error if the input is empty/malformed, no fills exist for the coin,
/// a taker side token is unknown, a price/size is non-positive or cannot be
/// represented, or a maker/taker `tid` pair disagrees on price/size.
pub fn reconstruct_trades(
    jsonl: &str,
    spec: &HyperliquidCoreInstrumentSpec,
) -> Result<ReconstructedTrades> {
    ensure!(
        !spec.raw_symbol.trim().is_empty(),
        "spec.raw_symbol must not be empty"
    );

    // Collect taker (crossed=true) legs for the requested coin, deduplicated by
    // tid. A tid should appear exactly once as a taker; if it appears twice the
    // legs must agree on price/size/time (they always do in real data) — disagree
    // and we fail loud rather than silently picking one.
    let mut takers: HashMap<u64, TakerFill> = HashMap::new();
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;

    for (line_index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let block: WireBlock = serde_json::from_str(line)
            .with_context(|| format!("line {line_index}: malformed node_fills_by_block JSON"))?;
        for (_user, fill) in &block.events {
            if fill.coin != spec.raw_symbol {
                continue;
            }
            if !fill.crossed {
                // Maker leg: skip; the taker leg of the same tid is the print.
                continue;
            }
            ensure!(
                fill.time > 0,
                "line {line_index}: non-positive fill time {}",
                fill.time
            );
            let price: Decimal = fill
                .px
                .parse()
                .with_context(|| format!("line {line_index}: invalid px {:?}", fill.px))?;
            let size: Decimal = fill
                .sz
                .parse()
                .with_context(|| format!("line {line_index}: invalid sz {:?}", fill.sz))?;
            ensure!(
                price > Decimal::ZERO,
                "line {line_index}: non-positive fill price {:?}",
                fill.px
            );
            ensure!(
                size > Decimal::ZERO,
                "line {line_index}: non-positive fill size {:?}",
                fill.sz
            );

            let aggressor = taker_side_to_aggressor(&fill.side)
                .map_err(|e| anyhow::anyhow!("line {line_index}: tid {}: {e}", fill.tid))?;
            let event_ms = fill.time;
            let event_ns = u64::try_from(
                event_ms
                    .checked_mul(NANOS_PER_MILLISECOND)
                    .context("fill time overflow")?,
            )
            .context("negative fill time")?;

            let candidate = TakerFill {
                tid: fill.tid,
                aggressor,
                price: fill.px.clone(),
                size: fill.sz.clone(),
                event_ns,
            };
            if let Some(existing) = takers.get(&fill.tid) {
                ensure!(
                    existing.price == candidate.price
                        && existing.size == candidate.size
                        && existing.event_ns == candidate.event_ns,
                    "line {line_index}: duplicate taker leg for tid {} disagrees on px/sz/time",
                    fill.tid
                );
                // Identical duplicate (defensive): keep the first.
                continue;
            }
            price_precision = price_precision.max(decimal_places(&fill.px));
            size_precision = size_precision.max(decimal_places(&fill.sz));
            takers.insert(fill.tid, candidate);
        }
    }

    ensure!(
        !takers.is_empty(),
        "no taker fills found for coin {:?}",
        spec.raw_symbol
    );

    let instrument = build_instrument(spec, price_precision, size_precision)?;
    let instrument_id = instrument.id();

    // Sort by event time, then tid for a stable order among same-timestamp fills.
    let mut selected: Vec<TakerFill> = takers.into_values().collect();
    selected.sort_by(|a, b| a.event_ns.cmp(&b.event_ns).then(a.tid.cmp(&b.tid)));

    let mut trades = Vec::with_capacity(selected.len());
    for fill in &selected {
        let price = Price::from_str(&rescaled(&fill.price, price_precision)?)
            .map_err(|e| anyhow::anyhow!("invalid price {:?}: {e}", fill.price))?;
        let size = Quantity::from_str(&rescaled(&fill.size, size_precision)?)
            .map_err(|e| anyhow::anyhow!("invalid size {:?}: {e}", fill.size))?;
        let trade_id = TradeId::new_checked(fill.tid.to_string())
            .map_err(|e| anyhow::anyhow!("invalid trade id {}: {e}", fill.tid))?;
        let ts = UnixNanos::from(fill.event_ns);
        trades.push(TradeTick::new(
            instrument_id,
            price,
            size,
            fill.aggressor,
            trade_id,
            ts,
            ts,
        ));
    }

    Ok(ReconstructedTrades {
        instrument: instrument.into_any(),
        trade_count: trades.len(),
        price_precision,
        size_precision,
        trades,
    })
}

/// Project reconstructed Hyperliquid core trades into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Writes the venue instrument and the `TradeTick` stream under `catalog_root`
/// using NautilusTrader's own write path, then returns a [`TradesCatalogProjection`].
/// Fails closed on a non-empty `catalog_root` (NT's writer skips existing files and
/// would otherwise read back stale data).
///
/// # Errors
///
/// Returns an error if there are no trades, the catalog root is dirty, or a catalog
/// write fails.
pub fn project_trades_to_catalog(
    reconstructed: &ReconstructedTrades,
    catalog_root: &Path,
) -> Result<TradesCatalogProjection> {
    ensure!(!reconstructed.trades.is_empty(), "no trades to project");
    if catalog_root.exists() {
        let mut entries = std::fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    std::fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;

    let instrument_id = reconstructed.instrument.id().to_string();
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![reconstructed.instrument.clone()])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(reconstructed.trades.clone(), None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(TradesCatalogProjection {
        nt_instrument_id: instrument_id,
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        trade_count: reconstructed.trades.len(),
    })
}

/// Read the projected `TradeTick` stream back from `catalog_root`, proving the
/// resolved NautilusTrader dependency can replay it.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_trade_ticks(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<TradeTick>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<TradeTick>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query trade ticks from catalog")
}

// ===========================================================================
// Bulk convert path: node_fills_by_block archive object -> TradeTick streams
// ===========================================================================
//
// A single staged `node_fills_by_block` object (one chain hour) carries fills
// for MANY coins. The bulk path therefore enumerates the distinct coins present
// in the object, builds a data-derived spec per coin, and reuses the committed
// `reconstruct_trades` (dedup-by-tid, keep-taker, derive precision, sort) once
// per coin, appending each coin's `TradeTick` stream into a shared, already-open
// catalog with NO clean-root guard. This mirrors `append_okx_trades_archive`:
// the `project_*_to_catalog` fns remain the hermetic single-object test harness.

/// Decompress an LZ4-frame object already held in memory into JSONL text.
///
/// The bulk path receives the raw object bytes (mirroring
/// [`append_okx_trades_archive`]'s `zip_bytes: &[u8]`), so this is the in-memory
/// twin of [`read_lz4_jsonl`] (which decompresses from a path). Both enforce the
/// LZ4 frame magic and fail loud on a non-LZ4 / non-UTF-8 payload.
///
/// # Errors
///
/// Returns an error if the bytes lack the LZ4 frame magic or are not valid
/// LZ4 / UTF-8.
pub fn decompress_lz4_frame(bytes: &[u8]) -> Result<String> {
    ensure!(
        bytes.len() >= 4 && bytes[..4] == LZ4_FRAME_MAGIC,
        "object is not an LZ4 frame (bad magic)"
    );
    let mut decoder = lz4_flex::frame::FrameDecoder::new(bytes);
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .context("decompress lz4 object")?;
    Ok(text)
}

/// Whether a venue coin token belongs to the Hyperliquid **core perp** family.
///
/// A single `node_fills_by_block` object multiplexes fills for every market that
/// traded that hour — top-level core perps (`BTC`, `ETH`, newly-listed alts like
/// `PUMP`) AND markets that are NOT core perps: builder-deployed HIP-3 DEX
/// instruments (`<dex>:<local>`, for example `xyz:SILVER`) and spot markets —
/// most named by an `@`-index (`@107`), plus the one pair the venue names by its
/// `BASE/QUOTE` symbol (`PURR/USDC`). The latter are owned by their own venue
/// families and would carry false provenance if written as USDC-settled core
/// perps, so the core converter fences them out of the shared object (the rule's
/// allowed "genuinely-foreign instrument fenced out of a multi-instrument
/// object").
///
/// The classification is the venue's OWN naming convention, not an invented
/// allowlist: a HIP-3 token always contains [`HYPERLIQUID_HIP3_NAMESPACE_SEP`]
/// (the same split the HIP-3 backfill uses), an `@`-indexed spot market starts
/// with [`HYPERLIQUID_SPOT_INDEX_PREFIX`], and the symbol-named spot pair
/// contains [`HYPERLIQUID_SPOT_PAIR_SEP`]. Anything else is a top-level core
/// perp (a bare ticker like `BTC`).
#[must_use]
pub fn is_core_perp_coin(coin: &str) -> bool {
    let coin = coin.trim();
    !coin.is_empty()
        && !coin.contains(HYPERLIQUID_HIP3_NAMESPACE_SEP)
        && !coin.starts_with(HYPERLIQUID_SPOT_INDEX_PREFIX)
        && !coin.contains(HYPERLIQUID_SPOT_PAIR_SEP)
}

/// Distinct Hyperliquid **core perp** coins that appear as a **taker**
/// (`crossed:true`) leg in a `node_fills_by_block` object, in first-seen order.
///
/// One object spans every market that traded in the hour, so the bulk converter
/// writes one catalog stream per distinct core-perp coin rather than assuming a
/// single instrument — the analogue of [`okx_trade_instruments`]. Two filters
/// apply: (1) only coins with a taker leg are returned, because the taker leg is
/// the canonical print `reconstruct_trades` keeps (a maker-only coin yields no
/// trade); (2) non-core-perp markets (HIP-3 DEX tokens, spot indices) are fenced
/// out via [`is_core_perp_coin`] because they belong to other venue families.
///
/// # Errors
///
/// Returns an error if a line is not valid `node_fills_by_block` JSON.
pub fn hyperliquid_core_fill_coins(jsonl: &str) -> Result<Vec<String>> {
    let mut seen: Vec<String> = Vec::new();
    for (line_index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let block: WireBlock = serde_json::from_str(line)
            .with_context(|| format!("line {line_index}: malformed node_fills_by_block JSON"))?;
        for (_user, fill) in &block.events {
            if !fill.crossed {
                continue;
            }
            if is_core_perp_coin(&fill.coin) && !seen.iter().any(|c| c == &fill.coin) {
                seen.push(fill.coin.clone());
            }
        }
    }
    Ok(seen)
}

/// Build the [`HyperliquidCoreInstrumentSpec`] for one coin in a
/// `node_fills_by_block` object.
///
/// Identity is derived entirely from the data + venue-shape constants: the
/// `nt_instrument_id` is `{coin}.{HYPERLIQUID_VENUE}`, the base currency is the
/// coin, and the quote/settlement currency is USDC (Hyperliquid core perps are
/// USDC-settled linear perps — [`HYPERLIQUID_CORE_QUOTE_CCY`]). No instrument
/// universe is consulted (Hyperliquid stages none); price/size precision is
/// derived from the data inside [`reconstruct_trades`], never here and never a
/// literal.
///
/// # Errors
///
/// Returns an error if `coin` is empty.
pub fn hyperliquid_core_fills_spec_for_coin(coin: &str) -> Result<HyperliquidCoreInstrumentSpec> {
    let coin = coin.trim();
    ensure!(!coin.is_empty(), "cannot build a spec for an empty coin");
    Ok(HyperliquidCoreInstrumentSpec {
        nt_instrument_id: format!("{coin}.{HYPERLIQUID_VENUE}"),
        raw_symbol: coin.to_string(),
        base_currency: coin.to_string(),
        quote_currency: HYPERLIQUID_CORE_QUOTE_CCY.to_string(),
        settlement_currency: HYPERLIQUID_CORE_QUOTE_CCY.to_string(),
    })
}

/// One coin's write summary produced by [`append_hyperliquid_core_fills_archive`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidCoreAppendSummary {
    pub nt_instrument_id: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Append every coin's `TradeTick`s from one Hyperliquid `node_fills_by_block`
/// LZ4 object into an already-open [`ParquetDataCatalog`] — the bulk-conversion
/// path.
///
/// Unlike [`project_trades_to_catalog`] (the hermetic single-coin proof harness,
/// which refuses a dirty root), this appends into a shared, possibly-S3 catalog:
/// it relies on NautilusTrader's own per-instrument, per-time-range file naming
/// and skip-on-existing so many objects flow into one catalog. The object's
/// core-perp coins are enumerated ([`hyperliquid_core_fill_coins`], which fences
/// out HIP-3 DEX tokens and spot indices); for each, a data-derived spec is built
/// ([`hyperliquid_core_fills_spec_for_coin`]) and the committed
/// [`reconstruct_trades`] does the dedup/precision/sort, after which the coin's
/// ticks are written. Returns one summary per coin written.
///
/// Newly-listed coins are not in NautilusTrader's static currency map, so before
/// reconstruction each coin (and the USDC quote) is registered as a crypto
/// currency via NautilusTrader's own adapter path
/// [`Currency::get_or_create_crypto`] — the sanctioned way to admit exchange
/// assets the static map predates. This does not fabricate identity: on a DEX the
/// coin token IS its own currency code. Without it, `build_instrument`'s
/// `Currency::from_str` would reject an unlisted coin and drop a real core perp.
///
/// # Errors
///
/// Returns an error if decompression, JSON parsing, trade reconstruction, or the
/// catalog write fails, or if the object yields no core-perp taker fills.
pub fn append_hyperliquid_core_fills_archive(
    lz4_bytes: &[u8],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<HyperliquidCoreAppendSummary>> {
    let jsonl = decompress_lz4_frame(lz4_bytes)?;
    let coins = hyperliquid_core_fill_coins(&jsonl)?;
    // USDC is the shared quote/settlement currency; register it once up front.
    let _ = Currency::get_or_create_crypto(HYPERLIQUID_CORE_QUOTE_CCY);
    let mut summaries = Vec::new();
    for coin in coins {
        // Admit the coin into NautilusTrader's currency map (no-op if already
        // registered) so the committed `build_instrument` -> `Currency::from_str`
        // resolves an unlisted DEX asset instead of failing.
        let _ = Currency::get_or_create_crypto(&coin);
        let spec = hyperliquid_core_fills_spec_for_coin(&coin)?;
        let reconstructed = reconstruct_trades(&jsonl, &spec)
            .with_context(|| format!("reconstruct Hyperliquid core trades for {coin}"))?;
        if reconstructed.trades.is_empty() {
            continue;
        }
        let summary = HyperliquidCoreAppendSummary {
            nt_instrument_id: spec.nt_instrument_id.clone(),
            record_count: reconstructed.trades.len(),
            price_precision: reconstructed.price_precision,
            size_precision: reconstructed.size_precision,
        };
        catalog
            .write_to_parquet(reconstructed.trades, None, None, None)
            .with_context(|| format!("append Hyperliquid core trade ticks for {coin}"))?;
        summaries.push(summary);
    }
    ensure!(
        !summaries.is_empty(),
        "Hyperliquid core node_fills_by_block object yielded no taker fills"
    );
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> HyperliquidCoreInstrumentSpec {
        HyperliquidCoreInstrumentSpec {
            nt_instrument_id: "BNB.HYPERLIQUID".to_string(),
            raw_symbol: "BNB".to_string(),
            base_currency: "BNB".to_string(),
            quote_currency: "USDC".to_string(),
            settlement_currency: "USDC".to_string(),
        }
    }

    const ONE_SNAPSHOT: &str = "{\"time\":\"2025-06-01T00:00:06.460737487\",\"ver_num\":1,\
        \"raw\":{\"channel\":\"l2Book\",\"data\":{\"coin\":\"BNB\",\"time\":1748736005738,\
        \"levels\":[[{\"px\":\"657.85\",\"sz\":\"0.986\",\"n\":2},\
        {\"px\":\"657.8\",\"sz\":\"1.5\",\"n\":1}],\
        [{\"px\":\"657.86\",\"sz\":\"0.011\",\"n\":1}]]}}}\n";

    #[test]
    fn decimal_places_reads_string_precision() {
        assert_eq!(decimal_places("657.85"), 2);
        assert_eq!(decimal_places("0.986"), 3);
        assert_eq!(decimal_places("658"), 0);
    }

    #[test]
    fn reconstructs_clear_then_adds_with_derived_precision() {
        let book = reconstruct_books(ONE_SNAPSHOT, &spec()).expect("reconstruct");
        // 1 snapshot -> 1 Clear + 2 bids + 1 ask = 4 deltas.
        assert_eq!(book.snapshot_count, 1);
        assert_eq!(book.deltas.len(), 4);
        // Precision derived from data: px max 2 dp, sz max 3 dp.
        assert_eq!(book.price_precision, 2);
        assert_eq!(book.size_precision, 3);

        assert_eq!(book.deltas[0].action, BookAction::Clear);
        assert!(
            (book.deltas[0].flags & (RecordFlag::F_SNAPSHOT as u8)) != 0,
            "Clear carries F_SNAPSHOT"
        );
        // First Add is a bid at the best bid price/size, rescaled to derived precision.
        assert_eq!(book.deltas[1].action, BookAction::Add);
        assert_eq!(book.deltas[1].order.side, OrderSide::Buy);
        assert_eq!(book.deltas[1].order.price, Price::from("657.85"));
        assert_eq!(book.deltas[1].order.size, Quantity::from("0.986"));
        // 657.8 must be rescaled to 2 dp -> 657.80.
        assert_eq!(book.deltas[2].order.price, Price::from("657.80"));
        // Last delta of the bundle is the ask, flagged F_LAST.
        assert_eq!(book.deltas[3].order.side, OrderSide::Sell);
        assert!(
            (book.deltas[3].flags & (RecordFlag::F_LAST as u8)) != 0,
            "final delta of a snapshot bundle carries F_LAST"
        );
        // Event time = ms * 1e6.
        assert_eq!(
            book.deltas[0].ts_event,
            UnixNanos::from(1_748_736_005_738u64 * 1_000_000)
        );
    }

    #[test]
    fn empty_book_snapshot_yields_only_a_flagged_clear() {
        let empty = "{\"time\":\"2025-06-01T00:00:06.4\",\"ver_num\":1,\
            \"raw\":{\"channel\":\"l2Book\",\"data\":{\"coin\":\"BNB\",\"time\":1748736005738,\
            \"levels\":[[],[]]}}}\n";
        let book = reconstruct_books(empty, &spec()).expect("reconstruct");
        assert_eq!(book.deltas.len(), 1);
        assert_eq!(book.deltas[0].action, BookAction::Clear);
        assert!(
            (book.deltas[0].flags & (RecordFlag::F_LAST as u8)) != 0,
            "a lone Clear is also the last record of its bundle"
        );
    }

    #[test]
    fn rejects_channel_mismatch() {
        let bad = "{\"time\":\"t\",\"raw\":{\"channel\":\"trades\",\
            \"data\":{\"coin\":\"BNB\",\"time\":1,\"levels\":[[],[]]}}}\n";
        let err = reconstruct_books(bad, &spec()).unwrap_err();
        assert!(err.to_string().contains("channel"), "{err}");
    }

    #[test]
    fn rejects_coin_mismatch() {
        let bad = "{\"time\":\"t\",\"raw\":{\"channel\":\"l2Book\",\
            \"data\":{\"coin\":\"ETH\",\"time\":1,\"levels\":[[],[]]}}}\n";
        let err = reconstruct_books(bad, &spec()).unwrap_err();
        assert!(err.to_string().contains("coin"), "{err}");
    }

    #[test]
    fn rejects_non_monotonic_event_time() {
        let bad = "{\"time\":\"a\",\"raw\":{\"channel\":\"l2Book\",\
            \"data\":{\"coin\":\"BNB\",\"time\":1000,\"levels\":[[],[]]}}}\n\
            {\"time\":\"b\",\"raw\":{\"channel\":\"l2Book\",\
            \"data\":{\"coin\":\"BNB\",\"time\":999,\"levels\":[[],[]]}}}\n";
        let err = reconstruct_books(bad, &spec()).unwrap_err();
        assert!(err.to_string().contains("precedes previous"), "{err}");
    }

    // -- node_fills_by_block -> TradeTick ---------------------------------

    fn btc_spec() -> HyperliquidCoreInstrumentSpec {
        HyperliquidCoreInstrumentSpec {
            nt_instrument_id: "BTC.HYPERLIQUID".to_string(),
            raw_symbol: "BTC".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USDC".to_string(),
            settlement_currency: "USDC".to_string(),
        }
    }

    /// Two blocks. Block 1 has one empty + one full BTC trade (maker B + taker A)
    /// plus a non-BTC fill that must be ignored. Block 2 has one BTC trade whose
    /// match time PRECEDES block 1's, proving the global sort. The taker leg is
    /// `crossed:true`; its `side` is the aggressor.
    const TWO_BLOCK_FILLS: &str = "{\"block_number\":1,\"events\":[]}\n\
        {\"block_number\":2,\"events\":[\
        [\"0xaaa\",{\"coin\":\"BTC\",\"px\":\"66428.0\",\"sz\":\"0.00018\",\"side\":\"B\",\"time\":2000,\"crossed\":false,\"tid\":111,\"oid\":1}],\
        [\"0xbbb\",{\"coin\":\"BTC\",\"px\":\"66428.0\",\"sz\":\"0.00018\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":111,\"oid\":2}],\
        [\"0xccc\",{\"coin\":\"HYPE\",\"px\":\"30.988\",\"sz\":\"37.93\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":222,\"oid\":3}]]}\n\
        {\"block_number\":3,\"events\":[\
        [\"0xddd\",{\"coin\":\"BTC\",\"px\":\"66430.5\",\"sz\":\"0.5\",\"side\":\"A\",\"time\":1000,\"crossed\":false,\"tid\":333,\"oid\":4}],\
        [\"0xeee\",{\"coin\":\"BTC\",\"px\":\"66430.5\",\"sz\":\"0.5\",\"side\":\"B\",\"time\":1000,\"crossed\":true,\"tid\":333,\"oid\":5}]]}\n";

    #[test]
    fn reconstructs_trades_dedup_by_tid_keeping_taker() {
        let trades = reconstruct_trades(TWO_BLOCK_FILLS, &btc_spec()).expect("reconstruct trades");
        // Two unique BTC tids (111, 333). HYPE (tid 222) is filtered out by coin.
        assert_eq!(trades.trade_count, 2);
        // Precision derived from data: px max 1 dp (66428.0 / 66430.5), sz max 5 dp.
        assert_eq!(trades.price_precision, 1);
        assert_eq!(trades.size_precision, 5);
        assert_eq!(trades.instrument.id().to_string(), "BTC.HYPERLIQUID");

        // Sorted ascending by event time: tid 333 (time=1000) before tid 111 (2000).
        assert_eq!(
            trades.trades[0].ts_event,
            UnixNanos::from(1000u64 * 1_000_000)
        );
        assert_eq!(
            trades.trades[1].ts_event,
            UnixNanos::from(2000u64 * 1_000_000)
        );
        // tid 333 taker side "B" -> Buyer aggressor.
        assert_eq!(trades.trades[0].aggressor_side, AggressorSide::Buyer);
        // tid 111 taker side "A" -> Seller aggressor.
        assert_eq!(trades.trades[1].aggressor_side, AggressorSide::Seller);
        // The taker print carries the trade id and the shared px/sz.
        assert_eq!(trades.trades[0].trade_id, TradeId::new("333"));
        assert_eq!(trades.trades[0].price, Price::from("66430.5"));
        assert_eq!(trades.trades[0].size, Quantity::from("0.50000"));
        assert_eq!(trades.trades[1].trade_id, TradeId::new("111"));
        // 66428.0 rescaled to derived 1 dp stays 66428.0.
        assert_eq!(trades.trades[1].price, Price::from("66428.0"));
    }

    #[test]
    fn ignores_other_coins() {
        // Only a HYPE taker fill; BTC spec must find nothing.
        let only_hype = "{\"block_number\":1,\"events\":[\
            [\"0x1\",{\"coin\":\"HYPE\",\"px\":\"30.0\",\"sz\":\"1.0\",\"side\":\"B\",\"time\":1,\"crossed\":true,\"tid\":9,\"oid\":1}]]}\n";
        let err = reconstruct_trades(only_hype, &btc_spec()).unwrap_err();
        assert!(err.to_string().contains("no taker fills"), "{err}");
    }

    #[test]
    fn rejects_unknown_taker_side() {
        let bad = "{\"block_number\":1,\"events\":[\
            [\"0x1\",{\"coin\":\"BTC\",\"px\":\"1.0\",\"sz\":\"1.0\",\"side\":\"X\",\"time\":1,\"crossed\":true,\"tid\":9,\"oid\":1}]]}\n";
        let err = reconstruct_trades(bad, &btc_spec()).unwrap_err();
        assert!(err.to_string().contains("unknown taker side"), "{err}");
    }

    #[test]
    fn rejects_non_positive_fill_price() {
        let bad = "{\"block_number\":1,\"events\":[\
            [\"0x1\",{\"coin\":\"BTC\",\"px\":\"0.0\",\"sz\":\"1.0\",\"side\":\"B\",\"time\":1,\"crossed\":true,\"tid\":9,\"oid\":1}]]}\n";
        let err = reconstruct_trades(bad, &btc_spec()).unwrap_err();
        assert!(err.to_string().contains("non-positive fill price"), "{err}");
    }

    #[test]
    fn maker_only_yields_nothing() {
        // A maker leg with no taker pair (defensive): no print is emitted.
        let maker_only = "{\"block_number\":1,\"events\":[\
            [\"0x1\",{\"coin\":\"BTC\",\"px\":\"1.0\",\"sz\":\"1.0\",\"side\":\"B\",\"time\":1,\"crossed\":false,\"tid\":9,\"oid\":1}]]}\n";
        let err = reconstruct_trades(maker_only, &btc_spec()).unwrap_err();
        assert!(err.to_string().contains("no taker fills"), "{err}");
    }

    #[test]
    fn is_core_perp_coin_fences_slash_named_spot_pair() {
        // Hyperliquid names exactly one spot pair by its `BASE/QUOTE` symbol
        // (`PURR/USDC`) rather than an `@`-index; it is spot, not a USDC-settled
        // core perp, so the fence must exclude it alongside the `@` and `:` forms.
        assert!(!is_core_perp_coin("PURR/USDC"));
        // The existing fence arms are unchanged.
        assert!(is_core_perp_coin("BTC"));
        assert!(!is_core_perp_coin("@230"));
        assert!(!is_core_perp_coin("xyz:SILVER"));
    }

    #[test]
    fn hyperliquid_core_fill_coins_excludes_purr_usdc() {
        // One block with a real BTC perp taker (unique tid) and two PURR/USDC
        // spot takers carrying the venue's `tid=0` spot sentinel with distinct
        // sizes. Only BTC is a core perp; the spot pair must never reach per-coin
        // reconstruction, where its many `tid=0` legs trip the perp tid-dedup
        // invariant (the exact RUN2 abort).
        let jsonl = "{\"block_number\":1,\"events\":[\
            [\"0xa\",{\"coin\":\"BTC\",\"px\":\"66428.0\",\"sz\":\"0.001\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":111,\"oid\":1}],\
            [\"0xb\",{\"coin\":\"PURR/USDC\",\"px\":\"0.073241\",\"sz\":\"0.57271\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":0,\"oid\":2}],\
            [\"0xc\",{\"coin\":\"PURR/USDC\",\"px\":\"0.073241\",\"sz\":\"0.77181\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":0,\"oid\":3}]]}\n";
        let coins = hyperliquid_core_fill_coins(jsonl).expect("enumerate coins");
        assert_eq!(coins, vec!["BTC".to_string()]);
    }

    #[test]
    fn perp_tid_collision_still_fails_loud() {
        // Regression guard: the tid-dedup corruption invariant is correct for
        // real perps and must stay intact. Two BTC taker legs share a nonzero tid
        // but disagree on size -> fail loud. The spot fence narrows routing; it
        // does not weaken this guard.
        let corrupt = "{\"block_number\":1,\"events\":[\
            [\"0xa\",{\"coin\":\"BTC\",\"px\":\"66428.0\",\"sz\":\"0.001\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":111,\"oid\":1}],\
            [\"0xb\",{\"coin\":\"BTC\",\"px\":\"66428.0\",\"sz\":\"0.002\",\"side\":\"A\",\"time\":2000,\"crossed\":true,\"tid\":111,\"oid\":2}]]}\n";
        let err = reconstruct_trades(corrupt, &btc_spec()).unwrap_err();
        assert!(err.to_string().contains("disagrees on px/sz/time"), "{err}");
    }
}
