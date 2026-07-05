//! Source-universe manifests derived from verified archive index manifests.
//!
//! This promotes a verified public archive index into the source-universe shape
//! consumed by conversion queues without inventing payload SHA-256s when the
//! archive only exposes object ETags.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::hashing::sha256_hex;
use crate::path_resolution::{resolve_existing_path, resolve_output_dir};
use crate::reference_artifact::ReferenceArtifactPin;
use crate::source_archive_index_manifest::{
    SourceArchiveIndexManifest, SourceArchiveIndexManifestStatus,
};

pub const SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_SCHEMA_VERSION: &str =
    "backfill-source-universe-object-manifest.v1";
pub const SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_FILE: &str = "source-universe-object-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexSourceUniverseSpec {
    pub manifest_id: String,
    pub universe_id: String,
    pub source_archive_index_manifest_path: PathBuf,
    pub output_dir: PathBuf,
    #[serde(default)]
    pub category_manifest_path: Option<PathBuf>,
    pub staging_uri_template: String,
    pub category: String,
    pub symbol: String,
    pub source_binding: String,
    pub source_hash_algorithm: String,
    pub schema_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexSourceUniverseCategorySummary {
    pub category: String,
    pub source_binding: String,
    pub instrument_count: u64,
    pub object_count: u64,
    pub compressed_bytes: u64,
    pub first_archive_date: String,
    pub last_archive_date: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexSourceUniversePayloadRecord {
    pub s3_uri: String,
    pub source_url: String,
    pub source_hash_algorithm: String,
    pub source_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub bytes: u64,
    pub archive_date: String,
    pub category: String,
    pub symbol: String,
    pub source_binding: String,
    pub schema_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexSourceUniverseManifest {
    pub schema_version: String,
    pub manifest_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub source_archive_index_manifest_id: String,
    pub source_archive_index_snapshot_id: String,
    pub source_hash_algorithm: String,
    pub staging_uri_template: String,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub category_summaries: Vec<SourceArchiveIndexSourceUniverseCategorySummary>,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub payload_records: Vec<SourceArchiveIndexSourceUniversePayloadRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexSourceUniverseCategoryManifest {
    pub schema_version: String,
    pub manifest_id: String,
    pub parent_manifest_id: String,
    pub universe_id: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub category: String,
    pub source_binding: String,
    pub object_count: u64,
    pub accepted_bytes: u64,
    pub first_archive_date: String,
    pub last_archive_date: String,
    pub payload_records: Vec<SourceArchiveIndexSourceUniversePayloadRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceArchiveIndexSourceUniverseArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub object_count: u64,
    pub accepted_bytes: u64,
}

pub fn write_source_archive_index_source_universe_manifest_from_spec_file(
    spec_path: &Path,
) -> Result<SourceArchiveIndexSourceUniverseArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source archive index source-universe spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceArchiveIndexSourceUniverseSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source archive index source-universe spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_archive_index_source_universe_manifest(&spec, base_dir)
}

pub fn write_source_archive_index_source_universe_manifest(
    spec: &SourceArchiveIndexSourceUniverseSpec,
    base_dir: &Path,
) -> Result<SourceArchiveIndexSourceUniverseArtifact> {
    let manifest = evaluate_source_archive_index_source_universe_manifest(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source archive index source-universe directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_FILE,
        &manifest,
    )
    .with_context(|| {
        format!(
            "write source archive index source-universe manifest {}",
            path.display()
        )
    })?;
    if let Some(category_manifest_path) = spec.category_manifest_path.as_ref() {
        let category_path = resolve_output_dir(base_dir, category_manifest_path);
        let category_manifest = category_manifest(&manifest);
        if let Some(parent) = category_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "create source archive index category manifest directory {}",
                    parent.display()
                )
            })?;
        }
        crate::reference_artifact::write_reference_artifact_with_len(
            &category_path,
            SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_SCHEMA_VERSION,
            &category_manifest,
        )
        .with_context(|| {
            format!(
                "write source archive index category manifest {}",
                category_path.display()
            )
        })?;
    }

    Ok(SourceArchiveIndexSourceUniverseArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        object_count: manifest.object_count,
        accepted_bytes: manifest.accepted_bytes,
    })
}

fn category_manifest(
    manifest: &SourceArchiveIndexSourceUniverseManifest,
) -> SourceArchiveIndexSourceUniverseCategoryManifest {
    let summary = manifest
        .category_summaries
        .first()
        .expect("source archive index source-universe has one category summary");
    SourceArchiveIndexSourceUniverseCategoryManifest {
        schema_version: SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_SCHEMA_VERSION.to_string(),
        manifest_id: format!("{}-category-{}", manifest.manifest_id, summary.category),
        parent_manifest_id: manifest.manifest_id.clone(),
        universe_id: manifest.universe_id.clone(),
        venue: manifest.venue.clone(),
        source: manifest.source.clone(),
        family: manifest.family.clone(),
        table_family: manifest.table_family.clone(),
        category: summary.category.clone(),
        source_binding: summary.source_binding.clone(),
        object_count: summary.object_count,
        accepted_bytes: summary.compressed_bytes,
        first_archive_date: summary.first_archive_date.clone(),
        last_archive_date: summary.last_archive_date.clone(),
        payload_records: manifest.payload_records.clone(),
    }
}

pub fn evaluate_source_archive_index_source_universe_manifest(
    spec: &SourceArchiveIndexSourceUniverseSpec,
    base_dir: &Path,
) -> Result<SourceArchiveIndexSourceUniverseManifest> {
    validate_spec(spec)?;

    let index_path = resolve_existing_path(base_dir, &spec.source_archive_index_manifest_path);
    let index_hash = sha256_file(&index_path)?;
    let index: SourceArchiveIndexManifest = read_json(&index_path)?;
    ensure!(
        index.status == SourceArchiveIndexManifestStatus::Ready,
        "source archive index manifest {} is not ready",
        index_path.display()
    );
    ensure!(
        index.object_count == index.verified_head_count,
        "source archive index manifest {} verified head count does not cover every object",
        index_path.display()
    );
    ensure!(
        index.object_count as usize == index.records.len(),
        "source archive index manifest {} object_count does not match records",
        index_path.display()
    );

    let mut accepted_bytes = 0_u64;
    let mut payload_records = Vec::with_capacity(index.records.len());
    for record in &index.records {
        let source_hash = source_hash(&spec.source_hash_algorithm, record.etag.as_deref())?;
        accepted_bytes = accepted_bytes.saturating_add(record.content_length_bytes);
        payload_records.push(SourceArchiveIndexSourceUniversePayloadRecord {
            s3_uri: render_staging_uri(
                spec,
                &index,
                &record.archive_hour_utc,
                &source_hash,
                &spec.staging_uri_template,
            )?,
            source_url: record.source_url.clone(),
            source_hash_algorithm: spec.source_hash_algorithm.clone(),
            source_hash,
            sha256: None,
            bytes: record.content_length_bytes,
            archive_date: record.archive_hour_utc.clone(),
            category: spec.category.clone(),
            symbol: spec.symbol.clone(),
            source_binding: spec.source_binding.clone(),
            schema_columns: spec.schema_columns.clone(),
        });
    }
    ensure!(
        accepted_bytes == index.total_content_length_bytes,
        "source archive index manifest {} total bytes do not match records",
        index_path.display()
    );

    Ok(SourceArchiveIndexSourceUniverseManifest {
        schema_version: SOURCE_ARCHIVE_INDEX_SOURCE_UNIVERSE_SCHEMA_VERSION.to_string(),
        manifest_id: spec.manifest_id.clone(),
        universe_id: spec.universe_id.clone(),
        venue: index.venue,
        source: index.source,
        family: index.family,
        table_family: index.table_family,
        source_archive_index_manifest_id: index.manifest_id,
        source_archive_index_snapshot_id: index.snapshot_id,
        source_hash_algorithm: spec.source_hash_algorithm.clone(),
        staging_uri_template: spec.staging_uri_template.clone(),
        object_count: payload_records.len() as u64,
        accepted_bytes,
        category_summaries: vec![SourceArchiveIndexSourceUniverseCategorySummary {
            category: spec.category.clone(),
            source_binding: spec.source_binding.clone(),
            instrument_count: 1,
            object_count: payload_records.len() as u64,
            compressed_bytes: accepted_bytes,
            first_archive_date: index.first_archive_hour_utc,
            last_archive_date: index.last_archive_hour_utc,
        }],
        // Committed manifests must be reproducible from any checkout: echo the
        // spec-authored path verbatim; resolution stays a read-time concern.
        artifact_refs: vec![ReferenceArtifactPin {
            role: "source_archive_index_manifest".to_string(),
            path: spec.source_archive_index_manifest_path.clone(),
            sha256: index_hash,
        }],
        payload_records,
    })
}

fn validate_spec(spec: &SourceArchiveIndexSourceUniverseSpec) -> Result<()> {
    for (field, value) in [
        ("manifest_id", spec.manifest_id.as_str()),
        ("universe_id", spec.universe_id.as_str()),
        ("staging_uri_template", spec.staging_uri_template.as_str()),
        ("category", spec.category.as_str()),
        ("symbol", spec.symbol.as_str()),
        ("source_binding", spec.source_binding.as_str()),
        ("source_hash_algorithm", spec.source_hash_algorithm.as_str()),
    ] {
        ensure!(!value.trim().is_empty(), "{field} must not be empty");
    }
    ensure!(
        spec.source_hash_algorithm.contains("etag"),
        "source_hash_algorithm {} is unsupported for archive index records",
        spec.source_hash_algorithm
    );
    ensure!(
        !spec.schema_columns.is_empty(),
        "schema_columns must not be empty"
    );
    Ok(())
}

fn source_hash(source_hash_algorithm: &str, etag: Option<&str>) -> Result<String> {
    ensure!(
        source_hash_algorithm.contains("etag"),
        "source_hash_algorithm {} is unsupported for archive index records",
        source_hash_algorithm
    );
    let etag = etag.context("archive index record missing ETag for ETag source hash")?;
    ensure!(
        !etag.trim().is_empty(),
        "archive index record ETag is empty"
    );
    Ok(etag.to_string())
}

fn render_staging_uri(
    spec: &SourceArchiveIndexSourceUniverseSpec,
    index: &SourceArchiveIndexManifest,
    archive_date: &str,
    source_hash: &str,
    template: &str,
) -> Result<String> {
    let source_hash_path = source_hash_path_component(&spec.source_hash_algorithm, source_hash);
    let mut output = template.to_string();
    for (token, value) in [
        ("{manifest_id}", spec.manifest_id.as_str()),
        ("{universe_id}", spec.universe_id.as_str()),
        ("{venue}", index.venue.as_str()),
        ("{source}", index.source.as_str()),
        ("{family}", index.family.as_str()),
        ("{table_family}", index.table_family.as_str()),
        ("{category}", spec.category.as_str()),
        ("{symbol}", spec.symbol.as_str()),
        ("{archive_date}", archive_date),
        (
            "{source_hash_algorithm}",
            spec.source_hash_algorithm.as_str(),
        ),
        ("{source_hash}", source_hash_path.as_str()),
        ("{source_hash_raw}", source_hash),
        ("{source_binding}", spec.source_binding.as_str()),
    ] {
        output = output.replace(token, value);
    }
    ensure!(
        !output.contains('{') && !output.contains('}'),
        "staging_uri_template contains an unsupported placeholder"
    );
    Ok(output)
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
