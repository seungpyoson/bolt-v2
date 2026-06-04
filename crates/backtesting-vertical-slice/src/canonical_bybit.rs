//! Bybit derivatives + kline converter (spec 023 `1-backtesting-engine`).
//!
//! This module extends the venue coverage of the backtesting vertical slice to
//! the two Bybit staged data shapes that the spot-only [`super::canonical_trades`]
//! module does not handle:
//!
//! ```text
//! source=public_archive / family=tick_trades (category=linear|inverse)
//!   header: timestamp,symbol,side,size,price,tickDirection,trdMatchID,...
//!   -> NautilusTrader TradeTick
//!
//! source=rest / family=kline_1m (Bybit V5 REST envelope, list rows
//!   [start_ms, open, high, low, close, volume, turnover])
//!   -> NautilusTrader Bar (1-MINUTE-LAST-EXTERNAL)
//!
//! source=rest / family=mark_price_kline_1m (Bybit V5 REST envelope, list rows
//!   [start_ms, open, high, low, close] — mark-price OHLC, NO volume/turnover)
//!   -> NautilusTrader Bar (1-MINUTE-MARK-EXTERNAL)
//! ```
//!
//! It is deliberately self-contained: it owns its own canonical row/table types,
//! parsing, validation, and NautilusTrader catalog projection, so it can be built
//! and proved without touching the shared dispatch in
//! [`super::catalog_projection`] or the spot path in [`super::canonical_trades`].
//!
//! NautilusTrader owns the catalog: data is written via
//! [`ParquetDataCatalog::write_to_parquet`] and read back via
//! [`ParquetDataCatalog::query_typed_data`]. No arrow/parquet for NT types is
//! hand-rolled here.
//!
//! Provenance and instrument identity/precision are caller-supplied
//! (config/run-spec driven). No instrument id, symbol, currency, tick size, or
//! timestamp literal is hardcoded in this module.

use std::{
    io::Read,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Bar, BarSpecification, BarType, TradeTick},
    enums::{AggregationSource, AggressorSide, BarAggregation, PriceType},
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::source_proof::{AcceptedDataset, IngestManifestObjectRecord, SourceProofFidelityClass};

/// NautilusTrader data type written for the derivatives trade projection.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the kline projection.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// NautilusTrader venue code for Bybit, appended to a venue-native symbol to form
/// the catalog instrument id (`<venue_symbol>.BYBIT`). The data-derived bulk path
/// needs this because Bybit stages no per-object instrument universe to carry it.
pub const BYBIT_VENUE: &str = "BYBIT";

/// Fixed label identifying THIS bulk NT-catalog conversion as the provenance
/// origin of a data-derived append. It is not a claim about the upstream source;
/// it records which ingest run materialised the catalog rows.
const BYBIT_BULK_INGEST_RUN: &str = "bybit-nt-catalog-bulk-append";

/// Honest forbidden-claims note carried by a trade/bar-replay table: these
/// families reconstruct prints/candles, never the order book, so they cannot back
/// execution-quality or queue-position claims.
const BYBIT_REPLAY_FORBIDDEN_CLAIM: &str = "No execution-quality or queue-position claims.";

/// Expected Bybit derivatives (linear/inverse) tick-trades header, in order.
///
/// Only the leading columns the converter consumes are pinned; the source object
/// carries additional value columns (`grossValue`, `homeNotional`, ...) after
/// these that the converter ignores but whose count is still validated.
pub const BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX: [&str; 7] = [
    "timestamp",
    "symbol",
    "side",
    "size",
    "price",
    "tickDirection",
    "trdMatchID",
];

/// Native trade prints only; aggregated prints must never satisfy the trade table.
pub const TRADE_SOURCE_TYPE_NATIVE: &str = "native";

const NANOS_PER_SECOND: i64 = 1_000_000_000;
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// Aggressor side of a native trade print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BybitAggressorSide {
    Buyer,
    Seller,
}

impl BybitAggressorSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Seller => "SELLER",
        }
    }

    #[must_use]
    pub const fn to_nt(self) -> AggressorSide {
        match self {
            Self::Buyer => AggressorSide::Buyer,
            Self::Seller => AggressorSide::Seller,
        }
    }

    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "buy" => Ok(Self::Buyer),
            "sell" => Ok(Self::Seller),
            other => bail!("unknown trade side token: {other:?}"),
        }
    }
}

/// Venue-native instrument identity + precision for one Bybit instrument.
///
/// Built by the caller from accepted instrument-universe data plus the accepted
/// dataset, so no instrument identity or precision is hardcoded in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitInstrumentSpec {
    /// Venue-native instrument id, unique within the venue universe.
    pub instrument_id: String,
    /// Display/wire symbol from the source.
    pub venue_symbol: String,
    /// NautilusTrader instrument id, for example `DOGEUSDT-05JUN26.BYBIT`.
    pub nt_instrument_id: String,
    /// Price tick size as a decimal string, for example `0.00001`.
    pub price_increment: String,
    /// Size precision step as a decimal string, for example `1`.
    pub size_increment: String,
}

impl BybitInstrumentSpec {
    fn price_precision(&self) -> u8 {
        decimal_places(&self.price_increment)
    }

    fn size_precision(&self) -> u8 {
        decimal_places(&self.size_increment)
    }
}

/// Decimal places implied by a decimal-string increment (`0.1` -> 1,
/// `0.00001` -> 5, `1` -> 0). Trailing zeros are significant.
#[must_use]
fn decimal_places(increment: &str) -> u8 {
    match increment.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    }
}

/// Rescale a decimal string to exactly `precision` places, refusing to silently
/// drop precision the instrument cannot represent.
fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

fn nt_price(value: &str, precision: u8) -> Result<Price> {
    let rescaled = rescaled(value, precision)?;
    Price::from_str(&rescaled)
        .map_err(|error| anyhow::anyhow!("invalid rescaled price {rescaled:?}: {error}"))
}

fn nt_quantity(value: &str, precision: u8) -> Result<Quantity> {
    let rescaled = rescaled(value, precision)?;
    Quantity::from_str(&rescaled)
        .map_err(|error| anyhow::anyhow!("invalid rescaled size {rescaled:?}: {error}"))
}

// ----------------------------------------------------------------------------
// Derivatives tick trades -> TradeTick
// ----------------------------------------------------------------------------

/// One parsed Bybit derivatives native trade print.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitTradeRow {
    /// Exchange event time in Unix nanoseconds.
    pub event_time: i64,
    /// Native trade id (`trdMatchID`).
    pub trade_id: String,
    pub aggressor_side: BybitAggressorSide,
    /// Exact source price string.
    pub price: String,
    /// Exact source size string.
    pub size: String,
}

/// A validated canonical derivatives `trades` table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitTradesTable {
    pub source_proof_id: String,
    pub venue: String,
    pub product_family: String,
    pub instrument_id: String,
    pub nt_instrument_id: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub rows: Vec<BybitTradeRow>,
}

/// Parse the leading float-seconds timestamp of a Bybit derivatives trade into
/// Unix nanoseconds without floating-point rounding error.
fn deriv_seconds_to_nanos(raw: &str) -> Result<i64> {
    let seconds = Decimal::from_str(raw.trim())
        .with_context(|| format!("invalid derivatives timestamp {raw:?}"))?;
    let nanos = (seconds * Decimal::from(NANOS_PER_SECOND)).trunc();
    nanos
        .to_string()
        .parse::<i64>()
        .with_context(|| format!("derivatives timestamp {raw:?} overflows i64 nanos"))
}

/// Normalize an accepted Bybit derivatives tick-trades CSV into a validated
/// canonical trades table.
///
/// `csv_text` is the decompressed text of the accepted object. The header must
/// begin with [`BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX`]; trailing value columns
/// are accepted but ignored. The `symbol` column of every row must equal
/// `spec.venue_symbol`.
///
/// # Errors
///
/// Returns an error if the header prefix does not match, a row is malformed, a
/// field fails to parse, or the table fails contract validation.
pub fn normalize_bybit_deriv_tick_trades(
    accepted: &AcceptedDataset,
    spec: &BybitInstrumentSpec,
    csv_text: &str,
) -> Result<BybitTradesTable> {
    let mut lines = csv_text.lines();
    let header = lines.next().context("empty csv: missing header")?;
    let header_columns: Vec<&str> = header.split(',').map(str::trim).collect();
    ensure!(
        header_columns.len() >= BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX.len(),
        "csv header {header_columns:?} has fewer columns than required prefix {BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX:?}"
    );
    ensure!(
        header_columns[..BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX.len()]
            == BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX,
        "csv header prefix {:?} does not match expected {BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX:?}",
        &header_columns[..BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX.len()]
    );
    let column_count = header_columns.len();

    let mut rows = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == column_count,
            "row {index} has {} fields, expected {column_count}",
            fields.len()
        );

        let event_time = deriv_seconds_to_nanos(fields[0])?;
        let symbol = fields[1].trim();
        ensure!(
            symbol == spec.venue_symbol,
            "row {index}: symbol {symbol:?} does not match instrument {:?}",
            spec.venue_symbol
        );
        let aggressor_side = BybitAggressorSide::parse(fields[2])?;
        let size_raw = fields[3].trim();
        let price_raw = fields[4].trim();
        let trade_id = fields[6].trim();

        ensure!(!trade_id.is_empty(), "row {index}: empty trade id");
        let price: Decimal = price_raw
            .parse()
            .with_context(|| format!("row {index}: invalid price {price_raw:?}"))?;
        let size: Decimal = size_raw
            .parse()
            .with_context(|| format!("row {index}: invalid size {size_raw:?}"))?;
        ensure!(price > Decimal::ZERO, "row {index}: non-positive price");
        ensure!(size > Decimal::ZERO, "row {index}: non-positive size");

        rows.push(BybitTradeRow {
            event_time,
            trade_id: trade_id.to_string(),
            aggressor_side,
            price: price_raw.to_string(),
            size: size_raw.to_string(),
        });
    }

    let table = BybitTradesTable {
        source_proof_id: accepted.source_proof_id.clone(),
        venue: accepted.venue.clone(),
        product_family: accepted.product_family.clone(),
        instrument_id: spec.instrument_id.clone(),
        nt_instrument_id: spec.nt_instrument_id.clone(),
        fidelity_class: accepted.fidelity_class,
        forbidden_claims: accepted.forbidden_claims.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl BybitTradesTable {
    /// Validate non-emptiness, monotonic event times, and required fields.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.rows.is_empty(), "bybit trades table is empty");
        ensure!(
            self.fidelity_class != SourceProofFidelityClass::L2Replay,
            "trade prints must not be labelled L2_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "trade-replay table must carry explicit forbidden claims"
        );
        for field in [
            &self.source_proof_id,
            &self.venue,
            &self.product_family,
            &self.instrument_id,
            &self.nt_instrument_id,
        ] {
            ensure!(!field.trim().is_empty(), "empty provenance/identity field");
        }

        let mut previous_event_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            ensure!(
                row.event_time >= previous_event_time,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous_event_time
            );
            previous_event_time = row.event_time;
            for field in [&row.trade_id, &row.price, &row.size] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
        }
        Ok(())
    }

    /// Convert the canonical rows into NautilusTrader `TradeTick`s at the
    /// instrument's price/size precision.
    ///
    /// # Errors
    ///
    /// Returns an error if a price/size cannot be represented at the precision.
    pub fn to_trade_ticks(&self, spec: &BybitInstrumentSpec) -> Result<Vec<TradeTick>> {
        ensure!(
            spec.nt_instrument_id == self.nt_instrument_id,
            "spec instrument {:?} does not match table {:?}",
            spec.nt_instrument_id,
            self.nt_instrument_id
        );
        let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
            .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
        let price_precision = spec.price_precision();
        let size_precision = spec.size_precision();
        self.rows
            .iter()
            .map(|row| {
                let price = nt_price(&row.price, price_precision)?;
                let size = nt_quantity(&row.size, size_precision)?;
                let ts =
                    UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
                Ok(TradeTick::new(
                    instrument_id,
                    price,
                    size,
                    row.aggressor_side.to_nt(),
                    TradeId::from(row.trade_id.as_str()),
                    ts,
                    ts,
                ))
            })
            .collect()
    }
}

/// Result of projecting a Bybit table into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BybitCatalogProjection {
    pub catalog_root: PathBuf,
    pub nt_identifier: String,
    pub data_type: String,
    pub record_count: usize,
    pub fidelity_class: SourceProofFidelityClass,
}

fn ensure_clean_root(catalog_root: &Path) -> Result<()> {
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

/// Project a canonical derivatives trades table into a NautilusTrader
/// `ParquetDataCatalog` as `TradeTick` data.
///
/// # Errors
///
/// Returns an error if conversion or the catalog write fails.
pub fn project_bybit_trades_to_catalog(
    table: &BybitTradesTable,
    spec: &BybitInstrumentSpec,
    catalog_root: &Path,
) -> Result<BybitCatalogProjection> {
    table.validate()?;
    let ticks = table.to_trade_ticks(spec)?;
    let record_count = ticks.len();
    ensure_clean_root(catalog_root)?;

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(BybitCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: spec.nt_instrument_id.clone(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        record_count,
        fidelity_class: table.fidelity_class,
    })
}

/// Read the projected `TradeTick` data back from `catalog_root`.
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

// ----------------------------------------------------------------------------
// kline_1m -> Bar
// ----------------------------------------------------------------------------

/// The Bybit V5 REST kline envelope as staged under `family=kline_1m`.
#[derive(Debug, Clone, Deserialize)]
struct BybitKlineEnvelope {
    #[serde(rename = "retCode")]
    ret_code: i64,
    result: BybitKlineResult,
}

#[derive(Debug, Clone, Deserialize)]
struct BybitKlineResult {
    symbol: String,
    /// Each entry is `[startMs, open, high, low, close, volume, turnover]`.
    list: Vec<Vec<String>>,
}

/// Number of leading fields the converter consumes from a kline list row.
const KLINE_ROW_MIN_FIELDS: usize = 6;

/// One parsed 1-minute kline candle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitBarRow {
    /// Candle open (start) time in Unix nanoseconds.
    pub open_time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

/// A validated canonical 1-minute `bars` table for one accepted kline object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitBarsTable {
    pub source_proof_id: String,
    pub venue: String,
    pub product_family: String,
    pub instrument_id: String,
    pub nt_instrument_id: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub rows: Vec<BybitBarRow>,
}

/// Normalize an accepted Bybit `kline_1m` REST object into a validated canonical
/// bars table.
///
/// `json_text` is the decompressed text of the accepted object. The envelope's
/// `result.symbol` must equal `spec.venue_symbol`. Kline rows arrive newest-first
/// from the venue; they are sorted ascending by open time so the projection is
/// monotonic for NautilusTrader.
///
/// # Errors
///
/// Returns an error if the envelope is malformed, `retCode` is non-zero, the
/// symbol mismatches, a row is malformed, or the table fails validation.
pub fn normalize_bybit_kline_1m(
    accepted: &AcceptedDataset,
    spec: &BybitInstrumentSpec,
    json_text: &str,
) -> Result<BybitBarsTable> {
    let envelope: BybitKlineEnvelope =
        serde_json::from_str(json_text).context("parse bybit kline envelope")?;
    ensure!(
        envelope.ret_code == 0,
        "bybit kline retCode {} is not OK",
        envelope.ret_code
    );
    ensure!(
        envelope.result.symbol == spec.venue_symbol,
        "kline symbol {:?} does not match instrument {:?}",
        envelope.result.symbol,
        spec.venue_symbol
    );

    let mut rows = Vec::new();
    for (index, entry) in envelope.result.list.iter().enumerate() {
        ensure!(
            entry.len() >= KLINE_ROW_MIN_FIELDS,
            "kline row {index} has {} fields, expected at least {KLINE_ROW_MIN_FIELDS}",
            entry.len()
        );
        let start_ms: i64 = entry[0]
            .trim()
            .parse()
            .with_context(|| format!("kline row {index}: invalid start ms {:?}", entry[0]))?;
        let open_time = start_ms
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("kline row {index}: start ms overflow"))?;

        let open = entry[1].trim().to_string();
        let high = entry[2].trim().to_string();
        let low = entry[3].trim().to_string();
        let close = entry[4].trim().to_string();
        let volume = entry[5].trim().to_string();

        // Decimal-level OHLC integrity. NautilusTrader's `Bar::new_checked`
        // re-asserts this on the rounded prices; checking here fails loudly on
        // the source values before any rounding.
        let (o, h, l, c) = (
            Decimal::from_str(&open).with_context(|| format!("kline row {index}: open"))?,
            Decimal::from_str(&high).with_context(|| format!("kline row {index}: high"))?,
            Decimal::from_str(&low).with_context(|| format!("kline row {index}: low"))?,
            Decimal::from_str(&close).with_context(|| format!("kline row {index}: close"))?,
        );
        ensure!(o > Decimal::ZERO, "kline row {index}: non-positive open");
        ensure!(
            h >= o && h >= l && h >= c && l <= o && l <= c,
            "kline row {index}: OHLC integrity violated (o={o} h={h} l={l} c={c})"
        );
        let v = Decimal::from_str(&volume).with_context(|| format!("kline row {index}: volume"))?;
        ensure!(v >= Decimal::ZERO, "kline row {index}: negative volume");

        rows.push(BybitBarRow {
            open_time,
            open,
            high,
            low,
            close,
            volume,
        });
    }

    // Venue returns newest-first; NautilusTrader requires ascending ts.
    rows.sort_by_key(|row| row.open_time);

    let table = BybitBarsTable {
        source_proof_id: accepted.source_proof_id.clone(),
        venue: accepted.venue.clone(),
        product_family: accepted.product_family.clone(),
        instrument_id: spec.instrument_id.clone(),
        nt_instrument_id: spec.nt_instrument_id.clone(),
        fidelity_class: accepted.fidelity_class,
        forbidden_claims: accepted.forbidden_claims.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl BybitBarsTable {
    /// The fixed NautilusTrader bar specification for a Bybit 1-minute kline:
    /// 1-MINUTE-LAST, externally aggregated by the venue.
    fn bar_type(nt_instrument_id: &str) -> Result<BarType> {
        let instrument_id = InstrumentId::from_str(nt_instrument_id)
            .with_context(|| format!("invalid nt_instrument_id {nt_instrument_id:?}"))?;
        let step = NonZeroUsize::new(1).expect("1 is non-zero");
        let spec = BarSpecification {
            step,
            aggregation: BarAggregation::Minute,
            price_type: PriceType::Last,
        };
        Ok(BarType::new(
            instrument_id,
            spec,
            AggregationSource::External,
        ))
    }

    /// Validate non-emptiness, monotonic open times, and required fields.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.rows.is_empty(), "bybit bars table is empty");
        ensure!(
            self.fidelity_class != SourceProofFidelityClass::L2Replay,
            "kline bars must not be labelled L2_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "bar-replay table must carry explicit forbidden claims"
        );
        for field in [
            &self.source_proof_id,
            &self.venue,
            &self.product_family,
            &self.instrument_id,
            &self.nt_instrument_id,
        ] {
            ensure!(!field.trim().is_empty(), "empty provenance/identity field");
        }

        let mut previous_open_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.open_time > 0, "row {index}: non-positive open_time");
            ensure!(
                row.open_time >= previous_open_time,
                "row {index}: open_time {} precedes previous {}",
                row.open_time,
                previous_open_time
            );
            previous_open_time = row.open_time;
            for field in [&row.open, &row.high, &row.low, &row.close, &row.volume] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
        }
        Ok(())
    }

    /// The NautilusTrader bar-type string for catalog identifier resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if the instrument id is invalid.
    pub fn bar_type_string(&self) -> Result<String> {
        Ok(Self::bar_type(&self.nt_instrument_id)?.to_string())
    }

    /// Convert the canonical rows into NautilusTrader `Bar`s at the instrument's
    /// price/size precision.
    ///
    /// # Errors
    ///
    /// Returns an error if a price/volume cannot be represented at the precision
    /// or fails NautilusTrader's OHLC checks.
    pub fn to_bars(&self, spec: &BybitInstrumentSpec) -> Result<Vec<Bar>> {
        ensure!(
            spec.nt_instrument_id == self.nt_instrument_id,
            "spec instrument {:?} does not match table {:?}",
            spec.nt_instrument_id,
            self.nt_instrument_id
        );
        let bar_type = Self::bar_type(&spec.nt_instrument_id)?;
        let price_precision = spec.price_precision();
        let size_precision = spec.size_precision();
        self.rows
            .iter()
            .map(|row| {
                let open = nt_price(&row.open, price_precision)?;
                let high = nt_price(&row.high, price_precision)?;
                let low = nt_price(&row.low, price_precision)?;
                let close = nt_price(&row.close, price_precision)?;
                let volume = nt_quantity(&row.volume, size_precision)?;
                let ts =
                    UnixNanos::from(u64::try_from(row.open_time).context("negative open_time")?);
                Bar::new_checked(bar_type, open, high, low, close, volume, ts, ts)
                    .context("build NautilusTrader bar")
            })
            .collect()
    }
}

/// Project a canonical bars table into a NautilusTrader `ParquetDataCatalog` as
/// `Bar` data.
///
/// # Errors
///
/// Returns an error if conversion or the catalog write fails.
pub fn project_bybit_bars_to_catalog(
    table: &BybitBarsTable,
    spec: &BybitInstrumentSpec,
    catalog_root: &Path,
) -> Result<BybitCatalogProjection> {
    table.validate()?;
    let bars = table.to_bars(spec)?;
    let record_count = bars.len();
    ensure_clean_root(catalog_root)?;

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write bars to catalog")?;

    Ok(BybitCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: table.bar_type_string()?,
        data_type: NT_DATA_TYPE_BAR.to_string(),
        record_count,
        fidelity_class: table.fidelity_class,
    })
}

/// Read the projected `Bar` data back from `catalog_root` by its bar-type id.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_bars(catalog_root: &Path, bar_type: &str) -> Result<Vec<Bar>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<Bar>(
            Some(vec![bar_type.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query bars from catalog")
}

// ----------------------------------------------------------------------------
// mark_price_kline_1m -> Bar (PriceType::Mark)
// ----------------------------------------------------------------------------

/// NautilusTrader data type written for the mark-price kline projection.
///
/// Identical NautilusTrader type as the trade kline (`Bar`); the projections are
/// distinguished by the bar specification's `price_type` (`Mark` vs `Last`),
/// which yields a distinct `BarType` catalog identifier.
pub const NT_DATA_TYPE_MARK_BAR: &str = "Bar";

/// Number of leading fields the converter consumes from a mark-price kline row.
///
/// Mark / index / premium price klines carry only `[startMs, O, H, L, C]`; they
/// have NO `volume`/`turnover` columns (unlike the traded [`KLINE_ROW_MIN_FIELDS`]
/// candles). The projected [`Bar`] therefore carries a zero volume, which is the
/// faithful representation: a mark-price candle has no traded size.
const MARK_KLINE_ROW_MIN_FIELDS: usize = 5;

/// Volume placeholder for a mark-price candle, which has no traded size.
const MARK_BAR_VOLUME: &str = "0";

/// One parsed 1-minute mark-price candle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitMarkBarRow {
    /// Candle open (start) time in Unix nanoseconds.
    pub open_time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
}

/// A validated canonical 1-minute mark-price `bars` table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BybitMarkBarsTable {
    pub source_proof_id: String,
    pub venue: String,
    pub product_family: String,
    pub instrument_id: String,
    pub nt_instrument_id: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub rows: Vec<BybitMarkBarRow>,
}

/// Normalize an accepted Bybit `mark_price_kline_1m` REST object into a validated
/// canonical mark-price bars table.
///
/// `json_text` is the decompressed text of the accepted object. The envelope is
/// the same Bybit V5 REST shape as the trade kline, but each `result.list` row is
/// `[startMs, open, high, low, close]` (mark-price OHLC, no volume/turnover). The
/// envelope's `result.symbol` must equal `spec.venue_symbol`. Kline rows arrive
/// newest-first from the venue; they are sorted ascending by open time so the
/// projection is monotonic for NautilusTrader.
///
/// # Errors
///
/// Returns an error if the envelope is malformed, `retCode` is non-zero, the
/// symbol mismatches, a row is malformed, or the table fails validation.
pub fn normalize_bybit_mark_price_kline_1m(
    accepted: &AcceptedDataset,
    spec: &BybitInstrumentSpec,
    json_text: &str,
) -> Result<BybitMarkBarsTable> {
    let envelope: BybitKlineEnvelope =
        serde_json::from_str(json_text).context("parse bybit mark-price kline envelope")?;
    ensure!(
        envelope.ret_code == 0,
        "bybit mark-price kline retCode {} is not OK",
        envelope.ret_code
    );
    ensure!(
        envelope.result.symbol == spec.venue_symbol,
        "mark-price kline symbol {:?} does not match instrument {:?}",
        envelope.result.symbol,
        spec.venue_symbol
    );

    let mut rows = Vec::new();
    for (index, entry) in envelope.result.list.iter().enumerate() {
        ensure!(
            entry.len() >= MARK_KLINE_ROW_MIN_FIELDS,
            "mark-price kline row {index} has {} fields, expected at least {MARK_KLINE_ROW_MIN_FIELDS}",
            entry.len()
        );
        let start_ms: i64 = entry[0].trim().parse().with_context(|| {
            format!(
                "mark-price kline row {index}: invalid start ms {:?}",
                entry[0]
            )
        })?;
        let open_time = start_ms
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("mark-price kline row {index}: start ms overflow"))?;

        let open = entry[1].trim().to_string();
        let high = entry[2].trim().to_string();
        let low = entry[3].trim().to_string();
        let close = entry[4].trim().to_string();

        // Decimal-level OHLC integrity on the source mark prices before any
        // rounding. NautilusTrader's `Bar::new_checked` re-asserts the same
        // ordering on the rounded prices. Mark price is a settlement/PnL price
        // (strictly positive), so the positivity check is sound here.
        let (o, h, l, c) = (
            Decimal::from_str(&open)
                .with_context(|| format!("mark-price kline row {index}: open"))?,
            Decimal::from_str(&high)
                .with_context(|| format!("mark-price kline row {index}: high"))?,
            Decimal::from_str(&low)
                .with_context(|| format!("mark-price kline row {index}: low"))?,
            Decimal::from_str(&close)
                .with_context(|| format!("mark-price kline row {index}: close"))?,
        );
        ensure!(
            o > Decimal::ZERO,
            "mark-price kline row {index}: non-positive open"
        );
        ensure!(
            h >= o && h >= l && h >= c && l <= o && l <= c,
            "mark-price kline row {index}: OHLC integrity violated (o={o} h={h} l={l} c={c})"
        );

        rows.push(BybitMarkBarRow {
            open_time,
            open,
            high,
            low,
            close,
        });
    }

    // Venue returns newest-first; NautilusTrader requires ascending ts.
    rows.sort_by_key(|row| row.open_time);

    let table = BybitMarkBarsTable {
        source_proof_id: accepted.source_proof_id.clone(),
        venue: accepted.venue.clone(),
        product_family: accepted.product_family.clone(),
        instrument_id: spec.instrument_id.clone(),
        nt_instrument_id: spec.nt_instrument_id.clone(),
        fidelity_class: accepted.fidelity_class,
        forbidden_claims: accepted.forbidden_claims.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl BybitMarkBarsTable {
    /// The fixed NautilusTrader bar specification for a Bybit 1-minute mark-price
    /// kline: 1-MINUTE-MARK, externally aggregated by the venue.
    ///
    /// `PriceType::Mark` is NautilusTrader's native price type for mark price, so
    /// the resulting `BarType` is distinct from the trade kline's `…-LAST-…` id
    /// and never collides with it in the catalog.
    fn bar_type(nt_instrument_id: &str) -> Result<BarType> {
        let instrument_id = InstrumentId::from_str(nt_instrument_id)
            .with_context(|| format!("invalid nt_instrument_id {nt_instrument_id:?}"))?;
        let step = NonZeroUsize::new(1).expect("1 is non-zero");
        let spec = BarSpecification {
            step,
            aggregation: BarAggregation::Minute,
            price_type: PriceType::Mark,
        };
        Ok(BarType::new(
            instrument_id,
            spec,
            AggregationSource::External,
        ))
    }

    /// Validate non-emptiness, monotonic open times, and required fields.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.rows.is_empty(),
            "bybit mark-price bars table is empty"
        );
        ensure!(
            self.fidelity_class != SourceProofFidelityClass::L2Replay,
            "mark-price kline bars must not be labelled L2_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "mark-price bar table must carry explicit forbidden claims"
        );
        for field in [
            &self.source_proof_id,
            &self.venue,
            &self.product_family,
            &self.instrument_id,
            &self.nt_instrument_id,
        ] {
            ensure!(!field.trim().is_empty(), "empty provenance/identity field");
        }

        let mut previous_open_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.open_time > 0, "row {index}: non-positive open_time");
            ensure!(
                row.open_time >= previous_open_time,
                "row {index}: open_time {} precedes previous {}",
                row.open_time,
                previous_open_time
            );
            previous_open_time = row.open_time;
            for field in [&row.open, &row.high, &row.low, &row.close] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
        }
        Ok(())
    }

    /// The NautilusTrader bar-type string for catalog identifier resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if the instrument id is invalid.
    pub fn bar_type_string(&self) -> Result<String> {
        Ok(Self::bar_type(&self.nt_instrument_id)?.to_string())
    }

    /// Convert the canonical rows into NautilusTrader `Bar`s at the instrument's
    /// price precision. The volume is a zero `Quantity` at the instrument's size
    /// precision: a mark-price candle carries no traded size.
    ///
    /// # Errors
    ///
    /// Returns an error if a price cannot be represented at the precision or fails
    /// NautilusTrader's OHLC checks.
    pub fn to_bars(&self, spec: &BybitInstrumentSpec) -> Result<Vec<Bar>> {
        ensure!(
            spec.nt_instrument_id == self.nt_instrument_id,
            "spec instrument {:?} does not match table {:?}",
            spec.nt_instrument_id,
            self.nt_instrument_id
        );
        let bar_type = Self::bar_type(&spec.nt_instrument_id)?;
        let price_precision = spec.price_precision();
        let size_precision = spec.size_precision();
        let volume = nt_quantity(MARK_BAR_VOLUME, size_precision)?;
        self.rows
            .iter()
            .map(|row| {
                let open = nt_price(&row.open, price_precision)?;
                let high = nt_price(&row.high, price_precision)?;
                let low = nt_price(&row.low, price_precision)?;
                let close = nt_price(&row.close, price_precision)?;
                let ts =
                    UnixNanos::from(u64::try_from(row.open_time).context("negative open_time")?);
                Bar::new_checked(bar_type, open, high, low, close, volume, ts, ts)
                    .context("build NautilusTrader mark-price bar")
            })
            .collect()
    }
}

/// Project a canonical mark-price bars table into a NautilusTrader
/// `ParquetDataCatalog` as `Bar` data.
///
/// # Errors
///
/// Returns an error if conversion or the catalog write fails.
pub fn project_bybit_mark_bars_to_catalog(
    table: &BybitMarkBarsTable,
    spec: &BybitInstrumentSpec,
    catalog_root: &Path,
) -> Result<BybitCatalogProjection> {
    table.validate()?;
    let bars = table.to_bars(spec)?;
    let record_count = bars.len();
    ensure_clean_root(catalog_root)?;

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write mark-price bars to catalog")?;

    Ok(BybitCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: table.bar_type_string()?,
        data_type: NT_DATA_TYPE_MARK_BAR.to_string(),
        record_count,
        fidelity_class: table.fidelity_class,
    })
}

/// Read the projected mark-price `Bar` data back from `catalog_root` by its
/// bar-type id.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_mark_bars(catalog_root: &Path, bar_type: &str) -> Result<Vec<Bar>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<Bar>(
            Some(vec![bar_type.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query mark-price bars from catalog")
}

// ===========================================================================
// Bulk-append path (data-derived precision, honest object-key provenance,
// no clean-root guard)
// ===========================================================================
//
// Mirrors `super::canonical_okx::append_okx_trades_archive`: the hermetic
// `project_*_to_catalog` functions above stay as the single-object TEST harness
// (they refuse a dirty root); this section is the bulk-conversion path that
// appends many staged objects into one shared, possibly-S3 catalog with NO
// clean-root guard, relying on NautilusTrader's own per-instrument file naming.
//
// Precision is derived from each object's own rows (Bybit stages no per-object
// instrument universe), and the provenance/identity the canonical `*Table`
// requires is constructed from HONEST sources: the SHA-256 of the object bytes,
// the venue/family/category/symbol/date read from the S3 object key, and a fixed
// label naming this conversion run. No origin is fabricated.

/// Maximum decimal places of a decimal value string (`"643.3"` -> 1,
/// `"5995"` -> 0, `"0.10"` -> 1). This reads the precision the exchange rendered
/// for a *value*, distinct from [`decimal_places`] which reads the fractional
/// length of an *increment* string.
///
/// # Errors
///
/// Returns an error if `value` is not a decimal string or its scale exceeds
/// [`u8`].
fn value_decimal_places(value: &str) -> Result<u8> {
    let decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    u8::try_from(decimal.scale()).context("decimal scale exceeds u8")
}

/// The decimal-string increment whose fractional length is exactly `precision`
/// (`0 -> "1"`, `1 -> "0.1"`, `5 -> "0.00001"`) — the inverse of
/// [`decimal_places`]. Lets a data-derived precision be expressed as the
/// [`BybitInstrumentSpec`] increment the converter consumes.
#[must_use]
fn increment_for(precision: u8) -> String {
    match precision {
        0 => "1".to_string(),
        n => format!("0.{}1", "0".repeat(usize::from(n) - 1)),
    }
}

/// SHA-256 hex of the raw object bytes — the honest payload hash of THIS object.
#[must_use]
fn object_sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Value of a `key=value` segment in an S3 object key, for example the `symbol`
/// or `category` partition. Returns the first matching segment's value.
fn key_segment<'a>(object_key: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    object_key
        .split('/')
        .find_map(|segment| segment.strip_prefix(&prefix))
}

/// Required `key=value` segment of an S3 object key.
///
/// # Errors
///
/// Returns an error naming the missing partition key.
fn require_key_segment<'a>(object_key: &'a str, key: &str) -> Result<&'a str> {
    let value = key_segment(object_key, key)
        .with_context(|| format!("object key {object_key:?} missing {key}= segment"))?;
    ensure!(
        !value.trim().is_empty(),
        "object key {object_key:?} has empty {key}= segment"
    );
    Ok(value)
}

/// The honest archive date (YYYY-MM-DD) for an object key: the `dt=` partition if
/// present (trade archives), otherwise the date prefix of the first date-bearing
/// window/page segment (REST kline pages carry `window_start=`/`page_start=`).
///
/// # Errors
///
/// Returns an error if no date-bearing segment is present.
fn archive_date_from_key(object_key: &str) -> Result<String> {
    if let Some(dt) = key_segment(object_key, "dt") {
        ensure!(
            !dt.trim().is_empty(),
            "object key {object_key:?} has empty dt= segment"
        );
        return Ok(dt.to_string());
    }
    for key in ["window_start", "page_start", "window_end", "page_end"] {
        if let Some(value) = key_segment(object_key, key) {
            // Bybit stages these as `2026-03-13T12_00_00Z`; the leading 10 chars
            // are the calendar date.
            let date = value.get(0..10).unwrap_or(value);
            if !date.trim().is_empty() {
                return Ok(date.to_string());
            }
        }
    }
    bail!("object key {object_key:?} has no dt=/window/page date segment")
}

/// Build the honest [`AcceptedDataset`] describing THIS object: identity and
/// provenance read from the object key (`venue/family/category/symbol/date`) plus
/// the SHA-256 of the object bytes. Every field has a real source; none claims a
/// false origin.
///
/// `expected_family` is the S3 `family=` partition this converter accepts,
/// enforced against the key so a mis-routed object fails loud.
fn accepted_from_object_key(
    object_key: &str,
    object_bytes: &[u8],
    expected_family: &str,
    schema_columns: Vec<String>,
) -> Result<(AcceptedDataset, String)> {
    let family = require_key_segment(object_key, "family")?;
    ensure!(
        family == expected_family,
        "object key family {family:?} is not the expected {expected_family:?}"
    );
    let category = require_key_segment(object_key, "category")?;
    let symbol = require_key_segment(object_key, "symbol")?.to_string();
    let archive_date = archive_date_from_key(object_key)?;
    let sha256 = object_sha256_hex(object_bytes);

    let object = IngestManifestObjectRecord {
        s3_uri: object_key.to_string(),
        source_url: object_key.to_string(),
        sha256: sha256.clone(),
        bytes: object_bytes.len() as u64,
        archive_date,
        schema_columns,
    };
    let accepted = AcceptedDataset {
        source_proof_id: format!("{BYBIT_BULK_INGEST_RUN}:{sha256}"),
        source_proof_version: 1,
        source_binding: BYBIT_BULK_INGEST_RUN.to_string(),
        venue: "bybit".to_string(),
        product_family: category.to_string(),
        product_category: category.to_string(),
        instrument_universe_id: String::new(),
        fidelity_class: SourceProofFidelityClass::TradeBarReplay,
        forbidden_claims: vec![BYBIT_REPLAY_FORBIDDEN_CLAIM.to_string()],
        object,
    };
    ensure!(
        accepted.fidelity_class != SourceProofFidelityClass::L2Replay,
        "trade/bar replay must never be labelled L2_REPLAY"
    );
    Ok((accepted, symbol))
}

/// A probe [`BybitInstrumentSpec`] for one symbol whose increments are
/// placeholders: `normalize_*` consumes only `venue_symbol` (to fence rows) and
/// `nt_instrument_id` (matched by `to_*`), never the increments, so the real
/// precision is derived from the parsed rows afterwards.
fn probe_spec(symbol: &str) -> BybitInstrumentSpec {
    BybitInstrumentSpec {
        instrument_id: symbol.to_string(),
        venue_symbol: symbol.to_string(),
        nt_instrument_id: format!("{symbol}.{BYBIT_VENUE}"),
        // Placeholders: replaced by the data-derived spec before any `to_*` call.
        price_increment: "1".to_string(),
        size_increment: "1".to_string(),
    }
}

/// One instrument's write summary produced by an `append_bybit_*_archive` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BybitAppendSummary {
    pub nt_instrument_id: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Build a data-derived [`BybitInstrumentSpec`] from a symbol and the maximum
/// decimal places observed across the supplied price/size value columns.
///
/// Bybit renders every row of a column at the instrument's native tick/lot scale,
/// so the maximum observed scale is the precision NautilusTrader pins per catalog
/// file — no external instrument universe is needed, and Bybit stages none.
///
/// # Errors
///
/// Returns an error if any value string is not decimal.
fn spec_from_value_columns<'a>(
    symbol: &str,
    price_values: impl IntoIterator<Item = &'a str>,
    size_values: impl IntoIterator<Item = &'a str>,
) -> Result<BybitInstrumentSpec> {
    let mut price_precision = 0u8;
    for value in price_values {
        price_precision = price_precision.max(value_decimal_places(value)?);
    }
    let mut size_precision = 0u8;
    for value in size_values {
        size_precision = size_precision.max(value_decimal_places(value)?);
    }
    Ok(BybitInstrumentSpec {
        instrument_id: symbol.to_string(),
        venue_symbol: symbol.to_string(),
        nt_instrument_id: format!("{symbol}.{BYBIT_VENUE}"),
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
    })
}

/// Append one Bybit derivatives (linear/inverse) tick-trades archive object into
/// an already-open [`ParquetDataCatalog`] as `TradeTick` data — the bulk path.
///
/// `gz_bytes` is the raw `.csv.gz` object body as staged under
/// `source=public_archive/family=tick_trades/category={linear|inverse}/`. The
/// symbol/category/date and provenance are read from `object_key` (the S3 key);
/// precision is derived from the object's own price/size columns. Unlike
/// [`project_bybit_trades_to_catalog`] (the hermetic proof harness, which refuses
/// a dirty root), this appends into a shared catalog with NO clean-root guard.
///
/// Returns one [`BybitAppendSummary`] (a Bybit trade archive object is a single
/// symbol; the `Vec` mirrors the multi-instrument OKX path).
///
/// # Errors
///
/// Returns an error if decompression, key parsing, normalization, tick
/// construction, or the catalog write fails.
pub fn append_bybit_deriv_tick_trades_archive(
    gz_bytes: &[u8],
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<BybitAppendSummary>> {
    let csv_text = gunzip_to_string(gz_bytes).context("gunzip bybit tick-trades object")?;
    let (accepted, symbol) = accepted_from_object_key(
        object_key,
        gz_bytes,
        "tick_trades",
        BYBIT_DERIV_TICK_TRADES_REQUIRED_PREFIX
            .iter()
            .map(|column| (*column).to_string())
            .collect(),
    )?;

    let probe = probe_spec(&symbol);
    let table = normalize_bybit_deriv_tick_trades(&accepted, &probe, &csv_text)?;
    let derived = spec_from_value_columns(
        &symbol,
        table.rows.iter().map(|row| row.price.as_str()),
        table.rows.iter().map(|row| row.size.as_str()),
    )?;
    let ticks = table.to_trade_ticks(&derived)?;
    let summary = BybitAppendSummary {
        nt_instrument_id: derived.nt_instrument_id.clone(),
        record_count: ticks.len(),
        price_precision: derived.price_precision(),
        size_precision: derived.size_precision(),
    };
    catalog
        .write_to_parquet(ticks, None, None, None)
        .with_context(|| format!("append bybit trade ticks for {symbol}"))?;
    Ok(vec![summary])
}

/// Append one Bybit `mark_price_kline_1m` REST archive object into an already-open
/// [`ParquetDataCatalog`] as mark-price `Bar` data — the bulk path.
///
/// `json_bytes` is the raw `.json` object body as staged under
/// `source=rest/family=mark_price_kline_1m/category={linear|inverse}/`. The
/// symbol/category/date and provenance are read from `object_key`; price
/// precision is derived from the object's own OHLC columns (a mark candle has no
/// traded size, so size precision is 0). Unlike
/// [`project_bybit_mark_bars_to_catalog`] (the hermetic proof harness, which
/// refuses a dirty root), this appends into a shared catalog with NO clean-root
/// guard.
///
/// Returns one [`BybitAppendSummary`].
///
/// # Errors
///
/// Returns an error if key parsing, normalization, bar construction, or the
/// catalog write fails.
pub fn append_bybit_mark_price_kline_1m_archive(
    json_bytes: &[u8],
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<BybitAppendSummary>> {
    let json_text =
        std::str::from_utf8(json_bytes).context("bybit mark-price kline object is not UTF-8")?;
    let (accepted, symbol) = accepted_from_object_key(
        object_key,
        json_bytes,
        "mark_price_kline_1m",
        ["start", "open", "high", "low", "close"]
            .iter()
            .map(|column| (*column).to_string())
            .collect(),
    )?;

    let probe = probe_spec(&symbol);
    let table = normalize_bybit_mark_price_kline_1m(&accepted, &probe, json_text)?;
    // A mark-price candle carries no traded size; the volume column is the fixed
    // zero placeholder, so size precision is data-derived as 0 (empty size set).
    let derived = spec_from_value_columns(
        &symbol,
        table.rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }),
        std::iter::empty::<&str>(),
    )?;
    let bars = table.to_bars(&derived)?;
    let summary = BybitAppendSummary {
        nt_instrument_id: derived.nt_instrument_id.clone(),
        record_count: bars.len(),
        price_precision: derived.price_precision(),
        size_precision: derived.size_precision(),
    };
    catalog
        .write_to_parquet(bars, None, None, None)
        .with_context(|| format!("append bybit mark-price bars for {symbol}"))?;
    Ok(vec![summary])
}

/// Bulk-append every Bybit `mark_price_kline_1m` REST object for a binding in a
/// single pass, deduplicating bars by open time per instrument.
///
/// Bybit's REST mark-price pages overlap in time — the backfill over-fetches
/// adjacent/redundant windows — so writing each page as its own catalog file
/// produces non-disjoint intervals for the same instrument and NautilusTrader's
/// `write_to_parquet` rejects it. This collects every page's 1-minute bars keyed
/// by `(symbol, open_time)` so duplicate minutes collapse, derives one uniform
/// precision per instrument across all pages, then writes one ascending `Bar`
/// stream per instrument — the single disjoint write the catalog contract
/// expects.
///
/// # Errors
///
/// Returns an error if any object is malformed, a price cannot be represented at
/// the derived precision, or a catalog write fails.
pub fn append_bybit_mark_price_kline_1m_batch(
    objects: &[(String, Vec<u8>)],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<BybitAppendSummary>> {
    // symbol -> (open_time -> row); BTreeMap keys keep minutes unique + ascending.
    let mut rows_by_symbol: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<i64, BybitMarkBarRow>,
    > = std::collections::BTreeMap::new();
    // First validated table per symbol, reused as the metadata template.
    let mut table_by_symbol: std::collections::BTreeMap<String, BybitMarkBarsTable> =
        std::collections::BTreeMap::new();

    for (object_key, bytes) in objects {
        let json_text = std::str::from_utf8(bytes)
            .with_context(|| format!("bybit mark-price kline object {object_key} is not UTF-8"))?;
        let (accepted, symbol) = accepted_from_object_key(
            object_key,
            bytes,
            "mark_price_kline_1m",
            ["start", "open", "high", "low", "close"]
                .iter()
                .map(|column| (*column).to_string())
                .collect(),
        )?;
        let probe = probe_spec(&symbol);
        let table = normalize_bybit_mark_price_kline_1m(&accepted, &probe, json_text)?;
        let dedup = rows_by_symbol.entry(symbol.clone()).or_default();
        for row in &table.rows {
            dedup.insert(row.open_time, row.clone());
        }
        table_by_symbol.entry(symbol).or_insert(table);
    }

    let mut summaries = Vec::new();
    for (symbol, rows_map) in rows_by_symbol {
        let rows: Vec<BybitMarkBarRow> = rows_map.into_values().collect();
        let derived = spec_from_value_columns(
            &symbol,
            rows.iter().flat_map(|row| {
                [
                    row.open.as_str(),
                    row.high.as_str(),
                    row.low.as_str(),
                    row.close.as_str(),
                ]
            }),
            std::iter::empty::<&str>(),
        )?;
        let mut table = table_by_symbol
            .remove(&symbol)
            .expect("symbol has a metadata template");
        table.rows = rows;
        let bars = table.to_bars(&derived)?;
        let summary = BybitAppendSummary {
            nt_instrument_id: derived.nt_instrument_id.clone(),
            record_count: bars.len(),
            price_precision: derived.price_precision(),
            size_precision: derived.size_precision(),
        };
        catalog
            .write_to_parquet(bars, None, None, None)
            .with_context(|| format!("append deduplicated bybit mark-price bars for {symbol}"))?;
        summaries.push(summary);
    }
    Ok(summaries)
}

/// Decompress a gzip object body to UTF-8 text.
///
/// # Errors
///
/// Returns an error if inflation fails or the inflated bytes are not UTF-8.
fn gunzip_to_string(gz_bytes: &[u8]) -> Result<String> {
    let mut decoder = GzDecoder::new(gz_bytes);
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .context("inflate gzip object body")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deriv_seconds_to_nanos_is_exact() {
        assert_eq!(
            deriv_seconds_to_nanos("1779321780.4324").unwrap(),
            1_779_321_780_432_400_000
        );
        assert_eq!(
            deriv_seconds_to_nanos("1779321780").unwrap(),
            1_779_321_780_000_000_000
        );
    }

    #[test]
    fn decimal_places_reads_increment_precision() {
        assert_eq!(decimal_places("0.00001"), 5);
        assert_eq!(decimal_places("1"), 0);
        assert_eq!(decimal_places("0.10"), 2);
    }

    #[test]
    fn aggressor_side_parses_capitalised_tokens() {
        assert_eq!(
            BybitAggressorSide::parse("Buy").unwrap(),
            BybitAggressorSide::Buyer
        );
        assert_eq!(
            BybitAggressorSide::parse("Sell").unwrap(),
            BybitAggressorSide::Seller
        );
        assert!(BybitAggressorSide::parse("Hold").is_err());
    }
}
