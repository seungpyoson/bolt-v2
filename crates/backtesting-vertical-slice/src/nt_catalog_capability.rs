use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    artifact_store::{ArtifactStoreConfig, ResolvedArtifactRoot},
    run_manifest::MarketStructureFixture,
};

pub const NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION: &str = "nt-catalog-capability-proof.v1";
pub const SYNTHETIC_SOURCE_PROOF_ID: &str = "synthetic-fixture";
pub const REQUIRED_AMBIENT_AWS_CREDENTIAL_ENV_VARS: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_DEFAULT_REGION",
    "AWS_REGION",
    "AWS_ENDPOINT",
    "AWS_ENDPOINT_URL_S3",
    "AWS_WEB_IDENTITY_TOKEN_FILE",
    "AWS_ROLE_ARN",
    "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
    "AWS_CONTAINER_CREDENTIALS_FULL_URI",
    "AWS_PROFILE",
];
const SYNTHETIC_PROVENANCE: &str = "synthetic";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NtCatalogCredentialSource {
    Ssm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogSsmParameterRefs {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default)]
    pub session_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AmbientCredentialScrubPlan {
    pub unset_env_vars: Vec<String>,
    pub profile_file_paths_redirected: bool,
    pub imds_blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityRunSpec {
    pub proof_run_id: String,
    pub nt_revision: String,
    pub credential_source: NtCatalogCredentialSource,
    pub expected_storage_options_keys: Vec<String>,
    pub synthetic_fixture_coverage: Vec<MarketStructureFixture>,
    pub synthetic_source_proof_id: String,
    pub provenance: String,
    pub ambient_credential_scrub: AmbientCredentialScrubPlan,
    pub ssm_parameter_refs: NtCatalogSsmParameterRefs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityPlan {
    pub proof_run_id: String,
    pub nt_revision: String,
    pub artifact_root_uri: String,
    pub synthetic_catalog_root_uri: String,
    pub credential_source: NtCatalogCredentialSource,
    pub storage_options_keys: Vec<String>,
    pub synthetic_fixture_coverage: Vec<MarketStructureFixture>,
    pub synthetic_source_proof_id: String,
    pub provenance: String,
    pub ambient_credential_scrub: AmbientCredentialScrubPlan,
    pub ssm_parameter_refs: NtCatalogSsmParameterRefs,
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

impl NtCatalogSsmParameterRefs {
    fn validate(&self) -> Result<()> {
        ensure_ssm_parameter_ref("ssm_parameter_refs.access_key_id", &self.access_key_id)?;
        ensure_ssm_parameter_ref(
            "ssm_parameter_refs.secret_access_key",
            &self.secret_access_key,
        )?;
        if let Some(session_token) = &self.session_token {
            ensure_ssm_parameter_ref("ssm_parameter_refs.session_token", session_token)?;
        }
        Ok(())
    }
}

impl AmbientCredentialScrubPlan {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.profile_file_paths_redirected,
            "ambient credential scrub must redirect AWS profile file paths"
        );
        ensure!(
            self.imds_blocked,
            "ambient credential scrub must block IMDS credential fallback"
        );
        let configured = unique_sorted_strings(&self.unset_env_vars)?;
        let mut required: Vec<String> = REQUIRED_AMBIENT_AWS_CREDENTIAL_ENV_VARS
            .iter()
            .map(|value| (*value).to_string())
            .collect();
        required.sort();
        ensure!(
            configured == required,
            "ambient credential scrub env var list must match the required AWS credential source set"
        );
        Ok(())
    }
}

impl NtCatalogCapabilityRunSpec {
    /// # Errors
    ///
    /// Returns an error if the proof run spec is incomplete or inconsistent
    /// with the configured artifact store.
    pub fn proof_plan(
        &self,
        artifact_store: &ArtifactStoreConfig,
    ) -> Result<NtCatalogCapabilityPlan> {
        ensure_valid_revision(&self.nt_revision)?;
        ensure!(
            self.credential_source == NtCatalogCredentialSource::Ssm,
            "NT catalog capability run spec credential source must be SSM"
        );
        ensure!(
            self.synthetic_source_proof_id == SYNTHETIC_SOURCE_PROOF_ID,
            "NT catalog capability run spec must use the synthetic fixture source proof id"
        );
        ensure!(
            self.provenance == SYNTHETIC_PROVENANCE,
            "NT catalog capability run spec provenance must be synthetic"
        );
        ensure_required_fixture_coverage(&self.synthetic_fixture_coverage)?;
        self.ambient_credential_scrub.validate()?;
        self.ssm_parameter_refs.validate()?;

        let artifact_root = artifact_store.resolve()?;
        let expected_storage_options_keys =
            ensure_sorted_storage_option_keys(&self.expected_storage_options_keys)?;
        let storage_options_keys = sorted_storage_option_keys(&artifact_root);
        ensure!(
            storage_options_keys == expected_storage_options_keys,
            "NT catalog capability run spec storage option keys must match artifact-store S3 config"
        );
        let synthetic_catalog_root_uri =
            artifact_root.nt_catalog_synthetic_proof_root(&self.proof_run_id)?;
        let plan = NtCatalogCapabilityPlan {
            proof_run_id: self.proof_run_id.clone(),
            nt_revision: self.nt_revision.clone(),
            artifact_root_uri: artifact_root.artifact_root_uri().to_string(),
            synthetic_catalog_root_uri,
            credential_source: self.credential_source,
            storage_options_keys,
            synthetic_fixture_coverage: self.synthetic_fixture_coverage.clone(),
            synthetic_source_proof_id: self.synthetic_source_proof_id.clone(),
            provenance: self.provenance.clone(),
            ambient_credential_scrub: self.ambient_credential_scrub.clone(),
            ssm_parameter_refs: self.ssm_parameter_refs.clone(),
        };
        plan.validate(&artifact_root)?;
        Ok(plan)
    }

    /// # Errors
    ///
    /// Returns an error unless the run spec and passed runtime controls prove a
    /// completed synthetic direct-S3 catalog capability run.
    pub fn completed_proof(
        &self,
        artifact_store: &ArtifactStoreConfig,
        controls: NtCatalogCapabilityControls,
    ) -> Result<NtCatalogCapabilityProof> {
        let artifact_root = artifact_store.resolve()?;
        let proof = self.proof_plan(artifact_store)?.completed_proof(controls);
        proof.validate(&artifact_root)?;
        Ok(proof)
    }
}

impl NtCatalogCapabilityPlan {
    fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        ensure_valid_revision(&self.nt_revision)?;
        ensure!(
            self.artifact_root_uri == artifact_root.artifact_root_uri(),
            "capability plan artifact_root_uri does not match configured artifact root"
        );
        ensure!(
            self.credential_source == NtCatalogCredentialSource::Ssm,
            "NT catalog capability plan credential source must be SSM"
        );
        ensure_storage_option_keys(&self.storage_options_keys)?;
        ensure_required_fixture_coverage(&self.synthetic_fixture_coverage)?;
        ensure!(
            self.synthetic_source_proof_id == SYNTHETIC_SOURCE_PROOF_ID,
            "capability plan must use the synthetic fixture source proof id"
        );
        ensure!(
            self.provenance == SYNTHETIC_PROVENANCE,
            "capability plan provenance must be synthetic"
        );
        self.ambient_credential_scrub.validate()?;
        self.ssm_parameter_refs.validate()?;
        let expected_synthetic_root =
            artifact_root.nt_catalog_synthetic_proof_root(&self.proof_run_id)?;
        ensure!(
            self.synthetic_catalog_root_uri == expected_synthetic_root,
            "capability plan synthetic catalog root must be derived from artifact_store.subpaths.nt_catalog_synthetic_proof"
        );
        ensure!(
            !self.synthetic_catalog_root_uri.contains("/nt-catalog/v1/"),
            "capability plan must not use the canonical NT catalog root"
        );
        Ok(())
    }

    #[must_use]
    pub fn completed_proof(
        self,
        controls: NtCatalogCapabilityControls,
    ) -> NtCatalogCapabilityProof {
        NtCatalogCapabilityProof {
            schema_version: NT_CATALOG_CAPABILITY_PROOF_SCHEMA_VERSION.to_string(),
            proof_run_id: self.proof_run_id,
            nt_revision: self.nt_revision,
            artifact_root_uri: self.artifact_root_uri,
            synthetic_catalog_root_uri: self.synthetic_catalog_root_uri,
            credential_source: self.credential_source,
            storage_options_keys: self.storage_options_keys,
            synthetic_fixture_coverage: self.synthetic_fixture_coverage,
            synthetic_source_proof_id: self.synthetic_source_proof_id,
            provenance: self.provenance,
            controls,
        }
    }
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
        ensure_required_fixture_coverage(&self.synthetic_fixture_coverage)?;
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

fn ensure_required_fixture_coverage(fixtures: &[MarketStructureFixture]) -> Result<()> {
    ensure!(
        fixtures.contains(&MarketStructureFixture::BinaryOption)
            && fixtures.contains(&MarketStructureFixture::PerpsSpot),
        "capability proof must cover binary-option and perps-spot synthetic fixtures"
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

fn ensure_sorted_storage_option_keys(keys: &[String]) -> Result<Vec<String>> {
    ensure_storage_option_keys(keys)?;
    let mut sorted = keys.to_vec();
    sorted.sort();
    Ok(sorted)
}

fn unique_sorted_strings(values: &[String]) -> Result<Vec<String>> {
    ensure!(!values.is_empty(), "value list must not be empty");
    let mut unique = BTreeSet::new();
    for value in values {
        let trimmed = value.trim();
        ensure!(!trimmed.is_empty(), "value must not be empty");
        ensure!(
            trimmed == value,
            "value must not contain leading or trailing whitespace"
        );
        ensure!(unique.insert(value.as_str()), "value list must be unique");
    }
    Ok(unique.into_iter().map(|value| value.to_string()).collect())
}

fn sorted_storage_option_keys(artifact_root: &ResolvedArtifactRoot) -> Vec<String> {
    let mut keys: Vec<String> = artifact_root
        .nt_catalog_storage_options()
        .into_keys()
        .collect();
    keys.sort();
    keys
}

fn ensure_ssm_parameter_ref(field: &'static str, value: &str) -> Result<()> {
    let trimmed = value.trim();
    ensure!(!trimmed.is_empty(), "{field} must not be empty");
    ensure!(
        trimmed == value,
        "{field} must not contain leading or trailing whitespace"
    );
    ensure!(
        value.starts_with('/'),
        "{field} must be an absolute SSM path"
    );
    ensure!(!value.contains("://"), "{field} must be an SSM path");
    ensure!(
        !value.split('/').any(|part| matches!(part, "." | "..")),
        "{field} must not contain current or parent path segments"
    );
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
