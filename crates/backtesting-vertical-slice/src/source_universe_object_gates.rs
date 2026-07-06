//! Object-gate materialization for source-universe conversion queues.
//!
//! This artifact binds each queued source object to the accepted source proof
//! and category manifest evidence needed before a converter may consume it.

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
use crate::{
    source_proof::{SourceBindingRegistry, SourceProofReport},
    source_universe_conversion_queue::{
        SourceUniverseConversionQueue, SourceUniverseConversionQueueStatus,
        SourceUniverseConversionWorkItem,
    },
};

pub const SOURCE_UNIVERSE_OBJECT_GATES_SCHEMA_VERSION: &str = "source-universe-object-gates.v1";
pub const SOURCE_UNIVERSE_OBJECT_GATES_FILE: &str = "source-universe-object-gates.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseObjectGateMaterializationSpec {
    pub gate_id: String,
    pub queue_path: PathBuf,
    pub output_dir: PathBuf,
    pub source_bindings_path: PathBuf,
    #[serde(default)]
    pub overwrite_existing_artifacts: bool,
    #[serde(rename = "source_binding", default)]
    pub source_bindings: Vec<SourceUniverseObjectGateSourceBindingSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseObjectGateSourceBindingSpec {
    pub source_binding: String,
    pub source_proof_path: PathBuf,
    pub category_manifest_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseObjectGateStatus {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseObjectGateSourceBindingSummary {
    pub source_binding: String,
    pub category_manifest_id: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub work_item_count: u64,
    pub accepted_bytes: u64,
    pub first_archive_date: String,
    pub last_archive_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseObjectGateRecord {
    pub work_item_id: String,
    pub gate_status: SourceUniverseObjectGateStatus,
    pub source_binding: String,
    pub table_family: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub source_uri: String,
    pub source_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_object_hash_algorithm: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_object_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub selected_object_sha256: String,
    pub selected_object_bytes: u64,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub source_proof_hash: String,
    pub category_manifest_id: String,
    pub category_manifest_hash: String,
    pub source_proof_scope_report_id: String,
    pub accepted_tranche_id: String,
    pub output_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseObjectGateMaterialization {
    pub schema_version: String,
    pub gate_id: String,
    pub status: SourceUniverseObjectGateStatus,
    pub queue_id: String,
    pub manifest_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub queue_path: PathBuf,
    pub queue_hash: String,
    pub work_item_count: u64,
    pub accepted_gate_count: u64,
    pub source_binding_count: u64,
    pub total_accepted_bytes: u64,
    pub source_binding_summaries: Vec<SourceUniverseObjectGateSourceBindingSummary>,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub records: Vec<SourceUniverseObjectGateRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseObjectGateMaterializationArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub work_item_count: u64,
}

#[derive(Debug, Deserialize)]
struct CategoryObjectManifest {
    manifest_id: String,
    source_binding: String,
    object_count: u64,
    accepted_bytes: u64,
    #[serde(default)]
    payload_records: Vec<CategoryObjectManifestRecord>,
}

#[derive(Debug, Clone, Deserialize)]
struct CategoryObjectManifestRecord {
    s3_uri: String,
    source_url: String,
    #[serde(default)]
    source_hash_algorithm: String,
    #[serde(default)]
    source_hash: String,
    #[serde(default)]
    sha256: String,
    bytes: u64,
    archive_date: String,
    category: String,
    symbol: String,
    source_binding: String,
}

struct BindingContext {
    source_binding: String,
    source_proof: SourceProofReport,
    source_proof_hash: String,
    source_proof_path: PathBuf,
    source_proof_artifact_path: PathBuf,
    category_manifest: CategoryObjectManifest,
    category_manifest_hash: String,
    category_manifest_path: PathBuf,
    category_manifest_artifact_path: PathBuf,
    records_by_uri: BTreeMap<String, CategoryObjectManifestRecord>,
}

#[derive(Default)]
struct BindingAccumulator {
    source_binding: String,
    category_manifest_id: String,
    source_proof_id: String,
    source_proof_version: u32,
    work_item_count: u64,
    accepted_bytes: u64,
    first_archive_date: Option<String>,
    last_archive_date: Option<String>,
}

impl BindingAccumulator {
    fn observe(&mut self, archive_date: &str, bytes: u64) {
        self.work_item_count += 1;
        self.accepted_bytes += bytes;
        match self.first_archive_date.as_ref() {
            Some(existing) if existing.as_str() <= archive_date => {}
            _ => self.first_archive_date = Some(archive_date.to_string()),
        }
        match self.last_archive_date.as_ref() {
            Some(existing) if existing.as_str() >= archive_date => {}
            _ => self.last_archive_date = Some(archive_date.to_string()),
        }
    }

    fn into_summary(self) -> SourceUniverseObjectGateSourceBindingSummary {
        SourceUniverseObjectGateSourceBindingSummary {
            source_binding: self.source_binding,
            category_manifest_id: self.category_manifest_id,
            source_proof_id: self.source_proof_id,
            source_proof_version: self.source_proof_version,
            work_item_count: self.work_item_count,
            accepted_bytes: self.accepted_bytes,
            first_archive_date: self.first_archive_date.unwrap_or_default(),
            last_archive_date: self.last_archive_date.unwrap_or_default(),
        }
    }
}

pub fn write_source_universe_object_gate_materialization_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseObjectGateMaterializationArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe object-gate spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseObjectGateMaterializationSpec = toml::from_slice(&spec_bytes)
        .with_context(|| {
            format!(
                "parse source-universe object-gate spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_object_gate_materialization(&spec, base_dir)
}

pub fn write_source_universe_object_gate_materialization(
    spec: &SourceUniverseObjectGateMaterializationSpec,
    base_dir: &Path,
) -> Result<SourceUniverseObjectGateMaterializationArtifact> {
    let materialization = evaluate_source_universe_object_gate_materialization(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe object-gate materialization directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_UNIVERSE_OBJECT_GATES_FILE);
    let rewrite = if spec.overwrite_existing_artifacts {
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteIfChanged
    } else {
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
    };
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_UNIVERSE_OBJECT_GATES_FILE,
        &materialization,
        rewrite,
    )
    .with_context(|| {
        format!(
            "write source-universe object-gate materialization {}",
            path.display()
        )
    })?;

    Ok(SourceUniverseObjectGateMaterializationArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        work_item_count: materialization.work_item_count,
    })
}

pub fn evaluate_source_universe_object_gate_materialization(
    spec: &SourceUniverseObjectGateMaterializationSpec,
    base_dir: &Path,
) -> Result<SourceUniverseObjectGateMaterialization> {
    ensure!(!spec.gate_id.trim().is_empty(), "gate_id must not be empty");
    ensure!(
        !spec.source_bindings.is_empty(),
        "source_binding set must not be empty"
    );

    let queue_path = resolve_existing_path(base_dir, &spec.queue_path);
    let queue_hash = sha256_file(&queue_path)?;
    let queue: SourceUniverseConversionQueue = read_json(&queue_path)?;
    ensure!(
        queue.status == SourceUniverseConversionQueueStatus::Ready,
        "source-universe conversion queue is not ready"
    );
    ensure!(
        queue.work_item_count as usize == queue.work_items.len(),
        "source-universe conversion queue work_item_count does not match records"
    );

    let registry =
        crate::source_proof::read_source_binding_registry_from_path(&spec.source_bindings_path)
            .with_context(|| {
                format!(
                    "read source-binding registry {}",
                    spec.source_bindings_path.display()
                )
            })?;
    let contexts = binding_contexts(base_dir, &spec.source_bindings, &registry)?;
    let queue_artifact_path = portable_artifact_path_for_spec(&queue_path, &spec.queue_path)?;
    let mut artifact_refs = vec![artifact_ref(
        "source_universe_conversion_queue",
        &queue_path,
        queue_artifact_path.clone(),
    )?];
    for context in contexts.values() {
        artifact_refs.push(artifact_ref(
            "source_proof",
            &context.source_proof_path,
            context.source_proof_artifact_path.clone(),
        )?);
        artifact_refs.push(artifact_ref(
            "category_manifest",
            &context.category_manifest_path,
            context.category_manifest_artifact_path.clone(),
        )?);
    }

    let mut records = Vec::with_capacity(queue.work_items.len());
    let mut accumulators = contexts
        .iter()
        .map(|(source_binding, context)| {
            (
                source_binding.clone(),
                BindingAccumulator {
                    source_binding: source_binding.clone(),
                    category_manifest_id: context.category_manifest.manifest_id.clone(),
                    source_proof_id: context.source_proof.source_proof_id.clone(),
                    source_proof_version: context.source_proof.source_proof_version,
                    ..BindingAccumulator::default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();

    for item in &queue.work_items {
        let context = contexts.get(&item.source_binding).with_context(|| {
            format!(
                "missing source proof/category manifest binding for {}",
                item.source_binding
            )
        })?;
        let object = context
            .records_by_uri
            .get(&item.source_uri)
            .with_context(|| format!("missing category manifest object {}", item.source_uri))?;
        validate_item_object_match(item, object)?;
        let accumulator = accumulators
            .get_mut(&item.source_binding)
            .expect("context and accumulator keys match");
        accumulator.observe(&item.archive_date, item.source_bytes);
        records.push(object_gate_record(spec, item, context)?);
    }

    let source_binding_summaries = accumulators
        .into_values()
        .map(BindingAccumulator::into_summary)
        .collect::<Vec<_>>();
    validate_context_coverage(&contexts, &source_binding_summaries)?;

    Ok(SourceUniverseObjectGateMaterialization {
        schema_version: SOURCE_UNIVERSE_OBJECT_GATES_SCHEMA_VERSION.to_string(),
        gate_id: spec.gate_id.clone(),
        status: SourceUniverseObjectGateStatus::Ready,
        queue_id: queue.queue_id,
        manifest_id: queue.manifest_id,
        universe_id: queue.universe_id,
        venue: queue.venue,
        source: queue.source,
        family: queue.family,
        table_family: queue.table_family,
        queue_path: queue_artifact_path,
        queue_hash,
        work_item_count: records.len() as u64,
        accepted_gate_count: records.len() as u64,
        source_binding_count: source_binding_summaries.len() as u64,
        total_accepted_bytes: records
            .iter()
            .map(|record| record.selected_object_bytes)
            .sum(),
        source_binding_summaries,
        artifact_refs,
        records,
    })
}

fn binding_contexts(
    base_dir: &Path,
    specs: &[SourceUniverseObjectGateSourceBindingSpec],
    registry: &SourceBindingRegistry,
) -> Result<BTreeMap<String, BindingContext>> {
    let mut seen = BTreeSet::new();
    let mut contexts = BTreeMap::new();
    for spec in specs {
        ensure!(
            !spec.source_binding.trim().is_empty(),
            "source_binding must not be empty"
        );
        ensure!(
            seen.insert(spec.source_binding.clone()),
            "duplicate source_binding {}",
            spec.source_binding
        );
        let source_proof_path = resolve_existing_path(base_dir, &spec.source_proof_path);
        let category_manifest_path = resolve_existing_path(base_dir, &spec.category_manifest_path);
        let source_proof_artifact_path =
            portable_artifact_path_for_spec(&source_proof_path, &spec.source_proof_path)?;
        let category_manifest_artifact_path =
            portable_artifact_path_for_spec(&category_manifest_path, &spec.category_manifest_path)?;
        let source_proof_hash = sha256_file(&source_proof_path)?;
        let category_manifest_hash = sha256_file(&category_manifest_path)?;
        let source_proof: SourceProofReport = read_json(&source_proof_path)?;
        ensure!(
            source_proof.source_binding == spec.source_binding,
            "source proof binding {:?} does not match spec {:?}",
            source_proof.source_binding,
            spec.source_binding
        );
        source_proof
            .evaluate_acceptance_with_registry(registry)
            .with_context(|| {
                format!(
                    "source proof {} is not accepted",
                    source_proof.source_proof_id
                )
            })?;
        let category_manifest: CategoryObjectManifest = read_json(&category_manifest_path)?;
        ensure!(
            category_manifest.source_binding == spec.source_binding,
            "category manifest binding {:?} does not match spec {:?}",
            category_manifest.source_binding,
            spec.source_binding
        );
        validate_manifest_acceptance_scope(&source_proof, &category_manifest)?;
        let records_by_uri = category_manifest
            .payload_records
            .iter()
            .map(|record| (record.s3_uri.clone(), record.clone()))
            .collect::<BTreeMap<_, _>>();
        ensure!(
            records_by_uri.len() == category_manifest.payload_records.len(),
            "category manifest {} has duplicate object URIs",
            category_manifest.manifest_id
        );
        contexts.insert(
            spec.source_binding.clone(),
            BindingContext {
                source_binding: spec.source_binding.clone(),
                source_proof,
                source_proof_hash,
                source_proof_path,
                source_proof_artifact_path,
                category_manifest,
                category_manifest_hash,
                category_manifest_path,
                category_manifest_artifact_path,
                records_by_uri,
            },
        );
    }
    Ok(contexts)
}

fn validate_manifest_acceptance_scope(
    proof: &SourceProofReport,
    manifest: &CategoryObjectManifest,
) -> Result<()> {
    ensure!(
        manifest.object_count as usize == manifest.payload_records.len(),
        "category manifest object_count does not match payload records"
    );
    let manifest_bytes = manifest
        .payload_records
        .iter()
        .map(|record| record.bytes)
        .sum::<u64>();
    ensure!(
        manifest_bytes == manifest.accepted_bytes,
        "category manifest accepted_bytes does not match payload record bytes"
    );
    let acceptance_scope = proof.acceptance_scope.as_ref().with_context(|| {
        format!(
            "source proof {} missing acceptance scope",
            proof.source_proof_id
        )
    })?;
    ensure!(
        acceptance_scope.completed_objects == manifest.object_count,
        "source proof {} completed_objects does not match category manifest object_count",
        proof.source_proof_id
    );
    ensure!(
        acceptance_scope.accepted_bytes == manifest.accepted_bytes,
        "source proof {} accepted_bytes does not match category manifest bytes",
        proof.source_proof_id
    );
    ensure_raw_sample_is_in_manifest(proof, manifest)?;
    Ok(())
}

fn ensure_raw_sample_is_in_manifest(
    proof: &SourceProofReport,
    manifest: &CategoryObjectManifest,
) -> Result<()> {
    for record in &manifest.payload_records {
        let object_hash_algorithm = object_hash_algorithm(record)?;
        let object_hash = object_hash(record)?;
        let object_sha256 = object_sha256(record, &object_hash_algorithm, &object_hash)?;
        if record.s3_uri == proof.raw_sample_uri
            && (object_hash == proof.raw_sample_hash
                || (!object_sha256.is_empty() && object_sha256 == proof.raw_sample_hash))
        {
            return Ok(());
        }
    }
    ensure!(
        false,
        "source proof {} raw sample is absent from category manifest {}",
        proof.source_proof_id,
        manifest.manifest_id
    );
    Ok(())
}

fn validate_item_object_match(
    item: &SourceUniverseConversionWorkItem,
    object: &CategoryObjectManifestRecord,
) -> Result<()> {
    let item_hash_algorithm = item_hash_algorithm(item)?;
    let item_hash = item_hash(item)?;
    let item_sha256 = item_sha256(item, &item_hash_algorithm, &item_hash)?;
    let object_hash_algorithm = object_hash_algorithm(object)?;
    let object_hash = object_hash(object)?;
    let object_sha256 = object_sha256(object, &object_hash_algorithm, &object_hash)?;
    ensure!(
        item.source_url == object.source_url,
        "queue source_url does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item_hash_algorithm == object_hash_algorithm,
        "queue source_hash_algorithm does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item_hash == object_hash,
        "queue source_hash does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item_sha256 == object_sha256,
        "queue sha256 does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item.source_bytes == object.bytes,
        "queue bytes do not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item.archive_date == object.archive_date,
        "queue archive_date does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item.category == object.category,
        "queue category does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item.symbol == object.symbol,
        "queue symbol does not match category manifest for {}",
        item.work_item_id
    );
    ensure!(
        item.source_binding == object.source_binding,
        "queue source_binding does not match category manifest for {}",
        item.work_item_id
    );
    Ok(())
}

fn item_hash_algorithm(item: &SourceUniverseConversionWorkItem) -> Result<String> {
    if !item.source_hash.trim().is_empty() {
        ensure!(
            !item.source_hash_algorithm.trim().is_empty(),
            "queue source_hash_algorithm must be set when source_hash is set"
        );
        return Ok(item.source_hash_algorithm.clone());
    }
    ensure!(
        !item.source_sha256.trim().is_empty(),
        "queue work item must include source_sha256 or source_hash"
    );
    Ok("sha256".to_string())
}

fn item_hash(item: &SourceUniverseConversionWorkItem) -> Result<String> {
    if !item.source_hash.trim().is_empty() {
        return Ok(item.source_hash.clone());
    }
    ensure!(
        !item.source_sha256.trim().is_empty(),
        "queue work item must include source_sha256 or source_hash"
    );
    Ok(item.source_sha256.clone())
}

fn item_sha256(
    item: &SourceUniverseConversionWorkItem,
    source_hash_algorithm: &str,
    source_hash: &str,
) -> Result<String> {
    if !item.source_sha256.trim().is_empty() {
        if source_hash_algorithm == "sha256" {
            ensure!(
                item.source_sha256 == source_hash,
                "queue source_sha256 must match source_hash when source_hash_algorithm is sha256"
            );
        }
        return Ok(item.source_sha256.clone());
    }
    if source_hash_algorithm == "sha256" {
        return Ok(source_hash.to_string());
    }
    Ok(String::new())
}

fn object_hash_algorithm(object: &CategoryObjectManifestRecord) -> Result<String> {
    if !object.source_hash.trim().is_empty() {
        ensure!(
            !object.source_hash_algorithm.trim().is_empty(),
            "category manifest source_hash_algorithm must be set when source_hash is set"
        );
        return Ok(object.source_hash_algorithm.clone());
    }
    ensure!(
        !object.sha256.trim().is_empty(),
        "category manifest object must include sha256 or source_hash"
    );
    Ok("sha256".to_string())
}

fn object_hash(object: &CategoryObjectManifestRecord) -> Result<String> {
    if !object.source_hash.trim().is_empty() {
        return Ok(object.source_hash.clone());
    }
    ensure!(
        !object.sha256.trim().is_empty(),
        "category manifest object must include sha256 or source_hash"
    );
    Ok(object.sha256.clone())
}

fn object_sha256(
    object: &CategoryObjectManifestRecord,
    source_hash_algorithm: &str,
    source_hash: &str,
) -> Result<String> {
    if !object.sha256.trim().is_empty() {
        if source_hash_algorithm == "sha256" {
            ensure!(
                object.sha256 == source_hash,
                "category manifest sha256 must match source_hash when source_hash_algorithm is sha256"
            );
        }
        return Ok(object.sha256.clone());
    }
    if source_hash_algorithm == "sha256" {
        return Ok(source_hash.to_string());
    }
    Ok(String::new())
}

fn validate_context_coverage(
    contexts: &BTreeMap<String, BindingContext>,
    summaries: &[SourceUniverseObjectGateSourceBindingSummary],
) -> Result<()> {
    for summary in summaries {
        let context = contexts
            .get(&summary.source_binding)
            .expect("summary source binding exists");
        ensure!(
            context.source_binding == summary.source_binding,
            "binding context key drifted for {}",
            summary.source_binding
        );
        ensure!(
            summary.work_item_count == context.category_manifest.object_count,
            "queue work item count for {} does not match category manifest object_count",
            summary.source_binding
        );
        ensure!(
            summary.accepted_bytes == context.category_manifest.accepted_bytes,
            "queue accepted bytes for {} do not match category manifest accepted_bytes",
            summary.source_binding
        );
    }
    Ok(())
}

fn object_gate_record(
    spec: &SourceUniverseObjectGateMaterializationSpec,
    item: &SourceUniverseConversionWorkItem,
    context: &BindingContext,
) -> Result<SourceUniverseObjectGateRecord> {
    let selected_object_hash_algorithm = item_hash_algorithm(item)?;
    let selected_object_hash = item_hash(item)?;
    let selected_object_sha256 =
        item_sha256(item, &selected_object_hash_algorithm, &selected_object_hash)?;
    Ok(SourceUniverseObjectGateRecord {
        work_item_id: item.work_item_id.clone(),
        gate_status: SourceUniverseObjectGateStatus::Ready,
        source_binding: item.source_binding.clone(),
        table_family: item.table_family.clone(),
        category: item.category.clone(),
        symbol: item.symbol.clone(),
        archive_date: item.archive_date.clone(),
        source_uri: item.source_uri.clone(),
        source_url: item.source_url.clone(),
        selected_object_hash_algorithm,
        selected_object_hash,
        selected_object_sha256,
        selected_object_bytes: item.source_bytes,
        source_proof_id: context.source_proof.source_proof_id.clone(),
        source_proof_version: context.source_proof.source_proof_version,
        source_proof_hash: context.source_proof_hash.clone(),
        category_manifest_id: context.category_manifest.manifest_id.clone(),
        category_manifest_hash: context.category_manifest_hash.clone(),
        source_proof_scope_report_id: format!(
            "{}:{}:source-proof-scope",
            spec.gate_id, item.work_item_id
        ),
        accepted_tranche_id: format!("{}:{}:accepted-tranche", spec.gate_id, item.work_item_id),
        output_prefix: item.output_prefix.clone(),
    })
}

fn artifact_ref(role: &str, path: &Path, artifact_path: PathBuf) -> Result<ReferenceArtifactPin> {
    Ok(ReferenceArtifactPin {
        role: role.to_string(),
        path: artifact_path,
        sha256: sha256_file(path)?,
    })
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
