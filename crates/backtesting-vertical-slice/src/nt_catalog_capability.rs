use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{artifact_store::ResolvedArtifactRoot, run_manifest::MarketStructureFixture};

pub const NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION: &str = "nt-catalog-capability-proof.v1";
pub const SYNTHETIC_SOURCE_PROOF_ID: &str = "synthetic-fixture";
const SYNTHETIC_PROVENANCE: &str = "synthetic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NtCatalogCredentialSource {
    Ssm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityControls {
    pub no_cloud_feature_gate_failed: bool,
    pub ambient_credentials_scrubbed: bool,
    pub invalid_credentials_write_failed: bool,
    pub ssm_credentials_write_reopen_query_succeeded: bool,
    pub conditional_put_probe_succeeded: bool,
    pub copy_if_not_exists_probe_succeeded: bool,
}

impl NtCatalogCapabilityControls {
    #[must_use]
    pub const fn all_passed(&self) -> bool {
        self.no_cloud_feature_gate_failed
            && self.ambient_credentials_scrubbed
            && self.invalid_credentials_write_failed
            && self.ssm_credentials_write_reopen_query_succeeded
            && self.conditional_put_probe_succeeded
            && self.copy_if_not_exists_probe_succeeded
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityProof {
    pub schema_version: String,
    pub proof_run_id: String,
    pub nt_revision: String,
    pub artifact_root_uri: String,
    pub synthetic_catalog_root_uri: String,
    pub credential_source: NtCatalogCredentialSource,
    pub storage_options_keys: Vec<String>,
    pub synthetic_fixture_coverage: Vec<MarketStructureFixture>,
    pub synthetic_source_proof_id: String,
    pub provenance: String,
    pub controls: NtCatalogCapabilityControls,
}

impl NtCatalogCapabilityProof {
    /// # Errors
    ///
    /// Returns an error if the proof root cannot be derived from the configured
    /// artifact root.
    pub fn synthetic_success(
        artifact_root: &ResolvedArtifactRoot,
        proof_run_id: impl Into<String>,
        nt_revision: impl Into<String>,
        storage_options_keys: Vec<String>,
    ) -> Result<Self> {
        let proof_run_id = proof_run_id.into();
        let synthetic_catalog_root_uri =
            artifact_root.nt_catalog_synthetic_proof_root(&proof_run_id)?;
        let proof = Self {
            schema_version: NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION.to_string(),
            proof_run_id,
            nt_revision: nt_revision.into(),
            artifact_root_uri: artifact_root.artifact_root_uri().to_string(),
            synthetic_catalog_root_uri,
            credential_source: NtCatalogCredentialSource::Ssm,
            storage_options_keys,
            synthetic_fixture_coverage: vec![
                MarketStructureFixture::BinaryOption,
                MarketStructureFixture::PerpsSpot,
            ],
            synthetic_source_proof_id: SYNTHETIC_SOURCE_PROOF_ID.to_string(),
            provenance: SYNTHETIC_PROVENANCE.to_string(),
            controls: NtCatalogCapabilityControls {
                no_cloud_feature_gate_failed: true,
                ambient_credentials_scrubbed: true,
                invalid_credentials_write_failed: true,
                ssm_credentials_write_reopen_query_succeeded: true,
                conditional_put_probe_succeeded: true,
                copy_if_not_exists_probe_succeeded: true,
            },
        };
        proof.validate(artifact_root)?;
        Ok(proof)
    }

    /// # Errors
    ///
    /// Returns an error if the proof record is incomplete or points outside the
    /// configured synthetic-only catalog proof root.
    pub fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        ensure!(
            self.schema_version == NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION,
            "unexpected NT catalog capability proof schema_version {:?}",
            self.schema_version
        );
        ensure!(
            self.artifact_root_uri == artifact_root.artifact_root_uri(),
            "capability proof artifact_root_uri does not match configured artifact root"
        );
        ensure_valid_revision(&self.nt_revision)?;
        let expected_synthetic_root =
            artifact_root.nt_catalog_synthetic_proof_root(&self.proof_run_id)?;
        ensure!(
            self.synthetic_catalog_root_uri == expected_synthetic_root,
            "capability proof synthetic catalog root must be derived from artifact_store.subpaths.nt_catalog_synthetic_proof"
        );
        ensure!(
            !self.synthetic_catalog_root_uri.contains("/nt-catalog/v1/"),
            "capability proof must not use the canonical NT catalog root"
        );
        ensure!(
            self.credential_source == NtCatalogCredentialSource::Ssm,
            "NT catalog capability proof credential source must be SSM"
        );
        ensure_storage_option_keys(&self.storage_options_keys)?;
        ensure!(
            self.synthetic_source_proof_id == SYNTHETIC_SOURCE_PROOF_ID,
            "capability proof must use the synthetic fixture source proof id"
        );
        ensure!(
            self.provenance == SYNTHETIC_PROVENANCE,
            "capability proof provenance must be synthetic"
        );
        ensure!(
            self.synthetic_fixture_coverage
                .contains(&MarketStructureFixture::BinaryOption)
                && self
                    .synthetic_fixture_coverage
                    .contains(&MarketStructureFixture::PerpsSpot),
            "capability proof must cover binary-option and perps-spot synthetic fixtures"
        );
        ensure!(
            self.controls.all_passed(),
            "capability proof controls must all pass before direct S3 catalog access is proven"
        );
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error unless the proof validates as a completed direct-S3
    /// synthetic capability proof.
    pub fn direct_s3_catalog_access_proven(
        &self,
        artifact_root: &ResolvedArtifactRoot,
    ) -> Result<()> {
        self.validate(artifact_root)
    }
}

fn ensure_valid_revision(value: &str) -> Result<()> {
    ensure!(
        value.len() == 40 && value.chars().all(|ch| ch.is_ascii_hexdigit()),
        "nt_revision must be a 40-character git revision"
    );
    Ok(())
}

fn ensure_storage_option_keys(keys: &[String]) -> Result<()> {
    ensure!(!keys.is_empty(), "storage_options_keys must not be empty");
    let mut unique = BTreeSet::new();
    for key in keys {
        let trimmed = key.trim();
        ensure!(!trimmed.is_empty(), "storage option key must not be empty");
        ensure!(
            trimmed == key,
            "storage option key must not contain leading or trailing whitespace"
        );
        ensure!(
            is_allowed_storage_option_key(key),
            "unsupported NT catalog storage option key {key:?}"
        );
        ensure!(
            unique.insert(key.as_str()),
            "storage option keys must be unique"
        );
    }
    Ok(())
}

fn is_allowed_storage_option_key(key: &str) -> bool {
    matches!(
        key,
        "endpoint_url"
            | "region"
            | "access_key_id"
            | "key"
            | "secret_access_key"
            | "secret"
            | "session_token"
            | "token"
            | "allow_http"
    )
}
