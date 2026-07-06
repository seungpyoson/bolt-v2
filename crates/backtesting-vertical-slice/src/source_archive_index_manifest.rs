//! Source archive index manifests.
//!
//! An index manifest records every object discovered from a public archive index
//! snapshot plus HEAD metadata. It is stronger than a discovery seed but weaker
//! than an accepted source-universe manifest because it does not prove payload
//! hashes or conversion policy.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::hashing::sha256_hex;
use crate::path_resolution::{resolve_existing_path, resolve_output_dir};
use crate::reference_artifact::ReferenceArtifactPin;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const SOURCE_ARCHIVE_INDEX_SNAPSHOT_SCHEMA_VERSION: &str = "source-archive-index-snapshot.v1";
pub const SOURCE_ARCHIVE_INDEX_MANIFEST_SCHEMA_VERSION: &str = "source-archive-index-manifest.v1";
pub const SOURCE_ARCHIVE_INDEX_MANIFEST_FILE: &str = "source-archive-index-manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexManifestSpec {
    pub manifest_id: String,
    pub index_snapshot_path: PathBuf,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexSnapshot {
    pub schema_version: String,
    pub snapshot_id: String,
    pub fetched_at_utc: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub index_url: String,
    pub page_count: u64,
    pub records: Vec<SourceArchiveIndexRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexRecord {
    pub page_number: u64,
    pub object_label: String,
    pub archive_hour_utc: String,
    pub source_url: String,
    pub listed_size_label: String,
    pub http_status: u16,
    pub content_length_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceArchiveIndexManifestStatus {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveIndexManifest {
    pub schema_version: String,
    pub manifest_id: String,
    pub status: SourceArchiveIndexManifestStatus,
    pub snapshot_id: String,
    pub fetched_at_utc: String,
    pub venue: String,
    pub source: String,
    pub family: String,
    pub table_family: String,
    pub index_url: String,
    pub page_count: u64,
    pub object_count: u64,
    pub verified_head_count: u64,
    pub total_content_length_bytes: u64,
    pub first_archive_hour_utc: String,
    pub last_archive_hour_utc: String,
    pub artifact_refs: Vec<ReferenceArtifactPin>,
    pub records: Vec<SourceArchiveIndexRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceArchiveIndexManifestArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub object_count: u64,
    pub verified_head_count: u64,
    pub total_content_length_bytes: u64,
}

pub fn write_source_archive_index_manifest_from_spec_file(
    spec_path: &Path,
) -> Result<SourceArchiveIndexManifestArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source archive index manifest spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceArchiveIndexManifestSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source archive index manifest spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_archive_index_manifest(&spec, base_dir)
}

pub fn write_source_archive_index_manifest(
    spec: &SourceArchiveIndexManifestSpec,
    base_dir: &Path,
) -> Result<SourceArchiveIndexManifestArtifact> {
    let manifest = evaluate_source_archive_index_manifest(spec, base_dir)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source archive index manifest directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_ARCHIVE_INDEX_MANIFEST_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_ARCHIVE_INDEX_MANIFEST_FILE,
        &manifest,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| format!("write source archive index manifest {}", path.display()))?;

    Ok(SourceArchiveIndexManifestArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        object_count: manifest.object_count,
        verified_head_count: manifest.verified_head_count,
        total_content_length_bytes: manifest.total_content_length_bytes,
    })
}

pub fn evaluate_source_archive_index_manifest(
    spec: &SourceArchiveIndexManifestSpec,
    base_dir: &Path,
) -> Result<SourceArchiveIndexManifest> {
    validate_non_empty("manifest_id", &spec.manifest_id)?;

    let snapshot_path = resolve_existing_path(base_dir, &spec.index_snapshot_path);
    let snapshot_hash = sha256_file(&snapshot_path)?;
    let snapshot: SourceArchiveIndexSnapshot = read_json(&snapshot_path)?;
    ensure!(
        snapshot.schema_version == SOURCE_ARCHIVE_INDEX_SNAPSHOT_SCHEMA_VERSION,
        "source archive index snapshot {} has unsupported schema version {}",
        snapshot_path.display(),
        snapshot.schema_version
    );
    validate_non_empty("snapshot_id", &snapshot.snapshot_id)?;
    validate_non_empty("fetched_at_utc", &snapshot.fetched_at_utc)?;
    validate_non_empty("venue", &snapshot.venue)?;
    validate_non_empty("source", &snapshot.source)?;
    validate_non_empty("family", &snapshot.family)?;
    validate_non_empty("table_family", &snapshot.table_family)?;
    validate_non_empty("index_url", &snapshot.index_url)?;
    ensure!(
        snapshot.index_url.starts_with("https://"),
        "index URL {} must use https",
        snapshot.index_url
    );
    ensure!(snapshot.page_count > 0, "page_count must be positive");
    ensure!(!snapshot.records.is_empty(), "records must not be empty");

    let mut seen_labels = BTreeSet::new();
    let mut seen_urls = BTreeSet::new();
    let mut verified_head_count = 0_u64;
    let mut total_content_length_bytes = 0_u64;
    let mut first_archive_hour_utc: Option<String> = None;
    let mut last_archive_hour_utc: Option<String> = None;

    for record in &snapshot.records {
        validate_non_empty("record.object_label", &record.object_label)?;
        validate_non_empty("record.archive_hour_utc", &record.archive_hour_utc)?;
        validate_non_empty("record.source_url", &record.source_url)?;
        validate_non_empty("record.listed_size_label", &record.listed_size_label)?;
        ensure!(
            record.page_number > 0 && record.page_number <= snapshot.page_count,
            "record {} page_number {} is outside page_count {}",
            record.object_label,
            record.page_number,
            snapshot.page_count
        );
        ensure!(
            record.source_url.starts_with("https://"),
            "record {} source URL must use https",
            record.object_label
        );
        ensure!(
            record.http_status == 200,
            "record {} must have HTTP 200 status",
            record.object_label
        );
        ensure!(
            record.content_length_bytes > 0,
            "record {} must have positive content length",
            record.object_label
        );
        ensure!(
            seen_labels.insert(record.object_label.clone()),
            "duplicate object label {}",
            record.object_label
        );
        ensure!(
            seen_urls.insert(record.source_url.clone()),
            "duplicate source URL {}",
            record.source_url
        );

        verified_head_count += 1;
        total_content_length_bytes =
            total_content_length_bytes.saturating_add(record.content_length_bytes);
        match first_archive_hour_utc.as_ref() {
            Some(existing) if existing <= &record.archive_hour_utc => {}
            _ => first_archive_hour_utc = Some(record.archive_hour_utc.clone()),
        }
        match last_archive_hour_utc.as_ref() {
            Some(existing) if existing >= &record.archive_hour_utc => {}
            _ => last_archive_hour_utc = Some(record.archive_hour_utc.clone()),
        }
    }

    Ok(SourceArchiveIndexManifest {
        schema_version: SOURCE_ARCHIVE_INDEX_MANIFEST_SCHEMA_VERSION.to_string(),
        manifest_id: spec.manifest_id.clone(),
        status: SourceArchiveIndexManifestStatus::Ready,
        snapshot_id: snapshot.snapshot_id,
        fetched_at_utc: snapshot.fetched_at_utc,
        venue: snapshot.venue,
        source: snapshot.source,
        family: snapshot.family,
        table_family: snapshot.table_family,
        index_url: snapshot.index_url,
        page_count: snapshot.page_count,
        object_count: snapshot.records.len() as u64,
        verified_head_count,
        total_content_length_bytes,
        first_archive_hour_utc: first_archive_hour_utc.unwrap_or_default(),
        last_archive_hour_utc: last_archive_hour_utc.unwrap_or_default(),
        // Committed manifests must be reproducible from any checkout: echo the
        // spec-authored path verbatim; resolution stays a read-time concern.
        artifact_refs: vec![ReferenceArtifactPin {
            role: "source_archive_index_snapshot".to_string(),
            path: spec.index_snapshot_path.clone(),
            sha256: snapshot_hash,
        }],
        records: snapshot.records,
    })
}

fn validate_non_empty(field: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    Ok(())
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
