//! Conversion work queue for accepted source-universe object manifests.
//!
//! This artifact bridges source acceptance and payload conversion by turning
//! every source-universe object into a deterministic, resumable work item.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::hashing::sha256_hex;
use crate::path_resolution::{
    resolve_existing_path, resolve_output_dir, stable_artifact_identity_path_for_spec,
};
use crate::reference_artifact::ReferenceArtifactPin;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const SOURCE_UNIVERSE_CONVERSION_QUEUE_SCHEMA_VERSION: &str =
    "source-universe-conversion-queue.v1";
pub const SOURCE_UNIVERSE_CONVERSION_QUEUE_FILE: &str = "source-universe-conversion-queue.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionQueueSpec {
    pub queue_id: String,
    pub source_universe_manifest_path: PathBuf,
    #[serde(default)]
    pub source_universe_manifest_artifact_path: Option<PathBuf>,
    pub output_dir: PathBuf,
    #[serde(default)]
    pub table_family: Option<String>,
    pub output_prefix_template: String,
    #[serde(default)]
    pub overwrite_existing_artifacts: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseConversionQueueStatus {
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseConversionWorkState {
    PendingConversion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionCategorySummary {
    pub category: String,
    pub source_binding: String,
    pub instrument_count: u64,
    pub work_item_count: u64,
    pub source_bytes: u64,
    pub first_archive_date: String,
    pub last_archive_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionWorkItem {
    pub work_item_id: String,
    pub work_state: SourceUniverseConversionWorkState,
    pub source_binding: String,
    pub table_family: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub source_uri: String,
    pub source_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_hash_algorithm: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_sha256: String,
    pub source_bytes: u64,
    pub schema_columns: Vec<String>,
    pub output_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseConversionQueue {
    pub schema_version: String,
    pub queue_id: String,
    pub status: SourceUniverseConversionQueueStatus,
    pub manifest_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub source_manifest_path: PathBuf,
    pub source_manifest_hash: String,
    pub output_prefix_template: String,
    pub work_item_count: u64,
    pub pending_conversion_items: u64,
    pub total_source_bytes: u64,
    pub category_summaries: Vec<SourceUniverseConversionCategorySummary>,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub work_items: Vec<SourceUniverseConversionWorkItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseConversionQueueArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub work_item_count: u64,
}

#[derive(Debug, Deserialize)]
struct SourceUniverseManifest {
    manifest_id: String,
    universe_id: String,
    venue: String,
    source: String,
    family: String,
    table_family: String,
    object_count: u64,
    accepted_bytes: u64,
    #[serde(default)]
    category_summaries: Vec<SourceUniverseManifestCategorySummary>,
    #[serde(default)]
    payload_records: Vec<SourceUniverseManifestPayloadRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseManifestCategorySummary {
    category: String,
    source_binding: String,
    instrument_count: u64,
    object_count: u64,
    compressed_bytes: u64,
    first_archive_date: String,
    last_archive_date: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceUniverseManifestPayloadRecord {
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
    schema_columns: Vec<String>,
}

pub fn write_source_universe_conversion_queue_from_spec_file(
    spec_path: &Path,
) -> Result<SourceUniverseConversionQueueArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source-universe conversion queue spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceUniverseConversionQueueSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source-universe conversion queue spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_universe_conversion_queue(&spec, base_dir)
}

pub fn write_source_universe_conversion_queue(
    spec: &SourceUniverseConversionQueueSpec,
    base_dir: &Path,
) -> Result<SourceUniverseConversionQueueArtifact> {
    let queue = evaluate_source_universe_conversion_queue(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source-universe conversion queue output directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_UNIVERSE_CONVERSION_QUEUE_FILE);
    let rewrite = if spec.overwrite_existing_artifacts {
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteIfChanged
    } else {
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty
    };
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_UNIVERSE_CONVERSION_QUEUE_FILE,
        &queue,
        rewrite,
    )
    .with_context(|| format!("write source-universe conversion queue {}", path.display()))?;

    Ok(SourceUniverseConversionQueueArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        work_item_count: queue.work_item_count,
    })
}

pub fn evaluate_source_universe_conversion_queue(
    spec: &SourceUniverseConversionQueueSpec,
    base_dir: &Path,
) -> Result<SourceUniverseConversionQueue> {
    ensure!(
        !spec.queue_id.trim().is_empty(),
        "queue_id must not be empty"
    );
    ensure!(
        !spec.output_prefix_template.trim().is_empty(),
        "output_prefix_template must not be empty"
    );

    let source_manifest_path = resolve_existing_path(base_dir, &spec.source_universe_manifest_path);
    let source_manifest_hash = sha256_file(&source_manifest_path)?;
    let source_manifest_artifact_path = stable_artifact_identity_path_for_spec(
        &source_manifest_path,
        &spec.source_universe_manifest_path,
        spec.source_universe_manifest_artifact_path.as_deref(),
    )?;
    let manifest: SourceUniverseManifest = read_json(&source_manifest_path)?;
    ensure!(
        manifest.object_count as usize == manifest.payload_records.len(),
        "source-universe manifest object_count does not match payload_records length"
    );
    ensure!(
        !manifest.payload_records.is_empty(),
        "source-universe manifest must contain payload records"
    );
    let table_family = spec
        .table_family
        .as_deref()
        .unwrap_or(manifest.table_family.as_str())
        .trim()
        .to_string();
    ensure!(!table_family.is_empty(), "table_family must not be empty");

    let mut work_items = Vec::with_capacity(manifest.payload_records.len());
    for record in &manifest.payload_records {
        work_items.push(work_item(
            &manifest,
            &table_family,
            record,
            &spec.output_prefix_template,
        )?);
    }

    let total_source_bytes = work_items.iter().map(|item| item.source_bytes).sum::<u64>();
    ensure!(
        total_source_bytes == manifest.accepted_bytes,
        "source-universe manifest accepted_bytes does not match payload record bytes"
    );

    let category_summaries = category_summaries(&manifest, &work_items)?;

    Ok(SourceUniverseConversionQueue {
        schema_version: SOURCE_UNIVERSE_CONVERSION_QUEUE_SCHEMA_VERSION.to_string(),
        queue_id: spec.queue_id.clone(),
        status: SourceUniverseConversionQueueStatus::Ready,
        manifest_id: manifest.manifest_id,
        universe_id: manifest.universe_id,
        venue: manifest.venue,
        source: manifest.source,
        family: manifest.family,
        table_family,
        source_manifest_path: source_manifest_artifact_path.clone(),
        source_manifest_hash: source_manifest_hash.clone(),
        output_prefix_template: spec.output_prefix_template.clone(),
        work_item_count: work_items.len() as u64,
        pending_conversion_items: work_items.len() as u64,
        total_source_bytes,
        category_summaries,
        artifact_refs: vec![ReferenceArtifactPin {
            role: "source_universe_manifest".to_string(),
            path: source_manifest_artifact_path,
            sha256: source_manifest_hash.clone(),
        }],
        work_items,
    })
}

fn work_item(
    manifest: &SourceUniverseManifest,
    table_family: &str,
    record: &SourceUniverseManifestPayloadRecord,
    output_prefix_template: &str,
) -> Result<SourceUniverseConversionWorkItem> {
    let source_hash_algorithm = source_hash_algorithm(record)?;
    let source_hash = source_hash(record)?;
    let source_sha256 = source_sha256(record, &source_hash_algorithm, &source_hash)?;
    let output_prefix = render_output_prefix(
        manifest,
        table_family,
        record,
        &source_hash_algorithm,
        &source_hash,
        &source_sha256,
        output_prefix_template,
    )?;
    Ok(SourceUniverseConversionWorkItem {
        work_item_id: work_item_id(record, &source_hash),
        work_state: SourceUniverseConversionWorkState::PendingConversion,
        source_binding: record.source_binding.clone(),
        table_family: table_family.to_string(),
        category: record.category.clone(),
        symbol: record.symbol.clone(),
        archive_date: record.archive_date.clone(),
        source_uri: record.s3_uri.clone(),
        source_url: record.source_url.clone(),
        source_hash_algorithm,
        source_hash,
        source_sha256,
        source_bytes: record.bytes,
        schema_columns: record.schema_columns.clone(),
        output_prefix,
    })
}

fn work_item_id(record: &SourceUniverseManifestPayloadRecord, source_hash: &str) -> String {
    format!(
        "{}:{}:{}:{}",
        record.source_binding, record.symbol, record.archive_date, source_hash
    )
}

fn render_output_prefix(
    manifest: &SourceUniverseManifest,
    table_family: &str,
    record: &SourceUniverseManifestPayloadRecord,
    source_hash_algorithm: &str,
    source_hash: &str,
    source_sha256: &str,
    template: &str,
) -> Result<String> {
    let mut output = template.to_string();
    let source_hash_path = source_hash_path_component(source_hash_algorithm, source_hash);
    for (token, value) in [
        ("{manifest_id}", manifest.manifest_id.as_str()),
        ("{universe_id}", manifest.universe_id.as_str()),
        ("{venue}", manifest.venue.as_str()),
        ("{source}", manifest.source.as_str()),
        ("{family}", manifest.family.as_str()),
        ("{table_family}", table_family),
        ("{category}", record.category.as_str()),
        ("{symbol}", record.symbol.as_str()),
        ("{archive_date}", record.archive_date.as_str()),
        ("{sha256}", source_sha256),
        ("{source_hash_algorithm}", source_hash_algorithm),
        ("{source_hash}", source_hash_path.as_str()),
        ("{source_hash_raw}", source_hash),
        ("{source_binding}", record.source_binding.as_str()),
    ] {
        output = output.replace(token, value);
    }
    ensure!(
        !output.contains('{') && !output.contains('}'),
        "output_prefix_template contains an unsupported placeholder"
    );
    Ok(output)
}

fn source_hash_algorithm(record: &SourceUniverseManifestPayloadRecord) -> Result<String> {
    if !record.source_hash.trim().is_empty() {
        ensure!(
            !record.source_hash_algorithm.trim().is_empty(),
            "source_hash_algorithm must be set when source_hash is set"
        );
        return Ok(record.source_hash_algorithm.clone());
    }
    ensure!(
        !record.sha256.trim().is_empty(),
        "payload record must include sha256 or source_hash"
    );
    Ok("sha256".to_string())
}

fn source_hash(record: &SourceUniverseManifestPayloadRecord) -> Result<String> {
    if !record.source_hash.trim().is_empty() {
        return Ok(record.source_hash.clone());
    }
    ensure!(
        !record.sha256.trim().is_empty(),
        "payload record must include sha256 or source_hash"
    );
    Ok(record.sha256.clone())
}

fn source_sha256(
    record: &SourceUniverseManifestPayloadRecord,
    source_hash_algorithm: &str,
    source_hash: &str,
) -> Result<String> {
    if !record.sha256.trim().is_empty() {
        if source_hash_algorithm == "sha256" {
            ensure!(
                record.sha256 == source_hash,
                "sha256 must match source_hash when source_hash_algorithm is sha256"
            );
        }
        return Ok(record.sha256.clone());
    }
    if source_hash_algorithm == "sha256" {
        return Ok(source_hash.to_string());
    }
    Ok(String::new())
}

fn source_hash_path_component(source_hash_algorithm: &str, source_hash: &str) -> String {
    let trimmed = source_hash.trim_matches('"');
    let value = if source_hash_algorithm.contains("etag") {
        format!("etag-{trimmed}")
    } else {
        trimmed.to_string()
    };
    let mut path = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            path.push(character);
        } else {
            path.push('-');
        }
    }
    path
}

fn category_summaries(
    manifest: &SourceUniverseManifest,
    work_items: &[SourceUniverseConversionWorkItem],
) -> Result<Vec<SourceUniverseConversionCategorySummary>> {
    let mut counts_by_category: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for item in work_items {
        let entry = counts_by_category
            .entry(item.category.as_str())
            .or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(item.source_bytes);
    }

    let mut summaries = Vec::with_capacity(manifest.category_summaries.len());
    for summary in &manifest.category_summaries {
        let (work_item_count, source_bytes) = counts_by_category
            .get(summary.category.as_str())
            .copied()
            .unwrap_or((0, 0));
        ensure!(
            work_item_count == summary.object_count,
            "category summary object_count does not match work item count"
        );
        ensure!(
            source_bytes == summary.compressed_bytes,
            "category summary compressed_bytes does not match work item bytes"
        );
        summaries.push(SourceUniverseConversionCategorySummary {
            category: summary.category.clone(),
            source_binding: summary.source_binding.clone(),
            instrument_count: summary.instrument_count,
            work_item_count,
            source_bytes,
            first_archive_date: summary.first_archive_date.clone(),
            last_archive_date: summary.last_archive_date.clone(),
        });
    }
    Ok(summaries)
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
