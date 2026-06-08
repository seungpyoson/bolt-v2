//! Generic first-proof instrument-universe selector.
//!
//! The selector consumes source-proof-owned event-count metadata. Runtime event
//! family labels, budgets, and source identifiers come from config/proof
//! artifacts; this module only applies the generic role rules.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FIRST_PROOF_SELECTOR_SCHEMA_VERSION: &str = "first-proof-selector-report.v1";
pub const FIRST_PROOF_SELECTOR_REPORT_FILE: &str = "first-proof-selector-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofSelection {
    pub required_event_families: Vec<String>,
    pub excluded_event_families: Vec<String>,
    pub row_budget: u64,
    pub max_selected_assets: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetEventCount {
    pub asset_id: String,
    pub event_family: String,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProofEventCountLedger {
    pub event_counts: Vec<AssetEventCount>,
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
pub enum FirstProofSelectorError {
    ReadSpec { path: String, error: String },
    ParseSpecToml { path: String, error: String },
    ReadEventCountLedger { path: String, error: String },
    ParseEventCountLedgerJson { path: String, error: String },
    CreateDir { path: String, error: String },
    ReadExisting { path: String, error: String },
    Write { path: String, error: String },
    ExistingArtifactMismatch { path: String },
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
    let mut counts_by_asset = BTreeMap::<String, BTreeMap<String, u64>>::new();
    for count in event_counts {
        let asset_counts = counts_by_asset.entry(count.asset_id.clone()).or_default();
        *asset_counts.entry(count.event_family.clone()).or_default() = asset_counts
            .get(&count.event_family)
            .copied()
            .unwrap_or(0)
            .saturating_add(count.rows);
    }

    let mut excluded_event_assets = BTreeSet::new();
    let mut excluded_event_row_count = 0_u64;
    let mut eligible = counts_by_asset
        .iter()
        .filter_map(|(asset_id, counts)| {
            let excluded_rows = counts
                .iter()
                .filter(|(event_family, rows)| {
                    excluded_event_families.contains(event_family.as_str()) && **rows > 0
                })
                .map(|(_, rows)| *rows)
                .sum::<u64>();
            if excluded_rows > 0 {
                excluded_event_assets.insert(asset_id.clone());
                excluded_event_row_count = excluded_event_row_count.saturating_add(excluded_rows);
                return None;
            }

            let mut replay_rows = 0_u64;
            for event_family in &required_event_families {
                let rows = counts.get(*event_family).copied().unwrap_or(0);
                if rows == 0 {
                    return None;
                }
                replay_rows = replay_rows.saturating_add(rows);
            }
            if replay_rows > selection.row_budget {
                return None;
            }
            Some(SelectedFirstProofAsset {
                asset_id: asset_id.clone(),
                replay_rows,
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
    let ledger_path_display = spec.event_count_ledger_path.display().to_string();
    let ledger_bytes = fs::read(&spec.event_count_ledger_path).map_err(|error| {
        FirstProofSelectorError::ReadEventCountLedger {
            path: ledger_path_display.clone(),
            error: error.to_string(),
        }
    })?;
    let ledger: FirstProofEventCountLedger =
        serde_json::from_slice(&ledger_bytes).map_err(|error| {
            FirstProofSelectorError::ParseEventCountLedgerJson {
                path: ledger_path_display,
                error: error.to_string(),
            }
        })?;
    let report =
        evaluate_first_proof_selector(spec.selector_id, &ledger.event_counts, &spec.selection);
    write_first_proof_selector_report(&spec.output_dir, &report)
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
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|error| FirstProofSelectorError::Serialize(error.to_string()))?;
    if path.exists() {
        let existing = fs::read(&path).map_err(|error| FirstProofSelectorError::ReadExisting {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
        if existing != bytes {
            return Err(FirstProofSelectorError::ExistingArtifactMismatch {
                path: path.display().to_string(),
            });
        }
    } else {
        fs::write(&path, &bytes).map_err(|error| FirstProofSelectorError::Write {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
    }
    Ok(FirstProofSelectorArtifact {
        path,
        content_hash: content_hash(report)?,
        bytes: bytes.len() as u64,
        selected_asset_count: report.selected_assets.len() as u64,
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

fn content_hash(report: &FirstProofSelectorReport) -> Result<String, FirstProofSelectorError> {
    let bytes = serde_json::to_vec(report)
        .map_err(|error| FirstProofSelectorError::Serialize(error.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hex::encode(hasher.finalize()))
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
    }
    hex::encode(hasher.finalize())
}
