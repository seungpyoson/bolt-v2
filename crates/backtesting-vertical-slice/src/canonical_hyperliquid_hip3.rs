//! Hyperliquid HIP-3 — canonical OHLCV `bars` table -> NautilusTrader `Bar`.
//!
//! Hyperliquid HIP-3 perpetuals expose **bars and funding only** — there is no
//! order book and no trade-tick stream in this venue's staged data. The
//! backtestable deliverable is therefore the OHLCV candle:
//!
//! ```text
//! staged table=bars (provider candleSnapshot, JSONL)
//!   -> CanonicalHip3BarsTable (validated OHLCV + provenance)
//!   -> NautilusTrader Bar projection (write_to_parquet)
//!   -> NautilusTrader ParquetDataCatalog read-back (query_typed_data<Bar>)
//! ```
//!
//! Bolt owns parse + canonical normalization + invariant validation; NautilusTrader
//! owns the catalog write/read and the `Bar`/`BarType` model. No bespoke Arrow or
//! Parquet is hand-rolled for the NautilusTrader catalog type — the catalog write
//! is `ParquetDataCatalog::write_to_parquet::<Bar>` and the read-back is
//! `query_typed_data::<Bar>`, exactly as for the trade-tick path.
//!
//! ## Funding rates
//!
//! The staged `funding_rates` table carries a funding **rate** (a dimensionless
//! per-interval rate such as `-0.0000026148`) and a premium — not a price level.
//! NautilusTrader's `MarkPriceUpdate`/`IndexPriceUpdate` carry a `Price` (a price
//! level), so projecting a funding rate into one would misrepresent the datum.
//! This venue's HIP-3 staged data has no mark/index **price** time series, so the
//! honest NautilusTrader deliverable here is `Bar`. Funding is intentionally not
//! projected; see the round-trip test for the proven path.
//!
//! ## No hardcodes
//!
//! Every runtime value (venue, instrument, interval, precision) is derived from
//! the parsed staged rows or from a caller-supplied [`Hip3BarSelector`] /
//! [`Hip3BarProvenance`]. The only literals here are the staged JSONL field names
//! and the venue's own interval vocabulary, which are part of the source schema
//! contract, not tunable runtime values.

use std::{collections::HashSet, path::Path, str::FromStr};

use anyhow::{Context, Result, bail, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Bar, BarSpecification, BarType},
    enums::{AggregationSource, BarAggregation, PriceType},
    identifiers::InstrumentId,
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::source_proof::SourceProofFidelityClass;

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// Provider-supplied OHLCV candles are an external (not internally aggregated)
/// `LAST`-price series, so they project to an `EXTERNAL` `LAST` bar type.
const BAR_PRICE_TYPE: PriceType = PriceType::Last;
const BAR_AGGREGATION_SOURCE: AggregationSource = AggregationSource::External;

const MILLIS_PER_SECOND: i64 = 1_000;
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// A single staged HIP-3 bar row as emitted by the backfill `candleSnapshot`
/// stage. Only the fields this converter consumes are bound; unknown fields
/// (`raw`, `requested_*`, `*_utc`, …) are ignored by serde.
#[derive(Debug, Clone, Deserialize)]
pub struct StagedHip3BarRow {
    /// Canonical instrument label, for example `hyna:BTC`.
    pub instrument_name: String,
    /// Venue-native symbol (equal to `instrument_name` for HIP-3).
    pub symbol: String,
    /// HIP-3 sub-DEX name, for example `hyna`.
    pub dex_name: String,
    /// Candle interval token, for example `1h`, `4h`, `1d`.
    pub interval: String,
    /// Candle open time in Unix milliseconds.
    pub open_time: i64,
    /// Candle close time in Unix milliseconds.
    pub close_time: i64,
    /// Open price as the exact source decimal string.
    pub open: String,
    /// High price as the exact source decimal string.
    pub high: String,
    /// Low price as the exact source decimal string.
    pub low: String,
    /// Close price as the exact source decimal string.
    pub close: String,
    /// Base-asset volume as the exact source decimal string.
    pub base_volume: String,
    /// Source venue, for example `hyperliquid`.
    pub venue: String,
    /// Product family, for example `hip3_perpetual`.
    pub product_family: String,
}

/// Selects exactly one `(instrument, interval)` series from a staged file.
///
/// A staged HIP-3 bars file mixes several instruments and several intervals.
/// A single NautilusTrader `BarType` is one instrument at one specification, so
/// the caller declares which series to project. Values come from a run spec,
/// never hardcoded in the converter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip3BarSelector {
    /// Source instrument label to keep, for example `hyna:BTC`.
    pub instrument_name: String,
    /// Source interval token to keep, for example `1h`.
    pub interval: String,
}

/// Run/lineage provenance recorded on the canonical table, supplied by the
/// caller (the ingest/run spec). Kept separate from the staged rows so no
/// provenance is invented inside the converter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip3BarProvenance {
    /// Stable identifier of the ingest/run that produced this normalization.
    pub ingest_run_id: String,
    /// Source-proof id the staged object was accepted under.
    pub source_proof_id: String,
    /// Source-proof version.
    pub source_proof_version: u32,
    /// Lowercase SHA-256 hex of the accepted staged object bytes.
    pub payload_hash: String,
    /// Fidelity class for this dataset.
    pub fidelity_class: SourceProofFidelityClass,
    /// Explicit forbidden-claims list carried through to the catalog.
    pub forbidden_claims: Vec<String>,
}

/// One normalized OHLCV bar row with provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalHip3BarRow {
    pub ingest_run_id: String,
    pub source_proof_id: String,
    pub venue: String,
    pub product_family: String,
    pub dex_name: String,
    pub instrument_name: String,
    pub venue_symbol: String,
    /// NautilusTrader instrument id, for example `hyna:BTC.HYPERLIQUID`.
    pub nt_instrument_id: String,
    /// NautilusTrader bar-type string, for example
    /// `hyna:BTC.HYPERLIQUID-1-HOUR-LAST-EXTERNAL`.
    pub nt_bar_type: String,
    /// Candle open time in Unix nanoseconds (the bar event time).
    pub open_time: i64,
    /// Candle close time in Unix nanoseconds.
    pub close_time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub payload_hash: String,
}

/// A validated canonical OHLCV `bars` table for one `(instrument, interval)`
/// series.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalHip3BarsTable {
    pub venue: String,
    pub instrument_name: String,
    pub interval: String,
    /// Uniform NautilusTrader bar-type string for the whole table.
    pub nt_bar_type: String,
    pub nt_instrument_id: String,
    /// Uniform price precision derived from the source decimal strings.
    pub price_precision: u8,
    /// Uniform size precision derived from the source decimal strings.
    pub size_precision: u8,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub payload_hash: String,
    pub rows: Vec<CanonicalHip3BarRow>,
}

/// Maps a HIP-3 interval token to a NautilusTrader `(step, aggregation)` pair.
///
/// # Errors
///
/// Returns an error for an interval token outside the venue's vocabulary.
fn parse_interval(interval: &str) -> Result<(usize, BarAggregation)> {
    // Split the leading integer step from the trailing unit suffix.
    let split = interval
        .find(|c: char| !c.is_ascii_digit())
        .with_context(|| format!("interval {interval:?} has no unit suffix"))?;
    ensure!(split > 0, "interval {interval:?} has no numeric step");
    let (step_str, unit) = interval.split_at(split);
    let step: usize = step_str
        .parse()
        .with_context(|| format!("interval {interval:?} has invalid step {step_str:?}"))?;
    let aggregation = match unit {
        "s" => BarAggregation::Second,
        "m" => BarAggregation::Minute,
        "h" => BarAggregation::Hour,
        "d" => BarAggregation::Day,
        "w" => BarAggregation::Week,
        other => bail!("unsupported interval unit {other:?} in {interval:?}"),
    };
    Ok((step, aggregation))
}

/// Decimal places implied by a decimal-string value (`66805.0` -> 1,
/// `2.52418` -> 5, `100` -> 0). Trailing zeros are significant: the source
/// declares them and trimming would understate the instrument precision.
///
/// # Errors
///
/// Returns an error if the value is not a valid decimal.
fn decimal_places(value: &str) -> Result<u8> {
    // Reject non-decimal tokens early so precision is never read off garbage.
    Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    Ok(match value.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    })
}

fn ms_to_nanos(ms: i64, field: &str) -> Result<i64> {
    ms.checked_mul(NANOS_PER_MILLISECOND)
        .with_context(|| format!("{field} {ms} overflows when scaled to nanoseconds"))
}

/// Build the NautilusTrader instrument id `"<instrument_name>.<VENUE>"`.
///
/// HIP-3 instrument labels contain a colon (`hyna:BTC`); NautilusTrader parses
/// the venue from the final `.` separator, so the colon is preserved in the
/// symbol component.
fn nt_instrument_id(instrument_name: &str, venue: &str) -> String {
    format!("{instrument_name}.{}", venue.to_ascii_uppercase())
}

/// Normalize a staged HIP-3 bars JSONL document into the canonical OHLCV table
/// for exactly one `(instrument, interval)` series.
///
/// `jsonl_text` is the decompressed text of the accepted staged object whose
/// hash already matched the manifest (gate-1 acceptance is the caller's job).
///
/// # Errors
///
/// Returns an error if a line is malformed, the selected series is empty, the
/// interval/precision is not uniform, or any OHLCV invariant is violated.
pub fn normalize_hip3_bars(
    jsonl_text: &str,
    selector: &Hip3BarSelector,
    provenance: &Hip3BarProvenance,
) -> Result<CanonicalHip3BarsTable> {
    ensure!(
        !provenance.ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        !selector.instrument_name.trim().is_empty() && !selector.interval.trim().is_empty(),
        "selector instrument_name and interval must not be empty"
    );

    // Validate that the interval token is one the venue model understands before
    // we commit to a bar specification.
    let (step, aggregation) = parse_interval(&selector.interval)?;
    let spec =
        BarSpecification::new_checked(step, aggregation, BAR_PRICE_TYPE).with_context(|| {
            format!(
                "invalid bar specification for interval {:?}",
                selector.interval
            )
        })?;

    let mut staged: Vec<StagedHip3BarRow> = Vec::new();
    for (index, line) in jsonl_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: StagedHip3BarRow = serde_json::from_str(line)
            .with_context(|| format!("line {index}: invalid staged bar json"))?;
        if row.instrument_name == selector.instrument_name && row.interval == selector.interval {
            staged.push(row);
        }
    }
    ensure!(
        !staged.is_empty(),
        "no staged rows for instrument {:?} interval {:?}",
        selector.instrument_name,
        selector.interval
    );

    // The selected series must be internally consistent: one venue, one symbol.
    // A mixed series would map to an ambiguous instrument id / bar type.
    let venues: HashSet<&str> = staged.iter().map(|r| r.venue.as_str()).collect();
    ensure!(
        venues.len() == 1,
        "selected series spans multiple venues: {venues:?}"
    );
    let venue = staged[0].venue.clone();
    let symbols: HashSet<&str> = staged.iter().map(|r| r.symbol.as_str()).collect();
    ensure!(
        symbols.len() == 1,
        "selected series spans multiple symbols: {symbols:?}"
    );

    let nt_instrument = nt_instrument_id(&selector.instrument_name, &venue);
    let instrument_id = InstrumentId::from_str(&nt_instrument)
        .with_context(|| format!("invalid nt instrument id {nt_instrument:?}"))?;
    let bar_type = BarType::new(instrument_id, spec, BAR_AGGREGATION_SOURCE);
    let nt_bar_type = bar_type.to_string();

    // Uniform precision across every OHLC field and every row: NautilusTrader
    // records one price precision and one size precision per bar type, so the
    // whole series must share them.
    let mut price_precision: u8 = 0;
    let mut size_precision: u8 = 0;
    for row in &staged {
        for field in [&row.open, &row.high, &row.low, &row.close] {
            price_precision = price_precision.max(decimal_places(field)?);
        }
        size_precision = size_precision.max(decimal_places(&row.base_volume)?);
    }

    let mut rows = Vec::with_capacity(staged.len());
    let mut previous_open = i64::MIN;
    for (index, row) in staged.iter().enumerate() {
        let open = Decimal::from_str(&row.open).with_context(|| format!("row {index}: open"))?;
        let high = Decimal::from_str(&row.high).with_context(|| format!("row {index}: high"))?;
        let low = Decimal::from_str(&row.low).with_context(|| format!("row {index}: low"))?;
        let close = Decimal::from_str(&row.close).with_context(|| format!("row {index}: close"))?;
        let volume =
            Decimal::from_str(&row.base_volume).with_context(|| format!("row {index}: volume"))?;
        ensure!(open > Decimal::ZERO, "row {index}: non-positive open");
        ensure!(low > Decimal::ZERO, "row {index}: non-positive low");
        ensure!(volume >= Decimal::ZERO, "row {index}: negative volume");
        // OHLC bar invariants — these are exactly what `Bar::new_checked` enforces;
        // failing here gives a precise per-row diagnostic instead of a panic later.
        ensure!(high >= open, "row {index}: high < open");
        ensure!(high >= low, "row {index}: high < low");
        ensure!(high >= close, "row {index}: high < close");
        ensure!(low <= open, "row {index}: low > open");
        ensure!(low <= close, "row {index}: low > close");

        let open_time = ms_to_nanos(row.open_time, "open_time")?;
        let close_time = ms_to_nanos(row.close_time, "close_time")?;
        ensure!(open_time > 0, "row {index}: non-positive open_time");
        ensure!(
            close_time > open_time,
            "row {index}: close_time <= open_time"
        );
        ensure!(
            open_time >= previous_open,
            "row {index}: open_time {open_time} precedes previous {previous_open}"
        );
        // Provider candle steps must match the declared interval, so the series
        // cannot silently mix a 1h row into a 4h selection via a label error.
        if let Some(expected_span) = interval_span_nanos(step, aggregation) {
            let span = close_time - open_time + NANOS_PER_MILLISECOND;
            ensure!(
                span == expected_span,
                "row {index}: candle span {span}ns != interval span {expected_span}ns"
            );
        }
        previous_open = open_time;

        rows.push(CanonicalHip3BarRow {
            ingest_run_id: provenance.ingest_run_id.clone(),
            source_proof_id: provenance.source_proof_id.clone(),
            venue: venue.clone(),
            product_family: row.product_family.clone(),
            dex_name: row.dex_name.clone(),
            instrument_name: row.instrument_name.clone(),
            venue_symbol: row.symbol.clone(),
            nt_instrument_id: nt_instrument.clone(),
            nt_bar_type: nt_bar_type.clone(),
            open_time,
            close_time,
            open: row.open.clone(),
            high: row.high.clone(),
            low: row.low.clone(),
            close: row.close.clone(),
            volume: row.base_volume.clone(),
            payload_hash: provenance.payload_hash.clone(),
        });
    }

    let table = CanonicalHip3BarsTable {
        venue,
        instrument_name: selector.instrument_name.clone(),
        interval: selector.interval.clone(),
        nt_bar_type,
        nt_instrument_id: nt_instrument,
        price_precision,
        size_precision,
        source_proof_id: provenance.source_proof_id.clone(),
        source_proof_version: provenance.source_proof_version,
        fidelity_class: provenance.fidelity_class,
        forbidden_claims: provenance.forbidden_claims.clone(),
        payload_hash: provenance.payload_hash.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

/// Nanosecond span of one fixed-length interval, or `None` for variable-length
/// aggregations (month/year) which HIP-3 candles do not use.
fn interval_span_nanos(step: usize, aggregation: BarAggregation) -> Option<i64> {
    let unit_ms: i64 = match aggregation {
        BarAggregation::Second => MILLIS_PER_SECOND,
        BarAggregation::Minute => 60 * MILLIS_PER_SECOND,
        BarAggregation::Hour => 60 * 60 * MILLIS_PER_SECOND,
        BarAggregation::Day => 24 * 60 * 60 * MILLIS_PER_SECOND,
        BarAggregation::Week => 7 * 24 * 60 * 60 * MILLIS_PER_SECOND,
        _ => return None,
    };
    let step_ms = i64::try_from(step).ok()?.checked_mul(unit_ms)?;
    step_ms.checked_mul(NANOS_PER_MILLISECOND)
}

impl CanonicalHip3BarsTable {
    /// Validate provenance fields, precision, identity, and per-row OHLCV
    /// invariants and monotonicity.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.rows.is_empty(), "canonical hip3 bars table is empty");
        for field in [
            &self.venue,
            &self.instrument_name,
            &self.interval,
            &self.nt_bar_type,
            &self.nt_instrument_id,
            &self.source_proof_id,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty identity/provenance field");
        }
        ensure!(
            self.fidelity_class != SourceProofFidelityClass::L2Replay,
            "OHLCV bars must not be labelled L2_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "bar-replay table must carry explicit forbidden claims"
        );

        let mut previous_open = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.open_time > 0, "row {index}: non-positive open_time");
            ensure!(
                row.close_time > row.open_time,
                "row {index}: close_time <= open_time"
            );
            ensure!(
                row.open_time >= previous_open,
                "row {index}: open_time {} precedes previous {}",
                row.open_time,
                previous_open
            );
            previous_open = row.open_time;
            ensure!(
                row.nt_bar_type == self.nt_bar_type,
                "row {index}: nt_bar_type does not match table"
            );
            ensure!(
                row.nt_instrument_id == self.nt_instrument_id,
                "row {index}: nt_instrument_id does not match table"
            );
            for field in [&row.open, &row.high, &row.low, &row.close, &row.volume] {
                ensure!(!field.trim().is_empty(), "row {index}: empty OHLCV field");
            }
        }
        Ok(())
    }

    /// Convert the canonical rows into NautilusTrader `Bar`s at the table's
    /// uniform price/size precision.
    ///
    /// # Errors
    ///
    /// Returns an error if a price/volume cannot be represented at the table
    /// precision, or if an OHLCV invariant fails NautilusTrader's own check.
    pub fn to_nt_bars(&self) -> Result<Vec<Bar>> {
        let bar_type = BarType::from_str(&self.nt_bar_type)
            .with_context(|| format!("invalid nt_bar_type {:?}", self.nt_bar_type))?;
        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let open = self
                    .price(&row.open)
                    .with_context(|| format!("row {index}: open"))?;
                let high = self
                    .price(&row.high)
                    .with_context(|| format!("row {index}: high"))?;
                let low = self
                    .price(&row.low)
                    .with_context(|| format!("row {index}: low"))?;
                let close = self
                    .price(&row.close)
                    .with_context(|| format!("row {index}: close"))?;
                let volume = self
                    .quantity(&row.volume)
                    .with_context(|| format!("row {index}: volume"))?;
                let ts = UnixNanos::from(u64::try_from(row.open_time).context("open_time")?);
                Bar::new_checked(bar_type, open, high, low, close, volume, ts, ts)
                    .with_context(|| format!("row {index}: bar invariant"))
            })
            .collect()
    }

    fn price(&self, value: &str) -> Result<Price> {
        let rescaled = rescale(value, self.price_precision)?;
        Price::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid price {rescaled:?}: {error}"))
    }

    fn quantity(&self, value: &str) -> Result<Quantity> {
        let rescaled = rescale(value, self.size_precision)?;
        Quantity::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid quantity {rescaled:?}: {error}"))
    }
}

fn rescale(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than the table allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Result of projecting canonical HIP-3 bars into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip3BarCatalogProjection {
    pub nt_instrument_id: String,
    pub nt_bar_type: String,
    pub data_type: String,
    pub bar_count: usize,
    pub fidelity_class: SourceProofFidelityClass,
}

/// Project a canonical HIP-3 bars table into a NautilusTrader `ParquetDataCatalog`
/// as `Bar` data, using NautilusTrader's own catalog writer.
///
/// The caller owns the catalog root lifecycle and must hand us a clean (absent
/// or empty) root, mirroring the trade-tick projection: NautilusTrader's
/// `write_to_parquet` skips writing when a file for the same interval already
/// exists, so projecting into a dirty root could silently read back stale data.
///
/// # Errors
///
/// Returns an error if conversion or the catalog write fails, or if the root is
/// non-empty.
pub fn project_hip3_bars_to_catalog(
    table: &CanonicalHip3BarsTable,
    catalog_root: &Path,
) -> Result<Hip3BarCatalogProjection> {
    table.validate()?;
    let bars = table.to_nt_bars()?;
    let bar_count = bars.len();

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

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write bars to catalog")?;

    Ok(Hip3BarCatalogProjection {
        nt_instrument_id: table.nt_instrument_id.clone(),
        nt_bar_type: table.nt_bar_type.clone(),
        data_type: NT_DATA_TYPE_BAR.to_string(),
        bar_count,
        fidelity_class: table.fidelity_class,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected `Bar`
/// data back from `catalog_root`.
///
/// The identifier passed to `query_typed_data` may be the bar-type string or its
/// instrument-id prefix (NautilusTrader matches bars on a prefix); we pass the
/// full bar-type for an exact match.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_hip3_bars(catalog_root: &Path, nt_bar_type: &str) -> Result<Vec<Bar>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<Bar>(
            Some(vec![nt_bar_type.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query bars from catalog")
}

// ===========================================================================
// Bulk-conversion append path (data-derived, no clean-root guard)
// ===========================================================================

/// Fixed forbidden-claims list every HIP-3 bar-replay dataset carries: an OHLCV
/// candle stream cannot support execution-quality, queue-position, or
/// order-book-liquidity statements. This is the same contract the per-object
/// proof harness records; it is a dataset-class invariant, not a tunable value.
const HIP3_BARS_FORBIDDEN_CLAIM: &str =
    "No execution-quality, queue-position, or order-book-liquidity claims.";

/// Source-proof id under which HIP-3 bar objects are accepted. Format constant
/// of this venue/family, not a runtime instrument/price value.
const HIP3_BARS_SOURCE_PROOF_ID: &str = "source-proof-hyperliquid-hip3-bars";

/// Source-proof schema version for the HIP-3 bars family.
const HIP3_BARS_SOURCE_PROOF_VERSION: u32 = 1;

/// Fidelity class of an exchange-aggregated OHLCV candle replay.
const HIP3_BARS_FIDELITY_CLASS: SourceProofFidelityClass = SourceProofFidelityClass::TradeBarReplay;

/// Distinct `(instrument_name, interval)` series appearing in a staged HIP-3
/// bars JSONL document, in first-seen order.
///
/// A staged bars object mixes several instruments and several intervals
/// (`hyna:BTC` 1h, `hyna:ETH` 1h, …). A single NautilusTrader `BarType` is one
/// instrument at one specification, so the bulk converter writes one catalog
/// stream per distinct series rather than assuming a single one — mirroring how
/// the OKX trades path enumerates distinct instruments before writing.
///
/// # Errors
///
/// Returns an error if a non-empty line is not valid staged-bar JSON.
pub fn hip3_bar_series(jsonl_text: &str) -> Result<Vec<Hip3BarSelector>> {
    let mut seen: Vec<Hip3BarSelector> = Vec::new();
    for (index, line) in jsonl_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let row: StagedHip3BarRow = serde_json::from_str(line)
            .with_context(|| format!("line {index}: invalid staged bar json"))?;
        let selector = Hip3BarSelector {
            instrument_name: row.instrument_name,
            interval: row.interval,
        };
        if !seen.contains(&selector) {
            seen.push(selector);
        }
    }
    Ok(seen)
}

/// Build honest run/lineage provenance for one staged HIP-3 bars object from the
/// object's own bytes and S3 key.
///
/// Every field describes *this* conversion, none is fabricated:
/// * `payload_hash` is the lowercase SHA-256 hex of the exact object bytes.
/// * `source_proof_id` / `source_proof_version` / `fidelity_class` /
///   `forbidden_claims` are this venue/family's fixed dataset-class contract.
/// * `ingest_run_id` is the staged object's own S3 key, so the canonical rows
///   trace back to the precise object they were normalized from.
///
/// The staged HIP-3 layout partitions by `run={run_id}`, **not** by a `dt=`
/// date segment (see the backfill script's S3 layout), so there is no honest
/// archive-date to extract from the key; the full key is therefore recorded as
/// the run identity rather than inventing a date. See the module-level
/// open-questions note carried in the handoff.
fn hip3_bars_provenance_from_object(object_key: &str, object_bytes: &[u8]) -> Hip3BarProvenance {
    let payload_hash = {
        let mut hasher = Sha256::new();
        hasher.update(object_bytes);
        hex::encode(hasher.finalize())
    };
    Hip3BarProvenance {
        ingest_run_id: object_key.to_string(),
        source_proof_id: HIP3_BARS_SOURCE_PROOF_ID.to_string(),
        source_proof_version: HIP3_BARS_SOURCE_PROOF_VERSION,
        payload_hash,
        fidelity_class: HIP3_BARS_FIDELITY_CLASS,
        forbidden_claims: vec![HIP3_BARS_FORBIDDEN_CLAIM.to_string()],
    }
}

/// One series' write summary produced by [`append_hyperliquid_hip3_bars_archive`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip3BarsAppendSummary {
    pub nt_instrument_id: String,
    pub nt_bar_type: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Append every `(instrument, interval)` series from one staged HIP-3 bars JSONL
/// object into an already-open [`ParquetDataCatalog`] — the bulk-conversion path.
///
/// Unlike [`project_hip3_bars_to_catalog`] (the hermetic single-object proof
/// harness, which refuses a dirty root), this appends into a shared, possibly-S3
/// catalog: it relies on NautilusTrader's own per-bar-type, per-time-range file
/// naming and skip-on-existing so many objects flow into one catalog. Precision
/// is derived from each series' own rows inside [`normalize_hip3_bars`] (HIP-3
/// stages no instrument universe), and provenance is honestly built from the
/// object bytes + key. Returns one summary per distinct series written.
///
/// `object_key` is the S3 key (or equivalent stable locator) of the staged
/// object; it is recorded as the canonical rows' `ingest_run_id` so the data
/// traces back to its source object.
///
/// # Errors
///
/// Returns an error if a line is malformed, a series fails OHLCV/precision
/// validation, bar construction or the catalog write fails, or the object yields
/// no series.
pub fn append_hyperliquid_hip3_bars_archive(
    jsonl_bytes: &[u8],
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip3BarsAppendSummary>> {
    ensure!(
        !object_key.trim().is_empty(),
        "object_key must not be empty"
    );
    let jsonl_text =
        std::str::from_utf8(jsonl_bytes).context("staged HIP-3 bars object is not valid UTF-8")?;
    let provenance = hip3_bars_provenance_from_object(object_key, jsonl_bytes);
    let series = hip3_bar_series(jsonl_text)?;

    let mut summaries = Vec::new();
    for selector in series {
        let table = normalize_hip3_bars(jsonl_text, &selector, &provenance)?;
        let bars = table.to_nt_bars()?;
        let summary = Hip3BarsAppendSummary {
            nt_instrument_id: table.nt_instrument_id.clone(),
            nt_bar_type: table.nt_bar_type.clone(),
            record_count: bars.len(),
            price_precision: table.price_precision,
            size_precision: table.size_precision,
        };
        catalog
            .write_to_parquet(bars, None, None, None)
            .with_context(|| {
                format!(
                    "append HIP-3 bars for {} {}",
                    selector.instrument_name, selector.interval
                )
            })?;
        summaries.push(summary);
    }
    ensure!(!summaries.is_empty(), "HIP-3 bars object yielded no series");
    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance() -> Hip3BarProvenance {
        Hip3BarProvenance {
            ingest_run_id: "ingest-run-test".to_string(),
            source_proof_id: "source-proof-hyperliquid-hip3-bars".to_string(),
            source_proof_version: 1,
            payload_hash: "0".repeat(64),
            fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            forbidden_claims: vec![
                "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
            ],
        }
    }

    const SAMPLE: &str = "\
{\"instrument_name\":\"hyna:BTC\",\"symbol\":\"hyna:BTC\",\"dex_name\":\"hyna\",\"interval\":\"1h\",\"open_time\":1772323200000,\"close_time\":1772326799999,\"open\":\"66980.0\",\"high\":\"67071.0\",\"low\":\"66636.0\",\"close\":\"66805.0\",\"base_volume\":\"2.52418\",\"venue\":\"hyperliquid\",\"product_family\":\"hip3_perpetual\"}
{\"instrument_name\":\"hyna:BTC\",\"symbol\":\"hyna:BTC\",\"dex_name\":\"hyna\",\"interval\":\"1h\",\"open_time\":1772326800000,\"close_time\":1772330399999,\"open\":\"66757.0\",\"high\":\"67670.0\",\"low\":\"66083.0\",\"close\":\"67319.0\",\"base_volume\":\"13.9612\",\"venue\":\"hyperliquid\",\"product_family\":\"hip3_perpetual\"}
{\"instrument_name\":\"hyna:ETH\",\"symbol\":\"hyna:ETH\",\"dex_name\":\"hyna\",\"interval\":\"1h\",\"open_time\":1772323200000,\"close_time\":1772326799999,\"open\":\"2200.0\",\"high\":\"2210.0\",\"low\":\"2180.0\",\"close\":\"2195.0\",\"base_volume\":\"100.5\",\"venue\":\"hyperliquid\",\"product_family\":\"hip3_perpetual\"}";

    fn selector() -> Hip3BarSelector {
        Hip3BarSelector {
            instrument_name: "hyna:BTC".to_string(),
            interval: "1h".to_string(),
        }
    }

    #[test]
    fn parse_interval_maps_venue_vocabulary() {
        assert_eq!(parse_interval("1h").unwrap(), (1, BarAggregation::Hour));
        assert_eq!(parse_interval("4h").unwrap(), (4, BarAggregation::Hour));
        assert_eq!(parse_interval("1d").unwrap(), (1, BarAggregation::Day));
        assert!(parse_interval("1y").is_err());
        assert!(parse_interval("h").is_err());
    }

    #[test]
    fn decimal_places_reads_source_precision() {
        assert_eq!(decimal_places("66805.0").unwrap(), 1);
        assert_eq!(decimal_places("2.52418").unwrap(), 5);
        assert_eq!(decimal_places("100").unwrap(), 0);
        assert!(decimal_places("not-a-number").is_err());
    }

    #[test]
    fn normalizes_only_selected_series() {
        let table = normalize_hip3_bars(SAMPLE, &selector(), &provenance()).expect("normalize");
        // Only the two BTC 1h rows survive; the ETH row is filtered out.
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.instrument_name, "hyna:BTC");
        assert_eq!(table.nt_instrument_id, "hyna:BTC.HYPERLIQUID");
        assert_eq!(table.price_precision, 1);
        assert_eq!(table.size_precision, 5);
        assert_eq!(
            table.rows[0].open_time,
            1_772_323_200_000 * NANOS_PER_MILLISECOND
        );
        assert!(
            table
                .nt_bar_type
                .starts_with("hyna:BTC.HYPERLIQUID-1-HOUR-LAST")
        );
    }

    #[test]
    fn rejects_empty_selection() {
        let mut sel = selector();
        sel.instrument_name = "hyna:NOPE".to_string();
        let err = normalize_hip3_bars(SAMPLE, &sel, &provenance()).unwrap_err();
        assert!(err.to_string().contains("no staged rows"), "{err}");
    }

    #[test]
    fn rejects_broken_ohlc_invariant() {
        // high < open must be caught with a per-row diagnostic.
        let bad = "{\"instrument_name\":\"hyna:BTC\",\"symbol\":\"hyna:BTC\",\"dex_name\":\"hyna\",\"interval\":\"1h\",\"open_time\":1772323200000,\"close_time\":1772326799999,\"open\":\"100.0\",\"high\":\"99.0\",\"low\":\"90.0\",\"close\":\"98.0\",\"base_volume\":\"1.0\",\"venue\":\"hyperliquid\",\"product_family\":\"hip3_perpetual\"}";
        let err = normalize_hip3_bars(bad, &selector(), &provenance()).unwrap_err();
        assert!(err.to_string().contains("high < open"), "{err}");
    }

    #[test]
    fn rejects_non_monotonic_open_time() {
        let bad = "\
{\"instrument_name\":\"hyna:BTC\",\"symbol\":\"hyna:BTC\",\"dex_name\":\"hyna\",\"interval\":\"1h\",\"open_time\":1772326800000,\"close_time\":1772330399999,\"open\":\"100.0\",\"high\":\"110.0\",\"low\":\"90.0\",\"close\":\"105.0\",\"base_volume\":\"1.0\",\"venue\":\"hyperliquid\",\"product_family\":\"hip3_perpetual\"}
{\"instrument_name\":\"hyna:BTC\",\"symbol\":\"hyna:BTC\",\"dex_name\":\"hyna\",\"interval\":\"1h\",\"open_time\":1772323200000,\"close_time\":1772326799999,\"open\":\"100.0\",\"high\":\"110.0\",\"low\":\"90.0\",\"close\":\"105.0\",\"base_volume\":\"1.0\",\"venue\":\"hyperliquid\",\"product_family\":\"hip3_perpetual\"}";
        let err = normalize_hip3_bars(bad, &selector(), &provenance()).unwrap_err();
        assert!(err.to_string().contains("precedes previous"), "{err}");
    }

    #[test]
    fn rejects_empty_ingest_run_id() {
        let mut prov = provenance();
        prov.ingest_run_id = "  ".to_string();
        let err = normalize_hip3_bars(SAMPLE, &selector(), &prov).unwrap_err();
        assert!(err.to_string().contains("ingest_run_id"), "{err}");
    }

    #[test]
    fn builds_nt_bars_at_table_precision() {
        let table = normalize_hip3_bars(SAMPLE, &selector(), &provenance()).expect("normalize");
        let bars = table.to_nt_bars().expect("bars");
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].close, Price::from("66805.0"));
        assert_eq!(bars[0].volume, Quantity::from("2.52418"));
    }
}
