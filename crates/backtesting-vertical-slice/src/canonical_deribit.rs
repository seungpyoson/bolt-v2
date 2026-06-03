//! Deribit (Tardis options-chain) — canonical top-of-book normalization and
//! NautilusTrader catalog projection.
//!
//! Deribit's options-chain archive is a TOP-OF-BOOK time series: every CSV row
//! is one option instrument's best bid / best ask snapshot at an exchange
//! timestamp. This module is the smallest verified path:
//!
//! ```text
//! accepted gzip-CSV options-chain object (one instrument's BBO series)
//!   -> canonical normalized top-of-book rows (skip one-sided/empty quotes)
//!   -> NautilusTrader `QuoteTick`s + a `CryptoOption` instrument
//!   -> NautilusTrader `ParquetDataCatalog::write_to_parquet`
//!   -> `query_typed_data::<QuoteTick>` read-back (count + ordering proven)
//! ```
//!
//! Bolt owns parsing and normalization; NautilusTrader owns the catalog and the
//! `QuoteTick` Arrow schema. No raw arrow/parquet is hand-rolled for the NT type.
//!
//! Everything that varies per instrument (id, precision, strike, expiry,
//! currencies, option kind) is supplied by the caller via
//! [`DeribitOptionInstrumentSpec`]; the only literals in this module are the
//! fixed source-schema column names and the micros->nanos scale, which are
//! properties of the Tardis options-chain format itself, not runtime config.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::QuoteTick,
    enums::OptionKind,
    identifiers::{InstrumentId, Symbol},
    instruments::{CryptoOption, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_QUOTE_TICK: &str = "QuoteTick";

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
}
