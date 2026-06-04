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
//! ## Spot archive (`product=spot`, families `trades` / `klines`)
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
//!
//! ## Futures archive (`product=futures_um`, families `aggTrades`,
//! `markPriceKlines`, `indexPriceKlines`, `premiumIndexKlines`)
//!
//! Verified against the smallest real object of each family. These differ from
//! the spot archive in three physical facts encoded below:
//!
//! - Every futures CSV carries a **header row** (the spot CSVs do not). The
//!   new normalizers consume and verify the header, failing loud on drift.
//! - All futures timestamps are **milliseconds** since the Unix epoch (the spot
//!   archive uses microseconds).
//! - `aggTrades` columns: `agg_trade_id, price, quantity, first_trade_id,
//!   last_trade_id, transact_time, is_buyer_maker`. `is_buyer_maker` is the
//!   lowercase `true`/`false` token and follows the same aggressor convention
//!   as spot (`true` -> seller-initiated, `false` -> buyer-initiated). The
//!   canonical trade id is the `agg_trade_id` (the stable per-object id Binance
//!   assigns to each aggregated print).
//! - `markPriceKlines` shares the 12-column kline layout above, but its OHLC
//!   values are the exchange **mark price** (a tradable reference NautilusTrader
//!   models first-class), with `volume` always `0`. It projects into NT `Bar`s
//!   via the shared positive price-feed kline path.
//! - `indexPriceKlines`, `premiumIndexKlines`, funding rates, open interest, and
//!   exchange metadata are NOT tradable market data (the premium index is a
//!   signed basis rate, not a price), so they are deliberately NOT converted to
//!   NT catalog types; they remain staged Parquet for direct (pandas/polars)
//!   research. The positive-only kline path here therefore rejects them.

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
use sha2::{Digest, Sha256};

/// NautilusTrader data type written for the trades family.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the klines family.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// NautilusTrader venue code for Binance, appended to a venue-native symbol to
/// form the catalog instrument id (`<symbol>.BINANCE`). The data-derived bulk
/// path needs this because the Binance public-archive CSVs carry no instrument
/// column — the symbol lives in the S3 object key's `symbol=` segment, not the
/// rows. This is a per-venue format constant (the venue suffix), not a runtime
/// instrument value.
pub const BINANCE_VENUE: &str = "BINANCE";

/// Number of comma-separated fields in a Binance public-archive `trades` row.
pub const BINANCE_TRADES_FIELD_COUNT: usize = 7;

/// Number of comma-separated fields in a Binance public-archive `klines` row.
pub const BINANCE_KLINES_FIELD_COUNT: usize = 12;

/// Number of comma-separated fields in a Binance futures `aggTrades` row.
pub const BINANCE_AGG_TRADES_FIELD_COUNT: usize = 7;

/// Header row of a Binance futures `aggTrades` CSV (verified against the
/// archive). Used to fail loud if the source layout drifts.
pub const BINANCE_AGG_TRADES_HEADER: &str =
    "agg_trade_id,price,quantity,first_trade_id,last_trade_id,transact_time,is_buyer_maker";

/// Header row shared by the Binance futures price-feed kline families
/// (`markPriceKlines`, `indexPriceKlines`, `premiumIndexKlines`); identical to
/// the documented spot kline column layout. Used to fail loud on layout drift.
pub const BINANCE_KLINES_HEADER: &str = "open_time,open,high,low,close,volume,close_time,quote_volume,count,taker_buy_volume,taker_buy_quote_volume,ignore";

/// Spot-archive Binance timestamps are microseconds since the Unix epoch.
const NANOS_PER_MICROSECOND: i64 = 1_000;

/// Futures-archive Binance timestamps are milliseconds since the Unix epoch.
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

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
// aggTrades normalization (Binance futures `aggTrades` family)
// ---------------------------------------------------------------------------

/// Strip and verify the single header line from a decompressed futures CSV.
///
/// Returns the remaining body. Fails loud if the first non-empty line is not
/// the expected header (guards against silent layout drift in the archive).
fn strip_verified_header<'a>(csv_text: &'a str, expected_header: &str) -> Result<&'a str> {
    let mut lines = csv_text.lines();
    let header = loop {
        match lines.next() {
            Some(line) if line.trim().is_empty() => continue,
            Some(line) => break line,
            None => bail!("empty CSV: no header row"),
        }
    };
    ensure!(
        header.trim() == expected_header,
        "unexpected CSV header {:?}, expected {:?}",
        header.trim(),
        expected_header
    );
    // The remainder is everything after the header line. `lines()` does not
    // expose a remainder, so reconstruct it from the byte offset of the header.
    let header_end = header.as_ptr() as usize - csv_text.as_ptr() as usize + header.len();
    Ok(&csv_text[header_end..])
}

/// Normalize a decompressed Binance futures `aggTrades` CSV into the canonical
/// trades table (reusing the shared [`BinanceTradeRow`] model so the
/// `TradeTick` projection and read-back paths are shared with the spot family).
///
/// `csv_text` is the decompressed text of the accepted `.zip` object (the unzip
/// is the ingest step). Unlike the spot `trades` CSV, this file carries a
/// header row and uses millisecond `transact_time`.
///
/// The canonical trade id is the `agg_trade_id`. Aggressor side follows the
/// same `is_buyer_maker` convention as spot: `true` -> seller-initiated,
/// `false` -> buyer-initiated.
///
/// # Errors
///
/// Returns an error if the header is unexpected, a row is malformed, a field
/// fails to parse, a price/size is non-positive, or event timestamps are not
/// monotonically non-decreasing.
pub fn normalize_binance_agg_trades(
    provenance: &BinanceProvenance,
    identity: &BinanceInstrumentIdentity,
    csv_text: &str,
) -> Result<BinanceTradesTable> {
    provenance.validate()?;
    identity.validate()?;

    let body = strip_verified_header(csv_text, BINANCE_AGG_TRADES_HEADER)?;

    let mut rows = Vec::new();
    for (index, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == BINANCE_AGG_TRADES_FIELD_COUNT,
            "aggTrades row {index} has {} fields, expected {}",
            fields.len(),
            BINANCE_AGG_TRADES_FIELD_COUNT
        );

        let trade_id = fields[0].trim();
        let price_raw = fields[1].trim();
        let size_raw = fields[2].trim();
        // fields[3] (first_trade_id) and fields[4] (last_trade_id) describe the
        // aggregation window; the canonical id is the agg_trade_id.
        let time_millis: i64 = fields[5].trim().parse().with_context(|| {
            format!(
                "aggTrades row {index}: invalid transact_time {:?}",
                fields[5]
            )
        })?;
        let aggressor = TradeAggressorSide::from_is_buyer_maker(fields[6]).with_context(|| {
            format!(
                "aggTrades row {index}: invalid is_buyer_maker {:?}",
                fields[6]
            )
        })?;

        ensure!(
            !trade_id.is_empty(),
            "aggTrades row {index}: empty trade id"
        );
        let price: Decimal = price_raw
            .parse()
            .with_context(|| format!("aggTrades row {index}: invalid price {price_raw:?}"))?;
        let size: Decimal = size_raw
            .parse()
            .with_context(|| format!("aggTrades row {index}: invalid size {size_raw:?}"))?;
        ensure!(
            price > Decimal::ZERO,
            "aggTrades row {index}: non-positive price"
        );
        ensure!(
            size > Decimal::ZERO,
            "aggTrades row {index}: non-positive size"
        );

        let event_time = time_millis
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("aggTrades row {index}: timestamp overflow"))?;

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
    // Spot klines: headerless, microsecond timestamps, strictly positive prices.
    parse_klines(
        provenance,
        identity,
        bar_spec,
        csv_text,
        "klines",
        NANOS_PER_MICROSECOND,
    )
}

/// Normalize a decompressed Binance futures **mark-price** kline CSV
/// (`markPriceKlines`) into the canonical klines table, reusing the shared
/// [`BinanceBarRow`] model so the `Bar` projection and read-back paths are
/// shared with the spot family.
///
/// The mark price is a tradable reference NautilusTrader models first-class, so
/// it converts to NT `Bar`s. The sibling `indexPriceKlines`/`premiumIndexKlines`
/// families are NOT tradable market data (the premium index is a signed basis
/// rate, not a price); they are kept as staged Parquet, not converted, so this
/// path enforces strict positivity and rejects them.
///
/// These futures CSVs differ from the spot `klines` archive in two physical
/// facts: a header row is present and timestamps are milliseconds.
///
/// `csv_text` is the decompressed text of the accepted `.zip` object (the unzip
/// is the ingest step).
///
/// # Errors
///
/// Returns an error if the header is unexpected, a row is malformed, a field
/// fails to parse, an OHLC value is non-positive, the OHLC ordering invariant is
/// violated, or open timestamps are not strictly increasing.
pub fn normalize_binance_price_feed_klines(
    provenance: &BinanceProvenance,
    identity: &BinanceInstrumentIdentity,
    bar_spec: KlineBarSpec,
    csv_text: &str,
) -> Result<BinanceKlinesTable> {
    let body = strip_verified_header(csv_text, BINANCE_KLINES_HEADER)?;
    parse_klines(
        provenance,
        identity,
        bar_spec,
        body,
        "price-feed klines",
        NANOS_PER_MILLISECOND,
    )
}

/// Shared kline body parser. `body` is the CSV with any header already removed;
/// `nanos_per_unit` is the source timestamp unit. OHLC values must be strictly
/// positive: only tradable price feeds (traded/mark klines) are converted to NT.
fn parse_klines(
    provenance: &BinanceProvenance,
    identity: &BinanceInstrumentIdentity,
    bar_spec: KlineBarSpec,
    body: &str,
    label: &str,
    nanos_per_unit: i64,
) -> Result<BinanceKlinesTable> {
    provenance.validate()?;
    identity.validate()?;
    ensure!(bar_spec.step > 0, "kline bar step must be positive");

    let mut rows = Vec::new();
    for (index, line) in body.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == BINANCE_KLINES_FIELD_COUNT,
            "{label} row {index} has {} fields, expected {}",
            fields.len(),
            BINANCE_KLINES_FIELD_COUNT
        );

        let open_units: i64 = fields[0]
            .trim()
            .parse()
            .with_context(|| format!("{label} row {index}: invalid open_time {:?}", fields[0]))?;
        let open_raw = fields[1].trim();
        let high_raw = fields[2].trim();
        let low_raw = fields[3].trim();
        let close_raw = fields[4].trim();
        let volume_raw = fields[5].trim();
        let close_units: i64 = fields[6]
            .trim()
            .parse()
            .with_context(|| format!("{label} row {index}: invalid close_time {:?}", fields[6]))?;

        let open: Decimal = open_raw
            .parse()
            .with_context(|| format!("{label} row {index}: invalid open {open_raw:?}"))?;
        let high: Decimal = high_raw
            .parse()
            .with_context(|| format!("{label} row {index}: invalid high {high_raw:?}"))?;
        let low: Decimal = low_raw
            .parse()
            .with_context(|| format!("{label} row {index}: invalid low {low_raw:?}"))?;
        let close: Decimal = close_raw
            .parse()
            .with_context(|| format!("{label} row {index}: invalid close {close_raw:?}"))?;
        let volume: Decimal = volume_raw
            .parse()
            .with_context(|| format!("{label} row {index}: invalid volume {volume_raw:?}"))?;

        // Only tradable price feeds (traded/mark klines) are converted to NT;
        // strict positivity rejects index/premium basis feeds (kept as Parquet).
        ensure!(
            open > Decimal::ZERO,
            "{label} row {index}: non-positive open"
        );
        ensure!(low > Decimal::ZERO, "{label} row {index}: non-positive low");
        ensure!(
            volume >= Decimal::ZERO,
            "{label} row {index}: negative volume"
        );
        // OHLC ordering is required by NautilusTrader's `Bar::new_checked` for
        // every sign policy; fail loud here with a precise message rather than a
        // downstream panic.
        ensure!(
            high >= open && high >= low && high >= close,
            "{label} row {index}: high {high} is not the maximum (o={open} l={low} c={close})"
        );
        ensure!(
            low <= open && low <= close,
            "{label} row {index}: low {low} is not the minimum (o={open} c={close})"
        );

        let open_time = open_units
            .checked_mul(nanos_per_unit)
            .with_context(|| format!("{label} row {index}: open_time overflow"))?;
        let close_time = close_units
            .checked_mul(nanos_per_unit)
            .with_context(|| format!("{label} row {index}: close_time overflow"))?;
        ensure!(
            close_time >= open_time,
            "{label} row {index}: close_time precedes open_time"
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

// ===========================================================================
// Bulk-append path (data-derived precision + key-derived identity/provenance,
// no clean-root guard)
// ===========================================================================
//
// The hermetic `project_*_to_catalog` functions above stay as the single-object
// TEST harness: they refuse a dirty root and take a fully-specified instrument
// universe (base/quote currency, notional bounds) so they can write a venue
// `CurrencyPair`. The bulk-conversion path below is different: it flows many
// objects into one shared (possibly-S3) catalog, so it must NOT refuse a
// non-empty root — it relies on NautilusTrader's own per-instrument,
// per-time-range file naming and skip-on-existing.
//
// Two facts make the Binance bulk path diverge from the OKX template:
//
//  * The Binance public-archive CSVs carry **no instrument column** (verified
//    against the futures `aggTrades`/`markPriceKlines` layouts), so the
//    instrument identity cannot be read from the rows. It comes from the S3
//    object key's `symbol=` segment instead. A `object_key` parameter therefore
//    threads through this path.
//  * No instrument universe is assumed staged. Precision is derived from the
//    object's own rows (the maximum decimal places the exchange rendered for
//    each column), exactly like the OKX bulk path. Because Binance renders every
//    row of a column at the instrument's native tick/lot scale, the maximum
//    observed scale is stable across objects of the same instrument and is the
//    precision NautilusTrader pins per catalog file. Only the data is written
//    (TradeTick / Bar) — no `CurrencyPair` — because base/quote currency are not
//    derivable from the data and are not part of the TradeTick/Bar payload.

/// Decimal places of a single decimal-string value (`"643.3"` -> 1,
/// `"5995"` -> 0). The data-derived counterpart of [`decimal_places`] (which
/// reads an increment); here it reads an observed value.
fn value_decimal_places(value: &str) -> Result<u8> {
    let decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    u8::try_from(decimal.scale()).context("decimal scale exceeds u8")
}

/// Extract the value of a `key=value/` segment from an S3 object key.
///
/// The Binance staging layout (see `scripts/backfill_binance_to_s3.py`) encodes
/// `product=`, `family=`, `symbol=`, and `dt=` as path segments, so identity and
/// provenance are read from the key, never hardcoded.
///
/// # Errors
///
/// Returns an error if the segment is absent or empty.
fn key_segment(object_key: &str, name: &str) -> Result<String> {
    let needle = format!("{name}=");
    for part in object_key.split('/') {
        if let Some(value) = part.strip_prefix(&needle) {
            ensure!(
                !value.trim().is_empty(),
                "empty `{name}=` segment in object key {object_key:?}"
            );
            return Ok(value.to_string());
        }
    }
    bail!("object key {object_key:?} has no `{name}=` segment")
}

/// Build the honest provenance + venue-native identity for a bulk-converted
/// Binance object from the object's own bytes and its S3 key.
///
/// Every field describes THIS conversion truthfully:
///
///  * `venue` / `product_family` / `product_category` from the key's
///    `product=` segment (`futures_um`).
///  * `archive_date` from the key's `dt=` segment.
///  * `payload_hash` = lowercase SHA-256 hex over the exact object bytes handed
///    to the converter.
///  * `source_proof_id` = a deterministic label naming the family + object hash
///    of THIS object (it does not claim acceptance under the separate
///    source-proof machinery — see the FLAG in the crate handoff notes; the
///    bulk dispatch is responsible for the real source-proof binding).
///  * `ingest_run_id` = the fixed bulk-ingest run label passed by the caller.
///
/// The venue-native symbol comes from the key's `symbol=` segment because the
/// CSV rows carry no instrument column.
fn bulk_inputs(
    object_bytes: &[u8],
    object_key: &str,
    family: &str,
    ingest_run_id: &str,
) -> Result<(BinanceProvenance, BinanceInstrumentIdentity)> {
    ensure!(!object_bytes.is_empty(), "empty object for {object_key:?}");
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "empty ingest_run_id for {object_key:?}"
    );
    let symbol = key_segment(object_key, "symbol")?;
    let product = key_segment(object_key, "product")?;
    let archive_date = key_segment(object_key, "dt")?;
    let mut hasher = Sha256::new();
    hasher.update(object_bytes);
    let payload_hash = hex::encode(hasher.finalize());

    let provenance = BinanceProvenance {
        ingest_run_id: ingest_run_id.to_string(),
        source_binding: format!("binance-{product}-{family}"),
        venue: "binance".to_string(),
        product_family: product.clone(),
        product_category: product,
        source_proof_id: format!("binance-bulk/{family}/{payload_hash}"),
        payload_hash,
        archive_date,
    };
    let identity = BinanceInstrumentIdentity {
        instrument_id: symbol.clone(),
        venue_symbol: symbol.clone(),
        nt_instrument_id: format!("{symbol}.{BINANCE_VENUE}"),
    };
    Ok((provenance, identity))
}

/// One object's write summary produced by a bulk-append call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinanceAppendSummary {
    pub nt_instrument_id: String,
    pub data_type: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Convert canonical trade rows into NautilusTrader `TradeTick`s at the supplied
/// data-derived precision, keyed by the supplied instrument id.
///
/// This is the bulk-path twin of [`canonical_rows_to_trade_ticks`]: instead of
/// taking a fully-specified `CurrencyPair`, it takes the instrument id and the
/// precision derived from the rows, because the bulk path writes only the data
/// (no instrument) and assumes no staged instrument universe.
fn rows_to_trade_ticks_at(
    table: &BinanceTradesTable,
    instrument_id: InstrumentId,
    price_precision: u8,
    size_precision: u8,
) -> Result<Vec<TradeTick>> {
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

/// Convert canonical bar rows into NautilusTrader `Bar`s at the supplied
/// data-derived precision, under the table's bar type keyed by the supplied
/// instrument id.
///
/// The bulk-path twin of [`canonical_rows_to_bars`].
fn rows_to_bars_at(
    table: &BinanceKlinesTable,
    instrument_id: InstrumentId,
    price_precision: u8,
    size_precision: u8,
) -> Result<Vec<Bar>> {
    let bar_type = table.bar_spec.to_bar_type(instrument_id)?;
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
            Bar::new_checked(bar_type, open, high, low, close, volume, ts_event, ts_event)
                .context("build bar")
        })
        .collect()
}

/// Maximum price/size decimal places observed across a canonical trades table.
fn derive_trade_precisions(table: &BinanceTradesTable) -> Result<(u8, u8)> {
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in &table.rows {
        price_precision = price_precision.max(value_decimal_places(&row.price)?);
        size_precision = size_precision.max(value_decimal_places(&row.size)?);
    }
    Ok((price_precision, size_precision))
}

/// Maximum price/volume decimal places observed across a canonical klines table.
/// Price precision is the max across all four OHLC columns; size precision is
/// the max across the volume column.
fn derive_kline_precisions(table: &BinanceKlinesTable) -> Result<(u8, u8)> {
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in &table.rows {
        for value in [&row.open, &row.high, &row.low, &row.close] {
            price_precision = price_precision.max(value_decimal_places(value)?);
        }
        size_precision = size_precision.max(value_decimal_places(&row.volume)?);
    }
    Ok((price_precision, size_precision))
}

/// Append one Binance futures `aggTrades` object into an already-open
/// [`ParquetDataCatalog`] as `TradeTick` data — the bulk-conversion path.
///
/// `csv_text` is the decompressed text of the accepted `.zip` object (the unzip
/// is the ingest step, per the module contract). `object_key` is the S3 key the
/// object was staged under; the instrument symbol (`symbol=` segment) and
/// provenance (`product=`, `dt=`, object hash) are read from it because the CSV
/// rows carry no instrument column. Precision is derived from the rows.
///
/// Unlike [`project_trades_to_catalog`] (the hermetic single-object proof
/// harness, which refuses a dirty root and writes a `CurrencyPair`), this appends
/// only the `TradeTick` data into a shared catalog with no clean-root guard,
/// relying on NautilusTrader's own per-instrument file naming.
///
/// # Errors
///
/// Returns an error if the key lacks a required segment, the CSV fails to
/// normalize, precision cannot be derived, tick construction fails, or the
/// catalog write fails.
pub fn append_binance_futures_agg_trades_archive(
    csv_text: &str,
    object_key: &str,
    ingest_run_id: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<BinanceAppendSummary> {
    let (provenance, identity) =
        bulk_inputs(csv_text.as_bytes(), object_key, "aggTrades", ingest_run_id)?;
    let table = normalize_binance_agg_trades(&provenance, &identity, csv_text)?;
    let (price_precision, size_precision) = derive_trade_precisions(&table)?;
    let instrument_id = InstrumentId::from_str(&identity.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", identity.nt_instrument_id))?;
    let ticks = rows_to_trade_ticks_at(&table, instrument_id, price_precision, size_precision)?;
    let record_count = ticks.len();

    catalog
        .write_to_parquet(ticks, None, None, None)
        .with_context(|| {
            format!(
                "append Binance aggTrades ticks for {}",
                identity.instrument_id
            )
        })?;

    Ok(BinanceAppendSummary {
        nt_instrument_id: identity.nt_instrument_id,
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        record_count,
        price_precision,
        size_precision,
    })
}

/// Append one Binance futures `markPriceKlines` object into an already-open
/// [`ParquetDataCatalog`] as `Bar` data — the bulk-conversion path.
///
/// `csv_text` is the decompressed text of the accepted `.zip` object. `object_key`
/// supplies the instrument symbol and provenance (the CSV carries no instrument
/// column). `bar_spec` carries the `interval=` step/unit from the key (for
/// example `1m` -> step 1, [`BarAggregation::Minute`]); like the OKX candle path,
/// the bar step/unit is a partition fact the caller passes from the key, not data
/// the converter can invent. Precision is derived from the rows.
///
/// Only the mark-price feed is admitted: the shared positive-only kline parser
/// rejects the sibling index/premium basis feeds (kept as staged Parquet).
///
/// Unlike [`project_klines_to_catalog`] (the hermetic single-object proof
/// harness), this appends only the `Bar` data into a shared catalog with no
/// clean-root guard.
///
/// # Errors
///
/// Returns an error if the key lacks a required segment, the CSV fails to
/// normalize (including a non-positive OHLC value from a non-mark feed), precision
/// cannot be derived, bar construction fails, or the catalog write fails.
pub fn append_binance_futures_mark_price_klines_archive(
    csv_text: &str,
    object_key: &str,
    ingest_run_id: &str,
    bar_spec: KlineBarSpec,
    catalog: &mut ParquetDataCatalog,
) -> Result<BinanceAppendSummary> {
    let (provenance, identity) = bulk_inputs(
        csv_text.as_bytes(),
        object_key,
        "markPriceKlines",
        ingest_run_id,
    )?;
    let table = normalize_binance_price_feed_klines(&provenance, &identity, bar_spec, csv_text)?;
    let (price_precision, size_precision) = derive_kline_precisions(&table)?;
    let instrument_id = InstrumentId::from_str(&identity.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", identity.nt_instrument_id))?;
    let bars = rows_to_bars_at(&table, instrument_id, price_precision, size_precision)?;
    let record_count = bars.len();

    catalog
        .write_to_parquet(bars, None, None, None)
        .with_context(|| {
            format!(
                "append Binance markPriceKlines bars for {}",
                identity.instrument_id
            )
        })?;

    Ok(BinanceAppendSummary {
        nt_instrument_id: identity.nt_instrument_id,
        data_type: NT_DATA_TYPE_BAR.to_string(),
        record_count,
        price_precision,
        size_precision,
    })
}

/// The NautilusTrader bar-type string a `markPriceKlines` bulk-append writes
/// under, for read-back by the bulk dispatch and tests. Mirrors the identity the
/// append path derives from the object key.
///
/// # Errors
///
/// Returns an error if the instrument id is invalid or the bar step is zero.
pub fn binance_bar_type_string(nt_instrument_id: &str, bar_spec: KlineBarSpec) -> Result<String> {
    let instrument_id = InstrumentId::from_str(nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {nt_instrument_id:?}"))?;
    Ok(bar_spec.to_bar_type(instrument_id)?.to_string())
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

    // -----------------------------------------------------------------------
    // Futures families
    // -----------------------------------------------------------------------

    const SAMPLE_AGG_TRADES: &str = "agg_trade_id,price,quantity,first_trade_id,last_trade_id,transact_time,is_buyer_maker\n\
        1,2088.0,40.0,1,1,1774599098684,false\n\
        2,2169.87,0.02,2,2,1774599109473,false\n\
        3,2133.02,0.026,42561,42561,1774599200000,true\n";

    // markPrice/indexPrice layout (positive feed). Header + millisecond stamps.
    const SAMPLE_MARK_KLINES: &str = "open_time,open,high,low,close,volume,close_time,quote_volume,count,taker_buy_volume,taker_buy_quote_volume,ignore\n\
        1774591740000,2061.40441860,2062.16093023,2061.23418605,2062.12488372,0,1774591799999,0.00000000,59,0,0.00000000,0\n\
        1774591800000,2062.12488372,2062.59953488,2061.72302326,2062.59953488,0,1774591859999,0.00000000,60,0,0.00000000,0\n";

    // premiumIndex layout (signed feed). Includes zero, negative, and a
    // mixed-sign bar (negative low, positive high).
    const SAMPLE_PREMIUM_KLINES: &str = "open_time,open,high,low,close,volume,close_time,quote_volume,count,taker_buy_volume,taker_buy_quote_volume,ignore\n\
        1758122460000,0,0,0,0,0,1758122519999,0,7,0,0,0\n\
        1758535320000,-0.14553663,-0.11260799,-0.14553663,-0.11260799,0,1758535379999,0,12,0,0,0\n\
        1758535380000,-0.00040648,0.00012519,-0.00047397,-0.00031924,0,1758535439999,0,12,0,0,0\n";

    #[test]
    fn normalizes_agg_trades_with_header_and_millis() {
        let table = normalize_binance_agg_trades(&provenance(), &identity(), SAMPLE_AGG_TRADES)
            .expect("normalize aggTrades");
        // Header consumed; three data rows remain.
        assert_eq!(table.rows.len(), 3);
        // is_buyer_maker=false -> buyer-initiated.
        assert_eq!(table.rows[0].aggressor_side, TradeAggressorSide::Buyer);
        // is_buyer_maker=true -> seller-initiated.
        assert_eq!(table.rows[2].aggressor_side, TradeAggressorSide::Seller);
        // Canonical id is the agg_trade_id.
        assert_eq!(table.rows[1].trade_id, "2");
        // Milliseconds -> nanoseconds (NOT microseconds).
        assert_eq!(
            table.rows[0].event_time,
            1_774_599_098_684 * NANOS_PER_MILLISECOND
        );
        assert_eq!(table.rows[0].price, "2088.0");
    }

    #[test]
    fn rejects_agg_trades_with_unexpected_header() {
        let bad = "id,price,qty,a,b,t,m\n1,2088.0,40.0,1,1,1774599098684,false\n";
        let err = normalize_binance_agg_trades(&provenance(), &identity(), bad).unwrap_err();
        assert!(err.to_string().contains("unexpected CSV header"), "{err}");
    }

    #[test]
    fn rejects_agg_trades_missing_header() {
        let bad = "";
        let err = normalize_binance_agg_trades(&provenance(), &identity(), bad).unwrap_err();
        assert!(err.to_string().contains("no header row"), "{err}");
    }

    #[test]
    fn rejects_agg_trades_wrong_field_count() {
        let bad = "agg_trade_id,price,quantity,first_trade_id,last_trade_id,transact_time,is_buyer_maker\n\
            1,2088.0,40.0,1,1,1774599098684\n";
        let err = normalize_binance_agg_trades(&provenance(), &identity(), bad).unwrap_err();
        assert!(err.to_string().contains("fields"), "{err}");
    }

    #[test]
    fn normalizes_positive_price_feed_klines_with_header_and_millis() {
        let bar_spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let table = normalize_binance_price_feed_klines(
            &provenance(),
            &identity(),
            bar_spec,
            SAMPLE_MARK_KLINES,
        )
        .expect("normalize positive klines");
        assert_eq!(table.rows.len(), 2);
        // Milliseconds -> nanoseconds.
        assert_eq!(
            table.rows[0].open_time,
            1_774_591_740_000 * NANOS_PER_MILLISECOND
        );
        assert_eq!(table.rows[1].open, "2062.12488372");
    }

    #[test]
    fn rejects_negative_premium_klines() {
        // The premium-index basis feed carries zero/negative OHLC values; it is
        // NOT tradable market data, so the positive-only kline path rejects it
        // (it stays staged Parquet, never converted to an NT catalog type).
        let bar_spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let err = normalize_binance_price_feed_klines(
            &provenance(),
            &identity(),
            bar_spec,
            SAMPLE_PREMIUM_KLINES,
        )
        .unwrap_err();
        assert!(err.to_string().contains("non-positive"), "{err}");
    }

    #[test]
    fn rejects_price_feed_klines_with_unexpected_header() {
        let bad = "ot,o,h,l,c,v,ct,qv,n,tbv,tbq,ig\n\
            1774591740000,1,1,1,1,0,1774591799999,0,1,0,0,0\n";
        let bar_spec = KlineBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        };
        let err = normalize_binance_price_feed_klines(&provenance(), &identity(), bar_spec, bad)
            .unwrap_err();
        assert!(err.to_string().contains("unexpected CSV header"), "{err}");
    }
}
