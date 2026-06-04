//! Deribit — canonical normalization and NautilusTrader catalog projection for
//! three Deribit market-data families:
//!
//! 1. **Tardis options-chain** (gzip-CSV): a per-instrument top-of-book time
//!    series, projected to NautilusTrader `QuoteTick`s + a `CryptoOption`.
//! 2. **RiveChen merged trades** (Parquet): native Deribit public-trade prints,
//!    projected to NautilusTrader `TradeTick`s.
//! 3. **1m OHLC bars** (Deribit `get_tradingview_chart_data` JSON): exchange-
//!    aggregated 1-minute candles, projected to NautilusTrader `Bar`s.
//!
//! ```text
//! options-chain gzip-CSV  -> top-of-book rows  -> QuoteTick + CryptoOption
//! merged-trades Parquet   -> trade rows        -> TradeTick
//! 1m-bars TradingView JSON-> OHLC bar rows      -> Bar (External / Last / 1-MINUTE)
//!   -> NautilusTrader `ParquetDataCatalog::write_to_parquet`
//!   -> `query_typed_data::<T>` read-back (count + ordering + payload proven)
//! ```
//!
//! Bolt owns parsing and normalization; NautilusTrader owns the catalog and the
//! per-type Arrow schema. No raw arrow/parquet is hand-rolled for any NT type:
//! the trades source Parquet is bolt-owned staged data (read with
//! NautilusTrader-independent Arrow), but every NautilusTrader type is written
//! and read exclusively through NautilusTrader's own catalog API.
//!
//! Everything that varies per instrument (id, precision, strike, expiry,
//! currencies, option kind, bar step/unit) is supplied by the caller via a spec
//! struct; the only literals are fixed source-schema facts (column names, the
//! micros/millis->nanos scales, the Deribit trade-`direction` aggressor
//! convention), which are properties of the upstream formats themselves, not
//! runtime config.

use std::{
    fs,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use arrow::array::{Array, Float64Array, Int64Array, StringArray};
use flate2::read::GzDecoder;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Bar, BarSpecification, BarType, QuoteTick, TradeTick},
    enums::{AggregationSource, AggressorSide, BarAggregation, OptionKind, PriceType},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{CryptoOption, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_QUOTE_TICK: &str = "QuoteTick";

/// NautilusTrader data type written for the merged-trades family.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the 1m-bars family.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// Required Parquet columns the RiveChen merged-trades object must expose.
///
/// A property of the upstream scraper's Parquet layout, not runtime config.
pub const DERIBIT_MERGED_TRADES_REQUIRED_COLUMNS: [&str; 5] = [
    "trade_id",
    "timestamp",
    "price",
    "instrument_name",
    "direction",
];

/// Source trade `direction` token: the side of the aggressor (taker). Deribit's
/// public-trade `direction` field is the taker's side, so `buy` is a
/// buyer-initiated trade and `sell` is seller-initiated. A property of the
/// Deribit trades API, not runtime config.
pub const DERIBIT_TRADE_DIRECTION_BUY: &str = "buy";
pub const DERIBIT_TRADE_DIRECTION_SELL: &str = "sell";

/// Source trade `timestamp` is Unix milliseconds; NautilusTrader `UnixNanos`
/// are nanoseconds.
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// Tardis options-chain header, in source order. A property of the upstream
/// archive format (not runtime config), so it lives in code as the parse fence.
pub const DERIBIT_OPTIONS_CHAIN_HEADER: [&str; 24] = [
    "exchange",
    "symbol",
    "timestamp",
    "local_timestamp",
    "type",
    "strike_price",
    "expiration",
    "open_interest",
    "last_price",
    "bid_price",
    "bid_amount",
    "bid_iv",
    "ask_price",
    "ask_amount",
    "ask_iv",
    "mark_price",
    "mark_iv",
    "underlying_index",
    "underlying_price",
    "delta",
    "gamma",
    "vega",
    "theta",
    "rho",
];

/// Source `timestamp` / `local_timestamp` are Unix microseconds; NautilusTrader
/// `UnixNanos` are nanoseconds.
const NANOS_PER_MICROSECOND: i64 = 1_000;

/// Column indices into a parsed options-chain row.
const COL_SYMBOL: usize = 1;
const COL_TIMESTAMP: usize = 2;
const COL_LOCAL_TIMESTAMP: usize = 3;
const COL_BID_PRICE: usize = 9;
const COL_BID_AMOUNT: usize = 10;
const COL_ASK_PRICE: usize = 12;
const COL_ASK_AMOUNT: usize = 13;

/// Accepted Deribit option-instrument metadata needed to build the
/// NautilusTrader `CryptoOption` and to project the top-of-book series.
///
/// Built by the caller from accepted instrument-universe data; nothing here is
/// hardcoded in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeribitOptionInstrumentSpec {
    /// NautilusTrader instrument id, for example `<symbol>.DERIBIT`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol exactly as it appears in the source `symbol`
    /// column. Rows for other symbols are ignored, so one spec maps one series.
    pub raw_symbol: String,
    /// Underlying currency code, for example the option's coin.
    pub underlying: String,
    /// Quote currency code (premium currency).
    pub quote_currency: String,
    /// Settlement currency code.
    pub settlement_currency: String,
    /// Whether the option is inverse-settled.
    pub is_inverse: bool,
    /// Option kind: must parse as `CALL` or `PUT`.
    pub option_kind: String,
    /// Strike price as a decimal string.
    pub strike_price: String,
    /// Activation (listing) time in Unix nanoseconds.
    pub activation_ns: u64,
    /// Expiration time in Unix nanoseconds.
    pub expiration_ns: u64,
    /// Premium price tick size as a decimal string, for example `0.0001`.
    pub price_increment: String,
    /// Contract size increment as a decimal string, for example `1`.
    pub size_increment: String,
}

/// One normalized top-of-book row: a two-sided quote that survived the
/// one-sided/empty filter, with timestamps already in nanoseconds and the exact
/// source price/size strings preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeribitQuoteRow {
    /// Exchange event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Capture (`local_timestamp`) timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Exact source best-bid price string.
    pub bid_price: String,
    /// Exact source best-bid size string.
    pub bid_size: String,
    /// Exact source best-ask price string.
    pub ask_price: String,
    /// Exact source best-ask size string.
    pub ask_size: String,
}

/// A validated canonical top-of-book series for one accepted Deribit option.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeribitQuoteSeries {
    /// Venue-native raw symbol the series belongs to.
    pub raw_symbol: String,
    /// Count of source rows skipped because they were not two-sided quotes.
    pub skipped_one_sided: usize,
    /// Normalized two-sided rows in source order (event-time non-decreasing).
    pub rows: Vec<DeribitQuoteRow>,
}

/// Result of projecting a top-of-book series into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeribitCatalogProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub quote_count: usize,
}

/// Decimal places implied by a decimal-string increment (`0.1` -> 1,
/// `0.0001` -> 4, `1` -> 0, `0.10` -> 2). Trailing zeros are significant and
/// must agree with the precision `Price::from_str`/`Quantity::from_str` infer
/// from the same increment string.
#[must_use]
fn decimal_places(increment: &str) -> u8 {
    match increment.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    }
}

/// Rescale a decimal string to `precision` places, failing closed if the source
/// carries more precision than the instrument declares.
fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Decompress a gzip-compressed options-chain object into UTF-8 text.
///
/// # Errors
///
/// Returns an error if the file cannot be read or is not valid gzip / UTF-8.
pub fn read_gzip_csv(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read gzip object {}", path.display()))?;
    let mut decoder = GzDecoder::new(bytes.as_slice());
    let mut text = String::new();
    decoder
        .read_to_string(&mut text)
        .with_context(|| format!("gunzip {}", path.display()))?;
    Ok(text)
}

/// Normalize an accepted Deribit options-chain CSV into the canonical top-of-book
/// series for a single instrument.
///
/// `csv_text` is the decompressed text of the accepted object. Only rows whose
/// `symbol` equals `spec.raw_symbol` and that carry BOTH a bid and an ask price
/// become quotes; one-sided or empty rows are skipped and counted. Source
/// `timestamp`/`local_timestamp` (microseconds) are converted to nanoseconds.
///
/// # Errors
///
/// Returns an error if the header does not match the Tardis options-chain
/// schema, a matching row is malformed, timestamps are non-monotonic, or a
/// price/size fails to parse.
pub fn normalize_deribit_options_chain(
    csv_text: &str,
    spec: &DeribitOptionInstrumentSpec,
) -> Result<DeribitQuoteSeries> {
    ensure!(
        !spec.raw_symbol.trim().is_empty(),
        "spec.raw_symbol must not be empty"
    );

    let mut lines = csv_text.lines();
    let header = lines.next().context("empty csv: missing header")?;
    let header_columns: Vec<&str> = header.split(',').map(str::trim).collect();
    ensure!(
        header_columns == DERIBIT_OPTIONS_CHAIN_HEADER,
        "csv header {header_columns:?} does not match expected options-chain header {DERIBIT_OPTIONS_CHAIN_HEADER:?}"
    );

    let mut rows = Vec::new();
    let mut skipped_one_sided = 0usize;

    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == DERIBIT_OPTIONS_CHAIN_HEADER.len(),
            "row {index} has {} fields, expected {}",
            fields.len(),
            DERIBIT_OPTIONS_CHAIN_HEADER.len()
        );

        // One spec maps exactly one instrument's series; ignore other symbols.
        if fields[COL_SYMBOL].trim() != spec.raw_symbol {
            continue;
        }

        let bid_price = fields[COL_BID_PRICE].trim();
        let bid_size = fields[COL_BID_AMOUNT].trim();
        let ask_price = fields[COL_ASK_PRICE].trim();
        let ask_size = fields[COL_ASK_AMOUNT].trim();

        // A `QuoteTick` is a two-sided top-of-book state. Rows that are
        // one-sided (only a bid or only an ask) or fully empty cannot form a
        // quote; skip and count them rather than fabricating a side.
        if bid_price.is_empty() || ask_price.is_empty() {
            skipped_one_sided += 1;
            continue;
        }
        ensure!(
            !bid_size.is_empty() && !ask_size.is_empty(),
            "row {index}: two-sided price with a missing size"
        );

        let timestamp_us: i64 = fields[COL_TIMESTAMP].trim().parse().with_context(|| {
            format!("row {index}: invalid timestamp {:?}", fields[COL_TIMESTAMP])
        })?;
        let local_us: i64 = fields[COL_LOCAL_TIMESTAMP]
            .trim()
            .parse()
            .with_context(|| {
                format!(
                    "row {index}: invalid local_timestamp {:?}",
                    fields[COL_LOCAL_TIMESTAMP]
                )
            })?;

        let event_time = timestamp_us
            .checked_mul(NANOS_PER_MICROSECOND)
            .with_context(|| format!("row {index}: timestamp overflow"))?;
        let capture_time = local_us
            .checked_mul(NANOS_PER_MICROSECOND)
            .with_context(|| format!("row {index}: local_timestamp overflow"))?;

        ensure!(event_time > 0, "row {index}: non-positive event_time");

        for (label, raw) in [
            ("bid_price", bid_price),
            ("bid_amount", bid_size),
            ("ask_price", ask_price),
            ("ask_amount", ask_size),
        ] {
            let decimal: Decimal = raw
                .parse()
                .with_context(|| format!("row {index}: invalid {label} {raw:?}"))?;
            ensure!(
                decimal > Decimal::ZERO,
                "row {index}: non-positive {label} {raw:?}"
            );
        }

        rows.push(DeribitQuoteRow {
            event_time,
            capture_time,
            bid_price: bid_price.to_string(),
            bid_size: bid_size.to_string(),
            ask_price: ask_price.to_string(),
            ask_size: ask_size.to_string(),
        });
    }

    // Real Deribit options-chain rows are per-instrument BBO snapshots that are
    // not strictly time-ordered in file order (near-simultaneous rows interleave).
    // NT's catalog write contract requires non-decreasing ts_init, so sort the
    // collected rows by event_time. Stable sort preserves capture order on ties.
    rows.sort_by_key(|row| row.event_time);

    let series = DeribitQuoteSeries {
        raw_symbol: spec.raw_symbol.clone(),
        skipped_one_sided,
        rows,
    };
    series.validate()?;
    Ok(series)
}

impl DeribitQuoteSeries {
    /// Validate the series carries at least one quote with non-decreasing,
    /// positive event times.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.raw_symbol.trim().is_empty(),
            "series has empty raw_symbol"
        );
        ensure!(!self.rows.is_empty(), "deribit quote series is empty");
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            ensure!(
                row.event_time >= previous,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous
            );
            previous = row.event_time;
            for field in [&row.bid_price, &row.bid_size, &row.ask_price, &row.ask_size] {
                ensure!(!field.trim().is_empty(), "row {index}: empty quote field");
            }
        }
        Ok(())
    }
}

/// Build the NautilusTrader `CryptoOption` instrument from accepted metadata.
///
/// # Errors
///
/// Returns an error if any field fails to parse.
pub fn build_crypto_option(spec: &DeribitOptionInstrumentSpec) -> Result<CryptoOption> {
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let price_precision = decimal_places(&spec.price_increment);
    let size_precision = decimal_places(&spec.size_increment);
    let underlying = Currency::from_str(&spec.underlying)
        .with_context(|| format!("invalid underlying {:?}", spec.underlying))?;
    let quote_currency = Currency::from_str(&spec.quote_currency)
        .with_context(|| format!("invalid quote_currency {:?}", spec.quote_currency))?;
    let settlement_currency = Currency::from_str(&spec.settlement_currency)
        .with_context(|| format!("invalid settlement_currency {:?}", spec.settlement_currency))?;
    let option_kind = match spec.option_kind.trim().to_ascii_uppercase().as_str() {
        "CALL" => OptionKind::Call,
        "PUT" => OptionKind::Put,
        other => bail!("invalid option_kind {other:?}"),
    };
    let strike_price = Price::from_str(&spec.strike_price).map_err(|error| {
        anyhow::anyhow!("invalid strike_price {:?}: {error}", spec.strike_price)
    })?;
    let price_increment = Price::from_str(&spec.price_increment).map_err(|error| {
        anyhow::anyhow!(
            "invalid price_increment {:?}: {error}",
            spec.price_increment
        )
    })?;
    let size_increment = Quantity::from_str(&spec.size_increment).map_err(|error| {
        anyhow::anyhow!("invalid size_increment {:?}: {error}", spec.size_increment)
    })?;

    Ok(CryptoOption::new(
        instrument_id,
        Symbol::from(spec.raw_symbol.as_str()),
        underlying,
        quote_currency,
        settlement_currency,
        spec.is_inverse,
        option_kind,
        strike_price,
        UnixNanos::from(spec.activation_ns),
        UnixNanos::from(spec.expiration_ns),
        price_precision,
        size_precision,
        price_increment,
        size_increment,
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
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

/// Convert a normalized top-of-book series into NautilusTrader `QuoteTick`s at
/// the instrument's price/size precision.
///
/// Both sides are rescaled to the same instrument precision, so the resulting
/// `QuoteTick`s satisfy `QuoteTick::new`'s bid/ask precision-equality invariant.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn series_to_quote_ticks(
    series: &DeribitQuoteSeries,
    instrument: &CryptoOption,
) -> Result<Vec<QuoteTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    series
        .rows
        .iter()
        .map(|row| {
            let bid_price = Price::from_str(&rescaled(&row.bid_price, price_precision)?)
                .map_err(|error| anyhow::anyhow!("invalid bid_price: {error}"))?;
            let ask_price = Price::from_str(&rescaled(&row.ask_price, price_precision)?)
                .map_err(|error| anyhow::anyhow!("invalid ask_price: {error}"))?;
            let bid_size = Quantity::from_str(&rescaled(&row.bid_size, size_precision)?)
                .map_err(|error| anyhow::anyhow!("invalid bid_size: {error}"))?;
            let ask_size = Quantity::from_str(&rescaled(&row.ask_size, size_precision)?)
                .map_err(|error| anyhow::anyhow!("invalid ask_size: {error}"))?;
            let ts = UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
            Ok(QuoteTick::new(
                instrument_id,
                bid_price,
                ask_price,
                bid_size,
                ask_size,
                ts,
                ts,
            ))
        })
        .collect()
}

/// Project a normalized top-of-book series into a NautilusTrader
/// `ParquetDataCatalog` as `QuoteTick` data plus the venue `CryptoOption`
/// instrument, using NautilusTrader APIs directly.
///
/// Fails closed on a dirty (non-empty) catalog root, because
/// `write_to_parquet` appends and would silently mix stale data.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_series_to_catalog(
    series: &DeribitQuoteSeries,
    spec: &DeribitOptionInstrumentSpec,
    catalog_root: &Path,
) -> Result<DeribitCatalogProjection> {
    series.validate()?;
    let instrument = build_crypto_option(spec)?;
    let instrument_id = instrument.id();
    ensure!(
        series.raw_symbol == spec.raw_symbol,
        "series symbol {:?} does not match spec {:?}",
        series.raw_symbol,
        spec.raw_symbol
    );
    let ticks = series_to_quote_ticks(series, &instrument)?;
    let quote_count = ticks.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CryptoOption(instrument)])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write quote ticks to catalog")?;

    Ok(DeribitCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_QUOTE_TICK.to_string(),
        quote_count,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `QuoteTick` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_quote_ticks(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<QuoteTick>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<QuoteTick>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query quote ticks from catalog")
}

// ===========================================================================
// Family 2: RiveChen merged trades (Parquet) -> NautilusTrader `TradeTick`
// ===========================================================================

/// Caller-supplied metadata for the Deribit option whose native trade prints
/// are projected. Mirrors [`DeribitOptionInstrumentSpec`] but is named for the
/// trades family so the two projections stay independent. Nothing here is
/// hardcoded in this module; precision is derived from the increment strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeribitTradesInstrumentSpec {
    /// NautilusTrader instrument id, for example `<symbol>.DERIBIT`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol exactly as it appears in the source
    /// `instrument_name` column. Rows for other instruments are ignored.
    pub raw_symbol: String,
    /// Underlying currency code.
    pub underlying: String,
    /// Quote currency code (premium currency).
    pub quote_currency: String,
    /// Settlement currency code.
    pub settlement_currency: String,
    /// Whether the option is inverse-settled.
    pub is_inverse: bool,
    /// Option kind: must parse as `CALL` or `PUT`.
    pub option_kind: String,
    /// Strike price as a decimal string.
    pub strike_price: String,
    /// Activation (listing) time in Unix nanoseconds.
    pub activation_ns: u64,
    /// Expiration time in Unix nanoseconds.
    pub expiration_ns: u64,
    /// Premium price tick size as a decimal string, for example `0.0001`.
    pub price_increment: String,
    /// Contract size increment as a decimal string, for example `1`.
    pub size_increment: String,
}

impl DeribitTradesInstrumentSpec {
    /// Build the NautilusTrader `CryptoOption` for the trades family.
    ///
    /// # Errors
    ///
    /// Returns an error if any field fails to parse.
    pub fn build_instrument(&self) -> Result<CryptoOption> {
        build_crypto_option(&DeribitOptionInstrumentSpec {
            nt_instrument_id: self.nt_instrument_id.clone(),
            raw_symbol: self.raw_symbol.clone(),
            underlying: self.underlying.clone(),
            quote_currency: self.quote_currency.clone(),
            settlement_currency: self.settlement_currency.clone(),
            is_inverse: self.is_inverse,
            option_kind: self.option_kind.clone(),
            strike_price: self.strike_price.clone(),
            activation_ns: self.activation_ns,
            expiration_ns: self.expiration_ns,
            price_increment: self.price_increment.clone(),
            size_increment: self.size_increment.clone(),
        })
    }
}

/// Aggressor side of a native Deribit trade print, mapped from the source
/// `direction` token (the taker's side).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DeribitTradeAggressorSide {
    Buyer,
    Seller,
}

impl DeribitTradeAggressorSide {
    /// Map the Deribit `direction` token to the aggressor side. `buy` ->
    /// buyer-initiated (BUYER); `sell` -> seller-initiated (SELLER).
    ///
    /// # Errors
    ///
    /// Returns an error for any token other than `buy`/`sell`.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            DERIBIT_TRADE_DIRECTION_BUY => Ok(Self::Buyer),
            DERIBIT_TRADE_DIRECTION_SELL => Ok(Self::Seller),
            other => bail!("unknown trade direction token: {other:?}"),
        }
    }

    fn to_nt(self) -> AggressorSide {
        match self {
            Self::Buyer => AggressorSide::Buyer,
            Self::Seller => AggressorSide::Seller,
        }
    }
}

/// One normalized native-trade row: timestamps already in nanoseconds, the
/// aggressor side mapped, and the exact source price/size doubles preserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeribitTradeRow {
    /// Exchange event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Venue-native trade id.
    pub trade_id: String,
    /// Aggressor (taker) side.
    pub aggressor_side: DeribitTradeAggressorSide,
    /// Source trade price.
    pub price: f64,
    /// Source trade size (contract amount).
    pub size: f64,
}

/// A validated canonical merged-trades series for one accepted Deribit option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeribitTradesSeries {
    /// Venue-native raw symbol the series belongs to.
    pub raw_symbol: String,
    /// Count of source rows skipped because they belonged to another symbol.
    pub skipped_other_symbol: usize,
    /// Normalized rows, sorted ascending by event time.
    pub rows: Vec<DeribitTradeRow>,
}

impl DeribitTradesSeries {
    /// Validate the series carries at least one trade with non-decreasing,
    /// positive event times and positive prices/sizes.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.raw_symbol.trim().is_empty(),
            "trades series has empty raw_symbol"
        );
        ensure!(!self.rows.is_empty(), "deribit trades series is empty");
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.event_time > 0, "trade {index}: non-positive event_time");
            ensure!(
                row.event_time >= previous,
                "trade {index}: event_time {} precedes previous {}",
                row.event_time,
                previous
            );
            previous = row.event_time;
            ensure!(
                !row.trade_id.trim().is_empty(),
                "trade {index}: empty trade_id"
            );
            ensure!(
                row.price > 0.0,
                "trade {index}: non-positive price {}",
                row.price
            );
            ensure!(
                row.size > 0.0,
                "trade {index}: non-positive size {}",
                row.size
            );
        }
        Ok(())
    }
}

/// Read a column out of a record batch, failing loud on a wrong Arrow type.
fn typed_column<'a, A: Array + 'static>(
    batch: &'a arrow::record_batch::RecordBatch,
    name: &str,
) -> Result<&'a A> {
    let column = batch
        .column_by_name(name)
        .with_context(|| format!("missing column {name:?}"))?;
    column
        .as_any()
        .downcast_ref::<A>()
        .with_context(|| format!("column {name:?} has unexpected Arrow type"))
}

/// Normalize an accepted RiveChen merged-trades Parquet object into the
/// canonical trades series for a single instrument.
///
/// The source object is bolt-owned staged data (one scraper run's merged
/// trades, mixing many instruments), so it is read with NautilusTrader-
/// independent Arrow. Only rows whose `instrument_name` equals
/// `spec.raw_symbol` become trades; other instruments are skipped and counted.
/// Source `timestamp` (milliseconds) is converted to nanoseconds, and the rows
/// are sorted ascending by event time to satisfy NautilusTrader's catalog
/// write contract (non-decreasing `ts_init`).
///
/// # Errors
///
/// Returns an error if the file cannot be read, a required column is missing or
/// of the wrong Arrow type, a value is null, a direction token is unknown, or
/// the series fails validation.
pub fn normalize_deribit_merged_trades(
    path: &Path,
    spec: &DeribitTradesInstrumentSpec,
) -> Result<DeribitTradesSeries> {
    ensure!(
        !spec.raw_symbol.trim().is_empty(),
        "spec.raw_symbol must not be empty"
    );

    let file = File::open(path).with_context(|| format!("open parquet {}", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| format!("open parquet reader {}", path.display()))?;

    let schema = builder.schema().clone();
    for column in DERIBIT_MERGED_TRADES_REQUIRED_COLUMNS {
        ensure!(
            schema.column_with_name(column).is_some(),
            "staged object {} is missing required column {:?}",
            path.display(),
            column
        );
    }

    let reader = builder
        .build()
        .with_context(|| format!("build parquet reader {}", path.display()))?;

    let mut rows: Vec<DeribitTradeRow> = Vec::new();
    let mut skipped_other_symbol = 0usize;
    for batch in reader {
        let batch = batch.context("read parquet record batch")?;
        let trade_id = typed_column::<StringArray>(&batch, "trade_id")?;
        let timestamp = typed_column::<Int64Array>(&batch, "timestamp")?;
        let price = typed_column::<Float64Array>(&batch, "price")?;
        let instrument_name = typed_column::<StringArray>(&batch, "instrument_name")?;
        let direction = typed_column::<StringArray>(&batch, "direction")?;
        let amount = typed_column::<Float64Array>(&batch, "amount")?;

        for index in 0..batch.num_rows() {
            ensure!(
                !instrument_name.is_null(index),
                "null instrument_name in row {index} of {}",
                path.display()
            );
            if instrument_name.value(index) != spec.raw_symbol {
                skipped_other_symbol += 1;
                continue;
            }
            ensure!(
                !trade_id.is_null(index)
                    && !timestamp.is_null(index)
                    && !price.is_null(index)
                    && !direction.is_null(index)
                    && !amount.is_null(index),
                "null value in matched row {index} of {}",
                path.display()
            );
            let millis = timestamp.value(index);
            let event_time = millis.checked_mul(NANOS_PER_MILLISECOND).with_context(|| {
                format!("row {index}: timestamp {millis} overflows nanoseconds")
            })?;
            ensure!(event_time > 0, "row {index}: non-positive event_time");
            let aggressor = DeribitTradeAggressorSide::parse(direction.value(index))
                .with_context(|| format!("row {index}: invalid direction"))?;
            rows.push(DeribitTradeRow {
                event_time,
                trade_id: trade_id.value(index).to_string(),
                aggressor_side: aggressor,
                price: price.value(index),
                size: amount.value(index),
            });
        }
    }

    // Real merged-trades objects interleave instruments; per-instrument the
    // rows may still arrive slightly out of event-time order. NT's catalog
    // write contract requires non-decreasing ts_init, so sort by event time.
    // Stable sort preserves source order on ties (same-timestamp prints).
    rows.sort_by_key(|row| row.event_time);

    let series = DeribitTradesSeries {
        raw_symbol: spec.raw_symbol.clone(),
        skipped_other_symbol,
        rows,
    };
    series.validate()?;
    Ok(series)
}

/// Convert a normalized trades series into NautilusTrader `TradeTick`s at the
/// instrument's price/size precision.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision, or a trade id exceeds the NautilusTrader limit.
pub fn trades_to_trade_ticks(
    series: &DeribitTradesSeries,
    instrument: &CryptoOption,
) -> Result<Vec<TradeTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    series
        .rows
        .iter()
        .map(|row| {
            let price = Price::new_checked(row.price, price_precision).map_err(|error| {
                anyhow::anyhow!(
                    "price {} not representable at precision {price_precision}: {error}",
                    row.price
                )
            })?;
            let size = Quantity::new_checked(row.size, size_precision).map_err(|error| {
                anyhow::anyhow!(
                    "size {} not representable at precision {size_precision}: {error}",
                    row.size
                )
            })?;
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

/// Project a normalized trades series into a NautilusTrader `ParquetDataCatalog`
/// as `TradeTick` data plus the venue `CryptoOption` instrument.
///
/// Fails closed on a dirty (non-empty) catalog root.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_trades_to_catalog(
    series: &DeribitTradesSeries,
    spec: &DeribitTradesInstrumentSpec,
    catalog_root: &Path,
) -> Result<DeribitCatalogProjection> {
    series.validate()?;
    ensure!(
        series.raw_symbol == spec.raw_symbol,
        "series symbol {:?} does not match spec {:?}",
        series.raw_symbol,
        spec.raw_symbol
    );
    let instrument = spec.build_instrument()?;
    let instrument_id = instrument.id();
    let ticks = trades_to_trade_ticks(series, &instrument)?;
    let count = ticks.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CryptoOption(instrument)])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(DeribitCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        quote_count: count,
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

// ===========================================================================
// Bulk-conversion path (data-derived precision, identity parsed from the
// Deribit symbol, no clean-root guard) — mirrors `canonical_okx`'s
// `append_okx_trades_archive`/`OkxAppendSummary`.
// ===========================================================================

/// NautilusTrader venue code appended to a Deribit venue-native instrument id to
/// form the catalog `nt_instrument_id` (for example
/// `BTC_USDC-29MAY26-66000-C.DERIBIT`). A per-venue format constant — the only
/// literal the bulk path injects, never a runtime value.
pub const DERIBIT_VENUE: &str = "DERIBIT";

/// Deribit renders a fractional strike with `d`/`D` standing in for the decimal
/// point (`1d45` -> `1.45`, `8D6` -> `8.6`); whole strikes carry no separator
/// (`66000`). A property of the Deribit instrument-name grammar, not config.
const DERIBIT_STRIKE_DECIMAL_SEPARATORS: [char; 2] = ['d', 'D'];

/// The `bars_1m` staging family is, by its name, a 1-minute candle partition:
/// step 1, `MINUTE` aggregation. These are facts of the source partition (the
/// `family=bars_1m` segment of the S3 key), not runtime config — exactly like
/// the column-name and millis->nanos constants above.
pub const DERIBIT_BARS_1M_STEP: usize = 1;
pub const DERIBIT_BARS_1M_AGGREGATION: BarAggregation = BarAggregation::Minute;

/// The decimal-string increment whose fractional length is exactly `precision`
/// (`0 -> "1"`, `1 -> "0.1"`, `4 -> "0.0001"`) — the inverse of
/// [`decimal_places`]. Lets a data-derived precision be expressed as the
/// increment string the instrument builder consumes. Mirrors
/// `canonical_okx::increment_for`.
#[must_use]
fn increment_for(precision: u8) -> String {
    match precision {
        0 => "1".to_string(),
        n => format!("0.{}1", "0".repeat(usize::from(n) - 1)),
    }
}

/// Decimal places actually rendered by a source `f64` value, via the exact
/// shortest decimal that round-trips the float (`660.0 -> 0`, `1.45 -> 2`,
/// `0.0001 -> 4`). This is the per-value scale; the spec builders take the
/// maximum across an instrument's rows, which is the precision NautilusTrader
/// pins per catalog file. Data-derived — never a hardcoded literal, never read
/// from an instrument universe (Deribit stages none for these families).
///
/// # Errors
///
/// Returns an error if the value is not finite (cannot have a decimal scale).
fn observed_decimal_places(value: f64) -> Result<u8> {
    use rust_decimal::prelude::FromPrimitive;

    ensure!(
        value.is_finite(),
        "non-finite value {value} has no decimal scale"
    );
    // `from_f64` (the `FromPrimitive` impl) removes excess IEEE-754 bits so the
    // result is the value's guaranteed shortest decimal (`0.1_f64 -> 0.1`, not
    // `0.10000000000000000555…`); `normalize` then strips trailing zeros so the
    // scale is exactly the decimal places the exchange rendered. Using the
    // bit-retaining `from_f64_retain` here would inflate precision to ~28 and is
    // deliberately avoided.
    let decimal = Decimal::from_f64(value)
        .with_context(|| format!("value {value} is not representable as a decimal"))?
        .normalize();
    Ok(u8::try_from(decimal.scale()).unwrap_or(u8::MAX))
}

/// Identity of a Deribit option, derived entirely from its venue-native
/// instrument name (the symbol carried in every trade row and in the bars S3
/// object key). Nothing here is invented: every field is read out of the symbol
/// the exchange itself assigned.
///
/// Linear (USDC-quoted) options name themselves `<BASE>_<QUOTE>-<EXPIRY>-
/// <STRIKE>-<C|P>` (for example `XRP_USDC-29MAY26-1d45-P`); classic
/// coin-margined options drop the `_<QUOTE>` and settle in the coin
/// (`BTC-29MAY26-66000-C`). Both forms are honoured.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeribitOptionIdentity {
    underlying: String,
    quote_currency: String,
    settlement_currency: String,
    is_inverse: bool,
    option_kind: String,
    strike_price: String,
    expiration_ns: u64,
}

/// Parse a Deribit option [`DeribitOptionIdentity`] out of its venue-native
/// instrument name. Fail loud on anything that is not a 4-part option symbol —
/// the bulk path must never fabricate identity for a foreign instrument.
fn parse_deribit_option_symbol(raw_symbol: &str) -> Result<DeribitOptionIdentity> {
    let symbol = raw_symbol.trim();
    ensure!(!symbol.is_empty(), "empty deribit symbol");
    let parts: Vec<&str> = symbol.split('-').collect();
    ensure!(
        parts.len() == 4,
        "deribit option symbol {symbol:?} is not <UNDERLYING>-<EXPIRY>-<STRIKE>-<KIND>"
    );
    let underlying_part = parts[0];
    let expiry = parts[1];
    let strike_raw = parts[2];
    let kind_raw = parts[3];

    // `<BASE>_<QUOTE>` (linear, quote-settled) or bare `<BASE>` (coin-margined,
    // inverse, coin-settled). The settlement currency is the quote for linear
    // options and the underlying coin for inverse ones — a property of the
    // Deribit naming convention, read straight off the symbol.
    let (underlying, quote_currency, settlement_currency, is_inverse) =
        match underlying_part.split_once('_') {
            Some((base, quote)) => {
                ensure!(
                    !base.is_empty() && !quote.is_empty(),
                    "deribit symbol {symbol:?} has an empty base or quote currency"
                );
                (
                    base.to_string(),
                    quote.to_string(),
                    quote.to_string(),
                    false,
                )
            }
            None => {
                ensure!(
                    !underlying_part.is_empty(),
                    "deribit symbol {symbol:?} has an empty underlying"
                );
                (
                    underlying_part.to_string(),
                    underlying_part.to_string(),
                    underlying_part.to_string(),
                    true,
                )
            }
        };

    let option_kind = match kind_raw.trim().to_ascii_uppercase().as_str() {
        "C" | "CALL" => "CALL".to_string(),
        "P" | "PUT" => "PUT".to_string(),
        other => bail!("deribit symbol {symbol:?} has non-option kind token {other:?}"),
    };

    // `d`/`D` is Deribit's in-symbol decimal point for fractional strikes.
    let strike_price = strike_raw.replacen(DERIBIT_STRIKE_DECIMAL_SEPARATORS, ".", 1);
    Decimal::from_str(&strike_price).with_context(|| {
        format!("deribit symbol {symbol:?} has non-decimal strike {strike_raw:?}")
    })?;

    let expiration_ns = parse_deribit_expiry_ns(expiry)
        .with_context(|| format!("deribit symbol {symbol:?} has unparseable expiry {expiry:?}"))?;

    Ok(DeribitOptionIdentity {
        underlying,
        quote_currency,
        settlement_currency,
        is_inverse,
        option_kind,
        strike_price,
        expiration_ns,
    })
}

/// Deribit expiry tokens are `<DAY><MON><YY>` in UTC (for example `29MAY26`).
/// The option expires at 08:00 UTC, the Deribit settlement hour — a fixed
/// property of every Deribit option, not runtime config. Returns the expiry as
/// Unix nanoseconds. The expiry never flows into the written `TradeTick`/`Bar`
/// payload (only the instrument id and precision do); it exists so the
/// transient `CryptoOption` scaffold carries an honest, symbol-derived
/// expiration rather than a fabricated one.
fn parse_deribit_expiry_ns(token: &str) -> Result<u64> {
    let token = token.trim().to_ascii_uppercase();
    ensure!(token.len() >= 6, "expiry token {token:?} too short");
    let (day_str, rest) = token.split_at(token.len() - 5);
    let (mon_str, yy_str) = rest.split_at(3);
    let day: u32 = day_str
        .parse()
        .with_context(|| format!("bad day in {token:?}"))?;
    let year: i64 = yy_str
        .parse::<i64>()
        .map(|yy| 2000 + yy)
        .with_context(|| format!("bad year in {token:?}"))?;
    let month = match mon_str {
        "JAN" => 1,
        "FEB" => 2,
        "MAR" => 3,
        "APR" => 4,
        "MAY" => 5,
        "JUN" => 6,
        "JUL" => 7,
        "AUG" => 8,
        "SEP" => 9,
        "OCT" => 10,
        "NOV" => 11,
        "DEC" => 12,
        other => bail!("unknown month {other:?} in expiry {token:?}"),
    };
    // Days since the Unix epoch for a UTC calendar date, via a civil-date
    // algorithm (Howard Hinnant's `days_from_civil`). Deribit settles at 08:00
    // UTC on the expiry date.
    let days = days_from_civil(year, month, day);
    let secs = days
        .checked_mul(86_400)
        .and_then(|d| d.checked_add(8 * 3_600))
        .context("expiry seconds overflow")?;
    ensure!(secs > 0, "expiry {token:?} resolves before the Unix epoch");
    u64::try_from(secs)
        .ok()
        .and_then(|s| s.checked_mul(1_000_000_000))
        .context("expiry nanoseconds overflow")
}

/// Days from the Unix epoch (1970-01-01) for a proleptic-Gregorian UTC date.
/// Howard Hinnant's `days_from_civil`, valid for all years in range.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Build a [`DeribitTradesInstrumentSpec`] whose price/size precision is derived
/// from the rows themselves (the maximum decimal places the exchange rendered)
/// and whose instrument identity is parsed from the Deribit symbol. Mirrors
/// `canonical_okx::okx_trades_spec_from_rows`.
///
/// # Errors
///
/// Returns an error if `rows` is empty, a price/size is non-finite, or the
/// symbol is not a parseable Deribit option name.
pub fn deribit_trades_spec_from_rows(
    rows: &[DeribitTradeRow],
    raw_symbol: &str,
) -> Result<DeribitTradesInstrumentSpec> {
    ensure!(
        !rows.is_empty(),
        "cannot derive Deribit trades precision from zero rows"
    );
    let identity = parse_deribit_option_symbol(raw_symbol)?;
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in rows {
        price_precision = price_precision.max(observed_decimal_places(row.price)?);
        size_precision = size_precision.max(observed_decimal_places(row.size)?);
    }
    Ok(DeribitTradesInstrumentSpec {
        nt_instrument_id: format!("{raw_symbol}.{DERIBIT_VENUE}"),
        raw_symbol: raw_symbol.to_string(),
        underlying: identity.underlying,
        quote_currency: identity.quote_currency,
        settlement_currency: identity.settlement_currency,
        is_inverse: identity.is_inverse,
        option_kind: identity.option_kind,
        strike_price: identity.strike_price,
        // Activation is not encoded in the Deribit symbol; the epoch is the
        // honest "unknown" floor and never flows into the written tick payload.
        activation_ns: 0,
        expiration_ns: identity.expiration_ns,
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
    })
}

/// One instrument's write summary produced by the bulk-append fns. Mirrors
/// `canonical_okx::OkxAppendSummary`: the written NautilusTrader `TradeTick`/
/// `Bar` records carry no provenance field, so neither does the summary — the
/// catalog's own per-instrument/per-time-range file layout is the record of
/// what was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeribitAppendSummary {
    pub nt_instrument_id: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Distinct venue-native instrument names appearing in a RiveChen merged-trades
/// Parquet object, in first-seen order. The source object interleaves many
/// instruments, so the bulk converter writes one catalog stream per distinct
/// instrument rather than assuming a single one. Mirrors
/// `canonical_okx::okx_trade_instruments`.
///
/// # Errors
///
/// Returns an error if the file cannot be read, the required `instrument_name`
/// column is missing or of the wrong Arrow type, or a value is null.
pub fn deribit_trade_instruments(path: &Path) -> Result<Vec<String>> {
    let file = File::open(path).with_context(|| format!("open parquet {}", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| format!("open parquet reader {}", path.display()))?;
    ensure!(
        builder
            .schema()
            .column_with_name("instrument_name")
            .is_some(),
        "staged object {} is missing required column \"instrument_name\"",
        path.display()
    );
    let reader = builder
        .build()
        .with_context(|| format!("build parquet reader {}", path.display()))?;
    let mut seen: Vec<String> = Vec::new();
    for batch in reader {
        let batch = batch.context("read parquet record batch")?;
        let instrument_name = typed_column::<StringArray>(&batch, "instrument_name")?;
        for index in 0..batch.num_rows() {
            ensure!(
                !instrument_name.is_null(index),
                "null instrument_name in row {index} of {}",
                path.display()
            );
            let inst = instrument_name.value(index);
            if !inst.is_empty() && !seen.iter().any(|s| s == inst) {
                seen.push(inst.to_string());
            }
        }
    }
    Ok(seen)
}

/// Materialize an in-memory object to a temporary file so a `&Path`-taking
/// parser can read it, run `f`, then remove the file. The RiveChen merged-trades
/// parser takes a `&Path`; the bulk dispatch hands this fn the object bytes.
///
/// Uses a content-addressed name under the system temp dir (no external temp
/// crate is a runtime dependency of this crate) and removes it on every exit
/// path, failing loud if the temp write fails.
fn with_object_tempfile<T>(object_bytes: &[u8], f: impl FnOnce(&Path) -> Result<T>) -> Result<T> {
    use std::io::Write;

    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(object_bytes);
    let digest = hex::encode(hasher.finalize());
    let path = std::env::temp_dir().join(format!("bolt-deribit-bulk-{digest}.bin"));
    {
        let mut file = File::create(&path)
            .with_context(|| format!("create temp object {}", path.display()))?;
        file.write_all(object_bytes)
            .with_context(|| format!("write temp object {}", path.display()))?;
        file.flush()
            .with_context(|| format!("flush temp object {}", path.display()))?;
    }
    let result = f(&path);
    // Best-effort cleanup; the result (Ok or Err) is what the caller cares about.
    let _ = fs::remove_file(&path);
    result
}

/// Append every instrument's trades from one RiveChen merged-trades Parquet
/// object into an already-open [`ParquetDataCatalog`] — the bulk-conversion
/// path. Mirrors `canonical_okx::append_okx_trades_archive`.
///
/// Unlike [`project_trades_to_catalog`] (the hermetic single-object proof
/// harness, which refuses a dirty root), this appends into a shared,
/// possibly-S3 catalog with NO clean-root guard, relying on NautilusTrader's own
/// per-instrument/per-time-range file naming and skip-on-existing. Precision is
/// derived from each instrument's own rows; identity is parsed from each
/// instrument's symbol. The trade `instrument_name` lives in the data, so no
/// object key is needed. Returns one summary per distinct instrument written.
///
/// # Errors
///
/// Returns an error if the temp materialization, parsing, tick construction, or
/// the catalog write fails, or if the object yields no instruments.
pub fn append_deribit_trades_archive(
    object_bytes: &[u8],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<DeribitAppendSummary>> {
    with_object_tempfile(object_bytes, |path| {
        let instruments = deribit_trade_instruments(path)?;
        let mut summaries = Vec::new();
        for raw_symbol in instruments {
            let spec_id = DeribitTradesInstrumentSpec {
                nt_instrument_id: String::new(),
                raw_symbol: raw_symbol.clone(),
                underlying: String::new(),
                quote_currency: String::new(),
                settlement_currency: String::new(),
                is_inverse: false,
                option_kind: "CALL".to_string(),
                strike_price: "1".to_string(),
                activation_ns: 0,
                expiration_ns: 0,
                price_increment: "1".to_string(),
                size_increment: "1".to_string(),
            };
            // Re-parse this instrument's rows to derive precision + identity.
            // `normalize_deribit_merged_trades` only uses `raw_symbol` to fence,
            // so the placeholder fields above never reach the data.
            let series = normalize_deribit_merged_trades(path, &spec_id)?;
            if series.rows.is_empty() {
                continue;
            }
            let spec = deribit_trades_spec_from_rows(&series.rows, &raw_symbol)?;
            let instrument = spec.build_instrument()?;
            let ticks = trades_to_trade_ticks(&series, &instrument)?;
            let summary = DeribitAppendSummary {
                nt_instrument_id: spec.nt_instrument_id.clone(),
                record_count: ticks.len(),
                price_precision: instrument.price_precision(),
                size_precision: instrument.size_precision(),
            };
            catalog
                .write_to_parquet(ticks, None, None, None)
                .with_context(|| format!("append Deribit trade ticks for {raw_symbol}"))?;
            summaries.push(summary);
        }
        ensure!(
            !summaries.is_empty(),
            "Deribit merged-trades object yielded no instruments"
        );
        Ok(summaries)
    })
}

// ===========================================================================
// Family 3: 1m OHLC bars (Deribit TradingView chart JSON) -> NautilusTrader `Bar`
// ===========================================================================

/// Caller-supplied metadata + bar specification for the Deribit instrument whose
/// 1-minute candles are projected. Precision is derived from the increment
/// strings; nothing here is hardcoded in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeribitBarsInstrumentSpec {
    /// NautilusTrader instrument id, for example `<symbol>.DERIBIT`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol.
    pub raw_symbol: String,
    /// Underlying currency code.
    pub underlying: String,
    /// Quote currency code.
    pub quote_currency: String,
    /// Settlement currency code.
    pub settlement_currency: String,
    /// Whether the option is inverse-settled.
    pub is_inverse: bool,
    /// Option kind: must parse as `CALL` or `PUT`.
    pub option_kind: String,
    /// Strike price as a decimal string.
    pub strike_price: String,
    /// Activation (listing) time in Unix nanoseconds.
    pub activation_ns: u64,
    /// Expiration time in Unix nanoseconds.
    pub expiration_ns: u64,
    /// Price tick size as a decimal string.
    pub price_increment: String,
    /// Size (volume) increment as a decimal string.
    pub size_increment: String,
    /// Bar step (the `1` of a 1m bar). Combined with `bar_aggregation`.
    pub bar_step: usize,
    /// Bar aggregation unit (for example `MINUTE`), provider-aggregated.
    pub bar_aggregation: BarAggregation,
}

impl DeribitBarsInstrumentSpec {
    /// Build the NautilusTrader `CryptoOption` for the bars family.
    ///
    /// # Errors
    ///
    /// Returns an error if any field fails to parse.
    pub fn build_instrument(&self) -> Result<CryptoOption> {
        build_crypto_option(&DeribitOptionInstrumentSpec {
            nt_instrument_id: self.nt_instrument_id.clone(),
            raw_symbol: self.raw_symbol.clone(),
            underlying: self.underlying.clone(),
            quote_currency: self.quote_currency.clone(),
            settlement_currency: self.settlement_currency.clone(),
            is_inverse: self.is_inverse,
            option_kind: self.option_kind.clone(),
            strike_price: self.strike_price.clone(),
            activation_ns: self.activation_ns,
            expiration_ns: self.expiration_ns,
            price_increment: self.price_increment.clone(),
            size_increment: self.size_increment.clone(),
        })
    }

    /// Build the NautilusTrader `BarType`: instrument + `BarSpecification(step,
    /// unit, Last)` + `AggregationSource::External` (the candles are aggregated
    /// by the exchange, outside the NautilusTrader boundary).
    fn to_bar_type(&self, instrument_id: InstrumentId) -> Result<BarType> {
        ensure!(self.bar_step > 0, "bar step must be positive");
        let spec = BarSpecification::new(self.bar_step, self.bar_aggregation, PriceType::Last);
        Ok(BarType::new(
            instrument_id,
            spec,
            AggregationSource::External,
        ))
    }
}

/// One normalized 1m OHLC bar row: open time in nanoseconds and the exact source
/// OHLCV doubles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeribitBarRow {
    /// Bar open (tick) timestamp in Unix nanoseconds.
    pub open_time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// A validated canonical 1m OHLC series for one accepted Deribit instrument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeribitBarsSeries {
    /// Venue-native raw symbol the series belongs to.
    pub raw_symbol: String,
    /// Source `status` token (for example `ok`).
    pub status: String,
    /// Normalized bar rows, strictly increasing in open time.
    pub rows: Vec<DeribitBarRow>,
}

impl DeribitBarsSeries {
    /// Validate non-emptiness, positive strictly-increasing open times, and the
    /// per-bar OHLC invariant.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.raw_symbol.trim().is_empty(),
            "bars series has empty raw_symbol"
        );
        ensure!(!self.rows.is_empty(), "deribit bars series is empty");
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.open_time > 0, "bar {index}: non-positive open_time");
            ensure!(
                row.open_time > previous,
                "bar {index}: open_time {} not after previous {}",
                row.open_time,
                previous
            );
            previous = row.open_time;
            ensure!(
                row.open > 0.0 && row.low > 0.0,
                "bar {index}: non-positive open/low"
            );
            ensure!(row.volume >= 0.0, "bar {index}: negative volume");
            ensure!(
                row.high >= row.open && row.high >= row.low && row.high >= row.close,
                "bar {index}: high {} is not the maximum (o={} l={} c={})",
                row.high,
                row.open,
                row.low,
                row.close
            );
            ensure!(
                row.low <= row.open && row.low <= row.close,
                "bar {index}: low {} is not the minimum (o={} c={})",
                row.low,
                row.open,
                row.close
            );
        }
        Ok(())
    }
}

/// Deribit `get_tradingview_chart_data` JSON-RPC envelope. The candle data lives
/// in `result` as parallel arrays. Only the fields this projection consumes are
/// modeled; unknown envelope fields are ignored.
#[derive(Debug, Deserialize)]
struct DeribitChartEnvelope {
    result: DeribitChartResult,
}

#[derive(Debug, Deserialize)]
struct DeribitChartResult {
    status: String,
    /// Bar open times in Unix milliseconds.
    ticks: Vec<i64>,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
}

/// Normalize an accepted Deribit 1m-bars TradingView-chart JSON object into the
/// canonical OHLC series.
///
/// `json_text` is the raw object bytes as UTF-8. The candle arrays are parallel
/// (`ticks`/`open`/`high`/`low`/`close`/`volume`); `ticks` carries bar open
/// times in milliseconds, converted here to nanoseconds. A `status` other than
/// `ok` (for example `no_data`) yields an empty series, which validation
/// rejects loudly — the caller chooses which instruments to project.
///
/// # Errors
///
/// Returns an error if the JSON cannot be parsed, the parallel arrays have
/// mismatched lengths, a timestamp overflows, or the series fails validation.
pub fn normalize_deribit_bars(
    json_text: &str,
    spec: &DeribitBarsInstrumentSpec,
) -> Result<DeribitBarsSeries> {
    ensure!(
        !spec.raw_symbol.trim().is_empty(),
        "spec.raw_symbol must not be empty"
    );
    let envelope: DeribitChartEnvelope =
        serde_json::from_str(json_text).context("parse tradingview chart JSON")?;
    let result = envelope.result;

    let n = result.ticks.len();
    for (name, len) in [
        ("open", result.open.len()),
        ("high", result.high.len()),
        ("low", result.low.len()),
        ("close", result.close.len()),
        ("volume", result.volume.len()),
    ] {
        ensure!(
            len == n,
            "chart array {name:?} has {len} entries, expected {n} (matching ticks)"
        );
    }

    let mut rows = Vec::with_capacity(n);
    for index in 0..n {
        let millis = result.ticks[index];
        let open_time = millis
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("bar {index}: tick {millis} overflows nanoseconds"))?;
        rows.push(DeribitBarRow {
            open_time,
            open: result.open[index],
            high: result.high[index],
            low: result.low[index],
            close: result.close[index],
            volume: result.volume[index],
        });
    }

    let series = DeribitBarsSeries {
        raw_symbol: spec.raw_symbol.clone(),
        status: result.status,
        rows,
    };
    series.validate()?;
    Ok(series)
}

/// Convert a normalized 1m OHLC series into NautilusTrader `Bar`s at the
/// instrument's price/size precision under the provider-aggregated bar type.
///
/// `ts_event`/`ts_init` are set to the bar OPEN time (the source `ticks`
/// value); this matches the `External` aggregation convention where the bar is
/// keyed by its opening minute.
///
/// # Errors
///
/// Returns an error if an OHLCV value cannot be represented at the instrument
/// precision, or NautilusTrader's bar OHLC invariant is violated.
pub fn bars_to_bars(
    series: &DeribitBarsSeries,
    spec: &DeribitBarsInstrumentSpec,
    instrument: &CryptoOption,
) -> Result<Vec<Bar>> {
    let instrument_id = instrument.id();
    let bar_type = spec.to_bar_type(instrument_id)?;
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    series
        .rows
        .iter()
        .map(|row| {
            let price_at = |value: f64, label: &str| -> Result<Price> {
                Price::new_checked(value, price_precision).map_err(|error| {
                    anyhow::anyhow!(
                        "{label} {value} not representable at precision {price_precision}: {error}"
                    )
                })
            };
            let open = price_at(row.open, "open")?;
            let high = price_at(row.high, "high")?;
            let low = price_at(row.low, "low")?;
            let close = price_at(row.close, "close")?;
            let volume = Quantity::new_checked(row.volume, size_precision).map_err(|error| {
                anyhow::anyhow!(
                    "volume {} not representable at precision {size_precision}: {error}",
                    row.volume
                )
            })?;
            let ts = UnixNanos::from(u64::try_from(row.open_time).context("negative open_time")?);
            Bar::new_checked(bar_type, open, high, low, close, volume, ts, ts).context("build bar")
        })
        .collect()
}

/// Project a normalized 1m OHLC series into a NautilusTrader `ParquetDataCatalog`
/// as `Bar` data plus the venue `CryptoOption` instrument.
///
/// Fails closed on a dirty (non-empty) catalog root.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail.
pub fn project_bars_to_catalog(
    series: &DeribitBarsSeries,
    spec: &DeribitBarsInstrumentSpec,
    catalog_root: &Path,
) -> Result<DeribitCatalogProjection> {
    series.validate()?;
    ensure!(
        series.raw_symbol == spec.raw_symbol,
        "series symbol {:?} does not match spec {:?}",
        series.raw_symbol,
        spec.raw_symbol
    );
    let instrument = spec.build_instrument()?;
    let instrument_id = instrument.id();
    let bars = bars_to_bars(series, spec, &instrument)?;
    let count = bars.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![InstrumentAny::CryptoOption(instrument)])
        .context("write instrument to catalog")?;
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write bars to catalog")?;

    Ok(DeribitCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_BAR.to_string(),
        quote_count: count,
    })
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
// bars bulk-append path (data-derived precision, identity from the object key)
// ===========================================================================

/// S3-key attribute segment carrying the instrument name in the Deribit
/// `bars_1m` staging layout: `.../family=bars_1m/instrument=<SYMBOL>/...`. The
/// `bars_1m` JSON payload itself carries no `instrument_name` (only parallel
/// OHLCV arrays), so the bulk path reads identity from the staged key. A
/// property of the staging layout (`backfill_deribit_to_s3.py`), not config.
const DERIBIT_KEY_INSTRUMENT_ATTR: &str = "instrument=";

/// Extract the venue-native instrument name from a Deribit `bars_1m` S3 object
/// key by reading its `instrument=<SYMBOL>` path segment. Deribit symbols use
/// only `[A-Za-z0-9._=-]`, which the staging key-sanitizer preserves verbatim,
/// so the symbol survives the round-trip into the key.
///
/// # Errors
///
/// Returns an error if the key has no `instrument=` segment.
fn raw_symbol_from_bars_key(object_key: &str) -> Result<String> {
    object_key
        .split('/')
        .find_map(|segment| segment.strip_prefix(DERIBIT_KEY_INSTRUMENT_ATTR))
        .filter(|symbol| !symbol.is_empty())
        .map(str::to_string)
        .with_context(|| {
            format!(
                "object key {object_key:?} has no non-empty {DERIBIT_KEY_INSTRUMENT_ATTR:?} segment"
            )
        })
}

/// Build a [`DeribitBarsInstrumentSpec`] whose price/size precision is derived
/// from the OHLCV rows themselves and whose instrument identity is parsed from
/// the Deribit symbol. The bar step/unit is the `bars_1m` partition contract
/// ([`DERIBIT_BARS_1M_STEP`]/[`DERIBIT_BARS_1M_AGGREGATION`]). Mirrors the
/// trades spec builder; the OKX analogue is `okx_trades_spec_from_rows` plus the
/// caller-supplied `OkxBarSpec`.
///
/// # Errors
///
/// Returns an error if `rows` is empty, an OHLCV value is non-finite, or the
/// symbol is not a parseable Deribit option name.
pub fn deribit_bars_spec_from_rows(
    rows: &[DeribitBarRow],
    raw_symbol: &str,
) -> Result<DeribitBarsInstrumentSpec> {
    ensure!(
        !rows.is_empty(),
        "cannot derive Deribit bars precision from zero rows"
    );
    let identity = parse_deribit_option_symbol(raw_symbol)?;
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in rows {
        for value in [row.open, row.high, row.low, row.close] {
            price_precision = price_precision.max(observed_decimal_places(value)?);
        }
        size_precision = size_precision.max(observed_decimal_places(row.volume)?);
    }
    Ok(DeribitBarsInstrumentSpec {
        nt_instrument_id: format!("{raw_symbol}.{DERIBIT_VENUE}"),
        raw_symbol: raw_symbol.to_string(),
        underlying: identity.underlying,
        quote_currency: identity.quote_currency,
        settlement_currency: identity.settlement_currency,
        is_inverse: identity.is_inverse,
        option_kind: identity.option_kind,
        strike_price: identity.strike_price,
        activation_ns: 0,
        expiration_ns: identity.expiration_ns,
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
        bar_step: DERIBIT_BARS_1M_STEP,
        bar_aggregation: DERIBIT_BARS_1M_AGGREGATION,
    })
}

/// Append one Deribit `bars_1m` TradingView-chart JSON object into an
/// already-open [`ParquetDataCatalog`] — the bulk-conversion path. Mirrors
/// `canonical_okx::append_okx_trades_archive`.
///
/// Unlike [`project_bars_to_catalog`] (the hermetic single-object proof harness,
/// which refuses a dirty root), this appends with NO clean-root guard. The
/// `bars_1m` payload carries one instrument's candles and no `instrument_name`,
/// so identity is read from `object_key`'s `instrument=` segment; precision is
/// derived from the OHLCV rows. Returns one summary (one instrument per object).
///
/// # Errors
///
/// Returns an error if the key has no instrument segment, the JSON cannot be
/// parsed/normalized, bar construction fails, or the catalog write fails.
pub fn append_deribit_bars_archive(
    object_bytes: &[u8],
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<DeribitAppendSummary> {
    let raw_symbol = raw_symbol_from_bars_key(object_key)?;
    let json_text = std::str::from_utf8(object_bytes)
        .with_context(|| format!("bars object {object_key:?} is not valid UTF-8"))?;

    // Derive the full spec by first normalizing the candles with a minimal
    // raw-symbol-only spec (normalization reads only `raw_symbol`), then
    // deriving precision + identity from the resulting rows.
    let probe_spec = DeribitBarsInstrumentSpec {
        nt_instrument_id: String::new(),
        raw_symbol: raw_symbol.clone(),
        underlying: String::new(),
        quote_currency: String::new(),
        settlement_currency: String::new(),
        is_inverse: false,
        option_kind: "CALL".to_string(),
        strike_price: "1".to_string(),
        activation_ns: 0,
        expiration_ns: 0,
        price_increment: "1".to_string(),
        size_increment: "1".to_string(),
        bar_step: DERIBIT_BARS_1M_STEP,
        bar_aggregation: DERIBIT_BARS_1M_AGGREGATION,
    };
    let series = normalize_deribit_bars(json_text, &probe_spec)?;
    let spec = deribit_bars_spec_from_rows(&series.rows, &raw_symbol)?;
    let instrument = spec.build_instrument()?;
    let bars = bars_to_bars(&series, &spec, &instrument)?;
    let summary = DeribitAppendSummary {
        nt_instrument_id: spec.nt_instrument_id.clone(),
        record_count: bars.len(),
        price_precision: instrument.price_precision(),
        size_precision: instrument.size_precision(),
    };
    catalog
        .write_to_parquet(bars, None, None, None)
        .with_context(|| format!("append Deribit bars for {raw_symbol}"))?;
    Ok(summary)
}

/// Fail closed on a dirty (non-empty) catalog root, then ensure it exists.
///
/// NautilusTrader's `write_to_parquet` appends/skips by identifier, so
/// projecting into a non-empty root could silently mix or hide stale data.
fn assert_clean_catalog_root(catalog_root: &Path) -> Result<()> {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> DeribitOptionInstrumentSpec {
        DeribitOptionInstrumentSpec {
            nt_instrument_id: "AVAX_USDC-29MAY26-8D6-P.DERIBIT".to_string(),
            raw_symbol: "AVAX_USDC-29MAY26-8D6-P".to_string(),
            underlying: "AVAX".to_string(),
            quote_currency: "USDC".to_string(),
            settlement_currency: "USDC".to_string(),
            is_inverse: false,
            option_kind: "PUT".to_string(),
            strike_price: "8.6".to_string(),
            activation_ns: 1_777_593_600_000_000_000,
            expiration_ns: 1_780_041_600_000_000_000,
            price_increment: "0.001".to_string(),
            size_increment: "1".to_string(),
        }
    }

    const SAMPLE_CSV: &str = concat!(
        "exchange,symbol,timestamp,local_timestamp,type,strike_price,expiration,open_interest,",
        "last_price,bid_price,bid_amount,bid_iv,ask_price,ask_amount,ask_iv,mark_price,mark_iv,",
        "underlying_index,underlying_price,delta,gamma,vega,theta,rho\n",
        // two-sided -> quote
        "deribit,AVAX_USDC-29MAY26-8D6-P,1777593600657000,1777593601775384,put,8.6,1780041600000000,300,0.348,0.312,2000,53.49,0.318,1800,54.15,0.3144,53.72,AVAX_USDC-29MAY26,9.103,-0.32469,0.26407,0.00912,-0.00865,-0.00254\n",
        // one-sided (no bid) -> skipped
        "deribit,AVAX_USDC-29MAY26-8D6-P,1777593601000000,1777593601900000,put,8.6,1780041600000000,300,,,,,0.318,1000,54.1,0.3114,53.81,AVAX_USDC-29MAY26,9.10,0,0,0,0,0\n",
        // different symbol -> ignored
        "deribit,ETH-3MAY26-2150-P,1777593601500000,1777593601950000,put,2150,1777795200000000,18,0.0065,0.001,1245,42.1,0.0013,424,43.84,0.0012,42.65,ETH-3MAY26,2256.85,-0.075,0.00184,0.25544,-2.33428,-0.01099\n",
        // two-sided -> quote (later ts)
        "deribit,AVAX_USDC-29MAY26-8D6-P,1777593604507000,1777593604700000,put,8.6,1780041600000000,300,0.348,0.312,2000,53.49,0.318,1000,54.15,0.3144,53.72,AVAX_USDC-29MAY26,9.103,-0.32469,0.26407,0.00912,-0.00865,-0.00254\n",
    );

    #[test]
    fn decimal_places_reads_increment_precision() {
        assert_eq!(decimal_places("0.001"), 3);
        assert_eq!(decimal_places("1"), 0);
        assert_eq!(decimal_places("0.10"), 2);
    }

    #[test]
    fn normalizes_two_sided_quotes_and_skips_others() {
        let series = normalize_deribit_options_chain(SAMPLE_CSV, &spec()).expect("normalize");
        assert_eq!(series.rows.len(), 2, "only two-sided same-symbol rows");
        assert_eq!(series.skipped_one_sided, 1, "one one-sided row skipped");
        assert_eq!(series.rows[0].event_time, 1_777_593_600_657_000 * 1_000);
        assert_eq!(series.rows[0].bid_price, "0.312");
        assert_eq!(series.rows[0].ask_price, "0.318");
        assert_eq!(series.rows[0].bid_size, "2000");
        // Capture time is local_timestamp (micros -> nanos), after event time.
        assert!(series.rows[0].capture_time >= series.rows[0].event_time);
    }

    #[test]
    fn rejects_header_mismatch() {
        let bad = "exchange,symbol,ts\n";
        let err = normalize_deribit_options_chain(bad, &spec()).unwrap_err();
        assert!(err.to_string().contains("header"), "{err}");
    }

    #[test]
    fn sorts_out_of_order_event_time() {
        // Real Deribit options-chain rows are per-instrument BBO snapshots that
        // interleave in time (file order != event-time order). The converter must
        // sort them into the non-decreasing order NT's catalog write contract
        // requires, not reject them. Here the two rows arrive newest-first.
        let header = SAMPLE_CSV.lines().next().unwrap();
        let out_of_order = format!(
            "{header}\n\
            deribit,AVAX_USDC-29MAY26-8D6-P,1777593604507000,1777593604700000,put,8.6,1780041600000000,300,0.348,0.312,2000,53.49,0.318,1000,54.15,0.3144,53.72,u,9.1,0,0,0,0,0\n\
            deribit,AVAX_USDC-29MAY26-8D6-P,1777593600657000,1777593601775384,put,8.6,1780041600000000,300,0.348,0.312,2000,53.49,0.318,1800,54.15,0.3144,53.72,u,9.1,0,0,0,0,0\n"
        );
        let series = normalize_deribit_options_chain(&out_of_order, &spec())
            .expect("out-of-order rows are sorted, not rejected");
        assert_eq!(series.rows.len(), 2);
        // Sorted ascending: the earlier event_time (micros 1777593600657000) is first.
        assert_eq!(series.rows[0].event_time, 1_777_593_600_657_000_000);
        assert_eq!(series.rows[1].event_time, 1_777_593_604_507_000_000);
        for pair in series.rows.windows(2) {
            assert!(pair[1].event_time >= pair[0].event_time);
        }
    }

    #[test]
    fn empty_series_is_rejected() {
        let mut spec = spec();
        spec.raw_symbol = "NOPE-DOES-NOT-EXIST".to_string();
        let header = SAMPLE_CSV.lines().next().unwrap();
        let err = normalize_deribit_options_chain(&format!("{header}\n"), &spec).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn builds_crypto_option_from_spec() {
        let instrument = build_crypto_option(&spec()).expect("build instrument");
        assert_eq!(
            instrument.id().to_string(),
            "AVAX_USDC-29MAY26-8D6-P.DERIBIT"
        );
        assert_eq!(instrument.price_precision(), 3);
        assert_eq!(instrument.size_precision(), 0);
    }

    #[test]
    fn build_crypto_option_rejects_bad_kind() {
        let mut spec = spec();
        spec.option_kind = "STRADDLE".to_string();
        assert!(build_crypto_option(&spec).is_err());
    }

    #[test]
    fn projects_and_reads_back_quote_ticks() {
        let series = normalize_deribit_options_chain(SAMPLE_CSV, &spec()).expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_series_to_catalog(&series, &spec(), dir.path()).expect("project");
        assert_eq!(projection.quote_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_QUOTE_TICK);

        let loaded = read_back_quote_ticks(dir.path(), "AVAX_USDC-29MAY26-8D6-P.DERIBIT")
            .expect("read back");
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded[0].instrument_id.to_string(),
            "AVAX_USDC-29MAY26-8D6-P.DERIBIT"
        );
        assert_eq!(loaded[0].bid_price, Price::from("0.312"));
        assert_eq!(loaded[0].ask_price, Price::from("0.318"));
    }

    #[test]
    fn projection_refuses_dirty_catalog_root() {
        let series = normalize_deribit_options_chain(SAMPLE_CSV, &spec()).expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_series_to_catalog(&series, &spec(), dir.path())
            .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    // --- Family 2: merged trades ---

    #[test]
    fn trade_aggressor_maps_direction_token() {
        assert_eq!(
            DeribitTradeAggressorSide::parse("buy").unwrap(),
            DeribitTradeAggressorSide::Buyer
        );
        assert_eq!(
            DeribitTradeAggressorSide::parse("SELL").unwrap(),
            DeribitTradeAggressorSide::Seller
        );
        assert!(DeribitTradeAggressorSide::parse("hold").is_err());
    }

    #[test]
    fn trades_series_rejects_empty() {
        let series = DeribitTradesSeries {
            raw_symbol: "X-1-C".to_string(),
            skipped_other_symbol: 0,
            rows: vec![],
        };
        assert!(series.validate().unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn trades_series_rejects_non_monotonic_event_time() {
        let series = DeribitTradesSeries {
            raw_symbol: "X-1-C".to_string(),
            skipped_other_symbol: 0,
            rows: vec![
                DeribitTradeRow {
                    event_time: 2_000,
                    trade_id: "a".to_string(),
                    aggressor_side: DeribitTradeAggressorSide::Buyer,
                    price: 1.0,
                    size: 1.0,
                },
                DeribitTradeRow {
                    event_time: 1_000,
                    trade_id: "b".to_string(),
                    aggressor_side: DeribitTradeAggressorSide::Seller,
                    price: 1.0,
                    size: 1.0,
                },
            ],
        };
        assert!(
            series
                .validate()
                .unwrap_err()
                .to_string()
                .contains("precedes previous")
        );
    }

    // --- Family 3: 1m OHLC bars ---

    const SAMPLE_BARS_JSON: &str = concat!(
        "{\"usOut\":1,\"usIn\":0,\"result\":{\"status\":\"ok\",",
        "\"ticks\":[1772323200000,1772323260000],",
        "\"open\":[10.0,11.0],\"high\":[12.0,11.5],\"low\":[9.5,10.5],",
        "\"close\":[11.0,10.5],\"volume\":[3.0,1.0],\"cost\":[30.0,10.0]},",
        "\"jsonrpc\":\"2.0\"}"
    );

    fn bars_spec() -> DeribitBarsInstrumentSpec {
        DeribitBarsInstrumentSpec {
            nt_instrument_id: "X_USDC-1JAN27-10-C.DERIBIT".to_string(),
            raw_symbol: "X_USDC-1JAN27-10-C".to_string(),
            underlying: "BTC".to_string(),
            quote_currency: "USDC".to_string(),
            settlement_currency: "USDC".to_string(),
            is_inverse: false,
            option_kind: "CALL".to_string(),
            strike_price: "10".to_string(),
            activation_ns: 1_777_593_600_000_000_000,
            expiration_ns: 1_780_041_600_000_000_000,
            price_increment: "0.1".to_string(),
            size_increment: "0.00000001".to_string(),
            bar_step: 1,
            bar_aggregation: BarAggregation::Minute,
        }
    }

    #[test]
    fn normalizes_bars_with_millisecond_ticks() {
        let series = normalize_deribit_bars(SAMPLE_BARS_JSON, &bars_spec()).expect("normalize");
        assert_eq!(series.status, "ok");
        assert_eq!(series.rows.len(), 2);
        // milliseconds -> nanoseconds.
        assert_eq!(series.rows[0].open_time, 1_772_323_200_000 * 1_000_000);
        assert_eq!(series.rows[0].open, 10.0);
        assert_eq!(series.rows[1].close, 10.5);
        // strictly increasing open times.
        assert!(series.rows[1].open_time > series.rows[0].open_time);
    }

    #[test]
    fn bars_reject_ohlc_violation() {
        // high (9.0) below open (10.0): invalid candle.
        let bad = concat!(
            "{\"result\":{\"status\":\"ok\",\"ticks\":[1772323200000],",
            "\"open\":[10.0],\"high\":[9.0],\"low\":[8.0],\"close\":[9.5],",
            "\"volume\":[1.0]}}"
        );
        let err = normalize_deribit_bars(bad, &bars_spec()).unwrap_err();
        assert!(err.to_string().contains("high"), "{err}");
    }

    #[test]
    fn bars_reject_mismatched_array_lengths() {
        let bad = concat!(
            "{\"result\":{\"status\":\"ok\",\"ticks\":[1,2],",
            "\"open\":[10.0],\"high\":[12.0],\"low\":[9.0],\"close\":[11.0],",
            "\"volume\":[1.0]}}"
        );
        let err = normalize_deribit_bars(bad, &bars_spec()).unwrap_err();
        assert!(err.to_string().contains("expected"), "{err}");
    }

    #[test]
    fn bars_reject_no_data_status_as_empty() {
        let no_data = concat!(
            "{\"result\":{\"status\":\"no_data\",\"ticks\":[],",
            "\"open\":[],\"high\":[],\"low\":[],\"close\":[],\"volume\":[]}}"
        );
        let err = normalize_deribit_bars(no_data, &bars_spec()).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn projects_and_reads_back_bars() {
        let series = normalize_deribit_bars(SAMPLE_BARS_JSON, &bars_spec()).expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_bars_to_catalog(&series, &bars_spec(), dir.path()).expect("project");
        assert_eq!(projection.quote_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);

        let loaded = read_back_bars(dir.path(), &bars_spec().nt_instrument_id).expect("read back");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].open, Price::from("10.0"));
        assert_eq!(loaded[0].close, Price::from("11.0"));
    }
}
