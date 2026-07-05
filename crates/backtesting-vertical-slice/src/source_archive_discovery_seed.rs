//! Source archive discovery seed artifacts.
//!
//! A discovery seed is deliberately weaker than an accepted source universe:
//! it records source bindings, list prefixes, and representative object HEAD
//! evidence so a later manifest generator has explicit venue-scale inputs.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::path_resolution::resolve_output_dir;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub const SOURCE_ARCHIVE_DISCOVERY_SEED_SCHEMA_VERSION: &str = "source-archive-discovery-seed.v1";
pub const SOURCE_ARCHIVE_DISCOVERY_SEED_FILE: &str = "source-archive-discovery-seed.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveDiscoverySeedSpec {
    pub discovery_id: String,
    pub venue: String,
    pub source: String,
    pub window_start: String,
    pub window_end: String,
    pub output_dir: PathBuf,
    #[serde(rename = "binding", default)]
    pub bindings: Vec<SourceArchiveDiscoveryBindingSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveDiscoveryBindingSpec {
    pub source_binding: String,
    pub product_family: String,
    pub table_family: String,
    pub source_uri_template: String,
    pub list_prefix: String,
    pub representative_object: SourceArchiveDiscoveryRepresentativeObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveDiscoveryRepresentativeObject {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_label: Option<String>,
    pub archive_date: String,
    pub source_url: String,
    pub http_status: u16,
    pub content_length_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceArchiveDiscoverySeedStatus {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveDiscoveryBinding {
    pub source_binding: String,
    pub product_family: String,
    pub table_family: String,
    pub source_uri_template: String,
    pub list_prefix: String,
    pub representative_object: SourceArchiveDiscoveryRepresentativeObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceArchiveDiscoverySeed {
    pub schema_version: String,
    pub discovery_id: String,
    pub status: SourceArchiveDiscoverySeedStatus,
    pub venue: String,
    pub source: String,
    pub window_start: String,
    pub window_end: String,
    pub source_binding_count: u64,
    pub representative_object_count: u64,
    pub total_representative_object_bytes: u64,
    pub product_families: Vec<String>,
    pub table_families: Vec<String>,
    pub bindings: Vec<SourceArchiveDiscoveryBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceArchiveDiscoverySeedArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub source_binding_count: u64,
    pub representative_object_count: u64,
}

pub fn write_source_archive_discovery_seed_from_spec_file(
    spec_path: &Path,
) -> Result<SourceArchiveDiscoverySeedArtifact> {
    let spec_bytes = fs::read(spec_path).with_context(|| {
        format!(
            "read source archive discovery seed spec {}",
            spec_path.display()
        )
    })?;
    let spec: SourceArchiveDiscoverySeedSpec =
        toml::from_slice(&spec_bytes).with_context(|| {
            format!(
                "parse source archive discovery seed spec TOML {}",
                spec_path.display()
            )
        })?;
    let base_dir = spec_path.parent().unwrap_or_else(|| Path::new("."));
    write_source_archive_discovery_seed(&spec, base_dir)
}

pub fn write_source_archive_discovery_seed(
    spec: &SourceArchiveDiscoverySeedSpec,
    base_dir: &Path,
) -> Result<SourceArchiveDiscoverySeedArtifact> {
    let seed = evaluate_source_archive_discovery_seed(spec)?;
    let output_dir = resolve_output_dir(base_dir, &spec.output_dir);
    fs::create_dir_all(&output_dir).with_context(|| {
        format!(
            "create source archive discovery seed directory {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(SOURCE_ARCHIVE_DISCOVERY_SEED_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        SOURCE_ARCHIVE_DISCOVERY_SEED_FILE,
        &seed,
    )
    .with_context(|| format!("write source archive discovery seed {}", path.display()))?;

    Ok(SourceArchiveDiscoverySeedArtifact {
        path,
        content_hash: written.pin.sha256,
        bytes: written.bytes,
        source_binding_count: seed.source_binding_count,
        representative_object_count: seed.representative_object_count,
    })
}

pub fn evaluate_source_archive_discovery_seed(
    spec: &SourceArchiveDiscoverySeedSpec,
) -> Result<SourceArchiveDiscoverySeed> {
    validate_non_empty("discovery_id", &spec.discovery_id)?;
    validate_non_empty("venue", &spec.venue)?;
    validate_non_empty("source", &spec.source)?;
    validate_non_empty("window_start", &spec.window_start)?;
    validate_non_empty("window_end", &spec.window_end)?;
    ensure!(!spec.bindings.is_empty(), "binding set must not be empty");

    let mut seen_bindings = BTreeSet::new();
    let mut seen_urls = BTreeSet::new();
    let mut product_families = BTreeSet::new();
    let mut table_families = BTreeSet::new();
    let mut total_representative_object_bytes = 0_u64;
    let mut bindings = Vec::with_capacity(spec.bindings.len());

    for binding in &spec.bindings {
        validate_non_empty("source_binding", &binding.source_binding)?;
        validate_non_empty("product_family", &binding.product_family)?;
        validate_non_empty("table_family", &binding.table_family)?;
        validate_non_empty("source_uri_template", &binding.source_uri_template)?;
        validate_non_empty("list_prefix", &binding.list_prefix)?;
        ensure!(
            seen_bindings.insert(binding.source_binding.clone()),
            "duplicate source binding {}",
            binding.source_binding
        );

        let representative = &binding.representative_object;
        let has_symbol = representative
            .symbol
            .as_deref()
            .is_some_and(|symbol| !symbol.trim().is_empty());
        let has_object_label = representative
            .object_label
            .as_deref()
            .is_some_and(|label| !label.trim().is_empty());
        ensure!(
            has_symbol || has_object_label,
            "representative object for {} must include symbol or object_label",
            binding.source_binding
        );
        validate_non_empty(
            "representative_object.archive_date",
            &representative.archive_date,
        )?;
        validate_non_empty(
            "representative_object.source_url",
            &representative.source_url,
        )?;
        ensure!(
            representative.source_url.starts_with("https://"),
            "representative object {} must use https URL",
            representative.source_url
        );
        ensure!(
            representative.http_status == 200,
            "representative object {} must have HTTP 200 status",
            representative.source_url
        );
        ensure!(
            representative.content_length_bytes > 0,
            "representative object {} must have positive content length",
            representative.source_url
        );
        ensure!(
            seen_urls.insert(representative.source_url.clone()),
            "duplicate representative object {}",
            representative.source_url
        );

        product_families.insert(binding.product_family.clone());
        table_families.insert(binding.table_family.clone());
        total_representative_object_bytes += representative.content_length_bytes;
        bindings.push(SourceArchiveDiscoveryBinding {
            source_binding: binding.source_binding.clone(),
            product_family: binding.product_family.clone(),
            table_family: binding.table_family.clone(),
            source_uri_template: binding.source_uri_template.clone(),
            list_prefix: binding.list_prefix.clone(),
            representative_object: representative.clone(),
        });
    }

    Ok(SourceArchiveDiscoverySeed {
        schema_version: SOURCE_ARCHIVE_DISCOVERY_SEED_SCHEMA_VERSION.to_string(),
        discovery_id: spec.discovery_id.clone(),
        status: SourceArchiveDiscoverySeedStatus::Ready,
        venue: spec.venue.clone(),
        source: spec.source.clone(),
        window_start: spec.window_start.clone(),
        window_end: spec.window_end.clone(),
        source_binding_count: bindings.len() as u64,
        representative_object_count: seen_urls.len() as u64,
        total_representative_object_bytes,
        product_families: product_families.into_iter().collect(),
        table_families: table_families.into_iter().collect(),
        bindings,
    })
}

fn validate_non_empty(field: &str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    Ok(())
}
