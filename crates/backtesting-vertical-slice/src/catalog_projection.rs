//! Gate 3 — NautilusTrader catalog projection.
//!
//! Projects a validated [`CanonicalTradesTable`] into a NautilusTrader
//! `ParquetDataCatalog` as `TradeTick` data plus the venue instrument, using
//! NautilusTrader APIs directly (no custom simulation behaviour), then proves
//! the resolved `bolt-v2` NautilusTrader dependency can read the projection back.
//!
//! The NautilusTrader instrument is built from accepted instrument-universe
//! metadata ([`SpotInstrumentSpec`]); price/size precision and increments
//! are derived from the source tick size and base precision, never hardcoded.

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, TradeTick},
    enums::{AggressorSide, AssetClass, BookAction, OrderSide, RecordFlag},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{BinaryOption, CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use super::{
    canonical_book::{
        BookSide, CanonicalBookEvent, CanonicalBookRow, CanonicalBookTable, LevelChange,
    },
    canonical_trades::{CanonicalTradesTable, TradeAggressorSide},
    source_proof::SourceProofFidelityClass,
};

/// NautilusTrader data type written for the trade projection.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the L2 order-book projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// Accepted Bybit spot instrument metadata needed to build the NautilusTrader
/// `CurrencyPair`. Built from the accepted instrument-universe payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpotInstrumentSpec {
    /// NautilusTrader instrument id, for example `BNBUSDC.BYBIT`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol, for example `BNBUSDC`.
    pub raw_symbol: String,
    /// Base currency code, for example `BNB`.
    pub base_currency: String,
    /// Quote currency code, for example `USDC`.
    pub quote_currency: String,
    /// Price tick size as a decimal string, for example `0.1`.
    pub price_increment: String,
    /// Base size precision as a decimal string, for example `0.0001`.
    pub size_increment: String,
    /// Minimum order quantity decimal string.
    pub min_quantity: String,
    /// Maximum order quantity decimal string.
    pub max_quantity: String,
    /// Minimum order notional decimal string (quote currency).
    pub min_notional: String,
    /// Maximum order notional decimal string (quote currency).
    pub max_notional: String,
}

/// Accepted Polymarket binary-outcome instrument metadata needed to build the
/// NautilusTrader `BinaryOption` for one outcome token. Built from the accepted
/// instrument-universe payload; precision is derived from the source tick size,
/// never hardcoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinaryOptionInstrumentSpec {
    /// NautilusTrader instrument id, for example `<asset_id>.POLYMARKET`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol (the outcome token id).
    pub raw_symbol: String,
    /// `AssetClass` token (`SCREAMING_SNAKE_CASE`), for example `ALTERNATIVE`.
    pub asset_class: String,
    /// Settlement/quote currency code, for example `USDC`.
    pub quote_currency: String,
    /// Free-form outcome label, for example `Up` / `Yes`.
    pub outcome: String,
    /// Instrument activation timestamp (Unix nanoseconds).
    pub activation_ns: u64,
    /// Instrument expiration timestamp (Unix nanoseconds).
    pub expiration_ns: u64,
    /// Source price tick size as a decimal string, for example `0.01`.
    pub price_increment: String,
    /// Source size increment as a decimal string, for example `0.01`.
    pub size_increment: String,
}

/// Result of projecting a canonical L2 book table into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogBookProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    /// Count of written `OrderBookDelta` records (snapshot expansions + deltas).
    pub delta_count: usize,
    /// Count of written `TradeTick` records (trade prints).
    pub trade_count: usize,
    /// Deterministic SHA-256 hex over the catalog's written data files.
    pub catalog_hash: String,
    pub fidelity_class: SourceProofFidelityClass,
}

/// Result of projecting canonical trades into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub trade_count: usize,
    /// Deterministic SHA-256 hex over the catalog's written data files.
    pub catalog_hash: String,
    pub fidelity_class: SourceProofFidelityClass,
}

/// Decimal places implied by a decimal-string increment (`0.1` -> 1,
/// `0.0001` -> 4, `0.10` -> 2, `1.00` -> 2, `1400` -> 0).
///
/// Trailing zeros are significant: an exchange increment of `0.10` declares two
/// decimal places, and trimming them would understate the precision and
/// disagree with the precision NautilusTrader infers from the same increment
/// string in `Price::from_str`/`Quantity::from_str`.
#[must_use]
fn decimal_places(increment: &str) -> u8 {
    match increment.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    }
}

/// Build the NautilusTrader `CurrencyPair` from accepted instrument metadata.
///
/// # Errors
///
/// Returns an error if any decimal field fails to parse.
pub fn build_currency_pair(spec: &SpotInstrumentSpec) -> Result<CurrencyPair> {
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let price_precision = decimal_places(&spec.price_increment);
    let size_precision = decimal_places(&spec.size_increment);
    let base_currency = Currency::from_str(&spec.base_currency)
        .with_context(|| format!("invalid base_currency {:?}", spec.base_currency))?;
    let quote_currency = Currency::from_str(&spec.quote_currency)
        .with_context(|| format!("invalid quote_currency {:?}", spec.quote_currency))?;
    let price_increment = Price::from_str(&spec.price_increment).map_err(|error| {
        anyhow::anyhow!(
            "invalid price_increment {:?}: {error}",
            spec.price_increment
        )
    })?;
    let size_increment = Quantity::from_str(&spec.size_increment).map_err(|error| {
        anyhow::anyhow!("invalid size_increment {:?}: {error}", spec.size_increment)
    })?;
    let max_quantity = Quantity::from_str(&spec.max_quantity).map_err(|error| {
        anyhow::anyhow!("invalid max_quantity {:?}: {error}", spec.max_quantity)
    })?;
    let min_quantity = Quantity::from_str(&spec.min_quantity).map_err(|error| {
        anyhow::anyhow!("invalid min_quantity {:?}: {error}", spec.min_quantity)
    })?;

    Ok(CurrencyPair::new(
        instrument_id,
        Symbol::from(spec.raw_symbol.as_str()),
        base_currency,
        quote_currency,
        price_precision,
        size_precision,
        price_increment,
        size_increment,
        None,
        None,
        Some(max_quantity),
        Some(min_quantity),
        Some(Money::new(
            spec.max_notional.parse().context("max_notional")?,
            quote_currency,
        )),
        Some(Money::new(
            spec.min_notional.parse().context("min_notional")?,
            quote_currency,
        )),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    // Source decimals carry trailing-zero padding from their storage scale (a
    // Parquet `Decimal128(9,4)` price renders `0.07` as `"0.0700"`). Tolerate
    // that padding: only the SIGNIFICANT scale (trailing zeros stripped) must
    // fit the instrument precision, so `"0.0700"` is admissible at precision 2
    // while a genuine `"0.071"` is not.
    ensure!(
        decimal.normalize().scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Convert canonical trade rows into NautilusTrader `TradeTick`s at the
/// instrument's price/size precision.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn canonical_rows_to_trade_ticks(
    table: &CanonicalTradesTable,
    instrument: &CurrencyPair,
) -> Result<Vec<TradeTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| {
            let price_str = rescaled(&row.price, price_precision)?;
            let price = Price::from_str(&price_str).map_err(|error| {
                anyhow::anyhow!("invalid rescaled price {price_str:?}: {error}")
            })?;
            let size_str = rescaled(&row.size, size_precision)?;
            let size = Quantity::from_str(&size_str)
                .map_err(|error| anyhow::anyhow!("invalid rescaled size {size_str:?}: {error}"))?;
            let aggressor = match row.aggressor_side.as_str() {
                s if s == TradeAggressorSide::Buyer.as_str() => AggressorSide::Buyer,
                s if s == TradeAggressorSide::Seller.as_str() => AggressorSide::Seller,
                other => anyhow::bail!("unknown aggressor side {other:?}"),
            };
            let ts = UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
            Ok(TradeTick::new(
                instrument_id,
                price,
                size,
                aggressor,
                TradeId::from(row.trade_id.as_str()),
                ts,
                ts,
            ))
        })
        .collect()
}

/// Project a canonical trades table into a NautilusTrader `ParquetDataCatalog`.
///
/// Writes the venue instrument and the `TradeTick` projection under
/// `catalog_root`, then returns a [`CatalogProjection`] with a deterministic
/// catalog hash. NautilusTrader writes its native
/// `data/<data_type>/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_canonical_trades_to_catalog(
    table: &CanonicalTradesTable,
    spec: &SpotInstrumentSpec,
    catalog_root: &Path,
) -> Result<CatalogProjection> {
    table.validate()?;
    let instrument = build_currency_pair(spec)?;
    let instrument_id = instrument.id();
    ensure!(
        instrument_id.to_string() == table.rows[0].nt_instrument_id,
        "instrument id {instrument_id} does not match canonical rows {}",
        table.rows[0].nt_instrument_id
    );
    let ticks = canonical_rows_to_trade_ticks(table, &instrument)?;
    let trade_count = ticks.len();

    // Fail closed on a dirty catalog root. NautilusTrader's `write_to_parquet`
    // skips writing when a file for the same instrument/interval already exists,
    // so projecting into a non-empty root could silently read back stale data
    // under this run's source proof and a stale catalog hash. The caller owns
    // the output lifecycle and must hand us a clean (absent or empty) root.
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CurrencyPair(instrument)])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        trade_count,
        catalog_hash: catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `TradeTick` data back from `catalog_root`.
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

/// Build the NautilusTrader `BinaryOption` from accepted instrument metadata.
///
/// Price/size precision is derived from the source tick/size increment strings,
/// never hardcoded (Polymarket's `0.01` tick implies precision 2). The
/// quote/settlement currency, outcome label, and lifecycle timestamps come from
/// the accepted instrument-universe payload.
///
/// # Errors
///
/// Returns an error if any field fails to parse.
pub fn build_binary_option(spec: &BinaryOptionInstrumentSpec) -> Result<BinaryOption> {
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let asset_class = AssetClass::from_str(&spec.asset_class)
        .map_err(|error| anyhow::anyhow!("invalid asset_class {:?}: {error}", spec.asset_class))?;
    let currency = Currency::from_str(&spec.quote_currency)
        .with_context(|| format!("invalid quote_currency {:?}", spec.quote_currency))?;
    let price_increment = Price::from_str(&spec.price_increment).map_err(|error| {
        anyhow::anyhow!(
            "invalid price_increment {:?}: {error}",
            spec.price_increment
        )
    })?;
    let size_increment = Quantity::from_str(&spec.size_increment).map_err(|error| {
        anyhow::anyhow!("invalid size_increment {:?}: {error}", spec.size_increment)
    })?;
    Ok(BinaryOption::new(
        instrument_id,
        Symbol::from(spec.raw_symbol.as_str()),
        asset_class,
        currency,
        UnixNanos::from(spec.activation_ns),
        UnixNanos::from(spec.expiration_ns),
        price_increment.precision,
        size_increment.precision,
        price_increment,
        size_increment,
        Some(Ustr::from(&spec.outcome)),
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
    ))
}

/// Rescale and parse a canonical decimal string to a NautilusTrader `Price` at
/// the instrument's price precision.
fn book_price(value: &str, precision: u8) -> Result<Price> {
    let rescaled = rescaled(value, precision)?;
    Price::from_str(&rescaled)
        .map_err(|error| anyhow::anyhow!("invalid rescaled price {rescaled:?}: {error}"))
}

/// Rescale and parse a canonical decimal string to a NautilusTrader `Quantity`
/// at the instrument's size precision.
fn book_size(value: &str, precision: u8) -> Result<Quantity> {
    let rescaled = rescaled(value, precision)?;
    Quantity::from_str(&rescaled)
        .map_err(|error| anyhow::anyhow!("invalid rescaled size {rescaled:?}: {error}"))
}

/// Map a canonical `BookSide` to the NautilusTrader `OrderSide`: a Polymarket
/// `BUY` is bid-side liquidity, a `SELL` is ask-side liquidity.
const fn book_order_side(side: BookSide) -> OrderSide {
    match side {
        BookSide::Buy => OrderSide::Buy,
        BookSide::Sell => OrderSide::Sell,
    }
}

/// Map a canonical `BookSide` to the NautilusTrader `AggressorSide` for trade
/// prints: a `BUY` print is a buyer-aggressed trade, a `SELL` a seller-aggressed
/// one.
const fn book_aggressor_side(side: BookSide) -> AggressorSide {
    match side {
        BookSide::Buy => AggressorSide::Buyer,
        BookSide::Sell => AggressorSide::Seller,
    }
}

/// Build one `OrderBookDelta` for a single-level update from a `price_change`
/// row: `size == 0` removes the level (`Delete`), any other size updates it
/// (`Update`). NautilusTrader's `OrderBook` treats `Update` on an unseen level
/// as an insert, so a single `Update` action faithfully replays both new and
/// changed levels.
fn level_change_delta(
    instrument_id: InstrumentId,
    change: &LevelChange,
    price_precision: u8,
    size_precision: u8,
    sequence: u64,
    ts: UnixNanos,
) -> Result<OrderBookDelta> {
    let side = book_order_side(change.side);
    let price = book_price(&change.price, price_precision)?;
    let size = book_size(&change.size, size_precision)?;
    let action = if change.is_removal {
        BookAction::Delete
    } else {
        BookAction::Update
    };
    // The `order_id` is unused by an L2 (MBP) book — NautilusTrader keys levels
    // by price on `L2_MBP` and the delta carries F_MBP — so it is fixed at 0.
    let order = BookOrder::new(side, price, size, 0);
    // A single-level update is a standalone, aggregated price-level (MBP) event;
    // it both opens and closes its own event, so F_MBP and F_LAST are set.
    Ok(OrderBookDelta::new(
        instrument_id,
        action,
        order,
        RecordFlag::F_MBP as u8 | RecordFlag::F_LAST as u8,
        sequence,
        ts,
        ts,
    ))
}

/// Convert one canonical `Snapshot` row into an `OrderBookDelta` sequence: one
/// `Clear`, then one `Add` per bid level and per ask level. Every expanded delta
/// carries the snapshot's `event_time` as both `ts_event` and `ts_init`
/// (NautilusTrader's `write_to_parquet` allows equal `ts_init` within an
/// expansion); levels are emitted in source order so the expansion is
/// deterministic. `sequence` advances per emitted delta and is returned for the
/// next row.
fn snapshot_deltas(
    instrument_id: InstrumentId,
    snapshot: &super::canonical_book::BookSnapshot,
    price_precision: u8,
    size_precision: u8,
    ts: UnixNanos,
    mut sequence: u64,
    out: &mut Vec<OrderBookDelta>,
) -> Result<u64> {
    // Every delta in a snapshot expansion is replayed-snapshot (F_SNAPSHOT) and
    // aggregated price-level (F_MBP) data. NautilusTrader's `clear()` only sets
    // F_SNAPSHOT, so OR in F_MBP to mark the whole expansion as MBP.
    let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
    let mut clear = OrderBookDelta::clear(instrument_id, sequence, ts, ts);
    clear.flags |= RecordFlag::F_MBP as u8;
    out.push(clear);
    sequence = sequence.checked_add(1).context("delta sequence overflow")?;
    for (side, levels) in [
        (OrderSide::Buy, &snapshot.bids),
        (OrderSide::Sell, &snapshot.asks),
    ] {
        for level in levels {
            let price = book_price(&level.price, price_precision)?;
            let size = book_size(&level.size, size_precision)?;
            // order_id is 0 for L2/MBP — levels are price-keyed (F_MBP set).
            let order = BookOrder::new(side, price, size, 0);
            out.push(OrderBookDelta::new(
                instrument_id,
                BookAction::Add,
                order,
                snapshot_flags,
                sequence,
                ts,
                ts,
            ));
            sequence = sequence.checked_add(1).context("delta sequence overflow")?;
        }
    }
    // Close the snapshot event: the final expanded delta carries F_LAST.
    if let Some(last) = out.last_mut() {
        last.flags |= RecordFlag::F_LAST as u8;
    }
    Ok(sequence)
}

/// Convert canonical L2 book rows into NautilusTrader `OrderBookDelta`s.
///
/// `Snapshot` rows expand to a `Clear` plus an `Add` per level; `LevelChange`
/// rows map to one `Update`/`Delete` delta. `Trade` rows are skipped here — they
/// are routed through the existing `TradeTick` projection. The returned deltas
/// carry a dense monotonic `sequence` and non-strict ascending `ts_init`,
/// matching NautilusTrader's catalog write contract.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision or the delta sequence overflows.
pub fn canonical_rows_to_order_book_deltas(
    table: &CanonicalBookTable,
    instrument: &BinaryOption,
) -> Result<Vec<OrderBookDelta>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    let mut deltas = Vec::new();
    let mut sequence: u64 = 0;
    for row in &table.rows {
        let ts = book_event_ts(row)?;
        match &row.event {
            CanonicalBookEvent::Snapshot(snapshot) => {
                sequence = snapshot_deltas(
                    instrument_id,
                    snapshot,
                    price_precision,
                    size_precision,
                    ts,
                    sequence,
                    &mut deltas,
                )?;
            }
            CanonicalBookEvent::LevelChange(change) => {
                deltas.push(level_change_delta(
                    instrument_id,
                    change,
                    price_precision,
                    size_precision,
                    sequence,
                    ts,
                )?);
                sequence = sequence.checked_add(1).context("delta sequence overflow")?;
            }
            CanonicalBookEvent::Trade(_) => {}
        }
    }
    Ok(deltas)
}

/// Trade-id prefix for book-table trade prints whose source `transaction_hash`
/// is absent or too long for NautilusTrader's `TradeId`. The dense canonical
/// `source_sequence` (unique per print within the run) follows this prefix.
const BOOK_TRADE_ID_PREFIX: &str = "POLYCLOB-";

/// NautilusTrader's `TradeId` maximum length (see `model::identifiers::TradeId`).
/// A source `transaction_hash` is used verbatim as the trade id only when it
/// fits; a full 66-char on-chain hash exceeds this and falls back to the
/// `source_sequence`-derived id.
const NT_TRADE_ID_MAX_LEN: usize = 36;

/// Convert the `Trade` rows of a canonical L2 book table into NautilusTrader
/// `TradeTick`s, reusing the same precision/aggressor mapping as the native
/// trade projection.
///
/// The NautilusTrader `TradeId` is the source `transaction_hash` when it is
/// present and within NautilusTrader's 36-char `TradeId` limit; otherwise it is
/// the dense canonical `source_sequence` (`POLYCLOB-<sequence>`). Polymarket's
/// CLOB archive leaves `transaction_hash` null on most trade prints and full
/// 66-char on-chain hashes exceed the limit, so the `source_sequence`-derived id
/// is the common case; a missing hash is never an error.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn canonical_book_rows_to_trade_ticks(
    table: &CanonicalBookTable,
    instrument: &BinaryOption,
) -> Result<Vec<TradeTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    let mut ticks = Vec::new();
    for row in &table.rows {
        let CanonicalBookEvent::Trade(trade) = &row.event else {
            continue;
        };
        let ts = book_event_ts(row)?;
        let hash = trade.transaction_hash.trim();
        let trade_id = if !hash.is_empty() && hash.len() <= NT_TRADE_ID_MAX_LEN {
            hash.to_string()
        } else {
            format!("{BOOK_TRADE_ID_PREFIX}{}", row.source_sequence)
        };
        ticks.push(TradeTick::new(
            instrument_id,
            book_price(&trade.price, price_precision)?,
            book_size(&trade.size, size_precision)?,
            book_aggressor_side(trade.side),
            TradeId::from(trade_id.as_str()),
            ts,
            ts,
        ));
    }
    Ok(ticks)
}

/// Convert a canonical row's `event_time` (Unix nanoseconds) into `UnixNanos`.
fn book_event_ts(row: &CanonicalBookRow) -> Result<UnixNanos> {
    let nanos = u64::try_from(row.event_time)
        .with_context(|| format!("row {}: negative event_time", row.source_sequence))?;
    Ok(UnixNanos::from(nanos))
}

/// Project a canonical L2 book table into a NautilusTrader `ParquetDataCatalog`.
///
/// Writes the binary-option instrument, the `OrderBookDelta` projection, and the
/// `TradeTick` projection (trade prints) under `catalog_root`, then returns a
/// [`CatalogBookProjection`] with a deterministic catalog hash. Both data types
/// share the one catalog so a single backtest can replay book deltas (driving
/// quote derivation) and trade prints for the same instrument.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_book_to_catalog(
    table: &CanonicalBookTable,
    spec: &BinaryOptionInstrumentSpec,
    catalog_root: &Path,
) -> Result<CatalogBookProjection> {
    table.validate()?;
    let instrument = build_binary_option(spec)?;
    let instrument_id = instrument.id();
    ensure!(
        instrument_id.to_string() == spec.nt_instrument_id,
        "instrument id {instrument_id} does not match spec {}",
        spec.nt_instrument_id
    );
    let deltas = canonical_rows_to_order_book_deltas(table, &instrument)?;
    let ticks = canonical_book_rows_to_trade_ticks(table, &instrument)?;
    let delta_count = deltas.len();
    let trade_count = ticks.len();

    // Fail closed on a dirty catalog root: NautilusTrader's `write_to_parquet`
    // appends, so projecting into a non-empty root could silently mix stale data
    // under this run's source proof and a stale catalog hash. The caller owns the
    // output lifecycle and must hand us a clean (absent or empty) root.
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::BinaryOption(instrument)])
        .context("write binary-option instrument to catalog")?;
    catalog
        .write_to_parquet(deltas, None, None, None)
        .context("write order book deltas to catalog")?;
    if !ticks.is_empty() {
        catalog
            .write_to_parquet(ticks, None, None, None)
            .context("write trade ticks to catalog")?;
    }

    Ok(CatalogBookProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        delta_count,
        trade_count,
        catalog_hash: catalog_hash(catalog_root)?,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `OrderBookDelta` data back from `catalog_root`.
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

/// Deterministic SHA-256 hex over every file under `root`, ordered by relative
/// path, mixing in each relative path so renames change the hash.
fn catalog_hash(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for relative in files {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        let bytes = fs::read(root.join(&relative))
            .with_context(|| format!("read catalog file {}", relative.display()))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        canonical_trades::{CanonicalInstrumentIdentity, normalize_bybit_spot_tick_trades},
        source_proof::{
            AcceptanceMode, AcceptedDataset, EvidenceState, FixtureType,
            IngestManifestObjectRecord, NtMappingStatus, RequiredCheck, RequiredChecks,
            SourceProofReport, SourceProofStatus, TimeRange, select_accepted_dataset,
        },
    };

    fn spec() -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
            raw_symbol: "BNBUSDC".to_string(),
            base_currency: "BNB".to_string(),
            quote_currency: "USDC".to_string(),
            price_increment: "0.1".to_string(),
            size_increment: "0.0001".to_string(),
            min_quantity: "0.0001".to_string(),
            max_quantity: "1400".to_string(),
            min_notional: "5".to_string(),
            max_notional: "200000".to_string(),
        }
    }

    fn accepted_dataset() -> AcceptedDataset {
        let checks = RequiredChecks {
            source_access: RequiredCheck::passed("manifest"),
            license: RequiredCheck::passed("attestation"),
            schema: RequiredCheck::passed("schema"),
            time_semantics: RequiredCheck::passed("ms_to_nanos"),
            instrument_universe: RequiredCheck::passed("universe"),
            coverage: RequiredCheck::passed("manifest"),
            granularity: RequiredCheck::passed("native"),
            completeness: RequiredCheck::passed("manifest"),
            nt_mapping: RequiredCheck::passed("TradeTick"),
            storage: RequiredCheck::passed("artifact_root"),
        };
        let object = IngestManifestObjectRecord {
            s3_uri: "s3://bolt-parquet/.../symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
            source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz"
                .to_string(),
            sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            bytes: 8505,
            archive_date: "2026-03-01".to_string(),
            schema_columns: vec![
                "id".to_string(),
                "timestamp".to_string(),
                "price".to_string(),
                "volume".to_string(),
                "side".to_string(),
                "rpi".to_string(),
            ],
        };
        let proof = SourceProofReport {
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            contract_version: "backfill-table-contract.v1".to_string(),
            schema_version: "backfill-source-proof.v1".to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "bybit-spot-tick-trades".to_string(),
            venue: "bybit".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            table_family: "trades".to_string(),
            evidence_state: EvidenceState::OwnerArchiveBackfillable,
            fixture_type: FixtureType::PerpsSpot,
            requested_time_range: TimeRange {
                start_utc: "2025-06-01T00:00:00Z".to_string(),
                end_utc: "2026-06-01T00:00:00Z".to_string(),
            },
            coverage_time_range: TimeRange {
                start_utc: "2026-03-01T00:00:00Z".to_string(),
                end_utc: "2026-03-02T00:00:00Z".to_string(),
            },
            instrument_universe_id: "bybit-spot-instruments-2026-03-01".to_string(),
            raw_sample_uri: object.s3_uri.clone(),
            raw_sample_hash: object.sha256.clone(),
            schema_sample_uri: "s3://.../schema.json".to_string(),
            schema_sample_hash: "bf26db".to_string(),
            license_ref: "https://public.bybit.com/ (attestation)".to_string(),
            retention_ref: "https://public.bybit.com/".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            gap_policy_id: String::new(),
            required_checks: checks,
            acceptance_mode: None,
            accepted_by: None,
            accepted_at: None,
            supersedes_source_proof_id: None,
        }
        .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
        .unwrap();
        select_accepted_dataset(&proof, &object, &object.sha256).unwrap()
    }

    const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
        1,1772323201665,617.2,0.3,buy,0\n\
        2,1772323312219,617.9,0.1456,sell,0\n\
        3,1772323312236,617,0.1544,sell,0\n";

    fn canonical_table() -> CanonicalTradesTable {
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        normalize_bybit_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            SAMPLE_CSV,
            42,
            "ingest-run-test",
        )
        .unwrap()
    }

    #[test]
    fn decimal_places_reads_increment_precision() {
        assert_eq!(decimal_places("0.1"), 1);
        assert_eq!(decimal_places("0.0001"), 4);
        assert_eq!(decimal_places("1400"), 0);
        // Trailing zeros are significant: an exchange tick of `0.10` is two
        // decimal places, matching the precision `Price::from_str` infers.
        assert_eq!(decimal_places("0.10"), 2);
        assert_eq!(decimal_places("1.00"), 2);
    }

    #[test]
    fn rescaled_tolerates_trailing_zero_padding() {
        // Source Parquet decimals render at their storage scale, so `0.07`
        // arrives as `"0.0700"`. Rescaling to the instrument precision (2) must
        // succeed and drop only the padding.
        assert_eq!(rescaled("0.0700", 2).unwrap(), "0.07");
        assert_eq!(rescaled("0.5100", 2).unwrap(), "0.51");
        assert_eq!(rescaled("14926.030000", 2).unwrap(), "14926.03");
    }

    #[test]
    fn rescaled_rejects_genuine_subprecision() {
        // A non-zero digit below the instrument precision is a real precision
        // loss and must be refused, not silently rounded.
        let err = rescaled("0.071", 2).expect_err("sub-precision must be refused");
        assert!(err.to_string().contains("more precision"), "{err}");
    }

    #[test]
    fn build_currency_pair_honours_trailing_zero_increment() {
        let mut spec = spec();
        spec.price_increment = "0.10".to_string();
        let instrument = build_currency_pair(&spec).expect("build instrument");
        // Precision derived from the increment must agree with the increment's
        // own precision, or `CurrencyPair::new` would carry mismatched scales.
        assert_eq!(instrument.price_precision(), 2);
    }

    #[test]
    fn build_currency_pair_rejects_malformed_decimal() {
        let mut spec = spec();
        spec.price_increment = "not-a-number".to_string();
        assert!(build_currency_pair(&spec).is_err());
    }

    #[test]
    fn builds_currency_pair_from_accepted_spec() {
        let instrument = build_currency_pair(&spec()).expect("build instrument");
        assert_eq!(instrument.id().to_string(), "BNBUSDC.BYBIT");
        assert_eq!(instrument.price_precision(), 1);
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn projects_and_reads_back_trade_ticks() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_canonical_trades_to_catalog(&table, &spec(), dir.path()).expect("project");
        assert_eq!(projection.trade_count, 3);
        assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_trade_ticks(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].instrument_id.to_string(), "BNBUSDC.BYBIT");
        // 617 rescaled to price precision 1 -> 617.0
        assert_eq!(loaded[2].price, Price::from("617.0"));
    }

    #[test]
    fn projection_refuses_dirty_catalog_root() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_trades_to_catalog(&table, &spec(), dir.path())
            .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn catalog_hash_is_deterministic_across_roots() {
        let table = canonical_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_trades_to_catalog(&table, &spec(), dir_a.path()).unwrap();
        let b = project_canonical_trades_to_catalog(&table, &spec(), dir_b.path()).unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same data must hash identically regardless of root"
        );
    }

    #[test]
    fn catalog_hash_is_relative_path_sensitive() {
        // Identical file bytes under different relative paths must hash
        // differently, proving the path is mixed into the digest.
        let root_a = tempfile::TempDir::new().unwrap();
        let root_b = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(root_a.path().join("data/alpha")).unwrap();
        fs::write(root_a.path().join("data/alpha/file.parquet"), b"identical").unwrap();
        fs::create_dir_all(root_b.path().join("data/beta")).unwrap();
        fs::write(root_b.path().join("data/beta/file.parquet"), b"identical").unwrap();
        assert_ne!(
            catalog_hash(root_a.path()).unwrap(),
            catalog_hash(root_b.path()).unwrap(),
            "identical bytes under different relative paths must hash differently"
        );
    }
}
