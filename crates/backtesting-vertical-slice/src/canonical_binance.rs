//! Binance venue converter — canonical normalization + NautilusTrader catalog
//! projection for the two Binance public-archive market-data families that have
//! no order book: native trade prints and OHLCV klines.
//!
//! ```text
//! data.binance.vision .zip  (unzipped to CSV by the ingest step)
//!   -> canonical normalized row (full provenance)
//!     -> NautilusTrader type (TradeTick for trades, Bar for klines)
//!       -> ParquetDataCatalog::write_to_parquet
//!         -> query_typed_data read-back (proves NT can replay it)
//! ```
//!
//! This module is the self-contained Binance slice of the multi-venue
//! "convert all venue data to NT-backtestable catalogs" effort. It mirrors the
//! TradeTick projection pattern of [`super::canonical_trades`] /
//! [`super::catalog_projection`] and additionally covers the Bar family that
//! klines map onto.
//!
//! Provenance, instrument identity, instrument precision, and the kline bar
//! specification are all supplied by the caller from accepted run/config inputs
//! — nothing venue- or instrument-specific is hardcoded here. The only literals
//! are the immutable physical facts of the Binance public-archive CSV layout
//! (column order, timestamp unit, the `is_buyer_maker` aggressor convention).
//!
//! Binance public-archive CSV facts encoded here (verified against
//! `s3://bolt-parquet/backfill-staging/.../source=data.binance.vision`):
//!
//! - Both `trades` and `klines` CSVs are **headerless**.
//! - `trades` columns: `id, price, qty, quote_qty, time, is_buyer_maker,
//!   is_best_match`. `time` is microseconds since the Unix epoch.
//! - `is_buyer_maker = True` means the buyer was the resting maker, so the
//!   trade was **seller-initiated** (aggressor = SELLER). `False` means the
//!   trade was buyer-initiated (aggressor = BUYER).
//! - `klines` columns: `open_time, open, high, low, close, volume, close_time,
//!   quote_volume, count, taker_buy_base, taker_buy_quote, ignore`. Both
//!   `open_time` and `close_time` are microseconds since the Unix epoch.

use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Bar, BarSpecification, BarType, TradeTick},
    enums::{AggregationSource, AggressorSide, BarAggregation, PriceType},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for the trades family.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the klines family.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// Number of comma-separated fields in a Binance public-archive `trades` row.
pub const BINANCE_TRADES_FIELD_COUNT: usize = 7;

/// Number of comma-separated fields in a Binance public-archive `klines` row.
pub const BINANCE_KLINES_FIELD_COUNT: usize = 12;

/// Binance public-archive timestamps are microseconds since the Unix epoch.
const NANOS_PER_MICROSECOND: i64 = 1_000;

// ---------------------------------------------------------------------------
// Shared inputs (caller-provided; never hardcoded)
// ---------------------------------------------------------------------------

/// Provenance recorded on every normalized row, supplied by the ingest run.
///
/// Mirrors the identity/provenance columns of the shared backfill table
/// contract without coupling this venue slice to the source-proof acceptance
/// machinery (left for a later integration pass).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceProvenance {
    /// Stable identifier of the ingest/run that produced this normalization.
    pub ingest_run_id: String,
    /// Source binding id (for example the manifest source binding).
    pub source_binding: String,
    /// Venue token (for example `binance`).
    pub venue: String,
    /// Product family (for example `spot`).
    pub product_family: String,
    /// Product category.
    pub product_category: String,
    /// Accepted source-proof id this object was admitted under.
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Archive partition date string (for example `2026-04`).
    pub archive_date: String,
}

impl BinanceProvenance {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("ingest_run_id", &self.ingest_run_id),
            ("source_binding", &self.source_binding),
            ("venue", &self.venue),
            ("product_family", &self.product_family),
            ("product_category", &self.product_category),
            ("source_proof_id", &self.source_proof_id),
            ("payload_hash", &self.payload_hash),
            ("archive_date", &self.archive_date),
        ] {
            ensure!(!value.trim().is_empty(), "empty provenance field: {name}");
        }
        Ok(())
    }
}

/// Venue-native instrument identity for normalized rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceInstrumentIdentity {
    /// Venue-native instrument id (for example `XRPTUSD`).
    pub instrument_id: String,
    /// Display or wire symbol from the source.
    pub venue_symbol: String,
    /// NautilusTrader instrument id (for example `XRPTUSD.BINANCE`).
    pub nt_instrument_id: String,
}

impl BinanceInstrumentIdentity {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("instrument_id", &self.instrument_id),
            ("venue_symbol", &self.venue_symbol),
            ("nt_instrument_id", &self.nt_instrument_id),
        ] {
            ensure!(!value.trim().is_empty(), "empty identity field: {name}");
        }
        Ok(())
    }
}

/// Accepted Binance spot-instrument metadata needed to build the NautilusTrader
/// `CurrencyPair`. Built by the caller from the accepted instrument universe;
/// precision is derived from the increment strings, never hardcoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceInstrumentSpec {
    /// NautilusTrader instrument id, for example `XRPTUSD.BINANCE`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol, for example `XRPTUSD`.
    pub raw_symbol: String,
    /// Base currency code, for example `XRP`.
    pub base_currency: String,
    /// Quote currency code, for example `TUSD`.
    pub quote_currency: String,
    /// Price tick size as a decimal string, for example `0.00000001`.
    pub price_increment: String,
    /// Base size precision as a decimal string, for example `0.00000001`.
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

/// Kline bar specification supplied by the caller from the `interval=` archive
/// partition (for example `1m` -> step 1, [`BarAggregation::Minute`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KlineBarSpec {
    pub step: usize,
    pub aggregation: BarAggregation,
}

impl KlineBarSpec {
    fn to_bar_type(self, instrument_id: InstrumentId) -> Result<BarType> {
        ensure!(self.step > 0, "kline bar step must be positive");
        // Klines are aggregated by the exchange, outside the Nautilus boundary,
        // so they replay as `EXTERNAL`-sourced, `LAST`-price bars.
        let spec = BarSpecification::new(self.step, self.aggregation, PriceType::Last);
        Ok(BarType::new(
            instrument_id,
            spec,
            AggregationSource::External,
        ))
    }
}

// ---------------------------------------------------------------------------
// Aggressor side (Binance `is_buyer_maker` convention)
// ---------------------------------------------------------------------------

/// Aggressor side of a native trade print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeAggressorSide {
    Buyer,
    Seller,
}

impl TradeAggressorSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Seller => "SELLER",
        }
    }

    /// Map Binance `is_buyer_maker` to the aggressor side.
    ///
    /// `True` -> buyer is the resting maker -> seller-initiated (SELLER).
    /// `False` -> seller is the resting maker -> buyer-initiated (BUYER).
    fn from_is_buyer_maker(raw: &str) -> Result<Self> {
        match raw.trim() {
            "True" | "true" => Ok(Self::Seller),
            "False" | "false" => Ok(Self::Buyer),
            other => bail!("unknown is_buyer_maker token: {other:?}"),
        }
    }

    fn to_nt(self) -> AggressorSide {
        match self {
            Self::Buyer => AggressorSide::Buyer,
            Self::Seller => AggressorSide::Seller,
        }
    }
}

// ---------------------------------------------------------------------------
// Canonical normalized rows
// ---------------------------------------------------------------------------

/// One normalized native-trade row with full provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceTradeRow {
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub venue_symbol: String,
    pub nt_instrument_id: String,
    /// Exchange event timestamp in Unix nanoseconds.
    pub event_time: i64,
    pub source_proof_id: String,
    pub payload_hash: String,
    pub trade_id: String,
    pub aggressor_side: TradeAggressorSide,
    /// Exact source price string.
    pub price: String,
    /// Exact source size string.
    pub size: String,
}

/// One normalized OHLCV bar row with full provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceBarRow {
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub venue_symbol: String,
    pub nt_instrument_id: String,
    /// Bar open (event) timestamp in Unix nanoseconds.
    pub open_time: i64,
    /// Bar close timestamp in Unix nanoseconds.
    pub close_time: i64,
    pub source_proof_id: String,
    pub payload_hash: String,
    /// Exact source OHLC + volume strings.
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

/// A validated canonical normalized Binance `trades` table for one object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceTradesTable {
    pub provenance: BinanceProvenance,
    pub identity: BinanceInstrumentIdentity,
    pub rows: Vec<BinanceTradeRow>,
}

/// A validated canonical normalized Binance `klines` table for one object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BinanceKlinesTable {
    pub provenance: BinanceProvenance,
    pub identity: BinanceInstrumentIdentity,
    pub bar_spec: KlineBarSpec,
    pub rows: Vec<BinanceBarRow>,
}

// ---------------------------------------------------------------------------
// Trades normalization
// ---------------------------------------------------------------------------

/// Normalize a decompressed Binance public-archive `trades` CSV into the
/// canonical trades table.
///
/// `csv_text` is the decompressed text of the accepted `.zip` object (the
/// unzip is the ingest step). The file is headerless.
///
/// # Errors
///
/// Returns an error if a row is malformed, a field fails to parse, a price/size
/// is non-positive, or event timestamps are not monotonically non-decreasing.
pub fn normalize_binance_trades(
    provenance: &BinanceProvenance,
    identity: &BinanceInstrumentIdentity,
    csv_text: &str,
) -> Result<BinanceTradesTable> {
    provenance.validate()?;
    identity.validate()?;

    let mut rows = Vec::new();
    for (index, line) in csv_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == BINANCE_TRADES_FIELD_COUNT,
            "trades row {index} has {} fields, expected {}",
            fields.len(),
            BINANCE_TRADES_FIELD_COUNT
        );

        let trade_id = fields[0].trim();
        let price_raw = fields[1].trim();
        let size_raw = fields[2].trim();
        // fields[3] is quote_qty (price*size); recomputed downstream, not stored.
        let time_micros: i64 = fields[4]
            .trim()
            .parse()
            .with_context(|| format!("trades row {index}: invalid time {:?}", fields[4]))?;
        let aggressor = TradeAggressorSide::from_is_buyer_maker(fields[5]).with_context(|| {
            format!("trades row {index}: invalid is_buyer_maker {:?}", fields[5])
        })?;
        // fields[6] is is_best_match; not part of the canonical model.

        ensure!(!trade_id.is_empty(), "trades row {index}: empty trade id");
        let price: Decimal = price_raw
            .parse()
            .with_context(|| format!("trades row {index}: invalid price {price_raw:?}"))?;
        let size: Decimal = size_raw
            .parse()
            .with_context(|| format!("trades row {index}: invalid size {size_raw:?}"))?;
        ensure!(
            price > Decimal::ZERO,
            "trades row {index}: non-positive price"
        );
        ensure!(
            size > Decimal::ZERO,
            "trades row {index}: non-positive size"
        );

        let event_time = time_micros
            .checked_mul(NANOS_PER_MICROSECOND)
            .with_context(|| format!("trades row {index}: timestamp overflow"))?;

        rows.push(BinanceTradeRow {
            venue: provenance.venue.clone(),
            product_family: provenance.product_family.clone(),
            product_category: provenance.product_category.clone(),
            instrument_id: identity.instrument_id.clone(),
            venue_symbol: identity.venue_symbol.clone(),
            nt_instrument_id: identity.nt_instrument_id.clone(),
            event_time,
            source_proof_id: provenance.source_proof_id.clone(),
            payload_hash: provenance.payload_hash.clone(),
            trade_id: trade_id.to_string(),
            aggressor_side: aggressor,
            price: price_raw.to_string(),
            size: size_raw.to_string(),
        });
    }

    let table = BinanceTradesTable {
        provenance: provenance.clone(),
        identity: identity.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl BinanceTradesTable {
    /// Validate non-emptiness, positive monotonic event timestamps, and
    /// per-row instrument identity.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        self.provenance.validate()?;
        self.identity.validate()?;
        ensure!(!self.rows.is_empty(), "binance trades table is empty");
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.event_time > 0,
                "trades row {index}: non-positive event_time"
            );
            ensure!(
                row.event_time >= previous,
                "trades row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous
            );
            previous = row.event_time;
            ensure!(
                row.instrument_id == self.identity.instrument_id,
                "trades row {index}: instrument_id mismatch"
            );
            for field in [&row.trade_id, &row.price, &row.size, &row.nt_instrument_id] {
                ensure!(!field.trim().is_empty(), "trades row {index}: empty field");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Klines normalization
// ---------------------------------------------------------------------------

/// Normalize a decompressed Binance public-archive `klines` CSV into the
/// canonical klines table.
///
/// `csv_text` is the decompressed text of the accepted `.zip` object (the
/// unzip is the ingest step). The file is headerless.
///
/// # Errors
///
/// Returns an error if a row is malformed, a field fails to parse, the OHLC
/// invariant is violated, or open timestamps are not strictly increasing.
pub fn normalize_binance_klines(
    provenance: &BinanceProvenance,
    identity: &BinanceInstrumentIdentity,
    bar_spec: KlineBarSpec,
    csv_text: &str,
) -> Result<BinanceKlinesTable> {
    provenance.validate()?;
    identity.validate()?;
    ensure!(bar_spec.step > 0, "kline bar step must be positive");

    let mut rows = Vec::new();
    for (index, line) in csv_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == BINANCE_KLINES_FIELD_COUNT,
            "klines row {index} has {} fields, expected {}",
            fields.len(),
            BINANCE_KLINES_FIELD_COUNT
        );

        let open_micros: i64 = fields[0]
            .trim()
            .parse()
            .with_context(|| format!("klines row {index}: invalid open_time {:?}", fields[0]))?;
        let open_raw = fields[1].trim();
        let high_raw = fields[2].trim();
        let low_raw = fields[3].trim();
        let close_raw = fields[4].trim();
        let volume_raw = fields[5].trim();
        let close_micros: i64 = fields[6]
            .trim()
            .parse()
            .with_context(|| format!("klines row {index}: invalid close_time {:?}", fields[6]))?;

        let open: Decimal = open_raw
            .parse()
            .with_context(|| format!("klines row {index}: invalid open {open_raw:?}"))?;
        let high: Decimal = high_raw
            .parse()
            .with_context(|| format!("klines row {index}: invalid high {high_raw:?}"))?;
        let low: Decimal = low_raw
            .parse()
            .with_context(|| format!("klines row {index}: invalid low {low_raw:?}"))?;
        let close: Decimal = close_raw
            .parse()
            .with_context(|| format!("klines row {index}: invalid close {close_raw:?}"))?;
        let volume: Decimal = volume_raw
            .parse()
            .with_context(|| format!("klines row {index}: invalid volume {volume_raw:?}"))?;

        ensure!(
            open > Decimal::ZERO,
            "klines row {index}: non-positive open"
        );
        ensure!(low > Decimal::ZERO, "klines row {index}: non-positive low");
        ensure!(
            volume >= Decimal::ZERO,
            "klines row {index}: negative volume"
        );
        // NautilusTrader's `Bar::new_checked` enforces these; fail loud earlier
        // with a precise message rather than a downstream panic.
        ensure!(
            high >= open && high >= low && high >= close,
            "klines row {index}: high {high} is not the maximum (o={open} l={low} c={close})"
        );
        ensure!(
            low <= open && low <= close,
            "klines row {index}: low {low} is not the minimum (o={open} c={close})"
        );

        let open_time = open_micros
            .checked_mul(NANOS_PER_MICROSECOND)
            .with_context(|| format!("klines row {index}: open_time overflow"))?;
        let close_time = close_micros
            .checked_mul(NANOS_PER_MICROSECOND)
            .with_context(|| format!("klines row {index}: close_time overflow"))?;
        ensure!(
            close_time >= open_time,
            "klines row {index}: close_time precedes open_time"
        );

        rows.push(BinanceBarRow {
            venue: provenance.venue.clone(),
            product_family: provenance.product_family.clone(),
            product_category: provenance.product_category.clone(),
            instrument_id: identity.instrument_id.clone(),
            venue_symbol: identity.venue_symbol.clone(),
            nt_instrument_id: identity.nt_instrument_id.clone(),
            open_time,
            close_time,
            source_proof_id: provenance.source_proof_id.clone(),
            payload_hash: provenance.payload_hash.clone(),
            open: open_raw.to_string(),
            high: high_raw.to_string(),
            low: low_raw.to_string(),
            close: close_raw.to_string(),
            volume: volume_raw.to_string(),
        });
    }

    let table = BinanceKlinesTable {
        provenance: provenance.clone(),
        identity: identity.clone(),
        bar_spec,
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl BinanceKlinesTable {
    /// Validate non-emptiness, positive strictly-increasing open timestamps,
    /// and per-row instrument identity.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        self.provenance.validate()?;
        self.identity.validate()?;
        ensure!(self.bar_spec.step > 0, "kline bar step must be positive");
        ensure!(!self.rows.is_empty(), "binance klines table is empty");
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.open_time > 0,
                "klines row {index}: non-positive open_time"
            );
            ensure!(
                row.open_time > previous,
                "klines row {index}: open_time {} not after previous {}",
                row.open_time,
                previous
            );
            previous = row.open_time;
            ensure!(
                row.instrument_id == self.identity.instrument_id,
                "klines row {index}: instrument_id mismatch"
            );
            for field in [
                &row.open,
                &row.high,
                &row.low,
                &row.close,
                &row.volume,
                &row.nt_instrument_id,
            ] {
                ensure!(!field.trim().is_empty(), "klines row {index}: empty field");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Instrument construction (shared by both projections)
// ---------------------------------------------------------------------------

/// Decimal places implied by a decimal-string increment (`0.1` -> 1,
/// `0.00000001` -> 8, `1` -> 0). Trailing zeros are significant.
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
/// Returns an error if any field fails to parse.
pub fn build_currency_pair(spec: &BinanceInstrumentSpec) -> Result<CurrencyPair> {
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
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

fn price_at(value: &str, precision: u8) -> Result<Price> {
    let scaled = rescaled(value, precision)?;
    Price::from_str(&scaled).map_err(|error| anyhow::anyhow!("invalid price {scaled:?}: {error}"))
}

fn quantity_at(value: &str, precision: u8) -> Result<Quantity> {
    let scaled = rescaled(value, precision)?;
    Quantity::from_str(&scaled)
        .map_err(|error| anyhow::anyhow!("invalid quantity {scaled:?}: {error}"))
}

// ---------------------------------------------------------------------------
// NautilusTrader type projections
// ---------------------------------------------------------------------------

/// Convert canonical trade rows into NautilusTrader `TradeTick`s at the
/// instrument's price/size precision.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision, or a trade id exceeds the NautilusTrader limit.
pub fn canonical_rows_to_trade_ticks(
    table: &BinanceTradesTable,
    instrument: &CurrencyPair,
) -> Result<Vec<TradeTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| {
            let price = price_at(&row.price, price_precision)?;
            let size = quantity_at(&row.size, size_precision)?;
            let trade_id = TradeId::new_checked(&row.trade_id)
                .map_err(|error| anyhow::anyhow!("invalid trade id {:?}: {error}", row.trade_id))?;
            let ts = UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
            Ok(TradeTick::new(
                instrument_id,
                price,
                size,
                row.aggressor_side.to_nt(),
                trade_id,
                ts,
                ts,
            ))
        })
        .collect()
}

/// Convert canonical bar rows into NautilusTrader `Bar`s at the instrument's
/// price/size precision under the kline bar type.
///
/// # Errors
///
/// Returns an error if an OHLCV value cannot be represented at the instrument
/// precision.
pub fn canonical_rows_to_bars(
    table: &BinanceKlinesTable,
    instrument: &CurrencyPair,
) -> Result<Vec<Bar>> {
    let instrument_id = instrument.id();
    let bar_type = table.bar_spec.to_bar_type(instrument_id)?;
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    table
        .rows
        .iter()
        .map(|row| {
            let open = price_at(&row.open, price_precision)?;
            let high = price_at(&row.high, price_precision)?;
            let low = price_at(&row.low, price_precision)?;
            let close = price_at(&row.close, price_precision)?;
            let volume = quantity_at(&row.volume, size_precision)?;
            let ts_event =
                UnixNanos::from(u64::try_from(row.close_time).context("negative close_time")?);
            // NautilusTrader's `Bar::new_checked` enforces the OHLC invariant
            // already validated upstream; use the checked constructor so any
            // residual precision-rescale edge fails loud rather than panicking.
            Bar::new_checked(bar_type, open, high, low, close, volume, ts_event, ts_event)
                .context("build bar")
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Catalog projection + read-back
// ---------------------------------------------------------------------------

/// Result of projecting canonical Binance data into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinanceCatalogProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub record_count: usize,
}

fn assert_clean_catalog_root(catalog_root: &Path) -> Result<()> {
    // Fail closed on a dirty catalog root. NautilusTrader's `write_to_parquet`
    // skips writing when a file for the same identifier/interval already exists,
    // so projecting into a non-empty root could silently read back stale data.
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
    Ok(())
}

/// Project a canonical trades table into a NautilusTrader `ParquetDataCatalog`
/// as `TradeTick` data plus the venue instrument.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_trades_to_catalog(
    table: &BinanceTradesTable,
    spec: &BinanceInstrumentSpec,
    catalog_root: &Path,
) -> Result<BinanceCatalogProjection> {
    table.validate()?;
    let instrument = build_currency_pair(spec)?;
    let instrument_id = instrument.id();
    ensure!(
        instrument_id.to_string() == table.identity.nt_instrument_id,
        "instrument id {instrument_id} does not match canonical identity {}",
        table.identity.nt_instrument_id
    );
    let ticks = canonical_rows_to_trade_ticks(table, &instrument)?;
    let record_count = ticks.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CurrencyPair(instrument)])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(BinanceCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        record_count,
    })
}

/// Project a canonical klines table into a NautilusTrader `ParquetDataCatalog`
/// as `Bar` data plus the venue instrument.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_klines_to_catalog(
    table: &BinanceKlinesTable,
    spec: &BinanceInstrumentSpec,
    catalog_root: &Path,
) -> Result<BinanceCatalogProjection> {
    table.validate()?;
    let instrument = build_currency_pair(spec)?;
    let instrument_id = instrument.id();
    ensure!(
        instrument_id.to_string() == table.identity.nt_instrument_id,
        "instrument id {instrument_id} does not match canonical identity {}",
        table.identity.nt_instrument_id
    );
    let bars = canonical_rows_to_bars(table, &instrument)?;
    let record_count = bars.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CurrencyPair(instrument)])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write bars to catalog")?;

    Ok(BinanceCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_BAR.to_string(),
        record_count,
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

/// Prove the resolved NautilusTrader dependency can read the projected `Bar`
/// data back from `catalog_root`.
///
/// Bars are keyed in the catalog by `bar_type` (a superstring of the instrument
/// id); NautilusTrader accepts the instrument id as a prefix match.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_bars(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<Bar>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<Bar>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query bars from catalog")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> BinanceProvenance {
        BinanceProvenance {
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "binance-spot-trades".to_string(),
            venue: "binance".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            source_proof_id: "source-proof-binance-spot".to_string(),
            payload_hash: "deadbeef".to_string(),
            archive_date: "2026-04".to_string(),
        }
    }

    fn identity() -> BinanceInstrumentIdentity {
        BinanceInstrumentIdentity {
            instrument_id: "XRPTUSD".to_string(),
            venue_symbol: "XRPTUSD".to_string(),
            nt_instrument_id: "XRPTUSD.BINANCE".to_string(),
        }
    }

    const SAMPLE_TRADES: &str = "1,1.36140000,64.70000000,88.08258000,1775024467633810,False,True\n\
        2,1.36150000,6.20000000,8.44130000,1775025255234180,False,True\n\
        3,1.36150000,65.00000000,88.49750000,1775053304229294,True,True\n";

    const SAMPLE_KLINES: &str = "1775053140000000,1.36150000,1.36150000,1.36150000,1.36150000,0.00000000,1775053199999999,0.00000000,0,0.00000000,0.00000000,0\n\
        1775055960000000,1.36240000,1.36680000,1.36240000,1.36680000,504.30000000,1775056019999999,688.97239000,3,504.30000000,688.97239000,0\n";

    #[test]
    fn normalizes_trades_with_aggressor_mapping() {
        let table = normalize_binance_trades(&provenance(), &identity(), SAMPLE_TRADES)
            .expect("normalize trades");
        assert_eq!(table.rows.len(), 3);
        // is_buyer_maker=False -> buyer-initiated.
        assert_eq!(table.rows[0].aggressor_side, TradeAggressorSide::Buyer);
        // is_buyer_maker=True -> seller-initiated.
        assert_eq!(table.rows[2].aggressor_side, TradeAggressorSide::Seller);
        // microseconds -> nanoseconds.
        assert_eq!(
            table.rows[0].event_time,
            1_775_024_467_633_810 * NANOS_PER_MICROSECOND
        );
        assert_eq!(table.rows[0].price, "1.36140000");
    }

    #[test]
    fn rejects_wrong_trades_field_count() {
        let bad = "1,1.0,1.0,1.0,1775024467633810,False\n";
        let err = normalize_binance_trades(&provenance(), &identity(), bad).unwrap_err();
        assert!(err.to_string().contains("fields"), "{err}");
    }

    #[test]
    fn rejects_unknown_is_buyer_maker() {
        let bad = "1,1.0,1.0,1.0,1775024467633810,Maybe,True\n";
        let err = normalize_binance_trades(&provenance(), &identity(), bad).unwrap_err();
        assert!(err.to_string().contains("is_buyer_maker"), "{err}");
    }

    #[test]
    fn rejects_non_monotonic_trade_time() {
        let bad = "1,1.0,1.0,1.0,1775024467633810,False,True\n\
            2,1.0,1.0,1.0,1775024467633800,False,True\n";
        let err = normalize_binance_trades(&provenance(), &identity(), bad).unwrap_err();
        assert!(err.to_string().contains("precedes previous"), "{err}");
    }

    #[test]
    fn normalizes_klines_with_microsecond_timestamps() {
        let spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let table = normalize_binance_klines(&provenance(), &identity(), spec, SAMPLE_KLINES)
            .expect("normalize klines");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(
            table.rows[0].open_time,
            1_775_053_140_000_000 * NANOS_PER_MICROSECOND
        );
        assert_eq!(table.rows[1].open, "1.36240000");
        assert_eq!(table.rows[1].high, "1.36680000");
    }

    #[test]
    fn rejects_wrong_klines_field_count() {
        let bad = "1775053140000000,1.0,1.0,1.0,1.0,0.0,1775053199999999\n";
        let spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let err = normalize_binance_klines(&provenance(), &identity(), spec, bad).unwrap_err();
        assert!(err.to_string().contains("fields"), "{err}");
    }

    #[test]
    fn rejects_klines_ohlc_violation() {
        // high < low: invalid bar.
        let bad = "1775053140000000,1.0,0.9,1.1,1.0,0.0,1775053199999999,0.0,0,0.0,0.0,0\n";
        let spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let err = normalize_binance_klines(&provenance(), &identity(), spec, bad).unwrap_err();
        assert!(err.to_string().contains("high"), "{err}");
    }

    #[test]
    fn rejects_non_increasing_kline_open_time() {
        let bad = "1775053200000000,1.0,1.0,1.0,1.0,0.0,1775053259999999,0.0,0,0.0,0.0,0\n\
            1775053140000000,1.0,1.0,1.0,1.0,0.0,1775053199999999,0.0,0,0.0,0.0,0\n";
        let spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let err = normalize_binance_klines(&provenance(), &identity(), spec, bad).unwrap_err();
        assert!(err.to_string().contains("not after previous"), "{err}");
    }

    #[test]
    fn decimal_places_reads_increment_precision() {
        assert_eq!(decimal_places("0.00000001"), 8);
        assert_eq!(decimal_places("0.1"), 1);
        assert_eq!(decimal_places("1"), 0);
    }
}
