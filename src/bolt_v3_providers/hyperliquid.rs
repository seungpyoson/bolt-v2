//! Fail-closed provider binding for `HYPERLIQUID`.
//!
//! This module registers the provider key and NT crate boundary. Execution
//! mapping stays gated behind SSM-resolved credentials, explicit TOML runtime
//! fields, and a consumed live-submit approval for the standard-perps surface.

use std::{any::Any, sync::Arc};

use nautilus_core::string::secret::REDACTED;
use nautilus_hyperliquid::{
    common::enums::HyperliquidEnvironment as NtHyperliquidEnvironment,
    config::HyperliquidExecClientConfig,
    factories::{HyperliquidExecFactoryConfig, HyperliquidExecutionClientFactory},
};
use nautilus_model::identifiers::AccountId;
use nautilus_network::websocket::TransportBackend;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::bolt_v3_providers::{ProviderExclusiveSignerOwner, ProviderResolvedSecrets};
use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3ExecutionClientAdapterConfig,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderSecretRequirement,
        ProviderSecretResolveContext, ProviderSsmPathReference, ResolvedClientSecrets,
        SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
};

use super::hyperliquid_artifacts::HyperliquidLiveSubmitApprovalConsumption;

pub const KEY: &str = "HYPERLIQUID";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Execution,
    consumer: "Hyperliquid execution client",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &[
    "private_key_ssm_path",
    "account_address_ssm_path",
    "vault_address_ssm_path",
];
const RAW_SECRET_FIELD_NAMES: &[&str] = &["private_key", "account_address", "vault_address"];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_hyperliquid::common::credential"];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "HYPERLIQUID_PK",
    "HYPERLIQUID_TESTNET_PK",
    "HYPERLIQUID_VAULT",
    "HYPERLIQUID_TESTNET_VAULT",
    "HYPERLIQUID_ACCOUNT_ADDRESS",
];

fn deserialize_account_id<'de, D>(deserializer: D) -> Result<AccountId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: String = String::deserialize(deserializer)?;
    AccountId::new_checked(value.as_str()).map_err(serde::de::Error::custom)
}

#[allow(dead_code)]
fn _credential_log_module_path_exists(
    _private_key: &nautilus_hyperliquid::common::credential::EvmPrivateKey,
) {
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HyperliquidExecutionConfig {
    #[serde(deserialize_with = "deserialize_account_id")]
    pub account_id: AccountId,
    pub environment: HyperliquidEnvironment,
    pub execution_mode: HyperliquidExecutionMode,
    pub product_surfaces: Vec<HyperliquidProductSurface>,
    pub live_submit_approval_id: Option<String>,
    pub base_url_ws: String,
    pub base_url_http: String,
    pub base_url_exchange: String,
    pub proxy_url: Option<String>,
    pub http_timeout_secs: u64,
    pub max_retries: u64,
    pub retry_delay_initial_ms: u64,
    pub retry_delay_max_ms: u64,
    pub normalize_prices: bool,
    pub market_order_slippage_bps: u32,
    pub transport_backend: TransportBackend,
    pub ws_post_timeout_secs: u64,
    pub outcome_settlement_poll_secs: u64,
    pub latency_profile: Option<HyperliquidLatencyProfileConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HyperliquidLatencyProfileConfig {
    pub local_info_node_url: String,
    pub placement_profile: String,
    pub measurement_artifact_path: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidEnvironment {
    Mainnet,
    Testnet,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidExecutionMode {
    DirectAccount,
    Vault,
    MasterAccountApiWallet,
    SubaccountApiWallet,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidProductSurface {
    StandardPerps,
    Spot,
    Hip3BuilderPerps,
    Hip4Outcomes,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidDiscoveryStatus {
    Supported,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidSubmitStatus {
    FailClosed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct HyperliquidProductMatrixEntry {
    pub provider_key: &'static str,
    pub product_surface: HyperliquidProductSurface,
    pub discovery_status: HyperliquidDiscoveryStatus,
    pub discovery_sources: &'static [&'static str],
    pub official_documentation_sources: &'static [&'static str],
    pub live_submit_status: HyperliquidSubmitStatus,
    pub missing_submit_proof: &'static [&'static str],
}

const STANDARD_PERPS_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::meta",
    "nautilus_hyperliquid::http::models::PerpMeta",
    "nautilus_hyperliquid::http::parse::parse_perp_instruments",
];
const STANDARD_PERPS_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint perpetuals metadata `meta`"];
const STANDARD_PERPS_MISSING_SUBMIT_PROOF: &[&str] = &[
    "standard perps live-submit approval artifact",
    "standard perps userFees rate-limit policy reconciliation",
];
const SPOT_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::spot_meta",
    "nautilus_hyperliquid::http::models::SpotMeta",
    "nautilus_hyperliquid::http::parse::parse_spot_instruments",
];
const SPOT_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint spot metadata `spotMeta`"];
const SPOT_MISSING_SUBMIT_PROOF: &[&str] = &["spot order/fill/rounding/fee proof"];
const HIP3_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::all_perp_metas",
    "nautilus_hyperliquid::http::models::PerpMeta",
    "nautilus_hyperliquid::http::parse::parse_perp_instruments",
];
const HIP3_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint all perp dex metadata `allPerpMetas`"];
const HIP3_MISSING_SUBMIT_PROOF: &[&str] = &["HIP-3 asset-id/order/fill/rounding/fee proof"];
const HIP4_DISCOVERY_SOURCES: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::outcome_meta",
    "nautilus_hyperliquid::http::models::OutcomeMeta",
    "nautilus_hyperliquid::http::parse::parse_outcome_instruments",
];
const HIP4_OFFICIAL_DOCUMENTATION_SOURCES: &[&str] =
    &["Hyperliquid Info endpoint outcome metadata `outcomeMeta`"];
const HIP4_MISSING_SUBMIT_PROOF: &[&str] =
    &["HIP-4 outcome order/fill/rounding/fee/settlement/userOutcome proof"];

const HYPERLIQUID_PRODUCT_MATRIX: &[HyperliquidProductMatrixEntry] = &[
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::StandardPerps,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: STANDARD_PERPS_DISCOVERY_SOURCES,
        official_documentation_sources: STANDARD_PERPS_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::FailClosed,
        missing_submit_proof: STANDARD_PERPS_MISSING_SUBMIT_PROOF,
    },
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::Spot,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: SPOT_DISCOVERY_SOURCES,
        official_documentation_sources: SPOT_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::FailClosed,
        missing_submit_proof: SPOT_MISSING_SUBMIT_PROOF,
    },
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::Hip3BuilderPerps,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: HIP3_DISCOVERY_SOURCES,
        official_documentation_sources: HIP3_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::FailClosed,
        missing_submit_proof: HIP3_MISSING_SUBMIT_PROOF,
    },
    HyperliquidProductMatrixEntry {
        provider_key: KEY,
        product_surface: HyperliquidProductSurface::Hip4Outcomes,
        discovery_status: HyperliquidDiscoveryStatus::Supported,
        discovery_sources: HIP4_DISCOVERY_SOURCES,
        official_documentation_sources: HIP4_OFFICIAL_DOCUMENTATION_SOURCES,
        live_submit_status: HyperliquidSubmitStatus::FailClosed,
        missing_submit_proof: HIP4_MISSING_SUBMIT_PROOF,
    },
];

pub fn hyperliquid_product_matrix() -> &'static [HyperliquidProductMatrixEntry] {
    HYPERLIQUID_PRODUCT_MATRIX
}

pub const USER_FEES_OFFICIAL_INFO_REQUEST_WEIGHT: u32 = 20;
pub const USER_FEES_OFFICIAL_RATE_LIMIT_SOURCE: &str =
    "Hyperliquid Docs: Rate limits and user limits - all other documented info requests weight 20";
pub const USER_FEES_NT_CALLERS: &[&str] = &[
    "nautilus_hyperliquid::http::query::InfoRequest::user_fees",
    "nautilus_hyperliquid::http::client::InnerHyperliquidHttpClient::info_user_fees",
    "nautilus_hyperliquid::http::client::HyperliquidHttpClient::info_user_fees",
    "nautilus_hyperliquid::python::http::HyperliquidHttpClient::py_info_user_fees",
];

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidUserFeesRequestWeightStatus {
    OfficialWeightAccounted,
    FailClosedPinnedNtWeightMismatch,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct HyperliquidUserFeesRequestWeightPolicy {
    pub request_type: &'static str,
    pub official_info_request_weight: u32,
    pub pinned_nt_info_base_weight: u32,
    pub status: HyperliquidUserFeesRequestWeightStatus,
    pub official_documentation_source: &'static str,
    pub nt_callers: &'static [&'static str],
}

pub fn hyperliquid_user_fees_request_weight_policy() -> HyperliquidUserFeesRequestWeightPolicy {
    let request = nautilus_hyperliquid::http::query::InfoRequest {
        request_type: nautilus_hyperliquid::common::enums::HyperliquidInfoRequestType::UserFees,
        params: nautilus_hyperliquid::http::query::InfoRequestParams::None,
    };
    let pinned_nt_info_base_weight =
        nautilus_hyperliquid::http::rate_limits::info_base_weight(&request);
    let status = if pinned_nt_info_base_weight == USER_FEES_OFFICIAL_INFO_REQUEST_WEIGHT {
        HyperliquidUserFeesRequestWeightStatus::OfficialWeightAccounted
    } else {
        HyperliquidUserFeesRequestWeightStatus::FailClosedPinnedNtWeightMismatch
    };
    HyperliquidUserFeesRequestWeightPolicy {
        request_type: "userFees",
        official_info_request_weight: USER_FEES_OFFICIAL_INFO_REQUEST_WEIGHT,
        pinned_nt_info_base_weight,
        status,
        official_documentation_source: USER_FEES_OFFICIAL_RATE_LIMIT_SOURCE,
        nt_callers: USER_FEES_NT_CALLERS,
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HyperliquidSecretsConfig {
    pub private_key_ssm_path: String,
    pub account_address_ssm_path: String,
    pub vault_address_ssm_path: Option<String>,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedBoltV3HyperliquidSecrets {
    pub private_key: Zeroizing<String>,
    pub account_address: Zeroizing<String>,
    pub vault_address: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for ResolvedBoltV3HyperliquidSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3HyperliquidSecrets")
            .field("private_key", &REDACTED)
            .field("account_address", &REDACTED)
            .field("vault_address", &REDACTED)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3HyperliquidSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn redaction_values(&self) -> Vec<&str> {
        let mut values = vec![self.private_key.as_str(), self.account_address.as_str()];
        if let Some(vault_address) = &self.vault_address {
            values.push(vault_address.as_str());
        }
        values
    }

    fn exclusive_signer_owner(&self) -> Option<ProviderExclusiveSignerOwner> {
        let mut hasher = Sha256::new();
        hasher.update(b"bolt-v3-hyperliquid-signer-owner-v1:");
        let private_key = self.private_key.as_str();
        let normalized = private_key
            .strip_prefix("0x")
            .or_else(|| private_key.strip_prefix("0X"))
            .unwrap_or(private_key);
        for &byte in normalized.as_bytes() {
            hasher.update([byte.to_ascii_lowercase()]);
        }
        Some(ProviderExclusiveSignerOwner {
            provider_key: KEY,
            fingerprint: hex::encode(hasher.finalize()),
        })
    }
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.data.is_none() && client.execution.is_none() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) must declare a proven [data] or [execution] block before Hyperliquid can be used"
        ));
    }
    if client.data.is_some() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) data mapping is not enabled in this slice"
        ));
    }
    if client.execution.is_some() && client.secrets.is_none() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) has [execution] configured but is missing the [secrets] block"
        ));
    }
    let parsed_execution = if let Some(execution) = &client.execution {
        match execution.clone().try_into::<HyperliquidExecutionConfig>() {
            Ok(parsed) => {
                errors.extend(validate_execution_config(key, &parsed));
                Some(parsed)
            }
            Err(message) => {
                errors.push(format!("clients.{key}.execution: {message}"));
                None
            }
        }
    } else {
        None
    };
    let parsed_secrets = if let Some(secrets) = &client.secrets {
        if client.execution.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [secrets] but no [execution] block is configured; \
                 Hyperliquid [secrets] are only allowed alongside the execution adapter that consumes them"
            ));
        }
        errors.extend(validate_no_raw_secret_fields(key, secrets));
        match secrets.clone().try_into::<HyperliquidSecretsConfig>() {
            Ok(parsed) => {
                errors.extend(validate_secret_paths(key, &parsed));
                Some(parsed)
            }
            Err(message) => {
                errors.push(format!("clients.{key}.secrets: {message}"));
                None
            }
        }
    } else {
        None
    };
    if let (Some(execution), Some(secrets)) = (&parsed_execution, &parsed_secrets) {
        errors.extend(validate_execution_secret_compatibility(
            key, execution, secrets,
        ));
    }
    errors
}

fn validate_no_raw_secret_fields(key: &str, secrets: &toml::Value) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(table) = secrets.as_table() else {
        return errors;
    };
    for field in RAW_SECRET_FIELD_NAMES {
        if table.contains_key(*field) {
            errors.push(format!(
                "clients.{key}.secrets.{field} contains raw secret material; configure {field}_ssm_path and resolve through AWS SSM"
            ));
        }
    }
    errors
}

fn validate_execution_config(key: &str, execution: &HyperliquidExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if execution.product_surfaces.is_empty() {
        errors.push(format!(
            "clients.{key}.execution.product_surfaces must select at least one Hyperliquid product surface"
        ));
    }
    if let Some(approval_id) = &execution.live_submit_approval_id
        && approval_id.trim().is_empty()
    {
        errors.push(format!(
            "clients.{key}.execution.live_submit_approval_id must be non-empty when configured"
        ));
    }
    if execution.live_submit_approval_id.is_some() {
        errors.extend(validate_user_fees_request_weight_policy(key));
    }
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_secs", execution.http_timeout_secs),
        ("max_retries", execution.max_retries),
        ("retry_delay_initial_ms", execution.retry_delay_initial_ms),
        ("retry_delay_max_ms", execution.retry_delay_max_ms),
        ("ws_post_timeout_secs", execution.ws_post_timeout_secs),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "clients.{key}.execution.{field} must be a positive integer"
            ));
        }
    }
    if execution.retry_delay_initial_ms > execution.retry_delay_max_ms {
        errors.push(format!(
            "clients.{key}.execution.retry_delay_initial_ms ({}) must be <= retry_delay_max_ms ({})",
            execution.retry_delay_initial_ms, execution.retry_delay_max_ms
        ));
    }
    if execution
        .product_surfaces
        .contains(&HyperliquidProductSurface::Hip4Outcomes)
        && execution.outcome_settlement_poll_secs == 0
    {
        errors.push(format!(
            "clients.{key}.execution.outcome_settlement_poll_secs must be positive when HIP-4 outcomes are enabled"
        ));
    }
    if let Some(latency_profile) = &execution.latency_profile {
        errors.extend(validate_latency_profile_config(key, latency_profile));
    }
    errors
}

fn validate_user_fees_request_weight_policy(key: &str) -> Vec<String> {
    let policy = hyperliquid_user_fees_request_weight_policy();
    if policy.status == HyperliquidUserFeesRequestWeightStatus::OfficialWeightAccounted {
        return Vec::new();
    }
    vec![format!(
        "clients.{key}.execution.live_submit_approval_id cannot enable Hyperliquid live submit while pinned NautilusTrader {} info request weight is {} but the official documented weight is {}; update the NT pin or the provider rate-limit policy before live submit",
        policy.request_type, policy.pinned_nt_info_base_weight, policy.official_info_request_weight
    )]
}

fn validate_latency_profile_config(
    key: &str,
    latency_profile: &HyperliquidLatencyProfileConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if latency_profile.local_info_node_url.trim().is_empty()
        || !(latency_profile.local_info_node_url.starts_with("http://")
            || latency_profile.local_info_node_url.starts_with("https://"))
    {
        errors.push(format!(
            "clients.{key}.execution.latency_profile.local_info_node_url must be a non-empty HTTP(S) URL"
        ));
    }
    if latency_profile.placement_profile.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.execution.latency_profile.placement_profile must be non-empty"
        ));
    }
    if latency_profile.measurement_artifact_path.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.execution.latency_profile.measurement_artifact_path must be non-empty"
        ));
    }
    errors
}

fn validate_execution_secret_compatibility(
    key: &str,
    execution: &HyperliquidExecutionConfig,
    secrets: &HyperliquidSecretsConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    if matches!(execution.execution_mode, HyperliquidExecutionMode::Vault)
        && secrets.vault_address_ssm_path.is_none()
    {
        errors.push(format!(
            "clients.{key}.execution.execution_mode `vault` requires vault_address_ssm_path in [secrets]"
        ));
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &HyperliquidSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let required_path_fields: &[(&str, &str)] = &[
        ("private_key_ssm_path", &secrets.private_key_ssm_path),
        (
            "account_address_ssm_path",
            &secrets.account_address_ssm_path,
        ),
    ];
    for (field, value) in required_path_fields {
        errors.extend(crate::bolt_v3_validate::validate_ssm_parameter_path(
            key, field, value,
        ));
    }
    if let Some(vault_address_ssm_path) = &secrets.vault_address_ssm_path {
        errors.extend(crate::bolt_v3_validate::validate_ssm_parameter_path(
            key,
            "vault_address_ssm_path",
            vault_address_ssm_path,
        ));
    }
    errors
}

pub fn resolve_secrets(
    context: ProviderSecretResolveContext<'_>,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<ResolvedClientSecrets, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    let private_key = resolve_field(
        context.client_key,
        "private_key_ssm_path",
        context.region,
        &secrets.private_key_ssm_path,
        resolver,
    )?;
    validate_private_key_shape(context.client_key, &private_key)?;
    let account_address = resolve_field(
        context.client_key,
        "account_address_ssm_path",
        context.region,
        &secrets.account_address_ssm_path,
        resolver,
    )?;
    validate_evm_address_shape(
        context.client_key,
        "account_address_ssm_path",
        &account_address,
    )?;
    let vault_address = match &secrets.vault_address_ssm_path {
        Some(path) => {
            let value = resolve_field(
                context.client_key,
                "vault_address_ssm_path",
                context.region,
                path,
                resolver,
            )?;
            validate_evm_address_shape(context.client_key, "vault_address_ssm_path", &value)?;
            Some(Zeroizing::new(value))
        }
        None => None,
    };
    Ok(Arc::new(ResolvedBoltV3HyperliquidSecrets {
        private_key: Zeroizing::new(private_key),
        account_address: Zeroizing::new(account_address),
        vault_address,
    }))
}

pub fn configured_secret_paths(
    context: ProviderSecretResolveContext<'_>,
) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    let mut paths = vec![
        ProviderSsmPathReference {
            field_name: "private_key_ssm_path",
            ssm_path: secrets.private_key_ssm_path,
        },
        ProviderSsmPathReference {
            field_name: "account_address_ssm_path",
            ssm_path: secrets.account_address_ssm_path,
        },
    ];
    if let Some(vault_address_ssm_path) = secrets.vault_address_ssm_path {
        paths.push(ProviderSsmPathReference {
            field_name: "vault_address_ssm_path",
            ssm_path: vault_address_ssm_path,
        });
    }
    Ok(paths)
}

fn parse_secrets_config(
    context: &ProviderSecretResolveContext<'_>,
) -> Result<HyperliquidSecretsConfig, BoltV3SecretError> {
    let secrets_value = context
        .client
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "secrets".to_string(),
            source: "missing [secrets] block".to_string(),
        })?;
    secrets_value
        .clone()
        .try_into()
        .map_err(|error: toml::de::Error| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: KEY.to_string(),
            source: format!("invalid hyperliquid secrets schema: {error}"),
        })
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    if context.client.data.is_some() {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: context.client_key.to_string(),
            field: "data",
            message: format!("provider {KEY} data mapping is not enabled in this slice"),
        });
    }
    let execution = match &context.client.execution {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3ExecutionClientAdapterConfig {
                factory: Box::new(HyperliquidExecutionClientFactory),
                config: Box::new(map_execution(&context, value, secrets)?),
            })
        }
        None => None,
    };
    Ok(BoltV3ClientAdapterConfig {
        data: None,
        execution,
    })
}

fn map_execution(
    context: &ProviderAdapterMapContext<'_>,
    value: &toml::Value,
    secrets: &ResolvedBoltV3HyperliquidSecrets,
) -> Result<HyperliquidExecFactoryConfig, BoltV3AdapterMappingError> {
    let cfg: HyperliquidExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: context.client_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
    validate_surface_live_submit_approval(context.client_key, &cfg, context)?;
    let max_retries =
        u32::try_from(cfg.max_retries).map_err(|_| BoltV3AdapterMappingError::NumericRange {
            client_key: context.client_key.to_string(),
            field: "execution.max_retries",
            message: format!(
                "value {} does not fit in u32 expected by NT",
                cfg.max_retries
            ),
        })?;
    Ok(HyperliquidExecFactoryConfig {
        trader_id: context.root.trader_id,
        account_id: cfg.account_id,
        config: HyperliquidExecClientConfig {
            private_key: Some(secrets.private_key.as_str().to_owned()),
            vault_address: secrets
                .vault_address
                .as_ref()
                .map(|vault_address| vault_address.as_str().to_owned()),
            account_address: Some(secrets.account_address.as_str().to_owned()),
            base_url_ws: Some(cfg.base_url_ws),
            base_url_http: Some(cfg.base_url_http),
            base_url_exchange: Some(cfg.base_url_exchange),
            proxy_url: cfg.proxy_url,
            environment: nt_environment(cfg.environment),
            http_timeout_secs: cfg.http_timeout_secs,
            max_retries,
            retry_delay_initial_ms: cfg.retry_delay_initial_ms,
            retry_delay_max_ms: cfg.retry_delay_max_ms,
            normalize_prices: cfg.normalize_prices,
            market_order_slippage_bps: cfg.market_order_slippage_bps,
            transport_backend: cfg.transport_backend,
            ws_post_timeout_secs: cfg.ws_post_timeout_secs,
            outcome_settlement_poll_secs: cfg.outcome_settlement_poll_secs,
        },
    })
}

fn validate_surface_live_submit_approval(
    client_key: &str,
    cfg: &HyperliquidExecutionConfig,
    context: &ProviderAdapterMapContext<'_>,
) -> Result<(), BoltV3AdapterMappingError> {
    let [configured_surface] = cfg.product_surfaces.as_slice() else {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.product_surfaces",
            message:
                "Hyperliquid live submit requires exactly one product surface per execution client"
                    .to_string(),
        });
    };
    if *configured_surface == HyperliquidProductSurface::Hip4Outcomes
        && cfg.outcome_settlement_poll_secs == 0
    {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.outcome_settlement_poll_secs",
            message: "HIP-4 outcomes live submit requires positive settlement polling".to_string(),
        });
    }
    let Some(expected_approval_id) = cfg.live_submit_approval_id.as_deref() else {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.live_submit_approval_id",
            message: format!(
                "{} live submit requires a configured consumed live-submit approval id",
                product_surface_name(*configured_surface)
            ),
        });
    };
    let Some(consumed) = context.runtime_approvals.live_submit else {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.live_submit_approval_id",
            message: format!(
                "{} live submit requires a consumed live-submit approval",
                product_surface_name(*configured_surface)
            ),
        });
    };
    let Some(consumed) = consumed.downcast_ref::<HyperliquidLiveSubmitApprovalConsumption>() else {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.live_submit_approval_id",
            message: "consumed live-submit approval has an unsupported provider payload"
                .to_string(),
        });
    };
    if consumed.approval_id() != expected_approval_id {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.live_submit_approval_id",
            message: "consumed live-submit approval id does not match configured approval id"
                .to_string(),
        });
    }
    if consumed.product_surface() != *configured_surface {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.product_surfaces",
            message: "consumed live-submit approval product surface does not match configured product surface"
                .to_string(),
        });
    }
    Ok(())
}

fn product_surface_name(surface: HyperliquidProductSurface) -> &'static str {
    match surface {
        HyperliquidProductSurface::StandardPerps => "standard_perps",
        HyperliquidProductSurface::Spot => "spot",
        HyperliquidProductSurface::Hip3BuilderPerps => "hip3_builder_perps",
        HyperliquidProductSurface::Hip4Outcomes => "hip4_outcomes",
    }
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3HyperliquidSecrets, BoltV3AdapterMappingError> {
    match resolved.clients.get(client_key) {
        Some(inner) => inner.as_any().downcast_ref().ok_or_else(|| {
            BoltV3AdapterMappingError::SecretProviderMismatch {
                client_key: client_key.to_string(),
                expected_provider_key: KEY,
            }
        }),
        None => Err(BoltV3AdapterMappingError::MissingResolvedSecrets {
            client_key: client_key.to_string(),
            expected_provider_key: KEY,
        }),
    }
}

fn nt_environment(value: HyperliquidEnvironment) -> NtHyperliquidEnvironment {
    match value {
        HyperliquidEnvironment::Mainnet => NtHyperliquidEnvironment::Mainnet,
        HyperliquidEnvironment::Testnet => NtHyperliquidEnvironment::Testnet,
    }
}

fn validate_private_key_shape(
    client_key: &str,
    private_key: &str,
) -> Result<(), BoltV3SecretError> {
    nautilus_hyperliquid::common::credential::EvmPrivateKey::new(private_key)
        .map(|_| ())
        .map_err(|error| BoltV3SecretError {
            client_key: client_key.to_string(),
            field: "private_key_ssm_path".to_string(),
            source: format!("resolved Hyperliquid private key is not accepted by the pinned NautilusTrader Hyperliquid adapter: {error}"),
        })
}

fn validate_evm_address_shape(
    client_key: &str,
    field: &'static str,
    value: &str,
) -> Result<(), BoltV3SecretError> {
    let valid = value
        .strip_prefix("0x")
        .filter(|rest| {
            rest.len() == 40
                && rest.chars().all(|character| character.is_ascii_hexdigit())
                && !rest.chars().all(|character| character == '0')
        })
        .is_some();
    if valid {
        Ok(())
    } else {
        Err(BoltV3SecretError {
            client_key: client_key.to_string(),
            field: field.to_string(),
            source: "resolved Hyperliquid EVM address is not a non-zero 20-byte hex address"
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use nautilus_hyperliquid::common::{
        credential::credential_env_vars, enums::HyperliquidEnvironment,
    };

    use super::FORBIDDEN_ENV_VARS;

    #[test]
    fn forbidden_env_vars_cover_nt_hyperliquid_credential_env_vars() {
        for environment in [
            HyperliquidEnvironment::Mainnet,
            HyperliquidEnvironment::Testnet,
        ] {
            let (private_key, vault_address) = credential_env_vars(environment);
            assert!(FORBIDDEN_ENV_VARS.contains(&private_key));
            assert!(FORBIDDEN_ENV_VARS.contains(&vault_address));
        }
        assert!(FORBIDDEN_ENV_VARS.contains(&"HYPERLIQUID_ACCOUNT_ADDRESS"));
    }
}
