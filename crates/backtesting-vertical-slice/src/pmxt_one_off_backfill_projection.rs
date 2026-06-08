//! PMXT one-off historical rows projected through NautilusTrader Polymarket APIs.
//!
//! PMXT is intentionally scoped as one-off backfill data. This module proves the
//! selected source rows can be transformed into NT-native objects without making
//! PMXT a canonical source-proof input or a reusable venue abstraction.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use arrow::array::{
    Array, BinaryArray, Decimal64Array, Decimal128Array, LargeBinaryArray, LargeStringArray,
    RecordBatch, StringArray, TimestampMicrosecondArray, TimestampMillisecondArray,
    TimestampNanosecondArray, TimestampSecondArray,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    identifiers::InstrumentId,
    instruments::{Instrument, InstrumentAny},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use nautilus_polymarket::{
    common::enums::PolymarketOrderSide,
    http::{
        models::GammaMarket,
        parse::{create_instrument_from_def, parse_gamma_market},
    },
    websocket::{
        messages::{
            PolymarketBookLevel, PolymarketBookSnapshot, PolymarketQuote, PolymarketQuotes,
        },
        parse::{parse_book_deltas, parse_book_snapshot},
    },
};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use sha2::{Digest, Sha256};
use ustr::Ustr;

use crate::{
    catalog_projection::logical_catalog_hash,
    selected_source_slice::{SelectedSourceSliceReport, SelectedSourceSliceUsageScope},
    source_proof::SourceProofUsageScope,
};

#[derive(Debug, Clone)]
pub struct PmxtOneOffProjectionRequest {
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub selected_condition_id: String,
    pub selected_token_id: String,
    pub gamma_markets: Vec<GammaMarket>,
    pub rows: Vec<PmxtOneOffSelectedRow>,
}

#[derive(Debug, Clone)]
pub struct PmxtSelectedSourceProjectionSpec {
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub selected_condition_id: String,
    pub selected_token_id: String,
    pub gamma_markets: Vec<GammaMarket>,
    pub selected_source_parquet_path: PathBuf,
    pub selected_source_report_path: PathBuf,
    pub schema: PmxtSelectedSourceSchema,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtSelectedSourceSchema {
    pub timestamp_received_column: String,
    pub timestamp_column: String,
    pub market_column: String,
    pub event_type_column: String,
    pub asset_id_column: String,
    pub bids_column: String,
    pub asks_column: String,
    pub price_column: String,
    pub size_column: String,
    pub side_column: String,
    pub best_bid_column: String,
    pub best_ask_column: String,
    pub buy_side: String,
    pub sell_side: String,
    pub book_event_type: String,
    pub price_change_event_type: String,
    pub ignored_event_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PmxtOneOffSelectedRow {
    BookSnapshot(PmxtOneOffSnapshotRow),
    PriceChange(PmxtPriceChangeRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtOneOffSnapshotRow {
    pub market: String,
    pub asset_id: String,
    pub bids: Vec<PmxtBookLevel>,
    pub asks: Vec<PmxtBookLevel>,
    pub timestamp_ms: String,
    pub ts_init: UnixNanos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtPriceChangeRow {
    pub market: String,
    pub asset_id: String,
    pub price: String,
    pub side: PmxtOneOffTickSide,
    pub size: String,
    pub best_bid: Option<String>,
    pub best_ask: Option<String>,
    pub timestamp_ms: String,
    pub ts_init: UnixNanos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PmxtOneOffTickSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtBookLevel {
    pub price: String,
    pub size: String,
}

#[derive(Debug, Clone)]
pub struct PmxtOneOffNtProjection {
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub instrument: InstrumentAny,
    pub order_book_deltas: Vec<OrderBookDelta>,
    pub trade_ticks: Vec<TradeTick>,
    pub nt_surfaces_used: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtOneOffCatalogProjection {
    pub catalog_root: PathBuf,
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub nt_instrument_id: String,
    pub order_book_delta_count: u64,
    pub trade_tick_count: u64,
    pub catalog_hash: String,
}

#[derive(Debug, Clone)]
pub struct PmxtSelectedSourceNtProjection {
    pub projection: PmxtOneOffNtProjection,
    pub selected_source_report_hash: String,
    pub selected_asset_ids_hash: String,
    pub selected_rows: u64,
    pub projected_l2_rows: u64,
    pub skipped_non_l2_rows: u64,
}

pub fn project_pmxt_selected_source_parquet_to_nt(
    spec: PmxtSelectedSourceProjectionSpec,
) -> Result<PmxtSelectedSourceNtProjection> {
    ensure!(
        spec.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT selected-source projection only accepts one_off_backfill_data usage_scope"
    );
    let report_bytes = fs::read(&spec.selected_source_report_path).with_context(|| {
        format!(
            "read selected-source report {}",
            spec.selected_source_report_path.display()
        )
    })?;
    let selected_source_report_hash = sha256_bytes(&report_bytes);
    let report: SelectedSourceSliceReport =
        serde_json::from_slice(&report_bytes).with_context(|| {
            format!(
                "parse selected-source report {}",
                spec.selected_source_report_path.display()
            )
        })?;
    ensure!(
        report.usage_scope == SelectedSourceSliceUsageScope::OneOffBackfillData,
        "selected-source report usage_scope must be one_off_backfill_data"
    );
    ensure!(
        report.output_parquet_path == spec.selected_source_parquet_path.display().to_string(),
        "selected-source report output_parquet_path {:?} does not match selected_source_parquet_path {:?}",
        report.output_parquet_path,
        spec.selected_source_parquet_path.display().to_string()
    );
    let selected_parquet_sha256 = sha256_file(&spec.selected_source_parquet_path)?;
    ensure!(
        report.output_parquet_sha256 == selected_parquet_sha256,
        "selected-source parquet sha256 mismatch: report {:?}, actual {:?}",
        report.output_parquet_sha256,
        selected_parquet_sha256
    );

    let decoded = decode_selected_source_rows(&spec, &report)?;
    let projected_l2_rows = decoded.rows.len() as u64;
    ensure!(
        projected_l2_rows > 0,
        "PMXT selected-source projection decoded zero L2 rows"
    );

    let projection = project_pmxt_one_off_rows_to_nt(PmxtOneOffProjectionRequest {
        source_binding: spec.source_binding,
        usage_scope: spec.usage_scope,
        selected_condition_id: spec.selected_condition_id,
        selected_token_id: spec.selected_token_id,
        gamma_markets: spec.gamma_markets,
        rows: decoded.rows,
    })?;

    Ok(PmxtSelectedSourceNtProjection {
        projection,
        selected_source_report_hash,
        selected_asset_ids_hash: report.selected_asset_ids_hash,
        selected_rows: decoded.total_rows,
        projected_l2_rows,
        skipped_non_l2_rows: decoded.skipped_non_l2_rows,
    })
}

#[derive(Debug, Clone)]
struct DecodedSelectedSourceRows {
    rows: Vec<PmxtOneOffSelectedRow>,
    total_rows: u64,
    skipped_non_l2_rows: u64,
}

fn decode_selected_source_rows(
    spec: &PmxtSelectedSourceProjectionSpec,
    report: &SelectedSourceSliceReport,
) -> Result<DecodedSelectedSourceRows> {
    let file = fs::File::open(&spec.selected_source_parquet_path).with_context(|| {
        format!(
            "open selected-source parquet {}",
            spec.selected_source_parquet_path.display()
        )
    })?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| {
            format!(
                "build selected-source parquet reader {}",
                spec.selected_source_parquet_path.display()
            )
        })?
        .build()
        .with_context(|| {
            format!(
                "build selected-source record batch reader {}",
                spec.selected_source_parquet_path.display()
            )
        })?;

    let mut rows = Vec::new();
    let mut total_rows = 0_u64;
    let mut skipped_non_l2_rows = 0_u64;
    for batch in reader {
        let batch = batch.with_context(|| {
            format!(
                "read selected-source parquet batch {}",
                spec.selected_source_parquet_path.display()
            )
        })?;
        decode_batch_rows(
            &batch,
            spec,
            &mut rows,
            &mut total_rows,
            &mut skipped_non_l2_rows,
        )?;
    }
    ensure!(
        total_rows == report.selected_rows,
        "selected-source report selected_rows {} does not match decoded rows {total_rows}",
        report.selected_rows
    );
    Ok(DecodedSelectedSourceRows {
        rows,
        total_rows,
        skipped_non_l2_rows,
    })
}

fn decode_batch_rows(
    batch: &RecordBatch,
    spec: &PmxtSelectedSourceProjectionSpec,
    rows: &mut Vec<PmxtOneOffSelectedRow>,
    total_rows: &mut u64,
    skipped_non_l2_rows: &mut u64,
) -> Result<()> {
    let schema = &spec.schema;
    for row in 0..batch.num_rows() {
        *total_rows = total_rows.saturating_add(1);
        let event_type = required_string(batch, &schema.event_type_column, row)?;
        if event_type == schema.book_event_type {
            rows.push(PmxtOneOffSelectedRow::BookSnapshot(PmxtOneOffSnapshotRow {
                market: required_market_string(batch, &schema.market_column, row)?,
                asset_id: required_string(batch, &schema.asset_id_column, row)?,
                bids: required_book_levels(batch, &schema.bids_column, row)?,
                asks: required_book_levels(batch, &schema.asks_column, row)?,
                timestamp_ms: timestamp_ms_string(required_timestamp_nanos(
                    batch,
                    &schema.timestamp_column,
                    row,
                )?)?,
                ts_init: required_timestamp_nanos(batch, &schema.timestamp_received_column, row)?,
            }));
        } else if event_type == schema.price_change_event_type {
            rows.push(PmxtOneOffSelectedRow::PriceChange(PmxtPriceChangeRow {
                market: required_market_string(batch, &schema.market_column, row)?,
                asset_id: required_string(batch, &schema.asset_id_column, row)?,
                price: required_decimal_string(batch, &schema.price_column, row)?,
                side: required_side(batch, schema, row)?,
                size: required_decimal_string(batch, &schema.size_column, row)?,
                best_bid: optional_decimal_string(batch, &schema.best_bid_column, row)?,
                best_ask: optional_decimal_string(batch, &schema.best_ask_column, row)?,
                timestamp_ms: timestamp_ms_string(required_timestamp_nanos(
                    batch,
                    &schema.timestamp_column,
                    row,
                )?)?,
                ts_init: required_timestamp_nanos(batch, &schema.timestamp_received_column, row)?,
            }));
        } else if schema
            .ignored_event_types
            .iter()
            .any(|ignored| ignored == &event_type)
        {
            *skipped_non_l2_rows = skipped_non_l2_rows.saturating_add(1);
        } else {
            bail!("unsupported PMXT selected-source event_type {event_type:?}");
        }
    }
    Ok(())
}

pub fn project_pmxt_one_off_rows_to_nt(
    request: PmxtOneOffProjectionRequest,
) -> Result<PmxtOneOffNtProjection> {
    ensure!(
        request.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT one-off projection only accepts one_off_backfill_data usage_scope"
    );

    let selected_def = request
        .gamma_markets
        .iter()
        .map(parse_gamma_market)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .find(|def| {
            def.condition_id.as_str() == request.selected_condition_id
                && def.token_id.as_str() == request.selected_token_id
        })
        .with_context(|| {
            format!(
                "NT Polymarket Gamma parser did not produce selected token {:?} for condition {:?}",
                request.selected_token_id, request.selected_condition_id
            )
        })?;

    let instrument = create_instrument_from_def(&selected_def, UnixNanos::default())
        .context("create NT Polymarket BinaryOption from selected Gamma definition")?;
    let (instrument_id, price_precision, size_precision) = binary_option_l2_metadata(&instrument)?;

    let mut order_book_deltas = Vec::new();
    let trade_ticks = Vec::new();
    let mut nt_surfaces_used = vec![
        "nautilus_polymarket::http::parse::parse_gamma_market".to_string(),
        "nautilus_polymarket::http::parse::create_instrument_from_def".to_string(),
    ];

    for row in request.rows {
        match row {
            PmxtOneOffSelectedRow::BookSnapshot(row) => {
                ensure_selected_row(
                    &row.market,
                    &row.asset_id,
                    &request.selected_condition_id,
                    &request.selected_token_id,
                )?;
                let snapshot = PolymarketBookSnapshot {
                    market: Ustr::from(row.market.as_str()),
                    asset_id: Ustr::from(row.asset_id.as_str()),
                    bids: row.bids.into_iter().map(Into::into).collect(),
                    asks: row.asks.into_iter().map(Into::into).collect(),
                    timestamp: row.timestamp_ms,
                };
                let parsed = parse_book_snapshot(
                    &snapshot,
                    instrument_id,
                    price_precision,
                    size_precision,
                    row.ts_init,
                )
                .context("parse PMXT one-off book snapshot with NT Polymarket parser")?;
                order_book_deltas.extend(parsed.deltas);
                push_surface_once(
                    &mut nt_surfaces_used,
                    "nautilus_polymarket::websocket::parse::parse_book_snapshot",
                );
            }
            PmxtOneOffSelectedRow::PriceChange(row) => {
                ensure_selected_row(
                    &row.market,
                    &row.asset_id,
                    &request.selected_condition_id,
                    &request.selected_token_id,
                )?;
                let quotes = PolymarketQuotes {
                    market: Ustr::from(row.market.as_str()),
                    price_changes: vec![PolymarketQuote {
                        asset_id: Ustr::from(row.asset_id.as_str()),
                        price: row.price,
                        side: row.side.into(),
                        size: row.size,
                        hash: String::new(),
                        best_bid: row.best_bid,
                        best_ask: row.best_ask,
                    }],
                    timestamp: row.timestamp_ms,
                };
                let parsed = parse_book_deltas(
                    &quotes,
                    instrument_id,
                    price_precision,
                    size_precision,
                    row.ts_init,
                )
                .context("parse PMXT one-off price_change with NT Polymarket parser")?;
                order_book_deltas.extend(parsed.deltas);
                push_surface_once(
                    &mut nt_surfaces_used,
                    "nautilus_polymarket::websocket::parse::parse_book_deltas",
                );
            }
        }
    }

    Ok(PmxtOneOffNtProjection {
        source_binding: request.source_binding,
        usage_scope: request.usage_scope,
        instrument,
        order_book_deltas,
        trade_ticks,
        nt_surfaces_used,
    })
}

pub fn write_pmxt_one_off_projection_to_catalog(
    catalog_root: &Path,
    projection: &PmxtOneOffNtProjection,
) -> Result<PmxtOneOffCatalogProjection> {
    ensure!(
        projection.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT one-off catalog projection only accepts one_off_backfill_data usage_scope"
    );
    ensure_clean_catalog_root(catalog_root)?;
    fs::create_dir_all(catalog_root).with_context(|| {
        format!(
            "create PMXT one-off catalog root {}",
            catalog_root.display()
        )
    })?;

    let instrument_id = projection.instrument.id().to_string();
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_instruments(vec![projection.instrument.clone()])
        .context("write PMXT one-off instrument to NT catalog")?;
    if !projection.order_book_deltas.is_empty() {
        catalog
            .write_to_parquet(projection.order_book_deltas.clone(), None, None, None)
            .context("write PMXT one-off L2 deltas to NT catalog")?;
    }
    if !projection.trade_ticks.is_empty() {
        catalog
            .write_to_parquet(projection.trade_ticks.clone(), None, None, None)
            .context("write PMXT one-off trade ticks to NT catalog")?;
    }

    Ok(PmxtOneOffCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        source_binding: projection.source_binding.clone(),
        usage_scope: projection.usage_scope,
        nt_instrument_id: instrument_id,
        order_book_delta_count: projection.order_book_deltas.len() as u64,
        trade_tick_count: projection.trade_ticks.len() as u64,
        catalog_hash: logical_catalog_hash(catalog_root)?,
    })
}

fn binary_option_l2_metadata(instrument: &InstrumentAny) -> Result<(InstrumentId, u8, u8)> {
    match instrument {
        InstrumentAny::BinaryOption(binary_option) => Ok((
            binary_option.id(),
            binary_option.price_precision(),
            binary_option.size_precision(),
        )),
        other => bail!("expected NT Polymarket parser to produce BinaryOption, got {other:?}"),
    }
}

fn ensure_selected_row(
    market: &str,
    asset_id: &str,
    selected_condition_id: &str,
    selected_token_id: &str,
) -> Result<()> {
    ensure!(
        market == selected_condition_id,
        "PMXT one-off row market {market:?} does not match selected condition {selected_condition_id:?}"
    );
    ensure!(
        asset_id == selected_token_id,
        "PMXT one-off row asset_id {asset_id:?} does not match selected token {selected_token_id:?}"
    );
    Ok(())
}

fn ensure_clean_catalog_root(catalog_root: &Path) -> Result<()> {
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    Ok(())
}

fn required_side(
    batch: &RecordBatch,
    schema: &PmxtSelectedSourceSchema,
    row: usize,
) -> Result<PmxtOneOffTickSide> {
    let raw = required_string(batch, &schema.side_column, row)?;
    if raw == schema.buy_side {
        Ok(PmxtOneOffTickSide::Buy)
    } else if raw == schema.sell_side {
        Ok(PmxtOneOffTickSide::Sell)
    } else {
        bail!(
            "PMXT selected-source side {raw:?} is neither configured buy_side {:?} nor sell_side {:?}",
            schema.buy_side,
            schema.sell_side
        )
    }
}

fn required_book_levels(
    batch: &RecordBatch,
    column: &str,
    row: usize,
) -> Result<Vec<PmxtBookLevel>> {
    let raw = required_string(batch, column, row)?;
    let pairs: Vec<(String, String)> = serde_json::from_str(&raw)
        .with_context(|| format!("parse PMXT book level JSON column {column:?} row {row}"))?;
    ensure!(
        !pairs.is_empty(),
        "PMXT book level column {column:?} row {row} is empty"
    );
    Ok(pairs
        .into_iter()
        .map(|(price, size)| PmxtBookLevel { price, size })
        .collect())
}

fn required_timestamp_nanos(batch: &RecordBatch, column: &str, row: usize) -> Result<UnixNanos> {
    let values = required_column(batch, column)?;
    ensure!(
        !values.is_null(row),
        "selected-source column {column:?} has null at row {row}"
    );
    let nanos =
        if let Some(array) = values.as_any().downcast_ref::<TimestampNanosecondArray>() {
            array.value(row)
        } else if let Some(array) = values.as_any().downcast_ref::<TimestampMicrosecondArray>() {
            array.value(row).checked_mul(1_000).with_context(|| {
                format!("timestamp microsecond overflow in {column:?} row {row}")
            })?
        } else if let Some(array) = values.as_any().downcast_ref::<TimestampMillisecondArray>() {
            array.value(row).checked_mul(1_000_000).with_context(|| {
                format!("timestamp millisecond overflow in {column:?} row {row}")
            })?
        } else if let Some(array) = values.as_any().downcast_ref::<TimestampSecondArray>() {
            array
                .value(row)
                .checked_mul(1_000_000_000)
                .with_context(|| format!("timestamp second overflow in {column:?} row {row}"))?
        } else {
            bail!("selected-source column {column:?} is not an Arrow timestamp")
        };
    ensure!(
        nanos >= 0,
        "selected-source timestamp column {column:?} row {row} is negative"
    );
    Ok(UnixNanos::from(nanos as u64))
}

fn timestamp_ms_string(timestamp: UnixNanos) -> Result<String> {
    let nanos = timestamp.as_u64();
    ensure!(
        nanos.is_multiple_of(1_000_000),
        "PMXT selected-source timestamp {nanos}ns is not millisecond aligned"
    );
    Ok((nanos / 1_000_000).to_string())
}

fn required_market_string(batch: &RecordBatch, column: &str, row: usize) -> Result<String> {
    let values = required_column(batch, column)?;
    ensure!(
        !values.is_null(row),
        "selected-source column {column:?} has null at row {row}"
    );
    if let Some(strings) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(strings.value(row).to_string());
    }
    if let Some(strings) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(strings.value(row).to_string());
    }
    if let Some(bytes) = values.as_any().downcast_ref::<BinaryArray>() {
        return std::str::from_utf8(bytes.value(row))
            .map(str::to_string)
            .with_context(|| format!("decode binary market column {column:?} row {row}"));
    }
    if let Some(bytes) = values.as_any().downcast_ref::<LargeBinaryArray>() {
        return std::str::from_utf8(bytes.value(row))
            .map(str::to_string)
            .with_context(|| format!("decode large binary market column {column:?} row {row}"));
    }
    bail!("selected-source market column {column:?} is not Utf8/LargeUtf8/Binary/LargeBinary")
}

fn required_string(batch: &RecordBatch, column: &str, row: usize) -> Result<String> {
    optional_string(batch, column, row)?
        .with_context(|| format!("selected-source column {column:?} has null at row {row}"))
}

fn optional_string(batch: &RecordBatch, column: &str, row: usize) -> Result<Option<String>> {
    let values = required_column(batch, column)?;
    if values.is_null(row) {
        return Ok(None);
    }
    if let Some(strings) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    if let Some(strings) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    bail!("selected-source column {column:?} is not Utf8 or LargeUtf8")
}

fn required_decimal_string(batch: &RecordBatch, column: &str, row: usize) -> Result<String> {
    optional_decimal_string(batch, column, row)?
        .with_context(|| format!("selected-source column {column:?} has null at row {row}"))
}

fn optional_decimal_string(
    batch: &RecordBatch,
    column: &str,
    row: usize,
) -> Result<Option<String>> {
    let values = required_column(batch, column)?;
    if values.is_null(row) {
        return Ok(None);
    }
    if let Some(decimal) = values.as_any().downcast_ref::<Decimal128Array>() {
        return Ok(Some(decimal_to_string(
            decimal.value(row),
            decimal.scale(),
        )?));
    }
    if let Some(decimal) = values.as_any().downcast_ref::<Decimal64Array>() {
        return Ok(Some(decimal_to_string(
            decimal.value(row) as i128,
            decimal.scale(),
        )?));
    }
    if let Some(strings) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    if let Some(strings) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    bail!("selected-source column {column:?} is not Decimal128/Decimal64/Utf8/LargeUtf8")
}

fn decimal_to_string(value: i128, scale: i8) -> Result<String> {
    ensure!(scale >= 0, "negative decimal scale {scale}");
    let scale = scale as usize;
    if scale == 0 {
        return Ok(value.to_string());
    }
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.abs().to_string();
    if absolute.len() <= scale {
        let padded = format!("{:0>width$}", absolute, width = scale + 1);
        let split = padded.len() - scale;
        Ok(format!("{sign}{}.{}", &padded[..split], &padded[split..]))
    } else {
        let split = absolute.len() - scale;
        Ok(format!(
            "{sign}{}.{}",
            &absolute[..split],
            &absolute[split..]
        ))
    }
}

fn required_column<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a dyn Array> {
    batch
        .column_by_name(column)
        .map(|array| array.as_ref())
        .with_context(|| format!("selected-source parquet missing column {column:?}"))
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("read file for sha256 {}", path.display()))?;
    Ok(sha256_bytes(&bytes))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn push_surface_once(surfaces: &mut Vec<String>, surface: &str) {
    if !surfaces.iter().any(|existing| existing == surface) {
        surfaces.push(surface.to_string());
    }
}

impl From<PmxtBookLevel> for PolymarketBookLevel {
    fn from(value: PmxtBookLevel) -> Self {
        Self {
            price: value.price,
            size: value.size,
        }
    }
}

impl From<PmxtOneOffTickSide> for PolymarketOrderSide {
    fn from(value: PmxtOneOffTickSide) -> Self {
        match value {
            PmxtOneOffTickSide::Buy => Self::Buy,
            PmxtOneOffTickSide::Sell => Self::Sell,
        }
    }
}
