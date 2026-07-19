//! Gate 3 — NautilusTrader catalog projection.
//!
//! Projects a validated [`CanonicalTradesTable`] into a NautilusTrader
//! `ParquetDataCatalog` as `TradeTick` data plus the venue instrument, using
//! NautilusTrader APIs directly (no custom simulation behaviour), then proves
//! the resolved `bolt-v2` NautilusTrader dependency can read the projection back.
//!
//! The NautilusTrader instrument is built from accepted instrument-universe
//! metadata ([`CatalogInstrumentSpec`]); price/size precision and increments
//! are derived from the source tick size and size precision, never hardcoded.
//! When the accepted archive carries finer prints than the venue's current
//! instrument metadata, the instrument precision is widened to the data's
//! actual scale (trailing-zero increment rescale; tick value unchanged) so
//! the projection represents the accepted data exactly.

use std::{
    cmp::Ordering,
    collections::HashSet,
    fmt::{self, Debug, Write},
    fs,
    io::{ErrorKind, Read},
    mem::{size_of, size_of_val},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, ensure};
use nautilus_core::{Params, UnixNanos, string::urlencoding};
use nautilus_model::{
    data::{
        Bar, BarSpecification, BarType, CatalogPathPrefix, FundingRateUpdate, IndexPriceUpdate,
        MarkPriceUpdate, OrderBookDelta, QuoteTick, TradeTick, order::BookOrder,
    },
    enums::{AggregationSource, AggressorSide, AssetClass, BookAction, OrderSide, PriceType},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{
        BinaryOption, CryptoFuture, CryptoPerpetual, CurrencyPair, Instrument, InstrumentAny,
    },
    types::{Currency, Money, Price, Quantity},
};
use nautilus_persistence::backend::catalog::{ParquetDataCatalog, urisafe_instrument_id};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use super::{
    artifact_store::{CatalogCompression, CatalogEncodingConfig},
    atomic_artifact_write::{
        DirectoryStageOutcome, OwnedTempDirectory, capture_owned_directory_manifest_guarded,
        compact_owned_temp_directory_to_receipt_guarded, create_owned_temp_directory_guarded,
        guarded_publication_child_path, initialize_owned_temp_directory_receipt_guarded,
        open_pinned_regular_file, stage_directory_rename_create_only_guarded,
        unique_temp_path_guarded, validate_existing_directory_manifest_identical_guarded,
        validate_staged_directory_manifest_guarded,
    },
    canonical_market_data::{
        CanonicalBarRow, CanonicalBarsTable, CanonicalFundingRateRow, CanonicalFundingRatesTable,
        CanonicalIndexPriceRow, CanonicalIndexPricesTable, CanonicalMarkPriceRow,
        CanonicalMarkPricesTable, CanonicalOrderBookDeltaRow, CanonicalOrderBookDeltasTable,
        CanonicalQuoteRow, CanonicalQuotesTable, DeltaAction, DeltaSide,
        bar_row_materialized_bytes, delta_row_materialized_bytes,
        funding_rate_row_materialized_bytes, mark_price_row_materialized_bytes,
        point_price_row_materialized_bytes, quote_row_materialized_bytes,
    },
    canonical_trades::{
        CanonicalTradesTable, TradeAggressorSide, canonical_trade_row_materialized_bytes,
        verify_canonical_rows_materialization, verify_parquet_file_trailer_preflight,
        verify_single_parquet_metadata_budget,
    },
    operator_work_budget::{
        OperatorWorkBudgetGuard, OperatorWorkBudgetStage, cooperative_stable_sort_by,
        cooperative_stable_sort_by_key, guarded_operation_outcome, projected_row_group_count,
    },
    source_proof::SourceProofFidelityClass,
    source_universe_local_storage::{
        SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES, SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE,
    },
};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

// Logical catalog v1 orders display-form keys lexicographically. Measure the
// actual keys once, reserve two bounded buffers, and reuse them for every sort
// comparison instead of allocating O(n log n) temporary Strings.
#[derive(Default)]
struct DisplayLengthCounter {
    bytes: usize,
}

impl Write for DisplayLengthCounter {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        self.bytes = self.bytes.checked_add(value.len()).ok_or(fmt::Error)?;
        Ok(())
    }
}

#[derive(Default)]
struct DisplayLengthMeasure {
    max_bytes: usize,
}

impl DisplayLengthMeasure {
    fn observe<T: fmt::Display>(&mut self, value: &T) -> Result<()> {
        let mut counter = DisplayLengthCounter::default();
        write!(&mut counter, "{value}").map_err(|_| {
            anyhow::anyhow!("logical-v1 display key length overflow or formatting failure")
        })?;
        self.max_bytes = self.max_bytes.max(counter.bytes);
        Ok(())
    }
}

struct ReusableDisplayBuffer {
    value: String,
}

impl ReusableDisplayBuffer {
    fn with_capacity(capacity: usize) -> Result<Self> {
        let mut value = String::new();
        value
            .try_reserve_exact(capacity)
            .context("reserve logical-v1 display-sort scratch buffer")?;
        Ok(Self { value })
    }

    fn clear(&mut self) {
        self.value.clear();
    }

    fn as_bytes(&self) -> &[u8] {
        self.value.as_bytes()
    }

    fn capacity(&self) -> usize {
        self.value.capacity()
    }
}

impl Write for ReusableDisplayBuffer {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self
            .value
            .len()
            .checked_add(value.len())
            .ok_or(fmt::Error)?;
        if end > self.value.capacity() {
            return Err(fmt::Error);
        }
        self.value.push_str(value);
        Ok(())
    }
}

struct DisplaySortScratch {
    left: ReusableDisplayBuffer,
    right: ReusableDisplayBuffer,
    formatting_failed: bool,
}

impl DisplaySortScratch {
    fn new_guarded(
        max_display_bytes: usize,
        row_count: usize,
        work_budget: &OperatorWorkBudgetGuard,
        stage: OperatorWorkBudgetStage,
    ) -> Result<Self> {
        let allocation_limit = work_budget
            .decoded_byte_limit()
            .map_or(usize::MAX, |limit| {
                usize::try_from(limit).unwrap_or(usize::MAX)
            });
        ensure!(
            max_display_bytes <= allocation_limit,
            "logical-v1 display-sort scratch request {max_display_bytes} exceeds max_decoded_bytes {allocation_limit}"
        );
        let metadata_bytes = row_count
            .checked_mul(size_of::<usize>())
            .context("logical-v1 stable-sort metadata byte count overflow")?;
        let scratch_control_bytes = size_of::<DisplaySortScratch>();
        let prospective_bytes = max_display_bytes
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(scratch_control_bytes))
            .and_then(|bytes| bytes.checked_add(metadata_bytes))
            .context("logical-v1 display-sort prospective byte count overflow")?;
        work_budget.verify_decoded_bytes(
            u64::try_from(prospective_bytes)
                .context("logical-v1 display-sort prospective bytes do not fit u64")?,
            stage,
        )?;
        work_budget.check_deadline(stage)?;

        let left = ReusableDisplayBuffer::with_capacity(max_display_bytes)?;
        let right = ReusableDisplayBuffer::with_capacity(max_display_bytes)?;
        ensure!(
            left.capacity() <= allocation_limit && right.capacity() <= allocation_limit,
            "logical-v1 actual display-sort scratch allocation exceeds max_decoded_bytes {allocation_limit}"
        );
        let actual_bytes = left
            .capacity()
            .checked_add(right.capacity())
            .and_then(|bytes| bytes.checked_add(scratch_control_bytes))
            .and_then(|bytes| bytes.checked_add(metadata_bytes))
            .context("logical-v1 display-sort actual byte count overflow")?;
        work_budget.verify_decoded_bytes(
            u64::try_from(actual_bytes)
                .context("logical-v1 display-sort actual bytes do not fit u64")?,
            stage,
        )?;
        work_budget.check_deadline(stage)?;
        Ok(Self {
            left,
            right,
            formatting_failed: false,
        })
    }

    fn compare<T: fmt::Display>(&mut self, left: &T, right: &T) -> Ordering {
        if self.formatting_failed {
            return Ordering::Equal;
        }
        self.left.clear();
        self.right.clear();
        if write!(&mut self.left, "{left}").is_err() || write!(&mut self.right, "{right}").is_err()
        {
            self.formatting_failed = true;
            return Ordering::Equal;
        }
        self.left.as_bytes().cmp(self.right.as_bytes())
    }

    fn ensure_succeeded(&self, key_label: &str) -> Result<()> {
        ensure!(
            !self.formatting_failed,
            "{key_label} display formatting exceeded its measured logical-v1 sort scratch"
        );
        Ok(())
    }
}

fn cooperative_stable_sort_by_display_guarded<T>(
    values: &mut [T],
    mut measure_keys: impl FnMut(&T, &mut DisplayLengthMeasure) -> Result<()>,
    mut compare: impl FnMut(&T, &T, &mut DisplaySortScratch) -> Ordering,
    key_label: &str,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    work_budget.check_deadline(stage)?;
    if values.len() < 2 {
        return Ok(());
    }
    let mut lengths = DisplayLengthMeasure::default();
    for value in values.iter() {
        work_budget.check_deadline(stage)?;
        measure_keys(value, &mut lengths)?;
        work_budget.check_deadline(stage)?;
    }
    let max_display_bytes = lengths.max_bytes;
    let mut scratch =
        DisplaySortScratch::new_guarded(max_display_bytes, values.len(), work_budget, stage)?;
    cooperative_stable_sort_by(
        values,
        |left, right| compare(left, right, &mut scratch),
        work_budget,
        stage,
    )?;
    scratch.ensure_succeeded(key_label)?;
    work_budget.check_deadline(stage)
}

/// NautilusTrader data type written for the order-book-delta projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// NautilusTrader data type written for the bar projection.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// NautilusTrader data type written for the top-of-book quote projection.
pub const NT_DATA_TYPE_QUOTE_TICK: &str = "QuoteTick";

/// NautilusTrader data type written for the index-price projection.
///
/// The token MUST equal the NT struct name; the catalog directory is
/// `index_prices` via NT's own `impl_catalog_path_prefix!(IndexPriceUpdate,
/// "index_prices")` — never redefined here.
pub const NT_DATA_TYPE_INDEX_PRICE_UPDATE: &str = "IndexPriceUpdate";

/// NautilusTrader data type written for the mark-price projection.
///
/// The token MUST equal the NT struct name; the catalog directory is
/// `mark_prices` via NT's own `impl_catalog_path_prefix!(MarkPriceUpdate,
/// "mark_prices")` — never redefined here.
pub const NT_DATA_TYPE_MARK_PRICE_UPDATE: &str = "MarkPriceUpdate";

/// NautilusTrader data type written for the funding-rate projection.
///
/// The token MUST equal the NT struct name; the catalog directory is
/// `funding_rate_update` via NT's own
/// `impl_catalog_path_prefix!(FundingRateUpdate, "funding_rate_update")`.
pub const NT_DATA_TYPE_FUNDING_RATE_UPDATE: &str = "FundingRateUpdate";

fn configured_nt_catalog(
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> ParquetDataCatalog {
    let compression = match encoding.compression() {
        CatalogCompression::Snappy => parquet::basic::Compression::SNAPPY,
    };
    ParquetDataCatalog::new(
        catalog_root,
        None,
        Some(encoding.batch_size()),
        Some(compression),
        Some(encoding.max_row_group_size()),
    )
}

/// Exact pre-write row-group projection for NT market-data tables.
pub(crate) fn projected_nt_market_data_row_groups(
    table_rows: impl IntoIterator<Item = u64>,
    encoding: &CatalogEncodingConfig,
) -> Result<u64> {
    projected_row_group_count(
        table_rows,
        u64::try_from(encoding.max_row_group_size())
            .context("configured catalog max_row_group_size does not fit u64")?,
    )
}

fn guarded_catalog_operation<T>(
    work_budget: &OperatorWorkBudgetGuard,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    // NT catalog queries/writes are opaque synchronous units: these fences
    // classify an over-deadline return correctly, but do not claim mid-call
    // preemption. Code-owned projection/hash/equality loops below additionally
    // observe the deadline at their natural row boundaries.
    guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
        operation,
    )?
}

fn collect_projected_rows_guarded<R, T>(
    rows: &[R],
    work_budget: &OperatorWorkBudgetGuard,
    row_materialized_bytes: impl Fn(&R) -> Result<usize>,
    mut project: impl FnMut(&R) -> Result<T>,
) -> Result<Vec<T>> {
    verify_canonical_rows_materialization(
        rows,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
        row_materialized_bytes,
    )?;
    let mut projected = Vec::new();
    projected
        .try_reserve_exact(rows.len())
        .context("reserve projected catalog rows")?;
    for row in rows {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        projected.push(project(row)?);
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    Ok(projected)
}

/// Actual Parquet metadata totals for NT market-data files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NtMarketDataParquetMetadata {
    pub rows: u64,
    pub row_groups: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NtCatalogParquetFilePreflight {
    pub(crate) relative_path: PathBuf,
    pub(crate) file_bytes: u64,
    pub(crate) footer_metadata_bytes: u64,
    pub(crate) rows: u64,
    pub(crate) row_groups: u64,
    pub(crate) uncompressed_bytes: u64,
    pub(crate) is_instrument_metadata: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NtCatalogPreflightSummary {
    pub(crate) files: Vec<NtCatalogParquetFilePreflight>,
    pub(crate) market_data: NtMarketDataParquetMetadata,
    pub(crate) total_file_bytes: u64,
    pub(crate) total_footer_metadata_bytes: u64,
    pub(crate) total_rows: u64,
    pub(crate) total_row_groups: u64,
    pub(crate) total_uncompressed_bytes: u64,
    pub(crate) total_inventory_bytes: u64,
    pub(crate) total_accounted_bytes: u64,
}

/// Count actual NT market-data Parquet rows and row groups, excluding only the
/// exact `data/instruments/**` subtree.
#[cfg(test)]
pub(crate) fn actual_nt_market_data_metadata(
    catalog_root: &Path,
) -> Result<NtMarketDataParquetMetadata> {
    actual_nt_market_data_metadata_guarded(catalog_root, &OperatorWorkBudgetGuard::unbounded())
}

pub(crate) fn actual_nt_market_data_metadata_guarded(
    catalog_root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<NtMarketDataParquetMetadata> {
    Ok(preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?
    .market_data)
}

pub(crate) fn preflight_nt_catalog_parquet_guarded(
    catalog_root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<NtCatalogPreflightSummary> {
    let root_metadata_outcome =
        guarded_operation_outcome(work_budget, stage, || fs::symlink_metadata(catalog_root))?;
    let root_metadata = match root_metadata_outcome {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(NtCatalogPreflightSummary {
                files: Vec::new(),
                market_data: NtMarketDataParquetMetadata {
                    rows: 0,
                    row_groups: 0,
                },
                total_file_bytes: 0,
                total_footer_metadata_bytes: 0,
                total_rows: 0,
                total_row_groups: 0,
                total_uncompressed_bytes: 0,
                total_inventory_bytes: 0,
                total_accounted_bytes: 0,
            });
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("lstat projected catalog root {}", catalog_root.display())
            });
        }
    };
    ensure!(
        root_metadata.file_type().is_dir(),
        "projected catalog root {} must be a directory, not a symlink or non-directory",
        catalog_root.display()
    );
    let mut summary = NtCatalogPreflightSummary {
        files: Vec::new(),
        market_data: NtMarketDataParquetMetadata {
            rows: 0,
            row_groups: 0,
        },
        total_file_bytes: 0,
        total_footer_metadata_bytes: 0,
        total_rows: 0,
        total_row_groups: 0,
        total_uncompressed_bytes: 0,
        total_inventory_bytes: 0,
        total_accounted_bytes: 0,
    };
    accumulate_catalog_parquet_preflight_guarded(
        catalog_root,
        catalog_root,
        &mut summary,
        work_budget,
        stage,
    )?;
    verify_catalog_preflight_totals(&summary, work_budget, stage)?;
    Ok(summary)
}

fn accumulate_catalog_parquet_preflight_guarded(
    catalog_root: &Path,
    directory: &Path,
    summary: &mut NtCatalogPreflightSummary,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let directory_metadata = guarded_operation_outcome(work_budget, stage, || {
        fs::symlink_metadata(directory)
            .with_context(|| format!("lstat projected catalog directory {}", directory.display()))
    })??;
    ensure!(
        directory_metadata.file_type().is_dir(),
        "projected catalog directory {} must remain a directory, not a symlink or non-directory",
        directory.display()
    );
    let mut entries = guarded_operation_outcome(work_budget, stage, || {
        fs::read_dir(directory)
            .with_context(|| format!("read projected catalog directory {}", directory.display()))
    })??;
    loop {
        let entry = guarded_operation_outcome(work_budget, stage, || {
            entries.next().transpose().with_context(|| {
                format!("read projected catalog entry under {}", directory.display())
            })
        })??;
        let Some(entry) = entry else { break };
        let path = entry.path();
        let path_metadata = guarded_operation_outcome(work_budget, stage, || {
            fs::symlink_metadata(&path)
                .with_context(|| format!("lstat projected catalog entry {}", path.display()))
        })??;
        let file_type = path_metadata.file_type();
        ensure!(
            !file_type.is_symlink(),
            "projected catalog entry {} must not be a symlink",
            path.display()
        );
        if file_type.is_dir() {
            accumulate_catalog_parquet_preflight_guarded(
                catalog_root,
                &path,
                summary,
                work_budget,
                stage,
            )?;
            continue;
        }
        ensure!(
            file_type.is_file(),
            "projected catalog entry {} must be a regular file or directory",
            path.display()
        );
        if path
            .extension()
            .is_none_or(|extension| extension != "parquet")
        {
            continue;
        }
        preflight_one_catalog_parquet_guarded(catalog_root, &path, summary, work_budget, stage)?;
    }
    let directory_metadata_after = guarded_operation_outcome(work_budget, stage, || {
        fs::symlink_metadata(directory).with_context(|| {
            format!(
                "re-lstat projected catalog directory {} after traversal",
                directory.display()
            )
        })
    })??;
    ensure!(
        directory_metadata_after.file_type().is_dir(),
        "projected catalog directory {} changed type during traversal",
        directory.display()
    );
    Ok(())
}

fn preflight_one_catalog_parquet_guarded(
    catalog_root: &Path,
    path: &Path,
    summary: &mut NtCatalogPreflightSummary,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let next_file_count = summary
        .files
        .len()
        .checked_add(1)
        .context("catalog Parquet file count overflow")?;
    work_budget
        .verify_actual_row_groups(
            u64::try_from(next_file_count).context("catalog Parquet file count does not fit u64")?,
            stage,
        )
        .with_context(|| {
            format!(
                "catalog Parquet file count actual {next_file_count} exceeds the configured row-group-derived file cap"
            )
        })?;
    let relative_path = path
        .strip_prefix(catalog_root)
        .with_context(|| format!("derive projected catalog relative path {}", path.display()))?;
    let inventory_bytes = size_of::<NtCatalogParquetFilePreflight>()
        .checked_add(relative_path.as_os_str().len())
        .context("catalog Parquet inventory record byte size overflow")?;
    let inventory_bytes_u64 =
        u64::try_from(inventory_bytes).context("catalog inventory bytes do not fit u64")?;
    let allocation_limit = work_budget
        .decoded_byte_limit()
        .map_or(usize::MAX, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        });
    ensure!(
        inventory_bytes <= allocation_limit,
        "catalog Parquet inventory record {} requires {inventory_bytes} bytes, exceeding max_decoded_bytes {allocation_limit}",
        relative_path.display()
    );
    let (mut file, identity) = guarded_operation_outcome(work_budget, stage, || {
        open_pinned_regular_file(path)
            .with_context(|| format!("pin projected Parquet file {}", path.display()))
    })??;
    let trailer = verify_parquet_file_trailer_preflight(&mut file, path, work_budget, stage)?;
    ensure!(
        trailer.file_bytes == identity.byte_len,
        "projected Parquet file {} trailer preflight length {} disagrees with pinned length {}",
        path.display(),
        trailer.file_bytes,
        identity.byte_len
    );
    let accounted_before_metadata = summary
        .total_accounted_bytes
        .checked_add(trailer.file_bytes)
        .and_then(|total| total.checked_add(trailer.footer_metadata_bytes))
        .and_then(|total| total.checked_add(inventory_bytes_u64))
        .context("catalog Parquet pre-builder accounted byte total overflow")?;
    work_budget.verify_decoded_bytes(accounted_before_metadata, stage)?;
    let builder = guarded_operation_outcome(work_budget, stage, || {
        ParquetRecordBatchReaderBuilder::try_new(file)
            .with_context(|| format!("read projected Parquet metadata {}", path.display()))
    })??;
    let metadata = verify_single_parquet_metadata_budget(builder.metadata(), work_budget, stage)?;
    ensure!(
        metadata.row_groups > 0,
        "projected Parquet file {} has zero row groups; every catalog file must consume at least one configured row-group/file-count unit",
        path.display()
    );
    guarded_operation_outcome(work_budget, stage, || {
        identity.revalidate_path(path).with_context(|| {
            format!(
                "revalidate projected Parquet path {} after metadata decode",
                path.display()
            )
        })
    })??;
    accumulate_catalog_preflight_total(
        &mut summary.total_file_bytes,
        trailer.file_bytes,
        "file bytes",
        path,
    )?;
    accumulate_catalog_preflight_total(
        &mut summary.total_footer_metadata_bytes,
        trailer.footer_metadata_bytes,
        "footer metadata bytes",
        path,
    )?;
    accumulate_catalog_preflight_total(&mut summary.total_rows, metadata.rows, "rows", path)?;
    accumulate_catalog_preflight_total(
        &mut summary.total_row_groups,
        metadata.row_groups,
        "row groups",
        path,
    )?;
    accumulate_catalog_preflight_total(
        &mut summary.total_uncompressed_bytes,
        metadata.uncompressed_bytes,
        "uncompressed bytes",
        path,
    )?;
    accumulate_catalog_preflight_total(
        &mut summary.total_inventory_bytes,
        inventory_bytes_u64,
        "inventory bytes",
        path,
    )?;
    summary.total_accounted_bytes = accounted_before_metadata
        .checked_add(metadata.uncompressed_bytes)
        .context("catalog Parquet post-metadata accounted byte total overflow")?;
    work_budget.verify_decoded_bytes(summary.total_accounted_bytes, stage)?;
    let is_instrument_metadata = relative_path.starts_with(Path::new("data").join("instruments"));
    if !is_instrument_metadata {
        accumulate_catalog_preflight_total(
            &mut summary.market_data.rows,
            metadata.rows,
            "market-data rows",
            path,
        )?;
        accumulate_catalog_preflight_total(
            &mut summary.market_data.row_groups,
            metadata.row_groups,
            "market-data row groups",
            path,
        )?;
    }
    guarded_operation_outcome(work_budget, stage, || {
        summary
            .files
            .try_reserve_exact(1)
            .map_err(|error| anyhow::anyhow!("reserve catalog Parquet inventory record: {error}"))
    })??;
    let mut relative_path_buf = PathBuf::new();
    relative_path_buf
        .try_reserve(relative_path.as_os_str().len())
        .context("reserve catalog Parquet relative path")?;
    relative_path_buf.push(relative_path);
    summary.files.push(NtCatalogParquetFilePreflight {
        relative_path: relative_path_buf,
        file_bytes: trailer.file_bytes,
        footer_metadata_bytes: trailer.footer_metadata_bytes,
        rows: metadata.rows,
        row_groups: metadata.row_groups,
        uncompressed_bytes: metadata.uncompressed_bytes,
        is_instrument_metadata,
    });
    verify_catalog_preflight_totals(summary, work_budget, stage)
}

fn accumulate_catalog_preflight_total(
    total: &mut u64,
    value: u64,
    label: &str,
    path: &Path,
) -> Result<()> {
    *total = total.checked_add(value).with_context(|| {
        format!(
            "catalog Parquet {label} total overflow at {}",
            path.display()
        )
    })?;
    Ok(())
}

fn verify_catalog_preflight_totals(
    summary: &NtCatalogPreflightSummary,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let recomputed_accounted_bytes = summary
        .total_file_bytes
        .checked_add(summary.total_footer_metadata_bytes)
        .and_then(|total| total.checked_add(summary.total_uncompressed_bytes))
        .and_then(|total| total.checked_add(summary.total_inventory_bytes))
        .context("catalog Parquet recomputed accounted byte total overflow")?;
    ensure!(
        summary.total_accounted_bytes == recomputed_accounted_bytes,
        "catalog Parquet accounted byte total {} disagrees with recomputed physical+footer+uncompressed+inventory total {recomputed_accounted_bytes}",
        summary.total_accounted_bytes
    );
    work_budget.verify_source_rows(summary.total_rows, stage)?;
    work_budget.verify_actual_row_groups(summary.total_row_groups, stage)?;
    work_budget.verify_decoded_bytes(summary.total_footer_metadata_bytes, stage)?;
    work_budget.verify_decoded_bytes(summary.total_uncompressed_bytes, stage)?;
    work_budget.verify_decoded_bytes(summary.total_inventory_bytes, stage)?;
    work_budget.verify_decoded_bytes(summary.total_accounted_bytes, stage)?;
    work_budget.check_deadline(stage)
}

/// Accepted spot instrument metadata needed to build the NautilusTrader
/// `CurrencyPair`. Built from the accepted instrument-universe payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpotInstrumentSpec {
    /// NautilusTrader instrument id, such as `SYMBOL.VENUE`.
    pub nt_instrument_id: String,
    /// Venue-native raw symbol.
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

/// Instrument spec parsed from run-spec TOML and projected through NT's native
/// instrument constructors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CatalogInstrumentSpec {
    CryptoPerpetual(CryptoPerpetualInstrumentSpec),
    CryptoFuture(CryptoFutureInstrumentSpec),
    BinaryOption(BinaryOptionInstrumentSpec),
    Spot(SpotInstrumentSpec),
}

impl CatalogInstrumentSpec {
    #[cfg(test)]
    pub(crate) fn spot_mut(&mut self) -> Option<&mut SpotInstrumentSpec> {
        match self {
            Self::Spot(spec) => Some(spec),
            Self::CryptoPerpetual(_) | Self::CryptoFuture(_) | Self::BinaryOption(_) => None,
        }
    }
}

/// TOML discriminator for an NT [`CryptoPerpetual`] instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CryptoPerpetualInstrumentKind {
    CryptoPerpetual,
}

/// Accepted crypto perpetual metadata needed to build NT's
/// [`CryptoPerpetual`] instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoPerpetualInstrumentSpec {
    pub instrument_kind: CryptoPerpetualInstrumentKind,
    pub nt_instrument_id: String,
    pub raw_symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub settlement_currency: String,
    pub is_inverse: bool,
    pub price_increment: String,
    pub size_increment: String,
    pub min_quantity: String,
    pub max_quantity: String,
    pub min_notional: String,
    pub max_notional: String,
    pub multiplier: Option<String>,
    pub lot_size: Option<String>,
    pub max_price: Option<String>,
    pub min_price: Option<String>,
    pub margin_init: Option<String>,
    pub margin_maint: Option<String>,
    pub maker_fee: Option<String>,
    pub taker_fee: Option<String>,
}

/// TOML discriminator for an NT [`CryptoFuture`] instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CryptoFutureInstrumentKind {
    CryptoFuture,
}

/// Accepted crypto future metadata needed to build NT's [`CryptoFuture`]
/// instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CryptoFutureInstrumentSpec {
    pub instrument_kind: CryptoFutureInstrumentKind,
    pub nt_instrument_id: String,
    pub raw_symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub settlement_currency: String,
    pub is_inverse: bool,
    pub activation_time_nanos: u64,
    pub expiration_time_nanos: u64,
    pub price_increment: String,
    pub size_increment: String,
    pub min_quantity: String,
    pub max_quantity: String,
    pub min_notional: String,
    pub max_notional: String,
    pub multiplier: Option<String>,
    pub lot_size: Option<String>,
    pub max_price: Option<String>,
    pub min_price: Option<String>,
    pub margin_init: Option<String>,
    pub margin_maint: Option<String>,
    pub maker_fee: Option<String>,
    pub taker_fee: Option<String>,
}

/// TOML discriminator for an NT [`BinaryOption`] instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryOptionInstrumentKind {
    BinaryOption,
}

/// Accepted binary-option metadata needed to build NT's [`BinaryOption`]
/// instrument.
///
/// Prediction-market archives (emitted by the parquet event-stream and
/// JSONL/tar snapshot adapters) carry an outcome-scoped binary contract rather
/// than a base/quote pair: one settlement currency holds the contract value and
/// the activation/expiration window bounds the resolvable epoch. Every field is
/// a decimal/identifier string parsed exactly like the other specs parse
/// theirs (fail-loud, never a panic); `price_precision`/`size_precision` are
/// derived from the parsed increments only, per the module's
/// single-source-of-precision rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOptionInstrumentSpec {
    pub instrument_kind: BinaryOptionInstrumentKind,
    pub nt_instrument_id: String,
    pub raw_symbol: String,
    /// NT [`AssetClass`] code, for example `ALTERNATIVE`.
    pub asset_class: String,
    /// Settlement (and quote) currency code, for example `USDC`.
    pub currency: String,
    pub activation_time_nanos: u64,
    pub expiration_time_nanos: u64,
    pub price_increment: String,
    pub size_increment: String,
    pub outcome: Option<String>,
    pub description: Option<String>,
    pub max_quantity: Option<String>,
    pub min_quantity: Option<String>,
    pub max_notional: Option<String>,
    pub min_notional: Option<String>,
    pub max_price: Option<String>,
    pub min_price: Option<String>,
    pub margin_init: Option<String>,
    pub margin_maint: Option<String>,
    pub maker_fee: Option<String>,
    pub taker_fee: Option<String>,
}

/// A source of accepted metadata that can build one NT instrument.
pub trait CatalogInstrumentSpecSource {
    /// Build the native NT instrument variant for this spec.
    ///
    /// # Errors
    ///
    /// Returns an error if any field fails to parse or violates NT instrument
    /// correctness checks.
    fn build_instrument_any(&self) -> Result<InstrumentAny>;
}

impl CatalogInstrumentSpecSource for SpotInstrumentSpec {
    fn build_instrument_any(&self) -> Result<InstrumentAny> {
        Ok(InstrumentAny::CurrencyPair(build_currency_pair(self)?))
    }
}

impl CatalogInstrumentSpecSource for BinaryOptionInstrumentSpec {
    fn build_instrument_any(&self) -> Result<InstrumentAny> {
        Ok(InstrumentAny::BinaryOption(build_binary_option(self)?))
    }
}

impl CatalogInstrumentSpecSource for CatalogInstrumentSpec {
    fn build_instrument_any(&self) -> Result<InstrumentAny> {
        match self {
            Self::Spot(spec) => spec.build_instrument_any(),
            Self::CryptoPerpetual(spec) => Ok(InstrumentAny::CryptoPerpetual(
                build_crypto_perpetual(spec)?,
            )),
            Self::CryptoFuture(spec) => Ok(InstrumentAny::CryptoFuture(build_crypto_future(spec)?)),
            Self::BinaryOption(spec) => Ok(InstrumentAny::BinaryOption(build_binary_option(spec)?)),
        }
    }
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

/// Build the NautilusTrader `CurrencyPair` from accepted instrument metadata.
///
/// Every NautilusTrader constructor on this path is routed through its checked
/// (`*_checked`) variant so malformed accepted metadata surfaces as an error,
/// never a panic.
///
/// # Errors
///
/// Returns an error if any field fails to parse or fails NautilusTrader's
/// instrument correctness checks.
pub fn build_currency_pair(spec: &SpotInstrumentSpec) -> Result<CurrencyPair> {
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let raw_symbol = Symbol::new_checked(&spec.raw_symbol)
        .map_err(|error| anyhow::anyhow!("invalid raw_symbol {:?}: {error}", spec.raw_symbol))?;
    let base_currency = parse_venue_currency(&spec.base_currency, "base_currency")?;
    let quote_currency = parse_venue_currency(&spec.quote_currency, "quote_currency")?;
    let price_increment = Price::from_str(&spec.price_increment).map_err(|error| {
        anyhow::anyhow!(
            "invalid price_increment {:?}: {error}",
            spec.price_increment
        )
    })?;
    let size_increment = Quantity::from_str(&spec.size_increment).map_err(|error| {
        anyhow::anyhow!("invalid size_increment {:?}: {error}", spec.size_increment)
    })?;
    // Single source of precision: the parsed increment. Deriving precision any
    // other way (for example a decimal-string char count) can disagree with the
    // precision NautilusTrader infers from the same value — `Price::from_str`
    // even accepts scientific notation — and panic `CurrencyPair::new_checked`'s
    // precision-equality check.
    let price_precision = price_increment.precision;
    let size_precision = size_increment.precision;
    let max_quantity = Quantity::from_str(&spec.max_quantity).map_err(|error| {
        anyhow::anyhow!("invalid max_quantity {:?}: {error}", spec.max_quantity)
    })?;
    let min_quantity = Quantity::from_str(&spec.min_quantity).map_err(|error| {
        anyhow::anyhow!("invalid min_quantity {:?}: {error}", spec.min_quantity)
    })?;
    let max_notional = parse_money(&spec.max_notional, quote_currency, "max_notional")?;
    let min_notional = parse_money(&spec.min_notional, quote_currency, "min_notional")?;

    CurrencyPair::new_checked(
        instrument_id,
        raw_symbol,
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
        Some(max_notional),
        Some(min_notional),
        None,
        None,
        None,
        None,
        None,
        None,
        None, // tick_scheme (NT bump): not populated by bolt
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid currency pair for {:?}: {error}",
            spec.nt_instrument_id
        )
    })
}

fn parse_instrument_id(value: &str) -> Result<InstrumentId> {
    InstrumentId::from_str(value).with_context(|| format!("invalid nt_instrument_id {value:?}"))
}

fn parse_raw_symbol(value: &str) -> Result<Symbol> {
    Symbol::new_checked(value)
        .map_err(|error| anyhow::anyhow!("invalid raw_symbol {value:?}: {error}"))
}

fn parse_venue_currency(value: &str, label: &str) -> Result<Currency> {
    let code = value.trim();
    ensure!(!code.is_empty(), "{label} must not be empty");
    Ok(Currency::get_or_create_crypto(code))
}

fn parse_asset_class(value: &str, label: &str) -> Result<AssetClass> {
    let code = value.trim();
    ensure!(!code.is_empty(), "{label} must not be empty");
    AssetClass::from_str(code)
        .map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_optional_ustr(value: Option<&str>, label: &str) -> Result<Option<Ustr>> {
    value
        .map(|value| {
            let text = value.trim();
            ensure!(!text.is_empty(), "{label} must not be empty when present");
            Ok(Ustr::from(text))
        })
        .transpose()
}

fn parse_price(value: &str, label: &str) -> Result<Price> {
    Price::from_str(value).map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_quantity(value: &str, label: &str) -> Result<Quantity> {
    Quantity::from_str(value).map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_optional_quantity(value: Option<&str>, label: &str) -> Result<Option<Quantity>> {
    value.map(|value| parse_quantity(value, label)).transpose()
}

fn parse_optional_price(value: Option<&str>, label: &str) -> Result<Option<Price>> {
    value.map(|value| parse_price(value, label)).transpose()
}

fn parse_optional_decimal(value: Option<&str>, label: &str) -> Result<Option<Decimal>> {
    value
        .map(|value| Decimal::from_str(value).with_context(|| format!("invalid {label} {value:?}")))
        .transpose()
}

fn parse_money(value: &str, currency: Currency, label: &str) -> Result<Money> {
    let mut decimal =
        Decimal::from_str(value).with_context(|| format!("invalid {label} {value:?}"))?;
    decimal.normalize_assign();
    ensure!(
        decimal.scale() <= u32::from(currency.precision),
        "invalid {label} {value:?}: normalized scale {} exceeds {} precision {}",
        decimal.scale(),
        currency,
        currency.precision
    );
    Money::from_decimal(decimal, currency)
        .map_err(|error| anyhow::anyhow!("invalid {label} {value:?}: {error}"))
}

fn parse_optional_money(
    value: Option<&str>,
    currency: Currency,
    label: &str,
) -> Result<Option<Money>> {
    value
        .map(|value| parse_money(value, currency, label))
        .transpose()
}

fn derivative_common_fields(
    input: DerivativeCommonFieldInput<'_>,
) -> Result<DerivativeCommonFields> {
    let instrument_id = parse_instrument_id(input.nt_instrument_id)?;
    let raw_symbol = parse_raw_symbol(input.raw_symbol)?;
    let price_increment = parse_price(input.price_increment, "price_increment")?;
    let size_increment = parse_quantity(input.size_increment, "size_increment")?;
    Ok(DerivativeCommonFields {
        instrument_id,
        raw_symbol,
        price_precision: price_increment.precision,
        size_precision: size_increment.precision,
        price_increment,
        size_increment,
        min_quantity: parse_quantity(input.min_quantity, "min_quantity")?,
        max_quantity: parse_quantity(input.max_quantity, "max_quantity")?,
        min_notional: parse_money(input.min_notional, input.quote_currency, "min_notional")?,
        max_notional: parse_money(input.max_notional, input.quote_currency, "max_notional")?,
    })
}

struct DerivativeCommonFieldInput<'a> {
    nt_instrument_id: &'a str,
    raw_symbol: &'a str,
    quote_currency: Currency,
    price_increment: &'a str,
    size_increment: &'a str,
    min_quantity: &'a str,
    max_quantity: &'a str,
    min_notional: &'a str,
    max_notional: &'a str,
}

struct DerivativeCommonFields {
    instrument_id: InstrumentId,
    raw_symbol: Symbol,
    price_precision: u8,
    size_precision: u8,
    price_increment: Price,
    size_increment: Quantity,
    min_quantity: Quantity,
    max_quantity: Quantity,
    min_notional: Money,
    max_notional: Money,
}

/// Build the NautilusTrader instrument variant from accepted metadata.
///
/// # Errors
///
/// Returns an error if any field fails to parse or fails NautilusTrader's
/// instrument correctness checks.
pub fn build_catalog_instrument(spec: &CatalogInstrumentSpec) -> Result<InstrumentAny> {
    spec.build_instrument_any()
}

/// Build NT's [`CryptoPerpetual`] from accepted derivative metadata.
///
/// # Errors
///
/// Returns an error if accepted metadata cannot construct a checked NT crypto
/// perpetual.
pub fn build_crypto_perpetual(spec: &CryptoPerpetualInstrumentSpec) -> Result<CryptoPerpetual> {
    let base_currency = parse_venue_currency(&spec.base_currency, "base_currency")?;
    let quote_currency = parse_venue_currency(&spec.quote_currency, "quote_currency")?;
    let settlement_currency =
        parse_venue_currency(&spec.settlement_currency, "settlement_currency")?;
    let common = derivative_common_fields(DerivativeCommonFieldInput {
        nt_instrument_id: &spec.nt_instrument_id,
        raw_symbol: &spec.raw_symbol,
        quote_currency,
        price_increment: &spec.price_increment,
        size_increment: &spec.size_increment,
        min_quantity: &spec.min_quantity,
        max_quantity: &spec.max_quantity,
        min_notional: &spec.min_notional,
        max_notional: &spec.max_notional,
    })?;
    CryptoPerpetual::new_checked(
        common.instrument_id,
        common.raw_symbol,
        base_currency,
        quote_currency,
        settlement_currency,
        spec.is_inverse,
        common.price_precision,
        common.size_precision,
        common.price_increment,
        common.size_increment,
        parse_optional_quantity(spec.multiplier.as_deref(), "multiplier")?,
        parse_optional_quantity(spec.lot_size.as_deref(), "lot_size")?,
        Some(common.max_quantity),
        Some(common.min_quantity),
        Some(common.max_notional),
        Some(common.min_notional),
        parse_optional_price(spec.max_price.as_deref(), "max_price")?,
        parse_optional_price(spec.min_price.as_deref(), "min_price")?,
        parse_optional_decimal(spec.margin_init.as_deref(), "margin_init")?,
        parse_optional_decimal(spec.margin_maint.as_deref(), "margin_maint")?,
        parse_optional_decimal(spec.maker_fee.as_deref(), "maker_fee")?,
        parse_optional_decimal(spec.taker_fee.as_deref(), "taker_fee")?,
        None, // tick_scheme (NT bump): not populated by bolt
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid crypto perpetual for {:?}: {error}",
            spec.nt_instrument_id
        )
    })
}

/// Build NT's [`CryptoFuture`] from accepted derivative metadata.
///
/// # Errors
///
/// Returns an error if accepted metadata cannot construct a checked NT crypto
/// future.
pub fn build_crypto_future(spec: &CryptoFutureInstrumentSpec) -> Result<CryptoFuture> {
    let underlying = parse_venue_currency(&spec.base_currency, "base_currency")?;
    let quote_currency = parse_venue_currency(&spec.quote_currency, "quote_currency")?;
    let settlement_currency =
        parse_venue_currency(&spec.settlement_currency, "settlement_currency")?;
    ensure!(
        spec.activation_time_nanos < spec.expiration_time_nanos,
        "activation_time_nanos must be before expiration_time_nanos"
    );
    let common = derivative_common_fields(DerivativeCommonFieldInput {
        nt_instrument_id: &spec.nt_instrument_id,
        raw_symbol: &spec.raw_symbol,
        quote_currency,
        price_increment: &spec.price_increment,
        size_increment: &spec.size_increment,
        min_quantity: &spec.min_quantity,
        max_quantity: &spec.max_quantity,
        min_notional: &spec.min_notional,
        max_notional: &spec.max_notional,
    })?;
    CryptoFuture::new_checked(
        common.instrument_id,
        common.raw_symbol,
        underlying,
        quote_currency,
        settlement_currency,
        spec.is_inverse,
        UnixNanos::from(spec.activation_time_nanos),
        UnixNanos::from(spec.expiration_time_nanos),
        common.price_precision,
        common.size_precision,
        common.price_increment,
        common.size_increment,
        parse_optional_quantity(spec.multiplier.as_deref(), "multiplier")?,
        parse_optional_quantity(spec.lot_size.as_deref(), "lot_size")?,
        Some(common.max_quantity),
        Some(common.min_quantity),
        Some(common.max_notional),
        Some(common.min_notional),
        parse_optional_price(spec.max_price.as_deref(), "max_price")?,
        parse_optional_price(spec.min_price.as_deref(), "min_price")?,
        parse_optional_decimal(spec.margin_init.as_deref(), "margin_init")?,
        parse_optional_decimal(spec.margin_maint.as_deref(), "margin_maint")?,
        parse_optional_decimal(spec.maker_fee.as_deref(), "maker_fee")?,
        parse_optional_decimal(spec.taker_fee.as_deref(), "taker_fee")?,
        None, // tick_scheme (NT bump): not populated by bolt
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid crypto future for {:?}: {error}",
            spec.nt_instrument_id
        )
    })
}

/// Build NT's [`BinaryOption`] from accepted prediction-market metadata.
///
/// Mirrors [`build_currency_pair`]'s structure: every constructor argument is
/// parsed through a checked/fail-loud helper, and price/size precision derive
/// from the parsed increments only (the module's single-source-of-precision
/// rule). The single settlement `currency` is NautilusTrader's contract
/// currency for a binary option (it has no base/quote pair); the
/// activation/expiration window bounds the resolvable epoch.
///
/// # Errors
///
/// Returns an error if accepted metadata cannot construct a checked NT binary
/// option.
pub fn build_binary_option(spec: &BinaryOptionInstrumentSpec) -> Result<BinaryOption> {
    let instrument_id = parse_instrument_id(&spec.nt_instrument_id)?;
    let raw_symbol = parse_raw_symbol(&spec.raw_symbol)?;
    let asset_class = parse_asset_class(&spec.asset_class, "asset_class")?;
    let currency = parse_venue_currency(&spec.currency, "currency")?;
    ensure!(
        spec.activation_time_nanos < spec.expiration_time_nanos,
        "activation_time_nanos must be before expiration_time_nanos"
    );
    let price_increment = parse_price(&spec.price_increment, "price_increment")?;
    let size_increment = parse_quantity(&spec.size_increment, "size_increment")?;
    let price_precision = price_increment.precision;
    let size_precision = size_increment.precision;
    let option = BinaryOption::new_checked(
        instrument_id,
        raw_symbol,
        asset_class,
        currency,
        UnixNanos::from(spec.activation_time_nanos),
        UnixNanos::from(spec.expiration_time_nanos),
        price_precision,
        size_precision,
        price_increment,
        size_increment,
        parse_optional_ustr(spec.outcome.as_deref(), "outcome")?,
        parse_optional_ustr(spec.description.as_deref(), "description")?,
        parse_optional_quantity(spec.max_quantity.as_deref(), "max_quantity")?,
        parse_optional_quantity(spec.min_quantity.as_deref(), "min_quantity")?,
        parse_optional_money(spec.max_notional.as_deref(), currency, "max_notional")?,
        parse_optional_money(spec.min_notional.as_deref(), currency, "min_notional")?,
        parse_optional_price(spec.max_price.as_deref(), "max_price")?,
        parse_optional_price(spec.min_price.as_deref(), "min_price")?,
        parse_optional_decimal(spec.margin_init.as_deref(), "margin_init")?,
        parse_optional_decimal(spec.margin_maint.as_deref(), "margin_maint")?,
        parse_optional_decimal(spec.maker_fee.as_deref(), "maker_fee")?,
        parse_optional_decimal(spec.taker_fee.as_deref(), "taker_fee")?,
        None, // tick_scheme (NT bump): not populated by bolt
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid binary option for {:?}: {error}",
            spec.nt_instrument_id
        )
    })?;
    Ok(option)
}

/// NT `ts_event` for a canonical row: the exchange/source event instant.
///
/// Event time is the per-row ordering clock the table's `validate()` already
/// proved positive and monotonic, so a non-positive value here is an internal
/// invariant breach — fail loud, never emit 0. This is the single owner of the
/// canonical-event-time → NT `UnixNanos` conversion for every data family, so
/// the projection seams and the runner's read-back/window gates cannot drift
/// into separate derivations (NO DUAL PATHS).
pub(crate) fn ts_event_nanos(event_time: i64, label: &str) -> Result<UnixNanos> {
    let nanos = u64::try_from(event_time)
        .with_context(|| format!("{label}: negative event time {event_time}"))?;
    ensure!(nanos > 0, "{label}: non-positive event time {event_time}");
    Ok(UnixNanos::from(nanos))
}

/// NT `ts_init` for a canonical row: when the data became available to the
/// system.
///
/// Source order is `availability_time` (the source's own availability instant)
/// when present, else `capture_time` (worker receipt). NT replays and windows
/// by `ts_init` (`HasTsInit`), so this must reflect receipt order, never the
/// exchange event clock. This NEVER falls back to event time or 0: if
/// `availability_time` is `Some` it must be valid; if it is `None`,
/// `capture_time` must be valid; otherwise fail loud so a missing receipt clock
/// can never silently become `ts_init=0` or be conflated with the event clock.
/// This is the single owner of the canonical-receipt-time → NT `UnixNanos`
/// derivation: the projection seams AND the runner's read-back/window gates call
/// it, so there is exactly one place that decides the `ts_init` precedence.
pub(crate) fn ts_init_nanos(
    availability_time: Option<i64>,
    capture_time: i64,
    label: &str,
) -> Result<UnixNanos> {
    let (raw, field) = match availability_time {
        Some(value) => (value, "availability_time"),
        None => (capture_time, "capture_time"),
    };
    let nanos = u64::try_from(raw)
        .with_context(|| format!("{label}: negative ts_init source {field}={raw}"))?;
    ensure!(
        nanos > 0,
        "{label}: non-positive ts_init source {field}={raw}"
    );
    Ok(UnixNanos::from(nanos))
}

fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    decimal.normalize_assign();
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Maximum decimal scale across one canonical column, after normalization so
/// trailing zeros do not count (mirrors `rescaled`'s normalize-before-check).
fn max_normalized_scale_guarded<'a>(
    values: impl Iterator<Item = &'a str>,
    label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<u32> {
    let mut max = 0u32;
    let byte_limit = work_budget
        .decoded_byte_limit()
        .map_or(usize::MAX, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        });
    for value in values {
        ensure!(
            value.len() <= byte_limit,
            "{label} value requires {} bytes, exceeding max_decoded_bytes {byte_limit}",
            value.len()
        );
        let scale = guarded_catalog_operation(work_budget, || {
            let mut decimal =
                Decimal::from_str(value).with_context(|| format!("{label} decimal {value:?}"))?;
            decimal.normalize_assign();
            Ok(decimal.scale())
        })?;
        max = max.max(scale);
    }
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    Ok(max)
}

/// Rescale a price increment to a wider decimal scale with trailing zeros.
/// The tick VALUE is unchanged; only its precision widens.
fn widened_price_increment(increment: Price, scale: u32) -> Result<Price> {
    let mut decimal = increment.as_decimal();
    decimal.rescale(scale);
    let widened = Price::from_str(&decimal.to_string()).map_err(|error| {
        anyhow::anyhow!("widen price_increment {increment} to scale {scale}: {error}")
    })?;
    ensure!(
        u32::from(widened.precision) == scale,
        "widened price_increment {widened} precision {} does not match requested scale {scale}",
        widened.precision
    );
    Ok(widened)
}

/// Rescale a size increment to a wider decimal scale with trailing zeros.
/// The step VALUE is unchanged; only its precision widens.
fn widened_size_increment(increment: Quantity, scale: u32) -> Result<Quantity> {
    let mut decimal = increment.as_decimal();
    decimal.rescale(scale);
    let widened = Quantity::from_str(&decimal.to_string()).map_err(|error| {
        anyhow::anyhow!("widen size_increment {increment} to scale {scale}: {error}")
    })?;
    ensure!(
        u32::from(widened.precision) == scale,
        "widened size_increment {widened} precision {} does not match requested scale {scale}",
        widened.precision
    );
    Ok(widened)
}

/// Read-only view over a canonical table's price-bearing and size-bearing
/// columns, used to derive the accepted data's actual decimal scale.
///
/// Each canonical family exposes its own column layout (one price/size per row
/// for trades and deltas; open/high/low/close prices plus volume for bars), so
/// the precision-widening logic depends on this view rather than on a single
/// concrete table type. Empty string cells (such as `CLEAR` delta rows) are
/// skipped by the iterator implementations so they never count toward scale.
pub(crate) trait CanonicalPriceSizeView {
    /// Iterate every non-empty price-bearing decimal string in the table.
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_>;
    /// Iterate every non-empty size-bearing decimal string in the table.
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_>;
}

impl CanonicalPriceSizeView for CanonicalTradesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.price.as_str()))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.size.as_str()))
    }
}

impl CanonicalPriceSizeView for CanonicalOrderBookDeltasTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .map(|row| row.price.as_str())
                .filter(|value| !value.is_empty()),
        )
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .map(|row| row.size.as_str())
                .filter(|value| !value.is_empty()),
        )
    }
}

impl CanonicalPriceSizeView for CanonicalBarsTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().flat_map(|row| {
            [
                row.open.as_str(),
                row.high.as_str(),
                row.low.as_str(),
                row.close.as_str(),
            ]
        }))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.volume.as_str()))
    }
}

impl CanonicalPriceSizeView for CanonicalQuotesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .flat_map(|row| [row.bid.as_str(), row.ask.as_str()]),
        )
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(
            self.rows
                .iter()
                .flat_map(|row| [row.bid_size.as_str(), row.ask_size.as_str()]),
        )
    }
}

impl CanonicalPriceSizeView for CanonicalIndexPricesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.value.as_str()))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        // An index price is a point update with no size column, so the data's
        // size scale folds to 0 and `widen_instrument_precision_for_data` keeps
        // the instrument's own size precision unchanged.
        Box::new(std::iter::empty())
    }
}

impl CanonicalPriceSizeView for CanonicalMarkPricesTable {
    fn price_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        Box::new(self.rows.iter().map(|row| row.value.as_str()))
    }
    fn size_values(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        // A mark price is a point update with no size column, so the data's
        // size scale folds to 0 and `widen_instrument_precision_for_data` keeps
        // the instrument's own size precision unchanged.
        Box::new(std::iter::empty())
    }
}

/// Widen the catalog instrument's price/size precision to the accepted
/// data's actual maximum decimal scale.
///
/// Venue instrument endpoints describe the CURRENT trading rules, but
/// historical archives can carry finer prints than today's tick size (the
/// accepted object is the authority on its own scale). The projected
/// instrument must represent the accepted data exactly, so the increments
/// are rescaled with trailing zeros (tick VALUE unchanged) and precision is
/// re-derived from the widened increments — preserving this module's
/// single-source-of-precision rule. Precision is never narrowed: data
/// coarser than the venue tick keeps the venue precision.
///
/// # Errors
///
/// Returns an error if a canonical value fails to parse, a widened increment
/// cannot be represented by NautilusTrader, or the instrument kind does not
/// support widening.
fn widen_instrument_precision_for_data_guarded(
    mut instrument: InstrumentAny,
    table: &dyn CanonicalPriceSizeView,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<InstrumentAny> {
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let data_price_scale =
        max_normalized_scale_guarded(table.price_values(), "price", work_budget)?;
    let data_size_scale = max_normalized_scale_guarded(table.size_values(), "size", work_budget)?;
    let price_scale = data_price_scale.max(u32::from(instrument.price_precision()));
    let size_scale = data_size_scale.max(u32::from(instrument.size_precision()));
    if price_scale == u32::from(instrument.price_precision())
        && size_scale == u32::from(instrument.size_precision())
    {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        return Ok(instrument);
    }
    let price_increment = widened_price_increment(instrument.price_increment(), price_scale)?;
    let size_increment = widened_size_increment(instrument.size_increment(), size_scale)?;
    match &mut instrument {
        InstrumentAny::CurrencyPair(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        InstrumentAny::CryptoPerpetual(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        InstrumentAny::CryptoFuture(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        InstrumentAny::BinaryOption(inner) => {
            inner.price_increment = price_increment;
            inner.size_increment = size_increment;
            inner.price_precision = price_increment.precision;
            inner.size_precision = size_increment.precision;
        }
        other => anyhow::bail!(
            "instrument kind for {} does not support data-derived precision widening",
            other.id()
        ),
    }
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    Ok(instrument)
}

/// Convert canonical trade rows into NautilusTrader `TradeTick`s at the
/// instrument's price/size precision.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn canonical_rows_to_trade_ticks<I: Instrument + ?Sized>(
    table: &CanonicalTradesTable,
    instrument: &I,
) -> Result<Vec<TradeTick>> {
    canonical_rows_to_trade_ticks_guarded(table, instrument, &OperatorWorkBudgetGuard::unbounded())
}

fn canonical_rows_to_trade_ticks_guarded<I: Instrument + ?Sized>(
    table: &CanonicalTradesTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<TradeTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        canonical_trade_row_materialized_bytes,
        |row| {
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
            let trade_id = TradeId::new_checked(&row.trade_id)
                .map_err(|error| anyhow::anyhow!("invalid trade_id {:?}: {error}", row.trade_id))?;
            let label = format!("trade {}", row.trade_id);
            let ts_event = ts_event_nanos(row.event_time, &label)?;
            let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
            Ok(TradeTick::new(
                instrument_id,
                price,
                size,
                aggressor,
                trade_id,
                ts_event,
                ts_init,
            ))
        },
    )
}

fn ensure_canonical_row_instrument_ids<'a>(
    instrument_id: &InstrumentId,
    row_instrument_ids: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<()> {
    let instrument_id_text = instrument_id.to_string();
    for (index, row_instrument_id) in row_instrument_ids.into_iter().enumerate() {
        let row_instrument_id = row_instrument_id
            .with_context(|| format!("row {index}: canonical row missing nt_instrument_id"))?;
        ensure!(
            instrument_id_text == row_instrument_id,
            "row {index}: instrument id {instrument_id} does not match canonical rows {row_instrument_id}"
        );
    }
    Ok(())
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
pub fn project_canonical_trades_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalTradesTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_trades_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_trades_to_catalog`].
/// `authoritative_output_root` is the exact output boundary: the projector
/// creates its private candidate as a same-filesystem sibling outside it.
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_trades_to_catalog_guarded<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalTradesTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data_guarded(instrument, table, work_budget)?;
    let instrument_id = instrument.id();
    ensure_canonical_row_instrument_ids(
        &instrument_id,
        table.rows.iter().map(|row| row.nt_instrument_id.as_deref()),
    )?;
    let ticks = canonical_rows_to_trade_ticks_guarded(table, &instrument, work_budget)?;
    let trade_count = ticks.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&ticks, None, None, None)
                    .context("write trade ticks to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
                trade_count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
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
    read_back_trade_ticks_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_trade_ticks_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<TradeTick>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments_guarded::<TradeTick>(
        &catalog,
        catalog_root,
        &instrument_ids,
        work_budget,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    guarded_catalog_operation(work_budget, || {
        catalog
            .query_typed_data::<TradeTick>(None, None, None, None, Some(files), false)
            .context("query trade ticks from catalog")
    })
}

fn validate_catalog_publication_root_shape_guarded(
    catalog_root: &Path,
    require_data_root: bool,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let metadata = guarded_catalog_operation(work_budget, || {
        fs::symlink_metadata(catalog_root)
            .with_context(|| format!("read catalog root metadata {}", catalog_root.display()))
    })?;
    ensure!(
        metadata.file_type().is_dir(),
        "catalog root {} is not a real directory",
        catalog_root.display()
    );
    let mut entries = guarded_catalog_operation(work_budget, || {
        fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))
    })?;
    let mut entry_count = 0usize;
    loop {
        let Some(entry) = guarded_catalog_operation(work_budget, || {
            entries
                .next()
                .transpose()
                .with_context(|| format!("read catalog root entry {}", catalog_root.display()))
        })?
        else {
            break;
        };
        entry_count = entry_count
            .checked_add(1)
            .context("catalog publication root entry count overflow")?;
        let entry_name = entry.file_name();
        ensure!(
            entry_count == 1 && entry_name.as_os_str() == std::ffi::OsStr::new("data"),
            "catalog root {} contains an unexpected entry {:?}; only data/ is permitted",
            catalog_root.display(),
            entry_name
        );
        let entry_metadata = guarded_catalog_operation(work_budget, || {
            fs::symlink_metadata(entry.path()).with_context(|| {
                format!("read catalog data root metadata {}", entry.path().display())
            })
        })?;
        ensure!(
            entry_metadata.file_type().is_dir(),
            "catalog data root {} is not a real directory",
            entry.path().display()
        );
    }
    ensure!(
        !require_data_root || entry_count == 1,
        "catalog root {} is missing its committed data directory",
        catalog_root.display()
    );
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    Ok(())
}

fn with_clean_catalog_root_guarded<T>(
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
    operation: impl FnOnce(&ParquetDataCatalog, &Path) -> Result<T>,
) -> Result<T> {
    let parent = catalog_root.parent().with_context(|| {
        format!(
            "catalog root {} has no parent directory",
            catalog_root.display()
        )
    })?;
    ensure!(
        catalog_root == authoritative_output_root
            || catalog_root.starts_with(authoritative_output_root),
        "catalog root {} must be within authoritative output root {}",
        catalog_root.display(),
        authoritative_output_root.display()
    );
    guarded_catalog_operation(work_budget, || {
        fs::create_dir_all(authoritative_output_root).with_context(|| {
            format!(
                "create authoritative output root {}",
                authoritative_output_root.display()
            )
        })
    })?;
    guarded_catalog_operation(work_budget, || {
        fs::create_dir_all(parent)
            .with_context(|| format!("create catalog parent {}", parent.display()))
    })?;
    let (temp_root, retained_temp_path_bytes) = external_catalog_candidate_path_guarded(
        catalog_root,
        authoritative_output_root,
        work_budget,
    )?;
    let temp_capability = create_owned_temp_directory_guarded(
        temp_root,
        retained_temp_path_bytes,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )
    .context("create identity-owned guarded catalog temp root")?;
    initialize_owned_temp_directory_receipt_guarded(
        &temp_capability,
        std::ffi::OsStr::new(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE),
        SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )
    .context("initialize catalog candidate lifecycle receipt")?;
    let temp_root = temp_capability.path();
    if let Err(error) = work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection) {
        return Err(cleanup_owned_catalog_temp(
            &temp_capability,
            error,
            work_budget,
        ));
    }

    let catalog = configured_nt_catalog(temp_capability.path(), encoding);
    let value = match operation(&catalog, temp_capability.path()) {
        Ok(value) => value,
        Err(error) => {
            return Err(cleanup_owned_catalog_temp(
                &temp_capability,
                error,
                work_budget,
            ));
        }
    };

    if let Err(error) = single_projected_data_root_guarded(temp_root, work_budget) {
        return Err(cleanup_owned_catalog_temp(
            &temp_capability,
            error,
            work_budget,
        ));
    }
    let manifest = match capture_owned_directory_manifest_guarded(
        &temp_capability,
        "data",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    ) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Err(cleanup_owned_catalog_temp(
                &temp_capability,
                error,
                work_budget,
            ));
        }
    };
    let final_data_root = match guarded_publication_child_path(
        catalog_root,
        std::ffi::OsStr::new("data"),
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    ) {
        Ok(path) => path,
        Err(error) => {
            return Err(cleanup_owned_catalog_temp(
                &temp_capability,
                error,
                work_budget,
            ));
        }
    };
    match fs::create_dir(catalog_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = guarded_catalog_operation(work_budget, || {
                fs::symlink_metadata(catalog_root).with_context(|| {
                    format!("inspect existing catalog root {}", catalog_root.display())
                })
            })?;
            ensure!(
                metadata.file_type().is_dir(),
                "existing catalog root {} is not a real directory",
                catalog_root.display()
            );
        }
        Err(error) => {
            return Err(cleanup_owned_catalog_temp(
                &temp_capability,
                anyhow::Error::new(error).context(format!(
                    "create final catalog root {}",
                    catalog_root.display()
                )),
                work_budget,
            ));
        }
    }
    if let Err(error) =
        validate_catalog_publication_root_shape_guarded(catalog_root, false, work_budget)
    {
        return Err(cleanup_owned_catalog_temp(
            &temp_capability,
            error,
            work_budget,
        ));
    }

    match stage_directory_rename_create_only_guarded(
        &temp_capability,
        &manifest,
        &final_data_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    ) {
        DirectoryStageOutcome::NotStaged(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            let validation = validate_existing_directory_manifest_identical_guarded(
                &manifest,
                &final_data_root,
                work_budget,
                OperatorWorkBudgetStage::CatalogProjection,
            )
            .with_context(|| {
                format!(
                    "existing catalog data root {} differs from the deterministic retry",
                    final_data_root.as_path().display()
                )
            })
            .and_then(|()| {
                validate_catalog_publication_root_shape_guarded(catalog_root, true, work_budget)
            });
            match validation {
                Ok(()) => finish_catalog_candidate(&temp_capability, value, work_budget),
                Err(error) => Err(cleanup_owned_catalog_temp(
                    &temp_capability,
                    error,
                    work_budget,
                )),
            }
        }
        DirectoryStageOutcome::NotStaged(error) => Err(cleanup_owned_catalog_temp(
            &temp_capability,
            anyhow::Error::new(error).context(format!(
                "create-only stage catalog data root to {}",
                final_data_root.as_path().display()
            )),
            work_budget,
        )),
        DirectoryStageOutcome::Staged => {
            let validation = validate_staged_directory_manifest_guarded(
                &manifest,
                &final_data_root,
                work_budget,
                OperatorWorkBudgetStage::CatalogProjection,
            )
            .with_context(|| {
                format!(
                    "catalog data root was staged at {} but exact validation failed; no reader authority was granted",
                    final_data_root.as_path().display()
                )
            })
            .and_then(|()| {
                validate_catalog_publication_root_shape_guarded(catalog_root, true, work_budget)
            });
            match validation {
                Ok(()) => finish_catalog_candidate(&temp_capability, value, work_budget),
                Err(error) => Err(cleanup_owned_catalog_temp(
                    &temp_capability,
                    error,
                    work_budget,
                )),
            }
        }
    }
}

fn external_catalog_candidate_path_guarded(
    catalog_root: &Path,
    authoritative_output_root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<(PathBuf, u64)> {
    let stage = OperatorWorkBudgetStage::CatalogProjection;
    work_budget.check_deadline(stage)?;
    let output_metadata = guarded_catalog_operation(work_budget, || {
        fs::symlink_metadata(authoritative_output_root).with_context(|| {
            format!(
                "inspect authoritative output root {}",
                authoritative_output_root.display()
            )
        })
    })?;
    ensure!(
        output_metadata.file_type().is_dir(),
        "authoritative output root {} is not a real directory",
        authoritative_output_root.display()
    );
    let canonical_output_root = guarded_catalog_operation(work_budget, || {
        authoritative_output_root.canonicalize().with_context(|| {
            format!(
                "canonicalize authoritative output root {}",
                authoritative_output_root.display()
            )
        })
    })?;
    let catalog_parent = catalog_root
        .parent()
        .context("catalog root has no parent directory")?;
    let canonical_catalog_parent = guarded_catalog_operation(work_budget, || {
        catalog_parent
            .canonicalize()
            .with_context(|| format!("canonicalize catalog parent {}", catalog_parent.display()))
    })?;
    let canonical_catalog_root = match guarded_catalog_operation(work_budget, || {
        fs::symlink_metadata(catalog_root)
            .with_context(|| format!("inspect catalog root {}", catalog_root.display()))
    }) {
        Ok(metadata) => {
            ensure!(
                metadata.file_type().is_dir(),
                "catalog root {} is not a real directory",
                catalog_root.display()
            );
            guarded_catalog_operation(work_budget, || {
                catalog_root.canonicalize().with_context(|| {
                    format!("canonicalize catalog root {}", catalog_root.display())
                })
            })?
        }
        Err(error)
            if error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            canonical_catalog_parent.join(
                catalog_root
                    .file_name()
                    .context("catalog root has no final path component")?,
            )
        }
        Err(error) => return Err(error),
    };
    ensure!(
        canonical_catalog_root == canonical_output_root
            || canonical_catalog_root.starts_with(&canonical_output_root),
        "catalog root {} resolves outside authoritative output root {}",
        catalog_root.display(),
        authoritative_output_root.display()
    );
    let (candidate_root, retained_path_bytes) =
        unique_temp_path_guarded(&canonical_output_root, work_budget, stage)?;
    ensure!(
        candidate_root.parent() == canonical_output_root.parent()
            && !candidate_root.starts_with(&canonical_output_root),
        "catalog candidate workspace {} must be a sibling outside authoritative output root {}",
        candidate_root.display(),
        canonical_output_root.display()
    );
    work_budget.check_deadline(stage)?;
    Ok((candidate_root, retained_path_bytes))
}

fn finish_catalog_candidate<T>(
    temp_root: &OwnedTempDirectory,
    value: T,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<T> {
    compact_owned_temp_directory_to_receipt_guarded(
        temp_root,
        std::ffi::OsStr::new(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE),
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )
    .context("compact completed catalog candidate to lifecycle receipt")?;
    Ok(value)
}

fn single_projected_data_root_guarded(
    temp_root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<PathBuf> {
    let mut entries = guarded_catalog_operation(work_budget, || {
        fs::read_dir(temp_root)
            .with_context(|| format!("read projected temp root {}", temp_root.display()))
    })?;
    let mut data_root = None;
    let mut receipt_seen = false;
    let mut entry_count = 0_u8;
    loop {
        let Some(entry) = guarded_catalog_operation(work_budget, || {
            entries
                .next()
                .transpose()
                .with_context(|| format!("read projected temp entry under {}", temp_root.display()))
        })?
        else {
            break;
        };
        entry_count = entry_count
            .checked_add(1)
            .context("projected catalog top-level entry count overflow")?;
        ensure!(
            entry_count <= 2,
            "NT projection produced more than the data directory and lifecycle receipt"
        );
        let file_type = guarded_catalog_operation(work_budget, || {
            entry
                .file_type()
                .with_context(|| format!("read projected entry type {}", entry.path().display()))
        })?;
        if entry.file_name() == "data" {
            ensure!(
                data_root.is_none() && file_type.is_dir(),
                "NT projected data root {} is not one unique directory",
                entry.path().display()
            );
            data_root = Some(entry.path());
            continue;
        }
        if entry.file_name() == SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE {
            ensure!(
                !receipt_seen && file_type.is_file(),
                "catalog lifecycle receipt {} is not one unique regular file",
                entry.path().display()
            );
            let receipt_path = entry.path();
            let (mut receipt, receipt_identity) = open_pinned_regular_file(&receipt_path)
                .with_context(|| {
                    format!("pin catalog lifecycle receipt {}", receipt_path.display())
                })?;
            ensure!(
                receipt
                    .metadata()
                    .with_context(|| {
                        format!("stat catalog lifecycle receipt {}", receipt_path.display())
                    })?
                    .len()
                    == u64::try_from(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES.len())?,
                "catalog lifecycle receipt {} has unexpected length",
                receipt_path.display()
            );
            let mut bytes = [0_u8; SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES.len()];
            guarded_catalog_operation(work_budget, || {
                receipt.read_exact(&mut bytes).with_context(|| {
                    format!("read catalog lifecycle receipt {}", receipt_path.display())
                })
            })?;
            receipt_identity.revalidate(&receipt_path, &receipt)?;
            ensure!(
                bytes == SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES,
                "catalog lifecycle receipt {} has unexpected bytes",
                receipt_path.display()
            );
            receipt_seen = true;
            continue;
        }
        anyhow::bail!(
            "NT projection produced unexpected top-level entry {:?}",
            entry.file_name()
        );
    }
    ensure!(receipt_seen, "catalog lifecycle receipt is missing");
    data_root.context("NT projection produced no top-level data directory")
}

fn cleanup_owned_catalog_temp(
    temp_root: &OwnedTempDirectory,
    error: anyhow::Error,
    work_budget: &OperatorWorkBudgetGuard,
) -> anyhow::Error {
    match compact_owned_temp_directory_to_receipt_guarded(
        temp_root,
        std::ffi::OsStr::new(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE),
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    ) {
        Ok(()) => error.context(format!(
            "compacted failed catalog candidate {} to lifecycle receipt",
            temp_root.path().display()
        )),
        Err(cleanup_error) => error.context(format!(
            "failed to compact catalog candidate {} to lifecycle receipt: {cleanup_error:#}",
            temp_root.path().display()
        )),
    }
}

/// Convert canonical order-book-delta rows into NautilusTrader `OrderBookDelta`s
/// at the instrument's price/size precision.
///
/// `CLEAR` rows become `OrderBookDelta::clear` deltas carrying the row's flags;
/// `ADD`/`UPDATE`/`DELETE` rows build a price-keyed `BookOrder` (order_id from
/// the row, `0` for L2/MBP levels) under the matching `BookAction`. Flags,
/// sequence, and timestamps are carried verbatim from the canonical rows, which
/// the table's `validate()` has already proven dense and well-formed.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision, a side/action token is unknown, or an event time is negative.
pub fn canonical_rows_to_order_book_deltas<I: Instrument + ?Sized>(
    table: &CanonicalOrderBookDeltasTable,
    instrument: &I,
) -> Result<Vec<OrderBookDelta>> {
    canonical_rows_to_order_book_deltas_guarded(
        table,
        instrument,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn canonical_rows_to_order_book_deltas_guarded<I: Instrument + ?Sized>(
    table: &CanonicalOrderBookDeltasTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<OrderBookDelta>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        delta_row_materialized_bytes,
        |row| {
            canonical_row_to_order_book_delta(instrument_id, row, price_precision, size_precision)
                .and_then(|delta| {
                    order_book_delta_at_precision(delta, price_precision, size_precision)
                })
        },
    )
}

/// Rebuild one delta at the instrument's exact price and size precision.
///
/// NautilusTrader derives Parquet batch metadata from every record and rejects
/// mixed metadata. Its `OrderBookDelta::clear` convenience constructor carries
/// zero-precision NULL order values, while the other deltas carry instrument
/// precision. Applying this one transformation to every delta keeps a catalog
/// batch homogeneous without a CLEAR-specific write path.
pub(crate) fn order_book_delta_at_precision(
    delta: OrderBookDelta,
    price_precision: u8,
    size_precision: u8,
) -> Result<OrderBookDelta> {
    let price_value = delta.order.price.as_decimal().to_string();
    let price_value = rescaled(&price_value, price_precision)
        .context("represent order-book delta price at instrument precision")?;
    let price = parse_price(&price_value, "order-book delta price")?;
    let size_value = delta.order.size.as_decimal().to_string();
    let size_value = rescaled(&size_value, size_precision)
        .context("represent order-book delta size at instrument precision")?;
    let size = parse_quantity(&size_value, "order-book delta size")?;
    OrderBookDelta::new_checked(
        delta.instrument_id,
        delta.action,
        BookOrder::new(delta.order.side, price, size, delta.order.order_id),
        delta.flags,
        delta.sequence,
        delta.ts_event,
        delta.ts_init,
    )
    .context("rebuild order-book delta at instrument precision")
}

fn canonical_row_to_order_book_delta(
    instrument_id: InstrumentId,
    row: &CanonicalOrderBookDeltaRow,
    price_precision: u8,
    size_precision: u8,
) -> Result<OrderBookDelta> {
    let label = format!("delta sequence {}", row.sequence);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    if row.action == DeltaAction::Clear.as_str() {
        // NautilusTrader's `clear` sets F_SNAPSHOT only; carry the canonical
        // row's full flag bitmask (F_SNAPSHOT required, optionally F_MBP and
        // F_LAST when the row closes a snapshot expansion), which validate()
        // has already enforced.
        let mut clear = OrderBookDelta::clear(instrument_id, row.sequence, ts_event, ts_init);
        clear.flags = row.flags;
        return Ok(clear);
    }
    let action = match row.action.as_str() {
        a if a == DeltaAction::Add.as_str() => BookAction::Add,
        a if a == DeltaAction::Update.as_str() => BookAction::Update,
        a if a == DeltaAction::Delete.as_str() => BookAction::Delete,
        other => anyhow::bail!("unknown delta action {other:?}"),
    };
    let side = match row.side.as_str() {
        s if s == DeltaSide::Buy.as_str() => OrderSide::Buy,
        s if s == DeltaSide::Sell.as_str() => OrderSide::Sell,
        other => anyhow::bail!("unknown delta side {other:?}"),
    };
    let price_str = rescaled(&row.price, price_precision)?;
    let price = Price::from_str(&price_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled price {price_str:?}: {error}"))?;
    let size_str = rescaled(&row.size, size_precision)?;
    let size = Quantity::from_str(&size_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled size {size_str:?}: {error}"))?;
    let order = BookOrder::new(side, price, size, row.order_id);
    OrderBookDelta::new_checked(
        instrument_id,
        action,
        order,
        row.flags,
        row.sequence,
        ts_event,
        ts_init,
    )
    .map_err(|error| {
        anyhow::anyhow!(
            "invalid order book delta at sequence {}: {error}",
            row.sequence
        )
    })
}

/// Project a canonical order-book-delta table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `OrderBookDelta` projection. NautilusTrader writes its
/// native `data/order_book_deltas/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_order_book_deltas_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalOrderBookDeltasTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_order_book_deltas_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_order_book_deltas_to_catalog`].
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_order_book_deltas_to_catalog_guarded<
    S: CatalogInstrumentSpecSource + ?Sized,
>(
    table: &CanonicalOrderBookDeltasTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data_guarded(instrument, table, work_budget)?;
    let instrument_id = instrument.id();
    ensure_canonical_row_instrument_ids(
        &instrument_id,
        table.rows.iter().map(|row| row.nt_instrument_id.as_deref()),
    )?;
    let deltas = canonical_rows_to_order_book_deltas_guarded(table, &instrument, work_budget)?;
    let delta_count = deltas.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&deltas, None, None, None)
                    .context("write order book deltas to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
                trade_count: delta_count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
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
    read_back_order_book_deltas_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_order_book_deltas_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<OrderBookDelta>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments_guarded::<OrderBookDelta>(
        &catalog,
        catalog_root,
        &instrument_ids,
        work_budget,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    guarded_catalog_operation(work_budget, || {
        catalog
            .query_typed_data::<OrderBookDelta>(None, None, None, None, Some(files), false)
            .context("query order book deltas from catalog")
    })
}

/// Convert canonical top-of-book quote rows into NautilusTrader `QuoteTick`s at
/// the instrument's price/size precision.
///
/// NT example strategies enter from `on_quote` (see the strategy examples at
/// `crates/.../strategy` @6e059dc); replaying `QuoteTick` data will drive
/// strategy `on_quote`. Keep the reference/non-traded instrument_id boundary
/// explicit at the run-spec layer (the `instrument_spec` keying at
/// `resolve_instrument_spec`): a quote on a reference instrument feeds signals,
/// a quote on the traded instrument can trigger entries.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the instrument
/// precision.
pub fn canonical_rows_to_quote_ticks<I: Instrument + ?Sized>(
    table: &CanonicalQuotesTable,
    instrument: &I,
) -> Result<Vec<QuoteTick>> {
    canonical_rows_to_quote_ticks_guarded(table, instrument, &OperatorWorkBudgetGuard::unbounded())
}

fn canonical_rows_to_quote_ticks_guarded<I: Instrument + ?Sized>(
    table: &CanonicalQuotesTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<QuoteTick>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        quote_row_materialized_bytes,
        |row| canonical_row_to_quote_tick(instrument_id, row, price_precision, size_precision),
    )
}

fn canonical_row_to_quote_tick(
    instrument_id: InstrumentId,
    row: &CanonicalQuoteRow,
    price_precision: u8,
    size_precision: u8,
) -> Result<QuoteTick> {
    let label = match row.source_sequence.as_deref() {
        Some(sequence) => format!("quote {sequence}"),
        None => format!("quote {}", row.event_time),
    };
    let price_at = |value: &str, name: &str| -> Result<Price> {
        let rescaled = rescaled(value, price_precision)?;
        Price::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid rescaled {name} {rescaled:?}: {error}"))
    };
    let size_at = |value: &str, name: &str| -> Result<Quantity> {
        let rescaled = rescaled(value, size_precision)?;
        Quantity::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid rescaled {name} {rescaled:?}: {error}"))
    };
    let bid_price = price_at(&row.bid, "bid")?;
    let ask_price = price_at(&row.ask, "ask")?;
    let bid_size = size_at(&row.bid_size, "bid_size")?;
    let ask_size = size_at(&row.ask_size, "ask_size")?;
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    // `QuoteTick::new` panics only on precision inequality (bid vs ask price, or
    // bid vs ask size). Both prices rescale to the SAME instrument price
    // precision and both sizes to the SAME size precision, so equality holds;
    // the canonical table's spread validation has already proven both sides carry
    // a price. This mirrors the `TradeTick::new` template choice (rescaling
    // guarantees the invariant), so no `_checked` branch is needed here.
    Ok(QuoteTick::new(
        instrument_id,
        bid_price,
        ask_price,
        bid_size,
        ask_size,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical top-of-book quote table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `QuoteTick` projection. NautilusTrader writes its native
/// `data/quotes/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_quotes_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalQuotesTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_quotes_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_quotes_to_catalog`].
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_quotes_to_catalog_guarded<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalQuotesTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data_guarded(instrument, table, work_budget)?;
    let instrument_id = instrument.id();
    ensure_canonical_row_instrument_ids(
        &instrument_id,
        table.rows.iter().map(|row| row.nt_instrument_id.as_deref()),
    )?;
    let ticks = canonical_rows_to_quote_ticks_guarded(table, &instrument, work_budget)?;
    let quote_count = ticks.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&ticks, None, None, None)
                    .context("write quote ticks to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_QUOTE_TICK.to_string(),
                trade_count: quote_count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `QuoteTick` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_quotes(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<QuoteTick>> {
    read_back_quotes_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_quotes_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<QuoteTick>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments_guarded::<QuoteTick>(
        &catalog,
        catalog_root,
        &instrument_ids,
        work_budget,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    guarded_catalog_operation(work_budget, || {
        catalog
            .query_typed_data::<QuoteTick>(None, None, None, None, Some(files), false)
            .context("query quote ticks from catalog")
    })
}

/// Convert canonical index-price rows into NautilusTrader `IndexPriceUpdate`s at
/// the instrument's price precision.
///
/// An index price is a point/reference update (the NT `IndexPriceUpdate.value`
/// is a `Price`): there is no size, aggressor, or trade id. Replaying it feeds
/// signals/reference series rather than driving strategy entries directly.
/// Timestamps route through the shared S1 receipt-clock owners
/// ([`ts_event_nanos`]/[`ts_init_nanos`]) — no new derivation here.
///
/// # Errors
///
/// Returns an error if a value cannot be represented at the instrument price
/// precision, or a timestamp source is invalid.
pub fn canonical_rows_to_index_price_updates<I: Instrument + ?Sized>(
    table: &CanonicalIndexPricesTable,
    instrument: &I,
) -> Result<Vec<IndexPriceUpdate>> {
    canonical_rows_to_index_price_updates_guarded(
        table,
        instrument,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn canonical_rows_to_index_price_updates_guarded<I: Instrument + ?Sized>(
    table: &CanonicalIndexPricesTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<IndexPriceUpdate>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        point_price_row_materialized_bytes,
        |row| canonical_row_to_index_price_update(instrument_id, row, price_precision),
    )
}

fn canonical_row_to_index_price_update(
    instrument_id: InstrumentId,
    row: &CanonicalIndexPriceRow,
    price_precision: u8,
) -> Result<IndexPriceUpdate> {
    let price_str = rescaled(&row.value, price_precision)?;
    let value = Price::from_str(&price_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled value {price_str:?}: {error}"))?;
    let label = format!("index price {}", row.event_time);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    Ok(IndexPriceUpdate::new(
        instrument_id,
        value,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical index-price table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `IndexPriceUpdate` projection. NautilusTrader writes its
/// native `data/index_prices/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_index_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalIndexPricesTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_index_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_index_to_catalog`].
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_index_to_catalog_guarded<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalIndexPricesTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data_guarded(instrument, table, work_budget)?;
    let instrument_id = instrument.id();
    ensure_canonical_row_instrument_ids(
        &instrument_id,
        table.rows.iter().map(|row| row.nt_instrument_id.as_deref()),
    )?;
    let updates = canonical_rows_to_index_price_updates_guarded(table, &instrument, work_budget)?;
    let count = updates.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&updates, None, None, None)
                    .context("write index prices to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_INDEX_PRICE_UPDATE.to_string(),
                trade_count: count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `IndexPriceUpdate` data back from `catalog_root`.
///
/// `IndexPriceUpdate` is keyed by bare instrument id under
/// `data/index_prices/<id>/` exactly like trades/deltas/quotes (not bar-type
/// keyed), so this uses the file-filter path, not the bar query path.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_index(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<IndexPriceUpdate>> {
    read_back_index_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_index_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<IndexPriceUpdate>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments_guarded::<IndexPriceUpdate>(
        &catalog,
        catalog_root,
        &instrument_ids,
        work_budget,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut read_back = guarded_catalog_operation(work_budget, || {
        catalog
            .query_typed_data::<IndexPriceUpdate>(None, None, None, None, Some(files), false)
            .context("query index prices from catalog")
    })?;
    cooperative_stable_sort_by_display_guarded(
        &mut read_back,
        |row, lengths| {
            lengths.observe(&row.instrument_id)?;
            lengths.observe(&row.value.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
                .then_with(|| scratch.compare(&left.value.as_decimal(), &right.value.as_decimal()))
                .then_with(|| left.ts_init.as_u64().cmp(&right.ts_init.as_u64()))
        },
        "index-price",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    Ok(read_back)
}

/// Convert canonical mark-price rows into NautilusTrader `MarkPriceUpdate`s at
/// the instrument's price precision.
///
/// A mark price is a point/reference update (the NT `MarkPriceUpdate.value`
/// is a `Price`): there is no size, aggressor, or trade id. Replaying it feeds
/// signals/reference series rather than driving strategy entries directly.
/// Timestamps route through the shared S1 receipt-clock owners
/// ([`ts_event_nanos`]/[`ts_init_nanos`]) — no new derivation here.
///
/// # Errors
///
/// Returns an error if a value cannot be represented at the instrument price
/// precision, or a timestamp source is invalid.
pub fn canonical_rows_to_mark_price_updates<I: Instrument + ?Sized>(
    table: &CanonicalMarkPricesTable,
    instrument: &I,
) -> Result<Vec<MarkPriceUpdate>> {
    canonical_rows_to_mark_price_updates_guarded(
        table,
        instrument,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn canonical_rows_to_mark_price_updates_guarded<I: Instrument + ?Sized>(
    table: &CanonicalMarkPricesTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<MarkPriceUpdate>> {
    let instrument_id = instrument.id();
    let price_precision = instrument.price_precision();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        mark_price_row_materialized_bytes,
        |row| canonical_row_to_mark_price_update(instrument_id, row, price_precision),
    )
}

fn canonical_row_to_mark_price_update(
    instrument_id: InstrumentId,
    row: &CanonicalMarkPriceRow,
    price_precision: u8,
) -> Result<MarkPriceUpdate> {
    let price_str = rescaled(&row.value, price_precision)?;
    let value = Price::from_str(&price_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled value {price_str:?}: {error}"))?;
    let label = format!("mark price {}", row.event_time);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    Ok(MarkPriceUpdate::new(
        instrument_id,
        value,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical mark-price table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `MarkPriceUpdate` projection. NautilusTrader writes its
/// native `data/mark_prices/<instrument_id>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_mark_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalMarkPricesTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_mark_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_mark_to_catalog`].
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_mark_to_catalog_guarded<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalMarkPricesTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data_guarded(instrument, table, work_budget)?;
    let instrument_id = instrument.id();
    ensure_canonical_row_instrument_ids(
        &instrument_id,
        table.rows.iter().map(|row| row.nt_instrument_id.as_deref()),
    )?;
    let updates = canonical_rows_to_mark_price_updates_guarded(table, &instrument, work_budget)?;
    let count = updates.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&updates, None, None, None)
                    .context("write mark prices to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_MARK_PRICE_UPDATE.to_string(),
                trade_count: count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `MarkPriceUpdate` data back from `catalog_root`.
///
/// `MarkPriceUpdate` is keyed by bare instrument id under
/// `data/mark_prices/<id>/` exactly like trades/deltas/quotes (not bar-type
/// keyed), so this uses the file-filter path, not the bar query path.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_mark(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<MarkPriceUpdate>> {
    read_back_mark_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_mark_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<MarkPriceUpdate>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments_guarded::<MarkPriceUpdate>(
        &catalog,
        catalog_root,
        &instrument_ids,
        work_budget,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut read_back = guarded_catalog_operation(work_budget, || {
        catalog
            .query_typed_data::<MarkPriceUpdate>(None, None, None, None, Some(files), false)
            .context("query mark prices from catalog")
    })?;
    cooperative_stable_sort_by_display_guarded(
        &mut read_back,
        |row, lengths| {
            lengths.observe(&row.instrument_id)?;
            lengths.observe(&row.value.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
                .then_with(|| scratch.compare(&left.value.as_decimal(), &right.value.as_decimal()))
                .then_with(|| left.ts_init.as_u64().cmp(&right.ts_init.as_u64()))
        },
        "mark-price",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    Ok(read_back)
}

/// Convert canonical funding-rate rows into NautilusTrader
/// `FundingRateUpdate`s.
///
/// Funding rate `rate` is a `Decimal`, not a price, so this conversion does not
/// use instrument price precision. Timestamps route through the shared S1
/// receipt-clock owners ([`ts_event_nanos`]/[`ts_init_nanos`]).
///
/// # Errors
///
/// Returns an error if a rate cannot be parsed or a timestamp source is invalid.
pub fn canonical_rows_to_funding_rate_updates<I: Instrument + ?Sized>(
    table: &CanonicalFundingRatesTable,
    instrument: &I,
) -> Result<Vec<FundingRateUpdate>> {
    canonical_rows_to_funding_rate_updates_guarded(
        table,
        instrument,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

fn canonical_rows_to_funding_rate_updates_guarded<I: Instrument + ?Sized>(
    table: &CanonicalFundingRatesTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<FundingRateUpdate>> {
    let instrument_id = instrument.id();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        funding_rate_row_materialized_bytes,
        |row| canonical_row_to_funding_rate_update(instrument_id, row),
    )
}

fn canonical_row_to_funding_rate_update(
    instrument_id: InstrumentId,
    row: &CanonicalFundingRateRow,
) -> Result<FundingRateUpdate> {
    // Rate is preserved at its source scale by design (NOT rescaled like index/mark); see the
    // scale-faithful hash contract at `hasher.update(funding_rate.rate.to_string().as_bytes())`.
    let rate = Decimal::from_str(&row.rate)
        .map_err(|error| anyhow::anyhow!("invalid funding rate {:?}: {error}", row.rate))?;
    let label = format!("funding rate {}", row.event_time);
    let ts_event = ts_event_nanos(row.event_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    let next_funding_ns = row
        .next_funding_time
        .map(|value| {
            let nanos = u64::try_from(value)
                .with_context(|| format!("{label}: negative next_funding_time {value}"))?;
            ensure!(nanos > 0, "{label}: non-positive next_funding_time {value}");
            Ok(UnixNanos::from(nanos))
        })
        .transpose()?;
    Ok(FundingRateUpdate::new(
        instrument_id,
        rate,
        row.interval_minutes,
        next_funding_ns,
        ts_event,
        ts_init,
    ))
}

/// Project a canonical funding-rate table into a NautilusTrader
/// `ParquetDataCatalog`.
///
/// Mirrors the point-update projections: validate, build the instrument, assert
/// the instrument id matches the canonical rows, convert, refuse a dirty root,
/// then write the instrument and the `FundingRateUpdate` projection.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_funding_rates_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalFundingRatesTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_funding_rates_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_funding_rates_to_catalog`].
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_funding_rates_to_catalog_guarded<
    S: CatalogInstrumentSpecSource + ?Sized,
>(
    table: &CanonicalFundingRatesTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    let instrument_id = instrument.id();
    let instrument_id_text = instrument_id.to_string();
    verify_canonical_rows_materialization(
        &table.rows,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
        funding_rate_row_materialized_bytes,
    )?;
    for (index, row) in table.rows.iter().enumerate() {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let row_instrument_id = row
            .nt_instrument_id
            .as_deref()
            .with_context(|| format!("row {index}: canonical row missing nt_instrument_id"))?;
        ensure!(
            instrument_id_text == row_instrument_id,
            "row {index}: instrument id {instrument_id} does not match canonical rows {}",
            row_instrument_id
        );
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    let updates = canonical_rows_to_funding_rate_updates_guarded(table, &instrument, work_budget)?;
    let count = updates.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&updates, None, None, None)
                    .context("write funding rates to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_FUNDING_RATE_UPDATE.to_string(),
                trade_count: count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `FundingRateUpdate` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_funding_rates(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<FundingRateUpdate>> {
    read_back_funding_rates_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_funding_rates_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<FundingRateUpdate>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    let instrument_ids = vec![nt_instrument_id.to_string()];
    let files = catalog_files_for_instruments_guarded::<FundingRateUpdate>(
        &catalog,
        catalog_root,
        &instrument_ids,
        work_budget,
    )?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut read_back = guarded_catalog_operation(work_budget, || {
        catalog
            .query_typed_data::<FundingRateUpdate>(None, None, None, None, Some(files), false)
            .context("query funding rates from catalog")
    })?;
    cooperative_stable_sort_by_key(
        &mut read_back,
        |row| {
            (
                row.ts_event,
                row.instrument_id,
                row.rate,
                row.rate.scale(),
                row.interval,
                row.next_funding_ns,
                row.ts_init,
            )
        },
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    Ok(read_back)
}

/// Convert canonical bar rows into NautilusTrader `Bar`s under the table's
/// externally-aggregated bar type, at the instrument's price/size precision.
///
/// Each row's OHLC is parsed at the instrument's price precision and the volume
/// at its size precision; `ts_event` is the row's `close_time` (the canonical
/// close is the bar's event instant) while `ts_init` is the row's
/// `availability_time` when present, else its `capture_time` (when the bar
/// became available to the system — the clock NautilusTrader replays by). The
/// OHLC ordering invariant the table's `validate()` already enforces is
/// re-checked by NautilusTrader's `Bar::new_checked`, so any residual
/// precision-rescale edge fails loud rather than panicking.
///
/// # Errors
///
/// Returns an error if an OHLCV value cannot be represented at the instrument
/// precision, the bar specification is invalid, or a close time is negative.
pub fn canonical_rows_to_bars<I: Instrument + ?Sized>(
    table: &CanonicalBarsTable,
    instrument: &I,
) -> Result<Vec<Bar>> {
    canonical_rows_to_bars_guarded(table, instrument, &OperatorWorkBudgetGuard::unbounded())
}

fn canonical_rows_to_bars_guarded<I: Instrument + ?Sized>(
    table: &CanonicalBarsTable,
    instrument: &I,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<Bar>> {
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument_id = instrument.id();
    let spec = BarSpecification::new_checked(
        table.bar_spec.step,
        table.bar_spec.aggregation,
        PriceType::Last,
    )
    .map_err(|error| anyhow::anyhow!("invalid bar specification: {error}"))?;
    let bar_type = BarType::new(instrument_id, spec, AggregationSource::External);
    let price_precision = instrument.price_precision();
    let size_precision = instrument.size_precision();
    let bar_aggregation = table.bar_spec.aggregation.to_string();
    collect_projected_rows_guarded(
        &table.rows,
        work_budget,
        |row| bar_row_materialized_bytes(row, &bar_aggregation),
        |row| canonical_row_to_bar(bar_type, row, price_precision, size_precision),
    )
}

fn canonical_row_to_bar(
    bar_type: BarType,
    row: &CanonicalBarRow,
    price_precision: u8,
    size_precision: u8,
) -> Result<Bar> {
    let price_at = |value: &str, label: &str| -> Result<Price> {
        let rescaled = rescaled(value, price_precision)?;
        Price::from_str(&rescaled)
            .map_err(|error| anyhow::anyhow!("invalid rescaled {label} {rescaled:?}: {error}"))
    };
    let open = price_at(&row.open, "open")?;
    let high = price_at(&row.high, "high")?;
    let low = price_at(&row.low, "low")?;
    let close = price_at(&row.close, "close")?;
    let volume_str = rescaled(&row.volume, size_precision)?;
    let volume = Quantity::from_str(&volume_str)
        .map_err(|error| anyhow::anyhow!("invalid rescaled volume {volume_str:?}: {error}"))?;
    let label = format!("bar close_time {}", row.close_time);
    let ts_event = ts_event_nanos(row.close_time, &label)?;
    let ts_init = ts_init_nanos(row.availability_time, row.capture_time, &label)?;
    Bar::new_checked(bar_type, open, high, low, close, volume, ts_event, ts_init)
        .context("build bar")
}

/// Project a canonical bar table into a NautilusTrader `ParquetDataCatalog`.
///
/// Mirrors [`project_canonical_trades_to_catalog`]: validate, build the
/// instrument, widen precision to the accepted data, assert the instrument id
/// matches the canonical rows, convert, refuse a dirty root, then write the
/// instrument and the `Bar` projection. NautilusTrader writes its native
/// `data/bars/<bar_type>/...` tree below `catalog_root`.
///
/// # Errors
///
/// Returns an error if instrument construction, conversion, or catalog writes
/// fail, or if `catalog_root` is a non-empty (dirty) directory.
pub fn project_canonical_bars_to_catalog<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalBarsTable,
    spec: &S,
    catalog_root: &Path,
    encoding: &CatalogEncodingConfig,
) -> Result<CatalogProjection> {
    project_canonical_bars_to_catalog_guarded(
        table,
        spec,
        catalog_root,
        catalog_root,
        encoding,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

/// Guarded counterpart of [`project_canonical_bars_to_catalog`].
///
/// # Errors
///
/// Returns an error on validation, conversion, budget expiry, or catalog I/O.
pub fn project_canonical_bars_to_catalog_guarded<S: CatalogInstrumentSpecSource + ?Sized>(
    table: &CanonicalBarsTable,
    spec: &S,
    catalog_root: &Path,
    authoritative_output_root: &Path,
    encoding: &CatalogEncodingConfig,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CatalogProjection> {
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::CatalogProjection)?;
    let instrument = guarded_catalog_operation(work_budget, || spec.build_instrument_any())?;
    // Venue instrument metadata can be coarser than the accepted archive's
    // actual prints; widen precision to the data before binding and writing.
    let instrument = widen_instrument_precision_for_data_guarded(instrument, table, work_budget)?;
    let instrument_id = instrument.id();
    ensure_canonical_row_instrument_ids(
        &instrument_id,
        table.rows.iter().map(|row| row.nt_instrument_id.as_deref()),
    )?;
    let bars = canonical_rows_to_bars_guarded(table, &instrument, work_budget)?;
    let bar_count = bars.len();

    with_clean_catalog_root_guarded(
        catalog_root,
        authoritative_output_root,
        encoding,
        work_budget,
        |catalog, projected_root| {
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_instruments(vec![instrument])
                    .context("write instrument to catalog")
            })?;
            guarded_catalog_operation(work_budget, || {
                catalog
                    .write_to_parquet(&bars, None, None, None)
                    .context("write bars to catalog")
            })?;
            let catalog_hash = logical_catalog_hash_guarded(projected_root, work_budget)?;
            work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
            Ok(CatalogProjection {
                catalog_root: catalog_root.to_path_buf(),
                nt_instrument_id: instrument_id.to_string(),
                data_type: NT_DATA_TYPE_BAR.to_string(),
                trade_count: bar_count,
                catalog_hash,
                fidelity_class: table.fidelity_class,
            })
        },
    )
}

/// Prove the resolved NautilusTrader dependency can read the projected `Bar`
/// data back from `catalog_root`.
///
/// NautilusTrader keys the bar catalog directory by the full bar type (not the
/// bare instrument id), so this resolves files through NautilusTrader's own
/// identifier filtering (`query_typed_data` with the instrument id) rather than
/// the instrument-directory file filter used for trades and deltas.
///
/// NautilusTrader resolves that identifier by substring match against the
/// bar-type directory name, so an instrument id that is a strict prefix of
/// another could over-collect in a shared catalog. The projectors in this
/// module never produce that shape: every projection writes exactly one bar
/// type into a clean root, so each catalog holds one bar directory.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_bars(catalog_root: &Path, nt_instrument_id: &str) -> Result<Vec<Bar>> {
    read_back_bars_guarded(
        catalog_root,
        nt_instrument_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn read_back_bars_guarded(
    catalog_root: &Path,
    nt_instrument_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<Bar>> {
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        catalog_root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    guarded_catalog_operation(work_budget, || {
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
    })
}

pub(crate) fn assert_row_pair_equality_guarded<A: Debug, B: Debug>(
    label: &str,
    actual: &[A],
    expected: &[B],
    work_budget: &OperatorWorkBudgetGuard,
    mut assert_row: impl FnMut(usize, &A, &B) -> Result<()>,
) -> Result<()> {
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    ensure!(
        actual.len() == expected.len(),
        "{label} row count mismatch: actual {}, expected {}",
        actual.len(),
        expected.len()
    );
    let mut materialized_bytes = 0_u64;
    for index in 0..actual.len() {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let row_bytes = guarded_catalog_operation(work_budget, || {
            let actual_debug = format!("{:?}", actual[index]);
            let expected_debug = format!("{:?}", expected[index]);
            size_of::<A>()
                .checked_add(size_of::<B>())
                .and_then(|bytes| bytes.checked_add(actual_debug.len()))
                .and_then(|bytes| bytes.checked_add(expected_debug.len()))
                .context("catalog equality materialized byte size overflow")
        })?;
        materialized_bytes = materialized_bytes
            .checked_add(u64::try_from(row_bytes).context("catalog equality bytes do not fit u64")?)
            .context("catalog equality materialized byte total overflow")?;
        work_budget.verify_decoded_bytes(
            materialized_bytes,
            OperatorWorkBudgetStage::CatalogProjection,
        )?;
        assert_row(index, &actual[index], &expected[index])?;
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    Ok(())
}

/// Deterministic SHA-256 hex over the logical NT catalog contents.
///
/// This intentionally hashes NT-read instruments and `TradeTick` values, not
/// raw Parquet bytes or paths. Parquet writer metadata can legitimately drift
/// across NT/Arrow builds while representing identical logical catalog input.
fn for_each_catalog_hash_row_guarded<T: Debug>(
    rows: &[T],
    work_budget: &OperatorWorkBudgetGuard,
    mut update: impl FnMut(&T) -> Result<()>,
) -> Result<()> {
    let mut materialized_bytes = 0_u64;
    for row in rows {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let row_bytes = guarded_catalog_operation(work_budget, || {
            size_of::<T>()
                .checked_add(format!("{row:?}").len())
                .context("catalog hash materialized row byte size overflow")
        })?;
        materialized_bytes = materialized_bytes
            .checked_add(u64::try_from(row_bytes).context("catalog hash bytes do not fit u64")?)
            .context("catalog hash materialized byte total overflow")?;
        work_budget.verify_decoded_bytes(
            materialized_bytes,
            OperatorWorkBudgetStage::CatalogProjection,
        )?;
        update(row)?;
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    Ok(())
}

/// Hash one already sorted logical family and consume its backing allocation.
///
/// Taking the vector by value makes the live-family bound structural: the
/// caller cannot retain the previous market-data family while querying the
/// next one unless it deliberately clones it.
fn hash_owned_catalog_family_guarded<T: Debug>(
    rows: Vec<T>,
    work_budget: &OperatorWorkBudgetGuard,
    update: impl FnMut(&T) -> Result<()>,
) -> Result<()> {
    for_each_catalog_hash_row_guarded(&rows, work_budget, update)
}

pub(crate) fn logical_catalog_hash(root: &Path) -> Result<String> {
    logical_catalog_hash_guarded(root, &OperatorWorkBudgetGuard::unbounded())
}

pub(crate) fn logical_catalog_hash_guarded(
    root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<String> {
    guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
        || logical_catalog_hash_inner(root, work_budget),
    )?
}

fn logical_catalog_hash_inner(
    root: &Path,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<String> {
    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    let _catalog_preflight = preflight_nt_catalog_parquet_guarded(
        root,
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut catalog = ParquetDataCatalog::new(root, None, None, None, None);
    let mut instruments = guarded_catalog_operation(work_budget, || {
        catalog
            .query_instruments(None)
            .context("query instruments from catalog for logical hash")
    })?;
    cooperative_stable_sort_by_display_guarded(
        &mut instruments,
        |instrument, lengths| lengths.observe(&instrument.id()),
        |left, right, scratch| scratch.compare(&left.id(), &right.id()),
        "instrument",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let instrument_ids = collect_projected_rows_guarded(
        &instruments,
        work_budget,
        |instrument| {
            size_of::<InstrumentAny>()
                .checked_add(instrument.id().to_string().len())
                .context("catalog instrument id materialized byte size overflow")
        },
        |instrument| Ok(instrument.id().to_string()),
    )?;
    let mut hasher = Sha256::new();
    hasher.update(b"nautilus-logical-catalog.v1");
    hash_owned_catalog_family_guarded(instruments, work_budget, |instrument| {
        hasher.update([0u8]);
        update_instrument_hash(&mut hasher, instrument, work_budget)
    })?;
    let trade_files = catalog_files_for_instruments_guarded::<TradeTick>(
        &catalog,
        root,
        &instrument_ids,
        work_budget,
    )?;
    let mut ticks = if trade_files.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<TradeTick>(None, None, None, None, Some(trade_files), false)
                .context("query trade ticks from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_display_guarded(
        &mut ticks,
        |row, lengths| {
            lengths.observe(&row.trade_id)?;
            lengths.observe(&row.instrument_id)
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.trade_id, &right.trade_id))
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
        },
        "trade",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    hash_owned_catalog_family_guarded(ticks, work_budget, |tick| {
        hasher.update([2u8]);
        hasher.update(tick.instrument_id.to_string().as_bytes());
        hasher.update([3u8]);
        hasher.update(tick.trade_id.to_string().as_bytes());
        hasher.update([4u8]);
        hasher.update(tick.price.as_decimal().to_string().as_bytes());
        hasher.update([5u8]);
        hasher.update(tick.size.as_decimal().to_string().as_bytes());
        hasher.update([6u8]);
        hasher.update(tick.aggressor_side.to_string().as_bytes());
        hasher.update([7u8]);
        hasher.update(tick.ts_event.as_u64().to_string().as_bytes());
        hasher.update([8u8]);
        hasher.update(tick.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;
    let delta_files = catalog_files_for_instruments_guarded::<OrderBookDelta>(
        &catalog,
        root,
        &instrument_ids,
        work_budget,
    )?;
    let mut deltas = if delta_files.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<OrderBookDelta>(
                    None,
                    None,
                    None,
                    None,
                    Some(delta_files),
                    false,
                )
                .context("query order book deltas from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_display_guarded(
        &mut deltas,
        |row, lengths| {
            lengths.observe(&row.instrument_id)?;
            lengths.observe(&row.action)?;
            lengths.observe(&row.order.side)?;
            lengths.observe(&row.order.price.as_decimal())?;
            lengths.observe(&row.order.size.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
                .then_with(|| left.sequence.cmp(&right.sequence))
                .then_with(|| scratch.compare(&left.action, &right.action))
                .then_with(|| scratch.compare(&left.order.side, &right.order.side))
                .then_with(|| {
                    scratch.compare(
                        &left.order.price.as_decimal(),
                        &right.order.price.as_decimal(),
                    )
                })
                .then_with(|| {
                    scratch.compare(
                        &left.order.size.as_decimal(),
                        &right.order.size.as_decimal(),
                    )
                })
                .then_with(|| left.order.order_id.cmp(&right.order.order_id))
        },
        "order-book-delta",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    hash_owned_catalog_family_guarded(deltas, work_budget, |delta| {
        hasher.update([9u8]);
        hasher.update(delta.instrument_id.to_string().as_bytes());
        hasher.update([10u8]);
        hasher.update(delta.action.to_string().as_bytes());
        hasher.update([11u8]);
        hasher.update(delta.order.side.to_string().as_bytes());
        hasher.update([12u8]);
        hasher.update(delta.order.price.as_decimal().to_string().as_bytes());
        hasher.update([13u8]);
        hasher.update(delta.order.size.as_decimal().to_string().as_bytes());
        hasher.update([14u8]);
        hasher.update(delta.order.order_id.to_string().as_bytes());
        hasher.update([15u8]);
        hasher.update(delta.flags.to_string().as_bytes());
        hasher.update([16u8]);
        hasher.update(delta.sequence.to_string().as_bytes());
        hasher.update([17u8]);
        hasher.update(delta.ts_event.as_u64().to_string().as_bytes());
        hasher.update([18u8]);
        hasher.update(delta.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;
    // NautilusTrader keys the bar catalog directory by the full bar type, not by
    // the bare instrument id, so bars are resolved through NautilusTrader's own
    // identifier filtering (instrument ids passed to `query_typed_data`) rather
    // than the instrument-directory file filter used for trades and deltas.
    let mut bars = if instrument_ids.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<Bar>(Some(instrument_ids.clone()), None, None, None, None, true)
                .context("query bars from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_display_guarded(
        &mut bars,
        |row, lengths| {
            lengths.observe(&row.bar_type)?;
            lengths.observe(&row.open.as_decimal())?;
            lengths.observe(&row.high.as_decimal())?;
            lengths.observe(&row.low.as_decimal())?;
            lengths.observe(&row.close.as_decimal())?;
            lengths.observe(&row.volume.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.bar_type, &right.bar_type))
                .then_with(|| scratch.compare(&left.open.as_decimal(), &right.open.as_decimal()))
                .then_with(|| scratch.compare(&left.high.as_decimal(), &right.high.as_decimal()))
                .then_with(|| scratch.compare(&left.low.as_decimal(), &right.low.as_decimal()))
                .then_with(|| scratch.compare(&left.close.as_decimal(), &right.close.as_decimal()))
                .then_with(|| {
                    scratch.compare(&left.volume.as_decimal(), &right.volume.as_decimal())
                })
        },
        "bar",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    hash_owned_catalog_family_guarded(bars, work_budget, |bar| {
        hasher.update([19u8]);
        hasher.update(bar.bar_type.to_string().as_bytes());
        hasher.update([20u8]);
        hasher.update(bar.open.as_decimal().to_string().as_bytes());
        hasher.update([21u8]);
        hasher.update(bar.high.as_decimal().to_string().as_bytes());
        hasher.update([22u8]);
        hasher.update(bar.low.as_decimal().to_string().as_bytes());
        hasher.update([23u8]);
        hasher.update(bar.close.as_decimal().to_string().as_bytes());
        hasher.update([24u8]);
        hasher.update(bar.volume.as_decimal().to_string().as_bytes());
        hasher.update([25u8]);
        hasher.update(bar.ts_event.as_u64().to_string().as_bytes());
        hasher.update([26u8]);
        hasher.update(bar.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;
    let quote_files = catalog_files_for_instruments_guarded::<QuoteTick>(
        &catalog,
        root,
        &instrument_ids,
        work_budget,
    )?;
    let mut quotes = if quote_files.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<QuoteTick>(None, None, None, None, Some(quote_files), false)
                .context("query quote ticks from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_display_guarded(
        &mut quotes,
        |row, lengths| {
            lengths.observe(&row.instrument_id)?;
            lengths.observe(&row.bid_price.as_decimal())?;
            lengths.observe(&row.ask_price.as_decimal())?;
            lengths.observe(&row.bid_size.as_decimal())?;
            lengths.observe(&row.ask_size.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
                .then_with(|| {
                    scratch.compare(&left.bid_price.as_decimal(), &right.bid_price.as_decimal())
                })
                .then_with(|| {
                    scratch.compare(&left.ask_price.as_decimal(), &right.ask_price.as_decimal())
                })
                .then_with(|| {
                    scratch.compare(&left.bid_size.as_decimal(), &right.bid_size.as_decimal())
                })
                .then_with(|| {
                    scratch.compare(&left.ask_size.as_decimal(), &right.ask_size.as_decimal())
                })
        },
        "quote",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    // Quote domain tags are appended after bars, preserving logical-v1 bytes
    // for catalogs without quote data.
    hash_owned_catalog_family_guarded(quotes, work_budget, |quote| {
        hasher.update([27u8]);
        hasher.update(quote.instrument_id.to_string().as_bytes());
        hasher.update([28u8]);
        hasher.update(quote.bid_price.as_decimal().to_string().as_bytes());
        hasher.update([29u8]);
        hasher.update(quote.ask_price.as_decimal().to_string().as_bytes());
        hasher.update([30u8]);
        hasher.update(quote.bid_size.as_decimal().to_string().as_bytes());
        hasher.update([31u8]);
        hasher.update(quote.ask_size.as_decimal().to_string().as_bytes());
        hasher.update([32u8]);
        hasher.update(quote.ts_event.as_u64().to_string().as_bytes());
        hasher.update([33u8]);
        hasher.update(quote.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;
    let index_files = catalog_files_for_instruments_guarded::<IndexPriceUpdate>(
        &catalog,
        root,
        &instrument_ids,
        work_budget,
    )?;
    let mut index_prices = if index_files.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<IndexPriceUpdate>(
                    None,
                    None,
                    None,
                    None,
                    Some(index_files),
                    false,
                )
                .context("query index prices from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_display_guarded(
        &mut index_prices,
        |row, lengths| {
            lengths.observe(&row.instrument_id)?;
            lengths.observe(&row.value.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
                .then_with(|| scratch.compare(&left.value.as_decimal(), &right.value.as_decimal()))
                .then_with(|| left.ts_init.as_u64().cmp(&right.ts_init.as_u64()))
        },
        "logical index-price",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    hash_owned_catalog_family_guarded(index_prices, work_budget, |index_price| {
        hasher.update([34u8]);
        hasher.update(index_price.instrument_id.to_string().as_bytes());
        hasher.update([35u8]);
        hasher.update(index_price.value.as_decimal().to_string().as_bytes());
        hasher.update([36u8]);
        hasher.update(index_price.ts_event.as_u64().to_string().as_bytes());
        hasher.update([37u8]);
        hasher.update(index_price.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;
    let mark_files = catalog_files_for_instruments_guarded::<MarkPriceUpdate>(
        &catalog,
        root,
        &instrument_ids,
        work_budget,
    )?;
    let mut mark_prices = if mark_files.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<MarkPriceUpdate>(
                    None,
                    None,
                    None,
                    None,
                    Some(mark_files),
                    false,
                )
                .context("query mark prices from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_display_guarded(
        &mut mark_prices,
        |row, lengths| {
            lengths.observe(&row.instrument_id)?;
            lengths.observe(&row.value.as_decimal())
        },
        |left, right, scratch| {
            left.ts_event
                .as_u64()
                .cmp(&right.ts_event.as_u64())
                .then_with(|| scratch.compare(&left.instrument_id, &right.instrument_id))
                .then_with(|| scratch.compare(&left.value.as_decimal(), &right.value.as_decimal()))
                .then_with(|| left.ts_init.as_u64().cmp(&right.ts_init.as_u64()))
        },
        "logical mark-price",
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    hash_owned_catalog_family_guarded(mark_prices, work_budget, |mark_price| {
        hasher.update([38u8]);
        hasher.update(mark_price.instrument_id.to_string().as_bytes());
        hasher.update([39u8]);
        hasher.update(mark_price.value.as_decimal().to_string().as_bytes());
        hasher.update([40u8]);
        hasher.update(mark_price.ts_event.as_u64().to_string().as_bytes());
        hasher.update([41u8]);
        hasher.update(mark_price.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;
    let funding_files = catalog_files_for_instruments_guarded::<FundingRateUpdate>(
        &catalog,
        root,
        &instrument_ids,
        work_budget,
    )?;
    let mut funding_rates = if funding_files.is_empty() {
        Vec::new()
    } else {
        guarded_catalog_operation(work_budget, || {
            catalog
                .query_typed_data::<FundingRateUpdate>(
                    None,
                    None,
                    None,
                    None,
                    Some(funding_files),
                    false,
                )
                .context("query funding rates from catalog for logical hash")
        })?
    };
    cooperative_stable_sort_by_key(
        &mut funding_rates,
        |row| {
            (
                row.ts_event,
                row.instrument_id,
                row.rate,
                row.rate.scale(),
                row.interval,
                row.next_funding_ns,
                row.ts_init,
            )
        },
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;

    hash_owned_catalog_family_guarded(funding_rates, work_budget, |funding_rate| {
        hasher.update([42u8]);
        hasher.update(funding_rate.instrument_id.to_string().as_bytes());
        hasher.update([43u8]);
        // Preserve Decimal scale as part of logical-v1 identity.
        hasher.update(funding_rate.rate.to_string().as_bytes());
        hasher.update([44u8]);
        if let Some(value) = funding_rate.interval {
            hasher.update(value.to_string().as_bytes());
        } else {
            hasher.update(b"<none>");
        }
        hasher.update([45u8]);
        if let Some(value) = funding_rate.next_funding_ns {
            hasher.update(value.as_u64().to_string().as_bytes());
        } else {
            hasher.update(b"<none>");
        }
        hasher.update([46u8]);
        hasher.update(funding_rate.ts_event.as_u64().to_string().as_bytes());
        hasher.update([47u8]);
        hasher.update(funding_rate.ts_init.as_u64().to_string().as_bytes());
        Ok(())
    })?;

    work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    Ok(hex::encode(hasher.finalize()))
}

fn catalog_files_for_instruments_guarded<T: CatalogPathPrefix>(
    catalog: &ParquetDataCatalog,
    catalog_root: &Path,
    instrument_ids: &[String],
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<String>> {
    if instrument_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut safe_instrument_ids = HashSet::new();
    safe_instrument_ids
        .try_reserve(instrument_ids.len())
        .context("reserve safe catalog instrument ids")?;
    let mut instrument_id_bytes = 0_u64;
    for id in instrument_ids {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let bytes = size_of::<String>()
            .checked_add(id.len())
            .context("catalog instrument id byte size overflow")?;
        instrument_id_bytes = instrument_id_bytes
            .checked_add(
                u64::try_from(bytes).context("catalog instrument id bytes do not fit u64")?,
            )
            .context("catalog instrument id byte total overflow")?;
        work_budget.verify_decoded_bytes(
            instrument_id_bytes,
            OperatorWorkBudgetStage::CatalogProjection,
        )?;
        safe_instrument_ids.insert(urisafe_instrument_id(id));
    }
    let files = guarded_catalog_operation(work_budget, || {
        catalog
            .query_files(T::path_prefix(), None, None, None)
            .with_context(|| format!("query {} files from catalog", T::path_prefix()))
    })?;
    let mut selected = Vec::new();
    selected
        .try_reserve_exact(files.len())
        .context("reserve selected catalog files")?;
    let mut file_path_bytes = 0_u64;
    for file in &files {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let bytes = size_of::<String>()
            .checked_add(file.len())
            .context("catalog file path byte size overflow")?;
        file_path_bytes = file_path_bytes
            .checked_add(u64::try_from(bytes).context("catalog file path bytes do not fit u64")?)
            .context("catalog file path byte total overflow")?;
        work_budget
            .verify_decoded_bytes(file_path_bytes, OperatorWorkBudgetStage::CatalogProjection)?;
        let matches = file.rsplit('/').nth(1).is_some_and(|directory| {
            let decoded = urlencoding::decode(directory)
                .map(|value| value.into_owned())
                .unwrap_or_else(|_| directory.to_string());
            safe_instrument_ids.contains(&urisafe_instrument_id(&decoded))
        });
        if matches {
            selected.push(datafusion_catalog_file_path(catalog_root, file));
        }
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    Ok(selected)
}

fn datafusion_catalog_file_path(catalog_root: &Path, catalog_file: &str) -> String {
    if catalog_file.contains("://") || Path::new(catalog_file).is_absolute() {
        catalog_file.to_string()
    } else {
        catalog_root
            .join(catalog_file)
            .to_string_lossy()
            .to_string()
    }
}

fn update_hash_field(hasher: &mut Sha256, label: &str, value: &str) {
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update(value.as_bytes());
    hasher.update([0xff]);
}

fn update_optional_hash_field<T: ToString>(hasher: &mut Sha256, label: &str, value: Option<&T>) {
    match value {
        Some(value) => update_hash_field(hasher, label, &value.to_string()),
        None => update_hash_field(hasher, label, "<none>"),
    }
}

fn update_instrument_hash(
    hasher: &mut Sha256,
    instrument: &InstrumentAny,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    match instrument {
        InstrumentAny::CurrencyPair(currency_pair) => {
            update_currency_pair_hash(hasher, currency_pair)?
        }
        InstrumentAny::BinaryOption(binary_option) => {
            update_binary_option_hash(hasher, binary_option, work_budget)?
        }
        InstrumentAny::CryptoPerpetual(crypto_perpetual) => {
            update_crypto_perpetual_hash(hasher, crypto_perpetual)?
        }
        InstrumentAny::CryptoFuture(crypto_future) => {
            update_crypto_future_hash(hasher, crypto_future)?
        }
        other => {
            anyhow::bail!(
                "logical catalog hash does not support instrument type for {}",
                other.id()
            );
        }
    }
    Ok(())
}

fn update_crypto_perpetual_hash(hasher: &mut Sha256, instrument: &CryptoPerpetual) -> Result<()> {
    ensure!(
        instrument.info.is_none(),
        "logical catalog hash does not support opaque crypto perpetual info for {}",
        instrument.id
    );
    update_hash_field(hasher, "instrument.type", "crypto_perpetual");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.base_currency",
        &instrument.base_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.quote_currency",
        &instrument.quote_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.settlement_currency",
        &instrument.settlement_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.is_inverse",
        &instrument.is_inverse.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.multiplier",
        &instrument.multiplier.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.lot_size",
        &instrument.lot_size.as_decimal().to_string(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

fn update_crypto_future_hash(hasher: &mut Sha256, instrument: &CryptoFuture) -> Result<()> {
    ensure!(
        instrument.info.is_none(),
        "logical catalog hash does not support opaque crypto future info for {}",
        instrument.id
    );
    update_hash_field(hasher, "instrument.type", "crypto_future");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.underlying",
        &instrument.underlying.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.quote_currency",
        &instrument.quote_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.settlement_currency",
        &instrument.settlement_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.is_inverse",
        &instrument.is_inverse.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.activation_ns",
        &instrument.activation_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.expiration_ns",
        &instrument.expiration_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.multiplier",
        &instrument.multiplier.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.lot_size",
        &instrument.lot_size.as_decimal().to_string(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

fn update_binary_option_hash(
    hasher: &mut Sha256,
    instrument: &BinaryOption,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    update_hash_field(hasher, "instrument.type", "binary_option");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.asset_class",
        instrument.asset_class.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.currency",
        &instrument.currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.activation_ns",
        &instrument.activation_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.expiration_ns",
        &instrument.expiration_ns.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_optional_hash_field(hasher, "instrument.outcome", instrument.outcome.as_ref());
    update_optional_hash_field(
        hasher,
        "instrument.description",
        instrument.description.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_optional_params_hash(
        hasher,
        "instrument.info",
        instrument.info.as_ref(),
        work_budget,
    )?;
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

fn update_optional_params_hash(
    hasher: &mut Sha256,
    label: &str,
    value: Option<&Params>,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    let Some(params) = value else {
        update_hash_field(hasher, label, "<none>");
        return Ok(());
    };
    update_hash_field(hasher, &format!("{label}.len"), &params.len().to_string());
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(params.len())
        .context("reserve instrument params entries")?;
    let byte_limit = work_budget
        .decoded_byte_limit()
        .map_or(usize::MAX, |limit| {
            usize::try_from(limit).unwrap_or(usize::MAX)
        });
    for entry in params.iter() {
        guarded_catalog_operation(work_budget, || {
            let serialized = serde_json::to_string(entry.1)
                .context("serialize instrument params value for byte preflight")?;
            let bytes = size_of_val(&entry)
                .checked_add(entry.0.len())
                .and_then(|bytes| bytes.checked_add(serialized.len()))
                .context("instrument params materialized byte size overflow")?;
            ensure!(
                bytes <= byte_limit,
                "instrument params entry requires {bytes} bytes, exceeding max_decoded_bytes {byte_limit}"
            );
            entries.push(entry);
            Ok(())
        })?;
    }
    cooperative_stable_sort_by(
        &mut entries,
        |(left_key, _), (right_key, _)| left_key.cmp(right_key),
        work_budget,
        OperatorWorkBudgetStage::CatalogProjection,
    )?;
    let mut hash_materialized_bytes = 0_u64;
    for (key, value) in &entries {
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
        let serialized = serde_json::to_string(value)
            .context("serialize instrument params value for hash byte preflight")?;
        let bytes = size_of_val(*value)
            .checked_add(key.len())
            .and_then(|bytes| bytes.checked_add(serialized.len()))
            .context("instrument params hash byte size overflow")?;
        hash_materialized_bytes = hash_materialized_bytes
            .checked_add(
                u64::try_from(bytes).context("instrument params hash bytes do not fit u64")?,
            )
            .context("instrument params hash byte total overflow")?;
        work_budget.verify_decoded_bytes(
            hash_materialized_bytes,
            OperatorWorkBudgetStage::CatalogProjection,
        )?;
        update_hash_field(hasher, &format!("{label}.key"), key);
        update_hash_field(hasher, &format!("{label}.value"), &serialized);
        work_budget.check_deadline(OperatorWorkBudgetStage::CatalogProjection)?;
    }
    Ok(())
}

fn update_currency_pair_hash(hasher: &mut Sha256, instrument: &CurrencyPair) -> Result<()> {
    ensure!(
        instrument.info.is_none(),
        "logical catalog hash does not support opaque currency pair info for {}",
        instrument.id
    );
    update_hash_field(hasher, "instrument.type", "currency_pair");
    update_hash_field(hasher, "instrument.id", &instrument.id.to_string());
    update_hash_field(
        hasher,
        "instrument.raw_symbol",
        instrument.raw_symbol.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.base_currency",
        &instrument.base_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.quote_currency",
        &instrument.quote_currency.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_precision",
        &instrument.price_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_precision",
        &instrument.size_precision.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.price_increment",
        &instrument.price_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.size_increment",
        &instrument.size_increment.as_decimal().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.multiplier",
        &instrument.multiplier.as_decimal().to_string(),
    );
    update_optional_hash_field(hasher, "instrument.lot_size", instrument.lot_size.as_ref());
    update_optional_hash_field(
        hasher,
        "instrument.max_quantity",
        instrument.max_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_quantity",
        instrument.min_quantity.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_notional",
        instrument.max_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_notional",
        instrument.min_notional.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.max_price",
        instrument.max_price.as_ref(),
    );
    update_optional_hash_field(
        hasher,
        "instrument.min_price",
        instrument.min_price.as_ref(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_init",
        &instrument.margin_init.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.margin_maint",
        &instrument.margin_maint.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.maker_fee",
        &instrument.maker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.taker_fee",
        &instrument.taker_fee.to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_event",
        &instrument.ts_event.as_u64().to_string(),
    );
    update_hash_field(
        hasher,
        "instrument.ts_init",
        &instrument.ts_init.as_u64().to_string(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::{
        sync::{
            Arc, Barrier,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use crate::{
        canonical_market_data::CanonicalBarSpec,
        canonical_trades::{CanonicalInstrumentIdentity, normalize_sample_spot_tick_trades},
        source_proof::{
            AcceptanceMode, AcceptanceScope, AcceptedDataset, EvidenceState, FixtureType,
            IngestManifestObjectRecord, L2ReplayEvidence, LicenseScope, NtMappingStatus,
            RequiredCheck, RequiredChecks, SourceCandidateClass, SourceProofClaimLimit,
            SourceProofReport, SourceProofStatus, SourceProofUsageScope, SourceSelectionStatus,
            TimeRange, select_accepted_dataset,
        },
    };
    use nautilus_model::enums::BarAggregation;

    fn test_catalog_encoding() -> CatalogEncodingConfig {
        CatalogEncodingConfig::new(5000, 5000, CatalogCompression::Snappy)
            .expect("positive test catalog encoding")
    }

    fn assert_catalog_candidate_is_receipt_only(candidate: &Path) {
        let entries = fs::read_dir(candidate)
            .expect("read retained catalog candidate")
            .map(|entry| entry.expect("read candidate entry").file_name())
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            [std::ffi::OsString::from(
                SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE,
            )],
            "terminal catalog candidate must retain only its lifecycle receipt"
        );
        assert_eq!(
            fs::read(candidate.join(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE))
                .expect("read lifecycle receipt"),
            SOURCE_UNIVERSE_CANDIDATE_RECEIPT_BYTES
        );
    }

    #[test]
    fn configured_nt_catalog_and_row_group_plan_share_encoding_values() {
        let encoding = CatalogEncodingConfig::new(7, 3, CatalogCompression::Snappy)
            .expect("positive catalog encoding");
        let directory = tempfile::TempDir::new().expect("temp dir");

        let catalog = configured_nt_catalog(directory.path(), &encoding);

        assert_eq!(catalog.batch_size, 7);
        assert_eq!(catalog.max_row_group_size, 3);
        assert_eq!(catalog.compression, parquet::basic::Compression::SNAPPY);
        assert_eq!(
            projected_nt_market_data_row_groups([7], &encoding).expect("project row groups"),
            3
        );
    }

    #[derive(Default)]
    struct IncrementingClock {
        ticks: AtomicU64,
    }

    impl crate::operator_work_budget::OperatorWorkBudgetClock for IncrementingClock {
        fn now(&self) -> Duration {
            Duration::from_secs(self.ticks.fetch_add(1, Ordering::SeqCst))
        }
    }

    #[derive(Debug)]
    struct LogicalHashDropProbe(Arc<AtomicUsize>);

    impl Drop for LogicalHashDropProbe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn logical_hash_family_consumer_drops_owned_rows_before_return() {
        let drops = Arc::new(AtomicUsize::new(0));
        let rows = vec![
            LogicalHashDropProbe(Arc::clone(&drops)),
            LogicalHashDropProbe(Arc::clone(&drops)),
        ];

        hash_owned_catalog_family_guarded(rows, &OperatorWorkBudgetGuard::unbounded(), |_row| {
            Ok(())
        })
        .expect("consume one logical-hash family");

        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn reusable_display_scratch_preserves_decimal_logical_catalog_v1_order() {
        let left = Price::from_str("10.0").expect("left price");
        let right = Price::from_str("2.0").expect("right price");
        let mut values = vec![right.as_decimal(), left.as_decimal()];

        cooperative_stable_sort_by_display_guarded(
            &mut values,
            |value, lengths| lengths.observe(value),
            |left, right, scratch| scratch.compare(left, right),
            "test decimal",
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect("sort decimal display keys");

        assert_eq!(values, vec![left.as_decimal(), right.as_decimal()]);
        assert_ne!(
            left.cmp(&right),
            left.as_decimal()
                .to_string()
                .cmp(&right.as_decimal().to_string())
        );
    }

    #[test]
    fn reusable_display_scratch_accepts_long_valid_display_keys() {
        let long = format!("a{}", "x".repeat(4_096));
        let mut values = vec!["b".to_string(), long.clone()];

        cooperative_stable_sort_by_display_guarded(
            &mut values,
            |value, lengths| lengths.observe(value),
            |left, right, scratch| scratch.compare(left, right),
            "test long key",
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect("sort long display keys");

        assert_eq!(values, vec![long, "b".to_string()]);
    }

    #[test]
    fn display_scratch_low_budget_rejects_before_sort_allocation() {
        let long = format!("a{}", "x".repeat(4_096));
        let mut values = vec!["b".to_string(), long.clone()];
        let original = values.clone();
        let guard = OperatorWorkBudgetGuard::new(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_decoded_bytes: u64::try_from(long.len() - 1).expect("decoded byte limit"),
                    max_source_rows: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 60,
                    require_object_selection_metadata: false,
                },
            ),
        )
        .expect("construct low display-scratch budget");

        let error = cooperative_stable_sort_by_display_guarded(
            &mut values,
            |value, lengths| lengths.observe(value),
            |left, right, scratch| scratch.compare(left, right),
            "test low budget",
            &guard,
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect_err("scratch request above max_decoded_bytes must fail before sorting");

        assert!(
            error.to_string().contains("max_decoded_bytes"),
            "unexpected error: {error:#}"
        );
        assert_eq!(values, original, "rejection must occur before sorting");
    }

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

    /// Same venue spec as `spec()` but with `price_increment = "0.01"` (precision
    /// 2). Used by ts_init/capture-clock validation tests whose table data values
    /// carry 2 decimal places (e.g. "617.05"). The canonical projection path
    /// always widens instrument precision to the data before calling
    /// `canonical_rows_to_*`; tests that call the conversion function directly
    /// must supply a pre-widened instrument so the precision gate in `rescaled`
    /// does not fire before the ts_init validation under test.
    fn spec_precision2() -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            price_increment: "0.01".to_string(),
            ..spec()
        }
    }

    fn linear_perpetual_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoPerpetual(CryptoPerpetualInstrumentSpec {
            instrument_kind: CryptoPerpetualInstrumentKind::CryptoPerpetual,
            nt_instrument_id: "BTCUSDT.BYBIT".to_string(),
            raw_symbol: "BTCUSDT".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USDT".to_string(),
            settlement_currency: "USDT".to_string(),
            is_inverse: false,
            price_increment: "0.1".to_string(),
            size_increment: "0.001".to_string(),
            min_quantity: "0.001".to_string(),
            max_quantity: "1000".to_string(),
            min_notional: "5".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.1".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn linear_future_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoFuture(CryptoFutureInstrumentSpec {
            instrument_kind: CryptoFutureInstrumentKind::CryptoFuture,
            nt_instrument_id: "BTCUSDT-05JUN26.BYBIT".to_string(),
            raw_symbol: "BTCUSDT-05JUN26".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USDT".to_string(),
            settlement_currency: "USDT".to_string(),
            is_inverse: false,
            activation_time_nanos: 1_778_832_000_000_000_000,
            expiration_time_nanos: 1_780_646_400_000_000_000,
            price_increment: "0.1".to_string(),
            size_increment: "0.001".to_string(),
            min_quantity: "0.001".to_string(),
            max_quantity: "1000".to_string(),
            min_notional: "5".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.1".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn inverse_perpetual_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoPerpetual(CryptoPerpetualInstrumentSpec {
            instrument_kind: CryptoPerpetualInstrumentKind::CryptoPerpetual,
            nt_instrument_id: "BTCUSD.BYBIT".to_string(),
            raw_symbol: "BTCUSD".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USD".to_string(),
            settlement_currency: "BTC".to_string(),
            is_inverse: true,
            price_increment: "0.5".to_string(),
            size_increment: "1".to_string(),
            min_quantity: "1".to_string(),
            max_quantity: "1000000".to_string(),
            min_notional: "1".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.5".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn inverse_future_spec() -> CatalogInstrumentSpec {
        CatalogInstrumentSpec::CryptoFuture(CryptoFutureInstrumentSpec {
            instrument_kind: CryptoFutureInstrumentKind::CryptoFuture,
            nt_instrument_id: "BTCUSDM26.BYBIT".to_string(),
            raw_symbol: "BTCUSDM26".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USD".to_string(),
            settlement_currency: "BTC".to_string(),
            is_inverse: true,
            activation_time_nanos: 1_764_892_800_000_000_000,
            expiration_time_nanos: 1_781_020_800_000_000_000,
            price_increment: "0.5".to_string(),
            size_increment: "1".to_string(),
            min_quantity: "1".to_string(),
            max_quantity: "1000000".to_string(),
            min_notional: "1".to_string(),
            max_notional: "100000000".to_string(),
            multiplier: Some("1".to_string()),
            lot_size: Some("1".to_string()),
            max_price: Some("10000000".to_string()),
            min_price: Some("0.5".to_string()),
            margin_init: Some("0".to_string()),
            margin_maint: Some("0".to_string()),
            maker_fee: Some("0".to_string()),
            taker_fee: Some("0".to_string()),
        })
    }

    fn binary_option_spec() -> CatalogInstrumentSpec {
        // Optional risk and bound metadata is omitted in this minimal fixture;
        // the dedicated official-catalog round-trip test covers populated values.
        CatalogInstrumentSpec::BinaryOption(BinaryOptionInstrumentSpec {
            instrument_kind: BinaryOptionInstrumentKind::BinaryOption,
            nt_instrument_id: "YES.TESTVENUE".to_string(),
            raw_symbol: "YES".to_string(),
            asset_class: "ALTERNATIVE".to_string(),
            currency: "USDC".to_string(),
            activation_time_nanos: 1_700_000_000_000_000_000,
            expiration_time_nanos: 1_700_086_400_000_000_000,
            price_increment: "0.01".to_string(),
            size_increment: "0.001".to_string(),
            outcome: Some("Yes".to_string()),
            description: Some("Bounded binary option fixture".to_string()),
            // Distinct values so a max/min swap fails the assertion.
            max_quantity: Some("1000000".to_string()),
            min_quantity: Some("1".to_string()),
            max_notional: None,
            min_notional: None,
            max_price: None,
            min_price: None,
            margin_init: None,
            margin_maint: None,
            // Distinct values so a maker/taker swap fails the assertion.
            maker_fee: Some("0.001".to_string()),
            taker_fee: Some("0.002".to_string()),
        })
    }

    fn accepted_dataset() -> AcceptedDataset {
        let checks = RequiredChecks {
            source_access: RequiredCheck::passed("manifest"),
            license: RequiredCheck::passed("attestation"),
            schema: RequiredCheck::passed("schema"),
            time_semantics: RequiredCheck::passed("ms_to_nanos"),
            instrument_universe: RequiredCheck::passed("universe"),
            coverage: RequiredCheck::passed("manifest"),
            retention_freshness: RequiredCheck::passed("retention"),
            granularity: RequiredCheck::passed("native"),
            completeness: RequiredCheck::passed("manifest"),
            nt_mapping: RequiredCheck::passed("TradeTick"),
            cost: RequiredCheck::passed("free"),
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
        let forbidden_claims = vec!["No execution-quality claims.".to_string()];
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
            source_candidate_class: SourceCandidateClass::OfficialFree,
            source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            official_free_gap_ref: None,
            paid_vendor_gap_ref: None,
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
            license_scope: LicenseScope::Public,
            retention_ref: "https://public.bybit.com/".to_string(),
            cost_ref: "cost://free-public-archive".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            l2_replay_evidence: L2ReplayEvidence {
                order_book_delta_ref: None,
                sufficient_snapshot_cadence_ref: None,
                no_tick_size_change_universe_ref: None,
                timed_instrument_epoch_replay_ref: None,
            },
            forbidden_claims: forbidden_claims.clone(),
            claim_limits: claim_limits_for(&forbidden_claims),
            cross_market_components: Vec::new(),
            acceptance_scope: Some(AcceptanceScope {
                planned_objects: 1,
                completed_objects: 1,
                failed_objects: 0,
                skipped_objects: 0,
                accepted_bytes: object.bytes,
                selector_scope_violations: 0,
            }),
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

    fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
        claims
            .iter()
            .enumerate()
            .map(|(index, claim)| SourceProofClaimLimit {
                id: format!("claim-limit-{}", index + 1),
                severity: "blocking".to_string(),
                claim: claim.clone(),
                reason: "source fidelity does not prove this claim".to_string(),
                evidence_ref: "source-proof://fidelity-class".to_string(),
            })
            .collect()
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
        normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            SAMPLE_CSV,
            42,
            "ingest-run-test",
        )
        .unwrap()
    }

    #[test]
    fn guarded_trade_projection_expires_between_configured_row_chunks() {
        let table = canonical_table();
        let instrument = spec().build_instrument_any().expect("instrument");
        let guard = OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_source_rows: u64::MAX,
                    max_decoded_bytes: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 5,
                    require_object_selection_metadata: false,
                },
            ),
            Arc::new(IncrementingClock::default()),
        )
        .expect("guard");

        let error = canonical_rows_to_trade_ticks_guarded(&table, &instrument, &guard)
            .expect_err("projection must observe expiry within row conversion");

        assert!(
            error.to_string().contains("catalog_projection"),
            "{error:#}"
        );
    }

    #[test]
    fn guarded_row_equality_expires_between_configured_chunks() {
        let guard = OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_source_rows: u64::MAX,
                    max_decoded_bytes: u64::MAX,
                    max_projected_row_groups: u64::MAX,
                    max_wall_seconds: 4,
                    require_object_selection_metadata: false,
                },
            ),
            Arc::new(IncrementingClock::default()),
        )
        .expect("guard");
        let mut comparisons = 0_usize;

        let error = assert_row_pair_equality_guarded(
            "test rows",
            &[1_u8, 2, 3],
            &[1_u8, 2, 3],
            &guard,
            |_index, actual, expected| {
                comparisons += 1;
                ensure!(actual == expected, "row mismatch");
                Ok(())
            },
        )
        .expect_err("equality must observe expiry between row chunks");

        assert_eq!(comparisons, 1, "only the first authorized chunk may run");
        assert!(
            error.to_string().contains("catalog_projection"),
            "{error:#}"
        );
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
    fn build_currency_pair_rejects_out_of_range_notional() {
        // An out-of-range decimal must surface as an error, never a panic, on
        // the accepted-data path.
        let mut spec = spec();
        spec.max_notional = "1e40".to_string();
        assert!(build_currency_pair(&spec).is_err());
    }

    #[test]
    fn build_currency_pair_rejects_notional_beyond_currency_precision() {
        let mut spec = spec();
        spec.quote_currency = "USD".to_string();
        spec.max_notional = "100.005".to_string();
        let error = build_currency_pair(&spec)
            .expect_err("USD precision overflow must fail instead of rounding");
        assert!(
            error.to_string().contains("exceeds USD precision 2"),
            "{error}"
        );
    }

    #[test]
    fn parse_money_rejects_precision_loss_before_nt_construction() {
        let error = parse_money("100.005", Currency::USD(), "max_notional")
            .expect_err("currency-scale overflow must fail instead of rounding");
        assert!(
            error.to_string().contains("exceeds USD precision 2"),
            "{error}"
        );
    }

    #[test]
    fn parse_money_accepts_insignificant_trailing_zeroes() {
        let money = parse_money("100.000", Currency::USD(), "max_notional")
            .expect("normalized scale fits currency precision");
        assert_eq!(money, Money::from("100.00 USD"));
    }

    #[test]
    fn canonical_row_identity_guard_rejects_later_row_mismatch() {
        let instrument_id = InstrumentId::from("BNBUSDC.BYBIT");
        let error = ensure_canonical_row_instrument_ids(
            &instrument_id,
            [Some("BNBUSDC.BYBIT"), Some("ETHUSDC.BYBIT")],
        )
        .expect_err("later cross-instrument row must fail before projection");
        assert!(error.to_string().contains("row 1"), "{error}");
        assert!(
            error.to_string().contains("does not match canonical rows"),
            "{error}"
        );
    }

    #[test]
    fn canonical_row_identity_guard_rejects_later_missing_identity() {
        let instrument_id = InstrumentId::from("BNBUSDC.BYBIT");
        let error =
            ensure_canonical_row_instrument_ids(&instrument_id, [Some("BNBUSDC.BYBIT"), None])
                .expect_err("later missing identity must fail before projection");
        assert!(error.to_string().contains("row 1"), "{error}");
        assert!(
            error.to_string().contains("missing nt_instrument_id"),
            "{error}"
        );
    }

    #[test]
    fn build_currency_pair_rejects_blank_raw_symbol() {
        // A blank raw symbol must error via the checked Symbol constructor,
        // never panic.
        let mut spec = spec();
        spec.raw_symbol = String::new();
        assert!(build_currency_pair(&spec).is_err());
    }

    #[test]
    fn build_currency_pair_derives_precision_from_scientific_increment() {
        // `Price::from_str` accepts scientific notation, so precision must be
        // derived from the parsed increment (not a decimal-string char count),
        // or `CurrencyPair::new` would panic on a precision mismatch.
        let mut spec = spec();
        spec.price_increment = "1e-2".to_string();
        let instrument = build_currency_pair(&spec).expect("scientific increment");
        assert_eq!(instrument.price_precision(), 2);
    }

    #[test]
    fn canonical_rows_to_trade_ticks_rejects_invalid_trade_id() {
        // A trade id longer than NautilusTrader's 36-char id limit must error,
        // never panic, when projected to a TradeTick.
        let long_id = "x".repeat(40);
        let csv = format!(
            "id,timestamp,price,volume,side,rpi\n{long_id},1772323201665,617.2,0.3,buy,0\n"
        );
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            &csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let instrument = build_currency_pair(&spec()).expect("instrument");
        assert!(canonical_rows_to_trade_ticks(&table, &instrument).is_err());
    }

    #[test]
    fn canonical_rows_to_trade_ticks_accepts_trailing_zero_source_values() {
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,617.20,0.3000,buy,0\n";
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument)
            .expect("trailing zero source values are exact at instrument precision");
        assert_eq!(ticks[0].price, Price::from("617.2"));
        assert_eq!(ticks[0].size, Quantity::from("0.3000"));
    }

    #[test]
    fn ts_event_nanos_rejects_non_positive_event_time() {
        // Event time is the per-row ordering clock validate() proved positive, so a
        // non-positive value here is an internal invariant breach: fail loud, never 0.
        let err = ts_event_nanos(0, "trade x").unwrap_err();
        assert!(err.to_string().contains("non-positive event time"), "{err}");
        let err = ts_event_nanos(-1, "trade x").unwrap_err();
        assert!(err.to_string().contains("negative event time"), "{err}");
    }

    #[test]
    fn ts_init_nanos_uses_capture_time_when_availability_none() {
        // No availability instant -> the worker receipt clock governs ts_init.
        assert_eq!(ts_init_nanos(None, 42, "trade x").unwrap().as_u64(), 42);
    }

    #[test]
    fn ts_init_nanos_prefers_availability_time_when_some() {
        // availability_time wins over capture_time when present (source order).
        assert_eq!(ts_init_nanos(Some(7), 42, "trade x").unwrap().as_u64(), 7);
    }

    #[test]
    fn ts_init_nanos_fails_loud_when_capture_invalid_and_no_availability() {
        // No availability and a non-positive capture clock must fail loud and name
        // the offending field, never fall back to the event clock or emit 0.
        let err = ts_init_nanos(None, 0, "trade x").unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
        let err = ts_init_nanos(None, -5, "trade x").unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn ts_init_nanos_fails_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must error rather than silently
        // fall back to capture_time.
        let err = ts_init_nanos(Some(0), 42, "trade x").unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
        let err = ts_init_nanos(Some(-1), 42, "trade x").unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn trade_ticks_ts_init_uses_capture_time_when_availability_none() {
        // canonical_table() rows carry availability_time=None and capture_time=42.
        // Every projected tick must stamp ts_init=42 (the receipt clock) while
        // ts_event stays the row's event_time (the event clock is preserved).
        let table = canonical_table();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("project trades");
        assert!(!ticks.is_empty(), "fixture must produce trades");
        for (tick, row) in ticks.iter().zip(table.rows.iter()) {
            assert_eq!(row.availability_time, None);
            assert_eq!(row.capture_time, 42);
            assert_eq!(tick.ts_init.as_u64(), 42);
            assert_eq!(
                tick.ts_event.as_u64(),
                u64::try_from(row.event_time).expect("positive event_time")
            );
        }
    }

    #[test]
    fn trade_ticks_ts_init_prefers_availability_time_when_some() {
        // With a source availability instant present, ts_init follows it over the
        // capture clock, while ts_event still preserves the event clock.
        let mut table = canonical_table();
        table.rows[0].availability_time = Some(7);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("project trades");
        assert_eq!(ticks[0].ts_init.as_u64(), 7);
        assert_eq!(
            ticks[0].ts_event.as_u64(),
            u64::try_from(table.rows[0].event_time).expect("positive event_time")
        );
    }

    #[test]
    fn trade_ticks_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming the
        // capture_time field rather than silently stamping ts_init=0.
        let mut table = canonical_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_trade_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn trade_ticks_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to the
        // (valid) capture clock.
        let mut table = canonical_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_trade_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn builds_currency_pair_from_accepted_spec() {
        let instrument = build_currency_pair(&spec()).expect("build instrument");
        assert_eq!(instrument.id().to_string(), "BNBUSDC.BYBIT");
        assert_eq!(instrument.price_precision(), 1);
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn builds_crypto_perpetual_from_accepted_spec() {
        let instrument = build_catalog_instrument(&linear_perpetual_spec()).expect("instrument");
        let InstrumentAny::CryptoPerpetual(perpetual) = instrument else {
            panic!("expected CryptoPerpetual");
        };
        assert_eq!(perpetual.id().to_string(), "BTCUSDT.BYBIT");
        assert_eq!(perpetual.base_currency.to_string(), "BTC");
        assert_eq!(perpetual.quote_currency.to_string(), "USDT");
        assert_eq!(perpetual.settlement_currency.to_string(), "USDT");
        assert!(!perpetual.is_inverse);
        assert_eq!(perpetual.price_precision(), 1);
        assert_eq!(perpetual.size_precision(), 3);
    }

    #[test]
    fn builds_crypto_future_from_accepted_spec() {
        let instrument = build_catalog_instrument(&linear_future_spec()).expect("instrument");
        let InstrumentAny::CryptoFuture(future) = instrument else {
            panic!("expected CryptoFuture");
        };
        assert_eq!(future.id().to_string(), "BTCUSDT-05JUN26.BYBIT");
        assert_eq!(future.underlying.to_string(), "BTC");
        assert_eq!(future.quote_currency.to_string(), "USDT");
        assert_eq!(future.settlement_currency.to_string(), "USDT");
        assert_eq!(future.activation_ns.as_u64(), 1_778_832_000_000_000_000);
        assert_eq!(future.expiration_ns.as_u64(), 1_780_646_400_000_000_000);
        assert!(!future.is_inverse);
    }

    #[test]
    fn builds_inverse_crypto_perpetual_from_accepted_spec() {
        let instrument = build_catalog_instrument(&inverse_perpetual_spec()).expect("instrument");
        let InstrumentAny::CryptoPerpetual(perpetual) = instrument else {
            panic!("expected CryptoPerpetual");
        };
        assert_eq!(perpetual.id().to_string(), "BTCUSD.BYBIT");
        assert_eq!(perpetual.base_currency.to_string(), "BTC");
        assert_eq!(perpetual.quote_currency.to_string(), "USD");
        assert_eq!(perpetual.settlement_currency.to_string(), "BTC");
        assert!(perpetual.is_inverse);
    }

    #[test]
    fn builds_inverse_crypto_future_from_accepted_spec() {
        let instrument = build_catalog_instrument(&inverse_future_spec()).expect("instrument");
        let InstrumentAny::CryptoFuture(future) = instrument else {
            panic!("expected CryptoFuture");
        };
        assert_eq!(future.id().to_string(), "BTCUSDM26.BYBIT");
        assert_eq!(future.underlying.to_string(), "BTC");
        assert_eq!(future.quote_currency.to_string(), "USD");
        assert_eq!(future.settlement_currency.to_string(), "BTC");
        assert!(future.is_inverse);
        assert!(future.activation_ns < future.expiration_ns);
    }

    fn binary_option_inner() -> BinaryOptionInstrumentSpec {
        let CatalogInstrumentSpec::BinaryOption(spec) = binary_option_spec() else {
            panic!("expected BinaryOption fixture");
        };
        spec
    }

    #[test]
    fn builds_binary_option_from_accepted_spec() {
        let instrument = build_catalog_instrument(&binary_option_spec()).expect("instrument");
        let InstrumentAny::BinaryOption(option) = instrument else {
            panic!("expected BinaryOption");
        };
        assert_eq!(option.id().to_string(), "YES.TESTVENUE");
        assert_eq!(option.raw_symbol.to_string(), "YES");
        assert_eq!(option.asset_class, AssetClass::Alternative);
        // Binary options carry one settlement/quote currency, not a base/quote
        // pair.
        assert_eq!(option.currency.to_string(), "USDC");
        assert_eq!(option.base_currency(), None);
        assert_eq!(option.quote_currency().to_string(), "USDC");
        assert_eq!(option.settlement_currency().to_string(), "USDC");
        assert_eq!(option.activation_ns.as_u64(), 1_700_000_000_000_000_000);
        assert_eq!(option.expiration_ns.as_u64(), 1_700_086_400_000_000_000);
        // Precision derives from the increments only (single-source-of-precision).
        assert_eq!(option.price_precision(), 2);
        assert_eq!(option.size_precision(), 3);
        assert_eq!(
            option.outcome.map(|o| o.to_string()),
            Some("Yes".to_string())
        );
        assert_eq!(
            option.description.map(|d| d.to_string()),
            Some("Bounded binary option fixture".to_string())
        );
        // Distinct max/min so a quantity swap would fail.
        assert_eq!(option.max_quantity, Some(Quantity::from("1000000")));
        assert_eq!(option.min_quantity, Some(Quantity::from("1")));
        // This minimal fixture leaves the optional risk and bound metadata absent.
        assert_eq!(option.max_notional, None);
        assert_eq!(option.min_notional, None);
        assert_eq!(option.max_price(), None);
        assert_eq!(option.min_price(), None);
        assert_eq!(option.margin_init, Decimal::ZERO);
        assert_eq!(option.margin_maint, Decimal::ZERO);
        // Distinct maker/taker so a fee swap would fail.
        assert_eq!(option.maker_fee, Decimal::from_str("0.001").unwrap());
        assert_eq!(option.taker_fee, Decimal::from_str("0.002").unwrap());
    }

    #[test]
    fn build_binary_option_omits_optional_fields() {
        // Every Option<String> field absent must build a valid instrument: NT's
        // BinaryOption constructor accepts None for outcome/description, the
        // quantity/notional/price bounds, the margins, and the fees.
        let mut spec = binary_option_inner();
        spec.outcome = None;
        spec.description = None;
        spec.max_quantity = None;
        spec.min_quantity = None;
        spec.max_notional = None;
        spec.min_notional = None;
        spec.max_price = None;
        spec.min_price = None;
        spec.margin_init = None;
        spec.margin_maint = None;
        spec.maker_fee = None;
        spec.taker_fee = None;
        let option = build_binary_option(&spec).expect("optional fields default cleanly");
        assert_eq!(option.outcome, None);
        assert_eq!(option.description, None);
        assert_eq!(option.max_quantity, None);
        assert_eq!(option.min_notional, None);
    }

    #[test]
    fn build_binary_option_honours_trailing_zero_increment() {
        // Precision must agree with the increment's own precision, or NT's
        // BinaryOption precision-equality check would reject the instrument.
        let mut spec = binary_option_inner();
        spec.price_increment = "0.010".to_string();
        let option = build_binary_option(&spec).expect("trailing-zero increment");
        assert_eq!(option.price_precision(), 3);
    }

    #[test]
    fn build_binary_option_rejects_malformed_decimal() {
        let mut spec = binary_option_inner();
        spec.price_increment = "not-a-number".to_string();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_blank_raw_symbol() {
        let mut spec = binary_option_inner();
        spec.raw_symbol = String::new();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_blank_currency() {
        let mut spec = binary_option_inner();
        spec.currency = "   ".to_string();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_unknown_asset_class() {
        let mut spec = binary_option_inner();
        spec.asset_class = "NOT_AN_ASSET_CLASS".to_string();
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_expiration_not_after_activation() {
        // The resolvable epoch must be a forward-bounded window, mirroring the
        // crypto-future activation/expiration ordering check.
        let mut spec = binary_option_inner();
        spec.expiration_time_nanos = spec.activation_time_nanos;
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_out_of_range_notional() {
        let mut spec = binary_option_inner();
        spec.max_notional = Some("1e40".to_string());
        assert!(build_binary_option(&spec).is_err());
    }

    #[test]
    fn build_binary_option_rejects_notional_beyond_currency_precision() {
        let mut spec = binary_option_inner();
        spec.max_notional = Some("100.000000001".to_string());
        let error = build_binary_option(&spec)
            .expect_err("USDC precision overflow must fail instead of rounding");
        assert!(
            error.to_string().contains("exceeds USDC precision 8"),
            "{error}"
        );
    }

    #[test]
    fn binary_option_supported_fields_round_trip_through_official_catalog() {
        let mut spec = binary_option_inner();
        spec.max_notional = Some("100000".to_string());
        spec.min_notional = Some("1".to_string());
        spec.max_price = Some("1.00".to_string());
        spec.min_price = Some("0.01".to_string());
        spec.margin_init = Some("0.05".to_string());
        spec.margin_maint = Some("0.03".to_string());
        let option = build_binary_option(&spec).expect("build supported binary option fields");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        catalog
            .write_instruments(vec![InstrumentAny::BinaryOption(option.clone())])
            .expect("write binary option");
        let read_back = catalog
            .query_instruments(None)
            .expect("read binary option")
            .into_iter()
            .next()
            .expect("one binary option");

        assert_eq!(
            serde_json::to_value(read_back).expect("serialize read-back instrument"),
            serde_json::to_value(InstrumentAny::BinaryOption(option))
                .expect("serialize built instrument")
        );
    }

    // Fix 4 — parse_optional_ustr empty-when-present rejection.
    #[test]
    fn build_binary_option_rejects_empty_outcome() {
        let mut spec = binary_option_inner();
        spec.outcome = Some(String::new());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("outcome must not be empty when present"),
            "error must name the field and state the rule: {msg}"
        );
    }

    #[test]
    fn build_binary_option_rejects_whitespace_only_description() {
        let mut spec = binary_option_inner();
        spec.description = Some("   ".to_string());
        let err = build_binary_option(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("description must not be empty when present"),
            "error must name the field and state the rule: {msg}"
        );
    }

    #[test]
    fn catalog_instrument_spec_deserializes_binary_option_shape() {
        let parsed: CatalogInstrumentSpec = toml::from_str(
            r#"
instrument_kind = "binary_option"
nt_instrument_id = "YES.TESTVENUE"
raw_symbol = "YES"
asset_class = "ALTERNATIVE"
currency = "USDC"
activation_time_nanos = 1700000000000000000
expiration_time_nanos = 1700086400000000000
price_increment = "0.01"
size_increment = "0.001"
"#,
        )
        .expect("binary option spec parses");
        assert!(matches!(parsed, CatalogInstrumentSpec::BinaryOption(_)));
    }

    #[test]
    fn catalog_instrument_spec_deserializes_legacy_spot_shape() {
        let parsed: CatalogInstrumentSpec = toml::from_str(
            r#"
nt_instrument_id = "BNBUSDC.BYBIT"
raw_symbol = "BNBUSDC"
base_currency = "BNB"
quote_currency = "USDC"
price_increment = "0.1"
size_increment = "0.0001"
min_quantity = "0.0001"
max_quantity = "1400"
min_notional = "5"
max_notional = "200000"
"#,
        )
        .expect("legacy spot spec parses");
        assert!(matches!(parsed, CatalogInstrumentSpec::Spot(_)));
    }

    #[test]
    fn projects_derivative_trade_ticks_with_nt_crypto_instrument() {
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BTCUSDT".to_string(),
            venue_symbol: "BTCUSDT".to_string(),
            nt_instrument_id: "BTCUSDT.BYBIT".to_string(),
        };
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,617.20,0.3000,buy,0\n";
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_trades_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project derivative");

        assert_eq!(projection.trade_count, 1);
        assert_eq!(projection.nt_instrument_id, "BTCUSDT.BYBIT");
        let loaded = read_back_trade_ticks(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].instrument_id.to_string(), "BTCUSDT.BYBIT");
    }

    #[test]
    fn projects_and_reads_back_trade_ticks() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        assert_eq!(projection.trade_count, 3);
        assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());
        let projected_row_groups = projected_nt_market_data_row_groups(
            [u64::try_from(table.rows.len()).expect("row count fits u64")],
            &test_catalog_encoding(),
        )
        .expect("project row groups");
        let actual_metadata =
            actual_nt_market_data_metadata(dir.path()).expect("read actual Parquet metadata");
        assert_eq!(actual_metadata.rows, table.rows.len() as u64);
        assert_eq!(actual_metadata.row_groups, projected_row_groups);

        let loaded = read_back_trade_ticks(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].instrument_id.to_string(), "BNBUSDC.BYBIT");
        // 617 rescaled to price precision 1 -> 617.0
        assert_eq!(loaded[2].price, Price::from("617.0"));
    }

    fn catalog_preflight_guard(
        max_source_rows: u64,
        max_decoded_bytes: u64,
        max_projected_row_groups: u64,
    ) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::new(crate::operator_work_budget::OperatorWorkBudget::Backfill(
            crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                max_source_rows,
                max_decoded_bytes,
                max_projected_row_groups,
                max_wall_seconds: 60,
                require_object_selection_metadata: false,
            },
        ))
        .expect("catalog preflight guard")
    }

    fn catalog_parquet_envelope_with_footer(footer: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::from(*b"PAR1");
        bytes.extend_from_slice(footer);
        bytes.extend_from_slice(
            &u32::try_from(footer.len())
                .expect("test footer length fits u32")
                .to_le_bytes(),
        );
        bytes.extend_from_slice(b"PAR1");
        bytes
    }

    #[test]
    fn catalog_preflight_rejects_stray_parquet_before_nt_query() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project");
        fs::write(
            dir.path().join("stray.parquet"),
            b"NOPE\x01\x00\x00\x00PAR1",
        )
        .expect("write stray Parquet");

        let error = read_back_trade_ticks_guarded(
            dir.path(),
            "BNBUSDC.BYBIT",
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect_err("stray malformed Parquet must fail before NT query");

        assert!(
            error
                .to_string()
                .contains("Parquet header magic is not PAR1"),
            "{error:#}"
        );
    }

    #[test]
    fn catalog_preflight_rejects_oversized_instrument_footer_before_nt_query() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project");
        let stray = dir.path().join("data/instruments/stray.parquet");
        fs::write(&stray, b"PAR1\xff\xff\xff\xffPAR1").expect("write malformed instrument Parquet");

        let error = read_back_trade_ticks_guarded(
            dir.path(),
            "BNBUSDC.BYBIT",
            &OperatorWorkBudgetGuard::unbounded(),
        )
        .expect_err("oversized instrument footer must fail before NT query");

        assert!(
            error
                .to_string()
                .contains("Parquet footer metadata length 4294967295 exceeds"),
            "{error:#}"
        );
    }

    #[test]
    fn catalog_preflight_rejects_tiny_footer_with_huge_list_before_builder() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let footer = [0x19, 0xf3, 0xff, 0xff, 0xff, 0xff, 0x0f];
        fs::write(
            dir.path().join("huge-list.parquet"),
            catalog_parquet_envelope_with_footer(&footer),
        )
        .expect("write compact-Thrift list bomb");

        let error = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &catalog_preflight_guard(8, 1_024, u64::MAX),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect_err("list cardinality must fail before catalog builder allocation");

        assert!(
            format!("{error:#}").contains(
                "compact-Thrift collection cardinality 4294967295 exceeds footer byte bound 7"
            ),
            "{error:#}"
        );
    }

    #[test]
    fn instrument_preflight_rejects_nested_binary_length_before_builder() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let instrument_dir = dir.path().join("data/instruments");
        fs::create_dir_all(&instrument_dir).expect("create instrument directory");
        let footer = [0x1c, 0x18, 0x81, 0x01, 0x00, 0x00];
        fs::write(
            instrument_dir.join("nested-length.parquet"),
            catalog_parquet_envelope_with_footer(&footer),
        )
        .expect("write compact-Thrift nested binary bomb");

        let error = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &catalog_preflight_guard(8, 128, u64::MAX),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect_err("nested binary length must fail before instrument builder allocation");

        assert!(
            format!("{error:#}")
                .contains("compact-Thrift binary length 129 exceeds max_decoded_bytes 128"),
            "{error:#}"
        );
    }

    #[test]
    fn catalog_preflight_enforces_all_file_row_groups_but_reports_market_data_only() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project");
        let unbounded = OperatorWorkBudgetGuard::unbounded();
        let summary = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &unbounded,
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect("preflight catalog");

        assert!(summary.files.iter().any(|file| file.is_instrument_metadata));
        assert!(
            summary
                .files
                .iter()
                .any(|file| !file.is_instrument_metadata)
        );
        assert_eq!(summary.market_data.rows, table.rows.len() as u64);
        assert!(summary.total_rows > summary.market_data.rows);
        assert!(summary.total_row_groups > summary.market_data.row_groups);

        let guard = catalog_preflight_guard(u64::MAX, u64::MAX, summary.market_data.row_groups);
        let error = read_back_trade_ticks_guarded(dir.path(), "BNBUSDC.BYBIT", &guard)
            .expect_err("instrument row groups must count toward the aggregate safety limit");

        assert!(
            format!("{error:#}").contains("max_projected_row_groups"),
            "{error:#}"
        );
    }

    #[test]
    fn catalog_preflight_rejects_many_tiny_files_at_row_group_derived_file_cap_plus_one() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let source = dir.path().join("tiny-0.parquet");
        table
            .write_parquet(&source, &test_catalog_encoding())
            .expect("write one-row-group Parquet");
        let single = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect("preflight single tiny Parquet");
        assert_eq!(single.files.len(), 1);
        assert_eq!(single.total_row_groups, 1);
        let allowed_files = 3_usize;
        for index in 1..=allowed_files {
            fs::copy(&source, dir.path().join(format!("tiny-{index}.parquet")))
                .expect("copy valid tiny Parquet");
        }

        let error = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &catalog_preflight_guard(
                u64::MAX,
                u64::MAX,
                u64::try_from(allowed_files).expect("allowed file count fits u64"),
            ),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect_err("row-group-derived file-count cap plus one must fail closed");

        assert!(
            error
                .to_string()
                .contains("catalog Parquet file count actual"),
            "{error:#}"
        );
    }

    #[test]
    fn catalog_preflight_combined_accounting_accepts_exact_cap_and_rejects_cap_plus_one() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project");
        let baseline = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect("baseline preflight");
        assert!(baseline.total_accounted_bytes > 0);

        preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &catalog_preflight_guard(u64::MAX, baseline.total_accounted_bytes, u64::MAX),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect("exact combined byte cap must pass");

        let error = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &catalog_preflight_guard(
                u64::MAX,
                baseline
                    .total_accounted_bytes
                    .checked_sub(1)
                    .expect("positive combined total"),
                u64::MAX,
            ),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect_err("combined cap plus one byte must fail closed");

        assert!(error.to_string().contains("max_decoded_bytes"), "{error:#}");
    }

    #[test]
    fn catalog_preflight_rejects_zero_row_group_parquet() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("empty.parquet");
        let schema = Arc::new(arrow::datatypes::Schema::new(vec![
            arrow::datatypes::Field::new("value", arrow::datatypes::DataType::Int64, false),
        ]));
        let file = std::fs::File::create(&path).expect("create empty Parquet");
        let writer = parquet::arrow::ArrowWriter::try_new(file, schema, None)
            .expect("construct empty Parquet writer");
        writer.close().expect("close empty Parquet writer");

        let error = preflight_nt_catalog_parquet_guarded(
            dir.path(),
            &OperatorWorkBudgetGuard::unbounded(),
            OperatorWorkBudgetStage::CatalogProjection,
        )
        .expect_err("zero-row-group files must not evade the structural file-count bound");

        assert!(
            error.to_string().contains("has zero row groups"),
            "{error:#}"
        );
    }

    fn quote_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        bid: &str,
        ask: &str,
        bid_size: &str,
        ask_size: &str,
    ) -> CanonicalQuoteRow {
        CanonicalQuoteRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BNBUSDC".to_string(),
            canonical_instrument_key: "bybit/spot/BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: Some("BNBUSDC.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            bid: bid.to_string(),
            ask: ask.to_string(),
            bid_size: bid_size.to_string(),
            ask_size: ask_size.to_string(),
        }
    }

    // Distinct capture_time != event_time (and availability_time=None) so the
    // ts_init==capture_time proof is non-vacuous: it actually distinguishes the
    // receipt clock from the event clock.
    fn canonical_quotes_table() -> CanonicalQuotesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            quote_row(event_time, capture_time, None, "617.0", "617.1", "10", "12"),
            quote_row(
                event_time + 1,
                capture_time + 1,
                None,
                "617.1",
                "617.2",
                "8",
                "0",
            ),
        ];
        CanonicalQuotesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "spot".to_string(),
                product_category: "spot".to_string(),
                instrument_id: "BNBUSDC".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::QuoteReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_quote_ticks() {
        let table = canonical_quotes_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_quotes_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_QUOTE_TICK);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_quotes(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        let mut loaded = loaded;
        loaded.sort_by_key(|quote| quote.ts_event.as_u64());
        for (quote, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(quote.instrument_id.to_string(), "BNBUSDC.BYBIT");
            assert_eq!(
                quote.bid_price.as_decimal(),
                Decimal::from_str(&row.bid).unwrap()
            );
            assert_eq!(
                quote.ask_price.as_decimal(),
                Decimal::from_str(&row.ask).unwrap()
            );
            assert_eq!(
                quote.bid_size.as_decimal(),
                Decimal::from_str(&row.bid_size).unwrap()
            );
            assert_eq!(
                quote.ask_size.as_decimal(),
                Decimal::from_str(&row.ask_size).unwrap()
            );
            // ts_event preserves the event clock; ts_init is the receipt clock.
            assert_eq!(
                quote.ts_event.as_u64(),
                u64::try_from(row.event_time).unwrap()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                quote.ts_init.as_u64(),
                u64::try_from(row.capture_time).unwrap(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn quote_ticks_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming
        // the capture_time field, never silently stamping ts_init=0.
        let mut table = canonical_quotes_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_quote_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn quote_ticks_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to
        // the (valid) capture clock.
        let mut table = canonical_quotes_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let err = canonical_rows_to_quote_ticks(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn quote_table_validate_rejects_crossed_book() {
        let mut table = canonical_quotes_table();
        table.rows[0].ask = "0.40".to_string();
        let err = table.validate().expect_err("crossed book rejected");
        assert!(err.to_string().contains("below bid"), "{err}");
    }

    #[test]
    fn quote_catalog_hash_matches_projection() {
        // Proves the new quote query-back + hash block is wired into the logical
        // digest: recomputing over the written catalog reproduces the hash the
        // projection recorded.
        let table = canonical_quotes_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_quotes_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            projection.catalog_hash,
            logical_catalog_hash(dir.path()).unwrap(),
            "quote catalog hash must describe the logical quote catalog contents"
        );
    }

    #[test]
    fn quote_catalog_hash_is_deterministic_across_roots() {
        // No committed reference quote catalog exists, so determinism across two
        // independent roots is the pin: the same quote data must hash identically.
        let table = canonical_quotes_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_quotes_to_catalog(
            &table,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_quotes_to_catalog(
            &table,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same quote data must hash identically regardless of root"
        );
    }

    fn index_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        value: &str,
    ) -> CanonicalIndexPriceRow {
        CanonicalIndexPriceRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BNBUSDC".to_string(),
            canonical_instrument_key: "bybit/spot/BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: Some("BNBUSDC.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            value: value.to_string(),
        }
    }

    // Distinct capture_time != event_time (availability None) so the
    // ts_init==capture_time proof is non-vacuous. The values carry 2 decimals,
    // finer than the spec()'s 1-decimal (0.1) tick, so projection widens the
    // instrument price precision (exercising the index price_values view); the
    // empty size_values view leaves size precision unchanged.
    fn canonical_index_prices_table() -> CanonicalIndexPricesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            index_row(event_time, capture_time, None, "617.05"),
            index_row(event_time + 1, capture_time + 1, None, "617.15"),
        ];
        CanonicalIndexPricesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "spot".to_string(),
                product_category: "spot".to_string(),
                instrument_id: "BNBUSDC".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::IndexReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_index_prices() {
        let table = canonical_index_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_index_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_INDEX_PRICE_UPDATE);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_index(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        for (update, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(update.instrument_id.to_string(), "BNBUSDC.BYBIT");
            assert_eq!(
                update.value.as_decimal(),
                Decimal::from_str(&row.value).unwrap()
            );
            let label = format!("index price {}", row.event_time);
            // ts_event preserves the event clock; ts_init is the receipt clock.
            assert_eq!(
                update.ts_event.as_u64(),
                ts_event_nanos(row.event_time, &label).unwrap().as_u64()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                update.ts_init.as_u64(),
                ts_init_nanos(row.availability_time, row.capture_time, &label)
                    .unwrap()
                    .as_u64(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn index_projection_widens_price_precision_and_keeps_size_precision() {
        // The 2-decimal index values are finer than spec()'s 1-decimal (0.1)
        // tick, so projection widens price precision; index carries no size, so
        // the empty size_values view leaves the instrument size precision intact.
        let table = canonical_index_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_index_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project");
        let loaded = read_back_index(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert!(!loaded.is_empty());
        for update in &loaded {
            assert_eq!(
                update.value.precision, 2,
                "index value precision must widen to the data's 2 decimals"
            );
        }
        // spec()'s size_increment is 0.0001 (precision 4) and must be unchanged
        // by an index projection that contributes no size column.
        let instrument = build_currency_pair(&spec()).expect("instrument");
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn index_prices_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming
        // the capture_time field, never silently stamping ts_init=0.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_index_prices_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_index_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn index_prices_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to
        // the (valid) capture clock.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_index_prices_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_index_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn index_projection_refuses_dirty_catalog_root() {
        let table = canonical_index_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_index_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn index_catalog_hash_is_deterministic_across_roots() {
        // No committed reference index catalog exists, so determinism across two
        // independent roots is the pin: the same index data must hash identically.
        let table = canonical_index_prices_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_index_to_catalog(
            &table,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_index_to_catalog(
            &table,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same index data must hash identically regardless of root"
        );
    }

    #[test]
    fn index_catalog_hash_changes_with_data_content() {
        // Two index tables differing only in one row's value must hash
        // differently, proving the new [34..37] tag block covers index value
        // bytes (not just file paths).
        let table_a = canonical_index_prices_table();
        let mut table_b = canonical_index_prices_table();
        table_b.rows[0].value = "618.05".to_string();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_index_to_catalog(
            &table_a,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_index_to_catalog(
            &table_b,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different index value must change the catalog hash"
        );
    }

    fn mark_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        value: &str,
    ) -> CanonicalMarkPriceRow {
        CanonicalMarkPriceRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BNBUSDC".to_string(),
            canonical_instrument_key: "bybit/spot/BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: Some("BNBUSDC.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            value: value.to_string(),
        }
    }

    // Distinct capture_time != event_time (availability None) so the
    // ts_init==capture_time proof is non-vacuous. The values carry 2 decimals,
    // finer than the spec()'s 1-decimal (0.1) tick, so projection widens the
    // instrument price precision (exercising the mark price_values view); the
    // empty size_values view leaves size precision unchanged.
    fn canonical_mark_prices_table() -> CanonicalMarkPricesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            mark_row(event_time, capture_time, None, "617.05"),
            mark_row(event_time + 1, capture_time + 1, None, "617.15"),
        ];
        CanonicalMarkPricesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "spot".to_string(),
                product_category: "spot".to_string(),
                instrument_id: "BNBUSDC".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::MarkReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_mark_prices() {
        let table = canonical_mark_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_mark_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_MARK_PRICE_UPDATE);
        assert_eq!(projection.nt_instrument_id, "BNBUSDC.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_mark(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        for (update, row) in loaded.iter().zip(table.rows.iter()) {
            assert_eq!(update.instrument_id.to_string(), "BNBUSDC.BYBIT");
            assert_eq!(
                update.value.as_decimal(),
                Decimal::from_str(&row.value).unwrap()
            );
            let label = format!("mark price {}", row.event_time);
            // ts_event preserves the event clock; ts_init is the receipt clock.
            assert_eq!(
                update.ts_event.as_u64(),
                ts_event_nanos(row.event_time, &label).unwrap().as_u64()
            );
            assert_eq!(row.availability_time, None);
            assert_eq!(
                update.ts_init.as_u64(),
                ts_init_nanos(row.availability_time, row.capture_time, &label)
                    .unwrap()
                    .as_u64(),
                "ts_init must equal capture_time (the receipt clock), not event_time"
            );
        }
    }

    #[test]
    fn mark_projection_widens_price_precision_and_keeps_size_precision() {
        // The 2-decimal mark values are finer than spec()'s 1-decimal (0.1)
        // tick, so projection widens price precision; mark carries no size, so
        // the empty size_values view leaves the instrument size precision intact.
        let table = canonical_mark_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_mark_to_catalog(&table, &spec(), dir.path(), &test_catalog_encoding())
            .expect("project");
        let loaded = read_back_mark(dir.path(), "BNBUSDC.BYBIT").expect("read back");
        assert!(!loaded.is_empty());
        for update in &loaded {
            assert_eq!(
                update.value.precision, 2,
                "mark value precision must widen to the data's 2 decimals"
            );
        }
        // spec()'s size_increment is 0.0001 (precision 4) and must be unchanged
        // by a mark projection that contributes no size column.
        let instrument = build_currency_pair(&spec()).expect("instrument");
        assert_eq!(instrument.size_precision(), 4);
    }

    #[test]
    fn mark_prices_fail_loud_when_capture_invalid_and_no_availability() {
        // validate() does not guard capture_time, so a None availability with a
        // non-positive capture clock reaches the seam and must fail loud naming
        // the capture_time field, never silently stamping ts_init=0.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_mark_prices_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_mark_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn mark_prices_fail_loud_when_availability_some_but_invalid() {
        // A present-but-invalid availability_time must fail, never fall back to
        // the (valid) capture clock.
        // Use spec_precision2() so rescaled("617.05", 2) passes and execution
        // reaches the ts_init_nanos validation rather than dying on the
        // precision gate first.
        let mut table = canonical_mark_prices_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = build_currency_pair(&spec_precision2()).expect("instrument");
        let err = canonical_rows_to_mark_price_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn mark_projection_refuses_dirty_catalog_root() {
        let table = canonical_mark_prices_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_mark_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn mark_catalog_hash_is_deterministic_across_roots() {
        // No committed reference mark catalog exists, so determinism across two
        // independent roots is the pin: the same mark data must hash identically.
        let table = canonical_mark_prices_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_mark_to_catalog(
            &table,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_mark_to_catalog(
            &table,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same mark data must hash identically regardless of root"
        );
    }

    #[test]
    fn mark_catalog_hash_changes_with_data_content() {
        // Two mark tables differing only in one row's value must hash
        // differently, proving the new [38..41] tag block covers mark value
        // bytes (not just file paths).
        let table_a = canonical_mark_prices_table();
        let mut table_b = canonical_mark_prices_table();
        table_b.rows[0].value = "618.05".to_string();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_mark_to_catalog(
            &table_a,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_mark_to_catalog(
            &table_b,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different mark value must change the catalog hash"
        );
    }

    fn funding_rate_row(
        event_time: i64,
        capture_time: i64,
        availability_time: Option<i64>,
        rate: &str,
        interval_minutes: Option<u16>,
        next_funding_time: Option<i64>,
    ) -> CanonicalFundingRateRow {
        CanonicalFundingRateRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "BYBIT".to_string(),
            product_family: "perpetual".to_string(),
            product_category: "linear-perp".to_string(),
            instrument_id: "BTCUSDT".to_string(),
            canonical_instrument_key: "bybit/perpetual/BTCUSDT".to_string(),
            venue_symbol: "BTCUSDT".to_string(),
            nt_instrument_id: Some("BTCUSDT.BYBIT".to_string()),
            event_time,
            capture_time,
            availability_time,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            rate: rate.to_string(),
            interval_minutes,
            next_funding_time,
        }
    }

    fn canonical_funding_rates_table() -> CanonicalFundingRatesTable {
        let event_time = 1_700_000_000_000_000_000;
        let capture_time = 1_700_000_000_000_000_500;
        let rows = vec![
            funding_rate_row(
                event_time,
                capture_time,
                None,
                "-0.000100",
                Some(480),
                Some(event_time + 28_800_000_000_000),
            ),
            funding_rate_row(
                event_time + 1,
                capture_time + 1,
                None,
                "0.000250",
                Some(480),
                Some(event_time + 28_800_000_000_000),
            ),
        ];
        CanonicalFundingRatesTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "BYBIT".to_string(),
                product_family: "perpetual".to_string(),
                product_category: "linear-perp".to_string(),
                instrument_id: "BTCUSDT".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::FundingReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn projects_and_reads_back_funding_rates() {
        let table = canonical_funding_rates_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        assert_eq!(projection.trade_count, 2);
        assert_eq!(projection.data_type, NT_DATA_TYPE_FUNDING_RATE_UPDATE);
        assert_eq!(projection.nt_instrument_id, "BTCUSDT.BYBIT");
        assert!(!projection.catalog_hash.is_empty());

        let loaded = read_back_funding_rates(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);
        crate::runner::assert_funding_read_back_matches(&loaded, &table, "BTCUSDT.BYBIT")
            .expect("shared funding read-back assertion");
    }

    #[test]
    fn funding_read_back_assert_is_order_independent_and_still_fails_loud() {
        // Differential guard for the order-independent pairing in
        // `assert_funding_read_back_matches`. The fixture's rows are already in
        // ascending `event_time` order, which is also the read-back sort order,
        // so `projects_and_reads_back_funding_rates` never actually exercises the
        // reorder. This test feeds the canonical table in a DIFFERENT stored
        // order than the read-back, plus a corrupted variant, so the sort-both-
        // sides logic is genuinely load-bearing here:
        //   - Under the current code both sides are key-sorted, so reversed input
        //     still pairs correctly and passes.
        //   - Under the old positional `zip`, reversed canonical row 0 (rate
        //     0.000250) would pair with read-back row 0 (rate -0.000100) and fail.
        let table = canonical_funding_rates_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        let loaded = read_back_funding_rates(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert_eq!(loaded.len(), 2);

        // Order-independence: reverse the canonical stored order so it no longer
        // matches the ascending read-back order. The assertion must still pass.
        let mut reversed = table.clone();
        reversed.rows.reverse();
        crate::runner::assert_funding_read_back_matches(&loaded, &reversed, "BTCUSDT.BYBIT")
            .expect("read-back assertion must be independent of canonical stored order");

        // Non-vacuity: a genuinely divergent canonical rate must still fail loud,
        // proving the self-sorting did not make the per-field comparison circular.
        // `event_time` is unchanged, so the corrupted row still pairs by ts_event
        // with the matching read-back row and trips the rate ensure!.
        let mut corrupted = table.clone();
        corrupted.rows[0].rate = "9.999999".to_string();
        assert!(
            crate::runner::assert_funding_read_back_matches(&loaded, &corrupted, "BTCUSDT.BYBIT")
                .is_err(),
            "a divergent canonical rate must fail the read-back assertion"
        );
    }

    #[test]
    fn read_back_funding_rates_returns_empty_when_catalog_has_no_funding_files() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let loaded = read_back_funding_rates(dir.path(), "BTCUSDT.BYBIT").expect("read back");
        assert!(loaded.is_empty());
    }

    #[test]
    fn funding_projection_requires_nt_instrument_id() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].nt_instrument_id = None;
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("missing nt_instrument_id rejected");

        assert!(
            err.to_string().contains("missing nt_instrument_id"),
            "{err}"
        );
    }

    #[test]
    fn funding_projection_rejects_later_missing_nt_instrument_id() {
        let mut table = canonical_funding_rates_table();
        table.rows[1].nt_instrument_id = None;
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("later missing nt_instrument_id rejected");

        assert!(err.to_string().contains("row 1"), "{err}");
        assert!(
            err.to_string().contains("missing nt_instrument_id"),
            "{err}"
        );
    }

    #[test]
    fn funding_projection_rejects_nt_instrument_id_mismatch() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].nt_instrument_id = Some("ETHUSDT.BYBIT".to_string());
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("nt_instrument_id mismatch rejected");

        assert!(
            err.to_string().contains("does not match canonical rows"),
            "{err}"
        );
    }

    #[test]
    fn funding_projection_rejects_later_nt_instrument_id_mismatch() {
        let mut table = canonical_funding_rates_table();
        table.rows[1].nt_instrument_id = Some("ETHUSDT.BYBIT".to_string());
        let dir = tempfile::TempDir::new().expect("temp dir");

        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("later nt_instrument_id mismatch rejected");

        assert!(err.to_string().contains("row 1"), "{err}");
        assert!(
            err.to_string().contains("does not match canonical rows"),
            "{err}"
        );
    }

    #[test]
    fn funding_rates_fail_loud_when_rate_is_malformed() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].rate = "not-a-decimal".to_string();
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("invalid funding rate"), "{err}");
    }

    #[test]
    fn funding_rates_fail_loud_when_next_funding_time_is_negative() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].next_funding_time = Some(-1);
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(
            err.to_string().contains("negative next_funding_time"),
            "{err}"
        );
    }

    #[test]
    fn funding_rates_fail_loud_when_next_funding_time_is_zero() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].next_funding_time = Some(0);
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(
            err.to_string().contains("non-positive next_funding_time"),
            "{err}"
        );
    }

    #[test]
    fn funding_rates_fail_loud_when_capture_invalid_and_no_availability() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].availability_time = None;
        table.rows[0].capture_time = 0;
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("capture_time"), "{err}");
    }

    #[test]
    fn funding_rates_fail_loud_when_availability_some_but_invalid() {
        let mut table = canonical_funding_rates_table();
        table.rows[0].availability_time = Some(0);
        table.rows[0].capture_time = 42;
        let instrument = linear_perpetual_spec()
            .build_instrument_any()
            .expect("instrument");
        let err = canonical_rows_to_funding_rate_updates(&table, &instrument).unwrap_err();
        assert!(err.to_string().contains("availability_time"), "{err}");
    }

    #[test]
    fn funding_projection_refuses_dirty_catalog_root() {
        let table = canonical_funding_rates_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn funding_catalog_hash_is_deterministic_across_roots() {
        let table = canonical_funding_rates_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same funding data must hash identically regardless of root"
        );
    }

    #[test]
    fn funding_catalog_hash_matches_golden_v1() {
        // Golden computed over the canonical fixture at NT rev 6be5a50 with the current bolt
        // hash-input layout; a future NT bump or hash-tag/layout change requires regenerating this.
        const EXPECTED: &str = "1193d55c1d22b2c3fc95398904d7ebeed6a5c939eacdebe7427f279df5967dfa";

        let table = canonical_funding_rates_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_funding_rates_to_catalog(
            &table,
            &linear_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .unwrap();

        assert_eq!(
            projection.catalog_hash, EXPECTED,
            "funding-bearing logical catalog hash v1 bytes changed"
        );
    }

    #[test]
    fn funding_catalog_hash_changes_with_data_content() {
        let table_a = canonical_funding_rates_table();
        let mut table_b = canonical_funding_rates_table();
        table_b.rows[0].rate = "-0.000200".to_string();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different funding rate must change the catalog hash"
        );
    }

    #[test]
    fn funding_catalog_hash_orders_scale_distinct_equal_rates_deterministically() {
        let mut table_a = canonical_funding_rates_table();
        table_a.rows[0].rate = "0.000100".to_string();
        table_a.rows[1] = table_a.rows[0].clone();
        table_a.rows[1].rate = "0.0001000".to_string();

        let mut table_b = table_a.clone();
        table_b.rows.reverse();

        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "scale-distinct equal rates must sort deterministically regardless of input order"
        );

        // Pin the load-bearing premise of the `.rate.scale()` tie-break in `logical_catalog_hash`:
        // NT serializes `FundingRateUpdate.rate` as a utf8 string (`FUNDING_RATE_UPDATE_FIELDS` in
        // nautilus-serialization), so the numerically-equal but scale-distinct rates "0.000100"
        // (scale 6) and "0.0001000" (scale 7) survive the Parquet round-trip as DISTINCT scales.
        // That distinctness is exactly what makes the scale tie-break load-bearing rather than dead
        // code; if a future NT rev collapses Decimal scale on round-trip, this assertion fails loud
        // and the tie-break must be re-evaluated.
        let mut read_back_scales: Vec<u32> = read_back_funding_rates(dir_a.path(), "BTCUSDT.BYBIT")
            .unwrap()
            .iter()
            .map(|update| update.rate.scale())
            .collect();
        read_back_scales.sort_unstable();
        assert_eq!(
            read_back_scales,
            vec![6, 7],
            "scale-distinct funding rates must round-trip through the NT catalog with distinct \
             Decimal scales, proving the catalog-hash scale tie-break is load-bearing"
        );
    }

    #[test]
    fn funding_catalog_hash_changes_with_interval_content() {
        let table_a = canonical_funding_rates_table();
        let mut table_b = canonical_funding_rates_table();
        table_b.rows[0].interval_minutes = Some(240);
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different funding interval must change the catalog hash"
        );
    }

    #[test]
    fn funding_catalog_hash_changes_with_next_funding_time_content() {
        let table_a = canonical_funding_rates_table();
        let mut table_b = canonical_funding_rates_table();
        table_b.rows[0].next_funding_time = Some(table_b.rows[0].event_time + 57_600_000_000_000);
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_funding_rates_to_catalog(
            &table_a,
            &linear_perpetual_spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_funding_rates_to_catalog(
            &table_b,
            &linear_perpetual_spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different next funding time must change the catalog hash"
        );
    }

    #[test]
    fn mark_section_does_not_change_trade_only_catalog_hash() {
        // The mark loop is appended AFTER the index loop with fresh tags 38..41
        // and emits nothing for an empty mark set, so a trade-only catalog must
        // still hash to expected_logical_catalog_hash (which hashes only the
        // instrument + ticks, no mark bytes). This protects the committed PMXT
        // hash pin against mark-section byte-tag drift.
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("ticks");
        assert_eq!(
            projection.catalog_hash,
            expected_logical_catalog_hash(&instrument, &ticks),
            "an empty mark section must add zero bytes to a trade-only catalog hash"
        );
    }

    #[test]
    fn funding_section_does_not_change_trade_only_catalog_hash() {
        // The funding loop is appended after mark with fresh tags 42..47 and
        // emits nothing for catalogs with no funding files. Trade-only catalogs
        // must keep the committed logical-hash byte stream unchanged.
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("ticks");
        assert_eq!(
            projection.catalog_hash,
            expected_logical_catalog_hash(&instrument, &ticks),
            "an empty funding section must add zero bytes to a trade-only catalog hash"
        );
    }

    // Synthetic, token-agnostic fixtures for the precision-widening tests.
    // The behaviour under test is data-driven and must not be tied to any
    // real token, venue, or incident value (same precedent as the
    // `YES.TESTVENUE` binary-option fixture below).
    fn synthetic_spot_spec() -> SpotInstrumentSpec {
        let mut spec = spec();
        spec.nt_instrument_id = "BASEQUOTE.TESTVENUE".to_string();
        spec.raw_symbol = "BASEQUOTE".to_string();
        spec.base_currency = "BASE".to_string();
        spec.quote_currency = "QUOTE".to_string();
        spec
    }

    fn synthetic_perpetual_spec() -> CatalogInstrumentSpec {
        let CatalogInstrumentSpec::CryptoPerpetual(mut spec) = linear_perpetual_spec() else {
            panic!("expected CryptoPerpetual fixture");
        };
        spec.nt_instrument_id = "BASEQUOTE-PERP.TESTVENUE".to_string();
        spec.raw_symbol = "BASEQUOTE-PERP".to_string();
        spec.base_currency = "BASE".to_string();
        spec.quote_currency = "QUOTE".to_string();
        spec.settlement_currency = "QUOTE".to_string();
        CatalogInstrumentSpec::CryptoPerpetual(spec)
    }

    fn synthetic_identity(
        instrument_id: &str,
        nt_instrument_id: &str,
    ) -> CanonicalInstrumentIdentity {
        CanonicalInstrumentIdentity {
            instrument_id: instrument_id.to_string(),
            venue_symbol: instrument_id.to_string(),
            nt_instrument_id: nt_instrument_id.to_string(),
        }
    }

    fn synthetic_table(
        csv: &str,
        instrument_id: &str,
        nt_instrument_id: &str,
    ) -> CanonicalTradesTable {
        normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &synthetic_identity(instrument_id, nt_instrument_id),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize")
    }

    #[test]
    fn projection_widens_precision_when_archive_prints_are_finer_than_tick() {
        // Regression class: a venue's live instrument endpoint describes the
        // CURRENT tick (0.1 here), but the historical archive carries finer
        // prints (price scale 2, size scale 5 vs size precision 4). The
        // projection must widen the instrument to the accepted data's actual
        // scale instead of rejecting the accepted object.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.34,0.30001,buy,0\n\
            2,1772323312219,12.3,0.1456,sell,0\n";
        let table = synthetic_table(csv, "BASEQUOTE", "BASEQUOTE.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_trades_to_catalog(
            &table,
            &synthetic_spot_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("projection widens precision instead of rejecting accepted data");
        assert_eq!(projection.trade_count, 2);

        // Read-back preserves the exact archived values.
        let loaded = read_back_trade_ticks(dir.path(), "BASEQUOTE.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("12.34"));
        assert_eq!(loaded[0].size, Quantity::from("0.30001"));

        // The catalog instrument carries the widened precision, with the tick
        // VALUE unchanged (0.1 -> 0.10, 0.0001 -> 0.00010).
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["BASEQUOTE.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments.len(), 1);
        assert_eq!(instruments[0].price_precision(), 2);
        assert_eq!(instruments[0].size_precision(), 5);
        assert_eq!(instruments[0].price_increment(), Price::from("0.10"));
        assert_eq!(instruments[0].size_increment(), Quantity::from("0.00010"));
    }

    #[test]
    fn projection_keeps_venue_precision_for_coarser_data() {
        // Coarse prints must NOT narrow the venue precision: a day of
        // whole-number trades keeps tick 0.1 / size precision 4 unchanged.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12,0.3,buy,0\n";
        let table = synthetic_table(csv, "BASEQUOTE", "BASEQUOTE.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(
            &table,
            &synthetic_spot_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["BASEQUOTE.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments[0].price_precision(), 1);
        assert_eq!(instruments[0].size_precision(), 4);
        assert_eq!(instruments[0].price_increment(), Price::from("0.1"));
        assert_eq!(instruments[0].size_increment(), Quantity::from("0.0001"));
    }

    #[test]
    fn widening_ignores_trailing_zeros_in_source_values() {
        // A source value like "12.30" normalizes to scale 1 — trailing zeros
        // must not force a widening (mirrors `rescaled`'s
        // normalize-before-check behaviour).
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.30,0.3000,buy,0\n";
        let table = synthetic_table(csv, "BASEQUOTE", "BASEQUOTE.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(
            &table,
            &synthetic_spot_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project");
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["BASEQUOTE.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments[0].price_precision(), 1);
        let loaded = read_back_trade_ticks(dir.path(), "BASEQUOTE.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("12.3"));
    }

    #[test]
    fn projection_widens_derivative_precision_when_data_is_finer() {
        // The widening is instrument-kind-agnostic: a CryptoPerpetual spec at
        // tick 0.1 / size precision 3 must also accept finer archived prints.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.34,0.3001,buy,0\n";
        let table = synthetic_table(csv, "BASEQUOTE-PERP", "BASEQUOTE-PERP.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(
            &table,
            &synthetic_perpetual_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("derivative projection widens precision");
        let loaded =
            read_back_trade_ticks(dir.path(), "BASEQUOTE-PERP.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("12.34"));
        assert_eq!(loaded[0].size, Quantity::from("0.3001"));
    }

    #[test]
    fn projection_widens_binary_option_precision_when_data_is_finer() {
        // The widening arm covers binary options too: the YES.TESTVENUE fixture
        // is tick 0.01 / size precision 3, but a prediction-market archive can
        // carry finer prints (price scale 3, size scale 4). The projection must
        // widen the instrument to the data's actual scale instead of rejecting
        // the accepted object.
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,0.491,10.0001,buy,0\n\
            2,1772323312219,0.512,12.5,sell,0\n";
        let table = synthetic_table(csv, "YES", "YES.TESTVENUE");
        let dir = tempfile::TempDir::new().expect("temp dir");
        project_canonical_trades_to_catalog(
            &table,
            &binary_option_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("binary option projection widens precision");

        let loaded = read_back_trade_ticks(dir.path(), "YES.TESTVENUE").expect("read back");
        assert_eq!(loaded[0].price, Price::from("0.491"));
        assert_eq!(loaded[0].size, Quantity::from("10.0001"));

        // The catalog instrument carries the widened precision, tick VALUE
        // unchanged (0.01 -> 0.010, 0.001 -> 0.0010).
        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["YES.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments.len(), 1);
        assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));
        assert_eq!(instruments[0].price_precision(), 3);
        assert_eq!(instruments[0].size_precision(), 4);
        assert_eq!(instruments[0].price_increment(), Price::from("0.010"));
        assert_eq!(instruments[0].size_increment(), Quantity::from("0.0010"));

        // This fixture leaves the optional risk and bound metadata absent; the
        // populated-field test above proves that the official catalog preserves
        // them. Keep explicit assertions here because BinaryOption equality is
        // identity-based rather than a complete serialized-field comparison.
        let InstrumentAny::BinaryOption(option) = &instruments[0] else {
            panic!("expected BinaryOption after catalog round-trip");
        };
        assert_eq!(
            option.outcome.map(|o| o.to_string()),
            Some("Yes".to_string()),
            "outcome must survive catalog round-trip"
        );
        assert_eq!(
            option.description.map(|d| d.to_string()),
            Some("Bounded binary option fixture".to_string()),
            "description must survive catalog round-trip"
        );
        assert_eq!(
            option.max_quantity,
            Some(Quantity::from("1000000")),
            "max_quantity must survive catalog round-trip"
        );
        assert_eq!(
            option.min_quantity,
            Some(Quantity::from("1")),
            "min_quantity must survive catalog round-trip"
        );
        assert_eq!(
            option.maker_fee,
            Decimal::from_str("0.001").unwrap(),
            "maker_fee must survive catalog round-trip"
        );
        assert_eq!(
            option.taker_fee,
            Decimal::from_str("0.002").unwrap(),
            "taker_fee must survive catalog round-trip"
        );
        // The minimal fixture omits these values, so absence must also survive.
        assert_eq!(option.max_notional, None);
        assert_eq!(option.min_notional, None);
        assert_eq!(option.max_price(), None);
        assert_eq!(option.min_price(), None);
        assert_eq!(option.margin_init, Decimal::ZERO);
        assert_eq!(option.margin_maint, Decimal::ZERO);
    }

    fn binary_option_bar_row(
        open_time: i64,
        open: &str,
        high: &str,
        low: &str,
        close: &str,
        volume: &str,
    ) -> CanonicalBarRow {
        CanonicalBarRow {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-binary-option-bars".to_string(),
            source_binding: "kalshi-official-historical-api".to_string(),
            venue: "TESTVENUE".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            open_time,
            close_time: open_time + 60_000_000_000,
            capture_time: open_time + 60_000_000_500,
            availability_time: None,
            source_sequence: Some(open_time.to_string()),
            raw_payload_id: "kalshi-bars-sample-1".to_string(),
            source_proof_id:
                "source-proof-kalshi-official-historical-binary-option-pending-2026-06-08"
                    .to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            open: open.to_string(),
            high: high.to_string(),
            low: low.to_string(),
            close: close.to_string(),
            volume: volume.to_string(),
        }
    }

    fn binary_option_bars_table() -> CanonicalBarsTable {
        let base = 1_700_000_000_000_000_000;
        CanonicalBarsTable {
            schema_version: crate::canonical_trades::NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: crate::canonical_trades::TradesPartition {
                venue: "TESTVENUE".to_string(),
                product_family: "prediction-market".to_string(),
                product_category: "binary".to_string(),
                instrument_id: "YES".to_string(),
                dt: "2023-11-14".to_string(),
            },
            source_proof_id:
                "source-proof-kalshi-official-historical-binary-option-pending-2026-06-08"
                    .to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            bar_spec: CanonicalBarSpec {
                step: 1,
                aggregation: BarAggregation::Minute,
            },
            rows: vec![
                binary_option_bar_row(base, "0.49", "0.55", "0.48", "0.52", "100"),
                binary_option_bar_row(
                    base + 60_000_000_000,
                    "0.52",
                    "0.58",
                    "0.51",
                    "0.57",
                    "120.5",
                ),
            ],
        }
    }

    #[test]
    fn binary_option_bar_catalog_projection_round_trips_through_nt_catalog() {
        let table = binary_option_bars_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_canonical_bars_to_catalog(
            &table,
            &binary_option_spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project binary-option bars");
        assert_eq!(projection.trade_count, table.rows.len());
        assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
        assert_eq!(projection.nt_instrument_id, "YES.TESTVENUE");
        assert_eq!(
            projection.fidelity_class,
            SourceProofFidelityClass::TradeBarReplay
        );
        assert!(!projection.catalog_hash.is_empty());

        let mut loaded = read_back_bars(dir.path(), "YES.TESTVENUE").expect("read back bars");
        loaded.sort_by_key(|bar| bar.ts_event.as_u64());
        assert_eq!(loaded.len(), table.rows.len());
        assert_eq!(loaded[0].instrument_id().to_string(), "YES.TESTVENUE");
        assert_eq!(loaded[0].open, Price::from("0.49"));
        assert_eq!(loaded[0].high, Price::from("0.55"));
        assert_eq!(loaded[0].low, Price::from("0.48"));
        assert_eq!(loaded[0].close, Price::from("0.52"));
        assert_eq!(loaded[0].volume, Quantity::from("100.000"));
        assert_eq!(loaded[0].ts_event.as_u64(), 1_700_000_060_000_000_000);
        assert_eq!(loaded[0].ts_init.as_u64(), 1_700_000_060_000_000_500);

        let catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let instruments = catalog
            .query_instruments(Some(&["YES.TESTVENUE".to_string()]))
            .expect("query instruments");
        assert_eq!(instruments.len(), 1);
        assert!(matches!(&instruments[0], InstrumentAny::BinaryOption(_)));
    }

    #[test]
    fn projection_hashes_url_encoded_non_ascii_instrument_catalog() {
        let mut spec = synthetic_spot_spec();
        spec.nt_instrument_id = "币安人生USDC.BINANCE".to_string();
        spec.raw_symbol = "币安人生USDC".to_string();
        spec.base_currency = "币安人生".to_string();
        spec.quote_currency = "USDC".to_string();
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,12.34,0.3001,buy,0\n";
        let table = synthetic_table(csv, "币安人生USDC", "币安人生USDC.BINANCE");
        let dir = tempfile::TempDir::new().expect("temp dir");

        let projection = project_canonical_trades_to_catalog(
            &table,
            &spec,
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect("project non-ASCII catalog path");
        let loaded = read_back_trade_ticks(dir.path(), "币安人生USDC.BINANCE").expect("read back");

        assert_eq!(projection.trade_count, 1);
        assert_eq!(projection.nt_instrument_id, "币安人生USDC.BINANCE");
        assert!(!projection.catalog_hash.is_empty());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].instrument_id.to_string(), "币安人生USDC.BINANCE");
    }

    #[test]
    fn datafusion_catalog_file_path_resolves_all_path_shapes() {
        // NT's resolve_path_for_datafusion passes absolute native paths and
        // full URIs through verbatim but URL-joins anything relative, which
        // percent-decodes encoded instrument directories into nonexistent
        // paths. Pin all three input shapes so the passthrough/join split
        // cannot silently regress.
        let root = Path::new("/catalog/root");

        assert_eq!(
            datafusion_catalog_file_path(root, "s3://bucket/data/trades/X.V/part-0.parquet"),
            "s3://bucket/data/trades/X.V/part-0.parquet"
        );
        assert_eq!(
            datafusion_catalog_file_path(root, "/elsewhere/data/trades/X.V/part-0.parquet"),
            "/elsewhere/data/trades/X.V/part-0.parquet"
        );
        assert_eq!(
            datafusion_catalog_file_path(root, "data/trades/X.V/part-0.parquet"),
            "/catalog/root/data/trades/X.V/part-0.parquet"
        );
    }

    #[test]
    fn order_book_delta_precision_normalization_rejects_rounding() {
        let instrument_id = InstrumentId::from("YES.TESTVENUE");
        for (price, size, expected_field) in [("0.491", "1.23", "price"), ("0.49", "1.234", "size")]
        {
            let delta = OrderBookDelta::new_checked(
                instrument_id,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from(price), Quantity::from(size), 1),
                0,
                0,
                UnixNanos::from(1_u64),
                UnixNanos::from(2_u64),
            )
            .expect("test delta should be valid before precision normalization");
            let error = order_book_delta_at_precision(delta, 2, 2)
                .expect_err("precision normalization must not round data");
            assert!(
                error.to_string().contains(expected_field),
                "error must identify the out-of-precision field: {error}"
            );
        }

        let delta = OrderBookDelta::new_checked(
            instrument_id,
            BookAction::Add,
            BookOrder::new(
                OrderSide::Buy,
                Price::from("0.49"),
                Quantity::from("1.23"),
                1,
            ),
            0,
            0,
            UnixNanos::from(1_u64),
            UnixNanos::from(2_u64),
        )
        .expect("in-precision test delta should be valid");
        let normalized = order_book_delta_at_precision(delta, 2, 2)
            .expect("in-precision values must normalize without loss");
        assert_eq!(normalized.order.price, Price::from("0.49"));
        assert_eq!(normalized.order.size, Quantity::from("1.23"));
    }

    #[test]
    fn binary_option_l2_catalog_records_round_trip_through_nt_catalog() {
        use nautilus_model::{
            data::{OrderBookDelta, order::BookOrder},
            enums::{AssetClass, BookAction, OrderSide, RecordFlag},
            instruments::BinaryOption,
        };
        use ustr::Ustr;

        let ts_init = UnixNanos::from(1_000_000_000u64);
        let instrument_id = InstrumentId::from_str("YES.TESTVENUE").unwrap();
        let instrument = InstrumentAny::BinaryOption(
            BinaryOption::new_checked(
                instrument_id,
                Symbol::new_checked("YES").unwrap(),
                AssetClass::Alternative,
                Currency::from_str("USD").unwrap(),
                UnixNanos::from(0),
                UnixNanos::from(2_000_000_000u64),
                2,
                6,
                Price::from("0.01"),
                Quantity::from("0.000001"),
                Some(Ustr::from("Yes")),
                Some(Ustr::from("Bounded binary option fixture")),
                None,
                Some(Quantity::from("1")),
                None,
                None,
                Some(Price::from("1.00")),
                Some(Price::from("0.01")),
                None,
                None,
                Some(Decimal::ZERO),
                Some(Decimal::ZERO),
                None, // tick_scheme (NT bump)
                None,
                ts_init,
                ts_init,
            )
            .expect("binary option"),
        );
        let instrument_id = instrument.id();
        let deltas = vec![
            OrderBookDelta::clear(
                instrument_id,
                0,
                UnixNanos::from(1_772_323_201_665_000_000u64),
                ts_init,
            ),
            OrderBookDelta::new_checked(
                instrument_id,
                BookAction::Add,
                BookOrder::new(OrderSide::Buy, Price::from("0.49"), Quantity::from("10"), 0),
                RecordFlag::F_LAST as u8,
                0,
                UnixNanos::from(1_772_323_201_665_000_000u64),
                ts_init,
            )
            .expect("bid delta"),
        ]
        .into_iter()
        .map(|delta| order_book_delta_at_precision(delta, 2, 6))
        .collect::<Result<Vec<_>>>()
        .expect("normalize delta metadata to instrument precision");
        let tick = TradeTick::new(
            instrument_id,
            Price::from("0.51"),
            Quantity::from("2"),
            AggressorSide::Buyer,
            TradeId::new_checked("pmxt-trade-1").unwrap(),
            UnixNanos::from(1_772_323_201_665_000_000u64),
            ts_init,
        );

        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        catalog
            .write_instruments(vec![instrument])
            .expect("write binary option instrument");
        catalog
            .write_to_parquet(&deltas, None, None, None)
            .expect("write order book deltas");
        catalog
            .write_to_parquet(&[tick], None, None, None)
            .expect("write trade tick");

        let loaded_deltas = catalog
            .query_typed_data::<OrderBookDelta>(
                Some(vec![instrument_id.to_string()]),
                None,
                None,
                None,
                None,
                true,
            )
            .expect("read order book deltas");
        let loaded_ticks =
            read_back_trade_ticks(dir.path(), &instrument_id.to_string()).expect("read ticks");

        assert_eq!(loaded_deltas.len(), deltas.len());
        assert_eq!(loaded_ticks.len(), 1);
        assert!(
            !logical_catalog_hash(dir.path())
                .expect("logical hash")
                .is_empty()
        );
    }

    #[test]
    fn projection_refuses_dirty_catalog_root() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().expect("temp dir");
        // Pre-seed the catalog root so it is non-empty.
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn catalog_hash_is_deterministic_across_roots() {
        let table = canonical_table();
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_eq!(
            a.catalog_hash, b.catalog_hash,
            "same data must hash identically regardless of root"
        );
    }

    #[test]
    fn catalog_hash_changes_with_data_content() {
        // Two projections that differ only in one trade's price must hash
        // differently, proving the catalog hash covers the written data bytes
        // (not just file paths).
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        };
        let table_a = canonical_table();
        let csv_b = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,999.9,0.3,buy,0\n\
            2,1772323312219,617.9,0.1456,sell,0\n\
            3,1772323312236,617,0.1544,sell,0\n";
        let table_b = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity,
            csv_b,
            42,
            "ingest-run-test",
        )
        .expect("normalize variant");
        let dir_a = tempfile::TempDir::new().unwrap();
        let dir_b = tempfile::TempDir::new().unwrap();
        let a = project_canonical_trades_to_catalog(
            &table_a,
            &spec(),
            dir_a.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let b = project_canonical_trades_to_catalog(
            &table_b,
            &spec(),
            dir_b.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        assert_ne!(
            a.catalog_hash, b.catalog_hash,
            "different trade data must change the catalog hash"
        );
    }

    fn expected_hash_field(hasher: &mut Sha256, label: &str, value: &str) {
        hasher.update(label.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
        hasher.update([0xff]);
    }

    fn expected_hash_optional_field<T: ToString>(
        hasher: &mut Sha256,
        label: &str,
        value: Option<&T>,
    ) {
        match value {
            Some(value) => expected_hash_field(hasher, label, &value.to_string()),
            None => expected_hash_field(hasher, label, "<none>"),
        }
    }

    fn expected_hash_currency_pair(hasher: &mut Sha256, instrument: &CurrencyPair) {
        assert!(
            instrument.info.is_none(),
            "test fixture uses no opaque info"
        );
        expected_hash_field(hasher, "instrument.type", "currency_pair");
        expected_hash_field(hasher, "instrument.id", &instrument.id.to_string());
        expected_hash_field(
            hasher,
            "instrument.raw_symbol",
            instrument.raw_symbol.as_ref(),
        );
        expected_hash_field(
            hasher,
            "instrument.base_currency",
            &instrument.base_currency.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.quote_currency",
            &instrument.quote_currency.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.price_precision",
            &instrument.price_precision.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.size_precision",
            &instrument.size_precision.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.price_increment",
            &instrument.price_increment.as_decimal().to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.size_increment",
            &instrument.size_increment.as_decimal().to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.multiplier",
            &instrument.multiplier.as_decimal().to_string(),
        );
        expected_hash_optional_field(hasher, "instrument.lot_size", instrument.lot_size.as_ref());
        expected_hash_optional_field(
            hasher,
            "instrument.max_quantity",
            instrument.max_quantity.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.min_quantity",
            instrument.min_quantity.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.max_notional",
            instrument.max_notional.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.min_notional",
            instrument.min_notional.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.max_price",
            instrument.max_price.as_ref(),
        );
        expected_hash_optional_field(
            hasher,
            "instrument.min_price",
            instrument.min_price.as_ref(),
        );
        expected_hash_field(
            hasher,
            "instrument.margin_init",
            &instrument.margin_init.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.margin_maint",
            &instrument.margin_maint.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.maker_fee",
            &instrument.maker_fee.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.taker_fee",
            &instrument.taker_fee.to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.ts_event",
            &instrument.ts_event.as_u64().to_string(),
        );
        expected_hash_field(
            hasher,
            "instrument.ts_init",
            &instrument.ts_init.as_u64().to_string(),
        );
    }

    fn expected_logical_catalog_hash(instrument: &CurrencyPair, ticks: &[TradeTick]) -> String {
        let mut ticks = ticks.to_vec();
        ticks.sort_by_key(|tick| {
            (
                tick.ts_event.as_u64(),
                tick.trade_id.to_string(),
                tick.instrument_id.to_string(),
            )
        });
        let mut hasher = Sha256::new();
        hasher.update(b"nautilus-logical-catalog.v1");
        hasher.update([0u8]);
        expected_hash_currency_pair(&mut hasher, instrument);
        for tick in ticks {
            hasher.update([2u8]);
            hasher.update(tick.instrument_id.to_string().as_bytes());
            hasher.update([3u8]);
            hasher.update(tick.trade_id.to_string().as_bytes());
            hasher.update([4u8]);
            hasher.update(tick.price.as_decimal().to_string().as_bytes());
            hasher.update([5u8]);
            hasher.update(tick.size.as_decimal().to_string().as_bytes());
            hasher.update([6u8]);
            hasher.update(tick.aggressor_side.to_string().as_bytes());
            hasher.update([7u8]);
            hasher.update(tick.ts_event.as_u64().to_string().as_bytes());
            hasher.update([8u8]);
            hasher.update(tick.ts_init.as_u64().to_string().as_bytes());
        }
        hex::encode(hasher.finalize())
    }

    #[test]
    fn projection_rejects_empty_canonical_table() {
        // Reproduction pin for the zero-row vacuous-pass concern: an empty
        // canonical table must fail loud at validate() before any catalog
        // write, so read-back can never compare 0 == 0 against an accepted
        // record.
        let mut table = canonical_table();
        table.rows.clear();
        let error = table.validate().expect_err("empty table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical trades table is empty")
        );
    }

    #[test]
    fn logical_catalog_hash_reproduces_committed_pmxt_reference_catalog_hash() {
        // Hash-invariance regression pin: the committed PMXT reference catalog
        // hash was recorded under the pre-explicit-file-list query mechanics.
        // Recomputing over the committed bytes must keep producing the
        // recorded value, or committed ledger records silently stop
        // verifying against their catalogs.
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("repo root");
        let run_dir = repo_root.join(
            "specs/023-nt-research-analytics-platform/reference/pmxt-polymarket-selected-source-conversion/backtests/pmxt-run",
        );
        let metadata: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(run_dir.join("catalog-metadata.json"))
                .expect("read committed catalog metadata"),
        )
        .expect("parse committed catalog metadata");
        let recorded = metadata["catalog_hash"]
            .as_str()
            .expect("catalog_hash present in committed metadata");
        let recomputed =
            logical_catalog_hash(&run_dir.join("nt-catalog")).expect("recompute logical hash");
        assert_eq!(recomputed, recorded);
    }

    #[test]
    fn catalog_hash_matches_stable_currency_pair_fields() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        let instrument = build_currency_pair(&spec()).expect("instrument");
        let ticks = canonical_rows_to_trade_ticks(&table, &instrument).expect("ticks");
        assert_eq!(
            projection.catalog_hash,
            expected_logical_catalog_hash(&instrument, &ticks),
            "catalog hash must use explicit stable instrument fields, not Debug output"
        );
    }

    #[test]
    fn catalog_hash_ignores_writer_sidecar_files() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let projection = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            dir.path(),
            &test_catalog_encoding(),
        )
        .unwrap();
        fs::write(dir.path().join("writer-version.txt"), b"nt writer metadata").unwrap();
        assert_eq!(
            projection.catalog_hash,
            logical_catalog_hash(dir.path()).unwrap(),
            "catalog hash must describe logical catalog contents, not unrelated writer files"
        );
    }

    #[test]
    fn deterministic_trade_projector_reconciles_an_identical_stable_root() {
        let table = canonical_table();
        let dir = tempfile::TempDir::new().unwrap();
        let catalog_root = dir.path().join("catalog");

        let first = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            &catalog_root,
            &test_catalog_encoding(),
        )
        .expect("first projection");
        let second = project_canonical_trades_to_catalog(
            &table,
            &spec(),
            &catalog_root,
            &test_catalog_encoding(),
        )
        .expect("identical retry projection");

        assert_eq!(first, second);
        assert_eq!(
            logical_catalog_hash(&catalog_root).expect("hash reconciled catalog"),
            first.catalog_hash
        );
        assert_eq!(
            read_back_trade_ticks(&catalog_root, &first.nt_instrument_id)
                .expect("read reconciled catalog")
                .len(),
            table.rows.len()
        );
    }

    #[test]
    fn catalog_hash_ignores_unrelated_relative_paths() {
        // Valid non-catalog Parquet sidecars under different relative paths
        // must not affect the logical digest. The digest is over NT-read
        // catalog records, not filesystem layout, while the structural
        // preflight still validates every `.parquet` file it inventories.
        let table = canonical_table();
        let root_a = tempfile::TempDir::new().unwrap();
        let root_b = tempfile::TempDir::new().unwrap();
        project_canonical_trades_to_catalog(
            &table,
            &spec(),
            root_a.path(),
            &test_catalog_encoding(),
        )
        .expect("project first valid NT catalog");
        project_canonical_trades_to_catalog(
            &table,
            &spec(),
            root_b.path(),
            &test_catalog_encoding(),
        )
        .expect("project second valid NT catalog");
        fs::create_dir_all(root_a.path().join("data/alpha")).unwrap();
        table
            .write_parquet(
                &root_a.path().join("data/alpha/file.parquet"),
                &test_catalog_encoding(),
            )
            .expect("write first valid unrelated Parquet sidecar");
        fs::create_dir_all(root_b.path().join("data/beta")).unwrap();
        table
            .write_parquet(
                &root_b.path().join("data/beta/file.parquet"),
                &test_catalog_encoding(),
            )
            .expect("write second valid unrelated Parquet sidecar");
        assert_eq!(
            logical_catalog_hash(root_a.path()).unwrap(),
            logical_catalog_hash(root_b.path()).unwrap(),
            "unrelated bytes under different relative paths must not change the logical hash"
        );
    }

    #[test]
    fn failed_projection_retains_only_unique_temp_and_preserves_preexisting_root() {
        let parent = tempfile::TempDir::new().expect("temp dir");
        let root = parent.path().join("catalog");
        fs::create_dir(&root).expect("create caller-owned empty root");

        let error = with_clean_catalog_root_guarded(
            &root,
            &root,
            &test_catalog_encoding(),
            &OperatorWorkBudgetGuard::unbounded(),
            |_catalog, temp_root| -> Result<()> {
                fs::create_dir(temp_root.join("data"))?;
                fs::write(temp_root.join("data/incomplete.parquet"), b"incomplete")?;
                anyhow::bail!("injected projection failure")
            },
        )
        .expect_err("injected failure must fail projection");

        assert!(format!("{error:#}").contains("injected projection failure"));
        assert!(root.is_dir(), "caller-owned root must be preserved");
        assert!(
            fs::read_dir(&root)
                .expect("read preserved root")
                .next()
                .is_none(),
            "failed projection must not leak partial final data"
        );
        let retained = fs::read_dir(parent.path())
            .expect("read parent")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 1, "one failed unique root must be retained");
        assert_catalog_candidate_is_receipt_only(&retained[0]);
    }

    #[test]
    fn concurrent_projection_publish_has_one_create_only_winner() {
        let parent = tempfile::TempDir::new().expect("temp dir");
        let root = Arc::new(parent.path().join("catalog"));
        fs::create_dir(root.as_path()).expect("create caller-owned empty root");
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [b'A', b'B']
            .into_iter()
            .map(|payload| {
                let root = Arc::clone(&root);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    with_clean_catalog_root_guarded(
                        root.as_path(),
                        root.as_path(),
                        &test_catalog_encoding(),
                        &OperatorWorkBudgetGuard::unbounded(),
                        |_catalog, temp_root| {
                            fs::create_dir(temp_root.join("data"))?;
                            fs::write(temp_root.join("data/winner.parquet"), [payload])?;
                            barrier.wait();
                            Ok(payload)
                        },
                    )
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("publisher thread must not panic"))
            .collect();

        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            1,
            "create-only catalog publication must have one winner: {outcomes:?}"
        );
        let winner = fs::read(root.join("data/winner.parquet")).expect("read winner");
        assert!(winner == [b'A'] || winner == [b'B']);
        let retained = fs::read_dir(parent.path())
            .expect("read parent")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(
            retained.len(),
            2,
            "both unique roots must remain isolated after the publish race"
        );
        for candidate in &retained {
            assert_catalog_candidate_is_receipt_only(candidate);
        }
    }

    #[test]
    fn identical_catalog_retry_succeeds_and_compacts_both_candidates() {
        let parent = tempfile::TempDir::new().expect("temp dir");
        let root = parent.path().join("catalog");
        fs::create_dir(&root).expect("create caller-owned root");
        let publish = || {
            with_clean_catalog_root_guarded(
                &root,
                &root,
                &test_catalog_encoding(),
                &OperatorWorkBudgetGuard::unbounded(),
                |_catalog, temp_root| {
                    fs::create_dir(temp_root.join("data"))?;
                    fs::write(temp_root.join("data/catalog.parquet"), b"identical")?;
                    Ok(())
                },
            )
        };

        publish().expect("first publication");
        publish().expect("identical retry");

        let retained = fs::read_dir(parent.path())
            .expect("read parent")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 2, "each attempt keeps an ownership root");
        for candidate in &retained {
            assert_catalog_candidate_is_receipt_only(candidate);
        }
    }

    #[test]
    fn failed_projection_retains_candidate_outside_authoritative_output() {
        let parent = tempfile::TempDir::new().expect("temp dir");
        let output = parent.path().join("authoritative-output");
        let catalog_root = output.join("catalog");

        let error = with_clean_catalog_root_guarded(
            &catalog_root,
            &output,
            &test_catalog_encoding(),
            &OperatorWorkBudgetGuard::unbounded(),
            |_catalog, candidate_root| -> Result<()> {
                fs::create_dir(candidate_root.join("data"))?;
                fs::write(
                    candidate_root.join("data/incomplete.residue"),
                    b"incomplete",
                )?;
                anyhow::bail!("injected projection failure")
            },
        )
        .expect_err("injected failure must fail projection");

        assert!(format!("{error:#}").contains("injected projection failure"));
        assert!(output.is_dir(), "authoritative output must exist");
        assert!(
            fs::read_dir(&output)
                .expect("read authoritative output")
                .next()
                .is_none(),
            "failed candidate must never become an authoritative output entry"
        );
        let retained = fs::read_dir(parent.path())
            .expect("read output parent")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(retained.len(), 1, "one external candidate is retained");
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&retained[0])
                .expect("stat retained external candidate")
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "external candidate workspace must remain private"
        );
        assert_catalog_candidate_is_receipt_only(&retained[0]);
    }

    #[test]
    fn stale_external_candidate_cannot_gain_catalog_authority() {
        let parent = tempfile::TempDir::new().expect("temp dir");
        let output = parent.path().join("authoritative-output");
        let catalog_root = output.join("catalog");
        fs::create_dir_all(&output).expect("create output");
        let stale_candidate = parent.path().join("authoritative-output.stale.tmp");
        fs::create_dir_all(stale_candidate.join("data")).expect("create stale candidate");
        fs::write(
            stale_candidate.join("data/stale.parquet"),
            b"must-not-publish",
        )
        .expect("seed stale candidate");

        with_clean_catalog_root_guarded(
            &catalog_root,
            &output,
            &test_catalog_encoding(),
            &OperatorWorkBudgetGuard::unbounded(),
            |_catalog, candidate_root| {
                fs::create_dir(candidate_root.join("data"))?;
                fs::write(candidate_root.join("data/owned.parquet"), b"owned")?;
                Ok(())
            },
        )
        .expect("publish owned candidate");

        assert_eq!(
            fs::read(catalog_root.join("data/owned.parquet")).expect("read authoritative catalog"),
            b"owned"
        );
        assert!(
            !catalog_root.join("data/stale.parquet").exists(),
            "stale external residue must not gain reader authority"
        );
        assert_eq!(
            fs::read(stale_candidate.join("data/stale.parquet"))
                .expect("stale residue remains external"),
            b"must-not-publish"
        );
    }

    #[test]
    fn projection_rejects_a_mutated_candidate_receipt() {
        let parent = tempfile::TempDir::new().expect("temp dir");
        let root = parent.path().join("catalog");

        let error = with_clean_catalog_root_guarded(
            &root,
            &root,
            &test_catalog_encoding(),
            &OperatorWorkBudgetGuard::unbounded(),
            |_catalog, candidate_root| {
                fs::create_dir(candidate_root.join("data"))?;
                fs::write(
                    candidate_root.join(SOURCE_UNIVERSE_CANDIDATE_RECEIPT_FILE),
                    b"mutated",
                )?;
                Ok(())
            },
        )
        .expect_err("mutated candidate receipt must fail closed");

        assert!(format!("{error:#}").contains("unexpected length"));
        assert!(!root.join("data").exists());
    }
}
