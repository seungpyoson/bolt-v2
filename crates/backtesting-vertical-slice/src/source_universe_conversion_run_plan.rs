//! Bounded conversion run planning for source-universe object gates.
//!
//! This pre-payload artifact turns accepted object gates into deterministic
//! run-sized batches. It does not download payloads or write NT catalog data.

use std::{
    collections::{BTreeMap, BTreeSet},
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
use crate::source_universe_object_gates::{
    SourceUniverseObjectGateMaterialization, SourceUniverseObjectGateRecord,
    SourceUniverseObjectGateStatus,
};

pub const SOURCE_UNIVERSE_CONVERSION_RUN_PLAN_SCHEMA_VERSION: &str =
    "source-universe-conversion-run-plan.v1";
pub const SOURCE_UNIVERSE_CONVERSION_RUN_PLAN_FILE: &str =
    "source-universe-conversion-run-plan.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionRunPlanSpec {
    pub plan_id: String,
    pub source_universe_object_gates_path: PathBuf,
    pub output_dir: PathBuf,
    pub max_objects_per_run: u64,
    pub max_source_bytes_per_run: u64,
    #[serde(default)]
    pub overwrite_existing_artifacts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseConversionRunPlanStatus {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionRunCategorySummary {
    pub category: String,
    pub source_binding: String,
    pub run_count: u64,
    pub object_count: u64,
    pub source_bytes: u64,
    pub first_archive_date: String,
    pub last_archive_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionRun {
    pub run_id: String,
    pub run_index: u64,
    pub source_binding: String,
    pub table_family: String,
    pub category: String,
    pub first_archive_date: String,
    pub last_archive_date: String,
    pub object_count: u64,
    pub source_bytes: u64,
    pub work_item_ids: Vec<String>,
    pub accepted_tranche_ids: Vec<String>,
    pub output_prefixes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionRunPlan {
    pub schema_version: String,
    pub plan_id: String,
    pub status: SourceUniverseConversionRunPlanStatus,
    pub gate_id: String,
    pub queue_id: String,
    pub manifest_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub object_gates_path: PathBuf,
    pub object_gates_hash: String,
    pub max_objects_per_run: u64,
    pub max_source_bytes_per_run: u64,
    pub source_binding_count: u64,
    pub object_count: u64,
    pub planned_object_count: u64,
    pub total_source_bytes: u64,
    pub planned_source_bytes: u64,
    pub run_count: u64,
    pub category_summaries: Vec<SourceUniverseConversionRunCategorySummary>,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub runs: Vec<SourceUniverseConversionRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseConversionRunPlanArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub run_count: u64,
    pub object_count: u64,
}

#[derive(Default)]
struct RunBuilder {
    source_binding: String,
    table_family: String,
    category: String,
    first_archive_date: Option<String>,
    last_archive_date: Option<String>,
    source_bytes: u64,
    work_item_ids: Vec<String>,
    accepted_tranche_ids: Vec<String>,
    output_prefixes: Vec<String>,
}

impl RunBuilder {
    fn new(record: &SourceUniverseObjectGateRecord) -> Self {
        Self {
            source_binding: record.source_binding.clone(),
            table_family: record.table_family.clone(),
            category: record.category.clone(),
            ..Self::default()
        }
    }

    fn is_empty(&self) -> bool {
        self.work_item_ids.is_empty()
    }

    fn can_accept(
        &self,
        record: &SourceUniverseObjectGateRecord,
        max_objects_per_run: u64,
        max_source_bytes_per_run: u64,
    ) -> bool {
        if self.is_empty() {
            return true;
        }
        self.source_binding == record.source_binding
            && self.category == record.category
            && self.table_family == record.table_family
            && (self.work_item_ids.len() as u64) < max_objects_per_run
            && self
                .source_bytes
                .saturating_add(record.selected_object_bytes)
                <= max_source_bytes_per_run
    }

    fn push(&mut self, record: &SourceUniverseObjectGateRecord) {
        self.source_bytes = self
            .source_bytes
            .saturating_add(record.selected_object_bytes);
        match self.first_archive_date.as_ref() {
            Some(existing) if existing.as_str() <= record.archive_date.as_str() => {}
            _ => self.first_archive_date = Some(record.archive_date.clone()),
        }
        match self.last_archive_date.as_ref() {
            Some(existing) if existing.as_str() >= record.archive_date.as_str() => {}
            _ => self.last_archive_date = Some(record.archive_date.clone()),
        }
        self.work_item_ids.push(record.work_item_id.clone());
        self.accepted_tranche_ids
            .push(record.accepted_tranche_id.clone());
        self.output_prefixes.push(record.output_prefix.clone());
    }

    fn into_run(self, plan_id: &str, run_index: u64) -> SourceUniverseConversionRun {
        SourceUniverseConversionRun {
            run_id: format!("{plan_id}:run-{run_index:05}"),
            run_index,
            source_binding: self.source_binding,
            table_family: self.table_family,
            category: self.category,
            first_archive_date: self.first_archive_date.unwrap_or_default(),
            last_archive_date: self.last_archive_date.unwrap_or_default(),
            object_count: self.work_item_ids.len() as u64,
            source_bytes: self.source_bytes,
            work_item_ids: self.work_item_ids,
            accepted_tranche_ids: self.accepted_tranche_ids,
            output_prefixes: self.output_prefixes,
        }
    }
}

pub fn write_source_universe_conversion_run_plan_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseConversionRunPlanArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe conversion run-plan spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseConversionRunPlanSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source-universe conversion run-plan spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_conversion_run_plan(&spec, base_dir)
}

pub fn write_source_universe_conversion_run_plan(
    spec: &SourceUniverseConversionRunPlanSpec,
    base_dir: &Path,
) -> Result<SourceUniverseConversionRunPlanArtifact> {
    let plan = evaluate_source_universe_conversion_run_plan(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe conversion run-plan directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_UNIVERSE_CONVERSION_RUN_PLAN_FILE);
    let rewrite = if spec.overwrite_existing_artifacts {
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteIfChanged
    } else {
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
    };
    let written = crate::reference_artifact::write_reference_artifact_with_len_mapped(
        &path,
        SOURCE_UNIVERSE_CONVERSION_RUN_PLAN_FILE,
        &plan,
        rewrite,
        crate::reference_artifact::ReferenceArtifactErrorMappers {
            serialize_error: |error| {
                anyhow::anyhow!("serialize source-universe conversion run-plan: {error}")
            },
            read_existing_error: |path, error| {
                anyhow::anyhow!("read existing source-universe conversion run-plan {path}: {error}")
            },
            mismatch_error: |path| {
                anyhow::anyhow!(
                    "dirty source-universe conversion run-plan {path}: existing file content differs"
                )
            },
            write_error: |path, error| {
                anyhow::anyhow!("write source-universe conversion run-plan {path}: {error}")
            },
        },
    )?;

    Ok(SourceUniverseConversionRunPlanArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        run_count: plan.run_count,
        object_count: plan.object_count,
    })
}

pub fn evaluate_source_universe_conversion_run_plan(
    spec: &SourceUniverseConversionRunPlanSpec,
    base_dir: &Path,
) -> Result<SourceUniverseConversionRunPlan> {
    ensure!(!spec.plan_id.trim().is_empty(), "plan_id must not be empty");
    ensure!(
        spec.max_objects_per_run > 0,
        "max_objects_per_run must be positive"
    );
    ensure!(
        spec.max_source_bytes_per_run > 0,
        "max_source_bytes_per_run must be positive"
    );

    let object_gates_path =
        resolve_existing_path(base_dir, &spec.source_universe_object_gates_path)
            .canonicalize()
            .with_context(|| {
                format!(
                    "canonicalize source-universe object gates path {}",
                    spec.source_universe_object_gates_path.display()
                )
            })?;
    let object_gates_hash = sha256_file(&object_gates_path)?;
    let gates: SourceUniverseObjectGateMaterialization = read_json(&object_gates_path)?;
    let portable_object_gates_path = portable_artifact_path_for_spec(
        &object_gates_path,
        &spec.source_universe_object_gates_path,
    )?;
    ensure!(
        gates.status == SourceUniverseObjectGateStatus::Ready,
        "source-universe object gates {} are not ready",
        object_gates_path.display()
    );
    ensure!(
        gates.work_item_count == gates.accepted_gate_count,
        "source-universe object gates accepted count does not cover every work item"
    );
    ensure!(
        gates.records.len() as u64 == gates.accepted_gate_count,
        "source-universe object gate records do not match accepted count"
    );

    let mut current: Option<RunBuilder> = None;
    let mut runs = Vec::new();
    let mut seen_work_items = BTreeSet::new();
    for record in &gates.records {
        ensure!(
            record.gate_status == SourceUniverseObjectGateStatus::Ready,
            "source-universe object gate record {} is not ready",
            record.work_item_id
        );
        ensure!(
            record.selected_object_bytes <= spec.max_source_bytes_per_run,
            "source-universe object gate record {} exceeds max_source_bytes_per_run",
            record.work_item_id
        );
        ensure!(
            seen_work_items.insert(record.work_item_id.clone()),
            "duplicate source-universe work item {}",
            record.work_item_id
        );

        let needs_new_run = current.as_ref().is_none_or(|builder| {
            !builder.can_accept(
                record,
                spec.max_objects_per_run,
                spec.max_source_bytes_per_run,
            )
        });
        if needs_new_run && let Some(builder) = current.take() {
            runs.push(builder.into_run(&spec.plan_id, runs.len() as u64 + 1));
        }

        let builder = current.get_or_insert_with(|| RunBuilder::new(record));
        builder.push(record);
    }
    if let Some(builder) = current.take() {
        runs.push(builder.into_run(&spec.plan_id, runs.len() as u64 + 1));
    }

    let planned_object_count = runs.iter().map(|run| run.object_count).sum::<u64>();
    let planned_source_bytes = runs.iter().map(|run| run.source_bytes).sum::<u64>();
    ensure!(
        planned_object_count == gates.accepted_gate_count,
        "planned object count does not match accepted object gates"
    );
    ensure!(
        planned_source_bytes == gates.total_accepted_bytes,
        "planned source bytes do not match accepted object gates"
    );

    let source_binding_count = gates
        .records
        .iter()
        .map(|record| record.source_binding.as_str())
        .collect::<BTreeSet<_>>()
        .len() as u64;
    ensure!(
        source_binding_count == gates.source_binding_count,
        "planned source binding count does not match object gates"
    );

    Ok(SourceUniverseConversionRunPlan {
        schema_version: SOURCE_UNIVERSE_CONVERSION_RUN_PLAN_SCHEMA_VERSION.to_string(),
        plan_id: spec.plan_id.clone(),
        status: SourceUniverseConversionRunPlanStatus::Ready,
        gate_id: gates.gate_id,
        queue_id: gates.queue_id,
        manifest_id: gates.manifest_id,
        universe_id: gates.universe_id,
        venue: gates.venue,
        source: gates.source,
        family: gates.family,
        table_family: gates.table_family,
        object_gates_path: portable_object_gates_path.clone(),
        object_gates_hash: object_gates_hash.clone(),
        max_objects_per_run: spec.max_objects_per_run,
        max_source_bytes_per_run: spec.max_source_bytes_per_run,
        source_binding_count,
        object_count: gates.accepted_gate_count,
        planned_object_count,
        total_source_bytes: gates.total_accepted_bytes,
        planned_source_bytes,
        run_count: runs.len() as u64,
        category_summaries: category_summaries(&runs),
        artifact_refs: vec![ReferenceArtifactPin {
            role: "source_universe_object_gates".to_string(),
            path: portable_object_gates_path,
            sha256: object_gates_hash,
        }],
        runs,
    })
}

fn category_summaries(
    runs: &[SourceUniverseConversionRun],
) -> Vec<SourceUniverseConversionRunCategorySummary> {
    let mut summaries =
        BTreeMap::<(String, String), SourceUniverseConversionRunCategorySummary>::new();
    for run in runs {
        let key = (run.category.clone(), run.source_binding.clone());
        let summary =
            summaries
                .entry(key)
                .or_insert_with(|| SourceUniverseConversionRunCategorySummary {
                    category: run.category.clone(),
                    source_binding: run.source_binding.clone(),
                    run_count: 0,
                    object_count: 0,
                    source_bytes: 0,
                    first_archive_date: run.first_archive_date.clone(),
                    last_archive_date: run.last_archive_date.clone(),
                });
        summary.run_count += 1;
        summary.object_count += run.object_count;
        summary.source_bytes = summary.source_bytes.saturating_add(run.source_bytes);
        if summary.first_archive_date > run.first_archive_date {
            summary.first_archive_date = run.first_archive_date.clone();
        }
        if summary.last_archive_date < run.last_archive_date {
            summary.last_archive_date = run.last_archive_date.clone();
        }
    }
    summaries.into_values().collect()
}

fn read_json<T>(path: &Path) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = fs::read(path).with_context(|| format!("read JSON artifact {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse JSON artifact {}", path.display()))
}
fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read artifact {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}
