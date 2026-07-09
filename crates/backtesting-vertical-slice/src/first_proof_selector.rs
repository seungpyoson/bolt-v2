//! Generic first-proof instrument-universe selector.
//!
//! The selector consumes source-proof-owned event-count metadata. Runtime event
//! family labels, budgets, and source identifiers come from config/proof
//! artifacts; this module only applies the generic role rules.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    fs::File,
    path::{Path, PathBuf},
};

use arrow::{
    array::{Array, LargeStringArray, StringArray},
    record_batch::RecordBatch,
};
use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::path_resolution::{resolve_existing_path, resolve_output_dir};

pub const FIRST_PROOF_EVENT_COUNT_LEDGER_SCHEMA_VERSION: &str = "first-proof-event-count-ledger.v1";
pub const FIRST_PROOF_SELECTOR_SCHEMA_VERSION: &str = "first-proof-selector-report.v1";
pub const FIRST_PROOF_SELECTOR_REPORT_FILE: &str = "first-proof-selector-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofSelection {
    pub required_event_families: Vec<String>,
    pub excluded_event_families: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_asset_ids: Vec<String>,
    pub row_budget: u64,
    pub max_selected_assets: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetEventCount {
    pub asset_id: String,
    pub event_family: String,
    pub rows: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_row_groups: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofEventCountLedger {
    pub event_counts: Vec<AssetEventCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofEventCountLedgerReport {
    pub schema_version: String,
    pub source_rows: u64,
    pub event_counts: Vec<AssetEventCount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofEventCountLedgerSpec {
    pub source_parquet_path: PathBuf,
    pub output_path: PathBuf,
    pub max_source_parquet_bytes: u64,
    pub asset_id_column: String,
    pub event_family_column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofSelectorSpec {
    pub selector_id: String,
    pub event_count_ledger_path: PathBuf,
    pub output_dir: PathBuf,
    pub selection: FirstProofSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstProofSelectorStatus {
    Selected,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirstProofSelectorIssue {
    EmptySelectorId,
    EmptyRequiredEventFamilies,
    EmptyExcludedEventFamilies,
    InvalidRowBudget,
    InvalidSelectedAssetBudget,
    NoEligibleAssets,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedFirstProofAsset {
    pub asset_id: String,
    pub replay_rows: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_row_groups: Vec<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct AssetEventAccumulator {
    rows: u64,
    source_row_groups: BTreeSet<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceRowGroupCursor {
    row_group_rows: Vec<u64>,
    source_rows: u64,
    current_row_group: usize,
    next_row_group_end: u64,
}

impl SourceRowGroupCursor {
    fn new(row_group_rows: Vec<u64>) -> Self {
        let next_row_group_end = row_group_rows.first().copied().unwrap_or(0);
        Self {
            row_group_rows,
            source_rows: 0,
            current_row_group: 0,
            next_row_group_end,
        }
    }

    fn expected_source_rows(&self) -> u64 {
        self.row_group_rows.iter().sum()
    }

    fn source_rows(&self) -> u64 {
        self.source_rows
    }

    fn current_row_group(&mut self) -> Result<u64, FirstProofSelectorError> {
        while self.current_row_group < self.row_group_rows.len()
            && self.source_rows >= self.next_row_group_end
        {
            self.current_row_group += 1;
            self.next_row_group_end += self
                .row_group_rows
                .get(self.current_row_group)
                .copied()
                .unwrap_or_default();
        }
        if self.current_row_group >= self.row_group_rows.len() {
            return Err(FirstProofSelectorError::SourceRowGroupBounds {
                source_rows: self.source_rows,
                expected_source_rows: self.expected_source_rows(),
            });
        }
        Ok(self.current_row_group as u64)
    }

    fn advance_row(&mut self) {
        self.source_rows = self.source_rows.saturating_add(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofSelectorReport {
    pub schema_version: String,
    pub selector_id: String,
    pub status: FirstProofSelectorStatus,
    pub selection: FirstProofSelection,
    pub event_count_ledger_hash: String,
    pub total_assets: u64,
    pub eligible_assets: u64,
    pub selected_assets: Vec<SelectedFirstProofAsset>,
    pub selected_asset_ids_hash: String,
    pub excluded_event_asset_count: u64,
    pub excluded_event_row_count: u64,
    pub blocking_issues: Vec<FirstProofSelectorIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstProofSelectorArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub selected_asset_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstProofEventCountLedgerArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub source_rows: u64,
    pub event_count_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstProofSelectorError {
    ReadSpec {
        path: String,
        error: String,
    },
    ParseSpecToml {
        path: String,
        error: String,
    },
    ReadEventCountLedger {
        path: String,
        error: String,
    },
    ParseEventCountLedgerJson {
        path: String,
        error: String,
    },
    ReadSourceParquet {
        path: String,
        error: String,
    },
    StatSourceParquet {
        path: String,
        error: String,
    },
    InvalidSourceParquetByteBudget,
    SourceParquetByteBudgetExceeded {
        source_parquet_bytes: u64,
        max_source_parquet_bytes: u64,
    },
    BuildSourceParquetReader {
        path: String,
        error: String,
    },
    ReadSourceParquetBatch {
        path: String,
        error: String,
    },
    MissingSourceColumn {
        column: String,
    },
    UnsupportedSourceColumn {
        column: String,
    },
    NullSourceColumnValue {
        column: String,
        row: usize,
    },
    SourceRowGroupBounds {
        source_rows: u64,
        expected_source_rows: u64,
    },
    CreateDir {
        path: String,
        error: String,
    },
    ReadExisting {
        path: String,
        error: String,
    },
    Write {
        path: String,
        error: String,
    },
    ExistingArtifactMismatch {
        path: String,
    },
    Serialize(String),
}

impl fmt::Display for FirstProofSelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadSpec { path, error } => {
                write!(f, "read first-proof selector spec {path}: {error}")
            }
            Self::ParseSpecToml { path, error } => {
                write!(f, "parse first-proof selector spec TOML {path}: {error}")
            }
            Self::ReadEventCountLedger { path, error } => {
                write!(f, "read first-proof event-count ledger {path}: {error}")
            }
            Self::ParseEventCountLedgerJson { path, error } => write!(
                f,
                "parse first-proof event-count ledger JSON {path}: {error}"
            ),
            Self::ReadSourceParquet { path, error } => {
                write!(f, "read first-proof source parquet {path}: {error}")
            }
            Self::StatSourceParquet { path, error } => {
                write!(f, "stat first-proof source parquet {path}: {error}")
            }
            Self::InvalidSourceParquetByteBudget => write!(
                f,
                "first-proof event-count ledger max_source_parquet_bytes must be positive"
            ),
            Self::SourceParquetByteBudgetExceeded {
                source_parquet_bytes,
                max_source_parquet_bytes,
            } => write!(
                f,
                "first-proof source parquet byte length {source_parquet_bytes} exceeds max_source_parquet_bytes {max_source_parquet_bytes}"
            ),
            Self::BuildSourceParquetReader { path, error } => {
                write!(f, "build first-proof source parquet reader {path}: {error}")
            }
            Self::ReadSourceParquetBatch { path, error } => {
                write!(f, "read first-proof source parquet batch {path}: {error}")
            }
            Self::MissingSourceColumn { column } => {
                write!(f, "first-proof source parquet missing column {column:?}")
            }
            Self::UnsupportedSourceColumn { column } => write!(
                f,
                "first-proof source parquet column {column:?} is not Utf8 or LargeUtf8"
            ),
            Self::NullSourceColumnValue { column, row } => write!(
                f,
                "first-proof source parquet column {column:?} has null at row {row}"
            ),
            Self::SourceRowGroupBounds {
                source_rows,
                expected_source_rows,
            } => write!(
                f,
                "first-proof source parquet row group scan exceeded row bounds: scanned {source_rows}, expected {expected_source_rows}"
            ),
            Self::CreateDir { path, error } => write!(
                f,
                "create first-proof selector artifact directory {path}: {error}"
            ),
            Self::ReadExisting { path, error } => write!(
                f,
                "read existing first-proof selector artifact {path}: {error}"
            ),
            Self::Write { path, error } => {
                write!(f, "write first-proof selector artifact {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => write!(
                f,
                "dirty first-proof selector artifact {path}: existing file content differs"
            ),
            Self::Serialize(error) => {
                write!(f, "serialize first-proof selector artifact: {error}")
            }
        }
    }
}

impl Error for FirstProofSelectorError {}

#[must_use]
pub fn evaluate_first_proof_selector(
    selector_id: impl Into<String>,
    event_counts: &[AssetEventCount],
    selection: &FirstProofSelection,
) -> FirstProofSelectorReport {
    let selector_id = selector_id.into();
    let mut blocking_issues = Vec::new();
    if selector_id.trim().is_empty() {
        blocking_issues.push(FirstProofSelectorIssue::EmptySelectorId);
    }
    if selection.required_event_families.is_empty() {
        blocking_issues.push(FirstProofSelectorIssue::EmptyRequiredEventFamilies);
    }
    if selection.excluded_event_families.is_empty() {
        blocking_issues.push(FirstProofSelectorIssue::EmptyExcludedEventFamilies);
    }
    if selection.row_budget == 0 {
        blocking_issues.push(FirstProofSelectorIssue::InvalidRowBudget);
    }
    if selection.max_selected_assets == 0 {
        blocking_issues.push(FirstProofSelectorIssue::InvalidSelectedAssetBudget);
    }

    let required_event_families = selection
        .required_event_families
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let excluded_event_families = selection
        .excluded_event_families
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let candidate_asset_ids = selection
        .candidate_asset_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut counts_by_asset = BTreeMap::<String, BTreeMap<String, AssetEventAccumulator>>::new();
    for count in event_counts {
        let asset_counts = counts_by_asset.entry(count.asset_id.clone()).or_default();
        let accumulator = asset_counts.entry(count.event_family.clone()).or_default();
        accumulator.rows = accumulator.rows.saturating_add(count.rows);
        accumulator
            .source_row_groups
            .extend(count.source_row_groups.iter().copied());
    }

    let mut excluded_event_assets = BTreeSet::new();
    let mut excluded_event_row_count = 0_u64;
    let mut eligible = counts_by_asset
        .iter()
        .filter_map(|(asset_id, counts)| {
            if !candidate_asset_ids.is_empty() && !candidate_asset_ids.contains(asset_id.as_str()) {
                return None;
            }

            let excluded_rows = counts
                .iter()
                .filter(|(event_family, count)| {
                    excluded_event_families.contains(event_family.as_str()) && count.rows > 0
                })
                .map(|(_, count)| count.rows)
                .sum::<u64>();
            if excluded_rows > 0 {
                excluded_event_assets.insert(asset_id.clone());
                excluded_event_row_count = excluded_event_row_count.saturating_add(excluded_rows);
                return None;
            }

            let mut replay_rows = 0_u64;
            let mut source_row_groups = BTreeSet::new();
            for event_family in &required_event_families {
                let event_count = counts.get(*event_family).cloned().unwrap_or_default();
                let rows = event_count.rows;
                if rows == 0 {
                    return None;
                }
                replay_rows = replay_rows.saturating_add(rows);
                source_row_groups.extend(event_count.source_row_groups);
            }
            if replay_rows > selection.row_budget {
                return None;
            }
            Some(SelectedFirstProofAsset {
                asset_id: asset_id.clone(),
                replay_rows,
                source_row_groups: source_row_groups.into_iter().collect(),
            })
        })
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        left.replay_rows
            .cmp(&right.replay_rows)
            .then(left.asset_id.cmp(&right.asset_id))
    });

    let eligible_assets = eligible.len() as u64;
    if eligible.is_empty() {
        blocking_issues.push(FirstProofSelectorIssue::NoEligibleAssets);
    }
    let mut selected_assets = if blocking_issues.is_empty() {
        eligible
    } else {
        Vec::new()
    };
    selected_assets.truncate(selection.max_selected_assets as usize);
    let status = if blocking_issues.is_empty() {
        FirstProofSelectorStatus::Selected
    } else {
        FirstProofSelectorStatus::Blocked
    };

    FirstProofSelectorReport {
        schema_version: FIRST_PROOF_SELECTOR_SCHEMA_VERSION.to_string(),
        selector_id,
        status,
        selection: selection.clone(),
        event_count_ledger_hash: event_count_ledger_hash(event_counts),
        total_assets: counts_by_asset.len() as u64,
        eligible_assets,
        selected_asset_ids_hash: selected_asset_ids_hash(&selected_assets),
        selected_assets,
        excluded_event_asset_count: excluded_event_assets.len() as u64,
        excluded_event_row_count,
        blocking_issues,
    }
}

pub fn write_first_proof_selector_report_from_spec_file(
    spec_path: &Path,
) -> Result<FirstProofSelectorArtifact, FirstProofSelectorError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| FirstProofSelectorError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: FirstProofSelectorSpec =
        toml::from_str(&spec_text).map_err(|error| FirstProofSelectorError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let ledger_path_display = spec.event_count_ledger_path.display().to_string();
    let resolved_ledger_path = resolve_existing_path(base_dir, &spec.event_count_ledger_path);
    let ledger_bytes = fs::read(&resolved_ledger_path).map_err(|error| {
        FirstProofSelectorError::ReadEventCountLedger {
            path: ledger_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let ledger = parse_event_count_ledger(&ledger_bytes, &ledger_path_display)?;
    let report =
        evaluate_first_proof_selector(spec.selector_id, &ledger.event_counts, &spec.selection);
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    write_first_proof_selector_report(&output_dir, &report)
}

pub fn write_first_proof_event_count_ledger_from_spec_file(
    spec_path: &Path,
) -> Result<FirstProofEventCountLedgerArtifact, FirstProofSelectorError> {
    let spec_path_display = spec_path.display().to_string();
    let spec_text =
        fs::read_to_string(spec_path).map_err(|error| FirstProofSelectorError::ReadSpec {
            path: spec_path_display.clone(),
            error: error.to_string(),
        })?;
    let spec: FirstProofEventCountLedgerSpec =
        toml::from_str(&spec_text).map_err(|error| FirstProofSelectorError::ParseSpecToml {
            path: spec_path_display,
            error: error.to_string(),
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    let resolved_spec = FirstProofEventCountLedgerSpec {
        source_parquet_path: resolve_existing_path(base_dir, &spec.source_parquet_path),
        output_path: resolve_output_dir(base_dir, &spec.output_path),
        ..spec
    };
    let report = build_first_proof_event_count_ledger_from_parquet(&resolved_spec)?;
    write_first_proof_event_count_ledger(&resolved_spec.output_path, &report)
}

pub fn build_first_proof_event_count_ledger_from_parquet(
    spec: &FirstProofEventCountLedgerSpec,
) -> Result<FirstProofEventCountLedgerReport, FirstProofSelectorError> {
    let source_path = spec.source_parquet_path.display().to_string();
    if spec.max_source_parquet_bytes == 0 {
        return Err(FirstProofSelectorError::InvalidSourceParquetByteBudget);
    }
    let source_parquet_bytes = fs::metadata(&spec.source_parquet_path)
        .map_err(|error| FirstProofSelectorError::StatSourceParquet {
            path: source_path.clone(),
            error: error.to_string(),
        })?
        .len();
    if source_parquet_bytes > spec.max_source_parquet_bytes {
        return Err(FirstProofSelectorError::SourceParquetByteBudgetExceeded {
            source_parquet_bytes,
            max_source_parquet_bytes: spec.max_source_parquet_bytes,
        });
    }
    let file = File::open(&spec.source_parquet_path).map_err(|error| {
        FirstProofSelectorError::ReadSourceParquet {
            path: source_path.clone(),
            error: error.to_string(),
        }
    })?;
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).map_err(|error| {
        FirstProofSelectorError::BuildSourceParquetReader {
            path: source_path.clone(),
            error: error.to_string(),
        }
    })?;
    let projection = ProjectionMask::columns(
        builder.parquet_schema(),
        [
            spec.asset_id_column.as_str(),
            spec.event_family_column.as_str(),
        ],
    );
    let row_group_rows = builder
        .metadata()
        .row_groups()
        .iter()
        .map(|row_group| row_group.num_rows() as u64)
        .collect::<Vec<_>>();
    let reader = builder
        .with_projection(projection)
        .build()
        .map_err(|error| FirstProofSelectorError::BuildSourceParquetReader {
            path: source_path.clone(),
            error: error.to_string(),
        })?;
    let mut counts = BTreeMap::<String, BTreeMap<String, AssetEventAccumulator>>::new();
    let mut row_group_cursor = SourceRowGroupCursor::new(row_group_rows);
    for batch in reader {
        let batch = batch.map_err(|error| FirstProofSelectorError::ReadSourceParquetBatch {
            path: source_path.clone(),
            error: error.to_string(),
        })?;
        add_batch_event_counts(
            &batch,
            &spec.asset_id_column,
            &spec.event_family_column,
            &mut counts,
            &mut row_group_cursor,
        )?;
    }
    let source_rows = row_group_cursor.source_rows();
    let expected_source_rows = row_group_cursor.expected_source_rows();
    if source_rows != expected_source_rows {
        return Err(FirstProofSelectorError::SourceRowGroupBounds {
            source_rows,
            expected_source_rows,
        });
    }
    Ok(FirstProofEventCountLedgerReport {
        schema_version: FIRST_PROOF_EVENT_COUNT_LEDGER_SCHEMA_VERSION.to_string(),
        source_rows,
        event_counts: counts
            .into_iter()
            .flat_map(|(asset_id, event_counts)| {
                event_counts
                    .into_iter()
                    .map(move |(event_family, count)| AssetEventCount {
                        asset_id: asset_id.clone(),
                        event_family,
                        rows: count.rows,
                        source_row_groups: count.source_row_groups.into_iter().collect(),
                    })
            })
            .collect(),
    })
}

pub fn write_first_proof_event_count_ledger(
    output_path: &Path,
    report: &FirstProofEventCountLedgerReport,
) -> Result<FirstProofEventCountLedgerArtifact, FirstProofSelectorError> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| FirstProofSelectorError::CreateDir {
            path: parent.display().to_string(),
            error: error.to_string(),
        })?;
    }
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        output_path,
        FIRST_PROOF_EVENT_COUNT_LEDGER_SCHEMA_VERSION,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: FirstProofSelectorError::Serialize,
            read_existing_error: |path, error| FirstProofSelectorError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| FirstProofSelectorError::ExistingArtifactMismatch { path },
            write_error: |path, error| FirstProofSelectorError::Write { path, error },
        },
    )?;
    Ok(FirstProofEventCountLedgerArtifact {
        path: output_path.to_path_buf(),
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        source_rows: report.source_rows,
        event_count_rows: report.event_counts.len() as u64,
    })
}

pub fn write_first_proof_selector_report(
    output_dir: &Path,
    report: &FirstProofSelectorReport,
) -> Result<FirstProofSelectorArtifact, FirstProofSelectorError> {
    fs::create_dir_all(output_dir).map_err(|error| FirstProofSelectorError::CreateDir {
        path: output_dir.display().to_string(),
        error: error.to_string(),
    })?;
    let path = output_dir.join(FIRST_PROOF_SELECTOR_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        FIRST_PROOF_SELECTOR_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: FirstProofSelectorError::Serialize,
            read_existing_error: |path, error| FirstProofSelectorError::ReadExisting {
                path,
                error,
            },
            mismatch_error: |path| FirstProofSelectorError::ExistingArtifactMismatch { path },
            write_error: |path, error| FirstProofSelectorError::Write { path, error },
        },
    )?;
    Ok(FirstProofSelectorArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        selected_asset_count: report.selected_assets.len() as u64,
    })
}

fn parse_event_count_ledger(
    bytes: &[u8],
    path: &str,
) -> Result<FirstProofEventCountLedger, FirstProofSelectorError> {
    if let Ok(report) = serde_json::from_slice::<FirstProofEventCountLedgerReport>(bytes) {
        return Ok(FirstProofEventCountLedger {
            event_counts: report.event_counts,
        });
    }
    serde_json::from_slice(bytes).map_err(|error| {
        FirstProofSelectorError::ParseEventCountLedgerJson {
            path: path.to_string(),
            error: error.to_string(),
        }
    })
}

fn add_batch_event_counts(
    batch: &RecordBatch,
    asset_id_column: &str,
    event_family_column: &str,
    counts: &mut BTreeMap<String, BTreeMap<String, AssetEventAccumulator>>,
    row_group_cursor: &mut SourceRowGroupCursor,
) -> Result<(), FirstProofSelectorError> {
    let asset_values = batch.column_by_name(asset_id_column).ok_or_else(|| {
        FirstProofSelectorError::MissingSourceColumn {
            column: asset_id_column.to_string(),
        }
    })?;
    let event_values = batch.column_by_name(event_family_column).ok_or_else(|| {
        FirstProofSelectorError::MissingSourceColumn {
            column: event_family_column.to_string(),
        }
    })?;
    for row in 0..batch.num_rows() {
        let source_row_group = row_group_cursor.current_row_group()?;
        let asset_id = string_column_value(asset_values.as_ref(), asset_id_column, row)?;
        let event_family = string_column_value(event_values.as_ref(), event_family_column, row)?;
        // Allocate an owned key only on first sight of an asset / event family.
        // Repeated rows (the common case on this per-batch hot path) reuse the
        // existing entry through a borrowed &str lookup instead of allocating a
        // throwaway String every row. The accumulator outlives each Arrow batch,
        // so its keys must stay owned — only the lookup avoids the allocation.
        let asset_counts = match counts.get_mut(asset_id) {
            Some(asset_counts) => asset_counts,
            None => counts.entry(asset_id.to_string()).or_default(),
        };
        let event_count = match asset_counts.get_mut(event_family) {
            Some(event_count) => event_count,
            None => asset_counts.entry(event_family.to_string()).or_default(),
        };
        event_count.rows = event_count.rows.saturating_add(1);
        event_count.source_row_groups.insert(source_row_group);
        row_group_cursor.advance_row();
    }
    Ok(())
}

fn string_column_value<'a>(
    values: &'a dyn Array,
    column: &str,
    row: usize,
) -> Result<&'a str, FirstProofSelectorError> {
    if values.is_null(row) {
        return Err(FirstProofSelectorError::NullSourceColumnValue {
            column: column.to_string(),
            row,
        });
    }
    if let Some(strings) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(strings.value(row));
    }
    if let Some(strings) = values.as_any().downcast_ref::<LargeStringArray>() {
        return Ok(strings.value(row));
    }
    Err(FirstProofSelectorError::UnsupportedSourceColumn {
        column: column.to_string(),
    })
}

fn selected_asset_ids_hash(selected_assets: &[SelectedFirstProofAsset]) -> String {
    if selected_assets.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    for asset in selected_assets {
        hasher.update(asset.asset_id.len().to_le_bytes());
        hasher.update(asset.asset_id.as_bytes());
    }
    hex::encode(hasher.finalize())
}

fn event_count_ledger_hash(event_counts: &[AssetEventCount]) -> String {
    if event_counts.is_empty() {
        return String::new();
    }
    let mut rows = event_counts.to_vec();
    rows.sort_by(|left, right| {
        left.asset_id
            .cmp(&right.asset_id)
            .then(left.event_family.cmp(&right.event_family))
            .then(left.rows.cmp(&right.rows))
    });
    let mut hasher = Sha256::new();
    for row in rows {
        hasher.update(row.asset_id.len().to_le_bytes());
        hasher.update(row.asset_id.as_bytes());
        hasher.update(row.event_family.len().to_le_bytes());
        hasher.update(row.event_family.as_bytes());
        hasher.update(row.rows.to_le_bytes());
        let source_row_groups = row.source_row_groups.into_iter().collect::<BTreeSet<_>>();
        hasher.update((source_row_groups.len() as u64).to_le_bytes());
        for source_row_group in source_row_groups {
            hasher.update(source_row_group.to_le_bytes());
        }
    }
    hex::encode(hasher.finalize())
}
