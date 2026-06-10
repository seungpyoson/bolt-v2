//! Venue-scale conversion execution acceptance ledger.
//!
//! This artifact sits after source-universe gates/run planning and before
//! post-conversion completion ledgers. It records whether a venue universe is
//! converted, ready for conversion execution, or still blocked by missing
//! acceptance prerequisites.

use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    backfill_conversion_completion::{
        BackfillConversionCompletionLedger, BackfillConversionCompletionStatus,
    },
    source_universe_conversion_run_plan::{
        SourceUniverseConversionRunPlan, SourceUniverseConversionRunPlanStatus,
    },
    source_universe_object_gates::{
        SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateStatus,
    },
};

pub const SOURCE_UNIVERSE_EXECUTION_ACCEPTANCE_SCHEMA_VERSION: &str =
    "source-universe-execution-acceptance-ledger.v1";
pub const SOURCE_UNIVERSE_EXECUTION_ACCEPTANCE_FILE: &str =
    "source-universe-execution-acceptance-ledger.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionAcceptanceLedgerSpec {
    pub ledger_id: String,
    pub output_dir: PathBuf,
    #[serde(rename = "universe", default)]
    pub universes: Vec<SourceUniverseExecutionAcceptanceUniverseSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionAcceptanceUniverseSpec {
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_manifest_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_conversion_queue_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_object_gates_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_conversion_run_plan_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion_completion_ledger_path: Option<PathBuf>,
    #[serde(default)]
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseExecutionAcceptanceLedgerStatus {
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseExecutionAcceptanceUniverseStatus {
    Converted,
    ReadyForConversionExecution,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionAcceptanceArtifactRef {
    pub role: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionAcceptanceRecord {
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: Option<String>,
    pub status: SourceUniverseExecutionAcceptanceUniverseStatus,
    pub source_gate_count: u64,
    pub source_conversion_batch_count: u64,
    pub planned_conversion_objects: u64,
    pub planned_source_bytes: u64,
    pub required_single_object_operator_runs: u64,
    pub completed_conversion_records: u64,
    pub completed_canonical_rows: u64,
    pub completed_nt_catalog_rows: u64,
    pub remaining_conversion_objects: u64,
    pub artifact_refs: Vec<SourceUniverseExecutionAcceptanceArtifactRef>,
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseExecutionAcceptanceLedger {
    pub schema_version: String,
    pub ledger_id: String,
    pub status: SourceUniverseExecutionAcceptanceLedgerStatus,
    pub universe_count: u64,
    pub converted_universes: u64,
    pub ready_for_conversion_universes: u64,
    pub blocked_universes: u64,
    pub total_planned_conversion_objects: u64,
    pub total_planned_source_bytes: u64,
    pub total_required_single_object_operator_runs: u64,
    pub total_completed_conversion_records: u64,
    pub total_completed_canonical_rows: u64,
    pub total_completed_nt_catalog_rows: u64,
    pub total_remaining_conversion_objects: u64,
    pub records: Vec<SourceUniverseExecutionAcceptanceRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseExecutionAcceptanceLedgerArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub universe_count: u64,
}

pub fn write_source_universe_execution_acceptance_ledger_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseExecutionAcceptanceLedgerArtifact> {
    let spec_text = fs::read_to_string(spec_path).with_context(|| {
        format!(
            "read source-universe execution acceptance spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseExecutionAcceptanceLedgerSpec = toml::from_str(&spec_text)
        .with_context(|| {
            format!(
                "parse source-universe execution acceptance spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_execution_acceptance_ledger(&spec, base_dir)
}

pub fn write_source_universe_execution_acceptance_ledger(
    spec: &SourceUniverseExecutionAcceptanceLedgerSpec,
    base_dir: &Path,
) -> Result<SourceUniverseExecutionAcceptanceLedgerArtifact> {
    let ledger = evaluate_source_universe_execution_acceptance_ledger(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe execution acceptance directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_UNIVERSE_EXECUTION_ACCEPTANCE_FILE);
    let bytes = serde_json::to_vec_pretty(&ledger)
        .context("serialize source-universe execution acceptance ledger")?;
    if path.exists() {
        let existing = fs::read(&path).with_context(|| {
            format!(
                "read existing source-universe execution acceptance ledger {}",
                path.display()
            )
        })?;
        ensure!(
            existing == bytes,
            "dirty source-universe execution acceptance ledger {}: existing file content differs",
            path.display()
        );
    } else {
        fs::write(&path, &bytes).with_context(|| {
            format!(
                "write source-universe execution acceptance ledger {}",
                path.display()
            )
        })?;
    }

    Ok(SourceUniverseExecutionAcceptanceLedgerArtifact {
        path,
        content_hash: sha256_bytes(&bytes),
        bytes: bytes.len() as u64,
        universe_count: ledger.universe_count,
    })
}

pub fn evaluate_source_universe_execution_acceptance_ledger(
    spec: &SourceUniverseExecutionAcceptanceLedgerSpec,
    base_dir: &Path,
) -> Result<SourceUniverseExecutionAcceptanceLedger> {
    ensure!(
        !spec.ledger_id.trim().is_empty(),
        "ledger_id must not be empty"
    );
    ensure!(
        !spec.universes.is_empty(),
        "source-universe execution acceptance ledger must include at least one universe"
    );

    let mut seen_universes = BTreeSet::new();
    let mut records = Vec::new();
    for universe in &spec.universes {
        ensure!(
            seen_universes.insert(universe.universe_id.clone()),
            "duplicate source-universe execution acceptance record {}",
            universe.universe_id
        );
        records.push(evaluate_universe(universe, base_dir)?);
    }

    let converted_universes = records
        .iter()
        .filter(|record| {
            record.status == SourceUniverseExecutionAcceptanceUniverseStatus::Converted
        })
        .count() as u64;
    let ready_for_conversion_universes = records
        .iter()
        .filter(|record| {
            record.status
                == SourceUniverseExecutionAcceptanceUniverseStatus::ReadyForConversionExecution
        })
        .count() as u64;
    let blocked_universes = records
        .iter()
        .filter(|record| record.status == SourceUniverseExecutionAcceptanceUniverseStatus::Blocked)
        .count() as u64;
    let total_planned_conversion_objects = records
        .iter()
        .map(|record| record.planned_conversion_objects)
        .sum();
    let total_planned_source_bytes = records
        .iter()
        .map(|record| record.planned_source_bytes)
        .sum();
    let total_required_single_object_operator_runs = records
        .iter()
        .map(|record| record.required_single_object_operator_runs)
        .sum();
    let total_completed_conversion_records = records
        .iter()
        .map(|record| record.completed_conversion_records)
        .sum();
    let total_completed_canonical_rows = records
        .iter()
        .map(|record| record.completed_canonical_rows)
        .sum();
    let total_completed_nt_catalog_rows = records
        .iter()
        .map(|record| record.completed_nt_catalog_rows)
        .sum();
    let total_remaining_conversion_objects = records
        .iter()
        .map(|record| record.remaining_conversion_objects)
        .sum();
    let status = if converted_universes == records.len() as u64 {
        SourceUniverseExecutionAcceptanceLedgerStatus::Complete
    } else {
        SourceUniverseExecutionAcceptanceLedgerStatus::Incomplete
    };

    Ok(SourceUniverseExecutionAcceptanceLedger {
        schema_version: SOURCE_UNIVERSE_EXECUTION_ACCEPTANCE_SCHEMA_VERSION.to_string(),
        ledger_id: spec.ledger_id.clone(),
        status,
        universe_count: records.len() as u64,
        converted_universes,
        ready_for_conversion_universes,
        blocked_universes,
        total_planned_conversion_objects,
        total_planned_source_bytes,
        total_required_single_object_operator_runs,
        total_completed_conversion_records,
        total_completed_canonical_rows,
        total_completed_nt_catalog_rows,
        total_remaining_conversion_objects,
        records,
    })
}

fn evaluate_universe(
    spec: &SourceUniverseExecutionAcceptanceUniverseSpec,
    base_dir: &Path,
) -> Result<SourceUniverseExecutionAcceptanceRecord> {
    ensure!(
        !spec.universe_id.trim().is_empty(),
        "universe_id must not be empty"
    );
    ensure!(!spec.venue.trim().is_empty(), "venue must not be empty");
    ensure!(!spec.source.trim().is_empty(), "source must not be empty");
    ensure!(!spec.family.trim().is_empty(), "family must not be empty");

    let mut artifact_refs = Vec::new();
    let mut blocking_reasons = spec
        .blocking_reasons
        .iter()
        .filter(|reason| !reason.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();

    push_optional_ref(
        &mut artifact_refs,
        base_dir,
        "source_universe_manifest",
        spec.source_universe_manifest_path.as_ref(),
    )?;
    push_optional_ref(
        &mut artifact_refs,
        base_dir,
        "source_universe_conversion_queue",
        spec.source_universe_conversion_queue_path.as_ref(),
    )?;

    let gates = read_optional_artifact::<SourceUniverseObjectGateMaterialization>(
        base_dir,
        "source_universe_object_gates",
        spec.source_universe_object_gates_path.as_ref(),
        &mut artifact_refs,
    )?;
    let run_plan = read_optional_artifact::<SourceUniverseConversionRunPlan>(
        base_dir,
        "source_universe_conversion_run_plan",
        spec.source_universe_conversion_run_plan_path.as_ref(),
        &mut artifact_refs,
    )?;
    let completion_ledger = read_optional_artifact::<BackfillConversionCompletionLedger>(
        base_dir,
        "conversion_completion_ledger",
        spec.conversion_completion_ledger_path.as_ref(),
        &mut artifact_refs,
    )?;

    let mut table_family = None;
    let mut source_gate_count = 0;
    let mut source_conversion_batch_count = 0;
    let mut planned_conversion_objects = 0;
    let mut planned_source_bytes = 0;
    let mut completed_conversion_records = 0;
    let mut completed_canonical_rows = 0;
    let mut completed_nt_catalog_rows = 0;

    match gates.as_ref() {
        Some(gates) => {
            table_family = Some(gates.table_family.clone());
            source_gate_count = gates.accepted_gate_count;
            if gates.status != SourceUniverseObjectGateStatus::Ready {
                blocking_reasons.push("source_universe_object_gates_not_ready".to_string());
            }
            if gates.accepted_gate_count != gates.work_item_count {
                blocking_reasons
                    .push("source_universe_object_gates_do_not_cover_all_work_items".to_string());
            }
            if gates.records.len() as u64 != gates.accepted_gate_count {
                blocking_reasons.push(
                    "source_universe_object_gate_records_do_not_match_accepted_count".to_string(),
                );
            }
        }
        None => blocking_reasons.push("missing_source_universe_object_gates".to_string()),
    }

    match run_plan.as_ref() {
        Some(run_plan) => {
            table_family.get_or_insert_with(|| run_plan.table_family.clone());
            source_conversion_batch_count = run_plan.run_count;
            planned_conversion_objects = run_plan.planned_object_count;
            planned_source_bytes = run_plan.planned_source_bytes;
            if run_plan.status != SourceUniverseConversionRunPlanStatus::Ready {
                blocking_reasons.push("source_universe_conversion_run_plan_not_ready".to_string());
            }
            if run_plan.planned_object_count != run_plan.object_count {
                blocking_reasons
                    .push("source_universe_conversion_run_plan_object_count_mismatch".to_string());
            }
            if run_plan.planned_source_bytes != run_plan.total_source_bytes {
                blocking_reasons
                    .push("source_universe_conversion_run_plan_source_bytes_mismatch".to_string());
            }
        }
        None => blocking_reasons.push("missing_source_universe_conversion_run_plan".to_string()),
    }

    if let (Some(gates), Some(run_plan)) = (gates.as_ref(), run_plan.as_ref()) {
        if gates.universe_id != run_plan.universe_id
            || gates.gate_id != run_plan.gate_id
            || gates.queue_id != run_plan.queue_id
            || gates.manifest_id != run_plan.manifest_id
        {
            blocking_reasons
                .push("source_universe_object_gates_run_plan_identity_mismatch".to_string());
        }
        if gates.accepted_gate_count != run_plan.planned_object_count {
            blocking_reasons
                .push("source_universe_object_gates_run_plan_object_count_mismatch".to_string());
        }
        if gates.total_accepted_bytes != run_plan.planned_source_bytes {
            blocking_reasons
                .push("source_universe_object_gates_run_plan_source_bytes_mismatch".to_string());
        }
    }

    if let Some(completion_ledger) = completion_ledger.as_ref() {
        completed_conversion_records = completion_ledger.record_count;
        completed_canonical_rows = completion_ledger.total_canonical_rows;
        completed_nt_catalog_rows = completion_ledger.total_nt_iterations;
        if completion_ledger.status != BackfillConversionCompletionStatus::Ready {
            blocking_reasons.push("conversion_completion_ledger_not_ready".to_string());
        }
        if planned_conversion_objects > 0
            && completion_ledger.record_count != planned_conversion_objects
        {
            blocking_reasons.push("conversion_completion_ledger_record_count_mismatch".to_string());
        }
    }

    blocking_reasons.sort();
    blocking_reasons.dedup();

    let status = if !blocking_reasons.is_empty() {
        SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
    } else if completion_ledger.is_some() {
        SourceUniverseExecutionAcceptanceUniverseStatus::Converted
    } else {
        SourceUniverseExecutionAcceptanceUniverseStatus::ReadyForConversionExecution
    };
    let remaining_conversion_objects =
        planned_conversion_objects.saturating_sub(completed_conversion_records);
    let required_single_object_operator_runs =
        if status == SourceUniverseExecutionAcceptanceUniverseStatus::Converted {
            0
        } else {
            remaining_conversion_objects
        };

    Ok(SourceUniverseExecutionAcceptanceRecord {
        universe_id: spec.universe_id.clone(),
        venue: spec.venue.clone(),
        source: spec.source.clone(),
        family: spec.family.clone(),
        table_family,
        status,
        source_gate_count,
        source_conversion_batch_count,
        planned_conversion_objects,
        planned_source_bytes,
        required_single_object_operator_runs,
        completed_conversion_records,
        completed_canonical_rows,
        completed_nt_catalog_rows,
        remaining_conversion_objects,
        artifact_refs,
        blocking_reasons,
    })
}

fn read_optional_artifact<T>(
    base_dir: &Path,
    role: &str,
    path: Option<&PathBuf>,
    artifact_refs: &mut Vec<SourceUniverseExecutionAcceptanceArtifactRef>,
) -> Result<Option<T>>
where
    T: for<'de> Deserialize<'de>,
{
    let Some(path) = path else {
        return Ok(None);
    };
    let resolved = resolve_existing_path(base_dir, path);
    let bytes = fs::read(&resolved)
        .with_context(|| format!("read {role} artifact {}", resolved.display()))?;
    artifact_refs.push(SourceUniverseExecutionAcceptanceArtifactRef {
        role: role.to_string(),
        path: resolved,
        sha256: sha256_bytes(&bytes),
    });
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {role} artifact {}", path.display()))
        .map(Some)
}

fn push_optional_ref(
    artifact_refs: &mut Vec<SourceUniverseExecutionAcceptanceArtifactRef>,
    base_dir: &Path,
    role: &str,
    path: Option<&PathBuf>,
) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let resolved = resolve_existing_path(base_dir, path);
    let bytes = fs::read(&resolved)
        .with_context(|| format!("read {role} artifact {}", resolved.display()))?;
    artifact_refs.push(SourceUniverseExecutionAcceptanceArtifactRef {
        role: role.to_string(),
        path: resolved,
        sha256: sha256_bytes(&bytes),
    });
    Ok(())
}

fn resolve_output_dir(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if looks_repo_relative(path)
        && let Some(candidate) = resolve_from_known_anchors(path)
    {
        return candidate;
    }
    let base_candidate = base_dir.join(path);
    if base_candidate
        .parent()
        .is_some_and(|parent| parent.exists())
    {
        return base_candidate;
    }
    resolve_from_known_anchors(path).unwrap_or(base_candidate)
}

fn resolve_existing_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() || path.exists() {
        return path.to_path_buf();
    }
    let base_candidate = base_dir.join(path);
    if base_candidate.exists() {
        return base_candidate;
    }
    resolve_from_known_anchors(path).unwrap_or_else(|| path.to_path_buf())
}

fn resolve_from_known_anchors(path: &Path) -> Option<PathBuf> {
    let mut anchors = Vec::new();
    if let Ok(current_dir) = std::env::current_dir() {
        anchors.push(current_dir);
    }
    anchors.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));

    for anchor in anchors {
        for ancestor in anchor.ancestors() {
            let candidate = ancestor.join(path);
            if candidate.exists() || candidate.parent().is_some_and(Path::exists) {
                return Some(candidate);
            }
        }
    }
    None
}

fn looks_repo_relative(path: &Path) -> bool {
    path.components()
        .next()
        .is_some_and(|component| matches!(component, Component::Normal(first) if first == "specs"))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
