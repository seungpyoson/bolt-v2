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

use std::{path::Path, str::FromStr};

use anyhow::{Context, Result, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{BookOrder, OrderBookDelta},
    enums::{BookAction, OrderSide, RecordFlag},
    identifiers::{InstrumentId, Symbol},
    instruments::{CryptoPerpetual, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

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
}
