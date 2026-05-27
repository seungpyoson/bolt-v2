//! Per-provider binding root for bolt-v3 client config block shapes
//! and per-client startup-validation policy.
//!
//! Core config in `crate::bolt_v3_config` owns the root and strategy
//! envelopes plus NT venue identifiers. Concrete NT venue literals and
//! `[clients.<name>.{data,execution,secrets}]` block shapes live in
//! per-provider binding modules under this root.
//!
//! This module also owns the family-agnostic dispatch surface that
//! core startup validation in `crate::bolt_v3_validate` calls into:
//! every `[clients.<id>]` block is routed here, the NT venue is read
//! once, and the matching per-provider
//! validator owns the rest of the structural venue-shape rules.
//! Provider-neutral helpers used by more than one provider validator
//! (today: `crate::bolt_v3_validate::validate_ssm_parameter_path`)
//! stay in core and are called from the per-provider modules.

pub mod binance;
pub mod polymarket;

use std::{any::Any, fmt, future::Future, pin::Pin, sync::Arc};

const EXTERNAL_SNAPSHOT_NO_REMAINING_RETRIES: u64 = 0;
const EXTERNAL_SNAPSHOT_RETRY_DECREMENT: u64 = 1;

use crate::{
    bolt_v3_adapters::{BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3MarketClockFn},
    bolt_v3_config::{BoltV3RootConfig, ClientBlock, LoadedBoltV3Config},
    bolt_v3_market_families::MarketIdentityPlan,
    bolt_v3_operator_artifacts::{
        BoltV3OperatorArtifactError, EntryDecisionSourceCollectionRequest,
        EntryDecisionSourceInputsWritten,
    },
    bolt_v3_secrets::{BoltV3SecretError, ResolvedBoltV3Secrets},
    strategies::registry::FeeProvider,
};

pub trait ProviderResolvedSecrets: fmt::Debug + Send + Sync {
    fn provider_key(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
    fn redaction_values(&self) -> Vec<&str> {
        Vec::new()
    }
}

pub type ResolvedClientSecrets = Arc<dyn ProviderResolvedSecrets>;

pub trait SsmSecretResolver {
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String>;
}

impl<F, E> SsmSecretResolver for F
where
    F: FnMut(&str, &str) -> Result<String, E>,
    E: fmt::Display,
{
    fn resolve_secret(&mut self, region: &str, ssm_path: &str) -> Result<String, String> {
        self(region, ssm_path).map_err(|error| error.to_string())
    }
}

pub struct ProviderSecretResolveContext<'a> {
    pub client_key: &'a str,
    pub region: &'a str,
    pub client: &'a ClientBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSsmPathReference {
    pub field_name: &'static str,
    pub ssm_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateProviderEvidenceBinding {
    pub provider_id: String,
    pub provider_kind: String,
    pub capabilities: Vec<String>,
    pub max_age_ms: u64,
    pub max_clock_skew_ms: u64,
}

pub fn gate_provider_evidence_binding(
    loaded: &LoadedBoltV3Config,
    provider_id: &str,
) -> Result<GateProviderEvidenceBinding, BoltV3OperatorArtifactError> {
    let provider = loaded
        .root
        .gate_providers
        .as_ref()
        .and_then(|providers| providers.get(provider_id))
        .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "provider_id",
        })?;
    let provider_kind = provider.provider_kind.as_deref().ok_or(
        BoltV3OperatorArtifactError::GateEvidenceInvalid {
            field: "provider_kind",
        },
    )?;
    let capabilities =
        provider
            .capabilities
            .as_ref()
            .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid {
                field: "capabilities",
            })?;
    let freshness = provider
        .freshness
        .as_ref()
        .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid { field: "freshness" })?;
    let max_age_ms =
        freshness
            .max_age_ms
            .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid {
                field: "freshness.max_age_ms",
            })?;
    let max_clock_skew_ms =
        freshness
            .max_clock_skew_ms
            .ok_or(BoltV3OperatorArtifactError::GateEvidenceInvalid {
                field: "freshness.max_clock_skew_ms",
            })?;
    Ok(GateProviderEvidenceBinding {
        provider_id: provider_id.to_string(),
        provider_kind: provider_kind.to_string(),
        capabilities: capabilities.clone(),
        max_age_ms,
        max_clock_skew_ms,
    })
}

pub struct ProviderAdapterMapContext<'a> {
    pub root: &'a BoltV3RootConfig,
    pub client_key: &'a str,
    pub client: &'a ClientBlock,
    pub resolved: &'a ResolvedBoltV3Secrets,
    pub plan: &'a MarketIdentityPlan,
    pub clock: BoltV3MarketClockFn,
}

type FeeProviderBuilder = fn(
    &str,
    &ClientBlock,
    &ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, BoltV3AdapterMappingError>;

pub struct EntryDecisionSourceProviderContext<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy_instance_id: &'a str,
    pub request: EntryDecisionSourceCollectionRequest<'a>,
}

pub type EntryDecisionSourceInputCollector = for<'a> fn(
    EntryDecisionSourceProviderContext<'a>,
) -> Pin<
    Box<
        dyn Future<Output = Result<EntryDecisionSourceInputsWritten, BoltV3OperatorArtifactError>>
            + 'a,
    >,
>;

#[derive(Clone, Copy)]
pub struct ClobV2AdapterSigningSourceMaterializationRequest<'a> {
    pub schema_version: u32,
    pub domain_requirements_record_kind: &'static str,
    pub signed_order_fixture_record_kind: &'static str,
    pub signature_verification_record_kind: &'static str,
    pub clob_signing_version: &'a str,
    pub clob_signing_source_sha256: &'a str,
    pub clob_signing_source: &'a str,
}

pub struct ClobV2AdapterSigningSourceMaterialization {
    pub domain_requirements_sha256: String,
    pub signed_order_fixture_sha256: String,
    pub signature_verification_sha256: String,
    pub signer_recovered_matches_expected: bool,
}

#[derive(Clone, Copy)]
pub struct ClobV2FeeBehaviorSourceMaterializationRequest<'a> {
    pub schema_version: u32,
    pub nt_execution_parse_source: &'a str,
    pub nt_http_parse_source: &'a str,
}

pub struct ClobV2FeeBehaviorSourceMaterialization {
    pub maker_zero_fee_verified: bool,
    pub taker_fee_schedule_verified: bool,
    pub market_buy_fee_adjustment_verified: bool,
    pub price: String,
    pub fee_rate: String,
    pub fee_behavior_source_sha256: String,
    pub fee_assumptions_sha256: String,
}

pub struct ClobV2CollateralAccountingSourceMaterializationRequest<'a> {
    pub schema_version: u32,
    pub balance_allowance_record_kind: &'static str,
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy_instance_id: &'a str,
    pub resolved: &'a ResolvedBoltV3Secrets,
}

pub struct ClobV2CollateralAccountingSourceMaterialization {
    pub p_usd_balance: String,
    pub p_usd_allowance: String,
    pub collateral_accounting_source_sha256: String,
    pub(crate) confirmation_policy: ExternalSnapshotConfirmationPolicy,
}

pub struct ClobV2BalanceAllowanceCacheSyncRequest<'a> {
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy_instance_id: &'a str,
    pub resolved: &'a ResolvedBoltV3Secrets,
}

pub struct ClobV2BalanceAllowanceCacheSync {
    pub execution_client_id: String,
    pub request_path: &'static str,
    pub base_url_http_sha256: String,
}

pub struct VenueAccountStateSourceMaterializationRequest<'a> {
    pub schema_version: u32,
    pub account_state_snapshot_record_kind: &'static str,
    pub loaded: &'a LoadedBoltV3Config,
    pub strategy_instance_id: &'a str,
    pub configured_target_id: &'a str,
    pub resolved: &'a ResolvedBoltV3Secrets,
}

pub struct VenueAccountStateSourceMaterialization {
    pub open_order_count: u64,
    pub open_position_count: u64,
    pub account_state_snapshot_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExternalSnapshotConfirmationPolicy {
    pub max_retries: u64,
    pub retry_delay_initial_ms: u64,
    pub retry_delay_max_ms: u64,
}

impl ExternalSnapshotConfirmationPolicy {
    pub(crate) fn from_retry_fields(
        max_retries: u64,
        retry_delay_initial_ms: u64,
        retry_delay_max_ms: u64,
    ) -> Self {
        Self {
            max_retries,
            retry_delay_initial_ms,
            retry_delay_max_ms,
        }
    }

    fn retry_delay_ms(self) -> u64 {
        self.retry_delay_initial_ms.min(self.retry_delay_max_ms)
    }
}

pub(crate) async fn fetch_external_snapshot_with_retries<T, E, Fetch, Fut>(
    policy: ExternalSnapshotConfirmationPolicy,
    mut fetch: Fetch,
) -> Result<T, E>
where
    Fetch: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut result = fetch().await;
    let mut remaining_retries = policy.max_retries;
    while result.is_err() && remaining_retries != EXTERNAL_SNAPSHOT_NO_REMAINING_RETRIES {
        sleep_external_snapshot_confirmation_delay(policy).await;
        result = fetch().await;
        remaining_retries -= EXTERNAL_SNAPSHOT_RETRY_DECREMENT;
    }
    result
}

pub(crate) async fn confirm_external_snapshot_before_hard_stop<T, E, Fetch, Fut, IsBlocking>(
    mut snapshot: T,
    policy: ExternalSnapshotConfirmationPolicy,
    mut fetch: Fetch,
    is_blocking: IsBlocking,
) -> T
where
    Fetch: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    IsBlocking: Fn(&T) -> bool,
{
    let mut remaining_retries = policy.max_retries;
    while is_blocking(&snapshot) && remaining_retries != EXTERNAL_SNAPSHOT_NO_REMAINING_RETRIES {
        sleep_external_snapshot_confirmation_delay(policy).await;
        match fetch().await {
            Ok(confirmed_snapshot) => {
                snapshot = confirmed_snapshot;
            }
            Err(_) => break,
        }
        remaining_retries -= EXTERNAL_SNAPSHOT_RETRY_DECREMENT;
    }
    snapshot
}

async fn sleep_external_snapshot_confirmation_delay(policy: ExternalSnapshotConfirmationPolicy) {
    let retry_delay_ms = policy.retry_delay_ms();
    if retry_delay_ms != 0 {
        tokio::time::sleep(std::time::Duration::from_millis(retry_delay_ms)).await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderCredentialedBlock {
    Data,
    Execution,
}

impl ProviderCredentialedBlock {
    fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Execution => "execution",
        }
    }

    fn is_present(self, client: &ClientBlock) -> bool {
        match self {
            Self::Data => client.data.is_some(),
            Self::Execution => client.execution.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderSecretRequirement {
    pub block: ProviderCredentialedBlock,
    pub consumer: &'static str,
}

pub struct ProviderBinding {
    pub key: &'static str,
    pub validate_client: fn(&str, &ClientBlock) -> Vec<String>,
    pub supported_market_families: &'static [&'static str],
    pub required_secret_blocks: &'static [ProviderSecretRequirement],
    pub secret_field_names: &'static [&'static str],
    pub credential_log_modules: &'static [&'static str],
    pub forbidden_env_vars: &'static [&'static str],
    pub resolve_secrets: for<'a> fn(
        ProviderSecretResolveContext<'a>,
        &mut dyn SsmSecretResolver,
    ) -> Result<ResolvedClientSecrets, BoltV3SecretError>,
    pub configured_secret_paths:
        for<'a> fn(
            ProviderSecretResolveContext<'a>,
        ) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError>,
    pub map_adapters: for<'a> fn(
        ProviderAdapterMapContext<'a>,
    )
        -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError>,
    pub build_fee_provider: Option<FeeProviderBuilder>,
    pub collect_entry_decision_source_inputs: Option<EntryDecisionSourceInputCollector>,
}

const PROVIDER_BINDINGS: &[ProviderBinding] = &[
    ProviderBinding {
        key: polymarket::KEY,
        validate_client: polymarket::validate_client,
        supported_market_families: polymarket::SUPPORTED_MARKET_FAMILIES,
        required_secret_blocks: polymarket::REQUIRED_SECRET_BLOCKS,
        secret_field_names: polymarket::SECRET_FIELD_NAMES,
        credential_log_modules: polymarket::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: polymarket::FORBIDDEN_ENV_VARS,
        resolve_secrets: polymarket::resolve_secrets,
        configured_secret_paths: polymarket::configured_secret_paths,
        map_adapters: polymarket::map_adapters,
        build_fee_provider: Some(polymarket::build_fee_provider),
        collect_entry_decision_source_inputs: Some(
            polymarket::collect_entry_decision_source_inputs,
        ),
    },
    ProviderBinding {
        key: binance::KEY,
        validate_client: binance::validate_client,
        supported_market_families: binance::SUPPORTED_MARKET_FAMILIES,
        required_secret_blocks: binance::REQUIRED_SECRET_BLOCKS,
        secret_field_names: binance::SECRET_FIELD_NAMES,
        credential_log_modules: binance::CREDENTIAL_LOG_MODULES,
        forbidden_env_vars: binance::FORBIDDEN_ENV_VARS,
        resolve_secrets: binance::resolve_secrets,
        configured_secret_paths: binance::configured_secret_paths,
        map_adapters: binance::map_adapters,
        build_fee_provider: None,
        collect_entry_decision_source_inputs: None,
    },
];

pub fn provider_bindings() -> &'static [ProviderBinding] {
    PROVIDER_BINDINGS
}

pub fn binding_for_provider_key(key: &str) -> Option<&'static ProviderBinding> {
    provider_bindings()
        .iter()
        .find(|binding| binding.key == key)
}

pub fn materialize_clob_v2_adapter_signing_source_from_nt_signing_source(
    request: ClobV2AdapterSigningSourceMaterializationRequest<'_>,
) -> Result<ClobV2AdapterSigningSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_clob_v2_adapter_signing_source_from_nt_signing_source(request)
}

pub fn materialize_clob_v2_fee_behavior_source_from_nt_fee_sources(
    request: ClobV2FeeBehaviorSourceMaterializationRequest<'_>,
) -> Result<ClobV2FeeBehaviorSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_clob_v2_fee_behavior_source_from_nt_fee_sources(request)
}

pub async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance(
        request,
    )
    .await
}

pub(crate) async fn materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_once(
    request: ClobV2CollateralAccountingSourceMaterializationRequest<'_>,
) -> Result<ClobV2CollateralAccountingSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance_once(
        request,
    )
    .await
}

pub async fn sync_clob_v2_balance_allowance_cache_from_configured_account(
    request: ClobV2BalanceAllowanceCacheSyncRequest<'_>,
) -> Result<ClobV2BalanceAllowanceCacheSync, BoltV3OperatorArtifactError> {
    polymarket::sync_clob_v2_balance_allowance_cache_from_configured_account(request).await
}

pub async fn materialize_venue_account_state_source_from_configured_account_queries(
    request: VenueAccountStateSourceMaterializationRequest<'_>,
) -> Result<VenueAccountStateSourceMaterialization, BoltV3OperatorArtifactError> {
    polymarket::materialize_venue_account_state_source_from_configured_account_queries(request)
        .await
}

/// Provider-owned NT adapter modules whose info logs can expose
/// credential metadata. The live-node builder consumes this provider
/// binding surface to install `WARN` module filters without hardcoding
/// concrete provider module paths in the live-node assembly layer.
pub fn credential_log_modules() -> impl Iterator<Item = &'static str> {
    provider_bindings()
        .iter()
        .flat_map(|binding| binding.credential_log_modules.iter().copied())
}

/// Family-agnostic surface read by core startup validation. Routes
/// each client block to its per-provider validator based on provider
/// key. Returns the full error list for the client block.
pub fn validate_client_block(key: &str, client: &ClientBlock) -> Vec<String> {
    match binding_for_provider_key(client.venue.as_str()) {
        Some(binding) => {
            let mut errors = validate_required_secret_blocks(
                key,
                binding.key,
                client,
                binding.required_secret_blocks,
            );
            errors.extend((binding.validate_client)(key, client));
            errors
        }
        None => vec![format!(
            "clients.{key}.venue `{}` is not supported by this build",
            client.venue.as_str()
        )],
    }
}

#[derive(Debug)]
pub enum FeeProviderResolutionError {
    MissingExecutionClient {
        execution_client_id: String,
    },
    UnsupportedProvider {
        client_key: String,
        provider_key: String,
    },
    ProviderWithoutFeeBinding {
        client_key: String,
        provider_key: String,
    },
    ProviderBuild {
        client_key: String,
        provider_key: String,
        source: BoltV3AdapterMappingError,
    },
}

impl std::fmt::Display for FeeProviderResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingExecutionClient {
                execution_client_id,
            } => write!(
                f,
                "strategy execution_client_id `{execution_client_id}` is not present in loaded clients",
            ),
            Self::UnsupportedProvider {
                client_key,
                provider_key,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` has no registered provider binding",
            ),
            Self::ProviderWithoutFeeBinding {
                client_key,
                provider_key,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` has no registered fee-provider binding",
            ),
            Self::ProviderBuild {
                client_key,
                provider_key,
                source,
            } => write!(
                f,
                "clients.{client_key}.venue `{provider_key}` fee-provider construction failed: {source}",
            ),
        }
    }
}

impl std::error::Error for FeeProviderResolutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ProviderBuild { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn resolve_fee_provider(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, FeeProviderResolutionError> {
    let client = loaded
        .root
        .clients
        .get(execution_client_id)
        .ok_or_else(|| FeeProviderResolutionError::MissingExecutionClient {
            execution_client_id: execution_client_id.to_string(),
        })?;
    let provider_key = client.venue.as_str();
    let binding = binding_for_provider_key(provider_key).ok_or_else(|| {
        FeeProviderResolutionError::UnsupportedProvider {
            client_key: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    let build_fee_provider = binding.build_fee_provider.ok_or_else(|| {
        FeeProviderResolutionError::ProviderWithoutFeeBinding {
            client_key: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
        }
    })?;
    build_fee_provider(execution_client_id, client, resolved).map_err(|source| {
        FeeProviderResolutionError::ProviderBuild {
            client_key: execution_client_id.to_string(),
            provider_key: provider_key.to_string(),
            source,
        }
    })
}

fn validate_required_secret_blocks(
    key: &str,
    provider_key: &str,
    client: &ClientBlock,
    requirements: &[ProviderSecretRequirement],
) -> Vec<String> {
    let mut errors = Vec::new();
    if client.secrets.is_some() {
        return errors;
    }
    for requirement in requirements {
        if requirement.block.is_present(client) {
            errors.push(format!(
                "clients.{key} (provider={provider_key}) declares [{}] but is missing the required [secrets] block; \
                 the bolt-v3 secret contract requires SSM credential resolution for every {}",
                requirement.block.as_str(),
                requirement.consumer
            ));
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bolt_v3_config::LoadedBoltV3Config,
        bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, resolve_bolt_v3_secrets_with},
    };
    use std::{collections::BTreeMap, path::PathBuf};

    fn client_from_toml(text: &str) -> ClientBlock {
        toml::from_str(text).expect("test client should parse")
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: "test-config-bundle-checksum".to_string(),
            root: toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root should parse"),
            strategies: Vec::new(),
        }
    }

    fn fake_secret_value(path: &str) -> String {
        match path {
            "/bolt/polymarket_main/private_key" => {
                "0x1111111111111111111111111111111111111111111111111111111111111111".to_string()
            }
            "/bolt/polymarket_main/api_key" => "poly-api-key".to_string(),
            "/bolt/polymarket_main/api_secret" => "YWJj".to_string(),
            "/bolt/polymarket_main/passphrase" => "poly-passphrase".to_string(),
            _ => panic!("unexpected test SSM path {path}"),
        }
    }

    fn resolved_polymarket_secrets() -> ResolvedBoltV3Secrets {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .clients
            .retain(|client_key, _| client_key == "polymarket_main");
        resolve_bolt_v3_secrets_with(&loaded, |_region, path| {
            Ok::<_, String>(fake_secret_value(path))
        })
        .expect("fixture polymarket secrets should resolve")
    }

    fn set_polymarket_execution_field(
        loaded: &mut LoadedBoltV3Config,
        field: &'static str,
        value: toml::Value,
    ) {
        let execution = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .and_then(|client| client.execution.as_mut())
            .expect("fixture should include polymarket execution block");
        let table = execution
            .as_table_mut()
            .expect("polymarket execution should be a table");
        table.insert(field.to_string(), value);
    }

    fn expect_resolution_error(
        result: Result<Arc<dyn FeeProvider>, FeeProviderResolutionError>,
        message: &'static str,
    ) -> FeeProviderResolutionError {
        match result {
            Ok(_) => panic!("{message}"),
            Err(error) => error,
        }
    }

    #[derive(Debug)]
    struct SentinelSecrets {
        value: String,
    }

    impl ProviderResolvedSecrets for SentinelSecrets {
        fn provider_key(&self) -> &'static str {
            "SENTINEL"
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        fn redaction_values(&self) -> Vec<&str> {
            vec![self.value.as_str()]
        }
    }

    #[test]
    fn credential_log_modules_are_provider_owned() {
        let polymarket = binding_for_provider_key(polymarket::KEY)
            .expect("Polymarket binding must be registered");
        assert_eq!(
            polymarket.credential_log_modules,
            polymarket::CREDENTIAL_LOG_MODULES
        );

        let binance =
            binding_for_provider_key(binance::KEY).expect("Binance binding must be registered");
        assert_eq!(
            binance.credential_log_modules,
            binance::CREDENTIAL_LOG_MODULES
        );
    }

    #[test]
    fn provider_required_secrets_rejects_credentialed_block_without_secrets() {
        let client = client_from_toml(
            r#"
            venue = "FAKE"

            [data]
            "#,
        );
        let requirement = ProviderSecretRequirement {
            block: ProviderCredentialedBlock::Data,
            consumer: "fake data adapter",
        };

        let errors =
            validate_required_secret_blocks("fake_client", "FAKE", &client, &[requirement]);

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("declares [data] but is missing the required [secrets] block"));
        assert!(errors[0].contains("fake data adapter"));
    }

    #[test]
    fn provider_required_secrets_ignores_absent_credentialed_block() {
        let client = client_from_toml(
            r#"
            venue = "FAKE"
            "#,
        );
        let requirement = ProviderSecretRequirement {
            block: ProviderCredentialedBlock::Execution,
            consumer: "fake execution adapter",
        };

        let errors =
            validate_required_secret_blocks("fake_client", "FAKE", &client, &[requirement]);

        assert!(errors.is_empty());
    }

    #[test]
    fn fee_provider_resolution_uses_provider_binding_registry() {
        let loaded = fixture_loaded_config();
        let resolved = resolved_polymarket_secrets();

        let provider = resolve_fee_provider(&loaded, "polymarket_main", &resolved)
            .expect("polymarket fee provider should resolve through provider binding");

        assert!(
            binding_for_provider_key(polymarket::KEY)
                .expect("polymarket binding should exist")
                .build_fee_provider
                .is_some(),
            "polymarket binding should expose a fee-provider capability"
        );
        assert!(
            provider
                .fee_bps("condition-token.POLYMARKET".into())
                .is_none()
        );
    }

    #[test]
    fn fee_provider_resolution_rejects_missing_execution_client_id() {
        let loaded = fixture_loaded_config();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "missing_execution_client", &resolved),
            "missing execution client id must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::MissingExecutionClient { .. }
        ));
        assert!(error.to_string().contains("missing_execution_client"));
    }

    #[test]
    fn fee_provider_resolution_rejects_unsupported_provider_kind() {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.insert(
            "fake_execution".to_string(),
            client_from_toml(
                r#"
                venue = "FAKE"
                "#,
            ),
        );
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "fake_execution", &resolved),
            "unsupported provider key must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::UnsupportedProvider { .. }
        ));
        assert!(error.to_string().contains("FAKE"));
    }

    #[test]
    fn fee_provider_resolution_rejects_provider_without_fee_binding() {
        let loaded = fixture_loaded_config();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "binance_reference", &resolved),
            "provider without fee binding must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderWithoutFeeBinding { .. }
        ));
        assert!(error.to_string().contains("BINANCE"));
    }

    #[test]
    fn fee_provider_resolution_reports_provider_config_parse_failure() {
        let mut loaded = fixture_loaded_config();
        set_polymarket_execution_field(
            &mut loaded,
            "fee_cache_ttl_secs",
            toml::Value::String("not-an-integer".to_string()),
        );
        let resolved = resolved_polymarket_secrets();

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "polymarket_main", &resolved),
            "provider config parse failure must surface through resolver",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild {
                source: BoltV3AdapterMappingError::SchemaParse { .. },
                ..
            }
        ));
        assert!(error.to_string().contains("failed to deserialize"));
    }

    #[test]
    fn fee_provider_resolution_rejects_invalid_secret_binding() {
        let loaded = fixture_loaded_config();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "polymarket_main", &resolved),
            "missing resolved secret binding must fail at resolver boundary",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild {
                source: BoltV3AdapterMappingError::MissingResolvedSecrets { .. },
                ..
            }
        ));
    }

    #[test]
    fn fee_provider_resolution_reports_provider_client_construction_failure() {
        let mut loaded = fixture_loaded_config();
        set_polymarket_execution_field(
            &mut loaded,
            "base_url_http",
            toml::Value::String("not a url".to_string()),
        );
        let resolved = resolved_polymarket_secrets();

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "polymarket_main", &resolved),
            "provider client construction failure must surface through resolver",
        );

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild {
                source: BoltV3AdapterMappingError::ValidationInvariant { .. },
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .contains("failed to create Polymarket fee HTTP client")
        );
    }

    #[test]
    fn fee_provider_resolution_error_display_debug_redacts_sentinel_secret() {
        let loaded = fixture_loaded_config();
        let sentinel = "sentinel-secret-value-453";
        let mut clients = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(SentinelSecrets {
                value: sentinel.to_string(),
            }) as ResolvedBoltV3ClientSecrets,
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "polymarket_main", &resolved),
            "sentinel secret provider mismatch must fail without leaking secret value",
        );
        let display = error.to_string();
        let debug = format!("{error:?}");

        assert!(!display.contains(sentinel), "{display}");
        assert!(!debug.contains(sentinel), "{debug}");
    }

    #[test]
    fn fee_provider_resolution_redacts_provider_build_secret_errors() {
        let loaded = fixture_loaded_config();
        let sentinel = "sentinel-secret-value-453";
        let mut clients = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(polymarket::ResolvedBoltV3PolymarketSecrets {
                private_key: sentinel.to_string(),
                api_key: "poly-api-key".to_string(),
                api_secret: "YWJj".to_string(),
                passphrase: "poly-passphrase".to_string(),
            }) as ResolvedBoltV3ClientSecrets,
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = expect_resolution_error(
            resolve_fee_provider(&loaded, "polymarket_main", &resolved),
            "provider build failure must not leak malformed resolved secret values",
        );
        let display = error.to_string();
        let debug = format!("{error:?}");

        assert!(matches!(
            error,
            FeeProviderResolutionError::ProviderBuild { .. }
        ));
        assert!(!display.contains(sentinel), "{display}");
        assert!(!debug.contains(sentinel), "{debug}");
    }
}
