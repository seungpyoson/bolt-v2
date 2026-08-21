//! Bolt-owned converter boundary metadata.
//!
//! NautilusTrader owns catalog encoding, catalog query, and backtest execution.
//! It does not own Bolt's source-proof acceptance, converter identity, resume
//! checkpoint, or artifact-governance decisions. This module records that thin
//! boundary so a raw accepted source can become NT-ready catalog input only when
//! the output prefix is either clean, exactly idempotent, or resumable from a
//! validated checkpoint.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::io_safety::{collect_regular_files, open_regular_file};
pub const CONVERSION_MANIFEST_FILE: &str = "conversion-manifest.json";
pub const CONVERSION_CHECKPOINT_FILE: &str = "conversion-checkpoint.json";
pub const CATALOG_METADATA_FILE: &str = "catalog-metadata.json";
/// Multi-table conversion index; written ONLY when one accepted object
/// produced more than one projected catalog table. Single-table conversions
/// never write it, so existing single-table outputs stay byte-identical.
pub const CONVERSION_TABLES_FILE: &str = "conversion-tables.json";

pub const CONVERSION_MANIFEST_VERSION: &str = "conversion-manifest.v1";
pub const CONVERSION_CHECKPOINT_VERSION: &str = "conversion-checkpoint.v1";
pub const CATALOG_METADATA_VERSION: &str = "catalog-metadata.v1";

/// Converter identity fields that must match before output can be reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionFingerprint {
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub accepted_object_sha256: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub converter_config_hash: String,
}

impl ConversionFingerprint {
    pub fn validate_against(&self, expected: &Self) -> Result<()> {
        ensure_identity_field(
            "source_proof_id",
            &self.source_proof_id,
            &expected.source_proof_id,
        )?;
        if self.source_proof_version != expected.source_proof_version {
            bail!(
                "conversion identity mismatch: source_proof_version expected {}, got {}",
                expected.source_proof_version,
                self.source_proof_version
            );
        }
        ensure_identity_field(
            "accepted_object_sha256",
            &self.accepted_object_sha256,
            &expected.accepted_object_sha256,
        )?;
        ensure_identity_field(
            "converter_identity",
            &self.converter_identity,
            &expected.converter_identity,
        )?;
        ensure_identity_field(
            "converter_version",
            &self.converter_version,
            &expected.converter_version,
        )?;
        ensure_identity_field(
            "converter_config_hash",
            &self.converter_config_hash,
            &expected.converter_config_hash,
        )?;
        Ok(())
    }
}

fn ensure_identity_field(field: &'static str, actual: &str, expected: &str) -> Result<()> {
    ensure!(
        actual == expected,
        "conversion identity mismatch: {field} expected {expected:?}, got {actual:?}"
    );
    Ok(())
}

/// Durable stage marker for a conversion run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionCheckpointStage {
    Started,
    CanonicalWritten,
    CatalogProjected,
    Completed,
}

/// Durable checkpoint written before and during conversion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionCheckpoint {
    pub checkpoint_version: String,
    pub fingerprint: ConversionFingerprint,
    pub stage: ConversionCheckpointStage,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub canonical_rows: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub catalog_hash: Option<String>,
    pub updated_at: String,
}

impl ConversionCheckpoint {
    #[must_use]
    pub fn started(fingerprint: ConversionFingerprint, updated_at: impl Into<String>) -> Self {
        Self {
            checkpoint_version: CONVERSION_CHECKPOINT_VERSION.to_string(),
            fingerprint,
            stage: ConversionCheckpointStage::Started,
            canonical_rows: None,
            catalog_hash: None,
            updated_at: updated_at.into(),
        }
    }

    #[must_use]
    pub fn completed(
        fingerprint: ConversionFingerprint,
        canonical_rows: usize,
        catalog_hash: impl Into<String>,
        updated_at: impl Into<String>,
    ) -> Self {
        Self {
            checkpoint_version: CONVERSION_CHECKPOINT_VERSION.to_string(),
            fingerprint,
            stage: ConversionCheckpointStage::Completed,
            canonical_rows: Some(canonical_rows),
            catalog_hash: Some(catalog_hash.into()),
            updated_at: updated_at.into(),
        }
    }

    pub fn validate_for(&self, expected: &ConversionFingerprint) -> Result<()> {
        ensure!(
            self.checkpoint_version == CONVERSION_CHECKPOINT_VERSION,
            "unexpected conversion checkpoint version: expected {CONVERSION_CHECKPOINT_VERSION:?}, got {:?}",
            self.checkpoint_version
        );
        ensure!(
            !self.updated_at.trim().is_empty(),
            "conversion checkpoint updated_at must not be empty"
        );
        self.fingerprint.validate_against(expected)?;
        if self.stage == ConversionCheckpointStage::Completed {
            ensure!(
                self.canonical_rows.is_some(),
                "completed conversion checkpoint missing canonical_rows"
            );
            ensure!(
                self.catalog_hash
                    .as_ref()
                    .is_some_and(|hash| !hash.trim().is_empty()),
                "completed conversion checkpoint missing catalog_hash"
            );
        }
        Ok(())
    }

    pub fn content_hash(&self) -> Result<String> {
        content_hash(self)
    }
}

/// Completed conversion manifest binding input proof to NT catalog output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionManifest {
    pub manifest_version: String,
    pub fingerprint: ConversionFingerprint,
    pub normalized_schema_version: String,
    pub nt_data_type: String,
    pub nt_instrument_id: String,
    pub canonical_rows: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog_nt_data_types: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub catalog_rows_by_nt_data_type: BTreeMap<String, usize>,
    pub output_catalog_uri: String,
    pub catalog_hash: String,
    pub checkpoint_hash: String,
    pub completed_at: String,
}

impl ConversionManifest {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn completed(
        fingerprint: ConversionFingerprint,
        normalized_schema_version: impl Into<String>,
        nt_data_type: impl Into<String>,
        nt_instrument_id: impl Into<String>,
        canonical_rows: usize,
        output_catalog_uri: impl Into<String>,
        catalog_hash: impl Into<String>,
        checkpoint_hash: impl Into<String>,
        completed_at: impl Into<String>,
    ) -> Self {
        let nt_data_type = nt_data_type.into();
        let canonical_rows_by_nt_data_type =
            BTreeMap::from([(nt_data_type.clone(), canonical_rows)]);
        Self {
            manifest_version: CONVERSION_MANIFEST_VERSION.to_string(),
            fingerprint,
            normalized_schema_version: normalized_schema_version.into(),
            nt_data_type: nt_data_type.clone(),
            nt_instrument_id: nt_instrument_id.into(),
            canonical_rows,
            catalog_nt_data_types: vec![nt_data_type],
            catalog_rows_by_nt_data_type: canonical_rows_by_nt_data_type,
            output_catalog_uri: output_catalog_uri.into(),
            catalog_hash: catalog_hash.into(),
            checkpoint_hash: checkpoint_hash.into(),
            completed_at: completed_at.into(),
        }
    }

    #[must_use]
    pub fn with_catalog_rows_by_nt_data_type(
        mut self,
        catalog_rows_by_nt_data_type: BTreeMap<String, usize>,
    ) -> Self {
        self.catalog_nt_data_types = catalog_rows_by_nt_data_type.keys().cloned().collect();
        self.catalog_rows_by_nt_data_type = catalog_rows_by_nt_data_type;
        self
    }

    #[must_use]
    pub fn effective_catalog_nt_data_types(&self) -> Vec<String> {
        if self.catalog_nt_data_types.is_empty() {
            vec![self.nt_data_type.clone()]
        } else {
            self.catalog_nt_data_types.clone()
        }
    }

    #[must_use]
    pub fn effective_catalog_rows_by_nt_data_type(&self) -> BTreeMap<String, usize> {
        if self.catalog_rows_by_nt_data_type.is_empty() {
            BTreeMap::from([(self.nt_data_type.clone(), self.canonical_rows)])
        } else {
            self.catalog_rows_by_nt_data_type.clone()
        }
    }

    pub fn validate_for(
        &self,
        expected: &ConversionFingerprint,
        checkpoint_hash: &str,
    ) -> Result<()> {
        ensure!(
            self.manifest_version == CONVERSION_MANIFEST_VERSION,
            "unexpected conversion manifest version: expected {CONVERSION_MANIFEST_VERSION:?}, got {:?}",
            self.manifest_version
        );
        self.fingerprint.validate_against(expected)?;
        ensure!(
            self.checkpoint_hash == checkpoint_hash,
            "conversion manifest checkpoint_hash mismatch: expected {checkpoint_hash:?}, got {:?}",
            self.checkpoint_hash
        );
        ensure!(
            !self.normalized_schema_version.trim().is_empty(),
            "conversion manifest missing normalized_schema_version"
        );
        ensure!(
            !self.nt_data_type.trim().is_empty(),
            "conversion manifest missing nt_data_type"
        );
        let catalog_nt_data_types = self.effective_catalog_nt_data_types();
        let catalog_rows_by_nt_data_type = self.effective_catalog_rows_by_nt_data_type();
        ensure!(
            !catalog_nt_data_types.is_empty(),
            "conversion manifest missing catalog_nt_data_types"
        );
        ensure!(
            catalog_nt_data_types
                .iter()
                .all(|data_type| !data_type.trim().is_empty()),
            "conversion manifest catalog_nt_data_types contains empty data type"
        );
        ensure!(
            catalog_rows_by_nt_data_type
                .keys()
                .all(|data_type| !data_type.trim().is_empty()),
            "conversion manifest catalog_rows_by_nt_data_type contains empty data type"
        );
        ensure!(
            catalog_nt_data_types
                .iter()
                .all(|data_type| catalog_rows_by_nt_data_type.contains_key(data_type)),
            "conversion manifest catalog_nt_data_types missing row-count binding"
        );
        ensure!(
            catalog_rows_by_nt_data_type.contains_key(&self.nt_data_type),
            "conversion manifest missing primary nt_data_type row-count binding"
        );
        ensure!(
            catalog_rows_by_nt_data_type
                .get(&self.nt_data_type)
                .copied()
                == Some(self.canonical_rows),
            "conversion manifest primary nt_data_type row count must match canonical_rows"
        );
        ensure!(
            !self.nt_instrument_id.trim().is_empty(),
            "conversion manifest missing nt_instrument_id"
        );
        ensure!(
            !self.output_catalog_uri.trim().is_empty(),
            "conversion manifest missing output_catalog_uri"
        );
        ensure!(
            !self.catalog_hash.trim().is_empty(),
            "conversion manifest missing catalog_hash"
        );
        ensure!(
            !self.completed_at.trim().is_empty(),
            "conversion manifest completed_at must not be empty"
        );
        Ok(())
    }

    pub fn content_hash(&self) -> Result<String> {
        content_hash(self)
    }
}

/// Catalog-local metadata written next to the NT catalog projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionCatalogMetadata {
    pub metadata_version: String,
    pub manifest_hash: String,
    pub checkpoint_hash: String,
    pub catalog_hash: String,
    pub nt_data_type: String,
    pub nt_instrument_id: String,
    pub canonical_rows: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog_nt_data_types: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub catalog_rows_by_nt_data_type: BTreeMap<String, usize>,
    pub output_catalog_uri: String,
    pub execution_catalog_uri: String,
    pub direct_s3_catalog_access_proven: bool,
}

impl ConversionCatalogMetadata {
    #[must_use]
    pub fn from_manifest(
        manifest: &ConversionManifest,
        manifest_hash: String,
        checkpoint_hash: String,
    ) -> Self {
        Self {
            metadata_version: CATALOG_METADATA_VERSION.to_string(),
            manifest_hash,
            checkpoint_hash,
            catalog_hash: manifest.catalog_hash.clone(),
            nt_data_type: manifest.nt_data_type.clone(),
            nt_instrument_id: manifest.nt_instrument_id.clone(),
            canonical_rows: manifest.canonical_rows,
            catalog_nt_data_types: manifest.catalog_nt_data_types.clone(),
            catalog_rows_by_nt_data_type: manifest.catalog_rows_by_nt_data_type.clone(),
            output_catalog_uri: manifest.output_catalog_uri.clone(),
            execution_catalog_uri: manifest.output_catalog_uri.clone(),
            direct_s3_catalog_access_proven: false,
        }
    }

    #[must_use]
    pub fn with_execution_catalog_access(
        mut self,
        execution_catalog_uri: impl Into<String>,
        direct_s3_catalog_access_proven: bool,
    ) -> Self {
        self.execution_catalog_uri = execution_catalog_uri.into();
        self.direct_s3_catalog_access_proven = direct_s3_catalog_access_proven;
        self
    }

    #[must_use]
    pub fn effective_catalog_nt_data_types(&self) -> Vec<String> {
        if self.catalog_nt_data_types.is_empty() {
            vec![self.nt_data_type.clone()]
        } else {
            self.catalog_nt_data_types.clone()
        }
    }

    #[must_use]
    pub fn effective_catalog_rows_by_nt_data_type(&self) -> BTreeMap<String, usize> {
        if self.catalog_rows_by_nt_data_type.is_empty() {
            BTreeMap::from([(self.nt_data_type.clone(), self.canonical_rows)])
        } else {
            self.catalog_rows_by_nt_data_type.clone()
        }
    }

    fn validate_against(
        &self,
        manifest: &ConversionManifest,
        manifest_hash: &str,
        checkpoint_hash: &str,
    ) -> Result<()> {
        ensure!(
            self.metadata_version == CATALOG_METADATA_VERSION,
            "unexpected catalog metadata version: expected {CATALOG_METADATA_VERSION:?}, got {:?}",
            self.metadata_version
        );
        ensure!(
            self.manifest_hash == manifest_hash,
            "catalog metadata manifest_hash mismatch: expected {manifest_hash:?}, got {:?}",
            self.manifest_hash
        );
        ensure!(
            self.checkpoint_hash == checkpoint_hash,
            "catalog metadata checkpoint_hash mismatch: expected {checkpoint_hash:?}, got {:?}",
            self.checkpoint_hash
        );
        ensure!(
            self.catalog_hash == manifest.catalog_hash,
            "catalog metadata catalog_hash mismatch: expected {:?}, got {:?}",
            manifest.catalog_hash,
            self.catalog_hash
        );
        ensure!(
            self.nt_data_type == manifest.nt_data_type,
            "catalog metadata nt_data_type mismatch"
        );
        ensure!(
            self.nt_instrument_id == manifest.nt_instrument_id,
            "catalog metadata nt_instrument_id mismatch"
        );
        ensure!(
            self.canonical_rows == manifest.canonical_rows,
            "catalog metadata canonical_rows mismatch"
        );
        let metadata_catalog_nt_data_types = self.effective_catalog_nt_data_types();
        let metadata_catalog_rows_by_nt_data_type = self.effective_catalog_rows_by_nt_data_type();
        ensure!(
            metadata_catalog_nt_data_types == manifest.effective_catalog_nt_data_types(),
            "catalog metadata catalog_nt_data_types mismatch"
        );
        ensure!(
            metadata_catalog_rows_by_nt_data_type
                == manifest.effective_catalog_rows_by_nt_data_type(),
            "catalog metadata catalog_rows_by_nt_data_type mismatch"
        );
        ensure!(
            self.output_catalog_uri == manifest.output_catalog_uri,
            "catalog metadata output_catalog_uri mismatch"
        );
        ensure!(
            !self.execution_catalog_uri.trim().is_empty(),
            "catalog metadata execution_catalog_uri must not be empty"
        );
        ensure!(
            !self.direct_s3_catalog_access_proven
                || self.execution_catalog_uri.starts_with("s3://"),
            "catalog metadata cannot claim direct S3 access for non-S3 execution catalog URI {:?}",
            self.execution_catalog_uri
        );
        Ok(())
    }

    pub fn content_hash(&self) -> Result<String> {
        content_hash(self)
    }
}

/// One projected catalog table of a multi-table conversion.
///
/// `subroot_uri` is the artifact-root-relative subroot path (joinable onto the
/// local output directory for inspection and onto the published output prefix
/// for the portable URI). `bar_spec` is the lowercase `<step><aggregation>`
/// discriminant, present only for the bar family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionTableRecord {
    pub table_family: String,
    pub nt_instrument_id: String,
    pub data_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bar_spec: Option<String>,
    pub subroot_uri: String,
    pub catalog_hash: String,
    pub rows: usize,
}

impl ConversionTableRecord {
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("table_family", &self.table_family),
            ("nt_instrument_id", &self.nt_instrument_id),
            ("data_type", &self.data_type),
            ("subroot_uri", &self.subroot_uri),
            ("catalog_hash", &self.catalog_hash),
        ] {
            ensure!(
                !value.trim().is_empty(),
                "conversion table record field {name} must not be empty"
            );
        }
        if let Some(bar_spec) = &self.bar_spec {
            ensure!(
                !bar_spec.trim().is_empty(),
                "conversion table record bar_spec must not be empty when present"
            );
        }
        ensure!(
            self.rows > 0,
            "conversion table record rows must be positive"
        );
        ensure!(
            !self.subroot_uri.starts_with('/'),
            "conversion table record subroot_uri must be artifact-root-relative, got {:?}",
            self.subroot_uri
        );
        ensure!(
            self.subroot_uri
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
            "conversion table record subroot_uri must be a clean relative path, got {:?}",
            self.subroot_uri
        );
        Ok(())
    }
}

/// Write the multi-table conversion index. The caller must only invoke this
/// for conversions that produced more than one table.
pub fn write_conversion_tables_index(
    output_dir: &Path,
    records: &[ConversionTableRecord],
) -> Result<PathBuf> {
    ensure!(
        records.len() > 1,
        "conversion tables index is only written for multi-table conversions, got {} record(s)",
        records.len()
    );
    for record in records {
        record.validate()?;
    }
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create conversion output dir {}", output_dir.display()))?;
    let path = output_dir.join(CONVERSION_TABLES_FILE);
    crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        CONVERSION_TABLES_FILE,
        &records,
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

/// Validate a completed conversion's optional multi-table index against the
/// aggregate manifest and the on-disk catalog subroots.
///
/// Returns the parsed records when the index is present, `None` for a
/// single-table conversion (no index file). Fails loud when the index exists
/// but is inconsistent: fewer than two records, duplicate table identities,
/// per-data-type row totals diverging from the manifest aggregate, a missing
/// primary record, or any subroot whose recomputed logical catalog hash does
/// not match the recorded one.
pub fn validate_conversion_tables_index(
    output_dir: &Path,
    manifest: &ConversionManifest,
) -> Result<Option<Vec<ConversionTableRecord>>> {
    let path = output_dir.join(CONVERSION_TABLES_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let records: Vec<ConversionTableRecord> = read_json(&path)?;
    ensure!(
        records.len() > 1,
        "conversion tables index {} must describe more than one table, got {}",
        path.display(),
        records.len()
    );
    let mut identities = std::collections::BTreeSet::new();
    let mut rows_by_data_type: BTreeMap<String, usize> = BTreeMap::new();
    for record in &records {
        record.validate()?;
        ensure!(
            identities.insert((
                record.table_family.clone(),
                record.nt_instrument_id.clone(),
                record.data_type.clone(),
                record.bar_spec.clone(),
            )),
            "conversion tables index contains duplicate table identity {}/{}/{}",
            record.table_family,
            record.nt_instrument_id,
            record.data_type
        );
        let total = rows_by_data_type
            .entry(record.data_type.clone())
            .or_insert(0);
        *total = total
            .checked_add(record.rows)
            .context("conversion tables index row total overflow")?;
        let subroot = output_dir.join(&record.subroot_uri);
        let actual_hash = crate::catalog_projection::logical_catalog_hash(&subroot)
            .with_context(|| format!("recompute catalog hash {}", subroot.display()))?;
        ensure!(
            actual_hash == record.catalog_hash,
            "conversion tables index subroot {} hash mismatch: recorded {:?}, recomputed {:?}",
            record.subroot_uri,
            record.catalog_hash,
            actual_hash
        );
    }
    ensure!(
        rows_by_data_type == manifest.effective_catalog_rows_by_nt_data_type(),
        "conversion tables index per-data-type rows {:?} do not match conversion manifest {:?}",
        rows_by_data_type,
        manifest.effective_catalog_rows_by_nt_data_type()
    );
    let primary_matches = records
        .iter()
        .filter(|record| {
            record.nt_instrument_id == manifest.nt_instrument_id
                && record.data_type == manifest.nt_data_type
                && record.catalog_hash == manifest.catalog_hash
        })
        .count();
    ensure!(
        primary_matches == 1,
        "conversion tables index must contain exactly one primary record matching the \
         conversion manifest, found {primary_matches}"
    );
    Ok(Some(records))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionOutputState {
    CleanNew,
    ResumeFromCheckpoint {
        stage: ConversionCheckpointStage,
    },
    Complete {
        manifest_hash: String,
        checkpoint_hash: String,
        catalog_hash: String,
    },
}

pub fn inspect_conversion_output(
    output_dir: &Path,
    expected: &ConversionFingerprint,
) -> Result<ConversionOutputState> {
    let root_metadata = match fs::symlink_metadata(output_dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConversionOutputState::CleanNew);
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("inspect conversion output root {}", output_dir.display())
            });
        }
    };
    ensure!(
        root_metadata.file_type().is_dir(),
        "dirty conversion output {}: output root is not a real directory",
        output_dir.display()
    );
    collect_regular_files(output_dir, "conversion output")?;

    let mut entries = fs::read_dir(output_dir)
        .with_context(|| format!("read conversion output dir {}", output_dir.display()))?;
    if entries
        .next()
        .transpose()
        .with_context(|| format!("read conversion output dir {}", output_dir.display()))?
        .is_none()
    {
        return Ok(ConversionOutputState::CleanNew);
    }

    let checkpoint_path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    let manifest_path = output_dir.join(CONVERSION_MANIFEST_FILE);
    let metadata_path = output_dir.join(CATALOG_METADATA_FILE);

    if !checkpoint_path.exists() {
        bail!(
            "dirty conversion output {}: non-empty output has no validated {CONVERSION_CHECKPOINT_FILE}",
            output_dir.display()
        );
    }

    let checkpoint: ConversionCheckpoint = read_json(&checkpoint_path)?;
    checkpoint.validate_for(expected)?;
    let checkpoint_hash = checkpoint.content_hash()?;

    if !manifest_path.exists() {
        ensure!(
            checkpoint.stage != ConversionCheckpointStage::Completed,
            "dirty conversion output {}: completed checkpoint is missing {CONVERSION_MANIFEST_FILE}",
            output_dir.display()
        );
        return Ok(ConversionOutputState::ResumeFromCheckpoint {
            stage: checkpoint.stage,
        });
    }

    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "dirty conversion output {}: manifest exists but checkpoint stage is {:?}",
        output_dir.display(),
        checkpoint.stage
    );
    let manifest: ConversionManifest = read_json(&manifest_path)?;
    manifest.validate_for(expected, &checkpoint_hash)?;
    let manifest_hash = manifest.content_hash()?;

    ensure!(
        metadata_path.exists(),
        "dirty conversion output {}: completed conversion is missing {CATALOG_METADATA_FILE}",
        output_dir.display()
    );
    let metadata: ConversionCatalogMetadata = read_json(&metadata_path)?;
    metadata.validate_against(&manifest, &manifest_hash, &checkpoint_hash)?;
    // Multi-table conversions additionally bind every projected subroot
    // through the tables index; single-table conversions have no index file
    // and stay byte-identical.
    validate_conversion_tables_index(output_dir, &manifest)?;

    Ok(ConversionOutputState::Complete {
        manifest_hash,
        checkpoint_hash,
        catalog_hash: manifest.catalog_hash,
    })
}

pub fn write_conversion_checkpoint(
    output_dir: &Path,
    checkpoint: &ConversionCheckpoint,
) -> Result<PathBuf> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create conversion output dir {}", output_dir.display()))?;
    let path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        CONVERSION_CHECKPOINT_FILE,
        checkpoint,
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn write_completed_conversion_artifacts(
    output_dir: &Path,
    manifest: &ConversionManifest,
    checkpoint: &ConversionCheckpoint,
    metadata: &ConversionCatalogMetadata,
) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create conversion output dir {}", output_dir.display()))?;
    let checkpoint_path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    crate::reference_artifact::write_reference_artifact_with_len(
        &checkpoint_path,
        CONVERSION_CHECKPOINT_FILE,
        checkpoint,
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| format!("write {}", checkpoint_path.display()))?;
    let manifest_path = output_dir.join(CONVERSION_MANIFEST_FILE);
    crate::reference_artifact::write_reference_artifact_with_len(
        &manifest_path,
        CONVERSION_MANIFEST_FILE,
        manifest,
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| format!("write {}", manifest_path.display()))?;
    let metadata_path = output_dir.join(CATALOG_METADATA_FILE);
    crate::reference_artifact::write_reference_artifact_with_len(
        &metadata_path,
        CATALOG_METADATA_FILE,
        metadata,
        crate::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| format!("write {}", metadata_path.display()))?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let mut file = open_regular_file(path, "conversion artifact")?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn content_hash<T: Serialize>(value: &T) -> Result<String> {
    crate::reference_artifact::canonical_json_sha256(value)
        .context("serialize conversion artifact for hash")
}
