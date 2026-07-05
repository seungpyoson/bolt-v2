//! Bounded source-row slicer for one-off historical bootstrap proofs.
//!
//! This does not convert venue data into NautilusTrader data. It only stages
//! the selector-approved raw rows into a small parquet artifact so the NT
//! projection proof can iterate quickly and reproducibly.

use std::{
    collections::BTreeSet,
    fs,
    fs::File,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use arrow::{
    array::{Array, BooleanArray, LargeStringArray, RecordBatchReader, StringArray},
    compute::filter_record_batch,
    record_batch::RecordBatch,
};
use parquet::arrow::{ArrowWriter, ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde::{Deserialize, Serialize};

use crate::first_proof_selector::{
    FirstProofSelectorReport, FirstProofSelectorStatus, SelectedFirstProofAsset,
};
use crate::hashing::sha256_hex;

pub const SELECTED_SOURCE_SLICE_REPORT_SCHEMA_VERSION: &str = "selected-source-slice-report.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectedSourceSliceUsageScope {
    OneOffBackfillData,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedSourceSliceSpec {
    pub source_parquet_path: PathBuf,
    pub selector_report_path: PathBuf,
    pub output_parquet_path: PathBuf,
    pub report_path: PathBuf,
    pub asset_id_column: String,
    pub usage_scope: SelectedSourceSliceUsageScope,
    pub max_source_parquet_bytes: u64,
    pub projected_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedSourceSliceReport {
    pub schema_version: String,
    pub source_parquet_path: String,
    pub source_parquet_sha256: String,
    pub selector_report_path: String,
    pub selector_report_sha256: String,
    pub output_parquet_path: String,
    pub asset_id_column: String,
    pub usage_scope: SelectedSourceSliceUsageScope,
    pub projected_columns: Vec<String>,
    pub source_rows: u64,
    pub source_row_groups: u64,
    pub projected_row_groups: u64,
    pub selected_rows: u64,
    pub selected_asset_count: u64,
    pub selected_asset_ids_hash: String,
    pub output_parquet_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedSourceSliceArtifact {
    pub output_parquet_path: PathBuf,
    pub report_path: PathBuf,
    pub source_parquet_sha256: String,
    pub selector_report_sha256: String,
    pub output_parquet_sha256: String,
    pub report_hash: String,
    pub report_bytes: u64,
    pub usage_scope: SelectedSourceSliceUsageScope,
    pub source_rows: u64,
    pub source_row_groups: u64,
    pub projected_row_groups: u64,
    pub selected_rows: u64,
    pub selected_asset_count: u64,
    pub selected_asset_ids_hash: String,
}

pub fn write_selected_source_slice_from_spec_file(
    spec_path: &Path,
) -> Result<SelectedSourceSliceArtifact> {
    let spec_text = fs::read_to_string(spec_path)
        .with_context(|| format!("read selected source slice spec {}", spec_path.display()))?;
    let spec: SelectedSourceSliceSpec = toml::from_str(&spec_text)
        .with_context(|| format!("parse selected source slice spec {}", spec_path.display()))?;
    write_selected_source_slice(&spec)
}

pub fn write_selected_source_slice(
    spec: &SelectedSourceSliceSpec,
) -> Result<SelectedSourceSliceArtifact> {
    ensure!(
        spec.projected_columns
            .iter()
            .any(|column| column == &spec.asset_id_column),
        "projected_columns must include asset_id_column {:?}",
        spec.asset_id_column
    );
    ensure!(
        !spec.projected_columns.is_empty(),
        "projected_columns must not be empty"
    );
    ensure!(
        spec.max_source_parquet_bytes > 0,
        "selected_source_slice.max_source_parquet_bytes must be positive"
    );
    let source_parquet_bytes = fs::metadata(&spec.source_parquet_path)
        .with_context(|| format!("stat source parquet {}", spec.source_parquet_path.display()))?
        .len();
    ensure!(
        source_parquet_bytes <= spec.max_source_parquet_bytes,
        "source parquet byte length {source_parquet_bytes} exceeds selected_source_slice.max_source_parquet_bytes {}",
        spec.max_source_parquet_bytes
    );

    let source_parquet_sha256 = sha256_file(&spec.source_parquet_path)?;
    let selector_bytes = fs::read(&spec.selector_report_path).with_context(|| {
        format!(
            "read selector report {}",
            spec.selector_report_path.display()
        )
    })?;
    let selector_report_sha256 = sha256_hex(&selector_bytes);
    let selector: FirstProofSelectorReport =
        serde_json::from_slice(&selector_bytes).with_context(|| {
            format!(
                "parse selector report {}",
                spec.selector_report_path.display()
            )
        })?;
    ensure!(
        selector.status == FirstProofSelectorStatus::Selected,
        "selector report status must be selected"
    );
    ensure!(
        !selector.selected_assets.is_empty(),
        "selector report must contain selected assets"
    );
    let selected_assets = selector
        .selected_assets
        .iter()
        .map(|asset| asset.asset_id.as_str())
        .collect::<BTreeSet<_>>();
    let matching_row_groups =
        selector_source_row_groups(&spec.source_parquet_path, &selector.selected_assets)?;
    ensure!(
        !matching_row_groups.row_groups.is_empty(),
        "selected source slice found zero matching row groups"
    );

    let file = File::open(&spec.source_parquet_path)
        .with_context(|| format!("open source parquet {}", spec.source_parquet_path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).with_context(|| {
        format!(
            "build source parquet reader {}",
            spec.source_parquet_path.display()
        )
    })?;
    let projected_columns = spec
        .projected_columns
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let projection = ProjectionMask::columns(builder.parquet_schema(), projected_columns);
    let reader = builder
        .with_projection(projection)
        .with_row_groups(matching_row_groups.row_groups.clone())
        .build()
        .with_context(|| {
            format!(
                "build projected source parquet reader {}",
                spec.source_parquet_path.display()
            )
        })?;
    let output_schema = reader.schema();

    if let Some(parent) = spec.output_parquet_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output parquet directory {}", parent.display()))?;
    }
    if let Some(parent) = spec.report_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "create selected source report directory {}",
                parent.display()
            )
        })?;
    }

    let temp_output_path = temp_artifact_path(&spec.output_parquet_path);
    // Atomic exclusive create (O_CREAT|O_EXCL): a pre-existing temp — crash
    // residue OR a concurrent writer for the same output — fails loud here
    // instead of being silently truncated and shared. This is the single-writer
    // enforcement. A prior `exists()` check followed by a plain `File::create`
    // would be TOCTOU: two racers could both pass the check, both truncate the
    // same inode, and one keep writing into it after the other renames it onto
    // the final artifact — publishing a torn parquet with a stale recorded hash.
    let file = File::create_new(&temp_output_path).with_context(|| {
        format!(
            "create temporary selected source parquet {} (a pre-existing temp \
             means a crashed prior run or a concurrent writer for the same output)",
            temp_output_path.display()
        )
    })?;
    let mut writer = ArrowWriter::try_new(file, output_schema, None).with_context(|| {
        format!(
            "create temporary selected source parquet writer {}",
            temp_output_path.display()
        )
    })?;
    let mut selected_rows = 0_u64;
    for batch in reader {
        let batch = batch.with_context(|| {
            format!(
                "read projected source parquet batch {}",
                spec.source_parquet_path.display()
            )
        })?;
        let mask = selected_asset_mask(&batch, &spec.asset_id_column, &selected_assets)?;
        let filtered = filter_record_batch(&batch, &mask)
            .context("filter selected source parquet batch by selected assets")?;
        if filtered.num_rows() > 0 {
            selected_rows = selected_rows.saturating_add(filtered.num_rows() as u64);
            writer.write(&filtered).with_context(|| {
                format!(
                    "write selected source parquet {}",
                    spec.output_parquet_path.display()
                )
            })?;
        }
    }
    writer.close().with_context(|| {
        format!(
            "close selected source parquet {}",
            temp_output_path.display()
        )
    })?;
    if selected_rows == 0 {
        let _ = fs::remove_file(&temp_output_path);
        bail!("selected source slice wrote zero rows");
    }

    let output_parquet_sha256 = sha256_file(&temp_output_path)?;
    commit_artifact_file(
        &temp_output_path,
        &spec.output_parquet_path,
        "selected source artifact",
    )?;
    let report = SelectedSourceSliceReport {
        schema_version: SELECTED_SOURCE_SLICE_REPORT_SCHEMA_VERSION.to_string(),
        source_parquet_path: spec.source_parquet_path.display().to_string(),
        source_parquet_sha256,
        selector_report_path: spec.selector_report_path.display().to_string(),
        selector_report_sha256,
        output_parquet_path: spec.output_parquet_path.display().to_string(),
        asset_id_column: spec.asset_id_column.clone(),
        usage_scope: spec.usage_scope,
        projected_columns: spec.projected_columns.clone(),
        source_rows: matching_row_groups.source_rows,
        source_row_groups: matching_row_groups.source_row_groups,
        projected_row_groups: matching_row_groups.row_groups.len() as u64,
        selected_rows,
        selected_asset_count: selector.selected_assets.len() as u64,
        selected_asset_ids_hash: selector.selected_asset_ids_hash,
        output_parquet_sha256,
    };
    let report_artifact = crate::reference_artifact::write_reference_artifact_with_len(
        &spec.report_path,
        SELECTED_SOURCE_SLICE_REPORT_SCHEMA_VERSION,
        &report,
    )
    .with_context(|| {
        format!(
            "write selected source report {}",
            spec.report_path.display()
        )
    })?;

    Ok(SelectedSourceSliceArtifact {
        output_parquet_path: spec.output_parquet_path.clone(),
        report_path: spec.report_path.clone(),
        source_parquet_sha256: report.source_parquet_sha256,
        selector_report_sha256: report.selector_report_sha256,
        output_parquet_sha256: report.output_parquet_sha256,
        report_hash: report_artifact.pin.sha256,
        report_bytes: report_artifact.bytes,
        usage_scope: report.usage_scope,
        source_rows: report.source_rows,
        source_row_groups: report.source_row_groups,
        projected_row_groups: report.projected_row_groups,
        selected_rows,
        selected_asset_count: report.selected_asset_count,
        selected_asset_ids_hash: report.selected_asset_ids_hash,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchingRowGroups {
    source_rows: u64,
    source_row_groups: u64,
    row_groups: Vec<usize>,
}

fn selector_source_row_groups(
    source_parquet_path: &Path,
    selected_assets: &[SelectedFirstProofAsset],
) -> Result<MatchingRowGroups> {
    let file = File::open(source_parquet_path)
        .with_context(|| format!("open source parquet {}", source_parquet_path.display()))?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).with_context(|| {
        format!(
            "build source parquet reader {}",
            source_parquet_path.display()
        )
    })?;
    let row_group_rows = builder
        .metadata()
        .row_groups()
        .iter()
        .map(|row_group| row_group.num_rows() as u64)
        .collect::<Vec<_>>();
    let source_rows = row_group_rows.iter().sum::<u64>();
    let source_row_groups = row_group_rows.len() as u64;
    let mut matching = BTreeSet::new();
    for asset in selected_assets {
        ensure!(
            !asset.source_row_groups.is_empty(),
            "selected asset {:?} is missing source_row_groups",
            asset.asset_id
        );
        for source_row_group in &asset.source_row_groups {
            let row_group = usize::try_from(*source_row_group).with_context(|| {
                format!(
                    "selector source_row_group {source_row_group} for selected asset {:?} does not fit usize",
                    asset.asset_id
                )
            })?;
            ensure!(
                row_group < row_group_rows.len(),
                "selector source_row_group {source_row_group} for selected asset {:?} exceeds parquet row group count {}",
                asset.asset_id,
                row_group_rows.len()
            );
            matching.insert(row_group);
        }
    }

    Ok(MatchingRowGroups {
        source_rows,
        source_row_groups,
        row_groups: matching.into_iter().collect(),
    })
}

fn selected_asset_mask(
    batch: &RecordBatch,
    asset_id_column: &str,
    selected_assets: &BTreeSet<&str>,
) -> Result<BooleanArray> {
    let values = batch
        .column_by_name(asset_id_column)
        .with_context(|| format!("source parquet missing asset column {asset_id_column:?}"))?;
    let mut mask = Vec::with_capacity(batch.num_rows());
    for row in 0..batch.num_rows() {
        let asset_id = string_column_value(values.as_ref(), asset_id_column, row)?;
        mask.push(selected_assets.contains(asset_id));
    }
    Ok(BooleanArray::from(mask))
}

fn string_column_value<'a>(values: &'a dyn Array, column: &str, row: usize) -> Result<&'a str> {
    if values.is_null(row) {
        bail!("source parquet column {column:?} has null at row {row}");
    }
    if let Some(strings) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(strings.value(row));
    }
    if let Some(strings) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(strings.value(row));
    }
    bail!("source parquet column {column:?} is not Utf8 or LargeUtf8")
}

fn sha256_file(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).with_context(|| format!("read file for sha256 {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn temp_artifact_path(path: &Path) -> PathBuf {
    let mut temp = path.as_os_str().to_os_string();
    temp.push(".tmp");
    PathBuf::from(temp)
}

fn commit_artifact_file(temp_path: &Path, final_path: &Path, label: &str) -> Result<()> {
    if final_path.exists() {
        let expected = fs::read(temp_path)
            .with_context(|| format!("read temporary {label} {}", temp_path.display()))?;
        let existing = fs::read(final_path)
            .with_context(|| format!("read existing {label} {}", final_path.display()))?;
        fs::remove_file(temp_path)
            .with_context(|| format!("remove temporary {label} {}", temp_path.display()))?;
        ensure!(
            existing == expected,
            "dirty {label} {}: existing file content differs",
            final_path.display()
        );
        return Ok(());
    }
    fs::rename(temp_path, final_path).with_context(|| {
        format!(
            "publish {label} {} from {}",
            final_path.display(),
            temp_path.display()
        )
    })
}
