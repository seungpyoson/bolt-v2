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
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    atomic_artifact_write::atomic_file_create_or_verify_guarded,
    operator_work_budget::{
        CooperativeDeadlineWriter, OperatorWorkBudgetGuard, OperatorWorkBudgetStage,
    },
};
pub const CONVERSION_MANIFEST_FILE: &str = "conversion-manifest.json";
pub const CONVERSION_CHECKPOINT_FILE: &str = "conversion-checkpoint.json";
pub const CATALOG_METADATA_FILE: &str = "catalog-metadata.json";
/// Sole structural path marker for a derived immutable conversion generation.
pub const CONVERSION_GENERATION_PATH_MARKER: &str = "/conversion=";
/// Multi-table conversion index; written ONLY when one accepted object
/// produced more than one projected catalog table. Single-table conversions
/// never write it, so existing single-table outputs stay byte-identical.
pub const CONVERSION_TABLES_FILE: &str = "conversion-tables.json";

// v4 adds the path-owned complete conversion-semantics digest to the embedded
// fingerprint. RA-001a intentionally uses a conservative normalized full
// RunSpec hash with only the terminal `/conversion=<sha256>` suffix removed.
// That can rotate a generation for provenance-only RunSpec changes; RA-001b
// owns narrowing it to the proven output-semantics projection plus the selected
// binding/capability digest. Other paths must bind their own complete
// output-determining semantics.
pub const CONVERSION_MANIFEST_VERSION: &str = "conversion-manifest.v4";
pub const CONVERSION_CHECKPOINT_VERSION: &str = "conversion-checkpoint.v4";
// v3 embeds a catalog publication receipt whose immutable identity requires a
// non-empty ETag; v2 metadata must fail closed. The v4 conversion fingerprint
// remains indirect through the embedded manifest/checkpoint hashes.
pub const CATALOG_METADATA_VERSION: &str = "catalog-metadata.v3";

/// Converter identity fields that must match before output can be reused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversionFingerprint {
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub accepted_object_sha256: String,
    /// Portable identity of the source-control artifact whose exact bytes
    /// authorized this conversion (for RunSpec flows, the source-bindings
    /// registry). This is deliberately generic so non-RunSpec conversions
    /// bind their own authoritative control artifact through the same reuse
    /// contract.
    pub control_artifact_path: String,
    pub control_artifact_sha256: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub converter_config_hash: String,
    /// Canonical digest of the explicit NT Parquet encoding configuration.
    /// Completed output is reusable only when this identity is unchanged.
    pub catalog_encoding_hash: String,
    /// Canonical digest of this path's conversion semantics. RA-001a RunSpec
    /// paths conservatively hash the normalized full RunSpec, removing only
    /// their derived terminal generation suffix; RA-001b owns the narrower
    /// proven output-semantics projection. Non-RunSpec paths bind their own
    /// exact semantics.
    pub conversion_semantics_sha256: String,
}

impl ConversionFingerprint {
    pub fn validate(&self) -> Result<()> {
        self.validate_control_artifact_identity()
    }

    pub fn validate_against(&self, expected: &Self) -> Result<()> {
        self.validate()?;
        expected.validate()?;
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
            "control_artifact_path",
            &self.control_artifact_path,
            &expected.control_artifact_path,
        )?;
        ensure_identity_field(
            "control_artifact_sha256",
            &self.control_artifact_sha256,
            &expected.control_artifact_sha256,
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
        ensure_identity_field(
            "catalog_encoding_hash",
            &self.catalog_encoding_hash,
            &expected.catalog_encoding_hash,
        )?;
        ensure_identity_field(
            "conversion_semantics_sha256",
            &self.conversion_semantics_sha256,
            &expected.conversion_semantics_sha256,
        )?;
        Ok(())
    }

    /// Canonical identity of one conversion generation.
    ///
    /// The fingerprint is the complete conversion-reuse identity. Hashing its
    /// canonical JSON representation gives durable publication a deterministic
    /// namespace without adding a separately configurable identity field.
    pub fn conversion_generation_sha256(&self) -> Result<String> {
        self.validate()?;
        crate::reference_artifact::canonical_json_sha256(self)
            .map_err(anyhow::Error::from)
            .context("hash canonical conversion fingerprint")
    }

    /// Require the durable output root to end at this exact generation.
    /// Trailing slashes and descendants are rejected because either would
    /// create another terminal namespace for the same fingerprint.
    pub fn validate_output_prefix_generation(&self, output_prefix: &str) -> Result<()> {
        let generation = self.conversion_generation_sha256()?;
        let expected_suffix = format!("{CONVERSION_GENERATION_PATH_MARKER}{generation}");
        ensure!(
            output_prefix.ends_with(&expected_suffix),
            "durable manifest.output_prefix conversion generation suffix mismatch: expected exact suffix {expected_suffix:?}, got {output_prefix:?}"
        );
        ensure!(
            output_prefix
                .matches(CONVERSION_GENERATION_PATH_MARKER)
                .count()
                == 1,
            "durable manifest.output_prefix must contain exactly one conversion generation suffix"
        );
        Ok(())
    }

    fn validate_control_artifact_identity(&self) -> Result<()> {
        ensure!(
            !self.control_artifact_path.trim().is_empty(),
            "conversion control_artifact_path must not be empty"
        );
        ensure!(
            crate::hashing::is_lowercase_sha256_hex(&self.control_artifact_sha256),
            "conversion control_artifact_sha256 must be 64 lowercase-hex characters"
        );
        ensure!(
            crate::hashing::is_lowercase_sha256_hex(&self.catalog_encoding_hash),
            "conversion catalog_encoding_hash must be 64 lowercase-hex characters"
        );
        ensure!(
            crate::hashing::is_lowercase_sha256_hex(&self.conversion_semantics_sha256),
            "conversion_semantics_sha256 must be 64 lowercase-hex characters"
        );
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

/// Local progress stage for a conversion run. This is never a remote
/// completion authority; durable completion is represented only by the exact
/// versioned terminal manifest in the operator lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversionCheckpointStage {
    Started,
    CanonicalWritten,
    CatalogProjected,
    Completed,
}

/// Local checkpoint written before and during conversion. A `Completed` stage
/// means conversion work finished locally, not that a durable run committed.
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
#[serde(deny_unknown_fields)]
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
    #[serde(default)]
    pub catalog_consumption: CatalogConsumption,
}

/// Exact immutable receipt identity produced by catalog publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPublicationReceiptIdentity {
    pub catalog_root_uri: String,
    pub receipt_uri: String,
    pub receipt_sha256: String,
    pub receipt_version_id: String,
    pub receipt_e_tag: String,
    pub physical_manifest_sha256: String,
}

impl CatalogPublicationReceiptIdentity {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.catalog_root_uri.starts_with("s3://") && self.catalog_root_uri.ends_with('/'),
            "catalog publication root URI must be a canonical S3 directory URI"
        );
        ensure!(
            !self.receipt_uri.trim().is_empty(),
            "catalog publication receipt URI must not be empty"
        );
        let receipt_relative = self
            .receipt_uri
            .strip_prefix(&self.catalog_root_uri)
            .context("catalog publication receipt URI must be beneath its exact catalog root")?;
        ensure!(
            !receipt_relative.is_empty() && !receipt_relative.ends_with('/'),
            "catalog publication receipt URI must identify an object beneath its exact catalog root"
        );
        ensure!(
            crate::hashing::is_lowercase_sha256_hex(&self.receipt_sha256),
            "catalog publication receipt SHA-256 must be 64 lowercase-hex characters"
        );
        ensure!(
            !self.receipt_version_id.trim().is_empty(),
            "catalog publication receipt version id must not be empty"
        );
        ensure!(
            !self.receipt_e_tag.trim().is_empty(),
            "catalog publication receipt ETag must not be empty"
        );
        ensure!(
            crate::hashing::is_lowercase_sha256_hex(&self.physical_manifest_sha256),
            "catalog publication physical-manifest SHA-256 must be 64 lowercase-hex characters"
        );
        Ok(())
    }
}

/// Stable catalog-consumption identity persisted into catalog metadata.
///
/// A hydrated publication deliberately omits its private local hydration path:
/// that path is a per-attempt implementation detail and would make an otherwise
/// identical retry produce different metadata and result-contract bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogConsumption {
    #[default]
    Unproven,
    LocalCatalog {
        catalog_uri: String,
    },
    HydratedPublication {
        receipt: CatalogPublicationReceiptIdentity,
    },
}

/// Runtime evidence used to bind one proven consumption path. Remote
/// publication is consumable only after its exact receipt has been hydrated
/// into one absolute local run view. The local hydration path is validated but
/// never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogConsumptionEvidence {
    LocalCatalog {
        catalog_uri: String,
    },
    HydratedPublication {
        local_catalog_root: PathBuf,
        receipt: CatalogPublicationReceiptIdentity,
    },
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
            catalog_consumption: CatalogConsumption::Unproven,
        }
    }

    /// Apply typed local-consumption evidence. Raw remote catalog execution is
    /// intentionally not representable by this API.
    pub fn with_catalog_consumption_evidence(
        mut self,
        evidence: CatalogConsumptionEvidence,
    ) -> Result<Self> {
        let consumption = match evidence {
            CatalogConsumptionEvidence::LocalCatalog { catalog_uri } => {
                validate_local_execution_catalog_uri(&catalog_uri)?;
                CatalogConsumption::LocalCatalog { catalog_uri }
            }
            CatalogConsumptionEvidence::HydratedPublication {
                local_catalog_root,
                receipt,
            } => {
                validate_local_execution_catalog_root(&local_catalog_root)?;
                receipt.validate()?;
                CatalogConsumption::HydratedPublication { receipt }
            }
        };
        ensure!(
            matches!(self.catalog_consumption, CatalogConsumption::Unproven)
                || self.catalog_consumption == consumption,
            "catalog consumption identity cannot be replaced"
        );
        self.catalog_consumption = consumption;
        Ok(self)
    }

    #[must_use]
    pub fn catalog_consumption_proven(&self) -> bool {
        !matches!(self.catalog_consumption, CatalogConsumption::Unproven)
    }

    #[must_use]
    pub fn hydrated_publication_receipt(&self) -> Option<&CatalogPublicationReceiptIdentity> {
        match &self.catalog_consumption {
            CatalogConsumption::HydratedPublication { receipt } => Some(receipt),
            CatalogConsumption::Unproven | CatalogConsumption::LocalCatalog { .. } => None,
        }
    }

    #[must_use]
    pub fn local_catalog_uri(&self) -> Option<&str> {
        match &self.catalog_consumption {
            CatalogConsumption::LocalCatalog { catalog_uri } => Some(catalog_uri),
            CatalogConsumption::Unproven | CatalogConsumption::HydratedPublication { .. } => None,
        }
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

    pub(crate) fn validate_against(
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
        match &self.catalog_consumption {
            CatalogConsumption::Unproven => {}
            CatalogConsumption::LocalCatalog { catalog_uri } => {
                validate_local_execution_catalog_uri(catalog_uri)?;
            }
            CatalogConsumption::HydratedPublication { receipt } => receipt.validate()?,
        }
        Ok(())
    }

    pub fn content_hash(&self) -> Result<String> {
        content_hash(self)
    }
}

fn validate_local_execution_catalog_uri(uri: &str) -> Result<()> {
    ensure!(
        !uri.trim().is_empty(),
        "local execution catalog URI must not be empty"
    );
    ensure!(
        !uri.contains("://") && Path::new(uri).is_absolute(),
        "execution catalog URI must be an absolute local path, got {uri:?}"
    );
    Ok(())
}

fn validate_local_execution_catalog_root(root: &Path) -> Result<()> {
    ensure!(
        root.is_absolute(),
        "hydrated execution catalog root must be absolute, got {}",
        root.display()
    );
    ensure!(
        root.to_str().is_some(),
        "hydrated execution catalog root must be valid UTF-8"
    );
    Ok(())
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
    write_conversion_tables_index_guarded(
        output_dir,
        records,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub fn write_conversion_tables_index_guarded(
    output_dir: &Path,
    records: &[ConversionTableRecord],
    work_budget: &OperatorWorkBudgetGuard,
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
    write_immutable_conversion_artifact_guarded(
        &path,
        CONVERSION_TABLES_FILE,
        &records,
        work_budget,
        OperatorWorkBudgetStage::Finalize,
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
    expected.validate()?;
    if !output_dir.exists() {
        return Ok(ConversionOutputState::CleanNew);
    }
    let mut entries = fs::read_dir(output_dir)
        .with_context(|| format!("read conversion output dir {}", output_dir.display()))?;
    let first_entry = entries
        .next()
        .transpose()
        .with_context(|| format!("read conversion output dir {}", output_dir.display()))?;
    if first_entry.is_none() {
        return Ok(ConversionOutputState::CleanNew);
    }

    let checkpoint_path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    let manifest_path = output_dir.join(CONVERSION_MANIFEST_FILE);
    let metadata_path = output_dir.join(CATALOG_METADATA_FILE);

    if !checkpoint_path.exists() {
        return Ok(ConversionOutputState::ResumeFromCheckpoint {
            stage: ConversionCheckpointStage::Started,
        });
    }

    let checkpoint: ConversionCheckpoint = read_json(&checkpoint_path)?;
    checkpoint.validate_for(expected)?;
    let checkpoint_hash = checkpoint.content_hash()?;

    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "legacy nonterminal conversion checkpoint cannot be overwritten by the immutable completion protocol"
    );

    if !manifest_path.exists() {
        bail!(
            "dirty conversion output {}: completed checkpoint is missing {CONVERSION_MANIFEST_FILE}",
            output_dir.display()
        );
    }

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
    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "only a completed immutable conversion checkpoint may be persisted"
    );
    checkpoint.validate_for(&checkpoint.fingerprint)?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create conversion output dir {}", output_dir.display()))?;
    let path = output_dir.join(CONVERSION_CHECKPOINT_FILE);
    write_immutable_conversion_artifact_guarded(
        &path,
        CONVERSION_CHECKPOINT_FILE,
        checkpoint,
        &OperatorWorkBudgetGuard::unbounded(),
        OperatorWorkBudgetStage::Finalize,
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
    write_completed_conversion_artifacts_guarded(
        output_dir,
        manifest,
        checkpoint,
        metadata,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub fn write_completed_conversion_artifacts_guarded(
    output_dir: &Path,
    manifest: &ConversionManifest,
    checkpoint: &ConversionCheckpoint,
    metadata: &ConversionCatalogMetadata,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    ensure!(
        checkpoint.stage == ConversionCheckpointStage::Completed,
        "completion commit requires a completed conversion checkpoint"
    );
    let checkpoint_hash = checkpoint.content_hash()?;
    checkpoint.validate_for(&manifest.fingerprint)?;
    manifest.validate_for(&manifest.fingerprint, &checkpoint_hash)?;
    let manifest_hash = manifest.content_hash()?;
    metadata.validate_against(manifest, &manifest_hash, &checkpoint_hash)?;
    write_pending_conversion_artifacts(output_dir, manifest, metadata, work_budget)?;
    write_immutable_conversion_artifact_guarded(
        &output_dir.join(CONVERSION_CHECKPOINT_FILE),
        CONVERSION_CHECKPOINT_FILE,
        checkpoint,
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )
    .context("commit immutable completed conversion checkpoint")?;
    Ok(())
}

/// Write all local completion artifacts except the completed checkpoint commit
/// object. Preterminal state is represented only by the immutable artifacts
/// already present; no mutable started checkpoint is persisted.
pub fn write_pending_conversion_artifacts(
    output_dir: &Path,
    manifest: &ConversionManifest,
    metadata: &ConversionCatalogMetadata,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    manifest.fingerprint.validate()?;
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create conversion output dir {}", output_dir.display()))?;
    let manifest_path = output_dir.join(CONVERSION_MANIFEST_FILE);
    write_immutable_conversion_artifact_guarded(
        &manifest_path,
        CONVERSION_MANIFEST_FILE,
        manifest,
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )
    .with_context(|| format!("write {}", manifest_path.display()))?;
    let metadata_path = output_dir.join(CATALOG_METADATA_FILE);
    write_immutable_conversion_artifact_guarded(
        &metadata_path,
        CATALOG_METADATA_FILE,
        metadata,
        work_budget,
        OperatorWorkBudgetStage::Finalize,
    )
    .with_context(|| format!("write {}", metadata_path.display()))?;
    Ok(())
}

fn write_immutable_conversion_artifact_guarded<T: Serialize>(
    path: &Path,
    role: &str,
    value: &T,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<()> {
    let bytes = crate::reference_artifact::canonical_json_bytes(value)
        .with_context(|| format!("serialize immutable {role}"))?;
    work_budget.verify_decoded_bytes(
        u64::try_from(bytes.len())
            .context("immutable conversion artifact length does not fit u64")?,
        stage,
    )?;
    atomic_file_create_or_verify_guarded(path, work_budget, stage, |file| {
        let mut writer = CooperativeDeadlineWriter::new(file, work_budget, stage);
        writer
            .write_all(&bytes)
            .with_context(|| format!("write immutable {role}"))?;
        writer
            .flush()
            .with_context(|| format!("flush immutable {role}"))?;
        Ok(())
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn content_hash<T: Serialize>(value: &T) -> Result<String> {
    crate::reference_artifact::canonical_json_sha256(value)
        .context("serialize conversion artifact for hash")
}
