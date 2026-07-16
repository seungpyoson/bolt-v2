use std::{collections::BTreeSet, env, str::FromStr};

use ahash::AHashMap;
use anyhow::{Context, Result, anyhow, ensure};
use aws_config::BehaviorVersion;
use aws_sdk_ssm::{Client as SsmClient, config::Region};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::{AggressorSide, AssetClass},
    identifiers::{InstrumentId, Symbol, TradeId},
    instruments::{BinaryOption, CryptoPerpetual, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::backend::catalog::{CatalogPathPrefix, ParquetDataCatalog};
use serde::{Deserialize, Serialize};

use crate::{
    artifact_store::{
        ArtifactStoreConfig, CreateOnlyArtifactWriter, CreateOnlyProbeTranscript,
        CreateOnlyWriteDisposition, ResolvedArtifactRoot, S3ArtifactStoreCredentials,
    },
    operator_work_budget::{
        OperatorWorkBudgetGuard, OperatorWorkBudgetStage, serialize_json_to_vec_guarded,
        serialized_json_len_with_budget, sha256_hex_with_budget,
    },
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

#[derive(Debug, Clone)]
pub struct NtCatalogSsmCredentialResolver {
    client: SsmClient,
}

impl NtCatalogSsmCredentialResolver {
    /// # Errors
    ///
    /// Returns an error if the configured AWS region is empty.
    pub async fn from_region(region: &str) -> Result<Self> {
        ensure!(
            !region.trim().is_empty(),
            "SSM credential resolver region must not be empty"
        );
        ensure!(
            region.trim() == region,
            "SSM credential resolver region must not include surrounding whitespace"
        );
        let config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .load()
            .await;
        Ok(Self {
            client: SsmClient::new(&config),
        })
    }

    #[must_use]
    pub fn new(client: SsmClient) -> Self {
        Self { client }
    }

    /// # Errors
    ///
    /// Returns an error when the configured SSM refs are invalid, a parameter
    /// cannot be fetched with decryption enabled, or a resolved credential
    /// value is empty.
    pub async fn resolve(
        &self,
        refs: &NtCatalogSsmParameterRefs,
    ) -> Result<S3ArtifactStoreCredentials> {
        refs.validate()?;
        let access_key_id = self
            .resolve_required_parameter(
                "ssm_parameter_refs.access_key_id",
                refs.access_key_id.as_str(),
            )
            .await?;
        let secret_access_key = self
            .resolve_required_parameter(
                "ssm_parameter_refs.secret_access_key",
                refs.secret_access_key.as_str(),
            )
            .await?;
        let session_token = if let Some(session_token_ref) = &refs.session_token {
            Some(
                self.resolve_required_parameter(
                    "ssm_parameter_refs.session_token",
                    session_token_ref.as_str(),
                )
                .await?,
            )
        } else {
            None
        };
        S3ArtifactStoreCredentials::new(access_key_id, secret_access_key, session_token)
    }

    async fn resolve_required_parameter(
        &self,
        field: &'static str,
        parameter_ref: &str,
    ) -> Result<String> {
        let response = self
            .client
            .get_parameter()
            .name(parameter_ref)
            .with_decryption(true)
            .send()
            .await
            .map_err(|error| {
                let source = aws_sdk_ssm::error::DisplayErrorContext(&error).to_string();
                let redacted_source = source.replace(parameter_ref, "[configured-ssm-parameter]");
                anyhow!("AWS SSM GetParameter failed for {field}: {redacted_source}")
            })?;
        response
            .parameter()
            .and_then(|parameter| parameter.value())
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("AWS SSM GetParameter returned no value for {field}"))
    }
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
pub struct NtCatalogSyntheticFixtures {
    pub binary_option: NtCatalogSyntheticBinaryOptionSpec,
    pub perps_spot: NtCatalogSyntheticPerpsSpotSpec,
    pub trade_ticks: Vec<NtCatalogSyntheticTradeTickSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogSyntheticBinaryOptionSpec {
    pub instrument_id: String,
    pub raw_symbol: String,
    pub asset_class: String,
    pub currency: String,
    pub activation_ns: u64,
    pub expiration_ns: u64,
    pub price_increment: String,
    pub size_increment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogSyntheticPerpsSpotSpec {
    pub instrument_id: String,
    pub raw_symbol: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub settlement_currency: String,
    pub is_inverse: bool,
    pub price_increment: String,
    pub size_increment: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogSyntheticTradeTickSpec {
    pub instrument_id: String,
    pub price: String,
    pub size: String,
    pub aggressor_side: String,
    pub trade_id: String,
    pub ts_event: u64,
    pub ts_init: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityRunSpec {
    pub proof_run_id: String,
    pub nt_revision: String,
    pub credential_source: NtCatalogCredentialSource,
    pub proof_artifact_object_name: String,
    pub expected_storage_options_keys: Vec<String>,
    pub synthetic_fixture_coverage: Vec<MarketStructureFixture>,
    pub synthetic_fixtures: NtCatalogSyntheticFixtures,
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
    pub proof_artifact_uri: String,
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
pub struct NtCatalogCapabilityProofArtifact {
    pub proof_artifact_uri: String,
    pub proof_artifact_sha256: String,
    pub proof_artifact_version_id: String,
    pub proof_artifact_e_tag: Option<String>,
    pub proof_artifact_create_only_write: CreateOnlyWriteDisposition,
    pub proof: NtCatalogCapabilityProof,
    pub evidence: NtCatalogCapabilityEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityProofDocument {
    pub proof: NtCatalogCapabilityProof,
    pub evidence: NtCatalogCapabilityEvidence,
}

#[derive(Serialize)]
struct NtCatalogCapabilityProofDocumentRef<'a> {
    proof: &'a NtCatalogCapabilityProof,
    evidence: &'a NtCatalogCapabilityEvidence,
}

pub struct NtCatalogS3ConformanceProbe {
    catalog_uri: String,
    storage_options: AHashMap<String, String>,
    instruments: Vec<InstrumentAny>,
    trade_ticks: Vec<TradeTick>,
    binary_option_instrument_id: String,
    perps_spot_instrument_id: String,
}

impl NtCatalogS3ConformanceProbe {
    /// # Errors
    ///
    /// Returns an error if the probe URI is not an S3 URI, required explicit
    /// S3 storage options are missing, or the synthetic fixture data is empty.
    pub fn new(
        catalog_uri: String,
        storage_options: AHashMap<String, String>,
        instruments: Vec<InstrumentAny>,
        trade_ticks: Vec<TradeTick>,
        binary_option_instrument_id: String,
        perps_spot_instrument_id: String,
    ) -> Result<Self> {
        let probe = Self {
            catalog_uri,
            storage_options,
            instruments,
            trade_ticks,
            binary_option_instrument_id,
            perps_spot_instrument_id,
        };
        probe.validate()?;
        Ok(probe)
    }

    fn validate(&self) -> Result<()> {
        ensure_read_back_catalog_uri(&self.catalog_uri)?;
        ensure!(
            !self.instruments.is_empty(),
            "NT catalog S3 conformance probe must include synthetic instruments"
        );
        ensure!(
            !self.trade_ticks.is_empty(),
            "NT catalog S3 conformance probe must include synthetic trade ticks"
        );
        validate_read_back_instrument_id(
            "binary-option",
            self.binary_option_instrument_id.as_str(),
        )?;
        validate_read_back_instrument_id("perps-spot", self.perps_spot_instrument_id.as_str())?;
        ensure_nt_s3_storage_options(&self.storage_options)?;
        let instrument_ids = self
            .instruments
            .iter()
            .map(|instrument| instrument.id().to_string())
            .collect::<BTreeSet<_>>();
        ensure!(
            instrument_ids.contains(&self.binary_option_instrument_id),
            "NT catalog S3 conformance probe must include the binary-option synthetic instrument"
        );
        ensure!(
            instrument_ids.contains(&self.perps_spot_instrument_id),
            "NT catalog S3 conformance probe must include the perps-spot synthetic instrument"
        );
        Ok(())
    }
}

/// Run the single NT catalog S3 conformance core under the caller's shared
/// cooperative work budget.
///
/// # Errors
///
/// Returns an error when the work budget expires before or after any NT catalog
/// operation, or when the conformance probe itself fails.
pub fn run_nt_catalog_s3_conformance_probe_guarded(
    probe: NtCatalogS3ConformanceProbe,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<NtCatalogReadBackEvidence> {
    run_guarded_nt_catalog_step(work_budget, || probe.validate())?;
    let NtCatalogS3ConformanceProbe {
        catalog_uri,
        storage_options,
        instruments,
        trade_ticks,
        binary_option_instrument_id,
        perps_spot_instrument_id,
    } = probe;
    let instrument_ids = run_guarded_nt_catalog_step(work_budget, || {
        Ok(instruments
            .iter()
            .map(|instrument| instrument.id().to_string())
            .collect::<Vec<_>>())
    })?;
    let expected_trade_tick_count = trade_ticks.len();
    let mut catalog = run_guarded_nt_catalog_step(work_budget, || {
        ParquetDataCatalog::from_uri(&catalog_uri, Some(storage_options), None, None, None)
    })?;
    run_guarded_nt_catalog_step(work_budget, || catalog.write_instruments(instruments))?;
    run_guarded_nt_catalog_step(work_budget, || {
        catalog.write_to_parquet(&trade_ticks, None, None, None)
    })?;
    let files = run_guarded_nt_catalog_step(work_budget, || {
        catalog.query_files(
            TradeTick::path_prefix(),
            Some(instrument_ids.clone()),
            None,
            None,
        )
    })?;
    let queried_instruments = run_guarded_nt_catalog_step(work_budget, || {
        catalog.query_instruments(Some(&instrument_ids))
    })?;
    let queried_instrument_ids = run_guarded_nt_catalog_step(work_budget, || {
        Ok(queried_instruments
            .iter()
            .map(|instrument| instrument.id().to_string())
            .collect::<BTreeSet<_>>())
    })?;
    let queried_trade_ticks = run_guarded_nt_catalog_step(work_budget, || {
        catalog.query_typed_data::<TradeTick>(Some(instrument_ids), None, None, None, None, true)
    })?;
    run_guarded_nt_catalog_step(work_budget, || {
        let evidence = NtCatalogReadBackEvidence {
            catalog_uri,
            query_files_succeeded: true,
            query_files_result_count: files.len(),
            write_instruments_succeeded: true,
            write_trade_ticks_succeeded: true,
            query_trade_ticks_succeeded: true,
            query_trade_ticks_result_count: queried_trade_ticks.len(),
            query_instruments_succeeded: true,
            query_instruments_result_count: queried_instruments.len(),
            binary_option_instrument_read_back: queried_instrument_ids
                .contains(&binary_option_instrument_id),
            binary_option_instrument_id,
            perps_spot_instrument_read_back: queried_instrument_ids
                .contains(&perps_spot_instrument_id),
            perps_spot_instrument_id,
        };
        ensure!(
            evidence.query_trade_ticks_result_count == expected_trade_tick_count,
            "NT catalog S3 conformance probe trade tick query count does not match write count"
        );
        evidence.validate()?;
        Ok(evidence)
    })
}

fn guarded_nt_catalog_operation_outcome<T>(
    work_budget: &OperatorWorkBudgetGuard,
    operation: impl FnOnce() -> Result<T>,
) -> Result<std::result::Result<T, anyhow::Error>> {
    crate::operator_work_budget::guarded_operation_outcome(
        work_budget,
        OperatorWorkBudgetStage::Publish,
        operation,
    )
}

fn run_guarded_nt_catalog_step<T>(
    work_budget: &OperatorWorkBudgetGuard,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    guarded_nt_catalog_operation_outcome(work_budget, operation)?
}

fn guarded_expected_failure(
    work_budget: &OperatorWorkBudgetGuard,
    operation: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    match guarded_nt_catalog_operation_outcome(work_budget, operation)? {
        Ok(()) => Ok(false),
        Err(_) => Ok(true),
    }
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
    /// # Errors
    ///
    /// Returns an error unless every runtime evidence field needed to prove
    /// direct-S3 NT catalog access has passed.
    pub fn from_evidence(evidence: &NtCatalogCapabilityEvidence) -> Result<Self> {
        ensure!(
            evidence.no_cloud_feature_gate_failed,
            "capability evidence must include the no-cloud-feature negative control"
        );
        ensure!(
            evidence.ambient_credentials_scrubbed,
            "capability evidence must prove ambient AWS credentials were scrubbed"
        );
        ensure!(
            evidence.invalid_credentials_write_failed,
            "capability evidence must prove invalid credentials fail writes"
        );
        ensure!(
            evidence.ssm_credentials_write_reopen_query_succeeded,
            "capability evidence must prove SSM credentials can write, reopen, and query"
        );
        ensure_storage_option_keys(&evidence.nt_catalog_storage_option_keys)?;
        evidence.read_back.validate()?;
        ensure!(
            evidence.create_only_probe.first_create_succeeded,
            "capability evidence must include the first create-only write"
        );
        ensure!(
            evidence.create_only_probe.duplicate_create_rejected,
            "capability evidence must prove duplicate create-only writes are rejected"
        );
        ensure!(
            evidence.create_only_probe.first_copy_succeeded,
            "capability evidence must include the first copy-if-not-exists write"
        );
        ensure!(
            evidence.create_only_probe.duplicate_copy_rejected,
            "capability evidence must prove duplicate copy-if-not-exists writes are rejected"
        );
        Ok(Self {
            no_cloud_feature_gate_failed: evidence.no_cloud_feature_gate_failed,
            ambient_credentials_scrubbed: evidence.ambient_credentials_scrubbed,
            invalid_credentials_write_failed: evidence.invalid_credentials_write_failed,
            ssm_credentials_write_reopen_query_succeeded: evidence
                .ssm_credentials_write_reopen_query_succeeded,
            conditional_put_probe_succeeded: evidence.create_only_probe.first_create_succeeded
                && evidence.create_only_probe.duplicate_create_rejected,
            copy_if_not_exists_probe_succeeded: evidence.create_only_probe.first_copy_succeeded
                && evidence.create_only_probe.duplicate_copy_rejected,
        })
    }

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
pub struct NtCatalogReadBackEvidence {
    pub catalog_uri: String,
    pub query_files_succeeded: bool,
    pub query_files_result_count: usize,
    pub write_instruments_succeeded: bool,
    pub write_trade_ticks_succeeded: bool,
    pub query_trade_ticks_succeeded: bool,
    pub query_trade_ticks_result_count: usize,
    pub query_instruments_succeeded: bool,
    pub query_instruments_result_count: usize,
    pub binary_option_instrument_read_back: bool,
    pub binary_option_instrument_id: String,
    pub perps_spot_instrument_read_back: bool,
    pub perps_spot_instrument_id: String,
}

impl NtCatalogReadBackEvidence {
    fn validate(&self) -> Result<()> {
        ensure_read_back_catalog_uri(&self.catalog_uri)?;
        ensure!(
            self.query_files_succeeded,
            "capability evidence must prove NT query_files read-back"
        );
        ensure!(
            self.query_files_result_count > 0,
            "capability evidence must include NT query_files result count"
        );
        ensure!(
            self.write_instruments_succeeded,
            "capability evidence must prove NT write_instruments over S3"
        );
        ensure!(
            self.write_trade_ticks_succeeded,
            "capability evidence must prove NT write_to_parquet over S3"
        );
        ensure!(
            self.query_trade_ticks_succeeded,
            "capability evidence must prove NT query_typed_data read-back over S3"
        );
        ensure!(
            self.query_trade_ticks_result_count > 0,
            "capability evidence must include NT query_typed_data result count"
        );
        ensure!(
            self.query_instruments_succeeded,
            "capability evidence must prove NT query_instruments read-back"
        );
        ensure!(
            self.query_instruments_result_count > 0,
            "capability evidence must include NT query_instruments result count"
        );
        ensure!(
            self.binary_option_instrument_read_back,
            "capability evidence must read back the binary-option synthetic instrument"
        );
        validate_read_back_instrument_id(
            "binary-option",
            self.binary_option_instrument_id.as_str(),
        )?;
        ensure!(
            self.perps_spot_instrument_read_back,
            "capability evidence must read back the perps-spot synthetic instrument"
        );
        validate_read_back_instrument_id("perps-spot", self.perps_spot_instrument_id.as_str())?;
        Ok(())
    }
}

fn ensure_read_back_catalog_uri(catalog_uri: &str) -> Result<()> {
    let trimmed = catalog_uri.trim();
    ensure!(
        !trimmed.is_empty(),
        "capability evidence must include the NT read-back catalog URI"
    );
    ensure!(
        trimmed == catalog_uri,
        "capability evidence NT read-back catalog URI must not include surrounding whitespace"
    );
    ensure!(
        catalog_uri.starts_with("s3://"),
        "capability evidence NT read-back catalog URI must be an S3 URI"
    );
    ensure!(
        !catalog_uri.chars().any(char::is_whitespace),
        "capability evidence NT read-back catalog URI must not contain whitespace"
    );
    Ok(())
}

fn ensure_read_back_catalog_uri_matches(
    catalog_uri: &str,
    synthetic_catalog_root_uri: &str,
) -> Result<()> {
    ensure_read_back_catalog_uri(catalog_uri)?;
    ensure!(
        catalog_uri == synthetic_catalog_root_uri,
        "capability proof read-back catalog URI must match synthetic catalog root"
    );
    Ok(())
}

fn parse_instrument_id(field: &'static str, value: &str) -> Result<InstrumentId> {
    InstrumentId::from_str(value).with_context(|| format!("invalid {field} {value:?}"))
}

fn parse_symbol(field: &'static str, value: &str) -> Result<Symbol> {
    Symbol::new_checked(value).map_err(|error| anyhow!("invalid {field} {value:?}: {error}"))
}

fn parse_currency(field: &'static str, value: &str) -> Result<Currency> {
    Currency::from_str(value).with_context(|| format!("invalid {field} {value:?}"))
}

fn parse_price(field: &'static str, value: &str) -> Result<Price> {
    Price::from_str(value).map_err(|error| anyhow!("invalid {field} {value:?}: {error}"))
}

fn parse_quantity(field: &'static str, value: &str) -> Result<Quantity> {
    Quantity::from_str(value).map_err(|error| anyhow!("invalid {field} {value:?}: {error}"))
}

fn validate_read_back_instrument_id(label: &str, instrument_id: &str) -> Result<()> {
    ensure!(
        !instrument_id.trim().is_empty(),
        "capability evidence must include the {label} synthetic instrument id"
    );
    ensure!(
        instrument_id == instrument_id.trim(),
        "capability evidence {label} synthetic instrument id must not include surrounding whitespace"
    );
    ensure!(
        !instrument_id.chars().any(char::is_whitespace),
        "capability evidence {label} synthetic instrument id must not contain whitespace"
    );
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogCapabilityEvidence {
    pub no_cloud_feature_gate_failed: bool,
    pub ambient_credentials_scrubbed: bool,
    pub invalid_credentials_write_failed: bool,
    pub ssm_credentials_write_reopen_query_succeeded: bool,
    pub nt_catalog_storage_option_keys: Vec<String>,
    pub read_back: NtCatalogReadBackEvidence,
    pub create_only_probe: CreateOnlyProbeTranscript,
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

    fn runtime_is_scrubbed(&self) -> bool {
        self.profile_file_paths_redirected
            && self.imds_blocked
            && self
                .unset_env_vars
                .iter()
                .all(|name| env::var_os(name).is_none())
    }
}

impl NtCatalogSyntheticFixtures {
    fn validate(&self) -> Result<()> {
        let (instruments, binary_option_instrument_id, perps_spot_instrument_id) =
            self.instruments()?;
        let instrument_ids = instruments
            .iter()
            .map(|instrument| instrument.id().to_string())
            .collect::<BTreeSet<_>>();
        ensure!(
            instrument_ids.contains(&binary_option_instrument_id),
            "synthetic fixtures must include the configured binary-option instrument"
        );
        ensure!(
            instrument_ids.contains(&perps_spot_instrument_id),
            "synthetic fixtures must include the configured perps-spot instrument"
        );
        let trade_ticks = self.trade_ticks()?;
        ensure!(
            !trade_ticks.is_empty(),
            "synthetic fixtures must include trade ticks"
        );
        for trade_tick in trade_ticks {
            ensure!(
                instrument_ids.contains(&trade_tick.instrument_id.to_string()),
                "synthetic trade tick instrument {} is not in synthetic instruments",
                trade_tick.instrument_id
            );
        }
        Ok(())
    }

    fn instruments(&self) -> Result<(Vec<InstrumentAny>, String, String)> {
        let binary_option = self.binary_option.instrument()?;
        let perps_spot = self.perps_spot.instrument()?;
        let binary_option_instrument_id = binary_option.id().to_string();
        let perps_spot_instrument_id = perps_spot.id().to_string();
        Ok((
            vec![
                InstrumentAny::BinaryOption(binary_option),
                InstrumentAny::CryptoPerpetual(perps_spot),
            ],
            binary_option_instrument_id,
            perps_spot_instrument_id,
        ))
    }

    fn trade_ticks(&self) -> Result<Vec<TradeTick>> {
        self.trade_ticks
            .iter()
            .map(NtCatalogSyntheticTradeTickSpec::trade_tick)
            .collect()
    }
}

impl NtCatalogSyntheticBinaryOptionSpec {
    fn instrument(&self) -> Result<BinaryOption> {
        let price_increment = parse_price(
            "synthetic_fixtures.binary_option.price_increment",
            &self.price_increment,
        )?;
        let size_increment = parse_quantity(
            "synthetic_fixtures.binary_option.size_increment",
            &self.size_increment,
        )?;
        BinaryOption::new_checked(
            parse_instrument_id(
                "synthetic_fixtures.binary_option.instrument_id",
                &self.instrument_id,
            )?,
            parse_symbol(
                "synthetic_fixtures.binary_option.raw_symbol",
                &self.raw_symbol,
            )?,
            AssetClass::from_str(&self.asset_class).with_context(|| {
                format!(
                    "invalid synthetic_fixtures.binary_option.asset_class {:?}",
                    self.asset_class
                )
            })?,
            parse_currency("synthetic_fixtures.binary_option.currency", &self.currency)?,
            UnixNanos::from(self.activation_ns),
            UnixNanos::from(self.expiration_ns),
            price_increment.precision,
            size_increment.precision,
            price_increment,
            size_increment,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // tick_scheme (NT bump)
            None,
            UnixNanos::default(),
            UnixNanos::default(),
        )
        .map_err(|error| anyhow!("invalid synthetic binary-option instrument: {error}"))
    }
}

impl NtCatalogSyntheticPerpsSpotSpec {
    fn instrument(&self) -> Result<CryptoPerpetual> {
        let price_increment = parse_price(
            "synthetic_fixtures.perps_spot.price_increment",
            &self.price_increment,
        )?;
        let size_increment = parse_quantity(
            "synthetic_fixtures.perps_spot.size_increment",
            &self.size_increment,
        )?;
        CryptoPerpetual::new_checked(
            parse_instrument_id(
                "synthetic_fixtures.perps_spot.instrument_id",
                &self.instrument_id,
            )?,
            parse_symbol("synthetic_fixtures.perps_spot.raw_symbol", &self.raw_symbol)?,
            parse_currency(
                "synthetic_fixtures.perps_spot.base_currency",
                &self.base_currency,
            )?,
            parse_currency(
                "synthetic_fixtures.perps_spot.quote_currency",
                &self.quote_currency,
            )?,
            parse_currency(
                "synthetic_fixtures.perps_spot.settlement_currency",
                &self.settlement_currency,
            )?,
            self.is_inverse,
            price_increment.precision,
            size_increment.precision,
            price_increment,
            size_increment,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None, // tick_scheme (NT bump)
            None,
            UnixNanos::default(),
            UnixNanos::default(),
        )
        .map_err(|error| anyhow!("invalid synthetic perps-spot instrument: {error}"))
    }
}

impl NtCatalogSyntheticTradeTickSpec {
    fn trade_tick(&self) -> Result<TradeTick> {
        TradeTick::new_checked(
            parse_instrument_id(
                "synthetic_fixtures.trade_ticks.instrument_id",
                &self.instrument_id,
            )?,
            parse_price("synthetic_fixtures.trade_ticks.price", &self.price)?,
            parse_quantity("synthetic_fixtures.trade_ticks.size", &self.size)?,
            AggressorSide::from_str(&self.aggressor_side).with_context(|| {
                format!(
                    "invalid synthetic_fixtures.trade_ticks.aggressor_side {:?}",
                    self.aggressor_side
                )
            })?,
            TradeId::from(self.trade_id.as_str()),
            UnixNanos::from(self.ts_event),
            UnixNanos::from(self.ts_init),
        )
        .map_err(|error| anyhow!("invalid synthetic trade tick {:?}: {error}", self.trade_id))
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
        self.synthetic_fixtures.validate()?;
        ensure_proof_artifact_object_name(&self.proof_artifact_object_name)?;
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
        let proof_artifact_uri = format!(
            "{synthetic_catalog_root_uri}{}",
            self.proof_artifact_object_name
        );
        artifact_root.object_path_for_uri(&proof_artifact_uri)?;
        let plan = NtCatalogCapabilityPlan {
            proof_run_id: self.proof_run_id.clone(),
            nt_revision: self.nt_revision.clone(),
            artifact_root_uri: artifact_root.artifact_root_uri().to_string(),
            synthetic_catalog_root_uri,
            proof_artifact_uri,
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
    /// Returns an error if the run spec, artifact store, credentials, or
    /// synthetic fixtures cannot produce a direct-S3 NT catalog conformance
    /// probe.
    pub fn s3_conformance_probe(
        &self,
        artifact_store: &ArtifactStoreConfig,
        credentials: &S3ArtifactStoreCredentials,
    ) -> Result<NtCatalogS3ConformanceProbe> {
        let plan = self.proof_plan(artifact_store)?;
        let storage_options =
            artifact_store.nt_catalog_storage_options_with_credentials(credentials)?;
        self.s3_conformance_probe_with_storage_options(
            plan.synthetic_catalog_root_uri,
            storage_options,
        )
    }

    /// Produce runtime capability evidence under the durable operator's shared
    /// cooperative work budget.
    ///
    /// # Errors
    ///
    /// Returns an error when any control, NT operation, or evidence-finalization
    /// step fails or crosses the configured wall deadline.
    pub fn runtime_evidence_guarded(
        &self,
        artifact_store: &ArtifactStoreConfig,
        credentials: &S3ArtifactStoreCredentials,
        create_only_probe: CreateOnlyProbeTranscript,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<NtCatalogCapabilityEvidence> {
        let plan = run_guarded_nt_catalog_step(work_budget, || self.proof_plan(artifact_store))?;
        let no_cloud_feature_gate_failed = guarded_expected_failure(work_budget, || {
            self.s3_conformance_probe_with_storage_options(
                plan.synthetic_catalog_root_uri.clone(),
                AHashMap::new(),
            )
            .map(|_| ())
        })?;
        let ambient_credentials_scrubbed = run_guarded_nt_catalog_step(work_budget, || {
            Ok(self.ambient_credential_scrub.runtime_is_scrubbed())
        })?;
        let invalid_credentials_write_failed =
            self.invalid_credentials_write_fails_guarded(artifact_store, credentials, work_budget)?;
        let read_back = run_nt_catalog_s3_conformance_probe_guarded(
            run_guarded_nt_catalog_step(work_budget, || {
                self.s3_conformance_probe(artifact_store, credentials)
            })?,
            work_budget,
        )?;
        let ssm_credentials_write_reopen_query_succeeded = read_back.write_instruments_succeeded
            && read_back.write_trade_ticks_succeeded
            && read_back.query_files_succeeded
            && read_back.query_instruments_succeeded
            && read_back.query_trade_ticks_succeeded;
        run_guarded_nt_catalog_step(work_budget, || {
            Ok(NtCatalogCapabilityEvidence {
                no_cloud_feature_gate_failed,
                ambient_credentials_scrubbed,
                invalid_credentials_write_failed,
                ssm_credentials_write_reopen_query_succeeded,
                nt_catalog_storage_option_keys: plan.storage_options_keys,
                read_back,
                create_only_probe,
            })
        })
    }

    fn invalid_credentials_write_fails_guarded(
        &self,
        artifact_store: &ArtifactStoreConfig,
        credentials: &S3ArtifactStoreCredentials,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<bool> {
        let invalid_credentials = run_guarded_nt_catalog_step(work_budget, || {
            S3ArtifactStoreCredentials::new(
                format!("{}-{}", credentials.access_key_id(), self.proof_run_id),
                format!("{}-{}", credentials.secret_access_key(), self.proof_run_id),
                credentials
                    .session_token()
                    .map(|token| format!("{token}-{}", self.proof_run_id)),
            )
        })?;
        let invalid_probe = run_guarded_nt_catalog_step(work_budget, || {
            self.s3_conformance_probe(artifact_store, &invalid_credentials)
        })?;
        let NtCatalogS3ConformanceProbe {
            catalog_uri,
            storage_options,
            instruments,
            ..
        } = invalid_probe;
        let catalog = run_guarded_nt_catalog_step(work_budget, || {
            ParquetDataCatalog::from_uri(&catalog_uri, Some(storage_options), None, None, None)
        })?;
        match guarded_nt_catalog_operation_outcome(work_budget, || {
            catalog.write_instruments(instruments)
        })? {
            Ok(_) => Ok(false),
            Err(_) => Ok(true),
        }
    }

    fn s3_conformance_probe_with_storage_options(
        &self,
        synthetic_catalog_root_uri: String,
        storage_options: AHashMap<String, String>,
    ) -> Result<NtCatalogS3ConformanceProbe> {
        let (instruments, binary_option_instrument_id, perps_spot_instrument_id) =
            self.synthetic_fixtures.instruments()?;
        let trade_ticks = self.synthetic_fixtures.trade_ticks()?;
        NtCatalogS3ConformanceProbe::new(
            synthetic_catalog_root_uri,
            storage_options,
            instruments,
            trade_ticks,
            binary_option_instrument_id,
            perps_spot_instrument_id,
        )
    }

    /// # Errors
    ///
    /// Returns an error unless the run spec and passed runtime controls prove a
    /// completed synthetic direct-S3 catalog capability run.
    fn completed_proof(
        &self,
        artifact_store: &ArtifactStoreConfig,
        controls: NtCatalogCapabilityControls,
    ) -> Result<NtCatalogCapabilityProof> {
        let artifact_root = artifact_store.resolve()?;
        let proof = self.proof_plan(artifact_store)?.completed_proof(controls);
        proof.validate(&artifact_root)?;
        Ok(proof)
    }

    /// # Errors
    ///
    /// Returns an error unless the run spec and observed runtime evidence prove
    /// a completed synthetic direct-S3 catalog capability run.
    pub fn completed_proof_from_evidence(
        &self,
        artifact_store: &ArtifactStoreConfig,
        evidence: &NtCatalogCapabilityEvidence,
    ) -> Result<NtCatalogCapabilityProof> {
        let controls = NtCatalogCapabilityControls::from_evidence(evidence)?;
        let proof = self.completed_proof(artifact_store, controls)?;
        ensure_read_back_catalog_uri_matches(
            &evidence.read_back.catalog_uri,
            &proof.synthetic_catalog_root_uri,
        )?;
        ensure_evidence_storage_options_match(
            &proof.storage_options_keys,
            &evidence.nt_catalog_storage_option_keys,
        )?;
        Ok(proof)
    }

    /// # Errors
    ///
    /// Returns an error if the run spec is invalid, the runtime evidence is
    /// incomplete, serialization fails, or create-only persistence fails.
    async fn persist_completed_proof(
        &self,
        artifact_store: &ArtifactStoreConfig,
        writer: &CreateOnlyArtifactWriter<'_>,
        evidence: &NtCatalogCapabilityEvidence,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<NtCatalogCapabilityProofArtifact> {
        let (
            proof,
            proof_artifact_uri,
            artifact_root,
            proof_bytes,
            proof_artifact_sha256,
            proof_evidence,
        ) = guarded_nt_catalog_operation_outcome(work_budget, || -> Result<_> {
            let artifact_root = artifact_store.resolve()?;
            let mut plan = self.proof_plan(artifact_store)?;
            let proof_artifact_uri = std::mem::take(&mut plan.proof_artifact_uri);
            let controls = NtCatalogCapabilityControls::from_evidence(evidence)?;
            let proof = plan.completed_proof(controls);
            validate_capability_proof_document_parts(&proof, evidence, &artifact_root)?;
            let proof_document = NtCatalogCapabilityProofDocumentRef {
                proof: &proof,
                evidence,
            };
            let serialized_bytes = serialized_json_len_with_budget(
                &proof_document,
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?;
            ensure!(
                serialized_bytes <= artifact_root.max_final_object_bytes(),
                "NT catalog capability proof byte length {serialized_bytes} exceeds artifact_store.max_final_object_bytes {}",
                artifact_root.max_final_object_bytes()
            );
            let proof_bytes = serialize_json_to_vec_guarded(
                &proof_document,
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?;
            ensure!(
                u64::try_from(proof_bytes.len())
                    .context("capability proof length does not fit u64")?
                    == serialized_bytes,
                "capability proof serialized length changed between preflight and allocation"
            );
            let proof_artifact_sha256 = sha256_hex_with_budget(
                &proof_bytes,
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?;
            let proof_evidence = clone_capability_evidence_guarded(
                evidence,
                u64::try_from(proof_bytes.capacity())
                    .context("capability proof allocation does not fit u64")?,
                work_budget,
                OperatorWorkBudgetStage::Publish,
            )?;
            Ok((
                proof,
                proof_artifact_uri,
                artifact_root,
                proof_bytes,
                proof_artifact_sha256,
                proof_evidence,
            ))
        })??;
        let prepared_proof = writer.prepare_terminal_create_uri(
            &artifact_root,
            &proof_artifact_uri,
            proof_bytes,
            format!("NT catalog capability proof {proof_artifact_uri}"),
        )?;
        let permit = work_budget.authorize_commit(OperatorWorkBudgetStage::Publish)?;
        let confirmed_proof = writer
            .create_or_confirm_terminal(prepared_proof, permit)
            .await?;
        Ok(NtCatalogCapabilityProofArtifact {
            proof_artifact_uri,
            proof_artifact_sha256,
            proof_artifact_version_id: confirmed_proof.version_id,
            proof_artifact_e_tag: confirmed_proof.e_tag,
            proof_artifact_create_only_write: confirmed_proof.disposition,
            proof,
            evidence: proof_evidence,
        })
    }

    /// Persist a completed proof under the shared operator deadline. The
    /// terminal proof write consumes a one-use commit permit and is never
    /// followed by a clock check in this protocol.
    ///
    /// # Errors
    ///
    /// Returns an error if evidence-derived controls, serialization,
    /// create-only persistence, or the explicit work budget fails.
    pub async fn persist_completed_proof_from_evidence_guarded(
        &self,
        artifact_store: &ArtifactStoreConfig,
        writer: &CreateOnlyArtifactWriter<'_>,
        evidence: &NtCatalogCapabilityEvidence,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<NtCatalogCapabilityProofArtifact> {
        self.persist_completed_proof(artifact_store, writer, evidence, work_budget)
            .await
    }
}

impl NtCatalogCapabilityProofDocument {
    /// # Errors
    ///
    /// Returns an error if the persisted proof document is incomplete, its
    /// evidence does not prove every control, or probe URIs leave the artifact
    /// root.
    pub fn validate(&self, artifact_root: &ResolvedArtifactRoot) -> Result<()> {
        validate_capability_proof_document_parts(&self.proof, &self.evidence, artifact_root)
    }
}

fn validate_capability_proof_document_parts(
    proof: &NtCatalogCapabilityProof,
    evidence: &NtCatalogCapabilityEvidence,
    artifact_root: &ResolvedArtifactRoot,
) -> Result<()> {
    proof.validate(artifact_root)?;
    let controls = NtCatalogCapabilityControls::from_evidence(evidence)?;
    ensure!(
        proof.controls == controls,
        "capability proof document controls must match observed evidence"
    );
    ensure_read_back_catalog_uri_matches(
        &evidence.read_back.catalog_uri,
        &proof.synthetic_catalog_root_uri,
    )?;
    ensure_evidence_storage_options_match(
        &proof.storage_options_keys,
        &evidence.nt_catalog_storage_option_keys,
    )?;
    artifact_root.object_path_for_uri(&evidence.create_only_probe.probe_uri)?;
    artifact_root.object_path_for_uri(&evidence.create_only_probe.copy_source_uri)?;
    artifact_root.object_path_for_uri(&evidence.create_only_probe.copy_dest_uri)?;
    Ok(())
}

fn clone_capability_string(value: &str, label: &str) -> Result<String> {
    let mut cloned = String::new();
    cloned
        .try_reserve_exact(value.len())
        .with_context(|| format!("reserve {label}"))?;
    cloned.push_str(value);
    Ok(cloned)
}

fn clone_capability_evidence_guarded(
    evidence: &NtCatalogCapabilityEvidence,
    retained_payload_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
    stage: OperatorWorkBudgetStage,
) -> Result<NtCatalogCapabilityEvidence> {
    let string_bytes = evidence
        .nt_catalog_storage_option_keys
        .iter()
        .map(String::len)
        .chain([
            evidence.read_back.catalog_uri.len(),
            evidence.read_back.binary_option_instrument_id.len(),
            evidence.read_back.perps_spot_instrument_id.len(),
            evidence.create_only_probe.probe_uri.len(),
            evidence.create_only_probe.copy_source_uri.len(),
            evidence.create_only_probe.copy_dest_uri.len(),
        ])
        .try_fold(0_u64, |total, bytes| {
            total.checked_add(u64::try_from(bytes).ok()?)
        })
        .context("capability evidence string allocation overflow")?;
    let string_slots = evidence
        .nt_catalog_storage_option_keys
        .len()
        .checked_mul(std::mem::size_of::<String>())
        .and_then(|bytes| u64::try_from(bytes).ok())
        .context("capability evidence vector allocation overflow")?;
    let retained_peak = retained_payload_bytes
        .checked_add(string_bytes)
        .and_then(|bytes| bytes.checked_add(string_slots))
        .context("capability proof retained allocation overflow")?;
    work_budget.verify_decoded_bytes(retained_peak, stage)?;
    work_budget.check_deadline(stage)?;

    let mut storage_keys = Vec::new();
    storage_keys
        .try_reserve_exact(evidence.nt_catalog_storage_option_keys.len())
        .context("reserve capability evidence storage-option keys")?;
    for key in &evidence.nt_catalog_storage_option_keys {
        storage_keys.push(clone_capability_string(
            key,
            "capability evidence storage-option key",
        )?);
    }
    let cloned = NtCatalogCapabilityEvidence {
        no_cloud_feature_gate_failed: evidence.no_cloud_feature_gate_failed,
        ambient_credentials_scrubbed: evidence.ambient_credentials_scrubbed,
        invalid_credentials_write_failed: evidence.invalid_credentials_write_failed,
        ssm_credentials_write_reopen_query_succeeded: evidence
            .ssm_credentials_write_reopen_query_succeeded,
        nt_catalog_storage_option_keys: storage_keys,
        read_back: NtCatalogReadBackEvidence {
            catalog_uri: clone_capability_string(
                &evidence.read_back.catalog_uri,
                "capability evidence catalog URI",
            )?,
            query_files_succeeded: evidence.read_back.query_files_succeeded,
            query_files_result_count: evidence.read_back.query_files_result_count,
            write_instruments_succeeded: evidence.read_back.write_instruments_succeeded,
            write_trade_ticks_succeeded: evidence.read_back.write_trade_ticks_succeeded,
            query_trade_ticks_succeeded: evidence.read_back.query_trade_ticks_succeeded,
            query_trade_ticks_result_count: evidence.read_back.query_trade_ticks_result_count,
            query_instruments_succeeded: evidence.read_back.query_instruments_succeeded,
            query_instruments_result_count: evidence.read_back.query_instruments_result_count,
            binary_option_instrument_read_back: evidence
                .read_back
                .binary_option_instrument_read_back,
            binary_option_instrument_id: clone_capability_string(
                &evidence.read_back.binary_option_instrument_id,
                "capability evidence binary-option instrument ID",
            )?,
            perps_spot_instrument_read_back: evidence.read_back.perps_spot_instrument_read_back,
            perps_spot_instrument_id: clone_capability_string(
                &evidence.read_back.perps_spot_instrument_id,
                "capability evidence perps-spot instrument ID",
            )?,
        },
        create_only_probe: CreateOnlyProbeTranscript {
            probe_uri: clone_capability_string(
                &evidence.create_only_probe.probe_uri,
                "capability evidence create-only probe URI",
            )?,
            copy_source_uri: clone_capability_string(
                &evidence.create_only_probe.copy_source_uri,
                "capability evidence copy source URI",
            )?,
            copy_dest_uri: clone_capability_string(
                &evidence.create_only_probe.copy_dest_uri,
                "capability evidence copy destination URI",
            )?,
            first_create_succeeded: evidence.create_only_probe.first_create_succeeded,
            duplicate_create_rejected: evidence.create_only_probe.duplicate_create_rejected,
            first_copy_succeeded: evidence.create_only_probe.first_copy_succeeded,
            duplicate_copy_rejected: evidence.create_only_probe.duplicate_copy_rejected,
        },
    };
    work_budget.check_deadline(stage)?;
    Ok(cloned)
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
        artifact_root.object_path_for_uri(&self.proof_artifact_uri)?;
        ensure!(
            self.proof_artifact_uri
                .starts_with(&self.synthetic_catalog_root_uri),
            "capability plan proof artifact URI must live under the synthetic catalog root"
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
    fn completed_proof(self, controls: NtCatalogCapabilityControls) -> NtCatalogCapabilityProof {
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

fn ensure_proof_artifact_object_name(value: &str) -> Result<()> {
    let trimmed = value.trim();
    ensure!(
        !trimmed.is_empty(),
        "proof_artifact_object_name must not be empty"
    );
    ensure!(
        trimmed == value,
        "proof_artifact_object_name must not contain leading or trailing whitespace"
    );
    ensure!(
        !value.contains("://"),
        "proof_artifact_object_name must be a relative object name"
    );
    ensure!(
        !value.contains('/'),
        "proof_artifact_object_name must not contain path separators"
    );
    ensure!(
        !matches!(value, "." | ".."),
        "proof_artifact_object_name must not be current or parent path"
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

fn ensure_nt_s3_storage_options(options: &AHashMap<String, String>) -> Result<()> {
    ensure!(
        option_is_nonblank(options, "region"),
        "NT S3 storage options must include region"
    );
    ensure!(
        option_is_nonblank(options, "access_key_id"),
        "NT S3 storage options must include access_key_id"
    );
    ensure!(
        option_is_nonblank(options, "secret_access_key"),
        "NT S3 storage options must include secret_access_key"
    );
    if let Some(session_token) = options.get("session_token") {
        ensure!(
            !session_token.trim().is_empty() && session_token.trim() == session_token,
            "NT S3 storage options session_token must not be blank or padded"
        );
    }
    Ok(())
}

fn option_is_nonblank(options: &AHashMap<String, String>, key: &str) -> bool {
    options
        .get(key)
        .is_some_and(|value| !value.trim().is_empty() && value.trim() == value)
}

fn ensure_sorted_storage_option_keys(keys: &[String]) -> Result<Vec<String>> {
    ensure_storage_option_keys(keys)?;
    let mut sorted = keys.to_vec();
    sorted.sort();
    Ok(sorted)
}

fn ensure_evidence_storage_options_match(
    proof_storage_option_keys: &[String],
    evidence_storage_option_keys: &[String],
) -> Result<()> {
    let expected = ensure_sorted_storage_option_keys(proof_storage_option_keys)?;
    let observed = ensure_sorted_storage_option_keys(evidence_storage_option_keys)?;
    ensure!(
        observed == expected,
        "capability evidence NT catalog storage option keys must match the proof plan"
    );
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use crate::{
        backfill_execution_plan::BackfillExecutionWorkBudget,
        operator_work_budget::{OperatorWorkBudget, OperatorWorkBudgetClock},
    };

    struct ObservationExpiryClock {
        observations: AtomicUsize,
        expires_after_observation: usize,
    }

    impl OperatorWorkBudgetClock for ObservationExpiryClock {
        fn now(&self) -> Duration {
            if self.observations.fetch_add(1, Ordering::SeqCst) >= self.expires_after_observation {
                Duration::from_secs(1)
            } else {
                Duration::ZERO
            }
        }
    }

    fn expiring_guard(expires_after_observation: usize) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::with_clock(
            OperatorWorkBudget::Backfill(BackfillExecutionWorkBudget {
                max_decoded_bytes: u64::MAX,
                max_source_rows: 1,
                max_projected_row_groups: 1,
                max_wall_seconds: 1,
                require_object_selection_metadata: false,
            }),
            Arc::new(ObservationExpiryClock {
                observations: AtomicUsize::new(0),
                expires_after_observation,
            }),
        )
        .expect("expiring guard")
    }

    #[test]
    fn nt_catalog_step_observes_expiry_after_the_opaque_operation() {
        let invoked = AtomicBool::new(false);
        let guard = expiring_guard(2);

        let error = run_guarded_nt_catalog_step(&guard, || {
            invoked.store(true, Ordering::SeqCst);
            Ok(())
        })
        .expect_err("post-operation deadline must fail the NT step");

        assert!(invoked.load(Ordering::SeqCst));
        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("publish"), "{error:#}");
    }

    #[test]
    fn negative_control_cannot_swallow_a_deadline_error() {
        let guard = expiring_guard(2);

        let error = guarded_expected_failure(&guard, || anyhow::bail!("expected probe failure"))
            .expect_err("deadline must win over an expected negative-control failure");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("publish"), "{error:#}");
    }

    #[test]
    fn invalid_credential_control_observes_only_the_dedicated_write() {
        let source = include_str!("nt_catalog_capability.rs");
        let body = source
            .split("fn invalid_credentials_write_fails_guarded(")
            .nth(1)
            .expect("dedicated invalid-credential function exists")
            .split("fn s3_conformance_probe_with_storage_options(")
            .next()
            .expect("function has a following boundary");

        assert!(body.contains("catalog.write_instruments(instruments)"));
        assert!(!body.contains("run_nt_catalog_s3_conformance_probe"));
        assert!(!body.contains("write_to_parquet"));
        assert!(!body.contains("query_files"));
        assert!(!body.contains("query_instruments"));
        assert!(!body.contains("query_typed_data"));
    }
}
