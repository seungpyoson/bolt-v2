//! Venue-scale conversion execution acceptance ledger.
//!
//! This artifact sits after source-universe gates/run planning and before
//! post-conversion completion ledgers. It records whether a venue universe is
//! converted, ready for conversion execution, or still blocked by missing
//! acceptance prerequisites.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;
use crate::path_resolution::{
    portable_artifact_path_for_spec, resolve_existing_path, resolve_output_dir,
    stable_artifact_identity_path_for_spec,
};
use crate::reference_artifact::ReferenceArtifactPin;
use crate::{
    backfill_conversion_completion::{
        BackfillConversionCompletionLedger, BackfillConversionCompletionStatus,
    },
    source_universe_conversion_queue::SourceUniverseConversionQueue,
    source_universe_conversion_run_plan::{
        SourceUniverseConversionRunPlan, SourceUniverseConversionRunPlanStatus,
    },
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackStatus,
    },
    source_universe_object_gates::{
        SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateStatus,
    },
    source_universe_operator_inputs::{
        SourceUniverseOperatorInputRecordStatus, SourceUniverseOperatorInputs,
        SourceUniverseOperatorInputsStatus,
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
    pub source_universe_manifest_artifact_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_conversion_queue_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_object_gates_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_conversion_run_plan_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_operator_inputs_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_universe_execution_pack_path: Option<PathBuf>,
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
    PartiallyReadyForConversionExecution,
    Blocked,
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
    pub operator_input_count: u64,
    pub ready_operator_input_count: u64,
    pub blocked_operator_input_count: u64,
    pub planned_conversion_objects: u64,
    pub planned_source_bytes: u64,
    pub required_single_object_operator_runs: u64,
    pub executable_single_object_operator_runs: u64,
    pub materialized_single_object_operator_runs: u64,
    pub withheld_conversion_objects: u64,
    pub completed_conversion_records: u64,
    pub completed_canonical_rows: u64,
    pub completed_nt_catalog_rows: u64,
    pub remaining_conversion_objects: u64,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
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
    pub partially_ready_for_conversion_universes: u64,
    pub blocked_universes: u64,
    pub total_planned_conversion_objects: u64,
    pub total_planned_source_bytes: u64,
    pub total_required_single_object_operator_runs: u64,
    pub total_executable_single_object_operator_runs: u64,
    pub total_materialized_single_object_operator_runs: u64,
    pub total_withheld_conversion_objects: u64,
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
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_UNIVERSE_EXECUTION_ACCEPTANCE_FILE,
        &ledger,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| {
        format!(
            "write source-universe execution acceptance ledger {}",
            path.display()
        )
    })?;

    Ok(SourceUniverseExecutionAcceptanceLedgerArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
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
    let partially_ready_for_conversion_universes = records
        .iter()
        .filter(|record| {
            record.status
                == SourceUniverseExecutionAcceptanceUniverseStatus::PartiallyReadyForConversionExecution
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
    let total_executable_single_object_operator_runs = records
        .iter()
        .map(|record| record.executable_single_object_operator_runs)
        .sum();
    let total_materialized_single_object_operator_runs = records
        .iter()
        .map(|record| record.materialized_single_object_operator_runs)
        .sum();
    let total_withheld_conversion_objects = records
        .iter()
        .map(|record| record.withheld_conversion_objects)
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
        partially_ready_for_conversion_universes,
        blocked_universes,
        total_planned_conversion_objects,
        total_planned_source_bytes,
        total_required_single_object_operator_runs,
        total_executable_single_object_operator_runs,
        total_materialized_single_object_operator_runs,
        total_withheld_conversion_objects,
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
    let mut has_non_operator_blocking_reasons = !blocking_reasons.is_empty();

    push_optional_ref(
        &mut artifact_refs,
        base_dir,
        "source_universe_manifest",
        spec.source_universe_manifest_path.as_ref(),
        spec.source_universe_manifest_artifact_path.as_deref(),
    )?;
    let queue = read_optional_artifact::<SourceUniverseConversionQueue>(
        base_dir,
        "source_universe_conversion_queue",
        spec.source_universe_conversion_queue_path.as_ref(),
        &mut artifact_refs,
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
    let operator_inputs = read_optional_artifact::<SourceUniverseOperatorInputs>(
        base_dir,
        "source_universe_operator_inputs",
        spec.source_universe_operator_inputs_path.as_ref(),
        &mut artifact_refs,
    )?;
    let execution_pack = read_optional_artifact::<SourceUniverseExecutionPack>(
        base_dir,
        "source_universe_execution_pack",
        spec.source_universe_execution_pack_path.as_ref(),
        &mut artifact_refs,
    )?;
    let completion_ledger = read_optional_artifact::<BackfillConversionCompletionLedger>(
        base_dir,
        "conversion_completion_ledger",
        spec.conversion_completion_ledger_path.as_ref(),
        &mut artifact_refs,
    )?;

    // spec.family is the declared source family for the universe; every loaded
    // artifact that carries its own `family` must agree with it. This is the
    // missing spec-vs-artifact leg of the existing artifact-vs-artifact identity
    // checks below (gates/run_plan/operator_inputs), and catches spec drift on
    // any venue/artifact, not just one instance.
    for (role, artifact_family) in [
        (
            "source_universe_conversion_queue",
            queue.as_ref().map(|artifact| artifact.family.as_str()),
        ),
        (
            "source_universe_object_gates",
            gates.as_ref().map(|artifact| artifact.family.as_str()),
        ),
        (
            "source_universe_conversion_run_plan",
            run_plan.as_ref().map(|artifact| artifact.family.as_str()),
        ),
        (
            "source_universe_operator_inputs",
            operator_inputs
                .as_ref()
                .map(|artifact| artifact.family.as_str()),
        ),
        (
            "source_universe_execution_pack",
            execution_pack
                .as_ref()
                .map(|artifact| artifact.family.as_str()),
        ),
    ] {
        let Some(artifact_family) = artifact_family else {
            continue;
        };
        if artifact_family != spec.family.as_str() {
            blocking_reasons.push(format!("source_universe_spec_family_mismatch_{role}"));
            has_non_operator_blocking_reasons = true;
        }
    }

    let mut table_family = None;
    let mut source_gate_count = 0;
    let mut source_conversion_batch_count = 0;
    let mut operator_input_count = 0;
    let mut ready_operator_input_count = 0;
    let mut blocked_operator_input_count = 0;
    let mut planned_conversion_objects = 0;
    let mut planned_source_bytes = 0;
    let mut materialized_single_object_operator_runs = 0;
    let mut completed_conversion_records = 0;
    let mut completed_canonical_rows = 0;
    let mut completed_nt_catalog_rows = 0;

    if let Some(queue) = queue.as_ref() {
        table_family = Some(queue.table_family.clone());
        planned_conversion_objects = queue.pending_conversion_items;
        planned_source_bytes = queue.total_source_bytes;
    }

    match gates.as_ref() {
        Some(gates) => {
            table_family = Some(gates.table_family.clone());
            source_gate_count = gates.accepted_gate_count;
            if gates.status != SourceUniverseObjectGateStatus::Ready {
                blocking_reasons.push("source_universe_object_gates_not_ready".to_string());
                has_non_operator_blocking_reasons = true;
            }
            if gates.accepted_gate_count != gates.work_item_count {
                blocking_reasons
                    .push("source_universe_object_gates_do_not_cover_all_work_items".to_string());
                has_non_operator_blocking_reasons = true;
            }
            if gates.records.len() as u64 != gates.accepted_gate_count {
                blocking_reasons.push(
                    "source_universe_object_gate_records_do_not_match_accepted_count".to_string(),
                );
                has_non_operator_blocking_reasons = true;
            }
        }
        None => {
            blocking_reasons.push("missing_source_universe_object_gates".to_string());
            has_non_operator_blocking_reasons = true;
        }
    }

    match run_plan.as_ref() {
        Some(run_plan) => {
            table_family.get_or_insert_with(|| run_plan.table_family.clone());
            source_conversion_batch_count = run_plan.run_count;
            planned_conversion_objects = run_plan.planned_object_count;
            planned_source_bytes = run_plan.planned_source_bytes;
            if run_plan.status != SourceUniverseConversionRunPlanStatus::Ready {
                blocking_reasons.push("source_universe_conversion_run_plan_not_ready".to_string());
                has_non_operator_blocking_reasons = true;
            }
            if run_plan.planned_object_count != run_plan.object_count {
                blocking_reasons
                    .push("source_universe_conversion_run_plan_object_count_mismatch".to_string());
                has_non_operator_blocking_reasons = true;
            }
            if run_plan.planned_source_bytes != run_plan.total_source_bytes {
                blocking_reasons
                    .push("source_universe_conversion_run_plan_source_bytes_mismatch".to_string());
                has_non_operator_blocking_reasons = true;
            }
        }
        None => {
            blocking_reasons.push("missing_source_universe_conversion_run_plan".to_string());
            has_non_operator_blocking_reasons = true;
        }
    }

    if let (Some(gates), Some(run_plan)) = (gates.as_ref(), run_plan.as_ref()) {
        if gates.universe_id != run_plan.universe_id
            || gates.gate_id != run_plan.gate_id
            || gates.queue_id != run_plan.queue_id
            || gates.manifest_id != run_plan.manifest_id
        {
            blocking_reasons
                .push("source_universe_object_gates_run_plan_identity_mismatch".to_string());
            has_non_operator_blocking_reasons = true;
        }
        if gates.accepted_gate_count != run_plan.planned_object_count {
            blocking_reasons
                .push("source_universe_object_gates_run_plan_object_count_mismatch".to_string());
            has_non_operator_blocking_reasons = true;
        }
        if gates.total_accepted_bytes != run_plan.planned_source_bytes {
            blocking_reasons
                .push("source_universe_object_gates_run_plan_source_bytes_mismatch".to_string());
            has_non_operator_blocking_reasons = true;
        }
    }

    match operator_inputs.as_ref() {
        Some(operator_inputs) => {
            table_family.get_or_insert_with(|| operator_inputs.table_family.clone());
            operator_input_count = operator_inputs.records.len() as u64;
            ready_operator_input_count = operator_inputs.ready_input_count;
            blocked_operator_input_count = operator_inputs.blocked_input_count;
            validate_operator_inputs(
                operator_inputs,
                OperatorInputValidationContext {
                    gates: gates.as_ref(),
                    run_plan: run_plan.as_ref(),
                    planned_conversion_objects,
                    planned_source_bytes,
                    source_conversion_batch_count,
                },
                &mut blocking_reasons,
                &mut has_non_operator_blocking_reasons,
            );
        }
        None if gates.is_some() && run_plan.is_some() && completion_ledger.is_none() => {
            blocking_reasons.push("missing_source_universe_operator_inputs".to_string());
            has_non_operator_blocking_reasons = true;
        }
        None => {}
    }

    if let Some(execution_pack) = execution_pack.as_ref() {
        table_family.get_or_insert_with(|| execution_pack.table_family.clone());
        materialized_single_object_operator_runs = execution_pack.materialized_record_count;
        validate_execution_pack(
            execution_pack,
            ExecutionPackValidationContext {
                operator_inputs: operator_inputs.as_ref(),
                planned_conversion_objects,
                planned_source_bytes,
                ready_operator_input_count,
            },
            &mut blocking_reasons,
            &mut has_non_operator_blocking_reasons,
        );
    }

    if let Some(completion_ledger) = completion_ledger.as_ref() {
        completed_conversion_records = completion_ledger.record_count;
        completed_canonical_rows = completion_ledger.total_canonical_rows;
        completed_nt_catalog_rows = completion_ledger.total_nt_iterations;
        if completion_ledger.status != BackfillConversionCompletionStatus::Ready {
            blocking_reasons.push("conversion_completion_ledger_not_ready".to_string());
            has_non_operator_blocking_reasons = true;
        }
        if planned_conversion_objects > 0
            && completion_ledger.record_count != planned_conversion_objects
        {
            blocking_reasons.push("conversion_completion_ledger_record_count_mismatch".to_string());
            has_non_operator_blocking_reasons = true;
        }
    }

    blocking_reasons.sort();
    blocking_reasons.dedup();

    let has_partial_operator_inputs = operator_inputs.is_some()
        && ready_operator_input_count > 0
        && blocked_operator_input_count > 0;
    let status = if !blocking_reasons.is_empty() {
        if has_partial_operator_inputs && !has_non_operator_blocking_reasons {
            SourceUniverseExecutionAcceptanceUniverseStatus::PartiallyReadyForConversionExecution
        } else {
            SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
        }
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
    let executable_single_object_operator_runs = if status
        == SourceUniverseExecutionAcceptanceUniverseStatus::Blocked
        || status == SourceUniverseExecutionAcceptanceUniverseStatus::Converted
    {
        0
    } else if operator_inputs.is_some() {
        ready_operator_input_count.saturating_sub(completed_conversion_records)
    } else {
        remaining_conversion_objects
    };
    let withheld_conversion_objects =
        remaining_conversion_objects.saturating_sub(executable_single_object_operator_runs);

    Ok(SourceUniverseExecutionAcceptanceRecord {
        universe_id: spec.universe_id.clone(),
        venue: spec.venue.clone(),
        source: spec.source.clone(),
        family: spec.family.clone(),
        table_family,
        status,
        source_gate_count,
        source_conversion_batch_count,
        operator_input_count,
        ready_operator_input_count,
        blocked_operator_input_count,
        planned_conversion_objects,
        planned_source_bytes,
        required_single_object_operator_runs,
        executable_single_object_operator_runs,
        materialized_single_object_operator_runs,
        withheld_conversion_objects,
        completed_conversion_records,
        completed_canonical_rows,
        completed_nt_catalog_rows,
        remaining_conversion_objects,
        artifact_refs,
        blocking_reasons,
    })
}

struct OperatorInputValidationContext<'a> {
    gates: Option<&'a SourceUniverseObjectGateMaterialization>,
    run_plan: Option<&'a SourceUniverseConversionRunPlan>,
    planned_conversion_objects: u64,
    planned_source_bytes: u64,
    source_conversion_batch_count: u64,
}

fn validate_operator_inputs(
    operator_inputs: &SourceUniverseOperatorInputs,
    context: OperatorInputValidationContext<'_>,
    blocking_reasons: &mut Vec<String>,
    has_non_operator_blocking_reasons: &mut bool,
) {
    let actual_ready_input_count = operator_inputs
        .records
        .iter()
        .filter(|record| record.status == SourceUniverseOperatorInputRecordStatus::Ready)
        .count() as u64;
    let actual_blocked_input_count = operator_inputs
        .records
        .iter()
        .filter(|record| record.status == SourceUniverseOperatorInputRecordStatus::Blocked)
        .count() as u64;

    if operator_inputs.ready_input_count != actual_ready_input_count {
        blocking_reasons.push("source_universe_operator_inputs_ready_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if operator_inputs.blocked_input_count != actual_blocked_input_count {
        blocking_reasons.push("source_universe_operator_inputs_blocked_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if operator_inputs.ready_input_count + operator_inputs.blocked_input_count
        != operator_inputs.records.len() as u64
    {
        blocking_reasons.push("source_universe_operator_inputs_record_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if operator_inputs.planned_object_count != operator_inputs.records.len() as u64 {
        blocking_reasons
            .push("source_universe_operator_inputs_planned_object_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if context.planned_conversion_objects > 0
        && operator_inputs.planned_object_count != context.planned_conversion_objects
    {
        blocking_reasons.push("source_universe_operator_inputs_planned_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if context.planned_source_bytes > 0
        && operator_inputs.planned_source_bytes != context.planned_source_bytes
    {
        blocking_reasons.push("source_universe_operator_inputs_source_bytes_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if context.source_conversion_batch_count > 0
        && operator_inputs.conversion_run_count != context.source_conversion_batch_count
    {
        blocking_reasons.push("source_universe_operator_inputs_run_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if operator_inputs.status != SourceUniverseOperatorInputsStatus::Ready {
        blocking_reasons.push("blocked_source_universe_operator_input_records".to_string());
    }
    for record in operator_inputs
        .records
        .iter()
        .filter(|record| record.status == SourceUniverseOperatorInputRecordStatus::Blocked)
    {
        blocking_reasons.extend(record.blocking_reasons.iter().cloned());
    }

    if let Some(gates) = context.gates
        && (operator_inputs.universe_id != gates.universe_id
            || operator_inputs.gate_id != gates.gate_id
            || operator_inputs.venue != gates.venue
            || operator_inputs.source != gates.source
            || operator_inputs.family != gates.family
            || operator_inputs.table_family != gates.table_family)
    {
        blocking_reasons
            .push("source_universe_operator_inputs_object_gates_identity_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }

    if let Some(run_plan) = context.run_plan
        && (operator_inputs.universe_id != run_plan.universe_id
            || operator_inputs.gate_id != run_plan.gate_id
            || operator_inputs.conversion_run_plan_id != run_plan.plan_id
            || operator_inputs.venue != run_plan.venue
            || operator_inputs.source != run_plan.source
            || operator_inputs.family != run_plan.family
            || operator_inputs.table_family != run_plan.table_family)
    {
        blocking_reasons
            .push("source_universe_operator_inputs_run_plan_identity_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
}

struct ExecutionPackValidationContext<'a> {
    operator_inputs: Option<&'a SourceUniverseOperatorInputs>,
    planned_conversion_objects: u64,
    planned_source_bytes: u64,
    ready_operator_input_count: u64,
}

fn validate_execution_pack(
    execution_pack: &SourceUniverseExecutionPack,
    context: ExecutionPackValidationContext<'_>,
    blocking_reasons: &mut Vec<String>,
    has_non_operator_blocking_reasons: &mut bool,
) {
    if execution_pack.status == SourceUniverseExecutionPackStatus::Blocked {
        blocking_reasons.push("source_universe_execution_pack_blocked".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if execution_pack.skipped_executable_record_count > 0 {
        blocking_reasons
            .push("source_universe_execution_pack_skipped_executable_records".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if execution_pack.materialized_record_count > execution_pack.executable_record_count {
        blocking_reasons.push(
            "source_universe_execution_pack_materialized_count_exceeds_executable".to_string(),
        );
        *has_non_operator_blocking_reasons = true;
    }
    if execution_pack.materialized_record_count < execution_pack.executable_record_count
        && execution_pack.skipped_executable_record_count == 0
    {
        blocking_reasons
            .push("source_universe_execution_pack_materialized_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if context.planned_conversion_objects > 0
        && execution_pack.planned_object_count != context.planned_conversion_objects
    {
        blocking_reasons.push("source_universe_execution_pack_planned_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if context.planned_source_bytes > 0
        && execution_pack.executable_source_bytes + execution_pack.materialized_source_bytes == 0
    {
        blocking_reasons.push("source_universe_execution_pack_source_bytes_missing".to_string());
        *has_non_operator_blocking_reasons = true;
    }
    if context.ready_operator_input_count > 0
        && execution_pack.executable_record_count != context.ready_operator_input_count
    {
        blocking_reasons
            .push("source_universe_execution_pack_executable_count_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }

    if let Some(operator_inputs) = context.operator_inputs
        && (execution_pack.universe_id != operator_inputs.universe_id
            || execution_pack.gate_id != operator_inputs.gate_id
            || execution_pack.input_id != operator_inputs.input_id
            || execution_pack.venue != operator_inputs.venue
            || execution_pack.source != operator_inputs.source
            || execution_pack.family != operator_inputs.family
            || execution_pack.table_family != operator_inputs.table_family)
    {
        blocking_reasons
            .push("source_universe_execution_pack_operator_inputs_identity_mismatch".to_string());
        *has_non_operator_blocking_reasons = true;
    }
}

fn read_optional_artifact<T>(
    base_dir: &Path,
    role: &str,
    path: Option<&PathBuf>,
    artifact_refs: &mut Vec<ReferenceArtifactPin>,
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
    artifact_refs.push(ReferenceArtifactPin {
        role: role.to_string(),
        path: portable_artifact_path_for_spec(&resolved, path)?,
        sha256: sha256_hex(&bytes),
    });
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {role} artifact {}", path.display()))
        .map(Some)
}

fn push_optional_ref(
    artifact_refs: &mut Vec<ReferenceArtifactPin>,
    base_dir: &Path,
    role: &str,
    path: Option<&PathBuf>,
    artifact_identity_path: Option<&Path>,
) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let resolved = resolve_existing_path(base_dir, path);
    let bytes = fs::read(&resolved)
        .with_context(|| format!("read {role} artifact {}", resolved.display()))?;
    artifact_refs.push(ReferenceArtifactPin {
        role: role.to_string(),
        path: stable_artifact_identity_path_for_spec(&resolved, path, artifact_identity_path)?,
        sha256: sha256_hex(&bytes),
    });
    Ok(())
}
