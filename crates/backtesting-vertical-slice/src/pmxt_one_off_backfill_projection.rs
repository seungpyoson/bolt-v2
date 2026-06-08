//! PMXT one-off historical rows projected through NautilusTrader Polymarket APIs.
//!
//! PMXT is intentionally scoped as one-off backfill data. This module proves the
//! selected source rows can be transformed into NT-native objects without making
//! PMXT a canonical source-proof input or a reusable venue abstraction.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use arrow::array::{
    Array, BinaryArray, BinaryViewArray, Decimal64Array, Decimal128Array, FixedSizeBinaryArray,
    LargeBinaryArray, LargeStringArray, RecordBatch, StringArray, StringViewArray,
    TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray,
};
use nautilus_backtest::result::BacktestResult;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    enums::AggressorSide,
    identifiers::InstrumentId,
    identifiers::TradeId,
    instruments::{Instrument, InstrumentAny},
    types::{Price, Quantity},
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
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use ustr::Ustr;

use crate::{
    catalog_projection::logical_catalog_hash,
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_CHECKPOINT_FILE, CONVERSION_MANIFEST_FILE,
        ConversionCatalogMetadata, ConversionCheckpoint, ConversionFingerprint, ConversionManifest,
        ConversionOutputState, inspect_conversion_output, write_completed_conversion_artifacts,
    },
    first_proof_selector::{FirstProofSelectorReport, FirstProofSelectorStatus},
    result_contract::{
        BacktestResultContract, ResultArtifactUris, ResultContractInputs, build_result_contract,
    },
    run_manifest::{BacktestingRunManifest, MarketStructureFixture, parse_manifest_toml},
    runner::{
        iterations_mismatch, market_structure_label, nt_extension_surface_claim_limits,
        result_contract_warnings, run_nt_backtest_node, run_purpose_label,
    },
    selected_source_slice::{SelectedSourceSliceReport, SelectedSourceSliceUsageScope},
    source_proof::{AcceptanceMode, SourceProofFidelityClass, SourceProofUsageScope},
};

/// NautilusTrader data type written by the PMXT one-off L2 projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";
pub const PMXT_ONE_OFF_RESULT_CONTRACT_FILE: &str = "backtest-result-contract.json";

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub last_trade_price_event_type: Option<String>,
    #[serde(default)]
    pub transaction_hash_column: Option<String>,
    #[serde(default)]
    pub fee_rate_bps_column: Option<String>,
    pub ignored_event_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PmxtOneOffSelectedRow {
    BookSnapshot(PmxtOneOffSnapshotRow),
    PriceChange(PmxtPriceChangeRow),
    LastTrade(PmxtOneOffTradeRow),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtOneOffTradeRow {
    pub market: String,
    pub asset_id: String,
    pub transaction_hash: String,
    pub price: String,
    pub side: PmxtOneOffTickSide,
    pub size: String,
    pub fee_rate_bps: String,
    pub timestamp: UnixNanos,
    pub ts_init: UnixNanos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub trade_dedupe_provenance: Vec<PmxtTradeDedupeProvenance>,
    pub nt_surfaces_used: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtTradeDedupeProvenance {
    pub trade_id: String,
    pub transaction_hash: String,
    pub asset_id: String,
    pub duplicate_count: u64,
    pub earliest_ts_init: UnixNanos,
    pub max_ts_init: UnixNanos,
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
    pub selected_source_parquet_hash: String,
    pub event_count_ledger_hash: String,
    pub selected_asset_ids_hash: String,
    pub selected_rows: u64,
    pub projected_l2_rows: u64,
    pub skipped_non_l2_rows: u64,
}

#[derive(Debug, Clone)]
pub struct PmxtOneOffConversionProjectionSpec {
    pub output_dir: PathBuf,
    pub catalog_root: PathBuf,
    pub projection: PmxtOneOffNtProjection,
    pub fingerprint: ConversionFingerprint,
    pub normalized_schema_version: String,
    pub output_catalog_uri: String,
    pub execution_catalog_uri: String,
    pub direct_s3_catalog_access_proven: bool,
    pub completed_at: String,
}

#[derive(Debug, Clone)]
pub struct PmxtOneOffCompletedConversionProjection {
    pub catalog_projection: PmxtOneOffCatalogProjection,
    pub conversion_checkpoint: ConversionCheckpoint,
    pub conversion_manifest: ConversionManifest,
    pub conversion_catalog_metadata: ConversionCatalogMetadata,
    pub conversion_checkpoint_hash: String,
    pub conversion_manifest_hash: String,
    pub conversion_catalog_metadata_hash: String,
}

#[derive(Debug, Clone)]
pub struct PmxtOneOffBacktestContractSpec<'a> {
    pub completed: &'a PmxtOneOffCompletedConversionProjection,
    pub manifest: &'a BacktestingRunManifest,
    pub manifest_hash: &'a str,
    pub acceptance_mode: AcceptanceMode,
    pub accepted_by: &'a str,
    pub accepted_at: &'a str,
    pub event_count_ledger_hash: &'a str,
    pub selected_asset_ids_hash: &'a str,
    pub artifact_uris: ResultArtifactUris,
    pub created_at: &'a str,
    pub claim_limits: Vec<String>,
}

#[derive(Debug)]
pub struct PmxtOneOffBacktestContractOutput {
    pub nt_result: BacktestResult,
    pub contract: BacktestResultContract,
}

#[derive(Debug, Clone)]
pub struct PmxtOneOffArtifactRootRunSpec {
    pub selected_source: PmxtSelectedSourceProjectionSpec,
    pub output_dir: PathBuf,
    pub catalog_root: PathBuf,
    pub fingerprint: ConversionFingerprint,
    pub manifest: BacktestingRunManifest,
    pub manifest_hash: String,
    pub normalized_schema_version: String,
    pub output_catalog_uri: String,
    pub execution_catalog_uri: String,
    pub direct_s3_catalog_access_proven: bool,
    pub acceptance_mode: AcceptanceMode,
    pub accepted_by: String,
    pub accepted_at: String,
    pub artifact_uris: ResultArtifactUris,
    pub created_at: String,
    pub claim_limits: Vec<String>,
}

#[derive(Debug)]
pub struct PmxtOneOffArtifactRootRun {
    pub selected_projection: PmxtSelectedSourceNtProjection,
    pub completed: PmxtOneOffCompletedConversionProjection,
    pub contract_output: PmxtOneOffBacktestContractOutput,
    pub result_contract_path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PmxtOneOffArtifactRootRunTomlSpec {
    pub selected_source: PmxtSelectedSourceProjectionTomlSpec,
    pub output_dir: PathBuf,
    pub catalog_root: PathBuf,
    pub fingerprint: ConversionFingerprint,
    pub manifest_path: PathBuf,
    pub normalized_schema_version: String,
    pub direct_s3_catalog_access_proven: bool,
    pub acceptance_mode: AcceptanceMode,
    pub accepted_by: String,
    pub accepted_at: String,
    pub artifact_uris: ResultArtifactUris,
    pub created_at: String,
    pub claim_limits: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PmxtSelectedSourceProjectionTomlSpec {
    pub source_binding: String,
    pub usage_scope: SourceProofUsageScope,
    pub selected_condition_id: String,
    pub selected_token_id: String,
    pub gamma_markets_json_path: PathBuf,
    pub selected_source_parquet_path: PathBuf,
    pub selected_source_report_path: PathBuf,
    pub schema: PmxtSelectedSourceSchema,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PmxtOneOffArtifactRootRunArtifact {
    pub output_dir: PathBuf,
    pub result_contract_path: PathBuf,
    pub result_contract_hash: String,
    pub conversion_manifest_hash: String,
    pub catalog_hash: String,
    pub selected_source_parquet_hash: String,
    pub event_count_ledger_hash: String,
    pub selected_asset_ids_hash: String,
    pub projected_l2_rows: u64,
    pub nt_iterations: usize,
}

pub fn write_pmxt_one_off_l2_artifact_root_run_from_spec_file(
    spec_path: &Path,
) -> Result<PmxtOneOffArtifactRootRunArtifact> {
    let spec_text = fs::read_to_string(spec_path).with_context(|| {
        format!(
            "read PMXT one-off artifact-root spec {}",
            spec_path.display()
        )
    })?;
    let spec: PmxtOneOffArtifactRootRunTomlSpec =
        toml::from_str(&spec_text).with_context(|| {
            format!(
                "parse PMXT one-off artifact-root spec {}",
                spec_path.display()
            )
        })?;
    write_pmxt_one_off_l2_artifact_root_run_from_toml_spec(spec)
}

pub fn write_pmxt_one_off_l2_artifact_root_run_from_toml_spec(
    spec: PmxtOneOffArtifactRootRunTomlSpec,
) -> Result<PmxtOneOffArtifactRootRunArtifact> {
    let gamma_markets_bytes = fs::read(&spec.selected_source.gamma_markets_json_path)
        .with_context(|| {
            format!(
                "read PMXT Gamma metadata {}",
                spec.selected_source.gamma_markets_json_path.display()
            )
        })?;
    let gamma_markets: Vec<GammaMarket> = serde_json::from_slice(&gamma_markets_bytes)
        .with_context(|| {
            format!(
                "parse PMXT Gamma metadata {}",
                spec.selected_source.gamma_markets_json_path.display()
            )
        })?;
    let manifest_text = fs::read_to_string(&spec.manifest_path).with_context(|| {
        format!(
            "read PMXT one-off manifest {}",
            spec.manifest_path.display()
        )
    })?;
    let manifest = parse_manifest_toml(&manifest_text).with_context(|| {
        format!(
            "parse PMXT one-off manifest {}",
            spec.manifest_path.display()
        )
    })?;
    ensure!(
        manifest.catalog_input.catalog_path == spec.catalog_root.display().to_string(),
        "PMXT one-off manifest catalog_path {:?} must match spec catalog_root {:?}",
        manifest.catalog_input.catalog_path,
        spec.catalog_root.display().to_string()
    );
    let output_catalog_uri = format!("file://{}", spec.catalog_root.display());
    let execution_catalog_uri = spec.catalog_root.display().to_string();
    let manifest_hash = manifest.manifest_hash();
    let run = write_pmxt_one_off_l2_artifact_root_run(PmxtOneOffArtifactRootRunSpec {
        selected_source: PmxtSelectedSourceProjectionSpec {
            source_binding: spec.selected_source.source_binding,
            usage_scope: spec.selected_source.usage_scope,
            selected_condition_id: spec.selected_source.selected_condition_id,
            selected_token_id: spec.selected_source.selected_token_id,
            gamma_markets,
            selected_source_parquet_path: spec.selected_source.selected_source_parquet_path,
            selected_source_report_path: spec.selected_source.selected_source_report_path,
            schema: spec.selected_source.schema,
        },
        output_dir: spec.output_dir.clone(),
        catalog_root: spec.catalog_root,
        fingerprint: spec.fingerprint,
        manifest,
        manifest_hash,
        normalized_schema_version: spec.normalized_schema_version,
        output_catalog_uri,
        execution_catalog_uri,
        direct_s3_catalog_access_proven: spec.direct_s3_catalog_access_proven,
        acceptance_mode: spec.acceptance_mode,
        accepted_by: spec.accepted_by,
        accepted_at: spec.accepted_at,
        artifact_uris: spec.artifact_uris,
        created_at: spec.created_at,
        claim_limits: spec.claim_limits,
    })?;
    Ok(PmxtOneOffArtifactRootRunArtifact {
        output_dir: spec.output_dir,
        result_contract_hash: sha256_file(&run.result_contract_path)?,
        result_contract_path: run.result_contract_path,
        conversion_manifest_hash: run.completed.conversion_manifest_hash,
        catalog_hash: run.completed.catalog_projection.catalog_hash,
        selected_source_parquet_hash: run.selected_projection.selected_source_parquet_hash,
        event_count_ledger_hash: run.selected_projection.event_count_ledger_hash,
        selected_asset_ids_hash: run.selected_projection.selected_asset_ids_hash,
        projected_l2_rows: run.selected_projection.projected_l2_rows,
        nt_iterations: run.contract_output.nt_result.iterations,
    })
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
    let selector_report = read_selected_source_selector_report(&report)?;
    ensure!(
        selector_report.selected_asset_ids_hash == report.selected_asset_ids_hash,
        "selected-source report selected_asset_ids_hash {:?} does not match selector report {:?}",
        report.selected_asset_ids_hash,
        selector_report.selected_asset_ids_hash
    );
    ensure!(
        selector_report.selected_assets.len() as u64 == report.selected_asset_count,
        "selected-source report selected_asset_count {} does not match selector report {}",
        report.selected_asset_count,
        selector_report.selected_assets.len()
    );
    ensure!(
        !selector_report.event_count_ledger_hash.trim().is_empty(),
        "selector report event_count_ledger_hash must not be empty"
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
        selected_source_parquet_hash: selected_parquet_sha256,
        event_count_ledger_hash: selector_report.event_count_ledger_hash,
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

fn read_selected_source_selector_report(
    report: &SelectedSourceSliceReport,
) -> Result<FirstProofSelectorReport> {
    let selector_path = Path::new(&report.selector_report_path);
    let selector_bytes = fs::read(selector_path)
        .with_context(|| format!("read selector report {}", selector_path.display()))?;
    let selector_sha256 = sha256_bytes(&selector_bytes);
    ensure!(
        selector_sha256 == report.selector_report_sha256,
        "selected-source selector report sha256 mismatch: report {:?}, actual {:?}",
        report.selector_report_sha256,
        selector_sha256
    );
    let selector_report: FirstProofSelectorReport = serde_json::from_slice(&selector_bytes)
        .with_context(|| format!("parse selector report {}", selector_path.display()))?;
    ensure!(
        selector_report.status == FirstProofSelectorStatus::Selected,
        "selector report status must be selected for PMXT one-off projection"
    );
    Ok(selector_report)
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
        } else if schema.last_trade_price_event_type.as_deref() == Some(event_type.as_str()) {
            let transaction_hash_column = schema
                .transaction_hash_column
                .as_deref()
                .context("PMXT selected-source schema missing transaction_hash_column")?;
            let fee_rate_bps_column = schema
                .fee_rate_bps_column
                .as_deref()
                .context("PMXT selected-source schema missing fee_rate_bps_column")?;
            rows.push(PmxtOneOffSelectedRow::LastTrade(PmxtOneOffTradeRow {
                market: required_market_string(batch, &schema.market_column, row)?,
                asset_id: required_string(batch, &schema.asset_id_column, row)?,
                transaction_hash: required_string(batch, transaction_hash_column, row)?,
                price: required_decimal_string(batch, &schema.price_column, row)?,
                side: required_side(batch, schema, row)?,
                size: required_decimal_string(batch, &schema.size_column, row)?,
                fee_rate_bps: required_decimal_string(batch, fee_rate_bps_column, row)?,
                timestamp: required_timestamp_nanos(batch, &schema.timestamp_column, row)?,
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
    let mut trade_rows = Vec::new();
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
            PmxtOneOffSelectedRow::LastTrade(row) => {
                ensure_selected_row(
                    &row.market,
                    &row.asset_id,
                    &request.selected_condition_id,
                    &request.selected_token_id,
                )?;
                trade_rows.push(row);
                push_surface_once(&mut nt_surfaces_used, "nautilus_model::data::TradeTick");
                push_surface_once(
                    &mut nt_surfaces_used,
                    "nautilus_model::types::{Price,Quantity}",
                );
                push_surface_once(
                    &mut nt_surfaces_used,
                    "pinned_nt_polymarket_http_data_api_trade_id_shape_mirrored",
                );
            }
        }
    }
    let (trade_ticks, trade_dedupe_provenance) =
        project_pmxt_trade_rows_to_nt(instrument_id, price_precision, size_precision, trade_rows)?;

    Ok(PmxtOneOffNtProjection {
        source_binding: request.source_binding,
        usage_scope: request.usage_scope,
        instrument,
        order_book_deltas,
        trade_ticks,
        trade_dedupe_provenance,
        nt_surfaces_used,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PmxtSemanticTradeKey {
    transaction_hash: String,
    asset_id: String,
    timestamp: u64,
    price: String,
    size: String,
    side: PmxtOneOffTickSide,
    fee_rate_bps: String,
}

#[derive(Debug, Clone)]
struct PmxtTradeGroup {
    row: PmxtOneOffTradeRow,
    duplicate_count: u64,
    earliest_ts_init: UnixNanos,
    max_ts_init: UnixNanos,
}

fn project_pmxt_trade_rows_to_nt(
    instrument_id: InstrumentId,
    price_precision: u8,
    size_precision: u8,
    rows: Vec<PmxtOneOffTradeRow>,
) -> Result<(Vec<TradeTick>, Vec<PmxtTradeDedupeProvenance>)> {
    let mut groups = Vec::<PmxtTradeGroup>::new();
    let mut group_index = HashMap::<PmxtSemanticTradeKey, usize>::new();
    for row in rows {
        ensure!(
            !row.transaction_hash.trim().is_empty(),
            "PMXT last_trade_price row missing transaction_hash"
        );
        ensure!(
            row.transaction_hash.is_ascii(),
            "PMXT last_trade_price transaction_hash must be ASCII to mirror pinned NT trade id slicing"
        );
        ensure!(
            row.asset_id.is_ascii(),
            "PMXT last_trade_price asset_id must be ASCII to mirror pinned NT trade id slicing"
        );
        let key = PmxtSemanticTradeKey {
            transaction_hash: row.transaction_hash.clone(),
            asset_id: row.asset_id.clone(),
            timestamp: row.timestamp.as_u64(),
            price: row.price.clone(),
            size: row.size.clone(),
            side: row.side,
            fee_rate_bps: row.fee_rate_bps.clone(),
        };
        if let Some(index) = group_index.get(&key).copied() {
            let group = &mut groups[index];
            group.duplicate_count = group.duplicate_count.saturating_add(1);
            if row.ts_init < group.earliest_ts_init {
                group.earliest_ts_init = row.ts_init;
            }
            if row.ts_init > group.max_ts_init {
                group.max_ts_init = row.ts_init;
            }
        } else {
            let index = groups.len();
            group_index.insert(key, index);
            groups.push(PmxtTradeGroup {
                earliest_ts_init: row.ts_init,
                max_ts_init: row.ts_init,
                row,
                duplicate_count: 1,
            });
        }
    }

    let mut tx_asset_counts = HashMap::<(String, String), u32>::new();
    let mut ticks = Vec::with_capacity(groups.len());
    let mut provenance = Vec::new();
    for group in groups {
        let key = (
            group.row.transaction_hash.clone(),
            group.row.asset_id.clone(),
        );
        let seq = *tx_asset_counts
            .entry(key)
            .and_modify(|count| *count = count.saturating_add(1))
            .or_insert(0);
        let trade_id =
            build_pmxt_historical_trade_id(&group.row.transaction_hash, &group.row.asset_id, seq)?;
        let price = parse_pmxt_trade_price(&group.row.price, price_precision)?;
        let size = parse_pmxt_trade_quantity(&group.row.size, size_precision)?;
        let tick = TradeTick::new_checked(
            instrument_id,
            price,
            size,
            AggressorSide::from(PolymarketOrderSide::from(group.row.side)),
            TradeId::new(trade_id.as_str()),
            group.row.timestamp,
            group.earliest_ts_init,
        )
        .context("create PMXT one-off NT TradeTick")?;
        if group.duplicate_count > 1 {
            provenance.push(PmxtTradeDedupeProvenance {
                trade_id: trade_id.clone(),
                transaction_hash: group.row.transaction_hash,
                asset_id: group.row.asset_id,
                duplicate_count: group.duplicate_count,
                earliest_ts_init: group.earliest_ts_init,
                max_ts_init: group.max_ts_init,
            });
        }
        ticks.push(tick);
    }
    Ok((ticks, provenance))
}

fn build_pmxt_historical_trade_id(
    transaction_hash: &str,
    asset_id: &str,
    seq: u32,
) -> Result<String> {
    ensure!(
        transaction_hash.is_ascii(),
        "PMXT transaction_hash must be ASCII"
    );
    ensure!(asset_id.is_ascii(), "PMXT asset_id must be ASCII");
    let hash_suffix = ascii_suffix(transaction_hash, 24)?;
    let asset_suffix = ascii_suffix(asset_id, 4)?;
    Ok(format!("{hash_suffix}-{asset_suffix}-{seq:06}"))
}

fn ascii_suffix(value: &str, max_len: usize) -> Result<&str> {
    ensure!(value.is_ascii(), "PMXT trade id component must be ASCII");
    if value.len() > max_len {
        Ok(&value[value.len() - max_len..])
    } else {
        Ok(value)
    }
}

fn parse_pmxt_trade_price(raw: &str, precision: u8) -> Result<Price> {
    let value = raw
        .parse::<f64>()
        .with_context(|| format!("parse PMXT trade price {raw:?}"))?;
    Price::new_checked(value, precision)
        .map_err(|error| anyhow::anyhow!("create NT Price from PMXT trade price {raw:?}: {error}"))
}

fn parse_pmxt_trade_quantity(raw: &str, precision: u8) -> Result<Quantity> {
    let value = raw
        .parse::<f64>()
        .with_context(|| format!("parse PMXT trade size {raw:?}"))?;
    Quantity::new_checked(value, precision).map_err(|error| {
        anyhow::anyhow!("create NT Quantity from PMXT trade size {raw:?}: {error}")
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

pub fn write_pmxt_one_off_conversion_projection(
    spec: PmxtOneOffConversionProjectionSpec,
) -> Result<PmxtOneOffCompletedConversionProjection> {
    ensure!(
        spec.projection.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT one-off conversion projection only accepts one_off_backfill_data usage_scope"
    );
    ensure!(
        !spec.normalized_schema_version.trim().is_empty(),
        "PMXT one-off conversion projection missing normalized_schema_version"
    );
    ensure!(
        !spec.output_catalog_uri.trim().is_empty(),
        "PMXT one-off conversion projection missing output_catalog_uri"
    );
    ensure!(
        !spec.execution_catalog_uri.trim().is_empty(),
        "PMXT one-off conversion projection missing execution_catalog_uri"
    );
    ensure!(
        !spec.completed_at.trim().is_empty(),
        "PMXT one-off conversion projection missing completed_at"
    );
    match inspect_conversion_output(&spec.output_dir, &spec.fingerprint)? {
        ConversionOutputState::CleanNew => write_new_pmxt_one_off_conversion_projection(spec),
        ConversionOutputState::Complete {
            manifest_hash,
            checkpoint_hash,
            catalog_hash,
        } => reuse_completed_pmxt_one_off_conversion_projection(
            spec,
            manifest_hash,
            checkpoint_hash,
            catalog_hash,
        ),
        ConversionOutputState::ResumeFromCheckpoint { stage } => {
            bail!(
                "PMXT one-off conversion projection cannot resume from checkpoint stage {stage:?}"
            )
        }
    }
}

fn write_new_pmxt_one_off_conversion_projection(
    spec: PmxtOneOffConversionProjectionSpec,
) -> Result<PmxtOneOffCompletedConversionProjection> {
    let catalog_projection =
        write_pmxt_one_off_projection_to_catalog(&spec.catalog_root, &spec.projection)?;
    let canonical_rows = usize::try_from(catalog_projection.order_book_delta_count)
        .context("PMXT one-off OrderBookDelta count does not fit usize")?;
    let conversion_checkpoint = ConversionCheckpoint::completed(
        spec.fingerprint.clone(),
        canonical_rows,
        catalog_projection.catalog_hash.clone(),
        spec.completed_at.clone(),
    );
    let conversion_checkpoint_hash = conversion_checkpoint
        .content_hash()
        .context("hash PMXT one-off conversion checkpoint")?;
    let conversion_manifest = ConversionManifest::completed(
        spec.fingerprint,
        spec.normalized_schema_version,
        NT_DATA_TYPE_ORDER_BOOK_DELTA,
        catalog_projection.nt_instrument_id.clone(),
        canonical_rows,
        spec.output_catalog_uri,
        catalog_projection.catalog_hash.clone(),
        conversion_checkpoint_hash.clone(),
        spec.completed_at,
    );
    let conversion_manifest_hash = conversion_manifest
        .content_hash()
        .context("hash PMXT one-off conversion manifest")?;
    let conversion_catalog_metadata = ConversionCatalogMetadata::from_manifest(
        &conversion_manifest,
        conversion_manifest_hash.clone(),
        conversion_checkpoint_hash.clone(),
    )
    .with_execution_catalog_access(
        spec.execution_catalog_uri,
        spec.direct_s3_catalog_access_proven,
    );
    let conversion_catalog_metadata_hash = conversion_catalog_metadata
        .content_hash()
        .context("hash PMXT one-off catalog metadata")?;
    write_completed_conversion_artifacts(
        &spec.output_dir,
        &conversion_manifest,
        &conversion_checkpoint,
        &conversion_catalog_metadata,
    )?;

    Ok(PmxtOneOffCompletedConversionProjection {
        catalog_projection,
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash,
        conversion_manifest_hash,
        conversion_catalog_metadata_hash,
    })
}

fn reuse_completed_pmxt_one_off_conversion_projection(
    spec: PmxtOneOffConversionProjectionSpec,
    manifest_hash: String,
    checkpoint_hash: String,
    catalog_hash: String,
) -> Result<PmxtOneOffCompletedConversionProjection> {
    let conversion_checkpoint: ConversionCheckpoint =
        read_conversion_json(&spec.output_dir.join(CONVERSION_CHECKPOINT_FILE))?;
    let conversion_manifest: ConversionManifest =
        read_conversion_json(&spec.output_dir.join(CONVERSION_MANIFEST_FILE))?;
    let conversion_catalog_metadata: ConversionCatalogMetadata =
        read_conversion_json(&spec.output_dir.join(CATALOG_METADATA_FILE))?;
    ensure!(
        conversion_checkpoint.content_hash()? == checkpoint_hash,
        "PMXT one-off completed checkpoint hash changed after validation"
    );
    ensure!(
        conversion_manifest.content_hash()? == manifest_hash,
        "PMXT one-off completed manifest hash changed after validation"
    );
    let conversion_catalog_metadata_hash = conversion_catalog_metadata
        .content_hash()
        .context("hash PMXT one-off catalog metadata")?;
    ensure!(
        conversion_catalog_metadata.catalog_hash == catalog_hash,
        "PMXT one-off catalog metadata hash binding changed after validation"
    );

    let nt_instrument_id = spec.projection.instrument.id().to_string();
    let canonical_rows = spec.projection.order_book_deltas.len();
    ensure!(
        conversion_manifest.normalized_schema_version == spec.normalized_schema_version,
        "PMXT one-off completed manifest normalized_schema_version mismatch"
    );
    ensure!(
        conversion_manifest.nt_data_type == NT_DATA_TYPE_ORDER_BOOK_DELTA,
        "PMXT one-off completed manifest nt_data_type mismatch"
    );
    ensure!(
        conversion_manifest.nt_instrument_id == nt_instrument_id,
        "PMXT one-off completed manifest nt_instrument_id mismatch"
    );
    ensure!(
        conversion_manifest.canonical_rows == canonical_rows,
        "PMXT one-off completed manifest canonical_rows mismatch"
    );
    ensure!(
        conversion_manifest.output_catalog_uri == spec.output_catalog_uri,
        "PMXT one-off completed manifest output_catalog_uri mismatch"
    );
    ensure!(
        conversion_manifest.catalog_hash == catalog_hash,
        "PMXT one-off completed manifest catalog_hash mismatch"
    );
    ensure!(
        conversion_catalog_metadata.execution_catalog_uri == spec.execution_catalog_uri,
        "PMXT one-off completed catalog metadata execution_catalog_uri mismatch"
    );
    ensure!(
        conversion_catalog_metadata.direct_s3_catalog_access_proven
            == spec.direct_s3_catalog_access_proven,
        "PMXT one-off completed catalog metadata direct_s3_catalog_access_proven mismatch"
    );

    Ok(PmxtOneOffCompletedConversionProjection {
        catalog_projection: PmxtOneOffCatalogProjection {
            catalog_root: spec.catalog_root,
            source_binding: spec.projection.source_binding,
            usage_scope: spec.projection.usage_scope,
            nt_instrument_id,
            order_book_delta_count: u64::try_from(canonical_rows)
                .context("PMXT one-off OrderBookDelta count does not fit u64")?,
            trade_tick_count: u64::try_from(spec.projection.trade_ticks.len())
                .context("PMXT one-off TradeTick count does not fit u64")?,
            catalog_hash,
        },
        conversion_checkpoint,
        conversion_manifest,
        conversion_catalog_metadata,
        conversion_checkpoint_hash: checkpoint_hash,
        conversion_manifest_hash: manifest_hash,
        conversion_catalog_metadata_hash,
    })
}

fn read_conversion_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

pub fn run_pmxt_one_off_l2_backtest_contract(
    spec: PmxtOneOffBacktestContractSpec<'_>,
) -> Result<PmxtOneOffBacktestContractOutput> {
    let completed = spec.completed;
    ensure!(
        completed.catalog_projection.usage_scope == SourceProofUsageScope::OneOffBackfillData,
        "PMXT one-off result contract only accepts one_off_backfill_data usage_scope"
    );
    ensure!(
        completed.conversion_manifest.nt_data_type == NT_DATA_TYPE_ORDER_BOOK_DELTA,
        "PMXT one-off result contract requires OrderBookDelta conversion output"
    );
    ensure!(
        completed.conversion_catalog_metadata.nt_data_type == NT_DATA_TYPE_ORDER_BOOK_DELTA,
        "PMXT one-off catalog metadata must describe OrderBookDelta"
    );
    ensure!(
        completed.conversion_manifest.catalog_hash == completed.catalog_projection.catalog_hash,
        "PMXT one-off conversion manifest catalog_hash does not match catalog projection"
    );
    ensure!(
        completed.conversion_catalog_metadata.catalog_hash
            == completed.catalog_projection.catalog_hash,
        "PMXT one-off catalog metadata catalog_hash does not match catalog projection"
    );
    ensure!(
        completed.conversion_catalog_metadata_hash
            == completed
                .conversion_catalog_metadata
                .content_hash()
                .context("hash PMXT one-off catalog metadata")?,
        "PMXT one-off catalog metadata hash does not match completed projection"
    );
    ensure!(
        !spec.manifest_hash.trim().is_empty(),
        "PMXT one-off result contract missing manifest_hash"
    );
    ensure!(
        !spec.accepted_by.trim().is_empty(),
        "PMXT one-off result contract missing accepted_by"
    );
    ensure!(
        !spec.accepted_at.trim().is_empty(),
        "PMXT one-off result contract missing accepted_at"
    );
    ensure!(
        !spec.created_at.trim().is_empty(),
        "PMXT one-off result contract missing created_at"
    );
    ensure!(
        !spec.event_count_ledger_hash.trim().is_empty(),
        "PMXT one-off L2 result contract requires event_count_ledger_hash"
    );
    ensure!(
        !spec.selected_asset_ids_hash.trim().is_empty(),
        "PMXT one-off L2 result contract requires selected_asset_ids_hash"
    );
    let manifest_catalog_root = spec.manifest.catalog_input.catalog_path.as_str();
    let catalog_root = completed
        .catalog_projection
        .catalog_root
        .to_str()
        .context("PMXT one-off catalog root is not valid UTF-8")?;
    ensure!(
        manifest_catalog_root == catalog_root,
        "PMXT one-off manifest catalog_path {manifest_catalog_root:?} does not match verified catalog root {catalog_root:?}"
    );
    ensure!(
        spec.manifest.catalog_input.data_type == NT_DATA_TYPE_ORDER_BOOK_DELTA,
        "PMXT one-off manifest catalog_input.data_type must be OrderBookDelta"
    );
    ensure!(
        spec.manifest.catalog_input.nt_instrument_id
            == completed.catalog_projection.nt_instrument_id,
        "PMXT one-off manifest instrument does not match conversion output"
    );
    ensure!(
        spec.manifest.market_structure_fixture == MarketStructureFixture::BinaryOption,
        "PMXT one-off result contract requires binary-option market structure"
    );
    ensure!(
        spec.manifest.source_proof_id == completed.conversion_manifest.fingerprint.source_proof_id,
        "PMXT one-off manifest source_proof_id does not match conversion fingerprint"
    );
    ensure!(
        spec.manifest.source_proof_version
            == completed
                .conversion_manifest
                .fingerprint
                .source_proof_version,
        "PMXT one-off manifest source_proof_version does not match conversion fingerprint"
    );

    let nt_result =
        run_nt_backtest_node(spec.manifest).context("run PMXT one-off L2 BacktestNode")?;
    let expected_iterations = usize::try_from(completed.catalog_projection.order_book_delta_count)
        .context("PMXT one-off OrderBookDelta count does not fit usize")?;
    if let Some(reason) = iterations_mismatch(nt_result.iterations, expected_iterations) {
        bail!("PMXT one-off BacktestNode did not consume verified L2 catalog: {reason}");
    }

    let mut claim_limits = spec.claim_limits;
    claim_limits.extend(nt_extension_surface_claim_limits(spec.manifest)?);
    let fingerprint = &completed.conversion_manifest.fingerprint;
    let contract = build_result_contract(ResultContractInputs {
        run_id: &spec.manifest.run_id,
        source_proof_id: &fingerprint.source_proof_id,
        source_proof_version: fingerprint.source_proof_version,
        manifest_hash: spec.manifest_hash,
        acceptance_mode: spec.acceptance_mode,
        accepted_by: spec.accepted_by,
        accepted_at: spec.accepted_at,
        accepted_object_sha256: &fingerprint.accepted_object_sha256,
        converter_identity: &fingerprint.converter_identity,
        converter_version: &fingerprint.converter_version,
        converter_config_hash: &fingerprint.converter_config_hash,
        conversion_manifest_hash: &completed.conversion_manifest_hash,
        conversion_checkpoint_hash: &completed.conversion_checkpoint_hash,
        catalog_hash: &completed.catalog_projection.catalog_hash,
        catalog_metadata_hash: &completed.conversion_catalog_metadata_hash,
        event_count_ledger_hash: Some(spec.event_count_ledger_hash),
        selected_asset_ids_hash: Some(spec.selected_asset_ids_hash),
        strategy: &spec.manifest.strategy,
        run_purpose: run_purpose_label(spec.manifest),
        market_structure_fixture: market_structure_label(spec.manifest),
        fidelity_class: SourceProofFidelityClass::L2Replay,
        claim_limits,
        warnings: result_contract_warnings(&nt_result),
        mechanical_blockers: Vec::new(),
        nt_result: &nt_result,
        artifact_uris: spec.artifact_uris,
        created_at: spec.created_at,
    })
    .map_err(|error| {
        anyhow::anyhow!("PMXT one-off result contract construction failed: {error}")
    })?;

    Ok(PmxtOneOffBacktestContractOutput {
        nt_result,
        contract,
    })
}

pub fn write_pmxt_one_off_l2_artifact_root_run(
    spec: PmxtOneOffArtifactRootRunSpec,
) -> Result<PmxtOneOffArtifactRootRun> {
    ensure!(
        spec.fingerprint.accepted_object_sha256
            == sha256_file(&spec.selected_source.selected_source_parquet_path)?,
        "PMXT one-off conversion fingerprint accepted_object_sha256 must match selected-source parquet"
    );
    ensure!(
        spec.artifact_uris.nt_catalog_uri == spec.output_catalog_uri,
        "PMXT one-off result artifact nt_catalog_uri must match conversion output_catalog_uri"
    );

    let selected_projection = project_pmxt_selected_source_parquet_to_nt(spec.selected_source)
        .context("project selected PMXT source rows into NT data")?;
    ensure!(
        selected_projection.selected_source_parquet_hash == spec.fingerprint.accepted_object_sha256,
        "PMXT one-off selected-source hash does not match conversion fingerprint"
    );
    let completed = write_pmxt_one_off_conversion_projection(PmxtOneOffConversionProjectionSpec {
        output_dir: spec.output_dir.clone(),
        catalog_root: spec.catalog_root,
        projection: selected_projection.projection.clone(),
        fingerprint: spec.fingerprint,
        normalized_schema_version: spec.normalized_schema_version,
        output_catalog_uri: spec.output_catalog_uri,
        execution_catalog_uri: spec.execution_catalog_uri,
        direct_s3_catalog_access_proven: spec.direct_s3_catalog_access_proven,
        completed_at: spec.created_at.clone(),
    })
    .context("write PMXT one-off conversion artifacts")?;
    let mut contract_output =
        run_pmxt_one_off_l2_backtest_contract(PmxtOneOffBacktestContractSpec {
            completed: &completed,
            manifest: &spec.manifest,
            manifest_hash: &spec.manifest_hash,
            acceptance_mode: spec.acceptance_mode,
            accepted_by: &spec.accepted_by,
            accepted_at: &spec.accepted_at,
            event_count_ledger_hash: &selected_projection.event_count_ledger_hash,
            selected_asset_ids_hash: &selected_projection.selected_asset_ids_hash,
            artifact_uris: spec.artifact_uris,
            created_at: &spec.created_at,
            claim_limits: spec.claim_limits,
        })
        .context("run PMXT one-off L2 backtest contract")?;
    fs::create_dir_all(&spec.output_dir).with_context(|| {
        format!(
            "create PMXT one-off artifact output dir {}",
            spec.output_dir.display()
        )
    })?;
    let result_contract_path = spec.output_dir.join(PMXT_ONE_OFF_RESULT_CONTRACT_FILE);
    contract_output.contract =
        write_result_contract_idempotent(&result_contract_path, &contract_output.contract)
            .with_context(|| format!("write {}", result_contract_path.display()))?;

    Ok(PmxtOneOffArtifactRootRun {
        selected_projection,
        completed,
        contract_output,
        result_contract_path,
    })
}

fn write_result_contract_idempotent(
    path: &Path,
    contract: &BacktestResultContract,
) -> Result<BacktestResultContract> {
    let bytes = serde_json::to_vec_pretty(contract).context("serialize PMXT result contract")?;
    if path.exists() {
        let existing = read_json_artifact::<BacktestResultContract>(path)?;
        let mut normalized = contract.clone();
        normalized.nt_result.machine_id = existing.nt_result.machine_id.clone();
        normalized.nt_result.instance_id = existing.nt_result.instance_id.clone();
        normalized.nt_result.elapsed_time_secs = existing.nt_result.elapsed_time_secs;
        ensure!(
            existing == normalized,
            "existing PMXT one-off result contract {} differs from newly generated stable content",
            path.display()
        );
        return Ok(existing);
    }
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(contract.clone())
}

fn read_json_artifact<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
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
    if let Some(strings) = values.as_any().downcast_ref::<StringViewArray>() {
        return Ok(strings.value(row).to_string());
    }
    if let Some(bytes) = values.as_any().downcast_ref::<BinaryArray>() {
        return Ok(market_bytes_to_string(bytes.value(row)));
    }
    if let Some(bytes) = values.as_any().downcast_ref::<LargeBinaryArray>() {
        return Ok(market_bytes_to_string(bytes.value(row)));
    }
    if let Some(bytes) = values.as_any().downcast_ref::<BinaryViewArray>() {
        return Ok(market_bytes_to_string(bytes.value(row)));
    }
    if let Some(bytes) = values.as_any().downcast_ref::<FixedSizeBinaryArray>() {
        return Ok(market_bytes_to_string(bytes.value(row)));
    }
    bail!(
        "selected-source market column {column:?} is not Utf8/LargeUtf8/Utf8View/Binary/LargeBinary/BinaryView/FixedSizeBinary"
    )
}

fn market_bytes_to_string(bytes: &[u8]) -> String {
    std::str::from_utf8(bytes).map_or_else(
        |_| format!("0x{}", hex::encode(bytes)),
        std::string::ToString::to_string,
    )
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
    if let Some(strings) = values.as_any().downcast_ref::<StringViewArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    bail!("selected-source column {column:?} is not Utf8, LargeUtf8, or Utf8View")
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
    if let Some(strings) = values.as_any().downcast_ref::<StringViewArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    bail!("selected-source column {column:?} is not Decimal128/Decimal64/Utf8/LargeUtf8/Utf8View")
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
