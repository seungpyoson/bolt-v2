//! Chainlink per-second underlying price feed -> NautilusTrader catalog.
//!
//! Chainlink Data Streams emit a per-second index price for the underlying of an
//! up/down market (no order book, no trades). Each staged Parquet object holds
//! one 5-minute cycle of a single market slug. The columns are:
//!
//! ```text
//! timestamp(bigint, UNIX SECONDS), datetime(timestamp), price(double),
//! resolution(varchar), source(varchar), market_slug(varchar),
//! cycle_start(bigint), cycle_end(bigint)
//! ```
//!
//! This module reads such an object into a canonical price table, then projects
//! each row into a NautilusTrader [`IndexPriceUpdate`] (`CatalogPathPrefix`
//! `index_prices`) and writes/reads it through NautilusTrader's own
//! [`ParquetDataCatalog`]. No order book or trade data exists for this venue, so
//! `IndexPriceUpdate` -- the underlying reference price NT replays -- is the
//! correct target type.
//!
//! NT-first: the catalog is written via [`ParquetDataCatalog::write_to_parquet`]
//! and read back via [`ParquetDataCatalog::query_typed_data`]. No Arrow/Parquet
//! is hand-rolled for the NT catalog type.
//!
//! No instrument identity, precision, or venue string is hardcoded in this
//! module: they arrive in a [`ChainlinkIndexSpec`] from the caller's run spec.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, ensure};
use arrow::array::{Array, Float64Array, Int64Array, StringArray};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::prices::IndexPriceUpdate,
    identifiers::InstrumentId,
    types::{Price, fixed::FIXED_PRECISION},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_INDEX_PRICE_UPDATE: &str = "IndexPriceUpdate";

/// Resolution token the per-second Chainlink feed must carry.
pub const CHAINLINK_RESOLUTION_PER_SECOND: &str = "1s";

/// Source token identifying the per-second Chainlink Data Streams feed.
pub const CHAINLINK_SOURCE_PER_SECOND: &str = "chainlink_datastreams_persecond";

/// Required raw Parquet columns, by name, that the per-second feed must expose.
pub const CHAINLINK_REQUIRED_COLUMNS: [&str; 8] = [
    "timestamp",
    "datetime",
    "price",
    "resolution",
    "source",
    "market_slug",
    "cycle_start",
    "cycle_end",
];

const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// Run-spec identity for one Chainlink price feed.
///
/// Supplied by the caller from the run spec / instrument universe, never
/// hardcoded here. `nt_instrument_id` must be a valid NautilusTrader
/// `SYMBOL.VENUE` identifier for the underlying (for example the up/down
/// market's underlying asset on the price-feed venue). `price_precision` is the
/// number of decimal places to materialize the index price at; it must not lose
/// significant digits versus the source feed and must fit NT's fixed precision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainlinkIndexSpec {
    /// NautilusTrader instrument id for the underlying, e.g. `BTCUSD.CHAINLINK`.
    pub nt_instrument_id: String,
    /// Decimal places to materialize the index price at.
    pub price_precision: u8,
    /// Expected market slug for the staged object (provenance guard).
    pub market_slug: String,
}

/// One canonical per-second price row with provenance preserved from the source.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalIndexPriceRow {
    /// Source event timestamp in Unix nanoseconds (seconds source, scaled up).
    pub event_time_nanos: i64,
    /// Source index price as a finite f64, exactly as stored in the feed.
    pub price: f64,
    /// Source `resolution` token (must be the per-second token).
    pub resolution: String,
    /// Source `source` token (must be the per-second Chainlink feed).
    pub source: String,
    /// Source `market_slug` for the cycle.
    pub market_slug: String,
    /// Cycle start (Unix seconds) from the source row.
    pub cycle_start: i64,
    /// Cycle end (Unix seconds) from the source row.
    pub cycle_end: i64,
}

/// Canonical per-second price table for one staged Chainlink object.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalIndexPriceTable {
    /// Market slug shared by every row in the table.
    pub market_slug: String,
    /// Rows in ascending `event_time_nanos` order.
    pub rows: Vec<CanonicalIndexPriceRow>,
    /// Lowercase SHA-256 hex over the source object bytes.
    pub source_object_hash: String,
}

impl CanonicalIndexPriceTable {
    /// Validate the canonical invariants the projection relies on.
    ///
    /// # Errors
    ///
    /// Returns an error when the table is empty, timestamps are not strictly
    /// ascending, a price is non-finite, or provenance tokens disagree.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.rows.is_empty(), "canonical price table is empty");
        let mut previous: Option<i64> = None;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.resolution == CHAINLINK_RESOLUTION_PER_SECOND,
                "row {index} resolution {:?} is not the per-second feed token {:?}",
                row.resolution,
                CHAINLINK_RESOLUTION_PER_SECOND
            );
            ensure!(
                row.source == CHAINLINK_SOURCE_PER_SECOND,
                "row {index} source {:?} is not the per-second Chainlink feed {:?}",
                row.source,
                CHAINLINK_SOURCE_PER_SECOND
            );
            ensure!(
                row.market_slug == self.market_slug,
                "row {index} market_slug {:?} differs from table slug {:?}",
                row.market_slug,
                self.market_slug
            );
            ensure!(
                row.price.is_finite(),
                "row {index} price {} is not finite",
                row.price
            );
            if let Some(previous) = previous {
                ensure!(
                    row.event_time_nanos > previous,
                    "row {index} timestamp {} not strictly after previous {}",
                    row.event_time_nanos,
                    previous
                );
            }
            previous = Some(row.event_time_nanos);
        }
        Ok(())
    }
}

/// Result of projecting a canonical price table into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainlinkCatalogProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub update_count: usize,
    /// Deterministic SHA-256 hex over the catalog's written data files.
    pub catalog_hash: String,
}

/// Read a staged Chainlink per-second Parquet object into a canonical table.
///
/// Reads exactly the contracted columns with NautilusTrader-independent Arrow
/// (the source object is bolt-owned staged data, not an NT catalog type), scales
/// the Unix-seconds `timestamp` to nanoseconds, and preserves provenance tokens.
///
/// # Errors
///
/// Returns an error when the file cannot be read, a required column is missing
/// or of the wrong Arrow type, a value is null, or the table fails validation.
pub fn read_chainlink_per_second_object(path: &Path) -> Result<CanonicalIndexPriceTable> {
    let bytes = fs::read(path).with_context(|| format!("read parquet {}", path.display()))?;
    let source_object_hash = hex::encode(Sha256::digest(&bytes));

    let file = File::open(path).with_context(|| format!("open parquet {}", path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| format!("open parquet reader {}", path.display()))?;

    // Fail loud if the staged object lost a contracted column.
    let schema = builder.schema().clone();
    for column in CHAINLINK_REQUIRED_COLUMNS {
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

    let mut rows: Vec<CanonicalIndexPriceRow> = Vec::new();
    for batch in reader {
        let batch = batch.context("read parquet record batch")?;
        let timestamp = typed_column::<Int64Array>(&batch, "timestamp")?;
        let price = typed_column::<Float64Array>(&batch, "price")?;
        let resolution = typed_column::<StringArray>(&batch, "resolution")?;
        let source = typed_column::<StringArray>(&batch, "source")?;
        let market_slug = typed_column::<StringArray>(&batch, "market_slug")?;
        let cycle_start = typed_column::<Int64Array>(&batch, "cycle_start")?;
        let cycle_end = typed_column::<Int64Array>(&batch, "cycle_end")?;

        for index in 0..batch.num_rows() {
            ensure!(
                !timestamp.is_null(index)
                    && !price.is_null(index)
                    && !resolution.is_null(index)
                    && !source.is_null(index)
                    && !market_slug.is_null(index)
                    && !cycle_start.is_null(index)
                    && !cycle_end.is_null(index),
                "null value in row {index} of {}",
                path.display()
            );
            let seconds = timestamp.value(index);
            let event_time_nanos = seconds
                .checked_mul(NANOS_PER_SECOND)
                .with_context(|| format!("timestamp {seconds} overflows nanoseconds"))?;
            ensure!(
                event_time_nanos >= 0,
                "negative event time {event_time_nanos} in {}",
                path.display()
            );
            rows.push(CanonicalIndexPriceRow {
                event_time_nanos,
                price: price.value(index),
                resolution: resolution.value(index).to_string(),
                source: source.value(index).to_string(),
                market_slug: market_slug.value(index).to_string(),
                cycle_start: cycle_start.value(index),
                cycle_end: cycle_end.value(index),
            });
        }
    }

    ensure!(
        !rows.is_empty(),
        "staged object {} yielded no rows",
        path.display()
    );
    let market_slug = rows[0].market_slug.clone();
    let table = CanonicalIndexPriceTable {
        market_slug,
        rows,
        source_object_hash,
    };
    table.validate()?;
    Ok(table)
}

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

/// Convert canonical rows into NautilusTrader `IndexPriceUpdate`s.
///
/// The index price is materialized at `spec.price_precision`. Construction fails
/// loud (rather than truncating) if the chosen precision exceeds NautilusTrader's
/// fixed precision or the value is outside the representable price range.
///
/// # Errors
///
/// Returns an error when the instrument id is invalid, the precision is
/// unrepresentable, or a price cannot be built at that precision.
pub fn canonical_rows_to_index_prices(
    table: &CanonicalIndexPriceTable,
    spec: &ChainlinkIndexSpec,
) -> Result<Vec<IndexPriceUpdate>> {
    ensure!(
        table.market_slug == spec.market_slug,
        "table market slug {:?} does not match spec slug {:?}",
        table.market_slug,
        spec.market_slug
    );
    ensure!(
        spec.price_precision <= FIXED_PRECISION,
        "price_precision {} exceeds NautilusTrader fixed precision {FIXED_PRECISION}",
        spec.price_precision
    );
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;

    table
        .rows
        .iter()
        .map(|row| {
            let price = Price::new_checked(row.price, spec.price_precision).map_err(|error| {
                anyhow::anyhow!(
                    "price {} not representable at precision {}: {error}",
                    row.price,
                    spec.price_precision
                )
            })?;
            let ts =
                UnixNanos::from(u64::try_from(row.event_time_nanos).map_err(|_| {
                    anyhow::anyhow!("negative event time {}", row.event_time_nanos)
                })?);
            Ok(IndexPriceUpdate::new(instrument_id, price, ts, ts))
        })
        .collect()
}

/// Project a canonical price table into a NautilusTrader `ParquetDataCatalog`.
///
/// Writes the `IndexPriceUpdate` projection under `catalog_root` via
/// NautilusTrader's own writer and returns a deterministic catalog hash.
///
/// # Errors
///
/// Returns an error when validation, conversion, or the catalog write fails, or
/// when `catalog_root` is a non-empty directory (refusing a dirty root keeps the
/// read-back honest, since NT skips writing over an existing interval file).
pub fn project_chainlink_to_catalog(
    table: &CanonicalIndexPriceTable,
    spec: &ChainlinkIndexSpec,
    catalog_root: &Path,
) -> Result<ChainlinkCatalogProjection> {
    table.validate()?;
    let updates = canonical_rows_to_index_prices(table, spec)?;
    let update_count = updates.len();
    let instrument_id = updates[0].instrument_id;

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
        .write_to_parquet(updates, None, None, None)
        .context("write index price updates to catalog")?;

    Ok(ChainlinkCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: instrument_id.to_string(),
        data_type: NT_DATA_TYPE_INDEX_PRICE_UPDATE.to_string(),
        update_count,
        catalog_hash: catalog_hash(catalog_root)?,
    })
}

/// Read the projected `IndexPriceUpdate` data back from `catalog_root` using
/// NautilusTrader's own typed query, proving the catalog is NT-replayable.
///
/// # Errors
///
/// Returns an error when the catalog query fails.
pub fn read_back_index_prices(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<IndexPriceUpdate>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<IndexPriceUpdate>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query index price updates from catalog")
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
