//! Source-universe conversion work-order materialization.
//!
//! This pre-payload artifact narrows source-universe operator inputs to the
//! records a bulk conversion runner may execute now. It does not download
//! source objects, materialize run specs, convert rows, or publish catalogs.

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
};
use crate::reference_artifact::ReferenceArtifactPin;
use crate::{
    canonical_trades::RawPayloadContainer,
    source_universe_operator_inputs::{
        SourceUniverseOperatorInputRecord, SourceUniverseOperatorInputRecordStatus,
        SourceUniverseOperatorInputs,
    },
};

pub const SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION: &str =
    "source-universe-conversion-work-order.v1";
pub const SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_FILE: &str =
    "source-universe-conversion-work-order.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionWorkOrderSpec {
    pub work_order_id: String,
    pub source_universe_operator_inputs_path: PathBuf,
    pub output_dir: PathBuf,
    #[serde(default)]
    pub overwrite_existing_artifacts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseConversionWorkOrderStatus {
    Ready,
    PartiallyReady,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionWorkOrderRecord {
    pub sequence: u64,
    pub work_item_id: String,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub source_uri: String,
    pub source_url: String,
    pub selected_object_sha256: String,
    pub selected_object_bytes: u64,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub accepted_tranche_id: String,
    pub output_prefix: String,
    pub instrument_key: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub raw_payload_container: RawPayloadContainer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zip_member: Option<String>,
    pub max_decoded_bytes: u64,
    pub max_source_rows: u64,
    pub max_projected_row_groups: u64,
    pub max_wall_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionWorkOrderWithheldRecord {
    pub work_item_id: String,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub selected_object_bytes: u64,
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionWorkOrder {
    pub schema_version: String,
    pub work_order_id: String,
    pub status: SourceUniverseConversionWorkOrderStatus,
    pub input_id: String,
    pub gate_id: String,
    pub conversion_run_plan_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub operator_run_id_prefix: String,
    pub planned_object_count: u64,
    pub planned_source_bytes: u64,
    pub operator_input_count: u64,
    pub ready_input_count: u64,
    pub blocked_input_count: u64,
    pub conversion_run_count: u64,
    pub executable_record_count: u64,
    pub withheld_record_count: u64,
    pub executable_source_bytes: u64,
    pub withheld_source_bytes: u64,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub records: Vec<SourceUniverseConversionWorkOrderRecord>,
    pub withheld_records: Vec<SourceUniverseConversionWorkOrderWithheldRecord>,
    pub blocking_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseConversionWorkOrderArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub executable_record_count: u64,
    pub withheld_record_count: u64,
}

pub fn write_source_universe_conversion_work_order_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseConversionWorkOrderArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe conversion work-order spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseConversionWorkOrderSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source-universe conversion work-order spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_conversion_work_order(&spec, base_dir)
}

pub fn write_source_universe_conversion_work_order(
    spec: &SourceUniverseConversionWorkOrderSpec,
    base_dir: &Path,
) -> Result<SourceUniverseConversionWorkOrderArtifact> {
    let work_order = evaluate_source_universe_conversion_work_order(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe conversion work-order directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped_overwrite(
        &path,
        SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_FILE,
        &work_order,
        spec.overwrite_existing_artifacts,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: |error| {
                anyhow::anyhow!("serialize source-universe conversion work-order: {error}")
            },
            read_existing_error: |path, error| {
                anyhow::anyhow!(
                    "read existing source-universe conversion work-order {path}: {error}"
                )
            },
            mismatch_error: |path| {
                anyhow::anyhow!(
                    "dirty source-universe conversion work-order {path}: existing file content differs"
                )
            },
            write_error: |path, error| {
                anyhow::anyhow!("write source-universe conversion work-order {path}: {error}")
            },
        },
    )?;

    Ok(SourceUniverseConversionWorkOrderArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        executable_record_count: work_order.executable_record_count,
        withheld_record_count: work_order.withheld_record_count,
    })
}

pub fn evaluate_source_universe_conversion_work_order(
    spec: &SourceUniverseConversionWorkOrderSpec,
    base_dir: &Path,
) -> Result<SourceUniverseConversionWorkOrder> {
    ensure!(
        !spec.work_order_id.trim().is_empty(),
        "work_order_id must not be empty"
    );

    let inputs_path = resolve_existing_path(base_dir, &spec.source_universe_operator_inputs_path);
    let inputs_bytes = fs::read(&inputs_path).with_context(|| {
        format!(
            "read source-universe operator inputs {}",
            inputs_path.display()
        )
    })?;
    let inputs_hash = sha256_hex(&inputs_bytes);
    let inputs: SourceUniverseOperatorInputs =
        serde_json::from_slice(&inputs_bytes).with_context(|| {
            format!(
                "parse source-universe operator inputs {}",
                inputs_path.display()
            )
        })?;
    validate_operator_input_counts(&inputs)?;

    let mut records = Vec::new();
    let mut withheld_records = Vec::new();
    let mut seen_work_items = BTreeSet::new();
    for input in &inputs.records {
        ensure!(
            seen_work_items.insert(input.work_item_id.clone()),
            "duplicate source-universe work-order item {}",
            input.work_item_id
        );
        match input.status {
            SourceUniverseOperatorInputRecordStatus::Ready => {
                records.push(work_order_record(records.len() as u64, input));
            }
            SourceUniverseOperatorInputRecordStatus::Blocked => {
                withheld_records.push(withheld_record(input));
            }
        }
    }

    let executable_source_bytes = records
        .iter()
        .map(|record| record.selected_object_bytes)
        .sum();
    let withheld_source_bytes = withheld_records
        .iter()
        .map(|record| record.selected_object_bytes)
        .sum();
    let mut blocking_reasons = inputs.blocking_reasons.clone();
    for record in &withheld_records {
        blocking_reasons.extend(record.blocking_reasons.iter().cloned());
    }
    if !withheld_records.is_empty() && blocking_reasons.is_empty() {
        blocking_reasons.push("blocked_source_universe_operator_input_records".to_string());
    }
    blocking_reasons.sort();
    blocking_reasons.dedup();

    let executable_record_count = records.len() as u64;
    let withheld_record_count = withheld_records.len() as u64;
    let status = if executable_record_count == inputs.planned_object_count
        && withheld_record_count == 0
        && blocking_reasons.is_empty()
    {
        SourceUniverseConversionWorkOrderStatus::Ready
    } else if executable_record_count > 0 {
        SourceUniverseConversionWorkOrderStatus::PartiallyReady
    } else {
        SourceUniverseConversionWorkOrderStatus::Blocked
    };

    Ok(SourceUniverseConversionWorkOrder {
        schema_version: SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION.to_string(),
        work_order_id: spec.work_order_id.clone(),
        status,
        input_id: inputs.input_id,
        gate_id: inputs.gate_id,
        conversion_run_plan_id: inputs.conversion_run_plan_id,
        universe_id: inputs.universe_id,
        venue: inputs.venue,
        source: inputs.source,
        family: inputs.family,
        table_family: inputs.table_family,
        operator_run_id_prefix: inputs.operator_run_id_prefix,
        planned_object_count: inputs.planned_object_count,
        planned_source_bytes: inputs.planned_source_bytes,
        operator_input_count: inputs.records.len() as u64,
        ready_input_count: inputs.ready_input_count,
        blocked_input_count: inputs.blocked_input_count,
        conversion_run_count: inputs.conversion_run_count,
        executable_record_count,
        withheld_record_count,
        executable_source_bytes,
        withheld_source_bytes,
        artifact_refs: vec![ReferenceArtifactPin {
            role: "source_universe_operator_inputs".to_string(),
            path: portable_artifact_path_for_spec(
                &inputs_path,
                &spec.source_universe_operator_inputs_path,
            )?,
            sha256: inputs_hash,
        }],
        records,
        withheld_records,
        blocking_reasons,
    })
}

fn validate_operator_input_counts(inputs: &SourceUniverseOperatorInputs) -> Result<()> {
    ensure!(
        inputs.records.len() as u64 == inputs.planned_object_count,
        "source-universe operator inputs records do not match planned_object_count"
    );
    let ready_count = inputs
        .records
        .iter()
        .filter(|record| record.status == SourceUniverseOperatorInputRecordStatus::Ready)
        .count() as u64;
    let blocked_count = inputs.records.len() as u64 - ready_count;
    ensure!(
        ready_count == inputs.ready_input_count,
        "source-universe operator inputs ready_input_count mismatch"
    );
    ensure!(
        blocked_count == inputs.blocked_input_count,
        "source-universe operator inputs blocked_input_count mismatch"
    );
    Ok(())
}

fn work_order_record(
    sequence: u64,
    input: &SourceUniverseOperatorInputRecord,
) -> SourceUniverseConversionWorkOrderRecord {
    SourceUniverseConversionWorkOrderRecord {
        sequence,
        work_item_id: input.work_item_id.clone(),
        operator_run_id: input.operator_run_id.clone(),
        source_binding: input.source_binding.clone(),
        category: input.category.clone(),
        symbol: input.symbol.clone(),
        archive_date: input.archive_date.clone(),
        source_uri: input.source_uri.clone(),
        source_url: input.source_url.clone(),
        selected_object_sha256: input.selected_object_sha256.clone(),
        selected_object_bytes: input.selected_object_bytes,
        source_proof_id: input.source_proof_id.clone(),
        source_proof_version: input.source_proof_version,
        accepted_tranche_id: input.accepted_tranche_id.clone(),
        output_prefix: input.output_prefix.clone(),
        instrument_key: input.instrument_key.clone(),
        converter_identity: input.converter_identity.clone(),
        converter_version: input.converter_version.clone(),
        raw_payload_container: input.raw_payload_container,
        zip_member: input.zip_member.clone(),
        max_decoded_bytes: input.max_decoded_bytes,
        max_source_rows: input.max_source_rows,
        max_projected_row_groups: input.max_projected_row_groups,
        max_wall_seconds: input.max_wall_seconds,
    }
}

fn withheld_record(
    input: &SourceUniverseOperatorInputRecord,
) -> SourceUniverseConversionWorkOrderWithheldRecord {
    SourceUniverseConversionWorkOrderWithheldRecord {
        work_item_id: input.work_item_id.clone(),
        operator_run_id: input.operator_run_id.clone(),
        source_binding: input.source_binding.clone(),
        category: input.category.clone(),
        symbol: input.symbol.clone(),
        archive_date: input.archive_date.clone(),
        selected_object_bytes: input.selected_object_bytes,
        blocking_reasons: input.blocking_reasons.clone(),
    }
}
